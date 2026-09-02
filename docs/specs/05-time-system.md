---
spec: "05-time-system"
phase: "Phase 4"
status: "design"
---

# Time System

## Overview

A single, deterministic notion of *simulation time* that all systems share,
plus the *wall-clock* machinery in Domain A that advances it. The host
(`crates/core`, spec `01-game-loop`) reads the OS clock and converts elapsed
real time into a fixed-rate sequence of simulation ticks; Domain B never sees
`std::time`. Everything a pure system needs — the tick number and the
fixed-point `delta_time` / `sim_time` for that step — is injected through
[`StateView`] exactly like every other read of world state.

The time system answers four questions that are easy to get wrong:

1. **What time is it now?** Wall-clock, Domain-A only, via the `instant` crate
   (winit-aligned, cross-platform, no `std::time` leakage into logic).
2. **When does simulation advance?** Only in whole, fixed 60 Hz steps.
3. **How much wall time is owed?** A clamped accumulator that rejects the
   "spiral of death" after a frame stall.
4. **What should the renderer see?** A frame `delta_time` plus an interpolation
   `alpha` between the previous and next fixed states.

## Design

### Two clocks, one number

* **Wall clock (`Clock`, Domain A).** Measured with `instant::Instant`. Used to
  (a) budget frame work and (b) feed the accumulator. It is *non-deterministic
  by nature* and is therefore confined to `crates/core`. Nothing in this number
  ever crosses into a `StateView`.
* **Sim clock (`SimTime`, deterministic).** An integer tick count plus an exact
  `openengine-math` fixed-point time-in-seconds derived from the tick and the
  fixed step. This is what Domain B reasons about. It advances *only* when a
  fixed step actually runs, so two hosts that run the same sequence of ticks
  see identical simulation time regardless of how fast each physical machine
  is.

```
        wall clock (instant, A)  ──frame delta──▶  accumulator
                                                       │  clamp → max_steps
                                                       ▼
                                           while acc ≥ fixed_delta:
                                               acc -= fixed_delta
                                               tick += 1
                                               sim_time += fixed_delta
                                               run fixed systems
                                                       │
                                  alpha = acc / fixed_delta ──▶ render (interpolated)
```

### The 60 Hz step

The default fixed step is `1/60` s. It is a compile-time constant
(`TimeStep::HZ_60`) so the arithmetic is exact; a runtime override is a design
review item because it changes every `delta_time` a pure system sees (an
`ARCH_VERSION`-adjacent concern, not a casual knob).

The step is defined in fixed-point, not as `f64`. `60 Hz` corresponds to a
`I16F16` step of `fx!(1.0/60.0)`; because `1/60` is not a finite binary
fraction, we store the *tick* as truth and treat `sim_time = tick * step` as a
derived value. Systems should prefer reading `tick` and multiplying by an exact
step constant rather than accumulating a `sim_time` that drifts from rounding.
The fixed-point `sim_time` injected into the view is thus recomputed from
`tick` each step (`tick.checked_mul`, exact in integer math), never summed
incrementally.

### Frame delta, accumulator, spiral-of-death guard

Each `AboutToWait` event (spec `01-game-loop`):

```rust
let now = Clock::now();
let frame = now.duration_since(self.last_frame).as_secs_f64();
self.last_frame = now;

// Clamp: never let a single stall schedule more than MAX_STEPS of work.
// A 100 ms GC hiccup must not queue 6 sim steps that then catch up in a
// burst — that bursts the fixed timestep and the interpolation alpha.
let owed = (frame / self.step_secs()).min(MAX_STEPS as f64);
self.accumulator += owed;
```

`MAX_STEPS` (default 5, per spec `01-game-loop`) is the **spiral-of-death
guard**: if the accumulator were allowed to grow without bound, a slow machine
would spend every frame running an ever-growing backlog of fixed steps and
never render. By capping how many steps a single frame may schedule we keep
frame time bounded and drop wall time we can no longer honor (simulation runs
"behind" — preferable to freezing).

### Injection into `StateView`

The pure ABI today is `tick: u64` (see `contracts`, [`StateView`]). The time
system extends that read with **deterministic fixed-point** values the guest
can use without any clock:

```rust
pub struct TimeView { /* proposed, see ABI note below */ }
```

Concretely, the two values Domain-B systems most often want are the step
length and the running sim time in `I16F16`. Because a *real* addition of a
field to [`StateView`] is a breaking ABI change, the design is:

* Ship `tick` (already present) as the source of truth immediately — no ABI
  break.
* When gameplay needs fractional time, add *fixed-point* fields
  (`sim_time: openengine_math::I16F16`, `delta_time: I16F16`) to a view
  handed to Domain B. This is an `ARCH_VERSION` bump performed under the
  `contracts/` procedure (bump → `docs/abi/` update → all consumers rebuilt in
  one commit). See **ABI note** below.

Nothing in Domain B calls `std::time::Instant`, `Instant::now`,
`SystemTime`, or any time function. CI forbids the import (see
**Constraints**). All timing that reaches logic is *injected*, never queried.

### Interpolation alpha

Rendering happens at display rate (possibly higher than 60 Hz, and possibly
lower when frames drop). Between fixed states the renderer blends visual
properties (position, rotation — never gameplay truth) by:

```rust
let alpha = (self.accumulator / self.step_secs()).clamp(0.0, 1.0);
```

`alpha` is pure Domain-A math (`f64` is fine here — it is display plumbing,
not gameplay). For the *render path that wants determinism*, alpha can be
quantized with `openengine-math::quantize_to_f32` (spec `01`, AGENTS.md
§ 3). Visual interpolation is documented in spec `01-game-loop`; the time
system owns producing `alpha`, not consuming it.

## Key Rust / types

