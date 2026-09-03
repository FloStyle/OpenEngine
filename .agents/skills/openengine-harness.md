# Skill: OpenEngine Harness — observe/mutate/verify a live engine

When an agent needs to read or change **live** engine state (not just static
files), talk to the headless harness over HTTP. The harness wraps the real
`World`; it is how an agent proves its edits are deterministic and how the
editor/human and the wasm game logic share one observe → propose → verify →
apply loop.

## Start (one shell)

```bash
# ensure the Domain B logic module exists (needed only for /load_wasm guest ticks)
bash scripts/build.sh
# start the headless server (no GPU / window)
cargo run -p openengine-harness -- --port 8080
```

Defaults to `http://127.0.0.1:8080`. The helper wraps curl:

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
| GET | `/health` | liveness + capability list |
| GET | `/spec` | this contract as JSON |
| GET | `/observe` | world snapshot (entity_count, tick, entities[]) |
| POST | `/spawn` | `{"transform":[x,y,z],"scale":[1,1,1],"color":[r,g,b,a]}` → `{"entity":i}` |
| POST | `/despawn` | `{"entity":i}` |
| POST | `/set` | `{"entity":i,"component":"transform\|scale\|color","value":[...]}` |
| POST | `/tick` | `{"n":100}` → `{"ticks":n,"hash":"…"}` |
| GET | `/hash` | `{"hash":"…","tick":T,"entity_count":N}` |
| POST | `/load_wasm` | `{"path":"crates/core/assets/logic.wasm"}` → ticks run guest logic |

## Determinism check (always do this after a mutation)

Reset, replay the same sequence on a fresh run, and compare hashes:

```bash
h1=$(bash scripts/harness.sh tick '{"n":200}' | sed -E 's/.*"hash":"([0-9a-f]+)".*/\1/')
# restart server / repeat on a fresh state, then:
h2=…
test "$h1" = "$h2" && echo DETERMINISTIC
```

Equal 64-hex hashes on identical input sequences = bit-identical world.

## Notes

- Single mutation channel is preserved: `/set`, `/spawn`, `/tick` funnel into
  `WorldDelta -> apply_delta` / host plumbing — never raw ECS writes.
- `/tick` uses a fixed 60 Hz cadence; with `/load_wasm` it runs the guest
  `gameplay_tick`, else an identity native integrator (guest is the real sim).
- Headless / CI-safe: the crate pulls no wgpu/winit/GPU.
