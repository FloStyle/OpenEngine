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

/// The concrete address a bound server is actually listening on.
pub fn bound_addr(server: &tiny_http::Server) -> String {
    server
        .server_addr()
        .to_ip()
        .map(|a| a.to_string())
        .unwrap_or_else(|| "127.0.0.1:0".into())
}

/// Bind on the first free loopback port at or above `prefer`, reporting the
/// real address chosen. Tries `prefer`, then scans upward until a port binds
/// (bounded, so it cannot scan forever). `prefer == 0` asks the OS for any free
/// port. Returns the server plus its actual listening address.
pub fn bind_free(prefer: u16) -> Result<(tiny_http::Server, String), String> {
    if prefer == 0 {
        let addr = "127.0.0.1:0";
        let server = bind(addr).map_err(|e| format!("could not bind {addr}: {e}"))?;
        let a = bound_addr(&server);
        return Ok((server, a));
    }
    let mut port = prefer;
    for _ in 0..100 {
        let addr = format!("127.0.0.1:{port}");
        match bind(&addr) {
            Ok(server) => {
                let a = bound_addr(&server);
                return Ok((server, a));
            }
            Err(_) => port += 1,
        }
    }
    Err(format!(
        "no free loopback port near {prefer} after 100 tries"
    ))
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bind_free_skips_a_busy_port_and_reports_the_real_addr() {
        // Occupy 127.0.0.1:0? tiny_http binds one concrete port, so grab a
        // known-busy anchor by binding 127.0.0.1:0 first and holding it.
        let occupied = bind("127.0.0.1:0").expect("bind an anchor port");
        let anchor = bound_addr(&occupied);
        let anchor_port: u16 = anchor
            .split(':')
            .nth(1)
            .and_then(|p| p.parse().ok())
            .expect("anchor port");
        // Asking for exactly the busy port must not fail — bind_free must skip it.
        let (server, addr) = bind_free(anchor_port).expect("finds a free port");
        let got_port: u16 = addr
            .split(':')
            .nth(1)
            .and_then(|p| p.parse().ok())
            .expect("chosen port");
        assert_ne!(got_port, anchor_port, "must skip the busy anchor port");
        // The reported address is real (reachable).
        assert_eq!(addr, bound_addr(&server));
        drop(server);
        drop(occupied);
    }

    #[test]
    fn bind_free_zero_asks_the_os_for_any_free_port() {
        let (server, addr) = bind_free(0).expect("os-assigned free port");
        let port: u16 = addr
            .split(':')
            .nth(1)
            .and_then(|p| p.parse().ok())
            .expect("chosen port");
        assert_ne!(port, 0, "os must assign a concrete port");
        assert_eq!(addr, bound_addr(&server));
        drop(server);
    }
}
