---
spec: "31-multi-selection"
phase: "Phase 5: Editor"
status: "draft"
author: "OpenEngine AI"
created: "2026-09-03"
depends_on:
  - "07-editor-inspector"
  - "08-editor-hierarchy"
  - "21-primitive-components"
  - "22-edit-vs-play"
  - "23-undo-redo"
  - "24-editor-viewport"
  - "25-editor-shell"
---
# 31 - Multi-Selection

## Overview

Spec `24` introduced single-click and rectangle ("box") select on the viewport,
and specs `07`/`08` share a `SelectionModel` that tracks a single `current`
entity (plus an `anchor` for range selection). This spec generalizes selection
into a first-class, multi-select-capable editor concept so a human or agent can
operate on *sets* of entities — lights, walls, an entire prop scatter — as one
unit.

`MultiSelection` extends — never replaces — the shared selection state. It adds:
a multi-entity selection *set*; **named selection sets** (e.g. `all_lights`) that
persist across sessions; richer geometric selectors (**lasso / freeform polygon**
in addition to box); **filtering** of the selection by component type and tag;
**group operations** (move / rotate / scale / align / distribute) applied to the
whole set as undoable spec-`23` commands; **selection history** (back / forward)
so a selection change is as navigable as a viewport move; and a **virtualized
selection list** so a 10k+ entity set is inspectable and re-orderable without
killing frame rate.

Everything here is Domain A, editor/UI tooling, and — critically — **selection
is never a gameplay component**. A selected/hovered/grouped entity is pure editor
state; it never leaks into the world or the guest (spec `24` already states this
for hover/selection). Group operations change the selected entities' *existing*
transform components through the normal undoable command channel; they introduce
no new ECS entity component. Named selection sets are editor-side metadata
persisted as JSON under the project, not ECS state.

## Core Concepts

### A real multi-select model (superset of the spec 07/08 anchor model)

The editor keeps **one** authoritative selection state shared by the viewport
(spec `24`), hierarchy (spec `08`), inspector (spec `07`) and every tool. Spec
`08`'s model held `current` + `anchor`; spec `24` added box-select. This spec
upgrades that model to a set while keeping the single `current` (the "primary"
entity the inspector edits) and the `anchor` (for shift-range) semantics intact
so existing panels keep working unchanged:

```rust
/// The editor's single authoritative multi-select state. Supersedes the
/// current/anchor pair of spec 07/08 while remaining backward-compatible with
/// them: current == the primary entity; the rest is the broader set.
pub struct SelectionModel {
    /// Ordered, deduplicated set of selected entities (primary first). Kept as a
    /// sorted Vec by (generation,index) for determinism; an insertion order
    /// companion Vec drives display order. See Constraints.
    pub set: Vec<Entity>,
    /// The primary entity (== set[0] normally). Inspector edits this one.
    pub current: Option<Entity>,
    /// Range anchor for Shift+click selection (spec 08).
    pub anchor: Option<Entity>,
}
```

Operations are the only way to mutate it: `select`, `toggle`, `add`,
`replace_set`, `clear`, `remove(Entity)`, `set_primary`. Every operation is
generation-guarded: an entity that was despawned is dropped from the set the next
time the selection is re-resolved against a fresh `WorldView` (via the same
`follow_migration` discipline as spec `08`). Nothing in `SelectionModel` stores a
component or writes to the world.

### Geometric selectors (beyond box)

The viewport already supports box select (spec `24`). `MultiSelection` adds a
lasso and reuses the same AABB-based picking math, always against a pure,
headless-testable ray/AABB projection:

```rust
/// How a drag-rectangle or freeform gesture turns into a set of entities.
pub enum SelectionMode {
    Replace,          // the gesture's result replaces the current set
    Add,              // result is unioned into the current set (Ctrl)
    Subtract,         // result is removed from the current set (Alt)
    Toggle,           // membership toggled per hit (Shift)
}
pub enum SelectorShape {
    Box(egui::Rect),                 // spec 24 rectangle
    Lasso(Vec<glam::Vec2>),          // freeform polygon in screen space
}
```

