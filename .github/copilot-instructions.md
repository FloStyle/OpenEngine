# GitHub Copilot instructions for OpenEngine

Context: AI-native Rust + Wasm engine. Game logic is pure; state is immutable;
side effects forbidden. See AGENTS.md (outranks this file).

## Domain boundaries
- Domain B (`crates/logic-sandbox`, `crates/math`): `#![no_std]`, `#![forbid(unsafe_code)]`. No wgpu, winit, wasmtime, egui, rayon. Pure functions returning `Result<WorldDelta, RecoverableError>`.
- Domain A (`crates/core`, `crates/ecs`, `crates/editor`): host side; std/GPU/threads allowed; own source still inherits `forbid(unsafe_code)`.

## Rules to honor when generating code
- Read `contracts/src/lib.rs` and note `ARCH_VERSION` before changing anything.
- Use `openengine-contracts` types and `postcard` for any host<->guest data. Do not invent ad-hoc wire structs.
- Always prefer `bytemuck::cast_slice` for SoA views.
- Never use `Mutex` in ECS hot paths; use atomic counters or lock-free queues.
- All Wasm host functions must be defined in `contracts/host_functions.rs`.
- Gameplay math must use `fixed` (via `openengine-math`) or explicit `glam` rounding. Raw `f32` is forbidden in logic.
- All `#[repr(C)]` cross-ABI structs must be `bytemuck::Pod` over POD fields.
- Fix clippy lints; add a reason on any scoped `#[allow]`.
