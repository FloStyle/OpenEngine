---
spec: "46-sequencer"
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
# 46 - Cinematic Sequencer

## Overview

The cinematic sequencer is OpenEngine's Unreal-Sequencer-like timeline system:
an author can place **tracks** (transform, animation, audio, event, and
cinematic-camera) on a **sequence** with **keyframes**, cut it into **shots**
with **cuts / transitions**, preview it live in the editor viewport, and render
it to a deterministic, GPU-free playout path.

Every piece of sequencer *data* — the sequence, its tracks, its keyframes, and
the shot list — is stored as **ECS components** (spec `00`), so the whole cut is
**pure, serializable data** (spec `16`), readable/writable by any tool exactly
like a scene. Sequencing has no wall-clock, no `f32` math, and no engine
singleton: it is just another set of components that Domain-B pure systems
consume each tick and that the editor mutates through spec `23` commands.

The design deliberately reuses two already-frozen primitives instead of
re-inventing them:

- **`Transform` (ComponentId 2)** — every 3D transform track writes into an
  existing `Transform` column. The sequencer never adds its own position/
  rotation/scale type.
- **`Camera` (ComponentId 7)** — every cinematic camera is an ordinary camera
  entity; a cinematic-camera track drives that entity's `Camera`/`Transform`,
  and active-camera switching is a write to the `Camera.active` byte exactly as
  the scene codec and render pipeline already interpret it.

New **sequencer components occupy the reserved ComponentId band 50–52**
(see Components and the registry note in spec `21`):

| `ComponentId` | Name              | role                                           |
|---------------|-------------------|------------------------------------------------|
| 50            | `Sequence`        | root of one cut; owns playhead + duration      |
| 51            | `SequenceTrack`   | one timeline lane bound to a target entity     |
| 52            | `SequenceKeyframe`| a typed sample on a track at a tick            |

IDs 53–59 are reserved for the UI system (spec `47`) and future cinematic
extras; they must not be reused by this spec.

## Core Concepts

### A sequence is a component tree (data, not a singleton)

A `Sequence` entity is the root. Its **tracks** and **shots** are child entities
linked through the existing `Parent` component (spec `21`, ComponentId 4 —
`Entity::INVALID` is a root). Tree vs. timeline:

```text
Sequence (e.g. "intro_cutscene")
├── Track  "hero.move"        kind=Transform   target=Hero
│     ├── Keyframe t=0    position/rotation
│     ├── Keyframe t=120  position/rotation
│     └── Keyframe t=300  position/rotation
├── Track  "hero.idle"        kind=Animation   target=Hero  anim_asset="idle"
├── Track  "vo"               kind=Audio       asset="lines/01"
├── Track  "door.open"        kind=Event       topic=0x10
└── Track  "cam.main"         kind=CinematicCamera  target=MainCamera
```

Tracks and keyframes hang off the sequence as children (`Parent`), so a sequence
is serialized, cloned, spawned/despawned, and undo/redo'd by the same ECS
machinery as any scene tree — the sequencer introduces **no second storage
format**.

> Why keyframes are child entities and not a `Vec` on the track: components are
> `Pod` + fixed-inline (spec `00`/`21`); a growable keyframe array is not a
> single `Pod` cell. Modeling one `SequenceKeyframe` per child entity keeps every
> sample a plain column row, lets a keyframe live in exactly one archetype, and
> lets keyframe edit commands (spec `23`) target one entity handle each. This is
> the "repeated multi-value ⇒ child components" idiom already used for tags.

### The playhead is tick-counted, never wall-clock

Following spec `05`, sequencer time is **whole fixed ticks** (`StateView::tick`),
not seconds and not `f32`. A sequence's `duration` is in ticks; every
`SequenceKeyframe.tick` is a tick offset from the sequence start. This keeps the
cut deterministic: the same play world, the same injected tick, produces the
same evaluated pose for every keyframe interpolation — independent of host
speed, pacing, or screen refresh.

`sim_time`/`delta_time` are *derived* from ticks with exact step constants
(spec `05`) and are never stored. Playback in the **edit/play world** (spec `22`)
is a Domain-B pure system that, for a given tick, emits a `WorldDelta`; the same
delta applies whether the user scrubs in the editor or the runtime plays the
cut, because scrubbing and playing only differ in *which ticks* are fed, never
in how a tick is evaluated.

