//! Exact, atomic source refinement. Geometry remains native parametric Stair geometry.
use std::collections::{BTreeMap, BTreeSet};

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use trigon_model::{Document, ModelError, Stair};

use crate::{Result, Session};

// Existing authoring/audit conventions: 60 storeys, 1000 stair sources, 100 buildings,
// 200 groups, 256 group targets, 256-byte entrance source IDs, 5000 batch objects.
const MAX_LEVELS: usize = 60;
const MAX_STAIRS: usize = 1000;
const MAX_CONSUMERS: usize = 5000;
const MAX_IDENTITIES: usize = 100_000; // 100 buildings × the audit's 1000-source bound.

#[derive(Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub(crate) struct RefineSource {
    /// Exact building ID; no default building or displayed occurrence selector.
    pub building: String,
    /// Complete current named native Stair, compared before any edits (including level order).
    pub expected_source: Stair,
    /// Complete replacement sources. Native defaults apply; nothing inherits from expected_source.
    /// Disjoint nonempty levels cover the original exactly; exactly one keeps its ID and slot.
    pub partitions: Vec<Stair>,
}

fn invalid(message: impl Into<String>) -> crate::EngineError {
    ModelError::Invalid(message.into()).into()
}

fn identifier(id: &str) -> Result<()> {
    if id.len() > 256 {
        return Err(invalid("stair refinement source/building ID exceeds 256 bytes"));
    }
    if id.is_empty() || !id.chars().all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '-' | '_')) {
        return Err(invalid(format!("stair refinement needs a named 1..256 byte simple source ID: {id:?}")));
    }
    Ok(())
}

fn levels(stair: &Stair, storeys: usize) -> Result<BTreeSet<usize>> {
    if stair.levels.is_empty() || stair.levels.len() > MAX_LEVELS {
        return Err(invalid(format!("{}: give 1..60 departure levels", stair.id)));
    }
    let mut levels = BTreeSet::new();
    for &level in &stair.levels {
        if level >= storeys {
            return Err(invalid(format!("{}: departure level {level} does not exist", stair.id)));
        }
        if !levels.insert(level) {
            return Err(invalid(format!("{}: duplicate departure level {level}", stair.id)));
        }
    }
    Ok(levels)
}

// Selectors accept numeric suffixes with leading zeroes. Reserve those aliases too,
// so a source named core.L00 cannot silently shadow the occurrence core.L0.
fn key(id: &str) -> String {
    match id.rsplit_once(".L").and_then(|(root, level)| Some((root, level.parse::<usize>().ok()?))) {
        Some((root, level)) => format!("{root}.L{level}"),
        None => id.to_string(),
    }
}

type Namespace = BTreeMap<String, BTreeSet<String>>;

fn reserve(namespace: &mut Namespace, id: String, owner: &str, work: &mut usize) -> Result<()> {
    *work += 1;
    if *work > MAX_IDENTITIES {
        return Err(invalid("stair refinement exceeds 100000 source/instance identity checks"));
    }
    namespace.entry(key(&id)).or_default().insert(owner.to_string());
    Ok(())
}

fn aliases(stair: &Stair) -> impl Iterator<Item = String> + '_ {
    [stair.id.clone(), format!("stair:{}", stair.id)].into_iter().chain(stair.levels.iter().map(|level| format!("{}.L{level}", stair.id)))
}

