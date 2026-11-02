//! Original lapped slate courses and roofing-felt laps, clipped to actual roof faces/voids.

use crate::{Material, Mesh, ModelError, Result, checked};
use i_overlay::{
    core::{fill_rule::FillRule, overlay_rule::OverlayRule},
    float::overlay::FloatOverlay,
};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Covering {
    #[serde(default)]
    pub lower: Surface,
    #[serde(default)]
    pub upper: Surface,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(tag = "material", rename_all = "snake_case", deny_unknown_fields)]
pub enum Surface {
    #[default]
    Plain,
    Slate {
        #[serde(default = "slate_width")]
        width_mm: f64,
        #[serde(default = "slate_length")]
        length_mm: f64,
        #[serde(default = "slate_exposure")]
        exposure_mm: f64,
        #[serde(default = "slate_thickness")]
        thickness_mm: f64,
        #[serde(default = "slate_joint")]
        joint_mm: f64,
    },
    Felt {
        #[serde(default = "felt_width")]
        sheet_width_mm: f64,
        #[serde(default = "felt_lap")]
        lap_mm: f64,
        #[serde(default = "felt_thickness")]
        thickness_mm: f64,
    },
}
fn slate_width() -> f64 {
    300.0
}
fn slate_length() -> f64 {
    400.0
}
fn slate_exposure() -> f64 {
    160.0
}
fn slate_thickness() -> f64 {
    6.0
}
fn slate_joint() -> f64 {
    3.0
}
fn felt_width() -> f64 {
    1000.0
}
fn felt_lap() -> f64 {
    100.0
}
fn felt_thickness() -> f64 {
    3.0
}

impl Surface {
    fn validate(&self) -> Result<()> {
        match self {
            Self::Plain => {}
            Self::Slate { width_mm, length_mm, exposure_mm, thickness_mm, joint_mm } => {
                checked("slate width", *width_mm, 100.0, 1000.0)?;
                checked("slate length", *length_mm, 240.0, 1500.0)?;
                checked("slate exposure", *exposure_mm, 100.0_f64.max(*length_mm / 3.0), (*length_mm - 20.0) / 2.0)?;
                checked("slate thickness", *thickness_mm, 2.0, 20.0)?;
                checked("slate joint", *joint_mm, 0.5, 10.0)?;
            }
            Self::Felt { sheet_width_mm, lap_mm, thickness_mm } => {
                checked("felt sheet width", *sheet_width_mm, 500.0, 2000.0)?;
                checked("felt lap", *lap_mm, 50.0, *sheet_width_mm / 3.0)?;
                checked("felt thickness", *thickness_mm, 1.0, 10.0)?;
            }
        }
        Ok(())
    }
}

impl Covering {
    /// Maximum normal projection of the selected construction, used to seat hosted
    /// dormer glazing above the finished roof rather than above the structural shell.
    pub(crate) fn offset(&self, pitch: f64, lower: f64, upper: f64) -> Result<f64> {
        self.lower.validate()?;
        self.upper.validate()?;
        let surface = if (pitch - lower).abs() < (pitch - upper).abs() { &self.lower } else { &self.upper };
        Ok(match surface {
            Surface::Plain => 0.0,
            Surface::Slate { length_mm, exposure_mm, thickness_mm, .. } => thickness_mm * (length_mm / exposure_mm + 1.0),
            Surface::Felt { thickness_mm, .. } => 2.0 * thickness_mm,
        })
    }
}

type Shapes = Vec<Vec<Vec<[f64; 2]>>>;
struct Face {
    normal: [f64; 3],
    along: [f64; 3],
    up: [f64; 3],
    offset: f64,
    triangles: Vec<Vec<[f64; 2]>>,
}
fn dot(a: [f64; 3], b: [f64; 3]) -> f64 {
    a.iter().zip(b).map(|(a, b)| a * b).sum()
}
fn cross(a: [f64; 3], b: [f64; 3]) -> [f64; 3] {
    [a[1] * b[2] - a[2] * b[1], a[2] * b[0] - a[0] * b[2], a[0] * b[1] - a[1] * b[0]]
}
fn unit(a: [f64; 3]) -> [f64; 3] {
    let l = dot(a, a).sqrt().max(1e-12);
    a.map(|v| v / l)
}
impl Face {
    fn point(&self, q: [f64; 2], height: f64) -> [f64; 3] {
        std::array::from_fn(|k| self.along[k] * q[0] + self.up[k] * q[1] + self.normal[k] * (self.offset + height))
    }
}

