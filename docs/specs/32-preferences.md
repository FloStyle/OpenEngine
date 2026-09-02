---
spec: "32-preferences"
phase: "Phase 5: Editor"
status: "draft"
author: "OpenEngine AI"
created: "2026-09-03"
depends_on:
  - "22-edit-vs-play"
  - "25-editor-shell"
  - "28-keyboard-shortcuts"
  - "30-project-playground"
---
# 32 - Preferences

## Overview

Preferences are the user-facing, user-editable settings that govern how the
*editor itself* behaves and how it presents the world — as opposed to game-level
authored settings (default scene, game mode, target platform), which live in the
project manifest (spec `30`). Preferences are organized into **categories**
(General / Editor / Viewport / Rendering / Audio / Debug), stored as versioned
JSON behind a typed **schema with defaults**, edited through a **modal Settings
UI with live search**, and made **exportable / importable / resettable** so a
user (or agent) can move a setup between machines or restore a known-good one.

There are **two preference scopes**, mirroring the split every editor draws
between "how I like my editor" and "how this project's team set it up":

- **Global preferences** — per-user, apply to every project on this machine.
  They live under the user-level config location (below), not inside any project.
- **Per-project preferences** — an optional overlay scoped to the *currently open
  project* (spec `30`), stored under that project's `config/`. When both define a
  key, the per-project value wins; otherwise the global value or the built-in
  default applies. This is exactly the precedence spec `30` declares for
  configurable editor values.

Preferences are Domain A (`crates/editor`), pure of gameplay state, and never
reach Domain B. A preference is read on a *frame boundary* or at the start of an
interaction, never mid-iteration; changing a preference takes effect on the next
read, with no restart. Critically, preferences must never affect the
determinism of the play world: only *editor presentation/tooling* keys are
preferences, and none of them feed the pure simulation (spec `22` guarantees the
play world is a deterministic clone independent of editor UI).

The **Settings modal integrates with spec `28` (keyboard shortcuts)**: the
shortcut table is surfaced as a preferences-backed section so one UI edits both
plain preference values and key bindings, and both persist consistently.

## Core Concepts

### Scopes and storage roots (portable)

```text
Global preferences   ->  <user config root>/preferences.json
                          <user config root>  = OPENENGINE_CONFIG_PATH if set,
                            else $XDG_CONFIG_HOME/openengine (Linux) or
                            $HOME/.config/openengine; never a hardcoded literal.
Per-project overlay  ->  <open project root>/config/preferences.json   (spec 30)
Shortcut table       ->  <same two roots>/shortcuts.json               (spec 28)
```

Both scopes are plain `serde_json` files written atomically (write-temp-rename)
and loaded at editor start and on project open/close. The global root is chosen
from `OPENENGINE_CONFIG_PATH` when present (AGENTS.md § 5 keeps the env var
authoritative); otherwise it resolves under `$XDG_CONFIG_HOME`/`$HOME`, **never**
a baked absolute or per-username path. Project-scoped files are always relative
to the open project (spec `30`).

### The typed schema with defaults

Preferences are **not** an ad-hoc bag of strings. Each key is a typed field in a
`serde` struct grouped by category, and each category struct implements a
`Default` that is the built-in identity. This gives a compile-time-known schema,
easy defaults, and diffable JSON.

```rust
/// The full, typed preference document (one scope). serde round-trips the
/// whole tree; missing keys fall back to `Default`.
#[derive(Clone, Debug, Default, serde::Serialize, serde::Deserialize)]
pub struct Preferences {
    pub general: GeneralPrefs,
    pub editor: EditorPrefs,
    pub viewport: ViewportPrefs,
    pub rendering: RenderingPrefs,
    pub audio: AudioPrefs,
    pub debug: DebugPrefs,
    pub schema_version: u32,   // bumped when a category gains/removes a field
}
```

Category contents (illustrative but concrete; each is a small typed struct):