### Keyframe evaluation is a pure, per-tick function

A track's value at tick `t` is computed from the track's sorted keyframes
(`BTreeMap`-ordered by construction, never a `HashMap` iteration). This spec
reuses the animation evaluation *pattern* of the planned skeletal-animation and
animation-state-machine specs (**36 / 37 / 38**): constant / linear / smoothstep
interpolation between two bracketing keyframes, applied through fixed-point
math. Spec 36/37/38 are not yet in the tree; when they land, the sequencer's
transform/animation tracks will delegate to their samplers rather than
re-implement splines. Until then the sequencer ships its own minimal, fixed-point
`evaluate_track(track, t)` used by the transform/camera/event tracks.

```rust
/// Pure: given a track's keyframes (already sorted) and a tick, yield the
/// bracketing keyframes and the fixed-point local fraction 0..=1 between them.
fn bracket<'a>(
    keys: &'a [KeyframeRef], t: u64,
) -> Option<(&'a SequenceKeyframe, &'a SequenceKeyframe, I16F16)>;
```

Because interpolation only ever blends two authored fixed-point samples, a
sequenced value is bit-identical given identical keyframes and tick.

### Cinematic camera tracks reuse `Camera`

A `SequenceTrack { kind: CinematicCamera }` targets an ordinary **camera rig
entity** carrying `Transform` (2) + `Camera` (7). Evaluating the track writes:

- the rig's `Transform.position` / `Transform.rotation` (camera placement), and
- `Camera.active` (which camera is live for this shot).

Cutting to a shot therefore toggles the `active` byte exactly the way the render
pipeline (spec `04`) expects "exactly one active camera"; the sequencer never
adds a second camera-representation type. Camera *lens* fields (`fov_y`, `near`,
`far`) are authored on the `Camera` entity itself and are not keyframed by this
spec's first pass (a future cinematic-lens track may key them; the type already
holds them).

### Shots, cuts, and transitions

A `Sequence` optionally names **shot boundaries**: tick ranges that denote a
single framing/camera intent. For this spec a shot is modelled by grouping
camera-keyframe markers: two consecutive `SequenceKeyframe`s on a
`CinematicCamera` track that carry a `ShotBoundary` flag delimit one shot.
The cuts are where a new camera `active` write happens; **transitions** (a
crossfade/dissolve between two cameras over N ticks) are authored as an
interpolation window between two shots rather than a hard toggle. A transition
of length `L` ticks blends the two cameras' placement/activation over `L` ticks
in fixed-point; a zero-length transition is a hard cut.

Hard cuts and short dissolves keep the GPU/visual layer simple: at render time
the active camera is *the* camera, and a dissolve is represented by authoring
both cameras' rigs to cross in space/activation across the window. No
multi-camera blending buffer is required in this spec's render path.

### Preview vs. render are the same tick loop

- **Preview** (editor): scrub/play feeds ticks of the sequence into the **play
  world** (spec `22` Play-in-Editor). The editor viewport (spec `24`) renders the
  play world each frame; the sequencer is only producing `WorldDelta`s, so
  preview is literally "play the sequence."
- **Render** (bake): the same tick-evaluation loop runs headlessly (no GPU, no
  window) and writes, per tick, the evaluated world columns to a **movie asset**
  or a deterministic `WorldDelta` log — the exact binary the runtime replays.
  There is **no separate render-only code path**; render is the same pure system
  plus a recorder that snapshots each post-tick `WorldHash` (spec `15`) for the
  master copy and provenance.

### Authoring is edit-world + spec-23 commands

Because the editor edits the **edit world** (spec `22`), every sequencing
authoring action is a spec-`23` `Command`:

- `AddSequenceCommand`, `AddTrackCommand`, `AddKeyframeCommand`,
  `MoveKeyframeCommand`, `DeleteKeyframeCommand`, `MoveTrackCommand`,
  `ReorderTrackCommand` — each produces the appropriate
  `WorldDelta` (spawn/despawn/migrate/`ColumnWrite`) and an inverse delta, and
  each is undoable/redoable and serializable (crash recovery).

