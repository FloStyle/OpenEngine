---
spec: "26-asset-browser-ui"
phase: "Phase 5: Editor"
status: "draft"
author: "OpenEngine AI"
created: "2026-09-03"
depends_on: ["02-asset-pipeline", "23-undo-redo", "21-primitive-components"]
---
# 26 - Asset Browser UI

## Overview

The asset browser is the **editor panel that lets a human or agent manage every
asset in the project** — browsing folders, previewing contents as thumbnails,
searching/filtering, importing new files by drag-and-drop, and turning assets
into entities. It is a Domain-A `egui` system in `crates/editor` and is mounted
as the **Asset Browser** panel in the editor shell (spec `25-editor-shell`).

The browser is a **view over the asset pipeline of spec `02`**, never a parallel
asset system. It reads the pipeline's canonical-name / content-hash registry
(spec `02`) and asks the pipeline to perform loads, imports and re-imports. All
I/O, decode and GPU-upload side effects stay inside Domain A's spec-`02`
loaders; the browser UI never touches files or GPU directly.

The invariant that shapes this spec: **the browser must never block the UI
thread**. Asset import, thumbnail generation and any decode are async — the
browser renders a placeholder/spinner and updates when a background job lands,
exactly as spec `02`'s async loaders already do. And every action that changes
the scene (dropping an asset onto the viewport to spawn, dropping onto the
hierarchy to attach) goes through an **undoable `Command`** from spec `23` so the
result can be undone/redone — never a direct world mutation.

## Core Concepts

### Data model: virtual tree over the asset registry

The browser does not walk the OS filesystem each frame. It builds a **virtual
folder tree** from the set of canonical asset names the pipeline knows about,
plus any on-disk folders under `OPENENGINE_ASSETS_PATH`. Folders are derived by
splitting each asset's *relative* path on `/`; an asset `tiles/grass` (spec `02`
`AssetName`) contributes a `tiles` folder containing `grass`.

```rust
// crates/editor — Domain A
pub struct AssetBrowser {
    pub current_path: String,        // relative dir under OPENENGINE_ASSETS_PATH, "" == root
    pub selected_assets: Vec<AssetId>, // stable ids, spec 02
    pub search_query: String,
    pub type_filter: Option<AssetKindFilter>,
    pub thumbnails: HashMap<AssetId, Thumbnail>, // cache keyed by asset id
    pub tree: FolderNode,            // the virtual tree snapshot (see below)
    pub pending: Vec<ImportJob>,     // in-flight imports/thumbnails (async)
}
```

Paths are **always relative to `OPENENGINE_ASSETS_PATH`** (AGENTS.md § 5): no
absolute path, no home directory, no platform separators baked in. The browser
normalizes `/` internally so layouts/config are portable across
`x86_64-linux`/`aarch64-linux`.

### Folders & files

```rust
pub struct FolderNode {
    pub name: String,                 // this folder's relative name ("" == root)
    pub folders: Vec<FolderNode>,     // sorted, deterministic
    pub files: Vec<AssetEntry>,       // sorted, deterministic
}

pub struct AssetEntry {
    pub id: AssetId,                  // pipeline registry id (spec 02)
    pub kind: AssetKind,              // Texture | Mesh | Shader | Audio | Font
    pub rel_path: String,             // relative to OPENENGINE_ASSETS_PATH
    pub tags: Vec<String>,            // user tags (stored in the .meta sidecar)
}
```

The tree is rebuilt when the pipeline's registry changes (import/delete/
re-import) or when the user collapses/expands — not every frame. Folders and
files within a folder are sorted by name so the tree is deterministic and
diffable.

### `Thumbnail`

A `Thumbnail` is a small rendered preview. Each asset kind gets a different
preview generated **offline and asynchronously** so the UI never blocks:

```rust
pub struct Thumbnail {
    pub asset_id: AssetId,
    pub kind: ThumbKind,          // which generator produced it
    pub image: egui::ColorImage,  // the rasterized preview (downscaled)
    pub generation: u64,          // bumps when the source bytes change (hash)
    pub status: ThumbStatus,      // Pending | Ready | Failed(placeholder)
}

pub enum ThumbKind { TexturePreview, MeshMiniRender, AudioWaveform, ShaderSnippet }
pub enum ThumbStatus { Pending, Ready, Failed }
```

Per-kind generators (all run in the spec-`02` background runtime, never the UI
thread):

