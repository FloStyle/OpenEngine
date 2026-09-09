//! JSON API dispatch for the harness. Pure logic (no HTTP plumbing): maps a
//! (method, path, query, body) to a `(status, JSON)` pair, mutating the
//! [`HarnessState`]. Kept separate so it is trivially testable headless.

use serde_json::{json, Value};

use crate::state::HarnessState;

pub const VERSION: &str = env!("CARGO_PKG_VERSION");

fn ok(v: Value) -> (u16, Value) {
    (200, v)
}
fn err(code: u16, msg: impl std::fmt::Display) -> (u16, Value) {
    (code, json!({ "error": msg.to_string() }))
}

/// Parse a float array of the requested length from a JSON body field.
fn floats(v: &Value, key: &str) -> Result<Vec<f32>, String> {
    v.get(key)
        .and_then(|x| x.as_array())
        .ok_or_else(|| format!("missing or invalid array field '{key}'"))?
        .iter()
        .map(|n| {
            n.as_f64()
                .map(|f| f as f32)
                .ok_or_else(|| format!("non-number in '{key}'"))
        })
        .collect()
}

fn hex_hash(h: u64) -> String {
    format!("{h:016x}")
}

/// Workspace root (from this crate: `crates/harness` -> repo root).
fn repo_root() -> std::path::PathBuf {
    std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("..")
        .join("..")
}

/// Run `cmd args` in the repo root; returns (success, short tail message).
fn run_cmd(cmd: &str, args: &[&str]) -> (bool, String) {
    match std::process::Command::new(cmd)
        .args(args)
        .current_dir(repo_root())
        .output()
    {
        Ok(o) => {
            let tail = String::from_utf8_lossy(if o.stdout.is_empty() {
                &o.stderr
            } else {
                &o.stdout
            });
            let tail: Vec<&str> = tail.lines().rev().take(3).collect();
            let msg = if tail.is_empty() {
                String::new()
            } else {
                tail.join(" | ")
            };
            (o.status.success(), msg)
        }
        Err(e) => (false, format!("spawn {cmd}: {e}")),
    }
}

/// Run the engine's own verification gates and return a structured verdict
/// (spec 53 / Phase 4): workspace tests + logic purity.
fn verify(state: &crate::state::HarnessState) -> Value {
    let (build, build_msg) = run_cmd("cargo", &["build", "--workspace"]);
    let (tests, t_msg) = run_cmd("cargo", &["test", "--workspace"]);
    let (purity, p_msg) = run_cmd(
        "python3",
        &[
            "brain/orchestrator.py",
            "verify-wasm-purity",
            "crates/core/assets/logic.wasm",
        ],
    );
    // Determinism gate: two fresh states replay 16 ticks identically.
    let mut det_ok = false;
    let mut det_hash = String::new();
    {
        let mut a = state.duplicate();
        let mut b = state.duplicate();
        if a.tick_n(16).is_ok() && b.tick_n(16).is_ok() {
            let ha = a.hash();
            let hb = b.hash();
            det_ok = ha == hb;
            det_hash = format!("{ha:016x}");
        }
    }
    let mut errors: Vec<Value> = Vec::new();
    if !build {
        errors.push(json!({"gate":"build","detail":build_msg}));
    }
    if !tests {
        errors.push(json!({"gate":"tests","detail":t_msg}));
    }
    if !purity {
        errors.push(json!({"gate":"purity","detail":p_msg}));
    }
    if !det_ok {
        errors.push(json!({"gate":"determinism","detail":"hash mismatch"}));
    }
    let status = if errors.is_empty() { "PASS" } else { "FAIL" };
    json!({
        "status": status,
        "build": { "ok": build },
        "tests": { "ok": tests },
        "purity": { "ok": purity, "status": if purity { "[PURE]".to_string() } else { p_msg } },
        "determinism": { "ok": det_ok, "hash": det_hash },
        "errors": errors
    })
}

