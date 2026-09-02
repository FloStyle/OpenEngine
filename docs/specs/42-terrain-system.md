---
spec: "42-terrain-system"
phase: "Phase 5: Environment & AI"
status: "draft"
author: "OpenEngine AI"
created: "2026-09-03"
depends_on:
  - "00-ecs-architecture"
  - "02-asset-pipeline"
  - "04-render-pipeline"
  - "16-serialization"
  - "21-primitive-components"
  - "22-edit-vs-play"
  - "23-undo-redo"
  - "24-editor-viewport"
  - "43-vegetation-system"
---
# 42 - Terrain System

## Overview

Terrain is the large-scale, height-driven ground surface of a world. It is a
streamed **grid of heightmap chunks** with continuous **LOD**, **material
splatting** across multiple paint **layers**, an undoable **sculpt/paint** tool
set, deterministic **heightfield collision**, and a **vegetation placement**
hookup to spec 43.

The hard architectural split is the one this repository lives by (AGENTS.md § 1,
§ 3, § 4):

* **Generation, sculpt math, collision, and vegetation placement are
  deterministic.** Any function that decides *where the ground is* or *where a
  thing goes on the ground* runs in Domain B as a pure system — fixed-point only
  (`openengine-math::I16F16`), seeded, `HashMap`-free, and it returns its result
  as a [`WorldDelta`] / a set of placement entities. Same heightmap, same seed,
  same tick → same world.
* **Rendering, mesh tessellation, LOD culling, and GPU upload of the splat are
  Domain A presentation** (`glam` `f32` at the boundary only). They never feed
  back into gameplay math.
* **Editor sculpt/paint are Domain A tools whose *mutations* are undoable
  `Command`s** (spec 23). The tool *gesture* runs in the viewport (spec 24, GPU
  ray-cast onto the heightfield) but every committed sample change is translated
  into a `TerrainEditCommand` that produces a [`WorldDelta`], which is then
  applied to the *edit world* only (spec 22).

This spec owns the heightmap substrate, splat layers, collision, and the
sculpt/paint channel; it hands the resulting surface (via a `Terrain` /
`TerrainLayer` archetype + a deterministic placement-request system) to spec 43
for foliage density queries and instance placement. Spec 45 (navigation)
consumes the same heightfield to build the walkable cost field.

## Core Concepts

### Chunk grid + a single persistent height column

Terrain is not one giant entity; it is a **root `Terrain` entity** (one per
scene, carries global extents/resolution/seed) plus one **child chunk column
entity per LOD-resolved grid cell**. The height data itself is *not* stored
inline in a component: a heightmap cell is a dense 2-D grid (e.g. 129×129
`I16F16` samples = ~67 KB per patch) that is far too large to live in a fixed
`Pod` component row and far too large to copy per-tick. Instead:

1. The **authoritative, deterministic height column** lives in the *world
   resource store* (Domain A `TerrainData`, a host-owned `Vec<I16F16>` per chunk,
   keyed by `(chunk_x, chunk_z)`), which is also the thing serialized by the spec
   16 codec.
2. Domain B never sees that `Vec` as a raw mutable buffer. It sees a
   **read-only, sampled `HeightView`** handed in through the system input
   (below): a fixed-resolution array plus a chunk origin and spacing. Domain B
   does bilinear height lookups against it with pure fixed-point math. This keeps
   the guest deterministic *and* memory-safe: it reads a borrowed slice it cannot
   mutate, exactly like the [`StateView::arena`] pattern of spec 00.
3. **Edits change the store through deltas, not in place in Domain B.** A sculpt
   writes a height `ColumnWrite` targeting the chunk's sample column descriptor
   (a synthetic per-chunk component whose "row" is the sample index — see
   Constraints on representation). The host applies it to `TerrainData` at the
   flush boundary.

This mirrors the whole engine: Domain B *describes* the next heights, Domain A
*materializes* them into GPU vertex buffers and the collision store.

### Height sample, world position, and the ground function

```rust
/// Bilinear height query on one chunk patch. Pure, fixed-point.
/// `fx`-spaced grid: x/z span [0, size]; sample (sx,sz) sits at
/// (origin + (sx*spacing, sample(sx,sz), sz*spacing)).
fn height_at(
    patch: &HeightPatch,        // read-only sample grid (Domain-B safe view)
    world_x: I16F16,
    world_z: I16F16,
) -> I16F16
```

`height_at` is the single ground-truth function everything else (placement,
collision, nav) calls. It must be monotone and cheap so Domain B systems can
call it in tight loops.

### Deterministic procedural generation

