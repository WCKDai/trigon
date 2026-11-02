//! Local entrance/register editing. These are unphased source records, not inferred doors.
//! `<building>.entrance<index>` locates the current document's entry; provider replacement can
//! reorder entries. Writes therefore check the exact expected address and previous value.

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use trigon_model::{Building, Document, Entrance, ModelError, Unit, checked};

use crate::{Result, Session};

#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub(crate) struct List {
    pub building: Option<String>,
}

#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub(crate) struct Id {
    pub id: String,
}

#[derive(Clone, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub(crate) struct ExpectedAddress {
    pub street: String,
    pub number: String,
    pub point_mm: [f64; 2],
}

/// Same JSON fields as the native Unit, with strict parsing for this write interface.
#[derive(Clone, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub(crate) struct UnitRecord {
    /// Native floor designation: `kl`, `st`, or a non-negative storey number.
    pub floor: String,
    /// Actual source designation; empty means unknown. No TV/TH side is inferred.
    pub door: String,
    #[serde(default)]
    pub area_m2: Option<f64>,
    #[serde(default)]
    pub rooms: Option<i64>,
}

impl UnitRecord {
    fn unit(&self) -> Unit {
        Unit { floor: self.floor.clone(), door: self.door.clone(), area_m2: self.area_m2, rooms: self.rooms }
    }
}

#[derive(Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub(crate) struct SetUnit {
    pub id: String,
    pub expected_address: ExpectedAddress,
    pub unit_index: usize,
    pub expected_unit: UnitRecord,
    pub unit: UnitRecord,
}

#[derive(Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub(crate) struct BindStair {
    pub id: String,
    pub expected_address: ExpectedAddress,
    /// None edits the default. Some edits only that departure-storey override.
    pub level: Option<usize>,
    /// Expected STORED slot, not the effective inherited stair. Missing means expect no slot.
    pub expected_stair: Option<String>,
    /// None clears that slot. Clearing an override resumes the unchanged default.
    pub stair: Option<String>,
}

fn invalid(message: impl Into<String>) -> crate::EngineError {
    ModelError::Invalid(message.into()).into()
}

fn locator(building: &str, index: usize) -> String {
    format!("{building}.entrance{index}")
}

fn locate(doc: &Document, id: &str) -> Result<(usize, usize)> {
    if id.len() > 256 {
        return Err(invalid("entrance locator is too long"));
    }
    let (building, index) = id.rsplit_once(".entrance").ok_or_else(|| invalid("use <building>.entrance<zero-based-index>"))?;
    if building.is_empty() || index.is_empty() || !index.bytes().all(|c| c.is_ascii_digit()) {
        return Err(invalid("use <building>.entrance<zero-based-index>"));
    }
    let index = index.parse::<usize>().map_err(|_| invalid("entrance index is outside the supported range"))?;
    let mut matches = doc.buildings.iter().enumerate().filter(|(_, b)| b.id == building);
    let (bi, b) = matches.next().ok_or_else(|| invalid("entrance building was not found"))?;
    if matches.next().is_some() {
        return Err(invalid("entrance building ID is ambiguous"));
    }
    if b.entrances.get(index).is_none() {
        return Err(invalid("entrance index was not found; inspect entrance.list again"));
    }
    Ok((bi, index))
}

fn address(e: &Entrance) -> Value {
    json!({"street":e.street,"number":e.number,"point_mm":e.point})
}

fn check_address(e: &Entrance, expected: &ExpectedAddress) -> Result<()> {
    for p in e.point.into_iter().chain(expected.point_mm) {
        checked("entrance point in site mm", p, -1e8, 1e8)?;
    }
    if e.street != expected.street || e.number != expected.number || e.point != expected.point_mm {
        return Err(invalid("entrance address or access point changed; inspect entrance.get again before editing"));
    }
    Ok(())
}

fn text(what: &str, value: &str, max: usize, empty: bool) -> Result<()> {
    if (!empty && value.is_empty()) || value.chars().count() > max || value.chars().any(char::is_control) {
        return Err(invalid(format!("{what} needs {}..{max} characters without control characters", usize::from(!empty))));
    }
    Ok(())
}

