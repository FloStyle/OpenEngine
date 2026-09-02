---
spec: "35-notifications"
phase: "Phase 5: Editor"
status: "draft"
author: "OpenEngine AI"
created: "2026-09-03"
depends_on:
  - "06-scene-management"
  - "23-undo-redo"
  - "26-asset-browser-ui"
  - "27-console"
  - "30-project-playground"
---
# 35 - Notifications

## Overview

The notification system is the editor's way of telling a human (or an agent
watching the UI) *something happened* — without stealing their work. It
centralizes every transient message, from a successful save to a failed import to
an asynchronous compile, into one place with one vocabulary. Four jobs share this
spec:

1. **Toast notifications** — short, non-blocking, self-dismissing messages
   (`info` / `success` / `warning` / `error`) that stack in the **bottom-right
   corner** and go away on a timer or by clicking their ✕.
2. **Notification center** — a persistent, filterable history of everything that
   was notified, plus a **clear** action, so a user can review what happened
   after a burst of background work.
3. **Progress indicators** — for longer asynchronous work (asset **import**,
   scene **save**, logic **compile**/reload), a dedicated non-blocking progress
   row with a bar/spinner and a cancel where supported.
4. **Blocking dialogs** — for decisions the editor cannot proceed without
   (the canonical **unsaved-changes confirm** before closing/discarding a scene or
   project). These are modal and require an explicit choice.

The system is Domain A (`crates/editor`) and purely presentational — it never
mutates the world and never reaches Domain B. The load-bearing rule that ties it
to the rest of the editor: **every `error`/`warning` notification is also written
into the console log (spec `27`)** and, for Domain-B-sourced failures, into the
diagnostics path, so no error is only ever a disappearing toast. Notifications
use host wall-clock only for *display* ordering/timeouts; nothing here feeds
determinism.

## Core Concepts

### Notification levels & kinds

A notification has a **level** (semantics) and a **kind** (how it is presented).
The two are orthogonal so a `warning` can surface as a toast *or* (when it blocks
nothing) simply be logged.

```rust
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum NoticeLevel { Info, Success, Warning, Error }   // ordered for filtering

pub enum NoticeKind {
    Toast(ToastDismiss),       // non-blocking; auto-dismiss or manual
    BlockingDialog,            // modal; requires explicit choice
    Progress(ProgressKind),    // async, cancellable where supported
}
pub enum ToastDismiss { AfterMs(u64), Manual }  // Error defaults to Manual
pub enum ProgressKind { Import, Save, Compile, HotReload } // spec 26/30/12/10
```

Each notification carries a stable `category` string (e.g. `"asset.import"`,
`"scene.save"`, `"logic.compile"`) plus a title and a message. The category drives
the console log grouping and lets the notification center filter or mute
categories.

### The toast stack (bottom-right, non-blocking)

Toasts render as a stack in the bottom-right corner, newest at the bottom. Each
toast shows an icon by level, a title, an optional short message, and a progress
spinner when it is a live `Progress` notice. Behavior:

- **Auto-dismiss** after the toast's `AfterMs` (defaults: info 4 s, success 3 s,
  warning 6 s; **errors are `Manual`** and persist until dismissed or the center
  is cleared — an error must never vanish silently).
- **Manual dismiss** by clicking the toast's ✕ or by a global "dismiss all toasts"
  action. Hovering a toast pauses its auto-dismiss timer.
- **Never steal focus / input.** A toast is a non-interactive overlay except its
  ✕ button; it never grabs keyboard or blocks the underlying editor. This is what
  makes toasts safe for *asynchronous* completion (import, save, compile) that
  arrives while the user is mid-edit.

```rust
pub struct Toast { pub id: u64, pub level: NoticeLevel,
    pub category: String, pub title: String, pub message: String,
    pub kind: NoticeKind, pub created_ms: u64, pub timeout_ms: u64,
    pub dismissed: bool }
```

### Progress indicators (import / save / compile)

Long-running asynchronous work reports its lifecycle as a `Progress` notice
rather than a toast-per-step. A progress notice shows a label, a
determinate/indeterminate indicator, and (when the owning job supports it) a
Cancel button:

- **Import** (spec `26`/`02` background converter) → one `Import` progress row
  per file batch; percent from the spec-`02` job, indeterminate while decoding.
- **Save** (spec `30` project/scene save via spec `16`) → an indeterminate or
  bounded `Save` row; runs off the UI thread so the editor stays responsive.
- **Compile / hot-reload** (spec `12` scripting build + spec `10` reload) → a
  `Compile`/`HotReload` row; a *successful* compile collapses to a short
  `Success` toast, a *failed* compile becomes an `Error` toast + console log.

A progress row is tied to a job token; when the job completes it either converts
to a transient result toast or is removed. Multiple progress rows can coexist
(e.g. importing several assets while an autosave runs). The rows live in the same
bottom-right stack, visually distinct (a bar) from plain toasts.

### Notification center (history + filter + clear)

A persistent **Notification Center** panel (opened from the toolbar bell / a
shortcut, spec `25`/`28`) shows the full history of notifications since the
editor session started (and optionally from a persisted last-session file). It
offers:

- **Filter** by `NoticeLevel` (info/success/warning/error, min-level or any
  combination) and by `category` text search.
- **Clear** — clears the *history list* (removes references) or *dismisses all
  active toasts*; a "clear errors" is a common fast action. Clearing never edits
  the underlying console log (spec `27` append-only) — it only forgets the
  notification view.
- Each row shows level, time-ago, category and message; clicking a row may open
  the related surface (e.g. the console at that log entry, or the asset browser
  folder for an import error).

History is a bounded `Vec` of `NotificationRecord`s (level, category, title,
message, created_ms, console_line_ref) capped at N entries (default 2000) with
FIFO eviction; iteration is over a `Vec` (deterministic). Nothing here is world
state.

### Blocking dialogs (the unsaved-changes confirm)

Some decisions cannot proceed without an answer. The canonical case: closing a
scene or project (or switching default scene) when the edit world is dirty
(spec `23` `is_dirty`, spec `30`/`06`). Blocking dialogs are **modal**: they
pause the invoking action and offer explicit choices as buttons.

```rust
pub struct BlockingDialog {
    pub title: String,            // "Unsaved changes"
    pub message: String,          // "Save changes to MyScene before closing?"
    pub choices: Vec<DialogChoice>,// e.g. [Save, Don't Save, Cancel]
    pub danger: Option<DialogDanger>, // none | Destructive(default-selected cancel)
}
pub struct DialogChoice { pub label: String, pub action: DialogAction }
pub enum DialogAction { Save, Discard, Cancel, Ok }
```

The shell drives the editor as a small state machine: when a blocking dialog is
open, normal input is queued/held and only the dialog's buttons are active. This
is deliberately the *only* modal, focus-stealing presentation in the system, and
it is reserved for irreversible/proceeding decisions (unsaved-changes, discard a
destructive asset delete confirm). Everything else stays non-blocking.

### Errors surface in the console too

The invariant that keeps errors auditable: **a `warning`/`error` notification
is always mirrored to the console log (spec `27`)** through the shared log sink.
The notification carries the `LogEntry` line reference (spec `27` `LogEntry`) so
the console and the notification center reference the same event. Errors
originating from Domain B come through the existing `RecoverableError`/diagnostics
path (spec `11`/`27`) and are *raised as notifications* from there — never the
reverse (a notification never fabricates a log line on its own; it routes real
events).

```rust
// A single sink both the console and the notice center read from is out of
// scope here; instead a NotificationCenter writes Warning/Error also through the
// Console (spec 27) append-only log, storing the back-ref on each record.
pub struct NotificationRecord { pub level: NoticeLevel, pub category: String,
    pub title: String, pub message: String, pub created_ms: u64,
    pub console_line: Option<usize> /* index into spec-27 log_entries */ }
```

### Lifecycle

The `NotificationCenter` is a Domain-A struct owned by the editor (spec `25`). It
accepts `post(...)` calls from any editor subsystem (asset pipeline, scene save,
script compile, drag/drop spec `33`, context-menu spec `34`, preferences spec
`32`, console spec `27`). Each frame the shell renders active toasts/progress/
dialogs and advances dismiss timers. `post` is cheap and safe to call from a
background job thread via an internal bounded queue drained on the editor thread
(reusing the spec-`02` async-drain contract) so no subsystem blocks or races on
notification delivery.