- **General** — editor language (default `"en"`), autosave interval, whether to
  show the start "welcome / no project" screen, recent-project count (max of
  spec `30`'s list), theme (light/dark/system).
- **Editor** — tool settings that are *editor* concerns: default transform mode
  (spec `25`), whether gizmo snapping starts enabled, the fixed-point snap grid,
  undo history depth (spec `23` `max_history`), console history size (spec `27`),
  default dock layout to load (spec `25`), whether to auto-open the last layout.
- **Viewport** — default grid spacing (spec `24`), grid colors, default view
  mode (Wireframe/Solid/Textured/Lit), default camera move speed, whether hover
  highlight is on, default projection (perspective/orthographic).
- **Rendering** — vsync, MSAA sample count, default `ViewMode`, tonemapping,
  max visible light count in the editor viewport, background clear color. These
  only affect the *editor* presentation renderer (spec `04`), never the
  simulation.
- **Audio** — master volume, mute, output device (by logical id, best-effort),
  voice count cap. Host/Domain-A audio (spec `14`) reads these at mixer
  boundaries; none affect deterministic logic.
- **Debug** — log level (`OPENENGINE_LOG_LEVEL` default override), whether
  console/overlay opens on error, gizmo/component debug overlays, physics debug
  draw toggle, and the "show stats" FPS/memory status-bar toggle (spec `25`).

Fixed-point discipline applies to any *spatial/timelike* editor value: snap grid,
grid spacing and volumes that reach authored content are `I16F16` (matching spec
`24`/`25`); plain UI counters (undo depth, list sizes) are `usize`. Raw `f32`
appears only for *display* quantities (camera speed, FPS targets) that never
enter committed world state.

### Reading preferences (typed accessor + merge)

Each scope is stored as its own typed document. The effective value for a key is
the **merged** result across the precedence chain. To keep this cheap and safe,
the editor maintains a single materialized `EffectivePreferences` that is
recomputed when a scope file changes or a project opens/closes — not per frame.

```rust
/// The resolved view every panel reads. Recomputed on file/project change.
pub struct EffectivePreferences { pub prefs: Preferences /* merged */ }

pub fn effective(
    global: &Preferences, project: &Option<Preferences>,
) -> EffectivePreferences { /* project overrides global over default */ }
```

`merge` is structural: for each category struct, any field present in the
project doc replaces the global/doc value; missing fields fall through. Because
both scopes use the *same* typed schema, merge is a small per-field fold, not
schema-guessing. Panels read `EffectivePreferences` (a plain reference) and never
touch disk or global/project maps directly.

### The modal Settings UI with search

A modal (opened from the toolbar/menu or the `settings.open` shortcut) shows all
categories in a left sidebar and the selected category's fields on the right.
A **search box** at the top filters both the category list and, within the open
category, highlights/filters fields whose label or help text matches — so a user
can jump straight to "grid spacing" without knowing which category it lives in.
Search is a pure projection over the schema's label/help index, issued once per
keystroke against a debounced timer.

Edits are staged into a `PreferencesDraft` and **committed atomically** (one
"Apply" button, or auto-commit on a per-field debounce that writes the whole
scope doc): the draft holds changed fields and Commit merges them into the target
scope (`Global` or `Project`), serializes, and writes the file. **Cancel** /
closing discards the draft. An edit that fails to serialize or write keeps the
UI consistent (no partial file) and surfaces a spec-`35` error toast.

### Export / Import / Reset

- **Export** serializes the current effective (or chosen-scope) preferences to a
  chosen `serde_json` file and offers it for copy/move. Export is a clean copy of
  the typed doc — portable across `x86_64-linux`/`aarch64-linux` and relocatable.
- **Import** reads such a file, validates `schema_version` + field types, and
  either *replaces* the chosen scope or *merges* (imported keys win over current).
  A too-new `schema_version` or a malformed file is refused with a clear spec-`35`
  error, never partially applied.
- **Reset** restores the built-in `Default` for a single field, a category, the
  whole scope, or (for shortcuts) the spec-`28` defaults. Reset writes the scope
  doc back through the same atomic commit. A per-project scope reset never touches
  the global scope, and vice versa.

### Integration with spec 28 shortcuts

The Settings modal shows a **Shortcuts** section (spec `28`) as another left-side
entry. Editing there uses the same capture-row UI as spec `28`'s Settings →
Shortcuts page and writes the *same* `shortcuts.json` (per current scope) that
spec `28` owns. There is a single source of truth for a given scope: the modal
and the spec-`28` panel are two views over one file. Shortcut *conflict
validation* (spec `28`) runs on every commit whether it came from the modal or
the capture UI.

## Key Rust Types

```rust
// crates/editor/prefs — Domain A
pub struct Preferences { pub general: GeneralPrefs, pub editor: EditorPrefs,
    pub viewport: ViewportPrefs, pub rendering: RenderingPrefs,
    pub audio: AudioPrefs, pub debug: DebugPrefs, pub schema_version: u32 }

#[derive(Default)] pub struct GeneralPrefs { pub language: String,
    pub autosave_secs: u64, pub show_welcome: bool,
    pub recent_count: usize, pub theme: Theme }  // Theme: Light|Dark|System
#[derive(Default)] pub struct EditorPrefs { pub default_transform: TransformMode,
    pub snap_enabled: bool, pub snap_step: I16F16, pub undo_depth: usize,
    pub console_history: usize, pub default_layout: Option<String> }
#[derive(Default)] pub struct ViewportPrefs { pub grid_spacing: I16F16,
    pub grid_major_every: u32, pub default_view_mode: ViewMode,
    pub cam_speed: f32, pub hover_highlight: bool, pub default_projection: ProjectionMode }
#[derive(Default)] pub struct RenderingPrefs { pub vsync: bool,
    pub msaa_samples: u32, pub view_mode: ViewMode, pub tonemap: bool,
    pub max_editor_lights: u32, pub clear_color: [u8; 4] }
#[derive(Default)] pub struct AudioPrefs { pub master_volume: I16F16,
    pub muted: bool, pub output: Option<String>, pub voice_cap: u32 }
#[derive(Default)] pub struct DebugPrefs { pub log_level: Option<String>,
    pub open_console_on_error: bool, pub show_gizmo_debug: bool,
    pub physics_debug_draw: bool, pub show_stats: bool }

pub enum PrefScope { Global, Project }
pub enum PrefCategory { General, Editor, Viewport, Rendering, Audio, Debug, Shortcuts }
pub struct EffectivePreferences { prefs: Preferences }  // merged; read-only views
pub fn effective(global: &Preferences, project: &Option<Preferences>) -> EffectivePreferences;
pub struct PreferencesStore { global: Preferences, project: Option<Preferences>,
    pub scope: PrefScope /* editing target */ }
impl PreferencesStore {
    pub fn load(global_root: &Path, project: Option<&OpenProject>) -> Self;
    pub fn effective(&self) -> EffectivePreferences;
    pub fn apply_draft(&mut self, scope: PrefScope, draft: &PreferencesDraft)
        -> Result<(), PrefError>;
    pub fn reset(&mut self, scope: PrefScope, field: ResetTarget) -> Result<(), PrefError>;
    pub fn export(&self, scope: PrefScope, to: &Path) -> Result<(), PrefError>;
    pub fn import(&mut self, scope: PrefScope, from: &Path, mode: ImportMode)
        -> Result<(), PrefError>;
}
pub struct PreferencesDraft { pub scope: PrefScope, pub changed: Preferences }
pub enum ResetTarget { Field(PathString), Category(PrefCategory), WholeScope }
pub enum ImportMode { Replace, Merge }
pub enum PrefError { Io(String), SchemaTooNew { have: u32, max: u32 },
    Malformed(String), ShortcutConflict { a: String, b: String } } // shortcut from spec 28
```

## Components

None — editor/UI tooling only; no new ECS component. Preferences are pure
Domain-A configuration documents (typed JSON) with zero representation in the ECS
or the guest. Reading or changing a preference never creates, mutates, or removes
an entity or component; preferences feed only editor tooling/render/audio host
surfaces, which are themselves outside the deterministic sim (spec `22`).

## Constraints

- **Domain A only.** `PreferencesStore`, schema, modal, merge and serialization
  live in `crates/editor`; never compiled into Domain B.
- **Portable storage roots.** Global preferences resolve from
  `OPENENGINE_CONFIG_PATH` when set, else `$XDG_CONFIG_HOME`/`$HOME`-anchored —
  no hardcoded absolute or per-username path (AGENTS.md § 5). Per-project
  preferences are always relative to the open project root (spec `30`).
- **No gameplay/determinism impact.** Only editor presentation/tooling/host
  keys are preferences. No preference feeds the pure simulation; the play world
  (spec `22`) is a deterministic clone independent of any preference value.
- **Fixed-point for spatial/timelike editor values.** Snap grid, grid spacing
  and volumes are `I16F16`; `f32` is restricted to display quantities that never
  enter committed world or authored data.
- **Atomic, schema-valid writes.** Files are written temp-then-rename; a
  malformed or too-new import is refused, never partially applied. `schema_version`
  gates forward migration (a later engine may add fields; missing fields fall to
  `Default`).
- **Merged effective view only.** Panels read `EffectivePreferences`; they never
  touch scope files or mutate globals/project maps directly.
- **Changes take effect next read, no restart.** Preferences are read at frame/
  interaction boundaries, never mid-iteration.
- **Deterministic ordering.** Schema fields are fixed struct fields (no `HashMap`
  iteration) so exported JSON is byte-stable for the same logical content.
- **Compiles on `x86_64-linux` and `aarch64-linux`.** All preference model,
  merge, reset, export/import and search logic is pure and headless-testable with
  no GPU and no window.

## Performance Targets

- **Read path:** a preference lookup is a plain field read on `EffectivePreferences`
  — sub-microsecond; panels never hit disk.
- **Recompute `EffectivePreferences`** on file/project change: < 1 ms (small
  typed fold), done off the hot path.
- **Search** over the schema's field index: < 1 ms per debounced keystroke.
- **Serialize/write** a scope doc (a few KB): < 1 ms; file writes are
  temp-then-rename and never on the egui paint path.
- **Import/export:** bounded by file size, < 5 ms for a typical doc.
- **Settings modal idle cost:** ≈ 0 when closed; open-modal cost is ordinary egui
  panel cost.

## Testing Strategy

All headless (no GPU / no window) in `crates/editor`:

- **Schema/default round-trip:** build a `Preferences` with defaults, serialize,
  deserialize; assert equality and byte-stable JSON across 3 serializations; a doc
  missing fields deserializes to `Default` for those fields (forward-compat).
- **Merge precedence:** with a global + a project doc setting the same key
  differently, `effective()` returns the project value; with no project override,
  the global value; with neither, the default. Category-level partial overrides
  merge per-field.
- **Atomic write:** a forced write failure leaves no partial file and keeps the
  in-memory doc consistent (assert temp/rename behavior with a mockable
  `PrefStore` file layer).
- **Export/import:** export → import (Replace and Merge) reproduces the expected
  effective prefs; a `schema_version` too new or a malformed file yields
  `PrefError`, never a partial state.
- **Reset:** reset one field / one category / whole scope restores the exact
  `Default` in each case without touching the other scope.
- **Search:** a search index over the schema's labels returns the right fields
  for a query; empty results shown for no match.
- **Shortcut integration:** a shortcut edit committed from the modal writes the
  same file spec `28` reads (assert file equality), and a conflicting binding is
  rejected with `PrefError::ShortcutConflict`.
- **No-world impact:** assert that loading/editing preferences produces no delta
  against the edit world and leaves the play world's `WorldHash` (spec `16`)
  unchanged — preferences never feed the sim.

## Dependencies

- `crates/editor` (Domain A) — `PreferencesStore`, schema, modal, `EffectivePreferences`;
  mounts the Settings modal into the shell (spec `25`).
- Project root + `config/` scope from spec `30`; edit-vs-play isolation from spec
  `22`; typed editor/transform/viewport value types from specs `24`/`25`.
- Shortcut schema/conflict/file ownership from spec `28`; undo depth and console
  history hooks feed spec `23`/`27`; render/audio host knobs feed spec `04`/`14`.
- `openengine-math::I16F16`; `serde` + `serde_json`; `OPENENGINE_CONFIG_PATH`
  (AGENTS.md § 5). No new `contracts`/ABI surface.

## Next Steps

1. Define the typed `Preferences` schema + category structs with `Default` and
   `schema_version`.
2. Implement `PreferencesStore` load/merge/`effective`, the `PrefScope` overlay,
   and atomic write-temp-rename persistence for both roots.
3. Implement `PreferencesDraft` + Apply/Cancel commit and the merge fold.
4. Build the modal Settings UI (category sidebar + typed editors + live search).
5. Implement export/import (Replace/Merge) and per-scope/per-field reset with
   schema validation.
6. Wire the modal's Shortcuts section to spec `28` (shared `shortcuts.json` +
   conflict validation) and mount the modal into the shell (spec `25`).
