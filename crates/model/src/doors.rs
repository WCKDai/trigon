//! Doors from gaps. Plans traced from archive drawings show a door as a gap in a wall. Two
//! collinear pieces of one wall either side of a door-sized gap become one wall with a door in
//! it, and a wall that stops a door's width short of another wall runs on to that wall with the
//! door at its end. A gap that another wall crosses is a junction, not a door, and stays.

use crate::doc::{InteriorWall, WallDoor};

/// What counts as a door.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Rules {
    /// Narrowest and widest gap taken for a door, mm.
    pub min: f64,
    pub max: f64,
    /// Height of the doors made, mm.
    pub height: f64,
}

/// How many doors were made: in gaps between two pieces of a wall, and at wall ends.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct Found {
    pub between: usize,
    pub at_ends: usize,
}

/// sin 2°: pieces of one wall are this close to parallel.
const PARALLEL: f64 = 0.035;
/// Pieces of one wall lie this close to one line, mm.
const IN_LINE: f64 = 40.0;
/// Pieces of one wall differ this little in thickness, mm.
const SAME: f64 = 40.0;
/// sin 20°: an end meets another wall only if it isn't nearly parallel.
const MIN_SIN: f64 = 0.34;
/// An end this close to another wall's face is a joint (see `joins`), not a door, mm.
const CLEAR: f64 = crate::joins::REACH;
const MAX_WALLS: usize = 5000;

fn sub(a: [f64; 2], b: [f64; 2]) -> [f64; 2] {
    [a[0] - b[0], a[1] - b[1]]
}

fn dot(a: [f64; 2], b: [f64; 2]) -> f64 {
    a[0] * b[0] + a[1] * b[1]
}

fn cross(a: [f64; 2], b: [f64; 2]) -> f64 {
    a[0] * b[1] - a[1] * b[0]
}

fn along(p: [f64; 2], t: [f64; 2], s: f64) -> [f64; 2] {
    [p[0] + t[0] * s, p[1] + t[1] * s]
}

/// Unit direction and length of a wall's axis.
fn axis(w: &InteriorWall) -> Option<([f64; 2], f64)> {
    let d = sub(w.b, w.a);
    let len = dot(d, d).sqrt();
    (len > 1.0 && len.is_finite()).then(|| ([d[0] / len, d[1] / len], len))
}

/// A rectangle along the line `p + s·t` from `s0` to `s1`, `h` either side.
fn band(p: [f64; 2], t: [f64; 2], (s0, s1): (f64, f64), h: f64) -> [[f64; 2]; 4] {
    let n = [-t[1], t[0]];
    let (a, b) = (along(p, t, s0), along(p, t, s1));
    [along(a, n, -h), along(b, n, -h), along(b, n, h), along(a, n, h)]
}

/// Whether two convex quadrilaterals overlap (separating axes), ignoring mere touching.
fn overlap(a: &[[f64; 2]; 4], b: &[[f64; 2]; 4]) -> bool {
    for poly in [a, b] {
        for i in 0..4 {
            let e = sub(poly[(i + 1) % 4], poly[i]);
            let n = [-e[1], e[0]];
            let range = |q: &[[f64; 2]; 4]| q.iter().map(|p| dot(*p, n)).fold((f64::MAX, f64::MIN), |(lo, hi), v| (lo.min(v), hi.max(v)));
            let ((a0, a1), (b0, b1)) = (range(a), range(b));
            let slack = 1.0 * dot(n, n).sqrt();
            if a1 <= b0 + slack || b1 <= a0 + slack {
                return false;
            }
        }
    }
    true
}

fn shares_level(a: &InteriorWall, b: &InteriorWall) -> bool {
    a.levels.iter().any(|l| b.levels.contains(l))
}

/// Whether any wall other than `skip` (on a shared storey with `w`) crosses the region `r`.
fn crossed(walls: &[InteriorWall], w: &InteriorWall, r: &[[f64; 2]; 4], skip: &[usize]) -> bool {
    walls.iter().enumerate().any(|(k, o)| {
        if skip.contains(&k) || !shares_level(w, o) {
            return false;
        }
        let Some((t, len)) = axis(o) else { return false };
        overlap(r, &band(o.a, t, (0.0, len), o.thickness / 2.0))
    })
}