fn ring(mut points: Vec<[f64; 2]>, ccw: bool) -> Vec<[f64; 2]> {
    loop {
        let n = points.len();
        if n < 3 {
            break;
        }
        let redundant = (0..n).find(|&k| {
            let (a, b, c) = (points[(k + n - 1) % n], points[k], points[(k + 1) % n]);
            let l = (c[0] - a[0]).hypot(c[1] - a[1]);
            l < 1e-5 || ((b[0] - a[0]) * (c[1] - a[1]) - (b[1] - a[1]) * (c[0] - a[0])).abs() / l < 1e-5
        });
        if let Some(k) = redundant {
            points.remove(k);
        } else {
            break;
        }
    }
    if (crate::footprint::signed_area2(&points) > 0.0) != ccw {
        points.reverse();
    }
    points
}

/// Each cut slate remains a closed solid. Course slopes make its underside seat on the
/// preceding course's top throughout the overlap; they are not coplanar decorative overlays.
fn solid(face: &Face, shape: &[Vec<[f64; 2]>], bottom: impl Fn([f64; 2]) -> f64, thickness: f64) -> Result<Mesh> {
    let rings = shape.iter().enumerate().map(|(k, p)| ring(p.clone(), k == 0)).collect::<Vec<_>>();
    let Some((outer, holes)) = rings.split_first() else {
        return Ok(Mesh::default());
    };
    if outer.len() < 3 {
        return Ok(Mesh::default());
    }
    let mut mesh = Mesh::default();
    if !mesh.polygon(outer, holes, |q| face.point(q, bottom(q) + thickness)) {
        return Err(ModelError::Invalid("roof covering top could not be triangulated".into()));
    }
    let mut back = Mesh::default();
    if !back.polygon(outer, holes, |q| face.point(q, bottom(q))) {
        return Err(ModelError::Invalid("roof covering underside could not be triangulated".into()));
    }
    for t in &mut back.triangles {
        t.swap(1, 2);
    }
    mesh.append(&back);
    for r in &rings {
        for k in 0..r.len() {
            let (a, b) = (r[k], r[(k + 1) % r.len()]);
            mesh.quad(face.point(a, bottom(a)), face.point(b, bottom(b)), face.point(b, bottom(b) + thickness), face.point(a, bottom(a) + thickness));
        }
    }
    Ok(mesh)
}

fn intersect(contours: &[Vec<[f64; 2]>], rect: [f64; 4]) -> Shapes {
    let [x0, y0, x1, y1] = rect;
    let clip = vec![vec![[x0, y0], [x1, y0], [x1, y1], [x0, y1]]];
    FloatOverlay::with_subj_and_clip(&contours.to_vec(), &clip).overlay(OverlayRule::Intersect, FillRule::NonZero)
}

