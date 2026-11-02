//! An explicitly fitted outer profile shared by actual steps, openings and plan symbols.
use super::{Frame, Symbol};
use crate::{ModelError, Result, Stair, StairKind, mesh::Mesh};

const EPS: f64 = 1e-6;

fn error(st: &Stair, reason: &str) -> ModelError {
    ModelError::Invalid(format!("stair {}: fitted boundary {reason}", st.id))
}

fn distance(p: [f64; 2], a: [f64; 2], b: [f64; 2]) -> f64 {
    let d = [b[0] - a[0], b[1] - a[1]];
    let t = (((p[0] - a[0]) * d[0] + (p[1] - a[1]) * d[1]) / (d[0] * d[0] + d[1] * d[1]).max(1e-30)).clamp(0., 1.);
    (p[0] - a[0] - t * d[0]).hypot(p[1] - a[1] - t * d[1])
}

fn parameters(a: [f64; 2], b: [f64; 2], ring: &[[f64; 2]]) -> Vec<f64> {
    let r = [b[0] - a[0], b[1] - a[1]];
    let mut ts = vec![0., 1.];
    for (&c, &d) in ring.iter().zip(ring.iter().cycle().skip(1)) {
        let s = [d[0] - c[0], d[1] - c[1]];
        let q = [c[0] - a[0], c[1] - a[1]];
        let den = r[0] * s[1] - r[1] * s[0];
        if den.abs() > 1e-12 * r[0].hypot(r[1]) * s[0].hypot(s[1]) {
            let t = (q[0] * s[1] - q[1] * s[0]) / den;
            let u = (q[0] * r[1] - q[1] * r[0]) / den;
            if (-1e-12..=1. + 1e-12).contains(&t) && (-1e-12..=1. + 1e-12).contains(&u) {
                ts.push(t.clamp(0., 1.));
            }
        } else {
            let n = (r[0] * r[0] + r[1] * r[1]).max(1e-30);
            for p in [c, d] {
                if distance(p, a, b) < EPS {
                    ts.push((((p[0] - a[0]) * r[0] + (p[1] - a[1]) * r[1]) / n).clamp(0., 1.));
                }
            }
        }
    }
    ts.sort_by(f64::total_cmp);
    ts.dedup_by(|a, b| (*a - *b).abs() < 1e-12);
    ts
}

pub(super) fn contains(ring: &[[f64; 2]], p: [f64; 2]) -> bool {
    let mut inside = false;
    for (&a, &b) in ring.iter().zip(ring.iter().cycle().skip(1)) {
        if distance(p, a, b) < EPS {
            return true;
        }
        if (a[1] > p[1]) != (b[1] > p[1]) && p[0] < (b[0] - a[0]) * (p[1] - a[1]) / (b[1] - a[1]) + a[0] {
            inside = !inside;
        }
    }
    inside
}

pub(super) fn segments(ring: &[[f64; 2]], a: [f64; 2], b: [f64; 2]) -> Vec<([f64; 2], [f64; 2])> {
    let at = |t: f64| [a[0] + (b[0] - a[0]) * t, a[1] + (b[1] - a[1]) * t];
    parameters(a, b, ring).windows(2).filter(|w| contains(ring, at((w[0] + w[1]) / 2.))).map(|w| (at(w[0]), at(w[1]))).collect()
}

fn covers_segment(ring: &[[f64; 2]], a: [f64; 2], b: [f64; 2]) -> bool {
    contains(ring, a)
        && contains(ring, b)
        && segments(ring, a, b).iter().map(|(a, b)| (a[0] - b[0]).hypot(a[1] - b[1])).sum::<f64>() + EPS >= (a[0] - b[0]).hypot(a[1] - b[1])
}

