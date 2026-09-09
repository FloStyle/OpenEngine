//! OpenEngine Editor Shell binary (Domain A) — winit 0.30 + wgpu 25 + egui 0.32.

use std::sync::atomic::AtomicU32;
use std::sync::Arc;

use anyhow::Context;
use egui::ViewportId;
use openengine_editor_shell::app::EditorApp;
use openengine_editor_shell::renderer::SceneRenderer;
use openengine_plugin_host::plugins::move_tool::MoveToolPlugin;
use openengine_plugin_host::{FrameCtx, PluginHost};
use winit::application::ApplicationHandler;
use winit::dpi::LogicalSize;
use winit::event::WindowEvent;
use winit::event_loop::{ActiveEventLoop, ControlFlow, EventLoop};
use winit::window::{Window, WindowId};

const DEPTH_FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Depth32Float;

fn make_depth(device: &wgpu::Device, w: u32, h: u32) -> (wgpu::Texture, wgpu::TextureView) {
    let tex = device.create_texture(&wgpu::TextureDescriptor {
        label: Some("scene.depth"),
        size: wgpu::Extent3d {
            width: w.max(1),
            height: h.max(1),
            depth_or_array_layers: 1,
        },
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format: DEPTH_FORMAT,
        usage: wgpu::TextureUsages::RENDER_ATTACHMENT,
        view_formats: &[],
    });
    let view = tex.create_view(&wgpu::TextureViewDescriptor::default());
    (tex, view)
}

struct Gpu {
    _instance: wgpu::Instance,
    surface: wgpu::Surface<'static>,
    device: wgpu::Device,
    queue: wgpu::Queue,
    config: wgpu::SurfaceConfiguration,
    depth: wgpu::Texture,
    depth_view: wgpu::TextureView,
}

impl Gpu {
    async fn new(window: Arc<Window>) -> anyhow::Result<Self> {
        let instance = wgpu::Instance::new(&wgpu::InstanceDescriptor {
            backends: wgpu::Backends::VULKAN,
            ..Default::default()
        });
        let surface = instance.create_surface(window.clone()).context("surface")?;
        let adapter = instance
            .request_adapter(&wgpu::RequestAdapterOptions {
                power_preference: wgpu::PowerPreference::HighPerformance,
                compatible_surface: Some(&surface),
                force_fallback_adapter: false,
            })
            .await
            .context("adapter")?;
        let (device, queue) = adapter
            .request_device(&wgpu::DeviceDescriptor {
                label: Some("editorshell.device"),
                required_features: wgpu::Features::empty(),
                required_limits: wgpu::Limits::default(),
                memory_hints: wgpu::MemoryHints::default(),
                trace: wgpu::Trace::Off,
            })
            .await
            .context("device")?;
        let size = window.inner_size();
        let caps = surface.get_capabilities(&adapter);
        let format = caps.formats.first().copied().context("no format")?;
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
        let (depth, depth_view) = make_depth(&device, config.width, config.height);
        Ok(Gpu {
            _instance: instance,
            surface,
            device,
            queue,
            config,
            depth,
            depth_view,
        })
    }
    fn resize(&mut self, w: u32, h: u32) {
        if w == 0 || h == 0 {
            return;
        }
        self.config.width = w;
        self.config.height = h;
        self.surface.configure(&self.device, &self.config);
        let (depth, depth_view) = make_depth(&self.device, self.config.width, self.config.height);
        self.depth = depth;
        self.depth_view = depth_view;
    }
}

struct Shell {
    window: Option<Arc<Window>>,
    gpu: Option<Gpu>,
    app: Option<EditorApp>,
    egui_state: Option<egui_winit::State>,
    egui_renderer: Option<egui_wgpu::Renderer>,
    scene: Option<SceneRenderer>,
    /// Optional scene to auto-load and immediately play (`--play scene.json`).
    launch_play: Option<String>,
    /// Phase 3 plugin host: owns the real editor tools (Move) as plugins.
    plugins: PluginHost,
}

