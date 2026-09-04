//! Harness live-state: owns a [`World`] plus a deterministic tick counter and an
//! optional wasm guest. All mutation goes through `WorldDelta -> apply_delta`;
//! the harness is a thin, headless observe/mutate/verify shell over it.

use openengine_contracts::{Actor, InputState3D, Transform, Velocity3D, WorldDelta};
use openengine_ecs::{Color, Position, Velocity, World};
use openengine_math::I16F16 as F;

use crate::wasm_guest::WasmGuest;

/// Wrapper the harness exposes over a single live [`World`].
pub struct HarnessState {
    world: World,
    /// Deterministic sim tick (frame) fed to the guest / native integrator.
    tick: u64,
    /// Optional wasm guest; when present `/tick` runs the real Domain B logic.
    guest: Option<WasmGuest>,
    /// Path of the loaded logic module, so a duplicate can spawn a fresh guest.
    wasm_path: Option<String>,
}

/// Compact snapshot of one entity for `/observe`.
#[derive(Clone, Debug, PartialEq, serde::Serialize)]
pub struct EntityView {
    pub index: u32,
    pub transform: [f32; 3],
    pub scale: [f32; 3],
    pub color: [u8; 4],
}

/// Re-export the shared entity codec (single source of truth in `openengine-ecs`).
pub use openengine_ecs::scene::{SceneContent, SceneEntity, SCENE_VERSION};

/// A scene file = the shared [`SceneContent`] plus a resume tick (harness
/// concept; 0 for a freshly authored scene). The entity encoding lives in
/// `openengine-ecs::scene` so the editor, runner and package share one format.
#[derive(Clone, Debug, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct SceneFile {
    /// Shared content version.
    pub version: u32,
    /// Resume tick (sim frame) — 0 for freshly authored scenes.
    pub tick: u64,
    /// Shared authored entities (ecs codec).
    pub entities: Vec<SceneEntity>,
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

impl HarnessState {
    /// A fresh, empty live world.
    pub fn new() -> Self {
        HarnessState {
            world: World::new(),
            tick: 0,
            guest: None,
            wasm_path: None,
        }
    }

    /// Deep, independent copy of this state (world + tick). The wasm guest is
    /// re-instantiated fresh from the same module so a duplicate is truly
    /// independent and deterministic (used by `/transaction` and `/prove`).
    pub fn duplicate(&self) -> HarnessState {
        let guest = self
            .wasm_path
            .as_deref()
            .and_then(|p| WasmGuest::load(p).ok());
        HarnessState {
            world: self.world.clone_state(),
            tick: self.tick,
            guest,
            wasm_path: self.wasm_path.clone(),
        }
    }

    /// Replace this state's world + tick with `src`'s (deep copy). Used to
    /// roll back a failed transaction.
    pub fn overwrite_from(&mut self, src: &HarnessState) {
        self.world = src.world.clone_state();
        self.tick = src.tick;
    }

    pub fn entity_count(&self) -> usize {
        self.world.entity_count()
    }
    pub fn tick(&self) -> u64 {
        self.tick
    }

    /// Spawn an entity at `pos`/`scale` with `color`. Returns its index.
    pub fn spawn(&mut self, pos: [f32; 3], scale: [f32; 3], color: [u8; 4]) -> usize {
        let idx = self.world.spawn(
            Position {
                x: fx(0.0),
                y: fx(0.0),
            },
            Velocity {
                x: fx(0.0),
                y: fx(0.0),
            },
            Color {
                r: color[0],
                g: color[1],
                b: color[2],
                a: color[3],
            },
        );
        self.world.set_transform(idx, tx(pos, scale));
        idx
    }

    /// Rebuild the world without `index` (no ECS mutation API exists; the
    /// mono-archetype makes a copy-except-one cheap and deterministic). Errors
    /// if the index is out of range.
    pub fn despawn(&mut self, index: usize) -> Result<(), String> {
        let n = self.world.entity_count();
        if index >= n {
            return Err(format!("entity {index} out of range (count {n})"));
        }
        let transforms = self
            .world
            .get_transforms()
            .map(|s| s.to_vec())
            .unwrap_or_default();
        let vels = self
            .world
            .get_velocity_3d()
            .map(|s| s.to_vec())
            .unwrap_or_default();
        let actors = self
            .world
            .get_actors()
            .map(|s| s.to_vec())
            .unwrap_or_default();
        let colors = self
            .world
            .get_colors()
            .map(|s| s.to_vec())
            .unwrap_or_default();
        let mut fresh = World::new();
        for i in 0..n {
            if i == index {
                continue;
            }
            let c = colors.get(i).copied().unwrap_or(Color {
                r: 255,
                g: 255,
                b: 255,
                a: 255,
            });
            let new = fresh.spawn(
                Position {
                    x: fx(0.0),
                    y: fx(0.0),
                },
                Velocity {
                    x: fx(0.0),
                    y: fx(0.0),
                },
                c,
            );
            if let Some(t) = transforms.get(i) {
                fresh.set_transform(new, *t);
            }
            if let Some(v) = vels.get(i) {
                fresh.set_velocity_3d(new, *v);
            }
            if let Some(a) = actors.get(i) {
                fresh.set_actor(new, *a);
            }
        }
        self.world = fresh;
        Ok(())
    }

