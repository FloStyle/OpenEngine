---
spec: "30-project-playground"
phase: "Phase 5: Editor"
status: "draft"
author: "OpenEngine AI"
created: "2026-09-03"
depends_on:
  - "06-scene-management"
  - "16-serialization"
  - "22-edit-vs-play"
  - "23-undo-redo"
  - "25-editor-shell"
  - "29-plugins"
---
# 30 - Project Playground

## Overview

A **project** is the top-level unit of authoring in OpenEngine: a single
directory that bundles the scenes, settings, and assets a game (or a demo, or a
prototype) is made of. It plays the role of Unreal's `.uproject` or Unity's
project folder — the thing a human or agent *opens* when they want to work on a
game, and the scope that a saved *layout*, the *preferences* (spec `32`), the
shortcut table (spec `28`), and the asset browser (spec `26`) all live under.

Everything about a project exists so the editor can treat a folder as one
portable, relocatable, self-describing unit. Two guarantees shape the whole
design and echo AGENTS.md § 5 (Portability) directly:

- **A project is self-contained and relocatable.** All cross-references inside
  it (scene default, asset manifest entries, plugin ids, config keys) are
  *relative to the project root* or logical names — never absolute paths, never
  home directories, never platform separators. You can copy a project folder to
  another machine (or another OS, `x86_64-linux` ↔ `aarch64-linux`) and open it
  without editing a single file.
- **A project is deterministic and diffable.** A project on disk is plain
  `serde_json` manifests + the existing versioned `postcard` scene/save format
  (spec `06`/`16`). Loading the same project twice yields the same project model;
  saving is byte-stable for the same logical content (no `HashMap` iteration
  order, sorted `Vec`s). Projects are therefore reviewable and shareable as text.

`ProjectPlayground` is the editor system that owns the **currently open
project**: it provides the create / open / save / recent-lifecycle, exposes the
per-project settings document, resolves every relative path against the open
project's root, and tells the rest of the editor (scene registry, asset browser,
plugins, preferences) which directory they should read from. It is Domain A
(`crates/editor`), pure of gameplay state, and never touches Domain B.

## Core Concepts

### What a project is on disk

A project is a directory with a fixed, small layout. The **manifest** file
`project.json` at the root is the project's identity and its settings header;
the sibling directories are conventional homes for the different kinds of
content the project owns.

```text
MyGame/                      # project root (arbitrary, relocatable name)
├── project.json             # manifest: name, version, default_scene, settings
├── assets/                  # imported assets (spec 02) under OPENENGINE_ASSETS_PATH semantics
├── scenes/                  # scene files (spec 06/16 serialized scenes)
├── scripts/                 # authored logic sources / logic.wasm targets (spec 12)
├── plugins/                 # plugin manifests / payloads (spec 29)
└── config/                  # per-project preferences overrides, layouts, shortcuts
```

The editor never hardcodes this directory *path* — the project root is whatever
the user chose at open time and is remembered as a *relative or `$HOME`-anchored*
recent-project entry (never a baked absolute path in code; see Constraints). All
four sibling directories are conventional defaults resolved from the root; any
code that needs `OPENENGINE_ASSETS_PATH` / `OPENENGINE_CONFIG_PATH`-style access
for this project reads the same env-var rules but pointed at the project subtree
(see **Path resolution** below).

### The manifest: `project.json`

The manifest is the one file that must exist for a folder to be openable as a
project. It is intentionally small — *identity + settings pointer* — so a
project stays trivially diffable and mergeable.

```rust
/// The on-disk project manifest (root of the project folder).
#[derive(Clone, Debug, serde::Serialize, serde::Deserialize)]
pub struct ProjectManifest {
    pub name: String,                 // display name ("MyGame")
    pub version: String,              // semver of the project, not of the engine
    pub default_scene: Option<String>,// relative scene path under scenes/, e.g. "main.oe_scene"
    pub settings: ProjectSettings,    // per-project settings (see below)
    pub schema_version: u32,          // manifest schema version for migration
    pub engine_min_abi: u32,          // ARCH_VERSION the project was authored against
}
```

