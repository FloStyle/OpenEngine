---
spec: "43-vegetation-system"
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
  - "42-terrain-system"
---
# 43 - Vegetation System

## Overview

Vegetation is the foliage that populates the terrain surface: trees, grass
tufts, rocks, and prop scatter. It is **GPU-instanced** for rendering at scale
while its *placement* is fully deterministic. The system owns:

* **`FoliageType`** — an authored species definition: logical mesh asset,
  density, allowed scale range, and an optional collision flag (interacts with
  spec 13 physics / spec 45 nav as small obstacles).
* **`FoliageInstance`** — one placed instance with an immutable deterministic
  `Transform` (position/rotation/scale), derived from a `FoliageType` id and a
  placement seed. Instances are *never* randomized at runtime; their placement is
  a pure function of seed + authored rules + the terrain height/biome.
* **Brush paint** (add/remove instances) as an undoable Domain-A `Command` set.
* **Procedural placement** — a Domain-B pure system that, given a
  `FoliageType` + region + `seed`, deterministically scatters instances by
  walking the heightfield (`height_at`, spec 42) and applying rules/constraints
  (slope, altitude band, biome mask, spacing/overlap avoidance).
* **LOD** for instanced rendering and optional **vertex-shader wind** (Domain A
  presentation only).

The determinism rule governs everything about *where foliage exists*: a type's
spawn pattern is reproducible from its seed, and the instance set is a pure
function of `(terrain, type rules, seed, region)`. Domain A only turns that
instance set into instanced draw calls; it never adds/removes instances by
itself. Same as terrain sculpting, manual brush edits go through the spec-23
`Command` path on the edit world.

## Core Concepts

### `FoliageType` as an authored definition

A `FoliageType` is a lightweight `Pod` component on a **type entity** (one per
species). It references the render mesh and encodes the placement policy:

```rust
pub struct FoliageType {
    pub name_tag: Tag,             // spec 21 classification label (editor)
    pub mesh: AssetRef,            // logical instanced-mesh token
    pub density: u16,              // instances per unit-area cell (scaled)
    pub min_scale: I16F16,         // unit scale floor
    pub max_scale: I16F16,         // unit scale ceiling (<= e.g. 8.0)
    pub has_collider: u8,          // 0/1: emit a collider (spec 13) / nav obstacle
    pub placement_mask: u8,        // biome mask bitfield (which biomes allow it)
    pub slope_limit: u8,           // max slope in degrees/2 (0..=45)
    pub min_height: I16F16,        // altitude band (world units)
    pub max_height: I16F16,
    pub align_to_slope: u8,        // 0/1
    pub _pad: u8,
}
```

`FoliageType` is not a per-instance blob; it is the shared "spawn settings" row.
It has **no** notion of absolute path — `mesh` is a logical `AssetRef` (spec 21
/ 02), resolved against `OPENENGINE_ASSETS_PATH` at render time only.

### `FoliageInstance` = immutable deterministic placement

A `FoliageInstance` is the actual placed item:

```rust
pub struct FoliageInstance {
    pub foliage_type: Entity,      // which FoliageType entity spawned this
    pub transform: Transform,      // spec 21 canonical 3D (fixed-point)
    pub seed: u64,                 // derived from type.seed + grid cell
}
```

