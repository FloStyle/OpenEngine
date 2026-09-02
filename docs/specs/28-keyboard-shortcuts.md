---
spec: "28-keyboard-shortcuts"
phase: "Phase 5: Editor"
status: "draft"
author: "OpenEngine AI"
created: "2026-09-03"
depends_on: ["25-editor-shell"]
---

# Keyboard Shortcuts

## Overview

A configurable, context-sensitive shortcut system for the editor, with
Unreal/Unity-like defaults and platform-aware modifiers (`Cmd` on macOS, `Ctrl`
everywhere else). A `ShortcutManager` owns a set of `Shortcut` bindings, each
scoped to a `ShortcutContext` (Global, Viewport, Hierarchy, Inspector, Console),
parses the serialized shortcut format `Ctrl+Shift+Key` / `Alt+Key`, detects
conflicts at definition time, and maps a raw key/modifier event into the right
action for the currently focused context. Bindings are user-editable at runtime
through **Settings → Shortcuts** and persist as JSON under
`OPENENGINE_CONFIG_PATH/shortcuts.json` (AGENTS.md § 5: no absolute/hardcoded
/home paths). The mapping is re-evaluated live — remapping a shortcut takes
effect on the next event, no restart.

The system sits in Domain A (`crates/editor`), sits on top of `winit`'s raw key
delivery (already present in the renderer/host, spec `04`), and feeds actions
into the same editor action path that menus, toolbars, and the console
(spec `27`) share. It never reaches into the ECS or Domain B — a shortcut only
triggers an *editor action*, and every world-mutating action goes through the
normal undoable `Command` machinery (spec `23`).

## Core Concepts

### Modifier model (platform-aware)

A physical modifier is normalized before matching so bindings do not need
per-OS duplicates. The **primary** modifier is `Cmd` on `macOS` and `Ctrl`
elsewhere; `Alt` and `Shift` are stable on all platforms. Bindings are authored
with the logical modifier names (`Primary`, `Alt`, `Shift`); on load, `Primary`
is expanded to the correct physical key for the current platform. The raw
`Ctrl+Z` binding still works on macOS when the user *physically* holds Ctrl, but
the default is authored as `Primary+Z`.

```rust
/// Logical modifiers after platform normalization.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
pub struct Modifiers {
    pub primary: bool,   // Cmd on macOS, Ctrl elsewhere (expanded at load)
    pub alt: bool,
    pub shift: bool,
}
```

### Shortcut format and parsing

The serialized format is the friendly string form `Ctrl+Shift+Key` / `Alt+Key`
used in docs and the Settings UI. The parser is case-insensitive for modifier
names and maps them to logical modifiers (`Ctrl`/`Cmd` → `primary`, plus a
`from_ctrl_or_cmd` flag so a literal Ctrl or Cmd authored by hand is honored on
the matching platform), then the trailing token is a named key (`Key::Escape`,
`Key::KeyW`, `Key::Digit1`, ...). The inverse formatter renders a binding back
to the friendly string using the *current* platform's primary label, so on
macOS a `Primary+Z` shows as `Cmd+Z`.

```rust
/// One named, user-facing command that a shortcut can trigger.
#[derive(Clone, Debug, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
pub struct ShortcutAction(String); // e.g. "editor.save_scene", "viewport.focus"

/// The active editing/input focus. A binding only fires in its context.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
pub enum ShortcutContext { Global, Viewport, Hierarchy, Inspector, Console }
```

### A binding

```rust
/// A parsed binding. `keys` is the primary key plus any chord of modifiers.
#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct Shortcut {
    pub action: ShortcutAction,      // what to invoke
    pub keys: Vec<String>,           // normalized friendly tokens, e.g. ["Primary","W"]
    pub modifiers: Modifiers,        // redundant with keys; kept for matching
    pub context: ShortcutContext,    // which surface owns this binding
}
```

### Context routing

