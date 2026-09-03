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
pub const ARCH_VERSION: u32 = 2;

// ────────────────────────────────────────────────────────────────────────────
// § 0.1 ABI helper types (additive — see docs/abi/CHANGES.md "v2 addendum").
//       Added for full-engine feature parity (specs 14-16, 24, 46-47).
//       All additions below are additive: no existing layout changed, so
//       ARCH_VERSION is NOT bumped for these.
// ────────────────────────────────────────────────────────────────────────────

/// Per-component schema revision, used by the world/scene codec (spec 16) to
/// migrate component-local layout drift that a global `ARCH_VERSION` cannot
/// describe. Bump per component on any `#[repr(C)]` field change.
pub const COMPONENT_LAYOUT_VERSION: u32 = 1;

/// Kind of a referenced asset (Domain A loads; Domain B only holds the id).
#[repr(u8)]
#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum AssetKind {
    Texture = 0,
    Mesh = 1,
    Audio = 2,
    Shader = 3,
    Font = 4,
    Scene = 5,
}

impl AssetKind {
    /// Stable discriminant (pod-safe storage form).
    #[inline]
    pub const fn to_u8(self) -> u8 {
        self as u8
    }
    /// Recover a kind from its stable discriminant.
    #[inline]
    pub fn from_u8(v: u8) -> Option<Self> {
        Some(match v {
            0 => Self::Texture,
            1 => Self::Mesh,
            2 => Self::Audio,
            3 => Self::Shader,
            4 => Self::Font,
            5 => Self::Scene,
            _ => return None,
        })
    }
}

/// A logical, portable reference to an asset. IDs are resolved by Domain A
/// against `OPENENGINE_ASSETS_PATH`; never an absolute/hardcoded path.
///
/// The `kind` is stored as its stable `u8` discriminant; use [`AssetRef::kind`]
/// for the [`AssetKind`]. (Not `Pod` yet: the current layout has trailing
/// padding. When it must live inside a `Pod` SoA column, add an explicit
/// `_pad: [u8;7]` field so bytemuck accepts it under `forbid(unsafe_code)`.)
#[repr(C)]
#[derive(Clone, Copy, PartialEq, Eq, Debug, serde::Serialize, serde::Deserialize)]
pub struct AssetRef {
    /// Stable asset id assigned by the Domain-A asset registry.
    pub id: u64,
    /// [`AssetKind`] discriminant (`AssetKind::to_u8`).
    pub kind: u8,
}

impl AssetRef {
    /// Construct from a logical asset kind.
    pub const fn new(id: u64, kind: AssetKind) -> Self {
        AssetRef {
            id,
            kind: kind.to_u8(),
        }
    }
    /// The resolved asset kind (returns `AssetKind::Scene` fallback for an
    /// unknown raw discriminant — unknown kinds are host-logged, never lost).
    pub fn kind(&self) -> AssetKind {
        AssetKind::from_u8(self.kind).unwrap_or(AssetKind::Scene)
    }
    /// An "unset" asset reference (no id, `Texture` placeholder kind).
    pub const NONE: AssetRef = AssetRef { id: 0, kind: 0 };
}

/// An opaque handle to a loaded audio voice/stream (spec 14). Domain A plays;
/// Domain B only ever holds this id.
#[repr(transparent)]
#[derive(
    Clone,
    Copy,
    PartialEq,
    Eq,
    Debug,
    bytemuck::Pod,
    bytemuck::Zeroable,
    serde::Serialize,
    serde::Deserialize,
)]
pub struct AudioHandle(pub u64);

/// Minimal deterministic network/rollback state carried across the wire
/// (spec 15). Widths match `StateView.tick` (u64). (Not `Pod`: see the padding
/// note on [`AssetRef`].)
#[repr(C)]
#[derive(Clone, Copy, PartialEq, Eq, Debug, serde::Serialize, serde::Deserialize)]
pub struct NetState {
    /// Simulation tick this state belongs to (matches `StateView::tick` width).
    pub tick: u64,
    /// Authoritative player that produced this input.
    pub player_id: u32,
    /// Hash of the batched inputs (determinism/desync detection).
    pub input_hash: u64,
}

/// A fixed-capacity, pod-safe string for component fields that must stay
/// `#![no_std]`/pod-representable (specs 21, 46, 47). Truncates on overflow;
/// `len` is the number of valid leading bytes in `bytes`.
#[repr(C)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct FixedString<const N: usize> {
    pub bytes: [u8; N],
    pub len: u32,
}

