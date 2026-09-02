---
spec: "02-asset-pipeline"
phase: "Phase 5"
status: "design"
---

# Asset Pipeline

## Overview

Async, cached, hot-reloadable asset loading for **Domain A only**
(`openengine-core`). Assets are raw bytes + decoded engine resources that the
host owns, loads off the main thread, uploads to the GPU, and hands out as
type-safe handles. Game logic in Domain B is pure and does **no I/O** — it can
only *request* an asset by stable name through the ABI and receive back a handle
on a later tick.

Asset loading is a host concern precisely because it is full of side effects
(files, sockets, decoders, GPU uploads) that the Prime Directive forbids in
Domain B. This spec keeps every one of those side effects inside Domain A while
giving Domain B a tiny, deterministic vocabulary for naming things it needs.

## Design

### Asset kinds and loader backends

Each kind maps to one decoder crate and one engine resource type. A "kind" is
the unit of type-safety, caching, and decode:

| Kind | Source formats | Decoder | Engine resource |
|------|----------------|---------|-----------------|
| `Texture` | PNG, JPG | `image` | `wgpu::Texture` + `TextureView` + sampler params |
| `Mesh` | glTF, OBJ | `gltf` (+ `obj` fallback) | vertex/index `wgpu::Buffer`s + `VertexLayout` |
| `Shader` | WGSL | none (embedded/string) | `wgpu::ShaderModule` |
| `Audio` | WAV, OGG | `rodio` (decoded samples) | `AudioClip` (PCM samples + meta) |
| `Font` | TTF, OTF | `ab_glyph` | `ab_glyph::FontArc` |

Files are located under `OPENENGINE_ASSETS_PATH` (default `./assets/`, resolved
relative to the workspace root or `CARGO_MANIFEST_DIR` — never a hardcoded or
home path, per the Portability Rules).

### Type-safe handle

Domain A wants compile-time kind safety, so a handle is a transparent `u64` but
branded per kind:

```rust
/// Registry-global, monotonically-assigned asset id. The high bits may encode
/// the kind for cheap debugging; the value is opaque to callers.
#[repr(transparent)]
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub struct AssetHandle<T: AssetKind>(pub u64);
```

`AssetKind` is a sealed marker trait implemented for the five kinds above so
`AssetHandle<Texture>` is not `AssetHandle<Mesh>`. There is no `unsafe`, no
pointer: a handle is only ever a lookup key into the host registry. Handles are
**not** valid ABI values by themselves — Domain B receives a handle as a plain
`u64` in a `ResourceReady` command (see below), because Domain B never dereferences
host memory.

### AssetRegistry

```rust
pub struct AssetRegistry {
    textures: KindRegistry<Texture>,
    meshes:    KindRegistry<Mesh>,
    shaders:   KindRegistry<Shader>,
    audio:     KindRegistry<Audio>,
    fonts:     KindRegistry<Font>,
    pending:   HashMap<AssetName, InFlight>,   // name -> load being awaited
    next_id:   u64,
}
```

Each `KindRegistry<T>` owns its decoded resources in a `HashMap<AssetId, Rc<T>>`
or slot-map. Two separate namespaces are cached:

1. **By canonical name** — the stable string Domain B (and config) uses.
2. **By content hash** (`sha2::Sha256` of the decoded/packed bytes) — so two
   files with identical bytes resolve to one resident asset (de-duplication).

### Cache key = content hash, not timestamp

Timestamps are wall-clock and break reproducibility. The dedup key is the
SHA-256 of the file bytes (texture/audio/font) or the normalized packed payload
(mesh/scene). Logically identical assets stored under different paths collapse
to one GPU resource. Hot-reload *watches paths*, but it only ever triggers a
re-decode when the **hash** of the on-disk bytes changes — an editor touch that
changes nothing produces no re-upload.

```rust
pub struct CachedAsset<T: AssetKind> {
    pub id: AssetId,
    pub hash: [u8; 32],
    pub resource: Rc<T>,
}
```

### Async loaders

