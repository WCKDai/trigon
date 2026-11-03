//! Direct plan tools. Sketches are local UI state; only completed actions emit engine commands.
//! All plan points already use the document's shared site-local millimetres. Buildings do not
//! have a second local transform, so neither a building centroid nor the map origin is applied.

use eframe::egui;
use serde_json::{Value, json};
use trigon_engine::SnapOptions;
use trigon_model::{Building, Document, InteriorWall, Roof, Storey, checked};

use crate::{authoring::Run, theme};

type Point = [f64; 2];
type Segment = [Point; 2];

#[derive(Clone, Copy, PartialEq, Eq)]
enum Tool {
    Wall,
    Roof,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum WallMode {
    Interior,
    Footprint,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum RoofForm {
    Flat,
    Hip,
    Gable,
}

enum Pending {
    Wall { end: Point, chain: bool },
    Footprint,
    Roof,
}

pub struct ModelTools {
    tool: Option<Tool>,
    wall_mode: WallMode,
    building: String,
    level: usize,
    points: Vec<Point>,
    attempted: Option<Point>,
    outline: Vec<Point>,
    pending: Option<Pending>,
    error: Option<String>,
    thickness: f64,
    load_bearing: bool,
    chain: bool,
    ortho: bool,
    axis_deg: f64,
    exact: bool,
    length: f64,
    snaps: SnapOptions,
    grid: bool,
    grid_step: f64,
    new_id: String,
    new_name: String,
    new_storeys: usize,
    new_height: f64,
    new_ground: f64,
    exterior_thickness: f64,
    roof_form: RoofForm,
    pitch: f64,
    overhang: f64,
    verge: f64,
    roof_thickness: f64,
    straighten: f64,
    gables: Vec<String>,
    replace_detail: bool,
    source_storeys: Vec<Storey>,
    source_phase: Option<String>,
    source_roof: Option<Roof>,
}

impl Default for ModelTools {
    fn default() -> Self {
        Self {
            tool: None,
            wall_mode: WallMode::Interior,
            building: String::new(),
            level: 0,
            points: Vec::new(),
            attempted: None,
            outline: Vec::new(),
            pending: None,
            error: None,
            thickness: 100.0,
            load_bearing: false,
            chain: true,
            ortho: true,
            axis_deg: 0.0,
            exact: false,
            length: 1000.0,
            snaps: SnapOptions::default(),
            grid: true,
            grid_step: 100.0,
            new_id: "building-1".into(),
            new_name: "Building 1".into(),
            new_storeys: 1,
            new_height: 3000.0,
            new_ground: 0.0,
            exterior_thickness: 400.0,
            roof_form: RoofForm::Gable,
            pitch: 35.0,
            overhang: 300.0,
            verge: 150.0,
            roof_thickness: 200.0,
            straighten: 0.0,
            gables: Vec::new(),
            replace_detail: false,
            source_storeys: Vec::new(),
            source_phase: None,
            source_roof: None,
        }
    }
}

fn number(ui: &mut egui::Ui, label: &str, value: &mut f64, range: std::ops::RangeInclusive<f64>, suffix: &str) {
    ui.horizontal(|ui| {
        ui.label(label);
        ui.add(egui::DragValue::new(value).speed(1.0).range(range).max_decimals(3).suffix(suffix));
    });
}

fn point_valid(point: Point) -> Result<(), String> {
    for value in point {
        checked("site-local coordinate (mm)", value, -1e8, 1e8).map_err(|e| e.to_string())?;
    }
    Ok(())
}

fn distance(a: Point, b: Point) -> f64 {
    (b[0] - a[0]).hypot(b[1] - a[1])
}

fn ring_segments(points: &[Point]) -> Vec<Segment> {
    points.iter().copied().zip(points.iter().copied().cycle().skip(1)).take(points.len()).map(|(a, b)| [a, b]).collect()
}

fn contains_or_touches(ring: &[Point], point: Point) -> bool {
    let mut inside = false;
    for [a, b] in ring_segments(ring) {
        let dx = b[0] - a[0];
        let dy = b[1] - a[1];
        let length_sq = dx * dx + dy * dy;
        if length_sq > 0.0 {
            let t = (((point[0] - a[0]) * dx + (point[1] - a[1]) * dy) / length_sq).clamp(0.0, 1.0);
            if distance(point, [a[0] + t * dx, a[1] + t * dy]) <= 1.0 {
                return true;
            }
        }
        if (a[1] > point[1]) != (b[1] > point[1]) && point[0] < (b[0] - a[0]) * (point[1] - a[1]) / (b[1] - a[1]) + a[0] {
            inside = !inside;
        }
    }
    inside
}

impl ModelTools {
    pub fn active(&self) -> bool {
        self.tool.is_some()
    }

    pub fn picking(&self) -> bool {
        self.active() && self.pending.is_none()
    }

    pub fn title(&self) -> &'static str {
        match (self.tool, self.wall_mode) {
            (Some(Tool::Wall), WallMode::Interior) => "Draw walls",
            (Some(Tool::Wall), WallMode::Footprint) => "Exterior footprint",
            (Some(Tool::Roof), _) => "Roof by footprint",
            _ => "Model tools",
        }
    }

    pub fn status(&self) -> &'static str {
        if self.pending.is_some() {
            return "Applying the completed action…";
        }
        match (self.tool, self.wall_mode, self.points.is_empty()) {
            (Some(Tool::Wall), WallMode::Interior, true) => "Click the wall start in plan. Esc cancels the tool.",
            (Some(Tool::Wall), WallMode::Interior, false) => "Click the wall end. Each accepted segment is undoable. Enter ends the chain.",
            (Some(Tool::Wall), WallMode::Footprint, _) => {
                "Click exterior corners, then Finish footprint or click the first corner. Esc discards the sketch."
            }
            (Some(Tool::Roof), _, _) => "Click inside a building footprint, choose the roof form, then Create / Update roof.",
            _ => "Choose Wall or Roof to start.",
        }
    }

    pub fn error(&self) -> Option<&str> {
        self.error.as_deref()
    }