impl<const N: usize> FixedString<N> {
    pub const EMPTY: Self = FixedString {
        bytes: [0; N],
        len: 0,
    };

    /// Build from a `&str`, truncating to `N` bytes if needed.
    pub fn new(s: &str) -> Self {
        let src = s.as_bytes();
        let n = core::cmp::min(src.len(), N);
        let mut bytes = [0u8; N];
        bytes[..n].copy_from_slice(&src[..n]);
        FixedString {
            bytes,
            len: n as u32,
        }
    }
    /// The contained string (UTF-8 prefix by construction from [`FixedString::new`]).
    pub fn as_str(&self) -> alloc::string::String {
        alloc::string::String::from_utf8_lossy(&self.bytes[..self.len as usize]).into_owned()
    }
}

// serde can't derive array deserialization for arbitrary const N, so encode
// the string form manually (canonical, `#![no_std]`-safe).
impl<const N: usize> serde::Serialize for FixedString<N> {
    fn serialize<S: serde::Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        s.serialize_str(&self.as_str())
    }
}

impl<'de, const N: usize> serde::Deserialize<'de> for FixedString<N> {
    fn deserialize<D: serde::Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        struct V<const K: usize>;
        impl<'de, const K: usize> serde::de::Visitor<'de> for V<K> {
            type Value = FixedString<K>;
            fn expecting(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
                write!(f, "a string")
            }
            fn visit_str<E: serde::de::Error>(self, v: &str) -> Result<Self::Value, E> {
                Ok(FixedString::new(v))
            }
            fn visit_string<E: serde::de::Error>(
                self,
                v: alloc::string::String,
            ) -> Result<Self::Value, E> {
                Ok(FixedString::new(&v))
            }
        }
        d.deserialize_str(V::<N>)
    }
}

/// Editor viewport display mode (canonical across specs 04/24/25).
#[repr(u8)]
#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum ViewMode {
    Wireframe = 0,
    Solid = 1,
    Textured = 2,
    Lit = 3,
}

/// Pure player intent, carried as DATA in the wasm input buffer (Phase D).
///
/// The guest reads this and derives the player's velocity itself, so the host
/// never writes gameplay columns — `fn(&StateView) -> WorldDelta` stays pure.
/// Booleans are `u8` (0/1); `speed` is an integer unit the guest converts to
/// fixed-point. `Pod` + serde so it crosses the bridge.
#[repr(C)]
#[derive(
    Clone,
    Copy,
    Debug,
    PartialEq,
    Eq,
    bytemuck::Pod,
    bytemuck::Zeroable,
    serde::Serialize,
    serde::Deserialize,
)]
pub struct PlayerInput {
    /// 1 = move up.
    pub up: u8,
    /// 1 = move down.
    pub down: u8,
    /// 1 = move left.
    pub left: u8,
    /// 1 = move right.
    pub right: u8,
    /// Player speed in fixed units.
    pub speed: u32,
}

impl Default for PlayerInput {
    fn default() -> Self {
        Self::none()
    }
}

impl PlayerInput {
    /// No input pressed, default speed.
    pub const fn none() -> Self {
        PlayerInput {
            up: 0,
            down: 0,
            left: 0,
            right: 0,
            speed: 5,
        }
    }
}

// ────────────────────────────────────────────────────────────────────────────
// § 0.0 Shared fixed-point components (no_std, cross-domain)
//       Canonical Position/Velocity/Color for the SoA bridge (ADR-0001) and
//       spec-21 registry (Position=0, Velocity=1, Color=72). Both the host ECS
//       and the no_std wasm guest read the SAME byte layout.
// ────────────────────────────────────────────────────────────────────────────

/// 16.16 fixed alias (== `openengine_math::I16F16`); used so `contracts` needs
/// no extra crate dependency for the bridge component types.
pub type Fx16 = fixed::FixedI32<fixed::types::extra::U16>;

