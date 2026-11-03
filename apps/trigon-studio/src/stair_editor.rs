//! Direct staircase drafts. Nothing here mutates the document; one guarded preview
//! carries the complete existing Stair values to the shared engine command.
use std::collections::BTreeSet;

use eframe::egui;
use serde_json::{Value, json};
use trigon_model::{Building, Document, Stair, StairKind, Storey};

use crate::{authoring::Run, properties::Handle, theme};

#[derive(Clone, Copy, Default, PartialEq)]
enum Scope {
    #[default]
    Picked,
    All,
    Selected,
}

struct Draft {
    picked: String,
    building: String,
    level: usize,
    ground: usize,
    storeys: Vec<Storey>,
    expected: Stair,
    value: Stair,
    scope: Scope,
    levels: BTreeSet<usize>,
    stale: bool,
    error: Option<String>,
}

#[derive(Default)]
pub struct Editor {
    draft: Option<Draft>,
}

fn resolve<'a>(doc: &'a Document, picked: &str) -> Result<(&'a Building, &'a Stair, usize), String> {
    let mut found = None;
    for b in &doc.buildings {
        for st in &b.stairs {
            if st.id.is_empty() {
                continue;
            }
            for &level in &st.levels {
                if picked == format!("{}.L{level}", st.id) {
                    if found.is_some() || b.storeys.get(level).is_none() {
                        return Err("This stair selection is ambiguous or its storey is missing.".into());
                    }
                    found = Some((b, st, level));
                }
            }
        }
    }
    found.ok_or_else(|| "Select a uniquely named staircase in the canvas or Project Browser.".into())
}

fn depths(stair: &Stair, width: f64) -> [f64; 2] {
    match stair.kind {
        StairKind::Straight => [0.0; 2],
        StairKind::UTurn => [0.0, stair.landing.unwrap_or(width / 2.0)],
        StairKind::DoubleQuarterTurn => stair.turn_depths.unwrap_or([width; 2]),
    }
}

/// Update source control coordinates, including a fitted boundary. The actual
/// steps, rails and shaft are still generated and validated by trigon-model.
fn resize(stair: &Stair, width: f64, length: f64) -> Result<Stair, String> {
    finish_candidate(stair, stair.clone(), width, length)
}

/// One form event is one draft transaction. Dependent fields are never clamped
/// merely because a proposed width has made their current value invalid.
fn finish_candidate(previous: &Stair, mut candidate: Stair, width: f64, length: f64) -> Result<Stair, String> {
    let old = trigon_model::stair::frame(previous).map_err(|e| e.to_string())?;
    if width != old.width || length != old.length {
        candidate.outline = vec![old.at(0.0, 0.0), old.at(length, 0.0), old.at(length, width), old.at(0.0, width)];
    }
    if candidate.boundary == previous.boundary
        && candidate.boundary.is_some()
        && (candidate.outline != previous.outline
            || candidate.landing != previous.landing
            || candidate.turn_depths != previous.turn_depths
            || candidate.kind != previous.kind)
    {
        remap_boundary(previous, &mut candidate)?;
    }
    trigon_model::stair::frame(&candidate).map_err(|e| e.to_string())?;
    Ok(candidate)
}

fn remap_boundary(stair: &Stair, next: &mut Stair) -> Result<(), String> {
    let old = trigon_model::stair::frame(stair).map_err(|e| e.to_string())?;
    let mut unfitted = next.clone();
    unfitted.boundary = None;
    let new = trigon_model::stair::frame(&unfitted).map_err(|e| e.to_string())?;
    if let Some(boundary) = &stair.boundary {
        let a = depths(stair, old.width);
        let b = depths(next, new.width);
        next.boundary = Some(
            boundary
                .iter()
                .map(|&p| {
                    let [s, t] = old.local(p);
                    let s = if s < 0.0 {
                        s * b[0] / a[0].max(1.0)
                    } else if s <= old.length {
                        s * new.length / old.length
                    } else {
                        new.length + (s - old.length) * b[1] / a[1].max(1.0)
                    };
                    new.at(s, t * new.width / old.width)
                })
                .collect(),
        );
    }
    Ok(())
}

fn optional_number(ui: &mut egui::Ui, label: &str, value: &mut Option<f64>, automatic: f64, range: std::ops::RangeInclusive<f64>) {
    ui.push_id(label, |ui| {
        ui.horizontal_wrapped(|ui| {
            ui.label(label);
            let mut auto = value.is_none();
            if let Some(number) = value {
                ui.add(
                    egui::DragValue::new(number)
                        .speed(1.0)
                        .range(range)
                        .clamp_existing_to_range(false)
                        .update_while_editing(false)
                        .suffix(" mm")
                        .custom_formatter(|n, _| n.to_string()),
                );
            }
            if ui.checkbox(&mut auto, "Auto").on_hover_text("Use the staircase's automatic value").changed() {
                *value = if auto { None } else { Some(automatic) };
            }
        });
    });
}

/// `None` means a flat landing for U-turns, but ten automatic turning steps
/// for double-quarter stairs. The control names the actual kind-specific choice.
fn winding_controls(ui: &mut egui::Ui, stair: &mut Stair) -> egui::Response {
    let response = if stair.kind == StairKind::DoubleQuarterTurn {
        let mut automatic = stair.winders.is_none();
        let response = ui.checkbox(&mut automatic, "Automatic turning steps (10)");
        if response.changed() {
            stair.winders = if automatic { None } else { Some(10) };
        }
        response
    } else {
        let mut winding = stair.winders.is_some();
        let response = ui.checkbox(&mut winding, "Winding steps instead of flat landing");
        if response.changed() {
            stair.winders = if winding { Some(6) } else { None };
        }
        response
    };
    if let Some(n) = &mut stair.winders {
        ui.add(egui::DragValue::new(n).range(2..=16).clamp_existing_to_range(false).update_while_editing(false).suffix(" turning steps"));
    }
    response
}

