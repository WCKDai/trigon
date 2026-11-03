//! Staircases: the plan symbol (treads, well, half landing, walking line) and the 3D steps.

use crate::doc::{Stair, StairKind};
use crate::mesh::Mesh;
use crate::{ModelError, Result};

mod fitted;

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

    /// Site coordinates back to a validated frame, including its accepted rectangle tolerance.
    pub fn local(&self, p: [f64; 2]) -> [f64; 2] {
        let d = [p[0] - self.o[0], p[1] - self.o[1]];
        let determinant = self.run[0] * self.across[1] - self.run[1] * self.across[0];
        [(d[0] * self.across[1] - d[1] * self.across[0]) / determinant, (self.run[0] * d[1] - self.run[1] * d[0]) / determinant]
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
    if let Some([run, across]) = st.turn_chamfer {
        if st.boundary.is_some() {
            return Err(ModelError::Invalid(
                "a fitted stair boundary and turn_chamfer are mutually exclusive; clear turn_chamfer when fitting".into(),
            ));
        }
        if st.kind != StairKind::UTurn || st.rounded {
            return Err(ModelError::Invalid("turn_chamfer needs a non-rounded U-turn stair".into()));
        }
        let depth = st.landing.unwrap_or(width / 2.0);
        crate::checked("turn chamfer along run", run, 1.0, depth - 1.0)?;
        crate::checked("turn chamfer across run", across, 1.0, width / 2.0 - 1.0)?;
        let support = (across * (depth - run) + run * width / 2.0) / run.hypot(across);
        if st.winders.is_some() && support <= st.well.unwrap_or(150.0) / 2.0 {
            return Err(ModelError::Invalid("turn chamfer cuts into the stair well".into()));
        }
    }
    let frame = Frame { o: *p0, run: [r[0] / length, r[1] / length], across: [a[0] / width, a[1] / width], length, width };
    fitted::validate(st, &frame)?;
    Ok(frame)
}

fn walking_line(st: &Stair, f: &Frame) -> Vec<[f64; 2]> {
    match st.kind {
        StairKind::Straight => vec![f.at(0., f.width / 2.), f.at(f.length, f.width / 2.)],
        StairKind::DoubleQuarterTurn => {
            let [d0, d1] = st.turn_depths.unwrap_or([f.width; 2]);
            vec![f.at(-d0 / 2., f.width), f.at(-d0 / 2., f.width / 2.), f.at(f.length + d1 / 2., f.width / 2.), f.at(f.length + d1 / 2., f.width)]
        }
        StairKind::UTurn => {
            let well = st.well.unwrap_or(150.).clamp(0., f.width / 3.);
            let flight = (f.width - well) / 2.;
            let depth = st.landing.unwrap_or(f.width / 2.).clamp(300., f.width * 2.);
            let (c1, c2) = (flight / 2., f.width - flight / 2.);
            vec![f.at(0., c1), f.at(f.length, c1), f.at(f.length + depth.min(f.width / 2.) * 0.5, f.width / 2.), f.at(f.length, c2), f.at(0., c2)]
        }
    }
}

fn plan_markings(walk: &[[f64; 2]]) -> Vec<([f64; 2], [f64; 2], f64)> {
    let mut marks = Vec::new();
    if let [w0, w1, ..] = walk {
        let (dx, dy) = (w1[0] - w0[0], w1[1] - w0[1]);
        let length = (dx * dx + dy * dy).sqrt().max(1.);
        let (ux, uy) = (dx / length, dy / length);
        let lead = 450.0_f64.min(length);
        let tip = [w0[0] + ux * lead, w0[1] + uy * lead];
        for side in [-1., 1.] {
            marks.push(([tip[0] - ux * 180. - uy * 90. * side, tip[1] - uy * 180. + ux * 90. * side], tip, 9.));
        }
        let dot = [w0[0] + ux * 60., w0[1] + uy * 60.];
        marks.push(([dot[0] - 40., dot[1]], [dot[0] + 40., dot[1]], 70.));
    }
    marks
}

