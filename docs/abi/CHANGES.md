# ABI change log

Rule: every entry records *who*, *what*, *why*, and a bumped `ARCH_VERSION`.
`ARCH_VERSION` never goes backwards.

## v2 addendum — additive ABI helper types (non-breaking)

Added for full-engine feature parity (specs 14–16, 24, 46–47). Purely additive —
no existing layout/type changed, so `ARCH_VERSION` stays `2` (the ABI wall for
already-shipped logic modules is intact).

- `COMPONENT_LAYOUT_VERSION: u32` + per-component schema-versioning contract (spec 16).
- `AssetKind` + `AssetRef { id: u64, kind: u8 }` (canonical logical asset ref; specs 21/46/47).
- `AudioHandle(u64)` (spec 14) and `NetState { tick:u64, player_id:u32, input_hash:u64 }` (spec 15).
- `FixedString<const N: usize>` (pod-safe fixed string; specs 21/46/47).
- `ViewMode` (canonical editor viewport mode; specs 04/24/25).
- `FatalError` (unrecoverable host error; complements `RecoverableError`).

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
