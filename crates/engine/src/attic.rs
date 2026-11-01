//! Explicit roof floors. Geometry stays in the model; the command owns history and saved-view guards.
use schemars::JsonSchema;
use serde::Deserialize;
use serde_json::{Value, json};
use trigon_model::{Building, Document, ModelError};

use crate::{Result, Session};

#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub(crate) struct SetAttic {
    building: Option<String>,
    /// Roof floor elevation in site-local mm. Keeps eaves, roof and dormers fixed.
    floor: Option<f64>,
    /// Explicitly remove the roof level, restoring the unoccupied eaves slab. Omit floor/name.
    #[serde(default)]
    remove: bool,
    /// Name of the new roof level; editing without a name preserves its existing name.
    name: Option<String>,
    source: Option<String>,
    attribution: Option<String>,
}

pub(crate) fn description(b: &Building) -> Value {
    b.attic_level().map_or(Value::Null, |level| {
        let s = &b.storeys[level];
        json!({
            "level": level, "name": s.name, "floor_mm": s.elevation,
            "knee_wall_mm": s.height, "floor_slab_id": format!("{}.attic", b.id),
            "room_height_mm": null, "habitable_area_m2": null,
            "interior_support": "Roof-clipped interior walls, rooms and habitable quantities are not supported"
        })
    })
}

fn check_removal(doc: &Document, level: usize) -> Result<()> {
    if doc.views.iter().any(|v| matches!(v.kind, trigon_model::annotation::DrawingViewKind::Plan { level: l } if l == level))
        || doc.drawings.iter().any(|d| d.plans.iter().any(|p| p.levels.contains(&level)))
    {
        return Err(ModelError::Invalid("remove saved/registered plans of the roof level before removing it".into()).into());
    }
    let suffix = format!(".L{level}");
    if doc.groups.iter().any(|g| g.targets.iter().any(|id| id.ends_with(&suffix) || id.contains(&format!("{suffix}.")))) {
        return Err(ModelError::Invalid("remove roof-level occurrences from saved groups before removing the level".into()).into());
    }
    Ok(())
}

pub(crate) fn set(s: &mut Session, p: SetAttic) -> Result<Value> {
    if p.remove == p.floor.is_some() || (p.remove && p.name.is_some()) {
        return Err(ModelError::Invalid("give floor to create/edit a roof level, or remove:true without floor/name".into()).into());
    }
    let b = s.doc.building(p.building.as_deref())?;
    if p.remove {
        let level = b.attic_level().ok_or_else(|| ModelError::Invalid("the building has no explicit roof level".into()))?;
        check_removal(&s.doc, level)?;
    }
    let b = s.doc.building_mut(p.building.as_deref())?;
    b.set_attic(p.floor, p.name.as_deref())?;
    if let Some(source) = p.source.filter(|x| !x.trim().is_empty()) {
        b.evidence.push(trigon_model::Evidence { source, supplies: "roof floor level".into(), attribution: p.attribution.unwrap_or_default() });
    }
    let id = b.id.clone();
    // Validate openings, floor/stair voids and every generated solid before history is committed.
    trigon_model::build(&s.doc)?;
    let b = s.doc.building(Some(&id))?;
    Ok(json!({
        "building": id, "eaves_mm": b.height(), "roof_level": description(b),
        "full_storeys_above_ground": b.storeys.iter().skip(b.ground_level).filter(|s| !s.roof_level).count(),
        "stairs": "Unchanged source definitions; extend only selected, evidenced departures with stair/edit commands"
    }))
}

#[cfg(test)]
mod tests {
    use super::*;
    use trigon_model::{Mesh, build};

