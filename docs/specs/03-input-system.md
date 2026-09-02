---
spec: "03-input-system"
phase: "Phase 5"
status: "design"
---

# Input System

## Overview

Deterministic input for OpenEngine. All raw device events — keyboard, mouse,
gamepad — arrive in **Domain A** (`openengine-core`) through `winit` and `gilrs`.
Domain A folds them into a per-frame `InputState`, applies an action-mapping
layer, and then publishes a **fixed-point, ordered snapshot** into the
`StateView` so pure Domain B systems can query named *actions* (`"move_left"`)
without touching a `std HashMap` or any nondeterministic iteration.

The Prime Directive is the reason this exists: raw OS events are asynchronous
and arrive with a `winit` order that is not reproducible. The engine turns that
non-deterministic stream into one per-tick, fully-ordered, reproducible snapshot
that Domain B consumes as if it were any other deterministic input column.

## Design

### InputEvent (Domain A raw)

`winit`/`gilrs` deliver raw events; Domain A normalizes them to one enum before
folding them into state:

```rust
pub enum InputEvent {
    Key { code: KeyCode, action: KeyAction },     // KeyAction = Pressed|Released
    MouseButton { button: MouseButton, action: MouseAction }, // same idea
    MouseMoved { position: (f64, f64) },          // window-local pixels
    MouseWheel { delta: ScrollDelta },            // line or pixel delta
    GamepadButton { gamepad: GamepadId, button: GamepadButton, action: KeyAction },
    GamepadAxis { gamepad: GamepadId, axis: GamepadAxis, value: f32 }, // raw -1..1
}
```

Only Domain A sees these. The raw stream is drained at the top of each frame
(the `PreUpdate` phase in `01-game-loop.md`) and consumed to build that frame's
`InputState`.

### Per-frame InputState (Domain A)

```rust
pub struct InputState {
    pressed:        HashSet<InputBinding>,   // held this frame
    just_pressed:   HashSet<InputBinding>,   // transitioned up this frame
    just_released:  HashSet<InputBinding>,   // transitioned down this frame
    mouse_position: (f64, f64),
    mouse_delta:    (f64, f64),
    wheel:          f64,
    gamepad_axes:   BTreeMap<GamepadAxis, f32>, // last value per live axis
}
```

A binding identifies a concrete physical input:
`Key(KeyCode) | Mouse(MouseButton) | GamepadButton(GamepadId, GamepadButton) |
Axis(GamepadId, GamepadAxis, Direction)` where `Direction ∈ {Positive, Negative}`
captures which way a stick must go to count as "pressed".

**Edge semantics:** `just_pressed` contains a binding for exactly **one frame**
(the frame the transition was observed); `pressed` contains it for every frame
it is held. `just_released` likewise fires for one frame. `KeyAction::Pressed`
that repeats (OS auto-repeat) does **not** re-fire `just_pressed` — a binding is
either transitioning or steady within a frame, never both, which is what keeps
Domain B deterministic.

**Per-frame clearing:** at the top of each frame Domain A resets
`just_pressed`/`just_released`, recomputes `mouse_delta` from the last position,
and zeroes `wheel` *after* building the snapshot. Raw events mutate `pressed`
throughout the frame but only steady-state membership and the *transition sets*
computed at drain time are published.

### Action mapping (name → bindings)

Game logic wants semantic actions, not hardware codes. Domain A holds a mapping:

```rust
pub struct ActionMap {
    /// Sorted by action name for a stable, reproducible canonical order.
    actions: BTreeMap<ActionId, Vec<InputBinding>>,
}
```

An `ActionId` is a short lowercase identifier (`"move_left"`, `"jump"`,
`"fire"`). Multiple bindings may map to one action (e.g. both `Key(A)` and
`Key(ArrowLeft)` → `"move_left"`); one physical binding may feed multiple
actions if the map says so. The mapping is a fixed, registered table (from
config or a startup registration), **not** something Domain B mutates.

From `InputState` + `ActionMap` Domain A derives an *action table* that has the
same structure regardless of how the user mapped keys — this is the object that
crosses into Domain B.

### Fixed-point, ordered snapshot for Domain B

Domain B must never iterate a `HashMap`, and it must never see raw `f32`. So the
value published into the `StateView` is a **fixed-point, sorted** structure:

```rust
/// Published per tick into the StateView. Fully ordered by action id so two
/// runs observing the same input produce byte-identical queries.
pub struct InputSnapshot {
    /// Sorted by action id: one entry per action, regardless of physical map.
    pub actions: &'a [ActionState],
    /// Raw key/mouse bindings currently held, sorted by binding. Optional;
    /// used by debug/editor systems that legitimately want physical truth.
    pub held: &'a [InputBinding],
    pub mouse: MouseSnapshot,        // fixed-point position + delta + wheel
}
```

```rust
pub struct ActionState {
    pub action: ActionId,            // sorted ascending; binary-searchable
    pub pressed: bool,               // held this frame
    pub just_pressed: bool,          // transitioned on this exact frame
    pub just_released: bool,
    pub value: I16F16,               // analog weight: 0 or 1 for keys/buttons,
                                     // -1..1 mapped for axes (fixed-point)
}
```

