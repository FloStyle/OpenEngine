---
spec: "47-ui-system"
phase: "Phase 5: Cinematic & UI"
status: "draft"
author: "OpenEngine AI"
created: "2026-09-03"
depends_on:
  - "00-ecs-architecture"
  - "03-input-system"
  - "04-render-pipeline"
  - "05-time-system"
  - "16-serialization"
  - "21-primitive-components"
  - "22-edit-vs-play"
  - "23-undo-redo"
  - "24-editor-viewport"
---
# 47 - Game UI / HUD System

## Overview

OpenEngine's game UI/HUD (an Unreal-UMG-like retained component system) is
described **entirely as ECS data** so that every screen, HUD, and widget is
deterministic, serializable, and editable with the same tooling as any scene.
A HUD is a **component tree** rooted at a `UICanvas` (53), built from
`UIElement` (54) nodes and the text/button primitives `UIText` (55) /
`UIButton` (56). Layout, appearance, and interaction state are columns on those
components — never code-in-`egui` singletons, never host-side retained state
that leaks across runs.

The architectural split mirrors the engine's domain boundary (AGENTS.md § 1):

- **Authoring / data (both domains).** UI components are `Pod` `#[repr(C)]`
  data (spec `00`/`21`). Their *layout* is deterministic fixed-point math.
- **Presentation (Domain A only).** The batched quad + SDF-font renderer and the
  OS event plumbing live in `crates/core`/`crates/editor`. `f32`, `wgpu`,
  `winit`, and the raster/SDF pipeline never leave Domain A.
- **Logic (Domain B, pure).** A pure `ui_logic_system` can **set element state**
  (visible, enabled, tint, value of a slider/progress) in response to gameplay,
  because element state is a component column and Domain B writes it via a
  `WorldDelta` just like any gameplay component. Domain B never touches layout
  pixels or OS input directly.

Input flows the spec-03 way: raw mouse/kb events → per-frame `InputSnapshot`
(Domain A, sorted, fixed-point) → a pure hit-test + focus system computes which
`UIElement`/`UIButton` is hovered / clicked / focused for this tick → the UI
system emits **UI events (OnClick / OnHover / OnFocus)** as
`DeferredCommand`s into the `WorldDelta`, which Domain A's renderer consumes to
drive the visual frame and, when appropriate, route gameplay effects through the
normal command path.

Rendering is a batched, mostly-static frame: the editor/widget system rasterizes
each canvas once into a retained quad+SDF command list (Domain A) and only
re-rasterizes the dirty subtree when an element state or layout changes. This
keeps the UI off the per-tick hot path while staying a pure function of the
component tree.

## Core Concepts

### The UI is a component tree: `UICanvas` → `UIElement` (+ primitives)

A canvas is a screen-aligned root; every child is a `UIElement`. Layering and
parenting reuse `Parent` (ComponentId 4) — UI entities live in the normal ECS
archetype tables, so a whole HUD is spawned/serialized/undo'd like any scene
tree (spec `00`/`08`/`16`).

```text
UICanvas "hud"              (ComponentId 53: root, resolution, z-sort)
├── UIButton "Start"        (54 + 56: rect + hit-test + state)
├── UIText   "health: 100"  (54 + 55: rect + glyph string + font)
├── UIElement "HealthBar"   (54 + element-variant Image/Slider/Progress)
│     ├── UIElement  bg     (54, Image variant)
│     └── UIElement  fill   (54, Progress variant)
└── UIButton "Menu"
```

`UIElement` (54) is the universal layout+visual node; `UIText` (55) and
`UIButton` (56) are specialized `UIElement`s that *add* a text/string or
hit-test/state facet. Image/Slider/ProgressBar are **element variants of
`UIElement`** discriminated by a `kind` byte (they reuse the same layout rect,
anchors, margins and do not add new registered component IDs in this pass;
extra registered variants would take the reserved 57–59 band).

> Why components and not host widgets: the Prime Directive. A HUD authored as
> ECS components is pure, ordered, deterministic data — the same world can be
> snapshot-cloned into the play world (spec `22`), replayed bit-identically,
> serialized (spec `16`), and previewed in the editor (spec `24`) with zero
> host-only state. Game logic can *drive* the HUD by writing component state;
> there is no engine singleton the logic must poke through a side channel.

### UI components (ComponentId band 53–56)

New UI components occupy the reserved **50–59** band; this spec takes 53–56 and
leaves 57–59 for future registered element variants. Sequencer components (spec
`46`) hold 50–52. **No UI spec re-registers `Transform`/`Camera` (2/7).**

