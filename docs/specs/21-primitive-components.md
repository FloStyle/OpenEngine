---
spec: "21-primitive-components"
phase: "Phase 5: Editor"
status: "draft"
author: "OpenEngine AI"
created: "2026-09-03"
depends_on:
  - "00-ecs-architecture"
  - "02-asset-pipeline"
  - "04-render-pipeline"
  - "06-scene-management"
  - "07-editor-inspector"
  - "08-editor-hierarchy"
  - "16-serialization"
---
# 21 - Primitive Components Registry

## Overview

This spec is the **single source of truth** for the *built-in primitive*
components that every other spec (00 ECS, 04 render, 06 scene, 07 inspector,
08 hierarchy, 09 gizmos) and every gameplay system already references by name
(`Position`, `Velocity`, `Transform`, `Name`, `Parent`, `Sprite`,
`MeshRenderer`, `Camera`, `Light`, `Tag`) **without defining them**. This
document fixes three things that were previously implicit:

1. A **stable** `ComponentId(u32)` assignment table. IDs are permanent and
   never reassigned, so a persisted scene or a saved snapshot written under
   these IDs stays readable forever.
2. The exact `#[repr(C)]` `Pod + Zeroable` **field layout** of each primitive,
   so `size_of`/`align_of` are known up front for column planning and for the
   shared `postcard` column codec (spec 16).
3. The **semantic contract** of each component (defaults, relationships,
   asset-reference discipline) so downstream code and the inspector edit them
   the same way.

This crate is **Domain B-clean**: every struct below is `#[repr(C)]`,
`bytemuck::Pod + Zeroable`, and `serde Serialize/Deserialize`. No `std`, no
`f32` in gameplay-visible data, no `unsafe`. All scalar math fields are
`openengine-math::I16F16`. Render-facing structs carry **logical asset
references only** (AGENTS.md § 5): never an absolute path.

> **Relationship to earlier drafts.** spec 04 sketched loose field names for
> `Transform`/`MeshRenderer`/`Camera`, and specs 00/08/13 named
> `Position`/`Velocity`/`Parent` in passing. This spec is the canonical,
> version-locked definition; where this file conflicts with an earlier sketch,
> this file wins, and the earlier spec should be read as pointing here.

## Core Concepts

### Stable ComponentId table (never reassigned)

Component types are registered in the host `ComponentRegistry` (spec 00) under
an immutable `ComponentId(u32)`. The numbers below are **frozen at
registration time** and may only ever be *extended* by appending new higher
IDs — never renumbered, never recycled after deprecation. This is what keeps
`WorldSnapshot`/scene columns (spec 16) and guest `StateView.column(id)` lookups
stable across ABI revisions.

| `ComponentId` | Name          | `size_of` | `align_of` | Domain use                                  |
|---------------|---------------|-----------|------------|---------------------------------------------|
| 0             | `Position`    | 8         | 4          | 2D kinematic position (00, 13 physics)      |
| 1             | `Velocity`    | 8         | 4          | 2D kinematic velocity (00, 13 physics)      |
| 2             | `Transform`   | 40        | 4          | 3D pos/rot/scale (04 render, 09 gizmos)      |
| 3             | `Name`        | 64        | 1          | editor/hierarchy label (08)                 |
| 4             | `Parent`      | 8         | 4          | hierarchy tree edge (08); `INVALID` == root  |
| 5             | `Sprite`      | 40        | 4          | 2D textured quad (render)                   |
| 6             | `MeshRenderer`| 68        | 1          | 3D mesh + material (04 render)              |
| 7             | `Camera`      | 16        | 4          | active view/projection params (04 render)   |
| 8             | `Light`       | 20        | 4          | light source (render, later phase)          |
| 9             | `Tag`         | 64        | 1          | classification label for queries/filters    |

IDs 10..=1023 are reserved for future *built-in* primitives (physics components
of spec 49, audio emitters of spec 14, etc.) and are appended by later specs in
the order they land (see the "Extended Component Registry" table below). IDs ≥ 1024 are reserved for **game/mod author components**
registered at runtime by a project. The `ComponentRegistry` enforces that no ID
is registered twice and that built-in IDs below 1024 are never taken by user
code.

### Zeroable vs. logical default