/// Dispatch one request. `body` is the raw (already-read) request body.
pub fn dispatch(state: &mut HarnessState, method: &str, path: &str, body: &[u8]) -> (u16, Value) {
    match (method, path) {
        ("GET", "/health") => ok(json!({
            "status": "ok",
            "version": VERSION,
            "headless": true,
            "capabilities": ["observe", "spawn", "despawn", "set", "tick", "hash", "load_wasm", "prove", "transaction", "save", "load", "verify", "reload_logic", "snapshot", "restore"],
        })),
        ("GET", "/spec") => ok(json!({
            "service": "openengine-harness",
            "version": VERSION,
            "endpoints": [
                {"method":"GET","path":"/health","desc":"liveness + capabilities"},
                {"method":"GET","path":"/spec","desc":"this contract"},
                {"method":"GET","path":"/observe?limit=50","desc":"world snapshot"},
                {"method":"POST","path":"/spawn","body":"{\"transform\":[x,y,z],\"scale\":[1,1,1],\"color\":[r,g,b,a]}"},
                {"method":"POST","path":"/despawn","body":"{\"entity\":i}"},
                {"method":"POST","path":"/set","body":"{\"entity\":i,\"component\":\"transform|scale|color\",\"value\":[...]}"},
                {"method":"POST","path":"/tick","body":"{\"n\":100}"},
                {"method":"GET","path":"/hash","desc":"determinism hash"},
                {"method":"POST","path":"/load_wasm","body":"{\"path\":\"...\"}"},
                {"method":"POST","path":"/prove","body":"{\"n\":100}","desc":"determinism PASS/FAIL over two fresh states"},
                {"method":"POST","path":"/transaction","body":"{\"ops\":[{method,path,body},...]}","desc":"atomic batch, rollback on any failing op"},
                {"method":"POST","path":"/save","body":"{\"path\":\"scene.json\"}","desc":"write current scene to a file"},
                {"method":"POST","path":"/load","body":"{\"path\":\"scene.json\"}","desc":"load a scene file into the world"},
                {"method":"GET","path":"/verify","desc":"run repo build+tests+purity, return structured PASS/FAIL"},
                {"method":"GET","path":"/snapshot","desc":"return full in-memory state (all columns + tick)"},
                {"method":"POST","path":"/restore","body":"{\"snapshot\":{...}}","desc":"replace the world from a snapshot"}
            ]
        })),
        ("GET", "/hash") => {
            let h = state.hash();
            ok(
                json!({ "hash": hex_hash(h), "tick": state.tick(), "entity_count": state.entity_count() }),
            )
        }
        ("GET", "/observe") => {
            // limit from query handled by caller-split path; default 50.
            let (entities, tick) = state.observe(50);
            ok(json!({ "entity_count": entities.len(), "tick": tick, "entities": entities }))
        }
        ("POST", "/spawn") => {
            let v: Value = match serde_json::from_slice(body) {
                Ok(x) => x,
                Err(e) => return err(400, format!("bad json: {e}")),
            };
            let pos = floats(&v, "transform").unwrap_or_default();
            let scale = floats(&v, "scale").unwrap_or_default();
            let color = floats(&v, "color").unwrap_or_default();
            let pos = [
                pos.first().copied().unwrap_or(0.0),
                pos.get(1).copied().unwrap_or(0.0),
                pos.get(2).copied().unwrap_or(0.0),
            ];
            let scale = [
                scale.first().copied().unwrap_or(1.0),
                scale.get(1).copied().unwrap_or(1.0),
                scale.get(2).copied().unwrap_or(1.0),
            ];
            let col = [
                color.first().copied().unwrap_or(255.0).clamp(0.0, 255.0) as u8,
                color.get(1).copied().unwrap_or(255.0).clamp(0.0, 255.0) as u8,
                color.get(2).copied().unwrap_or(255.0).clamp(0.0, 255.0) as u8,
                color.get(3).copied().unwrap_or(255.0).clamp(0.0, 255.0) as u8,
            ];
            let i = state.spawn(pos, scale, col);
            ok(json!({ "entity": i, "entity_count": state.entity_count() }))
        }
        ("POST", "/despawn") => {
            let v: Value = match serde_json::from_slice(body) {
                Ok(x) => x,
                Err(e) => return err(400, format!("bad json: {e}")),
            };
            let idx = match v.get("entity").and_then(|x| x.as_u64()) {
                Some(i) => i as usize,
                None => return err(400, "missing 'entity'"),
            };
            match state.despawn(idx) {
                Ok(()) => ok(json!({ "ok": true, "entity_count": state.entity_count() })),
                Err(e) => err(400, e),
            }
        }
        ("POST", "/set") => {
            let v: Value = match serde_json::from_slice(body) {
                Ok(x) => x,
                Err(e) => return err(400, format!("bad json: {e}")),
            };
            let idx = match v.get("entity").and_then(|x| x.as_u64()) {
                Some(i) => i as usize,
                None => return err(400, "missing 'entity'"),
            };
            let comp = match v.get("component").and_then(|x| x.as_str()) {
                Some(c) => c.to_string(),
                None => return err(400, "missing 'component'"),
            };
            let vals = match floats(&v, "value") {
                Ok(x) => x,
                Err(e) => return err(400, e),
            };
            match state.set(idx, &comp, &vals) {
                Ok(()) => ok(json!({ "ok": true })),
                Err(e) => err(400, e),
            }
        }
        ("POST", "/tick") => {
            let v: Value = match serde_json::from_slice(body) {
                Ok(x) => x,
                Err(e) => return err(400, format!("bad json: {e}")),
            };
            let n = v
                .get("n")
                .and_then(|x| x.as_u64())
                .unwrap_or(1)
                .min(100_000);
            match state.tick_n(n) {
                Ok(()) => {
                    ok(json!({ "ticks": n, "hash": hex_hash(state.hash()), "tick": state.tick() }))
                }
                Err(e) => err(500, e),
            }
        }
        ("POST", "/load_wasm") => {
            let v: Value = match serde_json::from_slice(body) {
                Ok(x) => x,
                Err(e) => return err(400, format!("bad json: {e}")),
            };
            let path = match v.get("path").and_then(|x| x.as_str()) {
                Some(p) => p.to_string(),
                None => return err(400, "missing 'path'"),
            };
            match state.load_wasm(&path) {
                Ok(()) => ok(json!({ "ok": true, "engine": "wasm" })),
                Err(e) => err(500, e),
            }
        }
        // ── Safety seams (spec 51/52): prove determinism + atomic transaction ──
        ("POST", "/prove") => {
            let v: Value = match serde_json::from_slice(body) {
                Ok(x) => x,
                Err(e) => return err(400, format!("bad json: {e}")),
            };
            let n = v
                .get("n")
                .and_then(|x| x.as_u64())
                .unwrap_or(100)
                .min(100_000);
            // Two independent, fresh states (each with its own guest if loaded)
            // replay the same deterministic tick sequence.
            let mut a = state.duplicate();
            let mut b = state.duplicate();
            if let Err(e) = a.tick_n(n) {
                return err(500, format!("prove run A failed: {e}"));
            }
            if let Err(e) = b.tick_n(n) {
                return err(500, format!("prove run B failed: {e}"));
            }
            let ha = hex_hash(a.hash());
            let hb = hex_hash(b.hash());
            ok(json!({ "equal": ha == hb, "hash_a": ha, "hash_b": hb, "ticks": n }))
        }
        ("POST", "/transaction") => {
            let v: Value = match serde_json::from_slice(body) {
                Ok(x) => x,
                Err(e) => return err(400, format!("bad json: {e}")),
            };
            let ops = match v.get("ops").and_then(|x| x.as_array()) {
                Some(o) => o,
                None => return err(400, "missing 'ops' array"),
            };
            // Snapshot; on any failing op, roll the whole batch back.
            let checkpoint = state.duplicate();
            for (i, op) in ops.iter().enumerate() {
                let m = op.get("method").and_then(|x| x.as_str()).unwrap_or("POST");
                let p = op.get("path").and_then(|x| x.as_str()).unwrap_or("");
                let body_bytes = match op.get("body") {
                    Some(b) => serde_json::to_vec(b).unwrap_or_else(|_| b"{}".to_vec()),
                    None => b"{}".to_vec(),
                };
                let (code, res) = dispatch(state, m, p, &body_bytes);
                if code >= 400 {
                    state.overwrite_from(&checkpoint);
                    return err(
                        409,
                        format!("op {i} failed ({}); rolled back", res["error"]),
                    );
                }
            }
            ok(json!({ "ok": true, "applied": ops.len(), "entity_count": state.entity_count() }))
        }
        // ── Scene persistence (spec 16): save/load a portable scene file ──
        ("POST", "/save") => {
            let v: Value = match serde_json::from_slice(body) {
                Ok(x) => x,
                Err(e) => return err(400, format!("bad json: {e}")),
            };
            let path = match v.get("path").and_then(|x| x.as_str()) {
                Some(p) => p.to_string(),
                None => return err(400, "missing 'path'"),
            };
            let scene = state.export_scene();
            match std::fs::write(&path, serde_json::to_vec_pretty(&scene).unwrap_or_default()) {
                Ok(()) => ok(json!({ "ok": true, "path": path, "entities": scene.entities.len() })),
                Err(e) => err(500, format!("write {path}: {e}")),
            }
        }
        ("POST", "/load") => {
            let v: Value = match serde_json::from_slice(body) {
                Ok(x) => x,
                Err(e) => return err(400, format!("bad json: {e}")),
            };
            let path = match v.get("path").and_then(|x| x.as_str()) {
                Some(p) => p.to_string(),
                None => return err(400, "missing 'path'"),
            };
            match std::fs::read(&path) {
                Ok(bytes) => match serde_json::from_slice::<crate::state::SceneFile>(&bytes) {
                    Ok(scene) => match state.import_scene(&scene) {
                        Ok(()) => ok(
                            json!({ "ok": true, "path": path, "entities": scene.entities.len() }),
                        ),
                        Err(e) => err(400, e),
                    },
                    Err(e) => err(400, format!("bad scene json: {e}")),
                },
                Err(e) => err(500, format!("read {path}: {e}")),
            }
        }
        // ── In-memory fork/rollback: snapshot + restore (all columns + tick) ──
        ("GET", "/snapshot") => {
            let s = state.export_scene();
            ok(serde_json::to_value(&s).unwrap_or(Value::Null))
        }
        ("POST", "/restore") => {
            let v: Value = match serde_json::from_slice(body) {
                Ok(x) => x,
                Err(e) => return err(400, format!("bad json: {e}")),
            };
            let snap = v.get("snapshot").unwrap_or(&v).clone();
            match serde_json::from_value::<crate::state::SceneFile>(snap) {
                Ok(scene) => match state.import_scene(&scene) {
                    Ok(()) => ok(json!({ "ok": true, "entities": scene.entities.len() })),
                    Err(e) => err(400, e),
                },
                Err(e) => err(400, format!("bad snapshot json: {e}")),
            }
        }
        ("GET", "/verify") | ("POST", "/verify") => ok(verify(state)),
        ("POST", "/reload_logic") => {
            let path = {
                let v: Value = match serde_json::from_slice(body) {
                    Ok(x) => x,
                    Err(_) => json!({}),
                };
                v.get("path")
                    .and_then(|x| x.as_str())
                    .unwrap_or("crates/core/assets/logic.wasm")
                    .to_string()
            };
            // Rebuild the pure logic module, then re-instantiate the guest.
            let (built, msg) = run_cmd("bash", &["scripts/build.sh"]);
            if !built {
                return err(500, format!("rebuild failed: {msg}"));
            }
            match state.load_wasm(&path) {
                Ok(()) => ok(json!({ "ok": true, "engine": "wasm", "path": path })),
                Err(e) => err(500, e),
            }
        }
        _ => err(404, format!("no route: {method} {path}")),
    }
}
