---
spec: "45-navigation"
phase: "Phase 5: Environment & AI"
status: "draft"
author: "OpenEngine AI"
created: "2026-09-03"
depends_on:
  - "00-ecs-architecture"
  - "05-time-system"
  - "13-physics-basics"
  - "16-serialization"
  - "21-primitive-components"
  - "22-edit-vs-play"
  - "42-terrain-system"
  - "43-vegetation-system"
  - "44-behavior-trees"
---
# 45 - Navigation

## Overview

Navigation gives AI agents a way to move from place to place over the world
surface. It provides **navmesh generation** (voxelize the collision/terrain into
a walkable surface, partition into regions), **pathfinding** (A* over the mesh,
with a **hierarchical** layer for long paths), **dynamic obstacles** with
**local updates**, and a **`NavAgent`** that follows a path with its radius /
height / speed. Editor tooling renders the mesh and paths.

Determinism is the load-bearing rule, as everywhere: **same input world (height +
obstacles + agent) ⇒ same generated mesh and same path**. Mesh building,
partitioning, and A* are pure Domain-B operations over fixed-point geometry and
integer cell indices, with no `f32`, no `HashMap` iteration, no wall clock, and
no ambient RNG. Path output is a stable sequence of waypoints an AI system (spec
44 `MoveTo`) consumes.

Components: **`NavMesh` (37), `NavAgent` (38), `NavObstacle` (39)**.

## Core Concepts

### From heightfield + obstacles to a voxel walkable layer

Generation is the classic Recast-style pipeline, but re-expressed as **pure,
fixed-point, `HashMap`-free Domain B**:

1. **Voxelize** the walkable region: over the world's axis-aligned volume above
   the terrain heightfield (spec 42 `height_at`), build a **sparse voxel grid**
   keyed by `(vx, vy, vz)` as a sorted `Vec` of integer cells (never a
   `HashMap`). A cell is *walkable* where the agent's `radius`/`height` (climb /
   step) clears the ground-to-ceiling gap against the terrain and any `NavObstacle`.
2. **Walkable / unwalkable classes** are decided by pure fixed tests: the agent's
   capsule fits, step height ≤ `max_step`, slope of the local surface ≤
   `max_slope`. Obstacles (NavObstacle + foliage with colliders, spec 43) carve
   cells.
3. **Region partition** groups connected walkable voxels into convex regions so
   A* runs on few nodes instead of raw voxels. Region ids are assigned by a
   deterministic flood-fill in sorted-cell order — identical world ⇒ identical
   region ids, so a generated mesh round-trips and hot-reloads reproducibly.

The voxelization origin/extent/cell-size are authored per world
(`NavMesh.voxel_size`, `cell_height`) and are pure integer/fixed inputs.

```rust
/// Voxelize + partition = one pure system producing a NavMesh delta.
pub fn navmesh_build_system(
    view: &StateView<'_>,       // read NavObstacle + terrain patches in view
    config: &NavMeshConfig,
) -> Result<WorldDelta, RecoverableError>
```

### The mesh representation

The `NavMesh` component describes, per world/region, the partition a pathfinder
uses. Two layered graphs:

* **Low-level**: the region graph — nodes are convex walkable regions, edges are
  shared borders between adjacent regions (a pure, sorted construction).
* **High-level (hierarchical)**: a *tile/region graph of regions* for long-range
  planning so a path across a large world is found by first choosing the sequence
  of big tiles, then refining inside each tile only as needed.

Because region ids are stable (above), both layers are deterministic and
serializable.

```rust
pub struct NavMesh {
    pub origin_x: I16F16,
    pub origin_z: I16F16,
    pub world_size: I16F16,
    pub voxel_size: I16F16,
    pub cell_height: I16F16,
    pub max_climb: I16F16,
    pub max_slope: u8,           // degrees/2 (0..45)
    pub tile_bits: u8,           // log2 tile span for the hierarchy
    pub revision: u32,           // bumps when obstacles change the mesh
    // region partition + edge adjacency stored as a host/domain resource table
    // referenced by this component (like terrain's HeightPatch view).
}
```

### Dynamic obstacles & local updates

`NavObstacle` marks a moving or newly placed blocker (e.g. a spawned crate, a
thrown object). Two costs are kept apart:

* **Global** changes (a world chunk regenerates, foliage populates, spec 42/43)
  trigger a **full or tile-region rebuild** of `NavMesh` (`revision` bump) — a
  batched, pure Domain-B pass.
* **Local** changes (an obstacle appears/moves) trigger a **local update**: only
  voxels in the obstacle's bounding box are re-marked and the affected region
  graph edges recomputed. Local updates are idempotent and deterministic and
  never require recomputing far regions.

