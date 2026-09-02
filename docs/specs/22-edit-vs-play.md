---
spec: "22-edit-vs-play"
phase: "Phase 5: Editor"
status: "draft"
author: "OpenEngine AI"
created: "2026-09-03"
depends_on:
  - "00-ecs-architecture"
  - "01-game-loop"
  - "05-time-system"
  - "06-scene-management"
  - "07-editor-inspector"
  - "08-editor-hierarchy"
  - "09-editor-gizmos"
  - "16-serialization"
  - "21-primitive-components"
---
# 22 - Edit vs. Play Mode

## Overview

OpenEngine keeps **two distinct worlds** so that editing is safe and play is
deterministic:

- **Edit world** — the persistent *source-of-truth* scene. It is what the editor
  hierarchy (spec 08), inspector (spec 07) and gizmos (spec 09) mutate, and it
  is the world anchored on disk (spec 06/16). **No gameplay systems run in Edit
  mode.** It contains authored, mostly-authoring state: initial transforms,
  static spawns, prefab roots, tags, and the camera rig.
- **Play world** — an ephemeral, **deterministic deep clone** of the edit world
  created exactly when the user presses Play. Gameplay (Domain-B pure systems,
  spec 01/12) runs only here, driven by the host ticker. Nothing the player or
  the simulation does may reach back into the edit world.

The two worlds are **fully isolated** — separate allocations, separate
archetype tables, no shared memory. The edit world is the rollback anchor: when
Play exits, the play world is destroyed and the edit world is restored exactly
as it was (undo history preserved and untouched). This is Unreal-style
Play-in-Editor / Exit-to-Edit behavior re-expressed under OpenEngine's
constraints (determinism, no shared sim memory, pure Domain B).

Pause freezes the play world and enables **read-only inspection** (the
inspector shows play state but disables mutation). Step advances exactly one
tick then re-pauses. A speed control (0.25×/0.5×/1×/2×/4×) scales how fast host
wall-clock time advances simulated ticks, **never** the fixed-timestep math
(spec 01/05).

## Core Concepts

### EditorMode state machine

Play-in-editor is a small, explicit state machine that lives in
`EditorState` (below) inside `crates/editor` (Domain A). Transitions are
synchronous and happen on the main/editor thread; no gameplay runs during a
transition.

```text
        ┌─────────┐   Play (deep clone)    ┌──────────┐
        │   Edit  │ ─────────────────────▶ │  Playing │◀───────────┐
        └─────────┘                        └──────────┘            │
            ▲                                        │             │
            │                                        │ Pause       │ Step (tick+1, then pause)
            │ Exit (destroy play,                   ▼             │
            │  restore edit)                    ┌──────────┐       │
            └───────────────────────────────────│  Paused  │───────┘
                                               └──────────┘
```

The legal transitions:

| From      | To        | Guard / action                                                        |
|-----------|-----------|------------------------------------------------------------------------|
| Edit      | Playing   | Snapshot edit world → deep-clone → sim starts at `play_start_tick`.    |
| Playing   | Paused    | Freeze the play world; keep its `tick` and buffers intact.             |
| Paused    | Playing   | Resume ticking from the paused tick (state unchanged).                |
| Paused    | Edit      | Exit: destroy play world; restore edit world unchanged.               |
| Playing   | Edit      | Same as Paused→Edit (allowed; an implicit pause+exit).                |
| Edit      | Paused    | Not allowed (must go Edit→Playing first).                            |

"Play-in-Editor" is the only mode in scope for this spec: while Playing/Paused,
the editor window is still shown but its **mutating** controls are disabled and
its viewport shows the live play world (spec 09 gizmos turn into read-only or
game-input overlay). **Standalone** (the shipping game run outside the editor,
spec 01 `GameLoop`) is out of scope here: it boots straight into a single
simulating world with no edit twin. This spec only guarantees that Standalone
never touches EditorState. Both worlds share the same `World`/codec types, so
Play-in-Editor exercises the exact simulation path Standalone uses.

### Entering Play = deterministic deep clone

`Edit → Playing` performs a **snapshot-and-rehydrate deep clone** of the edit
world, using the spec-16 `WorldSnapshot` format serialized with `postcard`:

1. **Capture** the edit world into a `WorldSnapshot` (archetypes sorted by id,
   columns sorted by component id, entities in slot order) and immediately hash
   it (`WorldHash`, spec 15) as the *clone source fingerprint*.