    fn session() -> Session {
        let mut s = Session::new(Box::new(trigon_data::Live::new(trigon_data::Credentials::default())));
        s.execute(
            "building.add",
            json!({"id":"b","name":"Synthetic","footprint":[[0,0],[20000,0],[20000,12000],[0,12000]],"storeys":3,"storey_height_mm":3400}),
        )
        .unwrap();
        s.execute("building.set_levels", json!({"floors":[0,3000,6510],"top":10200})).unwrap();
        s.execute(
            "building.set_roof",
            json!({"kind":"pitched","pitch_deg":35,"thickness_mm":220,"overhang_mm":300,"gutters":true,
            "dormers":[{"id":"d1","facade":"b.f1","u_mm":10000,"window_type":"window.casement.2","width_mm":1200,"height_mm":1100}]}),
        )
        .unwrap();
        s.execute("facade.set_bays", json!({"facade":"b.f1","centres":[5000]})).unwrap();
        s.execute("facade.place", json!({"facade":"b.f1","type":"window.casement.2","levels":[2],"sill":800,"height":1850})).unwrap();
        s.execute(
            "element.set",
            json!({"id":"b.f1","properties":{"bands":[
            {"levels":[1,2],"bottom":3270,"top":3390,"projection":50,"finish":"yellow_brick"},
            {"levels":[2],"bottom":3390,"top":3690,"projection":85,"finish":"yellow_brick"}]}}),
        )
        .unwrap();
        s.execute("interior.add_stairs", json!({"stairs":[{"id":"core","levels":[0,1],"outline":[[8000,4000],[11000,4000],[11000,6000],[8000,6000]],"kind":"u_turn","landing":1000,"rounded":false}]})).unwrap();
        s
    }

    fn bytes(s: &Session) -> Vec<u8> {
        serde_json::to_vec(&s.doc).unwrap()
    }

    fn assert_atomic(s: &mut Session, command: &str, params: Value) {
        let before = bytes(s);
        let dirty = s.dirty;
        let err = s.execute(command, params).unwrap_err();
        assert!(!matches!(err, crate::EngineError::Internal(_)), "{err}");
        assert_eq!(bytes(s), before, "failed {command} changed document/history");
        assert_eq!(s.dirty, dirty);
    }

    fn roof_meshes(b: &trigon_model::Built) -> Vec<Mesh> {
        let r = b.roof.as_ref().unwrap();
        std::iter::once(r.cover.clone())
            .chain(std::iter::once(r.gables.clone()))
            .chain(std::iter::once(r.gutters.clone()))
            .chain(r.covering.iter().map(|(_, m)| m.clone()))
            .chain(r.drainage.iter().map(|p| p.mesh.clone()))
            .chain(r.dormers.iter().flat_map(|d| [d.roof.clone(), d.cheeks.clone(), d.front.mesh.clone()]))
            .chain(r.dormers.iter().flat_map(|d| d.front.openings.iter().flat_map(|o| o.fill.parts.iter().map(|(_, m)| m.clone()))))
            .collect()
    }

