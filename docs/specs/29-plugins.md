---
spec: "29-plugins"
phase: "Phase 5: Editor"
status: "draft"
author: "OpenEngine AI"
created: "2026-09-03"
depends_on: ["23-undo-redo", "25-editor-shell", "26-scene-serialization", "27-console", "SECURITY.md"]
---

# Plugins

## Overview

Plugins extend the editor without ever weakening the two invariants this
repository is built on: the **Wasm sandbox** (Domain B purity/determinism) and
the **single undoable mutation channel** (spec `23`). OpenEngine supports two
plugin kinds with deliberately different power, matching the two domains:

* **HOST plugins** — native Rust `cdylib`s (`.so` / `.dylib` / `.dll`) loaded
  with `libloading`. They run in the editor process (Domain A) and may add
  UI-level capability: panels, console commands (spec `27`), shortcuts (spec
  `28`), and component definitions. They are *trusted* (they run as the editor),
  so they are held to the same discipline as editor code: every world mutation
  through `Command`s, no new unsafe surface, portability rules intact.
* **LOGIC plugins** — Wasm modules for gameplay logic, driven by `wasmtime`
  exactly like built-in Domain B logic. They obey Domain B purity rules
  verbatim: `no_std`, fixed-point math, pure `fn(&StateView) ->
  Result<WorldDelta, RecoverableError>`, exports in `crates/logic-export`, and
  they run under the full sandbox (memory cap, instruction budget). They can
  never be given an API that would let them reach ECS/file/network.

A `PluginManager` discovers, validates, loads, drives, and hot-reloads both
kinds from a plugin directory. Isolation and stability are first-class: each
plugin owns its state, cannot reach other plugins, and a panicking (or hostile)
plugin can never crash or corrupt the editor or another plugin.

## Core Concepts

### plugin.json manifest

Every plugin ships a `plugin.json` next to its payload, declaring identity,
dependencies, the entry point, and the permission set it requests. The manager
validates the manifest against the *registered* capability surface before it
loads anything, so a request for a capability that does not exist fails fast.

```json
{
  "name": "example_tools",
  "version": "1.0.0",
  "author": "OpenEngine AI",
  "description": "Editor tooling example",
  "dependencies": [],
  "entry_point": "libexample_tools",
  "permissions": ["panel", "command", "component", "world_view", "emit_delta"]
}
```

For a **host** plugin, `entry_point` is the `cdylib` name to load via
`libloading`. For a **logic** plugin, `permissions` are effectively ignored —
the sandbox is the permission — and `entry_point` names the Wasm module path.
`permissions` is an allow-list checked against the concrete `PluginAPI` handed
out; a plugin is instantiated with exactly the API its permissions grant.

### The safe plugin API (host kind)

A host plugin receives a **`PluginAPI`**, a capability handle that is *narrower*
than what editor code can do, so a plugin cannot accidentally do something a
first-party editor system would. It is built from the permissions in the
manifest: no permission → the corresponding method is absent/returns `Err`.

```rust
// crates/editor/plugins — Domain A
pub struct PluginAPI<'a> {
    panel: Option<PanelSink<'a>>,        // "panel"
    commands: Option<CommandSink<'a>>,   // "command"
    components: Option<ComponentSink<'a>>,// "component"
    world: Option<&'a WorldView<'a>>,    // "world_view" — read-only snapshot
    bus: &'a EditorCommandBus,           // always present: mutations go here
}
```

Through the API a plugin may:

* **register_panel** — add an egui panel rendered each frame (into spec `25`).
* **register_command** — add a `(name, description, handler)` console command
  (spec `27`); the handler receives the same safe `ConsoleHandle`, so it cannot
  mutate the world directly.
* **register_component** — define a new component type (id, `Pod` schema,
  displayer) in the host component registry.
* **register_shortcut** — add an action + context binding (spec `28`), subject
  to conflict detection.
* **read world via `WorldView`** — a safe, immutable SoA read of the *edit*
  world.
* **emit delta via `emit_delta`** — build a `WorldDelta` and hand it to the
  `EditorCommandBus`, which wraps it in an undoable `Command` (spec `23`). This
  is the *only* write channel.

**There is no `&mut World`, no filesystem handle, no network handle, and no
`unsafe` exposed to a plugin.** The `PluginAPI` is constructed from the same
`EditorCommandBus` that editor panels use (spec `07`/`08`/`25`), so a host
plugin's writes are just as undoable and deterministic as any editor write. A
host plugin that needs to persist state does so through its **own plugin
directory** (see **Isolation**), opened via an API method that validates the
path, never through an arbitrary filesystem capability.

### The safe plugin API (logic kind)

