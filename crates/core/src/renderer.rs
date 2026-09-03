//! Quad renderer for the PoC prototype (wgpu 25, Vulkan, Domain A).
//!
//! Draws each entity as a colored quad. Fixed-point world positions/colors are
//! converted to `f32` ONLY here at the presentation boundary (never stored).

use bytemuck::{Pod, Zeroable};
use openengine_ecs::{Color, World};

/// Max quads drawn per frame.
const MAX_QUADS: usize = 2048;
/// Size of each quad, in world/pixel units.
const QUAD_SIZE: f32 = 10.0;

#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable)]
struct Vertex {
    position: [f32; 2],
    color: [f32; 4],
}

/// GPU state + pipeline for colored quads.
pub struct QuadRenderer {
    pipeline: wgpu::RenderPipeline,
    vertex_buffer: wgpu::Buffer,
    uniform_buffer: wgpu::Buffer,
    bind_group: wgpu::BindGroup,
}

impl QuadRenderer {
    /// Build the pipeline, uniform buffer and a large reusable vertex buffer.
    pub fn new(device: &wgpu::Device, format: wgpu::TextureFormat) -> Self {
        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("quad_shader"),
            source: wgpu::ShaderSource::Wgsl(include_str!("quad_shader.wgsl").into()),
        });

        let uniform_buffer = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("quad.uniforms"),
            size: 16,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let bind_group_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("quad.bgl"),
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
        let bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("quad.bg"),
            layout: &bind_group_layout,
            entries: &[wgpu::BindGroupEntry {
                binding: 0,
                resource: uniform_buffer.as_entire_binding(),
            }],
        });

        let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("quad.layout"),
            bind_group_layouts: &[&bind_group_layout],
            push_constant_ranges: &[],
        });

        let pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("quad.pipeline"),
            layout: Some(&pipeline_layout),
            vertex: wgpu::VertexState {
                module: &shader,
                entry_point: Some("vs_main"),
                compilation_options: wgpu::PipelineCompilationOptions::default(),
                buffers: &[wgpu::VertexBufferLayout {
                    array_stride: std::mem::size_of::<Vertex>() as wgpu::BufferAddress,
                    step_mode: wgpu::VertexStepMode::Vertex,
                    attributes: &[
                        wgpu::VertexAttribute {
                            offset: 0,
                            shader_location: 0,
                            format: wgpu::VertexFormat::Float32x2,
                        },
                        wgpu::VertexAttribute {
                            offset: 8,
                            shader_location: 1,
                            format: wgpu::VertexFormat::Float32x4,
                        },
                    ],
                }],
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

        let vertex_buffer = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("quad.vertices"),
            size: (MAX_QUADS * 6 * std::mem::size_of::<Vertex>()) as u64,
            usage: wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });

        QuadRenderer {
            pipeline,
            vertex_buffer,
            uniform_buffer,
            bind_group,
        }
    }

    /// Rebuild vertex data from `world`, write buffers, and draw into `view`.
    pub fn render(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        view: &wgpu::TextureView,
        world: &World,
        viewport_size: (u32, u32),
    ) {
        let count = world.entity_count().min(MAX_QUADS);
        let positions = world.get_positions().unwrap_or(&[]);
        let colors = world.get_colors().unwrap_or(&[]);

        // Uniform: viewport size (vec2 + pad).
        let viewport_uniform: [f32; 4] = [viewport_size.0 as f32, viewport_size.1 as f32, 0.0, 0.0];
        queue.write_buffer(
            &self.uniform_buffer,
            0,
            bytemuck::cast_slice(&viewport_uniform),
        );

        let mut verts: Vec<Vertex> = Vec::with_capacity(count * 6);
        for i in 0..count {
            let p = &positions[i];
            let c = &colors[i];
            let x = p.x.to_num::<f32>();
            let y = p.y.to_num::<f32>();
            let color = [
                c.r as f32 / 255.0,
                c.g as f32 / 255.0,
                c.b as f32 / 255.0,
                c.a as f32 / 255.0,
            ];
            // Two triangles (6 verts) per quad.
            for corner in [
                [x, y],
                [x + QUAD_SIZE, y],
                [x + QUAD_SIZE, y + QUAD_SIZE],
                [x, y],
                [x + QUAD_SIZE, y + QUAD_SIZE],
                [x, y + QUAD_SIZE],
            ] {
                verts.push(Vertex {
                    position: corner,
                    color,
                });
            }
        }
        queue.write_buffer(&self.vertex_buffer, 0, bytemuck::cast_slice(&verts));

        let mut encoder = device.create_command_encoder(&wgpu::CommandEncoderDescriptor {
            label: Some("quad.encoder"),
        });
        {
            let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("quad.pass"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view,
                    resolve_target: None,
                    ops: wgpu::Operations {
                        load: wgpu::LoadOp::Clear(wgpu::Color {
                            r: 0.05,
                            g: 0.05,
                            b: 0.08,
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
            pass.set_bind_group(0, &self.bind_group, &[]);
            pass.set_vertex_buffer(0, self.vertex_buffer.slice(..));
            pass.draw(0..(count as u32 * 6), 0..1);
        }
        queue.submit(std::iter::once(encoder.finish()));
    }
}

// Silence "unused" in case only used via Gpu::present_quads.
#[allow(dead_code)]
fn _color_alias(_: Color) {}
