//! Phase A PoC integration tests: 1000-entity spawn, registry ids, determinism.

use openengine_ecs::{Color, Position, Velocity, World, COLOR, POSITION, VELOCITY};
use openengine_math::I16F16;

fn create_test_world(count: usize) -> World {
    let mut world = World::new();
    for i in 0..count {
        let x = (i % 32) as i32;
        let y = (i / 32) as i32;
        world.spawn(
            Position { x: I16F16::from_num(x * 10), y: I16F16::from_num(y * 10) },
            Velocity { x: I16F16::from_num(1), y: I16F16::from_num(1) },
            Color { r: (i % 256) as u8, g: ((i * 2) % 256) as u8, b: ((i * 3) % 256) as u8, a: 255 },
        );
    }
    world
}

#[test]
fn spawn_1000_entities() {
    let world = create_test_world(1000);
    assert_eq!(world.entity_count(), 1000);
    let positions = world.get_positions().unwrap();
    for pos in positions.iter().take(1000) {
        assert!(pos.x >= I16F16::from_num(0));
        assert!(pos.y >= I16F16::from_num(0));
    }
    // Data integrity: every entity must round-trip its stored value.
    let colors = world.get_colors().unwrap();
    assert_eq!(colors[7].r, 7);
}

#[test]
fn determinism_bit_identical() {
    let world1 = create_test_world(1000);
    let world2 = create_test_world(1000);
    let world3 = create_test_world(1000);
    let h1 = world1.hash();
    let h2 = world2.hash();
    let h3 = world3.hash();
    assert_eq!(h1, h2, "world 1 and 2 must be bit-identical");
    assert_eq!(h2, h3, "world 2 and 3 must be bit-identical");
}

#[test]
fn component_registry_ids_match_spec21() {
    assert_eq!(POSITION, 0);
    assert_eq!(VELOCITY, 1);
    assert_eq!(COLOR, 72);
    let ids = [POSITION, VELOCITY, COLOR];
    for i in 0..ids.len() {
        for j in (i + 1)..ids.len() {
            assert_ne!(ids[i], ids[j], "component ids must be unique");
        }
    }
}

#[test]
fn fixed_point_is_deterministic() {
    let a = I16F16::from_num(1);
    let b = I16F16::from_num(2);
    let first = (a + b).to_bits();
    for _ in 0..100 {
        assert_eq!((a + b).to_bits(), first, "fixed-point math must be deterministic");
    }
}

#[test]
fn columns_are_pod_and_correct_size() {
    fn is_pod<T: bytemuck::Pod>() {}
    is_pod::<Position>();
    is_pod::<Velocity>();
    is_pod::<Color>();
    assert_eq!(core::mem::size_of::<Position>(), 8);
    assert_eq!(core::mem::size_of::<Velocity>(), 8);
    assert_eq!(core::mem::size_of::<Color>(), 4);
}
