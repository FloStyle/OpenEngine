# Technical Constraints

---
name: "Technical Constraints"
updated: "2026-09-03"
---

## Domain B (Logic Sandbox)
- `#![no_std]` + `#![forbid(unsafe_code)]` (workspace default; `forbid` is not
  overridable by `#[allow]`).
- Pure functions only: `fn(&StateView) -> Result<WorldDelta, RecoverableError>`.
- No `f32` in game logic (use `openengine-math` fixed-point).
- Allowed imports: `contracts`, `openengine-math`, `bytemuck`, `serde`,
  `postcard`, `alloc` (+ `spin` for transport-buffer mutexes).
- No global *simulation* state; no RNG without an explicit seed; no time-based
  logic; no `HashMap` iteration (use `BTreeMap`/sorted `Vec`); no network/fs;
  no threads. Transport buffers (never simulation state) may use `spin::Mutex`.
- No `#[no_mangle]` here — exports live in `crates/logic-export`.

## Domain A (Core)
- Allowed: `std`, `wgpu`, `wasmtime`, `winit`, `rayon`.
- `unsafe` only in isolated modules with a SAFETY comment (RFC-gated).
- Must apply `WorldDelta` atomically; never reach into Domain B logic memory.

## Cross-Domain Boundary
- Communication only via `contracts/` ABI types; serialization via `postcard`.
- Zero-copy reads use `bytemuck::cast_slice` over guest-owned memory.
- `ARCH_VERSION` tracks ABI changes; breaking change requires a bump +
  `docs/abi/` update + all consumers updated in the same commit.

## Performance
- Domain B: ≤ 16 ms/tick, ≤ 256 MB/module, no allocations in hot paths.
- Domain A: ≤ 8 worker threads, no `Mutex` in hot paths, prefer `&[T]`.

## Portability
- Compiles on `x86_64-linux` and `aarch64-linux`.
- No distribution-specific paths/commands; `$HOME` or relative only.
- Docker + CI + offline (after `cargo fetch`) support. No GPU for logic tests.
