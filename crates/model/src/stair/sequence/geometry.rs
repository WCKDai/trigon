//! Bounded polygon predicates in millimetres. Source rings are never snapped, clipped,
//! convexified or reordered. All predicates use local differences to reduce cancellation.
use super::{Gate, Point, invalid};
use crate::{Mesh, Result};

pub(super) const EPS: f64 = 1e-6;
pub(super) const MAX_WORK: usize = 2_000_000;
pub(super) struct Work {
    remaining: usize,
}
impl Work {
    pub fn new() -> Self {
        Self { remaining: MAX_WORK }
    }
    pub fn take(&mut self, n: usize) -> Result<()> {
        self.remaining = self.remaining.checked_sub(n).ok_or_else(|| invalid("geometry work limit exceeded; result unverified"))?;
        Ok(())
    }
}
pub(super) fn sub(a: Point, b: Point) -> Point {
    [a[0] - b[0], a[1] - b[1]]
}
pub(super) fn cross(a: Point, b: Point) -> f64 {
    a[0] * b[1] - a[1] * b[0]
}
pub(super) fn dot(a: Point, b: Point) -> f64 {
    a[0] * b[0] + a[1] * b[1]
}
pub(super) fn length(a: Point) -> f64 {
    a[0].hypot(a[1])
}
pub(super) fn near(a: Point, b: Point) -> bool {
    length(sub(a, b)) <= EPS
}
pub(super) fn at(a: Point, b: Point, t: f64) -> Point {
    [a[0] + (b[0] - a[0]) * t, a[1] + (b[1] - a[1]) * t]
}
pub(super) fn mid(g: Gate) -> Point {
    at(g[0], g[1], 0.5)
}
pub(super) fn edges(r: &[Point]) -> impl Iterator<Item = Gate> + '_ {
    r.iter().copied().zip(r.iter().copied().cycle().skip(1)).map(|(a, b)| [a, b])
}
pub(super) fn distance(p: Point, g: Gate) -> f64 {
    let d = sub(g[1], g[0]);
    let n = dot(d, d);
    if n == 0. {
        return length(sub(p, g[0]));
    }
    length(sub(p, at(g[0], g[1], (dot(sub(p, g[0]), d) / n).clamp(0., 1.))))
}
pub(super) fn on(p: Point, g: Gate) -> bool {
    distance(p, g) <= EPS
}
pub(super) fn area(r: &[Point]) -> f64 {
    let Some(&o) = r.first() else { return 0. };
    edges(r).map(|[a, b]| cross(sub(a, o), sub(b, o))).sum::<f64>() * 0.5
}
#[derive(Debug, Clone, Copy)]
pub(super) enum Contact {
    Point(Point),
    Segment(Gate),
}
pub(super) fn contact(a: Gate, b: Gate, work: &mut Work) -> Result<Option<Contact>> {
    work.take(1)?;
    let r = sub(a[1], a[0]);
    let s = sub(b[1], b[0]);
    let nr = length(r);
    let ns = length(s);
    if nr == 0. || ns == 0. {
        return Ok(None);
    }
    let u = [r[0] / nr, r[1] / nr];
    let v = [s[0] / ns, s[1] / ns];
    let q = sub(b[0], a[0]);
    let den = cross(u, v);
    // Geometric collinearity uses distance, never an area tolerance with wrong units.
    if cross(q, u).abs() <= EPS && cross(sub(b[1], a[0]), u).abs() <= EPS {
        let t0 = dot(q, u);
        let t1 = dot(sub(b[1], a[0]), u);
        let lo = t0.min(t1).max(0.);
        let hi = t0.max(t1).min(nr);
        if hi < lo - EPS {
            return Ok(None);
        }
        if hi - lo <= EPS {
            return Ok(Some(Contact::Point(at(a[0], a[1], ((lo + hi) * 0.5 / nr).clamp(0., 1.)))));
        }
        return Ok(Some(Contact::Segment([at(a[0], a[1], lo / nr), at(a[0], a[1], hi / nr)])));
    }
    if den.abs() > f64::EPSILON {
        let t = cross(q, v) / den;
        let w = cross(q, u) / den;
        if t >= -EPS && t <= nr + EPS && w >= -EPS && w <= ns + EPS {
            return Ok(Some(Contact::Point(at(a[0], a[1], (t / nr).clamp(0., 1.)))));
        }
    }
    // Endpoint proximity matters even when near-parallel line intersection is distant.
    for p in a {
        if on(p, b) {
            return Ok(Some(Contact::Point(p)));
        }
    }
    for p in b {
        if on(p, a) {
            return Ok(Some(Contact::Point(p)));
        }
    }
    Ok(None)
}
/// -1 outside, 0 boundary, 1 interior.
fn location(r: &[Point], p: Point, work: &mut Work) -> Result<i8> {
    let mut inside = false;
    for [a, b] in edges(r) {
        work.take(1)?;
        if on(p, [a, b]) {
            return Ok(0);
        }
        if (a[1] > p[1]) != (b[1] > p[1]) && p[0] < a[0] + (b[0] - a[0]) * (p[1] - a[1]) / (b[1] - a[1]) {
            inside = !inside;
        }
    }
    Ok(if inside { 1 } else { -1 })
}
fn intervals(g: Gate, r: &[Point], work: &mut Work) -> Result<Vec<f64>> {
    let mut ts = vec![0., 1.];
    let d = sub(g[1], g[0]);
    let n = dot(d, d);
    if n == 0. {
        return Ok(ts);
    }
    for e in edges(r) {
        if let Some(c) = contact(g, e, work)? {
            let points = match c {
                Contact::Point(p) => [p, p],
                Contact::Segment(pair) => pair,
            };
            for p in points {
                ts.push((dot(sub(p, g[0]), d) / n).clamp(0., 1.));
            }
        }
    }
    ts.sort_by(f64::total_cmp);
    ts.dedup_by(|a, b| *a == *b);
    Ok(ts)
}
pub(super) fn covers(r: &[Point], g: Gate, work: &mut Work) -> Result<bool> {
    if location(r, g[0], work)? < 0 || location(r, g[1], work)? < 0 {
        return Ok(false);
    }
    for ts in intervals(g, r, work)?.windows(2) {
        if location(r, at(g[0], g[1], (ts[0] + ts[1]) * 0.5), work)? < 0 {
            return Ok(false);
        }
    }
    Ok(true)
}
pub(super) fn ring(r: &[Point], work: &mut Work) -> Result<()> {
    for (i, e) in edges(r).enumerate() {
        if length(sub(e[1], e[0])) < 0.001 {
            return Err(invalid("ring edge shorter than 0.001 mm"));
        }
        for (j, f) in edges(r).enumerate().skip(i + 1) {
            if let Some(c) = contact(e, f, work)? {
                let adjacent = j == i + 1 || (i == 0 && j + 1 == r.len());
                if !adjacent || matches!(c, Contact::Segment(_)) {
                    return Err(invalid("ring must be simple without crossing, repeated vertices or backtracking"));
                }
            }
        }
    }
    if area(r) < 1. {
        return Err(invalid("ring must be CCW with area at least 1 mm2"));
    }
    Ok(())
}
pub(super) fn boundary_gate(r: &[Point], g: Gate, forward: bool, work: &mut Work) -> Result<()> {
    // Width is derived from authored points. Use the same distance tolerance as
    // boundary contact so a rigid transform cannot reject an exact limit width.
    // This validates only; no endpoint, mesh coordinate or shared scalar bound changes.
    crate::checked("gate width", length(sub(g[1], g[0])), 600. - EPS, 4000. + EPS)?;
    for e in edges(r) {
        work.take(1)?;
        if on(g[0], e) && on(g[1], e) {
            if (dot(sub(g[1], g[0]), sub(e[1], e[0])) > 0.) != forward {
                return Err(invalid("gate orientation must be left/right in ascent direction"));
            }
            return Ok(());
        }
    }
    Err(invalid("gate must be a segment of one boundary edge"))
}
fn bounds(r: &[Point]) -> Gate {
    let mut lo = [f64::INFINITY; 2];
    let mut hi = [f64::NEG_INFINITY; 2];
    for p in r {
        for axis in 0..2 {
            lo[axis] = lo[axis].min(p[axis]);
            hi[axis] = hi[axis].max(p[axis]);
        }
    }
    [lo, hi]
}
/// Returns contacts; refuses positive-area intersection. Concave rings are handled by
/// splitting EVERY edge at intersections, then testing each open interval.
pub(super) fn disjoint(a: &[Point], b: &[Point], work: &mut Work) -> Result<Vec<Contact>> {
    work.take(1)?;
    let [alo, ahi] = bounds(a);
    let [blo, bhi] = bounds(b);
    if (0..2).any(|i| ahi[i] < blo[i] - EPS || bhi[i] < alo[i] - EPS) {
        return Ok(Vec::new());
    }
    let mut contacts = Vec::new();
    for e in edges(a) {
        for f in edges(b) {
            if let Some(c) = contact(e, f, work)? {
                if matches!(c, Contact::Segment(_)) && dot(sub(e[1], e[0]), sub(f[1], f[0])) > 0. {
                    return Err(invalid("polygon interiors overlap"));
                }
                contacts.push(c);
            }
        }
    }
    for (r, other) in [(a, b), (b, a)] {
        for e in edges(r) {
            if location(other, e[0], work)? == 1 {
                return Err(invalid("polygon interiors overlap"));
            }
            for ts in intervals(e, other, work)?.windows(2) {
                if location(other, at(e[0], e[1], (ts[0] + ts[1]) * 0.5), work)? == 1 {
                    return Err(invalid("polygon interiors overlap"));
                }
            }
        }
    }
    Ok(contacts)
}
pub(super) fn only_gate(contacts: &[Contact], g: Gate, work: &mut Work) -> Result<()> {
    let direction = sub(g[1], g[0]);
    let width = length(direction);
    let mut intervals = Vec::new();
    for c in contacts {
        work.take(3)?;
        let pts = match *c {
            Contact::Point(p) => [p, p],
            Contact::Segment(e) => e,
        };
        if pts.iter().any(|&p| !on(p, g)) {
            return Err(invalid("route cells touch outside their declared gate"));
        }
        if let Contact::Segment(_) = c {
            let [a, b] = pts.map(|p| dot(sub(p, g[0]), direction) / width);
            intervals.push([a.min(b), a.max(b)]);
        }
    }
    intervals.sort_by(|a, b| a[0].total_cmp(&b[0]));
    let mut end = 0.0_f64;
    for [a, b] in intervals {
        if a > end + EPS {
            return Err(invalid("actual boundaries must share the full declared gate"));
        }
        end = end.max(b);
    }
    if end < width - EPS {
        return Err(invalid("actual boundaries must share the full declared gate"));
    }
    Ok(())
}

