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

const EPS: f64 = 0.5;

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
    vec![[0.0, 0.0], [fr.length, 0.0], [s.in1, s.thickness], [s.in0, s.thickness]]
}

/// An opening's footprint through the wall, in (u, w).
fn through(r: &Rect, depth: f64) -> Vec<[f64; 2]> {
    rect(r.u0, r.u1, -1.0, depth + 1.0)
}

/// Openings that cross the height `z`.
fn crossing(holes: &[Rect], z: f64, depth: f64) -> Vec<Vec<[f64; 2]>> {
    holes.iter().filter(|r| r.v0 < z - EPS && r.v1 > z + EPS).map(|r| through(r, depth)).collect()
}

/// The inner face of a span with its holes, and the reveals of every opening within it.
fn inner_and_reveals(m: &mut Mesh, fr: &FacadeFrame, s: &Span, holes: &[Rect]) -> bool {
    let (n, d) = ([fr.n[0], fr.n[1], 0.0], s.thickness);
    let t = [fr.t[0], fr.t[1], 0.0];
    let mine: Vec<Rect> = holes
        .iter()
        .filter(|r| r.v0 < s.z1 - EPS && r.v1 > s.z0 + EPS)
        .map(|r| Rect { u0: r.u0, u1: r.u1, v0: r.v0.max(s.z0), v1: r.v1.min(s.z1) })
        .collect();
    let cuts: Vec<_> = mine.iter().map(|r| rect(r.u0, r.u1, r.v0, r.v1)).collect();
    let ok = fill(m, &cut(&rect(s.in0, s.in1, s.z0, s.z1), &cuts), |p| fr.point(p[0], d, p[1]), [-n[0], -n[1], 0.0]);
    for (r, whole) in mine.iter().zip(holes.iter().filter(|r| r.v0 < s.z1 - EPS && r.v1 > s.z0 + EPS)) {
        let Rect { u0, u1, v0, v1 } = *r;
        quad(m, [fr.point(u0, 0.0, v0), fr.point(u0, d, v0), fr.point(u0, d, v1), fr.point(u0, 0.0, v1)], t);
        quad(m, [fr.point(u1, 0.0, v0), fr.point(u1, d, v0), fr.point(u1, d, v1), fr.point(u1, 0.0, v1)], [-t[0], -t[1], 0.0]);
        // Sill and head only where the opening really ends, not where it crosses a floor.
        if (whole.v0 - v0).abs() < EPS && v0 > s.z0 + EPS {
            quad(m, [fr.point(u0, 0.0, v0), fr.point(u1, 0.0, v0), fr.point(u1, d, v0), fr.point(u0, d, v0)], [0.0, 0.0, 1.0]);
        }
        if (whole.v1 - v1).abs() < EPS && v1 < s.z1 - EPS {
            quad(m, [fr.point(u0, 0.0, v1), fr.point(u1, 0.0, v1), fr.point(u1, d, v1), fr.point(u0, d, v1)], [0.0, 0.0, -1.0]);
        }
    }
    ok
}

/// The horizontal face of a span's band at `z`, minus the openings crossing it.
fn cap(m: &mut Mesh, fr: &FacadeFrame, s: &Span, z: f64, up: bool, holes: &[Rect]) -> bool {
    let shapes = cut(&band(fr, s), &crossing(holes, z, s.thickness));
    fill(m, &shapes, |p| fr.point(p[0], p[1], z), [0.0, 0.0, if up { 1.0 } else { -1.0 }])
}

/// The mitred ends of a span, facing away from it.
fn ends(m: &mut Mesh, fr: &FacadeFrame, s: &Span) {
    let (a, e) = (fr.point(0.0, 0.0, 0.0), fr.point(fr.length, 0.0, 0.0));
    let mid = [(a[0] + e[0] + s.qs[0] + s.qe[0]) / 4.0, (a[1] + e[1] + s.qs[1] + s.qe[1]) / 4.0];
    for (o, q) in [([a[0], a[1]], s.qs), ([e[0], e[1]], s.qe)] {
        let c = [(o[0] + q[0]) / 2.0 - mid[0], (o[1] + q[1]) / 2.0 - mid[1], 0.0];
        quad(m, [[o[0], o[1], s.z0], [q[0], q[1], s.z0], [q[0], q[1], s.z1], [o[0], o[1], s.z1]], c);
    }
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
    ends(&mut m, fr, s);
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
        ends(&mut m, fr, s);
    }
    ok &= cap(&mut m, fr, last, last.z1, true, holes);
    ok &= cap(&mut m, fr, first, first.z0, false, holes);
    // Steps: where the storey below is thicker the step faces up, otherwise down.
    for pair in spans.windows(2) {
        let (lo, hi) = (&pair[0], &pair[1]);
        let z = lo.z1;
        let (wide, narrow, up) = if lo.thickness >= hi.thickness { (lo, hi, true) } else { (hi, lo, false) };
        if (wide.thickness - narrow.thickness).abs() < EPS && (wide.in0 - narrow.in0).abs() < EPS && (wide.in1 - narrow.in1).abs() < EPS {
            continue;
        }
        let mut cuts = crossing(holes, z, wide.thickness);
        cuts.push(band(fr, narrow));
        ok &= fill(&mut m, &cut(&band(fr, wide), &cuts), |p| fr.point(p[0], p[1], z), [0.0, 0.0, if up { 1.0 } else { -1.0 }]);
    }
    ok.then_some(m)
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
}
