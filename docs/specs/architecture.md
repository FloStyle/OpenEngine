# Architecture (human-readable spec)

Status: **scaffold / v0**. Mirrors `AGENTS.md` and `contracts/`.

## One picture

```
        Domain A (host, std)                 Domain B (guest, no_std wasm)
 ┌──────────────────────────────┐   postcard  ┌─────────────────────────────┐
 │  crates/core                 │   StateView │  crates/logic-sandbox       │
 │   wgpu (Vulkan) renderer     │◄────────────│  pure #[system] fns         │
 │   wasmtime sandbox host      │             │   read-only StateView       │
 │   winit event loop           │  WorldDelta │  fixed-point math           │
 │   job system (rayon)         │────────────►│  return Result<WorldDelta>  │
 │  crates/ecs  SoA+archetypes  │             └─────────────────────────────┘
 │  crates/editor  egui system  │        ┌──── math (fixed) shared no_std
 └──────────────┬───────────────┘        └──── contracts/ = THE WALL
                │
    crates/core sandbox: instantiate wasm module, zero-copy bridge SoA memory
```

## Tick loop (target)

1. ECS materializes a contiguous SoA **arena** per archetype.
2. Host builds a `StateView` (descriptors + arena bytes) into the guest.
3. Guest runs one or more pure systems → `WorldDelta`.
4. Guest returns the delta (postcard over shared memory).
5. ECS worker applies spawns/despawns/writes zero-copy (`cast_slice`).
6. Renderer + editor consume `DeferredCommand`s. Repeat.

## Decisions recorded here

- ABI is a dedicated top-level crate so both domains compile against one wall.
- `#![forbid(unsafe_code)]` is a workspace default; Domain B additionally is
  `#![no_std]`. Unsafe carve-outs in Domain A are RFC-gated to one module.
- Determinism: `fixed` math only in Domain B; `f32` only on the GPU/host side,
  crossing via `openengine-math::quantize_to_f32`.

## Living window (Phase 2 — the first end-to-end slice)

A running `winit` window whose clear colour is **computed inside Domain B** and
driven by Domain A, proving the Triforce boundary for real:

```text
winit frame ──tick──▶ logic-export (wasm) ──▶ logic-sandbox::tick_color
                      (no_std + fixed math)
                      returns WorldDelta{ DeferredCommand::ClearColor }
                      postcard-encoded into guest memory
   ▲                                                 │
   └── wgpu (Vulkan) clears surface with that colour ◄┘
```

- `crates/logic-export` is the wasm `cdylib` carrying the `#[no_mangle]`
  trampoline (`openengine_alloc`, `openengine_tick`). It must not inherit
  `forbid(unsafe_code)` because `#[no_mangle]` is an "unsafe attribute", so the
  forbid stays on all real logic.
- The guest stays **PURE** — `brain/orchestrator.py verify` reports no std/WASI
  imports. `StateView` carries only a `tick` for now; ECS memory bridging is not
  part of this slice.
- Build the module with `bash scripts/build.sh`, then
  `cargo run -p openengine-core` (needs a Vulkan-capable display).

## Not yet implemented (deliberately)
ECS column storage/archetypes, zero-copy SoA↔wasm bridging, the real renderer
pass (only a clear colour so far), the job scheduler, the egui panels, and the
LLM Critic RAG loop. Milestones to come.
