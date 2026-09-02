---
spec: "36-skeletal-animation"
phase: "Phase 5: Animation"
status: "draft"
author: "OpenEngine AI"
created: "2026-09-03"
depends_on:
  - "00-ecs-architecture"
  - "02-asset-pipeline"
  - "04-render-pipeline"
  - "12-scripting-macros"
  - "13-physics-basics"
  - "16-serialization"
  - "21-primitive-components"
---
# 36 - Skeletal Animation

## Overview

The skeletal animation layer is the deterministic substrate that drives rigged,
skinned characters. A **skeleton** is a hierarchy of joints that begins in an
authored **bind pose** and is animated over time by sampling one or more
**animation clips** (keyframes per joint). Domain B advances a **tick-driven
playback clock** (never wall-clock) and samples the active clip into a **local
pose** (per-joint position / rotation / scale), which is then flattened
joint-by-joint from bind/local space into **world (joint) matrices**. Those
matrices feed **skinning** — the per-vertex transformation of a bound mesh by a
weighted blend of up to four joints — which in OpenEngine is a **Domain A / GPU
concern** (spec 04).

Everything that determines *what pose is produced on which tick* is pure
fixed-point math that lives in Domain B, so identical inputs produce
bit-identical joint matrices on every platform and in the Wasm sandbox. The
remaining machinery — clip/skeleton **asset storage**, **GPU skinning**, and the
CPU preview skinner for the editor — is Domain A.

This is a design spec; it fixes ECS `ComponentId`s, lays out the deterministic
sample/interpolation/skinning math contract, and splits responsibilities across
the domains. It does **not** add any gameplay system to the shipped module; it is
a blueprint to be implemented under the DoD in `AGENTS.md` § 9.

## Core Concepts

### 1. Skeleton = joint hierarchy + bind pose (an asset, mirrored in a component)

The *definition* of a rig (its joint tree, per-joint local TRS in the bind pose,
per-joint inverse-bind matrices) is **authored art data** and therefore lives in
a **host asset** (spec 02). It is variable-length and pointer-free, so it cannot
fit in a fixed `#[repr(C)]` ECS column and must not be shipped to the guest. An
entity that wants to *be* a rig carries a small `Skeleton` component (id **10**)
that is little more than a handle to that asset plus a cached `joint_count` used
for SoA planning. Domain B never dereferences the asset bytes; it reasons only
about the integer joint count and reads joint *state* from pose columns the host
materializes for it.

Joints are indexed `0..joint_count`. Joint 0 is the **root**. Each joint has a
fixed `parent: i32` (its `parent_joint` index in `[0..joint_count)`, or `-1` for
the root). The asset also carries the **inverse bind pose** `invBind[j]` (the
inverse of the local-to-world transform of joint `j` *at bind time*), which is
needed for skinning and is only ever consumed in Domain A.

### 2. Clips are assets; sampling time is tick-driven

An **animation clip** is also a host asset (a set of per-joint keyframe
tracks), referenced by an `AnimationClip` component (id **11**). Because the
guest cannot reach the asset store, the engine never samples a clip *inside*
the Wasm from raw asset bytes. Instead:

- **Domain B owns the deterministic time law.** The pure playback system
  (spec 12 `#[system]`) reads each `AnimationPlayer`/`AnimatorController`
  component from a `StateView` and advances its `time` by exactly
  `FIXED_STEP * speed` per sim tick. This is integer/fixed accumulation only —
  the same tick count always yields the same clip time, so replays are
  reproducible (spec 05/22).
- **A pure sample core (`openengine-anim-core`, Domain-B-clean) turns
  (clip-keyframe table, clip-time) → local pose.** The keyframe table is a
  `&[u8]`/`&[KeyframeRow]` slice passed by the caller. In the **playback path**
  the host (Domain A) reads the clip asset and feeds the *same* `anim-core`
  sampler the keyframe slices, producing the pose the skinner consumes. In the
  **headless/test path** the same sampler is driven with synthetic keyframe
  buffers. Because one `no_std` + `forbid(unsafe_code)` crate implements the
  sampling, every consumer (host playback, GPU prep, headless determinism tests,
  editor preview) uses bit-identical fixed-point math.

