//! Harness core API round-trip + determinism tests (headless, CI-safe).
//!
//! Uses [`openengine_harness::api::dispatch`] directly (no HTTP needed) for the
//! round-trip / determinism assertions, plus one real socket smoke test against
//! an ephemeral-port server.

use openengine_harness::api;
use openengine_harness::{bind, serve, HarnessState};
use serde_json::Value;
use std::io::{Read, Write};
use std::net::TcpStream;

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
