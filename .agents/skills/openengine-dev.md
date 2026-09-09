# Skill: OpenEngine Dev — how an agent plugs in and iterates

A model (DeepSeek, Anthropic, local llama.cpp) becomes an OpenEngine operator by
talking to the **headless harness** over HTTP: observe the world, propose a
change, verify it is safe, reload logic if needed, prove determinism. This is
the entry-point skill — pair it with `openengine-verify` and `openengine-reload`
for the two heavy operations.

## Start the harness (one shell)

```bash
# build the Domain-B logic module once (only needed for wasm guest ticks)
bash scripts/build.sh
# start the headless server (no GPU / display)
bash scripts/harness.sh health --help >/dev/null 2>&1 || cargo run -p openengine-harness -- --port 8080 &
# give it a moment, then confirm
bash scripts/harness.sh health
```

## Endpoint table

| Method | Path | Purpose |
|---|---|---|
| GET | `/health` | liveness + capability list |
| GET | `/observe` | world snapshot `{entity_count, tick, entities[]}` |
| GET | `/schema` | editable components `{id,name,size,fields}` |
| POST | `/spawn` | `{"transform":[x,y,z],"color":[r,g,b,a]}` |
| POST | `/despawn` | `{"entity":i}` |
| POST | `/set` | `{"entity":i,"component":"transform\|scale\|color","value":[...]}` |
| POST | `/tick` | `{"n":100}` advance sim |
| POST | `/physics` | `{"n":100,"half":[1,1,1],"gravity":-0.05,"floor":0}` fixed-point physics |
| GET | `/hash` | deterministic world hash |
| POST | `/prove` | `{"n":100}` determinism replay → `equal:true` |
| POST | `/transaction` | atomic batch `{spawns,despawns,sets}`, rollback on failure |
| GET | `/snapshot` | export inline scene JSON |
| POST | `/restore` | `{"snapshot":{…}}` restore bit-for-bit |
| POST | `/verify` | structured PASS/FAIL gate (build+tests+purity+determinism) |
| POST | `/reload_logic` | rebuild wasm + re-instantiate guest in place |

## Basic recipe

1. `/observe` to see the current world.
2. `/schema` to learn the components you may edit.
3. `/spawn` or `/set` to propose a change (single mutation channel).
4. `/verify` to prove the repo is still green (build + tests + purity).
5. `/reload_logic` if you edited Domain-B logic, then `/tick`.
6. `/prove` to confirm the change stayed deterministic.

## Example session

```bash
# see the world
curl -s http://127.0.0.1:8080/observe
# what can I edit?
curl -s http://127.0.0.1:8080/schema
# spawn a red cube at (1,0,0)
curl -s -X POST http://127.0.0.1:8080/spawn -d '{"transform":[1,0,0],"color":[255,0,0,255]}'
# prove it's still deterministic
curl -s -X POST http://127.0.0.1:8080/prove -d '{"n":100}'
```

## Notes / boundaries

- All edits funnel through `WorldDelta -> apply_delta` (single mutation
  channel) — never raw ECS writes.
- Determinism is the one truth: equal inputs ⇒ bit-identical `World::hash()`.
  `/prove` is the authoritative gate.
- The harness is headless / CI-safe (no GPU, no winit).
