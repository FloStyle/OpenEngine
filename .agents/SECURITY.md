# SECURITY.md — Security & Portability Rules

---
name: "Security & Portability Rules"
version: "1.0.0"
updated: "2026-09-03"
---

## Critical Security Rules

### 1. No Secrets in Code
- Never commit API keys, passwords, or tokens.
- Never hardcode credentials — use environment variables or a gitignored `.env`.

### 2. Sandbox Purity (Domain B)
- Domain B MUST pass purity verification.
- Forbidden imports: `std`, `wgpu`, `wasmtime`, `winit`, `rayon`, `tokio`, WASI.
- Allowed: `contracts`, `openengine-math`, `bytemuck`, `serde`, `postcard`, `alloc`.
- Verify: `python3 brain/orchestrator.py verify-wasm-purity <wasm_path>`.

### 3. No Hardware/OS-Specific Code
- No distribution-specific paths, no hardcoded users/home dirs.
- All code must compile on `x86_64-linux` and `aarch64-linux`.
- Abstraction layers over `#[cfg(target_os)]`; document exceptions in an ADR.

### 4. Dependency Security
- crates.io only (no git URLs except the workspace itself).
- Version-pinned (no `*` in production); run `cargo audit` before
  security-sensitive tasks.

### 5. Network Security
- Domain B: no network access.
- Domain A: HTTPS only, explicit timeouts, mockable, logged.

### 6. Filesystem Security
- Never write to system directories — workspace or temp only.
- Use the `tempfile` crate for temporary files; validate paths.

### 7. Resource Limits
- Wasm memory: max 256 MB per module.
- Wasm execution: max 16 ms per tick.
- Worker threads: max 8 (Domain A only).

## Portability Rules

- Cross-compiles on `x86_64-linux` / `aarch64-linux`.
- Works in Docker (`Dockerfile` at repo root) and GitHub Actions.
- No GPU required for logic tests.
- Offline after an initial `cargo fetch`.

## Verification Checklist

Before marking ANY task complete:
- [ ] Compiles on x86_64-linux
- [ ] Unit + integration tests pass
- [ ] Domain B passes `verify-wasm-purity` → `[PURE]`
- [ ] No hardcoded paths or credentials; no OS-specific code without an ADR
- [ ] `docker build -t openengine-test .` succeeds
- [ ] `cargo clippy --workspace --all-targets -- -D warnings` clean
- [ ] `cargo audit` shows no vulnerabilities (security-sensitive tasks)

## Violation Reporting

1. Log a `security.violation` event in `.agents/events/`.
2. Document clearly, propose a fix, tag `critical`.
3. Do not commit a fix without review.
