//! Typed authoring forms. They produce engine commands; no model is changed in this module.

use std::collections::BTreeSet;

use eframe::egui;
use serde_json::{Value, json};
use trigon_model::component::{Deck, Finish, Muntins, Railing, Screens, Side, WindowShape};
use trigon_model::{Building, ComponentKind, ComponentType, Document};

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

#[derive(Clone, Copy, Default, PartialEq)]
enum FamilyCategory {
    #[default]
    All,
    Windows,
    Doors,
    Balconies,
    Guards,
}

impl FamilyCategory {
    const CHOICES: [(Self, &'static str); 5] = [
        (Self::All, "All categories"),
        (Self::Windows, "Windows"),
        (Self::Doors, "Doors"),
        (Self::Balconies, "Balconies"),
        (Self::Guards, "Guards & railings"),
    ];

    fn matches(self, ty: &ComponentType) -> bool {
        match self {
            Self::All => true,
            Self::Windows => matches!(ty.kind, ComponentKind::Window { .. } | ComponentKind::FixedWindow | ComponentKind::HistoricWindow { .. }),
            Self::Doors => matches!(
                ty.kind,
                ComponentKind::EntranceDoor | ComponentKind::PanelDoor { .. } | ComponentKind::FrenchDoor { .. } | ComponentKind::DoorWindow { .. }
            ),
            Self::Balconies => matches!(
                ty.kind,
                ComponentKind::FrenchBalcony { .. }
                    | ComponentKind::Balcony { .. }
                    | ComponentKind::BalconySlab { .. }
                    | ComponentKind::ShapedBalcony { .. }
                    | ComponentKind::SteelBalcony { .. }
                    | ComponentKind::Loggia { .. }
            ),
            Self::Guards => matches!(ty.kind, ComponentKind::GuardRail { .. }),
        }
    }
}

pub struct Run {
    pub command: &'static str,
    pub params: Value,
}

#[derive(Clone, PartialEq)]
struct PlacementScope {
    facade: String,
    storeys: Vec<trigon_model::Storey>,
    bays: Vec<f64>,
    phase: Option<String>,
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
    family_category: FamilyCategory,
    draft: Option<ComponentType>,
    draft_source: Option<ComponentType>,
    draft_is_new: bool,
    pending_type: Option<String>,
    show_placement: bool,
    facade: String,
    bay_count: usize,
    place_level: Option<usize>,
    place_bay: usize,
    place_all_bays: bool,
    place_replace: bool,
    place_scope: Option<PlacementScope>,
    place_scope_error: Option<String>,
    place_command_error: Option<String>,
    place_overrides: bool,
    place_sizes: [f64; 3],
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
            family_category: FamilyCategory::All,
            draft: None,
            draft_source: None,
            draft_is_new: false,
            pending_type: None,
            show_placement: false,
            facade: String::new(),
            bay_count: 5,
            place_level: None,
            place_bay: 0,
            place_all_bays: false,
            place_replace: false,
            place_scope: None,
            place_scope_error: None,
            place_command_error: None,
            place_overrides: false,
            place_sizes: [1200.0, 1500.0, 850.0],
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
    match ty.kind {
        ComponentKind::Window { .. } => "Casement window",
        ComponentKind::FixedWindow => "Fixed window",
        ComponentKind::HistoricWindow { .. } => "Traditional window",
        ComponentKind::PanelDoor { .. } => "Panel door",
        ComponentKind::EntranceDoor => "Entrance door",
        ComponentKind::FrenchDoor { .. } => "French door",
        ComponentKind::FrenchBalcony { .. } => "French balcony",
        ComponentKind::Balcony { .. } => "Cantilever balcony",
        ComponentKind::DoorWindow { .. } => "Door & window unit",
        ComponentKind::BalconySlab { .. } => "Balcony slab",
        ComponentKind::ShapedBalcony { .. } => "Shaped balcony",
        ComponentKind::Loggia { .. } => "Loggia",
        ComponentKind::SteelBalcony { .. } => "Steel balcony",
        ComponentKind::GuardRail { .. } => "Guard rail",
    }
    .into()
}

fn family_mm(ui: &mut egui::Ui, label: &str, value: &mut f64, min: f64, max: f64) {
    ui.add(egui::Label::new(label).wrap());
    ui.add(egui::DragValue::new(value).speed(1.0).range(min..=max.max(min)).max_decimals(2).suffix(" mm"));
    ui.end_row();
}

fn family_count(ui: &mut egui::Ui, label: &str, value: &mut u32, min: u32, max: u32) {
    ui.add(egui::Label::new(label).wrap());
    ui.add(egui::DragValue::new(value).range(min..=max));
    ui.end_row();
}

fn family_choice<T: Copy + PartialEq>(ui: &mut egui::Ui, id: &str, label: &str, value: &mut T, choices: &[(T, &str)]) {
    ui.add(egui::Label::new(label).wrap());
    let selected = choices.iter().find(|(choice, _)| choice == value).map_or("Choose", |(_, label)| *label);
    egui::ComboBox::from_id_salt(id).width(100.0).truncate().selected_text(selected).show_ui(ui, |ui| {
        for (choice, label) in choices {
            ui.selectable_value(value, *choice, *label);
        }
    });
    ui.end_row();
}

fn railing_fields(ui: &mut egui::Ui, railing: &mut Railing, finish: &mut Finish, handrail: &mut bool) {
    family_choice(
        ui,
        "family-railing",
        "Railing style",
        railing,
        &[
            (Railing::Frederiksberg, "Frederiksberg"),
            (Railing::Norrebro, "Nørrebro"),
            (Railing::Vesterbro, "Vesterbro"),
            (Railing::Osterbro, "Østerbro"),
        ],
    );
    family_choice(
        ui,
        "family-finish",
        "Steel finish",
        finish,
        &[(Finish::Galvanised, "Galvanised"), (Finish::White, "White"), (Finish::Anthracite, "Anthracite")],
    );
    ui.label("Timber handrail");
    ui.checkbox(handrail, "Include");
    ui.end_row();
}

fn family_parameters(ui: &mut egui::Ui, draft: &mut ComponentType) {
    let (width, height) = (draft.width, draft.height);
    egui::Grid::new("family-parameters").num_columns(2).max_col_width((ui.available_width() * 0.47).max(70.0)).spacing([6.0, 6.0]).show(ui, |ui| {
        match &mut draft.kind {
            ComponentKind::Window { lights, top_light } => {
                family_count(ui, "Window sashes", lights, 1, 8);
                family_mm(ui, "Top-light height", top_light, 0.0, height / 2.0);
            }
            ComponentKind::HistoricWindow { shape, lights, top_light, panes_across, panes_down, muntins, muntin_width_mm, operable } => {
                let mut profile = match shape {
                    WindowShape::Rectangular => 0,
                    WindowShape::Segmental { .. } => 1,
                    WindowShape::Round => 2,
                };
                let before = profile;
                family_choice(ui, "window-profile", "Opening profile", &mut profile, &[(0, "Rectangular"), (1, "Arched top"), (2, "Round")]);
                if before != profile {
                    *shape = match profile {
                        1 => WindowShape::Segmental { rise_mm: 150.0_f64.min(width / 2.0).min(height * 0.45) },
                        2 => WindowShape::Round,
                        _ => WindowShape::Rectangular,
                    };
                }
                if let WindowShape::Segmental { rise_mm } = shape {
                    family_mm(ui, "Arch rise", rise_mm, 20.0, (width / 2.0).min(height * 0.45));
                }
                family_count(ui, "Window sashes", lights, 1, 4);
                family_mm(ui, "Top-light height", top_light, 0.0, height / 2.0);
                family_count(ui, "Pane columns per sash", panes_across, 1, 6);
                family_count(ui, "Pane rows per sash", panes_down, 1, 8);
                family_choice(
                    ui,
                    "window-muntins",
                    "Glazing bars",
                    muntins,
                    &[(Muntins::TrueDivided, "True divided panes"), (Muntins::Applied, "Applied bars")],
                );
                family_mm(ui, "Glazing bar width", muntin_width_mm, 8.0, 60.0);
                ui.label("Opening sashes");
                ui.checkbox(operable, "Operable");
                ui.end_row();
            }
            ComponentKind::PanelDoor { leaves, panel_columns, panel_rows, glazed_rows } => {
                family_count(ui, "Door leaves", leaves, 1, 2);
                family_count(ui, "Panel columns", panel_columns, 1, 3);
                family_count(ui, "Panel rows", panel_rows, 1, 6);
                family_count(ui, "Glazed upper rows", glazed_rows, 0, *panel_rows);
            }
            ComponentKind::FrenchDoor { leaves } => family_count(ui, "Door leaves", leaves, 1, 4),
            ComponentKind::FrenchBalcony { leaves, rail_height } => {
                family_count(ui, "Door leaves", leaves, 1, 4);
                family_mm(ui, "Guard height", rail_height, 600.0, 1500.0);
            }
            ComponentKind::Balcony { leaves, depth, slab_width, rail_height } => {
                family_count(ui, "Door leaves", leaves, 1, 4);
                family_mm(ui, "Balcony depth", depth, 300.0, 3000.0);
                family_mm(ui, "Balcony slab width", slab_width, width, 12000.0);
                family_mm(ui, "Guard height", rail_height, 900.0, 1500.0);
            }
            ComponentKind::DoorWindow { door_width, door_side, window_sill, lights } => {
                family_mm(ui, "Door leaf width", door_width, 500.0, width - 300.0);
                family_choice(ui, "door-side", "Door side from outside", door_side, &[(Side::Left, "Left"), (Side::Right, "Right")]);
                family_mm(ui, "Window sill above opening", window_sill, 0.0, height - 400.0);
                family_count(ui, "Window sashes", lights, 1, 4);
            }
            ComponentKind::BalconySlab { depth, thickness } => {
                family_mm(ui, "Balcony depth", depth, 300.0, 3000.0);
                family_mm(ui, "Slab thickness", thickness, 60.0, 400.0);
            }
            ComponentKind::ShapedBalcony { depth, thickness, plan } => {
                family_mm(ui, "Balcony depth", depth, 400.0, 3000.0);
                family_mm(ui, "Slab thickness", thickness, 80.0, 400.0);
                match plan {
                    trigon_model::balcony::Plan::Bowed { return_fraction } => {
                        ui.label("Straight return / depth");
                        let mut percent = *return_fraction * 100.0;
                        if ui.add(egui::DragValue::new(&mut percent).range(10.0..=70.0).suffix(" %")).changed() {
                            *return_fraction = percent / 100.0;
                        }
                        ui.end_row();
                    }
                    trigon_model::balcony::Plan::Chamfered { corner_mm } => {
                        family_mm(ui, "Chamfered corner", corner_mm, 50.0, (width / 3.0).min(*depth * 0.8))
                    }
                    trigon_model::balcony::Plan::Polygon { points } => {
                        for (index, point) in points.iter_mut().enumerate() {
                            for (axis, label, min, max) in [(0, "width", -50.0, 50.0), (1, "depth", 0.0, 100.0)] {
                                ui.add(egui::Label::new(format!("Point {} · {label}", index + 1)).wrap());
                                let mut percent = point[axis] * 100.0;
                                if ui.add(egui::DragValue::new(&mut percent).range(min..=max).suffix(" %")).changed() {
                                    point[axis] = percent / 100.0;
                                }
                                ui.end_row();
                            }
                        }
                    }
                }
            }
            ComponentKind::Loggia { depth, rail_height, door_width, door_side } => {
                family_mm(ui, "Recess depth", depth, 300.0, 4000.0);
                family_mm(ui, "Guard height", rail_height, 600.0, 1500.0);
                family_mm(ui, "Door leaf width", door_width, 500.0, (width - 300.0).max(500.0));
                family_choice(ui, "loggia-door-side", "Door side from outside", door_side, &[(Side::Left, "Left"), (Side::Right, "Right")]);
            }
            ComponentKind::SteelBalcony { depth, railing, finish, deck, handrail, screens, tie_rods } => {
                family_mm(ui, "Balcony depth", depth, 300.0, 3000.0);
                railing_fields(ui, railing, finish, handrail);
                family_choice(
                    ui,
                    "balcony-deck",
                    "Deck material",
                    deck,
                    &[
                        (Deck::Cumaru, "Cumaru timber"),
                        (Deck::FibreConcrete, "Fibre concrete"),
                        (Deck::Composite, "Composite"),
                        (Deck::Bamboo, "Bamboo"),
                    ],
                );
                family_choice(
                    ui,
                    "balcony-screens",
                    "Privacy screens",
                    screens,
                    &[(Screens::None, "None"), (Screens::Left, "Left"), (Screens::Right, "Right"), (Screens::Both, "Both sides")],
                );
                ui.label("Support");
                ui.checkbox(tie_rods, "Tie rods");
                ui.end_row();
            }
            ComponentKind::GuardRail { projection, railing, finish, handrail } => {
                family_mm(ui, "Projection to guard centre", projection, if *handrail { 35.0 } else { 20.0 }, 3000.0);
                railing_fields(ui, railing, finish, handrail);
            }
            ComponentKind::FixedWindow | ComponentKind::EntranceDoor => {
                ui.weak("This family uses the dimensions above.");
                ui.end_row();
            }
        }
    });
}

fn family_facade_label(building: &Building, facade: &trigon_model::doc::Facade) -> String {
    let kind = match facade.kind {
        trigon_model::doc::FacadeKind::Unknown => "Facade",
        trigon_model::doc::FacadeKind::Street => "Street facade",
        trigon_model::doc::FacadeKind::Courtyard => "Courtyard facade",
        trigon_model::doc::FacadeKind::Party => "Party wall",
    };
    format!("{} · {kind} {}", building.name, facade.edge.saturating_add(1))
}

impl Authoring {
    pub fn choose_type(&mut self, ty: &ComponentType) {
        self.draft = Some(ty.clone());
        self.draft_source = Some(ty.clone());
        self.draft_is_new = false;
        self.pending_type = None;
        self.place_sizes = [ty.width, ty.height, ty.sill];
        self.place_overrides = false;
        self.place_replace = false;
        self.place_command_error = None;
        self.tab = Tab::Families;
    }