    /// Set a component. Supported: `transform` (position; optional [x,y,z,scx,
    /// scy,scz]), `scale`, `color`. Errors on bad component/index.
    pub fn set(&mut self, index: usize, component: &str, value: &[f32]) -> Result<(), String> {
        let n = self.world.entity_count();
        if index >= n {
            return Err(format!("entity {index} out of range (count {n})"));
        }
        match component {
            "transform" | "scale" => {
                let Some(tr) = self.world.get_transforms().map(|s| s[index]) else {
                    return Err("no transform column".into());
                };
                let mut t = tr;
                match component {
                    "transform" if value.len() >= 3 => {
                        t.position = [fx(value[0]), fx(value[1]), fx(value[2])];
                        if value.len() >= 6 {
                            t.scale = [fx(value[3]), fx(value[4]), fx(value[5])];
                        }
                    }
                    "scale" if value.len() >= 3 => {
                        t.scale = [fx(value[0]), fx(value[1]), fx(value[2])];
                    }
                    _ => return Err("need >=3 numbers".into()),
                }
                self.world.set_transform(index, t);
                Ok(())
            }
            "color" => {
                // Color is authoring plumbing; rebuild just that entity's row.
                let Some(tr) = self.world.get_transforms().map(|s| s[index]) else {
                    return Err("no transform".into());
                };
                let v = self
                    .world
                    .get_velocity_3d()
                    .map(|s| s[index])
                    .unwrap_or(Velocity3D::zero());
                let a = self
                    .world
                    .get_actors()
                    .map(|s| s[index])
                    .unwrap_or_else(|| Actor::npc(1, 1));
                let rgba = value
                    .iter()
                    .take(4)
                    .map(|&v| v.clamp(0.0, 255.0) as u8)
                    .collect::<Vec<_>>();
                let col = [
                    rgba[0],
                    rgba.get(1).copied().unwrap_or(255),
                    rgba.get(2).copied().unwrap_or(255),
                    rgba.get(3).copied().unwrap_or(255),
                ];
                self.despawn(index)?;
                let idx = self.spawn(
                    [
                        tr.position[0].to_num(),
                        tr.position[1].to_num(),
                        tr.position[2].to_num(),
                    ],
                    [
                        tr.scale[0].to_num(),
                        tr.scale[1].to_num(),
                        tr.scale[2].to_num(),
                    ],
                    col,
                );
                let _ = idx;
                self.world.set_transform(idx, tr);
                self.world.set_velocity_3d(idx, v);
                self.world.set_actor(idx, a);
                Ok(())
            }
            other => Err(format!("unsupported component '{other}'")),
        }
    }

    /// Run `count` ticks. If a wasm guest is loaded it runs the real Domain B
    /// logic; otherwise the world is left unchanged (identity native integrator)
    /// and only the deterministic tick counter advances.
    pub fn tick_n(&mut self, count: u64) -> Result<(), String> {
        for _ in 0..count {
            if let Some(guest) = self.guest.as_mut() {
                let delta: WorldDelta = guest
                    .tick(&self.world, self.tick)
                    .map_err(|e| format!("guest tick: {e}"))?;
                self.world.apply_delta(&delta);
            }
            // Native integrator = identity; the guest is the real sim.
            self.tick += 1;
        }
        Ok(())
    }

    /// Load a wasm logic module that exposes `openengine_gameplay_tick`.
    pub fn load_wasm(&mut self, path: &str) -> Result<(), String> {
        self.guest = Some(WasmGuest::load(path).map_err(|e| format!("load wasm: {e}"))?);
        self.wasm_path = Some(path.to_string());
        Ok(())
    }

    /// Feed pure input data to the guest on the next tick(s). Ignored when no
    /// guest is loaded (the native integrator is identity).
    pub fn set_input(&mut self, input: InputState3D) {
        if let Some(g) = self.guest.as_mut() {
            g.set_input(input);
        }
    }

    /// Determinism hash of the world (`World::hash()`).
    pub fn hash(&self) -> u64 {
        self.world.hash()
    }

    /// Export the whole world as a portable [`SceneFile`] (all columns).
    pub fn export_scene(&self) -> SceneFile {
        let content = openengine_ecs::scene::content_from_world(&self.world);
        SceneFile {
            version: content.version,
            tick: self.tick,
            entities: content.entities,
        }
    }

    /// Build a world from a [`SceneFile`]. Rejects an incompatible version.
    pub fn scene_to_world(scene: &SceneFile) -> Result<World, String> {
        let content = SceneContent {
            version: scene.version,
            entities: scene.entities.clone(),
        };
        openengine_ecs::scene::world_from_content(&content)
    }

    /// Replace this world+tick from a [`SceneFile`] (used by `/load`).
    pub fn import_scene(&mut self, scene: &SceneFile) -> Result<(), String> {
        let w = Self::scene_to_world(scene)?;
        self.world = w;
        self.tick = scene.tick;
        Ok(())
    }

    /// Snapshot up to `limit` entities.
    pub fn observe(&self, limit: usize) -> (Vec<EntityView>, u64) {
        let n = self.world.entity_count().min(limit.max(1));
        let transforms = self.world.get_transforms().unwrap_or(&[]);
        let colors = self.world.get_colors().unwrap_or(&[]);
        let mut out = Vec::with_capacity(n);
        for i in 0..n {
            let t = transforms.get(i);
            let p = t
                .map(|t| {
                    [
                        t.position[0].to_num(),
                        t.position[1].to_num(),
                        t.position[2].to_num(),
                    ]
                })
                .unwrap_or([0.0; 3]);
            let sc = t
                .map(|t| {
                    [
                        t.scale[0].to_num(),
                        t.scale[1].to_num(),
                        t.scale[2].to_num(),
                    ]
                })
                .unwrap_or([1.0; 3]);
            let c = colors.get(i).copied().unwrap_or(Color {
                r: 255,
                g: 255,
                b: 255,
                a: 255,
            });
            out.push(EntityView {
                index: i as u32,
                transform: p,
                scale: sc,
                color: [c.r, c.g, c.b, c.a],
            });
        }
        (out, self.tick)
    }
}

impl Default for HarnessState {
    fn default() -> Self {
        Self::new()
    }
}
