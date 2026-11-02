//! The made-up sample building, exported and read back by both IFC readers.
//!
//! `TRIGON_WRITE_SAMPLE=1 cargo test -p trigon-io --test sample_building` also writes it to
//! `examples/sample-building.ifc` for opening in Revit or Vectorworks.

use trigon_io::ifc;

#[test]
fn the_sample_building_reads_back_as_built() {
    let doc = ifc::sample::document().unwrap();
    let built = trigon_model::build(&doc).unwrap();
    let text = ifc::write(&doc, &built, "sample-building.ifc", "2026-01-01T00:00:00");
    if std::env::var_os("TRIGON_WRITE_SAMPLE").is_some() {
        std::fs::write(concat!(env!("CARGO_MANIFEST_DIR"), "/../../examples/sample-building.ifc"), &text).unwrap();
    }
    let lite = trigon_ifcread::read_ifc_lite(&text).unwrap();
    let bim = trigon_ifcread::read_bimifc(&text).unwrap();
    for r in [&lite, &bim] {
        assert!(r.failures.is_empty(), "{:?}", r.failures);
        assert!(r.georef.as_ref().is_some_and(|g| g.crs_name.as_deref() == Some("EPSG:25832")));
    }
    let cmp = trigon_ifcread::compare(&lite, &bim);
    assert!(cmp.only_first.is_empty() && cmp.only_second.is_empty(), "{cmp:?}");
    assert!(cmp.max_bounds_mm < 0.01, "{cmp:?}");
    // Every product with a body lands where Trigon built it.
    let shapes = ifc::model_shapes(&doc, &built);
    for r in [&lite, &bim] {
        assert_eq!(r.elements.len(), shapes.len(), "{}", r.reader.name());
        for (id, mesh) in &shapes {
            let g = ifc::guid(&format!("trigon:{id}"));
            let e = r.elements.iter().find(|e| e.global_id == g).unwrap_or_else(|| panic!("{id} missing in {}", r.reader.name()));
            let d = trigon_ifcread::bounds_deviation(mesh.bounds().unwrap(), e.mesh.bounds().unwrap());
            assert!(d < 0.01, "{id}: {d} mm in {}", r.reader.name());
        }
    }
}

#[test]
fn site_only_sources_export_without_a_fake_building_and_round_trip_exactly() {
    use trigon_model::build::View;
    use trigon_model::{Document, Mesh};
    let doc: Document = serde_json::from_str(include_str!("../../model/tests/fixtures/courtyard.trigon")).unwrap();
    let objects = trigon_model::site::build(&doc, View::latest(&doc), None).unwrap();
    assert_eq!(objects.len(), 4);
    assert!(trigon_model::build(&doc).unwrap().is_empty());
    let text = ifc::write_with_site(&doc, &[], &objects, "courtyard.ifc", "2026-01-01T00:00:00");
    assert!(!text.contains("=IFCBUILDING("));
    assert!(!text.contains("=IFCBUILDINGSTOREY("));
    assert!(!text.contains("=IFCSPACE("));
    assert_eq!(text.matches("=IFCBUILDINGELEMENTPROXY(").count(), 4);
    assert!(text.contains("exchange proxy") && text.contains("PROPOSAL") && text.contains("TrigonSourceJSON"));
    for r in [trigon_ifcread::read_ifc_lite(&text).unwrap(), trigon_ifcread::read_bimifc(&text).unwrap()] {
        assert!(r.failures.is_empty(), "{:?}", r.failures);
        assert_eq!(r.elements.len(), 4);
        for o in &objects {
            let id = ifc::guid(&format!("trigon:{}", o.id));
            let e = r.elements.iter().find(|e| e.global_id == id).unwrap();
            let mut mesh = Mesh::default();
            for (_, part) in &o.parts {
                mesh.append(part);
            }
            assert!(trigon_ifcread::bounds_deviation(mesh.bounds().unwrap(), e.mesh.bounds().unwrap()) < 0.01, "{}: {}", r.reader.name(), o.id);
        }
    }
    let glb = trigon_io::glb::write_with_site(&[], &objects, &[]);
    let len = u32::from_le_bytes(glb[12..16].try_into().unwrap()) as usize;
    let json: serde_json::Value = serde_json::from_slice(&glb[20..20 + len]).unwrap();
    let nodes = json["nodes"].as_array().unwrap();
    assert_eq!(nodes.len(), 4);
    for source in &doc.site_objects {
        let node = nodes.iter().find(|n| n["name"] == source.id).unwrap();
        let restored: trigon_model::site::SiteObject = serde_json::from_value(node["extras"]["trigon_source"].clone()).unwrap();
        assert_eq!(&restored, source);
    }
    assert_eq!(glb, trigon_io::glb::write_with_site(&[], &objects, &[]));
    let legacy = trigon_io::glb::write(&[], &[]);
    let empty = trigon_io::glb::write_with_site(&[], &[], &[]);
    assert_eq!(legacy, empty);
}
