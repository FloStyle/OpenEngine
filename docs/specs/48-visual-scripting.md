---
spec: "48-visual-scripting"
phase: "Phase 5: Advanced"
status: "draft"
author: "OpenEngine AI"
created: "2026-09-03"
depends_on:
  - "00-ecs-architecture"
  - "05-time-system"
  - "06-scene-management"
  - "07-editor-inspector"
  - "12-scripting-macros"
  - "16-serialization"
  - "21-primitive-components"
  - "22-edit-vs-play"
  - "23-undo-redo"
---
# 48 - Visual Scripting (Blueprint-style Node Graphs)

## Overview

OpenEngine's visual scripting is a **node-graph → pure-Wasm** pipeline: an agent
or level designer wires **nodes** together on a canvas in the editor (Domain A,
`crates/editor`), and the resulting graph is **compiled into an ordinary
Domain-B `#![no_std]` wasm module** exactly like a hand-written
`#[system]` function. That compiled module is a first-class `logic.wasm`
artefact: it passes the same purity verification, runs at the fixed 60 Hz tick
(spec 05), returns a `WorldDelta`, and is deterministic bit-for-bit on every
platform. There is **no interpretive layer** inside the simulation — the graph
becomes Rust, the Rust becomes wasm, and Domain B sees only a pure system.

This is the natural convergence of two existing pieces of the engine:

* **spec 12** (`#[system]` macros) already turns a pure `fn(&StateView) ->
  Result<WorldDelta, RecoverableError>` into an ABI-conformant Domain-B system
  with a generated trampoline in `crates/logic-export`. A compiled node graph is
  just *another* such pure function, produced by a code generator instead of by
  a human author.
* **spec 00** already runs Domain B purely against a `StateView`; the graph's
  "event → read → compute → write" cycle is the same read-produce-return
  contract.

The editor half is **visual only**: the canvas, pins, wires, palettes, and the
debugger (step-through, variable watch) all live in Domain A. The *runtime* half
is indistinguishable from hand-authored Rust. This keeps the Prime Directive
("Game logic is pure") intact because **the compiler never emits anything a
human `#[system]` author could not write**.

### Component allocation note

A node graph bound to an entity is stored as the `ScriptNodeGraph` component,
**`ComponentId(64)`** — one of the reserved built-in ids 60–69 (see
"Components"). It is a read-only, editor-side handle to a *compiled* module plus
the authored `#[repr(C)]` graph blob used by the editor/debugger. The heavy
per-tick evaluation is pure Domain-B code produced by the compiler, not data
interpreted at runtime.

## Core Concepts

### Node = a pure function; Wire = data/control dependency

A **node** has typed pins. **Data pins** (`In`/`Out`) carry a value that flows
along a wire. **Execution pins** (`Exec In`/`Exec Out`) form a control-flow chain
that runs the node's body exactly once per event. Every node body is a pure Rust
expression; the *schedule* of execution is fixed at compile time and encoded as
a flat instruction program, never a runtime walk of the authored graph. The
graph is therefore a **dataflow + control-flow DAG** whose evaluation order is
a pure function of the compiled program, satisfying determinism by construction
(no `HashMap`, no runtime topology iteration — spec 03/AGENTS.md).

Three pin "families" exist and are enforced by the editor and by the compiler:

* **Data** — typed values. In Domain B every gameplay value is fixed-point
  `openengine-math::I16F16`, or a `Pod` composite (vector, colour), or a
  registry `Entity`/`ComponentId` token. No `f32` (AGENTS.md §3).
* **Exec** — zero-width control-flow edges (Begin/Step → a block).
* **Event** — entry points: `OnBegin`, `OnTick` (fixed tick), `OnCollision`
  (from spec 13/49 contacts surfaced as deferred events), `OnInput`. Event nodes
  are the roots of the control-flow program.

### Built-in node palette

A curated, fixed set of pure built-ins shipped with the engine and mirrored as
templates in the palette:

* **Math**: Add, Sub, Mul, Div, Neg, Clamp, Min, Max, Abs, Lerp, Dot, Length,
  Normalize, Trig (Sin/Cos/Tan), Round. All operate on `I16F16`/`I32F32` and
  are emitted as calls into `openengine-math` (never `f32`).
