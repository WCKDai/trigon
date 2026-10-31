//! The command palette: search every engine command, edit its parameters as JSON (prefilled from
//! the command's schema) and run it as the owner, through the same session the agent uses.

use eframe::egui;
use serde_json::{Map, Value, json};
use trigon_engine::commands;

/// A placeholder value for a schema: its default, first example or enum value, or an empty value
/// of its type; objects get their required members. `defs` resolves `$ref`.
pub fn placeholder(schema: &Value, defs: &Value, depth: usize) -> Value {
    if depth > 6 {
        return Value::Null;
    }
    if let Some(r) = schema.get("$ref").and_then(Value::as_str) {
        let name = r.rsplit('/').next().unwrap_or_default();
        return defs.get(name).map_or(Value::Null, |s| placeholder(s, defs, depth + 1));
    }
    if let Some(v) = schema.get("default").or_else(|| schema.get("const")) {
        return v.clone();
    }
    if let Some(v) = schema.get("enum").and_then(Value::as_array).and_then(|a| a.first()) {
        return v.clone();
    }
    for key in ["oneOf", "anyOf"] {
        if let Some(first) = schema.get(key).and_then(Value::as_array).and_then(|a| a.iter().find(|s| s.get("type") != Some(&json!("null")))) {
            return placeholder(first, defs, depth + 1);
        }
    }
    let ty = match schema.get("type") {
        Some(Value::String(t)) => t.as_str(),
        Some(Value::Array(ts)) => ts.iter().filter_map(Value::as_str).find(|t| *t != "null").unwrap_or("null"),
        _ if schema.get("properties").is_some() => "object",
        _ => "null",
    };
    match ty {
        "string" => json!(""),
        "integer" => json!(0),
        "number" => json!(0.0),
        "boolean" => json!(false),
        "array" => json!([]),
        "object" => {
            let required: Vec<&str> =
                schema.get("required").and_then(Value::as_array).map(|r| r.iter().filter_map(Value::as_str).collect()).unwrap_or_default();
            let mut out = Map::new();
            if let Some(props) = schema.get("properties").and_then(Value::as_object) {
                for (k, s) in props {
                    if required.contains(&k.as_str()) {
                        out.insert(k.clone(), placeholder(s, defs, depth + 1));
                    }
                }
            }
            Value::Object(out)
        }
        _ => Value::Null,
    }
}

/// The parameters a command's schema asks for, as JSON text to edit.
pub fn skeleton(schema: &Value) -> String {
    let defs = schema.get("$defs").or_else(|| schema.get("definitions")).cloned().unwrap_or(Value::Null);
    serde_json::to_string_pretty(&placeholder(schema, &defs, 0)).unwrap_or_else(|_| "{}".into())
}

/// One row per parameter: name, type, required, description.
pub fn parameters(schema: &Value) -> Vec<(String, String, bool, String)> {
    let required: Vec<&str> =
        schema.get("required").and_then(Value::as_array).map(|r| r.iter().filter_map(Value::as_str).collect()).unwrap_or_default();
    let Some(props) = schema.get("properties").and_then(Value::as_object) else { return Vec::new() };
    props
        .iter()
        .map(|(k, s)| {
            let ty = match s.get("type") {
                Some(Value::String(t)) => t.clone(),
                Some(Value::Array(ts)) => ts.iter().filter_map(Value::as_str).filter(|t| *t != "null").collect::<Vec<_>>().join(" | "),
                _ => s.get("$ref").and_then(Value::as_str).and_then(|r| r.rsplit('/').next()).unwrap_or("any").to_string(),
            };
            let doc = s.get("description").and_then(Value::as_str).unwrap_or_default().to_string();
            (k.clone(), ty, required.contains(&k.as_str()), doc)
        })
        .collect()
}

#[derive(Default)]
pub struct Palette {
    pub open: bool,
    query: String,
    selected: Option<&'static str>,
    params: String,
    outcome: Option<Result<String, String>>,
    focus_search: bool,
}

/// What the palette asks the app to run.
pub struct Run {
    pub command: &'static str,
    pub params: Value,
}

impl Palette {
    pub fn show(&mut self) {
        self.open = true;
        self.focus_search = true;
    }

    /// The result of the last run, shown under the editor.
    pub fn set_outcome(&mut self, r: Result<String, String>) {
        self.outcome = Some(r);
    }

    fn select(&mut self, id: &'static str) {
        self.selected = Some(id);
        self.outcome = None;
        self.params = commands().iter().find(|c| c.id == id).map(|c| skeleton(&c.params_schema())).unwrap_or_else(|| "{}".into());
    }

