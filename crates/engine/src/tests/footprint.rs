//! Public synthetic footprint edits: source identity, actual hosted geometry and exchange.
use super::{Canned, Session};
use serde_json::{Value, json};
use trigon_model::{Document, FacadeFrame};

const RING: [[f64; 2]; 4] = [[0., 0.], [24000., 500.], [25000., 12500.], [1000., 12000.]];
fn fixture(ring: &[[f64; 2]]) -> Session {
    let mut s = Session::new(Box::new(Canned));
    s.execute(
        "building.add",
        json!({"id":"b","name":"Public footprint fixture","footprint":ring,
        "storeys":2,"storey_height_mm":3400,"evidence":[{"source":"synthetic:survey","supplies":"footprint","attribution":"Public fixture"}]}),
    )
    .unwrap();
    for (level, u) in [(0, 16000.125), (1, 16000.125), (0, 7000.625)] {
        s.execute("facade.place_at", json!({"facade":"b.f1","level":level,"u_mm":u,"type":"window.casement.2"})).unwrap();
    }
    s
}
fn request(s: &Session, policy: &str, moves: &[(usize, [f64; 2])]) -> Value {
    json!({"building":"b","expected_footprint":s.doc.buildings[0].footprint,
        "moves":moves.iter().map(|(corner,point)|json!({"corner":corner,"point_mm":point})).collect::<Vec<_>>(),"anchor_policy":policy})
}
fn glb(doc: &Document) -> Vec<u8> {
    trigon_io::glb::write(&trigon_model::build(doc).unwrap(), &[])
}
fn frame(doc: &Document, edge: usize) -> FacadeFrame {
    let (a, b) = doc.buildings[0].edge(edge).unwrap();
    FacadeFrame::new(a, b).unwrap()
}
fn near(a: f64, b: f64) {
    assert!((a - b).abs() < 1e-7, "{a:.12} != {b:.12}");
}

#[test]
fn footprint_moves_project_actual_hosted_centres_in_transformed_skew_frames() {
    for yaw in [0_f64, 31., -118.] {
        let (sin, cos) = yaw.to_radians().sin_cos();
        let transform = |[x, y]: [f64; 2]| [142000. + x * cos - y * sin, -270000. + x * sin + y * cos];
        let ring: Vec<_> = RING.iter().copied().map(transform).collect();
        for multiple in [false, true] {
            for policy in ["keep_facade_u", "project_wall_line_centres"] {
                let mut s = fixture(&ring);
                let before = s.doc.clone();
                let built_before = trigon_model::build(&before).unwrap();
                let mut moves = vec![(0, transform([-80., -120.]))];
                if multiple {
                    moves.push((1, transform([24120., 350.])));
                }
                let response = s.execute("building.set_footprint", request(&s, policy, &moves)).unwrap();
                assert_eq!(response["validation"]["model_build"], "passed");
                assert_eq!(response["corners"].as_array().unwrap().len(), moves.len());
                let old = frame(&before, 0);
                let new = frame(&s.doc, 0);
                let built = trigon_model::build(&s.doc).unwrap();
                for (a, b) in built_before[0].walls[0].openings.iter().zip(&built[0].walls[0].openings) {
                    assert_eq!((&a.id, &a.fill.id, a.level), (&b.id, &b.fill.id, b.level));
                    let u = (a.rect.u0 + a.rect.u1) / 2.;
                    let v = (b.rect.u0 + b.rect.u1) / 2.;
                    let p = old.point(u, 0., 0.);
                    let q = new.point(v, 0., 0.);
                    if policy == "keep_facade_u" {
                        near(u, v);
                    } else {
                        near((p[0] - q[0]) * new.t[0] + (p[1] - q[1]) * new.t[1], 0.);
                    }
                    assert!((p[0] - q[0]).hypot(p[1] - q[1]) > 1., "host movement must not be described as fixed world position");
                    near(a.rect.u1 - a.rect.u0, b.rect.u1 - b.rect.u0);
                    assert_eq!((a.rect.v0, a.rect.v1), (b.rect.v0, b.rect.v1));
                    assert_ne!(a.fill.parts, b.fill.parts, "actual opening geometry must follow the host");
                }
                let mut expected = before.clone();
                expected.buildings[0].footprint = s.doc.buildings[0].footprint.clone();
                if policy == "project_wall_line_centres" {
                    for (i, f) in expected.buildings[0].facades.iter_mut().enumerate() {
                        f.bays = s.doc.buildings[0].facades[i].bays.clone();
                        for (p, q) in f.placements.iter_mut().zip(&s.doc.buildings[0].facades[i].placements) {
                            p.u = q.u;
                        }
                    }
                }
                expected.journal = s.doc.journal.clone();
                assert_eq!(s.doc, expected, "all non-anchor source stays exact");
                assert!(s.doc.buildings[0].facades[0].bays[0] > s.doc.buildings[0].facades[0].bays[1], "never sort bays");
                assert_eq!(
                    s.doc.buildings[0].facades[0].placements.iter().map(|p| (p.level, p.bay)).collect::<Vec<_>>(),
                    vec![(0, 0), (1, 0), (0, 1)]
                );
            }
        }
    }
}

