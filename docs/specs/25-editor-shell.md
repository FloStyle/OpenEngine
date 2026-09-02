---
spec: "25-editor-shell"
phase: "Phase 5: Editor"
status: "draft"
author: "OpenEngine AI"
created: "2026-09-03"
depends_on: ["24-editor-viewport", "07-editor-inspector", "08-editor-hierarchy"]
---
# 25 - Editor Shell

## Overview

The editor shell is the **window that holds every editor panel**: the top
toolbar, the dockable panel area in the middle, and the status bar at the
bottom. Like the inspector (spec `07`) and hierarchy (spec `08`), the shell is
**not a separate application** — it is a Domain-A `egui` system inside the
`openengine-editor` crate, running on the host side of the game loop. It owns no
gameplay state; it observes the current scene through the same safe `WorldView`
read used by specs `07`/`08`, and any world change it causes is emitted as a
`WorldDelta`/`ColumnWrite` at the flush boundary (spec `01`/`06`).

What distinguishes the shell from an ordinary system is **purely presentation
and arrangement**: which panels exist, how they are docked/floated/tabbed, what
the toolbar and status bar report, and which saved *layout* is active. None of
that is simulation state, so a layout is trivially serializable (plain JSON) and
there is no "shell" inside the pure logic sandbox.

This spec covers the docking/tabbing machinery, the toolbar + status bar, the
panel registry, and layout save/load. The *content* of each panel is delegated
to its own spec; the shell only instantiates panels from its registry and hands
each a `&mut PanelHandle` it can draw into.

## Core Concepts

### Panels are ECS systems, arranged by the shell

Each panel is a Domain-A system `fn run(&mut self, ui: &mut egui::Ui,
view: &WorldView<'_>, draft: &mut ...)`. The shell does **not** decide what a
panel does — it decides *where* the panel's `egui::Ui` region is on screen, then
calls the panel system with that region. Conceptually:

```rust
// crates/editor — Domain A (skeleton of the per-frame dispatch)
pub struct EditorShell { /* panels + dock_state, see below */ }

impl EditorShell {
    /// Run once per (post-update) frame. Draws the shell chrome, then lets each
    /// docked panel system draw itself into its own `egui::Ui` region.
    pub fn run(
        &mut self,
        ctx: &egui::Context,
        view: &WorldView<'_>,
        frame_stats: &FrameStats,          // for the status bar
        out: &mut ShellOutput,             // staged toolbar commands (play/undo/...)
    );
}
```

The shell reads the scene *observationally* (`view`) to fill the status bar
(entity count, selection count) and to enable/disable toolbar actions. It never
writes into ECS storage; toolbar and dock actions that mutate the world are
queued into `ShellOutput` and flushed to `WorldDelta` at the normal boundary.

### Docking model

The shell hosts a docking area in the central region. A dock layout is a
**tree of regions**:

- **Docked** panels tile a region and share its space; edges can be dragged to
  resize siblings.
- **Tabbed** panels are stacked in the *same* region under a tab strip; clicking
  a tab brings one panel forward.
- **Floated** panels are detached into their own movable, resizable window
  (`egui::Window`-like) hovering over the docking area.
- Dragging a panel **titlebar** toward another region's edge, center, or tab
  strip shows a snap-zone highlight; releasing docks it there (split / center /
  tab).

We implement docking in-house as a plain tree (below) rather than depending on
`egui_dock`, so the layout is our own serde type and we control snapping. A thin
adapter may wrap `egui_dock` later if it proves stable, but the public snapshot
type (`LayoutSnapshot`) must stay the same so layouts remain portable.

The layout tree:

```rust
/// One node in the dock tree. A region is either a split with two children or a
/// stack of tabbed panels (with one visible).
pub enum DockNode {
    /// Horizontal/vertical split of two child regions with a draggable edge.
    Split { dir: SplitDir, ratio: f32, a: Box<DockNode>, b: Box<DockNode> },
    /// A stack of panels occupying the same region; `active` selects the tab.
    Tab { panels: Vec<u32>, active: usize },
    /// Empty region placeholder (e.g. nothing yet dropped here).
    Empty,
}

#[derive(Clone, Copy, PartialEq)]
pub enum SplitDir { Horizontal, Vertical }
```

Every panel id (`u32`) referenced by a node is resolved through the **panel
registry** to a concrete panel instance.

### `DockState`

`DockState` is the working, frame-to-frame arrangement plus per-panel geometry.
It wraps the root `DockNode` tree and the set of floaters. Floaters are kept out
of the tree so a floated panel is still *one* registered panel with a stable id.

