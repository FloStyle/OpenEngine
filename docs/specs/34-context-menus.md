---
spec: "34-context-menus"
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
  - "28-keyboard-shortcuts"
  - "31-multi-selection"
  - "33-drag-drop"
---
# 34 - Context Menus

## Overview

A context menu (right-click) is the fastest way to act *on what the cursor is
over*. OpenEngine gives every primary surface a right-click menu built from one
declarative model so a human or agent sees consistent, discoverable actions that
all funnel into the same undoable command channel: **Viewport** (spec `24`),
**Hierarchy** (spec `08`), **Inspector** (spec `07`), and **Asset Browser**
(spec `26`).

This spec defines a **single menu model + builder** shared by all four contexts.
A right-click asks the focused context to *build a menu* for the current cursor
target and selection; the builder returns an ordered, grouped list of `MenuItem`s
— some static, many **dynamic from the current selection** (a selected entity
offers "Duplicate", a mesh entity offers "Add Material", a multi-select offers
group ops from spec `31`). Choosing an item resolves to an `EditorAction`; every
action that touches the world becomes one or more spec-`23` `Command`s pushed
through the `EditorCommandBus`. **Keyboard integration** is free by construction:
each menu item carries the same `ShortcutAction` name as a shortcut (spec `28`),
so a menu item and its keyboard chord are two faces of one action with a single
implementation and a single undo path.

Menus are Domain A (`crates/editor`) presentation + intent. Building a menu is a
pure function over `(context, cursor target, selection, WorldView)`; nothing is
mutated until the user picks an action, at which point the chosen action is routed
exactly like a toolbar button or shortcut (spec `25`/`28`) — never a bespoke
write path.

## Core Concepts

### One declarative menu model

Every context menu is a `Vec<MenuItem>` produced by a registered **builder**. A
`MenuItem` is plain data: a label, an optional action, an optional shortcut hint,
nested children (submenus), an enabled flag, and an optional separator.

```rust
/// One row in a context menu. Plain data; the shell (spec 25) renders it.
pub struct MenuItem {
    pub label: String,                       // "Duplicate", "Add Component ▸"
    pub action: Option<EditorAction>,        // None == submenu header / disabled
    pub shortcut: Option<ShortcutAction>,    // shows its chord (spec 28); not a hotkey of its own
    pub children: Vec<MenuItem>,             // submenu (e.g. "Add Component ▸")
    pub enabled: bool,                       // false → greyed out, still shown
    pub separator_after: bool,               // visual group break
}

/// Which surface the right-click happened on.
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum ContextKind { Viewport, Hierarchy, Inspector, AssetBrowser }

/// Everything a builder needs to produce the right items, pure.
pub struct MenuContext<'a> {
    pub kind: ContextKind,
    /// The entity/asset the cursor is directly over (None = empty space).
    pub hit: HitTarget,                      // Entity(Entity) | Asset(AssetId) | Folder(String) | None
    pub selection: &'a SelectionModel,       // spec 31 multi-select
    pub view: &'a WorldView<'a>,             // read-only
}
pub enum HitTarget { Entity(Entity), Asset(AssetId), Folder(String), Empty }

/// Builds a menu for a context. Pure over (context) — no mutation.
pub type MenuBuilder = Box<dyn Fn(&MenuContext<'_>) -> Vec<MenuItem> + Send + Sync>;
pub struct ContextMenuRegistry { builders: BTreeMap<ContextKind, MenuBuilder> } // deterministic
```

### Actions are the shared vocabulary (menu = one view of it)

An `EditorAction` is the same named action type the toolbar (spec `25`) and
shortcuts (spec `28`) resolve — the spec-`28` `ShortcutAction` string used as its
stable name. The menu builder *names* actions; a central `ActionDispatcher`
*executes* them:

```rust
pub enum EditorAction {
    // world-mutating (routed to spec 23 commands):
    CreateChild, Duplicate, Delete, Rename,
    ReparentTo(Option<Entity>),             // None == detach to root
    ResetComponent(ComponentId),            // reset a field/component to registry default
    CopyEntity, PasteEntity,
    CopyComponent(ComponentId), PasteComponent,
    AddComponent(ComponentId), RemoveComponent(ComponentId),
    // asset browser (routed to spec 02/26 pipeline):
    Reimport, ShowInExplorer, OpenExternal, DeleteAsset,
    // group (spec 31):
    Group(GroupOp),
    // editor/host (no world change, still routed once):
    Focus, SelectAll, None_,
}
```