This "one sampler, called by both domains" split is the crux that reconciles
"clips live in Domain A assets" with "sampling must be deterministic fixed-point
math": the *math* is one shared Domain-B-clean source of truth, and only the
*asset bytes / buffer plumbing* differ by caller.

### 3. Keyframes, interpolation, easing

A clip stores, per joint, a time-sorted track of `Keyframe { t_tick: u32, trs:
JointTrs }` where `t_tick` is the keyframe time in fixed tick units and `JointTrs`
is position (`I16F16×3`), rotation (fixed quaternion, x/y/z/w), scale
(`I16F16×3`). Between two bracketing keyframes the pose is interpolated with a
mode stored on the track:

- `Step` — hold the previous frame (used for pose/visibility "stepping").
- `Linear` — component-wise LERP of translation and scale; **NLERP** (normalized
  LERP) of the rotation quaternion. NLERP needs only fixed multiply/add plus a
  fixed integer square root to renormalize — no transcendental, fully
  deterministic and cheap.
- `Slerp` — spherical interpolation for authored high-fidelity rotation. Slerp
  needs `acos`, `sin`, `cos`; these are evaluated through a **deterministic
  fixed-point lookup table** (a quantized unit-circle/arccos table plus
  short-polynomial refinement in `openengine-math`) so the result is identical
  on every CPU. NLERP is the default; `Slerp` is opt-in per track.

Each track may also carry an **easing** tag (`EaseIn`, `EaseOut`, `Smooth`,
`Hold`) applied to the interpolation *factor* between brackets; smoothing uses a
fixed-point `smoothstep` (Hermite) polynomial `3t²−2t³` computed in the wider
`I32F32` type, never a raw `f32`.

> **Transcendental determinism note.** No Domain-B code ever calls `std`
> `sin`/`cos`/`acos` (absent in `no_std` and CPU-variant). All rotation math that
> needs a transcendental uses `openengine-math`'s table-driven,
> quantized-argument implementations, whose outputs are a pure function of the
> input fixed value.

### 4. Local → world joint matrices

Given a local pose `{ p_j, q_j, s_j }` per joint, world (model-space) joint
placement is a forward tree traversal:

```
world[j] = compose( world[parent[j]] , local_TRS(p_j, q_j, s_j) )   // parent order
```

`world[0]` (root) is additionally transformed by the owning entity's own
`Transform` when the rig is placed in the scene. Composition is done with fixed
quaternion multiplication and fixed rotation-of-a-vector (using
`openengine-math` quaternion ops) — a 4×4 `f32` matrix is **not** formed until
Domain A quantizes the fixed pose to `f32` at the skinning/GPU boundary (spec 04
`quantize_to_f32`). Each joint's final skinning matrix is
`skin[j] = world[j] * invBind[j]`, computed in Domain A.

### 5. Skinning

A `SkinnedMeshRenderer` (id **13**) binds a mesh asset to a `Skeleton` entity and
points at a skinning-data asset (the vertex→4-joint-index + 4-weight table). The
skinner computes, for each vertex `v` bound to joints `(b0..b3)` with weights
`(w0..w3)`:

```
v' = Σ_i w_i · (skin[b_i] · v)
```

Weights are authored art data and are stored/consumed as **`f32` on the GPU /
host mesh**, exactly like mesh positions (spec 04 § "Mesh type"), so they are not
"gameplay math" and may be `f32`. **GPU skinning is Domain A** (a wgpu compute or
vertex-shader pass that reads the joint-matrix buffer). A **CPU reference
skinner** also lives in Domain A so skinning can be validated headlessly and so a
"bake bind-pose to mesh" fallback works with no GPU. The numeric *sampling* that
decides which pose is skinned is deterministic fixed-point; the *matrix
multiply on the GPU* is standard `f32` rendering, exempt from the fixed-point
law by the spec-04 boundary.

