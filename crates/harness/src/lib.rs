//! # OpenEngine harness core
//!
//! The headless connection point that turns OpenEngine into a self-developing
//! harness: a JSON-over-HTTP door over the live [`World`](openengine_ecs::World)
//! for observe / mutate / verify. No GPU, no winit — Domain A only.
//!
//! An agent (or human) starts it headless, then calls `/observe`, `/spawn`,
//! `/set`, `/tick`, `/hash`, `/load_wasm` to read and mutate real engine state
//! and to prove determinism via the returned hashes.

pub mod api;
pub mod runner;
pub mod state;
pub mod wasm_guest;

pub use state::HarnessState;

/// Bind a headless HTTP server on `addr` (e.g. `127.0.0.1:8080`).
pub fn bind(addr: &str) -> Result<tiny_http::Server, Box<dyn std::error::Error + Send + Sync>> {
    tiny_http::Server::http(addr)
}

/// Run the serve loop forever, owning the [`HarnessState`]. Each request is
/// dispatched mutably against `state`, so observability is single-threaded and
/// deterministic (no concurrent mutation of the world).
pub fn serve(server: tiny_http::Server, mut state: HarnessState) {
    for mut req in server.incoming_requests() {
        let method = req.method().as_str().to_string();
        let url = req.url().to_string();
        let path = url.split('?').next().unwrap_or("").to_string();
        let mut body = Vec::new();
        {
            let r = req.as_reader();
            let _ = r.read_to_end(&mut body);
        }
        let (code, value) = api::dispatch(&mut state, &method, &path, &body);
        let text = serde_json::to_string(&value).unwrap_or_else(|_| "{}".into());
        let ct = tiny_http::Header::from_bytes("Content-Type", "application/json").unwrap();
        let resp = tiny_http::Response::from_data(text)
            .with_status_code(code)
            .with_header(ct);
        let _ = req.respond(resp);
    }
}
