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
        plans: Vec::new(),
    });
    Ok(
        json!({"linked": name, "width_px": img.width, "height_px": img.height, "next": "drawing.view to read coordinates, then drawing.register_facade"}),
    )
}

/// A 3 × 5 digit font (and a minus sign), one row per entry, three bits per row.
pub(crate) const DIGITS: [[u8; 5]; 11] = [
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
pub(crate) fn facade_image(
    s: &mut Session,
    facade: &str,
    drawing: &str,
    mm_per_px: f64,
    overlay: bool,
    out: Option<PathBuf>,
    view: trigon_model::build::View,
) -> Result<Value> {
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
        draw_model(doc, b, facade, &fr, &mut Pen { rgb: &mut rgb, width, height, du, dv, h }, view)?;
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

pub(crate) fn digit(d: u32) -> [u8; 5] {
    DIGITS.get(d as usize).copied().unwrap_or([0; 5])
}

pub(crate) fn base64(bytes: &[u8]) -> String {
    const T: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::with_capacity(bytes.len().div_ceil(3) * 4);
    for c in bytes.chunks(3) {
        let n = (u32::from(c[0]) << 16) | (u32::from(*c.get(1).unwrap_or(&0)) << 8) | u32::from(*c.get(2).unwrap_or(&0));
        for (i, shift) in [18, 12, 6, 0].into_iter().enumerate() {
            out.push(if i <= c.len() { char::from(T[((n >> shift) & 63) as usize]) } else { '=' });
        }
    }
    out
}

/// Candidate interior walls read off a registered plan: dark bands between `min_mm` and `max_mm`
/// thick that run at least `min_len` along one of the building's wall directions, inside the outer
/// walls. Returns site-mm centre lines; nothing is added to the model.
pub(crate) fn trace_walls(
    s: &mut Session,
    name: &str,
    level: usize,
    dark: f64,
    min_mm: f64,
    max_mm: f64,
    min_len: f64,
) -> Result<(Vec<trigon_model::InteriorWall>, Vec<trigon_model::Stair>)> {
    let d = find(s, name)?.clone();
    let reg = d
        .plans
        .iter()
        .find(|p| p.levels.contains(&level))
        .cloned()
        .ok_or_else(|| ModelError::NotFound(format!("drawing {name} has no plan registered for level {level}; run drawing.register_plan")))?;
    let img = image(s, &d.path, d.page, d.rotate)?;
    // Paper brightness per 48 px block (about 0.4 m at 1:100), so lighter parts of a sheet read the same.
    let bg = img.background(48, 0.8);
    let b = s.doc.building(None)?.clone();
    let inside = trigon_model::build::inner_ring(&b.footprint, b.wall_thickness + 60.0);
    // Wall directions: the long facade edges' directions, merged within 3°, plus their perpendiculars.
    let n = b.footprint.len();
    let mut dirs: Vec<f64> = Vec::new();
    for i in 0..n {
        let (p, q) = (b.footprint[i], b.footprint[(i + 1) % n]);
        let (dx, dy) = (q[0] - p[0], q[1] - p[1]);
        if (dx * dx + dy * dy).sqrt() < 8000.0 {
            continue;
        }
        for t in [dy.atan2(dx), dy.atan2(dx) + std::f64::consts::FRAC_PI_2] {
            let t = t.rem_euclid(std::f64::consts::PI);
            if !dirs.iter().any(|o| {
                let diff = (o - t).abs();
                diff.min(std::f64::consts::PI - diff) < 3f64.to_radians()
            }) {
                dirs.push(t);
            }
        }
    }
    let contains = |p: [f64; 2]| {
        let mut c = false;
        for i in 0..inside.len() {
            let (a, e) = (inside[i], inside[(i + inside.len() - 1) % inside.len()]);
            if (a[1] > p[1]) != (e[1] > p[1]) && p[0] < (e[0] - a[0]) * (p[1] - a[1]) / (e[1] - a[1]) + a[0] {
                c = !c;
            }
        }
        c
    };
    let step = 25.0;
    // Bridge breaks along a wall up to this many cells (scan noise, crossing lines); doors are wider.
    const GAP_CELLS: usize = 6;
    let (lo, hi) = b
        .footprint
        .iter()
        .fold(([f64::MAX; 2], [f64::MIN; 2]), |(lo, hi), p| ([lo[0].min(p[0]), lo[1].min(p[1])], [hi[0].max(p[0]), hi[1].max(p[1])]));
    let centre = [(lo[0] + hi[0]) / 2.0, (lo[1] + hi[1]) / 2.0];
    let half = ((hi[0] - lo[0]).powi(2) + (hi[1] - lo[1]).powi(2)).sqrt() / 2.0 + 1000.0;
    let cells = (2.0 * half / step).ceil() as usize;
    if cells > 8000 {
        return Err(ModelError::Invalid("the building is too large to trace at 25 mm".into()).into());
    }
    let (min_c, max_c) = ((min_mm / step).round().max(2.0) as usize, (max_mm / step).round() as usize);
    let mut walls = Vec::new();
    let mut stairs: Vec<trigon_model::Stair> = Vec::new();
    for &t in &dirs {
        let (ux, uy) = (t.cos(), t.sin());
        let (vx, vy) = (-uy, ux);
        let site = |i: usize, j: usize| {
            let (u, v) = (-half + (i as f64 + 0.5) * step, -half + (j as f64 + 0.5) * step);
            [centre[0] + u * ux + v * vx, centre[1] + u * uy + v * vy]
        };
        // A wall being followed: its run centres (column, centre cell), mean thickness (negative = outlined), last column.
        struct Track {
            pts: Vec<(usize, f64)>,
            th: f64,
            last: usize,
        }
        let mut open: Vec<Track> = Vec::new();
        let mut done: Vec<Track> = Vec::new();
        // (column, first tread cell, last tread cell)
        let mut treads: Vec<(usize, usize, usize)> = Vec::new();
        for i in 0..cells {
            let is_dark = |p: [f64; 2]| {
                contains(p) && {
                    let [x, y] = reg.to_drawing(p);
                    let paper = bg.sample(x / 48.0, y / 48.0).map_or(200.0, f64::from);
                    img.sample(x, y).is_some_and(|g| f64::from(g) < (paper - dark).min(paper * 0.75))
                }
            };
            // All dark runs across this column: (first cell, length).
            let mut all: Vec<(usize, usize)> = Vec::new();
            let mut j = 0;
            while j < cells {
                if is_dark(site(i, j)) {
                    let s0 = j;
                    while j < cells && is_dark(site(i, j)) {
                        j += 1;
                    }
                    all.push((s0, j - s0));
                } else {
                    j += 1;
                }
            }
            // Stair evidence: six or more thin lines in a row, 200–325 mm apart (treads seen along the flight).
            {
                let thin: Vec<usize> = all.iter().filter(|r| r.1 <= 3).map(|r| r.0).collect();
                let mut k = 0;
                while k < thin.len() {
                    let mut e = k;
                    while e + 1 < thin.len() && (8..=13).contains(&(thin[e + 1] - thin[e])) {
                        e += 1;
                    }
                    if e - k + 1 >= 6 {
                        treads.push((i, thin[k], thin[e]));
                    }
                    k = e + 1;
                }
            }
            // Filled walls: one run as thick as a wall. Outlined walls: two thin lines a wall's thickness apart.
            let mut runs: Vec<(f64, f64)> = Vec::new();
            for (k, &(s0, len)) in all.iter().enumerate() {
                if len >= min_c && len <= max_c {
                    runs.push(((s0 + s0 + len - 1) as f64 / 2.0, len as f64));
                } else if len <= 3
                    && let Some(&(s1, len1)) = all.get(k + 1)
                {
                    let span = s1 + len1 - s0;
                    if len1 <= 3 && s1 > s0 + len && span >= min_c && span <= max_c {
                        runs.push(((s0 + s0 + span - 1) as f64 / 2.0, -(span as f64)));
                    }
                }
            }
            let mut next = Vec::new();
            for mut t in open.drain(..) {
                // Follow the wall from where it was last seen, so a slight angle against the grid doesn't break it.
                let c = t.pts.last().map_or(0.0, |p| p.1);
                if let Some(k) =
                    runs.iter().position(|(rc, rt)| (rc - c).abs() <= 1.0 && (rt.abs() - t.th.abs()).abs() <= 2.0 && rt.signum() == t.th.signum())
                {
                    let (rc, rt) = runs.remove(k);
                    let n = t.pts.len() as f64;
                    t.th = (t.th * n + rt) / (n + 1.0);
                    t.pts.push((i, rc));
                    t.last = i;
                    next.push(t);
                } else if i - t.last <= GAP_CELLS {
                    next.push(t);
                } else {
                    done.push(t);
                }
            }
            next.extend(runs.into_iter().map(|(c, th)| Track { pts: vec![(i, c)], th, last: i }));
            open = next;
        }
        done.extend(open);
        // Flights: columns with treads over overlapping ranges, at least 0.8 m wide.
        let mut groups: Vec<(usize, usize, usize, usize, usize)> = Vec::new(); // (i0, i1, j0, j1, count)
        for (i, j0, j1) in treads {
            if let Some(g) = groups.iter_mut().find(|g| i <= g.1 + 2 && j0 < g.3 && g.2 < j1) {
                g.1 = i;
                g.2 = g.2.min(j0);
                g.3 = g.3.max(j1);
                g.4 += 1;
            } else {
                groups.push((i, i, j0, j1, 1));
            }
        }
        for (i0, i1, j0, j1, count) in groups {
            if i1 - i0 + 1 < 32 || count * 10 < (i1 - i0 + 1) * 6 {
                continue;
            }
            let c = |i: f64, j: f64| {
                let (u, v) = (-half + i * step, -half + j * step);
                [centre[0] + u * ux + v * vx, centre[1] + u * uy + v * vy]
            };
            let (a, b, cc, d) = (i0 as f64, (i1 + 1) as f64, j0 as f64, (j1 + 1) as f64);
            // Outline with its first edge along the flight (v), so plans draw treads across it.
            stairs.push(trigon_model::Stair {
                id: String::new(),
                levels: vec![level],
                outline: vec![c(a, cc), c(a, d), c(b, d), c(b, cc)],
                tread: None,
            });
        }
        for t in done {
            let (first, last, th) = (t.pts.first().map_or(0, |p| p.0), t.last, t.th);
            let len = (last - first + 1) as f64 * step;
            // Outlined walls (negative thickness) need more length: stair treads and fixtures are short pairs of lines.
            if len < min_len || (th < 0.0 && len < min_len.max(1500.0)) {
                continue;
            }
            let th = th.abs();
            // The wall's own line through its run centres (least squares), so its true angle is kept.
            let n = t.pts.len() as f64;
            let (mi, mc) = t.pts.iter().fold((0.0, 0.0), |(a, b), p| (a + p.0 as f64 / n, b + p.1 / n));
            let (sxx, sxy) = t.pts.iter().fold((0.0, 0.0), |(a, b), p| (a + (p.0 as f64 - mi).powi(2), b + (p.0 as f64 - mi) * (p.1 - mc)));
            let slope = if sxx > 0.0 { sxy / sxx } else { 0.0 };
            let at = |i: f64| {
                let (u, v) = (-half + (i + 0.5) * step, -half + (mc + slope * (i - mi) + 0.5) * step);
                [centre[0] + u * ux + v * vx, centre[1] + u * uy + v * vy]
            };
            let (pa, pb) = (at(first as f64 - 0.5), at(last as f64 + 0.5));
            let thickness = (th * step).round();
            walls.push(trigon_model::InteriorWall {
                id: String::new(),
                levels: vec![level],
                a: pa,
                b: pb,
                thickness,
                load_bearing: thickness >= 200.0,
            });
        }
    }
    // Stair flights found in both directions of a grid are the same flight: keep the larger.
    let area = |s: &trigon_model::Stair| trigon_model::doc::polygon_area(&s.outline);
    let centre_of = |s: &trigon_model::Stair| {
        let n = s.outline.len() as f64;
        s.outline.iter().fold([0.0, 0.0], |c, p| [c[0] + p[0] / n, c[1] + p[1] / n])
    };
    stairs.sort_by(|x, y| area(y).total_cmp(&area(x)));
    let mut flights: Vec<trigon_model::Stair> = Vec::new();
    for st in stairs {
        let c = centre_of(&st);
        if !flights.iter().any(|f| inside_poly(&f.outline, c)) {
            flights.push(st);
        }
    }
    // Walls whose middle lies inside a flight are its treads or stringers, not walls.
    let walls: Vec<trigon_model::InteriorWall> = walls
        .into_iter()
        .filter(|w| {
            let m = [(w.a[0] + w.b[0]) / 2.0, (w.a[1] + w.b[1]) / 2.0];
            !flights.iter().any(|f| inside_poly(&shrink(&f.outline, 120.0), m))
        })
        .collect();
    Ok((clean_walls(walls, &b.footprint), flights))
}

pub(crate) fn inside(poly: &[[f64; 2]], p: [f64; 2]) -> bool {
    inside_poly(poly, p)
}

fn inside_poly(poly: &[[f64; 2]], p: [f64; 2]) -> bool {
    let mut c = false;
    for i in 0..poly.len() {
        let (a, e) = (poly[i], poly[(i + poly.len() - 1) % poly.len()]);
        if (a[1] > p[1]) != (e[1] > p[1]) && p[0] < (e[0] - a[0]) * (p[1] - a[1]) / (e[1] - a[1]) + a[0] {
            c = !c;
        }
    }
    c
}

/// A convex outline moved towards its centre by `d` mm (corners pulled in).
fn shrink(poly: &[[f64; 2]], d: f64) -> Vec<[f64; 2]> {
    let n = poly.len().max(1) as f64;
    let c = poly.iter().fold([0.0, 0.0], |c, p| [c[0] + p[0] / n, c[1] + p[1] / n]);
    poly.iter()
        .map(|p| {
            let (dx, dy) = (c[0] - p[0], c[1] - p[1]);
            let l = (dx * dx + dy * dy).sqrt().max(1e-9);
            let k = (d * std::f64::consts::SQRT_2 / l).min(0.5);
            [p[0] + dx * k, p[1] + dy * k]
        })
        .collect()
}

/// Cleans traced walls: keeps each only where its direction matches the nearest long facade (so
/// one wing's grid isn't read into another), merges collinear overlapping pieces, and drops thin
/// short bits (text, treads, fixtures).
fn clean_walls(walls: Vec<trigon_model::InteriorWall>, footprint: &[[f64; 2]]) -> Vec<trigon_model::InteriorWall> {
    let n = footprint.len();
    let long: Vec<([f64; 2], [f64; 2])> = (0..n)
        .map(|i| (footprint[i], footprint[(i + 1) % n]))
        .filter(|(p, q)| ((q[0] - p[0]).powi(2) + (q[1] - p[1]).powi(2)).sqrt() >= 8000.0)
        .collect();
    let dir = |a: [f64; 2], b: [f64; 2]| (b[1] - a[1]).atan2(b[0] - a[0]).rem_euclid(std::f64::consts::PI);
    let square = |t1: f64, t2: f64| {
        let d = (t1 - t2).rem_euclid(std::f64::consts::FRAC_PI_2);
        d.min(std::f64::consts::FRAC_PI_2 - d) < 3f64.to_radians()
    };
    let seg_dist = |p: [f64; 2], a: [f64; 2], b: [f64; 2]| {
        let (dx, dy) = (b[0] - a[0], b[1] - a[1]);
        let l2 = (dx * dx + dy * dy).max(1e-9);
        let t = (((p[0] - a[0]) * dx + (p[1] - a[1]) * dy) / l2).clamp(0.0, 1.0);
        ((a[0] + t * dx - p[0]).powi(2) + (a[1] + t * dy - p[1]).powi(2)).sqrt()
    };
    let mut kept: Vec<trigon_model::InteriorWall> = walls
        .into_iter()
        .filter(|w| {
            let m = [(w.a[0] + w.b[0]) / 2.0, (w.a[1] + w.b[1]) / 2.0];
            let len = ((w.b[0] - w.a[0]).powi(2) + (w.b[1] - w.a[1]).powi(2)).sqrt();
            let near = long.iter().min_by(|x, y| seg_dist(m, x.0, x.1).total_cmp(&seg_dist(m, y.0, y.1)));
            let aligned = near.is_none_or(|(p, q)| square(dir(w.a, w.b), dir(*p, *q)));
            aligned && (len >= 1000.0 || w.thickness >= 200.0)
        })
        .collect();
    // Merge collinear overlapping or touching pieces (same direction, centre lines closer than their half thicknesses).
    let mut merged = true;
    while merged {
        merged = false;
        'outer: for i in 0..kept.len() {
            for j in i + 1..kept.len() {
                let (a, b) = (&kept[i], &kept[j]);
                let (ta, tb) = (dir(a.a, a.b), dir(b.a, b.b));
                let dt = (ta - tb).abs().min(std::f64::consts::PI - (ta - tb).abs());
                if dt > 2f64.to_radians() {
                    continue;
                }
                let (ux, uy) = (ta.cos(), ta.sin());
                let off = |p: [f64; 2]| -(p[0] - a.a[0]) * uy + (p[1] - a.a[1]) * ux;
                if off(b.a).abs().max(off(b.b).abs()) > (a.thickness + b.thickness) / 2.0 {
                    continue;
                }
                let along = |p: [f64; 2]| (p[0] - a.a[0]) * ux + (p[1] - a.a[1]) * uy;
                let (a0, a1) = (along(a.a).min(along(a.b)), along(a.a).max(along(a.b)));
                let (b0, b1) = (along(b.a).min(along(b.b)), along(b.a).max(along(b.b)));
                if b0 > a1 + 150.0 || a0 > b1 + 150.0 {
                    continue;
                }
                let (s0, s1) = (a0.min(b0), a1.max(b1));
                let (la, lb) = (a1 - a0, b1 - b0);
                let o = (off(b.a) + off(b.b)) / 2.0 * lb / (la + lb);
                // The merged centre line: a's line moved by the length-weighted offset along the normal (−uy, ux).
                let base = [a.a[0] - o * uy, a.a[1] + o * ux];
                let p = |t: f64| [base[0] + t * ux, base[1] + t * uy];
                let thickness = if la >= lb { a.thickness } else { b.thickness }.max(a.thickness.min(b.thickness));
                let w = trigon_model::InteriorWall {
                    id: String::new(),
                    levels: a.levels.clone(),
                    a: p(s0),
                    b: p(s1),
                    thickness,
                    load_bearing: a.load_bearing || b.load_bearing,
                };
                kept.remove(j);
                kept[i] = w;
                merged = true;
                break 'outer;
            }
        }
    }
    kept
}
