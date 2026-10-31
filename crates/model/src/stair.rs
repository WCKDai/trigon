//! Staircases: the plan symbol (treads, well, half landing, walking line) and the 3D steps.

use crate::doc::{Stair, StairKind};
use crate::mesh::Mesh;
use crate::{ModelError, Result};

/// Riser height aimed for, mm (Danish practice: about 170–180 mm).
const RISER: f64 = 175.0;
const STEP_SOFFIT: f64 = 150.0;
const LANDING_SLAB: f64 = 180.0;

/// A stair's frame: origin p0, unit run direction, unit across direction, run length, width.
#[derive(Debug, Clone, Copy)]
pub struct Frame {
    pub o: [f64; 2],
    pub run: [f64; 2],
    pub across: [f64; 2],
    pub length: f64,
    pub width: f64,
}

impl Frame {
    pub fn at(&self, s: f64, t: f64) -> [f64; 2] {
        [self.o[0] + self.run[0] * s + self.across[0] * t, self.o[1] + self.run[1] * s + self.across[1] * t]
    }
}

pub fn frame(st: &Stair) -> Result<Frame> {
    let [p0, p1, p2, p3] = st.outline.as_slice() else {
        return Err(ModelError::Invalid(format!("stair {}: the outline needs four corners", st.id)));
    };
    let (r, a) = ([p1[0] - p0[0], p1[1] - p0[1]], [p2[0] - p1[0], p2[1] - p1[1]]);
    let (length, width) = ((r[0] * r[0] + r[1] * r[1]).sqrt(), (a[0] * a[0] + a[1] * a[1]).sqrt());
    let min_run = if st.kind == StairKind::DoubleQuarterTurn { 100.0 } else { 600.0 };
    if !(length >= min_run && width >= 600.0 && length <= 20_000.0 && width <= 10_000.0) {
        return Err(ModelError::Invalid(format!("stair {}: flights {length:.0} × {width:.0} mm are out of range", st.id)));
    }
    if st.outline.iter().flatten().any(|v| !v.is_finite() || v.abs() > 1e9)
        || (r[0] * a[0] + r[1] * a[1]).abs() > 1e-3 * length * width
        || (p3[0] - p0[0] - a[0]).hypot(p3[1] - p0[1] - a[1]) > 1.0
    {
        return Err(ModelError::Invalid(format!("stair {}: the four corners must form a finite rectangle", st.id)));
    }
    if st.winders.is_some_and(|n| st.kind == StairKind::Straight || !(2..=16).contains(&n)) {
        return Err(ModelError::Invalid("winders need a turning stair and 2 to 16 turning steps".into()));
    }
    if st.kind == StairKind::UTurn && st.winders.is_some() && st.landing.unwrap_or(width / 2.0) <= st.well.unwrap_or(150.0) / 2.0 {
        return Err(ModelError::Invalid("winding turn depth must extend beyond the stair well".into()));
    }
    if let Some(depths) = st.turn_depths {
        if st.kind != StairKind::DoubleQuarterTurn {
            return Err(ModelError::Invalid("turn_depths need a double quarter turn".into()));
        }
        for depth in depths {
            crate::checked("turn depth", depth, 300.0, 4000.0)?;
        }
    }
    for (name, value, lo, hi) in [("tread", st.tread, 150.0, 400.0), ("landing", st.landing, 300.0, width * 2.0), ("well", st.well, 0.0, width / 3.0)]
    {
        if let Some(v) = value {
            crate::checked(name, v, lo, hi)?;
        }
    }
    Ok(Frame { o: *p0, run: [r[0] / length, r[1] / length], across: [a[0] / width, a[1] / width], length, width })
}

/// The plan symbol: lines (pairs of points) for the outline, treads and well, the landing outline,
/// and the walking line with its arrow.
#[derive(Debug, Clone, Default)]
pub struct Symbol {
    pub outline: Vec<([f64; 2], [f64; 2])>,
    pub treads: Vec<([f64; 2], [f64; 2])>,
    pub landing: Vec<[f64; 2]>,
    pub walk: Vec<[f64; 2]>,
}

