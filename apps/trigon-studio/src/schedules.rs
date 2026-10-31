//! Opening inventory UI. Geometry and quantities arrive together from the build worker.

use eframe::egui;
use trigon_engine::{OpeningQuantityBasis, OpeningSchedule, OpeningScheduleRow, ScheduleOpeningKind};
use trigon_model::Document;

use crate::{scene::Scene, theme};

pub const AMBIGUOUS: &str = "This ID occurs more than once in this phase view. Use a complete view to select, edit or show it in 3D.";

#[derive(Default)]
pub struct Filters {
    pub search: String,
    pub building: Option<String>,
    /// Storey indices are local to a building, not global across the project.
    pub storey: Option<(String, usize)>,
    pub kind: Option<ScheduleOpeningKind>,
}

fn kind_name(kind: ScheduleOpeningKind) -> &'static str {
    match kind {
        ScheduleOpeningKind::Window => "Window",
        ScheduleOpeningKind::Door => "Door",
    }
}

fn basis_name(basis: OpeningQuantityBasis) -> &'static str {
    match basis {
        OpeningQuantityBasis::OuterLining => "Outer lining",
        OpeningQuantityBasis::FacadeOpening => "Facade opening",
    }
}

impl Filters {
    pub fn apply(&self, schedule: &OpeningSchedule) -> OpeningSchedule {
        let search = self.search.trim().to_lowercase();
        let rows = schedule
            .rows
            .iter()
            .filter(|row| {
                self.building.as_ref().is_none_or(|id| &row.building_id == id)
                    && self.storey.as_ref().is_none_or(|(id, level)| &row.building_id == id && row.level == *level)
                    && self.kind.is_none_or(|kind| row.kind == kind)
                    && (search.is_empty()
                        || format!(
                            "{} {} {} {} {} {} {} {} {} {}",
                            row.id,
                            row.building_id,
                            row.building_name,
                            row.level_name,
                            row.type_id,
                            row.name,
                            row.host_id,
                            row.status,
                            kind_name(row.kind),
                            basis_name(row.quantity_basis)
                        )
                        .to_lowercase()
                        .contains(&search))
            })
            .cloned()
            .collect();
        let mut filtered = OpeningSchedule { rows, ..Default::default() };
        // Keep quantity semantics and total recalculation in the shared engine API.
        filtered.retain(None, None);
        filtered
    }

    fn ui(&mut self, ui: &mut egui::Ui, doc: &Document) {
        ui.heading("Opening schedules");
        ui.add_space(8.0);
        ui.add(egui::TextEdit::singleline(&mut self.search).hint_text("Search openings…").desired_width(f32::INFINITY));
        theme::eyebrow(ui, "BUILDING");
        let label = self.building.as_ref().and_then(|id| doc.buildings.iter().find(|b| &b.id == id)).map_or("All buildings", |b| b.name.as_str());
        let before = self.building.clone();
        egui::ComboBox::from_id_salt("schedule-building").selected_text(label).show_ui(ui, |ui| {
            ui.selectable_value(&mut self.building, None, "All buildings");
            for b in &doc.buildings {
                ui.selectable_value(&mut self.building, Some(b.id.clone()), format!("{} · {}", b.name, b.id));
            }
        });
        if before != self.building {
            self.storey = None;
        }
        theme::eyebrow(ui, "STOREY");
        let label = self.storey.as_ref().and_then(|(id, level)| {
            let b = doc.buildings.iter().find(|b| &b.id == id)?;
            Some(format!("{} · {}", b.name, b.storeys.get(*level)?.name))
        });
        egui::ComboBox::from_id_salt("schedule-storey").selected_text(label.as_deref().unwrap_or("All storeys")).show_ui(ui, |ui| {
            ui.selectable_value(&mut self.storey, None, "All storeys");
            for b in doc.buildings.iter().filter(|b| self.building.as_ref().is_none_or(|id| &b.id == id)) {
                for (level, storey) in b.storeys.iter().enumerate() {
                    ui.selectable_value(&mut self.storey, Some((b.id.clone(), level)), format!("{} · {}", b.name, storey.name));
                }
            }
        });
        theme::eyebrow(ui, "KIND");
        ui.selectable_value(&mut self.kind, None, "Windows & doors");
        ui.selectable_value(&mut self.kind, Some(ScheduleOpeningKind::Window), "Windows");
        ui.selectable_value(&mut self.kind, Some(ScheduleOpeningKind::Door), "Doors");
        ui.add_space(10.0);
        if ui.button("Reset filters").clicked() {
            *self = Self::default();
        }
        ui.separator();
        ui.weak("Uses the phase and filter in the toolbar. Select an ID to edit it in Properties. Show in 3D opens the model view.");
    }
}

