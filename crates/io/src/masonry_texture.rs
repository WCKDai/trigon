//! Original running-bond masonry. No photographs or external texture assets are used.
//!
//! One repeat is 480 × 132 mm: two 240 mm modules and two 66 mm courses. Each module
//! contains a 228 × 54 mm brick and 12 mm joints. Colours enter as linear RGB and are
//! encoded as sRGB, as required for glTF base-colour textures.

use trigon_model::Material;

const WIDTH: u32 = 960;
const HEIGHT: u32 = 264;

fn noise(x: u32, y: u32) -> f32 {
    let mut value = x.wrapping_mul(0x9e37_79b9) ^ y.wrapping_mul(0x85eb_ca6b);
    value ^= value >> 16;
    value = value.wrapping_mul(0x7feb_352d);
    value ^= value >> 15;
    (value & 0xffff) as f32 / 65535.0
}

fn srgb(value: f32) -> u8 {
    let value = value.clamp(0.0, 1.0);
    let encoded = if value <= 0.003_130_8 { 12.92 * value } else { 1.055 * value.powf(1.0 / 2.4) - 0.055 };
    (encoded * 255.0).round() as u8
}

fn brick_edge_mm(u: f64, v: f64) -> (f64, u32, u32) {
    let (u, v) = (u.rem_euclid(1.0), v.rem_euclid(1.0));
    let course = (v * 2.0).floor() as u32;
    let x = u * 2.0 + f64::from(course) * 0.5;
    let local = [x.fract(), (v * 2.0).fract()];
    ((local[0].min(1.0 - local[0]) * 240.0).min(local[1].min(1.0 - local[1]) * 66.0), x.floor() as u32 % 2, course)
}

/// Sample the repeating tile in linear colour; wrapping happens before every variation term.
fn linear_at(material: Material, u: f64, v: f64) -> [f32; 3] {
    let (u, v) = (u.rem_euclid(1.0), v.rem_euclid(1.0));
    let (edge_mm, column, course) = brick_edge_mm(u, v);
    if edge_mm <= 6.0 {
        return [0.42, 0.40, 0.35];
    }
    let brick = material.rgba();
    let variation = 0.94 + 0.12 * noise(column, course + 17);
    let grain = 0.985 + 0.03 * noise((u * f64::from(WIDTH)).floor() as u32, (v * f64::from(HEIGHT)).floor() as u32);
    // A small bevel darkens the first two millimetres beyond the mortar edge.
    let bevel = 0.90 + 0.10 * ((edge_mm - 6.0) / 2.0).clamp(0.0, 1.0) as f32;
    std::array::from_fn(|channel| brick[channel] * variation * grain * bevel)
}

fn height_mm(u: f64, v: f64) -> f64 {
    let edge = brick_edge_mm(u, v).0;
    let ramp = ((edge - 6.0) / 2.0).clamp(0.0, 1.0);
    // A one-millimetre recess and a smooth two-millimetre bevel, entirely in the material.
    ramp * ramp * (3.0 - 2.0 * ramp)
}

fn normal_at(u: f64, v: f64) -> [u8; 3] {
    let du = 1.0 / f64::from(WIDTH);
    let dv = 1.0 / f64::from(HEIGHT);
    let dx = (height_mm(u + du, v) - height_mm(u - du, v)) / (2.0 * du * 480.0);
    let dy = (height_mm(u, v + dv) - height_mm(u, v - dv)) / (2.0 * dv * 132.0);
    let length = (dx * dx + dy * dy + 1.0).sqrt();
    // Normal images carry linear signed-vector components, not sRGB colour.
    [-dx / length, -dy / length, 1.0 / length].map(|component| ((component * 0.5 + 0.5) * 255.0).round() as u8)
}

fn encode(pixels: &[u8]) -> Result<Vec<u8>, png::EncodingError> {
    let mut bytes = Vec::new();
    {
        let mut encoder = png::Encoder::new(&mut bytes, WIDTH, HEIGHT);
        encoder.set_color(png::ColorType::Rgb);
        encoder.set_depth(png::BitDepth::Eight);
        let mut writer = encoder.write_header()?;
        writer.write_image_data(pixels)?;
        writer.finish()?;
    }
    Ok(bytes)
}

pub(crate) fn png(material: Material) -> Result<Vec<u8>, png::EncodingError> {
    let mut pixels = Vec::with_capacity((WIDTH * HEIGHT * 3) as usize);
    for y in 0..HEIGHT {
        for x in 0..WIDTH {
            pixels.extend(linear_at(material, (f64::from(x) + 0.5) / f64::from(WIDTH), (f64::from(y) + 0.5) / f64::from(HEIGHT)).map(srgb));
        }
    }
    encode(&pixels)
}

