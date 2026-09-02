---
spec: "23-undo-redo"
phase: "Phase 5: Editor"
status: "draft"
author: "OpenEngine AI"
created: "2026-09-03"
depends_on:
  - "00-ecs-architecture"
  - "16-serialization"
  - "22-edit-vs-play"
---
# 23 - Transactional Undo / Redo

## Overview

Undo/redo turns every editor mutation into a reversible, inspectable,
serializable operation. The rule that makes this tractable is simple and
matches how the rest of the editor already mutates the world: **every editor
action is a `Command`**, and a `Command` never touches ECS storage directly —
it translates, at execution time, into a `WorldDelta` (spawns / despawns /
`ColumnWrite`s / `DeferredCommand`s) that the host ECS applies atomically at a
flush boundary via `apply_delta` (spec `00`). Because the mutation channel is
already a delta, *undo is just applying the inverse delta*, and *redo is just
re-applying the original delta* — no second, divergent mutation path is ever
invented.

The command pattern is therefore not bolted on top of the editor; it is the
editor's only write path. Spec `07` (inspector), `08` (hierarchy), and `09`
(gizmos) all currently stage edits into a `WorldDelta` and flush at the
boundary. Undo/redo wraps that exact path so a flush is owned by a
`Command::execute` (and its inverse by `Command::undo`). UI never calls
`apply_delta` on its own — it always goes through the `UndoRedoManager`, which
owns execute/undo/redo and is the sole producer of edit-world deltas.

Undo/redo lives entirely in **Domain A** (`crates/editor`). Domain B never
sees it: the guest is pure and forward-only; there is no "undo" inside a
simulation tick. Undo only ever operates on the **edit-world** (spec `22`
edit-vs-play), never the play-world.

## Core Concepts

### A `Command` is a reversible, self-describing delta factory

A command records *enough intent* to (a) build the forward delta on `execute`,
(b) build the inverse delta on `undo`, (c) render a human/agent-readable
description for the UI and event log, and (d) serialize to bytes for crash
recovery (spec `16`). The canonical example is a component value change, which
stores both the old and the new raw byte payload:

```rust
pub trait Command: Send {
    /// Apply the command to the *edit world* and push the forward delta.
    /// Implementations build a `WorldDelta` and return it; the manager applies
    /// it atomically at the flush boundary. `Err` rolls back nothing (delta not
    /// applied) and surfaces `EditorError`.
    fn execute(&mut self) -> Result<WorldDelta, EditorError>;

    /// Return the inverse delta that restores the pre-execute state. Must be
    /// callable *after* `execute` and must mirror `execute` exactly
    /// (bit-identical world if undone immediately).
    fn undo(&self) -> Result<WorldDelta, EditorError>;

    /// Human/agent-readable label, e.g. "Move Sphere". Display-only.
    fn description(&self) -> String;

    /// Serialize this command for crash recovery / persistence. Because
    /// `Box<dyn Command>` is a trait object (postcard cannot round-trip trait
    /// objects), this returns the **concrete, kind-tagged postcard payload**
    /// (`CommandKind` discriminant + the concrete command struct's postcard
    /// bytes). See "History persistence & crash recovery" below.
    fn serialize(&self) -> Vec<u8>;

    /// Generation guard token captured at authoring time (see Constraints).
    fn entity_handle(&self) -> Option<Entity>;
}
```

A command must be **re-runnable**: calling `execute()` after a round-trip
through `serialize()`/deserialize must produce a byte-identical `WorldDelta`.
This is what makes undo/redo deterministic and crash-recoverable.

### History persistence & crash recovery (tagged command-kind encoding)

The in-memory stacks hold `Box<dyn Command>` trait objects, but a persisted
history snapshot **cannot serialize a `Box<dyn Command>` directly** — postcard
has no representation for a vtable / trait object, so it cannot round-trip one.
Persistence therefore uses a **tagged command-kind encoding**:

```rust
/// One entry in the persisted history. The discriminant tells the decoder which
/// concrete command struct the payload holds; `payload` is that struct's plain
/// postcard bytes (all concrete commands below are `serde` + postcard-friendly).
#[derive(Clone, Debug, serde::Serialize, serde::Deserialize)]
pub struct PersistedCommand {
    pub kind: CommandKind,
    pub payload: Vec<u8>,          // postcard(ConcreteCommand)
}

/// Discriminant per concrete command type. Adding a command type must register a
/// new variant here AND a decoder arm in the `CommandKind` registry.
#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum CommandKind {
    ModifyComponent,
    SpawnEntity,
    DespawnEntity,
    AddComponent,
    RemoveComponent,
    RenameEntity,
    ReparentEntity,
    Composite,
}
```

