//! egui editor shell application state (Domain A, egui 0.32 over wgpu 25).
//!
//! Drives the validated headless editor core (`openengine-editor`): Edit/Play,
//! undo/redo commands, selection/picking, orbit camera. Panels only here — the
//! 3D viewport is rendered by the shell's wgpu pass (in `main.rs`).

use egui::Context;
use openengine_contracts::{comp, ArchetypeId, ColumnWrite, ComponentId, Transform, WorldDelta};
use openengine_ecs::{Color as EcsColor, Position, Velocity, World};
use openengine_editor::camera::EditorCamera;
use openengine_editor::commands::{ModifyTransformCommand, UndoRedoManager};
use openengine_editor::selection::{pick, SelectionModel};
use openengine_editor::state::{EditorMode, EditorState};
use openengine_math::I16F16;

/// The interactive editor state + egui handles + the 3D camera.
pub struct EditorApp {
    pub state: EditorState,
    pub selection: SelectionModel,
    pub undo: UndoRedoManager,
    pub camera: EditorCamera,
    pub egui_ctx: Context,
    /// Rect of the last 3D viewport (central panel), set each frame by `ui`.
    pub viewport_rect: Option<egui::Rect>,
    /// Running frame counter used by the play sim.
    pub frame: u64,
}

fn x(v: f32) -> I16F16 {
    I16F16::from_num(v)
}

fn tf(pos: [f32; 3]) -> Transform {
    Transform::at(x(pos[0]), x(pos[1]), x(pos[2]))
}

fn build_scene() -> World {
    let mut w = World::new();
    let add = |w: &mut World, pos: [f32; 3], color: EcsColor| {
        let idx = w.spawn(
            Position {
                x: x(pos[0]),
                y: x(pos[2]),
            },
            Velocity {
                x: x(0.0),
                y: x(0.0),
            },
            color,
        );
        w.set_transform(idx, tf(pos));
    };
    // Player (entity 0) at origin.
    add(
        &mut w,
        [0.0, 0.5, 0.0],
        EcsColor {
            r: 0,
            g: 255,
            b: 0,
            a: 255,
        },
    );
    // 10 "NPC" cubes along +X.
    for i in 1..=10 {
        let pos = [i as f32 * 2.5, 0.5, 0.0];
        let c = EcsColor {
            r: ((i * 40) % 256) as u8,
            g: ((i * 90) % 256) as u8,
            b: ((i * 160) % 256) as u8,
            a: 255,
        };
        add(&mut w, pos, c);
    }
    w
}

impl EditorApp {
    /// Build the app and an empty-ish demo scene.
    pub fn new() -> Self {
        let mut app = EditorApp {
            state: EditorState::new(build_scene()),
            selection: SelectionModel::default(),
            undo: UndoRedoManager::new(),
            camera: EditorCamera::default(),
            egui_ctx: Context::default(),
            viewport_rect: None,
            frame: 0,
        };
        // Default framing so the spawned cubes (x in 0..25) are visible.
        app.camera.focus = glam::Vec3::new(12.5, 0.5, 0.0);
        app.camera.distance = 34.0;
        app.camera.pitch = 0.45;
        app.camera.yaw = 0.6;
        app
    }

    /// Advance the PLAY simulation one frame (called each frame in Playing mode).
    /// Deterministic orbit so cubes visibly move; edits are never touched.
    pub fn step_simulation(&mut self) {
        if self.state.mode != EditorMode::Playing {
            return;
        }
        let Some(world) = self.state.mutable_world() else {
            return;
        };
        let n = world.entity_count();
        let frame = self.frame as f32;
        let mut delta = WorldDelta::default();
        for i in 0..n {
            let ang = frame * 0.02 + (i as f32) * 0.5;
            let x = 12.5 + 13.0 * ang.cos();
            let y = 0.5 + 0.5 * (frame * 0.05).sin();
            let z = 6.0 * ang.sin();
            let t = Transform::at(
                I16F16::from_num(x),
                I16F16::from_num(y),
                I16F16::from_num(z),
            );
            delta.writes.push(ColumnWrite {
                archetype: ArchetypeId(0),
                component: ComponentId(comp::TRANSFORM),
                indices: vec![i as u32],
                payload: bytemuck::bytes_of(&t).to_vec(),
            });
        }
        world.apply_delta(&delta);
        self.frame += 1;
    }

    /// Render the toolbar + hierarchy + inspector + reserve the central viewport.
    pub fn ui(&mut self, ctx: &egui::Context) {
        self.toolbar(ctx);
        self.hierarchy(ctx);
        self.inspector(ctx);
        egui::CentralPanel::default()
            .frame(egui::Frame::NONE)
            .show(ctx, |ui| {
                let rect = ui.available_rect_before_wrap();
                self.viewport_rect = Some(rect);
                ui.label(egui::RichText::new("3D viewport (cubes)").weak());
            });
    }

