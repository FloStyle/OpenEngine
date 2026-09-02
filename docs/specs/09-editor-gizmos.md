---
spec: "09-editor-gizmos"
phase: "Phase 4"
status: "design"
---

# Editor Gizmos

## Overview

3D transform gizmos for the in-editor scene view: translate / rotate / scale
handles rendered as overlays and dragged with the mouse to author entity
placement. Gizmos are **pure Domain A** — they live in `crates/editor`, own no
gameplay state, and mutate the world only by emitting a [`WorldDelta`] that the
host ECS applies atomically. None of the code below ever runs in Domain B, so it
may freely use `std`, `wgpu`, `winit`, `egui`, and the camera math of the
editor. Gameplay logic never sees a gizmo: a gizmo edit is *input*, and input is
turned into a delta exactly like any other authoring action.

Because the engine's determinism law forbids `f32` in *logic* math only,
gizmo math — which is presentation + authoring and never runs in the pure
sandbox — uses `glam` `f32` transforms against the render camera, then emits
`Transform` (3D) updates snapped to fixed-point `openengine-math` values in the
delta.

## Core Concepts

### What a gizmo is

A gizmo is an interactive overlay bound to one selected entity (or a group).
It has three fixed modes:

- **Translate** — three axis arrows (X/Y/Z) plus optional planar squares.
- **Rotate** — three rings; drag arcs the handle around the selected axis.
- **Scale** — three axis handles plus a uniform corner handle.

Every handle is an axis tag + an id so hit-testing can name exactly one handle:

```rust
pub struct GizmoMode { /* translate | rotate | scale */ }

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Axis { X, Y, Z }

/// A concrete pickable handle for the current mode.
pub enum HandleId {
    Axis(Axis),
    Plane { u: Axis, v: Axis },   // translate/scale planar drag
    Uniform,                     // scale-only center handle
    Ring(Axis),                  // rotate
}
```

### Picking = mouse ray vs. handle geometry

When the mouse moves over the scene view the editor casts a ray from the mouse
NDC through the camera and tests it against each handle's *world-space proxy
shape* (a capsule along the axis for translate/scale, a torus for rings). The
closest hit wins; distance is capped to a screen-space pixel tolerance so
handles do not grab from arbitrarily far away.

```rust
pub struct Ray { pub origin: glam::Vec3, pub dir: glam::Vec3 }

pub struct Hit {
    pub handle: HandleId,
    pub t: f32,                 // parametric distance along the ray
    pub entity: Entity,         // which selected entity owns this handle
}

/// Screen-space tolerant ray/handle test. `pixels` derives from an editor
/// sensitivity setting; large views auto-scale the proxy radius.
fn ray_hit(ray: &Ray, gizmo: &GizmoState, cam: &EditorCamera, pixels: f32)
    -> Option<Hit>;
```

### Dragging

A drag captures the selected handle at the mouse-down `Hit`. Each frame the
editor projects the handle start point onto the current mouse ray's nearest
point within the *drag plane* (the plane through the anchor perpendicular to the
axis for axis drags, or the view-facing plane for plane drags). The delta from
the start to the current projected point is the drag vector.

## Emitting a world change

Gizmo edits do not write ECS memory directly. They return a [`WorldDelta`] —
the *same* mutation channel Domain B uses — so the invariant "state changes flow
one way: guest/host produce a delta, ECS applies it" is preserved even for
editor authorship. The editor routes the resulting delta through
`apply_delta(world, &delta)` so generation guards, column bounds checks and
rollback-on-error all behave exactly as they do for gameplay.

The gizmo targets the **`Transform`** component (**`ComponentId(2)`**, the 3D
component in the spec-21 registry) — never `Position` (`ComponentId(0)`), which
is the separate 2D pair. Because gizmos author 3D placement they mutate the 3D
`Transform` column. `ColumnWrite.payload` must be a **whole-element write** of
`indices.len() * element_size` bytes for that column — a bare `FxVec3`
(translation only) cannot be written into a larger `Transform` row. The editor
therefore performs a **full-row read-modify-write**: it reads the entity's
current whole `Transform` (as a `&[u8]`/fixed-point row via a read query, spec
00), applies the drag to the translation (and scale/rotation for those modes)
inside that row, and emits the *complete* updated `Transform` row as the payload.
(An alternative is a dedicated authored column holding just the gizmo-authored
value, but the default is a full `Transform` read-modify-write.)