    /// Read-only Studio feedback for human and AI clients. No document writes, credentials,
    /// external coordinates or file paths are involved in this transient tool state.
    pub fn feedback(&self) -> Value {
        let tool = self.tool.map(|tool| match tool {
            Tool::Wall => "wall",
            Tool::Roof => "roof",
        });
        let wall_mode = (self.tool == Some(Tool::Wall)).then_some(match self.wall_mode {
            WallMode::Interior => "interior",
            WallMode::Footprint => "exterior_footprint",
        });
        let pending_action = self.pending.as_ref().map(|pending| match pending {
            Pending::Wall { .. } => "interior.add_walls",
            Pending::Footprint => "building.add",
            Pending::Roof => "building.set_roof",
        });
        let building = if self.tool == Some(Tool::Wall) && self.wall_mode == WallMode::Footprint { &self.new_id } else { &self.building };
        let mut draft_points = self.points.clone();
        if self.tool == Some(Tool::Wall)
            && self.wall_mode == WallMode::Interior
            && let Some(end) = self.attempted
        {
            draft_points.push(end);
        }
        json!({
            "active":self.active(),"tool":tool,"wall_mode":wall_mode,
            "units":"mm","coordinate_system":"site_local_xy_z_up",
            "scope":{"building":self.active().then_some(building),"level":self.level},
            "draft_points":draft_points,"source_footprint":(wall_mode != Some("exterior_footprint")).then_some(&self.outline),
            "pending":self.pending.is_some(),"pending_action":pending_action,"error":self.error,
            "wall":{"thickness_mm":self.thickness,"load_bearing":self.load_bearing,"chain":self.chain,
                "location_line":"centre","height_constraint":"full_storey"},
            "sketch":{"ortho":self.ortho,"axes_deg":self.axis_deg,"exact_length":self.exact,
                "length_mm":self.length,"grid_mm":self.snap_options().grid_mm,
                "snaps":{"endpoints":self.snaps.endpoints,"midpoints":self.snaps.midpoints,"intersections":self.snaps.intersections}},
            "new_building":{"id":self.new_id,"name":self.new_name,"storeys":self.new_storeys,
                "storey_height_mm":self.new_height,"ground_floor_mm":self.new_ground,"wall_thickness_mm":self.exterior_thickness,
                "location_line":"exterior_face"},
            "roof":{"form":match self.roof_form {RoofForm::Flat=>"flat",RoofForm::Hip=>"hip",RoofForm::Gable=>"gable"},
                "pitch_deg":self.pitch,"overhang_mm":self.overhang,"verge_mm":self.verge,
                "thickness_mm":self.roof_thickness,"straighten_mm":self.straighten,"gables":self.gables,
                "replace_existing_details":self.replace_detail}
        })
    }

    pub fn start_wall(&mut self, doc: &Document, level: usize) {
        self.cancel();
        self.tool = Some(Tool::Wall);
        self.level = level;
        self.wall_mode = if doc.buildings.is_empty() { WallMode::Footprint } else { WallMode::Interior };
        self.ensure_building(doc);
        self.capture_scope(doc);
        if !doc.buildings.iter().any(|b| b.id == self.new_id) {
            return;
        }
        if let Some(id) = (1..=101).map(|n| format!("building-{n}")).find(|id| doc.buildings.iter().all(|b| &b.id != id)) {
            self.new_name = id.replace('-', " ");
            self.new_id = id;
        }
    }

    pub fn start_roof(&mut self, doc: &Document, level: usize) {
        self.cancel();
        self.tool = Some(Tool::Roof);
        self.level = level;
        self.ensure_building(doc);
        self.capture_scope(doc);
        if let Ok(building) = self.selected_building(doc) {
            self.load_roof(building);
        } else {
            self.error = Some("Create an explicit exterior footprint before adding a roof.".into());
        }
    }

    pub fn start_footprint(&mut self, doc: &Document, level: usize) {
        self.start_wall(doc, level);
        self.wall_mode = WallMode::Footprint;
    }

    /// The task row stays outside the scrolling settings and uses the same draft
    /// and commit path as the detailed panel and Enter shortcut.
    pub fn task_actions(&mut self, ui: &mut egui::Ui, doc: &Document) -> Option<Run> {
        let mut run = None;
        ui.horizontal_wrapped(|ui| {
            let (label, ready) = match (self.tool, self.wall_mode) {
                (Some(Tool::Wall), WallMode::Footprint) => ("Finish footprint", self.points.len() >= 3),
                (Some(Tool::Roof), _) => ("Create / Update roof", !self.outline.is_empty()),
                _ => ("Finish walls", true),
            };
            if ui.add_enabled(ready && self.pending.is_none(), egui::Button::new(label)).clicked() {
                if self.tool == Some(Tool::Wall) && self.wall_mode == WallMode::Interior {
                    self.cancel();
                } else {
                    run = self.finish(doc);
                }
            }
            if ui.button("Cancel tool").clicked() {
                self.cancel();
            }
            if self.anchor().is_some() {
                ui.separator();
                ui.checkbox(&mut self.exact, "Exact length");
                if self.exact {
                    ui.add(egui::DragValue::new(&mut self.length).range(100.0..=200_000.0).suffix(" mm"));
                }
            }
        });
        run
    }

    /// Keep the active scope current even when Properties is collapsed. A workplane or source
    /// boundary change discards only transient sketch state; it never writes to the document.
    pub(crate) fn sync_context(&mut self, doc: &Document, level: usize) {
        if !self.active() || self.pending.is_some() {
            return;
        }
        if self.level != level {
            self.level = level;
            self.end_chain();
        }
        if self.tool == Some(Tool::Wall) && self.wall_mode == WallMode::Footprint {
            return;
        }
        match self.selected_building(doc) {
            Ok(building) if self.outline != building.footprint => {
                self.end_chain();
                self.outline = building.footprint.clone();
                if self.tool == Some(Tool::Roof) {
                    self.load_roof(building);
                }
            }
            Err(error) => {
                self.end_chain();
                self.outline.clear();
                self.error = Some(error);
            }
            _ => {}
        }
        if let Ok(building) = self.selected_building(doc) {
            let phase = doc.phases.get(doc.active_index()).map(|phase| phase.id.clone());
            if self.source_storeys != building.storeys || self.source_phase != phase {
                self.end_chain();
                self.source_storeys = building.storeys.clone();
                self.source_phase = phase;
                self.error = Some(
                    "The storey schedule or active phase changed. The unfinished wall chain was discarded; review the current workplane.".into(),
                );
            }
            if self.tool == Some(Tool::Roof) && self.source_roof.as_ref() != Some(&building.roof) {
                self.error = Some("The roof source changed while this draft was open. Reload the current roof before applying.".into());
            }
        }
    }

