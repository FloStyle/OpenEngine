//! Headless GPU readback smoke test: render cubes to an offscreen texture and
//! prove non-background pixels were drawn. No window required (needs a wgpu
//! adapter — Vulkan/GL/llvmpipe all acceptable). Skips if no adapter exists.

use openengine_ecs::{Color as EcsColor, Position, Velocity, World};
use openengine_editor::camera::EditorCamera;
use openengine_editor_shell::SceneRenderer;
use openengine_math::I16F16;

fn fx(v: f32) -> I16F16 {
    I16F16::from_num(v)
}

fn build_world() -> World {
    let mut w = World::new();
    let add = |w: &mut World, pos: [f32; 3]| {
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
    };
    add(&mut w, [0.0, 0.0, 0.0]);
    add(&mut w, [3.0, 0.0, 0.0]);
    add(&mut w, [0.0, 0.0, 3.0]);
    w
}

#[test]
fn scene_draws_non_background_pixels() {
    let instance = wgpu::Instance::new(&wgpu::InstanceDescriptor {
        backends: wgpu::Backends::all(),
        ..Default::default()
    });
    let adapter = pollster::block_on(instance.request_adapter(&wgpu::RequestAdapterOptions {
        power_preference: wgpu::PowerPreference::LowPower,
        compatible_surface: None,
        force_fallback_adapter: true,
    }));
    let adapter = match adapter {
        Ok(a) => a,
        Err(e) => {
            eprintln!("SKIP: no wgpu adapter available ({e})");
            return;
        }
    };
    let (device, queue) = pollster::block_on(adapter.request_device(&wgpu::DeviceDescriptor {
        label: Some("smoke"),
        required_features: wgpu::Features::empty(),
        required_limits: wgpu::Limits::default(),
        memory_hints: wgpu::MemoryHints::default(),
        trace: wgpu::Trace::Off,
    }))
    .expect("device");

    let format = wgpu::TextureFormat::Rgba8Unorm;
    let size = wgpu::Extent3d {
        width: 256,
        height: 256,
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
    let bytes = 256u64 * 256 * 4;
    let readback = device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("readback"),
        size: bytes,
        usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
        mapped_at_creation: false,
    });

    let world = build_world();
    let camera = EditorCamera {
        focus: glam::Vec3::new(1.5, 0.5, 1.5),
        distance: 12.0,
        yaw: 0.6,
        pitch: 0.35,
        fov: 45f32.to_radians(),
    };
    let renderer = SceneRenderer::new(&device, &queue, format);

    // Draw cubes into the offscreen texture.
    let mut enc = device.create_command_encoder(&wgpu::CommandEncoderDescriptor { label: None });
    renderer.draw(&device, &queue, &mut enc, &view, &world, &camera, 1.0);
    queue.submit(std::iter::once(enc.finish()));

    // Copy to the readback buffer.
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
                bytes_per_row: Some(256 * 4),
                rows_per_image: Some(256),
            },
        },
        size,
    );
    queue.submit(std::iter::once(enc.finish()));

    let slice = readback.slice(..);
    slice.map_async(wgpu::MapMode::Read, |_| {});
    let _ = device.poll(wgpu::PollType::Wait);
    let data = slice.get_mapped_range();
    let mut non_bg = 0usize;
    for px in data.chunks_exact(4) {
        // Background clear is roughly (0.05,0.06,0.09)->(13,15,23); count pixels
        // that are clearly brighter/coloured (drawn cubes).
        if px[0] > 60 || px[1] > 60 || px[2] > 60 {
            non_bg += 1;
        }
    }
    drop(data);
    assert!(
        non_bg > 0,
        "renderer produced NO non-background pixels ({non_bg}) — pipeline/camera broken"
    );
    println!("render_smoke OK: {non_bg} non-background pixels drawn");
}
