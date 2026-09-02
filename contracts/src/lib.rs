//! # OpenEngine — The Immutable ABI (`contracts`)
//!
//! This crate is the **physical wall** between the two software domains of
//! OpenEngine:
//!
//! | Domain | Who                  | Host crate(s)            | Rights                                  |
//! |--------|----------------------|--------------------------|-----------------------------------------|
//! | A      | Renderer / Host      | `crates/core`, `ecs`, `editor` | `std`, `wgpu`, threads, `unsafe` in third-party |
//! | B      | Game logic (Wasm)    | `crates/logic-sandbox`   | `#![no_std]`, pure, deterministic       |
//!
//! **READ THIS BEFORE EDITING ANYTHING.** See [`ARCH_VERSION`]. Any change to
//! a `#[repr(C)]` layout, a field, or an enum variant below is an **ABI
//! break**. It invalidates every currently deployed Wasm logic module. The
//! correct procedure for an ABI change is:
//!
//! 1. Bump [`ARCH_VERSION`].
//! 2. Update `docs/abi/*.md` first (spec drives code).
//! 3. Land the Rust change in the SAME commit as every consuming crate update.
//!
//! Because both `crates/core` and `crates/logic-sandbox` depend on this single
//! crate, the moment one AI agent changes the boundary the other side's build
//! fails loudly. That failure *is* the safety mechanism.
//!
//! ## Memory-safety contract for AI agents
//!
//! * Logic (Domain B) compiles with `#![no_std]` + `#![forbid(unsafe_code)]`.
//! * Therefore nothing below may be written in a way that forces Domain B to
//!   call `unsafe`. All crossing is done through **plain data** and
//!   **`bytemuck`-safe conversions**.
//! * Structs with `#[repr(C)]` + [`bytemuck::Pod`] are **layout mirrors**:
//!   they may be cast over a shared-memory buffer with
//!   `bytemuck::cast_slice`, never deserialized field-by-field in a hot loop.
//! * Nothing below holds a `&mut` into ECS memory, owns a thread, or performs
//!   I/O. State mutation flows **out** of the guest through [`WorldDelta`];
//!   reads flow **in** through [`StateView`].

#![no_std]
#![forbid(unsafe_code)]

extern crate alloc;

use alloc::vec::Vec;
use core::fmt;

/// Current ABI revision.
///
/// Increment on every breaking layout/behaviour change to any type in this
/// crate. The host refuses to load a logic module whose compiled
/// `ABI_FINGERPRINT` does not match the host build. See `docs/abi/CHANGES.md`.
pub const ARCH_VERSION: u32 = 1;

// ────────────────────────────────────────────────────────────────────────────
// § 0. Handles & identifiers
// ────────────────────────────────────────────────────────────────────────────

/// A globally-unique component *type* identifier.
///
/// This is an index into the host component registry — it is **not** an
/// instance id. Two entities never share a value here; two instances of the
/// *same* component type always do.
#[repr(transparent)]
#[derive(
    Clone,
    Copy,
    PartialEq,
    Eq,
    Hash,
    PartialOrd,
    Ord,
    bytemuck::Pod,
    bytemuck::Zeroable,
    serde::Serialize,
    serde::Deserialize,
    Debug,
)]
pub struct ComponentId(pub u32);

/// An identifier for an *archetype*: a fixed, ordered tuple of component
/// types sharing one contiguous SoA table in the host ECS.
#[repr(transparent)]
#[derive(
    Clone,
    Copy,
    PartialEq,
    Eq,
    Hash,
    PartialOrd,
    Ord,
    bytemuck::Pod,
    bytemuck::Zeroable,
    serde::Serialize,
    serde::Deserialize,
    Debug,
)]
pub struct ArchetypeId(pub u32);

/// A game entity — a stable index plus a generation for ABA protection.
///
/// * `index` locates the slot in the archetype table.
/// * `generation` is incremented every time an `index` is recycled, so a stale
///   handle can never alias a brand-new entity.
///
/// `Entity` is `#[repr(C)]` + `Pod`: the host stores entities as a plain `u32`
/// column (index) or as two packed columns and may view them with
/// `bytemuck::cast_slice`.
#[repr(C)]
#[derive(
    Clone,
    Copy,
    PartialEq,
    Eq,
    Hash,
    bytemuck::Pod,
    bytemuck::Zeroable,
    serde::Serialize,
    serde::Deserialize,
    Debug,
)]
pub struct Entity {
    /// Monotonic-per-slot identity token. Must match the ECS slot generation.
    pub generation: u32,
    /// Slot index within the owning archetype's table.
    pub index: u32,
}

