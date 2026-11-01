//! Orthographic projection with hidden lines removed, for elevations and for what a section sees
//! beyond its cut.
//!
//! Only feature edges are drawn: where a surface ends or folds by more than 10°, never the
//! triangulation of a flat face. Each edge is clipped against every triangle in front of it (a
//! grid finds the candidates), so only its visible pieces remain and the line work is exact on its
//! own, in SVG and in DXF alike. Faces become light tonal fills, merged per flat face so no
//! triangle seams show; they are ordered back to front.

use std::collections::HashMap;

use trigon_model::mesh::Mesh;

use super::cut::{self, Ring, Shape};
use super::{Element, overlap};
use crate::views::{DrawingElementRegion, DrawingElementSegment};

/// How a view looks at the model: drawing x along `t`, drawing y up (model z), depth along `d`
/// (into the drawing, away from the viewer). `t` and `d` are horizontal unit vectors.
#[derive(Clone, Copy, Debug)]
pub(crate) struct Look {
    pub(crate) origin: [f64; 2],
    pub(crate) t: [f64; 2],
    pub(crate) d: [f64; 2],
}

impl Look {
    /// A model point as drawing `(x, y)` and depth.
    pub(crate) fn map(&self, p: [f64; 3]) -> [f64; 3] {
        let (dx, dy) = (p[0] - self.origin[0], p[1] - self.origin[1]);
        [dx * self.t[0] + dy * self.t[1], p[2], dx * self.d[0] + dy * self.d[1]]
    }
}

/// What a mesh is, for line weights, tones and layers.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub(crate) enum Class {
    Wall,
    Roof,
    Slab,
    Frame,
    Glass,
    Door,
    Balcony,
    Railing,
    Stair,
    Partition,
    /// The rendered or concrete base along the foot of the facade.
    Plinth,
}

#[derive(Clone)]
pub(crate) struct Source<'a> {
    pub(crate) mesh: &'a Mesh,
    pub(crate) class: Class,
    /// Meshes in one group are welded together before edges are classified (the walls of a
    /// building, so a joint between two facades in line shows no line).
    pub(crate) group: usize,
    pub(crate) element: Option<Element>,
}

/// A visible piece of a feature edge, in drawing mm.
#[derive(Clone, Debug)]
pub(crate) struct Edge {
    pub(crate) a: [f64; 2],
    pub(crate) b: [f64; 2],
    pub(crate) class: Class,
    /// Depth at its middle, mm.
    pub(crate) depth: f64,
    /// Endpoint depths, used to split primary/depth plan strokes at the exact range boundary.
    pub(crate) end_depths: [f64; 2],
    /// Against the background on one side: part of the outline.
    pub(crate) outline: bool,
    pub(crate) element_segments: Vec<DrawingElementSegment>,
}

/// A flat face seen from the front or behind: rings filled even-odd.
#[derive(Clone, Debug)]
pub(crate) struct Face {
    pub(crate) rings: Vec<Ring>,
    pub(crate) class: Class,
    pub(crate) depth: f64,
    pub(crate) element: Option<Element>,
    pub(crate) depth_plane: [f64; 3],
}

/// What happens at the near end of the depth range.
#[derive(Clone, Copy)]
pub(crate) enum Near<'a> {
    /// The near limit cuts the meshes: the cut is drawn as edges and its faces hide what is
    /// behind them.
    Cut,
    /// These shapes (a section's poche) lie at the near limit and hide what is behind them.
    Caps(&'a [Shape]),
}

#[derive(Debug, Default)]
pub(crate) struct Projection {
    pub(crate) edges: Vec<Edge>,
    /// Back to front.
    pub(crate) faces: Vec<Face>,
    /// Triangles that can hide something.
    pub(crate) triangles: usize,
    pub(crate) element_regions: Vec<DrawingElementRegion>,
}

/// Faces folding by less than this are one surface (cos 10°).
const FOLD: f64 = 0.984_807_753;
/// A triangle must be this much nearer than an edge to hide it, mm.
const DEPTH_EPS: f64 = 0.5;
/// Points this close to a triangle's edge count as inside it, mm.
const INSIDE_EPS: f64 = 0.01;
/// Shorter visible pieces are dropped, mm.
const MIN_PIECE: f64 = 0.05;
/// Coordinates beyond this (mm) are not a building.
const LIMIT: f64 = 1e8;

type P3 = [f64; 3];

fn sub(a: P3, b: P3) -> P3 {
    [a[0] - b[0], a[1] - b[1], a[2] - b[2]]
}

fn cross(a: P3, b: P3) -> P3 {
    [a[1] * b[2] - a[2] * b[1], a[2] * b[0] - a[0] * b[2], a[0] * b[1] - a[1] * b[0]]
}

fn unit(a: P3) -> Option<P3> {
    let l = (a[0] * a[0] + a[1] * a[1] + a[2] * a[2]).sqrt();
    (l > 1e-12 && l.is_finite()).then(|| [a[0] / l, a[1] / l, a[2] / l])
}

fn dot(a: P3, b: P3) -> f64 {
    a[0] * b[0] + a[1] * b[1] + a[2] * b[2]
}

/// Points within 0.05 mm are one vertex.
struct Weld {
    cells: HashMap<[i64; 3], Vec<u32>>,
    points: Vec<P3>,
}

impl Weld {
    const TOL: f64 = 0.05;

    fn new() -> Self {
        Weld { cells: HashMap::new(), points: Vec::new() }
    }

