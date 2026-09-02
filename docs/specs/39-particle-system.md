---
spec: "39-particle-system"
phase: "Phase 5: VFX & Rendering"
status: "draft"
author: "OpenEngine AI"
created: "2026-09-03"
depends_on:
  - "00-ecs-architecture"
  - "02-asset-pipeline"
  - "04-render-pipeline"
  - "16-serialization"
  - "21-primitive-components"
  - "22-edit-vs-play"
  - "23-undo-redo"
---
# 39 - Niagara-Style Deterministic Particle System

## Overview

A VFX particle system in the Niagara mould: an **emitter** entity (a scene
object with a `Transform`) declares *how and how often* particles are born, and
each live particle carries per-particle properties — position, velocity, color,
size, lifetime — held **Structure-of-Arrays (SoA)** so the hot loop is
cache-friendly and column-contiguous. Simulation is **seeded and pure**:

* A **CPU path** runs in Domain B as a fixed-point pure system so that
  particle positions, colors, and lifetimes are **reproducible** given the same
  scene, the same injected seed, and the same tick — matching the repository's
  determinism law (AGENTS.md § 3). This is the *correctness reference* and the
  path for modest emitter counts (thousands of particles).
* A **GPU/compute path** runs in Domain A for 10k+ particles per emitter. It is
  *presentation-grade* throughput work and never feeds back into gameplay, but
  it must **consume only deterministic inputs** (the fixed-point emitter /
  module columns and a seed) so that the rendered burst is reproducible from the
  same seed + tick and so the CPU and GPU paths can be cross-checked headlessly
  against a shared fixed-point fixture.

Rendering never blocks the simulation. The particle system produces, per tick,
**`ColumnWrite` updates** into a per-emitter particle record column plus a
deferred **`RenderCommand`/`DrawMesh`** for the billboard/instanced draw. Domain
A owns the GPU buffers; Domain B only ever *describes* the next particle state
through a [`WorldDelta`].

VFX in OpenEngine is authored in the **edit world** (spec 22) through undoable
editor `Command`s (spec 23) — e.g. a `SpawnParticleSystemCommand` or a value
change on a `ParticleModule` field. A **preview** instance may run in the
editor viewport (spec 24); a **play** instance deep-clones the authored emitter
and simulates it.

## Core Concepts

### Emitter entities, not per-particle entities

A spawning/moving actor that emits is one entity carrying `Transform`,
`ParticleEmitter` (ComponentId 20) and `ParticleModule` (ComponentId 21)
components (plus optional `Name`/`Parent`). Live particles are **not** ECS
entities — spawning and despawning thousands of `Entity`s a tick is far too
heavy and would thrash archetype tables (spec 00). Instead each live particle is
one **row** in a per-emitter particle record that the ECS/emitter host allocates
for the emitting entity. The record is conceptually *owned by* the emitter and
is keyed by `(emitter_entity, index_in_buffer)`.

```
  ┌──────────────┐   module/shape/force   ┌─────────────────────────┐
  │  Domain B    │   columns (fixed)      │  Domain A               │
  │  CPU sim     │ ─────────────────────► │  emitter host resource  │
  │  pure system │   per-tick writes      │  per-emitter SoA ring   │
  │  (seeded)    │   to particle record   │  + GPU instance buffer  │
  └──────────────┘                        └───────────┬─────────────┘
                                                      │ RenderCommand/
                                                      │ DrawMesh (deferred)
                                                      ▼
                                               wgpu billboard /
                                               instanced mesh draw
```

### Per-particle properties (SoA) — not gameplay `f32`

The per-particle record stores the *gameplay-relevant, deterministically
simulated* fields in **fixed-point**. `f32` exists only at the GPU emission
boundary (billboard vertices / instance buffer), exactly as spec 04 does for
`Transform`. Domain B reads and writes fixed values; Domain A quantizes to `f32`
(`openengine-math::quantize_to_f32`) when it uploads instances.

### Modules configure the simulation

Following Niagara, behaviour is decomposed into **modules**, each a parameter
block with an *enabled* flag. **Emitter modules** decide when/how particles
appear; **particle modules** evolve each particle's per-tick state:

* *Emitter modules* — spawn **rate** (particles/tick), **burst** (N at time T,
  possibly once or looping), **shape** (where a new particle starts: point,
  sphere shell, cone/ring, box).
* *Particle modules* — **force** (gravity, drag, constant/curl acceleration),
  **color-over-lifetime** (a small fixed set of RGBA keyframes), **size-over-
  lifetime** (keyframes). Modules compose: enabled flags select which blocks
  participate, and evaluation order is fixed so the result is reproducible.

