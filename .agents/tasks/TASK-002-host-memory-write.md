---
task_id: "TASK-002"
title: "Host writes SoA into guest-allocated buffer"
status: "todo"
priority: "high"
phase: "Phase 3: ECS Memory Bridging"
depends_on: ["TASK-001"]
blocks: ["TASK-003"]
estimated_time: "45min"
required_context:
  - "AGENTS.md"
  - ".agents/knowledge/CONSTRAINTS.md"
  - ".agents/knowledge/PATTERNS.md"
  - ".agents/decisions/ADR-0001-safe-memory-bridge.md"
  - "contracts/src/lib.rs"
  - "crates/core/src/main.rs"
  - "crates/logic-export/src/lib.rs"
---

# TASK-002: Host writes SoA into a guest-allocated buffer

## Goal
Host side of the safe memory bridge: write a `Vec<Position>` (as SoA bytes) into
a **guest-allocated** wasm buffer so Domain B can read it with safe Rust.

## Design (MUST follow ADR-0001 / PATTERNS.md)
**Guest allocates, host writes, guest reads its own `Vec`. No `unsafe`, no raw
pointers.**

1. In `crates/logic-export/src/lib.rs` add
   `openengine_prepare_input(size: u32) -> u32`:
   allocate a `Vec<u8>` of `size`, stash it in a `spin::Mutex<Option<Vec<u8>>>`,
   return the buffer pointer. (Transport buffer only — never simulation state.)
2. In `contracts` add a small input header (e.g. `InputHeader { tick, column_count,
   data_offset }`, serde/postcard) plus a byte-layout convention that reuses the
   existing `ColumnDescriptor` as offsets **into the input buffer**. Keep the
   Phase-2 `StateView`/`tick_color` intact — add, don't reshape.
3. In `crates/core/src/main.rs` (`Logic`):
   - Build a `HostWorld { positions: Vec<Position>, tick }`.
   - Serialize `InputHeader` + descriptors (postcard) and take
     `bytemuck::cast_slice(&world.positions)`.
   - Call `openengine_prepare_input(total_size)` to get the guest pointer.
   - Write each segment with `wasmtime::Memory::write(&mut store, offset, bytes)`.
   - Call `openengine_tick(...)` passing the input pointer/len and output buffer.
   Return the write offset + length for TASK-003.

A full worked example lives in PATTERNS.md (memory-bridge section). Do not copy
any earlier raw-pointer draft.

## Exports note
All `#[no_mangle]` exports live in `crates/logic-export`. `logic-sandbox` keeps
`forbid(unsafe_code)` and never exports.

## Test Protocol
- Unit: `cargo test -p openengine-core` → pass; add a test that
  `inject_input` writes the correct bytes (compare the guest memory region).
- Integration: add `crates/core/tests/integration.rs` asserting the host injects
  100 `Position`s into the guest buffer (offsets/len correct).
- Build: `cargo build -p openengine-core` → success (x86_64-linux).
- Lint: `cargo clippy -p openengine-core -- -D warnings` → clean.
- No GPU. Docker/CI/offline: yes.

## Acceptance Criteria
- [ ] `openengine_prepare_input` added in `logic-export`.
- [ ] Input transport types added to `contracts` (additive) + `ARCH_VERSION` bump
      with `docs/abi/CHANGES.md`.
- [ ] `HostWorld` + `inject_input` write header + columns + component bytes via
      `wasmtime::Memory::write`.
- [ ] Unit + integration tests pass; clippy clean.
- [ ] Zero `unsafe` added to Domain B; no hardcoded/OS-specific code.

## Next
`TASK-003-guest-read.md` — must follow immediately (same ABI).

## Handoff
Update task file, `INDEX.md`, `STATE.md`, `events/`.
