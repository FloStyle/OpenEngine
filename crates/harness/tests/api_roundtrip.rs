//! Harness core API round-trip + determinism tests (headless, CI-safe).
//!
//! Uses [`openengine_harness::api::dispatch`] directly (no HTTP needed) for the
//! round-trip / determinism assertions, plus one real socket smoke test against
//! an ephemeral-port server.

use openengine_harness::api;
use openengine_harness::{bind, serve, HarnessState};
use serde_json::{json, Value};
use std::io::{Read, Write};
use std::net::TcpStream;
use std::sync::Mutex;

// Tests that mutate the process-global OPENENGINE_AI_CONFIG env var must not run
// in parallel with each other (env is process-wide). Serialize them via a mutex.
static ENV_MUTEX: Mutex<()> = Mutex::new(());

const WASM_ASSET: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/../core/assets/logic.wasm");

fn post(state: &mut HarnessState, path: &str, body: &str) -> (u16, Value) {
    api::dispatch(state, "POST", path, body.as_bytes())
}
fn get(state: &mut HarnessState, path: &str) -> (u16, Value) {
    api::dispatch(state, "GET", path, b"")
}

fn hex(v: &Value) -> String {
    v.get("hash")
        .and_then(|x| x.as_str())
        .unwrap_or("")
        .to_string()
}