A `CommandKind` registry pairs each variant with a decoder that reconstructs the
concrete struct from `payload` and upcasts it to `Box<dyn Command>`. On load,
each `PersistedCommand` is decoded through the registry by its `kind`
(unknown/missing kinds fail loudly with `EditorError` rather than silently
dropping history), so a full snapshot round-trips to an identical
`UndoRedoManager` whose stacks reproduce byte-identical forward/inverse deltas.

### The manager owns the two stacks

```rust
pub struct UndoRedoManager {
    /// Applied commands, oldest at the bottom. Index 0 is the first action.
    undo_stack: Vec<Box<dyn Command>>,
    /// Undone commands, available for redo (cleared on any new edit).
    redo_stack: Vec<Box<dyn Command>>,
    /// `undo_stack.len()` value that marks the last saved/clean point.
    save_point: usize,
    /// Maximum retained history depth (default 1000, configurable).
    max_history: usize,
    /// Monotonic serial/id source so recorded commands keep a stable order.
    sequence: u64,
}
```

Core methods:

- `execute(&mut self, mut cmd: Box<dyn Command>) -> Result<WorldDelta, EditorError>`
  — ask the command for its forward delta, **clear `redo_stack`**, push onto
  `undo_stack`, apply FIFO eviction when over `max_history`, return the delta.
- `undo(&mut self) -> Result<Option<WorldDelta>, EditorError>` — pop the top of
  `undo_stack`, obtain its inverse delta, move the command onto `redo_stack`,
  return the inverse delta to apply. Returns `None` when empty.
- `redo(&mut self) -> Result<Option<WorldDelta>, EditorError>` — pop `redo_stack`,
  re-`execute` (or replay its forward delta), push back onto `undo_stack`.
- `set_save_point(&mut self)` / `is_dirty(&self) -> bool` — record / compare the
  current `undo_stack.len()` against `save_point`.
- `clear(&mut self)` — reset both stacks (used on scene load, spec `06`).

The manager returns the **delta**, and the caller (the editor frame driver)
applies it at the flush boundary with `apply_delta(world, &delta)`. The manager
never borrows `&mut World`; keeping delta production separate from application
preserves the "apply at flush" invariant of specs `07`/`08`/`09`.

### Transaction batching: one gesture = one undo step

The crux problem named in the overview: a gizmo drag produces ~60 frames of
`ColumnWrite`s, and we must not push 60 commands onto the undo stack. Solution:
a **transaction boundary** around a gesture. A `Transaction` opens on the
pointer-down that captures the gesture and closes on pointer-up / commit. While
a transaction is open, incremental per-frame deltas are applied to the world
normally but are **not** pushed as individual commands; instead they are folded
into the transaction's recorded history.

Two coherent designs are acceptable; this spec adopts **record-then-collapse**:

1. On pointer-down, open a `Transaction`. Subsequent `execute` calls push onto
   the undo stack but mark the entries as belonging to the open transaction.
2. On commit (pointer-up, or a discrete action completing), the manager
   **collapses** all commands belonging to the just-closed transaction into a
   single *composite* command whose `undo()` replays the members' inverse deltas
   in reverse order and whose `execute()` replays the members' forward deltas in
   order.

```rust
pub struct CompositeCommand {
    members: Vec<Box<dyn Command>>,   // applied in order on redo
    description: String,
}
impl Command for CompositeCommand {
    fn execute(&mut self) -> Result<WorldDelta, EditorError> {
        // merge members' forward deltas in order into one WorldDelta
        Ok(concat(&self.members_forward()))  // helper: fold deltas
    }
    fn undo(&self) -> Result<WorldDelta, EditorError> {
        // members' inverse deltas, applied in REVERSE order
        Ok(concat_rev(&self.members_undo()))
    }
    // ...
}
```