`engine_min_abi` is compared to `contracts::ARCH_VERSION` at open time: if the
project was authored against a *newer* ABI than the running host, opening is
refused loudly (mirroring the logic-module fingerprint gate). This keeps an old
editor from silently corrupting a newer project.

### Path resolution — everything relative to the root

This is the single most important rule and it is enforced at the type level. A
project exposes a `PathResolver` that converts every *logical relative* reference
(scene path, asset path, plugin path, script path) into a concrete path only at
the last possible moment, inside Domain A, and never stores the concrete result
back into any manifest, component, or preference.

```rust
/// Turns logical, project-relative references into concrete filesystem paths,
/// only at the I/O boundary, and never caches an absolute form.
pub struct PathResolver<'a> {
    root: &'a Path,   // the open project's directory (Domain A fs path)
}
impl PathResolver<'_> {
    /// "scenes/main.oe_scene" -> <root>/scenes/main.oe_scene
    pub fn scene(&self, rel: &str) -> PathBuf;
    pub fn asset(&self, rel: &str) -> PathBuf;    // under the assets subtree
    pub fn plugin(&self, rel: &str) -> PathBuf;   // under the plugins subtree
    /// Root-relative normalization: rejects "..", leading '/', drive letters,
    /// and Windows separators so no reference can escape the project.
    pub fn validate_relative(&self, rel: &str) -> Result<String, ProjectError>;
}
```

`validate_relative` is a small pure gate used by every loader (scene, asset,
plugin) before it touches disk: a reference that would traverse outside the
project root (`..`, absolute, drive-letter, platform separator) is a
`ProjectError::EscapeRoot`, never followed. This is what makes a portable
"no escape from the project" guarantee that holds on both supported platforms.

### Lifecycle: create / open / save / recent

`ProjectPlayground` owns the lifecycle state machine. It is Domain A and mostly
pure (the *model* transitions are headless-testable); only the actual `std::fs`
read/write touches the OS.

```rust
pub struct ProjectPlayground {
    pub open: Option<OpenProject>,    // the loaded project model + root
    pub recent: Vec<RecentProject>,   // most-recent-first, bounded, persisted
    pub activity: ProjectActivity,    // Idle | Creating | Opening | Saving(background)
}
pub struct OpenProject {
    pub manifest: ProjectManifest,
    pub root: PathBuf,                // resolved at open; kept in the editor only
    pub is_dirty: bool,               // any unsaved manifest/scene changes
}
```

- **Create.** The user (or agent command) supplies a name and a parent directory
  under which a new project folder is materialized: the four sibling directories
  plus a default `project.json` (name, `version = "0.1.0"`, no default scene, and
  per-project settings at their defaults). Creating a project does **not**
  auto-open it unless the caller asks; `recent` is updated only when a project is
  actually opened or created-and-opened.
- **Open.** Validate the manifest (`schema_version`, `engine_min_abi`), resolve
  the root, then load the *default scene* (spec `06`) through the scene registry
  into the edit world, register the project's plugins (spec `29`) that live under
  `plugins/`, and refresh the asset registry (spec `02`) from `assets/`. Opening
  never mutates the project; it only populates editor state.
- **Save.** Persists (a) any dirty scene(s) through spec `16`/`06` as the active
  edit world, (b) the `project.json` manifest including current per-project
  settings, and (c) notifies preferences (spec `32`) and layouts (spec `25`) so
  they flush their per-project files. Save is staged through the same boundary
  discipline as every other editor write and runs its disk work in the
  background so the UI never blocks (see spec `35` progress notification).
- **Recent.** `recent: Vec<RecentProject>` stores an ordered, bounded (default 8)
  list of *portable* handles to previously opened projects. A `RecentProject`
  records only a display name plus a path that is **relative or anchored to
  `$HOME`** (per AGENTS.md § 5 it may be `$HOME`-relative but never a hardcoded
  literal); on a machine where the anchor does not resolve it is skipped, not an
  error. The list persists as JSON under the **global** (user-level, not
  project-scoped) config location described in spec `32`.

```rust
/// A portable "recently opened project" handle. Never a raw absolute literal.
pub struct RecentProject {
    pub name: String,
    /// "$HOME/…", "~/…", or a bare relative path. Resolved at click time.
    pub anchor: String,
    pub opened_last: String,   // ISO-8601 display timestamp, not used for determinism
}
```

