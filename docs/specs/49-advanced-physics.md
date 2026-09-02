---
spec: "49-advanced-physics"
phase: "Phase 5: Advanced"
status: "draft"
author: "OpenEngine AI"
created: "2026-09-03"
depends_on:
  - "00-ecs-architecture"
  - "05-time-system"
  - "12-scripting-macros"
  - "13-physics-basics"
  - "16-serialization"
  - "21-primitive-components"
---
# 49 - Advanced Physics (Rigid Bodies, Constraints, Materials)

## Overview

This spec extends spec 13's **deterministic basic physics** (AABB/circle
broadphase + narrowphase + impulse response) into a full **rigid-body** layer,
still **entirely inside Domain B as pure systems** and still **deterministic and
fixed-point only**. It adds:

* **Rigid bodies** with mass, inertia, velocity and angular-velocity
  integration (spec 13 handled kinematic `Position`/`Velocity` without true
  rigid-body inertia).
* **Physical materials** (friction, restitution, density) as first-class
  data used by contact resolution and joints.
* **Colliders** as a proper typed component (shape kind, half-extents/radius,
  offsets, per-shape filter).
* **Constraints / joints** — revolute (hinge), prismatic (slider), distance,
  spring (damped harmonic) and motor drives — solved iteratively on the fixed
  tick.

Everything runs on the **fixed 60 Hz timestep (spec 05)**, on the **CPU**, in a
pure system of the canonical shape `fn(&StateView) -> Result<WorldDelta,
RecoverableError>`. There is no `f32` in any physics math, no `HashMap`
iteration, no wall clock, and no GPU. Identical inputs — same `StateView`, same
tick, same injected seed — produce bit-identical positions, velocities, contact
order, and joint impulses on `x86_64-linux`, `aarch64-linux`, and in Wasm.

The design inherits spec 13's choices (sorted `Vec` broadphase, fixed iteration
count, fixed-point `I16F16` widening to `I32F32` for squares) and composes them
into the solve loop of a classic sequential-impulse rigid-body solver, adapted to
the constraint that **every coefficient and every result is a fixed-point
`Pod` value that round-trips deterministically**. Where spec 13 read and wrote
`Position`/`Velocity` and a `PhysicsMaterial` column, this spec owns the new
components `RigidBody` (60), `Collider` (61), `PhysicsMaterial` (62), and
`Joint` (63) registered under the reserved built-in ids 60–63 (see
"Components").

## Core Concepts

### Component split: RigidBody vs Collider vs Material vs Joint

The engine separates *what the body is* (`RigidBody`), *what shape it has*
(`Collider`), *what its surface is like* (`PhysicsMaterial`), and *what links
it to others* (`Joint`). This is deliberate: a single entity may own a
`RigidBody` and several `Collider`s; many bodies may share one
`PhysicsMaterial` by id; joints are their own entities or second-entity edges.
Archetype shapes therefore vary (spec 00 migration), and every system queries
the columns it needs via `StateView`.

| `ComponentId` | Name              | Domain role |
|---------------|-------------------|-------------|
| 60            | `RigidBody`       | mass, inertia, velocities, gravity scale, damping, body type |
| 61            | `Collider`        | one shape (tagged union payload), local offset, filter |
| 62            | `PhysicsMaterial` | friction, restitution, density coefficients |
| 63            | `Joint`           | constraint between two bodies (revolute/prismatic/distance/spring/motor) |

### RigidBody — mass / inertia / integration

`RigidBody` holds the dynamic state the solver integrates. Two-axis (2D) rigid
motion uses linear `Position`/`Velocity` (from spec 21) plus a scalar angular
`velocity_z` and orientation stored in `RigidBody` (the engine's gameplay
physics is 2D-first per spec 13; the 3D `Transform` path is render-side). The
solver works in fixed-point:

* Mass properties are stored as **inverse mass** and **inverse inertia** (both
  `I16F16`). Inverse forms make static bodies (`inverse_mass == 0`) trivially
  immovable, matching spec 13's convention, and keep impulse division cheap.
  `mass`/`inertia` are authored (density × collider area) and inverted at
  registration; `inverse_* == 0` ⇒ static/immovable.
* **Integration** is a semi-implicit (symplectic) Euler on the fixed step:
  `v += (g * gravity_scale) * dt` then `x += v * dt`, with a **fixed** `dt`
  derived from the 60 Hz tick (spec 05), never a wall-clock dt. Angular follows
  `ω += (τ * inverse_inertia) * dt; θ += ω * dt`.