Because `ColumnWrite` payloads in a merged delta are per-(archetype, component,
indices) the merge must keep writes to the same column ordered; the merge helper
appends writes in member order, which preserves last-write-wins semantics
correctly for a drag. Alternatively — and preferred for a single-entity drag —
each per-frame `ModifyComponentCommand` carries old/new bytes, and the composite
can be **collapsed to a single** `ModifyComponentCommand` whose `old_value` is
the *first* member's old bytes and `new_value` is the *last* member's new bytes.
This "diff-from-first-to-last" collapse yields a compact, exact one-step undo.
The editor chooses collapse when all members mutate the same single value (a
drag), and falls back to composite replay when members span distinct
operations (a multi-select batch).

### Undo is the inverse delta; it reuses `apply_delta`

There is exactly one world-mutation function on the host:
`apply_delta(world, &WorldDelta)` (spec `00`). `execute` produces the forward
delta; `undo` produces the inverse delta. Both go through the same `apply_delta`
so generation guards, column bounds checks, archetype existence checks and
rollback-on-error behave identically to gameplay. Undo never reaches around the
ECS API to rewrite memory.

## Key Rust Types

- `Command` trait (above) — `crates/editor` / `crates/editor::commands`.
- `UndoRedoManager` (above).
- `EditorError` — host-side error type already shared with spec `07`'s
  inspector (parallels `RecoverableError` semantics). Used for e.g. stale
  generation, busy apply, unknown component, serialization failure.
- `Transaction` — an open gesture/action boundary accumulating member commands.

### Concrete commands

```rust
/// Move a gizmo-dragged entity or a discrete value change. Canonical example.
pub struct ModifyComponentCommand {
    pub entity: Entity,
    pub component_id: ComponentId,   // registry-known component type
    pub archetype: ArchetypeId,      // cached at authoring time
    pub old_value: Vec<u8>,          // pre-edit raw Pod bytes (element_size)
    pub new_value: Vec<u8>,          // post-edit raw Pod bytes (element_size)
    pub description: String,         // display label, e.g. "Move Cube"
}
```

Its `execute()` builds a forward `ColumnWrite`
(`indices = [entity.index]`, `payload = new_value`) and `undo()` builds the
inverse with `payload = old_value`. Both validate that `payload.len() ==`
registered `element_size` (spec `00` column contract).

The remaining command types share the pattern; each is a small `#[derive]`
struct in `crates/editor::commands`:

```rust
pub struct SpawnEntityCommand {
    pub archetype: ArchetypeId,
    pub parent: Option<Entity>,          // spawn under a parent (spec 08)
    pub spawned: Option<Entity>,         // resolved handle after first execute
    pub initial: Vec<ColumnWrite>,       // zeroable defaults + authored overrides
}
pub struct DespawnEntityCommand {
    pub entity: Entity,
    pub generation_at_author: u32,       // guard against slot recycling
    pub removed_archetype: Option<ArchetypeId>,  // cached at execute
    pub removed_columns: Vec<(ComponentId, Vec<u8>)>, // captured for undo
    pub removed_parent_links: Vec<(Entity, Entity)>,  // re-attach on undo
}
pub struct AddComponentCommand {
    pub entity: Entity,
    pub component_id: ComponentId,
    pub default_value: Vec<u8>,          // zeroable / registry default
}
pub struct RemoveComponentCommand {
    pub entity: Entity,
    pub component_id: ComponentId,
    pub removed_value: Vec<u8>,          // captured at execute for undo
}
pub struct RenameEntityCommand {
    pub entity: Entity,
    pub old_name: String,
    pub new_name: String,
}
pub struct ReparentEntityCommand {
    pub entity: Entity,
    pub old_parent: Option<Entity>,      // None == root
    pub new_parent: Option<Entity>,
}
```

Because **undo must reverse a structural command exactly**, the structural
commands capture enough state during `execute` to reconstruct the inverse:

- `SpawnEntityCommand::undo` → a despawn of `spawned` (guarded by generation).
- `DespawnEntityCommand::undo` → a respawn carrying `removed_columns` back into
  the original `removed_archetype` and re-establishing parent links.
- `AddComponentCommand::undo` → a remove of that component.
- `RemoveComponentCommand::undo` → an add of `removed_value`.
- `RenameEntityCommand::undo` → set name back to `old_name`.
- `ReparentEntityCommand::undo` → set parent back to `old_parent`.

Despawn/undo interplay with the generation guard is covered under Constraints.

## Constraints

