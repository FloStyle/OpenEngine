//! OpenEngine Editor Shell library (Domain A): reusable `EditorApp` + the
//! `SceneRenderer` viewport pipeline (used by the binary and by headless tests).

pub mod ai;
pub mod app;
pub mod renderer;
pub mod screenshot;

pub use ai::{apply_proposal, AiPanel};
pub use app::EditorApp;
pub use renderer::SceneRenderer;
