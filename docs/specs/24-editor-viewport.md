---
spec: "24-editor-viewport"
phase: "Phase 5: Editor"
status: "draft"
author: "OpenEngine AI"
created: "2026-09-03"
depends_on:
  - "04-render-pipeline"
  - "09-editor-gizmos"
  - "21-primitive-components"
  - "23-undo-redo"
---
# 24 - Editor Viewport

## Overview

The viewport is the interactive 3D (and orthographic) scene view of the editor:
the window through which an agent or human navigates, selects, and frames the
edit world before authoring changes with the gizmos (spec `09`) and inspector
(spec `07`). Where spec `09` covers the *transform handles drawn on top*, this
spec covers the *viewport itself* — camera navigation, grid, entity picking and
selection, multi-viewport layout, and view modes.

Like every other editor panel, the viewport is a **Domain-A ECS system** in
`crates/editor`. It is **purely a presentation + input + selection layer**: it
reads the edit world through a safe `WorldView` snapshot (spec `07`/`00`) and
its *only* writes are selection state plus, indirectly, structural edits that it
routes through the `UndoRedoManager` (spec `23`) as commands. The viewport never
mutates ECS storage directly and never touches the play world (spec `22`). It
produces a **composite render output**: the game render (spec `04`) for the
scene, with gizmo/selection overlays composited above it (spec `09`) and the egui
UI composited above everything.

All camera and picking math uses `glam` `f32` — permitted in Domain A for
presentation/camera — and is **never stored in ECS components** and never enters
the guest. Headless tests cover camera math, ray/AABB intersection, grid
generation, and selection with no GPU.

## Core Concepts

### The viewport as a presentation system

```rust
// crates/editor — Domain A
pub struct ViewportSystem {
    pub selection: SelectionModel,     // shared with specs 07/08
    pub active: ViewportHandle,        // which viewport has focus
    pub viewports: Vec<Viewport>,
}
```

The system runs each post-update frame: it consumes mouse/keyboard input in
egui's retained-UI rect, updates the focused viewport's camera, builds the
selection, and emits a composite render command list for the viewport's render
targets. It reads entity positions/bounds from the `WorldView`; it does not own a
copy of gameplay state.

### Camera navigation

An `EditorCamera` is pure Domain-A state (never a component). Navigation is
immediate-mode orbit-style, matching industry editor defaults:

- **Orbit** — `Alt` + left mouse drag around the current `focus_point`.
- **Pan** — middle mouse (or `Alt` + right) drag moves `focus_point` and the
  camera together in the camera's right/up plane.
- **Dolly / zoom** — mouse wheel moves the camera toward `focus_point`; zoom
  speed scales with distance so it feels constant on screen.
- **Focus selected** — `F` snaps `focus_point` and the camera to frame the
  selected entity (or the whole scene when nothing is selected), fitting its
  AABB into the viewport.
- **Speed** — `Ctrl` + wheel adjusts the base navigation/fly speed for large or
  small scenes.

```rust
/// f32 only: presentation camera math. Never serialized into components.
pub struct EditorCamera {
    pub position: glam::Vec3,
    pub rotation: glam::Quat,        // yaw/pitch orbit orientation
    pub fov_y: f32,                  // radians, perspective only
    pub near: f32,
    pub far: f32,
    pub speed: f32,                  // dolly/fly speed (Ctrl+wheel adjusts)
    pub focus_point: glam::Vec3,     // orbit pivot
    pub mode: ProjectionMode,        // Perspective | Orthographic (multi-view)
}

pub enum ProjectionMode { Perspective, Orthographic }
```

A helper converts the camera into view/projection matrices for the renderer:

```rust
impl EditorCamera {
    pub fn view_matrix(&self) -> glam::Mat4 { /* look_at(position, focus) */ }
    pub fn projection(&self, aspect: f32) -> glam::Mat4 {
        match self.mode {
            ProjectionMode::Perspective => glam::Mat4::perspective_rh(
                self.fov_y, aspect, self.near, self.far),
            ProjectionMode::Orthographic => glam::Mat4::orthographic_rh_gl(
                /* fit around focus_point at ~zoom */),
        }
    }
}
```

Input deltas resolve against screen-space drag distances; orbit/pan/dolly are
pure functions `fn orbit(&mut self, dx: f32, dy: f32)` so they unit-test
headlessly.

