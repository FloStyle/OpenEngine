---
spec: "50-build-deploy"
phase: "Phase 5: Advanced"
status: "draft"
author: "OpenEngine AI"
created: "2026-09-03"
depends_on:
  - "02-asset-pipeline"
  - "12-scripting-macros"
  - "16-serialization"
  - "20-ci-cd"
  - "22-edit-vs-play"
  - "48-visual-scripting"
  - "49-advanced-physics"
---
# 50 - Build & Deployment (Shipping Pipeline)

## Overview

This spec describes how OpenEngine goes from source to a **shippable game** —
the release pipeline beyond spec 20's CI/CD. It covers four responsibilities,
all of which are **Domain-A tooling** producing **portable artifacts**:

1. **Build configurations** — `Debug` / `Release` / `Shipping`, with a strict
   code/fidelity budget for each (what gets compiled in, what gets stripped out).
2. **Platform targets** — Linux (`x86_64-linux` + `aarch64-linux`), macOS,
   Windows, and Web (a `wasm32` build that is **wasmtime-compatible** because the
   engine's own Domain-B runtime already runs on `wasm32-unknown-unknown`).
3. **Asset cooking** — converting source assets (PNG/JPG/glTF/OBJ/WAV/OGG/TTF,
   spec 02) into a compact **runtime format**, deterministically, with the
   **`logic.wasm` Domain-B module as a first-class cooked artifact** whose purity
   is re-verified at cook time.
4. **Packaging & distribution** — producing a **single binary + assets**
   package, signing/versioning it with **semver**, and publishing through
   channels (itch.io / Steam / self-hosted) with an **update / patch / delta**
   system.

The pipeline leans on the real repository scaffolding already in place: the
`Dockerfile` (pinned Debian bookworm / `rust:1.85-slim-bookworm`), the
`.github/workflows/ci.yml` CI, and `scripts/build.sh` (the Domain-B → staged
`crates/core/assets/logic.wasm` bridge). This spec extends those with cook,
strip, package, and distribute stages while honoring every portability rule: no
hardcoded paths (`OPENENGINE_*` env vars, `CARGO_MANIFEST_DIR`), no
OS-specific code without an ADR, deterministic output, and offline builds after
an initial `cargo fetch`.

## Core Concepts

### Build configurations

A build configuration selects **optimization profile + what code ships**. The
editor and game runtime share one host binary in Debug/Release (spec 22 keeps
edit and play as two worlds *inside* one editor process); Shipping is a separate
target that **strips the editor and all debug/authoring capability** out of the
delivered artifact.

| Config | Profile | Purpose | What is compiled in / out |
|--------|---------|---------|---------------------------|
| `Debug` | `dev` | day-to-day editor + iteration | full editor, asset hot-reload (spec 02), visual-script debug instrumentation (spec 48) |
| `Release` | `release` | benchmarking, near-shipping with editor | optimized, still carries editor + debug symbols as wanted; used for editor distribution |
| `Shipping` | `release` + `cfg(feature="shipping")` | the player-facing game | **editor, debugger, inspect tooling, debug gizmos, spec 48 stepping, panic-with-messages/backtrace stripped**; minimal player surface only |

**Code stripping** for Shipping is compile-time feature-gated, not post-hoc
binary surgery:

```toml
# crates/editor/Cargo.toml (illustrative)
[features]
default = ["editor"]
shipping = []                 # editor crate still compiles but no-op surface
# …features that the editor, debugger, spec 48 stepping gate on.
```

```rust
// crates/editor/src/lib.rs (illustrative) — the whole editor is compiled out
// under shipping.
#[cfg(not(feature = "shipping"))]
pub fn run_editor(...) { /* full editor (spec 25 shell) */ }

#[cfg(feature = "shipping")]
pub fn run_editor(...) { /* unreachable: game boots standalone world, spec 22 */ }
```

Shipping builds: `cargo build --release --features shipping -p openengine-game`.
A Shipping binary still runs the exact Domain-B simulation (identical
`logic.wasm`), so a shipped game is bit-compatible with the same build in the
editor — determinism (AGENTS.md) is unaffected by stripping, which only removes
host authoring UI and debug observability. Panic handling in Shipping is
`panic = "abort"` (compact, deterministic, no unwinding payloads), whereas Debug
keeps `panic = "unwind"` for diagnostics.

### Platform targets

| Target | Build host | Notes |
|--------|-----------|-------|
| Linux `x86_64-unknown-linux-gnu` | any | primary; wgpu/winit deps from the pinned `Dockerfile` |
| Linux `aarch64-unknown-linux-gnu` | any (cross via the container / QEMU) | portability requirement (AGENTS.md §5) |
| macOS `x86_64-apple-darwin` / `aarch64-apple-darwin` | macOS runners (cross-sign tooling) | universal binary optional; notarization for distribution |
| Windows `x86_64-pc-windows-msvc` | Windows runners | wgpu/winit native deps; MSVC |
| Web `wasm32-unknown-unknown` | any | the game runtime's Domain-B module *is already* this target (spec 20); the **host shell** is compiled to wasm + wasmtime-compatible so it can run under `wasmtime` and be served to browsers via `wasm-bindgen`/WASI. |

The Web story reuses a deep engine property: **Domain B already compiles to
`wasm32-unknown-unknown`** (spec 20 builds `logic.wasm` for that target). A Web
build therefore ships *two* wasm artifacts — the pure logic module and a
host-shell module — and the host-shell is a **wasmtime-compatible** build (WASI /
`wasm32-wasi`-aligned ABI imports), so the exact same deterministic simulation
runs under `wasmtime` in tests and in a served context. Target selection is a
CI/CLI parameter, never baked into a path (portability rule).

### Asset cooking to runtime format

Source assets (spec 02's raw formats) are **cooked** into a compact, versioned
runtime blob at build time by a Domain-A tool (`crates/cook`), then packaged.
Cooking is **deterministic**: the same source bytes + same cook version ⇒ the
same runtime bytes, on every platform. Cooked assets are addressed by their
logical relative path (spec 02/21) resolved against `OPENENGINE_ASSETS_PATH`,
never an absolute path.

```text
source/ (PNG, glTF, OBJ, WAV, OGG, TTF, scene, graph, physics material)
   │  crates/cook (deterministic, offline, feature-selected)
   ▼
cooked/ (runtime blobs: GPU-ready buffers, decoded PCM, packed meshes,
         postcard scene/world columns (spec 16), compiled graph modules)
   │  crates/cook pack
   ▼
runtime/ package → single binary + cooked bundle
```

What gets cooked and into what:

| Source kind | Cooked form | Determinism note |
|-------------|-------------|------------------|
| Texture (PNG/JPG) | GPU-ready image + sampler params (spec 02) | fixed encoder settings, stable ordering |
| Mesh (glTF/OBJ) | packed vertex/index buffers + layout | fixed quantization, sorted by asset id |
| Shader (WGSL) | embedded string / `ShaderModule` blob | byte-identical |
| Audio (WAV/OGG) | decoded PCM samples + meta | fixed resample/encode |
| Font (TTF/OTF) | glyph atlas + metrics | fixed atlas build order |
| Scene / world (16) | postcard `WorldSnapshot`-family columns | postcard is little-endian & non-floating |
| Visual graph (48) | the compiled graph `.wasm` module | compiler is deterministic |

### `logic.wasm` is the Domain-B artifact; purity re-verified at cook time

The Domain-B module built by `scripts/build.sh` (staged to
`crates/core/assets/logic.wasm`, spec 20) is the **single source of gameplay
truth** shipped in every package. At **cook time** (and again at release, spec
20) the pipeline **re-verifies purity** on the exact bytes that will ship:

```bash
# in the cook / release stage, on the artifact, not a rebuild guess:
python3 brain/orchestrator.py verify-wasm-purity \
    <OPENENGINE_WASM_PATH>/logic.wasm     # must print [PURE]
# and check its ABI fingerprint matches the host build:
#   contracts::abi_fingerprint() — a mismatch refuses to load (spec 20).
```

Because spec 48's visual graphs and spec 49's physics are compiled into that
same Domain-B universe, cooking the final `logic.wasm` re-verifies *everything*
gameplay ships with. A cook that produces anything but `[PURE]`, or whose module
`abi_fingerprint` disagrees with the host, aborts the release — never ships.

### Packaging: single binary + assets

Shipping produces a **single host binary plus a cooked asset bundle** (embedded
via `include_bytes!`/a small archive, or shipped as a sibling `.assets` file
resolved relative to the exe — configurable, never an absolute hardcoded path).
Package layout is deterministic and versioned:

```text
OpenEngineGame-v1.2.3/            # semver-tagged
  game                             # (or game.exe / game.app)
  assets.bin                       # cooked bundle (embedded or adjacent)
  logic.wasm                       # the Domain-B module (or inside assets.bin)
  manifest.json                    # version, ABI fingerprint, asset map, signature
```

`manifest.json` (or the packed `FormatHeader`, spec 16) records the semver,
`ARCH_VERSION`/`abi_fingerprint`, and an asset map so the launcher can verify
integrity and drive updates.

### Semver versioning

Versions follow **semver** (`MAJOR.MINOR.PATCH`), aligned to the workspace
`version` and to release tags `v<semver>` (spec 20). The rule that ties
versioning to the ABI:

* **MAJOR** bump ⇔ a breaking `ARCH_VERSION` change (contracts layout/enum, spec
  20/AGENTS.md §2) — a new MAJOR may ship modules that only run against matching
  hosts.
* **MINOR** bump ⇔ additive, non-breaking features (a new built-in node, a new
  joint kind) that keep `abi_fingerprint` compatible.
* **PATCH** bump ⇔ fixes that change no observable contract for existing
  players.

The host refuses to load a `logic.wasm` whose `abi_fingerprint` mismatches, so a
MAJOR/version skew fails loudly rather than corrupting state (spec 20).

### Distribution channels & update / patch / delta

Channels wrap the same package:

* **itch.io** — upload the platform package (per-OS zip); itch hosts versioned
  downloads and the engine can point a "check update" at an itch-provided or
  self-hosted manifest.
* **Steam** — Steamworks build (publishing via SteamCMD); Steam's own patch/
  delta + autoupdate handles delivery.
* **Self-hosted** — a plain HTTPS endpoint serving a versioned `manifest.json` +
  packages; the in-game updater polls it.

**Update / patch / delta** reduces download size between two known versions:

* A **patch** is a byte-level delta from the previous version's cooked
  `assets.bin` / binary, computed deterministically (block hashing over fixed
  64 KB blocks). Only changed blocks transfer. Because cooking is deterministic,
  a patch between two *cooked* artifacts is stable and reproducible.
* The **update manifest** lists, per target and from-version, the delta blocks +
  the new full package hash, so a client from version X fetches only the diff to
  Y and verifies the result equals the full-Y hash.
* **Delta source** must be the exact published cooked artifact, never a
  rebuild of the tag that might differ byte-wise — determinism makes the 
  published bytes the ground truth (spec 20 reproducibility).

## Key Rust Types

```rust
//! crates/build-tools/src/ (Domain A) — a small CLI used by scripts/build.sh,
//! CI, and the cook/package steps. std-only host tooling.

/// One build configuration. Drives feature selection + profile.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BuildProfile { Debug, Release, Shipping }

impl BuildProfile {
    /// cargo `--release` flag and feature toggles for this profile.
    pub fn cargo_args(self) -> Vec<&'static str> { /* debug / release / shipping */ }
    /// Which editor/debug surfaces are compiled in (stripping decision).
    pub fn carries_editor(self) -> bool { matches!(self, Self::Debug | Self::Release) }
}

/// A platform target the pipeline can build/package for.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Platform {
    LinuxX64,
    LinuxAarch64,
    MacOsX64,
    MacOsAarch64,
    WindowsX64,
    WebWasm,
}

/// A cooked asset: source-relative logical token -> runtime blob.
#[derive(Clone, Debug, serde::Serialize, serde::Deserialize)]
pub struct CookedAsset {
    pub logical_path: String,   // relative, no absolute/host path (spec 02/21)
    pub kind: AssetKind,        // Texture | Mesh | Shader | Audio | Font | Scene | Graph
    pub bytes: Vec<u8>,         // deterministic runtime form
    pub hash: [u8; 32],         // block-hash root for delta/update
}

/// The release manifest for one platform build.
#[derive(Clone, Debug, serde::Serialize, serde::Deserialize)]
pub struct ReleaseManifest {
    pub semver: String,            // "1.2.3" aligned to workspace version
    pub arch_version: u32,         // == contracts::ARCH_VERSION
    pub abi_fingerprint: u64,      // host must match logic.wasm (spec 20)
    pub platform: Platform,
    pub profile: BuildProfile,     // Debug/Release are dev uploads; Shipping ships
    pub package_hash: [u8; 32],
    pub assets: Vec<CookedAsset>,  // ordered, stable
    pub signed_by: String,         // signer identity (channels)
}
```

## Components

No new `ComponentId` in 60–69 is claimed by this spec; the reserved built-in
range 60–69 stays partitioned as:

| `ComponentId` | Owner spec |
|---------------|------------|
| 60 `RigidBody` | 49 |
| 61 `Collider` | 49 |
| 62 `PhysicsMaterial` | 49 |
| 63 `Joint` | 49 |
| 64 `ScriptNodeGraph` | 48 |
| 65–69 | **reserved** (unclaimed; this spec keeps the gap so future physics/script/audio built-ins land without renumbering) |

This spec is packaging/tooling and adds no simulation component. If a future
built-in needs a component, it claims the lowest free id in 65–69 (or appends
≥ 70) and updates this reservation note.

## Constraints

- **Domain-A tooling, portable artifacts.** Cook, strip, package, and
  distribute are host (Domain A) operations. Domain B never performs I/O or
  packaging. Artifacts are portable (AGENTS.md §5): compile on
  `x86_64-linux`/`aarch64-linux`, build in Docker/CI, work offline after an
  initial `cargo fetch`.
- **No hardcoded paths / OS-specific code without an ADR.** Everything resolves
  via `OPENENGINE_ASSETS_PATH`, `OPENENGINE_CONFIG_PATH`, `OPENENGINE_WASM_PATH`,
  or `CARGO_MANIFEST_DIR`; package layout is relative to the exe; no `$HOME`
  baked in.
- **Deterministic output.** Same source + same cook/profile/toolchain version ⇒
  same cooked bytes and same package hash, on every platform. Fixed-point,
  `postcard` (little-endian, non-floating), fixed codec/encode settings, stable
  ordering. This is what makes patches/deltas reproducible.
- **`logic.wasm` purity re-verified at cook time and at release** on the exact
  shipped bytes → must print `[PURE]`; `abi_fingerprint` must match the host or
  load is refused (spec 20).
- **Semver discipline.** MAJOR ⇔ `ARCH_VERSION` break; host/guest fingerprint
  skew fails loudly. A shipping game is bit-compatible with the same build run in
  the editor (stripping removes host authoring/debug only, never simulation
  semantics).
- **Shipping strips editor + debug only.** Panic `abort`, no backtrace/panics
  payloads; no spec 48 stepping/instrumentation; no editor shell (spec 22
  standalone). Determinism and the Domain-B module are untouched.
- Portability of the Web shell: the `wasm32` host-shell is wasmtime-compatible
  (WASI-aligned), so the identical Domain-B simulation runs under `wasmtime` and
  in a browser.
- Versioning ties to `ARCH_VERSION` and the real `Dockerfile`/`ci.yml`/`build.sh`
  as the build ground truth (spec 20).

## Performance Targets

- **Cook** a typical asset set (tens of textures/meshes/audio): **deterministic**,
  runs offline; target < a few minutes in CI, incremental when only changed
  assets resurface (spec 02 watcher in dev).
- **Ship binary size** bounded: single binary + cooked bundle; editor/debug
  features compile out under Shipping so the shipped binary is smaller than a
  Release-editor binary.
- **Delta/patch**: block-level patch between two published versions transfers
  only changed blocks; a PATCH release (few changed cooked assets) is a small
  download.
- **Startup**: the packaged game resolves assets from the embedded/adjacent
  bundle without scanning an absolute path tree; load time dominated by cooked
  asset decode, not packaging overhead.

## Testing Strategy

- **Deterministic cook:** cook the same source twice on two platforms; assert
  byte-identical cooked outputs and identical `package_hash`.
- **Purity gate:** after cook and again at the release stage, run
  `verify-wasm-purity` on the shipped `logic.wasm` → `[PURE]`; assert its
  `abi_fingerprint` equals the host's (spec 20).
- **Profile stripping:** build Debug/Release/Shipping from one source; assert the
  Shipping binary no longer links the editor/debugger surface (a symbol/no-op
  gate) while still producing bit-identical gameplay output for the same
  `logic.wasm` and inputs.
- **Cross-platform:** build Linux x64/aarch64 (+ Docker/CI, spec 20); where a
  runner exists, build macOS and Windows and Web wasm; assert the cooked
  artifacts are byte-identical across targets (portability + determinism).
- **Packaging:** the single-binary+assets package boots a Shipping game
  standalone (spec 22) and resolves all assets by logical path with no absolute
  path present in the bundle.
- **Update/delta:** produce versions X and Y (a few changed assets), compute the
  patch, apply it to X, and assert the result hashes to Y's `package_hash`.
- **Semver/ABI:** a MAJOR bump with a new `ARCH_VERSION` yields a manifest whose
  host refuses an old `logic.wasm`; a MINOR/PATCH keeps `abi_fingerprint`
  compatible.

## Dependencies

- Real scaffolding: `scripts/build.sh`, `Dockerfile` (`rust:1.85-slim-bookworm`),
  `.github/workflows/ci.yml` (spec 20) — the build ground truth this pipeline
  extends.
- `contracts` (`ARCH_VERSION`, `abi_fingerprint`) for version gates; spec 16
  `postcard` columns for scene/world cooking; spec 02 asset kinds.
- Spec 12 (`#[system]` wasm module), spec 48 (compiled graph modules), spec 49
  (physics — pure Domain-B, cooked unchanged).
- Host tooling crates: `crates/build-tools`/`crates/cook` (Domain A), plus
  platform toolchains/CI runners for macOS/Windows.
- Distribution: itch.io / Steamworks / a self-hosted HTTPS endpoint (channel
  specifics live behind an ADR where OS/tool-specific).

## Next Steps

1. Add `BuildProfile`/`Platform`/`ReleaseManifest` types + a `crates/cook` CLI
   for deterministic asset cooking.
2. Extend `scripts/build.sh`/CI to emit a Shipping build (feature-gate the editor
   and spec 48 stepping; panic-abort) and re-verify `logic.wasm` purity +
   fingerprint at cook and release.
3. Implement the single-binary+assets packager and the block-level delta/patch
   + update manifest.
4. Wire semver tagging to `ARCH_VERSION` (MAJOR ⇔ ABI break) and add the release
   runbook for itch.io / Steam / self-hosted behind ADRs.
5. Add the Docker/CI + cross-platform (Linux x64/aarch64, macOS, Windows, Web
   wasmtime-compatible shell) build and determinism tests.
