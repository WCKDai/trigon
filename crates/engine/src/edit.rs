//! Precise edits of source parameters. A picked storey instance is split from its shared
//! definition before it changes; the other storeys and facade bay axes stay where they were.

use std::collections::BTreeSet;

use schemars::JsonSchema;
use serde::Deserialize;
use serde_json::{Value, json};
use trigon_model::build::FacadeFrame;
use trigon_model::doc::{InteriorWall, Room, Stair};
use trigon_model::{Document, ModelError, Placement, checked};

use crate::{Result, Session};

const MAX_TARGETS: usize = 256;
const MAX_COPIES: usize = 5000;

#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub(crate) struct Selection {
    /// Studio element ids, or wall:<id>, stair:<id>, room:<id> for every storey of a definition.
    /// placement:<facade>:<index> selects an exact placement from facade.list.
    pub targets: Vec<String>,
}

#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub(crate) struct Move {
    pub targets: Vec<String>,
    /// Site-local [dx, dy, dz], millimetres. Facade instances stay on their host wall;
    /// walls, stairs and rooms stay on their storeys (use copy_to_levels to change storeys).
    pub delta: [f64; 3],
}

#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub(crate) struct Copy {
    pub targets: Vec<String>,
    /// Offset per copy; copy n is at n * delta, with the original left in place.
    pub delta: [f64; 3],
    #[serde(default = "one")]
    pub count: usize,
}

fn one() -> usize {
    1
}

#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub(crate) struct CopyToLevels {
    pub targets: Vec<String>,
    /// Destination storey indices. Source storeys are skipped; duplicates count once.
    pub levels: Vec<usize>,
    /// Replace matching geometry/slots at destinations. Otherwise a match is an error.
    #[serde(default)]
    pub replace: bool,
}

#[derive(Clone, Copy, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub(crate) enum Axis {
    X,
    Y,
    U,
    Z,
}

#[derive(Clone, Copy, Default, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub(crate) enum Anchor {
    Start,
    #[default]
    Center,
    End,
}

#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub(crate) struct Align {
    pub targets: Vec<String>,
    /// X/Y in plan; U along a facade; Z is an opening's absolute height above the site origin.
    pub axis: Option<Axis>,
    /// Align the chosen edge/centre to this coordinate, or give a reference element instead.
    pub coordinate: Option<f64>,
    pub reference: Option<String>,
    /// Alternatively align to an arbitrary plan line (two site-local points).
    pub line: Option<[[f64; 2]; 2]>,
    #[serde(default)]
    pub anchor: Anchor,
}

fn invalid(message: impl Into<String>) -> crate::EngineError {
    ModelError::Invalid(message.into()).into()
}

#[derive(Clone, PartialEq)]
enum Item {
    Wall(InteriorWall),
    Stair(Stair),
    Room(Room),
    Placement(Placement),
}

