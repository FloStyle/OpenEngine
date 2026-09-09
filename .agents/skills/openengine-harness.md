# Skill: OpenEngine Harness — observe/mutate/verify a live engine

When an agent needs to read or change **live** engine state (not just static
files), talk to the headless harness over HTTP. The harness wraps the real
`World`; it is how an agent proves its edits are deterministic and how the
editor/human and the wasm game logic share one observe → propose → verify →
apply loop. It is the authoritative determinism gate: `/prove` equal hashes on
identical inputs = bit-identical world.

## Start (one shell)

```bash
# ensure the Domain B logic module exists (needed only for /load_wasm guest ticks)
bash scripts/build.sh
# start the headless server (no GPU / window)
cargo run -p openengine-harness -- --port 8080
```

Defaults to `http://127.0.0.1:8080` (use `--port` to override). The helper
wraps curl:

```bash
bash scripts/harness.sh health
bash scripts/harness.sh spawn '{"transform":[1,0,0],"color":[255,0,0,255]}'
bash scripts/harness.sh observe
bash scripts/harness.sh tick '{"n":100}'
bash scripts/harness.sh hash
```

## Endpoints

| Method | Path | Purpose |
|---|---|---|
| GET | `/health` | liveness + full capability list |
| GET | `/spec` | this contract as JSON |
| GET | `/observe` | world snapshot `{entity_count, tick, entities[]}` |
| POST | `/spawn` | `{"transform":[x,y,z],"scale":[1,1,1],"color":[r,g,b,a]}` → `{"entity":i}` |
| POST | `/despawn` | `{"entity":i}` |
| POST | `/set` | `{"entity":i,"component":"transform\|scale\|color","value":[...]}` |
| POST | `/tick` | `{"n":100}` → `{"ticks":n,"hash":"…"}` (native integrator unless wasm loaded) |
| GET | `/hash` | `{"hash":"…","tick":T,"entity_count":N}` |
| POST | `/physics` | run the fixed-point physics step (Domain B gravity/floor/AABB) |
| POST | `/load_wasm` | `{"path":"crates/core/assets/logic.wasm"}` → ticks run guest logic |
| POST | `/reload_logic` | `{"path":"crates/core/assets/logic.wasm"}` → rebuild script + re-instantiate guest |
| POST | `/prove` | `{"n":100}` → `{"equal":true,"hash_a":"…","hash_b":"…","ticks":n}` — replay two fresh runs, compare hashes |
| POST | `/transaction` | batch `{spawns:[…], despawns:[…], sets:[…]}` → `{"applied":N,"ok":true}` |
| GET | `/save` | `{"path":"…"}` → export current world as scene JSON (entities, tick) |
| POST | `/load` | `{"path":"…"}` → import a previously saved scene JSON |
| GET | `/snapshot` | export current world as inline scene JSON (no file) |
| POST | `/restore` | `{"snapshot":{…}}` → restore a snapshot bit-for-bit |
| POST | `/verify` | full structured gate → `{status, build, tests, purity, determinism, errors}` |
| GET | `/reload_logic` | see above (listed under POST) |

## Determinism / prove

The authoritative check is a fresh-run replay: `/prove` restarts state, applies
an identical input sequence twice on two fresh runs, and compares the 64-hex
world hashes. `equal:true` is the gate before shipping any logic change.

```bash
bash scripts/harness.sh prove '{"n":200}'
# → {"equal":true,"hash_a":"…","hash_b":"…","ticks":200}
```

Manual single-run variant (restart server between runs for a fresh state):

```bash
h1=$(bash scripts/harness.sh tick '{"n":200}' | sed -E 's/.*"hash":"([0-9a-f]+)".*/\1/')
# restart server, then:
h2=$(bash scripts/harness.sh tick '{"n":200}' | sed -E 's/.*"hash":"([0-9a-f]+)".*/\1/')
test "$h1" = "$h2" && echo DETERMINISTIC
```

## Save / snapshot round-trip

```bash
# export to a file, mutate, then restore bit-for-bit:
bash scripts/harness.sh snapshot                 # → {"snapshot":{…}} grab it
bash scripts/harness.sh save '{"path":"/tmp/scene.json"}'
bash scripts/harness.sh load '{"path":"/tmp/scene.json"}'
bash scripts/harness.sh restore '{"snapshot":{…}}'
```

`/snapshot` ↔ `/restore` and `/save` ↔ `/load` round-trip through the ECS
`scene` content codec; a restored world hashes identically to the captured one.

## Example session (editor-like workflow)

```bash
# 1. world starts empty
bash scripts/harness.sh observe                       # entity_count:0
# 2. spawn two actors in one atomic batch
bash scripts/harness.sh transaction '{"spawns":[{"transform":[0,0,0]},{"transform":[2,0,0]}]}'
#    → {"applied":2,"entity_count":2,"ok":true}
# 3. prove determinism across a replay
bash scripts/harness.sh prove '{"n":100}'            # equal:true
# 4. export + restore round-trip
bash scripts/harness.sh snapshot | tee /tmp/snap.json
bash scripts/harness.sh restore "$(cat /tmp/snap.json)"
# 5. full structured gate (build + tests + purity + determinism)
bash scripts/harness.sh verify
```

## Notes

- Single mutation channel is preserved: `/set`, `/spawn`, `/despawn`,
  `/tick`, `/transaction` all funnel into `WorldDelta -> apply_delta` / host
  plumbing — never raw ECS writes.
- `/tick` uses a fixed cadence; after `/load_wasm` it runs the guest
  `gameplay_tick`, otherwise an identity native integrator (the guest is the
  real sim). `/physics` runs the Domain B fixed-point physics step instead.
- Determinism is the one truth: a bit-identical input sequence must yield a
  bit-identical `World::hash()`. `/prove` is the authoritative gate; a logic
  change only ships when it passes and clippy/fmt are green.
- Headless / CI-safe: the crate pulls no wgpu/winit/GPU.
