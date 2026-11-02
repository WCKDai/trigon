//! Convex shaped joinery and explicit true/applied muntins. All profiles are original.

use super::{Parts, hardware};
use crate::build::{FacadeFrame, Rect};
use crate::component::{Muntins, WindowShape};
use crate::{Mesh, profile};

fn bounds(p: &[[f64; 2]]) -> Rect {
    p.iter().fold(Rect { u0: f64::INFINITY, u1: f64::NEG_INFINITY, v0: f64::INFINITY, v1: f64::NEG_INFINITY }, |r, p| Rect {
        u0: r.u0.min(p[0]),
        u1: r.u1.max(p[0]),
        v0: r.v0.min(p[1]),
        v1: r.v1.max(p[1]),
    })
}

fn offset(poly: &[[f64; 2]], distance: f64) -> Vec<[f64; 2]> {
    let n = poly.len();
    (0..n)
        .map(|i| {
            let (a, b, c) = (poly[(i + n - 1) % n], poly[i], poly[(i + 1) % n]);
            let e = [b[0] - a[0], b[1] - a[1]];
            let f = [c[0] - b[0], c[1] - b[1]];
            let (le, lf) = (e[0].hypot(e[1]).max(1e-12), f[0].hypot(f[1]).max(1e-12));
            let pa = [a[0] - e[1] / le * distance, a[1] + e[0] / le * distance];
            let pb = [b[0] - f[1] / lf * distance, b[1] + f[0] / lf * distance];
            let cross = e[0] * f[1] - e[1] * f[0];
            if cross.abs() < 1e-10 {
                return pb;
            }
            let t = ((pb[0] - pa[0]) * f[1] - (pb[1] - pa[1]) * f[0]) / cross;
            [pa[0] + e[0] * t, pa[1] + e[1] * t]
        })
        .collect()
}

fn inside(poly: &[[f64; 2]], q: [f64; 2]) -> bool {
    (0..poly.len()).all(|i| {
        let (a, b) = (poly[i], poly[(i + 1) % poly.len()]);
        (b[0] - a[0]) * (q[1] - a[1]) - (b[1] - a[1]) * (q[0] - a[0]) >= -1e-6
    })
}

fn sightline(poly: &[[f64; 2]], wanted: f64) -> f64 {
    let mut d = wanted;
    for _ in 0..16 {
        let q = offset(poly, d);
        let convex = (0..q.len()).all(|i| {
            let (a, b, c) = (q[i], q[(i + 1) % q.len()], q[(i + 2) % q.len()]);
            (b[0] - a[0]) * (c[1] - b[1]) - (b[1] - a[1]) * (c[0] - b[0]) > 1e-9
        });
        if convex && q.iter().all(|p| p.iter().all(|v| v.is_finite()) && inside(poly, *p)) {
            return d;
        }
        d *= 0.5;
    }
    d
}

pub(super) fn solid(fr: &FacadeFrame, poly: &[[f64; 2]], w: (f64, f64)) -> Mesh {
    let mut m = Mesh::default();
    if poly.len() < 3 {
        return m;
    }
    let mut front = Mesh::default();
    front.polygon(poly, &[], |p| fr.point(p[0], w.0, p[1]));
    m.append(&front);
    let mut back = Mesh::default();
    back.polygon(poly, &[], |p| fr.point(p[0], w.1, p[1]));
    for t in &mut back.triangles {
        t.swap(1, 2);
    }
    m.append(&back);
    for i in 0..poly.len() {
        let (a, b) = (poly[i], poly[(i + 1) % poly.len()]);
        m.quad(fr.point(a[0], w.0, a[1]), fr.point(a[0], w.1, a[1]), fr.point(b[0], w.1, b[1]), fr.point(b[0], w.0, b[1]));
    }
    m
}

