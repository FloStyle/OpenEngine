//! End-to-end "minimal game engine" proof, headless.
//!
//! Authored content lives in the repo's `examples/` scene; the pure logic lives
//! in `logic.wasm`. This test proves the full author→run loop on that real
//! example: without logic the scene is inert, with the guest the gameplay runs
//! (the chaser moves toward the player), and two runs are bit-identical.

use openengine_harness::runner;

const DEMO_SCENE: &str = concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../../examples/demo-chase.json"
);
const WASM_ASSET: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/../core/assets/logic.wasm");

fn chaser_x(report: &runner::RunReport) -> f32 {
    report.entities[1].transform[0]
}

#[test]
fn author_and_run_a_game_end_to_end() {
    // 1. Authored scene is inert without the logic module.
    let native = runner::run(DEMO_SCENE, None, 600).expect("native run");
    assert_eq!(native.entity_count, 3);
    assert_eq!(
        chaser_x(&native),
        10.0,
        "without logic the scene must not move"
    );

    if !std::path::Path::new(WASM_ASSET).exists() {
        eprintln!("SKIP: {WASM_ASSET} absent (run bash scripts/build.sh)");
        return;
    }

    // 2. With the guest logic, the gameplay runs: the chasing NPC moves toward
    //    the player (from x=10 toward the origin).
    let a = runner::run(DEMO_SCENE, Some(WASM_ASSET), 600).expect("guest run A");
    assert!(
        chaser_x(&a) < 5.0,
        "guest gameplay must move the chaser toward the player, got x={}",
        chaser_x(&a)
    );

    // 3. Determinism: a second run is bit-identical.
    let b = runner::run(DEMO_SCENE, Some(WASM_ASSET), 600).expect("guest run B");
    assert_eq!(a.hash, b.hash, "two guest runs of the same game must match");
    assert_eq!(a.entities, b.entities);
}
