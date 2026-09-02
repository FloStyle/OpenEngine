---
spec: "06-scene-management"
phase: "Phase 5"
status: "design"
---

# Scene Management

## Overview

A **scene** is a self-contained playable unit: one [`World`] (its archetypes,
SoA columns, live entities and their components) **plus** the list of systems
that drive it **plus** an ordered asset manifest. Scenes are how OpenEngine
dices a game into discrete, loadable, replaceable states — a menu scene, a
gameplay scene, an editor scene. Because the ECS host storage lives in Domain A
and pure logic in Domain B, a scene is a *host-side construct*: Domain A owns
scene I/O, lifecycle, and transitions; Domain B only ever sees the current
world through a [`StateView`] and is oblivious that a scene boundary exists.

This spec defines the `SceneHandle`, the load/unload protocol, the serialized
scene format (which this repo's save/load system — spec `16-save-load` — will
reuse), the transition policy, deterministic spawn order, the editor's notion
of a "current scene", and dev-time hot reload. It is deliberately independent
of renderer and input; a scene is a pure data + system bundle.

## Design

### What a scene contains

```rust
pub struct Scene {
    /// Stable identity used by handles and the editor.
    pub id: SceneId,
    /// Human/agent-facing name shown in the editor hierarchy (spec 08).
    pub name: String,
    /// The world storage: archetypes + entities + components (Domain A ECS).
    pub world: World,
    /// Ordered pure systems to run every fixed tick for this scene.
    pub systems: Vec<SceneSystem>,
    /// Systems that run on the host side (editor, UI, camera drives) — Domain A.
    pub host_systems: Vec<HostSystem>,
    /// Ordered asset paths this scene references (see asset-manifest section).
    pub assets: Vec<AssetRef>,
    /// Editor-only metadata (spawn order, tags) — stripped at shipping.
    pub meta: Option<SceneMeta>,
}
```

Domain B never owns or sees a `Scene`. The `SceneSystem` entries are
`PureSystem` function pointers / wasm exports driven through the sandbox
(spec `01`, `contracts::PureSystem`). Host systems are the Domain-A editor /
driving systems (AGENTS.md pillar 4).

### SceneHandle and the registry

