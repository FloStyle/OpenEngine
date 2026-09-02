# ABI change log

Rule: every entry records *who*, *what*, *why*, and a bumped `ARCH_VERSION`.
`ARCH_VERSION` never goes backwards.

## v1 — initial scaffold

- Defined `ARCH_VERSION = 1`.
- First frozen set: `Entity`, `ComponentId`, `ArchetypeId`, `ColumnDescriptor`,
  `StateView`, `WorldDelta`, `ColumnWrite`, `DeferredCommand`,
  `RecoverableError`, `PureSystem`.
- Crossings are `postcard`-encoded; `#[repr(C)] Pod` layouts are reserved for
  the zero-copy SoA descriptors.
- No host functions implemented yet — see `host_functions.rs` for the policy.