fn validate_unit(b: &Building, unit: &Unit) -> Result<usize> {
    text("unit floor", &unit.floor, 32, false)?;
    text("unit door designation", &unit.door, 64, true)?;
    let level = unit.level(b.ground_level).ok_or_else(|| invalid("unit floor must be kl, st, or a non-negative floor number without overflow"))?;
    if level >= b.storeys.len() {
        return Err(invalid("unit floor does not name a storey in this building"));
    }
    if let Some(area) = unit.area_m2 {
        checked("unit area in m2", area, 0.0, 1e9)?;
    }
    if unit.rooms.is_some_and(|rooms| !(0..=10000).contains(&rooms)) {
        return Err(invalid("unit rooms must be 0..10000, or null when unknown"));
    }
    Ok(level)
}

fn unknown_designation(door: &str) -> bool {
    Unit::is_unknown_door(door)
}

fn designation(unit: &Unit, ground: usize) -> Option<(usize, String)> {
    if unknown_designation(&unit.door) { None } else { unit.level(ground).map(|level| (level, unit.door.trim().to_lowercase())) }
}

fn stair_status(b: &Building, id: Option<&str>, departure: usize, point: [f64; 2]) -> Value {
    let Some(id) = id else { return json!({"stair":null,"valid":false,"reason":"no stored source"}) };
    let mut sources = b.stairs.iter().filter(|st| st.id == id);
    let Some(st) = sources.next() else { return json!({"stair":id,"valid":false,"reason":"source missing in this building"}) };
    if sources.next().is_some() {
        return json!({"stair":id,"valid":false,"reason":"source ID is ambiguous"});
    }
    if departure >= b.storeys.len() || !st.levels.contains(&departure) {
        return json!({"stair":id,"valid":false,"reason":"source does not depart on this storey"});
    }
    let Ok(frame) = trigon_model::stair::frame(st) else {
        return json!({"stair":id,"valid":false,"reason":"source stair frame is invalid"});
    };
    let centre = frame.at(frame.length / 2.0, frame.width / 2.0);
    json!({"stair":id,"valid":true,"departure_level":departure,"distance_mm":(centre[0]-point[0]).hypot(centre[1]-point[1]),
        "association_geometry_verified":false})
}

fn descriptor(b: &Building, index: usize) -> Result<Value> {
    let e = b.entrances.get(index).ok_or_else(|| invalid("entrance index was not found"))?;
    for p in e.point {
        checked("entrance point in site mm", p, -1e8, 1e8)?;
    }
    let bindings: Vec<_> = (0..b.storeys.len())
        .map(|level| {
            let stored = e.stair_by_level.get(&level);
            let effective = stair_status(b, e.stair_at(level), level, e.point);
            let drawing_source = [Some(level), level.checked_sub(1)].into_iter().flatten().find_map(|departure| {
                let status = stair_status(b, e.stair_at(departure), departure, e.point);
                status["valid"].as_bool().unwrap_or(false).then(|| {
                    json!({"stair":status["stair"],"departure_level":departure,
                "arrival":departure!=level})
                })
            });
            json!({"level":level,"name":b.storeys[level].name,"stored_override":stored,"effective_departure":effective,
            "drawing_source":drawing_source})
        })
        .collect();
    let units: Vec<_> = e
        .units
        .iter()
        .enumerate()
        .map(|(index, unit)| {
            json!({"index":index,"unit":unit,
        "level":unit.level(b.ground_level).filter(|&level|level<b.storeys.len()),"unknown_designation":unknown_designation(&unit.door)})
        })
        .collect();
    let available: Vec<_> = b
        .stairs
        .iter()
        .map(|st| {
            json!({"id":st.id,"departure_levels":st.levels,
        "can_bind_default":stair_status(b,Some(&st.id),b.ground_level,e.point)["valid"]})
        })
        .collect();
    Ok(json!({"id":locator(&b.id,index),"building":b.id,"index":index,"expected_address":address(e),"units":units,
        "default_stair":e.stair,"default_binding":stair_status(b,e.stair.as_deref(),b.ground_level,e.point),
        "stair_by_level":e.stair_by_level,"bindings":bindings,"available_source_stairs":available,"unphased":true,
        "invalid_override_levels":e.stair_by_level.keys().copied().filter(|&level|level>=b.storeys.len()).collect::<Vec<_>>(),
        "unknown_designations":e.units.iter().filter(|u|unknown_designation(&u.door)).count(),
        "unmapped_units":e.units.iter().enumerate().filter_map(|(i,u)|u.level(b.ground_level).filter(|&l|l<b.storeys.len()).is_none().then_some(i)).collect::<Vec<_>>(),
        "locator_scope":"current-document entry index; provider replacement can reorder entries",
        "source_provenance":"stored unit records have no acquisition or phase field; do not infer historic layout or missing sides",
        "unknown_designation_semantics":"blank, ?, unknown, ukendt, uoplyst and ikke oplyst remain unknown; source text is preserved verbatim",
        "binding_semantics":"level=null edits default; level=N edits its stored override. Clearing an override resumes the default. A valid source reference is not proof of its physical association."}))
}

