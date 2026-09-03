//! egui editor shell application state (Domain A, egui 0.32 over wgpu 25).
//!
//! Drives the validated headless editor core (`openengine-editor`): Edit/Play,
//! undo/redo commands, selection/picking, orbit camera. Panels only here — the
//! 3D viewport is rendered by the shell's wgpu pass (in `main.rs`).
//!
//! **Play mode runs the real Domain B logic in the wasm sandbox** via
//! [`WasmGameplayHost`] (`openengine_gameplay_tick`), at a fixed 60 Hz
//! timestep for determinism. If the `logic.wasm` module is unavailable it falls
//! back to a lightweight native placeholder so the shell never crashes.

use std::path::Path;
use std::time::Instant;

use egui::Context;
use openengine_contracts::{
    comp, Actor, ArchetypeId, ColumnWrite, ComponentId, InputState3D, Transform, Velocity3D,
    WorldDelta,
};
use openengine_core::wasm_gameplay_host::WasmGameplayHost;
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
    /// Running deterministic guest tick counter used by the play loop.
    pub frame: u64,
    /// Held movement keys captured each frame from egui (WASD + Space).
    /// Order: up, down, left, right, jump.
    pub keys: [bool; 5],
    /// Vertical velocity of the player used by the NATIVE fallback sim.
    pub player_vy: f32,
    /// Whether a Play session is currently following the player with a camera
    /// already initialized. Reset when leaving Play so re-entry re-frames once.
    pub follow_active: bool,
    /// Lazy wasm gameplay backend (loaded once on the first Play).
    pub backend: PlayBackend,
}

/// Loads + holds the guest gameplay module and paces ticks at a fixed 60 Hz.
pub struct PlayBackend {
    /// Loaded guest host, if the wasm module was available.
    pub host: Option<WasmGameplayHost>,
    /// True when the wasm module is absent/failed → run the native fallback.
    pub fallback: bool,
    /// Have we attempted to load the module yet?
    loaded: bool,
    /// Wall-clock accumulator for the fixed-timestep (60 Hz) scheduler.
    accum: f64,
    /// Last wall-clock sample.
    last: Option<Instant>,
}

impl Default for PlayBackend {
    fn default() -> Self {
        PlayBackend {
            host: None,
            fallback: false,
            loaded: false,
            accum: 0.0,
            last: None,
        }
    }
}

fn x(v: f32) -> I16F16 {
    I16F16::from_num(v)
}

fn tf(pos: [f32; 3]) -> Transform {
    Transform::at(x(pos[0]), x(pos[1]), x(pos[2]))
}

/// Resolve the Domain B logic module. Honors `OPENENGINE_WASM_PATH`; falls back
/// to the dev-layout asset next to `openengine-core`.
fn wasm_asset_path() -> Option<String> {
    if let Ok(p) = std::env::var("OPENENGINE_WASM_PATH") {
        if Path::new(&p).exists() {
            return Some(p);
        }
    }
    let dev = concat!(env!("CARGO_MANIFEST_DIR"), "/../core/assets/logic.wasm");
    if Path::new(dev).exists() {
        return Some(dev.to_string());
    }
    None
}

/// Build the demo scene: a player (entity 0) + 10 NPCs, each carrying the 3D
/// gameplay columns (Transform, Velocity3D, Actor) the guest needs.
fn build_scene() -> World {
    let mut w = World::new();
    let add = |w: &mut World, pos: [f32; 3], color: EcsColor, actor: Actor, vel: Velocity3D| {
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
        w.set_velocity_3d(idx, vel);
        w.set_actor(idx, actor);
    };
    // Player (entity 0) at origin, controllable by the guest from WASD/Space.
    add(
        &mut w,
        [0.0, 0.0, 0.0],
        EcsColor {
            r: 0,
            g: 255,
            b: 0,
            a: 255,
        },
        Actor::player(x(3.0), x(40.0)),
        Velocity3D::zero(),
    );
    // NPCs: alternate wander (1) / circle (2) / chase (3) for behaviour variety.
    for i in 1u32..=10 {
        let pos = [i as f32 * 2.5, 0.0, 0.0];
        let c = EcsColor {
            r: ((i * 40) % 256) as u8,
            g: ((i * 90) % 256) as u8,
            b: ((i * 160) % 256) as u8,
            a: 255,
        };
        let kind = 1 + (i % 3); // 2,3,1 repeating
        let actor = Actor::npc(kind, i * 2654435761);
        add(&mut w, pos, c, actor, Velocity3D::zero());
    }
    w
}