    fn type_dirty(&self) -> bool {
        self.draft.is_some() && (self.draft_is_new || self.draft != self.draft_source)
    }

    fn request_type(&mut self, ty: &ComponentType) {
        self.tab = Tab::Families;
        if self.draft.as_ref().is_some_and(|draft| draft.id == ty.id) && !self.draft_is_new {
            return;
        }
        if self.type_dirty() {
            self.pending_type = Some(ty.id.clone());
        } else {
            self.choose_type(ty);
        }
    }

    /// Choosing another type never silently discards an edited form.
    pub(crate) fn choose_instance_type(&mut self, doc: &Document, current: &Element) -> bool {
        let mut ids = current.props.iter().filter(|(key, _)| *key == "type");
        let Some((_, id)) = ids.next() else { return false };
        if ids.next().is_some() {
            return false;
        }
        let mut types = doc.types.iter().filter(|ty| ty.id == *id);
        let Some(ty) = types.next() else { return false };
        if types.next().is_some() {
            return false;
        }
        self.request_type(ty);
        true
    }

    pub(crate) fn begin_family_placement(&mut self, doc: &Document, current: Option<&Element>) {
        if let Some(current) = current {
            self.choose_instance_type(doc, current);
        }
        if self.draft.is_none()
            && let Some(ty) = doc.types.first()
        {
            self.choose_type(ty);
        }
        self.tab = Tab::Families;
        self.show_placement = true;
        self.place_replace = false;
        self.place_command_error = None;
        if let Some(current) = current
            && !self.pick_family_host(doc, current, None)
        {
            self.facade.clear();
            self.place_level = None;
            self.place_scope = None;
            self.place_scope_error = Some("Pick a facade bay or choose its host and storey explicitly.".into());
        }
    }

