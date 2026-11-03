//! Standalone, opt-in stair sequence geometry. **Not a native document source.**
//!
//! [`resolve_geometry`] validates only the authored route and its component solids.
//! It does not apply floor cuts or establish building clearance, retained floor support,
//! structural adequacy or safety. Existing Document/Stair/CLI consumers do not accept
//! this type. See `docs/stair-sequences.md` before using the result.
mod geometry;
mod params;
use crate::{Mesh, ModelError, Result, checked};
use geometry::{EPS, Work, area, at, boundary_gate, covers, cross, disjoint, dot, length, mid, near, only_gate, ring, sub};
pub use params::*;
use std::collections::{BTreeMap, BTreeSet};

pub const MAX_JSON_BYTES: usize = 2 * 1024 * 1024;
fn invalid(reason: impl std::fmt::Display) -> ModelError {
    ModelError::Invalid(format!("stair sequence: {reason}"))
}

/// Caller-supplied physical slab datum, in site-local millimetres. There is deliberately
/// no storey height. Attic means an existing physical attic slab, never a synthetic level.
/// The caller is responsible for deriving these from the actual building floor schedule.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct PhysicalFloor {
    pub floor: FloorRef,
    pub top_z_mm: f64,
    pub thickness_mm: f64,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SurfaceRole {
    OwnedLandingSlab,
    FloorOwnedSupportUnverified,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PartRole {
    Tread,
    LandingSlab,
}
#[derive(Debug, Clone, PartialEq)]
pub struct SolidPart {
    pub id: String,
    pub role: PartRole,
    pub outline: Vec<Point>,
    pub bottom_z_mm: f64,
    pub top_z_mm: f64,
    /// Gross component volume, not a building union or net material quantity.
    pub gross_volume_mm3: f64,
    pub mesh: Mesh,
}
#[derive(Debug, Clone, PartialEq)]
pub struct AccessDatum {
    pub level: usize,
    pub room_floor_top_z_mm: f64,
    pub offset_mm: f64,
    /// Nonzero local access offset; no implicit bridge to the room floor.
    pub requires_source_review: bool,
}
#[derive(Debug, Clone, PartialEq)]
pub struct ResolvedLanding {
    pub id: String,
    /// Authored patch, also retained when the physical floor owns its material.
    /// This is not proof that the patch is supported by retained floor material.
    pub outline: Vec<Point>,
    pub datum: PhysicalFloor,
    pub offset_mm: f64,
    pub top_z_mm: f64,
    pub access: Option<AccessDatum>,
    pub surface_role: SurfaceRole,
    /// Absent for floor-owned landings, so no duplicate slab mesh or volume exists.
    pub solid: Option<SolidPart>,
}
#[derive(Debug, Clone, PartialEq)]
pub struct ResolvedFlight {
    pub id: String,
    pub from_landing_id: String,
    pub to_landing_id: String,
    pub bottom_landing_z_mm: f64,
    pub top_landing_z_mm: f64,
    pub riser_count: usize,
    pub tread_count: usize,
    pub riser_height_mm: f64,
    pub treads: Vec<SolidPart>,
    pub gross_volume_mm3: f64,
}
#[derive(Debug, Clone, PartialEq)]
pub struct ProposedOpening {
    pub id: String,
    pub datum: PhysicalFloor,
    pub outline: Vec<Point>,
}
/// Geometry result explicitly carries the absence of integration checks. There is no
/// all-clear boolean. Non-null guards currently refuse, so success has absent guards.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Assessment {
    NotAssessed,
}
#[derive(Debug, Clone, PartialEq)]
pub struct ResolvedSequence {
    pub source_id: String,
    pub landings: Vec<ResolvedLanding>,
    pub flights: Vec<ResolvedFlight>,
    /// Validated 2D proposals only; never represented as applied floor holes.
    pub proposed_openings: Vec<ProposedOpening>,
    pub gross_volume_mm3: f64,
    pub building_clearance: Assessment,
    pub retained_floor_support: Assessment,
    pub guard_safety: Assessment,
}