    fn toolbar(&mut self, ctx: &egui::Context) {
        egui::TopBottomPanel::top("toolbar").show(ctx, |ui| {
            ui.horizontal(|ui| {
                ui.label(format!("Mode: {:?}", self.state.mode));
                if self.state.mode == EditorMode::Edit {
                    if ui.button("▶ Play").clicked() {
                        self.state.play();
                    }
                } else if ui.button("⏹ Stop").clicked() {
                    self.state.stop();
                }
                ui.separator();
                let can_undo = self.undo.can_undo();
                if ui
                    .add_enabled(can_undo, egui::Button::new("↶ Undo"))
                    .clicked()
                {
                    self.undo.undo(&mut self.state.edit_world);
                }
                let can_redo = self.undo.can_redo();
                if ui
                    .add_enabled(can_redo, egui::Button::new("↷ Redo"))
                    .clicked()
                {
                    self.undo.redo(&mut self.state.edit_world);
                }
                ui.separator();
                let can_edit = self.state.can_edit();
                ui.label(if can_edit {
                    "Editing: Edit world"
                } else {
                    "Playing: edits locked"
                });
            });
        });
    }

    fn hierarchy(&mut self, ctx: &egui::Context) {
        let entity_count = self.state.active_world().entity_count();
        egui::SidePanel::left("hierarchy")
            .default_width(180.0)
            .show(ctx, |ui| {
                ui.heading("Hierarchy");
                ui.separator();
                for i in 0..entity_count {
                    let id = i as u32;
                    let name = if i == 0 { "Player" } else { "NPC" };
                    let selected = self.selection.selected.contains(&id);
                    if ui
                        .selectable_label(selected, format!("{name} (entity {id})"))
                        .clicked()
                    {
                        let ctrl = ui.input(|inp| inp.modifiers.ctrl);
                        if ctrl {
                            if let Some(p) = self.selection.selected.iter().position(|&s| s == id) {
                                self.selection.selected.remove(p);
                            } else {
                                self.selection.selected.push(id);
                            }
                        } else {
                            self.selection.selected = vec![id];
                        }
                    }
                }
            });
    }

    fn inspector(&mut self, ctx: &egui::Context) {
        egui::SidePanel::right("inspector")
            .default_width(280.0)
            .show(ctx, |ui| {
                ui.heading("Inspector");
                ui.separator();
                let Some(&id) = self.selection.selected.first() else {
                    ui.label("(no entity selected)");
                    return;
                };
                if !self.state.can_edit() {
                    ui.label("Editing is locked while playing.");
                    return;
                }
                let world = &self.state.edit_world;
                let Some(transforms) = world.get_transforms() else {
                    ui.label("no transforms");
                    return;
                };
                let i = id as usize;
                if i >= transforms.len() {
                    ui.label("selection out of range");
                    return;
                }
                let old = transforms[i];
                let mut t = old;
                let mut changed = false;

                ui.label("Transform — Position");
                ui.horizontal(|ui| {
                    for (label, axis) in [("X", 0usize), ("Y", 1usize), ("Z", 2usize)] {
                        let mut v = t.position[axis].to_num::<f32>();
                        if ui
                            .add(
                                egui::DragValue::new(&mut v)
                                    .speed(0.05)
                                    .prefix(format!("{label} ")),
                            )
                            .changed()
                        {
                            t.position[axis] = x(v);
                            changed = true;
                        }
                    }
                });
                ui.label("Scale");
                ui.horizontal(|ui| {
                    for (label, axis) in [("X", 0usize), ("Y", 1usize), ("Z", 2usize)] {
                        let mut v = t.scale[axis].to_num::<f32>();
                        if ui
                            .add(
                                egui::DragValue::new(&mut v)
                                    .speed(0.01)
                                    .prefix(format!("{label} ")),
                            )
                            .changed()
                        {
                            t.scale[axis] = x(v);
                            changed = true;
                        }
                    }
                });

                if changed {
                    let cmd = Box::new(ModifyTransformCommand {
                        entity_index: id,
                        old_value: old,
                        new_value: t,
                    });
                    self.undo.execute(&mut self.state.edit_world, cmd);
                }
            });
    }

    /// Pick an entity under the mouse within the viewport rect.
    pub fn handle_viewport_click(&mut self, mouse: egui::Pos2) {
        let Some(rect) = self.viewport_rect else {
            return;
        };
        if !rect.contains(mouse) {
            return;
        }
        let aspect = (rect.width() / rect.height()).max(0.01);
        let ndc_x = ((mouse.x - rect.min.x) / rect.width()) * 2.0 - 1.0;
        let ndc_y = 1.0 - ((mouse.y - rect.min.y) / rect.height()) * 2.0;
        let (o, d) = self.camera.unproject_ray(ndc_x, ndc_y, aspect);
        let world = self.state.active_world();
        if let Some(id) = pick(o, d, world) {
            self.selection.selected = vec![id];
        } else {
            self.selection.selected.clear();
        }
    }
}

impl Default for EditorApp {
    fn default() -> Self {
        Self::new()
    }
}
