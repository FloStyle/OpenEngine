---
spec: "17-testing-strategy"
phase: "All (governance, evergreen)"
status: "design"
---

# Testing Strategy

## Overview

Testing is not a phase in OpenEngine — it is the enforcement arm of the
**Prime Directive**. Because gameplay logic (Domain B) is a pure function of its
inputs and state is immutable, the test suite is the referee that proves it:
deterministic, bit-identical, portable, and **headless** (no GPU for any logic
test). This spec is the map of *which* test fights *which* invariant, and
*when* it runs (locally vs CI vs release gate). It implements and expands
`AGENTS.md §6 (Testing Protocol)` and `.agents/SECURITY.md`'s verification
checklist.

The rule of thumb that keeps everything coherent:

> Every task that touches behavior ships with a **Test Protocol** — a named list
> of concrete commands. A change is *done* only when each command in that list
> passes. This spec tells you which command category a given property belongs to.

## Test layers

Tests are organized by **what they can see**, mirroring the domain split. Each
layer has its own crate location, tooling, and determinism contract.

| Layer | Where it lives | Host? | GPU? | Proves |
|-------|----------------|-------|------|--------|
| Unit | `#[cfg(test)]` mod in each crate | native | no | single pure function / small invariant |
| Domain B unit (guest logic) | `crates/logic-sandbox`, `crates/math` | native rlib | no | pure-system behavior on a host-built rlib |
| Property / proptest | `proptest` in Domain A+B rlibs | native | no | invariants over many randomized inputs |
| Fuzz (cargo-fuzz) | `fuzz/` per crate (Domain B safety) | native + wasm | no | no panics / no budget violation |
| Host↔guest bridge | `crates/core` integration tests | native | no | host drives the wasm guest via the safe bridge |
| Determinism | dedicated tests run **3×** | native | no | bit-identical outputs |
| Headless end-to-end | `tests/` integration | native | no | full tick pipeline without a window |

**Rule:** Domain B logic is *unit-tested twice* — once as a native rlib (fast,
debuggable) and once compiled to `wasm32-unknown-unknown` and driven by a host
test through `wasmtime`. The native run catches logic bugs; the wasm run catches
what the `no_std` artifact actually does. Both must pass.

## Domain B unit tests (the heart)

Domain B is `#![no_std]` on the wasm target, but the **same source** builds as a
native rlib for tests (see `crates/logic-sandbox/src/lib.rs`: the `std` build is
a test-only convenience, never the shipped artifact). So logic tests are plain
Rust `#[test]` functions that call pure systems with a constructed `StateView`
and assert on the returned `WorldDelta`.

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use openengine_contracts::{DeferredCommand, StateView};

    #[test]
    fn tick_color_is_deterministic() {
        let view = StateView::tick_only(37);
        let a = tick_color(&view).unwrap();
        let b = tick_color(&view).unwrap();
        // Identical inputs -> bit-identical outputs. This is the WHOLE point.
        let ra = a.clear_color().unwrap();
        let rb = b.clear_color().unwrap();
        assert_eq!(ra, rb);
    }
}
```

Guidance for writing Domain B tests:

- Build a `StateView` from **literals** (`StateView::tick_only(tick)` or a
  hand-built arena) — never from wall-clock or shared global state.
- Assert the *full* returned delta (`spawns`, `despawns`, `writes`, `deferred`)
  — not just one field — so an unexpected side effect fails the test.
- Prefer integer / fixed-point assertions over `approx`; the engine is exact by
  construction.
- Test the error path: feed an impossible `StateView` (e.g. a column descriptor
  past the arena) and assert you get a `RecoverableError` with the right
  numeric `code`, never a panic.

### Host-driven guest tests (the safe bridge)

The real proof that Domain B behaves identically behind the wasm wall is a
**Domain A integration test that loads the module and drives it**. This uses
exactly the production bridge (`wasmtime`, guest-allocated buffer, `Memory::write`,
`cast_slice`) — no mock, no shortcut:

```rust
// crates/core/tests/guest_tick.rs (integration test, no GPU, no window)
#[test]
fn guest_tick_matches_native_result() {
    let module = openengine_core::load_logic(assets_path("logic.wasm"));
    // Run the guest system at tick 5 via the safe bridge...
    let guest_rgba = drive_guest_tick(&module, 5);
    // ...and run the same pure fn natively on the rlib.
    let view = StateView::tick_only(5);
    let native_rgba = logic_sandbox::tick_color(&view).unwrap().clear_color();
    // The wasm artifact must be bit-identical to the native rlib.
    assert_eq!(guest_rgba, native_rgba);
}
```

This is the single most valuable integration test in the repo: it proves the
bridge neither drops nor reorders data and that the `no_std` artifact agrees
with the host-compiled rlib bit-for-bit.

## Property-based and proptest coverage

`proptest` is the default for property checks because it runs inside `cargo
test` with no extra daemon (fuzz needs a driver + nightly). Use it for:

- **SoA math**: `Position += Velocity * delta` preserves representable bounds.
- **Wire codec**: any `WorldDelta` round-trips through `postcard` losslessly
  (`encode_delta`/`decode_delta` round-trip identity).
- **Column packing**: for random `indices`/`payload`, `ColumnWrite` byte length
  always equals `count * element_size` (the zero-copy hand-off contract).
- **No panic on adversarial bytes**: random byte strings fed to `decode_delta`
  return `Err`, never panic.

```rust
proptest! {
    #[test]
    fn delta_roundtrips(delta in any::<WorldDelta>()) {
        let bytes = contracts::encode_delta(&delta).unwrap();
        let back = contracts::decode_delta(&bytes).unwrap();
        prop_assert_eq!(delta, back);
    }
}
```

### cargo-fuzz (long-running, on wasm builds)

When a security-sensitive or parser-heavy code path lands (e.g. `decode_delta`
over untrusted host data, a future bytecode deserializer), add a `cargo-fuzz`
target under `fuzz/`. Fuzz inputs go **directly at the boundary that consumes
host data** so a malicious host payload can never panic the guest. These are
nightly/release-gate jobs, not part of the fast per-PR cycle.

## Determinism tests (run 3×, bit-identical)

Determinism is a product requirement, so it gets its **own test category**, not
just a convention. Mark tests with `#[ignore]`-style discipline only where
warranted — most determinism checks are fast and run in CI normally.