impl Draft {
    fn new(doc: &Document, picked: &str) -> Result<Self, String> {
        let (b, st, level) = resolve(doc, picked)?;
        trigon_model::stair::frame(st).map_err(|e| e.to_string())?;
        Ok(Self {
            picked: picked.into(),
            building: b.id.clone(),
            level,
            ground: b.ground_level,
            storeys: b.storeys.clone(),
            expected: st.clone(),
            value: st.clone(),
            scope: Scope::Picked,
            levels: BTreeSet::from([level]),
            stale: false,
            error: None,
        })
    }

    fn current(&self, doc: &Document) -> bool {
        resolve(doc, &self.picked).is_ok_and(|(b, st, level)| {
            b.id == self.building && *st == self.expected && b.storeys == self.storeys && b.ground_level == self.ground && level == self.level
        })
    }

    fn selected_levels(&self) -> Vec<usize> {
        self.expected
            .levels
            .iter()
            .copied()
            .filter(|l| match self.scope {
                Scope::Picked => *l == self.level,
                Scope::All => true,
                Scope::Selected => self.levels.contains(l),
            })
            .collect()
    }

    fn request(&self, doc: &Document) -> Result<Run, String> {
        if self.stale || !self.current(doc) {
            return Err("The staircase or its storeys changed. Reload it before reviewing this draft.".into());
        }
        let selected = self.selected_levels();
        if selected.is_empty() {
            return Err("Choose at least one departure storey.".into());
        }
        if self.value == self.expected {
            return Err("Change a stair dimension or parameter before reviewing.".into());
        }
        let mut changed = self.value.clone();
        changed.levels = selected.clone();
        let kept: Vec<_> = self.expected.levels.iter().copied().filter(|l| !selected.contains(l)).collect();
        let partitions = if kept.is_empty() {
            vec![changed]
        } else {
            changed.id = trigon_engine::suggest_stair_split_id(doc, &changed).map_err(|e| e.to_string())?;
            let mut original = self.expected.clone();
            original.levels = kept;
            vec![original, changed]
        };
        let affected: BTreeSet<_> =
            selected.iter().copied().chain(selected.iter().filter_map(|l| l.checked_add(1))).filter(|l| self.storeys.get(*l).is_some()).collect();
        Ok(Run {
            command: "precision.preview",
            params: json!({
                "action":{"command":"stair.refine_source","params":{
                    "building":self.building,"expected_source":self.expected,"partitions":partitions}},
                "sanity":{"building":self.building,"levels":affected,"max_results":1000}
            }),
        })
    }
}

/// Preview rings describe changed shafts on their departure and receiving
/// storeys. Sections/elevations are not treated as plans by dropping their Z.
#[derive(Clone, Copy)]
pub enum PreviewView {
    Plan(usize),
    Spatial { clip_z: Option<f64> },
    Hidden,
}

fn outlines(b: &Building, stair: &Stair, levels: &[usize], view: PreviewView) -> Vec<Vec<[f64; 3]>> {
    let Ok(ring) = trigon_model::stair::opening(stair) else { return Vec::new() };
    let floors: BTreeSet<_> = levels.iter().copied().chain(levels.iter().filter_map(|l| l.checked_add(1))).collect();
    let mut rings: Vec<_> = floors
        .into_iter()
        .filter_map(|level| {
            let storey = b.storeys.get(level)?;
            let z = storey.elevation + 20.0;
            let visible = match view {
                PreviewView::Plan(at) => at == level,
                PreviewView::Spatial { clip_z } => clip_z.is_none_or(|cut| z <= cut),
                PreviewView::Hidden => false,
            };
            visible.then(|| ring.iter().map(|p| [p[0], p[1], z]).collect())
        })
        .collect();
    // Legacy roofs have a physical attic floor without a separate storey index.
    // Its arrival belongs at the bearing height in 3D, not in the top plan.
    if b.attic_level().is_none()
        && !matches!(b.roof.base(), trigon_model::Roof::Flat)
        && b.storeys.len().checked_sub(1).is_some_and(|top| levels.contains(&top))
    {
        let z = b.height() + 20.0;
        if matches!(view, PreviewView::Spatial { clip_z } if clip_z.is_none_or(|cut| z <= cut)) {
            rings.push(ring.iter().map(|p| [p[0], p[1], z]).collect());
        }
    }
    rings
}

struct Captured {
    building: String,
    expected_source: Stair,
    partitions: Vec<Stair>,
}

impl Captured {
    fn parse(params: &Value) -> Option<Self> {
        let fields = params.as_object()?;
        if fields.len() != 3 || fields.keys().any(|key| !matches!(key.as_str(), "building" | "expected_source" | "partitions")) {
            return None;
        }
        Some(Self {
            building: params["building"].as_str()?.to_owned(),
            expected_source: serde_json::from_value(params["expected_source"].clone()).ok()?,
            partitions: serde_json::from_value(params["partitions"].clone()).ok()?,
        })
    }
}

fn same_shape(expected: &Stair, part: &Stair) -> bool {
    let mut scoped = expected.clone();
    scoped.id = part.id.clone();
    scoped.levels = part.levels.clone();
    scoped == *part
}

