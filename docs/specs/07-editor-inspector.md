---
spec: "07-editor-inspector"
phase: "Phase 5"
status: "design"
---

# Editor Inspector

## Overview

The inspector is an `egui` panel that shows every component on the currently
selected entity and lets an agent or human edit those components. It is **not a
special tool** — it is a Domain-A **ECS system** that runs inside the game loop,
reading the world exactly like any other system and writing changes back through
the same deterministic mutation channel (`WorldDelta` / `ColumnWrite`). This is
AGENTS.md pillar 4: the editor is a host-resident system, privileged (may touch
the file system, pop windows) but architecturally first-class.

The hard rule this spec exists to uphold: **the inspector must never mutate ECS
storage while any system is iterating it, and it must never reach into Domain B
memory.** It observes through a safe SoA view and emits edits as deferred,
boundary-aligned deltas.

The inspector shares its selection state with the hierarchy panel (spec `08`).
This spec covers the panel: what it reads, how edits are committed, typed vs
raw field editing, asset picking, add/remove component (migration), and revert.

## Design

### The inspector as an ECS system

```rust
// crates/editor — Domain A
pub struct InspectorSystem {
    pub selection: SelectionModel,   // shared with spec 08 hierarchy
}

impl InspectorSystem {
    /// Runs once per (post-update) frame with a *snapshot* of the selection.
    pub fn run(
        &mut self,
        ui: &mut egui::Ui,
        view: &WorldView<'_>,        // safe, immutable SoA read of the current scene
        draft: &mut InspectorDraft,  // staged edits, applied at flush boundary
    );
}
```

The inspector reads the selected entity's archetype, its component list, and
each component's bytes **through a read-only view** (`WorldView` — the host-side
equivalent of `StateView`, zero-copy `bytemuck` column reads). It never holds
`&mut` into columns.

### Two-phase edit model: stage then commit

Directly mutating ECS columns under `egui` is forbidden (borrow + iteration
hazards, and it would bypass determinism). Instead edits are **staged** into an
`InspectorDraft` as a set of pending `ColumnWrite`s and applied at a flush
boundary (the same boundary spec `01`/`06` use). This mirrors how Domain B
returns a `WorldDelta` that the host applies atomically.

```rust
pub struct InspectorDraft {
    pub entity: Entity,
    /// Pending component writes keyed by (archetype, component) → payload.
    pub writes: Vec<ColumnWrite>,
    pub components_to_add: Vec<ComponentId>,   // drives migration
    pub components_to_remove: Vec<ComponentId>,// drives migration
    pub revert_requested: bool,                // "reset to loaded values"
}
```

A small drag/typing guard debounces egui input (which fires on every keystroke)
so a partially typed float doesn't produce dozens of micro-deltas. The draft
holds *edited units*; a "commit" collapses them into a minimal set of
`ColumnWrite`s sent through the same `apply_delta` the guest uses.

### Typed editors vs raw fields

Two editing modes:

* **Typed editor.** The component registry knows each component's `Pod` schema.
  For the well-known scalar/vector types (`I16F16` scalars, `[I16F16; 2/3/4]`,
  `ComponentId`, `Entity`, enums with known variants) the inspector renders a
  dedicated control (drag/value box, color picker, dropdown). Numbers are
  edited in fixed-point: an `I16F16` field is shown via its `f32` projection
  through `openengine-math::quantize_to_f32` for display only, and committed
  back through `fx!(value)`. **The stored value is always fixed-point.**
* **Raw (hex) fields.** For unknown or opaque components, or an "advanced"
  toggle, the inspector shows the raw `&[u8]` per field with a hex editor. This
  is the escape hatch when no typed editor exists; edits are validated to keep
  `Pod` element_size and are still committed as `ColumnWrite`s.

A registry lookup decides which editor to use:

```rust
pub enum FieldEditor {
    F32Scalar, F32Vec2, F32Vec3, F32Vec4,   // display projection; store I16F16
    Enum(Vec<(u32, String)>),               // fixed discriminant
    EntityHandle, ComponentId, AssetPick,   // typed
    RawHex,                                 // fallback for unknown schemas
}
```

### Asset picker

