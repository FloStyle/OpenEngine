//! Simple procedural mesh generation (Domain A, f32 presentation).
//!
//! Produces CPU-side vertex/index data that is later uploaded to wgpu buffers.

use bytemuck::{Pod, Zeroable};

/// Interleaved vertex: position + normal + uv (presentation only; never in
/// Domain B logic).
#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable)]
pub struct Vertex {
    pub position: [f32; 3],
    pub normal: [f32; 3],
    pub uv: [f32; 2],
}

impl Vertex {
    pub const LAYOUT: wgpu::VertexBufferLayout<'static> = wgpu::VertexBufferLayout {
        array_stride: std::mem::size_of::<Vertex>() as wgpu::BufferAddress,
        step_mode: wgpu::VertexStepMode::Vertex,
        attributes: &[
            wgpu::VertexAttribute {
                offset: 0,
                shader_location: 0,
                format: wgpu::VertexFormat::Float32x3,
            },
            wgpu::VertexAttribute {
                offset: 12,
                shader_location: 1,
                format: wgpu::VertexFormat::Float32x3,
            },
            wgpu::VertexAttribute {
                offset: 24,
                shader_location: 2,
                format: wgpu::VertexFormat::Float32x2,
            },
        ],
    };
}

/// A CPU mesh ready to upload.
pub struct Mesh {
    pub vertices: Vec<Vertex>,
    pub indices: Vec<u32>,
}

/// A UV sphere centered at the origin.
pub fn uv_sphere(rings: u32, segments: u32, radius: f32) -> Mesh {
    let mut vertices = Vec::new();
    let mut indices = Vec::new();

    for r in 0..=rings {
        let v = r as f32 / rings as f32;
        let phi = v * std::f32::consts::PI;
        for s in 0..=segments {
            let u = s as f32 / segments as f32;
            let theta = u * std::f32::consts::TAU;
            let x = radius * phi.sin() * theta.cos();
            let y = radius * phi.cos();
            let z = radius * phi.sin() * theta.sin();
            let n = glam::Vec3::new(x, y, z).normalize();
            vertices.push(Vertex {
                position: [x, y, z],
                normal: [n.x, n.y, n.z],
                uv: [u, v],
            });
        }
    }
    let stride = segments + 1;
    for r in 0..rings {
        for s in 0..segments {
            let a = r * stride + s;
            let b = a + stride;
            indices.extend_from_slice(&[a, b, a + 1, a + 1, b, b + 1]);
        }
    }
    Mesh { vertices, indices }
}

/// A flat quad lying in the XZ plane at `y=0`, subdivided `n x n`, normal +Y.
pub fn grid_plane(n: u32, size: f32) -> Mesh {
    let half = size / 2.0;
    let mut vertices = Vec::new();
    let mut indices = Vec::new();
    for j in 0..=n {
        for i in 0..=n {
            let fx = i as f32 / n as f32;
            let fz = j as f32 / n as f32;
            let x = -half + fx * size;
            let z = -half + fz * size;
            vertices.push(Vertex {
                position: [x, 0.0, z],
                normal: [0.0, 1.0, 0.0],
                uv: [fx * size / 4.0, fz * size / 4.0],
            });
        }
    }
    let stride = n + 1;
    for j in 0..n {
        for i in 0..n {
            let a = j * stride + i;
            let b = a + stride;
            indices.extend_from_slice(&[a, b, a + 1, a + 1, b, b + 1]);
        }
    }
    Mesh { vertices, indices }
}

/// Grid **line** segments across the ground plane (for the "grid" look),
/// used with `PrimitiveTopology::LineList`.
pub fn grid_lines(n: u32, size: f32) -> Mesh {
    let half = size / 2.0;
    let step = size / n as f32;
    let mut vertices = Vec::new();
    let mut indices = Vec::new();
    let push_line =
        |vertices: &mut Vec<Vertex>, indices: &mut Vec<u32>, a: [f32; 3], b: [f32; 3]| {
            let base = vertices.len() as u32;
            vertices.push(Vertex {
                position: a,
                normal: [0.0, 1.0, 0.0],
                uv: [0.0, 0.0],
            });
            vertices.push(Vertex {
                position: b,
                normal: [0.0, 1.0, 0.0],
                uv: [0.0, 0.0],
            });
            indices.extend_from_slice(&[base, base + 1]);
        };
    let mut t = -half;
    while t <= half + f32::EPSILON {
        push_line(
            &mut vertices,
            &mut indices,
            [t, 0.003, -half],
            [t, 0.003, half],
        );
        push_line(
            &mut vertices,
            &mut indices,
            [-half, 0.003, t],
            [half, 0.003, t],
        );
        t += step;
    }
    Mesh { vertices, indices }
}