    pub(crate) fn family_placement_active(&self) -> bool {
        self.tab == Tab::Families && self.show_placement
    }

    pub(crate) fn acknowledge_family_placement(&mut self, error: Option<String>) {
        self.place_command_error = error;
    }

    pub(crate) fn family_placement_feedback(&self, doc: &Document) -> Value {
        let stale = self.place_scope.is_some() && self.place_scope != self.current_placement_scope(doc);
        json!({"active":self.family_placement_active(),"type_id":self.draft.as_ref().map(|draft|&draft.id),
            "scope":{"facade":self.facade,"level":self.place_level,"bay":self.place_bay,
                "all_bays":self.place_all_bays,"phase":doc.phases.get(doc.active_index()).map(|phase|&phase.id)},
            "ready":self.family_placement_active() && self.place_type(doc).is_some(),"stale":stale,
            "error":self.place_scope_error.as_deref().or(stale.then_some("The placement source changed; choose its host and storey again.")).or(self.place_command_error.as_deref()),
            "command_error":self.place_command_error,
            "replace_existing":self.place_replace,"instance_overrides":self.place_overrides,
            "units":"mm"})
    }

    /// A viewport hit only fills the placement form. `point` is site-local XY in millimetres,
    /// including the building's translation/rotation. Wall picks retain the explicitly chosen
    /// storey; a picked component supplies its own storey. The explicit Place button commits.
    pub(crate) fn pick_family_host(&mut self, doc: &Document, current: &Element, point: Option<[f64; 2]>) -> bool {
        if !self.family_placement_active() {
            return false;
        }
        self.sync_placement_scope(doc);
        let facade_id = if current.host && current.kind == "wall" {
            current.id.as_str()
        } else if matches!(current.kind, "window" | "door" | "balcony" | "railing") {
            let mut props = current.props.iter().filter(|(key, _)| *key == "facade");
            let Some((_, id)) = props.next() else { return false };
            if props.next().is_some() {
                return false;
            }
            id.as_str()
        } else {
            return false;
        };
        let mut facades = doc
            .buildings
            .iter()
            .flat_map(|building| building.facades.iter().filter(|facade| facade.id == facade_id).map(move |facade| (building, facade)));
        let Some((building, facade)) = facades.next() else { return false };
        if facades.next().is_some() || facade.bays.is_empty() {
            return false;
        }
        let Some(level) = current.level.or(self.place_level) else { return false };
        if building.storeys.get(level).is_none() {
            return false;
        }
        let bay = if let Some(point) = point {
            if point.iter().any(|value| !value.is_finite()) {
                return false;
            }
            let Some((a, b)) = building.edge(facade.edge) else { return false };
            let Some(frame) = trigon_model::build::FacadeFrame::new(a, b) else { return false };
            let u = (point[0] - frame.origin[0]) * frame.t[0] + (point[1] - frame.origin[1]) * frame.t[1];
            if !u.is_finite() || !frame.length.is_finite() || facade.bays.iter().any(|bay| !bay.is_finite() || *bay < 0.0 || *bay > frame.length) {
                return false;
            }
            let mut nearest = None;
            let mut distance = f64::INFINITY;
            let mut tied = false;
            for (index, centre) in facade.bays.iter().enumerate() {
                let delta = (u - centre).abs();
                if delta + 1e-6 < distance {
                    nearest = Some(index);
                    distance = delta;
                    tied = false;
                } else if (delta - distance).abs() <= 1e-6 {
                    tied = true;
                }
            }
            let Some(nearest) = nearest.filter(|_| !tied) else { return false };
            nearest
        } else {
            let Some(occurrence) = current.id.strip_prefix(&format!("{}.L{level}.B", facade.id)) else { return false };
            let index = occurrence.strip_suffix(".balcony").unwrap_or(occurrence);
            let Ok(index) = index.parse::<usize>() else { return false };
            if facade.bays.get(index).is_none() {
                return false;
            }
            index
        };
        self.facade = facade.id.clone();
        self.place_level = Some(level);
        self.place_bay = bay;
        self.place_all_bays = false;
        self.place_replace = false;
        self.accept_placement_scope(doc);
        true
    }

    pub(crate) fn duplicate_selected_type(&mut self, doc: &Document) -> bool {
        if self.type_dirty() {
            return false;
        }
        let Some(source) = &self.draft else { return false };
        if !doc.types.iter().any(|ty| ty == source) {
            return false;
        }
        let mut id_base = source.id.clone();
        while id_base.len() > 108 {
            id_base.pop();
        }
        let mut name_base = source.name.clone();
        while name_base.len() > 236 {
            name_base.pop();
        }
        for n in 1..=10_000 {
            let id = format!("{id_base}.copy{n}");
            let name = format!("{name_base} copy {n}");
            if !doc.types.iter().any(|ty| ty.id == id || ty.name == name) {
                let mut copy = source.clone();
                copy.id = id;
                copy.name = name;
                self.draft = Some(copy);
                self.draft_is_new = true;
                self.pending_type = None;
                self.place_replace = false;
                return true;
            }
        }
        false
    }

    fn cancel_type_changes(&mut self, doc: &Document) {
        let id = self.pending_type.take().or_else(|| self.draft_source.as_ref().map(|ty| ty.id.clone()));
        if let Some(ty) = id.and_then(|id| doc.types.iter().find(|ty| ty.id == id)) {
            self.choose_type(ty);
        } else {
            self.draft = None;
            self.draft_source = None;
            self.draft_is_new = false;
        }
    }

    /// The document is authoritative: a successful command acknowledges its draft on the next
    /// frame. A failed command leaves the draft available for correction or cancellation.
    fn sync_type_draft(&mut self, doc: &Document) {
        if let Some(draft) = &self.draft
            && doc.types.iter().filter(|ty| ty.id == draft.id).count() == 1
            && doc.types.iter().any(|ty| ty == draft)
        {
            self.draft_source = Some(draft.clone());
            self.draft_is_new = false;
        }
        if !self.type_dirty()
            && let Some(id) = self.pending_type.take()
            && let Some(ty) = doc.types.iter().find(|ty| ty.id == id)
        {
            self.choose_type(ty);
        }
    }

    fn type_error(&self, doc: &Document) -> Option<String> {
        let draft = self.draft.as_ref()?;
        for (label, value, max) in [("Type name", draft.name.as_str(), 256), ("Type ID", draft.id.as_str(), 128)] {
            if value.trim().is_empty() || value.len() > max || value.chars().any(char::is_control) {
                return Some(format!("{label} needs 1–{max} characters without line breaks."));
            }
        }
        if self.draft_is_new {
            if doc.types.iter().any(|ty| ty.id == draft.id) {
                return Some("This type ID is already in use. Choose a distinct ID for the copy.".into());
            }
            if doc.types.iter().any(|ty| ty.name == draft.name) {
                return Some("Choose a distinct name for the new type.".into());
            }
        } else {
            let Some(source) = &self.draft_source else { return Some("Choose a saved type first.".into()) };
            let mut matches = doc.types.iter().filter(|ty| ty.id == source.id);
            if matches.next() != Some(source) || matches.next().is_some() || draft.id != source.id {
                return Some("The saved type changed. Cancel to reload it before editing.".into());
            }
        }
        draft.validate().err().map(|error| error.to_string())
    }

