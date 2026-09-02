---
spec: "33-drag-drop"
phase: "Phase 5: Editor"
status: "draft"
author: "OpenEngine AI"
created: "2026-09-03"
depends_on:
  - "07-editor-inspector"
  - "08-editor-hierarchy"
  - "22-edit-vs-play"
  - "23-undo-redo"
  - "24-editor-viewport"
  - "26-asset-browser-ui"
  - "27-console"
  - "31-multi-selection"
---
# 33 - Drag & Drop

## Overview

Drag & drop is a cross-cutting editor interaction: a user (or agent driving the
UI) grabs a **payload** from one place and releases it over a **target** to
perform an action. Today the pieces are ad-hoc and scattered — the asset browser
(spec `26`) drops assets onto the viewport/hierarchy/inspector with bespoke code,
and the hierarchy (spec `08`) reparents by dragging. This spec replaces those
one-offs with **one generic, registry-driven drag & drop system** so that *every*
drag source, payload type, and target speaks the same language, shows the same
ghost/feedback, and resolves every drop through the same undoable `Command`
channel.

The system is Domain A (`crates/editor`) and is **pure presentation + intent**:
dragging never mutates anything. All state change happens at *drop* time, and a
drop that touches the world always becomes a spec-`23` `Command` (or a composite
of them). The drag layer only (a) describes a payload, (b) asks each hovered
target whether it can accept that payload and which drop *action* would result,
(c) shows ghost + valid/invalid feedback, and (d) on release, asks the accepting
target to translate (payload, action) into an undoable command.

Payloads are typed and cover the entity/asset surface of the editor:
**Entity**, **Component** (type), **Asset**, **Folder**, and **Material**. Drop
actions are a small closed set: **spawn**, **attach**, **set-value**, **move**,
**copy**, plus the OS-file → **import** special case that delegates to spec `26`.
Targets are the editor's main surfaces: **Viewport** (spec `24`), **Hierarchy**
(spec `08`), **Inspector** (spec `07`), **Asset Browser** (spec `26`), and the
**Console** (spec `27`). A payload that no hovered target accepts drops to a
no-op with a `warning`/info toast (spec `35`).

## Core Concepts

### The drag payload (typed)

A drag in flight carries exactly one of these payloads. A payload is *just
data* — stable handles or logical references, never a live pointer into the
world, never an absolute OS path (except the transient OS-file case below).

```rust
/// What is being dragged. Plain data only; no world borrows.
pub enum DragPayload {
    /// One or more selected/grabbed entities (spec 31 multi-selection).
    Entities(Vec<Entity>),
    /// A component *type* chip (e.g. dragging "Transform" onto a blank entity).
    Component(ComponentId),
    /// A registered asset by id (spec 02). `Material` is a specialized asset;
    /// plain non-material assets stay `Asset`.
    Asset(AssetId),
    /// A virtual folder path, relative to the assets root (spec 26).
    Folder(String),
    /// A material asset being applied (spec 02 material kind) — distinct so
    /// targets can offer "assign material to selected" without inspecting type.
    Material(AssetId),
    /// OS file(s) dragged from the host file manager. Transient: converted to
    /// Asset(s) by import (spec 26) and never persisted as absolute paths.
    OsFiles(Vec<OsImportPath>),
}
```

`Entities(Vec<Entity>)` carries the current selection (spec `31`) when the grab
started on a selected item, so dragging one member of a selection moves the whole
group (mirroring multi-select). Payload handles are generation-guarded; a target
re-validates them against a fresh `WorldView` at hover and drop, so a stale
entity payload is rejected rather than applied to a recycled slot.

### Sources and targets register into a drag registry

A **source** claims it can start a drag with a given payload under a grab
condition. A **target** declares which payload variants it accepts and, for each,
what drop `action` results, plus a *handler* that turns `(payload, action)` into
an undoable `Command`. Both are registry entries, so new tools add sources/targets
without editing this spec's enums.

