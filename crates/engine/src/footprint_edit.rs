//! Same-topology source editing. Geometry remains exclusively in trigon-model.
use std::collections::{BTreeMap, BTreeSet};

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use trigon_model::{Building, Document, FacadeFrame, ModelError, Roof, checked};

use crate::{Result, Session};

#[derive(Clone, Copy, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub(crate) enum AnchorPolicy {
    /// Preserve source-facade distances; objects follow their host.
    KeepFacadeU,
    /// Project previous wall-line centres onto the new tangent. Perpendicular world
    /// position is NOT preserved. Run/setback, yaw and offsets remain host-relative.
    ProjectWallLineCentres,
}

#[derive(Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub(crate) struct CornerMove {
    /// Zero-based source index from building.outline; never a facade number.
    pub corner: usize,
    pub point_mm: [f64; 2],
}

#[derive(Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub(crate) struct SetFootprint {
    pub building: String,
    /// Complete exact current open CCW source ring. A stale value refuses the edit.
    pub expected_footprint: Vec<[f64; 2]>,
    /// Distinct existing indices. No inserting, deleting or reordering corners.
    pub moves: Vec<CornerMove>,
    pub anchor_policy: AnchorPolicy,
}

fn invalid(message: impl Into<String>) -> crate::EngineError {
    ModelError::Invalid(message.into()).into()
}
fn distance(a: [f64; 2], b: [f64; 2]) -> f64 {
    (a[0] - b[0]).hypot(a[1] - b[1])
}
fn dot(a: [f64; 2], b: [f64; 2]) -> f64 {
    a[0] * b[0] + a[1] * b[1]
}
fn frame(b: &Building, edge: usize) -> Result<FacadeFrame> {
    let (a, z) = b.edge(edge).ok_or_else(|| invalid(format!("{} has no footprint edge {edge}", b.id)))?;
    FacadeFrame::new(a, z).ok_or_else(|| invalid(format!("{} edge {edge} length {} mm must exceed 1 mm", b.id, distance(a, z))))
}
fn ring(points: &[[f64; 2]]) -> Result<()> {
    let n = points.len();
    if !(3..=2000).contains(&n) {
        return Err(invalid(format!("footprint needs 3..2000 corners, got {n}")));
    }
    for (i, p) in points.iter().enumerate() {
        for &v in p {
            checked(&format!("corner {i} coordinate mm"), v, -1e8, 1e8)?;
        }
    }
    for (i, (&a, &b)) in points.iter().zip(points.iter().cycle().skip(1)).take(n).enumerate() {
        checked(&format!("footprint edge {i} length mm"), distance(a, b), 1.001, 200000.)?;
        for (j, (&c, &d)) in points.iter().zip(points.iter().cycle().skip(1)).take(n).enumerate().skip(i + 2) {
            if i == 0 && j == n - 1 {
                continue;
            }
            if crate::local_building::intersects(a, b, c, d) {
                return Err(invalid(format!("footprint edges {i} and {j} cross or touch (clearance 0 mm)")));
            }
        }
        let c = points.get((i + 2) % n).copied().ok_or_else(|| invalid("missing next footprint corner"))?;
        let ab = [b[0] - a[0], b[1] - a[1]];
        let bc = [c[0] - b[0], c[1] - b[1]];
        if (ab[0] * bc[1] - ab[1] * bc[0]).abs() < 1e-6 && dot(ab, bc) <= 0. {
            return Err(invalid(format!("footprint edge {i} reverses at its next corner (adjacent clearance 0 mm)")));
        }
    }
    checked("CCW footprint signed area mm²", trigon_model::footprint::signed_area2(points) / 2., 4e6, 4e16)?;
    Ok(())
}