impl Entity {
    /// The tombstone sentinel for "no entity". Never a live handle.
    pub const INVALID: Entity = Entity {
        generation: 0,
        index: u32::MAX,
    };
}

/// A `Result`-compatible error reported from pure logic back to the host.
///
/// Recoverable means: the host may roll back the offending delta, log the
/// cause, and keep running. Fatal corruption of the ECS itself is *not*
/// expressible here — if Domain B ever observes impossible state it must fail
/// this error type loudly rather than guess.
#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[repr(C)]
pub struct RecoverableError {
    /// Stable machine-readable code (see [`ErrorCode`] constants).
    pub code: u32,
    /// Optional human/agent-readable detail. Empty for hot-path numeric codes.
    pub message: Option<alloc::string::String>,
}

// Stable numeric codes. Kept as plain `u32` (not an enum) so that a hot-path
// `RecoverableError` can be built and compared without extra match machinery
// and so unknown future codes do not break pattern matches in Domain A.
#[allow(dead_code)]
pub mod code {
    /// The guest returned a delta for an archetype it was not given.
    pub const UNKNOWN_ARCHETYPE: u32 = 0x0001;
    /// A column write referenced an entity index beyond the column's count.
    pub const OUT_OF_RANGE_ENTITY: u32 = 0x0002;
    /// A payload buffer was not a multiple of the target element size.
    pub const MISALIGNED_PAYLOAD: u32 = 0x0003;
    /// A logic function panicked or hit its instruction budget.
    pub const GUEST_PANIC: u32 = 0x0004;
    /// The delta exceeded the host-provided byte / command budget.
    pub const DELTA_BUDGET_EXCEEDED: u32 = 0x0005;
    /// An entity was despawned and spawned in the same tick (ambiguous).
    pub const DUPLICATE_LIFECYCLE: u32 = 0x0006;
}

impl RecoverableError {
    /// Build a numeric, string-free error (cheap, hot-path friendly).
    pub const fn numeric(code: u32) -> Self {
        RecoverableError {
            code,
            message: None,
        }
    }
    /// Build an error carrying a detail string for diagnostics.
    pub fn detailed(code: u32, message: impl Into<alloc::string::String>) -> Self {
        RecoverableError {
            code,
            message: Some(message.into()),
        }
    }
}

impl fmt::Display for RecoverableError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match &self.message {
            Some(m) => write!(f, "OpenEngine ABI error 0x{:04X}: {m}", self.code),
            None => write!(f, "OpenEngine ABI error 0x{:04X}", self.code),
        }
    }
}

// `core::error::Error` is stable (Rust ≥ 1.81). No std needed.
impl core::error::Error for RecoverableError {}

// ────────────────────────────────────────────────────────────────────────────
// § 1. SoA layout descriptors (the "read" side)
// ────────────────────────────────────────────────────────────────────────────

/// A read-only description of ONE column in an archetype's Structure-of-Arrays
/// table. It is pure metadata; the actual bytes live in [`StateView::arena`].
///
/// It is deliberately layout-identical on host and guest so the guest can walk
/// SoA memory *through* [`StateView`] without copying, and so Domain A can cast
/// a packed descriptor slice with `bytemuck::cast_slice`.
///
/// SAFETY contract for Domain B: you are handed a [`StateView`]. You read
/// `column.data()` (a `&[u8]`) and reinterpret it *only* via
/// `bytemuck::cast_slice` / `cast_slice_mut` against a type whose `Pod` layout
/// matches `element_size`. You NEVER hold the bytes past the call that returns
/// [`WorldDelta`], and you NEVER write into them — the guest is pure.
#[repr(C)]
#[derive(
    Clone,
    Copy,
    PartialEq,
    Eq,
    bytemuck::Pod,
    bytemuck::Zeroable,
    serde::Serialize,
    serde::Deserialize,
    Debug,
)]
pub struct ColumnDescriptor {
    /// Which component type this column holds.
    pub component_id: ComponentId,
    /// Byte width of one element. Must equal `size_of::<T>()` of the `Pod` T
    /// that Domain A stores here and that Domain B casts to.
    pub element_size: u32,
    /// Number of live elements in the column (≤ `capacity` on the host).
    pub count: u32,
    /// Byte offset of the first element into [`StateView::arena`].
    pub data_offset: u32,
}

