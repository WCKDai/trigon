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

#[test]
fn clearing_stair_walls_preserves_the_other_storeys() {
    let mut s = session();
    s.execute(
        "interior.add_walls",
        json!({"walls":[{"id":"shared", "levels":[0,1,2], "a":[-1000,0],"b":[1000,0],"thickness":100,"load_bearing":false}]}),
    )
    .unwrap();
    s.execute(
        "interior.add_stairs",
        json!({"stairs":[{"id":"s","levels":[1],"outline":[[-2000,-1000],[2000,-1000],[2000,1000],[-2000,1000]]}],"clear_walls_inside":true}),
    )
    .unwrap();
    let walls = &s.doc.buildings[0].interior_walls;
    assert_eq!(walls.len(), 1);
    assert_eq!(walls[0].levels, vec![0, 2]);
}

#[test]
fn stairs_and_basement_keep_levels_and_export_real_floor_openings() {
    let mut s = session();
    s.execute("interior.add_stairs",json!({"stairs":[{"id":"core","levels":[0,1,2,3,4],"outline":[[-2000,-1000],[0,-1000],[0,1000],[-2000,1000]],"kind":"u_turn","landing":1000,"rounded":false}]})).unwrap();
    s.execute("building.add_basement", json!({"floor":-2480})).unwrap();
    assert_eq!(s.doc.buildings[0].ground_level, 1);
    assert_eq!(s.doc.buildings[0].stairs[0].levels, vec![1, 2, 3, 4, 5]);
    assert_eq!(s.doc.buildings[0].storeys[1].elevation, 0.0);
    let built = trigon_model::build(&s.doc).unwrap();
    assert!(built[0].slabs[0].holes.is_empty());
    assert!(built[0].slabs[1].holes.is_empty());
    assert_eq!(built[0].slabs[2].holes.len(), 1);
    let text = trigon_io::ifc::write(&s.doc, &built, "test.ifc", "2000-01-01T00:00:00Z");
    let slab = &built[0].slabs[2];
    let expected = trigon_model::doc::polygon_area(&slab.polygon) - 6_000_000.0;
    for read in [trigon_ifcread::read_ifc_lite(&text).unwrap(), trigon_ifcread::read_bimifc(&text).unwrap()] {
        let id = trigon_io::ifc::guid(&format!("trigon:{}", slab.id));
        let m = &read.elements.iter().find(|e| e.global_id == id).unwrap().mesh;
        let top = m.bounds().unwrap().1[2];
        let area: f64 = m
            .triangles
            .iter()
            .filter_map(|t| {
                let [a, b, c] = t.map(|i| m.positions[i as usize]);
                ([a, b, c].iter().all(|p| (p[2] - top).abs() < 1e-3))
                    .then_some(((b[0] - a[0]) * (c[1] - a[1]) - (b[1] - a[1]) * (c[0] - a[0])).abs() / 2.0)
            })
            .sum();
        assert!((area - expected).abs() < expected * 1e-6, "{} filled stair shaft: {area} vs {expected}", read.reader.name());
    }
}

#[test]
fn malformed_stairs_are_rejected_without_mutating_the_document() {
    let mut s = session();
    for stair in [
        json!({"levels":[0],"outline":[[0,0],[2000,0],[2000,2000]]}),
        json!({"levels":[0],"outline":[[0,0],[2000,0],[2400,2000],[0,2000]]}),
        json!({"levels":[],"outline":[[0,0],[2000,0],[2000,2000],[0,2000]]}),
        json!({"levels":[0],"outline":[[0,0],[2000,0],[2000,2000],[0,2000]],"winders":20000000}),
    ] {
        let before = s.doc.clone();
        assert!(s.execute("interior.add_stairs", json!({"stairs":[stair]})).is_err());
        assert_eq!(s.doc, before);
    }
    let valid = json!({"levels":[0],"outline":[[0,0],[2000,0],[2000,2000],[0,2000]]});
    s.execute("interior.add_stairs", json!({"stairs":[valid.clone()]})).unwrap();
    let before = s.doc.clone();
    assert!(s.execute("interior.add_stairs", json!({"stairs":[valid]})).is_err(), "overlapping shafts");
    assert_eq!(s.doc, before);
    assert!(
        s.execute("interior.add_stairs", json!({"stairs":[{"levels":[0],"outline":[[20000,0],[22000,0],[22000,2000],[20000,2000]]}]})).is_err(),
        "outside building"
    );
    assert_eq!(s.doc, before);
}

