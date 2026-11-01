//! Cached model properties and screen handles. Only the completed edit becomes a command.

use std::collections::{BTreeMap, HashMap};

use eframe::egui;
use serde_json::{Value, json};

use crate::{camera::Orbit, theme};

#[derive(Clone, Debug, PartialEq)]
pub struct Edit {
    pub id: String,
    pub properties: Value,
}

#[derive(Clone, Debug)]
pub struct Handle {
    pub key: String,
    pub label: String,
    pub position: [f64; 3],
    pub direction: [f64; 3],
    pub value: f64,
    pub min: f64,
    pub max: f64,
    pub gain: f64,
}

fn triple(value: &Value) -> Option<[f64; 3]> {
    let values = value.as_array()?;
    if values.len() != 3 {
        return None;
    }
    let p = [values.first()?.as_f64()?, values.get(1)?.as_f64()?, values.get(2)?.as_f64()?];
    p.iter().all(|v| v.is_finite() && v.abs() <= 1e9).then_some(p)
}

impl Handle {
    fn parse(value: &Value) -> Option<Self> {
        let direction = triple(&value["direction"])?;
        let length = direction.iter().map(|v| v * v).sum::<f64>().sqrt();
        let handle = Self {
            key: value["key"].as_str()?.into(),
            label: value["label"].as_str().unwrap_or("Dimension").into(),
            position: triple(&value["position_mm"])?,
            direction: direction.map(|v| v / length),
            value: value["value"].as_f64()?,
            min: value["min"].as_f64()?,
            max: value["max"].as_f64()?,
            gain: value["gain"].as_f64().unwrap_or(1.0),
        };
        (length.is_finite()
            && length > 1e-12
            && !handle.key.is_empty()
            && [handle.value, handle.min, handle.max].iter().all(|v| v.is_finite())
            && handle.min <= handle.value
            && handle.value <= handle.max
            && handle.gain.is_finite()
            && handle.gain > 0.0
            && handle.gain <= 1000.0
            && handle.min < handle.max)
            .then_some(handle)
    }
}

#[derive(Default)]
pub struct Properties {
    wanted: u64,
    generation: u64,
    descriptors: HashMap<String, Value>,
    editing: Option<(u64, String)>,
    values: BTreeMap<String, Value>,
    complex: BTreeMap<String, String>,
    error: Option<String>,
    pub gizmo: Gizmo,
}

impl Properties {
    pub fn begin(&mut self, generation: u64) {
        self.wanted = generation;
        self.descriptors.clear();
        self.editing = None;
        self.gizmo.cancel();
    }

    pub fn finish(&mut self, generation: u64, descriptors: Vec<Value>) {
        if generation < self.wanted || generation < self.generation {
            return;
        }
        self.wanted = generation;
        self.generation = generation;
        self.descriptors.clear();
        for mut descriptor in descriptors {
            let Some(id) = descriptor["id"].as_str().filter(|id| !id.is_empty()).map(str::to_owned) else { continue };
            if self.descriptors.contains_key(&id) {
                descriptor["editable"] = json!(false);
                descriptor["reason"] = json!("This ID occurs more than once in the phase view. Use a complete view to edit it.");
                descriptor["handles"] = json!([]);
            }
            self.descriptors.insert(id, descriptor);
        }
        self.editing = None;
        self.gizmo.cancel();
    }

    pub fn descriptor(&self, id: &str) -> Option<&Value> {
        self.descriptors.get(id)
    }

    pub fn handles(&self, id: &str) -> Vec<Handle> {
        self.descriptor(id)
            .filter(|d| d["editable"] == true)
            .and_then(|d| d["handles"].as_array())
            .into_iter()
            .flatten()
            .filter_map(Handle::parse)
            .collect()
    }