/// Additive covering on a mansard's already cut top faces; lower and upper slopes can
/// have different constructions. Original shell geometry and all opening identities remain.
pub fn build(mesh: &Mesh, lower_pitch: f64, upper_pitch: f64, spec: &Covering) -> Result<Vec<(Material, Mesh)>> {
    checked("covering lower roof pitch", lower_pitch, 5.0, 80.0)?;
    checked("covering upper roof pitch", upper_pitch, 5.0, 80.0)?;
    spec.lower.validate()?;
    spec.upper.validate()?;
    let mut faces = BTreeMap::<[i64; 4], Face>::new();
    for tri in &mesh.triangles {
        let [a, b, c] = tri.map(|i| mesh.positions.get(i as usize).copied().unwrap_or_default());
        let n = cross(std::array::from_fn(|k| b[k] - a[k]), std::array::from_fn(|k| c[k] - a[k]));
        let length = dot(n, n).sqrt();
        if length < 1e-6 || n[2] / length < 0.05 {
            continue;
        }
        let n = unit(n);
        let offset = dot(n, a);
        let key = [(n[0] * 1e7).round() as i64, (n[1] * 1e7).round() as i64, (n[2] * 1e7).round() as i64, (offset * 1e3).round() as i64];
        let face = faces.entry(key).or_insert_with(|| {
            let along = unit(cross([0., 0., 1.], n));
            let up = cross(n, along);
            Face { normal: n, along, up, offset, triangles: Vec::new() }
        });
        face.triangles.push([a, b, c].map(|p| [dot(face.along, p), dot(face.up, p)]).to_vec());
    }
    let mut parts = BTreeMap::<Material, Mesh>::new();
    let mut count = 0usize;
    for face in faces.values() {
        let pitch = face.normal[2].clamp(-1.0, 1.0).acos().to_degrees();
        let surface = if (pitch - lower_pitch).abs() < (pitch - upper_pitch).abs() { &spec.lower } else { &spec.upper };
        if matches!(surface, Surface::Plain) {
            continue;
        }
        let shapes = FloatOverlay::with_subj_and_clip(&face.triangles, &Vec::<Vec<[f64; 2]>>::new()).overlay(OverlayRule::Union, FillRule::NonZero);
        let contours = shapes.iter().flat_map(|s| s.iter().cloned()).collect::<Vec<_>>();
        let bounds = contours.iter().flatten().fold([f64::INFINITY, f64::INFINITY, f64::NEG_INFINITY, f64::NEG_INFINITY], |r, p| {
            [r[0].min(p[0]), r[1].min(p[1]), r[2].max(p[0]), r[3].max(p[1])]
        });
        if bounds.iter().any(|v| !v.is_finite()) {
            return Err(ModelError::Invalid("roof covering face is empty or nonfinite".into()));
        }
        match surface {
            Surface::Plain => {}
            Surface::Slate { width_mm, length_mm, exposure_mm, thickness_mm, joint_mm } => {
                let rows = ((bounds[3] - bounds[1]) / exposure_mm).ceil() + 2.0;
                let columns = ((bounds[2] - bounds[0]) / width_mm).ceil() + 3.0;
                if rows * columns > 20000.0 - count as f64 {
                    return Err(ModelError::Invalid("roof covering exceeds 20000 candidate slates/laps; increase the module size".into()));
                }
                count += (rows * columns) as usize;
                let target = parts.entry(Material::Slate).or_default();
                for row in 0..rows as usize {
                    let y = bounds[1] + (row as f64 - 1.0) * exposure_mm;
                    let offset = if row % 2 == 0 { 0.0 } else { width_mm / 2.0 };
                    for col in 0..columns as usize {
                        let x = bounds[0] + (col as f64 - 1.0) * width_mm + offset;
                        for shape in intersect(&contours, [x + joint_mm / 2.0, y, x + width_mm - joint_mm / 2.0, y + length_mm]) {
                            let piece = solid(face, &shape, |q| thickness_mm * (length_mm - (q[1] - y)) / exposure_mm, *thickness_mm)?;
                            target.append(&piece);
                        }
                    }
                }
            }
            Surface::Felt { sheet_width_mm, lap_mm, thickness_mm } => {
                let target = parts.entry(Material::Felt).or_default();
                for shape in &shapes {
                    target.append(&solid(face, shape, |_| 0.0, *thickness_mm)?);
                }
                let columns = ((bounds[2] - bounds[0]) / sheet_width_mm).ceil() + 1.0;
                if columns > 20000.0 - count as f64 {
                    return Err(ModelError::Invalid("roof covering exceeds 20000 candidate slates/laps".into()));
                }
                count += columns as usize;
                for col in 1..columns as usize {
                    let x = bounds[0] + col as f64 * sheet_width_mm;
                    for shape in intersect(&contours, [x - lap_mm / 2.0, bounds[1] - 1.0, x + lap_mm / 2.0, bounds[3] + 1.0]) {
                        target.append(&solid(face, &shape, |_| *thickness_mm, *thickness_mm)?);
                    }
                }
            }
        }
    }
    Ok(parts.into_iter().filter(|(_, mesh)| !mesh.is_empty()).collect())
}

