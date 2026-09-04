//! Headless game **runner/player** (spec-50 build/deploy primitive).
//!
//! A "game" is a saved [`SceneFile`] (authored content) + a `logic.wasm`
//! module (the pure Domain-B logic). This module loads both into a
//! [`HarnessState`] and advances the deterministic sim for `frames` ticks,
//! returning a compact report. It is the headless step a cook/package pipeline
//! runs before shipping, and it needs no GPU/editor.

use openengine_contracts::InputState3D;

use crate::state::{EntityView, HarnessState, SceneFile};

/// Result of a headless play run.
#[derive(serde::Serialize)]
pub struct RunReport {
    pub entity_count: usize,
    pub tick: u64,
    pub hash: String,
    pub entities: Vec<EntityView>,
}

fn hex_hash(h: u64) -> String {
    format!("{h:016x}")
}

fn report_from(state: &HarnessState) -> RunReport {
    let n = state.entity_count();
    let tick = state.tick();
    let (entities, _) = state.observe(n);
    RunReport {
        entity_count: n,
        tick,
        hash: hex_hash(state.hash()),
        entities,
    }
}

/// Load `scene_path` (a `SceneFile` JSON), optionally load `wasm` logic, tick
/// `frames` times deterministically, and return a compact report. Pure input is
/// provided per frame by `input_at(i)` (input as data → the guest).
pub fn run_with(
    scene_path: &str,
    wasm: Option<&str>,
    frames: u64,
    input_at: impl Fn(u64) -> InputState3D,
) -> Result<RunReport, String> {
    let bytes = std::fs::read(scene_path).map_err(|e| format!("read scene {scene_path}: {e}"))?;
    let scene: SceneFile =
        serde_json::from_slice(&bytes).map_err(|e| format!("bad scene json: {e}"))?;

    let mut state = HarnessState::new();
    state.import_scene(&scene)?;
    if let Some(wasm_path) = wasm {
        state.load_wasm(wasm_path)?;
    }
    for i in 0..frames {
        state.set_input(input_at(i));
        state.tick_n(1)?;
    }
    Ok(report_from(&state))
}

/// Run with no input.
pub fn run(scene_path: &str, wasm: Option<&str>, frames: u64) -> Result<RunReport, String> {
    run_with(scene_path, wasm, frames, |_| InputState3D::none())
}

/// One authored input event at `tick`: the held key state from that frame on
/// until the next event. Fields default to 0 (key released).
#[derive(Clone, Debug, Default, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(default)]
pub struct FrameInput {
    /// Simulation frame this event applies from.
    pub tick: u64,
    pub forward: u8,
    pub backward: u8,
    pub left: u8,
    pub right: u8,
    pub jump: u8,
}

fn to_input(f: &FrameInput) -> InputState3D {
    InputState3D {
        forward: f.forward,
        backward: f.backward,
        left: f.left,
        right: f.right,
        jump: f.jump,
        ..InputState3D::none()
    }
}

/// Deterministically replay an authored input script (a sorted list of
/// [`FrameInput`] events; the held state persists until the next event).
/// Two identical scripts ⇒ bit-identical result — the basis for replay/testing
/// a game's logic changes.
pub fn run_script(
    scene_path: &str,
    wasm: Option<&str>,
    frames: u64,
    events: &[FrameInput],
) -> Result<RunReport, String> {
    let bytes = std::fs::read(scene_path).map_err(|e| format!("read scene {scene_path}: {e}"))?;
    let scene: SceneFile =
        serde_json::from_slice(&bytes).map_err(|e| format!("bad scene json: {e}"))?;
    let mut state = HarnessState::new();
    state.import_scene(&scene)?;
    if let Some(wasm_path) = wasm {
        state.load_wasm(wasm_path)?;
    }
    let mut sorted: Vec<&FrameInput> = events.iter().collect();
    sorted.sort_by_key(|e| e.tick);
    let mut ei = 0usize;
    let mut cur = InputState3D::none();
    for i in 0..frames {
        while ei < sorted.len() && sorted[ei].tick <= i {
            cur = to_input(sorted[ei]);
            ei += 1;
        }
        state.set_input(cur);
        state.tick_n(1)?;
    }
    Ok(report_from(&state))
}

/// Run holding "forward" for the first `forward_ticks` frames — a scripted,
/// deterministic "walk forward" demo so a packaged game is actually playable.
pub fn run_forward(
    scene_path: &str,
    wasm: Option<&str>,
    frames: u64,
    forward_ticks: u64,
) -> Result<RunReport, String> {
    let fwd = InputState3D {
        forward: 1,
        ..InputState3D::none()
    };
    run_with(scene_path, wasm, frames, |i| {
        if i < forward_ticks {
            fwd
        } else {
            InputState3D::none()
        }
    })
}
