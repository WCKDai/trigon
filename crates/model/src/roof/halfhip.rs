//! Explicit same-pitch end caps on bounded convex gable wings.
use crate::build::FacadeFrame;
use crate::{Mesh, ModelError, Result};

pub(super) fn planes(
    outline: &[[f64; 2]],
    eaves: f64,
    pitch: f64,
    gables: &[usize],
    buildup: &super::Buildup,
    caps: &[(usize, f64)],
    base: &super::PitchedRoof,
) -> Result<Vec<CapPlane>> {
    if caps.is_empty() {
        return Ok(Vec::new());
    }
    if outline.len() != 4 || gables.len() != 2 || (gables[0] + 2) % 4 != gables[1] || caps.len() > 2 {
        return Err(ModelError::Invalid("half-hip: give one or two source gables on a convex four-edge wing with opposite gables".into()));
    }
    for k in 0..4 {
        let (a, b, c) = (outline[k], outline[(k + 1) % 4], outline[(k + 2) % 4]);
        if (b[0] - a[0]) * (c[1] - b[1]) - (b[1] - a[1]) * (c[0] - b[0]) <= 1e-6 {
            return Err(ModelError::Invalid("half-hip: the four-edge source wing must be strictly convex and counter-clockwise".into()));
        }
    }
    crate::checked("half-hip roof thickness", buildup.thickness, 1., 1000.)?;
    let mut seen = std::collections::BTreeSet::new();
    let mut out = Vec::new();
    for &(edge, drop) in caps {
        if !gables.contains(&edge) || !seen.insert(edge) {
            return Err(ModelError::Invalid(format!("half-hip edge {edge}: select each existing source gable once")));
        }
        let frame = FacadeFrame::new(outline[edge], outline[(edge + 1) % 4])
            .ok_or_else(|| ModelError::Invalid("half-hip source gable has zero length".into()))?;
        let tip = base
            .gables
            .positions
            .iter()
            .filter(|p| ((p[0] - frame.origin[0]) * frame.n[0] + (p[1] - frame.origin[1]) * frame.n[1]).abs() < 1e-6)
            .map(|p| p[2])
            .fold(f64::NEG_INFINITY, f64::max);
        let plane = CapPlane::new(frame, tip, drop, pitch)?;
        // An inclusive physical boundary must survive roundoff in the normalized
        // source frame and derived intersection. Keep the 100 mm margin; only bound
        // the f64 arithmetic error by its operand scale, without snapping geometry.
        let retained = tip - eaves - drop;
        let roundoff = 64. * f64::EPSILON * (tip.abs() + eaves.abs() + drop.abs()).max(1.);
        if retained < 100. && 100. - retained > roundoff {
            return Err(ModelError::Invalid(format!(
                "half-hip edge {edge}: drop {drop:.6} mm exceeds available {:.6} mm with 100 mm retained gable",
                tip - eaves - 100.
            )));
        }
        out.push(plane);
    }
    Ok(out)
}

pub(super) fn remaining_ridge(under: &Mesh, outline: &[[f64; 2]], eaves: f64, pitch: f64, gables: &[usize]) -> Result<()> {
    let sides = (0..outline.len())
        .filter(|i| !gables.contains(i))
        .map(|i| FacadeFrame::new(outline[i], outline[(i + 1) % outline.len()]))
        .collect::<Option<Vec<_>>>()
        .ok_or_else(|| ModelError::Invalid("half-hip source eave has zero length".into()))?;
    let rise = pitch.to_radians().tan();
    // On a tapered wing the original ridge can slope. Its defining invariant is the
    // intersection of the two original eave planes, not a set of globally highest vertices.
    let crest: Vec<_> = under
        .positions
        .iter()
        .filter(|p| {
            sides.iter().all(|side| {
                let height = eaves - rise * ((p[0] - side.origin[0]) * side.n[0] + (p[1] - side.origin[1]) * side.n[1]);
                (p[2] - height).abs() < 1e-6
            })
        })
        .collect();
    let span = crest.iter().flat_map(|a| crest.iter().map(move |b| (a[0] - b[0]).hypot(a[1] - b[1]))).fold(0., f64::max);
    if span < 100. {
        return Err(ModelError::Invalid(format!("half-hip: remaining original ridge is {span:.6} mm; retain at least 100 mm")));
    }
    Ok(())
}

