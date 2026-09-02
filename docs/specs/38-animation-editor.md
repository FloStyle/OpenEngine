---
spec: "38-animation-editor"
phase: "Phase 5: Animation"
status: "draft"
author: "OpenEngine AI"
created: "2026-09-03"
depends_on:
  - "07-editor-inspector"
  - "16-serialization"
  - "21-primitive-components"
  - "22-edit-vs-play"
  - "23-undo-redo"
  - "24-editor-viewport"
  - "25-editor-shell"
  - "31-multi-selection"
  - "36-skeletal-animation"
  - "37-animation-state-machines"
---
# 38 - Animation Editor

## Overview

The **animation editor** is the Domain-A authoring tooling (spec 21 facts, Domain
A) for clip and controller assets. It is a suite of egui panels opened by the
shell's **Animation** layout (spec 25 `Custom("Timeline")`): a **timeline** for
positioning/scrubbing keyframes per joint, a **curve editor** for editing
interpolation/easing between keyframes, keyframe **add/delete/move**, an in-view
**preview** that plays the authored motion against the selected rig, plus
**retargeting**, **animation events**, and **bulk edit** on multi-selected
keyframes/entities (spec 31).

The animation editor writes **assets** (clips, controllers, retarget maps) that
specs 36/37 consume. Every edit is a reversible `Command` (spec 23) so
timeline/curve operations land on the undo stack like any other editor action. It
operates **on the edit world only** (spec 22): preview playback is a
presentation-only evaluation of the authored clip that never advances gameplay
state and never touches the play world; it is gated to `mode == Edit`. Bulk edits
fan out over the **multi-selection** set (spec 31) and are committed as one
transaction/command.

## Core Concepts

### 1. Editor surface = asset documents, backed by a preview world

Clips/controllers live as **host assets** (spec 02); the editor edits those
assets, not ECS columns. The entity that "owns" a clip in a scene does so through
the `AnimationClip`/`AnimatorController` components (specs 36/37), but the
editor's *unit of work* is the **clip/controller document** held by the editor
and saved back to the asset store on Save (spec 16). Panels therefore show and
mutate a document buffer, and only a "bake/apply to selected" step maps a pose
into a scene entity.

For *preview* the editor keeps a lightweight **preview rig**: a throwaway
`World`/rig copy (or a dedicated editor-skeleton mesh) driven by a
presentation-only sample clock. Preview never calls `enter_play`; it reuses the
`anim-core` sampler (spec 36/37) to evaluate the document being edited and draws
the result through the viewport (spec 24). See § "Preview" below.

### 2. Timeline & scrubbing

The timeline lays out the clip's duration on a horizontal time axis, one
horizontal **track per joint** (optionally grouped by joint, with a per-joint
visibility toggle). On each track sit that joint's **keyframes** (drag horizontally
to **move**, `Del` to delete, double-click to **add** a keyframe at the scrubber
position). A vertical **scrubber** shows the current preview time.

Because animation time in OpenEngine is **tick-driven** (spec 36), the timeline
is measured in **ticks** (1/60 s units), and the scrubber is an integer/fixed
tick value — not a wall-clock `f32` seconds counter. Scrubbing with the mouse
moves the preview sample clock and, when a curve/pose is baked to a rig, writes a
*deterministic* fixed time, never a live `f32`.

```
  track: Hips      ◆────◆──────◆
  track: Spine     ────◆──◆──────◆
  track: L_Arm      ◆──◆────────◆
  timeline: |─────|─────|─────|─────|─────|   (ticks)
             scrubber ▲           Events: "step" at t=30, "sound" at t=60
```

Keyframe **move** updates the keyframe's `t_tick`; keyframe **add** inserts a
keyframe interpolated at the current scrubber position (the editor snapshots the
bracketing track to compute the authored value, but the stored value is the
*explicit* fixed value the user then edits, not a live resample). Delete removes
it and re-links its neighbors.

### 3. Curve editor

Selecting a keyframe (or a joint track) opens the **curve editor**: a 2D graph of
the property (translation/rotation/scale channel, or the blend/easing curve)
against time. The user edits the **interpolation mode** (Step / Linear / Slerp —
spec 36 `CurveMode`) and the **easing** between two keyframes via handles, and can
drag keyframe values vertically to change the authored value at that time.
Curve edits change `CurveMode`/`EaseMode` tags and the `t_tick`/TRS values in the
document; they are pure asset-document edits and therefore deterministic and
replayable.

