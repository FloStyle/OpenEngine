//! # Domain A — `openengine-editor`
//!
//! The editor runs on the host side (Domain A). This is the **headless editor
//! core** (no egui, no GPU, testable in CI): Edit/Play isolation (spec 22),
//! undoable Commands (spec 23), camera + ray picking (spec 24), and drag
//! translate. The egui/gpu shell (panels over the 3D viewport) is deferred
//! until the wgpu/egui-wgpu versions align; it will sit on top of this core.
//!
//! Everything here is pure host logic over the ECS `World` (fixed-point
//! `Transform`, id 2) and `glam` `f32` only inside camera/picking math.

#![deny(missing_docs)]

pub mod camera;
pub mod commands;
pub mod grid;
pub mod selection;
pub mod state;
pub mod translate;

pub use commands::{Command, ModifyTransformCommand, UndoRedoManager};
pub use grid::{ground_point_snapped, snap_pos, EditorGrid};
pub use selection::{pick, SelectionModel};
pub use state::{EditorMode, EditorState};
pub use translate::ray_ground_plane;
