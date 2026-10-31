//! Roofs. A pitched roof is the straight skeleton of the outline lifted at one pitch: every wall
//! edge carries one roof face rising inward from the eaves. Gable edges (where a wing meets a
//! neighbour, or a gable end) keep their wall: the skeleton nodes of their face are slid onto the
//! edge, so the ridge runs out to the gable and the face turns into a vertical gable triangle.

use geo_straight_skeleton::Skeleton;
use serde::{Deserialize, Serialize};

use crate::mesh::Mesh;
use crate::{ModelError, Result};

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
    },
}

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

/// A pitched roof's geometry, in site mm.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct PitchedRoof {
    /// The roof surface (tiles, slates).
    pub cover: Mesh,
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
    let mut verts = sk.vertices.clone();
    let max_depth = verts.iter().map(|v| v[2]).fold(0.0, f64::max);
    for &e in gable_edges {
        let (Some(a), Some(b), Some(face)) = (outline.get(e), outline.get((e + 1) % n), sk.faces.get(e)) else {
            return Err(ModelError::Invalid(format!("gable edge {e} doesn't exist")));
        };
        let d = [b[0] - a[0], b[1] - a[1]];
        let l2 = d[0] * d[0] + d[1] * d[1];
        if !(l2 > 1.0) {
            continue;
        }
        for &vi in &face.vertices {
            if vi < n {
                continue;
            }
            if let Some(v) = verts.get_mut(vi) {
                let t = (((v[0] - a[0]) * d[0] + (v[1] - a[1]) * d[1]) / l2).clamp(0.0, 1.0);
                v[0] = a[0] + t * d[0];
                v[1] = a[1] + t * d[1];
            }
        }
    }
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
    if roof.cover.is_empty() {
        return Err(ModelError::Invalid("the roof came out empty".into()));
    }
    Ok(roof)
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
