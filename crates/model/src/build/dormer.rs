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
        let source = FacadeFrame::new(a, e).ok_or_else(|| ModelError::Invalid("dormer facade is too short".into()))?;
        // Keep the original wall-line u station as the rotation anchor. The zero path must
        // retain the original arithmetic, including for old documents and negative zero.
        let fr = if spec.yaw_deg == 0.0 {
            source
        } else {
            let (sin, cos) = spec.yaw_deg.to_radians().sin_cos();
            let t = [source.t[0] * cos - source.t[1] * sin, source.t[0] * sin + source.t[1] * cos];
            let anchor = source.point(spec.u_mm, 0.0, 0.0);
            FacadeFrame { origin: [anchor[0] - spec.u_mm * t[0], anchor[1] - spec.u_mm * t[1]], t, n: [t[1], -t[0]], length: source.length }
        };
        let mut ty = doc.component(&spec.window_type)?.clone();
        if !ty.kind.is_opening() || ty.kind.is_door() {
            return Err(ModelError::Invalid("dormer fill must be a window family".into()));
        }
        ty.width = spec.width_mm;
        ty.height = spec.height_mm;
        ty.validate()?;
        let (left, right) = (spec.u_mm - ty.width / 2.0, spec.u_mm + ty.width / 2.0);
        let front = spec.setback_mm;
        let sample = |u, w, up| {
            plane(&original, &fr, [u, w], up).map_err(|error| match error {
                ModelError::Invalid(message) => ModelError::Invalid(format!(
                    "{message}; dormer {} on {}: sampled (u={u:.6} mm, w={w:.6} mm), surface={}; \
                     no intersecting host surface, so no height is available at this sample",
                    spec.id,
                    spec.facade,
                    if up { "top" } else { "underside" }
                )),
                other => other,
            })
        };
        let (top, bottom) = (sample(spec.u_mm, front + FRONT, true)?, sample(spec.u_mm, front + FRONT, false)?);
        if top.0[0].abs() > EPS || bottom.0[0].abs() > EPS {
            let correction = (-top.0[0]).atan2(top.0[1]);
            let required = (spec.yaw_deg + correction.to_degrees() + 180.0).rem_euclid(360.0) - 180.0;
            let (sin, cos) = correction.sin_cos();
            let advice = if (bottom.0[0] * cos + bottom.0[1] * sin).abs() <= EPS {
                format!("sampled host requires yaw_deg={required:.9} deg; supported range is -45 to 45 deg, subject to the remaining host limits")
            } else {
                let underside = (spec.yaw_deg + (-bottom.0[0]).atan2(bottom.0[1]).to_degrees() + 180.0).rem_euclid(360.0) - 180.0;
                format!(
                    "sampled top requires yaw_deg={required:.9} deg and underside requires {underside:.9} deg; no common orientation, check the host planes"
                )
            };
            return Err(ModelError::Invalid(format!(
                "dormer {} on {}: yaw_deg={:.9} deg does not align with the host (along-front slopes top={:.9}, underside={:.9} mm/mm); \
                 {advice}. Yaw is right-handed about +Z from the source facade; u_mm={:.6} mm remains the wall-line anchor",
                spec.id, spec.facade, spec.yaw_deg, top.0[0], bottom.0[0], spec.u_mm
            )));
        }
        if top.z(spec.u_mm, front) - bottom.z(spec.u_mm, front) < 50.0 {
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
        let projection = if spec.yaw_deg == 0.0 {
            extent
        } else {
            let corners = [extent.0, extent.1].map(|u| [front - OVERHANG, back + UNDERLAP].map(|w| local(&source, fr.point(u, w, 0.0))[0]));
            (corners.iter().flatten().copied().fold(f64::INFINITY, f64::min), corners.iter().flatten().copied().fold(f64::NEG_INFINITY, f64::max))
        };
        if projection.0 < 100.0 || projection.1 > source.length - 100.0 {
            return Err(ModelError::Invalid(format!(
                "dormer runs too close to a facade end; dormer {} on {}: u_mm={:.6} mm, \
                 projected along span=[{:.6}, {:.6}] mm, permitted end range=[100.000000, {:.6}] mm; \
                 shortfall start={:.6} mm, end={:.6} mm; centre end limits=[{:.6}, {:.6}] mm for this projected envelope only \
                 (an empty range cannot fit). Other host checks still apply; no placement is changed",
                spec.id,
                spec.facade,
                spec.u_mm,
                projection.0,
                projection.1,
                source.length - 100.0,
                (100.0 - projection.0).max(0.0),
                (projection.1 - (source.length - 100.0)).max(0.0),
                spec.u_mm + (100.0 - projection.0),
                spec.u_mm + (source.length - 100.0 - projection.1)
            )));
        }
        if reserved.iter().any(|(f, l, r): &(&str, f64, f64)| *f == spec.facade && projection.0 < *r + 100.0 && *l < projection.1 + 100.0) {
            return Err(ModelError::Invalid("dormers need at least 100 mm between their roof projections".into()));
        }
        reserved.push((&spec.facade, projection.0, projection.1));
        for u in [extent.0, spec.u_mm, extent.1] {
            for w in [front - OVERHANG, front + FRONT, back, back + UNDERLAP] {
                for (p, up) in [(top, true), (bottom, false)] {
                    let found = sample(u, w, up)?;
                    if (found.z(u, w) - p.z(u, w)).abs() > 0.01 {
                        return Err(ModelError::Invalid(format!(
                            "dormer crosses a hip or slope break; reduce its width, height or setback; dormer {} on {}: \
                             sampled (u={u:.6} mm, w={w:.6} mm), surface={}; expected z={:.6} mm, found z={:.6} mm, \
                             found-expected={:.6} mm exceeds 0.010000 mm absolute tolerance. \
                             Expected height extends the front reference plane sampled at (u={:.6} mm, w={:.6} mm); \
                             this is a rejected sample, not a fit elsewhere",
                            spec.id,
                            spec.facade,
                            if up { "top" } else { "underside" },
                            p.z(u, w),
                            found.z(u, w),
                            found.z(u, w) - p.z(u, w),
                            spec.u_mm,
                            front + FRONT
                        )));
                    }
                }
            }
        }
        if spec.yaw_deg != 0.0 {
            let rear_height = roof_bottom + slope * (back + UNDERLAP - front) - bottom.z(spec.u_mm, back + UNDERLAP);
            if rear_height <= EPS {
                return Err(ModelError::Invalid(format!(
                    "dormer {} on {}: rear cheek height is {rear_height:.6} mm; it must stay positive through the {UNDERLAP} mm rear underlap. \
                     Check host build-up and shed roof_thickness_mm; these planes would form an inverted cheek",
                    spec.id, spec.facade
                )));
            }
        }
        let hole = Hole { fr, u0: left, u1: right, w0: front + FRONT, w1: back, top, bottom };
        holes.push(hole);
        *cover = cut(cover, hole);
        let id = format!("{}.dormer.{}", b.id, spec.id);
        let wall_id = format!("{id}.front");
        let start = fr.point(left - spec.cheek_mm, front, 0.0);
        let end = fr.point(right + spec.cheek_mm, front, 0.0);
        let face = if spec.yaw_deg == 0.0 {
            FacadeFrame::new([start[0], start[1]], [end[0], end[1]]).ok_or_else(|| ModelError::Invalid("dormer front is empty".into()))?
        } else {
            FacadeFrame { origin: [start[0], start[1]], t: fr.t, n: fr.n, length: width }
        };
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
        let mut segment = crate::walls::segment_profiles(&face, &wall_id, &span, std::slice::from_ref(&profile))
            .ok_or_else(|| ModelError::Invalid("dormer front could not be triangulated".into()))?;
        if spec.yaw_deg != 0.0 || matches!(&b.roof, crate::Roof::Pitched { eave_facades: Some(_), .. }) {
            // The wall polygon overlay quantizes its output; caps/reveals use the exact
            // inputs. Restore those known stations within the existing numerical EPS so
            // both sides share identical vertices, including at fractional host elevations.
            // Selected eave hosts can have these heights even at zero yaw; legacy None keeps
            // its original construction bytes.
            let stations: Vec<_> = [[0.0, span.z0], [width, span.z0], [width, span.z1], [0.0, span.z1]]
                .into_iter()
                .chain(profile.iter().copied())
                .flat_map(|[u, z]| [face.point(u, 0.0, z), face.point(u, FRONT, z)])
                .collect();
            for p in &mut segment.mesh.positions {
                if let Some(q) = stations.iter().find(|q| p.iter().zip(q.iter()).all(|(a, b)| (a - b).abs() < EPS)) {
                    *p = *q;
                }
            }
        }
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

    // A shallow reflex corner is straightened into an eave oblique to the original wall.
    // Mirroring the outline reverses yaw without changing the dimensions or host problem.
    fn skew_eave(positive: bool, rotation_deg: f64, translation: [f64; 2], kind: &str) -> (Document, Dormer) {
        let outline = if positive {
            [[12000., 0.], [24000., 0.], [24000., 11000.], [0., 11000.], [0., -1000.]]
        } else {
            [[0., 0.], [12000., 0.], [24000., -1000.], [24000., 11000.], [0., 11000.]]
        };
        let (sin, cos) = rotation_deg.to_radians().sin_cos();
        let outline = outline.map(|[x, y]| [translation[0] + x * cos - y * sin, translation[1] + x * sin + y * cos]);
        let mut b = Building::new("b", "Synthetic skew eave", &outline, 3, 3400.).unwrap();
        assert_eq!(b.footprint.len(), 5);
        assert_eq!(crate::roof::eave_outline(&b.footprint, 1200.).len(), 4);
        let mut roof = serde_json::json!({"kind":kind,"thickness_mm":220,"overhang_mm":250});
        if kind == "pitched" {
            roof["pitch_deg"] = serde_json::json!(46);
        } else {
            roof["break_run_mm"] = serde_json::json!(2000);
        }
        b.roof = serde_json::from_value(roof).unwrap();
        let spec = serde_json::from_value(serde_json::json!({
            "id":"skew","facade":"b.f1","u_mm":6000,
            "yaw_deg":if positive { 1.0 } else { -1.0 } * (1000.0f64 / 24000.0).atan().to_degrees(),
            "window_type":"window.casement.2","width_mm":1100,"height_mm":1100
        }))
        .unwrap();
        let mut doc = Document::default();
        doc.buildings.push(b);
        (doc, spec)
    }

    #[test]
    fn explicit_yaw_hosts_a_dormer_on_a_synthetic_straightened_eave() {
        let (mut doc, spec) = skew_eave(false, 0., [0., 0.], "pitched");
        doc.buildings[0].roof.dormers_mut().unwrap().push(spec);
        let built = crate::build(&doc).unwrap();
        let roof = built[0].roof.as_ref().unwrap();
        closed(&roof.cover);
        closed(&roof.dormers[0].front.mesh);
        closed(&roof.dormers[0].cheeks);
        closed(&roof.dormers[0].roof);
    }

    #[test]
    fn oriented_hosts_preserve_anchors_voids_and_clearance_in_rotated_translated_site_frames() {
        for kind in ["pitched", "mansard"] {
            for positive in [false, true] {
                for (rotation, offset) in [(0., [0., 0.]), (37.25, [150000., -270000.]), (-118., [-30000., 41000.])] {
                    let (mut doc, spec) = skew_eave(positive, rotation, offset, kind);
                    if let crate::Roof::Mansard { parameters } = &mut doc.buildings[0].roof {
                        parameters.covering = Some(serde_json::from_value(serde_json::json!({"lower":{"material":"felt"}})).unwrap());
                    }
                    let before = crate::build(&doc).unwrap();
                    let before_roof = before[0].roof.as_ref().unwrap();
                    let b = &doc.buildings[0];
                    let (a, e) = b.edge(0).unwrap();
                    let source = FacadeFrame::new(a, e).unwrap();
                    let anchor = source.point(spec.u_mm, 0., 0.);
                    let (a, e) = if positive { (b.footprint[4], b.footprint[1]) } else { (b.footprint[0], b.footprint[2]) };
                    let eave = FacadeFrame::new(a, e).unwrap();
                    // Independent reference: the actual straightened eave direction, centred
                    // at the original facade anchor, rather than the implementation's frame.
                    let reference = FacadeFrame { origin: [anchor[0], anchor[1]], ..eave };
                    doc.buildings[0].roof.dormers_mut().unwrap().push(spec.clone());
                    let built = crate::build(&doc).unwrap();
                    let roof = built[0].roof.as_ref().unwrap();
                    let d = &roof.dormers[0];
                    for mesh in [&roof.cover, &d.front.mesh, &d.cheeks, &d.roof] {
                        closed(mesh);
                    }
                    assert_eq!(d.id, "b.dormer.skew");
                    let width = spec.width_mm + 2. * spec.cheek_mm;
                    let actual_anchor = d.front.frame.point(width / 2., -spec.setback_mm, 0.);
                    for i in 0..2 {
                        assert!((d.front.frame.t[i] - eave.t[i]).abs() < 1e-10);
                        assert!((actual_anchor[i] - anchor[i]).abs() < 1e-8, "u_mm must stay on the source wall line");
                    }
                    let q = [0., spec.setback_mm + FRONT + 100.];
                    for top in [true, false] {
                        assert!(plane(&before_roof.cover, &reference, q, top).is_ok());
                        for u in [-spec.width_mm / 2. + 1., 0., spec.width_mm / 2. - 1.] {
                            assert!(plane(&roof.cover, &reference, [u, q[1]], top).is_err(), "the rotated host cut must be a real through-void");
                        }
                        assert!(plane(&roof.cover, &reference, [spec.width_mm / 2. + 1., q[1]], top).is_ok(), "do not cut into the cheek seat");
                    }
                    let fill = &d.front.openings[0].fill;
                    assert_eq!((fill.id.as_str(), fill.width, fill.height), ("b.dormer.skew.window", spec.width_mm, spec.height_mm));
                    let glass = &fill.parts.iter().find(|(m, _)| *m == Material::Glass).unwrap().1;
                    for &p in &glass.positions {
                        let q = local(&reference, p);
                        let host = plane(&before_roof.cover, &reference, [q[0], q[1]], true).unwrap();
                        assert!(q[2] >= host.z(q[0], q[1]) + spec.sill_clearance_mm - EPS);
                        for (_, covering) in &before_roof.covering {
                            if let Ok(host) = plane(covering, &reference, [q[0], q[1]], true) {
                                assert!(q[2] > host.z(q[0], q[1]), "glazing must clear the actual finished roof");
                            }
                        }
                    }
                    let bytes = serde_json::to_vec(&doc).unwrap();
                    let reopened: Document = serde_json::from_slice(&bytes).unwrap();
                    assert_eq!(doc, reopened);
                    assert_eq!(built, crate::build(&reopened).unwrap());
                }
            }
        }
    }

    #[test]
    fn omitted_and_zero_yaw_keep_legacy_source_bytes_and_geometry() {
        let legacy = r#"{"id":"test","facade":"b.f1","u_mm":15000.0,"window_type":"window.casement.2","width_mm":1100.0,"height_mm":1100.0,"setback_mm":200.0,"cheek_mm":100.0,"head_mm":150.0,"sill_clearance_mm":80.0,"roof_pitch_deg":6.0,"roof_thickness_mm":65.0}"#;
        let spec: Dormer = serde_json::from_str(legacy).unwrap();
        assert_eq!(spec.yaw_deg, 0.);
        assert_eq!(serde_json::to_string(&spec).unwrap(), legacy);
        let schema = serde_json::to_value(schemars::schema_for!(Dormer)).unwrap();
        assert_eq!(schema["properties"]["yaw_deg"]["minimum"], -45.);
        assert_eq!(schema["properties"]["yaw_deg"]["maximum"], 45.);
        assert!(!schema["required"].as_array().unwrap().contains(&serde_json::json!("yaw_deg")));
        for kind in ["pitched", "mansard"] {
            let mut doc = Document::default();
            let mut b = Building::new("b", "Legacy yaw", &[[0., 0.], [30000., 0.], [30000., 10000.], [0., 10000.]], 3, 3400.).unwrap();
            let mut source = serde_json::json!({"kind":kind,"thickness_mm":220,"dormers":[spec]});
            if kind == "pitched" {
                source["pitch_deg"] = serde_json::json!(46);
            }
            b.roof = serde_json::from_value(source).unwrap();
            doc.buildings.push(b);
            let original = crate::build(&doc).unwrap();
            let old_bytes = serde_json::to_vec(&doc).unwrap();
            for yaw in [0., -0.] {
                doc.buildings[0].roof.dormers_mut().unwrap()[0].yaw_deg = yaw;
                assert_eq!(serde_json::to_vec(&doc).unwrap(), old_bytes);
                assert_eq!(crate::build(&doc).unwrap(), original);
            }
        }
    }

    #[test]
    fn mismatched_yaw_reports_the_required_source_orientation_without_snapping() {
        for positive in [false, true] {
            let (mut doc, spec) = skew_eave(positive, 71., [100000., -80000.], "pitched");
            doc.buildings[0].roof.dormers_mut().unwrap().push(spec.clone());
            for wrong in [0., -spec.yaw_deg, spec.yaw_deg + 0.001] {
                doc.buildings[0].roof.dormers_mut().unwrap()[0].yaw_deg = wrong;
                let before = doc.clone();
                let error = crate::build(&doc).unwrap_err().to_string();
                assert!(error.contains(&format!("requires yaw_deg={:.9} deg", spec.yaw_deg)), "{error}");
                for detail in ["skew", "b.f1", "mm/mm", "+Z", "u_mm=6000.000000 mm", "-45 to 45"] {
                    assert!(error.contains(detail), "{error}");
                }
                assert_eq!(doc, before);
            }
        }
    }

    #[test]
    fn yaw_is_bounded_finite_and_typed_before_building() {
        let (mut doc, spec) = skew_eave(false, 0., [0., 0.], "pitched");
        for yaw in [-45., 0., 45.] {
            let mut d = spec.clone();
            d.yaw_deg = yaw;
            d.validate().unwrap();
        }
        for yaw in [f64::NAN, f64::INFINITY, f64::NEG_INFINITY, f64::MAX, -45.000001, 45.000001] {
            let mut d = spec.clone();
            d.yaw_deg = yaw;
            assert!(d.validate().unwrap_err().to_string().contains("dormer yaw"));
            *doc.buildings[0].roof.dormers_mut().unwrap() = vec![d];
            assert!(crate::build(&doc).is_err());
        }
        for value in [serde_json::json!(null), serde_json::json!("2"), serde_json::json!([]), serde_json::json!({})] {
            let mut source = serde_json::to_value(&spec).unwrap();
            source["yaw_deg"] = value;
            assert!(serde_json::from_value::<Dormer>(source).is_err());
        }
        let mut source = serde_json::to_value(&spec).unwrap();
        source["ywa_deg"] = serde_json::json!(2);
        assert!(serde_json::from_value::<Dormer>(source).is_err());
    }

    #[test]
    fn yaw_limit_orientations_build_and_feedback_can_explain_an_unsupported_host_angle() {
        for positive in [false, true] {
            let (mut doc, mut spec) = skew_eave(positive, 0., [0., 0.], "pitched");
            let b = &mut doc.buildings[0];
            if positive {
                b.footprint[4][1] = -24000.;
                b.footprint[2][1] = 40000.;
                b.footprint[3][1] = 40000.;
            } else {
                b.footprint[2][1] = -24000.;
                b.footprint[3][1] = 40000.;
                b.footprint[4][1] = 40000.;
            }
            if let crate::Roof::Pitched { straighten_mm, .. } = &mut b.roof {
                *straighten_mm = 10000.;
            }
            assert_eq!(crate::roof::eave_outline(&b.footprint, 10000.).len(), 4);
            spec.yaw_deg = if positive { 45. } else { -45. };
            b.roof.dormers_mut().unwrap().push(spec);
            let built = crate::build(&doc).unwrap();
            let roof = built[0].roof.as_ref().unwrap();
            for mesh in [&roof.cover, &roof.dormers[0].front.mesh, &roof.dormers[0].cheeks, &roof.dormers[0].roof] {
                closed(mesh);
            }
            let b = &mut doc.buildings[0];
            b.footprint[if positive { 4 } else { 2 }][1] = -30000.;
            b.roof.dormers_mut().unwrap()[0].yaw_deg = 0.;
            let required = if positive { 1. } else { -1. } * (30000.0f64 / 24000.).atan().to_degrees();
            let error = crate::build(&doc).unwrap_err().to_string();
            assert!(error.contains(&format!("requires yaw_deg={required:.9} deg")), "{error}");
            assert!(error.contains("supported range is -45 to 45"), "{error}");
        }
    }

    #[test]
    fn end_feedback_quantifies_both_shortfalls_and_centre_limits_in_site_frames() {
        for (rotation, offset) in [(0.0_f64, [0., 0.]), (37., [410000., -270000.])] {
            let (sin, cos) = rotation.to_radians().sin_cos();
            let outline =
                [[0., 0.], [12000., 0.], [12000., 11000.], [0., 11000.]].map(|[x, y]| [offset[0] + x * cos - y * sin, offset[1] + x * sin + y * cos]);
            let mut b = Building::new("b", "Synthetic end clearance", &outline, 3, 3400.).unwrap();
            b.roof = serde_json::from_value(serde_json::json!({
                "kind":"pitched","pitch_deg":46,"thickness_mm":220,"gables":["b.f2","b.f4"]
            }))
            .unwrap();
            let mut doc = Document::default();
            doc.buildings.push(b);
            for (u, span, shortfalls) in [
                (937.5, "[87.500000, 1787.500000]", "start=12.500000 mm, end=0.000000 mm"),
                (11062.5, "[10212.500000, 11912.500000]", "start=0.000000 mm, end=12.500000 mm"),
            ] {
                *doc.buildings[0].roof.dormers_mut().unwrap() = vec![
                    serde_json::from_value(serde_json::json!({
                        "id":"end","facade":"b.f1","u_mm":u,"window_type":"window.casement.2","width_mm":1400,"height_mm":1000
                    }))
                    .unwrap(),
                ];
                let source = serde_json::to_vec(&doc).unwrap();
                let error = crate::build(&doc).unwrap_err().to_string();
                assert!(error.starts_with("invalid: dormer runs too close to a facade end"), "{error}");
                assert!(error.contains("dormer end on b.f1"), "{error}");
                assert!(error.contains(&format!("u_mm={u:.6} mm")), "{error}");
                assert!(error.contains(&format!("projected along span={span} mm")), "{error}");
                assert!(error.contains("permitted end range=[100.000000, 11900.000000] mm"), "{error}");
                assert!(error.contains(shortfalls), "{error}");
                assert!(error.contains("centre end limits=[950.000000, 11050.000000] mm"), "{error}");
                assert_eq!(serde_json::to_vec(&doc).unwrap(), source);
            }
            doc.buildings[0].roof.dormers_mut().unwrap()[0].u_mm = 975.;
            assert!(crate::build(&doc).is_ok(), "feedback must leave the guard's accepted geometry unchanged");
        }
    }

    #[test]
    fn end_feedback_uses_the_full_skew_envelope_projection() {
        for positive in [false, true] {
            for (rotation, offset) in [(0., [0., 0.]), (-118., [-30000., 41000.])] {
                let (mut doc, mut spec) = skew_eave(positive, rotation, offset, "pitched");
                doc.buildings[0].roof.dormers_mut().unwrap().push(spec.clone());
                let valid = crate::build(&doc).unwrap();
                let roof = &valid[0].roof.as_ref().unwrap().dormers[0].roof;
                let (a, b) = doc.buildings[0].edge(0).unwrap();
                let source = FacadeFrame::new(a, b).unwrap();
                // Independent reference: project an actually built shed solid, then move
                // its wall-line anchor. The roof depth and dimensions stay unchanged.
                let stations: Vec<_> = roof.positions.iter().map(|&p| local(&source, p)[0]).collect();
                let delta = if positive { 820. - spec.u_mm } else { 11180. - spec.u_mm };
                let expected = [
                    stations.iter().copied().fold(f64::INFINITY, f64::min) + delta,
                    stations.iter().copied().fold(f64::NEG_INFINITY, f64::max) + delta,
                ];
                spec.u_mm += delta;
                doc.buildings[0].roof.dormers_mut().unwrap()[0] = spec;
                let error = crate::build(&doc).unwrap_err().to_string();
                assert!(error.contains("dormer skew on b.f1"), "{error}");
                let span = error.split_once("projected along span=[").unwrap().1.split_once("] mm").unwrap().0;
                let found: Vec<f64> = span.split(", ").map(|s| s.parse().unwrap()).collect();
                assert_eq!(found.len(), 2);
                for (got, want) in found.iter().zip(expected) {
                    assert!((got - want).abs() < 1e-5, "{error}; expected {expected:?} mm");
                }
                assert!(error.contains("shortfall start=") && error.contains("centre end limits="), "{error}");
            }
        }
    }

    #[test]
    fn hip_feedback_identifies_the_sample_surface_and_height_residual_on_skew_hosts() {
        for kind in ["pitched", "mansard"] {
            for (rotation, offset) in [(0., [0., 0.]), (37., [410000., -270000.])] {
                let (mut doc, mut spec) = skew_eave(false, rotation, offset, kind);
                if let crate::Roof::Mansard { parameters } = &mut doc.buildings[0].roof {
                    parameters.break_run_mm = 1300.;
                    spec.height_mm = 2200.;
                } else {
                    spec.u_mm = 1600.;
                }
                let uncut = crate::build(&doc).unwrap();
                let host = &uncut[0].roof.as_ref().unwrap().cover;
                let b = &doc.buildings[0];
                let (a, e) = b.edge(0).unwrap();
                let anchor = FacadeFrame::new(a, e).unwrap().point(spec.u_mm, 0., 0.);
                let eave = FacadeFrame::new(b.footprint[0], b.footprint[2]).unwrap();
                let reference = FacadeFrame { origin: [anchor[0] - spec.u_mm * eave.t[0], anchor[1] - spec.u_mm * eave.t[1]], ..eave };
                doc.buildings[0].roof.dormers_mut().unwrap().push(spec.clone());
                let error = crate::build(&doc).unwrap_err().to_string();
                assert!(error.starts_with("invalid: dormer crosses a hip or slope break; reduce its width, height or setback"), "{error}");
                assert!(error.contains("dormer skew on b.f1") && error.contains("surface=top"), "{error}");
                let number = |label| error.split_once(label).unwrap().1.split_whitespace().next().unwrap().parse::<f64>().unwrap();
                let (u, w) = (number("sampled (u="), number("w="));
                let expected = plane(host, &reference, [spec.u_mm, spec.setback_mm + FRONT], true).unwrap().z(u, w);
                let found = plane(host, &reference, [u, w], true).unwrap().z(u, w);
                assert!((number("expected z=") - expected).abs() < 1e-5, "{error}");
                assert!((number("found z=") - found).abs() < 1e-5, "{error}");
                assert!((number("found-expected=") - (found - expected)).abs() < 1e-5, "{error}");
                assert!((found - expected).abs() > 0.01);
                assert!(error.contains("0.010000 mm absolute tolerance"), "{error}");
            }
        }
    }

    #[test]
    fn rotated_projection_limits_repetition_and_overlap_remain_enforced() {
        for positive in [false, true] {
            let (mut doc, mut spec) = skew_eave(positive, 0., [0., 0.], "pitched");
            spec.u_mm = if positive { 820. } else { 11180. };
            let half_extent = spec.width_mm / 2. + spec.cheek_mm + OVERHANG;
            assert!(spec.u_mm - half_extent > 100. && spec.u_mm + half_extent < 11900., "the unrotated width alone would fit");
            doc.buildings[0].roof.dormers_mut().unwrap().push(spec);
            assert!(crate::build(&doc).unwrap_err().to_string().contains("facade end"));
        }
        let (mut doc, mut spec) = skew_eave(false, 0., [0., 0.], "pitched");
        spec.u_mm = 4000.;
        let mut second = spec.clone();
        second.id = "second".into();
        second.u_mm += 1520.; // > old width + 100, < the yawed full-depth projections + 100.
        *doc.buildings[0].roof.dormers_mut().unwrap() = vec![spec.clone(), second.clone()];
        assert!(crate::build(&doc).unwrap_err().to_string().contains("100 mm between"));
        second.u_mm = 8000.;
        *doc.buildings[0].roof.dormers_mut().unwrap() = vec![spec, second];
        let built = crate::build(&doc).unwrap();
        closed(&built[0].roof.as_ref().unwrap().cover);
        assert_eq!(built, crate::build(&doc).unwrap());
        let (mut doc, mut spec) = skew_eave(false, 0., [0., 0.], "pitched");
        for p in &mut doc.buildings[0].footprint {
            p[0] *= 2.;
        }
        spec.yaw_deg = -(1000.0f64 / 48000.).atan().to_degrees();
        spec.width_mm = 800.;
        let original = crate::build(&doc).unwrap();
        let host = &original[0].roof.as_ref().unwrap().cover;
        for i in 0..16 {
            spec.id = format!("repeat-{i}");
            spec.u_mm = 3000. + f64::from(i) * 1300.;
            doc.buildings[0].roof.dormers_mut().unwrap().push(spec.clone());
        }
        let built = crate::build(&doc).unwrap();
        let roof = built[0].roof.as_ref().unwrap();
        assert_eq!(roof.dormers.len(), 16);
        closed(&roof.cover);
        for d in &roof.dormers {
            closed(&d.front.mesh);
            closed(&d.cheeks);
            closed(&d.roof);
            let q = [d.front.frame.length / 2., FRONT + 100.];
            for top in [true, false] {
                assert!(plane(host, &d.front.frame, q, top).is_ok());
                assert!(plane(&roof.cover, &d.front.frame, q, top).is_err(), "every repeated instance needs its own through-void");
            }
        }
        assert_eq!(built, crate::build(&doc).unwrap());
        spec.id = "seventeenth".into();
        doc.buildings[0].roof.dormers_mut().unwrap().push(spec);
        assert!(crate::build(&doc).is_err());
    }

    #[test]
    fn oriented_hosts_still_reject_slope_breaks_hips_thin_build_up_and_excess_depth() {
        let (mut doc, spec) = skew_eave(false, 0., [0., 0.], "mansard");
        doc.buildings[0].roof.dormers_mut().unwrap().push(spec);
        if let crate::Roof::Mansard { parameters } = &mut doc.buildings[0].roof {
            parameters.break_run_mm = 1300.;
            parameters.dormers[0].height_mm = 2200.;
        }
        assert!(crate::build(&doc).unwrap_err().to_string().contains("slope break"));
        let (mut doc, mut spec) = skew_eave(false, 0., [0., 0.], "pitched");
        spec.u_mm = 1600.;
        doc.buildings[0].roof.dormers_mut().unwrap().push(spec);
        assert!(crate::build(&doc).unwrap_err().to_string().contains("slope break"));
        let (mut doc, spec) = skew_eave(false, 0., [0., 0.], "pitched");
        doc.buildings[0].roof.dormers_mut().unwrap().push(spec);
        if let crate::Roof::Pitched { thickness_mm, .. } = &mut doc.buildings[0].roof {
            *thickness_mm = 20.;
        }
        assert!(crate::build(&doc).unwrap_err().to_string().contains("50 mm of build-up"));
        if let crate::Roof::Pitched { thickness_mm, pitch_deg, dormers, .. } = &mut doc.buildings[0].roof {
            *thickness_mm = 220.;
            *pitch_deg = 5.;
            dormers[0].roof_pitch_deg = 15.;
        }
        assert!(crate::build(&doc).unwrap_err().to_string().contains("steeper"));
        doc.buildings[0].roof.dormers_mut().unwrap()[0].roof_pitch_deg = 0.;
        if let crate::Roof::Pitched { pitch_deg, .. } = &mut doc.buildings[0].roof {
            *pitch_deg = 6.;
        }
        assert!(crate::build(&doc).unwrap_err().to_string().contains("depth must be 200–5000 mm"));
    }

    #[test]
    fn oriented_cheeks_need_positive_height_through_the_rear_underlap() {
        let (mut doc, spec) = skew_eave(false, 0., [0., 0.], "pitched");
        doc.buildings[0].roof.dormers_mut().unwrap().push(spec);
        if let crate::Roof::Pitched { thickness_mm, .. } = &mut doc.buildings[0].roof {
            *thickness_mm = 50.; // > 50 mm vertically, but too thin to seat the rear cheek.
        }
        let result = crate::build(&doc);
        let minimum_height = result.as_ref().ok().map(|b| {
            let p = &b[0].roof.as_ref().unwrap().dormers[0].cheeks.positions;
            (0..4).map(|k| p[4 + k][2] - p[3 - k][2]).fold(f64::INFINITY, f64::min)
        });
        assert!(result.is_err(), "reject a cheek whose top crosses below its bottom inside the host: minimum height {minimum_height:?} mm");
        assert!(result.unwrap_err().to_string().contains("cheek"));
        if let crate::Roof::Pitched { thickness_mm, .. } = &mut doc.buildings[0].roof {
            *thickness_mm = 100.;
        }
        let built = crate::build(&doc).unwrap();
        closed(&built[0].roof.as_ref().unwrap().dormers[0].cheeks);
    }

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