Selection is a pure function `select_in_viewport(shape, candidates,
selection, mode) -> SelectionModel`: for each candidate entity, project its AABB
(spec `21`) through the viewport camera (spec `24`) into screen space and test
`shape`; **Box** tests rectangle intersection, **Lasso** tests point-in-polygon
(winding number) on a screen-AABB corner sample. The result is applied through
`SelectionModel` per `mode`. Because it is pure over `(candidates, camera,
shape, mode)`, the whole geometry side is unit-testable with no GPU.

A spatial index over entity bounds (the BVH spec `24` mentions) keeps candidate
enumeration fast at 10k+; lasso adds a per-candidate screen-projection cost that
is bounded because only bounds, never mesh triangles, are tested.

### Named selection sets

A named selection set is a reusable, human/agent-readable subset of a project's
entities. The flagship example is a **tag-style bucket** — `all_lights`,
`enemies`, `props` — captured once and re-applied later.

```rust
/// A persisted, named selection set. Editor-side metadata, NOT an ECS component.
#[derive(Clone, Debug, serde::Serialize, serde::Deserialize)]
pub struct NamedSelection {
    pub name: String,             // e.g. "all_lights"
    pub members: Vec<Entity>,     // generation-guarded handles; resolved on apply
    pub filter: Option<SelectionFilter>, // optional stored filter (see below)
    pub updated: String,          // ISO display timestamp, not for determinism
}
```

- **Capture** the current selection into a name (`CaptureSelection("all_lights")`
  is a menu/shortcut action). **Apply** a named set selects exactly its live
  members (stale ones dropped). **Delete/rename** manage the list.
- **Persistence.** Named sets are authored, project-relative metadata. They are
  stored as `serde_json` under the open project's `config/` directory
  (spec `30`): `config/selection_sets.json`, resolved relative to the project,
  never an absolute/home path (AGENTS.md § 5). The file is written on
  capture/rename/delete (debounced) and loaded when a project opens.
- **Determinism/portability of a set.** A named set stores only stable
  generation-guarded `Entity` handles (which are themselves deterministic across
  a spec-`16` load of the same scene, per spec `06`). Applying a set to the
  *same* scene twice yields the same selection. Handles are *relative to a
  scene*, so a named set records which scene it belongs to and is applied only
  against that scene id (a mismatched scene is a no-op with a spec-`35` info
  notification, never an error).

### Filtering the selection by component type and tag

Two orthogonal ways to reach "select all entities that are X":

1. **By component type** — the set of entities carrying a given `ComponentId` (or
   several) within the current scene.
2. **By tag** — entities carrying a user tag value (a `Tag` component carrying a
   fixed-width string, registered per spec `21`/`00`).

```rust
pub enum SelectionFilter {
    Component(Vec<ComponentId>),   // all of these components present
    AnyComponent(Vec<ComponentId>),// at least one present
    Tag(String),                    // has this tag value
    And(Box<SelectionFilter>, Box<SelectionFilter>),
    Or(Box<SelectionFilter>, Box<SelectionFilter>),
    Not(Box<SelectionFilter>),
}
```

Filtering is a pure read over a `WorldView` snapshot (spec `07`): it scans
archetypes, tests each entity's component set/tags, and produces a candidate set
that a `SelectionMode` then folds into the model. It issues no commands. Filters
can be applied to the current scene (`Select All with Filter`) or stored inside a
`NamedSelection` so `all_lights` stays "everything currently tagged light" rather
than a stale snapshot.

### Group operations as undoable commands

The reason to select *sets* is to transform them together. Group ops operate on
each selected entity's **existing** transform components (position / rotation /
scale, spec `21` — fixed-point in the world) and produce one undoable step per
operation, exactly like a gizmo gesture (spec `23` transaction):

```rust
pub enum GroupOp {
    Move(glam-like delta, quantized to I16F16),
    Rotate(axis-aligned degrees as I16F16),
    Scale(uniform factor as I16F16),
    Align { axis: AlignAxis, mode: AlignMode },  // e.g. align tops along +Y
    Distribute { axis: AlignAxis, bounds: Option<(I16F16, I16F16)> },
}
pub enum AlignAxis { X, Y, Z }
pub enum AlignMode { Min, Center, Max }  // which extremity lines up
```