/// Ear clipping preserves all original corners, including collinear boundary vertices.
/// A vertex on an ear's diagonal blocks that ear, so the mesh has no boundary T-junction.
pub(super) fn prism(r: &[Point], bottom: f64, top: f64, work: &mut Work) -> Result<Mesh> {
    let mut remaining: Vec<usize> = (0..r.len()).collect();
    let mut cap = Vec::new();
    while remaining.len() > 3 {
        let n = remaining.len();
        let mut ear = None;
        for i in 0..n {
            work.take(1)?;
            let ia = remaining[(i + n - 1) % n];
            let ib = remaining[i];
            let ic = remaining[(i + 1) % n];
            let (Some(&a), Some(&b), Some(&c)) = (r.get(ia), r.get(ib), r.get(ic)) else {
                return Err(invalid("triangulation index"));
            };
            if cross(sub(b, a), sub(c, a)) <= 0. {
                continue;
            }
            let mut blocked = false;
            for &j in &remaining {
                if j == ia || j == ib || j == ic {
                    continue;
                }
                work.take(1)?;
                let Some(&p) = r.get(j) else {
                    return Err(invalid("triangulation vertex"));
                };
                // Strict enough to preserve collinear vertices; EPS is a distance.
                if cross(sub(b, a), sub(p, a)) >= -EPS * length(sub(b, a))
                    && cross(sub(c, b), sub(p, b)) >= -EPS * length(sub(c, b))
                    && cross(sub(a, c), sub(p, c)) >= -EPS * length(sub(a, c))
                {
                    blocked = true;
                    break;
                }
            }
            if !blocked {
                ear = Some((i, [ia, ib, ic]));
                break;
            }
        }
        let Some((i, t)) = ear else {
            return Err(invalid("ring cannot be triangulated without degenerate faces"));
        };
        cap.push(t);
        remaining.remove(i);
    }
    let [a, b, c] = remaining.as_slice() else {
        return Err(invalid("triangulation remainder"));
    };
    cap.push([*a, *b, *c]);
    let n = r.len() as u32;
    let mut mesh = Mesh::default();
    mesh.positions.extend(r.iter().map(|p| [p[0], p[1], bottom]));
    mesh.positions.extend(r.iter().map(|p| [p[0], p[1], top]));
    for t in cap {
        let [a, b, c] = t.map(|i| i as u32);
        mesh.triangles.extend([[c, b, a], [a + n, b + n, c + n]]);
    }
    for i in 0..n {
        let j = (i + 1) % n;
        mesh.triangles.extend([[i, j, j + n], [i, j + n, i + n]]);
    }
    Ok(mesh)
}