fn treads_in(st: &Stair, length: f64, risers: usize) -> (usize, f64) {
    // A flight with r risers has r − 1 treads; the last riser lands on the landing or the floor.
    match st.tread.filter(|g| g.is_finite() && *g >= 150.0 && *g <= 400.0) {
        // A measured tread: as many as fit the run (the drawing's own count).
        Some(g) => {
            let n = ((length / g) + 0.3).floor().clamp(1.0, 80.0) as usize;
            (n, length / n as f64)
        }
        None => {
            let n = risers.saturating_sub(1).clamp(1, 80);
            (n, length / n as f64)
        }
    }
}

pub fn symbol(st: &Stair, storey_height: f64) -> Result<Symbol> {
    crate::checked("stair height", storey_height, 100.0, 20_000.0)?;
    let f = frame(st)?;
    let risers = (storey_height / RISER).round().max(2.0) as usize;
    let mut s = Symbol::default();
    let corners = [f.at(0.0, 0.0), f.at(f.length, 0.0), f.at(f.length, f.width), f.at(0.0, f.width)];
    match st.kind {
        StairKind::DoubleQuarterTurn => {
            let (_, treads) = quarter_steps(st, &f, storey_height);
            s.treads = treads;
            let ring = opening(st)?;
            for k in 0..ring.len() {
                s.outline.push((ring[k], ring[(k + 1) % ring.len()]));
            }
            let [d0, d1] = st.turn_depths.unwrap_or([f.width; 2]);
            s.walk = vec![
                f.at(-d0 / 2.0, f.width),
                f.at(-d0 / 2.0, f.width / 2.0),
                f.at(f.length + d1 / 2.0, f.width / 2.0),
                f.at(f.length + d1 / 2.0, f.width),
            ];
        }
        StairKind::Straight => {
            for k in 0..4 {
                s.outline.push((corners[k], corners[(k + 1) % 4]));
            }
            let (n, g) = treads_in(st, f.length, risers);
            for k in 1..=n.min(80) {
                let x = (k as f64 * g).min(f.length);
                s.treads.push((f.at(x, 0.0), f.at(x, f.width)));
            }
            s.walk = vec![f.at(0.0, f.width / 2.0), f.at(f.length, f.width / 2.0)];
        }
        StairKind::UTurn => {
            let well = st.well.unwrap_or(150.0).clamp(0.0, f.width / 3.0);
            let flight = (f.width - well) / 2.0;
            // Outer stringers along both sides, the well's two lines in the middle, the start edge.
            s.outline.push((corners[0], corners[1]));
            s.outline.push((corners[3], corners[2]));
            s.outline.push((corners[0], corners[3]));
            s.outline.push((f.at(0.0, flight), f.at(f.length, flight)));
            s.outline.push((f.at(0.0, flight + well), f.at(f.length, flight + well)));
            let (n, g) = treads_in(st, f.length, risers / 2);
            for k in 1..=n.min(80) {
                let x = (k as f64 * g).min(f.length);
                s.treads.push((f.at(x, 0.0), f.at(x, flight)));
                s.treads.push((f.at(x, flight + well), f.at(x, f.width)));
            }
            let depth = st.landing.unwrap_or(f.width / 2.0).clamp(300.0, f.width * 2.0);
            if st.rounded {
                let r = f.width / 2.0;
                for k in 0..=24 {
                    let a = std::f64::consts::PI * k as f64 / 24.0;
                    s.landing.push(f.at(f.length + depth.min(r) / r * r * a.sin(), r - r * a.cos()));
                }
            } else {
                s.landing = vec![corners[1], f.at(f.length + depth, 0.0), f.at(f.length + depth, f.width), corners[2]];
            }
            if let Some(winders) = st.winders {
                for k in 0..=winders {
                    let a = std::f64::consts::PI * k as f64 / winders as f64;
                    s.treads.push((turn_point(&f, st, a, true), turn_point(&f, st, a, false)));
                }
            }
            // Walking line: up the first flight, round the landing, back down the second.
            let (c1, c2) = (flight / 2.0, f.width - flight / 2.0);
            s.walk = vec![
                f.at(0.0, c1),
                f.at(f.length, c1),
                f.at(f.length + depth.min(f.width / 2.0) * 0.5, f.width / 2.0),
                f.at(f.length, c2),
                f.at(0.0, c2),
            ];
        }
    }
    Ok(s)
}

