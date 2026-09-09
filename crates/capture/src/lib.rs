//! # Headless offscreen capture (Domain A, wgpu)
//!
//! Renders an [`ecs::World`] with the same viewport pipeline the GUI editor uses
//! (a copy of `editor-shell`'s scene renderer + shader, so the PNG matches the
//! on-screen scene), entirely offscreen — no window — and encodes it as PNG.
//!
//! Used by the harness `GET /frame` endpoint (behind the `capture` feature) and
//! by a CLI so a multimodal model can *see* the engine's live world.

/// The copied scene renderer + WGSL (single viewport pipeline, wgpu-only).
pub mod renderer;

use openengine_ecs::World;
use openengine_editor::camera::EditorCamera;

pub use renderer::SceneRenderer;

/// Error surfaced by capture (typed; a missing GPU adapter is not a crash).
#[derive(Debug)]
pub enum CaptureError {
    /// No wgpu adapter/device could be acquired (headless CI, no GPU).
    NoAdapter(String),
    /// wgpu render/readback failed.
    Render(String),
    /// PNG encoding failed.
    Encode(String),
}

impl std::fmt::Display for CaptureError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            CaptureError::NoAdapter(m) => write!(f, "no gpu adapter: {m}"),
            CaptureError::Render(m) => write!(f, "render: {m}"),
            CaptureError::Encode(m) => write!(f, "encode: {m}"),
        }
    }
}

impl std::error::Error for CaptureError {}

/// Acquire an offscreen wgpu device+queue, retrying a few times (GPU
/// enumeration on CI/headless is occasionally transient). Returns `None` when no
/// adapter exists — callers turn that into [`CaptureError::NoAdapter`].
pub fn device() -> Option<(wgpu::Device, wgpu::Queue)> {
    let instance = wgpu::Instance::new(&wgpu::InstanceDescriptor {
        backends: wgpu::Backends::all(),
        ..Default::default()
    });
    let tries = [
        wgpu::PowerPreference::HighPerformance,
        wgpu::PowerPreference::LowPower,
    ];
    for _ in 0..8 {
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
                        label: Some("capture"),
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
    }
    None
}

/// Render `world` from `camera` into an offscreen RGBA8 texture and read the
/// raw bytes back (pre-PNG). `w`/`h` are the target dimensions.
fn render_rgba(
    world: &World,
    camera: &EditorCamera,
    w: u32,
    h: u32,
) -> Result<Vec<u8>, CaptureError> {
    let (device, queue) =
        device().ok_or_else(|| CaptureError::NoAdapter("no wgpu adapter available".into()))?;
    let format = wgpu::TextureFormat::Rgba8Unorm;
    let mut renderer = SceneRenderer::new(&device, &queue, format);

    let size = wgpu::Extent3d {
        width: w.max(1),
        height: h.max(1),
        depth_or_array_layers: 1,
    };
    let tex = device.create_texture(&wgpu::TextureDescriptor {
        label: Some("capture.tex"),
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
        label: Some("capture.depth"),
        size,
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format: wgpu::TextureFormat::Depth32Float,
        usage: wgpu::TextureUsages::RENDER_ATTACHMENT,
        view_formats: &[],
    });
    let depth_view = depth.create_view(&wgpu::TextureViewDescriptor::default());

    // bytes_per_row must be a multiple of 256 for arbitrary widths.
    let bytes_per_row_padded = {
        let bpr = w * 4;
        (bpr + 255) & !255
    };
    let readback = device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("capture.readback"),
        size: (bytes_per_row_padded * h) as u64,
        usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
        mapped_at_creation: false,
    });

    let aspect = w as f32 / (h.max(1) as f32);
    let mut enc = device.create_command_encoder(&wgpu::CommandEncoderDescriptor { label: None });
    renderer.draw(
        &device,
        &queue,
        &mut enc,
        &view,
        &depth_view,
        world,
        camera,
        aspect,
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
                bytes_per_row: Some(bytes_per_row_padded),
                rows_per_image: Some(h),
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

    // De-pad: PNG wants tightly-packed rows of w*4 bytes.
    if bytes_per_row_padded == w * 4 {
        Ok(out)
    } else {
        let mut tight = Vec::with_capacity((w * h * 4) as usize);
        for row in out.chunks(bytes_per_row_padded as usize) {
            tight.extend_from_slice(&row[..(w * 4) as usize]);
        }
        Ok(tight)
    }
}

