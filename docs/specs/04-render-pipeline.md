---
spec: "04-render-pipeline"
phase: "Phase 6"
status: "design"
---

# Render Pipeline

## Overview

A wgpu renderer that lives entirely in **Domain A** (`openengine-core`). The
world keeps render-relevant state as **fixed-point** components in the ECS
storage; Domain A converts those values to `f32` **only at the GPU emission
boundary**. Render systems produce an ordered `RenderCommand` list once per
frame, and the host submits it to the wgpu queue after the fixed update.

Deferred shading is **aspirational**. This spec deliberately starts with a
forward pipeline — clear → depth-prepass → shaded mesh draws — that is simple,
deterministic-friendly, and GPU-free in logic tests. Deferred G-buffer work is a
later phase; the command/component architecture below is shaped so a deferred
pass can be slotted in without reworking Domain B or the ECS render components.

## Design

### Render components (fixed-point in the world)

Render components are plain `#[repr(C)] Pod + Zeroable` ECS components (per
`00-ecs-architecture.md`), so Domain B can read them and can spawn/move things
the renderer will draw. They use fixed-point math where the value represents
gameplay-visible state; `f32` only appears at emission:

```rust
/// Position/rotation/scale in FIXED-point. Rotation is a yaw/pitch/roll
/// (degrees, I16F16) or a fixed quaternion; never raw f32.
pub struct Transform {
    pub translation: Vec3Fx,      // openengine-math I16F16 vector
    pub rotation:    QuatFx,      // fixed-point quaternion
    pub scale:       Vec3Fx,      // fixed-point
}

/// Reference to a host asset (see 02-asset-pipeline.md). Opaque u64 handle.
pub struct MeshRenderer {
    pub mesh:  AssetHandle<Mesh>,      // host-side opaque handle
    pub material: AssetHandle<Material>,
    pub visible: bool,
}

/// Camera. fov/near/far are held FIXED in the world (so Domain B can set them
/// deterministically) and converted to f32 only when the projection is built.
pub struct Camera {
    pub active: bool,
    pub fov_y:  I16F16,          // radians or degrees (fixed)
    pub near:   I16F16,          // fixed
    pub far:    I16F16,          // fixed
    pub clear:  ClearColor,      // fixed rgba? see note below
}
```

These components are read by Domain A render systems that scan the ECS
(`openengine-ecs` queries) each frame. Domain B mutates them through `WorldDelta`
`ColumnWrite`s like any other component; it never calls the GPU.

> **f32 discipline:** `Transform.translation` etc. are fixed in the world because
> gameplay writes them. Only the host's *render snapshot* build quantizes to
> `f32` (`openengine-math::quantize_to_f32`) to feed wgpu matrices. Logic tests
> never construct a matrix — they assert fixed component values.

### RenderCommand list (built once per frame)

Render systems (Domain A) walk the ECS and emit a flat, ordered command list;
the renderer submits it exactly once per frame:

```rust
pub enum RenderCommand {
    SetCamera(CameraView),            // picks active camera matrices
    SetClearColor { rgba: [f32; 4] }, // ABI emission of fixed clear color
    DrawMesh {
        entity: Entity,
        mesh:   AssetHandle<Mesh>,
        material: AssetHandle<Material>,
        transform: Mat4Fx,           // object -> world, still fixed
    },
    // later (aspirational): BeginGbuffer / SetAlbedo / SetNormal / ... 
    EndFrame,
}
```

The list is produced in a canonical order (sorted by material → depth → entity
handle) so batching is stable and reproducible. Domain A builds view/projection
matrices *from the fixed camera values*, quantizing only at the boundary.

### Camera view / projection (Domain A, from fixed values)

```rust
pub struct CameraView {
    pub view:        Mat4,   // f32, built in Domain A
    pub projection:  Mat4,   // f32 perspective from fixed fov/near/far
    pub position:    Vec3,   // f32 eye (quantized from fixed Transform)
}
```

Domain A constructs `view` from the active `Camera` entity's fixed Transform and
`projection` via `Mat4::perspective_rh(fov_y.to_f32(), aspect, near.to_f32(),
far.to_f32())`. This is the only place `f32` becomes authoritative for GPU math;
logic never sees these matrices.

