---
spec: "40-post-processing"
phase: "Phase 5: VFX & Rendering"
status: "draft"
author: "OpenEngine AI"
created: "2026-09-03"
depends_on:
  - "00-ecs-architecture"
  - "04-render-pipeline"
  - "16-serialization"
  - "21-primitive-components"
  - "22-edit-vs-play"
  - "23-undo-redo"
  - "24-editor-viewport"
---
# 40 - Post-Processing Stack

## Overview

The post-processing stack is the GPU pass(es) applied to the final rendered
image after the forward lighting pass of spec 04 — **before** the editor overlay
and egui are composited (spec 24). It implements the familiar suite of effects —
**bloom, depth of field, motion blur, color grading, tonemapping (ACES / filmic /
Reinhard), vignette, chromatic aberration, film grain**, and **auto-exposure** —
executed as an **ordered effect chain** against a single render target, with
**configurable quality tiers**.

Post-processing is **entirely Domain A / GPU**. It never runs in Domain B and
never feeds gameplay math. Its `f32` shader work is presentation by definition
(AGENTS.md § 1, § 3). However the *world-facing* description of the stack is
authored ECS state: **`PostProcessVolume`** (ComponentId 22) and
**`PostProcessSettings`** (ComponentId 23) components placed in the scene. The
one determinism obligation this spec keeps is **active-volume selection**: given
the camera and all volumes in the world, *which volumes apply and in what
order* is a pure, reproducible function of fixed inputs (positions, priorities,
weights). Volumes therefore carry **fixed-point** geometry and scalar fields;
only the resulting GPU effect constants are quantized to `f32` at the shader
boundary.

## Core Concepts

### Global + local volumes

Two kinds of volume decide what post settings apply:

* **Global volume** (`is_global`): applies scene-wide (sky/day-night grading,
  baseline exposure). There is one *default* global settings set (the scene
  root, or an explicit global `PostProcessSettings` entity). A scene may have at
  most one authoritative global stack; multiple globals merge by priority.
* **Local volume**: a bounded region (sphere by `radius`, or box by
  `half_extents`) with a **blend radius** — a feather over which the volume's
  contribution fades from `blend_weight` at the inner core down to `0` at the
  region boundary. Local volumes are classic "zone" volumes (caustics under
  water, a desaturated memory, heat haze in a doorway).

### Deterministic active-volume selection

Each frame the *selector* gathers the camera's enclosing volumes and returns an
ordered, weighted contribution list. Selection is **pure** (no wall clock, no
iteration-order dependence — volumes are walked by sorted `Entity` / id order)
and therefore headless-testable and reproducible for a given scene + camera:

```
  camera position (fixed)  +  sorted volume columns
        │  for each volume: inside(camera) ?
        │    global => always contributes (weight from priority)
        │    local  => distance to region, blend by blend_radius
        ▼
  active list, order = (global base) then locals by (priority desc,
  then fixed-point distance, then entity order for ties)
```

Weighted combination of scalar/color settings is deterministic fixed-point
interpolation. The single "which volume wins" decision for non-blendable flags
(tonemapper *mode*, chain *order*) resolves by priority, with the fixed
tie-break order guaranteeing reproducibility.

### Ordered effect chain

Effects are applied in a fixed order so results are stable and testable:

```
LDR scene → (auto-exposure meter) → color grading (pre) → bloom
  → depth of field → motion blur → chromatic aberration → vignette
  → grain → tonemapping (tone-map) → grading (post, gamut/contrast) → sRGB out
```

The chain is defined once; individual effects enable/disable per active
settings and are skipped when disabled, preserving the canonical order for those
remaining. Quality tier (below) may collapse passes (e.g. cheap bloom downsample
count), never reorder them.

### Tonemapping modes

The tail of the chain converts HDR → display range:

* **ACES** — reference-grade filmic look (default for play).
* **Filmic** — Uncharted-2-style S-curve.
* **Reinhard** — cheapest, simple `x/(1+x)`.

The mode is a discriminant enum (`#[repr(u8)]`) selected deterministically by
the winning global volume. Editor preview (spec 24 view modes) can force a mode
for inspection but never writes it back into the world.