Because modules are **authored data that Domain B must read**, their parameters
live in the `ParticleModule` component column (fixed-point `Pod`), not in a
host-side asset that the guest cannot see. A shared `EmitterTemplate` asset
(spec 02) may hold the *preset* used to initialise the column in the editor; the
authoritative values the CPU sim reads are always the ECS column.

### Deterministic seeding → reproducible VFX

Every emitter carries a `master_seed` (a `u32`, authorable or auto-assigned at
spawn). The CPU sim derives a per-tick stream from
`Hash64(master_seed, emitter_index, tick, burst_id)` using a pure integer PRNG
(no `std`, no ambient randomness). Identical `(scene, master_seed, tick)` ⇒
identical emission and identical particle states on `x86_64-linux`,
`aarch64-linux`, and in Wasm. The **same seed drives the GPU path**: the compute
shader receives the seed and tick as push constants and advances the identical
PRNG, so a GPU render of the same emitter reproduces the CPU fixture's output
*where they overlap* (GPU is allowed per-particle float jitter only for purely
presentational channels, see Constraints).

### Pure system output is deltas

The CPU path returns a [`WorldDelta`] whose:

* `writes` carry the per-emitter **particle record `ColumnWrite`s** (the next
  fixed particle fields) and any `ColumnWrite` on the `ParticleEmitter` column
  itself (e.g. a "stopped/looping" flag update);
* `deferred` carry a **`RenderCommand`/`DrawMesh`** for the emitter's
  billboard/instanced mesh so the host draws exactly what the sim just advanced,
  plus an optional `Emit { topic, data }` for gameplay (e.g. "emitter finished").

Domain A applies the delta, updates the ring buffer, and submits the draw — the
single-threaded, deterministic path is never bypassed by the editor tools.

### Loop lifetime

An emitter has `start_tick`, `duration`, and `looping`. A CPU-side *emitter
clock* is recomputed each tick from world columns (no hidden host timer) so
pausing/stepping in the editor (spec 22) and replaying are exact: while the play
world is frozen the emitter is frozen with it, and re-simulating from a snapshot
tick reproduces the burst.

## Key Rust Types

```rust
//! Domain B — crates/logic-sandbox (or a no_std crates/particle re-exported
//! by logic-sandbox). Pure, fixed-point, no GPU. Component mirror types live
//! in the shared Pod component crate (spec 21) so both domains agree on layout.

use openengine_math::I16F16;
use contracts::{Entity, StateView, WorldDelta, ColumnWrite, ComponentId,
                DeferredCommand, RenderKind, RecoverableError};

/// One live particle row (fixed gameplay state). Domain A mirrors this as
/// several SoA columns and uploads f32 billboard instances from it. It is NOT a
/// registered ComponentId — it is a per-emitter internal record row.
#[repr(C)]
#[derive(Clone, Copy, bytemuck::Pod, bytemuck::Zeroable,
         serde::Serialize, serde::Deserialize, Debug)]
pub struct Particle {
    pub pos:     [I16F16; 3],  // world/local space per ParticleEmitter.sim_space
    pub vel:     [I16F16; 3],
    pub color:   [I16F16; 4],  // RGBA, each in [0,1] (quantize at GPU)
    pub size:    I16F16,       // world units (billboard half-extent basis)
    pub age:     I16F16,       // elapsed lifetime, ticks
    pub max_age: I16F16,       // lifetime at spawn, ticks
}

/// Pure, integer PRNG used only for VFX choice randomness. State is threaded
/// through the return value so the caller stays pure (no &mut, no globals).
#[repr(C)]
#[derive(Clone, Copy, bytemuck::Pod, bytemuck::Zeroable)]
pub struct RngState(pub u64);
pub fn rng_next(mut s: RngState) -> (u32, RngState) {
    // splitmix64-style step over u64; cheap, integer-only, deterministic.
    s.0 = s.0.wrapping_add(0x9E37_79B9_7F4A_7C15);
    let mut z = s.0;
    z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
    z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
    (z as u32 ^ (z >> 31) as u32, s)
}

/// Pure integer hash: seed a stream from (master_seed, emitter, tick, burst).
pub fn stream_seed(master: u32, emitter: Entity, tick: u64, burst: u32) -> RngState {
    let mut h = master as u64;
    h ^= emitter.index as u64 * 0x9E37_79B9_7F4A_7C15;
    h ^= emitter.generation as u64 * 0xBF58_476D_1CE4_E5B9;
    h ^= tick.wrapping_mul(0x94D0_49BB_1331_11EB);
    h ^= burst as u64;
    RngState(h)
}

/// Pure system: advance every live particle of an emitter by one tick and
/// materialise new spawns according to the emitter + module columns.
pub fn particle_sim_system(view: &StateView<'_>, emitter: Entity)
    -> Result<WorldDelta, RecoverableError> {
    // 1. read the ParticleEmitter + ParticleModule + Transform columns (fixed)
    // 2. emitter clock from columns; emit spawn-rate + due bursts
    // 3. for each new particle: shape module picks pos/vel; colour/size keyframes
    //    at age 0; rng from stream_seed(...); write a fresh record row
    // 4. for each live particle: force module integrates vel/pos in fixed,
    //    age += 1; kill age >= max_age; write updated rows
    // 5. assemble record ColumnWrites + DeferredCommand::Render/DrawMesh
    Ok(WorldDelta::default()) // illustrative — real impl fills writes/deferred
}
```