```rust
// crates/core/src/time/mod.rs  (Domain A)
use instant::{Duration, Instant};

pub const FIXED_HZ: u64 = 60;
pub const MAX_STEPS_PER_FRAME: u32 = 5;   // spiral-of-death guard

/// One whole simulation step. Tick-counted; sim_time is derived.
#[derive(Clone, Copy)]
pub struct SimStep { pub tick: u64 }

impl SimStep {
    pub fn delta_time() -> openengine_math::I16F16 { fx!(1.0 / 60.0) }
    /// Derived, recomputed from tick each step (never incrementally summed).
    pub fn sim_time(self) -> openengine_math::I16F16 {
        fx!(1.0 / 60.0) * openengine_math::I16F16::from_num(self.tick)
    }
}

/// Domain-A only. Non-deterministic by definition — never crosses the ABI.
pub struct WallClock { origin: Instant }

pub struct TimeKeeper {
    accumulator: f64,     // owed wall-time, in step units or seconds
    tick: u64,            // current sim tick
    last_frame: Instant,  // wall time of the previous frame
}

impl TimeKeeper {
    /// Advance by one frame's wall delta. Returns how many fixed steps to run.
    pub fn on_frame(&mut self) -> u32 {
        let now = Instant::now();
        let frame = now.duration_since(self.last_frame).as_secs_f64();
        self.last_frame = now;
        self.accumulator += (frame * FIXED_HZ as f64).min(MAX_STEPS_PER_FRAME as f64);
        let mut steps = 0;
        while self.accumulator >= 1.0 {   // 1.0 == one 60 Hz step
            self.accumulator -= 1.0;
            self.tick = self.tick.wrapping_add(1);
            steps += 1;
        }
        steps
    }
    pub fn tick(&self) -> u64 { self.tick }
    pub fn alpha(&self) -> f64 { self.accumulator.clamp(0.0, 1.0) }
}
```

The `StateView` extension (proposed, ABI-gated) carries the derived values:

```rust
// contracts/src/lib.rs — PROPOSED fields on StateView; requires ARCH_VERSION bump.
// pub delta_time: openengine_math::I16F16,  // == SimStep::delta_time()
// pub sim_time:   openengine_math::I16F16,  // == SimStep::sim_time()
```

> **ABI note:** adding fields to a `Copy` struct in `contracts` is a layout
> change and therefore an `ARCH_VERSION` bump (AGENTS.md § 2). This spec does
> *not* mandate it. Teams that need fractional sim time in Domain B open that
> RFC; until then Domain B uses `StateView::tick` and exact step constants,
> which is sufficient and fully deterministic.

## Constraints

- **No `std::time` in Domain B.** `instant` / `std::time` are Domain-A only.
  CI greps Domain-B sources for `Instant::now`, `SystemTime`, and `time::` to
  reject accidental imports (same class of check as the wasm-purity gate).
- Fixed timestep exactly 60 Hz by default; tick is the source of truth.
- `sim_time` is *derived* from `tick`, never accumulated, so it cannot drift
  between two hosts that run the same ticks.
- Accumulator is clamped to `MAX_STEPS_PER_FRAME` per frame (no spiral of
  death). When frames stall, sim time runs behind wall time rather than
  bursting.
- Interpolation touches only *visual* properties; fixed gameplay state is never
  interpolated (spec `01`).
- All fractional simulation math uses `openengine-math` fixed-point; `f64`
  appears only in Domain-A accumulator/alpha plumbing and is quantized at the
  ABI boundary if it ever must reach logic.
- No OS-specific clock code (`instant` abstracts it); compiles on
  `x86_64-linux` and `aarch64-linux`.

## Performance targets

- Fixed-step bookkeeping (accumulator, tick increment, derived `sim_time`):
  sub-microsecond per step — pure integer math, no allocation.
- Per-frame overhead of `TimeKeeper::on_frame`: negligible (< 1 µs).
- No allocations in the time hot path; `SimStep` and `TimeKeeper` are `Copy`
  value types.
- The time system itself must never appear in a profile hot spot; it is a
  per-frame constant, not a per-entity cost.

## Testing strategy

- **Unit:** accumulator math — given synthetic frame deltas, assert the exact
  number of steps produced and the residual alpha. Feed frames of exactly
  `1/60` s, `0.5/60` s, and `5/60` s.
- **Spiral-of-death guard:** a single frame of 10 s (a stall) must schedule at
  most `MAX_STEPS` steps, never a runaway backlog.
- **Determinism:** run the same sequence of `N` ticks three times and assert
  identical derived `sim_time` and identical pure-system output
  (`tick_color`-style) — see spec `01` and the wasm-purity protocol.
- **No-time-in-logic gate:** Domain-B unit tests fail to compile if any
  `std::time` symbol is referenced; the CI purity gate reports `[PURE]`.
- **Cross-platform:** fixed arithmetic is bit-identical on x86_64 and aarch64;
  assert a golden `sim_time` value after 10 000 ticks.

## Dependencies

- Domain A: `instant` (timing), `contracts` (`StateView`), `crates/core`
  (game loop). Reuses `openengine-math` if fixed sim time is injected.
- Domain B: none — timing is injected through `StateView` or exact constants.

## Next steps

1. Implement `TimeKeeper` + `WallClock` in `crates/core/src/time/`.
2. Wire `TimeKeeper::on_frame` into the `AboutToWait` path of spec `01`.
3. Feed `tick` into every `StateView`; confirm Domain-B systems read `tick`.
4. Add the spiral-of-death unit test and the no-`std::time` CI gate.
5. (RFC) Add `delta_time`/`sim_time` `I16F16` fields to `StateView` under the
   `ARCH_VERSION` procedure only if a gameplay system genuinely needs them.