    fn capture_scope(&mut self, doc: &Document) {
        self.source_storeys = self.selected_building(doc).map(|building| building.storeys.clone()).unwrap_or_default();
        self.source_phase = doc.phases.get(doc.active_index()).map(|phase| phase.id.clone());
    }

    pub fn cancel(&mut self) {
        self.tool = None;
        self.end_chain();
        self.outline.clear();
        self.pending = None;
        self.source_roof = None;
        self.source_storeys.clear();
        self.source_phase = None;
    }

    pub fn escape(&mut self) {
        if self.tool == Some(Tool::Wall) && !self.points.is_empty() {
            self.end_chain();
        } else {
            self.cancel();
        }
    }

    pub fn end_chain(&mut self) {
        self.points.clear();
        self.attempted = None;
        self.error = None;
    }

    pub fn anchor(&self) -> Option<Point> {
        (self.tool == Some(Tool::Wall)).then(|| self.points.last().copied()).flatten()
    }

    pub fn snap_options(&self) -> SnapOptions {
        SnapOptions { grid_mm: (self.grid && checked("grid spacing", self.grid_step, 1.0, 10000.0).is_ok()).then_some(self.grid_step), ..self.snaps }
    }

    /// Constraints apply to the site plane. The editable ortho axes can follow a rotated wing.
    pub fn constrained(&self, point: Point) -> Point {
        if self.wall_mode == WallMode::Footprint && self.points.len() >= 3 && self.points.first().is_some_and(|first| distance(*first, point) <= 1.0)
        {
            return point;
        }
        let Some(anchor) = self.anchor() else { return point };
        if point_valid(point).is_err() || !self.axis_deg.is_finite() {
            return point;
        }
        let mut delta = [point[0] - anchor[0], point[1] - anchor[1]];
        if self.ortho {
            let (s, c) = self.axis_deg.to_radians().sin_cos();
            let u = delta[0] * c + delta[1] * s;
            let v = -delta[0] * s + delta[1] * c;
            delta = if u.abs() >= v.abs() { [u * c, u * s] } else { [-v * s, v * c] };
        }
        let length = delta[0].hypot(delta[1]);
        if self.exact && checked("exact length", self.length, 100.0, 200_000.0).is_ok() && length > 1e-9 {
            delta = [delta[0] / length * self.length, delta[1] / length * self.length];
        }
        [anchor[0] + delta[0], anchor[1] + delta[1]]
    }

    pub fn preview(&self, cursor: Option<Point>) -> Vec<Segment> {
        if self.tool == Some(Tool::Roof) {
            return ring_segments(&self.outline);
        }
        let mut lines: Vec<_> = self.points.windows(2).map(|p| [p[0], p[1]]).collect();
        if let Some(anchor) = self.anchor()
            && let Some(end) = cursor.or(self.attempted).map(|point| self.constrained(point))
            && point_valid(end).is_ok()
        {
            lines.push([anchor, end]);
        }
        if self.wall_mode == WallMode::Footprint
            && self.points.len() >= 3
            && let (Some(first), Some(last)) = (self.points.first(), self.points.last())
        {
            lines.push([*last, *first]);
        }
        lines
    }

    /// The parent executes the returned action once, then calls acknowledge with its outcome.
    pub fn point(&mut self, doc: &Document, point: Point) -> Option<Run> {
        self.sync_context(doc, self.level);
        if !self.picking() {
            return None;
        }
        let result = self.point_action(doc, point);
        match result {
            Ok(run) => {
                self.error = None;
                run
            }
            Err(error) => {
                self.error = Some(error);
                None
            }
        }
    }

    fn point_action(&mut self, doc: &Document, point: Point) -> Result<Option<Run>, String> {
        point_valid(point)?;
        if self.tool == Some(Tool::Roof) {
            let mut candidates = doc.buildings.iter().filter(|building| contains_or_touches(&building.footprint, point));
            let building =
                candidates.next().ok_or_else(|| "Click inside an existing building footprint, or choose its name in Properties.".to_string())?;
            if candidates.next().is_some() {
                return Err("The footprints overlap here. Choose the building explicitly in Properties.".into());
            }
            self.building = building.id.clone();
            self.load_roof(building);
            self.capture_scope(doc);
            return Ok(None);
        }
        if self.wall_mode == WallMode::Footprint {
            if self.points.len() >= 3 && self.points.first().is_some_and(|first| distance(*first, point) <= 1.0) {
                return self.completed_action(doc).map(Some);
            }
            if self.exact {
                checked("exact footprint length", self.length, 100.0, 200_000.0).map_err(|e| e.to_string())?;
            }
            checked("ortho axes (degrees)", self.axis_deg, -180.0, 180.0).map_err(|e| e.to_string())?;
            let point = self.constrained(point);
            point_valid(point)?;
            if self.points.len() >= 2000 {
                return Err("A footprint supports at most 2000 corners. Finish or cancel this sketch.".into());
            }
            if let Some(last) = self.points.last() {
                checked("footprint segment length", distance(*last, point), 1.001, 200_000.0).map_err(|e| e.to_string())?;
            }
            self.points.push(point);
            self.attempted = None;
            return Ok(None);
        }
        let building = self.wall_building(doc)?;
        let Some(anchor) = self.anchor() else {
            self.points.push(point);
            return Ok(None);
        };
        if self.exact {
            checked("exact wall length", self.length, 100.0, 200_000.0).map_err(|e| e.to_string())?;
        }
        checked("ortho axes (degrees)", self.axis_deg, -180.0, 180.0).map_err(|e| e.to_string())?;
        let end = self.constrained(point);
        let wall = InteriorWall {
            id: String::new(),
            levels: vec![self.level],
            a: anchor,
            b: end,
            thickness: self.thickness,
            load_bearing: self.load_bearing,
            doors: Vec::new(),
        };
        wall.validate(building.storeys.len()).map_err(|e| e.to_string())?;
        self.attempted = Some(end);
        self.pending = Some(Pending::Wall { end, chain: self.chain });
        Ok(Some(Run { command: "interior.add_walls", params: json!({"building":building.id,"walls":[wall]}) }))
    }

