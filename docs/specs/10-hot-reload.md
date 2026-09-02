---
spec: "10-hot-reload"
phase: "Phase 5"
status: "design"
---

# Hot Reload

## Overview

Development-only hot reload of the Domain B wasm module **and** of game assets
while the engine runs. A file watcher on `crates/core/assets/` (and the
source tree feeding it) notices a change, rebuilds the wasm through the existing
build bridge (`bash scripts/build.sh`), and swaps the freshly loaded module into
the `wasmtime` runtime **between fixed ticks**, preserving world state. Failed
builds or failed loads fall back to the last-good module so the editor never
dies mid-authoring.

Hot reload is gated to development builds. A release build compiles it out
entirely: no watcher threads, no rebuild subprocess, no `notify` dependency
reachable from a shipped artifact.

## Determinism guard: reload between ticks only

The cardinal rule is that reloads happen **between fixed ticks**, never mid-tick.
Domain B runs as a sequence of pure
`fn(&StateView) -> Result<WorldDelta, RecoverableError>` calls; a mid-tick swap
would leave two systems of the same tick running against two different module
generations and break the determinism guarantee. So the reload is queued:

```rust
pub enum ReloadRequest {
    None,
    Logic { module_bytes: Vec<u8>, fingerprint: u64 },
    Assets { changed: Vec<AssetPath>, kind: AssetKind },
}

impl HotReloader {
    /// Called by the watcher thread when a change lands. Only *records* intent;
    /// it never touches the wasmtime instance or the asset cache from here.
    pub fn stage_change(&self, kind: WatchEventKind) { /* push to a pending queue */ }
}
```

The owning `GameLoop` consumes the pending request at a **safe point** — after
`world.flush()` and before the accumulator loop begins its next `fixed_update`:

```rust
fn fixed_update(&mut self) {
    self.pre_update();
    let view = self.logic.build_state_view(&self.world, self.sim_time);
    for sys in self.phases.iter() { /* run pure systems, apply deltas */ }
    self.world.flush();

    // SAFE POINT: no pure system is mid-flight; state is fully flushed.
    if let Some(r) = self.reloader.take_pending() {
        self.apply_reload(r);
    }
}
```

Because reload only lands at this boundary, the sequence of applied deltas stays
deterministic *within* a tick generation, and the next tick simply runs the new
module's systems against the flushed, unchanged world.

## Wasm rebuild pipeline

The reloader does not rebuild in-process. It shells out to the canonical build
bridge so there is exactly one way a wasm artifact is produced (keeps
`verify-wasm-purity` and the CI artifact authoritative):

```rust
pub struct HotReloader {
    watch_root: PathBuf,             // derived from OPENENGINE_ASSETS_PATH/WASM_PATH
    watcher: notify::RecommendedWatcher,
    current: LoadedModule,           // last-good wasm module
    last_good_bytes: Arc<[u8]>,      // retained for graceful fallback
    pending: Mutex<ReloadRequest>,   // only touched by staging / safe point
    enabled: bool,                   // false in release
}
```

Steps taken on a `logic.wasm` change:

1. **Debounce** (e.g. 150 ms) so a multi-write save stages one rebuild.
2. Run `bash scripts/build.sh` (or a configurable `OPENENGINE_BUILD_CMD`). If it
   exits non-zero, log the build output and **keep the last-good module**.
3. Read the staged `crates/core/assets/logic.wasm` bytes.
4. Verify the `abi_fingerprint()` constant embedded in the module matches the
   host build's `ARCH_VERSION`; a mismatch is a hard refusal (keep last-good).
5. Enqueue `ReloadRequest::Logic` for the next safe point.

At the safe point the loader replaces the wasmtime instance while carrying the
world over:

```rust
fn apply_reload(&mut self, req: ReloadRequest) {
    match req {
        ReloadRequest::Logic { module_bytes, fingerprint } => {
            let view = self.logic.build_state_view(&self.world, self.sim_time);
            // Instantiate the new module and run its exported systems against
            // the CURRENT flushed view; the returned delta is applied as usual.
            match self.logic.swap_module(&module_bytes) {
                Ok(()) => {
                    self.last_good_bytes = Arc::from(module_bytes);
                    log::info!("hot-reloaded logic (fp={fingerprint})");
                }
                Err(e) => {
                    // Restore the previously retained last-good bytes; the world
                    // was never touched, so nothing needs rolling back.
                    let _ = self.logic.swap_module(&self.last_good_bytes);
                    log::error!("reload rejected, keeping last-good: {e}");
                }
            }
        }
        ReloadRequest::Assets { .. } => self.assets.reload(req),
    }
}
```

