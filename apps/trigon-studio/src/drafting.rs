//! Drawing authoring forms produce the same engine commands used by CLI/MCP. Picking fills
//! typed millimetre fields; only the explicit Add/Update button changes the document.

use eframe::egui;
use serde_json::json;
use trigon_engine::SnapOptions;
use trigon_model::Document;
use trigon_model::annotation::{Annotation, AnnotationKind, DrawingViewKind};

use crate::{authoring::Run, theme};

pub enum Action {
    Open(String),
    Run(Run),
}

pub struct Drafting {
    view_id: String,
    view_name: String,
    view_kind: usize,
    facade: String,
    level: usize,
    line: [[f64; 2]; 2],
    depth: f64,
    flip: bool,
    current_view: String,
    pub selected: Option<String>,
    note_id: String,
    kind: usize,
    points: [[f64; 2]; 3],
    text: String,
    size: f64,
    rotation: f64,
    offset: f64,
    datum: f64,
    pub picking: Option<usize>,
    pub snaps: SnapOptions,
    grid: bool,
    grid_step: f64,
    export_path: String,
}

impl Default for Drafting {
    fn default() -> Self {
        Self {
            view_id: "drawing-1".into(),
            view_name: "Ground floor".into(),
            view_kind: 0,
            facade: String::new(),
            level: 0,
            line: [[0.0, 0.0], [10000.0, 0.0]],
            depth: 30000.0,
            flip: false,
            current_view: String::new(),
            selected: None,
            note_id: "annotation-1".into(),
            kind: 0,
            points: [[0.0, 0.0], [1000.0, 0.0], [2000.0, 1000.0]],
            text: "Note".into(),
            size: 2.5,
            rotation: 0.0,
            offset: 500.0,
            datum: 0.0,
            picking: None,
            snaps: SnapOptions::default(),
            grid: true,
            grid_step: 100.0,
            export_path: "out/drawing.pdf".into(),
        }
    }
}

fn number(ui: &mut egui::Ui, value: &mut f64) -> egui::Response {
    ui.add(egui::DragValue::new(value).range(-1_000_000.0..=1_000_000.0).max_decimals(3).speed(1.0).suffix(" mm"))
}

fn point(ui: &mut egui::Ui, label: &str, value: &mut [f64; 2]) {
    ui.label(label);
    ui.horizontal(|ui| {
        number(ui, &mut value[0]);
        number(ui, &mut value[1]);
    });
}

fn unique_id(prefix: &str, used: impl Fn(&str) -> bool) -> String {
    (1..=10000).map(|n| format!("{prefix}-{n}")).find(|id| !used(id)).unwrap_or_else(|| format!("{prefix}-new"))
}

