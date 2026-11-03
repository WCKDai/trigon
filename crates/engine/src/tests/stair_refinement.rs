//! Public synthetic source edits, shared by people and MCP callers.
use super::{Canned, Session};
use serde_json::{Value, json};
use trigon_model::{Document, Stair};

fn fixture() -> Session {
    let mut s = Session::new(Box::new(Canned));
    s.execute(
        "building.add",
        json!({"id":"b","name":"Public stair refinement",
        "footprint":[[0,0],[16000,0],[16000,12000],[0,12000]],"storeys":4,
        "storey_height_mm":3000,"wall_thickness_mm":400}),
    )
    .unwrap();
    s.execute(
        "interior.add_stairs",
        json!({"building":"b","stairs":[
            {"id":"before","levels":[0],"outline":[[9000,2000],[11500,2000],[11500,4200],[9000,4200]]},
            {"id":"core","levels":[0,1,2],"outline":[[2000,2000],[4500,2000],[4500,4200],[2000,4200]],
             "tread":276,"kind":"u_turn","landing":1000,"well":150,"rounded":false},
            {"id":"after","levels":[2],"outline":[[9000,2000],[11500,2000],[11500,4200],[9000,4200]]}
        ]}),
    )
    .unwrap();
    s
}

fn split(s: &Session) -> Value {
    let source = &s.doc.buildings[0].stairs[1];
    let mut retained = source.clone();
    retained.levels = vec![0];
    let mut upper = source.clone();
    upper.id = "core.upper".into();
    upper.levels = vec![1, 2];
    upper.landing = Some(900.125);
    json!({"building":"b","expected_source":source,"partitions":[retained,upper]})
}

fn glb(doc: &Document) -> Vec<u8> {
    trigon_io::glb::write(&trigon_model::build(doc).unwrap(), &[])
}

fn refuses(s: &mut Session, command: &str, params: Value, message: &str) {
    let state = (s.doc.clone(), s.undo.clone(), s.redo.clone(), s.path.clone(), s.dirty);
    let error = s.execute(command, params).unwrap_err();
    assert!(!matches!(error, crate::EngineError::Internal(_)), "{error}");
    assert!(error.to_string().contains(message), "{error}; expected {message}");
    assert_eq!((s.doc.clone(), s.undo.clone(), s.redo.clone(), s.path.clone(), s.dirty), state);
}

#[test]
fn stair_refinement_split_preserves_slot_order_and_maps_exact_occurrences() {
    let mut s = fixture();
    let before = s.doc.clone();
    let params = split(&s);
    let result = s.execute("stair.refine_source", params.clone()).unwrap();
    let b = &s.doc.buildings[0];
    assert_eq!(b.stairs.iter().map(|s| s.id.as_str()).collect::<Vec<_>>(), ["before", "core", "after", "core.upper"]);
    assert_eq!(b.stairs[0], before.buildings[0].stairs[0]);
    assert_eq!(b.stairs[2], before.buildings[0].stairs[2]);
    assert_eq!(serde_json::to_value(&b.stairs[1]).unwrap(), params["partitions"][0]);
    assert_eq!(serde_json::to_value(&b.stairs[3]).unwrap(), params["partitions"][1]);
    assert_eq!(result["original"], json!({"id":"core","index":1,"levels":[0,1,2]}));
    assert_eq!(
        result["partitions"],
        json!([
            {"id":"core","index":1,"levels":[0]}, {"id":"core.upper","index":3,"levels":[1,2]}
        ])
    );
    assert_eq!(
        result["displayed_id_mapping"],
        json!([
            {"departure_level":0,"before":"core.L0","after":"core.L0"},
            {"departure_level":1,"before":"core.L1","after":"core.upper.L1"},
            {"departure_level":2,"before":"core.L2","after":"core.upper.L2"}
        ])
    );
    assert_eq!(result["departure_levels"], json!([0, 1, 2]));
    assert_eq!(result["arrival_levels"], json!([1, 2, 3]));
    assert_eq!(result["affected_levels"], json!([0, 1, 2, 3]));
    assert_eq!(result["validation"]["changed_shaft_floors"], "passed");
    assert_eq!(s.doc.journal.len(), before.journal.len() + 1);
    let mut expected = before;
    expected.buildings[0].stairs = b.stairs.clone();
    expected.journal = s.doc.journal.clone();
    assert_eq!(s.doc, expected);
}

