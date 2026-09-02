//! # Domain B — the AI Logic Sandbox
//!
//! **This crate is the entire gameplay surface of OpenEngine.** Everything the
//! player touches — physics quantized to ticks, economy, rules — is written
//! here as **pure functions** over [`StateView`] that return [`WorldDelta`].
//!
//! ## Architectural constraints (non-negotiable, mirrored from `AGENTS.md`)
//!
//! 1. `#![no_std]` — no OS, no heap-syscalls, no hidden entropy.
//! 2. `#![forbid(unsafe_code)]` — you literally cannot write `unsafe` here.
//! 3. No threads, no wgpu, no timing — the host owns all of that.
//! 4. All fractional math MUST use `openengine-math` (`fixed`-backed) — `f32`
//!    is forbidden inside game logic because it is not bit-deterministic
//!    across hosts. See the Determinism Law in `AGENTS.md`.
//! 5. You may only *observe* state through [`StateView`] and only *propose*
//!    change through the returned [`WorldDelta`]. State is immutable; side
//!    effects are forbidden.
//!
//! ## What "a system" looks like
//!
//! The idiomatic entry point has the signature below. A later proc-macro crate
//! (`openengine-system-macros`) will expand a real `#[system]` attribute into
//! an exported `extern "C"` trampoline; the scaffold keeps the plain function
//! form so the ABI compiles and unit-tests today without the macro.
//!
//! ```text
//! // #[system]            // <-- future proc-macro, same signature
//! pub fn gravity_tick(view: &StateView<'_>) -> Result<WorldDelta, RecoverableError>;
//! ```

// The shipped Domain-B artifact is ALWAYS built for `wasm32-unknown-unknown`,
// so there this crate is `#![no_std]`. When the host compiles it as an `rlib`
// (for unit tests / rust-analyzer on the native target) we let `std` in so the
// toolchain can link — that host build is never the artifact that ships.
#![cfg_attr(target_arch = "wasm32", no_std)]
#![forbid(unsafe_code)]

extern crate alloc;

// Guest memory allocator. The `wasm-alloc` feature is enabled ONLY when
// cross-compiling the real logic module (see `docs/specs/architecture.md`).
// `dlmalloc::GlobalDlmalloc` is the canonical no_std allocator for wasm; the
// `unsafe` it requires lives inside the dependency, not in this crate.
#[cfg(all(target_arch = "wasm32", feature = "wasm-alloc"))]
#[global_allocator]
static DLMALLOC: dlmalloc::GlobalDlmalloc = dlmalloc::GlobalDlmalloc;

use contracts::{ArchetypeId, ColumnWrite, ComponentId, RecoverableError, StateView, WorldDelta};
use openengine_contracts as contracts;

// ── Deterministic fixed-point aliases (the ONLY numeric language of logic) ──
// Re-exported so systems read `use openengine_logic_sandbox::prelude::*;`.
pub mod prelude {
    //! Deterministic math + ABI types for ergonomic system authoring.
    pub use openengine_contracts::{
        code, ArchetypeId, ColumnDescriptor, ComponentId, DeferredCommand, Entity,
        RecoverableError, RenderKind, SpawnCommand, StateView, WorldDelta,
    };
    pub use openengine_math::{fx, I16F16, I32F32};
}

// NOTE on wasm exports: exporting a raw `#[no_mangle] extern` symbol is an
// "unsafe attribute" and is therefore incompatible with `forbid(unsafe_code)`.
// When the wasmtime bridge milestone lands, the tiny host-call trampoline
// (which contains NO logic, only a typed call through the ABI) will live in a
// separate shim crate free of that forbid, or use `#[unsafe(no_mangle)]` gated
// to the guest target only. The pure logic here never needs a raw export.

/// Zero-cost panic surface. Logic never unwinds across the host boundary; a
/// trapped panic becomes a `GUEST_PANIC` [`RecoverableError`] on the host.
#[cfg(all(target_arch = "wasm32", not(test)))]
#[panic_handler]
fn guest_panic(_info: &core::panic::PanicInfo) -> ! {
    loop {}
}

// ────────────────────────────────────────────────────────────────────────────
// A dummy system proving the ABI compiles end to end.
//
// It performs a READ-ONLY, deterministic walk over the state view and returns
// an empty delta. It exists so that `cargo check` on `logic-sandbox` proves:
//   contracts (no_std) ──> logic-sandbox (no_std)   compiles.
// ────────────────────────────────────────────────────────────────────────────

/// Dummy column id used only by the scaffold example.
///
/// TODO(demo): replace this whole module with your first real system.
const COLUMN_POSITION: ComponentId = ComponentId(0);

/// Minimal "gravity" demo system.
///
/// Reads a read-only count of entities in a position column and returns an
/// empty delta. Demonstrates the exact pure signature:
///
/// `fn(&StateView) -> Result<WorldDelta, RecoverableError>`
///
/// Replace the body with real logic. Keep the signature and the `-> Result<..>`
/// — both are what the Wasm trampoline and the host scheduler depend on.
pub fn gravity_demo(view: &StateView<'_>) -> Result<WorldDelta, RecoverableError> {
    // Purely observational: we read, never write, never allocate needlessly.
    let observed_count = view.column(COLUMN_POSITION).map(|c| c.count).unwrap_or(0);

    // Deterministic fixed-point arithmetic — never f32. `fx!` builds a
    // fixed<16,16> from an f32 literal at build time so the source is legible
    // while the runtime value is exact.
    let gravity = openengine_math::fx!(0.0);
    let _scaled = gravity
        .checked_mul(openengine_math::fx!(1.0))
        .unwrap_or(gravity);
    let _ = observed_count; // read for determinism tracing in the future

    // Deterministic decision: nothing to change this tick → empty delta.
    Ok(WorldDelta::default())
}

/// A real example: build a batched SoA write the host can apply zero-copy.
/// Each element of `payload` is exactly `4` bytes (a `u32` velocity column).
#[allow(dead_code)]
fn example_column_write() -> ColumnWrite {
    let velocities: [u32; 3] = [10, 20, 30];
    ColumnWrite {
        archetype: ArchetypeId(1),
        component: COLUMN_POSITION,
        indices: alloc::vec![0, 1, 2],
        // Zero-copy: raw bytes already contiguous, host casts with bytemuck.
        payload: bytemuck_slice_to_vec(&velocities),
    }
}

/// NOTE: kept dependency-light so Domain B stays no_std — mirrors what the host
/// does with `bytemuck::cast_slice`, but producing an owned guest buffer.
#[allow(dead_code)]
fn bytemuck_slice_to_vec<T: bytemuck::Pod>(src: &[T]) -> alloc::vec::Vec<u8> {
    alloc::vec::Vec::from(bytemuck::cast_slice(src))
}
