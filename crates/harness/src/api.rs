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

/// Introspection: the spec-21 component registry an agent may read/edit.
///
/// Sizes come from the real Rust structs (`size_of`), field names match each
/// component's public API — so an agent knows *what* it can spawn/set without
/// reading source. `component_ids` is the name→id index for `/set`.
fn schema() -> Value {
    use std::mem::size_of;
    // (spec-21 id, name, byte size, public field names).
    let rows: [(u32, &str, usize, &[&str]); 6] = [
        (
            openengine_ecs::POSITION,
            "Position",
            size_of::<openengine_ecs::Position>(),
            &["x", "y"],
        ),
        (
            openengine_ecs::VELOCITY,
            "Velocity",
            size_of::<openengine_ecs::Velocity>(),
            &["x", "y"],
        ),
        (
            openengine_contracts::comp::TRANSFORM,
            "Transform",
            size_of::<openengine_contracts::Transform>(),
            &["position", "rotation", "scale"],
        ),
        (
            openengine_contracts::comp::COLOR,
            "Color",
            size_of::<openengine_ecs::Color>(),
            &["r", "g", "b", "a"],
        ),
        (
            openengine_contracts::comp::VELOCITY3D,
            "Velocity3D",
            size_of::<openengine_contracts::Velocity3D>(),
            &["linear"],
        ),
        (
            openengine_contracts::comp::ACTOR,
            "Actor",
            size_of::<openengine_contracts::Actor>(),
            &[
                "kind",
                "seed",
                "grounded",
                "jump_cd",
                "move_speed",
                "jump_force",
            ],
        ),
    ];
    let components: Vec<Value> = rows
        .iter()
        .map(|(id, name, size, fields)| {
            json!({
                "id": id,
                "name": name,
                "size": size,
                "fields": fields,
            })
        })
        .collect();
    let mut by_name = serde_json::Map::new();
    for (id, name, _, _) in &rows {
        by_name.insert((*name).to_string(), json!(id));
    }
    json!({
        "components": components,
        "component_ids": Value::Object(by_name),
        "note": "spec-21 registry; sizes are size_of::<T>() of the repr(C) Pod structs. ids 0/1/2 (2D bridge) and 72/80/81 (engine band).",
    })
}

