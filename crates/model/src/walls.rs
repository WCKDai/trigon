//! Facade walls by storey. Each storey's piece of a facade is a closed prism (`WallSegment`) at
//! that storey's thickness, from the underside of its floor to the underside of the next one, so
//! the wall steps on the inside at every floor while the outer face stays flush (Copenhagen
//! masonry: 3 sten in the basement, 2½ on the ground floor, 2 and 1½ higher up). The floors
//! rest on the steps. A second mesh, the skin, is the whole facade without the faces where
//! storeys meet, for display and elevations. Openings are cut as 2D holes in each face with
//! polygon booleans, so an opening that crosses a floor (a stairwell window) is cut in both
//! storeys; no 3D booleans.

use i_overlay::core::{fill_rule::FillRule, overlay_rule::OverlayRule};
use i_overlay::float::overlay::FloatOverlay;

use crate::build::{FacadeFrame, Rect};
use crate::mesh::Mesh;

/// One storey's piece of a facade wall.
#[derive(Debug, Clone, PartialEq)]
pub struct WallSegment {
    /// `<facade>.L<level>`.
    pub id: String,
    pub level: usize,
    pub thickness: f64,
    /// Bottom and top, mm.
    pub z0: f64,
    pub z1: f64,
    /// Plan outline: outer start, outer end, inner end, inner start.
    pub plan: [[f64; 2]; 4],
    /// A closed prism with the openings cut out.
    pub mesh: Mesh,
    /// The wall's openings (indices into `BuiltWall::openings`) that cut this piece.
    pub openings: Vec<usize>,
}

/// What a storey's piece of one facade spans.
#[derive(Debug, Clone, Copy)]
pub(crate) struct Span {
    pub level: usize,
    pub thickness: f64,
    pub z0: f64,
    pub z1: f64,
    /// Inner face's start and end along the facade (mitred at the corners).
    pub in0: f64,
    pub in1: f64,
    /// Inner face's start and end in plan.
    pub qs: [f64; 2],
    pub qe: [f64; 2],
}

// Numerical boundary tolerance, not a modelling clearance: sub-millimetre
// intersections must cut vertical faces, reveals and caps consistently.
const EPS: f64 = 1e-7;

type Shapes = Vec<Vec<Vec<[f64; 2]>>>;

/// `outer` minus every polygon in `cuts`, as shapes (an outer contour and its holes).
pub(crate) fn cut(outer: &[[f64; 2]], cuts: &[Vec<[f64; 2]>]) -> Shapes {
    if cuts.is_empty() {
        return vec![vec![outer.to_vec()]];
    }
    let subj = vec![outer.to_vec()];
    FloatOverlay::with_subj_and_clip(&subj, &cuts.to_vec()).overlay(OverlayRule::Difference, FillRule::NonZero)
}

fn rect(u0: f64, u1: f64, v0: f64, v1: f64) -> Vec<[f64; 2]> {
    vec![[u0, v0], [u1, v0], [u1, v1], [u0, v1]]
}

/// Turns every triangle of `m` to face `normal`.
fn orient(m: &mut Mesh, normal: [f64; 3]) {
    for t in &mut m.triangles {
        let p = |i: u32| m.positions.get(i as usize).copied().unwrap_or_default();
        let (a, b, c) = (p(t[0]), p(t[1]), p(t[2]));
        let (e, f) = ([b[0] - a[0], b[1] - a[1], b[2] - a[2]], [c[0] - a[0], c[1] - a[1], c[2] - a[2]]);
        let n = [e[1] * f[2] - e[2] * f[1], e[2] * f[0] - e[0] * f[2], e[0] * f[1] - e[1] * f[0]];
        if n[0] * normal[0] + n[1] * normal[1] + n[2] * normal[2] < 0.0 {
            t.swap(1, 2);
        }
    }
}

/// Triangulates shapes, maps them to 3D and adds them facing `normal`. False if any shape
/// couldn't be triangulated.
fn fill(m: &mut Mesh, shapes: &Shapes, to3d: impl Fn([f64; 2]) -> [f64; 3], normal: [f64; 3]) -> bool {
    let mut part = Mesh::default();
    let mut ok = true;
    for shape in shapes {
        let Some((outer, holes)) = shape.split_first() else { continue };
        ok &= part.polygon(outer, holes, &to3d);
    }
    orient(&mut part, normal);
    m.append(&part);
    ok
}

fn quad(m: &mut Mesh, q: [[f64; 3]; 4], normal: [f64; 3]) {
    let mut part = Mesh::default();
    part.quad(q[0], q[1], q[2], q[3]);
    orient(&mut part, normal);
    m.append(&part);
}

/// The plan band of a span in facade (u, w) coordinates.
fn band(fr: &FacadeFrame, s: &Span) -> Vec<[f64; 2]> {
    let [d0, d1] = inner_depths(fr, s);
    vec![[0.0, 0.0], [fr.length, 0.0], [s.in1, d1], [s.in0, d0]]
}

fn inner_depths(fr: &FacadeFrame, s: &Span) -> [f64; 2] {
    [s.qs, s.qe].map(|q| {
        let d = -(q[0] - fr.origin[0]) * fr.n[0] - (q[1] - fr.origin[1]) * fr.n[1];
        // Keep the established parallel-face arithmetic when the only difference is
        // projection roundoff. Joined/skew faces retain their actual endpoints.
        if (d - s.thickness).abs() < 1e-6 { s.thickness } else { d }
    })
}