```rust
pub struct DockState {
    pub root: DockNode,
    pub floaters: Vec<FloatingPanel>,      // detached windows, keyed by panel id
    pub titlebar_drag: Option<DragInFlight>, // which titlebar is being dragged
    pub resize_edge: Option<ResizeInFlight>, // which edge is being dragged
    pub snap_zone: Option<SnapZone>,         // highlighted target during drag
}

pub struct FloatingPanel {
    pub panel_id: u32,
    pub rect: egui::Rect,     // screen-space window rect
    pub titlebar: egui::Rect, // where the user grabbed to drag/redock
}

pub struct SnapZone { pub node_path: Vec<u32>, pub kind: SnapKind }
pub enum SnapKind { Left, Right, Top, Bottom, Center, TabStrip }
```

`DockState` is *transient* (holds `egui::Rect`s and drag state) and is **not**
what gets serialized. Only `LayoutSnapshot` (see **Key Rust Types**) is
serialized — it holds the logical tree and panel identity, no `Rect`s.

### Panel registry

A registry maps a stable `panel_id: u32` to a `Panel` descriptor and a live
drawable. Panels are instantiated once (not once per layout); a layout only
changes *where* existing panels sit or whether they are visible. This keeps
per-panel transient UI state (scroll positions, collapsed sections) alive when
switching layouts.

```rust
pub struct PanelRegistry {
    panels: BTreeMap<u32, Panel>,   // stable order => deterministic iteration
    next_id: u32,
    kinds: BTreeMap<PanelContent, Box<dyn DrawPanel>>, // one impl per content kind
}

pub struct Panel {
    pub id: u32,
    pub title: String,
    pub content: PanelContent,
    pub dock_state: DockState,  // the arrangement slot this panel occupies
    pub visible: bool,
    pub min_size: egui::Vec2,
}
```

`Panel::dock_state` records *where this panel lives* and its drag/float state;
the authoritative tree lives in `EditorShell::layout.dock_state` and keeps the
per-panel field in sync at the end of each frame (see sync rule below).

### Panel content kinds

```rust
pub enum PanelContent {
    Viewport,      // spec 24 — 3D scene view + gizmos
    Hierarchy,     // spec 08 — entity tree
    Inspector,     // spec 07 — selected-entity component editor
    AssetBrowser,  // spec 26 — project asset tree/thumbnails
    Console,       // spec 11 — logs / diagnostics
    Custom(String),// user or tool panel registered at runtime by id
}
```

The shell dispatches a docked panel to its draw function based on `content`:
Viewport → spec `24` system, Hierarchy → spec `08`, Inspector → spec `07`,
AssetBrowser → spec `26`, Console → the console log sink. `Custom(String)`
resolves through the registry's extension point so tools can register panels
without changing this enum's `#[repr]`-free host types.

### Toolbar

A horizontal strip above the docking area. Buttons are **commands**, not state
mutations:

```rust
pub struct Toolbar {
    pub mode: EditorMode,               // spec 22: Edit | Playing | Paused
    pub transform: TransformMode,       // Translate | Rotate | Scale
    pub snap_enabled: bool,
    pub snap_step: I16F16,              // world-space snap grid (fixed-point)
    pub view_mode: ViewMode,            // canonical contracts::ViewMode
}

// The shell's mode is spec 22's ONE editor-mode enum: EditorMode { Edit |
// Playing | Paused }. Paused (play world frozen, read-only / stepping) is a
// first-class shell state — not collapsed into Edit/Play.
pub enum TransformMode { Translate, Rotate, Scale }
// ViewMode is the canonical contracts::ViewMode { Wireframe | Solid |
// Textured | Lit }, shared verbatim with specs 04 and 24.
```

Toolbar buttons (all with tooltips):

- **Play / Pause / Stop** transition spec-`22` `EditorMode`
  (`Edit→Playing`, `Playing→Paused`, and `Playing`/`Paused→Edit`). Pause freezes
  the play world for read-only inspection/stepping; Stop exits to Edit. The edit
  world is never mutated while Playing/Paused (spec `22`), so Stop restores it
  unchanged and adds no undo entries.
- **Undo / Redo** (spec `23`) — enabled only when the undo stack is non-empty;
  clicking stages an `UndoCommand`/`RedoCommand`.