#[test]
fn stair_refinement_precision_and_direct_have_one_exact_journal_entry() {
    let mut s = fixture();
    let before = s.doc.clone();
    let before_glb = glb(&before);
    let params = split(&s);
    let action = json!({"command":"stair.refine_source","params":params});
    let preview = s.execute("precision.preview", json!({"action":action})).unwrap();
    assert_eq!(s.doc, before);
    s.execute("precision.apply", json!({"action":action,"preview_token":preview["preview_token"]})).unwrap();
    let after = s.doc.clone();
    let after_glb = glb(&after);
    assert_eq!(after.journal.last().unwrap().command, "stair.refine_source");
    assert_eq!(after.journal.len(), before.journal.len() + 1);
    s.execute("edit.undo", json!({})).unwrap();
    assert_eq!(s.doc, before);
    assert_eq!(glb(&s.doc), before_glb);
    refuses(&mut s, "precision.apply", json!({"action":action,"preview_token":"stale"}), "stale");
    let mut stale = params.clone();
    stale["expected_source"]["landing"] = json!(1001);
    refuses(&mut s, "stair.refine_source", stale, "stale");
    s.execute("edit.redo", json!({})).unwrap();
    assert_eq!(s.doc, after);
    assert_eq!(glb(&s.doc), after_glb);
    let mut replay = Session::new(Box::new(Canned));
    for e in &after.journal {
        replay.execute(&e.command, e.params.clone()).unwrap();
    }
    assert_eq!(replay.doc, after);
    assert_eq!(glb(&replay.doc), after_glb);
    let path = std::env::temp_dir().join(format!("trigon-source-refinement-{}.trigon", std::process::id()));
    s.execute("document.save", json!({"path":path})).unwrap();
    let bytes = std::fs::read(&path).unwrap();
    s.execute("document.open", json!({"path":path})).unwrap();
    s.execute("document.save", json!({"path":path})).unwrap();
    assert_eq!(std::fs::read(&path).unwrap(), bytes);
    std::fs::remove_file(path).unwrap();
    assert_eq!(s.doc, after);
    assert_eq!(glb(&s.doc), after_glb);
}

#[test]
fn stair_refinement_whole_source_noop_native_nulls_and_partition_order() {
    let mut s = fixture();
    let original = s.doc.buildings[0].stairs[1].clone();
    let mut p = json!({"building":"b","expected_source":original,"partitions":[original]});
    refuses(&mut s, "stair.refine_source", p.clone(), "no source change");
    refuses(&mut s, "precision.preview", json!({"action":{"command":"stair.refine_source","params":p}}), "no source change");
    p["partitions"][0]["kind"] = json!("double_quarter_turn");
    for point in p["partitions"][0]["outline"].as_array_mut().unwrap() {
        point[0] = json!(point[0].as_f64().unwrap() + 1500.);
    }
    p["partitions"][0]["winders"] = json!(6);
    p["partitions"][0]["turn_depths"] = json!([700.125, 800.25]);
    s.execute("stair.refine_source", p).unwrap();
    let original = s.doc.buildings[0].stairs[1].clone();
    let mut next = original.clone();
    next.winders = None;
    next.turn_depths = None;
    s.execute("stair.refine_source", json!({"building":"b","expected_source":original,"partitions":[next]})).unwrap();
    assert_eq!(s.doc.buildings[0].stairs[1], next);
    let source = s.doc.buildings[0].stairs[1].clone();
    let mut a = source.clone();
    a.id = "last.request.first.append".into();
    a.levels = vec![2];
    let mut b = source.clone();
    b.levels = vec![1];
    let mut c = source.clone();
    c.id = "first.request.last.append".into();
    c.levels = vec![0];
    s.execute("stair.refine_source", json!({"building":"b","expected_source":source,"partitions":[a,b,c]})).unwrap();
    assert_eq!(
        s.doc.buildings[0].stairs.iter().map(|s| s.id.as_str()).collect::<Vec<_>>(),
        ["before", "core", "after", "last.request.first.append", "first.request.last.append"]
    );
    assert_eq!(s.doc.buildings[0].stairs[1].levels, [1]);
}

