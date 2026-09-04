# STATE.md — Global Project State

---
name: "Global State"
phase: "Phase 4→6 merged: headless ECS/gameplay + editor shell; wasm gameplay wired into Play"
updated: "2026-09-04"
---

# STATE.md

## Current Phase

The vertical slice is effectively end-to-end. Verified headless (workspace tests
green) and, for interactive parts, validated on the user's display:

1. **Domain B pure gameplay** (`logic-sandbox`, no_std + fixed-point, `[PURE]`):
   `movement_system`, `gameplay_tick` (WASD + jump + gravity, NPC
   wander/circle/chase), 12 unit tests.
2. **Wasm bridge (ADR-0001)**: `openengine_alloc` / `openengine_move_tick`
   (SoA movement) and `openengine_gameplay_tick` (full 3D gameplay). Host drivers
   live in `openengine-core` (`wasm_move_host`, `wasm_gameplay_host`).
3. **Editor shell** (`editor-shell`, egui 0.32 + wgpu 25, on `main`): Edit/Play,
   orbit/pan/zoom camera, WASD/Space, Blender-style lit scene (spheres,
   checkered ground, depth), GPU smoke tests.
4. **Play runs the real wasm logic**: shell `PlayBackend` loads
   `logic.wasm` lazily and ticks the guest at a fixed 60 Hz, applying each
   `WorldDelta` to the play world. Native placeholder only as a fallback if the
   module is absent. Guest == native proven bit-for-bit (`gameplay_wasm_determinism`).

## Active work

| area | status |
|------|--------|
| Headless pipeline (start / develop+test / edit-save / player-run / package) | ✅ merged on `main` (see Completed) |
| Windowed player / render the packaged game | todo (needs your display) |
| Wire editor-shell to open/edit a real saved scene file (not the hardcoded demo) | todo (windowed) |
| Hot-reload of `logic.wasm` (spec 10) | todo |
| Swappable 2nd demo logic module (prove engine is logic-agnostic) | todo |
| Editor: gizmo translate / drag-select (spec 09/31) | todo |
| Asset pipeline (Phase 5): textures/meshes/cache | todo |
| STATE.md / ROADMAP kept current | ongoing |

## Completed (headless-verified unless noted)

- Phases 1–3: scaffold, living window (ABI v2), SoA memory bridging (100% safe).
- Phase A–E headless gameplay + physics (committed).
- Phase 6 core + shell: `crates/editor` headless core, `crates/editor-shell`
  interactive editor merged into `main` (`e4ebdbc` → `c9bc0c2`).
- Wasm gameplay bridge + determinism tests (guest == native; 3× deterministic).
- **Harness core** (`crates/harness`, in `main`): headless JSON-over-HTTP live
  surface — `/observe /spawn /despawn /set /tick /hash /load_wasm /prove
  /transaction /save /load`.
- **Full headless game pipeline (in `main`)**:
  - develop+test: `/prove` (determinism PASS/FAIL), `/transaction` (atomic, rollback);
  - edit scene: versioned `/save` `/load` (bit-for-bit world round-trip);
  - player: `openengine-runner` runs a scene + `logic.wasm` headless;
  - package: `scripts/package.sh` → runnable `dist/<game>/`;
  - proof: `full_pipeline` test authors `examples/demo-chase.json` → runs it →
    the guest chaser moves toward the player → two runs bit-identical.


## Important Notes

- All Domain B code must pass purity verification
  (`python3 brain/orchestrator.py verify-wasm-purity`).
- Memory bridging is **100% safe**: the guest allocates, the host writes via
  `Memory::write`, the guest reads its own `Vec`. No raw pointers, no `unsafe`
  in logic. Exports (`#[no_mangle]`) live in `crates/logic-export`, never in
  `logic-sandbox` (which is `forbid(unsafe_code)`).
- `logic.wasm` is a gitignored build artifact: `bash scripts/build.sh` rebuilds it.
- Single mutation channel: all writes flow through `WorldDelta` → `apply_delta`.
- Test protocol is mandatory for every task.