* **Logic/Branch**: Branch (data if), Switch (on a `u32` token), Sequence
  (fire exec out 0..N in order), DoOnce, Gate (delay-until).
* **Events**: BeginPlay, Tick(delta_time/sim_time from spec 05), OnComponent
  (entity-local), Collision/Touch (surface of spec 13/49), Input (spec 03,
  re-expressed as deterministic queued input commands).
* **Variables**: graph-local/entity-local variables (fixed-point or `Pod`),
  read/write nodes compiled to plain column-local scratch or component fields.
* **Entity/Component**: Get/Set component field by `ComponentId`, Spawn (queued
  `SpawnCommand`), Despawn, Get owner. These compile to `WorldDelta` assembly,
  the *only* mutation path.

Every built-in maps 1:1 to emitted Rust that a `#[system]` could have. There is
no "magic" node that reaches outside the ABI.

### Custom nodes = pure Rust functions

A **custom node** wraps an ordinary pure Rust `fn` and appears in the palette as
a first-class node with typed pins. The author supplies:

```rust
// crates/logic-sandbox/src/custom_nodes.rs (Domain B, forbid(unsafe_code))
use openengine_logic_sandbox::prelude::*;

/// A pure helper exposed to the visual graph. Args and result are I16F16.
#[visual_node(category = "Math/Custom")]
pub fn smoothstep(a: I16F16, b: I16F16, x: I16F16) -> I16F16 {
    // fixed-point only; identical to a hand-authored #[system] helper
    x.clamp(a, b)   // illustrative
}
```

The `#[visual_node]` attribute (companion proc-macro to spec 12's `#[system]`)
registers metadata: display name, category, input/output pin types, and the
pure-fn symbol. The host editor reads this registry to populate the palette, and
the **compiler inlines/refers** to these same pure fns when it generates the
graph module. Because the fn is already pure `no_std` fixed-point Rust, a custom
node is purity-safe *by construction* — the same guarantee spec 12 gives a
system. A custom node is **not** a host import: it lives in Domain B and cannot
perform I/O, read a clock, or mutate shared state (AGENTS.md §0).

### Compilation: graph → pure Rust → wasm module

This is the heart of the spec. Compilation is a Domain-A, editor/CLI-time
operation (`crates/visual-scripting` tooling + the existing `scripts/build.sh`
bridge). It never runs in-game. The compiler:

1. **Validates** the graph: type-checks every data wire (pin types equal),
   checks exec chains are acyclic and reachable from an event root, checks every
   variable is declared before use, and rejects dangling pins. Any error is a
   compile-time error shown in the editor — never a runtime trap in the module.
2. **Topologically sorts** dataflow and linearizes control-flow into a flat,
   ordered instruction list (a `&[GraphOp]` program is emitted inside the module,
   order fixed by the DAG, not by iteration).
3. **Emits pure Rust** in a generated `crates/generated-graphs/src/<graph>.rs`
   module: a `pub fn <graph>_system(view: &StateView) ->
   Result<WorldDelta, RecoverableError>` whose body is the linearized program,
   reading `view.column(...)` via `bytemuck::cast_slice` (spec 00), running each
   node body in emitted order, and assembling a `WorldDelta`. The generated file
   is committed so `cargo expand` is the reviewable source of truth (spec 12).
4. **Compiles to wasm** through `openengine-logic-export`, exactly as spec 12
   systems do, producing a `graph_<graph_id>.wasm` staged under
   `OPENENGINE_WASM_PATH` (never a hardcoded path, AGENTS.md §5).
5. **Verifies purity** of the produced module (`brain/orchestrator.py
   verify-wasm-purity ...` → `[PURE]`) before the editor ever allows the graph to
   be saved as playable. A graph whose module is not `[PURE]` is refused.

> The compiler output is deliberately **human-authorable**: an agent could write
> the same `fn` by hand. That property is what keeps the Prime Directive airtight
> — the generated system is no more capable than spec 12's systems.

### Deterministic tick-based evaluation

The compiled module registers one or more systems on `SystemPhase::FixedUpdate`
(spec 01). Each fixed tick the host builds a `StateView` from the world columns
the graph declared it reads (its `query`, spec 12 metadata), invokes the module's
trampoline exactly as it would any system, receives a `WorldDelta`, and applies
it atomically (spec 00). Because the compiled program is a pure function of
`(&StateView, tick)` and evaluation order is compile-time-fixed, two hosts
running the same compiled graph on the same tick produce bit-identical deltas —
including in Wasm and across `x86_64-linux`/`aarch64-linux`. Time enters only as
the injected `view.tick` / fixed `delta_time` from spec 05; the graph never asks
for wall time.

### Debug step-through + variable watch (Domain A)

Debugging is a **host-side, EditorMode-aware** facility (spec 11, spec 22):

* **Step-through** pauses the play world (spec 22 `Paused`) and advances one
  fixed tick at a time. The editor highlights the node(s) whose emitted
  instructions executed in that tick, by mapping the emitted program's
  instruction index back to source nodes via a debug map the compiler attaches to
  the graph blob.
* **Variable watch** shows the current value of any graph variable and the value
  carried on any data wire, read from the play world's `StateView` while Paused.
  Watches never mutate the play world — they are read-only views (spec 22 Pause =
  freeze + read-only).
* **Breakpoint** on a node: when the compiled system returns and the instruction
  pointer would enter that node's emitted range, the host freezes the tick before
  applying that node's writes (an instrumentation hook compiled in under a
  `debug` feature only; shipping/release graphs omit it — see spec 50 code
  stripping).

