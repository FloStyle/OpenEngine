//! Minimal 3D scene renderer for the editor viewport (Domain A).
//!
//! Draws each entity as a small colored cube at its fixed-point `Transform`
//! position, projected with the `EditorCamera` view-projection. Solid (unlit)
//! so it needs no normals/depth; good enough to show/select entities.

use bytemuck::{Pod, Zeroable};
use openengine_ecs::World;
use openengine_editor::camera::EditorCamera;

#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable)]
struct Vertex {
    pos: [f32; 3],
}

/// Cube half-extent (world units) used for every entity.
const CUBE: f32 = 0.4;

fn cube_vertices() -> Vec<Vertex> {
    let c = CUBE;
    let corners = [
        [-c, -c, -c],
        [c, -c, -c],
        [c, c, -c],
        [-c, c, -c],
        [-c, -c, c],
        [c, -c, c],
        [c, c, c],
        [-c, c, c],
    ];
    // 12 triangles (2 per face) -> 36 verts, expanded (no index buffer needed).
    let faces: [[usize; 4]; 6] = [
        [0, 3, 2, 1], // back
        [4, 5, 6, 7], // front
        [0, 1, 5, 4], // bottom
        [2, 3, 7, 6], // top
        [0, 4, 7, 3], // left
        [1, 2, 6, 5], // right
    ];
    let mut out = Vec::with_capacity(36);
    for f in faces {
        let [a, b, c_, d] = f;
        for v in [a, b, c_, a, c_, d] {
            out.push(Vertex { pos: corners[v] });
        }
    }
    out
}

/// Draws a colored cube for each entity of a world.
pub struct SceneRenderer {
    pipeline: wgpu::RenderPipeline,
    vertex_buffer: wgpu::Buffer,
    frame_buffer: wgpu::Buffer,
    object_buffer: wgpu::Buffer,
    frame_bind: wgpu::BindGroup,
    object_bind: wgpu::BindGroup,
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
                    has_dynamic_offset: false,
                    min_binding_size: None,
                },
                count: None,
            }],
        });
        let frame_buffer = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("frame.ub"),
            size: 64,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let object_buffer = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("object.ub"),
            size: 80,
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
        let object_bind = mk(&object_layout, &object_buffer);

        let pl = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("pl"),
            bind_group_layouts: &[&frame_layout, &object_layout],
            push_constant_ranges: &[],
        });
        let vertex_layout = wgpu::VertexBufferLayout {
            array_stride: std::mem::size_of::<Vertex>() as u64,
            step_mode: wgpu::VertexStepMode::Vertex,
            attributes: &[wgpu::VertexAttribute {
                offset: 0,
                shader_location: 0,
                format: wgpu::VertexFormat::Float32x3,
            }],
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
                cull_mode: Some(wgpu::Face::Back),
                polygon_mode: wgpu::PolygonMode::Fill,
                unclipped_depth: false,
                conservative: false,
            },
            depth_stencil: None,
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
        let verts = cube_vertices();
        let vertex_buffer = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("cube.vbuf"),
            size: (verts.len() * std::mem::size_of::<Vertex>()) as u64,
            usage: wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        queue.write_buffer(&vertex_buffer, 0, bytemuck::cast_slice(&verts));

        SceneRenderer {
            pipeline,
            vertex_buffer,
            frame_buffer,
            object_buffer,
            frame_bind,
            object_bind,
        }
    }

    /// Draw every entity of `world` with the given camera into `encoder`.
    #[allow(clippy::too_many_arguments)] // cohesive wgpu call; refactor later
    pub fn draw(
        &self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        encoder: &mut wgpu::CommandEncoder,
        view: &wgpu::TextureView,
        world: &World,
        camera: &EditorCamera,
        aspect: f32,
    ) {
        queue.write_buffer(
            &self.frame_buffer,
            0,
            bytemuck::cast_slice(&camera.view_proj(aspect).to_cols_array()),
        );
        let n = world.entity_count();
        let transforms = world.get_transforms().unwrap_or(&[]);
        let colors = world.get_colors().unwrap_or(&[]);

        let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
            label: Some("viewport"),
            color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                view,
                resolve_target: None,
                ops: wgpu::Operations {
                    load: wgpu::LoadOp::Clear(wgpu::Color {
                        r: 0.05,
                        g: 0.06,
                        b: 0.09,
                        a: 1.0,
                    }),
                    store: wgpu::StoreOp::Store,
                },
            })],
            depth_stencil_attachment: None,
            timestamp_writes: None,
            occlusion_query_set: None,
        });
        pass.set_pipeline(&self.pipeline);
        pass.set_bind_group(0, &self.frame_bind, &[]);
        pass.set_bind_group(1, &self.object_bind, &[]);
        pass.set_vertex_buffer(0, self.vertex_buffer.slice(..));
        for i in 0..n {
            let pos = transforms
                .get(i)
                .map(|t| t.position)
                .unwrap_or([openengine_contracts::Fx16::from_num(0); 3]);
            let col = colors.get(i).copied().unwrap_or(openengine_ecs::Color {
                r: 255,
                g: 255,
                b: 255,
                a: 255,
            });
            let p = [
                pos[0].to_num::<f32>(),
                pos[1].to_num::<f32>(),
                pos[2].to_num::<f32>(),
            ];
            let model = glam::Mat4::from_translation(glam::Vec3::new(p[0], p[1], p[2]));
            let object: [f32; 20] = model
                .to_cols_array()
                .into_iter()
                .chain([
                    col.r as f32 / 255.0,
                    col.g as f32 / 255.0,
                    col.b as f32 / 255.0,
                    1.0,
                ])
                .collect::<Vec<_>>()
                .try_into()
                .unwrap();
            queue.write_buffer(&self.object_buffer, 0, bytemuck::cast_slice(&object));
            pass.draw(0..36, 0..1);
        }
        drop(pass);
        let _ = device;
    }
}