```rust
pub struct DragSource { pub payload: DragPayloadKind, pub grab: GrabRule }
pub enum GrabRule { GrabSelected, Always, RequiresAltForCopy }

pub struct DropTarget {
    pub id: DropTargetId,                       // Viewport | Hierarchy | Inspector | AssetBrowser | Console | Custom(u32)
    /// (payload kind -> accepted action) table this target offers.
    pub accepts: BTreeMap<DragPayloadKind, Vec<DropAction>>,
    /// Translate (payload, action) -> undoable Command(s). May return None when
    /// the live validation fails (stale entity, unsupported asset kind).
    pub handler: Box<dyn Fn(&DropContext, &DragPayload, DropAction) -> Result<Option<Vec<Box<dyn Command>>>, DropError>>,
}
pub enum DropAction { Spawn, Attach, SetValue, Move, Copy, Import }
```

`DropContext` gives a handler only the safe surfaces it needs: a read-only
`WorldView` (spec `07`), the shared `SelectionModel`, the `EditorCommandBus`
(spec `23`) it will push its resulting `Command` onto, and the project
`PathResolver` (spec `30`) for asset/folder resolution. A handler has **no** way
to mutate the world directly — it can only return commands for the bus.

### The drop action table (payload × target)

| Payload → Target | Viewport (24) | Hierarchy (08) | Inspector (07) | Asset Browser (26) | Console (27) |
|---|---|---|---|---|---|
| Entities | Move (drag selection) | Move/reparent (08) | — | — | — |
| Component | Spawn (entity w/ that comp) | Attach (add comp to selected) | Attach (add comp to selected) | — | Spawn |
| Asset (mesh/sprite/etc.) | Spawn (entity using asset) | Attach (asset comp to selected) | SetValue (asset field) | Move/Copy (folder mgmt) | — |
| Folder | — | — | — | Move/Copy | — |
| Material | Attach (assign to selected meshes) | Attach (assign to selected) | SetValue (material field) | — | — |
| OsFiles | Import-then-… | — | — | Import (26) | — |

The table is the *default*; each target registers its own subset. Notable flows:

- **Asset → Viewport = Spawn.** The viewport target translates `Asset(AssetId)`
  into a spec-`26`/`21` spawn: a `SpawnEntityCommand` that creates an entity whose
  mesh/sprite/render components reference the asset. Drop position projects to a
  world position (fixed-point) for the initial transform. Undo despawns; redo
  re-spawns (spec `23`/`26`).
- **Entities → Hierarchy = Reparent (Move).** Dropping an entity row onto a
  hierarchy node issues a `ReparentEntityCommand` (spec `08`/`23`); dropping onto
  empty tree space detaches to root. Multi-selection (spec `31`) issues one
  `ReparentEntityCommand` per member, folded into a composite undo step.
- **Material → selected meshes = Attach.** Assign the material asset to every
  selected entity that can hold a material, as a batch of `ModifyComponentCommand`s
  in one composite (spec `31` group semantics).
- **Component chip → Inspector = Attach.** Add that component to the inspected
  entity via `AddComponentCommand` (spec `23` migration).
- **Asset → Inspector field = SetValue.** If the inspector is editing an
  `AssetRef` field (spec `07` asset picker), dropping sets the field's relative
  path via a `ModifyComponentCommand`.

### Ghost preview + valid/invalid feedback

While a drag is in flight, the drag manager renders a **ghost** — a translucent
snapshot of the payload (the dragged entity's gizmo frame / a thumbnail / the
icon) that follows the cursor. The manager also continuously asks the hovered
surface "do you accept this payload?" and paints feedback on both sides:

- **Valid target:** the surface under the cursor gets a highlight border and the
  ghost border turns green; a small tag shows the resulting action
  ("Attach to Selected", "Spawn", "Reparent").
- **Invalid:** the ghost border is red/grey with an ✕ overlay and no target
  highlights. Releasing on an invalid target is a no-op drop.