At any moment exactly one **focus context** is active (Global is always also
active). An incoming `winit::KeyboardInput` is first offered to the top of a
focus stack — e.g. when the Console overlay (spec `27`) is open, Console wins
for the keys it wants (text, `Enter`, `Tab`, arrows) and Global-only bindings
that do not collide are still honored. If the focused context does not consume
the event, the `ShortcutManager` looks up the binding in
`focused_context ∪ Global`. This is how `~` opens the console globally while `W`
means *move camera* in the Viewport but *type W* in a text field.

### Defaults

Defaults mirror Unreal/Unity muscle memory and are documented in **Default
shortcut table** below. They are the *identity* bindings loaded when no
`shortcuts.json` exists yet; a user remap overlays them. Reset restores them.

### Conflict detection

Two bindings conflict when they share `(context, keys, modifiers)` but map to
different actions, or when they share a context *and* differ only in a way that
cannot be disambiguated (identical chord in the same context). Detection runs at
(1) definition/registration time, (2) when a plugin (spec `29`) registers an
action+binding, and (3) when a user saves an edit from the Settings UI. A
conflict is reported to the caller with both bindings; a *newer* definition
needs an explicit override flag or it is rejected so silent shadowing never
happens. Cross-context bindings never conflict (they cannot fire together), so
`Ctrl+S` Global and `Ctrl+S` in Console are both legal.

## Default shortcut table

| Action | Default | Context |
|--------|---------|---------|
| File: save scene | `Primary+S` | Global |
| File: open scene | `Primary+O` | Global |
| Edit: undo | `Primary+Z` | Global |
| Edit: redo | `Primary+Y` | Global |
| Edit: select all | `Primary+A` | Hierarchy |
| Edit: delete | `Delete` | Hierarchy |
| View: focus selection | `F` | Viewport |
| View: viewport 1/2/3/4 | `1`/`2`/`3`/`4` | Viewport |
| View: toggle grid | `G` | Viewport |
| Gizmo: translate | `W` | Viewport |
| Gizmo: rotate | `E` | Viewport |
| Gizmo: scale | `R` | Viewport |
| Tool: select | `Q` | Viewport |
| Sim: play / pause | `Space` | Global |
| Sim: stop | `Primary+Space` | Global |
| Console: toggle | `~` (Backquote) | Global |
| Panel: focus hierarchy / inspector | `Primary+1`/`Primary+2` | Global |

`F`/`G`/`Q`/`W`/`E`/`R` and the digits only fire when the Viewport/Hierarchy
context is focused so a text editor in a property field never triggers them.

### Relation to other editor input

The shortcut system is only one of several input paths into the editor. The
pointer-driven paths (toolbar buttons, right-click context menus, gizmo drag
handles from spec `09`) and the console command line (spec `27`) all resolve to
the **same** `ShortcutAction` / spec `23` vocabulary. A user can therefore save
with `Ctrl+S`, the File menu "Save", or the console `save` command — each is a
single action under the hood, so there is exactly one path for a world
mutation and exactly one place to make it undoable. Shortcuts are the *fast*
face of that shared vocabulary, never a parallel implementation.

## Settings → Shortcuts UI

A `egui` page lists every action grouped by context, each row showing its
current chord. Clicking a row enters **capture mode**: the next chord typed
(press-and-release) becomes the binding, which is validated for conflicts before
being accepted. A "Reset" button restores all bindings to the defaults, and the
whole table is written to `OPENENGINE_CONFIG_PATH/shortcuts.json` on change
(de-bounced), and re-loaded at editor start (user file overlays defaults).

```rust
// crates/editor/shortcuts — Domain A
pub struct ShortcutManager {
    /// (context, chord) -> action, the effective match table.
    bindings: BTreeMap<(ShortcutContext, Chord), ShortcutAction>,
    pub defaults: Vec<Shortcut>,       // immutable identity set
    pub custom: Vec<Shortcut>,         // user overlays, serialized
    focus: ShortcutContext,            // current focused context
}
```

`BTreeMap` (not `HashMap`) keeps matching and serialization order stable, which
matters for reproducible conflict reports and diffable JSON.

## Key Rust Types

