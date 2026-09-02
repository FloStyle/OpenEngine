---
spec: "37-animation-state-machines"
phase: "Phase 5: Animation"
status: "draft"
author: "OpenEngine AI"
created: "2026-09-03"
depends_on:
  - "12-scripting-macros"
  - "21-primitive-components"
  - "36-skeletal-animation"
---
# 37 - Animation State Machines

## Overview

A **animation state machine (ASM)** lets authored locomotion logic — the classic
`idle → walk → run` graph — decide which clip (or blend of clips) a rig plays,
as a pure function of *parameters*. An ASM is a directed graph of **states**
(each state plays or blends one or more motions) connected by **transitions**
(each guarded by **conditions** over named **parameters**: `bool`, `float`,
`trigger`). **Blend trees** (1D / 2D) and **layers** (additive / override) are the
composable machinery inside states for crossing multiple clips at once.

The graph itself (states, transitions, blend-tree nodes, layer setup) is authored
**asset data** held inside an `AnimatorController` asset — it is **data inside
the controller, not separate ECS columns or entities** (see Constraints). The
entity that runs a controller carries a single `AnimatorController` component
(id **14**) that mirrors the *runtime* evaluation state: which layer is active,
the current state id, the clip-local time, and a bounded block of live
parameters.

Evaluation is **tick-driven**: a Domain-B pure system
(`animator_update_system`) reads the `AnimatorController` columns from a
`StateView`, advances each controller's per-layer time by `FIXED_STEP`, tests the
outgoing transitions of the current states in deterministic order, and on a
transition updates `current_state` / time and consumes **trigger** parameters.
Parameter changes (e.g. a gameplay system setting `speed` to move from `walk` to
`run`) are themselves ordinary fixed-point writes through `WorldDelta`; nothing
wall-clock driven, nothing random. Because the *decision* is a pure function of
(current states, current params, tick count), every playback reproduces the same
state sequence and pose (spec 36).

## Core Concepts

### 1. Graph is asset data; runtime is a component

`AnimatorController` (id **14**) has two faces:

- **Authoring/graph (asset).** Loaded by Domain A from the asset store (spec 02);
  edited by the animation editor (spec 38). Contains: an ordered list of **states**
  (each referencing a clip or a **blend-tree** root), an ordered list of
  **transitions** `(from_state, to_state, conditions[], duration_ticks,
  interruptible)`, an ordered list of **layers** (weight, `Additive`|`Override`,
  motion binding), and declared **parameters** (name → typed slot index).
- **Runtime (ECS component).** The live mirror Domain B actually reads/writes:
  current state per layer, current clip-local time per layer, layer weights, and
  the parameter block.

The component is a fixed-size `#[repr(C)] Pod`; the *graph* is variable-length and
lives only in the asset. The two are linked by the `controller_asset` handle.

### 2. States, transitions, conditions

**States** are just ids into the controller's asset state list. A state names a
*motion source*: a single clip, or a **blend-tree root**. Every layer has exactly
one `current_state`.

**Transitions** are directed edges with conditions. On each tick, after advancing
the current state's clip time, the system inspects the *outgoing* transitions of
the current state **in the asset's declared (sorted) order** and evaluates each
condition set in order. Conditions compare live parameters:

- `bool` param — `== true/false`, or an *automatic* edge that fires when the
  previous state's motion finished (`time >= duration`).
- `float` param — compare against a threshold/range using fixed-point
  comparison (`>=`, `<=`, `in [lo,hi]`), with a built-in small **hysteresis** so
  `walk→run` at `0.5` flips back to `walk` at `0.45`, preventing jitter near a
  boundary.
- `trigger` param — one-shot; a transition guarded by a trigger **consumes** it on
  fire (see below).

If multiple outgoing transitions could fire, the highest declared priority wins
(deterministic, since order is fixed in the asset). A transition also carries a
**blend duration** (`duration_ticks`): instead of popping instantly, the system
runs a **cross-fade** between the outgoing state's pose and the incoming state's
pose over that many ticks. During the cross-fade the ASM output is the LERP of the
two states' sampled poses (fixed, per spec-36 rules). This is what makes
`idle→walk→run` look continuous rather than snapped.

### 3. Parameters & triggers (runtime component block)

Parameters are declared once in the asset; their *values* live in a fixed block
inside the `AnimatorController` component. Bounded sizes keep the Pod struct a
stable, small multiple of 4 B:

