---
spec: "13-physics-basics"
phase: "Phase 6"
status: "design"
---

# Deterministic Basic Physics

## Overview

A deterministic 2D rigid-shape collision layer built entirely inside Domain B
as pure systems. It handles **AABB and circle** shapes, **broadphase** via a
uniform spatial grid with a fully deterministic iteration order, **narrowphase**
contact generation, **impulse-based** positional/corrective response, and
**raycasting** against world shapes. All arithmetic uses `openengine-math`
fixed-point (`I16F16`); there is no `f32` anywhere in the physics math and no
floating jitter. Identical inputs — same `StateView`, same tick, same injected
seed — produce bit-identical positions and collision events on every platform.

This spec is the substrate for later specs: `15-networking` relies on
deterministic physics for rollback, and `16-serialization` snapshots the
position columns produced here.

## Design

### Domain split

Physics runs entirely on the read side of the ECS. A physics system reads a
[`StateView`] describing the `Position`/`Velocity` columns (base primitives, spec
21) plus the physics components `RigidBody`(60), `Collider`(61), and
`PhysicsMaterial`(62) that spec 49 owns and registers, and returns a
[`WorldDelta`] whose `writes`
carry the updated `Position`/`Velocity` columns and whose `deferred` carry
`CollisionEvent`s for gameplay (sounds, damage) and rendering hints. The host
applies the delta atomically. Domain B never mutates memory directly.

```
        fixed timestep tick (60 Hz, tick u64)
                │
   build StateView (Position/Collider/Velocity columns)
                ▼
 ┌─ physics_prep_system  (integrate velocities, apply gravity)
 │   ┌───────────────────────────────┐
 │   │ pure fns -> Result<WorldDelta>│
 │   └───────────────────────────────┘
 │   broadphase_grid.build(grid)     │  deterministic cell ordering
 │   narrowphase(candidate pairs)    │  contact generation
 │   resolve_contacts(impulses)      │  position + velocity writes
 │   raycast_system (deterministic)  │  deferred RayHit events
 └─► host applies delta, flushes spawn/despawn
```

Broadphase, narrowphase and resolution may be separate `PureSystem`s chained in
registration order (see `01-game-loop.md`), each returning its own delta, so the
host applies them in order and determinism is by construction. They may also be
one fused `physics_step` system; the ordering guarantee is identical because all
of them read the post-apply world of the previous step.

### Collision representation

Collision geometry is **not** a set of standalone registered component types.
Spec 13's basic 2D colliders reuse the physics components that spec 49 owns and
that spec 21 registers: `RigidBody` (60, dynamic state + `inverse_mass`),
`Collider` (61, one shape + layer/mask filter + material handle), and
`PhysicsMaterial` (62, friction/restitution/density). They are `#[repr(C)]` +
`Pod` + `Zeroable` so they travel in SoA columns. Fixed-point only.

The two 2D primitives this spec's broadphase/narrowphase reason about —
axis-aligned boxes and circles — are **`Collider` shape variants**, not separate
registered component types:

* `ColliderShape::Box` (0) — an axis-aligned box, stored as `half_extent_x` /
  `half_extent_y` around the collider's local offset. Its world-space AABB (for
  the uniform grid and, on the Domain-A side, spec-24 viewport picking) may be
  mirrored in the registered `Bounds` (71) component.
* `ColliderShape::Circle` (1) — a circle, stored as `Collider.radius`.

The collision categories that older drafts sketched as a standalone
`CollisionFilter { layer, mask }` are now the `Collider`'s own `layer`/`mask`
fields (two bodies collide iff `(mask & other.layer) != 0`), and the mass
property is `RigidBody.inverse_mass` (`0` ⇒ static/immovable). Friction and
restitution come from the referenced `PhysicsMaterial` (62) row. Contact events
surfaced to gameplay are fixed-point values:

```rust
/// Event surfaced to gameplay via DeferredCommand; fixed-point coordinates.
#[derive(Clone, Debug, serde::Serialize, serde::Deserialize)]
pub struct Contact {
    pub entity_a: Entity,
    pub entity_b: Entity,
    pub normal_x: I16F16,   // unit normal pointing from A to B
    pub normal_y: I16F16,
    pub depth: I16F16,      // penetration depth, >= 0
}
```

What matters for this spec is that every value a collision decision depends on is
fixed-point `Pod` data that round-trips through [`WorldDelta::ColumnWrite`]
without reinterpretation. The exact column scheme — typically one `Collider` per
entity — is host layout detail shared with spec 49.

### Broadphase — uniform grid with deterministic ordering

Pairs are gathered into a uniform grid keyed by `(cell_x, cell_y)` computed from
a fixed world cell size (`I16F16`). The grid is intentionally **not** a `HashMap`
(iteration order is non-deterministic); it is a sorted `Vec<CellEntry>` ordered
by `(cell_key: u64, entity.index)` and, for duplicates, entity order. Sorting a
`Vec` by a total order derived only from integer cell coordinates and entity
indices is deterministic across platforms.

