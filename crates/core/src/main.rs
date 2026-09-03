//! # Domain A — "3D Demoscene" (`openengine-core` binary)
//!
//! Renders a metal sphere, a rough dielectric sphere, an emissive sphere and a
//! ground grid with a lit WGSL shader (Blinn-Phong + Schlick Fresnel + fake
//! sky env), perspective orbit camera, depth buffer, tone-mapped output, and
//! time animation. This proves the mesh + lighting + depth render path that the
//! editor viewport will reuse. f32/glam allowed here (presentation, Domain A).
//!
//! Requires a display + Vulkan: run on your machine (`cargo run -p openengine-core`).

mod mesh;

use std::sync::Arc;

use anyhow::Context;
use glam::{Mat4, Quat, Vec3};
use mesh::{Mesh, Vertex};
use winit::application::ApplicationHandler;
use winit::dpi::LogicalSize;
use winit::event::WindowEvent;
use winit::event_loop::{ActiveEventLoop, ControlFlow, EventLoop};
use winit::window::{Window, WindowId};

const FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Bgra8UnormSrgb;

// ---------------------------------------------------------------------------
// GPU surface state + depth
// ---------------------------------------------------------------------------
struct Gpu {
    _instance: wgpu::Instance,
    surface: wgpu::Surface<'static>,
    device: wgpu::Device,
    queue: wgpu::Queue,
    config: wgpu::SurfaceConfiguration,
    depth: wgpu::TextureView,
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
                label: Some("demoscene.device"),
                required_features: wgpu::Features::empty(),
                required_limits: wgpu::Limits::default(),
                memory_hints: wgpu::MemoryHints::default(),
                trace: wgpu::Trace::Off,
            })
            .await
            .context("device")?;
        let size = window.inner_size();
        let caps = surface.get_capabilities(&adapter);
        let format = if caps.formats.contains(&FORMAT) {
            FORMAT
        } else {
            caps.formats[0]
        };
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
        let depth = Self::create_depth(&device, config.width, config.height);
        Ok(Gpu {
            _instance: instance,
            surface,
            device,
            queue,
            config,
            depth,
        })
    }

    fn create_depth(device: &wgpu::Device, w: u32, h: u32) -> wgpu::TextureView {
        let tex = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("depth"),
            size: wgpu::Extent3d {
                width: w.max(1),
                height: h.max(1),
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: wgpu::TextureFormat::Depth32Float,
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT,
            view_formats: &[],
        });
        tex.create_view(&wgpu::TextureViewDescriptor::default())
    }

    fn resize(&mut self, w: u32, h: u32) {
        if w == 0 || h == 0 {
            return;
        }
        self.config.width = w;
        self.config.height = h;
        self.surface.configure(&self.device, &self.config);
        self.depth = Self::create_depth(&self.device, w, h);
    }
}

// ---------------------------------------------------------------------------
// Scene description + per-object uniform packing
// ---------------------------------------------------------------------------
#[derive(Clone, Copy)]
struct Object {
    pos: [f32; 3],
    scale: f32,
    color: [f32; 4],
    metallic: f32,
    roughness: f32,
    emissive: f32,
    model: Mat4,
}

fn pack_frame(vp: &Mat4, eye: Vec3, light: Vec3) -> Vec<f32> {
    let mut v = vp.to_cols_array().to_vec();
    v.extend_from_slice(&[eye.x, eye.y, eye.z, 1.0]);
    v.extend_from_slice(&[light.x, light.y, light.z, 1.0]);
    v
}

fn pack_object(o: &Object) -> Vec<f32> {
    let mut v = o.model.to_cols_array().to_vec();
    v.extend_from_slice(&o.color);
    v.extend_from_slice(&[o.metallic, o.roughness, o.emissive, 0.0]);
    v
}

fn write_u(queue: &wgpu::Queue, buf: &wgpu::Buffer, data: &[f32]) {
    queue.write_buffer(buf, 0, bytemuck::cast_slice(data));
}

// ---------------------------------------------------------------------------
// GPU mesh + asset holder
// ---------------------------------------------------------------------------
struct GpuMesh {
    vbuf: wgpu::Buffer,
    ibuf: wgpu::Buffer,
    n: u32,
}