`NavMesh.revision` lets caches (host tile meshes, editor viz) know when to
refresh. Because all changes are Commanded (edit world) or pure deltas (play),
undoing an obstacle edit also rolls the mesh back deterministically (spec 23).

```rust
pub struct NavObstacle {
    pub radius: I16F16,          // cylinder/box footprint (world units)
    pub height: I16F16,
    pub dynamic: u8,             // 0 = static (bake), 1 = local-update
    pub carve: u8,               // 0/1: actually block, or just add cost
}
```

### A* pathfinding + hierarchy (deterministic)

Given a start/goal (in region space) A* runs over the low-level region graph.
Determinism requires:

* An **open set ordered by a total, platform-independent key**: `(f = g+h as
  fixed, insertion order)` where `h` is an admissible fixed heuristic (octile /
  euclidean-squared promoted to `I32F32` like spec 13). Ties break by a stable
  region-id order, never by a pointer/hash order.
* The closed set and predecessors live in **sorted `Vec`s / BTreeMap-style fixed
  arrays**, never a `HashMap`, so iteration order is a pure function of region ids.
* A* returns the same waypoint list for the same `(mesh, start, goal, agent)` on
  every platform and every run.

Hierarchical mode first finds a coarse path of tiles (high-level A*), then, along
that coarse corridor, refines within each tile. The refinement is a pure function
of the coarse corridor, so it too is deterministic.

```rust
pub fn find_path(
    mesh: &NavMesh,
    start: (I16F16, I16F16),      // world x/z
    goal: (I16F16, I16F16),
    agent: &NavAgentParams,       // radius/height influence corridor width
) -> Result<Option<Vec<Waypoint>>, RecoverableError>;
```