pub fn captured_outlines(doc: &Document, params: &Value, view: PreviewView) -> Vec<Vec<[f64; 3]>> {
    let Some(captured) = Captured::parse(params) else { return Vec::new() };
    let mut buildings = doc.buildings.iter().filter(|b| b.id == captured.building);
    let Some(b) = buildings.next() else { return Vec::new() };
    if buildings.next().is_some()
        || b.stairs.iter().filter(|s| s.id == captured.expected_source.id).count() != 1
        || !b.stairs.contains(&captured.expected_source)
    {
        return Vec::new();
    }
    captured
        .partitions
        .iter()
        .filter(|part| !same_shape(&captured.expected_source, part))
        .flat_map(|part| outlines(b, part, &part.levels, view))
        .collect()
}

/// Text comes from the same immutable action as Apply and the canvas overlay.
/// The local editor may contain a different, deliberately retained draft.
pub fn captured_summary(params: &Value) -> Vec<String> {
    let Some(captured) = Captured::parse(params) else {
        return vec!["Captured staircase parameters cannot be displayed.".into()];
    };
    let mut text = vec![format!("Building: {} · source: {}", captured.building, captured.expected_source.id)];
    for part in &captured.partitions {
        let state = if same_shape(&captured.expected_source, part) { "Retained" } else { "Changed" };
        let levels = part.levels.iter().map(|l| format!("L{l}")).collect::<Vec<_>>().join(", ");
        text.push(format!("{state} definition {} · departures: {levels}", part.id));
        let Ok(f) = trigon_model::stair::frame(part) else {
            text.push("Invalid captured dimensions".into());
            continue;
        };
        let kind = match part.kind {
            StairKind::Straight => "Straight",
            StairKind::UTurn => "U-turn",
            StairKind::DoubleQuarterTurn => "Two quarter turns",
        };
        text.push(format!("{kind} · width {} mm · straight run {} mm", f.width, f.length));
        text.push(format!("Tread depth: {}", part.tread.map_or("Automatic (equal going)".into(), |n| format!("{n} mm"))));
        if part.kind == StairKind::UTurn {
            text.push(format!(
                "Half landing: {} mm{} · well: {} mm{}",
                part.landing.unwrap_or(f.width / 2.0),
                if part.landing.is_none() { " (automatic)" } else { "" },
                part.well.unwrap_or(150.0),
                if part.well.is_none() { " (automatic)" } else { "" }
            ));
            text.push(format!("Turn: {}", part.winders.map_or("Flat landing".into(), |n| format!("{n} winding steps"))));
            text.push(format!("Rounded turn: {}{}", part.rounded, if part.boundary.is_some() { " (fitted boundary overrides shape)" } else { "" }));
        } else if part.kind == StairKind::DoubleQuarterTurn {
            text.push(format!("Turning steps: {}{}", part.winders.unwrap_or(10), if part.winders.is_none() { " (automatic)" } else { "" }));
            let depths = part.turn_depths.unwrap_or([f.width; 2]);
            text.push(format!("Turn depths: {} / {} mm{}", depths[0], depths[1], if part.turn_depths.is_none() { " (automatic)" } else { "" }));
        }
        if let Some([run, across]) = part.turn_chamfer {
            text.push(format!("Turn bevel: {run} mm along / {across} mm across"));
        }
        if let Some(boundary) = &part.boundary {
            text.push(format!("Fitted boundary: {} vertices", boundary.len()));
        }
    }
    text
}

/// Detailed coordinates remain available without burying the Apply/Cancel row.
pub fn captured_coordinates(params: &Value) -> Vec<String> {
    let Some(captured) = Captured::parse(params) else { return Vec::new() };
    let mut text = Vec::new();
    for part in &captured.partitions {
        text.push(part.id.clone());
        for (i, p) in part.outline.iter().enumerate() {
            text.push(format!("Frame corner {}: {}, {} mm", i + 1, p[0], p[1]));
        }
        if let Some(boundary) = &part.boundary {
            for (i, p) in boundary.iter().enumerate() {
                text.push(format!("Boundary {}: {}, {} mm", i + 1, p[0], p[1]));
            }
        }
    }
    text
}

impl Editor {
    pub fn clear(&mut self) {
        self.draft = None;
    }

    pub fn preview(&self, doc: &Document) -> Result<Run, String> {
        self.draft.as_ref().ok_or_else(|| "Select a staircase before reviewing.".to_owned())?.request(doc)
    }

    /// Call even when Properties is hidden, before canvas handles can receive input.
    /// A stale draft stays visible but cannot silently target a new numeric storey.
    pub fn sync(&mut self, doc: &Document, picked: Option<&str>) {
        let Some(picked) = picked else {
            self.clear();
            return;
        };
        if self.draft.as_ref().is_none_or(|d| d.picked != picked) {
            self.draft = Draft::new(doc, picked).ok();
        } else if let Some(draft) = &mut self.draft
            && !draft.current(doc)
        {
            draft.stale = true;
            draft.error = Some("The staircase or storey schedule changed. Reload the current staircase to continue.".into());
        }
    }

    pub fn selected(&self, id: &str) -> bool {
        self.draft.as_ref().is_some_and(|d| d.picked == id)
    }

    pub fn failed(&mut self, error: &str) {
        if let Some(draft) = &mut self.draft {
            draft.error = Some(error.into());
        }
    }

    pub fn feedback(&self, doc: &Document) -> Value {
        let Some(d) = &self.draft else {
            return json!({"active":false});
        };
        let stale = d.stale || !d.current(doc);
        let error = d.error.as_deref().or_else(|| stale.then_some("The staircase or its storeys changed. Reload the current staircase."));
        json!({"active":true,"picked":d.picked,"source":d.expected.id,"building":d.building,
            "departure_levels":d.selected_levels(),"scope":match d.scope {Scope::Picked=>"picked",Scope::All=>"all",Scope::Selected=>"selected"},
            "stale":stale,"dirty":d.value!=d.expected,"error":error,"applied":false})
    }