### Auto-exposure

An **eye-adaptation** mode integrates log2-luminance over the scene to drive a
global exposure multiplier with configurable min/max EV and adaptation speed.
Auto-exposure *sampling* is GPU; the decision values it feeds are only used for
rendering. When disabled, a fixed `manual_exposure` EV applies. Because exposure
affects only presentation, it is exempt from the determinism law — but it never
writes ECS columns.

### Quality tiers

A per-project/per-platform tier picks shader pass densities and resolutions:

| Tier        | bloom downsamples | DoF   | MSAA-inv | motion blur |
|-------------|-------------------|-------|----------|-------------|
| **Low**     | 2× half-res       | bokeh-approx | none        | none        |
| **Medium**  | 3× half/quarter   | 16 samples   | cheap       | 4 taps      |
| **High**    | 5× half..1/16     | 32 samples   | good        | 8 taps      |

Tier is a Domain-A render config, not gameplay data; it never changes the
active-volume selection (selection is tier-independent and deterministic).

## Key Rust Types

```rust
//! Domain B / shared Pod component crate (spec 21) — world-facing types. All
//! numeric authored values are fixed-point I16F16; f32 only at the GPU boundary.

use openengine_math::I16F16;
use contracts::Entity;

/// Where a post volume lives and how far it reaches. ComponentId 22.
#[repr(C)]
#[derive(Clone, Copy, bytemuck::Pod, bytemuck::Zeroable,
         serde::Serialize, serde::Deserialize, Debug)]
pub struct PostProcessVolume {
    pub shape: VolumeShape,      // u8: Global(ignored bounds)=0, Sphere=1, Box=2
    pub is_global: u8,           // 1 => applies scene-wide
    pub enabled: u8,             // master on/off
    pub _pad: u8,
    pub radius: I16F16,          // sphere radius / box half-extent (x)
    pub half_extents: [I16F16; 3], // box half extents (x,y,z); sphere ignores
    pub blend_radius: I16F16,    // feather width from core(1.0) to edge(0.0)
    pub blend_weight: I16F16,    // 0..1 global strength multiplier for this vol
    pub priority: I16F16,        // higher wins for non-blendable settings
    pub settings: SettingsRef,   // which settings apply (see below)
}

/// What settings a volume uses: its own attached PostProcessSettings column,
/// a shared settings-holder entity, or a shared asset preset token.
#[repr(C)]
#[derive(Clone, Copy, bytemuck::Pod, bytemuck::Zeroable,
         serde::Serialize, serde::Deserialize, Debug)]
pub struct SettingsRef {
    pub mode: u8,                // 0=inline(self),1=entity,2=asset
    pub _pad: [u8; 3],
    pub holder: Entity,          // settings-holder entity when mode==1
    pub asset: crate::AssetRef,  // shared preset token when mode==2
}
```

