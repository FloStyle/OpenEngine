---
spec: "18-documentation"
phase: "All (governance, evergreen)"
status: "design"
---

# Documentation

## Overview

Documentation in OpenEngine is not an afterthought — it is an **AI interface**.
The engine is built and maintained by autonomous agents that never share
conversational context (`AGENTS.md §7.3`: *"Rustdoc is an AI interface: write
docs for a future agent reading cold"*). Every doc artifact has a single
job: let a cold-starting agent reconstruct *why* the code is the way it is,
*what* invariant it protects, and *how* to change it without breaking the wall.

There are five layers, each owned by a specific audience:

| Doc | Audience | Location | One-line job |
|-----|----------|----------|--------------|
| Rustdoc | every agent | inline `///` in every crate | explain each type/fn cold, esp. `contracts` = the ABI reference |
| Constitution | every agent | `AGENTS.md` | the master rulebook (outranks tool files) |
| Architecture bible | human + agent | `docs/specs/*.md` | the *why* behind the whole system |
| ABI changelog | human + agent | `docs/abi/` | every ABI mutation + revision history |
| User guide & examples | human player/dev | `docs/guide`, `docs/examples` | how to actually run it |

This spec (number `18`) sits in the middle of the set: the strategy specs
`17/19/20` lean on it, and it leans on the architecture specs `00`/`01` for
content. Keep the **docs-to-code link** explicit everywhere so a spec never
drifts from the ABI it claims to describe.

## Rustdoc: the crates are the truth

Every public item gets a doc comment that says, in this order: **what** it is,
**why** it exists, **what invariant** it protects, and **an example** when the
shape is non-obvious. The canonical example to copy is `contracts/src/lib.rs`,
whose module-level doc explains the domain table, the memory-safety contract,
and the ARCH_VERSION discipline before the reader hits a single type.

### contracts — the ABI reference

`contracts` is the single most important doc surface in the repo because it is
the **physical wall** between Domain A and Domain B. Its rustdoc is the ABI
reference every other doc points at:

- Document every `#[repr(C)]` layout, `Pod`/`Zeroable` bound, and why the type
  is layout-identical on host and guest.
- On every `pod` struct, state *how* it may cross the boundary
  (`bytemuck::cast_slice`, never `transmute`) and what `element_size` must
  equal.
- Enumerate the wire-codec contract: `postcard` is the only wire format,
  little-endian, deterministic (`contracts::encode_delta`/`decode_delta`).
- Keep the SAFETY contract comments readable for a guest author: "you read
  `column.data()`; you never hold bytes past the return; you never write."

**Docs-to-code sync rule:** `docs/specs/*.md` must reference `contracts` items
by their rustdoc link-style names (e.g. "see [`WorldDelta`]") so an agent can
jump from the prose spec to the authoritative definition in one step, and a
renaming of an ABI type breaks the doc link loudly (a signal to update both).

### Per-crate guidance

- **`openengine-math`** (Domain B): each numeric type's scale/precision
  (`I16F16`, `I32F32`), the fixed-point-only rule, and any rounding convention.
- **`logic-sandbox`** (Domain B): the top-of-file constraint block (mirrored
  from `AGENTS.md`), the `PureSystem` shape, and a worked pure function
  (`tick_color`) as a copy-paste starting point.
- **`logic-export`** (Domain B): why `#[no_mangle]` lives here and not in
  logic-sandbox (unsafe attribute vs `forbid`), and the exported C ABI surface.
- **`core` / `ecs` / `editor`** (Domain A): the bridge entry points, unsafe
  carve-outs (each with a written SAFETY justification), and any GPU/window
  surface that tests must not touch.

## AGENTS.md — the agent constitution

`AGENTS.md` is the **master rulebook** and outranks per-tool rule files. It is
the *why*; the workflow files (`.agents/`) are the *how*. This spec does not
rewrite it — it documents how the rest of the docs **point back to it**:

- Specs reference `AGENTS.md` sections (`§1 Domain Boundaries`, `§3
  Determinism Law`, `§6 Testing Protocol`, `§9 Definition of Done`) rather than
  restating them, so the constitution is the single source of policy.
- Any new doc that proposes a rule must say which `AGENTS.md` section it
  implements or extend — a doc that invents policy out of thin air is
  immediately suspect.

## docs/specs/* — the architecture bible

`docs/specs/` is a numbered sequence (00, 01, 17–20 here) where **lower numbers
are foundational and higher numbers operational**. Each spec is a standalone
Markdown file with a YAML header:

```yaml
---
spec: "18-documentation"
phase: "All"
status: "design"
---
```

The header makes the file self-describing and machine-greppable. A spec should
contain: an `## Overview` (the "why, in two sentences"), detailed sections, a
`## Dependencies` section naming the real crates/types it leans on, and a
`## Next steps` list. Code inside a spec is **illustrative** — it sketches the
shape, never substitutes for the real implementation, which lives in the crates
and is the source of truth.

### The specs index and ADRs

- A single index (this set of specs plus `00`/`01`) should be linked from the
  README so a new agent can find the full bible in one hop.
- Architecture Decision Records (ADRs) capture *why a decision was made*, with
  alternatives considered. Substantive cross-domain decisions (the Domain
  split, the `forbid(unsafe_code)` + carve-out policy, `postcard` over a custom
  codec) belong in an ADR so a future agent does not reopen a settled question.

## User guide and examples

The user guide is the *human* face of the engine — how to build, run, and write
a game:

- **Getting started**: `bash scripts/requirements.sh`, `bash scripts/build.sh`,
  `cargo run -p openengine-core`, environment variables
  (`OPENENGINE_*`), Docker build.
- **Architecture tour**: `docs/specs/architecture.md` is the human-readable
  entry; `contracts/` is the deep reference.
- **Examples** are runnable proofs, owned by spec `19-examples` (Pong,
  Platformer, Sprites). Each example's README shows: what it exercises, the
  expected behavior, how to run it, and its purity/determinism expectations.
  Keeping examples in the repo as crates/tests means they cannot silently
  rot — a doc that describes dead code is worse than no doc.

## Glossary

Terms with a single, load-bearing meaning — Domain A/B/C, `StateView`,
`WorldDelta`, archetype, SoA, `PureSystem`, `#[no_mangle]` trampoline, "the
wall" — should live in one glossary (a `docs/glossary.md` or the README) so
specs can use them without redefining them. Consistency of vocabulary across
agents is what stops specs from drifting.

## Doc generation and CI

Documentation must not be only prose in the repo — it should be *built and
validated* in CI (wired in spec `20-ci-cd`):

- **rustdoc build**: `cargo doc --workspace --no-deps` must succeed; broken
  intra-doc links (`[`Type`]`) are compile errors with
  `RUSTDOCFLAGS="-D rustdoc::broken_intra_doc_links"`. This is the mechanism
  that makes "specs link to contracts" enforced, not aspirational.
- **Lint the prose**: `markdownlint` on `docs/**` keeps tables/headers
  consistent (optional, but cheap and enforces the YAML-header shape).
- **Link check**: a `linkchecker` job (or `cargo doc`'s own link pass) to catch
  dead relative links between specs.

```bash
# Local doc checks that mirror CI:
cargo doc --workspace --no-deps --document-private-items
RUSTDOCFLAGS="-D rustdoc::broken_intra_doc_links" cargo doc --workspace --no-deps
```

## Keeping docs in sync with code

The number-one way docs rot is when behavior changes but prose does not. The
anti-rot rules, anchored in `AGENTS.md`:

1. **`contracts` + `docs/abi/` move together.** Every layout change bumps
   `ARCH_VERSION`, updates `docs/abi/CHANGES.md` and `docs/abi/README.md`
   first, and lands in the same commit as all consuming crates.
2. **Behavior-affecting changes are recorded in `docs/`.** A PR that changes
   how a system behaves must touch the owning spec in the same change (see
   `AGENTS.md §9` DoD, spec `20-ci-cd`'s PR policy).
3. **Specs point at contracts, not at copied code.** When a spec reproduces a
   struct in an illustrative snippet it must link back to the real `contracts`
   item, so a reader who needs the authoritative layout jumps to source.
4. **`STATE.md` is current.** The global state file records the active phase
   and open tasks; docs should never describe a completed phase as "in
   progress."

### The doc review checklist (Definition of Done subset)

- [ ] `cargo doc --workspace --no-deps` builds, with no broken intra-doc links.
- [ ] A behavior change updated the owning `docs/specs/*` in the same commit.
- [ ] An ABI change bumped `ARCH_VERSION` + updated `docs/abi/` in the same commit.
- [ ] New terms appear in the glossary / are linked on first use.
- [ ] `STATE.md` reflects reality.

## Doc command cheat-sheet

```bash
# Build rustdoc for all crates (no deps), fail on broken intra-doc links:
RUSTDOCFLAGS="-D rustdoc::broken_intra_doc_links" cargo doc --workspace --no-deps

# Preview the ABI reference crate's docs:
cargo doc -p openengine-contracts --no-deps --open

# Optional prose linting:
markdownlint docs/**.md          # if installed; CI mirrors this

# CI runs these automatically — see spec 20-ci-cd "doc" job.
```

## Dependencies
- `AGENTS.md`, `.agents/` (workflow), `docs/specs/{00,01,17,19,20}`, `docs/abi/`
  (`CHANGES.md`, `README.md`), `contracts/` (rustdoc), `cargo doc` (build),
  `markdownlint`/`linkchecker` (optional CI).

## Next steps
1. Write the one-sentence-per-spec index and link it from the README.
2. Stand up the `doc` CI job (rustdoc + broken-intra-doc-link deny).
3. Create `docs/glossary.md` and back-fill the first terms.
4. Author an ADR for the Domain split and the unsafe carve-out policy.