### Mesh type (GPU buffers, Domain A)

```rust
pub struct Mesh {
    pub vertex_buffer: wgpu::Buffer,
    pub index_buffer:  wgpu::Buffer,
    pub vertex_count:  u32,
    pub index_count:   u32,
    pub layout:        VertexLayout,
}

pub struct VertexLayout {
    pub position: VertexElem,   // offset + format
    pub normal:   VertexElem,
    pub uv:       VertexElem,
    pub color:    VertexElem,
}

#[repr(C)]
pub struct Vertex {
    pub position: [f32; 3],
    pub normal:   [f32; 3],
    pub uv:       [f32; 2],
    pub color:    [f32; 4],
}
```

The vertex format is **`f32`** — meshes are pre-authored art data (from the asset
pipeline), not gameplay math, so `f32` here is correct and standard for the GPU.
Meshes are uploaded by the asset pipeline (`02-asset-pipeline.md`) and referenced
only by opaque handles.

### Material + MaterialTemplate

```rust
/// A concrete, bindable material instance.
pub struct Material {
    pub template: AssetHandle<MaterialTemplate>,
    pub albedo:      AssetHandle<Texture>,
    pub normal_map:  Option<AssetHandle<Texture>>,
    pub uniforms:    Vec<u8>,          // serially-packed uniform block (f32 at GPU)
}

/// The reusable definition: which shader + which bind-group slots + defaults.
pub struct MaterialTemplate {
    pub shader: AssetHandle<Shader>,     // WGSL module
    pub bind_layout: BindGroupLayout,
    pub default_params: MaterialParams,  // base color, metallic, roughness, ...
}
```

A `MaterialTemplate` pins the shader and pipeline layout; `Material` instances
reference one template plus concrete textures and a uniform blob. Renderer caches
pipelines keyed on `(template, topology, blend)` so only bind groups change per
draw. Serialized uniforms carry gameplay fixed-point values (e.g. tint, an
`I16F16` color) that Domain B sets via `ColumnWrite` and the host quantizes to
`f32` at upload.

### Depth buffer

A `wgpu::TextureView` depth attachment (e.g. `Depth32Float`) matching the swap
surface size, created once and reused each frame. The forward path runs a depth
prepass or draws with depth-write-on so later opaque draws benefit from
z-culling; transparents sort back-to-front by depth and draw last with blending.

### Batching and draw order

Within `submit`, Domain A sorts `DrawMesh` commands into batches:

1. **By pipeline** (template + blend) — minimize pipeline switches.
2. **By texture set / bind group** — minimize bind-group changes.
3. **Back-to-front for transparent** by view-space depth; **front-to-back /
   opaque-before-transparent** otherwise.
4. The ECS's canonical handle order breaks ties deterministically.

Only *state-changing* commands flush a batch; the renderer issues one
`wgpu::RenderPass` per frame, setting depth, clear, then drawing each batch.

### Forward sample path (current target)

```
clear (SetClearColor) -> set depth view ->
  [prepass over opaque DrawMeshes writes depth] ->
  [opaque pass: shaded DrawMeshes, depth-test LessEqual, no blend] ->
  [transparent pass: back-to-front, blend add/alpha] ->
  present via swap chain (PresentMode::Fifo)
```

### Sample WGSL shader (forward, diffuse)