```rust
pub struct GridEntry { pub cell: (i32, i32), pub entity: Entity }

pub fn build_grid(view: &StateView<'_>, cell_size: I16F16) -> alloc::vec::Vec<GridEntry> {
    // 1. read each entity's Collider column and derive its world-space
    //    box (ColliderShape::Box → Bounds) or circle span
    // 2. for each entity compute the integer cell span it overlaps
    // 3. push one GridEntry per (cell, entity) — an entity may span cells
    // 4. sort by (cell.0, cell.1, entity.index)
    // Deterministic: only integer arithmetic on fixed values.
}
```

Narrowphase then walks sorted entries in order and, for each cell, tests every
pair of entities that share it (in entity order), skipping `a == b` and
duplicate `(a, b)` re-testing by only emitting pairs with
`entity_a.index < entity_b.index` under the filter test. Contact generation
order is thus a pure function of world state. Pair dedup uses a sorted pair list
or a fixed small bitmap per cell; never a `HashMap`.

### Narrowphase — fixed-point overlap tests

Pure functions, no branching on host float state. `Collider` yields either an
axis-aligned box (half-extents around its offset → a `[min_x, min_y, max_x,
max_y]` range) or a circle (`radius` around its offset). The predicates below
take those primitive values directly rather than named component types — the box
and circle are `Collider` shape data (see "Collision representation"), not
separate registered components:

```rust
/// AABB vs AABB, fixed-point only.
pub fn overlap_boxes(a_min: &[I16F16; 2], a_max: &[I16F16; 2],
                     b_min: &[I16F16; 2], b_max: &[I16F16; 2]) -> bool {
    a_min[0] <= b_max[0] && b_min[0] <= a_max[0] &&
    a_min[1] <= b_max[1] && b_min[1] <= a_max[1]
}

/// Circle vs circle: distance^2 vs (r1+r2)^2 avoids a square root in the
/// rejection test; only the resolved contact needs one sqrt, done with an
/// integer sqrt helper.
pub fn circles_overlap(a_c: &[I16F16; 2], a_r: I16F16,
                       b_c: &[I16F16; 2], b_r: I16F16) -> bool {
    let dx = b_c[0] - a_c[0];
    let dy = b_c[1] - a_c[1];
    let rr = a_r + b_r;
    dx * dx + dy * dy <= rr * rr
}
```

Squares of `I16F16` overflow the 16 fraction bits quickly, so geometric tests
that need squared distances promote to the wider fixed type
`openengine-math::I32F32` (or compare in integer sub-cell units) and only reduce
back to `I16F16` at the write boundary. Overlap/penetration along an axis uses
fixed `min`/`max` arithmetic, which is exact in `I16F16` because it is
comparison and subtraction only (the error budget of I16F16 — 16 fraction bits,
~1.5e-5 — is acceptable for axis separations, and identical on every CPU).

### Response — positional correction + impulse

Contact resolution writes corrected positions and velocities back through
`WorldDelta`. It is deterministic given the same contact set and same entity
order, so it must process contacts in a fully sorted order and resolve each with
a fixed iteration count (e.g. 4 iterations) — never "until converged" using a
host float tolerance, which could diverge across CPUs.

```rust
pub fn resolve_contacts(contacts: &[Contact], view: &StateView<'_>)
    -> Result<WorldDelta, RecoverableError> {
    let mut delta = WorldDelta::default();
    // contacts are already in deterministic order from narrowphase.
    for _ in 0..POSITION_ITERATIONS {          // fixed integer count
        for c in contacts {                     // sorted order
            // 1. positional correction: separate by c.depth along c.normal
            //    split by RigidBody.inverse_mass (0 = static, takes all)
            // 2. impulse from relative velocity along normal, restitution e
            //    accumulate into a fixed-point velocity column write
        }
    }
    // assemble ColumnWrite for Position and Velocity columns; append
    // deferred Contact events for gameplay consumers.
    Ok(delta)
}
```

Static bodies are entities whose `RigidBody.inverse_mass == 0`; they never
move. Positional correction splits the penetration displacement by
`inverse_mass / (inv_a + inv_b)` exactly as in integer-scaled fixed arithmetic.
Restitution and friction are `I16F16` coefficients read from the pair's
`PhysicsMaterial` (62) rows — constants, not floats.

### Raycasting

A deterministic raycast system casts a segment against every candidate returned
by a coarse grid walk (Amanatides–Woo traversal using only integer/fixed cell
arithmetic), and returns the earliest hit by a fixed-order comparison. Because
iteration order is sorted, ties resolve identically everywhere.