fn ring(fr: &FacadeFrame, poly: &[[f64; 2]], stations: &[(f64, f64)]) -> Mesh {
    let mut m = Mesh::default();
    for pair in stations.windows(2) {
        let ((wa, da), (wb, db)) = (pair[0], pair[1]);
        let (a, b) = (offset(poly, da), offset(poly, db));
        for i in 0..poly.len() {
            let j = (i + 1) % poly.len();
            if wa != wb {
                m.quad(
                    fr.point(poly[i][0], wa, poly[i][1]),
                    fr.point(poly[i][0], wb, poly[i][1]),
                    fr.point(poly[j][0], wb, poly[j][1]),
                    fr.point(poly[j][0], wa, poly[j][1]),
                );
            }
            m.quad(fr.point(a[i][0], wa, a[i][1]), fr.point(a[j][0], wa, a[j][1]), fr.point(b[j][0], wb, b[j][1]), fr.point(b[i][0], wb, b[i][1]));
        }
    }
    for (station, reverse) in [(stations.first(), false), (stations.last(), true)] {
        let Some(&(w, d)) = station else { continue };
        let inner = offset(poly, d);
        for i in 0..poly.len() {
            let j = (i + 1) % poly.len();
            let mut q = [
                fr.point(poly[i][0], w, poly[i][1]),
                fr.point(poly[j][0], w, poly[j][1]),
                fr.point(inner[j][0], w, inner[j][1]),
                fr.point(inner[i][0], w, inner[i][1]),
            ];
            if reverse {
                q.reverse();
            }
            m.quad(q[0], q[1], q[2], q[3]);
        }
    }
    m
}

fn pane(parts: &mut Parts, fr: &FacadeFrame, poly: &[[f64; 2]], mid: f64) {
    if poly.len() < 3 {
        return;
    }
    let gasket = sightline(poly, 3.0);
    parts.seal.append(&ring(fr, poly, &[(mid - 7.0, gasket), (mid + 7.0, gasket)]));
    parts.glass.append(&solid(fr, &offset(poly, gasket * 0.8), (mid - 3.0, mid + 3.0)));
}

/// Earcut can bridge holes across collinear boundary vertices. Split those cap edges
/// at the existing vertices so cap and extrusion sides share exactly the same edges.
fn conform_cap(cap: Mesh) -> Mesh {
    let mut out = Mesh::default();
    let add = |out: &mut Mesh, pts: [[f64; 3]; 3]| {
        let base = out.positions.len() as u32;
        out.positions.extend_from_slice(&pts);
        out.triangles.push([base, base + 1, base + 2]);
    };
    for triangle in &cap.triangles {
        let pts = triangle.map(|i| cap.positions[i as usize]);
        let mut boundary = Vec::new();
        for i in 0..3 {
            let (a, b) = (pts[i], pts[(i + 1) % 3]);
            let delta = [b[0] - a[0], b[1] - a[1], b[2] - a[2]];
            let length = delta.iter().map(|v| v * v).sum::<f64>();
            let mut along = vec![(0.0, a)];
            for p in &cap.positions {
                let t = p.iter().zip(a).zip(delta).map(|((p, a), d)| (p - a) * d).sum::<f64>() / length;
                if t > 1e-8 && t < 1.0 - 1e-8 && (0..3).all(|k| (p[k] - a[k] - t * delta[k]).abs() < 1e-7) {
                    along.push((t, *p));
                }
            }
            along.sort_by(|a, b| a.0.total_cmp(&b.0));
            along.dedup_by(|a, b| (a.0 - b.0).abs() < 1e-9);
            boundary.extend(along.into_iter().map(|(_, p)| p));
        }
        if boundary.len() == 3 {
            add(&mut out, pts);
        } else {
            let centre = std::array::from_fn(|k| (pts[0][k] + pts[1][k] + pts[2][k]) / 3.0);
            for i in 0..boundary.len() {
                add(&mut out, [centre, boundary[i], boundary[(i + 1) % boundary.len()]]);
            }
        }
    }
    out
}