Because stepping runs the *real* compiled module, a debugged graph behaves
exactly as it will in a release build (same code path, same determinism); the
only difference is added observability that is compiled out of shipping.

### Integration with spec 12 (`#[system]`)

A compiled visual graph is registered through the exact machinery of spec 12:
the `SystemMeta` registry (name, schedule, query) and the `#[no_mangle]`
trampoline in `crates/logic-export` that wasmtime drives. A graph module and a
hand-written system module are interchangeable in the host scheduler and
hot-reload enumerator (spec 10). A graph may also *call* a hand-authored
`#[system]`'s underlying pure fn if that fn is `#[visual_node]`-exposed, and a
hand-authored system may be wrapped as a custom node — the two authoring
surfaces feed the same purity-checked Domain-B universe.

## Key Rust Types

```rust
//! crates/visual-scripting/src/  (Domain A: editor + compiler tooling)
//! The authored-graph representation is Domain-A only. The compiled output is
//! ordinary Domain-B Rust. No graph type ever crosses into the runtime ABI.

/// A typed pin value kind. Gameplay values are fixed-point/Pod only.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PinKind {
    Exec,
    Float,     // I16F16
    Wide,      // I32F32 (wide-fixed scratch)
    Int,       // i32
    Bool,      // bool (stored as u8 Pod)
    Entity,    // contracts::Entity
    Component, // contracts::ComponentId
    Vec2,      // [I16F16; 2]
    Vec3,      // [I16F16; 3]
    Color,     // [I16F16; 4]
}

/// A node in the authored graph. `kind` selects a palette template or a
/// registered custom-node; `data` holds typed constant/config payload.
#[derive(Clone, Debug, serde::Serialize, serde::Deserialize)]
pub struct GraphNode {
    pub id: u32,
    pub kind: NodeKind,          // BuiltIn(BuiltinId) | Custom { fn_symbol, .. }
    pub inputs: Vec<Pin>,
    pub outputs: Vec<Pin>,
    pub exec_in: Option<PinRef>,
    pub exec_out: Vec<PinRef>,
    pub constants: Vec<Vec<u8>>, // zeroable/default Pod payloads, editor-authored
    pub position: (f32, f32),    // canvas only; never affects emitted semantics
    pub enabled: bool,
}

/// A wire between an output pin and an input pin.
#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct GraphWire {
    pub from: (u32, u32), // (node_id, output_pin_idx)
    pub to: (u32, u32),   // (node_id, input_pin_idx)
}

/// The authored graph blob attached to an entity as `ScriptNodeGraph`.
/// `#[repr(C)] Pod + Zeroable + serde` — serializable via spec 16 column codec.
#[repr(C)]
#[derive(Clone, Copy, Debug, PartialEq, Eq, bytemuck::Pod, bytemuck::Zeroable,
         serde::Serialize, serde::Deserialize)]