A `SceneHandle` is a generation-guarded token into a host registry of loaded
scenes (mirroring `Entity`'s ABA protection):

```rust
pub struct SceneHandle {
    pub generation: u32,
    pub index: u32,          // slot in the SceneRegistry
}
```

The registry owns all loaded scenes and tracks which is active:

```rust
pub struct SceneRegistry {
    scenes: Vec<SceneEntry>,          // entry = (scene, loaded state)
    active: Option<SceneHandle>,
    pending_transition: Option<SceneTransition>,
    current: SceneId,                 // editor's notion of "current scene"
}

enum LoadState { Loading, Ready, Unloading }
```

`SceneRegistry` lives in `crates/core` (Domain A) beside the `GameLoop`/`World`
owner from spec `01`. It is the single authority over which world is being
simulated and ticks.

### Load / unload protocol

Loading a scene is a *materialization* into ECS storage, not a process spawn.
The world it replaces must be swapped deterministically at a tick boundary:

```rust
impl SceneRegistry {
    /// Queue a transition; applied at the next flush boundary.
    pub fn request_transition(&mut self, target: SceneId);

    /// Load from bytes produced by the scene codec (or spec 16 save format).
    pub fn load_scene(&mut self, bytes: &[u8]) -> Result<SceneHandle, SceneError>;
    pub fn unload(&mut self, handle: SceneHandle);
}
```

Because Domain B systems may have a pending `WorldDelta` when a transition is
requested, transitions are **deferred to the flush boundary** (after all
systems ran and deltas were applied, matching spec `01`'s `world.flush()`).
This keeps a scene swap from interleaving with mid-tick writes.

### Serialized scene format

The on-disk scene format is the **save/load format** — a scene is a save of a
world plus its system list. It is *not* an ad-hoc format; it reuses the same
codec discipline as the ABI:

* Component column bytes serialized with `postcard` (deterministic,
  `#![no_std]`-friendly) rather than JSON (spec: postcard everywhere for
  reproducible bytes).
* A header records `ARCH_VERSION`, scene `id`/`name`, the ordered component
  registry snapshot (so archetypes resolve), and a `spawn_order` integer for
  each entity group.
* Assets are referenced by logical id/relative path, never absolute paths
  (AGENTS.md § 5).

```rust
pub struct SceneFileHeader {
    pub magic: [u8; 8],            // b"OESCENE\0"
    pub arch_version: u32,          // must match contracts::ARCH_VERSION
    pub scene: SceneId,
    pub name: String,
    pub components: Vec<ComponentSchema>,  // registry snapshot, ordered
    pub spawn_order_seed: u64,      // optional determinism seed
}

pub struct SceneAsset { pub kind: AssetKind, pub relative_path: String }
pub enum AssetKind { Mesh, Texture, LogicWasm, Audio }
```

Full save/load semantics (checkpoints, incremental deltas, rollback) are
specified in **spec `16-save-load`**. Scene serialization is the narrow case:
the whole world, one shot, for the transition/cold-start path. Scene loading
calls into the same codec spec 16 defines so there is exactly one world
codec.

### Transition policy

Transitions follow a strict three-phase order that keeps determinism:

1. **Load next** — materialize the target scene into the registry *without*
   activating it. Its world is built from the serialized scene and registered
   under a fresh `SceneHandle`. Spawn order is deterministic (below).
2. **Unload old** — drop the previously active scene's world (despawn all its
   live entities, release its archetype storage and host systems) at a flush
   boundary. Deferred `WorldDelta` writes from the old scene are discarded, not
   applied to the new world.
3. **Blend** — optional cross-fade for *visual* presentation only (camera
   dissolve, audio duck) driven by Domain-A host systems. Gameplay state never
   blends across scenes; `SceneState` switches atomically.

```rust
pub enum SceneTransition {
    Swap,                 // immediate: load new, unload old (default)
    Blend { fade: I16F16 } // host-only visual cross-fade; state still atomic
}
```

Because the *world* switches atomically at a flush boundary while any *visual*
blend is a separate host concern, two machines running the same transition see
identical world state regardless of their frame pacing.

### Deterministic spawn order

Entities inside a serialized scene are ordered so that spawning is
reproducible. The scene stores entities grouped by archetype in a fixed
(`BTreeMap`/sorted) key order, and within an archetype in row order. Spawn
does not depend on `HashMap` iteration (AGENTS.md § 3). A `spawn_order`
counter increments in this canonical order and, when present, a saved
`spawn_order_seed` lets determinism tests reproduce the exact entity sequence.
`Entity` generation counters restart deterministically on load: the codec
preserves the last generation per slot so re-loading yields identical handles.

### Editor "current scene"

The editor (spec 07/08) operates on exactly one world at a time: the
registry's `current` scene. The editor does not need special access — it reads
the current scene's `World` through the same safe SoA view every system uses
and writes back through `WorldDelta`/`ColumnWrite` (spec `01`). "Current
scene" is just which `World` the `GameLoop` is simulating this tick. The
editor panel can pick among loaded scenes to make one current; switching is a
`request_transition`.

### Hot-reloading a scene (dev only)

In a debug/dev build (`cfg(debug_assertions)` or an explicit
`OPENENGINE_EDITOR` flag), the scene file is watched for changes. On change:

1. The new scene bytes are loaded into a *staging* handle (load next).
2. At the next flush boundary the active world is swapped (unload old) — the
   exact same transition path as a runtime scene change, so dev reloads
   exercise production code.
3. The editor "current scene" follows the swap.

Hot reload is **never** a live patch of the running world; it is a full
transition to a freshly loaded scene. This keeps determinism (no partial
mutation) and keeps the reload path identical to a normal transition. In
release builds the file-watcher is compiled out entirely.

## Key Rust / types

- `Scene`, `SceneId`, `SceneHandle`, `SceneRegistry` — `crates/core` (Domain A).
- `SceneError` — error type for corrupt/version-mismatched scene files
  (mirrors `RecoverableError` semantics on the host side: recoverable, logged).
- `SceneFileHeader`, `SceneAsset` — the codec types shared with spec `16`.
- `PureSystem` / `WorldDelta` / `StateView` — from `contracts`.
- `openengine-math::I16F16` — used by `SceneTransition::Blend` and any
  fixed scene timing; never `f32` in gameplay-visible data.

## Constraints

- Domain A owns all scene I/O, codec, registry, and transition state. Domain B
  is scene-agnostic (pure functions over `StateView` only).
- Scene files contain no absolute/hardcoded paths — only `AssetRef`
  `relative_path` resolved against `OPENENGINE_ASSETS_PATH` (AGENTS.md § 5).
- Loading/transitions happen only at flush boundaries; never mid-system.
- No `HashMap` ordering in spawn or codec; use sorted/`BTreeMap` ordering.
- Compiles on `x86_64-linux` and `aarch64-linux`; scene load is CPU-only (no
  GPU requirement for logic tests).
- Serialized format is the save format (spec `16`); no second world codec.
- `ARCH_VERSION` mismatch between a scene file and the host build is a hard
  load error (scene files are version-locked like logic modules).

## Performance targets

- Cold scene load (decode + materialize): target < 50 ms for a 10 000-entity
  scene; dominated by decode + SoA allocation.
- Transition flush overhead: negligible vs one tick (sub-millisecond).
- Per-frame registry overhead when idle: none (no work when no transition is
  pending).

## Testing strategy

- **Unit:** codec round-trip — encode a `World`, decode, assert identical
  archetypes, entities, generations, and component bytes.
- **Determinism:** load the same scene three times and assert bit-identical
  worlds and identical `Entity` handles (spawn order stable).
- **Transition:** request a swap mid-tick, assert the old scene's pending
  delta is not applied to the new world and the swap lands at the flush
  boundary.
- **Corruption / version skew:** feed a bad magic and a mismatched
  `ARCH_VERSION`; assert a `SceneError`, never a panic.
- **Integration:** scripted A→B→A scene cycle runs 1 000 ticks; assert a
  reproducible final state across 3 runs.

## Dependencies

- `contracts` (`ARCH_VERSION`, `Entity`, `ComponentId`, `ArchetypeId`,
  `StateView`, `WorldDelta`), `postcard` (codec), `crates/ecs` (`World`),
  `crates/core` (`GameLoop`). Reuses the save/load codec from spec `16`.
- Dev-only file watching: a small Domain-A dependency (e.g. `notify`), guarded
  to debug builds; never present in release or shipped logic.

## Next steps

1. Define `Scene`/`SceneId`/`SceneRegistry` in `crates/core`.
2. Specify the shared world codec with spec `16-save-load`; implement
   `SceneFileHeader` + postcard encode/decode.
3. Implement load/unload at flush boundaries in `GameLoop`.
4. Implement deterministic spawn order + generation-preserving load.
5. Wire editor "current scene" selection and the debug-only hot-reload path.