## Key Rust Types

```rust
// crates/editor/notifications — Domain A
pub enum NoticeLevel { Info, Success, Warning, Error }
pub enum NoticeKind { Toast(ToastDismiss), BlockingDialog, Progress(ProgressKind) }
pub enum ToastDismiss { AfterMs(u64), Manual }
pub enum ProgressKind { Import, Save, Compile, HotReload }

pub struct NotificationCenter {
    pub toasts: Vec<Toast>,              // active bottom-right stack (newest last)
    pub history: Vec<NotificationRecord>,// bounded history for the center panel
    pub progress: Vec<ProgressNotice>,   // active async rows
    pub blocking: Vec<BlockingDialog>,   // modal queue (normally ≤1)
    queue: Vec<PostedNotification>,      // bounded channel drained each frame
}
impl NotificationCenter {
    /// Post a notification; safe to call from any (editor or worker) thread.
    pub fn post(&self, n: PostedNotification);
    pub fn toast(&mut self, level: NoticeLevel, category: &str, title: &str, msg: &str);
    pub fn progress(&mut self, kind: ProgressKind, job: u64) -> u64 /* token */;
    pub fn set_progress(&mut self, token: u64, frac: Option<f32>);
    pub fn finish_progress(&mut self, token: u64, ok: bool, message: &str);
    pub fn show_blocking(&mut self, d: BlockingDialog);
    pub fn dismiss_toast(&mut self, id: u64);
    pub fn filter_history(&self, min_level: NoticeLevel, q: &str) -> Vec<&NotificationRecord>;
    pub fn clear_history(&mut self); pub fn clear_all_toasts(&mut self);
}
pub struct Toast { pub id: u64, pub level: NoticeLevel, pub category: String,
    pub title: String, pub message: String, pub kind: NoticeKind,
    pub created_ms: u64, pub timeout_ms: u64, pub dismissed: bool }
pub struct ProgressNotice { pub id: u64, pub kind: ProgressKind, pub label: String,
    pub frac: Option<f32>, pub cancellable: bool }
pub struct BlockingDialog { pub title: String, pub message: String,
    pub choices: Vec<DialogChoice>, pub danger: Option<DialogDanger> }
pub struct DialogChoice { pub label: String, pub action: DialogAction }
pub enum DialogAction { Save, Discard, Cancel, Ok }
pub enum DialogDanger { Destructive }
pub struct NotificationRecord { pub level: NoticeLevel, pub category: String,
    pub title: String, pub message: String, pub created_ms: u64,
    pub console_line: Option<usize> }
pub struct PostedNotification { pub level: NoticeLevel, pub kind: NoticeKind,
    pub category: String, pub title: String, pub message: String }
```

`posted` messages and the internal queue are plain `serde`-able values so a
session's history can persist under the project/config location if desired
(spec `30`).

## Components

None — editor/UI tooling only; no new ECS component. Toasts, progress rows,
blocking dialogs, history and the notification center are pure Domain-A
presentation structs; they are never registered components, never stored in the
world, and never reach Domain B.

## Constraints

- **Domain A only.** `NotificationCenter` and all notice rendering live in
  `crates/editor`; never compiled into the guest, never touch ECS storage.
- **Non-blocking by default.** Toasts and progress rows never steal focus, never
  block input, and never appear over a modal decision the user has not made.
  Blocking dialogs are the **sole** intentional modal and are reserved for
  proceed/irreversible decisions (unsaved-changes, destructive deletes).
- **Every warning/error is mirrored to the console** (spec `27`) and stored with
  a console back-reference; an error is never only a transient toast. Domain-B
  errors arrive through the `RecoverableError`/diagnostics path and are raised as
  notifications there, not synthesized.
- **Errors persist until dismissed.** `Error` toasts default to `Manual` dismiss;
  they are not auto-cleared silently.
- **Async-safe posting.** `post` is thread-safe via a bounded queue drained on
  the editor thread (spec-`02` contract); no subsystem blocks or races on it.