#[test]
fn thick_gray_masonry_is_not_mistaken_for_its_own_paper_background() {
    let mut s = session();
    let (w, h) = (3000u32, 2200u32);
    let mut data = vec![210u8; (w * h) as usize];
    for y in 1150..1250 {
        for x in 700..2300 {
            data[(y * w + x) as usize] = 120;
        }
    }
    s.doc.drawings.push(trigon_model::Drawing {
        name: "thick".into(),
        path: "/virtual/thick.pdf".into(),
        page: 1,
        rotate: 0,
        width: w,
        height: h,
        note: String::new(),
        facades: Vec::new(),
        plans: vec![trigon_model::PlanRegistration::fit(vec![1], &[([300., 1800.], [-12000., -6000.]), ([2700., 1800.], [12000., -6000.])]).unwrap()],
    });
    s.drawing_cache = Some((("/virtual/thick.pdf".into(), 1, 0), std::sync::Arc::new(trigon_drawing::Gray { width: w, height: h, data })));
    let v = s.execute("drawing.trace_walls", json!({"name":"thick","level":1,"contrast":45,"max_mm":1100})).unwrap();
    let walls = v["walls"].as_array().unwrap();
    assert!(
        walls.iter().any(|w| {
            let (a, b) = (&w["a"], &w["b"]);
            w["thickness"].as_f64().unwrap() > 850.0 && (a[0].as_f64().unwrap() - b[0].as_f64().unwrap()).abs() > 15000.0
        }),
        "the solid 1m wall was split or lost: {walls:?}"
    );
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
        json!({"thickness": [f64::MAX, -1.0, 0.0]}),
        json!({"preset": "copenhagen_masonry", "facades": [south]}),
        json!({"facades": [south, "b9.f9"], "thickness": []}),
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
        plans: Vec::new(),
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

#[test]
fn wall_regularization_is_explicit_undoable_and_atomic() {
    let mut s = session();
    s.execute(
        "interior.add_walls",
        json!({"walls": [
            {"id":"measured", "levels":[1], "a":[0.,0.], "b":[4000.,80.], "thickness":100.}
        ]}),
    )
    .unwrap();
    let original = s.doc.clone();
    assert!(s.execute("interior.regularize", json!({"axes_deg":[0.], "tolerance_mm": -1.})).is_err());
    assert_eq!(s.doc, original);
    let result = s.execute("interior.regularize", json!({"axes_deg":[0.]})).unwrap();
    assert_eq!(result["moved"], 1);
    let wall = &s.doc.buildings[0].interior_walls[0];
    assert!((wall.a[1] - wall.b[1]).abs() < 1e-8);
    assert!(s.undo());
    assert_eq!(s.doc.buildings, original.buildings);
}

#[test]
fn presentation_sheets_are_vector_composed_and_leave_the_model_unchanged() {
    let mut s = session();
    let facade = first_facade_facing(&mut s, "S");
    s.execute("facade.set_bays", json!({"facade": facade, "count": 2})).unwrap();
    s.execute("facade.place", json!({"facade": facade, "type": "balcony.cantilever", "levels": [1], "bays": [0]})).unwrap();
    s.execute("facade.place", json!({"facade": facade, "type": "balcony.french", "levels": [1], "bays": [1]})).unwrap();
    let before = s.doc.clone();
    let dir = std::env::temp_dir().join(format!("trigon-sheet-{}", std::process::id()));
    let request = json!({"levels": [1], "path": dir.join("sheet.svg"), "caption": "Frederikssundsvej & gård"});
    let result = s.execute("plan.sheet", request).unwrap();
    assert_eq!(result["pages"], 1);
    let svg = std::fs::read_to_string(dir.join("sheet.svg")).unwrap();
    assert!(svg.contains("url(#masonry)") && svg.contains("fill-rule=\"evenodd\""));
    assert!(svg.contains("Frederikssundsvej &amp; gård") && svg.contains("Liberation Sans"));
    assert!(svg.contains("Ca. 1200 × 3000"), "the balcony's slab width differs from its 1000 mm door opening");
    assert!(!svg.contains("data:image") && !svg.contains(" m²"), "offer sheets have neither a scan nor derived room-area clutter");
    s.execute("plan.sheet", json!({"levels": [1], "path": dir.join("sheet.png")})).unwrap();
    let reader = png::Decoder::new(std::io::BufReader::new(std::fs::File::open(dir.join("sheet.png")).unwrap())).read_info().unwrap();
    assert_eq!((reader.info().width, reader.info().height), (1600, 900));
    s.execute("plan.sheet", json!({"levels": [1, 2], "path": dir.join("set.pdf")})).unwrap();
    let pdf = lopdf::Document::load(dir.join("set.pdf")).unwrap();
    assert_eq!(pdf.get_pages().len(), 2);
    assert_eq!(s.doc, before);
    for request in [
        json!({"levels": [], "path": dir.join("bad.pdf")}),
        json!({"levels": [1, 2], "path": dir.join("bad.png")}),
        json!({"levels": [1], "path": dir.join("bad.pdf"), "region": [[0.,0.],[0.,0.]]}),
        json!({"levels": [999], "path": dir.join("bad.pdf")}),
    ] {
        assert!(s.execute("plan.sheet", request).is_err());
    }
    assert!(!dir.join("bad.pdf").exists());
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn elevation_sheets_project_the_model_and_label_balconies() {
    let mut s = session();
    let facade = first_facade_facing(&mut s, "S");
    s.execute("facade.set_bays", json!({"facade": facade, "count": 2})).unwrap();
    s.execute("facade.place", json!({"facade": facade, "type": "balcony.cantilever", "levels": [1], "bays": [0]})).unwrap();
    let before = s.doc.clone();
    let dir = std::env::temp_dir().join(format!("trigon-elevation-{}", std::process::id()));
    let result = s.execute("elevation.sheet", json!({"facade": facade, "path": dir.join("e.svg"), "title": "Gårdfacade"})).unwrap();
    assert!(result["triangles"].as_u64().unwrap() > 10, "{result}");
    assert_eq!(result["balcony_labels"], 1);
    let svg = std::fs::read_to_string(dir.join("e.svg")).unwrap();
    assert!(svg.contains("Ca. 1200 × 3000") && svg.contains("Gårdfacade") && !svg.contains("data:image"));
    s.execute("elevation.sheet", json!({"facade": facade, "path": dir.join("e.pdf")})).unwrap();
    // Square-on to a line instead of a facade (a stepped street front): south of the building, looking north.
    let along = s.execute("elevation.sheet", json!({"along": [[-60000., -60000.], [60000., -60000.]], "path": dir.join("s.svg")})).unwrap();
    assert!(along["triangles"].as_u64().unwrap() > 10, "{along}");
    assert_eq!(lopdf::Document::load(dir.join("e.pdf")).unwrap().get_pages().len(), 1);
    assert_eq!(s.doc, before);
    for request in [
        json!({"facade": "nope", "path": dir.join("bad.svg")}),
        json!({"facade": facade, "path": dir.join("bad.svg"), "from_mm": 500.0, "to_mm": 600.0}),
        json!({"facade": facade, "path": dir.join("bad.svg"), "from_mm": f64::MAX}),
        json!({"facade": facade, "path": dir.join("bad.txt")}),
        json!({"path": dir.join("bad.svg")}),
        json!({"facade": facade, "along": [[0., 0.], [5000., 0.]], "path": dir.join("bad.svg")}),
        json!({"along": [[0., 0.], [0., 0.]], "path": dir.join("bad.svg")}),
        json!({"along": [[0., 0.], [f64::MAX, 0.]], "path": dir.join("bad.svg")}),
    ] {
        assert!(s.execute("elevation.sheet", request).is_err());
    }
    assert!(!dir.join("bad.svg").exists());
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn floor_plans_from_a_registered_sheet() {
    let mut s = session();
    let sheet = with_sheet(&mut s);
    let south = first_facade_facing(&mut s, "S");
    s.execute("facade.set_bays", json!({"facade": south, "count": 6})).unwrap();
    s.execute(
        "types.define",
        json!({"id": "dw", "name": "Door + window", "width": 1840.0, "height": 2150.0, "sill": 0.0,
               "kind": {"family": "door_window", "door_width": 880.0, "door_side": "left", "window_sill": 750.0, "lights": 2}}),
    )
    .unwrap();
    s.execute("facade.place", json!({"facade": south, "type": "dw", "levels": [1], "bays": [1, 4]})).unwrap();
    s.execute("facade.place", json!({"facade": south, "type": "window.casement.2", "levels": [1], "bays": [0, 2, 3, 5]})).unwrap();
    s.execute("facade.place", json!({"facade": south, "type": "balcony.minaltan", "levels": [1], "bays": [1, 4]})).unwrap();
    // The outline lists the corners in site mm.
    let outline = s.execute("building.outline", json!({})).unwrap();
    assert_eq!(outline.as_array().unwrap().len(), 4);
    // Register: the sheet is 100 px/m, rotated so the south facade runs along +x; two facade points and one site point.
    let fr_len =
        s.execute("facade.list", json!({})).unwrap().as_array().unwrap().iter().find(|f| f["id"] == south).unwrap()["length_mm"].as_f64().unwrap();
    let v = s
        .execute(
            "drawing.register_plan",
            json!({"name": sheet, "levels": [1], "points": [
                {"px": 200, "py": 1500, "facade": south, "u": 0.0},
                {"px": 200.0 + fr_len / 10.0, "py": 1500, "facade": south, "u": fr_len}
            ]}),
        )
        .unwrap();
    assert!((v["px_per_m"].as_f64().unwrap() - 100.0).abs() < 0.01, "{v}");
    assert_eq!(v["residuals_mm"], json!([0.0, 0.0]));
    let dir = std::env::temp_dir().join(format!("trigon-plan-{}", std::process::id()));
    let png = s.execute("plan.image", json!({"level": 1, "path": dir.join("p.png"), "mm_per_px": 50.0, "show_scan": true})).unwrap();
    assert_eq!(png["underlay"], "sheet");
    let clean = s.execute("plan.image", json!({"level": 1, "path": dir.join("clean.png"), "mm_per_px": 50.0})).unwrap();
    assert!(clean["underlay"].is_null(), "output material shows no scan by default");
    assert_eq!(png["labelled_balconies"], 2);
    let svg = s.execute("plan.image", json!({"level": 1, "path": dir.join("p.svg"), "mm_per_px": 50.0, "show_scan": true})).unwrap();
    let text = std::fs::read_to_string(svg["written"].as_str().unwrap()).unwrap();
    assert!(text.contains("Ca. 1300 x 2700") && text.contains("<polygon") && text.contains("data:image/png;base64,"));
    // No plan registered for the ground floor: a plain plan, or a clear error when a drawing is asked for.
    assert!(s.execute("plan.image", json!({"level": 0, "path": dir.join("g.png")})).unwrap()["underlay"].is_null());
    assert!(s.execute("plan.image", json!({"level": 0, "drawing": sheet, "show_scan": true, "path": dir.join("g.png")})).is_err());
    let plain = s.execute("plan.image", json!({"level": 1, "path": dir.join("plain.svg")})).unwrap();
    assert!(!std::fs::read_to_string(plain["written"].as_str().unwrap()).unwrap().contains("base64"), "no scan in the SVG");
    assert!(s.execute("plan.image", json!({"level": 9, "path": dir.join("x.png")})).is_err());
    assert!(s.execute("plan.image", json!({"level": 1, "path": dir.join("x.pdf")})).is_err());
    assert!(s.execute("drawing.register_plan", json!({"name": sheet, "levels": [1], "points": [{"px": 1, "py": 1, "x": 0.0, "y": 0.0}]})).is_err());
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn interior_walls_traced_from_a_plan_sheet() {
    let mut s = session();
    // A 1:100 plan at 100 px/m, north up: site (x, y) mm ↔ pixel (1500 + x/10, 1200 − y/10).
    let (w, h) = (3000u32, 2400u32);
    let mut data = vec![215u8; (w * h) as usize];
    // A 200 mm wall along x at y = 0 from x = −8 m to +8 m, drawn with a 1 px step half way (as scans do), and a 100 mm
    // partition along y at x = 4 m.
    for x in 700..2300 {
        let shift = if x < 1500 { 0 } else { 1 };
        for y in 1190 + shift..1210 + shift {
            data[(y * w + x) as usize] = 30;
        }
    }
    for y in 700..1190 {
        for x in 1895..1905 {
            data[(y * w + x) as usize] = 40;
        }
    }
    s.doc.drawings.push(trigon_model::Drawing {
        name: "plan".into(),
        path: "/virtual/plan.pdf".into(),
        page: 1,
        rotate: 0,
        width: w,
        height: h,
        note: String::new(),
        facades: Vec::new(),
        plans: Vec::new(),
    });
    s.drawing_cache = Some((("/virtual/plan.pdf".into(), 1, 0), std::sync::Arc::new(trigon_drawing::Gray { width: w, height: h, data })));
    s.execute(
        "drawing.register_plan",
        json!({"name": "plan", "levels": [1, 2], "points": [{"px": 300, "py": 1800, "x": -12000.0, "y": -6000.0}, {"px": 2700, "py": 1800, "x": 12000.0, "y": -6000.0}]}),
    )
    .unwrap();
    let v = s.execute("drawing.trace_walls", json!({"name": "plan", "level": 1})).unwrap();
    let walls = v["walls"].as_array().unwrap();
    let main = walls.iter().find(|w| w["thickness"].as_f64().unwrap() >= 150.0).expect("the 200 mm wall");
    let (a, b) = (main["a"].as_array().unwrap(), main["b"].as_array().unwrap());
    let len = ((b[0].as_f64().unwrap() - a[0].as_f64().unwrap()).powi(2) + (b[1].as_f64().unwrap() - a[1].as_f64().unwrap()).powi(2)).sqrt();
    assert!((len - 16000.0).abs() < 200.0, "length {len}");
    assert!(a[1].as_f64().unwrap().abs() < 30.0 && b[1].as_f64().unwrap().abs() < 30.0, "on y = 0 after merging the stepped pieces: {main}");
    assert_eq!(main["load_bearing"], true);
    assert!(walls.iter().any(|w| (w["thickness"].as_f64().unwrap() - 100.0).abs() <= 30.0), "the partition: {v}");
    // Add them on two storeys; they export as IfcWall and draw in the plan.
    let v = s.execute("drawing.trace_walls", json!({"name": "plan", "level": 1, "add": true, "levels": [1, 2]})).unwrap();
    assert!(v["added"].as_u64().unwrap() >= 2);
    let list = s.execute("interior.list", json!({})).unwrap();
    assert!(list[1]["load_bearing"].as_u64().unwrap() >= 1 && list[2]["partitions"].as_u64().unwrap() >= 1);
    s.execute(
        "interior.add_rooms",
        json!({"rooms": [{"levels": [1], "name": "Stue", "polygon": [[-8000.0, 0.0], [4000.0, 0.0], [4000.0, 5000.0], [-8000.0, 5000.0]]}]}),
    )
    .unwrap();
    // drawing.to_site: pixel (1500, 1200) is the site origin on this sheet.
    let q = s.execute("drawing.to_site", json!({"name": "plan", "level": 1, "points": [[1500.0, 1200.0], [1600.0, 1100.0]]})).unwrap();
    assert_eq!(q, json!([[0.0, 0.0], [1000.0, 1000.0]]));
    // A wall drawn inside a staircase (a traced rail) goes when the stair is added with clear_walls_inside.
    s.execute("interior.add_walls", json!({"walls": [{"levels": [1], "a": [5500.0, 1500.0], "b": [7500.0, 1500.0], "thickness": 80.0}]})).unwrap();
    let before = s.doc.building(None).unwrap().interior_walls.len();
    let v = s
        .execute(
            "interior.add_stairs",
            json!({"stairs": [{"levels": [1, 2], "outline": [[5000.0, 500.0], [8000.0, 500.0], [8000.0, 2700.0], [5000.0, 2700.0]]}], "clear_walls_inside": true}),
        )
        .unwrap();
    assert_eq!(v["walls_removed"], 1);
    assert_eq!(s.doc.building(None).unwrap().interior_walls.len(), before - 1);
    assert_eq!(s.execute("interior.list", json!({})).unwrap()[1]["rooms"][0]["area_m2"], 60.0);
    let dir = std::env::temp_dir().join(format!("trigon-interior-{}", std::process::id()));
    s.execute("export.ifc", json!({"path": dir.join("m.ifc")})).unwrap();
    let ifc = std::fs::read_to_string(dir.join("m.ifc")).unwrap();
    assert!(ifc.contains(".PARTITIONING.") && ifc.contains("Interior wall (load-bearing)"));
    let svg = s.execute("plan.image", json!({"level": 1, "path": dir.join("p.svg")})).unwrap();
    let text = std::fs::read_to_string(svg["written"].as_str().unwrap()).unwrap();
    assert!(text.contains(">Stue<") && text.contains("60.0 m²"));
    // Bad input is refused.
    assert!(s.execute("interior.add_walls", json!({"walls": [{"levels": [9], "a": [0.0, 0.0], "b": [1000.0, 0.0], "thickness": 100.0}]})).is_err());
    assert!(s.execute("interior.add_walls", json!({"walls": [{"levels": [1], "a": [0.0, 0.0], "b": [0.0, 0.0], "thickness": 100.0}]})).is_err());
    assert!(s.execute("drawing.trace_walls", json!({"name": "plan", "level": 3})).is_err(), "no plan registered for level 3");
    s.execute("interior.clear", json!({})).unwrap();
    assert_eq!(s.execute("interior.list", json!({})).unwrap()[1]["load_bearing"], 0);
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn masonry_walls_step_inside_at_every_floor_and_interior_walls_join_them() {
    let mut s = session();
    s.execute("building.add_basement", json!({"floor": -2480})).unwrap();
    let storeys = s.doc.buildings[0].storeys.len();
    let v = s.execute("building.set_wall_schedule", json!({"preset": "copenhagen_masonry"})).unwrap();
    let schedule: Vec<f64> = v["schedule"].as_array().unwrap().iter().map(|e| e["thickness_mm"].as_f64().unwrap()).collect();
    assert_eq!(schedule.len(), storeys);
    // Thinner going up; the basement is half a brick more than the ground storey; 360 on top.
    assert!(schedule.windows(2).all(|w| w[0] >= w[1]), "{schedule:?}");
    assert_eq!(schedule[0], schedule[1] + 100.0, "{schedule:?}");
    assert_eq!(*schedule.last().unwrap(), 360.0);

    // Each facade is one closed piece per storey at that storey's thickness; the floors rest on the steps.
    let built = trigon_model::build(&s.doc).unwrap();
    for w in &built[0].walls {
        let t: Vec<f64> = w.segments.iter().map(|g| g.thickness).collect();
        assert_eq!(t, schedule, "{}", w.id);
    }
    let area = |l: usize| trigon_model::doc::polygon_area(&built[0].slabs[l].polygon);
    assert!(area(0) < area(1) && area(1) < area(storeys - 1));

    // An interior wall drawn 650 mm in from the facade's outer face ends on the inner face on
    // every storey: trimmed where the wall is thicker (700), extended where it is thinner.
    let south = first_facade_facing(&mut s, "S");
    let b = &s.doc.buildings[0];
    let f = b.facade(&south).unwrap();
    let (a, e) = b.edge(f.edge).unwrap();
    let fr = trigon_model::FacadeFrame::new(a, e).unwrap();
    let start = fr.point(fr.length / 2.0, 650.0, 0.0);
    let end = fr.point(fr.length / 2.0, 4000.0, 0.0);
    s.execute(
        "interior.add_walls",
        json!({"walls": [{"id": "x", "levels": [0, 1, 2], "a": [start[0], start[1]], "b": [end[0], end[1]], "thickness": 120, "load_bearing": false}]}),
    )
    .unwrap();
    let built = trigon_model::build(&s.doc).unwrap();
    for (level, thickness) in schedule.iter().enumerate().take(3) {
        let iw = built[0].interior.iter().find(|w| w.id == format!("x.L{level}")).unwrap();
        let depth = |p: [f64; 2]| (p[0] - a[0]) * -fr.n[0] + (p[1] - a[1]) * -fr.n[1];
        assert!((depth(iw.axis[0]) - thickness).abs() < 1e-6, "level {level}: {:?}", iw.axis);
        assert!(matches!(iw.ends[0], Some(trigon_model::joins::Join { target: trigon_model::joins::Target::Facade(_), .. })));
    }

    // A fire gable keeps its own thickness; the IFC has one wall per facade and storey.
    s.execute("building.set_wall_schedule", json!({"facades": [south.clone()], "thickness": vec![360.0; storeys]})).unwrap();
    let b = &s.doc.buildings[0];
    let edge = b.facade(&south).unwrap().edge;
    assert!((0..storeys).all(|l| b.edge_thickness(edge, l) == 360.0));
    let built = trigon_model::build(&s.doc).unwrap();
    let text = trigon_io::ifc::write(&s.doc, &built, "t.ifc", "2000-01-01T00:00:00Z");
    assert_eq!(text.matches("IFCWALL(").count(), built[0].walls.len() * storeys + built[0].interior.len());

    // Refused: wrong count, hostile values, preset with facades. The document is unchanged.
    let before = s.doc.clone();
    for bad in [
        json!({"thickness": [500.0]}),
        json!({"thickness": vec![f64::MAX; storeys]}),
        json!({"preset": "copenhagen_masonry", "thickness": vec![400.0; storeys]}),
        json!({"facades": [south], "preset": "copenhagen_masonry"}),
        json!({"facades": ["b1.f999"], "thickness": vec![400.0; storeys]}),
        json!({}),
    ] {
        assert!(s.execute("building.set_wall_schedule", bad.clone()).is_err(), "{bad}");
    }
    assert_eq!(s.doc, before);
}

#[test]
fn door_gaps_become_doors_in_one_wall() {
    let mut s = session();
    s.execute(
        "interior.add_walls",
        json!({"walls": [
            {"id": "w1", "levels": [1], "a": [-4000, 0], "b": [-400, 0], "thickness": 120},
            {"id": "w2", "levels": [1], "a": [400, 0], "b": [4000, 0], "thickness": 120}
        ]}),
    )
    .unwrap();
    let v = s.execute("interior.doors_from_gaps", json!({})).unwrap();
    assert_eq!(v["doors_between_pieces"], 1, "{v}");
    let walls = &s.doc.buildings[0].interior_walls;
    assert_eq!(walls.len(), 1);
    assert_eq!(walls[0].doors.len(), 1);
    assert!((walls[0].doors[0].width - 800.0).abs() < 1e-6 && (walls[0].doors[0].at - 4000.0).abs() < 1e-6);

    // Built: one wall with a door (frame and leaf), the plan cut left open at the door.
    let built = trigon_model::build(&s.doc).unwrap();
    let w = built[0].interior.iter().find(|w| w.id == "w1.L1").unwrap();
    assert_eq!(w.doors.len(), 1);
    assert_eq!(w.cut.len(), 2, "the cut plan is two pieces either side of the door");
    assert_eq!(w.doors[0].fill.class, trigon_model::ElementClass::Door);
    assert!(s.execute("plan.image", json!({"level": 1, "path": std::env::temp_dir().join("trigon-doors.svg")})).is_ok());

    // Undo restores the two pieces; a second run finds nothing more.
    s.execute("edit.undo", json!({})).unwrap();
    assert_eq!(s.doc.buildings[0].interior_walls.len(), 2);
    s.execute("edit.redo", json!({})).unwrap();
    let again = s.execute("interior.doors_from_gaps", json!({})).unwrap();
    assert_eq!(again["doors_between_pieces"], 0);
    // Refused: max below min, hostile numbers.
    for bad in [json!({"min_mm": 900, "max_mm": 700}), json!({"min_mm": f64::MAX}), json!({"height_mm": -1})] {
        assert!(s.execute("interior.doors_from_gaps", bad.clone()).is_err(), "{bad}");
    }
}

#[test]
fn plinth_and_roof_build_up_are_part_of_the_model() {
    let mut s = session();
    let gable = s.doc.buildings[0].facades[0].id.clone();
    s.execute(
        "building.set_roof",
        json!({"kind": "pitched", "pitch_deg": 40, "gables": [gable], "overhang_mm": 300, "verge_mm": 100, "thickness_mm": 250, "gutters": true}),
    )
    .unwrap();
    s.execute("building.set_plinth", json!({"height_mm": 600})).unwrap();
    let built = trigon_model::build(&s.doc).unwrap();
    let roof = built[0].roof.as_ref().unwrap();
    assert!(!roof.gutters.is_empty());
    // The roof reaches past the footprint; the plinth stands proud of every wall up to 600 mm.
    let (lo, hi) = roof.cover.bounds().unwrap();
    let fp = &s.doc.buildings[0].footprint;
    let minx = fp.iter().map(|p| p[0]).fold(f64::MAX, f64::min);
    assert!(lo[0] < minx - 50.0 && hi[2] > lo[2]);
    for w in &built[0].walls {
        let (plo, phi) = w.plinth.bounds().unwrap();
        assert!((phi[2] - 600.0).abs() < 1e-6 && plo[2] < 0.0, "{}", w.id);
    }
    // Both export.
    let dir = std::env::temp_dir().join(format!("trigon-plinth-{}", std::process::id()));
    s.execute("export.gltf", json!({"path": dir.join("m.glb")})).unwrap();
    s.execute("export.ifc", json!({"path": dir.join("m.ifc")})).unwrap();
    let _ = std::fs::remove_dir_all(dir);
    // Removed again; refused when unclear or hostile.
    s.execute("building.set_plinth", json!({"remove": true})).unwrap();
    assert!(trigon_model::build(&s.doc).unwrap()[0].walls.iter().all(|w| w.plinth.is_empty()));
    for bad in [json!({}), json!({"height_mm": 600, "remove": true}), json!({"height_mm": f64::MAX}), json!({"height_mm": 600, "projection_mm": -5})]
    {
        assert!(s.execute("building.set_plinth", bad.clone()).is_err(), "{bad}");
    }
    assert!(s.execute("building.set_roof", json!({"kind": "pitched", "pitch_deg": 40, "overhang_mm": -1})).is_err());
}