/// Render `world` from `camera` and return a PNG-encoded image.
pub fn capture_world_png(
    world: &World,
    camera: &EditorCamera,
    w: u32,
    h: u32,
) -> Result<Vec<u8>, CaptureError> {
    let rgba = render_rgba(world, camera, w, h)?;
    encode_png(&rgba, w.max(1), h.max(1))
}

/// Encode tightly-packed RGBA8 bytes as PNG.
pub fn encode_png(rgba: &[u8], w: u32, h: u32) -> Result<Vec<u8>, CaptureError> {
    let mut out = std::io::Cursor::new(Vec::new());
    {
        let mut enc = png::Encoder::new(&mut out, w, h);
        enc.set_color(png::ColorType::Rgba);
        enc.set_depth(png::BitDepth::Eight);
        let mut writer = enc
            .write_header()
            .map_err(|e| CaptureError::Encode(e.to_string()))?;
        writer
            .write_image_data(rgba)
            .map_err(|e| CaptureError::Encode(e.to_string()))?;
    }
    Ok(out.into_inner())
}

#[cfg(test)]
mod tests {
    use super::*;
    use openengine_ecs::{Color as EcsColor, Position, Velocity};
    use openengine_math::I16F16;

    fn fx(v: f32) -> I16F16 {
        I16F16::from_num(v)
    }

    fn sample_world() -> World {
        let mut w = World::new();
        for (i, color) in [[255u8, 0, 0], [0, 255, 0], [255, 255, 0]]
            .into_iter()
            .enumerate()
        {
            let e = w.spawn(
                Position {
                    x: fx(0.0),
                    y: fx(0.0),
                },
                Velocity {
                    x: fx(0.0),
                    y: fx(0.0),
                },
                EcsColor {
                    r: color[0],
                    g: color[1],
                    b: color[2],
                    a: 255,
                },
            );
            let p = [(i as f32 - 1.0) * 4.0, 0.6, 0.0];
            w.set_transform(
                e,
                openengine_contracts::Transform::at(fx(p[0]), fx(p[1]), fx(p[2])),
            );
        }
        w
    }

    fn cam() -> EditorCamera {
        EditorCamera {
            focus: glam::Vec3::ZERO,
            distance: 16.0,
            yaw: 0.0,
            pitch: 0.3,
            fov: 45f32.to_radians(),
        }
    }

    #[test]
    fn png_encoder_is_valid() {
        let w = 8u32;
        let h = 8u32;
        let rgba = vec![128u8; (w * h * 4) as usize];
        let png = encode_png(&rgba, w, h).expect("png encode");
        // PNG magic.
        assert_eq!(&png[..8], &[0x89, b'P', b'N', b'G', 0x0d, 0x0a, 0x1a, 0x0a]);
    }

    // Real render+readback; SKIPs when no GPU adapter (like editor render_smoke).
    #[test]
    fn capture_renders_a_png_when_gpu_available() {
        let Some((device, queue)) = device() else {
            eprintln!("SKIP: no wgpu adapter available");
            return;
        };
        drop((device, queue));
        let w = sample_world();
        let png = capture_world_png(&w, &cam(), 128, 128);
        match png {
            Ok(bytes) => assert_eq!(
                &bytes[..8],
                &[0x89, b'P', b'N', b'G', 0x0d, 0x0a, 0x1a, 0x0a]
            ),
            Err(CaptureError::NoAdapter(_)) => {
                eprintln!("SKIP: adapter vanished between probe and render")
            }
            Err(e) => panic!("capture failed: {e}"),
        }
    }
}