    fn define_type(&self, doc: &Document) -> Option<Run> {
        if !self.type_dirty() || self.type_error(doc).is_some() {
            return None;
        }
        Some(Run { command: "types.define", params: serde_json::to_value(self.draft.as_ref()?).ok()? })
    }

    fn type_use_count(doc: &Document, id: &str) -> usize {
        doc.buildings
            .iter()
            .map(|building| {
                building.facades.iter().flat_map(|facade| &facade.placements).filter(|placement| placement.type_id == id).count()
                    + building.roof.dormers().iter().filter(|dormer| dormer.window_type == id).count()
                    + building.roof.rooflights().iter().filter(|rooflight| rooflight.window_type == id).count()
            })
            .sum()
    }

    pub fn library(&mut self, ui: &mut egui::Ui, doc: &Document) -> Option<Run> {
        self.sync_type_draft(doc);
        ui.style_mut().wrap_mode = Some(egui::TextWrapMode::Wrap);
        let mut run = None;
        egui::ComboBox::from_id_salt("family-category")
            .width(ui.available_width())
            .truncate()
            .selected_text(
                FamilyCategory::CHOICES.iter().find(|(category, _)| *category == self.family_category).map_or("All categories", |(_, label)| *label),
            )
            .show_ui(ui, |ui| {
                for (category, label) in FamilyCategory::CHOICES {
                    ui.selectable_value(&mut self.family_category, category, label);
                }
            });
        ui.add(egui::TextEdit::singleline(&mut self.search).hint_text("Find a family or type").desired_width(f32::INFINITY));
        let search = self.search.to_lowercase();
        theme::eyebrow(ui, "COMPONENT TYPES");
        let mut shown = 0;
        egui::ScrollArea::vertical().id_salt("family-type-list").max_height(330.0).show(ui, |ui| {
            for ty in &doc.types {
                if !self.family_category.matches(ty) || !format!("{} {} {}", ty.id, ty.name, family_name(ty)).to_lowercase().contains(&search) {
                    continue;
                }
                shown += 1;
                let selected = self.draft.as_ref().is_some_and(|d| d.id == ty.id);
                if ui
                    .add_sized(
                        [ui.available_width(), 40.0],
                        egui::Button::new(format!("{}\n{} · {:.0} × {:.0} mm", ty.name, family_name(ty), ty.width, ty.height))
                            .selected(selected)
                            .wrap(),
                    )
                    .on_hover_text(format!("{}\n{} linked instances across all phases", ty.id, Self::type_use_count(doc, &ty.id)))
                    .clicked()
                {
                    self.request_type(ty);
                }
            }
        });
        if shown == 0 {
            ui.weak("No matching types. Clear the search or choose another category.");
        } else {
            ui.weak(format!("{shown} of {} project types", doc.types.len()));
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
                ui.weak(self.draft_source.as_ref().map_or("Choose a saved type to export", |t| t.name.as_str()));
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
            params["ids"] = json!([self.draft_source.as_ref()?.id]);
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

    pub fn ribbon(&mut self, ui: &mut egui::Ui) -> bool {
        let mut chosen = false;
        egui::Grid::new("modify-ribbon").num_columns(3).spacing(egui::vec2(4.0, 2.0)).show(ui, |ui| {
            for (i, (tool, icon, label)) in [
                (Tool::Move, Icon::Move, "Move"),
                (Tool::Copy, Icon::Copy, "Copy"),
                (Tool::Align, Icon::Align, "Align"),
                (Tool::Levels, Icon::Levels, "To levels"),
            ]
            .into_iter()
            .enumerate()
            {
                if icons::ribbon_button(ui, icon, label, self.tool == tool).clicked() {
                    self.tool = tool;
                    chosen = true;
                }
                if i == 2 {
                    ui.end_row();
                }
            }
            if ui.add(egui::Button::new("Delete").frame(false).selected(self.tool == Tool::Delete).min_size(egui::vec2(100.0, 28.0))).clicked() {
                self.tool = Tool::Delete;
                chosen = true;
            }
        });
        chosen
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

    fn placement_facade<'a>(&self, doc: &'a Document) -> Option<(&'a Building, &'a trigon_model::doc::Facade)> {
        let mut matches = doc
            .buildings
            .iter()
            .flat_map(|building| building.facades.iter().filter(|facade| facade.id == self.facade).map(move |facade| (building, facade)));
        let result = matches.next()?;
        matches.next().is_none().then_some(result)
    }

    fn placement_counts(&self, doc: &Document) -> Option<(usize, usize)> {
        if self.type_dirty()
            || self.type_error(doc).is_some()
            || self.place_scope_error.is_some()
            || self.place_scope.is_none()
            || self.place_scope != self.current_placement_scope(doc)
        {
            return None;
        }
        let draft = self.draft.as_ref()?;
        let (building, facade) = self.placement_facade(doc)?;
        let level = self.place_level?;
        building.storeys.get(level)?;
        if facade.bays.is_empty() || (!self.place_all_bays && self.place_bay >= facade.bays.len()) {
            return None;
        }
        let mut occupied = BTreeSet::new();
        for placement in &facade.placements {
            if placement.level != level || (!self.place_all_bays && placement.bay != self.place_bay) {
                continue;
            }
            let (created, demolished) = doc.lifetime(placement).ok()?;
            let ty = doc.component(&placement.type_id).ok()?;
            if created <= doc.active_index() && doc.active_index() < demolished && ty.kind.is_opening() == draft.kind.is_opening() {
                occupied.insert(placement.bay);
            }
        }
        Some((if self.place_all_bays { facade.bays.len() } else { 1 }, occupied.len()))
    }

    fn place_type(&self, doc: &Document) -> Option<Run> {
        let (_, occupied) = self.placement_counts(doc)?;
        if occupied > 0 && !self.place_replace {
            return None;
        }
        let draft = self.draft.as_ref()?;
        let (_, facade) = self.placement_facade(doc)?;
        let bays = if self.place_all_bays { (0..facade.bays.len()).collect::<Vec<_>>() } else { vec![self.place_bay] };
        let mut params = json!({"facade":self.facade,"type":draft.id,"levels":[self.place_level?],"bays":bays});
        if self.place_overrides {
            let mut instance = draft.clone();
            [instance.width, instance.height, instance.sill] = self.place_sizes;
            instance.validate().ok()?;
            params["width"] = json!(instance.width);
            params["height"] = json!(instance.height);
            params["sill"] = json!(instance.sill);
        }
        Some(Run { command: "facade.place", params })
    }

    fn current_placement_scope(&self, doc: &Document) -> Option<PlacementScope> {
        let (building, facade) = self.placement_facade(doc)?;
        building.storeys.get(self.place_level?)?;
        Some(PlacementScope {
            facade: facade.id.clone(),
            storeys: building.storeys.clone(),
            bays: facade.bays.clone(),
            phase: doc.phases.get(doc.active_index()).map(|phase| phase.id.clone()),
        })
    }

    fn accept_placement_scope(&mut self, doc: &Document) {
        self.place_scope = self.current_placement_scope(doc);
        self.place_scope_error = None;
        self.place_command_error = None;
        self.place_replace = false;
    }

    pub(crate) fn sync_placement_scope(&mut self, doc: &Document) {
        if self.place_scope.is_some() && self.place_scope != self.current_placement_scope(doc) {
            self.place_level = None;
            self.place_replace = false;
            self.place_scope_error = Some(
                "The placement host, storey schedule, bay grid or active phase changed. Pick a host or choose its storey again before placing."
                    .into(),
            );
        }
    }

    fn family_placement(&mut self, ui: &mut egui::Ui, doc: &Document, level: usize) -> Option<Run> {
        self.sync_placement_scope(doc);
        let mut run = None;
        let response = egui::CollapsingHeader::new("Place component").id_salt("family-placement").open(Some(self.show_placement)).show(ui, |ui| {
            let facades = doc.buildings.iter().flat_map(|building| building.facades.iter().map(move |facade| (building, facade))).collect::<Vec<_>>();
            if facades.is_empty() {
                ui.weak("Create a building and facade before placing a component.");
                return;
            }
            ui.weak(
                "Pick a facade or component in the work area, then Place. Picks snap to existing bay axes; wall picks use the storey chosen below.",
            );
            ui.label("Host facade");
            let mut host_chosen = false;
            let facade_name =
                self.placement_facade(doc).map_or_else(|| "Choose facade".into(), |(building, facade)| family_facade_label(building, facade));
            egui::ComboBox::from_id_salt("place-facade").width(ui.available_width()).truncate().selected_text(facade_name).show_ui(ui, |ui| {
                for (building, facade) in &facades {
                    host_chosen |= ui
                        .selectable_value(&mut self.facade, facade.id.clone(), family_facade_label(building, facade))
                        .on_hover_text(&facade.id)
                        .clicked();
                }
            });
            if host_chosen {
                self.place_level = self.placement_facade(doc).and_then(|(building, _)| building.storeys.get(level).map(|_| level));
                self.place_bay = 0;
                self.accept_placement_scope(doc);
            }
            let Some((building, facade)) = self.placement_facade(doc) else {
                ui.colored_label(
                    theme::ACCENT,
                    if self.facade.is_empty() {
                        "Choose a host facade before placing a component.".into()
                    } else {
                        format!("Host {} is missing or ambiguous. Choose a uniquely named facade.", self.facade)
                    },
                );
                return;
            };
            if let Some(error) = &self.place_scope_error {
                ui.colored_label(theme::ACCENT, error);
            }
            if let Some(error) = &self.place_command_error {
                ui.colored_label(theme::ACCENT, error);
            }
            ui.label("Storey");
            let mut storey_chosen = false;
            let storey_name = self
                .place_level
                .and_then(|index| building.storeys.get(index).map(|storey| format!("{} · L{index}", storey.name)))
                .unwrap_or_else(|| "Choose storey".into());
            egui::ComboBox::from_id_salt("place-level").width(ui.available_width()).truncate().selected_text(storey_name).show_ui(ui, |ui| {
                for (index, storey) in building.storeys.iter().enumerate() {
                    storey_chosen |= ui.selectable_value(&mut self.place_level, Some(index), format!("{} · L{index}", storey.name)).clicked();
                }
            });
            if storey_chosen {
                self.accept_placement_scope(doc);
            }
            if self.place_bay >= facade.bays.len() {
                self.place_bay = 0;
                self.place_replace = false;
            }
            ui.label("Placement scope");
            let before = self.place_all_bays;
            ui.checkbox(&mut self.place_all_bays, "All bays on this storey");
            if self.place_all_bays != before {
                self.place_replace = false;
            }
            if !self.place_all_bays && !facade.bays.is_empty() {
                let before = self.place_bay;
                egui::ComboBox::from_id_salt("place-bay")
                    .width(ui.available_width())
                    .truncate()
                    .selected_text(format!("Bay {} of {}", self.place_bay + 1, facade.bays.len()))
                    .show_ui(ui, |ui| {
                        for (index, position) in facade.bays.iter().enumerate() {
                            ui.selectable_value(&mut self.place_bay, index, format!("Bay {} · {:.0} mm along facade", index + 1, position));
                        }
                    });
                if self.place_bay != before {
                    self.place_replace = false;
                }
            }
            ui.checkbox(&mut self.place_overrides, "Override new instance sizes");
            if self.place_overrides {
                ui.weak("These sizes apply to the chosen slots. The shared type stays unchanged.");
                egui::Grid::new("place-sizes").max_col_width(ui.available_width() * 0.47).show(ui, |ui| {
                    family_mm(ui, "Instance width", &mut self.place_sizes[0], 300.0, 6000.0);
                    family_mm(ui, "Instance height", &mut self.place_sizes[1], 300.0, 4000.0);
                    family_mm(ui, "Instance sill", &mut self.place_sizes[2], 0.0, 3000.0);
                });
            } else {
                ui.weak("New instances inherit the saved type dimensions.");
            }
            if self.type_dirty() {
                ui.colored_label(theme::ACCENT, "Apply or cancel the type draft before placement.");
            } else if let Some((count, occupied)) = self.placement_counts(doc) {
                ui.weak(format!("{count} {} · active phase", if count == 1 { "instance" } else { "instances" }));
                if occupied > 0 {
                    ui.colored_label(theme::ACCENT, format!("{occupied} occupied {}", if occupied == 1 { "slot" } else { "slots" }));
                    ui.checkbox(&mut self.place_replace, "Replace existing in these slots");
                    ui.weak("Replacement uses the chosen type sizes and bay centres. Earlier-phase instances are demolished in the active phase.");
                }
            }
            let placement = self.place_type(doc);
            if ui.add_enabled(placement.is_some(), egui::Button::new("Place component").wrap()).clicked() {
                run = placement;
                self.place_replace = false;
            }
            egui::CollapsingHeader::new("Facade bay grid").id_salt("family-bay-grid").show(ui, |ui| {
                ui.weak("Changing the shared facade grid affects every storey. Reducing the count removes placements beyond its end.");
                ui.horizontal(|ui| {
                    ui.label("Bay count");
                    ui.add(egui::DragValue::new(&mut self.bay_count).range(1..=200));
                });
                if ui.button("Update facade grid").clicked() {
                    run = Some(Run { command: "facade.set_bays", params: json!({"facade":self.facade,"count":self.bay_count}) });
                    self.place_replace = false;
                }
            });
        });
        if response.header_response.clicked() {
            self.show_placement = !self.show_placement;
        }
        run
    }

    fn family_editor(&mut self, ui: &mut egui::Ui, doc: &Document, level: usize) -> Option<Run> {
        self.sync_type_draft(doc);
        ui.style_mut().wrap_mode = Some(egui::TextWrapMode::Wrap);
        let Some(draft) = &self.draft else {
            theme::eyebrow(ui, "FAMILIES & TYPES");
            ui.label("Choose a type from the component browser, or edit the type of a picked component.");
            return None;
        };
        theme::eyebrow(ui, &family_name(draft).to_uppercase());
        ui.add(egui::Label::new(egui::RichText::new(&draft.name).heading()).wrap());
        ui.weak(format!("{:.0} × {:.0} mm · sill {:.0} mm", draft.width, draft.height, draft.sill));
        let count = if self.draft_is_new { 0 } else { Self::type_use_count(doc, &draft.id) };
        egui::Frame::new().fill(theme::CHROME).inner_margin(8.0).corner_radius(3).show(ui, |ui| {
            theme::eyebrow(ui, if self.draft_is_new { "NEW TYPE · UNSAVED COPY" } else { "SHARED TYPE · PROJECT-WIDE" });
            ui.label(if self.draft_is_new {
                "Apply creates a separate type. Existing instances keep their original type.".to_string()
            } else {
                format!(
                    "{count} linked {} across all phases. Apply updates every reference. Explicit instance overrides stay local.",
                    if count == 1 { "instance" } else { "instances" }
                )
            });
        });
        if let Some(id) = &self.pending_type {
            let name = doc.types.iter().find(|ty| ty.id == *id).map_or(id.as_str(), |ty| ty.name.as_str());
            ui.colored_label(theme::ACCENT, format!("Apply or cancel this draft to switch to {name}."));
        }
        let dirty = self.type_dirty();
        if ui
            .add_enabled(!dirty, egui::Button::new("Duplicate type…"))
            .on_hover_text("Creates an unsaved copy with a distinct name and ID. Apply the copy before placing it.")
            .clicked()
        {
            self.duplicate_selected_type(doc);
        }
        let Some(draft) = &mut self.draft else { return None };
        ui.label("Type name");
        ui.add(egui::TextEdit::singleline(&mut draft.name).char_limit(256).desired_width(f32::INFINITY));
        egui::Grid::new("family-sizes").max_col_width(ui.available_width() * 0.47).spacing([6.0, 6.0]).show(ui, |ui| {
            family_mm(ui, "Width", &mut draft.width, 300.0, 6000.0);
            let guard = matches!(
                draft.kind,
                ComponentKind::BalconySlab { .. }
                    | ComponentKind::ShapedBalcony { .. }
                    | ComponentKind::SteelBalcony { .. }
                    | ComponentKind::GuardRail { .. }
            );
            family_mm(ui, if guard { "Guard height" } else { "Height" }, &mut draft.height, 300.0, 4000.0);
            family_mm(ui, "Sill above floor", &mut draft.sill, 0.0, 3000.0);
        });
        egui::CollapsingHeader::new("Family parameters").default_open(true).show(ui, |ui| family_parameters(ui, draft));
        egui::CollapsingHeader::new("Type identity").id_salt("family-identity").show(ui, |ui| {
            ui.weak("The stable ID links model instances to this definition.");
            if self.draft_is_new {
                ui.label("New type ID");
                ui.add(egui::TextEdit::singleline(&mut draft.id).char_limit(128).desired_width(f32::INFINITY));
            } else {
                ui.add(egui::Label::new(&draft.id).wrap());
            }
        });
        let error = self.type_error(doc);
        if let Some(error) = &error {
            ui.colored_label(theme::ACCENT, error);
        }
        let apply = self.define_type(doc);
        let dirty = self.type_dirty();
        let mut run = None;
        ui.horizontal_wrapped(|ui| {
            if ui.add_enabled(apply.is_some(), egui::Button::new(if self.draft_is_new { "Apply new type" } else { "Apply type" })).clicked() {
                run = apply;
            }
            if ui.add_enabled(dirty || self.pending_type.is_some() || error.is_some(), egui::Button::new("Cancel")).clicked() {
                self.cancel_type_changes(doc);
            }
            if dirty {
                ui.weak("Unsaved draft");
            } else {
                ui.weak("Saved type");
            }
        });
        ui.separator();
        if let Some(placement) = self.family_placement(ui, doc, level) {
            run = Some(placement);
        }
        run
    }
}

#[cfg(test)]
mod library_tests {
    use super::*;

    fn family_document() -> Document {
        let mut doc = Document::default();
        let mut building =
            Building::new("family-test", "Family UI fixture", &[[0.0, 0.0], [12000.0, 0.0], [12000.0, 8000.0], [0.0, 8000.0]], 3, 3000.0).unwrap();
        building.facades[0].bays = vec![2500.0, 6000.0, 9500.0];
        doc.buildings.push(building);
        doc
    }

    fn family_session(doc: Document) -> trigon_engine::Session {
        let mut session = trigon_engine::Session::new(Box::new(trigon_data::Live::new(trigon_data::Credentials::default())));
        session.doc = doc;
        session
    }

    #[test]
    fn duplicate_type_has_a_distinct_stable_identity_and_never_changes_the_original() {
        let mut doc = family_document();
        let source = doc.types[0].clone();
        let mut first = source.clone();
        first.id = format!("{}.copy1", source.id);
        first.name = format!("{} copy 1", source.name);
        doc.types.push(first);
        let mut occupied_name = source.clone();
        occupied_name.id = "another-type".into();
        occupied_name.name = format!("{} copy 2", source.name);
        doc.types.push(occupied_name);
        let before = doc.clone();
        let mut form = Authoring::default();
        form.choose_type(&source);
        assert!(form.duplicate_selected_type(&doc));
        let copy = form.draft.as_ref().unwrap().clone();
        assert_eq!(copy.id, format!("{}.copy3", source.id));
        assert_eq!(copy.name, format!("{} copy 3", source.name));
        assert_eq!(doc, before, "starting a copy never writes the model");
        assert!(!form.duplicate_selected_type(&doc), "another click cannot discard an unsaved copy");
        let run = form.define_type(&doc).unwrap();
        assert_eq!(run.command, "types.define");
        assert_eq!(serde_json::from_value::<ComponentType>(run.params.clone()).unwrap(), copy);
        let mut session = family_session(doc);
        session.execute(run.command, run.params).unwrap();
        assert_eq!(session.doc.component(&source.id).unwrap(), &source);
        assert_eq!(session.doc.component(&copy.id).unwrap(), &copy);
        form.sync_type_draft(&session.doc);
        assert!(!form.type_dirty(), "successful Apply acknowledges only the saved definition");
        session.execute("edit.undo", json!({})).unwrap();
        assert_eq!(session.doc.types, before.types);
    }

    #[test]
    fn duplicate_keeps_unicode_identity_within_limits_and_rejects_late_collisions() {
        let mut doc = Document::default();
        doc.types[0].id = "æ".repeat(60);
        doc.types[0].name = "窓".repeat(85);
        let source = doc.types[0].clone();
        let mut form = Authoring::default();
        form.choose_type(&source);
        assert!(form.duplicate_selected_type(&doc));
        let copy = form.draft.as_ref().unwrap().clone();
        assert!(copy.id.len() <= 128 && copy.name.len() <= 256);
        assert!(form.define_type(&doc).is_some());
        let mut collision = copy.clone();
        collision.width += 50.0;
        doc.types.push(collision);
        assert!(form.define_type(&doc).is_none(), "a late ID collision must not turn a new copy into a shared replacement");
        form.cancel_type_changes(&doc);
        assert_eq!(form.draft, Some(source));
        assert!(!form.type_dirty());
    }

    #[test]
    fn family_drafts_require_apply_and_cancel_protects_selection_changes() {
        let doc = Document::default();
        let before = doc.clone();
        let mut form = Authoring::default();
        form.choose_type(&doc.types[0]);
        assert!(form.define_type(&doc).is_none(), "an unchanged form cannot create a redundant history entry");
        form.draft.as_mut().unwrap().width = 900.0;
        form.request_type(&doc.types[1]);
        assert_eq!(form.draft.as_ref().unwrap().id, doc.types[0].id, "switching types must not discard the edited draft");
        assert_eq!(form.pending_type.as_deref(), Some(doc.types[1].id.as_str()));
        let run = form.define_type(&doc).unwrap();
        assert_eq!(run.params["width"], 900.0);
        assert_eq!(doc, before, "even preparing Apply is a read-only operation");
        form.cancel_type_changes(&doc);
        assert_eq!(form.draft.as_ref(), Some(&doc.types[1]));
        assert!(form.pending_type.is_none());
        form.draft.as_mut().unwrap().width = f64::NAN;
        assert!(form.define_type(&doc).is_none(), "hostile nonfinite dimensions cannot produce a command");
        form.cancel_type_changes(&doc);
        assert_eq!(form.draft.as_ref(), Some(&doc.types[1]));
    }

    #[test]
    fn stale_type_drafts_do_not_overwrite_updates_from_another_command() {
        let mut doc = Document::default();
        let mut form = Authoring::default();
        form.choose_type(&doc.types[0]);
        form.draft.as_mut().unwrap().width = 900.0;
        doc.types[0].name = "Updated elsewhere".into();
        form.sync_type_draft(&doc);
        assert!(form.define_type(&doc).is_none());
        assert!(form.type_error(&doc).unwrap().contains("saved type changed"));
        form.cancel_type_changes(&doc);
        assert_eq!(form.draft.as_ref(), Some(&doc.types[0]));
    }

    #[test]
    fn rejected_shared_type_apply_keeps_the_document_and_the_editable_draft() {
        let doc = family_document();
        let mut session = family_session(doc);
        session.execute("facade.place", json!({"facade":"family-test.f1","type":"window.casement.1","levels":[0],"bays":[0,1]})).unwrap();
        let before = session.doc.clone();
        let mut form = Authoring::default();
        form.choose_type(&session.doc.types[0]);
        form.draft.as_mut().unwrap().width = 6000.0;
        let run = form.define_type(&session.doc).unwrap();
        assert!(session.execute(run.command, run.params).is_err(), "overlapping or out-of-host windows must be rejected by the shared engine");
        assert_eq!(session.doc, before);
        form.sync_type_draft(&session.doc);
        assert_eq!(form.draft.as_ref().unwrap().width, 6000.0);
        assert!(form.type_dirty(), "a rejected engine command must not acknowledge or erase the draft");
        form.cancel_type_changes(&session.doc);
        assert_eq!(form.draft.as_ref(), Some(&before.types[0]));
    }

    #[test]
    fn placement_defaults_to_one_explicit_slot_and_keeps_instance_overrides_local() {
        let doc = family_document();
        let before = doc.clone();
        let mut form = Authoring::default();
        form.begin_family_placement(&doc, None);
        let mut host = family_host_element();
        host.level = Some(1);
        assert!(form.pick_family_host(&doc, &host, Some([2500.0, 0.0])));
        let run = form.place_type(&doc).unwrap();
        assert_eq!(run.command, "facade.place");
        assert_eq!(run.params["levels"], json!([1]));
        assert_eq!(run.params["bays"], json!([0]));
        assert!(run.params.get("width").is_none(), "inherited sizes must stay linked to the saved type");
        form.place_overrides = true;
        form.place_sizes = [950.0, 1600.0, 700.0];
        let run = form.place_type(&doc).unwrap();
        let mut session = family_session(doc);
        session.execute(run.command, run.params).unwrap();
        assert_eq!(session.doc.types, before.types);
        let placement = &session.doc.buildings[0].facades[0].placements[0];
        assert_eq!((placement.width, placement.height, placement.sill), (Some(950.0), Some(1600.0), Some(700.0)));
        assert_eq!(placement.level, 1);
        assert_eq!(placement.bay, 0);
        assert!(form.place_type(&session.doc).is_none(), "replacing an occupied slot requires a fresh opt-in");
        form.place_replace = true;
        assert!(form.place_type(&session.doc).is_some());
        form.place_all_bays = true;
        assert_eq!(form.place_type(&session.doc).unwrap().params["bays"], json!([0, 1, 2]));
        form.place_sizes[1] = f64::INFINITY;
        assert!(form.place_type(&session.doc).is_none());
        form.place_sizes[1] = 1500.0;
        form.place_level = Some(99);
        assert!(form.place_type(&session.doc).is_none(), "an out-of-building storey must never become an unscoped command");
    }

    #[test]
    fn family_type_filters_and_reference_count_follow_real_component_families() {
        let mut doc = family_document();
        let window = doc.types[0].clone();
        assert!(FamilyCategory::Windows.matches(&window));
        assert!(!FamilyCategory::Doors.matches(&window));
        let balcony = doc.types.iter().find(|ty| matches!(ty.kind, ComponentKind::FrenchBalcony { .. })).unwrap();
        assert!(FamilyCategory::Balconies.matches(balcony));
        assert!(!FamilyCategory::Doors.matches(balcony));
        let placement = serde_json::from_value(json!({"type":window.id,"level":0,"bay":0})).unwrap();
        doc.buildings[0].facades[0].placements.push(placement);
        let dormer = json!({"id":"d1","facade":"family-test.f1","u_mm":6000,"window_type":window.id,"width_mm":700,"height_mm":1000});
        doc.buildings[0].roof = serde_json::from_value(json!({"kind":"mansard","dormers":[dormer]})).unwrap();
        assert_eq!(
            Authoring::type_use_count(&doc, &window.id),
            2,
            "dormer references belong to the shared scope even though their dimensions are local"
        );
    }
    #[test]
    fn picked_instance_routes_to_its_saved_type_without_discarding_another_draft() {
        let doc = family_document();
        let element = Element {
            id: "family-test.f1.L1.B1".into(),
            kind: "window",
            level: Some(1),
            props: vec![("type", doc.types[1].id.clone()), ("facade", "family-test.f1".into())],
            ranges: Vec::new(),
            fingerprint: 0,
            host: false,
            bounds: None,
            facing: None,
        };
        let mut form = Authoring::default();
        form.begin_family_placement(&doc, Some(&element));
        assert!(form.show_placement);
        assert_eq!(form.draft.as_ref(), Some(&doc.types[1]));
        assert_eq!(form.facade, "family-test.f1");
        assert_eq!(form.place_level, Some(1));
        form.choose_type(&doc.types[0]);
        form.draft.as_mut().unwrap().height = 1700.0;
        assert!(form.choose_instance_type(&doc, &element));
        assert_eq!(form.draft.as_ref().unwrap().height, 1700.0);
        assert_eq!(form.pending_type.as_deref(), Some(doc.types[1].id.as_str()));
        form.cancel_type_changes(&doc);
        assert_eq!(form.draft.as_ref(), Some(&doc.types[1]));
        let mut ambiguous = doc.clone();
        ambiguous.types.push(doc.types[1].clone());
        assert!(!form.choose_instance_type(&ambiguous, &element));
    }

    fn family_host_element() -> Element {
        Element {
            id: "family-test.f1".into(),
            kind: "wall",
            level: None,
            // Exterior wall `facade` metadata describes its classification, not its ID.
            props: vec![("facade", "street".into())],
            ranges: Vec::new(),
            fingerprint: 0,
            host: true,
            bounds: None,
            facing: None,
        }
    }

    #[test]
    fn vanished_component_host_never_substitutes_the_first_remaining_facade() {
        let mut doc = family_document();
        let mut other = Building::new("other", "Other", &[[20000.0, 0.0], [32000.0, 0.0], [32000.0, 8000.0], [20000.0, 8000.0]], 3, 3000.0).unwrap();
        other.facades[0].bays = vec![2500.0];
        doc.buildings.push(other);
        let mut host = family_host_element();
        host.id = "other.f1".into();
        host.level = Some(0);
        let mut form = Authoring::default();
        form.begin_family_placement(&doc, None);
        assert!(form.pick_family_host(&doc, &host, Some([22500.0, 0.0])));
        doc.buildings.pop();
        let ctx = egui::Context::default();
        let mut output = ctx.run_ui(egui::RawInput::default(), |ui| {
            form.family_placement(ui, &doc, 0);
        });
        output.textures_delta.clear();
        ctx.data_mut(|data| data.clear());
        assert_eq!(form.facade, "other.f1", "keep the missing identity visible; do not choose another host");
        assert!(form.place_type(&doc).is_none());
    }

    #[test]
    fn basement_insertion_invalidates_a_reviewed_component_storey_scope() {
        let mut session = family_session(family_document());
        let mut form = Authoring::default();
        form.begin_family_placement(&session.doc, None);
        let mut host = family_host_element();
        host.level = Some(0);
        assert!(form.pick_family_host(&session.doc, &host, Some([2500.0, 0.0])));
        assert!(form.place_type(&session.doc).is_some());
        session.execute("building.add_basement", json!({"building":"family-test","floor":-2500})).unwrap();
        assert!(form.place_type(&session.doc).is_none(), "L0 changed identity; require renewed scope selection");
        assert_eq!(form.family_placement_feedback(&session.doc)["stale"], true);
        assert_eq!(form.family_placement_feedback(&session.doc)["ready"], false);
        form.sync_placement_scope(&session.doc);
        assert!(form.place_level.is_none());
        host.level = Some(1);
        assert!(form.pick_family_host(&session.doc, &host, Some([2500.0, 0.0])));
        let before = session.doc.clone();
        let run = form.place_type(&session.doc).unwrap();
        assert_eq!(run.params["levels"], json!([1]));
        assert!(!form.place_replace);
        session.execute(run.command, run.params).unwrap();
        session.execute("edit.undo", json!({})).unwrap();
        assert_eq!(session.doc, before);
    }

    #[test]
    fn hidden_properties_bare_wall_pick_cannot_renew_a_stale_storey_index() {
        let mut session = family_session(family_document());
        let mut form = Authoring::default();
        form.begin_family_placement(&session.doc, None);
        let scene = crate::scene::Scene::from_built(&trigon_model::build::build(&session.doc).unwrap(), false);
        let host = scene.elements.iter().find(|element| element.id == "family-test.f1").unwrap();
        assert_eq!(host.level, None, "real facade walls span storeys");
        form.place_level = Some(0); // Explicit original ground-floor choice.
        assert!(form.pick_family_host(&session.doc, host, Some([2500.0, 0.0])));
        session.execute("building.add_basement", json!({"building":"family-test","floor":-2500})).unwrap();
        let rebuilt = crate::scene::Scene::from_built(&trigon_model::build::build(&session.doc).unwrap(), false);
        let host = rebuilt.elements.iter().find(|element| element.id == "family-test.f1").unwrap();
        // No Properties render or manual scope synchronization occurs before this pick.
        assert!(!form.pick_family_host(&session.doc, host, Some([2500.0, 0.0])), "a bare-wall hit contains no renewed storey choice");
        assert!(form.place_type(&session.doc).is_none());
    }

    #[test]
    fn family_host_pick_projects_site_local_mm_onto_translated_rotated_source_bays() {
        let mut doc = family_document();
        let angle = 0.73_f64;
        for point in &mut doc.buildings[0].footprint {
            let [x, y] = *point;
            *point = [15000.0 + x * angle.cos() - y * angle.sin(), -27000.0 + x * angle.sin() + y * angle.cos()];
        }
        let before = doc.clone();
        let mut form = Authoring::default();
        form.choose_type(&doc.types[0]);
        form.begin_family_placement(&doc, None);
        form.place_level = Some(2);
        form.place_all_bays = true;
        form.place_replace = true;
        let (a, b) = doc.buildings[0].edge(0).unwrap();
        let frame = trigon_model::build::FacadeFrame::new(a, b).unwrap();
        let point = frame.point(6100.0, -500.0, 0.0);
        assert!(form.pick_family_host(&doc, &family_host_element(), Some([point[0], point[1]])));
        assert_eq!(form.facade, "family-test.f1");
        assert_eq!(form.place_level, Some(2), "a facade wall pick retains the explicitly selected storey");
        assert_eq!(form.place_bay, 1, "the click snaps to the source bay at 6000 mm despite translation, rotation and outward offset");
        assert!(!form.place_all_bays && !form.place_replace);
        let run = form.place_type(&doc).unwrap();
        assert_eq!(run.params["levels"], json!([2]));
        assert_eq!(run.params["bays"], json!([1]));
        assert!(run.params.get("u").is_none(), "a bay-snapped pick must not invent a free host position");
        assert_eq!(doc, before, "picking fills the form without executing placement or altering source coordinates");
    }

    #[test]
    fn family_host_pick_uses_known_component_occurrences_when_a_3d_point_is_unavailable() {
        let doc = family_document();
        let mut picked = family_host_element();
        picked.host = false;
        picked.kind = "balcony";
        picked.id = "family-test.f1.L1.B2.balcony".into();
        picked.level = Some(1);
        picked.props = vec![("facade", "family-test.f1".into()), ("type", doc.types[6].id.clone())];
        let mut form = Authoring::default();
        form.choose_type(&doc.types[0]);
        form.begin_family_placement(&doc, None);
        assert!(form.pick_family_host(&doc, &picked, None));
        assert_eq!(form.place_level, Some(1));
        assert_eq!(form.place_bay, 2);
        assert_eq!(form.draft.as_ref(), Some(&doc.types[0]), "host picking never changes the type being placed");
        assert_eq!(form.place_type(&doc).unwrap().params["bays"], json!([2]));
        picked.id = "family-test.f1.L1.B99.balcony".into();
        assert!(!form.pick_family_host(&doc, &picked, None));
        picked.id = "not-an-identifiable-occurrence".into();
        assert!(!form.pick_family_host(&doc, &picked, None), "a missing point cannot become a guessed bay");
    }

    #[test]
    fn ambiguous_or_hostile_family_picks_leave_every_placement_choice_unchanged() {
        let doc = family_document();
        let wall = family_host_element();
        let mut form = Authoring::default();
        form.choose_type(&doc.types[0]);
        form.begin_family_placement(&doc, None);
        form.facade = "family-test.f2".into();
        form.place_level = Some(1);
        form.place_bay = 2;
        form.place_all_bays = true;
        form.place_replace = true;
        let choices =
            |form: &Authoring| (form.facade.clone(), form.place_level, form.place_bay, form.place_all_bays, form.place_replace, form.draft.clone());
        let before = choices(&form);
        for point in [None, Some([f64::NAN, 0.0]), Some([0.0, f64::INFINITY]), Some([4250.0, 0.0])] {
            assert!(!form.pick_family_host(&doc, &wall, point), "missing, hostile and equally near bay picks must be inert");
            assert_eq!(choices(&form), before);
        }
        let mut duplicate = doc.clone();
        duplicate.buildings.push(doc.buildings[0].clone());
        assert!(!form.pick_family_host(&duplicate, &wall, Some([6000.0, 0.0])));
        let mut no_bays = doc.clone();
        no_bays.buildings[0].facades[0].bays.clear();
        assert!(!form.pick_family_host(&no_bays, &wall, Some([6000.0, 0.0])));
        let mut missing = wall.clone();
        missing.id = "missing-facade".into();
        assert!(!form.pick_family_host(&doc, &missing, Some([6000.0, 0.0])));
        let mut interior = wall.clone();
        interior.kind = "interior wall";
        assert!(!form.pick_family_host(&doc, &interior, Some([6000.0, 0.0])));
        assert_eq!(choices(&form), before);
        form.place_level = None;
        assert!(!form.pick_family_host(&doc, &wall, Some([6000.0, 0.0])), "a wall cannot infer a missing storey from an XY point");
        form.place_level = Some(99);
        assert!(!form.pick_family_host(&doc, &wall, Some([6000.0, 0.0])));
        form.place_level = Some(1);
        form.tab = Tab::Project;
        assert!(!form.family_placement_active());
        assert!(!form.pick_family_host(&doc, &wall, Some([6000.0, 0.0])));
        assert_eq!(choices(&form), before);
    }

    #[test]
    fn typed_family_forms_fit_a_280_pixel_dock_and_render_without_changing_saved_values() {
        let mut doc = family_document();
        doc.types.extend(trigon_model::component::copenhagen_types());
        doc.types.extend(trigon_model::balcony::copenhagen_types());
        let ctx = egui::Context::default();
        theme::apply(&ctx);
        for ty in &doc.types {
            let mut form = Authoring::default();
            let mut named = ty.clone();
            named.name = "Long project type name for a narrow component properties dock".into();
            let mut displayed = doc.clone();
            displayed.types = vec![named.clone()];
            form.choose_type(&named);
            form.begin_family_placement(&displayed, None);
            let mut output = ctx.run_ui(
                egui::RawInput { screen_rect: Some(egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(280.0, 3000.0))), ..Default::default() },
                |ui| {
                    egui::CentralPanel::default().show(ui, |ui| {
                        assert!(form.family_editor(ui, &displayed, 0).is_none());
                        assert!(ui.min_rect().right() <= 280.0, "{} controls overflow the dock: {:?}", family_name(ty), ui.min_rect());
                    });
                },
            );
            output.textures_delta.clear();
            assert_eq!(form.draft.as_ref(), Some(&named), "rendering a typed form cannot silently clamp or rewrite saved parameters");
        }
    }

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