- `FLOAT_PARAMS` (e.g. 16) — `I16F16` each (e.g. `speed`, `move_x`, `move_y`).
- `BOOL_PARAMS` (e.g. 16) — packed as a `u32` bitmask.
- `TRIGGERS` (e.g. 8) — one **latch bit** + one **consume generation** per slot.

A gameplay system writes a parameter via a normal `ColumnWrite` on the
`AnimatorController` component (Domain-B pure) — e.g. a movement system sets
`speed` from `Velocity`. A **trigger** is set with a `DeferredCommand`/param
write; it latches `set=1` and stays latched until an ASM transition consumes it
(resets to 0) on the tick it fires. Because consumption is a deterministic
side-effect *of the ASM decision*, the same tick sequence consumes the same
triggers.

### 4. Blend trees 1D / 2D

A **blend tree** is the motion source of a state, producing a weighted mix of
child clips from one or two parameters, instead of a single clip.

- **1D blend** — one parameter `p` maps to a piecewise set of clips over
  `[0..1]` (e.g. `idle 0.0, walk 0.25, run 1.0`). Adjacent clips are LERP-mixed by
  the fractional position of `p` between their anchors; fixed-point only.
- **2D blend (Cartesian or freeform)** — two parameters `(x, y)` locate a point
  in a plane among several sample clips. Cartesian: clip contributions are
  computed from axis-aligned regions. Freeform (Delaunay / gradient-band)
  contributions need careful fixed-point geometry; Cartesian is the default and
  freeform is a later, opt-in refinement. The resulting per-clip weights are
  normalized to sum to 1, then the state's pose is a weighted fixed LERP/NLERP of
  the child poses (spec 36).

Blend trees are recursive node graphs (a child may itself be a clip, another
blend tree, or a nested blend-space node). They are authored data inside the
controller asset, evaluated by `anim-core` (shared deterministic core from spec
36).

### 5. Layers (additive / override)

A controller can stack **layers**, each an independent mini-ASM over the same
rig, composed onto the base layer's pose:

- **Base layer** — the ground motion (full body) at weight 1.
- **Override layer** — at weight `w`, replaces selected joints with the layer's
  sampled pose (joint mask authored in the asset). Base × (1−w) + layer × w.
- **Additive layer** — a "delta" clip (the difference between a pose and a
  reference/bind pose, e.g. an aim or recoil offset). Contribution =
  base_pose ⊗ (layer_local_delta)^w. In the fixed-pose model this is applied as
  an additive quaternion/translation offset scaled by layer weight.

Each layer has its own current state, clip time, and cross-fade, and its own
weight that gameplay (or a higher state) may drive. Layers are ordered and applied
in declared order; output is a single composed per-joint pose handed to the
skinner.

### 6. IK hooks

The ASM exposes **IK hooks** so gameplay can override effector placement after
the graph produced a pose (two-bone arms/legs, look-at). A hook is a runtime
declaration that certain joints are *IK effectors* whose target comes from
parameters or a `Transform`/`Parent`-tracked entity (spec 21). The **solver** is a
separate deterministic two-bone (or look-at) fixed-point routine in `anim-core`;
it runs *after* ASM output and *before* skinning, in Domain B / anim-core, so IK
is reproducible. This spec only defines the *hook* contract (which joints, target
source, weight); the full solver is an anim-core follow-up (a `16..19` reserved
concern for a dedicated IK component if one becomes an ECS column).

### 7. Tick-driven determinism

Every ASM step is a pure function of `(controller columns, tick)`. Per tick:
1. advance each layer time by `FIXED_STEP`;
2. evaluate outgoing transitions (asset order) against the live parameter block;
3. if a transition fires, set `current_state`, start a cross-fade, consume
   triggers;
4. sample active state (and cross-fade pair) poses via `anim-core`;
5. compose layers / IK and write the final pose.

No wall clock, no ambient RNG, no `HashMap`. Identical initial state + identical
param writes ⇒ identical state sequence, transitions, and pose.

## Key Rust Types

