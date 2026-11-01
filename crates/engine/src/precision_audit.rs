//! Read-only geometric candidates. Findings never imply a survey or authorize a repair.
use crate::{Result, Session};
use schemars::JsonSchema;
use serde::Deserialize;
use serde_json::{Value, json};
use trigon_model::{ModelError, checked};

#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub(crate) struct Audit {
    pub building: Option<String>,
    pub levels: Option<Vec<usize>>,
    #[serde(default = "default_gap")]
    pub gap_tolerance_mm: f64,
    /// Small deviations in end-to-end wall continuations, degrees (0.001..15).
    #[serde(default = "default_angle")]
    pub angle_tolerance_deg: f64,
    /// Unsigned angles expected at end-to-end joints (0..90 degrees). Add deliberate
    /// diagonal-wing angles explicitly; defaults are straight and perpendicular.
    #[serde(default = "default_angles")]
    pub expected_joint_angles_deg: Vec<f64>,
    /// Axis/face mismatch in near-parallel continuing wall runs, mm.
    #[serde(default = "default_alignment")]
    pub alignment_tolerance_mm: f64,
    /// Neighbourhood for possible wall continuations, mm (1..1000).
    #[serde(default = "default_joint")]
    pub joint_radius_mm: f64,
    #[serde(default = "default_results")]
    pub max_results: usize,
}
fn default_gap() -> f64 {
    10.0
}
fn default_angles() -> Vec<f64> {
    vec![0., 90.]
}
fn default_angle() -> f64 {
    0.5
}
fn default_alignment() -> f64 {
    2.0
}
fn default_joint() -> f64 {
    300.0
}
fn default_results() -> usize {
    1000
}

struct Findings {
    items: Vec<Value>,
    total: usize,
    limit: usize,
}
impl Findings {
    fn push(&mut self, value: Value) {
        self.total += 1;
        if self.items.len() < self.limit {
            self.items.push(value);
        }
    }
}