Editing the sequence never mutates ECS storage directly; the timeline UI builds
commands and flushes them at the boundary (spec `23` invariant). Scrubbing the
playhead for *preview* does not write authoring data — it feeds ticks to the
play world.

## Key Rust Types

```rust
//! crates/logic-sandbox/src/sequencer/  (Domain B: pure, no_std, fixed-point)
//! Storage layouts also live in crates/ecs/src/components.rs for Domain A.
#![forbid(unsafe_code)]

use contracts::{AssetRef, ComponentId, Entity, RenderKind, WorldDelta};
use openengine_math::I16F16;

/// Registry bindings for the cinematic band (frozen, see spec 21 registry).
pub const C_SEQUENCE:        ComponentId = ComponentId(50);
pub const C_SEQUENCE_TRACK:  ComponentId = ComponentId(51);
pub const C_SEQUENCE_KEYFRAME: ComponentId = ComponentId(52);

/// What one timeline lane drives. Discriminant stored as u8 + reserved pad.
#[repr(u8)]
#[derive(Clone, Copy, PartialEq, Eq, bytemuck::Pod, bytemuck::Zeroable,
         serde::Serialize, serde::Deserialize, Debug)]
pub enum TrackKind { Transform = 0, Animation = 1, Audio = 2,
                     Event = 3, CinematicCamera = 4 }

/// Root of one cut. `duration_ticks` is whole ticks (spec 05), not seconds.
#[repr(C)]
#[derive(Clone, Copy, PartialEq, Eq, bytemuck::Pod, bytemuck::Zeroable,
         serde::Serialize, serde::Deserialize, Debug)]
pub struct Sequence {
    pub duration_ticks: u64,   // authored cut length in ticks
    pub start_tick: u64,       // host/world tick the sequence began (scrub or play)
    pub playing: u8,           // 0/1 — sequencing is advancing this play session
    pub loop_: u8,             // 0/1 wrap duration_ticks; 0 plays once then holds
    pub muted: u8,             // 0/1 editor-only: skip all tracks (preview toggle)
    pub _reserved: [u8; 5],    // pad → multiple of 8 for SoA cleanliness
}

/// One lane. `sequence` is the owning root (mirrors Parent for fast lookup);
/// `target` is the entity this lane drives (INVALID for event/broadcast).
#[repr(C)]
#[derive(Clone, Copy, PartialEq, Eq, bytemuck::Pod, bytemuck::Zeroable,
         serde::Serialize, serde::Deserialize, Debug)]
pub struct SequenceTrack {
    pub sequence: Entity,          // owning Sequence root (also the Parent edge)
    pub target: Entity,            // driven entity (e.g. Hero / camera rig)
    pub kind: TrackKind,           // 1 byte
    pub weight: I16F16,            // lane blend weight 0..=1 (fixed) for mixing
    pub asset: AssetRef,           // contracts::AssetRef for Audio/Animation tracks
    pub mute: u8,                  // 0/1 per-lane editor mute
    pub _reserved: [u8; 3],        // pad → multiple of 4
}

/// A single typed sample. `track` is the owning lane. Values are fixed-point or
/// opaque u32/u8 tokens so the cell is Pod + no f32.
#[repr(C)]
#[derive(Clone, Copy, PartialEq, Eq, bytemuck::Pod, bytemuck::Zeroable,
         serde::Serialize, serde::Deserialize, Debug)]
pub struct SequenceKeyframe {
    pub track: Entity,             // owning SequenceTrack (also Parent edge)
    pub tick: u64,                 // tick offset within the sequence (0..=duration)
    pub interp: Interp,            // Constant / Linear / Smooth (1 byte)
    pub value: [I16F16; 4],        // typed sample payload (interpreted below)
    pub shot_boundary: u8,         // 0/1 marks a camera shot/cut start
    pub _pad: [u8; 2],
}
```