#[derive(Clone, Copy)]
struct Host {
    before: FacadeFrame,
    after: FacadeFrame,
}
impl Host {
    fn along(self, u: f64, policy: AnchorPolicy) -> f64 {
        if policy == AnchorPolicy::KeepFacadeU || self.before == self.after {
            return u;
        }
        dot([self.before.origin[0] - self.after.origin[0], self.before.origin[1] - self.after.origin[1]], self.after.t)
            + u * dot(self.before.t, self.after.t)
    }
    fn angle(self) -> f64 {
        (self.before.t[0] * self.after.t[1] - self.before.t[1] * self.after.t[0]).atan2(dot(self.before.t, self.after.t)).to_degrees()
    }
    fn record(self, id: &str, field: &str, u: f64, v: f64, out: &mut Vec<Value>) -> Result<()> {
        if out.len() >= 4096 {
            return Err(invalid("footprint feedback exceeds 4096 anchor records; split the edit"));
        }
        let p = self.before.point(u, 0., 0.);
        let q = self.after.point(v, 0., 0.);
        out.push(json!({"id":id,"field":field,"before_u_mm":u,"after_u_mm":v,"delta_u_mm":v-u,
            "before_wall_line_mm":[p[0],p[1]],"after_wall_line_mm":[q[0],q[1]],
            "site_delta_mm":[q[0]-p[0],q[1]-p[1]],"normal_displacement_mm":dot([q[0]-p[0],q[1]-p[1]],self.after.n)}));
        Ok(())
    }
    fn edit(self, id: &str, field: &str, u: &mut f64, policy: AnchorPolicy, out: &mut Vec<Value>) -> Result<()> {
        let old = checked(&format!("{id} {field} before mm"), *u, 0., self.before.length)?;
        let new = checked(&format!("{id} {field} projected mm"), self.along(old, policy), 0., self.after.length)?;
        self.record(id, field, old, new, out)?;
        *u = new;
        Ok(())
    }
}

fn retained_edges(before: &Building, after: &Building) -> Result<()> {
    for level in 0..before.storeys.len() {
        let a = before.storey_ring(level).1;
        let b = after.storey_ring(level).1;
        if a != b {
            return Err(invalid(format!(
                "{} level {level}: basement straightening at {} mm changes retained source corners {a:?} -> {b:?}; affected facades {:?}",
                before.id,
                before.basement_straighten_mm.unwrap_or(0.),
                before.facades.iter().filter(|f| a.contains(&f.edge) != b.contains(&f.edge)).map(|f| &f.id).collect::<Vec<_>>()
            )));
        }
    }
    let tolerance = match before.roof.base() {
        Roof::Pitched { straighten_mm, eave_facades: None, .. } | Roof::HalfHipped { straighten_mm, eave_facades: None, .. } => Some(*straighten_mm),
        Roof::Mansard { parameters } => Some(parameters.straighten_mm),
        _ => None,
    };
    if let Some(tolerance) = tolerance {
        let a = trigon_model::roof::eave_outline(&before.footprint, tolerance);
        let b = trigon_model::roof::eave_outline(&after.footprint, tolerance);
        if a != b {
            return Err(invalid(format!(
                "{}.roof: straightening at {tolerance} mm (50 mm collinearity) changes source corners {a:?} -> {b:?}; affected facades {:?}",
                before.id,
                before.facades.iter().filter(|f| a.contains(&f.edge) != b.contains(&f.edge)).map(|f| &f.id).collect::<Vec<_>>()
            )));
        }
    }
    Ok(())
}