Every `f32` mouse/axis value is quantized through
`openengine-math::quantize_to_f32`-inverse — i.e. converted **to** `I16F16` at
the ABI boundary — before it is placed in the snapshot. Raw floating mouse
coordinates never reach Domain B. `value` for an axis binding carries the
(quantized) stick deflection so analog movement is expressible.

**Why sorted:** `actions` is sorted by `ActionId` and `held` by `InputBinding`
(`BTreeMap`-derived order on the host, never `HashMap`). Domain B performs a
binary search by action name, so "query an action" is `O(log n)` and fully
reproducible. Determinism Law satisfied: no `HashMap`, no wall clock, no `f32`.

The snapshot is carried inside the `StateView` alongside the tick — added as an
auxiliary field (an ABI-coordinated change) or as a descriptor-backed column in
`StateView.arena` so it is `Pod`-castable and zero-copy. Design intent: append a
`StateView::input: Option<&InputSnapshot>` pointer the host fills each tick and
`None` for offline/headless logic runs (e.g. deterministic replays with scripted
input, which must also be expressible by injecting a synthetic snapshot).

### How Domain B queries

Domain B never touches device enums. It queries semantic actions through a tiny
read helper over the snapshot:

```rust
// no_std logic-side helper over the StateView input snapshot.
pub fn is_action_pressed(view: &StateView<'_>, id: ActionId) -> bool {
    view.input
        .map(|s| binary_search_action(&s.actions, id).map_or(false, |a| a.pressed))
        .unwrap_or(false)
}
```

Systems that need edges call `was_action_just_pressed`/`just_released` with the
same shape. Because the snapshot is ordered and fixed-point, these helpers return
identical answers for identical inputs every run — they can be unit-tested in
Domain B with no host, no GPU, and no device.

### winit/gilrs stay in Domain A

`winit` and `gilrs` are Domain A dependencies only (AGENTS.md Domain A rights).
`gilrs` runs its own event thread; Domain A pulls gamepad state into the
per-frame `InputState`, matching axes/buttons to the `GamepadId` slot map.
Neither crate is ever visible to Domain B, and CI enforces the dependency
boundary.

## Key types (Domain A)

```rust
pub enum ScrollDelta { Lines(f32), Pixels(f32) }
pub enum AxisDirection { Positive, Negative }
pub enum InputBinding {
    Key(KeyCode),
    Mouse(MouseButton),
    GamepadButton(GamepadId, GamepadButton),
    Axis(GamepadId, GamepadAxis, AxisDirection),
}
```

## Constraints

- Raw device handling (`winit`/`gilrs`), `InputState`, and `ActionMap` are
  Domain A only.
- Domain B sees only the sorted, fixed-point `InputSnapshot` — no `HashMap`, no
  raw `f32`, no device ids it must decode.
- `just_*` sets are single-frame; auto-repeat never re-fires an edge.
- All analog/mouse floats quantize to `I16F16` at the boundary.
- Snapshot order is canonical (sorted), never platform- or device-order.
- Determinism: same scripted input stream ⇒ identical snapshot every run.

## Performance targets

- Raw event drain + `InputState` update: < 50 µs/frame typical.
- Snapshot build + sort from the action map: < 20 µs with hundreds of bindings.
- Domain B action lookup: O(log n) binary search, ~20–40 ns.

## Testing strategy

- Unit: `just_pressed` fires once and only for one frame; `just_released`
  symmetric; key-repeat does not re-fire an edge.
- Unit: action mapping aggregation (multiple bindings → one action) and
  axis-direction matching.
- Unit: snapshot ordering is canonical and binary-searchable; byte-identical
  snapshot for identical synthetic event scripts across 3 runs.
- Domain B purity: query helpers compiled to wasm and verified
  `[PURE]`; no-device headless run uses a synthetic snapshot.
- Integration: feed a fixed `Vec<InputEvent>` (recorded on a dev machine) into
  the drain and assert deterministic `ActionState`s; mouse-delta accumulation.
- Headless/determinism: replay the same recorded event stream against a seeded
  world, assert bit-identical world after N ticks.

## Dependencies

Domain A: `winit`, `gilrs`, `openengine-contracts`, `openengine-math`,
`openengine-ecs`. If `InputSnapshot`/`ActionId` must cross into `StateView`, it
is an `ARCH_VERSION`-coordinated `contracts` change with a `docs/abi/` update.
Domain B: only the read helpers over the snapshot; no new Domain B dependencies
beyond `contracts` + `openengine-math`.

## Next steps

1. Define `InputEvent` normalization from `winit` window events.
2. Implement `InputState` with per-frame clear + single-frame edge sets.
3. `ActionMap` registration and binding→action aggregation.
4. Quantize + sort into a fixed-point `InputSnapshot`.
5. Add the snapshot to `StateView` (coordinated `contracts`/`docs/abi/` change).
6. Domain B `is_action_pressed` helper + wasm purity tests.
7. `gilrs` gamepad thread → per-frame state folding.
8. Deterministic replay harness over recorded `Vec<InputEvent>`.
