# ADR-0003: Physics approach — deterministic Domain-B runtime + optional Domain-A Rapier

---
id: "ADR-0003"
title: "Physics: deterministic home-grown in Domain B (decision), Rapier as optional Domain-A preview"
status: "Proposed"
date: "2026-09-04"
phase: "Phase 5 — groundwork (decide, don't implement)"
---

## Context

OpenEngine must be deterministic (same inputs + tick ⇒ bit-identical
`World::hash()`), keep Domain B `#![no_std]` / `forbid(unsafe_code)` / `[PURE]`
with fixed-point only, run headless in CI, and scale to large scenes. The editor
and any AI rely on the determinism gate and typed errors as the debugging file.

Options for runtime physics:
- **Rapier** (mature, Domain A std, float) — great features, but it is
  Domain A / `f32` / non-deterministic by default (float ordering), heavy, and
  cannot live in Domain B. Bridging it into pure gameplay would leak floats into
  the logic or require an awkward host-owned physics producing deltas.
- **Home-grown deterministic physics** in Domain B (fixed-point): AABB/approx
  collision, gravity + integration (already partly in `gameplay_tick`),
  resolves deterministically and stays `[PURE]`. Less featureful.

## Decision

- **Runtime / gameplay physics = deterministic, home-grown, in Domain B**
  (fixed-point): integrate the existing gravity/jump into a small `physics`
  module (AABB vs AABB + ground, fixed-point resolve). This keeps the
  determinism law, purity, and headless CI intact, and matches the current
  `gameplay_tick` style.
- **Rapier is allowed later ONLY as an optional Domain-A preview/editor layer**
  (non-gameplay, presentation/feel, e.g. in the shell for testing), never inside
  the pure logic or the determinism gate.
- Collision **authoring** stays primitive (AABB/sphere per component) so it is
  serializable and deterministic; no runtime mesh-convex decomposition.

## Consequences

- Physics results are reproducible bit-for-bit and verifiable by the engine.
- A future full-physics upgrade would swap the Domain-B module (logic is
  reloadable Wasm) without touching the engine core.
- Rapier, if added, is gated behind a Domain-A feature and excluded from the
  determinism/purity story.
