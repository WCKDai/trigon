//! Engine tests against a canned provider: no network.

use serde_json::{Value, json};
use trigon_data::{Address, Camera, DataError, Footprint, Heights, Photo, Provider, Region, Register};
use trigon_model::{Evidence, GeoPoint};

use crate::{EngineError, Session, commands};

/// A 24 × 12 m block at a fixed point in Copenhagen, five storeys of surface height.
struct Canned;

fn ev(s: &str) -> Evidence {
    Evidence { source: s.into(), supplies: "test".into(), attribution: "test".into() }
}

impl Provider for Canned {
    fn geocode(&self, q: &str) -> trigon_data::Result<Address> {
        if q.is_empty() {
            return Err(DataError::NotFound("address".into()));
        }
        Ok(Address {
            text: q.into(),
            id: Some("dar".into()),
            utm: [721989.65, 6178579.47],
            wgs: GeoPoint { lat: 55.70208, lon: 12.53306 },
            evidence: ev("dar:test"),
        })
    }
    fn footprint(&self, at: &Address) -> trigon_data::Result<Footprint> {
        let [e, n] = at.utm;
        // Clockwise on purpose: the model must turn it around.
        let ring = vec![[e - 12.0, n - 6.0], [e - 12.0, n + 6.0], [e + 12.0, n + 6.0], [e + 12.0, n - 6.0], [e - 12.0, n - 6.0]];
        Ok(Footprint { ring_utm: ring, tags: Default::default(), bbr_id: Some("b55fb3b8".into()), evidence: ev("geodanmark:bygning/1") })
    }
    fn heights(&self, _: &[[f64; 2]]) -> trigon_data::Result<Heights> {
        Ok(Heights { terrain: 12.0, median: 15.2, top: 17.9, eaves: 15.0, samples: 1800, evidence: ev("dhm:test") })
    }
    fn photos(&self, _: [f64; 2]) -> trigon_data::Result<Vec<Photo>> {
        Ok(vec![Photo {
            id: "p-north".into(),
            collection: "c".into(),
            direction: "north".into(),
            datetime: "2025".into(),
            camera: Some(south_camera()),
            href: "memory".into(),
        }])
    }
    fn register(&self, id: &str) -> trigon_data::Result<Option<Register>> {
        Ok(Some(Register {
            bbr_id: id.into(),
            year_built: Some(1940),
            storeys: Some(5),
            use_code: Some("140".into()),
            use_text: Some("Etagebolig, flerfamiliehus".into()),
            wall_material: Some("brick (mursten)".into()),
            roof_material: Some("clay tiles (tegl)".into()),
            built_area_m2: Some(288.0),
            floor_area_m2: Some(1440.0),
            evidence: ev("bbr:test"),
        }))
    }
    fn photo_region(&self, _: &Photo, w: [u32; 4]) -> trigon_data::Result<Region> {
        // A checkerboard of 40 px squares, so the straightened facade shows a pattern.
        let (width, height) = (w[2] - w[0], w[3] - w[1]);
        let mut rgb = Vec::with_capacity((width * height * 3) as usize);
        for y in w[1]..w[3] {
            for x in w[0]..w[2] {
                let v = if (x / 40 + y / 40) % 2 == 0 { 220 } else { 30 };
                rgb.extend([v, v, v]);
            }
        }
        Ok(Region { x0: w[0], y0: w[1], width, height, rgb })
    }
}

/// A level camera 200 m south of the block, 8 m above the terrain, looking north.
fn south_camera() -> Camera {
    Camera {
        center: [721989.65, 6178379.47, 20.0],
        rotation: [1.0, 0.0, 0.0, 0.0, 0.0, 1.0, 0.0, -1.0, 0.0],
        focal_mm: 100.0,
        pixel_mm: 0.01,
        width: 4000.0,
        height: 3000.0,
        principal_offset_mm: [0.0, 0.0],
    }
}

fn session() -> Session {
    let mut s = Session::new(Box::new(Canned));
    s.execute("site.load_address", json!({"address": "Frederikssundsvej 28, 2400 København"})).unwrap();
    s
}

fn first_facade_facing(s: &mut Session, faces: &str) -> String {
    let list = s.execute("facade.list", json!({})).unwrap();
    list.as_array().unwrap().iter().find(|f| f["faces"] == faces).unwrap()["id"].as_str().unwrap().to_string()
}