Execution is a **composite command** (spec `23` `CompositeCommand`):
1. Resolve the current `SelectionModel` set to a stable, sorted `Vec<Entity>`
   (sorted for determinism, no `HashMap` order).
2. Read each entity's current transform from a `WorldView` snapshot *once*.
3. Compute the target transform per entity using fixed-point math
   (`openengine-math`), producing a `ModifyComponentCommand` per entity carrying
   the exact old/new raw payload bytes.
4. Fold them into one `CompositeCommand` so **one** undo/redo step restores the
   whole group (spec `23` collapse when all members mutate the same value type).

```rust
/// One whole-group operation = one undoable spec-23 step.
pub struct GroupOperationCommand {
    pub op: GroupOp,
    pub members: Vec<ModifyComponentCommand>,  // old/new per entity, fixed-point
    pub description: String,                    // "Align 4 walls to +X max"
}
```

- **Align** — line up the selected entities' transforms along an axis by their
  `Min`/`Center`/`Max` extremity (computed from transform position + bounds when
  bounds are relevant, else position alone). The reference value is the global
  extremity across the set, so all members converge.
- **Distribute** — spread the set evenly along an axis within the set's own
  bounding range (or an explicit `bounds`). Only position changes; spacing uses
  fixed-point division so it is reproducible.
- All math is `openengine-math` fixed-point; `glam` `f32` appears only in the
  gizmo/preview layer of the viewport (spec `24`), never in the committed
  command payloads.

### Selection history (back / forward)

Changing which entities are selected is itself a navigation gesture, so it gets
a bounded history like the undo system but is *separate* — selection history is
not world undo:

```rust
pub struct SelectionHistory {
    past: Vec<SelectionModel>,   // older selections, most-recent last
    future: Vec<SelectionModel>, // cleared on a fresh selection action
    max_len: usize,              // default 32
}
```

`Back` (action `selection.back`, default `Alt+Left`) pops `past` into the current
model and pushes the old current onto `future`; `Forward` (`Alt+Right`) reverses
it. A *discrete* selection gesture (replace, box/lasso commit, apply a named set,
jump via history-panel) pushes a snapshot onto `past` and clears `future`.
**Per-frame transient changes** (hover, incremental Ctrl+add that is part of one
gesture) are folded, not snapshotted — mirroring spec `23`'s transaction batching
so 60 frames of a marquee never fills the history. History stores `SelectionModel`
value snapshots (entities are `Pod` handles), so it is trivially serializable and
headless-testable; it never touches the world.

### Virtualized selection list (10k+)

The hierarchy (spec `08`) and the new **Selection panel** must render a selected
set that can reach 10 000+ entities. The Selection panel shows the current set as
a virtualized list (same technique spec `26` uses for 10k+ assets and spec `08`
uses for the tree): a flat sorted `Vec<Entity>` plus a precomputed cumulative
row-height table; egui paints only the rows intersecting the visible `y` range.
Each row renders the entity's name (a read from the `WorldView`), its primary
components, and controls to remove it from the set, reorder it (drag within the
set → changes the primary / paint order), or make it `current`. Building the
display list is O(n) once per selection change; per-frame paint is O(visible
rows). Reordering the panel's display list does **not** change the world — only
editor-side ordering of `SelectionModel::set` for group-op stability and primary
selection.

## Key Rust Types