#[test]
fn stair_refinement_maps_groups_and_effective_entrances_without_rewriting_metadata() {
    for retain_ground in [true, false] {
        let mut s = fixture();
        s.doc.buildings[0].entrances = serde_json::from_value(json!([
            {"street":"Public Test Road","number":"7A","point":[1000.125,-42.875],"stair":"core",
             "stair_by_level":{"1":"after","2":"core","3":"after"},
             "units":[{"floor":"st","door":"tv","area_m2":82.125,"rooms":3},{"floor":"2","door":"?"}]},
            {"street":"Public Test Road","number":"7B","point":[-1000.25,500.125],"stair":null,
             "stair_by_level":{"1":"core","3":"before"},"units":[]},
            {"street":"Public Test Road","number":"7C","point":[0,0],"stair":"core","units":[]}
        ]))
        .unwrap();
        s.execute("group.define", json!({"id":"all","name":"All source","targets":["before.L0","stair:core"]})).unwrap();
        s.execute("group.define", json!({"id":"one","name":"One occurrence","targets":["core.L1"]})).unwrap();
        s.execute("group.define", json!({"id":"untouched","name":"Unrelated","targets":["after.L2"]})).unwrap();
        let before = s.doc.clone();
        let mut p = split(&s);
        if !retain_ground {
            p["partitions"][0]["levels"] = json!([2]);
            p["partitions"][1]["levels"] = json!([0, 1]);
        }
        let result = s.execute("stair.refine_source", p.clone()).unwrap();
        let mut expected = before.clone();
        expected.buildings[0].stairs = s.doc.buildings[0].stairs.clone();
        expected.journal = s.doc.journal.clone();
        for (old, new) in before.buildings[0].entrances.iter().zip(&mut expected.buildings[0].entrances) {
            for level in [0, 1, 2] {
                if old.stair_at(level) == Some("core") && p["partitions"][1]["levels"].as_array().unwrap().contains(&json!(level)) {
                    new.stair_by_level.insert(level, "core.upper".into());
                }
            }
        }
        expected.groups[0].targets = vec!["before.L0".into(), "stair:core".into(), "stair:core.upper".into()];
        expected.groups[1].targets = vec!["core.upper.L1".into()];
        assert_eq!(s.doc, expected);
        assert_eq!(s.doc.groups[2], before.groups[2]);
        assert!(!result["references"]["entrances"].as_array().unwrap().is_empty());
        assert!(s.execute("group.list", json!({})).unwrap()["groups"].as_array().unwrap().iter().all(|g| g["valid"] == true));
        s.execute("edit.undo", json!({})).unwrap();
        assert_eq!(s.doc, before);
        s.execute("edit.redo", json!({})).unwrap();
        assert_eq!(s.doc, expected);
    }
}

#[test]
fn stair_refinement_refusals_keep_redo_source_geometry_and_every_session_flag() {
    let mut s = fixture();
    let p = split(&s);
    s.execute("stair.refine_source", p.clone()).unwrap();
    let accepted = s.doc.clone();
    s.execute("edit.undo", json!({})).unwrap();
    s.dirty = false;
    s.path = Some("unchanged.trigon".into());
    let before_glb = glb(&s.doc);
    let cases = [
        ("/building", json!("missing"), "building missing"),
        ("/expected_source/id", json!(""), "named"),
        ("/expected_source/id", json!("unknown"), "does not exist"),
        ("/expected_source/levels", json!([2, 1, 0]), "stale"),
        ("/partitions", json!([]), "1..60 partitions"),
        ("/partitions/0/levels", json!([]), "1..60 departure"),
        ("/partitions/0/levels", json!([0, 0]), "duplicate departure"),
        ("/partitions/1/levels", json!([0, 1, 2]), "overlapping departure"),
        ("/partitions/1/levels", json!([1]), "union"),
        ("/partitions/1/levels", json!([1, 2, 3]), "union"),
        ("/partitions/1/levels", json!([usize::MAX]), "does not exist"),
        ("/partitions/1/id", json!("core"), "duplicate partition"),
        ("/partitions/0/id", json!("no.retained"), "exactly one"),
        ("/partitions/1/id", json!("before"), "already exists"),
        ("/partitions/1/id", json!("core.L0"), "already exists"),
        ("/partitions/1/id", json!("core.L00"), "already exists"),
        ("/partitions/1/id", json!("b.slab.L0"), "ambiguous source/instance"),
        ("/partitions/1/id", json!("a".repeat(257)), "256 bytes"),
        ("/partitions/1/outline", json!([[0, 0], [0, 0], [0, 0], [0, 0]]), "out of range"),
        ("/partitions/1/outline", json!([[0, 0]]), "four corners"),
        ("/partitions/1/outline/3/0", json!(2100), "finite rectangle"),
        ("/partitions/1/landing", json!(-1), "landing"),
        ("/partitions/1/well", json!(1e300), "well"),
        ("/partitions/1/tread", json!(0), "tread"),
    ];
    for (pointer, value, message) in cases {
        let mut bad = p.clone();
        *bad.pointer_mut(pointer).unwrap() = value;
        refuses(&mut s, "stair.refine_source", bad.clone(), message);
        refuses(&mut s, "precision.preview", json!({"action":{"command":"stair.refine_source","params":bad}}), message);
    }
    for (field, value, message) in [
        ("winders", json!(usize::MAX), "winders"),
        ("turn_depths", json!([600, 700]), "turn_depths"),
        ("boundary", json!([[0, 0], [0, 0]]), "boundary"),
        ("turn_chamfer", json!([0, 400]), "chamfer"),
    ] {
        let mut bad = p.clone();
        bad["partitions"][1][field] = value;
        refuses(&mut s, "stair.refine_source", bad, message);
    }
    assert_eq!(glb(&s.doc), before_glb);
    s.execute("edit.redo", json!({})).unwrap();
    assert_eq!(s.doc, accepted);
}

