//! OpenEngine Editor Shell library (Domain A): reusable `EditorApp` + the
//! `SceneRenderer` viewport pipeline (used by the binary and by headless tests).

pub mod app;
pub mod renderer;

pub use app::EditorApp;
pub use renderer::SceneRenderer;
