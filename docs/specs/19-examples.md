---
spec: "19-examples"
phase: "Phase 4+"
status: "design"
---

# Examples

## Overview

Examples are **runnable proofs**, not screenshots. Every example game is a real
crate (or a real test) in this workspace that exercises a specific slice of the
engine and asserts its expected behavior. Because gameplay lives in Domain B as
pure `fn(&StateView) -> Result<WorldDelta, RecoverableError>`, each example's
**logic must stay pure and portable** — it compiles to both a native rlib and
`wasm32-unknown-unknown`, uses only fixed-point math, never touches the GPU in
its logic, and must pass `verify-wasm-purity` as `[PURE]`.

The three examples here form a ladder — each adds one capability and one
testing burden:

| Example | Exercises | Purity | Render output |
|---------|-----------|--------|---------------|
| **Pong** | fixed-timestep sim in Domain B, clear + rect draw | pure, deterministic | `ClearColor` + rect commands |
| **Platformer** | AABB physics (collision + resolution) | pure, deterministic | rects + deferred spawn/despawn |
| **Sprites / demo** | asset loading + input → deterministic snapshot | pure logic; asset/input on host | textured sprite draw |

The cardinal rule for every example: **gameplay is deterministic given inputs**.
Host-side asset loading and input collection (Domain A) feed a *deterministic
snapshot* into Domain B; Domain B never reads the wall clock, files, or
ambient randomness. Each example therefore ships a headless "simulation replay"
test that runs N ticks from a fixed seed and asserts a bit-identical result,
independent of GPU.

## Example layout

Each example is a small pair mirroring the domain split:

```
examples/pong/
  Cargo.toml            # host bin: window + loop (Domain A)
  src/main.rs           # host driver: builds StateView, applies WorldDelta
  src/logic.rs          # DOMAIN B pure logic: fixed-point, returns WorldDelta
  tests/sim.rs          # headless determinism + unit tests (no GPU)
```

The pure `logic.rs` source is shared — compiled by the host crate natively for
tests *and* emitted as a `no_std` wasm module when the game runs. The clean way
to keep that split honest is to follow the same rule as the core engine:
`#[no_mangle]` trampolines live in an export shim, never in the pure logic.

## Example 1 — Pong (fixed-timestep + clear + rects)

### What it exercises
The **fixed-timestep simulation** end of `docs/specs/01-game-loop.md`, plus the
two render primitives the ABI already speaks today: `ClearColor` and a rectangle
draw (a `DeferredCommand` for the renderer). Domain B owns the whole simulation:
ball velocity, paddle reflection, wall bounce, and score — all in fixed-point
(`openengine-math::I16F16`).

```rust
// Pure ball-step: part of Domain B logic.rs.
fn step_ball(b: Ball, dt: Fixed, height: Fixed) -> Ball {
    let mut b = b;
    b.pos.y += b.vel.y * dt;
    // Deterministic wall bounce: reflect and invert, no f32, no sine.
    if b.pos.y < b.radius || b.pos.y > height - b.radius {
        b.vel.y = -b.vel.y;
    }
    b
}

fn pong_tick(view: &StateView<'_>) -> Result<WorldDelta, RecoverableError> {
    // read paddle/ball columns from the view arena via cast_slice,
    // compute the next fixed-timestep state, return WorldDelta{ writes,
    // deferred: [ClearColor, Rect{...} x 3] }.
    Ok(WorldDelta::default())
}
```

### Expected behavior
Given identical paddle inputs and seed, 1000 fixed ticks always produce the same
ball trajectory. The screen shows a clear colour, a ball rectangle, and two
paddle rectangles. Rendering never changes simulation state.

### How to run
```bash
cargo run -p openengine-example-pong        # needs a Vulkan-capable display
cargo test -p openengine-example-pong       # headless logic tests, no GPU
```

### Purity / determinism expectations
- `bash scripts/build.sh` then `verify-wasm-purity` reports `[PURE]`.
- `tests/sim.rs` runs the pure sim 3× and asserts byte-identical state
  (spec `17` determinism).
- No `f32` anywhere in `logic.rs`; only fixed-point.

## Example 2 — Platformer (AABB physics)

### What it exercises
**Axis-aligned bounding-box physics** in Domain B: gravity (fixed-point),
player-vs-world AABB collision detection and resolution (per-axis so sliding
works), ground/tile queries, and spawning/despawning via `WorldDelta` (e.g. a
collected coin despawns, a particle spawns). This is the first example that
meaningfully exercises the ECS side — columns of `Position`, `Velocity`,
`Aabb` read through `StateView`.

```rust
// Deterministic AABB overlap, pure integer/fixed math only.
fn aabb_overlap(a: &Aabb, b: &Aabb) -> bool {
    a.min_x < b.max_x && b.min_x < a.max_x && a.min_y < b.max_y && b.min_y < a.max_y
}

// Resolve on the axis of least penetration so the player slides along walls.
fn resolve_x(player: &mut Player, tiles: &[Tile]) { /* ... */ }
fn resolve_y(player: &mut Player, tiles: &[Tile]) { /* ... */ }
```