#[test]
fn stair_refinement_duplicate_ambiguous_and_unsupported_consumers_refuse_with_ids() {
    for case in 0..12 {
        let mut s = fixture();
        let p = split(&s);
        let needle = match case {
            0 => {
                let duplicate = s.doc.buildings[0].stairs[1].clone();
                s.doc.buildings[0].stairs.push(duplicate);
                "core"
            }
            1 => {
                s.doc.buildings.push(s.doc.buildings[0].clone());
                "building ID b"
            }
            2 => {
                s.doc.buildings[0].stairs[1].levels.push(0);
                "duplicate departure"
            }
            3 => {
                s.doc.groups.push(serde_json::from_value(json!({"id":"bad","name":"Bad","targets":["core.L3"]})).unwrap());
                "bad"
            }
            4 => {
                s.doc.groups.push(serde_json::from_value(json!({"id":"bad","name":"Bad","targets":["stair:core","core.L1"]})).unwrap());
                "bad"
            }
            5 => {
                s.doc.groups.push(serde_json::from_value(json!({"id":"bad","name":"Bad","targets":["core.upper.L1"]})).unwrap());
                "bad"
            }
            6 => {
                s.doc.buildings[0].entrances = serde_json::from_value(
                    json!([{"street":"Synthetic","number":"1","point":[0,0],"stair":null,"stair_by_level":{"3":"core"},"units":[]}]),
                )
                .unwrap();
                "b.entrance0"
            }
            7 => {
                s.doc.buildings[0].stairs[0].id = "core.L1".into();
                "ambiguous source/instance"
            }
            8 => {
                s.doc.buildings[0].interior_walls =
                    serde_json::from_value(json!([{"id":"core","levels":[0],"a":[8000,6000],"b":[10000,6000],"thickness":120}])).unwrap();
                "ambiguous source/instance"
            }
            9 => {
                s.doc.buildings[0].rooms = serde_json::from_value(
                    json!([{"id":"core.upper","levels":[1],"name":"Public","polygon":[[8000,6000],[10000,6000],[10000,8000],[8000,8000]]}]),
                )
                .unwrap();
                "already exists"
            }
            10 => {
                s.doc.groups.push(serde_json::from_value(json!({"id":"nested","name":"Public","targets":["group:all"]})).unwrap());
                "nested group reference group:all"
            }
            _ => {
                s.doc.groups.push(serde_json::from_value(json!({"id":"bad","name":"Public","targets":["stair:core.L1"]})).unwrap());
                "unsupported stair occurrence reference stair:core.L1"
            }
        };
        let mut p = p;
        if case == 2 {
            p["expected_source"] = json!(s.doc.buildings[0].stairs[1]);
        }
        refuses(&mut s, "stair.refine_source", p, needle);
    }
}

