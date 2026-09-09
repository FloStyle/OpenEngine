# ADR-0005: Plugin boundary & tool reload (design decision)

---
id: "ADR-0005"
title: "Plugin boundary: in-process Plugin trait now; ServiceRegistry typed; dylib reload as a later transport"
status: "Proposed"
date: "2026-09-04"
phase: "Phase 3 — plugin foundation (in-process done; dylib = design only)"
---

## Context

Per the vision, **everything is a plugin except the Anvil** (the headless core).
Two tiers were agreed: logic = Wasm (done, reloadable, `[PURE]`); tools/UI start
as an **in-process trait behind a clean boundary**, and native dylib hot-reload
transport comes later. `egui` stays host-side — plugins *describe* UI, they never
own the framework. The gizmo must itself become a plugin.

Rust constraints: a heterogeneous, fully generic service registry without
`Any`/downcast is not possible today (no specialization). We choose a typed,
host-owned registry instead, and keep plugin code concrete.

## Decision

1. **In-process plugin boundary** (`crates/plugin-host`, implemented):
   - `trait Plugin { id(); init(&FrameCtx); update(&FrameCtx); deinit(); }`.
   - `PluginHost` loads/initializes/drives/unloads plugins (id-unique).
   - Lifecycle is headless-tested. Tools (start with the gizmo) live behind this
     boundary.
2. **ServiceRegistry is typed and host-owned** — NOT a generic `Any` map. The
   host knows its services (ecs, render, input, ui-primitives); it passes them to
   plugins as concrete typed values via `init`/`update` args or typed getters on
   a host struct. Plugin code therefore never downcasts.
3. **UI is host-owned**: plugins *request/describe* UI primitives through the
   boundary; `egui` is not exposed as an object plugins can own.
4. **Native dylib reload**: designed, NOT implemented. A future transport would
   load a `cdylib` exposing a C-ABI `openengine_plugin_*` register function into
   a `PluginHost`, mirroring the Wasm story. Requires re-exporting the `Plugin`
   trait ABI (versioned) — a later ADR when a tool genuinely needs hot reload.
5. **`logic.wasm` reload stays the primary live-reload path** (already reloadable
   via `scripts/build.sh` + `openengine_runner`/harness `/load_wasm`).

## Consequences

- Tools become individually loadable/testable and later hot-reloadable without
  touching the Anvil.
- No `Any`/downcast in the boundary; typed host services keep determinism and
  compile-time checks.
- Dylib transport is deliberately deferred (complex/risky) until a real tool
  needs it.