### Render path (Domain A) is deferred and non-blocking

```rust
//! crates/render / crates/core — Domain A.
/// Build billboard/instanced draw from a committed particle record.
pub struct ParticleDrawJob {
    pub emitter: Entity,
    pub mesh:     Option<AssetHandle<Mesh>>, // None => camera-facing billboard quad
    pub material: AssetHandle<Material>,
    pub instance_buffer: wgpu::Buffer,       // f32, uploaded at GPU boundary
    pub instance_count: u32,
}
```

The CPU record (fixed) → GPU instance buffer (f32) conversion is a pure
Domain-A mapping (`quantize_to_f32`), kept separate from the wgpu submit so it
is headlessly testable.

## Components

Registered in the stable ComponentId window **20–29** (see spec 21 ID policy:
IDs are permanent, never renumbered). Components 20 and 21 are owned by this
spec. Per-particle record rows are **not** registered ComponentIds — they are a
host/ECS per-emitter internal column (mirroring spec 42's "dense data is a
host-managed column" decision) so ECS tables stay small.

| `ComponentId` | Name              | `size_of` (target) | Domain use (owner)                        |
|---------------|-------------------|--------------------|--------------------------------------------|
| **20**        | `ParticleEmitter` | 32 B               | Emission instance: mode, seed, loop, sim-space, material/mesh refs (spec 39). |
| **21**        | `ParticleModule`  | 96 B               | Emitter + particle module parameter blocks, enabled flags (spec 39). |
| 8             | `Light`           | 20 B               | Reused for light-emitting VFX if needed (spec 21). |

```rust
/// Emission instance declaration. ComponentId 20. Fixed-point + u32 flags.
#[repr(C)]
#[derive(Clone, Copy, bytemuck::Pod, bytemuck::Zeroable,
         serde::Serialize, serde::Deserialize, Debug)]
pub struct ParticleEmitter {
    pub template:   AssetRef,     // logical EmitterTemplate token; NONE = inline
    pub material:   AssetRef,     // logical material token (billboard/mesh)
    pub mesh:       AssetRef,     // NONE => camera-facing billboard quad
    pub mode:       EmitterMode,  // u8: Cpu=0, Gpu=1, Auto=2 (Auto: small=>Cpu)
    pub sim_space:  SimSpace,     // u8: Local=0, World=1
    pub flags:      u8,           // bit0 looping, bit1 enabled, bit2 burst-once
    pub _pad:       u8,
    pub master_seed: u32,         // deterministic seed (authorable)
    pub max_particles: u32,       // hard cap for this emitter's ring buffer
    pub duration:   I16F16,       // ticks per loop (0 => infinite/rate-driven)
}

#[repr(u8)]
#[derive(Clone, Copy, bytemuck::Pod, bytemuck::Zeroable,
         serde::Serialize, serde::Deserialize, Debug)]
pub enum EmitterMode { Cpu = 0, Gpu = 1, Auto = 2 }
#[repr(u8)]
#[derive(Clone, Copy, bytemuck::Pod, bytemuck::Zeroable,
         serde::Serialize, serde::Deserialize, Debug)]
pub enum SimSpace { Local = 0, World = 1 }
```

```rust
/// Module configuration block readable by Domain B. ComponentId 21.
/// Each module carries an `enabled` flag so evaluation order is fixed and only
/// selected blocks contribute. All scalars fixed-point (never f32).
#[repr(C)]
#[derive(Clone, Copy, bytemuck::Pod, bytemuck::Zeroable,
         serde::Serialize, serde::Deserialize, Debug)]
pub struct ParticleModule {
    // emitter modules
    pub spawn_rate_enabled: u8,   pub spawn_rate_pad: [u8; 3],
    pub spawn_rate: I16F16,       // particles per tick (0 => burst-only)
    pub burst_enabled:  u8,       pub burst_pad: [u8; 3],
    pub burst_count: I16F16,      // particles per burst
    pub burst_interval: I16F16,   // ticks between bursts (0 => one-shot)
    pub shape: ShapeKind,         // u8: Point/SphereShell/Cone/Ring/Box
    pub shape_enabled: u8,        pub shape_pad: [u8; 2],
    pub shape_r: I16F16,          // radius / half-extent scale
    pub shape_h: I16F16,          // height (cone) / thickness (sphere shell)
    // particle modules
    pub force_enabled: u8,        pub force_pad: [u8; 3],
    pub gravity: I16F16,          // y acceleration, world/s^2 in ticks
    pub drag: I16F16,             // 0..1 per-tick velocity damp
    pub accel: [I16F16; 3],       // constant acceleration
    pub color_enabled: u8,        pub color_pad: [u8; 3],
    pub color0: [I16F16; 4],      // RGBA keyframe at age 0
    pub color1: [I16F16; 4],      // RGBA keyframe at age 1 (full life)
    pub size_enabled: u8,         pub size_pad: [u8; 3],
    pub size0: I16F16,            // size at age 0
    pub size1: I16F16,            // size at age 1
}
```

### Editor integration

- **Authoring (edit world).** Spawning/placing an emitter and editing any
  `ParticleEmitter`/`ParticleModule` field are spec-23 `Command`s applied to the
  edit world (spec 22). The `ParticleModule` inspector (spec 07) toggles the
  `*_enabled` flags and edits fixed parameters; a module graph UI is deferred but
  still lands as `ParticleModule` value changes.
- **Preview.** The editor viewport (spec 24) may run a bounded CPU or GPU preview
  of an emitter in **edit** mode for feedback. A preview is a throwaway play-like
  sim over the edit world's emitter columns — it never writes back; a "bake to
  play" is simply pressing Play (spec 22).
- **Commands.** `SpawnParticleSystemCommand`, `DespawnParticleSystemCommand`,
  `ModifyComponentCommand` on the two columns — all standard spec-23 commands.

## Constraints

- **Determinism law.** All particle *gameplay* math (position/velocity/color/
  size/lifetime, force, shape sampling, emission counting) is Domain B
  fixed-point (`openengine-math::I16F16`, widened to `I32F32` for products where
  needed, exactly like spec 13). `f32` appears only in Domain A/GPU instance
  upload and billboard vertices (spec 04 discipline).
- **Seeded, pure RNG.** No ambient randomness, no wall clock, no `HashMap`
  iteration in simulation (AGENTS.md § 3). `master_seed` + emitter + tick +
  burst_id fully determines the stream. Same seed + tick ⇒ bit-identical output
  on all targets and in Wasm.
- **Guest never allocates the hot buffer.** Live particles live in a
  host/ECS per-emitter record column; Domain B returns `ColumnWrite`s of fixed
  rows and deferred `RenderCommand`s through a [`WorldDelta`]. No `&mut` to
  shared sim memory crosses into the guest (spec 00).
- **GPU path is Domain A and consumes only deterministic inputs.** The compute
  shader reads the fixed emitter/module columns (uploaded once) plus the seed and
  tick as push constants and advances the integer PRNG. Per-particle *presentational*
  channels (sub-texel billboard jitter, camera-facing billboard) may use GPU float
  only where the CPU/GPU cross-check excludes them — never a simulated gameplay value.
- **CPU vs GPU agreement.** For a shared fixture, the CPU reference and the GPU
  compute path must agree on spawn times/counts and on the deterministic fields;
  any divergence is a bug. (Small float-only presentation differences are allowed
  and documented.)
- **One writer to the record.** Only the particle sim (CPU Domain B path or the
  GPU compute path, one active per emitter per tick) writes a record; the editor
  and gameplay never mutate particle rows directly.
- **Hard cap.** `max_particles` bounds the ring buffer; an emitter over its cap
  drops *oldest* particles deterministically (fixed eviction), never arbitrarily.
- **Serializable.** `ParticleEmitter`/`ParticleModule` are `Pod + Zeroable +
  serde` and round-trip through the spec-16 codec; particle *records* are
  reproducible from (seed, tick), so play snapshots need not persist them.
- **Portability.** `AssetRef` logical tokens only (no absolute paths),
  `x86_64-linux` + `aarch64-linux`, no GPU required by logic tests.

## Performance Targets

- CPU sim per particle per tick (force + age + kill check): **≤ ~40 ns**
  (SoA, single column pass, no per-particle branching beyond fixed keyframe).
- CPU sim, 5k live particles on one emitter: **≤ ~1 ms/tick** in Domain B
  (within the 16 ms/tick and 256 MB budgets).
- GPU path supports **10k–100k particles/emitter** at 60 Hz with negligible
  Domain-B cost (emission *decided* deterministically host-side; only counts and
  seed shipped to the GPU).
- Record→instance upload conversion (quantize + fill): **< 0.5 ms** for 50k.
- Domain B allocations: buffers reused across ticks; hot loop allocation-free.

## Testing Strategy

All headless (no GPU) in Domain B + editor unit tests unless noted:
- **Determinism.** Simulate an emitter with several modules 3× on two targets
  (and in Wasm) and assert byte-identical per-particle output columns (spec 13
  protocol).
- **Seed reproducibility.** Same `(master_seed, tick)` ⇒ identical burst; two
  different seeds ⇒ (over a large sample) different, still-valid output.
- **Module composition.** Assert spawn-rate vs burst-only emissions; each
  `*_enabled` flag toggles exactly its block; evaluation order is stable.
- **Shape sampling.** Point/sphere/cone/ring/box spawns land within the expected
  region and match golden fixed fixtures.
- **Force integration.** A gravity-only particle matches the closed-form fixed
  solution at several ticks; drag converges monotonically to rest.
- **Color/size over lifetime.** Keyframes at age 0/1 produce expected lerped
  values at intermediate ages; clamping at age ≥ max_age.
- **Ring cap eviction.** Oldest-first eviction at `max_particles`, deterministic.
- **Command path.** `SpawnParticleSystemCommand`/`ModifyComponentCommand` on the
  two columns execute/undo to a bit-identical edit world (spec 23).
- **Edit/Play.** Author an emitter, deep-clone to play (spec 22), step N ticks;
  assert the play burst equals a same-seed fresh play from the same snapshot.
- **Render command.** A committed record yields the expected ordered
  `DeferredCommand::Render`/`DrawMesh`; the record→instance conversion is a
  headless pure function (golden `f32` buffers).
- **Purity.** `verify-wasm-purity` reports `[PURE]`.
- **GPU smoke (device only, not logic tests):** render one GPU-path emitter and
  assert its emitted counts match the CPU fixture.

## Dependencies

- `contracts` (`StateView`, `WorldDelta`, `ColumnWrite`, `DeferredCommand`,
  `RenderKind`, `Entity`, `ComponentId`, `RecoverableError`).
- `openengine-math` (`I16F16`, `I32F32`, `quantize_to_f32`), `bytemuck`,
  `serde`, `postcard`, `alloc`.
- `crates/ecs` (record column + archetype), `crates/core`/`crates/render`
  (billboard/instance draw, spec 04), `crates/editor` (commands + preview,
  specs 23/24), `crates/asset` for `EmitterTemplate` presets (spec 02).
- Pod mirror of `ParticleEmitter`/`ParticleModule` + spec 21 `AssetRef`/`Light`.
- *ABI extension note:* transporting per-particle record rows and the
  particle `DeferredCommand::Render` payload over the boundary extends
  `contracts` — requires an `ARCH_VERSION` bump with a matching `docs/abi/`
  update and all consumers rebuilt in the same commit (AGENTS.md § 7 / § 9).

## Next Steps

1. Register `ParticleEmitter` (20) and `ParticleModule` (21) Pod components +
   the ComponentId bindings (`C_PARTICLE_EMITTER`, `C_PARTICLE_MODULE`).
2. Add the per-emitter record column + host apply path for fixed particle rows.
3. Land the pure integer PRNG + `stream_seed` helpers in `openengine-math`.
4. Implement CPU `particle_sim_system` (emission, shape, force, color/size,
   aging, ring eviction) + determinism/purity tests.
5. Add the record→instance `quantize_to_f32` conversion + billboard DrawMesh path.
6. Wire the editor `Command`s (spec 23) and edit-mode preview (spec 24).
7. Implement the GPU/compute path consuming deterministic inputs; add the
   CPU/GPU cross-check fixture.
8. Author `docs/abi/` + bump `ARCH_VERSION` for the particle transport additions.
