# ADR-0001: Safe (zero-unsafe) host↔guest memory bridge

---
id: "ADR-0001"
title: "Safe host↔guest memory bridge"
status: "Accepted"
date: "2026-09-03"
phase: "Phase 3: ECS Memory Bridging"
---

## Context

Domain B must read SoA component data written by Domain A, and Domain B is
`#![forbid(unsafe_code)]` (`forbid`, not overridable). The earlier draft design
used raw pointers (`core::slice::from_raw_parts`) and an unsafe wrapper in
`contracts` — both violate the forbid.

## Decision

Adopt the **"guest allocates, host writes"** bridge:

1. The guest allocates a transport `Vec<u8>` (guarded by a `spin::Mutex`,
   transport buffer only — never simulation state) inside `crates/logic-export`.
2. The host writes serialized `ColumnDescriptor`s (postcard) and raw component
   bytes into that buffer via `wasmtime::Memory::write`.
3. The guest reads its own buffer with safe `&buffer[..]` and
   `bytemuck::cast_slice`.
4. Output flows the same way in reverse (`openengine_get_output`, read via
   `wasmtime::Memory::read`).
5. All `#[no_mangle]` trampolines live in `crates/logic-export`, never in
   `logic-sandbox`.

## Consequences

- Zero `unsafe`, zero raw pointers in Domain B.
- One host→guest write per tick (accepted for the MVP).
- The existing `StateView`/`tick_color` (Phase 2) is preserved; Phase 3 adds new
  input-transport ABI types rather than reshaping it.