fn segment_distance(p: [f64; 2], a: [f64; 2], b: [f64; 2]) -> f64 {
    let d = [b[0] - a[0], b[1] - a[1]];
    let t = (dot([p[0] - a[0], p[1] - a[1]], d) / dot(d, d).max(1e-12)).clamp(0., 1.);
    distance(p, [a[0] + t * d[0], a[1] + t * d[1]])
}
fn contains(ring: &[[f64; 2]], p: [f64; 2]) -> (bool, f64) {
    let mut inside = false;
    let mut clearance = f64::INFINITY;
    for (&a, &b) in ring.iter().zip(ring.iter().cycle().skip(1)).take(ring.len()) {
        clearance = clearance.min(segment_distance(p, a, b));
        if (a[1] > p[1]) != (b[1] > p[1]) && p[0] < (b[0] - a[0]) * (p[1] - a[1]) / (b[1] - a[1]) + a[0] {
            inside = !inside;
        }
    }
    (inside || clearance <= 1e-6, clearance)
}
fn fixed_shape(boundary: &[[f64; 2]], host: &str, id: &str, points: &[[f64; 2]], closed: bool) -> Result<()> {
    if points.len() > 2000 {
        return Err(invalid(format!("{id}: at most 2000 site-fixed source points can be checked")));
    }
    for (i, &p) in points.iter().enumerate() {
        for v in p {
            checked(&format!("{id} point {i} coordinate mm"), v, -1e8, 1e8)?;
        }
        let (inside, distance) = contains(boundary, p);
        if !inside {
            return Err(invalid(format!(
                "{id} site-fixed point {i} {p:?} lies {distance:.9} mm outside {}; moving interior sources requires a separate reviewed edit",
                host
            )));
        }
    }
    let cross = |a: [f64; 2], b: [f64; 2], p: [f64; 2]| (b[0] - a[0]) * (p[1] - a[1]) - (b[1] - a[1]) * (p[0] - a[0]);
    let count = if closed { points.len() } else { points.len().saturating_sub(1) };
    let mut probes = 0;
    for (&a, &b1) in points.iter().zip(points.iter().cycle().skip(1)).take(count) {
        let direction = [b1[0] - a[0], b1[1] - a[1]];
        let mut stations = vec![0., 1.];
        for (edge, (&c, &d)) in boundary.iter().zip(boundary.iter().cycle().skip(1)).take(boundary.len()).enumerate() {
            if cross(a, b1, c) * cross(a, b1, d) < -1e-6 && cross(c, d, a) * cross(c, d, b1) < -1e-6 {
                return Err(invalid(format!(
                    "{id} crosses {} source edge {edge} (clearance 0 mm); site-fixed geometry cannot be clipped implicitly",
                    host
                )));
            }
            // Proper intersections alone miss exits/reentries exactly at concave
            // vertices. Partition there and test each open interval, including
            // segments whose own endpoints lie on the boundary.
            if segment_distance(c, a, b1) <= 1e-6 {
                stations.push((dot([c[0] - a[0], c[1] - a[1]], direction) / dot(direction, direction).max(1e-12)).clamp(0., 1.));
            }
        }
        stations.sort_by(f64::total_cmp);
        stations.dedup();
        for pair in stations.windows(2) {
            probes += 1;
            if probes > 4096 {
                return Err(invalid(format!("{id} containment against {host} exceeds 4096 edge intervals")));
            }
            let t = (pair[0] + pair[1]) / 2.;
            let point = [a[0] + t * direction[0], a[1] + t * direction[1]];
            let (inside, distance) = contains(boundary, point);
            if !inside {
                return Err(invalid(format!(
                    "{id} site-fixed edge at {point:?} lies {distance:.9} mm outside {host}; implicit clipping is unsupported"
                )));
            }
        }
    }
    Ok(())
}
fn fixed_sources(b: &Building) -> Result<()> {
    for (i, w) in b.interior_walls.iter().enumerate() {
        w.validate(b.storeys.len())?;
        let id = if w.id.is_empty() { format!("{}.iw{i}", b.id) } else { w.id.clone() };
        fixed_shape(&b.footprint, &b.id, &id, &[w.a, w.b], false)?;
    }
    for (i, r) in b.rooms.iter().enumerate() {
        let id = if r.id.is_empty() { format!("{}.room{i}", b.id) } else { r.id.clone() };
        if r.levels.is_empty() || r.levels.iter().any(|&l| l >= b.storeys.len()) {
            return Err(invalid(format!("{id} requires existing storey indices below {}", b.storeys.len())));
        }
        crate::elements::polygon(&format!("{id} polygon"), &json!(r.polygon), None)?;
        fixed_shape(&b.footprint, &b.id, &id, &r.polygon, true)?;
    }
    for (i, st) in b.stairs.iter().enumerate() {
        let id = if st.id.is_empty() { format!("{}.stair{i}", b.id) } else { st.id.clone() };
        if st.levels.is_empty() || st.levels.iter().any(|&l| l >= b.storeys.len()) {
            return Err(invalid(format!("{id} requires existing storey indices below {}", b.storeys.len())));
        }
        fixed_shape(&b.footprint, &b.id, &id, &trigon_model::stair::opening(st)?, true)?;
    }
    Ok(())
}

