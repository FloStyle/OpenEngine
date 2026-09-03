//! openengine-runner — headless "play a game": load a saved scene + a logic.wasm
//! module, advance the deterministic sim, print a JSON report. Used by the
//! cook/package pipeline (spec 50) and by agents to verify a build headless.

use std::process::ExitCode;

fn arg(args: &[String], name: &str) -> Option<String> {
    let mut i = 0;
    while i < args.len() {
        if args[i] == name {
            return args.get(i + 1).cloned();
        }
        i += 1;
    }
    None
}

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().collect();
    let scene = match arg(&args, "--scene") {
        Some(s) => s,
        None => {
            eprintln!(
                "usage: openengine-runner --scene <scene.json> [--wasm <logic.wasm>] [--frames N]"
            );
            return ExitCode::from(2);
        }
    };
    let wasm = arg(&args, "--wasm");
    let frames: u64 = arg(&args, "--frames")
        .and_then(|f| f.parse().ok())
        .unwrap_or(60);

    match openengine_harness::runner::run(&scene, wasm.as_deref(), frames) {
        Ok(report) => {
            println!(
                "{}",
                serde_json::to_string_pretty(&report).unwrap_or_default()
            );
            ExitCode::SUCCESS
        }
        Err(e) => {
            eprintln!("runner error: {e}");
            ExitCode::FAILURE
        }
    }
}