```rust
// crates/ecs/src/components.rs — AnimatorController (ComponentId 14), Pod mirror.
use contracts::{ComponentId, Entity};
use openengine_math::I16F16;

pub const MAX_FLOAT_PARAMS: usize = 16;
pub const MAX_BOOL_PARAMS:  usize = 16;
pub const MAX_TRIGGERS:     usize = 8;
pub const MAX_LAYERS:       usize = 4;

#[repr(C)]
#[derive(Clone, Copy, PartialEq, Eq, bytemuck::Pod, bytemuck::Zeroable,
         serde::Serialize, serde::Deserialize, Debug)]
pub struct AnimatorController {
    pub controller_asset: u32,      // graph asset handle (states/transitions/layers)
    // base layer runtime:
    pub current_state: u32,         // active state id in the base layer
    pub time: I16F16,               // clip-local time (ticks/seconds) in current_state
    pub cross_fade_from_state: u32, // 0 == none; else fading-from state id
    pub cross_fade_from_time: I16F16, // ticks left in the cross-fade
    pub layer_weights: [I16F16; MAX_LAYERS], // per-layer contribution weights
    pub float_params: [I16F16; MAX_FLOAT_PARAMS],
    pub bool_mask: u32,             // BOOL_PARAMS <= 16 → one u32
    pub trigger_latch: u32,         // one bit per trigger slot (set, not yet consumed)
    pub _pad: u64,                  // pad to a clean multiple of 8 for SoA
}
// size_of note: [I16F16;16] = 64 B, [I16F16;4] = 16 B → total multiple of 4.
```

```rust
// crates/anim-core/src/asm.rs — Domain-B-clean decision + blend + layer helpers.
#![forbid(unsafe_code)]

use openengine_math::{I16F16, I32F32, fx};

/// Threshold compare with hysteresis, fixed point (drives walk/run float edges).
pub fn edge_cross(value: I16F16, enter: I16F16, exit: I16F16, was_in: bool) -> bool {
    if was_in { value <= exit } else { value >= enter }
}

/// 1D blend weights: given sorted anchors (clip ids + anchor p) and parameter p,
/// returns the two adjacent clips + a fixed blend fraction in [0,1].
pub fn blend1d(anchors: &[(u32, I16F16)], p: I16F16)
    -> (u32, u32, I16F16) { /* lower id, upper id, t */ }

/// Compose an additive layer: apply a layer delta pose scaled by weight `w` onto
/// a base pose. Pure fixed-point per-joint.
pub fn apply_additive(base: &[JointTrs], layer: &[JointTrs], w: I16F16,
                      mask: &[bool]) -> alloc::vec::Vec<JointTrs> { /* ... */ }
```

```rust
// crates/logic-sandbox/src/systems/animator.rs — Domain B, spec 12 #[system].
#[system(name = "animator_update", schedule = FixedUpdate,
         query = AnimatorController)]
fn animator_update(view: &StateView<'_>) -> Result<WorldDelta, RecoverableError> {
    // 1. read AnimatorController columns via bytemuck::cast_slice
    // 2. for each controller: advance time; test outgoing transitions in asset
    //    order against float/bool/trigger params; on fire update current_state,
    //    start cross-fade, consume (clear) consumed trigger latch bits;
    // 3. build ColumnWrite(s) for time/current_state/cross-fade/trigger_latch.
    // The graph edges themselves are NOT in the view; the host materializes for
    // this system the *currently relevant* transition/condition table (a read-only
    // column the host builds from the controller asset each time the controller or
    // its params change), so the guest stays asset-free and deterministic.
    Ok(WorldDelta::default()) // assembled from ColumnWrites in implementation
}
```

> **Graph-to-guest plumbing.** Because Domain B cannot read the controller asset
> directly, the host (Domain A) maintains a small read-only **eval table** column
> next to each `AnimatorController`: the sorted outgoing-transition/condition
> descriptors for the *current* state, rebuilt lazily when the controller asset
> or the set of live transitions changes. Domain B walks that fixed table, so the
> decision code never needs the asset bytes and remains pure. The blend-tree/layer
> *weights* come out of `anim-core` from the asset graph during the pose
> evaluation stage (host side), while Domain B drives time + state + params.

## Components

| ComponentId | Name                 | Domain | What it is |
|-------------|----------------------|--------|------------|
| 14          | `AnimatorController` | both   | Runtime mirror of an ASM: asset handle + current state/time + cross-fade + layer weights + bounded float/bool/trigger parameter block. |
| 15..19      | *(reserved)*         | —      | Reserved (e.g. dedicated IK-effector component). Not used by this spec. |

State IDs, transitions, conditions, blend-tree nodes, layers, joint masks and
parameter *declarations* are **asset sub-structures inside the
`AnimatorController` asset** — they are *never* separate ECS columns or entities,
so they need no ComponentId. The graph is authored data; only the runtime
evaluation mirror (14) is an ECS component. Component 14 is the only id this spec
assigns.

## Constraints

- **Graph is data inside the controller asset**, never its own ECS columns or
  child entities. `AnimatorController` (14) is a single fixed-size Pod mirror of
  the *runtime* evaluation state + bounded parameter block.