- **Transform mode** — Translate / Rotate / Scale, shown as W / E / R badges
  (keyboard shortcuts, see **Global shortcuts**). Writes `Toolbar::transform`,
  which spec `24`/`09` gizmos read to choose their handle.
- **Snap toggle** — enables a world-space snap grid on gizmo drags.
- **View-mode selector** — a dropdown for `ViewMode` consumed by the Viewport
  renderer.

Keyboard shortcuts are registered once per frame against the shell's input:
`W`/`E`/`R` = transform mode; `S` = snap toggle; `Space` = play/pause; `Ctrl+Z`/
`Ctrl+Shift+Z` = undo/redo. Shortcuts only fire when the pointer is over the
docking/toolbar area, **not** while typing in a text field (an inspector input,
a rename box) — egui's `Memory` focus state decides this.

### Status bar

A thin strip at the bottom, updated once per frame from the `WorldView` and the
frame timer. It never polls the ECS itself; the shell reads the already-built
`WorldView` snapshot plus a `FrameStats` struct handed in by the loop.

```rust
pub struct StatusBar {
    pub fps: f32,                // moving average, display-only
    pub entity_count: usize,     // == view.live_entity_count()
    pub selection_count: usize,  // == SelectionModel.current selection size
    pub mode: EditorMode,        // spec 22: Edit | Playing | Paused
    pub memory: usize,           // resident set or arena bytes, best-effort
}
```

The four pieces shown left-to-right: **mode** (Edit/Playing/Paused, spec 22), **selection count**,
**entity count**, then a right-aligned cluster **FPS + memory**. Selection count
comes from the shared `SelectionModel` (spec `07`/`08`); entity count from a
single linear count over the `WorldView` (O(1) if the ECS keeps a live counter,
spec `00`).

### Layouts

A **layout** is a named, serializable arrangement of the docking tree plus the
panel set and each panel's title/visibility. Layouts do **not** store tool
settings (transform mode, snap value) — only *where things are*. They are saved
as JSON under `OPENENGINE_CONFIG_PATH/layouts/<name>.json`, resolved relative to
config, never absolute/hardcoded (AGENTS.md § 5; `$HOME`-free).

```rust
/// The only thing persisted when a layout is saved. Pure data, no egui::Rect.
#[derive(serde::Serialize, serde::Deserialize)]
pub struct LayoutSnapshot {
    pub name: String,
    pub root: LayoutNode,             // tree mirroring DockNode minus floats
    pub floaters: Vec<LayoutFloater>, // panel_id + normalized rect
    pub panels: Vec<PanelSnapshot>,   // title, content, visible, min_size
}

pub struct LayoutNode {
    pub split: Option<(SplitDir, f32, Box<LayoutNode>, Box<LayoutNode>)>,
    pub tabs: Option<Vec<(u32, usize)>>, // (panel_id, active_tab_index)
}

pub struct PanelSnapshot { pub id: u32, pub title: String,
                           pub content: PanelContent, pub visible: bool }
pub struct LayoutFloater { pub panel_id: u32, pub norm_rect: (egui::Pos2, egui::Vec2) }
```

`PanelContent` and `SplitDir` derive serde, so the whole snapshot round-trips
through `serde_json`. Because a snapshot has no `Rect`s, saving/loading never
depends on window size at save time; floaters use *normalized* rects (0..1 of the
docking area) so they re-fit on resize.

