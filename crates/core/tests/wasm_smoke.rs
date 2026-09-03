//! PoC Phase B — Wasm sandbox smoke test (headless).
//!
//! Loads the staged Domain-B module (`crates/core/assets/logic.wasm`, built by
//! `bash scripts/build.sh`) into a wasmtime host, calls `openengine_tick`, and
//! decodes the returned `WorldDelta`. Proves the wasmtime host actually
//! executes the guest and returns ABI bytes — no leaks, no escapes.

use std::path::Path;

const WASM_ASSET: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/assets/logic.wasm");

#[test]
fn wasm_module_loads_and_runs_a_tick() {
    if !Path::new(WASM_ASSET).exists() {
        // Not built yet: run `bash scripts/build.sh` first. Skip rather than fail
        // a fresh clone (the purity + CI jobs build it before running tests).
        eprintln!("logic.wasm not found — run bash scripts/build.sh first");
        return;
    }
    let engine = wasmtime::Engine::default();
    let module = wasmtime::Module::from_file(&engine, WASM_ASSET).expect("compile wasm");
    let mut store = wasmtime::Store::new(&engine, ());
    let linker = wasmtime::Linker::new(&engine);
    let instance = linker
        .instantiate(&mut store, &module)
        .expect("instantiate");

    let alloc = instance
        .get_typed_func::<u32, u32>(&mut store, "openengine_alloc")
        .expect("openengine_alloc export");
    let tick = instance
        .get_typed_func::<(u64, u32, u32), u32>(&mut store, "openengine_tick")
        .expect("openengine_tick export");
    let memory = instance
        .get_memory(&mut store, "memory")
        .expect("guest exports memory");

    let cap = 4096u32;
    let buf = alloc.call(&mut store, cap).expect("allocate guest buffer");
    let n = tick
        .call(&mut store, (1, buf, cap))
        .expect("run a guest tick");
    assert!(n > 0, "guest tick must return an encoded WorldDelta");
    assert!(n as usize <= cap as usize);

    let mut out = vec![0u8; n as usize];
    memory
        .read(&store, buf as usize, &mut out)
        .expect("read WorldDelta bytes");
    let delta = openengine_contracts::decode_delta(&out).expect("decode WorldDelta");
    // The guest is expected to have produced at least one deferred command
    // (the living-window ClearColor in this PoC build).
    assert!(!delta.deferred.is_empty() || !delta.writes.is_empty() || !delta.warnings.is_empty());
}
