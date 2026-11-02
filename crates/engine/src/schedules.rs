//! Opening schedules from the same built occurrences used by Studio and IFC.

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use trigon_model::build::{Built, BuiltElement, ElementClass, View, build_view};
use trigon_model::{ComponentKind, Document, ModelError};

use crate::{Result, Session};

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ScheduleOpeningKind {
    Window,
    Door,
}

/// What the dimensions describe. A loggia's facade hole is not its recessed window/door lining.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum OpeningQuantityBasis {
    OuterLining,
    FacadeOpening,
}

#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct OpeningScheduleRow {
    /// The existing built fill ID, including the storey. Phase replacements can share this ID.
    pub id: String,
    pub building_id: String,
    pub building_name: String,
    pub host_id: String,
    pub level: usize,
    pub level_name: String,
    pub kind: ScheduleOpeningKind,
    pub type_id: String,
    pub name: String,
    pub width_mm: f64,
    pub height_mm: f64,
    /// Outer-lining profile area, as IFC base quantities. None for facade-hole dimensions.
    pub area_m2: Option<f64>,
    pub quantity_basis: OpeningQuantityBasis,
    pub status: String,
    pub external: bool,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize)]
pub struct OpeningScheduleTotals {
    pub instances: usize,
    pub windows: usize,
    pub doors: usize,
    /// Sum of the included outer-lining areas, m². No glazing or clear-passage area is inferred.
    pub area_m2: f64,
    pub area_excluded: usize,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize)]
pub struct OpeningSchedule {
    pub rows: Vec<OpeningScheduleRow>,
    pub totals: OpeningScheduleTotals,
}

impl OpeningSchedule {
    /// Keep selected buildings/storeys and recalculate totals. Empty storeys select no rows.
    pub fn retain(&mut self, building: Option<&str>, levels: Option<&[usize]>) {
        self.rows.retain(|row| building.is_none_or(|id| row.building_id == id) && levels.is_none_or(|levels| levels.contains(&row.level)));
        self.totals = OpeningScheduleTotals::default();
        for row in &self.rows {
            self.totals.instances += 1;
            match row.kind {
                ScheduleOpeningKind::Window => self.totals.windows += 1,
                ScheduleOpeningKind::Door => self.totals.doors += 1,
            }
            if let Some(area) = row.area_m2 {
                self.totals.area_m2 += area;
            } else {
                self.totals.area_excluded += 1;
            }
        }
    }
}

fn row(doc: &Document, building: &Built, host: &str, external: bool, fill: &BuiltElement) -> Result<OpeningScheduleRow> {
    let kind = match fill.class {
        ElementClass::Window => ScheduleOpeningKind::Window,
        ElementClass::Door => ScheduleOpeningKind::Door,
        _ => return Err(ModelError::Invalid("an opening schedule requires a window or door fill".into()).into()),
    };
    let level = building.storeys.get(fill.level).ok_or_else(|| ModelError::Invalid(format!("opening {} has no storey {}", fill.id, fill.level)))?;
    let width_mm = trigon_model::checked("schedule width", fill.width, 0.001, 1e8)?;
    let height_mm = trigon_model::checked("schedule height", fill.height, 0.001, 1e8)?;
    let loggia = match doc.component(&fill.type_id) {
        Ok(component) => matches!(component.kind, ComponentKind::Loggia { .. }),
        Err(_) if matches!(fill.type_id.as_str(), "door.interior" | "door.cellar") => false,
        Err(error) => return Err(error.into()),
    };
    Ok(OpeningScheduleRow {
        id: fill.id.clone(),
        building_id: building.building_id.clone(),
        building_name: building.name.clone(),
        host_id: host.into(),
        level: fill.level,
        level_name: level.name.clone(),
        kind,
        type_id: fill.type_id.clone(),
        name: fill.name.clone(),
        width_mm,
        height_mm,
        area_m2: (!loggia)
            .then(|| doc.component(&fill.type_id).map_or(width_mm * height_mm, |t| trigon_model::profile::area(&t.kind, width_mm, height_mm)) / 1e6),
        quantity_basis: if loggia { OpeningQuantityBasis::FacadeOpening } else { OpeningQuantityBasis::OuterLining },
        status: fill.status.name().into(),
        external,
    })
}

