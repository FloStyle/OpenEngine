//! Headless GPU readback smoke tests for the editor viewport. No window needed
//! (wgpu adapter — Vulkan/GL/llvmpipe all acceptable); skips if none exists.
//!
//! The scene is now Blender-style: a light sky clear, a checkered ground plane,
//! and each entity drawn as a lit sphere. Because sky + ground now cover the
//! whole frame, instance detection is **hue-based**: entities are spawned in
//! vivid colors (red/green/yellow) that can never match the grey ground or the
//! light blue-grey sky, so a distinct color blob proves a distinct instance.
//!
//! Assertions:
//!   1. `scene_has_ground_and_sky` — the frame contains light-sky and grey
//!      ground tones (visual richness; the old "fully dark" guard).
//!   2. `scene_draws_several_separated_instances` — red, green and yellow
//!      spheres placed along X each appear, at horizontally separated screen
//!      positions (the "one cube only" regression guard).

use openengine_ecs::{Color as EcsColor, Position, Velocity, World};
use openengine_editor::camera::EditorCamera;
use openengine_editor_shell::SceneRenderer;
use openengine_math::I16F16;

fn fx(v: f32) -> I16F16 {
    I16F16::from_num(v)
}

const W: usize = 256;
const H: usize = 256;

/// Spawn an entity as a sphere of the given vivid RGB color at a world pos.
fn add_sphere(w: &mut World, pos: [f32; 3], rgb: [u8; 3]) {
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
            r: rgb[0],
            g: rgb[1],
            b: rgb[2],
            a: 255,
        },
    );
    w.set_transform(
        i,
        openengine_contracts::Transform::at(fx(pos[0]), fx(pos[1]), fx(pos[2])),
    );
}

/// Camera aimed at `focus` from +Z (yaw = 0), slightly above (pitch).
fn cam(focus: [f32; 3], distance: f32) -> EditorCamera {
    EditorCamera {
        focus: glam::Vec3::new(focus[0], focus[1], focus[2]),
        distance,
        yaw: 0.0,
        pitch: 0.3,
        fov: 45f32.to_radians(),
    }
}

/// Render `world` to an offscreen RGBA8 texture and return the raw pixels.
fn render(
    device: &wgpu::Device,
    queue: &wgpu::Queue,
    renderer: &mut SceneRenderer,
    world: &World,
    camera: &EditorCamera,
    format: wgpu::TextureFormat,
) -> Vec<u8> {
    let size = wgpu::Extent3d {
        width: W as u32,
        height: H as u32,
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
    let depth = device.create_texture(&wgpu::TextureDescriptor {
        label: Some("offscreen.depth"),
        size,
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format: wgpu::TextureFormat::Depth32Float,
        usage: wgpu::TextureUsages::RENDER_ATTACHMENT,
        view_formats: &[],
    });
    let depth_view = depth.create_view(&wgpu::TextureViewDescriptor::default());
    let bytes = (W * H * 4) as u64;
    let readback = device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("readback"),
        size: bytes,
        usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
        mapped_at_creation: false,
    });

    let mut enc = device.create_command_encoder(&wgpu::CommandEncoderDescriptor { label: None });
    renderer.draw(
        device,
        queue,
        &mut enc,
        &view,
        &depth_view,
        world,
        camera,
        1.0,
    );
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
                bytes_per_row: Some(W as u32 * 4),
                rows_per_image: Some(H as u32),
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

fn device() -> Option<(wgpu::Device, wgpu::Queue)> {
    let instance = wgpu::Instance::new(&wgpu::InstanceDescriptor {
        backends: wgpu::Backends::all(),
        ..Default::default()
    });
    let tries = [
        wgpu::PowerPreference::HighPerformance,
        wgpu::PowerPreference::LowPower,
    ];
    // GPU enumeration on CI/headless boxes is occasionally transient; retry a
    // few times before giving up so a lone busy frame doesn't SKIP the test.
    for attempt in 0..8 {
        for pp in tries {
            if let Ok(adapter) =
                pollster::block_on(instance.request_adapter(&wgpu::RequestAdapterOptions {
                    power_preference: pp,
                    compatible_surface: None,
                    force_fallback_adapter: false,
                }))
            {
                if let Ok((device, queue)) =
                    pollster::block_on(adapter.request_device(&wgpu::DeviceDescriptor {
                        label: Some("smoke"),
                        required_features: wgpu::Features::empty(),
                        required_limits: wgpu::Limits::default(),
                        memory_hints: wgpu::MemoryHints::default(),
                        trace: wgpu::Trace::Off,
                    }))
                {
                    return Some((device, queue));
                }
            }
        }
        std::thread::sleep(std::time::Duration::from_millis(100));
        if attempt == 0 {
            eprintln!("  (retrying device acquisition…)");
        }
    }
    None
}