> **Safe payload, no `unsafe`.** Domain B is `forbid(unsafe_code)`, so the sample
> payload is a plain `value: [I16F16; 4]` (a `union KeyValue` would need an
> `unsafe` read — forbidden here). The four cells are reinterpreted by the owning
> `SequenceTrack.kind` / `Interp`; unused cells are padded with `0`:
>   - Transform tracks: position → `[tx, ty, tz, 0]`; rotation (quat, w last) →
>     `[qx, qy, qz, qw]`. Position (3) and rotation (4) never share one `value`
>     cell — they are **co-authored** as a position keyframe + a rotation
>     keyframe pair at the same tick, each stored in its own row.
>   - scale → `[sx, sy, sz, 0]`; Animation weight / Audio volume → `[v, 0, 0, 0]`;
>     Event arg → an `I16F16` scalar cell (a `u32` topic that fits `I16F16` is
>     stored as that scalar; a larger token is split across two cells).
> The **serialized** form (spec `16` raw column bytes + element_size) is the same
> as the union design; only the in-Rust view differs, and it stays 100% safe.

### Pure playback system

```rust
/// Deterministic per-tick sequencer evaluation. Pure: (&StateView) -> WorldDelta.
pub fn sequencer_system(view: &StateView<'_>) -> Result<WorldDelta, RecoverableError> {
    let mut delta = WorldDelta::default();
    // 1. Find active Sequence(s): those with playing == 1 (edit-world scrub
    //    drives the same system on the play world, spec 22).
    // 2. For each, compute local_tick = (view.tick - seq.start_tick)
    //    (looped via duration_ticks when loop_ == 1, else clamped).
    // 3. For each live SequenceTrack child (sorted by lane), gather its
    //    SequenceKeyframe children sorted by tick and evaluate at local_tick.
    // 4. Emit ColumnWrites:
    //      Transform tracks  → ColumnWrite over the target's Transform(2) column
    //      CinematicCamera    → ColumnWrite over target's Transform(2)+Camera(7)
    //      Audio              → DeferredCommand (host audio, no ECS pose)
    //      Event              → DeferredCommand::Emit { topic, data } at the tick
    // 5. If local_tick == sequence duration and not looping → clear playing.
    // Return delta; host applies at the flush boundary (spec 00).
    Ok(delta)
}
```

The renderer *never* evaluates keyframes itself. The recorded output is the
sequence of `WorldDelta`s / post-tick `WorldHash`es (spec `15`), which is the
single artifact that previews, plays, and renders all agree on.

## Components

The cinematic sequencer contributes three registered components. They sit in the
reserved **50–59** band declared by spec `21` and must never be renumbered or
recycled. This spec **does not** re-register `Transform` (2) or `Camera` (7); it
reuses them as write targets.

| `ComponentId` | Name                 | `size_of` (design) | semantic contract                          |
|---------------|----------------------|--------------------|--------------------------------------------|
| 50            | `Sequence`           | 32                 | root of one cut; ticks, loop, play state    |
| 51            | `SequenceTrack`      | 64                 | one lane: kind, target, weight, asset       |
| 52            | `SequenceKeyframe`   | ~40                | one typed sample at a tick                 |

IDs 53–56 belong to the UI system (spec `47`); 57–59 are reserved for future
cinematic/UI extras. Game/mod components ≥ 1024 stay free (spec `21`).

## Constraints

- **Determinism first.** All sequencing math is tick-based and fixed-point
  (`openengine-math`). No wall-clock, no `f32` in Domain B, no `HashMap`
  iteration (keyframes/tracks are kept sorted by tick/lane via `BTreeMap` or
  sorted `Vec`), no ambient RNG (AGENTS.md § 3).
- **Data, not a singleton.** A sequence, its tracks, and its keyframes are ECS
  components/entities in the normal archetype tables. There is no engine-global
  "sequencer" object with hidden state.
- **Reuse over invention.** Transform tracks write `Transform` (2); cinematic
  cameras are `Transform`+`Camera` (7) entities and cut by toggling `Camera.active`.
  No second camera/transform representation is added.
- **Domain B is pure.** `sequencer_system` returns a `WorldDelta`; the host
  applies it. Playback deltas are identical for identical (world, tick) inputs —
  this is what makes render reproducible and preview trustworthy.
- **Authoring is edit-world + spec-23 commands.** All timeline edits are
  undoable `Command`s against the edit world. Playback/preview runs against the
  play world and never mutates authored keyframes.
- **Keyframes as child entities.** Repeats the spec-21 multi-value idiom; each
  keyframe is one entity in exactly one archetype, and column writes stay
  contiguous.
- **Interpolation delegate (forward ref).** When specs 36/37/38 land, transform/
  animation evaluation reuses their samplers instead of duplicating spline code.
