//! Optional presentation drainage. Positions are explicit facade distances, never inferred from
//! photographs. These are geometric assemblies, not hydraulic sizing or below-ground drainage.

use std::collections::BTreeSet;
use std::f64::consts::PI;

use serde::{Deserialize, Serialize};

use crate::build::FacadeFrame;
use crate::{Building, Mesh, ModelError, Result, checked};

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Drainage {
    #[serde(default = "width")]
    pub gutter_width_mm: f64,
    #[serde(default = "wall")]
    pub wall_mm: f64,
    #[serde(default = "spacing")]
    pub joint_spacing_mm: f64,
    /// No downpipes are invented when this list is empty.
    #[serde(default)]
    pub downpipes: Vec<Downpipe>,
}

fn width() -> f64 {
    150.0
}
fn wall() -> f64 {
    2.0
}
fn spacing() -> f64 {
    3000.0
}

impl Default for Drainage {
    fn default() -> Self {
        Self { gutter_width_mm: width(), wall_mm: wall(), joint_spacing_mm: spacing(), downpipes: Vec::new() }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Downpipe {
    pub id: String,
    pub facade: String,
    /// Along the original facade from its start, mm; it must survive roof straightening.
    pub along_mm: f64,
    pub diameter_mm: f64,
    /// Pipe centre from the wall's outer face, mm, including its radius.
    pub wall_offset_mm: f64,
    /// Open shoe's outlet height, site-local mm.
    pub bottom_mm: f64,
    /// Outward throw of the open shoe, mm. Zero ends vertically.
    pub shoe_mm: f64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DrainClass {
    Gutter,
    Downpipe,
}

#[derive(Debug, Clone, PartialEq)]
pub struct DrainPart {
    pub id: String,
    pub class: DrainClass,
    pub mesh: Mesh,
}

fn invalid(s: &str) -> ModelError {
    ModelError::Invalid(s.into())
}

/// Build independently identified gutter runs and downpipe assemblies. The parent roof keeps its
/// identity and unchanged cover/gables. Each run has an open half-round channel, rolled rims,
/// capped ends and sleeve joints; outlets cut a small aperture through the channel bottom.
pub fn build(
    b: &Building,
    kept: &[usize],
    outline: &[[f64; 2]],
    eaves: f64,
    overhang: f64,
    pitch: f64,
    gables: &[usize],
    d: &Drainage,
) -> Result<Vec<DrainPart>> {
    checked("drainage overhang", overhang, 0.0, 2000.0)?;
    checked("drainage pitch", pitch, 5.0, 75.0)?;
    if b.footprint.len() < 3 || kept.iter().any(|&i| i >= b.footprint.len()) || outline.iter().flatten().any(|v| !v.is_finite() || v.abs() > 1e8) {
        return Err(invalid("drainage needs finite site-local eaves and valid source corners"));
    }
    checked("gutter width", d.gutter_width_mm, 100.0, 300.0)?;
    checked("drainage wall", d.wall_mm, 1.0, 5.0)?;
    checked("gutter joint spacing", d.joint_spacing_mm, 1000.0, 10000.0)?;
    if outline.len() < 3 || outline.len() > 256 || kept.len() != outline.len() || d.downpipes.len() > 64 {
        return Err(invalid("detailed drainage needs 3 to 256 eave corners and at most 64 downpipes"));
    }
    checked("eaves", eaves, -5000.0, 500000.0)?;
    let low = eaves - overhang * pitch.to_radians().tan();
    let r = d.gutter_width_mm / 2.0;
    let mut ids = BTreeSet::new();
    let mut outlets = Vec::new();
    for p in &d.downpipes {
        if p.id.is_empty() || p.id.len() > 64 || !p.id.bytes().all(|c| c.is_ascii_alphanumeric() || b"_-".contains(&c)) || !ids.insert(&p.id) {
            return Err(invalid("downpipe IDs need 1 to 64 unique ASCII letters, digits, _ or -"));
        }
        let f = b.facade(&p.facade)?;
        if f.edge >= b.footprint.len() {
            return Err(invalid("invalid downpipe facade edge"));
        }
        let k = kept
            .iter()
            .position(|&i| i == f.edge)
            .filter(|&k| kept[(k + 1) % kept.len()] == (f.edge + 1) % b.footprint.len())
            .ok_or_else(|| invalid("downpipe facade was straightened away; lower roof straighten_mm"))?;
        if gables.contains(&k) {
            return Err(invalid("a downpipe needs an eave facade, not a gable"));
        }
        let (a, c) = b.edge(f.edge).ok_or_else(|| invalid("missing downpipe facade edge"))?;
        let wall = FacadeFrame::new(a, c).ok_or_else(|| invalid("downpipe facade too short"))?;
        checked("downpipe diameter", p.diameter_mm, 50.0, d.gutter_width_mm * 0.75)?;
        checked("downpipe along", p.along_mm, p.diameter_mm, wall.length - p.diameter_mm)?;
        checked("downpipe wall offset", p.wall_offset_mm, p.diameter_mm / 2.0 + 20.0, 1000.0)?;
        checked("downpipe shoe", p.shoe_mm, 0.0, 500.0)?;
        checked("downpipe bottom", p.bottom_mm, -5000.0, low - r - 800.0)?;
        let fr = FacadeFrame::new(outline[k], outline[(k + 1) % outline.len()]).ok_or_else(|| invalid("invalid eave"))?;
        let anchor = wall.point(p.along_mm, 0.0, 0.0);
        let u = (anchor[0] - fr.origin[0]) * fr.t[0] + (anchor[1] - fr.origin[1]) * fr.t[1];
        checked("downpipe eave position", u, p.diameter_mm, fr.length - p.diameter_mm)?;
        if outlets.iter().any(|(edge, at, _, other): &(usize, f64, FacadeFrame, &Downpipe)| {
            *edge == k && (u - at).abs() < (p.diameter_mm + other.diameter_mm) / 2.0 + 50.0
        }) {
            return Err(invalid("downpipe outlets must be separated by 50 mm of gutter"));
        }
        outlets.push((k, u, wall, p));
    }
    let mut parts = Vec::new();
    let mut joints = 0usize;
    for k in (0..outline.len()).filter(|k| !gables.contains(k)) {
        let fr = FacadeFrame::new(outline[k], outline[(k + 1) % outline.len()]).ok_or_else(|| invalid("invalid eave"))?;
        checked("detailed gutter run length", fr.length, 100.0, 200000.0)?;
        joints += (fr.length / d.joint_spacing_mm) as usize;
        if joints > 1024 {
            return Err(invalid("at most 1024 gutter joints"));
        }
        let mut breaks = vec![0.0, fr.length];
        let mine: Vec<_> = outlets.iter().filter(|(edge, _, _, _)| *edge == k).collect();
        for (_, u, _, p) in &mine {
            breaks.extend([u - p.diameter_mm / 2.0, u + p.diameter_mm / 2.0]);
        }
        breaks.sort_by(f64::total_cmp);
        breaks.dedup();
        let mut mesh = Mesh::default();
        for range in breaks.windows(2) {
            let mid = (range[0] + range[1]) / 2.0;
            let aperture = mine.iter().find(|(_, u, _, p)| (mid - u).abs() < p.diameter_mm / 2.0);
            channel(&mut mesh, &fr, range[0], range[1], low, r, d.wall_mm, aperture.map(|(_, _, _, p)| p.diameter_mm));
        }
        // Close only the metal section at run ends; the water channel remains open on top.
        for u in [0.0, fr.length] {
            for k in 0..16 {
                let a = k as f64 * PI / 16.0;
                let c = (k + 1) as f64 * PI / 16.0;
                let pt = |rad: f64, t: f64| fr.point(u, -r + rad * t.cos(), low - rad * t.sin());
                let mut pts = [pt(r, a), pt(r, c), pt(r - d.wall_mm, c), pt(r - d.wall_mm, a)];
                if u == 0.0 {
                    pts.reverse();
                }
                mesh.quad(pts[0], pts[1], pts[2], pts[3]);
            }
        }
        for w in [-d.gutter_width_mm, 0.0] {
            tube(&mut mesh, &[fr.point(0.0, w, low), fr.point(fr.length, w, low)], 4.0, 1.0);
        }
        for j in 1..=(fr.length / d.joint_spacing_mm) as usize {
            let u = j as f64 * d.joint_spacing_mm;
            if u + 25.0 < fr.length && mine.iter().all(|(_, outlet, _, p)| (u - outlet).abs() > p.diameter_mm / 2.0 + 25.0) {
                channel(&mut mesh, &fr, u - 25.0, u + 25.0, low, r + 2.0, 1.0, None);
            }
        }
        parts.push(DrainPart { id: format!("{}.roof.gutter{k}", b.id), class: DrainClass::Gutter, mesh });
        for (_, u, wall, p) in mine {
            let mut mesh = Mesh::default();
            let z = low - r;
            let socket = fr.point(*u, -r, z);
            // A short open outlet and two offset knees lead to the wall stand-off.
            let wall_pt = wall.point(p.along_mm, -p.wall_offset_mm, z - 500.0);
            let path = [
                socket,
                [socket[0], socket[1], z - 150.0],
                wall_pt,
                [wall_pt[0], wall_pt[1], p.bottom_mm + p.shoe_mm.max(50.0)],
                [wall_pt[0] + wall.n[0] * p.shoe_mm, wall_pt[1] + wall.n[1] * p.shoe_mm, p.bottom_mm],
            ];
            tube(&mut mesh, &path, p.diameter_mm / 2.0, d.wall_mm);
            // Explicit joint collars, with fixed count independent of pipe height.
            for height in [z - 500.0, p.bottom_mm + 400.0] {
                tube(&mut mesh, &[[wall_pt[0], wall_pt[1], height], [wall_pt[0], wall_pt[1], height + 40.0]], p.diameter_mm / 2.0 + 3.0, 2.0);
            }
            parts.push(DrainPart { id: format!("{}.roof.downpipe.{}", b.id, p.id), class: DrainClass::Downpipe, mesh });
        }
    }
    Ok(parts)
}

fn channel(m: &mut Mesh, fr: &FacadeFrame, start: f64, end: f64, z: f64, r: f64, wall: f64, aperture: Option<f64>) {
    for t in [0.0, PI] {
        let pt = |u: f64, rad: f64| fr.point(u, -r + rad * t.cos(), z - rad * t.sin());
        if t == 0.0 {
            m.quad(pt(start, r - wall), pt(end, r - wall), pt(end, r), pt(start, r));
        } else {
            m.quad(pt(start, r), pt(end, r), pt(end, r - wall), pt(start, r - wall));
        }
    }
    for k in 0..16 {
        let a = k as f64 * PI / 16.0;
        let b = (k + 1) as f64 * PI / 16.0;
        if aperture.is_some_and(|diam| (r * ((a + b) / 2.0).cos()).abs() < diam / 2.0) {
            continue;
        }
        let pt = |u: f64, rad: f64, t: f64| fr.point(u, -r + rad * t.cos(), z - rad * t.sin());
        m.quad(pt(start, r, a), pt(end, r, a), pt(end, r, b), pt(start, r, b));
        m.quad(pt(start, r - wall, b), pt(end, r - wall, b), pt(end, r - wall, a), pt(start, r - wall, a));
    }
}

/// Hollow polygonal tube with annular end faces and connected bends. Fixed 16-sided sections;
/// tangent bisectors give adjacent segments a shared section.
fn tube(m: &mut Mesh, path: &[[f64; 3]], r: f64, wall: f64) {
    let unit = |v: [f64; 3]| {
        let l = v.iter().map(|x| x * x).sum::<f64>().sqrt().max(1e-9);
        v.map(|x| x / l)
    };
    let sub = |a: [f64; 3], b: [f64; 3]| std::array::from_fn(|k| a[k] - b[k]);
    let mut sections = Vec::new();
    for (i, &p) in path.iter().enumerate() {
        let before = unit(sub(p, path[i.saturating_sub(1)]));
        let after = unit(sub(path[(i + 1).min(path.len() - 1)], p));
        let tangent = unit(std::array::from_fn(|k| before[k] + after[k]));
        let x = unit(if tangent[2].abs() < 0.9 { [-tangent[1], tangent[0], 0.0] } else { [0.0, -tangent[2], tangent[1]] });
        let y = [tangent[1] * x[2] - tangent[2] * x[1], tangent[2] * x[0] - tangent[0] * x[2], tangent[0] * x[1] - tangent[1] * x[0]];
        let ring = |radius: f64| -> [[f64; 3]; 16] {
            std::array::from_fn(|k| {
                let t = 2.0 * PI * k as f64 / 16.0;
                std::array::from_fn(|a| p[a] + radius * (x[a] * t.cos() + y[a] * t.sin()))
            })
        };
        sections.push((ring(r), ring(r - wall)));
    }
    for pair in sections.windows(2) {
        let [(a, ai), (b, bi)] = [pair[0], pair[1]];
        for k in 0..16 {
            let j = (k + 1) % 16;
            m.quad(a[k], a[j], b[j], b[k]);
            m.quad(ai[j], ai[k], bi[k], bi[j]);
        }
    }
    for (i, (a, b)) in sections.iter().enumerate().filter(|(i, _)| *i == 0 || *i + 1 == sections.len()) {
        for k in 0..16 {
            let j = (k + 1) % 16;
            if i == 0 {
                m.quad(a[j], a[k], b[k], b[j]);
            } else {
                m.quad(a[k], a[j], b[j], b[k]);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn building() -> Building {
        Building::new("test", "Synthetic", &[[0., 0.], [12000., 0.], [12000., 8000.], [0., 8000.]], 2, 3000.).unwrap()
    }
    #[test]
    fn channels_are_open_deterministic_and_no_pipe_is_guessed() {
        let b = building();
        let d = Drainage::default();
        let parts = build(&b, &[0, 1, 2, 3], &b.footprint, 6000., 0., 35., &[1, 3], &d).unwrap();
        assert_eq!(parts.len(), 2);
        assert!(parts.iter().all(|p| p.class == DrainClass::Gutter));
        assert_eq!(parts, build(&b, &[0, 1, 2, 3], &b.footprint, 6000., 0., 35., &[1, 3], &d).unwrap());
        assert!(parts[0].mesh.positions.iter().any(|p| (p[1] + 75.).abs() < 0.01 && (p[2] - 5925.).abs() < 0.01));
        // No sheet spans the open mouth. Old solid boxes fail this section check.
        assert!(parts[0].mesh.triangles.iter().all(|t| {
            let pts = t.map(|i| parts[0].mesh.positions[i as usize]);
            !(pts.iter().all(|p| (p[2] - 6000.).abs() < 0.01)
                && pts.iter().map(|p| p[1]).fold(f64::MIN, f64::max) - pts.iter().map(|p| p[1]).fold(f64::MAX, f64::min) > 50.)
        }));
    }
    #[test]
    fn hostile_detail_is_rejected_without_panics() {
        let b = building();
        for bad in [f64::NAN, f64::INFINITY, f64::NEG_INFINITY, f64::MAX, -1., 0.] {
            for d in [
                Drainage { gutter_width_mm: bad, ..Default::default() },
                Drainage { wall_mm: bad, ..Default::default() },
                Drainage { joint_spacing_mm: bad, ..Default::default() },
            ] {
                assert!(build(&b, &[0, 1, 2, 3], &b.footprint, 6000., 0., 35., &[], &d).is_err());
            }
        }
        assert!(build(&b, &[], &[], 6000., 0., 35., &[], &Drainage::default()).is_err());
        assert!(build(&b, &[0, 1, 2, 3], &[[f64::NAN; 2]; 4], 6000., 0., 35., &[], &Drainage::default()).is_err());
    }
}
