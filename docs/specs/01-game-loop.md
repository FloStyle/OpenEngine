---
spec: "01-game-loop"
phase: "Phase 4"
status: "design"
---

# Game Loop

## Overview

Fixed-timestep simulation with interpolation for smooth rendering. The host
(domain A, `crates/core`) owns the loop; Domain B systems run at a fixed rate
and never see wall-clock time — only the injected tick.

## Loop structure

```rust
pub struct GameLoop {
    accumulator: f64,          // wall-time carried since the last fixed tick
    fixed_delta: f64,          // 1/60 (configurable)
    sim_time: f64,             // simulation time (advances in fixed steps)
    last_frame_time: f64,      // for computing frame dt
    phases: Vec<ScheduledSystem>,
    world: World,
    logic: Logic,              // wasmtime bridge to Domain B
    renderer: Renderer,
}

pub enum SystemPhase {
    PreUpdate,     // input, camera — every frame
    FixedUpdate,   // physics, gameplay — fixed rate
    PostUpdate,    // cleanup, queued spawn/despawn
    Render,        // interpolation + submit — display rate
}
```

## Main loop

```rust
fn run(mut self, event_loop: EventLoop<()>) {
    let mut last = Instant::now();
    event_loop.run(move |event, elwt| match event {
        Event::AboutToWait => {
            let now = Instant::now();
            let frame = now.duration_since(last).as_secs_f64();
            last = now;

            // Clamp to avoid the "spiral of death" after a stall.
            self.accumulator += frame.min(self.fixed_delta * 5.0);

            while self.accumulator >= self.fixed_delta {
                self.accumulator -= self.fixed_delta;
                self.sim_time += self.fixed_delta;
                self.fixed_update();
            }

            let alpha = (self.accumulator / self.fixed_delta).clamp(0.0, 1.0);
            self.render(alpha);
        }
        Event::WindowEvent { event: WindowEvent::CloseRequested, .. } => elwt.exit(),
        _ => {}
    });
}
```

## Fixed update

```rust
fn fixed_update(&mut self) {
    self.pre_update();                       // input → deterministic snapshot
    let view = self.logic.build_state_view(&self.world, self.sim_time);
    for sys in self.phases.iter().filter(|s| matches!(s.phase, SystemPhase::FixedUpdate)) {
        let delta = self.logic.run_pure(sys.system, &view)?;
        apply_delta(&mut self.world, &delta);
    }
    self.world.flush();                      // queued spawn/despawn
}
```
Systems run against the *same* immutable `StateView`; deltas are applied in
registration order. Deterministic by construction.

## Render (interpolation)

```rust
fn render(&mut self, alpha: f64) {
    let interpolated = self.logic.build_interpolated_view(&self.world, alpha);
    let mut commands = Vec::new();
    for sys in self.phases.iter().filter(|s| matches!(s.phase, SystemPhase::Render)) {
        let out = self.logic.run_render(sys.system, &interpolated)?;
        commands.extend(out);
    }
    self.renderer.submit(&commands, self.camera());
}
```
Only visual properties (position, rotation) are interpolated. Fixed gameplay
state is never interpolated.

## System registration

```rust
impl GameLoop {
    pub fn add_fixed(&mut self, system: PureSystem) { /* ... */ }
    pub fn add_preupdate(&mut self, system: PureSystem) { /* ... */ }
    pub fn add_render(&mut self, system: RenderSystem) { /* ... */ }
}
```

## Time injection
`StateView` carries `tick: u64` (frame counter) plus optional fixed-point
`delta_time`/`sim_time` helpers derived from `fixed_delta`, so Domain B logic
can reason in fixed units deterministically.

## Constraints
- Fixed timestep exactly 60 Hz by default (configurable).
- Render rate = display refresh (vsync via `PresentMode::Fifo`).
- Clamp `accumulator` to `5 * fixed_delta` (no spiral of death).
- Deterministic: same events + fixed seed ⇒ identical simulation.

## Performance targets
- Fixed update < 8 ms; render < 8 ms; total < 16.67 ms @ 60 Hz.

## Testing strategy
- Unit: accumulator / timestep math, phase ordering.
- Integration: run 1000 fixed ticks from a seeded world, assert bit-identical
  state across 3 runs.
- Stress: 100k entities — measure frame time.

## Dependencies
- `winit`, `instant` (timing), `contracts` (`StateView`, `WorldDelta`).

## Next steps
1. `GameLoop` struct in `crates/core`.
2. Fixed-timestep main loop wiring winit.
3. Phase registration + scheduling.
4. Integrate the safe memory bridge (see `00-ecs-architecture.md`,
   `.agents/knowledge/PATTERNS.md`).
