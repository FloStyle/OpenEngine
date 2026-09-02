# OpenEngine — Audit Report (Phase 3 / Pre-Implementation)

---
audit: "audit-report-phase3"
date: "2026-09-03"
scope: "51 specs (00-50) + AGENTS.md + contracts/src/lib.rs"
method: "5 parallel read-only auditors (00-10, 11-20, 21-29, 30-40, 41-50), each against canonical AGENTS/contracts/spec-21 registry/spec-22/23; then consolidated by the architect."
---

## Executive summary

**Coherence score: 6.5 / 10** — the foundation (Domain A/B split, single `WorldDelta`/Command mutation channel, determinism law, edit/play intent, headless testing, sandbox purity) is coherent and strong across nearly every spec. The defects are **cross-spec conceptual conflicts** and **registry/layout drift**, concentrated in the physics/netcode/serialization, the editor cluster, and the newest subsystems (animation, advanced physics, build). No spec rewrites the immutability contract; most fixes are surgical edits.

**Issue counts (deduplicated conceptual):**

| Severity | Count |
|----------|-------|
| CRITICAL | 1 |
| MAJOR | ~24 |
| MINOR | ~28 |

**Top 5 risks**
1. **Spec 50 claims the native wgpu/winit/wasmtime host compiles to a `wasm32`/wasmtime-compatible host shell for the Web** — infeasible and contradicted by spec 20 (CRITICAL).
2. **Single-world editor model (spec 06) vs edit-vs-play isolation (spec 22) conflict**, plus spec 25 vs 22 on play-time-edit rollback and a `Paused`-state model that spec 25 cannot represent.
3. **Two parallel mutation paths**: spec 00 exposes `QueryMut` "editor may write directly", conflicting with the one-channel rule used by 07/08/09/23.
4. **Registry ↔ code drift**: spec 09 writes 3D gizmo drags into the 2D `Position` (id 0); physics (13) components unregistered; several `size_of` claims (41/48) and the `AnimatorController` (37) multi-layer layout are wrong.
5. **Undefined ABI surface that specs depend on**: netcode wire/rollback types (15), audio carrier + `AssetHandle` (14), per-component migration versioning (16), phantom `AssetRefLike`/`FixedStringLike` (46/47), `Bounds`/AABB component (24) — none exist in `contracts`/spec-21.

**Recommendation: FIX FIRST (targeted edits, ~4-6 days of doc work), then implement from spec 00/21/16.** Not a rework; do not proceed to code with the top CRITICAL and the registry/code contradictions open.

---

## 1. Architectural coherence

**Status: ⚠️ WARNING**

| # | Severity | Spec(s) | Issue | Evidence | Recommendation |
|---|----------|---------|-------|----------|----------------|
| 1 | CRITICAL | 50 | Host shell "compiled to wasm + wasmtime-compatible … served via wasm-bindgen/WASI" | spec 50 Platform targets | Host is native on every desktop target; only `logic.wasm` (Domain B) is wasm32. Web requires a separate host re-architecture (wgpu WebGPU, no wasmtime). Reword spec 50. |
| 2 | MAJOR | 06 | Editor edits the same `World` the `GameLoop` simulates | spec 06 "Editor current scene" | Route all editor writes through a distinct edit world (spec 22) |
| 3 | MAJOR | 00 | `QueryMut` direct-write for editor = second mutation path | spec 00 §Queries | Forbid `QueryMut` as an editor write path; all writes via `WorldDelta`→`apply_delta` |
| 4 | MAJOR | 25 vs 22 | "Stop rolls back edits made while playing" contradicts spec 22 (play never mutates edit world; no undo entries) | spec 25 | Align stop semantics to spec 22 |
| 5 | MAJOR | 22 vs 25 | Mode model mismatch: 22 `EditorMode{Edit,Playing,Paused}` vs 25 `EditMode{Edit,Play}` — Paused unrepresentable | specs 22/25 | Unify one editor mode enum |
| 6 | MAJOR | 49 | "several Collider(s) on one entity" violates once-per-archetype (spec 21) | spec 49 | Extra colliders = child/table entities or single Collider fields |