```rust
/// Returns closest hit <= max_dist, or None. Fixed point only.
pub fn raycast(view: &StateView<'_>, origin: (I16F16, I16F16),
               dir: (I16F16, I16F16), max_dist: I16F16)
    -> Option<RayHit> {
    // 1. walk the uniform grid cells the ray crosses (integer traversal)
    // 2. for each candidate, segment/ray vs AABB or circle test (fixed)
    // 3. keep the smallest t >= 0 and t <= max_dist
    // deterministic: grid order + candidate order are sorted
}
```

Ray hits surface to gameplay as a `DeferredCommand::Emit { topic, data }` or a
dedicated contact event so the host can draw a debug gizmo; the ray result itself
is a pure value used by Domain B targeting AI.

## Key Rust / types

- Physics components are **reused** from spec 49 (registered in spec 21, never
  redefined here): `RigidBody`(60, holds `inverse_mass`/`inverse_inertia`/`kind`),
  `Collider`(61, one `ColliderShape` variant `Box`/`Circle` + `layer`/`mask` +
  `material` handle), `PhysicsMaterial`(62). There are **no** standalone `Aabb` /
  `Circle` / `CollisionFilter` / `Mass` component types; an axis-aligned box is
  `ColliderShape::Box`, and world-space bounds use `Bounds`(71). All are
  `#[repr(C)] Pod + Zeroable`.
- `Position`(0)/`Velocity`(1) use `I16F16` x/y as established by the ABI (spec 21).
- `fn physics_step(&StateView) -> Result<WorldDelta, RecoverableError>` plus
  helper pure fns, matching the `PureSystem` shape so `crates/core` drives them
  generically through wasmtime.
- `RayHit`/`Contact` travel as `serde`-serializable values (never raw `f32`).

## Constraints

- **No `f32` in any physics math.** Square distances widen to `I32F32` then
  return to `I16F16` only at a write boundary.
- **Deterministic iteration**: sorted grid entries, sorted contact order, fixed
  iteration counts, no `HashMap`, no `std::time`, no ambient RNG.
- All geometry must be representable exactly in `I16F16`; scene authors author in
  quantized fixed units, not raw floats.
- Static/dynamic split by `RigidBody.inverse_mass`; zero inverse mass ⇒
  unmovable.
- No GPU, no I/O, no network in Domain B physics.
- Identical inputs ⇒ identical contact order, impulses, and final positions on
  `x86_64-linux` and `aarch64-linux` and in Wasm.

## Performance

- Broadphase grid build: `O(n)` inserts into sorted entries + sort
  (`O(cells·k log k)` per span); amortised under a few hundred ns per entity.
- Narrowphase: only overlapping cells; pair tests are cheap fixed-point
  comparisons.
- Resolution: fixed iteration count (default 4), no host tolerance loop.
- Domain B budget: physics system fits within `≤16 ms/tick` and `≤256 MB/module`
  constraints with no allocations in the hot loop (reuse grid buffers; sorting
  uses guest-owned `alloc` scratch reused across ticks).
- Target: 10k colliders → broadphase+narrowphase+resolve under ~3 ms/tick.

## Testing strategy

- Unit: `overlap_boxes`, `circles_overlap`, `raycast` hit/miss/tie, correction
  split by mass.
- Integration: falling stack (AABB) settles and stays resting (no jitter);
  circles stack deterministically.
- Determinism: run a scripted scene (boxes + circles + rays) **3×** on two
  platforms / in Wasm and assert bit-identical final positions and event order.
- Rollback readiness: record snapshots at tick N and re-simulate (per
  `15-networking.md`) to confirm a loaded state reproduces the same outcome.
- Fuzz: randomized box/circle scenes compared against an integer reference.
- Purity: `verify-wasm-purity` must report `[PURE]`.

## Dependencies

- `openengine-math` (`I16F16`, `I32F32`, `fx!`, integer sqrt helper).
- `contracts` (`StateView`, `WorldDelta`, `Entity`, `ColumnWrite`,
  `DeferredCommand`, `RecoverableError`, `code`).
- `bytemuck`, `serde`, `postcard`, `alloc`. No new host crates required for the
  physics itself; the grid lives entirely in Domain B.
- Host `crates/core` only needs the existing wasmtime + apply-delta path.

## Next steps

1. Consume the registered physics components (`RigidBody`/`Collider`/
   `PhysicsMaterial` from spec 49/21) and the base `Position`/`Velocity`; do not
   introduce new collider component types.
2. Implement deterministic uniform-grid broadphase + narrowphase in
   `crates/logic-sandbox` (or a `crates/physics` no_std sub-crate that
   `logic-sandbox` re-exports).
3. Implement positional correction + impulse response with fixed iteration count.
4. Implement deterministic raycasting.
5. Add deterministic simulation tests + purity + cross-platform CI.
6. Layer `15-networking` rollback and `16-serialization` snapshots on top.