fn inventory(b: &trigon_model::Built) -> Vec<String> {
    let mut ids = Vec::new();
    for w in b.exterior_walls() {
        ids.push(format!("wall:{}", w.id));
        ids.extend(w.segments.iter().map(|s| format!("segment:{}", s.id)));
        for o in &w.openings {
            ids.push(format!("opening:{}", o.id));
            ids.push(format!("fill:{}", o.fill.id));
            ids.extend(o.attached.iter().map(|a| format!("attached:{}", a.id)));
        }
        ids.extend(w.attachments.iter().map(|a| format!("attachment:{}", a.id)));
    }
    ids.extend(b.slabs.iter().map(|x| format!("slab:{}", x.id)));
    ids.extend(b.stairs.iter().map(|x| format!("stair:{}", x.id)));
    for w in &b.interior {
        ids.push(format!("interior:{}", w.id));
        for d in &w.doors {
            ids.push(format!("door:{}", d.id));
            ids.push(format!("fill:{}", d.fill.id));
        }
    }
    if let Some(r) = &b.roof {
        ids.push(format!("roof:{}", r.id));
        ids.extend(r.drainage.iter().map(|x| format!("drain:{}", x.id)));
        ids.extend(r.dormers.iter().map(|x| format!("dormer:{}", x.id)));
        for x in &r.rooflights {
            ids.push(format!("rooflight:{}", x.id));
            ids.push(format!("opening:{}", x.opening_id));
        }
    }
    ids
}
fn built_guard(a: &trigon_model::Built, b: &trigon_model::Built) -> Result<()> {
    let old = inventory(a);
    let new = inventory(b);
    if old != new {
        let removed: Vec<_> = old.iter().filter(|id| !new.contains(id)).collect();
        let added: Vec<_> = new.iter().filter(|id| !old.contains(id)).collect();
        return Err(invalid(format!(
            "{} built identities/order changed ({} -> {} objects), removed {removed:?}, added {added:?}; source pruning is unsupported",
            a.building_id,
            old.len(),
            new.len()
        )));
    }
    for (a, b) in a.interior.iter().zip(&b.interior) {
        if a != b {
            let delta = a.axis.iter().zip(b.axis).map(|(&p, q)| distance(p, q)).fold(0., f64::max);
            let outline = a.outline.iter().zip(b.outline).map(|(&p, q)| distance(p, q)).fold(0., f64::max);
            return Err(invalid(format!(
                "{} automatic interior join/door geometry changes: endpoint displacement {delta:.9} mm, outline displacement {outline:.9} mm; site-fixed interiors require a separate reviewed edit",
                a.id
            )));
        }
    }
    Ok(())
}