## 2. Component registry consistency

**Status: ❌ FAIL (code drift; registry table itself OK)**

The spec-21 registry (base 0–9 + Extended 10–69) is internally unique and non-conflicting; the audit verified **all** 36–50 ids and the reuses of Transform(2)/Parent(4)/Camera(7)/Light(8). Defects are where specs **reference components that the registry never defines**, or **claim wrong layouts**:

| # | Severity | Spec(s) | Issue | Recommendation |
|---|----------|---------|-------|----------------|
| 7 | MAJOR | 09 | Gizmo writes 3D `FxVec3` drag into 2D `Position` (id 0, 8B) instead of `Transform` (id 2) | Point emission at `C_TRANSFORM`; whole-element write |
| 8 | MAJOR | 13 | `Aabb/Circle/CollisionFilter/PhysicsMaterial/Mass` have **no** ComponentId in spec 21 (physics band only assigned to spec 49: 60–63) | Allocate stable ids ≥10 distinct from 60–63; reconcile 13 vs 49 claim on the physics band |
| 9 | MAJOR | 13 | Prose `Collider` column never resolves; shape scheme waffles (tagged union vs two archetypes) | Define one canonical collider component set |
| 10 | MAJOR | 24 | Picking needs a "Bounds/AABB component (spec 21)" that spec 21 never registers | Register a `Bounds` id or define the AABB data source |
| 11 | MAJOR | 41 | `size_of` table impossible (LightProbe "32 B" but struct ≥44 B; ReflectionProbe/LightmapSettings also wrong) | Recompute from actual `repr(C)`; add layout asserts |
| 12 | MAJOR | 48 | `ScriptNodeGraph` declared 48 B; struct is 56 B | Fix size/padding; pin with assert |
| 13 | MAJOR | 37 | `AnimatorController` stores base-layer state + `layer_weights`, but claims independent per-layer ASM state | Add per-layer `[state/time/cross_fade; MAX_LAYERS]` arrays |
| 14 | MAJOR | 46/47 | Phantom `AssetRefLike`/`FixedStringLike` replace canonical `AssetRef`/`FixedString` (spec 21) | Reuse canonical types; drop `*Like` |
| 15 | MINOR | 04 | Render struct sketches diverge from spec 21 (AssetHandle vs AssetRef, `visible:bool`, undefined `Camera.clear`) | Point spec 04 at spec 21 |
| 16 | MINOR | 08 | References `Children` component absent from registry | Register a `Children` id or drop the mention |
| 17 | MINOR | 36 | `AnimationClip` pad comment says 12 B but `u64` reserved forces 16 B | Use `[u8;4]` reserved; size_of assert |
| 18 | MINOR | 49 | `RigidBody` prose stores orientation but struct has no θ; embeds `position: Position` alongside live `Position`/`Velocity` columns (two sources of truth) | Add orientation field; one canonical position column |

Layout audit note: base + extended registry numeric correctness is good; several `size_of`/`align_of` docs need a lock-test pass (a CI `static_assert` module per spec 21).

## 3. Dependency graph integrity

**Status: ⚠️ WARNING**