impl EditorApp {
    /// Build the app and a demo scene (player + NPCs with gameplay actors).
    pub fn new() -> Self {
        let mut app = EditorApp {
            state: EditorState::new(build_scene()),
            selection: SelectionModel::default(),
            undo: UndoRedoManager::new(),
            camera: EditorCamera::default(),
            egui_ctx: Context::default(),
            viewport_rect: None,
            frame: 0,
            keys: [false; 5],
            player_vy: 0.0,
            follow_active: false,
            backend: PlayBackend::default(),
        };
        // Default framing so the spawned spheres (x in 0..25) are visible.
        app.camera.focus = glam::Vec3::new(12.5, 0.0, 0.0);
        app.camera.distance = 34.0;
        app.camera.pitch = 0.45;
        app.camera.yaw = 0.6;
        app
    }

    /// Advance the PLAY simulation by wall-clock time at a FIXED 60 Hz guest
    /// tick. On each tick the real wasm logic runs over the play world; the
    /// resulting `WorldDelta` is applied. Falls back to a native placeholder if
    /// the wasm module could not be loaded. Edits are never touched.
    pub fn step_simulation(&mut self) {
        if self.state.mode != EditorMode::Playing {
            // Leaving Play: reset the follow session + the fixed-timestep clock.
            self.follow_active = false;
            self.backend.accum = 0.0;
            self.backend.last = None;
            return;
        }
        // Load the wasm backend lazily on first Play.
        if !self.backend.loaded {
            self.backend.loaded = true;
            match wasm_asset_path().and_then(|p| WasmGameplayHost::load(&p).ok()) {
                Some(host) => self.backend.host = Some(host),
                None => {
                    self.backend.fallback = true;
                    eprintln!("Play: logic.wasm unavailable → native fallback sim");
                }
            }
        }
        // Entering Play: frame the player once; afterwards user keeps orbit/zoom
        // and the camera only re-tracks the player position.
        if !self.follow_active {
            self.camera.distance = 12.0;
            self.camera.pitch = 0.35;
            self.camera.yaw = 0.6;
            self.follow_active = true;
        }

        // Fixed 60 Hz timestep from wall clock (Domain A pacing only).
        let now = Instant::now();
        let step_s = 1.0 / 60.0;
        if let Some(last) = self.backend.last {
            self.backend.accum += (now - last).as_secs_f64();
        }
        self.backend.last = Some(now);
        let max_ticks = 8; // catch-up cap avoids a death spiral on stalls.
        let mut ticks = 0;
        while self.backend.accum >= step_s && ticks < max_ticks {
            self.run_one_tick();
            self.backend.accum -= step_s;
            ticks += 1;
        }
        self.track_player();
    }

    /// Run exactly one gameplay tick against the active (play) world.
    fn run_one_tick(&mut self) {
        let Some(world) = self.state.mutable_world() else {
            return;
        };
        let input = InputState3D {
            forward: self.keys[0] as u8,
            backward: self.keys[1] as u8,
            left: self.keys[2] as u8,
            right: self.keys[3] as u8,
            jump: self.keys[4] as u8,
            ..InputState3D::none()
        };
        if let Some(host) = self.backend.host.as_mut() {
            host.set_input(input);
            if let Ok(delta) = host.tick(world, self.frame) {
                world.apply_delta(&delta);
            } else {
                // A failed tick is non-fatal; keep the world as-is this tick.
                eprintln!("Play: guest gameplay tick failed");
            }
        } else {
            native_fallback_tick(world, &self.keys, &mut self.player_vy);
        }
        self.frame += 1;
    }

    /// Recenter the follow camera on the player's current transform.
    fn track_player(&mut self) {
        let world = self.state.active_world();
        let Some(p) = world.get_transforms().and_then(|t| t.first()).map(|t| {
            [
                t.position[0].to_num::<f32>(),
                t.position[1].to_num::<f32>(),
                t.position[2].to_num::<f32>(),
            ]
        }) else {
            return;
        };
        self.camera.focus = glam::Vec3::new(p[0], p[1] + 1.2, p[2]);
    }

