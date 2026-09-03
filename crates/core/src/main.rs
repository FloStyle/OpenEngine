//! # Domain A — "First Playable Prototype" (`openengine-core` binary)
//!
//! A windowed PoC: 100 colored squares (one player, green), movement computed
//! inside the wasm guest through the SoA bridge (Phase 3), input (Domain A)
//! sets the player's velocity, the guest integrates + bounces. Rendered with
//! wgpu (Vulkan) as colored quads.
//!
//! Note: windowing needs a display + Vulkan, so this binary is run on your
//! machine (`cargo run -p openengine-core` after `bash scripts/build.sh`).

mod input;
mod renderer;

use std::sync::Arc;

use anyhow::Context;
use input::{InputState, PlayerInput};
use openengine_contracts::{comp, ArchetypeId, ColumnWrite, ComponentId, WorldDelta};
use openengine_core::wasm_move_host::WasmMoveHost;
use openengine_ecs::{Color, Position, Velocity, World};
use openengine_math::I16F16;
use renderer::QuadRenderer;
use winit::application::ApplicationHandler;
use winit::dpi::LogicalSize;
use winit::event::WindowEvent;
use winit::event_loop::{ActiveEventLoop, ControlFlow, EventLoop};
use winit::window::{Window, WindowId};

/// Compiled Domain-B module (scripts/build.sh).
const WASM_ASSET: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/assets/logic.wasm");

/// Logical playfield 0..FIELD pixels on both axes (matches guest wall bounds).
const FIELD: i32 = 500;
const QUAD: i32 = 10;
const ENTITY_COUNT: usize = 100;
const PLAYER_SPEED: i32 = 6;

// ────────────────────────────────────────────────────────────────────────────
// wgpu (Vulkan) state + frame presentation
// ────────────────────────────────────────────────────────────────────────────

struct Gpu {
    _instance: wgpu::Instance,
    surface: wgpu::Surface<'static>,
    device: wgpu::Device,
    queue: wgpu::Queue,
    config: wgpu::SurfaceConfiguration,
}

impl Gpu {
    async fn new(window: Arc<Window>) -> anyhow::Result<Self> {
        let instance = wgpu::Instance::new(&wgpu::InstanceDescriptor {
            backends: wgpu::Backends::VULKAN,
            ..Default::default()
        });
        let surface = instance
            .create_surface(window.clone())
            .context("create wgpu surface")?;
        let adapter = instance
            .request_adapter(&wgpu::RequestAdapterOptions {
                power_preference: wgpu::PowerPreference::HighPerformance,
                compatible_surface: Some(&surface),
                force_fallback_adapter: false,
            })
            .await
            .context("request adapter")?;
        let (device, queue) = adapter
            .request_device(&wgpu::DeviceDescriptor {
                label: Some("openengine.device"),
                required_features: wgpu::Features::empty(),
                required_limits: wgpu::Limits::default(),
                memory_hints: wgpu::MemoryHints::default(),
                trace: wgpu::Trace::Off,
            })
            .await
            .context("request device")?;
        let size = window.inner_size();
        let caps = surface.get_capabilities(&adapter);
        let format = caps.formats.first().copied().context("no surface format")?;
        let config = wgpu::SurfaceConfiguration {
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT,
            format,
            width: size.width.max(1),
            height: size.height.max(1),
            present_mode: wgpu::PresentMode::Fifo,
            alpha_mode: caps.alpha_modes[0],
            view_formats: vec![],
            desired_maximum_frame_latency: 2,
        };
        surface.configure(&device, &config);
        Ok(Gpu {
            _instance: instance,
            surface,
            device,
            queue,
            config,
        })
    }

    fn resize(&mut self, width: u32, height: u32) {
        if width == 0 || height == 0 {
            return;
        }
        self.config.width = width;
        self.config.height = height;
        self.surface.configure(&self.device, &self.config);
    }

    /// Render quads from `world` into the swapchain and present.
    fn present_quads(
        &mut self,
        renderer: &mut QuadRenderer,
        world: &World,
        size: (u32, u32),
    ) -> anyhow::Result<()> {
        let frame = self.surface.get_current_texture()?;
        let view = frame
            .texture
            .create_view(&wgpu::TextureViewDescriptor::default());
        renderer.render(&self.device, &self.queue, &view, world, size);
        frame.present();
        Ok(())
    }
}

// ────────────────────────────────────────────────────────────────────────────
// winit application
// ────────────────────────────────────────────────────────────────────────────

struct App {
    window: Option<Arc<Window>>,
    gpu: Option<Gpu>,
    renderer: Option<QuadRenderer>,
    world: World,
    wasm_host: Option<WasmMoveHost>,
    input: InputState,
    frame: u64,
}

impl App {
    fn new() -> Self {
        App {
            window: None,
            gpu: None,
            renderer: None,
            world: build_world(),
            wasm_host: None,
            input: InputState::new(),
            frame: 0,
        }
    }

