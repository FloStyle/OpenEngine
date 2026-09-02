//! # Domain A — `openengine-editor`
//!
//! The editor is **not** a separate application: it is a **system inside the
//! ECS**, exactly like any gameplay system, but running on the host side. It
//! observes a [`StateView`] and emits [`DeferredCommand`]s back through the same
//! pure pipeline — no special-casing, no privileged memory access.
//!
//! Because the editor is host-resident it may do things gameplay logic cannot:
//! open windows, talk to the file system, hot-reload Wasm, and render debug
//! gizmos. That privilege is why it is Domain A and never compiled into the
//! guest.
//!
//! ## Scaffold state
//! UI panels, docking and the inspector come in the editor milestone.

#![deny(missing_docs)]

/// Top-level editor context that owns the `egui` state for the frame loop.
pub struct EditorContext {
    /// egui's per-frame context handle.
    pub egui_ctx: egui::Context,
}

impl EditorContext {
    /// Construct an editor context. Skeleton.
    pub fn new() -> Self {
        EditorContext {
            egui_ctx: egui::Context::default(),
        }
    }
}

impl Default for EditorContext {
    fn default() -> Self {
        Self::new()
    }
}

/// The editor's per-frame "system" entry — mirrors a pure system but is allowed
/// host privileges. Skeleton; panels arrive in the editor milestone.
#[allow(dead_code)]
pub fn run_editor_system(ctx: &mut EditorContext, egui_input: &egui::RawInput) {
    let _output = ctx.egui_ctx.run(egui_input.clone(), |ctx| {
        egui::Window::new("OpenEngine Inspector").show(ctx, |_ui| {
            // placeholder: real ECS inspector panels here
        });
    });
    // `_output` includes platform_ output (needs repaint, cursor icon, ...)
    // that the renderer consumes. Scaffold ignores it for now.
}