pub(crate) fn normal_png() -> Result<Vec<u8>, png::EncodingError> {
    let mut pixels = Vec::with_capacity((WIDTH * HEIGHT * 3) as usize);
    for y in 0..HEIGHT {
        for x in 0..WIDTH {
            pixels.extend(normal_at((f64::from(x) + 0.5) / f64::from(WIDTH), (f64::from(y) + 0.5) / f64::from(HEIGHT)));
        }
    }
    encode(&pixels)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn original_tiles_are_deterministic_valid_rgb_pngs_with_repeatable_joints() {
        for material in Material::ALL.into_iter().filter(|material| material.masonry()) {
            let bytes = png(material).unwrap();
            assert_eq!(bytes, png(material).unwrap());
            let mut reader = png::Decoder::new(std::io::Cursor::new(&bytes)).read_info().unwrap();
            let mut pixels = vec![0; reader.output_buffer_size().unwrap()];
            let info = reader.next_frame(&mut pixels).unwrap();
            assert_eq!((info.width, info.height, info.color_type, info.bit_depth), (WIDTH, HEIGHT, png::ColorType::Rgb, png::BitDepth::Eight));
            assert_eq!(info.buffer_size(), (WIDTH * HEIGHT * 3) as usize);
            let pixel = |x: u32, y: u32| {
                let start = ((y * WIDTH + x) * 3) as usize;
                &pixels[start..start + 3]
            };
            // The horizontal joint crosses the whole repeat. First and last rows are its halves.
            assert_eq!(pixel(WIDTH / 4, 0), pixel(WIDTH / 4, HEIGHT - 1));
            assert_eq!(pixel(0, HEIGHT / 4), pixel(WIDTH - 1, HEIGHT / 4));
            assert_ne!(pixel(WIDTH / 4, HEIGHT / 4), pixel(0, 0), "brick differs from mortar");
            let mortar = [0.42, 0.40, 0.35];
            assert_eq!(linear_at(material, 0.0, 0.25), mortar);
            assert_ne!(linear_at(material, 0.0, 0.75), mortar, "the second course crosses the tile edge at the middle of a brick");
            for (u, v) in [(0.13, 0.21), (0.0, 0.75), (0.71, 0.0)] {
                let a = linear_at(material, u, v);
                let b = linear_at(material, u + 3.0, v - 2.0);
                for (a, b) in a.into_iter().zip(b) {
                    assert!((a - b).abs() < 1e-6, "repeat must not introduce a colour seam");
                }
            }
        }
        assert_eq!(srgb(0.0), 0);
        assert_eq!(srgb(1.0), 255);
        assert_eq!(srgb(0.5), 188, "glTF base colour images carry sRGB, not linear bytes");
    }

    #[test]
    fn joint_normal_map_is_linear_repeatable_and_contains_beveled_edges() {
        let bytes = normal_png().unwrap();
        assert_eq!(bytes, normal_png().unwrap());
        let mut reader = png::Decoder::new(std::io::Cursor::new(bytes)).read_info().unwrap();
        let mut pixels = vec![0; reader.output_buffer_size().unwrap()];
        let info = reader.next_frame(&mut pixels).unwrap();
        assert_eq!((info.width, info.height, info.color_type), (WIDTH, HEIGHT, png::ColorType::Rgb));
        assert_eq!(normal_at(0.0, 0.25), [128, 128, 255]);
        assert_eq!(normal_at(0.25, 0.25), [128, 128, 255]);
        let bevel = normal_at(7.0 / 480.0, 0.25);
        assert!(bevel[0] < 120 && bevel[2] < 255, "the brick bevel tilts the tangent-space normal");
        for (u, v) in [(0.13, 0.21), (7.0 / 480.0, 0.25), (0.71, 0.0)] {
            assert_eq!(normal_at(u, v), normal_at(u + 3.0, v - 2.0));
        }
        for pixel in pixels.as_chunks::<3>().0 {
            let normal: [f64; 3] = std::array::from_fn(|axis| f64::from(pixel[axis]) / 255.0 * 2.0 - 1.0);
            let length = normal.iter().map(|component| component * component).sum::<f64>().sqrt();
            assert!((length - 1.0).abs() < 0.01);
            assert!(normal[2] > 0.0);
        }
    }
}