    /// Derive reselection from the exact preview action, not an assumed ordering
    /// of the engine's output targets or the first member of a shared definition.
    pub fn target_after(&self, params: &Value) -> Option<String> {
        let d = self.draft.as_ref()?;
        if params["expected_source"]["id"].as_str() != Some(d.expected.id.as_str()) || params["building"].as_str() != Some(d.building.as_str()) {
            return None;
        }
        let mut matches = params["partitions"]
            .as_array()?
            .iter()
            .filter(|p| p["levels"].as_array().is_some_and(|ls| ls.iter().any(|l| l.as_u64() == Some(d.level as u64))));
        let id = matches.next()?["id"].as_str()?;
        (matches.next().is_none()).then(|| format!("{id}.L{}", d.level))
    }

    pub fn handles(&self, doc: &Document) -> Vec<Handle> {
        let Some(d) = &self.draft else { return Vec::new() };
        if d.stale || !d.current(doc) {
            return Vec::new();
        }
        let Ok(f) = trigon_model::stair::frame(&d.value) else { return Vec::new() };
        let Some(storey) = d.storeys.get(d.level) else { return Vec::new() };
        let make = |key: &str, label: &str, point: [f64; 2], direction: [f64; 2], value, min, max| Handle {
            key: key.into(),
            label: label.into(),
            position: [point[0], point[1], storey.elevation + 800.0],
            direction: [direction[0], direction[1], 0.0],
            value,
            min,
            max,
            gain: 1.0,
        };
        vec![
            make("width_mm", "Stair width", f.at(f.length / 2.0, f.width), f.across, f.width, 600.0, 10000.0),
            make(
                "length_mm",
                "Straight run",
                f.at(f.length, f.width / 2.0),
                f.run,
                f.length,
                if d.value.kind == StairKind::DoubleQuarterTurn { 100.0 } else { 600.0 },
                20000.0,
            ),
        ]
    }

    pub fn handles_in_view(&self, doc: &Document, view: PreviewView) -> Vec<Handle> {
        let Some(d) = &self.draft else { return Vec::new() };
        if !d.selected_levels().contains(&d.level) {
            return Vec::new();
        }
        self.handles(doc)
            .into_iter()
            .filter(|handle| match view {
                PreviewView::Plan(level) => level == d.level,
                PreviewView::Spatial { clip_z } => clip_z.is_none_or(|cut| handle.position[2] <= cut),
                PreviewView::Hidden => false,
            })
            .collect()
    }

    pub fn outlines(&self, doc: &Document, view: PreviewView) -> Vec<Vec<[f64; 3]>> {
        let Some(d) = &self.draft else { return Vec::new() };
        if d.stale || !d.current(doc) || d.value == d.expected {
            return Vec::new();
        }
        let Some(b) = doc.buildings.iter().find(|b| b.id == d.building) else { return Vec::new() };
        outlines(b, &d.value, &d.selected_levels(), view)
    }

    pub fn handle_edit(&mut self, doc: &Document, id: &str, properties: &Value) -> bool {
        let Some(d) = &mut self.draft else { return false };
        if d.picked != id {
            return false;
        }
        if d.stale || !d.current(doc) {
            d.stale = true;
            d.error = Some("The staircase changed during this drag. Reload it before editing.".into());
            return true;
        }
        let next = (|| {
            let f = trigon_model::stair::frame(&d.value).map_err(|e| e.to_string())?;
            let map = properties.as_object().ok_or("Invalid stair handle input.")?;
            if map.is_empty() || map.keys().any(|k| !matches!(k.as_str(), "width_mm" | "length_mm")) {
                return Err("This handle does not edit a stair dimension.".into());
            }
            let number = |key: &str, fallback| -> Result<f64, String> {
                match map.get(key) {
                    Some(v) => v.as_f64().filter(|n| n.is_finite()).ok_or_else(|| format!("{key} needs a finite dimension.")),
                    None => Ok(fallback),
                }
            };
            resize(&d.value, number("width_mm", f.width)?, number("length_mm", f.length)?)
        })();
        match next {
            Ok(value) => {
                d.value = value;
                d.error = None
            }
            Err(e) => d.error = Some(e),
        }
        true
    }