### 6. Playback clock & loop modes

`AnimationPlayer.time` is a fixed `I16F16` seconds value, advanced per tick. Loop
modes (Once, Loop, PingPong) determine what happens at the clip's duration edge.
This is the same clock the state machine (spec 37) and the editor preview (spec
38) rely on; no wall-clock ever drives it.

## Key Rust Types

The three animation crates:

- `crates/anim-core` — Domain-B-clean: `#![no_std] + #![forbid(unsafe_code)]`.
  Pure sampling/math: keyframe interpolation, easing, quaternion NLERP/Slerp
  (via table), joint local→world accumulation (fixed). **This is the shared
  deterministic core**.
- `crates/ecs` / `crates/editor` — Domain A: asset-backed clip/skeleton loaders,
  `Skeleton`/`AnimationClip`/`AnimationPlayer`/`SkinnedMeshRenderer`
  registration, the CPU preview skinner, GPU skinning buffers.
- `crates/logic-sandbox` + `crates/logic-export` — Domain B: the
  `animation_update_system` and its `#[no_mangle]` trampoline.

```rust
// crates/anim-core/src/pose.rs — Domain B clean.
#![forbid(unsafe_code)]

use openengine_math::{I16F16, I32F32, fx};
use openengine_math::quat as fxq;   // fixed quaternion helpers + trig tables

/// Signed parent index; -1 == root. Joint 0 is always the root.
pub type JointIndex = u32;
pub const ROOT_JOINT: JointIndex = 0;

/// Per-joint local transform (TRS) — a clip keyframe value and a pose row.
#[repr(C)]
#[derive(Clone, Copy, PartialEq, Eq, bytemuck::Pod, bytemuck::Zeroable,
         serde::Serialize, serde::Deserialize, Debug)]
pub struct JointTrs {
    pub translation: [I16F16; 3],
    pub rotation:    [I16F16; 4],  // (x,y,z,w) unit quaternion
    pub scale:       [I16F16; 3],
}
impl JointTrs {
    pub const BIND_DEFAULT: JointTrs = JointTrs {
        translation: [I16F16::ZERO; 3],
        rotation: [I16F16::ZERO, I16F16::ZERO, I16F16::ZERO, I16F16::ONE],
        scale: [I16F16::ONE; 3],
    };
}

/// Interpolation between bracketing keyframes.
#[repr(u8)]
#[derive(Clone, Copy, PartialEq, Eq, bytemuck::Pod, bytemuck::Zeroable,
         serde::Serialize, serde::Deserialize, Debug)]
pub enum CurveMode { Step = 0, Linear = 1, Slerp = 2 }

/// Easing tag applied to the interpolation factor.
#[repr(u8)]
#[derive(Clone, Copy, PartialEq, Eq, bytemuck::Pod, bytemuck::Zeroable,
         serde::Serialize, serde::Deserialize, Debug)]
pub enum EaseMode { Hold = 0, EaseIn = 1, EaseOut = 2, Smooth = 3 }

/// One keyframe: time in fixed tick units + TRS value.
#[repr(C)]
#[derive(Clone, Copy, PartialEq, Eq, bytemuck::Pod, bytemuck::Zeroable,
         serde::Serialize, serde::Deserialize, Debug)]
pub struct Keyframe {
    pub t_tick: u32,        // keyframe time (integer tick units within the clip)
    pub value:  JointTrs,
}

/// Pure, deterministic sampler. `track` is a sorted `&[Keyframe]`; the caller
/// (host playback OR headless test) supplies clip-time `t` in the same tick
/// units. Returns the interpolated local TRS for one joint.
///
/// Determinism: only integer + fixed arithmetic and table-driven transcendentals.
pub fn sample_track(track: &[Keyframe], t: u32,
                    mode: CurveMode, ease: EaseMode) -> JointTrs {
    if track.is_empty() { return JointTrs::BIND_DEFAULT; }
    if track.len() == 1 || t <= track[0].t_tick { return track[0].value; }
    let last = track.len() - 1;
    if t >= track[last].t_tick { return track[last].value; }
    // binary search for the bracketing pair [i, i+1] ...
    // let u = eased_fraction(...) with I32F32 Hermite for Smooth
    // let trs = switch(mode) { Step=>a, Linear|Slerp=>nlerp/slerp(a,b,u) };
    // Slerp only when mode==Slerp; both are renormalized to unit quaternion.
    unreachable!("see implementation; body elided in spec")
}

/// Flatten a local pose into world joint frames, parent order. `world[j]` output
/// is fixed point; it is quantized to f32 only by a Domain-A skinner.
pub fn local_to_world(parent: &[i32], local: &[JointTrs],
                      world_out: &mut [JointTrs]) { /* fixed compose + tree walk */ }
```

