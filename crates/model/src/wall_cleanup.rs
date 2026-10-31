//! Conservative, explicit regularization of measured interior wall centre lines.

use crate::{InteriorWall, ModelError, Result, checked};

const MAX_WALLS: usize = 5000;
const EPS: f64 = 1e-7;
const DOOR_MIN: f64 = 500.0;
const DOOR_MAX: f64 = 2000.0;

fn dot(a: [f64; 2], b: [f64; 2]) -> f64 {
    a[0] * b[0] + a[1] * b[1]
}

fn sub(a: [f64; 2], b: [f64; 2]) -> [f64; 2] {
    [a[0] - b[0], a[1] - b[1]]
}

fn cross(a: [f64; 2], b: [f64; 2]) -> f64 {
    a[0] * b[1] - a[1] * b[0]
}

fn distance(a: [f64; 2], b: [f64; 2]) -> f64 {
    (a[0] - b[0]).hypot(a[1] - b[1])
}

fn inside_or_on(poly: &[[f64; 2]], p: [f64; 2]) -> bool {
    if poly.is_empty() {
        return true;
    }
    let mut inside = false;
    for (i, &a) in poly.iter().enumerate() {
        let b = poly[(i + 1) % poly.len()];
        let d = sub(b, a);
        let length2 = dot(d, d);
        if length2 > EPS {
            let t = (dot(sub(p, a), d) / length2).clamp(0.0, 1.0);
            if distance(p, [a[0] + t * d[0], a[1] + t * d[1]]) <= EPS {
                return true;
            }
        }
        if (a[1] > p[1]) != (b[1] > p[1]) && p[0] < a[0] + (p[1] - a[1]) * (b[0] - a[0]) / (b[1] - a[1]) {
            inside = !inside;
        }
    }
    inside
}

#[derive(Clone)]
struct Work {
    wall: InteriorWall,
    order: usize,
    levels: Vec<usize>,
    axis: Option<usize>,
    u: [f64; 2],
    v: [f64; 2],
    lo: f64,
    hi: f64,
    offset: f64,
    reversed: bool,
    original: [[f64; 2]; 2],
    group: usize,
}

impl Work {
    fn at(&self, along: f64, offset: f64) -> [f64; 2] {
        [self.u[0] * along + self.v[0] * offset, self.u[1] * along + self.v[1] * offset]
    }

    fn ends(&self, offset: f64) -> [[f64; 2]; 2] {
        let ends = [self.at(self.lo, offset), self.at(self.hi, offset)];
        if self.reversed { [ends[1], ends[0]] } else { ends }
    }

    fn can_align(&self, offset: f64, budget: f64, inner: &[[f64; 2]]) -> bool {
        self.ends(offset).iter().zip(self.original).all(|(&p, q)| distance(p, q) <= budget + EPS && inside_or_on(inner, p))
    }

    fn align(&mut self, offset: f64) {
        self.offset = offset;
        let [a, b] = self.ends(offset);
        // Keep already-canonical coordinates exact on repeated calls.
        if distance(a, self.wall.a) > EPS {
            self.wall.a = a;
        }
        if distance(b, self.wall.b) > EPS {
            self.wall.b = b;
        }
    }
}

struct Group {
    anchor: usize,
    lo: f64,
    hi: f64,
    thickness: f64,
}

fn interval_gap(a0: f64, a1: f64, b0: f64, b1: f64) -> f64 {
    (a0 - b1).max(b0 - a1).max(0.0)
}

