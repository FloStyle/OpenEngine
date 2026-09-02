# Short-Term Memory

---
name: "Short Term Memory"
updated: "2026-09-03"
ttl: "7 days"
---

Temporary notes, reviewed and either promoted to `LONG_TERM.md` or discarded.

## 2026-09-03 — Agent OS infra
- `.agents/` + root `STATE.md`/`ROADMAP.md` added.
- AGENTS.md merged (technical rules + Agent-OS workflow + portability).
- Removed `.cursorrules` and `.github/copilot-instructions.md` (owner doesn't
  use Cursor/Copilot).

## 2026-09-03 — Safe memory bridge decision
- Phase 3 memory bridging must be **100% safe**: guest allocates a transport
  `Vec`, host writes via `wasmtime::Memory::write`, guest reads `&buffer[..]`.
- `#[no_mangle]` exports stay in `crates/logic-export`, never in
  `logic-sandbox` (forbid).
- See `.agents/knowledge/PATTERNS.md`.