    pub fn ui(&mut self, ui: &mut egui::Ui, doc: &Document, blocked: Option<&str>) -> Option<Run> {
        let d = self.draft.as_mut()?;
        ui.heading("Staircase");
        let mut reload = false;
        let mut review = false;
        if d.stale {
            ui.colored_label(theme::ACCENT, "The source changed while this draft was open.");
            reload = ui.button("Reload current staircase").clicked();
        }
        if let Some(reason) = blocked {
            ui.weak(reason);
        }
        ui.add_enabled_ui(!d.stale && blocked.is_none(), |ui| {
            theme::eyebrow(ui, "EDIT SCOPE");
            egui::ComboBox::from_id_salt("stair-scope")
                .selected_text(match d.scope {
                    Scope::Picked => "This storey",
                    Scope::All => "All storeys of this staircase",
                    Scope::Selected => "Selected storeys",
                })
                .width(ui.available_width())
                .show_ui(ui, |ui| {
                    ui.selectable_value(&mut d.scope, Scope::Picked, "This storey");
                    ui.selectable_value(&mut d.scope, Scope::All, "All storeys of this staircase");
                    ui.selectable_value(&mut d.scope, Scope::Selected, "Selected storeys");
                });
            if d.scope == Scope::Picked {
                if let Some(storey) = d.storeys.get(d.level) {
                    ui.label(format!("Departure: {}", storey.name));
                }
            } else if d.scope == Scope::All {
                ui.label(format!("{} departure storeys", d.expected.levels.len()));
            }
            if d.scope == Scope::Selected {
                for &level in &d.expected.levels {
                    let label = d.storeys.get(level).map(|s| s.name.as_str()).unwrap_or("Missing storey");
                    let mut selected = d.levels.contains(&level);
                    if ui.checkbox(&mut selected, label).changed() {
                        if selected {
                            d.levels.insert(level);
                        } else {
                            d.levels.remove(&level);
                        }
                    }
                }
            }
            ui.weak("Departure flights and their arrivals above are checked together.");
            ui.add_space(8.0);
            theme::eyebrow(ui, "FLIGHTS AND LANDING");
            let mut candidate = d.value.clone();
            let previous = candidate.clone();
            egui::ComboBox::from_id_salt("stair-kind")
                .selected_text(match candidate.kind {
                    StairKind::Straight => "Straight",
                    StairKind::UTurn => "U-turn",
                    StairKind::DoubleQuarterTurn => "Two quarter turns",
                })
                .show_ui(ui, |ui| {
                    ui.selectable_value(&mut candidate.kind, StairKind::Straight, "Straight");
                    ui.selectable_value(&mut candidate.kind, StairKind::UTurn, "U-turn");
                    ui.selectable_value(&mut candidate.kind, StairKind::DoubleQuarterTurn, "Two quarter turns");
                });
            if candidate.kind != previous.kind {
                if candidate.kind == StairKind::Straight {
                    candidate.winders = None;
                }
                if candidate.kind != StairKind::DoubleQuarterTurn {
                    candidate.turn_depths = None;
                }
                if candidate.kind != StairKind::UTurn {
                    candidate.turn_chamfer = None;
                }
            }
            let frame = trigon_model::stair::frame(&previous).ok();
            let mut dimensions = frame.as_ref().map(|f| (f.width, f.length));
            if let Some((width, length)) = dimensions.as_mut() {
                let number = |ui: &mut egui::Ui, label: &str, n: &mut f64, range| {
                    ui.horizontal(|ui| {
                        ui.label(label);
                        ui.add(
                            egui::DragValue::new(n)
                                .speed(1.0)
                                .range(range)
                                .clamp_existing_to_range(false)
                                .update_while_editing(false)
                                .suffix(" mm")
                                .custom_formatter(|v, _| v.to_string()),
                        );
                    });
                };
                number(ui, "Width", width, 600.0..=10000.0);
                number(ui, "Straight run", length, if candidate.kind == StairKind::DoubleQuarterTurn { 100.0..=20000.0 } else { 600.0..=20000.0 });
                optional_number(ui, "Tread depth", &mut candidate.tread, 275.0, 150.0..=400.0);
                if candidate.kind == StairKind::UTurn {
                    optional_number(ui, "Half landing", &mut candidate.landing, *width / 2.0, 300.0..=*width * 2.0);
                    optional_number(ui, "Stair well", &mut candidate.well, 150.0, 0.0..=*width / 3.0);
                    ui.add_enabled(candidate.boundary.is_none(), egui::Checkbox::new(&mut candidate.rounded, "Rounded turn"));
                    if candidate.boundary.is_none() && !candidate.rounded {
                        let mut bevel = candidate.turn_chamfer.is_some();
                        if ui.checkbox(&mut bevel, "Bevel turn corners").changed() {
                            candidate.turn_chamfer = if bevel { Some([100.0, 100.0]) } else { None };
                        }
                        if let Some(cut) = &mut candidate.turn_chamfer {
                            number(ui, "Cut along run", &mut cut[0], 1.0..=candidate.landing.unwrap_or(*width / 2.0) - 1.0);
                            number(ui, "Cut across run", &mut cut[1], 1.0..=*width / 2.0 - 1.0);
                        }
                    }
                }
                if candidate.kind != StairKind::Straight {
                    winding_controls(ui, &mut candidate);
                }

                if candidate.kind == StairKind::DoubleQuarterTurn {
                    let mut automatic = candidate.turn_depths.is_none();
                    if ui.checkbox(&mut automatic, "Automatic quarter-turn depths").changed() {
                        candidate.turn_depths = if automatic { None } else { Some([*width; 2]) };
                    }
                    if let Some(ds) = &mut candidate.turn_depths {
                        number(ui, "Lower turn", &mut ds[0], 300.0..=4000.0);
                        number(ui, "Upper turn", &mut ds[1], 300.0..=4000.0);
                    }
                }
            }
            if candidate.boundary.is_some() {
                ui.weak("A fitted boundary follows this stair's control frame.");
                if ui.button("Use parametric boundary").clicked() {
                    candidate.boundary = None;
                }
            }
            if let (Some(f), Some((width, length))) = (frame, dimensions)
                && (candidate != previous || width != f.width || length != f.length)
            {
                match finish_candidate(&previous, candidate, width, length) {
                    Ok(value) => {
                        d.value = value;
                        d.error = None;
                    }
                    Err(error) => d.error = Some(error),
                }
            }
            ui.add_space(8.0);
            ui.horizontal_wrapped(|ui| {
                review = ui.add_enabled(d.value != d.expected, egui::Button::new("Review changes")).clicked();
                if ui.button("Reset draft").clicked() {
                    d.value = d.expected.clone();
                    d.error = None;
                }
            });
        });
        if let Some(error) = &d.error {
            ui.colored_label(theme::ACCENT, error);
        }
        if reload {
            let picked = d.picked.clone();
            match Draft::new(doc, &picked) {
                Ok(new) => self.draft = Some(new),
                Err(e) => d.error = Some(e),
            }
            return None;
        }
        if review {
            match self.preview(doc) {
                Ok(run) => return Some(run),
                Err(e) => self.failed(&e),
            }
        }
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use trigon_data::{Credentials, Live};
    use trigon_engine::Session;

    fn session() -> Session {
        let mut s = Session::new(Box::new(Live::new(Credentials::default())));
        s.execute(
            "building.add",
            json!({"id":"wing","name":"Public stairs",
            "footprint":[[0,0],[14000,0],[14000,12000],[0,12000]],"storeys":4,"storey_height_mm":3000}),
        )
        .unwrap();
        s.execute(
            "interior.add_stairs",
            json!({"building":"wing","stairs":[{
            "id":"wing.stair-a","levels":[0,1,2],"outline":[[2000.125,2000.25],[5000.125,2000.25],[5000.125,4000.25],[2000.125,4000.25]],
            "kind":"u_turn","rounded":false,"well":160.125,"landing":1000.25}]}),
        )
        .unwrap();
        s
    }

    #[test]
    fn selected_departures_produce_two_coherent_sources_with_exact_unselected_values() {
        let s = session();
        let mut d = Draft::new(&s.doc, "wing.stair-a.L1").unwrap();
        d.scope = Scope::Selected;
        d.levels = BTreeSet::from([1, 2]);
        d.value.tread = Some(275.125);
        let run = d.request(&s.doc).unwrap();
        assert_eq!(run.command, "precision.preview");
        let p = &run.params["action"]["params"];
        assert_eq!(p["expected_source"], json!(d.expected));
        let parts: Vec<Stair> = serde_json::from_value(p["partitions"].clone()).unwrap();
        assert_eq!(parts.len(), 2);
        let mut retained = d.expected.clone();
        retained.levels = vec![0];
        assert_eq!(parts[0], retained);
        assert_eq!(parts[1].levels, vec![1, 2]);
        assert_eq!(parts[1].id, "wing.stair-a.refined1");
        assert_eq!(parts[1].outline, d.expected.outline);
        assert_eq!(run.params["sanity"]["levels"], json!([1, 2, 3]));
    }

    #[test]
    fn all_storeys_retains_source_id_and_empty_scope_emits_nothing() {
        let s = session();
        let mut d = Draft::new(&s.doc, "wing.stair-a.L1").unwrap();
        d.value.well = Some(165.0);
        d.scope = Scope::All;
        let run = d.request(&s.doc).unwrap();
        let parts = run.params["action"]["params"]["partitions"].as_array().unwrap();
        assert_eq!(parts.len(), 1);
        assert_eq!(parts[0]["id"], "wing.stair-a");
        d.scope = Scope::Selected;
        d.levels.clear();
        assert!(d.request(&s.doc).is_err());
    }

    #[test]
    fn split_name_skips_an_occupied_occurrence_alias_and_previews_with_exact_undo() {
        let mut s = session();
        s.execute(
            "interior.add_stairs",
            json!({"building":"wing","stairs":[{
            "id":"wing.stair-a.refined1.L1","levels":[0],
            "outline":[[8000,2000],[11000,2000],[11000,4000],[8000,4000]],
            "kind":"straight","rounded":false}]}),
        )
        .unwrap();
        let before = s.doc.clone();
        let mut editor = Editor::default();
        editor.sync(&s.doc, Some("wing.stair-a.L1"));
        assert!(editor.handle_edit(&s.doc, "wing.stair-a.L1", &json!({"width_mm":2200})));
        let run = editor.preview(&s.doc).unwrap();
        assert_eq!(run.params["action"]["params"]["partitions"][1]["id"], "wing.stair-a.refined2");
        let preview = s.execute(run.command, run.params).unwrap();
        assert_eq!(s.doc, before);
        s.execute("precision.apply", json!({"action":preview["action"],"preview_token":preview["preview_token"]})).unwrap();
        let after = s.doc.clone();
        assert_eq!(after.buildings[0].stairs.last().unwrap().id, "wing.stair-a.refined2");
        s.execute("edit.undo", json!({})).unwrap();
        assert_eq!(s.doc, before);
        s.execute("edit.redo", json!({})).unwrap();
        assert_eq!(s.doc, after);
    }

    #[test]
    fn hidden_properties_cannot_retarget_a_draft_after_basement_insertion() {
        let mut s = session();
        let mut editor = Editor::default();
        editor.sync(&s.doc, Some("wing.stair-a.L1"));
        assert!(editor.handle_edit(&s.doc, "wing.stair-a.L1", &json!({"width_mm":2100})));
        let candidate = editor.draft.as_ref().unwrap().value.clone();
        s.execute("building.add_basement", json!({"building":"wing","floor":-2500})).unwrap();
        // No panel UI or sync call has run since the external command.
        assert!(editor.handles(&s.doc).is_empty());
        assert!(editor.draft.as_ref().unwrap().request(&s.doc).is_err());
        assert!(editor.handle_edit(&s.doc, "wing.stair-a.L1", &json!({"width_mm":2400})));
        assert_eq!(editor.draft.as_ref().unwrap().value, candidate);
        assert_eq!(editor.feedback(&s.doc)["stale"], true);
    }

    #[test]
    fn unchanged_draft_keeps_full_precision_and_no_implicit_split() {
        let s = session();
        let mut editor = Editor::default();
        for _ in 0..5 {
            editor.sync(&s.doc, Some("wing.stair-a.L1"));
        }
        let d = editor.draft.as_ref().unwrap();
        let f = trigon_model::stair::frame(&d.value).unwrap();
        assert_eq!(resize(&d.value, f.width, f.length).unwrap(), d.expected);
        assert!(d.request(&s.doc).is_err());
        assert_eq!(editor.feedback(&s.doc)["dirty"], false);
    }

    #[test]
    fn stale_source_refusal_and_command_error_stay_in_feedback() {
        let mut s = session();
        let mut editor = Editor::default();
        editor.sync(&s.doc, Some("wing.stair-a.L1"));
        s.doc.buildings[0].stairs[0].landing = Some(1100.0);
        editor.sync(&s.doc, Some("wing.stair-a.L1"));
        assert_eq!(editor.feedback(&s.doc)["stale"], true);
        editor.failed("arrival floor excludes shaft by 70 mm");
        assert_eq!(editor.feedback(&s.doc)["error"], "arrival floor excludes shaft by 70 mm");
        assert!(editor.draft.as_ref().unwrap().request(&s.doc).is_err());
    }

    #[test]
    fn duplicate_or_disappeared_source_cannot_supply_a_draft_or_handles() {
        let mut s = session();
        let stair = s.doc.buildings[0].stairs[0].clone();
        s.doc.buildings[0].stairs.push(stair);
        assert!(Draft::new(&s.doc, "wing.stair-a.L1").is_err());
        s.doc.buildings[0].stairs.clear();
        assert!(Draft::new(&s.doc, "wing.stair-a.L1").is_err());
    }

    #[test]
    fn fitted_boundary_resize_preserves_vertices_and_invalid_handle_never_commits() {
        let s = session();
        let mut editor = Editor::default();
        editor.sync(&s.doc, Some("wing.stair-a.L1"));
        let d = editor.draft.as_mut().unwrap();
        let ring = trigon_model::stair::opening(&d.value).unwrap();
        d.value.boundary = Some(ring);
        let before = d.value.clone();
        let changed = resize(&before, 2200.0, 3200.0).unwrap();
        assert_eq!(changed.boundary.as_ref().unwrap().len(), before.boundary.as_ref().unwrap().len());
        trigon_model::stair::frame(&changed).unwrap();
        let same = editor.draft.as_ref().unwrap().value.clone();
        editor.handle_edit(&s.doc, "wing.stair-a.L1", &json!({"width_mm":12}));
        assert_eq!(editor.draft.as_ref().unwrap().value, same);
        assert!(editor.feedback(&s.doc)["error"].is_string());
    }
    fn labels(shapes: &[egui::Shape]) -> Vec<(String, egui::Rect)> {
        shapes
            .iter()
            .flat_map(|shape| match shape {
                egui::Shape::Text(t) => vec![(t.galley.text().to_owned(), egui::Rect::from_min_size(t.pos, t.galley.size()))],
                egui::Shape::Vec(v) => labels(v),
                _ => Vec::new(),
            })
            .collect()
    }

    fn frame_ui(editor: &mut Editor, doc: &Document, ctx: &egui::Context, events: Vec<egui::Event>) -> Vec<(String, egui::Rect)> {
        let mut output = ctx.run_ui(
            egui::RawInput {
                screen_rect: Some(egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(400.0, 1800.0))),
                events,
                ..Default::default()
            },
            |ui| {
                egui::CentralPanel::default().show(ui, |ui| {
                    assert!(editor.ui(ui, doc, None).is_none());
                });
            },
        );
        output.textures_delta.clear();
        labels(&output.shapes.into_iter().map(|s| s.shape).collect::<Vec<_>>())
    }