/// An id alone is not a safe row key. Refuse duplicates even if local search hides one of them.
pub fn resolve_row(scene: &Scene, row: &OpeningScheduleRow) -> Result<usize, &'static str> {
    if scene.ambiguous(&row.id) {
        return Err(AMBIGUOUS);
    }
    let index = scene.find_unique(&row.id).ok_or("This opening is unavailable in the current scene.")?;
    let e = scene.elements.get(index).ok_or("This opening is unavailable in the current scene.")?;
    let property = |key| e.props.iter().find(|(k, _)| *k == key).map(|(_, v)| v.as_str());
    if e.kind != kind_name(row.kind).to_lowercase()
        || e.level != Some(row.level)
        || property("building") != Some(&row.building_id)
        || property("type") != Some(&row.type_id)
        || property("phase status") != Some(&row.status)
        || property(if row.external { "facade" } else { "host" }) != Some(&row.host_id)
    {
        return Err("This row does not match the current scene. Wait for the current build.");
    }
    Ok(index)
}

#[derive(Default)]
pub struct Schedules {
    pub filters: Filters,
    wanted: u64,
    result: Option<Result<OpeningSchedule, String>>,
}

pub enum Action {
    Select(usize),
    Show(usize),
}

impl Schedules {
    pub fn begin(&mut self, generation: u64) {
        self.wanted = generation;
        self.result = None;
    }

    pub fn finish(&mut self, generation: u64, result: Result<OpeningSchedule, String>) -> bool {
        if generation < self.wanted {
            return false;
        }
        self.wanted = generation;
        self.result = Some(result);
        true
    }

    pub fn data(&self) -> Option<&OpeningSchedule> {
        self.result.as_ref().and_then(|r| r.as_ref().ok())
    }

    pub fn browser(&mut self, ui: &mut egui::Ui, doc: &Document) {
        self.filters.ui(ui, doc);
    }