#[test]
fn address_to_facade_to_exports() {
    let mut s = session();
    let info = s.execute("document.info", Value::Null).unwrap();
    assert_eq!(info["buildings"][0]["storeys"], 5, "15.2 m / 3 m rounds to 5 storeys");
    assert_eq!(info["site"]["terrain_m_dvr90"], 12.0);
    let south = first_facade_facing(&mut s, "S");
    s.execute("facade.set_kind", json!({"facade": south, "kind": "street"})).unwrap();
    s.execute("facade.set_bays", json!({"facade": south, "count": 7})).unwrap();
    s.execute("facade.place", json!({"facade": south, "type": "window.casement.2"})).unwrap();
    s.execute("facade.place", json!({"facade": south, "type": "door.entrance", "levels": [0], "bays": [3]})).unwrap();
    s.execute("facade.place", json!({"facade": south, "type": "balcony.french", "levels": [1, 2, 3, 4], "bays": [1, 5]})).unwrap();
    let dir = std::env::temp_dir().join(format!("trigon-test-{}", std::process::id()));
    let glb = dir.join("m.glb");
    let ifc = dir.join("m.ifc");
    s.execute("export.gltf", json!({"path": glb})).unwrap();
    s.execute("export.ifc", json!({"path": ifc})).unwrap();
    let text = std::fs::read_to_string(&ifc).unwrap();
    assert_eq!(text.matches("IFCWINDOW(").count(), 7 * 5 - 1 - 8);
    assert_eq!(text.matches("IFCDOOR(").count(), 1 + 8);
    assert_eq!(text.matches("IFCRAILING(").count(), 8);
    assert!(text.contains("IFCMAPCONVERSION("));
    // Save, reopen, same model.
    let doc_path = dir.join("m.trigon");
    s.execute("document.save", json!({"path": doc_path})).unwrap();
    let before = s.doc.clone();
    s.execute("document.open", json!({"path": doc_path})).unwrap();
    assert_eq!(s.doc, before);
    assert_eq!(s.doc.journal.len(), 6);
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn failing_commands_leave_the_document_unchanged() {
    let mut s = session();
    let south = first_facade_facing(&mut s, "S");
    s.execute("facade.set_bays", json!({"facade": south, "count": 7})).unwrap();
    s.execute("facade.place", json!({"facade": south, "type": "window.casement.2"})).unwrap();
    let before = s.doc.clone();
    // Too wide for its bay: overlaps the neighbours.
    let err = s.execute("facade.place", json!({"facade": south, "type": "window.casement.3", "bays": [3], "width": 6000.0})).unwrap_err();
    assert!(err.to_string().contains("overlaps"), "{err}");
    assert_eq!(s.doc, before);
    // A party wall can't take openings, and the error says so.
    let err = s.execute("facade.set_kind", json!({"facade": south, "kind": "party"})).unwrap_err();
    assert!(err.to_string().contains("party wall"), "{err}");
}

#[test]
fn undo_and_redo() {
    let mut s = session();
    let south = first_facade_facing(&mut s, "S");
    s.execute("facade.set_bays", json!({"facade": south, "count": 4})).unwrap();
    let with_bays = s.doc.clone();
    s.execute("facade.place", json!({"facade": south, "type": "window.fixed"})).unwrap();
    assert_eq!(s.execute("edit.undo", json!({})).unwrap()["undone"], true);
    assert_eq!(s.doc, with_bays);
    s.execute("edit.redo", json!({})).unwrap();
    assert_eq!(s.doc.building(None).unwrap().facade(&south).unwrap().placements.len(), 4 * 5);
}

#[test]
fn photos_are_matched_to_facades() {
    let mut s = session();
    let v = s.execute("evidence.photos", json!({})).unwrap();
    // A camera looking north sees the south facade.
    let south = v["by_facade"].as_array().unwrap().iter().find(|f| f["faces"] == "S").unwrap();
    assert_eq!(south["photos"], json!(["p-north"]));
}

#[test]
fn hostile_params_never_panic() {
    let mut s = session();
    let south = first_facade_facing(&mut s, "S");
    let hostile = [
        json!(null),
        json!([]),
        json!("x"),
        json!({"facade": south, "count": 0}),
        json!({"facade": south, "count": 1_000_000}),
        json!({"facade": south, "centres": [f64::MAX, -1.0]}),
        json!({"facade": south, "type": "nope"}),
        json!({"facade": "b9.f9", "type": "window.fixed"}),
        json!({"facade": south, "type": "window.fixed", "levels": [99]}),
        json!({"facade": south, "type": "window.fixed", "width": -5.0}),
        json!({"count": usize::MAX}),
        json!({"path": "/etc/passwd"}),
        json!({"id": "x", "name": "x", "width": 1e300, "height": 1.0, "sill": 0.0, "kind": {"family": "window", "lights": 0}}),
    ];
    for c in commands() {
        for p in &hostile {
            if let Err(EngineError::Internal(id)) = s.execute(c.id, p.clone()) {
                panic!("{id} panicked on {p}");
            }
            if c.id == "document.new" || c.id == "document.open" {
                s = session();
            }
        }
    }
}

#[test]
fn every_command_has_an_object_schema() {
    for c in commands() {
        let schema = c.params_schema();
        assert_eq!(schema["type"], "object", "{}", c.id);
        assert!(!c.summary.is_empty());
    }
}

fn exported(s: &mut Session, tag: &str) -> (std::path::PathBuf, std::path::PathBuf) {
    let south = first_facade_facing(s, "S");
    s.execute("facade.set_bays", json!({"facade": south, "count": 7})).unwrap();
    s.execute("facade.place", json!({"facade": south, "type": "window.casement.2"})).unwrap();
    s.execute("facade.place", json!({"facade": south, "type": "balcony.cantilever", "levels": [1, 2], "bays": [3]})).unwrap();
    let dir = std::env::temp_dir().join(format!("trigon-{tag}-{}", std::process::id()));
    let ifc = dir.join("m.ifc");
    s.execute("export.ifc", json!({"path": ifc})).unwrap();
    (dir, ifc)
}

#[test]
fn both_ifc_readers_agree_with_the_model() {
    let mut s = session();
    let (dir, ifc) = exported(&mut s, "check");
    let v = s.execute("ifc.check", json!({"path": ifc})).unwrap();
    for r in v["readers"].as_array().unwrap() {
        assert_eq!(r["failures"], 0, "{r}");
    }
    let cmp = &v["ifc_lite_vs_bimifc"];
    assert_eq!(cmp["only_in_first"], 0);
    assert_eq!(cmp["only_in_second"], 0);
    assert!(cmp["max_bounds_deviation_mm"].as_f64().unwrap() < 0.01, "{cmp}");
    for m in v["versus_this_model"].as_array().unwrap() {
        assert_eq!(m["matched"], m["model_elements"], "{m}");
        assert!(m["max_bounds_deviation_mm"].as_f64().unwrap() < 0.01, "{m}");
    }
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn a_linked_georeferenced_export_lands_on_the_model() {
    let mut s = session();
    let (dir, ifc) = exported(&mut s, "link");
    let v = s.execute("reference.link", json!({"path": ifc, "name": "self"})).unwrap();
    assert_eq!(v["alignment"], "map");
    let built = trigon_model::build(&s.doc).unwrap();
    let mut all = trigon_model::Mesh::default();
    for b in &built {
        for w in &b.walls {
            all.append(&w.mesh);
            for o in &w.openings {
                for e in std::iter::once(&o.fill).chain(&o.attached) {
                    for (_, m) in &e.parts {
                        all.append(m);
                    }
                }
            }
        }
        for sl in &b.slabs {
            all.append(&sl.mesh);
        }
    }
    let (lo, hi) = all.bounds().unwrap();
    let ext = &v["extent_mm"];
    for k in 0..3 {
        assert!((ext[0][k].as_f64().unwrap() - lo[k]).abs() < 0.01, "min {k}: {ext} vs {lo:?}");
        assert!((ext[1][k].as_f64().unwrap() - hi[k]).abs() < 0.01, "max {k}: {ext} vs {hi:?}");
    }
    // The link is a stored, undoable change, and it shows up in the glTF.
    assert_eq!(s.execute("reference.list", json!({})).unwrap()[0]["name"], "self");
    let glb = dir.join("m.glb");
    s.execute("export.gltf", json!({"path": glb})).unwrap();
    let bytes = std::fs::read(&glb).unwrap();
    assert!(bytes.windows(14).any(|w| w == b"self/IFCWINDOW"));
    assert!(s.execute("reference.link", json!({"path": ifc, "name": "self"})).is_err(), "duplicate names are refused");
    s.execute("edit.undo", json!({})).unwrap();
    assert!(s.doc.references.is_empty());
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn bbr_sets_storeys_and_attributes() {
    let mut s = session();
    let info = s.execute("document.info", json!({})).unwrap();
    let b = &info["buildings"][0];
    assert_eq!(b["storeys"], 5);
    assert_eq!(b["storey_height_mm"], 3000.0, "15 m eaves over 5 BBR storeys");
    assert_eq!(b["attributes"]["year_built"], "1940");
    assert_eq!(b["attributes"]["roof_material"], "clay tiles (tegl)");
}

#[test]
fn facade_image_is_straightened_from_the_photo_that_shows_it() {
    let mut s = session();
    let south = first_facade_facing(&mut s, "S");
    s.execute("facade.set_bays", json!({"facade": south, "count": 6})).unwrap();
    s.execute("facade.place", json!({"facade": south, "type": "window.casement.2"})).unwrap();
    let dir = std::env::temp_dir().join(format!("trigon-img-{}", std::process::id()));
    let path = dir.join("south.png");
    let v = s.execute("evidence.facade_image", json!({"facade": south, "path": path, "mm_per_px": 100.0})).unwrap();
    assert_eq!(v["photo"], "p-north");
    assert_eq!(v["width_px"], 240, "24 m at 100 mm/px");
    assert_eq!(v["height_px"], 150, "15 m at 100 mm/px");
    assert_eq!(v["photo_coverage"], 1.0, "{v}");
    let bytes = std::fs::read(&path).unwrap();
    assert_eq!(&bytes[1..4], b"PNG");
    // The north facade faces away from the only camera: no photo shows it.
    let north = first_facade_facing(&mut s, "N");
    let err = s.execute("evidence.facade_image", json!({"facade": north, "path": dir.join("n.png")})).unwrap_err();
    assert!(err.to_string().contains("no photo shows"), "{err}");
    assert!(s.execute("evidence.facade_image", json!({"facade": south, "path": dir.join("x.txt")})).is_err(), "only .png paths");
    let _ = std::fs::remove_dir_all(dir);
}

/// A synthetic 1:100 elevation sheet held in the session's drawing cache (no PDF needed): white,
/// with a black vertical line every metre at 100 px/m starting at x = 200.
fn with_sheet(s: &mut Session) -> String {
    let (w, h) = (3000u32, 2000u32);
    let mut data = vec![255u8; (w * h) as usize];
    for y in 0..h {
        for x in (200..w).step_by(100) {
            data[(y * w + x) as usize] = 0;
        }
    }
    s.doc.drawings.push(trigon_model::Drawing {
        name: "sheet".into(),
        path: "/virtual/sheet.pdf".into(),
        page: 1,
        rotate: 0,
        width: w,
        height: h,
        note: String::new(),
        facades: Vec::new(),
    });
    s.drawing_cache = Some((("/virtual/sheet.pdf".into(), 1, 0), std::sync::Arc::new(trigon_drawing::Gray { width: w, height: h, data })));
    "sheet".into()
}

#[test]
fn drawings_register_straighten_and_view() {
    let mut s = session();
    let sheet = with_sheet(&mut s);
    let south = first_facade_facing(&mut s, "S");
    // Not registered yet: a clear refusal.
    let err = s.execute("evidence.facade_image", json!({"facade": south, "drawing": sheet, "path": "/tmp/x.png"})).unwrap_err();
    assert!(err.to_string().contains("register_facade"), "{err}");
    // 24 m facade from x = 200 to x = 2600 (100 px/m); terrain at y = 1800, 15 m at y = 300.
    let v = s
        .execute(
            "drawing.register_facade",
            json!({"name": sheet, "facade": south, "x_start": 200, "x_end": 2600, "y_levels": [[1800, 0], [300, 15000]]}),
        )
        .unwrap();
    assert_eq!(v["px_per_m_along"], 100.0);
    assert_eq!(v["px_per_m_up"], 100.0);
    let dir = std::env::temp_dir().join(format!("trigon-sheet-{}", std::process::id()));
    let out = dir.join("south.png");
    let v =
        s.execute("evidence.facade_image", json!({"facade": south, "drawing": sheet, "mm_per_px": 100.0, "overlay": false, "path": out})).unwrap();
    assert_eq!(v["drawing_coverage"], 1.0);
    // Every metre of facade lands on a drawn line: column 10 (u = 1000…1100 mm) holds the x = 300 line.
    let png = std::fs::read(&out).unwrap();
    assert_eq!(&png[1..4], b"PNG");
    let view = s
        .execute("drawing.view", json!({"name": sheet, "region": [0, 0, 1000, 1000], "max_px": 500, "grid": 250, "path": dir.join("v.png")}))
        .unwrap();
    assert_eq!(view["drawing_px_per_image_px"], 2);
    assert_eq!(view["grid_px"], 250.0);
    // Registration sanity checks.
    for bad in [
        json!({"name": sheet, "facade": south, "x_start": 200, "x_end": 205, "y_levels": [[1800, 0], [300, 15000]]}),
        json!({"name": sheet, "facade": south, "x_start": 200, "x_end": 2600, "y_levels": [[1800, 0], [1800, 15000]]}),
        json!({"name": "nope", "facade": south, "x_start": 200, "x_end": 2600, "y_levels": [[1800, 0], [300, 15000]]}),
        json!({"name": sheet, "facade": "b9.f9", "x_start": 200, "x_end": 2600, "y_levels": [[1800, 0], [300, 15000]]}),
    ] {
        assert!(s.execute("drawing.register_facade", bad.clone()).is_err(), "{bad}");
    }
    assert!(s.execute("drawing.view", json!({"name": sheet, "region": [10, 10, 10, 10]})).is_err(), "empty region");
    assert!(s.execute("drawing.add", json!({"path": "/etc/hosts"})).is_err(), "only .pdf files");
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn phases_past_present_future() {
    let mut s = session();
    let south = first_facade_facing(&mut s, "S");
    s.execute("facade.set_bays", json!({"facade": south, "count": 6})).unwrap();
    s.execute("facade.place", json!({"facade": south, "type": "door.french", "levels": [1], "bays": [2]})).unwrap();
    s.execute(
        "types.define",
        json!({"id": "balcony.old", "name": "Old balcony", "width": 2000.0, "height": 1000.0, "sill": 0.0, "kind": {"family": "balcony_slab", "depth": 1000.0, "thickness": 150.0}}),
    )
    .unwrap();
    s.execute("facade.place", json!({"facade": south, "type": "balcony.old", "levels": [1], "bays": [2]})).unwrap();
    // Phases: 1938 → 2026 → forslag. Everything so far belongs to the first.
    let v = s
        .execute(
            "phase.set",
            json!({"phases": [{"id": "1938", "name": "Opført"}, {"id": "2026", "name": "Eksisterende"}, {"id": "forslag", "name": "Forslag"}]}),
        )
        .unwrap();
    assert_eq!(v["active"], "forslag", "the last phase is active by default");
    // In the proposal, a MinAltan balcony replaces the old one in the same slot: the old is demolished, not deleted.
    let v = s.execute("facade.place", json!({"facade": south, "type": "balcony.minaltan", "levels": [1], "bays": [2]})).unwrap();
    assert_eq!(v["demolished"], 1);
    let f = s.doc.building(None).unwrap().facade(&south).unwrap().clone();
    assert_eq!(f.placements.len(), 3, "door, old balcony (demolished in forslag), new balcony");
    // Placing again in the same phase replaces the new one outright.
    let v = s.execute("facade.place", json!({"facade": south, "type": "balcony.minaltan", "levels": [1], "bays": [2], "width": 2400.0})).unwrap();
    assert_eq!(v["demolished"], 0);
    assert_eq!(s.doc.building(None).unwrap().facade(&south).unwrap().placements.len(), 3);
    // Demolishing the door in the proposal closes the wall there afterwards, but not before.
    s.execute("facade.demolish", json!({"facade": south, "levels": [1], "bays": [2], "what": "openings"})).unwrap();
    let doors_in = |s: &Session, phase: &str| {
        let view = crate::commands::view_of(&s.doc, Some(phase), None).unwrap();
        trigon_model::build::build_view(&s.doc, view).unwrap()[0].walls.iter().map(|w| w.openings.len()).sum::<usize>()
    };
    assert_eq!(doors_in(&s, "2026"), 1);
    assert_eq!(doors_in(&s, "forslag"), 0);
    // A phase that elements still use can't be dropped.
    assert!(s.execute("phase.set", json!({"phases": [{"id": "1938", "name": "x"}]})).is_err());
    // IFC: status per element.
    let dir = std::env::temp_dir().join(format!("trigon-phase-{}", std::process::id()));
    let ifc = dir.join("demo.ifc");
    s.execute("export.ifc", json!({"path": ifc, "phase": "forslag", "filter": "all"})).unwrap();
    let text = std::fs::read_to_string(&ifc).unwrap();
    assert!(text.contains("IFCLABEL('NEW')") && text.contains("IFCLABEL('DEMOLISH')"), "status properties");
    s.execute("export.gltf", json!({"path": dir.join("demo.glb"), "phase": "forslag", "filter": "demolition"})).unwrap();
    assert!(s.execute("export.gltf", json!({"path": dir.join("x.glb"), "filter": "sideways"})).is_err());
    let list = s.execute("phase.list", json!({})).unwrap();
    assert_eq!(list["phases"][2]["builds"], 1);
    assert_eq!(list["phases"][2]["demolishes"], 2, "old balcony and door");
    let _ = std::fs::remove_dir_all(dir);
}