    fn pointer(pos: egui::Pos2, pressed: bool) -> Vec<egui::Event> {
        vec![
            egui::Event::PointerMoved(pos),
            egui::Event::PointerButton { pos, button: egui::PointerButton::Primary, pressed, modifiers: egui::Modifiers::NONE },
        ]
    }

    #[test]
    fn idle_dependent_number_does_not_rewrite_an_existing_out_of_range_value() {
        let ctx = egui::Context::default();
        let mut well = Some(900.0);
        let mut output = ctx.run_ui(egui::RawInput::default(), |ui| {
            egui::CentralPanel::default().show(ui, |ui| optional_number(ui, "Stair well", &mut well, 150.0, 0.0..=2000.0 / 3.0));
        });
        output.textures_delta.clear();
        assert_eq!(well, Some(900.0), "a narrower proposed width cannot implicitly edit the well");
    }

    #[test]
    fn refused_width_text_entry_preserves_every_draft_field_and_error_through_repaint() {
        let mut s = session();
        let source = &mut s.doc.buildings[0].stairs[0];
        *source = resize(source, 3000.0, 4000.0).unwrap();
        source.well = Some(900.0);
        let mut editor = Editor::default();
        editor.sync(&s.doc, Some("wing.stair-a.L1"));
        let before = editor.draft.as_ref().unwrap().value.clone();
        let ctx = egui::Context::default();
        let initial = frame_ui(&mut editor, &s.doc, &ctx, vec![]);
        let at = initial.iter().find(|(t, _)| t == "3000").expect("the real Width DragValue is displayed").1.center();
        frame_ui(&mut editor, &s.doc, &ctx, pointer(at, true));
        frame_ui(&mut editor, &s.doc, &ctx, pointer(at, false));
        frame_ui(&mut editor, &s.doc, &ctx, vec![]);
        frame_ui(&mut editor, &s.doc, &ctx, vec![egui::Event::Text("20".into())]);
        assert_eq!(editor.draft.as_ref().unwrap().value, before, "partial text stays uncommitted while focused");
        frame_ui(&mut editor, &s.doc, &ctx, vec![egui::Event::Text("00".into())]);
        assert_eq!(editor.draft.as_ref().unwrap().value, before, "complete text waits for blur");
        frame_ui(
            &mut editor,
            &s.doc,
            &ctx,
            vec![egui::Event::Key { key: egui::Key::Enter, physical_key: None, pressed: true, repeat: false, modifiers: egui::Modifiers::NONE }],
        );
        let error = editor.draft.as_ref().unwrap().error.clone().expect("width below 3 × existing well must be refused");
        assert_eq!(editor.draft.as_ref().unwrap().value, before);
        for _ in 0..3 {
            frame_ui(&mut editor, &s.doc, &ctx, vec![]);
            assert_eq!(editor.draft.as_ref().unwrap().value, before);
            assert_eq!(editor.draft.as_ref().unwrap().error.as_deref(), Some(error.as_str()));
        }
        assert!(editor.preview(&s.doc).is_err(), "the refused width cannot create a hidden well-only preview");
    }