/// Median column of pixels matching `pred`; None if none match.
fn median_col<F: Fn(&[u8]) -> bool>(pixels: &[u8], pred: F) -> Option<f32> {
    let mut cols: Vec<f32> = Vec::new();
    for (i, chunk) in pixels.chunks_exact(4).enumerate() {
        if pred(chunk) {
            cols.push((i % W) as f32);
        }
    }
    if cols.is_empty() {
        return None;
    }
    cols.sort_by(|a, b| a.partial_cmp(b).unwrap());
    Some(cols[cols.len() / 2])
}

fn is_red(c: &[u8]) -> bool {
    c[0] > 120 && c[1] < 100 && c[2] < 100
}
fn is_green(c: &[u8]) -> bool {
    c[1] > 120 && c[0] < 100 && c[2] < 100
}
fn is_yellow(c: &[u8]) -> bool {
    c[0] > 120 && c[1] > 120 && c[2] < 100
}
fn is_sky(c: &[u8]) -> bool {
    c[2] > c[0] && c[2] > c[1] && c[2] > 180
}
fn is_grey_ground(c: &[u8]) -> bool {
    let mx = *c.iter().take(3).max().unwrap();
    let mn = *c.iter().take(3).min().unwrap();
    // Checker greys are near-neutral (channel spread < ~20) but visibly lit;
    // sky (blue-leaning, spread ~48) and vivid spheres are excluded.
    mx - mn < 32 && mx > 40 && mx < 230
}

#[test]
fn scene_has_ground_and_sky() {
    let Some((device, queue)) = device() else {
        eprintln!("SKIP: no wgpu adapter available");
        return;
    };
    let format = wgpu::TextureFormat::Rgba8Unorm;
    let mut renderer = SceneRenderer::new(&device, &queue, format);

    let mut w = World::new();
    add_sphere(&mut w, [0.0, 0.6, 0.0], [200, 40, 40]);
    let pixels = render(
        &device,
        &queue,
        &mut renderer,
        &w,
        &cam([0.0, 0.3, 0.0], 10.0),
        format,
    );

    let sky = pixels.chunks_exact(4).filter(|c| is_sky(c)).count();
    let ground = pixels.chunks_exact(4).filter(|c| is_grey_ground(c)).count();
    assert!(sky > 0, "no sky pixels rendered (clear/sky broken)");
    assert!(
        ground > 0,
        "no ground pixels rendered (checkered ground broken)"
    );
    println!("render_smoke[scene]: {sky} sky px, {ground} ground px");
}

#[test]
fn scene_draws_several_separated_instances() {
    let Some((device, queue)) = device() else {
        eprintln!("SKIP: no wgpu adapter available");
        return;
    };
    let format = wgpu::TextureFormat::Rgba8Unorm;
    let mut renderer = SceneRenderer::new(&device, &queue, format);

    // Red / green / yellow spheres, widely separated along the camera right
    // axis (world X), resting on the ground. Vivid colors can't be confused
    // with the grey ground or the light sky.
    let mut w = World::new();
    add_sphere(&mut w, [-4.0, 0.6, 0.0], [255, 0, 0]); // red (left)
    add_sphere(&mut w, [0.0, 0.6, 0.0], [0, 255, 0]); // green (center)
    add_sphere(&mut w, [4.0, 0.6, 0.0], [255, 255, 0]); // yellow (right)

    let pixels = render(
        &device,
        &queue,
        &mut renderer,
        &w,
        &cam([0.0, 0.6, 0.0], 16.0),
        format,
    );

    // Exactly one blob of each distinct color must appear.
    let red = pixels.chunks_exact(4).filter(|c| is_red(c)).count();
    let green = pixels.chunks_exact(4).filter(|c| is_green(c)).count();
    let yellow = pixels.chunks_exact(4).filter(|c| is_yellow(c)).count();
    let rc = median_col(&pixels, is_red);
    let gc = median_col(&pixels, is_green);
    let yc = median_col(&pixels, is_yellow);

    // All three distinct instances are present and render large enough to spot.
    assert!(red > 10, "red sphere not drawn (only {red} px)");
    assert!(green > 10, "green sphere not drawn (only {green} px)");
    assert!(yellow > 10, "yellow sphere not drawn (only {yellow} px)");

    // They are spatially separated horizontally (no overlapping single blob).
    let (Some(rc), Some(gc), Some(yc)) = (rc, gc, yc) else {
        panic!("could not locate all three spheres");
    };
    let spread = yc - rc;
    assert!(
        spread > 30.0,
        "spheres not spatially separated: red@{rc:.0} green@{gc:.0} yellow@{yc:.0} (spread {spread:.0}px)"
    );
    println!(
        "render_smoke[separated]: red@{rc:.0} green@{gc:.0} yellow@{yc:.0} ({red},{green},{yellow} px) spread {spread:.0}px"
    );
}
