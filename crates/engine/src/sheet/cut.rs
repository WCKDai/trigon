//! Plane cuts. A mesh meets the cutting plane in segments; chained end to end they become closed
//! loops (poche) or, where a mesh isn't closed, open chains that are drawn as heavy lines. Nothing
//! here assumes a watertight or consistently wound mesh, and nothing panics on a broken one.

use std::collections::HashMap;

use i_overlay::core::fill_rule::FillRule;
use i_overlay::core::overlay_rule::OverlayRule;
use i_overlay::float::single::SingleFloatOverlay;
use trigon_model::mesh::Mesh;

pub(crate) type Segment = [[f64; 2]; 2];
pub(crate) type Ring = Vec<[f64; 2]>;
/// An outer ring followed by its holes.
pub(crate) type Shape = Vec<Ring>;

/// Coordinates beyond this (mm) are not a building.
const LIMIT: f64 = 1e8;

/// Appends the segments where `mesh` crosses the plane `depth = 0`. `map` takes a model point to
/// drawing `(x, y)` and its `depth` from the plane. A vertex on the plane counts as behind it, so
/// a triangle touching the plane at a corner or along an edge gives no sliver.
pub(crate) fn segments(mesh: &Mesh, map: &impl Fn([f64; 3]) -> [f64; 3], out: &mut Vec<Segment>) {
    let pts: Vec<[f64; 3]> = mesh.positions.iter().map(|p| map(*p)).collect();
    for t in &mesh.triangles {
        let Some(p) = t.iter().map(|&i| pts.get(i as usize).copied()).collect::<Option<Vec<_>>>() else { continue };
        if p.iter().flatten().any(|v| !v.is_finite() || v.abs() > LIMIT) {
            continue;
        }
        let behind: Vec<bool> = p.iter().map(|q| q[2] >= 0.0).collect();
        let n = behind.iter().filter(|b| **b).count();
        if n == 0 || n == 3 {
            continue;
        }
        let mut hits = Vec::with_capacity(2);
        for k in 0..3 {
            let (a, b) = (p[k], p[(k + 1) % 3]);
            if behind[k] == behind[(k + 1) % 3] {
                continue;
            }
            // The same edge in the neighbouring triangle gives bit-identical points.
            let (a, b) = if a < b { (a, b) } else { (b, a) };
            let s = a[2] / (a[2] - b[2]);
            hits.push([a[0] + (b[0] - a[0]) * s, a[1] + (b[1] - a[1]) * s]);
        }
        if let [a, b] = hits[..]
            && (a[0] - b[0]).hypot(a[1] - b[1]) > 1e-9
        {
            out.push([a, b]);
        }
    }
}

/// Points within `tol` of each other become one vertex.
struct Weld {
    tol: f64,
    cells: HashMap<(i64, i64), Vec<usize>>,
    points: Vec<[f64; 2]>,
}

impl Weld {
    fn new(tol: f64) -> Self {
        Weld { tol: tol.max(1e-9), cells: HashMap::new(), points: Vec::new() }
    }

    fn id(&mut self, p: [f64; 2]) -> usize {
        let cell = |v: f64| (v / self.tol).floor() as i64;
        let (cx, cy) = (cell(p[0]), cell(p[1]));
        for dx in -1..=1 {
            for dy in -1..=1 {
                if let Some(ids) = self.cells.get(&(cx + dx, cy + dy)) {
                    for &i in ids {
                        let q = self.points[i];
                        if (q[0] - p[0]).hypot(q[1] - p[1]) <= self.tol {
                            return i;
                        }
                    }
                }
            }
        }
        let i = self.points.len();
        self.points.push(p);
        self.cells.entry((cx, cy)).or_default().push(i);
        i
    }
}

/// Chained segments.
#[derive(Debug, Default)]
pub(crate) struct Chains {
    pub(crate) closed: Vec<Ring>,
    pub(crate) open: Vec<Ring>,
}

pub(crate) fn area2(ring: &[[f64; 2]]) -> f64 {
    let n = ring.len();
    (0..n).map(|i| ring[i][0] * ring[(i + 1) % n][1] - ring[(i + 1) % n][0] * ring[i][1]).sum()
}