    #[test]
    fn attic_geometry_preserves_roof_dormers_bands_openings_floors_and_source_stairs() {
        let mut s = session();
        let before_doc = s.doc.clone();
        let before = build(&s.doc).unwrap().remove(0);
        let result = s.execute("building.set_attic", json!({"floor":9450,"source":"Synthetic section; not a survey"})).unwrap();
        assert_eq!(result["eaves_mm"], 10200.0);
        assert_eq!(result["full_storeys_above_ground"], 3);
        assert_eq!(result["roof_level"]["knee_wall_mm"], 750.0);
        assert!(result["roof_level"]["room_height_mm"].is_null());
        let b = &s.doc.buildings[0];
        assert_eq!(b.attic_level(), Some(3));
        assert_eq!(b.storeys[3].name, "3. sal / tagkvist");
        assert_eq!(b.storeys[2].height, 2940.0);
        assert_eq!(b.storeys[0..2], before_doc.buildings[0].storeys[0..2]);
        for (old, new) in before_doc.buildings[0].facades.iter().zip(&b.facades) {
            assert_eq!(old.placements, new.placements);
        }
        assert_eq!(b.stairs, before_doc.buildings[0].stairs, "no invented or duplicated flights");
        let after = build(&s.doc).unwrap().remove(0);
        assert_eq!(roof_meshes(&before), roof_meshes(&after));
        for (old, new) in before.walls.iter().zip(&after.walls) {
            assert_eq!(old.trim, new.trim, "band world geometry moved on {}", old.id);
            assert_eq!(old.trim_parts, new.trim_parts);
            for (a, b) in old.openings.iter().zip(&new.openings) {
                assert_eq!(a.id, b.id);
                assert_eq!(a.rect, b.rect);
                assert_eq!(a.fill.parts, b.fill.parts);
            }
        }
        for old in before.slabs.iter().filter(|f| f.id != "b.attic") {
            let new = after.slabs.iter().find(|f| f.id == old.id).unwrap();
            assert_eq!(old.mesh, new.mesh);
            assert_eq!(old.holes, new.holes);
        }
        let floor = after.slabs.iter().find(|f| f.id == "b.attic").unwrap();
        assert_eq!(floor.level, 3);
        assert_eq!(floor.z_bottom, 9250.0);
        assert!(floor.holes.is_empty());
        assert!(!after.slabs.iter().any(|f| f.id == "b.slab.L3" || f.z_bottom + f.thickness == 10200.0));
        assert_eq!(after.stairs.len(), 2);
        let schedule = crate::opening_schedule(&s.doc, trigon_model::build::View::latest(&s.doc)).unwrap();
        let dormer = schedule.rows.iter().find(|r| r.id.contains("dormer")).unwrap();
        assert_eq!(dormer.level, 3);
        assert_eq!(dormer.level_name, "3. sal / tagkvist");
        let info = s.execute("document.info", json!({})).unwrap();
        assert_eq!(info["buildings"][0]["full_storeys_above_ground"], 3);
        assert_eq!(info["buildings"][0]["levels_mm"][3]["knee_wall_mm"], 750.0);
    }

    #[test]
    fn attic_property_edits_keep_eaves_and_bands_fixed_and_round_trip_history() {
        let mut s = session();
        let initial = bytes(&s);
        s.execute("building.set_attic", json!({"floor":9450,"name":"Tagkvist"})).unwrap();
        let created = bytes(&s);
        s.execute("edit.undo", json!({})).unwrap();
        assert_eq!(bytes(&s), initial);
        s.execute("edit.redo", json!({})).unwrap();
        assert_eq!(bytes(&s), created);
        let original = build(&s.doc).unwrap().remove(0);
        s.execute("element.set", json!({"id":"b.attic","properties":{"elevation_mm":9500}})).unwrap();
        assert_eq!(s.doc.buildings[0].height(), 10200.0);
        assert_eq!(s.doc.buildings[0].storeys[2].height, 2990.0);
        assert_eq!(s.doc.buildings[0].storeys[3].name, "Tagkvist");
        s.execute("element.set", json!({"id":"b.attic","properties":{"storey_height_mm":800}})).unwrap();
        assert_eq!(s.doc.buildings[0].height(), 10200.0);
        assert_eq!(s.doc.buildings[0].storeys[3].elevation, 9400.0);
        let edited = build(&s.doc).unwrap().remove(0);
        assert_eq!(roof_meshes(&original), roof_meshes(&edited));
        assert_eq!(original.walls[0].trim, edited.walls[0].trim);
        let descriptor = s.execute("element.get", json!({"id":"b.attic"})).unwrap();
        let text = descriptor.to_string();
        assert!(text.contains("Knee wall"));
        assert!(text.contains("not room height"));
        let before_save = bytes(&s);
        let path = std::env::temp_dir().join(format!("trigon-attic-{}.trigon", std::process::id()));
        s.execute("document.save", json!({"path":path})).unwrap();
        let mut reopened = Session::new(Box::new(trigon_data::Live::new(trigon_data::Credentials::default())));
        reopened.execute("document.open", json!({"path":path})).unwrap();
        assert_eq!(bytes(&reopened), before_save);
        assert_eq!(roof_meshes(&build(&reopened.doc).unwrap()[0]), roof_meshes(&edited));
        std::fs::remove_file(path).unwrap();
        let before_remove = bytes(&s);
        s.execute("building.set_attic", json!({"remove":true})).unwrap();
        assert_eq!(s.doc.buildings[0].height(), 10200.0);
        assert_eq!(s.doc.buildings[0].storeys.len(), 3);
        assert_eq!(s.doc.buildings[0].storeys[2].height, 3690.0);
        let legacy = build(&s.doc).unwrap();
        assert_eq!(legacy[0].slabs.last().unwrap().z_bottom, 10000.0);
        s.execute("edit.undo", json!({})).unwrap();
        assert_eq!(bytes(&s), before_remove);
        let restored = bytes(&s);
        s.execute("edit.redo", json!({})).unwrap();
        let removed = bytes(&s);
        s.execute("edit.undo", json!({})).unwrap();
        assert_eq!(bytes(&s), restored);
        s.execute("edit.redo", json!({})).unwrap();
        assert_eq!(bytes(&s), removed);
    }

