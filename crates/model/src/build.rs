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

mod balcony;
mod dormer;
mod opening;

/// Window and door frames sit this far behind the outer wall face, mm.
pub const REVEAL: f64 = 120.0;
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
    /// An outdoor stair (a cellar stair with its pit).
    Stair,
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
    /// Actual convex outline for shaped openings; empty retains the legacy rectangle path.
    pub profile: Vec<[f64; 2]>,
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
    /// Rendered bands standing proud of the wall (empty without any).
    pub trim: Mesh,
    /// Material-separated bands when any explicit finish is used. `trim` remains their
    /// combined geometry for drawing/picking; display/export must choose these parts once.
    pub trim_parts: Vec<(Material, Mesh)>,
    /// The wall's outer finish: brick, red brick or render.
    pub finish: Material,
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
    /// Explicit host finishes for display/export; empty retains the legacy default Wall path.
    pub gable_parts: Vec<(Material, Mesh)>,
    pub covering: Vec<(Material, Mesh)>,
    /// Gutters along the eaves.
    pub gutters: Mesh,
    /// Explicit detail assemblies, empty for legacy roofs.
    pub drainage: Vec<crate::roof::drainage::DrainPart>,
    pub eaves: f64,
    pub ridge: f64,
    pub pitch_deg: f64,
    /// Multiple slopes/mitres require their actual shell in exchange, not one-plane extrusion.
    pub compound: bool,
    pub dormers: Vec<BuiltDormer>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct BuiltDormer {
    pub id: String,
    pub front: BuiltWall,
    pub cheeks: Mesh,
    pub roof: Mesh,
}

impl Built {
    /// Facade walls and roof-dormer fronts share the same opening/export contracts.
    pub fn exterior_walls(&self) -> impl Iterator<Item = &BuiltWall> {
        self.walls.iter().chain(self.roof.iter().flat_map(|r| r.dormers.iter().map(|d| &d.front)))
    }
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
    /// Site terrain is carried once, on the first building, for shared display/exchange.
    pub terrain: Option<Mesh>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct BuiltStair {
    pub id: String,
    pub level: usize,
    pub kind: crate::StairKind,
    pub winding: bool,
    pub mesh: Mesh,
    /// The balustrade along the well (empty for stairs between walls).
    pub railing: Mesh,
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
    crate::site::validate(doc)?;
    let mut built: Vec<_> = doc.buildings.iter().map(|b| build_building(doc, b, view, cut)).collect::<Result<_>>()?;
    if let Some(terrain) = &doc.terrain {
        let first = built.first_mut().ok_or_else(|| ModelError::Invalid("terrain needs a building site".into()))?;
        let mesh = terrain.mesh(&doc.buildings)?;
        let mesh = if let Some(z) = cut { mesh.below(z) } else { mesh };
        first.terrain = (!mesh.is_empty()).then_some(mesh);
    }
    Ok(built)
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
        ComponentKind::BalconySlab { thickness, .. } | ComponentKind::ShapedBalcony { thickness, .. } => *thickness,
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
        let bay_centre = *f.bays.get(p.bay).ok_or_else(|| ModelError::Invalid(format!("{slot}: the facade has {} bays", f.bays.len())))?;
        let c = crate::checked("placement u", p.u.unwrap_or(bay_centre), -1e8, 1e8)?;
        let mut ty_eff = ty.clone();
        ty_eff.width = p.width.unwrap_or(ty.width);
        ty_eff.height = p.height.unwrap_or(ty.height);
        ty_eff.sill = p.sill.unwrap_or(ty.sill);
        // Above ground a sill may drop to terrain. Basements instead permit openings
        // below terrain, bounded by their own floor; exposed portions/light wells are
        // site geometry and must not silently move the building opening upwards.
        let mut check = ty_eff.clone();
        check.sill = check.sill.max(0.0);
        check.validate().map_err(|e| ModelError::Invalid(format!("{slot}: {e}")))?;
        let basement_opening = p.level < b.ground_level && ty_eff.kind.is_opening();
        let minimum_sill = if basement_opening { 0.0 } else { -storey.elevation };
        if !ty_eff.sill.is_finite() || ty_eff.sill < minimum_sill {
            return Err(ModelError::Invalid(if basement_opening {
                format!("{slot}: sill {} mm would put the opening below the basement floor", ty_eff.sill)
            } else {
                format!("{slot}: sill {} mm would put the opening below the terrain (the floor is {:.0} mm above it)", ty_eff.sill, storey.elevation)
            }));
        }
        let opening = ty_eff.kind.is_opening();
        if opening && !b.edge_on_storey(f.edge, p.level) {
            return Err(ModelError::Invalid(format!("{slot}: the facade has no wall on this storey (the basement wall runs straight past it)")));
        }
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
            if b.attic_level() == Some(p.level) && rect.v0 < storey.elevation {
                return Err(ModelError::Invalid(format!(
                    "{slot}: roof-level facade openings must stay above their floor; host crossing stairwell windows on the full storey explicitly"
                )));
            }
            if b.attic_level().is_some_and(|l| p.level.checked_add(1) == Some(l)) && !p.spans_storeys {
                let ceiling = storey.elevation + storey.height - b.attic_thickness();
                if rect.v1 > ceiling {
                    return Err(ModelError::Invalid(format!(
                        "{slot}: the roof floor slab would collide with the opening; its underside is {ceiling} mm"
                    )));
                }
            }
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
            let attachment = if matches!(ty_eff.kind, ComponentKind::GuardRail { .. }) { "guard" } else { "balcony" };
            return Err(ModelError::Invalid(format!(
                "{slot}: the {} mm {attachment} at {c:.0} mm runs past the facade ends (0 to {:.0} mm)",
                ty_eff.width, frame.length
            )));
        }
        if let ComponentKind::Balcony { slab_width, .. } = ty_eff.kind
            && (c - slab_width / 2.0 < 0.0 || c + slab_width / 2.0 > frame.length)
        {
            return Err(ModelError::Invalid(format!("{slot}: the {slab_width} mm balcony slab runs past the facade end")));
        }
        // Only things that stand at the same time can collide: openings with openings,
        // attachments with attachments.
        let together = |o: &Resolved| o.lifetime.0 < lifetime.1 && lifetime.0 < o.lifetime.1 && o.is_opening() == opening;
        let gap = if opening { GAP } else { 0.0 };
        if let Some(other) = out.iter().find(|o| together(o) && o.rect.overlaps(&rect, gap)) {
            let what = if opening {
                format!("the opening at level {} bay {} (keep {GAP} mm between openings)", other.placement.level, other.placement.bay)
            } else {
                let attachment = if matches!(other.ty.kind, ComponentKind::GuardRail { .. }) { "guard" } else { "balcony" };
                format!("the {attachment} at level {} bay {}", other.placement.level, other.placement.bay)
            };
            return Err(ModelError::Invalid(format!("{slot}: overlaps {what}")));
        }
        out.push(Resolved { placement: p.clone(), ty: ty_eff, rect, lifetime });
    }
    Ok(out)
}