    pub fn finish(&mut self, doc: &Document) -> Option<Run> {
        self.sync_context(doc, self.level);
        if !self.picking() {
            return None;
        }
        if self.tool == Some(Tool::Wall) && self.wall_mode == WallMode::Interior {
            self.end_chain();
            return None;
        }
        match self.completed_action(doc) {
            Ok(run) => {
                self.error = None;
                Some(run)
            }
            Err(error) => {
                self.error = Some(error);
                None
            }
        }
    }

    fn completed_action(&mut self, doc: &Document) -> Result<Run, String> {
        if self.tool == Some(Tool::Roof) {
            let run = self.roof_action(doc)?;
            self.pending = Some(Pending::Roof);
            return Ok(run);
        }
        if self.tool != Some(Tool::Wall) || self.wall_mode != WallMode::Footprint {
            return Err("Start an exterior footprint sketch first.".into());
        }
        if self.points.len() < 3 {
            return Err("Pick at least three exterior corners before finishing the closed footprint.".into());
        }
        if self.new_id.is_empty()
            || self.new_id.len() > 64
            || !self.new_id.chars().all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-'))
            || self.new_name.trim().is_empty()
            || self.new_name.len() > 200
            || self.new_name.chars().any(char::is_control)
        {
            return Err("Give the building a 1–64 byte simple ID and a 1–200 byte name.".into());
        }
        if doc.buildings.len() >= 100 || doc.buildings.iter().any(|building| building.id == self.new_id) {
            return Err("That building ID already exists, or the project has reached the 100-building limit.".into());
        }
        checked("exterior wall thickness", self.exterior_thickness, 100.0, 1500.0).map_err(|e| e.to_string())?;
        checked("ground floor", self.new_ground, 0.0, 5000.0).map_err(|e| e.to_string())?;
        Building::new(&self.new_id, &self.new_name, &self.points, self.new_storeys, self.new_height).map_err(|e| e.to_string())?;
        self.pending = Some(Pending::Footprint);
        Ok(Run {
            command: "building.add",
            params: json!({"id":self.new_id,"name":self.new_name,"footprint":self.points,"storeys":self.new_storeys,
                "storey_height_mm":self.new_height,"ground_floor_mm":self.new_ground,"wall_thickness_mm":self.exterior_thickness,"preserve_vertices":true}),
        })
    }

    pub fn acknowledge(&mut self, success: bool, error: Option<String>) {
        let Some(pending) = self.pending.take() else { return };
        if !success {
            self.error = Some(error.unwrap_or_else(|| "The command was rejected. The sketch is retained; correct the inputs and try again.".into()));
            return;
        }
        self.error = None;
        match pending {
            Pending::Wall { end, chain } => {
                self.end_chain();
                if chain {
                    self.points.push(end);
                }
            }
            Pending::Footprint => {
                self.building = self.new_id.clone();
                self.cancel();
            }
            Pending::Roof => self.cancel(),
        }
    }