/// Mirror the editable source/occurrence namespaces without geometry-equality inference.
/// All phase placements are reserved, including ones absent from the current view.
fn namespace(doc: &Document) -> Result<Namespace> {
    if doc.buildings.len() > 100 {
        return Err(invalid("stair refinement is bounded to 100 buildings"));
    }
    let mut result = Namespace::new();
    let mut work = 0;
    for (bi, b) in doc.buildings.iter().enumerate() {
        reserve(&mut result, b.id.clone(), &format!("building:{bi}"), &mut work)?;
        let sources = b
            .interior_walls
            .iter()
            .enumerate()
            .map(|(i, w)| ("wall", "iw", i, &w.id, &w.levels))
            .chain(b.stairs.iter().enumerate().map(|(i, s)| ("stair", "stair", i, &s.id, &s.levels)))
            .chain(b.rooms.iter().enumerate().map(|(i, r)| ("room", "room", i, &r.id, &r.levels)));
        for (kind, fallback, i, id, levels) in sources {
            let root = if id.is_empty() { format!("{}.{fallback}{i}", b.id) } else { id.clone() };
            let owner = format!("{bi}:{kind}:{i}");
            for alias in [root.clone(), format!("{kind}:{root}")] {
                reserve(&mut result, alias, &owner, &mut work)?;
            }
            for level in levels {
                reserve(&mut result, format!("{root}.L{level}"), &owner, &mut work)?;
                if kind == "room" {
                    reserve(&mut result, format!("{root}.space.L{level}"), &owner, &mut work)?;
                }
            }
        }
        for (fi, f) in b.facades.iter().enumerate() {
            let owner = format!("{bi}:facade:{fi}");
            reserve(&mut result, f.id.clone(), &owner, &mut work)?;
            for level in 0..b.storeys.len() {
                reserve(&mut result, format!("{}.L{level}", f.id), &owner, &mut work)?;
            }
            for (pi, p) in f.placements.iter().enumerate() {
                let owner = format!("{bi}:facade:{fi}:placement:{pi}");
                for alias in [
                    format!("placement:{}:{pi}", f.id),
                    format!("{}.L{}.B{}", f.id, p.level, p.bay),
                    format!("{}.L{}.B{}.balcony", f.id, p.level, p.bay),
                ] {
                    reserve(&mut result, alias, &owner, &mut work)?;
                }
            }
        }
    }
    for object in &doc.site_objects {
        reserve(&mut result, object.id.clone(), "site object", &mut work)?;
    }
    Ok(result)
}

fn unique(namespace: &Namespace, stair: &Stair, owner: &str) -> Result<()> {
    for alias in aliases(stair) {
        if namespace.get(&key(&alias)).is_none_or(|owners| owners.len() != 1 || !owners.contains(owner)) {
            return Err(invalid(format!("{}: ambiguous source/instance ID {alias}", stair.id)));
        }
    }
    Ok(())
}

fn placement_aliases(doc: &Document, stair: &Stair) -> Result<()> {
    for alias in aliases(stair) {
        for facade in doc.buildings.iter().flat_map(|b| &b.facades) {
            // The existing editable facade selector accepts .L01.B02 and arbitrary
            // display suffixes. Reserve all of them, even in historical phases.
            let native = alias
                .strip_prefix(&format!("{}.L", facade.id))
                .and_then(|s| s.split_once(".B"))
                .and_then(|(l, rest)| Some((l.parse::<usize>().ok()?, rest.split('.').next()?.parse::<usize>().ok()?)));
            if native.is_some_and(|(level, bay)| facade.placements.iter().any(|p| p.level == level && p.bay == bay)) {
                return Err(invalid(format!("{}: ambiguous facade instance selector {alias}", stair.id)));
            }
        }
    }
    Ok(())
}