impl Item {
    fn id(&self) -> &str {
        match self {
            Self::Wall(w) => &w.id,
            Self::Stair(s) => &s.id,
            Self::Room(r) => &r.id,
            Self::Placement(_) => "",
        }
    }
    fn levels(&self) -> Vec<usize> {
        match self {
            Self::Wall(w) => w.levels.clone(),
            Self::Stair(s) => s.levels.clone(),
            Self::Room(r) => r.levels.clone(),
            Self::Placement(p) => vec![p.level],
        }
    }
    fn set_levels(&mut self, levels: Vec<usize>) {
        match self {
            Self::Wall(w) => w.levels = levels,
            Self::Stair(s) => s.levels = levels,
            Self::Room(r) => r.levels = levels,
            Self::Placement(p) => {
                if let Some(&level) = levels.first() {
                    p.level = level;
                }
            }
        }
    }
    fn set_id(&mut self, id: String) {
        match self {
            Self::Wall(w) => w.id = id,
            Self::Stair(s) => s.id = id,
            Self::Room(r) => r.id = id,
            Self::Placement(_) => {}
        }
    }
    fn points(&self) -> Vec<[f64; 2]> {
        match self {
            Self::Wall(w) => {
                let dx = w.b[0] - w.a[0];
                let dy = w.b[1] - w.a[1];
                let len = dx.hypot(dy).max(1.0);
                let n = [-dy / len * w.thickness / 2.0, dx / len * w.thickness / 2.0];
                [w.a, w.b].into_iter().flat_map(|p| [[p[0] + n[0], p[1] + n[1]], [p[0] - n[0], p[1] - n[1]]]).collect()
            }
            Self::Stair(s) => s.outline.clone(),
            Self::Room(r) => r.polygon.clone(),
            Self::Placement(_) => Vec::new(),
        }
    }
    fn translate_plan(&mut self, delta: [f64; 3]) -> Result<()> {
        if delta[2].abs() > 1e-6 {
            return Err(invalid("plan elements stay on their storeys; use edit.copy_to_levels"));
        }
        let shift = |p: &mut [f64; 2]| -> Result<()> {
            for i in 0..2 {
                p[i] = checked("site coordinate", p[i] + delta[i], -1e8, 1e8)?;
            }
            Ok(())
        };
        match self {
            Self::Wall(w) => {
                shift(&mut w.a)?;
                shift(&mut w.b)?;
            }
            Self::Stair(s) => {
                for p in &mut s.outline {
                    shift(p)?;
                }
            }
            Self::Room(r) => {
                for p in &mut r.polygon {
                    shift(p)?;
                }
            }
            Self::Placement(_) => return Err(invalid("a facade instance needs its host frame")),
        }
        Ok(())
    }
}

#[derive(Clone)]
struct Entry {
    building: usize,
    facade: Option<String>,
    original: Item,
    item: Item,
}

impl Entry {
    fn frame(&self, doc: &Document) -> Result<FacadeFrame> {
        let b = doc.buildings.get(self.building).ok_or_else(|| invalid("missing building"))?;
        let f = b.facade(self.facade.as_deref().ok_or_else(|| invalid("select a facade instance"))?)?;
        let (a, b) = b.edge(f.edge).ok_or_else(|| invalid("missing facade edge"))?;
        FacadeFrame::new(a, b).ok_or_else(|| invalid("degenerate facade"))
    }
    fn u(&self, doc: &Document, p: &Placement) -> Result<f64> {
        let b = doc.buildings.get(self.building).ok_or_else(|| invalid("missing building"))?;
        let f = b.facade(self.facade.as_deref().ok_or_else(|| invalid("missing facade"))?)?;
        p.u.or_else(|| f.bays.get(p.bay).copied()).ok_or_else(|| invalid("missing bay"))
    }
    fn shifted(&self, doc: &Document, delta: [f64; 3]) -> Result<Item> {
        let mut item = self.item.clone();
        if let Item::Placement(p) = &mut item {
            let f = self.frame(doc)?;
            let across = delta[0] * f.n[0] + delta[1] * f.n[1];
            if across.abs() > 0.01 {
                return Err(invalid("facade instances can move only along their host wall and vertically"));
            }
            p.u = Some(checked("placement u", self.u(doc, p)? + delta[0] * f.t[0] + delta[1] * f.t[1], -1e8, 1e8)?);
            let ty = doc.component(&p.type_id)?;
            p.sill = Some(checked("sill", p.sill.unwrap_or(ty.sill) + delta[2], -1e6, 1e6)?);
        } else {
            item.translate_plan(delta)?;
        }
        Ok(item)
    }
    fn extent(&self, doc: &Document, axis: Axis, normal: Option<[f64; 2]>) -> Result<(f64, f64)> {
        if let Item::Placement(p) = &self.item {
            let ty = doc.component(&p.type_id)?;
            let u = self.u(doc, p)?;
            let width = p.width.unwrap_or(ty.width);
            if normal.is_none() && matches!(axis, Axis::U) {
                return Ok((u - width / 2.0, u + width / 2.0));
            }
            if normal.is_none() && matches!(axis, Axis::Z) {
                let b = doc.buildings.get(self.building).ok_or_else(|| invalid("missing building"))?;
                let z = b.storeys.get(p.level).ok_or_else(|| invalid("missing storey"))?.elevation + p.sill.unwrap_or(ty.sill);
                return Ok((z, z + p.height.unwrap_or(ty.height)));
            }
            let f = self.frame(doc)?;
            let n = normal.unwrap_or(if matches!(axis, Axis::X) { [1.0, 0.0] } else { [0.0, 1.0] });
            let a = f.point(u - width / 2.0, 0.0, 0.0);
            let b = f.point(u + width / 2.0, 0.0, 0.0);
            let (a, b) = (a[0] * n[0] + a[1] * n[1], b[0] * n[0] + b[1] * n[1]);
            return Ok((a.min(b), a.max(b)));
        }
        if normal.is_none() && matches!(axis, Axis::U | Axis::Z) {
            return Err(invalid("U and Z alignment require facade instances"));
        }
        let n = normal.unwrap_or(if matches!(axis, Axis::X) { [1.0, 0.0] } else { [0.0, 1.0] });
        let mut values = self.item.points().into_iter().map(|p| p[0] * n[0] + p[1] * n[1]);
        let first = values.next().ok_or_else(|| invalid("element has no outline"))?;
        Ok(values.fold((first, first), |(lo, hi), v| (lo.min(v), hi.max(v))))
    }
}