```wgsl
struct Globals {
    view_proj : mat4x4<f32>,
    model     : mat4x4<f32>,
};
@group(0) @binding(0) var<uniform> globals : Globals;
@group(1) @binding(0) var albedo : texture_2d<f32>;
@group(1) @binding(1) var albedo_sampler : sampler;

struct VsOut {
    @builtin(position) clip_pos : vec4<f32>,
    @location(0) world_pos : vec3<f32>,
    @location(1) normal    : vec3<f32>,
    @location(2) uv        : vec2<f32>,
    @location(3) color     : vec4<f32>,
};

@vertex
fn vs_main(in : VertexIn) -> VsOut {
    var out : VsOut;
    out.world_pos = (globals.model * vec4<f32>(in.position, 1.0)).xyz;
    out.clip_pos  = globals.view_proj * vec4<f32>(out.world_pos, 1.0);
    out.normal    = normalize((globals.model * vec4<f32>(in.normal, 0.0)).xyz);
    out.uv        = in.uv;
    out.color     = in.color;
    return out;
}

@fragment
fn fs_main(in : VsOut) -> @location(0) vec4<f32> {
    let light_dir = normalize(vec3<f32>(0.5, 0.8, 0.6));
    let diff = max(dot(normalize(in.normal), light_dir), 0.0);
    return vec4<f32>(textureSample(albedo, albedo_sampler, in.uv).rgb
                     * (0.3 + 0.7 * diff), 1.0) * in.color;
}
```

The sample is deliberately minimal and artifact-free; it demonstrates the
vertex+fragment contract the `Material`/`Mesh` types feed. WGSL is embedded/loaded
via the asset pipeline (Shader kind) so it can hot-reload in dev.

## Constraints

- All GPU code, buffers, pipelines, and submission live in Domain A.
- World-facing render state (`Transform`, `MeshRenderer`, `Camera`) is fixed-point
  and ECS-hosted; `f32` appears only for the GPU/ABI emission boundary.
- Logic tests never build matrices or touch wgpu (no GPU required).
- No `unsafe` in this crate's own source; third-party `wgpu`/`glam` unsafe stays
  inside those dependencies (AGENTS.md Unsafe Policy).
- Command order is canonical/reproducible for determinism.
- Mesh/vertex `f32` is authored art data, distinct from gameplay math.

## Performance targets

- Command list build + sort from render components: < 1 ms for ~5k drawables.
- Batching keeps pipeline switches < ~100 for a typical scene.
- Frame GPU work fits the 16.67 ms budget at 60 Hz on `x86_64-linux` /
  `aarch64-linux`; PresentMode::Fifo (vsync).
- Fixed update < 8 ms; render < 8 ms (matches `01-game-loop.md`).

## Testing strategy

- Unit: fixed→f32 quantize of `Transform`/`Camera` values (deterministic rounding).
- Unit: view/projection math from fixed camera values, golden matrices.
- Unit: command builder emits canonical order; batching groups by
  pipeline/state and sorts transparents correctly.
- GPU smoke (requires device; not part of logic tests): submit one frame with a
  single quad + albedo texture + depth, assert no validation errors and a
  presented surface.
- Integration: two camera entities, toggling `active`, depth ordering of two
  overlapping meshes; transparent sorting.
- Domain B: render components are ordinary ECS components — spawn/move via
  `WorldDelta`, read back in Domain A, verify the renderer reflects the delta.
- Determinism: identical fixed state + input ⇒ identical command list across 3
  runs (offscreen headless where possible).
- Forward vs deferred: forward remains the default until the deferred G-buffer
  phase lands; tests assert forward-correctness, not deferred features.

## Dependencies

Domain A only: `wgpu`, `glam` (f32 at emission; optional), `bytemuck`, plus
`openengine-contracts`, `openengine-ecs`, `openengine-math`,
`openengine-asset`-pipeline types. If `Transform`/`MeshRenderer`/`Camera`
components must be *readable by Domain B* they live in `contracts`/`ecs` with a
coordinated `ARCH_VERSION` + `docs/abi/` update. `DeferredCommand::Render` and
`RenderKind::Mesh` in `contracts` already let Domain B request drawing.

## Next steps

1. ECS render components (`Transform` fixed-point, `MeshRenderer`, `Camera`).
2. `RenderCommand` list builder + canonical sort/batch in Domain A.
3. Forward pipeline: surface config, depth attachment, clear, mesh draw.
4. Mesh/Vertex upload + `Material`/`MaterialTemplate` bind-group wiring.
5. Camera view/projection from fixed camera values (quantize at boundary).
6. Sample WGSL diffuse shader + pipeline caching.
7. Hot-reload aware material/pipeline re-bake (from asset pipeline).
8. (Aspirational) deferred G-buffer + lighting pass, gated behind a feature and
   keeping the forward path default.