    #[test]
    fn double_quarter_checkbox_selects_explicit_or_automatic_counts_and_never_promises_off() {
        let mut s = session();
        let source = &mut s.doc.buildings[0].stairs[0];
        source.kind = StairKind::DoubleQuarterTurn;
        source.winders = Some(6);
        let mut editor = Editor::default();
        editor.sync(&s.doc, Some("wing.stair-a.L1"));
        let ctx = egui::Context::default();
        let initial = frame_ui(&mut editor, &s.doc, &ctx, vec![]);
        let at = initial.iter().find(|(t, _)| t == "Automatic turning steps (10)").unwrap().1.center();
        assert!(!initial.iter().any(|(t, _)| t == "Winding steps" || t.contains("instead of flat landing")));
        frame_ui(&mut editor, &s.doc, &ctx, pointer(at, true));
        frame_ui(&mut editor, &s.doc, &ctx, pointer(at, false));
        assert_eq!(editor.draft.as_ref().unwrap().value.winders, None);
        frame_ui(&mut editor, &s.doc, &ctx, pointer(at, true));
        frame_ui(&mut editor, &s.doc, &ctx, pointer(at, false));
        assert_eq!(editor.draft.as_ref().unwrap().value.winders, Some(10));
        let run = editor.preview(&s.doc).unwrap();
        let summary = captured_summary(&run.params["action"]["params"]);
        assert!(summary.iter().any(|s| s == "Turning steps: 10"));
    }

