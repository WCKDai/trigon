use crate::{Session, commands};
use serde_json::json;
use trigon_model::{Building, build};

fn session() -> Session {
    let mut s = Session::new(Box::new(trigon_data::Live::new(trigon_data::Credentials::default())));
    s.doc
        .buildings
        .push(Building::new("b", "Original family gallery", &[[0.0, 0.0], [42000.0, 0.0], [42000.0, 10000.0], [0.0, 10000.0]], 2, 3500.0).unwrap());
    s
}

#[test]
fn shaped_balcony_is_a_separate_undoable_attachment_with_a_portable_outline() {
    let mut s = session();
    s.execute("facade.set_bays", json!({"facade":"b.f1","count":8})).unwrap();
    s.execute("facade.place", json!({"facade":"b.f1","type":"window.casement.2","levels":[1],"bays":[2]})).unwrap();
    s.execute("types.define",json!({"id":"original.bowed","name":"Original bowed balcony","width":3000,"height":1100,"sill":0,"kind":{"family":"shaped_balcony","depth":1500,"thickness":180,"plan":{"shape":"bowed","return_fraction":0.35}}})).unwrap();
    s.execute("view.define", json!({"id":"plan","name":"Original balcony plan","kind":{"view":"plan","level":1}})).unwrap();
    let before = s.doc.clone();
    let opening_before = build(&s.doc).unwrap()[0].walls[0].openings.clone();
    s.execute("facade.place", json!({"facade":"b.f1","type":"original.bowed","levels":[1],"bays":[2]})).unwrap();
    let built = build(&s.doc).unwrap();
    let wall = &built[0].walls[0];
    assert_eq!(wall.openings, opening_before, "adding a balcony must not change the host opening");
    assert_eq!(wall.attachments.len(), 2, "slab and railing have separate source-derived identities");
    let centre = s.doc.buildings[0].facades[0].bays[2];
    assert!(
        s.execute("view.pick", json!({"id":"plan","point":[centre,-750],"tolerance_mm":0})).unwrap()["hit"]["id"]
            .as_str()
            .is_some_and(|id| id.ends_with(".slab"))
    );
    assert_eq!(
        s.execute("view.pick", json!({"id":"plan","point":[centre-1450.0,-1450],"tolerance_mm":0})).unwrap()["hit"],
        serde_json::Value::Null,
        "empty corner outside a bowed slab must stay empty"
    );
    let text = trigon_io::ifc::write(&s.doc, &built, "original-bowed.ifc", "2026-01-01T00:00:00");
    assert!(text.contains("IFCELEMENTASSEMBLY"));
    for read in [trigon_ifcread::read_ifc_lite(&text).unwrap(), trigon_ifcread::read_bimifc(&text).unwrap()] {
        assert!(read.failures.is_empty(), "{:?}", read.failures);
    }
    let placed = s.doc.clone();
    s.execute("element.set", json!({"id":wall.attachments[0].id,"properties":{"depth_mm":1800}})).unwrap();
    s.execute("edit.undo", json!({})).unwrap();
    assert_eq!(s.doc, placed);
    s.execute("element.set", json!({"id":wall.attachments[0].id,"properties":{"balcony_plan":{"shape":"chamfered","corner_mm":300}}})).unwrap();
    assert_ne!(build(&s.doc).unwrap(), built);
    s.execute("edit.undo", json!({})).unwrap();
    assert_eq!(s.doc, placed);
    assert!(s.execute("element.set", json!({"id":wall.attachments[0].id,"properties":{"balcony_plan":{"shape":"polygon","points":[]}}})).is_err());
    assert_eq!(s.doc, placed);
    s.execute("edit.undo", json!({})).unwrap();
    assert_eq!(s.doc, before);
}