struct SceneAssets {
    frame_buffer: wgpu::Buffer,
    frame_bind: wgpu::BindGroup,
    object_buffer: wgpu::Buffer,
    object_bind: wgpu::BindGroup,
    solid: wgpu::RenderPipeline,
    lines: wgpu::RenderPipeline,
    sphere: GpuMesh,
    floor: GpuMesh,
    grid: GpuMesh,
}

fn upload_mesh(device: &wgpu::Device, queue: &wgpu::Queue, m: &Mesh) -> GpuMesh {
    let vbuf = device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("vbuf"),
        size: (m.vertices.len() * std::mem::size_of::<Vertex>()) as u64,
        usage: wgpu::BufferUsages::VERTEX,
        mapped_at_creation: false,
    });
    let ibuf = device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("ibuf"),
        size: (m.indices.len() * 4) as u64,
        usage: wgpu::BufferUsages::INDEX,
        mapped_at_creation: false,
    });
    queue.write_buffer(&vbuf, 0, bytemuck::cast_slice(&m.vertices));
    queue.write_buffer(&ibuf, 0, bytemuck::cast_slice(&m.indices));
    GpuMesh {
        vbuf,
        ibuf,
        n: m.indices.len() as u32,
    }
}

impl SceneAssets {
    fn new(device: &wgpu::Device, queue: &wgpu::Queue) -> Self {
        let mk_buffer = |label: &str, size: u64| {
            device.create_buffer(&wgpu::BufferDescriptor {
                label: Some(label),
                size,
                usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
                mapped_at_creation: false,
            })
        };
        let frame_buffer = mk_buffer("frame.ub", 96);
        let object_buffer = mk_buffer("object.ub", 96);

        let frame_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("frame.bgl"),
            entries: &[wgpu::BindGroupLayoutEntry {
                binding: 0,
                visibility: wgpu::ShaderStages::VERTEX_FRAGMENT,
                ty: wgpu::BindingType::Buffer {
                    ty: wgpu::BufferBindingType::Uniform,
                    has_dynamic_offset: false,
                    min_binding_size: None,
                },
                count: None,
            }],
        });
        let object_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("object.bgl"),
            entries: &[wgpu::BindGroupLayoutEntry {
                binding: 0,
                visibility: wgpu::ShaderStages::VERTEX_FRAGMENT,
                ty: wgpu::BindingType::Buffer {
                    ty: wgpu::BufferBindingType::Uniform,
                    has_dynamic_offset: false,
                    min_binding_size: None,
                },
                count: None,
            }],
        });
        let frame_bind = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("frame.bg"),
            layout: &frame_layout,
            entries: &[wgpu::BindGroupEntry {
                binding: 0,
                resource: frame_buffer.as_entire_binding(),
            }],
        });
        let object_bind = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("object.bg"),
            layout: &object_layout,
            entries: &[wgpu::BindGroupEntry {
                binding: 0,
                resource: object_buffer.as_entire_binding(),
            }],
        });

        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("scene"),
            source: wgpu::ShaderSource::Wgsl(include_str!("scene.wgsl").into()),
        });
        let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("layout"),
            bind_group_layouts: &[&frame_layout, &object_layout],
            push_constant_ranges: &[],
        });
        let mk_pipeline =
            |vs: &str, fs: &str, topo: wgpu::PrimitiveTopology, cull: Option<wgpu::Face>| {
                device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
                    label: Some("pipeline"),
                    layout: Some(&pipeline_layout),
                    vertex: wgpu::VertexState {
                        module: &shader,
                        entry_point: Some(vs),
                        compilation_options: wgpu::PipelineCompilationOptions::default(),
                        buffers: &[Vertex::LAYOUT],
                    },
                    primitive: wgpu::PrimitiveState {
                        topology: topo,
                        strip_index_format: None,
                        front_face: wgpu::FrontFace::Ccw,
                        cull_mode: cull,
                        polygon_mode: wgpu::PolygonMode::Fill,
                        unclipped_depth: false,
                        conservative: false,
                    },
                    depth_stencil: Some(wgpu::DepthStencilState {
                        format: wgpu::TextureFormat::Depth32Float,
                        depth_write_enabled: true,
                        depth_compare: wgpu::CompareFunction::Less,
                        stencil: wgpu::StencilState::default(),
                        bias: wgpu::DepthBiasState::default(),
                    }),
                    multisample: wgpu::MultisampleState::default(),
                    fragment: Some(wgpu::FragmentState {
                        module: &shader,
                        entry_point: Some(fs),
                        compilation_options: wgpu::PipelineCompilationOptions::default(),
                        targets: &[Some(wgpu::ColorTargetState {
                            format: FORMAT,
                            blend: None,
                            write_mask: wgpu::ColorWrites::ALL,
                        })],
                    }),
                    multiview: None,
                    cache: None,
                })
            };
        let solid = mk_pipeline(
            "vs_solid",
            "fs_solid",
            wgpu::PrimitiveTopology::TriangleList,
            Some(wgpu::Face::Back),
        );
        let lines = mk_pipeline(
            "vs_line",
            "fs_line",
            wgpu::PrimitiveTopology::LineList,
            None,
        );

        let sphere = upload_mesh(device, queue, &mesh::uv_sphere(48, 96, 1.0));
        let floor = upload_mesh(device, queue, &mesh::grid_plane(1, 1.0));
        let grid = upload_mesh(device, queue, &mesh::grid_lines(24, 12.0));

        SceneAssets {
            frame_buffer,
            frame_bind,
            object_buffer,
            object_bind,
            solid,
            lines,
            sphere,
            floor,
            grid,
        }
    }

    fn draw_mesh(&self, pass: &mut wgpu::RenderPass<'_>, m: &GpuMesh) {
        pass.set_vertex_buffer(0, m.vbuf.slice(..));
        pass.set_index_buffer(m.ibuf.slice(..), wgpu::IndexFormat::Uint32);
        pass.draw_indexed(0..m.n, 0, 0..1);
    }
}