/// Registry-stable component ids used by the SoA bridge.
pub mod comp {
    /// Position (2D) — spec-21 id 0.
    pub const POSITION: u32 = 0;
    /// Velocity (2D) — spec-21 id 1.
    pub const VELOCITY: u32 = 1;
    /// Transform (3D) — spec-21 id 2.
    pub const TRANSFORM: u32 = 2;
    /// Velocity3D (gameplay, homogeneous actor layout) — engine id 80.
    pub const VELOCITY3D: u32 = 80;
    /// Actor (player/NPC tag + physics params) — engine id 81.
    pub const ACTOR: u32 = 81;
    /// Color — spec-21 engine id 72.
    pub const COLOR: u32 = 72;
}

/// 3D velocity (gameplay). No serde: raw SoA column, fixed-point.
#[repr(C)]
#[derive(Clone, Copy, Debug, PartialEq, Eq, bytemuck::Pod, bytemuck::Zeroable)]
pub struct Velocity3D {
    /// Linear velocity (x, y, z).
    pub linear: [Fx16; 3],
}

impl Velocity3D {
    /// Zero velocity.
    pub const fn zero() -> Self {
        Velocity3D {
            linear: [Fx16::from_bits(0); 3],
        }
    }
}

/// Homogeneous gameplay actor tag: kind + deterministic seed + jump/ground.
#[repr(C)]
#[derive(Clone, Copy, Debug, PartialEq, Eq, bytemuck::Pod, bytemuck::Zeroable)]
pub struct Actor {
    /// 0 = player, 1 = wander, 2 = circle (deterministic NPC behaviours).
    pub kind: u32,
    /// Deterministic seed for NPC pseudo-random behaviour.
    pub seed: u32,
    /// 1 = on the ground (u32 keeps the struct padding-free/Pod).
    pub grounded: u32,
    /// Cooldown ticks before the next jump is allowed.
    pub jump_cd: u32,
    /// Horizontal move speed (fixed units/tick).
    pub move_speed: Fx16,
    /// Jump impulse (fixed units).
    pub jump_force: Fx16,
}

impl Actor {
    /// A player actor at a given horizontal speed / jump force.
    pub fn player(move_speed: Fx16, jump_force: Fx16) -> Self {
        Actor {
            kind: 0,
            seed: 0,
            grounded: 1,
            jump_cd: 0,
            move_speed,
            jump_force,
        }
    }
    /// An NPC actor with the given deterministic seed and behaviour kind.
    pub fn npc(kind: u32, seed: u32) -> Self {
        Actor {
            kind,
            seed,
            grounded: 1,
            jump_cd: 0,
            move_speed: Fx16::from_num(1),
            jump_force: Fx16::from_num(1),
        }
    }
}

/// Pure 3D gameplay input (WASD + jump + look deltas) carried as data.
#[repr(C)]
#[derive(Clone, Copy, Debug, PartialEq, Eq, bytemuck::Pod, bytemuck::Zeroable)]
pub struct InputState3D {
    /// Camera yaw delta (fixed).
    pub yaw_delta: Fx16,
    /// Camera pitch delta (fixed).
    pub pitch_delta: Fx16,
    /// 1 = forward.
    pub forward: u8,
    /// 1 = backward.
    pub backward: u8,
    /// 1 = left.
    pub left: u8,
    /// 1 = right.
    pub right: u8,
    /// 1 = jump.
    pub jump: u8,
    /// Explicit padding so the struct is padding-free/Pod.
    _pad: [u8; 3],
}

impl InputState3D {
    /// No input.
    pub const fn none() -> Self {
        InputState3D {
            yaw_delta: Fx16::from_bits(0),
            pitch_delta: Fx16::from_bits(0),
            forward: 0,
            backward: 0,
            left: 0,
            right: 0,
            jump: 0,
            _pad: [0; 3],
        }
    }
}

/// 3D transform (spec 21, id 2): fixed-point position, rotation (quaternion
/// x,y,z,w) and scale. Shared by the host editor (Domain A) and, via the ABI,
/// usable by deterministic logic. 40 bytes, `#[repr(C)] Pod`.
#[repr(C)]
#[derive(Clone, Copy, Debug, PartialEq, Eq, bytemuck::Pod, bytemuck::Zeroable)]
pub struct Transform {
    /// World position (x, y, z).
    pub position: [Fx16; 3],
    /// Rotation quaternion (x, y, z, w). Identity = (0,0,0,1).
    pub rotation: [Fx16; 4],
    /// Scale (x, y, z).
    pub scale: [Fx16; 3],
}