```rust
// Fixed-point world math used to build the write payload.
use openengine_math::{I16F16, FxVec3};
use contracts::{ComponentId, ColumnWrite};

const C_TRANSFORM: ComponentId = ComponentId(2);   // 3D Transform, spec-21 registry

struct DragSession {
    entity: Entity,
    handle: HandleId,
    mode: GizmoMode,
    space: TransformSpace,          // Local | World
    anchor_world: FxVec3,           // fixed-point origin of the drag
    start_world: FxVec3,            // fixed-point handle start
    last_world: FxVec3,             // for incremental updates / snapping
    snap: Option<FpSnap>,           // grid increment, fixed-point
}

impl DragSession {
    /// Produce the delta for this frame's mouse position (already resolved to
    /// a fixed-point point `p` on the drag plane by the editor).
    fn delta_to(&self, p: FxVec3) -> Option<WorldDelta> {
        // Per-mode derivation of the *new* transform for `entity`. `row` is the
        // full current `Transform` element bytes (read via a read-only query),
        // re-read each frame so the write is a whole-element read-modify-write.
        let row = self.read_transform(&self.entity)?;         // full Transform element
        let row = match self.mode {
            GizmoMode::Translate => set_translation(row, self.anchor_world + (p - self.start_world)),
            GizmoMode::Scale      => set_scale(row, apply_scale(self, p)),
            GizmoMode::Rotate     => set_rotation(row, rotate_around(self, p)),
        };
        let row = snap_row(self.snap, row);

        let archetype = self.entity_archetype;          // cached ArchetypeId
        let payload = row.to_bytes();                    // full element, == element_size
        debug_assert_eq!(payload.len() as u32, transform_element_size());
        let mut delta = WorldDelta::default();
        delta.writes.push(ColumnWrite {
            archetype,
            component: C_TRANSFORM,                       // Transform (3D), NOT Position
            indices: vec![self.entity.index],
            payload,                                       // whole Transform row
        });
        Some(delta)
    }
}
```

The delta targets the **edit world** (spec 22) and, when the gizmo action is
undoable, is wrapped in a spec-23 `Command` (via a transaction that collapses the
drag frames into one undo step) before being applied.

### Local vs world space

- **World**: drag vector is added to the world-space anchor; handles point
  along world axes.
- **Local**: the axis directions and the drag vector are rotated by the
  selected entity's current (fixed-point) orientation first.

The anchor and drag math above therefore runs in the chosen space; when the
delta is written the host converts once to the world column the ECS stores.

### Snapping

`FpSnap` is an optional fixed-point grid increment (e.g. `I16F16::from_num(0.5)`
units). `snap_to` rounds each axis to the nearest grid multiple *before* the
delta is emitted, so the snapped value is the one committed — no drift, and
bit-identical regardless of which frame the mouse lands on.

## Gizmo rendering

Gizmos are **debug/draw overlays**, not gameplay geometry. The editor emits
them through the same [`RenderKind::Gizmo`] debug channel Domain A owns. Line
and torus meshes are uploaded once into a dedicated gizmo pipeline (no
textures, small vertex budget), and a thin immediate draw list is rebuilt every
frame from the current gizmo state:

```rust
pub struct GizmoFrame {
    pub lines:   Vec<GizmoLine>,     // axis arrows, rings (as polyline)
    pub cones:   Vec<GizmoCone>,     // translate arrowheads
    pub quads:   Vec<GizmoQuad>,     // planar squares / hover highlights
    pub hover:   Option<HandleId>,
    pub active:  Option<HandleId>,   // brighten / color the held handle
}
```

The renderer rasterizes `GizmoFrame` *after* the opaque pass and *before* the
egui UI overlay, writing depth-test-disabled (or biased) so handles always read
over geometry. Handles only render while a gizmo is shown (entity selected +
gizmo toggle), and never touch the render list Domain B produces.

