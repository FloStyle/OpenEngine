# OpenEngine

**AI-native Rust + Wasm game engine.** Built and maintained by multiple
autonomous AI agents (DeepSeek, Cursor, Codex, GitHub Copilot) without context
loss or architectural drift.

## Architectural pillars

1. **Core (Domain A):** native Rust — `wgpu` (Vulkan-preferred), `wasmtime`
   host, `winit`, multithreaded Job System over `rayon`.
2. **Logic (Domain B):** `#![no_std]` Rust compiled to Wasm — pure FP,
   deterministic `fixed`-point math.
3. **ECS:** strict Structure-of-Arrays, zero-copy memory bridging to Wasm.
4. **Editor:** `egui`, running as a system within the ECS.
5. **Orchestration (Domain C):** Python *Brain* for CI, RAG, and LLM Critic loops.

## Layout

| Path | Purpose |
|------|---------|
| `contracts/` | **The Immutable ABI.** The physical wall between domains. |
| `crates/core` | Domain A — renderer, wasmtime host, job system. |
| `crates/ecs` | Domain A — SoA/archetype storage. |
| `crates/editor` | Domain A — `egui` editor system. |
| `crates/logic-sandbox` | Domain B — pure `#![no_std]` Wasm logic. |
| `crates/math` | Domain B — deterministic fixed-point. |
| `brain/` | Domain C — Python orchestration (purity checks, LLM critic). |
| `docs/` | Human-readable specs + ABI changelog. |

## Start here

- **Agents:** read [`AGENTS.md`](AGENTS.md) — it is the constitution.
- **Humans:** see [`docs/specs/architecture.md`](docs/specs/architecture.md).
- The ABI lives in [`contracts/src/lib.rs`](contracts/src/lib.rs) (`ARCH_VERSION`).

## Status

Scaffold milestone only: architecture, ABI contracts, and AI governance files.
The ECS and renderer implementations are intentionally not yet written.