| `ComponentId` | Name         | role                                             |
|---------------|--------------|--------------------------------------------------|
| 53            | `UICanvas`   | screen root: resolution, scale mode, sort order  |
| 54            | `UIElement`  | universal node: layout, anchors, variants (Image/Slider/Progress) |
| 55            | `UIText`     | text facet: fixed string, font ref, color        |
| 56            | `UIButton`   | interactive facet: hit rect, state, label, emit flags |

### Layout is deterministic fixed-point over anchors/margins/flex

Layout is pure Domain-B/Domain-A-shared fixed-point math. Every `UIElement`
carries an **anchor rect** (0..=1 normalized positions within its parent's
content box), **margins/offsets** in canvas-scaled units, and optional **flex**
(weights) for distributing sibling space. Resolution from the `UICanvas` is the
only external input. Because layout is a pure function of the component tree +
canvas resolution, the same tree lays out identically at the same resolution on
every run and every target — the HUD has no float-drift and no wall-clock
dependence.

Layout computation is expressed in `openengine-math::I16F16` and produces, per
node, a screen-space **layout rect** that the renderer consumes. A canvas may be
authored in a design resolution and **scaled** (fit / stretch / uniform) to the
live window — scaling is Domain-A presentation that never changes the authored
anchors.

### Element state is a column; Domain B sets it via a delta

Visibility, enabled, tint, current slider/progress value, and pressed/hover
appearance are all bytes/I16F16 on `UIElement`/`UIButton`. A pure Domain-B
`ui_logic_system` — e.g. "when health ≤ 20, flash the health text red" — reads
gameplay columns and returns a `WorldDelta` of `ColumnWrite`s against the UI
columns. This is the *only* way game logic touches the HUD: by writing element
state as data. Domain B never calls a draw call or an egui API.

### Input: snapshot → hit-test → DeferredCommand events

Raw OS events are Domain-A-only (spec `03`). The per-frame, sorted, fixed-point
`InputSnapshot` (mouse position in canvas pixels, plus key/button action
states) is fed to a **pure hit-test system** that, for this tick:

1. Walks active, visible, enabled canvases in z-order.
2. For each canvas, walks its `UIButton`/hit-testable `UIElement`s back-to-front,
   testing the pointer's fixed-point position against each element's computed
   layout rect (resolved from the last deterministic layout pass).
3. Determines hovered / focused elements and click edges (spec `03` edge
   semantics: `just_pressed` fires one tick, auto-repeat never re-fires).

Hit-test *results* then become **UI events surfaced as `DeferredCommand`s** in
the returned `WorldDelta`:

```rust
// ABI intent: emitted into WorldDelta.deferred for the host renderer/logic.
// (Concrete variant lands in contracts as an Emit topic + typed payload; the
//  UI renderer consumes it this frame, spec 03-style.)
DeferredCommand::Emit { topic: TOPIC_UI_CLICK, data: postcard(elem) } // OnClick
// OnHover / OnFocus use sibling topics. The host maps topic → renderer action.
```

The `DeferredCommand` is the *one* signal: the renderer uses it to redraw the
widget's pressed/hover visual state and, if the element is bound to gameplay
(a "Resume" button), the editor/host routes the action through the spec-23
command path (edit world) or a Domain-B `Emit` callback (play world). Logic for
what a button *does* is expressed as element-bound intent data (e.g. a topic the
system emits), not as a Rust closure stored on the host — keeping the HUD
deterministic and replayable.

### Batched quad + SDF rendering (Domain A)

The UI renderer in Domain A converts the deterministic layout into a **batched
GPU command list**: every element becomes a textured/batched quad and every
`UIText` a set of **SDF glyph quads** for its font. Batching groups quads by
texture/pipeline so the whole HUD draws in a handful of draw calls. The renderer:

- owns font atlases and their signed-distance-field textures (loaded via spec
  `02` asset registry, no absolute paths),
- builds a retained command list per canvas and **invalidates only the dirty
  subtree** on a state/layout change,
- quantizes fixed-point layout rects to `f32` only at the emission boundary
  (spec `04` / AGENTS.md § 3).

Rendering is pure presentation: it reads the component tree and never writes
gameplay state. Headless logic tests never construct a `wgpu::Device`.

### Editor authoring is edit-world + spec-23 commands

In the editor, creating/moving/retargeting UI nodes is normal ECS authoring on
the **edit world** through spec-23 `Command`s
(`AddUiElementCommand`, `SetAnchorCommand`, `SetTextCommand`, `RetargetCommand`),
each producing/inverting a `WorldDelta`. Preview runs the HUD on the **play
world** (spec `22`). UI layout preview and interactive testing therefore use the
same component data the shipping game uses.