impl Drafting {
    pub fn library(&mut self, ui: &mut egui::Ui, doc: &Document, active: &str) -> Option<Action> {
        theme::eyebrow(ui, "SAVED DRAWINGS");
        ui.heading("Drawing workspace");
        ui.weak("Plans, facade elevations and sections");
        let mut action = None;
        for v in &doc.views {
            let label = match v.kind {
                DrawingViewKind::Plan { .. } => "PLAN",
                DrawingViewKind::Facade { .. } => "ELEVATION",
                DrawingViewKind::Section { .. } => "SECTION",
            };
            let valid = v.validate_source(doc).is_ok();
            let response = ui.selectable_label(active == v.id, format!("{}\n{label} · {}", v.name, v.id));
            if !valid {
                response.clone().on_hover_text("The source changed. Repair the frame or remove the drawing with the view commands.");
            }
            if response.clicked() {
                action = Some(Action::Open(v.id.clone()));
            }
        }
        if doc.views.is_empty() {
            ui.weak("Save a drawing frame below to begin annotating.");
        }
        ui.add_space(10.0);
        egui::CollapsingHeader::new("New drawing frame").default_open(doc.views.is_empty()).show(ui, |ui| {
            ui.label("Drawing id");
            ui.text_edit_singleline(&mut self.view_id);
            ui.label("Name");
            ui.text_edit_singleline(&mut self.view_name);
            ui.horizontal_wrapped(|ui| {
                ui.selectable_value(&mut self.view_kind, 0, "Plan");
                ui.selectable_value(&mut self.view_kind, 1, "Facade");
                ui.selectable_value(&mut self.view_kind, 2, "Section");
            });
            match self.view_kind {
                0 => {
                    let levels = doc.buildings.first().map(|b| b.storeys.as_slice()).unwrap_or_default();
                    let label = levels.get(self.level).map_or("Choose level", |s| s.name.as_str());
                    egui::ComboBox::from_id_salt("draft-level").selected_text(label).show_ui(ui, |ui| {
                        for (i, l) in levels.iter().enumerate() {
                            ui.selectable_value(&mut self.level, i, &l.name);
                        }
                    });
                }
                1 => {
                    if self.facade.is_empty() {
                        self.facade = doc.buildings.iter().flat_map(|b| &b.facades).next().map(|f| f.id.clone()).unwrap_or_default();
                    }
                    egui::ComboBox::from_id_salt("draft-facade").selected_text(&self.facade).show_ui(ui, |ui| {
                        for f in doc.buildings.iter().flat_map(|b| &b.facades) {
                            ui.selectable_value(&mut self.facade, f.id.clone(), &f.id);
                        }
                    });
                }
                _ => {
                    point(ui, "Cut from · site x / y", &mut self.line[0]);
                    point(ui, "Cut to · site x / y", &mut self.line[1]);
                    ui.label("Depth beyond cut");
                    number(ui, &mut self.depth);
                    ui.checkbox(&mut self.flip, "Look right of the cut");
                }
            }
            if ui
                .add_enabled(
                    !doc.buildings.is_empty() && !doc.views.iter().any(|v| v.id == self.view_id),
                    egui::Button::new(egui::RichText::new("Create drawing").color(egui::Color32::WHITE))
                        .fill(theme::ACCENT)
                        .stroke(egui::Stroke::NONE),
                )
                .clicked()
            {
                let kind = match self.view_kind {
                    0 => DrawingViewKind::Plan { level: self.level },
                    1 => DrawingViewKind::Facade { facade: self.facade.clone() },
                    _ => DrawingViewKind::Section { from: self.line[0], to: self.line[1], flip: self.flip, depth_mm: self.depth },
                };
                action = Some(Action::Run(Run { command: "view.define", params: json!({"id":self.view_id,"name":self.view_name,"kind":kind}) }));
            }
        });
        action
    }

    pub fn opened(&mut self, doc: &Document, id: &str) {
        if self.current_view != id {
            self.current_view = id.into();
            self.new_note(doc);
            self.export_path = format!("out/{id}.pdf");
        }
        self.view_id = unique_id("drawing", |id| doc.views.iter().any(|v| v.id == id));
    }

    pub fn new_note(&mut self, doc: &Document) {
        self.selected = None;
        self.note_id = unique_id("annotation", |id| doc.annotations.iter().any(|a| a.id == id));
        self.picking = None;
    }

    pub fn select(&mut self, a: &Annotation) {
        self.selected = Some(a.id.clone());
        self.note_id = a.id.clone();
        self.size = a.size_mm;
        self.picking = None;
        match &a.kind {
            AnnotationKind::Text { at, text, rotation_deg } => {
                self.kind = 0;
                self.points[0] = *at;
                self.text = text.clone();
                self.rotation = *rotation_deg;
            }
            AnnotationKind::Dimension { from, to, offset_mm } => {
                self.kind = 1;
                self.points[0] = *from;
                self.points[1] = *to;
                self.offset = *offset_mm;
            }
            AnnotationKind::Leader { tip, elbow, at, text } => {
                self.kind = 2;
                self.points = [*tip, *elbow, *at];
                self.text = text.clone();
            }
            AnnotationKind::Level { at, datum_mm, label } => {
                self.kind = 3;
                self.points[0] = *at;
                self.datum = *datum_mm;
                self.text = label.clone();
            }
        }
    }

    pub fn options(&self) -> SnapOptions {
        SnapOptions { grid_mm: self.grid.then_some(self.grid_step), ..self.snaps }
    }

    pub fn picked(&mut self, p: [f64; 2]) {
        if let Some(i) = self.picking.filter(|i| *i < 3) {
            self.points[i] = p;
            self.picking = match (self.kind, i) {
                (1, 0) | (2, 0) => Some(1),
                (2, 1) => Some(2),
                _ => None,
            };
        }
    }