`transform` is **fixed-point** (`I16F16`, spec 21) so instances can be written
through a normal `ColumnWrite` and read deterministically by Domain B (e.g. AI
agents that must not walk through a trunk check the instance's fixed position).
`seed` records *how* this instance was derived; regenerating from the same seed
yields the same `transform`, so a serialized scene never needs a per-instance
random history.

Instances live in a dedicated archetype, e.g. `(Transform, FoliageInstance)`, so
the renderer can read one SoA column and the editor can query instances by type.
Because instances are immutable once placed, spawning them is the only write; the
host never perturbs them outside an explicit (Commanded) edit.

### Deterministic procedural placement

Placement is a **pure Domain-B system**, `foliage_populate_system`. Inputs: a
read-only `TerrainView` (spec 42 `HeightPatch` + biome sampler) and the world's
`FoliageType` entities. It returns a [`WorldDelta`] whose `spawns` carry all
derived `FoliageInstance`s (batched). The algorithm is fully ordered:

```rust
pub fn foliage_populate_system(
    view: &StateView<'_>,
    types: &[FoliageType],
    region: PlacementRegion,     // (chunk_min, chunk_max) world cell range
) -> Result<WorldDelta, RecoverableError>
```

Steps (each uses only integer/fixed arithmetic + a seeded, `HashMap`-free
stream):
1. For each type and each placement cell it covers, derive a deterministic
   count = `density` scaled by `height_at`/biome and by a `Hash64(type.seed ^
   cell_key)` jitter (so per-cell fill varies yet is reproducible).
2. Candidate (x,z) offsets are drawn from a **deterministic permutation** of the
   cell (a seeded shuffler, not ambient `rand`) — no `HashMap` iteration, no
   platform-dependent ordering.
3. Each candidate is **rejected or accepted** by constraints: altitude band
   `[min_height, max_height]`, slope ≤ `slope_limit` (from the heightfield
   gradient — fixed differences), biome mask bit set, and spacing (a minimum
   distance vs. already-accepted instances in a local cell, enforced with a
   bounded neighbor probe, not a global mutable set).
4. Accepted candidates yield a `FoliageInstance` whose `transform` places it on
   the ground (`y = height_at(...)`), scales within `[min_scale, max_scale]` via
   the seed, rotates deterministically from the seed, and optionally aligns to
   the local slope normal.
5. All instances are packed into spawn `ColumnWrite`s (instance `transform`
   columns are filled by the same delta), returned to the host to apply.

**Re-entrancy rule:** the system only *accepts a region into a delta* when the
host marks that region "needs populate" (a `DeferredCommand::Emit`/flag from
spec 42 when a chunk regenerates). Running it twice over the same un-marked
region is a no-op; running it over a newly marked region appends only new
instances. This keeps repeated ticks idempotent and deterministic.

### Brush paint add/remove (Domain A, undoable)

Manual authoring adds or deletes individual instances. Per spec 23 these are
`Command`s on the **edit world**:

```rust
pub enum VegetationBrush {
    Add(FoliageInstance),          // explicit instance (fixed transform)
    Remove(Entity),                // instance handle (generation-guarded)
}
```

The viewport (spec 24) brush picks a `FoliageType`, and on commit builds a
`FoliagePaintCommand`:

```rust
pub struct FoliagePaintCommand {
    pub add: Vec<FoliageInstance>,      // raw Pod fixed transforms
    pub remove: Vec<Entity>,            // instance handles + generations
    pub removed_snapshots: Vec<FoliageInstance>, // captured for undo
    pub description: String,
}
```

`execute()` returns a delta that spawns the adds and despawns the removes;
`undo()` spawns back the `removed_snapshots` and despawns the adds. All raw fixed
bytes — undo never re-derives from `f32`. A paint stroke over many instances is a
single transaction (one undo entry, spec 23). Brush paint *only* affects the edit
world; procedural population is what fills play's deterministic clone.

Because manual adds and procedural adds are both just "instances in the world,"
the two coexist; a **`FoliageInstance.source`** bit (`Procedural | Manual`)
records origin so the editor can offer "prune procedural under a brush" later
without confusing authored instances.

### GPU instancing, LOD, wind

Once instances exist, rendering is Domain A:

* **Instanced draw** — one vertex buffer + one per-instance `Transform` buffer
  built from the `FoliageInstance` column; the GPU emits `N` meshes per type.
  `f32` conversion happens exactly at the `fixed → GPU` emission boundary (spec
  04).
* **LOD** — the instancing pipeline selects per-type detail or distance-fades
  whole clumps; a pure `foliage_lod(dist)` may decide counts, but triangle/LOD
  culling and vertex streams are host renderer work.
* **Wind** — a Domain-A vertex-shader displacement (sine/curl in GPU `f32`)
  animates foliage. Wind **never** affects gameplay positions: gameplay reads the
  static fixed `FoliageInstance.transform`; the renderer only adds a *visual*
  offset. This keeps simulation deterministic and wind purely cosmetic.

### Collision / nav interaction

* If `FoliageType.has_collider`, each instance also carries a collider (spec 13)
  so actors collide with trunks; for nav this makes the instance a static
  obstacle baked into the cost field (spec 45) or excluded from walkable cells.
* Colliders are **deterministic**: they derive from the fixed `transform`, not
  from any animated wind position.

## Key Rust Types

```rust
//! Domain B — crates/logic-foliage (no_std, pure). Types crossing the ABI are
//! #[repr(C)] Pod + Zeroable; placement math is fixed-point only.
use openengine_math::I16F16;
use contracts::{StateView, WorldDelta, Entity, RecoverableError};

pub struct FoliageType { /* as above — ComponentId 32 */ }
pub struct FoliageInstance { /* as above — ComponentId 33 */ }

/// Which world-cell region to populate (inclusive). Ordered (min<=max).
pub struct PlacementRegion { pub min_x: i32, pub min_z: i32,
                             pub max_x: i32, pub max_z: i32 }

/// Seeded integer hash: stable across platforms (no HashMap, no std).
pub fn hash64(seed: u64, key: u64) -> u64;

pub fn foliage_populate_system(/* ... */) -> Result<WorldDelta, RecoverableError>;
pub fn foliage_lod(dist: I16F16) -> FoliageLod;
```

```rust
//! Domain A — crates/foliage-data + editor instancing feed + paint commands.
pub struct InstanceBatch { pub transforms: Vec<Transform>, /* Pod */ }
// FoliagePaintCommand as above (spec 23 Command impl).
```

## Components

| `ComponentId` | Name            | Domain use (owner)                                  |
|---------------|-----------------|-----------------------------------------------------|
| **32**        | `FoliageType`   | Authored species: mesh, density, scale, collider, constraints. |
| **33**        | `FoliageInstance`| One placed instance (fixed Transform + seed + source). |

IDs 32–33 are **frozen** here (spec 21 policy). Terrain 30/31 are spec 42's;
BehaviorTree/Blackboard/AIAgent (34/35/36) and NavMesh/NavAgent/NavObstacle
(37/38/39) are specs 44/45's. Any new vegetation components (e.g.
`FoliageDensitySettings`) append in the 40–49 reserved window, never reuse the
above.