    #[test]
    fn attic_invalid_positions_and_other_level_commands_fail_atomically() {
        let mut s = session();
        for params in [
            json!({}),
            json!({"remove":true}),
            json!({"floor":9450,"remove":true}),
            json!({"floor":9450,"name":""}),
            json!({"floor":f64::MAX}),
            json!({"floor":10200}),
            json!({"floor":10150}),
            json!({"floor":8000}),
            json!({"floor":9200}),
        ] {
            assert_atomic(&mut s, "building.set_attic", params);
        }
        s.execute("building.set_attic", json!({"floor":9450})).unwrap();
        for (command, params) in [
            ("building.set_storeys", json!({"count":3})),
            ("building.set_levels", json!({"floors":[0,3000,6510],"top":10200})),
            ("building.set_roof", json!({"kind":"flat"})),
            ("element.set", json!({"id":"b.attic","properties":{"storey_height_mm":99}})),
            ("element.set", json!({"id":"b.attic","properties":{"elevation_mm":9200}})),
        ] {
            assert_atomic(&mut s, command, params);
        }
        let mut flat = session();
        flat.doc.buildings[0].roof = trigon_model::Roof::Flat;
        assert_atomic(&mut flat, "building.set_attic", json!({"floor":9450}));
        let mut many = session();
        many.doc.buildings[0] = Building::new("b", "Many", &[[0., 0.], [20000., 0.], [20000., 12000.], [0., 12000.]], 60, 3000.).unwrap();
        many.doc.buildings[0].roof = serde_json::from_value(json!({"kind":"pitched","pitch_deg":35})).unwrap();
        assert_atomic(&mut many, "building.set_attic", json!({"floor":179250}));
        let mut b = s.doc.buildings[0].clone();
        let before = b.clone();
        for floor in [f64::NAN, f64::INFINITY, f64::NEG_INFINITY] {
            assert!(b.set_attic(Some(floor), None).is_err());
            assert_eq!(b, before);
        }
    }