A chunk whose samples are un-authored (or regenerating) is filled by a seeded,
pure generator (Perlin-style value noise over integer hashes — `Hash64(seed,
chunk_x, chunk_z, …)`, no `f32`, no ambient RNG). Same `Terrain.seed`, same
`(chunk_x, chunk_z)`, same octave/amplitudes ⇒ bit-identical samples on every
platform. Regeneration is a pure system that returns a height `ColumnWrite`
(and, for vegetation, emits the placement request handled in spec 43).

### LOD

Each chunk is *conceptually* one resolution; the renderer derives a mesh at a
LOD level from the same sample array (Domain A decimation). LOD selection per
chunk is a pure Domain-B decision (`lod_for_distance`) so headless tests can
predict culling without a GPU, but actual triangle generation is Domain A.
Geomorphing (vertex blending between LOD levels) is handled host-side in the
vertex shader using a per-vertex morph factor; it is presentation, never logic.

### Material splatting & `TerrainLayer`

Ground appearance is a set of **layers**, each an authored material (albedo +
a control signal). Layering uses a lightweight form of splatting suited to this
repo's determinism rule:

* Each `TerrainLayer` is its own *entity* (so an arbitrary number coexist) that
  references the owning `Terrain` and a logical material `AssetRef` (spec 21
  discipline — no absolute paths).
* **Where** a layer is strong is itself authored heightfield data: a small
  `layer_weights` buffer (per sample) is stored in the same Domain-A resource
  store and read by Domain B when paint tools / systems must query coverage.
  Paint (below) edits these weights through the same Command path as heights.
* At render time Domain A blends up to N layers per fragment via the splat
  weights; `f32` and the blend all happen on the GPU. Domain B only ever reasons
  about the integer/fixed weight column.

### Sculpt & paint tools (Domain A gesture, undoable Command)

Editing is the spec-23 command pattern extended to heightfields:

```rust
pub struct TerrainEditCommand {
    pub terrain: Entity,                 // owning Terrain root
    pub chunk: (u32, u32),               // target chunk
    pub kind: TerrainStampKind,          // Raise|Lower|Smooth|Flatten|PaintLayer
    pub radius: I16F16,                  // stamp radius in world units
    pub strength: I16F16,                // 0..1 falloff amount
    pub center: (I16F16, I16F16),        // stamp center (world x/z)
    pub layer: Option<Entity>,           // Some for PaintLayer
    pub old_samples: Vec<I16F16>,        // captured pre-stamp (for undo)
    pub new_samples: Vec<I16F16>,        // computed post-stamp (for redo)
}
```

`Command` (spec 23) contract: `execute()` returns the forward `ColumnWrite`
(`new_samples`); `undo()` returns the inverse (`old_samples`). Because samples
are captured as raw fixed bytes (`Vec<I16F16>` — `Pod`), undo is exact and never
re-derives from `f32`. The manager holds both stacks; gesture batching
(transaction collapse, spec 23) means one drag = one undo entry.

**Where the height *actually* gets edited**: the Domain-A viewport tool ray-casts
onto the GPU mesh purely to find the *hit point* (presentation). Once the agent
commits the stamp, the tool *does not* write the store directly. It:
1. reads the current samples for the affected samples,
2. computes `new_samples` with a **Domain-B, seeded, fixed** stamp kernel
   (the sculpt math itself is pure and testable headlessly; no `f32` falloff),
3. emits a `TerrainEditCommand` to the `UndoRedoManager`,
4. the returned [`WorldDelta`] is applied to the edit world (spec 22) and the
   host applies the height `ColumnWrite` into `TerrainData` at flush.

This is the one sanctioned place a height changes. There is no second writer.

### Heightfield collision & placement hookup

Domain B systems (physics spec 13 raycast; AI spec 44 ground tests; spec 43
foliage) read `height_at`. A dedicated **placement/vegetation hookup system** in
spec 43 queries this module's height field plus a deterministic
`terrain_biome(x,z)` function to pick foliage spots. Terrain exposes:
`fn sample_biome(view, x, z) -> (I16F16 height, u32 biome)` where `biome` is a
fixed integer cell from the seed — deterministic.

## Key Rust Types

```rust
//! crates/logic-terrain (Domain B, no_std) — pure systems.
//! Mirror types declared here; crossing types are `#[repr(C)] Pod + Zeroable`.

use openengine_math::I16F16;
use contracts::{ComponentId, Entity};

/// Read-only view of one height patch handed to a pure system. Borrows the
/// Domain-A sample store; never mutated by the guest.
pub struct HeightPatch<'a> {
    pub origin_x: I16F16,
    pub origin_z: I16F16,
    pub spacing: I16F16,
    pub size: u32,                 // samples per edge (== resolution)
    pub heights: &'a [I16F16],     // row-major, size*size
}

/// Deterministic heightfield query entry point for the whole terrain.
pub struct TerrainView<'a> {
    pub root: Terrain,             // copy of the root component (Pod)
    pub patch: HeightPatch<'a>,    // the LOD-0 authoritative patch in view
}