- **TexturePreview** — downscale the decoded texture (nearest/mip) to a small
  grid, e.g. ≤ 128 px.
- **MeshMiniRender** — the *only* thumbnail needing a GPU. For headless/CI and
  the no-GPU constraint this is optional: if no device is available the mesh
  shows a unit-cube wireframe icon instead. On a device, an off-screen render of
  the mesh is captured to a color image in a background task.
- **AudioWaveform** — render the decoded `AudioClip` samples (spec `02`) as a
  min/max waveform strip.
- **ShaderSnippet** — a code-styled rendering of the first N lines of the WGSL
  source, not a real raster.

Thumbnails are cached by `asset_id` in `AssetBrowser::thumbnails` (a `HashMap`
— see the deterministic-ordering note in **Constraints**) and invalidated when
the asset's content hash changes (spec `02`). A `ThumbStatus::Failed` shows a
placeholder icon; it never crashes or blocks.

### Search / filter / sort

- **Search** matches the `search_query` against each asset's `rel_path` and its
  `tags` (case-insensitive substring). Matches are gathered across the whole
  tree (not just `current_path`) unless the user scopes to the current folder.
- **Type filter** (`type_filter`) narrows to one `AssetKind` (or "All").
- **Sort** is by name, date, or size (of the packed file), toggled with a
  header control; the default is name. Sorting and the search result set are a
  pure projection over the tree snapshot and never mutate the world.

Filters combine: search ∩ type-filter, then sort. An empty result shows an
"Import assets by dropping files here" empty state.

### Virtualization for large trees (10k+ files)

The browser renders only the **visible** rows of the current viewport using
`egui`'s scroll-to-offset + row-skipping (a virtualized list). The underlying
data is a flat sorted `Vec<AssetEntry>` (the "flattened, filtered, sorted" list)
plus a precomputed cumulative row-height table; the browser asks egui for the
visible `y` range and draws only entries whose rows intersect it. Folder
collapsing folds a folder's entries out of the flat list. This keeps a 10 000+
file project responsive: building the *flat filterable list* is O(n log n) once
per registry change, and per-frame paint is O(visible rows).

### Drag & drop

The browser is a drop **source** and **target**:

- **asset → viewport** spawns a new entity that uses that asset via an undoable
  `Command` from spec `23`. The command carries the `AssetId`/kind and the
  spawn target archetype; spec `21-primitive-components` supplies the
  mesh/sprite/render components the spawn initializes. Undo despawns the spawned
  entity; redo re-spawns it.
- **asset → hierarchy** (spec `08`) attaches the asset to the *currently
  selected* entity — e.g. adding a sprite/mesh component referencing the asset —
  again via an undoable `Command`.
- **asset → inspector** drops the asset into an `AssetRef` field of the selected
  component (spec `07` asset picker), setting the relative path.
- **drag from the OS / file picker** into the browser window = **import** (below).

```rust
pub enum AssetDragPayload {
    Asset(AssetId),
    Folder(String),                 // relative folder path
    OsFiles(Vec<OsImportPath>),     // absolute OS paths seen only during import
}
```

The `OsFiles` absolute paths exist **only transiently inside the import dialog
transaction**; the moment an import completes the browser records relative
targets and drops the absolute handles. No absolute path is ever stored in a
layout, config, component, or the registry.

### Context menu

Right-click on a file/folder opens:

- **Open external** — opens the file with the platform handler (Domain A only;
  portability guarded, disabled where no handler is available).
- **Rename** — an inline text field; commits a rename by moving the file to a
  new relative path and re-keying the pipeline registry (spec `02` canonical
  name update).
- **Delete** — removes the file + its `.meta` sidecar and evicts it from the
  pipeline registry; a confirmation for folders.
- **Reimport** — re-run the import pipeline for the selected asset (see spec
  `02` hash-based re-decode).
- **Show in explorer** — reveal the asset's folder in the host file manager
  (Domain A; best-effort per OS, no crash when unsupported).
- **Copy / reveal in tree / set tags** — copy the relative path to clipboard,
  navigate the tree to reveal the asset, and edit its tag set.

Rename/delete/reimport/tag all route through the pipeline (spec `02`) and emit
an undoable `Command` (spec `23`) so destructive ops can be undone.

### Import pipeline

Dropping files or choosing "Import" opens a per-type settings dialog, then runs
a background conversion:

