//! Floor plans: a storey cut 1 m above its floor, drawn from the model (walls, windows, door
//! swings, balconies with their size, phase colours) over the registered archive plan, as a
//! PNG (for agents) or an SVG (vector, for drawing sets and offers).

use std::fmt::Write as _;
use std::path::PathBuf;

use serde_json::{Value, json};
use trigon_model::build::{FacadeFrame, Status, View, build_view};
use trigon_model::{ComponentKind, Document, ModelError};

use crate::facade_image::write_png;
use crate::{EngineError, Result, Session};

const CUT: f64 = 1000.0;
const WALL: [u8; 3] = [70, 70, 70];
const LINE: [u8; 3] = [30, 30, 30];
const NEW: [u8; 3] = [40, 150, 30];
const DEMOLISHED: [u8; 3] = [220, 30, 30];

enum Prim {
    Poly { pts: Vec<[f64; 2]>, fill: [u8; 3] },
    Line { a: [f64; 2], b: [f64; 2], color: [u8; 3], width: f64, dashed: bool },
    Text { at: [f64; 2], text: String, color: [u8; 3], size: f64 },
}

fn status_color(s: Status, normal: [u8; 3]) -> [u8; 3] {
    match s {
        Status::Existing => normal,
        Status::New => NEW,
        Status::Demolished => DEMOLISHED,
    }
}

fn pt(fr: &FacadeFrame, u: f64, w: f64) -> [f64; 2] {
    let p = fr.point(u, w, 0.0);
    [p[0], p[1]]
}

/// The storey's cut as primitives in site mm.
fn scene(doc: &Document, level: usize, view: View) -> Result<(Vec<Prim>, String)> {
    let mut prims = Vec::new();
    let mut title = String::new();
    for b in build_view(doc, view)? {
        let storey = b.storeys.get(level).ok_or_else(|| ModelError::Invalid(format!("level {level} doesn't exist")))?;
        title = format!("{} — {}", b.name, storey.name);
        let cut = storey.elevation + CUT;
        for w in &b.walls {
            let fr = &w.frame;
            let d = w.thickness;
            // Openings crossing the cut, by position along the wall.
            let mut cuts: Vec<_> = w.openings.iter().filter(|o| o.rect.v0 <= cut && o.rect.v1 >= cut).collect();
            cuts.sort_by(|x, y| x.rect.u0.total_cmp(&y.rect.u0));
            let [a, e, qe, qs] = w.plan;
            let mut start = (a, qs);
            for o in &cuts {
                let (u0, u1) = (o.rect.u0, o.rect.u1);
                prims.push(Prim::Poly { pts: vec![start.0, pt(fr, u0, 0.0), pt(fr, u0, d), start.1], fill: WALL });
                start = (pt(fr, u1, 0.0), pt(fr, u1, d));
                let color = status_color(o.status, LINE);
                let kind = doc.component(&o.fill.type_id).map(|t| t.kind.clone()).ok();
                // Window: frame and glass lines; door: leaf and swing; a door + window unit: both.
                let window = |prims: &mut Vec<Prim>, wu0: f64, wu1: f64| {
                    for depth in [d * 0.35, d * 0.55] {
                        prims.push(Prim::Line { a: pt(fr, wu0, depth), b: pt(fr, wu1, depth), color, width: 25.0, dashed: false });
                    }
                    prims.push(Prim::Line { a: pt(fr, wu0, 0.0), b: pt(fr, wu1, 0.0), color, width: 15.0, dashed: false });
                };
                // Doors as a leaf line in the closed position: no swing, no opening direction.
                let door = |prims: &mut Vec<Prim>, hinge: f64, other: f64| {
                    prims.push(Prim::Line { a: pt(fr, hinge, d * 0.5), b: pt(fr, other, d * 0.5), color, width: 45.0, dashed: false });
                };
                match kind {
                    Some(ComponentKind::Window { .. } | ComponentKind::FixedWindow) => window(&mut prims, u0, u1),
                    Some(ComponentKind::DoorWindow { door_width, door_side, .. }) => {
                        if door_side == trigon_model::component::Side::Left {
                            door(&mut prims, u0, u0 + door_width);
                            window(&mut prims, u0 + door_width, u1);
                        } else {
                            door(&mut prims, u1, u1 - door_width);
                            window(&mut prims, u0, u1 - door_width);
                        }
                    }
                    Some(
                        ComponentKind::FrenchDoor { leaves, .. }
                        | ComponentKind::FrenchBalcony { leaves, .. }
                        | ComponentKind::Balcony { leaves, .. },
                    ) if leaves >= 2 => {
                        let m = (u0 + u1) / 2.0;
                        door(&mut prims, u0, m);
                        door(&mut prims, u1, m);
                    }
                    _ => door(&mut prims, u0, u1),
                }
            }
            prims.push(Prim::Poly { pts: vec![start.0, e, qe, start.1], fill: WALL });
            // Balconies on this storey, from the facade's resolved placements.
            let Some(model_b) = doc.buildings.iter().find(|x| x.id == b.building_id) else { continue };
            let Ok(f) = model_b.facade(&w.id) else { continue };
            for r in trigon_model::build::resolve_facade(doc, model_b, f)? {
                if r.is_opening() || r.placement.level != level {
                    continue;
                }
                let Some(status) = view.status(r.lifetime) else { continue };
                let Some(depth) = r.ty.kind.depth() else { continue };
                let (u0, u1) = (r.rect.u0, r.rect.u1);
                let color = status_color(status, LINE);
                let corners = [pt(fr, u0, 0.0), pt(fr, u1, 0.0), pt(fr, u1, -depth), pt(fr, u0, -depth)];
                let dashed = status == Status::Demolished;
                for k in 0..4 {
                    prims.push(Prim::Line { a: corners[k], b: corners[(k + 1) % 4], color, width: 30.0, dashed });
                }
                if status != Status::Demolished {
                    let label_at = pt(fr, (u0 + u1) / 2.0, -depth - 450.0);
                    prims.push(Prim::Text { at: label_at, text: format!("Ca. {:.0} x {:.0}", depth, r.ty.width), color, size: 280.0 });
                }
            }
        }
    }
    Ok((prims, title))
}