fn selected(doc: &Document, targets: &[String]) -> Result<Vec<Entry>> {
    if targets.len() > MAX_TARGETS {
        return Err(invalid("select at most 256 elements"));
    }
    let mut expanded = Vec::new();
    for target in targets {
        if let Some(id) = target.strip_prefix("group:") {
            let group = doc.groups.iter().find(|g| g.id == id).ok_or_else(|| invalid(format!("group {id} does not exist")))?;
            if group.targets.iter().any(|t| t.starts_with("group:")) {
                return Err(invalid("groups must contain elements, not nested group references"));
            }
            if expanded.len().saturating_add(group.targets.len()) > MAX_TARGETS {
                return Err(invalid("expanded groups exceed 256 elements"));
            }
            expanded.extend(group.targets.iter().cloned());
        } else {
            expanded.push(target.clone());
        }
        if expanded.len() > MAX_TARGETS {
            return Err(invalid("expanded groups exceed 256 elements"));
        }
    }
    let targets = &expanded;
    if targets.is_empty() || targets.len() > MAX_TARGETS || targets.iter().any(|s| s.is_empty() || s.len() > 512) {
        return Err(invalid("select 1 to 256 elements (ids at most 512 bytes)"));
    }
    let mut entries = Vec::new();
    let mut seen = BTreeSet::new();
    for target in targets.iter().collect::<BTreeSet<_>>() {
        let mut matches = Vec::new();
        for (building, b) in doc.buildings.iter().enumerate() {
            let items = b
                .interior_walls
                .iter()
                .cloned()
                .map(Item::Wall)
                .chain(b.stairs.iter().cloned().map(Item::Stair))
                .chain(b.rooms.iter().cloned().map(Item::Room));
            for original in items {
                let kind = match original {
                    Item::Wall(_) => "wall",
                    Item::Stair(_) => "stair",
                    Item::Room(_) => "room",
                    Item::Placement(_) => "placement",
                };
                let root = original.id();
                let all = target == root || target == &format!("{kind}:{root}");
                let level = target.strip_prefix(&format!("{root}.L")).and_then(|s| s.parse::<usize>().ok());
                if all || level.is_some_and(|l| original.levels().contains(&l)) {
                    let mut item = original.clone();
                    if !all && let Some(l) = level {
                        item.set_levels(vec![l]);
                    }
                    matches.push((format!("{building}:{kind}:{root}"), Entry { building, facade: None, original, item }));
                }
            }
            for f in &b.facades {
                let exact = target.strip_prefix(&format!("placement:{}:", f.id)).and_then(|s| s.parse::<usize>().ok());
                let native = target.strip_prefix(&format!("{}.L", f.id)).and_then(|s| s.split_once(".B")).and_then(|(l, rest)| {
                    let (bay, suffix) = rest.split_once('.').unwrap_or((rest, ""));
                    Some((l.parse::<usize>().ok()?, bay.parse::<usize>().ok()?, suffix.starts_with("balcony")))
                });
                for (i, p) in f.placements.iter().enumerate() {
                    let hit = exact == Some(i)
                        || native.is_some_and(|(l, bay, attachment)| {
                            let active = doc.lifetime(p).is_ok_and(|(c, d)| c <= doc.active_index() && doc.active_index() < d);
                            p.level == l && p.bay == bay && active && doc.component(&p.type_id).is_ok_and(|t| t.kind.is_opening() != attachment)
                        });
                    if hit {
                        matches.push((
                            format!("{building}:{}:{i}", f.id),
                            Entry { building, facade: Some(f.id.clone()), original: Item::Placement(p.clone()), item: Item::Placement(p.clone()) },
                        ));
                    }
                }
            }
        }
        if matches.len() != 1 {
            return Err(invalid(format!("{target}: select one editable wall, stair, room or facade instance ({} matches)", matches.len())));
        }
        let (key, entry) = matches.pop().ok_or_else(|| invalid("missing selection"))?;
        if !seen.insert(key) {
            return Err(invalid("select each source definition once; a definition and its storey instance overlap"));
        }
        entries.push(entry);
    }
    Ok(entries)
}

