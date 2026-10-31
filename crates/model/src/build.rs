//! Model → geometry. Deterministic: the same document always gives the same geometry.
//!
//! Walls are 2.5D: per storey, one prism per facade between the outer wall line and its mitred
//! inward offset at that storey's thickness (`walls`), and interior walls joined face to face
//! (`joins`). Openings are cut as 2D holes in the wall faces. No 3D booleans are needed.

use crate::component::{ComponentKind, ComponentType, Deck, Finish, Railing, Screens, Side};
use crate::doc::{Building, Document, Facade, FacadeKind, Placement, Storey};
use crate::joins::{Axis, Join};
use crate::mesh::{Material, Mesh};
use crate::walls::{Span, WallSegment};
use crate::{ModelError, Result};

/// Window and door frames sit this far behind the outer wall face, mm.
pub const REVEAL: f64 = 120.0;
const FRAME: f64 = 60.0;
const FRAME_DEPTH: f64 = 70.0;
/// Wall kept between an opening and the inner face of the wall it meets at a corner, mm. An
/// opening may come no closer than this to the corner's inner face, so it never cuts the
/// mitre (and the other facade); the thick lower walls of Copenhagen masonry leave little room.
const END_MARGIN: f64 = 50.0;
/// Clear wall kept between openings, mm.
const GAP: f64 = 100.0;
const LINTEL: f64 = 150.0;
/// Floor build-up (joists or concrete with the floor on top), mm.
pub const FLOOR_SLAB: f64 = 200.0;
const ROOF_SLAB: f64 = 300.0;
const BALCONY_SLAB: f64 = 180.0;

/// A facade's local frame. `u` runs along the outer wall line from its start, `w` is depth
/// measured inward from the outer face (negative = outside), `v` is height (world Z).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct FacadeFrame {
    pub origin: [f64; 2],
    pub t: [f64; 2],
    /// Outward normal.
    pub n: [f64; 2],
    pub length: f64,
}

impl FacadeFrame {
    pub fn new(a: [f64; 2], b: [f64; 2]) -> Option<Self> {
        let d = [b[0] - a[0], b[1] - a[1]];
        let length = (d[0] * d[0] + d[1] * d[1]).sqrt();
        if !(length > 1.0) {
            return None;
        }
        let t = [d[0] / length, d[1] / length];
        // Counter-clockwise footprint: the outside is on the right of the edge direction.
        Some(Self { origin: a, t, n: [t[1], -t[0]], length })
    }

    pub fn point(&self, u: f64, w: f64, v: f64) -> [f64; 3] {
        [self.origin[0] + u * self.t[0] - w * self.n[0], self.origin[1] + u * self.t[1] - w * self.n[1], v]
    }

    /// Outward compass bearing in degrees (0 = north, 90 = east).
    pub fn bearing(&self) -> f64 {
        self.n[0].atan2(self.n[1]).to_degrees().rem_euclid(360.0)
    }
}

/// A rectangle in a facade's (u, v) plane.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Rect {
    pub u0: f64,
    pub u1: f64,
    pub v0: f64,
    pub v1: f64,
}

impl Rect {
    fn overlaps(&self, o: &Rect, gap: f64) -> bool {
        self.u0 < o.u1 + gap && o.u0 < self.u1 + gap && self.v0 < o.v1 + gap && o.v0 < self.v1 + gap
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ElementClass {
    Window,
    Door,
    Railing,
    BalconySlab,
}

/// An element's state in the phase being viewed (Revit's phase status).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Status {
    /// Built in an earlier phase (or in the first phase) and still standing.
    Existing,
    /// Built in the phase being viewed.
    New,
    /// Torn down in the phase being viewed.
    Demolished,
}

impl Status {
    pub fn name(self) -> &'static str {
        match self {
            Status::Existing => "existing",
            Status::New => "new",
            Status::Demolished => "demolished",
        }
    }
}

/// Which elements a view shows, as Revit's phase filters.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PhaseFilter {
    /// Existing and new: the building as it stands at the end of the phase.
    Complete,
    /// Existing and what this phase tears down.
    Demolition,
    /// Only what this phase builds.
    New,
    /// Existing, new and demolished.
    All,
}

/// A phase and a filter.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct View {
    pub phase: usize,
    pub filter: PhaseFilter,
}

impl View {
    /// The complete building at the end of the last phase.
    pub fn latest(doc: &Document) -> Self {
        View { phase: doc.phases.len().saturating_sub(1), filter: PhaseFilter::Complete }
    }