    /// Orbit/pan/zoom the camera from viewport mouse events (egui).
    pub fn handle_nav(&mut self, ctx: &egui::Context) {
        let Some(rect) = self.viewport_rect else {
            return;
        };
        ctx.input(|i| {
            let over = i.pointer.hover_pos().is_some_and(|p| rect.contains(p));
            if !over {
                return;
            }
            let scroll = i.raw_scroll_delta.y;
            if scroll != 0.0 {
                self.camera.zoom(scroll * self.camera.distance * 0.001);
            }
            let dragging =
                (i.modifiers.alt && i.pointer.primary_down()) || i.pointer.secondary_down();
            if dragging {
                let d = i.pointer.delta();
                self.camera.orbit(-d.x * 0.005, -d.y * 0.005);
            }
            if i.pointer.middle_down() {
                let d = i.pointer.delta();
                let fwd = (self.camera.focus - self.camera.eye()).normalize();
                let right = fwd.cross(glam::Vec3::Y).normalize();
                let up = right.cross(fwd).normalize();
                self.camera.pan(
                    right * (-d.x * self.camera.distance),
                    up * (d.y * self.camera.distance),
                );
            }
        });
        // F = frame scene (Edit mode convenience).
        if ctx.input(|i| i.key_pressed(egui::Key::F)) {
            self.camera.focus = glam::Vec3::new(12.5, 0.5, 0.0);
            self.camera.distance = 34.0;
        }
    }

    /// Render the toolbar + hierarchy + inspector + reserve the central viewport.
    pub fn ui(&mut self, ctx: &egui::Context) {
        // Capture keyboard (WASD + Space) into a compact array.
        let (up, down, left, right, jump) = ctx.input(|i| {
            (
                i.key_down(egui::Key::W) || i.key_down(egui::Key::ArrowUp),
                i.key_down(egui::Key::S) || i.key_down(egui::Key::ArrowDown),
                i.key_down(egui::Key::A) || i.key_down(egui::Key::ArrowLeft),
                i.key_down(egui::Key::D) || i.key_down(egui::Key::ArrowRight),
                i.key_down(egui::Key::Space),
            )
        });
        self.keys = [up, down, left, right, jump];
        self.handle_nav(ctx);
        self.toolbar(ctx);
        self.hierarchy(ctx);
        self.inspector(ctx);
        egui::CentralPanel::default()
            .frame(egui::Frame::NONE)
            .show(ctx, |ui| {
                let rect = ui.available_rect_before_wrap();
                self.viewport_rect = Some(rect);
                ui.label(egui::RichText::new("3D viewport (lit scene)").weak());
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
                // Engine indicator: whether Play runs the guest wasm or fallback.
                if self.state.mode == EditorMode::Playing {
                    let (text, color) = if self.backend.host.is_some() {
                        ("engine: wasm", egui::Color32::from_rgb(120, 220, 120))
                    } else {
                        (
                            "engine: native fallback",
                            egui::Color32::from_rgb(230, 200, 90),
                        )
                    };
                    ui.label(egui::RichText::new(text).color(color));
                    ui.separator();
                }
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

/// Lightweight native placeholder used only when the wasm module is absent:
/// WASD + a naive jump move the player; NPCs drift deterministically. This is
/// NOT the certified logic — it exists purely so the shell never crashes.
fn native_fallback_tick(world: &mut World, keys: &[bool; 5], player_vy: &mut f32) {
    let n = world.entity_count();
    if n == 0 {
        return;
    }
    let mut t = world
        .get_transforms()
        .map(|s| s[..n].to_vec())
        .unwrap_or_default();
    let (mut px, mut pz) = {
        let p = t[0].position;
        (p[0].to_num::<f32>(), p[2].to_num::<f32>())
    };
    let mut py = t[0].position[1].to_num::<f32>();
    let speed = 0.3;
    // W = forward (-Z), S = back (+Z), A = left (-X), D = right (+X).
    if keys[0] {
        pz -= speed;
    }
    if keys[1] {
        pz += speed;
    }
    if keys[2] {
        px -= speed;
    }
    if keys[3] {
        px += speed;
    }
    if keys[4] && *player_vy == 0.0 {
        *player_vy = 0.6;
    }
    *player_vy -= 0.02;
    py += *player_vy;
    if py <= 0.0 && *player_vy < 0.0 {
        py = 0.0;
        *player_vy = 0.0;
    }
    t[0] = Transform::at(x(px), x(py), x(pz));
    // NPCs drift slightly (visual placeholder only).
    for slot in t.iter_mut().skip(1) {
        slot.position[0] = x(slot.position[0].to_num::<f32>() + 0.01); // small +X drift
    }
    let mut delta = WorldDelta::default();
    for (i, tr) in t.iter().enumerate() {
        delta.writes.push(ColumnWrite {
            archetype: ArchetypeId(0),
            component: ComponentId(comp::TRANSFORM),
            indices: vec![i as u32],
            payload: bytemuck::bytes_of(tr).to_vec(),
        });
    }
    world.apply_delta(&delta);
}

impl Default for EditorApp {
    fn default() -> Self {
        Self::new()
    }
}