#[test]
fn footprint_preview_apply_undo_redo_reopen_replay_and_exchange_are_exact() {
    let mut s = fixture(&RING);
    let before = s.doc.clone();
    let before_glb = glb(&before);
    let params = request(&s, "project_wall_line_centres", &[(0, [-80., -120.]), (1, [23920., 380.])]);
    let action = json!({"command":"building.set_footprint","params":params});
    let preview = s.execute("precision.preview", json!({"action":action})).unwrap();
    assert_eq!(s.doc, before);
    assert!(!preview["result"]["anchors"].as_array().unwrap().is_empty());
    s.execute("precision.apply", json!({"action":action,"preview_token":preview["preview_token"]})).unwrap();
    let after = s.doc.clone();
    let after_glb = glb(&after);
    assert_eq!(after.journal.len(), before.journal.len() + 1);
    assert_eq!(after.journal.last().unwrap().command, "building.set_footprint");
    s.execute("edit.undo", json!({})).unwrap();
    assert_eq!(s.doc, before);
    assert_eq!(glb(&s.doc), before_glb);
    s.execute("edit.redo", json!({})).unwrap();
    assert_eq!(s.doc, after);
    assert_eq!(glb(&s.doc), after_glb);
    let mut replay = Session::new(Box::new(Canned));
    for e in &after.journal {
        replay.execute(&e.command, e.params.clone()).unwrap();
    }
    assert_eq!(replay.doc, after);
    assert_eq!(glb(&replay.doc), after_glb);
    let path = std::env::temp_dir().join(format!("trigon-footprint-{}.trigon", std::process::id()));
    s.execute("document.save", json!({"path":path})).unwrap();
    let bytes = std::fs::read(&path).unwrap();
    s.execute("document.open", json!({"path":path})).unwrap();
    s.execute("document.save", json!({"path":path})).unwrap();
    assert_eq!(std::fs::read(&path).unwrap(), bytes);
    std::fs::remove_file(path).unwrap();
    assert_eq!(s.doc, after);
    assert_eq!(glb(&s.doc), after_glb);
    for doc in [&before, &after] {
        let model = trigon_model::build(doc).unwrap();
        let shapes = trigon_io::ifc::model_shapes(doc, &model);
        let text = trigon_io::ifc::write(doc, &model, "public.ifc", "2000-01-01T00:00:00Z");
        for read in [trigon_ifcread::read_ifc_lite(&text).unwrap(), trigon_ifcread::read_bimifc(&text).unwrap()] {
            assert!(read.failures.is_empty(), "{:?}", read.failures);
            for id in ["b.f1.L0.B0", "b.f1.L1.B0", "b.f1.L0.B1"] {
                let element = read.elements.iter().find(|e| e.global_id == trigon_io::ifc::guid(&format!("trigon:{id}"))).unwrap();
                assert_eq!(element.class, "IFCWINDOW");
                let expected = shapes.iter().find(|(key, _)| key == id).unwrap().1.bounds().unwrap();
                assert!(trigon_ifcread::bounds_deviation(expected, element.mesh.bounds().unwrap()) < 0.01, "reader {} {id}", read.reader.name());
            }
        }
    }
}

fn refuses(s: &mut Session, command: &str, p: Value, message: &str) {
    let state = (s.doc.clone(), s.undo.clone(), s.redo.clone(), s.path.clone(), s.dirty);
    let error = s.execute(command, p).unwrap_err();
    assert!(!matches!(error, crate::EngineError::Internal(_)), "{error}");
    assert!(error.to_string().contains(message), "expected {message}: {error}");
    assert_eq!((s.doc.clone(), s.undo.clone(), s.redo.clone(), s.path.clone(), s.dirty), state);
}

