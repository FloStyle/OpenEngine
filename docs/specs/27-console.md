---
spec: "27-console"
phase: "Phase 5: Editor"
status: "draft"
author: "OpenEngine AI"
created: "2026-09-03"
depends_on: ["23-undo-redo", "25-editor-shell", "29-plugins"]
---

# Console

## Overview

The console is an interactive, text-driven developer surface in the editor. A
single keystroke (`~`, or `F1` where a layout does not have a tilde key) toggles
a terminal-style overlay that accepts commands (`name arg1 arg2`) and doubles as
a scrollable, filterable log viewer. It is the fastest way for a human or an
agent to drive the **edit world** without touching mouse UI: spawn an entity by
component list, delete/select one, run/pause/stop the simulation, undo/redo,
save/load scenes, quit, and run commands that plugins registered.

The console is unambiguously **Domain A** (`crates/editor`). It talks to the
world through the same safe channel every other editor panel uses (spec `07` /
`08`): a read-only `WorldView` for display and **undoable `Command`s** (spec
`23`) for every mutation. It never touches ECS storage directly and never
reaches into Domain B memory. When a command names a *logic* concern (for
example `play`/`pause`/`stop`), the console routes it to the host that drives the
sandbox — the console never pretends to mutate simulation state itself.

The log side reuses and extends the debugging-tools console buffer from spec
`11`, adding color-coding, level filtering, substring search, and an
append-only, persistent history shared with the command line.

## Core Concepts

### Overlay toggling

A global editor action "toggle console" is bound by default to the backquote key
`~` (spec `28`). When the overlay is open it captures keyboard focus: typed
characters go to the input buffer, `Enter` executes, `Up`/`Down` walk history,
and `Tab` auto-completes. When closed it is fully passive — it only consumes log
records and paints nothing. Toggling never cancels an in-flight command or
discards the input buffer.

### Command line model

The overlay has one input line. `Enter` parses the current buffer into
`(name, args: Vec<String>)` by splitting on ASCII whitespace (quoted tokens
allowed via a tiny tokenizer), looks the name up in the command registry, and
dispatches to its handler. Unknown names produce a `warn` log entry listing the
closest registered names (simple edit-distance suggestion) rather than failing
silently.

A command **never mutates the world directly.** Every mutating built-in
(`spawn`, `delete`, `select` is not a world mutation, `undo`, `redo`, editor
state) is expressed as one or more undoable `Command`s from spec `23` and pushed
onto the command stack, so console actions participate in the same
undo/redo/determinism machinery as every other editor write. Pure/read commands
(`help`, `clear`, `list`, `select`) do not allocate an undo entry.

### Two execution contexts: edit world vs sim

The editor keeps an **edit world** and hands deltas to the sandbox when
`play`ing (per spec `23`/`25`). Console commands therefore target the **edit
world**. During edit mode the world is directly editable through `Command`s.
During play, structural commands are either deferred until pause or routed to
the sandbox as host-originated deltas at the flush boundary — the console does
not pick; the `EditorCommandBus` decides based on the sim state.

### Log viewer (append-only)

The overlay's lower region renders the log as scrollable, **append-only** lines.
A line is a `LogEntry` with a timestamp, a severity level, and a message. Levels
`info`, `warn`, `error` map to deterministic colors (blue/green=info,
amber=yellow=warn, red=error) that a theme can restyle. Two controls sit above
the list: a **level filter** (min level shown: info/warn/error) and a **search**
box (case-insensitive substring; optional regex behind an advanced toggle).
Entries are never edited or removed in place — "clear" only moves the read
cursor / drops references, never rewrites history, so the append-only invariant
holds for anything that must be reproducible.

### Input history

Executed command lines are appended to `Console.history`. `Up`/`Down` walk the
ring in most-recent-first order with the current partially-typed buffer parked
at `history_index == history.len()` (a draft slot that survives the walk).
History is **persistent across sessions**: on editor start it is loaded from
`OPENENGINE_CONFIG_PATH/console_history.json`; on clean exit (or periodically)
it is written back, bounded to the most recent N entries (default 1000). It is
stored under the config env var only — never a hardcoded/home path (AGENTS.md §
5).

### Tab auto-completion

`Tab` completes against the union of registered command names plus, for a few
commands, their known argument vocabulary (component ids for `spawn`, entity ids
/ names for `select`, `delete`). A prefix match fills the buffer to the longest
common prefix; a second `Tab` (when the first was ambiguous) shows the candidates
as `info` log lines. Completion is a pure lookup — it executes nothing.

### Built-in commands

`help` lists built-ins and plugin commands with their one-line descriptions;
`clear` clears the visible log; `list` prints archetypes/component ids for
completion; the mutating set is described in **Console built-in commands** below.
Plugin registration is handled by spec `29`, which calls into the console to add
`(name, description, handler)` triples.