The invariant that keeps this clean: **a `MenuItem` only *names* an action.** The
builder never performs it. When the user clicks, the shell calls
`ActionDispatcher::dispatch(action, context)`, which is the sole place an
`EditorAction` is turned into `Command`s on the `EditorCommandBus` (spec `23`) or
into an asset-pipeline call (spec `02`). Because the toolbar, shortcut table and
menu all resolve into the same `EditorAction` names, there is exactly one
implementation and one undo path per action (spec `28` "one shared vocabulary").

### Dynamic items from the selection

Menus are largely a function of what is selected and what is under the cursor
(spec `31`). A single builder for the Hierarchy/Viewport (entity) contexts
produces, for example:

- **Empty space** → `Spawn Entity ▸` (archetype submenu)… and pastes.
- **One entity** → `Create Child`, `Duplicate`, `Rename`, `Delete`, `Copy`,
  `Paste`, `Reparent to Root`, and an `Add Component ▸` / `Remove Component ▸`
  submenu listing the registered component types (spec `00` registry) that the
  entity does/does not already carry (add disables types already present; remove
  disables types absent).
- **Multi-select (spec `31`)** → `Duplicate N`, `Delete N`, and a `Group Ops ▸`
  submenu (`Move`/`Rotate`/`Scale`/`Align`/`Distribute`), all of which dispatch a
  spec-`31` composite command.
- A right-click **component row in the Inspector** offers `Reset`, `Copy
  Component`, `Remove Component` (dynamic: only for the hovered component);
  a right-click **on the inspector's blank header** offers `Add Component`.

The dynamic computation is pure: `builder` reads the `HitTarget`, the
`SelectionModel` set, and the entity's current components from `view`, and emits
the correct `enabled` flags and submenu contents. Two identical
`MenuContext`s always produce identical menus (deterministic — no `HashMap`
iteration, sorted component lists).

### Context-by-context contents

- **Hierarchy / Viewport (entity)**: Create Child, Duplicate, Rename, Delete,
  Copy/Paste, Reparent (menu alternate to the spec-`33` drag), Add/Remove
  Component, and — for a mesh with a material-capable component — a `Material ▸`
  submenu to assign a material (an alias of the spec-`33` Attach action).
- **Inspector**: component-level Reset / Copy Component / Paste Component /
  Remove Component; blank-area Add Component; on an `AssetRef` field, a `Browse…`
  shortcut to the spec-`07` asset picker (same action a drop would take).
- **Asset Browser** (spec `26` file/folder context): Open External, Rename,
  Delete, Reimport, Show in Explorer, Copy path, and tag editing — all routing to
  the spec-`02` pipeline + spec-`26` context-menu items.
- Console overlay (spec `27`) right-click inside the log offers Clear / Copy
  selection (text clipboard), reused from the console's own actions — a thin
  extra target, not a full menu context.

### Keyboard integration

Every item that has a backing action shows its shortcut chord from spec `28` as a
right-aligned hint (read-only display; the chord is *not* re-bound by the menu).
Pressing that chord anywhere invokes the *same* `EditorAction`, so
`Primary+C`/`Primary+V` (copy/paste entity or component, context-dependent) and
`Delete`/`F2` (rename) work whether or not a menu is open. When a menu is open,
the arrow keys navigate items and `Enter`/`Space` selects the highlighted one —
the keyboard never bypasses the dispatcher, so it produces the same undoable
commands as a mouse click. Where spec `28` has no default yet (e.g. rename is
`F2`), the menu's action still works by mouse; its chord hint only appears once a
binding exists. New actions are registered into spec `28` so their chords can be
assigned/remapped.

### All world mutations route through spec 23

Right-click → pick → **world mutation must be a command**. The chain is:

1. Builder produces items (pure).
2. User picks `MenuItem.action`.
3. The shell calls `ActionDispatcher::dispatch`.
4. World-affecting actions are translated into spec-`23` `Command`s
   (`SpawnEntityCommand` for Create Child / Duplicate, `DespawnEntityCommand` for
   Delete, `RenameEntityCommand` for Rename, `ReparentEntityCommand` for Reparent,
   `AddComponentCommand`/`RemoveComponentCommand`, `ModifyComponentCommand` for
   Reset, and spec-`31` composite group commands for multi-select ops).