```rust
// crates/editor/selection — Domain A
pub struct SelectionModel { pub set: Vec<Entity>, pub current: Option<Entity>,
    pub anchor: Option<Entity> }
impl SelectionModel {
    pub fn select(&mut self, e: Entity); pub fn add(&mut self, e: Entity);
    pub fn toggle(&mut self, e: Entity); pub fn remove(&mut self, e: Entity);
    pub fn replace_set(&mut self, set: Vec<Entity>); pub fn clear(&mut self);
    pub fn set_primary(&mut self, e: Entity);
    pub fn follow_migration(&mut self, from: ArchetypeId, to: ArchetypeId, e: Entity);
    pub fn prune_stale(&mut self, view: &WorldView<'_>); // drop despawned handles
}
pub enum SelectionMode { Replace, Add, Subtract, Toggle }
pub enum SelectorShape { Box(egui::Rect), Lasso(Vec<glam::Vec2>) }
pub fn select_in_viewport(shape: SelectorShape, candidates: &[Entity],
    projection: &ViewportProjection, selection: &SelectionModel, mode: SelectionMode)
    -> SelectionModel;

pub struct NamedSelection { pub name: String, pub members: Vec<Entity>,
    pub filter: Option<SelectionFilter>, pub updated: String }
pub struct NamedSelectionRegistry { pub sets: Vec<NamedSelection>,
    pub scene_id: SceneId, pub dirty: bool } // stored as project config/selection_sets.json

pub enum SelectionFilter {
    Component(Vec<ComponentId>), AnyComponent(Vec<ComponentId>),
    Tag(String), And(Box<Self>, Box<Self>), Or(Box<Self>, Box<Self>), Not(Box<Self>),
}
pub fn filter_entities(view: &WorldView<'_>, f: &SelectionFilter) -> Vec<Entity>;

pub enum GroupOp { Move /*I16F16 delta*/, Rotate /*I16F16 deg*/, Scale /*I16F16*/,
    Align { axis: AlignAxis, mode: AlignMode }, Distribute { axis: AlignAxis, bounds: Option<(I16F16, I16F16)> } }
pub struct GroupOperationCommand { pub op: GroupOp,
    pub members: Vec<ModifyComponentCommand>, pub description: String } // impl Command (spec 23)

pub struct SelectionHistory { past: Vec<SelectionModel>, future: Vec<SelectionModel>,
    max_len: usize }  // back()/forward()/push_gesture()/push_discrete()

pub struct SelectionPanel { pub selection: SelectionModel,
    pub history: SelectionHistory, pub registry: NamedSelectionRegistry,
    pub display: Vec<Entity>, pub cumulative_rows: Vec<f32> } // virtualized UI state
```

## Components

None — editor/UI tooling only; no new ECS component. Selection set/history/
named-set/filter state is entirely editor-side (`SelectionModel` and friends are
Domain-A structs, never registered components). Group operations only rewrite the
selected entities' **existing** `Transform`/position components via spec-`23`
commands; they add no component. Named selection sets persist as project JSON,
not ECS state.

## Constraints

- **Domain A only.** All multi-select types live in `crates/editor`; never
  compiled into the guest, never stored in the world, never readable by Domain B.
- **Selection is not a component.** `SelectionModel`, hover, sets, history, and
  filters are editor UI state. Selection never leaks into the edit world's
  archetypes and never reaches the play world (spec `22`).
- **No direct ECS mutation.** Group ops and any structural consequence route
  through spec `23` commands (one `CompositeCommand` per group op, collapsed
  where possible). UI never calls `apply_delta` itself.
- **Fixed-point only in committed values.** Group-op payloads are
  `openengine-math` fixed-point raw bytes captured as old/new (spec `23`); `f32`
  exists only in the viewport gizmo/preview presentation layer (spec `24`).
- **Deterministic ordering.** The selection `set` and every group-op member list
  are kept sorted by `(generation, index)` for stable iteration; no `HashMap`
  ordering anywhere a result is serialized or applied. Display order is a separate
  editor Vec, never relied on for determinism.
- **Generation guards.** Every stored handle (set, named set member, history
  snapshot) is generation-guarded; stale handles are pruned on re-resolve, never
  written over a recycled slot (spec `23` `StaleGeneration` semantics).
- **Portable persistence.** Named sets live under the open project's `config/`
  (`config/selection_sets.json`), relative to the project root (spec `30`) — no
  absolute/home paths. Scene-relative handles are applied only against their
  recorded scene.
- **Virtualized big lists.** The selection panel renders O(visible rows) per
  frame over a precomputed flat list; 10k+ selected entities must not stall the
  frame.
- **Compiles on `x86_64-linux` and `aarch64-linux`.** Selection model, filter,
  box/lasso projection and group-op math are pure and headless-testable with no
  GPU and no window.

## Performance Targets

- Box/lasso select over 10 000 candidate AABBs (with a bounds BVH per spec `24`):
  **< 5 ms**; without the BVH, replace-only still targets < 10 ms.