fn handle(doc: &Document, entry: &Entry) -> Result<String> {
    let item = &entry.item;
    if let Item::Placement(p) = item {
        let attachment = !doc.component(&p.type_id)?.kind.is_opening();
        return Ok(format!("{}.L{}.B{}{}", entry.facade.as_deref().unwrap_or(""), p.level, p.bay, if attachment { ".balcony" } else { "" }));
    }
    if item.id().is_empty() {
        return Err(invalid("groups require named source definitions"));
    }
    let kind = match item {
        Item::Wall(_) => "wall",
        Item::Stair(_) => "stair",
        Item::Room(_) => "room",
        Item::Placement(_) => "placement",
    };
    let levels = item.levels();
    Ok(if levels.len() == 1 { format!("{}.L{}", item.id(), levels.first().copied().unwrap_or(0)) } else { format!("{kind}:{}", item.id()) })
}

pub(crate) fn define_group(s: &mut Session, mut group: trigon_model::doc::ModelGroup) -> Result<Value> {
    if group.id.is_empty()
        || group.id.len() > 128
        || !group.id.chars().all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '-' | '_'))
        || group.name.trim().is_empty()
        || group.name.len() > 256
    {
        return Err(invalid("give a group id of 1–128 letters/digits/dots/dashes and a name of 1–256 bytes"));
    }
    let entries = selected(&s.doc, &group.targets)?;
    for e in &entries {
        if let Item::Placement(p) = &e.item {
            let (c, d) = s.doc.lifetime(p)?;
            if !(c <= s.doc.active_index() && s.doc.active_index() < d) {
                return Err(invalid("groups select active facade instances"));
            }
        }
    }
    group.targets = entries.iter().map(|e| handle(&s.doc, e)).collect::<Result<_>>()?;
    let id = group.id.clone();
    if let Some(old) = s.doc.groups.iter_mut().find(|g| g.id == id) {
        *old = group;
    } else {
        if s.doc.groups.len() >= 200 {
            return Err(invalid("at most 200 groups"));
        }
        s.doc.groups.push(group);
    }
    Ok(json!({"defined":id}))
}

pub(crate) fn list_groups(s: &Session) -> Value {
    json!({"groups":s.doc.groups.iter().map(|g| {
        let error=selected(&s.doc,&g.targets).err().map(|e|e.to_string());
        json!({"id":g.id,"name":g.name,"targets":g.targets,"valid":error.is_none(),"error":error})
    }).collect::<Vec<_>>()})
}

