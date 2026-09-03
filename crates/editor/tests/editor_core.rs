//! Editor core headless tests: clone/edit-play isolation, undo/redo, picking.

use openengine_contracts::Transform;
use openengine_ecs::{Color, Position, Velocity, World};
use openengine_editor::commands::{ModifyTransformCommand, UndoRedoManager};
use openengine_editor::{pick, EditorMode, EditorState};
use openengine_math::I16F16;

fn x(v: i32) -> I16F16 {
    I16F16::from_num(v)
}

fn make_world(count: usize) -> World {
    let mut w = World::new();
    for i in 0..count {
        let idx = w.spawn(
            Position { x: x(0), y: x(0) },
            Velocity { x: x(0), y: x(0) },
            Color {
                r: 255,
                g: 128,
                b: 0,
                a: 255,
            },
        );
        w.set_transform(idx, Transform::at(x((i as i32) * 2), x(0), x(0)));
    }
    w
}

fn identity_at(z: i32) -> Transform {
    Transform::at(x(0), x(0), x(z))
}

#[test]
fn clone_state_is_bit_identical() {
    let a = make_world(10);
    let b = a.clone_state();
    assert_eq!(a.hash(), b.hash());
    assert_eq!(a.entity_count(), b.entity_count());
    // Mutating the clone must not affect the original.
    let mut c = a.clone_state();
    c.set_transform(0, identity_at(5));
    assert_ne!(a.hash(), c.hash());
}

#[test]
fn edit_play_isolation_keeps_edit_untouched() {
    let mut st = EditorState::new(make_world(4));
    let before = st.edit_world.hash();
    st.play();
    assert_eq!(st.mode, EditorMode::Playing);
    assert!(!st.can_edit());

    // Sim runs only on the play world.
    {
        let pw = st.mutable_world().expect("play world");
        pw.set_transform(0, identity_at(9));
    }
    assert_ne!(st.active_world().hash(), before);

    st.stop();
    assert_eq!(st.mode, EditorMode::Edit);
    assert_eq!(
        st.edit_world.hash(),
        before,
        "edit world must be untouched by play"
    );
    assert!(st.play_world.is_none());
}

#[test]
fn undo_redo_restores_hash_exactly() {
    let mut w = make_world(3);
    let mut mgr = UndoRedoManager::new();
    let baseline = make_world(3).hash();

    let old = w.get_transforms().unwrap()[0];
    let new = identity_at(7);
    mgr.execute(
        &mut w,
        Box::new(ModifyTransformCommand {
            entity_index: 0,
            old_value: old,
            new_value: new,
        }),
    );

    let after = w.hash();
    assert_ne!(baseline, after);

    assert!(mgr.undo(&mut w));
    assert_eq!(w.hash(), baseline, "undo must restore the pre-edit state");
    assert!(mgr.redo(&mut w));
    assert_eq!(w.hash(), after, "redo must restore the edited state");
}

#[test]
fn redo_is_cleared_on_new_edit() {
    let mut w = make_world(2);
    let mut mgr = UndoRedoManager::new();
    let old = w.get_transforms().unwrap()[0];

    mgr.execute(
        &mut w,
        Box::new(ModifyTransformCommand {
            entity_index: 0,
            old_value: old,
            new_value: identity_at(3),
        }),
    );
    assert!(mgr.undo(&mut w));
    assert!(mgr.can_redo());

    // A new edit clears the redo stack.
    mgr.execute(
        &mut w,
        Box::new(ModifyTransformCommand {
            entity_index: 0,
            old_value: old,
            new_value: identity_at(4),
        }),
    );
    assert!(!mgr.can_redo());
}

#[test]
fn determinism_same_sequence_3x() {
    let hashes = (0..3)
        .map(|_| {
            let mut w = make_world(5);
            let mut mgr = UndoRedoManager::new();
            for e in 0..5 {
                let old = w.get_transforms().unwrap()[e];
                let new = identity_at(e as i32 * 3 + 1);
                mgr.execute(
                    &mut w,
                    Box::new(ModifyTransformCommand {
                        entity_index: e as u32,
                        old_value: old,
                        new_value: new,
                    }),
                );
            }
            for _ in 0..5 {
                mgr.undo(&mut w);
            }
            w.hash()
        })
        .collect::<Vec<_>>();
    assert_eq!(hashes[0], hashes[1]);
    assert_eq!(hashes[1], hashes[2]);
}

#[test]
fn pick_hits_centered_entity_and_misses_far_away() {
    use openengine_editor::camera::EditorCamera;
    let w = make_world(1); // entity 0 at origin.

    let cam = EditorCamera {
        focus: glam::Vec3::ZERO,
        distance: 4.0,
        yaw: 0.0,
        pitch: 0.0,
        fov: 45f32.to_radians(),
    };
    // Eye sits at +Z looking at origin.
    let (o, d) = cam.unproject_ray(0.0, 0.0, 1.0);
    assert_eq!(pick(o, d, &w), Some(0), "centered ray should hit entity 0");

    // Point the camera the other way (focus behind -z, cube at origin is
    // behind the eye) so the ray never reaches the cube.
    let mut miss_world = make_world(1);
    miss_world.set_transform(0, Transform::at(x(100), x(0), x(0)));
    // Camera still looking at the origin (+Z eye, toward -Z) — cube is at +X.
    let (o2, d2) = cam.unproject_ray(0.0, 0.0, 1.0);
    assert_eq!(
        pick(o2, d2, &miss_world),
        None,
        "ray facing -Z must miss a cube at +X"
    );
}

#[test]
fn ground_plane_intersection() {
    let p = openengine_editor::translate::ray_ground_plane(
        glam::Vec3::new(0.0, 2.0, 0.0),
        glam::Vec3::new(0.0, -1.0, 0.0),
    )
    .expect("ray hits plane");
    assert!(p.y.abs() < 1e-5);
    // Parallel ray misses.
    assert!(openengine_editor::translate::ray_ground_plane(
        glam::Vec3::new(0.0, 1.0, 0.0),
        glam::Vec3::X
    )
    .is_none());
}
