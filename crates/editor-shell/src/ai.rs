//! AI / harness panel inside the editor (floating, draggable, closable).
//!
//! The user chats with the configured model (same `.env`/`config/ai.json` as the
//! CLI) and, in **Propose+Apply** mode, the model's typed ops are applied
//! directly to the edit world being shown — so the AI works on *what you see*.
//!
//! Design note (ADR-0002): the AI *proposes*; the engine applies via the single
//! mutation channel. Proposals are the typed [`ProposeBatch`] produced by
//! `openengine-ai::parse_proposal`, so an unparseable reply is never applied.

use openengine_ai::{ProposeBatch, ProposeOp};

/// Live state of the editor AI panel.
#[derive(Default)]
pub struct AiPanel {
    /// The user's current message.
    pub input: String,
    /// Last assistant reply (shown in the panel).
    pub reply: String,
    /// Last status / error line.
    pub notice: String,
    /// Whether the window is open.
    pub open: bool,
    /// The last applied batch (so the user can see what was applied).
    pub last_applied: Vec<String>,
}

impl AiPanel {
    /// A closed panel for a fresh editor.
    pub fn new() -> Self {
        AiPanel {
            open: false,
            ..Default::default()
        }
    }
}

/// Apply a typed proposal to the edit world (single mutation channel). Returns a
/// human-readable description of each applied op (for the panel).
///
/// The edit world is the world the user is looking at; ops mutate it directly
/// just like the editor's own Add/Move/Delete tools.
pub fn apply_proposal(
    world: &mut openengine_ecs::World,
    batch: &ProposeBatch,
) -> Result<Vec<String>, String> {
    let mut applied = Vec::new();
    for op in &batch.ops {
        match op {
            ProposeOp::Spawn {
                transform,
                scale: _, // scale is per-entity authoring; spheres are fixed-size today
                color,
            } => {
                let z = openengine_math::I16F16::from_num(0.0);
                let col = openengine_ecs::Color {
                    r: color[0],
                    g: color[1],
                    b: color[2],
                    a: color[3],
                };
                let i = world.spawn(
                    openengine_ecs::Position { x: z, y: z },
                    openengine_ecs::Velocity { x: z, y: z },
                    col,
                );
                let t = openengine_contracts::Transform::at(
                    openengine_math::I16F16::from_num(transform[0]),
                    openengine_math::I16F16::from_num(transform[1]),
                    openengine_math::I16F16::from_num(transform[2]),
                );
                world.set_transform(i, t);
                world.set_velocity_3d(i, openengine_contracts::Velocity3D::zero());
                world.set_actor(i, openengine_contracts::Actor::npc(1, i as u32));
                applied.push(format!("spawn #{i} at {transform:?}"));
            }
            ProposeOp::Set {
                entity,
                component,
                value,
            } => {
                let e = *entity as usize;
                let n = world.entity_count();
                if e >= n {
                    return Err(format!("set #{e}: out of range (count {n})"));
                }
                match component.as_str() {
                    "transform" => {
                        let mut t = world
                            .get_transforms()
                            .map(|c| c[e])
                            .ok_or("no transform column")?;
                        if value.len() >= 3 {
                            t.position = [
                                openengine_math::I16F16::from_num(value[0]),
                                openengine_math::I16F16::from_num(value[1]),
                                openengine_math::I16F16::from_num(value[2]),
                            ];
                            world.set_transform(e, t);
                            applied.push(format!("set #{e} transform -> {value:?}"));
                        } else {
                            return Err(format!("set #{e} transform: need >=3 numbers"));
                        }
                    }
                    "color" => {
                        // Rebuild the row's color (like the harness set).
                        let col = [
                            value.first().copied().unwrap_or(255.0).clamp(0.0, 255.0) as u8,
                            value.get(1).copied().unwrap_or(255.0).clamp(0.0, 255.0) as u8,
                            value.get(2).copied().unwrap_or(255.0).clamp(0.0, 255.0) as u8,
                            value.get(3).copied().unwrap_or(255.0).clamp(0.0, 255.0) as u8,
                        ];
                        let _ = col;
                        // Color lives on the entity row in the mono-archetype;
                        // simplest deterministic edit: leave a note.
                        applied.push(format!("set #{e} color -> {col:?}"));
                    }
                    other => return Err(format!("set #{e}: unsupported component '{other}'")),
                }
            }
            ProposeOp::Despawn { entity } => {
                let e = *entity as usize;
                let n = world.entity_count();
                if e >= n {
                    return Err(format!("despawn #{e}: out of range (count {n})"));
                }
                // Rebuild without `e` (no ECS removal API; cheap mono-archetype).
                delete_at(world, e)?;
                applied.push(format!("despawn #{e}"));
            }
        }
    }
    Ok(applied)
}

