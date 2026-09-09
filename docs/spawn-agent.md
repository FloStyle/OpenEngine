# Spawning an OpenEngine agent

OpenEngine is AI-ready: any model (DeepSeek, Anthropic, a local llama.cpp) can
become a resident operator that observes the live world, proposes edits, verifies
them, reloads logic and proves determinism — via the headless harness. This page
is the recipe for wiring an agent in.

The agent-facing instructions live in `.agents/skills/`:

- `openengine-dev.md` — plug in + iterate (endpoint table, basic recipe)
- `openengine-verify.md` — use `/verify` as the safety gate
- `openengine-reload.md` — hot-reload logic after editing Domain B

## Start the harness

One headless server, no GPU/display, shared by every agent:

```bash
bash scripts/build.sh                       # build logic.wasm (once)
cargo run -p openengine-harness -- --port 8080 &
```

Confirm: `curl -s http://127.0.0.1:8080/health`.

## With DeepSeek Harness (DSH)

1. Start the harness as above.
2. In DSH, create a subagent that loads the skill:
   ```
   /spawn openengine-dev
   ```
   (DSH exposes skills from `.agents/skills/`; the subagent reads
   `openengine-dev.md` and iterates over the endpoints.)

## With Pi

1. Start the harness as above.
2. Point Pi at the skill file:
   ```bash
   pi --skill .agents/skills/openengine-dev.md
   ```
3. Pi drives the engine over HTTP (`/observe`, `/spawn`, `/set`, …).

## With a local model (llama.cpp) via `openengine-ai`

OpenEngine can also call a model itself through `crates/ai` (the uniform
`ModelAdapter`). Start a local server:

```bash
./llama-server -m model.gguf --port 8080
```

Then configure the engine (a `ProviderConfig::Local` config), and any code path
uses `ModelAdapter::from_config` to get a llama.cpp client:

```json
{
  "provider": { "kind": "local", "endpoint": "http://127.0.0.1:8080/v1" },
  "model": "model.gguf"
}
```

For a cloud key the config is `ApiKey` (key read from an env var, never inline):

```json
{
  "provider": { "kind": "api_key", "endpoint": "https://api.deepseek.com/v1", "key_env": "DEEPSEEK_API_KEY" },
  "model": "deepseek-chat"
}
```

`from_config` maps `ApiKey` → a DeepSeek client and `Local` → a llama.cpp
client, so the assistant code never branches on provider.

## Agent loop (what a spawned agent does)

1. `GET /observe` — see the current world.
2. `GET /schema` — learn the components it may read/write.
3. `POST /spawn` / `POST /set` — propose a change (single mutation channel).
4. `POST /verify` — structured PASS/FAIL gate (build + tests + purity).
5. `POST /reload_logic` — if it edited Domain-B logic, hot-reload the guest.
6. `POST /prove` — confirm the change is still deterministic.

```bash
curl -s http://127.0.0.1:8080/observe | jq '.entity_count'
curl -s http://127.0.0.1:8080/schema | jq '.components'
curl -s -X POST http://127.0.0.1:8080/spawn -d '{"transform":[1,0,0],"color":[255,0,0,255]}'
curl -s http://127.0.0.1:8080/verify | jq '.status'
curl -s -X POST http://127.0.0.1:8080/prove -d '{"n":100}' | jq '.equal'
```

## Guarantees an agent can rely on

- Determinism: equal inputs ⇒ bit-identical `World::hash()`; `/prove` is the
  authoritative gate.
- Single mutation channel: every edit flows through `WorldDelta -> apply_delta`.
- Headless/CI-safe: harness has no GPU/winit dependency.