#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub(crate) struct GroupId {
    pub id: String,
}

pub(crate) fn remove_group(s: &mut Session, p: GroupId) -> Result<Value> {
    let i = s.doc.groups.iter().position(|g| g.id == p.id).ok_or_else(|| invalid(format!("group {} does not exist", p.id)))?;
    s.doc.groups.remove(i);
    Ok(json!({"removed":p.id}))
}

fn remap_groups(doc: &mut Document, entry: &Entry, next: &str, shared: bool) {
    if matches!(entry.item, Item::Placement(_)) {
        return;
    }
    let kind = match entry.item {
        Item::Wall(_) => "wall",
        Item::Stair(_) => "stair",
        Item::Room(_) => "room",
        Item::Placement(_) => "placement",
    };
    let all = format!("{kind}:{}", entry.original.id());
    let base = next.split_once(':').map(|(_, s)| s).or_else(|| next.rsplit_once(".L").map(|(s, _)| s)).unwrap_or(next);
    let levels = entry.item.levels();
    for group in &mut doc.groups {
        let mut add = false;
        for target in &mut group.targets {
            if *target == all {
                if shared {
                    add = true;
                } else {
                    *target = next.to_string();
                }
            } else if let Some(level) = target.strip_prefix(&format!("{}.L", entry.original.id())).and_then(|s| s.parse::<usize>().ok())
                && levels.contains(&level)
            {
                *target = format!("{base}.L{level}");
            }
        }
        if add {
            group.targets.push(next.to_string());
        }
        group.targets.sort();
        group.targets.dedup();
    }
}

/// Remove only the selected storeys. True means a shared definition remains behind.
fn remove(doc: &mut Document, entry: &Entry) -> Result<bool> {
    let b = doc.buildings.get_mut(entry.building).ok_or_else(|| invalid("missing building"))?;
    let levels = entry.item.levels();
    macro_rules! remove_levels {
        ($items:expr) => {{
            let i = $items.iter().position(|w| w.id == entry.original.id()).ok_or_else(|| invalid("source element changed"))?;
            let w = $items.get_mut(i).ok_or_else(|| invalid("missing source element"))?;
            w.levels.retain(|l| !levels.contains(l));
            let shared = !w.levels.is_empty();
            if !shared {
                $items.remove(i);
            }
            shared
        }};
    }
    Ok(match &entry.original {
        Item::Wall(_) => remove_levels!(b.interior_walls),
        Item::Stair(_) => remove_levels!(b.stairs),
        Item::Room(_) => remove_levels!(b.rooms),
        Item::Placement(original) => {
            let f = b.facade_mut(entry.facade.as_deref().ok_or_else(|| invalid("missing facade"))?)?;
            let i = f.placements.iter().position(|p| p == original).ok_or_else(|| invalid("source placement changed"))?;
            f.placements.remove(i);
            false
        }
    })
}

fn fresh_id(doc: &Document, base: &str, suffix: &str) -> Result<String> {
    for k in 1..=10_000 {
        let id = format!("{base}.{suffix}{k}");
        if id.len() > 512 {
            return Err(invalid("element id would exceed 512 bytes"));
        }
        let exists = doc
            .buildings
            .iter()
            .any(|b| b.interior_walls.iter().any(|w| w.id == id) || b.stairs.iter().any(|s| s.id == id) || b.rooms.iter().any(|r| r.id == id));
        if !exists {
            return Ok(id);
        }
    }
    Err(invalid("too many copies of this definition"))
}

