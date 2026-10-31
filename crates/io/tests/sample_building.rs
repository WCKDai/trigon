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
