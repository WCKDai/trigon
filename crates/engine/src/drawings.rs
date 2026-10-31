//! Drawings as underlays: link a scanned page, look at it with a coordinate grid, tie a drawn
//! elevation to a facade, and straighten it into the facade's frame with the model drawn over
//! it, the same way as photos.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use serde_json::{Value, json};
use trigon_drawing::Gray;
use trigon_model::{Drawing, FacadeRegistration, ModelError};

use crate::facade_image::{Pen, default_path, draw_model, facade_frame, write_png};
use crate::{EngineError, Result, Session};

fn err(e: trigon_drawing::DrawingError) -> EngineError {
    EngineError::Io(e.to_string())
}

/// The decoded, upright page; the last one stays cached in the session.
pub(crate) fn image(s: &mut Session, path: &str, page: u32, rotate: u32) -> Result<Arc<Gray>> {
    let key = (path.to_string(), page, rotate);
    if let Some((k, img)) = &s.drawing_cache
        && *k == key
    {
        return Ok(img.clone());
    }
    s.drawing_cache = None;
    let g = Arc::new(trigon_drawing::page_image(Path::new(path), page).map_err(err)?.rotated(rotate).map_err(err)?);
    s.drawing_cache = Some((key, g.clone()));
    Ok(g)
}

fn find<'a>(s: &'a Session, name: &str) -> Result<&'a Drawing> {
    s.doc.drawings.iter().find(|d| d.name == name).ok_or_else(|| {
        let known: Vec<&str> = s.doc.drawings.iter().map(|d| d.name.as_str()).collect();
        ModelError::NotFound(format!("drawing {name} (linked: {})", known.join(", "))).into()
    })
}

pub(crate) fn add(s: &mut Session, path: &Path, page: u32, name: Option<String>, rotate: u32, note: Option<String>) -> Result<Value> {
    let abs = std::fs::canonicalize(path).map_err(|e| EngineError::Io(format!("{}: {e}", path.display())))?;
    let abs = abs.to_string_lossy().into_owned();
    let name = name.unwrap_or_else(|| format!("{} p{page}", path.file_stem().and_then(|n| n.to_str()).unwrap_or("drawing")));
    if s.doc.drawings.iter().any(|d| d.name == name) {
        return Err(ModelError::Invalid(format!("a drawing named {name} is already linked")).into());
    }
    let img = image(s, &abs, page, rotate)?;
    s.doc.drawings.push(Drawing {
        name: name.clone(),
        path: abs,
        page,
        rotate,
        width: img.width,
        height: img.height,
        note: note.unwrap_or_default(),
        facades: Vec::new(),
    });
    Ok(
        json!({"linked": name, "width_px": img.width, "height_px": img.height, "next": "drawing.view to read coordinates, then drawing.register_facade"}),
    )
}

/// A 3 × 5 digit font (and a minus sign), one row per entry, three bits per row.
const DIGITS: [[u8; 5]; 11] = [
    [7, 5, 5, 5, 7],
    [2, 6, 2, 2, 7],
    [7, 1, 7, 4, 7],
    [7, 1, 7, 1, 7],
    [5, 5, 7, 1, 1],
    [7, 4, 7, 1, 7],
    [7, 4, 7, 5, 7],
    [7, 1, 1, 1, 1],
    [7, 5, 7, 5, 7],
    [7, 5, 7, 1, 7],
    [0, 0, 7, 0, 0],
];

struct Canvas {
    w: u32,
    h: u32,
    rgb: Vec<u8>,
}

impl Canvas {
    fn put(&mut self, x: i64, y: i64, c: [u8; 3]) {
        if x < 0 || y < 0 || x >= i64::from(self.w) || y >= i64::from(self.h) {
            return;
        }
        let k = ((y as usize) * self.w as usize + x as usize) * 3;
        if let Some(d) = self.rgb.get_mut(k..k + 3) {
            d.copy_from_slice(&c);
        }
    }