pub(crate) fn audit(s: &Session, p: Audit) -> Result<Value> {
    checked("gap tolerance", p.gap_tolerance_mm, 0.001, 300.0)?;
    checked("angle tolerance", p.angle_tolerance_deg, 0.001, 15.0)?;
    if p.expected_joint_angles_deg.is_empty() || p.expected_joint_angles_deg.len() > 32 {
        return Err(ModelError::Invalid("give 1..32 expected unsigned joint angles".into()).into());
    }
    for angle in &p.expected_joint_angles_deg {
        checked("expected joint angle", *angle, 0., 90.)?;
    }
    checked("alignment tolerance", p.alignment_tolerance_mm, 0.001, 300.0)?;
    checked("joint radius", p.joint_radius_mm, 1.0, 1000.0)?;
    if !(1..=10_000).contains(&p.max_results) || p.levels.as_ref().is_some_and(|l| l.is_empty() || l.len() > 60) {
        return Err(ModelError::Invalid("audit 1..60 storeys and request 1..10000 findings".into()).into());
    }
    if let Some(id) = &p.building {
        s.doc.building(Some(id))?;
    }
    let selected: Vec<_> = s.doc.buildings.iter().filter(|b| p.building.as_ref().is_none_or(|id| &b.id == id)).collect();
    if selected.is_empty() {
        return Err(ModelError::Invalid("precision audit needs a building".into()).into());
    }
    let mut findings = Findings { items: Vec::new(), total: 0, limit: p.max_results };
    let mut work = 0usize;
    let mut inspected = 0usize;
    let mut ambiguous = std::collections::BTreeMap::<String, usize>::new();
    // Inspect IDs before building: overlapping/duplicate sources can make a model unbuildable.
    for b in &s.doc.buildings {
        for id in b.interior_walls.iter().map(|w| &w.id).chain(b.stairs.iter().map(|st| &st.id)) {
            if !id.is_empty() {
                *ambiguous.entry(id.clone()).or_default() += 1;
            }
        }
    }
    for (id, count) in ambiguous.into_iter().filter(|(_, count)| *count > 1) {
        findings.push(json!({"kind":"ambiguous_source_id","id":id,"occurrences":count}));
    }
    for b in &selected {
        let levels: std::collections::BTreeSet<_> = p.levels.clone().unwrap_or_else(|| (0..b.storeys.len()).collect()).into_iter().collect();
        if levels.iter().any(|level| *level >= b.storeys.len()) {
            return Err(ModelError::Invalid("audit storey does not exist in the selected building".into()).into());
        }
        if b.stairs.len() > 1000 || levels.iter().any(|level| b.interior_walls.iter().filter(|w| w.levels.contains(level)).count() > 2000) {
            return Err(
                ModelError::Invalid("audit scope exceeds 1000 stair sources or 2000 walls per storey; select a smaller building".into()).into()
            );
        }
        for &level in &levels {
            let walls = b.interior_walls.iter().filter(|w| w.levels.contains(&level)).count();
            let stairs =
                b.stairs.iter().filter(|st| st.levels.contains(&level) || level.checked_sub(1).is_some_and(|l| st.levels.contains(&l))).count();
            work = work.saturating_add(walls.saturating_mul(walls)).saturating_add(stairs.saturating_mul(stairs));
            if work > 2_000_000 {
                return Err(ModelError::Invalid("audit comparison budget exceeded; select fewer storeys".into()).into());
            }
        }
        let stair_id = |stair: &trigon_model::Stair| {
            if stair.id.is_empty() {
                format!("{}.stair{}", b.id, b.stairs.iter().position(|source| std::ptr::eq(source, stair)).unwrap_or(0))
            } else {
                stair.id.clone()
            }
        };
        let mut seen_sources = std::collections::BTreeSet::new();
        for &level in &levels {
            wall_runs(b, level, &p, &mut findings);
            let shown = crate::plan::visible_stairs(b, level);
            for (i, a) in shown.iter().enumerate() {
                for b_stair in shown.iter().skip(i + 1) {
                    let (Ok(a_ring), Ok(b_ring)) = (trigon_model::stair::opening(a), trigon_model::stair::opening(b_stair)) else { continue };
                    let area = intersection_area(&a_ring, &b_ring);
                    if area > 1.0 {
                        let departures = |stair: &trigon_model::Stair| if stair.levels.contains(&level) { level } else { level.saturating_sub(1) };
                        let centre = |stair: &trigon_model::Stair| {
                            stair
                                .outline
                                .iter()
                                .fold([0.; 2], |c, q| [c[0] + q[0] / stair.outline.len() as f64, c[1] + q[1] / stair.outline.len() as f64])
                        };
                        let (ca, cb) = (centre(a), centre(b_stair));
                        findings.push(json!({"kind":"overlapping_stair_symbols","building":b.id,"level":level,
                            "ids":[format!("{}.L{}",stair_id(a),departures(a)),format!("{}.L{}",stair_id(b_stair),departures(b_stair))],
                            "departure_levels":[departures(a),departures(b_stair)],"opening_overlap_mm2":area,
                            "centre_delta_mm":[cb[0]-ca[0],cb[1]-ca[1]],"review_required":true}));
                    }
                }
            }
            // A receiving symbol may be suppressed; still report overlapping sources departing
            // on this same floor. These are candidates even if geometry validation later fails.
            let outgoing: Vec<_> = b.stairs.iter().filter(|st| st.levels.contains(&level)).collect();
            for (i, a) in outgoing.iter().enumerate() {
                for other in outgoing.iter().skip(i + 1) {
                    if let (Ok(ar), Ok(br)) = (trigon_model::stair::opening(a), trigon_model::stair::opening(other)) {
                        let area = intersection_area(&ar, &br);
                        if area > 1.0 && seen_sources.insert((a.id.clone(), other.id.clone(), level)) {
                            findings.push(json!({"kind":"overlapping_stair_sources","building":b.id,"level":level,
                                "ids":[format!("{}.L{level}",stair_id(a)),format!("{}.L{level}",stair_id(other))],"opening_overlap_mm2":area,"review_required":true}));
                        }
                    }
                }
            }
        }
    }
    let geometry = trigon_model::build(&s.doc);
    let geometry_valid = geometry.is_ok();
    match geometry {
        Err(error) => findings.push(json!({"kind":"model_build_error","message":error.to_string()})),
        Ok(built) => {
            for (source, model) in s.doc.buildings.iter().zip(&built).filter(|(b, _)| p.building.as_ref().is_none_or(|id| &b.id == id)) {
                let levels: std::collections::BTreeSet<_> =
                    p.levels.clone().unwrap_or_else(|| (0..source.storeys.len()).collect()).into_iter().collect();
                for level in levels {
                    let walls: Vec<_> = model.interior.iter().filter(|w| w.level == level).collect();
                    inspected += walls.len();
                    for (index, a) in walls.iter().enumerate() {
                        for b_wall in walls.iter().skip(index + 1) {
                            let dot = a.frame.t[0] * b_wall.frame.t[0] + a.frame.t[1] * b_wall.frame.t[1];
                            let angle = dot.abs().clamp(0., 1.).acos().to_degrees();
                            if angle > 15.0 {
                                continue;
                            } // Crossing/joined walls normally share material.
                            let overlap: f64 = a.cut.iter().flat_map(|ap| b_wall.cut.iter().map(move |bp| intersection_area(ap, bp))).sum();
                            if overlap > 1.0 {
                                findings.push(json!({"kind":"near_parallel_wall_overlap","building":source.id,"level":level,
                                "ids":[a.id,b_wall.id],"material_overlap_mm2":overlap,"angle_deg":angle,"review_required":true}));
                            }
                        }
                    }
                    let mut solids: Vec<(String, Vec<[f64; 2]>)> =
                        walls.iter().flat_map(|w| w.cut.iter().cloned().map(move |poly| (w.id.clone(), poly))).collect();
                    for wall in &model.walls {
                        let Some(segment) = wall.segment(level) else { continue };
                        let cut = model.storeys[level].elevation + 1000.0;
                        let mut holes: Vec<_> = wall
                            .openings
                            .iter()
                            .filter_map(|o| {
                                if o.rect.v0 > cut || o.rect.v1 < cut {
                                    return None;
                                }
                                Some(if o.profile.is_empty() { (o.rect.u0, o.rect.u1) } else { trigon_model::profile::width_at(&o.profile, cut)? })
                            })
                            .collect();
                        holes.sort_by(|a, b| a.0.total_cmp(&b.0));
                        let [a, b, inner_b, inner_a] = segment.plan;
                        let mut start = (a, inner_a);
                        let pt = |u: f64, w: f64| {
                            let p = wall.frame.point(u, w, 0.);
                            [p[0], p[1]]
                        };
                        for (u0, u1) in holes {
                            solids.push((segment.id.clone(), vec![start.0, pt(u0, 0.), pt(u0, segment.thickness), start.1]));
                            start = (pt(u1, 0.), pt(u1, segment.thickness));
                        }
                        solids.push((segment.id.clone(), vec![start.0, b, inner_b, start.1]));
                    }
                    for wall in &walls {
                        for (end, cap) in [("a", [wall.outline[0], wall.outline[3]]), ("b", [wall.outline[1], wall.outline[2]])] {
                            let mut nearest: Option<(f64, &str, [f64; 2], [f64; 2])> = None;
                            for (id, poly) in &solids {
                                if id == &wall.id || poly.len() < 3 || trigon_model::doc::polygon_area(poly) < 1e-6 {
                                    continue;
                                }
                                let (distance, from, to) = segment_polygon(cap, poly);
                                if nearest.as_ref().is_none_or(|n| distance < n.0 || (distance == n.0 && id.as_str() < n.1)) {
                                    nearest = Some((distance, id, from, to));
                                }
                            }
                            if let Some((gap, id, from, to)) = nearest
                                && gap > 0.001
                                && gap <= p.gap_tolerance_mm
                            {
                                findings.push(json!({"kind":"wall_gap","building":source.id,"level":level,"id":wall.id,
                                "end":end,"nearest_id":id,"gap_mm":gap,"from":from,"to":to,
                                "delta_mm":[to[0]-from[0],to[1]-from[1]],"review_required":true}));
                            }
                        }
                    }
                }
            }
        }
    }
    let total = findings.total;
    let findings = findings.items;
    Ok(json!({"status":if total==0&&geometry_valid {"no_findings_in_scope"} else {"needs_review"},"geometry_valid":geometry_valid,
        "wall_instances_inspected":inspected,"gap_tolerance_mm":p.gap_tolerance_mm,"angle_tolerance_deg":p.angle_tolerance_deg,"expected_joint_angles_deg":p.expected_joint_angles_deg,"alignment_tolerance_mm":p.alignment_tolerance_mm,"joint_radius_mm":p.joint_radius_mm,"findings":findings,
        "total_findings":total,"truncated":total>p.max_results,"scope":"latest phase; 1000 mm plan cut; nearest material at each interior wall end; near-parallel wall overlaps and end-to-end run deviations; overlapping visible/departing stair openings",
        "limitations":["Intentional gaps and adjacent-storey stair overlaps need source review; findings are not automatic repairs.","No structural, accessibility, survey or complete clash certification."]}))
}