    fn id(&mut self, p: P3) -> u32 {
        let c = p.map(|v| (v / Self::TOL).floor() as i64);
        for dx in -1..=1 {
            for dy in -1..=1 {
                for dz in -1..=1 {
                    if let Some(ids) = self.cells.get(&[c[0] + dx, c[1] + dy, c[2] + dz]) {
                        for &i in ids {
                            let q = self.points[i as usize];
                            if (q[0] - p[0]).abs() <= Self::TOL && (q[1] - p[1]).abs() <= Self::TOL && (q[2] - p[2]).abs() <= Self::TOL {
                                return i;
                            }
                        }
                    }
                }
            }
        }
        let i = self.points.len() as u32;
        self.points.push(p);
        self.cells.entry(c).or_default().push(i);
        i
    }
}

/// The part of polygon `poly` (mapped points) with depth in `range`.
fn clip_depth(poly: &[P3], range: (f64, f64)) -> Vec<P3> {
    let mut out = poly.to_vec();
    for (bound, keep_above) in [(range.0, true), (range.1, false)] {
        let input = std::mem::take(&mut out);
        let n = input.len();
        let inside = |p: &P3| if keep_above { p[2] >= bound } else { p[2] <= bound };
        for i in 0..n {
            let (p, q) = (input[i], input[(i + 1) % n]);
            if inside(&p) {
                out.push(p);
            }
            if inside(&p) != inside(&q) {
                // From the lexicographically smaller end, so a shared edge clips identically.
                let (a, b) = if p < q { (p, q) } else { (q, p) };
                let s = (bound - a[2]) / (b[2] - a[2]);
                out.push([a[0] + (b[0] - a[0]) * s, a[1] + (b[1] - a[1]) * s, bound]);
            }
        }
    }
    out
}

/// A projected triangle that can hide things: its corners in drawing mm, and its depth as a plane
/// `depth = c + gx·x + gy·y`.
struct Occluder {
    /// Inward unit normals and offsets of its three edges: inside where `n·p - o >= 0`.
    edges: [([f64; 2], f64); 3],
    plane: [f64; 3],
    bbox: [f64; 4],
}

impl Occluder {
    fn new(p: [P3; 3]) -> Option<Self> {
        let e1 = [p[1][0] - p[0][0], p[1][1] - p[0][1]];
        let e2 = [p[2][0] - p[0][0], p[2][1] - p[0][1]];
        let det = e1[0] * e2[1] - e1[1] * e2[0];
        // Edge-on (or nearly) triangles hide nothing.
        let n3 = unit(cross(sub(p[1], p[0]), sub(p[2], p[0])))?;
        if det.abs() < 1e-6 || n3[2].abs() < 1e-4 {
            return None;
        }
        let (d1, d2) = (p[1][2] - p[0][2], p[2][2] - p[0][2]);
        let gx = (d1 * e2[1] - d2 * e1[1]) / det;
        let gy = (d2 * e1[0] - d1 * e2[0]) / det;
        let plane = [p[0][2] - gx * p[0][0] - gy * p[0][1], gx, gy];
        let q = [[p[0][0], p[0][1]], [p[1][0], p[1][1]], [p[2][0], p[2][1]]];
        let sign = det.signum();
        let mut edges = [([0.0; 2], 0.0); 3];
        for k in 0..3 {
            let (a, b) = (q[k], q[(k + 1) % 3]);
            let (ex, ey) = (b[0] - a[0], b[1] - a[1]);
            let len = ex.hypot(ey);
            if len < 1e-9 {
                return None;
            }
            // Left of a counter-clockwise edge is inside.
            let n = [-ey / len * sign, ex / len * sign];
            edges[k] = (n, n[0] * a[0] + n[1] * a[1]);
        }
        let bbox = [
            q[0][0].min(q[1][0]).min(q[2][0]),
            q[0][1].min(q[1][1]).min(q[2][1]),
            q[0][0].max(q[1][0]).max(q[2][0]),
            q[0][1].max(q[1][1]).max(q[2][1]),
        ];
        Some(Occluder { edges, plane, bbox })
    }

    fn depth(&self, p: [f64; 2]) -> f64 {
        self.plane[0] + self.plane[1] * p[0] + self.plane[2] * p[1]
    }

    fn contains(&self, p: [f64; 2]) -> bool {
        self.edges.iter().all(|(n, o)| n[0] * p[0] + n[1] * p[1] - o >= -INSIDE_EPS)
    }

    /// The part `t ∈ [lo, hi]` of segment `a + t(b − a)` (depths `da`, `db`) this triangle hides.
    fn hides(&self, a: [f64; 2], b: [f64; 2], da: f64, db: f64) -> Option<(f64, f64)> {
        let (mut lo, mut hi) = (0.0_f64, 1.0_f64);
        let mut keep = |f0: f64, f1: f64, at_least: f64| {
            if (f1 - f0).abs() < 1e-12 {
                if f0 < at_least {
                    hi = -1.0;
                }
                return;
            }
            let t = (at_least - f0) / (f1 - f0);
            if f1 > f0 {
                lo = lo.max(t);
            } else {
                hi = hi.min(t);
            }
        };
        for (n, o) in &self.edges {
            keep(n[0] * a[0] + n[1] * a[1] - o, n[0] * b[0] + n[1] * b[1] - o, -INSIDE_EPS);
        }
        // In front: the edge is deeper than the triangle by more than DEPTH_EPS.
        keep(da - self.depth(a), db - self.depth(b), DEPTH_EPS);
        (hi > lo).then_some((lo, hi))
    }

