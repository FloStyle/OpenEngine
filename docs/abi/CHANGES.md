# ABI change log

Rule: every entry records *who*, *what*, *why*, and a bumped `ARCH_VERSION`.
`ARCH_VERSION` never goes backwards.

## v2 — minimal vertical slice ("the living window")

- Added `StateView::tick: u64` (host frame counter; the guest input before ECS
  bridging) plus `StateView::tick_only`.
- Added `DeferredCommand::ClearColor { rgba: [f32;4] }` and
  `WorldDelta::clear_color()` so a pure guest system can tell the host renderer
  what color to clear the window with.
- `f32` appears only at this display-emission boundary; math stays fixed-point.

## v1 — initial scaffold

- Defined `ARCH_VERSION = 1`.
- First frozen set: `Entity`, `ComponentId`, `ArchetypeId`, `ColumnDescriptor`,
  `StateView`, `WorldDelta`, `ColumnWrite`, `DeferredCommand`,
  `RecoverableError`, `PureSystem`.
- Crossings are `postcard`-encoded; `#[repr(C)] Pod` layouts are reserved for
  the zero-copy SoA descriptors.
- No host functions implemented yet — see `host_functions.rs` for the policy.