fn inner_point(fr: &FacadeFrame, s: &Span, u: f64, z: f64) -> [f64; 3] {
    let [d0, d1] = inner_depths(fr, s);
    let d = if d0 == d1 { d0 } else { d0 + (d1 - d0) * (u - s.in0) / (s.in1 - s.in0) };
    fr.point(u, d, z)
}

fn inner_normal(fr: &FacadeFrame, s: &Span) -> [f64; 3] {
    let [d0, d1] = inner_depths(fr, s);
    let slope = (d1 - d0) / (s.in1 - s.in0);
    [-fr.n[0] - slope * fr.t[0], -fr.n[1] - slope * fr.t[1], 0.0]
}

fn depth(fr: &FacadeFrame, s: &Span) -> f64 {
    inner_depths(fr, s).into_iter().fold(s.thickness, f64::max)
}

/// Intersection of the actual convex plan with a plane of constant facade U.
fn section(plan: &[[f64; 2]], u: f64) -> Option<[f64; 2]> {
    let mut range = [f64::INFINITY, f64::NEG_INFINITY];
    for (&a, &b) in plan.iter().zip(plan.iter().cycle().skip(1)).take(plan.len()) {
        if u < a[0].min(b[0]) - 1e-8 || u > a[0].max(b[0]) + 1e-8 {
            continue;
        }
        let values = if (a[0] - b[0]).abs() < 1e-8 {
            [a[1], b[1]]
        } else {
            let w = a[1] + (b[1] - a[1]) * ((u - a[0]) / (b[0] - a[0])).clamp(0.0, 1.0);
            [w, w]
        };
        for w in values {
            range[0] = range[0].min(w);
            range[1] = range[1].max(w);
        }
    }
    (range[1] >= range[0]).then_some(range)
}

/// Extrude an opening edge only through the wall's actual plan. Changes in the
/// mitred boundary split the strip, including a triangular tip at a wall corner.
fn reveal_edge(m: &mut Mesh, fr: &FacadeFrame, s: &Span, a: [f64; 2], b: [f64; 2], normal: [f64; 3]) {
    let plan = band(fr, s);
    let mut stations = vec![0.0, 1.0];
    if (b[0] - a[0]).abs() > 1e-8 {
        stations.extend(plan.iter().map(|p| (p[0] - a[0]) / (b[0] - a[0])).filter(|t| *t > 0.0 && *t < 1.0));
    } else {
        let min_u = plan.iter().map(|p| p[0]).fold(f64::INFINITY, f64::min);
        let max_u = plan.iter().map(|p| p[0]).fold(f64::NEG_INFINITY, f64::max);
        if a[0] <= min_u + 1e-8 || a[0] >= max_u - 1e-8 {
            return;
        }
    }
    stations.sort_by(f64::total_cmp);
    stations.dedup_by(|a, b| (*a - *b).abs() < 1e-10);
    let at = |t: f64| [a[0] + (b[0] - a[0]) * t, a[1] + (b[1] - a[1]) * t];
    for pair in stations.windows(2) {
        let (a, b) = (at(pair[0]), at(pair[1]));
        let (Some(wa), Some(wb)) = (section(&plan, a[0]), section(&plan, b[0])) else { continue };
        if (wa[1] - wa[0]).max(wb[1] - wb[0]) < 1e-8 {
            continue;
        }
        let mut part = Mesh::default();
        part.quad(fr.point(a[0], wa[0], a[1]), fr.point(b[0], wb[0], b[1]), fr.point(b[0], wb[1], b[1]), fr.point(a[0], wa[1], a[1]));
        part.triangles.retain(|tri| {
            let [a, b, c] = tri.map(|i| part.positions[i as usize]);
            let ab = std::array::from_fn::<_, 3, _>(|i| b[i] - a[i]);
            let ac = std::array::from_fn::<_, 3, _>(|i| c[i] - a[i]);
            (ab[1] * ac[2] - ab[2] * ac[1]).abs() + (ab[2] * ac[0] - ab[0] * ac[2]).abs() + (ab[0] * ac[1] - ab[1] * ac[0]).abs() > 1e-12
        });
        orient(&mut part, normal);
        m.append(&part);
    }
}

/// An opening's footprint through the wall, in (u, w).
fn through(r: &Rect, depth: f64) -> Vec<[f64; 2]> {
    rect(r.u0, r.u1, -1.0, depth + 1.0)
}

/// Openings that cross the height `z`.
fn crossing(holes: &[Rect], z: f64, depth: f64) -> Vec<Vec<[f64; 2]>> {
    holes.iter().filter(|r| r.v0 < z && r.v1 > z).map(|r| through(r, depth)).collect()
}

/// Touching voids have one boundary against material. Keep disjoint openings on
/// their established path; only merge when their facade-plane bounds meet.
fn merged_opening_edges(holes: &[Vec<[f64; 2]>]) -> Option<Vec<Vec<[f64; 2]>>> {
    let mut bounds: Vec<_> = holes
        .iter()
        .map(|p| {
            p.iter().fold([f64::INFINITY, f64::NEG_INFINITY, f64::INFINITY, f64::NEG_INFINITY], |a, p| {
                [a[0].min(p[0]), a[1].max(p[0]), a[2].min(p[1]), a[3].max(p[1])]
            })
        })
        .collect();
    bounds.sort_by(|a, b| a[0].total_cmp(&b[0]));
    let meet = bounds.iter().enumerate().any(|(i, a)| bounds[i + 1..].iter().take_while(|b| b[0] <= a[1]).any(|b| b[2] <= a[3] && b[3] >= a[2]));
    if !meet {
        return None;
    }
    let empty: Vec<Vec<[f64; 2]>> = Vec::new();
    let mut merged = FloatOverlay::with_subj_and_clip(&holes.to_vec(), &empty).overlay(OverlayRule::Union, FillRule::NonZero);
    for shape in &mut merged {
        for (i, ring) in shape.iter_mut().enumerate() {
            if (crate::footprint::signed_area2(ring) > 0.0) != (i == 0) {
                ring.reverse();
            }
        }
    }
    Some(merged.into_iter().flatten().collect())
}