impl ColumnDescriptor {
    /// The `&[u8]` view of this column inside a given arena.
    ///
    /// # Panics
    /// Panics in the guest if the descriptor points outside `arena`. Because
    /// logic is pure and reads-only this is a hard invariant; a broken host
    /// descriptor is a host bug, surfaced loudly rather than masked.
    #[inline]
    pub fn bytes<'a>(&self, arena: &'a [u8]) -> &'a [u8] {
        let start = self.data_offset as usize;
        let len = self.element_size as usize * self.count as usize;
        let end = start + len;
        assert!(
            end <= arena.len(),
            "ABI: column [{:?}] exceeds arena",
            self.component_id
        );
        &arena[start..end]
    }
}

/// A read-only snapshot of the world handed to a pure system.
///
/// `StateView` is the **only** way Domain B observes ECS state. Design notes:
///
/// * It borrows a *host-owned* contiguous byte arena (`[u8]`) that the Wasm
///   runtime maps (zero-copy bridging). The guest reads it; it cannot mutate.
/// * It is `Copy`: cheap to thread through nested pure functions.
/// * It intentionally carries **no** `&mut`, no iterators into host heap, no
///   locks, no allocations beyond what is needed to materialize the arena.
/// * Fields added later must keep this struct copy-able; do not introduce a
///   lifetime-free owned buffer here or you break the host↔guest sharing model.
#[derive(Clone, Copy, Debug)]
pub struct StateView<'arena> {
    /// Column descriptors, one per archetype column present in this call.
    pub columns: &'arena [ColumnDescriptor],
    /// The packed, byte-addressed arena backing every column above.
    pub arena: &'arena [u8],
    /// Per-call read budget that must never be exceeded (determinism guard).
    pub byte_budget: usize,
}

impl<'arena> StateView<'arena> {
    /// Returns the descriptor for a component id in a given archetype, if the
    /// archetype has it in view.
    pub fn column(&self, component: ComponentId) -> Option<ColumnDescriptor> {
        self.columns
            .iter()
            .copied()
            .find(|c| c.component_id == component)
    }
}

// ────────────────────────────────────────────────────────────────────────────
// § 2. The write side — what a pure system RETURNS (the "delta")
// ────────────────────────────────────────────────────────────────────────────

/// The **sole** mutation channel. A pure system returns one of these; the host
/// applies it atomically inside the ECS worker. It is a buffered, heap-backed
/// record (Domain B allocates freely within its own linear memory) that gets
/// serialized with `postcard` across the Wasm boundary.
///
/// "State is immutable" from Domain B's perspective: B never mutates what it
/// reads; it returns a *description of the next state* and the host materializes
/// it. This is what lets two logic modules run against the same `StateView` and
/// produce mergeable, deterministic deltas.
#[derive(Clone, Debug, Default, serde::Serialize, serde::Deserialize)]
pub struct WorldDelta {
    /// Entities to bring into existence this tick.
    pub spawns: Vec<SpawnCommand>,
    /// Entities to destroy this tick (by generation-guarded handle).
    pub despawns: Vec<Entity>,
    /// Column patches to apply (SoA writes). See [`ColumnWrite`].
    pub writes: Vec<ColumnWrite>,
    /// Out-of-band requests (UI, render hints, events). Applied after spawns.
    pub deferred: Vec<DeferredCommand>,
    /// Non-fatal diagnostics for the host to log / the Critic to review.
    pub warnings: Vec<RecoverableError>,
}

/// Spawn a new entity into an archetype. The host assigns the concrete `Entity`
/// handle; callers should not fabricate one (generation collisions).
#[derive(Clone, Debug, serde::Serialize, serde::Deserialize)]
pub struct SpawnCommand {
    /// Destination archetype.
    pub archetype: ArchetypeId,
    /// Optional preferred entity. Use `None` to let the host allocate.
    pub preferred: Option<Entity>,
    /// Initial values packed in archetype column order. See [`ColumnWrite`]
    /// for the packing contract.
    pub initial: Vec<ColumnWrite>,
}