A logic plugin is a Wasm module. It is instantiated by the same
`wasmtime`-based loader that runs built-in Domain B logic (spec `01`/`10`), so
it inherits every guard: guest linear memory cap (≤ 256 MB), per-tick
instruction budget (≤ 16 ms), a read-only `StateView`, and an exported pure
system `fn(&StateView) -> Result<WorldDelta, RecoverableError>`. Its exports
live in `crates/logic-export` (never bare `#[no_mangle]` in the module), and
the manifest registers the host-side system name it implements. **Nothing in the
logic-plugin path grants a `PluginAPI`**; the guest gets a `StateView`, not a
capability object, and cannot express file, network, or ECS access. This is the
line that keeps the sandbox intact.

```rust
// Domain A host driver for a logic plugin — reuses the built-in sandbox.
pub struct LogicPlugin {
    engine: wasmtime::Engine,
    module: wasmtime::Module,
    instance: wasmtime::Instance,
    // host-side caps: state memory ≤ 256 MiB, fuel/epoch, time ≤ 16 ms.
}
```

### Plugin trait (host kind)

A host plugin implements a small, stable trait that the manager calls over its
lifecycle. It is object-safe so the manager can store heterogeneous plugins.

```rust
pub trait Plugin: Send + Sync {
    /// One-time setup, after the PluginAPI has been granted.
    fn init(&mut self, api: PluginAPI<'_>) -> Result<(), PluginError>;
    /// Called each editor frame with a fresh read-only world view.
    fn update(&mut self, api: &PluginAPI<'_>, view: &WorldView<'_>);
    /// Release resources, drop registrations, flush state.
    fn shutdown(&mut self) {}
}
```

`LoadedPlugin` ties the trait object to its manifest, its native library (kept
alive while loaded), and its opaque plugin-owned state.

```rust
pub struct LoadedPlugin {
    pub manifest: PluginManifest,
    pub library: Option<libloading::Library>, // host kind; None for logic kind
    pub instance: Box<dyn Plugin>,            // host kind trait object
    pub state: PluginState,                   // manager-scoped lifecycle state
    pub kind: PluginKind,                     // Host | Logic
}
```

### Isolation and stability

- Each plugin gets its **own state container**; the `PluginAPI`/`WorldView`
  handed out never references another plugin's objects. `register_*` sinks are
  namespaced by plugin name (e.g. `example_tools.my_command`) so plugins cannot
  shadow or reach each other's registrations.
- **Panic containment.** Host plugins run inside `catch_unwind` at the
  `init`/`update`/handler boundary. A panic is logged, the plugin is marked
  `faulted`, and — if hot-reload is on — the manager schedules a reload. The
  editor and every other plugin keep running. No plugin code is ever in a
  position to unwind across an `unsafe` frame into the editor's loop.
- **Filesystem.** A plugin may only touch its **own plugin directory**, resolved
  and path-validated by the manager under `OPENENGINE_PLUGINS_PATH` (default
  `./plugins/`). There is no arbitrary-fs capability; the path never points at
  system or home directories.
- **Logic plugins** are additionally isolated by the Wasm sandbox itself: they
  have no WASI, no network, no file descriptors, a capped heap, and an
  instruction budget — identical guarantees to Domain B, verified by
  `verify-wasm-purity`.

### Hot-reload

A file watcher (or a manual "reload" from a menu) watches the plugin directory.
On a change to a plugin's manifest/payload the manager: (1) calls `shutdown` on
the old instance, (2) unloads it, (3) re-validates the manifest, (4) loads and
`init`s the new instance, and (5) **preserves plugin state when possible** by
snapshotting the plugin's persisted state (from its own dir) and handing it back
to the new `init`, so e.g. a tool's configured options survive a reload. A
plugin that fails to reload is rolled back to the previous working instance if
the manager can keep the old library resident (host kind); otherwise it is
marked `faulted` with the error logged. Hot-reload never applies to logic
plugins mid-tick — it swaps at the tick boundary like spec `10` hot-reload.

## Key Rust Types

- `PluginManager` — owns discovered `LoadedPlugin`s, capability tables, the
  manifest validator, the hot-reload watcher, and (for logic kind) a shared
  `wasmtime::Engine`.
- `LoadedPlugin { manifest, library, instance, state, kind }`.
- `PluginManifest` — name/version/author/description/dependencies/entry_point/
  permissions, plus a `kind` discriminator.
- `PluginKind { Host, Logic }`.
- `PluginAPI<'a>` — capability handle built from `permissions` (host kind).
- `trait Plugin { init, update, shutdown }`.
- `PluginError` — `Manifest`, `Load`, `Init`, `CapabilityDenied`, `Conflict`,
  `Unsafe` variants (parallels `RecoverableError` semantics, host-typed).
- `WorldView<'a>`, `EditorCommandBus`, `ConsoleHandle`, `ShortcutAction` — the
  shared safe surfaces the API exposes.

## Constraints