    pub fn ui(&mut self, ui: &mut egui::Ui, id: &str, blocked: Option<&str>) -> Option<Edit> {
        let Some(descriptor) = self.descriptor(id).cloned() else {
            ui.weak("Model properties are updating…");
            return None;
        };
        if self.editing.as_ref() != Some(&(self.generation, id.to_string())) {
            self.editing = Some((self.generation, id.into()));
            self.values.clear();
            self.complex.clear();
            self.error = None;
            for property in descriptor["properties"].as_array().into_iter().flatten() {
                if let Some(key) = property["key"].as_str() {
                    self.values.insert(key.into(), property["value"].clone());
                    if property["value"].is_array() || property["value"].is_object() || property["value"].is_null() {
                        self.complex.insert(key.into(), serde_json::to_string(&property["value"]).unwrap_or_default());
                    }
                }
            }
        }
        theme::eyebrow(ui, "MODEL PARAMETERS");
        let reason =
            blocked.or_else(|| (descriptor["editable"] != true).then(|| descriptor["reason"].as_str().unwrap_or("This element is read-only.")));
        if let Some(reason) = reason {
            ui.colored_label(theme::ACCENT, reason);
        }
        let enabled = reason.is_none();
        let mut changed = serde_json::Map::new();
        for property in descriptor["properties"].as_array().into_iter().flatten() {
            let Some(key) = property["key"].as_str() else { continue };
            let label = property["label"].as_str().unwrap_or(key);
            let original = &property["value"];
            let editable = enabled && property["editable"] == true;
            let value = self.values.entry(key.into()).or_insert_with(|| original.clone());
            ui.push_id(key, |ui| {
                ui.label(label);
                if !editable {
                    let text = value.as_str().map(str::to_string).unwrap_or_else(|| value.to_string());
                    let text = property["unit"].as_str().map_or(text.clone(), |unit| format!("{text} {unit}"));
                    ui.add(egui::Label::new(egui::RichText::new(text).color(theme::MUTED)).wrap());
                    return;
                }
                ui.add_enabled_ui(editable, |ui| {
                    if let Some(choices) = property["choices"].as_array() {
                        let mut text = value.as_str().unwrap_or_default().to_string();
                        egui::ComboBox::from_id_salt("value").selected_text(&text).show_ui(ui, |ui| {
                            for choice in choices.iter().filter_map(Value::as_str) {
                                ui.selectable_value(&mut text, choice.to_string(), choice);
                            }
                        });
                        *value = json!(text);
                    } else if let Some(mut number) = value.as_f64() {
                        let min = property["min"].as_f64().unwrap_or(-1e9);
                        let max = property["max"].as_f64().unwrap_or(1e9);
                        let mut drag = egui::DragValue::new(&mut number).speed(1.0).max_decimals(3);
                        if min.is_finite() && max.is_finite() && min <= max {
                            drag = drag.range(min..=max);
                        }
                        if let Some(unit) = property["unit"].as_str() {
                            drag = drag.suffix(format!(" {unit}"));
                        }
                        if ui.add(drag).changed() && number.is_finite() {
                            *value = json!(number);
                        }
                    } else if let Some(mut checked) = value.as_bool() {
                        if ui.checkbox(&mut checked, "Enabled").changed() {
                            *value = json!(checked);
                        }
                    } else if let Some(text) = value.as_str() {
                        let mut text = text.to_string();
                        if ui.add(egui::TextEdit::singleline(&mut text).char_limit(512)).changed() {
                            *value = json!(text);
                        }
                    } else {
                        let text = self.complex.entry(key.into()).or_default();
                        ui.add(egui::TextEdit::multiline(text).font(egui::TextStyle::Monospace).desired_rows(2).desired_width(f32::INFINITY));
                        ui.weak("JSON coordinates / values");
                    }
                });
            });
            if editable && value != original {
                changed.insert(key.into(), value.clone());
            }
        }
        if let Some(error) = &self.error {
            ui.colored_label(theme::ACCENT, error);
        }
        if ui.add_enabled(enabled, egui::Button::new("Apply model properties")).clicked() {
            for property in descriptor["properties"].as_array().into_iter().flatten().filter(|p| p["editable"] == true) {
                let Some(key) = property["key"].as_str() else { continue };
                if let Some(text) = self.complex.get(key) {
                    match serde_json::from_str::<Value>(text) {
                        Ok(value) if value != property["value"] => {
                            changed.insert(key.into(), value);
                        }
                        Ok(_) => {}
                        Err(error) => {
                            self.error = Some(format!("{}: {error}", property["label"].as_str().unwrap_or(key)));
                            return None;
                        }
                    }
                }
            }
            if !changed.is_empty() {
                return Some(Edit { id: id.into(), properties: Value::Object(changed) });
            }
        }
        None
    }
}

#[derive(Clone, Copy, Debug)]
pub enum Mapping {
    Drawing { at: egui::Pos2, per_mm: egui::Vec2 },
    Spatial { camera: Orbit, rect: egui::Rect, at: [f32; 3], direction: [f32; 3] },
}

