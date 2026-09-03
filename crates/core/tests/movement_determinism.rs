//! PoC Phase B integration tests: movement via WorldDelta (single channel)
//! and 3× determinism.

use openengine_core::native_movement::native_movement_system;
use openengine_ecs::{Color, Position, Velocity, World};
use openengine_math::I16F16;

fn create_test_world() -> World {
    let mut world = World::new();
    for i in 0..100 {
        let x = i % 10;
        let y = i / 10;
        world.spawn(
            Position {
                x: I16F16::from_num(x * 50),
                y: I16F16::from_num(y * 50),
            },
            Velocity {
                x: I16F16::from_num(5),
                y: I16F16::from_num(5),
            },
            Color {
                r: 255,
                g: 0,
                b: 0,
                a: 255,
            },
        );
    }
    world
}

fn simulate_ticks(world: &mut World, ticks: usize) {
    for _ in 0..ticks {
        // Single mutation channel: system returns a delta, host applies it.
        let delta = native_movement_system(world);
        world.apply_delta(&delta);
    }
}

#[test]
fn movement_determinism_3x() {
    let mut w1 = create_test_world();
    let mut w2 = create_test_world();
    let mut w3 = create_test_world();
    simulate_ticks(&mut w1, 100);
    simulate_ticks(&mut w2, 100);
    simulate_ticks(&mut w3, 100);
    let h1 = w1.hash();
    let h2 = w2.hash();
    let h3 = w3.hash();
    assert_eq!(h1, h2, "runs 1 and 2 must be bit-identical");
    assert_eq!(h2, h3, "runs 2 and 3 must be bit-identical");
}

#[test]
fn single_mutation_channel_changes_state() {
    let mut world = create_test_world();
    let before = world.hash();
    let delta = native_movement_system(&world);
    world.apply_delta(&delta);
    let after = world.hash();
    assert_ne!(before, after, "movement must change world state");
}

#[test]
fn positions_stay_in_bounds_after_many_ticks() {
    let mut world = create_test_world();
    simulate_ticks(&mut world, 500);
    let positions = world.get_positions().unwrap();
    let vel = world.get_velocities().unwrap();
    for i in 0..world.entity_count() {
        let (x, y) = (
            positions[i].x.to_num::<i32>(),
            positions[i].y.to_num::<i32>(),
        );
        assert!((0..=500).contains(&x), "entity {i} x out of bounds: {x}");
        assert!((0..=500).contains(&y), "entity {i} y out of bounds: {y}");
        // Velocity magnitude is conserved (only direction flips on bounce).
        let m = vel[i].x.to_num::<i32>().abs() + vel[i].y.to_num::<i32>().abs();
        assert_eq!(m, 10, "entity {i} speed must be conserved");
    }
}