A determinism test:

1. Constructs a fully-seeded world / `StateView` (fixed tick, fixed seed).
2. Runs N fixed ticks, serializing state each step.
3. Runs the *whole sequence again* (same process), serializing to a fresh vec.
4. Asserts the two byte streams are **identical** (`assert_eq!` on `Vec<u8>`).

Run **3 separate times** (see Testing Protocol §5). `cargo test` may cache a
pass; a flaky nondeterminism can slip through once. The 3× rule is about
reproducing the same answer across independent executions.

```rust
fn fingerprint_after(seed: u64, ticks: u64) -> Vec<u8> {
    // Build a deterministic world, run `ticks` fixed steps, return a byte hash
    // of the terminal ECS arena + any deferred commands.
}
#[test]
fn simulation_is_bit_identical_across_runs() {
    let a = fingerprint_after(0xC0FFEE, 1000);
    let b = fingerprint_after(0xC0FFEE, 1000); // same seed, same ticks
    assert_eq!(a, b, "Domain B simulation must be bit-identical run-to-run");
}
```

Also assert **cross-input** determinism where relevant: for the same inputs the
guest-wasm path and the native-rlib path produce identical bytes (see bridge
test above). `postcard` is little-endian and non-floating, so a wire
fingerprint is stable across `x86_64-linux` and `aarch64-linux` — that cross-
architecture identity is exactly what the CI determinism job (spec `20-ci-cd`)
checks.

## Purity verification as a gate

Purity is **not** proven by the Rust compiler alone (nothing stops a crate from
`unsafe`-importing WASI under a different name), so it is enforced structurally
by the Brain:

```bash
python3 brain/orchestrator.py verify-wasm-purity crates/core/assets/logic.wasm
# must print:  [PURE] crates/core/assets/logic.wasm
```

The Brain disassembles the module (via `wasm-tools` when present) and rejects
any import from `std`, WASI (`wasi_snapshot_preview1`, `wasi_unstable`), `env`,
or a Domain A crate. `verify-deps` forbids `wgpu`/`winit`/`wasmtime`/`egui`/
`rayon` from appearing in Domain B manifests:

```bash
python3 brain/orchestrator.py verify-deps      # [PURE] domain-B manifests
```

This is a **gate**: a change touching Domain B that leaves `[PURE]` failing is
not done, regardless of green unit tests. Before you can verify purity you must
produce the artifact:

```bash
bash scripts/build.sh      # builds openengine-logic-export for wasm32-unknown-unknown,
                           # stages it at crates/core/assets/logic.wasm
```

`scripts/requirements.sh` checks the local toolchain (rustc, cargo,
`wasm32-unknown-unknown`, python3, docker, wasm-tools) so a fresh agent knows
what a clean environment looks like.

## Headless logic tests (no GPU)

The Determinism Law + portability requirement mean Domain B tests must **never
open a window or touch the GPU**. The full ECS/bridge test path runs as plain
native tests on a headless runner or in Docker. Anything under `crates/core`/
`crates/ecs`/`crates/logic-sandbox`/`crates/math` is GPU-free. Only the truly
visual `crates/core` renderer smoke tests (a window + a `wgpu` surface) require
a display, and those are gated behind an explicit feature or `#[ignore]` so CI
and Docker skip them. CI never exercises them.

