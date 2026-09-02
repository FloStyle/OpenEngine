---
spec: "41-advanced-lighting"
phase: "Phase 5: VFX & Rendering"
status: "draft"
author: "OpenEngine AI"
created: "2026-09-03"
depends_on:
  - "00-ecs-architecture"
  - "02-asset-pipeline"
  - "04-render-pipeline"
  - "16-serialization"
  - "21-primitive-components"
  - "22-edit-vs-play"
  - "23-undo-redo"
  - "24-editor-viewport"
  - "40-post-processing"
---
# 41 - Advanced Lighting & Global Illumination

## Overview

This spec extends the basic forward lighting of spec 04 into a full advanced
lighting system: **shadow mapping** (cascaded, PCF-filtered, soft), **reflection
probes** (cube maps) plus **screen-space reflections (SSR)**, **ambient
occlusion (SSAO / HBAO+)**, and **global illumination** split between **baked
lightmaps for static geometry** and **light/reflection probes for dynamic
actors**. Light source types cover **directional, point, spot, and area**
(reusing `Light`, ComponentId 8), and an **offline, deterministic bake pipeline**
produces the lightmap and static-probe data from a scene.

Lighting math on the GPU is **Domain A / `f32`** (presentation, AGENTS.md § 1).
The *world-facing* description of lighting — light components, probe bounds,
lightmap assignments, bake seeds — is authored **fixed-point Pod** ECS state so
it is editable (spec 23), serializable (spec 16), and reproducible. The bake is
**offline and deterministic from a scene** so the same authored scene + bake
seed produces the same lightmaps on any machine.

## Core Concepts

### Light types (reusing `Light` = ComponentId 8)

`Light` (spec 21) already carries `LightKind` (Directional/Point/Spot),
`intensity`, and `color`. Advanced lighting **reuses** it for the first three
types and adds **Area**:

* **Directional** — parallel rays; the sun/moon. Drives the **cascaded shadow
  map** (CSM).
* **Point** — omni with inverse-square falloff; one 6-face shadow cubemap
  (optional).
* **Spot** — cone with falloff; a single shadow texture.
* **Area** — a soft emitter (rectangle/disk) for soft shadows / GI bounce hints.
  Adding `Area` is a **new `LightKind` variant** and so is an ABI change: it
  requires an `ARCH_VERSION` bump + a matching `docs/abi/` update + all consumers
  rebuilt in the same commit (AGENTS.md § 7). `Light`'s ComponentId stays **8**
  and its `size_of` layout is unchanged apart from the new discriminant value.

### Shadow mapping

* **Cascaded (CSM)** for directional light: a fixed number of frustum-split
  cascades (e.g. 3–4) each rendered to its own depth map, selected in the vertex/
  fragment stage by view depth. Cascade split boundaries are **pure functions**
  of the camera frustum + far plane so they are stable and testable.
* **PCF filtering** softens shadow edges (N×N taps / Poisson-disc, deterministic
  tap ordering); **soft shadows** via variable-width PCF on the light-size/contact
  term. Shadow map resolution, cascade count, and PCF tap counts are per-light
  authored fields and quality-tier-aware.
* **Self-shadow / bias** parameters (depth bias, normal bias) are fixed authored
  scalars quantized to `f32` at the shader; bias is tuned to avoid acne while
  staying reproducible.

### Reflection probes + SSR

* **ReflectionProbe** (ComponentId 24): a cube map capture point with an
  influence region (sphere/box). Probes may be **baked offline** (cube map stored
  as a logical cubemap `AssetRef`) or **realtime** (captured on demand by a
  Domain-A render). Per-`MeshRenderer` visibility of a probe uses the sorted,
  deterministic influence selection described below.
* **Screen-space reflections (SSR)** add specular detail by ray-marching the
  depth buffer; SSR blends into the probe cube map for occluded/non-screen hits,
  so probes remain the fallback and SSR is the cheap high-frequency addition.
* Probe **blending**: an object samples probes in sorted influence order
  (deterministic, like the post-volume selector of spec 40) and interpolates
  their cube maps by weight/parallax.

### Ambient occlusion

* **SSAO** — screen-space hemisphere sample occlusion around each pixel normal
  (deterministic sample kernel from a fixed seed).
* **HBAO+** — higher-quality horizon-based AO using depth/normal buffers; used at
  High tier, SSAO at Low/Medium. AO output is applied as a multiplicative
  ambient/indirect factor (never a direct-lighting factor) to stay physically
  plausible.

### Global illumination: lightmaps (static) + probes (dynamic)

The scene is split at authoring time into **static** and **dynamic** geometry:

* **Static geometry** (terrain, architecture, props flagged lightmap-static) is
  lit by **baked lightmaps**: the bake rasterizes direct + first-bounce indirect
  radiance per texel into an **atlas** texture. Each static `MeshRenderer` gets a
  `LightmapSettings` (ComponentId 26) mapping its triangles into the atlas
  (UV2 channel + atlas tile). The GPU shades static fragments from their baked
  lightmap + shadow + AO, with only dynamic-light contributions added live.
* **Dynamic actors** (characters, moving props) carry no lightmap; they are lit by
  live direct lights plus **light probes** and **reflection probes**. `LightProbe`
  (ComponentId 25) stores spherical-harmonic irradiance (or a reference to a baked
  irradiance asset) at a point so a dynamic object can read ambient/indirect from
  the nearest few probes.
* This split is what lets GI be both offline-cheap (static, baked once) and
  live-correct (dynamic, probe-interpolated) — the standard hybrid-GI pattern.

### Bake pipeline (offline, deterministic from scene)

The bake runs **offline** (a command/tool, not per-frame) over the **edit world**
(spec 22) and is deterministic:

1. **Static partition** — collect static `MeshRenderer`+`LightmapSettings`
   entities, terrain (spec 42), and lights/probes, in sorted canonical order.
2. **UV2/atlas packing** — build the lightmap atlas layout deterministically
   (sorted by mesh size then entity order) so two bakes of the same scene pack
   identically.
3. **Radiance solve** — a fixed-point path/radiance estimate (fixed sample counts
   and iteration order, seeded) writes per-texel radiance for direct + indirect.
   Bounce bakes into a second atlas iteration; convergence is by **fixed
   iteration count**, never a host float tolerance (spec 13 discipline).
4. **Probe bake** — irradiance SH + cube maps for static probes from the same
   solve.
5. **Output** — bake results are *assets* (lightmap atlas texture(s), cubemap /
   irradiance assets) referenced by logical `AssetRef`s on the components; the
   scene itself is unchanged except those refs.

Deterministic from the same `(scene, bake_seed)` ⇒ identical atlas bytes on any
target — so a committed bake is reproducible and diffable.

### Deterministic probe / shadow selection

Like spec 40's volume selector, **which probes influence a point** and **which
lights cast shadows** are pure functions over sorted columns (priority/weight/
distance + fixed tie-break), headless-testable and independent of GPU state.

## Key Rust Types

```rust
//! World-facing Pod components (shared component crate, spec 21). All authored
//! scalars fixed-point; f32 only at the Domain-A/GPU emission boundary.

use openengine_math::I16F16;
use contracts::Entity;

/// Reuses spec 21 `Light`. ComponentId 8. `LightKind` gains `Area = 3` via an
/// ABI v3 extension (layout otherwise unchanged).
#[repr(u8)]
pub enum LightKind { Directional = 0, Point = 1, Spot = 2, Area = 3 }

/// A cube-map reflection capture. ComponentId 24.
#[repr(C)]
#[derive(Clone, Copy, bytemuck::Pod, bytemuck::Zeroable,
         serde::Serialize, serde::Deserialize, Debug)]
pub struct ReflectionProbe {
    pub shape: VolumeShape,       // u8: Sphere/Box (parallax influence)
    pub mode: ProbeMode,          // u8: Baked=0, Realtime=1
    pub enabled: u8,
    pub _pad: u8,
    pub influence_radius: I16F16, // sphere influence (or box radius axis)
    pub half_extents: [I16F16; 3],// box influence extents
    pub intensity: I16F16,        // cube-map contribution multiplier
    pub priority: I16F16,         // higher wins for overlapping influences
    pub resolution: u16,          // cube face resolution (bake/realtime)
    pub cube_map: AssetRef,       // baked cubemap token (Baked); NONE realtime
}
#[repr(u8)]
#[derive(Clone, Copy, bytemuck::Pod, bytemuck::Zeroable,
         serde::Serialize, serde::Deserialize, Debug)]
pub enum VolumeShape { Sphere = 0, Box = 1 }
#[repr(u8)]
#[derive(Clone, Copy, bytemuck::Pod, bytemuck::Zeroable,
         serde::Serialize, serde::Deserialize, Debug)]
pub enum ProbeMode { Baked = 0, Realtime = 1 }

/// An SH irradiance probe for dynamic actors. ComponentId 25.
#[repr(C)]
#[derive(Clone, Copy, bytemuck::Pod, bytemuck::Zeroable,
         serde::Serialize, serde::Deserialize, Debug)]
pub struct LightProbe {
    pub influence_radius: I16F16,
    pub intensity: I16F16,
    pub priority: I16F16,
    pub irradiance: AssetRef,     // baked SH/irradiance asset (or NONE realtime)
    pub sh_resolution: u16,       // SH order hint (real-time captures)
    pub enabled: u8,
    pub _pad: u8,
}

/// Lightmap mapping for one static MeshRenderer. ComponentId 26.
#[repr(C)]
#[derive(Clone, Copy, bytemuck::Pod, bytemuck::Zeroable,
         serde::Serialize, serde::Deserialize, Debug)]
pub struct LightmapSettings {
    pub static_geo: u8,           // 1 => baked into the atlas, no live indirect
    pub enabled: u8,
    pub uv_channel: u8,           // 1/2 = which UV set is the lightmap channel
    pub _pad: u8,
    pub atlas_tile: u32,          // index into the scene lightmap atlas layout
    pub resolution_scale: I16F16, // relative texel density (author hint)
    pub atlas: AssetRef,          // logical lightmap-atlas texture token (baked)
}
```

