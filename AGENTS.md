# AGENTS.md — OpenEngine AI Constitution

> This file is the **master rulebook** for every autonomous agent (DeepSeek,
> Cursor, Codex, GitHub Copilot, CI critics) that writes code in this
> repository. It outranks all per-tool rule files. If a tool-specific file
> contradicts this document, **this document wins**.

---

## 0. Prime Directive

> **Game logic is pure. State is immutable. Side effects are forbidden.**

Every line of gameplay code in Domain B must be a pure function of its inputs:
same `StateView`, same tick, always the same `WorldDelta`. No I/O, no wall
clock, no randomness without an explicitly injected seed, no mutation of shared
memory. Determinism is a product requirement, not a preference — the same
simulation must reproduce on a laptop and on a locked-down CI VM.

When you are tempted to "just write to the state" or "just call `println!` in
logic," **stop and reconsider**: you are violating the Prime Directive.

---

## 1. Domain Boundaries

The repository is split into two software domains with a **hard dependency
direction**. Cross-domain calls go through the ABI in `contracts/` and nothing
else.

| Domain | Crates | Rights | Forbidden |
|--------|--------|--------|-----------|
| **A — Core** (host) | `crates/core`, `crates/ecs`, `crates/editor` | `std`, `wgpu`(Vulkan), threads, files, windowing | — (see Unsafe Policy) |
| **B — Logic** (guest) | `crates/logic-sandbox` (logic), `crates/logic-export` (wasm cdylib bridge), `crates/math` | `#![no_std]`, `alloc`, pure FP, `fixed` math | `std`, `wgpu`, threads, I/O, raw `f32` in logic |

Rules that keep the boundary intact:

1. **Domain B may depend only on** `contracts`, `openengine-math`, and other
   `no_std` + `forbid(unsafe_code)` crates. It must never list `wgpu`, `winit`,
   `wasmtime`, `egui`, or `rayon` in its `Cargo.toml`. CI enforces this.
2. **Domain B exports a pure system of the form**
   `fn(&StateView) -> Result<WorldDelta, RecoverableError>` — see
   `contracts/src/lib.rs`. That is the *only* sanctioned mutation path.
3. **Domain A never reaches into gameplay logic for state.** It reads
   `WorldDelta` out of the sandbox and applies it; it does not edit the guest's
   memory.
4. State changes flow **one way**: guest produces a delta, host applies it.

---

## 2. The Context Rule (READ THIS FIRST)

> **Before writing any code, read `contracts/` and note `ARCH_VERSION`.**

`ARCH_VERSION` in `contracts/src/lib.rs` is the current ABI revision. Every
`#[repr(C)]` layout and every enum variant in that crate is a **wall**: both
sides compile against it, so a change on one side breaks the other loudly.

- **You have full freedom** as long as you do not change `contracts/`.
- **You have almost no freedom** once you touch `contracts/`. Any layout change
  requires (a) a spec update under `docs/abi/`, (b) a bump of `ARCH_VERSION`,
  and (c) all consuming crates updated in the same commit. Prefer **adding**
  fields over reshaping; prefer never changing the meaning of an existing field.
- If a change is *purely internal* (e.g. ECS storage algorithm), `ARCH_VERSION`
  does not change but `docs/abi/CHANGES.md` should still record it.

---

## 3. The Determinism Law

> All gameplay math MUST use the `fixed` crate or an explicit `glam` rounding.
> Raw `f32` is **forbidden** in logic systems.

- Use `openengine-math` types (`I16F16`, `fx!`) — these compile to exact
  integer arithmetic, identical on every host.
- If a value must pass through the GPU/`f32` world, round-trip it through
  `openengine-math::quantize_to_f32` on the **Domain A side** only.
- No dependence on `std::time`, thread scheduling, `HashMap` iteration order
  (use a deterministic order), or ambient randomness.

---

## 4. Memory-Safety & Concurrency Policy

- `#![forbid(unsafe_code)]` is a workspace-wide default (inherited via
  `[workspace.lints]`). Domain B additionally cannot even *declare* the intent.
- **Unsafe Policy:** if a Domain-A crate ever genuinely needs `unsafe` for a
  zero-copy bridge, it is an RFC. The unsafe is moved into exactly **one**
  dedicated module with an `#[allow(unsafe_code)]` *on that module only*, a
  written safety justification, and a test that reads under sanitizers. You do
  not weaken the workspace default.
- **Hot paths never use `Mutex`.** Prefer per-archetype atomic counters and
  lock-free queues. Sparse/administrative locks only. (`rayon` work-stealing is
  the sanctioned parallel primitive in Domain A.)
- Prefer `bytemuck::cast_slice` for every SoA view. Never `transmute`.

---

## 5. How AI Agents Should Work Here

This repo is designed for **concurrent, autonomous agents** without context
drift. Conventions that keep you safe:

1. **One crate, one intent per change.** Small, reviewable PRs; the ECS is
   archetype+SoA and its growth is a milestone, not a side quest.
2. **Use the ABI types.** When you need to move data across the wall, use the
   `contracts` types and `postcard`. Do not invent ad-hoc message structs — that
   is how the boundary rots.
3. **Rustdoc is an AI interface.** Write docs as if a *future agent* (not a
   human) will read them cold. State invariants, lifetimes, and what "must
   hold" for memory safety.
4. **Never silence a lint to make CI green.** `deny(clippy::all)` is global. If
   you hit a false positive, add a scoped `#[allow]` **with a reason** on the
   narrowest item.
5. **Update `docs/` when behavior changes.** Human-readable specs live there;
   `contracts/` and `docs/abi/` move together.
6. **The Python Brain is your referee.** Before a Domain-B merge, run
   `brain/orchestrator.py verify` to prove the Wasm binary imports nothing
   forbidden. Wire it into your mental workflow, not as an afterthought.
7. **Never edit a file another agent "owns" in-flight** without checking
   `.ai/lock/`. Claim a lock for large edits to shared files (`contracts/`,
   root `Cargo.toml`, `AGENTS.md`).

### Suggested entry points by role

- **Building a new gameplay system** → read `contracts/`, copy
  `crates/logic-sandbox/src/lib.rs`'s `tick_color` shape, return a `WorldDelta`.
- **Working on the ECS / renderer** → Domain A; read `contracts/` then
  `crates/ecs/` or `crates/core/`.
- **Asked to "make it faster"** → profile first; `Mutex` removal and `cast_slice`
  are the usual wins; never trade determinism for speed in Domain B.

---

## 6. Definition of Done

A change is *done* only when **all** of the following hold:

- [ ] `ARCH_VERSION` in `contracts` is unchanged **or** deliberately bumped with
      a matching `docs/abi/` update and all consumers rebuilt.
- [ ] `cargo build --workspace` and `cargo test --workspace` pass.
- [ ] `cargo clippy --workspace --all-targets -- -D warnings` passes (no
      silenced lints without an inline reason).
- [ ] Domain-B crates compile with `--target wasm32-unknown-unknown`, and
      `brain/orchestrator.py verify` reports **pure**.
- [ ] `rustfmt` applied.
- [ ] Behaviour-affecting changes recorded in `docs/` and `docs/abi/CHANGES.md`.

---

*Last reviewed: ABI v2.*