### Editor camera & color convention

- X = red, Y = green, Z = blue (matches wgpu/egui conventions already used).
- Hovered handle brightens; the active/dragged handle is yellow.
- Handles shrink/scale with viewport so their screen size stays comfortable.

## Editor-only gate

`crates/editor` and any gizmo code are excluded from the Domain B build graph.
Nothing here is exported to wasm; the `openengine-logic-sandbox` /
`openengine-logic-export` crates never reference gizmos. The editor's `std`
usage (event loop, files, threads) is confined to Domain A, satisfying the
portability and dependency-direction rules.

## Key Rust types

- `GizmoState` — holds mode, selected `Entity` set, `DragSession`, snap.
- `HandleId`, `Axis`, `GizmoMode`, `TransformSpace`, `FpSnap`.
- `Ray`, `Hit`, `EditorCamera`.
- `GizmoFrame` + builder types consumed by the gizmo render pipeline.
- Output: `WorldDelta` built from `ColumnWrite` (see `contracts`).

## Constraints

- Gizmos are authoring-only: no influence on a shipped (non-editor) frame.
- Emitted deltas go through `apply_delta`; never direct ECS writes from the
  input path.
- **Edit world only.** Gizmo edits target the **edit world** (spec 22); the play
  world is read-only for the editor and gizmos never mutate a simulated world.
- **Undoable gizmo edits are spec-23 `Command`s.** A gizmo drag is a spec-23
  `Command` (collapsed into one undo step per transaction) whose produced delta
  is applied to the edit world; gizmo output is not a second mutation channel.
- `Transform` (3D) writes are snapped fixed-point (`openengine-math`), written
  as whole-element `ColumnWrite.payload` rows (read-modify-write of the full
  `Transform`), matching the units Domain B would write — a gizmo move and a
  script move land in the same units. Gizmos never write the 2D `Position`
  component.
- No `f32` enters any component column; `f32` stays inside editor camera and
  gizmo render math.
- Portability: no hardcoded paths, no GPU assumptions beyond wgpu, works with
  mouse + touch (editor), compiles on `x86_64-linux` and `aarch64-linux`.

## Performance

- Pick test is per-handle (≤ ~10 primitives per frame) — sub-microsecond.
- Gizmo draw list is a few thousand vertices; cached meshes never rebuilt per
  frame.
- Target: picking + gizmo frame build < 0.5 ms at any entity count (only the
  *selected* entity draws handles).
- Delta emission is once per authored frame, negligible.

## Testing strategy

- Unit: ray/axis/torus hit math; drag → `WorldDelta` `Transform` math; snapping
  to grid; local vs world derivation; scale/rotate math.
- Integration: drive a synthetic mouse ray + drag session, apply the resulting
  delta to a seeded world, assert the fixed-point `Transform` column changed by
  the expected vector and that two identical drags yield bit-identical columns.
- Property: for many random drags, the emitted `ColumnWrite.payload` is a whole
  `Transform` row of exactly the registered `Transform` `element_size` and
  remains in fixed-point range.
- Editor smoke test: no GPU in CI — verify gizmo *math* headless; render path
  gated behind a feature and tested manually.

## Dependencies

- Domain A only: `glam` (editor camera / gizmo mesh math), `bytemuck`,
  `crates/editor` (egui), `wgpu`, `contracts` (`Entity`, `ComponentId`,
  `ColumnWrite`, `WorldDelta`, `RenderKind::Gizmo`), `openengine-math`
  (fixed-point emission). The gizmo render pipeline reuses `crates/core`'s wgpu
  device/submit path via `RenderKind::Gizmo`.

## Next steps

1. `HandleId` / `Axis` / `GizmoMode` + `EditorCamera` ray casting in editor.
2. Ray/handle hit test + hover/active selection.
3. Drag → fixed-point `WorldDelta` emission + local/world space.
4. Snapping and `apply_delta` round-trip of gizmo edits.
5. Gizmo draw list + wgpu overlay pipeline behind a feature flag.