    pub fn hit<'a>(doc: &'a Document, view: &str, point: [f64; 2], tolerance: f64) -> Option<&'a Annotation> {
        let segment_distance = |a: [f64; 2], b: [f64; 2]| {
            let d = [b[0] - a[0], b[1] - a[1]];
            let l = d[0] * d[0] + d[1] * d[1];
            let t = if l > 1e-12 { (((point[0] - a[0]) * d[0] + (point[1] - a[1]) * d[1]) / l).clamp(0.0, 1.0) } else { 0.0 };
            (point[0] - a[0] - t * d[0]).hypot(point[1] - a[1] - t * d[1])
        };
        doc.annotations
            .iter()
            .filter(|a| a.view == view)
            .filter_map(|a| {
                let distance = match a.kind {
                    AnnotationKind::Text { at, .. } | AnnotationKind::Level { at, .. } => segment_distance(at, at),
                    AnnotationKind::Leader { tip, elbow, at, .. } => segment_distance(tip, elbow).min(segment_distance(elbow, at)),
                    AnnotationKind::Dimension { from, to, offset_mm } => {
                        let length = (to[0] - from[0]).hypot(to[1] - from[1]);
                        if length < 0.001 {
                            return None;
                        }
                        let shift = [-(to[1] - from[1]) / length * offset_mm, (to[0] - from[0]) / length * offset_mm];
                        let a = [from[0] + shift[0], from[1] + shift[1]];
                        let b = [to[0] + shift[0], to[1] + shift[1]];
                        segment_distance(a, b).min(segment_distance(from, a)).min(segment_distance(to, b))
                    }
                };
                (distance <= tolerance).then_some((distance, a))
            })
            .min_by(|a, b| a.0.total_cmp(&b.0))
            .map(|(_, a)| a)
    }

    pub fn points(&self) -> &[[f64; 2]] {
        &self.points[..match self.kind {
            1 => 2,
            2 => 3,
            _ => 1,
        }]
    }

    pub fn properties(&mut self, ui: &mut egui::Ui, doc: &Document, id: &str) -> Option<Run> {
        self.opened(doc, id);
        let Some(view) = doc.views.iter().find(|v| v.id == id) else {
            ui.weak("Choose a saved drawing in the Drawings browser.");
            return None;
        };
        let mut run = None;
        ui.heading(&view.name);
        ui.weak("Drawing-plane millimetres · paper-size lettering");
        ui.separator();
        theme::eyebrow(ui, "ANNOTATIONS");
        for a in doc.annotations.iter().filter(|a| a.view == id) {
            let label = match &a.kind {
                AnnotationKind::Text { text, .. } => format!("Text · {text}"),
                AnnotationKind::Dimension { .. } => format!("Dimension · {}", a.id),
                AnnotationKind::Leader { text, .. } => format!("Tag · {text}"),
                AnnotationKind::Level { label, .. } => format!("Level · {label}"),
            };
            if ui.selectable_label(self.selected.as_deref() == Some(&a.id), label).clicked() {
                self.select(a);
            }
        }
        if self.selected.as_ref().is_some_and(|id| !doc.annotations.iter().any(|a| &a.id == id && a.view == self.current_view)) {
            self.new_note(doc);
        }
        if ui.button("New annotation").clicked() {
            self.new_note(doc);
        }
        ui.separator();
        ui.horizontal_wrapped(|ui| {
            for (i, name) in ["Text", "Dimension", "Leader", "Level"].into_iter().enumerate() {
                if ui
                    .add_enabled(i != 3 || !matches!(view.kind, DrawingViewKind::Plan { .. }), egui::Button::selectable(self.kind == i, name))
                    .clicked()
                {
                    self.kind = i;
                    self.picking = None;
                }
            }
        });
        ui.label("Annotation id");
        ui.add_enabled(self.selected.is_none(), egui::TextEdit::singleline(&mut self.note_id));
        let labels: &[&str] = match self.kind {
            1 => &["From", "To"],
            2 => &["Arrow tip", "Bend", "Text position"],
            _ => &["Position"],
        };
        for (i, label) in labels.iter().enumerate() {
            ui.horizontal(|ui| {
                ui.label(*label);
                if ui.selectable_label(self.picking == Some(i), if self.picking == Some(i) { "Picking…" } else { "Pick" }).clicked() {
                    self.picking = if self.picking == Some(i) { None } else { Some(i) };
                }
            });
            ui.horizontal(|ui| {
                number(ui, &mut self.points[i][0]);
                number(ui, &mut self.points[i][1]);
            });
        }
        match self.kind {
            0 => {
                ui.label("Text");
                ui.text_edit_singleline(&mut self.text);
                ui.horizontal(|ui| {
                    ui.label("Rotation");
                    ui.add(egui::DragValue::new(&mut self.rotation).range(-360.0..=360.0).suffix("°"));
                });
            }
            1 => {
                ui.label("Offset · left of From to To");
                number(ui, &mut self.offset);
                let [a, b] = [self.points[0], self.points[1]];
                ui.weak(format!("Measured length: {:.3} mm", (b[0] - a[0]).hypot(b[1] - a[1])));
            }
            2 => {
                ui.label("Tag text");
                ui.text_edit_singleline(&mut self.text);
            }
            _ => {
                ui.label("Level label");
                ui.text_edit_singleline(&mut self.text);
                ui.label("Zero datum height");
                number(ui, &mut self.datum);
            }
        }
        ui.horizontal(|ui| {
            ui.label("Paper text size");
            ui.add(egui::DragValue::new(&mut self.size).range(0.5..=12.0).speed(0.1).suffix(" mm"));
        });
        ui.horizontal_wrapped(|ui| {
            if ui.button(if self.selected.is_some() { "Update annotation" } else { "Add annotation" }).clicked() {
                run = Some(Run {
                    command: "annotation.define",
                    params: json!({"id":self.note_id,"view":id,"size_mm":self.size,"kind":self.annotation_kind()}),
                });
            }
            if let Some(selected) = &self.selected
                && ui.button("Delete annotation").clicked()
            {
                run = Some(Run { command: "annotation.remove", params: json!({"id":selected}) });
            }
        });
        if self.picking.is_some() {
            ui.colored_label(theme::ACCENT, "Click the drawing to fill this point. Esc cancels. Middle-drag pans; Alt bypasses snaps.");
        }
        egui::CollapsingHeader::new("Snapping").default_open(true).show(ui, |ui| {
            ui.horizontal_wrapped(|ui| {
                ui.checkbox(&mut self.snaps.endpoints, "Endpoints");
                ui.checkbox(&mut self.snaps.midpoints, "Midpoints");
                ui.checkbox(&mut self.snaps.intersections, "Intersections");
            });
            ui.horizontal(|ui| {
                ui.checkbox(&mut self.grid, "Grid");
                ui.add(egui::DragValue::new(&mut self.grid_step).range(0.01..=100000.0).suffix(" mm"));
            });
        });
        egui::CollapsingHeader::new("Export drawing").show(ui, |ui| {
            ui.weak("PDF, SVG, PNG or DXF · file extension chooses format");
            ui.text_edit_singleline(&mut self.export_path);
            if ui.button("Export annotated view").clicked() {
                run = Some(Run { command: "view.export", params: json!({"id":id,"path":self.export_path}) });
            }
        });
        run
    }

    fn annotation_kind(&self) -> AnnotationKind {
        match self.kind {
            1 => AnnotationKind::Dimension { from: self.points[0], to: self.points[1], offset_mm: self.offset },
            2 => AnnotationKind::Leader { tip: self.points[0], elbow: self.points[1], at: self.points[2], text: self.text.clone() },
            3 => AnnotationKind::Level { at: self.points[0], datum_mm: self.datum, label: self.text.clone() },
            _ => AnnotationKind::Text { at: self.points[0], text: self.text.clone(), rotation_deg: self.rotation },
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn picking_fills_typed_points_without_mutating_the_document() {
        let doc = Document::default();
        let before = doc.clone();
        let mut tool = Drafting { kind: 2, picking: Some(0), ..Drafting::default() };
        for (i, p) in [[1100.0, 2200.0], [3300.0, 4400.0], [5500.0, 6600.0]].into_iter().enumerate() {
            tool.picked(p);
            assert_eq!(tool.points[i], p);
        }
        assert!(tool.picking.is_none());
        assert_eq!(
            tool.annotation_kind(),
            AnnotationKind::Leader { tip: [1100.0, 2200.0], elbow: [3300.0, 4400.0], at: [5500.0, 6600.0], text: "Note".into() }
        );
        assert_eq!(doc, before);
        tool.picking = Some(usize::MAX);
        tool.picked([0.0, 0.0]);
    }

    #[test]
    fn annotation_picking_follows_dimension_offsets_and_view_ownership() {
        let mut doc = Document::default();
        let annotation = Annotation {
            id: "d".into(),
            view: "elevation".into(),
            size_mm: 2.5,
            kind: AnnotationKind::Dimension { from: [0.0, 0.0], to: [1000.0, 0.0], offset_mm: 500.0 },
        };
        doc.annotations.push(annotation.clone());
        assert_eq!(Drafting::hit(&doc, "elevation", [500.0, 502.0], 10.0), Some(&annotation));
        assert!(Drafting::hit(&doc, "plan", [500.0, 502.0], 10.0).is_none());
        assert!(Drafting::hit(&doc, "elevation", [500.0, 0.0], 10.0).is_none());
        let mut tool = Drafting::default();
        tool.select(&annotation);
        assert_eq!(tool.annotation_kind(), annotation.kind);
        tool.new_note(&doc);
        assert_ne!(tool.note_id, annotation.id);
        assert!(tool.selected.is_none());
    }
}
