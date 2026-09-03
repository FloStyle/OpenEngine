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

/// Dispatch one request. `body` is the raw (already-read) request body.
pub fn dispatch(state: &mut HarnessState, method: &str, path: &str, body: &[u8]) -> (u16, Value) {
    match (method, path) {
        ("GET", "/health") => ok(json!({
            "status": "ok",
            "version": VERSION,
            "headless": true,
            "capabilities": ["observe", "spawn", "despawn", "set", "tick", "hash", "load_wasm"],
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
                {"method":"POST","path":"/load_wasm","body":"{\"path\":\"...\"}"}
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
        _ => err(404, format!("no route: {method} {path}")),
    }
}