/// What a part of a staircase is.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PartKind {
    /// Straight steps.
    Flight,
    /// Turning steps.
    Winders,
    /// A flat (half) landing.
    Landing,
}

/// One part of a staircase on one storey as a solid, with its steps (none for a landing).
#[derive(Debug, Clone, PartialEq)]
pub struct Part {
    pub kind: PartKind,
    pub mesh: Mesh,
    /// Risers climbed in this part, the last one onto the landing or floor above included.
    pub risers: usize,
    pub treads: usize,
    /// Riser height and going (tread depth on the walking line), mm.
    pub rise: f64,
    pub going: f64,
}

/// The steps and landing as solids, from `floor` (mm) up `storey_height`.
pub fn mesh(st: &Stair, floor: f64, storey_height: f64) -> Result<Mesh> {
    let mut m = Mesh::default();
    for p in parts(st, floor, storey_height)? {
        m.append(&p.mesh);
    }
    Ok(m)
}

/// The staircase's flights, turning steps and landing as separate solids, from `floor` (mm) up
/// `storey_height`, in the order they are climbed. Together they are [`mesh`].
pub fn parts(st: &Stair, floor: f64, storey_height: f64) -> Result<Vec<Part>> {
    crate::checked("stair floor", floor, -1e6, 1e6)?;
    crate::checked("stair height", storey_height, 100.0, 20_000.0)?;
    let f = frame(st)?;
    let block = |m: &mut Mesh, s0: f64, s1: f64, t0: f64, t1: f64, z0: f64, z1: f64| {
        let lo = [f.at(s0, t0), f.at(s1, t0), f.at(s1, t1), f.at(s0, t1)].map(|p| [p[0], p[1], z0]);
        let hi = lo.map(|p| [p[0], p[1], z1]);
        // Keep the faces outward whatever the frame's handedness.
        let ccw = crate::footprint::signed_area2(&lo.map(|p| [p[0], p[1]])) > 0.0;
        if ccw { m.hexahedron(lo, hi) } else { m.hexahedron([lo[3], lo[2], lo[1], lo[0]], [hi[3], hi[2], hi[1], hi[0]]) }
    };
    let part = |kind: PartKind, mesh: Mesh, risers: usize, treads: usize, rise: f64, going: f64| Part { kind, mesh, risers, treads, rise, going };
    let risers = (storey_height / RISER).round().max(2.0) as usize;
    let mut out = Vec::new();
    match st.kind {
        StairKind::DoubleQuarterTurn => {
            let (steps, _) = quarter_steps(st, &f, storey_height);
            let count = st.winders.unwrap_or(10);
            let (first, last) = (count.div_ceil(2), count / 2);
            let nominal = (storey_height / RISER).round() as usize;
            let (_, going) = treads_in(st, f.length, nominal.saturating_sub(count));
            let total = steps.len();
            let rise = storey_height / (total + 1) as f64;
            let straight = total.saturating_sub(first + last).min(total);
            let first = first.min(total - straight);
            let groups = [(PartKind::Winders, 0, first), (PartKind::Flight, first, first + straight), (PartKind::Winders, first + straight, total)];
            let mut solids: Vec<Mesh> = steps
                .into_iter()
                .enumerate()
                .map(|(k, mut ring)| {
                    if crate::footprint::signed_area2(&ring) < 0.0 {
                        ring.reverse();
                    }
                    let top = floor + (k + 1) as f64 * rise;
                    crate::build::prism(&ring, top - rise - STEP_SOFFIT, rise + STEP_SOFFIT)
                })
                .collect();
            let mut taken = 0;
            for (kind, from, to) in groups {
                if to <= from {
                    continue;
                }
                let mut m = Mesh::default();
                for solid in solids.drain(..(to - from).min(solids.len())) {
                    m.append(&solid);
                }
                taken += to - from;
                // The last riser steps onto the floor above.
                let risers = if taken == total { to - from + 1 } else { to - from };
                out.push(part(kind, m, risers, to - from, rise, going));
            }
        }
        StairKind::Straight => {
            let (n, g) = treads_in(st, f.length, risers);
            let hr = storey_height / (n + 1) as f64;
            let mut m = Mesh::default();
            for k in 1..=n.min(80) {
                let top = floor + k as f64 * hr;
                block(&mut m, (k - 1) as f64 * g, k as f64 * g, 0.0, f.width, top - hr - STEP_SOFFIT, top);
            }
            out.push(part(PartKind::Flight, m, n + 1, n, hr, g));
        }
        StairKind::UTurn => {
            let r = (risers / 2).max(2);
            let well = st.well.unwrap_or(150.0).clamp(0.0, f.width / 3.0);
            let flight = (f.width - well) / 2.0;
            let (n, g) = treads_in(st, f.length, r);
            let r = n + 1;
            let turn = st.winders.unwrap_or(1);
            let hr = storey_height / (2 * n + turn + 1) as f64;
            let (mut up, mut back) = (Mesh::default(), Mesh::default());
            for k in 1..=n.min(80) {
                // Up along the first flight towards the landing.
                let top = floor + k as f64 * hr;
                block(&mut up, (k - 1) as f64 * g, k as f64 * g, 0.0, flight, top - hr - STEP_SOFFIT, top);
                // Back along the second flight from the landing.
                let top2 = floor + (n + turn + k) as f64 * hr;
                block(&mut back, f.length - k as f64 * g, f.length - (k - 1) as f64 * g, flight + well, f.width, top2 - hr - STEP_SOFFIT, top2);
            }
            if let Some(winders) = st.winders {
                let mut m = Mesh::default();
                for k in 0..winders {
                    let a0 = std::f64::consts::PI * k as f64 / winders as f64;
                    let a1 = std::f64::consts::PI * (k + 1) as f64 / winders as f64;
                    let mut ring = vec![turn_point(&f, st, a0, true)];
                    // Include outer rectangle corners (45°/135° for a square turn) or arc samples.
                    let depth = st.landing.unwrap_or(f.width / 2.0);
                    let corner = (2.0 * depth / f.width).atan();
                    let mut angles = vec![a0, a1];
                    if st.rounded {
                        angles.extend((1..24).map(|j| std::f64::consts::PI * j as f64 / 24.0).filter(|a| *a > a0 && *a < a1));
                    } else {
                        angles.extend([corner, std::f64::consts::PI - corner].into_iter().filter(|a| *a > a0 && *a < a1));
                    }
                    angles.sort_by(f64::total_cmp);
                    ring.extend(angles.into_iter().map(|a| turn_point(&f, st, a, false)));
                    ring.push(turn_point(&f, st, a1, true));
                    if ring.first().zip(ring.last()).is_some_and(|(a, b)| (a[0] - b[0]).hypot(a[1] - b[1]) < 1e-9) {
                        ring.pop();
                    }
                    if crate::footprint::signed_area2(&ring) < 0.0 {
                        ring.reverse();
                    }
                    let top = floor + (n + k + 1) as f64 * hr;
                    m.append(&crate::build::prism(&ring, top - hr - STEP_SOFFIT, hr + STEP_SOFFIT));
                }
                out.push(part(PartKind::Flight, up, n, n, hr, g));
                out.push(part(PartKind::Winders, m, winders, winders, hr, g));
                out.push(part(PartKind::Flight, back, n + 1, n, hr, g));
                return Ok(out);
            }
            let zl = floor + r as f64 * hr;
            let depth = st.landing.unwrap_or(f.width / 2.0).clamp(300.0, f.width * 2.0);
            let poly: Vec<[f64; 2]> = if st.rounded {
                let rad = f.width / 2.0;
                (0..=24)
                    .map(|k| {
                        let a = std::f64::consts::PI * k as f64 / 24.0;
                        f.at(f.length + depth.min(rad) * a.sin(), rad - rad * a.cos())
                    })
                    .collect()
            } else {
                vec![f.at(f.length, 0.0), f.at(f.length + depth, 0.0), f.at(f.length + depth, f.width), f.at(f.length, f.width)]
            };
            let mut ring = poly;
            if crate::footprint::signed_area2(&ring) < 0.0 {
                ring.reverse();
            }
            out.push(part(PartKind::Flight, up, n + 1, n, hr, g));
            out.push(part(PartKind::Landing, crate::build::prism(&ring, zl - LANDING_SLAB, LANDING_SLAB), 0, 0, hr, 0.0));
            out.push(part(PartKind::Flight, back, n + 1, n, hr, g));
        }
    }
    Ok(out)
}