#[test]
fn stair_refinement_shaft_fit_checks_departure_arrival_attic_and_exact_contact() {
    for schedule in [[600., 400., 400., 400.], [200., 600., 400., 400.]] {
        for yaw in [0_f64, 31., -118.] {
            let mut s = fixture();
            s.doc.buildings[0].set_wall_schedule(&schedule).unwrap();
            let (sin, cos) = yaw.to_radians().sin_cos();
            let transform = |[x, y]: [f64; 2]| [140000. + x * cos - y * sin, -230000. + x * sin + y * cos];
            for p in &mut s.doc.buildings[0].footprint {
                *p = transform(*p);
            }
            for stair in &mut s.doc.buildings[0].stairs {
                for p in &mut stair.outline {
                    *p = transform(*p);
                }
            }
            let source = s.doc.buildings[0].stairs[1].clone();
            let mut st: Stair = source.clone();
            st.kind = trigon_model::StairKind::Straight;
            st.outline = [[1000., 500.], [4000., 500.], [4000., 1500.], [1000., 1500.]].into_iter().map(transform).collect();
            let p = json!({"building":"b","expected_source":source,"partitions":[st]});
            refuses(&mut s, "stair.refine_source", p, if schedule[0] == 600. { "outside b.slab.L0" } else { "outside b.slab.L1" });
            st.outline = [[1000., 600.], [4000., 600.], [4000., 1600.], [1000., 1600.]].into_iter().map(transform).collect();
            let result = s.execute("stair.refine_source", json!({"building":"b","expected_source":source,"partitions":[st]})).unwrap();
            assert_eq!(result["validation"]["changed_shaft_floors"], "passed");
            assert_eq!(result["affected_floors"].as_array().unwrap().len(), 4);
        }
    }
    let mut s = fixture();
    s.execute("building.set_roof", json!({"building":"b","kind":"pitched","pitch_deg":35})).unwrap();
    s.execute("building.set_attic", json!({"building":"b","floor":11250,"name":"Public attic"})).unwrap();
    s.doc.buildings[0].stairs[1].levels = vec![3];
    s.doc.buildings[0].set_wall_schedule(&[400., 400., 400., 200., 600.]).unwrap();
    let source = s.doc.buildings[0].stairs[1].clone();
    let mut st = source.clone();
    st.kind = trigon_model::StairKind::Straight;
    st.outline = vec![[1000., 500.], [4000., 500.], [4000., 1500.], [1000., 1500.]];
    refuses(&mut s, "stair.refine_source", json!({"building":"b","expected_source":source,"partitions":[st]}), "outside b.attic");
}

#[test]
fn stair_refinement_near_contact_cannot_hide_a_concave_floor_intrusion() {
    for yaw in [0_f64, 31., -118.] {
        let (sin, cos) = yaw.to_radians().sin_cos();
        let transform = |[x, y]: [f64; 2]| [x * cos - y * sin, x * sin + y * cos];
        // A 100 mm notch sits between almost-collinear floor edges. Contact at
        // one crossing endpoint must not excuse the entire crossing region.
        let footprint = [
            [-1100., -100.00000009999985],
            [1041.4213562665989, -100.0000001],
            [1100.000000424264, -41.42135588375615],
            [1158.5786440818788, -99.99999889700568],
            [9499.99999994, -99.9999995],
            [10500., -100.0000001],
            [11100., -100.0000001],
            [11100., 5100.],
            [-1100., 5100.],
        ]
        .map(transform);
        let mut s = Session::new(Box::new(Canned));
        s.execute(
            "building.add",
            json!({"id":"b","name":"Concave shaft contact",
            "footprint":footprint,"preserve_vertices":true,"storeys":2,
            "storey_height_mm":3000,"wall_thickness_mm":100}),
        )
        .unwrap();
        let outline = [[2000., 1000.], [4500., 1000.], [4500., 2000.], [2000., 2000.]].map(transform);
        s.execute(
            "interior.add_stairs",
            json!({"building":"b","stairs":[{
            "id":"core","levels":[0],"outline":outline,"kind":"straight"}]}),
        )
        .unwrap();
        let original = s.doc.buildings[0].stairs[0].clone();
        let mut valid = original.clone();
        valid.tread = Some(275.);
        s.execute("stair.refine_source", json!({"building":"b","expected_source":original,"partitions":[valid]})).unwrap();
        let after_valid = s.doc.clone();
        s.execute("edit.undo", json!({})).unwrap();
        assert!(!s.redo.is_empty());
        let baseline = glb(&s.doc);
        let mut candidate = original.clone();
        candidate.outline = [[0., 0.], [10000., 0.], [10000., 3000.], [0., 3000.]].into_iter().map(transform).collect();
        let params = json!({"building":"b","expected_source":original,"partitions":[candidate]});
        refuses(&mut s, "stair.refine_source", params.clone(), "site-fixed");
        assert_eq!(glb(&s.doc), baseline);
        refuses(&mut s, "precision.preview", json!({"action":{"command":"stair.refine_source","params":params}}), "site-fixed");
        assert_eq!(glb(&s.doc), baseline);
        s.execute("edit.redo", json!({})).unwrap();
        assert_eq!(s.doc, after_valid);
    }
}