```rust
/// The full, authored parameter bundle. ComponentId 23. Effect blocks are each
/// gated by an `*_enabled` flag; disabled blocks contribute nothing.
#[repr(C)]
#[derive(Clone, Copy, bytemuck::Pod, bytemuck::Zeroable,
         serde::Serialize, serde::Deserialize, Debug)]
pub struct PostProcessSettings {
    // tonemapper + base
    pub tonemapper: ToneMapper,  // u8: Aces/Filmic/Reinhard
    pub quality_hint: u8,        // u8: Low/Medium/High (Lowest author wins)
    pub _pad: [u8; 2],
    pub exposure_enabled: u8,    pub exposure_pad: [u8; 3],
    pub manual_exposure_ev: I16F16, // used when auto-exposure disabled
    pub auto_exposure_enabled: u8,  pub auto_pad: [u8; 3],
    pub target_luminance: I16F16,
    pub ev_min: I16F16,
    pub ev_max: I16F16,
    pub adapt_speed: I16F16,     // per-tick adaptation rate
    // color grading
    pub grading_enabled: u8,     pub grading_pad: [u8; 3],
    pub saturation: I16F16,      // 1.0 = neutral
    pub contrast: I16F16,        // 1.0 = neutral
    pub gamma: I16F16,           // 1.0 = neutral
    pub gain: I16F16,            // overall brightness scale
    pub tint: [I16F16; 3],       // per-channel color tint
    // bloom
    pub bloom_enabled: u8,       pub bloom_pad: [u8; 3],
    pub bloom_intensity: I16F16, // 0..N
    pub bloom_threshold: I16F16, // luminance threshold
    pub bloom_radius: I16F16,    // blur spread (downsample chain length)
    // depth of field
    pub dof_enabled: u8,         pub dof_pad: [u8; 3],
    pub dof_focus_dist: I16F16,  // distance in focus
    pub dof_focus_range: I16F16, // in-focus half-range
    pub dof_strength: I16F16,    // bokeh intensity
    // motion blur
    pub motion_blur_enabled: u8, pub mb_pad: [u8; 3],
    pub motion_blur_strength: I16F16,
    // vignette
    pub vignette_enabled: u8,    pub vignette_pad: [u8; 3],
    pub vignette_intensity: I16F16, // 0..1
    pub vignette_radius: I16F16,     // inner radius where fade begins
    // chromatic aberration
    pub chroma_enabled: u8,      pub chroma_pad: [u8; 3],
    pub chroma_strength: I16F16, // 0..N px offset scale
    // grain
    pub grain_enabled: u8,       pub grain_pad: [u8; 3],
    pub grain_intensity: I16F16, // 0..1
    pub grain_seed: u32,         // deterministic grain pattern seed (Domain A)
}
```

```rust
//! Domain A — selector + GPU chain. crates/render.
/// Result of deterministic active-volume selection for one camera frame.
pub struct ActivePostStack {
    pub volumes: Vec<SelectedVolume>,  // ordered by (global base, then priority)
}
pub struct SelectedVolume {
    pub entity: Entity,
    pub weight: f32,                   // quantized from fixed blend_weight*feather
    pub settings: f32 /*HostView*/,    // quantized effect constants for shader
}

/// Pure selector (headless-testable, deterministic given sorted columns).
pub fn select_active_volumes(
    camera_pos_fixed: [I16F16; 3],
    volumes_sorted: &[PostProcessVolumeRow],
) -> ActivePostStack;
```

## Components

Registered in the stable ComponentId window **20–29** (spec 21 policy). This
spec owns **22** and **23**. Both are Pod + Zeroable + serde and round-trip
through the spec-16 codec.

| `ComponentId` | Name                 | `size_of` (target) | Domain use (owner)                          |
|---------------|----------------------|--------------------|----------------------------------------------|
| **22**        | `PostProcessVolume`  | 56 B               | Region/priority/weight controlling apply (spec 40). |
| **23**        | `PostProcessSettings`| 160 B              | Effect parameter bundle referenced by volumes (spec 40). |

A typical scene is: a **global** volume (or a scene-root settings entity) that
defines the default stack, plus zero or more **local** `(Transform,
PostProcessVolume, PostProcessSettings)` entities placed around gameplay zones.
The editor places these with spec-23 `Command`s; selecting/blending previews
uses the spec-24 viewport.

## Constraints

- **Domain split.** All post effects are Domain A + GPU; Domain B never runs
  them and never reads effect outputs. `f32` is correct here (presentation) and
  never feeds gameplay columns (AGENTS.md § 1, § 3).
- **World-facing data is fixed-point Pod.** `PostProcessVolume`/`PostProcessSettings`
  authored scalars are `I16F16` (with documented ranges); they are quantized to
  `f32` only when building shader uniform blocks. `f32` never appears inside the
  components (spec 21 discipline).
- **Deterministic selection.** Active-volume selection is a pure function of
  (camera position, sorted volume columns): fixed compare/interpolate only, no
  wall clock, no `HashMap` iteration, tier-independent. Same scene + camera ⇒
  same active list on all targets (headless-verified).
- **Ordered chain.** Effects run in one canonical order; tiers collapse passes
  but never reorder them. The chain is authored once.