    fn selected_building<'a>(&self, doc: &'a Document) -> Result<&'a Building, String> {
        let mut buildings = doc.buildings.iter().filter(|building| building.id == self.building);
        let building = buildings.next().ok_or_else(|| "Choose an existing building in Properties.".to_string())?;
        if buildings.next().is_some() {
            return Err("This building ID is ambiguous. Choose a project with unique building IDs.".into());
        }
        Ok(building)
    }

    fn wall_building<'a>(&self, doc: &'a Document) -> Result<&'a Building, String> {
        let building = self.selected_building(doc)?;
        let storey = building
            .storeys
            .get(self.level)
            .ok_or_else(|| "This building has no storey at the current plan level. Choose another plan level.".to_string())?;
        if storey.roof_level {
            return Err("Interior walls on roof levels require roof clipping, which this wall tool does not support.".into());
        }
        Ok(building)
    }

    fn ensure_building(&mut self, doc: &Document) {
        if !doc.buildings.iter().any(|building| building.id == self.building) {
            self.building = doc
                .buildings
                .iter()
                .find(|building| building.storeys.get(self.level).is_some())
                .or(doc.buildings.first())
                .map(|b| b.id.clone())
                .unwrap_or_default();
        }
        if let Ok(building) = self.selected_building(doc) {
            self.outline = building.footprint.clone();
            if let Some([a, b]) = ring_segments(&building.footprint).first() {
                self.axis_deg = (b[1] - a[1]).atan2(b[0] - a[0]).to_degrees();
            }
        }
    }

    fn load_roof(&mut self, building: &Building) {
        self.source_roof = Some(building.roof.clone());
        self.error = None;
        self.outline = building.footprint.clone();
        self.gables.clear();
        self.replace_detail = false;
        match building.roof.base() {
            Roof::Flat => {
                self.roof_form = RoofForm::Gable;
                // Start with a visible pair of shorter opposite facades on a four-sided
                // footprint. Other shapes require the owner to choose their gables.
                if building.footprint.len() == 4 {
                    let edges = ring_segments(&building.footprint);
                    let pair = if distance(edges[0][0], edges[0][1]) <= distance(edges[1][0], edges[1][1]) { [0, 2] } else { [1, 3] };
                    self.gables = building.facades.iter().filter(|facade| pair.contains(&facade.edge)).map(|facade| facade.id.clone()).collect();
                }
            }
            Roof::Pitched { pitch_deg, gables, overhang_mm, verge_mm, thickness_mm, straighten_mm, .. }
            | Roof::HalfHipped { pitch_deg, gables, overhang_mm, verge_mm, thickness_mm, straighten_mm, .. } => {
                self.roof_form = if gables.is_empty() { RoofForm::Hip } else { RoofForm::Gable };
                self.pitch = *pitch_deg;
                self.overhang = *overhang_mm;
                self.verge = *verge_mm;
                self.roof_thickness = *thickness_mm;
                self.straighten = *straighten_mm;
                self.gables = gables.clone();
            }
            _ => self.roof_form = RoofForm::Gable,
        }
    }

    fn replacing_detail(&self, building: &Building) -> bool {
        match building.roof.base() {
            Roof::Flat => false,
            Roof::Pitched { eave_facades, dormers, rooflights, drainage, .. } => {
                eave_facades.is_some()
                    || (self.roof_form == RoofForm::Flat
                        && (!dormers.is_empty() || !rooflights.is_empty() || drainage.is_some() || building.roof.fascia_depth().is_some()))
            }
            _ => true,
        }
    }

    fn roof_action(&self, doc: &Document) -> Result<Run, String> {
        let building = self.selected_building(doc)?;
        if self.source_roof.as_ref() != Some(&building.roof) {
            return Err("The roof source changed while this draft was open. Reload the current roof before applying.".into());
        }
        if self.replacing_detail(building) && !self.replace_detail {
            return Err("This change replaces unsupported roof form or details. Review and enable Replace existing roof details first.".into());
        }
        if self.roof_form == RoofForm::Flat {
            return Ok(Run { command: "building.set_roof", params: json!({"building":building.id,"kind":"flat"}) });
        }
        checked("roof pitch (degrees)", self.pitch, 5.0, 75.0).map_err(|e| e.to_string())?;
        checked("roof overhang", self.overhang, 0.0, 2000.0).map_err(|e| e.to_string())?;
        checked("gable verge", self.verge, 0.0, 1500.0).map_err(|e| e.to_string())?;
        checked("roof thickness", self.roof_thickness, 0.0, 1000.0).map_err(|e| e.to_string())?;
        checked("roof straightening", self.straighten, 0.0, 10000.0).map_err(|e| e.to_string())?;
        let gables = if self.roof_form == RoofForm::Gable { self.gables.clone() } else { Vec::new() };
        if self.roof_form == RoofForm::Gable && gables.is_empty() {
            return Err("Choose at least one gable facade, or use Hip for slopes on every side.".into());
        }
        for gable in &gables {
            building.facade(gable).map_err(|e| e.to_string())?;
        }
        let mut params = json!({"building":building.id,"kind":"pitched","pitch_deg":self.pitch,"gables":gables,
            "overhang_mm":self.overhang,"verge_mm":self.verge,"thickness_mm":self.roof_thickness,"straighten_mm":self.straighten});
        // The command replaces its roof source. Preserve every representable existing pitched
        // detail, letting the engine reject a changed slope that cannot host it atomically.
        if !self.replacing_detail(building)
            && let Roof::Pitched { gutters, drainage, dormers, rooflights, .. } = building.roof.base()
        {
            params["gutters"] = json!(gutters);
            params["drainage"] = json!(drainage);
            params["dormers"] = json!(dormers);
            params["rooflights"] = json!(rooflights);
            if let Some(depth) = building.roof.fascia_depth() {
                params["fascia_depth_mm"] = json!(depth);
            }
        }
        Ok(Run { command: "building.set_roof", params })
    }

    pub fn properties(&mut self, ui: &mut egui::Ui, doc: &Document, level: usize) -> Option<Run> {
        if !self.active() {
            return None;
        }
        self.sync_context(doc, level);
        ui.label(egui::RichText::new(self.title()).strong().size(14.0));
        ui.weak(self.status());
        ui.add_space(6.0);
        let mut run = None;
        ui.add_enabled_ui(self.pending.is_none(), |ui| {
            if self.tool == Some(Tool::Wall) {
                let old_mode = self.wall_mode;
                ui.horizontal_wrapped(|ui| {
                    ui.add_enabled_ui(!doc.buildings.is_empty(), |ui| {
                        ui.selectable_value(&mut self.wall_mode, WallMode::Interior, "Interior wall");
                    });
                    ui.selectable_value(&mut self.wall_mode, WallMode::Footprint, "Exterior footprint");
                });
                if old_mode != self.wall_mode {
                    self.end_chain();
                }
            }
            if self.tool == Some(Tool::Roof) || self.wall_mode == WallMode::Interior {
                let previous = self.building.clone();
                egui::ComboBox::from_id_salt("model-tool-building")
                    .selected_text(doc.buildings.iter().find(|b| b.id == self.building).map_or("Choose building", |b| b.name.as_str()))
                    .width(ui.available_width())
                    .show_ui(ui, |ui| {
                        for building in &doc.buildings {
                            ui.selectable_value(&mut self.building, building.id.clone(), format!("{} · {}", building.name, building.id));
                        }
                    });
                if previous != self.building {
                    self.end_chain();
                    self.ensure_building(doc);
                    self.capture_scope(doc);
                    if self.tool == Some(Tool::Roof)
                        && let Ok(building) = self.selected_building(doc)
                    {
                        self.load_roof(building);
                    }
                }
            }
            if self.tool == Some(Tool::Roof) {
                self.roof_properties(ui, doc);
                if theme::primary(ui, "Create / Update roof").clicked() {
                    run = self.finish(doc);
                }
            } else {
                self.wall_properties(ui, doc);
                if self.wall_mode == WallMode::Footprint {
                    ui.weak(format!("{} corners · closed exterior wall line", self.points.len()));
                    if ui.add_enabled(self.points.len() >= 3, egui::Button::new("Finish footprint")).clicked() {
                        run = self.finish(doc);
                    }
                    if ui.add_enabled(!self.points.is_empty(), egui::Button::new("Remove last corner")).clicked() {
                        self.points.pop();
                        self.error = None;
                    }
                } else if ui.button("End wall chain").clicked() {
                    self.end_chain();
                }
            }
            if ui.button("Cancel tool · Esc").clicked() {
                self.cancel();
            }
        });
        if let Some(error) = self.error() {
            ui.colored_label(theme::ACCENT, error);
        }
        run
    }

    fn wall_properties(&mut self, ui: &mut egui::Ui, doc: &Document) {
        if self.wall_mode == WallMode::Footprint {
            ui.label("Building name");
            ui.add(egui::TextEdit::singleline(&mut self.new_name).char_limit(200).desired_width(f32::INFINITY));
            egui::CollapsingHeader::new("Building identity").show(ui, |ui| {
                ui.label("Building ID");
                ui.add(egui::TextEdit::singleline(&mut self.new_id).char_limit(64).desired_width(f32::INFINITY));
            });
            ui.horizontal(|ui| {
                ui.label("Storeys");
                ui.add(egui::DragValue::new(&mut self.new_storeys).range(1..=60));
            });
            number(ui, "Storey height", &mut self.new_height, 2000.0..=10000.0, " mm");
            number(ui, "Ground floor", &mut self.new_ground, 0.0..=5000.0, " mm");
            number(ui, "Wall thickness", &mut self.exterior_thickness, 100.0..=1500.0, " mm");
            ui.weak("Location line: exterior face. Finish creates the named building, its facade walls, and floor slabs as one undoable command.");
        } else {
            number(ui, "Wall thickness", &mut self.thickness, 40.0..=1000.0, " mm");
            ui.checkbox(&mut self.load_bearing, "Load bearing");
            ui.checkbox(&mut self.chain, "Chain walls");
            if let Ok(building) = self.wall_building(doc) {
                if let Some(storey) = building.storeys.get(self.level) {
                    ui.label(format!("Base: {} · L{} · {:.3} mm", storey.name, self.level, storey.elevation));
                    ui.label(format!("Height: full storey · {:.3} mm", storey.height));
                }
            } else if let Err(error) = self.wall_building(doc) {
                ui.colored_label(theme::ACCENT, error);
            }
            ui.weak("Location line: wall centre. Set the base level in the plan view controls. Custom heights, offsets and roof attachment are not supported.");
        }
        ui.separator();
        ui.checkbox(&mut self.ortho, "Ortho");
        if self.ortho {
            number(ui, "Axes", &mut self.axis_deg, -180.0..=180.0, "°");
        }
        ui.checkbox(&mut self.exact, "Exact next segment length");
        if self.exact {
            number(ui, "Length", &mut self.length, 100.0..=200_000.0, " mm");
        }
        egui::CollapsingHeader::new("Object snaps").id_salt("model-tool-snaps").show(ui, |ui| {
            ui.checkbox(&mut self.snaps.endpoints, "Endpoints");
            ui.checkbox(&mut self.snaps.midpoints, "Midpoints");
            ui.checkbox(&mut self.snaps.intersections, "Intersections");
            ui.checkbox(&mut self.grid, "Site grid");
            if self.grid {
                number(ui, "Spacing", &mut self.grid_step, 1.0..=10000.0, " mm");
            }
            ui.weak("Hold Alt to bypass object snaps. Ortho and exact length then constrain the chosen direction.");
        });
    }

    fn roof_properties(&mut self, ui: &mut egui::Ui, doc: &Document) {
        if let Ok(building) = self.selected_building(doc)
            && self.source_roof.as_ref() != Some(&building.roof)
        {
            ui.colored_label(theme::ACCENT, "This draft uses an older roof source. Reloading discards the local roof changes.");
            if ui.button("Reload current roof").clicked() {
                self.load_roof(building);
                self.capture_scope(doc);
            }
        }
        ui.horizontal_wrapped(|ui| {
            ui.selectable_value(&mut self.roof_form, RoofForm::Gable, "Gable");
            ui.selectable_value(&mut self.roof_form, RoofForm::Hip, "Hip");
            ui.selectable_value(&mut self.roof_form, RoofForm::Flat, "Flat");
        });
        if let Ok(building) = self.selected_building(doc) {
            ui.weak(format!("Boundary: {} existing exterior wall lines · eaves {:.3} mm", building.footprint.len(), building.height()));
            if self.roof_form != RoofForm::Flat {
                number(ui, "Pitch", &mut self.pitch, 5.0..=75.0, "°");
                number(ui, "Eave overhang", &mut self.overhang, 0.0..=2000.0, " mm");
                number(ui, "Build-up", &mut self.roof_thickness, 0.0..=1000.0, " mm");
                if self.roof_form == RoofForm::Gable {
                    number(ui, "Gable verge", &mut self.verge, 0.0..=1500.0, " mm");
                    ui.label("Gable ends · unchecked sides define slopes");
                    for facade in &building.facades {
                        let mut selected = self.gables.contains(&facade.id);
                        if ui.checkbox(&mut selected, &facade.id).changed() {
                            self.gables.retain(|id| id != &facade.id);
                            if selected {
                                self.gables.push(facade.id.clone());
                            }
                        }
                    }
                }
                egui::CollapsingHeader::new("Footprint steps").id_salt("model-tool-roof-steps").show(ui, |ui| {
                    number(ui, "Straighten steps", &mut self.straighten, 0.0..=10000.0, " mm");
                    ui.weak("0 follows every measured footprint edge.");
                });
            } else {
                ui.weak("Flat slab at the building eaves. Flat-roof overhang and arbitrary roof sketches are not supported by this command.");
            }
            if self.replacing_detail(building) {
                ui.colored_label(theme::ACCENT, "This form replaces existing advanced roof forms, selected eave lines or slope details. Review the existing roof before applying.");
                ui.checkbox(&mut self.replace_detail, "Replace existing roof details");
            }
        } else {
            ui.weak("Choose a building footprint first. Roofs use its real parametric wall boundary.");
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use trigon_data::{Credentials, Live};
    use trigon_engine::Session;

    fn session() -> Session {
        let mut session = Session::new(Box::new(Live::new(Credentials::default())));
        session.execute("building.add", json!({"id":"wing","name":"Public rotated fixture","footprint":[[12000,8000],[20000,14000],[15200,20400],[7200,14400]],"storeys":2,"storey_height_mm":3200})).unwrap();
        session
    }

    #[test]
    fn reviewed_roof_draft_cannot_overwrite_an_intervening_api_edit() {
        let mut session = session();
        session.execute("building.set_roof", json!({"building":"wing","kind":"pitched","pitch_deg":35,"gables":[]})).unwrap();
        let mut tools = ModelTools::default();
        tools.start_roof(&session.doc, 0);
        tools.overhang = 450.0;
        session.execute("building.set_roof", json!({"building":"wing","kind":"pitched","pitch_deg":45,"gables":[]})).unwrap();
        let changed = session.doc.clone();
        tools.sync_context(&session.doc, 0);
        assert!(tools.finish(&session.doc).is_none(), "a stale roof form must not emit a replacement");
        assert_eq!(tools.overhang, 450.0, "local changes remain available for review");
        assert_eq!(tools.pitch, 35.0);
        assert!(tools.error().is_some_and(|error| error.contains("changed")));
        assert_eq!(session.doc, changed);
    }

    #[test]
    fn basement_insertion_discards_the_unfinished_wall_instead_of_rehosting_it() {
        let mut session = session();
        let mut tools = ModelTools::default();
        tools.start_wall(&session.doc, 0);
        tools.point(&session.doc, [12000.0, 11000.0]);
        session.execute("building.add_basement", json!({"building":"wing","floor":-2500})).unwrap();
        let changed = session.doc.clone();
        tools.sync_context(&session.doc, 0);
        assert!(tools.anchor().is_none(), "L0 now identifies a different storey; the old chain must be discarded");
        assert!(tools.point(&session.doc, [15000.0, 13000.0]).is_none(), "the next click cannot finish the old wall");
        assert_eq!(session.doc, changed);
    }

    #[test]
    fn first_corner_closure_overrides_exact_length_and_phase_changes_end_wall_chains() {
        let mut session = Session::new(Box::new(Live::new(Credentials::default())));
        let mut tools = ModelTools::default();
        tools.start_wall(&session.doc, 0);
        for point in [[0.0, 0.0], [4000.0, 0.0], [4000.0, 3000.0], [0.0, 3000.0]] {
            tools.point(&session.doc, point);
        }
        tools.exact = true;
        tools.length = 1000.0;
        assert_eq!(tools.constrained([0.0, 0.0]), [0.0, 0.0], "the preview and click must both close at the first corner");
        let run = tools.point(&session.doc, [0.0, 0.0]).unwrap();
        assert_eq!(run.params["footprint"].as_array().unwrap().len(), 4);
        session.execute(run.command, run.params).unwrap();
        tools.acknowledge(true, None);
        tools.start_wall(&session.doc, 0);
        tools.point(&session.doc, [1000.0, 1000.0]);
        session.doc.phases.push(trigon_model::Phase { id: "proposed".into(), name: "Proposed".into() });
        session.doc.active_phase = Some("proposed".into());
        tools.sync_context(&session.doc, 0);
        assert!(tools.anchor().is_none());
        assert!(tools.point(&session.doc, [2000.0, 1000.0]).is_none());
    }

    #[test]
    fn site_points_and_rotated_axes_build_one_typed_wall_with_exact_undo() {
        let mut session = session();
        let before = session.doc.clone();
        let mut tools = ModelTools::default();
        tools.start_wall(&session.doc, 1);
        tools.exact = true;
        tools.length = 2500.0;
        assert!(tools.point(&session.doc, [12000.0, 11000.0]).is_none());
        assert_eq!(session.doc, before, "starting a sketch must not change or journal the model");
        let run = tools.point(&session.doc, [16000.0, 14000.0]).unwrap();
        assert_eq!(run.command, "interior.add_walls");
        assert_eq!(run.params["building"], "wing");
        assert_eq!(run.params["walls"][0]["levels"], json!([1]));
        assert_eq!(run.params["walls"][0]["a"], json!([12000.0, 11000.0]));
        let end: Point = serde_json::from_value(run.params["walls"][0]["b"].clone()).unwrap();
        assert!((end[0] - 14000.0).abs() < 1e-8 && (end[1] - 12500.0).abs() < 1e-8);
        assert_eq!(tools.anchor(), Some([12000.0, 11000.0]), "emission waits for the parent outcome");
        assert!(tools.point(&session.doc, [16000.0, 15000.0]).is_none(), "pending actions cannot emit twice");
        session.execute(run.command, run.params).unwrap();
        tools.acknowledge(true, None);
        assert_eq!(tools.anchor(), Some(end));
        let wall = &session.doc.buildings[0].interior_walls[0];
        assert_eq!(wall.a, [12000.0, 11000.0]);
        assert_eq!(wall.b, end);
        assert_eq!(wall.levels, vec![1]);
        session.execute("edit.undo", json!({})).unwrap();
        assert_eq!(session.doc, before);
    }

    #[test]
    fn failed_wall_commit_keeps_start_endpoint_and_real_error_for_retry() {
        let mut session = session();
        let mut tools = ModelTools::default();
        tools.start_wall(&session.doc, 0);
        tools.ortho = false;
        tools.point(&session.doc, [12000.0, 11000.0]);
        let run = tools.point(&session.doc, [15000.0, 13000.0]).unwrap();
        let mut invalid = run.params;
        invalid["walls"][0]["thickness"] = json!(-5);
        let before = session.doc.clone();
        let error = session.execute(run.command, invalid).unwrap_err().to_string();
        assert_eq!(session.doc, before);
        tools.acknowledge(false, Some(error.clone()));
        assert_eq!(tools.error(), Some(error.as_str()));
        assert_eq!(tools.anchor(), Some([12000.0, 11000.0]));
        assert_eq!(tools.preview(None), vec![[[12000.0, 11000.0], [15000.0, 13000.0]]]);
        let retry = tools.point(&session.doc, [15000.0, 13000.0]).unwrap();
        session.execute(retry.command, retry.params).unwrap();
        tools.acknowledge(true, None);
        assert_eq!(tools.anchor(), Some([15000.0, 13000.0]));
    }

    #[test]
    fn hostile_wall_points_and_constraints_never_emit_or_advance() {
        let session = session();
        let mut tools = ModelTools::default();
        tools.start_wall(&session.doc, 0);
        for point in [[f64::NAN, 0.0], [f64::INFINITY, 0.0], [1e20, 0.0]] {
            assert!(tools.point(&session.doc, point).is_none());
            assert!(tools.anchor().is_none());
            assert!(tools.error().is_some());
        }
        tools.point(&session.doc, [12000.0, 11000.0]);
        tools.exact = true;
        for length in [f64::NAN, f64::INFINITY, -1.0, 200_001.0] {
            tools.length = length;
            assert!(tools.point(&session.doc, [14000.0, 11000.0]).is_none());
            assert_eq!(tools.anchor(), Some([12000.0, 11000.0]));
            assert!(tools.pending.is_none());
        }
        tools.cancel();
        assert!(!tools.active());
        assert!(tools.preview(None).is_empty());
    }

    #[test]
    fn empty_project_exterior_sketch_only_creates_explicit_closed_building_on_finish() {
        let mut session = Session::new(Box::new(Live::new(Credentials::default())));
        let before = session.doc.clone();
        let mut tools = ModelTools::default();
        tools.start_wall(&session.doc, 0);
        assert!(tools.wall_mode == WallMode::Footprint);
        for point in [[10000.0, 20000.0], [18000.0, 20000.0], [18000.0, 26000.0], [10000.0, 26000.0]] {
            assert!(tools.point(&session.doc, point).is_none());
            assert_eq!(session.doc, before);
        }
        let run = tools.finish(&session.doc).unwrap();
        assert_eq!(run.command, "building.add");
        assert_eq!(run.params["id"], "building-1");
        assert_eq!(run.params["footprint"].as_array().unwrap().len(), 4);
        session.execute(run.command, run.params).unwrap();
        tools.acknowledge(true, None);
        assert!(!tools.active());
        assert_eq!(session.doc.buildings[0].footprint[0], [10000.0, 20000.0]);
        session.execute("edit.undo", json!({})).unwrap();
        assert_eq!(session.doc, before);
    }

    #[test]
    fn crossing_footprint_rejection_keeps_every_corner_and_document_unchanged() {
        let mut session = Session::new(Box::new(Live::new(Credentials::default())));
        let mut tools = ModelTools::default();
        tools.start_wall(&session.doc, 0);
        tools.ortho = false;
        for point in [[0.0, 0.0], [10000.0, 0.0], [0.0, 8000.0], [8000.0, 8000.0], [8000.0, -2000.0]] {
            tools.point(&session.doc, point);
        }
        let corners = tools.points.clone();
        let before = session.doc.clone();
        let run = tools.finish(&session.doc).unwrap();
        let error = session.execute(run.command, run.params).unwrap_err().to_string();
        tools.acknowledge(false, Some(error));
        assert_eq!(session.doc, before);
        assert_eq!(tools.points, corners);
        assert!(tools.active() && tools.error().is_some());
    }

    #[test]
    fn gable_hip_and_flat_use_existing_building_boundary_and_exact_undo() {
        for form in [RoofForm::Gable, RoofForm::Hip, RoofForm::Flat] {
            let mut session = session();
            let before = session.doc.clone();
            let mut tools = ModelTools::default();
            tools.start_roof(&session.doc, 0);
            tools.roof_form = form;
            assert_eq!(tools.preview(None).len(), 4);
            assert_eq!(session.doc, before);
            let run = tools.finish(&session.doc).unwrap();
            assert_eq!(run.command, "building.set_roof");
            assert_eq!(run.params["building"], "wing");
            if form == RoofForm::Hip {
                assert_eq!(run.params["gables"], json!([]));
            }
            session.execute(run.command, run.params).unwrap();
            tools.acknowledge(true, None);
            assert!(!tools.active());
            assert_eq!(session.doc.buildings[0].footprint, before.buildings[0].footprint);
            assert_eq!(session.doc.buildings[0].storeys, before.buildings[0].storeys);
            session.execute("edit.undo", json!({})).unwrap();
            assert_eq!(session.doc, before);
        }
    }

    #[test]
    fn roof_picking_uses_site_coordinates_and_rejects_overlapping_scope() {
        let mut session = session();
        session.execute("building.add", json!({"id":"other","name":"Other","footprint":[[30000,30000],[38000,30000],[38000,36000],[30000,36000]],"storeys":1,"storey_height_mm":3000})).unwrap();
        let mut tools = ModelTools::default();
        tools.start_roof(&session.doc, 0);
        assert!(tools.point(&session.doc, [32000.0, 32000.0]).is_none());
        assert_eq!(tools.building, "other");
        assert_eq!(tools.finish(&session.doc).unwrap().params["building"], "other");
        tools.acknowledge(false, Some("retained".into()));
        let mut duplicate = session.doc.buildings[1].clone();
        duplicate.id = "overlap".into();
        session.doc.buildings.push(duplicate);
        assert!(tools.point(&session.doc, [32000.0, 32000.0]).is_none());
        assert!(tools.error().unwrap().contains("overlap"));
        assert_eq!(tools.building, "other");
    }

    #[test]
    fn ai_feedback_reports_actual_draft_scope_pending_and_failure_without_document_changes() {
        let session = session();
        let before = session.doc.clone();
        let mut tools = ModelTools::default();
        tools.start_wall(&session.doc, 1);
        tools.ortho = false;
        tools.point(&session.doc, [12000.0, 11000.0]);
        let run = tools.point(&session.doc, [15000.0, 13000.0]).unwrap();
        let feedback = tools.feedback();
        assert_eq!(feedback["tool"], "wall");
        assert_eq!(feedback["scope"], json!({"building":"wing","level":1}));
        assert_eq!(feedback["units"], "mm");
        assert_eq!(feedback["coordinate_system"], "site_local_xy_z_up");
        assert_eq!(feedback["draft_points"], json!([[12000.0, 11000.0], [15000.0, 13000.0]]));
        assert_eq!(feedback["pending_action"], run.command);
        assert_eq!(feedback["pending"], true);
        tools.acknowledge(false, Some("public validation error".into()));
        let retained = tools.feedback();
        assert_eq!(retained["draft_points"], feedback["draft_points"]);
        assert_eq!(retained["pending"], false);
        assert_eq!(retained["error"], "public validation error");
        assert_eq!(session.doc, before);
        tools.cancel();
        assert_eq!(tools.feedback()["active"], false);
        assert_eq!(tools.feedback()["draft_points"], json!([]));
    }

    #[test]
    fn collapsed_properties_context_switch_uses_the_new_plan_level_and_discards_old_start() {
        let session = session();
        let before = session.doc.clone();
        let mut tools = ModelTools::default();
        tools.start_wall(&session.doc, 0);
        tools.ortho = false;
        tools.point(&session.doc, [12000.0, 11000.0]);
        tools.sync_context(&session.doc, 1);
        assert!(tools.anchor().is_none());
        assert_eq!(tools.feedback()["scope"]["level"], 1);
        assert!(tools.point(&session.doc, [13000.0, 12000.0]).is_none());
        let run = tools.point(&session.doc, [15000.0, 13000.0]).unwrap();
        assert_eq!(run.params["walls"][0]["levels"], json!([1]));
        assert_eq!(run.params["walls"][0]["a"], json!([13000.0, 12000.0]));
        tools.acknowledge(false, Some("retain for context check".into()));
        let mut changed = session.doc.clone();
        changed.buildings[0].footprint[0][0] += 100.0;
        tools.sync_context(&changed, 1);
        assert!(tools.anchor().is_none());
        assert_eq!(tools.feedback()["draft_points"], json!([]));
        assert_eq!(session.doc, before);
    }
}
