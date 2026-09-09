//! Save the live editor frame (as composited by egui/wgpu) to a PNG that an AI
//! model can see — the SAME view the human edits (gizmos, panels, viewport).
//!
//! The editor-shell requests a screenshot via egui's
//! `ViewportCommand::Screenshot`; egui-wgpu reads back the actual framebuffer
//! (including the 3D scene drawn *before* the egui overlay) and delivers an
//! `egui::Event::Screenshot { image: Arc<ColorImage> }`. We encode that
//! `ColorImage` (RGBA) to a PNG at a known path the harness/CLI can read.

/// Encode an egui `ColorImage` (RGBA8, row-major top-to-bottom) as PNG bytes.
///
/// `Color32` is premultiplied-RGBA; PNG wants straight alpha, so un-premultiply.
pub fn color_image_to_png(img: &egui::ColorImage) -> Result<Vec<u8>, png::EncodingError> {
    let [w, h] = img.size;
    let mut rgba = Vec::with_capacity(w * h * 4);
    for px in &img.pixels {
        // Un-premultiply alpha (PNG is straight alpha). Never divide by zero.
        let a = px.a() as u32;
        let scale = |ch: u8| -> u8 {
            if a == 0 {
                0
            } else {
                ((ch as u32 * 255_u32).checked_div(a).unwrap_or(0)).min(255) as u8
            }
        };
        rgba.extend_from_slice(&[scale(px.r()), scale(px.g()), scale(px.b()), px.a()]);
    }
    let mut out = std::io::Cursor::new(Vec::new());
    {
        let mut enc = png::Encoder::new(&mut out, w as u32, h as u32);
        enc.set_color(png::ColorType::Rgba);
        enc.set_depth(png::BitDepth::Eight);
        let mut writer = enc.write_header()?;
        writer.write_image_data(&rgba)?;
    }
    Ok(out.into_inner())
}

/// Write an egui `ColorImage` to `path` as a PNG (straight alpha).
pub fn save_screenshot(img: &egui::ColorImage, path: &std::path::Path) -> Result<(), String> {
    let bytes = color_image_to_png(img).map_err(|e| format!("png encode: {e}"))?;
    if let Some(parent) = path.parent() {
        if let Err(e) = std::fs::create_dir_all(parent) {
            return Err(format!("mkdir {}: {e}", parent.display()));
        }
    }
    std::fs::write(path, bytes).map_err(|e| format!("write {}: {e}", path.display()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn encodes_a_small_opaque_image() {
        let img = egui::ColorImage::new(
            [2, 2],
            vec![
                egui::Color32::from_rgb(255, 0, 0),
                egui::Color32::from_rgb(0, 255, 0),
                egui::Color32::from_rgb(0, 0, 255),
                egui::Color32::from_rgb(255, 255, 255),
            ],
        );
        let png = color_image_to_png(&img).expect("encode");
        assert_eq!(&png[..8], &[0x89, b'P', b'N', b'G', 0x0d, 0x0a, 0x1a, 0x0a]);
    }

    #[test]
    fn saves_and_roundtrips_a_screenshot() {
        let img = egui::ColorImage::new([1, 1], vec![egui::Color32::WHITE]);
        let p = std::env::temp_dir().join("oe_editor_shot.png");
        save_screenshot(&img, &p).expect("save");
        let bytes = std::fs::read(&p).expect("read");
        assert_eq!(
            &bytes[..8],
            &[0x89, b'P', b'N', b'G', 0x0d, 0x0a, 0x1a, 0x0a]
        );
        let _ = std::fs::remove_file(&p);
    }
}
