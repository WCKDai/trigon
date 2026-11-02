//! Roofs. A pitched roof is the straight skeleton of the outline lifted at one pitch: every wall
//! edge carries one roof face rising inward from the eaves. Gable edges (where a wing meets a
//! neighbour, or a gable end) keep their wall: the skeleton nodes meet that edge at the
//! intersection of the adjoining roof planes, preserving their pitch on oblique gables too.

pub mod drainage;
mod halfhip;
pub mod opening;
pub use opening::Rooflight;

use geo_straight_skeleton::Skeleton;
use serde::{Deserialize, Serialize};

use crate::mesh::Mesh;
use crate::{ModelError, Result};

mod mansard;
pub use mansard::{Mansard, build as mansard};
mod covering;
pub use covering::{Covering, Surface, build as covering};

/// Original shed dormer source parameters, relative to a straight sloping-roof facade.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Dormer {
    pub id: String,
    pub facade: String,
    pub u_mm: f64,
    /// Right-handed +Z rotation from the source facade, about its `u_mm` wall-line anchor.
    /// Explicit modelling intent, finite and within ±45 degrees; omitted means zero.
    #[serde(default, skip_serializing_if = "is_zero")]
    #[schemars(range(min = -45.0, max = 45.0))]
    pub yaw_deg: f64,
    /// Explicitly follow adjoining pitched hip planes. False retains the single-plane contract.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub multi_plane: bool,
    pub window_type: String,
    pub width_mm: f64,
    pub height_mm: f64,
    #[serde(default = "dormer_setback")]
    pub setback_mm: f64,
    #[serde(default = "dormer_cheek")]
    pub cheek_mm: f64,
    #[serde(default = "dormer_head")]
    pub head_mm: f64,
    #[serde(default = "dormer_clearance")]
    pub sill_clearance_mm: f64,
    #[serde(default = "dormer_pitch")]
    pub roof_pitch_deg: f64,
    #[serde(default = "dormer_thickness")]
    pub roof_thickness_mm: f64,
}
fn dormer_setback() -> f64 {
    200.0
}
fn dormer_cheek() -> f64 {
    100.0
}
fn dormer_head() -> f64 {
    150.0
}
fn dormer_clearance() -> f64 {
    80.0
}
fn dormer_pitch() -> f64 {
    6.0
}
fn dormer_thickness() -> f64 {
    65.0
}

impl Dormer {
    pub fn validate(&self) -> Result<()> {
        for (label, text) in [("dormer id", &self.id), ("dormer facade", &self.facade), ("dormer window type", &self.window_type)] {
            if text.is_empty() || text.len() > 128 || text.chars().any(char::is_control) {
                return Err(ModelError::Invalid(format!("{label} needs 1–128 bytes without control characters")));
            }
        }
        crate::checked("dormer centre", self.u_mm, 0.0, 1e8)?;
        crate::checked("dormer yaw (degrees about +Z from source facade)", self.yaw_deg, -45.0, 45.0)?;
        crate::checked("dormer window width", self.width_mm, 300.0, 3000.0)?;
        crate::checked("dormer window height", self.height_mm, 300.0, 2200.0)?;
        crate::checked("dormer setback", self.setback_mm, 100.0, 1500.0)?;
        crate::checked("dormer cheek", self.cheek_mm, 50.0, 200.0)?;
        crate::checked("dormer head", self.head_mm, 50.0, 400.0)?;
        crate::checked("dormer sill clearance", self.sill_clearance_mm, 20.0, 300.0)?;
        crate::checked("dormer roof pitch", self.roof_pitch_deg, 0.0, 15.0)?;
        crate::checked("dormer roof thickness", self.roof_thickness_mm, 20.0, 200.0)?;
        Ok(())
    }
}

/// An end cap at an existing source gable. The drop is below the original underside
/// gable tip at the wall line; the main eaves and remaining ridge stay fixed.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct HalfHip {
    pub facade: String,
    pub drop_mm: f64,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Roof {
    /// A flat roof slab on the walls.
    #[default]
    Flat,
    /// Pitched faces rising from the eaves (the top of the walls) at `pitch_deg`, with gables on
    /// the listed facades.
    Pitched {
        pitch_deg: f64,
        #[serde(default)]
        gables: Vec<String>,
        /// Steps in the facades up to this deep (mm) don't get their own roof faces: the eave runs
        /// straight along the outer corners (a stepped street facade under one roof plane).
        #[serde(default = "default_steps")]
        straighten_mm: f64,
        /// Optional ordered source facade lines. Adjacent lines determine each eave corner;
        /// overrides straightening without changing the footprint. None retains legacy bytes.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        eave_facades: Option<Vec<String>>,
        /// How far the roof runs past the walls at the eaves, mm (0: it stops at the wall line).
        #[serde(default, skip_serializing_if = "is_zero")]
        overhang_mm: f64,
        /// How far it runs past the gable walls (the verge), mm.
        #[serde(default, skip_serializing_if = "is_zero")]
        verge_mm: f64,
        /// The roof's build-up (rafters, battens, tiles) measured square to the roof, mm (0: a
        /// surface with no thickness). It rests on the walls: its underside meets the outer wall
        /// line at the eaves height.
        #[serde(default, skip_serializing_if = "is_zero")]
        thickness_mm: f64,
        /// Gutters along the eaves.
        #[serde(default, skip_serializing_if = "std::ops::Not::not")]
        gutters: bool,
        /// Optional detailed channels and explicit downpipes. Missing keeps legacy geometry.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        drainage: Option<drainage::Drainage>,
        /// Shed dormers; explicit multi_plane intent can follow adjoining pitched hip planes.
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        dormers: Vec<Dormer>,
        /// Optional inclined roof windows; omission/empty preserves legacy source and geometry.
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        rooflights: Vec<Rooflight>,
    },
    /// A single-pitch gable roof with one or two explicit clipped end caps.
    /// This distinct source kind makes unsupported older readers refuse the file.
    HalfHipped {
        /// Explicit clipped ends; the existing pitch also defines each end cap.
        half_hips: Vec<HalfHip>,
        pitch_deg: f64,
        #[serde(default)]
        gables: Vec<String>,
        /// Steps in the facades up to this deep (mm) don't get their own roof faces: the eave runs
        /// straight along the outer corners (a stepped street facade under one roof plane).
        #[serde(default = "default_steps")]
        straighten_mm: f64,
        /// Optional ordered source facade lines. Adjacent lines determine each eave corner;
        /// overrides straightening without changing the footprint. None retains legacy bytes.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        eave_facades: Option<Vec<String>>,
        /// How far the roof runs past the walls at the eaves, mm (0: it stops at the wall line).
        #[serde(default, skip_serializing_if = "is_zero")]
        overhang_mm: f64,
        /// How far it runs past the gable walls (the verge), mm.
        #[serde(default, skip_serializing_if = "is_zero")]
        verge_mm: f64,
        /// The roof's build-up (rafters, battens, tiles) measured square to the roof, mm (0: a
        /// surface with no thickness). It rests on the walls: its underside meets the outer wall
        /// line at the eaves height.
        #[serde(default, skip_serializing_if = "is_zero")]
        thickness_mm: f64,
        /// Gutters along the eaves.
        #[serde(default, skip_serializing_if = "std::ops::Not::not")]
        gutters: bool,
        /// Optional detailed channels and explicit downpipes. Missing keeps legacy geometry.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        drainage: Option<drainage::Drainage>,
        /// Shed dormers; explicit multi_plane intent can follow adjoining pitched hip planes.
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        dormers: Vec<Dormer>,
        /// Optional inclined roof windows; omission/empty preserves legacy source and geometry.
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        rooflights: Vec<Rooflight>,
    },
    /// Two continuous roof pitches with a shared hip/ridge skeleton and a real slope break.
    Mansard {
        #[serde(flatten)]
        parameters: Mansard,
    },
}