// ---------------------------------------------------------------------------
// App
// ---------------------------------------------------------------------------
struct App {
    window: Option<Arc<Window>>,
    gpu: Option<Gpu>,
    assets: Option<SceneAssets>,
    start: std::time::Instant,
    // Scene objects: build fresh each frame with animation.
}

impl App {
    fn new() -> Self {
        App {
            window: None,
            gpu: None,
            assets: None,
            start: std::time::Instant::now(),
        }
    }

    fn time(&self) -> f32 {
        self.start.elapsed().as_secs_f32()
    }
}

fn sphere_model(pos: Vec3, scale: f32) -> Mat4 {
    Mat4::from_translation(pos) * Mat4::from_scale(Vec3::splat(scale))
}

fn floor_model(scale: f32, y: f32) -> Mat4 {
    Mat4::from_scale_rotation_translation(
        Vec3::new(scale, 1.0, scale),
        Quat::IDENTITY,
        Vec3::new(0.0, y, 0.0),
    )
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
                        .with_title("OpenEngine — 3D Demoscene")
                        .with_inner_size(LogicalSize::new(900.0, 600.0)),
                )
                .expect("create window"),
        );
        let gpu = pollster::block_on(Gpu::new(window.clone())).expect("init gpu");
        let assets = SceneAssets::new(&gpu.device, &gpu.queue);
        self.window = Some(window);
        self.gpu = Some(gpu);
        self.assets = Some(assets);
    }

    fn window_event(&mut self, event_loop: &ActiveEventLoop, _id: WindowId, event: WindowEvent) {
        match event {
            WindowEvent::CloseRequested => event_loop.exit(),
            WindowEvent::Resized(size) => {
                if let Some(gpu) = self.gpu.as_mut() {
                    gpu.resize(size.width, size.height);
                }
            }
            _ => {}
        }
    }

    fn about_to_wait(&mut self, _el: &ActiveEventLoop) {
        let t = self.time();
        let (gpu, assets) = match (&mut self.gpu, &mut self.assets) {
            (Some(g), Some(a)) => (g, a),
            _ => return,
        };
        let w = gpu.config.width as f32;
        let h = gpu.config.height as f32;
        let aspect = w / h.max(1.0);

        // Camera (orbit) + light (rotating).
        let focus = Vec3::new(0.0, 0.3, 0.0);
        let dist = 6.0;
        let eye = focus + Vec3::new(dist * t.sin() * 0.35, dist * 0.4, dist * t.cos() * 0.35);
        let proj = Mat4::perspective_rh(45f32.to_radians(), aspect, 0.1, 100.0);
        let view_m = Mat4::look_at_rh(eye, focus, Vec3::Y);
        let vp = proj * view_m;
        let light = Vec3::new(t.cos() * 5.0, 6.0, t.sin() * 5.0);
        write_u(
            &gpu.queue,
            &assets.frame_buffer,
            &pack_frame(&vp, eye, light),
        );

        // Build animated scene objects.
        let bob = |x: f32, z: f32, s: f32| Vec3::new(x, s + 0.25 * t.sin() * (1.0 + x * 0.2), z);
        let objs = [
            Object {
                pos: bob(-1.6, 0.0, 0.4).to_array(),
                scale: 1.0,
                color: [0.75, 0.75, 0.78, 1.0],
                metallic: 1.0,
                roughness: 0.15,
                emissive: 0.0,
                model: Mat4::IDENTITY,
            },
            Object {
                pos: bob(1.6, 0.0, 0.4).to_array(),
                scale: 1.0,
                color: [0.9, 0.4, 0.2, 1.0],
                metallic: 0.0,
                roughness: 0.6,
                emissive: 0.0,
                model: Mat4::IDENTITY,
            },
            Object {
                pos: bob(0.0, -1.6, 0.7).to_array(),
                scale: 0.7,
                color: [1.0, 0.5, 0.0, 1.0],
                metallic: 0.0,
                roughness: 0.5,
                emissive: 2.0,
                model: Mat4::IDENTITY,
            },
        ];

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
            .create_command_encoder(&wgpu::CommandEncoderDescriptor { label: Some("enc") });
        {
            let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("pass"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: &view,
                    resolve_target: None,
                    ops: wgpu::Operations {
                        load: wgpu::LoadOp::Clear(wgpu::Color {
                            r: 0.08,
                            g: 0.09,
                            b: 0.12,
                            a: 1.0,
                        }),
                        store: wgpu::StoreOp::Store,
                    },
                })],
                depth_stencil_attachment: Some(wgpu::RenderPassDepthStencilAttachment {
                    view: &gpu.depth,
                    depth_ops: Some(wgpu::Operations {
                        load: wgpu::LoadOp::Clear(1.0),
                        store: wgpu::StoreOp::Store,
                    }),
                    stencil_ops: None,
                }),
                timestamp_writes: None,
                occlusion_query_set: None,
            });
            pass.set_bind_group(0, &assets.frame_bind, &[]);
            pass.set_bind_group(1, &assets.object_bind, &[]);

            // Floor (lit, big plane). Floor + grid use model identity for lines.
            pass.set_pipeline(&assets.solid);
            let floor = Object {
                pos: [0.0, -1.5, 0.0],
                scale: 1.0,
                color: [0.16, 0.16, 0.18, 1.0],
                metallic: 0.0,
                roughness: 0.8,
                emissive: 0.0,
                model: floor_model(6.0, -1.5),
            };
            write_u(&gpu.queue, &assets.object_buffer, &pack_object(&floor));
            assets.draw_mesh(&mut pass, &assets.floor);

            for o in &objs {
                let model = sphere_model(Vec3::new(o.pos[0], o.pos[1], o.pos[2]), o.scale);
                let oo = Object {
                    pos: o.pos,
                    scale: o.scale,
                    color: o.color,
                    metallic: o.metallic,
                    roughness: o.roughness,
                    emissive: o.emissive,
                    model,
                };
                write_u(&gpu.queue, &assets.object_buffer, &pack_object(&oo));
                assets.draw_mesh(&mut pass, &assets.sphere);
            }

            // Grid lines overlay.
            pass.set_pipeline(&assets.lines);
            let grid_obj = Object {
                pos: [0.0, -1.49, 0.0],
                scale: 1.0,
                color: [0.35, 0.35, 0.42, 1.0],
                metallic: 0.0,
                roughness: 1.0,
                emissive: 0.0,
                model: Mat4::IDENTITY,
            };
            write_u(&gpu.queue, &assets.object_buffer, &pack_object(&grid_obj));
            assets.draw_mesh(&mut pass, &assets.grid);
        }
        gpu.queue.submit(std::iter::once(encoder.finish()));
        frame.present();
    }
}

fn main() -> anyhow::Result<()> {
    let event_loop = EventLoop::new().context("event loop")?;
    event_loop.set_control_flow(ControlFlow::Poll);
    let mut app = App::new();
    event_loop.run_app(&mut app).map_err(anyhow::Error::from)?;
    Ok(())
}