/// Extrude boolean-unioned bar outlines, including any pane holes. A single solid at
/// crossings avoids the coincident end faces of separately capped horizontal bars.
fn bars(fr: &FacadeFrame, aperture: &[[f64; 2]], cells: &[Vec<[f64; 2]>], depth: (f64, f64)) -> Mesh {
    let mut m = Mesh::default();
    for shape in crate::walls::cut(aperture, cells) {
        let Some((outer, holes)) = shape.split_first() else { continue };
        // The overlay's contours can be clockwise; normalise outer and hole winding
        // before generating the side walls. Mesh::polygon handles cap triangulation.
        let mut rings = Vec::with_capacity(shape.len());
        for (i, ring) in std::iter::once(outer).chain(holes).enumerate() {
            let mut ring = ring.clone();
            loop {
                let n = ring.len();
                if n < 3 {
                    break;
                }
                let redundant = (0..n).find(|&k| {
                    let (a, b, c) = (ring[(k + n - 1) % n], ring[k], ring[(k + 1) % n]);
                    let length = (c[0] - a[0]).hypot(c[1] - a[1]);
                    length < 1e-5 || ((b[0] - a[0]) * (c[1] - a[1]) - (b[1] - a[1]) * (c[0] - a[0])).abs() / length < 1e-5
                });
                if let Some(k) = redundant {
                    ring.remove(k);
                } else {
                    break;
                }
            }
            let ccw = crate::footprint::signed_area2(&ring) > 0.0;
            if ccw != (i == 0) {
                ring.reverse();
            }
            rings.push(ring);
        }
        let Some((outer, holes)) = rings.split_first() else { continue };
        for (w, reverse) in [(depth.0, false), (depth.1, true)] {
            let mut cap = Mesh::default();
            cap.polygon(outer, holes, |p| fr.point(p[0], w, p[1]));
            cap = conform_cap(cap);
            if reverse {
                for t in &mut cap.triangles {
                    t.swap(1, 2);
                }
            }
            m.append(&cap);
        }
        for ring in &rings {
            for i in 0..ring.len() {
                let (a, b) = (ring[i], ring[(i + 1) % ring.len()]);
                m.quad(fr.point(a[0], depth.0, a[1]), fr.point(a[0], depth.1, a[1]), fr.point(b[0], depth.1, b[1]), fr.point(b[0], depth.0, b[1]));
            }
        }
    }
    m
}

fn divided(parts: &mut Parts, fr: &FacadeFrame, ap: &[[f64; 2]], w: (f64, f64), cols: u32, rows: u32, bar: f64, mode: Muntins) {
    let r = bounds(ap);
    let mid = w.1 - 14.0;
    let bw = bar.min((r.u1 - r.u0) / (f64::from(cols) * 4.0)).min((r.v1 - r.v0) / (f64::from(rows) * 4.0));
    let xs: Vec<_> = (0..=cols).map(|i| r.u0 + (r.u1 - r.u0) * f64::from(i) / f64::from(cols)).collect();
    let zs: Vec<_> = (0..=rows).map(|i| r.v0 + (r.v1 - r.v0) * f64::from(i) / f64::from(rows)).collect();
    let mut cells = Vec::new();
    for (i, x) in xs.windows(2).enumerate() {
        for (j, z) in zs.windows(2).enumerate() {
            let cell = profile::clip(
                ap,
                Rect {
                    u0: x[0] + if i > 0 { bw / 2.0 } else { 0.0 },
                    u1: x[1] - if i + 1 < cols as usize { bw / 2.0 } else { 0.0 },
                    v0: z[0] + if j > 0 { bw / 2.0 } else { 0.0 },
                    v1: z[1] - if j + 1 < rows as usize { bw / 2.0 } else { 0.0 },
                },
            );
            if !cell.is_empty() {
                cells.push(cell);
            }
        }
    }
    if mode == Muntins::TrueDivided {
        parts.frame.append(&bars(fr, ap, &cells, (w.0 + 0.5, w.1 - 0.5)));
        for cell in &cells {
            pane(parts, fr, cell, mid);
        }
    } else {
        pane(parts, fr, ap, mid);
        for depth in [(mid - 7.0, mid - 2.5), (mid + 2.5, mid + 7.0)] {
            parts.frame.append(&bars(fr, ap, &cells, depth));
        }
    }
}