    #[test]
    /// Default detection matches explicit full-storey selection, including the basement;
    /// refused roof requests must leave the detection transaction available for Redo.
    fn room_detection_defaults_to_full_storeys_and_keeps_explicit_roof_requests_atomic() {
        for attic in [false, true] {
            let mut s = session();
            s.execute("building.add_basement", json!({"floor":-2200})).unwrap();
            if attic {
                s.execute("building.set_attic", json!({"floor":9450})).unwrap();
            }
            let initial = bytes(&s);
            let mut explicit = Session::new(Box::new(trigon_data::Live::new(trigon_data::Credentials::default())));
            explicit.doc = s.doc.clone();
            let expected = explicit.execute("interior.detect_rooms", json!({"levels":[0,1,2,3],"use_scan":false})).unwrap();
            let actual = s.execute("interior.detect_rooms", json!({"use_scan":false})).unwrap();
            assert_eq!(actual, expected);
            assert_eq!(actual.as_array().unwrap().iter().map(|v| v["level"].as_u64().unwrap()).collect::<Vec<_>>(), vec![0, 1, 2, 3]);
            assert_eq!(s.doc.buildings[0].rooms, explicit.doc.buildings[0].rooms);
            assert!(s.doc.buildings[0].rooms.iter().any(|r| r.levels.contains(&0)), "basement detection stays included");
            assert!(s.doc.buildings[0].rooms.iter().all(|r| r.levels.iter().all(|l| *l < 4)));
            let detected = bytes(&s);
            s.execute("edit.undo", json!({})).unwrap();
            assert_eq!(bytes(&s), initial);
            if attic {
                assert_atomic(&mut s, "interior.detect_rooms", json!({"levels":[4],"use_scan":false}));
                assert_atomic(&mut s, "interior.detect_rooms", json!({"levels":[0,4],"use_scan":false}));
            }
            s.execute("edit.redo", json!({})).unwrap();
            assert_eq!(bytes(&s), detected, "explicit roof refusals preserve the pending Redo");
        }
    }

    #[test]
    fn attic_refuses_unclipped_interiors_and_dangling_level_references() {
        let mut s = session();
        s.execute("building.set_attic", json!({"floor":9450})).unwrap();
        for (command, params) in [
            ("interior.add_walls", json!({"walls":[{"id":"roof-wall","levels":[3],"a":[2000,3000],"b":[6000,3000],"thickness":100}]})),
            (
                "interior.add_rooms",
                json!({"rooms":[{"id":"roof-room","levels":[3],"name":"Unsupported","polygon":[[2000,2000],[5000,2000],[5000,5000],[2000,5000]]}]}),
            ),
            ("interior.detect_rooms", json!({"levels":[3],"use_scan":false})),
            (
                "interior.add_stairs",
                json!({"stairs":[{"id":"roof-flight","levels":[3],"outline":[[8000,4000],[11000,4000],[11000,6000],[8000,6000]]}]}),
            ),
            ("edit.copy_to_levels", json!({"targets":["core.L1"],"levels":[3]})),
        ] {
            assert_atomic(&mut s, command, params);
        }
        s.execute("view.define", json!({"id":"roof-plan","name":"Roof","kind":{"view":"plan","level":3}})).unwrap();
        assert_atomic(&mut s, "building.set_attic", json!({"remove":true}));
        s.execute("edit.undo", json!({})).unwrap();
        s.execute("facade.place", json!({"facade":"b.f1","type":"window.casement.2","levels":[3],"height":300,"sill":100})).unwrap();
        assert_atomic(&mut s, "building.set_attic", json!({"remove":true}));
        assert_atomic(&mut s, "building.set_attic", json!({"floor":9500}));
        let mut invalid = s.doc.clone();
        invalid.buildings[0].storeys[2].roof_level = true;
        assert!(build(&invalid).is_err());
    }