```rust
// crates/ecs/src/components.rs (Domain A storage) — new registrations 10..=13.
use contracts::{ComponentId, Entity};
use openengine_math::I16F16;

/// ComponentId 10 — handle to a skeleton *asset* + cached joint count mirror.
#[repr(C)]
#[derive(Clone, Copy, PartialEq, Eq, bytemuck::Pod, bytemuck::Zeroable,
         serde::Serialize, serde::Deserialize, Debug)]
pub struct Skeleton {
    pub skeleton_asset: u32, // host asset-registry handle (spec 02)
    pub joint_count: u32,    // cached mirror of asset joint count (SoA planning)
    pub _reserved: u64,      // pad to 16 B (multiple of 8)
}

/// ComponentId 11 — a clip *asset handle*. Entities that wish to bind to a clip
/// directly (the non-state-machine, simple path) carry this component. The clip
/// bytes never leave Domain A.
#[repr(C)]
#[derive(Clone, Copy, PartialEq, Eq, bytemuck::Pod, bytemuck::Zeroable,
         serde::Serialize, serde::Deserialize, Debug)]
pub struct AnimationClip {
    pub clip_asset: u32,
    pub _reserved: u64,     // pad to 12 B (multiple of 4)
}

/// ComponentId 12 — runtime playback clock for the simple/direct path.
#[repr(C)]
#[derive(Clone, Copy, PartialEq, Eq, bytemuck::Pod, bytemuck::Zeroable,
         serde::Serialize, serde::Deserialize, Debug)]
pub struct AnimationPlayer {
    pub clip: u32,          // bound clip asset handle (== AnimationClip.clip_asset)
    pub speed: I16F16,      // signed, fixed; 0 = freeze; <0 reverses
    pub time:  I16F16,      // seconds within clip, advanced FIXED_STEP*speed per tick
    pub weight: I16F16,     // [0,1] contribution (single-clip path keeps 1)
    pub loop_mode: u8,      // 0=Once 1=Loop 2=PingPong
    pub playing: u8,        // 0/1
    pub _reserved: [u8; 6], // pad to a multiple of 4
}

/// ComponentId 13 — binds a mesh to a skeleton for skinning.
#[repr(C)]
#[derive(Clone, Copy, PartialEq, Eq, bytemuck::Pod, bytemuck::Zeroable,
         serde::Serialize, serde::Deserialize, Debug)]
pub struct SkinnedMeshRenderer {
    pub mesh_asset: u32,       // static geometry (Domain A GPU mesh)
    pub skeleton: Entity,      // the entity that carries the Skeleton component
    pub skin_asset: u32,       // vertex→joint/weight table asset
    pub enabled: u8,           // 0/1
    pub _reserved: [u8; 3],    // pad to a multiple of 4
}
```