pub(crate) fn list(s: &Session, p: List) -> Result<Value> {
    if let Some(id) = &p.building {
        let count = s.doc.buildings.iter().filter(|b| &b.id == id).count();
        if count != 1 {
            return Err(invalid("entrance.list needs one unambiguous building"));
        }
    }
    let entries: Vec<_> = s.doc.buildings.iter().filter(|b|p.building.as_ref().is_none_or(|id|id==&b.id)).flat_map(|b| {
        b.entrances.iter().enumerate().map(|(index,e)|json!({"id":locator(&b.id,index),"building":b.id,"index":index,
            "expected_address":address(e),"units":e.units.len(),"unknown_designations":e.units.iter().filter(|u|unknown_designation(&u.door)).count()}))
    }).collect();
    Ok(json!({"entrances":entries,"unphased":true,"locator_scope":"current-document entry indices; verify expected_address before editing"}))
}

pub(crate) fn get(s: &Session, p: Id) -> Result<Value> {
    let (bi, index) = locate(&s.doc, &p.id)?;
    descriptor(&s.doc.buildings[bi], index)
}

pub(crate) fn set_unit(s: &mut Session, p: SetUnit) -> Result<Value> {
    let (bi, index) = locate(&s.doc, &p.id)?;
    let b = &s.doc.buildings[bi];
    let e = &b.entrances[index];
    check_address(e, &p.expected_address)?;
    let before = e.units.get(p.unit_index).ok_or_else(|| invalid("unit index was not found; inspect entrance.get again"))?;
    if *before != p.expected_unit.unit() {
        return Err(invalid("unit record changed; inspect entrance.get again before editing"));
    }
    let unit = p.unit.unit();
    let level = validate_unit(b, &unit)?;
    if *before == unit {
        return Err(invalid("unit edit makes no source change"));
    }
    let key = designation(&unit, b.ground_level);
    if key.is_some()
        && key != designation(before, b.ground_level)
        && e.units.iter().enumerate().any(|(i, u)| i != p.unit_index && designation(u, b.ground_level) == key)
    {
        return Err(invalid("unit edit introduces a duplicate known door designation on this storey"));
    }
    let affected_levels: Vec<_> = before
        .level(b.ground_level)
        .filter(|&l| l < b.storeys.len())
        .into_iter()
        .chain([level])
        .collect::<std::collections::BTreeSet<_>>()
        .into_iter()
        .collect();
    let before = before.clone();
    s.doc.buildings[bi].entrances[index].units[p.unit_index] = unit.clone();
    Ok(json!({"id":locator(&s.doc.buildings[bi].id,index),"unit_index":p.unit_index,"before":before,"unit":unit,
        "affected_levels":affected_levels,"unknown_designation":unknown_designation(&unit.door),"unphased":true,"geometry_changed":false}))
}