/// The inner face of a span with its holes, and the reveals of every opening within it.
fn inner_and_reveals(m: &mut Mesh, fr: &FacadeFrame, s: &Span, holes: &[Rect]) -> bool {
    let t = [fr.t[0], fr.t[1], 0.0];
    let mine: Vec<Rect> = holes
        .iter()
        .filter(|r| r.v0 < s.z1 - EPS && r.v1 > s.z0 + EPS)
        .map(|r| Rect { u0: r.u0, u1: r.u1, v0: r.v0.max(s.z0), v1: r.v1.min(s.z1) })
        .collect();
    let cuts: Vec<_> = mine.iter().map(|r| rect(r.u0, r.u1, r.v0, r.v1)).collect();
    let ok = fill(m, &cut(&rect(s.in0, s.in1, s.z0, s.z1), &cuts), |p| inner_point(fr, s, p[0], p[1]), inner_normal(fr, s));
    if let Some(edges) = merged_opening_edges(&cuts) {
        profile_reveals(m, fr, s, &edges);
        return ok;
    }
    let plan = band(fr, s);
    let min_u = plan.iter().map(|p| p[0]).fold(f64::INFINITY, f64::min);
    let max_u = plan.iter().map(|p| p[0]).fold(f64::NEG_INFINITY, f64::max);
    for (r, whole) in mine.iter().zip(holes.iter().filter(|r| r.v0 < s.z1 - EPS && r.v1 > s.z0 + EPS)) {
        let Rect { u0, u1, v0, v1 } = *r;
        for (u, solid_exists, normal) in [(u0, u0 > min_u + 1e-8, t), (u1, u1 < max_u - 1e-8, [-t[0], -t[1], 0.0])] {
            if solid_exists
                && let Some([a, b]) = section(&plan, u)
                && b - a > 1e-8
            {
                quad(m, [fr.point(u, a, v0), fr.point(u, b, v0), fr.point(u, b, v1), fr.point(u, a, v1)], normal);
            }
        }
        // Sill and head only where the opening really ends, not where it crosses a floor.
        if (whole.v0 - v0).abs() < EPS && v0 > s.z0 + EPS {
            reveal_edge(m, fr, s, [u0, v0], [u1, v0], [0.0, 0.0, 1.0]);
        }
        if (whole.v1 - v1).abs() < EPS && v1 < s.z1 - EPS {
            reveal_edge(m, fr, s, [u0, v1], [u1, v1], [0.0, 0.0, -1.0]);
        }
    }
    ok
}

/// The horizontal face of a span's band at `z`, minus the openings crossing it.
fn cap(m: &mut Mesh, fr: &FacadeFrame, s: &Span, z: f64, up: bool, holes: &[Rect]) -> bool {
    let inside_z = z + if up { -1e-7 } else { 1e-7 };
    let shapes = cut(&band(fr, s), &crossing(holes, inside_z, depth(fr, s)));
    fill(m, &shapes, |p| fr.point(p[0], p[1], z), [0.0, 0.0, if up { 1.0 } else { -1.0 }])
}

/// The mitred ends of a span, facing away from it.
fn ends(m: &mut Mesh, fr: &FacadeFrame, s: &Span, holes: &[Vec<[f64; 2]>]) -> bool {
    let (a, e) = (fr.point(0.0, 0.0, 0.0), fr.point(fr.length, 0.0, 0.0));
    let mid = [(a[0] + e[0] + s.qs[0] + s.qe[0]) / 4.0, (a[1] + e[1] + s.qs[1] + s.qe[1]) / 4.0];
    let mut ok = true;
    for (o, q) in [([a[0], a[1]], s.qs), ([e[0], e[1]], s.qe)] {
        let c = [(o[0] + q[0]) / 2.0 - mid[0], (o[1] + q[1]) / 2.0 - mid[1], 0.0];
        let along = |p: [f64; 2]| (p[0] - fr.origin[0]) * fr.t[0] + (p[1] - fr.origin[1]) * fr.t[1];
        let (u0, u1) = (along(o), along(q));
        let length = (q[0] - o[0]).hypot(q[1] - o[1]);
        let cuts: Vec<_> = holes
            .iter()
            .filter_map(|poly| {
                if (u1 - u0).abs() < 1e-8 {
                    // At a square end inspect from the material side, so a void that
                    // only touches the outside of the wall cannot remove its end face.
                    let inside_u = u0 + (along(mid) - u0).signum() * 1e-7;
                    let transposed: Vec<_> = poly.iter().map(|p| [p[1], p[0]]).collect();
                    let (z0, z1) = crate::profile::width_at(&transposed, inside_u)?;
                    (z1 > s.z0 && z0 < s.z1).then(|| rect(0.0, length, z0, z1))
                } else {
                    let range = poly.iter().map(|p| p[0]).fold([f64::INFINITY, f64::NEG_INFINITY], |[lo, hi], u| [lo.min(u), hi.max(u)]);
                    if range[1] <= u0.min(u1) || range[0] >= u0.max(u1) {
                        return None;
                    }
                    Some(poly.iter().map(|p| [(p[0] - u0) * length / (u1 - u0), p[1]]).collect())
                }
            })
            .collect();
        if cuts.is_empty() {
            quad(m, [[o[0], o[1], s.z0], [q[0], q[1], s.z0], [q[0], q[1], s.z1], [o[0], o[1], s.z1]], c);
        } else {
            ok &= fill(
                m,
                &cut(&rect(0.0, length, s.z0, s.z1), &cuts),
                |p| [o[0] + (q[0] - o[0]) * p[0] / length, o[1] + (q[1] - o[1]) * p[0] / length, p[1]],
                c,
            );
        }
    }
    ok
}