Every struct is `Zeroable`, so an all-zero byte pattern is always a **valid,
memory-safe** value (that is what lets a freshly allocated SoA column or a
`postcard`-deserialized struct be interpreted without UB). But "zeroable" is a
*memory* contract, **not** the component's *logical* default: e.g. a zeroed
`Transform.rotation` quaternion `(0,0,0,0)` is not a unit quaternion, and
`Camera.near == 0` is degenerate. Therefore each component also provides a
canonical `DEFAULT` initializer (an `I16F16`-typed, deterministic constructor)
used when the ECS spawns an entity or the inspector "resets to default". The
spawn path fills a fresh row from `DEFAULT`, never from raw zero bytes. This
distinction is spelled out per component below.

### Fixed-point scalars

All numeric gameplay fields are `openengine-math::I16F16` (a `fixed::FixedI32`
alias, 16 integer + 16 fraction bits) — **never `f32`**. Where a field must be
serialized or carried as an opaque token (asset refs), we use `u64`/`u32`/`u8`,
all `Pod`. `f32` appears nowhere in these structs; it only ever exists at a
Domain-A display/GPU emission boundary via `openengine-math::quantize_to_f32`
(spec 04).

### Logical asset references (no absolute paths)

`Sprite` and `MeshRenderer` reference assets by a **logical token**, not a
host path. Following AGENTS.md § 5 and spec 06, an asset is addressed by a
stable relative path resolved against `OPENENGINE_ASSETS_PATH` at load time; the
component stores a compact handle that the asset registry (spec 02) maps to that
relative path. There is **never** an absolute path, `$HOME`, or OS-specific
separator baked into a component. The packed form (below) is a
length-prefixed, relative-path token so the component itself is self-describing,
portable, and serializable without a host registry being present.

### Derived / renamed components

- `Position`/`Velocity` (2D, physics/kinematics per specs 00 and 13) and
  `Transform` (3D, renderer per spec 04) are **distinct** primitive component
  types — a 2D kinematic body and a 3D rendered actor do not share the same
  archetype shape unless both are attached deliberately. `Transform` is *not* a
  synonym for `Position + Rotation + Scale`; it is the canonical 3D
  `Position⊕Rotation⊕Scale` blob that the renderer consumes in one column.
- The relationship is: **`Transform == position + rotation + scale`** where
  `Rotation` and `Scale` have no standalone registered ID because 3D render uses
  the combined `Transform`. A future "split transform" could be introduced as a
  **new** component pair (IDs ≥ 10); we never rename `Transform`.
- `Parent` creates a hierarchy tree: entities whose `Parent.parent ==
  Entity::INVALID` are roots. `Parent` is a normal component, so attaching one is
  an archetype migration (spec 00/08).
- `Name` and `Tag` both carry small fixed text but differ in semantics: `Name` is
  a per-entity human/agent label; `Tag` is a classification token used by
  queries and filters. They share a backing fixed-string cell but are distinct
  registered types.

## Key Rust Types

