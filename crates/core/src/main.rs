//! # Domain A — "The Living Window" (`openengine-core` binary)
//!
//! The minimal vertical slice that proves the Triforce architecture end to end:
//!
//! ```text
//!   winit frame
//!      │  tick ─────────────────────────────┐
//!      ▼                                   ▼
//!   Domain B wasm (logic-sandbox, no_std)  ◄── wasmtime hosts & drives it
//!   tick_color computes a colour in fixed-point
//!      │  returns WorldDelta{ ClearColor }
//!      ▼  (postcard over guest linear memory)
//!   host decodes WorldDelta, reads ClearColor
//!      ▼
//!   wgpu (Vulkan) clears the surface & presents
//! ```
//!
//! Domain A owns the event loop, GPU, and sandbox. Domain B only ever sees a
//! `StateView` and answers with a `WorldDelta`. Nothing else crosses.

use std::sync::Arc;

use anyhow::Context;
use winit::application::ApplicationHandler;
use winit::dpi::LogicalSize;
use winit::event::WindowEvent;
use winit::event_loop::{ActiveEventLoop, ControlFlow, EventLoop};
use winit::window::{Window, WindowId};

/// Path of the compiled Domain-B logic module, produced by `scripts/build.sh`.
/// Relative to the `core` crate so `cargo run` works from any cwd.
const WASM_ASSET: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/assets/logic.wasm");

/// How many guest linear-memory bytes to reserve for the encoded `WorldDelta`.
const GUEST_BUFFER_CAP: u32 = 4096;

// ────────────────────────────────────────────────────────────────────────────
// wgpu (Vulkan) state
// ────────────────────────────────────────────────────────────────────────────

struct Gpu {
    /// Kept alive: the surface borrows the instance on the host side.
    _instance: wgpu::Instance,
    surface: wgpu::Surface<'static>,
    device: wgpu::Device,
    queue: wgpu::Queue,
    config: wgpu::SurfaceConfiguration,
}

impl Gpu {
    /// Create a Vulkan-only instance, surface, adapter, device and queue.
    async fn new(window: Arc<Window>) -> anyhow::Result<Self> {
        // Workspace rule: prefer (here: require) the Vulkan backend.
        let instance = wgpu::Instance::new(&wgpu::InstanceDescriptor {
            backends: wgpu::Backends::VULKAN,
            ..Default::default()
        });
        let surface = instance
            .create_surface(window.clone())
            .context("create wgpu surface from window")?;

        let adapter = instance
            .request_adapter(&wgpu::RequestAdapterOptions {
                power_preference: wgpu::PowerPreference::HighPerformance,
                compatible_surface: Some(&surface),
                force_fallback_adapter: false,
            })
            .await
            .context("request Vulkan adapter")?;

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
        let format = caps
            .formats
            .first()
            .copied()
            .context("surface has no formats")?;
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

    /// Clear the swapchain target with `rgba` and present it.
    fn clear_and_present(&mut self, rgba: [f32; 4]) -> anyhow::Result<()> {
        let frame = self.surface.get_current_texture()?;
        let view = frame
            .texture
            .create_view(&wgpu::TextureViewDescriptor::default());
        let mut encoder = self
            .device
            .create_command_encoder(&wgpu::CommandEncoderDescriptor {
                label: Some("openengine.frame"),
            });

        let clear = wgpu::Color {
            r: rgba[0] as f64,
            g: rgba[1] as f64,
            b: rgba[2] as f64,
            a: rgba[3] as f64,
        };
        {
            let _pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("openengine.clear"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: &view,
                    resolve_target: None,
                    ops: wgpu::Operations {
                        load: wgpu::LoadOp::Clear(clear),
                        store: wgpu::StoreOp::Store,
                    },
                })],
                depth_stencil_attachment: None,
                timestamp_writes: None,
                occlusion_query_set: None,
            });
        }
        self.queue.submit(std::iter::once(encoder.finish()));
        frame.present();
        Ok(())
    }
}

// ────────────────────────────────────────────────────────────────────────────
// wasmtime sandbox state
// ────────────────────────────────────────────────────────────────────────────

struct Logic {
    /// Engine must outlive the store; cloned handle keeps it alive.
    _engine: wasmtime::Engine,
    store: wasmtime::Store<()>,
    tick: wasmtime::TypedFunc<(u64, u32, u32), u32>,
    memory: wasmtime::Memory,
    /// Long-lived guest scratch buffer (from `openengine_alloc`).
    buf: u32,
    /// Monotonic Domain-A frame counter handed to Domain B.
    frame: u64,
}