#[test]
fn mansard_roof_is_opt_in_parametric_atomic_and_exactly_undoable() {
    let mut s = session();
    let original = s.doc.clone();
    let params =
        json!({"kind":"mansard","lower_pitch_deg":68,"upper_pitch_deg":25,"break_run_mm":1300,"thickness_mm":220,"overhang_mm":250,"gutters":true});
    s.execute("building.set_roof", params.clone()).unwrap();
    assert_eq!(s.doc.buildings[0].roof, serde_json::from_value(json!({"kind":"mansard","lower_pitch_deg":68,"upper_pitch_deg":25,"break_run_mm":1300,"thickness_mm":220,"overhang_mm":250,"gutters":true})).unwrap());
    let before = s.doc.clone();
    let built_before = build(&s.doc).unwrap();
    let r = built_before[0].roof.as_ref().unwrap();
    let expected =
        s.doc.buildings[0].height() + 1300.0 * 68f64.to_radians().tan() + 3700.0 * 25f64.to_radians().tan() + 220.0 / 25f64.to_radians().cos();
    assert!((r.ridge - expected).abs() < 0.01);
    let record = s.execute("element.get", json!({"id":"b.roof"})).unwrap();
    assert!(record.to_string().contains("lower_pitch_deg"));
    s.execute("element.set", json!({"id":"b.roof","properties":{"break_run_mm":1500}})).unwrap();
    assert_ne!(build(&s.doc).unwrap(), built_before);
    s.execute("edit.undo", json!({})).unwrap();
    assert_eq!(s.doc, before);
    assert_eq!(build(&s.doc).unwrap(), built_before);
    for bad in [
        json!({"kind":"mansard","lower_pitch_deg":10,"upper_pitch_deg":60}),
        json!({"kind":"mansard","break_run_mm":10000}),
        json!({"kind":"mansard","upper_pitch_deg":1e300}),
        json!({"kind":"pitched","pitch_deg":35,"break_run_mm":1300}),
        json!({"kind":"mansard","ridge":20000}),
    ] {
        assert!(s.execute("building.set_roof", bad).is_err());
        assert_eq!(s.doc, before);
    }
    s.execute("edit.undo", json!({})).unwrap();
    assert_eq!(s.doc, original);
}

#[test]
fn copenhagen_library_is_explicit_atomic_and_undoable() {
    let mut s = session();
    let before = s.doc.clone();
    let spec = commands().iter().find(|c| c.id == "types.install_copenhagen").unwrap();
    assert!(spec.mutates && spec.journaled);
    let result = s.execute("types.install_copenhagen", json!({})).unwrap();
    assert_eq!(result["installed"].as_array().unwrap().len(), 11);
    assert_eq!(s.doc.journal.len(), before.journal.len() + 1);
    assert_eq!(s.execute("edit.undo", json!({})).unwrap()["undone"], true);
    assert_eq!(s.doc, before);
    s.doc.types.push(trigon_model::ComponentType {
        id: "dk.window.dannebrog".into(),
        name: "Owner customised".into(),
        width: 800.0,
        height: 1400.0,
        sill: 800.0,
        kind: trigon_model::ComponentKind::FixedWindow,
    });
    let before = s.doc.clone();
    assert!(s.execute("types.install_copenhagen", json!({})).is_err());
    assert_eq!(s.doc, before);
}

#[test]
fn curved_family_exchange_quantities_and_exact_edit_undo_agree() {
    let mut s = session();
    s.execute("types.install_copenhagen", json!({})).unwrap();
    s.execute("facade.set_bays", json!({"facade":"b.f1","count":12})).unwrap();
    for (i, ty) in trigon_model::component::copenhagen_types().iter().enumerate() {
        s.execute("facade.place", json!({"facade":"b.f1","type":ty.id,"levels":[0],"bays":[i]})).unwrap();
    }
    let built = build(&s.doc).unwrap();
    let round = built[0].walls[0].openings.iter().find(|o| o.fill.type_id == "dk.window.round").unwrap();
    let area = trigon_model::doc::polygon_area(&round.profile) / 1e6;
    let schedule = s.execute("schedule.openings", json!({})).unwrap();
    let row = schedule["rows"].as_array().unwrap().iter().find(|r| r["type_id"] == "dk.window.round").unwrap();
    assert!((row["area_m2"].as_f64().unwrap() - area).abs() < 1e-9);
    let text = trigon_io::ifc::write(&s.doc, &built, "original-family-gallery.ifc", "2026-01-01T00:00:00");
    assert!(text.contains("IFCARBITRARYCLOSEDPROFILEDEF"));
    for read in [trigon_ifcread::read_ifc_lite(&text).unwrap(), trigon_ifcread::read_bimifc(&text).unwrap()] {
        assert!(read.failures.is_empty(), "{:?}", read.failures);
        let source = trigon_io::ifc::model_shapes(&s.doc, &built);
        for opening in &built[0].walls[0].openings {
            let id = trigon_io::ifc::guid(&format!("trigon:{}", opening.fill.id));
            let actual = read.elements.iter().find(|e| e.global_id == id).unwrap();
            let expected = source.iter().find(|(id, _)| id == &opening.fill.id).unwrap().1.bounds().unwrap();
            assert!(trigon_ifcread::bounds_deviation(expected, actual.mesh.bounds().unwrap()) < 0.01);
        }
    }
    let before = s.doc.clone();
    let built_before = build(&s.doc).unwrap();
    let target = built_before[0].walls[0].openings[5].fill.id.clone();
    s.execute("element.set", json!({"id":target,"properties":{"width_mm":1300,"height_mm":2100}})).unwrap();
    assert_ne!(build(&s.doc).unwrap(), built_before);
    s.execute("edit.undo", json!({})).unwrap();
    assert_eq!(s.doc, before);
    assert_eq!(build(&s.doc).unwrap(), built_before);
    for bad in [
        json!({"id":target,"properties":{"width_mm":1e300}}),
        json!({"id":target,"properties":{"kind":{"family":"historic_window","shape":{"kind":"segmental","rise_mm":-1},"lights":u32::MAX}}}),
    ] {
        assert!(s.execute("element.set", bad).is_err());
        assert_eq!(s.doc, before);
    }
}

