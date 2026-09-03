# ROADMAP.md — OpenEngine Roadmap

---
name: "OpenEngine Roadmap"
version: "1.0.0"
updated: "2026-09-03"
---

# ROADMAP.md

## Phase 1: Scaffold ✅
- Repository structure
- Workspace `Cargo.toml`
- ABI contracts (`contracts/`)
- AI governance files

## Phase 2: Living Window ✅
- `winit` window creation
- `wgpu` (Vulkan) initialization
- `wasmtime` sandbox loading
- Wasm↔host communication via postcard
- `ClearColor` command demonstration (ABI v2)

## Phase 3: ECS Memory Bridging ✅
- `Position` component definition (fixed-point `Pod`) ✅
- Host writes SoA memory into a guest-allocated buffer ✅
- Guest reads memory with safe Rust + `bytemuck::cast_slice` ✅
- Guest returns `ColumnWrite` delta ✅
- Host applies the delta back to its `Vec` ✅

## Phase 4: Real Game Loop 🚧 (mostly done)
- `winit` event polling (keyboard/mouse) ✅
- Fixed timestep (60 ticks/sec) ✅ — shell Play ticks the guest at fixed 60 Hz
- Multiple systems in Wasm (movement, input, render) ✅ — movement + full
  `gameplay_tick` (WASD/jump/gravity, NPC wander/circle/chase) run in the guest
- Entity spawning/despawning ⏳ — not yet exposed through the editor UI

## Phase 5: Asset Pipeline
- `ResourceRequest`/`ResourceReady` protocol — todo
- Texture loading — todo
- Mesh loading — todo
- Asset caching — todo

## Phase 6: Editor 🚧 (core + shell merged on `main`)
- `egui` integration ✅ — `editor-shell` (egui 0.32 + wgpu 25)
- Scene editor ✅ — Edit/Play, orbit camera, selection/picking, lit viewport
- Component inspector ✅ — transform inspector + undo/redo
- Play runs real wasm logic ✅ — `feat/wasm-play` (guest `gameplay_tick`)
- Hot-reload UI ⏳ — next (spec 10)
- Gizmo translate / drag-select / save-load scene ⏳

## Phase 7: Polish & Documentation
- Performance profiling — todo
- API documentation — ongoing (STATE.md / ROADMAP / docs/abi maintained)
- User guides — todo
- Release preparation — todo