```rust
pub enum ImportJob {
    Texture { src: OsImportPath, srgb: bool, generate_mips: bool },
    Mesh { src: OsImportPath, scale: I16F16, weld: bool },
    Shader { src: OsImportPath },
    Audio { src: OsImportPath, stream: bool },
    Font  { src: OsImportPath },
}
```

Flow (all of it async, off the UI thread):

1. Drop/file-pick yields OS paths.
2. A dialog sets per-type settings (color space/mips for textures, scale for
   meshes, ...). Settings are transient — never persisted into the project.
3. A background task runs the spec-`02` converter/decoder for the type,
   writing the converted payload under `OPENENGINE_ASSETS_PATH` at a relative
   target and creating a `.meta` sidecar (tags, import settings, content hash).
4. On completion the browser posts the new canonical name back to the pipeline
   registry; the tree rebuilds and a `Thumbnail` job is queued.
5. On failure the job logs (spec `11` console) and shows a toast; it never
   blocks or panics.

Because spec `02`'s placeholder-on-failure and async-drain machinery already
exist, the import pipeline is a thin Domain-A wrapper that maps dialog settings
into an `ImportJob` and relays pipeline events back into the tree/thumbnail
caches.

### Thumbnail/import async and never-blocking

The browser keeps a bounded queue of background jobs (`pending: Vec<ImportJob>`)
owned by the spec-`02` runtime. Each frame the browser drains ready results (the
same per-frame drain spec `02` uses for `LoadResult`s) and updates `thumbnails` /
the tree. The egui paint pass never awaits a job; if a preview isn't ready it
draws a placeholder box. This is the same contract spec `02` states: the frame
thread never blocks on asset work.

## Key Rust Types

```rust
// crates/editor — Domain A only
pub struct AssetBrowser { pub current_path: String,
    pub selected_assets: Vec<AssetId>, pub search_query: String,
    pub type_filter: Option<AssetKindFilter>,
    pub thumbnails: HashMap<AssetId, Thumbnail>, pub tree: FolderNode,
    pub pending: Vec<ImportJob> }
pub struct Thumbnail { pub asset_id: AssetId, pub kind: ThumbKind,
    pub image: egui::ColorImage, pub generation: u64, pub status: ThumbStatus }
pub enum ThumbKind { TexturePreview, MeshMiniRender, AudioWaveform, ShaderSnippet }
pub enum ThumbStatus { Pending, Ready, Failed }
pub struct FolderNode { pub name: String, pub folders: Vec<FolderNode>,
    pub files: Vec<AssetEntry> }
pub struct AssetEntry { pub id: AssetId, pub kind: AssetKind,
    pub rel_path: String, pub tags: Vec<String> }
pub enum AssetKindFilter { Texture, Mesh, Shader, Audio, Font, All }
pub enum AssetDragPayload { Asset(AssetId), Folder(String), OsFiles(Vec<OsImportPath>) }
pub enum ImportJob { Texture { src: OsImportPath, srgb: bool, generate_mips: bool },
    Mesh { src: OsImportPath, scale: I16F16, weld: bool }, Shader { src: OsImportPath },
    Audio { src: OsImportPath, stream: bool }, Font { src: OsImportPath } }
```

Supporting types: spec-`02` `AssetId`, `AssetKind`, `AssetName`, the pipeline
registry; spec-`23` `Command` for undoable drag/spawn/edit/delete; spec-`21`
primitive mesh/sprite/render components for spawn; `openengine-math::I16F16` for
the fixed-point import scale (no raw `f32` in authored data). Drag-to-spawn goes
through spec `24` viewport and spec `08` hierarchy drop targets.

## Constraints

- **Domain A only.** The browser, its jobs and thumbnail generators live in
  `crates/editor` and use the spec-`02` Domain-A runtime; never in the guest.
- **Relative paths only.** Everything under `OPENENGINE_ASSETS_PATH` is stored
  relative. Absolute OS import paths exist only transiently inside an `ImportJob`
  transaction and are never persisted into layout/config/registry/component.
- **Async, never-blocking.** Import and thumbnail generation run in the
  spec-`02` background runtime; the egui paint pass never awaits them (same
  drain contract as spec `02`). The mesh mini-render is the sole GPU-dependent
  thumbnail and falls back to a wireframe icon when no device is present
  (headless/CI-safe, per the no-GPU logic-test rule).
