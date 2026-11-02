//! Wall joins in plan. Where an interior wall meets the facade or another wall, its end is
//! trimmed or extended so the two meet face to face: a butt join on the near face (a T), and at a
//! corner of two walls the thicker one runs through to the other's far face (an L). The facade
//! walls are mitred at their corners (`build::inner_ring`), so after joining every wall end on a
//! storey either sits on a face or is free. Walls closer than 20° to parallel never join end to
//! face (collinear pieces either side of a door stay apart), and an end moves at most `REACH`.

pub(crate) mod profiles;

/// The farthest a wall end is extended to reach a face, mm.
pub const REACH: f64 = 300.0;
/// sin 20°: walls closer to parallel than this don't join end to face.
const MIN_SIN: f64 = 0.34;
/// Slack when deciding whether an end is at a wall, mm.
const SLACK: f64 = 60.0;
/// The shortest a wall may become by trimming, mm.
const MIN_LENGTH: f64 = 50.0;

/// A wall's centre line and thickness.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Axis {
    pub a: [f64; 2],
    pub b: [f64; 2],
    pub thickness: f64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Target {
    /// The facade wall of footprint edge `n`.
    Facade(usize),
    /// Another wall, by its index in the input.
    Wall(usize),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum JoinKind {
    /// The end stops on the near face of the target.
    Butt,
    /// The end runs to the target's far face (the through wall of an L corner).
    Through,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Join {
    pub target: Target,
    pub kind: JoinKind,
}

/// A wall after joining.
#[derive(Debug, Clone, PartialEq)]
pub struct Joined {
    /// The centre line, ends moved onto the faces they join.
    pub a: [f64; 2],
    pub b: [f64; 2],
    /// Plan outline, counter-clockwise: right side from `a` to `b`, then the left side back.
    pub outline: [[f64; 2]; 4],
    /// What the ends at `a` and at `b` join, if anything.
    pub ends: [Option<Join>; 2],
}

/// Disjoint plan shapes in site millimetres. Each shape has an outer ring followed by holes.
pub type MaterialProfile = Vec<Vec<Vec<[f64; 2]>>>;

/// A legacy join reference and, where a quad is insufficient, its actual gross material.
/// Door voids are not included; the built wall's mesh and cut contain its finished material.
#[derive(Debug, Clone, PartialEq)]
pub struct JoinedMaterial {
    pub joined: Joined,
    /// `None` preserves the ordinary quadrilateral path. `Some` may be concave or disconnected.
    pub material: Option<MaterialProfile>,
}

/// Join source axes with the same ordering and reach policy as [`join`], preserving stepped
/// material for mesh/export consumers. Compare `joined` with a built reference before pairing
/// it with separately supplied built walls. This does not validate arbitrary source numbers.
pub fn join_material(walls: &[Axis], facade: &[[f64; 2]], band: f64) -> Vec<JoinedMaterial> {
    profiles::join(walls, facade, band)
}

fn sub(a: [f64; 2], b: [f64; 2]) -> [f64; 2] {
    [a[0] - b[0], a[1] - b[1]]
}

fn cross(a: [f64; 2], b: [f64; 2]) -> f64 {
    a[0] * b[1] - a[1] * b[0]
}

fn dot(a: [f64; 2], b: [f64; 2]) -> f64 {
    a[0] * b[0] + a[1] * b[1]
}

fn along(p: [f64; 2], t: [f64; 2], s: f64) -> [f64; 2] {
    [p[0] + t[0] * s, p[1] + t[1] * s]
}

/// Unit direction, left normal and length of a segment.
fn frame(a: [f64; 2], b: [f64; 2]) -> Option<([f64; 2], [f64; 2], f64)> {
    let d = sub(b, a);
    let len = dot(d, d).sqrt();
    if !(len > 1e-6) || !len.is_finite() {
        return None;
    }
    let t = [d[0] / len, d[1] / len];
    Some((t, [-t[1], t[0]], len))
}

/// Whether `p` lies inside the closed polygon `ring` (even–odd rule).
fn inside(ring: &[[f64; 2]], p: [f64; 2]) -> bool {
    let mut yes = false;
    for i in 0..ring.len() {
        let (a, b) = (ring[i], ring[(i + 1) % ring.len()]);
        if (a[1] > p[1]) != (b[1] > p[1]) && p[0] < (b[0] - a[0]) * (p[1] - a[1]) / (b[1] - a[1]) + a[0] {
            yes = !yes;
        }
    }
    yes
}

/// Where the line `p + s·t` crosses the line through `q` along `d`, as `s`; `None` if parallel.
fn hit(p: [f64; 2], t: [f64; 2], q: [f64; 2], d: [f64; 2]) -> Option<f64> {
    let den = cross(t, d);
    (den.abs() > 1e-12).then(|| cross(sub(q, p), d) / den)
}

/// A face an end can stop on: a line, its rank (how far the end must move to reach the
/// target), and the join it makes.
struct Candidate {
    point: [f64; 2],
    dir: [f64; 2],
    rank: f64,
    join: Join,
}

#[derive(Clone, Copy)]
struct Run {
    continued: bool,
    length: f64,
    first: usize,
}

fn endpoint_key(point: [f64; 2]) -> [u64; 2] {
    // Signed zero names the same physical station; other source coordinates stay exact.
    point.map(|v| if v == 0.0 { 0 } else { v.to_bits() })
}

/// Joins the walls of one storey. `facade` is the facade's inner face for this storey
/// (counter-clockwise, one vertex per footprint corner) and `band` the facade wall's thickness.
/// Output order matches the input.
///
/// This quadrilateral interface handles single-face terminations. A branch meeting an exact
/// mixed-width continuation can need a stepped end with more than four corners; mesh and
/// exchange consumers use [`join_material`] for that case. These legacy quads alone do not
/// describe its material partition.
pub fn join(walls: &[Axis], facade: &[[f64; 2]], band: f64) -> Vec<Joined> {
    let frames: Vec<_> = walls.iter().map(|w| frame(w.a, w.b)).collect();
    // A shared, exactly collinear endpoint is a seam inside a continuous run,
    // rather than two independent L corners. Index the exact stations rather than
    // repeating the all-wall scan when determining each end's run priority.
    let mut endpoints = std::collections::BTreeMap::<[u64; 2], Vec<(usize, usize)>>::new();
    for (i, w) in walls.iter().enumerate() {
        if frames[i].is_some() {
            for (end, point) in [w.a, w.b].into_iter().enumerate() {
                endpoints.entry(endpoint_key(point)).or_default().push((i, end));
            }
        }
    }
    let runs: Vec<[Run; 2]> = walls
        .iter()
        .enumerate()
        .map(|(i, w)| {
            std::array::from_fn(|end| {
                let mut run = Run { continued: false, length: frames[i].map_or(0.0, |f| f.2), first: i };
                let Some((t, _, len)) = frames[i] else { return run };
                let point = if end == 0 { w.a } else { w.b };
                let inward = if end == 0 { t } else { [-t[0], -t[1]] };
                let Some(neighbours) = endpoints.get(&endpoint_key(point)) else { return run };
                for &(j, other_end) in neighbours {
                    let other = &walls[j];
                    if i == j || (w.thickness - other.thickness).abs() > 1e-6 {
                        continue;
                    }
                    let Some((tj, _, other_len)) = frames[j] else { continue };
                    let direction = if other_end == 0 { tj } else { [-tj[0], -tj[1]] };
                    if cross(inward, direction).abs() <= 1e-9 && dot(inward, direction) < 0.0 {
                        run.continued = true;
                        run.length = run.length.max(len + other_len);
                        run.first = run.first.min(j);
                    }
                }
                run
            })
        })
        .collect();
    walls
        .iter()
        .enumerate()
        .map(|(i, w)| {
            let Some((t, n, len)) = frames[i] else { return square(w, None) };
            let h = w.thickness / 2.0;
            // The two ends: where they stand on the axis, and which way is out.
            let mut s = [0.0, len];
            let mut ends = [None, None];
            let mut faces: [Option<([f64; 2], [f64; 2])>; 2] = [None, None];
            for e in 0..2 {
                let out = if e == 0 { -1.0 } else { 1.0 };
                let s_end = s[e];
                let mut best: Option<Candidate> = None;
                let mut offer = |c: Candidate| {
                    if best.as_ref().is_none_or(|b| c.rank < b.rank) {
                        best = Some(c);
                    }
                };
                // The facade's inner face. An end outside it (drawn out to where the facade used to
                // be) comes back to the face however far out it is.
                let m = facade.len();
                let end_point = along(w.a, t, s_end);
                let reach_in = if m >= 3 && !inside(facade, end_point) { len } else { band + SLACK };
                for k in 0..m {
                    let (p, q) = (facade[k], facade[(k + 1) % m]);
                    let Some((d, _, flen)) = frame(p, q) else { continue };
                    let sin = cross(t, d).abs();
                    if sin < MIN_SIN {
                        continue;
                    }
                    let Some(sc) = hit(w.a, t, p, d) else { continue };
                    let r = dot(sub(along(w.a, t, sc), p), d);
                    let margin = h / sin + SLACK;
                    if r < -margin || r > flen + margin {
                        continue;
                    }
                    let gap = (sc - s_end) * out;
                    if gap > REACH || gap < -reach_in {
                        continue;
                    }
                    offer(Candidate { point: p, dir: d, rank: gap.abs(), join: Join { target: Target::Facade(k), kind: JoinKind::Butt } });
                }
                // Other walls.
                for (j, o) in walls.iter().enumerate() {
                    if j == i {
                        continue;
                    }
                    let Some((tj, nj, lj)) = frames[j] else { continue };
                    let sin = cross(t, tj).abs();
                    if sin < MIN_SIN {
                        continue;
                    }
                    let hj = o.thickness / 2.0;
                    let Some(sc) = hit(w.a, t, o.a, tj) else { continue };
                    // Where the axis crosses the other wall, along that wall.
                    let r = dot(sub(along(w.a, t, sc), o.a), tj);
                    let margin = h / sin + SLACK;
                    if r < -margin || r > lj + margin {
                        continue;
                    }
                    let side = |off: f64| hit(w.a, t, along(o.a, nj, off), tj);
                    let (Some(s1), Some(s2)) = (side(hj), side(-hj)) else { continue };
                    let (near, far) = if out > 0.0 { (s1.min(s2), s1.max(s2)) } else { (s1.max(s2), s1.min(s2)) };
                    let gap = (near - s_end) * out;
                    if gap > REACH || gap < -(2.0 * hj + SLACK) {
                        continue;
                    }
                    // Near the other wall's end too: a corner, where the thicker wall runs through.
                    let corner = r < margin || r > lj - margin;
                    let at_start = r.abs() <= 1e-6;
                    let at_end = (r - lj).abs() <= 1e-6;
                    let junction = (sc - s_end).abs() <= 1e-6 && (at_start || at_end);
                    let own_run = if junction { runs[i][e] } else { Run { continued: false, length: len, first: i } };
                    let other_run = if junction { runs[j][usize::from(at_end)] } else { Run { continued: false, length: lj, first: j } };
                    let through = corner && runs_through(w, own_run.length, o, (other_run.length, own_run.first, other_run.first));
                    if through && own_run.continued {
                        // Its continuation already fills the far side. Extending both
                        // definitions through this branch would duplicate material.
                        continue;
                    }
                    let (face, kind) = if through { (far, JoinKind::Through) } else { (near, JoinKind::Butt) };
                    let face_off = side_offset(w.a, t, (o.a, nj, hj), face);
                    let rank = (sc - s_end).abs();
                    offer(Candidate { point: along(o.a, nj, face_off), dir: tj, rank, join: Join { target: Target::Wall(j), kind } });
                }
                if let Some(c) = best
                    && let Some(sn) = hit(w.a, t, c.point, c.dir)
                {
                    s[e] = sn;
                    ends[e] = Some(c.join);
                    faces[e] = Some((c.point, c.dir));
                }
            }
            // A join that would make the wall vanish or turn inside out is dropped.
            if s[1] - s[0] < MIN_LENGTH {
                return square(w, Some((t, n, len)));
            }
            let corner = |e: usize, off: f64| -> [f64; 2] {
                let base = along(w.a, n, off);
                let at = match faces[e] {
                    Some((p, d)) => hit(base, t, p, d).filter(|v| v.is_finite()).unwrap_or(s[e]),
                    None => s[e],
                };
                along(base, t, at)
            };
            let outline = [corner(0, -h), corner(1, -h), corner(1, h), corner(0, h)];
            // A very oblique face can push one side past the other end; keep it square then.
            let side_ok = |k0: usize, k1: usize| dot(sub(outline[k1], outline[k0]), t) > MIN_LENGTH / 2.0;
            if !side_ok(0, 1) || !side_ok(3, 2) {
                return square(w, Some((t, n, len)));
            }
            Joined { a: along(w.a, t, s[0]), b: along(w.a, t, s[1]), outline, ends }
        })
        .collect()
}

/// Which side line (`±hj` from the other wall's axis `oa` along its left normal `nj`) the line
/// `p + s·t` meets at `s`.
fn side_offset(p: [f64; 2], t: [f64; 2], (oa, nj, hj): ([f64; 2], [f64; 2], f64), s: f64) -> f64 {
    if dot(sub(along(p, t, s), oa), nj) >= 0.0 { hj } else { -hj }
}

/// At a corner the thicker wall runs through; with equal thickness the longer, then the first.
fn runs_through(w: &Axis, len: f64, o: &Axis, (lj, i, j): (f64, usize, usize)) -> bool {
    if (w.thickness - o.thickness).abs() > 1.0 {
        return w.thickness > o.thickness;
    }
    if (len - lj).abs() > 1.0 {
        return len > lj;
    }
    i < j
}

/// The wall as drawn, with square ends.
fn square(w: &Axis, f: Option<([f64; 2], [f64; 2], f64)>) -> Joined {
    let n = f.or_else(|| frame(w.a, w.b)).map_or([0.0, 0.0], |(_, n, _)| n);
    let h = w.thickness / 2.0;
    let outline = [along(w.a, n, -h), along(w.b, n, -h), along(w.b, n, h), along(w.a, n, h)];
    Joined { a: w.a, b: w.b, outline, ends: [None, None] }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn close(a: [f64; 2], b: [f64; 2]) -> bool {
        (a[0] - b[0]).abs() < 1e-6 && (a[1] - b[1]).abs() < 1e-6
    }

    /// A 10 × 8 m room inside a facade whose inner face is the rectangle (0,0)–(10000,8000).
    fn facade() -> Vec<[f64; 2]> {
        vec![[0.0, 0.0], [10000.0, 0.0], [10000.0, 8000.0], [0.0, 8000.0]]
    }

    #[test]
    fn ends_short_of_or_inside_the_facade_meet_its_face() {
        // One end 200 mm short of the facade, the other 150 mm into it.
        let w = Axis { a: [5000.0, 200.0], b: [5000.0, 8150.0], thickness: 120.0 };
        let j = &join(&[w], &facade(), 400.0)[0];
        assert!(close(j.a, [5000.0, 0.0]) && close(j.b, [5000.0, 8000.0]));
        assert_eq!(
            j.ends,
            [Some(Join { target: Target::Facade(0), kind: JoinKind::Butt }), Some(Join { target: Target::Facade(2), kind: JoinKind::Butt })]
        );
        assert!(close(j.outline[0], [5060.0, 0.0]) && close(j.outline[3], [4940.0, 0.0]));
    }

    #[test]
    fn a_t_stops_on_the_near_face_and_an_l_runs_the_thicker_wall_through() {
        // A 240 mm wall along y = 4000; a 100 mm partition from y = 0 up to 50 mm short of it (T);
        // a 100 mm wall whose end meets the 240 wall's end (L).
        let main = Axis { a: [2000.0, 4000.0], b: [8000.0, 4000.0], thickness: 240.0 };
        let t = Axis { a: [5000.0, 0.0], b: [5000.0, 3830.0], thickness: 100.0 };
        let l = Axis { a: [8000.0, 4000.0], b: [8000.0, 7000.0], thickness: 100.0 };
        let out = join(&[main, t, l], &facade(), 400.0);
        // T: stops at y = 4000 − 120.
        assert!(close(out[1].b, [5000.0, 3880.0]), "{:?}", out[1].b);
        assert_eq!(out[1].ends[1], Some(Join { target: Target::Wall(0), kind: JoinKind::Butt }));
        // L: the 240 wall runs through to the partition's far face (x = 8050) …
        assert!(close(out[0].b, [8050.0, 4000.0]), "{:?}", out[0].b);
        assert_eq!(out[0].ends[1], Some(Join { target: Target::Wall(2), kind: JoinKind::Through }));
        // … and the partition stops on the 240 wall's face (y = 4120).
        assert!(close(out[2].a, [8000.0, 4120.0]), "{:?}", out[2].a);
        // The far end of the partition, 1000 mm short of the facade, is beyond reach: free.
        assert_eq!(out[2].ends[1], None);
    }

    #[test]
    fn collinear_pieces_beside_a_door_and_far_walls_stay_as_drawn() {
        let left = Axis { a: [1000.0, 4000.0], b: [4000.0, 4000.0], thickness: 120.0 };
        let right = Axis { a: [4900.0, 4000.0], b: [8000.0, 4000.0], thickness: 120.0 };
        let out = join(&[left, right], &facade(), 400.0);
        assert!(close(out[0].b, [4000.0, 4000.0]) && close(out[1].a, [4900.0, 4000.0]));
    }

    #[test]
    fn shared_collinear_ends_form_disjoint_t_junctions_without_gaps() {
        // A continuous wall may be stored as two definitions. Its shared endpoint
        // must not make both pieces extend through the same perpendicular branch.
        for angle in [0.0_f64, 37.0, 90.0] {
            let (sin, cos) = angle.to_radians().sin_cos();
            let at = |p: [f64; 2]| [2700.0 + p[0] * cos - p[1] * sin, -800.0 + p[0] * sin + p[1] * cos];
            for (branch_length, branch_width) in [(800.0, 100.0), (1800.0, 100.0), (4800.0, 100.0), (800.0, 240.0)] {
                let base = [
                    Axis { a: at([0.0, -1200.0]), b: at([0.0, 0.0]), thickness: 100.0 },
                    Axis { a: at([0.0, 0.0]), b: at([0.0, 2400.0]), thickness: 100.0 },
                    Axis { a: at([-branch_length, 0.0]), b: at([0.0, 0.0]), thickness: branch_width },
                ];
                for crossing in [false, true] {
                    let mut raw = base.to_vec();
                    let orders = if crossing {
                        raw.push(Axis { a: at([0.0, 0.0]), b: at([1800.0, 0.0]), thickness: branch_width });
                        vec![vec![0, 1, 2, 3], vec![3, 2, 1, 0], vec![2, 0, 3, 1], vec![1, 3, 0, 2]]
                    } else {
                        vec![vec![0, 1, 2], vec![1, 0, 2], vec![2, 0, 1], vec![2, 1, 0], vec![0, 2, 1], vec![1, 2, 0]]
                    };
                    for order in orders {
                        for reversed in [false, true] {
                            let sources: Vec<_> =
                                order.iter().map(|&i| if reversed { Axis { a: raw[i].b, b: raw[i].a, ..raw[i] } } else { raw[i] }).collect();
                            let joined = join(&sources, &[], 0.0);
                            for x in [-200.0, -125.0, -75.0, -25.0, 25.0, 75.0, 125.0, 200.0] {
                                for y in [-200.0, -125.0, -75.0, -25.0, 25.0, 75.0, 125.0, 200.0] {
                                    let point = at([x, y]);
                                    let expected = raw.iter().any(|w| inside(&square(w, None).outline, point));
                                    let coverage = joined.iter().filter(|w| inside(&w.outline, point)).count();
                                    assert_eq!(
                                        coverage,
                                        usize::from(expected),
                                        "junction material must occur exactly once: angle={angle}, branch={branch_length}/{branch_width}, order={order:?}, reversed={reversed}, sample={x}/{y}, joined={joined:?}"
                                    );
                                }
                            }
                        }
                    }
                }
            }
        }
    }

    #[test]
    fn nearby_stepped_stations_preserve_one_material_owner() {
        let base = [
            Axis { a: [1000.0, 5000.0], b: [3000.0, 5000.0], thickness: 240.0 },
            Axis { a: [3000.0, 5000.0], b: [3120.0, 5000.0], thickness: 120.0 },
            Axis { a: [3120.0, 5000.0], b: [9000.0, 5000.0], thickness: 400.0 },
            Axis { a: [3000.0, 2000.0], b: [3000.0, 5000.0], thickness: 300.0 },
            Axis { a: [3120.0, 8000.0], b: [3120.0, 5000.0], thickness: 120.0 },
        ];
        for angle in [0.0_f64, 37.0, 90.0] {
            let (sin, cos) = angle.to_radians().sin_cos();
            let at = |[x, y]: [f64; 2]| [30000.0 + x * cos - y * sin, -27000.0 + x * sin + y * cos];
            let sources = base.map(|w| Axis { a: at(w.a), b: at(w.b), ..w });
            for order in [[0, 1, 2, 3, 4], [4, 3, 2, 1, 0], [2, 4, 1, 3, 0]] {
                for reversed in [false, true] {
                    let raw: Vec<_> =
                        order.iter().map(|&i| if reversed { Axis { a: sources[i].b, b: sources[i].a, ..sources[i] } } else { sources[i] }).collect();
                    let walls = built_sources(&raw);
                    assert_complete_partition(&raw, &walls);
                    for wall in &walls {
                        closed_outward_volume(&wall.mesh);
                    }
                }
            }
        }
    }

    #[test]
    fn stepped_run_recomputes_branches_at_equal_width_seams() {
        let sources = [
            Axis { a: [1000., 5000.], b: [3000., 5000.], thickness: 240. },
            Axis { a: [3000., 5000.], b: [3120., 5000.], thickness: 120. },
            Axis { a: [3120., 5000.], b: [9000., 5000.], thickness: 120. },
            Axis { a: [3000., 2000.], b: [3000., 5000.], thickness: 120. },
            Axis { a: [3120., 8000.], b: [3120., 5000.], thickness: 180. },
        ];
        for (angle, offset) in [(0_f64, [0.0, 0.0]), (37.0, [150000.0, -270000.0]), (-118.0, [-30000.0, 41000.0])] {
            let (sin, cos) = angle.to_radians().sin_cos();
            let at = |p: [f64; 2]| [offset[0] + p[0] * cos - p[1] * sin, offset[1] + p[0] * sin + p[1] * cos];
            for seam in [3120.0, 3360.0] {
                let mut sources = sources;
                sources[1].b[0] = seam;
                sources[2].a[0] = seam;
                sources[4].a[0] = seam;
                sources[4].b[0] = seam;
                let sources = sources.map(|w| Axis { a: at(w.a), b: at(w.b), ..w });
                for order in [[0, 1, 2, 3, 4], [4, 3, 2, 1, 0], [2, 4, 1, 3, 0]] {
                    for reversals in 0..32 {
                        let raw: Vec<_> = order
                            .iter()
                            .map(|&i| if reversals & (1 << i) == 0 { sources[i] } else { Axis { a: sources[i].b, b: sources[i].a, ..sources[i] } })
                            .collect();
                        let walls = built_sources(&raw);
                        assert_complete_partition(&raw, &walls);
                        for wall in &walls {
                            let area: f64 = wall.cut.iter().map(|r| crate::footprint::signed_area2(r) / 2.0).sum();
                            assert!((closed_outward_volume(&wall.mesh) - area * wall.height).abs() < 1.0);
                            for end in wall.ends.iter().flatten() {
                                if let Target::Wall(target) = end.target {
                                    assert!(shared_boundary(&wall.cut, &walls[target].cut) > 1e-3, "join metadata must name a contacted wall");
                                }
                            }
                        }
                    }
                }
            }
        }
    }

    #[test]
    fn stepped_run_follows_equal_width_seams_and_ignores_remote_material() {
        let sources = [
            Axis { a: [1000., 5000.], b: [3000., 5000.], thickness: 240. },
            Axis { a: [3000., 5000.], b: [3100., 5000.], thickness: 120. },
            Axis { a: [3100., 5000.], b: [3200., 5000.], thickness: 120. },
            Axis { a: [3200., 5000.], b: [9000., 5000.], thickness: 400. },
            Axis { a: [3000., 2000.], b: [3000., 5000.], thickness: 1000. },
        ];
        assert_complete_partition(&sources, &built_sources(&sources));
        let base = stepped_sources();
        let before = join_material(&base, &[], 0.0);
        let mut extended = base.to_vec();
        extended.push(Axis { a: base[0].b, b: [5000., 200000.], thickness: 400. });
        let after = join_material(&extended, &[], 0.0);
        assert_eq!(before[2], after[2], "remote continuation must not change the local overlay grid or contact");
    }

    fn stepped_sources() -> [Axis; 3] {
        [
            Axis { a: [5000.0, 5000.0], b: [5000.0, 6500.0], thickness: 120.0 },
            Axis { a: [5000.0, 5000.0], b: [5000.0, 3700.0], thickness: 240.0 },
            Axis { a: [4300.0, 4250.0], b: [5000.0, 5000.0], thickness: 120.0 },
        ]
    }

    fn source_document(sources: &[Axis]) -> crate::Document {
        let bounds = sources.iter().flat_map(|w| [w.a, w.b]).fold([f64::INFINITY, f64::INFINITY, f64::NEG_INFINITY, f64::NEG_INFINITY], |b, p| {
            [b[0].min(p[0]), b[1].min(p[1]), b[2].max(p[0]), b[3].max(p[1])]
        });
        let [x0, y0, x1, y1] = bounds;
        let footprint = [[x0 - 3000.0, y0 - 3000.0], [x1 + 3000.0, y0 - 3000.0], [x1 + 3000.0, y1 + 3000.0], [x0 - 3000.0, y1 + 3000.0]];
        let mut b = crate::Building::new("junction", "Synthetic stepped junction", &footprint, 1, 3000.0).unwrap();
        b.interior_walls = sources
            .iter()
            .enumerate()
            .map(|(i, w)| crate::InteriorWall {
                id: format!("wall{i}"),
                levels: vec![0],
                a: w.a,
                b: w.b,
                thickness: w.thickness,
                load_bearing: false,
                doors: Vec::new(),
            })
            .collect();
        let mut doc = crate::Document::default();
        doc.buildings.push(b);
        doc
    }

    fn built_sources(sources: &[Axis]) -> Vec<crate::build::BuiltInterior> {
        let doc = source_document(sources);
        let before = serde_json::to_vec(&doc).unwrap();
        let mut built = crate::build(&doc).unwrap();
        assert_eq!(before, serde_json::to_vec(&doc).unwrap(), "building must not edit the document or source endpoints");
        let walls = std::mem::take(&mut built[0].interior);
        assert_eq!(walls.len(), sources.len());
        for (i, w) in walls.iter().enumerate() {
            assert_eq!(w.id, format!("wall{i}.L0"), "source identity and order");
            assert_eq!(w.thickness, sources[i].thickness);
        }
        walls
    }

    fn overlay_area(a: &[Vec<[f64; 2]>], b: &[Vec<[f64; 2]>], rule: i_overlay::core::overlay_rule::OverlayRule) -> f64 {
        use i_overlay::core::fill_rule::FillRule;
        use i_overlay::float::overlay::FloatOverlay;
        FloatOverlay::with_subj_and_clip(&a.to_vec(), &b.to_vec())
            .overlay(rule, FillRule::NonZero)
            .iter()
            .map(|shape| shape.iter().map(|r| crate::footprint::signed_area2(r) / 2.0).sum::<f64>().abs())
            .sum()
    }

    fn assert_complete_partition(sources: &[Axis], walls: &[crate::build::BuiltInterior]) {
        use i_overlay::core::overlay_rule::OverlayRule;
        for (i, a) in walls.iter().enumerate() {
            for b in &walls[..i] {
                let overlap = overlay_area(&a.cut, &b.cut, OverlayRule::Intersect);
                assert!(overlap < 0.05, "{} / {} overlap {overlap} mm²", a.id, b.id);
            }
        }
        let expected: Vec<_> = sources.iter().map(|w| square(w, None).outline.to_vec()).collect();
        let actual: Vec<_> = walls.iter().flat_map(|w| w.cut.clone()).collect();
        let missing = overlay_area(&expected, &actual, OverlayRule::Difference);
        let extra = overlay_area(&actual, &expected, OverlayRule::Difference);
        assert!(
            missing < 0.05 && extra < 0.05,
            "source union coverage: missing={missing}, extra={extra} mm²; references={:?}; cuts={actual:?}",
            walls.iter().map(|w| (&w.id, w.axis, w.outline)).collect::<Vec<_>>()
        );
    }

    #[test]
    fn mixed_thickness_step_build_has_single_complete_coverage() {
        let sources = stepped_sources();
        let walls = built_sources(&sources);
        let point = [5020.0, 4980.0];
        let coverage = walls.iter().filter(|w| w.cut.iter().any(|ring| inside(ring, point))).count();
        assert_eq!(coverage, 1, "one physical material at the mixed-width junction");
        assert_complete_partition(&sources, &walls);
        // This oblique source leaves a main band and a small triangular return beyond the
        // thick wall's shoulder. Both touch the run; a single quad cannot describe them.
        assert_eq!(walls[2].cut.len(), 2, "retain the source-union return rather than throwing it away");
        assert!(shared_boundary(&walls[2].cut, &walls[0].cut) > 20.0);
        assert!(shared_boundary(&walls[2].cut, &walls[1].cut) > 100.0);
        for w in &walls {
            let volume = closed_outward_volume(&w.mesh);
            let area: f64 = w.cut.iter().map(|r| crate::footprint::signed_area2(r) / 2.0).sum();
            assert!((volume - area * w.height).abs() < 1.0);
        }
    }

    fn shared_boundary(a: &[Vec<[f64; 2]>], b: &[Vec<[f64; 2]>]) -> f64 {
        let edges = |rings: &[Vec<[f64; 2]>]| -> Vec<_> {
            rings.iter().flat_map(|r| r.iter().copied().zip(r.iter().copied().cycle().skip(1)).take(r.len())).collect()
        };
        let mut shared = 0.0;
        for (p, q) in edges(a) {
            let Some((t, _, len)) = frame(p, q) else { continue };
            for (r, s) in edges(b) {
                if cross(sub(r, p), t).abs() < 1e-3 && cross(sub(s, p), t).abs() < 1e-3 {
                    let (a, b) = (dot(sub(r, p), t), dot(sub(s, p), t));
                    shared += (a.max(b).min(len) - a.min(b).max(0.0)).max(0.0);
                }
            }
        }
        shared
    }

    fn closed_outward_volume(mesh: &crate::Mesh) -> f64 {
        use std::collections::{BTreeMap, BTreeSet};
        assert!(!mesh.triangles.is_empty());
        // Overlay uses a fixed integer grid; weld at 0.001 mm, well below source precision.
        let key = |p: [f64; 3]| p.map(|v| (v * 1000.0).round() as i64);
        let mut faces = BTreeSet::new();
        let mut edges = BTreeMap::<_, (usize, i32)>::new();
        let origin = mesh.positions[0];
        let mut volume = 0.0;
        for tri in &mesh.triangles {
            let points = tri.map(|i| mesh.positions[i as usize]);
            assert!(points.iter().flatten().all(|v| v.is_finite()));
            let mut face = points.map(key);
            face.sort();
            assert!(faces.insert(face), "duplicate triangle");
            for i in 0..3 {
                let (a, b) = (key(points[i]), key(points[(i + 1) % 3]));
                assert_ne!(a, b, "collapsed triangle edge: {points:?}");
                let entry = edges.entry([a.min(b), a.max(b)]).or_default();
                entry.0 += 1;
                entry.1 += if a < b { 1 } else { -1 };
            }
            let [a, b, c] = points.map(|p| std::array::from_fn::<_, 3, _>(|i| p[i] - origin[i]));
            volume += (a[0] * (b[1] * c[2] - b[2] * c[1]) + a[1] * (b[2] * c[0] - b[0] * c[2]) + a[2] * (b[0] * c[1] - b[1] * c[0])) / 6.0;
        }
        let bad: Vec<_> = edges.iter().filter(|(_, v)| **v != (2, 0)).take(5).collect();
        assert!(bad.is_empty(), "open, nonmanifold or inconsistent solid: {bad:?}");
        assert!(volume > 0.0, "outward positive volume: {volume}");
        volume
    }

    #[test]
    fn stepped_profiles_preserve_union_under_order_direction_and_rigid_transforms() {
        for (angle, offset) in [(0_f64, [5000.0, 5000.0]), (37.0, [150000.0, -270000.0]), (-118.0, [-30000.0, 41000.0])] {
            let (sin, cos) = angle.to_radians().sin_cos();
            let at = |p: [f64; 2]| [offset[0] + p[0] * cos - p[1] * sin, offset[1] + p[0] * sin + p[1] * cos];
            for (north, south, branch) in [(120.0, 240.0, 120.0), (240.0, 120.0, 120.0), (40.0, 1000.0, 1000.0), (120.0, 120.5, 120.0)] {
                for endpoint in [[-700.0, -750.0], [-700.0, 750.0], [700.0, -750.0], [700.0, 750.0], [-1800.0, 0.0]] {
                    let raw = [
                        Axis { a: at([0.0, 0.0]), b: at([0.0, 2500.0]), thickness: north },
                        Axis { a: at([0.0, 0.0]), b: at([0.0, -2300.0]), thickness: south },
                        Axis { a: at(endpoint), b: at([0.0, 0.0]), thickness: branch },
                    ];
                    for order in [[0, 1, 2], [1, 0, 2], [2, 0, 1], [2, 1, 0], [0, 2, 1], [1, 2, 0]] {
                        for reversals in 0..8 {
                            let sources: Vec<_> = order
                                .iter()
                                .map(|&i| if reversals & (1 << i) == 0 { raw[i] } else { Axis { a: raw[i].b, b: raw[i].a, ..raw[i] } })
                                .collect();
                            let walls = built_sources(&sources);
                            assert_complete_partition(&sources, &walls);
                            for w in &walls {
                                let volume = closed_outward_volume(&w.mesh);
                                let area: f64 = w.cut.iter().map(|r| crate::footprint::signed_area2(r) / 2.0).sum();
                                assert!(
                                    (volume - area * w.height).abs() < 1.0,
                                    "volume/plan mismatch at {angle}/{north}/{south}/{branch}/{endpoint:?}"
                                );
                                for end in w.ends.iter().flatten() {
                                    if let Target::Wall(target) = end.target {
                                        assert!(shared_boundary(&w.cut, &walls[target].cut) > 1e-3, "join metadata must name a contacted wall");
                                    }
                                }
                            }
                        }
                    }
                }
            }
        }
    }

    #[test]
    fn perpendicular_branch_has_six_corners_and_opposite_branches_remain_disjoint() {
        let mut sources = stepped_sources().to_vec();
        sources[2].a = [3200.0, 5000.0];
        let walls = built_sources(&sources);
        assert_complete_partition(&sources, &walls);
        assert_eq!(walls[2].cut.len(), 1);
        assert_eq!(walls[2].cut[0].len(), 6, "a shoulder needs six corners");
        sources.push(Axis { a: [6800.0, 5000.0], b: [5000.0, 5000.0], thickness: 120.0 });
        for order in [[0, 1, 2, 3], [3, 2, 1, 0], [1, 3, 0, 2]] {
            let raw: Vec<_> = order.iter().map(|&i| sources[i]).collect();
            let walls = built_sources(&raw);
            assert_complete_partition(&raw, &walls);
            for w in &walls {
                closed_outward_volume(&w.mesh);
            }
        }
        sources[3].thickness = 180.0;
        for order in [[0, 1, 2, 3], [3, 2, 1, 0], [1, 3, 0, 2]] {
            let raw: Vec<_> = order.iter().map(|&i| sources[i]).collect();
            let walls = built_sources(&raw);
            assert_complete_partition(&raw, &walls);
            for w in &walls {
                closed_outward_volume(&w.mesh);
            }
        }
        sources[3] = Axis { a: [3500.0, 5600.0], b: [5000.0, 5000.0], thickness: 120.0 };
        let walls = built_sources(&sources);
        assert_complete_partition(&sources, &walls);
        for w in &walls {
            closed_outward_volume(&w.mesh);
        }
    }

    #[test]
    fn stepped_profiles_retain_doors_including_a_door_across_the_shoulder() {
        let mut sources = stepped_sources();
        sources[2].a = [3200.0, 5000.0];
        for reversed in [false, true] {
            let sources = sources.map(|w| if reversed { Axis { a: w.b, b: w.a, ..w } } else { w });
            for at in [700.0, 1650.0] {
                let mut doc = source_document(&sources);
                let at = if reversed { 1800.0 - at } else { at };
                doc.buildings[0].interior_walls[2].doors.push(crate::WallDoor { at, width: 300.0, height: 2100.0 });
                let before = serde_json::to_vec(&doc).unwrap();
                let built = crate::build(&doc).unwrap();
                assert_eq!(serde_json::to_vec(&doc).unwrap(), before);
                let wall = &built[0].interior[2];
                assert_eq!(wall.doors.len(), 1);
                let full = built_sources(&sources);
                let area = |w: &crate::build::BuiltInterior| w.cut.iter().map(|r| crate::footprint::signed_area2(r) / 2.0).sum::<f64>();
                let gross = area(&full[2]);
                let opening = gross - area(wall);
                assert!(opening > 20_000.0, "the intended doorway must remain open");
                let expected = gross * wall.height - opening * 2100.0;
                let volume = closed_outward_volume(&wall.mesh);
                assert!((volume - expected).abs() < 1.0, "door volume: {volume} vs {expected}");
                // The same profile must remain closed when a 3D plan cut opens the door
                // through the top of the wall.
                let p = profiles::join(&sources, &[], 0.0);
                let material = p[2].material.as_ref().unwrap();
                let holes: Vec<_> = wall.doors.iter().map(|d| d.rect).collect();
                let short = profiles::mesh(material, &wall.frame, &holes, wall.thickness, wall.z_bottom, 1500.0).unwrap();
                let volume = closed_outward_volume(&short);
                assert!((volume - (gross - opening) * 1500.0).abs() < 1.0);
            }
        }
    }

    #[test]
    fn profile_path_preserves_independent_corners_gaps_near_collinear_and_reach() {
        let sources = stepped_sources();
        let mut gap = sources;
        gap[0].a[1] += 900.0;
        let mut nearby = sources;
        nearby[0].a[1] += 0.01;
        let mut almost_parallel = sources;
        almost_parallel[2].a = [4900.0, 3500.0];
        let mut beyond_reach = sources;
        beyond_reach[2].b = [4500.0, 4500.0];
        let mut equal_width = sources;
        equal_width[0].thickness = equal_width[1].thickness;
        for raw in [&sources[..2], &[sources[0], sources[2]], &gap, &nearby, &almost_parallel, &beyond_reach, &equal_width] {
            let legacy = join(raw, &[], 0.0);
            let profiled = profiles::join(raw, &[], 0.0);
            assert!(profiled.iter().all(|p| p.material.is_none()), "only exact supported shared stations use profiles");
            assert_eq!(legacy, profiled.into_iter().map(|p| p.joined).collect::<Vec<_>>());
        }
        let mut hostile = sources.to_vec();
        hostile.extend([
            Axis { a: sources[0].a, b: [f64::NAN, 0.0], thickness: 120.0 },
            Axis { a: sources[0].a, b: [5000.0, 6000.0], thickness: f64::INFINITY },
            Axis { a: sources[0].a, b: sources[0].a, thickness: 120.0 },
        ]);
        assert_eq!(profiles::join(&hostile, &[], 0.0).len(), hostile.len());
    }

    #[test]
    fn an_oblique_face_cuts_the_end_on_the_face_line() {
        // A facade edge at 45°; a wall running into it.
        let tri = vec![[0.0, 0.0], [10000.0, 0.0], [0.0, 10000.0]];
        let w = Axis { a: [2000.0, 2000.0], b: [2000.0, 7900.0], thickness: 200.0 };
        let j = &join(&[w], &tri, 400.0)[0];
        // Both corners of the end lie on x + y = 10000.
        for p in [j.outline[1], j.outline[2]] {
            assert!((p[0] + p[1] - 10000.0).abs() < 1e-6, "{p:?}");
        }
        assert!(close(j.b, [2000.0, 8000.0]));
    }

    #[test]
    fn an_end_drawn_out_past_the_facade_comes_back_to_its_face() {
        // Drawn 900 mm past the inner face (out to where a bay used to be): more than the band.
        let w = Axis { a: [5000.0, 4000.0], b: [5000.0, 8900.0], thickness: 120.0 };
        let j = &join(&[w], &facade(), 600.0)[0];
        assert!(close(j.b, [5000.0, 8000.0]), "{:?}", j.b);
    }

    #[test]
    fn degenerate_and_hostile_walls_never_panic() {
        let walls = [
            Axis { a: [0.0, 0.0], b: [0.0, 0.0], thickness: 100.0 },
            Axis { a: [f64::NAN, 0.0], b: [1.0, 1.0], thickness: 100.0 },
            Axis { a: [1e9, 1e9], b: [-1e9, 1e9], thickness: f64::INFINITY },
            Axis { a: [100.0, 100.0], b: [200.0, 100.0], thickness: 5000.0 },
        ];
        let out = join(&walls, &facade(), 400.0);
        assert_eq!(out.len(), walls.len());
        let _ = join(&walls, &[], 400.0);
    }
}
