---
spec: "08-editor-hierarchy"
phase: "Phase 5"
status: "design"
---

# Editor Hierarchy

## Overview

The hierarchy panel is the `egui` scene tree: it shows every entity in the
current scene, optionally as a parent/child tree, grouped by archetype, and lets
an agent or human select, spawn, despawn, reparent (drag), filter, and batch or
rename entities. Like the inspector (spec `07`) it is a **Domain-A ECS system**
and **host-side only**: it reads the world through a safe view and issues every
structural change (spawn/despawn/reparent) as a `WorldDelta` at the flush
boundary. It never writes into ECS storage mid-iteration.

The hierarchy and the inspector (spec `07`) share one `SelectionModel`: choosing
an entity in the tree selects it in the inspector, and vice-versa. This spec
focuses on the tree — structure, parent/child, grouping, and the structural
commands the tree issues — while spec `07` handles the per-component editing of
whatever the tree has selected.

## Design

### The hierarchy as an ECS system

```rust
// crates/editor — Domain A
pub struct HierarchySystem {
    pub selection: SelectionModel,   // shared with spec 07 inspector
}

impl HierarchySystem {
    /// Render the tree for the current scene each (post-update) frame.
    pub fn run(
        &mut self,
        ui: &mut egui::Ui,
        view: &WorldView<'_>,            // safe immutable SoA read
        commands: &mut HierarchyCommands,// staged structural ops, flushed later
    );
}
```

The tree is a read-only projection of the current scene's `World`. The system
collects entities into a display list once per frame (see **Grouping**), renders
`egui::CollapsingHeader`/tree rows, and records user intent into
`HierarchyCommands` — it does not touch ECS storage directly.

### Parent / child relation

OpenEngine's ECS is archetype- and component-based (spec `00`); there is no
built-in parent/child slot. Hierarchy is therefore represented as **a
component**: a `Parent { parent: Entity }`/`Children` relation carried as
regular `Pod` components on entities, resolved into a tree by the hierarchy
system for display.

```rust
// crates/ecs or a components crate — Domain A/B component definition.
#[repr(C)]
pub struct Parent { pub parent: Entity }      // Entity::INVALID == root
```

