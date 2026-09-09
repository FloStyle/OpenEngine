# Skill: OpenEngine self-development (agent loop)

When developing OpenEngine, an agent follows one loop over the live engine:

```
observe → propose → verify → apply  (rollback if verify FAILS)
```

The engine exposes observe/mutate/verify/rollback over HTTP so the agent can
test its changes against a running engine, not just static files.

## Start the live core

```bash
bash scripts/build.sh                      # rebuild logic.wasm (if editing logic)
cargo run -p openengine-harness -- --port 8080
```

## The safe loop (observe → propose → verify → apply)

```bash
B=http://127.0.0.1:8080

# 1. observe
curl -s $B/observe

# 2. propose a change safely — fork state so you can roll back
SNAP=$(curl -s $B/snapshot)                                   # remember the pre-change state
curl -s -X POST $B/spawn -d '{"transform":[1,0,0],"color":[255,0,0,255]}'   # propose

# 3. verify the whole repo gates (build + tests + purity + determinism)
curl -s $B/verify     # {"status":"PASS|FAIL","build":{...},"tests":{...},"purity":{...},...}

# 4. apply — if you wanted the change and verify passed, keep it; else roll back:
curl -s -X POST $B/restore -H 'Content-Type: application/json' \
     -d "{\"snapshot\":$SNAP}"

# determinism proof of a replay
curl -s -X POST $B/prove -d '{"n":100}'   # {"equal":true,...}
```

Use `scripts/selfdev.sh` for a canned version of this loop.

## Rules for an agent

- **Propose, don't override.** The ENGINE verifies (`/verify`, `/prove`) and is
  the source of truth; never claim a change is safe without a PASS.
- **Fork before you mutate** (`/snapshot`) so a failed change is reversible
  (`/restore`, `/transaction`).
- **Logic edits are hot**: after editing `logic-sandbox`, call
  `POST /reload_logic {"path":"crates/core/assets/logic.wasm"}` to rebuild +
  reload in the running process, then re-tick and `/prove`.
- **Breaking changes (ABI, contracts) require a human** (hierarchy of trust,
  ADR-0002 / spec 52).
- Feedback is typed: `/verify` returns structured PASS/FAIL, not logs.

## Endpoints (see `/spec`)

`/observe /spawn /despawn /set /tick /hash /load_wasm /save /load /snapshot
/restore /transaction /prove /verify /reload_logic`.