/// One storey's closed piece of the wall. `None` if a face can't be triangulated.
pub(crate) fn segment(fr: &FacadeFrame, id: &str, s: &Span, holes: &[Rect]) -> Option<WallSegment> {
    let mut m = Mesh::default();
    let n = [fr.n[0], fr.n[1], 0.0];
    let cuts: Vec<_> = holes.iter().filter(|r| r.v0 < s.z1 - EPS && r.v1 > s.z0 + EPS).map(|r| rect(r.u0, r.u1, r.v0, r.v1)).collect();
    let mut ok = fill(&mut m, &cut(&rect(0.0, fr.length, s.z0, s.z1), &cuts), |p| fr.point(p[0], 0.0, p[1]), n);
    ok &= inner_and_reveals(&mut m, fr, s, holes);
    ok &= cap(&mut m, fr, s, s.z1, true, holes);
    ok &= cap(&mut m, fr, s, s.z0, false, holes);
    ok &= ends(&mut m, fr, s, &cuts);
    let (a, e) = (fr.point(0.0, 0.0, 0.0), fr.point(fr.length, 0.0, 0.0));
    let plan = [[a[0], a[1]], [e[0], e[1]], s.qe, s.qs];
    let openings = holes.iter().enumerate().filter(|(_, r)| r.v0 < s.z1 - EPS && r.v1 > s.z0 + EPS).map(|(k, _)| k).collect();
    ok.then(|| WallSegment { id: id.to_string(), level: s.level, thickness: s.thickness, z0: s.z0, z1: s.z1, plan, mesh: m, openings })
}

/// The whole facade wall without the faces where storeys meet: one outer face, each storey's
/// inner face and mitred ends, the steps between them, the top and the bottom. `None` if a face can't be
/// triangulated.
pub(crate) fn skin(fr: &FacadeFrame, spans: &[Span], holes: &[Rect]) -> Option<Mesh> {
    let (first, last) = (spans.first()?, spans.last()?);
    let mut m = Mesh::default();
    let cuts: Vec<_> = holes.iter().map(|r| rect(r.u0, r.u1, r.v0, r.v1)).collect();
    let mut ok = fill(&mut m, &cut(&rect(0.0, fr.length, first.z0, last.z1), &cuts), |p| fr.point(p[0], 0.0, p[1]), [fr.n[0], fr.n[1], 0.0]);
    for s in spans {
        ok &= inner_and_reveals(&mut m, fr, s, holes);
        ok &= ends(&mut m, fr, s, &cuts);
    }
    ok &= cap(&mut m, fr, last, last.z1, true, holes);
    ok &= cap(&mut m, fr, first, first.z0, false, holes);
    // Each side of a storey joint can expose material, including crossing inner
    // edges and an opening that starts or ends exactly at this floor.
    for pair in spans.windows(2) {
        let (lo, hi) = (&pair[0], &pair[1]);
        let z = lo.z1;
        let d = depth(fr, lo).max(depth(fr, hi));
        ok &= step(&mut m, fr, lo, hi, crossing(holes, z - EPS, d), crossing(holes, z + EPS, d));
    }
    ok.then_some(m)
}

/// Convex curved openings use their actual outline on both wall faces and every reveal.
/// Kept separate from the established rectangular path to retain old drawing bytes.
pub(crate) fn skin_profiles(fr: &FacadeFrame, spans: &[Span], holes: &[Vec<[f64; 2]>]) -> Option<Mesh> {
    let (first, last) = (spans.first()?, spans.last()?);
    let mut m = Mesh::default();
    let mut ok = fill(&mut m, &cut(&rect(0.0, fr.length, first.z0, last.z1), holes), |p| fr.point(p[0], 0.0, p[1]), [fr.n[0], fr.n[1], 0.0]);
    for s in spans {
        ok &= profile_inner(&mut m, fr, s, holes);
        ok &= ends(&mut m, fr, s, holes);
    }
    ok &= profile_cap(&mut m, fr, last, last.z1, true, holes);
    ok &= profile_cap(&mut m, fr, first, first.z0, false, holes);
    for pair in spans.windows(2) {
        let (lo, hi) = (&pair[0], &pair[1]);
        let d = depth(fr, lo).max(depth(fr, hi));
        ok &= step(&mut m, fr, lo, hi, profile_crossing(holes, lo.z1 - EPS, d), profile_crossing(holes, lo.z1 + EPS, d));
    }
    ok.then_some(m)
}