| # | Severity | Spec(s) | Issue | Recommendation |
|---|----------|---------|-------|----------------|
| 19 | MAJOR | 29 | `depends_on` cites nonexistent `26-scene-serialization`; scene serialization is spec 16/06 | Fix to `16-serialization` |
| 20 | MAJOR | 46 | Body delegates to 36/37/38 and even claims they're "not in the tree", but omits them from `depends_on` | Add 36/37/38; remove stale statement |
| 21 | MINOR | 41 | Body uses spec 13/42 discipline but `depends_on` omits 13/42 | Add 13, 42 |
| 22 | MINOR | 37/38/39 | `depends_on` omits 02-asset-pipeline / 16 / 13 / 42 required by their bodies | Add them |
| 23 | MINOR | 24 | Body uses 22/07/08 but `depends_on` omits them | Add |
| 24 | MINOR | 42↔43, 44↔45 | Front-matter cycles (42↔43, 44↔45) | Keep `depends_on` a DAG; forward-ref in body only |
| 25 | MINOR | 27↔29 | Circular console↔plugin dependency | Pick one direction for front-matter |
| 26 | MINOR | naming | Spec 06 + some call spec 16 **"16-save-load"**; file is `16-serialization` (specs 11–20/22/23 all use `16-serialization`) | Rename refs to `16-serialization` |

Also: 36–50 files now exist, so no spec depends on a truly missing file except the mis-named `26-scene-serialization` in spec 29.

## 4. Technical feasibility

**Status: ⚠️ WARNING (one FAIL)**

