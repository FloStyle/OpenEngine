# Skill: OpenEngine Verify — prove a change is safe before you ship it

`/verify` is the trust gate. Instead of an agent reading random build/test
logs, it returns one structured PASS/FAIL verdict with typed errors, so the
decision "is this safe to keep?" is a JSON check, not log archaeology.

## When to use

Before you claim any code change is done — after editing logic, ecs, or docs —
run `/verify` and require `status == "PASS"`.

## Call it

```bash
bash scripts/harness.sh verify
# or raw:
curl -s http://127.0.0.1:8080/verify
```

## Verdict shape

```json
{
  "status": "PASS" | "FAIL",
  "build":       { "ok": true },
  "tests":       { "ok": true },
  "purity":      { "ok": true, "status": "[PURE]" },
  "determinism": { "ok": true, "hash": "…" },
  "errors": []
}
```

On FAIL, `errors` is a non-empty typed list, one entry per broken gate:
`build` / `tests` / `purity` / `determinism`, each with a short `detail`. Read
those, fix the underlying cause, and re-run — do not paper over a gate.

## What it runs

1. `cargo build --workspace`
2. `cargo test --workspace`
3. `python3 brain/orchestrator.py verify-wasm-purity crates/core/assets/logic.wasm`
4. A determinism replay over two fresh states (16 ticks, hashes must match)

## Rule of thumb

Only declare a change done when `/verify` is PASS **and** the repo is also
clippy/fmt clean (`cargo clippy --workspace --all-targets -- -D warnings` +
`cargo fmt --check --all`). `/verify` covers correctness + purity; clippy/fmt
cover hygiene.