pub fn project(camera: Orbit, rect: egui::Rect, point: [f32; 3]) -> Option<egui::Pos2> {
    if rect.width() <= 0.0 || rect.height() <= 0.0 || point.iter().any(|v| !v.is_finite()) {
        return None;
    }
    let m = camera.view_proj(rect.width() / rect.height());
    let clip = std::array::from_fn::<_, 4, _>(|r| m[r] * point[0] + m[4 + r] * point[1] + m[8 + r] * point[2] + m[12 + r]);
    if clip[3] <= 1e-5 || !clip.iter().all(|v| v.is_finite()) {
        return None;
    }
    let ndc = [clip[0] / clip[3], clip[1] / clip[3], clip[2] / clip[3]];
    (0.0..=1.0).contains(&ndc[2]).then(|| egui::pos2(rect.center().x + ndc[0] * rect.width() / 2.0, rect.center().y - ndc[1] * rect.height() / 2.0))
}

impl Mapping {
    pub fn parameter(self, pointer: egui::Pos2) -> Option<f64> {
        if !pointer.is_finite() {
            return None;
        }
        match self {
            Self::Drawing { at, per_mm } => (per_mm.length_sq() > 1e-10).then(|| f64::from((pointer - at).dot(per_mm) / per_mm.length_sq())),
            Self::Spatial { camera, rect, at, direction } => {
                if !(rect.width() > 0.0 && rect.height() > 0.0 && rect.width().is_finite() && rect.height().is_finite()) {
                    return None;
                }
                let ndc = [(pointer.x - rect.center().x) * 2.0 / rect.width(), (rect.center().y - pointer.y) * 2.0 / rect.height()];
                let (origin, ray) = camera.ray(ndc, rect.width() / rect.height());
                let dot = |a: [f32; 3], b: [f32; 3]| a.into_iter().zip(b).map(|(a, b)| f64::from(a) * f64::from(b)).sum::<f64>();
                let w = std::array::from_fn(|k| origin[k] - at[k]);
                let b = dot(ray, direction);
                let denominator = 1.0 - b * b;
                (denominator > 1e-5).then(|| (dot(direction, w) - b * dot(ray, w)) / denominator * 1000.0)
            }
        }
    }

    pub fn screen(self, delta: f64) -> Option<egui::Pos2> {
        match self {
            Self::Drawing { at, per_mm } => Some(at + per_mm * delta as f32),
            Self::Spatial { camera, rect, at, direction } => {
                project(camera, rect, std::array::from_fn(|k| at[k] + direction[k] * delta as f32 / 1000.0))
            }
        }
    }
}

#[derive(Clone)]
pub struct ProjectedHandle {
    pub handle: Handle,
    pub mapping: Mapping,
}

struct Drag {
    id: String,
    key: String,
    generation: u64,
    mapping: Mapping,
    press: f64,
    start: f64,
    value: f64,
    min: f64,
    max: f64,
    gain: f64,
    started: bool,
}

#[derive(Default)]
pub struct Gizmo {
    drag: Option<Drag>,
}

impl Gizmo {
    pub fn active(&self) -> bool {
        self.drag.is_some()
    }
    pub fn cancel(&mut self) {
        self.drag = None;
    }

    fn begin(&mut self, id: &str, generation: u64, handle: &ProjectedHandle, pointer: egui::Pos2) {
        if let Some(press) = handle.mapping.parameter(pointer) {
            self.drag = Some(Drag {
                id: id.into(),
                key: handle.handle.key.clone(),
                generation,
                mapping: handle.mapping,
                press,
                start: handle.handle.value,
                value: handle.handle.value,
                min: handle.handle.min,
                max: handle.handle.max,
                gain: handle.handle.gain,
                started: false,
            });
        }
    }

    fn update(&mut self, pointer: egui::Pos2) {
        if let Some(drag) = &mut self.drag
            && let Some(at) = drag.mapping.parameter(pointer)
        {
            let value = drag.start + (at - drag.press) * drag.gain;
            if value.is_finite() {
                drag.value = value.clamp(drag.min, drag.max);
                drag.started = true;
            }
        }
    }

    fn commit(&mut self, id: &str, generation: u64) -> Option<Edit> {
        let drag = self.drag.take()?;
        (drag.id == id && drag.generation == generation && (drag.value - drag.start).abs() > 1e-6)
            .then(|| Edit { id: drag.id, properties: json!({(drag.key):drag.value}) })
    }

