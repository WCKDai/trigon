//! Optional presentation drainage. Positions are explicit facade distances, never inferred from
//! photographs. These are geometric assemblies, not hydraulic sizing or below-ground drainage.

use std::collections::BTreeSet;
use std::f64::consts::PI;

use serde::{Deserialize, Serialize};

use crate::build::FacadeFrame;
use crate::{Building, Mesh, ModelError, Result, checked};

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Drainage {
    #[serde(default = "width")]
    pub gutter_width_mm: f64,
    #[serde(default = "wall")]
    pub wall_mm: f64,
    #[serde(default = "spacing")]
    pub joint_spacing_mm: f64,
    /// No downpipes are invented when this list is empty.
    #[serde(default)]
    pub downpipes: Vec<Downpipe>,
}

fn width() -> f64 {
    150.0
}
fn wall() -> f64 {
    2.0
}
fn spacing() -> f64 {
    3000.0
}

impl Default for Drainage {
    fn default() -> Self {
        Self { gutter_width_mm: width(), wall_mm: wall(), joint_spacing_mm: spacing(), downpipes: Vec::new() }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Downpipe {
    pub id: String,
    pub facade: String,
    /// Along the original facade from its start, mm; it must survive roof straightening.
    pub along_mm: f64,
    pub diameter_mm: f64,
    /// Pipe centre from the wall's outer face, mm, including its radius.
    pub wall_offset_mm: f64,
    /// Open shoe's outlet height, site-local mm.
    pub bottom_mm: f64,
    /// Outward throw of the open shoe, mm. Zero ends vertically.
    pub shoe_mm: f64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DrainClass {
    Gutter,
    Downpipe,
}

#[derive(Debug, Clone, PartialEq)]
pub struct DrainPart {
    pub id: String,
    pub class: DrainClass,
    pub mesh: Mesh,
}

fn invalid(s: &str) -> ModelError {
    ModelError::Invalid(s.into())
}

/// Build independently identified gutter runs and downpipe assemblies. The parent roof keeps its
/// identity and unchanged cover/gables. Each run has an open half-round channel, rolled rims,
/// shared mitres at adjacent eaves, stop ends only at gables, and closed sleeve joints. Metal is
/// closed across run boundaries as one assembly; per-eave semantic IDs remain stable.
pub fn build(
    b: &Building,
    kept: &[usize],
    outline: &[[f64; 2]],
    eaves: f64,
    overhang: f64,
    pitch: f64,
    gables: &[usize],
    d: &Drainage,
) -> Result<Vec<DrainPart>> {
    build_with_sources(b, kept, outline, eaves, overhang, pitch, gables, d, false)
}

/// Detailed drainage on explicitly selected source facade lines. `source_edges[k]` owns
/// eave run k, independent of the ring's cyclic starting corner. Along distances still use
/// the original facade, and gutter IDs use the original edge number (legacy IDs are untouched).
pub fn build_selected(
    b: &Building,
    source_edges: &[usize],
    outline: &[[f64; 2]],
    eaves: f64,
    overhang: f64,
    pitch: f64,
    gables: &[usize],
    d: &Drainage,
) -> Result<Vec<DrainPart>> {
    if !(3..=256).contains(&source_edges.len()) || source_edges.len() != outline.len() || !(3..=2000).contains(&b.footprint.len()) {
        return Err(invalid("selected drainage needs 3 to 256 eave runs with one original source edge per run"));
    }
    if outline.iter().flatten().chain(b.footprint.iter().flatten()).any(|v| !v.is_finite() || v.abs() > 1e8) {
        return Err(invalid("selected drainage needs finite site-local eaves and source lines"));
    }
    let mut edges = BTreeSet::new();
    let mut ids = BTreeSet::new();
    for (k, &edge) in source_edges.iter().enumerate() {
        let mut matches = b.facades.iter().filter(|f| f.edge == edge);
        let f = matches.next().ok_or_else(|| invalid(&format!("selected gutter run {k} has no known source facade for edge {edge}")))?;
        if matches.next().is_some()
            || !edges.insert(edge)
            || !ids.insert(&f.id)
            || f.id.is_empty()
            || f.id.len() > 128
            || f.id.chars().any(char::is_control)
        {
            return Err(invalid(&format!("selected gutter run {k} needs a unique source facade/edge; check {}", f.id)));
        }
        let (a, c) = b.edge(edge).ok_or_else(|| invalid(&format!("selected gutter source {} has no footprint edge", f.id)))?;
        let source = FacadeFrame::new(a, c).ok_or_else(|| invalid(&format!("selected gutter source {} is too short", f.id)))?;
        let run = FacadeFrame::new(outline[k], outline[(k + 1) % outline.len()])
            .ok_or_else(|| invalid(&format!("selected gutter run for {} is too short", f.id)))?;
        let direction = run.t[0] * source.t[0] + run.t[1] * source.t[1];
        let offset = (run.origin[0] - source.origin[0]) * source.n[0] + (run.origin[1] - source.origin[1]) * source.n[1];
        if direction < 1.0 - 1e-10 || (!gables.contains(&k) && (offset - overhang).abs() > 1e-5) {
            return Err(invalid(&format!(
                "selected gutter run {k} does not follow source facade {} at its declared overhang; revise eave_facades",
                f.id
            )));
        }
    }
    build_with_sources(b, source_edges, outline, eaves, overhang, pitch, gables, d, true)
}

fn build_with_sources(
    b: &Building,
    kept: &[usize],
    outline: &[[f64; 2]],
    eaves: f64,
    overhang: f64,
    pitch: f64,
    gables: &[usize],
    d: &Drainage,
    selected: bool,
) -> Result<Vec<DrainPart>> {
    checked("drainage overhang", overhang, 0.0, 2000.0)?;
    checked("drainage pitch", pitch, 5.0, 75.0)?;
    if b.footprint.len() < 3 || kept.iter().any(|&i| i >= b.footprint.len()) || outline.iter().flatten().any(|v| !v.is_finite() || v.abs() > 1e8) {
        return Err(invalid("drainage needs finite site-local eaves and valid source corners"));
    }
    checked("gutter width", d.gutter_width_mm, 100.0, 300.0)?;
    checked("drainage wall", d.wall_mm, 1.0, 5.0)?;
    checked("gutter joint spacing", d.joint_spacing_mm, 1000.0, 10000.0)?;
    if outline.len() < 3 || outline.len() > 256 || kept.len() != outline.len() || d.downpipes.len() > 64 {
        return Err(invalid("detailed drainage needs 3 to 256 eave corners and at most 64 downpipes"));
    }
    checked("eaves", eaves, -5000.0, 500000.0)?;
    if gables.iter().any(|k| *k >= outline.len()) {
        return Err(invalid("invalid drainage gable edge"));
    }
    let low = eaves - overhang * pitch.to_radians().tan();
    let r = d.gutter_width_mm / 2.0;
    let profile = ChannelProfile::new(r, d.wall_mm);
    let frames: Vec<_> = (0..outline.len())
        .map(|k| FacadeFrame::new(outline[k], outline[(k + 1) % outline.len()]).ok_or_else(|| invalid("invalid eave")))
        .collect::<Result<_>>()?;
    let mut corners = Vec::with_capacity(outline.len());
    for k in 0..outline.len() {
        let prev = (k + outline.len() - 1) % outline.len();
        let (a, c) = (frames[prev].n, frames[k].n);
        let joined = !gables.contains(&prev) && !gables.contains(&k);
        let denom = 1.0 + a[0] * c[0] + a[1] * c[1];
        if joined && denom < 1e-8 {
            return Err(invalid("gutter corner reverses direction; the nominal channel cannot fit"));
        }
        corners.push(joined.then(|| [(a[0] + c[0]) / denom, (a[1] + c[1]) / denom]));
    }
    let runs: Vec<_> = frames
        .iter()
        .enumerate()
        .map(|(k, &fr)| GutterRun {
            fr,
            start: corners[k].unwrap_or(fr.n),
            end: corners[(k + 1) % corners.len()].unwrap_or(fr.n),
            end_origin: outline[(k + 1) % outline.len()],
            stop_start: corners[k].is_none(),
            stop_end: corners[(k + 1) % corners.len()].is_none(),
            low,
        })
        .collect();
    for (k, run) in runs.iter().enumerate().filter(|(k, _)| !gables.contains(k)) {
        run.validate(&profile, d.wall_mm)
            .map_err(|_| invalid(&format!("gutter {k} is too short or its mitres cross; reduce width or revise the eave outline")))?;
    }
    let mut ids = BTreeSet::new();
    let mut outlets = Vec::new();
    for p in &d.downpipes {
        if p.id.is_empty() || p.id.len() > 64 || !p.id.bytes().all(|c| c.is_ascii_alphanumeric() || b"_-".contains(&c)) || !ids.insert(&p.id) {
            return Err(invalid("downpipe IDs need 1 to 64 unique ASCII letters, digits, _ or -"));
        }
        let f = b.facade(&p.facade)?;
        if f.edge >= b.footprint.len() {
            return Err(invalid("invalid downpipe facade edge"));
        }
        let k = if selected {
            kept.iter().position(|&i| i == f.edge).ok_or_else(|| {
                invalid(&format!("downpipe {} facade {} is absent from eave_facades; select its original eave facade", p.id, p.facade))
            })?
        } else {
            kept.iter()
                .position(|&i| i == f.edge)
                .filter(|&k| kept[(k + 1) % kept.len()] == (f.edge + 1) % b.footprint.len())
                .ok_or_else(|| invalid("downpipe facade was straightened away; lower roof straighten_mm"))?
        };
        if gables.contains(&k) {
            return Err(if selected {
                invalid(&format!("downpipe {} facade {} is a gable; choose a selected sloping eave facade", p.id, p.facade))
            } else {
                invalid("a downpipe needs an eave facade, not a gable")
            });
        }
        let (a, c) = b.edge(f.edge).ok_or_else(|| invalid("missing downpipe facade edge"))?;
        let wall = FacadeFrame::new(a, c).ok_or_else(|| invalid("downpipe facade too short"))?;
        checked("downpipe diameter", p.diameter_mm, 50.0, d.gutter_width_mm * 0.75)?;
        checked("downpipe along", p.along_mm, p.diameter_mm, wall.length - p.diameter_mm)?;
        checked("downpipe wall offset", p.wall_offset_mm, p.diameter_mm / 2.0 + 20.0, 1000.0)?;
        checked("downpipe shoe", p.shoe_mm, 0.0, 500.0)?;
        // Keep a real straight run between offset knees and shoe, and keep the lower
        // collar's full 40 mm extent below the upper collar (even with a small shoe).
        checked("downpipe bottom", p.bottom_mm, -5000.0, (low - r - 500.0 - p.shoe_mm.max(50.0) - p.diameter_mm.max(50.0)).min(low - r - 940.0))?;
        let fr = FacadeFrame::new(outline[k], outline[(k + 1) % outline.len()]).ok_or_else(|| invalid("invalid eave"))?;
        let anchor = wall.point(p.along_mm, 0.0, 0.0);
        let u = (anchor[0] - fr.origin[0]) * fr.t[0] + (anchor[1] - fr.origin[1]) * fr.t[1];
        checked("downpipe eave position", u, p.diameter_mm, fr.length - p.diameter_mm)?;
        let (start, end) = runs[k].straight_limits(&profile);
        checked("downpipe clear of gutter mitres", u, start + p.diameter_mm / 2.0 + d.wall_mm, end - p.diameter_mm / 2.0 - d.wall_mm)?;
        if outlets.iter().any(|(edge, at, _, other): &(usize, f64, FacadeFrame, &Downpipe)| {
            *edge == k && (u - at).abs() < (p.diameter_mm + other.diameter_mm) / 2.0 + 50.0
        }) {
            return Err(invalid("downpipe outlets must be separated by 50 mm of gutter"));
        }
        outlets.push((k, u, wall, p));
    }
    let mut parts = Vec::new();
    let mut joints = 0usize;
    for k in (0..outline.len()).filter(|k| !gables.contains(k)) {
        let run = &runs[k];
        let fr = run.fr;
        checked("detailed gutter run length", fr.length, 100.0, 200000.0)?;
        joints += (fr.length / d.joint_spacing_mm) as usize;
        if joints > 1024 {
            return Err(invalid("at most 1024 gutter joints"));
        }
        let start = if run.stop_start { d.wall_mm } else { 0.0 };
        let end = fr.length - if run.stop_end { d.wall_mm } else { 0.0 };
        let mut breaks = vec![start, end];
        let mine: Vec<_> = outlets.iter().filter(|(edge, _, _, _)| *edge == k).collect();
        for (_, u, _, p) in &mine {
            breaks.extend([u - p.diameter_mm / 2.0, u + p.diameter_mm / 2.0]);
        }
        breaks.sort_by(f64::total_cmp);
        breaks.dedup();
        let mut mesh = Mesh::default();
        for range in breaks.windows(2) {
            let mid = (range[0] + range[1]) / 2.0;
            let aperture = mine.iter().find(|(_, u, _, p)| (mid - u).abs() < p.diameter_mm / 2.0);
            channel(&mut mesh, run, &profile, range[0], range[1], aperture.map(|(_, _, _, p)| p.diameter_mm));
        }
        if run.stop_start {
            stop_end(&mut mesh, run, &profile, 0.0, start, true)?;
        }
        if run.stop_end {
            stop_end(&mut mesh, run, &profile, end, fr.length, false)?;
        }
        let (clear_start, clear_end) = run.straight_limits(&profile);
        for j in 1..=(fr.length / d.joint_spacing_mm) as usize {
            let u = j as f64 * d.joint_spacing_mm;
            if u - 25.0 > clear_start + d.wall_mm
                && u + 25.0 < clear_end - d.wall_mm
                && mine.iter().all(|(_, outlet, _, p)| (u - outlet).abs() > p.diameter_mm / 2.0 + 25.0)
            {
                sleeve(&mut mesh, run, u, r, profile.bead)?;
            }
        }
        let owner = if selected { kept[k] } else { k };
        parts.push(DrainPart { id: format!("{}.roof.gutter{owner}", b.id), class: DrainClass::Gutter, mesh });
        for (_, u, wall, p) in mine {
            let mut mesh = Mesh::default();
            let z = low - r;
            let socket = fr.point(*u, -r, z);
            // A short open outlet and two offset knees lead to the wall stand-off.
            let wall_pt = wall.point(p.along_mm, -p.wall_offset_mm, z - 500.0);
            let path = [
                socket,
                [socket[0], socket[1], z - 150.0],
                wall_pt,
                [wall_pt[0], wall_pt[1], p.bottom_mm + p.shoe_mm.max(50.0)],
                [wall_pt[0] + wall.n[0] * p.shoe_mm, wall_pt[1] + wall.n[1] * p.shoe_mm, p.bottom_mm],
            ];
            let stations = rounded_path(&path, p.diameter_mm / 2.0, d.wall_mm)?;
            tube_stations(&mut mesh, &stations, p.diameter_mm / 2.0, d.wall_mm)?;
            // Collars follow the same centreline, including short offsets/shoes. Separate,
            // closed sleeves have 1 mm radial clearance and cannot fold across a knee.
            let length = path_length(&stations);
            for distance in [length / 3.0, 2.0 * length / 3.0] {
                let collar = path_slice(&stations, distance - 20.0, distance + 20.0);
                tube_stations(&mut mesh, &collar, p.diameter_mm / 2.0 + 3.0, 2.0)?;
            }
            parts.push(DrainPart { id: format!("{}.roof.downpipe.{}", b.id, p.id), class: DrainClass::Downpipe, mesh });
        }
    }
    Ok(parts)
}

const CHANNEL_STEPS: usize = 32;
const PIPE_SIDES: usize = 32;

/// A single closed metal section, with the rolled beads integrated into the channel rather
/// than intersecting separate tubes. `s` is distance outward from the fascia, `v` relative Z.
struct ChannelProfile {
    points: Vec<[f64; 2]>,
    inner: usize,
    mouth_far: usize,
    mouth_near: usize,
    bead: f64,
}

impl ChannelProfile {
    fn new(r: f64, wall: f64) -> Self {
        let bead = 4_f64.max(wall + 1.0);
        // Intersections of each channel circle with the bead circle centred at its rim.
        let outer_s = bead * bead / (2.0 * r);
        let inner_s = (r * r + bead * bead - (r - wall).powi(2)) / (2.0 * r);
        let outer_angle = (1.0 - outer_s / r).acos();
        let inner_angle = ((r - inner_s) / (r - wall)).acos();
        let mut points: Vec<_> = (0..=CHANNEL_STEPS)
            .map(|i| {
                let t = outer_angle + (PI - 2.0 * outer_angle) * i as f64 / CHANNEL_STEPS as f64;
                [r - r * t.cos(), -r * t.sin()]
            })
            .collect();
        let mut angle = points[CHANNEL_STEPS][1].atan2(points[CHANNEL_STEPS][0] - 2.0 * r);
        for end in [0.0, PI / 2.0, PI] {
            profile_arc(&mut points, [2.0 * r, 0.0], bead, angle, end);
            angle = end;
        }
        let mouth_far = points.len() - 1;
        let inner_end = (-(r - wall) * inner_angle.sin()).atan2(-inner_s) + 2.0 * PI;
        profile_arc(&mut points, [2.0 * r, 0.0], bead, angle, inner_end);
        let inner = points.len() - 1;
        for i in 1..=CHANNEL_STEPS {
            let t = PI - inner_angle - (PI - 2.0 * inner_angle) * i as f64 / CHANNEL_STEPS as f64;
            points.push([r - (r - wall) * t.cos(), -(r - wall) * t.sin()]);
        }
        let last = points[points.len() - 1];
        let mut angle = last[1].atan2(last[0]);
        profile_arc(&mut points, [0.0, 0.0], bead, angle, 0.0);
        let mouth_near = points.len() - 1;
        angle = 0.0;
        let outer_end = points[0][1].atan2(points[0][0]) + 2.0 * PI;
        for end in [PI / 2.0, PI, 1.5 * PI, outer_end] {
            profile_arc(&mut points, [0.0, 0.0], bead, angle, end);
            angle = end;
        }
        points.pop(); // The closing edge reuses the original outer-circle intersection.
        Self { points, inner, mouth_far, mouth_near, bead }
    }
}

fn profile_arc(points: &mut Vec<[f64; 2]>, centre: [f64; 2], radius: f64, a: f64, b: f64) {
    let count = ((b - a).abs() / (PI / 16.0)).ceil().max(1.0) as usize;
    for i in 1..=count {
        let t = a + (b - a) * i as f64 / count as f64;
        points.push([centre[0] + radius * t.cos(), centre[1] + radius * t.sin()]);
    }
}

struct GutterRun {
    fr: FacadeFrame,
    start: [f64; 2],
    end: [f64; 2],
    end_origin: [f64; 2],
    stop_start: bool,
    stop_end: bool,
    low: f64,
}

impl GutterRun {
    fn point(&self, u: f64, p: [f64; 2]) -> [f64; 3] {
        let (origin, axis) = if u == 0.0 {
            (self.fr.origin, self.start)
        } else if u == self.fr.length {
            (self.end_origin, self.end)
        } else {
            ([self.fr.origin[0] + u * self.fr.t[0], self.fr.origin[1] + u * self.fr.t[1]], self.fr.n)
        };
        [origin[0] + p[0] * axis[0], origin[1] + p[0] * axis[1], self.low + p[1]]
    }

    fn straight_limits(&self, p: &ChannelProfile) -> (f64, f64) {
        let along = |axis: [f64; 2], s: f64| s * (axis[0] * self.fr.t[0] + axis[1] * self.fr.t[1]);
        let start = p.points.iter().map(|p| along(self.start, p[0])).fold(0.0, f64::max);
        let end = p.points.iter().map(|p| self.fr.length + along(self.end, p[0])).fold(self.fr.length, f64::min);
        (start, end)
    }

    fn validate(&self, p: &ChannelProfile, wall: f64) -> Result<()> {
        checked("detailed gutter run length", self.fr.length, 100.0, 200000.0)?;
        let (start, end) = self.straight_limits(p);
        if end - start < 2.0 * wall + 1.0 {
            return Err(invalid("gutter mitres consume the run"));
        }
        for &section in &p.points {
            let (a, b) = (self.point(0.0, section), self.point(self.fr.length, section));
            let along = (b[0] - a[0]) * self.fr.t[0] + (b[1] - a[1]) * self.fr.t[1];
            if along < 2.0 * wall + 1.0 || [a, b].iter().flatten().any(|v| !v.is_finite() || v.abs() > 1e8) {
                return Err(invalid("gutter mitres cross or leave site-local coordinates"));
            }
        }
        Ok(())
    }
}

fn sweep(m: &mut Mesh, run: &GutterRun, profile: &[[f64; 2]], start: f64, end: f64, skip: &[usize]) {
    for i in 0..profile.len() {
        if skip.contains(&i) {
            continue;
        }
        let j = (i + 1) % profile.len();
        m.quad(run.point(start, profile[i]), run.point(end, profile[i]), run.point(end, profile[j]), run.point(start, profile[j]));
    }
}

fn cap(m: &mut Mesh, run: &GutterRun, profile: &[[f64; 2]], u: f64, start: bool) -> Result<()> {
    let first = m.triangles.len();
    if !m.polygon(profile, &[], |p| run.point(u, p)) {
        return Err(invalid("drainage end face could not be triangulated"));
    }
    // The triangulator normalises polygon winding; orient the resulting triangles explicitly.
    for t in &mut m.triangles[first..] {
        let p = t.map(|i| m.positions[i as usize]);
        let n = cross(sub(p[1], p[0]), sub(p[2], p[0]));
        if (n[0] * run.fr.t[0] + n[1] * run.fr.t[1]) * if start { -1.0 } else { 1.0 } < 0.0 {
            t.swap(1, 2);
        }
    }
    Ok(())
}

fn stop_end(m: &mut Mesh, run: &GutterRun, p: &ChannelProfile, start: f64, end: f64, at_start: bool) -> Result<()> {
    // Fill the water channel only over the end plate's real thickness. The cavity face is
    // shared with the open channel; no coincident annular cap is hidden inside the assembly.
    let filled: Vec<_> = p.points[..=p.mouth_far].iter().chain(&p.points[p.mouth_near..]).copied().collect();
    let water: Vec<_> = p.points[p.mouth_far..=p.mouth_near].iter().rev().copied().collect();
    sweep(m, run, &filled, start, end, &[]);
    cap(m, run, &filled, if at_start { start } else { end }, at_start)?;
    cap(m, run, &water, if at_start { end } else { start }, !at_start)
}

fn channel(m: &mut Mesh, run: &GutterRun, p: &ChannelProfile, start: f64, end: f64, aperture: Option<f64>) {
    let r = (p.points[0][0] + p.points[CHANNEL_STEPS][0]) / 2.0;
    let removed: Vec<_> =
        (0..CHANNEL_STEPS).filter(|&k| aperture.is_some_and(|d| ((p.points[k][0] + p.points[k + 1][0]) / 2.0 - r).abs() < d / 2.0)).collect();
    let skip: Vec<_> = removed.iter().flat_map(|&k| [k, p.inner + CHANNEL_STEPS - k - 1]).collect();
    sweep(m, run, &p.points, start, end, &skip);
    // Close the thickness around all four sides of the aperture rather than leaving raw
    // mesh borders. This remains a presentation outlet, not a hydraulic connector/port.
    for &k in &removed {
        let (a, b, c, d) = (p.points[k], p.points[k + 1], p.points[p.inner + CHANNEL_STEPS - k - 1], p.points[p.inner + CHANNEL_STEPS - k]);
        m.quad(run.point(start, b), run.point(start, a), run.point(start, d), run.point(start, c));
        m.quad(run.point(end, a), run.point(end, b), run.point(end, c), run.point(end, d));
    }
    if let (Some(&first), Some(&last)) = (removed.first(), removed.last()) {
        for (k, reverse) in [(first, false), (last + 1, true)] {
            let (a, b) = (p.points[k], p.points[p.inner + CHANNEL_STEPS - k]);
            let mut pts = [run.point(start, a), run.point(end, a), run.point(end, b), run.point(start, b)];
            if reverse {
                pts.reverse();
            }
            m.quad(pts[0], pts[1], pts[2], pts[3]);
        }
    }
}

fn sleeve(m: &mut Mesh, run: &GutterRun, u: f64, r: f64, bead: f64) -> Result<()> {
    // A separate closed fitting, with 0.25 mm clearance to the sheet and clear of both rolls.
    let angle = ((bead + 2.0) / r).asin();
    let mut p = Vec::new();
    for (rad, reverse) in [(r + 1.25, false), (r + 0.25, true)] {
        for i in 0..=CHANNEL_STEPS {
            let t = angle + (PI - 2.0 * angle) * if reverse { CHANNEL_STEPS - i } else { i } as f64 / CHANNEL_STEPS as f64;
            p.push([r - rad * t.cos(), -rad * t.sin()]);
        }
    }
    sweep(m, run, &p, u - 25.0, u + 25.0, &[]);
    cap(m, run, &p, u - 25.0, true)?;
    cap(m, run, &p, u + 25.0, false)
}

fn sub(a: [f64; 3], b: [f64; 3]) -> [f64; 3] {
    std::array::from_fn(|k| a[k] - b[k])
}

fn dot(a: [f64; 3], b: [f64; 3]) -> f64 {
    a.iter().zip(b).map(|(a, b)| a * b).sum()
}

fn cross(a: [f64; 3], b: [f64; 3]) -> [f64; 3] {
    [a[1] * b[2] - a[2] * b[1], a[2] * b[0] - a[0] * b[2], a[0] * b[1] - a[1] * b[0]]
}

fn unit(v: [f64; 3]) -> Result<[f64; 3]> {
    let length = dot(v, v).sqrt();
    if !length.is_finite() || length < 1e-9 {
        return Err(invalid("drainage needs a nonzero finite direction"));
    }
    Ok(v.map(|v| v / length))
}

#[derive(Clone, Copy)]
struct Station {
    p: [f64; 3],
    t: [f64; 3],
}

/// Circular fillets tangent to the actual incoming/outgoing directions. Radius is normally
/// 1.5 diameters; tight paths reduce it consistently, leaving 10% straight clearance. A bend
/// smaller than the tube's outside radius plus wall is rejected instead of folded.
fn rounded_path(path: &[[f64; 3]], r: f64, wall: f64) -> Result<Vec<Station>> {
    checked("tube radius", r, 1.0, 200.0)?;
    checked("tube wall", wall, 0.1, r - 0.1)?;
    if path.len() < 2 || path.len() > 32 || path.iter().flatten().any(|v| !v.is_finite() || v.abs() > 1e8) {
        return Err(invalid("drainage needs 2 to 32 finite centreline points"));
    }
    let mut points = Vec::new();
    for &p in path {
        if points.last().is_none_or(|&prev| dot(sub(p, prev), sub(p, prev)) > 1e-12) {
            points.push(p);
        }
    }
    let mut i = 1;
    while i + 1 < points.len() {
        let before = unit(sub(points[i], points[i - 1]))?;
        let after = unit(sub(points[i + 1], points[i]))?;
        let c = dot(before, after).clamp(-1.0, 1.0);
        if c > 1.0 - 1e-12 {
            points.remove(i);
        } else if c < -1.0 + 1e-9 {
            return Err(invalid("a pipe centreline cannot reverse at a knee"));
        } else {
            i += 1;
        }
    }
    if points.len() < 2 {
        return Err(invalid("a pipe centreline needs a real run"));
    }
    let directions: Vec<_> = points.windows(2).map(|p| unit(sub(p[1], p[0]))).collect::<Result<_>>()?;
    let lengths: Vec<_> = points.windows(2).map(|p| dot(sub(p[1], p[0]), sub(p[1], p[0])).sqrt()).collect();
    let radius = 3.0 * r;
    let mut cuts = vec![0.0; points.len()];
    for i in 1..points.len() - 1 {
        let c = dot(directions[i - 1], directions[i]).clamp(-1.0, 1.0);
        cuts[i] = radius * ((1.0 - c) / (1.0 + c)).sqrt();
    }
    let scale = lengths
        .iter()
        .enumerate()
        .filter(|(i, _)| cuts[*i] + cuts[*i + 1] > 0.0)
        .map(|(i, length)| 0.9 * length / (cuts[i] + cuts[i + 1]))
        .fold(1.0, f64::min);
    let radius = radius * scale;
    if points.len() > 2 && radius <= r + wall {
        return Err(invalid("pipe segments are too short for nonintersecting elbows"));
    }
    let mut stations = vec![Station { p: points[0], t: directions[0] }];
    for i in 1..points.len() - 1 {
        let (before, after) = (directions[i - 1], directions[i]);
        let cut = cuts[i] * scale;
        let entry = std::array::from_fn(|k| points[i][k] - cut * before[k]);
        let exit = std::array::from_fn(|k| points[i][k] + cut * after[k]);
        let axis = unit(cross(before, after))?;
        let inward = cross(axis, before);
        let centre: [f64; 3] = std::array::from_fn(|k| entry[k] + radius * inward[k]);
        let radial = inward.map(|v| -v);
        let sweep = dot(before, after).clamp(-1.0, 1.0).acos();
        let steps = (sweep / (PI / 24.0)).ceil().max(1.0) as usize;
        stations.push(Station { p: entry, t: before });
        for j in 1..=steps {
            let a = sweep * j as f64 / steps as f64;
            let rotate = |v: [f64; 3]| {
                let w = cross(axis, v);
                std::array::from_fn(|k| v[k] * a.cos() + w[k] * a.sin())
            };
            let v = rotate(radial);
            stations.push(Station {
                p: if j == steps { exit } else { std::array::from_fn(|k| centre[k] + radius * v[k]) },
                t: if j == steps { after } else { rotate(before) },
            });
        }
    }
    stations.push(Station { p: points[points.len() - 1], t: directions[directions.len() - 1] });
    Ok(stations)
}

fn path_length(path: &[Station]) -> f64 {
    path.windows(2).map(|p| dot(sub(p[1].p, p[0].p), sub(p[1].p, p[0].p)).sqrt()).sum()
}

fn path_slice(path: &[Station], start: f64, end: f64) -> Vec<Station> {
    let mut out = Vec::new();
    let mut length = 0.0;
    for pair in path.windows(2) {
        let segment = dot(sub(pair[1].p, pair[0].p), sub(pair[1].p, pair[0].p)).sqrt();
        let sample = |distance: f64| {
            let u = ((distance - length) / segment).clamp(0.0, 1.0);
            Station {
                p: std::array::from_fn(|k| pair[0].p[k] + (pair[1].p[k] - pair[0].p[k]) * u),
                t: std::array::from_fn(|k| pair[0].t[k] + (pair[1].t[k] - pair[0].t[k]) * u),
            }
        };
        if start >= length && start < length + segment {
            out.push(sample(start));
        }
        if length + segment > start && length + segment < end {
            out.push(pair[1]);
        }
        if end > length && end <= length + segment {
            out.push(sample(end));
            break;
        }
        length += segment;
    }
    out
}

fn tube_stations(m: &mut Mesh, path: &[Station], r: f64, wall: f64) -> Result<()> {
    if path.len() < 2 {
        return Err(invalid("a drainage sleeve needs two sections"));
    }
    let reference = path.windows(2).map(|s| cross(s[0].t, s[1].t)).find(|n| dot(*n, *n) > 1e-12);
    let reference = unit(reference.unwrap_or_else(|| if path[0].t[0].abs() < 0.9 { [1., 0., 0.] } else { [0., 1., 0.] }))?;
    let mut sections = Vec::new();
    for s in path {
        let t = unit(s.t)?;
        let x = unit(std::array::from_fn(|k| reference[k] - dot(reference, t) * t[k]))?;
        let y = cross(t, x);
        let ring = |radius: f64| -> [[f64; 3]; PIPE_SIDES] {
            std::array::from_fn(|i| {
                let a = 2.0 * PI * i as f64 / PIPE_SIDES as f64;
                std::array::from_fn(|k| s.p[k] + radius * (x[k] * a.cos() + y[k] * a.sin()))
            })
        };
        sections.push((ring(r), ring(r - wall)));
    }
    for pair in sections.windows(2) {
        let [(a, ai), (b, bi)] = [pair[0], pair[1]];
        for k in 0..PIPE_SIDES {
            let j = (k + 1) % PIPE_SIDES;
            m.quad(a[k], a[j], b[j], b[k]);
            m.quad(ai[j], ai[k], bi[k], bi[j]);
        }
    }
    for (i, (a, b)) in sections.iter().enumerate().filter(|(i, _)| *i == 0 || *i + 1 == sections.len()) {
        for k in 0..PIPE_SIDES {
            let j = (k + 1) % PIPE_SIDES;
            if i == 0 {
                m.quad(a[j], a[k], b[k], b[j]);
            } else {
                m.quad(a[k], a[j], b[j], b[k]);
            }
        }
    }
    Ok(())
}

#[cfg(test)]
fn tube(m: &mut Mesh, path: &[[f64; 3]], r: f64, wall: f64) {
    tube_stations(m, &rounded_path(path, r, wall).unwrap(), r, wall).unwrap();
}

#[cfg(test)]
mod tests {
    use super::*;

    fn key(p: [f64; 3]) -> [i64; 3] {
        p.map(|v| (v * 1e6).round() as i64)
    }

    fn closed(m: &Mesh) {
        use std::collections::BTreeMap;
        let mut faces = BTreeSet::new();
        let mut edges = BTreeMap::<_, (usize, i32)>::new();
        assert!(!m.is_empty());
        assert!(m.positions.iter().flatten().all(|v| v.is_finite() && v.abs() <= 1e8));
        for t in &m.triangles {
            let p = t.map(|i| m.positions[i as usize]);
            let n = cross(sub(p[1], p[0]), sub(p[2], p[0]));
            assert!(dot(n, n) > 1e-16, "zero-area triangle: {p:?}");
            let mut face = p.map(key);
            face.sort();
            assert!(faces.insert(face), "duplicate or coincident face: {p:?}");
            for i in 0..3 {
                let (a, b) = (key(p[i]), key(p[(i + 1) % 3]));
                let e = edges.entry([a.min(b), a.max(b)]).or_default();
                e.0 += 1;
                e.1 += if a < b { 1 } else { -1 };
            }
        }
        let bad: Vec<_> = edges.iter().filter(|(_, (count, winding))| *count != 2 || *winding != 0).take(5).collect();
        assert!(bad.is_empty(), "nonmanifold/open/misoriented edges: {bad:?}");
        let volume: f64 = m
            .triangles
            .iter()
            .map(|t| {
                let [a, b, c] = t.map(|i| m.positions[i as usize]);
                dot(a, cross(b, c)) / 6.0
            })
            .sum();
        assert!(volume > 0.0, "closed metal must face outward: {volume}");
    }

    fn assembly(parts: &[DrainPart]) -> Mesh {
        let mut mesh = Mesh::default();
        for p in parts {
            mesh.append(&p.mesh);
        }
        mesh
    }

    #[test]
    fn selected_source_runs_keep_ids_outlets_and_mitres_after_notches_and_transformations() {
        let ring = [[0., 0.], [16000., 0.], [16000., 800.], [24000., 800.], [24000., 12800.], [28000., 12800.], [28000., 30000.], [0., 30000.]];
        for (angle, offset) in [(0_f64, [0., 0.]), (37.25, [150000., -270000.]), (-118., [-30000., 41000.])] {
            let (sin, cos) = angle.to_radians().sin_cos();
            let ring = ring.map(|[x, y]| [offset[0] + x * cos - y * sin, offset[1] + x * sin + y * cos]);
            let b = Building::new("b", "Synthetic selected drainage", &ring, 3, 3400.).unwrap();
            let ids = [1, 4, 5, 6, 7, 8].map(|i| format!("b.f{i}"));
            let (mut owners, points) = crate::roof::selected_eave_outline(&b, &ids).unwrap();
            let mut outline = crate::build::inner_ring_by(&points, |_| -250.);
            let d = Drainage {
                downpipes: vec![Downpipe {
                    id: "source-four".into(),
                    facade: "b.f4".into(),
                    along_mm: 6000.,
                    diameter_mm: 80.,
                    wall_offset_mm: 100.,
                    bottom_mm: 150.,
                    shoe_mm: 200.,
                }],
                ..Drainage::default()
            };
            let parts = build_selected(&b, &owners, &outline, 10200., 250., 46., &[], &d).unwrap();
            let gutters: Vec<_> = parts.iter().filter(|p| p.class == DrainClass::Gutter).map(|p| p.id.clone()).collect();
            assert_eq!(gutters, vec!["b.roof.gutter0", "b.roof.gutter3", "b.roof.gutter4", "b.roof.gutter5", "b.roof.gutter6", "b.roof.gutter7"]);
            assert!(parts.iter().any(|p| p.id == "b.roof.downpipe.source-four"));
            closed(&assembly(&parts));
            // Reordering only the cyclic starting corner cannot change semantic ownership.
            owners.rotate_left(2);
            outline.rotate_left(2);
            let shifted = build_selected(&b, &owners, &outline, 10200., 250., 46., &[], &d).unwrap();
            for part in &parts {
                assert_eq!(part, shifted.iter().find(|p| p.id == part.id).unwrap());
            }
            let mut bad = d.clone();
            bad.downpipes[0].facade = "b.f3".into();
            let err = build_selected(&b, &owners, &outline, 10200., 250., 46., &[], &bad).unwrap_err().to_string();
            assert!(err.contains("b.f3") && err.contains("absent") && err.contains("eave_facades"), "{err}");
            let k = owners.iter().position(|&e| e == 3).unwrap();
            let err = build_selected(&b, &owners, &outline, 10200., 250., 46., &[k], &d).unwrap_err().to_string();
            assert!(err.contains("b.f4") && err.contains("gable"), "{err}");
            for bad in [vec![], vec![0, 0, 4, 5, 6, 7], vec![0, 3, 4, 5, 6, usize::MAX], vec![0, 3, 4]] {
                assert!(build_selected(&b, &bad, &outline, 10200., 250., 46., &[], &d).is_err());
            }
            let mut wrong = owners.clone();
            wrong.rotate_left(1);
            assert!(
                build_selected(&b, &wrong, &outline, 10200., 250., 46., &[], &d).is_err(),
                "a known source must map to its actual parallel offset run"
            );
        }
        let b = Building::new("r", "Legacy drainage", &[[0., 0.], [12000., 0.], [12000., 8000.], [0., 8000.]], 2, 3000.).unwrap();
        let outline = crate::build::inner_ring_by(&b.footprint, |_| -180.);
        let d = Drainage::default();
        assert_eq!(
            build(&b, &[0, 1, 2, 3], &outline, 6000., 180., 46., &[], &d).unwrap(),
            build_selected(&b, &[0, 1, 2, 3], &outline, 6000., 180., 46., &[], &d).unwrap()
        );
    }

    #[test]
    fn architectural_corners_share_a_mitre_instead_of_disconnected_endcaps() {
        for outline in [
            vec![[0., 0.], [12000., 0.], [12000., 8000.], [0., 8000.]],
            vec![[0., 0.], [12000., 0.], [15000., 6000.], [0., 8000.]],
            vec![[0., 0.], [12000., 0.], [12000., 4000.], [8000., 4000.], [8000., 8000.], [0., 8000.]],
            vec![[0., 0.], [12000., 0.], [12000., 8000.], [8000., 8000.], [9000., 4000.], [0., 8000.]],
        ] {
            let b = Building::new("test", "Synthetic", &outline, 2, 3000.).unwrap();
            let kept: Vec<_> = (0..outline.len()).collect();
            let parts = build(&b, &kept, &outline, 6000., 0., 35., &[], &Drainage::default()).unwrap();
            closed(&assembly(&parts));
            assert_eq!(parts, build(&b, &kept, &outline, 6000., 0., 35., &[], &Drainage::default()).unwrap());
            for k in 0..parts.len() {
                let a: BTreeSet<_> = parts[k].mesh.positions.iter().copied().map(key).collect();
                let c: BTreeSet<_> = parts[(k + 1) % parts.len()].mesh.positions.iter().copied().map(key).collect();
                assert!(a.intersection(&c).count() > 60, "corner {k} must share the complete metal profile, not two isolated ends");
                let seam: BTreeSet<_> = a.intersection(&c).copied().collect();
                for part in [&parts[k], &parts[(k + 1) % parts.len()]] {
                    assert!(
                        part.mesh.triangles.iter().all(|t| !t.iter().all(|i| seam.contains(&key(part.mesh.positions[*i as usize])))),
                        "connected corners must not contain hidden or overlapping endcaps"
                    );
                }
            }
        }
    }

    #[test]
    fn pipe_facets_do_not_generate_longitudinal_crease_lines() {
        use std::collections::BTreeMap;
        let mut m = Mesh::default();
        tube(&mut m, &[[0., 0., 2000.], [0., 0., 0.]], 40., 2.);
        let mut edges = BTreeMap::<_, Vec<[f64; 3]>>::new();
        for t in &m.triangles {
            let p = t.map(|i| m.positions[i as usize]);
            let a: [f64; 3] = std::array::from_fn(|i| p[1][i] - p[0][i]);
            let b: [f64; 3] = std::array::from_fn(|i| p[2][i] - p[0][i]);
            let n = [a[1] * b[2] - a[2] * b[1], a[2] * b[0] - a[0] * b[2], a[0] * b[1] - a[1] * b[0]];
            let length = n.iter().map(|v| v * v).sum::<f64>().sqrt();
            let n = n.map(|v| v / length);
            for i in 0..3 {
                let mut e = [key(p[i]), key(p[(i + 1) % 3])];
                e.sort();
                if (e[0][2] - e[1][2]).abs() > 1_000_000 {
                    edges.entry(e).or_default().push(n);
                }
            }
        }
        assert!(!edges.is_empty());
        for normals in edges.values() {
            assert_eq!(normals.len(), 2);
            let dot: f64 = normals[0].iter().zip(normals[1]).map(|(a, b)| a * b).sum();
            assert!(dot.abs() > 20_f64.to_radians().cos(), "a round pipe must stay below Studio's 20-degree crease threshold");
        }
    }

    #[test]
    fn outlet_and_short_offset_shoe_assemblies_have_closed_oriented_metal() {
        let b = building();
        for (overhang, offset, diameter, shoe, bottom) in [
            (0., 75., 80., 0_f64, 150.),
            (180., 110., 80., 180., 150.),
            (0., 1000., 112.5, 1., 150.),
            (2000., 76.25, 112.5, 500., 150.),
            (0., 100., 80., 500., 6000. - 75. - 1080.),
            (180., 110., 50., 50., 150.),
        ] {
            let d = Drainage {
                downpipes: vec![Downpipe {
                    id: "rain-a".into(),
                    facade: "test.f1".into(),
                    along_mm: 3000.,
                    diameter_mm: diameter,
                    wall_offset_mm: offset,
                    bottom_mm: bottom,
                    shoe_mm: shoe,
                }],
                ..Default::default()
            };
            let outline = crate::build::inner_ring(&b.footprint, -overhang);
            let parts = build(&b, &[0, 1, 2, 3], &outline, 6000., overhang, 35., &[1, 3], &d).unwrap();
            for part in &parts {
                closed(&part.mesh);
            }
            assert_eq!(parts, build(&b, &[0, 1, 2, 3], &outline, 6000., overhang, 35., &[1, 3], &d).unwrap());
        }
    }

    #[test]
    fn elbows_are_circular_tangent_and_keep_nominal_terminal_sections() {
        let path = [[0., 0., 2000.], [0., 0., 1800.], [0., 300., 1500.], [0., 300., 400.], [0., 500., 200.]];
        let stations = rounded_path(&path, 40., 2.).unwrap();
        assert_eq!(stations.first().unwrap().p, path[0]);
        assert_eq!(stations.last().unwrap().p, path[4]);
        let curved: Vec<_> = stations.windows(2).filter(|s| dot(s[0].t, s[1].t) < 1.0 - 1e-10).collect();
        assert!(curved.len() >= 18, "all three knees need curved, rather than mitred, sections");
        for s in curved {
            let c = dot(s[0].t, s[1].t).clamp(-1.0, 1.0);
            assert!(c >= (PI / 24.0).cos() - 1e-9);
            let chord = sub(s[1].p, s[0].p);
            let radius = dot(chord, chord).sqrt() / (2.0 * ((1.0 - c) / 2.0).sqrt());
            assert!((radius - 120.).abs() < 1e-7, "centreline radius is 1.5 nominal diameters");
            let direction = unit(chord).unwrap();
            let bisector = unit(std::array::from_fn(|k| s[0].t[k] + s[1].t[k])).unwrap();
            assert!(dot(direction, bisector) > 1.0 - 1e-10, "arc samples must follow their exact tangents");
        }
        let mut mesh = Mesh::default();
        tube_stations(&mut mesh, &stations, 40., 2.).unwrap();
        closed(&mesh);
    }

    #[test]
    fn detailed_drainage_preserves_old_roof_documents_defaults_and_ids() {
        let legacy = r#"{"kind":"pitched","pitch_deg":35,"gables":[],"gutters":true}"#;
        let mut b = building();
        b.roof = serde_json::from_str(legacy).unwrap();
        let mut doc = crate::Document::default();
        doc.buildings.push(b);
        let before = crate::build(&doc).unwrap();
        let encoded = serde_json::to_string(&doc).unwrap();
        assert!(!encoded.contains("drainage"));
        assert_eq!(before, crate::build(&serde_json::from_str(&encoded).unwrap()).unwrap());
        if let crate::Roof::Pitched { drainage, .. } = &mut doc.buildings[0].roof {
            *drainage = Some(serde_json::from_str(r#"{}"#).unwrap());
        }
        let detail = crate::build(&doc).unwrap();
        let (old_roof, new_roof) = (before[0].roof.as_ref().unwrap(), detail[0].roof.as_ref().unwrap());
        assert_eq!(old_roof.cover, new_roof.cover);
        assert_eq!(old_roof.id, new_roof.id);
        let ids: Vec<_> = new_roof.drainage.iter().map(|p| p.id.as_str()).collect();
        assert_eq!(ids, ["test.roof.gutter0", "test.roof.gutter1", "test.roof.gutter2", "test.roof.gutter3"]);
        let mut changed = doc.clone();
        if let crate::Roof::Pitched { drainage: Some(d), .. } = &mut changed.buildings[0].roof {
            d.gutter_width_mm = 200.;
        }
        assert_eq!(
            crate::build(&changed).unwrap()[0].roof.as_ref().unwrap().drainage.iter().map(|p| &p.id).collect::<Vec<_>>(),
            new_roof.drainage.iter().map(|p| &p.id).collect::<Vec<_>>()
        );
        if let crate::Roof::Pitched { drainage, .. } = &mut doc.buildings[0].roof {
            *drainage = None;
        }
        assert_eq!(before, crate::build(&doc).unwrap());
    }

    #[test]
    fn short_reentrant_and_reversing_corners_are_rejected_instead_of_folded() {
        let b = building();
        for outline in [
            vec![[0., 0.], [12000., 0.], [12000., 4000.], [8000., 4000.], [8000., 4100.], [0., 4100.]],
            vec![[0., 0.], [12000., 0.], [0., 0.001], [0., 8000.]],
            vec![[0., 0.], [12000., 0.], [12000., 0.1], [0., 8000.]],
        ] {
            let mut b = b.clone();
            b.footprint = outline.clone();
            let kept: Vec<_> = (0..outline.len()).collect();
            assert!(build(&b, &kept, &outline, 6000., 0., 35., &[], &Drainage::default()).is_err());
        }
        for path in [
            vec![],
            vec![[0.; 3]],
            vec![[0.; 3]; 3],
            vec![[0., 0., 0.], [0., 0., 10.], [0., 0., 0.]],
            vec![[0., 0., 0.], [0., 0., 10.], [0., 10., 10.]],
            vec![[0., 0., 0.], [0., f64::NAN, 10.]],
        ] {
            assert!(rounded_path(&path, 40., 2.).is_err());
        }
    }

    fn building() -> Building {
        Building::new("test", "Synthetic", &[[0., 0.], [12000., 0.], [12000., 8000.], [0., 8000.]], 2, 3000.).unwrap()
    }
    #[test]
    fn channels_are_open_deterministic_and_no_pipe_is_guessed() {
        let b = building();
        let d = Drainage::default();
        let parts = build(&b, &[0, 1, 2, 3], &b.footprint, 6000., 0., 35., &[1, 3], &d).unwrap();
        assert_eq!(parts.len(), 2);
        assert!(parts.iter().all(|p| p.class == DrainClass::Gutter));
        assert_eq!(parts, build(&b, &[0, 1, 2, 3], &b.footprint, 6000., 0., 35., &[1, 3], &d).unwrap());
        assert!(parts[0].mesh.positions.iter().any(|p| (p[1] + 75.).abs() < 0.01 && (p[2] - 5925.).abs() < 0.01));
        // No sheet spans the open mouth. Old solid boxes fail this section check.
        assert!(parts[0].mesh.triangles.iter().all(|t| {
            let pts = t.map(|i| parts[0].mesh.positions[i as usize]);
            // The 2 mm terminal stop plates intentionally close the water channel at gables.
            // Its mouth stays open everywhere between those plates.
            !(pts.iter().all(|p| p[0] > d.wall_mm && p[0] < 12000. - d.wall_mm && (p[2] - 6000.).abs() < 0.01)
                && pts.iter().map(|p| p[1]).fold(f64::MIN, f64::max) - pts.iter().map(|p| p[1]).fold(f64::MAX, f64::min) > 50.)
        }));
        closed(&assembly(&parts));
    }
    #[test]
    fn downpipe_bends_keep_sections_aligned_in_the_bend_plane() {
        let path = [[0., 0., 2000.], [0., 0., 1800.], [0., 300., 1500.], [0., 300., 400.], [0., 500., 200.]];
        let mut mesh = Mesh::default();
        tube(&mut mesh, &path, 40., 2.);
        // Every corresponding radial sample keeps its distance from the bend plane.
        // Arbitrary 90-degree frame changes produce twisted, flattened knees in the render.
        for pair in mesh.positions[..(path.len() - 1) * 16 * 8].chunks(8) {
            assert!((pair[0][0] - pair[3][0]).abs() < 1e-9);
        }
    }

    #[test]
    fn downpipe_rejects_reversed_shoe_runs_and_overlapping_collars() {
        let b = building();
        let z = 6000. - 75.;
        for (shoe, bottom) in [(500., z - 800.), (0., z - 800.), (100., z - 900.)] {
            let d = Drainage {
                downpipes: vec![Downpipe {
                    id: "p".into(),
                    facade: "test.f1".into(),
                    along_mm: 3000.,
                    diameter_mm: 80.,
                    wall_offset_mm: 100.,
                    bottom_mm: bottom,
                    shoe_mm: shoe,
                }],
                ..Default::default()
            };
            assert!(
                build(&b, &[0, 1, 2, 3], &b.footprint, 6000., 0., 35., &[1, 3], &d).is_err(),
                "shoe {shoe}, bottom {bottom} must not fold or overlap collars"
            );
        }
        for shoe in [0_f64, 50., 100., 500.] {
            let bottom = (z - 500. - shoe.max(50.) - 80.).min(z - 940.);
            let d = Drainage {
                downpipes: vec![Downpipe {
                    id: "p".into(),
                    facade: "test.f1".into(),
                    along_mm: 3000.,
                    diameter_mm: 80.,
                    wall_offset_mm: 100.,
                    bottom_mm: bottom,
                    shoe_mm: shoe,
                }],
                ..Default::default()
            };
            assert!(build(&b, &[0, 1, 2, 3], &b.footprint, 6000., 0., 35., &[1, 3], &d).is_ok(), "valid lower bound for shoe {shoe}");
        }
    }

    #[test]
    fn hostile_detail_is_rejected_without_panics() {
        let b = building();
        for bad in [f64::NAN, f64::INFINITY, f64::NEG_INFINITY, f64::MAX, -1., 0.] {
            for d in [
                Drainage { gutter_width_mm: bad, ..Default::default() },
                Drainage { wall_mm: bad, ..Default::default() },
                Drainage { joint_spacing_mm: bad, ..Default::default() },
            ] {
                assert!(build(&b, &[0, 1, 2, 3], &b.footprint, 6000., 0., 35., &[], &d).is_err());
            }
        }
        assert!(build(&b, &[], &[], 6000., 0., 35., &[], &Drainage::default()).is_err());
        assert!(build(&b, &[0, 1, 2, 3], &[[f64::NAN; 2]; 4], 6000., 0., 35., &[], &Drainage::default()).is_err());
        for bad in [f64::NAN, f64::INFINITY, f64::NEG_INFINITY, f64::MAX] {
            for field in 0..6 {
                let mut p = Downpipe {
                    id: "rain-a".into(),
                    facade: "test.f1".into(),
                    along_mm: 3000.,
                    diameter_mm: 80.,
                    wall_offset_mm: 100.,
                    bottom_mm: 150.,
                    shoe_mm: 180.,
                };
                match field {
                    0 => p.along_mm = bad,
                    1 => p.diameter_mm = bad,
                    2 => p.wall_offset_mm = bad,
                    3 => p.bottom_mm = bad,
                    4 => p.shoe_mm = bad,
                    _ => {
                        assert!(build(&b, &[0, 1, 2, 3], &b.footprint, bad, 0., 35., &[], &Drainage::default()).is_err());
                        continue;
                    }
                }
                let d = Drainage { downpipes: vec![p], ..Default::default() };
                assert!(build(&b, &[0, 1, 2, 3], &b.footprint, 6000., 0., 35., &[], &d).is_err());
            }
        }
        assert!(build(&b, &[0, 1, 2, usize::MAX], &b.footprint, 6000., 0., 35., &[], &Drainage::default()).is_err());
        assert!(build(&b, &[0, 1, 2, 3], &b.footprint, 6000., 0., 35., &[usize::MAX], &Drainage::default()).is_err());
    }
}