**Default layouts** ship with the editor and are copied into
`OPENENGINE_CONFIG_PATH/layouts/` on first run only if absent (never overwrite a
user's file):

- **Modeling** — Viewport center, Hierarchy left, Inspector right, Asset Browser
  bottom, Console bottom-right tabbed with Asset Browser.
- **Animation** — a side **Animation/Timeline** area open (a `Custom("Timeline")`
  panel), Viewport center, Inspector right.
- **Coding** — Console raised (tall bottom), Inspector + a `Custom("Script")`
  panel on the right; Hierarchy reduced.

The shell exposes **Save Layout**, **Load Layout** (a file of `.json` under the
layouts dir) and **Reset to Defaults** (restore the current default layout or a
named built-in). Reset only touches shell arrangement — it issues no world
mutation.

```rust
pub struct EditorShell {
    pub panels: PanelRegistry,
    pub toolbar: Toolbar,
    pub status_bar: StatusBar,
    pub current_layout: String,     // name of the active layout ("Modeling", ...)
    pub layout: DockState,          // the live arrangement tree
}
```

`current_layout` names the built-in or user layout that produced the current
`layout`. Saving the current arrangement writes a snapshot under that name;
switching layout replaces `layout` and re-fits panels by id.

### ShellOutput (staged toolbar commands)

Every action the shell/toolbar emits that touches the world is staged, mirroring
spec `07`'s `InspectorDraft` and spec `08`'s `HierarchyCommands`:

```rust
pub enum ShellCommand {
    Play, Pause, Stop,
    Undo, Redo,                       // spec 23
    SetTransform(TransformMode),      // consumed by viewport gizmos
    SetSnap(bool, I16F16),            // consumed by gizmos (fixed-point snap)
    SetViewMode(ViewMode),            // consumed by the viewport renderer
    SaveLayout, LoadLayout(String), ResetLayout,
}
pub struct ShellOutput { pub commands: Vec<ShellCommand> }
```

Play/Pause/Stop and Undo/Redo are the only commands that actually change world
flow; the rest are host-side tool settings routed to panels. Commands are flushed
at the frame boundary like any other delta — the shell never applies a command
from inside an egui paint closure.

## Key Rust Types

```rust
// crates/editor — Domain A only
pub struct EditorShell { pub panels: PanelRegistry, pub toolbar: Toolbar,
    pub status_bar: StatusBar, pub current_layout: String, pub layout: DockState }
pub struct PanelRegistry { panels: BTreeMap<u32, Panel>, next_id: u32,
    kinds: BTreeMap<PanelContent, Box<dyn DrawPanel>> }
pub struct Panel { pub id: u32, pub title: String, pub content: PanelContent,
    pub dock_state: DockState, pub visible: bool, pub min_size: egui::Vec2 }
pub struct DockState { pub root: DockNode, pub floaters: Vec<FloatingPanel>,
    pub titlebar_drag: Option<DragInFlight>, pub resize_edge: Option<ResizeInFlight>,
    pub snap_zone: Option<SnapZone> }
pub struct FloatingPanel { pub panel_id: u32, pub rect: egui::Rect, pub titlebar: egui::Rect }
pub struct SnapZone { pub node_path: Vec<u32>, pub kind: SnapKind }
pub struct LayoutSnapshot { pub name: String, pub root: LayoutNode,
    pub floaters: Vec<LayoutFloater>, pub panels: Vec<PanelSnapshot> }
pub enum DockNode { Split { dir: SplitDir, ratio: f32, a: Box<DockNode>,
    b: Box<DockNode> }, Tab { panels: Vec<u32>, active: usize }, Empty }
pub enum PanelContent { Viewport, Hierarchy, Inspector, AssetBrowser,
    Console, Custom(String) }
pub enum SplitDir { Horizontal, Vertical }
pub enum SnapKind { Left, Right, Top, Bottom, Center, TabStrip }
pub enum EditorMode { Edit, Playing, Paused }   // spec 22 — shared editor mode
pub enum TransformMode { Translate, Rotate, Scale }
// ViewMode is canonical contracts::ViewMode { Wireframe | Solid | Textured | Lit }
pub struct Toolbar { pub mode: EditorMode, pub transform: TransformMode,
    pub snap_enabled: bool, pub snap_step: I16F16, pub view_mode: ViewMode }
pub struct StatusBar { pub fps: f32, pub entity_count: usize,
    pub selection_count: usize, pub mode: EditorMode, pub memory: usize }
pub enum ShellCommand { Play, Pause, Stop, Undo, Redo,
    SetTransform(TransformMode), SetSnap(bool, I16F16), SetViewMode(ViewMode),
    SaveLayout, LoadLayout(String), ResetLayout }
```

Supporting host types used by the shell: `WorldView<'a>` (safe SoA read shared
with specs `07`/`08`), `SelectionModel` (shared selection), `FrameStats`,
`EditorEditError` (parallels `RecoverableError`), and `openengine-math::I16F16`
for the fixed-point snap grid.

## Constraints

- **Domain A only.** The shell, docking tree, toolbar and status bar live in
  `crates/editor` and are never compiled into Domain B. There is no "shell" in
  the guest.
- **The shell holds no gameplay state.** It observes the scene through a safe
  `WorldView` snapshot and emits world-affecting actions only as staged
  `ShellCommand`s flushed to `WorldDelta` at the boundary (spec `01`/`06`). It
  never mutates ECS storage mid-iteration (same busy guard as specs `07`/`08`).
- **Display-only `f32`.** FPS and status text may be `f32` (host/display
  boundary). World-space snap values are fixed-point `I16F16`; never a raw `f32`
  snap grid.
- **Portable paths.** Layout files live under
  `OPENENGINE_CONFIG_PATH/layouts/` relative to config — no absolute, hardcoded,
  or `$HOME` paths. Defaults are copied only when absent (never overwrite).
- **Deterministic ordering.** Panels are stored in `BTreeMap` keyed by `u32`
  (never a `HashMap`), so layout save, iteration and selection stay deterministic
  across runs.
- **No absolute window sizes in a saved layout.** Floaters persist *normalized*
  rects so a layout reloads correctly at any window size.
- **Compiles on `x86_64-linux` and `aarch64-linux`.** Shell *logic* (dock-tree
  ops, layout serde, snap math) is tested headlessly with no GPU and no window.

## Performance Targets

- Panel/EGUI overhead: the shell's own chrome (toolbar + status bar + dock
  chrome) stays < 1 ms/frame; per-panel content cost is owned by each panel spec.