impl Roof {
    /// Explicit clipped-gable sources, empty for other roof kinds.
    pub fn half_hips(&self) -> &[HalfHip] {
        match self {
            Self::HalfHipped { half_hips, .. } => half_hips,
            _ => &[],
        }
    }
    /// Change only the single-pitch end-cap intent, preserving all other source parameters.
    /// An empty list restores the pitched kind. Building-specific host checks run in build().
    pub fn with_half_hips(self, half_hips: Vec<HalfHip>) -> Result<Self> {
        match self {
            Self::Pitched {
                pitch_deg,
                gables,
                straighten_mm,
                eave_facades,
                overhang_mm,
                verge_mm,
                thickness_mm,
                gutters,
                drainage,
                dormers,
                rooflights,
            }
            | Self::HalfHipped {
                pitch_deg,
                gables,
                straighten_mm,
                eave_facades,
                overhang_mm,
                verge_mm,
                thickness_mm,
                gutters,
                drainage,
                dormers,
                rooflights,
                ..
            } => {
                if half_hips.is_empty() {
                    Ok(Self::Pitched {
                        pitch_deg,
                        gables,
                        straighten_mm,
                        eave_facades,
                        overhang_mm,
                        verge_mm,
                        thickness_mm,
                        gutters,
                        drainage,
                        dormers,
                        rooflights,
                    })
                } else {
                    Ok(Self::HalfHipped {
                        pitch_deg,
                        gables,
                        straighten_mm,
                        eave_facades,
                        overhang_mm,
                        verge_mm,
                        thickness_mm,
                        gutters,
                        drainage,
                        dormers,
                        rooflights,
                        half_hips,
                    })
                }
            }
            _ => Err(ModelError::Invalid("clipped gables require an existing single-pitch roof".into())),
        }
    }
    pub fn rooflights(&self) -> &[Rooflight] {
        match self {
            Self::Flat => &[],
            Self::Pitched { rooflights, .. } | Self::HalfHipped { rooflights, .. } => rooflights,
            Self::Mansard { parameters } => &parameters.rooflights,
        }
    }
    pub fn rooflights_mut(&mut self) -> Option<&mut Vec<Rooflight>> {
        match self {
            Self::Flat => None,
            Self::Pitched { rooflights, .. } | Self::HalfHipped { rooflights, .. } => Some(rooflights),
            Self::Mansard { parameters } => Some(&mut parameters.rooflights),
        }
    }
    pub fn dormers(&self) -> &[Dormer] {
        match self {
            Self::Flat => &[],
            Self::Pitched { dormers, .. } | Self::HalfHipped { dormers, .. } => dormers,
            Self::Mansard { parameters } => &parameters.dormers,
        }
    }

    pub fn dormers_mut(&mut self) -> Option<&mut Vec<Dormer>> {
        match self {
            Self::Flat => None,
            Self::Pitched { dormers, .. } | Self::HalfHipped { dormers, .. } => Some(dormers),
            Self::Mansard { parameters } => Some(&mut parameters.dormers),
        }
    }
}

fn is_zero(v: &f64) -> bool {
    *v == 0.0
}

/// A pitched roof's build-up beyond its planes (see `Roof::Pitched`), mm.
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct Buildup {
    pub overhang: f64,
    pub verge: f64,
    pub thickness: f64,
    pub gutters: bool,
}

/// Gutter (half-round, zinc) width and depth, mm.
const GUTTER: (f64, f64) = (150.0, 110.0);

fn default_steps() -> f64 {
    1200.0
}

