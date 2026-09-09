//! A real editor **tool** running as a [`Plugin`]: a tool loaded into a
//! [`PluginHost`] that, on each update, applies the engine's transform math
//! (`openengine-editor::rotate_yaw`). Demonstrates the Phase 3 boundary: the
//! gizmo/tool is a reloadable plugin, not hard-coded in the shell.

use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::Arc;

use openengine_contracts::Transform;
use openengine_editor::transform_edit::rotate_yaw;
use openengine_math::I16F16 as F;
use openengine_plugin_host::{FrameCtx, Plugin, PluginHost};

/// A minimal "Rotate tool" plugin: each update turns the selected actor by a
/// fixed yaw step (the pointer would drive it in a real shell) and records the
/// apply through a shared counter so the test can observe the lifecycle.
struct RotateToolPlugin {
    transform: Transform,
    yaw_step: f32,
    applied: Arc<AtomicU32>,
}

impl RotateToolPlugin {
    fn new(applied: Arc<AtomicU32>) -> Self {
        let mut t = Transform::at(F::from_num(0), F::from_num(0), F::from_num(0));
        t.rotation = [
            F::from_num(0),
            F::from_num(0),
            F::from_num(0),
            F::from_num(1),
        ];
        RotateToolPlugin {
            transform: t,
            yaw_step: 0.1,
            applied,
        }
    }
}

impl Plugin for RotateToolPlugin {
    fn id(&self) -> &'static str {
        "rotate-tool"
    }
    fn update(&mut self, _f: &FrameCtx) -> Result<(), String> {
        self.transform = rotate_yaw(self.transform, self.yaw_step);
        self.applied.fetch_add(1, Ordering::Relaxed);
        Ok(())
    }
}

#[test]
fn a_rotate_tool_runs_as_a_plugin_and_is_unloaded_cleanly() {
    let applied = Arc::new(AtomicU32::new(0));
    let mut host = PluginHost::new();
    let mut f = FrameCtx { frame: 0, dt: 0.0 };
    host.add(Box::new(RotateToolPlugin::new(applied.clone())), &f)
        .unwrap();
    assert_eq!(host.ids(), vec!["rotate-tool"]);

    for _ in 0..10 {
        f.frame += 1;
        host.update_all(&f).unwrap();
    }
    assert_eq!(
        applied.load(Ordering::Relaxed),
        10,
        "the tool's transform update ran 10x"
    );
    assert!(host.unload("rotate-tool"), "tool plugin unloads");
    assert!(host.is_empty());
}
