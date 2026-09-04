//! Headless verification of the editor's windowed Save/Load + auto-play logic.
//!
//! `EditorApp` is constructible without a window (egui context + scene only), so
//! we can prove the exact functions behind the toolbar 💾 Save / 📂 Load buttons
//! and the `--play <scene>` launch path work — no GPU needed.

use openengine_ecs::{Color, Position, Velocity};
use openengine_editor::state::EditorMode;
use openengine_editor_shell::app::EditorApp;
use openengine_math::I16F16 as F;

fn fx(v: f32) -> F {
    F::from_num(v)
}

#[test]
fn editor_save_load_scene_roundtrip() {
    let mut a = EditorApp::new();
    // Make the edit world distinctive: add one extra entity.
    a.state.edit_world.spawn(
        Position {
            x: fx(0.0),
            y: fx(0.0),
        },
        Velocity {
            x: fx(0.0),
            y: fx(0.0),
        },
        Color {
            r: 1,
            g: 2,
            b: 3,
            a: 255,
        },
    );
    let expected = a.state.edit_world.entity_count();

    let path =
        std::env::temp_dir().join(format!("openengine_editorsave_{}.json", std::process::id()));
    let p = path.to_str().unwrap().to_string();
    a.scene_path = p.clone();
    a.save_scene();
    assert!(
        a.scene_notice
            .as_deref()
            .is_some_and(|m| m.starts_with("saved")),
        "save should set a success notice: {:?}",
        a.scene_notice
    );

    // Load into a fresh editor: its world must become exactly the saved one.
    let mut b = EditorApp::new();
    b.scene_path = p.clone();
    b.load_scene();
    assert!(
        b.scene_notice
            .as_deref()
            .is_some_and(|m| m.starts_with("loaded")),
        "load should set a success notice: {:?}",
        b.scene_notice
    );
    assert_eq!(
        b.state.edit_world.entity_count(),
        expected,
        "loaded scene must match the saved one"
    );

    // `--play` path: after load + play(), the editor is in Playing mode.
    b.state.play();
    assert_eq!(b.state.mode, EditorMode::Playing);

    let _ = std::fs::remove_file(&path);
}

#[test]
fn editor_load_scene_is_blocked_while_playing() {
    let mut a = EditorApp::new();
    a.state.play();
    a.scene_path = "unused.json".to_string();
    a.load_scene(); // should be a no-op with a notice
    assert!(
        a.scene_notice
            .as_deref()
            .is_some_and(|m| m.contains("stop Play")),
        "load while playing must be refused: {:?}",
        a.scene_notice
    );
}

#[test]
fn editor_add_and_delete_actor() {
    let mut a = EditorApp::new();
    let n0 = a.state.edit_world.entity_count();
    let idx = a.spawn_actor().expect("spawn adds an actor");
    assert_eq!(a.state.edit_world.entity_count(), n0 + 1);
    assert_eq!(a.selection.selected, vec![idx]);

    a.delete_actor(idx as usize);
    assert_eq!(
        a.state.edit_world.entity_count(),
        n0,
        "deleting the just-added actor must restore the count"
    );
    assert!(
        a.selection.selected.is_empty(),
        "delete clears the selection"
    );
}