### Grid rendering

The viewport draws a ground-plane grid for spatial reference and snapping.

- **Configurable spacing**: 1 m / 5 m / 10 m (per-viewport setting; also drives
  gizmo snapping in spec `09`).
- **Major / minor lines**: a minor line per spacing unit and a stronger major
  line every N (e.g. 10 × spacing) units.
- **Axis colors**: X = red, Y = green, Z = blue. Y (up) has no ground line color
  role; the X/Z ground axes get their canonical red/blue tints, with the origin
  emphasized.
- **Infinite with distance fade**: instead of a finite grid mesh, lines are
  generated up to a far plane and fade by distance from the camera origin so the
  horizon never shows a hard edge.

```rust
pub struct GridSettings {
    pub spacing: f32,          // 1.0 | 5.0 | 10.0
    pub major_every: u32,      // lines between majors (e.g. 10)
    pub fade_near: f32,        // distance where fade starts
    pub fade_far: f32,         // distance where lines vanish
    pub enabled: bool,
}
```

Grid *vertex generation* is a pure function producing a bounded line list for a
given camera extent (how many minor/major lines are visible) — unit-testable
headlessly, independent of the wgpu upload.

### Entity picking

Picking selects an entity by casting a mouse ray into the scene and testing
against **axis-aligned bounding boxes (AABBs)**, not against triangle meshes —
ray vs. every mesh triangle is far too expensive at scene scale and is not
needed for editor selection. The AABB source is the **registered `Bounds`
component (spec `21`, ComponentId **71**)**, read from the edit world's
`WorldView`: picking is a ray-vs-`Bounds`-AABB test against each candidate
entity.

```rust
pub struct Ray { pub origin: glam::Vec3, pub dir: glam::Vec3 }

/// World-space AABB gathered from the selected entity's bounds components.
pub struct BoundsAabb { pub min: glam::Vec3, pub max: glam::Vec3 }

fn ray_from_viewport(viewport: &Viewport, mouse_ndc: glam::Vec2)
    -> Option<Ray>;
fn ray_intersects_aabb(ray: &Ray, aabb: &BoundsAabb) -> Option<f32>; // t
```

Selection interaction:

- **Hover highlight**: the entity under the cursor is highlighted with an
  outline shader (a post/overlay pass that draws a brightened silhouette / edge
  around the hovered AABB or mesh), giving immediate feedback.
- **Click select**: single click replaces the current selection.
- **Ctrl+click add**: adds the clicked entity to the selection set.
- **Shift toggle**: toggles the clicked entity's membership (or a range when a
  selection anchor exists, matching spec `08`).
- **Box (drag rectangle) select**: dragging an empty area in the viewport draws
  a screen rectangle and selects all entities whose projected AABB intersects it.

Picking is a pure function of the world-space AABBs + the camera, so it is fully
headless-testable. Entity bounds live in the edit world as registered `Bounds`
components (spec `21`, ComponentId 71); the viewport never raymarches GPU meshes.

### Multi-viewport

The editor supports several layouts (perspective is default):

- **Perspective** — default, uses the interactive `EditorCamera` orbit mode.
- **Top / Front / Side** — orthographic views along fixed axes
  (X=right, Y=up, Z=toward viewer for the standard OpenGL-ish mapping; the actual
  handedness follows spec `04`), driven by an orthographic `EditorCamera` locked
  to the chosen axis with pan/zoom still available.
- **2×2 split** — four synchronized viewports (Perspective, Top, Front, Side)
  sharing one `focus_point` so orbiting in the perspective view pans the others.

```rust
pub struct Viewport {
    pub id: ViewportHandle,
    pub camera: EditorCamera,
    pub projection: ProjectionMode,      // mirrors camera; kept explicit
    pub mode: ViewMode,
    pub grid: GridSettings,
    pub selected_entities: Vec<Entity>,  // selection view into this port
    pub hovered_entity: Option<Entity>,
    pub layout: ViewportLayout,          // derives from split configuration
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum ViewportLayout { Perspective, Top, Front, Side }
```

### View modes

Per-viewport rendering of the scene's geometry. The viewport uses the **one**
canonical enum `contracts::ViewMode { Wireframe | Solid | Textured | Lit }`
shared verbatim with specs 04 and 25 (`Lit` is the spec-04 default); no local
`ViewMode` is defined here:

```rust
// contracts::ViewMode — canonical across specs 04/24/25.
// Variants: Wireframe (edges only) | Solid (unlit) | Textured (base color/tex) |
//           Lit (full forward lighting, spec 04 default).
```

Each mode is a render *style* applied to the same scene command list; switching
re-picks shader/pipeline variants but does not change ECS state. Selection and
grid overlays render in every mode.

### Composite render output

The viewport composes three layers, in order:

1. **Game render** — the scene geometry via spec `04`'s command list, drawn with
   the viewport camera and the active `ViewMode`.
2. **Overlay** — selection/hover outlines and the gizmo frame (spec `09`),
   composited *above* the game render but *below* egui, depth-tested against the
   scene so occluded handles hide.
3. **egui UI** — the viewport chrome, gizmo toolbar, and any panels, on top.

This layering keeps gizmo/selection pickable against the world while remaining
under the editor UI. The composite output for each viewport is a set of render
targets the editor owns; in headless tests we assert on the command-list order,
not pixels.

## Key Rust Types

- `ViewportSystem`, `Viewport`, `ViewportHandle`, `ViewportLayout` —
  `crates/editor`.
- `EditorCamera`, `ProjectionMode` — `crates/editor` (Domain A camera math).
- `GridSettings`, `GridFrame` (bounded line list) — `crates/editor`.
- `ViewMode` — the canonical `contracts::ViewMode` (shared with specs 04/25).
- `Ray`, `BoundsAabb`, `ray_intersects_aabb`, `ray_from_viewport` —
  `crates/editor` geometry helpers.
- `SelectionModel` — shared with specs `07`/`08`/`23` (selection lives here,
  not in the viewport).
- Output: selection writes go through `SelectionModel`; structural edits from
  selection-driven UI (e.g. `Del` to despawn) go through `UndoRedoManager`
  commands (spec `23`) and flush as `WorldDelta` at the boundary.
- Reads: `WorldView<'a>` (safe immutable SoA read, spec `07`) for positions,
  the registered `Bounds` component (spec `21`, ComponentId 71), parent
  relations (spec `08`).

## Constraints

- **Domain A, host-only.** The viewport, all camera/picking/grid math, and the
  render output live in `crates/editor`/`crates/core`; none of it is compiled for
  the guest. `glam` `f32` is permitted here (presentation/camera) and **never
  stored in ECS components** or sent to Domain B.
- **Edit world only.** The viewport renders and selects the **edit world** (spec
  `22`). It never reads or writes the play world; play-world rendering is a
  separate concern.
- **No direct ECS mutation.** Picking only changes `SelectionModel`. Any
  structural consequence (despawn via `Del`, spawn) is a spec-`23` command flushed
  at the boundary. The viewport never calls `apply_delta` itself for authoring;
  its sole mutation channel is the command path (spec `23`), which itself is the
  same `WorldDelta`/`apply_delta` path.
- **Selection is not a component.** Selected/hovered state is editor UI state in
  `SelectionModel`, not a gameplay component — selection never leaks into the
  world or the guest.
- **Picking cost.** Ray vs AABB only, never triangle-level. Expensive per-mesh
  raycasting is explicitly out of scope for selection.
- **Fixed-point boundary.** Fixed-point world positions (spec `00`, `21`) are
  quantized to `glam::f32` only at the viewport's presentation boundary for
  camera/picking math; gameplay columns are never rewritten with `f32`.
- **Grid / overlays are debug channels.** They go through the Domain-A
  `RenderKind` overlay/debug path (spec `09`), never through the game render
  list Domain B produces.
- **Portability.** No hardcoded paths or OS-specific code; compiles on
  `x86_64-linux` and `aarch64-linux`. All viewport *logic* (camera math, picking,
  grid gen, layout bookkeeping) is headless-testable with no GPU/window; the
  wgpu submit path is feature-gated and smoke-tested only on a device.
- **Determinism of navigation.** Camera is pure editor state (not simulated), so
  its float math has no determinism obligation — but selection *results* and any
  structural edits derived from them must be reproducible given the same inputs
  (they flow through the deterministic command/delta path of spec `23`).

## Performance Targets