/// Reuse a built phase view, avoiding another geometry build in Studio's background worker.
/// Every occurrence is retained, including replacements sharing a semantic ID in an all view.
pub fn opening_schedule_from_built(doc: &Document, buildings: &[Built]) -> Result<OpeningSchedule> {
    let mut schedule = OpeningSchedule::default();
    for building in buildings {
        for wall in building.exterior_walls() {
            for opening in &wall.openings {
                schedule.rows.push(row(doc, building, &wall.id, true, &opening.fill)?);
            }
        }
        if let Some(roof) = &building.roof {
            for light in &roof.rooflights {
                schedule.rows.push(row(doc, building, &roof.id, true, &light.fill)?);
            }
        }
        for wall in &building.interior {
            for opening in &wall.doors {
                schedule.rows.push(row(doc, building, &wall.id, false, &opening.fill)?);
            }
        }
    }
    schedule.rows.sort_by(|a, b| {
        a.building_id
            .cmp(&b.building_id)
            .then(a.level.cmp(&b.level))
            .then(a.kind.cmp(&b.kind))
            .then(a.type_id.cmp(&b.type_id))
            .then(a.id.cmp(&b.id))
            .then(a.status.cmp(&b.status))
            .then(a.host_id.cmp(&b.host_id))
            .then(a.width_mm.total_cmp(&b.width_mm))
            .then(a.height_mm.total_cmp(&b.height_mm))
    });
    schedule.retain(None, None);
    Ok(schedule)
}

/// Build the requested model phase once and derive its actual window/door inventory.
pub fn opening_schedule(doc: &Document, view: View) -> Result<OpeningSchedule> {
    opening_schedule_from_built(doc, &build_view(doc, view)?)
}

#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub(crate) struct OpeningScheduleRequest {
    pub building: Option<String>,
    pub levels: Option<Vec<usize>>,
    pub phase: Option<String>,
    pub filter: Option<String>,
}

pub(crate) fn query(session: &Session, request: &OpeningScheduleRequest) -> Result<(OpeningSchedule, View)> {
    if let Some(id) = &request.building {
        session.doc.building(Some(id))?;
    }
    if let Some(levels) = &request.levels {
        if levels.len() > 256 {
            return Err(ModelError::Invalid("schedule levels may contain at most 256 entries".into()).into());
        }
        let buildings: Vec<_> = session.doc.buildings.iter().filter(|b| request.building.as_ref().is_none_or(|id| &b.id == id)).collect();
        if !levels.is_empty()
            && (buildings.is_empty() || buildings.iter().any(|building| levels.iter().any(|level| *level >= building.storeys.len())))
        {
            return Err(ModelError::Invalid("schedule levels must exist in every selected building".into()).into());
        }
    }
    let view = crate::commands::view_of(&session.doc, request.phase.as_deref(), request.filter.as_deref())?;
    let mut schedule = opening_schedule(&session.doc, view)?;
    schedule.retain(request.building.as_deref(), request.levels.as_deref());
    Ok((schedule, view))
}