## Key Rust Types

```rust
//! crates/ecs/src/components.rs (layouts) + crates/logic-sandbox/src/ui/
//! Domain B: pure, no_std, fixed-point. Renderer lives in Domain A.
#![forbid(unsafe_code)]

use contracts::{ComponentId, Entity, WorldDelta};
use openengine_math::I16F16;

/// Registry bindings for the UI band (frozen, spec 21 registry).
pub const C_UI_CANVAS:  ComponentId = ComponentId(53);
pub const C_UI_ELEMENT: ComponentId = ComponentId(54);
pub const C_UI_TEXT:    ComponentId = ComponentId(55);
pub const C_UI_BUTTON:  ComponentId = ComponentId(56);

/// Screen root. Canvas resolution is the layout's only external input.
#[repr(C)]
#[derive(Clone, Copy, PartialEq, Eq, bytemuck::Pod, bytemuck::Zeroable,
         serde::Serialize, serde::Deserialize, Debug)]
pub struct UICanvas {
    pub design_width:  I16F16,   // authored design resolution width (units)
    pub design_height: I16F16,
    pub scale_mode: ScaleMode,   // Fit / Stretch / Uniform (1 byte)
    pub sort_order: I16F16,      // whole-canvas z-order across multiple canvases
    pub _pad: [u8; 2],
}

/// Universal layout/visual node. Image/Slider/Progress are `kind` variants that
/// reuse this same rect/anchors/margins (no extra registered ID in this pass).
#[repr(C)]
#[derive(Clone, Copy, PartialEq, Eq, bytemuck::Pod, bytemuck::Zeroable,
         serde::Serialize, serde::Deserialize, Debug)]
pub struct UIElement {
    pub canvas: Entity,          // owning UICanvas root
    pub kind: ElementKind,       // Panel / Image / Slider / Progress (1 byte)
    pub anchor_min: [I16F16; 2], // 0..=1 within parent content box (normalized)
    pub anchor_max: [I16F16; 2],
    pub offset_min: [I16F16; 2], // px/unit margins from anchor_min
    pub offset_max: [I16F16; 2], // px/unit margins from anchor_max
    pub flex: I16F16,            // flex weight for distributing sibling space (0=none)
    pub visible: u8,             // 0/1 (state: Domain B may set)
    pub enabled: u8,             // 0/1 hit-test/interaction gate
    pub tint: [u8; 4],           // RGBA byte tint for the batched quad
    pub value: I16F16,           // Slider position / ProgressBar fill 0..=1 (state)
    pub _reserved: [u8; 2],
}

/// Text facet (companion component; entity also has UIElement).
#[repr(C)]
#[derive(Clone, Copy, PartialEq, Eq, bytemuck::Pod, bytemuck::Zeroable,
         serde::Serialize, serde::Deserialize, Debug)]
pub struct UIText {
    pub element: Entity,         // owning UIElement node
    pub text: FixedStringLike,   // fixed inline string (label, spec 21 discipline)
    pub font: AssetRefLike,      // logical font asset ref (32 B, no abs path)
    pub size: I16F16,            // glyph size in canvas units
    pub color: [u8; 4],          // RGBA
    pub align_h: u8, align_v: u8,// horizontal/vertical alignment
}

/// Interactive facet: rect hit-testing + state + emitted topics.
#[repr(C)]
#[derive(Clone, Copy, PartialEq, Eq, bytemuck::Pod, bytemuck::Zeroable,
         serde::Serialize, serde::Deserialize, Debug)]
pub struct UIButton {
    pub element: Entity,         // owning UIElement node
    pub state: ButtonState,      // Normal / Hover / Pressed / Focused (1 byte)
    pub onClick_topic: u32,      // Emit topic when clicked (0 = none)
    pub onHover_topic: u32,      // Emit topic when hover state changes (0 = none)
    pub onFocus_topic: u32,      // Emit topic on focus gain (0 = none)
    pub _pad: [u8; 3],
}

#[repr(u8)]
#[derive(Clone, Copy, PartialEq, Eq, bytemuck::Pod, bytemuck::Zeroable,
         serde::Serialize, serde::Deserialize, Debug)]
pub enum ScaleMode { Fit = 0, Stretch = 1, Uniform = 2 }

#[repr(u8)]
#[derive(Clone, Copy, PartialEq, Eq, bytemuck::Pod, bytemuck::Zeroable,
         serde::Serialize, serde::Deserialize, Debug)]
pub enum ElementKind { Panel = 0, Image = 1, Slider = 2, Progress = 3 }

#[repr(u8)]
#[derive(Clone, Copy, PartialEq, Eq, bytemuck::Pod, bytemuck::Zeroable,
         serde::Serialize, serde::Deserialize, Debug)]
pub enum ButtonState { Normal = 0, Hover = 1, Pressed = 2, Focused = 3 }
```