```rust
//! Domain A — GPU shadow / probe / lightmap shading + offline bake driver.
pub struct CascadeShadowMap { pub cascades: Vec<ShadowCascade> }   // 3-4 splits
pub struct ShadowCascade { pub view_proj: glam::Mat4, pub map: wgpu::TextureView }
pub enum ShadowFilter { None, Pcf { taps: u32 }, SoftPcf { taps: u32 } }

/// Offline bake seed — deterministic from (scene_components_sorted, bake_seed).
pub struct BakeResult {
    pub lightmap_atlas: AssetHandle<Texture>,
    pub cube_maps: Vec<AssetHandle<Cubemap>>,
    pub irradiance: Vec<AssetHandle<Texture>>,  // or packed SH
}
```

## Components

Registered in the stable ComponentId window **20–29** (spec 21 policy). This
spec owns **24**, **25**, **26** and **reuses** `Light` = **8**.

| `ComponentId` | Name                | `size_of` (target) | Domain use (owner)                          |
|---------------|---------------------|--------------------|----------------------------------------------|
| 8             | `Light`             | 20 B               | Directional/Point/Spot/Area source (reused, spec 21/41). |
| **24**        | `ReflectionProbe`   | 96 B               | Cube-map reflection capture + influence (spec 41). |
| **25**        | `LightProbe`        | 32 B               | SH irradiance probe for dynamic actors (spec 41). |
| **26**        | `LightmapSettings`  | 48 B               | Per-static-mesh lightmap atlas mapping (spec 41). |

No extra registered components are needed; anything further (e.g. a bake-tool
only state) stays in the 25–29 window and would be recorded here before use, per
spec 21 policy.

## Constraints

