//! Portable scene codec shared by the editor, the headless player/runner, and
//! the cook/package pipeline.
//!
//! A *scene* is authored game content: a list of entities, each carrying its
//! authored components (all SoA columns of the mono-archetype `World`). The
//! format is **versioned** (a future engine can migrate or reject older files)
//! and stores fixed values losslessly as `f32` — their `2^-16` steps fit exactly
//! in an `f32` mantissa, so a save→load round-trip preserves the bit-identical
//! world (`World::hash()`).

use openengine_contracts::{Actor, Transform, Velocity3D};
use openengine_math::I16F16 as F;

use crate::components::Color;
use crate::world::World;

/// Current scene format version.
pub const SCENE_VERSION: u32 = 1;

/// A versioned list of authored entities.
#[derive(Clone, Debug, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct SceneContent {
    /// Scene format version (reject/migrate older files).
    pub version: u32,
    /// The authored entities.
    pub entities: Vec<SceneEntity>,
}

/// One entity's authored components (all SoA columns, in game terms).
#[derive(Clone, Debug, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct SceneEntity {
    /// World position (x, y, z).
    pub transform: [f32; 3],
    /// Scale (x, y, z).
    pub scale: [f32; 3],
    /// RGBA color.
    pub color: [u8; 4],
    /// Linear velocity (x, y, z).
    pub velocity: [f32; 3],
    /// Actor behaviour kind (0=player,1=wander,2=circle,3=chase).
    pub actor_kind: u32,
    /// Deterministic NPC seed.
    pub actor_seed: u32,
    /// 1 = on the ground.
    pub grounded: u32,
    /// Cooldown ticks before the next jump is allowed.
    pub jump_cd: u32,
    /// Horizontal move speed (fixed).
    pub move_speed: f32,
    /// Jump impulse (fixed).
    pub jump_force: f32,
}

fn fx(v: f32) -> F {
    F::from_num(v)
}

fn tx(pos: [f32; 3], scale: [f32; 3]) -> Transform {
    Transform {
        position: [fx(pos[0]), fx(pos[1]), fx(pos[2])],
        rotation: [fx(0.0), fx(0.0), fx(0.0), fx(1.0)],
        scale: [fx(scale[0]), fx(scale[1]), fx(scale[2])],
    }
}

/// Serialize a whole `World` into portable [`SceneContent`] (all columns).
pub fn content_from_world(world: &World) -> SceneContent {
    let n = world.entity_count();
    let transforms = world.get_transforms().unwrap_or(&[]);
    let vels = world.get_velocity_3d().unwrap_or(&[]);
    let actors = world.get_actors().unwrap_or(&[]);
    let colors = world.get_colors().unwrap_or(&[]);
    let mut entities = Vec::with_capacity(n);
    for i in 0..n {
        let t = transforms
            .get(i)
            .copied()
            .unwrap_or_else(|| tx([0.0; 3], [1.0; 3]));
        let v = vels.get(i).copied().unwrap_or(Velocity3D::zero());
        let a = actors.get(i).copied().unwrap_or_else(|| Actor::npc(1, 1));
        let c = colors.get(i).copied().unwrap_or(Color {
            r: 255,
            g: 255,
            b: 255,
            a: 255,
        });
        entities.push(SceneEntity {
            transform: [
                t.position[0].to_num(),
                t.position[1].to_num(),
                t.position[2].to_num(),
            ],
            scale: [
                t.scale[0].to_num(),
                t.scale[1].to_num(),
                t.scale[2].to_num(),
            ],
            color: [c.r, c.g, c.b, c.a],
            velocity: [
                v.linear[0].to_num(),
                v.linear[1].to_num(),
                v.linear[2].to_num(),
            ],
            actor_kind: a.kind,
            actor_seed: a.seed,
            grounded: a.grounded,
            jump_cd: a.jump_cd,
            move_speed: a.move_speed.to_num(),
            jump_force: a.jump_force.to_num(),
        });
    }
    SceneContent {
        version: SCENE_VERSION,
        entities,
    }
}

/// Build a fresh `World` from [`SceneContent`]. Rejects an incompatible version.
pub fn world_from_content(scene: &SceneContent) -> Result<World, String> {
    if scene.version != SCENE_VERSION {
        return Err(format!("unsupported scene version {}", scene.version));
    }
    let mut w = World::new();
    for e in &scene.entities {
        let idx = w.spawn(
            crate::components::Position {
                x: fx(0.0),
                y: fx(0.0),
            },
            crate::components::Velocity {
                x: fx(0.0),
                y: fx(0.0),
            },
            Color {
                r: e.color[0],
                g: e.color[1],
                b: e.color[2],
                a: e.color[3],
            },
        );
        w.set_transform(idx, tx(e.transform, e.scale));
        w.set_velocity_3d(
            idx,
            Velocity3D {
                linear: [fx(e.velocity[0]), fx(e.velocity[1]), fx(e.velocity[2])],
            },
        );
        w.set_actor(
            idx,
            Actor {
                kind: e.actor_kind,
                seed: e.actor_seed,
                grounded: e.grounded,
                jump_cd: e.jump_cd,
                move_speed: fx(e.move_speed),
                jump_force: fx(e.jump_force),
            },
        );
    }
    Ok(w)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::components::Color;

    fn build() -> World {
        let mut w = World::new();
        let p = w.spawn(
            crate::components::Position {
                x: fx(0.0),
                y: fx(0.0),
            },
            crate::components::Velocity {
                x: fx(0.0),
                y: fx(0.0),
            },
            Color {
                r: 10,
                g: 20,
                b: 30,
                a: 255,
            },
        );
        w.set_transform(p, tx([1.5, -2.0, 3.25], [2.0, 1.0, 1.0]));
        w.set_velocity_3d(
            p,
            Velocity3D {
                linear: [fx(1.0), fx(-0.5), fx(0.25)],
            },
        );
        w.set_actor(p, Actor::player(fx(3.0), fx(40.0)));
        let n = w.spawn(
            crate::components::Position {
                x: fx(0.0),
                y: fx(0.0),
            },
            crate::components::Velocity {
                x: fx(0.0),
                y: fx(0.0),
            },
            Color {
                r: 200,
                g: 90,
                b: 90,
                a: 255,
            },
        );
        w.set_transform(n, tx([10.0, 0.0, 0.0], [1.0; 3]));
        w.set_actor(n, Actor::npc(3, 7));
        w
    }

    #[test]
    fn content_roundtrip_preserves_world_bit_for_bit() {
        let original = build();
        let h0 = original.hash();
        let content = content_from_world(&original);
        assert_eq!(content.entities.len(), 2);
        let rebuilt = world_from_content(&content).unwrap();
        assert_eq!(
            original.hash(),
            rebuilt.hash(),
            "content round-trip must preserve the world"
        );
        // And the loaded world is genuinely a fresh, equal instance.
        assert_eq!(h0, rebuilt.hash());
    }

    #[test]
    fn world_from_content_rejects_unknown_version() {
        let original = build();
        let mut content = content_from_world(&original);
        content.version = 999;
        assert!(world_from_content(&content).is_err());
    }
}