    pub fn ui(&mut self, ctx: &egui::Context) -> Option<Run> {
        let mut run = None;
        let mut open = self.open;
        egui::Window::new("Commands").open(&mut open).default_size([640.0, 520.0]).collapsible(false).show(ctx, |ui| {
            let search =
                ui.add(egui::TextEdit::singleline(&mut self.query).hint_text("Search commands (facade, plan, export…)").desired_width(f32::INFINITY));
            if self.focus_search {
                search.request_focus();
                self.focus_search = false;
            }
            let q = self.query.to_lowercase();
            let matches: Vec<_> = commands().iter().filter(|c| q.is_empty() || c.id.contains(&q) || c.summary.to_lowercase().contains(&q)).collect();
            if search.lost_focus()
                && ui.input(|i| i.key_pressed(egui::Key::Enter))
                && let Some(first) = matches.first()
            {
                self.select(first.id);
            }
            ui.columns(2, |cols| {
                egui::ScrollArea::vertical().id_salt("palette-list").max_height(420.0).show(&mut cols[0], |ui| {
                    for c in &matches {
                        let r = ui.selectable_label(self.selected == Some(c.id), c.id).on_hover_text(c.summary);
                        if r.clicked() {
                            self.select(c.id);
                        }
                    }
                    if matches.is_empty() {
                        ui.weak("No command matches.");
                    }
                });
                let ui = &mut cols[1];
                let Some(id) = self.selected else {
                    ui.weak("Pick a command to see its parameters.");
                    return;
                };
                let Some(spec) = commands().iter().find(|c| c.id == id) else { return };
                ui.strong(id);
                ui.label(egui::RichText::new(spec.summary).small());
                ui.add_space(4.0);
                let edit = egui::TextEdit::multiline(&mut self.params).code_editor().desired_rows(8).desired_width(f32::INFINITY);
                ui.add(edit);
                let parsed: Result<Value, String> =
                    if self.params.trim().is_empty() { Ok(json!({})) } else { serde_json::from_str(&self.params).map_err(|e| e.to_string()) };
                ui.horizontal(|ui| {
                    let ready = parsed.is_ok();
                    let go = ui.add_enabled(ready, egui::Button::new(if spec.mutates { "Run (changes the model)" } else { "Run" })).clicked()
                        || (ready && ui.input(|i| i.modifiers.command && i.key_pressed(egui::Key::Enter)));
                    if go && let Ok(p) = &parsed {
                        run = Some(Run { command: id, params: p.clone() });
                    }
                    if let Err(e) = &parsed {
                        ui.colored_label(egui::Color32::from_rgb(200, 70, 60), format!("JSON: {e}"));
                    }
                });
                if let Some(out) = &self.outcome {
                    let (text, colour) = match out {
                        Ok(t) => (t.as_str(), ui.visuals().text_color()),
                        Err(e) => (e.as_str(), egui::Color32::from_rgb(200, 70, 60)),
                    };
                    egui::ScrollArea::vertical().id_salt("palette-out").max_height(140.0).show(ui, |ui| {
                        ui.add(egui::Label::new(egui::RichText::new(text).monospace().small().color(colour)).wrap().selectable(true));
                    });
                }
                ui.separator();
                egui::ScrollArea::vertical().id_salt("palette-params").max_height(160.0).show(ui, |ui| {
                    for (name, ty, required, doc) in parameters(&spec.params_schema()) {
                        ui.horizontal_wrapped(|ui| {
                            ui.monospace(if required { format!("{name}*") } else { name });
                            ui.weak(ty);
                            if !doc.is_empty() {
                                ui.label(egui::RichText::new(doc).small());
                            }
                        });
                    }
                });
            });
        });
        self.open = open;
        run
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_command_gets_parameters_that_parse_as_an_object() {
        for c in commands() {
            let text = skeleton(&c.params_schema());
            let v: Value = serde_json::from_str(&text).unwrap();
            assert!(v.is_object(), "{}: {text}", c.id);
        }
    }

    #[test]
    fn required_members_are_filled_by_type_and_optional_ones_left_out() {
        let schema = json!({
            "type": "object",
            "required": ["facade", "levels", "kind", "count"],
            "properties": {
                "facade": {"type": "string", "description": "id"},
                "levels": {"type": ["array", "null"]},
                "count": {"type": "integer"},
                "kind": {"$ref": "#/$defs/Kind"},
                "width": {"type": ["number", "null"]}
            },
            "$defs": {"Kind": {"enum": ["street", "courtyard"]}}
        });
        let v: Value = serde_json::from_str(&skeleton(&schema)).unwrap();
        assert_eq!(v, json!({"facade": "", "levels": [], "count": 0, "kind": "street"}));
        let rows = parameters(&schema);
        assert_eq!(rows.len(), 5);
        assert!(rows.iter().any(|(n, t, r, d)| n == "facade" && t == "string" && *r && d == "id"));
        assert!(rows.iter().any(|(n, t, r, _)| n == "width" && t == "number" && !*r));
    }

    #[test]
    fn hostile_schemas_never_panic() {
        let looped = json!({"$ref": "#/$defs/A", "$defs": {"A": {"$ref": "#/$defs/A"}}});
        assert_eq!(skeleton(&looped), "null");
        for s in [json!(null), json!([]), json!("x"), json!({"type": 5}), json!({"required": "x", "properties": []})] {
            let _ = skeleton(&s);
            let _ = parameters(&s);
        }
    }
}