Loading never blocks the frame thread. A `tokio` runtime (single worker for
asset I/O, or the engine's existing async runtime) performs the blocking decode
off the main thread, then marshals the finished resource back:

```rust
pub async fn load_texture(&mut self, name: AssetName, path: PathBuf)
    -> AssetHandle<Texture> {
    // 1. Resolve -> if resident by name or content-hash, return cached handle.
    // 2. Spawn a blocking task: fs::read + image::load_from_memory + decode.
    // 3. On success, enqueue a GPU-upload; return handle.
    // 4. On failure, register a placeholder and log — never crash.
}
```

The main-thread side drains a bounded `mpsc` of `LoadResult`s once per frame
(the asset tick), so the winit/wgpu-thread never awaits I/O directly. The caller
gets a handle immediately (or the pending-handle), and the resource becomes
resident when its `LoadResult` is drained.

### ResourceRequest / ResourceReady handshake (Domain B)

Domain B cannot open files. Instead it emits a **request** and the host answers
with a **ready**. Both are deferred commands routed by the host.

Guest request — a `DeferredCommand`-shaped `ResourceRequest`:

```rust
pub enum ResourceRequestKind { Texture, Mesh, Shader, Audio, Font }

/// Named, kind-tagged request. Carries only a stable string + kind; no bytes.
#[derive(Clone, Debug, serde::Serialize, serde::Deserialize)]
pub struct ResourceRequest {
    pub request_id: u64,     // guest-chosen correlation token
    pub kind: ResourceRequestKind,
    pub name: AssetName,     // stable logical name, e.g. "tiles/grass"
}
```

Because Domain B compiles `#![no_std]` against `contracts`, these request/ready
types live in `contracts` and travel inside `WorldDelta.deferred` (the existing
`Emit`/`DeferredCommand` bus). Domain A peels requests off the delta each tick.

Host answer — a `ResourceReady` command the host posts back into the *next*
`StateView`:

```rust
/// Host -> guest answer. Guest treats `handle` as an opaque id it echoes back
/// into render commands; it never dereferences it.
pub struct ResourceReady {
    pub request_id: u64,
    pub kind: ResourceRequestKind,
    pub name: AssetName,
    pub handle: u64,          // opaque host handle, or 0 if placeholder
    pub available: bool,      // false => failed, placeholder served
}
```

Round trip is multi-tick by design:

1. Tick *n*: guest system returns `WorldDelta` containing `ResourceRequest`.
2. Host asset tick: resolve or kick off an async load for `name`.
3. Tick *n+k* (k ≥ 1): host includes a matching `ResourceReady` in the
   `StateView` for that guest.
4. Guest stores `handle` in a fixed-point/sorted component and later emits a
   `DeferredCommand::Render` naming it.

The guest never assumes *when* a resource lands — readiness is an explicit
signal, not a timing assumption. This keeps logic deterministic even though the
host's load latency is not.

### GPU upload path

GPU work is Domain A only. After a `Texture`/`Mesh` decodes, the resource bytes
are written into wgpu buffers via the queue:

```rust
queue.write_texture(
    wgpu::TexelCopyTextureInfo {
        texture: &texture,
        mip_level: 0,
        origin: wgpu::Origin3d::ZERO,
        aspect: wgpu::TextureAspect::All,
    },
    &rgba_bytes,                 // tightly packed row 0
    wgpu::TexelCopyBufferLayout { offset: 0, bytes_per_row, rows_per_image: None },
    extent,
);
```

Meshes upload vertex/index buffers the same way with `queue.write_buffer`. All
uploads are queued on the single submission used that frame (see
`04-render-pipeline.md`); the upload does not introduce a second command queue.

### Hot reload (DEV-ONLY)

A `notify` watcher watches `OPENENGINE_ASSETS_PATH` recursively **only in dev
builds** (`cfg!(debug_assertions)` or an explicit `dev = true` runtime flag).
Production/release runs never start a watcher — no filesystem polling in a
shipped game. On a change event for a path that maps to a resident asset, the
pipeline re-hashes; if the hash moved it re-decodes off-thread and, on the next
asset tick, replaces the resident resource **by handle** (same `AssetId`, new
underlying resource). Live entities that already hold the handle automatically
see the new content next frame. Shaders hot-reload by recompiling the WGSL and
replacing the `ShaderModule` + re-baking dependent pipelines on a reload callback.

### Placeholder on failure (never crash)

A failed load (missing file, bad bytes, unsupported codec) must never bring the
engine down or poison a frame. The pipeline:

1. Logs the failure with the canonical name + underlying error.
2. Registers a **placeholder** resource for that name: a 1×1 white/error texture,
   a unit quad mesh, an identity font, a silent clip, an empty shader module.
3. Serves the placeholder under the requested name and reports
   `available: false` in the `ResourceReady`.
4. Clears the failed state so a later (or hot-reload) attempt can succeed.

Consequently the renderer and Domain B always have *something* drawable and
never need a "maybe absent" path for a requested asset.

## Key types

```rust
pub struct AssetName(pub String);                 // canonical, stable
pub struct AssetId(pub u64);                      // registry index (== handle inner)
pub struct Texture {
    pub texture: wgpu::Texture,
    pub view: wgpu::TextureView,
    pub size: (u32, u32),
    pub format: wgpu::TextureFormat,
    pub sampler: wgpu::Sampler,                   // built from TextureSamplerParams
}
pub struct Mesh {
    pub vertex_buffer: wgpu::Buffer,
    pub index_buffer: wgpu::Buffer,
    pub vertex_count: u32,
    pub index_count: u32,
    pub layout: VertexLayout,                      // see 04-render-pipeline.md
}
pub struct AudioClip {
    pub sample_rate: u32,
    pub channels: u16,
    pub samples: Vec<f32>,                         // Domain A only
}
```

## Constraints

- I/O, decode, GPU upload and the `notify` watcher live strictly in Domain A.
- Domain B only emits `ResourceRequest` by name/kind and consumes opaque
  handles from `ResourceReady`; it never touches bytes, paths, or devices.
- No timestamps as cache keys; content hash (SHA-256) only.
- No hardcoded paths: everything under `OPENENGINE_ASSETS_PATH` relative to the
  workspace root / `CARGO_MANIFEST_DIR`.
- `f32` decoded sample data is Domain A only; it never enters Domain B math.
- Hot reload gated to dev; absent in release.
- Failure ⇒ placeholder, never a panic or crash.

## Performance targets

- Async decode keeps the frame thread under its 16.67 ms budget regardless of
  asset load in flight.
- Resident-by-name lookup and content-hash dedup: < 1 µs.
- Hot-reload swap of an already-uploaded mesh/texture: no visible hitch beyond
  the natural re-upload cost of that resource.

## Testing strategy

- Unit: registry insert/lookup, content-hash dedup (two equal-byte files → one
  resource), placeholder-on-failure.
- Unit: `ResourceRequest`/`ResourceReady` round trip through a `WorldDelta`
  (encode/decode via postcard, multi-tick handshake logic).
- Integration (Domain A, no GPU for decode-only path where possible): load a
  fixture PNG/WAV/TTF, assert the resident registry and handle stability.
- GPU smoke (requires device): upload a texture + mesh, assert queue write.
- Hot-reload: touch a fixture with identical bytes → no re-upload; change bytes
  → re-decode; verify release build starts no watcher.
- Determinism: identical `ResourceRequest` sequence from a pure guest yields an
  identical `ResourceReady` schedule when load latencies are mocked constant.

## Dependencies

Domain A only: `tokio`, `image`, `gltf` (+ `obj`), `rodio`, `ab_glyph`,
`sha2`, `notify`, `wgpu`, `openengine-contracts`, `openengine-ecs`,
`openengine-math`. `contracts` gains the `ResourceRequest` / `ResourceReady`
wire types under a coordinated `docs/abi/` update (any `#[repr(C)]`/enum change
requires an `ARCH_VERSION` bump + all consumers rebuilt in one commit).

## Next steps

1. Add `ResourceRequest` / `ResourceReady` to `contracts` with `docs/abi/`
   update and `ARCH_VERSION` bump.
2. Stand up `AssetRegistry` + `KindRegistry<T>` and content-hash dedup.
3. Implement the tokio-backed loaders per kind and the per-frame drain.
4. Wire GPU upload on the frame submission.
5. Route `WorldDelta.deferred` requests into the asset tick and publish
   `ResourceReady` back into the `StateView`.
6. Add the dev-only `notify` watcher + by-handle hot swap.
7. Placeholder resources + failure logging.