`Waypoint` is `(I16F16, I16F16)` + an optional height from the terrain so an agent
can drive on the surface (feeding spec 44's `MoveTo`).

### NavAgent: follow a path with radius/height/speed

`NavAgent` is the mover. It holds path parameters and, each tick, a Domain-B
pure movement system (`nav_agent_step_system`) advances the agent along its
current path toward the next waypoint:

```rust
pub struct NavAgentParams {       // params, not runtime cursor
    pub radius: I16F16,
    pub height: I16F16,
    pub speed: I16F16,            // fixed units/tick
    pub acceleration: I16F16,
    pub turn_speed: I16F16,       // optional
    pub navmesh: Entity,          // which NavMesh to drive on
}

pub struct NavAgent {
    pub params: NavAgentParams,
    pub destination: Option<Entity>,   // or a waypoint vector
    pub path_revision: u32,            // mesh revision this path was built on
    // runtime cursor (current waypoint index, avoidance state) lives in the
    // blackboard / agent component, not a global store.
}
```

The stepping system is pure: given the agent's current fixed `Transform`, the
path, the mesh, and neighbor agents/obstacles (local), it writes the next
position via a `ColumnWrite`. **Path following never perturbs deterministic
output**: with the same path + speeds + starting state, the agent's per-tick
positions are bit-identical. Avoidance between agents uses a deterministic,
fixed-step local repulsion (integer/fixed arithmetic only) processed in sorted
entity order.

If the underlying `NavMesh.revision` changes under an agent, the agent invalidates
its path and (as a pure system, or an AI task in spec 44) requests a new
`find_path` next tick — no mid-path nondeterminism.

### Editor visualization (Domain A read)

The mesh is displayed as a Domain-A read of the generated region/voxel data and
agent paths are drawn as line lists (spec 24 / render overlay). Visualization
never feeds back into pathfinding; it is presentation of the already-computed
mesh and is testable headlessly on the mesh *data* (not pixels).

## Key Rust Types

```rust
//! crates/logic-nav (Domain B, no_std, pure). No std/threads/RNG/GPU/HashMap.
use openengine_math::I16F16;
use contracts::{StateView, WorldDelta, Entity, RecoverableError};

pub struct NavMeshConfig {
    pub origin_x: I16F16,
    pub origin_z: I16F16,
    pub world_size: I16F16,
    pub voxel_size: I16F16,
    pub cell_height: I16F16,
    pub max_climb: I16F16,
    pub max_slope: u8,
    pub tile_bits: u8,
}

pub fn navmesh_build_system(/* */) -> Result<WorldDelta, RecoverableError>;
pub fn navmesh_local_update_system(view: &StateView<'_>, region: BBox)
    -> Result<WorldDelta, RecoverableError>;
pub fn find_path(mesh: &NavMesh, start: (I16F16,I16F16), goal: (I16F16,I16F16),
                 agent: &NavAgentParams) -> Result<Option<Vec<Waypoint>>, RecoverableError>;
pub fn nav_agent_step_system(view: &StateView<'_>)
    -> Result<WorldDelta, RecoverableError>;

/// Fixed integer hash used to key voxel cells in a sorted table (no HashMap).
pub fn voxel_key(cx: i32, cy: i32, cz: i32, dims: (u32, u32)) -> u64;
```

```rust
//! Domain A — crates/nav-data: generated mesh store (serializable), editor viz.
pub struct NavMeshStore { /* sorted regions/edges, read by find_path host tools */ }
```

## Components

| `ComponentId` | Name           | Domain use (owner)                                   |
|---------------|----------------|------------------------------------------------------|
| **37**        | `NavMesh`      | Generated walkable partition + revision for a world. |
| **38**        | `NavAgent`     | Path-following mover (radius/height/speed params).   |
| **39**        | `NavObstacle`  | Static/dynamic blocker carving the walkable layer.   |

IDs 37–39 are **frozen** here. Terrain/Foliage (30–33) are specs 42/43's;
BehaviorTree/Blackboard/AIAgent (34/35/36) is spec 44's. Future navigation
components (e.g. `NavOffMeshLink`) append in the 40–49 reserved window of this
phase; never reuse 30–39.

```rust
/// NavMesh — generated partition handle + revision. ComponentId 37.
#[repr(C)]
#[derive(Clone, Copy, bytemuck::Pod, bytemuck::Zeroable,
         serde::Serialize, serde::Deserialize, Debug)]
pub struct NavMesh {
    pub origin_x: I16F16,
    pub origin_z: I16F16,
    pub world_size: I16F16,
    pub voxel_size: I16F16,
    pub cell_height: I16F16,
    pub max_climb: I16F16,
    pub max_slope: u8,
    pub tile_bits: u8,
    pub revision: u32,            // bump on (re)build / local update
}

/// NavObstacle — a blocker. ComponentId 39.
#[repr(C)]
#[derive(Clone, Copy, bytemuck::Pod, bytemuck::Zeroable,
         serde::Serialize, serde::Deserialize, Debug)]
pub struct NavObstacle {
    pub radius: I16F16,
    pub height: I16F16,
    pub dynamic: u8,
    pub carve: u8,
    pub _pad: [u8; 2],
}

/// NavAgent — the path follower params. ComponentId 38.
/// Runtime cursor (current waypoint) is blackboard/agent state, not this Pod.
#[repr(C)]
#[derive(Clone, Copy, bytemuck::Pod, bytemuck::Zeroable,
         serde::Serialize, serde::Deserialize, Debug)]
pub struct NavAgent {
    pub radius: I16F16,
    pub height: I16F16,
    pub speed: I16F16,
    pub acceleration: I16F16,
    pub turn_speed: I16F16,
    pub navmesh: Entity,          // which NavMesh (37) drives this agent
    pub enabled: u8,              // 0/1
    pub _pad: [u8; 3],
}
```

`NavMesh` is typically a single world-level entity; many `NavAgent`s and
`NavObstacle`s reference it. The dense region/voxel store is hosted Domain-A
resource data (serializable, spec 16), surfaced read-only to Domain B as a
`NavMeshView` — mirroring spec 42's `HeightPatch` pattern so columns stay small.

## Constraints

- **Deterministic generation & pathing.** Voxelization, region partition, and
  A* are pure Domain-B operations over fixed-point geometry + integer cell
  keys. No `f32`, no `HashMap` iteration (sorted `Vec`/fixed arrays + stable
  insertion keys), no wall clock, no ambient RNG, no threading in Domain B. Same
  input world ⇒ same mesh ⇒ same path on every platform and every run.
- **Same ground truth as terrain/foliage.** The walkable layer derives from spec
  42 `height_at` and spec 43 obstacle/collider geometry, so a world's nav and its
  rendering/placement never disagree about where the ground is.
- **Mesh & path changes flow through deltas.** Builds/local-updates return a
  `WorldDelta` the host applies; in the edit world they are spec-23 `Command`s so
  an obstacle edit is undoable and rolls the mesh back deterministically.
- **No global nav state in Domain B.** Region/voxel data is read-only view;
  agents carry their own params + cursor (in blackboard/agent component), never a
  shared mutable store.
- **Revision-invalidation.** An agent's cached path is only valid for the
  `revision` it was built on; a mesh rebuild/update under it forces a pure
  re-path next tick — never a mid-path nondeterministic patch.
- **Agent movement is deterministic.** `nav_agent_step_system` advances fixed
  positions each tick from path + local state in sorted entity order; avoidance is
  fixed-step, integer/fixed arithmetic.
- **Pod + serde + fixed-point components.** 37/38/39 are `#[repr(C)] Pod +
  Zeroable + serde`, fixed-point fields only, serializable (spec 16) for
  save/replay/netcode. No absolute paths.
- **Edit vs Play.** NavMesh/agents/obstacles authored in edit world deep-clone to
  play deterministically (spec 22); play runs nav + re-build systems only.
- **Portability / headless.** Compiles `x86_64-linux` + `aarch64-linux`; no GPU
  in logic tests; editor viz reads mesh *data*, never feeds back.

## Performance Targets

- Full navmesh build of a 512×512 m world (voxel size ~0.5 m): **< 200 ms**
  (editor-time or background rebuild), Domain B pure.
- Local obstacle update over a small bounding box: **< 2 ms**.
- A* region-graph path for a typical short path: **< 0.2 ms**; hierarchical long
  path across a large world: **< 1 ms**.
- `nav_agent_step_system` for **1000 agents** (path following + local avoidance):
  **< 3 ms/tick**.
- Editor viz of the mesh/path overlays: reads the generated store; **< 1 ms**
  line-list build per viewport.

## Testing Strategy

All headless (no GPU) in `crates/logic-nav` + editor tests:
- **Mesh build determinism:** build the same voxelized world (terrain patch from
  spec 42 + obstacle set from spec 43) 3× on two targets; assert the resulting
  region partition, adjacency, and `revision` are **bit-identical**.
- **Partition correctness:** connected walkable cells map to the same region id;
  obstacles split regions; step/slope filters reject unwalkable cells exactly.
- **Path determinism:** run `find_path` over the same mesh 3× and assert the
  same waypoint list (fixed waypoints, stable tie-break) every run/platform.
- **Path validity:** for a golden set of start/goal pairs, assert the returned
  path stays on walkable regions, has the expected length, and reaches the goal
  within tolerance.
- **Hierarchical vs low-level parity:** long-range hierarchical path end-to-end
  equals the low-level A* result on the same mesh (cost within a fixed ratio,
  same endpoint region).
- **Local update:** add/move a `NavObstacle`, run `navmesh_local_update_system`,
  assert only the affected bounding region's voxels/edges changed and `revision`
  bumped; a subsequent `find_path` around it detours deterministically.
- **Agent step determinism:** identical path + params ⇒ per-tick positions
  bit-identical across 3 runs; speed/acceleration respected.
- **Undo (edit world):** commit an obstacle add command (spec 23), then undo;
  assert the nav mesh store is bit-identical to pre-edit and `revision` rolled
  back.
- **Edit-vs-play:** authored mesh/obstacles deep-clone into play; play rebuilds
  deterministically and editor commands never touch the play twin (spec 22).
- **Serialization:** NavMesh/NavAgent/NavObstacle + the dense mesh store
  round-trip the spec-16 codec bit-identically.
- **AI integration (spec 44):** a `MoveTo` task consumes `find_path` and drives a
  `NavAgent`; end-to-end determinism over a scripted scene.
- **Purity:** `verify-wasm-purity` reports `[PURE]` for `crates/logic-nav`.

## Dependencies

- `contracts` (`StateView`, `WorldDelta`, `ColumnWrite`, `Entity`,
  `ComponentId`, `RecoverableError`), `bytemuck`, `serde`, `postcard`, `alloc`.
- `openengine-math` (`I16F16`, `I32F32`, integer voxel hash, fixed A* helpers).
- Spec 42 (`height_at`, terrain patches) + spec 43 (obstacle colliders) + spec
  21 (`Transform`, `Entity`) + spec 00 (system shape) + spec 13 (fixed AABB /
  ray overlap style + squared-distance widening) + spec 05 (tick) + spec 16
  (codec) + spec 22 (edit/play) + spec 23 (undoable obstacle commands) + spec
  24 (viewport viz) + **spec 44** (`MoveTo` task consumes paths).
- Domain A: `crates/nav-data`, `crates/editor` (viz), `crates/logic-sandbox`.

## Next Steps

1. Register `NavMesh` (37), `NavAgent` (38), `NavObstacle` (39) components.
2. Implement pure voxelization over spec-42 heightfield + obstacle carving.
3. Implement deterministic region partition (sorted flood-fill).
4. Implement region-graph A* with stable tie-break; add the hierarchical layer.
5. Implement `navmesh_local_update_system` + `revision` invalidation.
6. Implement `nav_agent_step_system` (fixed path-following + local avoidance).
7. Wire spec-44 `MoveTo`; add undoable obstacle commands (spec 23) and editor viz
   (spec 24); land the full determinism/serialization/purity test battery.
