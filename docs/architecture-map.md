# OpenEngine — Developer Map

> **Read this first** to develop OpenEngine (human or AI agent). It maps code →
> specs → decisions so you never need to reverse-engineer the layout. Keep it
> current whenever you add/move a crate, spec, or ADR.

## Repository shape

| Path | Role | Read |
|---|---|---|
| `contracts/` | The ABI wall (`no_std`, frozen component registry, `ARCH_VERSION`) | `docs/abi/README.md`, `docs/abi/CHANGES.md` |
| `crates/math` | Fixed-point (`I16F16`) shared Domain B | `docs/specs/03-input-system` (math determinism) |
| `crates/ecs` | Mono-archetype SoA `World`, `apply_delta`, `hash()` | `00-ecs-architecture` |
| `crates/logic-sandbox` | **Domain B** pure systems (`no_std`, `[PURE]`) | `01-game-loop`, `51/52` |
| `crates/logic-export` | wasm cdylib `#[no_mangle]` trampolines (never logic here) | `ADR-0001` |
| `crates/core` | Domain A host: renderer, sandbox host, movement/gameplay wasm hosts | `01`, `10-hot-reload` |
| `crates/editor` | Headless editor core (Edit/Play, commands, undo, selection, camera, grid/snap/move math) | `22-edit-vs-play`, `23-undo-redo`, `07/08/09`, `51` |
| `crates/editor-shell` | egui editor over wgpu — layout UE-like; tools Q/W, Move+drag/snap, Add/Delete/Duplicate, Save/Load, `--play` | `24-editor-viewport`, `25-editor-shell`, `docs/editor.md` |
| `crates/harness` | Headless live-state surface for agents/AI: HTTP server (`/observe` `/spawn` `/set` `/tick` `/hash` `/load_wasm` `/prove` `/transaction` `/save` `/load` `/snapshot` `/restore` `/verify` `/reload_logic` `/schema` `/ai/status`, + `/frame` under the `capture` feature) + `openengine-runner` (headless player) | `51`, `52`, `16`, `50`, `ADR-0001/0002`, `docs/self-heal.md` |
| `.agents/skills/openengine-*.md` | Agent recipes: `dev` (plug in + iterate), `verify` (safety gate), `reload` (hot-reload), `vision` (multimodal loop), `harness`, `self-dev` | `52` |
| `crates/capture` | Headless offscreen render + PNG of a `World` (wgpu, no window/egui); harness `capture` feature → `GET /frame` | `24-editor-viewport`, `52` |
| `scripts/selfdev.sh` | Canned self-development loop (observe/spawn/verify/prove/restore) | `52` |
| `examples/` | Authored demo scenes (e.g. `demo-chase.json`) | `50-build-deploy` |
| `crates/ai` | Uniform model adapter: types + trait + concrete clients (DeepSeek API key, local llama.cpp/unsloth) via `ModelAdapter::from_config`; config resolution + model catalog + multimodal content parts + gitignored `.env` loader (env wins); `openengine-ai` CLI (`describe/providers/models/list/check/load/test/chat/see`). Secrets guarded by `scripts/check-secrets.sh` (CI). | `52-ai-developer-surface`, `ADR-0002` |
| `crates/plugin-host` | Plugin boundary (Phase 3): `Plugin` trait + `PluginHost` lifecycle. In-process; dylib = ADR-0005. | `29-plugins`, `ADR-0005` |
| `scripts/` | `build.sh` (logic.wasm), `package.sh` (cook a game → `dist/`), `harness.sh` (API), `ai-live-test.sh` + `ai-vision-test.sh` (gated live paths) | `50-build-deploy` |

## Domains & the rule that keeps you safe

```
Domain A (host, std): core, ecs, editor, editor-shell, harness
Domain B (logic, no_std + forbid(unsafe)): logic-sandbox, logic-export, math, contracts
```
- Game logic is **pure**: `fn(&StateView) -> Result<WorldDelta, …>`, fixed-point only.
- **Single mutation channel**: all changes are `WorldDelta → apply_delta`
  (or editor `Command`). No actor writes columns directly (spec `51`).
- Determinism is a product requirement: same inputs ⇒ bit-identical `World::hash()`.

## Specs quick map (docs/specs/)

| Area | Specs |
|---|---|
| ECS / loop / time | `00` ECS, `01` game loop, `05` time, `21` primitive components |
| Assets / render | `02` assets, `04` render, `06` scene, `40` post, `41` lighting, `42` terrain |
| Editor | `07` inspector, `08` hierarchy, `09` gizmos, `22` edit-vs-play, `23` undo, `24` viewport, `25` shell, `31` multi-select |
| Serialization / hot reload / plugins | `16` serialization, `10` hot reload, `29` plugins, `46` sequencer, `48` visual scripting |
| Systems (game features) | `13` physics, `14` audio, `15` networking, `36-45` anim/particles/nav/… |
| **AI-native** | `51` operator protocol, `52` AI developer surface |
| Governance / tooling | `17` testing, `18` documentation, `19` examples, `20` CI |

## Decision records (.agents/decisions/)

| ADR | Decision |
|---|---|
| `ADR-0001` | Safe (zero-unsafe) host↔guest memory bridge |
| `ADR-0002` | AI-native, all-in-one engine (resident assistant, no side chatbot) |

## Documentation upkeep (mandatory)

To keep OpenEngine *developable* (humans + AI), any behavior-affecting change must
update its pointer here / in the right place before it is "done":

1. `AGENTS.md` — the constitution (domain rules, single mutation channel, purity).
2. `STATE.md` — current phase + what's active/completed (keep truthful, not stale).
3. `docs/specs/architecture.md` — the one-picture + tick loop.
4. The subsystem spec touched (always via the map above).
5. `docs/abi/CHANGES.md` — any ABI/layout change (with `ARCH_VERSION` rules).
6. `.agents/decisions/` — a new ADR for contract-layout / unsafe / platform changes.

> **Wiring an external agent in?** Read `docs/spawn-agent.md` (DSH / Pi / local
> llama.cpp recipes) and the `.agents/skills/openengine-*.md` recipes.

Rule of thumb: **if a future agent would have to read source to learn what a spec
claims, the doc is stale — fix it.**