Feedback is recomputed each frame by querying the hovered target's `accepts`
table against the payload kind — a pure lookup, no world interaction. The whole
ghost/feedback layer is presentation state owned by the drag manager and is
tested headlessly as geometry/validity logic (which border, which action label),
not pixels.

### Copy vs Move modifiers

A `Copy` action is a distinct drop action offered by targets (default
`GrabSelected` + holding the copy modifier, spec `28` primary modifier). Copy
semantics differ by payload:

- **Entity copy:** duplicate the dragged entity(ies) into the target location
  (a `SpawnEntityCommand` carrying a snapshot of the source's columns), leaving
  the source in place.
- **Asset copy:** duplicate the asset into the target folder (asset-pipeline
  command, spec `02`/`26`), not a world command.
- **Component copy:** same as attach but to a *copy* of the archetype? Not a
  meaningful distinction — treated as Attach. Copy is meaningful for Entities and
  Assets only.

`Move` (the default for entity drags) relocates; `Copy` duplicates. The editor
draws a small "+" on the ghost to indicate a copy is pending when the modifier is
held.

### OS file drag → import

Dragging files from the host file manager is a **host-only, transient** case. The
system's `OsFiles` payload never carries an absolute path into the world. When
dropped onto the Asset Browser (the only OS-file drop target), it becomes a spec
`26` import:

1. The drag manager hands the `OsFiles` to the asset browser's existing import
   path (`ImportJob`, spec `26`), which shows the per-type settings dialog and
   runs the background converter under `OPENENGINE_ASSETS_PATH`.
2. Absolute OS paths exist **only inside the import transaction** and are dropped
   the moment the pipeline registers canonical names (spec `26` guarantees this;
   AGENTS.md § 5).
3. A *successful* import is not a world mutation, so it is **not** a spec-`23`
   command; import/delete/reimport are asset-pipeline undoable operations owned by
   spec `02`/`26`. Optionally, if dropped onto the Asset Browser *and* released
   while over the viewport is impossible (one cursor), a subsequent asset→viewport
   drag can spawn. Import surfaces a progress notification via spec `35`.

### Routing every world-touching drop through spec 23

This is the load-bearing invariant. The drag system never applies a delta itself.
Its lifecycle is:

1. Grab starts → `DragSession { payload, source }` begins; ghost shown.
2. Hover → the manager asks the focused target for acceptance + action (pure).
3. Release over an accepting target → call `target.handler(payload, action)`.
4. The handler returns `Vec<Box<dyn Command>>` (possibly empty).
5. The drag manager pushes them through `EditorCommandBus.push_commands(...)`
   (spec `23`), which folds multiple members into one composite undo step when
   they came from one drop (spec `31`/`23` composite).
6. Only the bus applies the resulting `WorldDelta` at the flush boundary.

No other mutation path exists for a drop. This is what makes drag & drop undoable,
deterministic, and auditable like every other editor write.

## Key Rust Types

```rust
// crates/editor/dragdrop — Domain A
pub enum DragPayload { Entities(Vec<Entity>), Component(ComponentId),
    Asset(AssetId), Folder(String), Material(AssetId), OsFiles(Vec<OsImportPath>) }
pub enum DragPayloadKind { Entities, Component, Asset, Folder, Material, OsFiles }
impl DragPayload { pub fn kind(&self) -> DragPayloadKind; }

pub struct DragSource { pub payload: DragPayloadKind, pub grab: GrabRule }
pub enum GrabRule { GrabSelected, Always, RequiresAltForCopy }
pub enum DropAction { Spawn, Attach, SetValue, Move, Copy, Import }
pub enum DropTargetId { Viewport, Hierarchy, Inspector, AssetBrowser, Console, Custom(u32) }
pub struct DropTarget {
    pub id: DropTargetId,
    pub accepts: BTreeMap<DragPayloadKind, Vec<DropAction>>,
    pub handler: Box<dyn Fn(&DropContext<'_>, &DragPayload, DropAction)
        -> Result<Option<Vec<Box<dyn Command>>>, DropError> + Send>,
}
pub struct DropContext<'a> { pub view: &'a WorldView<'a>,
    pub selection: &'a SelectionModel, pub bus: &'a EditorCommandBus,
    pub resolver: &'a PathResolver<'a> }
pub struct DragRegistry { pub targets: BTreeMap<DropTargetId, DropTarget> } // deterministic
impl DragRegistry {
    /// Pure acceptance + default action for a payload over a target.
    pub fn accept(&self, target: DropTargetId, kind: DragPayloadKind)
        -> Option<DropAction>;
}

pub struct DragManager {
    pub session: Option<DragSession>,   // active drag + ghost geometry
    pub hovered_target: Option<DropTargetId>,
    pub hover_feedback: HoverFeedback,  // Valid(action) | Invalid | None
}
pub struct DragSession { pub payload: DragPayload, pub source_id: u32,
    pub copy_pending: bool, pub ghost_rect: egui::Rect }
pub enum HoverFeedback { Valid { target: DropTargetId, action: DropAction },
    Invalid, None }

pub enum DropError { StaleEntity, UnsupportedAsset, BusBusy, Internal(String) }
```

Sources/targets register once into the `DragRegistry`; the registry is consulted
for hover acceptance (pure) and dispatch. The `DragManager` owns the transient
session + ghost; it holds no world state.

## Components

None — editor/UI tooling only; no new ECS component. Drag payloads and the whole
drag/ghost/registry stack are Domain-A structs, never registered components and
never stored in the world or the guest. Every drop *result* is expressed through
existing spec-`23` commands operating on existing components (transform,
mesh/render/material/`AssetRef`, `Parent`), so the system adds no entity or
component type of its own.

## Constraints

- **Domain A only.** The drag registry, manager, ghost and handlers live in
  `crates/editor`; never compiled into Domain B.
- **Drag never mutates.** All state change happens at drop; a drag in flight is
  presentation + intent only (ghost/feedback/hover).
- **Every world-touching drop is an undoable spec-`23` command** (or composite)
  pushed through `EditorCommandBus`; the drag layer never calls `apply_delta`.
  UI never reaches around the bus (spec `23` invariant).
- **Plain-data payloads.** Payloads hold stable generation-guarded handles or
  logical references — never world borrows, never pointers. Targets re-validate
  against a fresh `WorldView`; stale handles → `DropError::StaleEntity`, never a
  write over a recycled slot.
- **No absolute paths persist.** `OsFiles` absolute paths exist only inside the
  spec-`26` import transaction; everything stored is relative to
  `OPENENGINE_ASSETS_PATH`/the project (AGENTS.md § 5, spec `30`).
- **Fixed-point for authored positions.** Drop-to-spawn world positions are
  quantized to `openengine-math` fixed-point before they enter command payloads;
  `f32` is confined to the ghost/cursor presentation layer.
- **Deterministic registry.** Targets and their accept tables are `BTreeMap`
  keyed by stable id (no `HashMap` iteration), so hover acceptance and
  dispatch are reproducible.
- **Edit-world only.** Drops operate on the edit world; during play the
  mutating targets are gated off (`mode == Edit`, spec `22`) and a drag over the
  live play viewport is read-only.
- **Virtualized/accepted large payloads.** An `Entities(Vec<…>)` payload from a
  10k+ selection (spec `31`) still batches into one composite command; the ghost
  shows a count badge, not 10k thumbnails.
- **Compiles on `x86_64-linux` and `aarch64-linux`.** Registry logic, acceptance,
  action resolution, ghost-validity math and handler translation are pure and
  headless-testable with no GPU and no window.

## Performance Targets

- **Hover acceptance:** a pure `BTreeMap` lookup + action resolution over the
  hovered target — **< 10 µs**, recomputed only when the cursor crosses a target
  boundary or payload kind changes, not every pixel.
- **Ghost update:** moving the ghost rect is O(1) presentation state; no world
  scan per mouse move.
- **Drop dispatch:** handler translation of a single-payload drop builds ≤ a
  handful of commands — **< 1 ms**; a 10k multi-entity drop batches into one
  composite command without per-entity UI stalls (compute O(n) off the paint
  path).
- **Copy entity/asset:** bounded by the snapshot/decode of the source; matches
  spec `26`/`21` spawn cost.
- **No-drop cost:** with no drag in flight, `DragManager` adds ~0 per frame.

## Testing Strategy

All headless (no GPU / no window) in `crates/editor`:

- **Registry/acceptance:** register the five target implementations; assert each
  `accept(target, kind)` yields the expected default `DropAction` per the payload×
  target table and `None` for unsupported combinations.
- **Entity drop → Reparent (integration):** drop an entity payload onto a
  hierarchy target; assert the handler returns a `ReparentEntityCommand`, the bus
  applies it, and Undo restores the pre-drop parent in one step (spec `23`).
- **Asset drop → Spawn:** drop an `Asset` onto the Viewport target; assert a
  `SpawnEntityCommand` referencing the asset is produced; Undo despawns, Redo
  re-spawns; the world returns bit-identical after undo/redo.
- **Material → selected:** drop a `Material` over a multi-selection (spec `31`);
  assert a single composite command assigns it to every eligible selected entity.
- **Component chip → Inspector/Attach:** assert an `AddComponentCommand` +
  archetype migration occurs and Undo removes it.
- **Stale payload:** carry an entity payload, despawn it, then drop; assert
  `DropError::StaleEntity` and no write to a recycled slot.
- **Copy vs Move:** with/without the copy modifier, assert entity drop yields
  duplicate (Copy) vs relocate (Move) commands and source state is correct.
- **OS import (headless):** feed an `OsFiles` payload to the Asset Browser
  target; assert it routes to a spec-`26` `ImportJob` and that no absolute path is
  stored afterward (relative-path invariant).
- **Feedback logic:** for a set of (payload, target) pairs assert the manager
  reports `Valid(action)`/`Invalid`/`None` exactly; run 3× for identical output.
- **No direct mutation:** assert the drag manager/handlers expose no world-write
  path and that every mutation observed went through a mock `EditorCommandBus`.

## Dependencies

- `crates/editor` (Domain A) — `DragRegistry`, `DragManager`, ghost/feedback,
  handler translation; shared `WorldView`/`SelectionModel`/`EditorCommandBus`.
- Undoable `Command`/`EditorCommandBus` + composite batching from spec `23`;
  edit-world gating from spec `22`; multi-select group semantics from spec `31`.
- Drop targets: viewport (spec `24`), hierarchy reparent (spec `08`), inspector
  `AssetRef`/add-component (spec `07`), asset browser import/asset ops (spec `26`),
  console spawn (spec `27`).
- Asset ids/material kinds from spec `02`; spawn component initialization from
  spec `21`; project `PathResolver` + config scope from spec `30`; drop/import
  result toasts from spec `35`.
- `egui` (Domain A), `openengine-math` (fixed-point drop positions),
  `contracts` (`Entity`, `ComponentId`, `AssetId`-adjacent logical refs are
  Domain-A asset ids). No new `contracts`/ABI surface.

## Next Steps

1. Define `DragPayload`/`DropAction`/`DropTarget`/`DragRegistry` and the
   acceptance lookup.
2. Implement `DragManager` (session, ghost geometry, hover feedback) and the
   copy-modifier handling.
3. Implement handler translation for the Entity→Hierarchy (reparent) and
   Asset→Viewport (spawn) flows through `EditorCommandBus`.
4. Implement Component→Attach, Material→selected, and Asset→Inspector `SetValue`
   handlers.
5. Wire the Asset Browser OS-file → spec-`26` `ImportJob` path and the
   relative-path invariant.
6. Land the headless registry/feedback/handler/undoability test battery and
   register the five production targets into the editor shell (spec `25`).
