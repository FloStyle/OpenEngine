# STATE.md — Global Project State

---
name: "Global State"
phase: "Phase 3: ECS Memory Bridging"
updated: "2026-09-03"
---

# STATE.md

## Current Phase: Phase 3 — ECS Memory Bridging

Goal: Prove that Domain A can write SoA memory into the guest, Domain B can read
it with safe Rust (no `unsafe`), and return a `WorldDelta` to update it.

## Active Tasks

| task_id | title | status | assigned_to | priority |
|---------|-------|--------|-------------|----------|
| TASK-001 | Add Position component to contracts | todo | unassigned | high |
| TASK-002 | Host writes SoA data into guest-allocated buffer | todo | unassigned | high |
| TASK-003 | Guest reads SoA data from its own buffer (zero unsafe) | todo | unassigned | high |

## Blockers

None currently.

## Next Actions

1. TASK-001 — add `Position` (fixed-point, `Pod`) + enable `fixed`'s `bytemuck`
   feature in `openengine-math`.
2. TASK-002 — host writes header + columns + component bytes into a
   guest-allocated buffer via `wasmtime::Memory::write`.
3. TASK-003 — guest reads its own buffer with `&buffer[..]` +
   `bytemuck::cast_slice`, returns `WorldDelta`; verify `[PURE]`.
4. Verify end-to-end with an integration test (host injects → guest reads →
   guest returns delta → host applies).

## Completed Tasks

- Phase 1: Scaffold architecture (ABI v1) — done.
- Phase 2: Living Window, wgpu(Vulkan) + wasmtime + `ClearColor` (ABI v2) — done.
- Agent OS infra (`.agents/`, STATE.md, ROADMAP.md) — done.

## Important Notes

- All Domain B code must pass purity verification
  (`python3 brain/orchestrator.py verify-wasm-purity`).
- Memory bridging is **100% safe**: the guest allocates, the host writes via
  `Memory::write`, the guest reads its own `Vec`. No raw pointers, no `unsafe`.
- Exports (`#[no_mangle]`) live in `crates/logic-export`, never in
  `logic-sandbox` (which is `forbid(unsafe_code)`).
- Test protocol is mandatory for every task.
