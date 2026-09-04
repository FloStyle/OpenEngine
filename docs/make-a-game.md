# Make a Game — OpenEngine quickstart (the "minimal game engine" loop)

This is the shortest path from **start** to a **distributable game**. Every
headless command below is verified to work on `main`; the windowed ones you run
locally (no GPU here).

```
Start → edit scene → develop logic (AI) → test → play → package → run
```

## 0. Prereqs (one-time)
```bash
rustup target add wasm32-unknown-unknown
bash scripts/build.sh   # rebuild the pure Domain-B logic (logic.wasm)
```

## 1. Start the editor (windowed — you run it)
```bash
cargo run -p openengine-editor-shell
```
- **Edit mode**: orbit = right-drag, pan = middle-drag, zoom = wheel. Select an
  entity in the left hierarchy; edit Transform in the right inspector.
- **Play**: click ▶ Play → WASD/Space move the player, NPCs wander/chase (the
  real wasm logic runs — the toolbar shows **engine: wasm**).

## 2. Author a scene by hand (headless — anyone)
A scene is the shared, versioned JSON format (`openengine-ecs::scene`). Example:
`examples/demo-chase.json` — a player (actor_kind 0) + a chaser (3) + a wanderer
(1). You can copy it, or save it from the editor (💾 Save).

## 3. Develop logic (give to an AI / write a system)
Domain-B logic lives in `crates/logic-sandbox` as **pure** fixed-point systems
(`gameplay_tick`, …), exported via `crates/logic-export`, `[PURE]`, no `f32`.
After editing, rebuild + verify purity:
```bash
bash scripts/build.sh
python3 brain/orchestrator.py verify-wasm-purity crates/core/assets/logic.wasm   # → [PURE]
```

## 4. Test (headless, deterministic — no GPU)
```bash
# full workspace tests (determinism, wasm, editor, harness, player)
cargo test --workspace

# prove determinism of a run over a live core
cargo run -p openengine-harness -- --port 8080 &     # then:
curl -s -X POST -H 'Content-Type: application/json' -d '{"n":200}' http://127.0.0.1:8080/prove
```

## 5. Play (headless via the runner)
```bash
# no logic → inert
cargo run -p openengine-harness --bin openengine-runner -- --scene examples/demo-chase.json --frames 120

# with the guest logic → the chaser moves; deterministic
cargo run -p openengine-harness --bin openengine-runner -- --scene examples/demo-chase.json --wasm crates/core/assets/logic.wasm --frames 120

# scripted input replay (a QA/playthrough script)
cargo run -p openengine-harness --bin openengine-runner -- --scene examples/demo-chase.json --wasm crates/core/assets/logic.wasm --frames 300 --script examples/../tmp/my-script.json
```

### Play windowed (you run it)
```bash
cargo run -p openengine-editor-shell -- --play examples/demo-chase.json
```
Opens the window already playing that scene.

## 6. Package & distribute
```bash
bash scripts/package.sh demo examples/demo-chase.json   # → dist/demo/
dist/demo/run.sh                                        # headless, default 120 frames
FRAMES=600 FORWARD=200 dist/demo/run.sh                 # scripted: player walks forward 200 ticks
```
`dist/<game>/` is self-contained: `openengine-game` + `logic.wasm` + `scene.json`
+ `run.sh`. Copy that folder anywhere and run it.

## The AI loop (longer term, per ADR-0002 / specs 51–52)
An agent/AI works in the engine's own language: `observe` → `propose` (deltas /
Rust) → `verify` (determinism/purity/build) → `apply` or rollback. The headless
core (`crates/harness`) exposes that surface today; the resident assistant is a
future Domain-A layer.

## Source map
- Pure logic (Domain B): `crates/logic-sandbox`, `crates/logic-export`
- Shared scene codec: `crates/ecs/src/scene.rs`
- Live headless core: `crates/harness` (HTTP + `openengine-runner`)
- Editor (windowed): `crates/editor-shell`
- Docs: `docs/architecture-map.md`, specs `50`/`51`/`52`, `docs/specs/*`
