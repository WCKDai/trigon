//! Typed authoring forms. They produce engine commands; no model is changed in this module.

use std::collections::BTreeSet;

use eframe::egui;
use serde_json::{Value, json};
use trigon_model::{ComponentType, Document};

use crate::{scene::Element, theme};

#[derive(Clone, Copy, Default, PartialEq)]
pub enum Tab {
    #[default]
    Project,
    Families,
    Drawings,
    Schedules,
}

#[derive(Clone, Copy, Default, PartialEq)]
enum Tool {
    #[default]
    Move,
    Copy,
    Align,
    Levels,
    Delete,
}

pub struct Run {
    pub command: &'static str,
    pub params: Value,
}

pub struct Authoring {
    pub tab: Tab,
    tool: Tool,
    pub targets: String,
    delta: [f64; 3],
    along: bool,
    count: usize,
    levels: BTreeSet<usize>,
    replace: bool,
    axis: usize,
    anchor: usize,
    align_mode: usize,
    coordinate: f64,
    reference: String,
    wall_reference_face: usize,
    wall_target_face: usize,
    line: [[f64; 2]; 2],
    group_id: String,
    group_name: String,
    pub search: String,
    draft: Option<ComponentType>,
    kind: Value,
    facade: String,
    bay_count: usize,
    library_import_path: String,
    library_export_path: String,
    library_replace: bool,
    library_selected_only: bool,
}

impl Default for Authoring {
    fn default() -> Self {
        Self {
            tab: Tab::Project,
            tool: Tool::Move,
            targets: String::new(),
            delta: [100.0, 0.0, 0.0],
            along: false,
            count: 1,
            levels: BTreeSet::new(),
            replace: false,
            axis: 0,
            anchor: 1,
            align_mode: 0,
            coordinate: 0.0,
            reference: String::new(),
            wall_reference_face: 0,
            wall_target_face: 0,
            line: [[0.0, 0.0], [1000.0, 0.0]],
            group_id: "selection".into(),
            group_name: "Selection group".into(),
            search: String::new(),
            draft: None,
            kind: Value::Null,
            facade: String::new(),
            bay_count: 5,
            library_import_path: String::new(),
            library_export_path: "out/components.trigonlib".into(),
            library_replace: false,
            library_selected_only: true,
        }
    }
}

fn field(ui: &mut egui::Ui, label: &str, value: &mut f64) {
    ui.label(label);
    ui.add(egui::DragValue::new(value).speed(1.0).range(-1e8..=1e8).max_decimals(3).suffix(" mm"));
    ui.end_row();
}

pub fn family_name(ty: &ComponentType) -> String {
    serde_json::to_value(&ty.kind)
        .ok()
        .and_then(|v| v.get("family").and_then(Value::as_str).map(|s| s.replace('_', " ")))
        .unwrap_or_else(|| "Component".into())
}

impl Authoring {
    pub fn choose_type(&mut self, ty: &ComponentType) {
        self.kind = serde_json::to_value(&ty.kind).unwrap_or(Value::Null);
        self.draft = Some(ty.clone());
        self.tab = Tab::Families;
    }

