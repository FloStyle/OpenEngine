# Skill: OpenEngine Vision — a multimodal model SEES the live engine

A vision-capable model can look at the engine's own rendered world and drive a
vision+verification loop no other engine offers. The harness serves a PNG of
the current scene; `openengine-ai see` sends it to a VLM; `/verify` /
`/prove` machine-check the model's proposed changes.

## Prerequisites

- A **vision-capable** model configured (a VLM loaded in local llama.cpp with
  `mmproj`, or a vision API key). A text-only local model (e.g. KAT-Coder) can
  **not** see — `/see` refuses with a typed error; that is correct.
- A running harness built **with the `capture` feature** (needs a GPU):
  ```bash
  cargo run -p openengine-harness --features capture -- --port 8090
  ```

## The loop

```bash
# 1. Model sees the live scene
curl -s http://127.0.0.1:8090/ai/status          # configured? vision?
openengine-ai see --config config/ai.json \
  --harness http://127.0.0.1:8090 "what actors are on the ground?"
# 2. Model proposes ops from what it saw (spawn/set/despawn)
curl -s http://127.0.0.1:8090/observe
curl -s http://127.0.0.1:8090/schema             # what components exist
# 3. Apply (single mutation channel)
curl -s -X POST http://127.0.0.1:8090/spawn -d '{"transform":[0,0,2],"color":[0,0,255,255]}'
# 4. Machine-checked verdict
curl -s http://127.0.0.1:8090/verify | jq .status   # PASS
curl -s -X POST http://127.0.0.1:8090/prove -d '{"n":100}' | jq .equal
```

## Raw endpoints

| Endpoint | Purpose |
|---|---|
| `GET /frame` | PNG base64 of the live world `{png_base64,mime,width,height}` (capture feature) |
| `GET /ai/status` | config-only status: `{configured,provider,model,endpoint,vision}` |

`GET /frame` returns `503 {"error":"no-adapter:…"}` when the harness was built
without `--features capture` or no GPU adapter exists — never a crash.

## What the frame shows

The editor viewport pipeline: a light sky clear, a checkered grey ground plane,
and each entity drawn as a **sphere** of its `Color` at its `Transform`
position (fixed 0.6 radius; scale/rotation not shown). So the model sees colors
and XZ layout, not scale/size.

## Headless note

`cargo test` stays offline; real vision runs only via
`scripts/ai-vision-test.sh` (gated by `OPENENGINE_AI_LIVE=1`).