    /// A number at (x, y), 2× scaled digits on a white box.
    fn label(&mut self, x: i64, y: i64, n: i64) {
        let text = n.to_string();
        let width = text.len() as i64 * 8 + 2;
        for dy in 0..14 {
            for dx in 0..width {
                self.put(x + dx, y + dy, [255, 255, 255]);
            }
        }
        for (i, ch) in text.chars().enumerate() {
            let glyph = match ch {
                '-' => DIGITS[10],
                c => match c.to_digit(10).and_then(|d| DIGITS.get(d as usize)) {
                    Some(g) => *g,
                    None => continue,
                },
            };
            for (row, bits) in glyph.iter().enumerate() {
                for col in 0..3 {
                    if bits & (4 >> col) != 0 {
                        for (sx, sy) in [(0, 0), (1, 0), (0, 1), (1, 1)] {
                            self.put(x + 2 + i as i64 * 8 + col * 2 + sx, y + 2 + row as i64 * 2 + sy, [200, 0, 0]);
                        }
                    }
                }
            }
        }
    }
}

/// A window of the drawing, reduced to at most `max_px` per side, with grid lines every `grid`
/// drawing pixels labelled with their drawing coordinates.
pub(crate) fn view(s: &mut Session, name: &str, region: Option<[f64; 4]>, max_px: u32, grid: Option<f64>, out: Option<PathBuf>) -> Result<Value> {
    let d = find(s, name)?.clone();
    let img = image(s, &d.path, d.page, d.rotate)?;
    let [x0, y0, x1, y1] = region.unwrap_or([0.0, 0.0, f64::from(img.width), f64::from(img.height)]);
    if [x0, y0, x1, y1].iter().any(|v| !v.is_finite()) {
        return Err(ModelError::Invalid("region must be four finite numbers [x0, y0, x1, y1]".into()).into());
    }
    let (x0, y0) = (x0.clamp(0.0, f64::from(img.width)) as u32, y0.clamp(0.0, f64::from(img.height)) as u32);
    let (x1, y1) = (x1.clamp(0.0, f64::from(img.width)) as u32, y1.clamp(0.0, f64::from(img.height)) as u32);
    if x1 <= x0 + 1 || y1 <= y0 + 1 {
        return Err(ModelError::Invalid("the region is empty".into()).into());
    }
    let max_px = max_px.clamp(200, 4000);
    let (rw, rh) = (x1 - x0, y1 - y0);
    let f = rw.max(rh).div_ceil(max_px).max(1);
    let (w, h) = ((rw / f).max(1), (rh / f).max(1));
    let mut c = Canvas { w, h, rgb: Vec::with_capacity((w * h * 3) as usize) };
    for j in 0..h {
        for i in 0..w {
            let mut sum = 0u32;
            for dy in 0..f {
                for dx in 0..f {
                    sum += u32::from(img.get(x0 + i * f + dx, y0 + j * f + dy));
                }
            }
            let v = (sum / (f * f)) as u8;
            c.rgb.extend([v, v, v]);
        }
    }
    let span = f64::from(rw.max(rh));
    let step = grid.filter(|g| g.is_finite() && *g >= 1.0).unwrap_or_else(|| {
        [10.0, 20.0, 50.0, 100.0, 200.0, 250.0, 500.0, 1000.0, 2000.0, 5000.0].into_iter().find(|g| span / g <= 12.0).unwrap_or(10000.0)
    });
    if span / step > 400.0 {
        return Err(ModelError::Invalid(format!("grid {step} px is too fine for a {span} px region")).into());
    }
    let to_out = |v: f64, o: u32| ((v - f64::from(o)) / f64::from(f)).round() as i64;
    let mut gx = (f64::from(x0) / step).ceil() * step;
    while gx < f64::from(x1) {
        let x = to_out(gx, x0);
        for y in 0..i64::from(h) {
            if y % 6 < 4 {
                c.put(x, y, [230, 60, 60]);
            }
        }
        c.label(x + 2, 2, gx as i64);
        gx += step;
    }
    let mut gy = (f64::from(y0) / step).ceil() * step;
    while gy < f64::from(y1) {
        let y = to_out(gy, y0);
        for x in 0..i64::from(w) {
            if x % 6 < 4 {
                c.put(x, y, [230, 60, 60]);
            }
        }
        c.label(2, y + 2, gy as i64);
        gy += step;
    }
    let path = out.unwrap_or_else(|| default_path(s.path.as_deref(), "drawing", &format!("{name}-{x0}-{y0}")));
    write_png(&path, w, h, &c.rgb)?;
    Ok(json!({
        "image_path": path,
        "region": [x0, y0, x1, y1],
        "drawing_px_per_image_px": f,
        "grid_px": step,
        "how_to_read": "Grid lines are labelled with drawing pixel coordinates (x along the top, y down the left). \
                        Drawing x = region x0 + image x × drawing_px_per_image_px; the same for y.",
    }))
}