```rust
// crates/logic-sandbox/src/systems/animation.rs — Domain B pure system (spec 12).
#[system(name = "animation_update", schedule = FixedUpdate,
         query = AnimationPlayer)]
fn animation_update(view: &StateView<'_>) -> Result<WorldDelta, RecoverableError> {
    // For each AnimationPlayer column row:
    //   let mut p = cast_slice::<AnimationPlayer>(col);       // read
    //   new_time = clamp/loop(p.time + FIXED_STEP * p.speed); // fixed only
    //   if edge crossed → Emit{...} or mark "reached end";
    // write new_time (and pingpong direction) back via a ColumnWrite.
    // Pure: no IO, no clock, no RNG; identicical tick ⇒ identical new_time.
    Ok(WorldDelta::default())   // assembled from ColumnWrites in implementation
}
```

## Components

| ComponentId | Name                 | Domain | What it is                                            |
|-------------|----------------------|--------|-------------------------------------------------------|
| 10          | `Skeleton`           | both   | Handle to skeleton asset (joints, bind, invBind) + cached joint_count. |
| 11          | `AnimationClip`      | both   | Clip asset handle (per-joint keyframe tracks live in the host asset). |
| 12          | `AnimationPlayer`    | both   | Tick-driven playback clock (clip, speed, time, weight, loop). |
| 13          | `SkinnedMeshRenderer`| both   | Mesh ↔ skeleton binding + vertex/joint/weight table asset. |
| 15..19      | *(reserved)*         | —      | Reserved for later animation ECS components (e.g. root-motion, IK rig). Never renumber 10–13. |

The heavy per-joint **definitional** data (clip tracks, skeleton hierarchy, bind
and inverse-bind, vertex weights) is authored art stored as **host assets** and is
*not* ECS columns. Only the small runtime/playback *mirrors* (10–13) are Pod
columns; joint **state** (the live pose) is carried per-frame either as a host
materialized read column feeding Domain B or as the shared `anim-core` pose
buffer consumed by the Domain-A skinner. IDs 15–19 remain available; this spec
only assigns 10–13.

## Constraints

- **Domain B-clean core:** `openengine-anim-core` is `#![no_std] +
  #![forbid(unsafe_code)]`; every `JointTrs`/`Keyframe` is `#[repr(C)] Pod +
  Zeroable + serde`. No `f32` in the sampler.
- **Tick-driven, not wall-clock.** `AnimationPlayer.time` advances by
  `FIXED_STEP * speed` per sim tick (fixed accumulation). No `std::time`, no
  scheduling dependency (AGENTS.md § 3, spec 05).
- **Determinism by construction.** Same clip, same tick count, same speed ⇒
  bit-identical local pose and (after Domain-A `f32` quantization is applied at
  the GPU boundary) identical skinning output. Transcendentals (only needed for
  `Slerp`/`EaseOut`) go through `openengine-math` fixed lookup tables.
- **No `HashMap` iteration.** Joint/track ordering is by explicit index and sorted
  `t_tick`; guest alloc is `Vec`/fixed scratch reused across ticks (spec 13 §
  Performance).
- **Pod columns must stay a multiple of 4 B** for clean SoA (spec 00/21); each
  struct above pads accordingly. `size_of` must be locked by layout tests.
- **Assets never ship as columns.** Clip/skeleton/skin bytes stay in Domain A;
  the guest sees handles (`u32` registry ids) and materialized fixed poses only.
- **Slerp is opt-in.** Default `Linear`/NLERP needs no transcendental and is the
  deterministic default; authored high-fidelity tracks may request table-based
  `Slerp`.
- **GPU/CPU skinning is Domain A.** `f32` appears in mesh/weight art and in the
  GPU skin pass only (spec 04 boundary); it never re-enters the sampler.