* Damping (linear/angular) is applied as fixed-point decay per tick. Restitution
  and friction come from the colliding pair's materials (below).

Because `dt = fx!(1/60)` is a compile-time exact constant and integration is a
pure function of the injected tick, two runs on the same tick sequence agree
bit-for-bit.

### Collider — shape + filter

`Collider` is the typed successor to spec 13's `Aabb`/`Circle` sketches: one
`#[repr(C)] Pod` struct whose `shape` field selects a small fixed payload
(axis-aligned box half-extents, or circle radius — spec 13's two primitives) plus
a local offset and a `CollisionFilter { layer, mask }` (spec 13). Broadphase
feeds a collider's world-space bounds; narrowphase tests exact overlap between
the two colliders' shapes. A `sensor` flag makes a collider detect overlap
without producing contact resolution impulses (events only) — useful for
triggers and reach.

### PhysicsMaterial — friction / restitution / density

`PhysicsMaterial` holds the surface coefficients the contact solver reads. To
keep one material per entity but allow many-to-one reuse, the `Collider` carries
a `material: u64` handle resolved host-side to a `PhysicsMaterial` row (an asset
per spec 02) — or the author may embed a fixed material inline. Contact response
combines the two bodies' materials deterministically:

```rust
// Fixed-point combine rules — identical on every platform (no min-max float).
// Reference (Newton-like): friction = sqrt(fa*fb) but via fixed sqrt helper;
// restitution = max(ea, eb). Both are pure I16F16 functions.
fn combine_friction(a: I16F16, b: I16F16) -> I16F16 { /* fixed */ }
fn combine_restitution(a: I16F16, b: I16F16) -> I16F16 { /* fixed */ }
```

Determinism is preserved because the combine is a fixed function of two fixed
coefficients — never an f32 heuristic whose rounding differs across CPUs.

### Broad + narrow phase (from spec 13)

Spec 13 already defines the deterministic uniform-grid broadphase (sorted
`Vec<GridEntry>` by `(cell_key, entity.index)`) and fixed-point narrowphase
(AABB/AABB, circle/circle via widened `I32F32` squares). This spec reuses those
exactly for rigid bodies: a body's collider is stamped into grid cells, candidate
pairs are enumerated in sorted order, and narrowphase emits contacts with
contact points, normals, and penetration depth. Contact order is a pure function
of world state — the solver's determinism anchor.

### Collision response — sequential impulses with restitution/friction

Response is the standard sequential-impulse approach, but every number is
fixed-point and iteration counts are fixed integers (never "until converged",
spec 13). For each contact in deterministic order, for a **fixed iteration
count** (e.g. 4 positional, 8 velocity iterations — compile-time constants), the
solver accumulates normal impulses (using restitution for the normal bounce) and
tangent impulses (using the combined friction) into a per-contact accumulator,
then writes corrected velocities and positional corrections back through
`WorldDelta::ColumnWrite`. The accumulator is per-(entity, contact) scratch that
is reset at the start of each tick, so the solve is deterministic.

### Constraints / Joints — revolute, prismatic, distance, spring, motor

`Joint` is the *data* (the constraint equation and its fixed coefficients); the
solver *is* the code that enforces it. Joints are entities carrying a `Joint`
component (id 63) whose fields name the two bodies (`body_a`, `body_b`, both
`Entity`) and a `kind` selecting the constraint. Each kind is solved as one or
more fixed-point positional/velocity constraints in the same iterative loop:

* **Revolute (hinge)** — keeps two anchor points coincident; exposes a motor
  drive for rotation.
* **Prismatic (slider)** — constrains relative translation to one axis; optional
  motor along that axis and a limit range.
* **Distance** — keeps the bodies at a fixed separation; modeled as a positional
  constraint or a stiff spring.
* **Spring** — damped harmonic oscillator between anchors (stiffness + damping,
  fixed-point), for soft bodies / suspension.
* **Motor** — a velocity source (target relative angular or linear velocity) with
  a maximum impulse ("torque/force limit").

Each joint writes velocity corrections and optional positional corrections back
into the two bodies' `RigidBody`/`Velocity` columns via the delta. Solver order
is a sorted joint list, so joint impulse order is deterministic.

### The physics step as pure systems

Consistent with spec 13, the phase runs as a chain (or one fused step) of pure
systems on `FixedUpdate`:

```
 fixed tick (spec 05) → build StateView (RigidBody/Collider/PhysicsMaterial/
                        Position/Velocity/Joint columns of relevant archetypes)
  1. integrate_system        : apply gravity + semi-implicit integration to each RigidBody
  2. broadphase_system       : uniform grid, sorted entries (spec 13)
  3. narrowphase_system      : contact generation (spec 13 + rigid contact points)
  4. solve_system            : sequential impulses (contacts) + joint constraints,
                               fixed iteration counts
  5. cleanup_system          : emit CollisionEvent deferred commands; reset accumulators
 host applies deltas atomically; flush spawn/despawn
```

Every subsystem is a pure `fn(&StateView) -> Result<WorldDelta, RecoverableError>`
registered through spec 12's `#[system]` machinery, so the host drives them
generically through the wasmtime bridge exactly as it drives any system. This
keeps the entire advanced-physics solver a **Domain-B CPU feature**: no GPU, no
host import, fully rollback-able (spec 15) and snapshot-able (spec 16).

## Key Rust Types

```rust
//! crates/logic-sandbox/src/physics.rs  (Domain B) — re-exports + pure solver.
//! Domain A registers the components (spec 21) and stores them as Pod columns.
#![forbid(unsafe_code)]

use contracts::{ComponentId, Entity};
use openengine_math::{fx, I16F16, I32F32};

/// ComponentId bindings — reserved built-in range 60–69 (this spec owns 60–63).
pub const C_RIGID_BODY: ComponentId         = ComponentId(60);
pub const C_COLLIDER: ComponentId           = ComponentId(61);
pub const C_PHYSICS_MATERIAL: ComponentId   = ComponentId(62);
pub const C_JOINT: ComponentId              = ComponentId(63);

/// RigidBody — dynamic state + mass properties. Fixed-point Pod.
#[repr(C)]
#[derive(Clone, Copy, PartialEq, Eq, bytemuck::Pod, bytemuck::Zeroable,
         serde::Serialize, serde::Deserialize)]
pub struct RigidBody {
    pub inverse_mass: I16F16,      // 0 => static/immovable
    pub inverse_inertia: I16F16,   // 0 => no rotation (angular locked)
    pub velocity_z: I16F16,        // angular velocity (rad/tick), 2D rotation
    pub gravity_scale: I16F16,     // 1 == normal gravity
    pub linear_damping: I16F16,    // per-tick fixed decay
    pub angular_damping: I16F16,
    pub kind: BodyKind,            // Static=0, Dynamic=1, Kinematic=2 (u8)
    pub _pad: [u8; 2],
    pub position: Position,        // reused primitive (spec 21); in-column copy
}

#[repr(u8)]
#[derive(Clone, Copy, PartialEq, Eq, bytemuck::Pod, bytemuck::Zeroable,
         serde::Serialize, serde::Deserialize)]
pub enum BodyKind { Static = 0, Dynamic = 1, Kinematic = 2 }

/// Collider — one shape + filter + material handle. Fixed-point Pod.
#[repr(C)]
#[derive(Clone, Copy, PartialEq, Eq, bytemuck::Pod, bytemuck::Zeroable,
         serde::Serialize, serde::Deserialize)]
pub struct Collider {
    pub shape: ColliderShape,      // u8: Box=0, Circle=1
    pub _pad0: [u8; 3],
    pub half_extent_x: I16F16,     // box half-width (circle: 0)
    pub half_extent_y: I16F16,     // box half-height (circle: 0)
    pub radius: I16F16,            // circle radius (box: 0)
    pub offset_x: I16F16,          // local offset from body origin
    pub offset_y: I16F16,
    pub layer: u32,                // collision layer
    pub mask: u32,                 // collide iff (mask & other.layer) != 0
    pub material: u64,             // logical handle to a PhysicsMaterial row (0 = default)
    pub sensor: u8,                // 1 => overlap events only, no impulse
    pub _pad1: [u8; 3],
}

#[repr(u8)]
#[derive(Clone, Copy, PartialEq, Eq, bytemuck::Pod, bytemuck::Zeroable,
         serde::Serialize, serde::Deserialize)]
pub enum ColliderShape { Box = 0, Circle = 1 }

/// PhysicsMaterial — surface coefficients. Fixed-point Pod.
#[repr(C)]
#[derive(Clone, Copy, PartialEq, Eq, bytemuck::Pod, bytemuck::Zeroable,
         serde::Serialize, serde::Deserialize)]
pub struct PhysicsMaterial {
    pub friction: I16F16,       // [0,1] fixed
    pub restitution: I16F16,    // [0,1] fixed
    pub density: I16F16,        // mass-per-area for auto inertia
    pub _reserved: u32,         // pad -> 16 B
}

/// Joint — a constraint edge between two bodies. Fixed-point Pod.
#[repr(C)]
#[derive(Clone, Copy, PartialEq, Eq, bytemuck::Pod, bytemuck::Zeroable,
         serde::Serialize, serde::Deserialize)]
pub struct Joint {
    pub body_a: Entity,
    pub body_b: Entity,
    pub kind: JointKind,         // Revolute/Prismatic/Distance/Spring/Motor
    pub enabled: u8,
    pub _pad: [u8; 2],
    pub local_anchor_a: (I16F16, I16F16),
    pub local_anchor_b: (I16F16, I16F16),
    pub axis_x: I16F16,          // prismatic/motor axis direction
    pub axis_y: I16F16,
    pub reference_angle: I16F16, // revolute offset
    pub limit_min: I16F16,       // prismatic/distance range
    pub limit_max: I16F16,
    pub stiffness: I16F16,       // spring
    pub damping: I16F16,         // spring / damper
    pub motor_speed: I16F16,     // target relative velocity
    pub max_motor_force: I16F16, // impulse limit
}

#[repr(u8)]
#[derive(Clone, Copy, PartialEq, Eq, bytemuck::Pod, bytemuck::Zeroable,
         serde::Serialize, serde::Deserialize)]
pub enum JointKind {
    Revolute = 0,
    Prismatic = 1,
    Distance = 2,
    Spring = 3,
    Motor = 4,
}
```

> The component layouts above keep every struct a multiple of 4 bytes for clean
> SoA columns (spec 00) and all scalars `I16F16`. `_pad`/`_reserved` cells are
> explicit so `size_of`/`align_of` are fixed and match the registered
> `element_size` (spec 16). Final field ordering and sizes are finalized when the
> layout tests in "Testing" are landed.

### Solver entry (pure system)

```rust
pub fn solve_contacts_and_joints(view: &StateView<'_>)
    -> Result<WorldDelta, RecoverableError>
{
    let mut delta = WorldDelta::default();
    // 1. read RigidBody/Position/Velocity/Collider/PhysicsMaterial/Joint columns
    //    via view.column(C_RIGID_BODY) etc. (bytemuck::cast_slice).
    // 2. broadphase + narrowphase (spec 13) produce sorted contacts.
    // 3. for _ in 0..VELOCITY_ITERATIONS { for c in &contacts { apply_impulse } }
    // 4. for _ in 0..POSITION_ITERATIONS { for j in &joints { solve_joint } }
    // 5. assemble ColumnWrite for RigidBody/Position/Velocity; append deferred
    //    CollisionEvent commands; return delta.
    Ok(delta)
}
```

## Components

All four physics components are registered under the reserved built-in ids
60–69. This spec owns 60–63 (spec 48 owns 64, and 65–69 remain reserved — no
other spec claims them without updating this reservation). Registration is once
and immutable (spec 00/21):

| `ComponentId` | Name              | size_of (target) | Domain role |
|---------------|-------------------|------------------|-------------|
| 60            | `RigidBody`       | 40 (16 B mass/props + 8 B Position + pad) | body dynamic state, mass/inertia, integration |
| 61            | `Collider`        | 48 | shape, local offset, filter, material handle, sensor |
| 62            | `PhysicsMaterial` | 16 | friction, restitution, density |
| 63            | `Joint`          | 80 | constraint edge: kind, anchors, limits, spring/motor |

Exact `size_of`/`align_of` are pinned by the layout tests (multiple of 4 for SoA
cleanliness, matching spec 00/21 conventions). `RigidBody` reuses `Position`; the
engine also stores `Velocity` (id 1, spec 21) for linear velocity.

## Constraints

- **Deterministic and fixed-point only (AGENTS.md §3).** No `f32` anywhere in
  physics math; squared distances / inertia products widen to `I32F32` then
  reduce at write boundaries (spec 13). Compile-time constants for `dt`,
  iteration counts, gravity.
- **Fixed timestep (spec 05).** Physics advances only on whole 60 Hz ticks via
  injected `view.tick`; never wall clock, never a runtime `dt`.
- **Pure Domain-B systems.** Every subsystem is `fn(&StateView) ->
  Result<WorldDelta, RecoverableError>` registered via spec 12. No host import,
  no GPU, no I/O, no threads, no wall clock, no ambient RNG. All memory
  bridging via `bytemuck::cast_slice` (never `transmute`).
- **Deterministic order.** Sorted broadphase grid, sorted contact list, sorted
  joint list, fixed iteration counts; no `HashMap` iteration, no "converge until
  tolerance" float loops.
- **Static/dynamic split by `inverse_mass`/`inverse_inertia` == 0.** Static
  bodies never move; kinematic bodies move only by authored inputs, never by
  impulses.
- **Materials are deterministic combine functions** of fixed coefficients.
- **`#[repr(C)] Pod + Zeroable + serde`** on all four components so they travel
  in SoA columns and cross the spec 16 codec unchanged.
- Portable on `x86_64-linux`, `aarch64-linux`, and in Wasm; no OS-specific code.

## Performance Targets

- Broadphase + narrowphase for 10k colliders: reuse spec 13's < ~3 ms/tick target.
- Contact solve (sequential impulses, fixed iterations) on a few hundred
  simultaneous contacts: within the ≤ 16 ms/tick Domain-B budget; typical scenes
  far below.
- Joint solve: per-joint fixed-point constraint work is ~O(1); thousands of
  joints fit the tick budget.
- No allocations in the hot solve loop (scratch buffers reused across ticks;
  sorted `Vec` reuse, spec 13). Module stays ≤ 256 MB.
- Determinism re-check: identical tick ⇒ bit-identical velocities/positions.

## Testing Strategy

- **Layout tests:** `assert_eq!(size_of::<T>(), N)` and `assert_eq!(align_of)`
  for RigidBody/Collider/PhysicsMaterial/Joint; assert each is `Pod + Zeroable`
  and a multiple of 4 bytes.
- **Unit:** mass/inertia inversion, material combine functions, gravity
  integration of a single body over N fixed ticks (closed-form check), damping
  decay.
- **Integration:** a falling box onto a static floor settles and rests (no
  jitter, no tunnelling at moderate speeds); stacked boxes and stacked circles
  settle deterministically; a restitution test bounces to a repeatable apex.
- **Constraints:** a revolute hinge keeps two boxes connected through a spin; a
  prismatic slider constrains one axis and a motor drives it to a target speed; a
  spring reaches rest length with damping; a distance joint holds a fixed
  separation. All driven headlessly over many ticks.
- **Sensor / filter:** sensors emit overlap events without impulses; layer/mask
  gating collides exactly the intended pairs.
- **Determinism 3×:** run a scripted rigid-body scene (boxes + circles + joints +
  a bouncing ball) **3×** on `x86_64-linux`, `aarch64-linux`, and in Wasm; assert
  bit-identical final positions, velocities, and event order.
- **Rollback/snapshot readiness:** capture a `WorldSnapshot` (spec 16) at tick N,
  re-simulate forward, and confirm the state reproduces (spec 15 rollback path).
- **Purity:** every physics module passes `verify-wasm-purity` → `[PURE]`.
- **Fuzz:** randomized rigid-body scenes compared against an integer reference.

## Dependencies

- `openengine-math` (`I16F16`, `I32F32`, `fx!`, integer sqrt) — extended as
  needed for inertia/moment computations.
- `contracts` (`StateView`, `WorldDelta`, `Entity`, `ComponentId`, `ColumnWrite`,
  `DeferredCommand`, `RecoverableError`).
- `bytemuck`, `serde`, `postcard`, `alloc`. Reuses spec 13's broad/narrowphase
  and spec 21's `Position`/`Velocity` primitives.
- Registered via spec 12 (`#[system]`) and driven by the host wasmtime bridge;
  snapshot/rollback via spec 16/15.
- Domain A registers the components + stores `PhysicsMaterial` assets (spec 02);
  the solver itself is pure Domain-B.

## Next Steps

1. Add the four components (`RigidBody`/`Collider`/`PhysicsMaterial`/`Joint`)
   with the id bindings above and layout tests.
2. Implement semi-implicit fixed-tick integration for `RigidBody`.
3. Port spec 13's broad/narrowphase to rigid contact points (world-space AABB,
   contact normal/depth) reusing its deterministic order.
4. Implement sequential-impulse contact solver (normal restitution + friction)
   with fixed iteration counts.
5. Implement the five joint kinds as positional/velocity constraints in the same
   loop, with fixed-point spring/motor drives.
6. Add deferred `CollisionEvent` emission (for spec 48 event nodes and gameplay).
7. Land the determinism-3×, purity, rollback, sensor/filter, and joint test
   suites.