/// Balustrade height above the pitch line, mm.
const BALUSTRADE: f64 = 900.0;
const BALUSTRADE_THICKNESS: f64 = 30.0;

/// The balustrade along the well of a U-turn stair: a panel on each flight's well edge following
/// the pitch, and one across the well at the half landing. Other stairs run between walls and
/// get none.
pub fn railing(st: &Stair, floor: f64, storey_height: f64) -> Result<Mesh> {
    crate::checked("stair floor", floor, -1e6, 1e6)?;
    crate::checked("stair height", storey_height, 100.0, 20_000.0)?;
    let f = frame(st)?;
    let mut m = Mesh::default();
    if st.kind != StairKind::UTurn {
        return Ok(m);
    }
    let risers = (storey_height / RISER).round().max(2.0) as usize;
    let well = st.well.unwrap_or(150.0).clamp(0.0, f.width / 3.0);
    let flight = (f.width - well) / 2.0;
    let (n, g) = treads_in(st, f.length, (risers / 2).max(2));
    let turn = st.winders.unwrap_or(1);
    let hr = storey_height / (2 * n + turn + 1) as f64;
    let landing = floor + (n + 1) as f64 * hr;
    let run = n as f64 * g;
    let th = BALUSTRADE_THICKNESS;
    // A panel from (s0, z0) to (s1, z1) between t0 and t1, BALUSTRADE high.
    let panel = |m: &mut Mesh, (s0, z0): (f64, f64), (s1, z1): (f64, f64), (t0, t1): (f64, f64)| {
        let lo = [(s0, t0, z0), (s1, t0, z1), (s1, t1, z1), (s0, t1, z0)].map(|(s, t, z)| {
            let p = f.at(s, t);
            [p[0], p[1], z]
        });
        let hi = lo.map(|p| [p[0], p[1], p[2] + BALUSTRADE]);
        let ccw = crate::footprint::signed_area2(&lo.map(|p| [p[0], p[1]])) > 0.0;
        if ccw { m.hexahedron(lo, hi) } else { m.hexahedron([lo[3], lo[2], lo[1], lo[0]], [hi[3], hi[2], hi[1], hi[0]]) }
    };
    // Up the first flight, along its well edge; back down the second from the landing.
    panel(&mut m, (0.0, floor + hr), (run, landing), (flight - th, flight));
    panel(&mut m, (f.length, landing), (f.length - run, floor + storey_height - hr), (flight + well, flight + well + th));
    // Across the well where the flights turn.
    panel(&mut m, (f.length, landing), (f.length + th, landing), (flight - th, flight + well + th));
    Ok(m)
}