fn step(m: &mut Mesh, fr: &FacadeFrame, lo: &Span, hi: &Span, below_cuts: Vec<Vec<[f64; 2]>>, above_cuts: Vec<Vec<[f64; 2]>>) -> bool {
    let (below_plan, above_plan) = (band(fr, lo), band(fr, hi));
    let mut ok = true;
    if below_cuts == above_cuts {
        if below_plan == above_plan {
            return true;
        }
        // Common voids can be subtracted in the same overlay as the adjoining
        // band, preserving the established arithmetic for ordinary shoulders.
        for (plan, adjoining, normal) in [(&below_plan, &above_plan, 1.0), (&above_plan, &below_plan, -1.0)] {
            let mut cuts = below_cuts.clone();
            cuts.push(adjoining.clone());
            ok &= fill(m, &cut(plan, &cuts), |p| fr.point(p[0], p[1], lo.z1), [0.0, 0.0, normal]);
        }
    } else {
        let below = cut(&below_plan, &below_cuts);
        let above = cut(&above_plan, &above_cuts);
        for (material, adjoining, normal) in [(&below, &above, 1.0), (&above, &below, -1.0)] {
            let exposed = if adjoining.is_empty() {
                material.clone()
            } else {
                FloatOverlay::with_subj_and_clip(material, adjoining).overlay(OverlayRule::Difference, FillRule::NonZero)
            };
            ok &= fill(m, &exposed, |p| fr.point(p[0], p[1], lo.z1), [0.0, 0.0, normal]);
        }
    }
    ok
}

fn profile_crossing(holes: &[Vec<[f64; 2]>], z: f64, depth: f64) -> Vec<Vec<[f64; 2]>> {
    holes.iter().filter_map(|p| crate::profile::width_at(p, z)).filter(|(a, b)| b - a > 1e-6).map(|(a, b)| rect(a, b, -1.0, depth + 1.0)).collect()
}

fn profile_cap(m: &mut Mesh, fr: &FacadeFrame, s: &Span, z: f64, up: bool, holes: &[Vec<[f64; 2]>]) -> bool {
    // Approach from inside the span so an opening at the floor also cuts its bottom cap.
    let inside_z = z + if up { -1e-7 } else { 1e-7 };
    fill(
        m,
        &cut(&band(fr, s), &profile_crossing(holes, inside_z, depth(fr, s))),
        |p| fr.point(p[0], p[1], z),
        [0.0, 0.0, if up { 1.0 } else { -1.0 }],
    )
}

fn profile_inner(m: &mut Mesh, fr: &FacadeFrame, s: &Span, holes: &[Vec<[f64; 2]>]) -> bool {
    let ok = fill(m, &cut(&rect(s.in0, s.in1, s.z0, s.z1), holes), |p| inner_point(fr, s, p[0], p[1]), inner_normal(fr, s));
    let merged = merged_opening_edges(holes);
    profile_reveals(m, fr, s, merged.as_deref().unwrap_or(holes));
    ok
}

fn profile_reveals(m: &mut Mesh, fr: &FacadeFrame, s: &Span, edges: &[Vec<[f64; 2]>]) {
    for poly in edges {
        for i in 0..poly.len() {
            let (mut a, mut b) = (poly[i], poly[(i + 1) % poly.len()]);
            if a[1].max(b[1]) < s.z0 || a[1].min(b[1]) > s.z1 {
                continue;
            }
            let (original_a, original_b) = (a, b);
            if (a[1] - b[1]).abs() < 1e-9 {
                if a[1] <= s.z0 + 1e-8 || a[1] >= s.z1 - 1e-8 {
                    continue;
                }
            } else {
                let clip = |p: [f64; 2]| {
                    let z = p[1].clamp(s.z0, s.z1);
                    [original_a[0] + (original_b[0] - original_a[0]) * (z - original_a[1]) / (original_b[1] - original_a[1]), z]
                };
                a = clip(a);
                b = clip(b);
            }
            if (a[0] - b[0]).hypot(a[1] - b[1]) < 1e-8 {
                continue;
            }
            let normal = [-(b[1] - a[1]) * fr.t[0], -(b[1] - a[1]) * fr.t[1], b[0] - a[0]];
            reveal_edge(m, fr, s, a, b, normal);
        }
    }
}