fn unique<'a>(what: &str, values: impl Iterator<Item = &'a str>) -> Result<()> {
    let mut seen = BTreeSet::new();
    for id in values {
        if !seen.insert(id) {
            return Err(invalid(format!("duplicate {what} ID {id}")));
        }
    }
    Ok(())
}
fn datum(floors: &[PhysicalFloor], floor: FloorRef) -> Result<PhysicalFloor> {
    floors.iter().find(|f| f.floor == floor).copied().ok_or_else(|| invalid(format!("missing physical floor {floor:?}")))
}
fn port<'a>(l: &'a Landing, endpoint: &Endpoint) -> Result<&'a Port> {
    l.ports.iter().find(|p| p.id == endpoint.port).ok_or_else(|| invalid(format!("landing {}: missing port {}", l.id, endpoint.port)))
}
fn solid(id: String, role: PartRole, outline: Vec<Point>, top: f64, thickness: f64, work: &mut Work) -> Result<SolidPart> {
    let bottom = top - thickness;
    checked("solid bottom Z", bottom, -1e6, 1e6)?;
    checked("solid top Z", top, -1e6, 1e6)?;
    let mesh = geometry::prism(&outline, bottom, top, work)?;
    Ok(SolidPart { id, role, gross_volume_mm3: area(&outline) * thickness, outline, bottom_z_mm: bottom, top_z_mm: top, mesh })
}
struct Cell {
    id: String,
    outline: Vec<Point>,
    entry: Gate,
    exit: Gate,
}
fn gate(t: &Tread, idx: [usize; 2], entry: bool) -> Result<Gate> {
    let (Some(&a), Some(&b)) = (t.outline.get(idx[0]), t.outline.get(idx[1])) else {
        return Err(invalid(format!("tread {}: edge index out of range", t.id)));
    };
    let oriented = if entry { (idx[0] + 1) % t.outline.len() == idx[1] } else { (idx[1] + 1) % t.outline.len() == idx[0] };
    if !oriented {
        return Err(invalid(format!("tread {}: entry/exit must be oriented boundary edges", t.id)));
    }
    Ok([a, b])
}
fn same_gate(a: Gate, b: Gate) -> bool {
    near(a[0], b[0]) && near(a[1], b[1])
}
fn cells(f: &Flight, entry: Gate, exit: Gate, work: &mut Work) -> Result<Vec<Cell>> {
    let result = match &f.profile {
        Profile::Straight { tread_ids } => {
            let run = sub(exit[0], entry[0]);
            let across = sub(entry[1], entry[0]);
            checked("straight run", length(run), 10., 20000.)?;
            if !near(sub(exit[1], entry[1]), run) || (dot(run, across) / length(across)).abs() > EPS || cross(across, run) <= 0. {
                return Err(invalid(format!("flight {}: straight gates need equal parallel widths and perpendicular positive run", f.id)));
            }
            let n = tread_ids.len();
            tread_ids
                .iter()
                .enumerate()
                .map(|(i, id)| {
                    let a = if i == 0 { entry } else { [at(entry[0], exit[0], i as f64 / n as f64), at(entry[1], exit[1], i as f64 / n as f64)] };
                    let b = if i + 1 == n {
                        exit
                    } else {
                        [at(entry[0], exit[0], (i + 1) as f64 / n as f64), at(entry[1], exit[1], (i + 1) as f64 / n as f64)]
                    };
                    Cell { id: id.clone(), outline: vec![a[0], a[1], b[1], b[0]], entry: a, exit: b }
                })
                .collect::<Vec<_>>()
        }
        Profile::TreadChain { treads } => {
            let mut result = Vec::new();
            for t in treads {
                result.push(Cell {
                    id: t.id.clone(),
                    outline: t.outline.clone(),
                    entry: gate(t, t.entry_edge, true)?,
                    exit: gate(t, t.exit_edge, false)?,
                });
            }
            result
        }
    };
    unique("tread", result.iter().map(|c| c.id.as_str()))?;
    let mut previous = entry;
    for c in &result {
        ring(&c.outline, work)?;
        boundary_gate(&c.outline, c.entry, true, work)?;
        boundary_gate(&c.outline, c.exit, false, work)?;
        if near(mid(c.entry), mid(c.exit)) {
            return Err(invalid(format!("tread {}: distinct entry and exit gates required", c.id)));
        }
        if !same_gate(previous, c.entry) {
            return Err(invalid(format!("tread {}: disconnected full-width gate", c.id)));
        }
        if !covers(&c.outline, [mid(c.entry), mid(c.exit)], work)? {
            return Err(invalid(format!("tread {}: walking line leaves polygon", c.id)));
        }
        previous = c.exit;
    }
    if !same_gate(previous, exit) {
        return Err(invalid("last tread does not match destination gate"));
    }
    for (i, a) in result.iter().enumerate() {
        for (j, b) in result.iter().enumerate().skip(i + 1) {
            let contacts = disjoint(&a.outline, &b.outline, work)?;
            if j == i + 1 {
                only_gate(&contacts, a.exit, work)?;
            } else {
                for c in contacts {
                    let geometry::Contact::Point(p) = c else {
                        return Err(invalid("nonadjacent tread cells share an edge"));
                    };
                    // A genuine fan pivot is the same zero-length side through EVERY intervening
                    // tread, not a general exemption for coincident endpoints.
                    let pivot = (0..2).any(|side| result.iter().skip(i).take(j - i + 1).all(|t| near(t.entry[side], p) && near(t.exit[side], p)));
                    if !pivot {
                        return Err(invalid("nonadjacent tread contact is not a shared inner pivot"));
                    }
                }
            }
        }
    }
    Ok(result)
}