- Dock hit-testing, edge-resize and snap-zone computation over the dock tree:
  < 50 µs per interaction.
- Layout save/load (serialize/deserialize a few dozen panels): < 5 ms.
- Status bar updates are O(1) reads of already-computed `FrameStats` +
  `WorldView`; no per-frame world rescan for entity count (reuse the ECS live
  counter from spec `00`).
- Responsive down to modest windows: default dock splits honor each panel's
  `min_size`; on a window smaller than the sum of `min_size`s the tabs collapse
  into overflow rather than jamming layout.

## Testing Strategy

- **Layout round-trip (headless):** build an `EditorShell`, arrange a
  docking/split/tab/float mix, `serde_json::to_string` a `LayoutSnapshot`, load
  it back, and assert the resulting tree + panel set are identical (no `Rect`s
  in the snapshot ⇒ byte-stable JSON for a fixed arrangement).
- **Default-layout first-run:** with an empty `OPENENGINE_CONFIG_PATH/layouts`,
  assert Modeling/Animation/Coding are materialized; assert a user file present
  is *not* overwritten.
- **Snap-zone math (headless):** for a set of target rects, assert that dragging
  a titlebar near an edge/center/tab-strip yields the correct `SnapZone` (edge
  wins near a corner; center wins on the interior; TabStrip only over an existing
  tab strip).
- **Deterministic iteration:** iterate `PanelRegistry` and save a layout three
  times; assert identical JSON (BTreeMap ordering).
- **Command staging:** toolbar actions produce `ShellCommand`s in `ShellOutput`,
  flushed at the boundary; assert no `ShellCommand` is applied inside an egui
  paint closure and the busy guard fires if a flush is attempted mid-iteration.
- **Integration:** drive the shell with a synthetic `WorldView` of N entities;
  assert status-bar entity/selection counts match the view and that switching
  Edit↔Playing (incl. the Paused state, spec `22`) only toggles the toolbar state
  (no world writes).

## Dependencies

- `crates/editor` (Domain A): `egui`, plus the crate's own `DockState`/dock tree.
- `openengine-ecs` (`WorldView`, live entity counter) and `openengine-contracts`
  (`Entity`, `ArchetypeId`, `WorldDelta`, `ColumnWrite`).
- `openengine-math::I16F16` for the fixed-point snap grid.
- `serde` + `serde_json` for `LayoutSnapshot` persistence.
- Panel content delegated to: spec `24-editor-viewport` (Viewport), spec
  `07-editor-inspector` (Inspector), spec `08-editor-hierarchy` (Hierarchy),
  spec `26-asset-browser-ui` (Asset Browser), spec `11-debugging-tools`
  (Console). Undo/Redo hook into spec `23-undo-redo`.
- `OPENENGINE_CONFIG_PATH` env (AGENTS.md § 5) for layout storage.

## Next Steps

1. Implement `DockNode`/`DockState` and the dock-tree editing operations (split,
   tab, float, snap-zone hit-testing, edge resize).
2. Implement `PanelRegistry` (BTreeMap-backed) and a `DrawPanel` dispatch keyed
   by `PanelContent`.
3. Implement `LayoutSnapshot` + save/load/reset against
   `OPENENGINE_CONFIG_PATH/layouts` and ship the three default layouts.
4. Implement `Toolbar` + global shortcut routing (respecting text-input focus)
   and `ShellCommand` staging/flush.
5. Implement `StatusBar` reads from `FrameStats` + `WorldView`.
6. Instantiate spec `24`/`07`/`08`/`26` panel systems into the shell and add the
   Console default; wire Undo/Redo to spec `23`.