pub(crate) fn window(
    fr: &FacadeFrame,
    r: Rect,
    w: (f64, f64),
    shape: &WindowShape,
    lights: u32,
    top: f64,
    cols: u32,
    rows: u32,
    bar: f64,
    mode: Muntins,
    operable: bool,
) -> Parts {
    let mut parts = Parts::default();
    let outer = profile::outline(shape, r);
    let fw = sightline(&outer, 46.0_f64.min((r.u1 - r.u0) / (4.0 * f64::from(lights) + 2.0)).min((r.v1 - r.v0) / 8.0) * 1.15) / 1.15;
    parts.frame.append(&ring(fr, &outer, &[(w.0, fw), (w.1 - 10.0, fw), (w.1 - 10.0, fw * 1.15), (w.1, fw * 1.15)]));
    let clear = offset(&outer, fw);
    let clear_bounds = bounds(&clear);
    let mullion = fw * 0.75;
    let xs: Vec<_> = (0..=lights).map(|i| clear_bounds.u0 + (clear_bounds.u1 - clear_bounds.u0) * f64::from(i) / f64::from(lights)).collect();
    let mut zs = vec![clear_bounds.v0, clear_bounds.v1];
    if top > 0.0 && clear_bounds.v1 - clear_bounds.v0 > top + 4.0 * fw {
        zs.insert(1, clear_bounds.v1 - top);
    }
    let embed = offset(&outer, (fw - 0.5).max(fw * 0.9));
    for &x in xs.iter().skip(1).take(lights.saturating_sub(1) as usize) {
        parts.frame.append(&solid(
            fr,
            &profile::clip(&embed, Rect { u0: x - mullion / 2.0, u1: x + mullion / 2.0, ..clear_bounds }),
            (w.0 + 0.5, w.1 - 0.5),
        ));
    }
    if zs.len() == 3 {
        for (i, x) in xs.windows(2).enumerate() {
            parts.frame.append(&solid(
                fr,
                &profile::clip(
                    &embed,
                    Rect {
                        u0: x[0] + if i > 0 { mullion / 2.0 } else { 0.0 },
                        u1: x[1] - if i + 1 < lights as usize { mullion / 2.0 } else { 0.0 },
                        v0: zs[1] - mullion / 2.0,
                        v1: zs[1] + mullion / 2.0,
                    },
                ),
                (w.0 + 0.5, w.1 - 0.5),
            ));
        }
    }
    for (i, x) in xs.windows(2).enumerate() {
        for (j, z) in zs.windows(2).enumerate() {
            let cell = profile::clip(
                &clear,
                Rect {
                    u0: x[0] + if i > 0 { mullion / 2.0 } else { 0.0 },
                    u1: x[1] - if i + 1 < lights as usize { mullion / 2.0 } else { 0.0 },
                    v0: z[0] + if j > 0 { mullion / 2.0 } else { 0.0 },
                    v1: z[1] - if j + 1 < zs.len() - 1 { mullion / 2.0 } else { 0.0 },
                },
            );
            if cell.len() < 3 {
                continue;
            }
            let gap = sightline(&cell, 2.0);
            let sash = offset(&cell, gap);
            let b = bounds(&sash);
            let width = sightline(&sash, 32.0_f64.min((b.u1 - b.u0).min(b.v1 - b.v0) * 0.18) * 1.13) / 1.13;
            let bevel = (width * 0.13).min(4.0);
            let sw = (w.0 + 8.0, w.1 - 12.0);
            parts.frame.append(&ring(
                fr,
                &sash,
                &[
                    (sw.0, width),
                    (sw.0 + 10.0, width),
                    (sw.0 + 14.0, width - bevel),
                    (sw.1 - 10.0, width - bevel),
                    (sw.1 - 10.0, width + bevel),
                    (sw.1, width + bevel),
                ],
            ));
            divided(&mut parts, fr, &offset(&sash, width - bevel), sw, cols, rows, bar, mode);
            let straight = match shape {
                WindowShape::Rectangular => true,
                WindowShape::Round => false,
                WindowShape::Segmental { rise_mm } => b.v1 < r.v1 - rise_mm,
            };
            if operable && straight {
                hardware(&mut parts, fr, b, sw, width, i * 2 < lights as usize, false, true);
            }
        }
    }
    parts
}