#[test]
fn stair_refinement_fitted_boundaries_and_null_turn_controls_use_native_geometry() {
    let mut s = fixture();
    let source = s.doc.buildings[0].stairs[1].clone();
    let mut next = source.clone();
    next.turn_chamfer = Some([200., 300.]);
    s.execute("stair.refine_source", json!({"building":"b","expected_source":source,"partitions":[next]})).unwrap();
    let source = s.doc.buildings[0].stairs[1].clone();
    let mut next = source.clone();
    next.turn_chamfer = None;
    next.boundary = Some(vec![[2000., 2000.], [5300., 2000.], [5500., 2300.], [5500., 3900.], [5300., 4200.], [2000., 4200.]]);
    s.execute("stair.refine_source", json!({"building":"b","expected_source":source,"partitions":[next]})).unwrap();
    assert_eq!(trigon_model::stair::opening(&s.doc.buildings[0].stairs[1]).unwrap(), next.boundary.clone().unwrap());
    let source = s.doc.buildings[0].stairs[1].clone();
    next.boundary = None;
    next.rounded = true;
    s.execute("stair.refine_source", json!({"building":"b","expected_source":source,"partitions":[next]})).unwrap();
    assert_eq!(s.doc.buildings[0].stairs[1], next);
}

#[test]
fn stair_refinement_reports_preexisting_overlap_and_exposed_adjacent_arrival() {
    let mut s = fixture();
    let mut same = s.doc.buildings[0].stairs[1].clone();
    same.id = "same.shape.distinct.id".into();
    same.levels = vec![3];
    s.doc.buildings[0].stairs[0] = same;
    s.doc.buildings[0].interior_walls = serde_json::from_value(json!([
        {"id":"preexisting","levels":[0,1,2],"a":[4800,1600],"b":[4800,4800],"thickness":120}
    ]))
    .unwrap();
    let p = split(&s);
    let result = s.execute("stair.refine_source", p).unwrap();
    assert_eq!(s.doc.buildings[0].stairs[0].id, "same.shape.distinct.id");
    for when in ["before", "after"] {
        let report = &result["adjacent_feedback"][when];
        assert_eq!(report["status"], "needs_review");
        assert!(
            report["findings"]
                .as_array()
                .unwrap()
                .iter()
                .any(|f| f["kind"] == "stair_wall_overlap" && f["opening_overlap_mm2"].as_f64().unwrap() > 1.)
        );
    }
    let findings = result["adjacent_feedback"]["after"]["findings"].as_array().unwrap();
    assert!(findings.iter().any(|f| f["kind"] == "overlapping_stair_symbols"
        && f["level"] == 1
        && f["ids"].as_array().unwrap().contains(&json!("core.L0"))
        && f["ids"].as_array().unwrap().contains(&json!("core.upper.L1"))));
    let plan = crate::plan_drawing(&s.doc, 1, trigon_model::build::View::latest(&s.doc)).unwrap();
    assert_eq!(plan.pick_element([5450., 3100.], 0.1).unwrap().id, "core.L0");
    assert_eq!(plan.pick_element([3300., 2600.], 0.1).unwrap().id, "core.upper.L1");
    let built = trigon_model::build(&s.doc).unwrap();
    for (id, level) in [("core.L0", 0), ("core.upper.L1", 1), ("core.upper.L2", 2)] {
        let st = built[0].stairs.iter().find(|st| st.id == id).unwrap();
        assert_eq!(st.level, level);
        assert!(!st.mesh.is_empty());
        assert!(!st.railing.is_empty());
        assert_eq!(s.execute("element.get", json!({"id":id})).unwrap()["kind"], "stair");
    }
    assert!(!built[0].stairs.iter().any(|st| st.id == "core.L1"));
    for (arrival, source_id) in [(1, "core"), (2, "core.upper")] {
        let source = s.doc.buildings[0].stairs.iter().find(|st| st.id == source_id).unwrap();
        let opening = trigon_model::stair::opening(source).unwrap();
        let area = trigon_model::doc::polygon_area(&opening);
        let floor = built[0].slabs.iter().find(|floor| floor.id == format!("b.slab.L{arrival}")).unwrap();
        assert!(
            floor.holes.iter().any(|hole| (trigon_model::doc::polygon_area(hole) - area).abs() < 0.001),
            "actual arrival slab opening for {source_id}"
        );
    }
}

