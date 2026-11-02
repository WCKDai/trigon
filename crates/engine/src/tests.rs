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
    fn terrain(&self, origin: [f64; 2], spacing: [f64; 2], columns: u32, rows: u32) -> trigon_data::Result<Vec<Vec<f64>>> {
        assert!((origin[0] - (721989.65 - 20.0)).abs() < 1e-6);
        assert!((origin[1] - (6178579.47 - 10.0)).abs() < 1e-6);
        assert_eq!(spacing, [20.0, 10.0]);
        Ok((0..rows).map(|row| (0..columns).map(|col| 12.0 + f64::from(col) * 0.1 + f64::from(row) * 0.2).collect()).collect())
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
fn explicit_roof_floor_keeps_eaves_and_reuses_the_attic_slab() {
    let mut s = session();
    s.execute("building.set_roof", json!({"kind":"pitched","pitch_deg":35})).unwrap();
    let eaves = s.doc.buildings[0].height();
    let floors = s.doc.buildings[0].storeys.clone();
    s.execute("building.set_attic", json!({"floor":eaves - 750.0,"name":"Tagkvist"})).unwrap();
    let b = &s.doc.buildings[0];
    assert_eq!(b.height(), eaves);
    assert_eq!(b.storeys.len(), floors.len() + 1);
    let built = trigon_model::build(&s.doc).unwrap();
    let slab = built[0].slabs.iter().find(|slab| slab.id == format!("{}.attic", b.id)).unwrap();
    assert_eq!(slab.z_bottom + slab.thickness, eaves - 750.0);
    assert_eq!(slab.level, floors.len());
    assert_eq!(built[0].slabs.len(), floors.len() + 1, "no duplicate eaves slab");
}

#[test]
fn reducing_storeys_prunes_facade_band_levels_and_undo_restores_them() {
    for (command, params) in
        [("building.set_storeys", json!({"count":2,"height":3000})), ("building.set_levels", json!({"floors":[0,3000],"top":6000}))]
    {
        let mut s = session();
        let facade = s.doc.buildings[0].facades[0].id.clone();
        s.execute(
            "element.set",
            json!({"id":facade,"properties":{"bands":[
                {"levels":[0,1,4],"bottom":2000,"top":2200,"projection":30,"finish":"yellow_brick"},
                {"levels":[4],"bottom":2500,"top":2700,"projection":40}
            ]}}),
        )
        .unwrap();
        let before = serde_json::to_vec(&s.doc).unwrap();
        s.execute(command, params).unwrap();
        let bands = &s.doc.buildings[0].facades[0].bands;
        assert_eq!(bands.len(), 1, "bands with no remaining occurrences must be removed");
        assert_eq!(bands[0].levels, vec![0, 1]);
        assert_eq!(bands[0].finish, Some(trigon_model::doc::WallFinish::YellowBrick));
        trigon_model::build(&s.doc).unwrap();
        let after = serde_json::to_vec(&s.doc).unwrap();
        assert_eq!(s.execute("edit.undo", json!({})).unwrap()["undone"], true);
        assert_eq!(serde_json::to_vec(&s.doc).unwrap(), before);
        assert_eq!(s.execute("edit.redo", json!({})).unwrap()["redone"], true);
        assert_eq!(serde_json::to_vec(&s.doc).unwrap(), after);
    }
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

#[test]
fn studio_plan_matches_the_exported_vector_sheet() {
    let mut s = session();
    let path = std::env::temp_dir().join(format!("trigon-live-plan-{}.svg", std::process::id()));
    let before = s.doc.clone();
    s.execute("plan.sheet", json!({"path": path, "levels": [0]})).unwrap();
    let live = crate::plan_sheet_svg(&s.doc, 0, trigon_model::build::View::latest(&s.doc)).unwrap();
    assert_eq!(live, std::fs::read_to_string(&path).unwrap());
    assert_eq!(s.doc, before);
    assert!(crate::plan_sheet_svg(&s.doc, usize::MAX, trigon_model::build::View::latest(&s.doc)).is_err());
    let _ = std::fs::remove_file(path);
}

#[test]
fn repeated_facade_indices_place_each_slot_once() {
    let mut s = session();
    let facade = first_facade_facing(&mut s, "S");
    s.execute("facade.set_bays", json!({"facade": facade, "count": 3})).unwrap();
    let out = s.execute("facade.place", json!({"facade": facade, "type": "window.casement.2", "levels": [0,0,1,1], "bays": [0,0,1,1]})).unwrap();
    assert_eq!(out["placed"], 4);
    assert_eq!(s.doc.buildings[0].facade(&facade).unwrap().placements.len(), 4);
}

fn editing_wall(s: &mut Session, levels: &[usize]) {
    s.execute("interior.add_walls", json!({"walls":[{"id":"shared","levels":levels,"a":[-4000,0],"b":[-2000,0],"thickness":120}]})).unwrap();
}

#[test]
fn editing_one_storey_preserves_the_other_instances_and_undo() {
    let mut s = session();
    editing_wall(&mut s, &[0, 1]);
    let before = s.doc.clone();
    let out = s.execute("edit.move", json!({"targets":["shared.L1"],"delta":[0,125.5,0]})).unwrap();
    let walls = &s.doc.buildings[0].interior_walls;
    assert_eq!(walls.len(), 2);
    assert!(walls.iter().any(|w| w.id == "shared" && w.levels == [0] && w.a == [-4000.0, 0.0]));
    assert!(walls.iter().any(|w| w.levels == [1] && w.a == [-4000.0, 125.5]));
    assert_eq!(out["targets"].as_array().unwrap().len(), 1);
    s.execute("edit.undo", json!({})).unwrap();
    assert_eq!(s.doc, before);
    s.execute("edit.redo", json!({})).unwrap();
    assert_eq!(s.doc.buildings[0].interior_walls.len(), 2);
}

#[test]
fn wall_alignment_makes_axes_collinear_preserving_length_doors_other_storeys_and_exact_undo() {
    let mut s = session();
    s.execute(
        "interior.add_walls",
        json!({"walls":[
            {"id":"reference","levels":[0,1],"a":[-4000,1000],"b":[-2000,1000],"thickness":200},
            {"id":"target","levels":[0,1],"a":[-4000,50],"b":[-2000,75],"thickness":120,"doors":[{"at":650,"width":700}]}
        ]}),
    )
    .unwrap();
    let before = s.doc.clone();
    let target = before.buildings[0].interior_walls.iter().find(|w| w.id == "target").unwrap();
    let length = (target.b[0] - target.a[0]).hypot(target.b[1] - target.a[1]);
    let out = s.execute("edit.align_walls", json!({"targets":["target.L1"],"reference":"reference.L1"})).unwrap();
    assert_eq!(out["targets"].as_array().unwrap().len(), 1);
    let walls = &s.doc.buildings[0].interior_walls;
    assert_eq!(walls.iter().find(|w| w.id == "reference"), before.buildings[0].interior_walls.iter().find(|w| w.id == "reference"));
    let ground = walls.iter().find(|w| w.id == "target").unwrap();
    assert_eq!(ground.levels, [0]);
    assert_eq!((ground.a, ground.b), (target.a, target.b));
    let aligned = walls.iter().find(|w| w.id != "reference" && w.levels == [1]).unwrap();
    assert_eq!((aligned.a[1], aligned.b[1]), (1000.0, 1000.0));
    assert!(((aligned.b[0] - aligned.a[0]).hypot(aligned.b[1] - aligned.a[1]) - length).abs() < 1e-9);
    assert_eq!(aligned.doors, target.doors);
    assert_eq!(aligned.thickness, target.thickness);
    assert_eq!(s.doc.journal.len(), before.journal.len() + 1);
    trigon_model::build(&s.doc).unwrap();
    s.execute("edit.undo", json!({})).unwrap();
    assert_eq!(s.doc, before);
}

#[test]
fn wall_alignment_matches_diagonal_faces_including_reversed_source_directions() {
    for theta in [0.0_f64, 0.61, 1.27] {
        for reversed in [false, true] {
            let mut s = session();
            let t = [theta.cos(), theta.sin()];
            let n = [-t[1], t[0]];
            let origin = [-3000., 0.];
            let at = |along: f64, across: f64| [origin[0] + t[0] * along + n[0] * across, origin[1] + t[1] * along + n[1] * across];
            let (mut a, mut b) = (at(100., 40.), at(1800., 70.));
            if reversed {
                std::mem::swap(&mut a, &mut b);
            }
            s.execute(
                "interior.add_walls",
                json!({"walls":[
                    {"id":"reference","levels":[0],"a":origin,"b":at(2000.,0.),"thickness":200},
                    {"id":"target","levels":[0],"a":a,"b":b,"thickness":100}
                ]}),
            )
            .unwrap();
            let before = s.doc.clone();
            s.execute("edit.align_walls", json!({"targets":["target.L0"],"reference":"reference.L0","reference_face":"left","target_face":"right"}))
                .unwrap();
            let w = s.doc.buildings[0].interior_walls.iter().find(|w| w.id == "target").unwrap();
            let direction = if reversed { -1.0 } else { 1.0 };
            for p in [w.a, w.b] {
                let face = [p[0] - direction * n[0] * 50., p[1] - direction * n[1] * 50.];
                let offset = (face[0] - origin[0]) * n[0] + (face[1] - origin[1]) * n[1];
                assert!((offset - 100.).abs() < 1e-8, "diagonal/reversed faces must be on the same exact line");
            }
            let original_length = (b[0] - a[0]).hypot(b[1] - a[1]);
            assert!(((w.b[0] - w.a[0]).hypot(w.b[1] - w.a[1]) - original_length).abs() < 1e-8);
            s.execute("edit.undo", json!({})).unwrap();
            assert_eq!(s.doc, before);
        }
    }
}

#[test]
fn wall_alignment_rejects_mixed_targets_self_reference_and_nonparallel_walls_atomically() {
    let mut s = session();
    s.execute(
        "interior.add_walls",
        json!({"walls":[
            {"id":"reference","levels":[0,1],"a":[-4000,1000],"b":[-2000,1000],"thickness":200},
            {"id":"target","levels":[0,1],"a":[-4000,50],"b":[-2000,75],"thickness":120},
            {"id":"perpendicular","levels":[0],"a":[-3000,0],"b":[-3000,2000],"thickness":120}
        ]}),
    )
    .unwrap();
    let before = s.doc.clone();
    let facade = before.buildings[0].facades[0].id.clone();
    s.execute("facade.set_bays", json!({"facade":facade,"count":2})).unwrap();
    s.execute("facade.place", json!({"facade":facade,"type":"window.casement.2","levels":[0],"bays":[0]})).unwrap();
    let before = s.doc.clone();
    let history = s.undo.len();
    for params in [
        json!({"targets":["target.L0","perpendicular.L0"],"reference":"reference.L0"}),
        json!({"targets":["target.L0",format!("{facade}.L0.B0")],"reference":"reference.L0"}),
        json!({"targets":["reference.L1"],"reference":"wall:reference"}),
        json!({"targets":["wall:target"],"reference":"reference.L1","target_face":"invalid"}),
        json!({"targets":["wall:target"],"reference":format!("{facade}.L0.B0")}),
        json!({"targets":[],"reference":"reference.L0"}),
        json!({"targets":["target.L0"],"reference":"reference.L0","angle":f64::MAX}),
    ] {
        assert!(s.execute("edit.align_walls", params).is_err());
        assert_eq!(s.doc, before);
        assert_eq!(s.undo.len(), history);
    }
}

#[test]
fn wall_cleanup_preserves_hosted_door_centres_and_definitions_with_atomic_exact_undo() {
    let mut s = session();
    s.execute(
        "interior.add_walls",
        json!({"walls":[
            {"id":"door-a","levels":[0],"a":[-4000,0],"b":[-2000,0],"thickness":100,"doors":[{"at":800,"width":700}]},
            {"id":"door-b","levels":[0],"a":[-1950,0],"b":[0,0],"thickness":100,"doors":[{"at":800,"width":700}]},
            {"id":"cross","levels":[0],"a":[-4080,-1000],"b":[-4080,1000],"thickness":100}
        ]}),
    )
    .unwrap();
    let before = s.doc.clone();
    s.execute("interior.regularize", json!({"axes_deg":[0.]})).unwrap();
    let after = s.doc.clone();
    assert_eq!(after.buildings[0].interior_walls.iter().map(|w| w.doors.len()).sum::<usize>(), 2);
    for old in before.buildings[0].interior_walls.iter().filter(|w| !w.doors.is_empty()) {
        let new = after.buildings[0].interior_walls.iter().find(|w| w.id == old.id).unwrap();
        assert_eq!(old.doors.len(), new.doors.len());
        for (a, b) in old.doors.iter().zip(&new.doors) {
            assert!((old.a[0] + a.at - new.a[0] - b.at).abs() < 1e-7);
            assert_eq!((a.width, a.height), (b.width, b.height));
        }
    }
    s.execute("interior.regularize", json!({"axes_deg":[0.]})).unwrap();
    assert_eq!(s.doc.buildings, after.buildings);
    s.execute("edit.undo", json!({})).unwrap();
    assert_eq!(s.doc, after);
    s.execute("edit.undo", json!({})).unwrap();
    assert_eq!(s.doc, before);
    assert!(s.execute("interior.regularize", json!({"axes_deg":[0.],"tolerance_mm":-1})).is_err());
    assert_eq!(s.doc, before);
}

#[test]
fn editing_copies_arrays_aligns_to_a_line_and_copies_over_storeys() {
    let mut s = session();
    editing_wall(&mut s, &[0]);
    let out = s.execute("edit.copy", json!({"targets":["wall:shared"],"delta":[0,1000,0],"count":2})).unwrap();
    assert_eq!(out["targets"].as_array().unwrap().len(), 2);
    assert_eq!(s.doc.buildings[0].interior_walls.len(), 3);
    s.execute("edit.align", json!({"targets":[out["targets"][0]],"line":[[0,2500],[10000,2500]],"anchor":"center"})).unwrap();
    assert!(s.doc.buildings[0].interior_walls.iter().any(|w| w.a[1] == 2500.0 && w.b[1] == 2500.0));
    let copied = s.execute("edit.copy_to_levels", json!({"targets":["wall:shared"],"levels":[0,1,1,2]})).unwrap();
    assert_eq!(copied["targets"].as_array().unwrap().len(), 2);
    for level in 0..=2 {
        assert!(s.doc.buildings[0].interior_walls.iter().any(|w| w.levels == [level] && w.a[1] == 0.0));
    }
    let before = s.doc.clone();
    assert!(s.execute("edit.copy_to_levels", json!({"targets":["wall:shared"],"levels":[1]})).is_err());
    assert_eq!(s.doc, before);
    s.execute("edit.copy_to_levels", json!({"targets":["wall:shared"],"levels":[1],"replace":true})).unwrap();
    assert_eq!(s.doc.buildings[0].interior_walls.iter().filter(|w| w.levels == [1] && w.a[1] == 0.0).count(), 1);
}

#[test]
fn editing_facade_instances_keeps_the_grid_and_other_storeys_fixed() {
    let mut s = session();
    let facade = first_facade_facing(&mut s, "S");
    s.execute("facade.set_bays", json!({"facade":facade,"count":3})).unwrap();
    s.execute("facade.place", json!({"facade":facade,"type":"window.casement.2","levels":[0,1],"bays":[0]})).unwrap();
    let b = &s.doc.buildings[0];
    let f = b.facade(&facade).unwrap();
    let grid = f.bays.clone();
    let edge = b.edge(f.edge).unwrap();
    let frame = trigon_model::build::FacadeFrame::new(edge.0, edge.1).unwrap();
    let target = format!("{facade}.L1.B0");
    s.execute("edit.move", json!({"targets":[target],"delta":[frame.t[0]*125.5,frame.t[1]*125.5,50]})).unwrap();
    let f = s.doc.buildings[0].facade(&facade).unwrap();
    assert_eq!(f.bays, grid);
    assert_eq!(f.placements.iter().find(|p| p.level == 0).unwrap().u, None);
    assert_eq!(f.placements.iter().find(|p| p.level == 1).unwrap().u, Some(grid[0] + 125.5));
    s.execute("edit.align", json!({"targets":[target],"axis":"u","coordinate":8000,"anchor":"center"})).unwrap();
    assert_eq!(s.doc.buildings[0].facade(&facade).unwrap().placements.iter().find(|p| p.level == 1).unwrap().u, Some(8000.0));
    let out =
        s.execute("edit.copy", json!({"targets":[format!("{facade}.L0.B0")],"delta":[frame.t[0]*3000.0,frame.t[1]*3000.0,0],"count":2})).unwrap();
    let ids = out["targets"].as_array().unwrap();
    assert_eq!(ids.len(), 2);
    assert_ne!(ids[0], ids[1]);
    let round_trip: trigon_model::Document = serde_json::from_str(&serde_json::to_string(&s.doc).unwrap()).unwrap();
    assert_eq!(s.doc, round_trip);
    trigon_model::build(&round_trip).unwrap();
}

#[test]
fn editing_errors_are_atomic_and_level_deletion_preserves_other_storeys() {
    let mut s = session();
    editing_wall(&mut s, &[0, 1, 2]);
    let before = s.doc.clone();
    let cases = [
        ("edit.move", json!({"targets":["shared.L0","missing"],"delta":[0,100,0]})),
        ("edit.move", json!({"targets":["shared.L0"],"delta":[1e300,0,0]})),
        ("edit.copy", json!({"targets":["wall:shared"],"delta":[0,100,0],"count":usize::MAX})),
        ("edit.copy", json!({"targets":[],"delta":[0,100,0]})),
        ("edit.align", json!({"targets":["wall:shared"],"line":[[0,0],[0,0]]})),
        ("edit.copy_to_levels", json!({"targets":["wall:shared"],"levels":[usize::MAX]})),
        ("edit.delete", json!({"targets":["wall:shared","shared.L1"]})),
    ];
    for (command, params) in cases {
        let err = s.execute(command, params).unwrap_err();
        assert!(!matches!(err, EngineError::Internal(_)), "{err}");
        assert_eq!(s.doc, before, "{command}");
    }
    s.execute("edit.delete", json!({"targets":["shared.L1"]})).unwrap();
    assert_eq!(s.doc.buildings[0].interior_walls[0].levels, vec![0, 2]);
    s.execute("edit.undo", json!({})).unwrap();
    assert_eq!(s.doc, before);
}

#[test]
fn old_placements_default_to_the_bay_centre() {
    let old = json!({"level":0,"bay":0,"type":"window.casement.2"});
    let placement: trigon_model::Placement = serde_json::from_value(old).unwrap();
    assert_eq!(placement.u, None);
    assert!(serde_json::to_value(&placement).unwrap().get("u").is_none());
}

#[test]
fn copying_over_a_facade_preserves_the_previous_phase() {
    let mut s = session();
    let facade = first_facade_facing(&mut s, "S");
    s.execute("facade.set_bays", json!({"facade":facade,"count":3})).unwrap();
    s.execute("facade.place", json!({"facade":facade,"type":"window.casement.2","levels":[0,1],"bays":[0]})).unwrap();
    s.execute("phase.set", json!({"phases":[{"id":"old","name":"Existing"},{"id":"new","name":"Proposal"}]})).unwrap();
    s.execute("edit.copy_to_levels", json!({"targets":[format!("{facade}.L0.B0")],"levels":[1],"replace":true})).unwrap();
    let b = &s.doc.buildings[0];
    let f = b.facade(&facade).unwrap();
    let previous = trigon_model::build::View { phase: 0, filter: trigon_model::build::PhaseFilter::Complete };
    let built = trigon_model::build::build_view(&s.doc, previous).unwrap();
    assert_eq!(built[0].walls.iter().find(|w| w.id == facade).unwrap().openings.len(), 2);
    assert_eq!(f.placements.iter().filter(|p| p.level == 1).count(), 2);
}

#[test]
fn editing_stairs_and_rooms_and_aligning_to_reference_elements() {
    let mut s = session();
    s.execute(
        "interior.add_stairs",
        json!({"stairs":[{"id":"flight","levels":[0],"outline":[[-8000,-3000],[-5000,-3000],[-5000,-1000],[-8000,-1000]]}]}),
    )
    .unwrap();
    s.execute(
        "interior.add_rooms",
        json!({"rooms":[{"id":"space","name":"Room","levels":[0,1],"polygon":[[2000,0],[4000,0],[4000,2000],[2000,2000]]}]}),
    )
    .unwrap();
    let out = s.execute("edit.copy", json!({"targets":["stair:flight"],"delta":[0,3500,0]})).unwrap();
    assert_eq!(s.doc.buildings[0].stairs.len(), 2);
    s.execute("edit.delete", json!({"targets":out["targets"]})).unwrap();
    assert_eq!(s.doc.buildings[0].stairs.len(), 1);
    s.execute("edit.move", json!({"targets":["space.L1"],"delta":[500,500,0]})).unwrap();
    assert!(s.doc.buildings[0].rooms.iter().any(|r| r.id == "space" && r.levels == [0] && r.polygon[0] == [2000.0, 0.0]));
    editing_wall(&mut s, &[0]);
    s.execute("edit.align", json!({"targets":["wall:shared"],"axis":"y","reference":"room:space","anchor":"end"})).unwrap();
    assert_eq!(s.doc.buildings[0].interior_walls[0].a[1], 1940.0, "the 120 mm wall's upper face meets the room's upper edge");
}

#[test]
fn saved_groups_edit_as_one_batch_and_follow_split_instances() {
    let mut s = session();
    editing_wall(&mut s, &[0, 1]);
    s.execute("group.define", json!({"id":"core","name":"Core walls","targets":["wall:shared"]})).unwrap();
    let before = s.doc.clone();
    s.execute("edit.move", json!({"targets":["shared.L1"],"delta":[0,100,0]})).unwrap();
    let group = s.execute("group.list", json!({})).unwrap();
    assert_eq!(group["groups"][0]["targets"].as_array().unwrap().len(), 2);
    assert_eq!(group["groups"][0]["valid"], true);
    s.execute("edit.move", json!({"targets":["group:core"],"delta":[0,200,0]})).unwrap();
    assert!(s.doc.buildings[0].interior_walls.iter().all(|w| w.a[1] >= 200.0));
    s.execute("edit.undo", json!({})).unwrap();
    s.execute("edit.undo", json!({})).unwrap();
    assert_eq!(s.doc, before);
    s.execute("edit.delete", json!({"targets":["group:core"]})).unwrap();
    assert!(s.doc.buildings[0].interior_walls.is_empty());
    assert_eq!(s.execute("group.list", json!({})).unwrap()["groups"], json!([]));
    s.execute("edit.undo", json!({})).unwrap();
    assert_eq!(s.doc, before);
}

#[test]
fn project_templates_are_detached_copies_and_never_change_the_source_file() {
    let mut s = session();
    editing_wall(&mut s, &[0]);
    s.execute("group.define", json!({"id":"seed","name":"Template group","targets":["wall:shared"]})).unwrap();
    let template = std::env::temp_dir().join(format!("trigon-template-{}.trigon", std::process::id()));
    let original_path = std::path::PathBuf::from("original.trigon");
    s.path = Some(original_path.clone());
    let original = s.doc.clone();
    let dirty = s.dirty;
    s.execute("document.save_template", json!({"path":template})).unwrap();
    assert_eq!(s.path, Some(original_path));
    assert_eq!(s.dirty, dirty);
    assert_eq!(s.doc, original);
    let source_bytes = std::fs::read(&template).unwrap();
    s.execute("document.from_template", json!({"path":template})).unwrap();
    assert!(s.path.is_none());
    assert!(s.dirty);
    assert_eq!(s.doc, original);
    s.execute("building.set_storeys", json!({"count":2})).unwrap();
    assert_eq!(std::fs::read(&template).unwrap(), source_bytes);
    let _ = std::fs::remove_file(template);
}

#[test]
fn copied_over_group_members_follow_the_replacement_definition() {
    let mut s = session();
    editing_wall(&mut s, &[0]);
    s.execute("interior.add_walls", json!({"walls":[{"id":"destination","levels":[1],"a":[-4000,0],"b":[-2000,0],"thickness":120}]})).unwrap();
    s.execute("group.define", json!({"id":"upper","name":"Upper walls","targets":["destination.L1"]})).unwrap();
    s.execute("edit.copy_to_levels", json!({"targets":["wall:shared"],"levels":[1],"replace":true})).unwrap();
    assert_eq!(s.execute("group.list", json!({})).unwrap()["groups"][0]["valid"], true);
    s.execute("edit.move", json!({"targets":["group:upper"],"delta":[0,100,0]})).unwrap();
    assert!(s.doc.buildings[0].interior_walls.iter().any(|w| w.levels == [1] && w.a[1] == 100.0));
}

#[test]
fn invalid_or_recursive_groups_are_refused_atomically() {
    let mut s = session();
    editing_wall(&mut s, &[0]);
    let before = s.doc.clone();
    assert!(s.execute("group.define", json!({"id":"bad","name":"Bad","targets":["missing"]})).is_err());
    assert_eq!(s.doc, before);
    s.execute("group.define", json!({"id":"one","name":"One","targets":["wall:shared"]})).unwrap();
    s.execute("group.define", json!({"id":"two","name":"Two","targets":["group:one"]})).unwrap();
    assert!(!s.doc.groups[1].targets.iter().any(|t| t.starts_with("group:")));
    s.doc.groups[0].targets = vec!["group:one".into()];
    let before = s.doc.clone();
    assert!(s.execute("edit.move", json!({"targets":["group:one"],"delta":[0,100,0]})).is_err());
    assert_eq!(s.doc, before);
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
    assert!(result["lines"].as_u64().unwrap() > 10, "{result}");
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

/// A DXF's group-code pairs, checked to be well formed.
fn dxf_pairs(path: &std::path::Path) -> Vec<(i32, String)> {
    // Windows-1252: one byte per character, æ is 0xE6.
    let text: String = std::fs::read(path).unwrap().iter().map(|b| char::from(*b)).collect();
    let lines: Vec<&str> = text.lines().collect();
    assert_eq!(lines.len() % 2, 0);
    let pairs: Vec<(i32, String)> = lines.chunks(2).map(|p| (p[0].trim().parse::<i32>().unwrap(), p[1].to_string())).collect();
    let sections: Vec<&str> = pairs.windows(2).filter(|w| w[0] == (0, "SECTION".into())).map(|w| w[1].1.as_str()).collect();
    assert_eq!(sections, ["HEADER", "CLASSES", "TABLES", "BLOCKS", "ENTITIES", "OBJECTS"]);
    assert_eq!(pairs.last().unwrap(), &(0, "EOF".to_string()));
    pairs
}

fn dxf_count(pairs: &[(i32, String)], kind: &str) -> usize {
    let start = pairs.iter().position(|p| *p == (2, "ENTITIES".to_string())).unwrap();
    pairs[start..].iter().take_while(|p| **p != (0, "ENDSEC".to_string())).filter(|p| p.0 == 0 && p.1 == kind).count()
}

#[test]
fn section_level_names_from_multiple_wings_stay_inside_the_sheet() {
    let mut s = Session::new(Box::new(Canned));
    for (id, name, y) in [("north", "North wing beside the crossing", 0.), ("south", "South wing beside the courtyard", 20000.)] {
        s.execute(
            "building.add",
            json!({"id":id,"name":name,"footprint":[[0,y],[24000,y],[24000,y+12000.],[0,y+12000.]],"storeys":3,"storey_height_mm":3400}),
        )
        .unwrap();
        s.execute("building.set_roof", json!({"building":id,"kind":"pitched","pitch_deg":35})).unwrap();
        s.execute("building.set_attic", json!({"building":id,"floor":9450})).unwrap();
    }
    let section =
        crate::section::scene(&s.doc, [[6000., -1000.], [6000., 33000.]], false, 1000., trigon_model::build::View::latest(&s.doc), "Both wings", "")
            .unwrap();
    let texts: Vec<_> = section.scene.items.iter().filter_map(|i| if let crate::sheet::Item::Text(t) = i { Some(t.clone()) } else { None }).collect();
    let ink = crate::plan_output::text_ink_bounds(&texts).unwrap();
    assert!(texts.iter().any(|t| t.text.contains("North wing") && t.text.contains("South wing")));
    for (text, bounds) in texts.iter().zip(ink) {
        assert_eq!(text.size, 2.6, "datum lettering stays at its physical reading size");
        let bounds = bounds.unwrap();
        let lo = section.vp.px([text.at[0] + bounds[0] * section.scene.scale, text.at[1] + bounds[1] * section.scene.scale]);
        let hi = section.vp.px([text.at[0] + bounds[2] * section.scene.scale, text.at[1] + bounds[3] * section.scene.scale]);
        assert!(lo[0] >= 16. && hi[0] <= 1584. && hi[1] >= 120. && lo[1] <= 820., "{} at {lo:?}..{hi:?}", text.text);
    }
}

#[test]
fn sections_cut_the_model_and_mark_the_levels() {
    let mut s = session();
    let facade = first_facade_facing(&mut s, "S");
    s.execute("facade.set_bays", json!({"facade": facade, "count": 2})).unwrap();
    s.execute("facade.place", json!({"facade": facade, "type": "balcony.cantilever", "levels": [1], "bays": [0]})).unwrap();
    s.execute("facade.place", json!({"facade": facade, "type": "window.casement.2", "levels": [2, 3], "bays": [0, 1]})).unwrap();
    let before = s.doc.clone();
    let dir = std::env::temp_dir().join(format!("trigon-section-{}", std::process::id()));
    // Across the 24 × 12 m block from south to north, through the balcony bay, looking west.
    let x = s.doc.buildings[0].footprint.iter().map(|p| p[0]).fold(f64::INFINITY, f64::min) + 6000.0;
    let line = |path: &str| json!({"from": [x, -12000.0], "to": [x, 12000.0], "path": dir.join(path), "title": "Snit A–A", "caption": "Gård"});
    let svg = s.execute("section.sheet", line("s.svg")).unwrap();
    assert_eq!(svg["looking"], "left of from → to");
    // Two cut walls, five floor slabs and the roof slab, the balcony slab.
    assert!(svg["cut_regions"].as_u64().unwrap() >= 8, "{svg}");
    assert!(svg["levels"].as_u64().unwrap() >= 6, "{svg}");
    let text = std::fs::read_to_string(dir.join("s.svg")).unwrap();
    assert!(text.contains("url(#masonry)") && text.contains("+0,00") && text.contains("Snit A–A") && text.contains("Stueetage"));
    assert!(!text.contains("NaN") && !text.contains("inf"));
    s.execute("section.sheet", line("s.pdf")).unwrap();
    assert_eq!(lopdf::Document::load(dir.join("s.pdf")).unwrap().get_pages().len(), 1);
    let png = s.execute("section.sheet", line("s.png")).unwrap();
    assert_eq!(png["image_path"], json!(dir.join("s.png")));
    let dxf = s.execute("section.sheet", line("s.dxf")).unwrap();
    let pairs = dxf_pairs(&dir.join("s.dxf"));
    assert!(dxf_count(&pairs, "HATCH") >= 8, "poche: {dxf}");
    assert!(dxf_count(&pairs, "TEXT") >= 12, "levels and names: {dxf}");
    for layer in ["A-WALL-CUT", "A-WALL-PATT", "A-LEVL", "A-GRND", "A-ANNO-TTLB"] {
        assert!(dxf["dxf"]["layers"].as_array().unwrap().iter().any(|l| l == layer), "{layer} used: {dxf}");
    }
    // The other way: the same cut, mirrored, with the north end on the left.
    let mut flipped = line("f.svg");
    flipped["flip"] = json!(true);
    flipped["depth_mm"] = json!(500.0);
    let f = s.execute("section.sheet", flipped).unwrap();
    assert_eq!(f["looking"], "right of from → to");
    assert_eq!(f["cut_regions"], svg["cut_regions"]);
    assert_eq!(s.doc, before);
    // Walls cut storey by storey: thicker walls lower down cut a larger area.
    s.execute("building.set_wall_schedule", json!({"thickness": [600.0, 600.0, 480.0, 480.0, 360.0]})).unwrap();
    let stepped = s.execute("section.sheet", line("t.svg")).unwrap();
    assert!(stepped["cut_area_m2"].as_f64().unwrap() > svg["cut_area_m2"].as_f64().unwrap(), "{stepped} vs {svg}");
    s.execute("edit.undo", json!({})).unwrap();
    for (k, request) in [
        json!({"from": [0., 0.], "to": [0., 0.], "path": dir.join("bad.svg")}),
        json!({"from": [0., 0.], "to": [500., 0.], "path": dir.join("bad.svg")}),
        json!({"from": [0., 0.], "to": [f64::MAX, 0.], "path": dir.join("bad.svg")}),
        json!({"from": [-1e300, 0.], "to": [0., 0.], "path": dir.join("bad.svg")}),
        json!({"from": [0., -9000.], "to": [0., 9000.], "depth_mm": 0.0, "path": dir.join("bad.svg")}),
        json!({"from": [0., -9000.], "to": [0., 9000.], "depth_mm": -5.0, "path": dir.join("bad.svg")}),
        json!({"from": [0., -9000.], "to": [0., 9000.], "depth_mm": 1e300, "path": dir.join("bad.svg")}),
        json!({"from": [0., -9000.], "to": [0., 9000.], "path": dir.join("bad.txt")}),
        json!({"from": [0., -9000.], "to": [0., 9000.], "path": dir.join("bad.svg"), "title": "x".repeat(300)}),
        json!({"from": [0., -9000.], "to": [0., 9000.], "path": dir.join("bad.svg"), "phase": "nope"}),
        json!({"from": [0., -9000.], "path": dir.join("bad.svg")}),
        json!({"from": [0., -9000.], "to": [0., 9000.], "path": dir.join("bad.svg"), "extra": 1}),
    ]
    .into_iter()
    .enumerate()
    {
        assert!(matches!(s.execute("section.sheet", request), Err(e) if !matches!(e, EngineError::Internal(_))), "request {k}");
    }
    assert!(!dir.join("bad.svg").exists());
    // A line that misses the building cuts nothing and still draws a sheet.
    let empty = s.execute("section.sheet", json!({"from": [-90000., 50000.], "to": [90000., 50000.], "path": dir.join("e.svg")})).unwrap();
    assert_eq!(empty["cut_regions"], 0);
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn plans_and_elevations_write_dxf() {
    let mut s = session();
    let facade = first_facade_facing(&mut s, "S");
    s.execute("facade.set_bays", json!({"facade": facade, "count": 2})).unwrap();
    s.execute("facade.place", json!({"facade": facade, "type": "balcony.cantilever", "levels": [1], "bays": [0]})).unwrap();
    s.execute("facade.place", json!({"facade": facade, "type": "window.casement.2", "levels": [1], "bays": [1]})).unwrap();
    let dir = std::env::temp_dir().join(format!("trigon-dxf-{}", std::process::id()));
    let plan = s.execute("plan.sheet", json!({"levels": [1], "path": dir.join("p.dxf"), "caption": "Kælder & gård"})).unwrap();
    let pairs = dxf_pairs(&dir.join("p.dxf"));
    assert!(dxf_count(&pairs, "HATCH") >= 1 && dxf_count(&pairs, "LWPOLYLINE") >= 1, "{plan}");
    let value = |pairs: &[(i32, String)], var: &str| pairs.iter().position(|(c, v)| *c == 9 && v == var).map(|i| pairs[i + 1].1.clone());
    assert_eq!(value(&pairs, "$INSUNITS").as_deref(), Some("4"));
    assert!(pairs.iter().any(|(c, v)| *c == 1 && v.contains("Kælder & gård")), "caption in the title block, in Windows-1252");
    // Unturned, the plan is in site coordinates: the footprint's corners are polyline vertices.
    let corner = s.doc.buildings[0].footprint[0];
    let xs: Vec<f64> = pairs.iter().filter(|(c, _)| *c == 10).filter_map(|(_, v)| v.parse().ok()).collect();
    assert!(xs.iter().any(|x| (x - corner[0]).abs() < 0.01), "site coordinates");
    assert!(s.execute("plan.sheet", json!({"levels": [1, 2], "path": dir.join("two.dxf")})).is_err(), "one level per DXF");
    let elevation = s.execute("elevation.sheet", json!({"facade": facade, "path": dir.join("e.dxf"), "title": "Gårdfacade"})).unwrap();
    let pairs = dxf_pairs(&dir.join("e.dxf"));
    assert!(dxf_count(&pairs, "LINE") > 20, "{elevation}");
    assert!(pairs.iter().any(|(c, v)| *c == 1 && v == "Ca. 1200 × 3000"), "the balcony's size as TEXT");
    assert!(pairs.iter().any(|(c, v)| *c == 8 && v == "A-GRND"));
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
    // Partitions and load-bearing walls are typed by their build-up; the room is a space with its name.
    assert!(ifc.contains(".PARTITIONING.") && ifc.contains("IFCWALLTYPE(") && ifc.contains("B\\X2\\00E6\\X0\\rende inderv\\X2\\00E6\\X0\\g"));
    assert!(ifc.contains("IFCSPACE(") && ifc.contains("'Stue'") && ifc.contains("IFCSTAIRFLIGHT("));
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
    let walls = text.matches("IFCWALL(").count() + text.matches("IFCWALLSTANDARDCASE(").count();
    assert_eq!(walls, built[0].walls.len() * storeys + built[0].interior.len());

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
fn a_bim_export_reads_back_the_same_in_both_readers() {
    // Stepped masonry, a pitched roof with a gable and gutters, a steel balcony with tie rods, a
    // cantilever balcony, a staircase, interior walls with a door and rooms, in two phases; then
    // a basement with a cellar stair, a rendered facade with a band, and a plinth.
    let mut s = session();
    let (south, north, east) = (first_facade_facing(&mut s, "S"), first_facade_facing(&mut s, "N"), first_facade_facing(&mut s, "E"));
    s.execute("phase.set", json!({"phases": [{"id": "1940", "name": "Opført"}, {"id": "forslag", "name": "Forslag"}]})).unwrap();
    s.execute("building.set_wall_schedule", json!({"preset": "copenhagen_masonry"})).unwrap();
    s.execute("building.set_roof", json!({"kind": "pitched", "pitch_deg": 40.0, "gables": [east], "gutters": true})).unwrap();
    s.execute(
        "types.define",
        json!({"id": "balcony.steel", "name": "Stålaltan", "width": 2400.0, "height": 1000.0, "sill": 0.0,
               "kind": {"family": "steel_balcony", "depth": 1300.0, "railing": "frederiksberg", "finish": "anthracite", "deck": "cumaru",
                        "handrail": true, "screens": "both", "tie_rods": true}}),
    )
    .unwrap();
    for f in [&south, &north] {
        s.execute("facade.set_bays", json!({"facade": f, "count": 6})).unwrap();
        s.execute("facade.place", json!({"facade": f, "type": "window.casement.2", "phase": "1940"})).unwrap();
    }
    s.execute("facade.place", json!({"facade": south, "type": "door.french", "levels": [1, 2], "bays": [2]})).unwrap();
    s.execute("facade.place", json!({"facade": south, "type": "balcony.steel", "levels": [1, 2], "bays": [2]})).unwrap();
    s.execute("facade.place", json!({"facade": north, "type": "balcony.cantilever", "levels": [1], "bays": [3], "phase": "1940"})).unwrap();
    s.execute(
        "interior.add_walls",
        json!({"walls": [{"id": "spine", "levels": [0, 1, 2, 3, 4], "a": [-11000.0, 0.0], "b": [11000.0, 0.0], "thickness": 240.0, "load_bearing": true},
                         {"id": "p", "levels": [1, 2], "a": [3000.0, 200.0], "b": [3000.0, 5000.0], "thickness": 100.0,
                          "doors": [{"at": 2400.0, "width": 900.0}]}]}),
    )
    .unwrap();
    s.execute(
        "interior.add_stairs",
        json!({"stairs": [{"id": "core", "levels": [0, 1], "outline": [[-6000.0, -4800.0], [-3200.0, -4800.0], [-3200.0, -2600.0], [-6000.0, -2600.0]],
                           "kind": "u_turn", "rounded": false}]}),
    )
    .unwrap();
    s.execute(
        "interior.add_rooms",
        json!({"rooms": [{"levels": [1, 2], "name": "Stue", "polygon": [[-2000.0, -5000.0], [2800.0, -5000.0], [2800.0, -200.0], [-2000.0, -200.0]]}]}),
    )
    .unwrap();
    s.execute("building.add_basement", json!({"floor": -2480})).unwrap();
    s.execute("facade.set_finish", json!({"facades": [north], "finish": "render"})).unwrap();
    s.execute("facade.set_bands", json!({"facades": [north], "bands": [{"levels": [2], "bottom": -450, "top": 500}]})).unwrap();
    s.execute("building.set_plinth", json!({"height_mm": 600})).unwrap();
    s.execute(
        "building.set_cellar_stairs",
        json!({"stairs": [{"facade": east, "from": 1000, "to": 6000, "width": 1500, "top": 1200, "bottom": 4000, "floor": -2480, "door_at": 5000}]}),
    )
    .unwrap();
    let dir = std::env::temp_dir().join(format!("trigon-bim-{}", std::process::id()));
    let ifc = dir.join("m.ifc");
    s.execute("export.ifc", json!({"path": ifc})).unwrap();
    let text = std::fs::read_to_string(&ifc).unwrap();
    for class in [
        "IFCWALLSTANDARDCASE(",
        "IFCMATERIALLAYERSETUSAGE(",
        "IFCRELCONNECTSPATHELEMENTS(",
        "IFCROOF(",
        "IFCSTAIRFLIGHT(",
        "IFCSPACE(",
        "IFCELEMENTASSEMBLY(",
        "IFCBEAM(",
        "IFCMEMBER(",
        "IFCPRESENTATIONLAYERASSIGNMENT(",
        "IFCSTYLEDITEM(",
        "Qto_WallBaseQuantities",
        "IFCPIPESEGMENT(",
        ".MOLDING.)",
        ".CLADDING.)",
        "pudset mur",
        "'K\\X2\\00E6\\X0\\ldernedgang'",
    ] {
        assert!(text.contains(class), "{class}");
    }
    assert!(text.contains("IFCLABEL('NEW')") && text.contains("IFCLABEL('EXISTING')"), "phase status");
    let v = s.execute("ifc.check", json!({"path": ifc})).unwrap();
    for r in v["readers"].as_array().unwrap() {
        assert_eq!(r["failures"], 0, "{r}");
    }
    let cmp = &v["ifc_lite_vs_bimifc"];
    assert_eq!((cmp["only_in_first"].as_u64(), cmp["only_in_second"].as_u64()), (Some(0), Some(0)), "{cmp}");
    assert!(cmp["max_bounds_deviation_mm"].as_f64().unwrap() < 0.01, "{cmp}");
    for m in v["versus_this_model"].as_array().unwrap() {
        assert_eq!(m["matched"], m["model_elements"], "{m}");
        assert!(m["max_bounds_deviation_mm"].as_f64().unwrap() < 0.01, "{m}");
    }
    let _ = std::fs::remove_dir_all(dir);
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

#[test]
fn a_3d_plan_cut_closes_the_walls_at_the_cut_and_leaves_out_what_is_above() {
    let mut s = session();
    s.execute(
        "interior.add_walls",
        json!({"walls": [{"id": "w", "levels": [0, 1, 2], "a": [-3000, 0], "b": [3000, 0], "thickness": 120, "doors": [{"at": 3000, "width": 900}]}]}),
    )
    .unwrap();
    let b = &s.doc.buildings[0];
    let z = b.storeys[1].elevation + 1200.0;
    let view = trigon_model::build::View::latest(&s.doc);
    let cut = trigon_model::build::build_cut(&s.doc, view, Some(z)).unwrap();
    let top = |m: &trigon_model::Mesh| m.bounds().map_or(f64::MIN, |(_, hi)| hi[2]);
    assert!(cut[0].roof.is_none());
    assert!(cut[0].walls.iter().all(|w| w.segments.iter().all(|g| g.level <= 1 && g.z1 <= z + 1e-6) && top(&w.mesh) <= z + 1e-6));
    assert!(cut[0].slabs.iter().all(|sl| sl.level <= 1));
    let wall = cut[0].interior.iter().find(|w| w.id == "w.L1").unwrap();
    assert!((top(&wall.mesh) - z).abs() < 1e-6 && wall.doors.len() == 1);
    assert!(!cut[0].interior.iter().any(|w| w.id == "w.L2"));
    // The door stays open through the cut.
    assert!(wall.doors[0].rect.v1 > z);
    // Exported; refused with a storey that doesn't exist or a height without a storey.
    let path = std::env::temp_dir().join(format!("trigon-cut-{}.glb", std::process::id()));
    assert!(s.execute("export.gltf", json!({"path": path, "cut_level": 1})).is_ok());
    let _ = std::fs::remove_file(path);
    for bad in [
        json!({"path": "x.glb", "cut_level": 99}),
        json!({"path": "x.glb", "cut_mm": 1200}),
        json!({"path": "x.glb", "cut_level": 1, "cut_mm": f64::MAX}),
    ] {
        assert!(s.execute("export.gltf", bad.clone()).is_err(), "{bad}");
    }
}

#[test]
fn explicit_yellow_brick_finish_is_undoable_without_changing_geometry_or_other_facades() {
    let mut s = session();
    let south = first_facade_facing(&mut s, "S");
    let before = s.doc.clone();
    let geometry = trigon_model::build(&s.doc).unwrap();
    s.execute("facade.set_finish", json!({"facades":[south],"finish":"yellow_brick"})).unwrap();
    let changed = trigon_model::build(&s.doc).unwrap();
    for (a, b) in geometry[0].walls.iter().zip(&changed[0].walls) {
        assert_eq!(a.id, b.id);
        assert_eq!(a.mesh, b.mesh, "a finish cannot change wall geometry");
        if a.id == south {
            assert_eq!(b.finish, trigon_model::Material::BrickYellow);
        } else {
            assert_eq!(a.finish, b.finish, "the finish is scoped to the requested facade");
        }
    }
    assert_eq!(s.execute("edit.undo", json!({})).unwrap()["undone"], true);
    assert_eq!(s.doc, before);
    assert_eq!(s.execute("edit.redo", json!({})).unwrap()["redone"], true);
    assert_eq!(s.doc.buildings[0].facade(&south).unwrap().finish, Some(trigon_model::doc::WallFinish::YellowBrick));
    let encoded = serde_json::to_value(&before).unwrap();
    assert_eq!(serde_json::from_value::<trigon_model::Document>(encoded).unwrap(), before);
}

#[test]
fn street_fronts_loggias_bands_finishes_and_cellar_stairs() {
    let mut s = session();
    s.execute("building.add_basement", json!({"floor": -2480})).unwrap();
    let south = first_facade_facing(&mut s, "S");
    let len = {
        let b = &s.doc.buildings[0];
        let (a, e) = b.edge(b.facade(&south).unwrap().edge).unwrap();
        ((e[0] - a[0]).powi(2) + (e[1] - a[1]).powi(2)).sqrt()
    };
    s.execute(
        "types.define",
        json!({"id": "loggia.t", "name": "Loggia", "width": 2000, "height": 2050, "sill": 500,
               "kind": {"family": "loggia", "depth": 1400, "rail_height": 1000, "door_width": 900, "door_side": "left"}}),
    )
    .unwrap();
    s.execute(
        "types.define",
        json!({"id": "window.t", "name": "Window with top lights", "width": 2400, "height": 2050, "sill": 500, "kind": {"family": "window", "lights": 3, "top_light": 500}}),
    )
    .unwrap();
    s.execute("facade.set_bays", json!({"facade": south, "centres": [len / 3.0, 2.0 * len / 3.0]})).unwrap();
    s.execute("facade.place", json!({"facade": south, "type": "loggia.t", "levels": [2], "bays": [0]})).unwrap();
    s.execute("facade.place", json!({"facade": south, "type": "window.t", "levels": [2], "bays": [1]})).unwrap();
    s.execute("facade.set_finish", json!({"facades": [south], "finish": "render"})).unwrap();
    s.execute("facade.set_bands", json!({"facades": [south], "bands": [{"levels": [2], "bottom": -450, "top": 500}]})).unwrap();
    s.execute(
        "building.set_cellar_stairs",
        json!({"stairs": [{"facade": south, "from": 1000, "to": 6000, "width": 1500, "top": 1200, "bottom": 4000, "floor": -2480, "door_at": 5000}]}),
    )
    .unwrap();
    let built = trigon_model::build(&s.doc).unwrap();
    let w = built[0].walls.iter().find(|w| w.id == south).unwrap();
    assert_eq!(w.finish, trigon_model::Material::Render);
    assert!(!w.trim.is_empty());
    // The loggia: brick returns, a white soffit, a rail; the cellar stair: a door into the basement and its pit.
    let loggia = w.openings.iter().find(|o| o.fill.type_id == "loggia.t").unwrap();
    let mats: Vec<trigon_model::Material> = loggia.fill.parts.iter().map(|(m, _)| *m).collect();
    assert!(
        mats.contains(&trigon_model::Material::BrickRed)
            && mats.contains(&trigon_model::Material::Render)
            && mats.contains(&trigon_model::Material::Glass)
    );
    let (_, hi) = loggia
        .fill
        .parts
        .iter()
        .filter_map(|(_, m)| m.bounds())
        .fold(([0.0; 3], [f64::MIN; 3]), |acc, b| (acc.0, [acc.1[0].max(b.1[0]), acc.1[1].max(b.1[1]), acc.1[2].max(b.1[2])]));
    assert!(hi[2] > 0.0);
    let cellar_door = w.openings.iter().find(|o| o.fill.type_id == "door.cellar").unwrap();
    assert_eq!(cellar_door.level, 0);
    assert!(w.segment(0).unwrap().openings.contains(&(w.openings.len() - 1)));
    assert!(w.attachments.iter().any(|a| a.class == trigon_model::ElementClass::Stair));
    // Plans, glTF and IFC take them.
    assert!(s.execute("plan.image", json!({"level": 0, "path": std::env::temp_dir().join("trigon-cellar.svg")})).is_ok());
    assert!(s.execute("plan.image", json!({"level": 2, "path": std::env::temp_dir().join("trigon-loggia.svg")})).is_ok());
    let dir = std::env::temp_dir().join(format!("trigon-street-{}", std::process::id()));
    s.execute("export.gltf", json!({"path": dir.join("m.glb")})).unwrap();
    s.execute("export.ifc", json!({"path": dir.join("m.ifc")})).unwrap();
    let _ = std::fs::remove_dir_all(dir);
    // Refused, and nothing changes.
    let before = s.doc.clone();
    for (cmd, bad) in [
        ("facade.set_finish", json!({})),
        ("facade.set_finish", json!({"facades": ["b1.f999"], "finish": "render"})),
        ("facade.set_bands", json!({"facades": [south], "bands": [{"levels": [99], "bottom": 0, "top": 100}]})),
        ("facade.set_bands", json!({"facades": [south], "bands": [{"levels": [1], "bottom": 0, "top": f64::MAX}]})),
        (
            "building.set_cellar_stairs",
            json!({"stairs": [{"facade": south, "from": 0, "to": 900, "width": 1500, "top": 0, "bottom": 800, "floor": -2480, "door_at": 400}]}),
        ),
        (
            "building.set_cellar_stairs",
            json!({"stairs": [{"facade": south, "from": 1000, "to": 6000, "width": 1500, "top": 1200, "bottom": 4000, "floor": -2480, "door_at": 9000}]}),
        ),
        (
            "types.define",
            json!({"id": "x", "name": "x", "width": 1000, "height": 2000, "sill": 500, "kind": {"family": "loggia", "depth": 1400, "rail_height": 1000, "door_width": 900, "door_side": "left"}}),
        ),
        (
            "types.define",
            json!({"id": "y", "name": "y", "width": 1000, "height": 1000, "sill": 500, "kind": {"family": "window", "lights": 2, "top_light": 900}}),
        ),
    ] {
        assert!(s.execute(cmd, bad.clone()).is_err(), "{cmd} {bad}");
    }
    assert_eq!(s.doc, before);
}

#[test]
fn drawing_annotations_are_atomic_journaled_and_persisted() {
    let mut s = session();
    let start = s.doc.clone();
    s.execute("view.define", json!({"id":"ground", "name":"Ground floor", "kind":{"view":"plan", "level":0}})).unwrap();
    let note = json!({"id":"note", "view":"ground", "size_mm":2.5,
        "kind":{"annotation":"text", "at":[1000.0,2000.0], "text":"Brick & <stone>", "rotation_deg":0.0}});
    s.execute("annotation.define", note.clone()).unwrap();
    let annotated = s.doc.clone();
    assert!(s.dirty);
    assert_eq!(s.doc.journal.last().unwrap().command, "annotation.define");
    let listed = s.execute("annotation.list", json!({"view":"ground"})).unwrap();
    assert_eq!(listed["annotations"][0], note);
    s.execute("edit.undo", json!({})).unwrap();
    assert_eq!(s.execute("annotation.list", json!({})).unwrap()["annotations"], json!([]));
    s.execute("edit.redo", json!({})).unwrap();
    assert_eq!(s.doc, annotated);
    for bad in [
        json!({"id":"note", "view":"missing", "kind":{"annotation":"text", "at":[0,0], "text":"x"}}),
        json!({"id":"note", "view":"ground", "kind":{"annotation":"dimension", "from":[0,0], "to":[0,0], "offset_mm":100}}),
        json!({"id":"note", "view":"ground", "kind":{"annotation":"text", "at":[1e300,0], "text":"x"}}),
    ] {
        assert!(s.execute("annotation.define", bad).is_err());
        assert_eq!(s.doc, annotated);
    }
    assert!(s.execute("view.remove", json!({"id":"ground"})).is_err());
    assert_eq!(s.doc, annotated);
    let path = std::env::temp_dir().join(format!("trigon-annotation-roundtrip-{}.trigon", std::process::id()));
    s.execute("document.save", json!({"path":path})).unwrap();
    let loaded = crate::commands::read_document(&path).unwrap();
    assert_eq!(loaded, annotated);
    std::fs::remove_file(path).unwrap();
    s.execute("annotation.remove", json!({"id":"note"})).unwrap();
    s.execute("view.remove", json!({"id":"ground"})).unwrap();
    s.execute("edit.undo", json!({})).unwrap();
    s.execute("edit.undo", json!({})).unwrap();
    assert_eq!(s.doc, annotated);
    assert_eq!(s.doc.buildings, start.buildings);
}

#[test]
fn saved_drawing_views_isolate_notes_and_share_every_export_format() {
    use trigon_model::build::View;
    let mut s = session();
    let facade = first_facade_facing(&mut s, "S");
    let kinds =
        [json!({"view":"plan","level":0}), json!({"view":"facade","facade":facade}), json!({"view":"section","from":[-15000,0],"to":[15000,0]})];
    let root = std::env::temp_dir().join(format!("trigon-annotation-export-{}", std::process::id()));
    std::fs::create_dir_all(&root).unwrap();
    for (i, kind) in kinds.into_iter().enumerate() {
        let id = format!("view-{i}");
        s.execute("view.define", json!({"id":id,"name":format!("Drawing {i}"),"kind":kind})).unwrap();
        for (tag, kind) in [
            ("note", json!({"annotation":"text","at":[1000,2000],"text":format!("Only view {i} & <stone>"),"rotation_deg":15})),
            ("dim", json!({"annotation":"dimension","from":[1000,1000],"to":[4000,5000],"offset_mm":-500})),
            ("tag", json!({"annotation":"leader","tip":[1000,2000],"elbow":[2000,3000],"at":[4000,3000],"text":"Wall type A"})),
        ] {
            s.execute("annotation.define", json!({"id":format!("{id}-{tag}"),"view":id,"kind":kind})).unwrap();
        }
        if i > 0 {
            s.execute(
                "annotation.define",
                json!({"id":format!("{id}-level"),"view":id,
                "kind":{"annotation":"level","at":[2000,3200],"datum_mm":200,"label":"FFL"}}),
            )
            .unwrap();
        }
        let preview = crate::drawing_view(&s.doc, &id, View::latest(&s.doc)).unwrap();
        assert!(preview.svg.contains(&format!("Only view {i} &amp; &lt;stone&gt;")));
        assert!(preview.svg.contains(">5000</text>"));
        assert!(preview.svg.contains("Wall type A"));
        if i > 0 {
            assert!(preview.svg.contains("+3,00 FFL"));
        }
        for p in [[0.0, 0.0], [1234.5678, -9876.5432]] {
            let q = preview.transform.to_model(preview.transform.to_sheet(p));
            assert!((p[0] - q[0]).abs() < 1e-8 && (p[1] - q[1]).abs() < 1e-8);
        }
        for ext in ["svg", "png", "pdf", "dxf"] {
            let path = root.join(format!("{id}.{ext}"));
            let result = s.execute("view.export", json!({"id":id,"path":path})).unwrap();
            assert_eq!(result["view"], id);
            let bytes = std::fs::read(&path).unwrap();
            match ext {
                "svg" => assert_eq!(String::from_utf8(bytes).unwrap(), preview.svg),
                "png" => assert!(bytes.starts_with(b"\x89PNG\r\n\x1a\n")),
                "pdf" => {
                    let pdf = lopdf::Document::load(&path).unwrap();
                    assert_eq!(pdf.get_pages().len(), 1);
                    // These remain vector sheets, not a whole-page raster image.
                    assert!(!pdf.objects.values().any(|o| {
                        let dict = match o {
                            lopdf::Object::Dictionary(dict) => Some(dict),
                            lopdf::Object::Stream(stream) => Some(&stream.dict),
                            _ => None,
                        };
                        dict.and_then(|d| d.get(b"Subtype").ok()).is_some_and(|v| v.as_name().ok() == Some(b"Image"))
                    }));
                }
                _ => {
                    assert_eq!(ext, "dxf");
                    let text = String::from_utf8_lossy(&bytes);
                    assert!(text.contains("A-ANNO"));
                    assert!(text.contains("Wall type A"));
                    assert!(text.contains("5000"));
                }
            }
        }
    }
    let notes = s.doc.annotations.clone();
    // Projection depth does not change the section coordinate frame.
    s.execute(
        "view.define",
        json!({"id":"view-2","name":"Deeper section","kind":{"view":"section","from":[-15000,0],"to":[15000,0],"depth_mm":45000}}),
    )
    .unwrap();
    assert_eq!(s.doc.annotations, notes);
    let before_snap = s.doc.clone();
    let snapped=s.execute("view.snap",json!({"id":"view-0","point":[11001,5001],"tolerance_mm":10,"options":{"endpoints":false,"midpoints":false,"intersections":false,"grid_mm":100}})).unwrap();
    assert_eq!(snapped["point"], json!([11000.0, 5000.0]));
    assert_eq!(snapped["snap"]["kind"], "grid");
    assert_eq!(s.doc, before_snap);
    let plan = crate::drawing_view(&s.doc, "view-0", View::latest(&s.doc)).unwrap().svg;
    assert!(!plan.contains("Only view 1") && !plan.contains("Only view 2"));
    let before = s.doc.clone();
    assert!(s.execute("view.define", json!({"id":"view-0","name":"Reframe","kind":{"view":"plan","level":1}})).is_err());
    assert_eq!(s.doc, before);
    assert!(s.execute("annotation.define", json!({"id":"bad-level","view":"view-0","kind":{"annotation":"level","at":[0,1000]}})).is_err());
    assert_eq!(s.doc, before);
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn drawing_commands_refuse_hostile_values_without_changing_history() {
    let mut s = session();
    s.execute("view.define", json!({"id":"p","name":"Plan","kind":{"view":"plan","level":0}})).unwrap();
    let before = s.doc.clone();
    let bad = [
        ("view.snap", json!({"id":"p","point":[1e300,0],"tolerance_mm":10})),
        ("view.snap", json!({"id":"p","point":[0,0],"tolerance_mm":10,"options":{"grid_mm":0}})),
        ("view.define", json!({"id":"p","name":"x","kind":{"view":"plan","level":usize::MAX}})),
        ("view.define", json!({"id":"p","name":"x","kind":{"view":"section","from":[0,0],"to":[1e300,0]}})),
        ("view.define", json!({"id":"p","name":"x","kind":{"view":"section","from":[0,0],"to":[0,0]}})),
        ("view.define", json!({"id":"p","name":"x","kind":{"view":"section","from":[0,0],"to":[1000,0],"depth_mm":1e300}})),
        ("view.define", json!({"id":"p","name":"x","kind":{"view":"facade","facade":"missing"}})),
        ("annotation.define", json!({"id":"a","view":"p","size_mm":1e300,"kind":{"annotation":"text","at":[0,0],"text":"x"}})),
        ("annotation.define", json!({"id":"a","view":"p","kind":{"annotation":"dimension","from":[0,0],"to":[1000,0],"offset_mm":1e300}})),
        ("annotation.define", json!({"id":"a","view":"p","kind":{"annotation":"leader","tip":[0,0],"elbow":[0,0],"at":[1000,0],"text":"x"}})),
        ("annotation.define", json!({"id":"a","view":"p","kind":{"annotation":"text","at":[0,0],"text":"0\nSECTION\n"}})),
        ("annotation.define", json!({"id":"a","view":"p","kind":{"annotation":"text","at":[0,0],"text":"x","rotation_deg":1e300}})),
        ("view.export", json!({"id":"p","path":"/tmp/trigon-annotations-must-not-write.invalid"})),
    ];
    for (command, params) in bad {
        let result = s.execute(command, params);
        assert!(result.is_err() && !matches!(result, Err(EngineError::Internal(_))), "{command}: {result:?}");
        assert_eq!(s.doc, before);
    }
    // Rejected edits did not consume a step in the undo history.
    s.execute("edit.undo", json!({})).unwrap();
    assert!(s.doc.views.is_empty());
}

#[test]
fn detailed_roof_drainage_is_explicit_atomic_and_undoable() {
    let mut s = session();
    s.execute("building.set_roof", json!({"kind":"pitched","pitch_deg":35,"gutters":true})).unwrap();
    let before = s.doc.clone();
    let legacy = trigon_model::build(&before).unwrap();
    let facade = before.buildings[0].facades[0].id.clone();
    let drainage = json!({"gutter_width_mm":160,"wall_mm":2,"joint_spacing_mm":3000,
        "downpipes":[{"id":"rain-a","facade":facade,"along_mm":2000,"diameter_mm":80,"wall_offset_mm":100,"bottom_mm":150,"shoe_mm":200}]});
    s.execute("element.set", json!({"id":"b1.roof","properties":{"drainage":drainage}})).unwrap();
    let detailed = trigon_model::build(&s.doc).unwrap();
    assert_eq!(legacy[0].roof.as_ref().unwrap().cover, detailed[0].roof.as_ref().unwrap().cover);
    assert_eq!(legacy[0].roof.as_ref().unwrap().id, detailed[0].roof.as_ref().unwrap().id);
    let mesh = &detailed[0].roof.as_ref().unwrap().gutters;
    assert!(mesh.triangles.len() > legacy[0].roof.as_ref().unwrap().gutters.triangles.len());
    assert!(mesh.bounds().unwrap().0[2] < 400.0, "only the explicit pipe reaches ground");
    let text = serde_json::to_string(&s.doc).unwrap();
    let restored: trigon_model::Document = serde_json::from_str(&text).unwrap();
    assert_eq!(restored, s.doc);
    assert_eq!(trigon_model::build(&restored).unwrap(), detailed);
    let good = s.doc.clone();
    for bad in [
        json!({"gutter_width_mm":1e300}),
        json!({"downpipes":[{"id":"x","facade":"absent","along_mm":0,"diameter_mm":80,"wall_offset_mm":100,"bottom_mm":0,"shoe_mm":200}]}),
        json!({"joint_spacing_mm":0}),
    ] {
        assert!(s.execute("element.set", json!({"id":"b1.roof","properties":{"drainage":bad}})).is_err());
        assert_eq!(s.doc, good);
    }
    assert!(s.undo());
    assert_eq!(s.doc, before);
    assert_eq!(trigon_model::build(&s.doc).unwrap(), legacy);
}

#[test]
fn site_objects_are_real_sources_with_atomic_properties_and_exact_undo() {
    let mut s = session();
    let before = s.doc.clone();
    let building = trigon_model::build(&s.doc).unwrap();
    let object = json!({"id":"site.tree","name":"Proposed tree","intent":"proposal","geometry":{"kind":"tree","position_mm":[-18000,20000,0],"height_mm":4500,"crown_radius_mm":1500,"trunk_radius_mm":100}});
    s.execute("site.add", json!({"object":object})).unwrap();
    assert_eq!(trigon_model::build(&s.doc).unwrap(), building, "landscape is not another building or facade opening");
    let added = s.doc.clone();
    assert_eq!(s.execute("element.get", json!({"id":"site.tree"})).unwrap()["kind"], "tree");
    s.execute("element.set", json!({"id":"site.tree","properties":{"height_mm":5000,"x_mm":-17000}})).unwrap();
    assert_ne!(s.doc, added);
    let changed = s.doc.clone();
    for patch in
        [json!({"height_mm":1e300}), json!({"crown_radius_mm":-1}), json!({"x_mm":1e300}), json!({"intent":"measured"}), json!({"unknown":42})]
    {
        assert!(s.execute("element.set", json!({"id":"site.tree","properties":patch})).is_err());
        assert_eq!(s.doc, changed);
    }
    assert!(s.execute("site.add", json!({"object":object})).is_err());
    assert_eq!(s.doc, changed);
    let copy: trigon_model::Document = serde_json::from_str(&serde_json::to_string(&s.doc).unwrap()).unwrap();
    assert_eq!(copy, s.doc);
    s.execute("site.delete", json!({"id":"site.tree"})).unwrap();
    assert!(s.execute("element.get", json!({"id":"site.tree"})).is_err());
    assert!(s.undo());
    assert_eq!(s.doc, changed);
    assert!(s.undo());
    assert_eq!(s.doc, added);
    assert!(s.undo());
    assert_eq!(s.doc, before);
}

#[test]
fn site_ground_plan_picks_only_displayed_objects_and_keeps_building_schedules() {
    let mut s = session();
    let schedule = crate::opening_schedule(&s.doc, trigon_model::build::View::latest(&s.doc)).unwrap();
    let fixture: trigon_model::Document = serde_json::from_str(include_str!("../../model/tests/fixtures/courtyard.trigon")).unwrap();
    for mut object in fixture.site_objects {
        object.created = None;
        match &mut object.geometry {
            trigon_model::site::SiteGeometry::Ground { polygon, .. } => {
                for p in polygon {
                    p[1] -= 12000.;
                }
            }
            trigon_model::site::SiteGeometry::Tree { position_mm, .. }
            | trigon_model::site::SiteGeometry::Bench { position_mm, .. }
            | trigon_model::site::SiteGeometry::Planter { position_mm, .. } => position_mm[1] -= 12000.,
        }
        s.execute("site.add", json!({"object":object})).unwrap();
    }
    let unchanged = crate::opening_schedule(&s.doc, trigon_model::build::View::latest(&s.doc)).unwrap();
    assert_eq!(serde_json::to_value(schedule).unwrap(), serde_json::to_value(unchanged).unwrap());
    s.execute("view.define", json!({"id":"ground-site","name":"Ground","kind":{"view":"plan","level":0}})).unwrap();
    s.execute("view.define", json!({"id":"upper-site","name":"Upper","kind":{"view":"plan","level":1}})).unwrap();
    let ground = crate::drawing_view(&s.doc, "ground-site", trigon_model::build::View::latest(&s.doc)).unwrap();
    assert!(ground.element_segments.iter().any(|e| e.id == "site.paving"));
    // The tree is outside the building footprint. Picking uses the actual ground-plan outline.
    let hit = ground.pick_element([-5000., -17600.], 100.).unwrap();
    assert_eq!(hit.id, "site.tree");
    assert_eq!(hit.kind, crate::DrawingElementKind::Site);
    let upper = crate::drawing_view(&s.doc, "upper-site", trigon_model::build::View::latest(&s.doc)).unwrap();
    assert!(upper.element_segments.iter().all(|e| !e.id.starts_with("site.")));
}

#[test]
fn each_site_family_add_replace_delete_and_properties_preserve_exact_sources() {
    let mut s = session();
    let fixture: trigon_model::Document = serde_json::from_str(include_str!("../../model/tests/fixtures/courtyard.trigon")).unwrap();
    for mut object in fixture.site_objects {
        object.created = None;
        let before = s.doc.clone();
        let id = object.id.clone();
        s.execute("site.add", json!({"object":object})).unwrap();
        let added = s.doc.clone();
        object.name.push_str(" edited");
        s.execute("site.set", json!({"id":id,"object":object})).unwrap();
        let replaced = s.doc.clone();
        s.execute("element.set", json!({"id":id,"properties":{"name":"Proposal under review"}})).unwrap();
        let properties = s.doc.clone();
        s.execute("site.delete", json!({"id":id})).unwrap();
        assert!(s.undo());
        assert_eq!(s.doc, properties);
        assert!(s.undo());
        assert_eq!(s.doc, replaced);
        assert!(s.undo());
        assert_eq!(s.doc, added);
        assert!(s.undo());
        assert_eq!(s.doc, before);
    }
}

#[test]
fn dtm_terrain_converts_map_samples_to_the_local_datum_and_undo_is_exact() {
    let mut s = session();
    let before = s.doc.clone();
    s.execute("site.sample_terrain", json!({"origin":[-20000,-10000],"spacing_mm":[20000,10000],"columns":3,"rows":3})).unwrap();
    let t = s.doc.terrain.as_ref().unwrap();
    assert_eq!(t.origin, [-20000., -10000.]);
    assert_eq!(t.source.as_deref(), Some("dhm:dhm_terraen"));
    for row in 0..3 {
        for col in 0..3 {
            assert!((t.heights_mm[row][col] - (col as f64 * 100. + row as f64 * 200.)).abs() < 1e-6);
        }
    }
    let built = trigon_model::build(&s.doc).unwrap();
    assert!(built[0].terrain.is_some());
    let ifc = trigon_io::ifc::write(&s.doc, &built, "terrain.ifc", "2026-01-01T00:00:00Z");
    assert!(ifc.contains("IFCGEOGRAPHICELEMENT") && ifc.contains(".TERRAIN."));
    for read in [trigon_ifcread::read_ifc_lite(&ifc).unwrap(), trigon_ifcread::read_bimifc(&ifc).unwrap()] {
        assert!(read.failures.is_empty());
        let actual = read.elements.iter().find(|e| e.global_id == trigon_io::ifc::guid("trigon:site.terrain")).unwrap();
        assert!(trigon_ifcread::bounds_deviation(built[0].terrain.as_ref().unwrap().bounds().unwrap(), actual.mesh.bounds().unwrap()) < 0.01);
    }
    s.execute("edit.undo", json!({})).unwrap();
    assert_eq!(s.doc, before);
}

// Proposed public synthetic regression for crates/engine/src/tests.rs.
// Read-only audit output: not inserted into source or run by this agent.
// Uses the existing Canned test provider; no private model, source plan, account or network.
#[test]
fn moving_one_stair_storey_keeps_entrance_labels_units_other_floors_and_exact_undo() {
    for selected in [1, 2] {
        let mut s = Session::new(Box::new(Canned));
        s.doc.buildings.push(
            trigon_model::Building::new("b", "Synthetic entrance", &[[0., 0.], [20000., 0.], [20000., 16000.], [0., 16000.]], 4, 3000.).unwrap(),
        );
        s.execute(
            "interior.add_stairs",
            json!({"stairs":[{
                "id":"core", "levels":[0,1,2],
                "outline":[[4000,4000],[6000,4000],[6000,6000],[4000,6000]],
                "kind":"u_turn", "rounded":false, "landing":1000, "well":150
            }]}),
        )
        .unwrap();
        let units: Vec<Value> =
            ["st", "1", "2", "3"].into_iter().flat_map(|floor| ["tv", "th"].map(|door| json!({"floor":floor,"door":door}))).collect();
        s.doc.buildings[0].entrances = serde_json::from_value(json!([{
            "street":"Synthetic street", "number":"SYNTHETIC-731", "point":[0,5000],
            "stair":"core", "units":units
        }]))
        .unwrap();
        let render = |s: &Session, level| crate::plan::sheet_svg_text(&s.doc, level, trigon_model::build::View::latest(&s.doc)).unwrap();
        let marker = ">TV  SYNTHETIC-731  TH<";
        let before = s.doc.clone();
        let plans: Vec<_> = (0..4).map(|level| render(&s, level)).collect();
        assert!(plans.iter().all(|svg| svg.matches(marker).count() == 1));
        let original = before.buildings[0].stairs[0].clone();
        let out = s
            .execute(
                "edit.move",
                json!({
                    "targets":[format!("core.L{selected}")], "delta":[125.5,-75.25,0]
                }),
            )
            .unwrap();
        assert_eq!(out["targets"].as_array().unwrap().len(), 1);
        let b = &s.doc.buildings[0];
        assert_eq!(b.entrances.len(), 1);
        assert_eq!(b.entrances[0].units, before.buildings[0].entrances[0].units);
        assert_eq!(b.stairs.len(), 2);
        let unchanged = b.stairs.iter().find(|stair| stair.id == "core").unwrap();
        assert_eq!(unchanged.outline, original.outline);
        assert_eq!(unchanged.levels, original.levels.iter().copied().filter(|l| *l != selected).collect::<Vec<_>>());
        let moved = b.stairs.iter().find(|stair| stair.levels == [selected]).unwrap();
        for (old, new) in original.outline.iter().zip(&moved.outline) {
            assert_eq!(*new, [old[0] + 125.5, old[1] - 75.25]);
        }
        assert_eq!(out["targets"][0], json!(format!("{}.L{selected}", moved.id)));
        assert_eq!(s.doc.journal.len(), before.journal.len() + 1);
        // Confirmed current-source failure: the bound original is suppressed by the moved
        // outgoing stair, while the entrance still references only that original source.
        assert_eq!(render(&s, selected).matches(marker).count(), 1, "the physical entrance and its units must follow the selected stair occurrence");
        if selected == 2 {
            assert_eq!(render(&s, 3).matches(marker).count(), 1, "the receiving floor must resolve the moved previous departure");
        }
        for (level, plan) in plans.iter().enumerate().take(3) {
            if level != selected {
                assert_eq!(&render(&s, level), plan, "unselected floor {level}");
            }
        }
        let changed = s.doc.clone();
        let round_trip: trigon_model::Document = serde_json::from_str(&serde_json::to_string(&changed).unwrap()).unwrap();
        assert_eq!(round_trip, changed);
        s.execute("edit.undo", json!({})).unwrap();
        assert_eq!(s.doc, before);
        s.execute("edit.redo", json!({})).unwrap();
        assert_eq!(s.doc, changed);
    }
}

#[test]
fn stair_associations_survive_align_parameter_copy_replace_basement_and_cleanup() {
    let setup = || {
        let mut s = session();
        s.execute(
            "interior.add_stairs",
            json!({"stairs":[{
                "id":"core", "levels":[0,1,2], "kind":"u_turn", "rounded":false,
                "outline":[[-6000,-2000],[-4000,-2000],[-4000,0],[-6000,0]], "landing":1000
            }]}),
        )
        .unwrap();
        s.doc.buildings[0].entrances = serde_json::from_value(json!([{
            "street":"Synthetic street","number":"731","point":[-5000,-6000],"stair":"core",
            "units":[{"floor":"st","door":"tv"},{"floor":"1","door":"th"}]
        }]))
        .unwrap();
        // Legacy data round trips without a new empty key.
        let entrance = &s.doc.buildings[0].entrances[0];
        assert!(entrance.stair_by_level.is_empty());
        assert!(serde_json::to_value(entrance).unwrap().get("stair_by_level").is_none());
        s
    };
    for (command, params) in [
        ("edit.align", json!({"targets":["core.L1"],"axis":"x","coordinate":-4900.125})),
        ("element.set", json!({"id":"core.L1","properties":{"width_mm":2100.125}})),
        ("edit.copy_to_levels", json!({"targets":["core.L0"],"levels":[1],"replace":true})),
    ] {
        let mut s = setup();
        let before = s.doc.clone();
        let result = s.execute(command, params).unwrap();
        let building = &s.doc.buildings[0];
        let entrance = &building.entrances[0];
        let id = entrance.stair_at(1).unwrap().to_string();
        assert_ne!(id, "core", "{command}: {result}");
        assert!(building.stairs.iter().any(|stair| stair.id == id && stair.levels == [1]));
        assert_eq!(entrance.stair_at(0), Some("core"));
        assert_eq!(entrance.stair_at(2), Some("core"));
        assert_eq!(entrance.units, before.buildings[0].entrances[0].units);
        assert_eq!(building.entrances.len(), 1);
        assert_eq!(s.doc.journal.len(), before.journal.len() + 1);
        let edited = s.doc.clone();
        s.execute("edit.copy", json!({"targets":["core.L0"],"delta":[12000,0,0]})).unwrap();
        assert_eq!(s.doc.buildings[0].entrances, edited.buildings[0].entrances);
        s.execute("edit.undo", json!({})).unwrap();
        assert_eq!(s.doc, edited);
        s.execute("building.add_basement", json!({"floor":-2500})).unwrap();
        assert_eq!(s.doc.buildings[0].entrances[0].stair_at(2), Some(id.as_str()));
        assert_eq!(s.doc.buildings[0].entrances[0].stair_by_level.len(), 1);
        s.execute("edit.undo", json!({})).unwrap();
        assert_eq!(s.doc, edited);
        s.execute("edit.delete", json!({"targets":[format!("{id}.L1")]})).unwrap();
        assert!(s.doc.buildings[0].entrances[0].stair_by_level.is_empty());
        assert_eq!(s.doc.buildings[0].entrances[0].stair.as_deref(), Some("core"));
        s.execute("edit.undo", json!({})).unwrap();
        s.execute("edit.delete", json!({"targets":["stair:core"]})).unwrap();
        assert_eq!(s.doc.buildings[0].entrances[0].stair, None);
        assert_eq!(s.doc.buildings[0].entrances[0].stair_at(1), Some(id.as_str()));
        s.execute("edit.undo", json!({})).unwrap();
        s.execute("interior.clear", json!({"what":"stairs"})).unwrap();
        let entrance = &s.doc.buildings[0].entrances[0];
        assert_eq!(entrance.stair, None);
        assert!(entrance.stair_by_level.is_empty());
        assert_eq!(entrance.units, before.buildings[0].entrances[0].units);
        s.execute("edit.undo", json!({})).unwrap();
        assert_eq!(s.doc, edited);
        s.execute("edit.undo", json!({})).unwrap();
        assert_eq!(s.doc, before);
    }
    let mut s = setup();
    s.doc.buildings[0].entrances[0].stair_by_level.insert(usize::MAX, "core".into());
    let before = s.doc.clone();
    assert!(s.doc.buildings[0].add_basement(-2500., "Basement").is_err());
    assert_eq!(s.doc, before);
}

#[test]
fn precision_preview_connect_apply_audit_preserves_doors_other_storeys_and_history() {
    let mut s = Session::new(Box::new(Canned));
    s.doc
        .buildings
        .push(trigon_model::Building::new("b", "Precision fixture", &[[0., 0.], [20000., 0.], [20000., 12000.], [0., 12000.]], 2, 3000.).unwrap());
    s.execute(
        "interior.add_walls",
        json!({"walls":[
            {"id":"west","levels":[0,1],"a":[1000,4000],"b":[5000,4000],"thickness":100,"doors":[{"at":2000,"width":800}]},
            {"id":"east","levels":[0,1],"a":[5005,4000],"b":[10000,4000],"thickness":100,"doors":[{"at":2000,"width":800}]},
            {"id":"door-gap","levels":[0,1],"a":[10800,4000],"b":[14000,4000],"thickness":100}
        ]}),
    )
    .unwrap();
    let before = s.doc.clone();
    let before_dirty = s.dirty;
    let audit = s.execute("precision.audit", json!({"levels":[0],"gap_tolerance_mm":10,"max_results":1})).unwrap();
    assert_eq!(audit["total_findings"], 2);
    assert_eq!(audit["truncated"], true);
    assert_eq!(audit["findings"][0]["gap_mm"], 5.);
    let action = json!({"command":"edit.connect_walls","params":{"endpoints":[
        {"target":"west.L0","end":"b"},{"target":"east.L0","end":"a"}],"junction":[5002.5,4000],"max_move_mm":3}});
    let preview = s.execute("precision.preview", json!({"action":action})).unwrap();
    assert_eq!(s.doc, before);
    assert_eq!(s.dirty, before_dirty);
    assert_eq!(preview["applied"], false);
    assert_eq!(preview["validation"]["model_build"], "passed");
    assert!(!preview["changes"].as_array().unwrap().is_empty());
    assert_eq!(preview, s.execute("precision.preview", json!({"action":action})).unwrap());
    let mut wrong = action.clone();
    wrong["params"]["junction"] = json!([5002.6, 4000]);
    assert!(s.execute("precision.apply", json!({"preview_token":preview["preview_token"],"action":wrong})).is_err());
    assert_eq!(s.doc, before);
    s.execute("precision.apply", json!({"preview_token":preview["preview_token"],"action":action})).unwrap();
    assert_eq!(s.doc.journal.len(), before.journal.len() + 1);
    assert_eq!(s.doc.journal.last().unwrap().command, "edit.connect_walls");
    let walls = &s.doc.buildings[0].interior_walls;
    let west = walls.iter().find(|w| w.levels == [0] && w.id.starts_with("west.")).unwrap();
    let east = walls.iter().find(|w| w.levels == [0] && w.id.starts_with("east.")).unwrap();
    assert_eq!(west.b, [5002.5, 4000.]);
    assert_eq!(east.a, west.b);
    assert_eq!(west.doors[0].at, 2000.);
    assert_eq!(east.a[0] + east.doors[0].at, 7005.);
    for original in &before.buildings[0].interior_walls {
        let retained = walls.iter().find(|w| w.id == original.id).unwrap();
        assert_eq!(retained.a, original.a);
        assert_eq!(retained.b, original.b);
        assert_eq!(retained.doors, original.doors);
    }
    let audit = s.execute("precision.audit", json!({"levels":[0]})).unwrap();
    assert_eq!(audit["status"], "no_findings_in_scope", "{audit}");
    assert!(s.execute("precision.apply", json!({"preview_token":preview["preview_token"],"action":action})).is_err());
    let after = s.doc.clone();
    s.execute("edit.undo", json!({})).unwrap();
    assert_eq!(s.doc, before);
    // Read-only previews and audits must also preserve a pending Redo.
    s.execute("precision.preview", json!({"action":action})).unwrap();
    s.execute("precision.audit", json!({"levels":[0]})).unwrap();
    s.execute("edit.redo", json!({})).unwrap();
    assert_eq!(s.doc, after);
}

#[test]
fn precision_repairs_reject_wrong_scope_excess_motion_and_hosted_door_drift_atomically() {
    let mut s = session();
    s.execute(
        "interior.add_walls",
        json!({"walls":[
            {"id":"one","levels":[0,1],"a":[-6000,0],"b":[-2000,0],"thickness":100,"doors":[{"at":2000,"width":800}]},
            {"id":"two","levels":[0,1],"a":[-1995,0],"b":[2000,0],"thickness":100}
        ]}),
    )
    .unwrap();
    let before = s.doc.clone();
    for params in [
        json!({"endpoints":[{"target":"one.L0","end":"b"},{"target":"two.L0","end":"a"}],"junction":[-1998,1],"max_move_mm":10}),
        json!({"endpoints":[{"target":"one.L0","end":"b"},{"target":"two.L1","end":"a"}],"junction":[-1998,0],"max_move_mm":10}),
        json!({"endpoints":[{"target":"one.L0","end":"b"},{"target":"two.L0","end":"a"}],"junction":[-1998,0],"max_move_mm":1}),
        json!({"endpoints":[{"target":"one.L0","end":"b"},{"target":"one.L0","end":"a"}],"junction":[-1998,0],"max_move_mm":10}),
        json!({"endpoints":[{"target":"one.L0","end":"b"},{"target":"two.L0","end":"a"}],"junction":[1e300,0],"max_move_mm":300}),
    ] {
        assert!(s.execute("edit.connect_walls", params.clone()).is_err());
        assert_eq!(s.doc, before);
        assert!(s.execute("precision.preview", json!({"action":{"command":"edit.connect_walls","params":params}})).is_err());
        assert_eq!(s.doc, before);
    }
    assert!(s.execute("precision.preview", json!({"action":{"command":"document.open","params":{"path":"/etc/passwd"}}})).is_err());
    assert_eq!(s.doc, before);
}

#[test]
fn precision_label_moves_preserve_witnesses_styles_and_level_values() {
    let mut s = session();
    s.execute("view.define", json!({"id":"p","name":"Plan","kind":{"view":"plan","level":0}})).unwrap();
    let facade = s.doc.buildings[0].facades[0].id.clone();
    s.execute("view.define", json!({"id":"e","name":"Elevation","kind":{"view":"facade","facade":facade}})).unwrap();
    for (id, kind, delta) in [
        ("text", json!({"annotation":"text","at":[0,0],"text":"Kitchen","rotation_deg":30}), [0.125, -0.375]),
        ("leader", json!({"annotation":"leader","tip":[0,0],"elbow":[1000,500],"at":[2000,500],"text":"Detail"}), [0.125, -0.375]),
        ("dimension", json!({"annotation":"dimension","from":[0,0],"to":[4000,0],"offset_mm":300}), [0., 125.125]),
        ("level", json!({"annotation":"level","at":[1000,3000],"datum_mm":1000,"label":"FFL"}), [125.125, 0.]),
    ] {
        s.execute(
            "annotation.define",
            json!({"id":id,"view":if id=="level" {"e"} else {"p"},"size_mm":2.5,"style":{"family":"serif","bold":true},"kind":kind}),
        )
        .unwrap();
        let before = s.doc.clone();
        let action = json!({"command":"annotation.move_label","params":{"id":id,"delta":delta}});
        let preview = s.execute("precision.preview", json!({"action":action})).unwrap();
        assert_eq!(s.doc, before);
        s.execute("precision.apply", json!({"preview_token":preview["preview_token"],"action":action})).unwrap();
        let mut expected = before.annotations.last().unwrap().clone();
        match &mut expected.kind {
            trigon_model::annotation::AnnotationKind::Text { at, .. } | trigon_model::annotation::AnnotationKind::Leader { at, .. } => {
                at[0] += delta[0];
                at[1] += delta[1];
            }
            trigon_model::annotation::AnnotationKind::Dimension { offset_mm, .. } => *offset_mm += delta[1],
            trigon_model::annotation::AnnotationKind::Level { at, .. } => at[0] += delta[0],
            _ => assert_eq!(expected.id, "unsupported fixture"),
        }
        assert_eq!(s.doc.annotations.last().unwrap(), &expected);
        let changed = s.doc.clone();
        s.execute("edit.undo", json!({})).unwrap();
        assert_eq!(s.doc, before);
        s.execute("edit.redo", json!({})).unwrap();
        assert_eq!(s.doc, changed);
    }
    let before = s.doc.clone();
    for (id, delta) in [("dimension", [1., 0.]), ("level", [0., 1.]), ("text", [f64::MAX, 0.])] {
        assert!(s.execute("annotation.move_label", json!({"id":id,"delta":delta})).is_err());
        assert_eq!(s.doc, before);
    }
}

#[test]
fn precision_lettering_audit_measures_real_fonts_scale_rotation_crop_and_exact_values() {
    let mut s = Session::new(Box::new(Canned));
    s.doc
        .buildings
        .push(trigon_model::Building::new("b", "Lettering fixture", &[[0., 0.], [12000., 0.], [12000., 8000.], [0., 8000.]], 2, 3000.).unwrap());
    s.execute("view.define", json!({"id":"p","name":"Plan","kind":{"view":"plan","level":0}})).unwrap();
    s.execute("view.configure", json!({"id":"p","template":{"name":"100","sizing":{"mode":"fixed","denominator":100,"centre":[6000,4000]}}}))
        .unwrap();
    for (id, at) in [("wide-a", [2000, 4000]), ("wide-b", [3500, 4000])] {
        s.execute(
            "annotation.define",
            json!({"id":id,"view":"p","size_mm":2.5,"kind":{"annotation":"text","at":at,"text":"WWWWWWWW","rotation_deg":0}}),
        )
        .unwrap();
    }
    s.execute(
        "annotation.define",
        json!({"id":"measured","view":"p","kind":{"annotation":"dimension","from":[2000,1000],"to":[4000.125,1000],"offset_mm":300}}),
    )
    .unwrap();
    let before = s.doc.clone();
    let audit = s.execute("view.audit", json!({"id":"p"})).unwrap();
    assert_eq!(s.doc, before);
    assert_eq!(audit, s.execute("view.audit", json!({"id":"p"})).unwrap());
    let labels = audit["labels"].as_array().unwrap();
    let a = labels.iter().find(|l| l["source"]["id"] == "wide-a").unwrap();
    let b = labels.iter().find(|l| l["source"]["id"] == "wide-b").unwrap();
    let points = a["ink_corners_mm"].as_array().unwrap();
    let width = (points[1][0].as_f64().unwrap() - points[0][0].as_f64().unwrap()) / 100.;
    assert!(width > 18. && width < 19.);
    assert!(audit["findings"].as_array().unwrap().iter().any(|f| f["kind"] == "potential_text_overlap"
        && f["keys"].as_array().unwrap().contains(&a["key"])
        && f["keys"].as_array().unwrap().contains(&b["key"])));
    let measured = labels.iter().find(|l| l["source"]["id"] == "measured").unwrap();
    assert_eq!(measured["measurement_mm"], 2000.125);
    assert_eq!(measured["text"], "2000.1");
    let action = json!({"command":"annotation.move_label","params":{"id":"wide-b","delta":[2000.125,0]}});
    let preview = s.execute("precision.preview", json!({"action":action})).unwrap();
    s.execute("precision.apply", json!({"preview_token":preview["preview_token"],"action":action})).unwrap();
    let fixed = s.execute("view.audit", json!({"id":"p"})).unwrap();
    assert!(!fixed["findings"].as_array().unwrap().iter().any(|f| f["kind"] == "potential_text_overlap"), "{fixed}");
    s.execute("edit.undo", json!({})).unwrap();
    assert_eq!(s.doc, before);
    s.execute("annotation.define",json!({"id":"wide-a","view":"p","size_mm":2.5,"style":{"family":"serif","bold":true,"italic":true},"kind":{"annotation":"text","at":[2000,4000],"text":"WWWWWWWW","rotation_deg":90}})).unwrap();
    s.execute(
        "view.configure",
        json!({"id":"p","template":{"name":"crop","sizing":{"mode":"fixed","denominator":50,"centre":[6000,4000]},"crop":[1000,3000,11000,4500]}}),
    )
    .unwrap();
    let audit = s.execute("view.audit", json!({"id":"p"})).unwrap();
    let rotated = audit["labels"].as_array().unwrap().iter().find(|l| l["source"]["id"] == "wide-a").unwrap();
    assert_eq!(rotated["rotation_deg"], 90.);
    assert_eq!(rotated["clipped"], true);
    let corners = rotated["ink_corners_mm"].as_array().unwrap();
    let vertical = (corners[1][1].as_f64().unwrap() - corners[0][1].as_f64().unwrap()) / 50.;
    assert!((vertical - 17.756).abs() < 0.005);
    assert!(audit["findings"].as_array().unwrap().iter().any(|f| f["kind"] == "clipped_label" && f["source"]["id"] == "wide-a"));
}

#[test]
fn precision_audit_reports_overlapping_stair_sources_without_mutating_invalid_model() {
    let mut s = session();
    s.execute(
        "interior.add_stairs",
        json!({"stairs":[{"id":"core","levels":[0],"outline":[[-6000,-2000],[-4000,-2000],[-4000,0],[-6000,0]],"kind":"u_turn","rounded":false}]}),
    )
    .unwrap();
    let mut duplicate = s.doc.buildings[0].stairs[0].clone();
    duplicate.id = "other".into();
    s.doc.buildings[0].stairs.push(duplicate);
    let before = s.doc.clone();
    let audit = s.execute("precision.audit", json!({"levels":[0]})).unwrap();
    assert_eq!(audit["geometry_valid"], false);
    assert!(audit["findings"].as_array().unwrap().iter().any(|f| f["kind"] == "overlapping_stair_sources"));
    assert_eq!(s.doc, before);
}

#[test]
fn precision_registered_pixel_conversion_keeps_opt_in_decimals_and_legacy_array_output() {
    let mut s = session();
    s.doc.drawings.push(trigon_model::Drawing {
        name: "synthetic".into(),
        path: "/virtual/not-read.pdf".into(),
        page: 1,
        rotate: 0,
        width: 1000,
        height: 1000,
        note: String::new(),
        facades: Vec::new(),
        plans: vec![trigon_model::PlanRegistration { levels: vec![0], a: 1.125, b: 0.25, tx: 1000.0625, ty: -500.125 }],
    });
    let before = s.doc.clone();
    let exact = s.execute("drawing.to_site", json!({"name":"synthetic","level":0,"exact":true,"points":[[100.125,200.375]]})).unwrap();
    assert_eq!(exact, json!([[1162.796875, -700.515625]]));
    let old = s.execute("drawing.to_site", json!({"name":"synthetic","level":0,"points":[[100.125,200.375]]})).unwrap();
    assert_eq!(old, json!([[1163., -701.]]));
    assert!(s.execute("drawing.to_site", json!({"name":"synthetic","level":0,"exact":true,"points":[[1e300,0]]})).is_err());
    assert_eq!(s.doc, before);
}

#[test]
fn precision_sanity_checks_find_kinks_misalignment_and_overlaps_without_straightening() {
    for (other, thickness, kind) in [
        (json!({"a":[5000,4000],"b":[9000,4200]}), 100., "sudden_wall_angle_change"),
        (json!({"a":[5000,4000],"b":[7000,6000]}), 100., "sudden_wall_angle_change"),
        (json!({"a":[5005,4005],"b":[9000,4005]}), 100., "misaligned_wall_run"),
        (json!({"a":[1500,4000],"b":[4500,4000]}), 100., "near_parallel_wall_overlap"),
    ] {
        let mut s = Session::new(Box::new(Canned));
        s.doc
            .buildings
            .push(trigon_model::Building::new("b", "Sanity fixture", &[[0., 0.], [12000., 0.], [12000., 8000.], [0., 8000.]], 1, 3000.).unwrap());
        s.execute(
            "interior.add_walls",
            json!({"walls":[
                {"id":"first","levels":[0],"a":[1000,4000],"b":[5000,4000],"thickness":100},
                {"id":"second","levels":[0],"a":other["a"],"b":other["b"],"thickness":thickness}
            ]}),
        )
        .unwrap();
        let before = s.doc.clone();
        let audit = s.execute("precision.audit", json!({"levels":[0]})).unwrap();
        assert!(audit["findings"].as_array().unwrap().iter().any(|f| f["kind"] == kind), "expected {kind}: {audit}");
        assert_eq!(s.doc, before);
        if other["b"] == json!([7000, 6000]) {
            let reviewed = s.execute("precision.audit", json!({"levels":[0],"expected_joint_angles_deg":[0,45,90]})).unwrap();
            assert!(!reviewed["findings"].as_array().unwrap().iter().any(|f| f["kind"] == "sudden_wall_angle_change"));
            assert_eq!(s.doc, before);
        }
    }
    // Different-thickness walls can intentionally align one face while their axes differ.
    let mut s = session();
    s.execute(
        "interior.add_walls",
        json!({"walls":[
            {"id":"thick","levels":[0],"a":[-6000,0],"b":[-2000,0],"thickness":200},
            {"id":"thin","levels":[0],"a":[-1995,50],"b":[2000,50],"thickness":100},
            {"id":"perpendicular","levels":[0],"a":[0,-2000],"b":[0,2000],"thickness":100}
        ]}),
    )
    .unwrap();
    let audit = s.execute("precision.audit", json!({"levels":[0]})).unwrap();
    assert!(
        !audit["findings"].as_array().unwrap().iter().any(|f| matches!(f["kind"].as_str(), Some("misaligned_wall_run" | "sudden_wall_angle_change"))),
        "{audit}"
    );
}

#[test]
fn measured_local_buildings_keep_small_features_other_buildings_and_exact_undo() {
    let mut s = Session::new(Box::new(Canned));
    let outline = json!([[0, 0], [6000, 0], [6000, 4000], [3000, 4100], [0, 4000], [0, 0]]);
    let before = s.doc.clone();
    let params = json!({"id":"west","name":"Measured west","footprint":outline,"storeys":3,"storey_height_mm":3050.125,"ground_floor_mm":650.25,"wall_thickness_mm":480,"evidence":[{"source":"synthetic:plan","supplies":"measured footprint","attribution":"test"}]});
    let action = json!({"command":"building.add","params":params});
    let preview = s.execute("precision.preview", json!({"action":action})).unwrap();
    assert_eq!(s.doc, before);
    let result = s.execute("precision.apply", json!({"preview_token":preview["preview_token"],"action":action})).unwrap();
    assert_eq!(s.doc.buildings[0].footprint.len(), 5, "100 mm deviation must not be removed");
    assert_eq!(s.doc.buildings[0].footprint[3], [3000., 4100.]);
    assert_eq!(s.doc.buildings[0].storeys[1].elevation, 3700.375);
    assert_eq!(result["facades"].as_array().unwrap().len(), 5);
    let first = s.doc.clone();
    s.execute(
        "building.add",
        json!({"id":"east","name":"Measured east","footprint":[[10000,0],[16000,0],[16000,4000],[10000,4000]],"storeys":2,"storey_height_mm":3000}),
    )
    .unwrap();
    assert_eq!(s.doc.buildings[0], first.buildings[0]);
    let changed = s.doc.clone();
    assert_eq!(s.doc.journal.len(), 2);
    s.execute("edit.undo", json!({})).unwrap();
    assert_eq!(s.doc, first);
    s.execute("edit.undo", json!({})).unwrap();
    assert_eq!(s.doc, before);
    s.execute("edit.redo", json!({})).unwrap();
    s.execute("edit.redo", json!({})).unwrap();
    assert_eq!(s.doc, changed);
    for params in [
        json!({"id":"west","name":"Duplicate","footprint":outline,"storeys":1,"storey_height_mm":3000}),
        json!({"id":"crossed","name":"Crossed","footprint":[[20000,0],[26000,0],[20000,4000],[26000,4000]],"storeys":1,"storey_height_mm":3000}),
        json!({"id":"hostile","name":"Hostile","footprint":[[1e300,0],[26000,0],[26000,4000],[20000,4000]],"storeys":usize::MAX,"storey_height_mm":3000}),
    ] {
        assert!(s.execute("building.add", params).is_err());
        assert_eq!(s.doc, changed);
    }
    let mut contextual = Session::new(Box::new(Canned));
    contextual.execute("building.add",json!({"id":"context","name":"Explicit context simplification","footprint":outline,"storeys":1,"storey_height_mm":3000,"preserve_vertices":false})).unwrap();
    assert_eq!(contextual.doc.buildings[0].footprint.len(), 4);
}

#[test]
fn precision_journal_replays_underlying_actions_without_ephemeral_tokens() {
    let mut s = Session::new(Box::new(Canned));
    s.execute(
        "building.add",
        json!({"id":"b","name":"Replay fixture","footprint":[[0,0],[12000,0],[12000,8000],[0,8000]],"storeys":2,"storey_height_mm":3000}),
    )
    .unwrap();
    s.execute("interior.add_walls", json!({"walls":[{"id":"wall","levels":[0,1],"a":[1000,3000],"b":[6000,3000],"thickness":100}]})).unwrap();
    let action = json!({"command":"edit.move","params":{"targets":["wall.L0"],"delta":[0.125,0.375,0]}});
    let preview = s.execute("precision.preview", json!({"action":action})).unwrap();
    s.execute("precision.apply", json!({"preview_token":preview["preview_token"],"action":action})).unwrap();
    assert!(s.doc.journal.iter().all(|entry| entry.command != "precision.apply"), "ephemeral conflict tokens must not be required for replay");
    let persisted: trigon_model::Document = serde_json::from_str(&serde_json::to_string(&s.doc).unwrap()).unwrap();
    let mut replay = Session::new(Box::new(Canned));
    for entry in persisted.journal {
        replay.execute(&entry.command, entry.params).unwrap();
    }
    assert_eq!(replay.doc, s.doc);
}

#[test]
fn multi_building_plans_keep_upper_floors_when_another_building_is_lower() {
    for low_first in [false, true] {
        let mut s = Session::new(Box::new(Canned));
        let high =
            json!({"id":"high","name":"High building","footprint":[[0,0],[12000,0],[12000,8000],[0,8000]],"storeys":3,"storey_height_mm":3000});
        let low =
            json!({"id":"low","name":"Low building","footprint":[[20000,0],[25000,0],[25000,6000],[20000,6000]],"storeys":1,"storey_height_mm":3000});
        for params in if low_first { [low, high] } else { [high, low] } {
            s.execute("building.add", params).unwrap();
        }
        s.execute("view.define", json!({"id":"upper","name":"Upper floor","kind":{"view":"plan","level":2}})).unwrap();
        for ranged in [false, true] {
            if ranged {
                s.execute(
                    "view.configure",
                    json!({"id":"upper","template":{"name":"Actual cut","range":{"top_mm":3000,"cut_mm":1000,"bottom_mm":0,"depth_mm":-300}}}),
                )
                .unwrap();
            }
            let drawing = crate::drawing_view(&s.doc, "upper", trigon_model::build::View::latest(&s.doc)).unwrap();
            assert!(drawing.element_regions.iter().any(|r| r.id.starts_with("high.")), "high building present");
            assert!(!drawing.element_regions.iter().any(|r| r.id.starts_with("low.")), "absent floor must not be invented");
            assert!(drawing.svg.contains("Upper floor"));
        }
        let before = s.doc.clone();
        assert!(s.execute("view.define", json!({"id":"invalid","name":"Missing floor","kind":{"view":"plan","level":9}})).is_err());
        assert_eq!(s.doc, before);
    }
}

#[test]
fn multi_building_plan_headers_do_not_misidentify_crops_in_any_plan_path() {
    let mut wrong = Vec::new();
    for west_first in [true, false] {
        for path in ["symbolic", "ranged", "attic", "attic-ranged"] {
            let mut s = Session::new(Box::new(Canned));
            let west =
                trigon_model::Building::new("west", "West fixture", &[[0., 0.], [12000., 0.], [12000., 9000.], [0., 9000.]], 1, 3000.).unwrap();
            let east = trigon_model::Building::new("east", "East fixture", &[[50000., 0.], [62000., 0.], [62000., 9000.], [50000., 9000.]], 2, 3000.)
                .unwrap();
            s.doc.buildings = if west_first { vec![west, east] } else { vec![east, west] };
            let level = if path.starts_with("attic") {
                s.execute("building.set_roof", json!({"building":"west","kind":"pitched","pitch_deg":35})).unwrap();
                s.execute("building.set_attic", json!({"building":"west","floor":2250})).unwrap();
                1
            } else {
                0
            };
            let mut template = json!({"name":"East detail","crop":[49000,-1000,63000,10000],"caption":"Public project",
                "sizing":{"mode":"fixed","denominator":100,"centre":[56000,4500]}});
            if path.ends_with("ranged") {
                template["range"] = json!({"top_mm":3000,"cut_mm":1000,"bottom_mm":0,"depth_mm":-300});
            }
            s.execute("view.define", json!({"id":"detail","name":"East review plan","kind":{"view":"plan","level":level},"template":template}))
                .unwrap();
            let sheet = crate::views::sheet(&s.doc, "detail", trigon_model::build::View::latest(&s.doc)).unwrap();
            if !sheet.project.is_empty() {
                wrong.push((west_first, path, sheet.project.clone()));
            }
            assert_eq!(sheet.caption, "Public project");
            let drawing = crate::drawing_view(&s.doc, "detail", trigon_model::build::View::latest(&s.doc)).unwrap();
            assert!(drawing.svg.contains("Public project"));
            if drawing.svg.contains("West fixture") || drawing.svg.contains("East fixture") {
                assert!(!sheet.project.is_empty(), "only the incorrect automatic project header may name a source building");
            }
        }
    }
    assert!(wrong.is_empty(), "multi-building index-wide plans cannot use the first building as their project header: {wrong:?}");
}

#[test]
fn plan_headers_preserve_the_sole_building_present_at_a_floor_index() {
    let mut s = Session::new(Box::new(Canned));
    s.doc
        .buildings
        .push(trigon_model::Building::new("west", "West fixture", &[[0., 0.], [12000., 0.], [12000., 9000.], [0., 9000.]], 1, 3000.).unwrap());
    s.doc.buildings.push(
        trigon_model::Building::new("east", "East fixture", &[[50000., 0.], [62000., 0.], [62000., 9000.], [50000., 9000.]], 2, 3000.).unwrap(),
    );
    for ranged in [false, true] {
        let template =
            if ranged { json!({"name":"Ranged","range":{"top_mm":3000,"cut_mm":1000,"bottom_mm":0,"depth_mm":-300}}) } else { Value::Null };
        s.execute("view.define", json!({"id":"upper","name":"Upper","kind":{"view":"plan","level":1},"template":template})).unwrap();
        let sheet = crate::views::sheet(&s.doc, "upper", trigon_model::build::View::latest(&s.doc)).unwrap();
        assert_eq!(sheet.project, "East fixture");
    }
    s.doc.buildings.remove(1);
    s.doc.views.clear();
    s.execute("building.set_roof", json!({"kind":"pitched","pitch_deg":35})).unwrap();
    s.execute("building.set_attic", json!({"floor":2250})).unwrap();
    s.execute("view.define", json!({"id":"roof","name":"Roof","kind":{"view":"plan","level":1}})).unwrap();
    assert_eq!(crate::views::sheet(&s.doc, "roof", trigon_model::build::View::latest(&s.doc)).unwrap().project, "West fixture");
}

mod rooflight_consumers_tests {
    use super::*;

    fn fixture() -> Session {
        let mut s = Session::new(Box::new(Canned));
        s.execute("building.add", json!({"id":"b","name":"Public rooflight consumers","footprint":[[0,0],[24000,0],[24000,12000],[0,12000]],"storeys":1,"storey_height_mm":3000})).unwrap();
        s.execute("building.set_roof", json!({"kind":"pitched","pitch_deg":46,"thickness_mm":220})).unwrap();
        s.execute(
            "rooflight.add",
            json!({"rooflight":{"id":"light","facade":"b.f1","u_mm":10000,"run_mm":500,
            "window_type":"window.casement.2","width_mm":800,"height_mm":1000,"stand_off_mm":100,"flashing_mm":100}}),
        )
        .unwrap();
        s
    }

    #[test]
    fn rooflight_schedule_uses_actual_slope_plane_dimensions_and_roof_host() {
        let s = fixture();
        let before = s.doc.clone();
        let schedule = crate::opening_schedule(&s.doc, trigon_model::build::View::latest(&s.doc)).unwrap();
        assert_eq!(schedule.rows.len(), 1, "one actual roof window, no flashing/void counted as fills");
        let row = &schedule.rows[0];
        assert_eq!(row.id, "b.rooflight.light");
        assert_eq!(row.host_id, "b.roof");
        assert_eq!((row.width_mm, row.height_mm, row.area_m2), (800., 1000., Some(0.8)));
        assert_eq!(row.kind, crate::ScheduleOpeningKind::Window);
        assert_eq!(row.quantity_basis, crate::OpeningQuantityBasis::OuterLining);
        assert!(row.external);
        assert_eq!((schedule.totals.instances, schedule.totals.windows, schedule.totals.area_excluded), (1, 1, 0));
        assert_eq!(s.doc, before);
    }

    #[test]
    fn rooflight_drawing_strokes_pick_the_same_editable_source_as_3d() {
        let mut s = fixture();
        s.execute("view.define", json!({"id":"front","name":"Rooflight facade","kind":{"view":"facade","facade":"b.f1"}})).unwrap();
        let before = s.doc.clone();
        let preview = crate::drawing_view(&s.doc, "front", trigon_model::build::View::latest(&s.doc)).unwrap();
        let segment = preview
            .element_segments
            .iter()
            .find(|s| s.id == "b.rooflight.light" && s.kind == crate::DrawingElementKind::Opening)
            .expect("visible roof window owns drawing strokes");
        let [a, b] = segment.segment;
        let hit = preview.pick_element([(a[0] + b[0]) / 2., (a[1] + b[1]) / 2.], 0.01).unwrap();
        assert_eq!(hit.id, "b.rooflight.light");
        assert!(preview.element_segments.iter().any(|s| s.id == "b.rooflight.light.flashing"));
        s.execute("element.get", json!({"id":hit.id})).unwrap();
        s.execute("element.get", json!({"id":"b.rooflight.light.flashing"})).unwrap();
        assert_eq!(s.doc, before);
    }

    #[test]
    fn rooflight_preflights_apply_once_replay_and_keep_redo_after_failed_preview() {
        let mut s = fixture();
        let before = s.doc.clone();
        let spec = json!({"id":"other","facade":"b.f1","u_mm":14000,"run_mm":500,
            "window_type":"window.casement.2","width_mm":800,"height_mm":1000,"stand_off_mm":100,"flashing_mm":100});
        let action = json!({"command":"rooflight.add","params":{"rooflight":spec}});
        let preview = s.execute("precision.preview", json!({"action":action})).unwrap();
        assert_eq!(s.doc, before);
        s.execute("precision.apply", json!({"action":preview["action"],"preview_token":preview["preview_token"]})).unwrap();
        let accepted = s.doc.clone();
        assert_eq!(accepted.journal.len(), before.journal.len() + 1);
        assert_eq!(accepted.journal.last().unwrap().command, "rooflight.add");
        s.execute("edit.undo", json!({})).unwrap();
        assert_eq!(s.doc, before);
        let mut invalid = action.clone();
        invalid["params"]["rooflight"]["run_mm"] = json!(-1);
        assert!(s.execute("precision.preview", json!({"action":invalid})).is_err());
        assert_eq!(s.doc, before);
        s.execute("edit.redo", json!({})).unwrap();
        assert_eq!(s.doc, accepted);
        assert!(s.execute("precision.apply", json!({"action":preview["action"],"preview_token":preview["preview_token"]})).is_err());
        let remove = s.execute("precision.preview", json!({"action":{"command":"rooflight.remove","params":{"id":"other"}}})).unwrap();
        assert_eq!(s.doc, accepted);
        s.execute("precision.apply", json!({"action":remove["action"],"preview_token":remove["preview_token"]})).unwrap();
        assert_eq!(s.doc.journal.last().unwrap().command, "rooflight.remove");
        let final_doc = s.doc.clone();
        s.execute("edit.undo", json!({})).unwrap();
        assert_eq!(s.doc, accepted);
        let mut replay = Session::new(Box::new(Canned));
        for entry in &final_doc.journal {
            replay.execute(&entry.command, entry.params.clone()).unwrap();
        }
        assert_eq!(replay.doc, final_doc);
    }
}