- **Undoable mutations.** Drag-to-spawn, drag-to-hierarchy, drag-to-inspector,
  rename, delete, reimport and tag edits all emit a spec-`23` `Command`; none of
  them mutates the world directly.
- **Virtualized big lists.** The file list is a flattened, filtered, sorted
  `Vec` rendered through egui offset/row-skipping — required for 10 000+ files.
- **Deterministic tree.** Folders/files sorted by name; iteration over the tree
  never uses `HashMap` ordering. (`thumbnails: HashMap<AssetId, _>` is a cache
  keyed by stable id, never iterated in a display- or serialization-order way.)
- **Placeholder over panic.** A failed/failed-to-generate thumbnail or import
  shows a placeholder/logs; never a crash (mirrors spec `02`).
- **Compiles on `x86_64-linux` and `aarch64-linux`.** Tree building, filtering,
  search and import-settings logic are headless unit-tested with no GPU/window.

## Performance Targets

- Flat filterable list build (10 000 assets): < 25 ms per registry change,
  amortized once; per-frame paint is O(visible rows) only.
- Search over the full tree (10 000 assets, substring on path+tags): < 5 ms for
  a single debounced query.
- Thumbnail render off-thread so the frame thread stays under its 16.67 ms
  budget regardless of import/thumbnail load (reuse spec-`02` budget).
- Thumbnail cache hit (already-generated, hash unchanged): no re-render, one
  `HashMap` lookup.
- Import of a typical texture/mesh completes without any visible UI hitch; the
  browser shows the new entry as soon as the pipeline registers it.

## Testing Strategy

- **Tree building (headless):** from a fixture of canonical names, assert the
  `FolderNode` tree and per-folder sort are correct and deterministic (run 3×,
  identical output).
- **Filtering/search:** assert search ∩ type-filter and name/date/size sort give
  the expected flat order; empty-state is shown for no matches.
- **Virtualization:** with a synthetic 10 000-file registry, assert only the
  visible rows are painted (row-skip math) and iteration cost is bounded.
- **Thumbnail lifecycle (headless):** TexturePreview/ShaderSnippet/AudioWaveform
  generate without a device; MeshMiniRender falls back to the icon when no GPU;
  a hash change invalidates and regenerates; `Failed` yields a placeholder, never
  a panic.
- **Async non-blocking:** start an import; assert the UI path returns immediately
  (job queued) and the entry appears only after the pipeline registers it.
- **Undoable drag (integration):** drop an asset onto the viewport → a spec-`23`
  `Command` spawns an entity with the spec-`21` components; Undo despawns it and
  Redo re-spawns it; assert the world returns to the identical state after
  undo/redo (deterministic).
- **Relative-path invariant:** import, rename and drag-to-inspector all assert
  the stored reference is relative to `OPENENGINE_ASSETS_PATH` — never absolute.

## Dependencies

- Spec `02-asset-pipeline`: `AssetRegistry`, `AssetId`/`AssetKind`/`AssetName`,
  async loaders, content-hash invalidation, placeholder-on-failure, drain.
- Spec `23-undo-redo`: the `Command` type every browser world mutation emits.
- Spec `21-primitive-components`: mesh/sprite/render components used when a
  drag-to-spawn initializes an entity.
- Spec `24-editor-viewport` and spec `08-editor-hierarchy`: the drop targets that
  receive `AssetDragPayload` for spawn/attach.
- Spec `07-editor-inspector`: the `AssetRef` field drop target.
- Spec `25-editor-shell`: the Asset Browser is mounted as a shell panel.
- `egui` (Domain A), `serde`/`serde_json` for the `.meta` sidecar,
  `openengine-math::I16F16`, and the spec-`02` Domain-A runtime for background
  import/thumbnail jobs.

## Next Steps

1. Build the virtual `FolderNode` tree + flat filterable `Vec` from the spec-`02`
   registry and implement folder collapse + row-skip virtualization.
2. Implement `Thumbnail` cache and the TexturePreview/ShaderSnippet/
   AudioWaveform generators; add the no-GPU mesh icon fallback.
3. Implement search/filter/sort controls over the snapshot.
4. Implement drag payloads and the viewport/hierarchy/inspector drop targets,
   routing world changes through spec-`23` `Command`s.
5. Implement the context menu (open external / rename / delete / reimport / show
   in explorer / tags) through the spec-`02` pipeline + spec-`23` commands.
6. Implement the per-type import dialog and background `ImportJob` → registry
   round-trip with the `.meta` sidecar.