/// Dispatch one request. `body` is the raw (already-read) request body.
pub fn dispatch(state: &mut HarnessState, method: &str, path: &str, body: &[u8]) -> (u16, Value) {
    match (method, path) {
        ("GET", "/health") => ok(json!({
            "status": "ok",
            "version": VERSION,
            "headless": true,
            "capabilities": ["observe", "spawn", "despawn", "set", "tick", "physics", "hash", "load_wasm", "prove", "transaction", "save", "load", "verify", "reload_logic", "snapshot", "restore", "schema", "ask"],
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
                {"method":"POST","path":"/physics","body":"{\"n\":100,\"half\":[1,1,1],\"gravity\":-0.05,\"floor\":0}","desc":"Domain-B deterministic physics (gravity+floor+AABB)"},
                {"method":"GET","path":"/hash","desc":"determinism hash"},
                {"method":"POST","path":"/load_wasm","body":"{\"path\":\"...\"}"},
                {"method":"POST","path":"/prove","body":"{\"n\":100}","desc":"determinism PASS/FAIL over two fresh states"},
                {"method":"POST","path":"/transaction","body":"{\"ops\":[{method,path,body},...]}","desc":"atomic batch, rollback on any failing op"},
                {"method":"POST","path":"/save","body":"{\"path\":\"scene.json\"}","desc":"write current scene to a file"},
                {"method":"POST","path":"/load","body":"{\"path\":\"scene.json\"}","desc":"load a scene file into the world"},
                {"method":"GET","path":"/verify","desc":"run repo build+tests+purity, return structured PASS/FAIL"},
                {"method":"GET","path":"/snapshot","desc":"return full in-memory state (all columns + tick)"},
                {"method":"POST","path":"/restore","body":"{\"snapshot\":{...}}","desc":"replace the world from a snapshot"},
                {"method":"GET","path":"/schema","desc":"spec-21 component registry (id/name/size/fields)"},
                {"method":"POST","path":"/ask","body":"{\"message\":\"...\",\"propose\":true}","desc":"resident-operator: call the configured model (optionally apply its proposal batch)"}
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
        // Domain-B deterministic physics (gravity + floor + AABB separation).
        ("POST", "/physics") => {
            let v: Value = match serde_json::from_slice(body) {
                Ok(x) => x,
                Err(e) => return err(400, format!("bad json: {e}")),
            };
            let n = v
                .get("n")
                .and_then(|x| x.as_u64())
                .unwrap_or(1)
                .min(100_000);
            let gravity = v.get("gravity").and_then(|x| x.as_f64()).unwrap_or(-0.05) as f32;
            let floor = v.get("floor").and_then(|x| x.as_f64()).unwrap_or(0.0) as f32;
            let half = v
                .get("half")
                .and_then(|x| x.as_array())
                .map(|a| {
                    a.iter()
                        .map(|e| e.as_f64().unwrap_or(1.0) as f32)
                        .collect::<Vec<_>>()
                })
                .unwrap_or_else(|| vec![1.0, 1.0, 1.0]);
            let half = [
                half.first().copied().unwrap_or(1.0),
                half.get(1).copied().unwrap_or(1.0),
                half.get(2).copied().unwrap_or(1.0),
            ];
            match state.physics_tick(half, gravity, floor, n) {
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
        ("GET", "/schema") => ok(schema()),
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
        // ── AI status: config read only, zero network. ──
        ("GET", "/ai/status") | ("POST", "/ai/status") => ok(ai_status()),
        // ── Resident operator: /ask calls the configured model (spec 51/52). ──
        ("POST", "/ask") => ask(state, body),
        // ── Vision: offscreen screenshot of the live world (capture feature). ──
        ("GET", "/frame") | ("GET", "/screenshot") => crate::api::frame(state, path),
        _ => err(404, format!("no route: {method} {path}")),
    }
}

/// Config-only AI status (no network, no GPU). Mirrors the resolved model
/// config the harness would use for `/ask`-style features.
fn ai_status() -> Value {
    use openengine_ai::config::{resolve_config, vision_supported};
    match resolve_config(None) {
        Ok((cfg, src)) => json!({
            "configured": true,
            "source": src.to_string(),
            "provider": match &cfg.provider {
                openengine_ai::ProviderConfig::ApiKey { .. } => "deepseek",
                openengine_ai::ProviderConfig::Local { .. } => "llama.cpp",
            },
            "model": cfg.model,
            "endpoint": cfg.describe(),
            "vision": vision_supported(&cfg),
        }),
        Err(_) => {
            json!({ "configured": false, "error": "no AI config. set OPENENGINE_AI_CONFIG or ./config/ai.json" })
        }
    }
}

/// `/ask`: resident-operator chat (spec 51/52). Calls the configured model with
/// an observe context; when `propose:true` the model is asked to reply with a
/// typed [`ProposeBatch`] which the engine parses and applies atomically
/// (reversible) via the single mutation channel.
fn ask(state: &mut HarnessState, body: &[u8]) -> (u16, Value) {
    // Resolve the model config (env wins; no network here).
    let (cfg, _src) = match openengine_ai::config::resolve_config(None) {
        Ok(x) => x,
        Err(_) => {
            return err(
                409,
                "no model configured. set OPENENGINE_AI_CONFIG or create config/ai.json (see config/ai*.example.json).",
            )
        }
    };
    let v: Value = match serde_json::from_slice(body) {
        Ok(x) => x,
        Err(e) => return err(400, format!("bad json: {e}")),
    };
    let message = v
        .get("message")
        .and_then(|x| x.as_str())
        .unwrap_or("")
        .to_string();
    let system = v.get("system").and_then(|x| x.as_str()).map(str::to_string);
    let propose = v.get("propose").and_then(|x| x.as_bool()).unwrap_or(false);

    // Build the observe context (entities) to ground the model.
    let (entities, _tick) = state.observe(50);
    let summaries: Vec<openengine_ai::EntitySummary> = entities
        .into_iter()
        .map(|e| openengine_ai::EntitySummary {
            index: e.index,
            transform: e.transform,
            color: e.color,
        })
        .collect();
    let observe = openengine_ai::observe_context(state.entity_count(), &summaries);

    let instruction = if propose {
        "You are the OpenEngine resident operator. Observe the world, then reply \
         with ONLY a JSON proposal batch of ops to carry out the user's request \
         (spawn/set/despawn). Do not add prose outside the JSON."
            .to_string()
    } else {
        "You are the OpenEngine resident assistant. Answer concisely using the world state."
            .to_string()
    };
    let system_prompt = match &system {
        Some(s) => format!("{observe}\n\n{instruction}\n\n{s}"),
        None => format!("{observe}\n\n{instruction}"),
    };

    // Build + call the model.
    let adapter = match openengine_ai::providers::from_config(&cfg) {
        Ok(a) => a,
        Err(e) => return err(500, format!("model init: {e}")),
    };
    let turns = vec![
        openengine_ai::ChatTurn::text("system", &system_prompt),
        openengine_ai::ChatTurn::text("user", &message),
    ];
    let reply = match adapter.complete(&turns) {
        Ok(r) => r,
        Err(e) => return err(502, format!("model call failed: {e}")),
    };

    if !propose {
        return ok(json!({ "reply": reply, "model": cfg.model, "applied": false }));
    }
    // Propose mode: parse the model's reply as typed ops and apply atomically.
    match openengine_ai::parse_proposal(&reply) {
        Ok(batch) => match state.apply_proposal(&batch) {
            Ok(n) => ok(json!({
                "reply": reply,
                "model": cfg.model,
                "applied": true,
                "ops_applied": n,
                "entity_count": state.entity_count(),
            })),
            Err(e) => err(422, format!("proposal apply failed: {e}")),
        },
        Err(e) => err(
            422,
            json!({
                "error": format!("model reply was not a valid proposal: {e}"),
                "model": cfg.model,
                "reply": reply,
            }),
        ),
    }
}

/// `/frame` offscreen capture. Requires the `capture` feature (wgpu) + a GPU
/// adapter; returns a typed 503 otherwise, never a crash.
#[cfg(feature = "capture")]
fn frame(state: &HarnessState, _path: &str) -> (u16, Value) {
    // Parse optional w/h from the query; dispatch already passed path w/o query,
    // so accept compact sizes only (the CLI /see requests /frame directly).
    let w = 640u32;
    let h = 480u32;
    let camera = openengine_editor::camera::EditorCamera::default();
    match openengine_capture::capture_world_png(state.world(), &camera, w, h) {
        Ok(png) => {
            use base64::Engine as _;
            let b64 = base64::engine::general_purpose::STANDARD.encode(&png);
            ok(json!({ "png_base64": b64, "mime": "image/png", "width": w, "height": h }))
        }
        Err(e) => err(503, format!("no-adapter: {e}")),
    }
}

/// `/frame` stub when the crate is built without the capture feature (headless
/// CI). Typed, never a crash.
#[cfg(not(feature = "capture"))]
fn frame(_state: &HarnessState, _path: &str) -> (u16, Value) {
    err(
        503,
        "no-adapter: harness built without the 'capture' feature (no wgpu). Rebuild with --features capture on a GPU machine.",
    )
}