    /// The status of an element living in phases `c..d` in this view, or `None` if hidden.
    pub fn status(&self, (c, d): (usize, usize)) -> Option<Status> {
        let k = self.phase;
        let status = if c <= k && k < d {
            if c == k && k > 0 { Status::New } else { Status::Existing }
        } else if d == k {
            Status::Demolished
        } else {
            return None;
        };
        let shown = match self.filter {
            PhaseFilter::Complete => status != Status::Demolished,
            PhaseFilter::Demolition => status != Status::New,
            PhaseFilter::New => status == Status::New,
            PhaseFilter::All => true,
        };
        shown.then_some(status)
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct BuiltElement {
    pub id: String,
    pub name: String,
    pub class: ElementClass,
    pub type_id: String,
    pub level: usize,
    pub width: f64,
    pub height: f64,
    pub parts: Vec<(Material, Mesh)>,
    pub status: Status,
}

#[derive(Debug, Clone, PartialEq)]
pub struct BuiltOpening {
    pub id: String,
    pub level: usize,
    pub rect: Rect,
    /// The window or door filling the opening.
    pub fill: BuiltElement,
    /// Railings and balcony slabs that belong to the opening.
    pub attached: Vec<BuiltElement>,
    pub status: Status,
}

/// One facade's wall, all storeys.
#[derive(Debug, Clone, PartialEq)]
pub struct BuiltWall {
    pub id: String,
    pub frame: FacadeFrame,
    pub kind: FacadeKind,
    /// Top of the wall (eaves), mm.
    pub height: f64,
    /// Bottom of the wall, mm (below the terrain with a basement).
    pub bottom: f64,
    /// The whole wall as one surface (no faces where storeys meet), for display and elevations.
    pub mesh: Mesh,
    /// One closed piece per storey at that storey's thickness, bottom up.
    pub segments: Vec<WallSegment>,
    /// The plinth in front of the wall's foot (empty without one).
    pub plinth: Mesh,
    pub openings: Vec<BuiltOpening>,
    /// Balconies hung on the wall without an opening of their own.
    pub attachments: Vec<BuiltElement>,
}

impl BuiltWall {
    /// The piece on a storey.
    pub fn segment(&self, level: usize) -> Option<&WallSegment> {
        self.segments.iter().find(|s| s.level == level)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SlabKind {
    Floor,
    Roof,
}

#[derive(Debug, Clone, PartialEq)]
pub struct BuiltSlab {
    pub id: String,
    pub kind: SlabKind,
    pub level: usize,
    pub polygon: Vec<[f64; 2]>,
    pub holes: Vec<Vec<[f64; 2]>>,
    pub z_bottom: f64,
    pub thickness: f64,
    pub mesh: Mesh,
}

#[derive(Debug, Clone, PartialEq)]
pub struct BuiltInterior {
    pub id: String,
    pub level: usize,
    /// Centre line after joining, site mm.
    pub axis: [[f64; 2]; 2],
    pub thickness: f64,
    /// Plan outline after joining (4 corners, counter-clockwise, site mm).
    pub outline: [[f64; 2]; 4],
    /// What the ends at `axis[0]` and `axis[1]` join.
    pub ends: [Option<Join>; 2],
    /// The wall's own frame: `u` along its right side from the start, `w` across into the wall.
    pub frame: FacadeFrame,
    /// Doors, as openings in the wall's frame.
    pub doors: Vec<BuiltOpening>,
    /// The plan as cut 1 m above the floor: the outline with the doors left open.
    pub cut: Vec<Vec<[f64; 2]>>,
    pub z_bottom: f64,
    pub height: f64,
    pub load_bearing: bool,
    pub mesh: Mesh,
}

#[derive(Debug, Clone, PartialEq)]
pub struct BuiltRoof {
    pub id: String,
    /// The roof surface.
    pub cover: Mesh,
    /// Vertical gable triangles above gable walls.
    pub gables: Mesh,
    /// Gutters along the eaves.
    pub gutters: Mesh,
    pub eaves: f64,
    pub ridge: f64,
    pub pitch_deg: f64,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Built {
    pub building_id: String,
    pub name: String,
    pub storeys: Vec<Storey>,
    pub walls: Vec<BuiltWall>,
    pub slabs: Vec<BuiltSlab>,
    /// A pitched roof; a flat roof is a slab in `slabs`.
    pub roof: Option<BuiltRoof>,
    pub interior: Vec<BuiltInterior>,
    pub stairs: Vec<BuiltStair>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct BuiltStair {
    pub id: String,
    pub level: usize,
    pub kind: crate::StairKind,
    pub winding: bool,
    pub mesh: Mesh,
}

/// Builds every building as it stands at the end of the last phase.
pub fn build(doc: &Document) -> Result<Vec<Built>> {
    build_view(doc, View::latest(doc))
}

/// Builds every building as seen in a phase view.
pub fn build_view(doc: &Document, view: View) -> Result<Vec<Built>> {
    build_cut(doc, view, None)
}

/// Builds every building as seen in a phase view, cut off at height `cut` (mm above the
/// terrain) when given: a 3D plan cut. Walls are closed exactly at the cut; what lies above it
/// (upper storeys, the roof) is left out and what crosses it is clipped.
pub fn build_cut(doc: &Document, view: View, cut: Option<f64>) -> Result<Vec<Built>> {
    if view.phase >= doc.phases.len().max(1) {
        return Err(ModelError::Invalid(format!("phase {} doesn't exist", view.phase)));
    }
    if let Some(z) = cut {
        crate::checked("cut height", z, -50_000.0, 500_000.0)?;
    }
    doc.buildings.iter().map(|b| build_building(doc, b, view, cut)).collect()
}

fn clip_element(mut e: BuiltElement, cut: Option<f64>) -> BuiltElement {
    if let Some(z) = cut {
        for (_, m) in &mut e.parts {
            *m = m.below(z);
        }
    }
    e
}

/// The inner wall line: each footprint vertex moved inward by `d` along the mitre of its two
/// edges. Very sharp corners fall back to a plain offset so the mitre can't run away. An edge too
/// short for the wall (a small jog in a thick wall) shrinks to a point where its neighbours'
/// inner lines meet, so the line never turns inside out; such an edge's facade wall is a wedge.
pub fn inner_ring(ring: &[[f64; 2]], d: f64) -> Vec<[f64; 2]> {
    inner_ring_by(ring, |_| d)
}

/// The inner wall line with each edge's wall at its own thickness `d(edge)`.
pub fn inner_ring_by(ring: &[[f64; 2]], d: impl Fn(usize) -> f64) -> Vec<[f64; 2]> {
    let n = ring.len();
    let offset = |i: usize| -> Option<([f64; 2], [f64; 2])> {
        let e = FacadeFrame::new(ring[i % n], ring[(i + 1) % n])?;
        let (p, d) = (ring[i % n], d(i % n));
        Some(([p[0] - e.n[0] * d, p[1] - e.n[1] * d], e.t))
    };
    // Where the inner lines of edges `i` and `j` meet.
    let meet = |i: usize, j: usize, fallback: [f64; 2]| -> [f64; 2] {
        let (Some((p0, t0)), Some((p1, t1))) = (offset(i), offset(j)) else { return fallback };
        let cross = t0[0] * t1[1] - t0[1] * t1[0];
        if cross.abs() < 1e-6 {
            return fallback;
        }
        let dp = [p1[0] - p0[0], p1[1] - p0[1]];
        let s = (dp[0] * t1[1] - dp[1] * t1[0]) / cross;
        let q = [p0[0] + s * t0[0], p0[1] + s * t0[1]];
        let miter = ((q[0] - fallback[0]).powi(2) + (q[1] - fallback[1]).powi(2)).sqrt();
        if miter > 5.0 * d(i % n).abs().max(d(j % n).abs()) { fallback } else { q }
    };
    let mut inner: Vec<[f64; 2]> = (0..n)
        .map(|i| {
            let p = ring[i];
            let plain = match FacadeFrame::new(p, ring[(i + 1) % n]) {
                Some(e1) => [p[0] - e1.n[0] * d(i), p[1] - e1.n[1] * d(i)],
                None => return p,
            };
            meet((i + n - 1) % n, i, plain)
        })
        .collect();
    if n < 4 {
        return inner;
    }
    // Collapse edges whose inner line runs backwards, one at a time, until none does.
    let mut alive = vec![true; n];
    for _ in 0..n {
        let reversed = (0..n).find(|&i| {
            let (a, b, p, q) = (ring[i], ring[(i + 1) % n], inner[i], inner[(i + 1) % n]);
            let along = (b[0] - a[0]) * (q[0] - p[0]) + (b[1] - a[1]) * (q[1] - p[1]);
            alive[i] && along < 0.0
        });
        let Some(i) = reversed else { break };
        alive[i] = false;
        if alive.iter().filter(|a| **a).count() < 3 {
            break;
        }
        let prev = (1..n).map(|k| (i + n - k) % n).find(|&k| alive[k]).unwrap_or(i);
        let next = (1..n).map(|k| (i + k) % n).find(|&k| alive[k]).unwrap_or(i);
        // Vertices from the end of `prev` to the start of `next` all move to where those two meet.
        let mid = [(inner[i][0] + inner[(i + 1) % n][0]) / 2.0, (inner[i][1] + inner[(i + 1) % n][1]) / 2.0];
        let x = meet(prev, next, mid);
        let mut v = (prev + 1) % n;
        loop {
            inner[v] = x;
            if v == next {
                break;
            }
            v = (v + 1) % n;
        }
    }
    inner
}

/// One resolved opening: the placement, its type and its rectangle.
#[derive(Debug, Clone, PartialEq)]
pub struct Resolved {
    pub placement: Placement,
    pub ty: ComponentType,
    /// In the facade's (u, v) plane: the opening, or for an attachment its extent from the
    /// underside of the slab to the top of the railing.
    pub rect: Rect,
    /// Phases it exists in (`c..d`).
    pub lifetime: (usize, usize),
}

impl Resolved {
    pub fn is_opening(&self) -> bool {
        self.ty.kind.is_opening()
    }
}

/// Underside of an attachment below its storey's floor, mm.
fn attachment_drop(kind: &ComponentKind) -> f64 {
    match kind {
        ComponentKind::BalconySlab { thickness, .. } => *thickness,
        ComponentKind::SteelBalcony { .. } => STEEL_FRAME,
        _ => 0.0,
    }
}

const STEEL_FRAME: f64 = 160.0;

/// Resolves and checks a facade's placements: each must fit its storey and the wall, and none
/// may overlap another. The errors say which slot is wrong and why.
pub fn resolve_facade(doc: &Document, b: &Building, f: &Facade) -> Result<Vec<Resolved>> {
    let (a, e) = b.edge(f.edge).ok_or_else(|| ModelError::Invalid(format!("facade {} has no edge", f.id)))?;
    let frame = FacadeFrame::new(a, e).ok_or_else(|| ModelError::Invalid(format!("facade {} has zero length", f.id)))?;
    if f.kind == FacadeKind::Party && !f.placements.is_empty() {
        return Err(ModelError::Invalid(format!("facade {} is a party wall and can't have openings", f.id)));
    }
    let n = b.footprint.len();
    let along = |q: [f64; 2]| (q[0] - a[0]) * frame.t[0] + (q[1] - a[1]) * frame.t[1];
    // Where the inner face starts and ends on each storey (the wall is thicker lower down).
    let inner_lines: Vec<(f64, f64)> = (0..b.storeys.len())
        .map(|l| {
            let inner = b.inner_line(l);
            match (inner.get(f.edge), inner.get((f.edge + 1) % n)) {
                (Some(&s), Some(&t)) => (along(s), along(t)),
                _ => (0.0, frame.length),
            }
        })
        .collect();

    let mut out: Vec<Resolved> = Vec::with_capacity(f.placements.len());
    for p in &f.placements {
        let slot = format!("{} level {} bay {}", f.id, p.level, p.bay);
        let lifetime = doc.lifetime(p).map_err(|e| ModelError::Invalid(format!("{slot}: {e}")))?;
        let ty = doc.component(&p.type_id)?.clone();
        let storey = b.storeys.get(p.level).ok_or_else(|| ModelError::Invalid(format!("{slot}: the building has {} storeys", b.storeys.len())))?;
        let c = *f.bays.get(p.bay).ok_or_else(|| ModelError::Invalid(format!("{slot}: the facade has {} bays", f.bays.len())))?;
        let mut ty_eff = ty.clone();
        ty_eff.width = p.width.unwrap_or(ty.width);
        ty_eff.height = p.height.unwrap_or(ty.height);
        ty_eff.sill = p.sill.unwrap_or(ty.sill);
        // The sill may go below the floor (down to the terrain); everything else as for the type.
        let mut check = ty_eff.clone();
        check.sill = check.sill.max(0.0);
        check.validate().map_err(|e| ModelError::Invalid(format!("{slot}: {e}")))?;
        if !ty_eff.sill.is_finite() || ty_eff.sill < -storey.elevation {
            return Err(ModelError::Invalid(format!(
                "{slot}: sill {} mm would put the opening below the terrain (the floor is {:.0} mm above it)",
                ty_eff.sill, storey.elevation
            )));
        }
        let opening = ty_eff.kind.is_opening();
        let (in0, in1) = inner_lines.get(p.level).copied().unwrap_or((0.0, frame.length));
        let lo = in0.max(0.0) + END_MARGIN;
        let hi = in1.min(frame.length) - END_MARGIN;
        let rect = if opening {
            Rect {
                u0: c - ty_eff.width / 2.0,
                u1: c + ty_eff.width / 2.0,
                v0: storey.elevation + ty_eff.sill,
                v1: storey.elevation + ty_eff.sill + ty_eff.height,
            }
        } else {
            let deck = storey.elevation + ty_eff.sill;
            Rect { u0: c - ty_eff.width / 2.0, u1: c + ty_eff.width / 2.0, v0: deck - attachment_drop(&ty_eff.kind), v1: deck + ty_eff.height }
        };
        if opening {
            if rect.u0 < lo || rect.u1 > hi {
                return Err(ModelError::Invalid(format!(
                    "{slot}: {} mm wide at {c:.0} mm doesn't fit the wall ({lo:.0} to {hi:.0} mm along the facade)",
                    ty_eff.width
                )));
            }
            let top = if p.spans_storeys { b.height() } else { storey.elevation + storey.height };
            if rect.v1 > top - LINTEL {
                return Err(ModelError::Invalid(if p.spans_storeys {
                    format!("{slot}: the opening runs to {:.0} mm, less than {LINTEL} mm below the eaves ({:.0} mm)", rect.v1, b.height())
                } else {
                    format!(
                        "{slot}: sill {} + height {} leaves less than {LINTEL} mm below the storey above (storey height {}); \
                         stairwell windows that cross a floor need spans_storeys",
                        ty_eff.sill, ty_eff.height, storey.height
                    )
                }));
            }
        } else if rect.u0 < 0.0 || rect.u1 > frame.length {
            return Err(ModelError::Invalid(format!(
                "{slot}: the {} mm balcony at {c:.0} mm runs past the facade ends (0 to {:.0} mm)",
                ty_eff.width, frame.length
            )));
        }
        if let ComponentKind::Balcony { slab_width, .. } = ty_eff.kind
            && (c - slab_width / 2.0 < 0.0 || c + slab_width / 2.0 > frame.length)
        {
            return Err(ModelError::Invalid(format!("{slot}: the {slab_width} mm balcony slab runs past the facade end")));
        }
        // Only things that stand at the same time can collide: openings with openings,
        // balconies with balconies.
        let together = |o: &Resolved| o.lifetime.0 < lifetime.1 && lifetime.0 < o.lifetime.1 && o.is_opening() == opening;
        let gap = if opening { GAP } else { 0.0 };
        if let Some(other) = out.iter().find(|o| together(o) && o.rect.overlaps(&rect, gap)) {
            let what = if opening {
                format!("the opening at level {} bay {} (keep {GAP} mm between openings)", other.placement.level, other.placement.bay)
            } else {
                format!("the balcony at level {} bay {}", other.placement.level, other.placement.bay)
            };
            return Err(ModelError::Invalid(format!("{slot}: overlaps {what}")));
        }
        out.push(Resolved { placement: p.clone(), ty: ty_eff, rect, lifetime });
    }
    Ok(out)
}

fn build_building(doc: &Document, b: &Building, view: View, cut: Option<f64>) -> Result<Built> {
    let above = |z: f64| cut.is_some_and(|c| z >= c - 1.0);
    let top_at = |z: f64| cut.map_or(z, |c| z.min(c));
    let h = b.height();
    let n = b.footprint.len();
    let levels = b.storeys.len();
    let zb = b.bottom();
    // Each storey's inner wall line: the facade is thicker lower down and steps at every floor.
    let inners: Vec<Vec<[f64; 2]>> = (0..levels).map(|l| b.inner_line(l)).collect();
    for (l, inner) in inners.iter().enumerate() {
        check_inner_ring(&b.footprint, inner)
            .map_err(|e| ModelError::Invalid(format!("{}: {e}", b.storeys.get(l).map_or("storey", |s| s.name.as_str()))))?;
    }
    // A storey's wall runs from the underside of its floor to the underside of the next one.
    let heights: Vec<(f64, f64)> = (0..levels)
        .map(|l| {
            let z0 = if l == 0 { zb } else { b.storeys.get(l).map_or(zb, |s| s.elevation - FLOOR_SLAB) };
            (z0, b.storeys.get(l + 1).map_or(h, |s| s.elevation - FLOOR_SLAB))
        })
        .collect();
    let mut walls = Vec::with_capacity(b.facades.len());
    // The plinth's face: the footprint carried out by its projection, mitred at the corners.
    let plinth_line = match &b.plinth {
        Some(p) => {
            crate::checked("plinth height", p.height, 100.0, 5000.0)?;
            crate::checked("plinth projection", p.projection, 0.0, 300.0)?;
            Some(inner_ring(&b.footprint, -p.projection.max(1.0)))
        }
        None => None,
    };
    for f in &b.facades {
        let (a, e) = b.edge(f.edge).ok_or_else(|| ModelError::Invalid(format!("facade {} has no edge", f.id)))?;
        let fr = FacadeFrame::new(a, e).ok_or_else(|| ModelError::Invalid(format!("facade {} has zero length", f.id)))?;
        let along = |q: [f64; 2]| (q[0] - a[0]) * fr.t[0] + (q[1] - a[1]) * fr.t[1];
        let mut spans = Vec::with_capacity(levels);
        for (l, inner) in inners.iter().enumerate() {
            let (Some(&qs), Some(&qe), Some(&(z0, z1))) = (inner.get(f.edge), inner.get((f.edge + 1) % n), heights.get(l)) else {
                return Err(ModelError::Invalid(format!("facade {} has no inner line", f.id)));
            };
            if above(z0) {
                continue;
            }
            spans.push(Span { level: l, thickness: b.edge_thickness(f.edge, l), z0, z1: top_at(z1), in0: along(qs), in1: along(qe), qs, qe });
        }
        if spans.is_empty() {
            continue;
        }
        let all = resolve_facade(doc, b, f)?;
        // What this view shows, with each element's status.
        let shown: Vec<(&Resolved, Status)> =
            all.iter().filter(|r| !above(r.rect.v0)).filter_map(|r| view.status(r.lifetime).map(|s| (r, s))).collect();
        let resolved: Vec<(&Resolved, Status)> = shown.iter().filter(|(r, _)| r.is_opening()).copied().collect();
        let holes: Vec<Rect> = resolved.iter().map(|(r, _)| r.rect).collect();
        let unbuildable = || ModelError::Invalid(format!("facade {}: the wall faces could not be triangulated", f.id));
        let mesh = crate::walls::skin(&fr, &spans, &holes).ok_or_else(unbuildable)?;
        let segments = spans
            .iter()
            .map(|s| crate::walls::segment(&fr, &format!("{}.L{}", f.id, s.level), s, &holes).ok_or_else(unbuildable))
            .collect::<Result<Vec<_>>>()?;

        let elevation_of = |r: &Resolved| b.storeys.get(r.placement.level).map_or(0.0, |s| s.elevation);
        let openings = resolved
            .iter()
            .map(|(r, status)| {
                let id = format!("{}.L{}.B{}", f.id, r.placement.level, r.placement.bay);
                let mut o = component(&fr, &id, r, elevation_of(r), *status);
                if cut.is_some() {
                    o.fill = clip_element(o.fill, cut);
                    o.attached = o.attached.into_iter().map(|e| clip_element(e, cut)).collect();
                }
                o
            })
            .collect();
        let attachments = shown
            .iter()
            .filter(|(r, _)| !r.is_opening())
            .flat_map(|(r, status)| {
                let id = format!("{}.L{}.B{}.balcony", f.id, r.placement.level, r.placement.bay);
                attachment(&fr, &id, r, elevation_of(r), *status).into_iter().map(|e| clip_element(e, cut)).collect::<Vec<_>>()
            })
            .collect();
        let plinth = match (&b.plinth, &plinth_line) {
            (Some(p), Some(line)) => {
                let m = plinth_piece(&fr, f, (a, e), line, p, &holes).ok_or_else(unbuildable)?;
                match cut {
                    Some(z) => m.below(z),
                    None => m,
                }
            }
            _ => Mesh::default(),
        };
        walls.push(BuiltWall { id: f.id.clone(), frame: fr, kind: f.kind, height: h, bottom: zb, mesh, segments, plinth, openings, attachments });
    }

    // Floors span to the inner face of the storey's own wall and rest on the step of the thicker wall below.
    let mut slabs = Vec::with_capacity(levels + 1);
    let openings =
        |level: usize| -> Result<Vec<Vec<[f64; 2]>>> { b.stairs.iter().filter(|st| st.levels.contains(&level)).map(crate::stair::opening).collect() };
    for (i, s) in b.storeys.iter().enumerate() {
        let inner = inners.get(i).cloned().unwrap_or_default();
        let z0 = s.elevation - FLOOR_SLAB;
        if above(z0) {
            continue;
        }
        let holes = match i.checked_sub(1) {
            Some(l) => openings(l)?,
            None => Vec::new(),
        };
        let (inner, holes) = slab_shape(&inner, holes)?;
        slabs.push(BuiltSlab {
            id: format!("{}.slab.L{i}", b.id),
            kind: SlabKind::Floor,
            level: i,
            mesh: prism_with_holes(&inner, &holes, z0, FLOOR_SLAB),
            polygon: inner,
            z_bottom: z0,
            thickness: FLOOR_SLAB,
            holes,
        });
    }
    let top_level = levels.saturating_sub(1);
    let top_inner = inners.get(top_level).cloned().unwrap_or_default();
    let roof = match &b.roof {
        _ if above(h - FLOOR_SLAB) => None,
        crate::roof::Roof::Flat => {
            let (outline, holes) = slab_shape(&b.footprint, openings(top_level)?)?;
            slabs.push(BuiltSlab {
                id: format!("{}.roof", b.id),
                kind: SlabKind::Roof,
                level: top_level,
                z_bottom: h,
                thickness: ROOF_SLAB,
                mesh: prism_with_holes(&outline, &holes, h, ROOF_SLAB),
                polygon: outline,
                holes,
            });
            None
        }
        crate::roof::Roof::Pitched { pitch_deg, gables, straighten_mm, overhang_mm, verge_mm, thickness_mm, gutters } => {
            let (top_inner, holes) = slab_shape(&top_inner, openings(top_level)?)?;
            // The attic floor closes the top storey; the roof rises from the eaves above it.
            slabs.push(BuiltSlab {
                id: format!("{}.attic", b.id),
                kind: SlabKind::Floor,
                level: top_level,
                mesh: prism_with_holes(&top_inner, &holes, h - FLOOR_SLAB, FLOOR_SLAB),
                polygon: top_inner,
                z_bottom: h - FLOOR_SLAB,
                thickness: FLOOR_SLAB,
                holes,
            });
            let kept = crate::roof::eave_outline(&b.footprint, *straighten_mm);
            let outline: Vec<[f64; 2]> = kept.iter().filter_map(|&i| b.footprint.get(i).copied()).collect();
            // A gable facade is an edge of the eave outline when both its corners were kept next to each other.
            let mut edges = Vec::with_capacity(gables.len());
            for g in gables {
                let f = b.facade(g)?;
                let (i, j) = (f.edge, (f.edge + 1) % n);
                let pos = kept.iter().position(|&k| k == i).filter(|&p| kept.get((p + 1) % kept.len()) == Some(&j));
                edges.push(pos.ok_or_else(|| ModelError::Invalid(format!("gable {g} was straightened away; lower straighten_mm")))?);
            }
            let buildup = crate::roof::Buildup { overhang: *overhang_mm, verge: *verge_mm, thickness: *thickness_mm, gutters: *gutters };
            let r = crate::roof::pitched_with(&outline, h, *pitch_deg, &edges, &buildup)?;
            Some(BuiltRoof {
                id: format!("{}.roof", b.id),
                cover: r.cover,
                gables: r.gables,
                gutters: r.gutters,
                eaves: h,
                ridge: r.ridge,
                pitch_deg: *pitch_deg,
            })
        }
    };
    // Interior walls, storey by storey, joined to each other and to that storey's facade face.
    let mut interior = Vec::new();
    for (level, s) in b.storeys.iter().enumerate() {
        let on: Vec<(usize, &crate::doc::InteriorWall)> = b.interior_walls.iter().enumerate().filter(|(_, w)| w.levels.contains(&level)).collect();
        if on.is_empty() || above(s.elevation) {
            continue;
        }
        let wall_height = top_at(s.elevation + s.height - FLOOR_SLAB) - s.elevation;
        let axes: Vec<Axis> = on.iter().map(|(_, w)| Axis { a: w.a, b: w.b, thickness: w.thickness }).collect();
        let joined = crate::joins::join(&axes, inners.get(level).map_or(&[][..], Vec::as_slice), b.thickest_wall(level));
        for ((k, iw), j) in on.iter().zip(joined) {
            if !(crate::footprint::signed_area2(&j.outline).abs() > 1.0) {
                continue;
            }
            let id = if iw.id.is_empty() { format!("{}.iw{k}", b.id) } else { iw.id.clone() };
            if let Some(mut w) = interior_wall(iw, &format!("{id}.L{level}"), level, &j, (s.elevation, wall_height)) {
                if cut.is_some() {
                    for d in &mut w.doors {
                        d.fill = clip_element(d.fill.clone(), cut);
                    }
                }
                interior.push(w);
            }
        }
    }
    let mut stairs = Vec::new();
    for (k, st) in b.stairs.iter().enumerate() {
        for &level in &st.levels {
            let Some(s) = b.storeys.get(level) else { continue };
            let id = if st.id.is_empty() { format!("{}.stair{k}", b.id) } else { st.id.clone() };
            if above(s.elevation) {
                continue;
            }
            let mesh = crate::stair::mesh(st, s.elevation, s.height)?;
            let mesh = match cut {
                Some(z) => mesh.below(z),
                None => mesh,
            };
            stairs.push(BuiltStair { id: format!("{id}.L{level}"), level, kind: st.kind, winding: st.winders.is_some(), mesh });
        }
    }
    Ok(Built { building_id: b.id.clone(), name: b.name.clone(), storeys: b.storeys.clone(), walls, slabs, roof, interior, stairs })
}

/// The plinth runs this far below the terrain, mm.
const PLINTH_FOOT: f64 = 300.0;

/// The plinth in front of one facade: a closed piece from below the terrain to its top, between
/// the wall's outer face and the plinth's face, with the facade's openings cut through it.
fn plinth_piece(
    fr: &FacadeFrame,
    f: &Facade,
    (a, e): ([f64; 2], [f64; 2]),
    line: &[[f64; 2]],
    p: &crate::doc::Plinth,
    holes: &[Rect],
) -> Option<Mesh> {
    let n = line.len();
    let (s, t) = (*line.get(f.edge)?, *line.get((f.edge + 1) % n)?);
    let face = FacadeFrame::new(s, t)?;
    let along = |q: [f64; 2]| (q[0] - s[0]) * face.t[0] + (q[1] - s[1]) * face.t[1];
    // Openings, from the wall's frame into the plinth's (which starts further out at the corner).
    let shift = along(fr.origin);
    let holes: Vec<Rect> = holes.iter().map(|r| Rect { u0: r.u0 + shift, u1: r.u1 + shift, ..*r }).collect();
    let span = Span { level: 0, thickness: p.projection.max(1.0), z0: -PLINTH_FOOT, z1: p.height, in0: along(a), in1: along(e), qs: a, qe: e };
    crate::walls::segment(&face, &format!("{}.plinth", f.id), &span, &holes).map(|seg| seg.mesh)
}

/// Door frame (karm) width, mm.
const DOOR_FRAME: f64 = 45.0;
/// Door leaf thickness, mm.
const DOOR_LEAF: f64 = 40.0;

/// An interior wall on one storey after joining: a closed prism with its doors cut out, a
/// frame and a closed leaf in each door, and the plan cut with the doors left open. `None` if
/// the wall is degenerate.
fn interior_wall(iw: &crate::doc::InteriorWall, id: &str, level: usize, j: &crate::joins::Joined, (z0, height): (f64, f64)) -> Option<BuiltInterior> {
    let [r0, r1, l1, l0] = j.outline;
    let fr = FacadeFrame::new(r0, r1)?;
    let d = iw.thickness;
    let along = |q: [f64; 2]| (q[0] - r0[0]) * fr.t[0] + (q[1] - r0[1]) * fr.t[1];
    // Door positions are measured from the wall's drawn start; the frame starts at the joined right side.
    let shift = (r0[0] - iw.a[0]) * fr.t[0] + (r0[1] - iw.a[1]) * fr.t[1];
    let span = Span { level, thickness: d, z0, z1: z0 + height, in0: along(l0), in1: along(l1), qs: l0, qe: l1 };
    let mut doors = Vec::with_capacity(iw.doors.len());
    for (k, door) in iw.doors.iter().enumerate() {
        let (u0, u1) = ((door.at - door.width / 2.0 - shift).max(0.0), (door.at + door.width / 2.0 - shift).min(fr.length));
        if u1 - u0 < 300.0 {
            continue;
        }
        // A wall cut lower than the door (a 3D plan cut) leaves the door open through the top.
        let top = if door.height + 50.0 > height { height + 10.0 } else { door.height };
        let rect = Rect { u0, u1, v0: z0, v1: z0 + top };
        let (fw, leaf) = (DOOR_FRAME.min((u1 - u0) / 4.0), DOOR_LEAF.min(d / 2.0));
        let mut frame = fbox(&fr, (u0, u0 + fw), (0.0, d), (rect.v0, rect.v1));
        frame.append(&fbox(&fr, (u1 - fw, u1), (0.0, d), (rect.v0, rect.v1)));
        frame.append(&fbox(&fr, (u0 + fw, u1 - fw), (0.0, d), (rect.v1 - fw, rect.v1)));
        let panel = fbox(&fr, (u0 + fw, u1 - fw), ((d - leaf) / 2.0, (d + leaf) / 2.0), (rect.v0 + 10.0, rect.v1 - fw));
        let door_id = format!("{id}.D{k}");
        let fill = BuiltElement {
            id: door_id.clone(),
            name: "Dør".into(),
            class: ElementClass::Door,
            type_id: "door.interior".into(),
            level,
            width: u1 - u0,
            height: rect.v1 - rect.v0,
            parts: vec![(Material::Frame, frame), (Material::Door, panel)],
            status: Status::Existing,
        };
        doors.push(BuiltOpening { id: door_id, level, rect, fill, attached: Vec::new(), status: Status::Existing });
    }
    let holes: Vec<Rect> = doors.iter().map(|o| o.rect).collect();
    let mesh = crate::walls::segment(&fr, id, &span, &holes).map_or_else(|| prism(&j.outline, z0, height), |s| s.mesh);
    let footprints: Vec<Vec<[f64; 2]>> = holes
        .iter()
        .map(|r| {
            [(r.u0, -5.0), (r.u1, -5.0), (r.u1, d + 5.0), (r.u0, d + 5.0)]
                .iter()
                .map(|&(u, w)| {
                    let p = fr.point(u, w, 0.0);
                    [p[0], p[1]]
                })
                .collect()
        })
        .collect();
    let cut = if footprints.is_empty() {
        vec![j.outline.to_vec()]
    } else {
        crate::walls::cut(&j.outline, &footprints).into_iter().filter_map(|shape| shape.into_iter().next()).collect()
    };
    Some(BuiltInterior {
        id: id.to_string(),
        level,
        axis: [j.a, j.b],
        thickness: d,
        outline: j.outline,
        ends: j.ends,
        frame: fr,
        doors,
        cut,
        z_bottom: z0,
        height,
        load_bearing: iw.load_bearing,
        mesh,
    })
}

/// A facade's inner wall line must keep every edge pointing the way its outer edge does: a wall
/// too thick for a short facade turns that edge inside out.
fn check_inner_ring(outer: &[[f64; 2]], inner: &[[f64; 2]]) -> Result<()> {
    let n = outer.len();
    for i in 0..n {
        let (Some(a), Some(b), Some(p), Some(q)) = (outer.get(i), outer.get((i + 1) % n), inner.get(i), inner.get((i + 1) % n)) else {
            return Err(ModelError::Invalid("the inner wall line doesn't match the footprint".into()));
        };
        let (d, e) = ([b[0] - a[0], b[1] - a[1]], [q[0] - p[0], q[1] - p[1]]);
        // A collapsed edge (a point) is fine; a reversed one is not.
        if (e[0] * e[0] + e[1] * e[1]).sqrt() > 1.0 && d[0] * e[0] + d[1] * e[1] <= 0.0 {
            return Err(ModelError::Invalid(format!(
                "the facade wall is too thick for footprint edge {i} ({:.0} mm long); use a thinner wall there",
                (d[0] * d[0] + d[1] * d[1]).sqrt()
            )));
        }
    }
    Ok(())
}

/// A slab's outline and holes with its stair openings cut out. Openings may not overlap each
/// other. An opening that runs into the walls (a stairwell against the facade) is clipped to the
/// slab, so the slab's outline takes a notch there instead of a hole.
fn slab_shape(outer: &[[f64; 2]], holes: Vec<Vec<[f64; 2]>>) -> Result<(Vec<[f64; 2]>, Vec<Vec<[f64; 2]>>)> {
    let inside = |ring: &[[f64; 2]], p: [f64; 2]| {
        let mut yes = false;
        for i in 0..ring.len() {
            let (a, b) = (ring[i], ring[(i + 1) % ring.len()]);
            if (a[1] > p[1]) != (b[1] > p[1]) && p[0] < (b[0] - a[0]) * (p[1] - a[1]) / (b[1] - a[1]) + a[0] {
                yes = !yes;
            }
        }
        yes
    };
    let cross = |a: [f64; 2], b: [f64; 2], c: [f64; 2]| (b[0] - a[0]) * (c[1] - a[1]) - (b[1] - a[1]) * (c[0] - a[0]);
    let intersects = |r: &[[f64; 2]], s: &[[f64; 2]]| {
        (0..r.len()).any(|i| {
            (0..s.len()).any(|j| {
                let (a, b, c, d) = (r[i], r[(i + 1) % r.len()], s[j], s[(j + 1) % s.len()]);
                let bbox = (0..2).all(|k| a[k].min(b[k]) <= c[k].max(d[k]) + 1e-6 && c[k].min(d[k]) <= a[k].max(b[k]) + 1e-6);
                bbox && cross(a, b, c) * cross(a, b, d) <= 0.0 && cross(c, d, a) * cross(c, d, b) <= 0.0
            })
        })
    };
    for (i, hole) in holes.iter().enumerate() {
        if !hole.iter().any(|p| inside(outer, *p)) && !intersects(outer, hole) {
            return Err(ModelError::Invalid(format!("stair opening {i} lies outside its floor slab")));
        }
        for other in &holes[..i] {
            if intersects(other, hole) || hole.first().is_some_and(|p| inside(other, *p)) || other.first().is_some_and(|p| inside(hole, *p)) {
                return Err(ModelError::Invalid(format!("stair opening {i} overlaps another stair opening")));
            }
        }
    }
    if holes.iter().all(|h| h.iter().all(|p| inside(outer, *p)) && !intersects(outer, h)) {
        return Ok((outer.to_vec(), holes));
    }
    use i_overlay::core::{fill_rule::FillRule, overlay_rule::OverlayRule};
    use i_overlay::float::overlay::FloatOverlay;
    let subj = vec![outer.to_vec()];
    let shapes = FloatOverlay::with_subj_and_clip(&subj, &holes).overlay(OverlayRule::Difference, FillRule::NonZero);
    let area = |shape: &Vec<Vec<[f64; 2]>>| shape.first().map_or(0.0, |o| crate::footprint::signed_area2(o).abs() / 2.0);
    if shapes.iter().filter(|sh| area(sh) > 1.0e6).count() > 1 {
        return Err(ModelError::Invalid("stair openings cut the floor slab in two".into()));
    }
    let mut biggest = shapes.into_iter().max_by(|a, b| area(a).total_cmp(&area(b))).unwrap_or_default();
    if biggest.is_empty() {
        return Err(ModelError::Invalid("stair openings leave nothing of the floor slab".into()));
    }
    let outline = biggest.remove(0);
    Ok((outline, biggest))
}

/// A vertical prism on a counter-clockwise polygon.
pub fn prism(poly: &[[f64; 2]], z0: f64, height: f64) -> Mesh {
    prism_with_holes(poly, &[], z0, height)
}

fn prism_with_holes(poly: &[[f64; 2]], holes: &[Vec<[f64; 2]>], z0: f64, height: f64) -> Mesh {
    let mut m = Mesh::default();
    let z1 = z0 + height;
    m.polygon(poly, holes, |p| [p[0], p[1], z1]);
    let mut bottom = Mesh::default();
    bottom.polygon(poly, holes, |p| [p[0], p[1], z0]);
    for t in &mut bottom.triangles {
        t.swap(1, 2);
    }
    m.append(&bottom);
    let n = poly.len();
    for i in 0..n {
        let (a, b) = (poly[i], poly[(i + 1) % n]);
        m.quad([a[0], a[1], z0], [b[0], b[1], z0], [b[0], b[1], z1], [a[0], a[1], z1]);
    }
    for hole in holes {
        for i in 0..hole.len() {
            let (a, b) = (hole[i], hole[(i + 1) % hole.len()]);
            m.quad([b[0], b[1], z0], [a[0], a[1], z0], [a[0], a[1], z1], [b[0], b[1], z1]);
        }
    }
    m
}

/// A box in facade coordinates.
fn fbox(fr: &FacadeFrame, u: (f64, f64), w: (f64, f64), v: (f64, f64)) -> Mesh {
    let lo = [fr.point(u.0, w.0, v.0), fr.point(u.1, w.0, v.0), fr.point(u.1, w.1, v.0), fr.point(u.0, w.1, v.0)];
    let hi = [fr.point(u.0, w.0, v.1), fr.point(u.1, w.0, v.1), fr.point(u.1, w.1, v.1), fr.point(u.0, w.1, v.1)];
    let mut m = Mesh::default();
    m.hexahedron(lo, hi);
    m
}

/// Vertical bars at even spacing between `u0` and `u1` (`count` bars, not at the ends).
fn dividers(u0: f64, u1: f64, count: u32) -> Vec<f64> {
    (1..count).map(|k| u0 + (u1 - u0) * f64::from(k) / f64::from(count)).collect()
}

/// Frame, sashes or leaves and glazing for a window or door in `rect`.
fn fill_parts(fr: &FacadeFrame, rect: Rect, kind: &ComponentKind) -> Vec<(Material, Mesh)> {
    let Rect { u0, u1, v0, v1 } = rect;
    let w = (REVEAL, REVEAL + FRAME_DEPTH);
    let mut frame = Mesh::default();
    frame.append(&fbox(fr, (u0, u0 + FRAME), w, (v0, v1)));
    frame.append(&fbox(fr, (u1 - FRAME, u1), w, (v0, v1)));
    frame.append(&fbox(fr, (u0 + FRAME, u1 - FRAME), w, (v1 - FRAME, v1)));
    let (iu0, iu1) = (u0 + FRAME, u1 - FRAME);
    let mut glass = Mesh::default();
    let glass_w = (REVEAL + 30.0, REVEAL + 36.0);
    let mut parts = Vec::new();
    match kind {
        ComponentKind::Window { lights } => {
            frame.append(&fbox(fr, (iu0, iu1), w, (v0, v0 + FRAME)));
            for c in dividers(iu0, iu1, *lights) {
                frame.append(&fbox(fr, (c - FRAME / 2.0, c + FRAME / 2.0), w, (v0 + FRAME, v1 - FRAME)));
            }
            glass.append(&fbox(fr, (iu0, iu1), glass_w, (v0 + FRAME, v1 - FRAME)));
            parts.push((Material::Sill, fbox(fr, (u0 - 30.0, u1 + 30.0), (-40.0, REVEAL), (v0 - 40.0, v0))));
        }
        ComponentKind::FixedWindow => {
            frame.append(&fbox(fr, (iu0, iu1), w, (v0, v0 + FRAME)));
            glass.append(&fbox(fr, (iu0, iu1), glass_w, (v0 + FRAME, v1 - FRAME)));
            parts.push((Material::Sill, fbox(fr, (u0 - 30.0, u1 + 30.0), (-40.0, REVEAL), (v0 - 40.0, v0))));
        }
        ComponentKind::EntranceDoor => {
            parts.push((Material::Door, fbox(fr, (iu0, iu1), (REVEAL + 15.0, REVEAL + 60.0), (v0, v1 - FRAME))));
        }
        ComponentKind::FrenchDoor { leaves } | ComponentKind::FrenchBalcony { leaves, .. } | ComponentKind::Balcony { leaves, .. } => {
            let panel = 400.0_f64.min((v1 - v0) / 3.0);
            frame.append(&fbox(fr, (iu0, iu1), w, (v0, v0 + panel)));
            for c in dividers(iu0, iu1, *leaves) {
                frame.append(&fbox(fr, (c - FRAME / 2.0, c + FRAME / 2.0), w, (v0 + panel, v1 - FRAME)));
            }
            glass.append(&fbox(fr, (iu0, iu1), glass_w, (v0 + panel, v1 - FRAME)));
        }
        ComponentKind::DoorWindow { door_width, door_side, window_sill, lights } => {
            // Split the opening: door leaf on one side, window over a brick panel on the other.
            let (du0, du1, wu0, wu1) = match door_side {
                Side::Left => (u0, u0 + door_width, u0 + door_width, u1),
                Side::Right => (u1 - door_width, u1, u0, u1 - door_width),
            };
            let mullion = if door_side == &Side::Left { wu0 } else { wu1 };
            frame.append(&fbox(fr, (mullion - FRAME / 2.0, mullion + FRAME / 2.0), w, (v0, v1 - FRAME)));
            parts.push((Material::Door, fbox(fr, (du0 + FRAME / 2.0, du1 - FRAME / 2.0), (REVEAL + 15.0, REVEAL + 60.0), (v0, v1 - FRAME))));
            let ws = v0 + window_sill;
            parts.push((Material::Wall, fbox(fr, (wu0 + FRAME / 2.0, wu1 - FRAME / 2.0), (0.0, REVEAL + FRAME_DEPTH), (v0, ws))));
            frame.append(&fbox(fr, (wu0 + FRAME / 2.0, wu1 - FRAME / 2.0), w, (ws, ws + FRAME)));
            for c in dividers(wu0 + FRAME / 2.0, wu1 - FRAME / 2.0, *lights) {
                frame.append(&fbox(fr, (c - FRAME / 2.0, c + FRAME / 2.0), w, (ws + FRAME, v1 - FRAME)));
            }
            glass.append(&fbox(fr, (wu0 + FRAME / 2.0, wu1 - FRAME / 2.0), glass_w, (ws + FRAME, v1 - FRAME)));
            parts.push((Material::Sill, fbox(fr, (wu0, wu1), (-40.0, REVEAL), (ws - 40.0, ws))));
        }
        ComponentKind::BalconySlab { .. } | ComponentKind::SteelBalcony { .. } => return Vec::new(),
    }
    parts.push((Material::Frame, frame));
    if !glass.is_empty() {
        parts.push((Material::Glass, glass));
    }
    parts
}

/// A guard rail along a straight run in facade coordinates: posts at the ends, top and
/// bottom rails, balusters at 110 mm centres.
fn rail_run(fr: &FacadeFrame, from: (f64, f64), to: (f64, f64), z0: f64, height: f64) -> Mesh {
    let mut m = Mesh::default();
    let (du, dw) = (to.0 - from.0, to.1 - from.1);
    let len = (du * du + dw * dw).sqrt();
    if !(len > 1.0) {
        return m;
    }
    let at = |s: f64| (from.0 + du * s / len, from.1 + dw * s / len);
    let bar = |m: &mut Mesh, s: f64, size: f64, v: (f64, f64)| {
        let (u, w) = at(s);
        m.append(&fbox(fr, (u - size / 2.0, u + size / 2.0), (w - size / 2.0, w + size / 2.0), v));
    };
    bar(&mut m, 20.0, 40.0, (z0, z0 + height));
    bar(&mut m, len - 20.0, 40.0, (z0, z0 + height));
    let count = (len / 110.0).floor() as u32;
    for k in 1..count {
        bar(&mut m, len * f64::from(k) / f64::from(count), 16.0, (z0 + 100.0, z0 + height - 40.0));
    }
    // Rails as thin boxes along the run (axis-aligned in u or w; runs here always are).
    let (ua, ub) = (from.0.min(to.0) - 20.0, from.0.max(to.0) + 20.0);
    let (wa, wb) = (from.1.min(to.1) - 20.0, from.1.max(to.1) + 20.0);
    m.append(&fbox(fr, (ua, ub), (wa, wb), (z0 + height - 40.0, z0 + height)));
    m.append(&fbox(fr, (ua, ub), (wa, wb), (z0 + 100.0, z0 + 130.0)));
    m
}

fn component(fr: &FacadeFrame, id: &str, r: &Resolved, elevation: f64, status: Status) -> BuiltOpening {
    let ty = &r.ty;
    let class = if ty.kind.is_door() { ElementClass::Door } else { ElementClass::Window };
    let level = r.placement.level;
    let fill = BuiltElement {
        id: id.to_string(),
        name: ty.name.clone(),
        class,
        type_id: ty.id.clone(),
        level,
        width: ty.width,
        height: ty.height,
        parts: fill_parts(fr, r.rect, &ty.kind),
        status,
    };
    let mut attached = Vec::new();
    let Rect { u0, u1, v0, .. } = r.rect;
    match ty.kind {
        ComponentKind::FrenchBalcony { rail_height, .. } => {
            let rail = rail_run(fr, (u0 - 50.0, -130.0), (u1 + 50.0, -130.0), v0, rail_height);
            attached.push(element(id, "rail", "French balcony rail", ElementClass::Railing, ty, level, rail, status));
        }
        ComponentKind::Balcony { depth, slab_width, rail_height, .. } => {
            let c = (u0 + u1) / 2.0;
            let (s0, s1) = (c - slab_width / 2.0, c + slab_width / 2.0);
            let slab = fbox(fr, (s0, s1), (-depth, 0.0), (elevation - BALCONY_SLAB, elevation));
            attached.push(element(id, "slab", "Balcony slab", ElementClass::BalconySlab, ty, level, slab, status));
            let mut rail = rail_run(fr, (s0 + 40.0, -depth + 40.0), (s1 - 40.0, -depth + 40.0), elevation, rail_height);
            rail.append(&rail_run(fr, (s0 + 40.0, -depth + 40.0), (s0 + 40.0, -60.0), elevation, rail_height));
            rail.append(&rail_run(fr, (s1 - 40.0, -depth + 40.0), (s1 - 40.0, -60.0), elevation, rail_height));
            attached.push(element(id, "rail", "Balcony rail", ElementClass::Railing, ty, level, rail, status));
        }
        _ => {}
    }
    BuiltOpening { id: format!("{id}.opening"), level, rect: r.rect, fill, attached, status }
}

/// A straight rod between two facade points `(u, w, v)` with a square section `r` mm wide.
fn rod(fr: &FacadeFrame, a: (f64, f64, f64), b: (f64, f64, f64), r: f64) -> Mesh {
    let mut m = Mesh::default();
    let (pa, pb) = (fr.point(a.0, a.1, a.2), fr.point(b.0, b.1, b.2));
    let d = [pb[0] - pa[0], pb[1] - pa[1], pb[2] - pa[2]];
    let len = (d[0] * d[0] + d[1] * d[1] + d[2] * d[2]).sqrt();
    if !(len > 1.0) {
        return m;
    }
    let t = [d[0] / len, d[1] / len, d[2] / len];
    // Two directions across the rod.
    let helper = if t[2].abs() < 0.9 { [0.0, 0.0, 1.0] } else { [1.0, 0.0, 0.0] };
    let cross = |x: [f64; 3], y: [f64; 3]| [x[1] * y[2] - x[2] * y[1], x[2] * y[0] - x[0] * y[2], x[0] * y[1] - x[1] * y[0]];
    let norm = |x: [f64; 3]| {
        let l = (x[0] * x[0] + x[1] * x[1] + x[2] * x[2]).sqrt().max(1e-12);
        [x[0] / l, x[1] / l, x[2] / l]
    };
    let s1 = norm(cross(t, helper));
    let s2 = cross(t, s1);
    let h = r / 2.0;
    let corner =
        |p: [f64; 3], i: f64, j: f64| [p[0] + (s1[0] * i + s2[0] * j) * h, p[1] + (s1[1] * i + s2[1] * j) * h, p[2] + (s1[2] * i + s2[2] * j) * h];
    let ring = |p: [f64; 3]| [corner(p, -1.0, -1.0), corner(p, 1.0, -1.0), corner(p, 1.0, 1.0), corner(p, -1.0, 1.0)];
    m.hexahedron(ring(pa), ring(pb));
    m
}

/// A railing run in facade coordinates with MinAltan profiles: corner posts, balusters at
/// about 110 mm centres and the style's flat rails.
fn styled_rail(fr: &FacadeFrame, from: (f64, f64), to: (f64, f64), z0: f64, height: f64, style: Railing) -> Mesh {
    let mut m = Mesh::default();
    let (du, dw) = (to.0 - from.0, to.1 - from.1);
    let len = (du * du + dw * dw).sqrt();
    if !(len > 1.0) {
        return m;
    }
    let along_u = du.abs() >= dw.abs();
    // (baluster size along the run, across it), rails as (height of their top above z0, rail height, rail depth).
    let (bal, rails): ((f64, f64), Vec<(f64, f64, f64)>) = match style {
        Railing::Frederiksberg => ((12.0, 12.0), vec![(height, 50.0, 10.0), (150.0, 50.0, 10.0)]),
        Railing::Norrebro => ((30.0, 6.0), vec![(height, 60.0, 15.0)]),
        Railing::Vesterbro => ((30.0, 6.0), vec![(height, 50.0, 10.0), (150.0, 50.0, 10.0)]),
        Railing::Osterbro => ((30.0, 6.0), vec![]),
    };
    let at = |s: f64| (from.0 + du * s / len, from.1 + dw * s / len);
    let bar = |m: &mut Mesh, s: f64, size: (f64, f64), v: (f64, f64)| {
        let (u, w) = at(s);
        let (su, sw) = if along_u { size } else { (size.1, size.0) };
        m.append(&fbox(fr, (u - su / 2.0, u + su / 2.0), (w - sw / 2.0, w + sw / 2.0), v));
    };
    bar(&mut m, 20.0, (40.0, 40.0), (z0, z0 + height));
    bar(&mut m, len - 20.0, (40.0, 40.0), (z0, z0 + height));
    let count = (len / 110.0).ceil().max(1.0) as u32;
    for k in 1..count {
        bar(&mut m, len * f64::from(k) / f64::from(count), bal, (z0 + 60.0, z0 + height));
    }
    for (top, rh, rd) in rails {
        let (ua, ub) = (from.0.min(to.0) - 20.0, from.0.max(to.0) + 20.0);
        let (wa, wb) = (from.1.min(to.1) - 20.0, from.1.max(to.1) + 20.0);
        let (ua, ub, wa, wb) = if along_u { (ua, ub, from.1 - rd / 2.0, from.1 + rd / 2.0) } else { (from.0 - rd / 2.0, from.0 + rd / 2.0, wa, wb) };
        m.append(&fbox(fr, (ua, ub), (wa, wb), (z0 + top - rh, z0 + top)));
    }
    m
}

/// A balcony hung on the wall without an opening: slab and railing elements.
fn attachment(fr: &FacadeFrame, id: &str, r: &Resolved, elevation: f64, status: Status) -> Vec<BuiltElement> {
    let ty = &r.ty;
    let level = r.placement.level;
    let Rect { u0, u1, .. } = r.rect;
    let deck_top = elevation + ty.sill;
    let mk = |suffix: &str, name: &str, class: ElementClass, parts: Vec<(Material, Mesh)>| BuiltElement {
        id: format!("{id}.{suffix}"),
        name: name.to_string(),
        class,
        type_id: ty.id.clone(),
        level,
        width: ty.width,
        height: ty.height,
        parts,
        status,
    };
    match ty.kind {
        ComponentKind::BalconySlab { depth, thickness } => {
            let slab = fbox(fr, (u0, u1), (-depth, 0.0), (deck_top - thickness, deck_top));
            let mut rail = rail_run(fr, (u0 + 40.0, -depth + 40.0), (u1 - 40.0, -depth + 40.0), deck_top, ty.height);
            rail.append(&rail_run(fr, (u0 + 40.0, -depth + 40.0), (u0 + 40.0, -60.0), deck_top, ty.height));
            rail.append(&rail_run(fr, (u1 - 40.0, -depth + 40.0), (u1 - 40.0, -60.0), deck_top, ty.height));
            vec![
                mk("slab", &ty.name, ElementClass::BalconySlab, vec![(Material::Slab, slab)]),
                mk("rail", &format!("{} rail", ty.name), ElementClass::Railing, vec![(Material::Railing, rail)]),
            ]
        }
        ComponentKind::SteelBalcony { depth, railing, finish, deck, handrail, screens, tie_rods } => {
            let metal = match finish {
                Finish::Galvanised => Material::Steel,
                Finish::White => Material::Frame,
                Finish::Anthracite => Material::Railing,
            };
            let deck_material = if deck == Deck::FibreConcrete { Material::Slab } else { Material::Deck };
            let (f, web, gap) = (STEEL_FRAME, 60.0, 30.0);
            // UPE frame: front, sides and back beams; the deck sits inside it, 10 mm below the edge.
            let mut frame = fbox(fr, (u0, u1), (-depth, -depth + web), (deck_top - f, deck_top + 10.0));
            frame.append(&fbox(fr, (u0, u0 + web), (-depth + web, -gap), (deck_top - f, deck_top + 10.0)));
            frame.append(&fbox(fr, (u1 - web, u1), (-depth + web, -gap), (deck_top - f, deck_top + 10.0)));
            frame.append(&fbox(fr, (u0 + web, u1 - web), (-gap - web, -gap), (deck_top - f, deck_top)));
            let floor = fbox(fr, (u0 + web, u1 - web), (-depth + web, -gap - web), (deck_top - 35.0, deck_top));
            let underside = fbox(fr, (u0 + web, u1 - web), (-depth + web, -gap - web), (deck_top - f, deck_top - f + 3.0));
            let mut slab_parts = vec![(metal, frame), (deck_material, floor), (Material::Sill, underside)];
            if tie_rods {
                let mut rods = rod(fr, (u0 + 30.0, -depth + 30.0, deck_top), (u0 + 30.0, 0.0, deck_top + 2400.0), 24.0);
                rods.append(&rod(fr, (u1 - 30.0, -depth + 30.0, deck_top), (u1 - 30.0, 0.0, deck_top + 2400.0), 24.0));
                slab_parts.push((metal, rods));
            }
            let z = deck_top + 10.0;
            let mut rail = styled_rail(fr, (u0 + 25.0, -depth + 25.0), (u1 - 25.0, -depth + 25.0), z, ty.height, railing);
            rail.append(&styled_rail(fr, (u0 + 25.0, -depth + 25.0), (u0 + 25.0, -gap - 20.0), z, ty.height, railing));
            rail.append(&styled_rail(fr, (u1 - 25.0, -depth + 25.0), (u1 - 25.0, -gap - 20.0), z, ty.height, railing));
            let mut rail_parts = vec![(metal, rail)];
            if handrail {
                let mut wood = fbox(fr, (u0, u1), (-depth, -depth + 70.0), (z + ty.height, z + ty.height + 40.0));
                wood.append(&fbox(fr, (u0, u0 + 70.0), (-depth + 70.0, -gap), (z + ty.height, z + ty.height + 40.0)));
                wood.append(&fbox(fr, (u1 - 70.0, u1), (-depth + 70.0, -gap), (z + ty.height, z + ty.height + 40.0)));
                rail_parts.push((Material::Deck, wood));
            }
            let mut glass = Mesh::default();
            if matches!(screens, Screens::Left | Screens::Both) {
                glass.append(&fbox(fr, (u0 + 45.0, u0 + 55.0), (-depth + web, -gap), (z, z + 1800.0)));
            }
            if matches!(screens, Screens::Right | Screens::Both) {
                glass.append(&fbox(fr, (u1 - 55.0, u1 - 45.0), (-depth + web, -gap), (z, z + 1800.0)));
            }
            if !glass.is_empty() {
                rail_parts.push((Material::Glass, glass));
            }
            vec![
                mk("slab", &ty.name, ElementClass::BalconySlab, slab_parts),
                mk("rail", &format!("{} railing", ty.name), ElementClass::Railing, rail_parts),
            ]
        }
        _ => Vec::new(),
    }
}

fn element(id: &str, suffix: &str, name: &str, class: ElementClass, ty: &ComponentType, level: usize, mesh: Mesh, status: Status) -> BuiltElement {
    let material = if class == ElementClass::Railing { Material::Railing } else { Material::Slab };
    BuiltElement {
        id: format!("{id}.{suffix}"),
        name: name.to_string(),
        class,
        type_id: ty.id.clone(),
        level,
        width: ty.width,
        height: ty.height,
        parts: vec![(material, mesh)],
        status,
    }
}

/// Evenly spaced bay centres along a facade: `count` bays in the usable length.
pub fn even_bays(length: f64, count: usize, margin: f64) -> Vec<f64> {
    let usable = (length - 2.0 * margin).max(0.0);
    let step = usable / count.max(1) as f64;
    (0..count).map(|i| margin + step * (i as f64 + 0.5)).collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::doc::Building;

    #[test]
    fn a_chamfer_shorter_than_the_wall_collapses_to_a_point() {
        // A 10 × 10 m square with a 283 mm chamfer at one corner (two convex turns, as at the
        // north end of a wing); the wall is 700 mm.
        let ring = [[0.0, 0.0], [10000.0, 0.0], [10000.0, 9800.0], [9800.0, 10000.0], [0.0, 10000.0]];
        let inner = super::inner_ring(&ring, 700.0);
        assert_eq!(inner.len(), ring.len());
        // The chamfer's two inner corners are one point, where the neighbours' inner lines meet.
        let (p, q) = (inner[2], inner[3]);
        assert!((p[0] - 9300.0).abs() < 1e-6 && (p[1] - 9300.0).abs() < 1e-6, "{p:?}");
        assert!((p[0] - q[0]).abs() < 1e-6 && (p[1] - q[1]).abs() < 1e-6, "{p:?} {q:?}");
        // No inner edge runs against its outer edge.
        for i in 0..ring.len() {
            let (a, b, c, d) = (ring[i], ring[(i + 1) % 5], inner[i], inner[(i + 1) % 5]);
            assert!((b[0] - a[0]) * (d[0] - c[0]) + (b[1] - a[1]) * (d[1] - c[1]) >= -1e-6, "edge {i}");
        }
        // A thin wall keeps the chamfer.
        let thin = super::inner_ring(&ring, 100.0);
        assert!((thin[2][0] - thin[3][0]).abs() > 100.0);
    }

    fn doc_with_block() -> Document {
        let mut doc = Document::default();
        let fp = [[0.0, 0.0], [20000.0, 0.0], [20000.0, 12000.0], [0.0, 12000.0]];
        let mut b = Building::new("b1", "Test", &fp, 5, 3000.0).unwrap();
        let f = &mut b.facades[0];
        f.bays = even_bays(20000.0, 6, 600.0);
        for level in 0..5 {
            for bay in 0..6 {
                let ty = match (level, bay) {
                    (0, 0) => "door.entrance",
                    (0, _) => "window.casement.2",
                    (_, 2) => "balcony.cantilever",
                    (_, 4) => "balcony.french",
                    _ => "window.casement.2",
                };
                f.placements.push(Placement {
                    level,
                    bay,
                    type_id: ty.into(),
                    width: None,
                    height: None,
                    sill: None,
                    spans_storeys: false,
                    created: None,
                    demolished: None,
                });
            }
        }
        doc.buildings.push(b);
        doc
    }

    #[test]
    fn block_builds_with_openings_and_balconies() {
        let doc = doc_with_block();
        let built = build(&doc).unwrap();
        let b = &built[0];
        assert_eq!(b.walls.len(), 4);
        assert_eq!(b.walls[0].openings.len(), 30);
        assert_eq!(b.slabs.len(), 6);
        let balconies = b.walls[0].openings.iter().filter(|o| o.attached.iter().any(|a| a.class == ElementClass::BalconySlab)).count();
        assert_eq!(balconies, 4);
        // Wall geometry stays inside the building's extent plus nothing outward.
        let (lo, hi) = b.walls[0].mesh.bounds().unwrap();
        assert!(lo[1] >= -1e-6 && hi[1] <= 400.0 + 1e-6, "{lo:?} {hi:?}");
    }

    #[test]
    fn stairs_open_the_arrival_slab_but_leave_the_lowest_floor_solid() {
        let mut doc = doc_with_block();
        let st: crate::Stair = serde_json::from_value(serde_json::json!({
            "id":"stair", "levels":[0,1], "outline":[[4000,4000],[6000,4000],[6000,5900],[4000,5900]],
            "kind":"u_turn", "rounded":false, "landing":950
        }))
        .unwrap();
        doc.buildings[0].stairs.push(st);
        let built = build(&doc).unwrap();
        let area = |s: &BuiltSlab| {
            s.mesh
                .triangles
                .iter()
                .filter_map(|t| {
                    let [a, b, c] = t.map(|i| s.mesh.positions[i as usize]);
                    ((a[2] - (s.z_bottom + s.thickness)).abs() < 1e-6 && (a[2] - b[2]).abs() < 1e-6 && (a[2] - c[2]).abs() < 1e-6)
                        .then_some(((b[0] - a[0]) * (c[1] - a[1]) - (b[1] - a[1]) * (c[0] - a[0])).abs() / 2.0)
                })
                .sum::<f64>()
        };
        let slabs = &built[0].slabs;
        let solid = area(&slabs[0]);
        for level in [1, 2] {
            assert!((solid - area(&slabs[level]) - 2950.0 * 1900.0).abs() < 1e-3, "arrival slab {level} must have the flight and landing opening");
        }
        assert!((area(&slabs[3]) - solid).abs() < 1e-3);
    }

    #[test]
    fn overlapping_and_oversized_openings_are_refused() {
        let mut doc = doc_with_block();
        doc.buildings[0].facades[0].placements[1].width = Some(6000.0);
        let err = build(&doc).unwrap_err();
        assert!(format!("{err}").contains("overlaps"), "{err}");
        let mut doc = doc_with_block();
        doc.buildings[0].facades[0].placements[1].height = Some(2900.0);
        assert!(format!("{}", build(&doc).unwrap_err()).contains("storey above"));
    }

    #[test]
    fn raised_ground_floor_stairwells_and_openings_below_the_floor() {
        let mut doc = doc_with_block();
        let b = &mut doc.buildings[0];
        b.set_storeys_from(5, 3000.0, 900.0).unwrap();
        assert_eq!(b.height(), 15900.0);
        assert_eq!(b.storeys[1].elevation, 3900.0);
        let f = &mut b.facades[0];
        f.placements.clear();
        // A landing window: 2580 mm above the ground storey's floor, 950 high, crosses the floor above.
        let landing = Placement {
            level: 0,
            bay: 5,
            type_id: "window.fixed".into(),
            width: Some(1000.0),
            height: Some(950.0),
            sill: Some(2580.0),
            spans_storeys: false,
            created: None,
            demolished: None,
        };
        f.placements.push(landing.clone());
        let err = build(&doc).unwrap_err().to_string();
        assert!(err.contains("spans_storeys"), "{err}");
        doc.buildings[0].facades[0].placements[0].spans_storeys = true;
        let built = build(&doc).unwrap();
        assert_eq!(built[0].walls[0].openings[0].rect.v0, 900.0 + 2580.0);
        // A door down to the courtyard: below the floor is fine, below the terrain is not.
        let door = Placement {
            level: 0,
            bay: 0,
            type_id: "door.french".into(),
            width: None,
            height: None,
            sill: Some(-500.0),
            spans_storeys: false,
            created: None,
            demolished: None,
        };
        doc.buildings[0].facades[0].placements.push(door);
        assert_eq!(build(&doc).unwrap()[0].walls[0].openings[1].rect.v0, 400.0);
        doc.buildings[0].facades[0].placements[1].sill = Some(-1000.0);
        assert!(build(&doc).unwrap_err().to_string().contains("below the terrain"));
    }

    fn put(doc: &mut Document, level: usize, bay: usize, ty: &str, created: Option<&str>, demolished: Option<&str>) {
        doc.buildings[0].facades[0].placements.push(Placement {
            level,
            bay,
            type_id: ty.into(),
            width: None,
            height: None,
            sill: None,
            spans_storeys: false,
            created: created.map(str::to_string),
            demolished: demolished.map(str::to_string),
        });
    }

    #[test]
    fn phases_replace_an_old_balcony_with_a_larger_centred_one() {
        let mut doc = doc_with_block();
        doc.phases = vec![
            crate::doc::Phase { id: "1938".into(), name: "Opført".into() },
            crate::doc::Phase { id: "2026".into(), name: "Eksisterende".into() },
            crate::doc::Phase { id: "forslag".into(), name: "Forslag".into() },
        ];
        let f = &mut doc.buildings[0].facades[0];
        f.placements.clear();
        // Old balcony under bay 2's door; the new one is centred between bays 2 and 3 and overlaps it.
        f.bays.push((f.bays[2] + f.bays[3]) / 2.0);
        let new_bay = f.bays.len() - 1;
        doc.types.push(ComponentType {
            id: "balcony.old".into(),
            name: "Old balcony".into(),
            width: 2000.0,
            height: 1000.0,
            sill: 0.0,
            kind: ComponentKind::BalconySlab { depth: 1000.0, thickness: 150.0 },
        });
        put(&mut doc, 1, 2, "door.french", None, None);
        put(&mut doc, 1, 2, "balcony.old", None, Some("forslag"));
        put(&mut doc, 1, new_bay, "balcony.minaltan", Some("forslag"), None);
        let count = |doc: &Document, phase: usize, filter: PhaseFilter| {
            let b = build_view(doc, View { phase, filter }).unwrap();
            b[0].walls[0].attachments.iter().map(|e| (e.type_id.clone(), e.status)).collect::<Vec<_>>()
        };
        // 1938 and 2026: only the old balcony (slab + rail).
        assert_eq!(count(&doc, 0, PhaseFilter::Complete), vec![("balcony.old".into(), Status::Existing); 2]);
        assert_eq!(count(&doc, 1, PhaseFilter::Complete).len(), 2);
        // Forslag, complete: only the new one, marked new.
        assert_eq!(count(&doc, 2, PhaseFilter::Complete), vec![("balcony.minaltan".into(), Status::New); 2]);
        // Forslag, demolition: the old one, marked demolished; the door stays existing.
        assert_eq!(count(&doc, 2, PhaseFilter::Demolition), vec![("balcony.old".into(), Status::Demolished); 2]);
        let b = build_view(&doc, View { phase: 2, filter: PhaseFilter::Demolition }).unwrap();
        assert_eq!(b[0].walls[0].openings[0].status, Status::Existing);
        // The overlap is fine because they never stand at the same time; keeping the old one is not.
        doc.buildings[0].facades[0].placements[1].demolished = None;
        let err = build(&doc).unwrap_err().to_string();
        assert!(err.contains("overlaps the balcony"), "{err}");
        // A phase that doesn't exist, or demolition before creation, is refused.
        doc.buildings[0].facades[0].placements[1].demolished = Some("1938".into());
        assert!(build(&doc).is_err());
        doc.buildings[0].facades[0].placements[1].demolished = Some("later".into());
        assert!(build(&doc).is_err());
    }

    #[test]
    fn steel_balcony_geometry_has_frame_deck_and_rail() {
        let mut doc = doc_with_block();
        let f = &mut doc.buildings[0].facades[0];
        f.placements.clear();
        put(&mut doc, 2, 2, "balcony.minaltan", None, None);
        let b = build(&doc).unwrap();
        let att = &b[0].walls[0].attachments;
        assert_eq!(att.len(), 2);
        let (lo, hi) = att.iter().flat_map(|e| e.parts.iter()).fold(([f64::MAX; 3], [f64::MIN; 3]), |(mut lo, mut hi), (_, m)| {
            if let Some((a, z)) = m.bounds() {
                for k in 0..3 {
                    lo[k] = lo[k].min(a[k]);
                    hi[k] = hi[k].max(z[k]);
                }
            }
            (lo, hi)
        });
        // South-facing test facade: the balcony projects 1300 mm towards -y and is 2700 wide.
        assert!((hi[0] - lo[0] - 2700.0).abs() < 1.0, "{lo:?} {hi:?}");
        assert!((lo[1] + 1300.0).abs() < 1.0, "{lo:?}");
        // Deck top at the storey floor (6000), frame 160 below, railing 1000 above (+10 edge).
        assert!((lo[2] - (6000.0 - 160.0)).abs() < 1.0 && (hi[2] - 7010.0).abs() < 1.0, "{lo:?} {hi:?}");
        // No hole in the wall for an attachment.
        assert!(b[0].walls[0].openings.is_empty());
    }

    #[test]
    fn explicit_levels_from_a_section() {
        let mut doc = doc_with_block();
        let b = &mut doc.buildings[0];
        // Frederikssundsvej 28, section S505/39: a 3.61 m ground floor, then 3.00 m storeys.
        b.set_levels(&[350.0, 3960.0, 6960.0, 9960.0, 12960.0], 15960.0).unwrap();
        assert_eq!(b.height(), 15960.0);
        assert_eq!(b.storeys[0].height, 3610.0);
        assert_eq!(b.ground_floor, 350.0);
        assert!(b.set_levels(&[0.0, 1000.0], 4000.0).is_err(), "a 1 m storey is refused");
        assert!(b.set_levels(&[3000.0, 1000.0], 5000.0).is_err(), "levels must rise");
        assert!(b.set_levels(&[f64::NAN], 3000.0).is_err());
        build(&doc).unwrap();
    }

    #[test]
    fn party_walls_refuse_openings() {
        let mut doc = doc_with_block();
        doc.buildings[0].facades[0].kind = FacadeKind::Party;
        assert!(build(&doc).is_err());
    }

    #[test]
    fn inner_ring_of_a_rectangle_is_offset_by_the_thickness() {
        let r = inner_ring(&[[0.0, 0.0], [10000.0, 0.0], [10000.0, 5000.0], [0.0, 5000.0]], 400.0);
        assert_eq!(r[0], [400.0, 400.0]);
        assert_eq!(r[2], [9600.0, 4600.0]);
    }

    #[test]
    fn bearing_of_a_south_facing_facade_is_180() {
        let f = FacadeFrame::new([0.0, 0.0], [10.0, 0.0]).unwrap();
        assert!((f.bearing() - 180.0).abs() < 1e-9);
    }
}