fn put(doc: &mut Document, entry: &Entry, mut item: Item, new_id: bool, append_bay: bool, copy: bool) -> Result<String> {
    if new_id && !matches!(item, Item::Placement(_)) {
        item.set_id(fresh_id(doc, entry.original.id(), if copy { "copy" } else { "edit" })?);
    }
    let phase = doc.phases.get(doc.active_index()).filter(|_| doc.active_index() > 0).map(|p| p.id.clone());
    let placement_info = if let Item::Placement(p) = &item { Some((entry.u(doc, p)?, !doc.component(&p.type_id)?.kind.is_opening())) } else { None };
    let b = doc.buildings.get_mut(entry.building).ok_or_else(|| invalid("missing building"))?;
    if let Item::Placement(mut p) = item {
        let (u, attachment) = placement_info.ok_or_else(|| invalid("missing placement geometry"))?;
        let f = b.facade_mut(entry.facade.as_deref().ok_or_else(|| invalid("missing facade"))?)?;
        if append_bay {
            if f.bays.len() >= 200 {
                return Err(invalid("at most 200 bay axes per facade"));
            }
            p.bay = f.bays.len();
            f.bays.push(u);
        }
        if copy {
            p.created = phase;
            p.demolished = None;
        }
        let id = format!("{}.L{}.B{}{}", f.id, p.level, p.bay, if attachment { ".balcony" } else { "" });
        f.placements.push(p);
        return Ok(id);
    }
    let id = item.id().to_string();
    let levels = item.levels();
    let kind = match item {
        Item::Wall(w) => {
            w.validate(b.storeys.len())?;
            b.interior_walls.push(w);
            "wall"
        }
        Item::Stair(s) => {
            b.stairs.push(s);
            "stair"
        }
        Item::Room(r) => {
            b.rooms.push(r);
            "room"
        }
        Item::Placement(_) => return Err(invalid("unexpected placement")),
    };
    Ok(if levels.len() == 1 { format!("{id}.L{}", levels.first().copied().unwrap_or(0)) } else { format!("{kind}:{id}") })
}

fn checked_delta(delta: [f64; 3]) -> Result<()> {
    for v in delta {
        checked("delta", v, -100_000.0, 100_000.0)?;
    }
    Ok(())
}

fn finish(s: &Session, targets: Vec<String>) -> Result<Value> {
    trigon_model::build(&s.doc)?;
    Ok(json!({"changed":targets.len(),"targets":targets}))
}

pub(crate) fn move_elements(s: &mut Session, p: Move) -> Result<Value> {
    checked_delta(p.delta)?;
    let entries = selected(&s.doc, &p.targets)?;
    let changes: Vec<Item> = entries.iter().map(|e| e.shifted(&s.doc, p.delta)).collect::<Result<_>>()?;
    let mut ids = Vec::new();
    for (entry, item) in entries.iter().zip(changes) {
        let shared = remove(&mut s.doc, entry)?;
        let next = put(&mut s.doc, entry, item, shared, false, false)?;
        remap_groups(&mut s.doc, entry, &next, shared);
        ids.push(next);
    }
    finish(s, ids)
}

pub(crate) fn copy_elements(s: &mut Session, p: Copy) -> Result<Value> {
    checked_delta(p.delta)?;
    let entries = selected(&s.doc, &p.targets)?;
    if p.count == 0 || p.count > 100 || entries.len().saturating_mul(p.count) > MAX_COPIES {
        return Err(invalid("give 1 to 100 copies, at most 5000 new elements"));
    }
    if p.delta.iter().all(|v| v.abs() < 1e-9) {
        return Err(invalid("give a nonzero copy offset"));
    }
    let mut copies = Vec::new();
    for entry in &entries {
        for k in 1..=p.count {
            let d = p.delta.map(|v| v * k as f64);
            copies.push((entry, entry.shifted(&s.doc, d)?));
        }
    }
    let mut ids = Vec::new();
    for (entry, item) in copies {
        ids.push(put(&mut s.doc, entry, item, true, true, true)?);
    }
    finish(s, ids)
}