pub(crate) fn closed(mesh: &Mesh) -> Result<()> {
    let mut edges = std::collections::BTreeMap::<_, (u32, i32)>::new();
    let reference = mesh.positions.first().ok_or_else(|| ModelError::Invalid("half-hip shell is empty".into()))?;
    let mut volume = 0.;
    for t in &mesh.triangles {
        let mut keys = [[0i64; 3]; 3];
        let mut points = [[0.; 3]; 3];
        for ((key, p), i) in keys.iter_mut().zip(&mut points).zip(t) {
            let source = *mesh.positions.get(*i as usize).ok_or_else(|| ModelError::Invalid("half-hip shell index is absent".into()))?;
            if source.iter().any(|v| !v.is_finite() || v.abs() > 1e9) {
                return Err(ModelError::Invalid("half-hip shell coordinate is not representable".into()));
            }
            *key = source.map(|v| (v * 1e6).round() as i64);
            *p = std::array::from_fn(|j| key[j] as f64 / 1e6 - reference[j]);
        }
        if keys[0] == keys[1] || keys[1] == keys[2] || keys[2] == keys[0] || normal(points) == [0.; 3] {
            return Err(ModelError::Invalid("half-hip shell triangle collapses at 1e-6 mm".into()));
        }
        let [a, b, c] = points;
        volume += (a[0] * (b[1] * c[2] - b[2] * c[1]) + a[1] * (b[2] * c[0] - b[0] * c[2]) + a[2] * (b[0] * c[1] - b[1] * c[0])) / 6.;
        for k in 0..3 {
            let (a, b) = (keys[k], keys[(k + 1) % 3]);
            let e = edges.entry((a.min(b), a.max(b))).or_default();
            e.0 += 1;
            e.1 += if a < b { 1 } else { -1 };
        }
    }
    if edges.is_empty() || edges.values().any(|v| *v != (2, 0)) || !volume.is_finite() || volume <= 0. {
        return Err(ModelError::Invalid("half-hip: assembled roof shell is not closed and consistently wound at 1e-6 mm".into()));
    }
    Ok(())
}

#[derive(Clone, Copy)]
pub(super) struct CapPlane {
    origin: [f64; 2],
    inward: [f64; 2],
    wall_height: f64,
    rise: f64,
}

impl CapPlane {
    pub(super) fn new(frame: FacadeFrame, original_tip: f64, drop: f64, pitch: f64) -> Result<Self> {
        for v in frame.origin {
            crate::checked("half-hip source coordinate", v, -1e8, 1e8)?;
        }
        if frame.n.iter().any(|v| !v.is_finite()) || (frame.n[0].hypot(frame.n[1]) - 1.0).abs() > 1e-10 {
            return Err(ModelError::Invalid("half-hip source frame needs a finite unit normal".into()));
        }
        crate::checked("half-hip original gable tip", original_tip, -15000.0, 1e6)?;
        crate::checked("half-hip drop", drop, 1.0, 10000.0)?;
        crate::checked("half-hip host pitch", pitch, 5.0, 75.0)?;
        Ok(Self { origin: frame.origin, inward: [-frame.n[0], -frame.n[1]], wall_height: original_tip - drop, rise: pitch.to_radians().tan() })
    }

    pub(super) fn height(&self, p: [f64; 3]) -> f64 {
        self.wall_height + self.rise * ((p[0] - self.origin[0]) * self.inward[0] + (p[1] - self.origin[1]) * self.inward[1])
    }