/// Recognise possible continuations by opposing inward directions at nearby endpoints.
/// This avoids calling every pair of legitimate parallel room walls an alignment defect.
fn wall_runs(building: &trigon_model::Building, level: usize, p: &Audit, findings: &mut Findings) {
    let walls: Vec<_> = building.interior_walls.iter().enumerate().filter(|(_, w)| w.levels.contains(&level)).collect();
    let id = |index: usize, w: &trigon_model::InteriorWall| {
        if w.id.is_empty() { format!("{}.iw{index}.L{level}", building.id) } else { format!("{}.L{level}", w.id) }
    };
    for (index, (ai, a)) in walls.iter().enumerate() {
        let al = (a.b[0] - a.a[0]).hypot(a.b[1] - a.a[1]);
        if !al.is_finite() || al < 100. {
            continue;
        }
        let at = [(a.b[0] - a.a[0]) / al, (a.b[1] - a.a[1]) / al];
        for (bi, b) in walls.iter().skip(index + 1) {
            let bl = (b.b[0] - b.a[0]).hypot(b.b[1] - b.a[1]);
            if !bl.is_finite() || bl < 100. {
                continue;
            }
            let bt = [(b.b[0] - b.a[0]) / bl, (b.b[1] - b.a[1]) / bl];
            let dot = (at[0] * bt[0] + at[1] * bt[1]).clamp(-1., 1.);
            let angle = dot.abs().acos().to_degrees();
            let expected = p.expected_joint_angles_deg.iter().any(|a| (angle - a).abs() <= p.angle_tolerance_deg);
            let mut nearest = None;
            for (ae, ap, asign) in [("a", a.a, 1.), ("b", a.b, -1.)] {
                for (be, bp, bsign) in [("a", b.a, 1.), ("b", b.b, -1.)] {
                    if angle <= 15.0 && dot * asign * bsign > -15.0_f64.to_radians().cos() {
                        continue;
                    }
                    let distance = (bp[0] - ap[0]).hypot(bp[1] - ap[1]);
                    if distance <= p.joint_radius_mm && nearest.as_ref().is_none_or(|n: &(f64, &str, &str, [f64; 2], [f64; 2])| distance < n.0) {
                        nearest = Some((distance, ae, be, ap, bp));
                    }
                }
            }
            let Some((distance, ae, be, ap, bp)) = nearest else { continue };
            let offset = -(bp[0] - ap[0]) * at[1] + (bp[1] - ap[1]) * at[0];
            let face = [-1., 1.]
                .into_iter()
                .flat_map(|sa| [-1., 1.].into_iter().map(move |sb| (offset + sb * b.thickness / 2. - sa * a.thickness / 2.).abs()))
                .fold(f64::INFINITY, f64::min);
            let kind = if !expected {
                Some("sudden_wall_angle_change")
            } else if angle <= p.angle_tolerance_deg && offset.abs() > p.alignment_tolerance_mm && face > p.alignment_tolerance_mm {
                Some("misaligned_wall_run")
            } else {
                None
            };
            if let Some(kind) = kind {
                findings.push(json!({"kind":kind,"building":building.id,"level":level,"ids":[id(*ai,a),id(*bi,b)],
                    "ends":[ae,be],"points":[ap,bp],"endpoint_distance_mm":distance,"axis_offset_mm":offset,
                    "nearest_face_offset_mm":face,"angle_deg":angle,"review_required":true}));
            }
        }
    }
}