/// Turns door-sized gaps into doors (see the module docs). Only walls standing on at least one of
/// `levels` are touched (all walls when `None`). Returns how many doors were made.
pub fn doors_from_gaps(walls: &mut Vec<InteriorWall>, levels: Option<&[usize]>, rules: &Rules) -> Found {
    let mut found = Found::default();
    if walls.len() > MAX_WALLS || !(rules.min > 0.0 && rules.max >= rules.min && rules.max.is_finite() && rules.height.is_finite()) {
        return found;
    }
    let wanted = |w: &InteriorWall| levels.is_none_or(|ls| w.levels.iter().any(|l| ls.contains(l)));
    // Pieces of one wall either side of a door: merge them, again and again (a wall with
    // several doors is several pieces).
    'merging: for _ in 0..walls.len() {
        for i in 0..walls.len() {
            let w = &walls[i];
            let Some((t, len)) = axis(w) else { continue };
            if !wanted(w) {
                continue;
            }
            let n = [-t[1], t[0]];
            for j in 0..walls.len() {
                let o = &walls[j];
                if j == i || !wanted(o) {
                    continue;
                }
                let Some((to, _)) = axis(o) else { continue };
                let mut lv = (w.levels.clone(), o.levels.clone());
                lv.0.sort_unstable();
                lv.1.sort_unstable();
                if lv.0 != lv.1 || cross(t, to).abs() > PARALLEL || (w.thickness - o.thickness).abs() > SAME {
                    continue;
                }
                if [o.a, o.b].iter().any(|p| dot(sub(*p, w.a), n).abs() > IN_LINE) {
                    continue;
                }
                // Only a piece beyond `w`'s end `b` (the other side is found from the other piece).
                let (sa, sb) = (dot(sub(o.a, w.a), t), dot(sub(o.b, w.a), t));
                let (near, far) = (sa.min(sb), sa.max(sb));
                let gap = near - len;
                if gap < rules.min || gap > rules.max {
                    continue;
                }
                let h = w.thickness.max(o.thickness) / 2.0;
                if crossed(walls, w, &band(w.a, t, (len, near), h - 1.0), &[i, j]) {
                    continue;
                }
                // `o`'s doors, measured along `w`'s line.
                let flipped = sb < sa;
                let moved: Vec<WallDoor> =
                    o.doors.iter().map(|d| WallDoor { at: if flipped { sa - d.at } else { sa + d.at }, width: d.width, height: d.height }).collect();
                let mut merged = w.clone();
                merged.b = along(w.a, t, far);
                merged.thickness = if len >= far - near { w.thickness } else { o.thickness };
                merged.load_bearing |= o.load_bearing;
                merged.doors.push(WallDoor { at: len + gap / 2.0, width: gap, height: rules.height });
                merged.doors.extend(moved);
                merged.doors.sort_by(|x, y| x.at.total_cmp(&y.at));
                walls[i] = merged;
                walls.remove(j);
                found.between += 1;
                continue 'merging;
            }
        }
        break;
    }
    // Walls that stop a door's width short of another wall: run on to it, door at the end.
    for i in 0..walls.len() {
        if !wanted(&walls[i]) {
            continue;
        }
        for end in 0..2 {
            let w = &walls[i];
            let Some((t, len)) = axis(w) else { break };
            let (from, out) = if end == 0 { (w.a, [-t[0], -t[1]]) } else { (w.b, t) };
            let h = w.thickness / 2.0;
            // The nearest face ahead, of any wall on a shared storey.
            let mut nearest: Option<(f64, usize)> = None;
            for (k, o) in walls.iter().enumerate() {
                if k == i || !shares_level(w, o) {
                    continue;
                }
                let Some((to, lo)) = axis(o) else { continue };
                let sin = cross(out, to).abs();
                if sin < MIN_SIN {
                    continue;
                }
                let no = [-to[1], to[0]];
                // Distance along `out` from `from` to each side line of `o`.
                let hit = |off: f64| -> Option<f64> {
                    let q = along(o.a, no, off);
                    let den = cross(out, to);
                    (den.abs() > 1e-12).then(|| cross(sub(q, from), to) / den)
                };
                let (Some(g1), Some(g2)) = (hit(o.thickness / 2.0), hit(-o.thickness / 2.0)) else { continue };
                let g = g1.min(g2);
                if g < -h || !g.is_finite() {
                    continue;
                }
                // Where the axis meets `o`, along `o`: well inside it, not at its end.
                let r = dot(sub(along(from, out, g), o.a), to);
                let margin = h / sin;
                if r < -margin || r > lo + margin {
                    continue;
                }
                if nearest.is_none_or(|(best, _)| g < best) {
                    nearest = Some((g, k));
                }
            }
            let Some((g, k)) = nearest else { continue };
            let target = &walls[k];
            let inside = axis(target).is_some_and(|(to, lo)| {
                let r = dot(sub(along(from, out, g), target.a), to);
                let margin = h / cross(out, to).abs().max(MIN_SIN) + 50.0;
                r > margin && r < lo - margin
            });
            let covers = w.levels.iter().all(|l| target.levels.contains(l));
            if g < CLEAR.max(rules.min) || g > rules.max || !inside || !covers {
                continue;
            }
            let (s0, s1) = if end == 0 { (-g, 0.0) } else { (len, len + g) };
            if crossed(walls, w, &band(w.a, t, (s0, s1), h - 1.0), &[i, k]) {
                continue;
            }
            let mut longer = w.clone();
            if end == 0 {
                longer.a = along(w.a, t, -g);
                for d in &mut longer.doors {
                    d.at += g;
                }
                longer.doors.insert(0, WallDoor { at: g / 2.0, width: g, height: rules.height });
            } else {
                longer.b = along(w.a, t, len + g);
                longer.doors.push(WallDoor { at: len + g / 2.0, width: g, height: rules.height });
            }
            walls[i] = longer;
            found.at_ends += 1;
        }
    }
    found
}

