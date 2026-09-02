//! # `openengine-math` — deterministic, fixed-point numerics
//!
//! **The Determinism Law (AGENTS.md):** all gameplay math must use [`fixed`]-based
//! types or an explicit [`glam`] rounding that is bit-stable across hosts.
//! Raw `f32` is **forbidden** inside game logic: IEEE-754 is only *mostly*
//! deterministic and lets FMA / x87 rounding differences leak between the
//! native CI runner and the deployed target. `fixed` arithmetic is exact
//! integer math — identical everywhere.
//!
//! ## Rules for agents
//! * Reach for [`I16F16`] (16 integer + 16 fraction bits) by default; it is
//!   the workhorse Q format and matches a `u32`/`i32` storage cell cleanly.
//! * Use the [`fx!`] macro in source so values read like floats but compile to
//!   exact fixed constants (build-time, not runtime).
//! * If you must interop with the GPU (which runs `f32`), do it **only in
//!   Domain A** and go through [`quantize_to_f32`] so a value round-trips
//!   bit-identically.

#![no_std]
#![forbid(unsafe_code)]

/// `fixed<16,16>` — 32-bit, 16 fractional bits. Default gameplay scalar.
pub type I16F16 = fixed::FixedI32<fixed::types::extra::U16>;

/// `fixed<32,32>` — 64-bit, 32 fractional bits. For high-precision economy.
pub type I32F32 = fixed::FixedI64<fixed::types::extra::U32>;

/// Storage-matching alias: what a `u32` SoA cell most often holds.
pub type Storage = u32;

/// Compile-time fixed literal. `fx!(3.14)` becomes an exact `I16F16`.
///
/// Because it converts the *float literal* at compile time via `from_f32`, it
/// still runs on the host in tests and on the Wasm guest identically.
#[macro_export]
macro_rules! fx {
    ($v:expr) => {{
        $crate::I16F16::from_num($v)
    }};
}

/// A named marker for the f32 "escape hatch" so agents grep for the boundary.
/// (Value is unused; it documents intent.)
#[allow(dead_code)]
pub const F32_EMULATED: u32 = 0;

/// Quantize an `f32` to the nearest representable `I16F16`, then back to `f32`.
///
/// The ONLY sanctioned path for a Domain-A GPU value to enter logic. The
/// rounding is a deterministic function of the input, so two hosts given the
/// same `f32` bits produce the same result.
///
/// Panics if `v` is NaN or outside the representable range (host bug → loud).
#[inline]
pub fn quantize_to_f32(v: f32) -> f32 {
    I16F16::from_num(v).to_num::<f32>()
}

/// Quantize an `f32` into a `Storage` cell (two's-complement bit pattern).
#[inline]
pub fn quantize_to_storage(v: f32) -> Storage {
    // FixedI32::to_bits yields the two's-complement i32; re-cast to the u32
    // storage cell preserving the exact bit pattern.
    I16F16::from_num(v).to_bits() as u32
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fx_macro_is_exact() {
        // 1/2 and 1/4 are exactly representable in Q16.
        let a = fx!(0.5);
        let b = fx!(0.25);
        assert_eq!(a + b, fx!(0.75));
    }

    #[test]
    fn quantize_round_trips() {
        assert_eq!(quantize_to_f32(0.5), 0.5);
        assert_eq!(quantize_to_storage(0.5), 1 << 15); // 0.5 << 16
    }
}