/// Align measured wall fragments with supplied building grids without filling door openings.
/// Angles are degrees modulo 90; all distances are millimetres. A merged wall retains the first
/// input wall's id, thickness and level list. Uncertain short or off-grid walls are retained.
///
/// Normal movement uses at most 75% of `tolerance_mm`; end extensions use at most 50%, capped
/// at 150 mm. Separate walls only co-align within a quarter of their smaller thickness (75 mm
/// maximum). Merging fills at most 100 mm, and never joins different level sets or wall kinds.
/// This is a geometric cleanup, not evidence that a missing wall or opening exists.
pub fn regularize(
    walls: &[InteriorWall],
    axes_deg: &[f64],
    tolerance_mm: f64,
    angle_tolerance_deg: f64,
    inner: &[[f64; 2]],
) -> Result<Vec<InteriorWall>> {
    checked("wall cleanup tolerance", tolerance_mm, 0.0, 300.0)?;
    checked("wall cleanup angle tolerance", angle_tolerance_deg, 0.0, 10.0)?;
    if walls.len() > MAX_WALLS || axes_deg.is_empty() || axes_deg.len() > 32 || axes_deg.iter().any(|a| !a.is_finite()) {
        return Err(ModelError::Invalid("wall cleanup needs at most 5000 walls and 1 to 32 finite grid angles".into()));
    }
    if inner.len() > 1024 || (!inner.is_empty() && inner.len() < 3) || inner.iter().flatten().any(|v| !v.is_finite() || v.abs() > 1e9) {
        return Err(ModelError::Invalid("wall cleanup boundary needs 3 to 1024 finite corners, or no boundary".into()));
    }
    for w in walls {
        if w.a.iter().chain(&w.b).any(|v| !v.is_finite() || v.abs() > 1e9) || distance(w.a, w.b) < 1.0 || w.levels.is_empty() || w.levels.len() > 256
        {
            return Err(ModelError::Invalid("wall cleanup needs finite nonzero wall segments and 1 to 256 storeys per wall".into()));
        }
        checked("wall thickness", w.thickness, 1.0, 5000.0)?;
    }
    if tolerance_mm == 0.0 {
        return Ok(walls.to_vec());
    }
    let normal_budget = tolerance_mm * 0.75;
    let end_budget = (tolerance_mm * 0.5).min(150.0);
    let merge_gap = end_budget.min(100.0);
    let mut angles: Vec<f64> = axes_deg.iter().flat_map(|a| [a.rem_euclid(90.0), a.rem_euclid(90.0) + 90.0]).collect();
    angles.sort_by(f64::total_cmp);
    angles.dedup_by(|a, b| (*a - *b).abs() < EPS);
    let axes: Vec<[f64; 2]> = angles.iter().map(|a| [a.to_radians().cos(), a.to_radians().sin()]).collect();
    let mut work = Vec::with_capacity(walls.len());
    for (order, wall) in walls.iter().enumerate() {
        let delta = sub(wall.b, wall.a);
        let length = distance(wall.a, wall.b);
        let direction = [delta[0] / length, delta[1] / length];
        let nearest = axes
            .iter()
            .enumerate()
            .map(|(i, u)| (i, dot(direction, *u).abs().clamp(0.0, 1.0).acos().to_degrees()))
            .min_by(|a, b| a.1.total_cmp(&b.1));
        let axis = nearest.filter(|(_, angle)| *angle <= angle_tolerance_deg + EPS).map(|(i, _)| i);
        let u = axis.and_then(|i| axes.get(i).copied()).unwrap_or(direction);
        let v = [-u[1], u[0]];
        let (pa, pb) = (dot(wall.a, u), dot(wall.b, u));
        let mut levels = wall.levels.clone();
        levels.sort_unstable();
        levels.dedup();
        let mut w = Work {
            wall: wall.clone(),
            order,
            levels,
            axis,
            u,
            v,
            lo: pa.min(pb),
            hi: pa.max(pb),
            offset: (dot(wall.a, v) + dot(wall.b, v)) / 2.0,
            reversed: pa > pb,
            original: [wall.a, wall.b],
            group: order,
        };
        if w.axis.is_some() && w.can_align(w.offset, normal_budget, inner) {
            w.align(w.offset);
        } else {
            w.axis = None;
        }
        work.push(w);
    }

    // The longest measured fragment anchors each line. Comparing every candidate to this fixed
    // anchor prevents a chain of small offsets from walking a line across a room.
    let mut order: Vec<usize> = (0..work.len()).collect();
    order.sort_by(|&a, &b| (work[b].hi - work[b].lo).total_cmp(&(work[a].hi - work[a].lo)).then(a.cmp(&b)));
    let mut groups: Vec<Group> = Vec::new();
    for i in order {
        let w = &work[i];
        let best = groups
            .iter()
            .enumerate()
            .filter_map(|(gi, group)| {
                let anchor = &work[group.anchor];
                let limit = normal_budget.min(75.0).min(w.wall.thickness.min(group.thickness) * 0.25);
                let offset = (w.offset - anchor.offset).abs();
                (w.axis.is_some()
                    && w.axis == anchor.axis
                    && w.levels == anchor.levels
                    && offset <= limit + EPS
                    && interval_gap(w.lo, w.hi, group.lo, group.hi) <= DOOR_MAX
                    && w.can_align(anchor.offset, normal_budget, inner))
                .then_some((gi, offset))
            })
            .min_by(|a, b| a.1.total_cmp(&b.1).then(a.0.cmp(&b.0)));
        if let Some((gi, _)) = best {
            let offset = work[groups[gi].anchor].offset;
            work[i].align(offset);
            work[i].group = gi;
            groups[gi].lo = groups[gi].lo.min(work[i].lo);
            groups[gi].hi = groups[gi].hi.max(work[i].hi);
            groups[gi].thickness = groups[gi].thickness.min(work[i].wall.thickness);
        } else {
            work[i].group = groups.len();
            groups.push(Group { anchor: i, lo: work[i].lo, hi: work[i].hi, thickness: work[i].wall.thickness });
        }
    }

    work.sort_by(|a, b| {
        a.group
            .cmp(&b.group)
            .then(a.wall.load_bearing.cmp(&b.wall.load_bearing))
            .then(a.lo.total_cmp(&b.lo))
            .then(a.hi.total_cmp(&b.hi))
            .then(a.order.cmp(&b.order))
    });
    let mut merged: Vec<Work> = Vec::with_capacity(work.len());
    for w in work {
        if let Some(previous) = merged.last_mut() {
            let thickness_limit = 10.0f64.max(previous.wall.thickness.min(w.wall.thickness) * 0.2);
            let gap_midpoint = previous.at((previous.hi + w.lo) / 2.0, previous.offset);
            if w.axis.is_some()
                && previous.group == w.group
                && previous.wall.load_bearing == w.wall.load_bearing
                && (previous.wall.thickness - w.wall.thickness).abs() <= thickness_limit
                && w.lo <= previous.hi + merge_gap + EPS
                && (w.lo <= previous.hi || inside_or_on(inner, gap_midpoint))
            {
                let (lo, hi) = (previous.lo.min(w.lo), previous.hi.max(w.hi));
                if w.order < previous.order {
                    *previous = w;
                }
                previous.lo = lo;
                previous.hi = hi;
                previous.align(previous.offset);
                continue;
            }
        }
        merged.push(w);
    }
    merged.sort_by_key(|w| w.order);

    // A clear gap between collinear fragments is evidence of an opening. In particular, a nearby
    // crossing wall must not pull one jamb into a 500–2000 mm door opening.
    let mut protected = vec![[false; 2]; merged.len()];
    for (i, a) in merged.iter().enumerate() {
        if a.axis.is_none() {
            continue;
        }
        for (j, b) in merged.iter().enumerate() {
            if i == j || a.axis != b.axis || a.levels != b.levels || (a.offset - b.offset).abs() > 25.0 {
                continue;
            }
            let low_gap = a.lo - b.hi;
            let high_gap = b.lo - a.hi;
            if (DOOR_MIN - EPS..=DOOR_MAX).contains(&low_gap) {
                protected[i][usize::from(a.reversed)] = true;
            }
            if (DOOR_MIN - EPS..=DOOR_MAX).contains(&high_gap) {
                protected[i][usize::from(!a.reversed)] = true;
            }
        }
    }
    let snapshot: Vec<InteriorWall> = merged.iter().map(|w| w.wall.clone()).collect();
    let level_sets: Vec<Vec<usize>> = merged.iter().map(|w| w.levels.clone()).collect();
    for (i, w) in merged.iter_mut().enumerate() {
        if w.axis.is_none() {
            continue;
        }
        let source = &snapshot[i];
        for end in 0..2 {
            if protected[i][end] {
                continue;
            }
            let (p, other) = if end == 0 { (source.a, source.b) } else { (source.b, source.a) };
            let length = distance(p, other);
            let d = [(p[0] - other[0]) / length, (p[1] - other[1]) / length];
            let mut best: Option<(f64, [f64; 2])> = None;
            let mut consider = |a: [f64; 2], b: [f64; 2], half: f64, target_protected: Option<[bool; 2]>| {
                let e = sub(b, a);
                let el = distance(a, b);
                let den = cross(d, e);
                if el < 1.0 || den.abs() / el < 0.5 {
                    return;
                }
                let t = cross(sub(a, p), e) / den;
                let fraction = cross(sub(a, p), d) / den;
                if t <= EPS {
                    return;
                }
                let extension = if let Some(locked) = target_protected {
                    if fraction <= EPS || fraction >= 1.0 - EPS {
                        let te = usize::from(fraction >= 1.0 - EPS);
                        let beyond = if te == 0 { -fraction * el } else { (fraction - 1.0) * el };
                        // L: both endpoints can reach their line intersection, without shortening
                        // either wall or extrapolating a target beyond its own movement budget.
                        if locked[te] || beyond > end_budget + EPS {
                            return;
                        }
                        t
                    } else {
                        // T: stop at the near face of a target that continues past the junction.
                        t - half / (den.abs() / el)
                    }
                } else {
                    if !(0.0..=1.0).contains(&fraction) {
                        return;
                    }
                    t
                };
                if extension <= EPS || extension > end_budget + EPS {
                    return;
                }
                let q = [p[0] + extension * d[0], p[1] + extension * d[1]];
                if inside_or_on(inner, q) && best.is_none_or(|(bt, _)| extension < bt) {
                    best = Some((extension, q));
                }
            };
            for (j, target) in snapshot.iter().enumerate() {
                if i != j && w.levels == level_sets[j] {
                    consider(target.a, target.b, target.thickness / 2.0, Some(protected[j]));
                }
            }
            for (j, &a) in inner.iter().enumerate() {
                consider(a, inner[(j + 1) % inner.len()], 0.0, None);
            }
            if let Some((_, q)) = best {
                if end == 0 {
                    w.wall.a = q;
                } else {
                    w.wall.b = q;
                }
            }
        }
    }
    Ok(merged.into_iter().map(|w| w.wall).collect())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn wall(id: &str, a: [f64; 2], b: [f64; 2]) -> InteriorWall {
        InteriorWall { id: id.into(), levels: vec![1], a, b, thickness: 100.0, load_bearing: false, doors: Vec::new() }
    }

    fn clean(w: &[InteriorWall]) -> Vec<InteriorWall> {
        regularize(w, &[0.0], 100.0, 3.0, &[]).unwrap()
    }

    fn close(a: f64, b: f64) {
        assert!((a - b).abs() < 1e-7, "{a} != {b}");
    }

    #[test]
    fn measured_walls_follow_the_grid_with_bounded_movement() {
        let input = wall("a", [0.0, 0.0], [4000.0, 80.0]);
        let out = clean(std::slice::from_ref(&input));
        close(out[0].a[1], 40.0);
        close(out[0].b[1], 40.0);
        close(out[0].a[0], input.a[0]);
        close(out[0].b[0], input.b[0]);
        for (p, q) in [(out[0].a, input.a), (out[0].b, input.b)] {
            assert!((p[0] - q[0]).hypot(p[1] - q[1]) <= 100.0);
        }
        let long = wall("long", [0.0, 0.0], [20_000.0, 800.0]);
        assert_eq!(clean(std::slice::from_ref(&long)), vec![long]);
    }

    #[test]
    fn collinear_fragments_align_across_a_door_without_closing_it() {
        let out = clean(&[wall("a", [0.0, 0.0], [1500.0, 20.0]), wall("b", [2300.0, 40.0], [4000.0, 20.0])]);
        assert_eq!(out.len(), 2);
        close(out[0].a[1], out[1].a[1]);
        close(out[0].a[1], out[0].b[1]);
        close(out[1].b[1], out[1].a[1]);
        close(out[1].a[0] - out[0].b[0], 800.0);
    }

    #[test]
    fn only_overlaps_and_small_scan_breaks_merge() {
        let mut b = wall("duplicate", [1000.0, 15.0], [3000.0, 15.0]);
        b.thickness = 105.0;
        let out = clean(&[wall("retained", [0.0, 0.0], [2000.0, 0.0]), b, wall("tail", [3040.0, 5.0], [4000.0, 5.0])]);
        assert_eq!(out.len(), 1);
        assert_eq!(out[0].id, "retained");
        close(out[0].thickness, 100.0);
        close(out[0].a[0], 0.0);
        close(out[0].b[0], 4000.0);
    }

    #[test]
    fn nearby_parallel_partitions_are_not_duplicates() {
        let input = vec![wall("a", [0.0, 0.0], [3000.0, 0.0]), wall("b", [0.0, 80.0], [3000.0, 80.0])];
        assert_eq!(clean(&input), input);
    }

    #[test]
    fn independent_storeys_do_not_align_merge_or_supply_junctions() {
        let a = wall("a", [0.0, 0.0], [1000.0, 0.0]);
        let mut b = wall("b", [0.0, 15.0], [1000.0, 15.0]);
        b.levels = vec![2];
        let mut c = wall("c", [1080.0, -500.0], [1080.0, 500.0]);
        c.levels = vec![3];
        let out = clean(&[a.clone(), b.clone(), c]);
        assert_eq!(out.len(), 3);
        assert_eq!(out[0], a);
        assert_eq!(out[1], b);
    }

    #[test]
    fn t_junctions_meet_the_face_and_l_junctions_meet_at_the_corner() {
        let t = clean(&[wall("spine", [0.0, 0.0], [2000.0, 0.0]), wall("branch", [1000.0, 500.0], [1000.0, 80.0])]);
        close(t[1].b[1], 50.0);
        let l = clean(&[wall("a", [0.0, 0.0], [960.0, 0.0]), wall("b", [1000.0, 40.0], [1000.0, 1000.0])]);
        close(l[0].b[0], 1000.0);
        close(l[1].a[1], 0.0);
    }

    #[test]
    fn a_five_hundred_mm_door_is_not_reduced_by_a_nearby_junction() {
        let input = vec![
            wall("left", [0.0, 0.0], [1000.0, 0.0]),
            wall("right", [1500.0, 0.0], [2500.0, 0.0]),
            wall("cross", [1080.0, -500.0], [1080.0, 500.0]),
        ];
        let out = clean(&input);
        close(out[0].b[0], 1000.0);
        close(out[1].a[0], 1500.0);
    }

    #[test]
    fn outer_faces_close_only_small_gaps_and_repeated_cleanup_is_stable() {
        let inner = [[-200.0, -200.0], [1200.0, -200.0], [1200.0, 1200.0], [-200.0, 1200.0]];
        let input = vec![wall("edge", [0.0, 0.0], [1140.0, 0.0]), wall("branch", [700.0, 700.0], [710.0, 80.0])];
        let first = regularize(&input, &[0.0], 200.0, 3.0, &inner).unwrap();
        close(first[0].b[0], 1200.0);
        let again = regularize(&first, &[0.0], 200.0, 3.0, &inner).unwrap();
        assert_eq!(first.len(), again.len());
        for (a, b) in first.iter().zip(&again) {
            assert_eq!(a.id, b.id);
            for (p, q) in [(a.a, b.a), (a.b, b.b)] {
                close(p[0], q[0]);
                close(p[1], q[1]);
            }
        }
    }

    #[test]
    fn hostile_and_unbounded_inputs_are_refused() {
        let w = wall("a", [0.0, 0.0], [1000.0, 0.0]);
        assert!(regularize(std::slice::from_ref(&w), &[f64::NAN], 100.0, 3.0, &[]).is_err());
        assert!(regularize(std::slice::from_ref(&w), &[0.0], f64::INFINITY, 3.0, &[]).is_err());
        assert!(regularize(std::slice::from_ref(&w), &[0.0], 100.0, -1.0, &[]).is_err());
        assert!(regularize(&vec![w; 5001], &[0.0], 100.0, 3.0, &[]).is_err());
        assert!(clean_input_with_nan().is_err());
    }

    fn clean_input_with_nan() -> Result<Vec<InteriorWall>> {
        regularize(&[wall("bad", [f64::NAN, 0.0], [1000.0, 0.0])], &[0.0], 100.0, 3.0, &[])
    }
}