    /// Whether the triangle touches the box `[x0, y0, x1, y1]` (separating axes).
    fn touches(&self, r: [f64; 4]) -> bool {
        if self.bbox[0] > r[2] || self.bbox[2] < r[0] || self.bbox[1] > r[3] || self.bbox[3] < r[1] {
            return false;
        }
        let corners = [[r[0], r[1]], [r[2], r[1]], [r[2], r[3]], [r[0], r[3]]];
        self.edges.iter().all(|(n, o)| corners.iter().any(|c| n[0] * c[0] + n[1] * c[1] - o >= -INSIDE_EPS))
    }
}

/// Occluders bucketed by the cells they touch.
struct Grid {
    x0: f64,
    y0: f64,
    cell: f64,
    nx: usize,
    ny: usize,
    cells: Vec<Vec<u32>>,
}

impl Grid {
    fn new(occ: &[Occluder]) -> Self {
        let mut b = [f64::INFINITY, f64::INFINITY, f64::NEG_INFINITY, f64::NEG_INFINITY];
        for o in occ {
            b = [b[0].min(o.bbox[0]), b[1].min(o.bbox[1]), b[2].max(o.bbox[2]), b[3].max(o.bbox[3])];
        }
        if !b.iter().all(|v| v.is_finite()) {
            return Grid { x0: 0.0, y0: 0.0, cell: 1.0, nx: 0, ny: 0, cells: Vec::new() };
        }
        let extent = (b[2] - b[0]).max(b[3] - b[1]).max(1.0);
        let cell = (extent / 400.0).max(50.0);
        let nx = (((b[2] - b[0]) / cell).floor() as usize + 1).min(2000);
        let ny = (((b[3] - b[1]) / cell).floor() as usize + 1).min(2000);
        let mut g = Grid { x0: b[0], y0: b[1], cell, nx, ny, cells: vec![Vec::new(); nx * ny] };
        for (k, o) in occ.iter().enumerate() {
            let (i0, j0, i1, j1) = g.range(o.bbox);
            for j in j0..=j1 {
                for i in i0..=i1 {
                    let r = [g.x0 + i as f64 * cell, g.y0 + j as f64 * cell, g.x0 + (i + 1) as f64 * cell, g.y0 + (j + 1) as f64 * cell];
                    if o.touches(r) {
                        g.cells[j * nx + i].push(k as u32);
                    }
                }
            }
        }
        g
    }

    /// Cell index ranges covering `bbox`, clamped to the grid.
    fn range(&self, bbox: [f64; 4]) -> (usize, usize, usize, usize) {
        let ix = |x: f64| (((x - self.x0) / self.cell).floor().max(0.0) as usize).min(self.nx.saturating_sub(1));
        let iy = |y: f64| (((y - self.y0) / self.cell).floor().max(0.0) as usize).min(self.ny.saturating_sub(1));
        (ix(bbox[0]), iy(bbox[1]), ix(bbox[2]), iy(bbox[3]))
    }

    /// Occluders in the cells segment `a b` passes through, each once.
    fn along(&self, a: [f64; 2], b: [f64; 2], stamp: &mut [u32], round: u32, out: &mut Vec<u32>) {
        out.clear();
        if self.cells.is_empty() {
            return;
        }
        let (i0, j0, i1, j1) = self.range([a[0].min(b[0]), a[1].min(b[1]), a[0].max(b[0]), a[1].max(b[1])]);
        for j in j0..=j1 {
            for i in i0..=i1 {
                let r = [
                    self.x0 + i as f64 * self.cell,
                    self.y0 + j as f64 * self.cell,
                    self.x0 + (i + 1) as f64 * self.cell,
                    self.y0 + (j + 1) as f64 * self.cell,
                ];
                let pad = [r[0] - 1.0, r[1] - 1.0, r[2] + 1.0, r[3] + 1.0];
                if super::clip_segment(a, b, pad).is_none() {
                    continue;
                }
                for &k in &self.cells[j * self.nx + i] {
                    if let Some(s) = stamp.get_mut(k as usize)
                        && *s != round
                    {
                        *s = round;
                        out.push(k);
                    }
                }
            }
        }
    }

    fn at(&self, p: [f64; 2]) -> &[u32] {
        let (i, j, _, _) = self.range([p[0], p[1], p[0], p[1]]);
        let inside =
            p[0] >= self.x0 && p[1] >= self.y0 && p[0] <= self.x0 + self.nx as f64 * self.cell && p[1] <= self.y0 + self.ny as f64 * self.cell;
        if !inside { &[] } else { self.cells.get(j * self.nx + i).map_or(&[], Vec::as_slice) }
    }
}

