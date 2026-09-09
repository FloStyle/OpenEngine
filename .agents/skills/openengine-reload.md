# Skill: OpenEngine Reload — hot-reload the game logic, no server restart

`/reload_logic` closes the code ↔ state loop: you edit Domain-B logic (Rust),
rebuild the wasm, and re-instantiate the guest **inside the running process** —
then keep ticking/verifying. No need to restart the harness or lose live state.

## When to use

After editing anything under `crates/logic-sandbox` or `crates/logic-export`
(the Domain-B guest), so the running server picks up your change.

## Call it

```bash
# rebuild the wasm + reload the guest in place
bash scripts/harness.sh reload '{"path":"crates/core/assets/logic.wasm"}'
# or raw:
curl -s -X POST http://127.0.0.1:8080/reload_logic \
  -d '{"path":"crates/core/assets/logic.wasm"}'
```

Returns: `{"ok":true,"engine":"wasm"}`.

## Full loop (edit → reload → verify)

```bash
# 1. edit logic (e.g. crates/logic-sandbox/src/lib.rs)

# 2. reload the guest (rebuild + re-instantiate) — no server restart
bash scripts/harness.sh reload '{"path":"crates/core/assets/logic.wasm"}'

# 3. confirm the running sim picked up the change
bash scripts/harness.sh tick '{"n":100}'

# 4. prove determinism of the new logic
bash scripts/harness.sh prove '{"n":200}'          # equal:true

# 5. structured safety gate
bash scripts/harness.sh verify                      # status: PASS
```

## Notes

- `/load_wasm` loads a prebuilt module; `/reload_logic` additionally runs the
  rebuild script first. Use `/reload_logic` after source edits.
- Requires a toolchain that can build the wasm target
  (`wasm32-unknown-unknown`); run `bash scripts/build.sh` once if it fails.