    #[test]
    fn attic_holes_and_ifc_follow_only_explicit_selected_stair_departures() {
        let mut s = session();
        s.execute("building.set_attic", json!({"floor":9450})).unwrap();
        let before = build(&s.doc).unwrap();
        assert!(before[0].slabs.last().unwrap().holes.is_empty());
        s.execute("edit.copy_to_levels", json!({"targets":["core.L1"],"levels":[2]})).unwrap();
        let built = build(&s.doc).unwrap();
        let floor = built[0].slabs.iter().find(|s| s.id == "b.attic").unwrap();
        assert_eq!(floor.holes.len(), 1);
        assert_eq!(built[0].stairs.len(), 3);
        let ifc = trigon_io::ifc::write(&s.doc, &built, "synthetic.ifc", "2000-01-01T00:00:00Z");
        assert!(ifc.lines().any(|line| line.contains("=IFCBUILDINGSTOREY(")
            && line.contains(&trigon_io::ifc::string("3. sal / tagkvist"))
            && line.ends_with("9450.);")));
        assert!(!ifc.contains("=IFCSPACE("));
        let guid = trigon_io::ifc::guid("trigon:b.attic");
        for read in [trigon_ifcread::read_ifc_lite(&ifc).unwrap(), trigon_ifcread::read_bimifc(&ifc).unwrap()] {
            assert!(read.failures.is_empty(), "{:?}", read.failures);
            let slab = read.elements.iter().find(|e| e.global_id == guid).unwrap();
            let (lo, hi) = slab.mesh.bounds().unwrap();
            assert!((lo[2] - 9250.).abs() < 1e-3);
            assert!((hi[2] - 9450.).abs() < 1e-3);
            let top_area: f64 = slab
                .mesh
                .triangles
                .iter()
                .filter_map(|t| {
                    let [a, b, c] = t.map(|i| slab.mesh.positions[i as usize]);
                    ([a, b, c].iter().all(|p| (p[2] - 9450.).abs() < 1e-3))
                        .then_some(((b[0] - a[0]) * (c[1] - a[1]) - (b[1] - a[1]) * (c[0] - a[0])).abs() / 2.)
                })
                .sum();
            let expected =
                trigon_model::doc::polygon_area(&floor.polygon) - floor.holes.iter().map(|h| trigon_model::doc::polygon_area(h)).sum::<f64>();
            assert!((top_area - expected).abs() < expected * 1e-6, "{} filled roof stair hole", read.reader.name());
        }
        let view = trigon_model::build::View::latest(&s.doc);
        let plan = crate::plan_sheet_svg(&s.doc, 3, view).unwrap();
        assert!(plan.contains("3. sal / tagkvist"));
        let section = crate::section::scene(&s.doc, [[10000., -1000.], [10000., 13000.]], false, 10000., view, "Section", "").unwrap();
        assert!(section.svg().contains("tagkvist"));
    }
    #[test]
    fn attic_marker_is_opt_in_legacy_bytes_and_unmarked_height_guards_survive() {
        let legacy = r#"{"name":"2. sal","elevation":6510.0,"height":3690.0}"#;
        let storey: trigon_model::Storey = serde_json::from_str(legacy).unwrap();
        assert!(!storey.roof_level);
        assert_eq!(serde_json::to_string(&storey).unwrap(), legacy);
        let mut b = Building::new("b", "Short full storey", &[[0., 0.], [10000., 0.], [10000., 10000.], [0., 10000.]], 1, 2400.).unwrap();
        b.roof = serde_json::from_value(json!({"kind":"pitched","pitch_deg":35})).unwrap();
        let before = b.clone();
        assert!(b.set_attic(Some(1650.), None).unwrap_err().to_string().contains("full storey"));
        assert_eq!(b, before);
        assert!(b.set_storeys(1, 750.).is_err());
        assert_eq!(b, before);
        assert!(b.set_levels(&[0.], 750.).is_err());
        assert_eq!(b, before);
        let mut s = session();
        assert_atomic(&mut s, "building.set_attic", json!({"floor":9320}));
        s.execute("building.set_attic", json!({"floor":9450})).unwrap();
        assert_atomic(&mut s, "element.set", json!({"id":"b.attic","properties":{"thickness_mm":400}}));
        let mut invalid = s.doc.clone();
        invalid.buildings[0].storeys[3].roof_level = false;
        invalid.buildings[0].storeys[2].roof_level = true;
        assert!(build(&invalid).is_err());
    }