- **Commands are the ONLY path for editor mutations.** No spec may introduce a
  second mutation channel. Every editor write is expressed as a `Command` that
  yields a `WorldDelta`, applied through `apply_delta`; there is no `QueryMut`
  write path, no direct ECS write from UI/inspector/hierarchy/gizmos, and no
  other editor-side mutation API.
- **Edit-world only.** Commands produce deltas for the **edit world** (spec `22`
  edit-vs-play). Undo/redo never touch the play world; the play world is a
  separate sandboxed simulation that ignores editor history. This is
  non-negotiable: undo of an edit while play is live must not mutate a running
  gameplay simulation.
- **No direct ECS mutation from UI.** Every editor write — inspector field,
  hierarchy structural op, gizmo drag — must be expressed as a `Command` and
  routed through the `UndoRedoManager`. UI never calls `apply_delta` or migrates
  an entity itself. This is the invariant that keeps history *complete*; a
  bypass would make history diverge from world state.
- **Apply at flush boundary.** Commands *produce* deltas; the frame driver
  *applies* them at the flush boundary (specs `01`, `06`, `07`). No command may
  mutate ECS storage mid-iteration; a busy attempt returns
  `EditorError::Busy` (same guard as spec `07`).
- **Generation guard / ABA protection.** Commands capture generation at
  authoring time. A despawn command **cannot be undone** if the freed slot has
  since been recycled to a new entity: `entity.generation` captured at author
  must equal the generation the world reports for `entity.index` at undo time.
  If it differs, the despawn's inverse is refused with
  `EditorError::StaleGeneration` (undo of the despawn would otherwise resurrect a
  value over a brand-new entity). Similarly `ModifyComponentCommand::undo` checks
  the target's generation before writing `old_value`.
- **Determinism.** Identical command + identical pre-state ⇒ identical forward
  and inverse deltas. Old/new payloads are captured raw (`Vec<u8>` of Pod bytes),
  never re-derived from display `f32` at undo time. No `HashMap` iteration in
  command construction or stack management (use `Vec`, sorted where needed).
  Fixed-point values stay fixed; `f32` appears only inside an existing
  presentation path, never in a stored payload.
- **Redo cleared on new edit.** Any new `execute` clears `redo_stack`. Undo
  then a different action is a permanent branch.
- **Save point.** `save_point` is a *stack-depth marker*, updated by
  save (`spec 16`) and by the explicit `set_save_point`. `is_dirty() ==`
  `undo_stack.len() != save_point`. On exit / scene-switch, if `is_dirty()` the
  editor warns "unsaved changes" before discarding (see spec `06` scene
  lifecycle).
- **FIFO eviction.** When `undo_stack.len()` exceeds `max_history`
  (default 1000, configurable via editor settings / env), the *bottom* commands
  are dropped. A dropped command may still make `save_point` unreachable; the
  manager keeps `save_point` clamped to the current floor and recomputes dirtiness
  conservatively (an eviction that passes the old save point marks the document
  effectively "cannot be saved back to that exact depth" → treated as needing a
  save).
- **Serializable for crash recovery.** Commands persist via the **tagged
  `CommandKind` registry** (see "History persistence & crash recovery" above) —
  postcard cannot round-trip a `Box<dyn Command>` trait object, so the persisted
  history is a `Vec<PersistedCommand>` of `(kind, postcard concrete payload)`
  entries, decoded by kind on load. A full history snapshot can be persisted
  alongside the scene (spec `16`) so a crash/restart can restore undo history.
  Every command records only stable handles and raw bytes — no pointers, no host
  addresses, no vtables.
- **Portability / host-only.** Undo/redo lives in Domain A (`crates/editor`),
  compiled on `x86_64-linux` and `aarch64-linux`; never exported to the guest.
  Headless editor logic tests (no GPU, no window) drive commands and the manager
  directly.

## Performance Targets

- `execute` / `undo` / `redo` of a discrete single-value command:
  **< 1 ms** each (dominant cost is building a `ColumnWrite` of ≤ a few bytes).
- Transaction collapse of a full gizmo drag (60 frames): collapse + push as one
  step **< 1 ms**, and the resulting composite undo is one `apply_delta`.
- History maintenance (push/pop/eviction): `O(1)` amortized, no full-stack copy.
- `serialize()` of a single command: **< 100 ms** budget ceiling (in practice
  sub-ms for value commands); a full-history snapshot to disk is bounded and
  run in the background, never on the UI thread, if > ~1 MB.