    pub fn library(&mut self, ui: &mut egui::Ui, doc: &Document) -> Option<Run> {
        let mut run = None;
        ui.add(egui::TextEdit::singleline(&mut self.search).hint_text("Find a family or type").desired_width(f32::INFINITY));
        let search = self.search.to_lowercase();
        theme::eyebrow(ui, "COMPONENT TYPES");
        for ty in &doc.types {
            if !format!("{} {} {}", ty.id, ty.name, family_name(ty)).to_lowercase().contains(&search) {
                continue;
            }
            let selected = self.draft.as_ref().is_some_and(|d| d.id == ty.id);
            if ui.selectable_label(selected, &ty.name).on_hover_text(format!("{} · {:.0} × {:.0} mm", family_name(ty), ty.width, ty.height)).clicked()
            {
                self.choose_type(ty);
            }
        }
        ui.add_space(8.0);
        ui.weak("Select a shared type to edit all referencing instances, or duplicate it as a new type.");
        ui.add_space(10.0);
        egui::CollapsingHeader::new("Portable component libraries").id_salt("portable-libraries").show(ui, |ui| {
            ui.weak("Local .trigonlib snapshots · no account required. Imported definitions become shared project types.");
            ui.label("Import library");
            ui.add(
                egui::TextEdit::singleline(&mut self.library_import_path)
                    .char_limit(4096)
                    .hint_text("path/to/components.trigonlib")
                    .desired_width(f32::INFINITY),
            );
            #[cfg(any(target_os = "macos", target_os = "windows"))]
            if ui.button("Browse library…").clicked()
                && let Some(path) = rfd::FileDialog::new().add_filter("Trigon component library", &["trigonlib"]).pick_file()
            {
                self.library_import_path = path.to_string_lossy().into_owned();
            }
            ui.checkbox(&mut self.library_replace, "Replace matching type IDs globally");
            if self.library_replace {
                ui.colored_label(theme::ACCENT, "Replacement updates every referencing instance. Explicit instance overrides remain local.");
            }
            let supported = trigon_engine::commands().iter().any(|c| c.id == "types.import");
            if ui.add_enabled(supported && self.import_library().is_some(), egui::Button::new("Import shared types")).clicked() {
                run = self.import_library();
            }
            if !supported {
                ui.weak("Import requires the matching portable-library engine commands.");
            }
            ui.separator();
            ui.label("Export library");
            ui.add(
                egui::TextEdit::singleline(&mut self.library_export_path)
                    .char_limit(4096)
                    .hint_text("out/components.trigonlib")
                    .desired_width(f32::INFINITY),
            );
            #[cfg(any(target_os = "macos", target_os = "windows"))]
            if ui.button("Choose export file…").clicked()
                && let Some(path) =
                    rfd::FileDialog::new().add_filter("Trigon component library", &["trigonlib"]).set_file_name("components.trigonlib").save_file()
            {
                self.library_export_path = path.to_string_lossy().into_owned();
            }
            ui.checkbox(&mut self.library_selected_only, "Selected type only");
            if self.library_selected_only {
                ui.weak(self.draft.as_ref().map_or("Choose a type to export", |t| t.id.as_str()));
            } else {
                ui.weak(format!("All {} shared project types", doc.types.len()));
            }
            let supported = trigon_engine::commands().iter().any(|c| c.id == "types.export");
            if ui
                .add_enabled(supported && !doc.types.is_empty() && self.export_library().is_some(), egui::Button::new("Export type library"))
                .clicked()
            {
                run = self.export_library();
            }
            if !supported {
                ui.weak("Export requires the matching portable-library engine commands.");
            }
        });
        run
    }

    fn library_path(path: &str) -> Option<&str> {
        let path = path.trim();
        (!path.is_empty()
            && path.len() <= 4096
            && !path.chars().any(char::is_control)
            && !path.contains("://")
            && std::path::Path::new(path).extension().and_then(|s| s.to_str()).is_some_and(|s| s.eq_ignore_ascii_case("trigonlib")))
        .then_some(path)
    }
    fn import_library(&self) -> Option<Run> {
        Some(Run { command: "types.import", params: json!({"path":Self::library_path(&self.library_import_path)?,"replace":self.library_replace}) })
    }
    fn export_library(&self) -> Option<Run> {
        let mut params = json!({"path":Self::library_path(&self.library_export_path)?});
        if self.library_selected_only {
            params["ids"] = json!([self.draft.as_ref()?.id]);
        }
        Some(Run { command: "types.export", params })
    }

    pub(crate) fn selected(&self, current: Option<&Element>) -> Vec<String> {
        if self.targets.trim().is_empty() {
            current.map(|e| vec![e.id.clone()]).unwrap_or_default()
        } else {
            self.targets.split(|c: char| c.is_whitespace() || c == ',').filter(|s| !s.is_empty()).map(str::to_string).collect()
        }
    }

