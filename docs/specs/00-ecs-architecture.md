---
spec: "00-ecs-architecture"
phase: "Phase 3+"
status: "design"
---

# ECS Architecture

## Overview

Structure-of-Arrays ECS with archetypes, optimized for zero-copy Wasm bridging
and deterministic simulation. All storage is host-side (Domain A,
`crates/ecs`); Domain B only ever reads a [`StateView`] and returns a
[`WorldDelta`].

## Core Concepts

### Entity
- Stable handle: `(generation: u32, index: u32)` — the [`Entity`] ABI type.
- `generation` increments on slot recycle (ABA protection).
- Never reused within the same frame.

### Component
- Plain data: `#[repr(C)]`, `Pod`, `Zeroable` (via `bytemuck`), and
  `serde`-serializable so it can cross as a wire type.
- Registered in a global component registry with a unique [`ComponentId(u32)`].
- `size_of::<T>()` should be a multiple of 4 for clean column layout.
- Domain B math uses fixed-point only (`openengine-math::I16F16`), never `f32`.

### Archetype
- A fixed tuple of component types: `(Position, Velocity, Sprite)`.
- Each archetype owns its own SoA storage.
- Entities move between archetypes when components are added/removed.

### SoA Storage
```rust
pub struct ArchetypeStorage {
    component_ids: Vec<ComponentId>, // which components this archetype holds
    columns: Vec<Column>,            // one per component
    entities: Vec<Entity>,           // live entities in this archetype
    capacity: usize,                 // allocated element slots
    len: usize,                      // used slots
}

pub struct Column {
    component_id: ComponentId,
    data: Vec<u8>,        // raw bytes: len * element_size
    element_size: u32,    // size_of::<T>()
}
```

## Memory Layout

### Column Data
- Contiguous `[T0, T1, …, Tn]`; byte size `len * element_size`.
- Column base is aligned to `align_of::<T>()`; produced views are read via
  `bytemuck::cast_slice` (never `transmute`).

### Entity indexing
- Each entity maps to `(archetype_index, row_index)`.
- Lookup:
  `world.archetypes[entity.archetype_index].entities[entity.row_index]`.
- Row indices are stable within a frame (no reallocation mid-iteration).

## Operations

### Spawn
1. Find (or lazily create) the archetype matching the requested components.
2. Allocate a row; initialize columns (zeros / defaults).
3. Return the [`Entity`] handle.

### Despawn
1. Locate `(archetype, row)`.
2. Swap-remove: move the last row into the freed slot.
3. Update the moved entity's `row_index`.
4. Decrement `len`. Deferred to the end of the tick (see Game Loop).

### Add / Remove component
1. Compute the target archetype (`current ± component`).
2. Migrate the entity: copy all kept columns, (re)add/remove the one column.
3. Update the entity's archetype pointer.
Batch these through `WorldDelta` so migration is deterministic.

## Queries

Read query (immutable; used to build a [`StateView`]):
```rust
pub struct Query<'a, T: Component> {
    pub columns: Vec<&'a [T]>,
    pub entities: Vec<&'a [Entity]>,
}
impl<'a, T: Component> Query<'a, T> {
    pub fn iter(&self) -> impl Iterator<Item = (&'a Entity, &'a T)> { /* ... */ }
}
```
A `QueryMut` mirrors this with `Vec<&'a mut [T]>` for host-side systems that may
write directly (Domain A tools like the editor). Multi-component queries combine
archetype matches:
```rust
// all entities with Position AND Velocity
let mut q = world.query_many::<(Position, Velocity)>();
for (e, (pos, vel)) in q.iter() { /* ... */ }
```
Queries never cross into Domain B as references — they are materialized into a
`StateView` descriptor set for the guest.

## System interface

### Pure system (Domain B)
```rust
pub fn movement_system(view: &StateView<'_>) -> Result<WorldDelta, RecoverableError> {
    // view.columns / view.arena describe SoA columns; guest reads zero-copy.
    // ... return a WorldDelta of ColumnWrite / deferred commands ...
}
```
Guest systems never mutate host memory. They return a [`WorldDelta`].

### Apply delta (Domain A)
```rust
fn apply_delta(world: &mut World, delta: &WorldDelta) {
    for spawn in &delta.spawns { world.spawn(spawn); }
    for entity in &delta.despawns { world.despawn(*entity); }
    for write in &delta.writes { world.apply_column_write(write); }
    for cmd in &delta.deferred { apply_deferred(world, cmd); }
}
```
`ColumnWrite` payloads are contiguous and applied with `bytemuck::cast_slice`.

## Constraints
- All components are `Pod + Zeroable`; columns byte-aligned.
- Row indices stable within a frame.
- Spawn/despawn are queued and applied after all systems run (`flush`).
- Determinism: identical input tick + identical delta sequence ⇒ identical world.

## Performance targets
- Spawn/Despawn: < 1 µs/entity
- Query iteration: < 10 ns/entity (cache-friendly SoA)
- Column write: < 100 ns

## Testing strategy
- Unit: spawn/despawn/add/remove and swap-remove correctness.
- Integration: spawn 10k, query, apply delta, verify.
- Determinism: run identical tick 3×, assert bit-identical.
- Fuzz: randomized spawn/despawn/add/remove sequences.

## Dependencies
- `contracts` (`Entity`, `ComponentId`, `ColumnDescriptor`, `WorldDelta`,
  `ColumnWrite`), `bytemuck`, `postcard`. Host-only: `rayon`.

## Next steps
1. `ArchetypeStorage`/`Column` in `crates/ecs`.
2. `World` owning archetypes + component registry.
3. spawn/despawn/add/remove + migration.
4. `Query`/`QueryMut`/multi-component.
5. `apply_delta`.