- Memory: a composite command stores bounded old/new bytes; long drags collapse
  so history does not balloon with 60× redundant payloads.

## Testing Strategy

All tests headless (no GPU / no window) in `crates/editor`:

- **Unit per command type.** For each of the seven concrete commands: build it
  against a seeded world, `execute`, assert the forward `WorldDelta` is correct;
  apply it, then `undo`, apply the inverse delta, and assert the world is
  bit-identical to the pre-execute state. Cover `ModifyComponentCommand`
  explicitly with old/new raw byte payloads.
- **Complex edit sequence undo-all / redo-all.** Script a realistic edit
  sequence (spawn → add component → modify → rename → reparent → modify →
  despawn → modify another). Undo all the way to empty, assert the world equals
  the initial snapshot; redo all the way, assert it equals the final snapshot.
  Both assertions must be **bit-identical** to the corresponding directly-built
  worlds (compare every archetype column via a `WorldHash` / snapshot check from
  spec `16`).
- **Replay determinism.** Run the full undo-all/redo-all script **3×**; assert
  byte-identical world state at every step across the three runs.
- **Edge cases.**
  - Undo past the save point is *allowed* (undo/redo is depth-based); assert the
    document simply becomes "dirty relative to a now-farther save point" and warn
    correctly at exit.
  - Redo is cleared after a new edit: undo twice, perform a new action, assert
    `redo_stack` is empty and `redo()` returns `None`.
  - Undo of a stale-generation entity: despawn an entity, spawn another into the
    recycled slot (bump generation), then attempt the despawn command's undo —
    assert `EditorError::StaleGeneration`, never a corrupt write.
  - FIFO eviction at `max_history`: exceed the cap, assert bottom commands are
    dropped and `is_dirty()`/save-point bookkeeping stays consistent.
- **Transaction batching.** Drive a synthetic gizmo drag emitting 60
  `ModifyComponentCommand`s inside one transaction; assert exactly **one** undo
  step results and that a single undo restores the original value in one delta.
- **Serialization / persistence.** Encode each concrete command as
  `(CommandKind, postcard payload)`, decode it back through the `CommandKind`
  registry, and assert `execute` reproduces a byte-identical delta. Persist a
  full history snapshot (a `Vec<PersistedCommand>`) with `postcard`, reload it
  into a fresh `UndoRedoManager`, and assert the same undo/redo behavior on the
  same world (crash-recovery path). Also assert that an unknown/missing
  `CommandKind` on load fails with `EditorError` rather than silently truncating
  history.
- **Edit-vs-play isolation.** While a (headless) play-world tick runs, assert
  that undo/redo commands are refused or only mutate the edit world and never
  surface in the play-world columns (spec `22`).

## Dependencies

- `crates/editor` (Domain A, host) — `Command`, `UndoRedoManager`,
  `Transaction`, concrete commands.
- `crates/ecs` (`World`, `apply_delta`, migration, `WorldView`) — spec `00`.
- `contracts` (`Entity`, `ComponentId`, `ArchetypeId`, `ColumnWrite`,
  `WorldDelta`, `SpawnCommand`) — spec `00` + `contracts/src/lib.rs`.
- `openengine-math` (fixed-point payloads); `postcard` + `serde` for
  serialization (spec `16`).
- Edit-vs-play separation from spec `22`; scene lifecycle from spec `06`;
  shared `SelectionModel`/`WorldView`/`EditorError` from specs `07`/`08`.
- Structural commands originate in specs `07`/`08`/`09` (inspector/hierarchy/
  gizmo) — this spec wraps their existing flush path rather than replacing it.

## Next Steps

1. Define `Command` trait + `EditorError::StaleGeneration`/`Busy` variants in
   `crates/editor`.
2. Implement `UndoRedoManager` (stacks, save point, FIFO eviction, redo-clear).
3. Implement the seven concrete commands with correct forward/inverse deltas and
   generation guards.
4. Implement `Transaction` + composite/collapse for gesture batching.
5. Wire the manager into the frame flush boundary so the inspector/hierarchy/
   gizmo edits flow through it (and only it).
6. Add the `CommandKind` tagged encoding + registry and postcard full-history
   persistence (`Vec<PersistedCommand>`) for crash recovery.
7. Persist `save_point` semantics with scene save/load (spec `16`) and the
   exit-unsaved-changes warning.
8. Land the headless test suite (unit, undo-all/redo-all, determinism, edge
   cases).