pub struct TerrainBiome {
    pub height: I16F16,
    pub biome: u32,
}

/// Pure system: lazily (re)generate a chunk's samples when dirtied/unseeded.
pub fn terrain_regenerate_system(
    view: &StateView<'_>,
    terrain: &Terrain,
    chunk: (u32, u32),
) -> Result<WorldDelta, RecoverableError> {
    // pure noise -> Vec<I16F16> new_samples from seed
    // -> single ColumnWrite height patch (or a DeferredCommand::Emit
    //    "height_patch_ready" for Domain A to upload)
}

/// Pure LOD decision: which detail a chunk needs at a distance.
pub fn lod_for_distance(dist: I16F16) -> LOD;

/// Pure ground + biome sampler used by foliage/nav.
pub fn sample_biome(view: &TerrainView<'_>, x: I16F16, z: I16F16) -> TerrainBiome;
```

```rust
//! crates/terrain-data (Domain A) — host resource store + GPU feed.
/// Authoritative, serializable height/weight storage.
pub struct TerrainChunkStore {
    pub chunks: BTreeMap<(u32, u32), TerrainChunk>, // stable key order
}
pub struct TerrainChunk {
    pub heights: Vec<I16F16>,       // size*size
    pub layer_weights: Vec<u8>,     // size*size*num_layers, or per-layer map
    pub lod_requests: Vec<LOD>,     // per-viewport last request
}
```

```rust
//! crates/editor — Domain A tools + commands (spec 23/24).
pub struct SculptTool { pub radius: I16F16, pub strength: I16F16, pub kind: TerrainStampKind }
pub enum TerrainStampKind { Raise, Lower, Smooth, Flatten, PaintLayer }
// TerrainEditCommand as above; viewport ray -> hit point -> stamp commit.
```

## Components

| `ComponentId` | Name          | Domain use (owner)                            |
|---------------|---------------|-----------------------------------------------|
| **30**        | `Terrain`     | Root: extents, resolution, spacing, `seed`.   |
| **31**        | `TerrainLayer`| One splat layer referencing owner terrain.    |

IDs 30–31 are **frozen** here (never reassigned; see spec 21 ID policy). The
per-chunk height sample column is **not** a registered gameplay component id —
it is a host-managed resource column synthesized for `ColumnWrite` transport so
row memory stays small. Any future registered terrain components (e.g. a
`TerrainBrushSettings`) must be appended in the 40–49 reserved window of this
phase and recorded here; **do not** reuse 32–39 (owned by specs 43/44/45).

```rust
/// Root terrain descriptor (Pod, fixed-point). ComponentId 30.
#[repr(C)]
#[derive(Clone, Copy, bytemuck::Pod, bytemuck::Zeroable,
         serde::Serialize, serde::Deserialize, Debug)]
pub struct Terrain {
    pub origin_x: I16F16,
    pub origin_z: I16F16,
    pub size: I16F16,             // full world extent on each axis
    pub chunk_resolution: u32,    // samples per chunk edge (e.g. 129)
    pub chunk_world_size: I16F16, // world units per chunk edge
    pub seed: u64,                // deterministic generation seed
    pub _pad: [u32; 1],
}

/// One splat material layer. Lives as its own entity (so many coexist).
/// ComponentId 31.
#[repr(C)]
#[derive(Clone, Copy, bytemuck::Pod, bytemuck::Zeroable,
         serde::Serialize, serde::Deserialize, Debug)]
pub struct TerrainLayer {
    pub terrain: Entity,          // owning Terrain root
    pub material: AssetRef,       // logical relative-path material token
    pub index: u16,               // stable layer ordering for blend priority
    pub weight_scale: I16F16,     // authored global strength (0..1)
    pub _pad: u16,
}
```

`Terrain` root and `TerrainLayer` are `Parent`-linked (spec 21) — layers are
children of the root terrain entity, `TerrainLayer.terrain` mirrors the parent.

## Constraints

- **Domain split is hard.** Heightmath (generation, `height_at`, `sample_biome`,
  LOD decision, stamp kernel) is Domain B: `#![no_std]`, fixed-point `I16F16`,
  no `f32`, no `HashMap`, no wall clock, seeded. Rendering, tessellation,
  splatting blend, LOD mesh build, and viewport ray are Domain A presentation;
  their `f32` never enters gameplay data.
- **No raw height arrays as gameplay components.** Dense samples live in the
  Domain-A store; Domain B only borrows a read-only `HeightPatch`/`TerrainView`
  and returns deltas. This respects spec 00's "guest reads, host writes" and
  keeps columns small.
- **One writer.** Height and layer weights change only through the spec-23
  Command path applied to the edit world. No Domain-A tool writes the store
  directly; no Domain-B system mutates it in place.
