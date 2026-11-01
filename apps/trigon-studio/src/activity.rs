//! The activity feed: every command run in this session, by whom (the owner or an agent), with
//! its parameters, result or error, how long it took and what it changed in the model. This is
//! where the owner watches the agent work.

use std::collections::VecDeque;
use std::time::{Duration, Instant};

use eframe::egui;
use serde_json::Value;

/// Entries kept; older ones fall off.
const MAX_ENTRIES: usize = 2000;
/// Longest JSON text shown for a parameter set or result.
const MAX_TEXT: usize = 64 * 1024;

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Who {
    Owner,
    Agent(String),
}

impl Who {
    pub fn label(&self) -> &str {
        match self {
            Who::Owner => "you",
            Who::Agent(name) => name,
        }
    }
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct Changes {
    pub changed: Vec<String>,
    pub removed: Vec<String>,
}

pub struct Entry {
    pub seq: u64,
    pub who: Who,
    pub command: String,
    pub params: Value,
    pub result: Result<Value, String>,
    pub ms: f64,
    pub at: Instant,
    /// What the rebuild after it changed (model-changing commands only).
    pub changes: Option<Changes>,
}

#[derive(Default)]
pub struct Feed {
    entries: VecDeque<Entry>,
    next: u64,
    open: Option<u64>,
}

pub enum Action {
    Undo,
    Redo,
}

/// Pretty JSON, cut to a readable length.
pub fn pretty(v: &Value) -> String {
    let mut s = serde_json::to_string_pretty(v).unwrap_or_default();
    if s.len() > MAX_TEXT {
        let mut cut = MAX_TEXT;
        while !s.is_char_boundary(cut) {
            cut -= 1;
        }
        s.truncate(cut);
        s.push_str("\n… (cut)");
    }
    s
}

/// One line about a result: a count, the first fields, or the error.
fn gist(r: &Result<Value, String>) -> String {
    let text = match r {
        Err(e) => e.clone(),
        Ok(Value::Array(a)) => format!("{} items", a.len()),
        Ok(Value::Object(o)) => o.iter().take(3).map(|(k, v)| format!("{k}: {}", short(v))).collect::<Vec<_>>().join(", "),
        Ok(v) => short(v),
    };
    text.chars().take(160).collect()
}

fn short(v: &Value) -> String {
    match v {
        Value::Array(a) => format!("[{}]", a.len()),
        Value::Object(o) => format!("{{{}}}", o.len()),
        Value::String(s) => s.chars().take(40).collect(),
        other => other.to_string().chars().take(40).collect(),
    }
}

fn age(d: Duration) -> String {
    match d.as_secs() {
        0..=4 => "now".into(),
        s @ 5..=59 => format!("{s} s"),
        s @ 60..=3599 => format!("{} min", s / 60),
        s => format!("{} h", s / 3600),
    }
}

impl Feed {
    pub fn push(&mut self, who: Who, command: &str, params: Value, result: Result<Value, String>, ms: f64) -> u64 {
        self.next += 1;
        if self.entries.len() >= MAX_ENTRIES {
            self.entries.pop_front();
        }
        self.entries.push_back(Entry { seq: self.next, who, command: command.into(), params, result, ms, at: Instant::now(), changes: None });
        self.next
    }

    pub fn set_changes(&mut self, seq: u64, changes: Changes) {
        if let Some(e) = self.entries.iter_mut().rev().find(|e| e.seq == seq) {
            e.changes = Some(changes);
        }
    }

    #[cfg(test)]
    pub fn len(&self) -> usize {
        self.entries.len()
    }

    pub fn latest(&self) -> Option<&Entry> {
        self.entries.back()
    }