/// Vision: handle a screenshot request + save any delivered editor frame.
///
/// When the user clicks "📷 Send to AI", `app.screenshot_requested` is set; we
/// issue an egui `ViewportCommand::Screenshot` once. egui-wgpu reads back the
/// real framebuffer (3D scene + panels) and delivers `egui::Event::Screenshot`,
/// which arrives on a later frame's `events`. We save it to `.editor/frame.png`
/// so the VLM sees the SAME view the human edits.
fn handle_screenshot_request(ctx: &egui::Context, app: &mut EditorApp, events: &[egui::Event]) {
    if app.screenshot_requested {
        app.screenshot_requested = false;
        ctx.send_viewport_cmd(egui::ViewportCommand::Screenshot(Default::default()));
    }
    for ev in events {
        if let egui::Event::Screenshot { image, .. } = ev {
            let path = std::path::Path::new(".editor/frame.png");
            match openengine_editor_shell::screenshot::save_screenshot(image, path) {
                Ok(()) => app.ai_notice = Some(format!("✅ frame → {}", path.display())),
                Err(e) => app.ai_notice = Some(format!("screenshot failed: {e}")),
            }
        }
    }
}

impl Shell {
    fn new(launch_play: Option<String>) -> Self {
        // Register the real Move tool behind the plugin boundary at startup.
        let mut plugins = PluginHost::new();
        let f = FrameCtx { frame: 0, dt: 0.0 };
        let move_tool = MoveToolPlugin::new(0.5, [0.0, 0.0], Arc::new(AtomicU32::new(0)));
        // A tool failing to register is a non-fatal boundary error: log it and
        // keep running (tool just absent). Never crash the editor shell.
        if let Err(e) = plugins.add(Box::new(move_tool), &f) {
            eprintln!("move-tool plugin: {e}");
        }
        Shell {
            window: None,
            gpu: None,
            app: None,
            egui_state: None,
            egui_renderer: None,
            scene: None,
            launch_play,
            plugins,
        }
    }
}

impl ApplicationHandler for Shell {
    fn resumed(&mut self, event_loop: &ActiveEventLoop) {
        if self.window.is_some() {
            return;
        }
        let window = Arc::new(
            event_loop
                .create_window(
                    Window::default_attributes()
                        .with_title("OpenEngine — Editor Shell")
                        .with_inner_size(LogicalSize::new(1400.0, 900.0)),
                )
                .expect("create window"),
        );
        let gpu = pollster::block_on(Gpu::new(window.clone())).expect("init gpu");
        let format = gpu.config.format;

        let egui_ctx = egui::Context::default();
        let mut app = EditorApp::new();
        app.egui_ctx = egui_ctx.clone();
        // `--play <scene>`: auto-load the scene and start playing it.
        if let Some(scene) = self.launch_play.take() {
            app.scene_path = scene;
            app.load_scene();
            app.state.play();
            app.scene_notice = Some("playing (stop to edit)".into());
        }
        let egui_state = egui_winit::State::new(
            egui_ctx,
            ViewportId::ROOT,
            window.as_ref(),
            None,
            None,
            None,
        );
        let egui_renderer = egui_wgpu::Renderer::new(&gpu.device, format, None, 1, false);
        let scene = SceneRenderer::new(&gpu.device, &gpu.queue, format);

        self.window = Some(window);
        self.gpu = Some(gpu);
        self.app = Some(app);
        self.egui_state = Some(egui_state);
        self.egui_renderer = Some(egui_renderer);
        self.scene = Some(scene);
    }

    fn window_event(&mut self, event_loop: &ActiveEventLoop, _id: WindowId, event: WindowEvent) {
        let Some(window) = &self.window else { return };
        if let Some(state) = &mut self.egui_state {
            if state.on_window_event(window, &event).consumed {
                return;
            }
        }
        match event {
            WindowEvent::CloseRequested => event_loop.exit(),
            WindowEvent::Resized(size) => {
                if let Some(gpu) = &mut self.gpu {
                    gpu.resize(size.width, size.height);
                }
            }
            WindowEvent::MouseInput {
                state: winit::event::ElementState::Pressed,
                button: winit::event::MouseButton::Left,
                ..
            } => {
                if let Some(app) = &mut self.app {
                    let pos = app.egui_ctx.input(|i| i.pointer.hover_pos());
                    if let Some(p) = pos {
                        app.handle_viewport_click(p);
                    }
                }
            }
            WindowEvent::RedrawRequested => self.render(),
            _ => {}
        }
    }
}

