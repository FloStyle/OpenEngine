//! Headless GPU readback smoke tests: render cubes to an offscreen texture and
//! prove distinct, spatially separated instances are drawn. No window required
//! (needs a wgpu adapter — Vulkan/GL/llvmpipe all acceptable). Skips if none.
//!
//! Two assertions matter:
//!   1. `scene_draws_some_pixels`   — the pipeline emits *something* (guards
//!      against a fully-dark viewport, the culling bug we hit before).
//!   2. `scene_draws_several_separated_instances` — 3 cubes placed along a line
//!      show up as >= 3 column-runs of lit pixels with empty gaps between them
//!      (guards against the "one cube only" regression: a single instance, or
//!      overlapping instances, cannot produce multiple separated column runs).

use openengine_ecs::{Color as EcsColor, Position, Velocity, World};
use openengine_editor::camera::EditorCamera;
use openengine_editor_shell::SceneRenderer;
use openengine_math::I16F16;

fn fx(v: f32) -> I16F16 {
    I16F16::from_num(v)
}

/// Add a white cube entity at a world position.
fn add_cube(w: &mut World, pos: [f32; 3]) {
    let i = w.spawn(
        Position {
            x: fx(0.0),
            y: fx(0.0),
        },
        Velocity {
            x: fx(0.0),
            y: fx(0.0),
        },
        EcsColor {
            r: 255,
            g: 255,
            b: 255,
            a: 255,
        },
    );
    w.set_transform(
        i,
        openengine_contracts::Transform::at(fx(pos[0]), fx(pos[1]), fx(pos[2])),
    );
}

const W: u32 = 256;
const H: u32 = 256;

/// Camera aimed at the origin from +Z (yaw = 0), slightly above (pitch).
fn cam(distance: f32) -> EditorCamera {
    EditorCamera {
        focus: glam::Vec3::new(0.0, 0.0, 0.0),
        distance,
        yaw: 0.0,
        pitch: 0.3,
        fov: 45f32.to_radians(),
    }
}

/// Render `world` to an offscreen texture and return the RGBA8 pixels.
fn render(
    device: &wgpu::Device,
    queue: &wgpu::Queue,
    renderer: &SceneRenderer,
    world: &World,
    camera: &EditorCamera,
    format: wgpu::TextureFormat,
) -> Vec<u8> {
    let size = wgpu::Extent3d {
        width: W,
        height: H,
        depth_or_array_layers: 1,
    };
    let tex = device.create_texture(&wgpu::TextureDescriptor {
        label: Some("offscreen"),
        size,
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format,
        usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC,
        view_formats: &[],
    });
    let view = tex.create_view(&wgpu::TextureViewDescriptor::default());
    let bytes = (W * H * 4) as u64;
    let readback = device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("readback"),
        size: bytes,
        usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
        mapped_at_creation: false,
    });

    let mut enc = device.create_command_encoder(&wgpu::CommandEncoderDescriptor { label: None });
    renderer.draw(device, queue, &mut enc, &view, world, camera, 1.0);
    queue.submit(std::iter::once(enc.finish()));

    let mut enc = device.create_command_encoder(&wgpu::CommandEncoderDescriptor { label: None });
    enc.copy_texture_to_buffer(
        wgpu::TexelCopyTextureInfo {
            texture: &tex,
            mip_level: 0,
            origin: wgpu::Origin3d::ZERO,
            aspect: wgpu::TextureAspect::All,
        },
        wgpu::TexelCopyBufferInfo {
            buffer: &readback,
            layout: wgpu::TexelCopyBufferLayout {
                offset: 0,
                bytes_per_row: Some(W * 4),
                rows_per_image: Some(H),
            },
        },
        size,
    );
    queue.submit(std::iter::once(enc.finish()));

    let slice = readback.slice(..);
    slice.map_async(wgpu::MapMode::Read, |_| {});
    let _ = device.poll(wgpu::PollType::Wait);
    let data = slice.get_mapped_range();
    let out = data.to_vec();
    drop(data);
    out
}