Because `Parent` is a normal component, reparenting is an **archetype
migration** (the entity leaves the "has no Parent" archetype for the "has a
Parent" one, or vice versa) — the same migration machinery spec `00` and the
inspector (spec `07`) use. That keeps determinism and reuses one code path.
Entities whose `Parent` is `Entity::INVALID` are tree roots.

### Selection model (shared)

```rust
pub struct SelectionModel {
    current: Option<Entity>,       // generation-guarded; follows migration
    anchor: Option<Entity>,        // for shift/multi-select range
}

impl SelectionModel {
    pub fn select(&mut self, e: Entity);
    pub fn current(&self) -> Option<Entity>;
    /// Re-resolve after any structural change so a moved entity stays selected.
    pub fn follow_migration(&mut self, from: ArchetypeId, to: ArchetypeId, e: Entity);
}
```

Selecting a row in the tree sets `selection.current`; the inspector (spec `07`)
reads the same model. When a structural change migrates the selected entity to a
new archetype, `follow_migration` keeps it selected by its stable `Entity`
handle (generation-guarded, so a despawned-then-recycled slot never falsely
stays selected).

### Grouping

Two mutually-exclusive display modes, selectable in the panel header:

* **Tree by parent/child.** Real entity structure; nested rows under parents.
  Collapsed branches hide descendants. This is the default when a scene uses
  `Parent` relations.
* **Grouped by archetype.** Entities bucketed under their `ArchetypeId`
  (e.g. "Archetype 3 — Position,Velocity,Sprite"). Good for reasoning about
  SoA storage; shows the real archetype composition. Within an archetype,
  entities are ordered by row for stable display.

When "grouped by archetype" is active, parent/child edges are shown as an
overlay or suppressed; the two modes are not merged to avoid a confusing dual
axis. Filtering applies before grouping.

### Structural commands (staged)

The tree never mutates ECS directly. User actions emit `HierarchyCommands`,
which are flushed to `WorldDelta` at the frame's flush boundary — the same
boundary used by spec `01`, `06`, and `07`. Commands are structural:

```rust
pub enum HierarchyCommand {
    Spawn { archetype: ArchetypeId, parent: Option<Entity> },
    Despawn { entity: Entity },
    Reparent { entity: Entity, new_parent: Option<Entity> },
    Rename { entity: Entity, new_name: String },   // sets a Name component
}
```

`Despawn` is deferred like guest despawns (spec `00` swap-remove + flush).
`Reparent` becomes an add/remove-`Parent` migration (above). Spawning into a
scene reuses the scene's canonical spawn path so editor-spawned entities are
indistinguishable from logic-spawned ones.

### Spawn / despawn from the tree

A "+" affordance on the panel (and per-archetype in grouped mode) spawns a new
entity into a chosen archetype with default (`Zeroable`) component values,
optionally under a selected parent; a "Delete" action (or `Del` key) despawns
the selected entity. Both go through `HierarchyCommand`. A spawned entity
becomes the new selection.

### Filtering

A text box filters the tree by entity name (a `Name` component) and/or by
archetype/component id. Matched entities and any ancestors that lead to a match
are kept visible; non-matching siblings are hidden. Filtering is a pure
client-side projection over the `WorldView` snapshot — it issues no commands and
does not change the world.

### Batch vs rename

* **Rename** targets one entity: an editable name field edits its `Name`
  component, committed as a single `ColumnWrite` at flush.
* **Batch** targets the current selection (multi-select via `anchor`/shift).
  Batch actions apply a single structural or `Name`-edit operation to every
  selected entity (e.g. "rename prefix", "reparent all under X"). Batch is
  issued as a set of `HierarchyCommand`s applied in stable (sorted) order so
  the result is deterministic. Batch never reuses a single row index across
  distinct entities without re-resolving handles first.

## Key Rust / types

- `HierarchySystem`, `HierarchyCommand`, `HierarchyCommands` — `crates/editor`.
- `SelectionModel` — shared with spec `07`.
- `Parent` component (`Entity::INVALID` == root) — where the relation lives.
- `WorldView<'a>` — safe immutable SoA read (same as spec `07`).
- `contracts::{Entity, ArchetypeId, ComponentId, ColumnWrite, WorldDelta}`.

## Constraints

- **Host-side only.** The hierarchy tree and all its commands live in Domain A
  (`crates/editor`) and are never compiled into the guest. There is no "entity
  tree" in Domain B.
- The tree reads through a safe `WorldView` snapshot and issues
  `HierarchyCommand`s; it never mutates ECS storage mid-iteration.
- All structural changes (spawn/despawn/reparent/rename/batch) flush through
  `WorldDelta` at the flush boundary and reuse the migration path.
- Parent/child is a component (`Parent`), not a special ECS slot; reparent is a
  migration.
- Deterministic ordering: batch and reparent resolve handles before issuing and
  apply in stable order; no `HashMap` iteration in tree building.
- Rename/selection never keep stale handles past a despawn (generation guard).
- Compiles on `x86_64-linux` and `aarch64-linux`; tree logic unit-tests need no
  GPU/window (headless tests of `SelectionModel` + command staging).

## Performance targets

- Tree rebuild over the current scene's entities: target linear in entity count,
  < 1 ms for 10 000 entities (snapshot once per frame, no per-frame world scan
  in egui paint).
- Selection + structural-command staging: sub-millisecond, constant per command.
- Reparent migration cost equals a normal archetype migration (spec `00`).

## Testing strategy

- **Unit (headless):** `SelectionModel` select/migrate-follow/despawn-guard;
  command staging produces the right `HierarchyCommand`s.
- **Migration:** reparent (add/remove `Parent`) moves the entity to the correct
  archetype and keeps it selected via `follow_migration`.
- **Batch determinism:** run a batch rename/reparent over a set of entities
  three times; assert identical final structure and ordering.
- **No-mutation invariant:** assert the hierarchy issues commands and flushes at
  the boundary, never mutating mid-iteration (same busy guard as spec `07`).
- **Integration:** spawn→reparent→rename→despawn a chain in a scene, run 1 000
  ticks, and assert reproducible world state across 3 runs.

## Dependencies

- `crates/editor` (egui), `crates/ecs` (`World`, migration), `contracts`
  (`Entity`, `ArchetypeId`, `WorldDelta`, `ColumnWrite`), `openengine-math`.
- Shares `SelectionModel` and `WorldView` with spec `07`; follows scene
  lifecycle from spec `06`.

## Next steps

1. Implement `SelectionModel` (shared) with migration-follow + generation guard.
2. Implement `WorldView` snapshot + tree building (archetype grouping, filter).
3. Implement `HierarchyCommand` staging + flush to `WorldDelta`.
4. Implement the `Parent` component and reparent-as-migration.
5. Implement spawn/despawn, rename, batch actions, and the shared selection with
   spec `07`'s inspector.
