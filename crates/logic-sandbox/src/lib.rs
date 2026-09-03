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
//!    is forbidden *inside the maths* because it is not bit-deterministic
//!    across hosts. `f32` appears only at the instant a value is emitted to
//!    the display boundary (see [`DeferredCommand::ClearColor`]).
//! 5. You may only *observe* state through [`StateView`] and only *propose*
//!    change through the returned [`WorldDelta`]. State is immutable; side
//!    effects are forbidden.
//!
//! ## The current vertical slice: `tick_color`
//!
//! [`tick_color`] is the pure system that powers "the living window". Given a
//! frame `tick`, it computes a cycling RGB value with deterministic fixed-point
//! arithmetic and returns it as a [`WorldDelta`] carrying a single
//! [`DeferredCommand::ClearColor`]. The host renders that color.

// The shipped Domain-B artifact is ALWAYS built for `wasm32-unknown-unknown`,
// so there this crate is `#![no_std]`. When the host compiles it as an `rlib`
// (for unit tests / rust-analyzer on the native target) we let `std` in so the
// toolchain can link — that host build is never the artifact that ships.
#![cfg_attr(target_arch = "wasm32", no_std)]
#![forbid(unsafe_code)]

extern crate alloc;

use contracts::{DeferredCommand, RecoverableError, StateView, WorldDelta};
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
// The tiny host-call trampoline (which contains NO logic, only a typed call
// through the ABI) lives in the separate `logic-export` crate, which is built
// for wasm without that forbid. The pure logic here never needs a raw export.
// That crate is also where the guest panic handler + global allocator live, so
// this rlib defines neither (a lang item may only appear once in the link).

// ────────────────────────────────────────────────────────────────────────────
// § The "living window" pure system
// ────────────────────────────────────────────────────────────────────────────

/// A fixed-point scalar for color channels. `Q16.16`, exact integer math.
type ColorFixed = openengine_math::I16F16;

/// Length of one full colour cycle, in ticks (a triangle wave up & back).
const CYCLE_TICKS: u64 = 510;

/// Triangle-wave intensity in `0..=255` for a given tick.
///
/// Deterministic by construction: a pure function of `tick` — no sine tables,
/// no floating point, no ambient randomness. Over one cycle the value ramps
/// `0→255→0`, which keeps the colour visibly cycling.
fn wave(tick: u64) -> u16 {
    let m = tick % CYCLE_TICKS;
    if m < (CYCLE_TICKS / 2) {
        m as u16
    } else {
        (CYCLE_TICKS - m) as u16
    }
}

/// Normalise a `0..=255` channel to the fixed interval `0.0..=1.0`.
fn channel_fixed(byte: u16) -> ColorFixed {
    ColorFixed::from_num(byte as i32) / ColorFixed::from_num(255)
}

/// Convert a fixed colour channel to `f32` — the ONLY place raw `f32` enters.
///
/// This happens solely because the [`DeferredCommand::ClearColor`] ABI and the
/// GPU both speak `f32`. All computation stays fixed-point; the conversion is a
/// lossless-enough last step that is identical across hosts.
fn channel_f32(byte: u16) -> f32 {
    channel_fixed(byte).to_num::<f32>()
}

/// Pure RGB computation for a tick. Returns channels `0..=255`.
///
/// Three triangle waves offset by a third of the cycle so the channels never
/// peak together, producing a continuously shifting hue.
pub fn rgb_for_tick(tick: u64) -> [u16; 3] {
    let third = CYCLE_TICKS / 3; // 170
    [
        wave(tick),
        wave(tick.wrapping_add(third)),
        wave(tick.wrapping_add(2 * third)),
    ]
}

/// The pure system that drives the living window.
///
/// Reads the frame tick from the view, computes a deterministic colour with
/// fixed-point math, and returns a [`WorldDelta`] whose single deferred command
/// is a [`DeferredCommand::ClearColor`]. This is the exact signature the host
/// trampoline (`logic-export`) drives through the Wasm boundary.
pub fn tick_color(view: &StateView<'_>) -> Result<WorldDelta, RecoverableError> {
    let [r, g, b] = rgb_for_tick(view.tick);
    let mut delta = WorldDelta::default();
    delta.deferred.push(DeferredCommand::ClearColor {
        rgba: [channel_f32(r), channel_f32(g), channel_f32(b), 1.0],
    });
    Ok(delta)
}

#[cfg(test)]
mod tests {
    use super::*;
    use openengine_contracts::{DeferredCommand, StateView};

    #[test]
    fn tick_color_returns_a_clear_color_delta() {
        let view = StateView::tick_only(10);
        let delta = tick_color(&view).expect("pure system cannot fail");
        let rgba = delta.clear_color().expect("must carry a ClearColor");
        // Alpha is always opaque.
        assert_eq!(rgba[3], 1.0);
        // All channels within [0,1].
        assert!(rgba.iter().all(|c| (0.0..=1.0).contains(c)));
    }

    #[test]
    fn color_is_deterministic() {
        let view = StateView::tick_only(37);
        let a = tick_color(&view).unwrap();
        let b = tick_color(&view).unwrap();
        // Two identical inputs -> bit-identical outputs (the whole point).
        let rgba_a = a.clear_color().unwrap();
        let rgba_b = b.clear_color().unwrap();
        assert_eq!(rgba_a, rgba_b);
    }