| # | Severity | Spec(s) | Issue | Recommendation |
|---|----------|---------|-------|----------------|
| 27 | MAJOR | 23 | Crash-recovery needs round-tripping `Vec<Box<dyn Command>>`; no tagged/enum reconstruction scheme (postcard can't generically serialize dyn trait objects) | Define a tagged enum Command encoding / registry of CommandKinds |
| 28 | MAJOR | 15 | Rollback/wire types (`InputBatch`, `LocalInput/RemoteInput`, `WorldHash`, `Addr`, `NetError`) referenced but never defined | Define concrete layouts in `contracts` + `docs/abi` |
| 29 | MAJOR | 16 | Migration assumes a per-component `layout_version` none of spec-21's Pod components carry; global `abi_version` can't describe component-local drift | Add explicit per-component schema/layout version + relocation rules |
| 30 | MAJOR | 13 | Overlap sample squares in `I16F16` (overflow) yet prose says to widen to `I32F32` | Show widening in code / integer sub-cell units |
| 31 | MAJOR | 13 | Deterministic integer `sqrt` referenced but `openengine-math` defines none | Add deterministic integer-sqrt + cross-platform identity tests |
| 32 | MINOR | 46 | `union KeyValue` access is `unsafe` (forbidden in Domain B); one member can't hold pos+rot on a transform keyframe | Use `[I16F16;4]+kind`; define pos+rot co-authoring |
| 33 | MINOR | 39 | GPU/CPU byte-agreement claim is not headless-testable (GPU is device-only) | Restrict determinism guarantee to CPU path / software-wgpu |
| 34 | MINOR | 36 | "Slerp/EaseOut need transcendentals" — only Slerp does (EaseOut is cubic Hermite) | Reword |

## 5. Testability

**Status: ✅ PASS (with two fixes)**

All specs state headless/no-GPU unit+integration plans and, where relevant, 3× determinism and purity `[PURE]`. Fixes:
- MAJOR — the determinism-3× CI command `cargo test --workspace determinism -- --exact` runs **zero** tests (`--exact` needs an exact "determinism" name). Drop `--exact` or assert ≥1 executed (specs 17/20).
- MINOR — GPU-particle reproducibility (spec 39) isn't headless; reword (see finding 33).

## 6. Completeness (standalone engine)

**Status: ⚠️ WARNING**

- MAJOR (49): rigid-body orientation storage missing; duplicated linear-position storage.
- MINOR (36/37): playback/runtime stubs return `Ok(WorldDelta::default())` with no `RecoverableError` paths for missing `Skeleton`, `joint_count` drift, invalid state/entry handling.
- MINOR (47): layout/hit-test ownership between Domain B and A unspecified — no defined mechanism carries layout rects into the next Domain-B tick.
- Missing/underspecified workflows worth one pass before implementation: editor "new empty project" onboarding, "play preview with a different camera", save-invariant/unsaved-warning wiring to spec 35, and a canonical error-toast mapping.

## 7. Security & sandboxing

**Status: ⚠️ WARNING**

- **Domain B purity is preserved everywhere** audited — no fs/net/threads in Domain B, no f32 in logic math, no `unsafe` in Domain B, exports confined to `logic-export`, `verify-wasm-purity` as a gate. ✅
- MAJOR (29): "hostile plugin can never crash the editor" is **false for in-process native `libloading` host plugins** — `catch_unwind` cannot contain segfaults/UB. Only the logic-wasm path is truly isolated. Recommend: host plugins are trusted (code-review gated) or run out-of-process; the isolation guarantee applies to logic plugins only.
- MINOR (46): `union` keyframes would need an `#[allow(unsafe_code)]` module — flag per AGENTS unsafe policy (Domain A only).

## 8. Performance coherence

**Status: ⚠️ WARNING**

- MAJOR (13): physics budget "≤16 ms/tick" contradicts the engine's 8 ms fixed-update cap (spec 15/11) and the 256 MB figure vs rollback-window memory.
- MINOR (10): dev-only reloader takes a `Mutex` on the fixed-update path — acceptable if debug-gated and compiled out of release; prefer lock-free.
- No other hot-path `Mutex` or hot-path allocation found. Individual per-frame budgets are otherwise coherent.

## 9. Serialization & persistence

**Status: ⚠️ WARNING**

- Strong: all spec-21 components are `serde` + postcard codec; world/scene share one codec (spec 16); navmesh/terrain/baked-lightmap persistence documented; editor state (project/prefs/shortcuts/layouts/undo) JSON.
- MAJOR (16): per-component `layout_version` migration story is unimplementable as written (see #29) — **the single biggest persistence blocker**.
- MINOR (16): `SaveMeta` referenced but never defined; `sim_time: I16F16` (16 integer bits) can overflow long sessions.
- MINOR (15): rollback-buffer tick width `u32` vs `u64` elsewhere.

## 10. Error handling

**Status: ⚠️ WARNING**

- Domain `RecoverableError` + `WorldDelta` semantics are used consistently; scene/load error types and spec-23 inverse deltas exist.
- MAJOR naming drift: host editor error type differs across specs (`EditorEditError` 07 vs `EditorError` 23 vs `ModeError` 22/25); `SceneError` never actually defined in 21–29. Unify into one host `EditorError` + one `SceneError`.
- MINOR: spec 01 `fixed_update` uses bare `?` with no recovery/message path for `RecoverableError` (should roll back delta + surface to user).
- MINOR: "roll back that tick's partial delta" (spec 11) is ill-defined under single-`Result` returns (on `Err` no delta exists).

---

## Component registry audit (consolidated)

| Id | Component | Source spec | Status |
|----|-----------|-------------|--------|
| 0 | Position (2D) | 21 | OK |
| 1 | Velocity | 21 | OK |
| 2 | Transform | 21 | OK (reused by 09/46…) |
| 3 | Name | 21 | OK |
| 4 | Parent | 21 | OK (reused by 47 UI tree) |
| 5 | Sprite | 21 | OK |
| 6 | MeshRenderer | 21 | OK |
| 7 | Camera | 21 | OK (reused by 46) |
| 8 | Light | 21 | OK (reused by 41; LightKind::Area = ABI v3) |
| 9 | Tag | 21 | OK |
| 10–14 | Skeleton/AnimationClip/AnimationPlayer/SkinnedMeshRenderer/AnimatorController | 36/37 | OK (14 stores per-layer gap → fix) |
| 15–19 | reserved animation | | OK |
| 20–23 | ParticleEmitter/ParticleModule/PostProcessVolume/PostProcessSettings | 39/40 | OK |
| 24–26 | ReflectionProbe/LightProbe/LightmapSettings | 41 | OK (size_of wrong → fix) |
| 27–29 | reserved VFX | | OK |
| 30–39 | Terrain/TerrainLayer/FoliageType/FoliageInstance/BehaviorTree/Blackboard/AIAgent/NavMesh/NavAgent/NavObstacle | 42–45 | OK |
| 40–49 | reserved env/AI | | OK |
| 50–56 | Sequence/SequenceTrack/SequenceKeyframe/UICanvas/UIElement/UIText/UIButton | 46/47 | OK (UI uses no `String` in Pod) |
| 57–59 | reserved cinematic/UI | | OK |
| 60–63 | RigidBody/Collider/PhysicsMaterial/Joint | 49 | OK (see once-per-archetype + RigidBody fields) |
| 64 | ScriptNodeGraph | 48 | OK (size wrong → fix) |
| 65–69 | reserved advanced | | OK |
| ≥1024 | game/user components | | OK |

**Gaps / unregistered references to fix:** basic-physics colliders of spec 13 (`Aabb/Circle/CollisionFilter/PhysicsMaterial/Mass`) — allocate ≥10, distinct from 60–63; `Bounds` (spec 24); `Children` (spec 08); reconcile the spec 13-vs-49 physics band.

## Dependency graph

Acyclic with the exceptions in finding group 3 (24–26). Full matrix not reproduced here; cross-file links were verified present (all 00–50 files exist) except the mis-named `26-scene-serialization` (29) and stale "36/37/38 not in tree" (46).

## Risk assessment

| Risk | Likelihood | Impact | Mitigation |
|------|------------|--------|------------|
| Wrong `Position`/`Transform` authoring in gizmo/renderer | High | High (data corruption / 2D-3D mismatch) | Fix spec 09 + 04 vs 21 before code |
| Two mutation paths (QueryMut) silently creep into editor | Med | High (bypasses determinism) | Remove/forbid QueryMut write for tools |
| Undefined netcode + serialization schema land first | High | High (blocked impl) | Define contracts layouts (15/16) + abi bump |
| Budget contradictions cause dropped frames | Med | Med | Reconcile 8 ms caps (13/15) |
| In-process host-plugin crash | Low-Med | High | Trust-gate or out-of-process host plugins (29) |

## Recommendations (before implementation)

1. **[CRITICAL]** Spec 50: correct the Web target — host is native; `logic.wasm` is the only wasm artifact; Web is a separate re-architecture. Drop "wasmtime-compatible host shell".
2. **[MAJOR]** Enforce **one mutation channel**: remove `QueryMut` write access for tools (00); route gizmo emission to `Transform` whole-element writes (09); gate all editor writes to the edit world (06/22/25 reconcile + single mode enum incl. `Paused`).
3. **[MAJOR]** Register physics components + `Bounds` + `Children` in spec 21; fix every `size_of`/layout doc (41/48/36/37) and add CI layout-asserts.
4. **[MAJOR]** Define the missing ABI surface in `contracts` + `docs/abi` (ARCH bump): netcode wire/rollback types (15), audio carrier + one asset-id type (14), per-component `layout_version` + relocation (16), canonical `AssetRef`/`FixedString` reuse (drop `*Like`).
5. **[MAJOR]** Reconcile budgets (physics ≤ 8 ms), fix the 3×-determinism CI command to actually run tests (17/20), make animation runtime stubs return real `RecoverableError` codes (36/37), and unify host error types (`EditorError`, `SceneError`).
6. **[MINOR]** Batch: rename `16-save-load`→`16-serialization` refs; fix `depends_on` gaps/cycles (41/37/38/39/24/46, 42↔43, 44↔45, 27↔29); unify `ViewMode`, `AssetKind`, `sim_time` (f64 vs I16F16 vs u64).

**Estimated effort to fix all issues:** ~4–6 days of documentation editing + 2–3 ABI-design decisions (netcode/audio/schema versioning) before the first code commit.

**Recommendation: FIX FIRST → then implement starting from spec 00 (ECS World) → spec 21 (registry) → spec 16 (codec) → spec 22/23 (editor core).**
