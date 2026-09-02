---
task_id: "TASK-001"
title: "Add Position component to contracts"
status: "todo"
priority: "high"
phase: "Phase 3: ECS Memory Bridging"
depends_on: []
blocks: ["TASK-002", "TASK-003"]
estimated_time: "15min"
required_context:
  - "AGENTS.md"
  - ".agents/knowledge/CONSTRAINTS.md"
  - "contracts/src/lib.rs"
  - "crates/math/src/lib.rs"
---

# TASK-001: Add Position Component

## Goal
Add a `Position` component to `contracts` that can cross the host↔guest boundary
with fixed-point math and `bytemuck` zero-copy.

## Preconditions (do these first)
- Enable `fixed`'s `bytemuck` feature so `I16F16` implements `Pod`/`Zeroable`:
  in `crates/math/Cargo.toml`, `fixed = { workspace = true, features = ["bytemuck"] }`.
- Make `contracts` depend on `openengine-math` (no cycle: math→fixed only).

## Requirements
Add to `contracts/src/lib.rs`:

```rust
#[repr(C)]
#[derive(Clone, Copy, Debug, PartialEq, Eq,
         bytemuck::Pod, bytemuck::Zeroable,
         serde::Serialize, serde::Deserialize)]
pub struct Position {
    pub x: openengine_math::I16F16,
    pub y: openengine_math::I16F16,
}
```

Constraints:
- `openengine_math::I16F16` (fixed-point), NOT `f32`.
- `#[repr(C)]`, `pub`, derives above (Pod requires the field type to be Pod,
  hence the `fixed` bytemuck feature precondition).

Add unit tests (`cargo test -p openengine-contracts`):
- `size_of::<Position>() == 8`.
- `bytemuck::bytes_of(&pos).len() == 8` and round-trip via `cast_slice`.

## ABI
Adding a new type is additive. Bump `ARCH_VERSION` 2 → 3 and add a
`docs/abi/CHANGES.md` entry (a new ABI payload is a boundary change). Update all
consumers in the same commit.

## Test Protocol
- Unit: `cargo test -p openengine-contracts` → pass.
- Build: `cargo build -p openengine-contracts` → success (x86_64-linux).
- Lint: `cargo clippy -p openengine-contracts -- -D warnings` → clean.
- Determinism: `Position` from identical fixed inputs → identical (run 3×).
- Docker/CI/offline: yes. No GPU.

## Acceptance Criteria
- [ ] `Position` in `contracts`, all derives present, `#[repr(C)]`, `pub`,
      fixed-point only.
- [ ] `fixed` bytemuck feature enabled; `contracts` depends on `openengine-math`.
- [ ] `ARCH_VERSION` bumped 2→3 with `docs/abi/CHANGES.md`.
- [ ] Tests/build/clippy pass; no hardcoded paths or OS-specific code.

## Next
`TASK-002-host-write.md`

## Handoff
When done update this file, `tasks/INDEX.md`, `STATE.md`, log in `events/`.
