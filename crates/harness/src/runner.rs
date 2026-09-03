//! Headless game **runner/player** (spec-50 build/deploy primitive).
//!
//! A "game" is a saved [`SceneFile`] (authored content) + a `logic.wasm`
//! module (the pure Domain-B logic). This module loads both into a
//! [`HarnessState`] and advances the deterministic sim for `frames` ticks,
//! returning a compact report. It is the headless step a cook/package pipeline
//! runs before shipping, and it needs no GPU/editor.

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

/// Load `scene_path` (a `SceneFile` JSON), optionally load `wasm` logic, tick
/// `frames` times deterministically, and return a compact report.
pub fn run(scene_path: &str, wasm: Option<&str>, frames: u64) -> Result<RunReport, String> {
    let bytes = std::fs::read(scene_path).map_err(|e| format!("read scene {scene_path}: {e}"))?;
    let scene: SceneFile =
        serde_json::from_slice(&bytes).map_err(|e| format!("bad scene json: {e}"))?;

    let mut state = HarnessState::new();
    state.import_scene(&scene)?;
    if let Some(wasm_path) = wasm {
        state.load_wasm(wasm_path)?;
    }
    state.tick_n(frames)?;

    let n = state.entity_count();
    let tick = state.tick();
    let (entities, _) = state.observe(n);
    Ok(RunReport {
        entity_count: n,
        tick,
        hash: hex_hash(state.hash()),
        entities,
    })
}
