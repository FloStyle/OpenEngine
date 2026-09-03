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

/// Movement that applies pure `PlayerInput` data for entity 0 (the player).
///
/// When no arrow is pressed, entity 0 keeps its stored velocity (so a
/// no-input run is identical to [`movement_system`]); when an arrow is held,
/// its velocity is derived purely from the input data. NPCs always use their
/// stored velocities. Fully deterministic, fixed-point.
pub fn movement_system_with_input(
    positions: &[openengine_contracts::Position],
    velocities: &[openengine_contracts::Velocity],
    input: &openengine_contracts::PlayerInput,
) -> Result<WorldDelta, RecoverableError> {
    let n = core::cmp::min(positions.len(), velocities.len());
    let speed = I16F16::from_num(input.speed as i32);
    let mut new_pos: Vec<openengine_contracts::Position> = Vec::with_capacity(n);
    let mut new_vel: Vec<openengine_contracts::Velocity> = Vec::with_capacity(n);
    for i in 0..n {
        let p = positions[i];
        let v = velocities[i];
        // Player derives velocity from input only when a direction is held.
        let (mut vx, mut vy) = (v.x, v.y);
        if i == 0 && (input.up | input.down | input.left | input.right) != 0 {
            vx = I16F16::from_num(0);
            vy = I16F16::from_num(0);
            if input.left != 0 {
                vx = -speed;
            }
            if input.right != 0 {
                vx = speed;
            }
            if input.up != 0 {
                vy = -speed;
            }
            if input.down != 0 {
                vy = speed;
            }
        }
        let nx = p.x + vx;
        let ny = p.y + vy;
        let (fx, fvx) = if nx < I16F16::from_num(WALL_MIN) || nx > I16F16::from_num(WALL_MAX) {
            (p.x, -vx)
        } else {
            (nx, vx)
        };
        let (fy, fvy) = if ny < I16F16::from_num(WALL_MIN) || ny > I16F16::from_num(WALL_MAX) {
            (p.y, -vy)
        } else {
            (ny, vy)
        };
        new_pos.push(openengine_contracts::Position { x: fx, y: fy });
        new_vel.push(openengine_contracts::Velocity { x: fvx, y: fvy });
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

#[cfg(test)]
mod with_input_tests {
    use super::*;
    use openengine_contracts::PlayerInput;
    use openengine_contracts::Position;

    fn pos_vel() -> (Vec<Position>, Vec<openengine_contracts::Velocity>) {
        let pos = vec![Position {
            x: I16F16::from_num(100),
            y: I16F16::from_num(100),
        }];
        let vel = vec![openengine_contracts::Velocity {
            x: I16F16::from_num(0),
            y: I16F16::from_num(0),
        }];
        (pos, vel)
    }

    fn first_position(delta: &WorldDelta) -> Position {
        // Position ColumnWrite payload = packed Position; read first element.
        let w = delta
            .writes
            .iter()
            .find(|w| w.component.0 == comp::POSITION)
            .unwrap();
        bytemuck::pod_read_unaligned::<Position>(&w.payload[..core::mem::size_of::<Position>()])
    }

    #[test]
    fn left_input_moves_player_left_purely() {
        let (p, v) = pos_vel();
        let input = PlayerInput {
            left: 1,
            ..PlayerInput::none()
        };
        let d = movement_system_with_input(&p, &v, &input).unwrap();
        let pos = first_position(&d);
        assert!(
            pos.x < p[0].x,
            "holding left must move the player -x in pure guest logic"
        );
    }

    #[test]
    fn no_input_keeps_stored_velocity() {
        let (p, v) = pos_vel();
        let none = PlayerInput::none();
        let a = movement_system_with_input(&p, &v, &none).unwrap();
        let b = movement_system(&p, &v).unwrap();
        assert_eq!(a.writes.len(), b.writes.len());
        // Same payload bytes => pure no-input path is identical to movement_system.
        for (wa, wb) in a.writes.iter().zip(b.writes.iter()) {
            assert_eq!(wa.payload, wb.payload);
        }
    }
}

// ────────────────────────────────────────────────────────────────────────────
// § Phase E — headless 3D gameplay (homogeneous mono-archetype layout)
//
// Pure function over SoA slices: player (entity 0) driven by InputState3D
// (WASD + jump), NPCs (entities 1..N) by deterministic behaviours (kind 1 =
// wander, 2 = circle, 3 = chase). Gravity/ground/jump in fixed-point, no f32,
// no trig/sqrt — integer-derived, bit-deterministic. Returns batched writes.
// ────────────────────────────────────────────────────────────────────────────

use openengine_contracts::{Actor, Fx16, InputState3D, Transform, Velocity3D};

/// Gravity added to a rising/falling y-velocity per tick while airborne.
const GRAVITY_TICK: i32 = -2;

/// Deterministic xorshift32 (same seed => same stream on every platform).
fn xorshift32(mut state: u32) -> u32 {
    state ^= state << 13;
    state ^= state >> 17;
    state ^= state << 5;
    state
}

fn pack3<T: bytemuck::Pod>(rows: &[T]) -> alloc::vec::Vec<u8> {
    let mut out = alloc::vec![0u8; core::mem::size_of_val(rows)];
    let mut i = 0usize;
    for r in rows {
        let b = bytemuck::bytes_of(r);
        out[i..i + b.len()].copy_from_slice(b);
        i += b.len();
    }
    out
}

/// One deterministic gameplay tick.
pub fn gameplay_tick(
    frame: u64,
    transforms: &[Transform],
    velocities: &[Velocity3D],
    actors: &[Actor],
    input: &InputState3D,
) -> Result<WorldDelta, RecoverableError> {
    let n = core::cmp::min(
        core::cmp::min(transforms.len(), velocities.len()),
        actors.len(),
    );
    let mut t = transforms[..n].to_vec();
    let mut v = velocities[..n].to_vec();
    let mut a = actors[..n].to_vec();

    for i in 0..n {
        let mut pos = t[i].position;
        let vel = v[i].linear;
        let actor = a[i];
        let (mut vx, mut vy, mut vz) = (vel[0], vel[1], vel[2]);

        if i == 0 {
            // Player: WASD -> horizontal velocity from pure input data.
            let s = actor.move_speed;
            let (mut mx, mut mz) = (Fx16::from_num(0), Fx16::from_num(0));
            if input.forward != 0 {
                mz = -s;
            }
            if input.backward != 0 {
                mz = s;
            }
            if input.left != 0 {
                mx = -s;
            }
            if input.right != 0 {
                mx = s;
            }
            vx = mx;
            vz = mz;
            // Jump.
            if input.jump != 0 && actor.grounded != 0 && actor.jump_cd == 0 {
                vy = actor.jump_force;
                a[i].grounded = 0;
                a[i].jump_cd = 12;
            }
        } else {
            match actor.kind {
                1 => {
                    // Wander: pick a new 8-direction on a fixed cadence.
                    if frame % 120 == 0 {
                        let r = xorshift32(actor.seed.wrapping_add(frame as u32));
                        let dir = r % 8;
                        let (dx, dz) = match dir {
                            0 => (1, 0),
                            1 => (1, 1),
                            2 => (0, 1),
                            3 => (-1, 1),
                            4 => (-1, 0),
                            5 => (-1, -1),
                            6 => (0, -1),
                            _ => (1, -1),
                        };
                        vx = Fx16::from_num(dx * actor.move_speed.to_num::<i32>());
                        vz = Fx16::from_num(dz * actor.move_speed.to_num::<i32>());
                    }
                }
                2 => {
                    // Circle around origin (deterministic tangential drift).
                    let r = Fx16::from_num(10);
                    let sx = Fx16::from_num(actor.move_speed.to_num::<i32>());
                    vx = -(pos[2] * sx) / r;
                    vz = (pos[0] * sx) / r;
                }
                _ => {
                    // Chase entity 0: integer axis step toward the player.
                    let p = t[0].position;
                    vx = if p[0] > pos[0] {
                        actor.move_speed
                    } else if p[0] < pos[0] {
                        -actor.move_speed
                    } else {
                        Fx16::from_num(0)
                    };
                    vz = if p[2] > pos[2] {
                        actor.move_speed
                    } else if p[2] < pos[2] {
                        -actor.move_speed
                    } else {
                        Fx16::from_num(0)
                    };
                }
            }
        }

        // Gravity while airborne.
        if actor.grounded == 0 {
            vy += Fx16::from_num(GRAVITY_TICK);
        }
        // Integrate (velocity in units/tick).
        pos[0] += vx;
        pos[1] += vy;
        pos[2] += vz;
        // Ground clamp.
        if pos[1] <= Fx16::from_num(0) && vy <= Fx16::from_num(0) {
            pos[1] = Fx16::from_num(0);
            vy = Fx16::from_num(0);
            a[i].grounded = 1;
        }
        if a[i].jump_cd > 0 {
            a[i].jump_cd -= 1;
        }

        t[i].position = pos;
        v[i] = Velocity3D {
            linear: [vx, vy, vz],
        };
    }

    let indices: Vec<u32> = (0..n as u32).collect();
    let mut delta = WorldDelta::default();
    delta.writes.push(ColumnWrite {
        archetype: ArchetypeId(0),
        component: ComponentId(comp::TRANSFORM),
        indices: indices.clone(),
        payload: pack3(&t),
    });
    delta.writes.push(ColumnWrite {
        archetype: ArchetypeId(0),
        component: ComponentId(comp::VELOCITY3D),
        indices: indices.clone(),
        payload: pack3(&v),
    });
    delta.writes.push(ColumnWrite {
        archetype: ArchetypeId(0),
        component: ComponentId(comp::ACTOR),
        indices,
        payload: pack3(&a),
    });
    Ok(delta)
}

#[cfg(test)]
mod gameplay_tests {
    use super::*;
    use openengine_contracts::Transform;
    use openengine_math::I16F16 as F;

    fn read_tx(delta: &WorldDelta) -> Vec<Transform> {
        let w = delta
            .writes
            .iter()
            .find(|w| w.component.0 == comp::TRANSFORM)
            .unwrap();
        (0..(w.payload.len() / core::mem::size_of::<Transform>()))
            .map(|i| bytemuck::pod_read_unaligned::<Transform>(&w.payload[i * 40..i * 40 + 40]))
            .collect()
    }
    fn read_vy(delta: &WorldDelta) -> Vec<Velocity3D> {
        let w = delta
            .writes
            .iter()
            .find(|w| w.component.0 == comp::VELOCITY3D)
            .unwrap();
        (0..(w.payload.len() / core::mem::size_of::<Velocity3D>()))
            .map(|i| bytemuck::pod_read_unaligned::<Velocity3D>(&w.payload[i * 12..i * 12 + 12]))
            .collect()
    }
    fn read_actor(delta: &WorldDelta) -> Vec<Actor> {
        let w = delta
            .writes
            .iter()
            .find(|w| w.component.0 == comp::ACTOR)
            .unwrap();
        (0..(w.payload.len() / core::mem::size_of::<Actor>()))
            .map(|i| bytemuck::pod_read_unaligned::<Actor>(&w.payload[i * 24..i * 24 + 24]))
            .collect()
    }
    fn at(x: i32, y: i32, z: i32) -> Transform {
        Transform::at(F::from_num(x), F::from_num(y), F::from_num(z))
    }
    fn step(
        t: &mut Vec<Transform>,
        v: &mut Vec<Velocity3D>,
        a: &mut Vec<Actor>,
        frame: u64,
        input: &InputState3D,
    ) {
        let d = gameplay_tick(frame, t, v, a, input).unwrap();
        *t = read_tx(&d);
        *v = read_vy(&d);
        *a = read_actor(&d);
    }

    #[test]
    fn horizontal_forward_moves_negative_z() {
        let mut t = vec![at(0, 0, 0)];
        let mut v = vec![Velocity3D::zero()];
        let mut a = vec![Actor::player(F::from_num(5), F::from_num(30))];
        let input = InputState3D {
            forward: 1,
            ..InputState3D::none()
        };
        for f in 0..100 {
            step(&mut t, &mut v, &mut a, f, &input);
        }
        assert!(
            t[0].position[2] < F::from_num(-100),
            "forward must move the player along -Z"
        );
    }

    #[test]
    fn jump_rises_then_lands() {
        let mut t = vec![at(0, 0, 0)];
        let mut v = vec![Velocity3D::zero()];
        let mut a = vec![Actor::player(F::from_num(5), F::from_num(40))];
        let jump = InputState3D {
            jump: 1,
            ..InputState3D::none()
        };
        step(&mut t, &mut v, &mut a, 0, &jump);
        assert_eq!(a[0].grounded, 0, "jump must leave the ground");
        assert!(
            v[0].linear[1] > F::from_num(0),
            "jump must give upward velocity"
        );
        // Continue (no input) until it lands.
        let still = InputState3D::none();
        for f in 1..200 {
            step(&mut t, &mut v, &mut a, f, &still);
        }
        assert_eq!(a[0].grounded, 1, "must land again");
        assert_eq!(t[0].position[1], F::from_num(0), "must clamp at ground y=0");
    }

    #[test]
    fn full_gameplay_determinism_3x() {
        let run = || {
            let mut t: Vec<Transform> = (0..100)
                .map(|i| at((i % 10) * 2, 0, (i / 10) * 2))
                .collect();
            let mut v: Vec<Velocity3D> = vec![Velocity3D::zero(); 100];
            let mut a: Vec<Actor> = (0..100)
                .map(|_| Actor::player(F::from_num(5), F::from_num(30)))
                .collect();
            for (i, slot) in a.iter_mut().enumerate().skip(1) {
                *slot = Actor::npc(if i % 2 == 0 { 1 } else { 3 }, i as u32);
            }
            for f in 0..1000 {
                let input = if f < 200 {
                    InputState3D {
                        forward: 1,
                        ..InputState3D::none()
                    }
                } else {
                    InputState3D::none()
                };
                step(&mut t, &mut v, &mut a, f, &input);
            }
            (t, v, a)
        };
        let (t1, v1, a1) = run();
        let (t2, v2, a2) = run();
        let (t3, v3, a3) = run();
        assert_eq!(t1, t2);
        assert_eq!(t2, t3);
        assert_eq!(v1, v2);
        assert_eq!(v2, v3);
        assert_eq!(a1, a2);
        assert_eq!(a2, a3);
    }

    #[test]
    fn npc_chase_moves_toward_player() {
        // Player at origin (idle); NPC at x=+20 chasing (kind 3).
        let mut t = vec![at(0, 0, 0), at(20, 0, 0)];
        let mut v = vec![Velocity3D::zero(), Velocity3D::zero()];
        let mut a = vec![
            Actor::player(F::from_num(5), F::from_num(30)),
            Actor::npc(3, 7),
        ];
        for f in 0..10 {
            step(&mut t, &mut v, &mut a, f, &InputState3D::none());
        }
        assert!(
            t[1].position[0] < F::from_num(20),
            "chaser must move toward the player (-X)"
        );
    }
}
