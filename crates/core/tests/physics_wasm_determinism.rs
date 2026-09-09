//! Domain-B physics runs in the wasm guest (`openengine_physics_tick`) and is
//! bit-identical to the native `physics_delta`, and deterministic across runs.

use openengine_contracts::{Actor, Transform, Velocity3D};
use openengine_core::wasm_physics_host::{PhysicsParams as HostParams, WasmPhysicsHost};
use openengine_ecs::{Color, Position, Velocity, World};
use openengine_logic_sandbox::PhysicsParams;
use openengine_math::I16F16 as F;

const WASM_ASSET: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/assets/logic.wasm");

fn build() -> World {
    let mut w = World::new();
    let z = F::from_num(0);
    for (i, y) in [(0u32, 10i32), (1, 10), (2, 8)] {
        let idx = w.spawn(
            Position { x: z, y: z },
            Velocity { x: z, y: z },
            Color {
                r: 100,
                g: 120,
                b: 150,
                a: 255,
            },
        );
        w.set_transform(
            idx,
            Transform::at(F::from_num(i as i32), F::from_num(y), F::from_num(0)),
        );
        w.set_velocity_3d(idx, Velocity3D::zero());
        w.set_actor(idx, Actor::npc(1, i + 1));
    }
    w
}

fn native_step(world: &mut World, ticks: u32) {
    let params = PhysicsParams {
        gravity: F::from_num(-0.05),
        floor: 0,
        half: [F::from_num(1); 3],
    };
    for _ in 0..ticks {
        let n = world.entity_count();
        let t = world.get_transforms().unwrap()[..n].to_vec();
        let v = world.get_velocity_3d().unwrap()[..n].to_vec();
        let d = openengine_logic_sandbox::physics_delta(&t, &v, &params).unwrap();
        world.apply_delta(&d);
    }
}

fn guest_step(world: &mut World, host: &mut WasmPhysicsHost, ticks: u32) -> anyhow::Result<()> {
    let p = HostParams {
        gravity: -0.05,
        floor: 0,
        half: [1.0; 3],
    };
    for _ in 0..ticks {
        let d = host.tick(world, &p)?;
        world.apply_delta(&d);
    }
    Ok(())
}

fn poses(world: &World) -> Vec<[i32; 3]> {
    world
        .get_transforms()
        .unwrap()
        .iter()
        .take(world.entity_count())
        .map(|t| {
            [
                t.position[0].to_num::<i32>(),
                t.position[1].to_num::<i32>(),
                t.position[2].to_num::<i32>(),
            ]
        })
        .collect()
}

#[test]
fn wasm_physics_matches_native() {
    let Ok(mut host) = WasmPhysicsHost::load(WASM_ASSET) else {
        eprintln!("SKIP: logic.wasm absent");
        return;
    };
    let mut wasm = build();
    let mut native = build();
    guest_step(&mut wasm, &mut host, 60).expect("guest physics");
    native_step(&mut native, 60);
    assert_eq!(
        poses(&wasm),
        poses(&native),
        "guest physics must match native bit-for-bit"
    );
}

#[test]
fn wasm_physics_deterministic() {
    let Ok(_) = WasmPhysicsHost::load(WASM_ASSET) else {
        eprintln!("SKIP: logic.wasm absent");
        return;
    };
    let run = || -> Vec<[i32; 3]> {
        let mut host = WasmPhysicsHost::load(WASM_ASSET).unwrap();
        let mut w = build();
        guest_step(&mut w, &mut host, 60).unwrap();
        poses(&w)
    };
    assert_eq!(run(), run());
}