    pub fn ui(
        &mut self,
        ui: &mut egui::Ui,
        rect: egui::Rect,
        id: &str,
        generation: u64,
        handles: &[ProjectedHandle],
        enabled: bool,
    ) -> (bool, Option<Edit>) {
        let cancelled = ui.input(|i| i.key_pressed(egui::Key::Escape) || i.viewport().focused == Some(false));
        if !enabled || self.drag.as_ref().is_some_and(|d| d.id != id || d.generation != generation) || cancelled {
            self.cancel();
        }
        if cancelled {
            return (true, None);
        }
        if !enabled {
            return (false, None);
        }
        let painter = ui.painter_at(rect);
        let pointer = ui.input(|i| i.pointer.interact_pos());
        let nearest = pointer.filter(|_| ui.rect_contains_pointer(rect)).and_then(|pointer| {
            handles
                .iter()
                .filter_map(|handle| handle.mapping.screen(0.0).filter(|at| rect.contains(*at)).map(|at| (handle, at.distance_sq(pointer))))
                .filter(|(_, distance)| *distance <= 144.0)
                .min_by(|(a, da), (b, db)| da.total_cmp(db).then(a.handle.key.cmp(&b.handle.key)))
                .map(|(handle, _)| handle)
        });
        if !self.active()
            && ui.input(|i| i.pointer.button_pressed(egui::PointerButton::Primary))
            && let (Some(handle), Some(pointer)) = (nearest, pointer)
        {
            self.begin(id, generation, handle, pointer);
        }
        let busy = self.active() || nearest.is_some();
        let mut edit = None;
        // Keep updating and finish a captured drag even when its marker leaves the viewport.
        if self.active() {
            if (ui.input(|i| i.pointer.is_decidedly_dragging()) || self.drag.as_ref().is_some_and(|drag| drag.started))
                && let Some(pointer) = pointer
            {
                self.update(pointer);
            }
            if ui.input(|i| i.pointer.button_released(egui::PointerButton::Primary)) {
                edit = self.commit(id, generation);
            }
        }
        for handle in handles {
            let delta = self.drag.as_ref().filter(|d| d.key == handle.handle.key).map_or(0.0, |d| (d.value - d.start) / d.gain);
            let Some(at) = handle.mapping.screen(delta).filter(|p| rect.contains(*p)) else { continue };
            let response = ui.interact(
                egui::Rect::from_center_size(at, egui::vec2(18.0, 18.0)),
                egui::Id::new(("model-handle", id, &handle.handle.key)),
                egui::Sense::hover(),
            );
            painter.circle_filled(at, 5.0, theme::PAPER);
            painter.circle_stroke(at, 5.0, egui::Stroke::new(2.0, theme::ACCENT));
            response
                .clone()
                .on_hover_text(format!("Drag {} · {:.1} mm", handle.handle.label, handle.handle.value))
                .on_hover_cursor(egui::CursorIcon::ResizeHorizontal);
        }
        if let Some(drag) = &self.drag
            && let Some(at) = drag.mapping.screen((drag.value - drag.start) / drag.gain)
        {
            painter.text(
                at + egui::vec2(12.0, -12.0),
                egui::Align2::LEFT_BOTTOM,
                format!("{:.1} mm", drag.value),
                egui::FontId::monospace(13.0),
                theme::ACCENT,
            );
        }
        (busy, edit)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn handle() -> ProjectedHandle {
        ProjectedHandle {
            handle: Handle {
                key: "width_mm".into(),
                label: "Width".into(),
                position: [0.0; 3],
                direction: [1.0, 0.0, 0.0],
                value: 1200.0,
                min: 100.0,
                max: 4000.0,
                gain: 1.0,
            },
            mapping: Mapping::Drawing { at: egui::pos2(100.0, 100.0), per_mm: egui::vec2(0.2, 0.0) },
        }
    }

    #[test]
    fn drag_commits_once_only_after_release_and_cancel_or_generation_change_cannot_commit() {
        let mut gizmo = Gizmo::default();
        let h = handle();
        gizmo.begin("window", 3, &h, egui::pos2(101.0, 100.0));
        gizmo.update(egui::pos2(141.0, 150.0));
        assert!((gizmo.drag.as_ref().unwrap().value - 1400.0).abs() < 0.001);
        assert!((gizmo.commit("window", 3).unwrap().properties["width_mm"].as_f64().unwrap() - 1400.0).abs() < 0.001);
        assert!(gizmo.commit("window", 3).is_none());
        gizmo.begin("window", 3, &h, egui::pos2(100.0, 100.0));
        gizmo.update(egui::pos2(900.0, 100.0));
        assert_eq!(gizmo.drag.as_ref().unwrap().value, 4000.0);
        assert!(gizmo.commit("window", 4).is_none());
        gizmo.begin("window", 4, &h, egui::pos2(100.0, 100.0));
        gizmo.update(egui::pos2(150.0, 100.0));
        gizmo.cancel();
        assert!(gizmo.commit("window", 4).is_none());
    }

    #[test]
    fn perspective_drag_uses_the_world_axis_instead_of_a_fixed_pixel_scale() {
        let camera = Orbit { target: [0.0, 0.0, 0.0], distance: 20.0, ..Default::default() };
        let rect = egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(1000.0, 700.0));
        let mapping = Mapping::Spatial { camera, rect, at: [0.0, 0.0, 0.0], direction: [1.0, 0.0, 0.0] };
        let a = mapping.screen(0.0).unwrap();
        let b = mapping.screen(1500.0).unwrap();
        let delta = mapping.parameter(b).unwrap() - mapping.parameter(a).unwrap();
        assert!((delta - 1500.0).abs() < 0.05, "{delta}");
        assert!(mapping.parameter(egui::pos2(f32::NAN, 0.0)).is_none());
    }