/// Maps site mm to output pixels: rotated by `rot` (degrees, counter-clockwise) about the
/// bounds' centre, north-up when 0.
struct Frame {
    c: [f64; 2],
    cos: f64,
    sin: f64,
    x0: f64,
    y1: f64,
    mm: f64,
    w: u32,
    h: u32,
}

impl Frame {
    fn rot(&self, p: [f64; 2]) -> [f64; 2] {
        let (x, y) = (p[0] - self.c[0], p[1] - self.c[1]);
        [self.cos * x - self.sin * y, self.sin * x + self.cos * y]
    }

    fn px(&self, p: [f64; 2]) -> [f64; 2] {
        let r = self.rot(p);
        [(r[0] - self.x0) / self.mm, (self.y1 - r[1]) / self.mm]
    }

    /// Output pixel centre back to site mm.
    fn site(&self, i: u32, j: u32) -> [f64; 2] {
        let (rx, ry) = (self.x0 + (f64::from(i) + 0.5) * self.mm, self.y1 - (f64::from(j) + 0.5) * self.mm);
        [self.c[0] + self.cos * rx + self.sin * ry, self.c[1] - self.sin * rx + self.cos * ry]
    }
}

fn frame_for(doc: &Document, mm: f64, rot_deg: f64) -> Result<Frame> {
    let pts: Vec<[f64; 2]> = doc.buildings.iter().flat_map(|b| b.footprint.iter().copied()).collect();
    let first = *pts.first().ok_or_else(|| ModelError::NotFound("no building yet".into()))?;
    let (lo, hi) = pts.iter().fold((first, first), |(lo, hi), p| ([lo[0].min(p[0]), lo[1].min(p[1])], [hi[0].max(p[0]), hi[1].max(p[1])]));
    let c = [(lo[0] + hi[0]) / 2.0, (lo[1] + hi[1]) / 2.0];
    let t = rot_deg.to_radians();
    let mut f = Frame { c, cos: t.cos(), sin: t.sin(), x0: 0.0, y1: 0.0, mm, w: 1, h: 1 };
    let r: Vec<[f64; 2]> = pts.iter().map(|p| f.rot(*p)).collect();
    let margin = 4000.0;
    let (rx0, rx1) = r.iter().fold((f64::MAX, f64::MIN), |(a, b), p| (a.min(p[0]), b.max(p[0])));
    let (ry0, ry1) = r.iter().fold((f64::MAX, f64::MIN), |(a, b), p| (a.min(p[1]), b.max(p[1])));
    f.x0 = rx0 - margin;
    f.y1 = ry1 + margin;
    f.w = ((rx1 - rx0 + 2.0 * margin) / mm).ceil().clamp(1.0, 6000.0) as u32;
    f.h = ((ry1 - ry0 + 2.0 * margin) / mm).ceil().clamp(1.0, 6000.0) as u32;
    Ok(f)
}