### Expected behavior
The player accelerates under gravity, never sinks through a tile, stops on
ground, can jump, and slides along a wall when pushing against it horizontally.
A full AABB test suite verifies every edge: corner contact, one-pixel overlap,
and moving exactly onto a boundary. Headless tests assert the replay is
bit-identical.

### How to run
```bash
cargo run -p openengine-example-platformer    # windowed
cargo test -p openengine-example-platformer    # AABB unit + replay determinism
```

### Purity / determinism expectations
- Logic is pure; collision is a pure function of `Position`/`Aabb` columns.
- Gravity and velocities are `I16F16`; the tests assert no tunneling at
  maximum fall speed within one tick (a property test in spec `17`).
- `verify-wasm-purity` → `[PURE]`; AABB tests run headless and 3× identical.

## Example 3 — Sprites / demo (assets + input)

### What it exercises
The boundary where **assets and input live on the host** (Domain A) and are fed
into Domain B as a *deterministic snapshot*. A host asset store maps
`sprite_id -> texture`; a host input collector turns window/controller events
into a deterministic, tick-stamped input struct. Domain B reads that struct and
a sprite-table column, and returns `WorldDelta` `Render` deferred commands that
name sprites by handle (never raw geometry).

```rust
// Host collects input across the frame and snapshots it deterministically:
struct InputSnapshot { move_x: Fixed, move_y: Fixed, jump: bool, tick: u64 }
// (Asset bytes + input are host concerns; Domain B only sees the snapshot and
//  sprite handle columns.)

// Domain B consumes it purely:
fn move_hero(view: &StateView<'_>, input: &InputSnapshot)
    -> Result<WorldDelta, RecoverableError> { /* ... */ }
```

### Expected behavior
The demo window shows a hero sprite that moves smoothly under input, with an
animated background. When the same recorded input is replayed from a fixed seed,
the sprite trajectory is bit-identical — proving input capture is lossless and
Domain B is deterministic even under live user control.

### How to run
```bash
OPENENGINE_ASSETS_PATH=./assets cargo run -p openengine-example-sprites
# Replay a recorded input sequence headlessly:
cargo test -p openengine-example-sprites -- --ignored   # opt-in replay test
```

### Purity / determinism expectations
- All gameplay (movement, animation selection) is pure and fixed-point.
- Texture bytes and input collection never enter Domain B; only `sprite_id`
  handles and the `InputSnapshot` cross the boundary.
- Sprite animation is tick-driven (no wall clock), so it is deterministic.

## Testing expectations that apply to every example

1. **Headless first.** Every example has a `tests/sim.rs` or `tests/*.rs` that
   runs the full logic with no window and no GPU. The windowed `main.rs` is a
   thin host shell.
2. **Determinism 3×.** Each example's pure sim is run 3 times; the state
   fingerprints must be identical (spec `17`).
3. **Purity gate.** The example's wasm logic passes
   `python3 brain/orchestrator.py verify-wasm-purity <module>` → `[PURE]`.
4. **Portable.** Compiles on `x86_64-linux` and `aarch64-linux`; builds in
   Docker and CI; works offline after `cargo fetch`. No hardcoded paths — use
   `OPENENGINE_ASSETS_PATH` and `CARGO_MANIFEST_DIR`-relative references.
5. **Docs with every example.** Each example has a short README stating what it
   exercises, expected behavior, run commands, and purity/determinism
   expectations (spec `18`).

## Adding a new example

1. Add the crate to `[workspace].members` in the root `Cargo.toml` (no globs —
   onboarding is intentional).
2. Follow the `logic.rs` pure / host `main.rs` split; keep gameplay out of the
   host.
3. Ship a headless test + a determinism test up front.
4. Run the full gate: fmt, clippy, build, `verify-wasm-purity`, determinism 3×,
   Docker. Update this spec's index and the README.

## Command cheat-sheet

```bash
cargo run -p openengine-example-pong        # windowed Pong
cargo test -p openengine-example-pong       # headless Pong logic + determinism
cargo run -p openengine-example-platformer
cargo test -p openengine-example-platformer
OPENENGINE_ASSETS_PATH=./assets cargo run -p openengine-example-sprites
cargo test -p openengine-example-platformer determinism   # the 3×-able tests
```

## Dependencies
- `openengine-contracts` (`StateView`, `WorldDelta`, `DeferredCommand`),
  `openengine-math` (fixed-point), host crates for the window/GPU shell.
- `docs/specs/00-ecs-architecture.md`, `docs/specs/01-game-loop.md`,
  `docs/specs/17-testing-strategy.md`.

## Next steps
1. Land Pong first — it needs only the primitives the ABI already has
   (`ClearColor`) plus a rect command; keep logic pure.
2. Land Platformer once the ECS column read/write path (`00`) is real.
3. Land Sprites/demo once host asset + input snapshot plumbing exists.
4. Register each in the README examples section and this index as it ships.
