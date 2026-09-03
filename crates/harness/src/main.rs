//! OpenEngine harness — headless JSON-over-HTTP server binary.

use std::env;

use openengine_harness::{bind, serve, HarnessState};

fn main() {
    let args: Vec<String> = env::args().collect();
    let mut port = String::from("8080");
    let mut i = 1;
    while i < args.len() {
        let a = &args[i];
        if a == "--port" {
            if let Some(v) = args.get(i + 1) {
                port = v.clone();
                i += 1;
            }
        } else if let Some(v) = a.strip_prefix("--port=") {
            port = v.to_string();
        }
        i += 1;
    }
    let addr = format!("127.0.0.1:{port}");
    let server = match bind(&addr) {
        Ok(s) => s,
        Err(e) => {
            eprintln!("could not bind {addr}: {e}");
            std::process::exit(1);
        }
    };
    let state = HarnessState::new();
    eprintln!("openengine-harness listening on http://{addr} (headless, no GPU)");
    serve(server, state);
}
