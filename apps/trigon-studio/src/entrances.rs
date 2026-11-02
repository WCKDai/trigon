//! Local entrance forms. They retain typed drafts and produce guarded precision previews;
//! only the shared review/apply path can change source records.

use std::collections::BTreeMap;

use eframe::egui;
use serde_json::{Value, json};
use trigon_model::{Building, Document, Entrance, Stair, Storey, Unit};

use crate::{authoring::Run, theme};

#[derive(Default)]
pub struct Editor {
    selected: Option<String>,
    draft: Option<Draft>,
}

struct Draft {
    source: Source,
    units: Vec<UnitDraft>,
    unit_index: Option<usize>,
    binding_level: Option<usize>,
    /// None is the stored default; Some(level) is the stored departure override.
    bindings: BTreeMap<Option<usize>, Option<String>>,
}

struct Source {
    entrance: Entrance,
    ground: usize,
    storeys: Vec<Storey>,
    stairs: Vec<Stair>,
}

#[derive(Clone, Debug, PartialEq)]
struct UnitDraft {
    floor: String,
    door: String,
    area: String,
    rooms: String,
}

fn resolve<'a>(doc: &'a Document, id: &str) -> Result<(&'a Building, &'a Entrance), &'static str> {
    if id.len() > 256 {
        return Err("The entrance locator is too long.");
    }
    let (building, index) = id.rsplit_once(".entrance").ok_or("Select an entrance from the Project Browser.")?;
    if building.is_empty() || index.is_empty() || !index.bytes().all(|c| c.is_ascii_digit()) {
        return Err("The entrance locator needs a building and a zero-based entry index.");
    }
    let index = index.parse::<usize>().map_err(|_| "The entrance index is outside the supported range.")?;
    let mut matches = doc.buildings.iter().filter(|b| b.id == building);
    let b = matches.next().ok_or("This entrance's building is no longer present.")?;
    if matches.next().is_some() {
        return Err("This building ID is ambiguous. Choose an unambiguous source before editing.");
    }
    let e = b.entrances.get(index).ok_or("This entrance index is no longer present. Select it again from the Project Browser.")?;
    if e.point.iter().any(|v| !v.is_finite() || v.abs() > 1e8) {
        return Err("The entrance access point needs finite site-local coordinates.");
    }
    Ok((b, e))
}

fn unknown(door: &str) -> bool {
    Unit::is_unknown_door(door)
}

fn floor_level(floor: &str, ground: usize) -> Option<usize> {
    match floor {
        "st" => Some(ground),
        "kl" => ground.checked_sub(1),
        floor => floor.parse::<usize>().ok().and_then(|n| ground.checked_add(n)),
    }
}

fn short(text: &str, count: usize) -> String {
    let mut chars = text.chars();
    let text: String = chars.by_ref().take(count).collect();
    if chars.next().is_some() { format!("{text}…") } else { text }
}

fn eligible(stairs: &[Stair], id: &str, departure: usize) -> bool {
    if id.is_empty() || id.chars().count() > 256 || id.chars().any(char::is_control) {
        return false;
    }
    let mut matches = stairs.iter().filter(|st| st.id == id);
    let Some(st) = matches.next() else { return false };
    matches.next().is_none() && st.levels.contains(&departure) && trigon_model::stair::frame(st).is_ok()
}

impl UnitDraft {
    fn from_unit(unit: &Unit) -> Self {
        Self {
            floor: unit.floor.clone(),
            door: unit.door.clone(),
            area: unit.area_m2.map(|v| v.to_string()).unwrap_or_default(),
            rooms: unit.rooms.map(|v| v.to_string()).unwrap_or_default(),
        }
    }

