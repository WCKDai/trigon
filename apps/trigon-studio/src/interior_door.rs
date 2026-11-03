//! A selected-wall form; geometry and validation belong to interior.place_door.

use eframe::egui;
use serde_json::{Value, json};

use crate::{authoring::Run, theme};

pub struct Editor {
    source: Option<Value>,
    active: bool,
    generation: u64,
    needs_review: bool,
    at: f64,
    width: f64,
    height: f64,
    error: Option<String>,
}

impl Default for Editor {
    fn default() -> Self {
        Self { source: None, active: false, generation: 0, needs_review: false, at: 0.0, width: 900.0, height: 2100.0, error: None }
    }
}

impl Editor {
    pub fn start(&mut self) {
        self.active = true;
        self.source = None;
        self.needs_review = false;
    }
    pub fn cancel(&mut self) {
        self.active = false;
        self.source = None;
    }

    pub fn outcome(&mut self, error: Option<String>) {
        self.error = error;
    }

    pub fn ui(&mut self, ui: &mut egui::Ui, descriptor: &Value, generation: u64, blocked: Option<&str>) -> Option<Run> {
        if !self.active {
            if ui.add_enabled(blocked.is_none() && descriptor["editable"] == true, egui::Button::new("Add interior door…")).clicked() {
                self.start();
            }
            return None;
        }
        let properties = descriptor["properties"].as_array()?;
        let point = |key| {
            let value = &properties.iter().find(|p| p["key"] == key)?["value"];
            Some([value.get(0)?.as_f64()?, value.get(1)?.as_f64()?])
        };
        let a = point("a")?;
        let b = point("b")?;
        let length = (b[0] - a[0]).hypot(b[1] - a[1]);
        if !length.is_finite() || length <= 0.0 {
            return None;
        }
        if self.source.as_ref() != Some(descriptor) || self.generation != generation {
            self.needs_review = self.source.is_some();
            self.source = Some(descriptor.clone());
            self.generation = generation;
            self.at = length / 2.0;
            self.error = None;
        }
        ui.separator();
        theme::eyebrow(ui, "Add interior door");
        ui.weak("Selected storey only. Existing doors stay in place.");
        ui.label(format!("Wall start: X {:.0}, Y {:.0} mm", a[0], a[1]));
        if self.needs_review {
            ui.colored_label(theme::ACCENT, "The model or selected wall changed. Review the current host and dimensions.");
            if ui.add_enabled(blocked.is_none(), egui::Button::new("Review current wall")).clicked() {
                self.needs_review = false;
            }
        }
        let enabled = blocked.is_none() && descriptor["editable"] == true && !self.needs_review;
        let mut run = None;
        ui.add_enabled_ui(enabled, |ui| {
            for (label, value, min, max) in [
                ("Centre from wall start", &mut self.at, 0.0, length),
                ("Opening width", &mut self.width, 300.0, 4000.0),
                ("Opening height", &mut self.height, 1000.0, 4000.0),
            ] {
                ui.label(label);
                ui.add(egui::DragValue::new(value).speed(10.0).range(min..=max).suffix(" mm"));
            }
            if theme::primary(ui, "Add door").clicked() {
                run = Some(Run {
                    command: "interior.place_door",
                    params: json!({
                        "id":descriptor["id"], "at_mm":self.at, "width_mm":self.width, "height_mm":self.height
                    }),
                });
            }
        });
        if let Some(error) = &self.error {
            ui.colored_label(theme::ACCENT, error);
        }
        if ui.button("Cancel door").clicked() {
            self.cancel();
        }
        ui.separator();
        run
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn descriptor(id: &str, end: f64) -> Value {
        json!({"id":id,"editable":true,"properties":[{"key":"a","value":[0.0,0.0]},{"key":"b","value":[end,0.0]}]})
    }

    #[test]
    fn door_form_emits_only_on_a_human_click_and_rebinds_to_the_current_wall() {
        let mut editor = Editor::default();
        editor.start();
        let ctx = egui::Context::default();
        theme::apply(&ctx);
        let mut time = 0.0;
        let mut draw = |editor: &mut Editor, descriptor: &Value, generation, blocked, events| {
            time += 0.05;
            let mut run = None;
            let mut output = ctx.run_ui(
                egui::RawInput {
                    screen_rect: Some(egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(280.0, 500.0))),
                    time: Some(time),
                    events,
                    ..Default::default()
                },
                |ui| {
                    run = editor.ui(ui, descriptor, generation, blocked);
                },
            );
            output.textures_delta.clear();
            (run, output.shapes)
        };
        let source = descriptor("partition.L0", 6000.0);
        let (run, shapes) = draw(&mut editor, &source, 1, None, vec![]);
        assert!(run.is_none());
        let point = shapes
            .iter()
            .find_map(|shape| match &shape.shape {
                egui::Shape::Text(text) if text.galley.text() == "Add door" => {
                    let rect = egui::Rect::from_min_size(text.pos, text.galley.size());
                    assert!(shape.clip_rect.contains_rect(rect));
                    Some(rect.center())
                }
                _ => None,
            })
            .unwrap();
        let mut accepted = None;
        for event in [
            egui::Event::PointerMoved(point),
            egui::Event::PointerButton { pos: point, button: egui::PointerButton::Primary, pressed: true, modifiers: Default::default() },
            egui::Event::PointerButton { pos: point, button: egui::PointerButton::Primary, pressed: false, modifiers: Default::default() },
        ] {
            if let (Some(run), _) = draw(&mut editor, &source, 1, None, vec![event]) {
                accepted = Some(run);
            }
        }
        let run = accepted.unwrap();
        assert_eq!(run.command, "interior.place_door");
        assert_eq!(run.params, json!({"id":"partition.L0","at_mm":3000.0,"width_mm":900.0,"height_mm":2100.0}));
        editor.outcome(Some("Opening overlaps another door".into()));
        draw(&mut editor, &source, 1, None, vec![]);
        assert_eq!(editor.at, 3000.0);
        assert!(editor.error.is_some(), "failed commands retain the reviewed form and error");
        draw(&mut editor, &source, 2, None, vec![]);
        assert!(editor.needs_review, "the same visible ID after a storey/model change must require a fresh review");
        draw(&mut editor, &descriptor("other.L1", 8000.0), 3, Some("building"), vec![]);
        assert_eq!(editor.at, 4000.0, "a new host cannot reuse the previous wall coordinate");
        assert!(editor.error.is_none());
        ctx.data_mut(|data| data.clear());
    }
}
