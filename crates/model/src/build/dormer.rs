//! Shed dormers on a single straight pitched or mansard plane, with actual host-roof voids.

use super::*;
use crate::roof::Dormer;
use std::collections::BTreeSet;

const FRONT: f64 = 100.0;
const UNDERLAP: f64 = 70.0;
const OVERHANG: f64 = 50.0;
const EPS: f64 = 1e-6;

#[derive(Clone, Copy)]
struct Plane([f64; 3]);
impl Plane {
    fn z(self, u: f64, w: f64) -> f64 {
        self.0[0] * u + self.0[1] * w + self.0[2]
    }
}

fn local(fr: &FacadeFrame, p: [f64; 3]) -> [f64; 3] {
    let d = [p[0] - fr.origin[0], p[1] - fr.origin[1]];
    [d[0] * fr.t[0] + d[1] * fr.t[1], -d[0] * fr.n[0] - d[1] * fr.n[1], p[2]]
}

fn plane(mesh: &Mesh, fr: &FacadeFrame, q: [f64; 2], top: bool) -> Result<Plane> {
    for tri in &mesh.triangles {
        let [a, b, c] = tri.map(|i| local(fr, mesh.positions.get(i as usize).copied().unwrap_or_default()));
        let det = (b[0] - a[0]) * (c[1] - a[1]) - (b[1] - a[1]) * (c[0] - a[0]);
        if (top && det <= EPS) || (!top && det >= -EPS) {
            continue;
        }
        let s = ((q[0] - a[0]) * (c[1] - a[1]) - (q[1] - a[1]) * (c[0] - a[0])) / det;
        let t = ((b[0] - a[0]) * (q[1] - a[1]) - (b[1] - a[1]) * (q[0] - a[0])) / det;
        if s < -EPS || t < -EPS || s + t > 1.0 + EPS {
            continue;
        }
        let u = ((b[2] - a[2]) * (c[1] - a[1]) - (c[2] - a[2]) * (b[1] - a[1])) / det;
        let w = ((b[0] - a[0]) * (c[2] - a[2]) - (c[0] - a[0]) * (b[2] - a[2])) / det;
        return Ok(Plane([u, w, a[2] - u * a[0] - w * a[1]]));
    }
    Err(ModelError::Invalid("dormer is outside a continuous, thick host roof face".into()))
}

fn clip(poly: &[[f64; 3]], axis: usize, value: f64, less: bool) -> Vec<[f64; 3]> {
    let mut out = Vec::new();
    for (&a, &b) in poly.iter().zip(poly.iter().cycle().skip(1)).take(poly.len()) {
        let (da, db) = (a[axis] - value, b[axis] - value);
        let inside = |d: f64| if less { d <= 0.0 } else { d >= 0.0 };
        if inside(da) {
            out.push(a);
        }
        if inside(da) != inside(db) {
            let t = da / (da - db);
            let mut p = std::array::from_fn(|k| a[k] + t * (b[k] - a[k]));
            p[axis] = value;
            out.push(p);
        }
    }
    out.dedup_by(|a, b| a.iter().zip(b.iter()).all(|(a, b)| (a - b).abs() < EPS));
    if out.first().zip(out.last()).is_some_and(|(a, b)| a.iter().zip(b).all(|(a, b)| (a - b).abs() < EPS)) {
        out.pop();
    }
    out
}

fn polygon(mesh: &mut Mesh, points: &[[f64; 3]]) {
    if points.len() < 3 {
        return;
    }
    let base = mesh.positions.len() as u32;
    mesh.positions.extend_from_slice(points);
    for k in 1..points.len() - 1 {
        let [a, b, c] = [points[0], points[k], points[k + 1]];
        let ab = [b[0] - a[0], b[1] - a[1], b[2] - a[2]];
        let ac = [c[0] - a[0], c[1] - a[1], c[2] - a[2]];
        let area = (ab[1] * ac[2] - ab[2] * ac[1]).powi(2) + (ab[2] * ac[0] - ab[0] * ac[2]).powi(2) + (ab[0] * ac[1] - ab[1] * ac[0]).powi(2);
        if area > 1e-12 {
            mesh.triangles.push([base, base + k as u32, base + k as u32 + 1]);
        }
    }
}