/// The registered archive plan under the cut, lightened, or white.
fn underlay(s: &mut Session, f: &Frame, level: usize, drawing: Option<&str>) -> Result<(Vec<u8>, Option<String>)> {
    let mut rgb = vec![255u8; (f.w * f.h * 3) as usize];
    let pick = s.doc.drawings.iter().find(|d| drawing.is_none_or(|n| n == d.name) && d.plans.iter().any(|p| p.levels.contains(&level))).cloned();
    let Some(d) = pick else {
        if let Some(n) = drawing {
            return Err(ModelError::NotFound(format!("drawing {n} has no plan registered for level {level}; run drawing.register_plan")).into());
        }
        return Ok((rgb, None));
    };
    let Some(reg) = d.plans.iter().find(|p| p.levels.contains(&level)).cloned() else { return Ok((rgb, None)) };
    let img = crate::drawings::image(s, &d.path, d.page, d.rotate)?;
    for j in 0..f.h {
        for i in 0..f.w {
            let [x, y] = reg.to_drawing(f.site(i, j));
            if let Some(g) = img.sample(x, y) {
                let v = (f64::from(g) * 0.55 + 255.0 * 0.45) as u8;
                let k = ((j * f.w + i) * 3) as usize;
                if let Some(dst) = rgb.get_mut(k..k + 3) {
                    dst.copy_from_slice(&[v, v, v]);
                }
            }
        }
    }
    Ok((rgb, Some(d.name)))
}

fn put(rgb: &mut [u8], w: u32, h: u32, x: i64, y: i64, c: [u8; 3]) {
    if x < 0 || y < 0 || x >= i64::from(w) || y >= i64::from(h) {
        return;
    }
    let k = ((y as usize) * w as usize + x as usize) * 3;
    if let Some(d) = rgb.get_mut(k..k + 3) {
        d.copy_from_slice(&c);
    }
}