### Pure layout + hit-test systems

```rust
/// Deterministic layout: (tree, canvas resolution) -> screen rects.
/// Pure fixed-point; identical for identical input. Run per affected canvas
/// (Domain B or A; layout has no I/O). Result is cached for render batching.
pub fn layout_canvas(canvas: &UICanvas, elements: &[&UIElement]) -> Vec<LayoutRect>;

/// Pure hit-test: pointer position (fixed) against resolved layout rects,
/// back-to-front. Returns the topmost hit-testable element this tick.
pub fn hit_test(rects: &[LayoutRect], pointer: I16F16Point) -> Option<usize>;

/// Domain B: sets element state (visibility/tint/value) in response to gameplay.
pub fn ui_logic_system(view: &StateView<'_>) -> Result<WorldDelta, RecoverableError> {
    // read gameplay columns → produce ColumnWrites against UIElement.value/
    // visible/tint and UIText (e.g. health text). Return delta.
    Ok(WorldDelta::default())
}

/// Domain B: turns this tick's InputSnapshot into hover/focus + click edges,
/// then emits UI events as DeferredCommands into the delta.
pub fn ui_input_system(view: &StateView<'_>) -> Result<WorldDelta, RecoverableError> {
    // 1. read InputSnapshot (spec 03): pointer fixed position, just_pressed.
    // 2. hit_test across active canvases in z-order.
    // 3. on a hover change / focus / just_pressed edge, update UIButton.state
    //    column and push DeferredCommand::Emit{ topic: ..., data }.
    Ok(WorldDelta::default())
}
```

Domain A then drains `WorldDelta.deferred`, consumes the UI topics to (a) re-tint
the pressed/hovered widget's retained quad and (b) invoke any gameplay-bound
action through the host action router (which for edit-world design-time actions
goes through spec-23 commands, and for play-world actions calls back into Domain
B via the normal `Emit` event path). The renderer draws from the retained quad
list produced by `layout_canvas`.

## Components

The game-UI system contributes four registered components in the reserved
**50–59** band (spec `21`). **No UI component re-registers `Transform` (2) or
`Camera` (7).**

| `ComponentId` | Name        | `size_of` (design) | semantic contract                           |
|---------------|-------------|--------------------|---------------------------------------------|
| 53            | `UICanvas`  | ~24                | screen root; design res + scale mode + sort |
| 54            | `UIElement` | ~64                | layout (anchors/margins/flex) + kind + state|
| 55            | `UIText`    | ~112               | text facet: string, font ref, color, align  |
| 56            | `UIButton`  | ~28                | hit-test facet + state + emit topics        |

IDs 50–52 are the sequencer (spec `46`); 57–59 are reserved for future
registered element variants (e.g. a dedicated `UISprite`, `UIPanel`) and must
not be reused here. Game/mod components ≥ 1024 remain free (spec `21`).
Image/Slider/ProgressBar are `UIElement.kind` variants, not separate registered
IDs in this pass.

## Constraints

- **Determinism first.** Layout and hit-test are fixed-point, resolution-driven
  pure functions; no wall-clock, no `f32` in Domain B, no `HashMap` iteration
  (elements walked in `Parent`-sorted order via `BTreeMap`/sorted `Vec`)
  (AGENTS.md § 3). Element state is component data Domain B writes by delta.
- **Data, not a singleton.** Every canvas/element/primitives is an ECS entity +
  component; no host-retained widget singleton the game pokes.
- **Domain boundaries.** Raw input (`winit`) and the quad/SDF renderer
  (`wgpu`, fonts) are Domain A. Domain B sees only the fixed `InputSnapshot`
  (spec `03`) and returns `WorldDelta`s (layout/hit-test/logic). No `egui`,
  `wgpu`, or device call ever crosses into the guest.
- **UI events are `DeferredCommand`s.** OnClick/OnHover/OnFocus surface through
  `WorldDelta.deferred` as `Emit` topics; Domain A consumes them. No hidden
  callback closures live on the host as gameplay behavior.
- **Game logic drives the HUD by setting state.** Visibility/tint/value/visibility
  are columns; Domain B writes them. Gameplay never draws.