/// Feature edges of a welded group: folds over 10°, non-manifold edges and boundaries. A boundary
/// edge only counts where no face in the same plane continues on its other side: zero-area
/// triangles (earcut leaves them where hole corners line up) and T-junctions between coplanar faces
/// don't show.
fn feature_edges(tris: &[([u32; 3], Option<P3>, Class)], points: &[P3]) -> Vec<(P3, P3, Class)> {
    let mut faces: HashMap<(u32, u32), Vec<(usize, u32)>> = HashMap::new();
    for (i, (t, n, _)) in tris.iter().enumerate() {
        if n.is_none() || t[0] == t[1] || t[1] == t[2] || t[0] == t[2] {
            continue;
        }
        for k in 0..3 {
            let (a, b) = (t[k], t[(k + 1) % 3]);
            faces.entry((a.min(b), a.max(b))).or_default().push((i, t[(k + 2) % 3]));
        }
    }
    let point = |i: u32| points.get(i as usize).copied();
    let mut out = Vec::new();
    // Boundary candidates by the line they lie on: (along-line interval, normal, side into the face, class).
    let mut lines: HashMap<[i64; 6], (P3, P3, Vec<(f64, f64, P3, P3, Class)>)> = HashMap::new();
    let mut keys = Vec::new();
    for ((a, b), f) in &faces {
        let (Some(pa), Some(pb)) = (point(*a), point(*b)) else { continue };
        let mut normals: Vec<P3> = f.iter().filter_map(|(i, _)| tris.get(*i)?.1).collect();
        let class = f.first().and_then(|(i, _)| tris.get(*i)).map_or(Class::Wall, |t| t.2);
        if f.len() == 1 {
            let (Some(u), Some(n), Some(c)) = (unit(sub(pb, pa)), normals.first().copied(), f.first().and_then(|(_, c)| point(*c))) else { continue };
            // One direction per line, whichever way the edge runs.
            let u = if u[0] < -1e-9 || (u[0].abs() <= 1e-9 && (u[1] < -1e-9 || (u[1].abs() <= 1e-9 && u[2] < 0.0))) { u.map(|v| -v) } else { u };
            let mut side = cross(n, u);
            if dot(side, sub(c, pa)) < 0.0 {
                side = side.map(|v| -v);
            }
            let foot = sub(pa, u.map(|v| v * dot(pa, u)));
            let key = [u[0], u[1], u[2]].map(|v| (v * 1e5).round() as i64);
            let key = [key[0], key[1], key[2], (foot[0] / 0.05).round() as i64, (foot[1] / 0.05).round() as i64, (foot[2] / 0.05).round() as i64];
            let (s0, s1) = (dot(pa, u), dot(pb, u));
            let entry = lines.entry(key).or_insert_with(|| {
                keys.push(key);
                (u, foot, Vec::new())
            });
            entry.2.push((s0.min(s1), s0.max(s1), n, side, class));
            continue;
        }
        if f.len() >= 3 {
            // Where solids touch face to face (wall segments side by side or stacked), the touching
            // faces point at each other: they are inside, not surface.
            let mut k = 0;
            while k < normals.len() {
                match (k + 1..normals.len()).find(|&j| dot(normals[k], normals[j]) < -FOLD) {
                    Some(j) => {
                        normals.remove(j);
                        normals.remove(k);
                    }
                    None => k += 1,
                }
            }
        }
        let feature = match normals.len() {
            0 => false,
            1 => true,
            _ => normals.split_first().is_some_and(|(n0, rest)| rest.iter().any(|n| dot(*n0, *n).abs() < FOLD)),
        };
        if feature {
            out.push((pa, pb, class));
        }
    }
    keys.sort_unstable();
    for key in keys {
        let Some((u, foot, pieces)) = lines.get(&key) else { continue };
        let at = |s: f64| [foot[0] + u[0] * s, foot[1] + u[1] * s, foot[2] + u[2] * s];
        let mut cuts: Vec<f64> = pieces.iter().flat_map(|p| [p.0, p.1]).collect();
        cuts.sort_by(f64::total_cmp);
        cuts.dedup_by(|x, y| (*x - *y).abs() < 1e-6);
        let mut run: Option<(f64, f64, Class)> = None;
        for w in cuts.windows(2) {
            let m = (w[0] + w[1]) / 2.0;
            let here: Vec<&(f64, f64, P3, P3, Class)> = pieces.iter().filter(|p| p.0 <= m && m <= p.1).collect();
            // Inside the surface where coplanar faces meet here from both sides.
            let inside = here.iter().enumerate().any(|(i, p)| here[i + 1..].iter().any(|q| dot(p.2, q.2).abs() > FOLD && dot(p.3, q.3) < 0.0));
            let edge = if here.is_empty() || inside { None } else { here.first().map(|p| p.4) };
            match (edge, run) {
                (Some(c), Some((s, _, rc))) if c == rc => run = Some((s, w[1], rc)),
                (edge, prev) => {
                    if let Some((s, e, c)) = prev {
                        out.push((at(s), at(e), c));
                    }
                    run = edge.map(|c| (w[0], w[1], c));
                }
            }
        }
        if let Some((s, e, c)) = run {
            out.push((at(s), at(e), c));
        }
    }
    out
}

/// Flat caps at depth `depth` as occluders (triangulated shapes with their holes).
fn caps(shapes: &[Shape], depth: f64, occ: &mut Vec<Occluder>) {
    for shape in shapes {
        let mut flat = Vec::new();
        let mut holes = Vec::new();
        let mut all: Vec<[f64; 2]> = Vec::new();
        for (k, ring) in shape.iter().enumerate() {
            if k > 0 {
                holes.push(all.len());
            }
            all.extend(ring);
            flat.extend(ring.iter().flat_map(|p| [p[0], p[1]]));
        }
        let Ok(idx) = earcutr::earcut(&flat, &holes, 2) else { continue };
        for t in idx.as_chunks::<3>().0 {
            let (Some(a), Some(b), Some(c)) = (all.get(t[0]), all.get(t[1]), all.get(t[2])) else { continue };
            if let Some(o) = Occluder::new([[a[0], a[1], depth], [b[0], b[1], depth], [c[0], c[1], depth]]) {
                occ.push(o);
            }
        }
    }
}