    #[test]
    fn u_turn_checkbox_switches_between_flat_landing_and_explicit_winders() {
        let s = session();
        let mut editor = Editor::default();
        editor.sync(&s.doc, Some("wing.stair-a.L1"));
        let original = editor.draft.as_ref().unwrap().value.clone();
        assert_eq!(original.winders, None);
        let ctx = egui::Context::default();
        let shown = frame_ui(&mut editor, &s.doc, &ctx, vec![]);
        let at = shown.iter().find(|(t, _)| t == "Winding steps instead of flat landing").unwrap().1.center();
        frame_ui(&mut editor, &s.doc, &ctx, pointer(at, true));
        frame_ui(&mut editor, &s.doc, &ctx, pointer(at, false));
        assert_eq!(editor.draft.as_ref().unwrap().value.winders, Some(6));
        let run = editor.preview(&s.doc).unwrap();
        assert!(captured_summary(&run.params["action"]["params"]).iter().any(|s| s == "Turn: 6 winding steps"));
        frame_ui(&mut editor, &s.doc, &ctx, pointer(at, true));
        frame_ui(&mut editor, &s.doc, &ctx, pointer(at, false));
        assert_eq!(editor.draft.as_ref().unwrap().value, original);
    }

    #[test]
    fn captured_outline_rejects_changed_source_and_never_draws_retained_partitions() {
        let mut s = session();
        let mut d = Draft::new(&s.doc, "wing.stair-a.L1").unwrap();
        d.value = resize(&d.value, 2300.0, 3000.0).unwrap();
        let run = d.request(&s.doc).unwrap();
        let params = &run.params["action"]["params"];
        assert!(captured_outlines(&s.doc, params, PreviewView::Plan(0)).is_empty());
        assert_eq!(captured_outlines(&s.doc, params, PreviewView::Plan(1)).len(), 1);
        assert_eq!(captured_outlines(&s.doc, params, PreviewView::Plan(2)).len(), 1);
        assert!(captured_outlines(&s.doc, params, PreviewView::Plan(3)).is_empty());
        s.doc.buildings[0].stairs[0].well = Some(170.0);
        assert!(captured_outlines(&s.doc, params, PreviewView::Plan(1)).is_empty());
    }
    #[test]
    fn legacy_attic_preview_uses_physical_arrival_height_without_aliasing_the_top_plan() {
        let mut s = session();
        s.doc.buildings[0].stairs[0].levels = vec![3];
        s.execute("building.set_roof", json!({"building":"wing","kind":"pitched","pitch_deg":35})).unwrap();
        let mut editor = Editor::default();
        editor.sync(&s.doc, Some("wing.stair-a.L3"));
        editor.handle_edit(&s.doc, "wing.stair-a.L3", &json!({"width_mm":2200}));
        let built = trigon_model::build(&s.doc).unwrap();
        let attic = built[0].slabs.iter().find(|floor| floor.id == "wing.attic").unwrap();
        let attic_z = attic.z_bottom + attic.thickness + 20.0;
        let rings = editor.outlines(&s.doc, PreviewView::Spatial { clip_z: None });
        assert_eq!(rings.len(), 2, "top departure also changes the real implicit attic arrival");
        assert_eq!(rings[1][0][2], attic_z);
        assert_eq!(editor.outlines(&s.doc, PreviewView::Plan(3)).len(), 1, "index-sharing attic is not a second top-floor plan ring");
        assert_eq!(editor.outlines(&s.doc, PreviewView::Spatial { clip_z: Some(attic_z - 1.0) }).len(), 1);
        let request = editor.preview(&s.doc).unwrap();
        assert_eq!(captured_outlines(&s.doc, &request.params["action"]["params"], PreviewView::Spatial { clip_z: None }), rings);
        s.doc.buildings[0].roof = trigon_model::Roof::Flat;
        assert_eq!(editor.outlines(&s.doc, PreviewView::Spatial { clip_z: None }).len(), 1, "flat roof has no implicit attic floor");
    }
}