```rust
//! crates/ecs/src/components.rs (Domain A storage) and mirrored for
//! Domain-B reads through openengine-math / contracts. All derive below.
#![forbid(unsafe_code)]

use openengine_math::I16F16;
use contracts::Entity;

/// Relative-path asset token: length-prefixed UTF-8, forward slashes only,
/// resolved against OPENENGINE_ASSETS_PATH. `len == 0` means "no asset".
#[repr(C)]
#[derive(Clone, Copy, PartialEq, Eq, bytemuck::Pod, bytemuck::Zeroable,
         serde::Serialize, serde::Deserialize, Debug)]
pub struct AssetRef {
    pub len: u8,
    pub path: [u8; 31],   // longest supported relative token; pad to 32B
}
impl AssetRef {
    pub const NONE: AssetRef = AssetRef { len: 0, path: [0u8; 31] };
}

/// Position — 2D kinematic position. Fixed-point.
#[repr(C)]
#[derive(Clone, Copy, PartialEq, Eq, bytemuck::Pod, bytemuck::Zeroable,
         serde::Serialize, serde::Deserialize, Debug)]
pub struct Position { pub x: I16F16, pub y: I16F16 }

/// Velocity — 2D kinematic velocity (units/tick).
#[repr(C)]
#[derive(Clone, Copy, PartialEq, Eq, bytemuck::Pod, bytemuck::Zeroable,
         serde::Serialize, serde::Deserialize, Debug)]
pub struct Velocity { pub x: I16F16, pub y: I16F16 }

/// Transform — canonical 3D placement. Quaternion (w last).
/// Zeroable is valid memory; logical default is identity (scale 1).
#[repr(C)]
#[derive(Clone, Copy, PartialEq, Eq, bytemuck::Pod, bytemuck::Zeroable,
         serde::Serialize, serde::Deserialize, Debug)]
pub struct Transform {
    pub position: [I16F16; 3],
    pub rotation: [I16F16; 4],  // (x, y, z, w) unit quaternion
    pub scale:    [I16F16; 3],
}
impl Transform {
    // `fx!` produces exact I16F16 literals (openengine-math). Zeroable is the
    // memory contract; this DEFAULT is the semantic one used by spawn.
    pub const DEFAULT: Transform = Transform {
        position: [I16F16::ZERO; 3],
        rotation: [I16F16::ZERO, I16F16::ZERO, I16F16::ZERO, I16F16::ONE],
        scale:    [I16F16::ONE; 3],
    };
}

/// Fixed small string cell backing Name/Tag. UTF-8, truncated to 63 bytes.
/// No `String`/`Vec`: Pod needs a fixed inline buffer.
#[repr(C)]
#[derive(Clone, Copy, PartialEq, Eq, bytemuck::Pod, bytemuck::Zeroable,
         serde::Serialize, serde::Deserialize, Debug)]
pub struct FixedString {
    pub len: u8,
    pub bytes: [u8; 63],
}

/// Name — per-entity label shown in the hierarchy (spec 08).
#[repr(C)]
#[derive(Clone, Copy, PartialEq, Eq, bytemuck::Pod, bytemuck::Zeroable,
         serde::Serialize, serde::Deserialize, Debug)]
pub struct Name { pub text: FixedString }

/// Tag — classification label for queries/filters.
#[repr(C)]
#[derive(Clone, Copy, PartialEq, Eq, bytemuck::Pod, bytemuck::Zeroable,
         serde::Serialize, serde::Deserialize, Debug)]
pub struct Tag { pub text: FixedString }

/// Parent — hierarchy edge. Entity::INVALID == root.
#[repr(C)]
#[derive(Clone, Copy, PartialEq, Eq, bytemuck::Pod, bytemuck::Zeroable,
         serde::Serialize, serde::Deserialize, Debug)]
pub struct Parent { pub parent: Entity }

/// Sprite — 2D textured quad. Logical asset ref + z-order.
#[repr(C)]
#[derive(Clone, Copy, PartialEq, Eq, bytemuck::Pod, bytemuck::Zeroable,
         serde::Serialize, serde::Deserialize, Debug)]
pub struct Sprite {
    pub texture: AssetRef,     // logical relative-path token (32 B)
    pub sort_order: I16F16,    // painting order for overlapping sprites
    pub _reserved: u32,        // pad → 40 B, multiple of 8 for SoA cleanliness
}

/// MeshRenderer — 3D mesh + material. Logical asset refs.
#[repr(C)]
#[derive(Clone, Copy, PartialEq, Eq, bytemuck::Pod, bytemuck::Zeroable,
         serde::Serialize, serde::Deserialize, Debug)]
pub struct MeshRenderer {
    pub mesh: AssetRef,        // logical relative-path token (32 B)
    pub material: AssetRef,    // logical relative-path token; NONE = default
    pub visible: u8,           // 0/1; kept u8 for Pod simplicity
    pub _reserved: [u8; 3],    // pad → 68 B (multiple of 4)
}

/// Camera — view/projection params. fov_y, near, far fixed-point.
#[repr(C)]
#[derive(Clone, Copy, PartialEq, Eq, bytemuck::Pod, bytemuck::Zeroable,
         serde::Serialize, serde::Deserialize, Debug)]
pub struct Camera {
    pub fov_y: I16F16,   // radians (fixed)
    pub near:  I16F16,   // near plane distance (fixed)
    pub far:   I16F16,   // far plane distance (fixed)
    pub active: u8,      // 0/1 — exactly one active camera at render time
    pub _reserved: [u8; 3],
}
impl Camera {
    /// Degenerate-free default; real values via `openengine-math::fx!` at
    /// construction (e.g. 60° fov, near/far set by the scene). Kept `ZERO` here
    /// only to show a valid all-zeroable bit pattern; the spawn path overrides.
    pub const DEFAULT: Camera = Camera {
        fov_y: I16F16::ZERO,
        near:  I16F16::ZERO,
        far:   I16F16::ZERO,
        active: 0,
        _reserved: [0u8; 3],
    };
}

/// LightKind — discriminant-only enum (stored as u8 + reserved).
#[repr(u8)]
#[derive(Clone, Copy, PartialEq, Eq, bytemuck::Pod, bytemuck::Zeroable,
         serde::Serialize, serde::Deserialize, Debug)]
pub enum LightKind { Directional = 0, Point = 1, Spot = 2 }

/// Light — directional/point/spot source.
#[repr(C)]
#[derive(Clone, Copy, PartialEq, Eq, bytemuck::Pod, bytemuck::Zeroable,
         serde::Serialize, serde::Deserialize, Debug)]
pub struct Light {
    pub kind: LightKind,     // 1 byte
    pub _pad: [u8; 3],       // align intensity to 4
    pub intensity: I16F16,   // fixed intensity
    pub color: [I16F16; 3],  // fixed RGB in [0,1]
}
```