    fn align_walls(&self, targets: &[String]) -> Option<Run> {
        let reference = self.reference.trim();
        if reference.is_empty() || targets.is_empty() || targets.iter().any(|id| id == reference) {
            return None;
        }
        let faces = ["center", "left", "right"];
        Some(Run {
            command: "edit.align_walls",
            params: json!({"targets":targets,"reference":reference,
            "reference_face":faces[self.wall_reference_face],"target_face":faces[self.wall_target_face]}),
        })
    }

    pub fn ribbon(&mut self, ui: &mut egui::Ui) {
        for (tool, label) in
            [(Tool::Move, "Move"), (Tool::Copy, "Copy"), (Tool::Align, "Align"), (Tool::Levels, "To levels"), (Tool::Delete, "Delete")]
        {
            ui.selectable_value(&mut self.tool, tool, label);
        }
    }

    pub fn ui(&mut self, ui: &mut egui::Ui, doc: &Document, current: Option<&Element>, level: usize) -> Option<Run> {
        if self.tab == Tab::Families {
            return self.family_editor(ui, doc, level);
        }
        let mut run = None;
        theme::eyebrow(ui, "MODIFY");
        ui.horizontal_wrapped(|ui| {
            for (tool, label) in
                [(Tool::Move, "Move"), (Tool::Copy, "Copy"), (Tool::Align, "Align"), (Tool::Levels, "To levels"), (Tool::Delete, "Delete")]
            {
                ui.selectable_value(&mut self.tool, tool, label);
            }
        });
        ui.add_space(4.0);
        let targets = self.selected(current);
        ui.label(
            egui::RichText::new(if self.targets.trim().is_empty() {
                current.map_or("Pick an element", |e| e.id.as_str()).to_string()
            } else {
                format!("{} selected targets", targets.len())
            })
            .strong(),
        );
        let facade_frame = current
            .and_then(|e| e.props.iter().find(|(k, _)| *k == "facade"))
            .and_then(|(_, id)| doc.buildings.iter().find_map(|b| b.facades.iter().find(|f| &f.id == id).and_then(|f| b.edge(f.edge))))
            .and_then(|(a, b)| trigon_model::build::FacadeFrame::new(a, b));
        let editable = !targets.is_empty()
            && (!self.targets.trim().is_empty()
                || current.is_some_and(|e| {
                    matches!(e.kind, "stair" | "interior wall")
                        || (facade_frame.is_some() && matches!(e.kind, "window" | "door" | "balcony" | "railing"))
                }));
        ui.add_enabled_ui(editable, |ui| match self.tool {
            Tool::Move | Tool::Copy => {
                let can_along = facade_frame.is_some() && self.targets.trim().is_empty();
                if !can_along {
                    self.along = false;
                }
                ui.add_enabled(can_along, egui::Checkbox::new(&mut self.along, "Along the selected facade"))
                    .on_hover_text("For one picked facade instance: use the first distance along its host wall instead of site X/Y.");
                egui::Grid::new("offset-fields").num_columns(2).spacing([12.0, 7.0]).show(ui, |ui| {
                    field(ui, if self.along { "Along wall" } else { "X offset" }, &mut self.delta[0]);
                    if !self.along {
                        field(ui, "Y offset", &mut self.delta[1]);
                    }
                    field(ui, "Height offset", &mut self.delta[2]);
                    if self.tool == Tool::Copy {
                        ui.label("Copies");
                        ui.add(egui::DragValue::new(&mut self.count).range(1..=100));
                        ui.end_row();
                    }
                });
                let label = if self.tool == Tool::Move { "Move selection" } else { "Create copies" };
                if ui.add_sized([ui.available_width(), 30.0], egui::Button::new(label)).clicked() {
                    let mut delta = self.delta;
                    if self.along {
                        delta[1] = 0.0;
                        if let Some(frame) = facade_frame {
                            delta[0] = frame.t[0] * self.delta[0];
                            delta[1] = frame.t[1] * self.delta[0];
                        } else {
                            return;
                        }
                    }
                    let command = if self.tool == Tool::Move { "edit.move" } else { "edit.copy" };
                    let mut params = json!({"targets":targets,"delta":delta});
                    if self.tool == Tool::Copy {
                        params["count"] = json!(self.count);
                    }
                    run = Some(Run { command, params });
                }
            }
            Tool::Align => {
                let axes = ["x", "y", "u", "z"];
                let anchors = ["start", "center", "end"];
                let modes = ["Coordinate", "Reference element", "Plan line", "Wall line / face"];
                egui::ComboBox::from_id_salt("align-mode").selected_text(modes[self.align_mode]).show_ui(ui, |ui| {
                    for (i, label) in modes.iter().enumerate() {
                        ui.selectable_value(&mut self.align_mode, i, *label);
                    }
                });
                if self.align_mode < 2 {
                    ui.horizontal(|ui| {
                        egui::ComboBox::from_id_salt("align-axis").width(60.0).selected_text(axes[self.axis].to_uppercase()).show_ui(ui, |ui| {
                            for (i, a) in axes.iter().enumerate() {
                                ui.selectable_value(&mut self.axis, i, a.to_uppercase());
                            }
                        });
                        egui::ComboBox::from_id_salt("align-anchor").width(90.0).selected_text(anchors[self.anchor]).show_ui(ui, |ui| {
                            for (i, a) in anchors.iter().enumerate() {
                                ui.selectable_value(&mut self.anchor, i, *a);
                            }
                        });
                    });
                }
                if self.align_mode == 2 {
                    egui::ComboBox::from_id_salt("line-anchor").selected_text(anchors[self.anchor]).show_ui(ui, |ui| {
                        for (i, a) in anchors.iter().enumerate() {
                            ui.selectable_value(&mut self.anchor, i, *a);
                        }
                    });
                }
                match self.align_mode {
                    0 => {
                        ui.add(egui::DragValue::new(&mut self.coordinate).speed(1.0).suffix(" mm"));
                    }
                    1 => {
                        ui.add(egui::TextEdit::singleline(&mut self.reference).hint_text("Reference element id"));
                    }
                    2 => {
                        egui::Grid::new("align-line").show(ui, |ui| {
                            field(ui, "Start X", &mut self.line[0][0]);
                            field(ui, "Start Y", &mut self.line[0][1]);
                            field(ui, "End X", &mut self.line[1][0]);
                            field(ui, "End Y", &mut self.line[1][1]);
                        });
                    }
                    _ => {
                        ui.label("Pick the reference wall, capture it, then pick the wall to align.");
                        if ui.add_enabled(current.is_some_and(|e|e.kind == "interior wall"),egui::Button::new("Use picked wall as reference")).clicked()
                            && let Some(wall)=current {
                            self.reference=wall.id.clone();
                        }
                        ui.add(egui::TextEdit::singleline(&mut self.reference).hint_text("Reference wall id").char_limit(512));
                        let labels=["Centreline","Left face","Right face"];
                        for (name,id,selected) in [("Reference", "wall-reference-face", &mut self.wall_reference_face),
                            ("Target", "wall-target-face", &mut self.wall_target_face)] {
                            ui.horizontal(|ui| {
                                ui.label(name);
                                egui::ComboBox::from_id_salt(id).selected_text(labels[*selected]).show_ui(ui,|ui| {
                                    for (index,label) in labels.iter().enumerate() { ui.selectable_value(selected,index,*label); }
                                });
                            });
                        }
                        ui.weak("Left/right follow each wall's start-to-end direction. Aligns walls within 15°, keeping lengths and door positions along the wall.");
                    }
                }
                if self.align_mode == 3 {
                    let alignment=self.align_walls(&targets);
                    if ui.add_enabled(alignment.is_some(),egui::Button::new("Align wall selection")).clicked() { run=alignment; }
                } else if ui.button("Align selection").clicked() {
                    let mut params = json!({"targets":targets,"anchor":anchors[self.anchor]});
                    if self.align_mode == 2 {
                        params["line"] = json!(self.line);
                    } else {
                        params["axis"] = json!(axes[self.axis]);
                        if self.align_mode == 0 {
                            params["coordinate"] = json!(self.coordinate);
                        } else {
                            params["reference"] = json!(self.reference);
                        }
                    }
                    run = Some(Run { command: "edit.align", params });
                }
            }
            Tool::Levels => {
                ui.label("Destination storeys");
                let building = current
                    .and_then(|e| e.props.iter().find(|(k, _)| *k == "building"))
                    .and_then(|(_, id)| doc.buildings.iter().find(|b| b.id == *id))
                    .or_else(|| doc.buildings.first());
                if let Some(b) = building {
                    for (i, storey) in b.storeys.iter().enumerate() {
                        let mut selected = self.levels.contains(&i);
                        if ui.checkbox(&mut selected, &storey.name).changed() {
                            if selected {
                                self.levels.insert(i);
                            } else {
                                self.levels.remove(&i);
                            }
                        }
                    }
                }
                ui.checkbox(&mut self.replace, "Copy over matching elements");
                if ui.add_enabled(!self.levels.is_empty(), egui::Button::new("Copy to storeys")).clicked() {
                    run =
                        Some(Run { command: "edit.copy_to_levels", params: json!({"targets":targets,"levels":self.levels,"replace":self.replace}) });
                }
            }
            Tool::Delete => {
                ui.weak("Removes the selected source instances in every phase. You can undo this batch.");
                if ui.button("Delete selection").clicked() {
                    run = Some(Run { command: "edit.delete", params: json!({"targets":targets}) });
                }
            }
        });
        if !editable {
            ui.weak("Select a window, balcony, interior wall, stair or saved group. Exterior wall hosts use their facade tools.");
        }
        ui.separator();
        egui::CollapsingHeader::new("Selection sets & groups").show(ui, |ui| {
            ui.add(
                egui::TextEdit::multiline(&mut self.targets)
                    .desired_rows(2)
                    .hint_text("Element ids or group:id; blank uses the picked element")
                    .desired_width(f32::INFINITY),
            );
            ui.horizontal(|ui| {
                if ui.add_enabled(current.is_some(), egui::Button::new("Add picked")).clicked()
                    && let Some(e) = current
                {
                    let mut ids = self.targets.split_whitespace().map(str::to_string).collect::<Vec<_>>();
                    if !ids.contains(&e.id) {
                        ids.push(e.id.clone());
                    }
                    self.targets = ids.join("\n");
                }
                if ui.button("Clear set").clicked() {
                    self.targets.clear();
                }
            });
            ui.add(egui::TextEdit::singleline(&mut self.group_name).hint_text("Group name"));
            ui.add(egui::TextEdit::singleline(&mut self.group_id).hint_text("Group id"));
            if ui.add_enabled(!targets.is_empty(), egui::Button::new("Save group")).clicked() {
                run = Some(Run {
                    command: "group.define",
                    params: json!({"id":self.group_id,"name":self.group_name,"targets":self.selected(current)}),
                });
            }
        });
        run
    }