## Console built-in commands

The built-in command set is deliberately small and stable. All mutating
built-ins wrap their effect in spec `23` commands.

| Command | Args | Effect | Undoable |
|---------|------|--------|----------|
| `spawn` | `<component_list>` | Spawn an entity into the archetype given by the comma/space-separated component list (resolved via the component registry); selects it | yes |
| `delete` | `<entity_id>` | Despawn the referenced entity (generation-guarded) | yes |
| `select` | `<entity_id>` | Set the shared `SelectionModel` current entity; no world change | no |
| `play` | — | Enter play mode (edit world snapshot pushed; sandbox driven) | yes (pause restores) |
| `pause` | — | Suspend the sim, keep the live world | no |
| `stop` | — | Exit play, restore the pre-play edit snapshot | yes |
| `undo` | — | Undo the top spec `23` command | n/a |
| `redo` | — | Re-apply the last undone command | n/a |
| `save` | `<scene>` | Save the current edit world to the named scene (relative to `OPENENGINE_ASSETS_PATH` scenes) | no |
| `load` | `<scene>` | Replace the edit world with the named scene | yes (undoable swap) |
| `quit` | — | Request editor shutdown (via the shell) | no |
| `help` | — | List registered commands + descriptions | no |
| `clear` | — | Clear the visible log region | no |

Parsing `<entity_id>` accepts a numeric slot, a decimal `index.generation`, or a
unique `Name` component value; `delete`/`select` re-resolve handles against the
live world before acting so a stale id is rejected, not applied to a recycled
slot.

## Key Rust Types

```rust
// crates/editor/console — Domain A
use std::collections::HashMap;

/// Severity of a log/console line; drives color + filtering.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, serde::Serialize, serde::Deserialize)]
pub enum LogLevel { Info, Warn, Error }

/// One immutable, append-only log line. Timestamps are host wall-clock ms
/// relative to editor start (display only; never used for determinism).
#[derive(Clone, Debug, serde::Serialize, serde::Deserialize)]
pub struct LogEntry {
    pub timestamp: u64,     // ms since editor start (host, Domain A)
    pub level: LogLevel,
    pub message: String,
}

/// A registered console command. `handler` is called on the console thread
/// with a parsed arg vector and a handle to the safe console facade.
pub type CommandHandler = dyn Fn(&[String], &mut ConsoleHandle) + Send + Sync;

/// The stateful console: current input, history ring, immutable log, and the
/// name -> handler registry. Lives on the Domain A editor thread.
pub struct Console {
    pub input_buffer: String,
    pub history: Vec<String>,          // executed lines, newest last; persisted
    pub history_index: usize,          // cursor into history (len == draft slot)
    pub log_entries: Vec<LogEntry>,    // append-only
    pub commands: HashMap<String, CommandSpec>, // name -> (desc, handler)
    // UI-only, reset on toggle:
    pub min_level: LogLevel,
    pub search: String,
}

/// Registry descriptor for one command (built-in or plugin-provided).
pub struct CommandSpec {
    pub description: String,
    pub handler: Box<CommandHandler>,
}

/// Opaque handle handed to a handler: the ONLY world/undo surface it sees.
/// Bound to the edit world; hides the ECS entirely.
pub struct ConsoleHandle<'a> {
    bus: &'a EditorCommandBus,        // spec 23 / 25
    selection: &'a SelectionModel,    // spec 07 / 08
    log: &'a mut Vec<LogEntry>,       // append only
}
```

`Console` owns all of its state; egui panels (spec `25`) render a read-only
borrow each frame. `commands` uses a `HashMap` — iteration order is never
depended on (help output sorts names; completion uses longest-common-prefix over
a sorted key snapshot) to respect the determinism/ordering rules. Because the
console is Domain A, a `HashMap` here is permitted as long as nothing
order-sensitive reads it unordered.

### The append-only guarantee

`log_entries` only ever grows via `push`. A "clear" keeps the buffer and merely
resets the paint cursor / a generation counter so it can be reused for
reproduction without losing the byte history. Command output is written as log
entries too, so everything the console prints is reproducible in the log.

### Command execution flow (edit world)

