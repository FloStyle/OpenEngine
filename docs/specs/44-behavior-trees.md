---
spec: "44-behavior-trees"
phase: "Phase 5: Environment & AI"
status: "draft"
author: "OpenEngine AI"
created: "2026-09-03"
depends_on:
  - "00-ecs-architecture"
  - "05-time-system"
  - "16-serialization"
  - "21-primitive-components"
  - "22-edit-vs-play"
  - "24-editor-viewport"
  - "45-navigation"
---
# 44 - Behavior Trees

## Overview

Behavior trees are OpenEngine's AI "brain" representation: a tree of **nodes**
(`selector` / `sequence` / `decorator` / `task`) evaluated per tick, using a
**blackboard** of key–value memory, so an **`AIAgent`** decides its next action
from current world state.

The defining property, as everywhere in this repo, is **determinism**:
*The same tree + same blackboard + same tick ⇒ the same behavior output.* Tick
evaluation is a pure function of a read-only world snapshot; there is no hidden
mutable AI runtime state, no per-node heap captured at runtime, and no
wall-clock. Nodes are *static, serialized structure*; all dynamic values live in
the **blackboard**, which is stored as ECS/data — never a global mutable
structure.

AI *tasks* are not inline closures that poke the world. Each leaf task is a
**Domain-B pure system** of the canonical shape
`fn(&StateView) -> Result<WorldDelta, RecoverableError>`. A tick walks the tree
to select *which* task to run and returns that task's delta (or
success/failure/running); the host applies the returned `WorldDelta` atomically.
This keeps AI inside the AGENTS.md "guest produces a delta, host applies it"
rule and makes the whole brain testable headless.

Debug visualization (highlighting the active node each tick) is a **Domain-A
read** of a per-tick trace — it never influences the decision. Components in this
spec: **`BehaviorTree` (34), `Blackboard` (35), `AIAgent` (36)**.

## Core Concepts

### Tree = serialized static structure; values = blackboard

A behavior tree is **pure structure** — a `#[repr(C)] Pod` forest of node
descriptors plus a root handle. It contains *no* runtime memory and *no*
pointers into ECS. What varies between agents or ticks lives in the blackboard,
so the same tree object can be shared (referenced) by many `AIAgent`s.

```rust
/// One node in a serialized tree. Node "inputs" (which child/key) are indices,
/// never pointers. `NodeKind` carries no payload state.
#[repr(C)]
pub struct BehaviorNode {
    pub kind: NodeKind,        // Selector|Sequence|Decorator|Task  (1 byte)
    pub decorator: u8,         // for Decorator kind: Inverter|Repeater|... 0=none
    pub task: TaskId,          // u16 handle into the registered task table
    pub first_child: u16,      // index into node table; u16::MAX = none
    pub child_count: u16,
    pub blackboard_key: u16,   // where to read/write this node's value
}
```

`NodeKind` is a discriminant-only enum (`#[repr(u8)]`) so it stays `Pod`:

```rust
#[repr(u8)]
pub enum NodeKind { Selector = 0, Sequence = 1, Decorator = 2, Task = 3 }
```

`TaskId` is a **stable, registered** handle into the Domain-B task table, not a
raw function address — so the tree is serializable and survives hot-reload (spec
12) while still resolving to a pure function.

### Tick semantics (deterministic)

Behavior trees are evaluated on the engine's fixed tick (spec 01/05). The whole
evaluation is one **pure system**, `ai_tick_system`, which:

1. Reads, per `AIAgent`, its `tree_id`, `blackboard`, and the world read-only
   (`StateView`).
2. Walks the tree from the root with a deterministic depth-first evaluation,
   *using an explicit stack in a sorted Vec*, never a `HashMap`, never recursion
   that could blow the guest stack unpredictably.
3. Composite nodes short-circuit by fixed rules:
   - **Selector**: try children left→right; returns success at the first child
     that succeeds; returns running if any child returns running (keeps its
     position in a blackboard `running_child` cell); else failure.
   - **Sequence**: try children left→right; returns failure at the first child
     that fails; returns running if any child returns running; else success.
   - **Decorator**: applies its transform to the single child's result
     (Inverter, Repeater N, Retry N, Succeed/Fail-always, Cooldown as a
     blackboard-stamped countdown — *never* a wall-clock timer).
   - **Task**: invokes the registered pure task (below) and returns its status.
4. For the agent whose selected leaf is a **running** task, the `WorldDelta` the
   task produced is returned for the host to apply; tasks whose preconditions are
   not met (the branch was not entered) are **not** invoked, so they cannot emit
   spurious writes.
