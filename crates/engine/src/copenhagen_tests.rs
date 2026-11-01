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