- **Authoring is edit-world + spec-23 commands.** Layout/text/hierarchy edits are
  undoable commands; preview runs on the play world (spec `22`).
- **Reuse over invention.** Parenting uses `Parent`(4); asset/font refs are
  logical tokens (no absolute paths, AGENTS.md § 5); rendering reuses the spec-04
  batch/emission boundary and quantizes fixed→f32 there.
- **Headless-clean.** Layout/hit-test/state systems run with no GPU/window; only
  the actual quad/SDF raster is Domain-A/device-gated. Portability
  (`x86_64-linux`/`aarch64-linux`) and offline CI hold.

## Performance Targets

- `layout_canvas`: < **100 µs** for a 200-element canvas (pure fixed-point,
  O(n) + flex passes); cached, re-run only on change.
- `hit_test` per tick: < **10 µs** (back-to-front scan over resolved rects; an
  AABB/spatial index of rects keeps it flat at HUD scale).
- Retained frame: a static HUD draws the cached quad+SDF list with **< 8 draw
  calls** for a typical HUD; invalidating a single dirty widget re-rasterizes
  only that subtree (< 1 ms).
- Per-element Domain-B state writes (value/tint/visible): one `ColumnWrite`, **<
  1 µs**.
- Whole UI logic+layout+hit-test Domain-B pass per tick: **< 1 ms** typical HUD.

## Testing Strategy

All headless (no GPU / no window):

- **Layout math:** anchors/margins/flex produce exact fixed-point rects at given
  design resolutions; assert against hand-computed golden rects; fit/stretch/
  uniform scale maps design → live canvas identically across 2 resolutions.
- **Hit-test:** pointer over/under/overlapping widgets, disabled nodes skipped,
  topmost-wins, canvas z-order; edge semantics from spec `03` (click fires one
  tick, hover on state *change*).
- **State writes via delta:** a Domain-B `ui_logic_system` updates health text
  value/color; assert the emitted `ColumnWrite` targets the UIText/UIElement
  columns and the host applies it (spec `00`).
- **Event surfacing:** a click on a button produces one
  `DeferredCommand::Emit{ onClick_topic }`; hover/focus symmetric. Verify the
  host consumes it and, for a bound action, routes through spec-23 command
  (edit world) — end to end, no GPU.
- **Authoring via commands:** add/move/retarget UI nodes through spec-23
  commands; undo/redo restores bit-identical UI columns; serialized history
  replays identical deltas (spec `16`/`23`).
- **Determinism:** layout + hit-test + state the same input 3×; assert
  byte-identical `WorldDelta`s and identical resolved rects each run.
- **Isolation:** HUD authored on the edit world is unchanged when previewed on
  the play world; play-time element-state writes never reach the edit world
  (spec `22`).
- **No time/device leak:** Domain-B UI tests fail to compile on `std::time`/
  `wgpu` references; purity gate reports `[PURE]`; no `wgpu::Device` is ever
  constructed in logic tests.

## Dependencies

- `contracts` (`StateView`, `WorldDelta`, `ColumnWrite`, `DeferredCommand`,
  `ComponentId`, `Entity`, `RenderKind`) — spec `00`.
- `openengine-math` (fixed-point layout/hit-test/state math).
- `crates/ecs` (`Parent`=4 layout; spec `21` fixed-inline string / asset-ref
  discipline).
- Input snapshot & edge semantics from spec `03`.
- Quad + SDF render emission boundary from spec `04`; font/texture assets via
  spec `02` registry.
- `crates/editor` — spec-23 authoring commands, edit/play preview (spec `22`),
  viewport compositing of UI overlay (spec `24`).
- `openengine-serial` (spec `16`) for HUD persistence.
- ComponentId band shared with sequencer spec `46` (50–52 there, 53–56 here,
  57–59 reserved).

## Next Steps

1. Register `UICanvas`/`UIElement`/`UIText`/`UIButton` (53/54/55/56) with
   `#[repr(C)]` `Pod` layouts + layout asserts in
   `crates/ecs/src/components.rs`.
2. Implement pure `layout_canvas` (anchors/margins/flex, fixed-point) + headless
   golden-rect tests.
3. Implement `hit_test` + the Domain-B `ui_input_system` producing hover/focus/
   click edges and `DeferredCommand` UI events.
4. Implement the Domain-B `ui_logic_system` setting element state by delta.
5. Build the Domain-A batched quad + SDF renderer consuming the deterministic
   layout, with dirty-subtree invalidation.
6. Add spec-23 UI authoring commands with inverse deltas; wire editor preview on
   the play world (spec `22`).
7. Land the headless determinism/purity/event-surfacing test battery.
