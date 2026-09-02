---
spec: "11-debugging-tools"
phase: "Phase 5"
status: "design"
---

# Debugging Tools

## Overview

An in-editor diagnostics suite for OpenEngine that gives a developer (and an
agent) visibility into a frame end to end: how long each phase/system took, what
the ECS actually holds, what the renderer submitted, and whether simulation is
still deterministic. Every tool lives in **Domain A** (`crates/core`,
`crates/editor`), reads world and pipeline state from the host, and renders into
the egui overlay. None of it runs in the wasm sandbox, so nothing here can
perturb Domain B's determinism — instrumentation is purely observational and
outside the pure path.

The tools share one rule: **a profiler or inspector that must not change
behaviour.** Debug panels therefore consume *snapshots* taken at well-defined
points rather than poking live ECS internals mid-system.

## Core Concepts

### Frame profiler (Domain A)

Timings are recorded around the host phases from the game loop — `PreUpdate`,
`FixedUpdate`, `PostUpdate`, `Render` — and, when a host "system" runs inside a
phase, per-system. Because Domain B runs through `wasmtime`, guest systems are
measured **as host-side wall-clock spans around the `wasmtime` call**, not from
inside the guest (a guest could not read a clock anyway).

```rust
pub struct ProfileFrame {
    pub tick: u64,
    pub sim_time_us: u64,            // fixed-time budget consumed
    pub phases: Vec<PhaseSample>,
    pub budget_us: u64,              // e.g. 8_000 fixed, 8_000 render
}

pub struct PhaseSample {
    pub phase: SystemPhase,
    pub start_us: u64,
    pub elapsed_us: u64,
    pub systems: Vec<SystemSample>,  // host-side spans per system
}
```

A ring buffer of recent `ProfileFrame`s feeds the egui flame/graph panel. The
profiler is **explicitly Domain A** and, when enabled, may run on a
`rayon`-independent, lock-free ring so it never adds a contended `Mutex` to a
hot path.

### ECS viewer

A read-only tree: archetypes → entities → component values. It materializes a
snapshot once per frame from the world and lets the developer expand any
component into its typed, fixed-point fields. It is read-only by design — edits
go through gizmos / deltas, never through this panel.

```rust
pub struct EcsSnapshot {
    pub archetypes: Vec<ArchetypeView>,
}

pub struct ArchetypeView {
    pub id: ArchetypeId,
    pub component_ids: Vec<ComponentId>,
    pub entities: Vec<Entity>,
    pub len: usize,
}
```

To render values, the viewer reinterprets each column with `bytemuck::cast_slice`
against the registered `Pod` type's displayer (Domain A knows the layout via the
component registry) — never `transmute`, matching the codebase pattern.

### Console / log panel

Collects host `log` records and Domain B `warnings` (`RecoverableError` carried
out of the sandbox in a `WorldDelta`) into one filterable panel with severity
levels and timestamps. `RecoverableError` is rendered from its stable numeric
`code` plus optional detail string via the codec helpers in `contracts`.

### Render draw-call viewer

Lists the draw calls the renderer submitted for the last frame — pass, pipeline,
mesh, instance count, and (for Domain B) the `RenderKind` that requested it.
Debug gizmos submitted by the editor (see spec `09`) are visible here too, which
is how a developer confirms gizmo calls are separate from gameplay draws.

### Determinism hash-of-state inspector

The engine computes a **deterministic hash** of simulation state at a chosen
boundary (end of `FixedUpdate`) using a stable hash over the canonical column
bytes in archetype order. It is a scalar the developer can copy and compare, and
the same hash computed at the same tick across two runs must match bit-for-bit.
The inspector shows, per recent tick, the hash value and whether it changed.

```rust
/// Stable, order-dependent hash (not a HashMap iteration) so identical world
/// bytes ⇒ identical hash on every platform/run.
pub fn state_hash(world: &World, seed: u64) -> u64 {
    // Iterate archetypes in their canonical (sorted) order and columns in
    // component-id order; feed bytes via a deterministic hasher.
    // Domain A only; the guest never computes this.
}
```