#[test]
fn footprint_hostile_stale_topology_and_missing_policy_refusals_keep_redo() {
    let mut s = fixture(&RING);
    let good = request(&s, "keep_facade_u", &[(0, [-80., -120.])]);
    s.execute("building.set_footprint", good.clone()).unwrap();
    let accepted = s.doc.clone();
    s.execute("edit.undo", json!({})).unwrap();
    s.dirty = false;
    s.path = Some("unchanged.trigon".into());
    let mut cases = vec![
        (json!({"moves":[]}), "corner moves"),
        (json!({"moves":[{"corner":usize::MAX,"point_mm":[0,0]}]}), "corner"),
        (json!({"moves":[{"corner":0,"point_mm":[1e300,0]}]}), "coordinate mm"),
        (json!({"moves":[{"corner":0,"point_mm":[null,0]}]}), "bad parameters"),
        (json!({"moves":[{"corner":0,"point_mm":[0,0]}]}), "displacement mm"),
        (json!({"moves":[{"corner":0,"point_mm":[-80,-120]},{"corner":0,"point_mm":[-100,-100]}]}), "duplicate corner"),
        (json!({"footprint":RING}), "unknown field"),
        (json!({"anchor_policy":"keep_world_position"}), "unknown variant"),
        (json!({"building":"missing"}), "unknown building"),
    ];
    let mut stale = RING;
    stale[0][0] = 0.25;
    cases.push((json!({"expected_footprint":stale}), "stale"));
    cases.push((json!({"expected_footprint":&RING[..3]}), "stale"));
    for shift in [1, 3] {
        let moves: Vec<_> = (0..4).map(|i| json!({"corner":i,"point_mm":RING[(i+shift)%4]})).collect();
        cases.push((json!({"moves":moves}), "registration bound"));
    }
    for (patch, message) in cases {
        let mut p = good.clone();
        p.as_object_mut().unwrap().extend(patch.as_object().unwrap().clone());
        refuses(&mut s, "building.set_footprint", p.clone(), message);
        refuses(&mut s, "precision.preview", json!({"action":{"command":"building.set_footprint","params":p}}), message);
    }
    let mut p = good.clone();
    p.as_object_mut().unwrap().remove("anchor_policy");
    refuses(&mut s, "building.set_footprint", p, "missing field");
    for value in [f64::NAN, f64::INFINITY, f64::NEG_INFINITY] {
        let before = s.doc.clone();
        let p = crate::footprint_edit::SetFootprint {
            building: "b".into(),
            expected_footprint: RING.to_vec(),
            moves: vec![crate::footprint_edit::CornerMove { corner: 0, point_mm: [value, 0.] }],
            anchor_policy: crate::footprint_edit::AnchorPolicy::KeepFacadeU,
        };
        assert!(crate::footprint_edit::set(&mut s, p).is_err());
        assert_eq!(s.doc, before);
    }
    s.execute("edit.redo", json!({})).unwrap();
    assert_eq!(s.doc, accepted);
    refuses(&mut s, "building.set_footprint", good, "stale");
}

#[test]
fn footprint_self_crossings_and_derived_roof_pruning_are_refused() {
    let mut s = Session::new(Box::new(Canned));
    s.execute("building.add",json!({"id":"b","name":"Public notch","footprint":[[0,0],[12000,0],[12000,10000],[6000,1000],[0,10000]],"storeys":1,"storey_height_mm":3000,"wall_thickness_mm":100})).unwrap();
    let p = request(&s, "keep_facade_u", &[(3, [6000., -100.])]);
    refuses(&mut s, "building.set_footprint", p, "cross or touch");
    let mut s = Session::new(Box::new(Canned));
    s.execute("building.add",json!({"id":"b","name":"Public near-collinear","footprint":[[0,0],[12000,0],[12000,8000],[6000,8100],[0,8000]],"storeys":1,"storey_height_mm":3000})).unwrap();
    s.execute("building.set_roof", json!({"kind":"pitched","pitch_deg":40,"straighten_mm":0})).unwrap();
    let p = request(&s, "keep_facade_u", &[(3, [6000., 8040.])]);
    refuses(&mut s, "building.set_footprint", p, "50 mm collinearity");
    // In a flat roof there is no implicit eave pruning: a measured 100 mm corner stays a corner.
    s.execute("building.set_roof", json!({"kind":"flat"})).unwrap();
    let p = request(&s, "keep_facade_u", &[(3, [6000., 8040.])]);
    s.execute("building.set_footprint", p).unwrap();
    assert_eq!(s.doc.buildings[0].footprint.len(), 5);
}

