---
spec: "51-operator-protocol"
phase: "Vision — AI-native"
status: "draft"
author: "OpenEngine AI"
created: "2026-09-04"
depends_on: ["00-ecs-architecture", "22-edit-vs-play", "23-undo-redo"]
---
# 51 - Operator Protocol (one loop: human · game logic · AI)

## Overview

Every actor that changes OpenEngine — a **human** in the editor, the **game
logic** in wasm, and (per `ADR-0002`) a **resident AI assistant** — is an
*operator* on the **same** loop:

```
observe  →  propose  →  verify  →  apply
```

The engine is *predictable, safe to auto-modify*, and *easy for an AI to develop*
precisely because there is **one** mutation discipline and **one** verification
gate, not per-actor special cases. There is no privileged actor that bypasses
the pipeline.

## The four stages

| Stage | What | Who owns it | Safe because |
|---|---|---|---|
| **observe** | read the current state as typed data (entities, components, `World` hash) | the engine exposes a **read-only** typed view | no mutation possible from observation |
| **propose** | an operator builds an intended change | human / game logic / AI | it is a *candidate*, not yet applied |
| **verify** | the engine checks the candidate: determinism, purity, invariants, build/tests | **the engine** (machine) | the proposer is fallible; the gate is not |
| **apply** | if verified, apply via a reversible channel; else reject/rollback | **the engine** | every apply is reversible (snapshot/delta) |

### Proposal kinds (unified)

All proposals are typed and go through the same verify/apply path:

- `WorldDelta` — runtime state change (spawn / set component / tick). Already
  the single mutation channel (`AGENTS.md`, `00-ecs-architecture`).
- `Command` (spec `23`) — editor/undoable change.
- `CodeDelta` — a *source* change (patch to `crates/*`, a new logic module). It
  is verified by running the real gate (compile / clippy / tests / purity /
  determinism) and only then proposed for apply.

### Hierarchy of trust (do not invert)

```
PROPOSE   → human / wasm logic / AI      (fallible)      UNTRUSTED
VERIFY    → engine gate (determinism, purity, build/test) TRUSTED
APPLY     → engine (reversible: delta/snapshot/rollback)  TRUSTED but reversible
APPROVE   → human (only for breaking: ABI/ARCH changes)    final guardian
```

The model or agent **proposes**; the **machine** verifies and applies. This makes
self-modification safe even with a non-reliable model, because the engine keeps
the last word on integrity.

## Rules

1. **No actor mutates `World` columns directly.** Everything is a proposal
   through the pipeline (`apply_delta`). Editor tools use `Command` (spec `23`).
2. **Verification is authoritative and typed.** Proposers receive structured
   results (determinism PASS/FAIL, purity `[PURE]`, build/test errors with
   location), never "read random logs and guess".
3. **Every apply is reversible** (snapshot + rollback). A failed/false proposal
   never leaves the engine corrupted.
4. **Breaking changes** (ABI layout, `ARCH_VERSION`, unsafe carve-outs) require a
   **human approval** step — the machine will not autonomously ship them.

## Consequences

- `openengine-harness` (`ADR-0001` bridge + its HTTP surface) exposes these
  stages to external agents today.
- The same surface becomes the contract for the **resident AI assistant**
  (spec `52`), so it needs **no** deep engine-internal knowledge.
- Determinism + the typed gate are the *debugging file* for an AI: reproduce =
  observe → replay → compare hash → the engine localizes the drift.

## Acceptance (eventual)

- [ ] All three operator kinds (human, wasm logic, AI agent) act through the one
      observe→propose→verify→apply pipeline.
- [ ] A proposal that breaks determinism or purity is rejected by the engine
      with a typed error.
- [ ] Every apply can be rolled back to the prior snapshot.
- [ ] Breaking proposals require an explicit human approval flag.