### ComponentId bindings

```rust
// crates/ecs/src/components.rs — stable, immutable bindings
pub const C_POSITION: ComponentId       = ComponentId(0);
pub const C_VELOCITY: ComponentId       = ComponentId(1);
pub const C_TRANSFORM: ComponentId      = ComponentId(2);
pub const C_NAME: ComponentId           = ComponentId(3);
pub const C_PARENT: ComponentId         = ComponentId(4);
pub const C_SPRITE: ComponentId         = ComponentId(5);
pub const C_MESH_RENDERER: ComponentId  = ComponentId(6);
pub const C_CAMERA: ComponentId         = ComponentId(7);
pub const C_LIGHT: ComponentId          = ComponentId(8);
pub const C_TAG: ComponentId            = ComponentId(9);
```

### size_of / align_of summary (for column planning)

| Type          | `size_of` | `align_of` | note                                |
|---------------|-----------|------------|-------------------------------------|
| `AssetRef`    | 32        | 1          | 31-byte token + len                 |
| `FixedString` | 64        | 1          | 63 bytes + len                      |
| `Position`    | 8         | 4          | 2 × I16F16                          |
| `Velocity`    | 8         | 4          | 2 × I16F16                          |
| `Transform`   | 40        | 4          | 3 + 4 + 3 I16F16                    |
| `Name`        | 64        | 1          | wraps `FixedString`                 |
| `Tag`         | 64        | 1          | wraps `FixedString`                 |
| `Parent`      | 8         | 4          | one `Entity`                        |
| `Sprite`      | 40        | 4          | AssetRef + I16F16 + u32             |
| `MeshRenderer`| 68        | 1          | 2 × AssetRef + visible + pad        |
| `Camera`      | 16        | 4          | 3 × I16F16 + active + pad           |
| `Light`       | 20        | 4          | kind + intensity + 3×color(I16F16)  |

> Column byte-size sanity: `count * element_size` must be a multiple of 4 (spec
> 00, clean SoA). Every struct above is a multiple of 4 bytes; `Sprite` and
> `Light` round up with a `_reserved`/`_pad` cell so rendering columns stay
> naturally aligned when `cast_slice` is used.

### Logical default rationale

- `Position`/`Velocity`: zero == at origin / at rest. Zeroable default == logical
  default.
- `Transform`: zeroable quaternion is **not** valid, so spawn uses
  `Transform::DEFAULT` (identity rotation, scale 1).
- `Parent`: zeroed `parent.index == 0` must never be mistaken for root; use
  `Entity::INVALID` (generation 0, index `u32::MAX`) to mean "no parent". A
  freshly spawned root writes `parent: Entity::INVALID`.
- `Sprite`/`MeshRenderer`: `AssetRef::NONE` (len 0) = "no asset"; the default
  `Sprite` points at no texture (the caller assigns one through the asset
  picker, spec 07).
- `Camera`: the numeric params (`fov_y`/`near`/`far`) are authored per-scene and
  are not globally defaultable; `Camera::DEFAULT` is memory-safe (valid zeroable
  bits) and `active: 0` so nothing renders until a system or the editor
  explicitly configures and activates a camera. Column spawns that claim to
  carry a ready camera must fill numeric fields explicitly, never rely on
  `DEFAULT` zeros.

### Relationships & archetype consequences

- Adding `Parent` or `Tag` moves an entity into a new archetype (migration).
- Typical composed archetypes: `(Position, Velocity, Sprite)` for 2D movers;
  `(Transform, MeshRenderer, Parent, Name)` for 3D actors; a scene always
  contains at least one `(Transform, Camera)` "camera rig" entity.