When a component references an asset (by relative path / `AssetRef` — never an
absolute path, AGENTS.md § 5), the typed editor inserts an **asset picker**
button that opens an egui file/dialog browsing `OPENENGINE_ASSETS_PATH`. The
chosen asset is stored as its logical `relative_path` in the component, exactly
as gameplay expects it, and logged as a normal edit. Picking does not load the
asset itself (loading is the scene/asset manager's job); it only sets the
reference.

### Add / remove component (migration)

Adding or removing a component on an entity moves it between archetypes
(spec `00`, ECS add/remove → migration). The inspector requests this through
the draft (`components_to_add` / `components_to_remove`), and the commit
performs the migration deterministically:

1. Compute target archetype = current archetype ± component set.
2. Copy kept column bytes, (re)add/remove the target column.
3. Update the entity's archetype pointer.
4. Apply the migration and any pending field `ColumnWrite` in the same commit.

Because this mirrors guest migration exactly, a component added in the editor
produces the same world state as a guest spawn with that component — one code
path, deterministic. Component *schemas* themselves are registered in the
component registry (host, Domain A); the inspector does not invent schemas at
runtime.

### Revert

`InspectorDraft::revert_requested` discards staged writes and re-reads the
component's authoritative bytes from the world view — a "revert to loaded /
last-committed" button. Revert is pure local state drop; it issues no delta.
The canonical "undo history" (multi-step undo across operations) is out of
scope here and belongs to the editor/agent-OS milestone; the inspector only
provides one-level revert of the in-flight draft.

### No mutation during iteration

The invariant that protects the ECS: the inspector's read (build `WorldView`
snapshot) and its commit (flush the draft) are **strictly separated in the
frame**. The inspector never calls `apply_delta` or migrates an entity from
inside an egui paint closure or while another system iterates. Commits are
queued to the flush boundary; if the editor tries to write mid-iteration the
API refuses (returns an `Err(EditorEditError::Busy)`) rather than corrupt
storage.

## Key Rust / types

- `InspectorSystem`, `InspectorDraft`, `FieldEditor` — `crates/editor` (Domain A).
- `WorldView<'a>` — safe immutable SoA read of the current scene's `World`
  (host analog of `StateView`); the inspector depends on it, never on `&mut`.
- `EditorEditError` — host-side recoverable error for busy/validation failures
  (parallels `RecoverableError` semantics, host-typed).
- `SelectionModel` — shared selection state (this spec + spec `08`).
- `contracts::{Entity, ComponentId, ArchetypeId, ColumnWrite, WorldDelta}` and
  `openengine-math::{I16F16, fx!, quantize_to_f32}` for fixed-point display.

## Constraints

- Editor is Domain A (`crates/editor`); it may use `std`, `egui`, files — never
  compiled into the guest.
- The inspector writes only via `WorldDelta`/`ColumnWrite` at flush boundaries;
  it never mutates columns mid-iteration (hard invariant above).
- Stored component values are fixed-point; `f32` is a display-only projection
  through `quantize_to_f32`.
- Asset references are relative/logical paths resolved against
  `OPENENGINE_ASSETS_PATH`; no absolute/hardcoded paths.
- Add/remove component uses the same migration path as guest logic — no second,
  divergent ECS mutation path.
- Compiles on `x86_64-linux` and `aarch64-linux`; editor logic tests need no
  GPU and no window (headless egui-free unit tests for the draft/commit layer).

## Performance targets

- Inspector overhead per frame is bounded and only incurred for the selected
  entity: rebuild the component list + redraw the panel. Target < 1 ms/frame
  for a single entity with a dozen components.
- Commit of a draft emits a *minimal* set of `ColumnWrite`s (no full-entity
  rewrites); migration is the same cost as guest migration.
- Draft/commit layer is allocation-light on the commit hot path.

## Testing strategy

- **Unit (headless):** draft → commit produces a minimal, correct `ColumnWrite`
  set for typed scalar/vector/enum edits; raw-hex edits validate `element_size`.
- **Fixed-point round-trip:** an `I16F16` edited in the `f32` display projection
  and committed via `fx!` round-trips exactly for representable values.
- **Migration:** add/remove a component through the draft and assert the entity
  moved to the correct archetype with kept columns preserved — identical to the
  guest migration unit test in spec `00`.
- **Busy invariant:** attempt a commit while a simulated system iterates; assert
  an `EditorEditError::Busy`, never a partial write.
- **Asset picker:** set an `AssetRef` and assert the stored value is a relative
  path, not an absolute one.
- **Determinism (integration):** perform the same edit script on a scene three
  times and assert identical final world state (edits flow through the same
  deterministic `WorldDelta` path).

## Dependencies

- `crates/editor` (egui panel), `crates/ecs` (`World`, migration), `contracts`
  (`Entity`, `ColumnWrite`, `WorldDelta`, `ComponentId`), `openengine-math`.
- `OPENENGINE_ASSETS_PATH` from env (AGENTS.md § 5) for the asset picker.
- egui itself (Domain-A permitted) for the panel UI.

## Next steps

1. Implement `WorldView` (safe immutable SoA read) in `crates/ecs`.
2. Implement `SelectionModel` shared with spec `08`.
3. Implement `InspectorDraft` + the draft→`ColumnWrite` commit path.
4. Implement typed editors for `I16F16` scalars/vectors and enums, plus the
   raw-hex fallback.
5. Wire add/remove component to the migration path and the asset picker to
   `OPENENGINE_ASSETS_PATH`.
6. Implement one-level revert and the busy-invariant guard.
