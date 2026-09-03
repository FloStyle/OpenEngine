---
spec: "52-ai-developer-surface"
phase: "Vision — AI-native"
status: "draft"
author: "OpenEngine AI"
created: "2026-09-04"
depends_on: ["51-operator-protocol", "00-ecs-architecture", "16-serialization", "10-hot-reload"]
---
# 52 - AI Developer Surface (resident assistant contract)

## Overview

Per `ADR-0002`, OpenEngine ships as an **all-in-one** product: an AI assistant is
a resident operator, not a side chatbot and not an embedded agent runtime. This
spec is the *contract* that assistant uses so it can develop OpenEngine —
write Rust, add features, debug — **without** requiring knowledge of engine
internals and **without** reading raw logs or a fragile MCP setup.

The guiding rule: **the AI works in the language of the engine** — typed
observe, typed propose, typed verify feedback.

## Non-goals (what this deliberately is NOT)

- **Not an MCP server** — a plain, documented contract over the operator loop
  (spec `51`) is enough; MCP could wrap it later but adds nothing now.
- **Not an embedded agent runtime** — the assistant is a Domain-A, optional,
  swappable capability, never in the `no_std` core.
- **Not engine-internal probing** — the AI never needs Vulkan/wgpu internals or
  log scraping to be useful.

## The assistant's three jobs (one loop)

| Job | What the AI does | Engine gives it |
|---|---|---|
| **Assist (copilot)** | read the live scene + the code; suggest entities/components/features | typed observe (state + schema) |
| **Code / test / features** | propose Rust changes or a new logic module | typed verification (build/clippy/tests/purity/determinism) |
| **Debug / self-heal** | find why behavior drifted; propose a fix | delta + hash replay + localization |

All three use the same observe → propose → verify → apply loop (spec `51`).

## The surface the AI depends on

### 1. Observe (typed, semantic)
- Entities / components by the frozen `ComponentId` registry (`Transform`, `Actor`,
  `Velocity3D`, …) with their schema (`16-serialization`), **not** raw ECS internals.
- Live `World` hash + tick, and per-component invariants where defined.
- The source tree in *game terms*: which crates/`#[system]`s exist, what each
  pure system consumes/produces.

### 2. Propose (typed)
- `WorldDelta` / `Command` for runtime features (already the single mutation
  channel).
- `CodeDelta` for source: patch or new logic module; always goes through the
  gate before apply.

### 3. Verify (typed feedback — no log reading)
The engine returns **structured** results the AI can act on directly:
- determinism: `PASS`/`FAIL` + which tick/hash drifted;
- purity: `[PURE]`/not;
- build/clippy/test errors with file/line;
- invariants broken, if any.

### 4. Apply / rollback
- Reversible apply (`/transaction`, snapshot) with automatic rollback on failure;
- human approval for breaking (ABI/`ARCH_VERSION`) changes.

## Model adapter (uniform, no fragile setup)

The same assistant works against **any** endpoint:

```text
[assistant UI]  ⇄  ModelAdapter  ⇄  deepseek / anthropic / ... / llama.cpp-local / unsloth
```

- Config is a small, explicit file/env (API key or `http://127.0.0.1:8080` local
  endpoint), **not** a bespoke per-model setup.
- The adapter exposes one interface (`observe → propose → verify-feedback`) to the
  assistant; the engine's verify stage is the single source of truth regardless
  of which model is plugged in.
- The model is *proposing*; the engine *verifies* (hierarchy of trust, spec `51`).

## Determinism = the debugging file

To debug, the assistant does not read logs: it **reproduces** —
`observe` state → replay an input sequence → compare `World::hash()`. The engine
localizes the first tick where the hash diverges. Compilation + purity catch the
rest at the source. This keeps model context small and grounding high.

## Acceptance (eventual)

- [ ] An assistant on the surface can read a scene in game terms and propose a
      valid runtime feature (delta) that passes the gate.
- [ ] It can propose a Rust change / new logic module and get typed build + purity
      + determinism feedback, then a verified apply or rollback.
- [ ] Swapping the model endpoint (cloud key ⇄ local `llama.cpp`) requires **no
      assistant code change**.
- [ ] No engine-internal knowledge (Vulkan) or log scraping is needed for these.

## Documentation upkeep

Because the assistant and future developers depend on this surface, the following
must stay current whenever engine behavior changes:

- `AGENTS.md` (constitution), `STATE.md`, `docs/specs/architecture.md`,
  `docs/specs/*` for every subsystem, `docs/abi/CHANGES.md` on ABI changes,
  ADRs in `.agents/decisions/`, and `docs/architecture-map.md` (code → spec → ADR).
- A change is not "done" until its doc pointer is updated (see `AGENTS.md` § 7).