/// Resolve authored route geometry against explicit physical floor tops. No Building is
/// accepted, so storey heights cannot accidentally replace independent landing offsets.
/// This does NOT validate floor subtraction, slab crossings, support or building clashes.
pub fn resolve_geometry(source: &SequenceV1, floors: &[PhysicalFloor]) -> Result<ResolvedSequence> {
    // Validate identifier length before adding it to an error context.
    source.check_bounds()?;
    resolve(source, floors).map_err(|e| invalid(format!("source {}: {e}", source.id)))
}
fn resolve(s: &SequenceV1, floors: &[PhysicalFloor]) -> Result<ResolvedSequence> {
    params::count("physical floor datums", floors.len(), 1, 61)?;
    let mut floor_ids = BTreeSet::new();
    for f in floors {
        params::floor_bounds(f.floor)?;
        if !floor_ids.insert(f.floor) {
            return Err(invalid("duplicate physical floor datum"));
        }
        checked("physical floor top", f.top_z_mm, -1e6, 1e6)?;
        checked("physical floor thickness", f.thickness_mm, 1., 2000.)?;
        checked("physical floor bottom", f.top_z_mm - f.thickness_mm, -1e6, 1e6)?;
    }
    if s.landings.len() != s.flights.len() + 1 {
        return Err(invalid("path needs one more landing than flights"));
    }
    unique("landing", s.landings.iter().map(|l| l.id.as_str()))?;
    unique("flight", s.flights.iter().map(|f| f.id.as_str()))?;
    unique("opening", s.openings.iter().map(|o| o.id.as_str()))?;
    if s.landings.iter().any(|l| !l.guards.is_empty()) || s.flights.iter().any(|f| f.guards.left.is_some() || f.guards.right.is_some()) {
        return Err(invalid("non-null guards are unsupported by the geometry foundation"));
    }
    let mut work = Work::new();
    let mut landings = Vec::new();
    let mut access_levels = BTreeSet::new();
    for (i, l) in s.landings.iter().enumerate() {
        ring(&l.outline, &mut work).map_err(|e| invalid(format!("landing {}: {e}", l.id)))?;
        unique("port", l.ports.iter().map(|p| p.id.as_str()))?;
        let incoming = if i > 0 { s.flights.get(i - 1) } else { None };
        let outgoing = s.flights.get(i);
        if l.ports.len() != usize::from(incoming.is_some()) + usize::from(outgoing.is_some()) {
            return Err(invalid(format!("landing {}: unused or missing port", l.id)));
        }
        let mut walk = Vec::new();
        let mut used = BTreeSet::new();
        if let Some(f) = incoming {
            if f.to.landing != l.id {
                return Err(invalid(format!("flight {}: destination violates path ordering", f.id)));
            }
            let p = port(l, &f.to)?;
            used.insert(&p.id);
            boundary_gate(&l.outline, p.edge, true, &mut work)?;
            walk.push(mid(p.edge));
        }
        walk.extend_from_slice(&l.walk_via);
        if let Some(f) = outgoing {
            if f.from.landing != l.id {
                return Err(invalid(format!("flight {}: departure violates path ordering", f.id)));
            }
            let p = port(l, &f.from)?;
            if !used.insert(&p.id) {
                return Err(invalid(format!("landing {}: port used twice", l.id)));
            }
            boundary_gate(&l.outline, p.edge, false, &mut work)?;
            walk.push(mid(p.edge));
        }
        if let [a, b] = l.ports.as_slice()
            && matches!(geometry::contact(a.edge, b.edge, &mut work)?, Some(geometry::Contact::Segment(_)))
        {
            return Err(invalid("landing ports overlap"));
        }
        for &p in &walk {
            if !covers(&l.outline, [p, p], &mut work)? {
                return Err(invalid(format!("landing {}: walk_via outside polygon", l.id)));
            }
        }
        for p in walk.windows(2) {
            if !covers(&l.outline, [p[0], p[1]], &mut work)? {
                return Err(invalid(format!("landing {}: walking line leaves polygon", l.id)));
            }
        }
        let d = datum(floors, l.elevation.floor)?;
        let z = checked("landing resolved Z", d.top_z_mm + l.elevation.offset_mm, -1e6, 1e6)?;
        let id = format!("{}.landing.{}", s.id, l.id);
        let access = if let Some(level) = l.access_level {
            if l.elevation.floor != (FloorRef::Storey { level }) {
                return Err(invalid(format!("landing {}: access must match storey datum", l.id)));
            }
            if !access_levels.insert(level) {
                return Err(invalid("duplicate access level"));
            }
            Some(AccessDatum {
                level,
                room_floor_top_z_mm: d.top_z_mm,
                offset_mm: l.elevation.offset_mm,
                requires_source_review: l.elevation.offset_mm != 0.,
            })
        } else {
            None
        };
        let (surface_role, body) = match l.surface {
            Surface::Slab { thickness_mm } => {
                (SurfaceRole::OwnedLandingSlab, Some(solid(id.clone(), PartRole::LandingSlab, l.outline.clone(), z, thickness_mm, &mut work)?))
            }
            Surface::Floor { floor } => {
                if floor != l.elevation.floor || l.elevation.offset_mm != 0. {
                    return Err(invalid(format!("landing {}: floor-owned surface requires same floor and zero offset", l.id)));
                }
                (SurfaceRole::FloorOwnedSupportUnverified, None)
            }
        };
        landings.push(ResolvedLanding {
            id,
            outline: l.outline.clone(),
            datum: d,
            offset_mm: l.elevation.offset_mm,
            top_z_mm: z,
            access,
            surface_role,
            solid: body,
        });
    }
    let (Some(first), Some(last)) = (landings.first(), landings.last()) else {
        return Err(invalid("missing path endpoints"));
    };
    checked("total path rise", last.top_z_mm - first.top_z_mm, 40., 100000.)?;
    let mut flights = Vec::new();
    for (i, f) in s.flights.iter().enumerate() {
        let (Some(a), Some(b), Some(ra), Some(rb)) = (s.landings.get(i), s.landings.get(i + 1), landings.get(i), landings.get(i + 1)) else {
            return Err(invalid("missing flight endpoints"));
        };
        let rise = checked("flight total rise", rb.top_z_mm - ra.top_z_mm, 40., 20000.)?;
        let riser_count = f.profile.tread_count() + 1;
        let h = checked("flight riser height", rise / riser_count as f64, 20., 400.)?;
        let arrival_thickness = match b.surface {
            Surface::Slab { thickness_mm } => thickness_mm,
            Surface::Floor { .. } => rb.datum.thickness_mm,
        };
        if arrival_thickness < h + 1. {
            return Err(invalid(format!("landing {}: arrival thickness must exceed riser by at least 1 mm", b.id)));
        }
        let entry = port(a, &f.from)?.edge;
        let exit = port(b, &f.to)?.edge;
        let cells = cells(f, entry, exit, &mut work).map_err(|e| invalid(format!("flight {}: {e}", f.id)))?;
        if let (Some(first), Some(last)) = (cells.first(), cells.last()) {
            only_gate(&disjoint(&a.outline, &first.outline, &mut work)?, entry, &mut work)?;
            only_gate(&disjoint(&b.outline, &last.outline, &mut work)?, exit, &mut work)?;
        }
        let id = format!("{}.flight.{}", s.id, f.id);
        let mut treads = Vec::new();
        for (j, c) in cells.into_iter().enumerate() {
            treads.push(solid(
                format!("{id}.tread.{}", c.id),
                PartRole::Tread,
                c.outline,
                ra.top_z_mm + (j + 1) as f64 * h,
                h + f.soffit_mm,
                &mut work,
            )?);
        }
        flights.push(ResolvedFlight {
            id,
            from_landing_id: ra.id.clone(),
            to_landing_id: rb.id.clone(),
            bottom_landing_z_mm: ra.top_z_mm,
            top_landing_z_mm: rb.top_z_mm,
            riser_count,
            tread_count: treads.len(),
            riser_height_mm: h,
            gross_volume_mm3: treads.iter().map(|t| t.gross_volume_mm3).sum(),
            treads,
        });
    }
    // Actual owned material must not overlap in 3D. Different flights may legitimately
    // overlap in plan at disjoint heights; same-flight topology was checked above.
    let bodies: Vec<_> = landings.iter().filter_map(|l| l.solid.as_ref()).chain(flights.iter().flat_map(|f| &f.treads)).collect();
    let mut triangles = 0;
    for (i, a) in bodies.iter().enumerate() {
        triangles += a.mesh.triangles.len();
        for b in bodies.iter().skip(i + 1) {
            work.take(1)?;
            if a.top_z_mm.min(b.top_z_mm) - a.bottom_z_mm.max(b.bottom_z_mm) > EPS {
                disjoint(&a.outline, &b.outline, &mut work).map_err(|e| invalid(format!("parts {} and {}: {e}", a.id, b.id)))?;
            }
        }
    }
    params::count("result triangles", triangles, 0, 250000)?;
    let mut proposed_openings = Vec::new();
    let mut per_floor = BTreeMap::new();
    for (i, o) in s.openings.iter().enumerate() {
        let d = datum(floors, o.floor)?;
        let n = per_floor.entry(o.floor).or_insert(0);
        *n += 1;
        params::count("opening rings per floor", *n, 1, 4)?;
        ring(&o.outline, &mut work).map_err(|e| invalid(format!("opening {}: {e}", o.id)))?;
        for other in s.openings.iter().take(i).filter(|p| p.floor == o.floor) {
            if !disjoint(&o.outline, &other.outline, &mut work)?.is_empty() {
                return Err(invalid("touching opening rings require later floor union integration"));
            }
        }
        proposed_openings.push(ProposedOpening { id: format!("{}.opening.{}", s.id, o.id), datum: d, outline: o.outline.clone() });
    }
    let gross_volume_mm3 = bodies.iter().map(|p| p.gross_volume_mm3).sum();
    Ok(ResolvedSequence {
        source_id: s.id.clone(),
        landings,
        flights,
        proposed_openings,
        gross_volume_mm3,
        building_clearance: Assessment::NotAssessed,
        retained_floor_support: Assessment::NotAssessed,
        guard_safety: Assessment::NotAssessed,
    })
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FlightCountChange {
    pub flight_id: String,
    /// Zero for an added/removed flight.
    pub old_treads: usize,
    pub new_treads: usize,
    pub old_risers: usize,
    pub new_risers: usize,
}
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct IdentityDelta {
    pub added_ids: Vec<String>,
    pub removed_ids: Vec<String>,
    pub count_changes: Vec<FlightCountChange>,
}
impl ResolvedSequence {
    /// Authored order, never sorted by ID or elevation. Parent flight IDs are metadata;
    /// only their tread parts carry meshes/volume. Includes floor-owned landing IDs.
    pub fn part_ids(&self) -> Vec<String> {
        self.flights
            .iter()
            .flat_map(|f| std::iter::once(f.id.clone()).chain(f.treads.iter().map(|t| t.id.clone())))
            .chain(self.landings.iter().map(|l| l.id.clone()))
            .chain(self.proposed_openings.iter().map(|o| o.id.clone()))
            .collect()
    }
    /// Explicit identity/count delta for a future caller staging replacement. This does
    /// not update saved sources, group references or any native consumer.
    pub fn identity_delta(&self, previous: &Self) -> Result<IdentityDelta> {
        if self.source_id != previous.source_id {
            return Err(invalid("replacement must preserve source ID"));
        }
        let old = previous.part_ids();
        let new = self.part_ids();
        let old_set: BTreeSet<_> = old.iter().collect();
        let new_set: BTreeSet<_> = new.iter().collect();
        let added_ids = new.iter().filter(|i| !old_set.contains(i)).cloned().collect();
        let removed_ids = old.iter().filter(|i| !new_set.contains(i)).cloned().collect();
        let mut count_changes = Vec::new();
        for f in &self.flights {
            let old = previous.flights.iter().find(|p| p.id == f.id);
            let (old_treads, old_risers) = old.map_or((0, 0), |p| (p.tread_count, p.riser_count));
            if old_treads != f.tread_count {
                count_changes.push(FlightCountChange {
                    flight_id: f.id.clone(),
                    old_treads,
                    new_treads: f.tread_count,
                    old_risers,
                    new_risers: f.riser_count,
                });
            }
        }
        for f in previous.flights.iter().filter(|f| !self.flights.iter().any(|n| n.id == f.id)) {
            count_changes.push(FlightCountChange {
                flight_id: f.id.clone(),
                old_treads: f.tread_count,
                new_treads: 0,
                old_risers: f.riser_count,
                new_risers: 0,
            });
        }
        Ok(IdentityDelta { added_ids, removed_ids, count_changes })
    }
}
#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
mod tests;
