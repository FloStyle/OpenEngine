//! Native (Domain A) movement system — PoC Phase B.
//!
//! Proves the **single mutation channel**: reads the world, returns a
//! [`WorldDelta`] of batched `ColumnWrite`s, and never holds `&mut` into a
//! column. The host applies the delta with `World::apply_delta`.

use openengine_contracts::{ArchetypeId, ColumnWrite, ComponentId, WorldDelta};
use openengine_ecs::{Position, Velocity, World, POSITION, VELOCITY};
use openengine_math::I16F16;

/// World bounds for the bounce demo (in fixed units).
const WALL_MIN: i32 = 0;
const WALL_MAX: i32 = 500;

/// Run one movement tick: `pos += vel`, bouncing off the `0..500` walls.
///
/// Produces a [`WorldDelta`] containing two batched `ColumnWrite`s (all
/// positions; all velocities) applied through `World::apply_delta`.
pub fn native_movement_system(world: &World) -> WorldDelta {
    let n = world.entity_count();
    let positions = world.get_positions().map(|s| &s[..n]).unwrap_or(&[]);
    let velocities = world.get_velocities().map(|s| &s[..n]).unwrap_or(&[]);

    let mut new_pos = Vec::with_capacity(n);
    let mut new_vel = Vec::with_capacity(n);
    for i in 0..n {
        let p = positions[i];
        let v = velocities[i];
        let nx = p.x + v.x;
        let ny = p.y + v.y;
        // Bounce: clamp to wall and flip the offending axis.
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

    let mut delta = WorldDelta::default();
    let indices: Vec<u32> = (0..n as u32).collect();
    delta.writes.push(ColumnWrite {
        archetype: ArchetypeId(0),
        component: ComponentId(POSITION),
        indices: indices.clone(),
        payload: pack::<Position>(&new_pos),
    });
    delta.writes.push(ColumnWrite {
        archetype: ArchetypeId(0),
        component: ComponentId(VELOCITY),
        indices,
        payload: pack::<Velocity>(&new_vel),
    });
    delta
}

/// Pack a slice of `Pod` elements into a contiguous byte payload for a batched
/// column write (length = `len * size_of::<T>()`).
fn pack<T: bytemuck::Pod>(rows: &[T]) -> Vec<u8> {
    let mut out = Vec::with_capacity(core::mem::size_of_val(rows));
    for row in rows {
        out.extend_from_slice(bytemuck::bytes_of(row));
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use openengine_ecs::Color;

    #[test]
    fn bounces_off_walls_deterministically() {
        let mut world = World::new();
        world.spawn(
            Position {
                x: I16F16::from_num(490),
                y: I16F16::from_num(490),
            },
            Velocity {
                x: I16F16::from_num(20),
                y: I16F16::from_num(20),
            },
            Color {
                r: 255,
                g: 0,
                b: 0,
                a: 255,
            },
        );
        // Crossing the wall flips velocity and pins position at the wall.
        let delta = native_movement_system(&world);
        world.apply_delta(&delta);
        let p = world.get_positions().unwrap()[0];
        let v = world.get_velocities().unwrap()[0];
        assert_eq!(p.x, I16F16::from_num(490));
        assert_eq!(p.y, I16F16::from_num(490));
        assert_eq!(v.x, -I16F16::from_num(20));
        assert_eq!(v.y, -I16F16::from_num(20));
    }
}