    fn family_editor(&mut self, ui: &mut egui::Ui, doc: &Document, level: usize) -> Option<Run> {
        let Some(draft) = &mut self.draft else {
            theme::eyebrow(ui, "FAMILIES");
            ui.label("Choose a component type from the library.");
            return None;
        };
        let mut run = None;
        theme::eyebrow(ui, &family_name(draft).to_uppercase());
        ui.heading(&draft.name);
        egui::Frame::new().fill(theme::CHROME).inner_margin(8.0).corner_radius(3).show(ui, |ui| {
            theme::eyebrow(ui, "SHARED TYPE · PROJECT-WIDE");
            ui.label("Sizes, materials and colors apply to every instance using this type. Explicit instance overrides stay local.");
        });
        if ui.button("Duplicate as new type").clicked() {
            let base = draft.id.clone();
            for n in 1..=10_000 {
                let id = format!("{base}.copy{n}");
                if !doc.types.iter().any(|t| t.id == id) {
                    draft.id = id;
                    draft.name = format!("{} copy", draft.name);
                    break;
                }
            }
        }
        ui.label("Type name");
        ui.text_edit_singleline(&mut draft.name);
        ui.label("Type id");
        ui.text_edit_singleline(&mut draft.id);
        egui::Grid::new("family-sizes").show(ui, |ui| {
            field(ui, "Width", &mut draft.width);
            field(ui, "Height", &mut draft.height);
            field(ui, "Sill", &mut draft.sill);
        });
        egui::CollapsingHeader::new("Family parameters").default_open(true).show(ui, |ui| {
            if let Some(fields) = self.kind.as_object_mut() {
                egui::Grid::new("family-parameters").show(ui, |ui| {
                    for (key, value) in fields {
                        if key == "family" {
                            continue;
                        }
                        let label = key.replace('_', " ");
                        match value {
                            Value::Number(n) if n.is_u64() => {
                                let mut v = n.as_u64().unwrap_or(0);
                                ui.label(label);
                                if ui.add(egui::DragValue::new(&mut v).range(0..=100_000)).changed() {
                                    *value = json!(v);
                                }
                                ui.end_row();
                            }
                            Value::Number(n) => {
                                let mut v = n.as_f64().unwrap_or(0.0);
                                ui.label(label);
                                if ui.add(egui::DragValue::new(&mut v).speed(1.0).range(0.0..=100_000.0)).changed() {
                                    *value = json!(v);
                                }
                                ui.end_row();
                            }
                            Value::Bool(b) => {
                                ui.checkbox(b, label);
                                ui.end_row();
                            }
                            Value::String(text) => {
                                ui.label(label);
                                let options: &[&str] = match key.as_str() {
                                    "door_side" => &["left", "right"],
                                    "railing" => &["frederiksberg", "norrebro", "vesterbro", "osterbro"],
                                    "finish" => &["galvanised", "white", "anthracite"],
                                    "deck" => &["cumaru", "fibre_concrete", "composite", "bamboo"],
                                    "screens" => &["none", "left", "right", "both"],
                                    _ => &[],
                                };
                                if options.is_empty() {
                                    ui.add(egui::TextEdit::singleline(text).desired_width(130.0));
                                } else {
                                    egui::ComboBox::from_id_salt(key).width(130.0).selected_text(text.as_str()).show_ui(ui, |ui| {
                                        for option in options {
                                            ui.selectable_value(text, option.to_string(), *option);
                                        }
                                    });
                                }
                                ui.end_row();
                            }
                            _ => {}
                        }
                    }
                });
            }
        });
        let exists = doc.types.iter().any(|t| t.id == draft.id);
        if ui
            .add_sized(
                [ui.available_width(), 30.0],
                egui::Button::new(if exists { "Update shared type and its instances" } else { "Create component type" }),
            )
            .clicked()
        {
            let mut params = serde_json::to_value(&*draft).unwrap_or(Value::Null);
            params["kind"] = self.kind.clone();
            run = Some(Run { command: "types.define", params });
        }
        ui.separator();
        theme::eyebrow(ui, "PLACE ON A FACADE");
        let facades = doc.buildings.iter().flat_map(|b| &b.facades).collect::<Vec<_>>();
        if !facades.iter().any(|f| f.id == self.facade) {
            self.facade = facades.first().map_or(String::new(), |f| f.id.clone());
        }
        egui::ComboBox::from_id_salt("place-facade").selected_text(&self.facade).show_ui(ui, |ui| {
            for f in &facades {
                ui.selectable_value(&mut self.facade, f.id.clone(), format!("{} · {:?}", f.id, f.kind));
            }
        });
        ui.horizontal(|ui| {
            ui.label("Bay axes");
            ui.add(egui::DragValue::new(&mut self.bay_count).range(1..=200));
            if ui.add_enabled(!self.facade.is_empty(), egui::Button::new("Set grid")).clicked() {
                run = Some(Run { command: "facade.set_bays", params: json!({"facade":self.facade,"count":self.bay_count}) });
            }
        });
        ui.weak(format!("Places on the current plan storey ({level})."));
        let has_bays = facades.iter().any(|f| f.id == self.facade && !f.bays.is_empty());
        if ui.add_enabled(exists && has_bays, egui::Button::new("Place type on this storey")).clicked() {
            run = Some(Run { command: "facade.place", params: json!({"facade":self.facade,"type":draft.id,"levels":[level]}) });
        }
        run
    }
}