#[derive(Clone, Copy)]
struct Hole {
    fr: FacadeFrame,
    u0: f64,
    u1: f64,
    w0: f64,
    w1: f64,
    top: Plane,
    bottom: Plane,
}
impl Hole {
    fn lines(self) -> [(usize, f64, bool); 4] {
        [(0, self.u0, false), (0, self.u1, true), (1, self.w0, false), (1, self.w1, true)]
    }
}

fn cut(mesh: &Mesh, hole: Hole) -> Mesh {
    let mut output = Mesh::default();
    for tri in &mesh.triangles {
        let mut remaining = tri.iter().map(|&i| local(&hole.fr, mesh.positions.get(i as usize).copied().unwrap_or_default())).collect::<Vec<_>>();
        for (axis, value, less) in hole.lines() {
            // A whole face on the split plane belongs to the remaining side once.
            if remaining.iter().all(|p| (p[axis] - value).abs() < EPS) {
                continue;
            }
            let outside = clip(&remaining, axis, value, !less);
            let outside = outside.iter().map(|p| hole.fr.point(p[0], p[1], p[2])).collect::<Vec<_>>();
            polygon(&mut output, &outside);
            remaining = clip(&remaining, axis, value, less);
            if remaining.len() < 3 {
                break;
            }
        }
    }
    let corners = [[hole.u0, hole.w0], [hole.u1, hole.w0], [hole.u1, hole.w1], [hole.u0, hole.w1]];
    for k in 0..4 {
        let (a, b) = (corners[k], corners[(k + 1) % 4]);
        let p = |q: [f64; 2], plane: Plane| hole.fr.point(q[0], q[1], plane.z(q[0], q[1]));
        output.quad(p(a, hole.bottom), p(a, hole.top), p(b, hole.top), p(b, hole.bottom));
    }
    output
}

/// Conform only edges on the clipping planes. Source faces and caps then retain every
/// shared intersection station, without a global all-vertex/all-edge welding pass.
fn conform(mesh: &Mesh, holes: &[Hole]) -> Mesh {
    let mut lines = Vec::new();
    for &h in holes {
        for (axis, value, _) in h.lines() {
            let mut stations =
                mesh.positions.iter().map(|&p| local(&h.fr, p)).filter(|p| (p[axis] - value).abs() < EPS).map(|p| p[1 - axis]).collect::<Vec<_>>();
            stations.sort_by(f64::total_cmp);
            stations.dedup_by(|a, b| (*a - *b).abs() < EPS);
            lines.push((h.fr, axis, value, stations));
        }
    }
    let mut output = Mesh::default();
    for tri in &mesh.triangles {
        let points = tri.map(|i| mesh.positions.get(i as usize).copied().unwrap_or_default());
        let mut boundary = Vec::new();
        for k in 0..3 {
            let (a, b) = (points[k], points[(k + 1) % 3]);
            let mut steps = vec![0.0];
            for (fr, axis, value, stations) in &lines {
                let (aa, bb) = (local(fr, a), local(fr, b));
                let span = bb[1 - axis] - aa[1 - axis];
                if (aa[*axis] - value).abs() > EPS || (bb[*axis] - value).abs() > EPS || span.abs() < EPS {
                    continue;
                }
                for station in stations {
                    let t = (station - aa[1 - axis]) / span;
                    if t > EPS && t < 1.0 - EPS {
                        steps.push(t);
                    }
                }
            }
            steps.sort_by(f64::total_cmp);
            steps.dedup_by(|a, b| (*a - *b).abs() < EPS);
            boundary.extend(steps.into_iter().map(|t| std::array::from_fn(|i| a[i] + t * (b[i] - a[i]))));
        }
        if boundary.len() == 3 {
            polygon(&mut output, &boundary);
        } else {
            let centre = std::array::from_fn(|i| (points[0][i] + points[1][i] + points[2][i]) / 3.0);
            for k in 0..boundary.len() {
                polygon(&mut output, &[centre, boundary[k], boundary[(k + 1) % boundary.len()]]);
            }
        }
    }
    output
}