- Because a component type can appear **once** per archetype, repeated
  classification (multiple tags on one entity) is not expressible with a single
  `Tag`; use a **tag table** (`Tag` → parent entity) or a dedicated
  child-component pattern if multi-tag is ever required. Single-`Tag` covers the
  editor filter/hierarchy use-cases in spec 08.

## Constraints

- **Domain B-clean:** every struct is `#[repr(C)] Pod + Zeroable`, `serde`
  both, no `std`, no `unsafe`, no `f32` in any gameplay-visible field
  (AGENTS.md § 3, § 4).
- **Stable IDs:** ComponentId assignments 0–9 are immutable; extensions append
  new IDs. Never renumber, never reuse a deprecated ID.
- **Fixed-point only:** scalar math uses `openengine-math::I16F16`. No raw
  `f32` inside these components.
- **No absolute paths:** asset refs are logical relative-path tokens resolved
  against `OPENENGINE_ASSETS_PATH` (AGENTS.md § 5, spec 02/06). No `$HOME`, no
  OS-specific separators.
- **No `String`/`Vec` in Pod:** text is length-prefixed fixed inline arrays;
  anything longer than 63 bytes is truncated (lossless round-trip is not
  required for labels; the editor shows a truncation ellipsis).
- **Serializable deterministically:** each struct is `serde`-serializable; the
  scene/world codec (spec 16) stores raw Pod column bytes plus a registered
  element-size, so a deserialized column must match `size_of` exactly.
- **Per-component `layout_version`:** every registered component carries a
  per-component schema revision starting at `contracts::COMPONENT_LAYOUT_VERSION`
  (= `1`). It is bumped on any `#[repr(C)]` field change and used by the spec-16
  codec to migrate component-local layout drift independently of the global
  `ARCH_VERSION`.
- Cross-platform identical on `x86_64-linux` and `aarch64-linux`; no platform
  `repr` differences for these field types (I16F16 is `i32`; fixed arrays have
  no padding surprises given the alignment table above).
- Values resolve to a sensible logical state whether the column is zero-filled
  (memory-safe) or `DEFAULT`-initialized (semantically correct); spawning always
  uses `DEFAULT`.

## Performance Targets

- Registration: compile-time / one-time table build; `ComponentRegistry::lookup`
  is a `u32`-indexed `Vec` fetch — O(1), no hash.
- `size_of` guarantees columns are cache-line friendly for the common primitives
  (`Position` 8 B, `Velocity` 8 B, `Transform` 40 B).
- No per-element branching in the SoA hot path; all values are fixed-width and
  aligned to 4 B boundaries, so `bytemuck::cast_slice` column writes (spec 00)
  copy contiguously.
- String cells (64 B) are larger; they are optional components so they are only
  paid for on entities that need a label/tag (rare relative to kinematic rows).

## Testing Strategy

- **Compile-time bounds:** a `test` module asserts every type satisfies
  `T: bytemuck::Pod + bytemuck::Zeroable + serde::Serialize +
  serde::Deserialize` via generic `fn assert_primitive<T: Pod + Zeroable>() {}`.
- **Layout asserts:** `assert_eq!(size_of::<T>(), expected)` and
  `assert_eq!(align_of::<T>(), expected)` for every row in the table above;
  catches accidental padding/repacking.
- **Zeroable validity:** `assert_eq!(size_of::<T>(), core::mem::size_of::<[u8;
  size]>())` style checks that a zeroed `&[u8]` is a valid `T` (no padding
  invariants).
- **Default semantics:** construct `Transform::DEFAULT` and assert the quaternion
  is unit length and scale is all-`I16F16::ONE`; assert `Position`/`Velocity`
  defaults are zero.
