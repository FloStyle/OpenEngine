//! The real editor Move tool runs as a plugin through a [`PluginHost`]:
//! load -> N updates (each applying the real move math) -> unload.
//!
//! This is the Part-2 proof that a genuine editor tool lives behind the Phase 3
//! plugin boundary: a host registers the tool, drives it every frame, observes
//! that it really executed the ground-grid drag math, then unloads it cleanly.

use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::Arc;

use openengine_plugin_host::plugins::move_tool::{compute_move_transform, MoveToolPlugin};
use openengine_plugin_host::{FrameCtx, PluginHost};

#[test]
fn move_tool_loads_runs_the_real_math_and_unloads_cleanly() {
    let applied = Arc::new(AtomicU32::new(0));
    let mut host = PluginHost::new();
    let mut f = FrameCtx { frame: 0, dt: 0.0 };

    // Load the Move tool behind the boundary, as a host would.
    host.add(
        Box::new(MoveToolPlugin::new(0.5, [0.0, 0.0], applied.clone())),
        &f,
    )
    .unwrap();
    assert_eq!(host.ids(), vec!["move-tool"], "registered as 'move-tool'");

    // Drive it for 10 frames like a shell would; each update must have run.
    for _ in 0..10 {
        f.frame += 1;
        host.update_all(&f).unwrap();
    }
    assert_eq!(
        applied.load(Ordering::Relaxed),
        10,
        "the Move tool ran its update 10x"
    );

    // Unload -> deinit runs, host is empty again.
    assert!(host.unload("move-tool"));
    assert!(host.is_empty());
}

#[test]
fn move_tool_compute_is_the_real_snapped_move() {
    // Cross-check the exposed math independently (domain-A floats, like the
    // headless editor grid): a target at (0.4, 0.6) with a 1.0 grid snaps to
    // (0.0 / 1.0) while the current height y is preserved.
    let grid = openengine_editor::grid::EditorGrid { step: 1.0 };
    let out = compute_move_transform([1.0, 3.0, 1.0], [0.4, 0.6], &grid);
    assert_eq!(out, [0.0, 3.0, 1.0], "target xz snapped, y kept");
}
