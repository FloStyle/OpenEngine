# `.ai/` — AI governance workspace

Everything here is *process* for the agent swarm, not product code.

| Path | Purpose |
|------|---------|
| `AGENTS.md` (repo root) | The constitution — **outranks** every file in this folder. |
| `lock/` | Advisory claim files (`<file>.lock`) so two agents don't edit the same shared file (`contracts/`, root `Cargo.toml`) in-flight. |
| `sessions/` | Per-agent run telemetry / context handoffs (gitignored — never commit raw dumps; commit only distilled notes). |

## Lock protocol
1. Before a large edit to a *shared* file, write `lock/<relative-path-with-dashes>.lock` containing `agent:<name> owner:<github> reason:...`.
2. Do the work, then delete the lock file.
3. Never force past an existing lock — coordinate via the PR/branch, never silently overwrite.

## Commit discipline
- Raw session dumps are `gitignore`d. Distill decisions into `docs/abi/CHANGES.md`
  and ADRs under `docs/specs/` — that is the durable memory, not chat logs.
