//! OpenEngine harness — headless JSON-over-HTTP server binary.

use std::env;

use openengine_harness::{bind_free, serve, HarnessState};

fn main() {
    let args: Vec<String> = env::args().collect();
    let mut prefer: u16 = 8080;
    let mut i = 1;
    while i < args.len() {
        let a = &args[i];
        if a == "--port" {
            if let Some(v) = args.get(i + 1) {
                prefer = v.parse().unwrap_or(8080);
                i += 1;
            }
        } else if let Some(v) = a.strip_prefix("--port=") {
            prefer = v.parse().unwrap_or(8080);
        }
        i += 1;
    }
    // Bind on the requested port, or (if busy / --port 0) the next free one.
    let (server, addr) = match bind_free(prefer) {
        Ok((s, a)) => (s, a),
        Err(e) => {
            eprintln!("{e}");
            std::process::exit(1);
        }
    };
    let state = HarnessState::new();
    eprintln!("openengine-harness listening on http://{addr} (headless, no GPU)");
    serve(server, state);
}
