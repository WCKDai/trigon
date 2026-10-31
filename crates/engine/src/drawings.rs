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

/// Facades at least this long set the wall directions the tracer follows (a block's grid).
const MIN_FACADE: f64 = 3000.0;

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
    // Sample paper over a region wider than the largest wall. A small tile wholly inside
    // thick masonry would otherwise classify the wall itself as the paper background.
    let paper_block = (3.0 * max_mm * reg.px_per_m() / 1000.0).ceil().clamp(128.0, 2048.0) as u32;
    let bg = img.background(paper_block, 0.8);
    let b = s.doc.building(None)?.clone();
    let inside = trigon_model::build::inner_ring(&b.footprint, b.wall_thickness + 60.0);
    // Wall directions: the building's grids, i.e. facade directions (modulo 90°) clustered within 2.5°, weighted
    // by facade length; a grid needs at least 20 m of facade. Each grid is traced along both of its axes.
    let n = b.footprint.len();
    let mut clusters: Vec<(f64, f64)> = Vec::new(); // (angle mod 90° in radians, total length)
    let quarter = std::f64::consts::FRAC_PI_2;
    for i in 0..n {
        let (p, q) = (b.footprint[i], b.footprint[(i + 1) % n]);
        let (dx, dy) = (q[0] - p[0], q[1] - p[1]);
        let len = (dx * dx + dy * dy).sqrt();
        if len < MIN_FACADE {
            continue;
        }
        let t = dy.atan2(dx).rem_euclid(quarter);
        let close = |o: f64| {
            let d = (o - t).abs();
            d.min(quarter - d) < 2.5f64.to_radians()
        };
        match clusters.iter_mut().find(|c| close(c.0)) {
            Some(c) => {
                // Length-weighted mean, unwrapped around the 0/90° seam.
                let mut tt = t;
                if tt - c.0 > quarter / 2.0 {
                    tt -= quarter;
                } else if c.0 - tt > quarter / 2.0 {
                    tt += quarter;
                }
                c.0 = ((c.0 * c.1 + tt * len) / (c.1 + len)).rem_euclid(quarter);
                c.1 += len;
            }
            None => clusters.push((t, len)),
        }
    }
    let grids: Vec<f64> = clusters.iter().filter(|c| c.1 >= 20_000.0).map(|c| c.0).collect();
    let dirs: Vec<f64> = grids.iter().flat_map(|&t| [t, t + quarter]).collect();
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
                    let paper = bg.sample(x / f64::from(paper_block), y / f64::from(paper_block)).map_or(200.0, f64::from);
                    img.sample(x, y).is_some_and(|g| f64::from(g) < (paper - dark).min(paper * 0.75))
                }
            };
            // Walls filled with grey (not black): lighter, so only accepted as thick runs.
            let is_grey = |p: [f64; 2]| {
                contains(p) && {
                    let [x, y] = reg.to_drawing(p);
                    let paper = bg.sample(x / f64::from(paper_block), y / f64::from(paper_block)).map_or(200.0, f64::from);
                    img.sample(x, y).is_some_and(|g| f64::from(g) < paper - dark * 0.5)
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
            // Grey-filled walls: runs of the lighter threshold at least 150 mm thick that the dark pass didn't see as a wall.
            let grey_min = ((150.0 / step).round() as usize).max(min_c);
            let mut j = 0;
            let mut grey_runs: Vec<(f64, f64)> = Vec::new();
            while j < cells {
                if is_grey(site(i, j)) {
                    let s0 = j;
                    while j < cells && is_grey(site(i, j)) {
                        j += 1;
                    }
                    let len = j - s0;
                    if len >= grey_min && len <= max_c {
                        grey_runs.push(((s0 + j - 1) as f64 / 2.0, len as f64));
                    }
                } else {
                    j += 1;
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
            for g in grey_runs {
                if !runs.iter().any(|r| (r.0 - g.0).abs() <= g.1 / 2.0 + 1.0) {
                    runs.push(g);
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
                kind: trigon_model::StairKind::Straight,
                landing: None,
                well: None,
                rounded: true,
                winders: None,
                turn_depths: None,
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
    let inner = trigon_model::build::inner_ring(&b.footprint, b.wall_thickness);
    Ok((snap_ends(clean_walls(walls, &grids), &inner), flights))
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

/// Cleans traced walls: keeps those square to one of the building's grids (each wall keeps the angle
/// fitted to its own runs), merges collinear overlapping pieces (one wall traced along two grids comes
/// out twice), and drops thin short bits (text, treads, fixtures).
fn clean_walls(walls: Vec<trigon_model::InteriorWall>, grids: &[f64]) -> Vec<trigon_model::InteriorWall> {
    let dir = |a: [f64; 2], b: [f64; 2]| (b[1] - a[1]).atan2(b[0] - a[0]).rem_euclid(std::f64::consts::PI);
    let square = |t1: f64, t2: f64| {
        let d = (t1 - t2).rem_euclid(std::f64::consts::FRAC_PI_2);
        d.min(std::f64::consts::FRAC_PI_2 - d) < 3f64.to_radians()
    };
    let mut kept: Vec<trigon_model::InteriorWall> = walls
        .into_iter()
        .filter(|w| {
            let len = ((w.b[0] - w.a[0]).powi(2) + (w.b[1] - w.a[1]).powi(2)).sqrt();
            grids.iter().any(|g| square(dir(w.a, w.b), *g)) && (len >= 1000.0 || w.thickness >= 200.0)
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

/// Joins wall ends to the walls they meet: each end moves along its wall to the nearest centre line
/// of another wall, or the inside face of the outer walls, if that is at most 600 mm beyond the end
/// (or 150 mm short of it) and the meeting point lies on that wall.
fn snap_ends(mut walls: Vec<trigon_model::InteriorWall>, inner: &[[f64; 2]]) -> Vec<trigon_model::InteriorWall> {
    let n = inner.len();
    let mut targets: Vec<([f64; 2], [f64; 2], f64)> = (0..n).map(|i| (inner[i], inner[(i + 1) % n], 0.0)).collect();
    targets.extend(walls.iter().map(|w| (w.a, w.b, w.thickness / 2.0)));
    let outer_count = n;
    for (k, w) in walls.iter_mut().enumerate() {
        for end in 0..2 {
            let (p, other) = if end == 0 { (w.a, w.b) } else { (w.b, w.a) };
            let (dx, dy) = (p[0] - other[0], p[1] - other[1]);
            let len = (dx * dx + dy * dy).sqrt();
            if len < 1.0 {
                continue;
            }
            let d = [dx / len, dy / len];
            let mut best: Option<(f64, [f64; 2])> = None;
            for (ti, (a, b, half)) in targets.iter().enumerate() {
                if ti >= outer_count && ti - outer_count == k {
                    continue;
                }
                let e = [b[0] - a[0], b[1] - a[1]];
                let el = (e[0] * e[0] + e[1] * e[1]).sqrt();
                let denom = d[0] * e[1] - d[1] * e[0];
                if el < 1.0 || denom.abs() / el < 0.5 {
                    continue; // parallel-ish: not a junction
                }
                // p + t·d = a + s·e
                let t = ((a[0] - p[0]) * e[1] - (a[1] - p[1]) * e[0]) / denom;
                let sp = ((a[0] - p[0]) * d[1] - (a[1] - p[1]) * d[0]) / denom;
                let tol = (200.0 + half) / el;
                // Stop at the near face of a wall, at the centre line of a thin one.
                let t_face = t - half / (denom.abs() / el);
                if !(-150.0..=600.0).contains(&t_face) || sp < -tol || sp > 1.0 + tol {
                    continue;
                }
                if best.is_none_or(|(bt, _)| t_face.abs() < bt.abs()) {
                    best = Some((t_face, [p[0] + d[0] * t_face, p[1] + d[1] * t_face]));
                }
            }
            if let Some((_, q)) = best {
                if end == 0 {
                    w.a = q;
                } else {
                    w.b = q;
                }
            }
        }
    }
    walls.retain(|w| ((w.b[0] - w.a[0]).powi(2) + (w.b[1] - w.a[1]).powi(2)).sqrt() >= 200.0);
    walls
}

/// What `fit_stair` measured.
/// A known stair design to match (all stairwells of one building usually share it).
#[derive(Debug, Clone, Copy, serde::Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub(crate) struct StairTemplate {
    /// Tread lines in one flight (including the first nosing).
    pub treads: usize,
    pub tread_mm: f64,
    pub width_mm: f64,
    pub well_mm: f64,
}

pub(crate) struct StairFit {
    pub stair: trigon_model::Stair,
    /// Lines found across the flights, mm from p0's side, with their strength.
    pub lines_across: Vec<(f64, f64)>,
    pub treads: usize,
    pub tread_mm: f64,
    pub length_mm: f64,
    pub width_mm: f64,
    pub well_mm: f64,
}

/// Local maxima of a profile above `min`, at least `gap` apart (strongest kept).
fn peaks(p: &[f64], min: f64, gap: usize) -> Vec<usize> {
    let mut c: Vec<usize> = (1..p.len().saturating_sub(1)).filter(|&i| p[i] >= min && p[i] >= p[i - 1] && p[i] >= p[i + 1]).collect();
    c.sort_by(|a, b| p[*b].total_cmp(&p[*a]));
    let mut kept: Vec<usize> = Vec::new();
    for i in c {
        if kept.iter().all(|k| k.abs_diff(i) >= gap) {
            kept.push(i);
        }
    }
    kept.sort_unstable();
    kept
}

/// Measures a U-turn staircase on a registered plan from a rough box `[p0, p1, p2, p3]` in drawing
/// pixels (p0→p1 along the flights towards the half landing, p1→p2 across): the tread lines give
/// the run and tread depth, the stringers and the well's lines the width and the well.
pub(crate) fn fit_stair(s: &mut Session, name: &str, level: usize, rough: [[f64; 2]; 4], template: Option<StairTemplate>) -> Result<StairFit> {
    let d = find(s, name)?.clone();
    let reg = d
        .plans
        .iter()
        .find(|p| p.levels.contains(&level))
        .cloned()
        .ok_or_else(|| ModelError::NotFound(format!("drawing {name} has no plan registered for level {level}; run drawing.register_plan")))?;
    let img = image(s, &d.path, d.page, d.rotate)?;
    let bg = img.background(48, 0.8);
    let [p0, p1, _, p3] = rough;
    let (rv, av) = ([p1[0] - p0[0], p1[1] - p0[1]], [p3[0] - p0[0], p3[1] - p0[1]]);
    let (len, wid) = ((rv[0] * rv[0] + rv[1] * rv[1]).sqrt(), (av[0] * av[0] + av[1] * av[1]).sqrt());
    if !(len >= 40.0 && wid >= 40.0 && len <= 3000.0 && wid <= 3000.0) {
        return Err(ModelError::Invalid("the rough box must be 40 to 3000 drawing px on each side".into()).into());
    }
    let (ru, au) = ([rv[0] / len, rv[1] / len], [av[0] / wid, av[1] / wid]);
    let m = (0.25 * len.max(wid)).round();
    let (ns, nt) = ((len + 2.0 * m) as usize, (wid + 2.0 * m) as usize);
    let dark = |sx: f64, ty: f64| {
        let (x, y) = (p0[0] + ru[0] * sx + au[0] * ty, p0[1] + ru[1] * sx + au[1] * ty);
        let paper = bg.sample(x / 48.0, y / 48.0).map_or(200.0, f64::from).max(1.0);
        img.sample(x, y).map_or(0.0, |g| ((paper - f64::from(g)) / paper).max(0.0))
    };
    let grid: Vec<Vec<f64>> = (0..ns).map(|i| (0..nt).map(|j| dark(i as f64 - m, j as f64 - m)).collect()).collect();
    let px_mm0 = 1000.0 / reg.px_per_m();
    if let Some(tp) = template {
        // Slide the template's lines (treads in both flights, well lines, outer edges) over the box.
        if !(2..=40).contains(&tp.treads)
            || !(150.0..=400.0).contains(&tp.tread_mm)
            || !(600.0..=5000.0).contains(&tp.width_mm)
            || !(0.0..=600.0).contains(&tp.well_mm)
        {
            return Err(ModelError::Invalid("template out of range".into()).into());
        }
        let (g, w, wl) = (tp.tread_mm / px_mm0, tp.width_mm / px_mm0, tp.well_mm / px_mm0);
        let run_px = g * (tp.treads - 1) as f64;
        let flight = (w - wl) / 2.0;
        let mut samples: Vec<(f64, f64)> = Vec::new();
        for k in 0..tp.treads {
            let sx = k as f64 * g;
            let mut t = 0.0;
            while t <= w {
                if t <= flight || t >= flight + wl {
                    samples.push((sx, t));
                }
                t += 2.0;
            }
        }
        let mut sx = 0.0;
        while sx <= run_px {
            for t in [0.0, flight, flight + wl, w] {
                samples.push((sx, t));
            }
            sx += 2.0;
        }
        let cell = |i: f64, j: f64| grid.get(i.round() as usize).and_then(|r| r.get(j.round() as usize)).copied().unwrap_or(0.0);
        let mut best = (f64::MIN, 0.0, 0.0);
        let (si_max, tj_max) = (ns as f64 - run_px - 1.0, nt as f64 - w - 1.0);
        let mut i0 = 0.0;
        while i0 <= si_max {
            let mut j0 = 0.0;
            while j0 <= tj_max {
                let score: f64 = samples.iter().map(|(a, b)| cell(i0 + a, j0 + b)).sum::<f64>() / samples.len() as f64;
                if score > best.0 {
                    best = (score, i0, j0);
                }
                j0 += 1.0;
            }
            i0 += 1.0;
        }
        let (_, i0, j0) = best;
        let at = |i: f64, j: f64| {
            let (sx, ty) = (i - m, j - m);
            reg.to_site([p0[0] + ru[0] * sx + au[0] * ty, p0[1] + ru[1] * sx + au[1] * ty])
        };
        let outline = vec![at(i0, j0), at(i0 + run_px, j0), at(i0 + run_px, j0 + w), at(i0, j0 + w)];
        let stair = trigon_model::Stair {
            id: String::new(),
            levels: vec![level],
            outline,
            tread: Some(tp.tread_mm),
            kind: trigon_model::StairKind::UTurn,
            landing: None,
            well: Some(tp.well_mm),
            rounded: true,
            winders: None,
            turn_depths: None,
        };
        return Ok(StairFit {
            stair,
            lines_across: vec![(0.0, best.0)],
            treads: tp.treads,
            tread_mm: tp.tread_mm,
            length_mm: run_px * px_mm0,
            width_mm: tp.width_mm,
            well_mm: tp.well_mm,
        });
    }
    // Treads: lines across the flights, so peaks of darkness averaged across the middle of each flight.
    let (t_lo, t_hi) = ((m + 0.08 * wid) as usize, (m + 0.92 * wid) as usize);
    let run: Vec<f64> = grid.iter().map(|row| row[t_lo..t_hi.min(nt)].iter().sum::<f64>() / (t_hi - t_lo).max(1) as f64).collect();
    let base = {
        let mut v = run.clone();
        v.sort_by(f64::total_cmp);
        v.get(v.len() / 2).copied().unwrap_or(0.0)
    };
    let pk = peaks(&run, base + 0.08, 12);
    // The longest chain of evenly spaced peaks (150–380 mm apart) is the flight.
    let px_mm = 1000.0 / reg.px_per_m();
    let (gmin, gmax) = (150.0 / px_mm, 380.0 / px_mm);
    let mut best: Vec<usize> = Vec::new();
    for (a, &start) in pk.iter().enumerate() {
        let mut chain = vec![start];
        let mut step: Option<f64> = None;
        for &q in &pk[a + 1..] {
            let last = *chain.last().unwrap_or(&start);
            let gap = (q - last) as f64;
            if !(gmin..=gmax * 1.1).contains(&gap) {
                if gap > gmax * 1.1 {
                    break;
                }
                continue;
            }
            if step.is_none_or(|g| (gap - g).abs() <= 0.3 * g) {
                step = Some(step.map_or(gap, |g| (g * (chain.len() - 1) as f64 + gap) / chain.len() as f64));
                chain.push(q);
            }
        }
        if chain.len() > best.len() {
            best = chain;
        }
    }
    if best.len() < 4 {
        return Err(ModelError::NotFound(format!("only {} evenly spaced tread lines found; adjust the box", best.len())).into());
    }
    let (sf, sl) = (best[0], best[best.len() - 1]);
    // Stringers and the well's lines: peaks across, averaged over the treads' run.
    let across: Vec<f64> = (0..nt).map(|j| (sf..=sl).map(|i| grid[i][j]).sum::<f64>() / (sl - sf + 1) as f64).collect();
    let abase = {
        let mut v = across.clone();
        v.sort_by(f64::total_cmp);
        v.get(v.len() / 2).copied().unwrap_or(0.0)
    };
    let ap = peaks(&across, abase + 0.15, 4);
    // The well: the strongest pair of lines 60–420 mm apart near the middle of the box.
    let mm = |j: usize| (j as f64 - m) * px_mm;
    let centre = wid * px_mm / 2.0;
    let strength = |j: usize| across.get(j).copied().unwrap_or(0.0);
    let mut well: Option<(usize, usize, f64)> = None;
    for (x, &a) in ap.iter().enumerate() {
        for &b in &ap[x + 1..] {
            let gap = mm(b) - mm(a);
            let mid = (mm(a) + mm(b)) / 2.0;
            if !(60.0..=420.0).contains(&gap) || (mid - centre).abs() > 0.3 * wid * px_mm {
                continue;
            }
            let score = strength(a) + strength(b) - (mid - centre).abs() / (wid * px_mm);
            if well.is_none_or(|w| score > w.2) {
                well = Some((a, b, score));
            }
        }
    }
    let (wa, wb) = match well {
        Some((a, b, _)) => (a, b),
        None => return Err(ModelError::NotFound("no stair well (two lines near the middle) found; adjust the box".into()).into()),
    };
    // The flights' outer edges: a pair of lines either side of the well at matching distances (0.5–1.6 m).
    let mut edges: Option<(usize, usize, f64)> = None;
    for &l in ap.iter().filter(|&&j| j < wa) {
        for &r in ap.iter().filter(|&&j| j > wb) {
            let (dl, dr) = (mm(wa) - mm(l), mm(r) - mm(wb));
            if !(500.0..=1600.0).contains(&dl) || !(500.0..=1600.0).contains(&dr) {
                continue;
            }
            let score = strength(l) + strength(r) - (dl - dr).abs() / 200.0;
            if edges.is_none_or(|e| score > e.2) {
                edges = Some((l, r, score));
            }
        }
    }
    let (j0, j1) = match edges {
        Some((l, r, _)) => (l, r),
        None => return Err(ModelError::NotFound("no matching flight edges either side of the well; adjust the box".into()).into()),
    };
    let well_px = (wb - wa) as f64;
    let at = |i: f64, j: f64| {
        let (sx, ty) = (i - m, j - m);
        reg.to_site([p0[0] + ru[0] * sx + au[0] * ty, p0[1] + ru[1] * sx + au[1] * ty])
    };
    let (sf, sl, j0, j1) = (sf as f64, sl as f64, j0 as f64, j1 as f64);
    let outline = vec![at(sf, j0), at(sl, j0), at(sl, j1), at(sf, j1)];
    let tread_mm = (sl - sf) / (best.len() - 1) as f64 * px_mm;
    let stair = trigon_model::Stair {
        id: String::new(),
        levels: vec![level],
        outline,
        tread: Some(tread_mm),
        kind: trigon_model::StairKind::UTurn,
        landing: None,
        well: Some(well_px * px_mm),
        rounded: true,
        winders: None,
        turn_depths: None,
    };
    let lines_across = ap.iter().map(|&j| (((j as f64) - m) * px_mm, across[j])).collect();
    Ok(StairFit {
        stair,
        lines_across,
        treads: best.len(),
        tread_mm,
        length_mm: (sl - sf) * px_mm,
        width_mm: (j1 - j0) * px_mm,
        well_mm: well_px * px_mm,
    })
}

/// A test for drawn line work on the plan registered for `level`: the darkest scan pixel within a
/// 50 mm cell around a site point, compared with the local paper.
pub(crate) fn ink_for(s: &mut Session, level: usize, contrast: f64) -> Result<Option<impl Fn([f64; 2]) -> bool + use<>>> {
    let Some(d) = s.doc.drawings.iter().find(|d| d.plans.iter().any(|p| p.levels.contains(&level))).cloned() else { return Ok(None) };
    let Some(reg) = d.plans.iter().find(|p| p.levels.contains(&level)).cloned() else { return Ok(None) };
    let img = image(s, &d.path, d.page, d.rotate)?;
    let bg = std::sync::Arc::new(img.background(48, 0.8));
    let half = (25.0 * reg.px_per_m() / 1000.0).max(1.0);
    Ok(Some(move |p: [f64; 2]| {
        let [x, y] = reg.to_drawing(p);
        let paper = bg.sample(x / 48.0, y / 48.0).map_or(200.0, f64::from);
        let (x0, x1, y0, y1) =
            ((x - half).floor().max(0.0) as u32, (x + half).ceil() as u32, (y - half).floor().max(0.0) as u32, (y + half).ceil() as u32);
        let mut darkest = 255u8;
        for yy in y0..y1.min(img.height) {
            for xx in x0..x1.min(img.width) {
                darkest = darkest.min(img.get(xx, yy));
            }
        }
        f64::from(darkest) < paper - contrast
    }))
}
