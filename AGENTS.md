# AGENTS.md — OpenEngine AI Constitution

---
name: "OpenEngine AI Constitution"
version: "1.0.0"
updated: "2026-09-03"
state: "STATE.md"
tasks: ".agents/tasks/INDEX.md"
---

> This file is the **master rulebook** for every autonomous agent that writes
> code in this repository. It outranks any per-tool rule file. If a tool file
> contradicts this document, **this document wins**.
>
> Agent workflow (STATE.md / .agents/) is layered on top of the technical rules
> below — the rules are the *why*, the workflow is the *how*.

---

## 0. Prime Directive

> **Game logic is pure. State is immutable. Side effects are forbidden.**
> All code must be portable, reproducible, and hardware-agnostic.

Every line of gameplay code in Domain B must be a pure function of its inputs:
same `StateView`, same tick, always the same `WorldDelta`. No I/O, no wall
clock, no randomness without an explicitly injected seed, no mutation of shared
simulation memory. Determinism is a product requirement, not a preference.

---

## 1. Domain Boundaries

The repository is split into two domains with a **hard dependency direction**.
Cross-domain calls go through the ABI in `contracts/` and nothing else.

| Domain | Crates | Rights | Forbidden |
|--------|--------|--------|-----------|
| **A — Core** (host) | `crates/core`, `crates/ecs`, `crates/editor` | `std`, `wgpu`, `winit`, threads, files, windowing | — (see Unsafe Policy) |
| **B — Logic** (guest) | `crates/logic-sandbox` (logic), `crates/logic-export` (wasm cdylib bridge), `crates/math` | `#![no_std]`, `alloc`, pure FP, `fixed` math | `std`, `wgpu`, threads, I/O, raw `f32` in logic |

Rules that keep the boundary intact:

1. **Domain B depends only on** `contracts`, `openengine-math`, and other
   `no_std` + `forbid(unsafe_code)` crates. Never `wgpu`/`winit`/`wasmtime`/
   `egui`/`rayon`/`tokio`. CI enforces this.
2. **Domain B exports a pure system of the form**
   `fn(&StateView) -> Result<WorldDelta, RecoverableError>`.
3. **Domain A never reaches into gameplay logic for state** — it reads a
   `WorldDelta` out of the sandbox and applies it.
4. State changes flow **one way**: guest produces a delta, host applies it.

---

## 2. The Context Rule (READ THIS FIRST)

Before writing ANY code:
1. Read `AGENTS.md` (this file).
2. Read `STATE.md` (current project state).
3. Read `.agents/tasks/INDEX.md` (available work).
4. Read the specific task file you were assigned.
5. Read every file in the task's `required_context`.
6. Read `contracts/src/lib.rs` and note `ARCH_VERSION`.

`ARCH_VERSION` is the current ABI revision. Every `#[repr(C)]` layout and enum
variant is a **wall**: both sides compile against it, so a change on one side
breaks the other loudly.

- **Full freedom** as long as you do not change `contracts/`.
- **Almost no freedom** in `contracts/`: any layout change requires a
  `docs/abi/` update, an `ARCH_VERSION` bump, and all consumers updated in the
  same commit. Prefer adding over reshaping.

---

## 3. The Determinism Law

> All gameplay math MUST use `openengine-math` (fixed-point) or an explicit
> `glam` rounding. Raw `f32` is **forbidden** in Domain B.

- `#![no_std]` + `#![forbid(unsafe_code)]` is the workspace-wide default;
  Domain B cannot even declare the intent.
- No dependence on `std::time`, thread scheduling, `HashMap` iteration order
  (use `BTreeMap`/sorted `Vec`), or ambient randomness.
- `f32` may appear only at a display/ABI emission boundary, never inside logic
  math.

---

## 4. Memory Safety & Concurrency

- `#![forbid(unsafe_code)]` is a workspace default (inherited via
  `[workspace.lints]`). `forbid` cannot be overridden by `#[allow]`.
- **Unsafe Policy (Domain A):** if a host crate ever needs `unsafe` for a
  zero-copy bridge it is an RFC; the unsafe goes in **one** reviewed module
  with an `#[allow(unsafe_code)]` *on that module only* + a written SAFETY
  justification + sanitizer tests. Never weaken the workspace default.
- **Domain B is always 100% safe Rust** — including memory bridging. Prefer
  guest-allocated buffers the host writes into (`wasmtime::Memory::write`);
  the guest reads its own memory with safe `&buffer[..]`.
- Hot paths never use `Mutex` (prefer atomics / lock-free queues). The only
  sanctioned guest `Mutex` is a `spin::Mutex` guarding *transport* buffers,
  never simulation state.
- Prefer `bytemuck::cast_slice` for SoA views; never `transmute`.

---

## 5. Portability Rules

- NO hardcoded paths. Use `CARGO_MANIFEST_DIR`, `env!("CARGO_MANIFEST_DIR")`,
  or paths relative to the workspace root.
- NO hardcoded usernames / home directories (`$HOME` or relative only).
- NO OS-specific or distribution-specific code without an ADR.
- All code must compile on `x86_64-linux` and `aarch64-linux`.
- Logic tests require NO GPU.
- Everything must build/test in Docker and in CI (GitHub Actions), and work
  offline after an initial `cargo fetch`.
- Configuration is via environment variables (never baked-in credentials):
  - `OPENENGINE_CONFIG_PATH` (default `./config/`)
  - `OPENENGINE_LOG_LEVEL` (default `info`)
  - `OPENENGINE_ASSETS_PATH` (default `./assets/`)
  - `OPENENGINE_WASM_PATH` (default `./assets/logic.wasm`)

---