- Filter by component/tag over a 10 000-entity scene: **< 5 ms** on a single
  debounced query; results fold into the model in O(result).
- Group op over a 10 000-entity set: bounded by reading N transforms + emitting N
  `ModifyComponentCommand`s — target **< 10 ms** for the read/compute, then one
  collapsed apply at flush; the single undo step is < 1 ms.
- Selection history push/pop/back/forward: O(set size) for the snapshot, bounded
  to `max_len`=32; a per-frame marquee pushes **one** snapshot, not one per frame.
- Selection panel paint: O(visible rows) only; the flat display list is built
  O(n) once per selection change.
- Named-set capture/apply over a 10k scene: **< 5 ms** (apply just filters the
  current scene membership), independent of set size; JSON persistence debounced.

## Testing Strategy

All headless (no GPU / no window) in `crates/editor`:

- **Model ops:** add/toggle/remove/replace/clear/set_primary maintain the sorted,
  deduplicated `set` and correct `current`; `prune_stale` drops despawned handles
  and never selects over a recycled slot.
- **Geometry (box & lasso):** from a synthetic set of AABBs + a camera, assert
  box and lasso (point-in-polygon) hit the exact expected entities in each
  `SelectionMode` (Replace/Add/Subtract/Toggle). Run 3×, identical results.
- **Filter:** component / tag / And / Or / Not filters yield the expected sets on
  a seeded scene; stored-filter named sets track scene membership over edits.
- **Group op determinism:** apply `Align`/`Distribute`/`Move`/`Rotate`/`Scale` to
  a fixed set and assert the resulting transforms are bit-identical across 3 runs
  and that one `GroupOperationCommand` undo restores the pre-op transforms in one
  delta (spec `23`).
- **Selection history:** a scripted sequence of discrete selections + one marquee
  produces the expected `past`/`future`; back/forward round-trips exactly; a fresh
  gesture clears `future`; the marquee folded to a single snapshot.
- **Virtualization:** with a synthetic 100 000-entity selection, assert only the
  visible rows are painted and paint cost is bounded; reordering changes display
  order but never world state.
- **Persistence:** capture `all_lights`, serialize `config/selection_sets.json`,
  reload, assert identical; applying a set against a mismatched scene id is a
  no-op (info), never an error.
- **No-GPU invariant:** every test exercises the pure model/filter/geometry/group
  math; the egui panel is behind the window feature with a stub.

## Dependencies

- `crates/editor` (Domain A) — `SelectionModel`, selection panel, filter,
  history, group ops; shared `WorldView`/`SelectionModel` with specs `07`/`08`;
  mounts into the shell (spec `25`).
- Bounds/AABB + `Transform` components from spec `21`; picking/camera projection
  and the bounds BVH from spec `24`; parent/hierarchy display from spec `08`.
- Undo/redo `Command`, `CompositeCommand`, `ModifyComponentCommand` and
  `StaleGeneration` semantics from spec `23`; edit-world-only rule from spec `22`.
- Scene id + world codec from spec `06`/`16`; project `config/` path + scope from
  spec `30`; notification of capture/apply results from spec `35`.
- `openengine-math` (fixed-point group math), `glam` (presentation projection in
  the viewport only), `serde`/`serde_json`, `contracts` (`Entity`, `ComponentId`,
  `ArchetypeId`). No new `contracts`/ABI surface.

## Next Steps

1. Upgrade `SelectionModel` to a sorted multi-`set` with `current`/`anchor`
   preserved; add add/toggle/remove/prune operations.
2. Implement pure box/lasso `select_in_viewport` over the spec-`24` projection +
   bounds BVH, and the `SelectionMode` fold.
3. Implement `SelectionFilter` and `filter_entities` over a `WorldView`; add the
   tag component dependency from spec `21`.
4. Implement group ops (align/distribute/move/rotate/scale) as
   spec-`23` `CompositeCommand`s with fixed-point math.
5. Implement `SelectionHistory` (back/forward, gesture folding) and the
   virtualized Selection panel; wire `Back`/`Forward` shortcuts via spec `28`.
6. Implement `NamedSelectionRegistry` persisted under the project's
   `config/selection_sets.json` (spec `30`) and the capture/apply UI.