    fn unit(&self, source: &Source) -> Result<Unit, &'static str> {
        if self.floor.is_empty() || self.floor.chars().count() > 32 || self.floor.chars().any(char::is_control) {
            return Err("Use a floor designation up to 32 characters without control characters.");
        }
        if floor_level(&self.floor, source.ground).is_none_or(|l| l >= source.storeys.len()) {
            return Err("Floor must name an existing storey: kl, st, 1, 2, …");
        }
        if self.door.chars().count() > 64 || self.door.chars().any(char::is_control) {
            return Err("Use a door designation up to 64 characters without control characters.");
        }
        let area_m2 = if self.area.trim().is_empty() {
            None
        } else {
            let area = self.area.trim().parse::<f64>().map_err(|_| "Area needs a number in m², or blank when unknown.")?;
            if !area.is_finite() || !(0.0..=1e9).contains(&area) {
                return Err("Area must be a finite number between 0 and 1,000,000,000 m².");
            }
            Some(area)
        };
        let rooms = if self.rooms.trim().is_empty() {
            None
        } else {
            let rooms = self.rooms.trim().parse::<i64>().map_err(|_| "Rooms needs a whole number, or blank when unknown.")?;
            if !(0..=10000).contains(&rooms) {
                return Err("Rooms must be a whole number between 0 and 10,000.");
            }
            Some(rooms)
        };
        Ok(Unit { floor: self.floor.clone(), door: self.door.clone(), area_m2, rooms })
    }
}

impl Source {
    fn matches(&self, b: &Building, e: &Entrance) -> bool {
        self.entrance == *e && self.ground == b.ground_level && self.storeys == b.storeys && self.stairs == b.stairs
    }

    fn address(&self) -> Value {
        let e = &self.entrance;
        json!({"street":e.street,"number":e.number,"point_mm":e.point})
    }

    fn stored_stair(&self, level: Option<usize>) -> Option<&str> {
        match level {
            Some(level) => self.entrance.stair_by_level.get(&level).map(String::as_str),
            None => self.entrance.stair.as_deref(),
        }
    }
}

impl Draft {
    fn new(b: &Building, e: &Entrance) -> Self {
        let source = Source { entrance: e.clone(), ground: b.ground_level, storeys: b.storeys.clone(), stairs: b.stairs.clone() };
        let bindings = std::iter::once((None, e.stair.clone()))
            .chain((0..b.storeys.len()).map(|level| (Some(level), e.stair_by_level.get(&level).cloned())))
            .collect();
        Self {
            units: e.units.iter().map(UnitDraft::from_unit).collect(),
            unit_index: (!e.units.is_empty()).then_some(0),
            source,
            binding_level: None,
            bindings,
        }
    }

    fn unit_params(&self, id: &str) -> Result<Value, &'static str> {
        let index = self.unit_index.ok_or("This entrance has no stored unit records.")?;
        let before = self.source.entrance.units.get(index).ok_or("Choose a stored unit before editing.")?;
        let unit = self.units.get(index).ok_or("Choose a stored unit before editing.")?.unit(&self.source)?;
        if *before == unit {
            return Err("Change a unit field to preview the edit.");
        }
        let key = |u: &Unit| (!unknown(&u.door)).then(|| (floor_level(&u.floor, self.source.ground), u.door.trim().to_lowercase()));
        let next_key = key(&unit);
        if next_key.is_some()
            && next_key != key(before)
            && self.source.entrance.units.iter().enumerate().any(|(i, u)| i != index && key(u) == next_key)
        {
            return Err("That known door designation already exists on this storey. Check the source before changing it.");
        }
        Ok(json!({"id":id,"expected_address":self.source.address(),"unit_index":index,"expected_unit":before,"unit":unit}))
    }

    fn binding_params(&self, id: &str) -> Result<Value, &'static str> {
        let level = self.binding_level;
        if level.is_some_and(|l| l >= self.source.storeys.len()) {
            return Err("Choose a departure storey in this building.");
        }
        let before = self.source.stored_stair(level);
        let stair = self.bindings.get(&level).and_then(Option::as_deref);
        if before == stair {
            return Err("Choose a different source or clear the stored binding to preview.");
        }
        let departure = level.unwrap_or(self.source.ground);
        if stair.is_some_and(|id| departure >= self.source.storeys.len() || !eligible(&self.source.stairs, id, departure)) {
            return Err("Choose one eligible source stair departing on this storey.");
        }
        Ok(json!({"id":id,"expected_address":self.source.address(),"level":level,"expected_stair":before,"stair":stair}))
    }
}