/// Compute references against the untouched document, then apply this mapped copy once.
fn consumers(
    doc: &Document,
    bi: usize,
    original: &Stair,
    partitions: &[Stair],
    owners: &BTreeMap<usize, String>,
    ns: &Namespace,
) -> Result<(Document, Value)> {
    if doc.groups.len() > 200 || doc.buildings.iter().fold(0usize, |n, b| n.saturating_add(b.entrances.len())) > MAX_CONSUMERS {
        return Err(invalid("stair refinement is bounded to 200 groups and 5000 entrances"));
    }
    let new_ids: BTreeSet<_> = partitions.iter().filter(|p| p.id != original.id).map(|p| p.id.as_str()).collect();
    let mut candidate = doc.clone();
    let mut entrance_changes = Vec::new();
    for (building_index, b) in candidate.buildings.iter_mut().enumerate() {
        for (ei, e) in b.entrances.iter_mut().enumerate() {
            let id = format!("{}.entrance{ei}", b.id);
            if e.stair_by_level.len() > MAX_LEVELS {
                return Err(invalid(format!("{id}: at most 60 stored departure overrides can be mapped")));
            }
            for reference in e.stair.iter().chain(e.stair_by_level.values()) {
                if new_ids.contains(reference.as_str()) {
                    return Err(invalid(format!("{id}: new source ID {reference} already occurs in an entrance reference")));
                }
                if reference == &original.id && building_index != bi {
                    return Err(invalid(format!("{id}: unsupported cross-building stair reference {reference}")));
                }
            }
            if building_index != bi {
                continue;
            }
            for (&level, reference) in &e.stair_by_level {
                if reference == &original.id && !owners.contains_key(&level) {
                    return Err(invalid(format!("{id}: unsupported stair reference {reference} at departure {level}")));
                }
            }
            // Preserve the stored default and unrelated overrides byte-for-byte. Add/replace
            // only slots whose OLD effective assignment was this source and whose owner moves.
            for (&level, next) in owners {
                if e.stair_at(level) == Some(original.id.as_str()) && next != &original.id {
                    let stored = e.stair_by_level.insert(level, next.clone());
                    entrance_changes.push(json!({"id":id,"departure_level":level,
                        "before_stored":stored,"before_effective":original.id,"after_stored":next,"after_effective":next}));
                }
            }
        }
    }
    let mut group_changes = Vec::new();
    let source_selector = format!("stair:{}", original.id);
    let prefix = format!("{}.L", original.id);
    let new_aliases: BTreeSet<_> = partitions.iter().filter(|p| p.id != original.id).flat_map(aliases).map(|s| key(&s)).collect();
    for group in &mut candidate.groups {
        if group.targets.len() > 256 || group.targets.iter().any(|t| t.len() > 512) {
            return Err(invalid(format!("group {}: at most 256 targets of at most 512 bytes", group.id)));
        }
        if let Some(target) = group.targets.iter().find(|t| t.starts_with("group:")) {
            return Err(invalid(format!("group {}: unsupported nested group reference {target}", group.id)));
        }
        let before = group.targets.clone();
        let mut targets = Vec::new();
        let mut selected = BTreeSet::new();
        for target in &before {
            if new_aliases.contains(&key(target)) {
                return Err(invalid(format!("group {}: new source/instance ID {target} already occurs in a reference", group.id)));
            }
            let all = target == &original.id || target == &source_selector;
            let occurrence = target.strip_prefix(&prefix).and_then(|s| s.parse::<usize>().ok());
            if all || occurrence.is_some_and(|level| owners.contains_key(&level)) {
                let selected_levels: Vec<_> = if all { original.levels.clone() } else { occurrence.into_iter().collect() };
                if selected_levels.iter().any(|level| !selected.insert(*level)) {
                    return Err(invalid(format!("group {}: overlapping source/occurrence references to {} ({target})", group.id, original.id)));
                }
                if all {
                    targets.extend(partitions.iter().map(|p| format!("stair:{}", p.id)));
                } else if let Some(level) = occurrence {
                    let next = owners.get(&level).ok_or_else(|| invalid(format!("group {}: unmapped {target}", group.id)))?;
                    targets.push(format!("{next}.L{level}"));
                }
            } else {
                let related = target.starts_with(&prefix)
                    || target.starts_with(&format!("stair:{prefix}"))
                    || target == &format!("wall:{}", original.id)
                    || target == &format!("room:{}", original.id);
                if related && !ns.contains_key(&key(target)) {
                    return Err(invalid(format!("group {}: unsupported stair occurrence reference {target}", group.id)));
                }
                targets.push(target.clone());
            }
        }
        if targets != before {
            if doc.groups.iter().filter(|g| g.id == group.id).count() != 1 {
                return Err(invalid(format!("ambiguous group ID {} while mapping {}", group.id, original.id)));
            }
            if targets.len() > 256 {
                return Err(invalid(format!("group {}: mapped targets exceed 256", group.id)));
            }
            group_changes.push(json!({"id":group.id,"before":before,"after":targets}));
            group.targets = targets;
        }
    }
    Ok((candidate, json!({"entrances":entrance_changes,"groups":group_changes})))
}