/// Walks a graph's edges into trails, each a list of `(edge, forwards)`: open trails start at
/// vertices with an odd number of edges, the rest are closed.
fn trails(vertices: usize, edges: &[[usize; 2]]) -> (Vec<Vec<(usize, bool)>>, Vec<Vec<(usize, bool)>>) {
    let mut at: Vec<Vec<usize>> = vec![Vec::new(); vertices];
    for (k, e) in edges.iter().enumerate() {
        if let Some(list) = at.get_mut(e[0]) {
            list.push(k);
        }
        if e[1] != e[0]
            && let Some(list) = at.get_mut(e[1])
        {
            list.push(k);
        }
    }
    let mut used = vec![false; edges.len()];
    // Unused edge ends at each vertex (a loop from a vertex to itself has two).
    let mut left = vec![0usize; vertices];
    for e in edges {
        for v in e {
            if let Some(n) = left.get_mut(*v) {
                *n += 1;
            }
        }
    }
    let mut next = vec![0usize; vertices];
    let mut walk = |start: usize, used: &mut Vec<bool>, left: &mut Vec<usize>| -> (Vec<(usize, bool)>, usize) {
        let mut trail = Vec::new();
        let mut v = start;
        loop {
            let list = &at[v];
            while next[v] < list.len() && used[list[next[v]]] {
                next[v] += 1;
            }
            let Some(&k) = list.get(next[v]) else { break };
            used[k] = true;
            let [a, b] = edges[k];
            left[a] -= 1;
            left[b] -= 1;
            trail.push((k, a == v));
            v = if a == v { b } else { a };
        }
        (trail, v)
    };
    let (mut closed, mut open) = (Vec::new(), Vec::new());
    for v in 0..vertices {
        while left[v] % 2 == 1 {
            open.push(walk(v, &mut used, &mut left).0);
        }
    }
    for v in 0..vertices {
        while left[v] > 0 {
            let (trail, end) = walk(v, &mut used, &mut left);
            if end == v { closed.push(trail) } else { open.push(trail) }
        }
    }
    (closed, open)
}

/// Polylines joined along a trail, without doubling the points where they meet.
fn join_along(trail: &[(usize, bool)], lines: &[Ring]) -> Ring {
    let mut out: Ring = Vec::new();
    for &(k, forwards) in trail {
        let Some(line) = lines.get(k) else { continue };
        let mut pts = line.clone();
        if !forwards {
            pts.reverse();
        }
        if !out.is_empty() && !pts.is_empty() {
            pts.remove(0);
        }
        out.extend(pts);
    }
    out
}

/// Chains segments end to end. Endpoints within `snap` mm are the same point; open chains whose
/// ends lie within `join` mm of each other are joined (or closed).
pub(crate) fn chain(segments: &[Segment], snap: f64, join: f64) -> Chains {
    let mut weld = Weld::new(snap);
    let mut edges: Vec<[usize; 2]> = Vec::with_capacity(segments.len());
    for s in segments {
        if s.iter().flatten().any(|v| !v.is_finite() || v.abs() > LIMIT) {
            continue;
        }
        let (a, b) = (weld.id(s[0]), weld.id(s[1]));
        if a != b {
            edges.push([a, b]);
        }
    }
    let lines: Vec<Ring> = edges.iter().map(|e| vec![weld.points[e[0]], weld.points[e[1]]]).collect();
    let (closed, open) = trails(weld.points.len(), &edges);
    let mut out = Chains::default();
    for t in &closed {
        let mut ring = join_along(t, &lines);
        ring.pop();
        out.closed.push(ring);
    }
    // Open chains again, end to end within `join`.
    let open: Vec<Ring> = open.iter().map(|t| join_along(t, &lines)).filter(|c| c.len() >= 2).collect();
    let mut ends = Weld::new(join.max(snap));
    let mut links: Vec<[usize; 2]> = Vec::with_capacity(open.len());
    for c in &open {
        let (Some(a), Some(b)) = (c.first(), c.last()) else { continue };
        links.push([ends.id(*a), ends.id(*b)]);
    }
    let (closed, still_open) = trails(ends.points.len(), &links);
    for t in &closed {
        let mut ring = join_along(t, &open);
        if ring.len() > 1 && ring.first().zip(ring.last()).is_some_and(|(a, b)| (a[0] - b[0]).hypot(a[1] - b[1]) <= join.max(snap)) {
            ring.pop();
        }
        out.closed.push(ring);
    }
    out.open = still_open.iter().map(|t| join_along(t, &open)).filter(|c| c.len() >= 2).collect();
    out.closed.retain(|r| r.len() >= 3 && area2(r).abs() > 1e-6);
    out
}