#[cfg(test)]
mod tests {
    use super::*;

    fn wall(id: &str, a: [f64; 2], b: [f64; 2], thickness: f64) -> InteriorWall {
        InteriorWall { id: id.into(), levels: vec![1, 2], a, b, thickness, load_bearing: false, doors: Vec::new() }
    }

    const RULES: Rules = Rules { min: 600.0, max: 1300.0, height: 2100.0 };

    #[test]
    fn pieces_either_side_of_a_gap_become_one_wall_with_a_door() {
        // Three pieces, two 800 mm gaps; the last piece drawn backwards.
        let mut walls = vec![
            wall("a", [0.0, 0.0], [3000.0, 0.0], 120.0),
            wall("b", [3800.0, 0.0], [6000.0, 10.0], 110.0),
            wall("c", [9000.0, 0.0], [6800.0, 0.0], 120.0),
        ];
        let found = doors_from_gaps(&mut walls, None, &RULES);
        assert_eq!(found, Found { between: 2, at_ends: 0 });
        assert_eq!(walls.len(), 1);
        let w = &walls[0];
        assert!((w.b[0] - 9000.0).abs() < 1e-6, "{:?}", w.b);
        let doors: Vec<(f64, f64)> = w.doors.iter().map(|d| (d.at, d.width)).collect();
        assert_eq!(doors.len(), 2);
        assert!((doors[0].0 - 3400.0).abs() < 1.0 && (doors[0].1 - 800.0).abs() < 1.0, "{doors:?}");
        assert!((doors[1].0 - 6400.0).abs() < 15.0 && (doors[1].1 - 800.0).abs() < 15.0, "{doors:?}");
        assert!(w.validate(3).is_ok());
    }

    #[test]
    fn a_gap_crossed_by_a_wall_and_a_gap_too_wide_stay_open() {
        let mut walls = vec![
            wall("a", [0.0, 0.0], [3000.0, 0.0], 120.0),
            wall("b", [3800.0, 0.0], [6000.0, 0.0], 120.0),
            // A wall running through the first gap.
            wall("x", [3400.0, -2000.0], [3400.0, 2000.0], 120.0),
            wall("c", [8000.0, 0.0], [9000.0, 0.0], 120.0),
        ];
        let found = doors_from_gaps(&mut walls, None, &RULES);
        assert_eq!(found.between, 0);
        assert_eq!(walls.len(), 4);
    }

    #[test]
    fn a_wall_stopping_a_door_short_of_another_runs_on_to_it() {
        // A 240 mm wall along y = 0; a partition from y = 4000 down to 900 mm above its face.
        let mut walls = vec![wall("main", [-3000.0, 0.0], [3000.0, 0.0], 240.0), wall("p", [0.0, 4000.0], [0.0, 1020.0], 100.0)];
        let found = doors_from_gaps(&mut walls, None, &RULES);
        assert_eq!(found, Found { between: 0, at_ends: 1 });
        let p = &walls[1];
        assert!((p.b[1] - 120.0).abs() < 1e-6, "{:?}", p.b);
        assert_eq!(p.doors.len(), 1);
        assert!((p.doors[0].width - 900.0).abs() < 1e-6 && (p.doors[0].at - (2980.0 + 450.0)).abs() < 1e-6, "{:?}", p.doors);
        assert!(p.validate(3).is_ok());
        // Running it again finds nothing more.
        assert_eq!(doors_from_gaps(&mut walls, None, &RULES), Found::default());
    }

    #[test]
    fn hostile_rules_and_walls_do_nothing() {
        let mut walls = vec![wall("a", [0.0, 0.0], [0.0, 0.0], 100.0), wall("b", [f64::NAN, 0.0], [1.0, 0.0], 100.0)];
        let _ = doors_from_gaps(&mut walls, None, &RULES);
        let mut ok = vec![wall("a", [0.0, 0.0], [3000.0, 0.0], 120.0), wall("b", [3800.0, 0.0], [6000.0, 0.0], 120.0)];
        assert_eq!(doors_from_gaps(&mut ok, None, &Rules { min: f64::NAN, max: 1.0, height: 2100.0 }), Found::default());
        assert_eq!(doors_from_gaps(&mut ok, Some(&[7]), &RULES), Found::default());
    }
}