#[test]
fn footprint_preserves_shared_bays_phases_groups_and_unrelated_sources() {
    let mut s = fixture(&RING);
    // One shared grid pattern with no instance overrides, alongside exact overrides.
    s.doc.buildings[0].facades[0].placements[0].u = None;
    s.doc.buildings[0].facades[0].placements[1].u = None;
    s.execute("group.define", json!({"id":"windows","name":"Existing selection","targets":["placement:b.f1:0","placement:b.f1:1"]})).unwrap();
    s.execute(
        "building.add",
        json!({"id":"other","name":"Unrelated","footprint":[[50000,0],[58000,0],[58000,6000],[50000,6000]],"storeys":1,"storey_height_mm":3000}),
    )
    .unwrap();
    s.doc.drawings.push(trigon_model::Drawing {
        name: "Public underlay".into(),
        path: "unread-public-plan.pdf".into(),
        page: 1,
        rotate: 0,
        width: 1000,
        height: 1000,
        note: "Synthetic".into(),
        plans: vec![trigon_model::PlanRegistration { levels: vec![0, 1], a: 10., b: 0., tx: 0., ty: 0. }],
        facades: vec![trigon_model::FacadeRegistration { facade: "b.f1".into(), x_start: 0., x_end: 900., y_levels: [[900., 0.], [300., 6000.]] }],
    });
    let before = s.doc.clone();
    let other = trigon_model::build(&before).unwrap()[1].clone();
    let p = request(&s, "project_wall_line_centres", &[(0, [-80., -120.])]);
    let result = s.execute("building.set_footprint", p).unwrap();
    assert_eq!(s.doc.drawings, before.drawings);
    assert_eq!(s.doc.groups, before.groups);
    assert_eq!(s.doc.buildings[1], before.buildings[1]);
    assert_eq!(trigon_model::build(&s.doc).unwrap()[1], other);
    assert_eq!(s.doc.buildings[0].facades[0].placements[..2], before.buildings[0].facades[0].placements[..2]);
    assert_eq!(result["registration_feedback"][0]["status"], "needs_source_review");
    assert_ne!(result["registration_feedback"][0]["before_px_per_mm"], result["registration_feedback"][0]["after_px_per_mm"]);
    assert!(s.execute("group.list", json!({})).unwrap()["groups"][0]["valid"].as_bool().unwrap());
}

fn rectangle() -> Session {
    fixture(&[[0., 0.], [24000., 0.], [24000., 12000.], [0., 12000.]])
}
fn detailed_roof(s: &mut Session, kind: &str) {
    let mut p = json!({"building":"b","kind":kind,"pitch_deg":46.,"thickness_mm":220.,"overhang_mm":300.,"gutters":true,
        "dormers":[{"id":"d","facade":"b.f1","u_mm":6000.,"window_type":"window.casement.2","width_mm":1000.,"height_mm":1000.}],
        "rooflights":[{"id":"r","facade":"b.f1","u_mm":12000.,"run_mm":500.,"window_type":"window.casement.2","width_mm":800.,"height_mm":1000.,"stand_off_mm":100.,"flashing_mm":100.}]});
    if kind == "mansard" {
        p.as_object_mut().unwrap().remove("pitch_deg");
    } else {
        p["drainage"] = json!({"downpipes":[{"id":"p","facade":"b.f1","along_mm":20000.,"diameter_mm":100.,"wall_offset_mm":150.,"bottom_mm":200.,"shoe_mm":100.}]});
    }
    if kind == "half_hipped" {
        p["gables"] = json!(["b.f2", "b.f4"]);
        p["half_hips"] = json!([{"facade":"b.f2","drop_mm":400.}]);
        p["fascia_depth_mm"] = json!(20.);
    }
    s.execute("building.set_roof", p).unwrap();
    if kind == "pitched" {
        s.execute("element.set", json!({"id":"b.roof","properties":{"eave_facades":["b.f1","b.f2","b.f3","b.f4"]}})).unwrap();
    }
}