fn feedback(doc: &Document, building: &str, levels: &[usize]) -> Value {
    let mut session = Session::new(Box::new(trigon_data::Live::new(Default::default())));
    session.doc = doc.clone();
    let report = serde_json::from_value(json!({"building":building,"levels":levels}))
        .map_err(|e| invalid(format!("cannot construct stair audit: {e}")))
        .and_then(|scope| crate::precision_audit::audit(&session, scope));
    match report {
        Ok(report) => report,
        Err(error) => json!({"status":"unverified","error":error.to_string()}),
    }
}

pub(crate) fn refine(s: &mut Session, p: RefineSource) -> Result<Value> {
    identifier(&p.building)?;
    identifier(&p.expected_source.id)?;
    if p.partitions.is_empty() || p.partitions.len() > MAX_LEVELS {
        return Err(invalid("stair refinement needs 1..60 partitions"));
    }
    let mut buildings = s.doc.buildings.iter().enumerate().filter(|(_, b)| b.id == p.building);
    let (bi, building) = buildings.next().ok_or_else(|| invalid(format!("building {} does not exist", p.building)))?;
    if buildings.next().is_some() {
        return Err(invalid(format!("ambiguous building ID {}", p.building)));
    }
    if building.storeys.len() > MAX_LEVELS || building.stairs.len().saturating_add(p.partitions.len() - 1) > MAX_STAIRS {
        return Err(invalid(format!("{}: stair refinement is bounded to 60 storeys and 1000 resulting stair sources", p.building)));
    }
    let mut sources = building.stairs.iter().enumerate().filter(|(_, st)| st.id == p.expected_source.id);
    let (si, original) =
        sources.next().ok_or_else(|| invalid(format!("named stair source {} does not exist in {}", p.expected_source.id, p.building)))?;
    if sources.next().is_some() {
        return Err(invalid(format!("ambiguous stair source ID {}", original.id)));
    }
    if original != &p.expected_source {
        return Err(invalid(format!("{}: stale expected_source; read the complete source again", original.id)));
    }
    let original_levels = levels(original, building.storeys.len())?;
    let mut owners = BTreeMap::new();
    let mut ids = BTreeSet::new();
    let mut retained = 0;
    for stair in &p.partitions {
        identifier(&stair.id)?;
        if !ids.insert(&stair.id) {
            return Err(invalid(format!("duplicate partition source ID {}", stair.id)));
        }
        retained += usize::from(stair.id == original.id);
        for level in levels(stair, building.storeys.len())? {
            if let Some(other) = owners.insert(level, stair.id.clone()) {
                return Err(invalid(format!("{} and {other}: overlapping departure level {level}", stair.id)));
            }
        }
        trigon_model::stair::frame(stair).map_err(|e| invalid(format!("{}: {e}", stair.id)))?;
    }
    if retained != 1 {
        return Err(invalid(format!("exactly one partition must retain original source ID {}", original.id)));
    }
    if owners.keys().copied().collect::<BTreeSet<_>>() != original_levels {
        return Err(invalid(format!("{}: partition departure union must equal original levels {:?} exactly", original.id, original.levels)));
    }
    if p.partitions.as_slice() == std::slice::from_ref(original) {
        return Err(invalid(format!("{}: refinement makes no source change", original.id)));
    }
    let before_namespace = namespace(&s.doc)?;
    unique(&before_namespace, original, &format!("{bi}:stair:{si}"))?;
    placement_aliases(&s.doc, original)?;
    for stair in p.partitions.iter().filter(|st| st.id != original.id) {
        placement_aliases(&s.doc, stair)?;
        for alias in aliases(stair) {
            if before_namespace.contains_key(&key(&alias)) {
                return Err(invalid(format!("{}: source/instance ID {alias} already exists", stair.id)));
            }
        }
    }
    let (mut candidate, references) = consumers(&s.doc, bi, original, &p.partitions, &owners, &before_namespace)?;
    let next = candidate.buildings.get_mut(bi).ok_or_else(|| invalid("missing candidate building"))?;
    let mut membership = Vec::new();
    for stair in &p.partitions {
        let index = if stair.id == original.id {
            *next.stairs.get_mut(si).ok_or_else(|| invalid("missing original source slot"))? = stair.clone();
            si
        } else {
            let index = next.stairs.len();
            next.stairs.push(stair.clone());
            index
        };
        membership.push(json!({"id":stair.id,"index":index,"levels":stair.levels}));
    }
    let after_namespace = namespace(&candidate)?;
    for (index, stair) in candidate
        .buildings
        .get(bi)
        .ok_or_else(|| invalid("missing candidate building"))?
        .stairs
        .iter()
        .enumerate()
        .filter(|(_, st)| ids.contains(&st.id))
    {
        unique(&after_namespace, stair, &format!("{bi}:stair:{index}"))?;
    }
    let context = |error: ModelError| invalid(format!("{} source {} refinement: {error}", p.building, original.id));
    trigon_model::annotation::validate_document(&candidate).map_err(context)?;
    let built = trigon_model::build(&candidate).map_err(context)?;
    // Derived non-stair element IDs (slabs, roof products, doors, etc.) also occupy
    // the displayed namespace. Never introduce a source that shadows one of them.
    let changed_aliases: BTreeSet<_> = p.partitions.iter().chain(std::iter::once(original)).flat_map(aliases).map(|id| key(&id)).collect();
    for record in crate::element_properties_from_built(&candidate, &built, trigon_model::build::View::latest(&candidate))? {
        if record["kind"] != "stair" && record["id"].as_str().is_some_and(|id| changed_aliases.contains(&key(id))) {
            return Err(invalid(format!("ambiguous source/instance ID {} ({})", record["id"], record["kind"])));
        }
    }
    let next = candidate.buildings.get(bi).ok_or_else(|| invalid("missing candidate building"))?;
    let model = built.get(bi).ok_or_else(|| invalid("missing candidate geometry"))?;
    let floors = crate::footprint_edit::stair_shaft_floors(next, model, &p.partitions, true)?;
    let arrival_levels: BTreeSet<_> =
        original_levels.iter().filter_map(|level| level.checked_add(1).filter(|l| *l < building.storeys.len())).collect();
    let affected: Vec<_> = original_levels.union(&arrival_levels).copied().collect();
    let mapping: Vec<_> = original
        .levels
        .iter()
        .map(|level| {
            let next = owners.get(level).ok_or_else(|| invalid(format!("{}: unmapped departure {level}", original.id)))?;
            Ok(json!({"departure_level":level,"before":format!("{}.L{level}",original.id),"after":format!("{next}.L{level}")}))
        })
        .collect::<Result<_>>()?;
    let result = json!({"building":p.building,"original":{"id":original.id,"index":si,"levels":original.levels},
        "partitions":membership,"displayed_id_mapping":mapping,"departure_levels":original_levels,"arrival_levels":arrival_levels,
        "affected_levels":affected,"affected_floors":floors,"references":references,
        "validation":{"model_build":"passed","annotations":"passed","changed_shaft_floors":"passed"},
        "adjacent_feedback":{"before":feedback(&s.doc,&p.building,&affected),"after":feedback(&candidate,&p.building,&affected)},
        "scope":"Changed shaft fit and bounded affected-storey audit only; not an all-source clearance, survey or accessibility approval."});
    s.doc = candidate;
    Ok(result)
}
