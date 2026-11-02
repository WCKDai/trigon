//! Exact, bounded clipping of roof height fields. Only explicit new opening paths use this.
use super::dormer::{Plane, local, plane, polygon};
use super::*;
const EPS: f64 = 1e-6;

fn cross(a: [f64; 2], b: [f64; 2]) -> f64 {
    a[0] * b[1] - a[1] * b[0]
}
fn sub(a: [f64; 2], b: [f64; 2]) -> [f64; 2] {
    [a[0] - b[0], a[1] - b[1]]
}
pub(super) fn area(p: &[[f64; 2]]) -> f64 {
    p.iter().zip(p.iter().cycle().skip(1)).take(p.len()).map(|(a, b)| cross(*a, *b)).sum::<f64>().abs() / 2.
}
pub(super) fn rect(u0: f64, u1: f64, w0: f64, w1: f64) -> Vec<[f64; 2]> {
    vec![[u0, w0], [u1, w0], [u1, w1], [u0, w1]]
}
fn triangles(outline: &[[f64; 2]]) -> Result<Vec<[[f64; 2]; 3]>> {
    let flat: Vec<_> = outline.iter().flatten().copied().collect();
    let indices = earcutr::earcut(&flat, &[], 2).map_err(|_| ModelError::Invalid("roof opening outline cannot be triangulated".into()))?;
    Ok(indices
        .as_chunks::<3>()
        .0
        .iter()
        .map(|t| {
            let mut p = t.map(|i| outline[i]);
            if cross(sub(p[1], p[0]), sub(p[2], p[0])) < 0. {
                p.swap(1, 2);
            }
            p
        })
        .collect())
}
fn clip(poly: &[[f64; 3]], a: [f64; 2], b: [f64; 2], inside: bool) -> Vec<[f64; 3]> {
    let d = sub(b, a);
    let dist = |p: [f64; 3]| cross(d, sub([p[0], p[1]], a));
    let mut out = Vec::new();
    for (&p, &q) in poly.iter().zip(poly.iter().cycle().skip(1)).take(poly.len()) {
        let (dp, dq) = (dist(p), dist(q));
        let keep = |v: f64| if inside { v >= 0. } else { v <= 0. };
        if keep(dp) {
            out.push(p);
        }
        if keep(dp) != keep(dq) {
            let t = dp / (dp - dq);
            out.push(std::array::from_fn(|k| p[k] + t * (q[k] - p[k])));
        }
    }
    out.dedup_by(|a, b| a.iter().zip(b.iter()).all(|(a, b)| (a - b).abs() < EPS));
    if out.first().zip(out.last()).is_some_and(|(a, b)| a.iter().zip(b.iter()).all(|(a, b)| (a - b).abs() < EPS)) {
        out.pop();
    }
    out
}
fn patches(host: &Mesh, fr: &FacadeFrame, outline: &[[f64; 2]], top: bool) -> Result<Vec<Vec<[f64; 3]>>> {
    let mut out = Vec::new();
    let cuts = triangles(outline)?;
    for t in &host.triangles {
        let p = t.map(|i| local(fr, host.positions.get(i as usize).copied().unwrap_or_default()));
        let det = cross(sub([p[1][0], p[1][1]], [p[0][0], p[0][1]]), sub([p[2][0], p[2][1]], [p[0][0], p[0][1]]));
        if (top && det <= EPS) || (!top && det >= -EPS) {
            continue;
        }
        for cut in &cuts {
            let mut p = p.to_vec();
            for k in 0..3 {
                p = clip(&p, cut[k], cut[(k + 1) % 3], true);
            }
            if p.len() >= 3 {
                out.push(p);
            }
        }
    }
    let found: f64 = out.iter().map(|p| area(&p.iter().map(|q| [q[0], q[1]]).collect::<Vec<_>>())).sum();
    let required = area(outline);
    if (found - required).abs() > EPS * required.sqrt().max(1.) {
        return Err(ModelError::Invalid(format!(
            "roof opening has incomplete {} host support: covered {found:.6} mm², required {required:.6} mm², difference {:.6} mm²",
            if top { "top" } else { "underside" },
            found - required
        )));
    }
    Ok(out)
}
fn station(p: [f64; 2], a: [f64; 2], b: [f64; 2]) -> Option<f64> {
    let d = sub(b, a);
    let len = d[0].hypot(d[1]);
    if len < EPS {
        return None;
    }
    let q = sub(p, a);
    if cross(d, q).abs() > EPS * len {
        return None;
    }
    Some((q[0] * d[0] + q[1] * d[1]) / len)
}
fn conform(mesh: &Mesh, fr: &FacadeFrame, lines: &[([f64; 2], [f64; 2])]) -> Mesh {
    let lines: Vec<_> = lines
        .iter()
        .filter_map(|&(a, b)| {
            let len = (b[0] - a[0]).hypot(b[1] - a[1]);
            if len < EPS {
                return None;
            }
            let mut s: Vec<_> = mesh
                .positions
                .iter()
                .filter_map(|&p| {
                    let p = local(fr, p);
                    station([p[0], p[1]], a, b)
                })
                .collect();
            s.sort_by(f64::total_cmp);
            s.dedup_by(|a, b| (*a - *b).abs() < EPS);
            Some((a, b, s))
        })
        .collect();
    let mut out = Mesh::default();
    for t in &mesh.triangles {
        let p = t.map(|i| mesh.positions.get(i as usize).copied().unwrap_or_default());
        let mut boundary = Vec::new();
        for k in 0..3 {
            let (a, b) = (p[k], p[(k + 1) % 3]);
            let (aa, bb) = (local(fr, a), local(fr, b));
            let length = a.iter().zip(b).map(|(a, b)| (a - b).powi(2)).sum::<f64>().sqrt();
            let mut steps = vec![0.];
            for (l, r, ss) in &lines {
                if let (Some(sa), Some(sb)) = (station([aa[0], aa[1]], *l, *r), station([bb[0], bb[1]], *l, *r)) {
                    if (sb - sa).abs() < EPS {
                        continue;
                    }
                    for s in ss {
                        let t = (s - sa) / (sb - sa);
                        if t > 0. && t < 1. && t * length > EPS && (1. - t) * length > EPS {
                            steps.push(t);
                        }
                    }
                }
            }
            steps.sort_by(f64::total_cmp);
            steps.dedup_by(|a, b| (*a - *b).abs() * length < EPS);
            boundary.extend(steps.into_iter().map(|t| std::array::from_fn(|i| a[i] + t * (b[i] - a[i]))));
        }
        if boundary.len() == 3 {
            polygon(&mut out, &boundary);
        } else {
            let c = std::array::from_fn(|i| (p[0][i] + p[1][i] + p[2][i]) / 3.);
            for k in 0..boundary.len() {
                polygon(&mut out, &[c, boundary[k], boundary[(k + 1) % boundary.len()]]);
            }
        }
    }
    out
}
/// Reconcile only unmatched collinear edge subdivisions. This inserts existing
/// boundary vertices into incident triangles; it cannot fill a hole or reverse a face.
/// Clipping an already clipped shell can split a shared source edge outside the
/// current aperture, so checking just the new aperture's lines is insufficient.
pub(super) fn conform_shared_edges(mesh: &Mesh) -> Mesh {
    use std::collections::BTreeMap;
    let key = |p: [f64; 3]| p.map(|v| (v * 1e6).round() as i64);
    let edge = |a: [i64; 3], b: [i64; 3]| if a < b { ((a, b), 1_i64) } else { ((b, a), -1_i64) };
    let mut counts = BTreeMap::<_, (usize, i64)>::new();
    let mut points = BTreeMap::new();
    for t in &mesh.triangles {
        let p = t.map(|i| mesh.positions.get(i as usize).copied().unwrap_or_default());
        for k in 0..3 {
            let a = key(p[k]);
            let b = key(p[(k + 1) % 3]);
            points.insert(a, p[k]);
            let (e, sign) = edge(a, b);
            let c = counts.entry(e).or_default();
            c.0 += 1;
            c.1 += sign;
        }
    }
    let bad: std::collections::BTreeSet<_> = counts.into_iter().filter(|(_, c)| *c != (2, 0)).map(|(e, _)| e).collect();
    if bad.is_empty() {
        return mesh.clone();
    }
    let mut seen = std::collections::BTreeSet::new();
    let candidates: Vec<_> = bad.iter().flat_map(|(a, b)| [a, b]).filter(|k| seen.insert(**k)).filter_map(|k| points.get(k).copied()).collect();
    let mut out = Mesh::default();
    for t in &mesh.triangles {
        let p = t.map(|i| mesh.positions.get(i as usize).copied().unwrap_or_default());
        let mut boundary = Vec::new();
        for k in 0..3 {
            let (a, b) = (p[k], p[(k + 1) % 3]);
            let mut stations = vec![(0., a)];
            if bad.contains(&edge(key(a), key(b)).0) {
                let d = std::array::from_fn::<_, 3, _>(|k| b[k] - a[k]);
                let length2 = d.iter().map(|v| v * v).sum::<f64>();
                let length = length2.sqrt();
                if length > EPS {
                    for &q in &candidates {
                        let step = q.iter().zip(a).zip(d).map(|((q, a), d)| (q - a) * d).sum::<f64>() / length2;
                        if step <= 0. || step >= 1. || step * length <= EPS || (1. - step) * length <= EPS {
                            continue;
                        }
                        let residual = q.iter().zip(a).zip(d).map(|((q, a), d)| (q - a - step * d).powi(2)).sum::<f64>();
                        if residual <= EPS * EPS {
                            stations.push((step, q));
                        }
                    }
                }
            }
            let length = a.iter().zip(b).map(|(a, b)| (a - b).powi(2)).sum::<f64>().sqrt();
            stations.sort_by(|a, b| a.0.total_cmp(&b.0));
            stations.dedup_by(|a, b| (a.0 - b.0).abs() * length < EPS);
            boundary.extend(stations.into_iter().map(|(_, p)| p));
        }
        if boundary.len() == 3 {
            polygon(&mut out, &boundary);
        } else {
            let c = std::array::from_fn(|k| (p[0][k] + p[1][k] + p[2][k]) / 3.);
            for k in 0..boundary.len() {
                polygon(&mut out, &[c, boundary[k], boundary[(k + 1) % boundary.len()]]);
            }
        }
    }
    out
}
fn edges(outline: &[[f64; 2]]) -> Vec<([f64; 2], [f64; 2])> {
    outline.iter().copied().zip(outline.iter().copied().cycle().skip(1)).take(outline.len()).collect()
}
// A separating axis proves these projected convex polygons are disjoint. Avoid
// splitting remote triangles along the aperture's infinite supporting lines:
// repeated openings would otherwise multiply irrelevant fragments and slivers.
fn disjoint(a: &[[f64; 3]], b: &[[f64; 2]]) -> bool {
    let aa: Vec<_> = a.iter().map(|p| [p[0], p[1]]).collect();
    for ring in [&aa[..], b] {
        for (&p, &q) in ring.iter().zip(ring.iter().cycle().skip(1)).take(ring.len()) {
            let d = sub(q, p);
            if d == [0., 0.] {
                continue;
            }
            let range = |ring: &[[f64; 2]]| {
                ring.iter().map(|&q| cross(d, sub(q, p))).fold((f64::INFINITY, f64::NEG_INFINITY), |(lo, hi), v| (lo.min(v), hi.max(v)))
            };
            let (x, y) = (range(&aa), range(b));
            if x.1 <= y.0 || y.1 <= x.0 {
                return true;
            }
        }
    }
    false
}
fn carve(mesh: &Mesh, fr: &FacadeFrame, outline: &[[f64; 2]]) -> Result<Mesh> {
    let mut current = mesh.clone();
    for cut in triangles(outline)? {
        let mut out = Mesh::default();
        for t in &current.triangles {
            let mut p = t.iter().map(|&i| local(fr, current.positions.get(i as usize).copied().unwrap_or_default())).collect::<Vec<_>>();
            if disjoint(&p, &cut) {
                polygon(&mut out, &t.map(|i| current.positions.get(i as usize).copied().unwrap_or_default()));
                continue;
            }
            for k in 0..3 {
                if p.is_empty() {
                    break;
                }
                let q = clip(&p, cut[k], cut[(k + 1) % 3], false);
                polygon(&mut out, &q.iter().map(|p| fr.point(p[0], p[1], p[2])).collect::<Vec<_>>());
                p = clip(&p, cut[k], cut[(k + 1) % 3], true);
            }
        }
        current = out;
    }
    Ok(current)
}
fn boundary_stations(mesh: &Mesh, fr: &FacadeFrame, a: [f64; 2], b: [f64; 2]) -> Vec<[f64; 2]> {
    let len = (b[0] - a[0]).hypot(b[1] - a[1]);
    let mut s = vec![0., len];
    s.extend(
        mesh.positions
            .iter()
            .filter_map(|&p| {
                let p = local(fr, p);
                station([p[0], p[1]], a, b)
            })
            .filter(|s| *s > EPS && *s < len - EPS),
    );
    s.sort_by(f64::total_cmp);
    s.dedup_by(|a, b| (*a - *b).abs() < EPS);
    s.into_iter().map(|t| [a[0] + t / len * (b[0] - a[0]), a[1] + t / len * (b[1] - a[1])]).collect()
}
pub(super) fn cut(mesh: &Mesh, host: &Mesh, fr: &FacadeFrame, outline: &[[f64; 2]]) -> Result<Mesh> {
    patches(host, fr, outline, true)?;
    patches(host, fr, outline, false)?;
    let mut out = carve(mesh, fr, outline)?;
    // Include both skins' crease and triangulation stations before constructing the reveals.
    let mut stations = out.clone();
    stations.append(host);
    for (a, b) in edges(outline) {
        let pts = boundary_stations(&stations, fr, a, b);
        for q in pts.windows(2) {
            let mid = [(q[0][0] + q[1][0]) / 2., (q[0][1] + q[1][1]) / 2.];
            let (top, bottom) = (plane(host, fr, mid, true)?, plane(host, fr, mid, false)?);
            let p = |q: [f64; 2], p: Plane| fr.point(q[0], q[1], p.z(q[0], q[1]));
            polygon(&mut out, &[p(q[0], bottom), p(q[0], top), p(q[1], top), p(q[1], bottom)]);
        }
    }
    let mut lines = edges(outline);
    for t in triangles(outline)? {
        lines.extend(edges(&t));
    }
    Ok(conform_shared_edges(&conform(&out, fr, &lines)))
}
pub(super) fn volume(host: &Mesh, fr: &FacadeFrame, outline: &[[f64; 2]], high: Plane) -> Result<Mesh> {
    let polys = patches(host, fr, outline, false)?;
    let mut out = Mesh::default();
    for p in polys {
        for q in &p {
            if high.z(q[0], q[1]) - q[2] <= EPS {
                return Err(ModelError::Invalid(format!(
                    "roof opening seat is inverted at u={:.6} mm, w={:.6} mm: top-minus-underside={:.6} mm",
                    q[0],
                    q[1],
                    high.z(q[0], q[1]) - q[2]
                )));
            }
        }
        polygon(&mut out, &p.iter().map(|p| fr.point(p[0], p[1], p[2])).collect::<Vec<_>>());
        polygon(&mut out, &p.iter().rev().map(|p| fr.point(p[0], p[1], high.z(p[0], p[1]))).collect::<Vec<_>>());
    }
    for (a, b) in edges(outline) {
        let pts = boundary_stations(&out, fr, a, b);
        for q in pts.windows(2) {
            let mid = [(q[0][0] + q[1][0]) / 2., (q[0][1] + q[1][1]) / 2.];
            let low = plane(host, fr, mid, false)?;
            let p = |q: [f64; 2], z: Plane| fr.point(q[0], q[1], z.z(q[0], q[1]));
            polygon(&mut out, &[p(q[0], low), p(q[1], low), p(q[1], high), p(q[0], high)]);
        }
    }
    let mut lines = edges(outline);
    for t in triangles(outline)? {
        lines.extend(edges(&t));
    }
    Ok(conform_shared_edges(&conform(&out, fr, &lines)))
}
pub(super) fn prism(fr: &FacadeFrame, outline: &[[f64; 2]], low: Plane, high: Plane) -> Result<Mesh> {
    // Intersections on one rear plane can differ from a straight line by floating
    // round-off. Remove only intermediate stations within the existing 1e-6 mm
    // geometry resolution, before triangulating either skin or making the sides.
    let mut ring = outline.to_vec();
    while ring.len() > 3 {
        let n = ring.len();
        let redundant = (0..n).find(|&k| {
            let (a, p, b) = (ring[(k + n - 1) % n], ring[k], ring[(k + 1) % n]);
            let d = sub(b, a);
            let span = d[0].hypot(d[1]);
            if span <= EPS {
                return false;
            }
            let q = sub(p, a);
            let along = (q[0] * d[0] + q[1] * d[1]) / span;
            along >= 0. && along <= span && cross(d, q).abs() / span <= EPS
        });
        match redundant {
            Some(k) => {
                ring.remove(k);
            }
            None => break,
        }
    }
    let outline = ring.as_slice();
    let mut mesh = Mesh::default();
    if !mesh.polygon(outline, &[], |p| fr.point(p[0], p[1], high.z(p[0], p[1]))) {
        return Err(ModelError::Invalid("empty roof opening prism".into()));
    }
    let mut bottom = Mesh::default();
    bottom.polygon(outline, &[], |p| fr.point(p[0], p[1], low.z(p[0], p[1])));
    for t in &mut bottom.triangles {
        t.swap(1, 2);
    }
    mesh.append(&bottom);
    for (a, b) in edges(outline) {
        mesh.quad(
            fr.point(a[0], a[1], low.z(a[0], a[1])),
            fr.point(b[0], b[1], low.z(b[0], b[1])),
            fr.point(b[0], b[1], high.z(b[0], b[1])),
            fr.point(a[0], a[1], high.z(a[0], a[1])),
        );
    }
    Ok(mesh)
}
/// Piecewise rear intersection, computed from actual top triangles rather than an extended front plane.
pub(super) fn rear(host: &Mesh, fr: &FacadeFrame, u0: f64, u1: f64, front: f64, high: Plane) -> Result<Vec<[f64; 2]>> {
    let mut stations = vec![u0, u1];
    let mut faces = Vec::new();
    for t in &host.triangles {
        let p = t.map(|i| local(fr, host.positions.get(i as usize).copied().unwrap_or_default()));
        if cross(sub([p[1][0], p[1][1]], [p[0][0], p[0][1]]), sub([p[2][0], p[2][1]], [p[0][0], p[0][1]])) <= EPS {
            continue;
        }
        for k in 0..3 {
            let (a, b) = (p[k], p[(k + 1) % 3]);
            let (da, db) = (a[2] - high.z(a[0], a[1]), b[2] - high.z(b[0], b[1]));
            if da * db <= 0. && (da - db).abs() > EPS {
                let t = da / (da - db);
                let u = a[0] + t * (b[0] - a[0]);
                if u > u0 + EPS && u < u1 - EPS {
                    stations.push(u);
                }
            }
        }
        faces.push(p);
    }
    stations.sort_by(f64::total_cmp);
    stations.dedup_by(|a, b| (*a - *b).abs() < EPS);
    let mut rear = Vec::new();
    for u in stations {
        let mut crossings = Vec::new();
        for p in &faces {
            let mut ws = Vec::new();
            for k in 0..3 {
                let (a, b) = (p[k], p[(k + 1) % 3]);
                if (a[0] - u).abs() < EPS {
                    ws.push(a[1]);
                }
                if (a[0] - u) * (b[0] - u) < 0. {
                    let t = (u - a[0]) / (b[0] - a[0]);
                    ws.push(a[1] + t * (b[1] - a[1]));
                }
            }
            if ws.len() < 2 {
                continue;
            }
            let (lo, hi) = (ws.iter().copied().fold(f64::INFINITY, f64::min), ws.iter().copied().fold(f64::NEG_INFINITY, f64::max));
            let mid = (lo + hi) / 2.;
            let pl = plane(host, fr, [u, mid], true)?;
            let slope = pl.0[1] - high.0[1];
            if slope <= 0.1 {
                continue;
            }
            let w = (high.0[2] - pl.0[2] + (high.0[0] - pl.0[0]) * u) / slope;
            if w >= lo - EPS && w <= hi + EPS && w >= front + 100. + 200. && w <= front + 5000. {
                crossings.push(w);
            }
        }
        let w = crossings.into_iter().min_by(f64::total_cmp).ok_or_else(|| {
            ModelError::Invalid(format!(
                "multi-plane dormer has no rising rear intersection at u={u:.6} mm; required w=[{:.6}, {:.6}] mm",
                front + 300.,
                front + 5000.
            ))
        })?;
        rear.push([u, w]);
    }
    Ok(rear)
}
pub(super) fn footprint(rear: &[[f64; 2]], u0: f64, u1: f64, front: f64, underlap: f64) -> Result<Vec<[f64; 2]>> {
    let at = |u: f64| {
        rear.windows(2)
            .find(|p| u >= p[0][0] - EPS && u <= p[1][0] + EPS)
            .map(|p| p[0][1] + (u - p[0][0]) / (p[1][0] - p[0][0]) * (p[1][1] - p[0][1]))
            .ok_or_else(|| ModelError::Invalid(format!("roof opening rear boundary has no station at u={u:.6} mm")))
    };
    let mut out = vec![[u0, front], [u1, front], [u1, at(u1)? + underlap]];
    out.extend(rear.iter().rev().filter(|p| p[0] > u0 + EPS && p[0] < u1 - EPS).map(|p| [p[0], p[1] + underlap]));
    out.push([u0, at(u0)? + underlap]);
    Ok(out)
}
pub(super) fn overlap(a: &[[f64; 2]], b: &[[f64; 2]]) -> Result<f64> {
    let mut sum = 0.;
    for ta in triangles(a)? {
        for tb in triangles(b)? {
            let mut p = ta.map(|q| [q[0], q[1], 0.]).to_vec();
            for k in 0..3 {
                p = clip(&p, tb[k], tb[(k + 1) % 3], true);
            }
            sum += area(&p.iter().map(|q| [q[0], q[1]]).collect::<Vec<_>>());
        }
    }
    Ok(sum)
}
/// Through-window subtraction in a front wall. Rotate its local (u,w,z) to (u,z,-w).
pub(super) fn front_window(mesh: &Mesh, fr: &FacadeFrame, opening: Rect, front: f64, depth: f64) -> Result<Mesh> {
    let identity = FacadeFrame::new([0., 0.], [1000., 0.]).ok_or_else(|| ModelError::Invalid("empty opening frame".into()))?;
    let transformed = Mesh {
        positions: mesh
            .positions
            .iter()
            .map(|&p| {
                let q = local(fr, p);
                [q[0], q[2], -q[1]]
            })
            .collect(),
        triangles: mesh.triangles.clone(),
    };
    let outline = rect(opening.u0, opening.u1, opening.v0, opening.v1);
    let mut out = carve(&transformed, &identity, &outline)?;
    for (a, b) in edges(&outline) {
        out.quad([a[0], a[1], -front - depth], [a[0], a[1], -front], [b[0], b[1], -front], [b[0], b[1], -front - depth]);
    }
    let mut lines = edges(&outline);
    for t in triangles(&outline)? {
        lines.extend(edges(&t));
    }
    let out = conform_shared_edges(&conform(&out, &identity, &lines));
    Ok(Mesh { positions: out.positions.iter().map(|p| fr.point(p[0], -p[2], p[1])).collect(), triangles: out.triangles })
}
pub(super) fn limits(host: &Mesh, fr: &FacadeFrame, outline: &[[f64; 2]], top: bool) -> Result<(f64, f64)> {
    let p = patches(host, fr, outline, top)?;
    let values = p.iter().flatten().map(|p| p[2]);
    Ok((values.clone().fold(f64::INFINITY, f64::min), values.fold(f64::NEG_INFINITY, f64::max)))
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn rear_crease_stations_keep_both_prism_skins_conforming() {
        let fr = FacadeFrame::new([150000., -270000.], [150999., -269963.]).unwrap();
        let outline = [
            [0., 0.],
            [1700., 0.],
            [1700., 2200.],
            [1500., 2100.],
            [1300., 2000. + 1e-8],
            [1100., 1900.],
            [900., 1800.],
            [800., 1800.],
            [600., 1900. - 1e-8],
            [400., 2000.],
            [200., 2100.],
            [0., 2200.],
        ];
        let mesh = prism(&fr, &outline, Plane([0., 0.1, 12000.]), Plane([0., 0.1, 12065.])).unwrap();
        crate::build::dormer::tests::closed(&mesh);
    }
}