fn turn_point(f: &Frame, st: &Stair, angle: f64, inner: bool) -> [f64; 2] {
    let half = f.width / 2.0;
    let depth = st.landing.unwrap_or(half);
    let (s, c) = (angle.sin(), angle.cos());
    if inner {
        let r = st.well.unwrap_or(150.0) / 2.0;
        f.at(f.length + r * s, half - r * c)
    } else if st.rounded {
        f.at(f.length + depth.min(half) * s, half - half * c)
    } else {
        let radius = (depth / s.abs().max(1e-12)).min(half / c.abs().max(1e-12));
        f.at(f.length + radius * s, half - radius * c)
    }
}

/// The shaft boundary shared by floor openings and stair clearance, counter-clockwise.
pub fn opening(st: &Stair) -> Result<Vec<[f64; 2]>> {
    let f = frame(st)?;
    if st.kind == StairKind::DoubleQuarterTurn {
        let [d0, d1] = st.turn_depths.unwrap_or([f.width; 2]);
        let mut ring = vec![f.at(-d0, 0.0), f.at(f.length + d1, 0.0), f.at(f.length + d1, f.width), f.at(-d0, f.width)];
        if crate::footprint::signed_area2(&ring) < 0.0 {
            ring.reverse();
        }
        return Ok(ring);
    }
    let mut ring = vec![f.at(0.0, 0.0), f.at(f.length, 0.0)];
    if st.kind == StairKind::UTurn {
        ring.extend(symbol(st, 3000.0)?.landing.into_iter().skip(1));
    } else {
        ring.push(f.at(f.length, f.width));
    }
    ring.push(f.at(0.0, f.width));
    if crate::footprint::signed_area2(&ring) < 0.0 {
        ring.reverse();
    }
    Ok(ring)
}