/// The plan symbol: lines (pairs of points) for the outline, treads and well, the landing outline,
/// and the walking line with its arrow.
#[derive(Debug, Clone, Default)]
pub struct Symbol {
    pub outline: Vec<([f64; 2], [f64; 2])>,
    pub treads: Vec<([f64; 2], [f64; 2])>,
    pub landing: Vec<[f64; 2]>,
    pub walk: Vec<[f64; 2]>,
    /// Walking arrow and start marker; endpoints and stroke width in model millimetres.
    pub markings: Vec<([f64; 2], [f64; 2], f64)>,
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

fn u_turn_treads(st: &Stair, length: f64, risers: usize) -> (usize, f64) {
    // Winders already climb part of the storey. Reserve those and the final floor riser
    // before splitting the remaining straight treads between the two equal flights.
    // Measured tread spacing still takes precedence inside treads_in.
    let flight_risers = st.winders.map_or(risers / 2, |turn| risers.saturating_sub(turn + 1).div_ceil(2) + 1);
    treads_in(st, length, flight_risers)
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
            let (n, g) = u_turn_treads(st, f.length, risers);
            for k in 1..=n.min(80) {
                let x = (k as f64 * g).min(f.length);
                s.treads.push((f.at(x, 0.0), f.at(x, flight)));
                s.treads.push((f.at(x, flight + well), f.at(x, f.width)));
            }
            let depth = st.landing.unwrap_or(f.width / 2.0).clamp(300.0, f.width * 2.0);
            if st.boundary.is_some() {
                s.landing = vec![corners[1], f.at(f.length + depth, 0.), f.at(f.length + depth, f.width), corners[2]];
            } else if st.turn_chamfer.is_some() {
                s.landing = chamfered_turn(&f, st);
            } else if st.rounded {
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
        }
    }
    s.walk = walking_line(st, &f);
    s.markings = plan_markings(&s.walk);
    fitted::symbol(st, &mut s)?;
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
    let block = |m: &mut Mesh, s0: f64, s1: f64, t0: f64, t1: f64, z0: f64, z1: f64| -> Result<()> {
        if st.boundary.is_some() {
            m.append(&fitted::prism(st, &[f.at(s0, t0), f.at(s1, t0), f.at(s1, t1), f.at(s0, t1)], z0, z1 - z0)?);
            return Ok(());
        }
        let lo = [f.at(s0, t0), f.at(s1, t0), f.at(s1, t1), f.at(s0, t1)].map(|p| [p[0], p[1], z0]);
        let hi = lo.map(|p| [p[0], p[1], z1]);
        // Keep the faces outward whatever the frame's handedness.
        let ccw = crate::footprint::signed_area2(&lo.map(|p| [p[0], p[1]])) > 0.0;
        if ccw {
            m.hexahedron(lo, hi)
        } else {
            m.hexahedron([lo[3], lo[2], lo[1], lo[0]], [hi[3], hi[2], hi[1], hi[0]])
        }
        Ok(())
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
                    fitted::prism(st, &ring, top - rise - STEP_SOFFIT, rise + STEP_SOFFIT)
                })
                .collect::<Result<Vec<_>>>()?;
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
                block(&mut m, (k - 1) as f64 * g, k as f64 * g, 0.0, f.width, top - hr - STEP_SOFFIT, top)?;
            }
            out.push(part(PartKind::Flight, m, n + 1, n, hr, g));
        }
        StairKind::UTurn => {
            let well = st.well.unwrap_or(150.0).clamp(0.0, f.width / 3.0);
            let flight = (f.width - well) / 2.0;
            let (n, g) = u_turn_treads(st, f.length, risers);
            let r = n + 1;
            let turn = st.winders.unwrap_or(1);
            let hr = storey_height / (2 * n + turn + 1) as f64;
            let (mut up, mut back) = (Mesh::default(), Mesh::default());
            for k in 1..=n.min(80) {
                // Up along the first flight towards the landing.
                let top = floor + k as f64 * hr;
                block(&mut up, (k - 1) as f64 * g, k as f64 * g, 0.0, flight, top - hr - STEP_SOFFIT, top)?;
                // Back along the second flight from the landing.
                let top2 = floor + (n + turn + k) as f64 * hr;
                block(&mut back, f.length - k as f64 * g, f.length - (k - 1) as f64 * g, flight + well, f.width, top2 - hr - STEP_SOFFIT, top2)?;
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
                    if st.boundary.is_some() {
                        angles.extend([corner, std::f64::consts::PI - corner].into_iter().filter(|a| *a > a0 && *a < a1));
                    } else if let Some([run, across]) = st.turn_chamfer {
                        let first = (depth - run).atan2(f.width / 2.0);
                        let second = depth.atan2(f.width / 2.0 - across);
                        angles.extend(
                            [first, second, std::f64::consts::PI - second, std::f64::consts::PI - first]
                                .into_iter()
                                // A transformed frame can move a corner angle by a few ULPs
                                // onto a step boundary. Keep the exact boundary once.
                                .filter(|a| *a > a0 + 1e-12 && *a < a1 - 1e-12),
                        );
                    } else if st.rounded {
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
                    m.append(&fitted::prism(st, &ring, top - hr - STEP_SOFFIT, hr + STEP_SOFFIT)?);
                }
                out.push(part(PartKind::Flight, up, n, n, hr, g));
                out.push(part(PartKind::Winders, m, winders, winders, hr, g));
                out.push(part(PartKind::Flight, back, n + 1, n, hr, g));
                return Ok(out);
            }
            let zl = floor + r as f64 * hr;
            let depth = st.landing.unwrap_or(f.width / 2.0).clamp(300.0, f.width * 2.0);
            let poly: Vec<[f64; 2]> = if st.boundary.is_some() {
                vec![f.at(f.length, 0.), f.at(f.length + depth, 0.), f.at(f.length + depth, f.width), f.at(f.length, f.width)]
            } else if st.turn_chamfer.is_some() {
                chamfered_turn(&f, st)
            } else if st.rounded {
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
            out.push(part(PartKind::Landing, fitted::prism(st, &ring, zl - LANDING_SLAB, LANDING_SLAB)?, 0, 0, hr, 0.0));
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
    let (n, g) = u_turn_treads(st, f.length, risers);
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
    let return_height = if st.winders.is_some() { floor + (n + turn + 1) as f64 * hr } else { landing };
    panel(&mut m, (f.length, return_height), (f.length - run, floor + storey_height - hr), (flight + well, flight + well + th));
    // Submicrometre wells have the zero-well rail topology: an annulus there would
    // collapse onto the pivot at supported site coordinates and create zero-area faces.
    if st.winders.is_some() && well >= 0.001 {
        // Follow the same inside turn as the winders and meet both flight pitch lines.
        // Each short pitched strip is a closed solid; no flat panel crosses the stair well.
        for k in 0..24 {
            let at = |j: usize, r: f64| {
                let a = std::f64::consts::PI * j as f64 / 24.0;
                let r = r.min(turn_outer_radius(&f, st, a));
                let p = f.at(f.length + r * a.sin(), f.width / 2.0 - r * a.cos());
                [p[0], p[1], landing + (return_height - landing) * j as f64 / 24.0]
            };
            let r = well / 2.0;
            let lo = [at(k, r), at(k, r + th), at(k + 1, r + th), at(k + 1, r)];
            let hi = lo.map(|p| [p[0], p[1], p[2] + BALUSTRADE]);
            let ccw = crate::footprint::signed_area2(&lo.map(|p| [p[0], p[1]])) > 0.0;
            if ccw { m.hexahedron(lo, hi) } else { m.hexahedron([lo[3], lo[2], lo[1], lo[0]], [hi[3], hi[2], hi[1], hi[0]]) }
        }
    } else if st.winders.is_none() {
        // Legacy half-landing closure retains its exact geometry.
        panel(&mut m, (f.length, landing), (f.length + th, landing), (flight - th, flight + well + th));
    }
    fitted::railing(st, &m)?;
    Ok(m)
}

fn turn_point(f: &Frame, st: &Stair, angle: f64, inner: bool) -> [f64; 2] {
    let half = f.width / 2.0;
    let depth = st.landing.unwrap_or(half);
    let (s, c) = (angle.sin(), angle.cos());
    if inner {
        let r = st.well.unwrap_or(150.0) / 2.0;
        f.at(f.length + r * s, half - r * c)
    } else if st.boundary.is_some() {
        let radius = (depth / s.abs().max(1e-12)).min(half / c.abs().max(1e-12));
        f.at(f.length + radius * s, half - radius * c)
    } else if st.rounded {
        f.at(f.length + depth.min(half) * s, half - half * c)
    } else {
        let radius = turn_outer_radius(f, st, angle);
        f.at(f.length + radius * s, half - radius * c)
    }
}

fn turn_outer_radius(f: &Frame, st: &Stair, angle: f64) -> f64 {
    let half = f.width / 2.0;
    let depth = st.landing.unwrap_or(half);
    let (s, c) = (angle.sin(), angle.cos());
    if st.rounded {
        // opening() uses the 24-chord head, not the ideal ellipse. Its half-plane
        // bounds keep the rail inside the same physical floor-opening polygon.
        let mut radius = f64::INFINITY;
        for k in 0..24 {
            let at = |j: usize| {
                let a = std::f64::consts::PI * j as f64 / 24.0;
                [depth.min(half) * a.sin(), -half * a.cos()]
            };
            let (a, b) = (at(k), at(k + 1));
            let normal = [b[1] - a[1], a[0] - b[0]];
            let denom = normal[0] * s - normal[1] * c;
            if denom > 1e-12 {
                radius = radius.min((normal[0] * a[0] + normal[1] * a[1]) / denom);
            }
        }
        return radius;
    }
    let mut radius = (depth / s.abs().max(1e-12)).min(half / c.abs().max(1e-12));
    if let Some([run, across]) = st.turn_chamfer {
        let bound = across * (depth - run) + run * half;
        radius = radius.min(bound / (across * s + run * c.abs()).max(1e-12));
    }
    radius
}

fn chamfered_turn(f: &Frame, st: &Stair) -> Vec<[f64; 2]> {
    let [run, across] = st.turn_chamfer.unwrap_or([0.0, 0.0]);
    let depth = st.landing.unwrap_or(f.width / 2.0);
    vec![
        f.at(f.length, 0.0),
        f.at(f.length + depth - run, 0.0),
        f.at(f.length + depth, across),
        f.at(f.length + depth, f.width - across),
        f.at(f.length + depth - run, f.width),
        f.at(f.length, f.width),
    ]
}

/// The shaft boundary shared by floor openings and stair clearance, counter-clockwise.
pub fn opening(st: &Stair) -> Result<Vec<[f64; 2]>> {
    let f = frame(st)?;
    if let Some(boundary) = &st.boundary {
        let mut ring = boundary.clone();
        if crate::footprint::signed_area2(&ring) < 0. {
            ring.reverse();
        }
        return Ok(ring);
    }
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

/// Whether one complete shaft opening covers another, including concave fitted edges.
/// Uses the same 0.000001 mm boundary tolerance as fitted stair validation, rather than
/// polygon-overlay quantization or an area cutoff. This is a numerical tolerance, not
/// a statement of measurement accuracy. Neither stair is changed.
pub fn opening_covers(outer: &Stair, inner: &Stair) -> Result<bool> {
    let outer = opening(outer)?;
    let inner = opening(inner)?;
    Ok(inner.iter().zip(inner.iter().cycle().skip(1)).all(|(&a, &b)| fitted::covers_segment(&outer, a, b)))
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
            turn_chamfer: None,
            boundary: None,
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
    fn automatic_winding_stairs_budget_the_turn_before_counting_straight_treads() {
        let st = Stair {
            outline: vec![[0.0, 0.0], [1080.0, 0.0], [1080.0, 1680.0], [0.0, 1680.0]],
            landing: Some(500.0),
            well: Some(90.0),
            rounded: false,
            winders: Some(8),
            ..u()
        };
        let parts = parts(&st, 0.0, 3060.0).unwrap();
        assert_eq!(parts.iter().map(|p| p.risers).collect::<Vec<_>>(), [4, 8, 5]);
        assert_eq!(parts.iter().map(|p| p.treads).collect::<Vec<_>>(), [4, 8, 4]);
        assert!(parts.iter().all(|p| (p.rise - 180.0).abs() < 1e-9));
        assert!((parts[0].going - 270.0).abs() < 1e-9);
        let symbol = symbol(&st, 3060.0).unwrap();
        assert_eq!(symbol.treads.len(), 2 * 4 + 8 + 1);
        let mesh = mesh(&st, 0.0, 3060.0).unwrap();
        assert!((mesh.bounds().unwrap().1[2] - (3060.0 - 180.0)).abs() < 1e-9);
        // The first railing panel rises to the same turning elevation as the steps.
        let rail = railing(&st, 0.0, 3060.0).unwrap();
        let first_panel_top = rail.positions[..8].iter().map(|p| p[2]).fold(f64::NEG_INFINITY, f64::max);
        assert!((first_panel_top - (5.0 * 180.0 + BALUSTRADE)).abs() < 1e-9);
    }

    #[test]
    fn winding_railing_connects_the_two_flight_pitch_lines_at_their_actual_heights() {
        let st = Stair { rounded: false, landing: Some(600.0), winders: Some(8), ..u() };
        let f = frame(&st).unwrap();
        let parts = parts(&st, 850.0, 3060.0).unwrap();
        let n = parts[0].treads;
        let rise = parts[0].rise;
        let well = st.well.unwrap_or(150.0);
        let flight = (f.width - well) / 2.0;
        let rail = railing(&st, 850.0, 3060.0).unwrap();
        let first_height = 850.0 + (n + 1) as f64 * rise;
        let return_height = 850.0 + (n + 8 + 1) as f64 * rise;
        for (at, height) in [(f.at(f.length, flight), first_height), (f.at(f.length, flight + well), return_height)] {
            let heights: Vec<_> = rail.positions.iter().filter(|p| (p[0] - at[0]).hypot(p[1] - at[1]) < 1e-6).map(|p| p[2]).collect();
            assert!(
                heights.iter().all(|z| (*z - height).abs() < 1e-6 || (*z - height - BALUSTRADE).abs() < 1e-6),
                "{at:?}: {heights:?}, expected {height}"
            );
            assert!(heights.iter().any(|z| (*z - height).abs() < 1e-6));
        }
        // The rail follows the actual inside turn; it does not cut straight across the well.
        assert!(rail.positions.iter().any(|p| p[0] > f.length + well / 2.0 - 1e-6));
    }

    #[test]
    fn winding_railing_stays_inside_shallow_and_chamfered_turn_heads() {
        for (rounded, well, chamfer) in [(false, 590.0, None), (true, 590.0, None), (false, 570.0, Some([299.0, 949.0]))] {
            let st = Stair { rounded, landing: Some(300.0), well: Some(well), turn_chamfer: chamfer, winders: Some(8), ..u() };
            let f = frame(&st).unwrap();
            let m = railing(&st, 0.0, 3060.0).unwrap();
            let boundary = opening(&st).unwrap();
            for p in &m.positions {
                for (a, b) in boundary.iter().zip(boundary.iter().cycle().skip(1)) {
                    assert!(
                        (b[0] - a[0]) * (p[1] - a[1]) - (b[1] - a[1]) * (p[0] - a[0]) >= -1e-6,
                        "outside the actual floor opening: {rounded}, {p:?}"
                    );
                }
                assert!(p[0] <= f.length + 300.0 + 1e-6, "{rounded}, {well}, {chamfer:?}: {p:?}");
                if let Some([run, across]) = chamfer {
                    let bound = across * (300.0 - run) + run * f.width / 2.0;
                    assert!(across * (p[0] - f.length) + run * (p[1] - f.width / 2.0).abs() <= bound + 1e-6, "{p:?}");
                }
                if rounded && p[0] >= f.length {
                    assert!(((p[0] - f.length) / 300.0).powi(2) + ((p[1] - f.width / 2.0) / (f.width / 2.0)).powi(2) <= 1.0 + 1e-9);
                }
            }
        }
    }

    #[test]
    fn submicrometre_wells_do_not_add_collapsed_turn_rail_faces() {
        let zero = Stair { winders: Some(8), well: Some(0.0), ..u() };
        let tiny = Stair { well: Some(1e-15), ..zero.clone() };
        assert_eq!(railing(&zero, 0.0, 3060.0).unwrap(), railing(&tiny, 0.0, 3060.0).unwrap());
    }

    #[test]
    fn fitted_stair_fills_the_whole_concave_wall_and_window_recess_boundary() {
        let boundary = vec![[0., 0.], [1700., 0.], [1700., 500.], [1900., 500.], [1900., 1400.], [1700., 1400.], [1700., 1900.], [0., 1900.]];
        let mut source = serde_json::to_value(Stair {
            outline: vec![[0., 0.], [1000., 0.], [1000., 1900.], [0., 1900.]],
            landing: Some(900.),
            rounded: false,
            winders: Some(8),
            ..u()
        })
        .unwrap();
        source["boundary"] = serde_json::json!(boundary);
        let st: Stair = serde_json::from_value(source.clone()).unwrap();
        assert_eq!(opening(&st).unwrap(), boundary);
        let inside = |p: [f64; 2]| {
            let mut winding = 0.;
            for (a, b) in boundary.iter().zip(boundary.iter().cycle().skip(1)) {
                let ab = [b[0] - a[0], b[1] - a[1]];
                let ap = [p[0] - a[0], p[1] - a[1]];
                let t = ((ap[0] * ab[0] + ap[1] * ab[1]) / (ab[0] * ab[0] + ab[1] * ab[1])).clamp(0., 1.);
                if (p[0] - a[0] - t * ab[0]).hypot(p[1] - a[1] - t * ab[1]) < 1e-6 {
                    return true;
                }
                let bp = [b[0] - p[0], b[1] - p[1]];
                let ap = [a[0] - p[0], a[1] - p[1]];
                winding += (ap[0] * bp[1] - ap[1] * bp[0]).atan2(ap[0] * bp[0] + ap[1] * bp[1]);
            }
            winding.abs() > std::f64::consts::PI
        };
        let symbol = symbol(&st, 3060.).unwrap();
        for k in 0..boundary.len() {
            assert!(symbol.outline.contains(&(boundary[k], boundary[(k + 1) % boundary.len()])));
        }
        assert!(symbol.treads.iter().all(|(a, b)| inside(*a) && inside(*b)));
        let parts = parts(&st, 850., 3060.).unwrap();
        assert!(parts.iter().flat_map(|p| &p.mesh.positions).all(|p| inside([p[0], p[1]])));
        for corner in &boundary {
            assert!(
                parts.iter().flat_map(|p| &p.mesh.positions).any(|p| (p[0] - corner[0]).hypot(p[1] - corner[1]) < 1e-6),
                "unfilled enclosure corner {corner:?}"
            );
        }
        assert!(railing(&st, 850., 3060.).unwrap().positions.iter().all(|p| inside([p[0], p[1]])));
        assert_eq!(serde_json::to_value(st).unwrap(), source);
        assert!(serde_json::to_value(u()).unwrap().get("boundary").is_none());
    }

    #[test]
    fn fitted_concave_parts_are_closed_outward_and_fill_rotated_mirrored_enclosures() {
        let boundary = [[0., 0.], [1700., 0.], [1700., 500.], [1900., 500.], [1900., 1400.], [1700., 1400.], [1700., 1900.], [0., 1900.]];
        for degrees in [0.0_f64, 37.25, -118.] {
            for mirror in [-1., 1.] {
                for reverse in [false, true] {
                    for winders in [None, Some(2), Some(7), Some(16)] {
                        let (sin, cos) = degrees.to_radians().sin_cos();
                        let at = |[x, y]: [f64; 2]| [150000. + x * cos - mirror * y * sin, -270000. + x * sin + mirror * y * cos];
                        let mut ring: Vec<_> = boundary.iter().copied().map(at).collect();
                        if reverse {
                            ring.reverse();
                        }
                        let st = Stair {
                            outline: vec![at([0., 0.]), at([1000., 0.]), at([1000., 1900.]), at([0., 1900.])],
                            landing: Some(900.),
                            rounded: false,
                            boundary: Some(ring.clone()),
                            winders,
                            ..u()
                        };
                        let parts = parts(&st, 850., 3060.).unwrap();
                        for p in &ring {
                            let distance =
                                parts.iter().flat_map(|p| &p.mesh.positions).map(|q| (p[0] - q[0]).hypot(p[1] - q[1])).fold(f64::INFINITY, f64::min);
                            assert!(distance < 1e-6, "{degrees}, {mirror}, {reverse}, {winders:?}: missing {p:?} by {distance} mm");
                        }
                        assert!(crate::footprint::signed_area2(&opening(&st).unwrap()) > 0.);
                        for part in parts {
                            let mut edges = std::collections::BTreeMap::<_, (usize, i32)>::new();
                            let key = |p: [f64; 3]| p.map(|v| (v * 1e6).round() as i64);
                            let mut volume = 0.;
                            for tri in &part.mesh.triangles {
                                let [a, b, c] = tri.map(|i| part.mesh.positions[i as usize]);
                                let ab = [b[0] - a[0], b[1] - a[1], b[2] - a[2]];
                                let ac = [c[0] - a[0], c[1] - a[1], c[2] - a[2]];
                                let n = [ab[1] * ac[2] - ab[2] * ac[1], ab[2] * ac[0] - ab[0] * ac[2], ab[0] * ac[1] - ab[1] * ac[0]];
                                assert!(n.iter().all(|v| v.is_finite()) && n.iter().map(|v| v * v).sum::<f64>() > 1e-10);
                                volume += [a[0] - 150000., a[1] + 270000., a[2] - 850.].iter().zip(n).map(|(v, n)| v * n).sum::<f64>() / 6.;
                                for (a, b) in [(key(a), key(b)), (key(b), key(c)), (key(c), key(a))] {
                                    let (edge, sign) = if a < b { ((a, b), 1) } else { ((b, a), -1) };
                                    let entry = edges.entry(edge).or_default();
                                    entry.0 += 1;
                                    entry.1 += sign;
                                }
                            }
                            assert!(volume > 0., "{degrees}, {mirror}, {reverse}, {winders:?}");
                            assert!(edges.values().all(|&(count, sign)| count == 2 && sign == 0), "{degrees}, {mirror}, {reverse}, {winders:?}");
                        }
                    }
                }
            }
        }
    }

    #[test]
    fn fitted_boundary_rejects_crossings_walking_line_cuts_and_disconnected_steps() {
        let mut st = Stair { rounded: false, ..u() };
        for ring in [
            vec![],
            vec![[0., 0.], [2000., 1900.], [2000., 0.], [0., 1900.]],
            vec![[0., 0.], [2000., 0.], [2000., 1900.], [0., 1900.], [0., 0.]],
            vec![[0., 0.], [2000., 0.], [2000., 1900.], [f64::NAN, 1900.]],
            vec![[0., 0.], [2000., 0.], [2000., 1900.], [0., 1e9]],
            vec![[0., 0.], [2000., 0.], [2000., 1900.], [0., 1900.]], // Omits the U-turn walking line.
        ] {
            st.boundary = Some(ring);
            assert!(frame(&st).is_err());
        }
        st.kind = StairKind::Straight;
        st.boundary = Some(vec![[0., 0.], [2000., 0.], [2000., 1050.], [200., 1050.], [200., 1300.], [2000., 1300.], [2000., 1900.], [0., 1900.]]);
        assert!(frame(&st).is_ok());
        assert!(mesh(&st, 0., 3060.).unwrap_err().to_string().contains("disconnects a tread"));
    }

    #[test]
    fn fitted_boundary_rejects_a_notch_through_the_middle_of_a_railing() {
        let st = Stair {
            rounded: false,
            landing: Some(600.),
            winders: Some(8),
            boundary: Some(vec![
                [0., 0.],
                [2600., 0.],
                [2600., 1900.],
                [0., 1900.],
                [0., 970.],
                [800., 970.],
                [800., 1100.],
                [1200., 1100.],
                [1200., 800.],
                [800., 800.],
                [800., 930.],
                [0., 930.],
            ]),
            ..u()
        };
        assert!(frame(&st).is_ok());
        assert!(parts(&st, 0., 3060.).is_ok());
        assert!(railing(&st, 0., 3060.).unwrap_err().to_string().contains("well railing"));
    }

    #[test]
    fn fitted_boundary_retains_complete_arrow_strokes_and_the_accepted_frame_tolerance() {
        let st = Stair {
            rounded: false,
            landing: Some(600.),
            winders: Some(8),
            boundary: Some(vec![[0., 400.], [2000., 400.], [2000., 0.], [2600., 0.], [2600., 1900.], [0., 1900.]]),
            ..u()
        };
        assert!(symbol(&st, 3060.).unwrap_err().to_string().contains("plan arrow"));
        let outline = vec![[0., 0.], [1000., 0.], [1001., 1900.], [1., 1900.]];
        let st = Stair { kind: StairKind::Straight, boundary: Some(outline.clone()), outline, ..u() };
        let f = frame(&st).unwrap();
        for (s, t) in [(0., 0.), (f.length, 0.), (f.length, f.width), (0., f.width)] {
            let actual = f.local(f.at(s, t));
            assert!((s - actual[0]).abs() < 1e-9 && (t - actual[1]).abs() < 1e-9);
        }
        assert!(!mesh(&st, 0., 3060.).unwrap().triangles.is_empty());
    }

    #[test]
    fn fitted_boundary_restores_the_nearest_distinct_source_corners() {
        let boundary = vec![[0., 0.], [2000., 0.], [2000., 1899.999995], [1999.999995, 1900.], [0., 1900.]];
        let st = Stair { kind: StairKind::Straight, boundary: Some(boundary.clone()), ..u() };
        let mesh = mesh(&st, 0., 3060.).unwrap();
        for p in boundary {
            assert!(mesh.positions.iter().any(|q| (p[0] - q[0]).hypot(p[1] - q[1]) < 1e-12), "missing source corner {p:?}");
        }
        for tri in &mesh.triangles {
            let [a, b, c] = tri.map(|i| mesh.positions[i as usize]);
            let ab = [b[0] - a[0], b[1] - a[1], b[2] - a[2]];
            let ac = [c[0] - a[0], c[1] - a[1], c[2] - a[2]];
            assert!([ab[1] * ac[2] - ab[2] * ac[1], ab[2] * ac[0] - ab[0] * ac[2], ab[0] * ac[1] - ab[1] * ac[0]].iter().any(|v| *v != 0.));
        }
    }

    #[test]
    fn explicit_winding_tread_keeps_the_measured_count_instead_of_the_target_rise() {
        let st = Stair {
            outline: vec![[0.0, 0.0], [1080.0, 0.0], [1080.0, 1680.0], [0.0, 1680.0]],
            tread: Some(180.0),
            landing: Some(500.0),
            well: Some(90.0),
            rounded: false,
            winders: Some(8),
            ..u()
        };
        let parts = parts(&st, 0.0, 3060.0).unwrap();
        assert_eq!(parts.iter().map(|p| p.risers).collect::<Vec<_>>(), [6, 8, 7]);
        assert!(parts.iter().all(|p| (p.rise - 3060.0 / 21.0).abs() < 1e-9));
        assert!((parts[0].going - 180.0).abs() < 1e-9);
        assert_eq!(symbol(&st, 3060.0).unwrap().treads.len(), 2 * 6 + 8 + 1);
    }

    #[test]
    fn chamfered_turn_uses_the_same_enclosure_for_treads_solids_and_floor_opening() {
        let mut value = serde_json::to_value(Stair { rounded: false, landing: Some(600.0), winders: Some(8), ..u() }).unwrap();
        value["turn_chamfer"] = serde_json::json!([250.0, 350.0]);
        let st: Stair = serde_json::from_value(value.clone()).unwrap();
        let f = frame(&st).unwrap();
        let symbol = symbol(&st, 3060.0).unwrap();
        let head = vec![f.at(2000.0, 0.0), f.at(2350.0, 0.0), f.at(2600.0, 350.0), f.at(2600.0, 1550.0), f.at(2350.0, 1900.0), f.at(2000.0, 1900.0)];
        assert_eq!(symbol.landing, head);
        let opening = opening(&st).unwrap();
        let inside = |p: [f64; 2]| {
            p[0] >= -1e-6
                && p[0] <= 2600.0 + 1e-6
                && p[1] >= -1e-6
                && p[1] <= 1900.0 + 1e-6
                && 350.0 * (p[0] - 2000.0) + 250.0 * (p[1] - 950.0).abs() <= 350.0 * 350.0 + 250.0 * 950.0 + 1e-6
        };
        assert!(opening.iter().all(|p| inside(*p)));
        assert!(symbol.treads.iter().all(|(a, b)| inside(*a) && inside(*b)));
        assert!(symbol.walk.iter().all(|p| inside(*p)));
        let meshes = parts(&st, 0.0, 3060.0).unwrap();
        assert!(meshes.iter().flat_map(|p| &p.mesh.positions).all(|p| inside([p[0], p[1]])));
        let turn = &meshes[1].mesh;
        for p in &head {
            assert!(turn.positions.iter().any(|q| (q[0] - p[0]).hypot(q[1] - p[1]) < 1e-6), "missing corner {p:?}");
        }
        assert_eq!(serde_json::to_value(&st).unwrap(), value);
        for cuts in [serde_json::json!([0.0, 350.0]), serde_json::json!([601.0, 350.0]), serde_json::json!([250.0, 951.0])] {
            let mut invalid = value.clone();
            invalid["turn_chamfer"] = cuts;
            assert!(frame(&serde_json::from_value(invalid).unwrap()).is_err());
        }
        let mut rounded = value;
        rounded["rounded"] = serde_json::json!(true);
        assert!(frame(&serde_json::from_value(rounded).unwrap()).is_err());
        assert!(serde_json::to_value(u()).unwrap().get("turn_chamfer").is_none());
    }

    #[test]
    fn chamfered_turn_parts_remain_closed_and_outward_in_rotated_and_mirrored_frames() {
        for rotation in [0.0_f64, 37.25, -118.0] {
            for mirror in [-1.0, 1.0] {
                for winders in [None, Some(2), Some(7), Some(16)] {
                    let (sin, cos) = rotation.to_radians().sin_cos();
                    let transform = |[x, y]: [f64; 2]| [150000.0 + x * cos - mirror * y * sin, -270000.0 + x * sin + mirror * y * cos];
                    let st = Stair {
                        outline: u().outline.into_iter().map(transform).collect(),
                        rounded: false,
                        landing: Some(600.0),
                        turn_chamfer: Some([250.0, 350.0]),
                        winders,
                        ..u()
                    };
                    for part in parts(&st, 850.0, 3060.0).unwrap() {
                        let mut edges = std::collections::BTreeMap::<_, (usize, i32)>::new();
                        let mut volume = 0.0;
                        let key = |p: [f64; 3]| p.map(|v| (v * 1e6).round() as i64);
                        for tri in &part.mesh.triangles {
                            let [a, b, c] = tri.map(|i| part.mesh.positions[i as usize]);
                            assert!(a.iter().chain(&b).chain(&c).all(|v| v.is_finite()));
                            let ab = [b[0] - a[0], b[1] - a[1], b[2] - a[2]];
                            let ac = [c[0] - a[0], c[1] - a[1], c[2] - a[2]];
                            let cross = [ab[1] * ac[2] - ab[2] * ac[1], ab[2] * ac[0] - ab[0] * ac[2], ab[0] * ac[1] - ab[1] * ac[0]];
                            assert!(
                                cross.iter().map(|v| v * v).sum::<f64>() > 1e-10,
                                "{rotation}, {mirror}, {winders:?}, {:?}: {a:?}, {b:?}, {c:?}",
                                part.kind
                            );
                            // Translate near the origin before measuring signed volume.
                            volume += [a[0] - 150000.0, a[1] + 270000.0, a[2] - 850.0].iter().zip(cross).map(|(p, n)| p * n).sum::<f64>() / 6.0;
                            for (a, b) in [(key(a), key(b)), (key(b), key(c)), (key(c), key(a))] {
                                let (edge, sign) = if a < b { ((a, b), 1) } else { ((b, a), -1) };
                                let entry = edges.entry(edge).or_default();
                                entry.0 += 1;
                                entry.1 += sign;
                            }
                        }
                        assert!(volume > 0.0, "{rotation}, {mirror}, {winders:?}, {:?}", part.kind);
                        assert!(
                            edges.values().all(|&(count, winding)| count == 2 && winding == 0),
                            "{rotation}, {mirror}, {winders:?}, {:?}",
                            part.kind
                        );
                    }
                }
            }
        }
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