pub(super) fn validate(st: &Stair, f: &Frame) -> Result<()> {
    let Some(ring) = &st.boundary else { return Ok(()) };
    if !(3..=64).contains(&ring.len()) || ring.iter().flatten().any(|v| !v.is_finite() || v.abs() > 1e8) {
        return Err(error(st, "needs 3..64 finite site-local corners"));
    }
    if crate::doc::polygon_area(ring) <= 1. {
        return Err(error(st, "has no usable area"));
    }
    let (lo, hi) = match st.kind {
        StairKind::Straight => (0., f.length),
        StairKind::UTurn => (0., f.length + st.landing.unwrap_or(f.width / 2.)),
        StairKind::DoubleQuarterTurn => {
            let [a, b] = st.turn_depths.unwrap_or([f.width; 2]);
            (-a, f.length + b)
        }
    };
    for (i, &a) in ring.iter().enumerate() {
        let b = ring[(i + 1) % ring.len()];
        if (a[0] - b[0]).hypot(a[1] - b[1]) <= EPS {
            return Err(error(st, "contains a repeated corner"));
        }
        let [s, t] = f.local(a);
        if s < lo - EPS || s > hi + EPS || t < -EPS || t > f.width + EPS {
            return Err(error(st, "extends outside its flight/turn envelope; update outline and landing together"));
        }
        for j in (i + 1)..ring.len() {
            if j == i + 1 || (i == 0 && j == ring.len() - 1) {
                continue;
            }
            let c = ring[j];
            let d = ring[(j + 1) % ring.len()];
            if distance(c, a, b) < EPS
                || distance(d, a, b) < EPS
                || distance(a, c, d) < EPS
                || distance(b, c, d) < EPS
                || parameters(a, b, &[c, d]).iter().any(|t| *t > 1e-12 && *t < 1. - 1e-12)
            {
                return Err(error(st, "must be a simple polygon without crossings"));
            }
        }
    }
    let walk = super::walking_line(st, f);
    for pair in walk.windows(2) {
        if !covers_segment(ring, pair[0], pair[1]) {
            return Err(error(st, "cuts through the walking line; revise the flight frame"));
        }
    }
    for (a, b, width) in super::plan_markings(&walk) {
        let length = (b[0] - a[0]).hypot(b[1] - a[1]);
        let n = [-(b[1] - a[1]) * width / (2. * length), (b[0] - a[0]) * width / (2. * length)];
        let stroke = [[a[0] - n[0], a[1] - n[1]], [b[0] - n[0], b[1] - n[1]], [b[0] + n[0], b[1] + n[1]], [a[0] + n[0], a[1] + n[1]]];
        if stroke.iter().zip(stroke.iter().cycle().skip(1)).any(|(&a, &b)| !covers_segment(ring, a, b)) {
            return Err(error(st, "cuts through the plan arrow or start marker; revise the flight frame"));
        }
    }
    Ok(())
}

pub(super) fn prism(st: &Stair, ring: &[[f64; 2]], z: f64, height: f64) -> Result<Mesh> {
    let Some(boundary) = &st.boundary else {
        return Ok(crate::build::prism(ring, z, height));
    };
    use i_overlay::{
        core::{fill_rule::FillRule, overlay_rule::OverlayRule},
        float::overlay::FloatOverlay,
    };
    let shapes = FloatOverlay::with_subj_and_clip(&ring.to_vec(), boundary).overlay(OverlayRule::Intersect, FillRule::EvenOdd);
    if shapes.len() != 1 || shapes[0].len() != 1 {
        return Err(error(st, "removes or disconnects a tread/landing"));
    }
    let mut clipped = shapes[0][0].clone();
    // The overlay quantizes coordinates. Restore an existing source corner only when
    // it belongs to the intersection, retaining the exact wall-contact coordinates.
    for p in &mut clipped {
        if let Some(&source) = boundary
            .iter()
            .filter(|&&q| contains(ring, q))
            .chain(ring.iter().filter(|&&q| contains(boundary, q)))
            .filter(|&&q| (p[0] - q[0]).hypot(p[1] - q[1]) < 1e-5)
            .min_by(|a, b| (p[0] - a[0]).hypot(p[1] - a[1]).total_cmp(&(p[0] - b[0]).hypot(p[1] - b[1])))
        {
            *p = source;
        }
    }
    if crate::doc::polygon_area(&clipped) <= 1e-6 {
        return Err(error(st, "leaves a tread without material"));
    }
    if clipped.iter().zip(clipped.iter().cycle().skip(1)).any(|(a, b)| (a[0] - b[0]).hypot(a[1] - b[1]) <= EPS) {
        return Err(error(st, "contains a collapsed tread edge after fitting"));
    }
    if crate::footprint::signed_area2(&clipped) < 0. {
        clipped.reverse();
    }
    Ok(crate::build::prism(&clipped, z, height))
}

pub(super) fn symbol(st: &Stair, s: &mut Symbol) -> Result<()> {
    let Some(boundary) = &st.boundary else {
        return Ok(());
    };
    let well: Vec<_> = if st.kind == StairKind::UTurn { s.outline.iter().skip(3).copied().collect() } else { Vec::new() };
    s.outline = boundary.iter().copied().zip(boundary.iter().copied().cycle().skip(1)).take(boundary.len()).collect();
    s.outline.extend(well.into_iter().flat_map(|(a, b)| segments(boundary, a, b)));
    s.treads = s.treads.iter().flat_map(|(a, b)| segments(boundary, *a, *b)).collect();
    s.landing.clear();
    Ok(())
}

pub(super) fn railing(st: &Stair, mesh: &Mesh) -> Result<()> {
    if let Some(boundary) = &st.boundary {
        if mesh.positions.iter().any(|p| !contains(boundary, [p[0], p[1]])) {
            return Err(error(st, "cuts through the well railing; revise the flight frame"));
        }
        for triangle in &mesh.triangles {
            let [a, b, c] = triangle.map(|i| [mesh.positions[i as usize][0], mesh.positions[i as usize][1]]);
            for (a, b) in [(a, b), (b, c), (c, a)] {
                if !covers_segment(boundary, a, b) {
                    return Err(error(st, "cuts through the well railing; revise the flight frame"));
                }
            }
        }
    }
    Ok(())
}
