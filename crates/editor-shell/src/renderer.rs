//! Minimal 3D scene renderer for the editor viewport (Domain A).
//!
//! Blender-style lit scene: a checkered ground plane plus each entity drawn as
//! an analytic sphere lit by one directional light + ambient (no depth sort, a
//! depth buffer handles occlusion). Everything projects with the
//! `EditorCamera` view-projection.
//!
//! Two draw modes share one pipeline (see `viewport.wgsl`):
//!   * mode 0 — entity sphere at its fixed-point `Transform` position;
//!   * mode 1 — the ground plane, whose fragment shades XZ checkers.

use bytemuck::{Pod, Zeroable};
use openengine_ecs::World;
use openengine_editor::camera::EditorCamera;

#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable)]
struct Vertex {
    pos: [f32; 3],
    normal: [f32; 3],
}

const DEPTH_FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Depth32Float;

/// Sphere radius (world units) used to draw each entity.
const RADIUS: f32 = 0.6;
/// Half-extent of the ground quad (world units).
const GROUND_EXTENT: f32 = 60.0;

/// Generate a UV sphere mesh of `radius` with given stack/sector density.
fn sphere_mesh(radius: f32, stacks: u32, sectors: u32) -> (Vec<Vertex>, Vec<u16>) {
    let mut verts: Vec<Vertex> = Vec::new();
    let mut idx: Vec<u16> = Vec::new();
    let push = |verts: &mut Vec<Vertex>, u: u32, v: u32| {
        let phi = u as f32 / stacks as f32 * std::f32::consts::PI;
        let theta = v as f32 / sectors as f32 * std::f32::consts::TAU;
        let (sp, cp) = phi.sin_cos();
        let (st, ct) = theta.sin_cos();
        let dir = [sp * ct, cp, sp * st];
        verts.push(Vertex {
            pos: [dir[0] * radius, dir[1] * radius, dir[2] * radius],
            normal: dir,
        });
    };
    for u in 0..=stacks {
        for v in 0..=sectors {
            push(&mut verts, u, v);
        }
    }
    let w = sectors + 1;
    for u in 0..stacks {
        for v in 0..sectors {
            let a: u16 = (u * w + v) as u16;
            let b: u16 = a + w as u16;
            // Two triangles (CCW when viewed from outside).
            idx.extend_from_slice(&[a, b, a + 1, a + 1, b, b + 1]);
        }
    }
    (verts, idx)
}

/// Bind a 96-byte window of an arrayed dynamic-offset uniform buffer. Binding a
/// fixed window (not the whole buffer) is what lets a non-zero dynamic offset
/// slide the shader's view to a different slot.
fn object_bind_for(
    device: &wgpu::Device,
    layout: &wgpu::BindGroupLayout,
    buf: &wgpu::Buffer,
) -> wgpu::BindGroup {
    device.create_bind_group(&wgpu::BindGroupDescriptor {
        label: Some("object.bg"),
        layout,
        entries: &[wgpu::BindGroupEntry {
            binding: 0,
            resource: wgpu::BindingResource::Buffer(wgpu::BufferBinding {
                buffer: buf,
                offset: 0,
                size: wgpu::BufferSize::new(96),
            }),
        }],
    })
}

/// One 4-vertex ground quad at y=0 spanning +/-`extent` in X and Z.
fn ground_quad(extent: f32) -> (Vec<Vertex>, Vec<u16>) {
    let e = extent;
    let verts = vec![
        Vertex {
            pos: [-e, 0.0, -e],
            normal: [0.0, 1.0, 0.0],
        },
        Vertex {
            pos: [e, 0.0, -e],
            normal: [0.0, 1.0, 0.0],
        },
        Vertex {
            pos: [e, 0.0, e],
            normal: [0.0, 1.0, 0.0],
        },
        Vertex {
            pos: [-e, 0.0, e],
            normal: [0.0, 1.0, 0.0],
        },
    ];
    (verts, vec![0, 1, 2, 0, 2, 3])
}

/// Byte stride between per-draw uniform slots. wgpu requires dynamic uniform
/// offsets be a multiple of the adapter's alignment (256B for most adapters),
/// so each 96B object uniform lives in its own aligned slot.
const OBJECT_STRIDE: u64 = 256;

/// Draws the lit ground + one sphere per entity.
pub struct SceneRenderer {
    pipeline: wgpu::RenderPipeline,
    object_layout: wgpu::BindGroupLayout,
    sphere_vbuf: wgpu::Buffer,
    sphere_ibuf: wgpu::Buffer,
    sphere_count: u32,
    ground_vbuf: wgpu::Buffer,
    ground_ibuf: wgpu::Buffer,
    frame_buffer: wgpu::Buffer,
    object_buffer: wgpu::Buffer,
    frame_bind: wgpu::BindGroup,
    object_bind: wgpu::BindGroup,
    /// How many 256B object slots `object_buffer` currently holds.
    object_slots: u32,
}