#[test]
fn footprint_rehosts_roof_windows_dormers_downpipes_trims_and_slabs_without_losing_ids() {
    for kind in ["pitched", "half_hipped", "mansard"] {
        for policy in ["keep_facade_u", "project_wall_line_centres"] {
            let mut s = rectangle();
            detailed_roof(&mut s, kind);
            s.execute("building.set_plinth", json!({"height_mm":600.,"projection_mm":50.})).unwrap();
            s.execute("facade.set_bands", json!({"facades":["b.f1"],"bands":[{"levels":[0,1],"bottom":2500.,"top":2600.,"projection":30.}]}))
                .unwrap();
            let before = s.doc.clone();
            let built_before = trigon_model::build(&before).unwrap();
            let moves: Vec<_> = before.buildings[0].footprint.iter().enumerate().map(|(i, p)| (i, [p[0] - 80., p[1] - 120.])).collect();
            let result = s.execute("building.set_footprint", request(&s, policy, &moves)).unwrap();
            let built = trigon_model::build(&s.doc).unwrap();
            let roof = built[0].roof.as_ref().unwrap();
            let old = built_before[0].roof.as_ref().unwrap();
            let du = if policy == "keep_facade_u" { 0. } else { 80. };
            near(s.doc.buildings[0].roof.dormers()[0].u_mm, 6000. + du);
            near(s.doc.buildings[0].roof.rooflights()[0].u_mm, 12000. + du);
            let dormer = &roof.dormers[0];
            let old_dormer = &old.dormers[0];
            let a = dormer.front.frame.point(dormer.front.frame.length / 2., 0., 0.);
            let b = old_dormer.front.frame.point(old_dormer.front.frame.length / 2., 0., 0.);
            near(a[0] - b[0], -80. + du);
            near(a[1] - b[1], -120.);
            near(roof.rooflights[0].frame.origin[0] - old.rooflights[0].frame.origin[0], -80. + du);
            near(roof.rooflights[0].frame.origin[1] - old.rooflights[0].frame.origin[1], -120.);
            assert_eq!(roof.dormers[0].id, old.dormers[0].id);
            assert_eq!(roof.rooflights[0].id, old.rooflights[0].id);
            assert_eq!(roof.drainage.iter().map(|d| &d.id).collect::<Vec<_>>(), old.drainage.iter().map(|d| &d.id).collect::<Vec<_>>());
            if kind != "mansard" {
                let a = roof.drainage.iter().find(|d| d.id == "b.roof.downpipe.p").unwrap().mesh.bounds().unwrap();
                let b = old.drainage.iter().find(|d| d.id == "b.roof.downpipe.p").unwrap().mesh.bounds().unwrap();
                near(a.0[0] - b.0[0], -80. + du);
                near(a.0[1] - b.0[1], -120.);
                assert!(result["anchors"].as_array().unwrap().iter().any(|a| a["id"] == "b.roof.downpipe.p"));
            }
            for (a, b) in built[0].slabs.iter().zip(&built_before[0].slabs) {
                assert_eq!((&a.id, a.level, a.thickness), (&b.id, b.level, b.thickness));
                for (p, q) in a.polygon.iter().zip(&b.polygon) {
                    near(p[0] - q[0], -80.);
                    near(p[1] - q[1], -120.);
                }
            }
            assert_eq!(s.doc.buildings[0].facades[0].bands, before.buildings[0].facades[0].bands);
            assert_ne!(built[0].walls[0].trim, built_before[0].walls[0].trim);
            assert_ne!(built[0].walls[0].plinth, built_before[0].walls[0].plinth);
        }
    }
}

