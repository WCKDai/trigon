//! Drawing authoring forms produce the same engine commands used by CLI/MCP. Picking fills
//! typed millimetre fields; only the explicit Add/Update button changes the document.

use eframe::egui;
use serde_json::json;
use trigon_engine::SnapOptions;
use trigon_model::Document;
use trigon_model::annotation::{
    Annotation, AnnotationKind, DrawingTemplate, DrawingViewKind, FontFamily, LinePattern, LineStyle, LineWeightProfile, PlanRange, RoomLabels,
    TextStyle, ViewSizing,
};

use crate::{authoring::Run, theme};

pub enum Action {
    Open(String),
    Run(Run),
}

pub struct Drafting {
    template: DrawingTemplate,
    loaded_template: Option<(String, Option<DrawingTemplate>)>,
    template_source: String,
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
    style: TextStyle,
    stroke: LineStyle,
    pub alignment: bool,
    pub alignment_offset: f64,
    region_points: Vec<[f64; 2]>,
    region_fill: [u8; 3],
    region_border: bool,
    offset: f64,
    datum: f64,
    pub picking: Option<usize>,
    pub snaps: SnapOptions,
    grid: bool,
    grid_step: f64,
    export_path: String,
    reveal_export: bool,
}

impl Default for Drafting {
    fn default() -> Self {
        Self {
            template: DrawingTemplate::default(),
            loaded_template: None,
            template_source: String::new(),
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
            style: TextStyle::default(),
            stroke: LineStyle::default(),
            alignment: true,
            alignment_offset: 0.0,
            region_points: Vec::new(),
            region_fill: [232, 232, 220],
            region_border: true,
            offset: 500.0,
            datum: 0.0,
            picking: None,
            snaps: SnapOptions::default(),
            grid: true,
            grid_step: 100.0,
            export_path: "out/drawing.pdf".into(),
            reveal_export: false,
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

fn polygon_contains(points: &[[f64; 2]], point: [f64; 2]) -> bool {
    if !point.iter().all(|v| v.is_finite()) || points.iter().flatten().any(|v| !v.is_finite()) {
        return false;
    }
    let mut inside = false;
    for (a, b) in points.iter().zip(points.iter().cycle().skip(1)).take(points.len()) {
        if (a[1] > point[1]) != (b[1] > point[1]) && point[0] < (b[0] - a[0]) * (point[1] - a[1]) / (b[1] - a[1]) + a[0] {
            inside = !inside;
        }
    }
    inside
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
        let changed = self.current_view != id;
        if changed {
            self.current_view = id.into();
            self.new_note(doc);
            self.export_path = format!("out/{id}.pdf");
        }
        if changed || doc.views.iter().any(|v| v.id == self.view_id) {
            self.view_id = unique_id("drawing", |id| doc.views.iter().any(|v| v.id == id));
        }
    }

    pub fn new_note(&mut self, doc: &Document) {
        self.selected = None;
        self.note_id = unique_id("annotation", |id| doc.annotations.iter().any(|a| a.id == id));
        self.picking = None;
        self.region_points.clear();
    }

    pub fn select(&mut self, a: &Annotation) {
        self.selected = Some(a.id.clone());
        self.note_id = a.id.clone();
        self.size = a.size_mm;
        self.style = a.style;
        self.picking = None;
        match &a.kind {
            AnnotationKind::Line { from, to, stroke } => {
                self.kind = 5;
                self.points[0] = *from;
                self.points[1] = *to;
                self.stroke = *stroke;
            }
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
            AnnotationKind::Region { points, fill, border } => {
                self.kind = 4;
                self.region_points = points.clone();
                self.region_fill = *fill;
                self.region_border = *border;
            }
        }
    }

    pub fn show_export(&mut self) {
        self.reveal_export = true;
    }

    pub fn ribbon(&mut self, ui: &mut egui::Ui, doc: &Document, id: &str) {
        if ui.selectable_label(self.picking.is_none(), "Select").clicked() {
            self.picking = None;
        }
        let view = doc.views.iter().find(|v| v.id == id);
        for (i, label) in ["Text", "Dimension", "Leader", "Level", "Area fill", "Line"].into_iter().enumerate() {
            let enabled = view.is_some_and(|v| i != 3 || !matches!(v.kind, DrawingViewKind::Plan { .. }));
            if ui.add_enabled(enabled, egui::Button::selectable(self.picking.is_some() && self.kind == i, label)).clicked() {
                self.new_note(doc);
                self.kind = i;
                self.picking = Some(0);
            }
        }
    }

    pub fn options(&self) -> SnapOptions {
        SnapOptions { grid_mm: self.grid.then_some(self.grid_step), ..self.snaps }
    }

    pub fn alignment_options(&self, doc: &Document, view: &str) -> trigon_engine::AlignmentOptions {
        let mut anchors = Vec::new();
        for a in doc.annotations.iter().filter(|a| a.view == view && self.selected.as_deref() != Some(&a.id)) {
            match &a.kind {
                AnnotationKind::Text { at, .. } | AnnotationKind::Level { at, .. } | AnnotationKind::Leader { at, .. } => anchors.push(*at),
                _ => {}
            }
            if anchors.len() == 509 {
                break;
            }
        }
        let picked = if self.kind == 4 { self.region_points.as_slice() } else { &self.points[..self.picking.unwrap_or(0).min(3)] };
        anchors.extend(picked.iter().copied().take(512 - anchors.len()));
        trigon_engine::AlignmentOptions { anchors, offset_mm: self.alignment_offset }
    }

    pub fn picked(&mut self, p: [f64; 2]) {
        if self.kind == 4 && self.picking.is_some() {
            if self.region_points.len() < 256 && self.region_points.last().is_none_or(|last| (last[0] - p[0]).hypot(last[1] - p[1]) > 0.001) {
                self.region_points.push(p);
            }
            self.picking = (self.region_points.len() < 256).then_some(self.region_points.len());
            return;
        }
        if let Some(i) = self.picking.filter(|i| *i < 3) {
            self.points[i] = p;
            self.picking = match (self.kind, i) {
                (1, 0) | (2, 0) | (5, 0) => Some(1),
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
                let distance = match &a.kind {
                    AnnotationKind::Text { at, .. } | AnnotationKind::Level { at, .. } => segment_distance(*at, *at),
                    AnnotationKind::Line { from, to, .. } => segment_distance(*from, *to),
                    AnnotationKind::Leader { tip, elbow, at, .. } => segment_distance(*tip, *elbow).min(segment_distance(*elbow, *at)),
                    AnnotationKind::Dimension { from, to, offset_mm } => {
                        let length = (to[0] - from[0]).hypot(to[1] - from[1]);
                        if length < 0.001 {
                            return None;
                        }
                        let shift = [-(to[1] - from[1]) / length * offset_mm, (to[0] - from[0]) / length * offset_mm];
                        let a = [from[0] + shift[0], from[1] + shift[1]];
                        let b = [to[0] + shift[0], to[1] + shift[1]];
                        segment_distance(a, b).min(segment_distance(*from, a)).min(segment_distance(*to, b))
                    }
                    AnnotationKind::Region { points, .. } => {
                        if points.len() < 3 || points.len() > 256 {
                            return None;
                        }
                        if polygon_contains(points, point) {
                            0.0
                        } else {
                            points
                                .iter()
                                .zip(points.iter().cycle().skip(1))
                                .take(points.len())
                                .map(|(a, b)| segment_distance(*a, *b))
                                .min_by(f64::total_cmp)?
                        }
                    }
                };
                (distance <= tolerance).then_some((matches!(a.kind, AnnotationKind::Region { .. }), distance, a))
            })
            .min_by(|a, b| a.0.cmp(&b.0).then(a.1.total_cmp(&b.1)).then(a.2.id.cmp(&b.2.id)))
            .map(|(_, _, a)| a)
    }

    pub fn points(&self) -> &[[f64; 2]] {
        if self.kind == 4 {
            return &self.region_points;
        }
        &self.points[..match self.kind {
            1 | 5 => 2,
            2 => 3,
            _ => 1,
        }]
    }

    pub fn is_region(&self) -> bool {
        self.kind == 4
    }

    pub fn properties(&mut self, ui: &mut egui::Ui, doc: &Document, id: &str) -> Option<Run> {
        self.opened(doc, id);
        let Some(view) = doc.views.iter().find(|v| v.id == id) else {
            ui.weak("Choose a saved drawing in the Drawings browser.");
            return None;
        };
        let mut run = None;
        if let Some(run) = self.template_ui(ui, doc, view) {
            return Some(run);
        }
        ui.heading(&view.name);
        ui.weak("Drawing-plane millimetres · paper-size lettering");
        ui.separator();
        theme::eyebrow(ui, "ANNOTATIONS");
        for a in doc.annotations.iter().filter(|a| a.view == id) {
            let label = match &a.kind {
                AnnotationKind::Text { text, .. } => format!("Text · {text}"),
                AnnotationKind::Line { .. } => format!("Manual line · {}", a.id),
                AnnotationKind::Dimension { .. } => format!("Dimension · {}", a.id),
                AnnotationKind::Leader { text, .. } => format!("Tag · {text}"),
                AnnotationKind::Level { label, .. } => format!("Level · {label}"),
                AnnotationKind::Region { points, .. } => format!("Area fill · {} · {} vertices", a.id, points.len()),
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
            for (i, name) in ["Text", "Dimension", "Leader", "Level", "Area fill", "Line"].into_iter().enumerate() {
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
            1 | 5 => &["From", "To"],
            2 => &["Arrow tip", "Bend", "Text position"],
            4 => &[],
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
                ui.label("Offset · positive to the left");
                number(ui, &mut self.offset);
                let [a, b] = [self.points[0], self.points[1]];
                ui.weak(format!("Measured length: {:.3} mm", (b[0] - a[0]).hypot(b[1] - a[1])));
            }
            2 => {
                ui.label("Tag text");
                ui.text_edit_singleline(&mut self.text);
            }
            3 => {
                ui.label("Level label");
                ui.text_edit_singleline(&mut self.text);
                ui.label("Zero datum height");
                number(ui, &mut self.datum);
            }
            5 => {
                ui.weak("Manual drafting line · independent of BIM geometry");
                ui.horizontal(|ui| {
                    ui.label("Line colour");
                    ui.color_edit_button_srgb(&mut self.stroke.colour);
                });
                ui.horizontal(|ui| {
                    ui.label("Paper width");
                    ui.add(egui::DragValue::new(&mut self.stroke.width_mm).range(0.05..=2.11).speed(0.01).suffix(" mm"));
                });
                egui::ComboBox::from_id_salt("line-pattern").selected_text(format!("{:?}", self.stroke.pattern)).show_ui(ui, |ui| {
                    for (pattern, label) in [
                        (LinePattern::Solid, "Solid"),
                        (LinePattern::Dashed, "Dashed"),
                        (LinePattern::Dotted, "Dotted"),
                        (LinePattern::DashDot, "Dash-dot"),
                    ] {
                        ui.selectable_value(&mut self.stroke.pattern, pattern, label);
                    }
                });
                ui.weak("DXF uses indexed colour approximations and its nearest supported lineweight.");
            }
            _ => {
                ui.horizontal(|ui| {
                    ui.label("Fill colour");
                    ui.color_edit_button_srgb(&mut self.region_fill);
                });
                ui.checkbox(&mut self.region_border, "Draw boundary");
                ui.horizontal_wrapped(|ui| {
                    if ui.button(if self.picking.is_some() { "Finish picking" } else { "Pick vertices" }).clicked() {
                        self.picking = if self.picking.is_some() { None } else { Some(self.region_points.len()) };
                    }
                    if ui.add_enabled(self.region_points.len() < 256, egui::Button::new("Add vertex")).clicked() {
                        let last = self.region_points.last().copied().unwrap_or([-1000.0, 0.0]);
                        self.region_points.push([last[0] + 1000.0, last[1]]);
                    }
                    if ui.add_enabled(!self.region_points.is_empty(), egui::Button::new("Remove last")).clicked() {
                        self.region_points.pop();
                        self.picking = None;
                    }
                });
                for (index, point) in self.region_points.iter_mut().enumerate() {
                    ui.push_id(index, |ui| {
                        ui.label(format!("Vertex {}", index + 1));
                        ui.horizontal(|ui| {
                            number(ui, &mut point[0]);
                            number(ui, &mut point[1]);
                        });
                    });
                }
                ui.weak("Pick at least three corners. Do not repeat the first corner; Trigon closes and validates the boundary.");
            }
        }
        if self.kind != 4 && self.kind != 5 {
            ui.horizontal(|ui| {
                ui.label("Paper text size");
                ui.add(egui::DragValue::new(&mut self.size).range(0.5..=12.0).speed(0.1).suffix(" mm"));
            });
            egui::ComboBox::from_id_salt("annotation-font")
                .selected_text(match self.style.family {
                    FontFamily::Sans => "Sans",
                    FontFamily::Serif => "Serif",
                    FontFamily::Mono => "Monospace",
                })
                .show_ui(ui, |ui| {
                    ui.selectable_value(&mut self.style.family, FontFamily::Sans, "Sans");
                    ui.selectable_value(&mut self.style.family, FontFamily::Serif, "Serif");
                    ui.selectable_value(&mut self.style.family, FontFamily::Mono, "Monospace");
                });
            ui.horizontal(|ui| {
                ui.checkbox(&mut self.style.bold, "Bold");
                ui.checkbox(&mut self.style.italic, "Italic");
            });
        }
        ui.horizontal_wrapped(|ui| {
            if theme::primary(ui, if self.selected.is_some() { "Update annotation" } else { "Add annotation" }).clicked() {
                run = Some(Run {
                    command: "annotation.define",
                    params: json!({"id":self.note_id,"view":id,"size_mm":self.size,"style":self.style,"kind":self.annotation_kind()}),
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
            ui.checkbox(&mut self.alignment, "Temporary alignment guides");
            ui.horizontal(|ui| {
                ui.label("Guide offset");
                ui.add(egui::DragValue::new(&mut self.alignment_offset).range(-100_000.0..=100_000.0).speed(1.0).suffix(" mm"));
            });
            ui.horizontal(|ui| {
                ui.checkbox(&mut self.grid, "Grid");
                ui.add(egui::DragValue::new(&mut self.grid_step).range(0.01..=100000.0).suffix(" mm"));
            });
        });
        let reveal_export = std::mem::take(&mut self.reveal_export);
        let export = egui::CollapsingHeader::new("Export drawing").open(reveal_export.then_some(true)).show(ui, |ui| {
            ui.weak("PDF, SVG, PNG or DXF · file extension chooses format");
            ui.text_edit_singleline(&mut self.export_path);
            if ui.button("Export annotated view").clicked() {
                run = Some(Run { command: "view.export", params: json!({"id":id,"path":self.export_path}) });
            }
        });
        if reveal_export {
            export.header_response.scroll_to_me(Some(egui::Align::TOP));
        }
        run
    }

    fn template_ui(&mut self, ui: &mut egui::Ui, doc: &Document, view: &trigon_model::annotation::DrawingView) -> Option<Run> {
        let key = (view.id.clone(), view.template.clone());
        if self.loaded_template.as_ref() != Some(&key) {
            self.template = view.template.clone().unwrap_or_default();
            self.loaded_template = Some(key);
        }
        let mut run = None;
        egui::CollapsingHeader::new("View template · paper · range").show(ui, |ui| {
            ui.weak(if view.template.is_some() {
                "Saved configuration · Apply commits changes"
            } else {
                "Legacy automatic drawing · Apply enables this template"
            });
            ui.label("Template name");
            ui.text_edit_singleline(&mut self.template.name);
            egui::ComboBox::from_id_salt("line-profile").selected_text(format!("{:?} pens", self.template.line_weights)).show_ui(ui, |ui| {
                for (profile, label) in [
                    (LineWeightProfile::Fine, "Fine · 75%"),
                    (LineWeightProfile::Standard, "Standard · 100%"),
                    (LineWeightProfile::Bold, "Bold · 140%"),
                ] {
                    ui.selectable_value(&mut self.template.line_weights, profile, label);
                }
            });
            ui.weak("Model linework only; manual line paper weights remain explicit.");
            let mut fixed = matches!(self.template.sizing, ViewSizing::Fixed { .. });
            if ui.checkbox(&mut fixed, "Fixed physical scale · crop to paper").changed() {
                let b = doc.buildings.first();
                let centre = if matches!(view.kind, DrawingViewKind::Plan { .. }) {
                    b.map(|b| {
                        let mut lo = [f64::INFINITY; 2];
                        let mut hi = [f64::NEG_INFINITY; 2];
                        for p in &b.footprint {
                            for k in 0..2 {
                                lo[k] = lo[k].min(p[k]);
                                hi[k] = hi[k].max(p[k]);
                            }
                        }
                        [(lo[0] + hi[0]) / 2.0, (lo[1] + hi[1]) / 2.0]
                    })
                    .unwrap_or([0.0; 2])
                } else {
                    [5000.0, 5000.0]
                };
                self.template.sizing = if fixed { ViewSizing::Fixed { denominator: 100.0, centre } } else { ViewSizing::Fit };
            }
            if let ViewSizing::Fixed { denominator, centre } = &mut self.template.sizing {
                ui.horizontal(|ui| {
                    ui.label("1 :");
                    ui.add(egui::DragValue::new(denominator).range(1.0..=10000.0));
                });
                point(ui, "Paper centre · drawing x / y", centre);
                ui.weak("338.67 × 190.50 mm paper at 100% print size. Content outside the viewport is cropped.");
            } else {
                ui.weak("Fit uses an actual fitted paper scale; the footer reports that scale.");
            }
            let mut cropped = self.template.crop.is_some();
            if ui.checkbox(&mut cropped, "Drawing crop rectangle").changed() {
                self.template.crop = if cropped { Some([0.0, 0.0, 10000.0, 8000.0]) } else { None };
            }
            if let Some(crop) = &mut self.template.crop {
                for (label, value) in ["Min x", "Min y", "Max x", "Max y"].into_iter().zip(crop) {
                    ui.horizontal(|ui| {
                        ui.label(label);
                        number(ui, value);
                    });
                }
            }
            if matches!(view.kind, DrawingViewKind::Plan { .. }) {
                egui::ComboBox::from_id_salt("room-labels")
                    .selected_text(match self.template.room_labels {
                        RoomLabels::Hidden => "Room tags · hidden",
                        RoomLabels::Area => "Room tags · areas",
                        RoomLabels::NameAndArea => "Room tags · names and areas",
                    })
                    .show_ui(ui, |ui| {
                        for (mode, label) in
                            [(RoomLabels::Hidden, "Hidden"), (RoomLabels::Area, "Areas"), (RoomLabels::NameAndArea, "Names and areas")]
                        {
                            ui.selectable_value(&mut self.template.room_labels, mode, label);
                        }
                    });
                ui.weak("Live room definitions · 2.5 mm paper text, fitted inside rooms and the visible crop.");
                let mut ranged = self.template.range.is_some();
                if ui.checkbox(&mut ranged, "Explicit floor-plan view range").changed() {
                    self.template.range = if ranged { Some(PlanRange::default()) } else { None };
                }
                if let Some(range) = &mut self.template.range {
                    ui.weak("Offsets from selected storey floor · mm. Depth ≤ bottom < cut < top.");
                    for (label, value) in [
                        ("Top", &mut range.top_mm),
                        ("Cut plane", &mut range.cut_mm),
                        ("Bottom", &mut range.bottom_mm),
                        ("View depth", &mut range.depth_mm),
                    ] {
                        ui.horizontal(|ui| {
                            ui.label(label);
                            ui.add(egui::DragValue::new(value).range(-100000.0..=100000.0).speed(10.0).suffix(" mm"));
                        });
                    }
                    ui.weak("Dashed overhead; dotted below primary range. Uses actual mesh cuts and hidden lines, including adjacent storeys.");
                }
            }
            if matches!(view.kind, DrawingViewKind::Facade { .. }) {
                ui.checkbox(&mut self.template.hide_balcony_dimensions, "Hide balcony size labels");
                ui.weak("Presentation elevation · manual dimensions and notes stay visible.");
            }
            let mut custom_caption = self.template.caption.is_some();
            if ui.checkbox(&mut custom_caption, "Custom caption").changed() {
                self.template.caption = custom_caption.then(String::new);
            }
            if let Some(caption) = &mut self.template.caption {
                ui.add(
                    egui::TextEdit::singleline(caption)
                        .hint_text("Blank hides the caption")
                        .char_limit(trigon_model::annotation::MAX_FRAME_CAPTION_CHARS),
                );
            }
            let mut custom_note = self.template.note.is_some();
            if ui.checkbox(&mut custom_note, "Custom note").changed() {
                self.template.note = custom_note.then(String::new);
            }
            if let Some(note) = &mut self.template.note {
                ui.add(
                    egui::TextEdit::singleline(note)
                        .hint_text("Blank hides the generated note")
                        .char_limit(trigon_model::annotation::MAX_FRAME_NOTE_CHARS),
                );
            }
            ui.weak("Printed frame text only; model names and manual annotations stay unchanged.");
            let error = self.template.validate().err();
            if let Some(error) = &error {
                ui.colored_label(theme::ACCENT, error.to_string());
            }
            if ui.add_enabled(error.is_none(), egui::Button::new("Apply view template")).clicked() {
                run = Some(Run { command: "view.configure", params: json!({"id":view.id,"template":self.template}) });
            }
            if ui.add_enabled(view.template.is_some(), egui::Button::new("Restore legacy drawing defaults")).clicked() {
                run = Some(Run { command: "view.configure", params: json!({"id":view.id,"template":null}) });
            }
            egui::ComboBox::from_id_salt("copy-view-template")
                .selected_text(if self.template_source.is_empty() { "Choose saved template" } else { &self.template_source })
                .show_ui(ui, |ui| {
                    for v in doc.views.iter().filter(|v| v.template.is_some() && v.id != view.id) {
                        ui.selectable_value(
                            &mut self.template_source,
                            v.id.clone(),
                            format!("{} · {}", v.template.as_ref().map_or("", |t| t.name.as_str()), v.name),
                        );
                    }
                });
            if ui.add_enabled(!self.template_source.is_empty(), egui::Button::new("Use saved template")).clicked() {
                run = Some(Run { command: "view.apply_template", params: json!({"source":self.template_source,"targets":[view.id]}) });
            }
        });
        run
    }

    fn annotation_kind(&self) -> AnnotationKind {
        match self.kind {
            5 => AnnotationKind::Line { from: self.points[0], to: self.points[1], stroke: self.stroke },
            1 => AnnotationKind::Dimension { from: self.points[0], to: self.points[1], offset_mm: self.offset },
            2 => AnnotationKind::Leader { tip: self.points[0], elbow: self.points[1], at: self.points[2], text: self.text.clone() },
            3 => AnnotationKind::Level { at: self.points[0], datum_mm: self.datum, label: self.text.clone() },
            4 => AnnotationKind::Region { points: self.region_points.clone(), fill: self.region_fill, border: self.region_border },
            _ => AnnotationKind::Text { at: self.points[0], text: self.text.clone(), rotation_deg: self.rotation },
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn manual_line_selection_and_picking_preserve_paper_style_and_ownership() {
        let mut tool = Drafting {
            kind: 5,
            picking: Some(0),
            stroke: LineStyle { colour: [184, 65, 44], width_mm: 0.7, pattern: LinePattern::DashDot },
            ..Default::default()
        };
        tool.picked([1000.0, 2000.0]);
        assert_eq!(tool.picking, Some(1));
        tool.picked([5000.0, 2000.0]);
        assert!(tool.picking.is_none());
        let line = Annotation { id: "separation".into(), view: "plan".into(), size_mm: 2.5, style: Default::default(), kind: tool.annotation_kind() };
        line.validate().unwrap();
        let doc = Document { annotations: vec![line.clone()], ..Default::default() };
        assert_eq!(Drafting::hit(&doc, "plan", [3000.0, 2003.0], 10.0), Some(&line));
        assert!(Drafting::hit(&doc, "other", [3000.0, 2003.0], 10.0).is_none());
        let mut edit = Drafting::default();
        edit.select(&line);
        assert_eq!(edit.annotation_kind(), line.kind);
        assert_eq!(edit.points(), &[[1000.0, 2000.0], [5000.0, 2000.0]]);
    }

    #[test]
    fn opening_the_same_drawing_preserves_a_custom_new_drawing_id() {
        let mut doc = Document::default();
        doc.views.push(trigon_model::annotation::DrawingView {
            id: "drawing-1".into(),
            name: "Ground floor".into(),
            kind: DrawingViewKind::Plan { level: 0 },
            template: None,
        });
        let mut tool = Drafting::default();
        tool.opened(&doc, "drawing-1");
        assert_eq!(tool.view_id, "drawing-2");
        tool.view_id = "measured-section".into();
        for _ in 0..3 {
            tool.opened(&doc, "drawing-1");
            assert_eq!(tool.view_id, "measured-section");
        }
        doc.views.push(trigon_model::annotation::DrawingView {
            id: "measured-section".into(),
            name: "Measured section".into(),
            kind: DrawingViewKind::Plan { level: 0 },
            template: None,
        });
        tool.opened(&doc, "drawing-1");
        assert_eq!(tool.view_id, "drawing-2", "a newly taken ID must be replaced");
        tool.view_id = "custom-elevation".into();
        tool.opened(&doc, "measured-section");
        assert_eq!(tool.view_id, "drawing-2", "switching views initializes a fresh drawing form");
    }

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
    fn region_picking_and_font_controls_round_trip_without_masking_text_selection() {
        let mut tool = Drafting { kind: 4, picking: Some(0), ..Default::default() };
        for p in [[0.0, 0.0], [3000.0, 0.0], [3000.0, 2000.0], [0.0, 2000.0]] {
            tool.picked(p);
        }
        assert_eq!(tool.points().len(), 4);
        assert_eq!(tool.picking, Some(4));
        let region = Annotation { id: "fill".into(), view: "plan".into(), size_mm: 2.5, style: TextStyle::default(), kind: tool.annotation_kind() };
        region.validate().unwrap();
        let note = Annotation {
            id: "note".into(),
            view: "plan".into(),
            size_mm: 3.5,
            style: TextStyle { family: FontFamily::Serif, bold: true, italic: true },
            kind: AnnotationKind::Text { at: [1000.0, 1000.0], text: "Room".into(), rotation_deg: 0.0 },
        };
        let doc = Document { annotations: vec![region.clone(), note.clone()], ..Default::default() };
        assert_eq!(Drafting::hit(&doc, "plan", [1001.0, 1000.0], 10.0), Some(&note));
        assert_eq!(Drafting::hit(&doc, "plan", [2000.0, 1500.0], 10.0), Some(&region));
        assert!(Drafting::hit(&doc, "plan", [4000.0, 1500.0], 10.0).is_none());
        tool.select(&note);
        assert_eq!(tool.style, note.style);
        assert_eq!(tool.annotation_kind(), note.kind);
        tool.select(&region);
        assert_eq!(tool.annotation_kind(), region.kind);
        assert_eq!(tool.points().len(), 4);
    }

    #[test]
    fn annotation_picking_follows_dimension_offsets_and_view_ownership() {
        let mut doc = Document::default();
        let annotation = Annotation {
            id: "d".into(),
            view: "elevation".into(),
            size_mm: 2.5,
            style: TextStyle::default(),
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
