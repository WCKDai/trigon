//! Footprint clean-up: traced outlines (OSM, GeoDanmark) carry duplicate points, a repeated
//! closing vertex and small wiggles. A facade per wiggle would be useless, so near-collinear
//! vertices are removed.

use crate::{ModelError, Result};

/// A vertex closer than this to the line through its neighbours is dropped, mm.
pub const COLLINEAR_TOLERANCE: f64 = 200.0;
const MAX_VERTICES: usize = 2000;

/// Twice the signed area (positive = counter-clockwise).
pub fn signed_area2(ring: &[[f64; 2]]) -> f64 {
    let n = ring.len();
    (0..n)
        .map(|i| {
            let a = ring[i];
            let b = ring[(i + 1) % n];
            a[0] * b[1] - b[0] * a[1]
        })
        .sum()
}

fn dist(a: [f64; 2], b: [f64; 2]) -> f64 {
    ((a[0] - b[0]).powi(2) + (a[1] - b[1]).powi(2)).sqrt()
}

/// Distance from `p` to the line through `a` and `b` (or to `a` if they coincide).
fn line_distance(p: [f64; 2], a: [f64; 2], b: [f64; 2]) -> f64 {
    let l = dist(a, b);
    if l < 1e-9 {
        return dist(p, a);
    }
    ((b[0] - a[0]) * (p[1] - a[1]) - (b[1] - a[1]) * (p[0] - a[0])).abs() / l
}

/// Cleans a footprint ring and returns it counter-clockwise with no closing vertex.
pub fn clean(points: &[[f64; 2]]) -> Result<Vec<[f64; 2]>> {
    if points.len() > MAX_VERTICES {
        return Err(ModelError::Invalid(format!("footprint has {} vertices; at most {MAX_VERTICES}", points.len())));
    }
    if points.iter().flatten().any(|v| !v.is_finite() || v.abs() > 1e8) {
        return Err(ModelError::Invalid("footprint coordinates must be finite and within 100 km of the site origin".into()));
    }
    let mut ring: Vec<[f64; 2]> = Vec::with_capacity(points.len());
    for &p in points {
        if ring.last().is_none_or(|&q| dist(p, q) > 1.0) {
            ring.push(p);
        }
    }
    while ring.len() > 1 && ring.first().zip(ring.last()).is_some_and(|(a, b)| dist(*a, *b) <= 1.0) {
        ring.pop();
    }
    // Drop the vertex that deviates least, while it is within the tolerance.
    loop {
        let n = ring.len();
        if n <= 3 {
            break;
        }
        let best = (0..n).map(|i| (i, line_distance(ring[i], ring[(i + n - 1) % n], ring[(i + 1) % n]))).min_by(|a, b| a.1.total_cmp(&b.1));
        match best {
            Some((i, d)) if d < COLLINEAR_TOLERANCE => {
                ring.remove(i);
            }
            _ => break,
        }
    }
    if ring.len() < 3 {
        return Err(ModelError::Invalid("footprint needs at least 3 distinct corners".into()));
    }
    let area2 = signed_area2(&ring);
    if area2.abs() / 2.0 < 4.0e6 {
        return Err(ModelError::Invalid(format!("footprint area {:.1} m² is too small for a building", area2.abs() / 2.0e6)));
    }
    if area2 < 0.0 {
        ring.reverse();
    }
    Ok(ring)
}

/// The ring with outward steps up to `tolerance` deep cut off: convex corners standing at most
/// `tolerance` outside the line between their neighbours are dropped, so a wall runs straight
/// along the inner corners of a stepped facade (a basement under bays). Only convex corners go,
/// so the result never leaves the ring. `may_drop(k)` says whether corner `k` may go at all.
/// Returns the kept corner indices.
pub fn inner_outline(ring: &[[f64; 2]], tolerance: f64, may_drop: impl Fn(usize) -> bool) -> Vec<usize> {
    let mut keep: Vec<usize> = (0..ring.len()).collect();
    if !(tolerance > 0.0) {
        return keep;
    }
    loop {
        let n = keep.len();
        if n <= 4 {
            break;
        }
        let mut best: Option<(usize, f64)> = None;
        for k in 0..n {
            let (p, c, q) = (ring[keep[(k + n - 1) % n]], ring[keep[k]], ring[keep[(k + 1) % n]]);
            let cross = (c[0] - p[0]) * (q[1] - p[1]) - (c[1] - p[1]) * (q[0] - p[0]);
            let len = ((q[0] - p[0]).powi(2) + (q[1] - p[1]).powi(2)).sqrt().max(1e-9);
            // How far c stands outside the chord p–q (a convex corner of a CCW ring).
            let out = cross / len;
            if out >= 0.0 && out <= tolerance && may_drop(keep[k]) && best.is_none_or(|(_, d)| out < d) {
                best = Some((k, out));
            }
        }
        match best {
            Some((k, _)) => {
                keep.remove(k);
            }
            None => break,
        }
    }
    keep
}