    #[test]
    fn centered_dimension_gain_updates_the_parameter_and_moves_the_edge_half_as_far() {
        let mut h = handle();
        h.handle.gain = 2.0;
        let mut gizmo = Gizmo::default();
        gizmo.begin("window", 1, &h, egui::pos2(100.0, 100.0));
        gizmo.update(egui::pos2(140.0, 100.0));
        let drag = gizmo.drag.as_ref().unwrap();
        assert!((drag.value - 1600.0).abs() < 0.001);
        assert_eq!(drag.mapping.screen((drag.value - drag.start) / drag.gain), Some(egui::pos2(140.0, 100.0)));
        assert!((gizmo.commit("window", 1).unwrap().properties["width_mm"].as_f64().unwrap() - 1600.0).abs() < 0.001);
    }

    #[test]
    fn crowded_drawing_handles_choose_the_nearest_marker_instead_of_the_last_hit_box() {
        let mut height = handle();
        height.handle.key = "height_mm".into();
        height.handle.value = 1400.0;
        height.mapping = Mapping::Drawing { at: egui::pos2(200.0, 200.0), per_mm: egui::vec2(0.0, -0.012) };
        let mut along = handle();
        along.handle.key = "u_mm".into();
        along.mapping = Mapping::Drawing { at: egui::pos2(200.0, 208.0), per_mm: egui::vec2(0.012, 0.0) };
        let handles = vec![height, along];
        let ctx = egui::Context::default();
        let mut gizmo = Gizmo::default();
        let mut edits = Vec::new();
        let rect = egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(600.0, 400.0));
        let mut frame = 0;
        let mut draw = |events| {
            frame += 1;
            let mut output =
                ctx.run_ui(egui::RawInput { screen_rect: Some(rect), time: Some(frame as f64 * 0.03), events, ..Default::default() }, |ui| {
                    ui.allocate_exact_size(rect.size(), egui::Sense::click_and_drag());
                    if let (_, Some(edit)) = gizmo.ui(ui, rect, "window", 1, &handles, true) {
                        edits.push(edit);
                    }
                });
            output.textures_delta.clear();
        };
        draw(vec![]);
        for event in crate::control::parse_input("ui.drag", &json!({"from":[200,200],"to":[200,192],"steps":8})).unwrap() {
            draw(vec![event]);
        }
        assert_eq!(edits.len(), 1, "the nearest height handle must commit");
        assert!(edits[0].properties.get("height_mm").is_some());
        assert!((edits[0].properties["height_mm"].as_f64().unwrap() - (1400.0 + 8.0 / 0.012)).abs() < 0.1);
    }

    #[test]
    fn properties_invalidation_rejects_stale_worker_data_and_cancels_handles() {
        let descriptor = json!({"id":"wall","editable":true,"properties":[],"handles":[]});
        let mut properties = Properties::default();
        properties.finish(1, vec![descriptor.clone()]);
        properties.gizmo.begin("wall", 1, &handle(), egui::pos2(100.0, 100.0));
        properties.begin(3);
        properties.finish(2, vec![descriptor.clone()]);
        assert!(properties.descriptor("wall").is_none());
        assert!(!properties.gizmo.active());
        properties.finish(3, vec![descriptor.clone(), descriptor]);
        assert_eq!(properties.descriptor("wall").unwrap()["editable"], false);
    }
}