    #[test]
    fn attic_mansard_and_basement_preserve_roof_bands_and_full_storey_count() {
        let mut s = session();
        s.execute("building.set_roof", json!({"kind":"mansard","lower_pitch_deg":68,"upper_pitch_deg":25,"break_run_mm":1300,"thickness_mm":220}))
            .unwrap();
        let before = build(&s.doc).unwrap().remove(0);
        s.execute("building.set_attic", json!({"floor":9450})).unwrap();
        assert_eq!(roof_meshes(&before), roof_meshes(&build(&s.doc).unwrap()[0]));
        let trim = build(&s.doc).unwrap()[0].walls[0].trim.clone();
        s.execute("building.add_basement", json!({"floor":-2500})).unwrap();
        assert_eq!(s.doc.buildings[0].attic_level(), Some(4));
        assert_eq!(s.doc.buildings[0].height(), 10200.0);
        assert_eq!(s.doc.buildings[0].storeys[4].name, "3. sal / tagkvist");
        assert_eq!(trim, build(&s.doc).unwrap()[0].walls[0].trim);
        let info = s.execute("document.info", json!({})).unwrap();
        assert_eq!(info["buildings"][0]["full_storeys_above_ground"], 3);
        assert_eq!(info["buildings"][0]["ground_level"], 1);
    }

    #[test]
    fn attic_rejects_colliding_doors_holes_and_preserves_exact_journal_replay() {
        let mut s = session();
        s.execute("interior.add_walls",json!({"walls":[{"id":"tall-door","levels":[2],"a":[2000,3000],"b":[6000,3000],"thickness":100,"doors":[{"at":2000,"width":900,"height":2800}]}]})).unwrap();
        assert_atomic(&mut s, "building.set_attic", json!({"floor":9450}));
        s.execute("edit.undo", json!({})).unwrap();
        s.execute("building.set_attic", json!({"floor":9450})).unwrap();
        s.execute("edit.copy_to_levels", json!({"targets":["core.L1"],"levels":[2]})).unwrap();
        assert_atomic(
            &mut s,
            "interior.add_stairs",
            json!({"stairs":[{"id":"collision","levels":[2],"outline":[[8500,4000],[11500,4000],[11500,6000],[8500,6000]],"rounded":false}]}),
        );
        let journal = s.doc.journal.clone();
        let mut replay = Session::new(Box::new(trigon_data::Live::new(trigon_data::Credentials::default())));
        for entry in journal {
            replay.execute(&entry.command, entry.params).unwrap();
        }
        assert_eq!(bytes(&s), bytes(&replay));
    }
    #[test]
    fn attic_sections_name_the_cut_wings_roof_level_instead_of_the_first_building() {
        let mut s = session();
        s.execute("building.set_attic", json!({"floor":9450})).unwrap();
        s.execute(
            "building.add",
            json!({"id":"rear","name":"Rear wing","footprint":[[40000,0],[60000,0],[60000,12000],[40000,12000]],"storeys":2,"storey_height_mm":3900}),
        )
        .unwrap();
        s.execute("building.set_roof", json!({"building":"rear","kind":"pitched","pitch_deg":35})).unwrap();
        s.execute("building.set_attic", json!({"building":"rear","floor":7050,"name":"Rear roof"})).unwrap();
        let section = crate::section::scene(
            &s.doc,
            [[50000., -1000.], [50000., 13000.]],
            false,
            10000.,
            trigon_model::build::View::latest(&s.doc),
            "Rear section",
            "",
        )
        .unwrap();
        let svg = section.svg();
        assert!(svg.contains("Rear roof"));
        assert!(!svg.contains("3. sal / tagkvist"));
        assert_eq!(s.doc.level_label(2).as_deref(), Some("2. sal · Rear roof"));
        let view = trigon_model::build::View::latest(&s.doc);
        let plan = crate::plan_sheet_svg(&s.doc, 2, view).unwrap();
        assert!(plan.contains("2. sal · Rear roof"));
        let range = crate::plan_range::sheet(&s.doc, 2, view, Default::default()).unwrap();
        assert_eq!(range.title, "2. sal · Rear roof");
    }
    #[test]
    fn attic_rehosting_keeps_the_detailed_eave_and_seated_cornice_fixed() {
        let mut s = session();
        s.execute("building.set_roof", json!({"kind":"pitched","pitch_deg":35,"thickness_mm":220,"overhang_mm":300,"gutters":true,"drainage":{}}))
            .unwrap();
        let before = build(&s.doc).unwrap().remove(0);
        s.execute("building.set_attic", json!({"floor":9450})).unwrap();
        s.execute("building.set_attic", json!({"floor":9500})).unwrap();
        let after = build(&s.doc).unwrap().remove(0);
        assert_eq!(roof_meshes(&before), roof_meshes(&after));
        assert_eq!(before.walls[0].trim, after.walls[0].trim);
        assert_eq!(before.walls[0].trim_parts, after.walls[0].trim_parts);
    }
    #[test]
    fn attic_default_plan_shows_the_roof_cut_instead_of_an_empty_symbolic_plan() {
        let mut s = session();
        s.execute("building.set_attic", json!({"floor":9450})).unwrap();
        let sheet = crate::plan::drawing_sheet(&s.doc, 3, trigon_model::build::View::latest(&s.doc)).unwrap();
        assert!(sheet.scene.element_regions.iter().any(|r| r.id == "b.roof" && r.kind == crate::DrawingElementKind::Roof));
    }