fn finite_rings(rings: &[Ring]) -> Vec<Ring> {
    rings.iter().filter(|r| r.len() >= 3 && r.iter().flatten().all(|v| v.is_finite() && v.abs() <= LIMIT)).cloned().collect()
}

/// The area inside an odd number of `loops` (one solid's cut), as shapes with their holes.
pub(crate) fn even_odd(loops: &[Ring]) -> Vec<Shape> {
    let loops = finite_rings(loops);
    if loops.is_empty() {
        return Vec::new();
    }
    loops.overlay(&Vec::<Ring>::new(), OverlayRule::Subject, FillRule::EvenOdd)
}

/// The union of shapes (outer rings counter-clockwise, holes clockwise).
pub(crate) fn union(shapes: &[Shape]) -> Vec<Shape> {
    let rings: Vec<Ring> = finite_rings(&shapes.iter().flatten().cloned().collect::<Vec<_>>());
    if rings.is_empty() {
        return Vec::new();
    }
    rings.overlay(&Vec::<Ring>::new(), OverlayRule::Subject, FillRule::NonZero)
}

/// `shapes` minus `minus`.
pub(crate) fn difference(shapes: &[Shape], minus: &[Shape]) -> Vec<Shape> {
    let a: Vec<Ring> = finite_rings(&shapes.iter().flatten().cloned().collect::<Vec<_>>());
    let b: Vec<Ring> = finite_rings(&minus.iter().flatten().cloned().collect::<Vec<_>>());
    if a.is_empty() || b.is_empty() {
        return if a.is_empty() { Vec::new() } else { shapes.to_vec() };
    }
    a.overlay(&b, OverlayRule::Difference, FillRule::NonZero)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A closed box `lo..hi`, wound outward.
    fn cuboid(lo: [f64; 3], hi: [f64; 3]) -> Mesh {
        let c = |i: usize| [if i & 1 == 0 { lo[0] } else { hi[0] }, if i & 2 == 0 { lo[1] } else { hi[1] }, if i & 4 == 0 { lo[2] } else { hi[2] }];
        let mut m = Mesh::default();
        m.hexahedron([c(0), c(1), c(3), c(2)], [c(4), c(5), c(7), c(6)]);
        m
    }

    /// Looking north along y: x across, z up, depth from y = `y0`.
    fn at(y0: f64) -> impl Fn([f64; 3]) -> [f64; 3] {
        move |p| [p[0], p[2], p[1] - y0]
    }

    #[test]
    fn a_closed_box_cuts_to_one_rectangle() {
        let mut segs = Vec::new();
        segments(&cuboid([0.0, 0.0, 0.0], [4000.0, 3000.0, 2500.0]), &at(1200.0), &mut segs);
        let c = chain(&segs, 0.5, 5.0);
        assert_eq!((c.closed.len(), c.open.len()), (1, 0));
        assert!((area2(&c.closed[0]).abs() / 2.0 - 4000.0 * 2500.0).abs() < 1e-3);
        // Through a face's corner and along an edge: still one clean rectangle, no slivers.
        let mut segs = Vec::new();
        segments(&cuboid([0.0, 0.0, 0.0], [4000.0, 3000.0, 2500.0]), &at(0.0), &mut segs);
        let c = chain(&segs, 0.5, 5.0);
        assert!(c.closed.iter().all(|r| area2(r).abs() / 2.0 > 1.0) && c.open.is_empty());
        let shapes = even_odd(&c.closed);
        let area: f64 = shapes.iter().flatten().map(|r| area2(r)).sum::<f64>() / 2.0;
        assert!(area.abs() <= 4000.0 * 2500.0 + 1.0);
    }

    #[test]
    fn a_box_with_a_hole_cuts_to_a_ring() {
        // A square tube along y: four walls around a 2 × 1 m hole.
        let mut m = cuboid([0.0, 0.0, 0.0], [4000.0, 3000.0, 500.0]);
        m.append(&cuboid([0.0, 0.0, 1500.0], [4000.0, 3000.0, 2000.0]));
        m.append(&cuboid([0.0, 0.0, 500.0], [1000.0, 3000.0, 1500.0]));
        m.append(&cuboid([3000.0, 0.0, 500.0], [4000.0, 3000.0, 1500.0]));
        let mut segs = Vec::new();
        segments(&m, &at(1500.0), &mut segs);
        let c = chain(&segs, 0.5, 5.0);
        assert!(c.open.is_empty());
        let shapes = even_odd(&c.closed);
        assert_eq!(shapes.len(), 1, "one solid");
        assert_eq!(shapes[0].len(), 2, "an outline and one hole");
        let area: f64 = shapes[0].iter().map(|r| area2(r)).sum::<f64>() / 2.0;
        assert!((area - (4000.0 * 2000.0 - 2000.0 * 1000.0)).abs() < 1.0, "{area}");
        assert!(area2(&shapes[0][0]) > 0.0 && area2(&shapes[0][1]) < 0.0, "outline counter-clockwise, hole clockwise");
    }

    #[test]
    fn overlapping_solids_union_without_seams_and_differences_cut_holes() {
        let rect = |x0: f64, y0: f64, x1: f64, y1: f64| vec![vec![[x0, y0], [x1, y0], [x1, y1], [x0, y1]]];
        let u = union(&[rect(0.0, 0.0, 2000.0, 1000.0), rect(1000.0, 0.0, 3000.0, 1000.0)]);
        assert_eq!(u.len(), 1);
        let d = difference(&u, &[rect(500.0, 250.0, 700.0, 750.0)]);
        assert_eq!(d[0].len(), 2);
    }

    #[test]
    fn open_and_broken_meshes_never_panic() {
        // One quad: a single surface cut gives an open chain.
        let mut quad = Mesh::default();
        quad.quad([0.0, -500.0, 0.0], [3000.0, 500.0, 0.0], [3000.0, 500.0, 2000.0], [0.0, -500.0, 2000.0]);
        let mut segs = Vec::new();
        segments(&quad, &at(0.0), &mut segs);
        let c = chain(&segs, 0.5, 5.0);
        assert_eq!((c.closed.len(), c.open.len()), (0, 1));
        // Bad indices, NaN, infinity, huge numbers, a box missing a face, duplicate triangles.
        let mut broken = cuboid([0.0, 0.0, 0.0], [1000.0, 1000.0, 1000.0]);
        broken.triangles.truncate(10);
        broken.triangles.push([0, 1, 99]);
        broken.triangles.push(broken.triangles[0]);
        broken.positions.push([f64::NAN, 0.0, 0.0]);
        broken.positions.push([f64::INFINITY, 1e300, -1e300]);
        let n = broken.positions.len() as u32;
        broken.triangles.push([n - 1, n - 2, 0]);
        let mut segs = Vec::new();
        segments(&broken, &at(500.0), &mut segs);
        segs.push([[f64::NAN, 0.0], [1.0, 1.0]]);
        segs.push([[0.0, 0.0], [0.0, 0.0]]);
        let c = chain(&segs, 0.5, 5.0);
        let _ = even_odd(&c.closed);
        let _ = union(&[c.closed.clone(), c.open.clone()]);
        // Open chains whose ends nearly meet are closed.
        let gap =
            vec![[[0.0, 0.0], [1000.0, 0.0]], [[1000.0, 0.0], [1000.0, 1000.0]], [[1000.0, 1000.0], [0.0, 1000.0]], [[0.0, 1000.0], [0.0, 3.0]]];
        let c = chain(&gap, 0.5, 5.0);
        assert_eq!((c.closed.len(), c.open.len()), (1, 0));
    }
}
