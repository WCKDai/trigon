//! Terrain inspection and explicit import requests. The engine owns execution and Undo.

use eframe::egui;
use serde_json::json;
use trigon_model::{Document, terrain::Terrain};

use crate::{authoring::Run, theme};

/// Bound pasted data before deserializing, including UTF-8 provenance strings.
const MAX_JSON_CHARS: usize = 131_072;
const MAX_JSON_BYTES: usize = 262_144;

#[derive(Clone, Debug, PartialEq)]
struct Sampling {
    origin: [f64; 2],
    spacing_mm: [f64; 2],
    columns: u32,
    rows: u32,
}

impl Default for Sampling {
    fn default() -> Self {
        Self { origin: [0.0, 0.0], spacing_mm: [5000.0, 5000.0], columns: 17, rows: 17 }
    }
}

impl Sampling {
    fn validate(&self) -> Result<(), String> {
        if !(2..=65).contains(&self.columns) || !(2..=65).contains(&self.rows) {
            return Err("Choose 2–65 rows and columns.".into());
        }
        for axis in 0..2 {
            trigon_model::checked("terrain origin", self.origin[axis], -1e8, 1e8).map_err(|error| error.to_string())?;
            trigon_model::checked("terrain spacing", self.spacing_mm[axis], 100.0, 200_000.0).map_err(|error| error.to_string())?;
            let count = if axis == 0 { self.columns } else { self.rows };
            trigon_model::checked("terrain extent", self.origin[axis] + self.spacing_mm[axis] * f64::from(count - 1), -1e8, 1e8)
                .map_err(|error| error.to_string())?;
        }
        Ok(())
    }

    fn request(&self, doc: &Document) -> Result<Run, String> {
        self.validate()?;
        let site = doc.site.as_ref().ok_or("Set a site address before importing DHM terrain.")?;
        if !site.elevation.is_finite() || site.origin_utm.iter().any(|coordinate| !coordinate.is_finite()) {
            return Err("Review the invalid site coordinates or datum before importing DHM terrain.".into());
        }
        Ok(Run {
            command: "site.sample_terrain",
            params: json!({"origin":self.origin,"spacing_mm":self.spacing_mm,"columns":self.columns,"rows":self.rows}),
        })
    }

    fn use_grid(&mut self, grid: &Terrain) -> Result<(), String> {
        let readout = Readout::new(grid)?;
        self.origin = grid.origin;
        self.spacing_mm = grid.spacing_mm;
        self.columns = readout.columns as u32;
        self.rows = readout.rows as u32;
        Ok(())
    }
}

#[derive(Debug, PartialEq)]
struct Readout {
    rows: usize,
    columns: usize,
    /// West, south, east, north in site-local mm.
    extent: [f64; 4],
    heights: [f64; 2],
}

impl Readout {
    fn new(grid: &Terrain) -> Result<Self, String> {
        grid.validate().map_err(|error| error.to_string())?;
        let rows = grid.heights_mm.len();
        let columns = grid.heights_mm.first().map_or(0, Vec::len);
        let extent = [
            grid.origin[0],
            grid.origin[1],
            grid.origin[0] + grid.spacing_mm[0] * (columns - 1) as f64,
            grid.origin[1] + grid.spacing_mm[1] * (rows - 1) as f64,
        ];
        let heights =
            grid.heights_mm.iter().flatten().fold([f64::INFINITY, f64::NEG_INFINITY], |range, height| [range[0].min(*height), range[1].max(*height)]);
        Ok(Self { rows, columns, extent, heights })
    }
}

fn local_grid(input: &str) -> Result<Terrain, String> {
    if input.len() > MAX_JSON_BYTES || input.chars().take(MAX_JSON_CHARS + 1).count() > MAX_JSON_CHARS {
        return Err("This height grid is too large to paste. Use at most 131,072 characters and 262,144 UTF-8 bytes.".into());
    }
    if input.trim().is_empty() {
        return Err("Paste a Terrain JSON object first.".into());
    }
    let terrain: Terrain = serde_json::from_str(input).map_err(|error| format!("Invalid Terrain JSON: {error}"))?;
    terrain.validate().map_err(|error| error.to_string())?;
    Ok(terrain)
}