fn raster(prims: &[Prim], f: &Frame, rgb: &mut [u8]) {
    let (w, h) = (f.w, f.h);
    for p in prims {
        match p {
            Prim::Poly { pts, fill } => {
                let q: Vec<[f64; 2]> = pts.iter().map(|p| f.px(*p)).collect();
                let (ymin, ymax) = q.iter().fold((f64::MAX, f64::MIN), |(a, b), p| (a.min(p[1]), b.max(p[1])));
                let (y0, y1) = (ymin.floor().max(0.0) as i64, ymax.ceil().min(f64::from(h)) as i64);
                for y in y0..y1 {
                    let yc = y as f64 + 0.5;
                    let mut xs: Vec<f64> = Vec::new();
                    for k in 0..q.len() {
                        let (a, b) = (q[k], q[(k + 1) % q.len()]);
                        if (a[1] > yc) != (b[1] > yc) {
                            xs.push(a[0] + (yc - a[1]) * (b[0] - a[0]) / (b[1] - a[1]));
                        }
                    }
                    xs.sort_by(f64::total_cmp);
                    for pair in xs.chunks(2) {
                        if let [x0, x1] = pair {
                            for x in (x0.round() as i64)..(x1.round() as i64) {
                                put(rgb, w, h, x, y, *fill);
                            }
                        }
                    }
                }
            }
            Prim::Line { a, b, color, width, dashed } => {
                let (pa, pb) = (f.px(*a), f.px(*b));
                let len = ((pb[0] - pa[0]).powi(2) + (pb[1] - pa[1]).powi(2)).sqrt();
                let n = len.ceil().clamp(1.0, 20000.0) as i64;
                let r = (width / f.mm / 2.0).max(0.5);
                for s in 0..=n {
                    if *dashed && (s / 6) % 2 == 1 {
                        continue;
                    }
                    let t = s as f64 / n as f64;
                    let (x, y) = (pa[0] + (pb[0] - pa[0]) * t, pa[1] + (pb[1] - pa[1]) * t);
                    let ri = r.ceil() as i64;
                    for dy in -ri..=ri {
                        for dx in -ri..=ri {
                            if ((dx * dx + dy * dy) as f64) <= r * r + 0.5 {
                                put(rgb, w, h, x.round() as i64 + dx, y.round() as i64 + dy, *color);
                            }
                        }
                    }
                }
            }
            Prim::Text { at, text, color, size } => {
                let p = f.px(*at);
                let scale = ((size / f.mm) / 7.0).round().clamp(1.0, 8.0) as i64;
                let digits: String = text.chars().filter(|c| c.is_ascii_digit() || *c == 'x').collect();
                let width = digits.len() as i64 * 4 * scale;
                let (x0, y0) = (p[0].round() as i64 - width / 2, p[1].round() as i64 - 3 * scale);
                for (i, ch) in digits.chars().enumerate() {
                    let glyph: [u8; 5] = match ch {
                        'x' => [0, 5, 2, 5, 0],
                        c => match c.to_digit(10) {
                            Some(d) => crate::drawings::digit(d),
                            None => continue,
                        },
                    };
                    for (row, bits) in glyph.iter().enumerate() {
                        for col in 0..3i64 {
                            if bits & (4 >> col) != 0 {
                                for sy in 0..scale {
                                    for sx in 0..scale {
                                        put(rgb, w, h, x0 + i as i64 * 4 * scale + col * scale + sx, y0 + row as i64 * scale + sy, *color);
                                    }
                                }
                            }
                        }
                    }
                }
            }
        }
    }
}

fn hex(c: [u8; 3]) -> String {
    format!("#{:02x}{:02x}{:02x}", c[0], c[1], c[2])
}