#[test]
fn stair_refinement_ifc_readers_keep_semantic_identities_and_exact_bounds() {
    let mut s = fixture();
    let p = split(&s);
    s.execute("stair.refine_source", p).unwrap();
    let built = trigon_model::build(&s.doc).unwrap();
    let shapes = trigon_io::ifc::model_shapes(&s.doc, &built);
    let text = trigon_io::ifc::write(&s.doc, &built, "public-stair.ifc", "2000-01-01T00:00:00Z");
    for read in [trigon_ifcread::read_ifc_lite(&text).unwrap(), trigon_ifcread::read_bimifc(&text).unwrap()] {
        assert!(read.failures.is_empty(), "{:?}", read.failures);
        for root in ["core.L0", "core.upper.L1", "core.upper.L2"] {
            let count = shapes.iter().filter(|(id, _)| id.starts_with(&format!("{root}."))).count();
            assert!(count >= 3);
            for (id, shape) in shapes.iter().filter(|(id, _)| id.starts_with(&format!("{root}."))) {
                let element = read.elements.iter().find(|e| e.global_id == trigon_io::ifc::guid(&format!("trigon:{id}"))).unwrap();
                assert!(
                    trigon_ifcread::bounds_deviation(shape.bounds().unwrap(), element.mesh.bounds().unwrap()) < 0.01,
                    "{} {id}",
                    read.reader.name()
                );
            }
        }
        assert!(!read.elements.iter().any(|e| e.global_id == trigon_io::ifc::guid("trigon:core.L1.flight0")));
    }
}

#[test]
fn stair_refinement_hostile_nested_numbers_and_unknown_fields_never_panic() {
    let mut s = fixture();
    let p = split(&s);
    for pointer in ["/partitions/1/outline/0/0", "/partitions/1/tread", "/partitions/1/landing", "/partitions/1/well"] {
        for value in [-f64::MAX, -1e9, -1., 0., 1e9, f64::MAX] {
            let mut bad = p.clone();
            *bad.pointer_mut(pointer).unwrap() = json!(value);
            let before = (s.doc.clone(), s.undo.clone(), s.redo.clone());
            match s.execute("stair.refine_source", bad) {
                Err(crate::EngineError::Internal(error)) => panic!("panic on {pointer}={value}: {error}"),
                Err(_) => assert_eq!((s.doc.clone(), s.undo.clone(), s.redo.clone()), before),
                Ok(_) => {
                    s.execute("edit.undo", json!({})).unwrap();
                    assert_eq!(s.doc, before.0);
                }
            }
        }
    }
    for field in ["winders", "turn_depths", "turn_chamfer", "boundary"] {
        for value in [json!(false), json!("NaN"), json!([f64::MAX, f64::MAX]), json!({"unexpected":true})] {
            let mut bad = p.clone();
            bad["partitions"][1][field] = value;
            refuses(&mut s, "stair.refine_source", bad, "");
        }
    }
    let mut bad = p.clone();
    bad["unexpected"] = json!(true);
    refuses(&mut s, "stair.refine_source", bad, "unknown field");
    let mut bad = p.clone();
    bad["partitions"][0]["unexpected"] = json!(true);
    refuses(&mut s, "stair.refine_source", bad, "unknown field");
    let mut bad = p.clone();
    bad["partitions"] = json!(vec![p["partitions"][0].clone(); 61]);
    refuses(&mut s, "stair.refine_source", bad, "1..60 partitions");
    let mut bad = p.clone();
    bad["partitions"][0]["levels"] = json!(vec![0; 61]);
    refuses(&mut s, "stair.refine_source", bad, "1..60 departure");
    let schema = crate::commands().iter().find(|c| c.id == "stair.refine_source").unwrap().params_schema();
    assert_eq!(schema["additionalProperties"], false);
    assert_eq!(schema["properties"]["expected_source"]["$ref"], schema["properties"]["partitions"]["items"]["$ref"]);
}