## 6. Testing Protocol

Before marking ANY task complete:

1. Run every test in the task's **Test Protocol** section.
2. Verify Domain B purity (if applicable):
   `python3 brain/orchestrator.py verify-wasm-purity crates/core/assets/logic.wasm`
   → must print `[PURE]`.
3. Verify Docker build: `docker build -t openengine-test .`
4. Verify cross-platform compilation.
5. Determinism: run the deterministic tests 3 times; results must be
   bit-identical.
6. Update `STATE.md` and log in `.agents/events/`.

---

## 7. How AI Agents Should Work Here

1. **One crate, one intent per change.** Small reviewable changes.
2. **Use the ABI types** (`contracts`) + `postcard`. Do not invent ad-hoc wire
   messages — that is how the boundary rots.
3. **Rustdoc is an AI interface**: write docs for a future agent reading cold.
4. **Never silence a lint to make CI green.** `deny(clippy::all)` is global; a
   scoped `#[allow]` needs an inline reason.
5. **Update `docs/` when behavior changes**; `contracts/` + `docs/abi/` move
   together.
6. **The Brain is your referee** — `python3 brain/orchestrator.py verify-wasm-purity`.
7. **Never edit a file another agent owns in-flight** without claiming the
   advisory lock under `.agents/sessions/` (or `.ai/lock/`).

### Entry points by role

- **New gameplay system** → read `contracts/`, copy the `tick_color` shape in
  `crates/logic-sandbox/src/lib.rs`, return a `WorldDelta`.
- **ECS / renderer / bridge** → Domain A; read `contracts/`, then
  `crates/core/` / `crates/ecs/`.
- **"Make it faster"** → profile first; never trade determinism for speed.

---

## 8. How to Find Work

1. Read `STATE.md` to see the current phase and blockers.
2. Read `.agents/tasks/INDEX.md` for available tasks.
3. Pick a task with status `todo` and no blockers.
4. Read the task file completely; verify you have its `required_context`.
5. Update task status to `assigned` + your agent id, create a session file.
6. Execute the task following its **Test Protocol**.
7. Update task status to `done`, log the event, update `STATE.md`.

---

## 9. Definition of Done

A change is *done* only when **all** hold:

- [ ] `ARCH_VERSION` unchanged, or deliberately bumped with matching
      `docs/abi/` update + all consumers rebuilt in the same commit.
- [ ] `cargo build --workspace` and `cargo test --workspace` pass.
- [ ] `cargo clippy --workspace --all-targets -- -D warnings` passes (no
      silenced lints without an inline reason).
- [ ] Domain-B crates compile for `wasm32-unknown-unknown` and
      `verify-wasm-purity` reports `[PURE]`.
- [ ] `rustfmt` applied.
- [ ] No hardcoded paths / credentials / OS-specific code (Portability Rules).
- [ ] Behavior-affecting changes recorded in `docs/` + `docs/abi/CHANGES.md`,
      task/session files updated, `STATE.md` current.

---

*Reviewed: ABI v2 — Phase 3 (ECS Memory Bridging) underway.*

---

## Canonical definitions (post-audit)

These authoritative definitions resolve cross-spec drift flagged by
`docs/audit/audit-report-phase3.md`. Where a spec contradicts them, this section
wins.

### Time
- **`sim_time`** — deterministic simulation time (fixed timestep, tick-driven,
  Domain B). The only time gameplay logic may use.
- **`wall_clock`** — real elapsed time (Domain A: profiling, UI, timers only).
  **Domain B MUST use `sim_time`; wall clock is forbidden in gameplay logic.**
- `StateView` carries `tick: u64`. Any fractional time crossing to Domain B is
  injected as `I16F16` under an `ARCH_VERSION` bump, never `f64`.

### Frame budget (master)
- Fixed update ≤ 8 ms · Render ≤ 8 ms · Editor/overlay ≤ 4 ms (non-blocking).
- Total frame ≤ 16.67 ms. No single subsystem exceeds its slice (spec 00, 13, 15).

### Single mutation channel
- **All** world mutation — gameplay or editor — flows through `WorldDelta`
  (guest) or a spec-23 `Command` → `WorldDelta` → `apply_delta` (editor). There
  is **no** second channel: no `QueryMut` write path for tools, no direct ECS
  write from UI/inspector/gizmos. Editor mutations target the **edit world**
  only (spec 22); `play_world` is read-only for the editor.

### Error model
- Domain B / recoverable: `RecoverableError` (logged, continue).
- Unrecoverable: `FatalError` (rollback, else controlled abort). Guest reports
  only `RecoverableError`; a guest `FatalError` is a hard trap.
- Host typed wrappers are allowed as **only** two names: `EditorError`
  (edit/UI/commands) and `SceneError` (scene load/codec). No other host error
  synonyms (`EditorEditError`, `ModeError`, …).

### Canonical ABI helper types (`contracts`, ARCH v2 addendum)
- `AssetRef { id: u64, kind: u8 }` + `AssetKind` — the **only** asset reference.
  No raw asset paths/strings, no `AssetHandle`/`AssetRefLike`/`FixedStringLike`.
- `FixedString<const N>` — pod-safe fixed string.
- `AudioHandle(u64)`, `NetState { tick:u64, player_id:u32, input_hash:u64 }`.
- `ViewMode { Wireframe|Solid|Textured|Lit }` — one enum for the editor viewport.

### Component registry
- Every component is registered in spec 21 before use. Engine band 10–1023,
  game ≥ 1024; each subsystem appends inside its reserved window and updates the
  spec-21 table. Every component carries a `layout_version` (bump on any
  `#[repr(C)]` change) used by the spec-16 codec for migration.