impl SceneRenderer {
    pub fn new(device: &wgpu::Device, queue: &wgpu::Queue, format: wgpu::TextureFormat) -> Self {
        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("viewport"),
            source: wgpu::ShaderSource::Wgsl(include_str!("viewport.wgsl").into()),
        });
        let frame_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("frame.bgl"),
            entries: &[wgpu::BindGroupLayoutEntry {
                binding: 0,
                visibility: wgpu::ShaderStages::VERTEX,
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
                    // A single arrayed buffer with per-draw dynamic offsets. The
                    // 96B min lets wgpu validate that each offset window fits.
                    has_dynamic_offset: true,
                    min_binding_size: wgpu::BufferSize::new(96),
                },
                count: None,
            }],
        });
        // Frame = mat4 (64B), single static slot. Object uniforms use a separate
        // growable dynamic-offset buffer allocated in `ensure_object_slots`.
        let frame_buffer = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("frame.ub"),
            size: 64,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let object_buffer = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("object.ub"),
            size: OBJECT_STRIDE,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let mk = |layout: &wgpu::BindGroupLayout, buf: &wgpu::Buffer| {
            device.create_bind_group(&wgpu::BindGroupDescriptor {
                label: Some("bg"),
                layout,
                entries: &[wgpu::BindGroupEntry {
                    binding: 0,
                    resource: buf.as_entire_binding(),
                }],
            })
        };
        let frame_bind = mk(&frame_layout, &frame_buffer);
        let object_bind = object_bind_for(device, &object_layout, &object_buffer);

        let pl = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("pl"),
            bind_group_layouts: &[&frame_layout, &object_layout],
            push_constant_ranges: &[],
        });
        let vertex_layout = wgpu::VertexBufferLayout {
            array_stride: std::mem::size_of::<Vertex>() as u64,
            step_mode: wgpu::VertexStepMode::Vertex,
            attributes: &[
                wgpu::VertexAttribute {
                    offset: 0,
                    shader_location: 0,
                    format: wgpu::VertexFormat::Float32x3,
                },
                wgpu::VertexAttribute {
                    offset: std::mem::size_of::<[f32; 3]>() as u64,
                    shader_location: 1,
                    format: wgpu::VertexFormat::Float32x3,
                },
            ],
        };
        let pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("viewport.pipeline"),
            layout: Some(&pl),
            vertex: wgpu::VertexState {
                module: &shader,
                entry_point: Some("vs_main"),
                compilation_options: wgpu::PipelineCompilationOptions::default(),
                buffers: &[vertex_layout],
            },
            primitive: wgpu::PrimitiveState {
                topology: wgpu::PrimitiveTopology::TriangleList,
                strip_index_format: None,
                front_face: wgpu::FrontFace::Ccw,
                cull_mode: None,
                polygon_mode: wgpu::PolygonMode::Fill,
                unclipped_depth: false,
                conservative: false,
            },
            depth_stencil: Some(wgpu::DepthStencilState {
                format: DEPTH_FORMAT,
                depth_write_enabled: true,
                depth_compare: wgpu::CompareFunction::LessEqual,
                stencil: wgpu::StencilState::default(),
                bias: wgpu::DepthBiasState::default(),
            }),
            multisample: wgpu::MultisampleState::default(),
            fragment: Some(wgpu::FragmentState {
                module: &shader,
                entry_point: Some("fs_main"),
                compilation_options: wgpu::PipelineCompilationOptions::default(),
                targets: &[Some(wgpu::ColorTargetState {
                    format,
                    blend: Some(wgpu::BlendState::REPLACE),
                    write_mask: wgpu::ColorWrites::ALL,
                })],
            }),
            multiview: None,
            cache: None,
        });

        let mkbuf = |device: &wgpu::Device, verts: &[Vertex], idx: &[u16]| {
            let vb = device.create_buffer(&wgpu::BufferDescriptor {
                label: Some("mesh.vbuf"),
                size: std::mem::size_of_val(verts) as u64,
                usage: wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::COPY_DST,
                mapped_at_creation: false,
            });
            queue.write_buffer(&vb, 0, bytemuck::cast_slice(verts));
            let ib = device.create_buffer(&wgpu::BufferDescriptor {
                label: Some("mesh.ibuf"),
                size: std::mem::size_of_val(idx) as u64,
                usage: wgpu::BufferUsages::INDEX | wgpu::BufferUsages::COPY_DST,
                mapped_at_creation: false,
            });
            queue.write_buffer(&ib, 0, bytemuck::cast_slice(idx));
            (vb, ib)
        };

        let (sphere_verts, sphere_idx) = sphere_mesh(RADIUS, 12, 20);
        let sphere_count = sphere_idx.len() as u32;
        let (sphere_vbuf, sphere_ibuf) = mkbuf(device, &sphere_verts, &sphere_idx);
        let (ground_verts, ground_idx) = ground_quad(GROUND_EXTENT);
        let (ground_vbuf, ground_ibuf) = mkbuf(device, &ground_verts, &ground_idx);

        SceneRenderer {
            pipeline,
            object_layout,
            sphere_vbuf,
            sphere_ibuf,
            sphere_count,
            ground_vbuf,
            ground_ibuf,
            frame_buffer,
            object_buffer,
            frame_bind,
            object_bind,
            object_slots: 1,
        }
    }

    /// (Re)size the dynamic-offset object buffer to hold at least `needed`
    /// slots, recreating the bind group since it captures the whole buffer.
    fn ensure_object_slots(&mut self, device: &wgpu::Device, needed: u32) {
        if self.object_slots >= needed {
            return;
        }
        self.object_buffer = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("object.ub"),
            size: OBJECT_STRIDE * needed as u64,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        self.object_bind = object_bind_for(device, &self.object_layout, &self.object_buffer);
        self.object_slots = needed;
    }

    /// Serialize one object uniform (model, color, mode) into `out`.
    fn object_bytes(model: &glam::Mat4, color: [f32; 3], mode: f32, out: &mut [u8]) {
        let mut obj = [0.0f32; 24];
        obj[..16].copy_from_slice(&model.to_cols_array());
        obj[16..19].copy_from_slice(&color);
        obj[20] = mode;
        out[..96].copy_from_slice(bytemuck::cast_slice(&obj));
    }

    /// Draw the lit scene (ground + one sphere per entity) into `view` with the
    /// given depth `view` for occlusion. The color target is cleared to a sky.
    #[allow(clippy::too_many_arguments)] // cohesive wgpu call; refactor later
    pub fn draw(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        encoder: &mut wgpu::CommandEncoder,
        view: &wgpu::TextureView,
        depth_view: &wgpu::TextureView,
        world: &World,
        camera: &EditorCamera,
        aspect: f32,
    ) {
        let n = world.entity_count();
        // Slot 0 = ground; slots 1..=n = entities.
        let slots = 1u32 + n as u32;
        self.ensure_object_slots(device, slots);

        // Fill every object uniform up-front in one write so the pass never
        // reads a half-updated buffer (see note in module docs re write order).
        let mut staging = vec![0u8; (OBJECT_STRIDE * slots as u64) as usize];
        Self::object_bytes(&glam::Mat4::IDENTITY, [1.0; 3], 1.0, &mut staging[0..96]);
        let transforms = world.get_transforms().unwrap_or(&[]);
        let colors = world.get_colors().unwrap_or(&[]);
        for i in 0..n {
            let Some(p) = transforms.get(i).map(|t| {
                [
                    t.position[0].to_num::<f32>(),
                    t.position[1].to_num::<f32>(),
                    t.position[2].to_num::<f32>(),
                ]
            }) else {
                continue;
            };
            let col = colors.get(i).copied().unwrap_or(openengine_ecs::Color {
                r: 255,
                g: 255,
                b: 255,
                a: 255,
            });
            let model = glam::Mat4::from_translation(glam::Vec3::new(p[0], p[1], p[2]));
            let off = (OBJECT_STRIDE * (i as u64 + 1)) as usize;
            Self::object_bytes(
                &model,
                [
                    col.r as f32 / 255.0,
                    col.g as f32 / 255.0,
                    col.b as f32 / 255.0,
                ],
                0.0,
                &mut staging[off..off + 96],
            );
        }
        queue.write_buffer(&self.object_buffer, 0, &staging);

        queue.write_buffer(
            &self.frame_buffer,
            0,
            bytemuck::cast_slice(&camera.view_proj(aspect).to_cols_array()),
        );

        let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
            label: Some("viewport"),
            color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                view,
                resolve_target: None,
                ops: wgpu::Operations {
                    load: wgpu::LoadOp::Clear(wgpu::Color {
                        r: 0.62,
                        g: 0.68,
                        b: 0.78,
                        a: 1.0,
                    }),
                    store: wgpu::StoreOp::Store,
                },
            })],
            depth_stencil_attachment: Some(wgpu::RenderPassDepthStencilAttachment {
                view: depth_view,
                depth_ops: Some(wgpu::Operations {
                    load: wgpu::LoadOp::Clear(1.0),
                    store: wgpu::StoreOp::Discard,
                }),
                stencil_ops: None,
            }),
            timestamp_writes: None,
            occlusion_query_set: None,
        });
        pass.set_pipeline(&self.pipeline);
        pass.set_bind_group(0, &self.frame_bind, &[]);

        // Ground (slot 0).
        pass.set_bind_group(1, &self.object_bind, &[0]);
        pass.set_vertex_buffer(0, self.ground_vbuf.slice(..));
        pass.set_index_buffer(self.ground_ibuf.slice(..), wgpu::IndexFormat::Uint16);
        pass.draw_indexed(0..6, 0, 0..1);

        // Each entity as a lit sphere from its own dynamic-offset slot.
        pass.set_vertex_buffer(0, self.sphere_vbuf.slice(..));
        pass.set_index_buffer(self.sphere_ibuf.slice(..), wgpu::IndexFormat::Uint16);
        for i in 0..n {
            let off = ((i as u64 + 1) * OBJECT_STRIDE) as u32;
            pass.set_bind_group(1, &self.object_bind, &[off]);
            pass.draw_indexed(0..self.sphere_count, 0, 0..1);
        }
        drop(pass);
        let _ = device;
    }
}