#[test]
fn stair_refinement_full_build_and_annotation_refusals_are_atomic() {
    let mut s = fixture();
    let p = split(&s);
    let mut duplicate = s.doc.buildings[0].stairs[1].clone();
    duplicate.id = "overlapping.source".into();
    s.doc.buildings[0].stairs.push(duplicate);
    refuses(&mut s, "stair.refine_source", p, "overlaps another stair opening");
    let mut s = fixture();
    let p = split(&s);
    s.doc.annotations.push(
        serde_json::from_value(json!({"id":"bad.annotation","view":"missing.view",
        "kind":{"annotation":"text","at":[0,0],"text":"Public fixture"}}))
        .unwrap(),
    );
    refuses(&mut s, "stair.refine_source", p, "missing.view");
}

#[test]
fn stair_refinement_reserves_aliases_new_partition_pairs_and_unsupported_references() {
    let mut s = fixture();
    s.execute("facade.place_at", json!({"facade":"b.f1","level":0,"u_mm":8000,"type":"window.casement.2"})).unwrap();
    let mut p = split(&s);
    p["partitions"][1]["id"] = json!("b.f1.L00.B00.arbitrary");
    refuses(&mut s, "stair.refine_source", p, "ambiguous facade instance");
    let mut p = split(&s);
    p["partitions"] = json!([
        dict_stair(&s.doc.buildings[0].stairs[1], "core", vec![0]),
        dict_stair(&s.doc.buildings[0].stairs[1], "new", vec![1]),
        dict_stair(&s.doc.buildings[0].stairs[1], "new.L1", vec![2])
    ]);
    refuses(&mut s, "stair.refine_source", p, "ambiguous source/instance");
    let p = split(&s);
    s.doc.groups.push(
        serde_json::from_value(json!({"id":"oversize","name":"Public","targets":
        std::iter::once("stair:core").chain(std::iter::repeat_n("before.L0",255)).collect::<Vec<_>>()}))
        .unwrap(),
    );
    refuses(&mut s, "stair.refine_source", p.clone(), "oversize");
    s.doc.groups.clear();
    let mut other = s.doc.buildings[0].clone();
    other.id = "other".into();
    other.stairs.clear();
    other.facades.clear();
    other.entrances = serde_json::from_value(json!([{"street":"Public","number":"1","point":[0,0],"stair":"core","units":[]}])).unwrap();
    s.doc.buildings.push(other);
    refuses(&mut s, "stair.refine_source", p, "other.entrance0");
}

fn dict_stair(stair: &Stair, id: &str, levels: Vec<usize>) -> Stair {
    let mut result = stair.clone();
    result.id = id.into();
    result.levels = levels;
    result
}

#[test]
fn stair_refinement_legacy_attic_is_an_arrival_only_from_the_top_departure() {
    let mut s = fixture();
    s.execute("building.set_roof", json!({"building":"b","kind":"pitched","pitch_deg":35})).unwrap();
    let p = split(&s);
    let result = s.execute("stair.refine_source", p).unwrap();
    assert!(
        !result["affected_floors"].as_array().unwrap().iter().any(|f| f["floor"] == "b.attic"),
        "arrival at storey 3 is not arrival at the attic above storey 3"
    );
    s.doc.buildings[0].stairs[1].levels = vec![3];
    let source = s.doc.buildings[0].stairs[1].clone();
    let mut next = source.clone();
    next.landing = Some(925.125);
    let result = s.execute("stair.refine_source", json!({"building":"b","expected_source":source,"partitions":[next]})).unwrap();
    let floors = result["affected_floors"].as_array().unwrap();
    let attic = floors.iter().find(|f| f["floor"] == "b.attic").unwrap();
    assert_eq!(attic["departure"], false);
    assert_eq!(attic["arrival"], true);
    assert_eq!(result["arrival_levels"], json!([]), "do not invent a stored attic storey");
    assert_eq!(result["affected_levels"], json!([3]));
}