- **AssetRef discipline:** build an `AssetRef` from a relative token, assert
  `len` is exact and the token contains no `/../`, no leading `/`, no `\`, and no
  drive/`$HOME` prefix (regression guard for AGENTS.md § 5).
- **Spawn-all integration:** spawn one entity per *distinct composed archetype*
  (e.g. all primitives together: `Position+Velocity+Transform+Name+Parent+Sprite+
  MeshRenderer+Camera+Light+Tag`), assert a new archetype is created, all columns
  have the expected `element_size`, and every component reads back its `DEFAULT`.
- **Determinism:** serialize the same filled world 3× with the spec-16 codec on
  two targets and assert byte-identical output; and two hosts spawning the same
  archetype set produce the same archetype IDs in the same order.
- **Round-trip:** fill a representative set of components, `postcard`-encode
  their column bytes, decode, and assert bit-identical columns.

## Dependencies

- `contracts` (`ComponentId`, `Entity`, `Entity::INVALID`, `Pod/Zeroable`
  via `bytemuck`).
- `openengine-math` (`I16F16`).
- `bytemuck` (`Pod`, `Zeroable`), `serde` + `postcard` (codec, spec 16).
- Consumed by `crates/ecs` (archetype storage), `crates/editor` (inspector/hierarchy/gizmos,
  specs 07/08/09), render pipeline (spec 04), physics (spec 13), scene codec
  (spec 06/16).

## Extended Component Registry (engine additions — specs 30–50)

The base registry above (0–9) is the source of truth for primitive components.
As the engine grew to full feature parity (specs 30–50), engine subsystems
registered additional components. **Reservation policy:** engine components own
the `10–1023` band, reserved in ordered windows; game/user components live at
`≥ 1024`. IDs are stable and never reassigned. Extend within the reserved window
of the owning subsystem only; cross-subsystem claims must update this table.

| Id | Component | Defining spec |
|----|-----------|---------------|
| 10 | `Skeleton` | 36-skeletal-animation |
| 11 | `AnimationClip` (asset handle) | 36 |
| 12 | `AnimationPlayer` | 36 |
| 13 | `SkinnedMeshRenderer` | 36 |
| 14 | `AnimatorController` | 37-animation-state-machines |
| 15–19 | *(reserved — animation)* | 36/37 |
| 20 | `ParticleEmitter` | 39-particle-system |
| 21 | `ParticleModule` | 39 |
| 22 | `PostProcessVolume` | 40-post-processing |
| 23 | `PostProcessSettings` | 40 |
| 24 | `ReflectionProbe` | 41-advanced-lighting |
| 25 | `LightProbe` | 41 |
| 26 | `LightmapSettings` | 41 |
| 27–29 | *(reserved — VFX/rendering)* | 39–41 |
| 30 | `Terrain` | 42-terrain-system |
| 31 | `TerrainLayer` | 42 |
| 32 | `FoliageType` | 43-vegetation-system |
| 33 | `FoliageInstance` | 43 |
| 34 | `BehaviorTree` | 44-behavior-trees |
| 35 | `Blackboard` | 44 |
| 36 | `AIAgent` | 44 |
| 37 | `NavMesh` | 45-navigation |
| 38 | `NavAgent` | 45 |
| 39 | `NavObstacle` | 45 |
| 40–49 | *(reserved — environment/AI)* | 42–45 |
| 50 | `Sequence` | 46-sequencer |
| 51 | `SequenceTrack` | 46 |
| 52 | `SequenceKeyframe` | 46 |
| 53 | `UICanvas` | 47-ui-system |
| 54 | `UIElement` | 47 |
| 55 | `UIText` | 47 |
| 56 | `UIButton` | 47 |
| 57–59 | *(reserved — cinematic/UI)* | 46–47 |
| 60 | `RigidBody` | 49-advanced-physics |
| 61 | `Collider` | 49 |
| 62 | `PhysicsMaterial` | 49 |
| 63 | `Joint` | 49 |
| 64 | `ScriptNodeGraph` | 48-visual-scripting |
| 65–69 | *(reserved — advanced)* | 48–50 |
| 70 | `Children` | 08-editor-hierarchy |
| 71 | `Bounds` | 24-editor-viewport |
| ≥ 1024 | game/user components | — |

Reuses from the base registry (not re-registered): `Transform`(2) for transform
tracks/group ops, `Parent`(4) for UI trees and hierarchy, `Camera`(7) for
cinematic-camera tracks, `Light`(8) for all light kinds. Editor/UI tooling
(specs 30–35) introduces **no** new component.

A few of these require a `contracts`/`ARCH_VERSION` bump plus `docs/abi/` entry
before landing in code (e.g. particle record transport, `LightKind::Area`,
`DeferredCommand` topics for UI events) — treat those as ABI-gated work, never
silent changes.

## Next Steps

1. Land this file's type definitions in `crates/ecs/src/components.rs` with the
   frozen ComponentId bindings and `ComponentRegistry` registration (spec 00).
2. Add the compile-time Pod/Zeroable + layout test module.
3. Wire the `DEFAULT` initializers into the ECS spawn path so fresh rows are
   never raw-zero for `Transform`/`Camera`.
4. Have spec 04's render structs and spec 08's `Parent` logic reference this
   file as the canonical definition.
5. Re-verify column `element_size` consistency with the spec-16 codec and add
   the spawn-all-integration test.
