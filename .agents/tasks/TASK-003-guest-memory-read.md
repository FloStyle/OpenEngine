---
task_id: "TASK-003"
title: "Guest reads SoA data from its own buffer (zero unsafe)"
status: "todo"
priority: "high"
phase: "Phase 3: ECS Memory Bridging"
depends_on: ["TASK-002"]
blocks: []
estimated_time: "60min"
required_context:
  - "AGENTS.md"
  - ".agents/knowledge/CONSTRAINTS.md"
  - ".agents/knowledge/PATTERNS.md"
  - ".agents/decisions/ADR-0001-safe-memory-bridge.md"
  - "contracts/src/lib.rs"
  - "crates/logic-export/src/lib.rs"
  - "crates/logic-sandbox/src/lib.rs"
---

# TASK-003: Guest reads SoA data from its own buffer (zero unsafe)

## Goal
Guest side of the safe bridge: read the injected `&[Position]` from the
**guest-allocated** input buffer with safe Rust, run a pure movement system in
`logic-sandbox`, and return a `WorldDelta` (writes + optional `ClearColor`).

## Design (MUST be 100% safe)
- Reading input: the input `Vec<u8>` lives in a `spin::Mutex` in
  `crates/logic-export`. `openengine_tick(...)` locks it, takes
  `&buffer[..input_len]` (safe), parses the header/columns (postcard), and
  yields `&[Position]` via `bytemuck::cast_slice` over the safe slice.
- Pure logic (in `logic-sandbox`): `fn movement(tick: u64, positions: &[Position])
  -> WorldDelta`. Fixed-point only (`I16F16`); produce `ColumnWrite`s (and a
  color derived deterministically, converted to `f32` only at emission).
- Output: serialize the `WorldDelta` to a guest `Vec`, stash it, export
  `openengine_get_output() -> u32`; the host reads it with
  `wasmtime::Memory::read`.
- NO `core::slice::from_raw_parts`, NO `from_raw_parts_mut`, NO raw pointers.
  If the pattern tempts you to write `unsafe`, you've deviated from ADR-0001.

## Where things live
- `logic-export`: transport state (`spin::Mutex` buffers), the tick trampoline,
  `openengine_prepare_input`, `openengine_get_output`.
- `logic-sandbox`: pure `movement` / `tick_color` systems only.

## Test Protocol
- Unit: `cargo test -p openengine-logic-sandbox` → pass. Add tests: parsing a
  known buffer returns the right columns/slice; `movement` is deterministic
  (run 3×, bit-identical).
- Sandbox purity (CRITICAL): build the module, then
  `python3 brain/orchestrator.py verify-wasm-purity <wasm>` → must print `[PURE]`.
- Build:
  `cargo build -p openengine-logic-export --target wasm32-unknown-unknown --features wasm-alloc --release`
  → success (stage via `bash scripts/build.sh`).
- Lint: `cargo clippy -p openengine-logic-sandbox -p openengine-logic-export -- -D warnings`
  → clean.
- Zero unsafe: `grep -rn "unsafe" crates/logic-sandbox/src/` → no output.
- Integration: `cargo test -p openengine-core --test integration` → host injects,
  guest reads, guest returns a delta, host applies it.
- No GPU. Docker/CI/offline: yes.

## Acceptance Criteria
- [ ] Input read + typed slice via safe `&buffer[..]` + `bytemuck::cast_slice`.
- [ ] Pure movement system in `logic-sandbox`, fixed-point only.
- [ ] Output via guest `Vec` + `openengine_get_output` (host `Memory::read`).
- [ ] **Zero `unsafe` in Domain B** (grep clean).
- [ ] `verify-wasm-purity` → `[PURE]` (CRITICAL).
- [ ] Unit + integration + determinism tests pass; clippy clean.

## Next
Phase 3 complete → create Phase 4 task files (real game loop).

## Handoff
Record the finalized bridge in `.agents/knowledge/PATTERNS.md` if it differs from
the draft. Update task file, `INDEX.md`, `STATE.md`, `events/`.