    pub fn ui(&mut self, ui: &mut egui::Ui, can_undo: bool, can_redo: bool) -> Option<Action> {
        let mut action = None;
        ui.horizontal(|ui| {
            ui.heading("Activity");
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if ui.add_enabled(can_redo, egui::Button::new("Redo")).on_hover_text("Redo (⇧⌘Z)").clicked() {
                    action = Some(Action::Redo);
                }
                if ui.add_enabled(can_undo, egui::Button::new("Undo")).on_hover_text("Undo (⌘Z)").clicked() {
                    action = Some(Action::Undo);
                }
            });
        });
        ui.separator();
        if self.entries.is_empty() {
            ui.weak("Commands you or an agent run appear here, newest first.");
            return action;
        }
        egui::ScrollArea::vertical().auto_shrink([false, false]).show(ui, |ui| {
            for e in self.entries.iter().rev() {
                let open = self.open == Some(e.seq);
                let colour = match (&e.who, &e.result) {
                    (_, Err(_)) => egui::Color32::from_rgb(210, 60, 50),
                    (Who::Agent(_), Ok(_)) => egui::Color32::from_rgb(70, 140, 230),
                    (Who::Owner, Ok(_)) => egui::Color32::from_gray(150),
                };
                let frame = egui::Frame::group(ui.style()).inner_margin(6.0).fill(if open {
                    ui.visuals().faint_bg_color
                } else {
                    egui::Color32::TRANSPARENT
                });
                let summary = frame
                    .show(ui, |ui| {
                        ui.set_width(ui.available_width());
                        // The summary toggles the details; the details stay selectable.
                        let summary = ui
                            .scope(|ui| {
                                ui.horizontal(|ui| {
                                    dot(ui, colour);
                                    ui.add(egui::Label::new(egui::RichText::new(&e.command).strong()).wrap());
                                });
                                ui.horizontal_wrapped(|ui| {
                                    let who = egui::RichText::new(e.who.label()).small();
                                    match e.who {
                                        Who::Agent(_) => ui.label(who.color(egui::Color32::from_rgb(70, 140, 230))),
                                        Who::Owner => ui.label(who.weak()),
                                    };
                                    ui.weak(if e.ms < 10.0 { format!("{:.1} ms", e.ms) } else { format!("{:.0} ms", e.ms) });
                                    ui.weak(age(e.at.elapsed()));
                                });
                                let line = gist(&e.result);
                                if e.result.is_err() {
                                    ui.colored_label(egui::Color32::from_rgb(200, 70, 60), line);
                                } else if !line.is_empty() {
                                    ui.weak(line);
                                }
                                if let Some(c) = &e.changes {
                                    let mut parts = Vec::new();
                                    if !c.changed.is_empty() {
                                        parts.push(format!("changed {}", list(&c.changed)));
                                    }
                                    if !c.removed.is_empty() {
                                        parts.push(format!("removed {}", list(&c.removed)));
                                    }
                                    if parts.is_empty() {
                                        parts.push(
                                            if e.command.starts_with("annotation.") || e.command.starts_with("view.") {
                                                "drawing updated"
                                            } else {
                                                "no visible change"
                                            }
                                            .into(),
                                        );
                                    }
                                    ui.label(egui::RichText::new(parts.join("; ")).small().color(egui::Color32::from_rgb(200, 130, 20)));
                                }
                            })
                            .response;
                        if open {
                            ui.add_space(4.0);
                            ui.label(egui::RichText::new("Parameters").small().strong());
                            json_block(ui, &pretty(&e.params));
                            ui.label(egui::RichText::new(if e.result.is_ok() { "Result" } else { "Error" }).small().strong());
                            match &e.result {
                                Ok(v) => json_block(ui, &pretty(v)),
                                Err(err) => json_block(ui, err),
                            }
                        }
                        summary
                    })
                    .inner;
                if summary.interact(egui::Sense::click()).on_hover_cursor(egui::CursorIcon::PointingHand).clicked() {
                    self.open = if open { None } else { Some(e.seq) };
                }
            }
        });
        action
    }
}

/// A small filled circle in the text line (the default fonts have no bullet glyphs).
pub fn dot(ui: &mut egui::Ui, colour: egui::Color32) {
    let (r, _) = ui.allocate_exact_size(egui::vec2(10.0, 10.0), egui::Sense::hover());
    ui.painter().circle_filled(r.center(), 4.0, colour);
}

fn list(ids: &[String]) -> String {
    let shown: Vec<&str> = ids.iter().take(4).map(String::as_str).collect();
    if ids.len() > 4 { format!("{} (+{})", shown.join(", "), ids.len() - 4) } else { shown.join(", ") }
}

fn json_block(ui: &mut egui::Ui, text: &str) {
    egui::Frame::new().fill(ui.visuals().extreme_bg_color).inner_margin(4.0).show(ui, |ui| {
        ui.set_width(ui.available_width());
        egui::ScrollArea::vertical().max_height(240.0).id_salt(text.len()).show(ui, |ui| {
            ui.add(egui::Label::new(egui::RichText::new(text).monospace().small()).wrap().selectable(true));
        });
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn the_feed_keeps_the_newest_entries_and_their_changes() {
        let mut f = Feed::default();
        for i in 0..MAX_ENTRIES + 5 {
            f.push(Who::Owner, "facade.list", json!({}), Ok(json!(i)), 1.0);
        }
        assert_eq!(f.len(), MAX_ENTRIES);
        let seq = f.push(Who::Agent("claude-code".into()), "facade.place", json!({"facade": "b1.f1"}), Err("no".into()), 2.0);
        f.set_changes(seq, Changes { changed: vec!["w1".into()], removed: vec![] });
        let last = f.latest().unwrap();
        assert_eq!((last.seq, last.who.label()), (seq, "claude-code"));
        assert_eq!(last.changes.as_ref().unwrap().changed, ["w1"]);
    }

    #[test]
    fn long_results_are_cut_on_a_character_boundary() {
        let v = json!("ø".repeat(MAX_TEXT));
        let p = pretty(&v);
        assert!(p.len() <= MAX_TEXT + 20 && p.ends_with("(cut)"));
        assert_eq!(gist(&Ok(json!({"a": 1, "b": [1, 2], "c": {"x": 1}, "d": 4}))), "a: 1, b: [2], c: {1}");
        assert_eq!(gist(&Err("x".repeat(500))).len(), 160);
    }
}