/// The roof's outline: the footprint with inward steps up to `tolerance` deep removed, so the eave
/// follows the outer corners. Only inner (reflex) corners are dropped, which can only enlarge the
/// outline, plus corners that are straight within 50 mm. Returns the kept corner indices.
pub fn eave_outline(ring: &[[f64; 2]], tolerance: f64) -> Vec<usize> {
    let mut keep: Vec<usize> = (0..ring.len()).collect();
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
            // Distance of c from the chord p–q; positive when c lies inside (a reflex corner of a CCW ring).
            let dev = -cross / len;
            let removable = (dev > 0.0 && dev <= tolerance) || dev.abs() <= 50.0;
            if removable && best.is_none_or(|(_, d)| dev.abs() < d) {
                best = Some((k, dev.abs()));
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

/// Intersect ordered source facade lines without moving or simplifying the source walls.
/// Each returned edge owns the corresponding original footprint edge. The first corner is
/// the intersection of the last and first selected lines; no point is an arbitrary input.
pub(crate) fn selected_eave_outline(b: &crate::Building, facades: &[String]) -> Result<(Vec<usize>, Vec<[f64; 2]>)> {
    let invalid = |message: String| ModelError::Invalid(format!("eave_facades: {message}"));
    let n = b.footprint.len();
    if !(3..=2000).contains(&n) || !(3..=256).contains(&facades.len()) || facades.len() > n {
        return Err(invalid("select 3 to 256 source facades in footprint order, on a footprint with 3 to 2000 corners".into()));
    }
    if b.footprint.iter().flatten().any(|v| !v.is_finite() || v.abs() > 1e8) {
        return Err(invalid("source footprint must be finite and within 100 km of the site origin".into()));
    }
    let mut ids = std::collections::BTreeSet::new();
    let mut edges = Vec::with_capacity(facades.len());
    let mut frames = Vec::with_capacity(facades.len());
    for id in facades {
        if id.is_empty() || id.len() > 128 || id.chars().any(char::is_control) {
            return Err(invalid("facade IDs need 1 to 128 bytes without control characters".into()));
        }
        if !ids.insert(id) {
            return Err(invalid(format!("duplicate facade {id}; select each source line once")));
        }
        let mut matches = b.facades.iter().filter(|f| &f.id == id);
        let f = matches.next().ok_or_else(|| invalid(format!("unknown facade {id}; select a facade of {}", b.id)))?;
        if matches.next().is_some() || edges.contains(&f.edge) {
            return Err(invalid(format!("{id} does not identify a unique source edge")));
        }
        let (a, c) = b.edge(f.edge).ok_or_else(|| invalid(format!("{id} has no source edge")))?;
        let frame = crate::FacadeFrame::new(a, c).ok_or_else(|| invalid(format!("{id} has a degenerate source line")))?;
        crate::checked(&format!("eave_facades source length {id}"), frame.length, 1.0, 200000.0)?;
        edges.push(f.edge);
        frames.push(frame);
    }
    let travelled: usize = edges.iter().zip(edges.iter().cycle().skip(1)).take(edges.len()).map(|(&a, &c)| (c + n - a) % n).sum();
    if travelled != n {
        return Err(invalid("facades must follow the counter-clockwise source footprint order once; any cyclic starting facade is allowed".into()));
    }
    let mut outline = Vec::with_capacity(frames.len());
    for k in 0..frames.len() {
        let prev = (k + frames.len() - 1) % frames.len();
        let (a, c) = (frames[prev], frames[k]);
        let det = a.t[0] * c.t[1] - a.t[1] * c.t[0];
        if det.abs() < 1e-6 {
            return Err(invalid(format!(
                "{} and {} have parallel or unstable lines; select a turning facade between them",
                facades[prev], facades[k]
            )));
        }
        let delta = [c.origin[0] - a.origin[0], c.origin[1] - a.origin[1]];
        let along = (delta[0] * c.t[1] - delta[1] * c.t[0]) / det;
        let p = [a.origin[0] + along * a.t[0], a.origin[1] + along * a.t[1]];
        if p.iter().any(|v| !v.is_finite() || v.abs() > 1e8)
            || (p[0] - a.origin[0]).hypot(p[1] - a.origin[1]) > 200000.0
            || (p[0] - c.origin[0]).hypot(p[1] - c.origin[1]) > 200000.0
        {
            return Err(invalid(format!(
                "intersection of {} and {} is unbounded; select nearer intersecting facade lines (within 200 m)",
                facades[prev], facades[k]
            )));
        }
        outline.push(p);
    }
    for (k, fr) in frames.iter().enumerate() {
        let (a, c) = (outline[k], outline[(k + 1) % outline.len()]);
        let forward = (c[0] - a[0]) * fr.t[0] + (c[1] - a[1]) * fr.t[1];
        crate::checked(&format!("eave_facades forward run {}", facades[k]), forward, 1.0, 200000.0)?;
    }
    crate::site::polygon(&outline).map_err(|e| invalid(format!("selected lines must form a simple ring: {e}")))?;
    if crate::footprint::signed_area2(&outline) <= 0.0 {
        return Err(invalid("selected lines must form a counter-clockwise ring".into()));
    }
    if let Some(edge) = uncovered_edge(&outline, &b.footprint) {
        let id = b.facades.iter().find(|f| f.edge == edge).map_or_else(|| format!("source edge {edge}"), |f| f.id.clone());
        return Err(invalid(format!(
            "selected ring does not cover the full footprint at {id}; include an outer source facade, without moving the walls"
        )));
    }
    Ok((edges, outline))
}

/// Test every open interval of each source edge, split where it meets the selected boundary.
/// Checking only the source vertices misses an edge crossing a concavity of the selected ring.
fn uncovered_edge(outline: &[[f64; 2]], source: &[[f64; 2]]) -> Option<usize> {
    let cross = |a: [f64; 2], b: [f64; 2]| a[0] * b[1] - a[1] * b[0];
    let sub = |a: [f64; 2], b: [f64; 2]| [a[0] - b[0], a[1] - b[1]];
    let inside = |p: [f64; 2]| {
        let mut yes = false;
        for (&a, &c) in outline.iter().zip(outline.iter().cycle().skip(1)).take(outline.len()) {
            let d = sub(c, a);
            let length = d[0].hypot(d[1]);
            if cross(d, sub(p, a)).abs() <= 1e-6 * length && (0..2).all(|i| p[i] >= a[i].min(c[i]) - 1e-6 && p[i] <= a[i].max(c[i]) + 1e-6) {
                return true;
            }
            if (a[1] > p[1]) != (c[1] > p[1]) && p[0] < a[0] + (p[1] - a[1]) * d[0] / d[1] {
                yes = !yes;
            }
        }
        yes
    };
    for (edge, (&a, &c)) in source.iter().zip(source.iter().cycle().skip(1)).take(source.len()).enumerate() {
        if !inside(a) || !inside(c) {
            return Some(edge);
        }
        let d = sub(c, a);
        let length2 = d[0] * d[0] + d[1] * d[1];
        if length2 < 1.0 {
            return Some(edge);
        }
        let mut stations = vec![0.0, 1.0];
        for (&p, &q) in outline.iter().zip(outline.iter().cycle().skip(1)).take(outline.len()) {
            let e = sub(q, p);
            let det = cross(d, e);
            if det.abs() > 1e-12 {
                let delta = sub(p, a);
                let t = cross(delta, e) / det;
                let u = cross(delta, d) / det;
                if (0.0..=1.0).contains(&t) && (-1e-10..=1.0 + 1e-10).contains(&u) {
                    stations.push(t);
                }
            } else {
                for v in [p, q] {
                    let delta = sub(v, a);
                    let t = (delta[0] * d[0] + delta[1] * d[1]) / length2;
                    if (0.0..=1.0).contains(&t) {
                        stations.push(t);
                    }
                }
            }
        }
        stations.sort_by(f64::total_cmp);
        for pair in stations.windows(2) {
            let t = (pair[0] + pair[1]) / 2.0;
            if !inside([a[0] + t * d[0], a[1] + t * d[1]]) {
                return Some(edge);
            }
        }
    }
    None
}

/// A pitched roof's geometry, in site mm.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct PitchedRoof {
    /// The roof: its surface, or with a build-up a closed slab (surface, soffit, fascia and verges).
    pub cover: Mesh,
    /// Gutters along the eaves.
    pub gutters: Mesh,
    /// Vertical gable triangles above gable walls.
    pub gables: Mesh,
    /// Highest point, mm above the terrain.
    pub ridge: f64,
    /// Largest inward distance on the skeleton (half the widest wing), mm.
    pub max_depth: f64,
}

/// The largest inward distance of the outline's straight skeleton (half its widest part), mm.
pub fn max_depth(outline: &[[f64; 2]]) -> Result<f64> {
    let sk = skeleton(outline)?;
    Ok(sk.vertices.iter().map(|v| v[2]).fold(0.0, f64::max))
}

fn skeleton(outline: &[[f64; 2]]) -> Result<Skeleton> {
    Skeleton::build(&[outline.to_vec()]).map_err(|e| ModelError::Invalid(format!("the outline has no roof skeleton: {e:?}")))
}

/// Builds a pitched roof over `outline` (counter-clockwise, site mm) from `eaves` (mm above the
/// terrain) at `pitch_deg`, with gables on the edges in `gable_edges` (edge `k` runs from point
/// `k` to the next).
pub fn pitched(outline: &[[f64; 2]], eaves: f64, pitch_deg: f64, gable_edges: &[usize]) -> Result<PitchedRoof> {
    if !(5.0..=75.0).contains(&pitch_deg) {
        return Err(ModelError::Invalid(format!("roof pitch must be 5° to 75° (got {pitch_deg})")));
    }
    let sk = skeleton(outline)?;
    let n = outline.len();
    if sk.faces.len() != n || sk.vertices.len() < n {
        return Err(ModelError::Invalid("roof skeleton does not match its outline".into()));
    }
    let mut verts = sk.vertices.clone();
    for &e in gable_edges {
        if e >= n {
            return Err(ModelError::Invalid(format!("gable edge {e} doesn't exist")));
        }
    }
    if !gable_edges.is_empty() {
        let mut incident = vec![Vec::new(); verts.len()];
        for (edge, face) in sk.faces.iter().enumerate() {
            for &vertex in &face.vertices {
                if let Some(edges) = incident.get_mut(vertex) {
                    edges.push(edge);
                }
            }
        }
        let planes: Vec<_> = outline
            .iter()
            .zip(outline.iter().cycle().skip(1))
            .take(n)
            .map(|(a, b)| {
                let (dx, dy) = (b[0] - a[0], b[1] - a[1]);
                let length = dx.hypot(dy);
                let normal = [-dy / length, dx / length];
                (normal, normal[0] * a[0] + normal[1] * a[1])
            })
            .collect();
        for (vertex, edges) in verts.iter_mut().zip(&incident).skip(n) {
            let gables: Vec<_> = edges.iter().copied().filter(|e| gable_edges.contains(e)).collect();
            let Some(&gable) = gables.first() else { continue };
            let slopes: Vec<_> = edges.iter().copied().filter(|e| !gable_edges.contains(e)).collect();
            let Some(&slope) = slopes.first() else {
                return Err(ModelError::Invalid("gable layout leaves no adjoining roof plane".into()));
            };
            let (a, c) = planes[gable];
            let (normal, constant) = planes[slope];
            let (b, d) = if let Some(&second) = slopes.get(1) {
                // Equal signed distance to two eave lines defines their ridge/valley.
                let (other, value) = planes[second];
                ([normal[0] - other[0], normal[1] - other[1]], constant - value)
            } else if let Some(&second) = gables.get(1) {
                planes[second]
            } else {
                // A degree-two skeleton station retains its distance to the sole slope.
                (normal, constant + vertex[2])
            };
            let det = a[0] * b[1] - a[1] * b[0];
            if !det.is_finite() || det.abs() < 1e-10 {
                return Err(ModelError::Invalid("gable layout has no stable intersection with its adjoining roof planes".into()));
            }
            let x = (c * b[1] - a[1] * d) / det;
            let y = (a[0] * d - c * b[0]) / det;
            let depth = normal[0] * x + normal[1] * y - constant;
            if !x.is_finite() || !y.is_finite() || !depth.is_finite() || depth < -1e-6 {
                return Err(ModelError::Invalid("gable intersection lies below its eaves".into()));
            }
            for &edge in edges {
                let (normal, constant) = planes[edge];
                let expected = if gable_edges.contains(&edge) { 0. } else { depth };
                if (normal[0] * x + normal[1] * y - constant - expected).abs() > 1e-5 {
                    return Err(ModelError::Invalid(
                        "gable layout needs a different roof topology; adjoining roof planes cannot meet at one node".into(),
                    ));
                }
            }
            *vertex = [x, y, depth.max(0.)];
        }
    }
    let max_depth = verts.iter().map(|v| v[2]).fold(0.0, f64::max);
    let rise = pitch_deg.to_radians().tan();
    let z = |v: &[f64; 3]| eaves + rise * v[2];
    let mut roof = PitchedRoof { max_depth, ..Default::default() };
    for (k, face) in sk.faces.iter().enumerate() {
        if face.vertices.len() < 3 {
            continue;
        }
        let pts: Vec<[f64; 3]> = face.vertices.iter().filter_map(|&i| verts.get(i)).map(|v| [v[0], v[1], z(v)]).collect();
        if pts.len() < 3 {
            continue;
        }
        if gable_edges.contains(&k) {
            // Vertical: triangulate in the edge's (along, up) plane.
            let (Some(a), Some(b)) = (outline.get(k), outline.get((k + 1) % n)) else { continue };
            let d = [b[0] - a[0], b[1] - a[1]];
            let l = (d[0] * d[0] + d[1] * d[1]).sqrt().max(1e-9);
            let flat: Vec<f64> = pts.iter().flat_map(|p| [((p[0] - a[0]) * d[0] + (p[1] - a[1]) * d[1]) / l, p[2]]).collect();
            push_triangulated(&mut roof.gables, &pts, &flat);
        } else {
            let flat: Vec<f64> = pts.iter().flat_map(|p| [p[0], p[1]]).collect();
            push_triangulated(&mut roof.cover, &pts, &flat);
        }
    }
    roof.ridge = roof.cover.positions.iter().map(|p| p[2]).fold(eaves, f64::max);
    // Faces point up and out.
    for t in &mut roof.cover.triangles {
        let p = |i: u32| roof.cover.positions.get(i as usize).copied().unwrap_or_default();
        let (a, b, c) = (p(t[0]), p(t[1]), p(t[2]));
        if (b[0] - a[0]) * (c[1] - a[1]) - (b[1] - a[1]) * (c[0] - a[0]) < 0.0 {
            t.swap(1, 2);
        }
    }
    if roof.cover.is_empty() {
        return Err(ModelError::Invalid("the roof came out empty".into()));
    }
    Ok(roof)
}

/// A pitched roof with a build-up: the same roof planes carried out past the walls by the
/// overhang (eaves) and verge (gables), given a thickness with fascia, soffit and verge boards,
/// and gutters along the eaves. The gable triangles stay in the gable walls.
pub fn pitched_with(outline: &[[f64; 2]], eaves: f64, pitch_deg: f64, gable_edges: &[usize], b: &Buildup) -> Result<PitchedRoof> {
    pitched_with_caps(outline, eaves, pitch_deg, gable_edges, b, &[])
}

pub(crate) fn pitched_with_caps(
    outline: &[[f64; 2]],
    eaves: f64,
    pitch_deg: f64,
    gable_edges: &[usize],
    b: &Buildup,
    caps: &[(usize, f64)],
) -> Result<PitchedRoof> {
    let mut base = pitched(outline, eaves, pitch_deg, gable_edges)?;
    let planes = halfhip::planes(outline, eaves, pitch_deg, gable_edges, b, caps, &base)?;
    let overhang = crate::checked("roof overhang", b.overhang, 0.0, 2000.0)?;
    let verge = crate::checked("roof verge", b.verge, 0.0, 1500.0)?;
    let thickness = crate::checked("roof thickness", b.thickness, 0.0, 1000.0)?;
    if overhang == 0.0 && verge == 0.0 && thickness == 0.0 && !b.gutters {
        return Ok(base);
    }
    let rise = pitch_deg.to_radians().tan();
    // The roof planes pass through the wall line at the eaves: moving an eave edge out by the
    // overhang and down by overhang × rise keeps every plane where it was.
    let n = outline.len();
    let out: Vec<[f64; 2]> = crate::build::inner_ring_by(outline, |e| if gable_edges.contains(&e) { -verge } else { -overhang });
    if out.len() != n {
        return Err(ModelError::Invalid("the roof outline could not be carried out past the walls".into()));
    }
    let low = eaves - overhang * rise;
    let mut under = pitched(&out, low, pitch_deg, gable_edges)?;
    for plane in &planes {
        if out.iter().any(|p| plane.height([p[0], p[1], low]) < low - 1e-6) {
            return Err(ModelError::Invalid("half-hip cap would lower an extended side-eave corner".into()));
        }
        under.cover = halfhip::lower(&under.cover, *plane)?;
        base.gables = halfhip::clip_gables(&base.gables, *plane)?;
    }
    if !planes.is_empty() {
        halfhip::remaining_ridge(&under.cover, &out, low, pitch_deg, gable_edges)?;
    }
    let lift = thickness / pitch_deg.to_radians().cos();
    let mut cover = Mesh::default();
    if thickness > 0.0 {
        // Top surface, soffit (facing down) and the boards round the edge (vertical).
        let mut top = under.cover.clone();
        for p in &mut top.positions {
            p[2] += lift;
        }
        let mut bottom = under.cover.clone();
        for t in &mut bottom.triangles {
            t.swap(1, 2);
        }
        cover.append(&top);
        cover.append(&bottom);
        for (a, b) in boundary(&under.cover) {
            cover.quad([a[0], a[1], a[2]], [b[0], b[1], b[2]], [b[0], b[1], b[2] + lift], [a[0], a[1], a[2] + lift]);
        }
    } else {
        cover = under.cover.clone();
    }
    let mut gutters = Mesh::default();
    if b.gutters {
        for k in (0..n).filter(|k| !gable_edges.contains(k)) {
            let (Some(&p), Some(&q)) = (out.get(k), out.get((k + 1) % n)) else { continue };
            let Some(fr) = crate::build::FacadeFrame::new(p, q) else { continue };
            let (w, d) = GUTTER;
            // Counter-clockwise seen from above: outside the fascia first, then back along it.
            let ring = |z: f64| [fr.point(0.0, -w, z), fr.point(fr.length, -w, z), fr.point(fr.length, 0.0, z), fr.point(0.0, 0.0, z)];
            let (lo, hi) = (ring(low - d / 2.0), ring(low + d / 2.0));
            gutters.hexahedron(lo, hi);
        }
    }
    if !planes.is_empty() {
        halfhip::closed(&cover)?;
    }
    let ridge = cover.positions.iter().map(|p| p[2]).fold(eaves, f64::max);
    Ok(PitchedRoof { cover, gutters, gables: base.gables, ridge, max_depth: base.max_depth })
}

/// The edges of a triangle mesh that belong to one triangle only, once vertices at the same
/// place are taken as one, oriented as in their triangle.
fn boundary(m: &Mesh) -> Vec<([f64; 3], [f64; 3])> {
    use std::collections::HashMap;
    let key = |p: [f64; 3]| ((p[0] * 10.0).round() as i64, (p[1] * 10.0).round() as i64, (p[2] * 10.0).round() as i64);
    let mut ids: HashMap<(i64, i64, i64), usize> = HashMap::new();
    let mut at: Vec<[f64; 3]> = Vec::new();
    let mut id = |p: [f64; 3]| {
        *ids.entry(key(p)).or_insert_with(|| {
            at.push(p);
            at.len() - 1
        })
    };
    let welded: Vec<[usize; 3]> = m.triangles.iter().map(|t| t.map(|i| id(m.positions.get(i as usize).copied().unwrap_or_default()))).collect();
    let mut count: HashMap<(usize, usize), usize> = HashMap::new();
    for t in &welded {
        for k in 0..3 {
            let (a, b) = (t[k], t[(k + 1) % 3]);
            *count.entry((a.min(b), a.max(b))).or_default() += 1;
        }
    }
    let mut out = Vec::new();
    for t in &welded {
        for k in 0..3 {
            let (a, b) = (t[k], t[(k + 1) % 3]);
            // Triangles run counter-clockwise seen from above, so the outside of edge a→b is on
            // its right, where a quad a, b, b↑, a↑ faces.
            if a != b
                && count.get(&(a.min(b), a.max(b))) == Some(&1)
                && let (Some(&pa), Some(&pb)) = (at.get(a), at.get(b))
            {
                out.push((pa, pb));
            }
        }
    }
    out
}

fn push_triangulated(m: &mut Mesh, pts: &[[f64; 3]], flat: &[f64]) {
    let Ok(idx) = earcutr::earcut(flat, &[], 2) else { return };
    let base = m.positions.len() as u32;
    m.positions.extend_from_slice(pts);
    m.triangles.extend(idx.as_chunks::<3>().0.iter().map(|t| [base + t[0] as u32, base + t[1] as u32, base + t[2] as u32]));
}

#[cfg(test)]
mod tests {
    use super::*;

    const WING: [[f64; 2]; 4] = [[0.0, 0.0], [30000.0, 0.0], [30000.0, 12000.0], [0.0, 12000.0]];

    #[test]
    fn half_hip_retains_the_original_sloping_ridge_on_a_tapered_wing() {
        let outline = [[0., 0.], [30000., 0.], [32000., 12010.], [-600., 12000.]];
        let caps = [(3, 1200.)];
        let result =
            pitched_with_caps(&outline, 9000., 45., &[1, 3], &Buildup { overhang: 180., verge: 135., thickness: 200., gutters: false }, &caps)
                .unwrap();
        assert!(!result.cover.is_empty());
        let before = pitched_with(&outline, 9000., 45., &[1, 3], &Buildup { overhang: 180., verge: 135., thickness: 200., gutters: false }).unwrap();
        assert!(result.ridge <= before.ridge + 1e-6);
        assert!(result.ridge > 14900.);
    }

    #[test]
    fn half_hipped_source_clips_the_gable_and_preserves_eaves_and_ridge() {
        for rotation in [0.0_f64, 31.0, -118.0] {
            let (sin, cos) = rotation.to_radians().sin_cos();
            let outline = WING.map(|[x, y]| [142000. + x * cos - y * sin, -270000. + x * sin + y * cos]);
            let mut doc = crate::Document::default();
            let mut building = crate::Building::new("b", "Public clipped gable", &outline, 3, 3000.).unwrap();
            let mut source = serde_json::json!({"kind":"pitched","pitch_deg":45.,"gables":["b.f2","b.f4"],"straighten_mm":0.,"overhang_mm":180.,"verge_mm":135.,"thickness_mm":200.,"gutters":true});
            building.roof = serde_json::from_value(source.clone()).unwrap();
            doc.buildings.push(building);
            let baseline = crate::build(&doc).unwrap();
            source["kind"] = serde_json::json!("half_hipped");
            source["half_hips"] = serde_json::json!([{"facade":"b.f4","drop_mm":1200.}]);
            doc.buildings[0].roof = serde_json::from_value(source).unwrap();
            let built = crate::build(&doc).unwrap();
            assert_eq!(built[0].walls, baseline[0].walls);
            let (roof, original) = (built[0].roof.as_ref().unwrap(), baseline[0].roof.as_ref().unwrap());
            assert_eq!(roof.eaves, original.eaves);
            assert!((roof.ridge - original.ridge).abs() < 1e-6);
            assert!(!roof.compound, "a clipped gable is not a mansard");
            assert_eq!(roof.gutters, original.gutters);
            let frame = crate::build::FacadeFrame::new(outline[3], outline[0]).unwrap();
            let gable_peak = roof
                .gables
                .positions
                .iter()
                .filter(|p| ((p[0] - frame.origin[0]) * frame.n[0] + (p[1] - frame.origin[1]) * frame.n[1]).abs() < 1e-5)
                .map(|p| p[2])
                .fold(f64::NEG_INFINITY, f64::max);
            assert!((gable_peak - 13800.).abs() < 1e-5, "{gable_peak}");
            let mut edges = std::collections::BTreeMap::<_, (u32, i32)>::new();
            for t in &roof.cover.triangles {
                let keys = t.map(|i| roof.cover.positions[i as usize].map(|v| (v * 1e6).round() as i64));
                for k in 0..3 {
                    let (a, b) = (keys[k], keys[(k + 1) % 3]);
                    let e = edges.entry((a.min(b), a.max(b))).or_default();
                    e.0 += 1;
                    e.1 += if a < b { 1 } else { -1 };
                }
            }
            assert!(
                edges.values().all(|v| *v == (2, 0)),
                "half-hip shell has open, doubled or inward seams: {:?}",
                edges.iter().find(|(_, v)| **v != (2, 0))
            );
            let roundtrip: crate::Document = serde_json::from_slice(&serde_json::to_vec(&doc).unwrap()).unwrap();
            assert_eq!(doc, roundtrip);
        }
    }

    #[test]
    fn hipped_roof_on_a_wing() {
        let r = pitched(&WING, 16000.0, 40.0, &[]).unwrap();
        // Half the 12 m depth at 40°: 6000 × tan 40° above the eaves.
        let expect = 16000.0 + 6000.0 * 40f64.to_radians().tan();
        assert!((r.ridge - expect).abs() < 1.0, "{}", r.ridge);
        assert!((r.max_depth - 6000.0).abs() < 1e-6);
        assert!(r.gables.is_empty());
    }

    #[test]
    fn gable_ends_run_the_ridge_to_the_end_walls() {
        // Gables on both short ends (edges 1 and 3).
        let r = pitched(&WING, 16000.0, 40.0, &[1, 3]).unwrap();
        assert!(!r.gables.is_empty());
        // The ridge now reaches x = 0 and x = 30000.
        let ridge = r.ridge;
        let at_ends: Vec<f64> = r.cover.positions.iter().filter(|p| (p[2] - ridge).abs() < 1.0).map(|p| p[0]).collect();
        assert!(at_ends.iter().any(|x| x.abs() < 1.0) && at_ends.iter().any(|x| (x - 30000.0).abs() < 1.0), "{at_ends:?}");
        // The gable triangles stand in the end walls' planes.
        assert!(r.gables.positions.iter().all(|p| p[0].abs() < 1.0 || (p[0] - 30000.0).abs() < 1.0));
    }

    #[test]
    fn skew_gable_ends_preserve_the_adjoining_roof_planes() {
        // The two long eaves stay parallel; the gables are deliberately oblique.
        // Moving a hip node perpendicular to a gable while retaining its old height
        // takes it out of the adjoining roof planes and creates artificial creases.
        let outline = [[0., 0.], [30000., 0.], [32000., 9000.], [-600., 9000.]];
        let eaves = 10000.;
        let pitch: f64 = 46.;
        let roof = pitched(&outline, eaves, pitch, &[1, 3]).unwrap();
        for p in &roof.cover.positions {
            let expected = eaves + p[1].min(9000. - p[1]) * pitch.to_radians().tan();
            assert!((p[2] - expected).abs() < 1e-6, "skew gable warped the roof at {p:?}: expected {expected}");
        }
        assert!((roof.ridge - (eaves + 4500. * pitch.to_radians().tan())).abs() < 1e-6);
    }

    #[test]
    fn skew_gable_buildup_preserves_pitch_after_rotation_and_translation() {
        let source = [[0., 0.], [30000., 0.], [32000., 9000.], [-600., 9000.]];
        let (sin, cos) = 31_f64.to_radians().sin_cos();
        let outline = source.map(|[x, y]| [14000. + x * cos - y * sin, -7000. + x * sin + y * cos]);
        let eaves = 10000.;
        let pitch = 46_f64.to_radians();
        let buildup = Buildup { overhang: 180., verge: 80., thickness: 200., gutters: true };
        let roof = pitched_with(&outline, eaves, pitch.to_degrees(), &[1, 3], &buildup).unwrap();
        let lift = buildup.thickness / pitch.cos();
        for p in &roof.cover.positions {
            let y = -(p[0] - 14000.) * sin + (p[1] + 7000.) * cos;
            let soffit = eaves + y.min(9000. - y) * pitch.tan();
            assert!((p[2] - soffit).abs() < 1e-5 || (p[2] - soffit - lift).abs() < 1e-5, "built-up roof warped at {p:?}");
        }
        assert!((roof.ridge - (eaves + 4500. * pitch.tan() + lift)).abs() < 1e-5);
        assert_eq!(roof, pitched_with(&outline, eaves, pitch.to_degrees(), &[3, 1], &buildup).unwrap(), "gable input order cannot change geometry");
    }

    #[test]
    fn a_built_up_roof_keeps_its_planes_and_overhangs_the_walls() {
        let b = Buildup { overhang: 400.0, verge: 150.0, thickness: 250.0, gutters: true };
        let plain = pitched(&WING, 16000.0, 40.0, &[1, 3]).unwrap();
        let r = pitched_with(&WING, 16000.0, 40.0, &[1, 3], &b).unwrap();
        let (rise, lift) = (40f64.to_radians().tan(), 250.0 / 40f64.to_radians().cos());
        // The top surface is the plain roof lifted square to the roof by the build-up.
        assert!((r.ridge - (plain.ridge + lift)).abs() < 1.0, "{} vs {}", r.ridge, plain.ridge + lift);
        // It reaches 400 mm past the eaves walls (y = −400) at 400 × tan 40° below the eaves, and 150 past the gables.
        let (lo, hi) = r.cover.bounds().unwrap();
        assert!((lo[1] + 400.0).abs() < 1.0 && (hi[1] - 12400.0).abs() < 1.0, "{lo:?} {hi:?}");
        assert!((lo[0] + 150.0).abs() < 1.0 && (hi[0] - 30150.0).abs() < 1.0, "{lo:?} {hi:?}");
        assert!((lo[2] - (16000.0 - 400.0 * rise)).abs() < 1.0, "{lo:?}");
        // A closed slab: soffit and boards as well as the surface; gable triangles stay in the walls.
        assert!(r.cover.triangles.len() > 2 * plain.cover.triangles.len());
        assert!(r.gables.positions.iter().all(|p| p[0].abs() < 1.0 || (p[0] - 30000.0).abs() < 1.0));
        // Gutters along the two eaves.
        let (glo, ghi) = r.gutters.bounds().unwrap();
        assert!(glo[1] < -400.0 && ghi[1] > 12400.0);
        // Nothing asked: the plain roof.
        assert_eq!(pitched_with(&WING, 16000.0, 40.0, &[1, 3], &Buildup::default()).unwrap(), plain);
        assert!(pitched_with(&WING, 16000.0, 40.0, &[], &Buildup { overhang: f64::NAN, ..b }).is_err());
    }

    #[test]
    fn selected_eaves_validate_order_coverage_and_hostile_sources_without_changing_defaults() {
        let ring = [[0., 0.], [4000., 0.], [4000., 800.], [10000., 800.], [10000., 0.], [24000., 0.], [24000., 10000.], [0., 10000.]];
        let b = crate::Building::new("b", "Synthetic line selection", &ring, 2, 3400.).unwrap();
        let ids = |numbers: &[usize]| numbers.iter().map(|i| format!("b.f{i}")).collect::<Vec<_>>();
        let (owners, outline) = selected_eave_outline(&b, &ids(&[1, 6, 7, 8])).unwrap();
        assert_eq!(owners, vec![0, 5, 6, 7]);
        assert_eq!(outline, vec![[0., 0.], [24000., 0.], [24000., 10000.], [0., 10000.]]);
        let (rotated, points) = selected_eave_outline(&b, &ids(&[7, 8, 1, 6])).unwrap();
        assert_eq!(rotated, vec![6, 7, 0, 5]);
        assert_eq!(points, vec![outline[2], outline[3], outline[0], outline[1]]);
        for bad in [
            vec![],
            ids(&[1, 6]),
            ids(&[1, 6, 6, 8]),
            ids(&[1, 7, 6, 8]),
            ids(&[1, 3, 6, 7, 8]),
            ids(&[3, 6, 7, 8]),
            ids(&[1, 6, 7, 99]),
            vec!["x".repeat(129); 3],
            vec!["\n".into(); 3],
            vec!["b.f1".into(); 257],
        ] {
            assert!(selected_eave_outline(&b, &bad).is_err(), "must reject {bad:?}");
        }
        for value in [f64::NAN, f64::INFINITY, f64::NEG_INFINITY, f64::MAX, 1e8 + 1.] {
            let mut hostile = b.clone();
            hostile.footprint[0][0] = value;
            assert!(selected_eave_outline(&hostile, &ids(&[1, 6, 7, 8])).is_err());
        }
        let mut unstable = b.clone();
        unstable.footprint = vec![[0., 0.], [30000., 0.], [35000., 10000.], [30000., 10000.1], [0., 11000.]];
        unstable.facades.truncate(5);
        let error = selected_eave_outline(&unstable, &ids(&[1, 3, 4, 5])).unwrap_err().to_string();
        assert!(error.contains("unbounded"), "{error}");
        unstable.footprint[3][1] = 10000.0001;
        let error = selected_eave_outline(&unstable, &ids(&[1, 3, 4, 5])).unwrap_err().to_string();
        assert!(error.contains("unstable"), "{error}");
        let mut hostile = b.clone();
        hostile.facades[0].edge = usize::MAX;
        assert!(selected_eave_outline(&hostile, &ids(&[1, 6, 7, 8])).is_err());
        let mut hostile = b.clone();
        hostile.facades.push(hostile.facades[0].clone());
        assert!(selected_eave_outline(&hostile, &ids(&[1, 6, 7, 8])).is_err());
        let legacy = r#"{"kind":"pitched","pitch_deg":46.0,"gables":[],"straighten_mm":1200.0}"#;
        let roof: Roof = serde_json::from_str(legacy).unwrap();
        assert_eq!(serde_json::to_string(&roof).unwrap(), legacy);
        let mut null = serde_json::to_value(&roof).unwrap();
        null["eave_facades"] = serde_json::Value::Null;
        assert_eq!(serde_json::to_string(&serde_json::from_value::<Roof>(null).unwrap()).unwrap(), legacy);
        for value in [serde_json::json!(4), serde_json::json!([1, 2, 3]), serde_json::json!({}), serde_json::json!("b.f1")] {
            let mut bad = serde_json::to_value(&roof).unwrap();
            bad["eave_facades"] = value;
            assert!(serde_json::from_value::<Roof>(bad).is_err());
        }
    }

    #[test]
    fn eave_coverage_checks_whole_source_edges_across_concavities() {
        let u = [[0., 0.], [10000., 0.], [10000., 10000.], [7000., 10000.], [7000., 3000.], [3000., 3000.], [3000., 10000.], [0., 10000.]];
        // All three source vertices are inside, but the first edge crosses the open notch.
        let source = [[1000., 9000.], [9000., 9000.], [5000., 1000.]];
        assert_eq!(uncovered_edge(&u, &source), Some(0));
        assert_eq!(uncovered_edge(&u, &u), None);
    }

    #[test]
    fn refuses_silly_pitches() {
        assert!(pitched(&WING, 0.0, 89.0, &[]).is_err());
        assert!(pitched(&WING, 0.0, f64::NAN, &[]).is_err());
    }
}

#[cfg(test)]
mod straighten_tests {
    use super::*;

    #[test]
    fn sawtooth_street_facade_gets_a_straight_eave() {
        // A 30 × 12 m wing whose street side (y = 12000) steps in and out by 800 mm every 6 m.
        let mut ring = vec![[0.0, 0.0], [30000.0, 0.0]];
        for k in 0..5 {
            let (x1, x0) = (30000.0 - k as f64 * 6000.0, 24000.0 - k as f64 * 6000.0);
            let y = if k % 2 == 0 { 12000.0 } else { 11200.0 };
            ring.push([x1, y]);
            ring.push([x0, y]);
        }
        let ring = crate::footprint::clean(&ring).unwrap();
        let kept = eave_outline(&ring, 1200.0);
        assert_eq!(kept.len(), 4, "a plain rectangle under the roof: {:?}", kept.iter().map(|&i| ring[i]).collect::<Vec<_>>());
        let outline: Vec<[f64; 2]> = kept.iter().map(|&i| ring[i]).collect();
        // Never inside the walls: every footprint corner is inside or on the eave outline.
        let r = pitched(&outline, 16000.0, 40.0, &[]).unwrap();
        assert!((r.max_depth - 6000.0).abs() < 1.0);
        // With straightening off, every step keeps its corners.
        assert_eq!(eave_outline(&ring, 0.0).len(), ring.len());
    }
}