## Linting, format, and cross-target compilation

Static gates run in CI but must also pass locally before any commit. Because
these are cheap they gate every PR, not just releases:

```bash
cargo fmt --all -- --check            # rustfmt applied
cargo clippy --workspace --all-targets -- -D warnings   # deny(clippy::all) inherited
cargo build --workspace               # native, all crates
cargo build --workspace --target wasm32-unknown-unknown # Domain B target compiles
cargo check --workspace --target aarch64-unknown-linux-gnu  # cross-target (best-effort)
```

- `clippy::all = deny` is inherited via `[workspace.lints]` (see root
  `Cargo.toml`). A scoped `#[allow]` needs an inline reason; it is never used
  just to turn CI green.
- `unsafe_code = "forbid"` is the workspace default. A Domain A crate that
  needs a narrow carve-out is RFC-gated into one reviewed module (see
  `AGENTS.md §4`).

## When to run what

| Scenario | Minimum command set |
|----------|---------------------|
| Every commit (local, before push) | `cargo fmt --check` · `cargo clippy -D warnings` · `cargo test --workspace` |
| Any Domain B change | + `bash scripts/build.sh` · `python3 brain/orchestrator.py verify-wasm-purity crates/core/assets/logic.wasm` (→ `[PURE]`) · `verify-deps` |
| Task with a Test Protocol | Run **every** command named in that task's protocol |
| Blocking / definition-of-done | Full stack + determinism 3× + Docker build |
| Release gate | Full stack + fuzz + `cargo audit` + cross-target + wasm purity |

### Definition of Done mapping (from `AGENTS.md §9`)

| DoD item | Covered by |
|----------|-----------|
| `ARCH_VERSION` unchanged or bumped w/ `docs/abi/` | code review + `contracts` unit test |
| `cargo build`/`test --workspace` pass | per-commit set |
| `clippy -D warnings` clean | per-commit set |
| Domain B compiles for wasm + `[PURE]` | Domain B set |
| `rustfmt` applied | per-commit set |
| No hardcoded paths/credentials/OS-specific | portability tests + review |
| Behavior recorded in `docs/` | see spec `18-documentation` |

## Test command cheat-sheet

```bash
# Fast per-commit (always):
cargo fmt --all -- --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace

# Domain B (produce + verify artifact):
bash scripts/build.sh
python3 brain/orchestrator.py verify-wasm-purity crates/core/assets/logic.wasm   # -> [PURE]
python3 brain/orchestrator.py verify-deps

# Determinism (run the deterministic tests 3×, bit-identical).
# 'determinism' is a substring filter — do NOT add --exact, which only matches a
# test literally named "determinism" and would run zero tests. The guard below
# asserts each run actually executed >= 1 determinism test.
for i in 1 2 3; do
  out="$(cargo test --workspace determinism)" || exit 1
  printf '%s\n' "$out" | grep -q 'test result: ok' \
    || { echo "FAIL: determinism run $i executed no tests"; exit 1; }
done

# Reproducible environment / full gate:
bash scripts/requirements.sh
docker build -t openengine-test .
docker run --rm openengine-test                  # cargo test --workspace, headless

# Property tests (part of cargo test if proptest cases are capped):
cargo test --workspace proptest

# Fuzz / security (release gate, nightly):
cargo +nightly fuzz run decode_delta             # if a fuzz target exists
cargo audit                                       # security-sensitive changes

# Cross-target:
cargo check --workspace --target aarch64-unknown-linux-gnu
```

## CI ↔ local division of labor

CI (spec `20-ci-cd`) and the local loop are **two halves of one policy**, not
duplicates. Locally an agent runs the fast set to iterate; CI runs the same
fast set on every PR plus the slow gates (fuzz, audit, cross-arch determinism,
Docker) that no single machine should block on. A change is only *done* when
both the local loop and the CI checks are green; the branch/PR policy in
`20-ci-cd` prevents an agent from merging without those checks.

## Dependencies
- `proptest` (workspace dev-dep), `cargo-fuzz` (nightly, fuzz targets),
  `wasm-tools` (optional, structural purity), `wasmtime` (host test runtime,
  Domain A), `python3` (Brain), Docker (reproducible gate).

## Next steps
1. Land a proptest target for the `WorldDelta`/`ColumnWrite` packing contract.
2. Land the host-driven guest integration test (`guest_tick_matches_native`).
3. Stand up the fuzz target for `decode_delta` over untrusted payloads.
4. Wire the determinism 3× job and cross-arch determinism job into CI (spec
   `20-ci-cd`).