- **Headless-clean.** The tick evaluator needs no GPU, no window, no
  `wgpu::Device`; render/bake is the same loop plus a recorder. Portability
  (`x86_64-linux` / `aarch64-linux`) and offline CI hold.

## Performance Targets

- Per-keyframe evaluation: **< 50 ns** (bracketing search over sorted keys is
  O(log n) + a fixed-point blend; no allocation).
- Typical sequence (8 tracks × ~60 keyframes) per tick: **< 20 µs**.
- Sequence tick bookkeeping (playhead, loop, clamp): sub-microsecond, no alloc.
- Authoring command latency (add/move/delete keyframe): **< 1 ms** (one
  `ColumnWrite`/spawn/despawn, spec `23`).
- Recorded render: bounded by per-tick cost × tick count; recorder snapshots a
  post-tick `WorldHash` (< 1 ms for a modest world) per tick.

## Testing Strategy

All headless (no GPU / no window):

- **Keyframe bracket/interp:** constant/linear/smoothstep between authored
  fixed-point samples at t = before/between/after keys; assert exact fixed-point
  blends and no `f32`.
- **Loop/hold:** local_tick wraps on `loop_`, clamps on non-loop; playing clears
  at duration end.
- **Track evaluation correctness:** one transform track, verify the emitted
  `ColumnWrite` target (Transform, ComponentId 2) matches hand-computed pose at
  several ticks; cinematic-camera track asserts `Camera.active` transitions at
  shot boundaries.
- **Shot/cut/transition:** a hard cut toggles `active` in one tick; a transition
  window blends activation over N ticks.
- **Edit-world authoring via commands:** add/move/delete keyframes through
  spec-23 commands; undo/redo restores bit-identical keyframe columns; serialized
  history replays the same deltas (spec `16`/`23`).
- **Determinism:** run the same sequence 1000 ticks 3×; assert byte-identical
  post-tick `WorldHash`es each run (spec `15`).
- **Render = playback:** bake a sequence headlessly and replay the recorded
  deltas into a fresh world; assert final world equals the preview-world final
  `WorldHash` (same loop, proven equivalent).
- **Isolation:** while Playing, assert authored sequence data in the edit world is
  unchanged by playback deltas (spec `22`).
- **No time leak:** Domain-B sequencer unit tests fail to compile if any
  `std::time` symbol is referenced; purity gate reports `[PURE]`.

## Dependencies

- `contracts` (`StateView`, `WorldDelta`, `ColumnWrite`, `DeferredCommand`,
  `ComponentId`, `Entity`, `RenderKind`) — spec `00`.
- `openengine-math` (fixed-point interpolation).
- `crates/ecs` (`Transform`=2, `Camera`=7, `Parent`=4 layouts) — spec `21`.
- `crates/editor` — spec-23 undoable sequencing commands (spec `23`); viewport
  preview (spec `24`); edit-vs-play worlds (spec `22`).
- `openengine-serial` (`WorldSnapshot`, spec `16`) for persistence of authored
  sequences and baked renders; `WorldHash` (spec `15`) for render provenance.
- Forward: spec 36/37/38 skeletal-animation/state-machine samplers for
  animation-track delegation when they land.

## Next Steps

1. Register `Sequence`/`SequenceTrack`/`SequenceKeyframe` (50/51/52) and the
   `#[repr(C)]` `Pod` layouts in `crates/ecs/src/components.rs` with layout
   asserts (pattern of spec `21`).
2. Implement `bracket` + fixed-point interpolators and the pure
   `sequencer_system` (Domain B) with unit tests.
3. Implement transform/camera track evaluation writing `Transform`(2)/`Camera`(7)
   columns; add hard-cut and transition (blended active window) tests.
4. Add the spec-23 sequencing commands (`AddSequence`, `AddTrack`,
   `Add/Move/DeleteKeyframe`, `Move/ReorderTrack`) with inverse deltas.
5. Wire edit-world scrubbing and Play-in-Editor preview (spec `22`) to the same
   pure tick evaluator.
6. Implement headless render/bake recorder producing deterministic delta/log
   output + `WorldHash` provenance.
7. When 36/37/38 land, route animation tracks through their samplers.