    pub fn ui(&self, ui: &mut egui::Ui, scene: &Scene, selection: Option<usize>) -> Option<Action> {
        let schedule = match &self.result {
            None => {
                ui.horizontal(|ui| {
                    ui.spinner();
                    ui.weak("Updating opening schedules… Selection and editing will resume after the build.");
                });
                return None;
            }
            Some(Err(error)) => {
                ui.heading("Opening schedule unavailable");
                ui.label(error);
                ui.weak("Correct the model to rebuild the inventory.");
                return None;
            }
            Some(Ok(schedule)) => schedule,
        };
        let filtered = self.filters.apply(schedule);
        let totals = &filtered.totals;
        egui::Frame::new().fill(theme::PANEL).inner_margin(12).show(ui, |ui| {
            ui.set_min_width(ui.available_width());
            ui.heading("Windows & doors");
            ui.horizontal_wrapped(|ui| {
                ui.strong(format!("{} openings", totals.instances));
                ui.separator();
                ui.label(format!("{} windows · {} doors", totals.windows, totals.doors));
                ui.separator();
                ui.label(format!("{:.2} m² included", totals.area_m2));
                ui.label(format!("{} excluded from area", totals.area_excluded));
                ui.weak(format!("/ {} in phase view", schedule.totals.instances));
            });
            ui.weak("Area = outer lining width × height. Facade openings (loggias) have no lining area. Dimensions in mm.");
        });
        if filtered.rows.is_empty() {
            ui.add_space(12.0);
            ui.weak(if schedule.rows.is_empty() { "No windows or doors in this phase view." } else { "No openings match these filters." });
            return None;
        }
        if filtered.rows.iter().any(|row| scene.ambiguous(&row.id)) {
            ui.label(egui::RichText::new(AMBIGUOUS).color(theme::ACCENT));
        }
        let mut action = None;
        egui::ScrollArea::both().id_salt("schedule-table").auto_shrink([false, false]).show(ui, |ui| {
            egui::Grid::new("openings").striped(true).spacing([16.0, 6.0]).max_col_width(240.0).show(ui, |ui| {
                for label in [
                    "Element ID",
                    "Model",
                    "Building",
                    "Storey",
                    "Kind",
                    "Type / name",
                    "Status",
                    "Width / mm",
                    "Height / mm",
                    "Area / m²",
                    "Basis",
                    "Location",
                ] {
                    ui.strong(label);
                }
                ui.end_row();
                // Grid cells use sequential widget ids, independent of duplicate semantic ids.
                for row in &filtered.rows {
                    let target = resolve_row(scene, row);
                    let response = ui.add_enabled(target.is_ok(), egui::Button::selectable(target.is_ok_and(|i| selection == Some(i)), &row.id));
                    if let Err(reason) = target {
                        response.clone().on_disabled_hover_text(reason);
                    }
                    if response.clicked()
                        && let Ok(index) = target
                    {
                        action = Some(Action::Select(index));
                    }
                    let show = ui.add_enabled(target.is_ok(), egui::Button::new("Show in 3D"));
                    if let Err(reason) = target {
                        show.clone().on_disabled_hover_text(reason);
                    }
                    if show.clicked()
                        && let Ok(index) = target
                    {
                        action = Some(Action::Show(index));
                    }
                    ui.label(&row.building_name).on_hover_text(&row.building_id);
                    ui.label(&row.level_name);
                    ui.label(kind_name(row.kind));
                    ui.vertical(|ui| {
                        ui.label(&row.type_id);
                        ui.weak(&row.name);
                    });
                    ui.label(&row.status);
                    ui.label(format!("{:.1}", row.width_mm));
                    ui.label(format!("{:.1}", row.height_mm));
                    ui.label(row.area_m2.map_or_else(|| "Excluded".into(), |a| format!("{a:.2}")));
                    ui.label(basis_name(row.quantity_basis));
                    ui.label(if row.external { "Exterior" } else { "Interior" }).on_hover_text(&row.host_id);
                    ui.end_row();
                }
            });
        });
        action
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use serde_json::json;
    use trigon_engine::Session;
    use trigon_model::build::{PhaseFilter, View, build_view};

    pub fn session() -> Session {
        let mut s = Session::new(Box::new(trigon_data::Live::new(trigon_data::Credentials::default())));
        s.doc.buildings.push(
            trigon_model::Building::new("b", "Main building", &[[0.0, 0.0], [18000.0, 0.0], [18000.0, 10000.0], [0.0, 10000.0]], 2, 3000.0).unwrap(),
        );
        for (command, params) in [
            ("facade.set_bays", json!({"facade":"b.f1","count":3})),
            ("facade.place", json!({"facade":"b.f1","type":"window.casement.2","levels":[0],"bays":[0],"width":1200,"height":1800})),
            ("facade.place", json!({"facade":"b.f1","type":"door.french","levels":[1],"bays":[1],"width":900,"height":2100})),
            (
                "types.define",
                json!({"id":"loggia.test","name":"Recessed loggia","width":1500,"height":2100,"sill":0,"kind":{"family":"loggia","depth":1000,"rail_height":1000,"door_width":800,"door_side":"left"}}),
            ),
            ("facade.place", json!({"facade":"b.f1","type":"loggia.test","levels":[0],"bays":[2]})),
            (
                "interior.add_walls",
                json!({"walls":[{"id":"partition","a":[2000,4000],"b":[12000,4000],"thickness":150,"levels":[0,1],"doors":[{"at":3000,"width":900,"height":2100}]}]}),
            ),
        ] {
            s.execute(command, params).unwrap();
        }
        s
    }

    pub fn phase_session() -> Session {
        let mut s = session();
        s.doc.phases.push(trigon_model::doc::Phase { id: "proposal".into(), name: "Proposal".into() });
        s.doc.active_phase = Some("proposal".into());
        let original = &mut s.doc.buildings[0].facades[0].placements[0];
        original.demolished = Some("proposal".into());
        let mut replacement = original.clone();
        replacement.created = Some("proposal".into());
        replacement.demolished = None;
        s.doc.buildings[0].facades[0].placements.push(replacement);
        s
    }

    #[test]
    fn filters_recompute_counts_included_area_and_exclusions_without_conflating_storeys() {
        let mut s = session();
        s.doc.buildings.push(
            trigon_model::Building::new("annex", "Annex", &[[30000.0, 0.0], [40000.0, 0.0], [40000.0, 6000.0], [30000.0, 6000.0]], 1, 3000.0)
                .unwrap(),
        );
        s.execute("facade.set_bays", json!({"facade":"annex.f1","count":1})).unwrap();
        s.execute("facade.place", json!({"facade":"annex.f1","type":"window.casement.1","levels":[0],"width":1000,"height":1000})).unwrap();
        let built = build_view(&s.doc, View::latest(&s.doc)).unwrap();
        let schedule = trigon_engine::opening_schedule_from_built(&s.doc, &built).unwrap();
        let original = schedule.clone();
        let mut filters = Filters::default();
        let all = filters.apply(&schedule);
        assert_eq!((all.totals.instances, all.totals.windows, all.totals.doors, all.totals.area_excluded), (6, 2, 4, 1));
        assert!((all.totals.area_m2 - 8.83).abs() < 1e-9);
        filters.building = Some("b".into());
        filters.storey = Some(("b".into(), 0));
        let ground = filters.apply(&schedule);
        assert_eq!((ground.totals.instances, ground.totals.windows, ground.totals.doors, ground.totals.area_excluded), (3, 1, 2, 1));
        assert!((ground.totals.area_m2 - 4.05).abs() < 1e-9);
        filters.kind = Some(ScheduleOpeningKind::Door);
        let doors = filters.apply(&schedule);
        assert_eq!((doors.totals.instances, doors.totals.windows, doors.totals.doors, doors.totals.area_excluded), (2, 0, 2, 1));
        assert!((doors.totals.area_m2 - 1.89).abs() < 1e-9);
        filters.search = "  LOGGIA.TEST  ".into();
        let loggia = filters.apply(&schedule);
        assert_eq!(loggia.rows.len(), 1);
        assert_eq!(loggia.rows[0].quantity_basis, OpeningQuantityBasis::FacadeOpening);
        assert_eq!(loggia.totals.area_m2, 0.0);
        assert_eq!(loggia.totals.area_excluded, 1);
        filters.search = "no match".into();
        assert_eq!(filters.apply(&schedule), OpeningSchedule::default());
        filters = Filters { storey: Some(("annex".into(), 0)), ..Default::default() };
        assert_eq!(filters.apply(&schedule).rows[0].building_id, "annex");
        assert_eq!(filters.apply(&schedule).totals.instances, 1);
        assert_eq!(schedule, original, "filtering must not mutate the worker's inventory");
    }

    #[test]
    fn every_unambiguous_row_resolves_to_the_same_scene_occurrence_and_checks_its_identity() {
        let s = session();
        let built = build_view(&s.doc, View::latest(&s.doc)).unwrap();
        let scene = Scene::from_built(&built, false);
        let schedule = trigon_engine::opening_schedule_from_built(&s.doc, &built).unwrap();
        for row in &schedule.rows {
            let index = resolve_row(&scene, row).unwrap();
            assert_eq!(Some(index), scene.find_unique(&row.id));
            let e = &scene.elements[index];
            assert_eq!(e.id, row.id);
            assert_eq!(e.level, Some(row.level));
            assert_eq!(e.kind, kind_name(row.kind).to_lowercase());
            for key in ["building", "type", "phase status", "host", "level", "kind"] {
                let mut mismatch = row.clone();
                match key {
                    "building" => mismatch.building_id.push_str("wrong"),
                    "type" => mismatch.type_id.push_str("wrong"),
                    "phase status" => mismatch.status = "wrong".into(),
                    "host" => mismatch.host_id.push_str("wrong"),
                    "level" => mismatch.level += 1,
                    _ => mismatch.kind = if row.kind == ScheduleOpeningKind::Door { ScheduleOpeningKind::Window } else { ScheduleOpeningKind::Door },
                }
                assert!(resolve_row(&scene, &mismatch).is_err(), "identity mismatch: {key}");
            }
        }
    }

    #[test]
    fn same_id_and_type_with_different_status_stays_distinct_and_cannot_select_even_when_search_hides_one() {
        for filter in [PhaseFilter::All, PhaseFilter::Demolition] {
            let mut s = phase_session();
            if filter == PhaseFilter::Demolition {
                // A retained existing occurrence can share the demolished fill's semantic id too.
                let retained = s.doc.buildings[0].facades[0].placements.last_mut().unwrap();
                retained.created = None;
                retained.u = Some(5000.0);
            }
            let built = build_view(&s.doc, View { phase: 1, filter }).unwrap();
            let scene = Scene::from_built(&built, true);
            let schedule = trigon_engine::opening_schedule_from_built(&s.doc, &built).unwrap();
            let duplicates: Vec<_> = schedule.rows.iter().filter(|r| r.id == "b.f1.L0.B0").collect();
            assert_eq!(duplicates.len(), 2);
            assert_eq!(duplicates[0].type_id, duplicates[1].type_id);
            assert_ne!(duplicates[0].status, duplicates[1].status);
            assert!(scene.find_unique("b.f1.L0.B0").is_none());
            for row in duplicates {
                assert_eq!(resolve_row(&scene, row), Err(AMBIGUOUS));
            }
            let filtered = Filters { search: "demolished".into(), ..Default::default() }.apply(&schedule);
            assert_eq!(filtered.rows.len(), 1);
            assert_eq!(resolve_row(&scene, &filtered.rows[0]), Err(AMBIGUOUS));
            assert_eq!(schedule.rows.len(), 6, "no id deduplication");
        }
    }

    #[test]
    fn stale_successes_and_errors_never_restore_an_invalidated_schedule() {
        let mut schedules = Schedules::default();
        schedules.begin(1);
        assert!(schedules.finish(1, Ok(OpeningSchedule::default())));
        assert!(schedules.data().is_some());
        schedules.begin(3);
        assert!(schedules.data().is_none());
        assert!(!schedules.finish(2, Ok(OpeningSchedule::default())));
        assert!(!schedules.finish(1, Err("stale error".into())));
        assert!(schedules.data().is_none());
        assert!(schedules.finish(3, Err("current error".into())));
        assert!(schedules.data().is_none());
        schedules.begin(4);
        assert!(schedules.finish(5, Ok(OpeningSchedule::default())), "coalesced plan/model jobs may have a newer generation");
        assert!(!schedules.finish(4, Err("late error".into())));
        assert!(schedules.data().is_some());
    }

    #[test]
    fn headless_table_keeps_occurrences_in_aligned_rows_and_reports_pending_and_errors() {
        fn labels(shapes: &[egui::Shape]) -> Vec<(String, egui::Pos2)> {
            shapes
                .iter()
                .flat_map(|s| match s {
                    egui::Shape::Text(t) => vec![(t.galley.text().to_string(), t.pos)],
                    egui::Shape::Vec(shapes) => labels(shapes),
                    _ => Vec::new(),
                })
                .collect()
        }
        let s = phase_session();
        let built = build_view(&s.doc, View { phase: 1, filter: PhaseFilter::All }).unwrap();
        let scene = Scene::from_built(&built, true);
        let mut schedules = Schedules::default();
        schedules.finish(1, trigon_engine::opening_schedule_from_built(&s.doc, &built).map_err(|e| e.to_string()));
        let ctx = egui::Context::default();
        theme::apply(&ctx);
        let draw = |schedules: &Schedules| {
            let mut output = ctx.run_ui(
                egui::RawInput { screen_rect: Some(egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(2400.0, 900.0))), ..Default::default() },
                |ui| {
                    egui::CentralPanel::default().show(ui, |ui| {
                        schedules.ui(ui, &scene, None);
                    });
                },
            );
            output.textures_delta.clear(); // This test inspects layout without uploading textures.
            labels(&output.shapes.into_iter().map(|s| s.shape).collect::<Vec<_>>())
        };
        let table = draw(&schedules);
        let occurrences: Vec<_> = table.iter().filter(|(text, _)| text == "b.f1.L0.B0").collect();
        assert_eq!(occurrences.len(), 2, "both same-id phase rows are shown");
        assert!((occurrences[0].1.x - occurrences[1].1.x).abs() < 1.0, "occurrence IDs share one column");
        assert!(occurrences[1].1.y > occurrences[0].1.y + 20.0, "each occurrence occupies its own row");
        assert!(table.iter().any(|(text, _)| text == "Excluded"));
        assert!(table.iter().any(|(text, _)| text == AMBIGUOUS));
        schedules.begin(2);
        let pending = draw(&schedules);
        assert!(pending.iter().any(|(text, _)| text.starts_with("Updating opening schedules")));
        assert!(!pending.iter().any(|(text, _)| text == "b.f1.L0.B0"));
        schedules.finish(2, Err("test model error".into()));
        let error = draw(&schedules);
        assert!(error.iter().any(|(text, _)| text == "test model error"));
        assert!(!error.iter().any(|(text, _)| text == "b.f1.L0.B0"));
    }
}