/// One batched SoA write to a single column of a single archetype.
///
/// Packing contract (this is the zero-copy hand-off):
/// * `payload` must be exactly `count * element_size` bytes.
/// * `indices` gives, for each payload slice of `element_size` bytes, the
///   element slot to write, in matching order.
/// * The host applies it as a contiguous `bytemuck::cast_slice` copy — no
///   per-element boundary crossing — so `payload` should already be laid out
///   contiguously.
#[derive(Clone, Debug, serde::Serialize, serde::Deserialize)]
pub struct ColumnWrite {
    /// Archetype the column belongs to.
    pub archetype: ArchetypeId,
    /// Component type id of the target column.
    pub component: ComponentId,
    /// Slot indices within the column, one per payload element.
    pub indices: Vec<u32>,
    /// Tightly packed `element_size`-aligned values, length == indices.len().
    pub payload: Vec<u8>,
}

/// A command whose application has side effects the pure world does not want
/// to reason about (rendering, UI, engine events).
#[derive(Clone, Debug, serde::Serialize, serde::Deserialize)]
pub enum DeferredCommand {
    /// Ask the renderer to draw an entity this frame.
    Render { entity: Entity, kind: RenderKind },
    /// Arbitrary engine event (input consumed, timer fired, ...).
    Emit {
        topic: u32,
        data: alloc::vec::Vec<u8>,
    },
}

/// What kind of renderable a render command refers to. Pure logic may only
/// *request* rendering; geometry lives on the host.
#[repr(u32)]
#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum RenderKind {
    /// A host-registered static mesh by handle.
    Mesh = 0,
    /// A text/immediate draw.
    Text = 1,
    /// Debug gizmo.
    Gizmo = 2,
}

// ────────────────────────────────────────────────────────────────────────────
// § 3. The pure-system trait the sandbox must satisfy
// ────────────────────────────────────────────────────────────────────────────

/// A pure system: reads a [`StateView`], returns a [`WorldDelta`].
///
/// Domain B must implement exactly this shape for every exported `#[system]`
/// so Domain A can hot-reload and drive it generically. It is deliberately not
/// an `unsafe extern` callback — Domain B stays 100% safe Rust.
pub type PureSystem = fn(&StateView<'_>) -> Result<WorldDelta, RecoverableError>;

/// Byte-identical marker for the *host* ABI build that produced a given Wasm
/// module. Computed from [`ARCH_VERSION`] plus a build-time constant; the host
/// compares this against its own and refuses mismatched logic modules.
pub const fn abi_fingerprint() -> u64 {
    (ARCH_VERSION as u64) << 32 | 0x0E45_0001
}

// ────────────────────────────────────────────────────────────────────────────
// § 4. Wire codec helpers (no_std-safe)
// ────────────────────────────────────────────────────────────────────────────

/// Serialize a delta into a `Vec<u8>` for the postcard wire codec.
///
/// Deterministic across platforms because `postcard` is little-endian and does
/// not float-round on the way out. Prefer this over `serde_json` anywhere it
/// must be reproducible.
pub fn encode_delta(delta: &WorldDelta) -> Result<Vec<u8>, postcard::Error> {
    postcard::to_allocvec(delta)
}

/// Deserialize a delta produced by [`encode_delta`]. Returns `Err` on any
/// structural mismatch — a cheap host-side sanity gate before applying.
pub fn decode_delta(bytes: &[u8]) -> Result<WorldDelta, postcard::Error> {
    postcard::from_bytes(bytes)
}

#[cfg(test)]
mod tests {
    use super::*;
    // This crate is `#![no_std]`, so the `vec!` macro is not in the prelude.
    use alloc::vec;

    #[test]
    fn arch_is_one() {
        assert_eq!(ARCH_VERSION, 1);
    }

    #[test]
    fn entity_is_pod_and_expected_size() {
        assert_eq!(core::mem::size_of::<Entity>(), 8);
        // A Pod marker ensures we *may* cast buffers of it.
        fn assert_pod<T: bytemuck::Pod>() {}
        assert_pod::<Entity>();
        assert_pod::<ColumnDescriptor>();
    }

    #[test]
    fn roundtrip_delta() {
        let d = WorldDelta {
            spawns: vec![SpawnCommand {
                archetype: ArchetypeId(0),
                preferred: None,
                initial: vec![],
            }],
            ..Default::default()
        };
        let bytes = encode_delta(&d).unwrap();
        let back = decode_delta(&bytes).unwrap();
        assert_eq!(back.spawns.len(), 1);
    }
}