impl Editor {
    /// Re-selecting the same locator never erases a typed draft.
    pub fn select(&mut self, id: impl Into<String>) {
        let id = id.into();
        if self.selected.as_ref() != Some(&id) {
            self.selected = Some(id);
            self.draft = None;
        }
    }

    pub fn clear(&mut self) {
        self.selected = None;
        self.draft = None;
    }

    pub fn is_selected(&self) -> bool {
        self.selected.is_some()
    }

    pub fn selected_id(&self) -> Option<&str> {
        self.selected.as_deref()
    }

    fn refresh(&mut self, doc: &Document) -> Result<(), &'static str> {
        let (b, e) = resolve(doc, self.selected.as_deref().ok_or("Select an entrance.")?)?;
        self.draft = Some(Draft::new(b, e));
        Ok(())
    }

    fn synchronize(&mut self, doc: &Document) -> Result<(), &'static str> {
        if self.draft.is_none() {
            self.refresh(doc)?;
        }
        Ok(())
    }

    fn current(&self, doc: &Document) -> Result<&Draft, &'static str> {
        let id = self.selected.as_deref().ok_or("Select an entrance.")?;
        let (b, e) = resolve(doc, id)?;
        let draft = self.draft.as_ref().ok_or("Refresh this entrance from the current document.")?;
        if !draft.source.matches(b, e) {
            return Err("The entrance, levels or source stairs changed. Your draft is retained; refresh from source before previewing.");
        }
        Ok(draft)
    }

    fn unit_preview(&self, doc: &Document) -> Result<Run, &'static str> {
        let draft = self.current(doc)?;
        let params = draft.unit_params(self.selected.as_deref().ok_or("Select an entrance.")?)?;
        Ok(Run { command: "precision.preview", params: json!({"action":{"command":"entrance.set_unit","params":params}}) })
    }

    fn binding_preview(&self, doc: &Document) -> Result<Run, &'static str> {
        let draft = self.current(doc)?;
        let params = draft.binding_params(self.selected.as_deref().ok_or("Select an entrance.")?)?;
        Ok(Run { command: "precision.preview", params: json!({"action":{"command":"entrance.bind_stair","params":params}}) })
    }

    pub fn ui(&mut self, ui: &mut egui::Ui, doc: &Document) -> Option<Run> {
        if !self.is_selected() {
            return None;
        }
        ui.heading("Entrance");
        let loaded = self.synchronize(doc);
        let stale = loaded.err().or_else(|| self.current(doc).err());
        if let Some(reason) = stale {
            ui.add(egui::Label::new(egui::RichText::new(reason).color(theme::ACCENT)).wrap());
        }
        if ui.button("Refresh from source").on_hover_text("Discard these entrance drafts and reload the current local source records.").clicked() {
            let _ = self.refresh(doc);
        }
        let draft = self.draft.as_mut()?;
        let e = &draft.source.entrance;
        ui.add(egui::Label::new(egui::RichText::new(format!("{} {}", e.street, e.number)).strong()).wrap());
        ui.add(
            egui::Label::new(egui::RichText::new(format!("Access point  X {}  ·  Y {} mm", e.point[0], e.point[1])).small().color(theme::MUTED))
                .wrap(),
        );
        ui.add(egui::Label::new("Local, unphased records. Blank or unknown doors stay unknown; TH/TV are never inferred.").wrap());
        ui.separator();
        theme::eyebrow(ui, "UNITS BY STOREY");
        let mut grouped: BTreeMap<Option<usize>, Vec<usize>> = BTreeMap::new();
        for (index, unit) in draft.source.entrance.units.iter().enumerate() {
            grouped.entry(floor_level(&unit.floor, draft.source.ground).filter(|&l| l < draft.source.storeys.len())).or_default().push(index);
        }
        if grouped.is_empty() {
            ui.weak("No local unit records for this entrance.");
        }
        for (level, indices) in grouped {
            let level_name = level.and_then(|l| draft.source.storeys.get(l)).map_or("Unmapped source floor", |s| s.name.as_str());
            ui.add(egui::Label::new(egui::RichText::new(level_name).small().color(theme::MUTED)).wrap());
            ui.horizontal_wrapped(|ui| {
                for index in indices {
                    let unit = &draft.source.entrance.units[index];
                    let label = if unknown(&unit.door) { "Unknown door".into() } else { short(&unit.door.to_uppercase(), 20) };
                    if ui
                        .selectable_label(draft.unit_index == Some(index), label)
                        .on_hover_text(format!("Unit record {index} · floor {} · door {:?}", unit.floor, unit.door))
                        .clicked()
                    {
                        draft.unit_index = Some(index);
                    }
                }
            });
        }
        if let Some(index) = draft.unit_index
            && let Some(unit) = draft.units.get_mut(index)
        {
            ui.add_space(5.0);
            ui.label("Floor designation");
            ui.add(egui::TextEdit::singleline(&mut unit.floor).char_limit(32).hint_text("kl, st, 1, 2, …").desired_width(f32::INFINITY));
            ui.label("Door designation");
            ui.add(egui::TextEdit::singleline(&mut unit.door).char_limit(64).hint_text("Blank when unknown").desired_width(f32::INFINITY));
            ui.horizontal_wrapped(|ui| {
                for (label, value) in [("TV", "tv"), ("TH", "th"), ("MF", "mf"), ("Unknown", "")] {
                    if ui.small_button(label).clicked() {
                        unit.door = value.into();
                    }
                }
            });
            ui.label("Area · m²");
            ui.add(egui::TextEdit::singleline(&mut unit.area).char_limit(32).hint_text("Unknown").desired_width(f32::INFINITY));
            ui.label("Rooms");
            ui.add(egui::TextEdit::singleline(&mut unit.rooms).char_limit(16).hint_text("Unknown").desired_width(f32::INFINITY));
        }
        let unit_candidate = self.unit_preview(doc);
        if ui.add_enabled(unit_candidate.is_ok(), egui::Button::new("Review unit edit")).clicked() {
            return unit_candidate.ok();
        }
        if let Err(reason) = unit_candidate
            && stale.is_none()
        {
            ui.add(egui::Label::new(egui::RichText::new(reason).small().color(theme::MUTED)).wrap());
        }
        ui.separator();
        theme::eyebrow(ui, "SOURCE STAIR BINDING");
        let draft = self.draft.as_mut()?;
        let scope_name = draft.binding_level.and_then(|level| draft.source.storeys.get(level)).map(|s| format!("Departure · {}", s.name));
        egui::ComboBox::from_id_salt("entrance-binding-level")
            .width(ui.available_width())
            .selected_text(short(scope_name.as_deref().unwrap_or("Default · ground floor"), 28))
            .show_ui(ui, |ui| {
                ui.selectable_value(&mut draft.binding_level, None, "Default · ground floor");
                for (level, storey) in draft.source.storeys.iter().enumerate() {
                    ui.selectable_value(&mut draft.binding_level, Some(level), short(&format!("Departure · {}", storey.name), 30));
                }
            });
        let departure = draft.binding_level.unwrap_or(draft.source.ground);
        let selected = draft.bindings.entry(draft.binding_level).or_default();
        let empty = if draft.binding_level.is_some() { "Use default · clear override" } else { "No default source" };
        let selected_name = selected.as_deref().unwrap_or(empty);
        egui::ComboBox::from_id_salt("entrance-binding-source").width(ui.available_width()).selected_text(short(selected_name, 28)).show_ui(
            ui,
            |ui| {
                ui.selectable_value(selected, None, empty);
                for stair in &draft.source.stairs {
                    if departure < draft.source.storeys.len() && eligible(&draft.source.stairs, &stair.id, departure) {
                        ui.selectable_value(selected, Some(stair.id.clone()), short(&stair.id, 30)).on_hover_text(&stair.id);
                    }
                }
            },
        );
        let stored = draft.source.stored_stair(draft.binding_level);
        ui.add(egui::Label::new(egui::RichText::new(format!("Stored slot: {}", stored.unwrap_or("none"))).small().color(theme::MUTED)).wrap());
        if stored.is_some_and(|id| departure >= draft.source.storeys.len() || !eligible(&draft.source.stairs, id, departure)) {
            ui.add(
                egui::Label::new(egui::RichText::new("Stored source is missing, ambiguous or does not depart on this storey.").color(theme::ACCENT))
                    .wrap(),
            );
        }
        ui.add(egui::Label::new("Only existing stairs departing here are offered. Clearing an override resumes the default; clearing the default leaves overrides intact.").wrap());
        ui.add(
            egui::Label::new(
                egui::RichText::new("A valid source reference does not verify its physical entrance association.").small().color(theme::MUTED),
            )
            .wrap(),
        );
        let binding_candidate = self.binding_preview(doc);
        if ui.add_enabled(binding_candidate.is_ok(), egui::Button::new("Review stair binding")).clicked() {
            return binding_candidate.ok();
        }
        if let Err(reason) = binding_candidate
            && stale.is_none()
        {
            ui.add(egui::Label::new(egui::RichText::new(reason).small().color(theme::MUTED)).wrap());
        }
        ui.add_space(5.0);
        ui.add(
            egui::Label::new(
                egui::RichText::new("Entry indices belong to this document. Provider replacement can reorder them.").small().color(theme::MUTED),
            )
            .wrap(),
        );
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn document() -> Document {
        let mut doc = Document::default();
        let mut b = Building::new("b", "Local editor fixture", &[[0., 0.], [12000., 0.], [12000., 9000.], [0., 9000.]], 3, 3000.).unwrap();
        b.ground_level = 1;
        for (s, name) in b.storeys.iter_mut().zip(["Basement", "Ground", "First"]) {
            s.name = name.into();
        }
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
                Unit { floor: "st".into(), door: String::new(), area_m2: None, rooms: None },
                Unit { floor: "1".into(), door: "2".into(), area_m2: Some(61.), rooms: Some(3) },
            ],
        });
        let mut other = b.entrances[0].clone();
        other.number = "3".into();
        other.point = [8000., -200.25];
        b.entrances.push(other);
        doc.buildings.push(b);
        doc
    }

    fn editor(doc: &Document) -> Editor {
        let mut editor = Editor::default();
        editor.select("b.entrance0");
        editor.synchronize(doc).unwrap();
        editor
    }

    #[test]
    fn unit_preview_carries_exact_source_guards_without_mutating_document() {
        let doc = document();
        let before = serde_json::to_vec(&doc).unwrap();
        let mut editor = editor(&doc);
        let draft = editor.draft.as_mut().unwrap();
        draft.unit_index = Some(1);
        draft.units[1].door = "th".into();
        let run = editor.unit_preview(&doc).unwrap();
        assert_eq!(run.command, "precision.preview");
        assert_eq!(run.params["action"]["command"], "entrance.set_unit");
        let p = &run.params["action"]["params"];
        assert_eq!(p["id"], "b.entrance0");
        assert_eq!(p["expected_address"], json!({"street":"Sample Street","number":"1A","point_mm":[1250.125,-200.25]}));
        assert_eq!(p["unit_index"], 1);
        assert_eq!(p["expected_unit"], serde_json::to_value(&doc.buildings[0].entrances[0].units[1]).unwrap());
        assert_eq!(p["unit"]["door"], "th");
        assert_eq!(p["unit"]["area_m2"], Value::Null);
        assert_eq!(serde_json::to_vec(&doc).unwrap(), before);
    }

    #[test]
    fn same_selection_and_repaints_retain_drafts_until_explicit_source_refresh() {
        let mut doc = document();
        let mut editor = editor(&doc);
        let draft = editor.draft.as_mut().unwrap();
        draft.unit_index = Some(1);
        draft.units[1].door = "th".into();
        draft.units[1].area = "61.125".into();
        let retained = draft.units.clone();
        for _ in 0..4 {
            editor.select("b.entrance0");
            editor.synchronize(&doc).unwrap();
            assert_eq!(editor.draft.as_ref().unwrap().units, retained);
        }
        doc.buildings[0].entrances[0].units[1].rooms = Some(4);
        editor.synchronize(&doc).unwrap();
        assert!(editor.unit_preview(&doc).is_err());
        assert_eq!(editor.draft.as_ref().unwrap().units, retained);
        editor.refresh(&doc).unwrap();
        assert_eq!(editor.draft.as_ref().unwrap().units[1].door, "");
        assert_eq!(editor.draft.as_ref().unwrap().units[1].rooms, "4");
        assert!(editor.unit_preview(&doc).is_err(), "fresh unchanged records do not produce no-op edits");
        editor.clear();
        assert!(!editor.is_selected());
        assert_eq!(editor.selected_id(), None);
    }

    #[test]
    fn ambiguous_reordered_or_invalid_locators_never_edit_another_record() {
        let mut doc = document();
        let mut editor = editor(&doc);
        editor.draft.as_mut().unwrap().units[0].rooms = "3".into();
        doc.buildings[0].entrances.swap(0, 1);
        assert!(editor.unit_preview(&doc).is_err());
        doc.buildings[0].entrances.swap(0, 1);
        let mut duplicate = doc.buildings[0].clone();
        duplicate.name = "Duplicate ID".into();
        doc.buildings.push(duplicate);
        assert!(editor.unit_preview(&doc).is_err());
        doc.buildings.pop();
        for id in ["", "b", "b.entrance", "b.entrance-1", "b.entrance99", "missing.entrance0"] {
            editor.select(id);
            assert!(editor.synchronize(&doc).is_err());
            assert!(editor.unit_preview(&doc).is_err());
        }
    }

    #[test]
    fn unknown_and_numeric_designations_are_preserved_as_source_values() {
        let mut doc = document();
        for door in ["", "Ukendt", "?", "unknown", "2"] {
            doc.buildings[0].entrances[0].units[1].door = door.into();
            let mut editor = editor(&doc);
            let draft = editor.draft.as_mut().unwrap();
            draft.unit_index = Some(1);
            draft.units[1].rooms = "4".into();
            let run = editor.unit_preview(&doc).unwrap();
            assert_eq!(run.params["action"]["params"]["unit"]["door"], door);
            assert_eq!(unknown(door), door != "2");
        }
    }

    #[test]
    fn binding_preview_guards_the_stored_slot_and_clear_restores_default() {
        let mut doc = document();
        let mut editor = editor(&doc);
        let draft = editor.draft.as_mut().unwrap();
        draft.bindings.insert(None, Some("basement".into()));
        assert!(editor.binding_preview(&doc).is_err(), "default must serve ground");
        let draft = editor.draft.as_mut().unwrap();
        draft.binding_level = Some(0);
        draft.bindings.insert(Some(0), Some("basement".into()));
        let run = editor.binding_preview(&doc).unwrap();
        let p = &run.params["action"]["params"];
        assert_eq!(p["level"], 0);
        assert_eq!(p["expected_stair"], Value::Null);
        assert_eq!(p["stair"], "basement");
        doc.buildings[0].entrances[0].stair_by_level.insert(0, "basement".into());
        assert!(editor.binding_preview(&doc).is_err());
        editor.refresh(&doc).unwrap();
        let draft = editor.draft.as_mut().unwrap();
        draft.binding_level = Some(0);
        draft.bindings.insert(Some(0), None);
        let clear = editor.binding_preview(&doc).unwrap();
        let p = &clear.params["action"]["params"];
        assert_eq!(p["expected_stair"], "basement");
        assert_eq!(p["stair"], Value::Null);
        assert_eq!(p["level"], 0);
        assert_eq!(doc.buildings[0].entrances[0].stair.as_deref(), Some("core"));
        editor.draft.as_mut().unwrap().binding_level = None;
        editor.draft.as_mut().unwrap().bindings.insert(None, None);
        let clear = editor.binding_preview(&doc).unwrap();
        assert_eq!(clear.params["action"]["params"]["level"], Value::Null);
        assert_eq!(clear.params["action"]["params"]["expected_stair"], "core");
        assert_eq!(doc.buildings[0].entrances[0].stair_by_level.get(&0).map(String::as_str), Some("basement"));
    }

    #[test]
    fn source_stair_changes_and_ineligible_targets_require_explicit_review() {
        let mut doc = document();
        let mut editor = editor(&doc);
        let draft = editor.draft.as_mut().unwrap();
        draft.binding_level = Some(0);
        for id in ["core", "missing", "other.core", ""] {
            editor.draft.as_mut().unwrap().bindings.insert(Some(0), Some(id.into()));
            assert!(editor.binding_preview(&doc).is_err());
        }
        editor.draft.as_mut().unwrap().bindings.insert(Some(0), Some("basement".into()));
        assert!(editor.binding_preview(&doc).is_ok());
        doc.buildings[0].stairs[1].outline[0][0] += 1.;
        assert!(editor.binding_preview(&doc).is_err());
        assert_eq!(editor.draft.as_ref().unwrap().bindings[&Some(0)].as_deref(), Some("basement"));
    }

    #[test]
    fn incomplete_or_hostile_unit_fields_do_not_produce_preview_actions() {
        let doc = document();
        let mut editor = editor(&doc);
        editor.draft.as_mut().unwrap().unit_index = Some(1);
        for (field, value) in [
            ("floor", usize::MAX.to_string()),
            ("floor", "unknown".into()),
            ("door", "bad\nlabel".into()),
            ("area", "NaN".into()),
            ("area", "inf".into()),
            ("area", "-1".into()),
            ("rooms", "1.5".into()),
            ("rooms", "-1".into()),
        ] {
            editor.refresh(&doc).unwrap();
            let draft = editor.draft.as_mut().unwrap();
            draft.unit_index = Some(1);
            draft.units[1].door = "th".into();
            let unit = &mut draft.units[1];
            match field {
                "floor" => unit.floor = value,
                "door" => unit.door = value,
                "area" => unit.area = value,
                _ => unit.rooms = value,
            };
            assert!(editor.unit_preview(&doc).is_err());
        }
    }

    #[test]
    fn entrance_forms_fit_narrow_properties_columns_and_retain_fields_during_ui_frames() {
        let mut doc = document();
        doc.buildings[0].entrances[0].street = "A long source street name that must wrap inside the Properties panel".into();
        for width in [260., 340.] {
            let mut editor = editor(&doc);
            editor.draft.as_mut().unwrap().unit_index = Some(1);
            editor.draft.as_mut().unwrap().units[1].door = "th".into();
            let ctx = egui::Context::default();
            theme::apply(&ctx);
            let mut output = ctx.run_ui(
                egui::RawInput { screen_rect: Some(egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(width, 1700.))), ..Default::default() },
                |ui| {
                    egui::CentralPanel::default().show(ui, |ui| {
                        assert!(editor.ui(ui, &doc).is_none());
                        assert!(ui.min_rect().right() <= width + 1., "entrance editor overflowed a {width}px column: {:?}", ui.min_rect());
                    });
                },
            );
            output.textures_delta.clear();
            assert!(!output.shapes.is_empty());
            assert_eq!(editor.draft.as_ref().unwrap().units[1].door, "th");
        }
    }
}