### Per-project settings

A project carries a small, explicit settings document that travels with the
project (it lives in `project.json`) and is distinct from *global user
preferences* (spec `32`). The split is: **preferences are per-user** (how this
editor behaves for this human/agent everywhere), **project settings are per-game**
(how *this game* runs and is authored), and the two never collide because
project settings always win when both define a key (a user preference cannot
override a project's authored default-scene). This is the same conceptual split
Unreal/Unity draw between *Editor preferences* and *Project settings*.

```rust
/// Per-project settings — game-level, shipped-affecting, authored.
#[derive(Clone, Debug, serde::Serialize, serde::Deserialize)]
pub struct ProjectSettings {
    pub default_scene: Option<String>,   // mirrors manifest for convenience; resolved here
    pub game_mode: Option<String>,       // which scene/system runs at play, by name
    pub target_platform: TargetPlatform, // primary build/play target (see below)
    pub editor: EditorScopeSettings,     // per-project editor tweaks (grid, snap)
}
```

- **Default scene / map** — the scene to open on project load and the scene that
  a fresh Play session boots when no explicit scene is chosen. Stored as a
  relative path into `scenes/`, resolved via `PathResolver::scene`.
- **Game mode** — a logical name of the play session's top-level system bundle
  (which scripts/systems the play world runs, spec `22`/`12`). The editor maps
  the name to registered host systems / sandbox systems; it is a *name*, not a
  file path, so it stays portable.
- **Target platform** — the intended primary target, which the play/run and
  build tooling use to pick shader/storage decisions. It is **not** a hard
  constraint that forbids other platforms (AGENTS.md keeps everything compiling
  on both `x86_64-linux` and `aarch64-linux`); it only selects defaults.

```rust
#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum TargetPlatform {
    LinuxX86_64, LinuxAarch64,  // both always supported; these are authored intentions
    DedicatedServer,            // headless, no renderer (still x86_64/aarch64)
}
```

`TargetPlatform` is deliberately a small, portable enum (no "Windows"/"macOS"
that this repo cannot CI) so the manifest stays honest about what OpenEngine can
actually run and test. It drives *which* bits get built/played, never a
hardcoded filesystem assumption.

### Integration with scene management and serialization

Project *loading and saving are thin orchestrators over spec `06` (scenes) and
spec `16` (world codec)* — the project system never defines a second scene or
save format.

- The project's `scenes/` folder holds scene files in the spec `06`/`16`
  versioned `postcard` envelope (`magic`/`abi_version`/`format_version` gated).
  Opening a project calls the scene registry's existing load path with the
  default scene's bytes; saving writes the active edit world through spec `16`.
- The project's *default_scene* is only a pointer; scene files themselves are
  fully specified by spec `06`/`16`, so a scene authored under project A can be
  copied into project B's `scenes/` and referenced by B's manifest without
  conversion. The project system adds **no** wrapping scene format.
- `ARCH_VERSION` and the `FormatHeader` gate still apply verbatim: a project
  whose default scene carries a newer `abi_version` than the host refuses to
  open that scene (spec `16`), surfacing a spec-`35` blocking error with the
  version mismatch.

Because scenes are authoring-time (spec `06`) and the world codec round-trips
raw `Pod` column bytes deterministically (spec `16`), "open project" and "save
project" reduce to: resolve the root → load/save the one or more scene files +
the manifest → done. Everything else the project owns (assets, plugins,
preferences overrides) delegates to its owning system.

### Project ↔ preferences / shortcuts / layout scope

A project does **not** own the *global* user preferences or the global shortcut
table — those are per-user and specified in spec `32`/`28`. But a project may
carry **scoped overrides** under `config/` (a per-project layout snapshot, a
per-project `project.shortcuts.json`, per-project editor settings inside
`ProjectSettings::editor`). Resolution precedence when reading any configurable:

```text
project-level value (ProjectSettings::editor)  >  per-project config/ file  >  global preference (spec 32)  >  built-in default
```

The highest-priority present value wins; each layer is optional. This lets a
team ship a project with an opinionated grid/snap/layout while each developer's
global preferences still supply anything the project did not pin.

## Key Rust Types

```rust
// crates/editor/project — Domain A (std + egui-free model; UI wraps it)
use std::path::{Path, PathBuf};
use contracts::ARCH_VERSION;

pub struct ProjectManifest {
    pub name: String,
    pub version: String,
    pub default_scene: Option<String>,
    pub settings: ProjectSettings,
    pub schema_version: u32,
    pub engine_min_abi: u32,          // compared against contracts::ARCH_VERSION
}
pub struct ProjectSettings {
    pub default_scene: Option<String>,
    pub game_mode: Option<String>,
    pub target_platform: TargetPlatform,
    pub editor: EditorScopeSettings,
}
pub struct EditorScopeSettings {
    pub grid_spacing: I16F16,          // fixed-point, matches spec 24 GridSettings spacing
    pub snap_step: I16F16,             // world-space snap grid (spec 25 Toolbar)
    pub default_layout: Option<String>,// spec 25 layout name to load on open
}
pub enum TargetPlatform { LinuxX86_64, LinuxAarch64, DedicatedServer }

pub struct ProjectPlayground {
    pub open: Option<OpenProject>,
    pub recent: Vec<RecentProject>,
    pub activity: ProjectActivity,
}
pub struct OpenProject { pub manifest: ProjectManifest, pub root: PathBuf,
    pub is_dirty: bool }
pub struct RecentProject { pub name: String, pub anchor: String,
    pub opened_last: String }

pub enum ProjectError {
    ManifestMissing,            // no project.json
    BadSchema(u32),             // schema_version not migratable
    AbiTooNew { want: u32, have: u32 },   // engine_min_abi > ARCH_VERSION
    EscapeRoot,                 // a reference left the project root
    SceneVersionSkew,           // a scene's abi_version mismatched (spec 16)
    Io(std::io::Error),         // host read/write failure
}
```

Supporting types from the owning systems: `World`, `SceneRegistry`,
`SceneId`/`SceneHandle` (spec `06`); `FormatHeader`/`WorldSnapshot` codec
(spec `16`); `openengine-math::I16F16` (fixed-point grid/snap); plugin registry
(spec `29`); asset registry + `AssetId` (spec `02`); `EditorCommandBus` /
undoable `Command` (spec `23`).

## Components

None — editor/UI tooling only; no new ECS component. A project is a *folder on
disk + an in-editor model*; it never introduces a component or an entity, and
nothing a project contains is represented as ECS state. The scenes the project
opens already carry their own components via the scene/world codec (spec `06`/
`16`/`00`).

## Constraints

- **Domain A only.** `ProjectPlayground`, path resolution, lifecycle and I/O live
  in `crates/editor` (with the on-disk format shared only through the existing
  scene codec). Never compiled into Domain B.
- **Portability — no absolute/hardcoded/home-literal paths in code.** All
  references inside the project are root-relative logical strings; the concrete
  root is chosen by the user at open time and stored only as `$HOME`-anchored or
  relative (AGENTS.md § 5). `PathResolver::validate_relative` is the enforced gate
  against path escape on both `x86_64-linux` and `aarch64-linux`.
- **Deterministic and diffable on disk.** Manifests are `serde_json` over sorted
  structures (no `HashMap` iteration); scene bytes are the spec `16`/`06`
  deterministic `postcard` envelope. Saving the same logical project twice is
  byte-stable.
- **One scene/save format.** The project adds no second scene or save format; it
  orchestrates spec `06` (scenes) and spec `16` (world codec). `ARCH_VERSION`
  /`abi_version` gating still applies and a too-new project/scene is refused, not
  guessed.
- **Fixed-point for any authored numeric setting.** Grid spacing and snap are
  `I16F16` (matches spec `25`); `f32` never appears in authored project data.
- **No gameplay state.** A project is authoring scope; entering Play (spec `22`)
  deep-clones the default scene's world and never marks the project "dirty" by
  itself. Only explicit Save-to-Edit (spec `22`) or a scene save dirties it.
- **Undoable project actions are commands.** Any project action that changes the
  *edit world* (e.g. "new scene from default" or "load default scene") flows
  through spec `23` commands; open/close/save themselves are host operations
  surfaced with spec `35` notifications, not world mutations.
- **Recent-list paths are portable handles**, skipped (never fatal) when their
  anchor no longer resolves.
- **Headless-testable.** The whole project *model* (manifest serde, validation,
  lifecycle transitions, recent-list bookkeeping) runs in unit tests with no GPU
  and no window; only the `std::fs` layer is OS-touching and is factored behind a
  thin trait for tests.

## Performance Targets

- **Open** a typical project (manifest + default scene decode + asset registry
  refresh): < **100 ms** cold; the scene decode dominates (spec `06` target
  < 50 ms for 10k entities) and runs off the UI thread with a progress
  notification (spec `35`).
- **Save:** staging the scene + writing the manifest stays off the hot path and
  completes with no visible UI hitch (background write, spec `35`).
- **Path resolution:** `validate_relative` is a pure string check < 1 µs; scene /
  asset / plugin resolution is a single `PathBuf` join, negligible.
- **Recent-list load/save:** trivially fast (< 1 ms), bounded to N=8 entries.
- **Idle cost:** with no project open, `ProjectPlayground` adds ~0 per frame; the
  "no project / welcome" shell state is static.

## Testing Strategy

All tests headless (no GPU / no window) in `crates/editor`:

- **Manifest round-trip:** build a `ProjectManifest`, serialize to
  `serde_json`, deserialize, assert equality and that serializing twice is
  byte-identical (determinism).
- **Path-resolution gate:** feed `scene("../../etc/passwd")`, `asset("/abs")`,
  `plugin("C:\\…")`, and Windows-separator forms to `validate_relative`; assert
  each returns `ProjectError::EscapeRoot` and that valid relative references
  resolve under the root. Run on both target-style path semantics (pure string,
  so a single test set covers both).
- **Lifecycle (model only):** create → open → save transitions update
  `OpenProject`/`is_dirty`/`recent` exactly; open of a too-new `engine_min_abi`
  or bad `schema_version` yields the matching `ProjectError`, never a panic.
- **Recent-list:** open several, assert most-recent-first bounded order; persist
  and reload; an unresolvable anchor is skipped silently.
- **Integration with scenes:** build a project whose default scene is a saved
  spec-`16` world; open it and assert the edit world equals the scene's content
  (via `WorldHash`, spec `16`). Save a dirty edit world back and assert the on-disk
  scene bytes round-trip.
- **Setting precedence:** with a global preference and a project override for the
  same key, assert the project value wins; with no override, the global
  preference wins.
- **No-GPU invariant:** every test above exercises only the model + codec; the
  fs layer is behind a mockable `ProjectStore` trait used in tests.

## Dependencies

- `crates/editor` (Domain A) — `ProjectPlayground`, `PathResolver`,
  lifecycle, welcome/shell integration (spec `25`).
- Scene codec + registry from spec `06`; world codec + `WorldHash` from spec
  `16`; edit-vs-play `EditorState` from spec `22`; undoable `Command`/
  `EditorCommandBus` from spec `23`.
- Asset registry + `AssetId` from spec `02` (opening refreshes the browser);
  plugin registry from spec `29` (loading the `plugins/` subtree); preferences
  scope + global/user config location from spec `32`; shortcut/layout scoping
  from spec `28`/`25`.
- `openengine-math::I16F16`; `serde` + `serde_json`; `postcard` (via spec `16`);
  `contracts::ARCH_VERSION`. No new `contracts`/ABI surface.

## Next Steps

1. Define `ProjectManifest`/`ProjectSettings`/`TargetPlatform` and the
   `serde_json` schema + `schema_version` migration hooks.
2. Implement `PathResolver::validate_relative` and the path gate; factor the
   `std::fs` layer behind a `ProjectStore` trait.
3. Implement `ProjectPlayground` lifecycle (create/open/save) + `recent` and
   the `$HOME`-anchored persistence.
4. Wire open/save to the spec `06`/`16` scene codec and the spec `02`/`29`
   asset/plugin registries; surface progress via spec `35`.
5. Define per-project setting precedence vs spec `32` preferences and spec
   `25`/`28` scoped overrides.
6. Land the headless model/serialization/path-gate test battery and the
   welcome-state ("no project") shell path.
