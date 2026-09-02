---
spec: "16-serialization"
phase: "Phase 5"
status: "design"
---

# Serialization: Save / Load Worlds & Scenes

## Overview

Persistence in OpenEngine has two jobs that must stay consistent with one
another and with determinism:

1. **World snapshots** — save the *current live simulation* (all SoA columns of
   every archetype) to a stable, versioned binary form so the world can be
   resumed mid-game, and so rollback netcode (`15-networking.md`) can rewind to
   an earlier tick. Saving the world *at tick N together with the ordered replay
   inputs* lets a load deterministically re-simulate forward from a snapshot.
2. **Scene files** — save the *authoring-time* content (what an editor places in
   a level) as an asset, independent of any in-progress simulation.

Both serialize **SoA columns** into a compact, versioned, `postcard`-encoded
binary format that never depends on pointer addresses, `HashMap` iteration
order, or host memory layout. Every snapshot is tagged with `ARCH_VERSION`
(see `contracts`), so a save written by one ABI revision is either rejected or
migrated by a later one — never silently misread.

## Design

### Two formats, one core principle

The shared principle: **serialize plain `Pod` column bytes in a deterministic,
schema-versioned envelope**, plus enough *schema metadata* to know how to
interpret and migrate them. Because components are `#[repr(C)] Pod + Zeroable`,
a column is just a run of bytes whose meaning is fixed by a registered
`ComponentId` + element size; we serialize the bytes and the descriptor, not
field-by-field objects.

### Envelope + header

Every persisted blob starts with a fixed header carrying the ABI/format version,
so readers can reject or migrate:

```rust
/// Common header for all OpenEngine binary persistence.
#[repr(C)]
#[derive(Clone, Debug, serde::Serialize, serde::Deserialize)]
pub struct FormatHeader {
    pub magic: [u8; 8],        // b"OESNAP\0\0" or b"OESCENE" etc.
    pub abi_version: u32,      // == ARCH_VERSION at write time
    pub format_version: u32,   // bump for this file-family layout changes
    pub endian: u16,           // 0x1E1E marker: asserts little-endian
}
```

`postcard` is little-endian and does not float-round, so it is already
deterministic across platforms. The explicit `abi_version` lets the loader gate
migration before it touches any bytes.

### WorldSnapshot (for save/load + rollback)

A snapshot captures the full world at a tick: every archetype and its live SoA
columns. It is deliberately **ordered** (archetypes sorted by id, columns sorted
by component id, entities in slot order) so that two saves of the same logical
world are byte-identical and hashable for desync checks.

```rust
/// Deterministic, versioned capture of the world at one tick.
#[derive(Clone, Debug, serde::Serialize, serde::Deserialize)]
pub struct WorldSnapshot {
    pub header: FormatHeader,
    pub tick: u64,                        // the sim tick this was captured at
    pub sim_time: I16F16,                 // deterministic, fixed
    pub archetypes: Vec<ArchetypeSnapshot>,
}

pub struct ArchetypeSnapshot {
    pub archetype: ArchetypeId,
    pub entities: Vec<Entity>,            // slot order — not a HashMap
    pub columns: Vec<ColumnSnapshot>,     // sorted by ComponentId
}

pub struct ColumnSnapshot {
    pub component: ComponentId,
    pub element_size: u32,                // validation vs registered size
    pub data: Vec<u8>,                    // len * element_size, raw Pod bytes
}
```

Because entities store `(generation, index)` and indices are relative to slot
order in `entities`, serialization never records memory addresses — only stable
generation-guarded handles and their per-column data. This is what makes a
snapshot loadable on any platform and re-hashable for `15-networking`'s desync
detection.

### Saving the current world at tick N + replay inputs

To make a save *deterministically resumable*, we store the snapshot at tick `N`
**plus the replay input log** starting at `N` (the `InputCommand`s from
`15-networking.md`, already the sole non-deterministic input). On load, the world
restores to the `N` snapshot and re-simulates forward with those recorded inputs,
reproducing exactly the state the game reached. This is the same replay mechanism
used to test rollback.

```rust
/// A complete resumable save.
#[derive(Clone, Debug, serde::Serialize, serde::Deserialize)]
pub struct SaveFile {
    pub header: FormatHeader,
    pub snapshot: WorldSnapshot,          // world at tick N
    pub replay_from: u32,                 // == snapshot.tick
    pub replay_inputs: Vec<InputBatch>,   // ordered inputs N..present
    pub metadata: SaveMeta,               // title, wall-clock for UI only
}
```

`metadata.wall_clock` is presentation-only and excluded from any determinism
checksum.

### Loading & deterministic re-simulation

Load is a pure, order-independent rebuild then an ordinary re-sim:

1. Validate `header.abi_version` vs `ARCH_VERSION`; run migration if behind.
2. Validate each column's `element_size` equals the currently registered
   component's size; reject (or migrate) on mismatch.
3. Rebuild archetypes by spawning entities in slot order and writing each column
   back with `cast_slice` — order is fixed, so rebuild is deterministic.
