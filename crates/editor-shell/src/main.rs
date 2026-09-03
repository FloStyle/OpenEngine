//! OpenEngine Editor Shell binary (Domain A) — winit 0.30 + wgpu 25 + egui 0.32.

use std::sync::Arc;

use anyhow::Context;
use egui::ViewportId;
use openengine_editor_shell::app::EditorApp;
use openengine_editor_shell::renderer::SceneRenderer;
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
}

impl Shell {
    fn new() -> Self {
        Shell {
            window: None,
            gpu: None,
            app: None,
            egui_state: None,
            egui_renderer: None,
            scene: None,
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
        let full_output = ctx.run(raw_input, |ctx| app.ui(ctx));
        state.handle_platform_output(window, full_output.platform_output);
        app.step_simulation();

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
    let event_loop = EventLoop::new().context("event loop")?;
    event_loop.set_control_flow(ControlFlow::Poll);
    let mut shell = Shell::new();
    event_loop
        .run_app(&mut shell)
        .map_err(anyhow::Error::from)?;
    Ok(())
}