#[test]
fn terrain_is_an_explicit_atomic_undoable_source_and_portable_mesh() {
    let mut s = session();
    let before = s.doc.clone();
    s.execute("site.set_terrain", json!({"terrain":{"origin":[-10000,-10000],"spacing_mm":[65000,30000],"heights_mm":[[0,100],[200,300]]}})).unwrap();
    let bytes = trigon_io::glb::write(&build(&s.doc).unwrap(), &[]);
    assert!(bytes.windows(b"site.terrain".len()).any(|w| w == b"site.terrain"));
    let valid = s.doc.clone();
    assert!(s.execute("site.set_terrain", json!({})).is_err());
    assert_eq!(s.doc, valid);
    for heights in [json!([]), json!([[0, 0]]), json!([[0, 0], [0]]), json!([[0, 0], [0, 1e300]])] {
        assert!(s.execute("site.set_terrain", json!({"terrain":{"origin":[0,0],"spacing_mm":[1000,1000],"heights_mm":heights}})).is_err());
        assert_eq!(s.doc, valid);
    }
    s.execute("edit.undo", json!({})).unwrap();
    assert_eq!(s.doc, before);
}

#[test]
fn fbx_export_is_read_only_and_refuses_document_supplied_executables() {
    let mut s = session();
    let before = s.doc.clone();
    let spec = commands().iter().find(|c| c.id == "export.fbx").unwrap();
    assert!(!spec.mutates && !spec.journaled);
    for bad in [
        json!({"path":"wrong.txt"}),
        json!({"path":"out/never.fbx","blender":"/untrusted/program"}),
        json!({"path":"out/never.fbx","filter":"invalid"}),
    ] {
        let error = s.execute("export.fbx", bad).unwrap_err();
        assert!(!matches!(error, crate::EngineError::Internal(_)));
        assert_eq!(s.doc, before);
    }
}

#[test]
fn terrain_removal_is_required_explicitly_and_described_as_nullable() {
    fn nullable(v: &serde_json::Value, root: &serde_json::Value) -> bool {
        if v["type"] == "null" || v["type"].as_array().is_some_and(|a| a.iter().any(|t| t == "null")) {
            return true;
        }
        if let Some(reference) = v["$ref"].as_str().and_then(|r| r.strip_prefix('#')) {
            return root.pointer(reference).is_some_and(|s| nullable(s, root));
        }
        ["anyOf", "oneOf"].iter().any(|key| v[key].as_array().is_some_and(|a| a.iter().any(|s| nullable(s, root))))
    }
    let schema = commands().iter().find(|c| c.id == "site.set_terrain").unwrap().params_schema();
    assert!(schema["required"].as_array().unwrap().iter().any(|x| x == "terrain"));
    assert!(nullable(&schema["properties"]["terrain"], &schema), "explicit null removal must be accepted by the command schema");
    let mut s = session();
    s.execute("site.set_terrain", json!({"terrain":{"origin":[-10000,-10000],"spacing_mm":[65000,30000],"heights_mm":[[0,0],[0,0]]}})).unwrap();
    let before = s.doc.clone();
    s.execute("site.set_terrain", json!({"terrain":null})).unwrap();
    assert!(s.doc.terrain.is_none());
    s.execute("edit.undo", json!({})).unwrap();
    assert_eq!(s.doc, before);
}

#[test]
fn displayed_terrain_has_read_only_source_properties_in_the_shared_authoring_api() {
    let mut s = session();
    s.execute("site.set_terrain", json!({"terrain":{"origin":[-10000,-10000],"spacing_mm":[65000,30000],"heights_mm":[[0,0],[0,0]]}})).unwrap();
    let before = s.doc.clone();
    let p = s.execute("element.get", json!({"id":"site.terrain"})).unwrap();
    assert_eq!(p["kind"], "terrain");
    assert_eq!(p["editable"], false);
    assert!(s.execute("element.set", json!({"id":"site.terrain","properties":{"width_mm":1234}})).is_err());
    assert_eq!(s.doc, before);
}
