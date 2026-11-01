//! Original two-pitch roof geometry. Dimensions are configurable family defaults, not a survey.

use super::{Buildup, PitchedRoof, boundary, default_steps, pitched_with};
use crate::{FacadeFrame, Mesh, ModelError, Result, checked};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Mansard {
    #[serde(default = "lower")]
    pub lower_pitch_deg: f64,
    #[serde(default = "upper")]
    pub upper_pitch_deg: f64,
    /// Horizontal inward run from the original exterior wall face to the underside's break.
    #[serde(default = "run")]
    pub break_run_mm: f64,
    #[serde(default)]
    pub gables: Vec<String>,
    #[serde(default = "default_steps")]
    pub straighten_mm: f64,
    #[serde(default)]
    pub overhang_mm: f64,
    #[serde(default)]
    pub verge_mm: f64,
    /// Normal thickness on both slopes; their outer faces meet at a mitred break.
    #[serde(default)]
    pub thickness_mm: f64,
    #[serde(default)]
    pub gutters: bool,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub dormers: Vec<super::Dormer>,
}

fn lower() -> f64 {
    68.0
}
fn upper() -> f64 {
    25.0
}
fn run() -> f64 {
    1300.0
}

impl Default for Mansard {
    fn default() -> Self {
        Self {
            lower_pitch_deg: lower(),
            upper_pitch_deg: upper(),
            break_run_mm: run(),
            gables: Vec::new(),
            straighten_mm: default_steps(),
            overhang_mm: 0.0,
            verge_mm: 0.0,
            thickness_mm: 0.0,
            gutters: false,
            dormers: Vec::new(),
        }
    }
}

/// Split a convex face in the skeleton's depth coordinate (temporarily stored in Z).
fn clip(poly: &[[f64; 3]], cut: f64, low: bool) -> Vec<[f64; 3]> {
    let mut out = Vec::new();
    for (&a, &b) in poly.iter().zip(poly.iter().cycle().skip(1)).take(poly.len()) {
        let (da, db) = (a[2] - cut, b[2] - cut);
        let (ina, inb) = (if low { da <= 0.0 } else { da >= 0.0 }, if low { db <= 0.0 } else { db >= 0.0 });
        if ina {
            out.push(a);
        }
        if ina != inb {
            let t = da / (da - db);
            out.push([a[0] + t * (b[0] - a[0]), a[1] + t * (b[1] - a[1]), cut]);
        }
    }
    out.dedup_by(|a, b| a.iter().zip(b.iter()).all(|(a, b)| (a - b).abs() < 1e-8));
    if out.len() > 1 && out.first().zip(out.last()).is_some_and(|(a, b)| a.iter().zip(b).all(|(a, b)| (a - b).abs() < 1e-8)) {
        out.pop();
    }
    out
}

fn surface(source: &Mesh, cuts: &[f64], height: impl Fn(f64) -> f64, reverse: bool) -> Mesh {
    let mut mesh = Mesh::default();
    for tri in &source.triangles {
        let points = tri.map(|i| source.positions.get(i as usize).copied().unwrap_or_default());
        let mut polygons = vec![points.to_vec()];
        for &cut in cuts {
            polygons = polygons
                .into_iter()
                .flat_map(|poly| {
                    if poly.iter().all(|p| p[2] <= cut) || poly.iter().all(|p| p[2] >= cut) {
                        vec![poly]
                    } else {
                        vec![clip(&poly, cut, true), clip(&poly, cut, false)]
                    }
                })
                .collect();
        }
        for poly in polygons {
            if poly.len() < 3 {
                continue;
            }
            let base = mesh.positions.len() as u32;
            mesh.positions.extend(poly.iter().map(|p| [p[0], p[1], height(p[2])]));
            for k in 1..poly.len() - 1 {
                let mut t = [base, base + k as u32, base + k as u32 + 1];
                if reverse {
                    t.swap(1, 2);
                }
                mesh.triangles.push(t);
            }
        }
    }
    mesh
}