#[cfg(test)]
mod tests {
    use super::*;
    fn closed(mesh: &Mesh) -> f64 {
        let mut edges = BTreeMap::<_, (u32, i32)>::new();
        let mut volume = 0.0;
        let key = |p: [f64; 3]| p.map(|v| (v * 1e5).round() as i64);
        assert!(mesh.positions.iter().flatten().all(|v| v.is_finite()));
        for tri in &mesh.triangles {
            let [a, b, c] = tri.map(|i| mesh.positions[i as usize]);
            let n = cross(std::array::from_fn(|k| b[k] - a[k]), std::array::from_fn(|k| c[k] - a[k]));
            assert!(dot(n, n) > 1e-10);
            volume += dot(a, n) / 6.0;
            for (a, b) in [(key(a), key(b)), (key(b), key(c)), (key(c), key(a))] {
                let (edge, sign) = if a < b { ((a, b), 1) } else { ((b, a), -1) };
                let e = edges.entry(edge).or_default();
                e.0 += 1;
                e.1 += sign;
            }
        }
        assert!(volume > 0.0);
        assert!(
            edges.values().all(|&(n, w)| n == 2 && w == 0),
            "open/duplicate covering edge {:?}",
            edges.iter().find(|(_, e)| e.0 != 2 || e.1 != 0)
        );
        volume
    }
    #[test]
    fn cut_slates_and_felt_keep_closed_solids_including_internal_roof_holes() {
        let angle = 68f64.to_radians();
        let f = Face {
            normal: [0., -angle.sin(), angle.cos()],
            along: [1., 0., 0.],
            up: [0., angle.cos(), angle.sin()],
            offset: 17000.,
            triangles: Vec::new(),
        };
        let shape = vec![vec![[0., 0.], [1000., 0.], [1000., 1000.], [0., 1000.]], vec![[300., 300.], [300., 700.], [700., 700.], [700., 300.]]];
        let mesh = solid(&f, &shape, |p| 4.0 - p[1] * 0.002, 6.0).unwrap();
        let volume = closed(&mesh);
        assert!((volume - 840000.0 * 6.0).abs() < 0.01);
        let mesh = solid(&f, &shape, |_| 0.0, 3.0).unwrap();
        closed(&mesh);
    }
    #[test]
    fn covering_is_deterministic_bounded_and_rejects_hostile_or_single_lap_slates() {
        let spec: Covering = serde_json::from_value(serde_json::json!({"lower":{"material":"slate"},"upper":{"material":"felt"}})).unwrap();
        let r = super::super::mansard(
            &[[0., 0.], [10000., 0.], [10000., 10000.], [0., 10000.]],
            10000.,
            &[],
            &super::super::Mansard { thickness_mm: 220., ..Default::default() },
        )
        .unwrap();
        let parts = build(&r.cover, 68., 25., &spec).unwrap();
        assert_eq!(parts.len(), 2);
        assert_eq!(build(&r.cover, 68., 25., &spec).unwrap(), parts);
        assert!(parts.iter().map(|(_, m)| m.triangles.len()).sum::<usize>() < 80000);
        for (_, m) in parts {
            assert!(m.positions.iter().flatten().all(|v| v.is_finite()));
        }
        for value in [f64::NAN, f64::INFINITY, -1e300, 1e300] {
            let bad = Covering {
                lower: Surface::Slate { width_mm: value, length_mm: 400., exposure_mm: 160., thickness_mm: 6., joint_mm: 3. },
                upper: Surface::Plain,
            };
            assert!(build(&r.cover, 68., 25., &bad).is_err());
        }
        let bad = Surface::Slate { width_mm: 300., length_mm: 400., exposure_mm: 260., thickness_mm: 6., joint_mm: 3. };
        assert!(bad.validate().is_err(), "slates require at least a double lap");
        let r = super::super::mansard(
            &[[0., 0.], [1000000., 0.], [1000000., 1000000.], [0., 1000000.]],
            10000.,
            &[],
            &super::super::Mansard { thickness_mm: 220., ..Default::default() },
        )
        .unwrap();
        assert!(build(&r.cover, 68., 25., &Covering { lower: spec.lower.clone(), upper: spec.lower }).is_err());
    }
}