pub(crate) fn set(s: &mut Session, p: SetFootprint) -> Result<Value> {
    ring(&p.expected_footprint)?;
    let mut matches = s.doc.buildings.iter().enumerate().filter(|(_, b)| b.id == p.building);
    let (index, before) = matches.next().ok_or_else(|| invalid(format!("unknown building {}", p.building)))?;
    if matches.next().is_some() {
        return Err(invalid(format!("ambiguous building {}", p.building)));
    }
    if before.footprint != p.expected_footprint {
        return Err(invalid(format!(
            "{} stale expected_footprint: expected {} corners, current {}; read building.outline again",
            p.building,
            p.expected_footprint.len(),
            before.footprint.len()
        )));
    }
    if p.moves.is_empty() || p.moves.len() > before.footprint.len() {
        return Err(invalid(format!("give 1..{} distinct existing corner moves", before.footprint.len())));
    }
    let mut after = before.clone();
    let mut seen = BTreeSet::new();
    let mut corners = Vec::new();
    for m in &p.moves {
        if !seen.insert(m.corner) {
            return Err(invalid(format!("duplicate corner {}", m.corner)));
        }
        let old = *before.footprint.get(m.corner).ok_or_else(|| invalid(format!("corner {} outside 0..{}", m.corner, before.footprint.len())))?;
        for v in m.point_mm {
            checked(&format!("corner {} coordinate mm", m.corner), v, -1e8, 1e8)?;
        }
        let n = before.footprint.len();
        let prev = (m.corner + n - 1) % n;
        let limit = (0.49 * frame(before, prev)?.length.min(frame(before, m.corner)?.length)).min(10000.);
        let movement = distance(old, m.point_mm);
        checked(&format!("corner {} displacement mm (same-topology registration bound)", m.corner), movement, 1e-9, limit)?;
        let slot = after.footprint.get_mut(m.corner).ok_or_else(|| invalid("missing source corner"))?;
        *slot = m.point_mm;
        corners.push(json!({"corner":m.corner,"before_mm":old,"after_mm":m.point_mm,"delta_mm":[m.point_mm[0]-old[0],m.point_mm[1]-old[1]],"distance_mm":movement}));
    }
    ring(&after.footprint)?;
    let mut edges = BTreeSet::new();
    let mut hosts = BTreeMap::new();
    let mut facades = Vec::new();
    for f in &before.facades {
        if !edges.insert(f.edge) || hosts.contains_key(&f.id) || s.doc.buildings.iter().flat_map(|b| &b.facades).filter(|g| g.id == f.id).count() != 1
        {
            return Err(invalid(format!("{} needs a unique facade ID and source edge {}, without remapping", f.id, f.edge)));
        }
        let host = Host { before: frame(before, f.edge)?, after: frame(&after, f.edge)? };
        if dot(host.before.t, host.after.t) <= 0. {
            return Err(invalid(format!(
                "{} tangent changes by {} degrees at lengths {} -> {} mm; reversal/reindexing is unsupported",
                f.id,
                host.angle(),
                host.before.length,
                host.after.length
            )));
        }
        if host.before != host.after {
            facades.push(json!({"id":f.id,"edge":f.edge,"before_origin_mm":host.before.origin,"after_origin_mm":host.after.origin,
                "before_tangent":host.before.t,"after_tangent":host.after.t,"before_length_mm":host.before.length,"after_length_mm":host.after.length,"rotation_deg":host.angle()}));
        }
        hosts.insert(f.id.clone(), host);
    }
    if edges.len() != before.footprint.len() {
        return Err(invalid(format!(
            "{} requires one facade per source edge: {} facades, {} corners",
            before.id,
            edges.len(),
            before.footprint.len()
        )));
    }
    retained_edges(before, &after)?;
    let mut anchors = Vec::new();
    for f in &mut after.facades {
        let h = *hosts.get(&f.id).ok_or_else(|| invalid("missing source host"))?;
        if h.before == h.after {
            continue;
        }
        let old_bays = f.bays.clone();
        for (i, u) in f.bays.iter_mut().enumerate() {
            h.edit(&f.id, &format!("bays/{i}"), u, p.anchor_policy, &mut anchors)?;
        }
        for (i, placement) in f.placements.iter_mut().enumerate() {
            let id = format!("placement:{}:{i}", f.id);
            if let Some(u) = &mut placement.u {
                h.edit(&id, "u", u, p.anchor_policy, &mut anchors)?;
            } else {
                let old = *old_bays.get(placement.bay).ok_or_else(|| invalid(format!("{id} has no bay {}", placement.bay)))?;
                let new = *f.bays.get(placement.bay).ok_or_else(|| invalid("missing preserved bay"))?;
                h.record(&id, &format!("shared_bay/{}", placement.bay), old, new, &mut anchors)?;
            }
            if let Some(record) = anchors.last_mut() {
                let suffix = if s.doc.component(&placement.type_id)?.kind.is_opening() { "" } else { ".balcony" };
                record["target_id"] = json!(format!("{}.L{}.B{}{suffix}", f.id, placement.level, placement.bay));
                record["facade"] = json!(f.id);
                record["level"] = json!(placement.level);
                record["bay"] = json!(placement.bay);
            }
        }
    }
    if let Some(dormers) = after.roof.dormers_mut() {
        for d in dormers {
            let h = *hosts.get(&d.facade).ok_or_else(|| invalid(format!("dormer {} has unknown facade {}", d.id, d.facade)))?;
            if h.before != h.after {
                h.edit(&format!("{}.dormer.{}", before.id, d.id), "u_mm", &mut d.u_mm, p.anchor_policy, &mut anchors)?;
            }
        }
    }
    if let Some(lights) = after.roof.rooflights_mut() {
        for d in lights {
            let h = *hosts.get(&d.facade).ok_or_else(|| invalid(format!("rooflight {} has unknown facade {}", d.id, d.facade)))?;
            if h.before != h.after {
                h.edit(&format!("{}.rooflight.{}", before.id, d.id), "u_mm", &mut d.u_mm, p.anchor_policy, &mut anchors)?;
            }
        }
    }
    if let Roof::Pitched { drainage: Some(d), .. } | Roof::HalfHipped { drainage: Some(d), .. } = after.roof.base_mut() {
        for pipe in &mut d.downpipes {
            let h = *hosts.get(&pipe.facade).ok_or_else(|| invalid(format!("downpipe {} has unknown facade {}", pipe.id, pipe.facade)))?;
            if h.before != h.after {
                h.edit(&format!("{}.roof.downpipe.{}", before.id, pipe.id), "along_mm", &mut pipe.along_mm, p.anchor_policy, &mut anchors)?;
            }
        }
    }
    for (i, c) in after.cellar_stairs.iter_mut().enumerate() {
        let id = if c.id.is_empty() { format!("{}.cellar{i}", c.facade) } else { c.id.clone() };
        let h = *hosts.get(&c.facade).ok_or_else(|| invalid(format!("{id} has unknown facade {}", c.facade)))?;
        if h.before == h.after {
            continue;
        }
        if p.anchor_policy == AnchorPolicy::ProjectWallLineCentres && h.angle().abs() > 1e-9 {
            return Err(invalid(format!(
                "{id} on {}: projected multi-station cellar stair with host rotation {} degrees and origin displacement {} mm is unsupported; keep_facade_u or review the stair separately",
                c.facade,
                h.angle(),
                distance(h.before.origin, h.after.origin)
            )));
        }
        for (field, u) in [("from", &mut c.from), ("to", &mut c.to), ("top", &mut c.top), ("bottom", &mut c.bottom), ("door_at", &mut c.door_at)] {
            h.edit(&id, field, u, p.anchor_policy, &mut anchors)?;
        }
    }
    fixed_sources(&after)?;
    if s.doc.phases.len() > 64 {
        return Err(invalid("footprint validation is bounded to 64 phases"));
    }
    // Some low-level roof support errors name only the numerical coverage failure.
    // Keep the model error verbatim and attach exact candidate source IDs/stations.
    let roof_dependencies: Vec<_> = after
        .roof
        .dormers()
        .iter()
        .map(|d| format!("dormer {} [{}.dormer.{}] on {} u={} mm setback={} mm", d.id, after.id, d.id, d.facade, d.u_mm, d.setback_mm))
        .chain(
            after
                .roof
                .rooflights()
                .iter()
                .map(|d| format!("rooflight {} [{}.rooflight.{}] on {} u={} mm run={} mm", d.id, after.id, d.id, d.facade, d.u_mm, d.run_mm)),
        )
        .collect();
    let mut candidate = s.doc.clone();
    *candidate.buildings.get_mut(index).ok_or_else(|| invalid("missing building slot"))? = after;
    let context = || {
        format!(
            "{} footprint edit; affected facade lengths mm {:?}; roof dependents {roof_dependencies:?}",
            before.id,
            facades.iter().map(|f| (&f["id"], &f["before_length_mm"], &f["after_length_mm"])).collect::<Vec<_>>()
        )
    };
    let validate = |doc: &Document| trigon_model::build(doc).map_err(|e| invalid(format!("{}: {e}", context())));
    let old_built = validate(&s.doc)?;
    let new_built = validate(&candidate)?;
    let source = candidate.buildings.get(index).ok_or_else(|| invalid("missing edited building"))?;
    let model = new_built.get(index).ok_or_else(|| invalid("missing built edited building"))?;
    // Rooms do not participate in model::build. Their source can still lie inside the
    // outer footprint while a moved wall consumes room floor. Use the model's derived
    // floor boundary (including its mitres/normalization), never invent a second offset.
    for (i, room) in source.rooms.iter().enumerate() {
        let id = if room.id.is_empty() { format!("{}.room{i}", source.id) } else { room.id.clone() };
        for &level in &room.levels {
            let floor = model
                .slabs
                .iter()
                .find(|slab| slab.kind == trigon_model::SlabKind::Floor && slab.level == level)
                .ok_or_else(|| invalid(format!("{id} level {level} has no built floor")))?;
            fixed_shape(&floor.polygon, &floor.id, &id, &room.polygon, true)?;
        }
    }
    for (a, b) in old_built.iter().zip(&new_built) {
        built_guard(a, b)?;
    }
    for phase in 0..s.doc.phases.len() {
        let view = trigon_model::build::View { phase, filter: trigon_model::build::PhaseFilter::All };
        let old = trigon_model::build::build_view(&s.doc, view)?;
        let new = trigon_model::build::build_view(&candidate, view).map_err(|e| invalid(format!("{} phase {phase}: {e}", context())))?;
        for (a, b) in old.iter().zip(&new) {
            built_guard(a, b)?;
        }
    }
    trigon_model::annotation::validate_document(&candidate)?;
    let registrations: Vec<_> = s
        .doc
        .drawings
        .iter()
        .flat_map(|d| {
            d.facades.iter().filter_map(|r| {
                let h = hosts.get(&r.facade)?;
                (h.before != h.after).then(|| {
                    json!({"drawing":d.name,"facade":r.facade,"before_px_per_mm":(r.x_end-r.x_start)/h.before.length,
            "after_px_per_mm":(r.x_end-r.x_start)/h.after.length,"status":"needs_source_review","stored_registration":"unchanged"})
                })
            })
        })
        .collect();
    let roof_heights = old_built
        .get(index)
        .and_then(|b| b.roof.as_ref())
        .zip(new_built.get(index).and_then(|b| b.roof.as_ref()))
        .map(|(a, b)| json!({"id":a.id,"before_ridge_mm":a.ridge,"after_ridge_mm":b.ridge,"before_eaves_mm":a.eaves,"after_eaves_mm":b.eaves}));
    let result = json!({"building":p.building,"anchor_policy":p.anchor_policy,"corners":corners,"facades":facades,"anchors":anchors,"roof_heights":roof_heights,
        "registration_feedback":registrations,"validation":{"model_build":"passed","annotations":"passed","phase_identities":"passed","interior_geometry":"unchanged","source_accuracy":"unverified"},
        "anchor_semantics":"wall-line tangential registration; perpendicular world position and offset component centroids are not preserved",
        "drawing_feedback":"Stored plan registrations, annotations and evidence remain unchanged; review dimensions and facade registrations against source drawings."});
    s.doc = candidate;
    Ok(result)
}