- **No writes back.** Auto-exposure meter, grain, and any adaptive value are GPU
  presentation state only — they never write ECS columns, so a frame's selection
  stays reproducible.
- **Edit vs Play.** Volumes are authored in the edit world (spec 22). Play deep-
  clones them; the *selector* runs in both but only the play render consumes it
  for gameplay output. The editor viewport may force view-mode grades (spec 24)
  for preview without mutating columns.
- **Undoable.** Volume/settings edits are spec-23 `ModifyComponentCommand`s (old/
  new raw fixed bytes) applied to the edit world; no direct ECS writes from UI.
- **Serialization.** Both components round-trip `postcard` (spec 16); a
  `SettingsRef` pointing at a shared preset asset stays a logical token (spec 02).
- **Portability.** `x86_64-linux` + `aarch64-linux`; logic tests require no GPU.

## Performance Targets

- Active-volume selection for up to 64 volumes: **< 0.1 ms** (pure, no alloc in
  hot path; reuse a sorted scratch).
- Full High-tier chain per frame (bloom 5× + DoF 32 + motion 8 + chroma +
  vignette + grain + tonemap): **≤ ~3 ms** on a mid GPU at 1080p.
- Low-tier fallback (mobile / editor preview): **≤ ~1 ms**.
- Uniform rebuild per active stack: **< 0.05 ms** (only rebuilt when the active
  stack or a volume weight changes).
- No post cost added to logic/fixed update (render-only, Domain A).

## Testing Strategy

All headless (no GPU) unless marked *GPU*:
- **Determinism of selection.** Given a fixed camera and a seeded set of
  global/local volumes, run the selector 3× on two targets; assert an identical
  ordered active list and identical computed weights.
- **Global/local semantics.** Camera inside a local sphere/box with a
  `blend_radius` fades the volume 1.0→0.0 across the feather; outside a local
  volume yields no contribution; a global volume always contributes.
- **Priority + tie-break.** Two globals differing in `priority` resolve to the
  higher one for the tonemapper *mode*; equal priority resolves by the fixed
  entity-order tie-break (no iteration-order nondeterminism).
- **Weighted merge.** Two overlapping locals interpolate shared scalar/color
  settings deterministically by their blended weights.
- **Chain order & tiering.** Assert the canonical effect order is preserved and
  that each quality tier yields a valid (possibly collapsed) pass list; disabled
  blocks drop out without reordering survivors.
- **Tonemapper math (golden).** ACES/filmic/Reinhard on a set of HDR inputs
  produce expected LDR outputs (pure shader-free reference function or golden
  CPU mirrors) — deterministic reference values.
- **Editor command path.** Modify a volume/settings field via spec-23 command,
  undo, assert bit-identical edit world.
- **Edit/Play.** Author volumes in edit, clone to play (spec 22), assert the
  selector reproduces the same active list from the cloned columns.
- ***GPU* smoke (device only, not logic tests):** render a High-tier chain with
  no validation errors; assert the presented frame changes with each effect
  enabled.

## Dependencies

- `crates/render` / `crates/core` (Domain A) — post passes, render targets,
  shader chain on top of spec 04's forward output.
- `crates/ecs` (volume/settings archetypes), `crates/editor` (commands +
  viewport preview, specs 23/24).
- `openengine-math` (`I16F16`, `quantize_to_f32`), `bytemuck`, `serde`,
  `postcard`.
- Pod mirror of the two components + spec 21 `AssetRef`/`Transform`.
- Consumes spec 04's rendered scene; consumed visually by spec 24's viewport.

## Next Steps

1. Register `PostProcessVolume` (22) + `PostProcessSettings` (23) Pod components
   and the `C_POST_PROCESS_VOLUME`/`C_POST_PROCESS_SETTINGS` bindings.
2. Implement the pure active-volume selector + unit/determinism tests.
3. Build the ordered effect chain skeleton + render-target plumbing in Domain A.
4. Land per-effect shaders behind quality tiers (Low first, then Medium/High).
5. Quantize active-stack settings → uniform block; wire to the spec 04 output.
6. Add editor placement/`ModifyComponentCommand` (spec 23) + viewport preview
   controls (spec 24).