- **Host clock for display only.** `created_ms`/timeouts are host wall-clock for
  ordering/timeouts and never enter determinism or committed world data.
- **Bounded, deterministic history.** History is a capped `Vec` (FIFO eviction);
  filtering iterates a `Vec` — no `HashMap` order.
- **Portable.** No hardcoded paths; any persisted history lives under
  project/config locations (spec `30`). Compiles on `x86_64-linux` and
  `aarch64-linux`; center logic (post/drain/filter/dismiss) is headless-testable
  with no GPU and no window.

## Performance Targets

- **post/drain:** posting enqueues O(1) to a bounded queue; per-frame drain +
  timer advance is O(active notices) — **< 0.1 ms** typical.
- **Toast render:** bottom-right stack paints only visible notices —
  negligible at editor frame rates.
- **Filter/clear:** over a bounded 2000-record history, filtering is < 1 ms;
  clear is O(1) references drop.
- **Progress updates:** `set_progress` updates one row O(1); a compile/import job
  posting at most a few updates/sec adds no measurable overhead.
- **Idle cost:** with no active notices, the center adds ~0 per frame.

## Testing Strategy

All headless (no GPU / no window) in `crates/editor`:

- **Lifecycle:** post → toast appears; auto-dismiss fires after `AfterMs`; a
  `Manual` error toast is not auto-dismissed; ✕ dismisses; hover-pause holds the
  timer (assert via a fake clock).
- **Progress:** start/progress/finish transitions; `finish(ok=true)` yields a
  `Success` result; `finish(ok=false)` yields an `Error` toast *and* a console
  record; indeterminate vs determinate states.
- **Blocking dialog:** `show_blocking` holds the invoking action until a choice;
  choices dispatch the right `DialogAction` (Save/Discard/Cancel); the danger
  choice is never the default.
- **Console mirroring:** every `Warning`/`Error` post produces a spec-`27`
  `LogEntry` and the record's `console_line` references it; `Info`/`Success` do
  not (unless configured).
- **Filter/clear:** a mixed corpus filters correctly by min-level and category
  text; clear removes records/active toasts without touching the spec-`27` log.
- **Async safety:** from a worker thread, burst-post N notices; assert the
  editor-thread drain delivers all N in order with no loss/panic and bounded
  queue (no unbounded growth under a flood).
- **Unsaved-changes integration:** dirty the edit world (spec `23` `is_dirty`),
  request close, assert a blocking dialog offers Save/Discard/Cancel and that
  Save flushes via spec `16` while Cancel aborts the close (spec `30`/`06`).
- **Determinism:** assert notification activity never changes the edit world's
  `WorldHash` (spec `16`) and never produces a delta.

## Dependencies

- `crates/editor` (Domain A) — `NotificationCenter`, toast/progress/dialog
  rendering; mounted into the shell (spec `25`).
- Console append-only log + `LogEntry` from spec `27` (mirror sink);
  `RecoverableError`/diagnostics from spec `11` (Domain-B error surfacing).
- Import jobs from spec `26`/`02`, save from spec `30`/`16`, compile/hot-reload
  from spec `12`/`10`; unsaved-changes dirty state from spec `23`/`30`/`06`.
- Toast-emitting callers: drag/drop (spec `33`), context menus (spec `34`),
  preferences (spec `32`).
- No new `contracts`/ABI surface.

## Next Steps

1. Define `NoticeLevel`/`NoticeKind`/`Toast`/`ProgressNotice`/`BlockingDialog`
   and the notification model.
2. Implement `NotificationCenter` (post/drain queue, toast stack, dismiss
   timers, bounded history) and its headless lifecycle tests.
3. Wire the spec-`27` console mirror for `Warning`/`Error` and the
   `console_line` back-reference.
4. Build the toast + progress row rendering (bottom-right stack) and the
   Notification Center panel (history, filter, clear) into the shell (spec `25`).
5. Implement `show_blocking`/the modal choice flow and wire the unsaved-changes
   confirm into scene/project close (spec `06`/`30`/`23`).
6. Land the async-safety, determinism, and integration test battery.