/// Quarter winders and a straight middle flight, built once for both the plan and solids.
fn quarter_steps(st: &Stair, f: &Frame, height: f64) -> (Vec<Vec<[f64; 2]>>, Vec<([f64; 2], [f64; 2])>) {
    let count = st.winders.unwrap_or(10);
    let (first, last) = (count.div_ceil(2), count / 2);
    let nominal = (height / RISER).round() as usize;
    let (n, g) = treads_in(st, f.length, nominal.saturating_sub(count));
    let [d0, d1] = st.turn_depths.unwrap_or([f.width; 2]);
    let mut steps = Vec::new();
    let mut lines = Vec::new();
    for (at, depth, turns, entry) in [(0.0, d0, first, true), (f.length, d1, last, false)] {
        if !entry {
            for k in 0..n {
                let (x0, x1) = (k as f64 * g, (k + 1) as f64 * g);
                steps.push(vec![f.at(x0, 0.0), f.at(x1, 0.0), f.at(x1, f.width), f.at(x0, f.width)]);
                lines.push((f.at(x1, 0.0), f.at(x1, f.width)));
            }
        }
        let pivot = f.at(at, f.width);
        let corner = if entry { (f.width / depth).atan() } else { (depth / f.width).atan() };
        let outer = |a: f64| {
            let (s, c) = (a.sin(), a.cos());
            let (x, y) = if entry { (-c, -s) } else { (s, -c) };
            let radius = (depth / x.abs().max(1e-12)).min(f.width / y.abs().max(1e-12));
            f.at(at + radius * x, f.width + radius * y)
        };
        for k in 0..turns {
            let (a0, a1) = (std::f64::consts::FRAC_PI_2 * k as f64 / turns as f64, std::f64::consts::FRAC_PI_2 * (k + 1) as f64 / turns as f64);
            let mut ring = vec![pivot, outer(a0)];
            if corner > a0 && corner < a1 {
                ring.push(outer(corner));
            }
            ring.push(outer(a1));
            steps.push(ring);
            lines.push((pivot, outer(a1)));
        }
    }
    (steps, lines)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_u_turn_stair_has_a_balustrade_along_its_well_and_a_straight_one_none() {
        let st = u();
        let r = railing(&st, 0.0, 3000.0).unwrap();
        let (lo, hi) = r.bounds().unwrap();
        // From just above the floor to 900 mm over the top step, along the well (y ≈ 875–1025).
        assert!(lo[2] > 0.0 && hi[2] <= 3000.0 + BALUSTRADE + 1e-6, "{lo:?} {hi:?}");
        assert!(lo[1] > 800.0 && hi[1] < 1100.0, "{lo:?} {hi:?}");
        let straight = Stair { kind: StairKind::Straight, ..u() };
        assert!(railing(&straight, 0.0, 3000.0).unwrap().is_empty());
        assert!(railing(&st, f64::NAN, 3000.0).is_err());
    }

    fn u() -> Stair {
        Stair {
            id: "s".into(),
            levels: vec![1],
            outline: vec![[0.0, 0.0], [2000.0, 0.0], [2000.0, 1900.0], [0.0, 1900.0]],
            tread: None,
            kind: StairKind::UTurn,
            landing: None,
            well: None,
            rounded: true,
            winders: None,
            turn_depths: None,
        }
    }

    #[test]
    fn u_turn_symbol_has_two_flights_a_well_and_a_round_landing() {
        let s = symbol(&u(), 3000.0).unwrap();
        // 3000 / 175 → 17 risers → 8 per flight → 7 treads per flight, both flights.
        assert_eq!(s.treads.len(), 14);
        assert_eq!(s.outline.len(), 5);
        assert_eq!(s.landing.len(), 25);
        let far = s.landing.iter().map(|p| p[0]).fold(f64::MIN, f64::max);
        assert!((far - 2950.0).abs() < 1.0, "landing reaches half the width beyond the flights: {far}");
        assert_eq!(s.walk.len(), 5);
    }

    #[test]
    fn u_turn_steps_climb_a_full_storey() {
        let m = mesh(&u(), 3960.0, 3000.0).unwrap();
        let (lo, hi) = m.bounds().unwrap();
        // The top step of the second flight is one riser below the next floor.
        assert!((hi[2] - (3960.0 + 3000.0 - 3000.0 / 16.0)).abs() < 1.0, "{hi:?}");
        assert!(lo[2] < 3960.0);
        assert!(frame(&Stair { outline: vec![[0.0, 0.0], [10.0, 0.0], [10.0, 10.0], [0.0, 10.0]], ..u() }).is_err());
    }

    #[test]
    fn measured_flights_have_equal_risers_through_the_landing_and_floor() {
        for (length, tread) in [(2208.0, 276.0), (1750.0, 250.0), (2100.0, 276.0)] {
            let st = Stair { outline: vec![[0.0, 0.0], [length, 0.0], [length, 2250.0], [0.0, 2250.0]], tread: Some(tread), ..u() };
            for height in [2830.0, 3000.0, 3610.0] {
                let n = symbol(&st, height).unwrap().treads.len() / 2;
                let rise = height / (2 * (n + 1)) as f64;
                let m = mesh(&st, 0.0, height).unwrap();
                let (_, hi) = m.bounds().unwrap();
                assert!((hi[2] - (height - rise)).abs() < 1e-6, "{length}, {height}: final riser is {} instead of {rise}", height - hi[2]);
                let first_top = m.positions.iter().filter(|p| p[1] < 1051.0 && p[0] < length - 1e-6).map(|p| p[2]).fold(f64::MIN, f64::max);
                assert!((height / 2.0 - first_top - rise).abs() < 1e-6);
                assert!(m.positions.iter().all(|p| p[0] >= -1e-6), "steps must not overrun the flight start");
            }
        }
    }

    #[test]
    fn winding_turn_has_separate_rising_treads_and_old_stairs_still_load() {
        let old =
            serde_json::json!({"id":"old","levels":[0],"outline":[[0,0],[1500,0],[1500,2000],[0,2000]],"kind":"u_turn","tread":250,"rounded":false});
        let mut st: Stair = serde_json::from_value(old).unwrap();
        assert_eq!(st.winders, None);
        st.winders = Some(6);
        let m = mesh(&st, 0.0, 2830.0).unwrap();
        let rise = 2830.0 / 19.0; // 6 + 6 + 6 treads, then destination floor.
        assert!((m.bounds().unwrap().1[2] - (2830.0 - rise)).abs() < 1e-6);
        let mut tops: Vec<_> = m
            .triangles
            .iter()
            .filter_map(|t| {
                let [a, b, c] = t.map(|i| m.positions[i as usize]);
                ((a[2] - b[2]).abs() < 1e-6 && (a[2] - c[2]).abs() < 1e-6 && a[0] >= 1500.0 && b[0] >= 1500.0 && c[0] >= 1500.0 && a[2] > 0.0)
                    .then_some(a[2])
            })
            .collect();
        tops.sort_by(f64::total_cmp);
        for k in 7..=12 {
            assert!(tops.iter().any(|z| (*z - k as f64 * rise).abs() < 1e-6));
        }
        assert_eq!(symbol(&st, 2830.0).unwrap().treads.len(), 19);
        assert_eq!(serde_json::from_value::<Stair>(serde_json::to_value(&st).unwrap()).unwrap(), st);
    }

    #[test]
    fn parts_are_the_mesh_split_into_flights_turns_and_landings() {
        let dqt = Stair {
            kind: StairKind::DoubleQuarterTurn,
            outline: vec![[0., 0.], [1375., 0.], [1375., 950.], [0., 950.]],
            tread: Some(275.),
            winders: Some(9),
            turn_depths: Some([1400., 1150.]),
            rounded: false,
            ..u()
        };
        let winding = Stair { winders: Some(6), rounded: false, ..u() };
        let straight = Stair { kind: StairKind::Straight, ..u() };
        let cases = [
            (u(), vec![PartKind::Flight, PartKind::Landing, PartKind::Flight]),
            (winding, vec![PartKind::Flight, PartKind::Winders, PartKind::Flight]),
            (straight, vec![PartKind::Flight]),
            (dqt, vec![PartKind::Winders, PartKind::Flight, PartKind::Winders]),
        ];
        for (st, kinds) in cases {
            let height = 2830.0;
            let parts = parts(&st, 100.0, height).unwrap();
            assert_eq!(parts.iter().map(|p| p.kind).collect::<Vec<_>>(), kinds, "{:?}", st.kind);
            let mut all = Mesh::default();
            for p in &parts {
                all.append(&p.mesh);
            }
            let whole = mesh(&st, 100.0, height).unwrap();
            assert_eq!(all.bounds(), whole.bounds());
            assert_eq!(all.triangles.len(), whole.triangles.len());
            // Every riser of the storey is counted once, and they add up to its height.
            let risers: usize = parts.iter().map(|p| p.risers).sum();
            let rise = parts[0].rise;
            assert!((risers as f64 * rise - height).abs() < 1e-6, "{:?}: {risers} × {rise}", st.kind);
        }
    }

    #[test]
    fn double_quarter_turn_follows_one_side_of_the_stairwell() {
        let st = Stair {
            kind: StairKind::DoubleQuarterTurn,
            outline: vec![[0., 0.], [1375., 0.], [1375., 950.], [0., 950.]],
            tread: Some(275.),
            winders: Some(9),
            turn_depths: Some([1400., 1150.]),
            rounded: false,
            ..u()
        };
        let f = frame(&st).unwrap();
        let (steps, lines) = quarter_steps(&st, &f, 2830.);
        assert_eq!(steps.len(), 14); // 5 first winders, 5 straight treads, 4 last winders.
        assert_eq!(lines.len(), 14);
        let area: f64 = steps.iter().map(|p| crate::doc::polygon_area(p)).sum();
        assert!((area - (1400. + 1375. + 1150.) * 950.).abs() < 1e-6);
        let mesh = mesh(&st, -2480., 2830.).unwrap();
        let (lo, hi) = mesh.bounds().unwrap();
        assert!((hi[2] - (350. - 2830. / 15.)).abs() < 1e-6);
        assert_eq!((lo[1], hi[1]), (0., 950.));
        assert!(st.outline.iter().all(|p| p[1] <= 950.));
    }
}