```rust
/// FoliageType — authored species definition. ComponentId 32.
#[repr(C)]
#[derive(Clone, Copy, bytemuck::Pod, bytemuck::Zeroable,
         serde::Serialize, serde::Deserialize, Debug)]
pub struct FoliageType {
    pub name_tag: Tag,             // editor label (spec 21)
    pub mesh: AssetRef,            // logical instanced mesh token
    pub density: u16,              // per-cell fill target
    pub min_scale: I16F16,         // scale floor
    pub max_scale: I16F16,         // scale ceiling
    pub has_collider: u8,
    pub placement_mask: u8,        // biome mask bitfield
    pub slope_limit: u8,           // max slope (deg/2)
    pub min_height: I16F16,
    pub max_height: I16F16,
    pub align_to_slope: u8,
    pub _pad: u8,
}

/// FoliageInstance — one immutable deterministic placement. ComponentId 33.
#[repr(C)]
#[derive(Clone, Copy, bytemuck::Pod, bytemuck::Zeroable,
         serde::Serialize, serde::Deserialize, Debug)]
pub struct FoliageInstance {
    pub foliage_type: Entity,
    pub transform: Transform,      // spec 21 fixed-point 3D placement
    pub seed: u64,                 // how this instance was derived
    pub source: u8,                // 0 = procedural, 1 = manual
    pub _pad: [u8; 3],
}
```

Instances assemble into an archetype of the form
`(Transform, FoliageInstance[, Parent])`, one contiguous SoA column each — ideal
for an instancing renderer reading the `Transform` column directly.

## Constraints

- **Placement is pure & seeded.** Procedural scatter is a Domain-B pure system
  returning a `WorldDelta`; every decision is fixed-point + seeded integer hash.
  No ambient RNG, no `HashMap`, no `std::time`, no GPU in logic tests. Identical
  (terrain, type set, seed, region) ⇒ identical instance set on every platform.
- **Instances are immutable fixed state.** Once placed, a `FoliageInstance`
  transform never changes from wind or LOD. Gameplay/AI reads static fixed
  positions; all animation is a Domain-A GPU vertex offset.