fn matching(doc: &Document, source: &Entry, item: &Item, level: usize) -> Result<Vec<Entry>> {
    let b = doc.buildings.get(source.building).ok_or_else(|| invalid("missing building"))?;
    let candidates: Vec<Item> = match item {
        Item::Wall(_) => b.interior_walls.iter().cloned().map(Item::Wall).collect(),
        Item::Stair(_) => b.stairs.iter().cloned().map(Item::Stair).collect(),
        Item::Room(_) => b.rooms.iter().cloned().map(Item::Room).collect(),
        Item::Placement(_) => {
            b.facade(source.facade.as_deref().ok_or_else(|| invalid("missing facade"))?)?.placements.iter().cloned().map(Item::Placement).collect()
        }
    };
    let mut out = Vec::new();
    for original in candidates {
        if !original.levels().contains(&level) {
            continue;
        }
        let equal = match (&original, item) {
            (Item::Placement(a), Item::Placement(b)) => {
                let active = doc.lifetime(a).is_ok_and(|(c, d)| c <= doc.active_index() && doc.active_index() < d);
                active && a.bay == b.bay && doc.component(&a.type_id)?.kind.is_opening() == doc.component(&b.type_id)?.kind.is_opening()
            }
            _ => {
                let mut a = original.clone();
                let mut b = item.clone();
                a.set_id(String::new());
                b.set_id(String::new());
                a.set_levels(Vec::new());
                b.set_levels(Vec::new());
                a == b
            }
        };
        if equal {
            let mut selected = original.clone();
            selected.set_levels(vec![level]);
            out.push(Entry { building: source.building, facade: source.facade.clone(), original, item: selected });
        }
    }
    Ok(out)
}

pub(crate) fn copy_to_levels(s: &mut Session, mut p: CopyToLevels) -> Result<Value> {
    let entries = selected(&s.doc, &p.targets)?;
    if p.levels.is_empty() || p.levels.len() > 60 {
        return Err(invalid("give 1 to 60 destination storeys"));
    }
    p.levels.sort_unstable();
    p.levels.dedup();
    if entries.len().saturating_mul(p.levels.len()) > MAX_COPIES {
        return Err(invalid("at most 5000 new elements"));
    }
    let mut ids = Vec::new();
    for entry in entries {
        let n = s.doc.buildings.get(entry.building).map_or(0, |b| b.storeys.len());
        for &level in &p.levels {
            if level >= n {
                return Err(invalid(format!("storey {level} does not exist")));
            }
            if entry.item.levels().contains(&level) {
                continue;
            }
            let mut item = entry.item.clone();
            item.set_levels(vec![level]);
            let matches = matching(&s.doc, &entry, &item, level)?;
            if !p.replace && !matches.is_empty() {
                return Err(invalid(format!("storey {level} already contains this geometry or facade slot; use replace to copy over it")));
            }
            let mut replacements = Vec::new();
            for m in matches {
                // Replacing an older placement ends its life in the active phase, just as
                // facade.place does. Earlier phases must retain their original opening.
                let phase = s.doc.active_index();
                let shared = if let Item::Placement(original) = &m.original
                    && s.doc.lifetime(original)?.0 < phase
                {
                    let demolished = s.doc.phases.get(phase).map(|p| p.id.clone());
                    let b = s.doc.buildings.get_mut(m.building).ok_or_else(|| invalid("missing building"))?;
                    let f = b.facade_mut(m.facade.as_deref().ok_or_else(|| invalid("missing facade"))?)?;
                    let placement = f.placements.iter_mut().find(|p| *p == original).ok_or_else(|| invalid("source placement changed"))?;
                    placement.demolished = demolished;
                    false
                } else {
                    remove(&mut s.doc, &m)?
                };
                replacements.push((m, shared));
            }
            let next = put(&mut s.doc, &entry, item, true, false, true)?;
            for (replaced, shared) in replacements {
                remap_groups(&mut s.doc, &replaced, &next, shared);
            }
            ids.push(next);
        }
    }
    finish(s, ids)
}