World state lives **on the host** (Domain A ECS). `wasmtime` instances hold no
persistent simulation state between calls — each tick materializes a fresh
`StateView`. Swapping the module therefore cannot orphan guest-side state; the
world is simply read into the new module's view on the next tick. This is the
property that makes hot reload cheap and safe by construction.

## Asset reload invalidation

Non-wasm assets (meshes, textures, config) are watched the same way. On change
the reloader invalidates the matching entry in the host asset cache and marks
dependent render/editor resources for re-upload at the next frame boundary. The
invalidation is asset-scoped so a texture edit does not tear down geometry:

```rust
pub enum AssetKind { Mesh, Texture, Scene, Config }

pub struct AssetCache {
    entries: HashMap<AssetPath, Arc<LoadedAsset>>,   // host-side, Domain A
}

impl AssetCache {
    /// Swap one asset atomically; consumers holding the old Arc keep rendering
    /// the old data until they next query by path.
    pub fn invalidate(&mut self, path: AssetPath) -> Option<Arc<LoadedAsset>>;
}
```

Asset paths are always relative to `OPENENGINE_ASSETS_PATH` (default
`./assets/`); no absolute or home-relative paths, per the portability rule. The
watcher ignores transient writes and the engine's own output file so reloading
does not self-trigger.

## Graceful fallback matrix

| Event | Behaviour |
|-------|-----------|
| Build command exits non-zero | Log; keep last-good; no swap. |
| New module abi mismatch | Refuse swap; keep last-good; log fingerprint diff. |
| `wasmtime` instantiate / start error | Restore retained last-good bytes; world untouched. |
| New module panics on first tick | RecoverableError path rolls back that tick's delta; world stays consistent. |
| Asset load fails | Keep the previous `Arc<LoadedAsset>`; warn in editor console. |

## Disabled in release

A build-gated `cfg(debug_assertions)` (or an explicit `hot-reload` feature that
release profiles do not enable) removes the watcher, the rebuild spawn, the
`notify` dependency and the reload `Mutex` from the shipped binary. Release
simply loads the module once at startup and never re-reads it. CI runs the
release profile, so the watcher code path is compile-checked in debug and
proven-absent in release.

## Key Rust types

- `HotReloader`, `LoadedModule`, `ReloadRequest`, `WatchEventKind`.
- `AssetCache`, `LoadedAsset`, `AssetPath`, `AssetKind`.
- Build bridge boundary: `RebuildCmd { cmd, args }` run via `std::process`.

## Constraints

- Reloads land only at the post-flush safe point; never mid-tick.
- Only the last-good bytes are retained; no unbounded history.
- `abi_fingerprint` mismatch is a hard refusal (never load a foreign module).
- Watcher and asset cache are Domain A (`std`, `notify`); never reach Domain B.
- Relative paths only; derive roots from `OPENENGINE_*` env vars.
- Determinism preserved: same tick, same flushed world → same deltas regardless
  of module generation swap points.

## Performance

- Watcher overhead ≈ 0 when idle (inotify/kqueue, no polling).
- Debounce collapses bursts into one rebuild.
- Swap cost is a `wasmtime` module instantiation at a safe point; next tick pays
  its normal materialization cost. No per-frame reload cost.
- Asset invalidation is O(paths-touched); only dependent uploads re-run.

## Testing strategy

- Unit: `apply_reload` safe-point placement; build-failure → last-good;
  fingerprint mismatch → refusal; asset invalidation keeps old Arc on failure.
- Integration (headless, no GPU): touch a source file, run `scripts/build.sh`,
  assert the module generation advanced and a post-reload tick applied the new
  system's delta to a seeded world; run the identical tick before/after reload
  and verify determinism across 3 runs.
- Fallback test: feed a deliberately broken source; assert the engine keeps
  running the previous module and logs the error.
- Release test: `cargo build --release`; grep symbols to assert no `notify`
  watcher or reload paths are linked.

## Dependencies

- Domain A only: `notify`, `std::process` (build bridge), `wasmtime`,
  `contracts` (`abi_fingerprint`, `RecoverableError`). Domain B and the asset
  payloads are unchanged — reload never alters the ABI.

## Next steps

1. Watch roots derived from `OPENENGINE_*` env vars; debounced watcher.
2. Safe-point consumption in `GameLoop::fixed_update`.
3. `swap_module` in the wasmtime bridge + abi fingerprint gate.
4. Asset cache invalidation + dependent re-upload.
5. Release gating (`hot-reload` feature / `cfg(debug_assertions)`).