2. **Deserialize** into a fresh `World` allocation — this is the play world. It
   has its own `ComponentRegistry` table, own archetype storage, own entity
   generations. Because the codec round-trips raw Pod column bytes and regenerates
   deterministic entity handles (spec 06/16), the rehydrated play world is a
   **bit-identical clone** of the edit world *at the moment of capture* (same
   `WorldHash`).
3. Record `play_start_tick = current host tick`.
4. Hand the play world to the `GameLoop`/sandbox driver (spec 01) as the active
   simulating world.

The clone is deliberately a **serialize/deserialize round-trip**, not a
`memcpy`/deep pointer clone: serialization is what guarantees (a) no shared
memory between the twins, (b) a deterministic, platform-independent copy (spec
16's byte-identity), and (c) that the clone passes through the *same* codec the
save/load and networking rollback paths use — one code path, audited once. A raw
`Vec` clone would risk sharing interned IDs or asset handles; the codec is the
trusted isolation boundary.

### Exiting Play = restore edit world exactly

`→ Edit` destroys the play world and leaves the **edit world byte-for-byte as it
was before Play was pressed**. Implementation notes:

- The edit world is **never mutated** during Playing/Paused. Gameplay deltas are
  applied to the play world only. So "restore" is trivial: the edit world was
  already untouched — we just destroy the play world and its driver.
- Any **play-time capture** the user chose to keep (e.g. an edited transform they
  want to copy back) is an explicit, separate user action ("Save to Edit"),
  *not* automatic. That action is a normal edit-world mutation through the
  editor's deterministic channel (spec 07) and is recorded in the undo history.
- **Undo history is anchored in the edit world only** and is *never* touched by
  Play. Entering/leaving Play adds **no** undo entries; the edit undo stack
  remains identical before and after a play session.

### Pause / Step / Speed

- **Pause** (`Playing → Paused`): stop advancing the fixed tick. The play world,
  its current `tick`, the last applied `WorldDelta`, and the input buffer are all
  kept intact. The inspector is placed in read-only mode (it may *show* live play
  component values from the play `StateView` but cannot emit edits).
- **Step** (`Paused`, button): advance exactly one fixed tick against the play
  world, then return to Paused. This is used to single-step Domain-B systems for
  debugging (spec 11).
- **Speed** (0.25×…4×): scales how host wall-clock time maps onto tick
  advancement. It changes **when** the fixed timestep fires, never the timestep
  math itself. At 0.5× the editor advances the play world half as fast as real
  time; the per-tick delta each simulation produces is identical regardless of
  speed (determinism preserved — spec 05). Speed does not resample or extrapolate
  state; it only paces tick dispatch.

## Key Rust Types

```rust
//! crates/editor/src/modes.rs — Domain A. All types std + egui-free logic
//! (the mode state machine is testable headless; UI wraps it).

use contracts::Entity;
use contracts::WorldDelta;
use openengine_ecs::World;         // Domain-A ECS world (spec 00)
use openengine_math::I16F16;

/// Where the editor is in its lifecycle. Standalone (non-editor) never holds
/// this enum; it runs a single simulating world via spec 01 directly.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum EditorMode {
    /// Authoring the source-of-truth scene. No gameplay systems run.
    Edit,
    /// Play world is live and ticking.
    Playing,
    /// Play world is frozen; read-only inspection / stepping.
    Paused,
}

/// A replay of a play frame is not part of mode state — see `EditorTransition`.

/// The full editor lifecycle state. Owns the edit world (always) and,
/// optionally, the live play world.
pub struct EditorState {
    /// Current lifecycle mode.
    pub mode: EditorMode,
    /// The persistent source-of-truth scene world. Never simulated.
    pub edit_world: World,
    /// The live play world; `None` unless Playing/Paused. Fully isolated.
    pub play_world: Option<World>,
    /// Host tick at which the current play session began (edit world's tick is
    /// unrelated). Used for stats + deterministic stepping.
    pub play_start_tick: u64,
    /// Current play tick (== how many Domain-B ticks this session ran).
    pub play_current_tick: u64,
    /// Wall-clock pacing multiplier for tick dispatch: 0.25/0.5/1/2/4.
    pub speed_multiplier: SpeedMultiplier,
    /// Optional read-only view of the play world handed to the inspector while
    /// Paused. (Built from `play_world` each pause; never a stored &mut.)
    // pub inspection: Option<&World>,   // conceptual — see note below
}

/// Speed control. Discrete, deterministic-friendly.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SpeedMultiplier {
    Quarter = 0,
    Half = 1,
    Normal = 2,   // 1×
    Double = 3,   // 2×
    Quad = 4,     // 4×
}

/// The only sanctioned way to change EditorMode. Every transition is expressed
/// as one of these so the transition logic stays testable and total.
pub enum EditorTransition {
    Play,      // Edit → Playing
    Pause,     // Playing → Paused
    Resume,    // Paused → Playing
    Step,      // Paused → (tick once) → Paused
    ExitToEdit,// {Playing,Paused} → Edit
}

impl EditorState {
    /// Play the editor's current scene: deep-clone edit_world into play_world.
    pub fn enter_play(&mut self) -> Result<(), ModeError> {
        debug_assert_eq!(self.mode, EditorMode::Edit);
        let snap = openengine_serial::snapshot(&self.edit_world)?;   // spec 16
        let clone = openengine_serial::restore(&snap)?;              // deep clone
        self.play_world = Some(clone);
        self.play_start_tick = self.host_tick();
        self.play_current_tick = 0;
        self.mode = EditorMode::Playing;
        Ok(())
    }

    /// Run one Domain-B tick on the play world (Playing only).
    pub fn advance_play_tick(&mut self) -> Result<(), ModeError> {
        let play = self.play_world.as_mut()
            .ok_or(ModeError::NoPlayWorld)?;
        let delta: WorldDelta = openengine_sandbox::run_systems(play)?; // spec 01
        play.apply_delta(&delta)?;                                       // spec 00
        self.play_current_tick += 1;
        Ok(())
    }

    /// Freeze the play world (keeps state). Edit world untouched.
    pub fn pause(&mut self) {
        debug_assert_eq!(self.mode, EditorMode::Playing);
        self.mode = EditorMode::Paused;
    }

    /// Step exactly one tick, then remain Paused.
    pub fn step(&mut self) -> Result<(), ModeError> {
        if self.mode != EditorMode::Paused {
            return Err(ModeError::NotPaused);
        }
        self.advance_play_tick()?;
        self.mode = EditorMode::Paused; // re-pause after one tick
        Ok(())
    }

    /// Exit Play: destroy the play world, leaving the edit world untouched.
    pub fn exit_to_edit(&mut self) {
        if self.mode == EditorMode::Edit {
            return; // idempotent no-op
        }
        self.play_world = None;   // drop → free isolated allocation
        self.mode = EditorMode::Edit;
        // Undo history, edit_world: untouched by design.
    }
}

/// Recoverable mode errors (host-typed, mirrors RecoverableError semantics).
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ModeError {
    NoPlayWorld,     // step/advance without a live play world
    NotPaused,       // step requested outside Paused
    NotPlaying,      // pause requested outside Playing
    CloneFailed,     // snapshot/restore codec error
}
```

> The inspector's read-only Paused view never stores `&mut World`. When Paused,
> a fresh `StateView` (spec 00/07 `WorldView`) is built from `play_world` for the
> UI frame; mutating panels are disabled by checking `self.mode != Edit`. See
> spec 07 for how edits are gated to Edit mode only.

### Speed pacing (does not resample)

The host ticker (spec 01/05) is a fixed-timestep accumulator. Speed is applied
only to the *dispatch cadence*:

```rust
/// Fixed timestep in wall-clock terms (simpler restatement of spec 01/05).
/// `speed_multiplier` scales how often the accumulator fires a tick — never the
/// tick's content.
fn advance_wall(&mut self, dt_real: I16F16) {
    // accumulate real dt scaled by speed; when accumulator >= fixed_step → tick
    self.accumulator += dt_real * self.speed_scale();   // 0.25..4.0
    while self.accumulator >= FIXED_STEP {
        self.accumulator -= FIXED_STEP;
        let _ = self.advance_play_tick();
    }
}
```

`I16F16` here is used only to scale pacing on the host; the Domain-B simulation
never sees wall-clock `dt`. Determinism holds because each tick still produces
the same `WorldDelta` for the same `StateView` regardless of pacing.

## Constraints

- **Two fully isolated worlds.** Edit and play never share memory: no shared
  `ComponentRegistry` entries held by both, no interned handle reused across
  them, no `Arc<Rc>` pointing into the other. The codec round-trip is the
  isolation boundary (spec 16).
- **Play is a deterministic deep clone.** Rehydrating from `WorldSnapshot` +
  `postcard` yields a `WorldHash`-identical twin. No `HashMap` iteration order,
  no wall-clock, no ambient randomness (AGENTS.md § 3).
- **No gameplay systems in Edit mode.** Domain-B systems run against the play
  world only. The edit world is authored state; simulating it would violate the
  source-of-truth invariant.
- **Edit world is never mutated by play.** All deltas target the play world.
  Exiting always restores the (untouched) edit world.
- **Undo history anchored in the edit world only**, preserved across play
  sessions (never added-to, never cleared, by Play).
- **Pause = freeze + read-only.** Paused play state is inspectable but not
  editable; edits are gated to `mode == Edit`.
- **Step advances exactly one tick** then returns to Paused — no drift into
  free-running.
- **Speed scales tick dispatch cadence only**; it never changes fixed timestep,
  timestep math, or per-tick deltas.
- Determinism, portability (`x86_64-linux`/`aarch64-linux`), headless tests (no
  GPU, no window), Docker/CI/offline all hold — the mode machine and clone path
  are pure enough to test without any editor UI.

## Performance Targets

- **Entering Play (clone):** deep clone of a 10k-entity world via snapshot +
  rehydrate < **100 ms** (dominated by postcard encode/decode + SoA alloc).
- **Mode transitions** (`Play`, `Pause`, `Resume`, `ExitToEdit`): < **50 ms**
  total overhead (no per-entity work for pause/exit; clone only on Enter).
- **Paused stepping** cost == one tick cost (no extra state work).
- **Speed pacing** adds negligible per-frame cost (an `I16F16` accumulator
  multiply + compare); it never re-serializes.
- Per-frame overhead in Edit mode: zero (no play world allocated).

## Testing Strategy

- **Clone fidelity (bit-identical):** build a seeded edit world (all primitive
  archetypes from spec 21), `enter_play`, and assert `edit_world.world_hash() ==
  play_world.world_hash()` and every column byte-identical to the snapshot.
- **Edit untouched by play:** edit→play→mutate play heavily→exit→assert the edit
  world's `WorldHash` is unchanged and equals its pre-play hash.
- **Exit preserves undo:** push several undo entries in Edit, play/exit, assert
  the undo stack length and top entry are identical.
- **Deterministic replay:** from the same edit world, play 1000 ticks and record
  the final play `WorldHash`; repeat twice more; assert all three hashes are
  identical.
- **Exit-from-anywhere:** exit from Playing and from Paused both land in Edit
  with the edit world restored; `play_world` is `None`.
- **Step semantics:** play→pause→step exactly once→assert `play_current_tick`
  advanced by 1 and mode is Paused; a second step advances to +2.
- **Pause is read-only:** while Paused, attempt an inspector edit through spec
  07's channel and assert it is rejected (the mode gate blocks it) and the play
  world hash is unchanged.
- **Speed invariance:** run the same 1000 ticks at 0.25× and 4×; assert the
  resulting play worlds are byte-identical (pacing did not affect sim).
- **Mode-machine fuzz:** generate random sequences of
  `Play/Pause/Resume/Step/ExitToEdit` and assert (a) every transition obeys the
  legal table above (illegal ones are rejected, never corrupt state) and (b) the
  invariant "edit world hash constant unless an explicit save-to-edit occurs"
  holds throughout.
- **Headless:** all of the above run with no GPU/window (pure `World` + codec).

## Dependencies

- `crates/editor` (owns `EditorState`, `EditorMode`, transitions).
- `crates/ecs` (`World`, `apply_delta`) — spec 00.
- `openengine-serial` (`WorldSnapshot`, `snapshot`/`restore`, `postcard`) —
  spec 16 (the deep-clone mechanism).
- `crates/core` / `crates/logic-sandbox` (`run_systems`, fixed ticker) —
  spec 01/05/12.
- `openengine-math` (`I16F16` for speed pacing only).
- Inspector/hierarchy/gizmos (spec 07/08/09) gate their mutating UIs off this
  spec's `mode == Edit`.
- Primitive component layouts from spec 21 for the seed-world clone tests.

## Next Steps

1. Add `EditorMode`, `EditorState`, `SpeedMultiplier`, `ModeError` to
   `crates/editor` with the transition methods above.
2. Confirm `openengine-serial` exposes `snapshot(&World) -> WorldSnapshot` and
   `restore(&WorldSnapshot) -> World` and add a `world_hash()` for clones.
3. Wire the fixed-ticker (spec 01/05) to drive `play_world` only, with
   `advance_play_tick` / Step and speed pacing.
4. Gate spec 07/08/09 mutating UIs on `mode == Edit`; make inspector read-only
   (view of play `StateView`) while Paused.
5. Implement the full mode-machine + fuzz + determinism test battery
   (headless), and hook Play/Exit buttons to `EditorTransition`.
