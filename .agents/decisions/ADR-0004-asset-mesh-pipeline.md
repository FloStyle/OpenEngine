# ADR-0004: Asset & mesh pipeline — replace analytic spheres

---
id: "ADR-0004"
title: "Asset & mesh pipeline: internal MeshAsset format + AssetRef; host loads, Domain B stays pure"
status: "Proposed"
date: "2026-09-04"
phase: "Phase 5 — groundwork (decide, don't implement)"
---

## Context

The viewport currently draws every entity as an analytic sphere + a checkered
ground. To be an AAA/UE-like engine it must render authored meshes and assets,
with a cacheable, versioned pipeline (spec 02 / 26), cross-platform, headless-CI
friendly, and without breaking Domain B purity or determinism.

## Decision

- **Internal `MeshAsset`**: a runtime-versioned vertex+index format (positions,
  normals; optional uvs/tangents) produced by a **host-side importer** from an
  authored source (e.g. `.obj` now; glTF later). Meshes live in Domain A only.
- **Referencing**: components hold `AssetRef { id, kind }` (already in
  `contracts`) — never raw paths in gameplay. The host `AssetRegistry` maps an
  id to loaded GPU/mesh data.
- **Domain B stays pure**: logic never touches files or mesh data; it operates on
  components/`WorldDelta`. Mesh/asset loading, caching, uploads are Domain A.
- **Collision/primitives**: authored components keep primitive shape
  (AABB/sphere) for deterministic logic; the mesh is for rendering.
- **Renderer**: `SceneRenderer` gets a mesh path (instance transform+material)
  replacing the per-entity sphere, keeping one depth + camera pipeline; a
  `Material` (flat diffuse color) mirrors the current shader so no big render
  rework.
- **Pipeline (spec 02)**: `ResourceRequest/ResourceReady` handshake; a cache keyed
  by `AssetRef`; assets are data files staged next to scenes (no hardcoded paths —
  `OPENENGINE_ASSETS_PATH`).

## Consequences

- Entities stop being uniform spheres → authored, instanced meshes.
- Determinism/purity preserved (mesh is render-only; logic uses primitives).
- Versioned `MeshAsset` format allows migration and matches spec 16 versioning.
