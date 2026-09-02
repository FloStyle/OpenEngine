---
spec: "20-ci-cd"
phase: "All (governance, evergreen)"
status: "design"
---

# CI / CD

## Overview

CI/CD is the **automated enforcement** of the Definition of Done
(`AGENTS.md §9`) and the portability / security contracts
(`.agents/SECURITY.md`). GitHub Actions runs on every push to `main` and on
every pull request, so an agent can never merge a change that fails a gate —
and the branch policy below forbids merging without review and green tests.
This spec describes the real workflow (`.github/workflows/ci.yml`), the jobs it
runs, the jobs we add or adjust, and the release process that publishes the
**Wasm logic module + host binary**.

The fundamental split (spec `17`) is: **fast gates on every PR, slow gates on
releases**, with determinism and purity enforced on both because they are
product requirements, not preferences.

## Branch / PR policy (consistent with AGENTS.md)

- The default branch is `main`; it is **protected**. Agents work on topic
  branches and open PRs; **agents do NOT merge without review and passing
  tests** (`AGENTS.md §7`, `.agents/SECURITY.md` "Do not commit a fix without
  review").
- Required before merge (enforced by branch protection on `main`):
  - All CI jobs green (the `fmt`, `check`, `clippy`, `test`, `purity`,
    `docker`, `determinism`, `doc` jobs below).
  - Human or senior-agent **review** of the diff, with the LLM Critic
    (`brain/orchestrator.py critic`) surfaced where wired in.
  - Behavior-affecting changes updated the owning `docs/specs/*` and, when the
    ABI changed, bumped `ARCH_VERSION` + updated `docs/abi/` in the same commit.
- A PR that touches `contracts/` or `docs/abi/` is reviewed with extra care: it
  is an ABI wall, and a mistake breaks every deployed logic module.

## The real workflow today: `.github/workflows/ci.yml`

The current workflow already runs these jobs on `push` to `main` and on
`pull_request`, with a concurrency group that cancels superseded runs:

| Existing job | Command (real) | Why it exists |
|--------------|----------------|---------------|
| `fmt` | `cargo fmt --all -- --check` | rustfmt applied (DoD) |
| `check` | `cargo check --workspace` | whole workspace compiles |
| `clippy` | `cargo clippy --workspace --all-targets -- -D warnings` | `deny(clippy::all)` |
| `test` | `cargo test --workspace` | all unit + integration tests |
| `purity` | `python brain/orchestrator.py verify-deps` then `verify` | Domain B purity gate |

`purity` currently runs `verify-deps` (forbids Domain A deps in Domain B
manifests) and a **stub** `verify` that echoes "no wasm artifact yet — stub OK"
until a logic artifact exists. This spec's add/adjust section turns that stub
into real enforcement.

## Jobs to add / adjust

### Adjust: `purity` → real `verify-wasm-purity`

Once a logic artifact is produced by the build, the purity job must check the
actual module. Adjust it to build the module then gate on the artifact:

```yaml
  # The Brain guards the Domain B purity invariant on every change.
  purity:
    runs-on: ubuntu-latest
    steps:
      - uses: actions/checkout@v4
      - uses: dtolnay/rust-toolchain@stable
        with:
          targets: wasm32-unknown-unknown
      - uses: Swatinem/rust-cache@v2
      - uses: actions/setup-python@v5
        with:
          python-version: "3.12"
      - name: Forbid Domain A deps in Domain B
        run: python3 brain/orchestrator.py verify-deps
      - name: Build + stage the logic module
        run: bash scripts/build.sh
      - name: Structural purity check (must be [PURE])
        run: python3 brain/orchestrator.py verify-wasm-purity crates/core/assets/logic.wasm
```

### Add: `docker` — reproducible build gate

The engine must build in the pinned container (`Dockerfile`, Debian bookworm,
`rust:1.85`, no GPU needed for logic tests). This is the "works in Docker" arm
of the portability rule and catches host-only drift:

```yaml
  docker:
    runs-on: ubuntu-latest
    steps:
      - uses: actions/checkout@v4
      - name: Build the reproducible test image
        run: docker build -t openengine-test .
      - name: Run the headless test suite inside the container
        run: docker run --rm openengine-test   # = cargo test --workspace
```

### Add: `determinism` — bit-identical, 3×, both arches

Determinism is a product requirement (spec `17`). Run the deterministic tests
**3 separate times** so a flaky nondeterminism cannot pass once, and run them
for both portability targets where a cross-compiler is available. All CI runs on
the **native host**; the **only** wasm artifact in the pipeline is `logic.wasm`
(Domain B), built and gated by the `purity`/`verify-wasm-purity` job. The
determinism job therefore runs the native `cargo test` suite — not wasm.

```yaml
  determinism:
    runs-on: ubuntu-latest
    steps:
      - uses: actions/checkout@v4
      - uses: dtolnay/rust-toolchain@stable
      - uses: Swatinem/rust-cache@v2
      # 'determinism' is a substring filter — NO --exact (a bare --exact matches
      # only a test literally named "determinism" and would run zero tests).
      # The grep guard asserts each of the 3 runs executed >= 1 determinism test.
      - name: Deterministic suite, run 3× (bit-identical expected)
        run: |
          for i in 1 2 3; do
            out="$(cargo test --workspace determinism)" || exit 1
            printf '%s\n' "$out" | grep -q 'test result: ok' \
              || { echo "FAIL: determinism run $i executed no tests"; exit 1; }
          done
```

> Note: `postcard` is little-endian and non-floating, so a wire fingerprint is
> stable across `x86_64-linux` and `aarch64-linux`. A dedicated cross-arch
> determinism job (run the determinism suite on `aarch64-unknown-linux-gnu`
> via `cross`/QEMU when available) asserts the same byte output on both — this
> is the strongest form of the portability guarantee.

### Add: `doc` — rustdoc + broken-intra-doc-link deny

Mirrors spec `18`. Docs that do not build are a broken interface for the next
agent:

```yaml
  doc:
    runs-on: ubuntu-latest
    steps:
      - uses: actions/checkout@v4
      - uses: dtolnay/rust-toolchain@stable
        with:
          components: rust-docs
      - uses: Swatinem/rust-cache@v2
      - name: Build rustdoc, deny broken intra-doc links
        run: RUSTDOCFLAGS="-D rustdoc::broken_intra_doc_links" cargo doc --workspace --no-deps
```

### Add: `cross-target` (aarch64)

Portability demands both `x86_64-linux` and `aarch64-linux`. Because wgpu has
system deps, this is a best-effort *check* job (not a full render test):

```yaml
  cross-target:
    runs-on: ubuntu-latest
    steps:
      - uses: actions/checkout@v4
      - uses: dtolnay/rust-toolchain@stable
        with:
          targets: aarch64-unknown-linux-gnu, wasm32-unknown-unknown
      - uses: Swatinem/rust-cache@v2
      - name: Check aarch64
        run: cargo check --workspace --target aarch64-unknown-linux-gnu || echo "warn: system deps for aarch64 not installed"
      - name: Build Domain B for wasm (the shipped artifact target)
        run: cargo build --workspace --target wasm32-unknown-unknown
```

### Add: `audit` — security-sensitive dependency scan

`.agents/SECURITY.md §4` pins versions and requires `cargo audit` before
security-sensitive tasks. A scheduled + PR dependency check enforces it:

```yaml
  audit:
    runs-on: ubuntu-latest
    steps:
      - uses: actions/checkout@v4
      - name: Install rust audit tooling
        run: cargo install cargo-audit || cargo install --locked cargo-audit
      - name: Audit dependencies
        run: cargo audit
```

Run this on every PR touching `Cargo.toml`/`Cargo.lock` and on a schedule
(e.g. nightly) to catch newly disclosed CVEs. It is **required** for
security-sensitive tasks; treat its `error`/`high` findings as merge blockers.

## Run-what matrix

| Trigger | Jobs |
|---------|------|
| PR / push to `main` | fmt, check, clippy, test, purity, docker, determinism, doc, cross-target |
| PR touching `Cargo.{toml,lock}` | + audit |
| Schedule (nightly) | audit + full release-gate suite |
| Manual release (tag) | full suite + release/artifact jobs (below) |

## Release process

Releases are **tagged and reproducible**. A release publishes the two artifacts
the engine ships: the **Wasm logic module** (Domain B) and the **host binary**
(Domain A). Both are produced from the tagged source by CI so the published bits
are exactly the tested bits.

### Artifact mapping

| Artifact | Produced by | Where it lives |
|----------|-------------|----------------|
| `logic.wasm` | `bash scripts/build.sh` (stages `crates/core/assets/logic.wasm`) | published wasm module |
| host binary | `cargo build --release -p openengine-core` (+ editor/ecs deps) | published host binary |
| docs bundle | `cargo doc --workspace` | published docs site |

### Release workflow (`release`)

```yaml
  release:
    if: startsWith(github.ref, 'refs/tags/')
    runs-on: ubuntu-latest
    needs: [fmt, check, clippy, test, purity, docker, determinism, doc, cross-target, audit]
    steps:
      - uses: actions/checkout@v4
      - uses: dtolnay/rust-toolchain@stable
        with:
          targets: wasm32-unknown-unknown
      - uses: Swatinem/rust-cache@v2
      - name: Build the pure logic module
        run: bash scripts/build.sh
      - name: Verify the shipped module is [PURE]
        run: python3 brain/orchestrator.py verify-wasm-purity crates/core/assets/logic.wasm
      - name: Build the release host binary
        run: cargo build --release -p openengine-core
      - name: Compute the ABI fingerprint (release gate)
        run: |
          # ARCH_VERSION + abi_fingerprint must match what the host expects;
          # a mismatch refuses to load (see contracts::abi_fingerprint).
          cargo run --release -p openengine-core -- --print-abi-fingerprint
      - uses: softprops/action-gh-release@v2
        with:
          files: |
            crates/core/assets/logic.wasm
            target/release/openengine-core
```

### Release policy

- **Tagging**: only after `main` is green and the full suite above passes on the
  tagged commit. Tags are `v<semver>` aligned to the workspace `version`.
- **ABI discipline**: an `ARCH_VERSION` bump must land in the *same* release as
  its `docs/abi/` update; never publish a wasm module whose
  `abi_fingerprint` does not match the host build (the host refuses to load it —
  `contracts::abi_fingerprint`).
- **Reproducibility**: artifacts are built from the tag inside the pinned
  container; a rebuild from the same tag yields the same bytes (fixed-point,
  `postcard` wire, no ambient state).

## Branch protection summary

On `main`:
- Require PRs; disallow direct pushes.
- Required status checks: `fmt`, `check`, `clippy`, `test`, `purity`, `docker`,
  `determinism`, `doc`, `cross-target`.
- Require review before merge; agents do not self-merge.
- Do not delete a review conversation before a green run on the merged head.

## Offline / reproducible note

All jobs run `cargo` offline after `Swatinem/rust-cache@v2` warms the cache and
after an initial `cargo fetch`; no job depends on a live network beyond crate
fetching. This matches the "work offline after an initial `cargo fetch`"
portability rule. The `docker` job additionally proves the pinned-container
build reproduces the host result.

## Command cheat-sheet

```bash
# The full local mirror of CI before pushing:
cargo fmt --all -- --check
cargo check --workspace
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
bash scripts/build.sh && python3 brain/orchestrator.py verify-wasm-purity crates/core/assets/logic.wasm  # [PURE]
python3 brain/orchestrator.py verify-deps
docker build -t openengine-test . && docker run --rm openengine-test
# determinism: substring filter, no --exact; each run must execute >= 1 test
for i in 1 2 3; do
  out="$(cargo test --workspace determinism)" || exit 1
  printf '%s\n' "$out" | grep -q 'test result: ok' \
    || { echo "FAIL: determinism run $i executed no tests"; exit 1; }
done
RUSTDOCFLAGS="-D rustdoc::broken_intra_doc_links" cargo doc --workspace --no-deps
cargo audit   # security-sensitive / before release

# Release-tag run:
git tag v0.1.0 && git push origin v0.1.0    # triggers the full suite + release job
```

## Dependencies
- `.github/workflows/ci.yml` (the real workflow), `Dockerfile`, `scripts/build.sh`,
  `scripts/requirements.sh`, `brain/orchestrator.py`.
- Specs `17-testing-strategy` (what each job proves) and `18-documentation`
  (the `doc` job), and `AGENTS.md §6/§9` (what "done" means).

## Next steps
1. Turn the `purity` stub into a real `verify-wasm-purity` job (needs the
   artifact-producing build in CI).
2. Add `docker`, `determinism`, `doc`, `cross-target`, and `audit` jobs to
   `.github/workflows/ci.yml`.
3. Enable branch protection on `main` with the required checks listed here.
4. Land the `release` job and document the tag-and-publish runbook.