/// Return (col_runs, pixel_count). A "lit column" is one where at least one
/// pixel is clearly non-background. `col_runs` is the number of contiguous
/// groups of lit columns separated by at least `gap` empty columns.
fn profile(pixels: &[u8]) -> (Vec<(usize, usize)>, usize) {
    let mut lit_cols = vec![false; W as usize];
    let mut count = 0usize;
    for (pix_i, chunk) in pixels.chunks_exact(4).enumerate() {
        let x = pix_i % W as usize;
        if chunk[0] > 60 || chunk[1] > 60 || chunk[2] > 60 {
            lit_cols[x] = true;
            count += 1;
        }
    }
    // Collapse lit columns into runs with a gap tolerance of 3 empty columns.
    let gap = 3usize;
    let mut runs: Vec<(usize, usize)> = Vec::new();
    let mut start: Option<usize> = None;
    let mut last_lit: usize = 0;
    for (x, is_lit) in lit_cols.iter().enumerate() {
        if *is_lit {
            if start.is_none() {
                start = Some(x);
            }
            last_lit = x;
        } else if let Some(s) = start {
            if x - last_lit > gap {
                runs.push((s, last_lit));
                start = None;
            }
        }
    }
    if let Some(s) = start {
        runs.push((s, last_lit));
    }
    (runs, count)
}

fn device() -> Option<(wgpu::Device, wgpu::Queue)> {
    let instance = wgpu::Instance::new(&wgpu::InstanceDescriptor {
        backends: wgpu::Backends::all(),
        ..Default::default()
    });
    let adapter = pollster::block_on(instance.request_adapter(&wgpu::RequestAdapterOptions {
        power_preference: wgpu::PowerPreference::LowPower,
        compatible_surface: None,
        force_fallback_adapter: true,
    }))
    .ok()?;
    let (device, queue) = pollster::block_on(adapter.request_device(&wgpu::DeviceDescriptor {
        label: Some("smoke"),
        required_features: wgpu::Features::empty(),
        required_limits: wgpu::Limits::default(),
        memory_hints: wgpu::MemoryHints::default(),
        trace: wgpu::Trace::Off,
    }))
    .ok()?;
    Some((device, queue))
}

#[test]
fn scene_draws_some_pixels() {
    let Some((device, queue)) = device() else {
        eprintln!("SKIP: no wgpu adapter available");
        return;
    };
    let format = wgpu::TextureFormat::Rgba8Unorm;
    let renderer = SceneRenderer::new(&device, &queue, format);

    let mut w = World::new();
    add_cube(&mut w, [0.0, 0.0, 0.0]);
    add_cube(&mut w, [3.0, 0.0, 0.0]);
    add_cube(&mut w, [0.0, 0.0, 3.0]);

    let pixels = render(&device, &queue, &renderer, &w, &cam(12.0), format);
    let (_, count) = profile(&pixels);
    assert!(
        count > 0,
        "renderer produced NO non-background pixels ({count}) — pipeline/camera broken"
    );
    println!("render_smoke[some]: {count} lit pixels");
}

#[test]
fn scene_draws_several_separated_instances() {
    let Some((device, queue)) = device() else {
        eprintln!("SKIP: no wgpu adapter available");
        return;
    };
    let format = wgpu::TextureFormat::Rgba8Unorm;
    let renderer = SceneRenderer::new(&device, &queue, format);

    // Three cubes well separated along the camera's right axis (world X).
    // Symmetric around the origin, camera looking down -Z from distance 16.
    let mut w = World::new();
    add_cube(&mut w, [-5.0, 0.0, 0.0]);
    add_cube(&mut w, [0.0, 0.0, 0.0]);
    add_cube(&mut w, [5.0, 0.0, 0.0]);

    let camera = cam(16.0);
    let pixels = render(&device, &queue, &renderer, &w, &camera, format);
    let (runs, count) = profile(&pixels);

    // A single instance, or overlapping instances, yields <= ~2 column runs at
    // most; three genuinely separated cubes must yield >= 3 separated runs.
    let msg =
        format!("expected >=3 separated column-runs for 3 cubes, got {runs:?} ({count} lit px)");
    assert!(runs.len() >= 3, "{msg}");
    println!("render_smoke[separated]: {count} lit px across {runs:?}");
}