#[test]
fn footprint_surfaces_downstream_roof_failures_and_guards_autojoined_interiors() {
    let mut s = rectangle();
    detailed_roof(&mut s, "pitched");
    let p = request(&s, "keep_facade_u", &[(1, [18500., 0.])]);
    refuses(&mut s, "building.set_footprint", p, "b.roof.downpipe.p");
    let mut s = rectangle();
    s.execute("building.set_roof",json!({"kind":"pitched","pitch_deg":46.,"thickness_mm":220.,
        "rooflights":[{"id":"hip-window","facade":"b.f1","u_mm":17500.,"run_mm":2200.,"window_type":"window.casement.2","width_mm":800.,"height_mm":1000.,"stand_off_mm":100.,"flashing_mm":100.}]})).unwrap();
    let p = request(&s, "keep_facade_u", &[(1, [20000., 0.]), (2, [20000., 12000.])]);
    refuses(&mut s, "building.set_footprint", p.clone(), "rooflight hip-window");
    refuses(&mut s, "precision.preview", json!({"action":{"command":"building.set_footprint","params":p}}), "run=2200");
    let mut s = rectangle();
    s.execute(
        "interior.add_walls",
        json!({"walls":[{"id":"joined","levels":[0],"a":[5000.,400.],"b":[5000.,6000.],"thickness":150.,"doors":[{"at":2500.,"width":900.}]}]}),
    )
    .unwrap();
    let before = trigon_model::build(&s.doc).unwrap();
    assert_eq!(before[0].interior[0].doors.len(), 1);
    let p = request(&s, "keep_facade_u", &[(0, [0., -120.]), (1, [24000., -120.])]);
    refuses(&mut s, "building.set_footprint", p.clone(), "joined.L0 automatic interior");
    refuses(&mut s, "precision.preview", json!({"action":{"command":"building.set_footprint","params":p}}), "displacement");
    assert_eq!(trigon_model::build(&s.doc).unwrap(), before);
}

#[test]
fn footprint_refuses_excluded_rooms_and_rotated_projected_cellar_stairs() {
    let mut s = rectangle();
    s.execute(
        "interior.add_rooms",
        json!({"rooms":[{"id":"fixed-room","levels":[0],"name":"Synthetic room","polygon":[[1000.,500.],[6000.,500.],[6000.,5000.],[1000.,5000.]]}]}),
    )
    .unwrap();
    let p = request(&s, "keep_facade_u", &[(0, [0., 1000.]), (1, [24000., 1000.])]);
    refuses(&mut s, "building.set_footprint", p, "fixed-room site-fixed point 0");
    let mut s = rectangle();
    s.execute("building.set_cellar_stairs",json!({"stairs":[{"id":"cellar","facade":"b.f1","from":2000.,"to":6000.,"width":1000.,"top":2200.,"bottom":5000.,"floor":-2000.,"door_at":5400.,"door_width":900.}]})).unwrap();
    let p = request(&s, "project_wall_line_centres", &[(0, [-80., -120.])]);
    refuses(&mut s, "building.set_footprint", p, "cellar on b.f1: projected multi-station");
    let p = request(&s, "project_wall_line_centres", &[(0, [-80., -120.]), (1, [23920., -120.])]);
    s.execute("building.set_footprint", p).unwrap();
    near(s.doc.buildings[0].cellar_stairs[0].from, 2080.);
    near(s.doc.buildings[0].cellar_stairs[0].to, 6080.);
}

#[test]
fn footprint_basement_mapping_and_ambiguous_facade_sources_fail_atomically() {
    let mut s = Session::new(Box::new(Canned));
    s.execute("building.add",json!({"id":"b","name":"Public basement","footprint":[[0,0],[12000,0],[12000,8000],[6000,8100],[0,8000]],"storeys":2,"storey_height_mm":3000})).unwrap();
    s.execute("building.add_basement", json!({"floor":-2500.})).unwrap();
    for facade in ["b.f3", "b.f4"] {
        s.execute("facade.set_kind", json!({"facade":facade,"kind":"street"})).unwrap();
    }
    s.execute("building.set_basement_line", json!({"straighten_mm":80.})).unwrap();
    let p = request(&s, "keep_facade_u", &[(3, [6000., 8040.])]);
    refuses(&mut s, "building.set_footprint", p, "basement straightening at 80 mm");
    let mut s = rectangle();
    s.doc.buildings[0].facades[1].edge = 0;
    let p = request(&s, "keep_facade_u", &[(0, [-80., -120.])]);
    refuses(&mut s, "building.set_footprint", p, "unique facade ID and source edge");
}