    /// Apply player input as a velocity write to entity 0 (host-authored),
    /// then let the wasm guest integrate + bounce (gameplay stays in Domain B).
    fn step(&mut self) {
        let player = self.input.get_player_input();
        self.apply_player_velocity(&player);

        let Some(host) = self.wasm_host.as_mut() else {
            return;
        };
        match host.tick(&self.world) {
            Ok(delta) => self.world.apply_delta(&delta),
            Err(e) => eprintln!("wasm movement error: {e:#}"),
        }
    }

    fn apply_player_velocity(&mut self, input: &PlayerInput) {
        let (mut vx, mut vy) = (0, 0);
        if input.left {
            vx = -PLAYER_SPEED;
        }
        if input.right {
            vx = PLAYER_SPEED;
        }
        if input.up {
            vy = -PLAYER_SPEED;
        }
        if input.down {
            vy = PLAYER_SPEED;
        }
        let vel = Velocity {
            x: I16F16::from_num(vx),
            y: I16F16::from_num(vy),
        };
        let mut delta = WorldDelta::default();
        delta.writes.push(ColumnWrite {
            archetype: ArchetypeId(0),
            component: ComponentId(comp::VELOCITY),
            indices: vec![0],
            payload: bytemuck::bytes_of(&vel).to_vec(),
        });
        self.world.apply_delta(&delta);
    }
}

/// Spawn the player (entity 0, green) + 99 NPCs (deterministic colours/velocities).
fn build_world() -> World {
    let mut world = World::new();
    let spacing = FIELD / 10;
    for i in 0..ENTITY_COUNT {
        let (x, y) = if i == 0 {
            (FIELD / 2, FIELD / 2)
        } else {
            let gx = (i as i32) % 10;
            let gy = (i as i32) / 10;
            (gx * spacing + spacing / 2, gy * spacing + spacing / 2)
        };
        let color = if i == 0 {
            Color {
                r: 0,
                g: 255,
                b: 0,
                a: 255,
            }
        } else {
            Color {
                r: ((i * 37) % 256) as u8,
                g: ((i * 73) % 256) as u8,
                b: ((i * 109) % 256) as u8,
                a: 255,
            }
        };
        // Player starts still; NPCs drift deterministically (nonzero velocity).
        let vel = if i == 0 {
            Velocity {
                x: I16F16::from_num(0),
                y: I16F16::from_num(0),
            }
        } else {
            Velocity {
                x: I16F16::from_num(((((i * 7) % 9) as i32) - 4) * 2),
                y: I16F16::from_num(((((i * 13) % 9) as i32) - 4) * 2),
            }
        };
        world.spawn(
            Position {
                x: I16F16::from_num(x),
                y: I16F16::from_num(y),
            },
            vel,
            color,
        );
    }
    // QUAD is exposed for sizing; entity positions already leave room.
    let _ = QUAD;
    world
}

impl ApplicationHandler for App {
    fn resumed(&mut self, event_loop: &ActiveEventLoop) {
        if self.window.is_some() {
            return;
        }
        let window = Arc::new(
            event_loop
                .create_window(
                    Window::default_attributes()
                        .with_title("OpenEngine — First Playable Prototype")
                        .with_inner_size(LogicalSize::new(520.0, 520.0)),
                )
                .expect("create window"),
        );
        let gpu = pollster::block_on(Gpu::new(window.clone())).expect("init wgpu (Vulkan)");
        let renderer = QuadRenderer::new(&gpu.device, gpu.config.format);
        let wasm_host = WasmMoveHost::load(WASM_ASSET).expect("init wasm movement host");

        self.window = Some(window);
        self.gpu = Some(gpu);
        self.renderer = Some(renderer);
        self.wasm_host = Some(wasm_host);
    }

    fn window_event(&mut self, event_loop: &ActiveEventLoop, _id: WindowId, event: WindowEvent) {
        match event {
            WindowEvent::CloseRequested => event_loop.exit(),
            WindowEvent::Resized(size) => {
                if let Some(gpu) = self.gpu.as_mut() {
                    gpu.resize(size.width, size.height);
                }
            }
            WindowEvent::KeyboardInput { event, .. } => self.input.handle_key_event(&event),
            _ => {}
        }
    }

    fn about_to_wait(&mut self, _event_loop: &ActiveEventLoop) {
        self.step();

        if let (Some(gpu), Some(renderer), Some(window)) =
            (&mut self.gpu, &mut self.renderer, &self.window)
        {
            let size = window.inner_size();
            if let Err(e) = gpu.present_quads(renderer, &self.world, (size.width, size.height)) {
                eprintln!("render error: {e:#}");
            }
        }
        self.input.clear_frame_state();
        self.frame += 1;
    }
}

fn main() -> anyhow::Result<()> {
    let event_loop = EventLoop::new().context("create event loop")?;
    event_loop.set_control_flow(ControlFlow::Poll);
    let mut app = App::new();
    event_loop.run_app(&mut app).map_err(anyhow::Error::from)?;
    Ok(())
}