fn build_building(doc: &Document, b: &Building, view: View, cut: Option<f64>) -> Result<Built> {
    b.validate_attic()?;
    if b.floor_thicknesses.len() > b.storeys.len() {
        return Err(ModelError::Invalid("floor thicknesses must refer to existing storeys".into()));
    }
    for thickness in b.floor_thicknesses.iter().copied().chain(b.attic_thickness_mm).chain(b.roof_slab_thickness_mm) {
        crate::checked("slab thickness", thickness, 20.0, 1000.0)?;
    }
    let above = |z: f64| cut.is_some_and(|c| z >= c - 1.0);
    let top_at = |z: f64| cut.map_or(z, |c| z.min(c));
    let h = b.height();
    let n = b.footprint.len();
    let levels = b.storeys.len();
    let zb = b.bottom();
    // Each storey's inner wall line: the facade is thicker lower down and steps at every floor.
    let inners: Vec<Vec<[f64; 2]>> = (0..levels).map(|l| b.inner_line(l)).collect();
    // Each storey's own wall line (straight below ground where steps are cut off) and inner face.
    let rings: Vec<(Vec<[f64; 2]>, Vec<usize>)> = (0..levels).map(|l| b.storey_ring(l)).collect();
    let storey_inners: Vec<Vec<[f64; 2]>> = (0..levels).map(|l| b.storey_inner(l)).collect();
    for (l, inner) in inners.iter().enumerate() {
        let name = b.storeys.get(l).map_or("storey", |s| s.name.as_str());
        check_inner_ring(&b.footprint, inner).map_err(|e| ModelError::Invalid(format!("{name}: {e}")))?;
        if let (Some((ring, _)), Some(own)) = (rings.get(l), storey_inners.get(l)) {
            check_inner_ring(ring, own).map_err(|e| ModelError::Invalid(format!("{name}: {e}")))?;
        }
    }
    // A storey's wall runs from the underside of its floor to the underside of the next one.
    let heights: Vec<(f64, f64)> = (0..levels)
        .map(|l| {
            let z0 = if l == 0 { zb } else { b.storeys.get(l).map_or(zb, |s| s.elevation - b.floor_thickness(l)) };
            (z0, b.storeys.get(l + 1).map_or(h, |s| s.elevation - b.floor_thickness(l + 1)))
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
            if above(z0) || !b.edge_on_storey(f.edge, l) {
                continue;
            }
            spans.push(Span { level: l, thickness: b.edge_thickness(f.edge, l), z0, z1: top_at(z1), in0: along(qs), in1: along(qe), qs, qe });
        }
        // A step cut off below ground stands on its own foundation: run it down into the ground.
        if let Some(first) = spans.first_mut()
            && first.level > 0
            && !b.edge_on_storey(f.edge, first.level - 1)
        {
            first.z0 = first.z0.min(-PLINTH_FOOT);
        }
        if spans.is_empty() {
            continue;
        }
        let finish = f.finish.unwrap_or(b.finish).material();
        let all = resolve_facade(doc, b, f)?;
        // What this view shows, with each element's status.
        let shown: Vec<(&Resolved, Status)> =
            all.iter().filter(|r| !above(r.rect.v0)).filter_map(|r| view.status(r.lifetime).map(|s| (r, s))).collect();
        let resolved: Vec<(&Resolved, Status)> = shown.iter().filter(|(r, _)| r.is_opening()).copied().collect();
        let mut holes: Vec<Rect> = resolved.iter().map(|(r, _)| r.rect).collect();
        // Outdoor cellar stairs along this facade: each opens a door into the basement.
        let cellars: Vec<(String, &crate::doc::CellarStair, usize, Rect)> = b
            .cellar_stairs
            .iter()
            .enumerate()
            .filter(|(_, c)| c.facade == f.id)
            .map(|(k, c)| {
                c.validate(fr.length).map_err(|e| ModelError::Invalid(format!("cellar stair {k} on {}: {e}", f.id)))?;
                let id = if c.id.is_empty() { format!("{}.cellar{k}", f.id) } else { c.id.clone() };
                let level = (0..levels).rev().find(|&l| b.storeys.get(l).is_some_and(|s| s.elevation <= c.floor + 1.0)).unwrap_or(0);
                let rect = Rect { u0: c.door_at - c.door_width / 2.0, u1: c.door_at + c.door_width / 2.0, v0: c.floor, v1: c.floor + CELLAR_DOOR };
                Ok((id, c, level, rect))
            })
            .collect::<Result<_>>()?;
        let cellars: Vec<_> = cellars.into_iter().filter(|(_, c, _, _)| !above(c.floor)).collect();
        holes.extend(cellars.iter().map(|(_, _, _, r)| *r));
        let unbuildable = || ModelError::Invalid(format!("facade {}: the wall faces could not be triangulated", f.id));
        let shaped = resolved.iter().any(|(r, _)| {
            matches!(
                r.ty.kind,
                ComponentKind::HistoricWindow { shape: crate::component::WindowShape::Segmental { .. } | crate::component::WindowShape::Round, .. }
            )
        });
        let profiles: Vec<_> = resolved
            .iter()
            .map(|(r, _)| crate::profile::opening(&r.ty.kind, r.rect))
            .chain(cellars.iter().map(|(_, _, _, r)| crate::profile::outline(&crate::component::WindowShape::Rectangular, *r)))
            .collect();
        let mesh = if shaped { crate::walls::skin_profiles(&fr, &spans, &profiles) } else { crate::walls::skin(&fr, &spans, &holes) }
            .ok_or_else(unbuildable)?;
        let segments = spans
            .iter()
            .map(|s| {
                if shaped {
                    crate::walls::segment_profiles(&fr, &format!("{}.L{}", f.id, s.level), s, &profiles)
                } else {
                    crate::walls::segment(&fr, &format!("{}.L{}", f.id, s.level), s, &holes)
                }
                .ok_or_else(unbuildable)
            })
            .collect::<Result<Vec<_>>>()?;

        let elevation_of = |r: &Resolved| b.storeys.get(r.placement.level).map_or(0.0, |s| s.elevation);
        let openings = resolved
            .iter()
            .map(|(r, status)| {
                let id = format!("{}.L{}.B{}", f.id, r.placement.level, r.placement.bay);
                let level = r.placement.level;
                let at = Fill {
                    wall: b.edge_thickness(f.edge, level),
                    finish,
                    floor: elevation_of(r),
                    ceiling: b.storeys.get(level + 1).map_or(h, |s| s.elevation - b.floor_thickness(level + 1)),
                };
                let mut o = component(&fr, &id, r, elevation_of(r), *status, at);
                if cut.is_some() {
                    o.fill = clip_element(o.fill, cut);
                    o.attached = o.attached.into_iter().map(|e| clip_element(e, cut)).collect();
                }
                o
            })
            .collect();
        let mut attachments = Vec::new();
        for (r, status) in shown.iter().filter(|(r, _)| !r.is_opening()) {
            let id = format!("{}.L{}.B{}.balcony", f.id, r.placement.level, r.placement.bay);
            attachments.extend(attachment(&fr, &id, r, elevation_of(r), *status)?.into_iter().map(|e| clip_element(e, cut)));
        }
        let mut openings: Vec<BuiltOpening> = openings;
        for (id, c, level, rect) in &cellars {
            let at = Fill {
                wall: b.edge_thickness(f.edge, *level),
                finish,
                floor: c.floor,
                ceiling: b.storeys.get(level + 1).map_or(h, |s| s.elevation - b.floor_thickness(level + 1)),
            };
            let fill = BuiltElement {
                id: format!("{id}.door"),
                name: "Kælderdør".into(),
                class: ElementClass::Door,
                type_id: "door.cellar".into(),
                level: *level,
                width: c.door_width,
                height: CELLAR_DOOR,
                parts: fill_parts(&fr, *rect, &ComponentKind::EntranceDoor, at),
                status: Status::Existing,
            };
            openings.push(BuiltOpening {
                profile: Vec::new(),
                id: format!("{id}.opening"),
                level: *level,
                rect: *rect,
                fill: clip_element(fill, cut),
                attached: Vec::new(),
                status: Status::Existing,
            });
            attachments.push(clip_element(cellar_stair(&fr, id, c, *level), cut));
        }
        let plinth = match (&b.plinth, &plinth_line) {
            (Some(p), Some(line)) => {
                let m = facing_piece(
                    &fr,
                    f,
                    (a, e),
                    line,
                    p.projection.max(1.0),
                    (-PLINTH_FOOT, p.height),
                    &holes,
                    shaped.then_some(profiles.as_slice()),
                )
                .ok_or_else(unbuildable)?;
                match cut {
                    Some(z) => m.below(z),
                    None => m,
                }
            }
            _ => Mesh::default(),
        };
        // Rendered bands standing proud of the wall, on each of their storeys.
        let mut trim = Mesh::default();
        let explicit_finishes = f.bands.iter().any(|band| band.finish.is_some());
        let mut trim_parts = std::collections::BTreeMap::<Material, Mesh>::new();
        if f.bands.len() > 50 {
            return Err(ModelError::Invalid("at most 50 facade bands are allowed".into()));
        }
        for band in &f.bands {
            band.validate(levels)?;
            let projection = crate::checked("band projection", band.projection, 1.0, 500.0)?;
            let line = inner_ring(&b.footprint, -projection);
            for &level in &band.levels {
                let Some(s) = b.storeys.get(level) else { continue };
                let (z0, z1) = (s.elevation + band.bottom, s.elevation + band.top);
                if !(z1 - z0 >= 10.0) || above(z0) {
                    continue;
                }
                let bearing_cap =
                    (z1 - h).abs() < 1e-6 && matches!(&b.roof, crate::Roof::Pitched { drainage: Some(_), gables, .. } if !gables.contains(&f.id));
                let mut piece = facing_piece(
                    &fr,
                    f,
                    (a, e),
                    &line,
                    projection,
                    (z0, if bearing_cap { z1 } else { top_at(z1) }),
                    &holes,
                    shaped.then_some(profiles.as_slice()),
                )
                .ok_or_else(unbuildable)?;
                if bearing_cap {
                    seat_cornice(&mut piece, b, f, &fr, z1)?;
                    if let Some(cut) = cut {
                        piece = piece.below(cut);
                    }
                }
                trim.append(&piece);
                if explicit_finishes {
                    trim_parts.entry(band.finish.map_or(Material::Render, |finish| finish.material())).or_default().append(&piece);
                }
            }
        }
        walls.push(BuiltWall {
            id: f.id.clone(),
            frame: fr,
            kind: f.kind,
            height: h,
            bottom: zb,
            mesh,
            finish,
            segments,
            plinth,
            trim,
            trim_parts: trim_parts.into_iter().collect(),
            openings,
            attachments,
        });
    }

    // Straight basement walls past cut-off steps: one wall per such run, on those storeys only.
    for (l, (ring, kept)) in rings.iter().enumerate() {
        let m = ring.len();
        if m == n || above(heights.get(l).map_or(0.0, |z| z.0)) {
            continue;
        }
        let Some(own) = storey_inners.get(l) else { continue };
        for i in 0..m {
            let (Some(&k0), Some(&k1)) = (kept.get(i), kept.get((i + 1) % m)) else { continue };
            if (k0 + 1) % n == k1 {
                continue;
            }
            let (Some(&a), Some(&e), Some(&qs), Some(&qe), Some(&(z0, z1))) =
                (ring.get(i), ring.get((i + 1) % m), own.get(i), own.get((i + 1) % m), heights.get(l))
            else {
                continue;
            };
            let Some(fr) = FacadeFrame::new(a, e) else { continue };
            let along = |q: [f64; 2]| (q[0] - a[0]) * fr.t[0] + (q[1] - a[1]) * fr.t[1];
            let span = Span { level: l, thickness: b.edge_thickness(k0, l), z0, z1: top_at(z1), in0: along(qs), in1: along(qe), qs, qe };
            let id = format!("{}.basement{k0}", b.id);
            let unbuildable = || ModelError::Invalid(format!("{id}: the wall faces could not be triangulated"));
            let mesh = crate::walls::skin(&fr, &[span], &[]).ok_or_else(unbuildable)?;
            let segment = crate::walls::segment(&fr, &format!("{id}.L{l}"), &span, &[]).ok_or_else(unbuildable)?;
            let kind = b.facades.iter().find(|f| f.edge == k0).map_or(FacadeKind::Unknown, |f| f.kind);
            walls.push(BuiltWall {
                id,
                frame: fr,
                kind,
                height: span.z1,
                bottom: z0,
                mesh,
                finish: Material::Wall,
                segments: vec![segment],
                plinth: Mesh::default(),
                trim: Mesh::default(),
                trim_parts: Vec::new(),
                openings: Vec::new(),
                attachments: Vec::new(),
            });
        }
    }

    // Floors span to the inner face of the storey's own wall and rest on the step of the thicker wall below.
    let mut slabs = Vec::with_capacity(levels + 1);
    let openings =
        |level: usize| -> Result<Vec<Vec<[f64; 2]>>> { b.stairs.iter().filter(|st| st.levels.contains(&level)).map(crate::stair::opening).collect() };
    for (i, s) in b.storeys.iter().enumerate() {
        let inner = storey_inners.get(i).cloned().unwrap_or_default();
        let thickness = b.floor_thickness(i);
        let z0 = s.elevation - thickness;
        if above(z0) {
            continue;
        }
        let holes = match i.checked_sub(1) {
            Some(l) => openings(l)?,
            None => Vec::new(),
        };
        let (inner, holes) = slab_shape(&inner, holes)?;
        slabs.push(BuiltSlab {
            id: if s.roof_level { format!("{}.attic", b.id) } else { format!("{}.slab.L{i}", b.id) },
            kind: SlabKind::Floor,
            level: i,
            mesh: prism_with_holes(&inner, &holes, z0, thickness),
            polygon: inner,
            z_bottom: z0,
            thickness,
            holes,
        });
    }
    let top_level = levels.saturating_sub(1);
    let top_inner = inners.get(top_level).cloned().unwrap_or_default();
    let roof = match &b.roof {
        _ if above(h - b.ceiling_thickness(top_level)) => None,
        crate::roof::Roof::Flat => {
            let (outline, holes) = slab_shape(&b.footprint, openings(top_level)?)?;
            slabs.push(BuiltSlab {
                id: format!("{}.roof", b.id),
                kind: SlabKind::Roof,
                level: top_level,
                z_bottom: h,
                thickness: b.roof_slab_thickness_mm.unwrap_or(ROOF_SLAB),
                mesh: prism_with_holes(&outline, &holes, h, b.roof_slab_thickness_mm.unwrap_or(ROOF_SLAB)),
                polygon: outline,
                holes,
            });
            None
        }
        crate::roof::Roof::Pitched { gables, straighten_mm, overhang_mm, verge_mm, thickness_mm, gutters, .. }
        | crate::roof::Roof::Mansard {
            parameters: crate::roof::Mansard { gables, straighten_mm, overhang_mm, verge_mm, thickness_mm, gutters, .. },
        } => {
            if b.attic_level().is_none() {
                let (top_inner, holes) = slab_shape(&top_inner, openings(top_level)?)?;
                // The attic floor closes the top storey; the roof rises from the eaves above it.
                slabs.push(BuiltSlab {
                    id: format!("{}.attic", b.id),
                    kind: SlabKind::Floor,
                    level: top_level,
                    mesh: prism_with_holes(&top_inner, &holes, h - b.attic_thickness(), b.attic_thickness()),
                    polygon: top_inner,
                    z_bottom: h - b.attic_thickness(),
                    thickness: b.attic_thickness(),
                    holes,
                });
            }
            let selected = match &b.roof {
                crate::Roof::Pitched { eave_facades, .. } => eave_facades.as_deref(),
                _ => None,
            };
            let (kept, outline) = if let Some(ids) = selected {
                crate::roof::selected_eave_outline(b, ids)?
            } else {
                let kept = crate::roof::eave_outline(&b.footprint, *straighten_mm);
                let outline = kept.iter().filter_map(|&i| b.footprint.get(i).copied()).collect();
                (kept, outline)
            };
            // A gable facade is an edge of the eave outline when both its corners were kept next to each other.
            let mut edges = Vec::with_capacity(gables.len());
            for g in gables {
                let f = b.facade(g)?;
                let (i, j) = (f.edge, (f.edge + 1) % n);
                let pos = kept.iter().position(|&k| k == i);
                if selected.is_some() {
                    let p = pos.ok_or_else(|| ModelError::Invalid(format!("gable {g} is absent from eave_facades; select its source facade")))?;
                    let (a, c) = b.edge(i).ok_or_else(|| ModelError::Invalid(format!("gable {g} has no source edge")))?;
                    if [a, c].iter().zip([outline[p], outline[(p + 1) % outline.len()]]).any(|(a, c)| (a[0] - c[0]).hypot(a[1] - c[1]) > 1e-6) {
                        return Err(ModelError::Invalid(format!(
                            "gable {g} must retain its source endpoints; include its adjacent facade lines in eave_facades"
                        )));
                    }
                    edges.push(p);
                } else {
                    let pos = pos.filter(|&p| kept.get((p + 1) % kept.len()) == Some(&j));
                    edges.push(pos.ok_or_else(|| ModelError::Invalid(format!("gable {g} was straightened away; lower straighten_mm")))?);
                }
            }
            let buildup = crate::roof::Buildup { overhang: *overhang_mm, verge: *verge_mm, thickness: *thickness_mm, gutters: *gutters };
            let (mut r, pitch_deg, compound) = match &b.roof {
                crate::Roof::Pitched { pitch_deg, .. } => (crate::roof::pitched_with(&outline, h, *pitch_deg, &edges, &buildup)?, *pitch_deg, false),
                crate::Roof::Mansard { parameters } => (crate::roof::mansard(&outline, h, &edges, parameters)?, parameters.lower_pitch_deg, true),
                crate::Roof::Flat => return Err(ModelError::Invalid("a sloping roof configuration is required".into())),
            };
            let mut drain_parts = Vec::new();
            if let crate::Roof::Pitched { drainage: Some(d), .. } = &b.roof {
                if !gutters {
                    return Err(ModelError::Invalid("detailed drainage requires gutters".into()));
                }
                let out = inner_ring_by(&outline, |k| if edges.contains(&k) { -*verge_mm } else { -*overhang_mm });
                drain_parts = if selected.is_some() {
                    crate::roof::drainage::build_selected(b, &kept, &out, h, *overhang_mm, pitch_deg, &edges, d)?
                } else {
                    crate::roof::drainage::build(b, &kept, &out, h, *overhang_mm, pitch_deg, &edges, d)?
                };
                r.gutters = Mesh::default();
                for part in &drain_parts {
                    r.gutters.append(&part.mesh);
                }
            }
            let dormers = dormer::build(doc, b, &mut r.cover, b.roof.dormers())?;
            let mut gable_parts = std::collections::BTreeMap::<Material, Mesh>::new();
            if compound || walls.iter().any(|w| gables.contains(&w.id) && w.finish != Material::Wall) {
                for tri in &r.gables.triangles {
                    let points = tri.map(|i| r.gables.positions.get(i as usize).copied().unwrap_or_default());
                    let wall = walls
                        .iter()
                        .find(|w| {
                            points.iter().all(|p| {
                                let d = [p[0] - w.frame.origin[0], p[1] - w.frame.origin[1]];
                                let along = d[0] * w.frame.t[0] + d[1] * w.frame.t[1];
                                (d[0] * w.frame.n[0] + d[1] * w.frame.n[1]).abs() < 0.01 && along >= -0.01 && along <= w.frame.length + 0.01
                            })
                        })
                        .ok_or_else(|| ModelError::Invalid("roof gable has no matching host facade".into()))?;
                    let mesh = gable_parts.entry(wall.finish).or_default();
                    let base = mesh.positions.len() as u32;
                    mesh.positions.extend(points);
                    mesh.triangles.push([base, base + 1, base + 2]);
                }
            }
            let covering = if let crate::Roof::Mansard { parameters } = &b.roof {
                match &parameters.covering {
                    Some(spec) => crate::roof::covering(&r.cover, parameters.lower_pitch_deg, parameters.upper_pitch_deg, spec)?,
                    None => Vec::new(),
                }
            } else {
                Vec::new()
            };
            let ridge = covering.iter().flat_map(|(_, m)| m.positions.iter().map(|p| p[2])).fold(r.ridge, f64::max);
            Some(BuiltRoof {
                id: format!("{}.roof", b.id),
                cover: r.cover,
                gables: r.gables,
                gable_parts: gable_parts.into_iter().collect(),
                covering,
                gutters: r.gutters,
                drainage: drain_parts,
                eaves: h,
                ridge,
                pitch_deg,
                compound,
                dormers,
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
        let wall_height = top_at(s.elevation + s.height - b.ceiling_thickness(level)) - s.elevation;
        let axes: Vec<Axis> = on.iter().map(|(_, w)| Axis { a: w.a, b: w.b, thickness: w.thickness }).collect();
        let joined = crate::joins::join(&axes, storey_inners.get(level).map_or(&[][..], Vec::as_slice), b.thickest_wall(level));
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
            let (mesh, railing) = (crate::stair::mesh(st, s.elevation, s.height)?, crate::stair::railing(st, s.elevation, s.height)?);
            let (mesh, railing) = match cut {
                Some(z) => (mesh.below(z), railing.below(z)),
                None => (mesh, railing),
            };
            stairs.push(BuiltStair { id: format!("{id}.L{level}"), level, kind: st.kind, winding: st.winders.is_some(), mesh, railing });
        }
    }
    Ok(Built { building_id: b.id.clone(), name: b.name.clone(), storeys: b.storeys.clone(), walls, slabs, roof, interior, stairs, terrain: None })
}

/// The plinth runs this far below the terrain, mm.
const PLINTH_FOOT: f64 = 300.0;

/// A cap explicitly ending at the wall bearing on a detailed pitched roof follows the soffit.
/// Its declared height remains the height at the wall; projection consumes height at tan(pitch).
/// Moving only the upper face preserves a closed solid and all opening edges/material ownership.
fn seat_cornice(piece: &mut Mesh, b: &Building, f: &Facade, fr: &FacadeFrame, top: f64) -> Result<()> {
    let crate::Roof::Pitched { pitch_deg, gables, straighten_mm, overhang_mm, drainage: Some(_), .. } = &b.roof else {
        return Ok(());
    };
    if (top - b.height()).abs() > 1e-6 || gables.contains(&f.id) {
        return Ok(());
    }
    if let crate::Roof::Pitched { eave_facades: Some(ids), .. } = &b.roof {
        let (owners, _) = crate::roof::selected_eave_outline(b, ids)?;
        if !owners.contains(&f.edge) {
            return Err(ModelError::Invalid(format!(
                "roof-bearing cornice on {} needs its source facade in eave_facades; revise the cap or select its eave line",
                f.id
            )));
        }
    } else {
        let kept = crate::roof::eave_outline(&b.footprint, *straighten_mm);
        if !kept.iter().enumerate().any(|(k, &i)| i == f.edge && kept[(k + 1) % kept.len()] == (f.edge + 1) % b.footprint.len()) {
            return Err(ModelError::Invalid("a roof-bearing cornice needs an eave facade retained by roof straightening".into()));
        }
    }
    let rise = crate::checked("cornice roof pitch", *pitch_deg, 5.0, 75.0)?.to_radians().tan();
    let overhang = crate::checked("cornice roof overhang", *overhang_mm, 0.0, 2000.0)?;
    for p in &mut piece.positions {
        let outward = ((p[0] - fr.origin[0]) * fr.n[0] + (p[1] - fr.origin[1]) * fr.n[1]).max(0.0);
        let soffit = top - outward * rise;
        if outward > overhang + 1e-6 {
            return Err(ModelError::Invalid("a roof-bearing cornice projection must fit under the roof overhang".into()));
        }
        if (p[2] - top).abs() < 1e-6 {
            p[2] = soffit;
        } else if p[2] > soffit - 1.0 {
            return Err(ModelError::Invalid(
                "a roof-bearing cornice needs a lower bottom/opening head: projection times tan(pitch) plus 1 mm clear below its top".into(),
            ));
        }
    }
    Ok(())
}

/// A facing in front of one facade (a plinth, a band): a closed piece from `z0` to `z1` between
/// the wall's outer face and the facing's face `projection` mm out along `line` (the footprint
/// carried out by that much), with the facade's openings cut through it.
fn facing_piece(
    fr: &FacadeFrame,
    f: &Facade,
    (a, e): ([f64; 2], [f64; 2]),
    line: &[[f64; 2]],
    projection: f64,
    (z0, z1): (f64, f64),
    holes: &[Rect],
    profiles: Option<&[Vec<[f64; 2]>]>,
) -> Option<Mesh> {
    let n = line.len();
    let (s, t) = (*line.get(f.edge)?, *line.get((f.edge + 1) % n)?);
    let face = FacadeFrame::new(s, t)?;
    let along = |q: [f64; 2]| (q[0] - s[0]) * face.t[0] + (q[1] - s[1]) * face.t[1];
    // Openings, from the wall's frame into the plinth's (which starts further out at the corner).
    let shift = along(fr.origin);
    let holes: Vec<Rect> = holes.iter().map(|r| Rect { u0: r.u0 + shift, u1: r.u1 + shift, ..*r }).collect();
    let span = Span { level: 0, thickness: projection, z0, z1, in0: along(a), in1: along(e), qs: a, qe: e };
    if let Some(profiles) = profiles {
        let profiles: Vec<_> = profiles.iter().map(|poly| poly.iter().map(|p| [p[0] + shift, p[1]]).collect()).collect();
        crate::walls::segment_profiles(&face, &format!("{}.facing", f.id), &span, &profiles).map(|seg| seg.mesh)
    } else {
        crate::walls::segment(&face, &format!("{}.facing", f.id), &span, &holes).map(|seg| seg.mesh)
    }
}

/// Cellar door height, mm.
const CELLAR_DOOR: f64 = 2000.0;
const RETAINING: f64 = 150.0;

/// An outdoor cellar stair's pit in facade coordinates: retaining walls with a kerb, the pit
/// floor, the flight down along the wall, and a railing on the walls except at the top of the
/// flight, where the stair is entered.
fn cellar_stair(fr: &FacadeFrame, id: &str, c: &crate::doc::CellarStair, level: usize) -> BuiltElement {
    const KERB: f64 = 100.0;
    let (out, foot) = (-c.width, c.floor - 200.0);
    let entry_at_to = c.top > c.bottom;
    let mut concrete = Mesh::default();
    concrete.append(&fbox(fr, (c.from - RETAINING, c.to + RETAINING), (out - RETAINING, out), (foot, KERB)));
    concrete.append(&fbox(fr, (c.from - RETAINING, c.from), (out, 0.0), (foot, if entry_at_to { KERB } else { 0.0 })));
    concrete.append(&fbox(fr, (c.to, c.to + RETAINING), (out, 0.0), (foot, if entry_at_to { 0.0 } else { KERB })));
    concrete.append(&fbox(fr, (c.from, c.to), (out, 0.0), (foot, c.floor)));
    // Steps from the pit floor up to the terrain.
    let risers = ((-c.floor) / 175.0).round().clamp(3.0, 40.0) as usize;
    let rise = -c.floor / risers as f64;
    let going = (c.top - c.bottom) / risers as f64;
    let mut steps = Mesh::default();
    for i in 0..risers {
        let (s0, s1) = (c.bottom + going * i as f64, c.bottom + going * (i + 1) as f64);
        steps.append(&fbox(fr, (s0.min(s1), s0.max(s1)), (out, 0.0), (c.floor, c.floor + rise * (i + 1) as f64)));
    }
    // Railing on the kerb: along the outer wall, and across the end away from the entry.
    let rail_w = out - RETAINING / 2.0;
    let mut rail = rail_run(fr, (c.from - RETAINING / 2.0, rail_w), (c.to + RETAINING / 2.0, rail_w), KERB, 900.0);
    let closed_end = if entry_at_to { c.from - RETAINING / 2.0 } else { c.to + RETAINING / 2.0 };
    rail.append(&rail_run(fr, (closed_end, rail_w), (closed_end, -100.0), KERB, 900.0));
    BuiltElement {
        id: id.to_string(),
        name: "Kældernedgang".into(),
        class: ElementClass::Stair,
        type_id: "stair.cellar".into(),
        level,
        width: c.to - c.from,
        height: -c.floor,
        parts: vec![(Material::Slab, concrete), (Material::Sill, steps), (Material::Railing, rail)],
        status: Status::Existing,
    }
}

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
        let door_id = format!("{id}.D{k}");
        let fill = BuiltElement {
            id: door_id.clone(),
            name: "Dør".into(),
            class: ElementClass::Door,
            type_id: "door.interior".into(),
            level,
            width: u1 - u0,
            height: rect.v1 - rect.v0,
            parts: opening::interior(&fr, rect, d).finish(),
            status: Status::Existing,
        };
        doors.push(BuiltOpening { id: door_id, level, rect, profile: Vec::new(), fill, attached: Vec::new(), status: Status::Existing });
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

/// What a window or door needs to know about where it sits: the wall's thickness and finish,
/// and the storey's floor and ceiling (underside of the floor above), mm.
#[derive(Debug, Clone, Copy)]
struct Fill {
    wall: f64,
    finish: Material,
    floor: f64,
    ceiling: f64,
}

/// Partition a composite without moving its nominal boundary. A legacy loggia can have a
/// door_width larger than its opening; keep both generated subdivisions positive in that case.
fn opening_split(r: Rect, width: f64, side: Side) -> (Rect, Rect) {
    let margin = 100.0_f64.min((r.u1 - r.u0) * 0.1);
    let width = width.clamp(margin, r.u1 - r.u0 - margin);
    match side {
        Side::Left => (Rect { u1: r.u0 + width, ..r }, Rect { u0: r.u0 + width, ..r }),
        Side::Right => (Rect { u0: r.u1 - width, ..r }, Rect { u1: r.u1 - width, ..r }),
    }
}

/// A 1 mm assembly joint between the two factory frames. Keeping their exposed ends
/// separate avoids coincident faces; the nominal subdivision and scheduled sizes stay intact.
fn opening_joint(mut door: Rect, mut window: Rect, side: Side) -> (Rect, Rect) {
    match side {
        Side::Left => {
            door.u1 -= 0.5;
            window.u0 += 0.5;
        }
        Side::Right => {
            door.u0 += 0.5;
            window.u1 -= 0.5;
        }
    }
    (door, window)
}

/// Frame, sashes or leaves and glazing for a window or door in `rect`.
fn fill_parts(fr: &FacadeFrame, rect: Rect, kind: &ComponentKind, at: Fill) -> Vec<(Material, Mesh)> {
    let Rect { u0, u1, v0, v1 } = rect;
    // The family reveal is measured from the exterior wall face. A thin host cannot
    // accommodate the usual 120 mm reveal plus the 70 mm frame: seat its rear at
    // the inner wall face instead of leaving the assembly floating behind the wall.
    let reveal = REVEAL.min((at.wall - FRAME_DEPTH).max(0.0));
    let w = (reveal, reveal + FRAME_DEPTH);
    let mut detail = opening::Parts::default();
    let mut parts = Vec::new();
    match kind {
        ComponentKind::HistoricWindow { shape, lights, top_light, panes_across, panes_down, muntins, muntin_width_mm, operable } => {
            detail =
                opening::historic_window(fr, rect, w, shape, *lights, *top_light, *panes_across, *panes_down, *muntin_width_mm, *muntins, *operable);
            if !matches!(shape, crate::component::WindowShape::Round) {
                detail.sill = opening::sill(fr, (u0, u1), v0, reveal, 30.0);
            }
        }
        ComponentKind::PanelDoor { leaves, panel_columns, panel_rows, glazed_rows } => {
            detail = opening::panel_door(fr, rect, w, *leaves, *panel_columns, *panel_rows, *glazed_rows);
        }
        ComponentKind::Window { lights, top_light } => {
            detail = opening::window(fr, rect, w, *lights, *top_light, true, false);
            detail.sill = opening::sill(fr, (u0, u1), v0, reveal, 30.0);
        }
        ComponentKind::FixedWindow => {
            detail = opening::window(fr, rect, w, 1, 0.0, false, false);
            detail.sill = opening::sill(fr, (u0, u1), v0, reveal, 30.0);
        }
        ComponentKind::EntranceDoor => {
            let gate = u1 - u0 > 2000.0;
            let top = if gate { 0.0 } else { (v1 - v0 - 2100.0).clamp(0.0, 600.0) };
            detail = opening::door(fr, rect, w, if gate { 2 } else { 1 }, !gate, top, false);
        }
        ComponentKind::FrenchDoor { leaves } | ComponentKind::FrenchBalcony { leaves, .. } | ComponentKind::Balcony { leaves, .. } => {
            detail = opening::door(fr, rect, w, *leaves, true, 0.0, false);
        }
        ComponentKind::DoorWindow { door_width, door_side, window_sill, lights } => {
            let (dr, wr) = opening_split(rect, *door_width, *door_side);
            let (dr, wr) = opening_joint(dr, wr, *door_side);
            detail = opening::door(fr, dr, w, 1, true, 0.0, *door_side == Side::Right);
            let ws = v0 + window_sill;
            if ws > v0 {
                parts.push((at.finish, fbox(fr, (wr.u0, wr.u1), (0.0, w.1), (v0, ws))));
            }
            detail.append(opening::window(fr, Rect { v0: ws, ..wr }, w, *lights, 0.0, true, *door_side == Side::Left));
            detail.sill.append(&opening::sill(fr, (wr.u0, wr.u1), ws, reveal, 0.0));
        }
        ComponentKind::Loggia { depth, rail_height, door_width, door_side } => {
            // Set back behind the facade: brick returns, a soffit, and the back wall with its
            // glazed door and window; a guard rail across the opening.
            let back = depth.max(at.wall + 300.0);
            let (floor, ceiling) = (at.floor, at.ceiling.max(v1 + 1.0));
            const RETURN: f64 = 110.0;
            let mut brick = Mesh::default();
            brick.append(&fbox(fr, (u0 - RETURN, u0), (at.wall, back + RETURN), (floor, ceiling)));
            brick.append(&fbox(fr, (u1, u1 + RETURN), (at.wall, back + RETURN), (floor, ceiling)));
            brick.append(&fbox(fr, (u0, u1), (back, back + RETURN), (v1, ceiling)));
            parts.push((Material::BrickRed, brick));
            parts.push((Material::Render, fbox(fr, (u0, u1), (at.wall, back), (v1, ceiling))));
            let gw = (back - FRAME_DEPTH, back);
            let (dr, wr) = opening_split(Rect { v0: floor, ..rect }, *door_width, *door_side);
            let (dr, wr) = opening_joint(dr, wr, *door_side);
            detail.append(opening::door(fr, dr, gw, 1, true, 0.0, *door_side == Side::Right));
            let ws = (floor + 800.0).min(v1 - 100.0);
            parts.push((Material::BrickRed, fbox(fr, (wr.u0, wr.u1), (back - 20.0, back + RETURN), (floor, ws))));
            detail.append(opening::window(fr, Rect { v0: ws, ..wr }, gw, 2, 0.0, true, *door_side == Side::Left));
            // Two white rails and end posts across the opening, over the parapet.
            let (top, mid) = (floor + rail_height, (v0 + floor + rail_height) / 2.0);
            let wr = at.wall * 0.4;
            let mut rail = Mesh::default();
            for z in [top, mid] {
                rail.append(&fbox(fr, (u0, u1), (wr - 15.0, wr + 15.0), (z - 30.0, z)));
            }
            for u in [u0 + 15.0, u1 - 45.0] {
                rail.append(&fbox(fr, (u, u + 30.0), (wr - 15.0, wr + 15.0), (v0, top)));
            }
            parts.push((Material::SteelWhite, rail));
        }
        ComponentKind::BalconySlab { .. }
        | ComponentKind::SteelBalcony { .. }
        | ComponentKind::GuardRail { .. }
        | ComponentKind::ShapedBalcony { .. } => return Vec::new(),
    }
    parts.extend(detail.finish());
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

fn component(fr: &FacadeFrame, id: &str, r: &Resolved, elevation: f64, status: Status, at: Fill) -> BuiltOpening {
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
        parts: fill_parts(fr, r.rect, &ty.kind, at),
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
    let profile = if matches!(ty.kind, ComponentKind::HistoricWindow { .. }) { crate::profile::opening(&ty.kind, r.rect) } else { Vec::new() };
    BuiltOpening { id: format!("{id}.opening"), level, rect: r.rect, profile, fill, attached, status }
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

/// An attachment hung on the wall without an opening: balcony parts or an independent guard.
fn attachment(fr: &FacadeFrame, id: &str, r: &Resolved, elevation: f64, status: Status) -> Result<Vec<BuiltElement>> {
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
    Ok(match ty.kind {
        ComponentKind::GuardRail { projection, railing, finish, handrail } => {
            let metal = match finish {
                Finish::Galvanised => Material::Steel,
                Finish::White => Material::SteelWhite,
                Finish::Anthracite => Material::Railing,
            };
            let metal_height = ty.height - if handrail { 40.0 } else { 0.0 };
            // Inset flat rails; the rail-free style instead ends at its outer post faces.
            let inset = if railing == Railing::Osterbro { 0.0 } else { 20.0 };
            let mut guard = styled_rail(fr, (u0 + inset, -projection), (u1 - inset, -projection), deck_top, metal_height, railing);
            // Parametric presentation brackets and wall plates; no structural capacity is inferred.
            for u in [u0 + inset + 20.0, u1 - inset - 20.0] {
                for z in [deck_top + 150.0, deck_top + metal_height - 150.0] {
                    guard.append(&fbox(fr, (u - 20.0, u + 20.0), (-projection, 0.0), (z, z + 20.0)));
                    guard.append(&fbox(fr, (u - 20.0, u + 20.0), (-8.0, 0.0), (z - 20.0, z + 40.0)));
                }
            }
            let mut parts = vec![(metal, guard)];
            if handrail {
                parts.push((
                    Material::Deck,
                    fbox(fr, (u0, u1), (-projection - 35.0, -projection + 35.0), (deck_top + metal_height, deck_top + ty.height)),
                ));
            }
            vec![mk("rail", &ty.name, ElementClass::Railing, parts)]
        }
        ComponentKind::ShapedBalcony { depth, thickness, ref plan } => {
            let (slab, rail) = balcony::meshes(fr, (u0 + u1) / 2.0, ty.width, depth, thickness, ty.height, deck_top, plan)?;
            vec![
                mk("slab", &ty.name, ElementClass::BalconySlab, vec![(Material::Slab, slab)]),
                mk("rail", &format!("{} rail", ty.name), ElementClass::Railing, vec![(Material::Railing, rail)]),
            ]
        }
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
                Finish::White => Material::SteelWhite,
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
    })
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

    #[test]
    fn a_basement_wall_runs_straight_past_street_bays() {
        // A 30 × 12 m wing whose street side (y = 12000) has two 800 mm bays.
        let fp = [
            [0.0, 0.0],
            [30000.0, 0.0],
            [30000.0, 12000.0],
            [24000.0, 12000.0],
            [24000.0, 12800.0],
            [18000.0, 12800.0],
            [18000.0, 12000.0],
            [12000.0, 12000.0],
            [12000.0, 12800.0],
            [6000.0, 12800.0],
            [6000.0, 12000.0],
            [0.0, 12000.0],
        ];
        let mut doc = Document::default();
        let mut b = Building::new("b1", "Test", &fp, 3, 3000.0).unwrap();
        b.add_basement(-2500.0, "Kælder").unwrap();
        for f in &mut b.facades {
            let (p, q) = (fp[f.edge], fp[(f.edge + 1) % fp.len()]);
            if p[1] > 11000.0 && q[1] > 11000.0 {
                f.kind = FacadeKind::Street;
            }
        }
        b.basement_straighten_mm = Some(1000.0);
        doc.buildings.push(b);
        let b = &doc.buildings[0];
        let (ring, _) = b.storey_ring(0);
        assert_eq!(ring.len(), 4, "the basement is a plain rectangle: {ring:?}");
        assert_eq!(b.storey_ring(1).0.len(), fp.len(), "the ground storey keeps its bays");
        let built = build(&doc).unwrap();
        // One straight basement wall along the street; the bay walls start below the terrain on the ground storey.
        let basement: Vec<&BuiltWall> = built[0].walls.iter().filter(|w| w.id.starts_with("b1.basement")).collect();
        assert_eq!(basement.len(), 1);
        assert_eq!(basement[0].segments.len(), 1);
        let bay = built[0].walls.iter().find(|w| w.frame.origin == [24000.0, 12000.0]).unwrap();
        assert_eq!(bay.segments.first().map(|s| (s.level, s.z0)), Some((1, -PLINTH_FOOT)));
        // The basement floor stops at the straight wall.
        assert!(built[0].slabs[0].polygon.iter().all(|p| p[1] <= 12000.0));
        assert!(built[0].slabs[1].polygon.iter().any(|p| p[1] > 12000.0));
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
                    u: None,
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
    fn ordinary_pitched_gables_inherit_each_source_facade_finish_without_moving_geometry() {
        let mut doc = Document::default();
        let mut b = Building::new("b", "Synthetic gable finishes", &[[0., 0.], [24000., 0.], [24000., 10000.], [0., 10000.]], 2, 3400.).unwrap();
        b.roof = serde_json::from_value(serde_json::json!({"kind":"pitched","pitch_deg":40,"gables":["b.f2","b.f4"],
            "thickness_mm":200,"overhang_mm":180,"verge_mm":100,"gutters":true}))
        .unwrap();
        doc.buildings.push(b);
        let legacy = build(&doc).unwrap();
        let old_roof = legacy[0].roof.as_ref().unwrap();
        assert!(old_roof.gable_parts.is_empty(), "default Wall finish keeps the exact legacy display/export path");
        doc.buildings[0].facades[1].finish = Some(crate::doc::WallFinish::YellowBrick);
        doc.buildings[0].facades[3].finish = Some(crate::doc::WallFinish::Render);
        let source = serde_json::to_vec(&doc).unwrap();
        let built = build(&doc).unwrap();
        let r = built[0].roof.as_ref().unwrap();
        assert_eq!(r.id, old_roof.id);
        assert_eq!(r.cover, old_roof.cover);
        assert_eq!(r.gables, old_roof.gables);
        assert_eq!(r.gutters, old_roof.gutters);
        assert_eq!(r.ridge, old_roof.ridge);
        assert!(!r.compound);
        assert_eq!(source, serde_json::to_vec(&doc).unwrap());
        assert_eq!(r.gable_parts.len(), 2, "ordinary gables must carry their actual facade finishes");
        for (material, mesh) in &r.gable_parts {
            let x = match material {
                Material::BrickYellow => 24000.,
                Material::Render => 0.,
                _ => panic!("wrong gable finish {material:?}"),
            };
            assert!(mesh.positions.iter().all(|p| (p[0] - x).abs() < 1e-6));
        }
        let triangles: Vec<_> = r.gable_parts.iter().flat_map(|(_, m)| m.triangles.iter().map(|t| t.map(|i| m.positions[i as usize]))).collect();
        assert_eq!(triangles.len(), r.gables.triangles.len());
        for t in &r.gables.triangles {
            assert!(triangles.contains(&t.map(|i| r.gables.positions[i as usize])));
        }
    }

    fn roof_shell_is_closed_and_outward(mesh: &Mesh) {
        use std::collections::{BTreeMap, BTreeSet};
        let key = |p: [f64; 3]| p.map(|v| (v * 1e6).round() as i64);
        let mut faces = BTreeSet::new();
        let mut edges = BTreeMap::<_, (usize, i32)>::new();
        let mut volume = 0.;
        let origin = mesh.positions[0];
        for tri in &mesh.triangles {
            let points = tri.map(|i| mesh.positions[i as usize]);
            assert!(points.iter().flatten().all(|v| v.is_finite()));
            let mut face = points.map(key);
            face.sort();
            assert!(faces.insert(face), "duplicate face");
            for i in 0..3 {
                let (a, b) = (key(points[i]), key(points[(i + 1) % 3]));
                let e = edges.entry([a.min(b), a.max(b)]).or_default();
                e.0 += 1;
                e.1 += if a < b { 1 } else { -1 };
            }
            let [a, b, c] = points.map(|p| std::array::from_fn::<_, 3, _>(|i| p[i] - origin[i]));
            volume += (a[0] * (b[1] * c[2] - b[2] * c[1]) + a[1] * (b[2] * c[0] - b[0] * c[2]) + a[2] * (b[0] * c[1] - b[1] * c[0])) / 6.;
        }
        let bad: Vec<_> = edges.iter().filter(|(_, v)| **v != (2, 0)).take(8).collect();
        assert!(bad.is_empty(), "open/nonmanifold/misoriented roof assembly: {bad:?}");
        assert!(volume > 0., "roof solids must face outward: {volume}");
    }

    fn roof_vertical_hits(mesh: &Mesh, p: [f64; 3]) -> usize {
        mesh.triangles
            .iter()
            .filter(|t| {
                let [a, b, c] = t.map(|i| mesh.positions[i as usize]);
                let det = (b[0] - a[0]) * (c[1] - a[1]) - (b[1] - a[1]) * (c[0] - a[0]);
                if det.abs() < 1e-8 {
                    return false;
                }
                let s = ((p[0] - a[0]) * (c[1] - a[1]) - (p[1] - a[1]) * (c[0] - a[0])) / det;
                let t = ((b[0] - a[0]) * (p[1] - a[1]) - (b[1] - a[1]) * (p[0] - a[0])) / det;
                s >= -1e-8 && t >= -1e-8 && s + t <= 1. + 1e-8
            })
            .count()
    }

    #[test]
    fn selected_source_eaves_remove_chord_skew_without_rotating_the_dormer_or_moving_walls() {
        let source = [[0., 0.], [16000., 0.], [16000., 800.], [24000., 800.], [24000., 12800.], [28000., 12800.], [28000., 30000.], [0., 30000.]];
        for (shear, angle, offset) in [(0., 0_f64, [0., 0.]), (0.17, 37.25, [150000., -270000.]), (-0.12, -118., [-30000., 41000.])] {
            let (sin, cos) = angle.to_radians().sin_cos();
            let transform = |[x, y]: [f64; 2]| {
                let x = x + shear * y;
                [offset[0] + x * cos - y * sin, offset[1] + x * sin + y * cos]
            };
            let mut doc = Document::default();
            let mut b = Building::new("b", "Synthetic notched source eaves", &source.map(transform), 3, 3400.).unwrap();
            assert_eq!(b.footprint.len(), 8);
            let original = b.clone();
            b.roof = serde_json::from_value(serde_json::json!({"kind":"pitched","pitch_deg":46,"straighten_mm":10000,
                "eave_facades":["b.f1","b.f4","b.f5","b.f6","b.f7","b.f8"],"thickness_mm":220,"overhang_mm":250,
                "dormers":[{"id":"source-host","facade":"b.f4","u_mm":6000,"window_type":"window.casement.2",
                    "width_mm":1100,"height_mm":1100,"setback_mm":350}]}))
            .unwrap();
            doc.buildings.push(b);
            let before = serde_json::to_vec(&doc).unwrap();
            let built = build(&doc).expect("the original facade direction must be a level zero-yaw host, without chording across the recess");
            let roof = built[0].roof.as_ref().unwrap();
            assert_eq!(roof.dormers.len(), 1);
            assert_eq!(roof.id, "b.roof");
            assert_eq!(doc.buildings[0].roof.dormers()[0].yaw_deg, 0.);
            assert_eq!(before, serde_json::to_vec(&doc).unwrap());
            assert_eq!(doc.buildings[0].footprint, original.footprint);
            assert_eq!(doc.buildings[0].facades, original.facades);
            // Every source wall and its stable ID remain independent of the eave proposal.
            let mut flat = doc.clone();
            flat.buildings[0].roof = crate::Roof::Flat;
            assert_eq!(built[0].walls, build(&flat).unwrap()[0].walls);
            let fr = FacadeFrame::new(transform([24000., 800.]), transform([24000., 12800.])).unwrap();
            assert!(roof.dormers[0].front.frame.t.iter().zip(fr.t).all(|(a, b)| (a - b).abs() < 1e-9));
            for mesh in [&roof.cover, &roof.dormers[0].front.mesh, &roof.dormers[0].cheeks, &roof.dormers[0].roof] {
                roof_shell_is_closed_and_outward(mesh);
            }
            let mut uncut = doc.clone();
            uncut.buildings[0].roof.dormers_mut().unwrap().clear();
            let host = build(&uncut).unwrap();
            let host = &host[0].roof.as_ref().unwrap().cover;
            for u in [5451., 6000., 6549.] {
                let q = fr.point(u, 550., 0.);
                assert!(roof_vertical_hits(host, q) >= 2, "upper and lower host planes must exist before the cut");
                assert_eq!(roof_vertical_hits(&roof.cover, q), 0, "the selected host must have a real through-void");
            }
            assert!(roof_vertical_hits(&roof.cover, fr.point(6551., 550., 0.)) >= 2, "the cut cannot remove the cheek seat");
            let reopened: Document = serde_json::from_slice(&serde_json::to_vec(&doc).unwrap()).unwrap();
            assert_eq!(built, build(&reopened).unwrap());
        }
    }

    #[test]
    fn selected_eaves_preserve_gable_hosts_and_seat_source_cornices_above_connected_drainage() {
        let ring = [[0., 0.], [4000., 0.], [4000., 800.], [10000., 800.], [10000., 0.], [24000., 0.], [24000., 10000.], [0., 10000.]];
        let mut b = Building::new("b", "Synthetic selected bearings", &ring, 3, 3400.).unwrap();
        b.roof = serde_json::from_value(serde_json::json!({"kind":"pitched","pitch_deg":46,"eave_facades":["b.f1","b.f6","b.f7","b.f8"],
            "gables":["b.f6","b.f8"],"thickness_mm":220,"overhang_mm":180,"verge_mm":180,"gutters":true,"drainage":{}}))
        .unwrap();
        b.facades[0].bands.push(crate::doc::Band {
            levels: vec![2],
            bottom: 3220.,
            top: 3400.,
            projection: 135.,
            finish: Some(crate::doc::WallFinish::YellowBrick),
        });
        let mut doc = Document::default();
        doc.buildings.push(b);
        let source = serde_json::to_vec(&doc).unwrap();
        let built = build(&doc).unwrap();
        let roof = built[0].roof.as_ref().unwrap();
        assert_eq!(roof.drainage.iter().map(|p| p.id.as_str()).collect::<Vec<_>>(), vec!["b.roof.gutter0", "b.roof.gutter6"]);
        assert!(!roof.gables.is_empty());
        roof_shell_is_closed_and_outward(&roof.cover);
        let wall = &built[0].walls[0];
        assert_eq!(wall.trim_parts[0].0, Material::BrickYellow);
        for p in &wall.trim.positions {
            let outward = ((p[0] - wall.frame.origin[0]) * wall.frame.n[0] + (p[1] - wall.frame.origin[1]) * wall.frame.n[1]).max(0.);
            assert!(p[2] <= 10200. - outward * 46_f64.to_radians().tan() + 1e-6, "cap must clear the actual selected source-line soffit");
        }
        roof_shell_is_closed_and_outward(&wall.trim);
        assert_eq!(source, serde_json::to_vec(&doc).unwrap());
        for (key, value, detail) in [("gables", serde_json::json!(["b.f3"]), "absent"), ("gables", serde_json::json!(["b.f1"]), "endpoints")] {
            let mut bad = doc.clone();
            let mut roof = serde_json::to_value(&bad.buildings[0].roof).unwrap();
            roof[key] = value;
            bad.buildings[0].roof = serde_json::from_value(roof).unwrap();
            assert!(build(&bad).unwrap_err().to_string().contains(detail));
        }
        let mut bad = doc.clone();
        bad.buildings[0].facades[2].bands = bad.buildings[0].facades[0].bands.clone();
        let err = build(&bad).unwrap_err().to_string();
        assert!(err.contains("b.f3") && err.contains("eave_facades"), "{err}");
        for (bottom, projection) in [(3350., 135.), (3220., 200.)] {
            let mut bad = doc.clone();
            bad.buildings[0].facades[0].bands[0].bottom = bottom;
            bad.buildings[0].facades[0].bands[0].projection = projection;
            assert!(build(&bad).is_err(), "selected eaves retain cap depth/overhang guards");
        }
    }

    #[test]
    fn detailed_roof_cornice_seats_at_wall_top_without_crossing_the_soffit() {
        let mut doc = Document::default();
        let mut b = Building::new("b1", "Synthetic", &[[0., 0.], [12000., 0.], [12000., 8000.], [0., 8000.]], 1, 3000.).unwrap();
        b.roof = serde_json::from_value(serde_json::json!({"kind":"pitched","pitch_deg":46,"gutters":true,"overhang_mm":180,
            "thickness_mm":200,"drainage":{}}))
        .unwrap();
        for f in &mut b.facades {
            f.bands.push(crate::doc::Band {
                levels: vec![0],
                bottom: 2820.,
                top: 3000.,
                projection: 135.,
                finish: Some(crate::doc::WallFinish::YellowBrick),
            });
        }
        doc.buildings.push(b);
        let built = build(&doc).unwrap();
        let rise = 46_f64.to_radians().tan();
        for w in &built[0].walls {
            let outward = |p: &[f64; 3]| (p[0] - w.frame.origin[0]) * w.frame.n[0] + (p[1] - w.frame.origin[1]) * w.frame.n[1];
            assert!(
                w.trim.positions.iter().all(|p| p[2] <= 3000. - outward(p).max(0.0) * rise + 1e-6),
                "flat projecting caps intersect the sloping roof underside"
            );
            assert!(
                w.trim.positions.iter().any(|p| outward(p).abs() < 1e-6 && (p[2] - 3000.).abs() < 1e-6),
                "the cap must reach the bearing at the wall, with no exposed strip"
            );
            assert!(w.trim.positions.iter().any(|p| (outward(p) - 135.).abs() < 1e-6 && (p[2] - (3000. - 135. * rise)).abs() < 1e-6));
            assert_eq!(w.trim_parts.len(), 1);
            assert_eq!(w.trim_parts[0].0, Material::BrickYellow);
            assert_eq!(w.trim_parts[0].1, w.trim);
        }
        let clipped = build_cut(&doc, View::latest(&doc), Some(2950.)).unwrap();
        assert_eq!(clipped[0].walls[0].trim, built[0].walls[0].trim.below(2950.));
        assert_eq!(built, build(&doc).unwrap());
        let mut legacy = doc.clone();
        if let crate::Roof::Pitched { drainage, .. } = &mut legacy.buildings[0].roof {
            *drainage = None;
        }
        let old = build(&legacy).unwrap();
        assert_eq!(old[0].roof.as_ref().unwrap().cover, built[0].roof.as_ref().unwrap().cover);
        let flat_top = old[0].walls[0].trim.positions.iter().filter(|p| (p[2] - 3000.).abs() < 1e-6).count();
        assert!(flat_top > 4, "legacy None cornices keep their original flat caps");
        for (bottom, projection) in [(2950., 135.), (2820., 200.)] {
            let mut bad = doc.clone();
            bad.buildings[0].facades[0].bands[0].bottom = bottom;
            bad.buildings[0].facades[0].bands[0].projection = projection;
            assert!(build(&bad).is_err(), "a shallow cap or one extending past the overhang must be refused");
        }
    }

    #[test]
    fn facade_bands_preserve_declared_finish_and_legacy_defaults() {
        let mut doc = doc_with_block();
        let legacy = serde_json::json!({"levels":[0],"bottom":0.0,"top":2400.0,"projection":20.0});
        let band: crate::doc::Band = serde_json::from_value(legacy.clone()).unwrap();
        assert_eq!(serde_json::to_value(&band).unwrap(), legacy);
        let coloured = serde_json::json!({"levels":[0],"bottom":0.0,"top":2400.0,"projection":20.0,"finish":"render_terracotta"});
        let band: crate::doc::Band = serde_json::from_value(coloured.clone()).expect("band finish must be a typed optional source parameter");
        assert_eq!(serde_json::to_value(&band).unwrap(), coloured);
        doc.buildings[0].facades[0].bands.push(band);
        doc.buildings[0].facades[0].bands.push(
            serde_json::from_value(serde_json::json!({"levels":[1],"bottom":2700.0,"top":2800.0,"projection":40.0,"finish":"yellow_brick"})).unwrap(),
        );
        let built = build(&doc).unwrap();
        let wall = &built[0].walls[0];
        assert_eq!(wall.trim_parts.len(), 2);
        assert!(wall.trim_parts.iter().any(|(m, _)| *m == Material::RenderTerracotta));
        assert!(wall.trim_parts.iter().any(|(m, _)| *m == Material::BrickYellow));
        assert_eq!(wall.trim_parts.iter().map(|(_, m)| m.triangles.len()).sum::<usize>(), wall.trim.triangles.len());
        for band in &mut doc.buildings[0].facades[0].bands {
            band.finish = None;
        }
        let legacy = build(&doc).unwrap();
        assert_eq!(wall.trim, legacy[0].walls[0].trim, "colour cannot alter opening cuts or band geometry");
        assert!(legacy[0].walls[0].trim_parts.is_empty());
        doc.buildings[0].facades[0].bands[0].top = f64::NAN;
        assert!(build(&doc).is_err());
    }

    #[test]
    fn basement_windows_may_cross_terrain_but_cannot_start_below_their_floor() {
        let mut doc = doc_with_block();
        let b = &mut doc.buildings[0];
        b.set_storeys_from(3, 3000.0, 650.0).unwrap();
        b.facades[0].placements.clear();
        b.add_basement(-1560.0, "Basement").unwrap();
        b.facades[0].placements.push(Placement {
            u: None,
            level: 0,
            bay: 0,
            type_id: "window.fixed".into(),
            width: Some(900.0),
            height: Some(650.0),
            sill: Some(1350.0),
            spans_storeys: false,
            created: None,
            demolished: None,
        });
        let built = build(&doc).unwrap();
        assert_eq!(built[0].walls[0].openings[0].rect.v0, -210.0);
        assert_eq!(built[0].walls[0].openings[0].rect.v1, 440.0);
        doc.buildings[0].facades[0].placements[0].sill = Some(-1.0);
        assert!(build(&doc).unwrap_err().to_string().contains("below the basement floor"));
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
            u: None,
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
            u: None,
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
            u: None,
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
    fn independent_guard_styles_retain_nominal_width_overall_height_and_wall_clearance() {
        assert!(!crate::component::default_types().iter().any(|ty| matches!(ty.kind, ComponentKind::GuardRail { .. })));
        for railing in [Railing::Frederiksberg, Railing::Norrebro, Railing::Vesterbro, Railing::Osterbro] {
            for handrail in [false, true] {
                for (width, height, projection) in [(300.0, 900.0, if handrail { 35.0 } else { 20.0 }), (6000.0, 1500.0, 3000.0)] {
                    let mut doc = doc_with_block();
                    doc.buildings[0].facades[0].placements.clear();
                    let ty = ComponentType {
                        id: "guard".into(),
                        name: "Guard".into(),
                        width,
                        height,
                        sill: 200.0,
                        kind: ComponentKind::GuardRail { projection, railing, finish: Finish::White, handrail },
                    };
                    ty.validate().unwrap();
                    doc.types.push(ty.clone());
                    put(&mut doc, 1, 2, "guard", None, None);
                    let built = build(&doc).unwrap();
                    let wall = &built[0].walls[0];
                    assert!(wall.openings.is_empty());
                    assert_eq!(wall.attachments.len(), 1);
                    let guard = &wall.attachments[0];
                    assert_eq!(guard.class, ElementClass::Railing);
                    assert_eq!(guard.parts[0].0, Material::SteelWhite);
                    let mut combined = Mesh::default();
                    for (_, mesh) in &guard.parts {
                        combined.append(mesh);
                    }
                    let (lo, hi) = combined.bounds().unwrap();
                    let expected_front = -projection - if handrail { 35.0 } else { 20.0 };
                    assert!((hi[0] - lo[0] - width).abs() < 1e-8, "{railing:?}/{handrail}: width {lo:?} {hi:?}");
                    assert_eq!((lo[1], hi[1]), (expected_front, 0.0));
                    assert_eq!((lo[2], hi[2]), (3200.0, 3200.0 + height));
                    for projection in [f64::NAN, f64::INFINITY, f64::NEG_INFINITY, -1.0, 3001.0] {
                        let invalid =
                            ComponentType { kind: ComponentKind::GuardRail { projection, railing, finish: Finish::White, handrail }, ..ty.clone() };
                        assert!(invalid.validate().is_err());
                    }
                }
            }
        }
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