5. It writes a per-agent **tick trace** into a Domain-A-readable buffer (active
   node path + each node's status this tick) *without* feeding that trace back
   into the decision — pure instrumentation.

Because every step is a function of (tree, blackboard, world state, tick), two
identical runs produce identical node-visits and identical `WorldDelta`s.

### Statuses: Success / Failure / Running

```rust
#[repr(u8)]
pub enum NodeStatus { Failure = 0, Success = 1, Running = 2 }
```

`Running` is the mechanism for multi-tick behaviors (a long task yields). Its
continuation point (`running_child`) is stored in the blackboard so the next tick
resumes deterministically — there is no stack of *live closures* persisting
between ticks. On the tick the task completes, the running cell is cleared.

### Blackboard = ECS/data key–value memory (never global)

The blackboard is an **owned per-agent blob of typed cells**, stored in the
`Blackboard` component (not a `static`/global). It answers "what does this agent
know?" The design keeps it `Pod`/serializable:

```rust
#[repr(C)]
pub struct Blackboard {
    pub capacity: u16,          // number of typed slots (fixed max)
    pub len: u16,               // live slots
    // value payload bytes + a parallel key/id column; see representation note
    // in Constraints for how Pod cell layout is kept.
}
```

In practice a fixed-capacity, typed cell array with a compact **slot table**
(slot → {key hash as `u32`, value bytes}) mirrors spec 21's fixed-inline-array
discipline: no `Vec`/`String` in the `Pod` component proper; the host may back a
larger blackboard in a world resource keyed by entity, still surfaced to Domain B
as a read-only view. Keys are compared by a `u32` hash of a stable key name, and
collisions are rejected at tree-compile time (registering a blackboard key
returns an error on hash clash) so lookups never rely on ambiguous hashing.

Blackboard value types are the usual scalars + small buffers: `I16F16`,
`I32F32`, `u32`/`i32`/`u64`, `bool`, `Entity`, fixed `[I16F16; 3]` position, and
a small `Vec` of targets when needed. Because gameplay math must stay fixed-point
(AGENTS.md § 3), the blackboard stores `I16F16`, never `f32`.

### Tasks as Domain-B pure systems

A **task** is a registered pure function of the form already used across the
engine (`PureSystem` in spec 00), plus a typed status return. The tree selects
which task runs; the task reads the blackboard + world view, decides, and returns
a small delta that *both* updates the blackboard (via a `ColumnWrite`) *and*
emits gameplay writes (movement, spawning, `DeferredCommand`s) — or a status
only.

```rust
/// The registered AI task signature. Returns the node status it completes with
/// and (optionally) a delta to apply. `Err` aborts the tick recoverably.
type AiTask = fn(
    view: &StateView<'_>,
    bboard: &BlackboardView<'_>,
    tick: u64,
) -> Result<(NodeStatus, Option<WorldDelta>), RecoverableError>;
```

Examples of tasks expressed this way:
- **`MoveTo(target)`** reads the agent's `Transform`, the nav result from spec
  45, and writes the next step back through a `ColumnWrite`.
- **`CheckSight(entity)`** reads a fixed distance/orientation from the blackboard
  and returns Success/Failure (pure geometry, spec 13 raycast).
- **`Wander(seed)`** advances a deterministic wander target from a blackboard
  cell seeded by the agent id.

Tasks never write to a global AI store; all their memory is the blackboard + the
world delta.

### Task table & tree compile

A compile step (Domain B, deterministic) validates a serialized tree: node
indices in bounds, single root, decorators have exactly one child, task ids
resolve, blackboard key hashes unique. Compilation emits the fixed node table the
`BehaviorTree` component references; nothing about it depends on runtime state,
so identical source trees compile identically (useful for editor round-trips and
for netcode that must share brains).

### Debug highlight active node (Domain A read)

The per-tick **trace** (step 5 above) is written into a small ring buffer in a
world resource the editor (spec 24) reads to *highlight* the currently active
node in the tree editor. This is strictly observational: Domain A never routes
trace data back into the guest, and toggling the debugger changes no tick output
— proving determinism is preserved whether or not the debugger is on.

## Key Rust Types

```rust
//! crates/logic-ai (Domain B, no_std, pure). No std/threads/RNG/GPU.
use openengine_math::I16F16;
use contracts::{StateView, WorldDelta, Entity, ComponentId, RecoverableError};

pub struct BehaviorNode { /* as above — ComponentId 34 payload */ }
pub enum NodeKind { Selector = 0, Sequence = 1, Decorator = 2, Task = 3 } // repr(u8)
pub enum NodeStatus { Failure = 0, Success = 1, Running = 2 }             // repr(u8)
pub type AiTask = fn(/* ... */) -> Result<(NodeStatus, Option<WorldDelta>),
                                          RecoverableError>;

/// One pure, deterministic AI tick over all agents with an AIAgent component.
pub fn ai_tick_system(view: &StateView<'_>)
    -> Result<WorldDelta, RecoverableError>;

/// Evaluate one tree against one blackboard (used by the tick, and directly by
/// headless tests). Returns the status at the root + trace.
pub fn evaluate(
    tree: &BehaviorTree,
    bb: &BlackboardView<'_>,
    view: &StateView<'_>,
    tick: u64,
) -> Result<(NodeStatus, Trace), RecoverableError>;

/// Deterministic tree compile/validate.
pub fn compile_tree(source: &TreeSource, task_table: &TaskTable)
    -> Result<BehaviorTree, RecoverableError>;
```

```rust
//! crates/editor + crates/ai-data — Domain A: tree editor, live trace reader.
pub struct TreeEditorView { /* read BehaviorTree node table */ }
pub struct ActiveTrace { pub node_path: Vec<u32>, pub statuses: Vec<NodeStatus> }
```

## Components

| `ComponentId` | Name           | Domain use (owner)                                   |
|---------------|----------------|------------------------------------------------------|
| **34**        | `BehaviorTree` | Serialized static node forest + root for a tree.     |
| **35**        | `Blackboard`   | Per-agent typed key–value memory (ECS/data).         |
| **36**        | `AIAgent`      | Which tree + blackboard + agent params (id/seed).    |

IDs 34–36 are **frozen** here. Terrain/Foliage (30–33) are specs 42/43's;
NavMesh/NavAgent/NavObstacle (37/38/39) is spec 45's. Additional AI components
(such as a `BlackboardKeyRegistry` or `BehaviorTreeSettings`) append in the
40–49 reserved window of this phase; never reuse 30–39.

```rust
/// BehaviorTree — the serialized forest a tree refers to. ComponentId 34.
#[repr(C)]
#[derive(Clone, Copy, bytemuck::Pod, bytemuck::Zeroable,
         serde::Serialize, serde::Deserialize, Debug)]
pub struct BehaviorTree {
    pub root: u16,                // index into node_table
    pub node_count: u16,
    pub node_table: [BehaviorNode; MAX_TREE_NODES], // fixed inline table
    pub key_hash_seed: u64,       // deterministic blackboard-key hash domain
}

/// Blackboard — per-agent typed memory. ComponentId 35.
/// Fixed-capacity typed cell array; backing store surfaced read-only to Domain B.
#[repr(C)]
#[derive(Clone, Copy, bytemuck::Pod, bytemuck::Zeroable,
         serde::Serialize, serde::Deserialize, Debug)]
pub struct Blackboard {
    pub len: u16,
    pub capacity: u16,
    pub slots: [BlackboardSlot; MAX_BLACKBOARD_SLOTS], // key hash + type tag
    pub payload: [u8; MAX_BLACKBOARD_BYTES],           // typed value bytes
}

/// AIAgent — bind a tree + blackboard to an entity. ComponentId 36.
#[repr(C)]
#[derive(Clone, Copy, bytemuck::Pod, bytemuck::Zeroable,
         serde::Serialize, serde::Deserialize, Debug)]
pub struct AIAgent {
    pub entity: Entity,           // the actor this brain drives
    pub tree: Entity,             // entity holding the BehaviorTree (34)
    pub blackboard: Entity,       // entity holding the Blackboard (35)
    pub agent_id: u32,            // stable id; also seeds deterministic wander
    pub active: u8,               // 0/1: participate in ai_tick_system
    pub _pad: [u8; 3],
}
```

Because component types appear once per archetype, an agent attaches
`AIAgent` (+ its own `Blackboard`), while a shared `BehaviorTree` is a separate
entity referenced by `tree`; several agents may share one tree. This is the
spec 21 "component appears once; repeated config = a table entity" pattern.

## Constraints

- **Determinism law.** `ai_tick_system` is a pure Domain-B system: no wall
  clock, no ambient RNG, no `HashMap` iteration, fixed-point only, seeded wander.
  Cooldown/timers are blackboard-stamped **tick counts**, not `std::time`.
  Identical (tree, blackboard, world, tick) ⇒ identical node visits + deltas.
- **No global mutable AI state.** Blackboards are per-agent ECS/data, not a
  `static`; the runtime uses no hidden global tables. The task table is an
  immutable, compiled registry.
- **Tasks are pure systems returning `WorldDelta`.** Tasks never reach into ECS
  memory with `&mut`; all writes flow out through the delta. A running task
  yields by writing its continuation to the blackboard (data), never by holding
  a live closure across ticks.
- **Fixed-point blackboard.** Values are `I16F16`/integers/`Entity`/fixed
  vectors — never `f32` — so the blackboard round-trips deterministically.
- **Pod + serde.** All three components are `#[repr(C)] Pod + Zeroable` + serde,
  byte-size multiple of 4 where useful; serializable via spec 16 for save/replay
  and netcode.
- **Tree is static structure.** No per-node runtime memory, no pointers into
  ECS; serialization of a `BehaviorTree` is lossless.
- **Debug is read-only.** Active-node highlighting reads a trace Domain A never
  feeds back; the debugger cannot alter tick output (guards determinism
  verification).
- **Edit vs Play.** Trees/blackboards are authored in edit world; play deep-clones
  them deterministically (spec 22). Play only runs `ai_tick_system`; the tree
  editor never ticks the edit world.
- **Cross-domain transport only via contracts.** Tasks resolve through a stable
  `TaskId`, not raw addresses, keeping the ABI clean (spec 12 hot-reload safe).
- **Portability/no path/GPU-free logic.** Compiles `x86_64-linux` +
  `aarch64-linux`; no absolute paths; headless tests run with no GPU.

## Performance Targets

- `ai_tick_system` for **1000 agents**, shallow trees (≤ 8 nodes): **< 4 ms /
  tick** Domain B.
- `evaluate` per agent on a shallow tree: **< 5 µs** (explicit-stack walk, no
  recursion, no allocation beyond a reused scratch Vec).
- Blackboard slot get/set by key: O(slots) linear scan over a small fixed array,
  or a sorted-index fast path — **< 100 ns** typical.
- Compile/validate a 500-node tree: **< 5 ms** (editor-time, one-off).
- Trace write per agent: bounded append, **< 1 µs**, ring buffer so it never
  grows unbounded.
- Active-node debug read adds **~0** to tick cost (trace produced in-tick, read
  post-hoc).

## Testing Strategy

All headless (no GPU) in `crates/logic-ai` + editor tests:
- **Node semantics:** unit tests for Selector/Sequence short-circuit, Decorator
  (Inverter/Repeater/Retry/Succeed/Fail), and Running continuation, against
  scripted child statuses.
- **Determinism:** run a tree with seeded wander over the same initial world
  3× on two targets; assert identical node visit sequence, blackboard final
  bytes, and applied `WorldDelta` (spec 13/23 determinism protocol).
- **Running task resume:** a multi-tick `MoveTo` (spec 45 nav) yields Running
  with `running_child` set; next tick resumes at the recorded child and
  completes deterministically.
- **Blackboard integrity:** set/get round-trip all supported value types; assert
  key-hash collision is rejected at compile time and never silently misreads.
- **Task table:** a tree referencing an unknown `TaskId` fails compile; valid
  trees resolve to the intended pure tasks (including through a simulated
  hot-reload recompile, spec 12).
- **Edit-vs-play:** authored tree/blackboard deep-clone into play bit-identically;
  editor actions never mutate a playing agent (spec 22).
- **Serialization:** `BehaviorTree`/`Blackboard`/`AIAgent` round-trip the spec-16
  codec bit-identically; recompile-from-same-source matches the serialized node
  table.
- **Debug isolation:** run the same scene with the node-highlight debugger on and
  off; assert the resulting world states are byte-identical (debug never steers
  the sim).
- **Purity:** `verify-wasm-purity` reports `[PURE]` for `crates/logic-ai`.

## Dependencies

- `contracts` (`StateView`, `WorldDelta`, `ColumnWrite`, `DeferredCommand`,
  `Entity`, `ComponentId`, `RecoverableError`), `bytemuck`, `serde`, `postcard`,
  `alloc`.
- `openengine-math` (`I16F16`, `I32F32`).
- Spec 21 (`Transform`, `Entity::INVALID`), spec 01/05 (tick), spec 00 (system
  shape), spec 16 (codec), spec 22 (edit/play), spec 24 (tree debug UI), spec 12
  (hot-reload task table), spec 13 (raycast for sight tasks), **spec 45**
  (`MoveTo` nav task reads nav result).
- Domain A: `crates/editor` tree editor; `crates/logic-sandbox` export path.

## Next Steps

1. Register `BehaviorTree` (34), `Blackboard` (35), `AIAgent` (36) components.
2. Implement node-table Pod layout + deterministic compile/validate.
3. Implement `evaluate` + explicit-stack walk with Running continuation.
4. Implement `ai_tick_system` over all agents and the pure task table.
5. Add reference tasks (`MoveTo`/nav from spec 45, `CheckSight`, seeded
   `Wander`) as Domain-B pure systems.
6. Domain-A trace reader + tree-editor active-node highlight (spec 24).
7. Determinism/serialization/debug-isolation test battery + purity CI.
