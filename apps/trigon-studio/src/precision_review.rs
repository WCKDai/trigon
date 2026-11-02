//! One shared review step for human and AI precision previews. The returned action and
//! conflict token are applied unchanged; displaying a preview never changes the model.

use eframe::egui;
use serde_json::{Value, json};

use crate::{authoring::Run, theme};

#[derive(Default)]
pub struct Review {
    value: Option<Value>,
}

impl Review {
    pub fn set(&mut self, value: Value) {
        self.value = (value["applied"] == false
            && value["preview_token"].as_str().is_some_and(|token| !token.is_empty())
            && value["action"]["command"].is_string()
            && value["action"]["params"].is_object())
        .then_some(value);
    }

    pub fn clear(&mut self) {
        self.value = None;
    }

    pub fn is_pending(&self) -> bool {
        self.value.is_some()
    }

    fn apply(&self) -> Option<Run> {
        let value = self.value.as_ref()?;
        Some(Run { command: "precision.apply", params: json!({"action":value["action"],"preview_token":value["preview_token"]}) })
    }

    pub fn ui(&mut self, ui: &mut egui::Ui, blocked: Option<&str>) -> Option<Run> {
        let value = self.value.as_ref()?;
        let mut apply = false;
        let mut cancel = false;
        egui::Frame::new().fill(theme::CHROME).inner_margin(10.0).corner_radius(4).show(ui, |ui| {
            theme::eyebrow(ui, "PRECISION PREVIEW · NOT APPLIED");
            let command = value["action"]["command"].as_str().unwrap_or_default();
            ui.heading(match command {
                "edit.align_walls" => "Align to reference",
                "edit.copy_to_levels" => "Copy to levels",
                "entrance.set_unit" => "Update apartment record",
                "entrance.bind_stair" => "Connect entrance to stair",
                "annotation.define" => "Save reference guide",
                _ => "Review model change",
            });
            ui.label(format!("{} source fields will change", value["changes"].as_array().map_or(0, Vec::len)));
            let validation = &value["validation"];
            ui.label(format!("Model: {} · Annotation structure: {}", status(&validation["model_build"]), status(&validation["annotations"])));
            ui.label(format!("Drawing review: {}", status(&validation["drawing_quality"]).replace('_', " ")));
            if let Some(reports) = value["drawing_feedback"].as_array() {
                for report in reports.iter().take(8) {
                    let view = report["view"].as_str().unwrap_or("Drawing");
                    let status = report["status"].as_str().unwrap_or("unverified").replace('_', " ");
                    ui.add(egui::Label::new(format!("{view}: {status}")).wrap());
                }
                if reports.len() > 8 {
                    ui.weak(format!("…and {} more checked drawings", reports.len() - 8));
                }
            }
            if let Some(status) = validation["sanity"].as_str() {
                ui.label(format!("Geometry audit: {}", status.replace('_', " ")));
                if let Some(findings) = value["sanity_feedback"]["total_findings"].as_u64() {
                    ui.weak(format!("{findings} review candidates; a valid build does not establish measured accuracy."));
                }
            }
            let result = &value["result"];
            if let Some(id) = result["id"].as_str() {
                ui.add(egui::Label::new(id).wrap());
            }
            if let Some(units) = result["unit"].as_object() {
                let floor = units.get("floor").and_then(Value::as_str).unwrap_or_default();
                let door = units.get("door").and_then(Value::as_str).filter(|door| !door.trim().is_empty()).unwrap_or("unknown door");
                ui.label(format!("Apartment: {floor} · {door}"));
            }
            if result["unphased"] == true {
                ui.weak("Unit registers are unphased. Missing apartment sides are never inferred.");
            }
            let alignment = &result["alignment"];
            if let Some(reference) = alignment["reference"]["id"].as_str() {
                ui.add(egui::Label::new(format!("Reference: {reference}")).wrap());
                if let Some(line) = alignment["reference"]["line_mm"].as_array() {
                    for (index, point) in line.iter().enumerate() {
                        if let (Some(x), Some(y)) = (point[0].as_f64(), point[1].as_f64()) {
                            ui.label(format!("{}: {x:.6}, {y:.6} mm", if index == 0 { "From" } else { "To" }));
                        }
                    }
                }
                ui.weak("One-time alignment. Lengths and hosted door distances remain fixed.");
                if let Some(limit) = alignment["max_move_mm"].as_f64() {
                    ui.label(format!("Allowed endpoint motion: {limit:.6} mm"));
                }
            }
            if let Some(changes) = alignment["changes"].as_array() {
                for change in changes.iter().take(12) {
                    ui.add(egui::Label::new(change["target_id"].as_str().unwrap_or_default()).wrap());
                    ui.label(format!(
                        "Move ≤ {:.6} mm · rotate {:.6}°",
                        change["max_endpoint_move_mm"].as_f64().unwrap_or(0.0),
                        change["rotation_deg"].as_f64().unwrap_or(0.0)
                    ));
                }
                if changes.len() > 12 {
                    ui.weak(format!("…and {} more walls", changes.len() - 12));
                }
            }
            if let Some(targets) = result["targets"].as_array() {
                egui::CollapsingHeader::new(format!("Affected elements ({})", targets.len())).show(ui, |ui| {
                    for target in targets.iter().take(100) {
                        ui.add(egui::Label::new(target.as_str().unwrap_or_default()).wrap());
                    }
                    if targets.len() > 100 {
                        ui.weak(format!("…and {} more elements", targets.len() - 100));
                    }
                });
            }
            if let Some(reason) = blocked {
                ui.colored_label(theme::ACCENT, reason);
            }
            ui.horizontal_wrapped(|ui| {
                apply = ui
                    .add_enabled(blocked.is_none(), egui::Button::new(egui::RichText::new("Apply change").color(theme::CHROME)).fill(theme::ACCENT))
                    .clicked();
                cancel = ui.button("Cancel preview").clicked();
            });
        });
        if cancel {
            self.clear();
        }
        apply.then(|| self.apply()).flatten()
    }
}

fn status(value: &Value) -> &str {
    value.as_str().unwrap_or("unverified")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn applying_preserves_the_exact_captured_action_and_token() {
        let action = json!({"command":"edit.align_walls","params":{"targets":["wall.L2"],
            "reference":{"kind":"annotation","id":"guide"},"max_move_mm":0.1256789}});
        let mut review = Review::default();
        review.set(json!({"applied":false,"action":action,"preview_token":"precision/1-exact"}));
        assert!(review.is_pending());
        let run = review.apply().unwrap();
        assert_eq!(run.command, "precision.apply");
        assert_eq!(run.params, json!({"action":action,"preview_token":"precision/1-exact"}));
        review.clear();
        assert!(review.apply().is_none());
    }

    #[test]
    fn incomplete_or_already_applied_responses_cannot_be_applied() {
        let mut review = Review::default();
        for value in [json!(null), json!({"applied":true}), json!({"applied":false,"action":{},"preview_token":""})] {
            review.set(value);
            assert!(!review.is_pending());
            assert!(review.apply().is_none());
        }
    }
}