pub(crate) fn segment_profiles(fr: &FacadeFrame, id: &str, s: &Span, holes: &[Vec<[f64; 2]>]) -> Option<WallSegment> {
    let mut m = Mesh::default();
    let mut ok = fill(&mut m, &cut(&rect(0.0, fr.length, s.z0, s.z1), holes), |p| fr.point(p[0], 0.0, p[1]), [fr.n[0], fr.n[1], 0.0]);
    ok &= profile_inner(&mut m, fr, s, holes);
    ok &= profile_cap(&mut m, fr, s, s.z0, false, holes);
    ok &= profile_cap(&mut m, fr, s, s.z1, true, holes);
    ok &= ends(&mut m, fr, s, holes);
    let openings = holes.iter().enumerate().filter(|(_, p)| p.iter().any(|p| p[1] > s.z0) && p.iter().any(|p| p[1] < s.z1)).map(|(i, _)| i).collect();
    let (a, b) = (fr.point(0.0, 0.0, 0.0), fr.point(fr.length, 0.0, 0.0));
    ok.then(|| WallSegment {
        id: id.into(),
        level: s.level,
        thickness: s.thickness,
        z0: s.z0,
        z1: s.z1,
        plan: [[a[0], a[1]], [b[0], b[1]], s.qe, s.qs],
        mesh: m,
        openings,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn frame() -> FacadeFrame {
        FacadeFrame::new([0.0, 0.0], [10000.0, 0.0]).unwrap_or(FacadeFrame { origin: [0.0, 0.0], t: [1.0, 0.0], n: [0.0, -1.0], length: 10000.0 })
    }

    fn span(level: usize, thickness: f64, z0: f64, z1: f64) -> Span {
        // A straight wall with square ends: the inner face runs the full length.
        Span { level, thickness, z0, z1, in0: 0.0, in1: 10000.0, qs: [0.0, thickness], qe: [10000.0, thickness] }
    }

    /// Volume of a closed, consistently oriented mesh (divergence theorem).
    fn volume(m: &Mesh) -> f64 {
        m.triangles
            .iter()
            .map(|t| {
                let [a, b, c] = t.map(|i| m.positions[i as usize]);
                (a[0] * (b[1] * c[2] - b[2] * c[1]) - a[1] * (b[0] * c[2] - b[2] * c[0]) + a[2] * (b[0] * c[1] - b[1] * c[0])) / 6.0
            })
            .sum()
    }

    fn assert_closed(m: &Mesh) {
        use std::collections::{BTreeMap, BTreeSet};
        let key = |p: [f64; 3]| p.map(|v| (v * 1000.0).round() as i64);
        let mut edges = BTreeMap::<_, (usize, i32)>::new();
        let mut faces = BTreeSet::new();
        for tri in &m.triangles {
            let points = tri.map(|i| m.positions[i as usize]);
            assert!(points.iter().flatten().all(|v| v.is_finite()));
            let mut face = points.map(key);
            face.sort();
            assert!(faces.insert(face), "duplicate triangle: {points:?}");
            for i in 0..3 {
                let (a, b) = (key(points[i]), key(points[(i + 1) % 3]));
                assert_ne!(a, b, "collapsed edge: {points:?}");
                let edge = edges.entry([a.min(b), a.max(b)]).or_default();
                edge.0 += 1;
                edge.1 += if a < b { 1 } else { -1 };
            }
        }
        let bad: Vec<_> = edges.iter().filter(|(_, v)| **v != (2, 0)).collect();
        assert!(bad.is_empty(), "open or inconsistent solid: {bad:?}");
        assert!(volume(m) > 0.0);
    }

    #[test]
    fn a_segment_is_a_closed_prism_less_its_openings() {
        let fr = frame();
        let s = span(1, 600.0, 0.0, 3000.0);
        let window = Rect { u0: 2000.0, u1: 3200.0, v0: 900.0, v1: 2400.0 };
        let seg = segment(&fr, "w.L1", &s, &[window]).unwrap();
        let expected = 10000.0 * 600.0 * 3000.0 - 1200.0 * 600.0 * 1500.0;
        assert!((volume(&seg.mesh) - expected).abs() < 1.0, "{} vs {expected}", volume(&seg.mesh));
        assert_eq!(seg.openings, vec![0]);
    }

    #[test]
    fn an_opening_across_a_floor_cuts_both_storeys_and_the_skin_steps_inside() {
        let fr = frame();
        let spans = [span(0, 600.0, 0.0, 3000.0), span(1, 360.0, 3000.0, 6000.0)];
        // A stairwell window from 2000 to 4000 mm crosses the floor at 3000.
        let stair_window = Rect { u0: 4000.0, u1: 5000.0, v0: 2000.0, v1: 4000.0 };
        let lower = segment(&fr, "w.L0", &spans[0], &[stair_window]).unwrap();
        let upper = segment(&fr, "w.L1", &spans[1], &[stair_window]).unwrap();
        let v0 = 10000.0 * 600.0 * 3000.0 - 1000.0 * 600.0 * 1000.0;
        let v1 = 10000.0 * 360.0 * 3000.0 - 1000.0 * 360.0 * 1000.0;
        assert!((volume(&lower.mesh) - v0).abs() < 1.0 && (volume(&upper.mesh) - v1).abs() < 1.0);
        let skin = skin(&fr, &spans, &[stair_window]).unwrap();
        assert!((volume(&skin) - (v0 + v1)).abs() < 1.0, "the skin is closed: {} vs {}", volume(&skin), v0 + v1);
    }
    #[test]
    fn floor_touching_door_segment_is_closed_at_zero_and_nonzero_elevation() {
        let fr = frame();
        for elevation in [0.0, 5000.0] {
            let s = span(1, 600.0, elevation, elevation + 3000.0);
            let door = Rect { u0: 2000.0, u1: 3200.0, v0: elevation, v1: elevation + 2100.0 };
            let seg = segment(&fr, "synthetic.L1", &s, &[door]).unwrap();
            assert_closed(&seg.mesh);
            let expected = 10000.0 * 600.0 * 3000.0 - 1200.0 * 600.0 * 2100.0;
            assert!((volume(&seg.mesh) - expected).abs() < 1.0, "{elevation}: {}", volume(&seg.mesh));

            let mut area_vector = [0.0; 3];
            for tri in &seg.mesh.triangles {
                let [a, b, c] = tri.map(|i| seg.mesh.positions[i as usize]);
                let ab = std::array::from_fn::<_, 3, _>(|axis| b[axis] - a[axis]);
                let ac = std::array::from_fn::<_, 3, _>(|axis| c[axis] - a[axis]);
                let cross = [ab[1] * ac[2] - ab[2] * ac[1], ab[2] * ac[0] - ab[0] * ac[2], ab[0] * ac[1] - ab[1] * ac[0]];
                for axis in 0..3 {
                    area_vector[axis] += cross[axis] / 2.0;
                }
            }
            assert!(area_vector.iter().all(|value| value.abs() < 1e-3), "{area_vector:?}");
        }
    }

    // Case 2 isolates the quadrilateral/reference-depth contract without openings.
    // The actual material outline has an oblique inner edge. Rotation makes the
    // wrong constant-depth endpoint enlarge the world AABB by 146.4101615 mm.

    #[test]
    fn oblique_inner_edge_segment_bounds_match_its_declared_plan_quad() {
        let angle = 30_f64.to_radians();
        let (sin, cos) = angle.sin_cos();
        let at = |[x, y]: [f64; 2]| [x * cos - y * sin, x * sin + y * cos];
        let fr = FacadeFrame::new([0.0, 0.0], at([1000.0, 0.0])).unwrap();
        let s = Span { level: 0, thickness: 400.0, z0: 0.0, z1: 3000.0, in0: -300.0, in1: 600.0, qs: at([-300.0, 400.0]), qe: at([600.0, 200.0]) };
        let seg = segment(&fr, "synthetic.L0", &s, &[]).unwrap();
        assert_closed(&seg.mesh);
        let (lo, hi) = seg.mesh.bounds().unwrap();
        let expected_min_y = 0.0;
        let expected_max_y = 500.0;
        assert!((lo[1] - expected_min_y).abs() < 0.01);
        assert!((hi[1] - expected_max_y).abs() < 0.01, "mesh {:?}; declared plan {:?}", hi, seg.plan);
        for point in &seg.mesh.positions {
            assert!(
                seg.plan.iter().any(|corner| (point[0] - corner[0]).hypot(point[1] - corner[1]) < 1e-6),
                "no openings: every vertical/cap corner must lie on the declared quad, got {point:?}"
            );
            assert!(point[2].abs() < 1e-6 || (point[2] - 3000.0).abs() < 1e-6);
        }
        let expected_volume = 250000.0 * 3000.0;
        assert!((volume(&seg.mesh) - expected_volume).abs() < 1.0);
    }

    // Case 3 isolates wall-end contact independently of the bottom-cap assertion.
    // It checks only vertical triangles lying in the wall-end plane, so changing
    // bottom-cap selection alone cannot make this test pass. There must be no
    // wall-end OR reveal membrane below the opening head at an exterior boundary.

    #[test]
    fn door_clamped_to_either_wall_end_does_not_leave_an_end_or_reveal_membrane() {
        let fr = frame();
        let s = span(1, 600.0, 0.0, 3000.0);
        for (u0, u1, end_x) in [(0.0, 1000.0, 0.0), (9000.0, 10000.0, 10000.0)] {
            let door = Rect { u0, u1, v0: 0.0, v1: 2100.0 };
            let seg = segment(&fr, "synthetic.L1", &s, &[door]).unwrap();
            assert_closed(&seg.mesh);
            let end_faces = seg
                .mesh
                .triangles
                .iter()
                .map(|tri| tri.map(|i| seg.mesh.positions[i as usize]))
                .filter(|points| points.iter().all(|point| (point[0] - end_x).abs() < 1e-6))
                .collect::<Vec<_>>();
            assert!(!end_faces.is_empty(), "retain the solid end face above the door head");
            assert!(
                end_faces.iter().all(|points| points.iter().all(|point| point[2] >= door.v1 - 1e-6)),
                "opening reaches end {end_x}; no wall material exists below its head there: {end_faces:?}"
            );
        }
    }

    #[test]
    fn skew_faces_and_their_openings_remain_closed_after_rigid_transforms() {
        for (angle, offset, z0) in [(0_f64, [0.0, 0.0], 0.0), (37.0, [123456.0, -98765.0], 5000.0), (-118.0, [-23000.0, 41000.0], -2480.0)] {
            let (sin, cos) = angle.to_radians().sin_cos();
            let at = |[x, y]: [f64; 2]| [offset[0] + x * cos - y * sin, offset[1] + x * sin + y * cos];
            let fr = FacadeFrame::new(at([0.0, 0.0]), at([1000.0, 0.0])).unwrap();
            let s =
                Span { level: 0, thickness: 400.0, z0, z1: z0 + 3000.0, in0: -300.0, in1: 600.0, qs: at([-300.0, 400.0]), qe: at([600.0, 200.0]) };
            for bottom in [0.0, 700.0] {
                let r = Rect { u0: 100.0, u1: 400.0, v0: z0 + bottom, v1: z0 + 2100.0 };
                let expected = 250000.0 * 3000.0 - 300.0 * (2500.0 / 9.0) * (2100.0 - bottom);
                let p = rect(r.u0, r.u1, r.v0, r.v1);
                for mesh in [
                    segment(&fr, "test", &s, &[r]).unwrap().mesh,
                    segment_profiles(&fr, "test", &s, std::slice::from_ref(&p)).unwrap().mesh,
                    skin(&fr, &[s], &[r]).unwrap(),
                    skin_profiles(&fr, &[s], std::slice::from_ref(&p)).unwrap(),
                ] {
                    assert_closed(&mesh);
                    assert!((volume(&mesh) - expected).abs() < 3.0, "angle {angle}, z {z0}, bottom {bottom}: {} vs {expected}", volume(&mesh));
                }
            }
        }
    }

    #[test]
    fn shaped_door_at_either_end_cuts_the_end_and_reveals() {
        let fr = frame();
        let s = span(0, 600.0, 5000.0, 8000.0);
        for (u0, u1, end) in [(0.0, 1000.0, 0.0), (9000.0, 10000.0, 10000.0)] {
            let p = vec![[u0, 5000.0], [u1, 5000.0], [u1, 7000.0], [(u0 + u1) / 2.0, 7200.0], [u0, 7000.0]];
            for mesh in [
                segment_profiles(&fr, "test", &s, std::slice::from_ref(&p)).unwrap().mesh,
                skin_profiles(&fr, &[s], std::slice::from_ref(&p)).unwrap(),
            ] {
                assert_closed(&mesh);
                let expected = 10000.0 * 600.0 * 3000.0 - (1000.0 * 2000.0 + 1000.0 * 200.0 / 2.0) * 600.0;
                assert!((volume(&mesh) - expected).abs() < 1.0);
                for tri in &mesh.triangles {
                    let points = tri.map(|i| mesh.positions[i as usize]);
                    if points.iter().all(|p| (p[0] - end).abs() < 1e-6) {
                        assert!(points.iter().all(|p| p[2] >= 7000.0 - 1e-6), "end membrane: {points:?}");
                    }
                }
            }
        }
    }

    fn horizontal_areas(mesh: &Mesh, z: f64) -> [f64; 2] {
        let mut areas = [0.0, 0.0];
        for tri in &mesh.triangles {
            let [a, b, c] = tri.map(|i| mesh.positions[i as usize]);
            if [a, b, c].iter().all(|p| (p[2] - z).abs() < 1e-6) {
                let area = ((b[0] - a[0]) * (c[1] - a[1]) - (b[1] - a[1]) * (c[0] - a[0])) / 2.0;
                areas[usize::from(area < 0.0)] += area.abs();
            }
        }
        areas
    }

    #[test]
    fn crossing_inner_faces_close_both_sides_of_a_storey_joint() {
        let fr = FacadeFrame::new([0.0, 0.0], [1000.0, 0.0]).unwrap();
        let lo = Span { level: 0, thickness: 400.0, in0: 0.0, in1: 1000.0, qs: [0.0, 400.0], qe: [1000.0, 200.0], z0: 0.0, z1: 3000.0 };
        let hi = Span { level: 1, qs: [0.0, 300.0], qe: [1000.0, 300.0], z0: 3000.0, z1: 6000.0, ..lo };
        for mesh in [skin(&fr, &[lo, hi], &[]).unwrap(), skin_profiles(&fr, &[lo, hi], &[]).unwrap()] {
            let areas = horizontal_areas(&mesh, 3000.0);
            assert!((areas[0] - 25000.0).abs() < 0.01 && (areas[1] - 25000.0).abs() < 0.01, "{areas:?}");
            assert!((volume(&mesh) - 1800000000.0).abs() < 1.0);
        }
    }

    #[test]
    fn openings_starting_or_ending_at_a_storey_joint_close_from_the_material_side() {
        let fr = frame();
        let spans = [span(0, 600.0, 0.0, 3000.0), span(1, 360.0, 3000.0, 6000.0)];
        for (v0, v1, expected_areas) in [(3000.0, 5100.0, [2832000.0, 0.0]), (900.0, 3000.0, [2112000.0, 432000.0])] {
            let r = Rect { u0: 2000.0, u1: 3200.0, v0, v1 };
            let p = rect(r.u0, r.u1, r.v0, r.v1);
            let pieces_volume: f64 = spans.iter().map(|s| volume(&segment(&fr, "test", s, &[r]).unwrap().mesh)).sum();
            for mesh in [skin(&fr, &spans, &[r]).unwrap(), skin_profiles(&fr, &spans, std::slice::from_ref(&p)).unwrap()] {
                let areas = horizontal_areas(&mesh, 3000.0);
                assert!(areas.iter().zip(expected_areas).all(|(got, expected)| (*got - expected).abs() < 0.01), "{v0}..{v1}: {areas:?}");
                assert!((volume(&mesh) - pieces_volume).abs() < 1.0);
            }
        }
    }

    #[test]
    fn submillimetre_clearance_or_overlap_at_a_floor_preserves_wall_closure() {
        let fr = frame();
        let s = span(0, 600.0, 0.0, 3000.0);
        for (v0, v1) in [(0.1, 2100.0), (-2100.0, 0.1), (2999.9, 5100.0), (900.0, 2999.9)] {
            let r = Rect { u0: 2000.0, u1: 3200.0, v0, v1 };
            let mesh = segment(&fr, "test", &s, &[r]).unwrap().mesh;
            assert_closed(&mesh);
            let height = v1.min(s.z1) - v0.max(s.z0);
            let expected = 10000.0 * 600.0 * 3000.0 - 1200.0 * 600.0 * height;
            assert!((volume(&mesh) - expected).abs() < 5.0);
        }
    }

    #[test]
    fn touching_openings_have_reveals_only_against_remaining_wall_material() {
        let fr = frame();
        let s = span(0, 600.0, 0.0, 3000.0);
        for right_height in [2100.0, 2400.0] {
            let holes = [Rect { u0: 1000.0, u1: 2000.0, v0: 0.0, v1: 2100.0 }, Rect { u0: 2000.0, u1: 3000.0, v0: 0.0, v1: right_height }];
            let profiles: Vec<_> = holes.iter().map(|r| rect(r.u0, r.u1, r.v0, r.v1)).collect();
            for mesh in [segment(&fr, "test", &s, &holes).unwrap().mesh, segment_profiles(&fr, "test", &s, &profiles).unwrap().mesh] {
                assert_closed(&mesh);
                let expected = 10000.0 * 600.0 * 3000.0 - 1000.0 * 600.0 * (2100.0 + right_height);
                assert!((volume(&mesh) - expected).abs() < 1.0);
                let mut area = 0.0;
                for tri in &mesh.triangles {
                    let points = tri.map(|i| mesh.positions[i as usize]);
                    if points.iter().all(|p| (p[0] - 2000.0).abs() < 1e-6) {
                        assert!(points.iter().all(|p| p[2] >= 2100.0 - 1e-6));
                        let [a, b, c] = points;
                        area += ((b[1] - a[1]) * (c[2] - a[2]) - (b[2] - a[2]) * (c[1] - a[1])).abs() / 2.0;
                    }
                }
                assert!((area - 600.0 * (right_height - 2100.0)).abs() < 0.01);
            }
        }
    }
}
