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
uses `ModelAdapter::from_config` to get a llama.cpp client (`key_env` optional —
only for a local server that requires auth):

```json
{
  "provider": { "kind": "local", "endpoint": "http://127.0.0.1:8080/v1", "key_env": "UNSLOTH_API_KEY" },
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

### Configure your key and test it

Config resolution order: `--config <path>` > `$OPENENGINE_AI_CONFIG` >
`./config/ai.json`. Keys are **never** in the file (env only). Copy
`config/ai.example.json` (DeepSeek) or `config/ai-local.example.json`
(unsloth/llama.cpp) to `config/ai.json` and set the key env:

```bash
export DEEPSEEK_API_KEY=sk-...            # for an ApiKey config
# or for a Local server that needs auth (unsloth):
export UNSLOTH_API_KEY=sk-...
# or the local server needs no key:
#   config/ai-local.example.json without key_env

# Ping it — the key+model test:
cargo run -p openengine-ai -- test --config config/ai.json
# one-turn chat:
cargo run -p openengine-ai -- chat --config config/ai.json "say hi"
# catalog:
cargo run -p openengine-ai -- models
```

> The built-in DeepSeek example expects `DEEPSEEK_API_KEY`; the committed
> `config/ai-local.example.json` targets a local unsloth at
> `127.0.0.1:8889/v1` reading `UNSLOTH_API_KEY`.

### Vision — make a model see the live scene

Build the harness with capture and give a **vision-capable** model eyes:

```bash
# GPU machine: run the harness with the capture feature
cargo run -p openengine-harness --features capture -- --port 8090 &
openengine-ai see --config <vision-config> --harness http://127.0.0.1:8090 \
  "describe the actors on the ground"
# or raw:
curl -s http://127.0.0.1:8090/ai/status     # configured? model? vision?
curl -s http://127.0.0.1:8090/frame          # {png_base64,mime,width,height}
```

A local **text-only** model (e.g. KAT-Coder) cannot see — `see` refuses with a
typed error. Load a VLM (llama.cpp with `mmproj`) or use a vision API key for
image turns. Live vision runs via `scripts/ai-vision-test.sh`
(`OPENENGINE_AI_LIVE=1`).

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