#[test]
fn footprint_historical_placements_and_stale_preview_keep_source_history() {
    let mut s = rectangle();
    s.execute("phase.set", json!({"phases":[{"id":"existing","name":"Existing"},{"id":"future","name":"Future"}]})).unwrap();
    s.execute("facade.place_at", json!({"facade":"b.f1","level":0,"u_mm":16000.125,"type":"window.casement.1","phase":"future","replace":true}))
        .unwrap();
    let before = s.doc.clone();
    assert!(before.buildings[0].facades[0].placements.iter().any(|p| p.demolished.is_some()));
    let action = json!({"command":"building.set_footprint","params":request(&s,"project_wall_line_centres",&[(0,[-80.,-120.]),(1,[23920.,-120.])])});
    let preview = s.execute("precision.preview", json!({"action":action})).unwrap();
    s.execute("precision.apply", json!({"action":action,"preview_token":preview["preview_token"]})).unwrap();
    let after = s.doc.clone();
    for (a, b) in before.buildings[0].facades[0].placements.iter().zip(&after.buildings[0].facades[0].placements) {
        assert_eq!((&a.type_id, a.level, a.bay, &a.created, &a.demolished), (&b.type_id, b.level, b.bay, &b.created, &b.demolished));
        near(b.u.unwrap() - a.u.unwrap(), 80.);
    }
    refuses(&mut s, "precision.apply", json!({"action":action,"preview_token":preview["preview_token"]}), "stale");
    s.execute("edit.undo", json!({})).unwrap();
    assert_eq!(s.doc, before);
    let mut altered = action.clone();
    altered["params"]["anchor_policy"] = json!("keep_facade_u");
    refuses(&mut s, "precision.apply", json!({"action":altered,"preview_token":preview["preview_token"]}), "stale");
    s.execute("edit.redo", json!({})).unwrap();
    assert_eq!(s.doc, after);
    let mut replay = Session::new(Box::new(Canned));
    for e in &after.journal {
        replay.execute(&e.command, e.params.clone()).unwrap();
    }
    assert_eq!(replay.doc, after);
}

#[test]
fn footprint_room_must_fit_actual_floor_not_just_outer_wall_line() {
    let mut s = rectangle();
    s.execute(
        "interior.add_rooms",
        json!({"rooms":[{"id":"floor-room","levels":[0],"name":"Room","polygon":[[1000.,450.],[6000.,450.],[6000.,5000.],[1000.,5000.]]}]}),
    )
    .unwrap();
    let p = request(&s, "keep_facade_u", &[(0, [0., 120.]), (1, [24000., 120.])]);
    // Original inner face y=400; candidate y=520. The room remains inside the outer
    // line y=120 but loses 70 mm of actual floor, so retaining its source would be misleading.
    refuses(&mut s, "building.set_footprint", p.clone(), "70.000000000 mm outside b.slab.L0");
    refuses(&mut s, "precision.preview", json!({"action":{"command":"building.set_footprint","params":p}}), "floor-room");
    // Outward motion increases the floor without moving/redefining the room.
    let room = s.doc.buildings[0].rooms.clone();
    let p = request(&s, "keep_facade_u", &[(0, [0., -120.]), (1, [24000., -120.])]);
    s.execute("building.set_footprint", p).unwrap();
    assert_eq!(s.doc.buildings[0].rooms, room);
}

#[test]
fn footprint_room_edge_cannot_leave_floor_through_concave_vertices() {
    let mut s = fixture(&[
        [0., 0.],
        [24000., 0.],
        [24000., 16000.],
        [18000., 16000.],
        [18000., 6000.],
        [16000., 5900.],
        [14000., 6000.],
        [14000., 16000.],
        [0., 16000.],
    ]);
    let built = trigon_model::build(&s.doc).unwrap();
    let floor = built[0].slabs.iter().find(|slab| slab.kind == trigon_model::SlabKind::Floor && slab.level == 0).unwrap();
    // The model offsets this shallow V-shaped notch. Its shoulder vertices have the
    // same y; an edge through both is outside the floor in between, even though no
    // boundary segment is crossed in its interior and every room vertex is inside.
    let right = floor.polygon.iter().find(|p| p[0] > 18000. && p[0] < 19000. && p[1] < 6000.).unwrap();
    let left = floor.polygon.iter().find(|p| p[0] > 13000. && p[0] < 14000. && p[1] < 6000.).unwrap();
    assert!(right[1] > 5000. && right[1] < 5900.);
    s.execute(
        "interior.add_rooms",
        json!({"rooms":[{"id":"notch-room","levels":[0],"name":"Corner containment",
        "polygon":[left,[16000.,2000.],right]}]}),
    )
    .unwrap();
    let p = request(&s, "keep_facade_u", &[(0, [0., -120.]), (1, [24000., -120.])]);
    refuses(&mut s, "building.set_footprint", p.clone(), "notch-room");
    refuses(&mut s, "precision.preview", json!({"action":{"command":"building.set_footprint","params":p}}), "outside b.slab.L0");
}