/// A two-pitch shell with the actual lower/upper plane intersection. Splitting in skeleton
/// depth before lifting keeps hips watertight; separate top/bottom breaks maintain normal
/// thickness on each slope instead of stretching one uniform vertical extrusion.
pub fn build(outline: &[[f64; 2]], eaves: f64, gables: &[usize], p: &Mansard) -> Result<PitchedRoof> {
    checked("mansard eaves", eaves, -15000.0, 1e6)?;
    checked("mansard lower pitch", p.lower_pitch_deg, 40.0, 80.0)?;
    checked("mansard upper pitch", p.upper_pitch_deg, 5.0, 45.0)?;
    checked("mansard break run", p.break_run_mm, 200.0, 10000.0)?;
    checked("mansard straightening", p.straighten_mm, 0.0, 10000.0)?;
    if p.dormers.len() > 16 {
        return Err(ModelError::Invalid("at most 16 dormers per mansard roof".into()));
    }
    for dormer in &p.dormers {
        dormer.validate()?;
    }
    if p.lower_pitch_deg < p.upper_pitch_deg + 5.0 {
        return Err(ModelError::Invalid("mansard lower pitch must exceed upper pitch by at least 5°".into()));
    }
    let buildup = Buildup { overhang: p.overhang_mm, verge: p.verge_mm, thickness: 0.0, gutters: false };
    // At 45° the lifted Z coordinate is the skeleton's inward run from the original walls.
    let mut base = pitched_with(outline, eaves, 45.0, gables, &buildup)?;
    if p.break_run_mm >= base.max_depth - 100.0 {
        return Err(ModelError::Invalid("mansard break must leave at least 100 mm of upper roof run".into()));
    }
    let thickness = checked("mansard thickness", p.thickness_mm, 0.0, 1000.0)?;
    let (lower, upper) = (p.lower_pitch_deg.to_radians().tan(), p.upper_pitch_deg.to_radians().tan());
    let lifts = [thickness / p.lower_pitch_deg.to_radians().cos(), thickness / p.upper_pitch_deg.to_radians().cos()];
    let run = p.break_run_mm;
    let top_break = run + (lifts[1] - lifts[0]) / (lower - upper);
    if top_break <= -p.overhang_mm + 1.0 {
        return Err(ModelError::Invalid("mansard thickness consumes the lower roof slope; increase break run or reduce thickness".into()));
    }
    let bottom = |d: f64| eaves + (lower * d).min(lower * run + upper * (d - run));
    let top = |d: f64| eaves + (lower * d + lifts[0]).min(lower * run + upper * (d - run) + lifts[1]);
    for point in &mut base.cover.positions {
        point[2] -= eaves;
    }
    // Both surfaces include both break stations on their triangulation, so the fascia
    // connects without T junctions even where only one of the surfaces changes slope.
    let cuts = [run, top_break];
    let mut cover = surface(&base.cover, &cuts, top, false);
    if thickness > 0.0 {
        cover.append(&surface(&base.cover, &cuts, bottom, true));
        for (a, b) in boundary(&base.cover) {
            let mut steps = vec![0.0, 1.0];
            if (b[2] - a[2]).abs() > 1e-9 {
                for cut in [run, top_break] {
                    let t = (cut - a[2]) / (b[2] - a[2]);
                    if t > 1e-9 && t < 1.0 - 1e-9 {
                        steps.push(t);
                    }
                }
            }
            steps.sort_by(f64::total_cmp);
            steps.dedup_by(|a, b| (*a - *b).abs() < 1e-9);
            for pair in steps.windows(2) {
                let point = |t: f64| [a[0] + t * (b[0] - a[0]), a[1] + t * (b[1] - a[1]), a[2] + t * (b[2] - a[2])];
                let (a, b) = (point(pair[0]), point(pair[1]));
                cover.quad([a[0], a[1], bottom(a[2])], [b[0], b[1], bottom(b[2])], [b[0], b[1], top(b[2])], [a[0], a[1], top(a[2])]);
            }
        }
    }
    base.gables = surface(&base.gables, &[eaves + run, eaves + top_break], |z| bottom(z - eaves), false);
    let mut gutters = Mesh::default();
    if p.gutters {
        let out = crate::build::inner_ring_by(outline, |e| if gables.contains(&e) { -p.verge_mm } else { -p.overhang_mm });
        let z = top(-p.overhang_mm) - 25.0;
        for k in (0..out.len()).filter(|k| !gables.contains(k)) {
            if let Some(fr) = out.get(k).zip(out.get((k + 1) % out.len())).and_then(|(&a, &b)| FacadeFrame::new(a, b)) {
                gutters.append(&gutter(&fr, z));
            }
        }
    }
    let ridge = cover.positions.iter().map(|p| p[2]).fold(eaves, f64::max);
    Ok(PitchedRoof { cover, gutters, gables: base.gables, ridge, max_depth: base.max_depth })
}