4. Re-run the fixed-timestep pure systems over `replay_inputs` exactly as the
   live loop does, applying each delta — this is the identical path used by
   rollback in `15-networking.md`.

### Scene files (see spec 06)

Scenes are the authoring-time counterpart: they store what an editor placed,
*not* a live simulation. A scene references archetypes/component ids the same way
but carries no tick and no run state; `WorldSnapshot` capture is how an
in-progress world is persisted, whereas a scene is a template that spawns a fresh
world. Both share the `ColumnSnapshot`/envelope machinery so the editor can
round-trip authored entities into a world and back. (Scene layout is detailed by
the dedicated scene spec; here we only fix that scenes share the versioned
`postcard` column encoding and never embed pointers/host addresses.)

### Editor save/load

The editor (`crates/editor`, Domain A, `egui`) drives the same serialization
path. Save = capture current world (or the edited scene) → `postcard` → disk
under `$OPENENGINE_ASSETS_PATH`/`$OPENENGINE_CONFIG_PATH` (portable env-based
paths, never hardcoded home dirs — see Portability). Load = validate version →
rebuild → (optionally) resume. Because both runtime and editor serialize the
same format, an agent can open an editor scene and a runtime can boot it with one
code path.

### Schema versioning & migration

- Bump `ARCH_VERSION` in `contracts` on any breaking layout/enum change; the
  loader refuses snapshots from a newer ABI.
- `format_version` in `FormatHeader` distinguishes the persistence envelope
  (which may add/remove wrapper fields) from the ABI wall (component layouts).
- A `migration: Vec<(from, to, fn)>` table, keyed by `abi_version`, upgrades old
  component bytes **field-versioned** (e.g. a component records
  `layout_version`), so an older column is converted to the current layout
  rather than misread. Any component without a forward-migration path is
  rejected loudly — never guessed.

## Key Rust / types

- `FormatHeader`, `WorldSnapshot`, `ArchetypeSnapshot`, `ColumnSnapshot`,
  `SaveFile`, `SaveMeta` — all `serde` value types using `postcard`.
- `ComponentRegistry` maps `ComponentId -> element_size`, used for validation.
- Host `save_to_disk`/`load_from_disk` in Domain A only (files, I/O); Domain B
  never serializes to disk — it may only *produce a snapshot-shaped value* that
  the host persists.

## Constraints

- **No pointers/addresses, no `HashMap` iteration order**: all structures are
  sorted `Vec`s or slot-order `Vec`s; determinism of bytes is a hard requirement
  (needed for `WorldHash` desync checks in `15-networking`).
- Postcard only (little-endian, deterministic); no JSON for on-disk world/save
  data.
- Disk writes/reads are Domain A only; files live under env-configured paths
  (`OPENENGINE_*`), relative or `$HOME`, never a hardcoded path.
- Portable across `x86_64-linux`/`aarch64-linux`, Docker/CI/offline.
- Loading a snapshot at tick N + replay inputs reproduces the original state
  deterministically (re-sim is the same path rollback uses).

## Performance

- Snapshot of a typical world: serialize only live archetype columns; target
  < 1 MB save for ~10k entities and < 1 ms capture for a rollback-sized subset
  (using the mutate-only-columns optimisation from `15-networking`).
- Load + re-sim matches live-tick cost; bulk column restore via `cast_slice`
  copy is fast and allocation-light.
- No allocations in the hot encode path for a single rollback snapshot beyond the
  output buffer.

## Testing strategy

- Round-trip: save a seeded world → load → assert bit-identical `WorldHash` to
  the pre-save world and identical columns.
- Determinism: serialize the same world 3× on two platforms and in Wasm; assert
  identical bytes.
- Re-sim resume: save world at tick N + replay inputs; load and re-simulate to
  `M > N`; assert state equals a fresh run from tick 0 to `M` (the key rollback +
  replay guarantee).
- Migration: write a snapshot under an older `abi_version`, bump `ARCH_VERSION`,
  run the migration table, assert correct upgraded bytes.
- Editor: open a scene, edit, save, re-open; assert the entity set and column
  data round-trip exactly.
- Hostile/foreign input: malformed or wrong-`abi_version` bytes are rejected, not
  misread (fuzz with truncated/corrupt buffers).

## Dependencies

- `postcard`, `serde`, `bytemuck` (`cast_slice`), `contracts` (`Entity`,
  `ComponentId`, `ArchetypeId`, `ColumnDescriptor`, `ARCH_VERSION`),
  `openengine-math` (`I16F16`).
- Host (Domain A): `std::fs` for disk I/O under `OPENENGINE_*` paths. Domain B
  unchanged; it never writes files.

## Next steps

1. Define `FormatHeader`/`ColumnSnapshot`/`WorldSnapshot` and validate against
   `ComponentRegistry`.
2. Implement `save_world`/`load_world` in Domain A with version gating.
3. Add `SaveFile` = snapshot@N + `replay_inputs`; wire re-sim resume.
4. Add the migration table + `abi_version` gating and tests.
5. Add the editor scene save/load (scene format per spec 06).
6. Confirm round-trip determinism and cross-platform byte-identity in CI.