impl Logic {
    fn load() -> anyhow::Result<Self> {
        let bytes = std::fs::read(WASM_ASSET).with_context(|| {
            format!(
                "missing {WASM_ASSET} — run `bash scripts/build.sh` first to \
                 compile the Domain-B wasm module"
            )
        })?;

        let engine = wasmtime::Engine::default();
        let module = wasmtime::Module::new(&engine, bytes).context("compile wasm module")?;
        let mut store = wasmtime::Store::new(&engine, ());

        let linker = wasmtime::Linker::new(&engine);
        let instance = linker
            .instantiate(&mut store, &module)
            .context("instantiate logic module")?;

        let alloc = instance
            .get_typed_func::<u32, u32>(&mut store, "openengine_alloc")
            .context("missing guest export openengine_alloc")?;
        let tick = instance
            .get_typed_func::<(u64, u32, u32), u32>(&mut store, "openengine_tick")
            .context("missing guest export openengine_tick")?;
        let memory = instance
            .get_memory(&mut store, "memory")
            .context("guest must export linear memory as 'memory'")?;

        // Reserve the scratch buffer ONCE; it is reused every frame.
        let buf = alloc.call(&mut store, GUEST_BUFFER_CAP)?;

        Ok(Logic {
            _engine: engine,
            store,
            tick,
            memory,
            buf,
            frame: 0,
        })
    }

    /// Run one tick of Domain B and return the requested clear colour.
    fn clear_color(&mut self) -> anyhow::Result<[f32; 4]> {
        self.frame += 1;
        let n = self
            .tick
            .call(&mut self.store, (self.frame, self.buf, GUEST_BUFFER_CAP))?;
        if n == 0 || n as usize > GUEST_BUFFER_CAP as usize {
            anyhow::bail!("guest tick returned an invalid delta length ({n})");
        }
        let mut out = vec![0u8; n as usize];
        self.memory
            .read(&self.store, self.buf as usize, &mut out)
            .context("read WorldDelta from guest memory")?;
        let delta = openengine_contracts::decode_delta(&out).context("decode WorldDelta")?;
        Ok(delta.clear_color().unwrap_or([0.05, 0.05, 0.08, 1.0]))
    }
}

// ────────────────────────────────────────────────────────────────────────────
// winit application
// ────────────────────────────────────────────────────────────────────────────

struct App {
    /// None until the OS gives us a surface in `resumed`.
    window: Option<Arc<Window>>,
    gpu: Option<Gpu>,
    logic: Option<Logic>,
}

impl App {
    fn new() -> Self {
        App {
            window: None,
            gpu: None,
            logic: None,
        }
    }
}

impl ApplicationHandler for App {
    fn resumed(&mut self, event_loop: &ActiveEventLoop) {
        // (Re)create everything when the platform gives us a surface.
        if self.window.is_some() {
            return;
        }
        let window = Arc::new(
            event_loop
                .create_window(
                    Window::default_attributes()
                        .with_title("OpenEngine — Living Window")
                        .with_inner_size(LogicalSize::new(900.0, 600.0)),
                )
                .expect("create window"),
        );

        let gpu = pollster::block_on(Gpu::new(window.clone())).expect("init wgpu (Vulkan)");
        let logic = Logic::load().expect("init wasmtime logic sandbox");

        self.window = Some(window);
        self.gpu = Some(gpu);
        self.logic = Some(logic);
    }

    fn window_event(
        &mut self,
        event_loop: &ActiveEventLoop,
        _window_id: WindowId,
        event: WindowEvent,
    ) {
        match event {
            WindowEvent::CloseRequested => event_loop.exit(),
            WindowEvent::Resized(size) => {
                if let Some(gpu) = self.gpu.as_mut() {
                    gpu.resize(size.width, size.height);
                }
            }
            // Continuous animation is driven from `about_to_wait` (ControlFlow::Poll).
            _ => {}
        }
    }

    fn about_to_wait(&mut self, _event_loop: &ActiveEventLoop) {
        let Some(gpu) = self.gpu.as_mut() else { return };
        let Some(logic) = self.logic.as_mut() else {
            return;
        };

        // 1..4. Run Domain B and read the colour out of its WorldDelta.
        let color = match logic.clear_color() {
            Ok(c) => c,
            Err(e) => {
                eprintln!("logic tick error: {e:#}");
                [0.2, 0.0, 0.0, 1.0] // red-ish: surface the failure visibly
            }
        };

        // 6-7. Clear with the guest-computed colour and present.
        if let Err(e) = gpu.clear_and_present(color) {
            // Surface loss is normal on resize/occlusion; recover next frame.
            eprintln!("render error: {e:#}");
        }
    }
}

fn main() -> anyhow::Result<()> {
    let event_loop = EventLoop::new().context("create winit event loop")?;
    // Continuous animation: Poll + render in about_to_wait.
    event_loop.set_control_flow(ControlFlow::Poll);

    let mut app = App::new();
    event_loop.run_app(&mut app).map_err(anyhow::Error::from)?;
    Ok(())
}
