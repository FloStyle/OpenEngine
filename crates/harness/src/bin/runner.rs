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
                "usage: openengine-runner --scene <scene.json> [--wasm <logic.wasm>] [--frames N] [--forward N | --script events.json]"
            );
            return ExitCode::from(2);
        }
    };
    let wasm = arg(&args, "--wasm");
    let frames: u64 = arg(&args, "--frames")
        .and_then(|f| f.parse().ok())
        .unwrap_or(60);
    let forward: u64 = arg(&args, "--forward")
        .and_then(|f| f.parse().ok())
        .unwrap_or(0);

    let result = if let Some(script_path) = arg(&args, "--script") {
        let events: Vec<openengine_harness::runner::FrameInput> = match std::fs::read(&script_path)
            .map_err(|e| e.to_string())
            .and_then(|b| serde_json::from_slice(&b).map_err(|e| e.to_string()))
        {
            Ok(v) => v,
            Err(e) => {
                eprintln!("--script parse error: {e}");
                return ExitCode::FAILURE;
            }
        };
        openengine_harness::runner::run_script(&scene, wasm.as_deref(), frames, &events)
    } else if forward > 0 {
        openengine_harness::runner::run_forward(&scene, wasm.as_deref(), frames, forward)
    } else {
        openengine_harness::runner::run(&scene, wasm.as_deref(), frames)
    };
    match result {
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