    fn signed(&self, p: [f64; 3]) -> f64 {
        p[2] - self.height(p)
    }

    fn project(&self, p: [f64; 3]) -> [f64; 3] {
        [p[0], p[1], self.height(p)]
    }

    fn intersection(&self, mut a: [f64; 3], mut b: [f64; 3]) -> [f64; 3] {
        // Shared source edges evaluated in reverse must produce the same intersection.
        let order = a[0].total_cmp(&b[0]).then(a[1].total_cmp(&b[1])).then(a[2].total_cmp(&b[2]));
        if order.is_gt() {
            std::mem::swap(&mut a, &mut b);
        }
        let (da, db) = (self.signed(a), self.signed(b));
        let t = (da / (da - db)).clamp(0.0, 1.0);
        self.project([a[0] + (b[0] - a[0]) * t, a[1] + (b[1] - a[1]) * t, 0.0])
    }
}

fn clip(poly: &[[f64; 3]], plane: CapPlane, above: bool) -> Vec<[f64; 3]> {
    let mut out = Vec::new();
    for (&a, &b) in poly.iter().zip(poly.iter().cycle().skip(1)).take(poly.len()) {
        let (da, db) = (plane.signed(a), plane.signed(b));
        let inside = |d: f64| if above { d >= 0.0 } else { d <= 0.0 };
        if inside(da) {
            out.push(a);
        }
        if inside(da) != inside(db) {
            out.push(plane.intersection(a, b));
        }
    }
    out.dedup();
    if out.len() > 1 && out.first() == out.last() {
        out.pop();
    }
    out
}

fn normal(t: [[f64; 3]; 3]) -> [f64; 3] {
    let (a, b, c) = (t[0], t[1], t[2]);
    let (e, f) = ([b[0] - a[0], b[1] - a[1], b[2] - a[2]], [c[0] - a[0], c[1] - a[1], c[2] - a[2]]);
    [e[1] * f[2] - e[2] * f[1], e[2] * f[0] - e[0] * f[2], e[0] * f[1] - e[1] * f[0]]
}

fn fan(out: &mut Mesh, poly: &[[f64; 3]]) -> Result<()> {
    let Some(&a) = poly.first() else { return Ok(()) };
    for edge in poly.windows(2).skip(1) {
        let t = [a, edge[0], edge[1]];
        if normal(t) == [0.0; 3] {
            continue;
        }
        let base = u32::try_from(out.positions.len())
            .ok()
            .filter(|n| *n <= u32::MAX - 2)
            .ok_or_else(|| ModelError::Invalid("half-hip mesh exceeds index capacity".into()))?;
        out.positions.extend(t);
        out.triangles.push([base, base + 1, base + 2]);
    }
    Ok(())
}

/// Apply z := min(z, cap(x,y)) to an UPWARD UNTHICKENED heightfield only.
/// This does not reconstruct a shell or validate the caller's source layout.
pub(super) fn lower(under: &Mesh, plane: CapPlane) -> Result<Mesh> {
    transform(under, plane, true)
}

/// Clip vertical source gable faces; never project their upper vertices into the wall.
pub(super) fn clip_gables(gables: &Mesh, plane: CapPlane) -> Result<Mesh> {
    let clipped = transform(gables, plane, false)?;
    // Consumers identify each source gable by connected vertex indices. A fan can
    // otherwise turn one clipped face into several IFC walls. Reuse only exact
    // coordinates, including across successive caps; never round or move a vertex.
    let mut mesh = Mesh::default();
    let mut indices = std::collections::BTreeMap::new();
    for triangle in &clipped.triangles {
        let mut shared = [0u32; 3];
        for (to, from) in shared.iter_mut().zip(triangle) {
            let point = *clipped.positions.get(*from as usize).ok_or_else(|| ModelError::Invalid("clipped gable index is absent".into()))?;
            let key = point.map(|v| if v == 0. { 0 } else { v.to_bits() });
            *to = match indices.get(&key) {
                Some(i) => *i,
                None => {
                    let index =
                        u32::try_from(mesh.positions.len()).map_err(|_| ModelError::Invalid("clipped gable exceeds index capacity".into()))?;
                    mesh.positions.push(point);
                    indices.insert(key, index);
                    index
                }
            };
        }
        mesh.triangles.push(shared);
    }
    Ok(mesh)
}

