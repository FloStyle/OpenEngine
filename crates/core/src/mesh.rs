//! Simple procedural mesh generation (Domain A, f32 presentation).
//!
//! Produces CPU-side vertex/index data that is later uploaded to wgpu buffers.
//! Also hosts ADR-0004 asset import (`.obj` + registry). The host renderer wires
//! these in; until then the module is intentionally unused (allowed dead_code).
#![allow(dead_code)]

use bytemuck::{Pod, Zeroable};

/// Interleaved vertex: position + normal + uv (presentation only; never in
/// Domain B logic).
#[repr(C)]
#[derive(Clone, Copy, Debug, Pod, Zeroable)]
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

// ────────────────────────────────────────────────────────────────────────────
// § Mesh assets (ADR-0004, Domain A — host only). Data import + registry.
// ────────────────────────────────────────────────────────────────────────────

/// Axis-aligned bounding box over vertex positions.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Aabb {
    pub min: [f32; 3],
    pub max: [f32; 3],
}

impl Aabb {
    pub fn size(&self) -> [f32; 3] {
        [
            self.max[0] - self.min[0],
            self.max[1] - self.min[1],
            self.max[2] - self.min[2],
        ]
    }
}

/// A versioned, render-ready triangle mesh (reuses `Vertex`).
#[derive(Clone, Debug)]
pub struct MeshAsset {
    pub version: u32,
    pub vertices: Vec<Vertex>,
    pub indices: Vec<u32>,
    pub bounds: Aabb,
}

pub const MESH_VERSION: u32 = 1;

fn bounds_of(verts: &[Vertex]) -> Aabb {
    let mut b = Aabb {
        min: [f32::INFINITY; 3],
        max: [f32::NEG_INFINITY; 3],
    };
    for v in verts {
        for i in 0..3 {
            b.min[i] = b.min[i].min(v.position[i]);
            b.max[i] = b.max[i].max(v.position[i]);
        }
    }
    b
}

fn build_mesh(positions: &[[f32; 3]], faces: &[Vec<u32>]) -> MeshAsset {
    let mut vertices: Vec<Vertex> = Vec::new();
    let mut indices: Vec<u32> = Vec::new();
    for face in faces {
        let n = face.len();
        if n < 3 {
            continue;
        }
        for k in 1..n - 1 {
            let (a, b, c) = (face[0], face[k], face[k + 1]);
            let pa = positions[a as usize];
            let pb = positions[b as usize];
            let pc = positions[c as usize];
            let u = [pb[0] - pa[0], pb[1] - pa[1], pb[2] - pa[2]];
            let w = [pc[0] - pa[0], pc[1] - pa[1], pc[2] - pa[2]];
            let nx = u[1] * w[2] - u[2] * w[1];
            let ny = u[2] * w[0] - u[0] * w[2];
            let nz = u[0] * w[1] - u[1] * w[0];
            let len = (nx * nx + ny * ny + nz * nz).sqrt().max(1e-6);
            let normal = [nx / len, ny / len, nz / len];
            let base = vertices.len() as u32;
            vertices.push(Vertex {
                position: pa,
                normal,
                uv: [0.0, 0.0],
            });
            vertices.push(Vertex {
                position: pb,
                normal,
                uv: [0.0, 0.0],
            });
            vertices.push(Vertex {
                position: pc,
                normal,
                uv: [0.0, 0.0],
            });
            indices.extend_from_slice(&[base, base + 1, base + 2]);
        }
    }
    let bounds = bounds_of(&vertices);
    MeshAsset {
        version: MESH_VERSION,
        vertices,
        indices,
        bounds,
    }
}

/// Parse a minimal Wavefront `.obj` (only `v x y z` and `f i[/t][/n]`, 1-based)
/// into a flat-shaded [`MeshAsset`].
pub fn obj_to_mesh(obj: &str) -> Result<MeshAsset, String> {
    let mut positions: Vec<[f32; 3]> = Vec::new();
    let mut faces: Vec<Vec<u32>> = Vec::new();
    for (lineno, raw) in obj.lines().enumerate() {
        let line = raw.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let mut it = line.split_whitespace();
        match it.next() {
            Some("v") => {
                let v: Vec<f32> = it
                    .take(3)
                    .map(|s| {
                        s.parse()
                            .map_err(|_| format!("bad coord @ line {}", lineno + 1))
                    })
                    .collect::<Result<_, _>>()?;
                if v.len() != 3 {
                    return Err(format!("vertex needs 3 coords @ line {}", lineno + 1));
                }
                positions.push([v[0], v[1], v[2]]);
            }
            Some("f") => {
                let mut face = Vec::new();
                for tok in it {
                    let idx: usize = tok
                        .split('/')
                        .next()
                        .ok_or_else(|| format!("bad face @ line {}", lineno + 1))?
                        .parse()
                        .map_err(|_| format!("bad face @ line {}", lineno + 1))?;
                    if idx < 1 {
                        return Err(format!("face index 1-based @ line {}", lineno + 1));
                    }
                    face.push((idx - 1) as u32);
                }
                if face.len() >= 3 {
                    faces.push(face);
                }
            }
            Some(_) => {}
            None => {}
        }
    }
    Ok(build_mesh(&positions, &faces))
}

/// A typed registry of loaded meshes keyed by asset id (`AssetRef.id`).
pub struct AssetRegistry {
    assets: std::collections::BTreeMap<u64, MeshAsset>,
    next: u64,
}

impl Default for AssetRegistry {
    fn default() -> Self {
        Self::new()
    }
}

impl AssetRegistry {
    pub fn new() -> Self {
        AssetRegistry {
            assets: Default::default(),
            next: 1,
        }
    }
    pub fn insert(&mut self, mesh: MeshAsset) -> u64 {
        let id = self.next;
        self.next += 1;
        self.assets.insert(id, mesh);
        id
    }
    pub fn get(&self, id: u64) -> Option<&MeshAsset> {
        self.assets.get(&id)
    }
    pub fn len(&self) -> usize {
        self.assets.len()
    }
}

const CUBE_OBJ: &str = r#"
v -0.5 -0.5 -0.5
v  0.5 -0.5 -0.5
v  0.5  0.5 -0.5
v -0.5  0.5 -0.5
v -0.5 -0.5  0.5
v  0.5 -0.5  0.5
v  0.5  0.5  0.5
v -0.5  0.5  0.5
f 1 2 3 4
f 5 8 7 6
f 1 5 6 2
f 2 6 7 3
f 3 7 8 4
f 5 1 4 8
"#;

/// A unit cube mesh from the built-in `.obj` (test/placeholder).
pub fn cube_mesh() -> MeshAsset {
    obj_to_mesh(CUBE_OBJ).expect("built-in cube obj")
}

#[cfg(test)]
mod asset_tests {
    use super::*;

    #[test]
    fn imports_cube_and_registry() {
        let mut reg = AssetRegistry::new();
        let id = reg.insert(cube_mesh());
        let m = reg.get(id).unwrap();
        // 6 quads = 12 triangles.
        assert_eq!(m.indices.len(), 36);
        assert!((m.bounds.size()[0] - 1.0).abs() < 1e-5);
        assert_eq!(reg.len(), 1);
    }

    #[test]
    fn rejects_malformed_obj() {
        assert!(obj_to_mesh("v 0 0\nf 1 2").is_err());
        assert!(obj_to_mesh("v 0 0 0\nv 1 0 0\nv 0 1 0\nf 0 2 3").is_err());
        assert!(obj_to_mesh("v 0 0 0\nv 1 0 0\nv 0 1 0\nf 1 2 3").is_ok());
    }
}