#[cfg(test)]
mod library_tests {
    use super::*;
    #[test]
    fn wall_alignment_form_keeps_the_captured_reference_separate_from_the_picked_target() {
        let mut form = Authoring::default();
        assert!(form.align_walls(&["target.L1".into()]).is_none());
        form.reference = "reference.L1".into();
        assert!(form.align_walls(&["reference.L1".into()]).is_none());
        form.wall_reference_face = 1;
        form.wall_target_face = 2;
        let run = form.align_walls(&["target.L1".into()]).unwrap();
        assert_eq!(run.command, "edit.align_walls");
        assert_eq!(run.params, json!({"reference":"reference.L1","targets":["target.L1"],"reference_face":"left","target_face":"right"}));
        assert!(run.params.get("axis").is_none(), "wall face alignment must not fall back to bounding-box translation");
    }

    #[test]
    fn shared_type_scope_stays_readable_in_a_narrow_properties_column() {
        let doc = Document::default();
        let mut form = Authoring::default();
        form.choose_type(doc.types.first().unwrap());
        let ctx = egui::Context::default();
        theme::apply(&ctx);
        let mut output = ctx.run_ui(
            egui::RawInput { screen_rect: Some(egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(340.0, 1500.0))), ..Default::default() },
            |ui| {
                egui::CentralPanel::default().show(ui, |ui| {
                    form.family_editor(ui, &doc, 0);
                });
            },
        );
        output.textures_delta.clear();
        fn scope(shapes: &[egui::Shape]) -> Vec<(String, egui::Rect)> {
            shapes
                .iter()
                .flat_map(|shape| match shape {
                    egui::Shape::Text(t) => vec![(t.galley.text().to_string(), egui::Rect::from_min_size(t.pos, t.galley.size()))],
                    egui::Shape::Vec(v) => scope(v),
                    _ => Vec::new(),
                })
                .collect()
        }
        let labels = scope(&output.shapes.into_iter().map(|s| s.shape).collect::<Vec<_>>());
        let banner = labels.iter().find(|(s, _)| s.contains("Explicit instance overrides stay local")).unwrap();
        assert!(banner.1.right() <= 340.0 && banner.1.left() >= 0.0, "scope text must wrap inside the properties column");
        assert!(labels.iter().any(|(s, _)| s == "SHARED TYPE · PROJECT-WIDE"));
    }

    #[test]
    fn portable_library_actions_preserve_global_replace_opt_in_and_selected_type_filter() {
        let doc = Document::default();
        let mut form = Authoring::default();
        assert!(form.import_library().is_none());
        form.library_import_path = "/tmp/Æøå library.trigonlib".into();
        let import = form.import_library().unwrap();
        assert_eq!(import.command, "types.import");
        assert_eq!(import.params["replace"], false);
        form.library_replace = true;
        assert_eq!(form.import_library().unwrap().params["replace"], true);
        assert!(form.export_library().is_none(), "selected-only export needs a selected shared type");
        form.choose_type(doc.types.first().unwrap());
        let export = form.export_library().unwrap();
        assert_eq!(export.command, "types.export");
        assert_eq!(export.params["ids"], json!([doc.types[0].id]));
        form.library_selected_only = false;
        assert!(form.export_library().unwrap().params.get("ids").is_none());
        for invalid in ["", "library.json", "bad\nname.trigonlib", "https://example.invalid/library.trigonlib"] {
            form.library_import_path = invalid.into();
            assert!(form.import_library().is_none());
        }
        assert_eq!(doc, Document::default(), "form choices never mutate type definitions or instances");
    }
}