    #[test]
    fn attic_preserves_selected_eaves_zero_yaw_dormer_and_source_drainage() {
        let mut s = Session::new(Box::new(trigon_data::Live::new(trigon_data::Credentials::default())));
        s.execute("building.add", json!({"id":"b","name":"Selected bearings","footprint":[[0,0],[16000,0],[16000,800],[24000,800],[24000,12800],[28000,12800],[28000,30000],[0,30000]],"storeys":3,"storey_height_mm":3400})).unwrap();
        s.execute("building.set_roof", json!({"kind":"pitched","pitch_deg":46,"thickness_mm":220,"overhang_mm":250,"gutters":true})).unwrap();
        s.execute("element.set", json!({"id":"b.roof","properties":{
            "straighten_mm":10000,
            "eave_facades":["b.f1","b.f4","b.f5","b.f6","b.f7","b.f8"],
            "dormers":[{"id":"source-host","facade":"b.f4","u_mm":6000,"window_type":"window.casement.2","width_mm":1100,"height_mm":1100,"setback_mm":350}],
            "drainage":{"downpipes":[{"id":"source-four","facade":"b.f4","along_mm":6000,"diameter_mm":80,"wall_offset_mm":100,"bottom_mm":150,"shoe_mm":200}]}
        }})).unwrap();
        let source = bytes(&s);
        let roof_source = s.doc.buildings[0].roof.clone();
        let before = build(&s.doc).unwrap().remove(0);
        s.execute("building.set_attic", json!({"floor":9450})).unwrap();
        assert_eq!(s.doc.buildings[0].roof, roof_source);
        let after = build(&s.doc).unwrap().remove(0);
        assert_eq!(roof_meshes(&before), roof_meshes(&after));
        let roof = after.roof.as_ref().unwrap();
        assert!(roof.drainage.iter().any(|p| p.id == "b.roof.downpipe.source-four"));
        assert_eq!(roof.dormers.len(), 1);
        assert_eq!(after.slabs.iter().filter(|slab| slab.id == "b.attic").count(), 1);
        assert_eq!(after.slabs.iter().find(|slab| slab.id == "b.attic").unwrap().z_bottom, 9250.);
        s.execute("edit.undo", json!({})).unwrap();
        assert_eq!(bytes(&s), source);
    }
}
