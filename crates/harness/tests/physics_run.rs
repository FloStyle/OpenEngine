//! Headless test that a real `World` can be driven by the Domain-B physics
//! system (gravity + floor + AABB XZ separation) through `HarnessState`.

use openengine_harness::HarnessState;

fn scene_xy() -> Vec<[f32; 3]> {
    let mut s = HarnessState::new();
    s.spawn([0.0, 10.0, 0.0], [1.0, 1.0, 1.0], [255, 0, 0, 255]);
    s.spawn([1.0, 10.0, 0.0], [1.0, 1.0, 1.0], [0, 255, 0, 255]);
    s.physics_tick([1.0, 1.0, 1.0], -0.05, 0.0, 600)
        .expect("physics");
    s.observe(4).0.into_iter().map(|e| e.transform).collect()
}

#[test]
fn physics_drives_a_world_to_rest_separated() {
    let pos = scene_xy();
    assert_eq!(pos.len(), 2);
    // Both bodies rest on the floor.
    assert!(
        pos[0][1].abs() < 0.001,
        "entity 0 must rest at y=0, got {}",
        pos[0][1]
    );
    assert!(
        pos[1][1].abs() < 0.001,
        "entity 1 must rest at y=0, got {}",
        pos[1][1]
    );
    // And are separated in X (no overlap).
    assert!(
        (pos[0][0] - pos[1][0]).abs() > 0.5,
        "bodies must separate in X"
    );
}

#[test]
fn physics_world_is_deterministic() {
    let a = scene_xy();
    let b = scene_xy();
    assert_eq!(a, b, "two physics runs must be bit-identical");
}