fn intersection_area(a: &[[f64; 2]], b: &[[f64; 2]]) -> f64 {
    use i_overlay::core::{fill_rule::FillRule, overlay_rule::OverlayRule};
    use i_overlay::float::overlay::FloatOverlay;
    let shapes = FloatOverlay::with_subj_and_clip(&a.to_vec(), &b.to_vec()).overlay(OverlayRule::Intersect, FillRule::EvenOdd);
    shapes
        .iter()
        .map(|shape| {
            shape
                .iter()
                .enumerate()
                .map(|(i, ring)| if i == 0 { trigon_model::doc::polygon_area(ring) } else { -trigon_model::doc::polygon_area(ring) })
                .sum::<f64>()
        })
        .sum()
}
fn closest(p: [f64; 2], a: [f64; 2], b: [f64; 2]) -> [f64; 2] {
    let d = [b[0] - a[0], b[1] - a[1]];
    let length = d[0] * d[0] + d[1] * d[1];
    let t = if length > 1e-18 { ((p[0] - a[0]) * d[0] + (p[1] - a[1]) * d[1]) / length } else { 0. };
    [a[0] + d[0] * t.clamp(0., 1.), a[1] + d[1] * t.clamp(0., 1.)]
}
fn segment_polygon(cap: [[f64; 2]; 2], poly: &[[f64; 2]]) -> (f64, [f64; 2], [f64; 2]) {
    if crate::drawings::inside(poly, cap[0]) {
        return (0., cap[0], cap[0]);
    }
    if crate::drawings::inside(poly, cap[1]) {
        return (0., cap[1], cap[1]);
    }
    let mut best = (f64::INFINITY, cap[0], cap[0]);
    for i in 0..poly.len() {
        let (a, b) = (poly[i], poly[(i + 1) % poly.len()]);
        let d = [cap[1][0] - cap[0][0], cap[1][1] - cap[0][1]];
        let e = [b[0] - a[0], b[1] - a[1]];
        let den = d[0] * e[1] - d[1] * e[0];
        if den.abs() > 1e-12 {
            let v = [a[0] - cap[0][0], a[1] - cap[0][1]];
            let (t, u) = ((v[0] * e[1] - v[1] * e[0]) / den, (v[0] * d[1] - v[1] * d[0]) / den);
            if (0.0..=1.0).contains(&t) && (0.0..=1.0).contains(&u) {
                let p = [cap[0][0] + d[0] * t, cap[0][1] + d[1] * t];
                return (0., p, p);
            }
        }
        for (from, to) in
            [(cap[0], closest(cap[0], a, b)), (cap[1], closest(cap[1], a, b)), (closest(a, cap[0], cap[1]), a), (closest(b, cap[0], cap[1]), b)]
        {
            let distance = (from[0] - to[0]).hypot(from[1] - to[1]);
            if distance < best.0 {
                best = (distance, from, to);
            }
        }
    }
    best
}