- **Plugins must never break the Wasm sandbox or determinism.** Host plugins
  get no guest-visible power and write only via `EditorCommandBus`; logic
  plugins are Wasm modules with exactly Domain B purity rules. There is **no
  `unsafe` passed into any guest**, and no path by which a host plugin could
  grant a logic module file/network/ECS access.
- Two capabilities are **never** handed to a plugin: direct ECS mutation (only
  `emit_delta` → undoable `Command`) and an unrestricted filesystem/network
  handle. Filesystem access is limited to the plugin's own validated directory.
- Mutations are undoable and deterministic because they ride spec `23` and the
  spec `07`/`08`/`25` `EditorCommandBus`; a plugin cannot open a second write
  channel.
- Registry/command/component/shortcut registration is namespaced and conflict-
  checked (spec `28`) so plugins are isolated from each other and from the
  editor.
- Panics are caught; a plugin cannot crash the editor or unwind into the loop.
- Portability: host plugins compile for `x86_64-linux`/`aarch64-linux`;
  everything headless-testable; plugin logic tests need **no GPU**.
- Manifest/paths use env vars (`OPENENGINE_PLUGINS_PATH`), never absolute/
  hardcoded/home paths (AGENTS.md § 5, SECURITY.md § 6).

## Performance Targets

- Load + `init` a host plugin: < 50 ms cold (dominated by `dlopen`/`dylib`);
  not on the frame hot path.
- `update` dispatch to all host plugins: bounded, total < 1 ms/frame at ≤ 16
  plugins; a faulting plugin skips without stalling others.
- Logic plugin tick cost obeys the sandbox budget (≤ 16 ms/tick, ≤ 256 MB), and
  is measured/wall-clock-gated exactly like built-in Domain B (spec `11`).
- Hot-reload detection latency: < 500 ms after a file change; swap done at a
  safe boundary with no world mutation in progress.
- Idle (no plugins, or watcher quiet): ≈ 0 overhead when no plugins are loaded.

## Testing Strategy

- **Forbidden-op blocked:** a test plugin requests `network`/`filesystem`
  permissions (absent from the capability surface) → manifest validation fails
  with `CapabilityDenied`; a plugin that nonetheless attempts an ECS `&mut` has
  no API surface to express it (compile-time proof by construction).
- **Panic survival:** a host plugin that panics in `init`/`update`/a command
  handler → editor keeps running, plugin marked `faulted`, others unaffected
  (assert no process-level unwind).
- **Isolation:** two plugins registering the same namespaced command coexist;
  a faulting plugin cannot reach another plugin's state (no shared handle).
- **Sandbox purity for logic plugins preserved:** compile a logic plugin for
  `wasm32-unknown-unknown`, run `python3 brain/orchestrator.py
  verify-wasm-purity <wasm>` → `[PURE]`; assert the guest saw a `StateView`, not
  a `PluginAPI`; assert memory cap and instruction budget are enforced.
- **Undoable writes:** a host plugin's `emit_delta` flows into the
  `EditorCommandBus` and appears as a spec `23` command that `undo`/`redo`
  handle; the world mutates only through that channel.
- **Hot-reload:** edit a plugin's `init` code, trigger reload, assert the new
  behavior is live and plugin state (its own dir) survived when possible; a
  broken reload rolls back or marks `faulted`.
- **No-GPU invariant:** manager, validator, capability, isolation, and panic
  tests run headless in CI; panel rendering is behind a window feature.

## Dependencies

- `libloading` (host `cdylib`s), `wasmtime` (logic Wasm) — both Domain A.
- Reuses the spec `10` hot-reload watcher and the Domain B sandbox driver;
  `crates/logic-export` for logic exports; `verify-wasm-purity` in `brain/`.
- Extension points: `EditorCommandBus` + `Command`s (spec `23`), shell/panels
  (spec `25`), scene load/save hooks (spec `26`), console registration (spec
  `27`), shortcut registration + conflict detection (spec `28`).
- Governed by `SECURITY.md` (§ 2 purity, § 6 filesystem, § 7 resource limits)
  and AGENTS.md (§ 1 Domain Boundaries, § 5 Portability).

## Next Steps

1. Define `PluginManifest`, `PluginKind`, `PluginError`, and the manifest
   validator + capability allow-list.
2. Implement the host loader (`libloading`) and the `Plugin` trait + `LoadedPlugin`.
3. Implement the safe `PluginAPI` (`register_panel/command/component/shortcut`,
   `WorldView`, `emit_delta`) wired into the shared `EditorCommandBus`.
4. Implement logic-plugin loading by reusing the Domain B `wasmtime` sandbox and
   `crates/logic-export`; verify purity.
5. Implement panic containment at init/update/handler boundaries and the
   `faulted` lifecycle.
6. Implement hot-reload with state preservation, plus isolation/namespacing.
7. Add integration + security tests (forbidden-op, panic, isolation, purity).