/// Projects `sources` seen through `look`, keeping depths in `range`; `near` says what happens
/// at the near limit.
pub(crate) fn project(sources: &[Source<'_>], look: &Look, range: (f64, f64), near: Near<'_>) -> Projection {
    let mut groups: Vec<usize> = sources.iter().map(|s| s.group).collect();
    groups.sort_unstable();
    groups.dedup();
    let mut occ: Vec<Occluder> = Vec::new();
    let mut lines: Vec<(P3, P3, Class, Vec<DrawingElementSegment>)> = Vec::new();
    // Clipped triangles per (group, class) for the fills: welded corners and the 3D normal.
    let mut fill_tris: Vec<(usize, Class, [P3; 3], P3)> = Vec::new();
    let mut cap_faces: Vec<Face> = Vec::new();
    let mut element_regions = Vec::new();
    if let Near::Caps(shapes) = near {
        caps(shapes, range.0, &mut occ);
    }
    for g in groups {
        let mut near_segs = Vec::new();
        let mut owned_edges: Vec<(usize, [P3; 2])> = Vec::new();
        let mut weld = Weld::new();
        let mut tris: Vec<([u32; 3], Option<P3>, Class)> = Vec::new();
        for (si, s) in sources.iter().enumerate().filter(|(_, s)| s.group == g) {
            let pts: Vec<P3> = s.mesh.positions.iter().map(|p| look.map(*p)).collect();
            for t in &s.mesh.triangles {
                let Some(p) = t.iter().map(|&i| pts.get(i as usize).copied()).collect::<Option<Vec<_>>>() else { continue };
                if p.iter().flatten().any(|v| !v.is_finite() || v.abs() > LIMIT) {
                    continue;
                }
                let (p0, p1, p2) = (p[0], p[1], p[2]);
                if s.element.is_some() {
                    owned_edges.extend([[p0, p1], [p1, p2], [p2, p0]].map(|segment| (si, segment)));
                }
                let n = unit(cross(sub(p1, p0), sub(p2, p0)));
                tris.push(([weld.id(p0), weld.id(p1), weld.id(p2)], n, s.class));
                let Some(n) = n else { continue };
                let clipped = clip_depth(&[p0, p1, p2], range);
                for k in 1..clipped.len().saturating_sub(1) {
                    let tri = [clipped[0], clipped[k], clipped[k + 1]];
                    if let Some(o) = Occluder::new(tri) {
                        occ.push(o);
                        fill_tris.push((si, s.class, tri, n));
                    }
                }
            }
            if matches!(near, Near::Cut) {
                let at = range.0;
                let start = near_segs.len();
                cut::segments(
                    s.mesh,
                    &|p| {
                        let m = look.map(p);
                        [m[0], m[1], m[2] - at]
                    },
                    &mut near_segs,
                );
                if s.element.is_some() {
                    owned_edges.extend(near_segs[start..].iter().map(|segment| (si, segment.map(|point| [point[0], point[1], at]))));
                    let rings = cut::chain(&near_segs[start..], 0.5, 5.0).closed;
                    if let Some(element) = &s.element
                        && !rings.is_empty()
                    {
                        element_regions.push(DrawingElementRegion {
                            id: element.id.clone(),
                            kind: element.kind,
                            rings,
                            depth_plane: Some([at, 0.0, 0.0]),
                        });
                    }
                }
            }
        }
        let attribution = |start: P3, end: P3| {
            owned_edges
                .iter()
                .filter_map(|(index, source)| {
                    let element = sources.get(*index)?.element.as_ref()?;
                    overlap([start, end], *source, 0.1).map(|segment| element.segment(segment.map(|point| [point[0], point[1]])))
                })
                .collect::<Vec<_>>()
        };
        lines.extend(feature_edges(&tris, &weld.points).into_iter().map(|(start, end, class)| (start, end, class, attribution(start, end))));
        if !near_segs.is_empty() {
            // The cut through this group: its outline is drawn, its faces hide and are toned.
            let class = sources.iter().find(|s| s.group == g).map_or(Class::Wall, |s| s.class);
            let chains = cut::chain(&near_segs, 0.5, 5.0);
            let shapes = cut::even_odd(&chains.closed);
            let at = range.0;
            let rings = shapes.iter().flatten().map(|r| (r, true)).chain(chains.open.iter().map(|r| (r, false)));
            for (ring, closed) in rings {
                let n = ring.len();
                for k in 0..if closed { n } else { n.saturating_sub(1) } {
                    let (a, b) = (ring[k], ring[(k + 1) % n]);
                    let (start, end) = ([a[0], a[1], at], [b[0], b[1], at]);
                    lines.push((start, end, class, attribution(start, end)));
                }
            }
            caps(&shapes, at, &mut occ);
            cap_faces.extend(shapes.into_iter().map(|rings| Face { rings, class, depth: at, element: None, depth_plane: [at, 0.0, 0.0] }));
        }
    }
    let grid = Grid::new(&occ);
    let mut stamp = vec![u32::MAX; occ.len()];
    let mut candidates = Vec::new();
    let mut round = 0_u32;
    let covered = |p: [f64; 2]| grid.at(p).iter().any(|&k| occ.get(k as usize).is_some_and(|o| o.contains(p)));
    let mut edges = Vec::new();
    for (a, b, class, elements) in lines {
        // Clip the edge to the depth range.
        let (mut t0, mut t1) = (0.0_f64, 1.0_f64);
        let dd = b[2] - a[2];
        for (bound, above) in [(range.0, true), (range.1, false)] {
            if dd.abs() < 1e-12 {
                if (above && a[2] < bound - 1e-9) || (!above && a[2] > bound + 1e-9) {
                    t1 = -1.0;
                }
            } else {
                let t = (bound - a[2]) / dd;
                if (dd > 0.0) == above {
                    t0 = t0.max(t);
                } else {
                    t1 = t1.min(t);
                }
            }
        }
        if t1 <= t0 {
            continue;
        }
        let at = |t: f64| [a[0] + (b[0] - a[0]) * t, a[1] + (b[1] - a[1]) * t, a[2] + dd * t];
        let (pa, pb) = (at(t0), at(t1));
        let (a2, b2) = ([pa[0], pa[1]], [pb[0], pb[1]]);
        let len = (b2[0] - a2[0]).hypot(b2[1] - a2[1]);
        if len < MIN_PIECE {
            continue;
        }
        round = round.wrapping_add(1);
        grid.along(a2, b2, &mut stamp, round, &mut candidates);
        let mut hidden: Vec<(f64, f64)> = candidates.iter().filter_map(|&k| occ.get(k as usize)?.hides(a2, b2, pa[2], pb[2])).collect();
        hidden.sort_by(|x, y| x.0.total_cmp(&y.0));
        let mut visible = Vec::new();
        let mut from = 0.0_f64;
        for (lo, hi) in hidden {
            if lo > from {
                visible.push((from, lo));
            }
            from = from.max(hi);
        }
        if from < 1.0 {
            visible.push((from, 1.0));
        }
        let normal = [-(b2[1] - a2[1]) / len, (b2[0] - a2[0]) / len];
        for (s, e) in visible {
            if (e - s) * len < MIN_PIECE {
                continue;
            }
            let p = |t: f64| [a2[0] + (b2[0] - a2[0]) * t, a2[1] + (b2[1] - a2[1]) * t];
            let m = p((s + e) / 2.0);
            let side = |k: f64| [m[0] + normal[0] * 2.0 * k, m[1] + normal[1] * 2.0 * k];
            let outline = !covered(side(1.0)) || !covered(side(-1.0));
            let element_segments = elements
                .iter()
                .filter_map(|element| overlap([p(s), p(e)], element.segment, 0.1).map(|segment| DrawingElementSegment { segment, ..element.clone() }))
                .collect();
            edges.push(Edge {
                a: p(s),
                b: p(e),
                class,
                depth: pa[2] + (pb[2] - pa[2]) * (s + e) / 2.0,
                end_depths: [pa[2] + (pb[2] - pa[2]) * s, pa[2] + (pb[2] - pa[2]) * e],
                outline,
                element_segments,
            });
        }
    }
    let mut faces = faces(&fill_tris, sources);
    // Caps are nearest of all.
    faces.extend(cap_faces);
    Projection { edges, faces, triangles: occ.len(), element_regions }
}

/// Merges coplanar neighbouring triangles of one source into faces, back to front.
fn faces(tris: &[(usize, Class, [P3; 3], P3)], sources: &[Source<'_>]) -> Vec<Face> {
    let mut by_source: HashMap<usize, Vec<usize>> = HashMap::new();
    for (k, t) in tris.iter().enumerate() {
        // Faces nearly edge-on carry no tone.
        if t.3[2].abs() > 0.02 {
            by_source.entry(t.0).or_default().push(k);
        }
    }
    let mut keys: Vec<usize> = by_source.keys().copied().collect();
    keys.sort_unstable();
    let mut out = Vec::new();
    for key in keys {
        let Some(list) = by_source.get(&key) else { continue };
        let Some(class) = sources.get(key).map(|s| s.class) else { continue };
        let mut weld = Weld::new();
        let ids: Vec<[u32; 3]> = list.iter().map(|&k| tris[k].2.map(|p| weld.id(p))).collect();
        // Union-find over shared edges between triangles in one plane.
        let mut parent: Vec<usize> = (0..list.len()).collect();
        fn root(parent: &mut [usize], mut i: usize) -> usize {
            while parent[i] != i {
                parent[i] = parent[parent[i]];
                i = parent[i];
            }
            i
        }
        let mut by_edge: HashMap<(u32, u32), Vec<usize>> = HashMap::new();
        for (i, t) in ids.iter().enumerate() {
            for k in 0..3 {
                let (a, b) = (t[k], t[(k + 1) % 3]);
                if a != b {
                    by_edge.entry((a.min(b), a.max(b))).or_default().push(i);
                }
            }
        }
        let plane = |i: usize| {
            let (n, p) = (tris[list[i]].3, tris[list[i]].2[0]);
            (n, dot(n, p))
        };
        for sharing in by_edge.values() {
            for w in sharing.windows(2) {
                let ((n0, o0), (n1, o1)) = (plane(w[0]), plane(w[1]));
                let same = dot(n0, n1);
                if same.abs() > 0.999_9 && (o0 - o1 * same.signum()).abs() < 1.0 {
                    let (r0, r1) = (root(&mut parent, w[0]), root(&mut parent, w[1]));
                    parent[r0] = r1;
                }
            }
        }
        let mut members: HashMap<usize, Vec<usize>> = HashMap::new();
        for i in 0..list.len() {
            let r = root(&mut parent, i);
            members.entry(r).or_default().push(i);
        }
        let mut roots: Vec<usize> = members.keys().copied().collect();
        roots.sort_unstable();
        for r in roots {
            let Some(m) = members.get(&r) else { continue };
            let mut count: HashMap<(u32, u32), usize> = HashMap::new();
            let (mut area, mut depth) = (0.0, 0.0);
            for &i in m {
                let t = ids[i];
                for k in 0..3 {
                    let (a, b) = (t[k], t[(k + 1) % 3]);
                    if a != b {
                        *count.entry((a.min(b), a.max(b))).or_default() += 1;
                    }
                }
                let p = tris[list[i]].2;
                let a2 = ((p[1][0] - p[0][0]) * (p[2][1] - p[0][1]) - (p[2][0] - p[0][0]) * (p[1][1] - p[0][1])).abs() / 2.0;
                area += a2;
                depth += a2 * (p[0][2] + p[1][2] + p[2][2]) / 3.0;
            }
            if area < 1.0 {
                continue;
            }
            let segs: Vec<cut::Segment> = count
                .iter()
                .filter(|(_, n)| **n == 1)
                .filter_map(|((a, b), _)| {
                    let (pa, pb) = (weld.points.get(*a as usize)?, weld.points.get(*b as usize)?);
                    Some([[pa[0], pa[1]], [pb[0], pb[1]]])
                })
                .collect();
            let rings = cut::chain(&segs, 0.01, 0.01).closed;
            if !rings.is_empty() {
                let plane =
                    m.first().and_then(|index| Occluder::new(tris[list[*index]].2)).map_or([depth / area, 0.0, 0.0], |triangle| triangle.plane);
                out.push(Face {
                    rings,
                    class,
                    depth: depth / area,
                    element: sources.get(key).and_then(|source| source.element.clone()),
                    depth_plane: plane,
                });
            }
        }
    }
    out.sort_by(|a, b| b.depth.total_cmp(&a.depth));
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cuboid(lo: P3, hi: P3) -> Mesh {
        let c = |i: usize| [if i & 1 == 0 { lo[0] } else { hi[0] }, if i & 2 == 0 { lo[1] } else { hi[1] }, if i & 4 == 0 { lo[2] } else { hi[2] }];
        let mut m = Mesh::default();
        m.hexahedron([c(0), c(1), c(3), c(2)], [c(4), c(5), c(7), c(6)]);
        m
    }

    /// Looking north (+y) from the south: x across, z up.
    fn north() -> Look {
        Look { origin: [0.0, 0.0], t: [1.0, 0.0], d: [0.0, 1.0] }
    }

    fn total(edges: &[Edge]) -> f64 {
        edges.iter().map(|e| (e.b[0] - e.a[0]).hypot(e.b[1] - e.a[1])).sum()
    }

    #[test]
    fn a_box_shows_its_outline_and_no_diagonals() {
        let m = cuboid([0.0, 0.0, 0.0], [4000.0, 3000.0, 2000.0]);
        let p = project(&[Source { mesh: &m, class: Class::Wall, group: 0, element: None }], &north(), (-1e5, 1e5), Near::Caps(&[]));
        // Square-on, the front face's four edges; the back face's coincide and are hidden or
        // merged, the depth edges are points. No triangle diagonals.
        assert!(p.edges.iter().all(|e| (e.a[0] - e.b[0]).abs() < 1e-6 || (e.a[1] - e.b[1]).abs() < 1e-6), "no diagonals");
        assert!(p.edges.iter().all(|e| e.outline), "everything is against the background");
        assert!((total(&p.edges) - 12_000.0).abs() < 1.0, "{}", total(&p.edges));
        assert_eq!(p.faces.len(), 2, "front and back face, back first");
        assert!(p.faces[0].depth > p.faces[1].depth);
    }

    #[test]
    fn a_box_in_front_hides_the_edges_behind_it() {
        let back = cuboid([0.0, 5000.0, 0.0], [4000.0, 6000.0, 4000.0]);
        let front = cuboid([1000.0, 0.0, 0.0], [3000.0, 1000.0, 2000.0]);
        let sources = [
            Source { mesh: &back, class: Class::Wall, group: 0, element: None },
            Source { mesh: &front, class: Class::Balcony, group: 1, element: None },
        ];
        let p = project(&sources, &north(), (-1e5, 1e5), Near::Caps(&[]));
        // The back box's bottom edge is hidden where the front box stands.
        let bottom: f64 =
            p.edges.iter().filter(|e| e.class == Class::Wall && e.a[1].abs() < 1e-6 && e.b[1].abs() < 1e-6).map(|e| (e.b[0] - e.a[0]).abs()).sum();
        assert!((bottom - 2000.0).abs() < 1.0, "{bottom}");
        // The front box's top edge is inside the back box's outline: a visible edge, not outline.
        let top = p.edges.iter().find(|e| e.class == Class::Balcony && (e.a[1] - 2000.0).abs() < 1e-6).unwrap();
        assert!(!top.outline);
        // Limiting the depth drops the back box.
        let near = project(&sources, &north(), (-1e5, 2000.0), Near::Caps(&[]));
        assert!(near.edges.iter().all(|e| e.class == Class::Balcony));
    }

    #[test]
    fn walls_in_line_show_no_joint_and_a_near_cut_closes_the_shape() {
        // Two boxes side by side, welded as one group: the joint between them is a coplanar seam.
        let a = cuboid([0.0, 0.0, 0.0], [2000.0, 400.0, 3000.0]);
        let b = cuboid([2000.0, 0.0, 0.0], [5000.0, 400.0, 3000.0]);
        let p = project(
            &[Source { mesh: &a, class: Class::Wall, group: 7, element: None }, Source { mesh: &b, class: Class::Wall, group: 7, element: None }],
            &north(),
            (-1e5, 1e5),
            Near::Caps(&[]),
        );
        // The touching end faces are inside: no joint line, just the 5 × 3 m outline.
        assert!((total(&p.edges) - 16_000.0).abs() < 1.0, "{}", total(&p.edges));
        // Cut 200 mm into a box: its section is drawn even though no edge lies there, and the cut
        // face hides the back of the box.
        let deep = cuboid([0.0, 0.0, 0.0], [1000.0, 3000.0, 1000.0]);
        let p = project(&[Source { mesh: &deep, class: Class::Wall, group: 0, element: None }], &north(), (200.0, 1e5), Near::Cut);
        assert!((total(&p.edges) - 4000.0).abs() < 1.0, "{}", total(&p.edges));
        assert!(p.faces.last().is_some_and(|f| f.depth == 200.0), "the cap is toned, nearest");
        // A section's poche hides what is behind it.
        let poche = vec![vec![vec![[-10.0, -10.0], [500.0, -10.0], [500.0, 1010.0], [-10.0, 1010.0]]]];
        let p = project(&[Source { mesh: &deep, class: Class::Wall, group: 0, element: None }], &north(), (200.0, 1e5), Near::Caps(&poche));
        assert!((total(&p.edges) - 2000.0).abs() < 1.0, "only the back face's right half shows: {}", total(&p.edges));
    }

    #[test]
    fn slivers_and_t_junctions_in_a_flat_face_never_show() {
        // A 2 × 1 m face in the plane y = 0: one square on the left, two strips on the right meeting
        // its edge in a T, and a zero-area triangle along the bottom (as earcut leaves them).
        let mut m = Mesh::default();
        let p = |x: f64, z: f64| [x, 0.0, z];
        m.quad(p(0.0, 0.0), p(1000.0, 0.0), p(1000.0, 1000.0), p(0.0, 1000.0));
        m.quad(p(1000.0, 0.0), p(2000.0, 0.0), p(2000.0, 500.0), p(1000.0, 500.0));
        m.quad(p(1000.0, 500.0), p(2000.0, 500.0), p(2000.0, 1000.0), p(1000.0, 1000.0));
        let base = m.positions.len() as u32;
        m.positions.extend([p(0.0, 0.0), p(500.0, 0.0), p(1000.0, 0.0)]);
        m.triangles.push([base, base + 1, base + 2]);
        let pr = project(&[Source { mesh: &m, class: Class::Wall, group: 0, element: None }], &north(), (-1e5, 1e5), Near::Caps(&[]));
        assert!((total(&pr.edges) - 6000.0).abs() < 1.0, "only the outline: {}", total(&pr.edges));
        assert!(!pr.edges.iter().any(|e| (e.a[0] - 1000.0).abs() < 1e-6 && (e.b[0] - 1000.0).abs() < 1e-6), "no joint at x = 1 m");
    }

    #[test]
    fn broken_meshes_project_without_panicking() {
        let mut m = cuboid([0.0, 0.0, 0.0], [1000.0, 1000.0, 1000.0]);
        m.triangles.push([0, 1, 1000]);
        m.triangles.push([0, 0, 0]);
        m.positions.push([f64::NAN, 1.0, 2.0]);
        m.positions.push([1e300, 1.0, 2.0]);
        let n = m.positions.len() as u32;
        m.triangles.push([n - 1, n - 2, 3]);
        let p = project(&[Source { mesh: &m, class: Class::Wall, group: 0, element: None }], &north(), (-1e5, 1e5), Near::Cut);
        assert!(!p.edges.is_empty());
        let empty = Mesh::default();
        let p = project(&[Source { mesh: &empty, class: Class::Wall, group: 0, element: None }], &north(), (f64::NAN, 1e5), Near::Cut);
        assert!(p.edges.is_empty() && p.faces.is_empty());
    }

    #[test]
    fn visible_edge_ownership_respects_depth_and_hidden_pieces() {
        use crate::views::DrawingElementKind;
        let back = cuboid([0.0, 1000.0, 0.0], [3000.0, 1200.0, 3000.0]);
        let front = cuboid([1000.0, 0.0, -100.0], [2000.0, 200.0, 4000.0]);
        let sources = [
            Source { mesh: &back, class: Class::Wall, group: 0, element: Some(Element::new("back", DrawingElementKind::FacadeWall)) },
            Source { mesh: &front, class: Class::Frame, group: 1, element: Some(Element::new("front", DrawingElementKind::Opening)) },
        ];
        let projection = project(&sources, &north(), (-1000.0, 5000.0), Near::Caps(&[]));
        let elements: Vec<_> = projection.edges.iter().flat_map(|edge| &edge.element_segments).collect();
        assert!(elements.iter().any(|element| element.id == "back"));
        assert!(elements.iter().any(|element| element.id == "front"));
        assert!(elements.iter().filter(|element| element.id == "back").all(|element| {
            let middle = (element.segment[0][0] + element.segment[1][0]) / 2.0;
            !(1000.01..1999.99).contains(&middle)
        }));
        assert!(
            projection
                .edges
                .iter()
                .filter(|edge| edge.class == Class::Frame)
                .flat_map(|edge| &edge.element_segments)
                .all(|element| element.id == "front")
        );
    }

    #[test]
    fn welded_collinear_facades_keep_their_own_visible_spans() {
        use crate::views::DrawingElementKind;
        let left = cuboid([0.0, 0.0, 0.0], [1000.0, 200.0, 3000.0]);
        let right = cuboid([1000.0, 0.0, 0.0], [2000.0, 200.0, 3000.0]);
        let projection = project(
            &[
                Source { mesh: &left, class: Class::Wall, group: 7, element: Some(Element::new("left", DrawingElementKind::FacadeWall)) },
                Source { mesh: &right, class: Class::Wall, group: 7, element: Some(Element::new("right", DrawingElementKind::FacadeWall)) },
            ],
            &north(),
            (-1000.0, 5000.0),
            Near::Caps(&[]),
        );
        let elements: Vec<_> = projection.edges.iter().flat_map(|edge| &edge.element_segments).collect();
        assert!(elements.iter().any(|element| element.id == "left"));
        assert!(elements.iter().any(|element| element.id == "right"));
        for element in elements {
            let middle = (element.segment[0][0] + element.segment[1][0]) / 2.0;
            assert!(if element.id == "left" { middle <= 1000.0 } else { middle >= 1000.0 }, "{element:?}");
            assert!(!element.segment.iter().all(|point| (point[0] - 1000.0).abs() < 1e-8));
        }
    }
}