pub(crate) fn register(s: &mut Session, name: &str, reg: FacadeRegistration) -> Result<Value> {
    reg.validate()?;
    let b = s
        .doc
        .buildings
        .iter()
        .find(|b| b.facades.iter().any(|f| f.id == reg.facade))
        .ok_or_else(|| ModelError::NotFound(format!("facade {}", reg.facade)))?;
    let fr = facade_frame(b, &reg.facade)?;
    let d = s.doc.drawings.iter_mut().find(|d| d.name == name).ok_or_else(|| ModelError::NotFound(format!("drawing {name}")))?;
    let px_per_m_x = (reg.x_end - reg.x_start).abs() / fr.length * 1000.0;
    let [[ya, va], [yb, vb]] = reg.y_levels;
    let px_per_m_y = (yb - ya).abs() / (vb - va).abs() * 1000.0;
    d.facades.retain(|r| r.facade != reg.facade);
    d.facades.push(reg.clone());
    Ok(json!({
        "drawing": name,
        "facade": reg.facade,
        "px_per_m_along": (px_per_m_x * 100.0).round() / 100.0,
        "px_per_m_up": (px_per_m_y * 100.0).round() / 100.0,
        "scale_check": "on a true-scale elevation both numbers are equal; a large difference means a wrong end or level",
    }))
}

/// The registered elevation straightened into the facade's frame, with the model over it.
pub(crate) fn facade_image(s: &mut Session, facade: &str, drawing: &str, mm_per_px: f64, overlay: bool, out: Option<PathBuf>) -> Result<Value> {
    let d = find(s, drawing)?.clone();
    let reg =
        d.facades.iter().find(|r| r.facade == facade).cloned().ok_or_else(|| {
            ModelError::NotFound(format!("facade {facade} isn't registered on drawing {drawing}; run drawing.register_facade first"))
        })?;
    let img = image(s, &d.path, d.page, d.rotate)?;
    let doc = &s.doc;
    let b =
        doc.buildings.iter().find(|b| b.facades.iter().any(|f| f.id == facade)).ok_or_else(|| ModelError::NotFound(format!("facade {facade}")))?;
    let fr = facade_frame(b, facade)?;
    let h = b.height();
    let mm_per_px = trigon_model::checked("mm_per_px", mm_per_px, 10.0, 500.0)?;
    let width = (fr.length / mm_per_px).ceil().clamp(1.0, 4000.0) as u32;
    let height = (h / mm_per_px).ceil().clamp(1.0, 4000.0) as u32;
    let (du, dv) = (fr.length / f64::from(width), h / f64::from(height));
    let mut rgb = vec![255u8; (width * height * 3) as usize];
    let mut covered = 0usize;
    for j in 0..height {
        let v = h - (f64::from(j) + 0.5) * dv;
        for i in 0..width {
            let u = (f64::from(i) + 0.5) * du;
            let [x, y] = reg.to_drawing(u, v, fr.length);
            if let Some(g) = img.sample(x, y) {
                let k = ((j * width + i) * 3) as usize;
                if let Some(dst) = rgb.get_mut(k..k + 3) {
                    dst.copy_from_slice(&[g, g, g]);
                    covered += 1;
                }
            }
        }
    }
    if overlay {
        draw_model(doc, b, facade, &fr, &mut Pen { rgb: &mut rgb, width, height, du, dv, h })?;
    }
    let path = out.unwrap_or_else(|| default_path(s.path.as_deref(), facade, &format!("drawing-{drawing}")));
    write_png(&path, width, height, &rgb)?;
    Ok(json!({
        "image_path": path,
        "drawing": drawing,
        "width_px": width,
        "height_px": height,
        "mm_per_px": mm_per_px,
        "facade_length_mm": fr.length.round(),
        "facade_height_mm": h.round(),
        "drawing_coverage": (covered as f64 / f64::from(width * height) * 1000.0).round() / 1000.0,
    }))
}