- **Domain split.** All shadow/SSR/AO/lightmap/indirect shading is Domain A GPU
  (`f32` correct for presentation). Domain B never runs it and never reads
  lighting *output*; it may read authored light/probe columns only to drive
  deterministic gameplay (e.g. a light's `enabled` flag for a day/night event).
- **Reuse `Light` (8).** Advanced lighting adds area light support as a new
  `LightKind::Area` discriminant on the existing component — ComponentId 8 and
  byte layout stay put; the added variant is an ABI v3 change (docs/abi + bump).
- **Fixed-point authored state.** Probe bounds, priorities, intensities, shadow
  bias, lightmap resolution hints are `I16F16` Pod; quantized to `f32` only when
  building GPU uniform/texture state. No `f32` in components (spec 21).
- **Deterministic selections.** Probe-influence resolution, cascade splits, AO
  sample kernels, shadow-caster culling, and bake traversal all use sorted
  columns + fixed iteration/sample counts + seeded PRNG — no `HashMap`
  iteration, no wall clock, no host-float tolerance (spec 13 discipline).
- **Bake is offline and deterministic from scene.** It runs over the edit world
  (spec 22) from `(sorted scene, bake_seed)`; same inputs ⇒ identical atlas /
  probe assets on any target. Bake never runs in the fixed tick.
- **Edit vs Play.** Lights/probes/lightmap assignments are authored in the edit
  world via spec-23 `Command`s. Play deep-clones them and renders the *baked*
  assets that the components reference; a bake result is itself a normal asset
  edit (undoable) on the edit world.
- **Static vs dynamic is explicit.** A static mesh without `LightmapSettings`
  falls back to live lighting only (correct but costlier); enabling lightmap
  requires the bake to have packed it. Dynamic actors never take a lightmap.
- **No f32 feedback.** SSR/AO/auto values never write ECS columns (presentation
  only), preserving reproducible authored state.
- **Portability.** Logical `AssetRef`s (no absolute paths), `x86_64-linux` +
  `aarch64-linux`; logic tests require no GPU.

## Performance Targets

- CSM render of 3–4 cascades + PCF: **≤ ~2 ms** (shadow passes) at 60 Hz on a
  mid GPU; cascade split selection per fragment negligible.
- SSAO: **≤ ~1 ms** (Low/Medium); HBAO+: **≤ ~2 ms** (High) at 1080p.
- SSR + reflection probe sample: **≤ ~1.5 ms** at High tier.
- Probe/light selection per entity: **< 0.1 ms** for a scene with 32 probes and
  128 lights (deterministic, no alloc in hot path).
- Lightmap shading for 50k static fragments: negligible incremental cost (texture
  fetch + one dynamic-light term).
- **Offline bake:** a 512²×N static-mesh scene (fixed iterations) completes
  **< ~60 s** headlessly on a CPU reference; GPU-accelerated bake is later and
  must reproduce the CPU fixture.
- Logic/fixed update: zero added cost (all lighting is Domain A).

## Testing Strategy

All headless (no GPU) unless marked *GPU*:
- **Determinism.** Fix a scene + bake seed; run shadow-cascade split, AO kernel
  gen, probe selection, and the bake solve 3× on two targets and assert identical
  outputs (cascade boundaries, sorted probe lists, atlas bytes).
- **Cascade splits.** Given a camera frustum + far, assert the expected cascade
  near/far boundaries (golden fixed values); they are independent of GPU state.
- **Probe influence.** A point in a sphere/box influence picks the expected
  sorted probe set; overlap resolves by `priority` then fixed entity tie-break;
  outside all probes yields none (fallback to a scene ambient).
- **Lightmap assignment.** A static mesh's `LightmapSettings` maps to the packed
  atlas tile; packing is reproducible and injective across a sorted mesh set.
- **Reuse of `Light`.** ComponentId 8 unchanged; `LightKind::Area` is a valid new
  discriminant with `size_of::<Light>() == 20` still enforced (layout test).
- **Bake determinism / fidelity.** Bake the same scene twice from the same seed;
  assert identical atlas assets. Bake direct + a fixed-iteration indirect pass;
  assert texels converge to a reference (fixed-iteration, not tolerance).
- **Editor command path.** Move/add a light, probe, or lightmap assignment via
  spec-23 commands; undo restores a bit-identical edit world. Bake is an undoable
  asset edit on the edit world.
- **Edit/Play.** Author lights/probes, bake, clone to play (spec 22); assert the
  play render consumes the same baked assets and the deterministic selectors
  reproduce the same active lists from the clone.
- ***GPU* smoke (device only, not logic tests):** one CSM + PCF + SSAO + lightmap
  frame with no validation errors; assert toggling area-light / SSR changes the
  presented output.

## Dependencies

- `crates/render` / `crates/core` (Domain A) — CSM/PCF, SSR, SSAO/HBAO+, lightmap
  + probe shading, on top of spec 04's forward output; post chain from spec 40.
- `crates/ecs` (light/probe/lightmap archetypes), `crates/editor` (commands +
  viewport, specs 23/24), `crates/asset` (cubemap/atlas/irradiance assets,
  spec 02), `crates/serial` (bake asset codec, spec 16).
- `contracts` (`Entity`, `ComponentId`, `ColumnWrite`, `WorldDelta`),
  `openengine-math` (`I16F16`, seeded hashing / fixed solve helpers), `bytemuck`,
  `serde`, `postcard`.
- Pod mirrors: spec 21 `Light`/`AssetRef`/`Transform`; terrain (spec 42) as a
  static-lightmap consumer.
- *ABI note:* adding `LightKind::Area` bumps `ARCH_VERSION` with a `docs/abi/`
  update; all consumers rebuild in the same commit (AGENTS.md § 7).

## Next Steps

1. Register `ReflectionProbe` (24), `LightProbe` (25), `LightmapSettings` (26) +
   bindings (`C_REFLECTION_PROBE`, `C_LIGHT_PROBE`, `C_LIGHTMAP_SETTINGS`).
2. Extend `LightKind` with `Area` under an `ARCH_VERSION` bump + `docs/abi/`.
3. Implement deterministic cascade splits, probe selection, and AO kernels;
   add headless determinism tests.
4. Land CSM+PCF+soft shadow shading; then SSR; then SSAO/HBAO+ (by tier).
5. Implement static/dynamic lightmap shading + live dynamic-light term.
6. Build the offline deterministic bake pipeline (atlas pack → fixed-iteration
   solve → probe bake → assets) with CPU reference + reproducibility tests.
7. Wire spec-23 commands for authoring and the spec-24 viewport lighting preview.