### Error surfacing for RecoverableError

When a pure system returns `Err(RecoverableError)`, the host logs it, records
the offending system + tick, and (for inspection) rolls back that tick's partial
delta. The debugging tools render these as first-class entries in the console
with the stable `code`, the message, and a "jump to source" affordance in the
editor.

## Rendering into egui

Each tool is an egui panel behind a toggle (`Window`). They share a top-level
`Debugger` that owns profiler rings, the ECS snapshot, console buffer and hash
trail, and holds no references across egui frames:

```rust
pub struct Debugger {
    pub profile_ring: RingBuffer<ProfileFrame>,
    pub ecs_snapshot: Option<EcsSnapshot>,
    pub console: ConsoleBuffer,
    pub draw_calls: Vec<DrawCallInfo>,
    pub hash_trail: VecDeque<(u64, u64)>,   // (tick, state_hash)
    pub errors: Vec<SystemErrorRecord>,
}

impl Debugger {
    /// Called from the game loop at snapshot points; cheap, never blocking.
    pub fn capture(&mut self, world: &World, loop: &GameLoop) { /* ... */ }
    pub fn ui(&mut self, ctx: &egui::Context) { /* egui windows */ }
}
```

`Debugger::capture` runs at host safe points (post-flush) so snapshots are
consistent; egui only reads the captured data in `ui`. Nothing in `Debugger`
calls into Domain B or allocates in a Domain B hot path.

## Key Rust types

- `ProfileFrame`, `PhaseSample`, `SystemSample`, `RingBuffer`.
- `EcsSnapshot`, `ArchetypeView`, `EntityView`, component displayers.
- `ConsoleBuffer`, `LogRecord`, severity enum.
- `DrawCallInfo` (pass / pipeline / mesh / instances / `RenderKind`).
- `Debugger`, `SystemErrorRecord`.
- `state_hash(world, seed) -> u64`.

## Constraints

- All tools are Domain A; none instrument or call into Domain B.
- Snapshot-based: never mutate ECS or the renderer from a debug panel.
- The determinism hash is computed over canonical order only — no
  `HashMap`/iteration-order dependence, so it is reproducible.
- Profiling must not add a contended `Mutex` to host hot paths (lock-free ring).
- Panel data is displayed host-side; `f32` appears only for readability of the
  timing charts, never in logic state.
- Portability: no hardcoded paths; compiles on `x86_64-linux` and
  `aarch64-linux`; UI headless-safe in CI (panels skipped without a window).

## Performance

- `Debugger::capture` is O(world size) at most once per frame at a safe point;
  the frame profiler and hash computation dominate and are budgeted (hash of
  columns) to stay ≪ 1 ms.
- Draw-call list is already produced by the renderer; the viewer only copies
  small summaries.
- Profiler ring is bounded (e.g. 600 frames); no unbounded growth.
- Tools are toggled off by default; idle cost ≈ 0.

## Testing strategy

- Unit: `state_hash` stability — same seeded world 3× → identical hash; two
  differing worlds → differing hashes (property test); canonical ordering.
- Unit: profiler ring bounds; console severity filtering; `RecoverableError`
  code rendering.
- Integration (headless): run seeded ticks, capture snapshots/hashes at safe
  points, assert no tool code path touches Domain B and deterministic hashes
  match across runs.
- Smoke: egui panels render behind a window feature; CI uses a headless stub.

## Dependencies

- Domain A only: `egui`, `wgpu`, `log`, `bytemuck`, `contracts`
  (`RecoverableError`, `code` constants, `RenderKind`, ECS types), `crates/ecs`
  (world inspection). No new Domain B or ABI surface is required.

## Next steps

1. `RingBuffer<ProfileFrame>` + host phase timing capture points.
2. ECS snapshot materialization + typed component display.
3. Console/log panel bridging `RecoverableError`.
4. Draw-call summary collection in the renderer.
5. `state_hash` + hash-of-state inspector trail.
