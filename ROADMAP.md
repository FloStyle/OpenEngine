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

## Phase 3: ECS Memory Bridging 🚧 (CURRENT)
- `Position` component definition (fixed-point `Pod`)
- Host writes SoA memory into a guest-allocated buffer
- Guest reads memory with safe Rust + `bytemuck::cast_slice`
- Guest returns `ColumnWrite` delta
- Host applies the delta back to its `Vec`

## Phase 4: Real Game Loop
- `winit` event polling (keyboard/mouse)
- Fixed timestep (60 ticks/sec)
- Multiple systems in Wasm (movement, input, render)
- Entity spawning/despawning

## Phase 5: Asset Pipeline
- `ResourceRequest`/`ResourceReady` protocol
- Texture loading
- Mesh loading
- Asset caching

## Phase 6: Editor
- `egui` integration
- Scene editor
- Component inspector
- Hot-reload UI

## Phase 7: Polish & Documentation
- Performance profiling
- API documentation
- User guides
- Release preparation
