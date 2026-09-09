# Self-healing OpenEngine (vision, per ADR-0002 / spec 52)

The finished product should be self-evolving and self-healable: when an anomaly
occurs the engine detects it, proposes a fix, verifies it, applies it (or rolls
back). This is NOT a side agent — it is the engine's own loop, with the human as
the final guardian for breaking changes.

## The self-heal loop

```
ANOMALY (hash drift, test fail, purity violation, invariant break)
   → DETECT   engine signal (World::hash, /verify, /prove)
   → PROPOSE  an AI/agent proposes a delta or a new logic module   (untrusted)
   → VERIFY   the ENGINE checks determinism/purity/tests           (trusted)
   → APPLY    if PASS, apply via /snapshot + delta + /reload_logic (reversible)
   → ROLLBACK on FAIL via /restore or /transaction
```

## How the engine detects an anomaly

- **Determinism drift**: re-run a sequence and compare `World::hash()` —
  `POST /prove {n}` returns `equal`. A mismatch is an anomaly.
- **Purity violation**: `python3 brain/orchestrator.py verify-wasm-purity` must
  stay `[PURE]`; `GET /verify` surfaces it.
- **Test/build failure**: `GET /verify` runs `cargo build --workspace` +
  `cargo test --workspace` and returns a structured `PASS`/`FAIL`.

## How it proposes and applies a fix

1. Fork state: `GET /snapshot`.
2. Apply a candidate delta (e.g. via `/transaction`, `/spawn`/`/set`/`/tick`) or
   a new logic module (rebuild + `POST /reload_logic`).
3. Ask the engine to verify: `GET /verify` + `POST /prove`.
4. PASS → keep (optionally `POST /save` to persist); FAIL → `POST /restore`.

## Autonomy policy (who may do what alone)

| Change | Who approves |
|---|---|
| Additive component / new pure system / bug fix (non-breaking) | engine verification PASS → apply; human reviews later |
| Runtime state (spawn/set/tick) in the sandbox | engine (reversible) |
| **ABI / contract layout / `ARCH_VERSION` / unsafe carve-out** | **human only** |
| Deploy / package (`scripts/package.sh`) | human |

Guardrail: the machine (determinism/purity/tests) always has the last word on
integrity; an AI/agent only ever *proposes*.

## Concretely today

- `/verify`, `/prove`, `/transaction`, `/snapshot`+`/restore`, `/reload_logic`
  give the engine its verification + reversible-apply seams (headless core).
- An agent uses the recipe in `.agents/skills/openengine-self-dev.md`.
- A full resident AI operator (auto-proposing on anomalies) is the future
  Domain-A layer per ADR-0002; the seams above are what it will call.
