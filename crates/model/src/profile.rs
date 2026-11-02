//! Original opening outlines in facade (u, v) millimetres. Convex profiles are shared by
//! masonry voids, joinery and exchange so an arch is never a rectangular hole with a decal.

use crate::build::Rect;
use crate::component::{ComponentKind, WindowShape};

/// Outer-lining profile area, consistent with the actual faceted opening. Existing
/// rectangular families retain their exact previous width × height quantity.
pub fn area(kind: &ComponentKind, width: f64, height: f64) -> f64 {
    if matches!(kind, ComponentKind::HistoricWindow { .. }) {
        crate::doc::polygon_area(&opening(kind, Rect { u0: 0.0, u1: width, v0: 0.0, v1: height }))
    } else {
        width * height
    }
}

pub fn opening(kind: &ComponentKind, r: Rect) -> Vec<[f64; 2]> {
    match kind {
        ComponentKind::HistoricWindow { shape, .. } => outline(shape, r),
        _ => outline(&WindowShape::Rectangular, r),
    }
}

pub fn outline(shape: &WindowShape, r: Rect) -> Vec<[f64; 2]> {
    match shape {
        WindowShape::Rectangular => vec![[r.u0, r.v0], [r.u1, r.v0], [r.u1, r.v1], [r.u0, r.v1]],
        WindowShape::Round => (0..48)
            .map(|i| {
                let a = f64::from(i) * std::f64::consts::TAU / 48.0;
                [(r.u0 + r.u1) / 2.0 + (r.u1 - r.u0) / 2.0 * a.cos(), (r.v0 + r.v1) / 2.0 + (r.v1 - r.v0) / 2.0 * a.sin()]
            })
            .collect(),
        WindowShape::Segmental { rise_mm } => {
            let half = (r.u1 - r.u0) / 2.0;
            let radius = (half * half + rise_mm * rise_mm) / (2.0 * rise_mm);
            let centre = (r.u0 + r.u1) / 2.0;
            let mut p = vec![[r.u0, r.v0], [r.u1, r.v0]];
            for i in 0..=24 {
                let u = r.u1 - (r.u1 - r.u0) * f64::from(i) / 24.0;
                p.push([u, r.v1 - radius + (radius * radius - (u - centre).powi(2)).max(0.0).sqrt()]);
            }
            p
        }
    }
}

/// Clip a convex polygon to an axis-aligned rectangle, without adding collinear duplicates.
pub fn clip(poly: &[[f64; 2]], r: Rect) -> Vec<[f64; 2]> {
    let mut p = poly.to_vec();
    for (axis, at, sign) in [(0, r.u0, 1.0), (0, r.u1, -1.0), (1, r.v0, 1.0), (1, r.v1, -1.0)] {
        let mut q = Vec::new();
        for i in 0..p.len() {
            let (a, b) = (p[i], p[(i + 1) % p.len()]);
            let (da, db) = ((a[axis] - at) * sign, (b[axis] - at) * sign);
            if da >= 0.0 {
                q.push(a);
            }
            if (da >= 0.0) != (db >= 0.0) {
                let t = da / (da - db);
                q.push([a[0] + (b[0] - a[0]) * t, a[1] + (b[1] - a[1]) * t]);
            }
        }
        p = q;
    }
    loop {
        let n = p.len();
        if n < 3 {
            return Vec::new();
        }
        let redundant = (0..n).find(|&i| {
            let (a, b, c) = (p[(i + n - 1) % n], p[i], p[(i + 1) % n]);
            ((b[0] - a[0]) * (c[1] - b[1]) - (b[1] - a[1]) * (c[0] - b[0])).abs() < 1e-7
        });
        if let Some(i) = redundant {
            p.remove(i);
        } else {
            break;
        }
    }
    if crate::doc::polygon_area(&p) < 0.01 { Vec::new() } else { p }
}

pub fn width_at(poly: &[[f64; 2]], z: f64) -> Option<(f64, f64)> {
    let mut xs = Vec::new();
    for i in 0..poly.len() {
        let (a, b) = (poly[i], poly[(i + 1) % poly.len()]);
        if (a[1] <= z && z < b[1]) || (b[1] <= z && z < a[1]) {
            xs.push(a[0] + (b[0] - a[0]) * (z - a[1]) / (b[1] - a[1]));
        }
    }
    (xs.len() >= 2).then(|| (xs.iter().copied().fold(f64::INFINITY, f64::min), xs.iter().copied().fold(f64::NEG_INFINITY, f64::max)))
}