fn svg(prims: &[Prim], f: &Frame, background: Option<&[u8]>, title: &str) -> Result<String> {
    let mut out = String::new();
    let _ = writeln!(out, r#"<svg xmlns="http://www.w3.org/2000/svg" width="{w}" height="{h}" viewBox="0 0 {w} {h}">"#, w = f.w, h = f.h);
    let _ = writeln!(out, r#"<rect width="{}" height="{}" fill="white"/>"#, f.w, f.h);
    if let Some(bg) = background {
        let mut png = Vec::new();
        {
            let mut enc = png::Encoder::new(&mut png, f.w, f.h);
            enc.set_color(png::ColorType::Rgb);
            enc.set_depth(png::BitDepth::Eight);
            let mut wr = enc.write_header().map_err(|e| EngineError::Io(e.to_string()))?;
            wr.write_image_data(bg).map_err(|e| EngineError::Io(e.to_string()))?;
        }
        let _ = writeln!(out, r#"<image width="{}" height="{}" href="data:image/png;base64,{}"/>"#, f.w, f.h, crate::drawings::base64(&png));
    }
    for p in prims {
        match p {
            Prim::Poly { pts, fill } => {
                let d: Vec<String> = pts.iter().map(|p| f.px(*p)).map(|q| format!("{:.1},{:.1}", q[0], q[1])).collect();
                let _ = writeln!(out, r#"<polygon points="{}" fill="{}"/>"#, d.join(" "), hex(*fill));
            }
            Prim::Line { a, b, color, width, dashed } => {
                let (pa, pb) = (f.px(*a), f.px(*b));
                let dash = if *dashed { r#" stroke-dasharray="6 6""# } else { "" };
                let _ = writeln!(
                    out,
                    r#"<line x1="{:.1}" y1="{:.1}" x2="{:.1}" y2="{:.1}" stroke="{}" stroke-width="{:.2}" stroke-linecap="round"{dash}/>"#,
                    pa[0],
                    pa[1],
                    pb[0],
                    pb[1],
                    hex(*color),
                    (width / f.mm).max(0.6)
                );
            }
            Prim::Text { at, text, color, size } => {
                let p = f.px(*at);
                let _ = writeln!(
                    out,
                    r#"<text x="{:.1}" y="{:.1}" font-family="Helvetica, Arial, sans-serif" font-size="{:.1}" fill="{}" text-anchor="middle" dominant-baseline="middle">{}</text>"#,
                    p[0],
                    p[1],
                    size / f.mm,
                    hex(*color),
                    text.replace('&', "&amp;").replace('<', "&lt;")
                );
            }
        }
    }
    let _ = writeln!(
        out,
        r#"<text x="20" y="40" font-family="Helvetica, Arial, sans-serif" font-size="28" fill="black">{}</text>"#,
        title.replace('&', "&amp;").replace('<', "&lt;")
    );
    out.push_str("</svg>\n");
    Ok(out)
}

pub(crate) fn render(
    s: &mut Session,
    level: usize,
    drawing: Option<&str>,
    show_scan: bool,
    view: View,
    mm: f64,
    rot_deg: f64,
    path: PathBuf,
) -> Result<Value> {
    let mm = trigon_model::checked("mm_per_px", mm, 5.0, 200.0)?;
    let rot_deg = trigon_model::checked("rotate_deg", rot_deg, -360.0, 360.0)?;
    let (prims, title) = scene(&s.doc, level, view)?;
    let f = frame_for(&s.doc, mm, rot_deg)?;
    let (mut rgb, used) = if show_scan { underlay(s, &f, level, drawing)? } else { (vec![255u8; (f.w * f.h * 3) as usize], None) };
    let ext = path.extension().and_then(|e| e.to_str()).unwrap_or_default().to_string();
    match ext.as_str() {
        "png" => {
            raster(&prims, &f, &mut rgb);
            write_png(&path, f.w, f.h, &rgb)?;
        }
        "svg" => {
            let text = svg(&prims, &f, used.as_ref().map(|_| rgb.as_slice()), &title)?;
            if let Some(dir) = path.parent().filter(|d| !d.as_os_str().is_empty()) {
                std::fs::create_dir_all(dir).map_err(|e| EngineError::Io(format!("{}: {e}", dir.display())))?;
            }
            std::fs::write(&path, text).map_err(|e| EngineError::Io(format!("{}: {e}", path.display())))?;
        }
        _ => return Err(EngineError::Io(format!("{}: the file name must end in .png or .svg", path.display()))),
    }
    let balconies = prims.iter().filter(|p| matches!(p, Prim::Text { .. })).count();
    let mut out = json!({
        "title": title,
        "underlay": used,
        "width_px": f.w,
        "height_px": f.h,
        "mm_per_px": mm,
        "labelled_balconies": balconies,
        "legend": "black = load-bearing walls, grey = partitions; double lines = windows; single thick line = doors; rectangles = balconies (green new, red dashed demolished)",
    });
    if ext == "png" {
        out["image_path"] = json!(path);
    } else {
        out["written"] = json!(path);
    }
    Ok(out)
}
