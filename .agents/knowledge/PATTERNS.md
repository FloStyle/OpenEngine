# Patterns

---
name: "Reusable Patterns"
updated: "2026-09-03"
---

Reusable, architecture-approved patterns. If you use one, reference it by
heading so reviewers can audit intent.

## Safe memory bridge (guest allocates, host writes)

The only sanctioned way to move SoA data host→guest without `unsafe`:

```text
1. Guest exports  openengine_prepare_input(size) -> ptr   (crates/logic-export)
   - allocates a Vec<u8> and stashes it in a spin::Mutex (transport only)
2. Host calls openengine_prepare_input(total_size)         (crates/core)
   - gets a u32 offset into wasm linear memory
3. Host writes via Memory::write:  header+ColumnDescriptors (postcard)
                                   component bytes (bytemuck)
4. Host calls openengine_tick(...)
5. Guest reads its OWN Vec safely: &buffer[..] -> bytemuck::cast_slice
```

Why safe: the guest owns the `Vec`; the host only writes into it; the guest reads
its own allocation. No raw pointers, no `from_raw_parts`, no `unsafe`.

### Guest-side exports belong in `logic-export`
`#[no_mangle]` is an "unsafe attribute"; it must not appear in `logic-sandbox`
(which is `forbid(unsafe_code)`). Keep all trampoline exports in
`crates/logic-export`.

### Output path (guest → host)
The guest serializes its `WorldDelta` to a guest `Vec`, stashes it, and the host
reads it with `wasmtime::Memory::read` at a pointer the guest returns
(`openengine_get_output`). Safe on both sides.

## Deterministic math
All gameplay math in fixed-point via `openengine-math` (`I16F16`, `fx!`). A
pure `fn(tick) -> WorldDelta` must produce bit-identical output across runs and
platforms (verify by running 3×).
