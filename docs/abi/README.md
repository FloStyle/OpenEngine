# OpenEngine ABI — current revision

| Type | Domain role | Notes |
|------|-------------|-------|
| `ARCH_VERSION` | both | Bump on any breaking layout/behaviour change. |
| `Entity` | both | `repr(C)` `Pod`: generation + slot index. |
| `ComponentId` / `ArchetypeId` | both | transparent `u32` handles. |
| `ColumnDescriptor` | both | SoA metadata (`element_size`, `count`, `data_offset`). |
| `StateView` | Domain B (read) | borrowed arena + descriptors; read-only. |
| `WorldDelta` | Domain B → A | the only mutation channel; postcard-encoded. |
| `ColumnWrite` | Domain B → A | zero-copy SoA patch (contiguous payload). |
| `DeferredCommand` | Domain B → A | Render / Emit side-effect requests. |
| `RecoverableError` | both | recoverable failure, never silent. |
| `PureSystem` | both | the function shape: `fn(&StateView) -> Result<WorldDelta, RecoverableError>`. |

## Host-function boundary

All Wasm host functions (imports the guest may call — logging, RNG seed
injection, memory alloc for big payloads) MUST be declared in
`contracts/host_functions.rs` as their single source of truth. That file is the
allow-list: any import the guest uses that is not listed there is rejected by
`brain/orchestrator.py verify`.

See `docs/abi/CHANGES.md` for the change log.