5. The dispatcher pushes them through `EditorCommandBus`; multiple members of one
   action (duplicate N, group op) fold into a single composite undo step.
6. Only the bus applies the delta at the flush boundary.

Asset-browser items (Reimport/Delete/Show in Explorer/Open External) are not
world mutations; they route to the spec-`02`/`26` asset pipeline (undoable as
asset commands there) and to host open/explorer (Domain A only, best-effort, no
crash when unsupported). Copy/paste of an entity/component is an *editor
clipboard* (Domain-A value, never a world component): `CopyEntity` snapshots the
entity's columns into a `ClipboardPayload`; `PasteEntity` turns that snapshot
into a `SpawnEntityCommand` on the current selection's parent.

## Key Rust Types

```rust
// crates/editor/contextmenu — Domain A
pub struct MenuItem { pub label: String, pub action: Option<EditorAction>,
    pub shortcut: Option<ShortcutAction>, pub children: Vec<MenuItem>,
    pub enabled: bool, pub separator_after: bool }
pub enum ContextKind { Viewport, Hierarchy, Inspector, AssetBrowser }
pub enum HitTarget { Entity(Entity), Asset(AssetId), Folder(String), Empty }
pub struct MenuContext<'a> { pub kind: ContextKind, pub hit: HitTarget,
    pub selection: &'a SelectionModel, pub view: &'a WorldView<'a> }
pub type MenuBuilder = Box<dyn Fn(&MenuContext<'_>) -> Vec<MenuItem> + Send + Sync>;
pub struct ContextMenuRegistry { builders: BTreeMap<ContextKind, MenuBuilder> }

pub enum EditorAction { CreateChild, Duplicate, Delete, Rename,
    ReparentTo(Option<Entity>), ResetComponent(ComponentId),
    CopyEntity, PasteEntity, CopyComponent(ComponentId), PasteComponent,
    AddComponent(ComponentId), RemoveComponent(ComponentId),
    Reimport, ShowInExplorer, OpenExternal, DeleteAsset,
    Group(GroupOp), Focus, SelectAll, None_ }

pub struct ContextMenuSystem { pub registry: ContextMenuRegistry,
    pub open: Option<OpenMenu>, pub dispatcher: ActionDispatcher }
pub struct OpenMenu { pub at: egui::Pos2, pub items: Vec<MenuItem>,
    pub cursor: MenuCursor /* keyboard highlight */ }

pub enum ClipboardPayload {
    Entity(Vec<ColumnSnapshot>),  // entity copy (spec 16 column snapshots)
    Component { component: ComponentId, value: Vec<u8> },
    Empty,
}
pub struct ActionDispatcher { /* maps EditorAction -> commands on the bus, or
                                 asset-pipeline / host calls; see below */ }
```

Supporting types: spec-`23` `Command`/`EditorCommandBus`/concrete commands;
spec-`31` `GroupOp`/`GroupOperationCommand`; spec-`02` `AssetId`/pipeline;
spec-`07` `AssetRef`; spec-`28` `ShortcutAction`/`Chord`; `contracts` `Entity`/
`ComponentId`.

## Components

None — editor/UI tooling only; no new ECS component. Menus, actions, the menu
registry, and the editor clipboard are Domain-A structs, never registered
components and never stored in the world or the guest. Every action's effect
flows through existing spec-`23` commands (transform/parent/component
operations) or the existing asset pipeline, so this system introduces no entity
or component type.

## Constraints

- **Domain A only.** Context menus, builders, dispatcher and clipboard live in
  `crates/editor`; never compiled into Domain B.
- **Menus name actions; they never perform them.** The builder emits
  `EditorAction`s; `ActionDispatcher` is the only place they execute. A menu item
  has no direct world-mutation path.
- **Every world-affecting action is a spec-`23` command** (or composite) pushed
  through `EditorCommandBus`; menus add no second mutation channel (spec `23`).
- **Dynamic menus are deterministic.** Builders are pure over
  `(kind, hit, selection, view)`; component lists/submenus are sorted; no
  `HashMap` iteration order. Identical contexts ⇒ identical menus.
- **Keyboard and mouse are one action.** Items carry spec-`28` `ShortcutAction`
  names and chords; a chord and a click resolve to the same `EditorAction`, the
  same commands, the same undo step.
