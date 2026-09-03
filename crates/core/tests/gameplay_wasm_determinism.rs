//! Phase E — full 3D gameplay executed INSIDE the wasm guest via
//! `openengine_gameplay_tick`, headless. Proves the guest pipeline is
//! bit-identical to the native `gameplay_tick` over the same deterministic
//! input sequence, and identical across 3 guest runs.

use openengine_contracts::{Actor, InputState3D, Transform, Velocity3D};
use openengine_core::wasm_gameplay_host::WasmGameplayHost;
use openengine_ecs::{Color, Position, Velocity, World};
use openengine_math::I16F16 as F;

const WASM_ASSET: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/assets/logic.wasm");

/// A player at the origin + wander (kind 1) and chase (kind 3) NPCs.
fn create_scene() -> World {
    let mut w = World::new();
    // Player (entity 0).
    let p = w.spawn(
        Position {
            x: F::from_num(0),
            y: F::from_num(0),
        },
        Velocity {
            x: F::from_num(0),
            y: F::from_num(0),
        },
        Color {
            r: 0,
            g: 255,
            b: 0,
            a: 255,
        },
    );
    w.set_transform(p, tf(0, 0, 0));
    w.set_velocity_3d(p, Velocity3D::zero());
    w.set_actor(p, Actor::player(F::from_num(2), F::from_num(20)));
    // 9 NPCs: alternate wander (1) / chase (3) for behavioural coverage.
    for i in 0..9 {
        let idx = w.spawn(
            Position {
                x: F::from_num(0),
                y: F::from_num(0),
            },
            Velocity {
                x: F::from_num(0),
                y: F::from_num(0),
            },
            Color {
                r: 200,
                g: 60,
                b: 60,
                a: 255,
            },
        );
        w.set_transform(idx, tf((i + 1) * 4, 0, (i % 3) * 3));
        w.set_velocity_3d(idx, Velocity3D::zero());
        w.set_actor(
            idx,
            Actor::npc(if i % 2 == 0 { 1 } else { 3 }, (i as u32 + 1) * 7919),
        );
    }
    w
}

fn tf(x: i32, y: i32, z: i32) -> Transform {
    Transform::at(F::from_num(x), F::from_num(y), F::from_num(z))
}

/// Input script: forward for the first 150 ticks, then a jump, then idle.
fn input_for(tick: u64) -> InputState3D {
    if tick < 150 {
        InputState3D {
            forward: 1,
            ..InputState3D::none()
        }
    } else if tick == 150 {
        InputState3D {
            jump: 1,
            ..InputState3D::none()
        }
    } else {
        InputState3D::none()
    }
}

/// Native gameplay stepping (mirrors exactly what the guest does).
fn simulate_native(world: &mut World, ticks: u64) {
    for f in 0..ticks {
        let n = world.entity_count();
        let t = world.get_transforms().unwrap()[..n].to_vec();
        let v = world.get_velocity_3d().unwrap()[..n].to_vec();
        let a = world.get_actors().unwrap()[..n].to_vec();
        let delta = openengine_logic_sandbox::gameplay_tick(f, &t, &v, &a, &input_for(f)).unwrap();
        world.apply_delta(&delta);
    }
}

/// Guest gameplay stepping via the wasm host.
fn simulate_wasm(world: &mut World, host: &mut WasmGameplayHost, ticks: u64) -> anyhow::Result<()> {
    for f in 0..ticks {
        host.set_input(input_for(f));
        let delta = host.tick(world, f)?;
        world.apply_delta(&delta);
    }
    Ok(())
}

#[test]
fn gameplay_wasm_matches_native() {
    // Load the guest once; if the asset is absent, skip (not a logic failure).
    let Ok(mut host) = WasmGameplayHost::load(WASM_ASSET) else {
        eprintln!("SKIP: logic.wasm not present ({WASM_ASSET})");
        return;
    };
    let mut wasm_world = create_scene();
    let mut native_world = create_scene();

    simulate_wasm(&mut wasm_world, &mut host, 600).expect("guest tick");
    simulate_native(&mut native_world, 600);

    let wt = wasm_world.get_transforms().unwrap();
    let nt = native_world.get_transforms().unwrap();
    assert_eq!(
        wt, nt,
        "guest (wasm) transforms must match native gameplay_tick bit-for-bit"
    );
    let wa = wasm_world.get_actors().unwrap();
    let na = native_world.get_actors().unwrap();
    assert_eq!(
        wa, na,
        "guest actor state (grounded/jump_cd) must match native"
    );
    let wv = wasm_world.get_velocity_3d().unwrap();
    let nv = native_world.get_velocity_3d().unwrap();
    assert_eq!(wv, nv, "guest velocity must match native");
}

#[test]
fn gameplay_wasm_deterministic_3x() {
    let Ok(_host) = WasmGameplayHost::load(WASM_ASSET) else {
        eprintln!("SKIP: logic.wasm not present ({WASM_ASSET})");
        return;
    };
    // Three separate guest worlds run through their own host instances.
    let run = || -> u64 {
        let mut host = WasmGameplayHost::load(WASM_ASSET).unwrap();
        let mut w = create_scene();
        simulate_wasm(&mut w, &mut host, 400).unwrap();
        w.hash()
    };
    let h1 = run();
    let h2 = run();
    let h3 = run();
    assert_eq!(h1, h2, "guest runs 1 and 2 must be bit-identical");
    assert_eq!(h2, h3, "guest runs 2 and 3 must be bit-identical");
}