pub struct ScriptNodeGraph {
    pub graph_asset: AssetRef,   // logical relative-path token -> compiled module
    pub root_graph_id: u32,      // discriminator if a graph asset packs many
    pub module_abi_fingerprint: u64, // sanity: matches contracts::abi_fingerprint
    pub _reserved: [u8; 8],      // pad to 48 B (multiple of 8)
}
```

The full authored topology (nodes/wires/variables) is an **editor asset** (spec
02/16) stored under `OPENENGINE_ASSETS_PATH`, not inside the ECS component. The
`ScriptNodeGraph` component is the *binding* an entity uses to run a compiled
graph, plus the debug map reference for stepping. This keeps the component a
small `Pod` and keeps the runtime module decoupled from editor-only graph JSON.

```rust
/// The compiled program's shape (emitted as a const inside the generated
/// Domain-B module — a flat, ordered op list, NOT a runtime graph walk).
/// (Illustrative — lives in generated Domain-B code.)
pub enum GraphOp {
    LoadConst { dst: Slot, value: I16F16 },
    ReadColumn  { dst: Slot, component: ComponentId, index_expr: Slot },
    CallPure    { dst: Slot, fn_ordinal: u32, args: &'static [Slot] },
    Branch      { cond: Slot, then: u32, else_: u32 }, // control-flow
    WriteColumn { component: ComponentId, index: Slot, value: Slot },
    EmitDeferred{ topic: u32, payload: &'static [u8] },
}
```

`GraphOp` carries no heap structure and is laid out in a fixed order at compile
time, so its evaluation is a straight-line/deterministic fixed-point pass. It
never appears in an ECS column or crosses the ABI as an interpreted program.

## Components

| `ComponentId` | Name              | size_of | Domain use (all gameplay in Domain B) |
|---------------|-------------------|---------|----------------------------------------|
| 64            | `ScriptNodeGraph` | 48      | Binding: entity → compiled graph module + debug map (Domain A editor reads; Domain B merely keys by graph id). |

`ScriptNodeGraph` is assigned **`ComponentId(64)`**, inside the reserved
built-in range 60–69 staked out by specs 48/49/50. It is `#[repr(C)] Pod +
Zeroable + serde` and serializable through the spec 16 column codec; the actual
executable module lives as a `logic.wasm`/graph-module asset (spec 02), never
embedded in the column. Registering `ComponentId(64)` only once is enforced by
the `ComponentRegistry` (spec 00/21).

> Other built-ins in 60–69 are owned by spec 49 (advanced physics: 60–63) and
> remain reserved by spec 50's governance. No other spec may claim an id in
> 60–69 without updating this reservation note.

## Constraints

- **Compiled, never interpreted at runtime.** A playable graph is always a
  `[PURE]` Domain-B wasm module. No graph interpreter runs on the fixed tick; the
  authored graph is editor/asset-time data only.
- **Purity is mandatory and compiler-guaranteed**: the emitter only produces code
  a spec-12 `#[system]` author could write (fixed-point, `no_std`, read-only
  `StateView` → `WorldDelta`). Custom nodes are pure `no_std` fns. Every compiled
  module must pass `verify-wasm-purity` → `[PURE]` before it is playable.
- **No `f32` in gameplay math.** Pins carry fixed-point/Pod values; `f32` appears
  only in Domain-A canvas coordinates and display plumbing.
- **Deterministic evaluation.** Program order is compile-time-fixed (topological
  sort of a DAG); no `HashMap`, no runtime topology iteration, no wall clock, no
  ambient RNG. Same `StateView` + same tick ⇒ same `WorldDelta` everywhere.
- **Fixed timestep (spec 05).** Graphs run on `FixedUpdate`; time is the injected
  `view.tick`/fixed `delta_time`. No `Instant`/`SystemTime` in generated or custom
  node code (CI grep, spec 05).
- **Generated code is committed and reviewable** (`cargo expand` / emitted
  modules under a generated-graphs crate); `#[no_mangle]` confined to
  `logic-export` (spec 12).
- **Debugger is Domain A and read-only against the play world.** Step/watch use
  the `Paused` mode (spec 22); breakpoint/instrumentation is compiled in under a
  `debug` feature and stripped from shipping (spec 50).
- **Structural edits are undoable** via spec 23 (a node/wire add/remove/move is a
  `Command` on the *edit* graph asset), isolated from play by spec 22.
- Portability: editor tooling compiles on `x86_64-linux`/`aarch64-linux`; runtime
  graphs are pure wasm.

## Performance Targets

- Graph **compile** of a typical 200-node graph: < 1 s (emit + wasm link).
- Generated module **tick cost**: pure straight-line fixed-point ops; budget
  ≤ 16 ms/tick total Domain B (spec CONSTRAINTS), far below for ordinary graphs.
- No runtime graph interpretation: per-tick cost is proportional to the number of
  *emitted ops*, not to graph size; unreachable subgraphs compile out.
- Module size within the ≤ 256 MB/module Domain-B ceiling; typical graph modules
  are tens of KB.
- Step/breakpoint adds zero cost when compiled out (shipping), observable-overhead
  only under `debug`.

## Testing Strategy

- **Editor validation (Domain A, headless):** type errors, cycles, dangling pins,
  undeclared variables are all rejected at compile time; golden valid/invalid
  graph fixtures.
- **Emitter unit tests:** for a set of hand-authored graphs (math chain, branch,
  sequence, variable read/write, spawn/write-column), assert the emitted Rust
  calls the expected `openengine-math` ops and reads the expected `ComponentId`s.
- **Equivalence:** compile a graph and hand-write the equivalent `#[system]`;
  run both against the same seeded `StateView` for N ticks; assert byte-identical
  `WorldDelta`s (the compiler produces nothing a human could not).
- **Purity gate:** every compiled module runs `verify-wasm-purity` → `[PURE]`.
- **Determinism 3×:** run a compiled graph 3 times on `x86_64-linux`,
  `aarch64-linux`, and in Wasm; assert bit-identical outputs and event order.
- **Debugger:** pause (spec 22), step one tick, assert the highlighted node set
  matches the debug map and a watched variable reads the correct fixed value;
  release build has no stepping symbols.
- **Integration:** bind `ScriptNodeGraph(64)` to entities, run the graph as a
  `FixedUpdate` system through `logic-export`, apply deltas, and verify spawned
  entities / written columns / deferred events.
- **Undo isolation:** graph edits are undoable on the edit asset; play is never
  affected (spec 22/23).

## Dependencies

- Domain A (editor/compiler tooling): `crates/visual-scripting`, `crates/editor`,
  spec 02 asset storage, spec 07 palette/inspector, spec 22/23 edit/undo. Uses
  the existing `scripts/build.sh` + `openengine-logic-export` wasm pipeline.
- Domain B runtime: the *generated module* depends on `contracts`, `openengine-math`
  (`I16F16`/`I32F32`), and re-exports pure custom fns from `crates/logic-sandbox`
  — all spec-12-compatible, no new runtime dependency.
- `openengine-macros` (`#[visual_node]`, sharing spec 12's registration table).
- `brain/orchestrator.py verify-wasm-purity` as the compile gate.

## Next Steps

1. Bootstrap `crates/visual-scripting` (graph model, palette, validation) in
   Domain A.
2. Add `#[visual_node]` proc-macro + registry next to spec 12's `#[system]`.
3. Implement the **emitter**: validated DAG → committed pure-Rust module → wasm
   via `logic-export`; wire `verify-wasm-purity` as a hard gate.
4. Register `ScriptNodeGraph` = `ComponentId(64)`; bind graph assets to entities.
5. Build the canvas UI (pins/wires/palette) and route edits through spec 23.
6. Build the debugger (step/breakpoint/watch) on spec 22's Paused mode; gate
   instrumentation behind a `debug` feature for spec 50 stripping.
7. Land the equivalence, purity, determinism-3×, and integration test suite.