impl Shell {
    fn render(&mut self) {
        let (Some(window), Some(gpu), Some(app), Some(state), Some(renderer), Some(scene)) = (
            &self.window,
            &mut self.gpu,
            &mut self.app,
            &mut self.egui_state,
            &mut self.egui_renderer,
            &mut self.scene,
        ) else {
            return;
        };
        // 1. Run egui to compute panels + the central viewport rect.
        let raw_input = state.take_egui_input(window);
        let ctx = app.egui_ctx.clone();
        // Vision: respond to a screenshot request + save any returned frame.
        handle_screenshot_request(&ctx, app, &raw_input.events);
        let full_output = ctx.run(raw_input, |ctx| app.ui(ctx));
        state.handle_platform_output(window, full_output.platform_output);
        app.step_simulation();

        // Phase 3: step the hosted editor tools once per shell frame so the
        // registered Move plugin really runs (its updates are synthetic drags —
        // the live UI rewiring is deferred to visual validation). A plugin error
        // is non-fatal here; log and continue.
        let plugin_frame = FrameCtx {
            frame: app.frame,
            dt: 1.0 / 60.0,
        };
        if let Err(e) = self.plugins.update_all(&plugin_frame) {
            eprintln!("plugin update: {e}");
        }

        // 2. Render the 3D scene (cubes) first, then egui on top (Load).
        let frame = match gpu.surface.get_current_texture() {
            Ok(f) => f,
            Err(e) => {
                eprintln!("surface: {e:#}");
                return;
            }
        };
        let view = frame
            .texture
            .create_view(&wgpu::TextureViewDescriptor::default());
        let mut encoder = gpu
            .device
            .create_command_encoder(&wgpu::CommandEncoderDescriptor { label: None });

        if let Some(rect) = app.viewport_rect {
            scene.set_show_grid(app.show_grid);
            let aspect = (rect.width() / rect.height().max(1.0)).max(0.01);
            let world = app.state.active_world();
            scene.draw(
                &gpu.device,
                &gpu.queue,
                &mut encoder,
                &view,
                &gpu.depth_view,
                world,
                &app.camera,
                aspect,
            );
        }

        // 3. egui paint pass over the cubes (LoadOp::Load).
        let clipped = app
            .egui_ctx
            .tessellate(full_output.shapes, full_output.pixels_per_point);
        let screen = egui_wgpu::ScreenDescriptor {
            size_in_pixels: [gpu.config.width, gpu.config.height],
            pixels_per_point: window.scale_factor() as f32,
        };
        for (id, image_delta) in &full_output.textures_delta.set {
            renderer.update_texture(&gpu.device, &gpu.queue, *id, image_delta);
        }
        renderer.update_buffers(&gpu.device, &gpu.queue, &mut encoder, &clipped, &screen);
        {
            let mut pass = encoder
                .begin_render_pass(&wgpu::RenderPassDescriptor {
                    label: Some("egui"),
                    color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                        view: &view,
                        resolve_target: None,
                        ops: wgpu::Operations {
                            load: wgpu::LoadOp::Load,
                            store: wgpu::StoreOp::Store,
                        },
                    })],
                    depth_stencil_attachment: None,
                    timestamp_writes: None,
                    occlusion_query_set: None,
                })
                .forget_lifetime();
            renderer.render(&mut pass, &clipped, &screen);
        }
        for id in &full_output.textures_delta.free {
            renderer.free_texture(id);
        }
        gpu.queue.submit(std::iter::once(encoder.finish()));
        frame.present();
        window.request_redraw();
    }
}

fn main() -> anyhow::Result<()> {
    let args: Vec<String> = std::env::args().collect();
    // `--play <scene.json>` auto-loads and plays a saved scene.
    let mut play = None;
    let mut i = 1;
    while i < args.len() {
        if args[i] == "--play" {
            play = args.get(i + 1).cloned();
            i += 1;
        }
        i += 1;
    }
    let event_loop = EventLoop::new().context("event loop")?;
    event_loop.set_control_flow(ControlFlow::Poll);
    let mut shell = Shell::new(play);
    event_loop
        .run_app(&mut shell)
        .map_err(anyhow::Error::from)?;
    Ok(())
}
