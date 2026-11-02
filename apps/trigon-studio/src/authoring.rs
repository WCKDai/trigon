//! Typed authoring forms. They produce engine commands; no model is changed in this module.

use std::collections::BTreeSet;

use eframe::egui;
use serde_json::{Value, json};
use trigon_model::{Building, ComponentType, Document};

use crate::{
    icons::{self, Icon},
    scene::Element,
    theme,
};

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

#[derive(Clone, Copy, Default, PartialEq)]
enum WallReference {
    #[default]
    Interior,
    PlanGuide,
    Facade,
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
    wall_reference: WallReference,
    wall_guide: String,
    wall_facade: String,
    wall_level: Option<usize>,
    wall_max_move_mm: f64,
    copy_scope: Option<(String, Vec<trigon_model::doc::Storey>)>,
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
            wall_reference: WallReference::Interior,
            wall_guide: String::new(),
            wall_facade: String::new(),
            wall_level: None,
            wall_max_move_mm: 10.0,
            copy_scope: None,
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
            ui.weak("Local snapshots with a lowercase .trigonlib extension · no account required. Imported definitions become shared project types.");
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
            && std::path::Path::new(path).extension().and_then(|s| s.to_str()) == Some("trigonlib"))
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

    /// Entry points for the ribbon/browser. The form still only emits a read-only preflight;
    /// the application owns reviewing and applying its unchanged returned action/token.
    pub(crate) fn choose_wall_alignment(&mut self) {
        self.tab = Tab::Project;
        self.tool = Tool::Align;
        self.align_mode = 3;
    }

    pub(crate) fn choose_copy_to_levels(&mut self) {
        self.tab = Tab::Project;
        self.tool = Tool::Levels;
    }

    fn guide_level(doc: &Document, id: &str) -> Option<usize> {
        let mut annotations = doc.annotations.iter().filter(|annotation| annotation.id == id);
        let annotation = annotations.next()?;
        if annotations.next().is_some() || !matches!(annotation.kind, trigon_model::annotation::AnnotationKind::Line { .. }) {
            return None;
        }
        let mut views = doc.views.iter().filter(|view| view.id == annotation.view);
        let view = views.next()?;
        if views.next().is_some() {
            return None;
        }
        let trigon_model::annotation::DrawingViewKind::Plan { level } = view.kind else { return None };
        doc.buildings.iter().any(|building| building.storeys.get(level).is_some()).then_some(level)
    }

    fn material_building<'a>(&self, doc: &'a Document) -> Option<&'a Building> {
        let mut matches =
            doc.buildings.iter().flat_map(|building| building.facades.iter().filter(|facade| facade.id == self.wall_facade).map(move |_| building));
        let building = matches.next()?;
        matches.next().is_none().then_some(building)
    }

    /// Only establishes the owner for naming destination levels. The engine remains the
    /// authority for occurrence validity, slot collisions, phases and exact geometry.
    fn targets_building<'a>(doc: &'a Document, targets: &[String]) -> Option<&'a Building> {
        if targets.is_empty() || targets.len() > 256 {
            return None;
        }
        let mut expanded = Vec::new();
        for target in targets {
            if let Some(id) = target.strip_prefix("group:") {
                let mut groups = doc.groups.iter().filter(|group| group.id == id);
                let group = groups.next()?;
                if groups.next().is_some() || group.targets.iter().any(|target| target.starts_with("group:")) {
                    return None;
                }
                expanded.extend(group.targets.iter().map(String::as_str));
            } else {
                expanded.push(target.as_str());
            }
            if expanded.len() > 256 {
                return None;
            }
        }
        if expanded.is_empty() {
            return None;
        }
        let mut owner = None;
        for target in expanded {
            let source = |id: &str, levels: &[usize], kind: &str| {
                target == id
                    || target == format!("{kind}:{id}")
                    || target
                        .strip_prefix(&format!("{id}.L"))
                        .or_else(|| if kind == "room" { target.strip_prefix(&format!("{id}.space.L")) } else { None })
                        .and_then(|level| level.parse::<usize>().ok())
                        .is_some_and(|level| levels.contains(&level))
            };
            let mut matches = doc.buildings.iter().enumerate().filter(|(_, building)| {
                building.interior_walls.iter().any(|wall| source(&wall.id, &wall.levels, "wall"))
                    || building.stairs.iter().any(|stair| source(&stair.id, &stair.levels, "stair"))
                    || building.rooms.iter().any(|room| source(&room.id, &room.levels, "room"))
                    || building.facades.iter().any(|facade| {
                        let exact = target.strip_prefix(&format!("placement:{}:", facade.id)).and_then(|index| index.parse::<usize>().ok());
                        let native = target
                            .strip_prefix(&format!("{}.L", facade.id))
                            .and_then(|value| value.split_once(".B"))
                            .and_then(|(level, rest)| Some((level.parse::<usize>().ok()?, rest.split('.').next()?.parse::<usize>().ok()?)));
                        facade.placements.iter().enumerate().any(|(index, placement)| {
                            exact == Some(index) || native.is_some_and(|(level, bay)| placement.level == level && placement.bay == bay)
                        })
                    })
            });
            let (index, _) = matches.next()?;
            if matches.next().is_some() || owner.is_some_and(|owner| owner != index) {
                return None;
            }
            owner = Some(index);
        }
        doc.buildings.get(owner?)
    }

    fn align_walls(&self, doc: &Document, targets: &[String]) -> Option<Run> {
        if targets.is_empty() || !self.wall_max_move_mm.is_finite() || !(0.001..=100_000.0).contains(&self.wall_max_move_mm) {
            return None;
        }
        let faces = ["center", "left", "right"];
        let mut reference_face = *faces.get(self.wall_reference_face)?;
        let target_face = *faces.get(self.wall_target_face)?;
        let (reference, mut sanity) = match self.wall_reference {
            WallReference::Interior => {
                let reference = self.reference.trim();
                if reference.is_empty() || reference.len() > 512 || targets.iter().any(|id| id == reference) {
                    return None;
                }
                (json!(reference), None)
            }
            WallReference::PlanGuide => {
                let level = Self::guide_level(doc, &self.wall_guide)?;
                reference_face = "center";
                (json!({"kind":"annotation","id":self.wall_guide}), Some(json!({"levels":[level]})))
            }
            WallReference::Facade => {
                let building = self.material_building(doc)?;
                let level = self.wall_level?;
                building.storeys.get(level)?;
                (json!({"kind":"facade","id":self.wall_facade,"level":level}), Some(json!({"building":building.id,"levels":[level]})))
            }
        };
        if let Some(building) = Self::targets_building(doc, targets) {
            let scope = sanity.get_or_insert_with(|| json!({}));
            if scope.get("building").is_none() {
                scope["building"] = json!(building.id);
            }
        }
        let mut params = json!({"action":{"command":"edit.align_walls","params":{"targets":targets,"reference":reference,
            "reference_face":reference_face,"target_face":target_face,"max_move_mm":self.wall_max_move_mm}}});
        if let Some(mut sanity) = sanity {
            sanity["max_results"] = json!(200);
            params["sanity"] = sanity;
        }
        Some(Run { command: "precision.preview", params })
    }

    fn copy_building<'a>(&mut self, doc: &'a Document, targets: &[String]) -> Option<&'a Building> {
        let building = Self::targets_building(doc, targets);
        let scope = building.map(|building| (building.id.clone(), building.storeys.clone()));
        if self.copy_scope != scope {
            self.levels.clear();
            self.replace = false;
            self.copy_scope = scope;
        }
        building
    }

    fn copy_to_levels(&self, doc: &Document, targets: &[String]) -> Option<Run> {
        let building = Self::targets_building(doc, targets)?;
        let (owner, storeys) = self.copy_scope.as_ref()?;
        if *owner != building.id || *storeys != building.storeys || self.levels.is_empty() || self.levels.iter().any(|level| *level >= storeys.len())
        {
            return None;
        }
        Some(Run {
            command: "precision.preview",
            params: json!({"action":{"command":"edit.copy_to_levels",
            "params":{"targets":targets,"levels":self.levels,"replace":self.replace}},
            "sanity":{"building":building.id,"levels":self.levels,"max_results":200}}),
        })
    }

    fn wall_reference_controls(&mut self, ui: &mut egui::Ui, doc: &Document, current: Option<&Element>) {
        ui.label("Alignment reference");
        let label = match self.wall_reference {
            WallReference::Interior => "Interior wall",
            WallReference::PlanGuide => "Saved plan guide",
            WallReference::Facade => "Actual facade material",
        };
        egui::ComboBox::from_id_salt("wall-reference-kind").width(ui.available_width()).truncate().selected_text(label).show_ui(ui, |ui| {
            for (kind, label) in [
                (WallReference::Interior, "Interior wall"),
                (WallReference::PlanGuide, "Saved plan guide"),
                (WallReference::Facade, "Actual facade material"),
            ] {
                ui.selectable_value(&mut self.wall_reference, kind, label);
            }
        });
        match self.wall_reference {
            WallReference::Interior => {
                if ui.add_enabled(current.is_some_and(|element| element.kind == "interior wall"), egui::Button::new("Capture picked wall")).clicked()
                    && let Some(wall) = current
                {
                    self.reference = wall.id.clone();
                }
                ui.add(
                    egui::TextEdit::singleline(&mut self.reference)
                        .hint_text("Reference wall occurrence, e.g. core.L1")
                        .char_limit(512)
                        .desired_width(ui.available_width()),
                );
                ui.add(egui::Label::new("Capture the reference, then pick the targets. The reference stays in place.").wrap());
            }
            WallReference::PlanGuide => {
                let selected = doc
                    .annotations
                    .iter()
                    .find(|annotation| annotation.id == self.wall_guide)
                    .map_or("Choose a saved Plan line", |annotation| annotation.id.as_str());
                egui::ComboBox::from_id_salt("wall-plan-guide").width(ui.available_width()).truncate().selected_text(selected).show_ui(ui, |ui| {
                    let views: std::collections::BTreeMap<_, _> = doc
                        .views
                        .iter()
                        .filter_map(|view| match &view.kind {
                            trigon_model::annotation::DrawingViewKind::Plan { level } => Some((view.id.as_str(), (view.name.as_str(), *level))),
                            _ => None,
                        })
                        .collect();
                    let storeys = doc.buildings.iter().map(|building| building.storeys.len()).max().unwrap_or(0);
                    for annotation in &doc.annotations {
                        if matches!(annotation.kind, trigon_model::annotation::AnnotationKind::Line { .. })
                            && let Some((name, level)) = views.get(annotation.view.as_str())
                            && *level < storeys
                        {
                            ui.selectable_value(&mut self.wall_guide, annotation.id.clone(), format!("{name} · L{level} · {}", annotation.id));
                        }
                    }
                });
                if let Some(level) = Self::guide_level(doc, &self.wall_guide) {
                    ui.add(egui::Label::new(format!("Site XY guide · target occurrences on L{level}")).wrap());
                    egui::CollapsingHeader::new("Exact guide endpoints").show(ui, |ui| {
                        if let Some(annotation) = doc.annotations.iter().find(|annotation| annotation.id == self.wall_guide)
                            && let trigon_model::annotation::AnnotationKind::Line { from, to, .. } = &annotation.kind
                        {
                            for (name, point) in [("A", from), ("B", to)] {
                                ui.add(egui::Label::new(format!("{name} [{}, {}] mm", point[0], point[1])).wrap().selectable(true));
                            }
                        }
                    });
                } else {
                    ui.add(egui::Label::new("Save a Line in a Plan view first. Facade and section coordinates cannot guide plan walls.").wrap());
                }
                if ui.add(egui::Button::new("Create or edit guides in Drawings").wrap()).clicked() {
                    self.tab = Tab::Drawings;
                }
            }
            WallReference::Facade => {
                let picked = current.and_then(|element| {
                    let id = if doc.buildings.iter().any(|building| building.facades.iter().any(|facade| facade.id == element.id)) {
                        Some(element.id.as_str())
                    } else {
                        element.props.iter().find(|(key, _)| *key == "facade").map(|(_, id)| id.as_str())
                    }?;
                    Some((id, element.level))
                });
                if ui.add_enabled(picked.is_some(), egui::Button::new("Capture picked facade host")).clicked()
                    && let Some((id, level)) = picked
                {
                    self.wall_facade = id.to_string();
                    self.wall_level = level;
                }
                let before = self.wall_facade.clone();
                egui::ComboBox::from_id_salt("wall-material-facade")
                    .width(ui.available_width())
                    .truncate()
                    .selected_text(if self.wall_facade.is_empty() { "Choose a facade" } else { &self.wall_facade })
                    .show_ui(ui, |ui| {
                        for building in &doc.buildings {
                            for facade in &building.facades {
                                ui.selectable_value(&mut self.wall_facade, facade.id.clone(), format!("{} · {}", building.name, facade.id));
                            }
                        }
                    });
                if self.wall_facade != before {
                    self.wall_level = None;
                }
                if let Some(building) = self.material_building(doc) {
                    let label = self
                        .wall_level
                        .and_then(|level| building.storeys.get(level).map(|storey| format!("{} · L{level}", storey.name)))
                        .unwrap_or_else(|| "Choose the material's storey".into());
                    egui::ComboBox::from_id_salt("wall-material-level").width(ui.available_width()).truncate().selected_text(label).show_ui(
                        ui,
                        |ui| {
                            for (level, storey) in building.storeys.iter().enumerate() {
                                ui.selectable_value(&mut self.wall_level, Some(level), format!("{} · L{level}", storey.name))
                                    .on_hover_text(format!("Floor Z {} mm", storey.elevation));
                            }
                        },
                    );
                } else {
                    ui.add(
                        egui::Label::new(
                            "Choose one uniquely identified facade. Its actual material and selected storey are checked in the preview.",
                        )
                        .wrap(),
                    );
                }
            }
        }
        let target_labels = ["Centreline", "Left face", "Right face"];
        let reference_labels =
            if self.wall_reference == WallReference::Facade { ["Midline", "Inner material", "Outer material"] } else { target_labels };
        if self.wall_reference != WallReference::PlanGuide {
            ui.label("Reference face");
            egui::ComboBox::from_id_salt("wall-reference-face")
                .width(ui.available_width())
                .truncate()
                .selected_text(reference_labels.get(self.wall_reference_face).copied().unwrap_or("Choose a face"))
                .show_ui(ui, |ui| {
                    for (index, label) in reference_labels.iter().enumerate() {
                        ui.selectable_value(&mut self.wall_reference_face, index, *label);
                    }
                });
        }
        ui.label("Target wall face");
        egui::ComboBox::from_id_salt("wall-target-face")
            .width(ui.available_width())
            .truncate()
            .selected_text(target_labels.get(self.wall_target_face).copied().unwrap_or("Choose a face"))
            .show_ui(ui, |ui| {
                for (index, label) in target_labels.iter().enumerate() {
                    ui.selectable_value(&mut self.wall_target_face, index, *label);
                }
            });
        ui.label("Maximum endpoint movement");
        ui.add(egui::DragValue::new(&mut self.wall_max_move_mm).range(0.001..=100_000.0).speed(0.1).max_decimals(3).suffix(" mm"));
        ui.add(egui::Label::new("Choose a movement cap from your evidence; this is not a measurement tolerance.").wrap());
        ui.add(egui::Label::new("One-time alignment within 15°. Lengths and door distances along each wall stay fixed. Use exact storey occurrences for guides or facade material.").wrap());
    }

    pub fn ribbon(&mut self, ui: &mut egui::Ui) {
        for (tool, icon, label) in [
            (Tool::Move, Icon::Move, "Move"),
            (Tool::Copy, Icon::Copy, "Copy"),
            (Tool::Align, Icon::Align, "Align"),
            (Tool::Levels, Icon::Levels, "To levels"),
        ] {
            if icons::ribbon_button(ui, icon, label, self.tool == tool).clicked() {
                self.tool = tool;
            }
        }
        ui.selectable_value(&mut self.tool, Tool::Delete, "Delete");
    }

    pub(crate) fn command_controls_first(&self) -> bool {
        self.tab != Tab::Families && self.tool != Tool::Move
    }

    pub(crate) fn wall_alignment_active(&self) -> bool {
        self.tool == Tool::Align && self.align_mode == 3
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
        ui.add(
            egui::Label::new(
                egui::RichText::new(if self.targets.trim().is_empty() {
                    current.map_or("Pick an element", |e| e.id.as_str()).to_string()
                } else {
                    format!("{} selected targets", targets.len())
                })
                .strong(),
            )
            .wrap(),
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
        // Reference capture must remain usable when a facade host (rather than a target
        // interior wall) is picked. Only a valid editable target can request the preflight.
        ui.add_enabled_ui(editable || self.wall_alignment_active(), |ui| match self.tool {
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
                    _ => self.wall_reference_controls(ui, doc, current),
                }
                if self.align_mode == 3 {
                    let alignment = self.align_walls(doc, &targets);
                    if ui.add_enabled(editable && alignment.is_some(), egui::Button::new("Preview wall alignment").wrap()).clicked() {
                        run = alignment;
                    }
                    ui.add(egui::Label::new("Review the exact reference, endpoint changes, angle and affected levels before applying.").wrap());
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
                if let Some(building) = self.copy_building(doc, &targets) {
                    ui.add(egui::Label::new(format!("{} · {}",building.name,building.id)).wrap());
                    egui::ScrollArea::vertical().id_salt("copy-level-list").max_height(220.0).show(ui, |ui| {
                        for (level, storey) in building.storeys.iter().enumerate() {
                            let mut selected = self.levels.contains(&level);
                            if ui.checkbox(&mut selected, format!("{} · L{level}",storey.name))
                                .on_hover_text(format!("Floor Z {} mm · floor spacing {} mm",storey.elevation,storey.height)).changed()
                            {
                                if selected {self.levels.insert(level);} else {self.levels.remove(&level);}
                            }
                        }
                    });
                    ui.checkbox(&mut self.replace, "Replace matching destinations")
                        .on_hover_text("Explicitly replace matching geometry or facade slots; unrelated elements stay in place.");
                    ui.add(egui::Label::new("Source storeys are skipped. Plan coordinates stay fixed; destination heights come from the model's levels.").wrap());
                } else {
                    ui.add(egui::Label::new("Select identifiable elements from one building to choose its named storeys. Unknown or mixed-building sets need a scoped selection.").wrap());
                }
                let preview = self.copy_to_levels(doc, &targets);
                if ui.add_enabled(preview.is_some(),egui::Button::new("Preview copies to levels").wrap()).clicked() {
                    run = preview;
                }
                ui.add(egui::Label::new("Review destination collisions and exact source changes before applying. Changing the building or its level schedule clears these choices.").wrap());
            }
            Tool::Delete => {
                ui.weak("Removes the selected source instances in every phase. You can undo this batch.");
                if ui.button("Delete selection").clicked() {
                    run = Some(Run { command: "edit.delete", params: json!({"targets":targets}) });
                }
            }
        });
        if !editable {
            ui.add(
                egui::Label::new(if self.wall_alignment_active() {
                    "Capture the reference, then select the target interior-wall occurrences."
                } else {
                    "Select a window, balcony, interior wall, stair or saved group. Exterior wall hosts use their facade tools."
                })
                .wrap(),
            );
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
    fn wall_alignment_form_keeps_the_captured_reference_and_only_requests_a_preflight() {
        let doc = Document::default();
        let mut form = Authoring::default();
        assert!(form.align_walls(&doc, &["target.L1".into()]).is_none());
        form.reference = "reference.L1".into();
        assert!(form.align_walls(&doc, &["reference.L1".into()]).is_none());
        form.wall_reference_face = 1;
        form.wall_target_face = 2;
        let run = form.align_walls(&doc, &["target.L1".into()]).unwrap();
        assert_eq!(run.command, "precision.preview");
        assert_eq!(
            run.params["action"],
            json!({"command":"edit.align_walls","params":{
            "reference":"reference.L1","targets":["target.L1"],"reference_face":"left","target_face":"right","max_move_mm":10.0}})
        );
        assert!(run.params["action"]["params"].get("axis").is_none(), "wall face alignment must not fall back to bounding-box translation");
        assert!(run.params.get("preview_token").is_none(), "the form cannot invent an Apply token");
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
        for invalid in
            ["", "library.json", "library.TRIGONLIB", "library.TrigonLib", "bad\nname.trigonlib", "https://example.invalid/library.trigonlib"]
        {
            form.library_import_path = invalid.into();
            form.library_export_path = invalid.into();
            assert!(form.import_library().is_none(), "Import must not enable a path the engine rejects: {invalid:?}");
            assert!(form.export_library().is_none(), "Export must not enable a path the engine rejects: {invalid:?}");
        }
        assert_eq!(doc, Document::default(), "form choices never mutate type definitions or instances");
    }

    fn precision_document() -> Document {
        let mut doc = Document::default();
        let mut building =
            Building::new("b", "Public UI fixture", &[[0.0, 0.0], [12000.0, 0.0], [12000.0, 8000.0], [0.0, 8000.0]], 3, 3000.0).unwrap();
        building
            .interior_walls
            .push(serde_json::from_value(json!({"id":"target","levels":[1,2],"a":[1000,2050],"b":[5000,2250],"thickness":180})).unwrap());
        doc.buildings.push(building);
        doc.views.push(serde_json::from_value(json!({"id":"coord","name":"Coordination","kind":{"view":"plan","level":1}})).unwrap());
        doc.annotations.push(
            serde_json::from_value(json!({"id":"axis","view":"coord","kind":{"annotation":"line","from":[0.125,2000.375],"to":[12000.625,2010.875],
            "stroke":{"colour":[184,65,44],"width_mm":0.13,"pattern":"dotted"}}}))
            .unwrap(),
        );
        doc
    }

    #[test]
    fn precision_form_plan_guides_use_explicit_datum_objects_with_exact_occurrences_and_movement_bound() {
        let doc = precision_document();
        let before = doc.clone();
        let mut form = Authoring { wall_reference: WallReference::PlanGuide, ..Default::default() };
        form.wall_guide = "axis".into();
        form.wall_reference_face = 1; // A stale interior choice must never offset a guide.
        form.wall_target_face = 2;
        form.wall_max_move_mm = 2.75;
        let run = form.align_walls(&doc, &["target.L1".into()]).unwrap();
        assert_eq!(run.command, "precision.preview");
        assert_eq!(
            run.params["action"],
            json!({"command":"edit.align_walls","params":{"targets":["target.L1"],
            "reference":{"kind":"annotation","id":"axis"},"reference_face":"center","target_face":"right","max_move_mm":2.75}})
        );
        assert_eq!(run.params["sanity"], json!({"building":"b","levels":[1],"max_results":200}));
        assert_eq!(doc, before, "control choices cannot move walls or rewrite measured guide endpoints");
        for invalid in [0.0, -1.0, f64::NAN, f64::INFINITY, 100_000.1] {
            form.wall_max_move_mm = invalid;
            assert!(form.align_walls(&doc, &["target.L1".into()]).is_none());
        }
    }

    #[test]
    fn precision_form_only_admits_saved_plan_lines_as_guides() {
        let mut doc = precision_document();
        let mut form = Authoring { wall_reference: WallReference::PlanGuide, ..Default::default() };
        for (id, kind) in [("facade", json!({"view":"facade","facade":"b.f1"})), ("section", json!({"view":"section","from":[0,0],"to":[12000,0]}))] {
            doc.views.push(serde_json::from_value(json!({"id":id,"name":id,"kind":kind})).unwrap());
            doc.annotations
                .push(serde_json::from_value(json!({"id":id,"view":id,"kind":{"annotation":"line","from":[0,2000],"to":[12000,2000]}})).unwrap());
            form.wall_guide = id.into();
            assert!(form.align_walls(&doc, &["target.L1".into()]).is_none());
        }
        doc.annotations
            .push(serde_json::from_value(json!({"id":"text","view":"coord","kind":{"annotation":"text","at":[0,0],"text":"Not a line"}})).unwrap());
        for id in ["text", "missing"] {
            form.wall_guide = id.into();
            assert!(form.align_walls(&doc, &["target.L1".into()]).is_none());
        }
        form.wall_guide = "axis".into();
        assert!(form.align_walls(&doc, &["target.L1".into()]).is_some());
        doc.annotations.push(doc.annotations[0].clone());
        assert!(form.align_walls(&doc, &["target.L1".into()]).is_none(), "ambiguous guides cannot preflight");
    }

    #[test]
    fn precision_form_material_reference_requires_an_explicit_real_storey_and_never_uses_an_interior_alias() {
        let doc = precision_document();
        let mut form = Authoring { wall_reference: WallReference::Facade, ..Default::default() };
        form.wall_facade = "b.f1".into();
        form.reference = "facade:b.f1:L1".into();
        assert!(form.align_walls(&doc, &["target.L1".into()]).is_none(), "no guessed current or first storey");
        form.wall_level = Some(1);
        form.wall_reference_face = 1;
        let run = form.align_walls(&doc, &["target.L1".into()]).unwrap();
        assert_eq!(run.command, "precision.preview");
        assert_eq!(run.params["action"]["params"]["reference"], json!({"kind":"facade","id":"b.f1","level":1}));
        assert_eq!(run.params["action"]["params"]["reference_face"], "left");
        assert_eq!(run.params["sanity"], json!({"building":"b","levels":[1],"max_results":200}));
        form.wall_level = Some(999);
        assert!(form.align_walls(&doc, &["target.L1".into()]).is_none());
        form.wall_level = Some(1);
        form.wall_facade = "missing".into();
        assert!(form.align_walls(&doc, &["target.L1".into()]).is_none());
    }

    #[test]
    fn precision_form_level_copy_has_no_direct_mutation_bypass_and_keeps_replace_explicit() {
        let doc = precision_document();
        let before = doc.clone();
        let targets = ["target.L1".into()];
        let mut form = Authoring::default();
        assert!(form.copy_to_levels(&doc, &targets).is_none());
        assert_eq!(form.copy_building(&doc, &targets).unwrap().id, "b");
        assert!(form.levels.is_empty(), "no destinations selected implicitly");
        form.levels.insert(0);
        let run = form.copy_to_levels(&doc, &targets).unwrap();
        assert_eq!(run.command, "precision.preview");
        assert_eq!(run.params["action"], json!({"command":"edit.copy_to_levels","params":{"targets":["target.L1"],"levels":[0],"replace":false}}));
        assert_eq!(run.params["sanity"], json!({"building":"b","levels":[0],"max_results":200}));
        form.replace = true;
        assert_eq!(form.copy_to_levels(&doc, &targets).unwrap().params["action"]["params"]["replace"], true);
        assert_eq!(doc, before);
        form.levels.insert(999);
        assert!(form.copy_to_levels(&doc, &targets).is_none());
    }

    #[test]
    fn precision_form_copy_owner_is_not_the_first_building_and_level_schedule_changes_clear_choices() {
        let mut doc = precision_document();
        let mut other =
            Building::new("other", "Another building", &[[20000.0, 0.0], [32000.0, 0.0], [32000.0, 8000.0], [20000.0, 8000.0]], 2, 3400.0).unwrap();
        other
            .interior_walls
            .push(serde_json::from_value(json!({"id":"other-wall","levels":[1],"a":[22000,2000],"b":[26000,2000],"thickness":180})).unwrap());
        doc.buildings.insert(0, other);
        let mut form = Authoring::default();
        assert_eq!(form.copy_building(&doc, &["target.L1".into()]).unwrap().id, "b");
        form.levels.insert(0);
        form.replace = true;
        doc.buildings[1].storeys[0].elevation = -50.5;
        assert!(form.copy_to_levels(&doc, &["target.L1".into()]).is_none(), "stale named level choices cannot preflight");
        form.copy_building(&doc, &["target.L1".into()]);
        assert!(form.levels.is_empty());
        assert!(!form.replace);
        form.levels.insert(0);
        form.copy_building(&doc, &["other-wall.L1".into()]);
        assert!(form.levels.is_empty(), "moving to another building requires new destination choices");
        assert!(form.copy_building(&doc, &["target.L1".into(), "other-wall.L1".into()]).is_none());
        assert!(form.copy_building(&doc, &["unknown".into()]).is_none());
    }

    #[test]
    fn precision_form_named_level_scope_supports_flat_groups_and_refuses_ambiguous_building_ownership() {
        let mut doc = precision_document();
        doc.groups.push(serde_json::from_value(json!({"id":"g","name":"Wall group","targets":["target.L1"]})).unwrap());
        assert_eq!(Authoring::targets_building(&doc, &["group:g".into()]).unwrap().id, "b");
        doc.groups[0].targets = vec!["group:g".into()];
        assert!(Authoring::targets_building(&doc, &["group:g".into()]).is_none());
        let mut duplicate = doc.buildings[0].clone();
        duplicate.id = "other".into();
        doc.buildings.push(duplicate);
        assert!(Authoring::targets_building(&doc, &["target.L1".into()]).is_none());
    }

    #[test]
    fn precision_form_entry_points_select_the_actual_authoring_flows() {
        let mut form = Authoring { tab: Tab::Families, ..Default::default() };
        form.choose_wall_alignment();
        assert!(form.tab == Tab::Project && form.wall_alignment_active() && form.command_controls_first());
        form.choose_copy_to_levels();
        assert!(form.tab == Tab::Project && form.tool == Tool::Levels && form.command_controls_first());
    }

    #[test]
    fn precision_form_primary_controls_fit_narrow_columns_for_all_reference_modes_and_copy() {
        fn text(shapes: &[egui::Shape]) -> Vec<(String, egui::Rect)> {
            shapes
                .iter()
                .flat_map(|shape| match shape {
                    egui::Shape::Text(text) => vec![(text.galley.text().to_string(), egui::Rect::from_min_size(text.pos, text.galley.size()))],
                    egui::Shape::Vec(shapes) => text(shapes),
                    _ => Vec::new(),
                })
                .collect()
        }
        let doc = precision_document();
        for width in [240.0, 340.0, 780.0] {
            for mode in [0, 1, 2, 3] {
                let mut form = Authoring { targets: "target.L1".into(), ..Default::default() };
                if mode == 3 {
                    form.choose_copy_to_levels();
                    form.copy_building(&doc, &["target.L1".into()]);
                    form.levels.insert(0);
                } else {
                    form.choose_wall_alignment();
                    form.wall_reference = [WallReference::Interior, WallReference::PlanGuide, WallReference::Facade][mode];
                    form.reference = "reference.L1".into();
                    form.wall_guide = "axis".into();
                    form.wall_facade = "b.f1".into();
                    form.wall_level = Some(1);
                }
                let ctx = egui::Context::default();
                theme::apply(&ctx);
                let mut output = ctx.run_ui(
                    egui::RawInput {
                        screen_rect: Some(egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(width, 2200.0))),
                        ..Default::default()
                    },
                    |ui| {
                        egui::CentralPanel::default().show(ui, |ui| {
                            assert!(form.ui(ui, &doc, None, 0).is_none());
                        });
                    },
                );
                output.textures_delta.clear();
                let labels = text(&output.shapes.into_iter().map(|shape| shape.shape).collect::<Vec<_>>());
                let button = if mode == 3 { "Preview copies to levels" } else { "Preview wall alignment" };
                let bounds = labels.iter().find(|(label, _)| label == button).unwrap().1;
                assert!(bounds.left() >= 0.0 && bounds.right() <= width, "{button} overflows {width} px: {bounds:?}");
                for (label, bounds) in &labels {
                    if ["Alignment reference", "Target wall face", "Maximum endpoint movement"].contains(&label.as_str()) {
                        assert!(bounds.left() >= 0.0 && bounds.right() <= width, "{label} overflows {width} px");
                    }
                }
                assert!(!labels.iter().any(|(label, _)| label == "Align wall selection" || label == "Copy to storeys"));
            }
        }
    }
}