fn local_request(input: &str) -> Result<Run, String> {
    let terrain = local_grid(input)?;
    Ok(Run { command: "site.set_terrain", params: json!({"terrain":terrain}) })
}

fn remove_request(doc: &Document) -> Option<Run> {
    doc.terrain.as_ref().map(|_| Run { command: "site.set_terrain", params: json!({"terrain":null}) })
}

fn datum(doc: &Document) -> String {
    match &doc.site {
        Some(site) if site.elevation.is_finite() => format!("Local Z 0 = {:.3} m DVR90", site.elevation),
        Some(_) => "Site datum is invalid — review the site first".into(),
        None => "Local Z 0 · no registered geographic datum".into(),
    }
}

fn mm_field(ui: &mut egui::Ui, label: &str, value: &mut f64, range: std::ops::RangeInclusive<f64>) {
    ui.label(label);
    ui.add_sized([116.0, 28.0], egui::DragValue::new(value).speed(100.0).range(range).max_decimals(3));
    ui.end_row();
}

#[derive(Default)]
pub struct TerrainPanel {
    sampling: Sampling,
    local_json: String,
    /// Cached on editing, not reparsed on every frame. Import revalidates before emitting a run.
    local_validation: Option<Result<Terrain, String>>,
}

impl TerrainPanel {
    /// Returns at most one explicit user request. Drawing this form never changes the document.
    pub fn ui(&mut self, ui: &mut egui::Ui, doc: &Document) -> Option<Run> {
        let mut run = None;
        theme::eyebrow(ui, "SITE & TERRAIN");
        ui.heading("Terrain");
        if let Some(site) = &doc.site {
            ui.label(egui::RichText::new(&site.name).strong());
        }
        ui.label(egui::RichText::new(datum(doc)).small().color(theme::MUTED));
        ui.add_space(6.0);
        egui::Frame::new().fill(theme::CHROME).corner_radius(4).inner_margin(10).show(ui, |ui| match &doc.terrain {
            Some(grid) => match Readout::new(grid) {
                Ok(info) => {
                    ui.label(egui::RichText::new(format!("{} × {} samples", info.columns, info.rows)).strong());
                    ui.label(format!(
                        "{:.1} × {:.1} m coverage",
                        (info.extent[2] - info.extent[0]) / 1000.0,
                        (info.extent[3] - info.extent[1]) / 1000.0
                    ));
                    ui.small(format!("Spacing X/Y: {:.3} / {:.3} mm", grid.spacing_mm[0], grid.spacing_mm[1]));
                    ui.small(format!("Height: {:+.3} to {:+.3} mm from local Z 0", info.heights[0], info.heights[1]));
                }
                Err(error) => {
                    ui.colored_label(theme::ACCENT, "Stored terrain needs review");
                    ui.label(error);
                }
            },
            None => {
                ui.label("No terrain in this project");
                ui.weak("Import observed terrain or a measured local grid.");
            }
        });
        ui.add_space(6.0);
        egui::CollapsingHeader::new("Source & coordinate details").id_salt("terrain-source-details").show(ui, |ui| {
            if let Some(grid) = &doc.terrain {
                ui.label(egui::RichText::new("Source").strong());
                ui.label(grid.source.as_deref().filter(|text| !text.is_empty()).unwrap_or("Not supplied"));
                ui.label(egui::RichText::new("Attribution").strong());
                ui.label(grid.attribution.as_deref().filter(|text| !text.is_empty()).unwrap_or("Not supplied"));
                if let Ok(info) = Readout::new(grid) {
                    ui.small(format!("West / east: {:.3} / {:.3} mm", info.extent[0], info.extent[2]));
                    ui.small(format!("South / north: {:.3} / {:.3} mm", info.extent[1], info.extent[3]));
                }
            } else {
                ui.weak("Source and attribution will come from the imported data.");
            }
            if let Some(site) = &doc.site {
                ui.small(format!("EPSG:25832 site offset: {:.3}, {:.3} m", site.origin_utm[0], site.origin_utm[1]));
            }
            ui.weak("Grid rows run south to north; columns west to east. Coordinates and heights inside the project use local millimetres.");
        });
        egui::CollapsingHeader::new("DHM sampling").id_salt("terrain-sampling").default_open(doc.terrain.is_none()).show(ui, |ui| {
            ui.weak("Choose the south-west sample and grid area in local millimetres.");
            egui::Grid::new("terrain-sampling-fields").num_columns(2).spacing([10.0, 7.0]).show(ui, |ui| {
                mm_field(ui, "Origin X (mm)", &mut self.sampling.origin[0], -1e8..=1e8);
                mm_field(ui, "Origin Y (mm)", &mut self.sampling.origin[1], -1e8..=1e8);
                mm_field(ui, "Spacing X (mm)", &mut self.sampling.spacing_mm[0], 100.0..=200_000.0);
                mm_field(ui, "Spacing Y (mm)", &mut self.sampling.spacing_mm[1], 100.0..=200_000.0);
                ui.label("Columns");
                ui.add(egui::DragValue::new(&mut self.sampling.columns).range(2..=65));
                ui.end_row();
                ui.label("Rows");
                ui.add(egui::DragValue::new(&mut self.sampling.rows).range(2..=65));
                ui.end_row();
            });
            if self.sampling.validate().is_ok() {
                ui.small(format!("{:.1} × {:.1} m · {} samples", self.sampling.spacing_mm[0] * f64::from(self.sampling.columns - 1) / 1000.0,
                    self.sampling.spacing_mm[1] * f64::from(self.sampling.rows - 1) / 1000.0, self.sampling.columns * self.sampling.rows));
            }
            if let Some(grid) = &doc.terrain
                && grid.validate().is_ok()
                && ui.button("Use current grid area").clicked()
            {
                // Only populates the draft. No replacement, provider call or document edit.
                let _ = self.sampling.use_grid(grid);
            }
            let request = self.sampling.request(doc);
            let enabled = request.is_ok();
            if let Err(error) = &request {
                ui.weak(error);
            }
            if ui.add_enabled_ui(enabled, |ui| theme::primary(ui, "Import DHM terrain")).inner.clicked() {
                run = request.ok();
            }
            ui.weak("Uses configured Dataforsyningen access when you click Import. Sampling is observed coarse context, not surveyed grade or construction setting-out.");
        });
        egui::CollapsingHeader::new("Import a local height grid").id_salt("terrain-local-grid").show(ui, |ui| {
            ui.weak("Paste a Terrain JSON object. Heights are already relative to local Z 0; source and attribution are retained.");
            let changed = ui
                .add(
                    egui::TextEdit::multiline(&mut self.local_json)
                        .code_editor()
                        .desired_width(f32::INFINITY)
                        .desired_rows(7)
                        .char_limit(MAX_JSON_CHARS)
                        .hint_text("Terrain JSON · origin, spacing_mm, heights_mm…"),
                )
                .changed();
            if changed || (self.local_validation.is_none() && !self.local_json.trim().is_empty()) {
                self.local_validation = Some(local_grid(&self.local_json));
            }
            match &self.local_validation {
                Some(Ok(grid)) => {
                    if let Ok(info) = Readout::new(grid) {
                        ui.small(format!("Ready · {} × {} samples · supplied provenance retained", info.columns, info.rows));
                    }
                }
                Some(Err(error)) if !self.local_json.trim().is_empty() => {
                    ui.colored_label(theme::ACCENT, error);
                }
                _ => {}
            }
            let enabled = !self.local_json.trim().is_empty() && self.local_validation.as_ref().is_some_and(Result::is_ok);
            if ui.add_enabled(enabled, egui::Button::new("Import local grid")).clicked() {
                match local_request(&self.local_json) {
                    Ok(request) => run = Some(request),
                    Err(error) => self.local_validation = Some(Err(error)),
                }
            }
        });
        if doc.terrain.is_some() {
            ui.add_space(6.0);
            if ui.button("Remove terrain").on_hover_text("Removes the terrain grid through an undoable project command.").clicked() {
                run = remove_request(doc);
            }
        }
        run
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn site_doc() -> Document {
        Document {
            site: Some(trigon_model::Site {
                name: "Public terrain site".into(),
                origin: trigon_model::GeoPoint { lat: 55.0, lon: 12.0 },
                origin_utm: [700000.0, 6100000.0],
                elevation: 12.345,
                evidence: vec![],
            }),
            ..Default::default()
        }
    }

    fn terrain_json() -> String {
        json!({"origin":[-1200,400],"spacing_mm":[2000,3000],"heights_mm":[[-25,0,80],[100,180,210]],
            "source":"public-survey:control-grid","attribution":"Synthetic survey · ÆØÅ"})
        .to_string()
    }

    #[test]
    fn sampling_emits_exact_local_units_without_datum_or_map_coordinate_rebasing() {
        let doc = site_doc();
        let before = doc.clone();
        let draft = Sampling { origin: [-2500.125, 800.75], spacing_mm: [1250.0, 2000.0], columns: 13, rows: 9 };
        let request = draft.request(&doc).unwrap();
        assert_eq!(request.command, "site.sample_terrain");
        assert_eq!(request.params, json!({"origin":[-2500.125,800.75],"spacing_mm":[1250.0,2000.0],"columns":13,"rows":9}));
        assert_eq!(doc, before);
        assert_eq!(datum(&doc), "Local Z 0 = 12.345 m DVR90");
    }

    #[test]
    fn sampling_requires_a_site_and_rejects_invalid_grid_or_site_data() {
        assert!(Sampling::default().request(&Document::default()).is_err());
        let doc = site_doc();
        for count in [0, 1, 66, u32::MAX] {
            let draft = Sampling { rows: count, ..Default::default() };
            assert!(draft.request(&doc).is_err());
            let draft = Sampling { columns: count, ..Default::default() };
            assert!(draft.request(&doc).is_err());
        }
        for value in [f64::NAN, f64::INFINITY, f64::NEG_INFINITY, -1e300, 1e300] {
            for axis in 0..2 {
                let mut draft = Sampling::default();
                draft.origin[axis] = value;
                assert!(draft.request(&doc).is_err());
                draft = Sampling::default();
                draft.spacing_mm[axis] = value;
                assert!(draft.request(&doc).is_err());
            }
        }
        for value in [0.0, 99.999, 200000.001] {
            let mut draft = Sampling::default();
            draft.spacing_mm[0] = value;
            assert!(draft.request(&doc).is_err());
        }
        let mut outside = Sampling::default();
        outside.origin[0] = 1e8;
        assert!(outside.request(&doc).is_err(), "refuse an invalid extent before requesting provider data");
        let mut invalid_site = doc.clone();
        invalid_site.site.as_mut().unwrap().elevation = f64::NAN;
        assert!(Sampling::default().request(&invalid_site).is_err());
        invalid_site = doc;
        invalid_site.site.as_mut().unwrap().origin_utm[1] = f64::INFINITY;
        assert!(Sampling::default().request(&invalid_site).is_err());
    }

    #[test]
    fn local_import_preserves_height_values_units_and_provenance_exactly() {
        let raw = terrain_json();
        let expected: Terrain = serde_json::from_str(&raw).unwrap();
        let request = local_request(&raw).unwrap();
        assert_eq!(request.command, "site.set_terrain");
        let actual: Terrain = serde_json::from_value(request.params["terrain"].clone()).unwrap();
        assert_eq!(actual, expected);
        assert_eq!(actual.heights_mm[0][0], -25.0);
        assert_eq!(actual.source.as_deref(), Some("public-survey:control-grid"));
        assert_eq!(actual.attribution.as_deref(), Some("Synthetic survey · ÆØÅ"));
        let unknown: Terrain = serde_json::from_value(json!({"origin":[0,0],"spacing_mm":[100,100],"heights_mm":[[0,0],[0,0]]})).unwrap();
        let request = local_request(&serde_json::to_string(&unknown).unwrap()).unwrap();
        let actual: Terrain = serde_json::from_value(request.params["terrain"].clone()).unwrap();
        assert_eq!(actual.source, None);
        assert_eq!(actual.attribution, None);
    }

    #[test]
    fn local_grid_parser_reuses_strict_schema_geometry_and_size_limits() {
        for raw in ["", "null", "{}", "{broken}", "{\"terrain\":null}"] {
            assert!(local_request(raw).is_err());
        }
        let mut value: serde_json::Value = serde_json::from_str(&terrain_json()).unwrap();
        value["unknown_field"] = json!(true);
        assert!(local_request(&value.to_string()).is_err());
        value.as_object_mut().unwrap().remove("unknown_field");
        value["heights_mm"] = json!([[1, 2], [3]]);
        assert!(local_request(&value.to_string()).is_err());
        value["heights_mm"] = json!(vec![vec![0.0; 2]; 66]);
        assert!(local_request(&value.to_string()).is_err());
        value["heights_mm"] = json!([[0, 0], [0, 1000001]]);
        assert!(local_request(&value.to_string()).is_err());
        value["heights_mm"] = json!([[0, 0], [0, 0]]);
        value["source"] = json!("x".repeat(2001));
        assert!(local_request(&value.to_string()).is_err());
        assert!(local_request(&" ".repeat(MAX_JSON_CHARS + 1)).is_err());
        assert!(local_request(&"🧭".repeat(MAX_JSON_BYTES / 4 + 1)).is_err());
    }

    #[test]
    fn terrain_readout_uses_sample_extent_min_max_and_existing_grid_area() {
        let terrain = local_grid(&terrain_json()).unwrap();
        assert_eq!(
            Readout::new(&terrain).unwrap(),
            Readout { rows: 2, columns: 3, extent: [-1200.0, 400.0, 2800.0, 3400.0], heights: [-25.0, 210.0] }
        );
        let mut draft = Sampling::default();
        draft.use_grid(&terrain).unwrap();
        assert_eq!(draft, Sampling { origin: [-1200.0, 400.0], spacing_mm: [2000.0, 3000.0], columns: 3, rows: 2 });
        let mut invalid = terrain;
        invalid.heights_mm.clear();
        let before = draft.clone();
        assert!(draft.use_grid(&invalid).is_err());
        assert_eq!(draft, before, "invalid stored grids must not partially replace the form");
    }

    #[test]
    fn removal_is_an_explicit_undoable_command_and_does_not_touch_site_datum() {
        let mut doc = site_doc();
        assert!(remove_request(&doc).is_none());
        doc.terrain = Some(local_grid(&terrain_json()).unwrap());
        let before = doc.clone();
        let request = remove_request(&doc).unwrap();
        assert_eq!(request.command, "site.set_terrain");
        assert_eq!(request.params, json!({"terrain":null}));
        assert_eq!(doc, before);
        assert_eq!(datum(&Document::default()), "Local Z 0 · no registered geographic datum");
    }

    #[test]
    fn opening_or_repainting_the_panel_never_emits_an_import_or_mutates_the_document() {
        let mut doc = site_doc();
        let mut panel = TerrainPanel { local_json: terrain_json(), ..Default::default() };
        let before = doc.clone();
        let ctx = egui::Context::default();
        theme::apply(&ctx);
        for present in [false, true] {
            if present {
                doc.terrain = Some(local_grid(&terrain_json()).unwrap());
            }
            let snapshot = doc.clone();
            for _ in 0..2 {
                let mut request = None;
                let mut output = ctx.run_ui(
                    egui::RawInput {
                        screen_rect: Some(egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(340.0, 1600.0))),
                        ..Default::default()
                    },
                    |ui| {
                        egui::CentralPanel::default().show(ui, |ui| {
                            request = panel.ui(ui, &doc);
                        });
                    },
                );
                output.textures_delta.clear();
                assert!(request.is_none());
                assert!(!output.shapes.is_empty());
                assert_eq!(doc, snapshot);
            }
        }
        assert_eq!(doc.site, before.site);
    }
    fn panel_frame(ctx: &egui::Context, panel: &mut TerrainPanel, doc: &Document, events: Vec<egui::Event>) -> (Option<Run>, Vec<egui::Shape>) {
        let mut request = None;
        let mut output = ctx.run_ui(
            egui::RawInput {
                screen_rect: Some(egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(340.0, 1600.0))),
                events,
                ..Default::default()
            },
            |ui| {
                egui::CentralPanel::default().show(ui, |ui| {
                    request = panel.ui(ui, doc);
                });
            },
        );
        output.textures_delta.clear();
        (request, output.shapes.into_iter().map(|shape| shape.shape).collect())
    }

    fn label_rect(shapes: &[egui::Shape], label: &str) -> Option<egui::Rect> {
        shapes.iter().find_map(|shape| match shape {
            egui::Shape::Text(text) if text.galley.text() == label => Some(egui::Rect::from_min_size(text.pos, text.galley.size())),
            egui::Shape::Vec(shapes) => label_rect(shapes, label),
            _ => None,
        })
    }

    #[test]
    fn dhm_button_emits_only_on_explicit_click_and_retains_exact_fields() {
        let doc = site_doc();
        let before = doc.clone();
        let mut panel = TerrainPanel {
            sampling: Sampling { origin: [-2500.125, 800.75], spacing_mm: [1250.0, 2000.0], columns: 13, rows: 9 },
            ..Default::default()
        };
        let ctx = egui::Context::default();
        theme::apply(&ctx);
        let (request, shapes) = panel_frame(&ctx, &mut panel, &doc, vec![]);
        assert!(request.is_none());
        let action = label_rect(&shapes, "Import DHM terrain").unwrap();
        assert!(action.left() >= 0.0 && action.right() <= 340.0, "the action must remain inside a narrow properties column");
        let position = action.center();
        let (request, _) = panel_frame(
            &ctx,
            &mut panel,
            &doc,
            vec![
                egui::Event::PointerMoved(position),
                egui::Event::PointerButton { pos: position, button: egui::PointerButton::Primary, pressed: true, modifiers: egui::Modifiers::NONE },
            ],
        );
        assert!(request.is_none(), "pressing does not import before the completed click");
        let (request, _) = panel_frame(
            &ctx,
            &mut panel,
            &doc,
            vec![egui::Event::PointerButton {
                pos: position,
                button: egui::PointerButton::Primary,
                pressed: false,
                modifiers: egui::Modifiers::NONE,
            }],
        );
        let request = request.unwrap();
        assert_eq!(request.command, "site.sample_terrain");
        assert_eq!(request.params, json!({"origin":[-2500.125,800.75],"spacing_mm":[1250.0,2000.0],"columns":13,"rows":9}));
        assert!(panel_frame(&ctx, &mut panel, &doc, vec![]).0.is_none(), "later frames do not repeat the import");
        assert_eq!(doc, before);
    }
    #[test]
    fn sampling_keeps_the_existing_minimum_and_maximum_grid_limits_usable() {
        let doc = site_doc();
        for count in [2, 65] {
            let draft = Sampling { origin: [-1e8, -1e8], spacing_mm: [100.0, 200_000.0], columns: count, rows: count };
            assert!(draft.request(&doc).is_ok());
        }
        let edge = Sampling { origin: [1e8 - 100.0, 1e8 - 100.0], spacing_mm: [100.0, 100.0], columns: 2, rows: 2 };
        assert!(edge.request(&doc).is_ok(), "a valid final sample at the model bound remains allowed");
    }
}
