//! Floor plans: a storey cut 1 m above its floor, drawn from the model (walls, windows, closed door
//! leaves, balconies with their size, phase colours) over the registered archive plan, as a
//! PNG (for agents) or an SVG (vector, for drawing sets and offers).

use std::fmt::Write as _;
use std::path::PathBuf;

use serde_json::{Value, json};
use trigon_model::build::{FacadeFrame, Status, View, build_view};
use trigon_model::{ComponentKind, Document, ModelError};

use crate::{EngineError, Result, Session};

const CUT: f64 = 1000.0;
const WALL: [u8; 3] = [25, 25, 25];
const PARTITION: [u8; 3] = [110, 110, 110];
const ROOM: [u8; 3] = [90, 90, 90];
const LINE: [u8; 3] = [30, 30, 30];
const NEW: [u8; 3] = [40, 150, 30];
const DEMOLISHED: [u8; 3] = [220, 30, 30];

/// Choose a point with wall clearance inside a room, including concave room polygons.
fn room_label_point(poly: &[[f64; 2]]) -> Option<[f64; 2]> {
    let first = *poly.first()?;
    let (lo, hi) = poly.iter().fold((first[1], first[1]), |(a, b), p| (a.min(p[1]), b.max(p[1])));
    let mut best: Option<([f64; 2], f64)> = None;
    for row in 0..33 {
        let y = lo + (hi - lo) * (row as f64 + 0.5) / 33.0;
        let mut xs = Vec::new();
        for i in 0..poly.len() {
            let (a, b) = (poly[i], poly[(i + 1) % poly.len()]);
            if (a[1] > y) != (b[1] > y) {
                xs.push(a[0] + (y - a[1]) * (b[0] - a[0]) / (b[1] - a[1]));
            }
        }
        xs.sort_by(f64::total_cmp);
        for pair in xs.as_chunks::<2>().0 {
            let p = [(pair[0] + pair[1]) / 2.0, y];
            let clearance = (0..poly.len())
                .map(|i| {
                    let (a, b) = (poly[i], poly[(i + 1) % poly.len()]);
                    let d = [b[0] - a[0], b[1] - a[1]];
                    let t = (((p[0] - a[0]) * d[0] + (p[1] - a[1]) * d[1]) / (d[0] * d[0] + d[1] * d[1]).max(1e-12)).clamp(0.0, 1.0);
                    (p[0] - a[0] - t * d[0]).hypot(p[1] - a[1] - t * d[1])
                })
                .fold(f64::INFINITY, f64::min);
            if clearance > 0.0 && best.is_none_or(|(_, d)| clearance > d) {
                best = Some((p, clearance));
            }
        }
    }
    best.map(|(p, _)| p)
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Surface {
    Exterior,
    Interior,
    Stair,
}

enum Prim {
    Poly {
        pts: Vec<[f64; 2]>,
        fill: [u8; 3],
        surface: Surface,
    },
    Line {
        a: [f64; 2],
        b: [f64; 2],
        color: [u8; 3],
        width: f64,
        dashed: bool,
    },
    /// `away`: the unit direction a label may step out along when it collides with another.
    Text {
        at: [f64; 2],
        text: String,
        color: [u8; 3],
        size: f64,
        angle: Option<f64>,
        away: Option<[f64; 2]>,
    },
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

/// Distance along the ray from `from` in direction `dir` (unit) to the first footprint edge it
/// crosses.
pub(crate) fn facade_crossing(ring: &[[f64; 2]], from: [f64; 2], dir: [f64; 2]) -> Option<f64> {
    let n = ring.len();
    (0..n)
        .filter_map(|i| {
            let (a, b) = (ring[i], ring[(i + 1) % n]);
            let e = [b[0] - a[0], b[1] - a[1]];
            let den = dir[0] * e[1] - dir[1] * e[0];
            if den.abs() < 1e-9 {
                return None;
            }
            let w = [a[0] - from[0], a[1] - from[1]];
            let t = (w[0] * e[1] - w[1] * e[0]) / den;
            let s = (w[0] * dir[1] - w[1] * dir[0]) / den;
            (t > 0.0 && (0.0..=1.0).contains(&s)).then_some(t)
        })
        .min_by(f64::total_cmp)
}

fn visible_stairs(b: &trigon_model::Building, level: usize) -> Vec<&trigon_model::Stair> {
    let outgoing: Vec<_> = b.stairs.iter().filter(|s| s.levels.contains(&level)).collect();
    b.stairs
        .iter()
        .filter(|s| {
            if s.levels.contains(&level) {
                return true;
            }
            if !level.checked_sub(1).is_some_and(|previous| s.levels.contains(&previous)) {
                return false;
            }
            let n = s.outline.len().max(1) as f64;
            let centre = s.outline.iter().fold([0.0; 2], |a, p| [a[0] + p[0] / n, a[1] + p[1] / n]);
            !outgoing.iter().any(|next| trigon_model::stair::opening(next).is_ok_and(|ring| crate::drawings::inside(&ring, centre)))
        })
        .collect()
}

/// The storey's cut as primitives in site mm.
fn scene(doc: &Document, level: usize, view: View, presentation: bool) -> Result<(Vec<Prim>, String)> {
    let mut prims = Vec::new();
    let mut title = String::new();
    let built = build_view(doc, view)?;
    for b in &built {
        let storey = b.storeys.get(level).ok_or_else(|| ModelError::Invalid(format!("level {level} doesn't exist")))?;
        title = format!("{} — {}", b.name, storey.name);
        let cut = storey.elevation + CUT;
        for w in &b.walls {
            let fr = &w.frame;
            // The facade wall as it is on this storey (thicker lower down).
            let Some(seg) = w.segment(level) else { continue };
            let d = seg.thickness;
            // Openings crossing the cut, by position along the wall.
            let mut cuts: Vec<_> = w.openings.iter().filter(|o| o.rect.v0 <= cut && o.rect.v1 >= cut).collect();
            cuts.sort_by(|x, y| x.rect.u0.total_cmp(&y.rect.u0));
            let [a, e, qe, qs] = seg.plan;
            let mut start = (a, qs);
            for o in &cuts {
                let (u0, u1) = (o.rect.u0, o.rect.u1);
                prims.push(Prim::Poly { pts: vec![start.0, pt(fr, u0, 0.0), pt(fr, u0, d), start.1], fill: WALL, surface: Surface::Exterior });
                start = (pt(fr, u1, 0.0), pt(fr, u1, d));
                let color = status_color(o.status, LINE);
                let kind = doc.component(&o.fill.type_id).map(|t| t.kind.clone()).ok();
                let french_rail = matches!(&kind, Some(ComponentKind::FrenchBalcony { .. }));
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
                if french_rail {
                    for depth in [-110.0, -150.0] {
                        prims.push(Prim::Line {
                            a: pt(fr, u0 - 50.0, depth),
                            b: pt(fr, u1 + 50.0, depth),
                            color,
                            width: 14.0,
                            dashed: o.status == Status::Demolished,
                        });
                    }
                }
            }
            prims.push(Prim::Poly { pts: vec![start.0, e, qe, start.1], fill: WALL, surface: Surface::Exterior });
            // Balconies on this storey, from the facade's resolved placements.
            let Some(model_b) = doc.buildings.iter().find(|x| x.id == b.building_id) else { continue };
            let Ok(f) = model_b.facade(&w.id) else { continue };
            for r in trigon_model::build::resolve_facade(doc, model_b, f)? {
                if (r.is_opening() && !matches!(r.ty.kind, ComponentKind::Balcony { .. })) || r.placement.level != level {
                    continue;
                }
                let Some(status) = view.status(r.lifetime) else { continue };
                let Some(depth) = r.ty.kind.depth() else { continue };
                let slab_width = match r.ty.kind {
                    ComponentKind::Balcony { slab_width, .. } => slab_width,
                    _ => r.ty.width,
                };
                let centre = (r.rect.u0 + r.rect.u1) / 2.0;
                let (u0, u1) = (centre - slab_width / 2.0, centre + slab_width / 2.0);
                let color = status_color(status, LINE);
                let corners = [pt(fr, u0, 0.0), pt(fr, u1, 0.0), pt(fr, u1, -depth), pt(fr, u0, -depth)];
                let dashed = status == Status::Demolished;
                for k in 0..4 {
                    prims.push(Prim::Line { a: corners[k], b: corners[(k + 1) % 4], color, width: 30.0, dashed });
                }
                if presentation && status != Status::Demolished {
                    // The deck and perimeter rail are separate from the cut wall.
                    for v in [70.0, depth - 70.0] {
                        prims.push(Prim::Line { a: pt(fr, u0 + 70.0, -v), b: pt(fr, u1 - 70.0, -v), color, width: 14.0, dashed: false });
                    }
                    for u in [u0 + 70.0, u1 - 70.0] {
                        prims.push(Prim::Line { a: pt(fr, u, -70.0), b: pt(fr, u, -depth + 70.0), color, width: 14.0, dashed: false });
                    }
                    let boards = if matches!(r.ty.kind, ComponentKind::SteelBalcony { deck, .. } if deck != trigon_model::component::Deck::FibreConcrete)
                    {
                        (depth / 140.0).round().clamp(2.0, 50.0) as usize
                    } else {
                        1
                    };
                    for k in 1..boards {
                        let v = -depth * k as f64 / boards as f64;
                        prims.push(Prim::Line { a: pt(fr, u0 + 70.0, v), b: pt(fr, u1 - 70.0, v), color, width: 7.0, dashed: false });
                    }
                    let dim = -depth - 300.0;
                    prims.push(Prim::Line { a: pt(fr, u0, dim), b: pt(fr, u1, dim), color, width: 10.0, dashed: false });
                    for u in [u0, u1] {
                        prims.push(Prim::Line { a: pt(fr, u, -depth - 100.0), b: pt(fr, u, dim - 130.0), color, width: 7.0, dashed: false });
                        prims.push(Prim::Line {
                            a: pt(fr, u - 90.0, dim + 90.0),
                            b: pt(fr, u + 90.0, dim - 90.0),
                            color,
                            width: 18.0,
                            dashed: false,
                        });
                    }
                }
                if status != Status::Demolished {
                    let label_at = pt(fr, (u0 + u1) / 2.0, -depth - 450.0);
                    let out = pt(fr, (u0 + u1) / 2.0, -depth - 1450.0);
                    prims.push(Prim::Text {
                        at: label_at,
                        text: format!("Ca. {:.0} x {:.0}", depth, slab_width),
                        color,
                        size: 280.0,
                        angle: presentation.then(|| fr.t[1].atan2(fr.t[0])),
                        away: Some([(out[0] - label_at[0]) / 1000.0, (out[1] - label_at[1]) / 1000.0]),
                    });
                }
            }
        }
    }
    // Interior: walls cut at this storey (joined to each other and to the facade), stairs with
    // treads, room names and areas.
    for built in &built {
        for w in built.interior.iter().filter(|w| w.level == level) {
            prims.push(Prim::Poly { pts: w.outline.to_vec(), fill: if w.load_bearing { WALL } else { PARTITION }, surface: Surface::Interior });
        }
    }
    for b in &doc.buildings {
        let visible = visible_stairs(b, level);
        for st in &visible {
            if presentation {
                prims.push(Prim::Poly { pts: trigon_model::stair::opening(st)?, fill: [245, 245, 245], surface: Surface::Stair });
                continue;
            }
            let departure = if st.levels.contains(&level) { level } else { level.saturating_sub(1) };
            let h = b.storeys.get(departure).map_or(3000.0, |s| s.height);
            let Ok(sym) = trigon_model::stair::symbol(st, h) else { continue };
            for (a, e) in &sym.outline {
                prims.push(Prim::Line { a: *a, b: *e, color: LINE, width: 18.0, dashed: false });
            }
            for (a, e) in &sym.treads {
                prims.push(Prim::Line { a: *a, b: *e, color: LINE, width: 9.0, dashed: false });
            }
            for k in 1..sym.landing.len() {
                prims.push(Prim::Line { a: sym.landing[k - 1], b: sym.landing[k], color: LINE, width: 18.0, dashed: false });
            }
            for k in 1..sym.walk.len() {
                prims.push(Prim::Line { a: sym.walk[k - 1], b: sym.walk[k], color: LINE, width: 7.0, dashed: true });
            }
            // Arrowhead at the start of the walking line, pointing up the first flight.
            if let [w0, w1, ..] = sym.walk.as_slice() {
                let (dx, dy) = (w1[0] - w0[0], w1[1] - w0[1]);
                let l = (dx * dx + dy * dy).sqrt().max(1.0);
                let (ux, uy) = (dx / l, dy / l);
                let tip = [w0[0] + ux * 450.0, w0[1] + uy * 450.0];
                for side in [-1.0, 1.0] {
                    let base = [tip[0] - ux * 180.0 - uy * 90.0 * side, tip[1] - uy * 180.0 + ux * 90.0 * side];
                    prims.push(Prim::Line { a: base, b: tip, color: LINE, width: 9.0, dashed: false });
                }
                let dot = [w0[0] + ux * 60.0, w0[1] + uy * 60.0];
                prims.push(Prim::Line { a: [dot[0] - 40.0, dot[1]], b: [dot[0] + 40.0, dot[1]], color: LINE, width: 70.0, dashed: false });
            }
        }
        // Entrances: house number on the staircase's floor landing, the apartments' doors either side
        // ("TV" to the left and "TH" to the right as you arrive up the stairs).
        for e in &b.entrances {
            let Some(st) = e.stair.as_ref().and_then(|id| b.stairs.iter().find(|s| &s.id == id)) else { continue };
            if !visible.iter().any(|shown| shown.id == st.id) {
                continue;
            }
            let Ok(f) = trigon_model::stair::frame(st) else { continue };
            let landing = f.at(-700.0, f.width / 2.0);
            // Arriving on the floor you walk back along the second flight: facing −run.
            let left = [f.run[1], -f.run[0]];
            let units: Vec<&trigon_model::Unit> = e.units.iter().filter(|u| u.level(b.ground_level) == Some(level)).collect();
            if presentation {
                // On the stair's axis, just outside the facade it meets on the entrance's side, and
                // aligned with the wing (across the flights) so stepped facades don't tilt it.
                let core = f.at(f.length / 2.0, f.width / 2.0);
                let label = [f.run, [-f.run[0], -f.run[1]]]
                    .into_iter()
                    .filter_map(|dir| {
                        let t = facade_crossing(&b.footprint, core, dir)?;
                        let hit = [core[0] + dir[0] * t, core[1] + dir[1] * t];
                        Some(((hit[0] - e.point[0]).hypot(hit[1] - e.point[1]), [hit[0] + dir[0] * 1100.0, hit[1] + dir[1] * 1100.0]))
                    })
                    .min_by(|a, b| a.0.total_cmp(&b.0));
                if let Some((_, at)) = label {
                    let tv = if units.iter().any(|u| u.door == "tv") { "TV  " } else { "" };
                    let th = if units.iter().any(|u| u.door == "th") { "  TH" } else { "" };
                    prims.push(Prim::Text {
                        at,
                        text: format!("{tv}{}{th}", e.number),
                        color: LINE,
                        size: 380.0,
                        angle: Some(f.across[1].atan2(f.across[0])),
                        away: None,
                    });
                }
                continue;
            }
            prims.push(Prim::Text { at: landing, text: e.number.clone(), color: LINE, size: 380.0, angle: None, away: None });
            let mut others = Vec::new();
            let mut unnamed = 0;
            for u in &units {
                let side = match u.door.as_str() {
                    "tv" => Some(1.0),
                    "th" => Some(-1.0),
                    _ => None,
                };
                match side {
                    Some(k) => {
                        let at = [landing[0] + left[0] * 1700.0 * k, landing[1] + left[1] * 1700.0 * k];
                        prims.push(Prim::Text { at, text: u.door.to_uppercase(), color: LINE, size: 320.0, angle: None, away: None });
                    }
                    None if u.door.is_empty() => unnamed += 1,
                    None => others.push(u.door.to_uppercase()),
                }
            }
            if unnamed > 0 {
                others.push(format!("{unnamed} lejl."));
            }
            if !others.is_empty() {
                let at = [landing[0] - f.run[0] * 450.0, landing[1] - f.run[1] * 450.0];
                prims.push(Prim::Text { at, text: others.join(" "), color: LINE, size: 240.0, angle: None, away: None });
            }
        }
        for r in &b.rooms {
            if presentation || !r.levels.contains(&level) || r.polygon.is_empty() {
                continue;
            }
            let Some(c) = room_label_point(&r.polygon) else { continue };
            let area = trigon_model::doc::polygon_area(&r.polygon) / 1e6;
            if r.name.is_empty() {
                prims.push(Prim::Text { at: c, text: format!("{area:.1} m²"), color: ROOM, size: 200.0, angle: None, away: None });
            } else {
                prims.push(Prim::Text { at: [c[0], c[1] + 220.0], text: r.name.clone(), color: LINE, size: 240.0, angle: None, away: None });
                prims
                    .push(Prim::Text { at: [c[0], c[1] - 150.0], text: format!("{area:.1} m²"), color: ROOM, size: 200.0, angle: None, away: None });
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

fn hex(c: [u8; 3]) -> String {
    format!("#{:02x}{:02x}{:02x}", c[0], c[1], c[2])
}

pub(crate) fn write_output(path: &std::path::Path, bytes: &[u8]) -> Result<()> {
    if let Some(dir) = path.parent().filter(|d| !d.as_os_str().is_empty()) {
        std::fs::create_dir_all(dir).map_err(|e| EngineError::Io(format!("{}: {e}", dir.display())))?;
    }
    use std::io::Write as _;
    use std::sync::atomic::{AtomicU64, Ordering};
    static NEXT: AtomicU64 = AtomicU64::new(0);
    let name = path.file_name().ok_or_else(|| EngineError::Io("the output needs a file name".into()))?;
    let mut temporary = name.to_os_string();
    temporary.push(format!(".{}-{}.tmp", std::process::id(), NEXT.fetch_add(1, Ordering::Relaxed)));
    let temporary = path.with_file_name(temporary);
    let mut file =
        std::fs::OpenOptions::new().write(true).create_new(true).open(&temporary).map_err(|e| EngineError::Io(format!("{}: {e}", path.display())))?;
    let write = (|| -> std::io::Result<()> {
        file.write_all(bytes)?;
        file.sync_all()?;
        drop(file);
        std::fs::rename(&temporary, path)
    })();
    if let Err(e) = write {
        let _ = std::fs::remove_file(&temporary);
        return Err(EngineError::Io(format!("{}: {e}", path.display())));
    }
    Ok(())
}

pub(crate) fn escape(s: &str) -> String {
    s.replace('&', "&amp;").replace('<', "&lt;").replace('>', "&gt;").replace('"', "&quot;")
}

fn region_corners(points: &[[f64; 2]]) -> Vec<[f64; 2]> {
    match points {
        [a, b] => vec![*a, [b[0], a[1]], *b, [a[0], b[1]]],
        _ => points.to_vec(),
    }
}

/// A sheet label in pixels: centre, rotation (degrees, clockwise) and font size.
#[derive(Clone)]
pub(crate) struct Label {
    pub(crate) at: [f64; 2],
    pub(crate) alt: Option<[f64; 2]>,
    pub(crate) deg: f64,
    pub(crate) fs: f64,
    pub(crate) text: String,
    pub(crate) balcony: bool,
}

impl Label {
    /// Corners of the text's box (Liberation Sans averages about 0.56 em per character), with a
    /// little air around it.
    fn corners(&self) -> [[f64; 2]; 4] {
        let (hw, hh) = (0.28 * self.fs * self.text.chars().count() as f64 + 4.0, 0.6 * self.fs + 2.0);
        let (s, c) = self.deg.to_radians().sin_cos();
        [[-hw, -hh], [hw, -hh], [hw, hh], [-hw, hh]].map(|[x, y]| [self.at[0] + x * c - y * s, self.at[1] + x * s + y * c])
    }

    /// Separating-axis test of the two rotated boxes.
    pub(crate) fn overlaps(&self, other: &Label) -> bool {
        let (a, b) = (self.corners(), other.corners());
        let axes = [
            [a[1][0] - a[0][0], a[1][1] - a[0][1]],
            [a[3][0] - a[0][0], a[3][1] - a[0][1]],
            [b[1][0] - b[0][0], b[1][1] - b[0][1]],
            [b[3][0] - b[0][0], b[3][1] - b[0][1]],
        ];
        axes.iter().all(|ax| {
            let span = |r: &[[f64; 2]; 4]| {
                r.iter().map(|p| p[0] * ax[0] + p[1] * ax[1]).fold((f64::INFINITY, f64::NEG_INFINITY), |(lo, hi), v| (lo.min(v), hi.max(v)))
            };
            let ((a0, a1), (b0, b1)) = (span(&a), span(&b));
            a0 < b1 && b0 < a1
        })
    }
}

/// A composed landscape sheet. Its crop and rotation affect the viewport only, never the model.
fn sheet_frame(doc: &Document, region: Option<&[[f64; 2]]>, rotation: f64) -> Result<Frame> {
    trigon_model::checked("rotate_deg", rotation, -360.0, 360.0)?;
    let points: Vec<_> = region.map(region_corners).unwrap_or_else(|| doc.buildings.iter().flat_map(|b| b.footprint.iter().copied()).collect());
    if points.len() < 2 || points.len() > 10_000 || points.iter().flatten().any(|x| !x.is_finite() || x.abs() > 1e8) {
        return Err(ModelError::Invalid("sheet region needs 2 to 10000 finite site points".into()).into());
    }
    let t = rotation.to_radians();
    let mut f = Frame { c: [0.0; 2], cos: t.cos(), sin: t.sin(), x0: 0.0, y1: 0.0, mm: 1.0, w: 1600, h: 900 };
    let mut lo = [f64::INFINITY; 2];
    let mut hi = [f64::NEG_INFINITY; 2];
    for p in points {
        let r = f.rot(p);
        for k in 0..2 {
            lo[k] = lo[k].min(r[k]);
            hi[k] = hi[k].max(r[k]);
        }
    }
    if hi[0] - lo[0] < 100.0 || hi[1] - lo[1] < 100.0 {
        return Err(ModelError::Invalid("sheet region needs a nonzero width and height".into()).into());
    }
    let margin = if region.is_some() { 0.0 } else { 2000.0 };
    f.mm = ((hi[0] - lo[0] + 2.0 * margin) / 1440.0).max((hi[1] - lo[1] + 2.0 * margin) / 650.0);
    f.x0 = (lo[0] + hi[0]) / 2.0 - 800.0 * f.mm;
    f.y1 = (lo[1] + hi[1]) / 2.0 + 465.0 * f.mm;
    Ok(f)
}

fn path_data(contours: &[Vec<[f64; 2]>], f: &Frame) -> String {
    let mut d = String::new();
    for ring in contours {
        for (k, p) in ring.iter().enumerate() {
            let q = f.px(*p);
            let _ = write!(d, "{} {:.3} {:.3} ", if k == 0 { 'M' } else { 'L' }, q[0], q[1]);
        }
        d.push_str("Z ");
    }
    d
}

fn sheet_svg(doc: &Document, level: usize, view: View, rotation: f64, region: Option<&[[f64; 2]]>, caption: &str) -> Result<String> {
    let (prims, _) = scene(doc, level, view, true)?;
    let f = sheet_frame(doc, region, rotation)?;
    let floor = doc.buildings.first().and_then(|b| b.storeys.get(level)).map(|s| s.name.as_str()).unwrap_or_default();
    let project = doc.buildings.first().map(|b| b.name.as_str()).unwrap_or_default();
    let caption = if caption.is_empty() { project } else { caption };
    let clip = if let Some(region) = region {
        let mut lo = [f64::INFINITY; 2];
        let mut hi = [f64::NEG_INFINITY; 2];
        for p in region_corners(region) {
            let q = f.px(p);
            for k in 0..2 {
                lo[k] = lo[k].min(q[k]);
                hi[k] = hi[k].max(q[k]);
            }
        }
        let pad = 0.0;
        let x = (lo[0] - pad).max(70.0);
        let y = (lo[1] - pad).max(130.0);
        [x, y, (hi[0] + pad).min(1530.0) - x, (hi[1] + pad).min(800.0) - y]
    } else {
        [70.0, 130.0, 1460.0, 670.0]
    };
    let mut out = String::from(
        r##"<svg xmlns="http://www.w3.org/2000/svg" width="1600" height="900" viewBox="0 0 1600 900">
<rect width="1600" height="900" fill="white"/>
<defs>
<pattern id="masonry" width="7" height="7" patternUnits="userSpaceOnUse"><rect width="7" height="7" fill="white"/><path d="M-2 2 L2 -2 M0 7 L7 0 M5 9 L9 5" fill="none" stroke="#777" stroke-width="0.65"/></pattern>
<pattern id="stair" width="8" height="8" patternUnits="userSpaceOnUse"><rect width="8" height="8" fill="#fafafa"/><circle cx="2" cy="2" r="0.6" fill="#aaa"/></pattern>
</defs>
<g font-family="Liberation Sans, Helvetica, Arial, sans-serif" fill="#242424">
"##,
    );
    let _ = writeln!(
        out,
        r#"<defs><clipPath id="viewport"><rect x="{}" y="{}" width="{}" height="{}"/></clipPath></defs>"#,
        clip[0], clip[1], clip[2], clip[3]
    );
    let title_width = (floor.chars().count() as f64 * 23.0 + 44.0).max(160.0);
    let _ = writeln!(
        out,
        r##"<rect x="80" y="42" width="{title_width:.1}" height="70" fill="white" stroke="#999" stroke-width="0.8"/><text x="102" y="91" font-size="40">{}</text><text x="1520" y="72" text-anchor="end" font-size="18">{}</text>"##,
        escape(floor),
        escape(project)
    );
    out.push_str("<g clip-path=\"url(#viewport)\">\n");
    // Union each family before stroking: no rectangles' internal seams or doubled junction edges.
    for surface in [Surface::Stair, Surface::Interior, Surface::Exterior] {
        let polys: Vec<_> = prims
            .iter()
            .filter_map(|p| match p {
                Prim::Poly { pts, surface: s, .. } if *s == surface => Some(pts.clone()),
                _ => None,
            })
            .collect();
        let fill = match surface {
            Surface::Exterior => "url(#masonry)",
            Surface::Interior => "#d6d6d6",
            Surface::Stair => "url(#stair)",
        };
        for shape in trigon_model::footprint::union(&polys)? {
            let _ = writeln!(
                out,
                r##"<path d="{}" fill="{fill}" fill-rule="evenodd" stroke="#333" stroke-width="1.1" stroke-linejoin="miter"/>"##,
                path_data(&shape, &f)
            );
        }
    }
    let mut labels: Vec<Label> = Vec::new();
    for p in &prims {
        match p {
            Prim::Poly { .. } => {}
            Prim::Line { a, b, width, dashed, color } => {
                let (a, b) = (f.px(*a), f.px(*b));
                let stroke = if *dashed || *color == DEMOLISHED { "#bd3030" } else { "#333" };
                let dash = if *dashed { " stroke-dasharray=\"8 5\"" } else { "" };
                let weight = if *width >= 25.0 {
                    1.15
                } else if *width >= 14.0 {
                    0.8
                } else {
                    0.55
                };
                let _ = writeln!(
                    out,
                    r#"<line x1="{:.3}" y1="{:.3}" x2="{:.3}" y2="{:.3}" stroke="{stroke}" stroke-width="{weight}"{dash}/>"#,
                    a[0], a[1], b[0], b[1]
                );
            }
            Prim::Text { at, text, size, angle, away, .. } => {
                let p = f.px(*at);
                if !(80.0..=1520.0).contains(&p[0]) || !(140.0..=790.0).contains(&p[1]) {
                    continue;
                }
                let mut deg = angle.map_or(0.0, |a| -(a.to_degrees() + rotation)).rem_euclid(360.0);
                if deg > 90.0 && deg < 270.0 {
                    deg += 180.0;
                }
                let fs = if *size >= 380.0 { 18.0 } else { 14.0 };
                // One text line further out, for a label that collides where it is.
                let alt = away.and_then(|d| {
                    let q = f.px([at[0] + d[0] * 1000.0, at[1] + d[1] * 1000.0]);
                    let l = (q[0] - p[0]).hypot(q[1] - p[1]);
                    (l > 1e-6).then(|| [p[0] + (q[0] - p[0]) / l * fs * 1.7, p[1] + (q[1] - p[1]) / l * fs * 1.7])
                });
                labels.push(Label { at: p, alt, deg, fs, text: text.replace(" x ", " × "), balcony: text.starts_with("Ca. ") });
            }
        }
    }
    // Labels never overlap: entrance labels first, then balcony sizes. When the balcony sizes don't
    // fit side by side at this scale they move to one line under the drawing instead.
    labels.sort_by_key(|l| l.balcony);
    let mut sizes: Vec<(String, usize)> = Vec::new();
    for l in labels.iter().filter(|l| l.balcony) {
        match sizes.iter_mut().find(|(t, _)| *t == l.text) {
            Some((_, n)) => *n += 1,
            None => sizes.push((l.text.clone(), 1)),
        }
    }
    let mut placed: Vec<Label> = Vec::new();
    let mut crowded = false;
    for l in labels {
        let fits = |l: &Label| !placed.iter().any(|q| q.overlaps(l));
        if fits(&l) {
            placed.push(l);
            continue;
        }
        match l.alt {
            Some(at) if fits(&Label { at, alt: None, ..l.clone() }) => placed.push(Label { at, alt: None, ..l }),
            _ => crowded |= l.balcony,
        }
    }
    if !crowded {
        sizes.clear();
    }
    for l in placed.iter().filter(|l| !(crowded && l.balcony)) {
        let _ = writeln!(
            out,
            r##"<text x="{:.3}" y="{:.3}" text-anchor="middle" dominant-baseline="middle" font-size="{}" transform="rotate({:.3} {:.3} {:.3})" paint-order="stroke fill" stroke="white" stroke-width="4" stroke-linejoin="round">{}</text>"##,
            l.at[0],
            l.at[1],
            l.fs,
            l.deg,
            l.at[0],
            l.at[1],
            escape(&l.text)
        );
    }
    out.push_str("</g>\n");
    if !sizes.is_empty() {
        let list: Vec<String> = sizes.iter().map(|(t, n)| format!("{n} stk. {t}")).collect();
        let _ = writeln!(out, r##"<text x="80" y="812" font-size="15">Altaner: {}</text>"##, escape(&list.join(", ")));
    }
    let _ = writeln!(
        out,
        r##"<text x="800" y="847" text-anchor="middle" font-size="28">{}</text><text x="80" y="886" font-size="12" fill="#777">Mål og tegninger er vejledende</text><text x="1520" y="886" text-anchor="end" font-size="12" fill="#777">Trigon · {}</text></g></svg>"##,
        escape(caption),
        escape(floor)
    );
    Ok(out)
}

/// A presentation drawing set: vector PDF pages, or one matching SVG/PNG sheet.
pub(crate) fn sheet(
    doc: &Document,
    levels: &[usize],
    view: View,
    rotation: f64,
    region: Option<&[[f64; 2]]>,
    caption: &str,
    path: PathBuf,
) -> Result<Value> {
    if levels.is_empty() || levels.len() > 100 || caption.len() > 500 {
        return Err(ModelError::Invalid("a drawing set needs 1 to 100 levels and a caption of at most 500 bytes".into()).into());
    }
    let ext = path.extension().and_then(|s| s.to_str()).unwrap_or_default();
    if !matches!(ext, "png" | "svg" | "pdf") || (ext != "pdf" && levels.len() != 1) {
        return Err(ModelError::Invalid("use .pdf for a drawing set, or .png/.svg for a single sheet".into()).into());
    }
    let pages: Vec<String> = levels.iter().map(|level| sheet_svg(doc, *level, view, rotation, region, caption)).collect::<Result<_>>()?;
    let bytes = match ext {
        "pdf" => crate::plan_output::pdf(&pages, [338.666_666_7, 190.5])?,
        "png" => crate::plan_output::png(&pages[0])?,
        _ => pages[0].as_bytes().to_vec(),
    };
    write_output(&path, &bytes)?;
    let mut result = json!({"written": path, "pages": levels.len(), "width_px": 1600, "height_px": 900, "underlay": null, "style": "presentation"});
    if ext == "png" {
        result["image_path"] = json!(path);
    }
    Ok(result)
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
            Prim::Poly { pts, fill, .. } => {
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
            Prim::Text { at, text, color, size, .. } => {
                let p = f.px(*at);
                let _ = writeln!(
                    out,
                    r#"<text x="{:.1}" y="{:.1}" font-family="Liberation Sans, Helvetica, Arial, sans-serif" font-size="{:.1}" fill="{}" text-anchor="middle" dominant-baseline="middle">{}</text>"#,
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
        r#"<text x="20" y="40" font-family="Liberation Sans, Helvetica, Arial, sans-serif" font-size="28" fill="black">{}</text>"#,
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
    let (mut prims, title) = scene(&s.doc, level, view, false)?;
    if show_scan {
        // Check views: the model in colour so it stands apart from the scan's greys.
        for p in &mut prims {
            match p {
                Prim::Poly { fill, .. } if *fill == WALL => *fill = [210, 20, 20],
                Prim::Poly { fill, .. } if *fill == PARTITION => *fill = [20, 90, 230],
                Prim::Line { color, .. } if *color == LINE => *color = [200, 0, 160],
                _ => {}
            }
        }
    }
    let f = frame_for(&s.doc, mm, rot_deg)?;
    let (rgb, used) = if show_scan { underlay(s, &f, level, drawing)? } else { (vec![255u8; (f.w * f.h * 3) as usize], None) };
    let ext = path.extension().and_then(|e| e.to_str()).unwrap_or_default().to_string();
    match ext.as_str() {
        "png" => {
            let text = svg(&prims, &f, used.as_ref().map(|_| rgb.as_slice()), &title)?;
            let bytes = crate::plan_output::png(&text)?;
            write_output(&path, &bytes)?;
        }
        "svg" => {
            let text = svg(&prims, &f, used.as_ref().map(|_| rgb.as_slice()), &title)?;
            write_output(&path, text.as_bytes())?;
        }
        _ => return Err(EngineError::Io(format!("{}: the file name must end in .png or .svg", path.display()))),
    }
    let balconies = prims.iter().filter(|p| matches!(p, Prim::Text { text, .. } if text.starts_with("Ca. "))).count();
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

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn receiving_floor_shows_its_stair_and_rotated_box_crop_is_valid() {
        let mut b = trigon_model::Building::new("b", "test", &[[0., 0.], [10000., 0.], [10000., 10000.], [0., 10000.]], 2, 3000.).unwrap();
        b.stairs.push(
            serde_json::from_value(json!({"id":"only-flight", "levels":[0], "outline":[[2000.,2000.],[4000.,2000.],[4000.,3200.],[2000.,3200.]]}))
                .unwrap(),
        );
        assert_eq!(visible_stairs(&b, 1).len(), 1);
        let mut doc = Document::default();
        doc.buildings.push(b);
        let frame = sheet_frame(&doc, Some(&[[0., 0.], [10000., 10000.]]), 45.).unwrap();
        assert!(frame.mm.is_finite());
        for p in [[0., 0.], [10000., 0.], [10000., 10000.], [0., 10000.]] {
            let q = frame.px(p);
            assert!((70.0..=1530.0).contains(&q[0]) && (130.0..=800.0).contains(&q[1]));
        }
        let (prims, _) = scene(&doc, 1, View::latest(&doc), true).unwrap();
        assert!(prims.iter().any(|p| matches!(p, Prim::Poly { surface: Surface::Stair, .. })));
    }

    #[test]
    fn labels_collide_as_rotated_boxes() {
        let label = |at: [f64; 2], deg: f64| Label { at, alt: None, deg, fs: 14.0, text: "Ca. 1300 × 2700".into(), balcony: true };
        // Side by side 80 px apart: the 15-character labels overlap; 140 px apart they don't.
        assert!(label([100., 100.], 0.).overlaps(&label([180., 100.], 0.)));
        assert!(!label([100., 100.], 0.).overlaps(&label([240., 100.], 0.)));
        // One line further out clears it; turned 90° the boxes are narrow across the line.
        assert!(!label([100., 100.], 0.).overlaps(&label([180., 124.], 0.)));
        assert!(!label([100., 100.], 90.).overlaps(&label([130., 100.], 90.)));
    }

    #[test]
    fn entrance_labels_find_the_facade_on_a_stair_axis() {
        let square = [[0., 0.], [10000., 0.], [10000., 8000.], [0., 8000.]];
        assert_eq!(facade_crossing(&square, [4000., 3000.], [0., -1.]), Some(3000.));
        assert_eq!(facade_crossing(&square, [4000., 3000.], [1., 0.]), Some(6000.));
        assert_eq!(facade_crossing(&square, [-100., 3000.], [-1., 0.]), None);
    }

    #[test]
    fn concave_room_label_stays_inside_and_away_from_walls() {
        let p = [[0., 0.], [6000., 0.], [6000., 1500.], [1500., 1500.], [1500., 6000.], [0., 6000.]];
        let at = room_label_point(&p).unwrap();
        assert!(crate::drawings::inside(&p, at));
        assert!(at[0] > 500. && at[1] > 500.);
        assert!(at[0] < 1000. || at[1] < 1000.);
    }
}