/// Open half-round zinc channel with a real 2 mm wall and capped run ends. Separate runs
/// stop at facade corners; roof drainage design/outlet locations are project information.
fn gutter(fr: &FacadeFrame, z: f64) -> Mesh {
    let mut profile = Vec::new();
    for (radius, reverse) in [(75.0, false), (73.0, true)] {
        for k in 0..=16 {
            let k = if reverse { 16 - k } else { k };
            let a = std::f64::consts::PI * f64::from(k) / 16.0;
            profile.push([-80.0 + radius * a.cos(), z - radius * a.sin()]);
        }
    }
    let mut mesh = Mesh::default();
    for (u, reverse) in [(0.0, true), (fr.length, false)] {
        let mut cap = Mesh::default();
        cap.polygon(&profile, &[], |p| fr.point(u, p[0], p[1]));
        if reverse {
            for t in &mut cap.triangles {
                t.swap(1, 2);
            }
        }
        mesh.append(&cap);
    }
    for k in 0..profile.len() {
        let (a, b) = (profile[k], profile[(k + 1) % profile.len()]);
        mesh.quad(fr.point(0.0, a[0], a[1]), fr.point(fr.length, a[0], a[1]), fr.point(fr.length, b[0], b[1]), fr.point(0.0, b[0], b[1]));
    }
    mesh
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeMap;

    fn closed(mesh: &Mesh) {
        let mut edges = BTreeMap::<_, (u32, i32)>::new();
        let mut volume = 0.0;
        let key = |p: [f64; 3]| p.map(|v| (v * 1e6).round() as i64);
        assert!(mesh.positions.iter().flatten().all(|v| v.is_finite()));
        for tri in &mesh.triangles {
            let [a, b, c] = tri.map(|i| mesh.positions[i as usize]);
            let ab = [b[0] - a[0], b[1] - a[1], b[2] - a[2]];
            let ac = [c[0] - a[0], c[1] - a[1], c[2] - a[2]];
            let n = [ab[1] * ac[2] - ab[2] * ac[1], ab[2] * ac[0] - ab[0] * ac[2], ab[0] * ac[1] - ab[1] * ac[0]];
            assert!(n.iter().map(|v| v * v).sum::<f64>() > 1e-10, "degenerate triangle {a:?} {b:?} {c:?}");
            volume += a.iter().zip(n).map(|(v, n)| v * n).sum::<f64>() / 6.0;
            for (a, b) in [(key(a), key(b)), (key(b), key(c)), (key(c), key(a))] {
                let (edge, sign) = if a < b { ((a, b), 1) } else { ((b, a), -1) };
                let e = edges.entry(edge).or_default();
                e.0 += 1;
                e.1 += sign;
            }
        }
        assert!(volume > 0.0, "volume {volume}");
        assert!(edges.values().all(|&(n, w)| n == 2 && w == 0), "unmatched/duplicate edge: {:?}", edges.iter().find(|(_, e)| e.0 != 2 || e.1 != 0));
    }

    #[test]
    fn slopes_meet_as_closed_normally_thick_shells_on_hips_gables_and_reentrant_plans() {
        let rectangle = vec![[0., 0.], [30000., 0.], [30000., 10000.], [0., 10000.]];
        let chamfer = vec![[0., 1500.], [1500., 0.], [30000., 0.], [30000., 10000.], [0., 10000.]];
        let courtyard = vec![[0., 0.], [30000., 0.], [30000., 10000.], [10000., 10000.], [10000., 20000.], [0., 20000.]];
        for outline in [&rectangle, &chamfer, &courtyard] {
            for thickness in [80., 220., 500.] {
                let p = Mansard { thickness_mm: thickness, overhang_mm: 250., gutters: true, ..Default::default() };
                let r = build(outline, 16000., &[], &p).unwrap();
                closed(&r.cover);
                closed(&r.gutters);
                assert!(r.cover.triangles.len() < 1000, "bounded roof shell cost");
                assert_eq!(build(outline, 16000., &[], &p).unwrap(), r, "deterministic mesh ordering");
            }
        }
        let p = Mansard { thickness_mm: 220., overhang_mm: 250., verge_mm: 150., ..Default::default() };
        let r = build(&rectangle, 16000., &[1, 3], &p).unwrap();
        closed(&r.cover);
        assert!(!r.gables.is_empty());
        assert!(r.gables.positions.iter().all(|p| p[0].abs() < 1e-6 || (p[0] - 30000.).abs() < 1e-6));
        let expected = 16000. + 1300. * 68f64.to_radians().tan() + 3700. * 25f64.to_radians().tan() + 220. / 25f64.to_radians().cos();
        assert!((r.ridge - expected).abs() < 1e-6);
    }

    #[test]
    fn hostile_mansard_parameters_never_generate_nonfinite_or_consumed_slopes() {
        let outline = [[0., 0.], [10000., 0.], [10000., 10000.], [0., 10000.]];
        for field in 0..7 {
            for value in [f64::NAN, f64::INFINITY, -1e300, 1e300] {
                let mut p = Mansard::default();
                match field {
                    0 => p.lower_pitch_deg = value,
                    1 => p.upper_pitch_deg = value,
                    2 => p.break_run_mm = value,
                    3 => p.thickness_mm = value,
                    4 => p.overhang_mm = value,
                    5 => p.verge_mm = value,
                    _ => p.straighten_mm = value,
                };
                assert!(build(&outline, 16000., &[], &p).is_err());
            }
        }
        assert!(build(&outline, 16000., &[], &Mansard { break_run_mm: 5000., ..Default::default() }).is_err());
        assert!(build(&outline, 16000., &[], &Mansard { break_run_mm: 200., thickness_mm: 1000., ..Default::default() }).is_err());
    }
}