fn transform(source: &Mesh, plane: CapPlane, lower_above: bool) -> Result<Mesh> {
    if source.positions.iter().flatten().any(|v| !v.is_finite() || v.abs() > 1e9) {
        return Err(ModelError::Invalid("half-hip source mesh needs finite site-local coordinates".into()));
    }
    let mut out = Mesh::default();
    for t in &source.triangles {
        let mut p = [[0.0; 3]; 3];
        for (at, &index) in p.iter_mut().zip(t) {
            *at = *source.positions.get(index as usize).ok_or_else(|| ModelError::Invalid("half-hip source mesh index is absent".into()))?;
        }
        if lower_above && normal(p)[2] <= 0.0 {
            return Err(ModelError::Invalid("half-hip lowering requires an upward unthickened heightfield".into()));
        }
        let ds = p.map(|q| plane.signed(q));
        if ds.iter().all(|d| *d <= 0.0) {
            fan(&mut out, &p)?;
        } else if ds.iter().all(|d| *d >= 0.0) {
            if lower_above {
                fan(&mut out, &p.map(|q| plane.project(q)))?;
            }
        } else {
            fan(&mut out, &clip(&p, plane, false))?;
            if lower_above {
                let cap = clip(&p, plane, true).into_iter().map(|q| plane.project(q)).collect::<Vec<_>>();
                fan(&mut out, &cap)?;
            }
        }
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn plane() -> CapPlane {
        CapPlane::new(FacadeFrame::new([0.0, 12000.0], [0.0, 0.0]).unwrap(), 16000.0, 1200.0, 45.0).unwrap()
    }

    #[test]
    fn clip_removes_the_gable_tip_without_lowering_the_ridge() {
        let outline = [[0.0, 0.0], [30000.0, 0.0], [30000.0, 12000.0], [0.0, 12000.0]];
        let base = crate::roof::pitched(&outline, 10000.0, 45.0, &[1, 3]).unwrap();
        let under = lower(&base.cover, plane()).unwrap();
        assert!((under.positions.iter().map(|p| p[2]).fold(0.0, f64::max) - 16000.0).abs() < 1e-6);
        let clipped = clip_gables(&base.gables, plane()).unwrap();
        let tip = clipped.positions.iter().filter(|p| p[0].abs() < 1e-6).map(|p| p[2]).fold(0.0, f64::max);
        assert!((tip - 14800.0).abs() < 1e-6);
        assert!(under.triangles.iter().all(|t| normal(t.map(|i| under.positions[i as usize]))[2] > 0.0));
    }

    #[test]
    fn coplanar_triangles_are_kept_once() {
        let c = plane();
        let ps = [[0.0, 0.0, 0.0], [1000.0, 0.0, 0.0], [0.0, 1000.0, 0.0]].map(|p| c.project(p));
        let input = Mesh { positions: ps.to_vec(), triangles: vec![[0, 1, 2]] };
        assert_eq!(lower(&input, c).unwrap().triangles.len(), 1);
    }

    #[test]
    fn reversed_edges_have_identical_cap_intersections() {
        let c = plane();
        let (a, b) = ([0.0, 6000.0, 16000.0], [3000.0, 6000.0, 16000.0]);
        assert_eq!(c.intersection(a, b), c.intersection(b, a));
    }

    #[test]
    fn hostile_numbers_and_missing_indices_are_refused() {
        let f = FacadeFrame::new([0.0, 12000.0], [0.0, 0.0]).unwrap();
        for d in [0.0, -1.0, f64::NAN, f64::INFINITY, 10001.0] {
            assert!(CapPlane::new(f, 16000.0, d, 45.0).is_err());
        }
        assert!(CapPlane::new(f, f64::NAN, 1200.0, 45.0).is_err());
        assert!(CapPlane::new(f, 16000.0, 1200.0, 85.0).is_err());
        let input = Mesh { positions: vec![[0.0; 3]], triangles: vec![[0, 1, 2]] };
        assert!(lower(&input, plane()).is_err());
        assert!(closed(&input).is_err());
    }

    #[test]
    fn opposite_caps_and_skew_tapered_frames_keep_a_closed_original_ridge() {
        for outline in [
            [[0., 0.], [30000., 0.], [30000., 12000.], [0., 12000.]],
            [[0., 0.], [30000., 0.], [32000., 12000.], [-600., 12000.]],
            [[0., 0.], [30000., 0.], [32000., 12010.], [-600., 12000.]],
        ] {
            for rotation in [0.0_f64, 37.25, -118.] {
                let (sin, cos) = rotation.to_radians().sin_cos();
                let outline = outline.map(|[x, y]| [183000. + x * cos - y * sin, -241000. + x * sin + y * cos]);
                for caps in [vec![(3, 1200.)], vec![(1, 900.), (3, 1200.)]] {
                    for overhang in [0., 180.] {
                        let roof = super::super::pitched_with_caps(
                            &outline,
                            9000.,
                            46.,
                            &[1, 3],
                            &super::super::Buildup { overhang, verge: 135., thickness: 200., gutters: true },
                            &caps,
                        )
                        .unwrap();
                        closed(&roof.cover).unwrap();
                    }
                }
            }
        }
    }

    #[test]
    fn inclusive_gable_limit_survives_rigid_transform_roundoff() {
        for rotation in [0.0_f64, 31., -118.] {
            for translation in [[0., 0.], [142000., -270000.]] {
                let (sin, cos) = rotation.to_radians().sin_cos();
                let outline = [[0., 0.], [30000., 0.], [30000., 12000.], [0., 12000.]]
                    .map(|[x, y]| [translation[0] + x * cos - y * sin, translation[1] + x * sin + y * cos]);
                let buildup = super::super::Buildup { overhang: 180., verge: 135., thickness: 200., gutters: false };
                let result = super::super::pitched_with_caps(&outline, 9000., 45., &[1, 3], &buildup, &[(3, 5900.)]);
                assert!(result.is_ok(), "inclusive100mm: rotation{rotation}, translation{translation:?}: {:?}", result.as_ref().err());
                assert!(
                    super::super::pitched_with_caps(&outline, 9000., 45., &[1, 3], &buildup, &[(3, 5900.001)]).is_err(),
                    "a deliberate0.001mm physical shortfall must remain refused"
                );
            }
        }
    }

    #[test]
    fn invalid_cap_hosts_and_consumed_gables_or_ridges_are_rejected() {
        let outline = [[0., 0.], [30000., 0.], [30000., 12000.], [0., 12000.]];
        let buildup = super::super::Buildup { overhang: 180., verge: 135., thickness: 200., gutters: false };
        for caps in [vec![(0, 1200.)], vec![(3, 1200.), (3, 1200.)], vec![(3, 0.)], vec![(3, f64::NAN)], vec![(3, 6000.)], vec![(3, 10001.)]] {
            assert!(super::super::pitched_with_caps(&outline, 9000., 45., &[1, 3], &buildup, &caps).is_err(), "{caps:?}");
        }
        let short = [[0., 0.], [2000., 0.], [2000., 12000.], [0., 12000.]];
        assert!(super::super::pitched_with_caps(&short, 9000., 45., &[1, 3], &buildup, &[(1, 1200.), (3, 1200.)]).is_err());
    }
}
