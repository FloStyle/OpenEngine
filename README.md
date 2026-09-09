# OpenEngine

**AI-native Rust + Wasm game engine.** Built and maintained by multiple
autonomous AI agents without context loss or architectural drift. Agent
governance lives in [`AGENTS.md`](AGENTS.md) and `.agents/`.

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
| `crates/core` | Domain A — wasmtime host + gameplay/physics wasm bridges + meshes. |
| `crates/ecs` | Domain A — SoA/archetype `World` storage + scene codec. |
| `crates/editor` | Domain A — headless editor math (camera/grid/gizmo/commands). |
| `crates/editor-shell` | Domain A — `egui`/wgpu interactive editor over the headless core. |
| `crates/logic-sandbox` | Domain B — pure `#![no_std]` Wasm logic + physics. |
| `crates/logic-export` | Domain B — `#[no_mangle]` wasm trampoline (tick/gameplay/physics). |
| `crates/math` | Domain B — deterministic fixed-point (`I16F16`). |
| `crates/harness` | Domain A — headless JSON-over-HTTP live surface + `/verify` `/schema` `/frame`. |
| `crates/capture` | Domain A — headless offscreen render → PNG (vision `/frame`). |
| `crates/ai` | Domain A — uniform model adapter + CLI (`openengine-ai`); `.env` keys. |
| `crates/plugin-host` | Domain A — plugin boundary (`Plugin` trait + `PluginHost`). |
| `brain/` | Domain C — Python orchestration (purity checks, LLM critic). |
| `docs/`, `docs/specs/`, `.agents/` | Specs, decisions (ADRs), skills, agent governance. |

## Start here

- **Agents:** read [`AGENTS.md`](AGENTS.md) — it is the constitution.
- **Humans:** see [`docs/specs/architecture.md`](docs/specs/architecture.md).
- **Run the editor:** `bash scripts/editor.sh` (double-click friendly; see
  [`docs/editor.md`](docs/editor.md)).
- The ABI lives in [`contracts/src/lib.rs`](contracts/src/lib.rs) (`ARCH_VERSION`).

## Status

Beyond the scaffold: a live ECS (`crates/ecs`), an interactive `egui` editor
(`crates/editor-shell`), deterministic Wasm gameplay + fixed-point physics
(Domain B), a headless harness (`/observe /spawn /tick /prove /verify /schema
/frame`), a plugin boundary, and an AI-connected model adapter + CLI
(`openengine-ai test|chat|see`) with **vision**: a multimodal model can look at
the engine's live rendered world. See [`STATE.md`](STATE.md) for the detailed,
current record. API keys live in a gitignored `.env` (never committed) — see
[`docs/spawn-agent.md`](docs/spawn-agent.md) and `.env.example`.
