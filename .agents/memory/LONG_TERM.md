# Long-Term Memory

---
name: "Long Term Memory"
updated: "2026-09-03"
---

Persistent learnings that survive across sessions.

## 2026-09-03
- **`#[no_mangle]` is an "unsafe attribute"** → incompatible with
  `forbid(unsafe_code)`. Keep all wasm exports in `crates/logic-export`.
- **`#![no_std]` cannot be a cargo lint** → gated by target:
  `#![cfg_attr(target_arch = "wasm32", no_std)]`; guest builds always target
  `wasm32-unknown-unknown` with `--features wasm-alloc`.
- **Domain B purity is enforced and verified**:
  `python3 brain/orchestrator.py verify-wasm-purity <wasm>` → `[PURE]`.
- **Guest memory reads must be safe**: guest allocates, host writes, guest
  reads its own `Vec` (see PATTERNS.md). No raw pointers in Domain B.
- **`ARCH_VERSION` bumps** (v1→v2 for the living window) require a matching
  `docs/abi/CHANGES.md` entry + all consumers in the same commit.
