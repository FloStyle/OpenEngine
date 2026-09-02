# Architecture Decisions

---
name: "Architecture Decisions"
updated: "2026-09-03"
---

## Why wgpu over ash (raw Vulkan)?
**Decision:** Use `wgpu`. **Status:** Accepted.
Rationale: native Vulkan backend; WebGPU path for browsers; far less boilerplate;
better errors/debugging; cross-platform (Vulkan/Metal/DX12); simpler for agents.
Trade-off: slight overhead vs raw Vulkan.

## Why postcard for host↔guest communication?
**Decision:** Use `postcard`. **Status:** Accepted.
Rationale: `#![no_std]`; compact binary; deterministic (no float variance); fast;
well-audited. Alternatives rejected: `bincode` (not no_std-friendly), `serde_json`
(verbose, non-deterministic), MessagePack (less compact).

## Why fixed-point math in Domain B?
**Decision:** Use `openengine-math` (fixed-point); forbid `f32` in logic.
**Status:** Accepted.
Rationale: `f32` is not bit-identical across x86 vs ARM; determinism is a product
requirement. Trade-offs: slightly slower, more verbose.

## Why SoA over AoS?
**Decision:** Structure-of-Arrays for ECS. **Status:** Accepted.
Rationale: cache locality; trivial zero-copy serialization; SIMD-friendly.

## Why the "guest allocates, host writes" memory bridge?
**Decision:** The guest allocates a `Vec<u8>` (transport buffer, no simulation
state); the host writes into it via `wasmtime::Memory::write`; the guest reads its
own buffer with safe `&buffer[..]` + `bytemuck::cast_slice`. **Status:** Accepted.
Rationale: respects `#![forbid(unsafe_code)]` in Domain B — no raw pointers, no
`from_raw_parts`, no `#[no_mangle]` in `logic-sandbox`. Trade-offs: one copy
host→guest per tick (acceptable for the MVP).
See `.agents/knowledge/PATTERNS.md`.

## Why exports live in `crates/logic-export`, not `logic-sandbox`?
**Decision:** `#[no_mangle]` is an "unsafe attribute" and cannot coexist with
`forbid(unsafe_code)`. The wasm cdylib trampoline is a separate logic-free crate.
**Status:** Accepted. Rationale: keeps the forbid on all real logic.
