//! Convex slab and continuously swept guard, with an open wall connection.

use super::{FacadeFrame, fbox, prism, rod};
use crate::{Mesh, Result};

/// A rectangular rail section swept along an open polyline with mitred corners.
fn rail(fr: &FacadeFrame, path: &[[f64; 2]], z: f64, width: f64, height: f64) -> Mesh {
    let mut mesh = Mesh::default();
    let mut sections = Vec::new();
    let normal = |a: [f64; 2], b: [f64; 2]| {
        let d = [b[0] - a[0], b[1] - a[1]];
        let len = d[0].hypot(d[1]).max(1e-9);
        [d[1] / len, -d[0] / len]
    };
    for (k, &p) in path.iter().enumerate() {
        let n = if k == 0 {
            normal(p, path[1])
        } else if k + 1 == path.len() {
            normal(path[k - 1], p)
        } else {
            let (a, b) = (normal(path[k - 1], p), normal(p, path[k + 1]));
            let dot = (1.0 + a[0] * b[0] + a[1] * b[1]).max(1e-9);
            [(a[0] + b[0]) / dot, (a[1] + b[1]) / dot]
        };
        let at = |sign: f64, z: f64| fr.point(p[0] + sign * n[0] * width / 2.0, -p[1] - sign * n[1] * width / 2.0, z);
        sections.push([at(-1.0, z), at(1.0, z), at(1.0, z + height), at(-1.0, z + height)]);
    }
    for pair in sections.windows(2) {
        for k in 0..4 {
            let j = (k + 1) % 4;
            mesh.quad(pair[0][k], pair[0][j], pair[1][j], pair[1][k]);
        }
    }
    if let Some(s) = sections.first() {
        mesh.quad(s[3], s[2], s[1], s[0]);
    }
    if let Some(s) = sections.last() {
        mesh.quad(s[0], s[1], s[2], s[3]);
    }
    mesh
}

pub(super) fn meshes(
    fr: &FacadeFrame,
    centre: f64,
    width: f64,
    depth: f64,
    thickness: f64,
    height: f64,
    z: f64,
    plan: &crate::balcony::Plan,
) -> Result<(Mesh, Mesh)> {
    let outline = plan.outline(width, depth)?;
    let polygon = outline
        .iter()
        .map(|p| {
            let q = fr.point(centre + p[0], -p[1], 0.0);
            [q[0], q[1]]
        })
        .collect::<Vec<_>>();
    let slab = prism(&polygon, z - thickness, thickness);
    let path = plan.guard_path(width, depth)?.into_iter().map(|p| [centre + p[0], p[1]]).collect::<Vec<_>>();
    let mut guard = rail(fr, &path, z + height - 35.0, 40.0, 35.0);
    guard.append(&rail(fr, &path, z + 100.0, 25.0, 25.0));
    let mut cumulative = vec![0.0];
    for pair in path.windows(2) {
        let length = (pair[1][0] - pair[0][0]).hypot(pair[1][1] - pair[0][1]);
        cumulative.push(cumulative.last().copied().unwrap_or_default() + length);
    }
    let total = cumulative.last().copied().unwrap_or_default();
    let count = (total / 105.0).ceil().max(1.0) as usize;
    for k in 0..=count {
        let distance = total * k as f64 / count as f64;
        let i = cumulative.partition_point(|&s| s < distance).saturating_sub(1).min(path.len().saturating_sub(2));
        let (a, b) = (path[i], path[i + 1]);
        let t = ((distance - cumulative[i]) / (cumulative[i + 1] - cumulative[i])).clamp(0.0, 1.0);
        let p = [a[0] + t * (b[0] - a[0]), a[1] + t * (b[1] - a[1])];
        let post = k == 0 || k == count || k % 10 == 0;
        let size = if post { 40.0 } else { 16.0 };
        guard.append(&rod(fr, (p[0], -p[1], z - 5.0), (p[0], -p[1], z + height - 20.0), size));
        if post {
            guard.append(&fbox(fr, (p[0] - 35.0, p[0] + 35.0), (-p[1] - 35.0, -p[1] + 35.0), (z, z + 8.0)));
        }
    }
    Ok((slab, guard))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::balcony::Plan;
    use std::collections::BTreeMap;

    fn solid(mesh: &Mesh) {
        let mut edges = BTreeMap::<_, (u32, i32)>::new();
        let key = |p: [f64; 3]| p.map(|v| (v * 1e6).round() as i64);
        let mut volume = 0.0;
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
        assert!(volume > 0.0, "non-positive volume {volume}");
        assert!(edges.values().all(|&(n, w)| n == 2 && w == 0), "open/duplicate edge {:?}", edges.iter().find(|(_, e)| e.0 != 2 || e.1 != 0));
    }

    #[test]
    fn curved_and_faceted_guards_follow_closed_slabs_without_a_rear_barrier() {
        let frame = FacadeFrame::new([0., 0.], [12000., 0.]).unwrap();
        for width in [600., 3000., 6000.] {
            for depth in [400., 1500., 3000.] {
                for plan in [
                    Plan::Bowed { return_fraction: 0.35 },
                    Plan::Bowed { return_fraction: 0.7 },
                    Plan::Chamfered { corner_mm: 100. },
                    Plan::Polygon { points: vec![[-0.5, 0.], [-0.5, 0.6], [-0.3, 1.], [0.3, 1.], [0.5, 0.6], [0.5, 0.]] },
                ] {
                    let (slab, guard) = meshes(&frame, 6000., width, depth, 180., 1100., 3000., &plan).unwrap();
                    solid(&slab);
                    solid(&guard);
                    assert!(slab.triangles.len() + guard.triangles.len() < 4000, "bounded family mesh cost");
                    let (lo, hi) = slab.bounds().unwrap();
                    assert_eq!((lo[2], hi[2]), (2820., 3000.));
                    assert!(
                        (lo[0] - (6000. - width / 2.)).abs() < 1e-6
                            && (hi[0] - (6000. + width / 2.)).abs() < 1e-6
                            && hi[1] == 0.
                            && (lo[1] + depth).abs() < 1e-6
                    );
                    assert!(
                        guard.positions.iter().all(|p| p[1] <= -10.0 && p[0] >= lo[0] && p[0] <= hi[0]),
                        "guard must remain over the slab and outside the host wall"
                    );
                    assert!(
                        guard.positions.iter().all(|p| p[1] < -100.0 || (p[0] - 6000.).abs() > width / 4.),
                        "no rear rail may obstruct access from the door"
                    );
                }
            }
        }
    }

    #[test]
    fn malformed_or_hostile_balcony_plans_are_rejected() {
        for plan in [
            Plan::Bowed { return_fraction: f64::NAN },
            Plan::Chamfered { corner_mm: 1e300 },
            Plan::Polygon { points: vec![] },
            Plan::Polygon { points: vec![[-0.5, 0.], [0.5, 1.], [-0.5, 1.], [0.5, 0.]] },
            Plan::Polygon { points: vec![[-0.5, 0.], [0., f64::INFINITY], [0.5, 0.]] },
        ] {
            assert!(plan.outline(3000., 1500.).is_err());
        }
        for size in [f64::NAN, f64::INFINITY, -1e300, 1e300] {
            assert!(Plan::Bowed { return_fraction: 0.35 }.outline(size, 1500.).is_err());
        }
    }
}