```rust
impl Console {
    /// Parse `input_buffer`, resolve the name, execute via ConsoleHandle.
    /// Unknown name -> warn + nearest-name suggestion; panic in a handler is
    /// caught (console never takes down the editor, mirroring spec 29 policy).
    pub fn execute(&mut self, bus: &EditorCommandBus, selection: &SelectionModel) {
        let (name, args) = tokenize(self.input_buffer.clone());
        if name.is_empty() { return; }
        match self.commands.get(&name) {
            Some(spec) => {
                // Handler gets ONLY the safe facade; mutations go through bus.
                let mut handle = ConsoleHandle { bus, selection, log: &mut self.log_entries };
                if let Err(e) = catch_unwind(AssertUnwindSafe(|| (spec.handler)(&args, &mut handle))) {
                    self.push(LogEntry::error(format!("console `{name}` panicked: {e:?}")));
                }
            }
            None => {
                let hint = nearest_name(name, self.commands.keys());
                self.push(LogEntry::warn(format!("unknown command `{name}`{hint}")));
            }
        }
        self.commit_history(); // push input_buffer to history, clamp, persist
    }
}
```

Every mutating handler reduces to `bus.push(command)` (spec `23`) so undo/redo
and determinism hold; handlers that need to read the edit world get a
`WorldView` through the bus, never a `&mut World`.

## Constraints

- **Domain A only.** The console is part of `crates/editor`; it is never
  compiled into the guest, never references `contracts` types for mutation, and
  never touches Domain B memory.
- Console commands operate on the **edit world** and wrap every mutation in
  undoable `Command`s from spec `23`. **No direct ECS mutation** — a handler only
  sees `ConsoleHandle` (bus + selection + log). A handler that tries to bypass
  the bus (writes to world storage) has no API surface to do so.
- The log is **append-only**; entries are never mutated or removed in place.
- Sim/play concerns are routed to the host sim driver; the console issues no
  delta against a running sandbox mid-tick.
- History persistence uses `OPENENGINE_CONFIG_PATH` only (AGENTS.md § 5) —
  **no absolute, hardcoded, or home paths.**
- Platform-agnostic input: the toggle key, history navigation, and completion
  come from spec `28` shortcut bindings, not hardcoded keycodes.
- Compiles on `x86_64-linux` and `aarch64-linux`; all parser/registry/history
  logic is unit-testable **headless with no GPU**.
- Determinism-adjacent ordering: help/completion sort names; no dependence on
  `HashMap` iteration order.

## Performance Targets

- Toggle overlay open/close: < 1 ms (no world work on open).
- Parse + dispatch a command: < 0.1 ms for typical arg counts; `spawn` cost is
  dominated by the ECS spawn it wraps.
- Append a log line: O(1) amortized; the viewer renders only the visible
  window of entries (virtualized, per spec `11`), so scrolling 100k entries
  stays interactive.
- History persist: bounded to N=1000 entries, written once on exit and at most
  every 5 s — never on the hot path.
- Closed overlay idle cost: ≈ 0 (only the shared log ring is written).

## Testing Strategy

- **Unit (headless):** tokenizer (`name arg1 arg2`, quoted args, empty input);
  built-in dispatch table resolves every name; unknown name produces a `warn`
  + suggestion.
- **Mutating-via-commands:** `spawn <components>` / `delete <entity>` /
  `load <scene>` push the correct spec `23` `Command`s (assert via a mock
  `EditorCommandBus` that records pushes) and issue **no** world write outside
  the bus.
- **Append-only:** after `clear` and arbitrary command output, assert the
  backing log grew only by `push` (capture lengths/generation, never in-place
  mutation).
- **History:** Up/Down walk order; draft slot preserved; after N executes the
  ring is clamped to N; serialize → deserialize round-trips and reloads.
- **Level filter + search:** a mixed info/warn/error corpus filters correctly by
  `min_level` and substring search.
- **No-GPU invariant:** every parser/registry/history test runs headless in CI;
  egui rendering is behind a window feature with a stub.
- **Plugin command registration:** a spec `29` test-only plugin registers a
  command; assert it appears in `help` and executes — proving the extension
  seam, not just built-ins.

## Dependencies

- `crates/editor` (egui overlay + shared `SelectionModel` / `WorldView` from
  specs `07`/`08`), `crates/ecs` (via the safe read path).
- `EditorCommandBus` + undoable `Command` types from spec `23`; panel placement
  / shell from spec `25`; command registration hook consumed by spec `29`.
- `log` records and `RecoverableError` surfacing from spec `11` feed the log
  viewer.
- `serde_json` for `console_history.json` under `OPENENGINE_CONFIG_PATH`.
- No new `contracts`/ABI surface is required.

## Next Steps

1. Implement `LogLevel`, `LogEntry`, `CommandSpec`, and the append-only buffer.
2. Implement the tokenizer + dispatch and the `spawn`/`delete`/`select` built-ins
   routing through `EditorCommandBus`.
3. Implement history ring + `OPENENGINE_CONFIG_PATH` persistence.
4. Implement Tab completion over sorted command/component names.
5. Wire the egui overlay + color-coded/filterable log viewer into spec `25`.
6. Add the spec `29` registration hook and `play`/`pause`/`stop`/`save`/`load`
   /`undo`/`redo`/`quit` built-ins end to end.