### 4. Keyframe operations & snapping

- **Add** (`I` or double-click) at scrubber → new `Keyframe`.
- **Delete** (`Del`) on selected keyframes.
- **Move** (drag) with optional **time snapping** to a grid (1 tick, 1/4 tick
  set; or frame-multiple). Snapping quantizes `t_tick`; it is fixed, never float.
- Value editing via the curve editor or inspector (spec 07) updates the TRS.

Each of these routes through a `Command` (below) and is thus undoable.

### 5. Preview playback (edit-world only, spec 22)

Preview plays the authored clip/controller against the selected preview rig:

- **Loop** — `Once`, `Loop`, `PingPong` (matches spec 36 `AnimationPlayer`
  loop modes).
- **Speed** — 0.25×/0.5×/1×/2×/4× playback of the *sample clock*, mirroring spec
  22's speed control: it changes *when* the preview advances ticks, **never** the
  sampled time math, so preview frames are reproducible.
- Play/pause/step forward one tick/step backward one tick.

Crucially, preview is **presentation-only**. It advances a preview sample clock
that drives `anim-core` over the document; it does **not** run `animation_update`
as gameplay, does not mutate gameplay `AnimationPlayer`/`AnimatorController`
columns, does not call `enter_play`, and is disabled unless
`EditorState.mode == Edit`. The play world (spec 22) is untouched by any preview
scrubbing or playback. (Real in-play animation runs in Play mode via specs 36/37;
the editor's own playback is only for inspecting authored data.)

### 6. Retargeting

**Retargeting** lets a clip authored for one rig play on another rig whose joint
hierarchy/names differ. A **retarget map** is an editor-authored asset mapping
source joints → target joints (by joint name, with wildcard/path fallback), plus
optional transform adjustment (per-axis flip / axis remap). The editor previews a
retarget by loading the clip with the map and re-sampling onto the target rig's
`Skeleton` (spec 36), showing which joints mapped and which are unmapped (drawn
as "baked to bind"). Retarget maps are plain serializable data (spec 16) authored
in a dedicated editor pane; a successful retarget is validated headlessly by
sampling the target rig at key ticks.

### 7. Animation events

Clips can carry **events**: time-stamped markers with a small payload (a
user-readable name + a bounded fixed/u8 payload, e.g. a footstep "sound" at a
given tick). The editor shows them on an events lane of the timeline
(add/delete/move/snap, same as keyframes). When a rig plays the clip (spec 36),
crossing an event tick emits it (host side, via a deferred event / sound request);
the editor only *authors* them. Events are stored in the clip asset, not as ECS
columns.

### 8. Bulk edit via multi-selection (spec 31)

Using the editor's **multi-selection** (spec 31 `SelectionModel`), the user can
select multiple keyframes across tracks (and/or multiple rigs) and apply a
**bulk edit** — move all selected by an offset, retime/snap all to a grid, set
all selected to Linear/Slerp, delete all. A bulk edit is issued as **one**
transaction/`Command` that internally fans out per-keyframe edits and collapses
to a single undo step (spec 23 transaction batching). Because the command captures
old/new payloads per keyframe, undoing a bulk edit restores every touched
keyframe in one step.

## Key Rust Types

```rust
// crates/editor/src/anim/timeline.rs — Domain A document + panels.

/// One authored motion document: the clip's joint keyframe tracks, plus events.
/// (Controller graphs are separate controller documents.)
pub struct ClipDocument {
    pub id: AssetId,                 // the asset being edited
    pub duration_ticks: u32,
    pub tracks: Vec<JointTrack>,     // one per joint, parallel to Skeleton joint order
    pub events: Vec<ClipEvent>,
}
pub struct JointTrack {
    pub joint: u32,
    pub curve: CurveMode,            // track default (Step/Linear/Slerp)
    pub keys: Vec<Keyframe>,         // sorted by t_tick (spec 36 Keyframe)
}
pub struct ClipEvent { pub t_tick: u32, pub name: FixedString, pub payload: u32 }

/// The scrubbing sample clock. Tick units, NOT wall clock. Fixed only.
pub struct SampleClock { pub t_ticks: I16F16, pub speed: SpeedMultiplier,
                         pub mode: SampleMode } // Once | Loop | PingPong
```

```rust
// crates/editor/src/anim/commands.rs — spec-23 Command wrapping asset edits.
use crate::commands::{Command, EditorError};
use contracts::WorldDelta;

/// Undoable keyframe/value edit. Captures old and new raw keyframe bytes.
/// execute()/undo() apply to the ClipDocument buffer (the editor's asset work
/// set) rather than an ECS column; the UndoRedoManager (spec 23) owns the stack,
/// transaction collapse, and redo-clearing. Because a clip is asset data, the
/// command's "delta" is an asset-document patch, applied at the flush boundary by
/// the editor's apply_asset_edit (the asset analogue of apply_delta).
pub struct ModifyKeyframeCommand {
    pub doc: AssetId,
    pub joint: u32,
    pub index: u32,                 // index in the sorted track
    pub old: Keyframe, pub new: Keyframe,   // Pod, captured raw (spec 36)
}
impl Command for ModifyKeyframeCommand {
    fn execute(&mut self) -> Result<WorldDelta, EditorError> {
        // replace track[..][self.index] with self.new; return an asset-doc delta
        // (empty WorldDelta on ECS; the doc patch is applied via apply_asset_edit)
    }
    fn undo(&self) -> Result<WorldDelta, EditorError> { /* write self.old back */ }
    fn description(&self) -> String { "Edit keyframe".into() }
    fn serialize(&self) -> Vec<u8> { /* postcard(self), spec 16 */ }
    fn entity_handle(&self) -> Option<Entity> { None } // asset-scoped command
}
```

Other commands share the shape: `InsertKeyframeCommand`, `DeleteKeyframeCommand`,
`MoveKeyframeCommand`, `SetInterpolationCommand`, `AddAnimationEventCommand`,
`RetargetRemapCommand`, and the multi-selection `BulkKeyframeCommand` (which owns a
`Vec` of member commands and collapses to one undo step, per spec 23).

> **Asset-edit vs. ECS-edit note (spec 23).** Spec 23's canonical commands produce
> a `WorldDelta` applied to the edit-world ECS. Clip/controller/retarget edits
> mutate **asset documents** instead. This spec reuses spec 23's *manager*,
> transaction batching, redo-clearing, save-point and serialization discipline
> unchanged, but the executed payload is an **asset-document patch** routed through
> an `apply_asset_edit` boundary (the asset analogue of `apply_delta`). Both are
> pure editor (Domain A) concerns; neither ever touches the play world. This is a
> deliberate, documented extension of spec 23 to non-column editor state.

## Components

| ComponentId | Name | Domain | What it is |
|-------------|------|--------|------------|
| —           | (none) | —    | The animation editor **defines no new ECS components**. It edits clip/controller assets (specs 36/37) and reuses existing `Skeleton`, `AnimationClip`, `AnimationPlayer`, `SkinnedMeshRenderer`, `AnimatorController` (10–14) only as handles it reads to bind preview rigs. |

The editor is tooling (Domain A) and stays out of the component registry. Any
future ECS column the tooling would *add* (e.g. a dedicated IK-effector
component) falls in the reserved **15–19** range and is authored by the spec that
defines it, not by this one. This spec does **not** consume a new ComponentId.

## Constraints

- **Domain A only.** The editor, timeline, curves, preview, retargeting, events,
  and bulk-edit commands live in `crates/editor` (Domain A). `glam`/egui `f32`
  is allowed for the *UI/render presentation* (drawing curves, gizmos) but **never
  stored in documents or used as the sample clock**; authored values stay fixed.
- **Edit world / edit mode only (spec 22).** Preview playback and scrubbing run
  only when `EditorState.mode == Edit`; preview never calls `enter_play`, never
  mutates gameplay components, and never touches the play world. Real playback is
  Play-mode specs 36/37.
- **Tick-driven sample clock.** Preview/scrub time is fixed `I16F16` tick units,
  advanced by `FIXED_STEP * speed`; speed changes dispatch cadence only (spec 22
  speed semantics), so preview frames are reproducible. No wall-clock `f32`
  seconds in the clock.
- **Every edit is a `Command`.** Timeline/curve/retarget/event/bulk edits route
  through the spec-23 manager as (asset-document) commands. UI never mutates a
  document directly. Bulk edits are one transaction → one undo step.
- **Command re-runnability.** Keyframe commands capture old/new `Keyframe` Pod
  bytes (never re-derived `f32` at undo time) and serialize via postcard, so
  undo/redo and crash recovery are deterministic and bit-exact (spec 23).
- **Assets stay assets.** Clip/controller/retarget/event data are serializable
  host assets (spec 02/16), never shipped as ECS columns or into the guest.
- **Graph/data is in the document.** Controller states/transitions/layers remain
  asset sub-structures (spec 37); the editor edits that asset, not per-state
  entities.
- Portability `x86_64-linux`/`aarch64-linux`; panel *logic* (timeline math,
  command deltas, retarget validation) is headless-testable with no GPU/window;
  egui/wgpu paths are feature-gated.

## Performance Targets

- Timeline redraw (track/keyframe hit-testing + layout) for a 60-joint clip at
  60 Hz: **< 1 ms**.
- Keyframe add/delete/move command construction: **< 0.5 ms** each.
- Scrubbing sample (via `anim-core`) at 60 ticks/s: negligible; preview of a
  60-joint clip **< 2 ms/tick**.
- Curve editor redraw: **< 1 ms** (sub-line culling).
- Bulk edit fan-out on a 100-keyframe selection: issue + collapse to one command
  **< 5 ms**; single-step undo restores all.
- Retarget validation (sample target rig at ~8 key ticks): **< 5 ms**.

## Testing Strategy

All headless (no GPU/window) in `crates/editor`:

- **Timeline math.** Scrubber tick ↔ track/keyframe hit mapping is exact;
  snapping quantizes `t_tick` to the active grid.
- **Keyframe ops.** Insert/delete/move mutate the sorted track correctly and keep
  it sorted; deleting the last keyframe is handled; adding at scrubber
  interpolates a correct authored value.
- **Undo/redo per command.** For each command: execute → apply_asset_edit →
  undo → assert the document is bit-identical to pre-edit (postcard compare of
  the `ClipDocument`); redo restores the post-edit state.
- **Transaction collapse.** A bulk edit of 100 keyframes produces exactly **one**
  undo step that restores all 100 in a single undo.
- **Determinism.** Run the same keyframe edit + scrub script **3×**; assert
  identical document bytes and identical preview sample output.
- **Preview isolation.** While `mode == Edit`, drive preview playback/scrub across
  the full loop range; assert no gameplay component changed, `play_world` is
  `None`, and undo history is unchanged (spec 22). Assert preview is rejected when
  `mode != Edit`.
- **Preview modes.** Once clamps and stops; Loop wraps; PingPong reverses; speed
  scaling changes dispatch cadence but not the per-tick sample math (spec 22
  speed invariance, run preview at 0.25× and 4× → identical sampled poses at each
  covered tick).
- **Retarget.** Map a clip onto a renamed target rig; assert joint mapping,
  unmapped joints bake to bind, and sampled poses on target match source where the
  hierarchy matches.
- **Animation events.** Add/move/delete events on the lane; assert authored
  `t_tick`/payloads round-trip through the clip asset and fire when the clip
  plays in spec 36 (host event emit at crossing ticks).
- **Editor-ECS isolation.** Assert the editor defines/registers no ComponentId and
  adds none to the shared component registry (only reads handles 10–14).

## Dependencies

- `crates/editor` (Domain A) — timeline/curve/preview/retarget/event panels and
  commands; egui + `glam` (presentation only).
- Undo/redo manager + transaction batching from spec 23 (asset-document commands);
  edit-vs-play gating from spec 22; viewport/gizmo rendering from spec 24; shell
  `Custom("Timeline")` panel + Animation layout from spec 25; inspector (07) for
  value editing; multi-selection from spec 31 for bulk edits.
- Asset store/codec (spec 02/16) for clip/controller/retarget persistence;
  `openengine-anim-core` (spec 36/37) for preview sampling; component handle
  bindings 10–14 (specs 36/37) for preview-rig binding.
- `bytemuck`, `serde`/`postcard`, `openengine-math`.

## Next Steps

1. `ClipDocument`/`JointTrack`/`ClipEvent` + asset load/save in the editor.
2. `SampleClock` (tick-driven) + preview rig binding to `anim-core` (spec 36).
3. Timeline panel: tracks, keyframe add/delete/move, scrubber, snapping.
4. Curve editor: interpolation/easing editing.
5. Asset-document `Command` set + `apply_asset_edit` boundary wired into the
   spec-23 manager; per-command undo/redo + serialization tests.
6. Retargeting pane + map asset + validation.
7. Animation-events lane + authoring, and spec-36 host emission on clip play.
8. Bulk edit via spec-31 multi-selection (one transaction).
9. Headless test battery (timeline, undo/redo, determinism, preview isolation,
   retarget, events).

---