    #[test]
    fn rgb_waves_never_exceed_bounds() {
        for tick in 0..2000 {
            let [r, g, b] = rgb_for_tick(tick);
            assert!(r <= 255 && g <= 255 && b <= 255);
        }
    }

    #[test]
    fn emits_only_deferred_clear() {
        let view = StateView::tick_only(1);
        let delta = tick_color(&view).unwrap();
        assert!(delta.spawns.is_empty());
        assert!(delta.despawns.is_empty());
        assert!(delta.writes.is_empty());
        assert!(matches!(
            delta.deferred.first(),
            Some(DeferredCommand::ClearColor { .. })
        ));
    }
}

// ────────────────────────────────────────────────────────────────────────────
// § SoA bridge movement (Phase 3 / ADR-0001)
//
// Pure function: given read-only SoA `Position`/`Velocity` slices (bytes that
// the host wrote into a guest-allocated buffer), returns a batched `WorldDelta`
// of new positions/velocities. This is Domain B logic — no `unsafe`, no I/O,
// fixed-point only. It is byte-identical to `openengine_core::native_movement`.
// ────────────────────────────────────────────────────────────────────────────

use alloc::vec;
use alloc::vec::Vec;
use openengine_contracts::comp;
use openengine_contracts::{ArchetypeId, ColumnWrite, ComponentId, Position, Velocity};
use openengine_math::I16F16;

/// Wall bounds (same units as the native demo).
const WALL_MIN: i32 = 0;
const WALL_MAX: i32 = 500;

/// Run one movement tick over the supplied SoA slices.
pub fn movement_system(
    positions: &[Position],
    velocities: &[Velocity],
) -> Result<WorldDelta, RecoverableError> {
    let n = core::cmp::min(positions.len(), velocities.len());
    let mut new_pos: Vec<Position> = Vec::with_capacity(n);
    let mut new_vel: Vec<Velocity> = Vec::with_capacity(n);
    for i in 0..n {
        let p = positions[i];
        let v = velocities[i];
        let nx = p.x + v.x;
        let ny = p.y + v.y;
        let (fx, vx) = if nx < I16F16::from_num(WALL_MIN) || nx > I16F16::from_num(WALL_MAX) {
            (p.x, -v.x)
        } else {
            (nx, v.x)
        };
        let (fy, vy) = if ny < I16F16::from_num(WALL_MIN) || ny > I16F16::from_num(WALL_MAX) {
            (p.y, -v.y)
        } else {
            (ny, v.y)
        };
        new_pos.push(Position { x: fx, y: fy });
        new_vel.push(Velocity { x: vx, y: vy });
    }

    let indices: Vec<u32> = (0..n as u32).collect();
    let mut delta = WorldDelta::default();
    delta.writes.push(ColumnWrite {
        archetype: ArchetypeId(0),
        component: ComponentId(comp::POSITION),
        indices: indices.clone(),
        payload: pack(&new_pos),
    });
    delta.writes.push(ColumnWrite {
        archetype: ArchetypeId(0),
        component: ComponentId(comp::VELOCITY),
        indices,
        payload: pack(&new_vel),
    });
    Ok(delta)
}

/// Pack `Pod` rows into a contiguous byte payload for a batched column write.
fn pack<T: bytemuck::Pod>(rows: &[T]) -> Vec<u8> {
    let mut out = vec![0u8; core::mem::size_of_val(rows)];
    let mut i = 0usize;
    for row in rows {
        let b = bytemuck::bytes_of(row);
        out[i..i + b.len()].copy_from_slice(b);
        i += b.len();
    }
    out
}

#[cfg(test)]
mod movement_tests {
    use super::*;
    use openengine_contracts::Position;

    fn world(count: usize) -> (Vec<Position>, Vec<Velocity>) {
        let pos: Vec<Position> = (0..count)
            .map(|i| Position {
                x: I16F16::from_num(((i % 10) as i32) * 50),
                y: I16F16::from_num(((i / 10) as i32) * 50),
            })
            .collect();
        let vel = vec![
            Velocity {
                x: I16F16::from_num(5),
                y: I16F16::from_num(5)
            };
            count
        ];
        (pos, vel)
    }

    #[test]
    fn movement_is_deterministic() {
        let (p1, v1) = world(100);
        let (p2, v2) = world(100);
        let d1 = movement_system(&p1, &v1).unwrap();
        let d2 = movement_system(&p2, &v2).unwrap();
        assert_eq!(d1.writes.len(), 2);
        assert_eq!(d1.writes[0].payload, d2.writes[0].payload);
        assert_eq!(d1.writes[1].payload, d2.writes[1].payload);
    }

    #[test]
    fn bounce_pins_at_wall() {
        let pos = vec![Position {
            x: I16F16::from_num(498),
            y: I16F16::from_num(0),
        }];
        let vel = vec![Velocity {
            x: I16F16::from_num(10),
            y: I16F16::from_num(0),
        }];
        let d = movement_system(&pos, &vel).unwrap();
        // payload is one packed Position; read x (first 4 bytes) back.
        let px = i32::from_le_bytes(d.writes[0].payload[0..4].try_into().unwrap());
        assert_eq!(px, 498 << 16);
    }
}