#[test]
fn observe_spawn_set_tick_hash_roundtrip() {
    let mut s = HarnessState::new();
    // /health.
    let (c, h) = get(&mut s, "/health");
    assert_eq!(c, 200);
    assert_eq!(h["status"], "ok");
    // spawn two entities.
    let (c, r) = post(
        &mut s,
        "/spawn",
        r#"{"transform":[1,0,0],"color":[255,0,0,255]}"#,
    );
    assert_eq!(c, 200, "spawn failed: {r}");
    let e0 = r["entity"].as_u64().unwrap();
    let (_, r) = post(
        &mut s,
        "/spawn",
        r#"{"transform":[-2,0,3],"scale":[2,1,1],"color":[0,255,0,255]}"#,
    );
    assert_eq!(r["entity"].as_u64().unwrap(), e0 + 1);
    // observe reflects 2.
    let (c, o) = get(&mut s, "/observe");
    assert_eq!(c, 200);
    assert_eq!(o["entity_count"], 2);
    let first = &o["entities"][0];
    assert_eq!(first["index"], e0);
    assert_eq!(first["color"][0], 255);
    // set transform of entity 0.
    let (c, r) = post(
        &mut s,
        "/set",
        &format!(r#"{{"entity":{e0},"component":"transform","value":[9,9,9]}}"#),
    );
    assert_eq!(c, 200, "set failed: {r}");
    let (_, o) = get(&mut s, "/observe");
    assert_eq!(o["entities"][0]["transform"][0], 9.0);
    // tick advances + returns a hash.
    let (c, r) = post(&mut s, "/tick", r#"{"n":25}"#);
    assert_eq!(c, 200);
    assert_eq!(r["ticks"], 25);
    assert!(!hex(&r).is_empty());
    // hash endpoint matches.
    let (_, h) = get(&mut s, "/hash");
    assert_eq!(hex(&h), hex(&r));
}

#[test]
fn determinism_two_identical_runs_identical_hash() {
    let run = || -> String {
        let mut s = HarnessState::new();
        post(
            &mut s,
            "/spawn",
            r#"{"transform":[0,0,0],"color":[255,255,255,255]}"#,
        );
        post(
            &mut s,
            "/spawn",
            r#"{"transform":[5,0,0],"color":[1,2,3,255]}"#,
        );
        post(
            &mut s,
            "/set",
            r#"{"entity":0,"component":"scale","value":[3,3,3]}"#,
        );
        let (_, r) = post(&mut s, "/tick", r#"{"n":100}"#);
        hex(&r)
    };
    let a = run();
    let b = run();
    assert_eq!(
        a, b,
        "identical input sequences must produce identical hashes"
    );
}

#[test]
fn wasm_guest_tick_runs_and_is_deterministic() {
    if !std::path::Path::new(WASM_ASSET).exists() {
        eprintln!("SKIP: {WASM_ASSET} absent (run bash scripts/build.sh)");
        return;
    }
    let guest = || -> String {
        let mut s = HarnessState::new();
        post(
            &mut s,
            "/spawn",
            r#"{"transform":[0,0,0],"color":[255,255,255,255]}"#,
        );
        let (c, r) = post(
            &mut s,
            "/load_wasm",
            &format!(r#"{{"path":"{WASM_ASSET}"}}"#),
        );
        assert_eq!(c, 200, "load_wasm failed: {r}");
        assert_eq!(r["engine"], "wasm");
        let (_, r) = post(&mut s, "/tick", r#"{"n":200}"#);
        hex(&r)
    };
    let a = guest();
    let b = guest();
    assert!(!a.is_empty());
    assert_eq!(a, b, "guest ticks must be deterministic across fresh runs");
}

#[test]
fn server_binds_ephemeral_and_answers_health() {
    let server = bind("127.0.0.1:0").expect("bind ephemeral");
    let addr = server.server_addr().to_string();
    let state = HarnessState::new();
    std::thread::spawn(move || serve(server, state));

    let mut sock = TcpStream::connect(&addr).expect("connect");
    let req = "GET /health HTTP/1.1\r\nHost: x\r\nConnection: close\r\n\r\n";
    sock.write_all(req.as_bytes()).expect("write");
    let mut buf = String::new();
    sock.read_to_string(&mut buf).expect("read");
    assert!(buf.starts_with("HTTP/1.1 200"), "bad status line:\n{buf}");
    assert!(
        buf.contains("\"status\":\"ok\"") || buf.contains("\"status\": \"ok\""),
        "no ok: {buf}"
    );
}

#[test]
fn prove_reports_determinism() {
    let mut s = HarnessState::new();
    post(
        &mut s,
        "/spawn",
        r#"{"transform":[0,0,0],"color":[255,255,255,255]}"#,
    );
    post(
        &mut s,
        "/spawn",
        r#"{"transform":[3,0,0],"color":[1,2,3,255]}"#,
    );
    let (c, r) = post(&mut s, "/prove", r#"{"n":200}"#);
    assert_eq!(c, 200, "prove failed: {r}");
    assert_eq!(r["equal"], true, "identical fresh replays must match: {r}");
    assert_eq!(r["hash_a"], r["hash_b"]);
}

#[test]
fn transaction_applies_batch_atomically() {
    let mut s = HarnessState::new();
    let (c, r) = post(
        &mut s,
        "/transaction",
        r#"{"ops":[
            {"path":"/spawn","body":{"transform":[1,0,0],"color":[255,0,0,255]}},
            {"path":"/spawn","body":{"transform":[2,0,0],"color":[0,255,0,255]}}
        ]}"#,
    );
    assert_eq!(c, 200, "transaction failed: {r}");
    assert_eq!(r["applied"], 2);
    assert_eq!(s.entity_count(), 2);
}

#[test]
fn transaction_rolls_back_on_failing_op() {
    let mut s = HarnessState::new();
    post(
        &mut s,
        "/spawn",
        r#"{"transform":[0,0,0],"color":[255,255,255,255]}"#,
    );
    let before = s.entity_count();
    // Op 1 succeeds (spawn), op 2 fails (despawn of nonexistent entity 99).
    let (c, r) = post(
        &mut s,
        "/transaction",
        r#"{"ops":[
            {"path":"/spawn","body":{"transform":[9,9,9],"color":[0,0,0,255]}},
            {"path":"/despawn","body":{"entity":99}}
        ]}"#,
    );
    assert_eq!(c, 409, "expected rollback, got: {r}");
    assert_eq!(
        s.entity_count(),
        before,
        "failed transaction must roll back fully"
    );
    // World unchanged: still one entity at origin.
    let (_, o) = get(&mut s, "/observe");
    assert_eq!(o["entity_count"], before);
    assert_eq!(o["entities"][0]["transform"][0], 0.0);
}

#[test]
fn save_then_load_preserves_world_bit_for_bit() {
    let mut s = HarnessState::new();
    post(
        &mut s,
        "/spawn",
        r#"{"transform":[0,0,0],"scale":[1,1,1],"color":[10,20,30,255]}"#,
    );
    post(
        &mut s,
        "/spawn",
        r#"{"transform":[-4,2,7],"scale":[2,1,1],"color":[200,0,90,255]}"#,
    );
    let h_before = format!("{:016x}", s.hash());

    let path =
        std::env::temp_dir().join(format!("openengine_scene_test_{}.json", std::process::id()));
    let p = path.to_str().unwrap();
    let (c, r) = post(&mut s, "/save", &format!(r#"{{"path":"{p}"}}"#));
    assert_eq!(c, 200, "save failed: {r}");
    assert_eq!(r["entities"], 2);

    let mut fresh = HarnessState::new();
    let (c, r) = post(&mut fresh, "/load", &format!(r#"{{"path":"{p}"}}"#));
    assert_eq!(c, 200, "load failed: {r}");
    let h_after = format!("{:016x}", fresh.hash());
    assert_eq!(
        h_before, h_after,
        "save/load round-trip must preserve the world bit-for-bit"
    );
    let _ = std::fs::remove_file(&path);
}

#[test]
fn scene_round_trip_then_ticks_stay_deterministic() {
    let mut s = HarnessState::new();
    post(
        &mut s,
        "/spawn",
        r#"{"transform":[1,0,0],"color":[255,255,255,255]}"#,
    );
    let path =
        std::env::temp_dir().join(format!("openengine_scene_det_{}.json", std::process::id()));
    let p = path.to_str().unwrap();
    let (c0, _) = post(&mut s, "/save", &format!(r#"{{"path":"{p}"}}"#));
    assert_eq!(c0, 200);

    let mut a = HarnessState::new();
    let (_, _) = post(&mut a, "/load", &format!(r#"{{"path":"{p}"}}"#));
    let mut b = HarnessState::new();
    let (_, _) = post(&mut b, "/load", &format!(r#"{{"path":"{p}"}}"#));
    let (ca, _) = post(&mut a, "/tick", r#"{"n":300}"#);
    let (cb, _) = post(&mut b, "/tick", r#"{"n":300}"#);
    assert_eq!(ca, 200);
    assert_eq!(cb, 200);
    let (_, oa) = get(&mut a, "/hash");
    let (_, ob) = get(&mut b, "/hash");
    assert_eq!(
        hex(&oa),
        hex(&ob),
        "two loads of the same scene must tick identically"
    );
    let _ = std::fs::remove_file(&path);
}

#[test]
fn snapshot_restore_forks_and_rolls_back_in_memory() {
    let mut s = HarnessState::new();
    post(
        &mut s,
        "/spawn",
        r#"{"transform":[1,2,3],"color":[10,20,30,255]}"#,
    );
    let h0 = format!("{:016x}", s.hash());
    let (c, snap) = get(&mut s, "/snapshot");
    assert_eq!(c, 200);
    post(
        &mut s,
        "/spawn",
        r#"{"transform":[9,9,9],"color":[200,200,200,255]}"#,
    );
    assert_eq!(s.entity_count(), 2);
    let body = serde_json::to_string(&json!({ "snapshot": snap })).unwrap();
    let (c, r) = post(&mut s, "/restore", &body);
    assert_eq!(c, 200, "restore failed: {r}");
    assert_eq!(s.entity_count(), 1, "restore must undo the spawn");
    let h1 = format!("{:016x}", s.hash());
    assert_eq!(
        h0, h1,
        "restore must reproduce the original world bit-for-bit"
    );
}

// Heavy (spawns cargo build/test + purity); kept out of the default fast set.
#[test]
#[ignore]
fn verify_returns_structured_verdict() {
    let mut s = HarnessState::new();
    let (c, r) = get(&mut s, "/verify");
    assert_eq!(c, 200);
    assert!(
        r.get("status").is_some(),
        "verify must return a status: {r}"
    );
    assert!(r.get("build").is_some() && r.get("tests").is_some() && r.get("purity").is_some());
    assert!(r.get("determinism").is_some());
}

#[test]
fn schema_lists_spec21_components() {
    let mut s = HarnessState::new();
    let (c, r) = get(&mut s, "/schema");
    assert_eq!(c, 200);
    let comps = r["components"].as_array().expect("components array");
    // At least the engine-band components an agent edits via /set are present.
    let by_id: std::collections::HashMap<u64, &Value> = comps
        .iter()
        .map(|c| (c["id"].as_u64().unwrap(), c))
        .collect();
    for want in [
        2u64, /*Transform*/
        72,   /*Color*/
        80,   /*Velocity3D*/
        81,   /*Actor*/
    ] {
        assert!(
            by_id.contains_key(&want),
            "/schema must list component id {want}, got {r}"
        );
    }
    // Name<->id index lets an agent address a component by name.
    assert_eq!(r["component_ids"]["Transform"], 2);
    assert_eq!(r["component_ids"]["Actor"], 81);
    // A size is exposed for each component.
    for c in comps {
        assert!(c["size"].as_u64().is_some(), "component needs a size: {c}");
        assert!(
            c["fields"]
                .as_array()
                .map(|f| !f.is_empty())
                .unwrap_or(false),
            "component needs field names: {c}"
        );
    }
}

#[test]
fn ai_status_never_network_and_config_aware() {
    let mut s = HarnessState::new();
    let (c, r) = get(&mut s, "/ai/status");
    assert_eq!(c, 200);
    // configured may be true or false depending on env, but shape is fixed.
    assert!(
        r.get("configured").is_some(),
        "ai/status needs 'configured': {r}"
    );
}

#[test]
fn ai_status_reflects_config_when_env_set() {
    let _guard = ENV_MUTEX.lock().unwrap();
    // Point OPENENGINE_AI_CONFIG at a temp Local config -> configured:true.
    let tmp = std::env::temp_dir().join("ai_status_cfg.json");
    std::fs::write(
        &tmp,
        r#"{"provider":{"kind":"local","endpoint":"http://127.0.0.1:8889/v1"},"model":"m"}"#,
    )
    .unwrap();
    std::env::set_var("OPENENGINE_AI_CONFIG", tmp.to_str().unwrap());
    let mut s = HarnessState::new();
    let (c, r) = get(&mut s, "/ai/status");
    assert_eq!(c, 200);
    assert_eq!(r["configured"], true);
    assert_eq!(r["provider"], "llama.cpp");
    std::env::remove_var("OPENENGINE_AI_CONFIG");
    let _ = std::fs::remove_file(&tmp);
}

// /frame needs wgpu (capture feature) — only test the typed stub here; the
// real capture test lives in crates/capture and skips when no GPU.
#[test]
fn frame_returns_typed_response_or_png() {
    let mut s = HarnessState::new();
    let (c, r) = get(&mut s, "/frame");
    // Either 200 (capture feature + GPU) or a typed 503 no-adapter.
    if c == 200 {
        assert!(
            r.get("png_base64").is_some(),
            "200 /frame must carry png_base64"
        );
        assert_eq!(r["mime"], "image/png");
    } else {
        assert_eq!(c, 503, "/frame without GPU/capture -> 503, got {c}: {r}");
    }
}

#[test]
fn ask_without_config_returns_409() {
    let _guard = ENV_MUTEX.lock().unwrap();
    // No OPENENGINE_AI_CONFIG + no config/ai.json -> typed 409, no network.
    std::env::remove_var("OPENENGINE_AI_CONFIG");
    let mut s = HarnessState::new();
    let (c, r) = post(&mut s, "/ask", r#"{"message":"hello"}"#);
    assert_eq!(c, 409, "no-model-config must 409, got {r}");
    assert!(r["error"]
        .as_str()
        .unwrap_or("")
        .contains("no model configured"));
}

#[test]
fn apply_proposal_spawns_and_rolls_back_on_failure() {
    let mut s = HarnessState::new();
    // A valid batch: spawn two + despawn the first -> net +1.
    let ok_batch: openengine_ai::ProposeBatch = serde_json::from_str(
        r#"{"ops":[
            {"op":"spawn","transform":[1,0,0],"color":[255,0,0,255]},
            {"op":"spawn","transform":[2,0,0],"color":[0,255,0,255]},
            {"op":"despawn","entity":0}
        ]}"#,
    )
    .unwrap();
    let n = s.apply_proposal(&ok_batch).expect("batch applies");
    assert_eq!(n, 3);
    assert_eq!(s.entity_count(), 1);

    // A failing batch (despawn out of range) must roll back to the pre-state.
    let s0 = s.entity_count();
    let bad_batch: openengine_ai::ProposeBatch = serde_json::from_str(
        r#"{"ops":[{"op":"spawn","transform":[9,9,9],"color":[1,2,3,255]},{"op":"despawn","entity":99}]}"#,
    )
    .unwrap();
    assert!(s.apply_proposal(&bad_batch).is_err());
    assert_eq!(s.entity_count(), s0, "failed proposal must roll back");
}

#[test]
fn parse_proposal_and_apply_roundtrip() {
    // Model-shaped reply -> parse -> apply through the mutation channel.
    let reply = r#"{"ops":[{"op":"spawn","transform":[0,0,0],"color":[9,9,9,255]}]}"#;
    let mut s = HarnessState::new();
    let batch = openengine_ai::parse_proposal(reply).expect("parse");
    let n = s.apply_proposal(&batch).expect("apply");
    assert_eq!(n, 1);
    assert_eq!(s.entity_count(), 1);
}
