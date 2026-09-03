//! Headless "player" test: build a scene file, then run it through the runner
//! (native and, when logic.wasm is present, via the guest). Verifies the
//! scene→tick→report pipeline and that two runs are bit-identical.

use openengine_harness::runner::{self, RunReport};
use openengine_harness::state::SceneFile;
use openengine_harness::HarnessState;

const WASM_ASSET: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/../core/assets/logic.wasm");

fn write_scene_file(label: &str) -> (String, u64) {
    let mut s = HarnessState::new();
    s.spawn([0.0, 0.0, 0.0], [1.0, 1.0, 1.0], [255, 0, 0, 255]);
    s.spawn([3.0, 0.0, 2.0], [1.0, 1.0, 1.0], [0, 255, 0, 255]);
    s.spawn([-4.0, 1.0, 0.0], [2.0, 1.0, 1.0], [0, 0, 255, 255]);
    let scene: SceneFile = s.export_scene();
    let path = std::env::temp_dir().join(format!(
        "openengine_{label}_scene_{}.json",
        std::process::id()
    ));
    let p = path.to_str().unwrap().to_string();
    std::fs::write(&p, serde_json::to_vec(&scene).unwrap()).unwrap();
    (p, scene.tick)
}

#[test]
fn runner_plays_scene_native_and_is_deterministic() {
    let (path, _) = write_scene_file("native");
    let a: RunReport = runner::run(&path, None, 120).expect("native run A");
    let b: RunReport = runner::run(&path, None, 120).expect("native run B");
    assert_eq!(a.hash, b.hash, "two native runs must match");
    assert_eq!(a.entity_count, 3);
    assert_eq!(a.tick, 120);
    let _ = std::fs::remove_file(&path);
}

#[test]
fn runner_plays_scene_with_guest_and_is_deterministic() {
    if !std::path::Path::new(WASM_ASSET).exists() {
        eprintln!("SKIP: {WASM_ASSET} absent (run bash scripts/build.sh)");
        return;
    }
    let (path, _) = write_scene_file("guest");
    let a: RunReport = runner::run(&path, Some(WASM_ASSET), 300).expect("guest run A");
    let b: RunReport = runner::run(&path, Some(WASM_ASSET), 300).expect("guest run B");
    assert_eq!(a.hash, b.hash, "two guest runs must be bit-identical");
    assert_eq!(a.entity_count, 3);
    let _ = std::fs::remove_file(&path);
}