impl Transform {
    /// Identity transform at a fixed-point position.
    pub fn at(x: Fx16, y: Fx16, z: Fx16) -> Self {
        Transform {
            position: [x, y, z],
            rotation: [
                Fx16::from_num(0),
                Fx16::from_num(0),
                Fx16::from_num(0),
                Fx16::from_num(1),
            ],
            scale: [Fx16::from_num(1), Fx16::from_num(1), Fx16::from_num(1)],
        }
    }
}

/// 2D position, fixed-point, `#[repr(C)] Pod`.
#[repr(C)]
#[derive(Clone, Copy, Debug, PartialEq, Eq, bytemuck::Pod, bytemuck::Zeroable)]
pub struct Position {
    /// X coordinate.
    pub x: Fx16,
    /// Y coordinate.
    pub y: Fx16,
}

/// 2D velocity, fixed-point, `#[repr(C)] Pod`.
#[repr(C)]
#[derive(Clone, Copy, Debug, PartialEq, Eq, bytemuck::Pod, bytemuck::Zeroable)]
pub struct Velocity {
    /// X velocity.
    pub x: Fx16,
    /// Y velocity.
    pub y: Fx16,
}

/// RGBA color (host rendering; not gameplay math).
#[repr(C)]
#[derive(Clone, Copy, Debug, PartialEq, Eq, bytemuck::Pod, bytemuck::Zeroable)]
pub struct Color {
    /// Red.
    pub r: u8,
    /// Green.
    pub g: u8,
    /// Blue.
    pub b: u8,
    /// Alpha.
    pub a: u8,
}

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

/// An **unrecoverable** engine error (Domain A mostly). Triggers rollback of the
/// offending transaction and, if it repeats, a controlled abort — never a
/// silent continue. Domain B pure systems should report only [`RecoverableError`];
/// a `FatalError` from the guest is treated as a hard trap.
#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[repr(C)]
pub struct FatalError {
    /// Stable machine-readable code (see [`code`] for shared values).
    pub code: u32,
    /// Human/agent-readable detail.
    pub message: Option<alloc::string::String>,
}

impl FatalError {
    pub const fn numeric(code: u32) -> Self {
        FatalError {
            code,
            message: None,
        }
    }
    pub fn detailed(code: u32, message: impl Into<alloc::string::String>) -> Self {
        FatalError {
            code,
            message: Some(message.into()),
        }
    }
}

impl core::fmt::Display for FatalError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match &self.message {
            Some(m) => write!(f, "OpenEngine fatal 0x{:04X}: {m}", self.code),
            None => write!(f, "OpenEngine fatal 0x{:04X}", self.code),
        }
    }
}
impl core::error::Error for FatalError {}

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
    /// Deterministic frame counter for this system invocation. In the full ECS
    /// bridge this is derived from the host tick; for the minimal vertical
    /// slice it is the entire input to the pure system.
    pub tick: u64,
    /// Column descriptors, one per archetype column present in this call.
    pub columns: &'arena [ColumnDescriptor],
    /// The packed, byte-addressed arena backing every column above.
    pub arena: &'arena [u8],
    /// Per-call read budget that must never be exceeded (determinism guard).
    pub byte_budget: usize,
}

impl<'arena> StateView<'arena> {
    /// An empty, tick-only view — used before ECS bridging exists and by the
    /// guest trampoline, which receives the tick as a raw argument.
    pub fn tick_only(tick: u64) -> Self {
        StateView {
            tick,
            columns: &[],
            arena: &[],
            byte_budget: 0,
        }
    }

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
    /// Clear the host window/framebuffer to `rgba` before any further drawing.
    ///
    /// This is the ABI signal used by the minimal vertical slice ("the living
    /// window"). The `f32` values are a **display boundary only** — they are
    /// the exact `fixed -> f32` conversion performed at the instant the command
    /// is emitted. Gameplay math never uses raw `f32` (see the Determinism Law);
    /// it reaches the GPU in normalized color space.
    ClearColor { rgba: [f32; 4] },
}

impl WorldDelta {
    /// The first `ClearColor` command in this delta, if any. Convenience used
    /// by the host renderer in the minimal vertical slice.
    pub fn clear_color(&self) -> Option<[f32; 4]> {
        self.deferred.iter().find_map(|cmd| match cmd {
            DeferredCommand::ClearColor { rgba } => Some(*rgba),
            _ => None,
        })
    }
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
    fn arch_matches() {
        assert_eq!(ARCH_VERSION, 2);
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