- **One editor writer.** Manual add/remove go through spec-23 `Command`s on the
  edit world; the viewport brush never writes the instance column directly.
- **Regeneration idempotency.** `foliage_populate_system` over an already-filled,
  un-marked region is a no-op; a freshly marked region appends deterministically
  and never duplicates prior runs.
- **Serialization.** Instance and type columns round-trip the spec-16 codec with
  sorted archetype/column order; `source` + `seed` survive so a regenerated scene
  stays identical.
- **No absolute paths.** `mesh` is a logical `AssetRef`; instanced-mesh
  resolution happens host-side against `OPENENGINE_ASSETS_PATH`.
- **Fixed-point transforms.** `Transform` is the spec-21 fixed type; no `f32`
  stored. `f32` appears only at the render emission boundary.
- **Edit vs Play.** Paint mutates the edit world; Play deep-clones the
  deterministic instance set (spec 22). Procedural population produces the same
  set in a fresh clone from the same seed.
- **Terrain ground truth.** Instances sit on `height_at` from spec 42 — foliage
  and terrain share one height function; foliage never reinvents ground.

## Performance Targets

- Placement: deterministic scatter of **10 000 instances** across a marked
  region in **< 8 ms/tick** Domain B (bounded per-tick fill; bulk happens as an
  editor "populate" pass, not per gameplay tick).
- Query an instance's fixed `Transform` by type column: SoA `cast_slice` read,
  **< 5 ns/instance**.
- Render: GPU instancing target **10k–100k visible instances** at interactive
  frame times; Domain-A per-type stream build adds **< 0.5 ms** per frame for a
  few hundred types.
- LOD decision pure function: negligible (< 1 µs per type batch).

## Testing Strategy

All headless (no GPU) in Domain B + editor unit tests:
- **Placement determinism:** populate the same marked region 3× on two targets
  and assert the resulting instance `Transform`/seed lists are **bit-identical**
  and in identical spawn order.
- **Constraint correctness:** verify slope-limit, altitude band, biome-mask, and
  spacing rejections exactly against a scripted heightfield (spec 42 test patch);
  no candidate violates a constraint.
- **Idempotency:** run `foliage_populate_system` twice over the same region
  without a re-mark; assert no duplicate instances and unchanged world hash.
- **Brush command round-trip:** `FoliagePaintCommand.execute` then `.undo` and
  assert the instance column is bit-identical to pre-edit (spec 23); transaction
  collapse gives one undo per paint stroke; generation-guarded removal refuses
  stale handles.
- **Edit-vs-play:** paint edits change the edit world; entering Play deep-clones
  the deterministic set, and no editor command mutates the play twin (spec 22).
- **Wind does not affect logic:** run a gameplay query against a `FoliageInstance`
  while the renderer applies wind offset; assert the fixed `transform` is
  unchanged (wind is visual-only).
- **Serialization:** type + instance columns round-trip the spec-16 codec
  bit-identically with sorted iteration order.
- **Purity:** `verify-wasm-purity` reports `[PURE]` for `crates/logic-foliage`.

## Dependencies

- `contracts` (`StateView`, `WorldDelta`, `SpawnCommand`, `ColumnWrite`,
  `Entity`, `ComponentId`, `RecoverableError`), `bytemuck`, `serde`,
  `postcard`, `alloc`.
- `openengine-math` (`I16F16`, seeded integer hash / permutation helpers).
- Spec 42 (`height_at`, `TerrainView`, `sample_biome`, chunk regen marking).
- Spec 21 (`Transform`, `Tag`, `AssetRef`) + spec 23 (`Command`, manager) +
  spec 24 (viewport brush) + spec 13 (collider) + spec 45 (static obstacle).
- `crates/foliage-data` / `crates/editor` (Domain A instancing + paint).

## Next Steps

1. Register `FoliageType` (32) + `FoliageInstance` (33) components.
2. Implement `foliage_populate_system` (pure, seeded) + `foliage_lod`.
3. Build the Domain-A instanced renderer feed (instance stream from the column).
4. Implement `FoliagePaintCommand` + brush wiring (spec 23/24) with undo.
5. Add wind vertex-shader offset (visual-only) + collision/nav-obstacle hookup.
6. Determinism/serialization/purity test battery + cross-platform CI.