- Camera navigation (orbit/pan/dolly update): **< 0.1 ms** per input event.
- Picking: build ray + test ~N candidate AABBs. Target **< 1 ms** for up to
  10 000 candidate entities; a spatial BVH/index over `Bounds` (spec `21`,
  ComponentId 71) keeps it sub-ms as scenes grow.
- Grid vertex generation for the visible extent: **< 0.5 ms**, generated only
  when camera extent changes (cached otherwise).
- Composite overlay command-list build (selection outlines + gizmo frame, spec
  `09`): **< 0.5 ms**; only the selected entities draw overlay geometry.
- Whole viewport frame overhead over the game render: bounded, **< 2 ms** in
  editor builds with a typical scene.

## Testing Strategy

All headless (no GPU / no window) in `crates/editor` / `crates/core` unit tests:

- **Camera math.** Orbit keeps `focus_point` fixed and produces the expected
  rotation; pan moves camera + focus together; dolly changes distance toward
  focus; zoom-to-fit computes the distance that frames a given AABB. Assert
  against known-good matrices (golden tests).
- **Orthographic views.** Top/Front/Side project a known point to the expected
  screen coordinate; the locked-axis camera only pans/zooms, never orbits off
  axis.
- **Ray/AABB intersection.** Brute tests against axis-aligned boxes from inside,
  outside, grazing edges, and the no-hit miss case; assert exact `t` values.
- **Grid vertex generation.** For a given camera extent and spacing, assert the
  produced line list has the expected major/minor counts and correct axis colors
  (X red, Y green, Z blue), and that lines fade/are culled beyond `fade_far`.
- **Picking & selection.** Build a scene of known AABBs, cast synthetic rays, and
  assert click/Ctrl+add/Shift-toggle/box-select update `SelectionModel` exactly;
  hover sets `hovered_entity`. Assert depth-preferred (nearest `t`) selection.
- **Multi-viewport.** Assert that a perspective orbit updates the shared
  `focus_point` seen by the synchronized Top/Front/Side ortho ports; assert
  per-viewport `ViewMode`/grid settings apply independently.
- **Integration: navigate + pick → selection.** Drive a scripted sequence
  (orbit to a viewpoint, click a target entity), assert the selection becomes that
  entity; then assert `Del` routes a `DespawnEntityCommand` into the
  `UndoRedoManager` (spec `23`) and its delta, applied at flush, removes the
  entity — proving the viewport→selection→command→delta path end to end with no
  GPU.
- **Determinism.** Run the navigate+pick script 3× and assert identical
  selections and identical resulting edit-world state (edits are spec-`23`
  commands).
- **No GPU in CI.** Viewport logic tests never construct a `wgpu::Device`; the
  submit path is gated behind a feature and smoke-tested separately/manually.

## Dependencies

- `crates/editor` (Domain A) — viewport system, camera, grid, picking,
  selection; egui for the panel/chrome.
- `crates/core` (Domain A) — composite render targets, wgpu submit path.
- Render pipeline from spec `04` (game render + view modes); gizmo overlay and
  debug-channel compositing from spec `09`; the registered `Bounds` component
  (spec `21`, ComponentId 71) as the ray-vs-AABB picking source.
- `SelectionModel`/`WorldView`/`EditorError` shared with specs `07`/`08`.
- Undo/redo path from spec `23` for all structural edits the viewport initiates.
- Edit-vs-play split from spec `22`.
- `glam` (camera/ray math), `bytemuck`, `contracts` (`Entity`, `ComponentId`,
  `ArchetypeId`, `RenderKind`, `WorldDelta`, `ColumnWrite`),
  `openengine-math` (quantize fixed→f32 at the presentation boundary).

## Next Steps

1. `EditorCamera` + projection/view helpers and headless camera-math tests.
2. Navigation input mapping (orbit/pan/dolly/focus/speed) wired to the active
   viewport.
3. `GridSettings` + pure grid vertex generation and the overlay draw path.
4. Ray/AABB picking + hover/click/Ctrl/Shift/box selection into the shared
   `SelectionModel`.
5. Multi-viewport layouts (Perspective + Top/Front/Side, 2×2) and per-port
   `ViewMode`.
6. Composite render ordering (game render → gizmo/selection overlay → egui) and
   the headless navigate+pick integration test wired to spec-`23` commands.