- **Domain B clean:** `animator_update` is a pure `#[system]`; blend/layer/decision
  helpers live in `anim-core` (`no_std`, `forbid(unsafe_code)`). No `f32`, no
  `HashMap` (transitions/conditions iterate in declared asset order), no
  `std::time`.
- **Tick-driven:** state/time advance by `FIXED_STEP` per tick; loop/finish edges
  keyed off clip duration in ticks. Determinism holds because every step is a pure
  function of columns + tick.
- **Deterministic transition order:** outgoing transitions are evaluated in the
  asset's declared order; ties resolved by declared priority, then state order.
  Hysteresis (float) prevents boundary jitter without randomness.
- **Trigger one-shot semantics:** a trigger latches and is consumed exactly once by
  the transition it fires; consumption is deterministic per tick.
- **Cross-fades are fixed LERP over an integer tick count** — a finite, closed
  transition, never "until converged" (mirrors spec 13's fixed-iteration rule).
- **Layer composition & IK are pure anim-core passes** applied in fixed order after
  the graph decision and before skinning (spec 36). Override masks and IK targets
  are authored in the asset / runtime hooks, not ad-hoc.
- Portability to `x86_64-linux`/`aarch64-linux`; headless (no GPU);
  `verify-wasm-purity` reports `[PURE]`.

## Performance Targets

- Decision step per controller per tick: evaluating all outgoing transitions of
  the current state + advancing time + trigger handling **< ~1 µs** (a handful of
  fixed compares over a small sorted table).
- Cross-fade bookkeeping: O(1) per active fade.
- Blend-tree weight computation: 1D O(clips), 2D Cartesian O(clips) with
  normalization; **< 2 µs** for a typical `(idle, walk, run, sprint)` node.
- Layer composition + IK hooks: fixed passes over `joint_count`;
  **< 3 µs/joint** aggregate with reused buffers, no per-tick alloc.
- Whole `animator_update` over, say, 200 animated characters stays **well under
  the 16 ms Domain-B tick budget**.

## Testing Strategy

- **Decision determinism:** given a fixed controller asset and a scripted sequence
  of param writes, drive the ASM for N ticks **3×** and assert identical
  `current_state`, time, and fired/consumed-trigger sequences.
- **Transitions unit:** idle→walk→run over a float param `speed` with hysteresis;
  assert the exact enter/exit thresholds and no toggling at the boundary.
- **Trigger lifecycle:** set a trigger → transition consumes it exactly once on the
  fire tick; a second evaluation sees it clear; un-consumed triggers persist.
- **Cross-fade:** assert output pose over `duration_ticks` equals the fixed LERP of
  the two states' poses at each tick (golden).
- **Blend tree:** 1D anchors and 2D Cartesian produce the expected clip weights and
  normalized fixed sum at sample points.
- **Layers:** override replaces masked joints at weight `w`; additive applies the
  scaled delta; composition order is exact.
- **Graph-is-data constraint test:** assert the ASM asset round-trips through the
  controller asset (spec 02/16) and that *no* per-state or per-transition entity
  or column is ever spawned (guard test).
- **Parameter plumbing:** a movement system sets `speed`/`float_params` via
  `ColumnWrite`; assert `animator_update` reads the new value and transitions
  deterministically.
- **Purity gate + headless replay + cross-platform** bit-identity per spec 36's
  battery.

## Dependencies

- `openengine-anim-core` from spec 36 (`JointTrs`, `sample_track`,
  `local_to_world`, plus new ASM blend/layer helpers).
- `contracts`, `openengine-math` (`I16F16`, `I32F32`), `bytemuck`, `serde`
  /`postcard`, `alloc`.
- Asset pipeline (spec 02) for the controller asset; animation editor (spec 38)
  authors the graph; the GPU/CPU skinner (spec 36) consumes the composed pose.
- Domain-B `#[system]` plumbing from spec 12.

## Next Steps

1. Register component 14 (`AnimatorController`) + layout tests.
2. Define the controller asset schema (states, transitions, conditions, layers,
   blend trees, parameter declarations) and a Domain-A loader.
3. Implement `anim-core` ASM helpers (hysteresis, blend1d/2D, additive/override,
   cross-fade LERP).
4. Build the host "eval-table" read column that materializes the current state's
   transitions/conditions for Domain B.
5. Implement the `animator_update` system + `logic-export` trampoline + parameter
   write plumbing.
6. Add IK-hook contract and (follow-up) two-bone/look-at solvers in anim-core.
7. Determinism + purity + cross-platform + replay test battery.

---
