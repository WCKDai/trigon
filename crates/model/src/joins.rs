//! Wall joins in plan. Where an interior wall meets the facade or another wall, its end is
//! trimmed or extended so the two meet face to face: a butt join on the near face (a T), and at a
//! corner of two walls the thicker one runs through to the other's far face (an L). The facade
//! walls are mitred at their corners (`build::inner_ring`), so after joining every wall end on a
//! storey either sits on a face or is free. Walls closer than 20° to parallel never join end to
//! face (collinear pieces either side of a door stay apart), and an end moves at most `REACH`.

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