- Portability `x86_64-linux`/`aarch64-linux`; headless logic tests need no GPU;
  `verify-wasm-purity` reports `[PURE]`.

## Performance Targets

- Sampler: `sample_track` over the active joints is O(log k) per joint (binary
  search) — **< ~1 µs per joint**; a 64-joint clip at 60 Hz is comfortably within
  the Domain-B tick budget.
- `local_to_world` tree walk: **< 2 µs per joint** (fixed compose, no alloc).
- Playback time-advance system: **< 0.1 µs / entity** (one fixed accumulate +
  edge check + optional `ColumnWrite`).
- Skinning (Domain A): GPU pass keeps the 16.67 ms frame budget at 60 Hz for the
  render scene (spec 04); CPU reference skinner targets **< 5 ms** for a 10k-vertex
  × 64-joint character in headless/preview builds.
- Clip memory: sampled pose is `joint_count × 40 B` (10 `I16F16` + pads) held in
  a reused buffer; no per-tick allocation.

## Testing Strategy

- **Purity + layout:** every `JointTrs`/`Keyframe`/component satisfies
  `Pod + Zeroable`; assert exact `size_of`/`align_of` (multiple of 4).
- **Interpolation unit:** step/linear/slerp between two known keyframes produce
  expected fixed values (golden). Assert NLERP output is a unit-length quaternion
  to fixed precision.
- **Easing:** `Smooth` Hermite matches the analytic `3t²−2t³` table within the
  `I32F32` error budget.
- **Transcendental determinism:** `sin/cos/acos` at fixed grid points match a
  precomputed reference table bit-for-bit on `x86_64-linux` and `aarch64-linux`.
- **local→world:** a 2-joint chain (hip→thigh) composed by hand matches the tree
  walk; a 3-joint chain matches a reference.
- **Playback determinism:** advance the same clip for 1000 ticks **3×**; assert
  bit-identical final `time` and local pose.
- **Skinning (headless, Domain A CPU skinner):** skinned vertex positions for a
  known bind + pose match a golden reference; weights that sum to 1 keep affine
  correctness.
- **Loop modes:** Once clamps and emits a "clip end" event once; Loop wraps; both
  stay reproducible under replay.
- **Purity gate:** `python3 brain/orchestrator.py verify-wasm-purity` reports
  `[PURE]` for the Domain-B animation system.
- **Integration:** a spawned rig (Skeleton+SkinnedMeshRenderer+AnimationPlayer)
  ticks, the sampler produces a pose, the Domain-A skinner emits a mesh for
  spec-04 draw — headless on the CPU skinner.

## Dependencies

- `openengine-math` (`I16F16`, `I32F32`, `fx!`, fixed quaternion ops + trig
  tables, integer sqrt).
- `contracts` (`StateView`, `WorldDelta`, `ColumnWrite`, `Entity`, `ComponentId`,
  `RecoverableError`, `code`), `bytemuck`, `serde`/`postcard`, `alloc`.
- `crates/anim-core` (shared sampler) consumed by `crates/ecs`/`crates/editor`
  (Domain A) and referenced by `logic-sandbox` (Domain B).
- Asset pipeline (spec 02) for clip/skeleton/skin loading; render pipeline (spec
  04) for the GPU skin pass; state machine (spec 37) builds on the same clock.

## Next Steps

1. Add 10–13 component registrations + layout tests to `crates/ecs`.
2. Bootstrap `crates/anim-core`: `JointTrs`, `Keyframe`, `CurveMode`, `EaseMode`,
   `sample_track`, `local_to_world`.
3. Implement the fixed trig/sqrt tables in `openengine-math` needed by Slerp.
4. Domain-A clip/skeleton asset loaders (spec 02) + inverse-bind computation.
5. `animation_update` Domain-B system + `logic-export` trampoline.
6. CPU reference skinner (headless) then GPU skin pass in spec-04.
7. Determinism + purity + cross-platform test battery.

---
