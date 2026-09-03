//! PoC Phase 3 — movement executed INSIDE the wasm guest via the SoA bridge,
//! headless. Proves the guest reads columns through StateView descriptors,
//! returns a WorldDelta, and that the full wasm pipeline is bit-identical to
//! the native pipeline and across 3 runs.

use openengine_core::native_movement::native_movement_system;
use openengine_core::wasm_move_host::WasmMoveHost;
use openengine_ecs::{Color, Position, Velocity, World};
use openengine_math::I16F16;
use std::path::Path;

const WASM_ASSET: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/assets/logic.wasm");

fn create_test_world() -> World {
    let mut world = World::new();
    for i in 0..100 {
        let x = (i % 10) * 50;
        let y = (i / 10) * 50;
        world.spawn(
            Position { x: I16F16::from_num(x), y: I16F16::from_num(y) },
            Velocity { x: I16F16::from_num(5), y: I16F16::from_num(5) },
            Color { r: 255, g: 0, b: 0, a: 255 },
        );
    }
    world
}

fn simulate_native(world: &mut World, ticks: usize) {
    for _ in 0..ticks {
        let delta = native_movement_system(world);
        world.apply_delta(&delta);
    }
}

fn simulate_wasm(world: &mut World, host: &mut WasmMoveHost, ticks: usize) -> Result<(), anyhow::Error> {
    for _ in 0..ticks {
        let delta = host.tick(world)?;
        world.apply_delta(&delta);
    }
    Ok(())
}

#[test]
fn wasm_movement_matches_native_and_is_deterministic() {
    if !Path::new(WASM_ASSET).exists() {
        eprintln!("logic.wasm not found — run bash scripts/build.sh first");
        return;
    }

    // Native reference.
    let mut native = create_test_world();
    simulate_native(&mut native, 100);

    // Three identical wasm runs.
    let mut h = [0u64; 3];
    for slot in h.iter_mut() {
        let mut world = create_test_world();
        let mut host = WasmMoveHost::load(WASM_ASSET).expect("load wasm");
        simulate_wasm(&mut world, &mut host, 100).expect("wasm simulation");
        *slot = world.hash();
    }

    // Wasm is bit-identical across runs.
    assert_eq!(h[0], h[1], "wasm run 1 vs 2");
    assert_eq!(h[1], h[2], "wasm run 2 vs 3");

    // Wasm guest movement == native movement (same math, same bridge target).
    assert_eq!(h[0], native.hash(), "wasm must match native movement byte-for-byte");
}
