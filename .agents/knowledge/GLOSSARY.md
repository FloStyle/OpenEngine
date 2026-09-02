# Glossary

---
name: "Glossary"
updated: "2026-09-03"
---

## Core Concepts
- **Domain A (Core)** — host runtime: `std`, `wgpu`, `winit`, `wasmtime`,
  threads.
- **Domain B (Logic)** — `#![no_std]` wasm guest: pure, deterministic, no
  `unsafe`.
- **StateView** — read-only view of world state handed to a pure system.
- **WorldDelta** — the sole mutation channel a pure system returns.
- **DeferredCommand** — out-of-band request (render/clear/event) in a delta.
- **ARCH_VERSION** — ABI revision in `contracts/`; bump on breaking change.

## Technical Terms
- **SoA (Structure of Arrays)** — one contiguous array per component type.
- **Pod (Plain Old Data)** — `bytemuck` marker for types safely castable to/from
  bytes.
- **Zero-copy** — reading SoA memory without a copy (here: safe
  `bytemuck::cast_slice` over guest-owned memory).
- **Fixed-point math** — integer-based exact arithmetic (`openengine-math`) used
  instead of `f32` for determinism.
- **Postcard** — compact, deterministic, `#![no_std]` codec for host↔guest.

## Agent-OS Terms
- **Task** — atomic unit of work (goal, context, test protocol, acceptance).
- **Session** — an agent executing a task, tracked in `.agents/sessions/`.
- **Handoff** — context transfer between agents/sessions.
- **Checkpoint** — resume point for long tasks.
- **ADR** — Architecture Decision Record in `.agents/decisions/`.