pub(crate) fn delete(s: &mut Session, p: Selection) -> Result<Value> {
    let entries = selected(&s.doc, &p.targets)?;
    let deleted = entries.len();
    for e in entries {
        remove(&mut s.doc, &e)?;
    }
    let valid: Vec<Vec<String>> =
        s.doc.groups.iter().map(|g| g.targets.iter().filter(|t| selected(&s.doc, std::slice::from_ref(*t)).is_ok()).cloned().collect()).collect();
    for (group, targets) in s.doc.groups.iter_mut().zip(valid) {
        group.targets = targets;
    }
    s.doc.groups.retain(|g| !g.targets.is_empty());
    for b in &mut s.doc.buildings {
        for entrance in &mut b.entrances {
            if entrance.stair.as_ref().is_some_and(|id| !b.stairs.iter().any(|s| &s.id == id)) {
                entrance.stair = None;
            }
        }
    }
    trigon_model::build(&s.doc)?;
    Ok(json!({"deleted":deleted}))
}

fn anchor_value((lo, hi): (f64, f64), anchor: Anchor) -> f64 {
    match anchor {
        Anchor::Start => lo,
        Anchor::Center => (lo + hi) / 2.0,
        Anchor::End => hi,
    }
}

pub(crate) fn align(s: &mut Session, p: Align) -> Result<Value> {
    let entries = selected(&s.doc, &p.targets)?;
    let (axis, normal, coordinate) = if let Some([a, b]) = p.line {
        if p.axis.is_some() || p.coordinate.is_some() || p.reference.is_some() {
            return Err(invalid("give a plan line, or an axis with a coordinate/reference"));
        }
        for v in a.into_iter().chain(b) {
            checked("line coordinate", v, -1e8, 1e8)?;
        }
        let length = (b[0] - a[0]).hypot(b[1] - a[1]);
        checked("alignment line length", length, 1.0, 1e8)?;
        let n = [-(b[1] - a[1]) / length, (b[0] - a[0]) / length];
        (Axis::X, Some(n), a[0] * n[0] + a[1] * n[1])
    } else {
        let axis = p.axis.ok_or_else(|| invalid("give an alignment axis"))?;
        let coordinate = match (p.coordinate, p.reference.as_ref()) {
            (Some(v), None) => checked("alignment coordinate", v, -1e8, 1e8)?,
            (None, Some(id)) => {
                let reference = selected(&s.doc, std::slice::from_ref(id))?.pop().ok_or_else(|| invalid("missing reference"))?;
                if matches!(axis, Axis::U) && entries.iter().any(|e| e.building != reference.building || e.facade != reference.facade) {
                    return Err(invalid("U alignment needs a reference on the same facade"));
                }
                anchor_value(reference.extent(&s.doc, axis, None)?, p.anchor)
            }
            _ => return Err(invalid("give one alignment coordinate or reference")),
        };
        (axis, None, coordinate)
    };
    let mut changes = Vec::new();
    for entry in &entries {
        let distance = coordinate - anchor_value(entry.extent(&s.doc, axis, normal)?, p.anchor);
        let delta = if normal.is_none() && matches!(axis, Axis::Z) {
            [0.0, 0.0, distance]
        } else if normal.is_none() && matches!(axis, Axis::U) {
            let f = entry.frame(&s.doc)?;
            [f.t[0] * distance, f.t[1] * distance, 0.0]
        } else {
            let n = normal.unwrap_or(if matches!(axis, Axis::X) { [1.0, 0.0] } else { [0.0, 1.0] });
            if matches!(entry.item, Item::Placement(_)) {
                let f = entry.frame(&s.doc)?;
                let projection = f.t[0] * n[0] + f.t[1] * n[1];
                if projection.abs() < 1e-6 {
                    return Err(invalid("this facade is parallel to the alignment line; its instances cannot move across the wall"));
                }
                [f.t[0] * distance / projection, f.t[1] * distance / projection, 0.0]
            } else {
                [n[0] * distance, n[1] * distance, 0.0]
            }
        };
        checked_delta(delta)?;
        changes.push(entry.shifted(&s.doc, delta)?);
    }
    let mut ids = Vec::new();
    for (entry, item) in entries.iter().zip(changes) {
        let shared = remove(&mut s.doc, entry)?;
        let next = put(&mut s.doc, entry, item, shared, false, false)?;
        remap_groups(&mut s.doc, entry, &next, shared);
        ids.push(next);
    }
    finish(s, ids)
}