pub(crate) fn openings(session: &Session, request: OpeningScheduleRequest) -> Result<Value> {
    let (schedule, view) = query(session, &request)?;
    Ok(json!({
        "phase":session.doc.phases.get(view.phase).map(|phase|phase.id.as_str()),
        "filter":request.filter.as_deref().unwrap_or("complete"),
        "rows":schedule.rows,
        "totals":schedule.totals,
    }))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn session() -> Session {
        let mut session = Session::new(Box::new(trigon_data::Live::new(trigon_data::Credentials::default())));
        session.doc = trigon_io::ifc::sample::document().unwrap();
        session
    }

    #[test]
    fn the_shared_command_lists_actual_model_occurrences_without_mutation() {
        let mut session = session();
        let before = session.doc.clone();
        let result = session.execute("schedule.openings", json!({})).unwrap();
        assert_eq!(result["totals"]["instances"], 55);
        assert_eq!(result["totals"]["windows"], 39);
        assert_eq!(result["totals"]["doors"], 16);
        assert_eq!(session.doc, before);
        assert!(!session.dirty);
        assert_eq!(session.execute("edit.undo", json!({})).unwrap()["undone"], false);
        let spec = crate::commands().iter().find(|command| command.id == "schedule.openings").unwrap();
        assert!(!spec.mutates && !spec.journaled);
        assert_eq!(spec.params_schema()["type"], "object");
    }

    #[test]
    fn schedules_match_ifc_occurrences_and_quantities_in_every_phase_view() {
        let session = session();
        for phase in 0..session.doc.phases.len() {
            for filter in [
                trigon_model::build::PhaseFilter::Complete,
                trigon_model::build::PhaseFilter::Demolition,
                trigon_model::build::PhaseFilter::New,
                trigon_model::build::PhaseFilter::All,
            ] {
                let built = build_view(&session.doc, View { phase, filter }).unwrap();
                let schedule = opening_schedule_from_built(&session.doc, &built).unwrap();
                let text = trigon_io::ifc::write(&session.doc, &built, "schedule.ifc", "2026-01-01T00:00:00");
                assert_eq!(schedule.totals.windows, text.matches("=IFCWINDOW(").count());
                assert_eq!(schedule.totals.doors, text.matches("=IFCDOOR(").count());
                assert_eq!(schedule.totals.instances, schedule.rows.len());
                assert_eq!(schedule.rows.iter().filter(|row| !row.external).count(), 11, "generated interior doors retain inherited phase behavior");
                let areas: Vec<f64> = text
                    .lines()
                    .filter(|line| line.contains("=IFCQUANTITYAREA('Area',"))
                    .map(|line| line.split(',').nth(3).unwrap().parse().unwrap())
                    .collect();
                assert_eq!(areas.len(), schedule.totals.instances - schedule.totals.area_excluded);
                assert!((areas.iter().sum::<f64>() - schedule.totals.area_m2).abs() < 1e-6);
                let mut reversed = built.clone();
                reversed.reverse();
                for building in &mut reversed {
                    building.walls.reverse();
                    building.interior.reverse();
                    for wall in &mut building.walls {
                        wall.openings.reverse();
                    }
                }
                assert_eq!(schedule, opening_schedule_from_built(&session.doc, &reversed).unwrap());
            }
        }
    }

    #[test]
    fn filters_are_validated_and_duplicate_storeys_do_not_duplicate_occurrences() {
        let mut session = session();
        let all = session.execute("schedule.openings", json!({})).unwrap();
        let ground = session.execute("schedule.openings", json!({"building":"b1","levels":[0,0]})).unwrap();
        assert!(ground["rows"].as_array().unwrap().iter().all(|row| row["level"] == 0));
        assert_eq!(ground, session.execute("schedule.openings", json!({"building":"b1","levels":[0]})).unwrap());
        assert!(ground["rows"].as_array().unwrap().len() < all["rows"].as_array().unwrap().len());
        let empty = session.execute("schedule.openings", json!({"levels":[]})).unwrap();
        assert!(empty["rows"].as_array().unwrap().is_empty());
        assert_eq!(empty["totals"]["area_m2"], 0.0);
        for params in [
            json!({"building":"missing"}),
            json!({"levels":[3]}),
            json!({"levels":[-1]}),
            json!({"levels":[18446744073709551615u64]}),
            json!({"levels":vec![0;257]}),
            json!({"phase":"missing"}),
            json!({"filter":"future"}),
            json!({"unknown":1}),
        ] {
            let before = session.doc.clone();
            assert!(session.execute("schedule.openings", params).is_err());
            assert_eq!(session.doc, before);
        }
        let mut empty = Session::new(Box::new(trigon_data::Live::new(trigon_data::Credentials::default())));
        assert_eq!(empty.execute("schedule.openings", json!({})).unwrap()["totals"]["instances"], 0);
        assert!(empty.execute("schedule.openings", json!({"levels":[0]})).is_err());
    }

    #[test]
    fn occurrence_overrides_and_loggia_basis_are_not_confused_with_type_sizes() {
        let mut session = session();
        session
            .execute("facade.place", json!({"facade":"b1.f1","type":"window.casement.2","levels":[0],"bays":[0],"width":1250,"height":1800}))
            .unwrap();
        let schedule = opening_schedule(&session.doc, View::latest(&session.doc)).unwrap();
        let row = schedule.rows.iter().find(|row| row.id == "b1.f1.L0.B0").unwrap();
        assert_eq!((row.width_mm, row.height_mm, row.area_m2), (1250.0, 1800.0, Some(2.25)));
        assert_eq!(row.quantity_basis, OpeningQuantityBasis::OuterLining);
        let component = session.doc.types.iter().find(|component| component.id == "window.casement.2").unwrap().clone();
        let component = trigon_model::ComponentType {
            id: "loggia.schedule".into(),
            kind: ComponentKind::Loggia { depth: 1000.0, door_side: trigon_model::component::Side::Left, door_width: 800.0, rail_height: 1000.0 },
            ..component
        };
        session.execute("types.define", serde_json::to_value(component).unwrap()).unwrap();
        session
            .execute("facade.place", json!({"facade":"b1.f1","type":"loggia.schedule","levels":[0],"bays":[0],"width":1250,"height":1800}))
            .unwrap();
        let schedule = opening_schedule(&session.doc, View::latest(&session.doc)).unwrap();
        let row = schedule.rows.iter().find(|row| row.id == "b1.f1.L0.B0").unwrap();
        assert_eq!(row.kind, ScheduleOpeningKind::Door);
        assert_eq!(row.quantity_basis, OpeningQuantityBasis::FacadeOpening);
        assert_eq!(row.area_m2, None);
        assert_eq!(schedule.totals.area_excluded, 1);
    }

    #[test]
    fn same_id_phase_replacements_remain_distinct_rows() {
        let mut session = session();
        for placement in &mut session.doc.buildings[0].facades[0].placements {
            if placement.created.is_some() && placement.type_id == "door.french" {
                placement.type_id = "window.casement.2".into();
            }
        }
        let result = session.execute("schedule.openings", json!({"filter":"all"})).unwrap();
        let rows: Vec<_> = result["rows"].as_array().unwrap().iter().filter(|row| row["id"] == "b1.f1.L1.B2").collect();
        assert_eq!(rows.len(), 2);
        assert!(rows.iter().any(|row| row["status"] == "new"));
        assert!(rows.iter().any(|row| row["status"] == "demolished"));
        assert!(rows.iter().all(|row| row["type_id"] == "window.casement.2"));
    }

    #[test]
    fn schedules_refresh_after_type_changes_and_preserve_redo() {
        let mut session = session();
        let before = session.execute("schedule.openings", json!({})).unwrap();
        session.execute("edit.copy", json!({"targets":["b1.f1.L0.B0"],"delta":[1600,0,0],"count":1})).unwrap();
        let changed = session.execute("schedule.openings", json!({})).unwrap();
        assert_eq!(changed["totals"]["instances"].as_u64().unwrap(), before["totals"]["instances"].as_u64().unwrap() + 1);
        session.execute("edit.undo", json!({})).unwrap();
        let doc = session.doc.clone();
        assert_eq!(session.execute("schedule.openings", json!({})).unwrap(), before);
        assert_eq!(session.doc, doc);
        assert_eq!(session.execute("edit.redo", json!({})).unwrap()["redone"], true);
        assert_eq!(session.execute("schedule.openings", json!({})).unwrap(), changed);
        let mut component = session.doc.component("window.casement.2").unwrap().clone();
        component.width = 1100.0;
        session.execute("types.define", serde_json::to_value(component).unwrap()).unwrap();
        let resized = session.execute("schedule.openings", json!({})).unwrap();
        assert_ne!(resized["totals"]["area_m2"], changed["totals"]["area_m2"]);
        session.execute("edit.undo", json!({})).unwrap();
        assert_eq!(session.execute("schedule.openings", json!({})).unwrap(), changed);
    }

    #[test]
    fn hostile_built_dimensions_and_storeys_are_reported_without_panics() {
        let session = session();
        let built = build_view(&session.doc, View::latest(&session.doc)).unwrap();
        for number in [f64::NAN, f64::INFINITY, -1.0, 0.0, 1e300] {
            let mut invalid = built.clone();
            invalid[0].walls[0].openings[0].fill.width = number;
            assert!(opening_schedule_from_built(&session.doc, &invalid).is_err());
        }
        let mut invalid = built;
        invalid[0].walls[0].openings[0].fill.level = usize::MAX;
        assert!(opening_schedule_from_built(&session.doc, &invalid).is_err());
    }

    #[test]
    fn generated_cellar_doors_are_included_without_a_library_type() {
        let mut session = session();
        session.execute("building.add_basement", json!({"floor":-2400})).unwrap();
        session.execute("building.set_cellar_stairs",json!({"stairs":[{"id":"cellar","facade":"b1.f2","from":1000,"to":5000,"width":1100,"top":4800,"bottom":2000,"floor":-2400,"door_at":1450,"door_width":900}]})).unwrap();
        let schedule = session.execute("schedule.openings", json!({"levels":[0],"filter":"new"})).unwrap();
        let rows = schedule["rows"].as_array().unwrap();
        let door = rows.iter().find(|row| row["type_id"] == "door.cellar").unwrap();
        assert_eq!(door["id"], "cellar.door");
        assert_eq!(door["status"], "existing");
        assert_eq!(door["width_mm"], 900.0);
        assert_eq!(door["kind"], "door");
        assert_eq!(door["external"], true);
        assert!(door["area_m2"].as_f64().unwrap() > 1.0);
    }

    #[test]
    fn building_filters_do_not_conflate_storeys_from_different_buildings() {
        let mut session = session();
        session.doc.buildings.push(
            trigon_model::Building::new("annex", "Annex", &[[30000.0, 0.0], [40000.0, 0.0], [40000.0, 6000.0], [30000.0, 6000.0]], 1, 3000.0)
                .unwrap(),
        );
        session.execute("facade.set_bays", json!({"facade":"annex.f1","count":1})).unwrap();
        session.execute("facade.place", json!({"facade":"annex.f1","type":"window.casement.1","levels":[0]})).unwrap();
        let schedule = session.execute("schedule.openings", json!({"building":"annex"})).unwrap();
        assert_eq!(schedule["rows"].as_array().unwrap().len(), 1);
        assert_eq!(schedule["rows"][0]["id"], "annex.f1.L0.B0");
        assert_eq!(schedule["rows"][0]["building_name"], "Annex");
        assert!(session.execute("schedule.openings", json!({"levels":[2]})).is_err());
        assert!(session.execute("schedule.openings", json!({"building":"b1","levels":[2]})).is_ok());
    }
}