- `ShortcutManager` — owns `bindings`/`defaults`/`custom`, `focus`, parse/serialize.
- `Shortcut { action, keys, modifiers, context }`.
- `ShortcutContext { Global, Viewport, Hierarchy, Inspector, Console }`.
- `ShortcutAction(String)`, `Modifiers { primary, alt, shift }`.
- `Chord` — an interned, ordered `(Vec<Key>, Modifiers)` normalization used as
  the match key; parsed from friendly strings and from raw `winit` events.
- `ShortcutError` — `Parse`, `Conflict { a, b }`, `UnknownKey`, `UnknownAction`.
- `SettingsAction` — the resolved action name the shell dispatches after a match
  (shared vocabulary with menus/console).

## Constraints

- **Domain A only.** Lives in `crates/editor`, on top of `winit`; never compiled
  into the guest, never touches ECS or Domain B.
- A shortcut only emits a `ShortcutAction`; all world mutation still flows
  through spec `23` undoable `Command`s. Shortcuts add **no** second mutation
  path.
- Bindings are context-scoped; a chord never fires outside its context, so text
  entry and gameplay input are never hijacked.
- Configuration JSON lives under `OPENENGINE_CONFIG_PATH` only; **no
  absolute/hardcoded/home paths** (AGENTS.md § 5).
- Platform-aware modifiers: authored as `Primary`, expanded to `Cmd`/`Ctrl` at
  load; parsing accepts both spellings.
- Conflicts are detected and never silently shadowed; a remap needs explicit
  override.
- Runtime-remappable — bindings are re-read per event, no restart.
- Compiles on `x86_64-linux` and `aarch64-linux`; parsing/conflict/serialization
  logic is headless-testable (no GPU/window needed).

## Performance Targets

- Chord lookup on a key event: O(log n) over bindings via `BTreeMap` — target
  < 1 µs; never a linear scan per keypress.
- Parse a friendly chord into a `Chord`: microsecond, done once at load/edit.
- Serialize the full table (< ~200 rows): sub-millisecond; persisted on
  debounced change/exit, not per keystroke.
- Idle cost with no key events: 0. Open Settings page cost is that of an egui
  table, negligible at editor frame rates.

## Testing Strategy

- **Parsing:** `Ctrl+Shift+Key`/`Alt+Key`/`Primary+W` round-trip; case
  insensitivity; malformed strings (`Ctrl+`, `++X`, empty) → `ShortcutError`.
- **Conflict detection:** duplicate chord in the same context → `Conflict`;
  cross-context duplicates are legal; plugin-vs-user conflict is caught at
  registration.
- **Serialization:** a set of bindings → `shortcuts.json` → reload → identical
  match table; `defaults` identity set unchanged after a reset.
- **Press → action (integration, headless):** feed synthetic `winit` key events
  in a known focus context and assert the correct `ShortcutAction` is emitted;
  assert `W` in the Viewport triggers *translate* while in a text field it is
  swallowed.
- **Ctrl vs Cmd:** on a macOS-simulated platform the authored `Primary+Z` maps
  to `Cmd+Z`; the same binding expands to `Ctrl+Z` on Linux/Windows; a literal
  `Ctrl+Z` authored by hand honors the physical Ctrl.
- **No world-mutation path:** assert a matched shortcut only produced an action
  handle; no ECS write occurred outside spec `23`.

## Dependencies

- `winit` (raw keyboard input) — Domain A permitted.
- `serde` + `serde_json` for `shortcuts.json` under `OPENENGINE_CONFIG_PATH`.
- `crates/editor` action vocabulary shared with menus/toolbars and spec `25`;
  console toggle targets spec `27`; gizmo/viewport actions target spec `09`.
- No new `contracts`/ABI surface; no Domain B changes.

## Next Steps

1. Implement `Modifiers`, `Chord`, parser/formatter, and the friendly-string
   grammar.
2. Implement `ShortcutManager` with `BTreeMap` matching + conflict detection.
3. Wire `winit` keyboard events into `ShortcutManager` and route by focus
   context in the spec `25` shell.
4. Implement the defaults table and the `OPENENGINE_CONFIG_PATH/shortcuts.json`
   load/overlay/save.
5. Implement the Settings → Shortcuts capture UI with conflict validation and
   reset.
6. Add the spec `29` registration seam so plugins can add actions + bindings.