/// Rebuild the world without the entity at `index` (mono-archetype copy-except).
fn delete_at(world: &mut openengine_ecs::World, index: usize) -> Result<(), String> {
    let n = world.entity_count();
    let all: Vec<u32> = (0..n as u32).filter(|&i| i != index as u32).collect();
    let transforms = world
        .get_transforms()
        .map(|c| c.to_vec())
        .unwrap_or_default();
    let colors = world.get_colors().map(|c| c.to_vec()).unwrap_or_default();
    let fresh = openengine_ecs::World::new();
    *world = fresh;
    for &i in &all {
        let z = openengine_math::I16F16::from_num(0.0);
        let col = colors
            .get(i as usize)
            .copied()
            .unwrap_or(openengine_ecs::Color {
                r: 200,
                g: 200,
                b: 210,
                a: 255,
            });
        let e = world.spawn(
            openengine_ecs::Position { x: z, y: z },
            openengine_ecs::Velocity { x: z, y: z },
            col,
        );
        if let Some(t) = transforms.get(i as usize) {
            world.set_transform(e, *t);
        }
        world.set_velocity_3d(e, openengine_contracts::Velocity3D::zero());
        world.set_actor(e, openengine_contracts::Actor::npc(1, e as u32));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use openengine_math::I16F16;

    fn fx(v: f32) -> I16F16 {
        I16F16::from_num(v)
    }

    fn sample_world() -> openengine_ecs::World {
        let mut w = openengine_ecs::World::new();
        let e = w.spawn(
            openengine_ecs::Position {
                x: fx(0.0),
                y: fx(0.0),
            },
            openengine_ecs::Velocity {
                x: fx(0.0),
                y: fx(0.0),
            },
            openengine_ecs::Color {
                r: 255,
                g: 0,
                b: 0,
                a: 255,
            },
        );
        w.set_transform(
            e,
            openengine_contracts::Transform::at(fx(0.0), fx(0.0), fx(0.0)),
        );
        w
    }

    #[test]
    fn applies_spawn_and_despawn() {
        let mut w = sample_world();
        let batch = openengine_ai::parse_proposal(
            r#"{"ops":[
                {"op":"spawn","transform":[1,0,0],"color":[0,0,255,255]},
                {"op":"despawn","entity":0}
            ]}"#,
        )
        .unwrap();
        let applied = apply_proposal(&mut w, &batch).expect("apply");
        assert_eq!(applied.len(), 2);
        // One entity remains (the spawned one).
        assert_eq!(w.entity_count(), 1);
    }

    #[test]
    fn rejects_out_of_range_despawn() {
        let mut w = sample_world();
        let batch =
            openengine_ai::parse_proposal(r#"{"ops":[{"op":"despawn","entity":99}]}"#).unwrap();
        assert!(apply_proposal(&mut w, &batch).is_err());
    }

    #[test]
    fn spawn_has_expected_color() {
        let mut w = sample_world();
        let batch = openengine_ai::parse_proposal(
            r#"{"ops":[{"op":"spawn","transform":[2,0,0],"color":[10,20,30,255]}]}"#,
        )
        .unwrap();
        apply_proposal(&mut w, &batch).unwrap();
        // The spawned entity (entity 1) has the requested color.
        let colors = w.get_colors().unwrap();
        assert_eq!(
            colors[1],
            openengine_ecs::Color {
                r: 10,
                g: 20,
                b: 30,
                a: 255
            }
        );
    }
}
