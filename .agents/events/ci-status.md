# CI Status — Diagnostic Report

---
date: "2026-09-03"
status: "PASS (after fix)"
auditor: "ci-agent"
---

## Status: ✅ PASS (after fix)

Recent `main` CI runs showed **3 failures**; the latest was the **`fmt`** job
(`cargo fmt --check`). Build/test/clippy/purity jobs were green. A locally
reproduced `cargo fmt -- --check` confirmed unformatted `.rs` from recent
PoC/ABI edits.

## Root cause
New code landed without `rustfmt`, so the CI `fmt` check failed.

## Fix applied (commit `e6582ff`)
- `cargo fmt --all`
- `cargo fmt --all -- --check` → clean
- Re-verified `cargo build --workspace` OK.

## Full local verification (this run)
| Check | Result |
|---|---|
| `bash scripts/build.sh` (wasm guest) | ✅ |
| `cargo build --workspace --all-targets` | ✅ |
| `cargo test --workspace --all-targets` | ✅ (all pass; incl. PoC ecs/movement/wasm bridge) |
| `cargo clippy --workspace --all-targets -- -D warnings` | ✅ clean |
| `verify-wasm-purity crates/core/assets/logic.wasm` | ✅ `[PURE]` |
| `cargo doc --workspace --no-deps` | ✅ (1 non-blocking rustdoc intra-doc-link warning in core — fix later) |

## Action items
1. ✅ rustfmt applied (CI fmt job fixed).
2. Minor: fix the one `rustdoc::broken_intra_doc_links` warning in `openengine-core`.
3. Keep formatting gate green: run `cargo fmt --all` before each push.