/// Union wall-cut polygons into continuous bands, preserving enclosed holes.
/// Inputs are solid contours, irrespective of winding; this does not simplify or move walls.
pub fn union(polygons: &[Vec<[f64; 2]>]) -> Result<Vec<Vec<Vec<[f64; 2]>>>> {
    if polygons.len() > 10_000 || polygons.iter().map(Vec::len).sum::<usize>() > 100_000 {
        return Err(ModelError::Invalid("too many cut-wall vertices".into()));
    }
    if polygons.iter().flatten().flatten().any(|x| !x.is_finite() || x.abs() > 1e8) {
        return Err(ModelError::Invalid("cut-wall coordinates must be finite site coordinates".into()));
    }
    let solid: Vec<_> = polygons
        .iter()
        .filter(|p| p.len() >= 3 && signed_area2(p).abs() > 1e-6)
        .map(|p| {
            let mut p = p.clone();
            if signed_area2(&p) < 0.0 {
                p.reverse();
            }
            p
        })
        .collect();
    if solid.is_empty() {
        return Ok(Vec::new());
    }
    use i_overlay::{
        core::{fill_rule::FillRule, overlay_rule::OverlayRule},
        float::overlay::FloatOverlay,
    };
    Ok(FloatOverlay::with_subj(&solid).overlay(OverlayRule::Union, FillRule::NonZero))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_stepped_front_gets_a_straight_line_along_its_inner_corners() {
        // A 30 × 12 m wing whose street side (y = 12000) steps out by 800 mm on every other 6 m bay.
        let mut ring = vec![[0.0, 0.0], [30000.0, 0.0]];
        for k in 0..5 {
            let (x1, x0) = (30000.0 - k as f64 * 6000.0, 24000.0 - k as f64 * 6000.0);
            let y = if k % 2 == 0 { 12000.0 } else { 12800.0 };
            ring.push([x1, y]);
            ring.push([x0, y]);
        }
        let ring = clean(&ring).unwrap();
        let kept = inner_outline(&ring, 1000.0, |_| true);
        let line: Vec<[f64; 2]> = kept.iter().map(|&i| ring[i]).collect();
        assert_eq!(line.len(), 4, "{line:?}");
        assert!(line.iter().all(|p| p[1] <= 12000.0 + 1e-6), "the bays are cut off: {line:?}");
        assert_eq!(inner_outline(&ring, 0.0, |_| true).len(), ring.len());
        assert_eq!(inner_outline(&ring, 500.0, |_| true).len(), ring.len(), "steps deeper than the tolerance stay");
        assert_eq!(inner_outline(&ring, 1000.0, |_| false).len(), ring.len(), "corners that may not go stay");
    }

    #[test]
    fn closes_orients_and_removes_wiggles() {
        // Clockwise 20 × 10 m rectangle with a closing vertex, a duplicate and a 50 mm wiggle.
        let cw = [[0.0, 0.0], [0.0, 10000.0], [10000.0, 10050.0], [20000.0, 10000.0], [20000.0, 0.0], [20000.0, 0.0], [0.0, 0.0]];
        let r = clean(&cw).unwrap();
        assert_eq!(r.len(), 4);
        assert!(signed_area2(&r) > 0.0);
    }

    #[test]
    fn refuses_degenerate_and_hostile_input() {
        assert!(clean(&[[0.0, 0.0], [1.0, 1.0]]).is_err());
        assert!(clean(&[[0.0, 0.0], [f64::NAN, 0.0], [0.0, 1.0]]).is_err());
        assert!(clean(&[[0.0, 0.0], [100.0, 0.0], [0.0, 100.0]]).is_err(), "too small");
    }

    #[test]
    fn wall_band_union_removes_junction_seams_without_filling_door_gaps() {
        let rect = |x0, y0, x1, y1| vec![[x0, y0], [x1, y0], [x1, y1], [x0, y1]];
        let walls = vec![rect(0., 0., 1000., 200.), rect(800., 0., 1000., 1500.), rect(1800., 0., 3000., 200.)];
        let united = union(&walls).unwrap();
        assert_eq!(united.len(), 2, "the T junction is one band; the 800 mm door stays open");
        let area: f64 = united.iter().flatten().map(|r| signed_area2(r)).sum::<f64>().abs() / 2.;
        assert!((area - 700_000.).abs() < 1.);
        assert!(union(&[vec![[f64::NAN, 0.], [1., 0.], [1., 1.]]]).is_err());
    }
}