- **Deterministic regeneration & sculpt.** Same seed + same chunk + same tick ⇒
  same samples. Stamps captured as raw fixed `Vec<I16F16>` old/new — undo never
  re-derives from `f32`.
- **Edit vs Play.** Sculpt/paint operate on the **edit world** (spec 22). Play
  deep-clones the edited store; in-Play terrain edits (if any later land as
  gameplay) are a separate Domain-B feature, never the editor tool.
- **Vegetation & nav depend on the same ground.** Spec 43 places foliage and
  spec 45 builds cost fields from `height_at`/`sample_biome` so there is one
  ground-truth height, not two.
- **Serialization.** `TerrainChunkStore` round-trips through the spec-16 codec;
  chunks are keyed by sorted `(u32,u32)` so serialization order is deterministic.
- **Portability.** No absolute paths (materials via logical `AssetRef`),
  compiles on `x86_64-linux` + `aarch64-linux`, no GPU in logic tests.
- **LOD is presentation.** `lod_for_distance` may be pure/tested, but LOD never
  alters authoritative samples; it only picks a decimated *view* of them.

## Performance Targets

- `height_at` single query: **≤ ~30 ns** amortized (index into a cached patch,
  one fixed lerp path).
- Chunk regeneration (129×129 = 16 641 samples, seeded noise): **< 1 ms**
  per chunk in Domain B; budgeted ≤ 16 ms/tick for a *limited* number of chunks
  regenerated per tick (incremental restreaming).
- Sculpt stamp over a radius (falloff kernel, pure): **< 0.5 ms** for a
  64-world-unit radius at chunk resolution.
- Full frame terrain render overhead (all chunks, tessellation + splatting):
  Domain A target **< 4 ms** on a mid GPU for a 1 km² world at target density.
- Height queries for 10k foliage placements (spec 43): amortized via a coarse
  `biome` cell so per-placement overhead stays negligible.

## Testing Strategy

All headless (no GPU) in Domain B + editor unit tests:
- **Generation determinism:** regenerate the same chunk 3× on two targets and
  assert byte-identical `Vec<I16F16>` (fixed-point bit-exact, spec 13 protocol).
- **`height_at` correctness:** known flat/ramp patches return exact expected
  heights at sample and mid-sample points; bilinear continuity across chunk
  borders is monotone (no seams).
- **LOD decision:** given distances, `lod_for_distance` picks the predicted LOD
  for a set of golden inputs; pure + repeatable.
- **Sculpt stamp math:** Raise/Lower/Smooth/Flatten produce the exact expected
  fixed samples from a scripted starting patch; Smooth averages neighbors with
  a fixed kernel; boundary samples clamp correctly.
- **Undo/redo round-trip:** apply `TerrainEditCommand.execute` then `.undo` and
  assert the chunk store is bit-identical to pre-edit (spec 23 protocol);
  transaction collapse gives one undo per drag.
- **Edit-vs-play isolation:** sculpt in edit world, deep-clone to play, and
  assert play's store starts identical and that no editor command mutates the
  play twin (spec 22).
- **Serialization:** `TerrainChunkStore` → codec → decode is byte-identical;
  chunk iteration order is the sorted key order.
- **Placement hookup (spec 43 cross-test):** a seeded terrain yields the same
  `sample_biome` list consumed by the foliage test — assert stable across runs.
- **Collision integration:** `height_at` matches what spec 45's nav cost-field
  builder reads for the same chunk.

## Dependencies

- `contracts` (`StateView`, `WorldDelta`, `ColumnWrite`, `DeferredCommand`,
  `Entity`, `ComponentId`, `RecoverableError`).
- `openengine-math` (`I16F16`, seeded integer hashing / value-noise helpers).
- `bytemuck`, `serde`, `postcard`, `alloc`.
- `crates/ecs` (archetype/column), `crates/editor` (commands + viewport tools,
  specs 23/24), `crates/core` + render pipeline (spec 04) for GPU feed.
- `AssetRef` from spec 21 for layer materials.
- Consumed/consumes: **spec 43** (foliage placement on this heightfield),
  **spec 45** (nav from this heightfield). Domain-A `TerrainData` feeds
  `crates/render`.

## Next Steps

1. Add `Terrain` (30) + `TerrainLayer` (31) Pod components and register them.
2. Land `TerrainChunkStore` in Domain A with a serializable layout + codec.
3. Implement seeded `terrain_regenerate_system` + `height_at`/`sample_biome`
   in a `no_std` `crates/logic-terrain`.
4. Define the height `ColumnWrite` transport and host apply path into the store.
5. Implement the pure sculpt stamp kernel and the `TerrainEditCommand`
   (spec 23) + viewport hit → commit flow (spec 24).
6. LOD decision system + Domain-A tessellation/splat feed (render).
7. Wire foliage hookup with spec 43 and nav cost field with spec 45.
