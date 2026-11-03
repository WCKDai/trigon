//! Drawing authoring produces the same engine commands used by CLI/MCP. Canvas gestures
//! collect a local draft; Finish/Enter explicitly submits one undoable annotation command.

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

#[derive(Default)]
struct Placement {
    picked: usize,
    pending: bool,
    focus_text: bool,
    frame: Option<(DrawingViewKind, Option<[[f64; 2]; 2]>)>,
    stale: bool,
    error: Option<String>,
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
    guide_mode: bool,
    placement: Option<Placement>,
    repeat_placement: bool,
    chain_placement: bool,
    text_editor: Option<egui::Id>,
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
            guide_mode: false,
            placement: None,
            repeat_placement: true,
            chain_placement: false,
            text_editor: None,
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
                    egui::Button::new(egui::RichText::new("Create drawing").color(theme::CHROME)).fill(theme::ACCENT).stroke(egui::Stroke::NONE),
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
        self.sync_context(doc);
    }

    fn frame(doc: &Document, id: &str) -> Option<(DrawingViewKind, Option<[[f64; 2]; 2]>)> {
        let mut views = doc.views.iter().filter(|view| view.id == id);
        let view = views.next()?;
        if views.next().is_some() {
            return None;
        }
        let mut kind = view.kind.clone();
        if let DrawingViewKind::Section { depth_mm, .. } = &mut kind {
            *depth_mm = 0.0;
        }
        let edge = if let DrawingViewKind::Facade { facade } = &kind {
            let mut sources =
                doc.buildings.iter().flat_map(|building| building.facades.iter().filter(|f| &f.id == facade).map(move |f| (building, f)));
            let (building, facade) = sources.next()?;
            if sources.next().is_some() {
                return None;
            }
            let (a, b) = building.edge(facade.edge)?;
            Some([a, b])
        } else {
            None
        };
        Some((kind, edge))
    }

    pub(crate) fn sync_context(&mut self, doc: &Document) {
        let frame = Self::frame(doc, &self.current_view);
        if let Some(placement) = &mut self.placement
            && (placement.frame != frame || frame.is_none())
        {
            placement.stale = true;
            placement.pending = false;
            placement.error = Some("The drawing coordinate frame changed. Restart the annotation tool before placing this draft.".into());
            self.picking = None;
        }
    }

    pub fn new_note(&mut self, doc: &Document) {
        self.text_editor = None;
        self.guide_mode = false;
        self.selected = None;
        self.note_id = unique_id("annotation", |id| doc.annotations.iter().any(|a| a.id == id));
        self.picking = None;
        self.region_points.clear();
        self.placement = None;
    }

    /// Start a saved plan datum with exact site coordinates; the shared preview
    /// reviews its definition before it becomes an alignment reference.
    pub fn reference_guide(&mut self, doc: &Document, view: &str, line: Option<[[f64; 2]; 2]>) -> bool {
        if !doc.views.iter().any(|v| v.id == view && matches!(v.kind, DrawingViewKind::Plan { .. })) {
            return false;
        }
        self.opened(doc, view);
        self.new_note(doc);
        self.note_id = unique_id("reference-guide", |id| doc.annotations.iter().any(|a| a.id == id));
        self.kind = 5;
        self.stroke = LineStyle { colour: [197, 63, 45], width_mm: 0.18, pattern: LinePattern::Dotted };
        self.guide_mode = true;
        if let Some(line) = line {
            self.points[0] = line[0];
            self.points[1] = line[1];
            self.picking = None;
        } else {
            self.picking = Some(0);
        }
        true
    }

    pub fn guide_active(&self) -> bool {
        self.guide_mode
    }

    pub fn select(&mut self, a: &Annotation) {
        self.text_editor = None;
        self.placement = None;
        self.guide_mode = a.id.starts_with("reference-guide-") && matches!(a.kind, AnnotationKind::Line { .. });
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
        if ui.selectable_label(self.placement.is_none() && self.picking.is_none(), "Select").clicked() {
            self.cancel_placement();
        }
        let view = doc.views.iter().find(|v| v.id == id);
        for (i, label) in ["Text", "Dimension", "Leader", "Level", "Area fill", "Line"].into_iter().enumerate() {
            let enabled = view.is_some_and(|v| i != 3 || !matches!(v.kind, DrawingViewKind::Plan { .. }));
            if ui.add_enabled(enabled, egui::Button::selectable(self.placement.is_some() && self.kind == i, label)).clicked() {
                self.start_placement(doc, id, i);
            }
        }
    }

    /// Tool indices match the ribbon: text, aligned dimension, leader, level, fill, line.
    /// Starting or cancelling a gesture never changes the document.
    pub fn start_placement(&mut self, doc: &Document, id: &str, kind: usize) -> bool {
        if kind > 5 || !doc.views.iter().any(|view| view.id == id && (kind != 3 || !matches!(view.kind, DrawingViewKind::Plan { .. }))) {
            return false;
        }
        self.opened(doc, id);
        self.new_note(doc);
        self.kind = kind;
        self.picking = Some(0);
        self.placement = Some(Placement { frame: Self::frame(doc, id), ..Default::default() });
        true
    }

    pub fn placement_active(&self) -> bool {
        self.placement.is_some()
    }

    pub fn placement_status(&self) -> &'static str {
        let Some(placement) = &self.placement else { return "Choose an annotation tool, then click the drawing." };
        if placement.pending {
            return "Placing annotation…";
        }
        match (self.kind, self.picking) {
            (0, Some(_)) => "Click the text position, then type your note.",
            (1, Some(0)) => "Aligned dimension · click the first witness point.",
            (1, Some(1)) => "Click the second witness point.",
            (1, Some(_)) => "Click to position the dimension line and its witness offset.",
            (2, Some(0)) => "Click the arrow tip on the drawing.",
            (2, Some(1)) => "Click the leader bend.",
            (2, Some(_)) => "Click the text position, then type your note.",
            (3, Some(_)) => "Click the level mark position.",
            (4, Some(_)) if self.region_points.len() < 3 => "Area fill · click at least three corners.",
            (4, Some(_)) => "Add corners, or press Enter / Finish to close the fill.",
            (5, Some(0)) => "Manual line · click the start point.",
            (5, Some(_)) => "Click the end point.",
            (0 | 2 | 3, None) => "Edit the text, then press Enter or Finish.",
            _ => "Press Enter or Finish to place. Esc cancels this draft.",
        }
    }

    fn placement_geometry_complete(&self) -> bool {
        self.placement.as_ref().is_some_and(|placement| {
            placement.picked
                >= match self.kind {
                    1 | 2 | 4 => 3,
                    5 => 2,
                    _ => 1,
                }
        })
    }

    fn draft_annotation(&self) -> Annotation {
        Annotation { id: self.note_id.clone(), view: self.current_view.clone(), size_mm: self.size, style: self.style, kind: self.annotation_kind() }
    }

    pub fn placement_ready(&self) -> bool {
        self.placement.as_ref().is_some_and(|placement| !placement.pending && !placement.stale)
            && self.placement_geometry_complete()
            && self.draft_annotation().validate().is_ok()
    }

    /// Read-only feedback for the same gesture humans see on the canvas. Unpicked form
    /// defaults are deliberately omitted; dimension picks include its offset-position click.
    pub fn placement_feedback(&self) -> serde_json::Value {
        let active = self.placement.is_some();
        let pending = self.placement.as_ref().is_some_and(|placement| placement.pending);
        let picked = if !active {
            &[][..]
        } else if self.kind == 4 {
            self.region_points.as_slice()
        } else {
            &self.points[..self.placement.as_ref().map_or(0, |placement| placement.picked.min(3))]
        };
        let command_error = self.placement.as_ref().and_then(|placement| placement.error.clone());
        let validation_error =
            self.placement_geometry_complete().then(|| self.draft_annotation().validate().err().map(|error| error.to_string())).flatten();
        let error = validation_error.or_else(|| command_error.clone());
        json!({
            "active":active,
            "tool":active.then_some(["text","aligned_dimension","leader","level","area_fill","line"][self.kind.min(5)]),
            "kind":active.then_some(["text","dimension","leader","level","region","line"][self.kind.min(5)]),
            "annotation_id":active.then_some(self.note_id.as_str()),
            "view":active.then_some(self.current_view.as_str()),
            "selected":self.selected,
            "picked_points":picked,
            "next_step":self.placement_status(),
            "ready":self.placement_ready(),
            "pending":pending,
            "stale":self.placement.as_ref().is_some_and(|placement| placement.stale),
            "error":error,
            "command_error":command_error,
            "units":{"points":"drawing_plane_mm","witness_offset":"model_mm","text_size":"paper_mm","line_width":"paper_mm"},
            "options":active.then(|| json!({
                "text":self.text,"text_size_mm":self.size,"text_style":self.style,
                "witness_offset_mm":self.offset,"repeat":self.repeat_placement,"chain":self.chain_placement,
                "graphic_only":matches!(self.kind,4|5)
            }))
        })
    }

    pub fn cancel_placement(&mut self) {
        if self.placement.take().is_some() {
            self.region_points.clear();
        }
        self.picking = None;
    }

    /// Only an explicit Finish/Enter should call this. A command remains pending until the
    /// caller reports success/failure, so repaint and repeated input cannot submit it twice.
    pub fn finish_placement(&mut self, doc: &Document, id: &str) -> Option<Run> {
        self.sync_context(doc);
        if self.current_view != id
            || !self.placement_ready()
            || !doc.views.iter().any(|view| view.id == id && (self.kind != 3 || !matches!(view.kind, DrawingViewKind::Plan { .. })))
        {
            return None;
        }
        // Another command may have claimed the provisional id since this gesture began.
        if doc.annotations.iter().any(|annotation| annotation.id == self.note_id) {
            self.note_id = unique_id("annotation", |id| doc.annotations.iter().any(|annotation| annotation.id == id));
            if doc.annotations.iter().any(|annotation| annotation.id == self.note_id) {
                return None;
            }
        }
        let annotation = self.draft_annotation();
        if let Some(placement) = &mut self.placement {
            placement.pending = true;
            placement.error = None;
        }
        Some(Run {
            command: "annotation.define",
            params: json!({"id":annotation.id,"view":annotation.view,"size_mm":annotation.size_mm,"style":annotation.style,"kind":annotation.kind}),
        })
    }

    /// Call for a successfully returned annotation instead of `select`. Repeat placement
    /// starts only after success; errors preserve the original draft for correction/retry.
    pub fn annotation_applied(&mut self, doc: &Document, annotation: &Annotation) {
        let placing = self.placement.as_ref().is_some_and(|placement| placement.pending)
            && annotation.id == self.note_id
            && annotation.view == self.current_view;
        if placing && self.repeat_placement {
            let kind = self.kind;
            let next_start = self.points[1];
            if self.start_placement(doc, &annotation.view, kind) {
                if self.chain_placement && matches!(kind, 1 | 5) {
                    self.points[0] = next_start;
                    self.picking = Some(1);
                    if let Some(placement) = &mut self.placement {
                        placement.picked = 1;
                    }
                }
                return;
            }
        }
        self.select(annotation);
    }

    pub fn placement_failed(&mut self, error: &str) {
        if let Some(placement) = &mut self.placement {
            placement.pending = false;
            placement.error = Some(error.into());
        }
    }

    /// A small native options editor intended for a bounded floating canvas panel.
    /// Enter is handled after the text field has processed this frame's typing.
    /// Capture this before other widgets process input: a foreign single-line editor
    /// may surrender focus on Enter without consuming that event for later widgets.
    pub(crate) fn keyboard_owned(&self, ctx: &egui::Context) -> bool {
        ctx.memory(|memory| memory.focused()).is_none_or(|focused| Some(focused) == self.text_editor)
    }

    pub fn canvas_editor(&mut self, ui: &mut egui::Ui, doc: &Document, id: &str, keyboard_owned: bool) -> Option<Run> {
        self.placement.as_ref()?;
        ui.set_max_width(320.0);
        ui.strong(["Text", "Aligned dimension", "Leader", "Level", "Area fill", "Manual line"][self.kind.min(5)]);
        ui.label(self.placement_status());
        let pending = self.placement.as_ref().is_some_and(|placement| placement.pending);
        let mut finish = false;
        let mut cancel = false;
        ui.add_enabled_ui(!pending, |ui| {
            if matches!(self.kind, 0 | 2 | 3) {
                let text = ui.add(egui::TextEdit::singleline(&mut self.text).hint_text("Type a note").desired_width(280.0));
                self.text_editor = Some(text.id);
                if self.placement.as_ref().is_some_and(|placement| placement.focus_text) {
                    text.request_focus();
                    let mut state = egui::TextEdit::load_state(ui.ctx(), text.id).unwrap_or_default();
                    state.cursor.set_char_range(Some(egui::text::CCursorRange::two(
                        egui::text::CCursor::default(),
                        egui::text::CCursor::new(self.text.chars().count()),
                    )));
                    state.store(ui.ctx(), text.id);
                    if let Some(placement) = &mut self.placement {
                        placement.focus_text = false;
                    }
                }
            }
            match self.kind {
                1 => {
                    ui.horizontal(|ui| {
                        ui.label("Witness offset");
                        number(ui, &mut self.offset);
                    });
                    ui.weak("Aligned to the two witnesses · offset in model mm");
                }
                3 => {
                    ui.horizontal(|ui| {
                        ui.label("Zero datum");
                        number(ui, &mut self.datum);
                    });
                }
                4 => {
                    ui.horizontal(|ui| {
                        ui.label("Fill");
                        ui.color_edit_button_srgb(&mut self.region_fill);
                        ui.checkbox(&mut self.region_border, "Border");
                    });
                    ui.weak("Graphic fill only. Define rooms and apartments with model tools.");
                }
                5 => {
                    ui.horizontal(|ui| {
                        ui.label("Pen");
                        ui.color_edit_button_srgb(&mut self.stroke.colour);
                        ui.add(egui::DragValue::new(&mut self.stroke.width_mm).range(0.05..=2.11).speed(0.01).suffix(" mm paper"));
                    });
                    ui.weak("Graphic line only. Room and apartment boundaries use model tools.");
                }
                _ => {}
            }
            if !matches!(self.kind, 4 | 5) {
                ui.horizontal(|ui| {
                    ui.label("Paper text");
                    ui.add(egui::DragValue::new(&mut self.size).range(0.5..=12.0).speed(0.1).suffix(" mm"));
                    egui::ComboBox::from_id_salt("canvas-annotation-font").selected_text(self.style.family.name()).show_ui(ui, |ui| {
                        for family in [FontFamily::Sans, FontFamily::Serif, FontFamily::Mono] {
                            ui.selectable_value(&mut self.style.family, family, family.name());
                        }
                    });
                });
                ui.horizontal(|ui| {
                    ui.checkbox(&mut self.style.bold, "Bold");
                    ui.checkbox(&mut self.style.italic, "Italic");
                });
            }
            ui.horizontal(|ui| {
                ui.checkbox(&mut self.repeat_placement, "Keep placing");
                if matches!(self.kind, 1 | 5) {
                    ui.add_enabled(self.repeat_placement, egui::Checkbox::new(&mut self.chain_placement, "Chain"));
                }
            });
            if self.placement_geometry_complete()
                && let Err(error) = self.draft_annotation().validate()
            {
                ui.colored_label(theme::ACCENT, error.to_string());
            }
            ui.horizontal(|ui| {
                finish = ui.add_enabled(self.placement_ready(), egui::Button::new("Finish · Enter")).clicked();
                cancel = ui.button("Cancel · Esc").clicked();
            });
            if keyboard_owned {
                finish |= ui.input_mut(|input| input.consume_key(egui::Modifiers::NONE, egui::Key::Enter));
                cancel |= ui.input_mut(|input| input.consume_key(egui::Modifiers::NONE, egui::Key::Escape));
            }
        });
        if let Some(error) = self.placement.as_ref().and_then(|placement| placement.error.as_deref()) {
            ui.colored_label(theme::ACCENT, error);
        }
        if cancel {
            self.cancel_placement();
            None
        } else if finish {
            self.finish_placement(doc, id)
        } else {
            None
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
        let picked = if self.kind == 4 {
            self.region_points.as_slice()
        } else {
            let count = self.placement.as_ref().map_or(self.picking.unwrap_or(0), |placement| placement.picked);
            &self.points[..count.min(if self.kind == 1 { 2 } else { 3 })]
        };
        anchors.extend(picked.iter().copied().take(512 - anchors.len()));
        trigon_engine::AlignmentOptions { anchors, offset_mm: self.alignment_offset }
    }

    pub fn picked(&mut self, p: [f64; 2]) {
        if p.iter().any(|value| !value.is_finite() || value.abs() > 1_000_000.0)
            || self.placement.as_ref().is_some_and(|placement| placement.pending || placement.stale)
        {
            return;
        }
        if self.kind == 4 && self.picking.is_some() {
            if self.placement.is_some()
                && self.region_points.len() >= 3
                && self.region_points.first().is_some_and(|first| (first[0] - p[0]).hypot(first[1] - p[1]) <= 0.001)
            {
                self.picking = None;
                return;
            }
            if self.region_points.len() < 256 && self.region_points.last().is_none_or(|last| (last[0] - p[0]).hypot(last[1] - p[1]) > 0.001) {
                self.region_points.push(p);
            }
            self.picking = (self.region_points.len() < 256).then_some(self.region_points.len());
            if let Some(placement) = &mut self.placement {
                placement.picked = self.region_points.len();
            }
            return;
        }
        if let Some(i) = self.picking.filter(|i| *i < 3) {
            let direct = self.placement.is_some();
            if direct && i == 1 && matches!(self.kind, 1 | 2 | 5) {
                let length = (p[0] - self.points[0][0]).hypot(p[1] - self.points[0][1]);
                if !(0.001..=1_000_000.0).contains(&length) {
                    return;
                }
            }
            if direct && self.kind == 1 && i == 2 {
                let Some(offset) = self.dimension_offset(p).filter(|offset| offset.abs() <= 100_000.0) else { return };
                self.offset = offset;
            }
            self.points[i] = p;
            self.picking = match (self.kind, i) {
                (1, 0) | (2, 0) | (5, 0) => Some(1),
                (1, 1) if direct => Some(2),
                (2, 1) => Some(2),
                _ => None,
            };
            if let Some(placement) = &mut self.placement {
                placement.picked = placement.picked.max(i + 1);
                placement.focus_text = self.picking.is_none() && matches!(self.kind, 0 | 2 | 3);
            }
        }
    }

    fn dimension_offset(&self, point: [f64; 2]) -> Option<f64> {
        let [a, b] = [self.points[0], self.points[1]];
        let length = (b[0] - a[0]).hypot(b[1] - a[1]);
        if !length.is_finite() || length < 0.001 {
            return None;
        }
        let offset = ((b[0] - a[0]) * (point[1] - a[1]) - (b[1] - a[1]) * (point[0] - a[0])) / length;
        offset.is_finite().then_some(offset)
    }

    /// The live rubber band contains only points actually picked, plus a valid cursor.
    /// For a dimension's third click it follows the aligned dimension line, not the cursor's
    /// arbitrary distance along that line. This is preview data only and emits no command.
    pub fn preview_points(&self, cursor: Option<[f64; 2]>) -> Vec<[f64; 2]> {
        let Some(placement) = &self.placement else { return self.points().to_vec() };
        let cursor = cursor.filter(|point| point.iter().all(|value| value.is_finite() && value.abs() <= 1_000_000.0));
        if self.kind == 1 && placement.picked >= 2 {
            let [a, b] = [self.points[0], self.points[1]];
            let length = (b[0] - a[0]).hypot(b[1] - a[1]);
            if length < 0.001 {
                return Vec::new();
            }
            let offset =
                if self.picking == Some(2) { cursor.and_then(|point| self.dimension_offset(point)).unwrap_or(self.offset) } else { self.offset };
            let shift = [-(b[1] - a[1]) / length * offset, (b[0] - a[0]) / length * offset];
            return vec![[a[0] + shift[0], a[1] + shift[1]], [b[0] + shift[0], b[1] + shift[1]]];
        }
        let mut points = self.points().to_vec();
        if self.picking.is_some()
            && !placement.pending
            && let Some(cursor) = cursor
        {
            points.push(cursor);
        }
        points
    }

    /// A dimension previews its actual offset line and both witnesses. The same
    /// signed offset calculation is used by the final click and command.
    pub fn preview_segments(&self, cursor: Option<[f64; 2]>) -> Vec<[[f64; 2]; 2]> {
        let mut points = self.preview_points(cursor);
        let source = self.points();
        if self.kind == 1 && source.len() == 2 {
            if self.placement.is_none() {
                let [a, b] = [source[0], source[1]];
                let length = (b[0] - a[0]).hypot(b[1] - a[1]);
                if length < 0.001 {
                    return Vec::new();
                }
                let shift = [-(b[1] - a[1]) / length * self.offset, (b[0] - a[0]) / length * self.offset];
                points = vec![[a[0] + shift[0], a[1] + shift[1]], [b[0] + shift[0], b[1] + shift[1]]];
            }
            if points.len() == 2 {
                return vec![[source[0], points[0]], [points[0], points[1]], [points[1], source[1]]];
            }
        }
        let mut segments: Vec<_> = points.windows(2).map(|pair| [pair[0], pair[1]]).collect();
        if self.is_region()
            && points.len() >= 3
            && let (Some(first), Some(last)) = (points.first(), points.last())
        {
            segments.push([*last, *first]);
        }
        segments
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
        let count = match self.kind {
            1 | 5 => 2,
            2 => 3,
            _ => 1,
        };
        &self.points[..self.placement.as_ref().map_or(count, |placement| placement.picked.min(count))]
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
                    if self.selected.is_some() {
                        self.kind = i;
                        self.picking = None;
                    } else {
                        self.start_placement(doc, id, i);
                    }
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
                ui.weak("Graphic fill only. Room and apartment definitions belong to model tools.");
                if let Some(placement) = &mut self.placement {
                    placement.picked = self.region_points.len();
                }
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
            let label = if self.guide_mode {
                "Preview guide"
            } else if self.placement_active() {
                "Finish annotation · Enter"
            } else if self.selected.is_some() {
                "Update annotation"
            } else {
                "Add annotation"
            };
            if ui.add_enabled_ui(!self.placement_active() || self.placement_ready(), |ui| theme::primary(ui, label)).inner.clicked() {
                run = if self.placement_active() {
                    self.finish_placement(doc, id)
                } else {
                    Some(Run {
                        command: "annotation.define",
                        params: json!({"id":self.note_id,"view":id,"size_mm":self.size,"style":self.style,"kind":self.annotation_kind()}),
                    })
                };
            }
            if let Some(selected) = &self.selected
                && ui.button("Delete annotation").clicked()
            {
                run = Some(Run { command: "annotation.remove", params: json!({"id":selected}) });
            }
        });
        if self.picking.is_some() {
            ui.colored_label(
                theme::ACCENT,
                if self.placement_active() { self.placement_status() } else { "Click the drawing to fill this point. Esc cancels." },
            );
            ui.weak("Middle-drag pans · Alt bypasses snaps");
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
            let mut round_dimensions = self.template.dimension_rounding_mm.is_some();
            if ui.checkbox(&mut round_dimensions, "Round printed dimension annotations").changed() {
                self.template.dimension_rounding_mm = round_dimensions.then_some(100);
            }
            if let Some(quantum) = &mut self.template.dimension_rounding_mm {
                ui.horizontal(|ui| {
                    ui.label("Nearest");
                    ui.add(egui::DragValue::new(quantum).range(1..=1000).suffix(" mm"));
                });
                ui.weak("Prints ≈ before rounded labels. Witness points and model dimensions remain exact.");
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

    fn placement_document() -> Document {
        let mut doc = Document::default();
        doc.buildings.push(
            serde_json::from_value(json!({
                "id":"block","name":"Drawing fixture",
                "footprint":[[0,0],[10000,0],[10000,8000],[0,8000]],
                "wall_thickness":300,
                "storeys":[{"name":"Ground floor","elevation":0,"height":3000}],
                "facades":[],"evidence":[]
            }))
            .unwrap(),
        );
        doc.views.push(serde_json::from_value(json!({"id":"plan","name":"Ground floor","kind":{"view":"plan","level":0}})).unwrap());
        doc
    }

    #[test]
    fn same_id_section_frame_changes_refuse_a_retained_annotation_draft() {
        let mut doc = placement_document();
        doc.views[0].kind = DrawingViewKind::Section { from: [0.0, 0.0], to: [10000.0, 0.0], flip: false, depth_mm: 30000.0 };
        let mut tool = Drafting::default();
        tool.start_placement(&doc, "plan", 0);
        tool.picked([2000.0, 2500.0]);
        if let DrawingViewKind::Section { flip, .. } = &mut doc.views[0].kind {
            *flip = true;
        }
        tool.opened(&doc, "plan");
        assert!(tool.finish_placement(&doc, "plan").is_none(), "old coordinates must not enter the reversed frame");
        assert!(tool.placement_feedback()["error"].as_str().is_some());
        assert!(doc.annotations.is_empty());
    }

    #[test]
    fn section_depth_and_paper_style_changes_keep_the_same_annotation_frame() {
        let mut doc = placement_document();
        doc.views[0].kind = DrawingViewKind::Section { from: [0.0, 0.0], to: [10000.0, 0.0], flip: false, depth_mm: 30000.0 };
        let mut tool = Drafting::default();
        tool.start_placement(&doc, "plan", 0);
        tool.picked([2000.0, 2500.0]);
        if let DrawingViewKind::Section { depth_mm, .. } = &mut doc.views[0].kind {
            *depth_mm = 40000.0;
        }
        doc.views[0].template = Some(DrawingTemplate::default());
        tool.opened(&doc, "plan");
        assert!(tool.finish_placement(&doc, "plan").is_some());
    }

    #[test]
    fn live_dimension_segments_match_the_signed_offset_in_the_committed_command() {
        let doc = placement_document();
        let mut tool = Drafting::default();
        tool.start_placement(&doc, "plan", 1);
        tool.picked([0.0, 0.0]);
        tool.picked([4000.0, 0.0]);
        for point in [[3000.0, -1000.0], [1200.0, 800.0]] {
            let segments = tool.preview_segments(Some(point));
            assert_eq!(segments[1], [[0.0, point[1]], [4000.0, point[1]]]);
            assert_eq!(segments[0], [[0.0, 0.0], [0.0, point[1]]]);
            assert_eq!(segments[2], [[4000.0, point[1]], [4000.0, 0.0]]);
        }
        tool.picked([3000.0, -1000.0]);
        let preview = tool.preview_segments(None);
        let run = tool.finish_placement(&doc, "plan").unwrap();
        assert_eq!(run.params["kind"]["offset_mm"], -1000.0);
        assert_eq!(preview[1], [[0.0, -1000.0], [4000.0, -1000.0]]);
    }

    #[test]
    fn rejected_annotation_reports_a_command_failure_with_the_retained_draft() {
        let doc = placement_document();
        let mut tool = Drafting::default();
        tool.start_placement(&doc, "plan", 0);
        tool.picked([2000.0, 2500.0]);
        tool.finish_placement(&doc, "plan").unwrap();
        tool.placement_failed("public annotation command rejection");
        assert!(tool.placement_feedback()["error"].as_str().is_some(), "engine rejection must survive in AI feedback");
        assert_eq!(tool.placement_feedback()["command_error"], "public annotation command rejection");
        assert_eq!(tool.points(), &[[2000.0, 2500.0]]);
    }

    #[test]
    fn direct_aligned_dimension_requires_two_witnesses_offset_and_explicit_finish() {
        let doc = placement_document();
        let before = doc.clone();
        let mut tool = Drafting::default();
        assert!(tool.start_placement(&doc, "plan", 1));
        assert!(tool.points().is_empty(), "untouched form defaults are not preview geometry");
        assert!(tool.finish_placement(&doc, "plan").is_none());
        tool.picked([0.0, 0.0]);
        assert_eq!(tool.picking, Some(1));
        assert_eq!(tool.preview_points(Some([3000.0, 4000.0])), vec![[0.0, 0.0], [3000.0, 4000.0]]);
        tool.picked([3000.0, 4000.0]);
        assert_eq!(tool.picking, Some(2));
        assert!(!tool.placement_ready());
        assert!(tool.finish_placement(&doc, "plan").is_none());
        assert_eq!(tool.preview_points(Some([1400.0, 1450.0])), vec![[200.0, -150.0], [3200.0, 3850.0]]);
        tool.picked([1400.0, 1450.0]);
        assert!(tool.placement_ready());
        assert_eq!(tool.points(), &[[0.0, 0.0], [3000.0, 4000.0]]);
        assert_eq!(tool.annotation_kind(), AnnotationKind::Dimension { from: [0.0, 0.0], to: [3000.0, 4000.0], offset_mm: -250.0 });
        let run = tool.finish_placement(&doc, "plan").unwrap();
        assert_eq!(run.command, "annotation.define");
        assert_eq!(run.params["kind"]["offset_mm"], -250.0);
        assert!(tool.finish_placement(&doc, "plan").is_none(), "one gesture may submit only once while pending");
        assert_eq!(doc, before, "picking, preview and producing a command do not mutate the document");
    }

    #[test]
    fn direct_text_preserves_paper_style_and_failed_drafts_until_retry() {
        let doc = placement_document();
        let mut tool = Drafting::default();
        assert!(tool.start_placement(&doc, "plan", 0));
        tool.size = 3.5;
        tool.style = TextStyle { family: FontFamily::Serif, bold: true, italic: true };
        tool.text = "North entrance".into();
        tool.picked([1234.56789, -456.12345]);
        assert!(tool.placement_ready());
        assert!(tool.placement.as_ref().unwrap().focus_text);
        let first = tool.finish_placement(&doc, "plan").unwrap();
        assert_eq!(first.params["size_mm"], 3.5);
        assert_eq!(first.params["kind"]["at"], json!([1234.56789, -456.12345]));
        assert_eq!(first.params["style"], json!({"family":"serif","bold":true,"italic":true}));
        assert!(!tool.placement_ready());
        tool.picked([0.0, 0.0]);
        assert_eq!(tool.points(), &[[1234.56789, -456.12345]], "pending geometry cannot be overwritten");
        tool.placement_failed("public annotation command rejection");
        assert!(tool.placement_ready());
        let retry = tool.finish_placement(&doc, "plan").unwrap();
        assert_eq!(retry.params, first.params);
    }

    #[test]
    fn cancel_discards_only_the_unfinished_gesture_at_every_step() {
        let doc = placement_document();
        let before = doc.clone();
        for kind in [0, 1, 2, 4, 5] {
            for count in 0..=3 {
                let mut tool = Drafting::default();
                assert!(tool.start_placement(&doc, "plan", kind));
                for point in [[0.0, 0.0], [2000.0, 0.0], [2000.0, 1000.0]].into_iter().take(count) {
                    tool.picked(point);
                }
                tool.cancel_placement();
                assert!(!tool.placement_active());
                assert!(tool.picking.is_none());
                assert!(tool.finish_placement(&doc, "plan").is_none());
            }
        }
        assert_eq!(doc, before);
        let note = Annotation {
            id: "saved-note".into(),
            view: "plan".into(),
            size_mm: 2.5,
            style: TextStyle::default(),
            kind: AnnotationKind::Text { at: [100.0, 200.0], text: "Saved".into(), rotation_deg: 0.0 },
        };
        let mut tool = Drafting::default();
        tool.select(&note);
        tool.picking = Some(0);
        tool.cancel_placement();
        assert_eq!(tool.selected.as_deref(), Some("saved-note"));
        assert_eq!(tool.annotation_kind(), note.kind, "cancelling a selected annotation's pick preserves its edit values");
    }

    #[test]
    fn direct_region_closes_without_repeating_a_corner_and_rejects_crossing_fills() {
        let doc = placement_document();
        let mut tool = Drafting::default();
        assert!(tool.start_placement(&doc, "plan", 4));
        for point in [[0.0, 0.0], [3000.0, 0.0], [3000.0, 2000.0]] {
            tool.picked(point);
        }
        assert!(tool.placement_ready());
        tool.picked([0.0, 0.0]);
        assert!(tool.picking.is_none());
        assert_eq!(tool.points().len(), 3);
        assert!(tool.finish_placement(&doc, "plan").is_some());
        tool.cancel_placement();
        assert!(tool.start_placement(&doc, "plan", 4));
        for point in [[0.0, 0.0], [3000.0, 2000.0], [0.0, 2000.0], [3000.0, 0.0]] {
            tool.picked(point);
        }
        assert!(!tool.placement_ready());
        assert!(tool.finish_placement(&doc, "plan").is_none());
        assert_eq!(tool.points().len(), 4, "invalid geometry stays available for correction");
    }

    #[test]
    fn repeat_and_chain_restart_only_after_success_with_a_new_annotation_id() {
        let mut doc = placement_document();
        let mut tool = Drafting { chain_placement: true, ..Default::default() };
        assert!(tool.start_placement(&doc, "plan", 5));
        tool.picked([1000.0, 2000.0]);
        tool.picked([5000.0, 2000.0]);
        let run = tool.finish_placement(&doc, "plan").unwrap();
        let first: Annotation = serde_json::from_value(run.params).unwrap();
        assert!(!tool.placement_ready());
        doc.annotations.push(first.clone());
        tool.annotation_applied(&doc, &first);
        assert!(tool.placement_active());
        assert_eq!(tool.picking, Some(1));
        assert_eq!(tool.points(), &[[5000.0, 2000.0]]);
        assert!(tool.selected.is_none());
        assert_ne!(tool.note_id, first.id);
        assert!(tool.finish_placement(&doc, "plan").is_none());
        tool.picked([5000.0, 5000.0]);
        let next = tool.finish_placement(&doc, "plan").unwrap();
        assert_ne!(next.params["id"], first.id);
        assert_eq!(next.params["kind"]["from"], json!([5000.0, 2000.0]));
        assert_eq!(next.params["kind"]["to"], json!([5000.0, 5000.0]));
        tool.repeat_placement = false;
        let second: Annotation = serde_json::from_value(next.params).unwrap();
        doc.annotations.push(second.clone());
        tool.annotation_applied(&doc, &second);
        assert!(!tool.placement_active());
        assert_eq!(tool.selected.as_deref(), Some(second.id.as_str()));
    }

    #[test]
    fn completed_direct_placement_is_one_engine_command_and_one_exact_undo() {
        let mut session = trigon_engine::Session::new(Box::new(trigon_data::Live::new(trigon_data::Credentials::default())));
        session.doc = placement_document();
        let before = session.doc.clone();
        let mut tool = Drafting { repeat_placement: false, ..Default::default() };
        assert!(tool.start_placement(&session.doc, "plan", 0));
        tool.picked([2000.0, 1000.0]);
        tool.text = "Entrance".into();
        let run = tool.finish_placement(&session.doc, "plan").unwrap();
        assert_eq!(session.doc, before);
        let result = session.execute(run.command, run.params).unwrap();
        let placed: Annotation = serde_json::from_value(result["annotation"].clone()).unwrap();
        tool.annotation_applied(&session.doc, &placed);
        assert_eq!(session.doc.annotations.len(), 1);
        assert_eq!(session.doc.journal.len(), before.journal.len() + 1);
        assert_eq!(session.doc.journal.last().unwrap().command, "annotation.define");
        session.execute("edit.undo", json!({})).unwrap();
        assert_eq!(session.doc, before);
        session.execute("edit.redo", json!({})).unwrap();
        assert_eq!(session.doc.annotations, vec![placed]);
    }

    #[test]
    fn hostile_or_degenerate_picks_do_not_advance_or_emit_commands() {
        let doc = placement_document();
        let mut tool = Drafting::default();
        assert!(!tool.start_placement(&doc, "missing", 0));
        assert!(!tool.start_placement(&doc, "plan", 3), "vertical level marks are unavailable in plans");
        assert!(!tool.start_placement(&doc, "plan", usize::MAX));
        assert!(tool.start_placement(&doc, "plan", 1));
        for point in [[f64::NAN, 0.0], [0.0, f64::INFINITY], [1_000_001.0, 0.0]] {
            tool.picked(point);
        }
        assert!(tool.points().is_empty());
        tool.picked([0.0, 0.0]);
        tool.picked([0.0, 0.0]);
        assert_eq!(tool.picking, Some(1));
        tool.picked([1000.0, 0.0]);
        tool.picked([0.0, 100_001.0]);
        assert_eq!(tool.picking, Some(2));
        assert!(tool.finish_placement(&doc, "plan").is_none());
        tool.picked([0.0, -1000.0]);
        assert!(tool.finish_placement(&doc, "other-view").is_none());
        assert!(tool.finish_placement(&doc, "plan").is_some());
    }

    #[test]
    fn a_provisional_id_claimed_during_placement_never_overwrites_the_existing_note() {
        let mut doc = placement_document();
        let mut tool = Drafting::default();
        assert!(tool.start_placement(&doc, "plan", 0));
        tool.picked([10.0, 20.0]);
        let existing = tool.draft_annotation();
        doc.annotations.push(existing.clone());
        let run = tool.finish_placement(&doc, "plan").unwrap();
        assert_ne!(run.params["id"], existing.id);
        assert_eq!(doc.annotations, vec![existing]);
    }

    #[test]
    fn placement_feedback_reports_actual_picks_step_units_validation_and_pending() {
        let doc = placement_document();
        let mut tool = Drafting::default();
        assert_eq!(tool.placement_feedback()["active"], false);
        assert_eq!(tool.placement_feedback()["picked_points"], json!([]));
        tool.start_placement(&doc, "plan", 1);
        let empty = tool.placement_feedback();
        assert_eq!(empty["tool"], "aligned_dimension");
        assert_eq!(empty["kind"], "dimension");
        assert_eq!(empty["picked_points"], json!([]));
        assert_eq!(empty["units"]["points"], "drawing_plane_mm");
        assert_eq!(empty["units"]["text_size"], "paper_mm");
        assert_eq!(empty["ready"], false);
        tool.picked([0.0, 0.0]);
        tool.picked([3000.0, 0.0]);
        let witnesses = tool.placement_feedback();
        assert_eq!(witnesses["picked_points"], json!([[0.0, 0.0], [3000.0, 0.0]]));
        assert!(witnesses["next_step"].as_str().unwrap().contains("witness offset"));
        tool.picked([1000.0, 400.0]);
        assert_eq!(tool.placement_feedback()["picked_points"], json!([[0.0, 0.0], [3000.0, 0.0], [1000.0, 400.0]]));
        assert_eq!(tool.placement_feedback()["ready"], true);
        tool.finish_placement(&doc, "plan").unwrap();
        assert_eq!(tool.placement_feedback()["pending"], true);
        assert_eq!(tool.placement_feedback()["ready"], false);
        tool.placement_failed("public annotation command rejection");
        tool.offset = 100_001.0;
        assert_eq!(tool.placement_feedback()["ready"], false);
        assert!(tool.placement_feedback()["error"].as_str().unwrap().contains("offset_mm"));
        tool.cancel_placement();
        assert_eq!(tool.placement_feedback()["picked_points"], json!([]));
    }

    #[test]
    fn native_canvas_text_editor_commits_current_typing_on_enter_and_never_on_repaint() {
        let doc = placement_document();
        let mut tool = Drafting::default();
        tool.start_placement(&doc, "plan", 0);
        tool.picked([1000.0, 2000.0]);
        let ctx = egui::Context::default();
        for _ in 0..2 {
            let mut run = None;
            let mut output = ctx.run_ui(egui::RawInput::default(), |ui| {
                egui::CentralPanel::default().show(ui, |ui| {
                    if let Some(action) = tool.canvas_editor(ui, &doc, "plan", true) {
                        run = Some(action);
                    }
                });
            });
            output.textures_delta.clear();
            assert!(run.is_none(), "repainting a complete draft does not submit it");
        }
        let mut run = None;
        let input = egui::RawInput {
            events: vec![
                egui::Event::Text("Lobby".into()),
                egui::Event::Key { key: egui::Key::Enter, physical_key: None, pressed: true, repeat: false, modifiers: egui::Modifiers::NONE },
            ],
            ..Default::default()
        };
        let mut output = ctx.run_ui(input, |ui| {
            egui::CentralPanel::default().show(ui, |ui| {
                if let Some(action) = tool.canvas_editor(ui, &doc, "plan", true) {
                    run = Some(action);
                }
            });
        });
        output.textures_delta.clear();
        let run = run.expect("Enter finishes the current native editor's draft");
        assert_eq!(run.params["kind"]["text"], "Lobby");
        assert_eq!(run.params["kind"]["at"], json!([1000.0, 2000.0]));
        assert!(tool.finish_placement(&doc, "plan").is_none());
    }

    #[test]
    fn foreign_editor_losing_focus_on_enter_does_not_finish_the_canvas_note() {
        let doc = placement_document();
        let mut tool = Drafting::default();
        tool.start_placement(&doc, "plan", 0);
        tool.picked([1000.0, 2000.0]);
        let ctx = egui::Context::default();
        let foreign = egui::Id::new("foreign-drawing-name");
        let mut name = "Drawing name".to_string();
        for _ in 0..2 {
            let mut output = ctx.run_ui(egui::RawInput::default(), |ui| {
                tool.canvas_editor(ui, &doc, "plan", true);
            });
            output.textures_delta.clear();
        }
        ctx.memory_mut(|memory| memory.request_focus(foreign));
        let mut run = None;
        let mut output = ctx.run_ui(
            egui::RawInput {
                events: vec![egui::Event::Key {
                    key: egui::Key::Enter,
                    physical_key: None,
                    pressed: true,
                    repeat: false,
                    modifiers: Default::default(),
                }],
                ..Default::default()
            },
            |ui| {
                let owned = tool.keyboard_owned(ui.ctx());
                ui.add(egui::TextEdit::singleline(&mut name).id(foreign));
                run = tool.canvas_editor(ui, &doc, "plan", owned);
            },
        );
        output.textures_delta.clear();
        ctx.data_mut(|data| data.clear());
        assert!(run.is_none(), "Enter belonged to the foreign single-line editor before it surrendered focus");
        assert_eq!(tool.placement_feedback()["pending"], false);
    }

    #[test]
    fn reference_preset_uses_exact_plan_coordinates_and_a_red_dotted_pen() {
        let mut doc = Document::default();
        doc.views.push(serde_json::from_value(json!({"id":"plan","name":"Coordination","kind":{"view":"plan","level":0}})).unwrap());
        let line = [[-123.456789, 88.000001], [4567.891234, 999.123456]];
        let mut tool = Drafting::default();
        assert!(tool.reference_guide(&doc, "plan", Some(line)));
        assert!(tool.guide_active());
        assert_eq!(tool.points[0], line[0]);
        assert_eq!(tool.points[1], line[1]);
        assert_eq!(tool.stroke, LineStyle { colour: [197, 63, 45], width_mm: 0.18, pattern: LinePattern::Dotted });
        assert!(doc.annotations.is_empty());
        assert!(!tool.reference_guide(&doc, "missing", None));
        tool.new_note(&doc);
        assert!(!tool.guide_active());
    }

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