- **Edit-world only.** Entity/component actions are gated to `mode == Edit`
  (spec `22`); during play the mutating items are disabled (shown greyed with a
  tooltip, not hidden, so the user learns why).
- **Clipboard is Domain-A editor state**, not a world component; pasting
  re-derives a spawn/modify command and is undoable like any other edit.
- **Portable asset-browser items.** Open external / Show in Explorer are
  best-effort host actions, disabled where no handler exists (spec `26`); file
  paths remain project-relative (spec `30`).
- **Compiles on `x86_64-linux` and `aarch64-linux`.** Menu building, action
  resolution and command translation are pure and headless-testable with no GPU
  and no window.

## Performance Targets

- **Menu build:** a pure builder over the selection + hovered entity —
  **< 1 ms** typical; a 10k-entity context only enumerates the hovered entity's
  components plus, for multi-select, the selected set (no full-scene scan).
- **Menu open/paint:** plain egui; negligible.
- **Dispatch:** resolution + building ≤ a handful of commands — **< 1 ms** for
  single-item actions; multi-select group/duplicate batches into one composite
  (spec `31`).
- **Keyboard navigation:** O(items); no per-keystroke world work.
- **Closed menu idle cost:** ≈ 0.

## Testing Strategy

All headless (no GPU / no window) in `crates/editor`:

- **Builder correctness:** for a set of `(kind, hit, selection)` fixtures, assert
  the produced `Vec<MenuItem>` (labels, submenus, enabled flags, separators) is
  exactly expected. Run 3× for identical output.
- **Dynamic selection:** one-entity vs multi-select vs empty-space contexts
  produce the right actions (Duplicate N / Group Ops only on multi-select; Add
  Component only where components are absent).
- **Action → command mapping:** for each world-mutating `EditorAction`, assert
  the dispatcher pushes the expected spec-`23` command(s) onto a mock
  `EditorCommandBus` and issues **no** direct world write.
- **Undoability:** scripted right-click actions (Duplicate, Delete, Rename,
  Reparent, Add/Remove Component, Reset) → Undo/Redo return the world to
  bit-identical prior/final states (spec `23`/`16` `WorldHash`).
- **Keyboard parity:** a `Delete` keypress and a right-click → Delete produce the
  same action/commands (assert through the shared dispatcher).
- **Edit-vs-play gating:** while the play mode is active, mutating items are
  disabled and dispatching them is rejected (spec `22`).
- **Clipboard:** CopyEntity then PasteEntity re-derives an identical
  `SpawnEntityCommand`; Copy/PasteComponent round-trips a component's bytes; a
  paste onto a stale entity returns an error, never a corrupt write.
- **No direct mutation:** assert the menu/dispatcher surface exposes no
  world-write path outside the bus.

## Dependencies

- `crates/editor` (Domain A) — menu model, builders, `ContextMenuSystem`,
  `ActionDispatcher`, clipboard; mounted into the shell (spec `25`).
- Undoable `Command`/`EditorCommandBus` + concrete commands from spec `23`;
  edit-world gating from spec `22`; multi-select + group ops from spec `31`.
- Contexts: hierarchy structural ops (spec `08`), inspector component ops
  (spec `07`), viewport (spec `24`), asset browser items (spec `26`).
- Shared action/shortcut vocabulary from spec `28`; asset pipeline + `AssetId`
  from spec `02`; drag-drop Attach alias from spec `33`; toasts/errors surfaced
  from spec `35`; project relative paths from spec `30`.
- `egui` (Domain A), `contracts` (`Entity`, `ComponentId`). No new
  `contracts`/ABI surface.

## Next Steps

1. Define `MenuItem`/`MenuContext`/`HitTarget`/`ContextKind` and the
   `ContextMenuRegistry` + pure `MenuBuilder`.
2. Implement `EditorAction` + `ActionDispatcher` mapping every world action onto
   spec-`23` commands (and asset actions onto spec `02`).
3. Build the Hierarchy/Viewport entity builder (create child / duplicate /
   delete / rename / reparent / add-remove component) and the Inspector builder
   (reset / copy-paste / add-remove).
4. Build the Asset Browser builder (open external / rename / delete / reimport /
   show in explorer / tags) onto spec `26`.
5. Add keyboard navigation + the shared-action/chord hints via spec `28`; wire
   the editor clipboard (entity/component copy-paste).
6. Register the four context builders into the shell (spec `25`) and land the
   headless builder/dispatcher/undoability test battery.