fn sloped_box(fr: &FacadeFrame, u: (f64, f64), w: (f64, f64), bottom: impl Fn(f64) -> f64, top: impl Fn(f64) -> f64) -> Mesh {
    let corners = [[u.0, w.0], [u.1, w.0], [u.1, w.1], [u.0, w.1]];
    let mut mesh = Mesh::default();
    mesh.hexahedron(corners.map(|p| fr.point(p[0], p[1], bottom(p[1]))), corners.map(|p| fr.point(p[0], p[1], top(p[1]))));
    mesh
}

pub(super) fn build(doc: &Document, b: &Building, cover: &mut Mesh, specs: &[Dormer]) -> Result<Vec<BuiltDormer>> {
    if specs.is_empty() {
        return Ok(Vec::new());
    }
    if b.footprint.len() > 64 || specs.len() > 16 {
        return Err(ModelError::Invalid("dormers support at most 64 footprint corners and 16 instances".into()));
    }
    let original = cover.clone();
    let mut ids = BTreeSet::new();
    let mut reserved = Vec::new();
    let mut holes = Vec::new();
    let mut built = Vec::new();
    for spec in specs {
        spec.validate()?;
        if !ids.insert(&spec.id) {
            return Err(ModelError::Invalid(format!("duplicate dormer id {}", spec.id)));
        }
        let facade = b.facade(&spec.facade)?;
        let (a, e) = b.edge(facade.edge).ok_or_else(|| ModelError::Invalid("dormer facade has no edge".into()))?;
        let fr = FacadeFrame::new(a, e).ok_or_else(|| ModelError::Invalid("dormer facade is too short".into()))?;
        let mut ty = doc.component(&spec.window_type)?.clone();
        if !ty.kind.is_opening() || ty.kind.is_door() {
            return Err(ModelError::Invalid("dormer fill must be a window family".into()));
        }
        ty.width = spec.width_mm;
        ty.height = spec.height_mm;
        ty.validate()?;
        let (left, right) = (spec.u_mm - ty.width / 2.0, spec.u_mm + ty.width / 2.0);
        let front = spec.setback_mm;
        let (top, bottom) = (plane(&original, &fr, [spec.u_mm, front + FRONT], true)?, plane(&original, &fr, [spec.u_mm, front + FRONT], false)?);
        if top.0[0].abs() > EPS || bottom.0[0].abs() > EPS || top.z(spec.u_mm, front) - bottom.z(spec.u_mm, front) < 50.0 {
            return Err(ModelError::Invalid("dormer needs a straight roof face with at least 50 mm of build-up".into()));
        }
        let pitch = spec.roof_pitch_deg.to_radians();
        let slope = pitch.tan();
        if top.0[1] < slope + 0.1 {
            return Err(ModelError::Invalid("dormer host roof must be steeper than its shed roof".into()));
        }
        let finished = if let crate::Roof::Mansard { parameters } = &b.roof {
            match &parameters.covering {
                Some(covering) => {
                    covering.offset(top.0[1].atan().to_degrees(), parameters.lower_pitch_deg, parameters.upper_pitch_deg)?
                        * (1.0 + top.0[1].powi(2)).sqrt()
                }
                None => 0.0,
            }
        } else {
            0.0
        };
        let sill = top.z(spec.u_mm, front + FRONT) + finished + spec.sill_clearance_mm;
        let roof_bottom = sill + ty.height + spec.head_mm;
        let lift = spec.roof_thickness_mm / pitch.cos();
        let back = (roof_bottom + lift - slope * front - top.0[2]) / (top.0[1] - slope);
        if !back.is_finite() || back < front + FRONT + 200.0 || back > front + 5000.0 {
            return Err(ModelError::Invalid("dormer depth must be 200–5000 mm behind its front lining".into()));
        }
        let width = ty.width + 2.0 * spec.cheek_mm;
        let extent = (spec.u_mm - width / 2.0 - OVERHANG, spec.u_mm + width / 2.0 + OVERHANG);
        if extent.0 < 100.0 || extent.1 > fr.length - 100.0 {
            return Err(ModelError::Invalid("dormer runs too close to a facade end".into()));
        }
        if reserved.iter().any(|(f, l, r): &(&str, f64, f64)| *f == spec.facade && extent.0 < *r + 100.0 && *l < extent.1 + 100.0) {
            return Err(ModelError::Invalid("dormers need at least 100 mm between their roof projections".into()));
        }
        reserved.push((&spec.facade, extent.0, extent.1));
        for u in [extent.0, spec.u_mm, extent.1] {
            for w in [front - OVERHANG, front + FRONT, back, back + UNDERLAP] {
                for (p, up) in [(top, true), (bottom, false)] {
                    let found = plane(&original, &fr, [u, w], up)?;
                    if (found.z(u, w) - p.z(u, w)).abs() > 0.01 {
                        return Err(ModelError::Invalid("dormer crosses a hip or slope break; reduce its width, height or setback".into()));
                    }
                }
            }
        }
        let hole = Hole { fr, u0: left, u1: right, w0: front + FRONT, w1: back, top, bottom };
        holes.push(hole);
        *cover = cut(cover, hole);
        let id = format!("{}.dormer.{}", b.id, spec.id);
        let wall_id = format!("{id}.front");
        let start = fr.point(left - spec.cheek_mm, front, 0.0);
        let end = fr.point(right + spec.cheek_mm, front, 0.0);
        let face = FacadeFrame::new([start[0], start[1]], [end[0], end[1]]).ok_or_else(|| ModelError::Invalid("dormer front is empty".into()))?;
        let inner0 = face.point(0.0, FRONT, 0.0);
        let inner1 = face.point(width, FRONT, 0.0);
        let span = Span {
            level: b.storeys.len().saturating_sub(1),
            thickness: FRONT,
            z0: bottom.z(spec.u_mm, front),
            z1: roof_bottom + slope * FRONT,
            in0: 0.0,
            in1: width,
            qs: [inner0[0], inner0[1]],
            qe: [inner1[0], inner1[1]],
        };
        let rect = Rect { u0: spec.cheek_mm, u1: spec.cheek_mm + ty.width, v0: sill, v1: sill + ty.height };
        let profile = crate::profile::opening(&ty.kind, rect);
        let segment = crate::walls::segment_profiles(&face, &wall_id, &span, std::slice::from_ref(&profile))
            .ok_or_else(|| ModelError::Invalid("dormer front could not be triangulated".into()))?;
        let fill = BuiltElement {
            id: format!("{id}.window"),
            name: ty.name.clone(),
            class: ElementClass::Window,
            type_id: ty.id.clone(),
            level: span.level,
            width: ty.width,
            height: ty.height,
            parts: fill_parts(&face, rect, &ty.kind, Fill { wall: FRONT, finish: Material::Render, floor: b.height(), ceiling: span.z1 }),
            status: Status::Existing,
        };
        let opening =
            BuiltOpening { id: format!("{id}.opening"), level: span.level, rect, profile, fill, attached: Vec::new(), status: Status::Existing };
        let front_wall = BuiltWall {
            id: wall_id,
            frame: face,
            kind: FacadeKind::Street,
            height: span.z1,
            bottom: span.z0,
            mesh: segment.mesh.clone(),
            segments: vec![segment],
            plinth: Mesh::default(),
            trim: Mesh::default(),
            trim_parts: Vec::new(),
            finish: Material::Render,
            openings: vec![opening],
            attachments: Vec::new(),
        };
        let low = |w: f64| bottom.z(spec.u_mm, w);
        let high = |w: f64| roof_bottom + slope * (w - front);
        let mut cheeks = sloped_box(&fr, (left - spec.cheek_mm, left), (front + FRONT, back + UNDERLAP), low, high);
        cheeks.append(&sloped_box(&fr, (right, right + spec.cheek_mm), (front + FRONT, back + UNDERLAP), low, high));
        let roof = sloped_box(&fr, extent, (front - OVERHANG, back + UNDERLAP), high, |w| high(w) + lift);
        built.push(BuiltDormer { id, front: front_wall, cheeks, roof });
    }
    *cover = conform(cover, &holes);
    Ok(built)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeMap;
    fn closed(mesh: &Mesh) {
        let mut edges = BTreeMap::<_, (u32, i32)>::new();
        let mut volume = 0.0;
        let key = |p: [f64; 3]| p.map(|v| (v * 1e5).round() as i64);
        assert!(mesh.positions.iter().flatten().all(|v| v.is_finite()));
        for tri in &mesh.triangles {
            let [a, b, c] = tri.map(|i| mesh.positions[i as usize]);
            let ab = [b[0] - a[0], b[1] - a[1], b[2] - a[2]];
            let ac = [c[0] - a[0], c[1] - a[1], c[2] - a[2]];
            let n = [ab[1] * ac[2] - ab[2] * ac[1], ab[2] * ac[0] - ab[0] * ac[2], ab[0] * ac[1] - ab[1] * ac[0]];
            assert!(n.iter().map(|v| v * v).sum::<f64>() > 1e-10);
            volume += a.iter().zip(n).map(|(a, n)| a * n).sum::<f64>() / 6.0;
            for (a, b) in [(key(a), key(b)), (key(b), key(c)), (key(c), key(a))] {
                let (edge, sign) = if a < b { ((a, b), 1) } else { ((b, a), -1) };
                let e = edges.entry(edge).or_default();
                e.0 += 1;
                e.1 += sign;
            }
        }
        assert!(volume > 0.0);
        assert!(
            edges.values().all(|&(n, w)| n == 2 && w == 0),
            "unmatched or duplicate roof edge: {:?}",
            edges.iter().find(|(_, e)| e.0 != 2 || e.1 != 0)
        );
    }
    #[test]
    fn dormer_cuts_a_closed_host_void_and_seats_glazing_clear_of_the_roof() {
        let mut doc = Document::default();
        doc.types.extend(crate::component::copenhagen_types());
        let mut b = Building::new("b", "Original dormer fixture", &[[0., 0.], [30000., 0.], [30000., 10000.], [0., 10000.]], 3, 3400.).unwrap();
        b.roof = serde_json::from_value(serde_json::json!({"kind":"mansard","thickness_mm":220,"overhang_mm":250})).unwrap();
        doc.buildings.push(b);
        let before = crate::build(&doc).unwrap();
        let before_roof = before[0].roof.as_ref().unwrap();
        closed(&before_roof.cover);
        let spec: Dormer = serde_json::from_value(
            serde_json::json!({"id":"test","facade":"b.f1","u_mm":15000,"window_type":"dk.window.dannebrog","width_mm":1100,"height_mm":1200}),
        )
        .unwrap();
        if let crate::Roof::Mansard { parameters } = &mut doc.buildings[0].roof {
            parameters.dormers.push(spec.clone());
        }
        let after = crate::build(&doc).unwrap();
        let roof = after[0].roof.as_ref().unwrap();
        assert_eq!(roof.dormers.len(), 1);
        closed(&roof.cover);
        let d = &roof.dormers[0];
        closed(&d.roof);
        closed(&d.cheeks);
        let fr = FacadeFrame::new([0., 0.], [30000., 0.]).unwrap();
        assert!(plane(&before_roof.cover, &fr, [15000., 600.], true).is_ok());
        assert!(plane(&roof.cover, &fr, [15000., 600.], true).is_err(), "roof cover must be removed behind glazing");
        let window = &d.front.openings[0].fill;
        assert_eq!((window.width, window.height), (1100., 1200.));
        let glass = &window.parts.iter().find(|(m, _)| *m == Material::Glass).unwrap().1;
        for p in &glass.positions {
            let p = local(&fr, *p);
            let host = plane(&before_roof.cover, &fr, [p[0], p[1]], true).unwrap();
            assert!(p[2] > host.z(p[0], p[1]), "front glazing is buried in the host roof");
        }
        assert!(roof.cover.triangles.len() < 1000);
        assert_eq!(after, crate::build(&doc).unwrap());
        if let crate::Roof::Mansard { parameters } = &mut doc.buildings[0].roof {
            parameters.dormers[0].u_mm = 100.;
        }
        assert!(crate::build(&doc).is_err(), "crossing the hip must be rejected");
    }

    #[test]
    fn pitched_dormers_cut_closed_voids_and_keep_glazing_clear_without_changing_legacy_roofs() {
        let mut doc = Document::default();
        let mut b = Building::new("b", "Pitched roof fixture", &[[0., 0.], [30000., 0.], [30000., 10000.], [0., 10000.]], 3, 3400.).unwrap();
        let mut source = serde_json::json!({"kind":"pitched","pitch_deg":46,"thickness_mm":220,"overhang_mm":250});
        b.roof = serde_json::from_value(source.clone()).unwrap();
        assert!(serde_json::to_value(&b.roof).unwrap().get("dormers").is_none());
        doc.buildings.push(b);
        let before = crate::build(&doc).unwrap();
        let original = &before[0].roof.as_ref().unwrap().cover;
        source["dormers"] = serde_json::json!([
            {"id":"front","facade":"b.f1","u_mm":15000,"window_type":"window.casement.2","width_mm":1100,"height_mm":1100},
            {"id":"back","facade":"b.f3","u_mm":15000,"window_type":"window.casement.2","width_mm":1100,"height_mm":1100}
        ]);
        doc.buildings[0].roof = serde_json::from_value(source).unwrap();
        let after = crate::build(&doc).unwrap();
        let roof = after[0].roof.as_ref().unwrap();
        assert_eq!(roof.dormers.len(), 2, "pitched roof dormers must not be silently discarded");
        closed(&roof.cover);
        for dormer in &roof.dormers {
            closed(&dormer.roof);
            closed(&dormer.cheeks);
        }
        let fr = FacadeFrame::new([0., 0.], [30000., 0.]).unwrap();
        assert!(plane(original, &fr, [15000., 600.], true).is_ok());
        assert!(plane(&roof.cover, &fr, [15000., 600.], true).is_err());
        let window = &roof.dormers[0].front.openings[0].fill;
        for p in &window.parts.iter().find(|(m, _)| *m == Material::Glass).unwrap().1.positions {
            let q = local(&fr, *p);
            assert!(q[2] > plane(original, &fr, [q[0], q[1]], true).unwrap().z(q[0], q[1]));
        }
        assert_eq!(after, crate::build(&doc).unwrap());
        let mut invalid = serde_json::to_value(&doc.buildings[0].roof).unwrap();
        invalid["dormers"][0]["u_mm"] = serde_json::json!(100.0);
        doc.buildings[0].roof = serde_json::from_value(invalid).unwrap();
        assert!(crate::build(&doc).is_err(), "the existing hip-clearance check also applies to pitched roofs");
    }

    #[test]
    fn repeated_dormers_on_a_skew_gabled_roof_keep_a_closed_continuous_host() {
        let mut doc = Document::default();
        let mut building = Building::new("b", "Skew gable fixture", &[[0., 0.], [30000., 0.], [32000., 9000.], [-600., 9000.]], 3, 3400.).unwrap();
        let dormers: Vec<_> = [3000, 10000, 20000, 28000]
            .into_iter()
            .map(|u| serde_json::json!({"id":format!("d-{u}"),"facade":"b.f1","u_mm":u,"window_type":"window.casement.2","width_mm":1000,"height_mm":1000}))
            .collect();
        building.roof = serde_json::from_value(serde_json::json!({
            "kind":"pitched","pitch_deg":46,"gables":["b.f2","b.f4"],"thickness_mm":220,"overhang_mm":180,"dormers":dormers
        }))
        .unwrap();
        doc.buildings.push(building);
        let built = crate::build(&doc).unwrap();
        let roof = built[0].roof.as_ref().unwrap();
        assert_eq!(roof.dormers.len(), 4);
        closed(&roof.cover);
        for dormer in &roof.dormers {
            closed(&dormer.roof);
            closed(&dormer.cheeks);
            assert_eq!(dormer.front.openings[0].fill.width, 1000.);
        }
        assert_eq!(built, crate::build(&doc).unwrap());
    }

    #[test]
    fn repeated_dormers_on_opposite_faces_keep_the_host_shell_closed_and_deterministic() {
        let mut doc = Document::default();
        doc.types.extend(crate::component::copenhagen_types());
        let mut b =
            Building::new("b", "Original multiple-dormer fixture", &[[0., 0.], [30000., 0.], [30000., 10000.], [0., 10000.]], 3, 3400.).unwrap();
        b.roof = serde_json::from_value(serde_json::json!({"kind":"mansard","thickness_mm":220,"overhang_mm":250})).unwrap();
        if let crate::Roof::Mansard { parameters } = &mut b.roof {
            for facade in ["b.f1", "b.f3"] {
                for u in [7500, 15000, 22500] {
                    parameters.dormers.push(serde_json::from_value(serde_json::json!({"id":format!("{facade}-{u}"),"facade":facade,"u_mm":u,"window_type":"dk.window.dannebrog","width_mm":1100,"height_mm":1200})).unwrap());
                }
            }
        }
        doc.buildings.push(b);
        let model = crate::build(&doc).unwrap();
        let roof = model[0].roof.as_ref().unwrap();
        assert_eq!(roof.dormers.len(), 6);
        closed(&roof.cover);
        assert!(roof.cover.triangles.len() < 6000);
        assert_eq!(model, crate::build(&doc).unwrap());
    }

    #[test]
    fn hosted_window_clearance_follows_the_finished_roof_envelope() {
        let mut doc = Document::default();
        doc.types.extend(crate::component::copenhagen_types());
        let mut b =
            Building::new("b", "Original finished-roof fixture", &[[0., 0.], [30000., 0.], [30000., 10000.], [0., 10000.]], 3, 3400.).unwrap();
        b.roof=serde_json::from_value(serde_json::json!({"kind":"mansard","thickness_mm":220,"overhang_mm":250,"dormers":[{"id":"test","facade":"b.f1","u_mm":15000,"window_type":"dk.window.dannebrog","width_mm":1100,"height_mm":1200}]})).unwrap();
        doc.buildings.push(b);
        let bare = crate::build(&doc).unwrap();
        let before = &bare[0].roof.as_ref().unwrap().dormers[0].front.openings[0];
        if let crate::Roof::Mansard { parameters } = &mut doc.buildings[0].roof {
            parameters.covering = Some(
                serde_json::from_value(
                    serde_json::json!({"lower":{"material":"slate","width_mm":600,"length_mm":1200,"exposure_mm":400,"thickness_mm":20}}),
                )
                .unwrap(),
            );
        }
        let model = crate::build(&doc).unwrap();
        let roof = model[0].roof.as_ref().unwrap();
        let after = &roof.dormers[0].front.openings[0];
        assert_eq!((after.fill.width, after.fill.height), (before.fill.width, before.fill.height));
        assert_eq!(after.fill.id, before.fill.id);
        let expected = 80.0 / 68f64.to_radians().cos();
        assert!((after.rect.v0 - before.rect.v0 - expected).abs() < 1e-6, "clearance follows the declared lap thickness");
        closed(&roof.cover);
        assert!(roof.covering.iter().any(|(m, _)| *m == Material::Slate));
        let fr = FacadeFrame::new([0., 0.], [30000., 0.]).unwrap();
        let glass = &after.fill.parts.iter().find(|(m, _)| *m == Material::Glass).unwrap().1;
        for p in &glass.positions {
            let q = local(&fr, *p);
            for (_, cover) in &roof.covering {
                for tri in &cover.triangles {
                    let [a, b, c] = tri.map(|i| local(&fr, cover.positions[i as usize]));
                    let det = (b[0] - a[0]) * (c[1] - a[1]) - (b[1] - a[1]) * (c[0] - a[0]);
                    if det <= EPS {
                        continue;
                    }
                    let s = ((q[0] - a[0]) * (c[1] - a[1]) - (q[1] - a[1]) * (c[0] - a[0])) / det;
                    let t = ((b[0] - a[0]) * (q[1] - a[1]) - (b[1] - a[1]) * (q[0] - a[0])) / det;
                    if s >= -EPS && t >= -EPS && s + t <= 1.0 + EPS {
                        assert!(q[2] > a[2] + s * (b[2] - a[2]) + t * (c[2] - a[2]), "glazing is covered by a slate lap");
                    }
                }
            }
        }
    }
}