pub(crate) fn bind_stair(s: &mut Session, p: BindStair) -> Result<Value> {
    let (bi, index) = locate(&s.doc, &p.id)?;
    let b = &s.doc.buildings[bi];
    let e = &b.entrances[index];
    check_address(e, &p.expected_address)?;
    if p.level.is_some_and(|level| level >= b.storeys.len()) {
        return Err(invalid("entrance binding level is outside this building"));
    }
    let before = match p.level {
        Some(level) => e.stair_by_level.get(&level),
        None => e.stair.as_ref(),
    };
    if before != p.expected_stair.as_ref() {
        return Err(invalid("stored stair binding changed; inspect entrance.get again"));
    }
    if before == p.stair.as_ref() {
        return Err(invalid("stair binding edit makes no source change"));
    }
    let departure = p.level.unwrap_or(b.ground_level);
    let status = if let Some(id) = &p.stair {
        text("stair source ID", id, 256, false)?;
        let status = stair_status(b, Some(id), departure, e.point);
        if !status["valid"].as_bool().unwrap_or(false) {
            return Err(invalid(format!("cannot bind stair: {}", status["reason"].as_str().unwrap_or("source is invalid"))));
        }
        status
    } else {
        Value::Null
    };
    let affected_levels: Vec<_> = p.level.map_or_else(
        || (0..b.storeys.len()).collect(),
        |level| [Some(level), level.checked_add(1).filter(|&arrival| arrival < b.storeys.len())].into_iter().flatten().collect(),
    );
    let before = before.cloned();
    let e = &mut s.doc.buildings[bi].entrances[index];
    match p.level {
        Some(level) => match &p.stair {
            Some(id) => {
                e.stair_by_level.insert(level, id.clone());
            }
            None => {
                e.stair_by_level.remove(&level);
            }
        },
        None => e.stair.clone_from(&p.stair),
    }
    Ok(json!({"id":locator(&s.doc.buildings[bi].id,index),"departure_level":p.level,"before_stored_stair":before,
        "stored_stair":p.stair,"validated_source":status,"affected_levels":affected_levels,
        "unphased":true,"geometry_changed":false,"cleared_override_uses_default":p.level.is_some()&&p.stair.is_none()}))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn session() -> Session {
        let mut s = Session::new(Box::new(trigon_data::Live::new(Default::default())));
        let mut b = Building::new("b", "Local entrance fixture", &[[0., 0.], [12000., 0.], [12000., 9000.], [0., 9000.]], 3, 3000.).unwrap();
        b.ground_level = 1;
        b.storeys[0].name = "Basement".into();
        b.storeys[1].name = "Ground".into();
        b.storeys[2].name = "First".into();
        for (id, levels, x) in [("core", vec![1], 1000.), ("basement", vec![0], 6500.)] {
            b.stairs.push(serde_json::from_value(json!({"id":id,"levels":levels,"outline":[[x,1000.],[x+2500.,1000.],[x+2500.,4500.],[x,4500.]],"kind":"u_turn","landing":1000.,"rounded":false})).unwrap());
        }
        b.entrances.push(Entrance {
            street: "Sample Street".into(),
            number: "1A".into(),
            point: [1250.125, -200.25],
            stair: Some("core".into()),
            stair_by_level: Default::default(),
            units: vec![
                Unit { floor: "st".into(), door: "tv".into(), area_m2: Some(55.25), rooms: Some(2) },
                Unit { floor: "st".into(), door: String::new(), area_m2: Some(61.), rooms: Some(3) },
                Unit { floor: "1".into(), door: "2".into(), area_m2: None, rooms: None },
                Unit { floor: "kl".into(), door: "mf".into(), area_m2: None, rooms: None },
            ],
        });
        let mut other = b.entrances[0].clone();
        other.number = "3".into();
        other.point = [8000., -200.25];
        b.entrances.push(other);
        s.doc.buildings.push(b);
        s
    }

    fn bytes(s: &Session) -> Vec<u8> {
        serde_json::to_vec(&s.doc).unwrap()
    }
    fn info(s: &mut Session) -> Value {
        s.execute("entrance.get", json!({"id":"b.entrance0"})).unwrap()
    }
    fn edit(s: &mut Session) -> Value {
        let data = info(s);
        let before = data["units"][1]["unit"].clone();
        let mut unit = before.clone();
        unit["door"] = json!("th");
        json!({"id":"b.entrance0","expected_address":data["expected_address"],"unit_index":1,"expected_unit":before,"unit":unit})
    }

    #[test]
    fn local_entrance_inspection_is_exact_read_only_and_discoverable() {
        let mut s = session();
        let before = bytes(&s);
        let out = info(&mut s);
        assert_eq!(out["expected_address"]["point_mm"], json!([1250.125, -200.25]));
        assert_eq!(out["units"][0]["level"], 1);
        assert_eq!(out["units"][2]["level"], 2);
        assert_eq!(out["units"][3]["level"], 0);
        assert_eq!(out["units"][1]["unknown_designation"], true);
        assert_eq!(out["unknown_designations"], 1);
        assert_eq!(out["unphased"], true);
        assert_eq!(out["default_binding"]["valid"], true);
        assert_eq!(out["bindings"][2]["drawing_source"]["departure_level"], 1);
        assert_eq!(out["bindings"][2]["drawing_source"]["arrival"], true);
        let all = s.execute("entrance.list", json!({"building":"b"})).unwrap();
        assert_eq!(all["entrances"].as_array().unwrap().len(), 2);
        assert_eq!(all["entrances"][1]["id"], "b.entrance1");
        assert_eq!(bytes(&s), before);
        assert!(s.undo.is_empty() && s.redo.is_empty());
        for id in ["entrance.list", "entrance.get", "entrance.set_unit", "entrance.bind_stair"] {
            let spec = crate::commands().iter().find(|c| c.id == id).unwrap();
            assert_eq!(spec.params_schema()["additionalProperties"], false);
            assert_eq!(spec.mutates, id == "entrance.set_unit" || id == "entrance.bind_stair");
            assert_eq!(spec.journaled, spec.mutates);
        }
        let raw = serde_json::to_value(&s.doc.buildings[0].entrances[0]).unwrap();
        assert!(raw.get("id").is_none());
        assert!(raw.get("stair_by_level").is_none());
        let legacy = json!({"street":"Sample Street","number":"1A","point":[1250.125,-200.25],"stair":"core","units":[{"floor":"st","door":"","area_m2":null,"rooms":null}]});
        let restored: Entrance = serde_json::from_value(legacy.clone()).unwrap();
        assert_eq!(serde_json::to_value(restored).unwrap(), legacy, "the inspector must not add native IDs, phases or provenance fields");
    }

    #[test]
    fn local_unit_edit_preserves_geometry_other_records_exact_undo_redo_and_native_reopen() {
        let mut s = session();
        let before = bytes(&s);
        let glb = trigon_io::glb::write(&trigon_model::build(&s.doc).unwrap(), &[]);
        let p = edit(&mut s);
        let out = s.execute("entrance.set_unit", p).unwrap();
        assert_eq!(out["unit"]["door"], "th");
        assert_eq!(out["affected_levels"], json!([1]));
        assert_eq!(out["geometry_changed"], false);
        let after = bytes(&s);
        let mut expected: Document = serde_json::from_slice(&before).unwrap();
        expected.buildings[0].entrances[0].units[1].door = "th".into();
        expected.journal = s.doc.journal.clone();
        assert_eq!(s.doc, expected);
        assert_eq!(trigon_io::glb::write(&trigon_model::build(&s.doc).unwrap(), &[]), glb);
        s.execute("edit.undo", json!({})).unwrap();
        assert_eq!(bytes(&s), before);
        assert_eq!(trigon_io::glb::write(&trigon_model::build(&s.doc).unwrap(), &[]), glb);
        s.execute("edit.redo", json!({})).unwrap();
        assert_eq!(bytes(&s), after);
        let path = std::env::temp_dir().join(format!("trigon-local-entrance-{}.trigon", std::process::id()));
        s.execute("document.save", json!({"path":path})).unwrap();
        let mut reopened = Session::new(Box::new(trigon_data::Live::new(Default::default())));
        reopened.execute("document.open", json!({"path":path})).unwrap();
        assert_eq!(bytes(&reopened), after);
        std::fs::remove_file(path).unwrap();
    }

    #[test]
    fn stale_or_hostile_unit_edits_preserve_document_history_and_redo() {
        let mut s = session();
        let good = edit(&mut s);
        s.execute("entrance.set_unit", good.clone()).unwrap();
        s.execute("edit.undo", json!({})).unwrap();
        let before = bytes(&s);
        let history = (s.undo.len(), s.redo.len());
        let mut bad = Vec::new();
        for (pointer, value) in [
            ("/expected_address/number", json!("changed")),
            ("/expected_address/point_mm/0", json!(1250.126)),
            ("/expected_unit/rooms", json!(4)),
            ("/id", json!("b.entrance99")),
            ("/unit_index", json!(usize::MAX)),
            ("/unit/floor", json!(usize::MAX.to_string())),
            ("/unit/floor", json!("unknown")),
            ("/unit/floor", json!("99")),
            ("/unit/door", json!("tv")),
            ("/unit/door", json!("line\nbreak")),
            ("/unit/door", json!("x".repeat(65))),
            ("/unit/area_m2", json!(-1.)),
            ("/unit/area_m2", json!(1e300)),
            ("/unit/rooms", json!(-1)),
            ("/unit/rooms", json!(10001)),
        ] {
            let mut p = good.clone();
            *p.pointer_mut(pointer).unwrap() = value;
            bad.push(p);
        }
        let mut extra = good.clone();
        extra["unit"]["unexpected"] = json!(true);
        bad.push(extra);
        for p in bad {
            assert!(s.execute("entrance.set_unit", p.clone()).is_err(), "{p}");
            assert_eq!(bytes(&s), before);
            assert_eq!((s.undo.len(), s.redo.len()), history);
        }
        for value in [f64::NAN, f64::INFINITY, f64::NEG_INFINITY] {
            let mut p: SetUnit = serde_json::from_value(good.clone()).unwrap();
            p.unit.area_m2 = Some(value);
            assert!(set_unit(&mut s, p).is_err());
            assert_eq!(bytes(&s), before);
        }
        s.execute("edit.redo", json!({})).unwrap();
        assert_eq!(s.doc.buildings[0].entrances[0].units[1].door, "th");
    }

    #[test]
    fn index_locator_address_guard_refuses_provider_reordering() {
        let mut s = session();
        let p = edit(&mut s);
        s.doc.buildings[0].entrances.swap(0, 1);
        let before = bytes(&s);
        assert!(s.execute("entrance.set_unit", p).is_err());
        assert_eq!(bytes(&s), before);
    }

    #[test]
    fn unknown_labels_and_existing_duplicates_are_preserved_without_inventing_sides() {
        let mut s = session();
        for marker in ["", "?", "Ukendt", "unknown", "uoplyst", "ikke oplyst"] {
            s.doc.buildings[0].entrances[0].units[1].door = marker.into();
            let data = info(&mut s);
            assert_eq!(data["units"][1]["unit"]["door"], marker);
            assert_eq!(data["units"][1]["unknown_designation"], true);
            let old = data["units"][1]["unit"].clone();
            let mut unit = old.clone();
            unit["area_m2"] = json!(62.);
            let result = s
                .execute(
                    "entrance.set_unit",
                    json!({"id":"b.entrance0","expected_address":data["expected_address"],"unit_index":1,"expected_unit":old,"unit":unit}),
                )
                .unwrap();
            assert_eq!(result["unit"]["door"], marker);
            assert_eq!(result["unknown_designation"], true);
            s.execute("edit.undo", json!({})).unwrap();
        }
        s.doc.buildings[0].entrances[0].units[1].door = "tv".into();
        let data = info(&mut s);
        let old = data["units"][1]["unit"].clone();
        let mut unit = old.clone();
        unit["rooms"] = json!(4);
        s.execute(
            "entrance.set_unit",
            json!({"id":"b.entrance0","expected_address":data["expected_address"],"unit_index":1,"expected_unit":old,"unit":unit}),
        )
        .unwrap();
        assert_eq!(
            s.doc.buildings[0].entrances[0].units.iter().filter(|u| u.floor == "st" && u.door == "tv").count(),
            2,
            "a quantity edit must not rewrite pre-existing duplicate labels"
        );
        s.execute("edit.undo", json!({})).unwrap();
        let mut p = edit(&mut s);
        p["unit"]["floor"] = json!("1");
        let result = s.execute("entrance.set_unit", p).unwrap();
        assert_eq!(result["affected_levels"], json!([1, 2]));
    }

    #[test]
    fn explicit_stair_bindings_validate_departures_and_clearing_uses_the_default() {
        let mut s = session();
        let old = bytes(&s);
        let address = info(&mut s)["expected_address"].clone();
        let base = json!({"id":"b.entrance0","expected_address":address,"level":null,"expected_stair":"core","stair":"basement"});
        assert!(s.execute("entrance.bind_stair", base).is_err(), "basement-only source cannot become the default ground-floor association");
        assert_eq!(bytes(&s), old);
        let bind = json!({"id":"b.entrance0","expected_address":address,"level":0,"expected_stair":null,"stair":"basement"});
        s.execute("entrance.bind_stair", bind).unwrap();
        let bound = bytes(&s);
        let out = info(&mut s);
        assert_eq!(out["default_stair"], "core");
        assert_eq!(out["bindings"][0]["stored_override"], "basement");
        assert_eq!(out["bindings"][0]["effective_departure"]["valid"], true);
        for (level, source) in [(2, "core"), (999, "core"), (0, "missing")] {
            let expected = if level == 0 { json!("basement") } else { Value::Null };
            assert!(
                s.execute(
                    "entrance.bind_stair",
                    json!({"id":"b.entrance0","expected_address":address,"level":level,"expected_stair":expected,"stair":source})
                )
                .is_err()
            );
            assert_eq!(bytes(&s), bound);
        }
        s.execute("entrance.bind_stair", json!({"id":"b.entrance0","expected_address":address,"level":0,"expected_stair":"basement","stair":null}))
            .unwrap();
        assert!(s.doc.buildings[0].entrances[0].stair_by_level.is_empty());
        assert_eq!(s.doc.buildings[0].entrances[0].stair_at(0), Some("core"));
        s.execute("edit.undo", json!({})).unwrap();
        assert_eq!(bytes(&s), bound);
        s.execute("edit.undo", json!({})).unwrap();
        assert_eq!(bytes(&s), old);
        s.execute("edit.redo", json!({})).unwrap();
        assert_eq!(bytes(&s), bound);
    }

    #[test]
    fn invalid_stair_sources_and_stale_bindings_preserve_redo_and_exact_geometry() {
        let mut s = session();
        let mut other = s.doc.buildings[0].clone();
        other.id = "other".into();
        other.stairs[0].id = "other.core".into();
        s.doc.buildings.push(other);
        let data = info(&mut s);
        let p = json!({"id":"b.entrance0","expected_address":data["expected_address"],"level":0,"expected_stair":null,"stair":"basement"});
        s.execute("entrance.bind_stair", p.clone()).unwrap();
        s.execute("edit.undo", json!({})).unwrap();
        let before = bytes(&s);
        let history = (s.undo.len(), s.redo.len());
        for source in ["other.core", "missing", "", "core\n"] {
            let mut bad = p.clone();
            bad["stair"] = json!(source);
            assert!(s.execute("entrance.bind_stair", bad).is_err());
            assert_eq!(bytes(&s), before);
            assert_eq!((s.undo.len(), s.redo.len()), history);
        }
        let mut stale = p.clone();
        stale["expected_stair"] = json!("core");
        assert!(s.execute("entrance.bind_stair", stale).is_err());
        let mut moved = p.clone();
        moved["expected_address"]["point_mm"][1] = json!(-200.25001);
        assert!(s.execute("entrance.bind_stair", moved).is_err());
        assert_eq!(bytes(&s), before);
        assert_eq!((s.undo.len(), s.redo.len()), history);
        s.execute("edit.redo", json!({})).unwrap();
        let data = info(&mut s);
        s.execute(
            "entrance.bind_stair",
            json!({"id":"b.entrance0","expected_address":data["expected_address"],"level":null,"expected_stair":"core","stair":null}),
        )
        .unwrap();
        let data = info(&mut s);
        assert_eq!(data["default_stair"], Value::Null);
        assert_eq!(data["bindings"][0]["stored_override"], "basement");
        assert_eq!(data["bindings"][0]["effective_departure"]["valid"], true);
        assert_eq!(data["bindings"][1]["effective_departure"]["valid"], false);
        assert!(s.doc.buildings[0].entrances[0].stair_by_level.contains_key(&0), "clearing the default leaves overrides intact");
    }

    #[test]
    fn unit_floor_overflow_is_unmapped_instead_of_panicking() {
        let u = Unit { floor: usize::MAX.to_string(), door: String::new(), area_m2: None, rooms: None };
        assert_eq!(u.level(1), None);
    }

    #[test]
    fn local_entrance_previews_are_atomic_and_stale_apply_preserves_redo() {
        let mut s = session();
        let p = edit(&mut s);
        let before = bytes(&s);
        let preview = s.execute("precision.preview", json!({"action":{"command":"entrance.set_unit","params":p}})).unwrap();
        assert_eq!(bytes(&s), before);
        let data = info(&mut s);
        let old = data["units"][0]["unit"].clone();
        let mut unit = old.clone();
        unit["rooms"] = json!(3);
        s.execute(
            "entrance.set_unit",
            json!({"id":"b.entrance0","expected_address":data["expected_address"],"unit_index":0,"expected_unit":old,"unit":unit}),
        )
        .unwrap();
        s.execute("view.define", json!({"id":"review","name":"Review","kind":{"view":"plan","level":1}})).unwrap();
        s.execute("edit.undo", json!({})).unwrap();
        let changed = bytes(&s);
        let history = (s.undo.len(), s.redo.len());
        assert!(s.execute("precision.apply", json!({"preview_token":preview["preview_token"],"action":preview["action"]})).is_err());
        assert_eq!(bytes(&s), changed);
        assert_eq!((s.undo.len(), s.redo.len()), history);
        let fresh_params = edit(&mut s);
        let fresh = s.execute("precision.preview", json!({"action":{"command":"entrance.set_unit","params":fresh_params}})).unwrap();
        s.execute("precision.apply", json!({"preview_token":fresh["preview_token"],"action":fresh["action"]})).unwrap();
        assert_eq!(s.doc.journal.last().unwrap().command, "entrance.set_unit");
        assert_eq!(s.doc.buildings[0].entrances[0].units[1].door, "th");
        let address = info(&mut s)["expected_address"].clone();
        let bind=s.execute("precision.preview",json!({"action":{"command":"entrance.bind_stair","params":{"id":"b.entrance0","expected_address":address,"level":0,"expected_stair":null,"stair":"basement"}}})).unwrap();
        s.execute("precision.apply", json!({"preview_token":bind["preview_token"],"action":bind["action"]})).unwrap();
        assert_eq!(s.doc.journal.last().unwrap().command, "entrance.bind_stair");
    }

    #[test]
    fn entrance_preflight_reports_actual_drawing_fit_before_applying_a_long_designation() {
        let mut s = session();
        s.execute("view.define", json!({"id":"tag-review","name":"Public unit tag review","kind":{"view":"plan","level":1}})).unwrap();
        s.execute(
            "view.configure",
            json!({"id":"tag-review","template":{"name":"Tight unit tag frame","crop":[-2000,-2000,14000,11000],
            "sizing":{"mode":"fixed","denominator":100,"centre":[6000,4500]}}}),
        )
        .unwrap();
        let mut params = edit(&mut s);
        params["unit"]["door"] = json!("W".repeat(64));
        let before = s.doc.clone();
        let history = (s.undo.len(), s.redo.len());
        let preview = s.execute("precision.preview", json!({"action":{"command":"entrance.set_unit","params":params}})).unwrap();
        assert_eq!(s.doc, before);
        assert_eq!((s.undo.len(), s.redo.len()), history);
        assert_eq!(preview["validation"]["drawing_quality"], "needs_review");
        let report = preview["drawing_feedback"].as_array().unwrap().iter().find(|r| r["view"] == "tag-review").unwrap();
        assert_eq!(report["status"], "needs_review");
        assert!(report["entrance_labels"]["unplaced"].as_u64().unwrap() > 0);
    }
}
