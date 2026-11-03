//! Requested height is measured on the generated outer roof cover, in site millimetres.
use super::{Canned, Session};
use serde_json::{Value, json};

const RECTANGLE: [[f64; 2]; 4] = [[0., 0.], [8000., 0.], [8000., 6000.], [0., 6000.]];
const HEIGHT_TOLERANCE_MM: f64 = 1e-6;

fn fixture(outline: &[[f64; 2]]) -> Session {
    let mut s = Session::new(Box::new(Canned));
    s.execute(
        "building.add",
        json!({"id":"b","name":"Public roof height fixture","footprint":outline,
            "storeys":1,"storey_height_mm":3000,"wall_thickness_mm":400}),
    )
    .unwrap();
    s
}

fn request(thickness: f64) -> Value {
    json!({"building":"b","kind":"pitched","ridge":6000,"gables":["b.f2","b.f4"],
        "straighten_mm":0,"overhang_mm":400,"thickness_mm":thickness})
}

fn actual_height(s: &Session) -> f64 {
    let built = trigon_model::build(&s.doc).unwrap();
    built[0].roof.as_ref().unwrap().cover.positions.iter().map(|p| p[2]).fold(f64::NEG_INFINITY, f64::max)
}

#[test]
fn requested_roof_height_includes_normal_buildup() {
    let mut s = fixture(&RECTANGLE);
    s.execute("building.set_roof", request(200.)).unwrap();
    let actual = actual_height(&s);
    assert!((actual - 6000.).abs() <= HEIGHT_TOLERANCE_MM, "requested 6000 mm, actual outer cover maximum {actual:.12} mm");
}

#[test]
fn requested_roof_height_zero_thickness_control() {
    let mut s = fixture(&RECTANGLE);
    s.execute("building.set_roof", request(0.)).unwrap();
    assert!((actual_height(&s) - 6000.).abs() <= HEIGHT_TOLERANCE_MM);
}

fn glb_height(bytes: &[u8]) -> f64 {
    let json_len = u32::from_le_bytes(bytes[12..16].try_into().unwrap()) as usize;
    let glb: Value = serde_json::from_slice(&bytes[20..20 + json_len]).unwrap();
    let bin = &bytes[28 + json_len..];
    let node = glb["nodes"].as_array().unwrap().iter().find(|n| n["name"] == "b.roof").unwrap();
    let mesh = &glb["meshes"][node["mesh"].as_u64().unwrap() as usize];
    mesh["primitives"]
        .as_array()
        .unwrap()
        .iter()
        .flat_map(|primitive| {
            let accessor = &glb["accessors"][primitive["attributes"]["POSITION"].as_u64().unwrap() as usize];
            assert_eq!(accessor["componentType"], 5126);
            let view = &glb["bufferViews"][accessor["bufferView"].as_u64().unwrap() as usize];
            let start = view["byteOffset"].as_u64().unwrap_or(0) as usize + accessor["byteOffset"].as_u64().unwrap_or(0) as usize;
            let stride = view["byteStride"].as_u64().unwrap_or(12) as usize;
            (0..accessor["count"].as_u64().unwrap() as usize).map(move |i| {
                let y = start + i * stride + 4;
                f64::from(f32::from_le_bytes(bin[y..y + 4].try_into().unwrap())) * 1000.
            })
        })
        .fold(f64::NEG_INFINITY, f64::max)
}

#[test]
fn requested_roof_height_tracks_gables_verges_caps_and_transforms() {
    for ring in [
        RECTANGLE.to_vec(),
        vec![[0., 0.], [30000., 0.], [32000., 12010.], [-600., 12000.]],
        vec![[0., 0.], [12000., 600.], [11500., 8600.], [-500., 8000.]],
    ] {
        for yaw in [0_f64, 31., -118.] {
            let (sin, cos) = yaw.to_radians().sin_cos();
            let ring: Vec<_> = ring.iter().map(|[x, y]| [142000. + x * cos - y * sin, -270000. + x * sin + y * cos]).collect();
            for kind in ["hip", "gable", "single_gable", "half_hipped", "two_cut_plumb", "half_hipped_two_cut"] {
                let mut s = fixture(&ring);
                let mut p = request(200.);
                p.as_object_mut().unwrap().remove("ridge");
                p["pitch_deg"] = json!(46.);
                p["verge_mm"] = json!(135.);
                if kind == "hip" {
                    p["gables"] = json!([]);
                } else if kind == "single_gable" {
                    p["gables"] = json!(["b.f2"]);
                }
                if kind.starts_with("half_hipped") {
                    p["kind"] = json!("half_hipped");
                    p["half_hips"] = json!([{"facade":"b.f2","drop_mm":300.},{"facade":"b.f4","drop_mm":400.}]);
                }
                if kind.contains("two_cut") {
                    p["fascia_depth_mm"] = json!(20.);
                }
                s.execute("building.set_roof", p.clone()).unwrap_or_else(|e| panic!("explicit {kind}, {yaw}: {e}"));
                let target = actual_height(&s);
                let footprint = s.doc.buildings[0].footprint.clone();
                p.as_object_mut().unwrap().remove("pitch_deg");
                p["ridge"] = json!(target);
                s.execute("building.set_roof", p).unwrap_or_else(|e| panic!("requested {kind}, {yaw}, {target}: {e}"));
                assert!((actual_height(&s) - target).abs() <= HEIGHT_TOLERANCE_MM, "{kind}, {yaw}, {target}");
                assert_eq!(s.doc.buildings[0].footprint, footprint);
                let built = trigon_model::build(&s.doc).unwrap();
                assert!((built[0].roof.as_ref().unwrap().pitch_deg - 46.).abs() < 1e-7);
                let glb = trigon_io::glb::write(&built, &[]);
                // glTF stores metres as f32: allow one relative f32 ulp, with a 0.001 mm floor.
                assert!((glb_height(&glb) - target).abs() <= (target.abs() * f64::from(f32::EPSILON)).max(0.001));
            }
        }
    }
}

#[test]
fn requested_roof_height_preserves_pitch_boundaries_and_rejects_unreachable_heights() {
    for pitch in [5_f64, 75.] {
        let mut s = fixture(&RECTANGLE);
        let mut p = request(400.);
        p.as_object_mut().unwrap().remove("ridge");
        p["pitch_deg"] = json!(pitch);
        // A cap at the retained-gable boundary at 5° must not require a viable lower trial.
        p["kind"] = json!("half_hipped");
        p["half_hips"] = json!([{"facade":"b.f4","drop_mm":3000. * 5_f64.to_radians().tan() - 100.}]);
        p["fascia_depth_mm"] = json!(400.);
        s.execute("building.set_roof", p.clone()).unwrap();
        let target = actual_height(&s);
        p.as_object_mut().unwrap().remove("pitch_deg");
        p["ridge"] = json!(target);
        s.execute("building.set_roof", p.clone()).unwrap();
        let built = trigon_model::build(&s.doc).unwrap();
        assert_eq!(built[0].roof.as_ref().unwrap().pitch_deg, pitch);
        assert!((actual_height(&s) - target).abs() <= HEIGHT_TOLERANCE_MM);
        let before = s.doc.clone();
        p["ridge"] = json!(target + if pitch == 5. { -0.001 } else { 0.001 });
        let err = s.execute("building.set_roof", p).unwrap_err();
        assert!(err.to_string().contains("5..75"), "{err}");
        assert_eq!(s.doc, before);
    }
}

#[test]
fn requested_roof_height_failures_keep_exact_state_and_redo() {
    let mut s = fixture(&RECTANGLE);
    s.execute("building.set_roof", request(200.)).unwrap();
    let accepted = s.doc.clone();
    s.execute("edit.undo", json!({})).unwrap();
    s.path = Some(std::path::PathBuf::from("roof-height-original.trigon"));
    s.dirty = false;
    let before = (s.doc.clone(), s.undo.clone(), s.redo.clone(), s.path.clone(), s.dirty);
    for overrides in [
        json!({"ridge":3499.999}),
        json!({"ridge":3500,"thickness_mm":1000}),
        json!({"ridge":20000}),
        json!({"ridge":1e300}),
        json!({"thickness_mm":-1}),
        json!({"overhang_mm":2001}),
        json!({"verge_mm":1501}),
        json!({"gables":["missing"]}),
        json!({"gables":["b.f1","b.f3"]}),
        json!({"pitch_deg":45}),
        json!({"kind":"half_hipped","half_hips":[{"facade":"b.f4","drop_mm":5000}]}),
        json!({"kind":"half_hipped","half_hips":[{"facade":"b.f4","drop_mm":300},{"facade":"b.f4","drop_mm":400}]}),
        json!({"fascia_depth_mm":20,"overhang_mm":0}),
    ] {
        let mut p = request(200.);
        p.as_object_mut().unwrap().extend(overrides.as_object().unwrap().clone());
        p["source"] = json!("Public failed height must leave no evidence entry");
        let error = s.execute("building.set_roof", p.clone()).unwrap_err();
        assert!(!matches!(error, crate::EngineError::Internal(_)), "{p}: {error}");
        assert_eq!((s.doc.clone(), s.undo.clone(), s.redo.clone(), s.path.clone(), s.dirty), before, "{p}");
    }
    s.execute("edit.redo", json!({})).unwrap();
    assert_eq!(s.doc, accepted);
    assert!((actual_height(&s) - 6000.).abs() <= HEIGHT_TOLERANCE_MM);
}

#[test]
fn requested_roof_height_retains_an_exact_half_hip_gable_margin() {
    for pitch in [32.7_f64, 46., 61.2] {
        let mut s = fixture(&RECTANGLE);
        let mut p = request(200.);
        p.as_object_mut().unwrap().remove("ridge");
        p["pitch_deg"] = json!(pitch);
        p["kind"] = json!("half_hipped");
        p["half_hips"] = json!([{"facade":"b.f4","drop_mm":3000. * pitch.to_radians().tan() - 100.}]);
        s.execute("building.set_roof", p.clone()).unwrap();
        let target = actual_height(&s);
        p.as_object_mut().unwrap().remove("pitch_deg");
        p["ridge"] = json!(target);
        s.execute("building.set_roof", p).unwrap_or_else(|e| panic!("pitch {pitch}: {e}"));
        assert!((actual_height(&s) - target).abs() <= HEIGHT_TOLERANCE_MM);
    }
}

#[test]
fn requested_roof_height_uses_the_existing_concave_and_straightened_outlines() {
    let ring = [[0., 0.], [12000., 0.], [12000., 6000.], [6500., 6000.], [6500., 12000.], [0., 12000.]];
    for straighten in [0., 1200., 6000.] {
        let mut s = fixture(&ring);
        let mut p = json!({"building":"b","kind":"pitched","pitch_deg":37.,"straighten_mm":straighten,
            "overhang_mm":400.,"thickness_mm":200.});
        s.execute("building.set_roof", p.clone()).unwrap();
        let target = actual_height(&s);
        let before = s.doc.buildings[0].footprint.clone();
        p.as_object_mut().unwrap().remove("pitch_deg");
        p["ridge"] = json!(target);
        s.execute("building.set_roof", p).unwrap();
        assert_eq!(s.doc.buildings[0].footprint, before);
        assert!((actual_height(&s) - target).abs() <= HEIGHT_TOLERANCE_MM);
    }
}

#[test]
fn requested_roof_height_undo_redo_and_native_reopen_preserve_geometry() {
    let mut s = fixture(&RECTANGLE);
    let before = s.doc.clone();
    let mut p = request(200.);
    p["fascia_depth_mm"] = json!(20.);
    s.execute("building.set_roof", p).unwrap();
    let after = s.doc.clone();
    let bytes = trigon_io::glb::write(&trigon_model::build(&after).unwrap(), &[]);
    s.execute("edit.undo", json!({})).unwrap();
    assert_eq!(s.doc, before);
    s.execute("edit.redo", json!({})).unwrap();
    assert_eq!(s.doc, after);
    let path = std::env::temp_dir().join(format!("trigon-requested-roof-height-{}.trigon", std::process::id()));
    s.execute("document.save", json!({"path":path})).unwrap();
    assert_eq!(std::fs::read(&path).unwrap(), serde_json::to_vec_pretty(&after).unwrap());
    let mut reopened = Session::new(Box::new(Canned));
    reopened.execute("document.open", json!({"path":path})).unwrap();
    std::fs::remove_file(path).unwrap();
    assert_eq!(reopened.doc, after);
    assert_eq!(trigon_io::glb::write(&trigon_model::build(&reopened.doc).unwrap(), &[]), bytes);
    assert!((actual_height(&reopened) - 6000.).abs() <= HEIGHT_TOLERANCE_MM);
}

#[test]
fn explicit_pitch_default_native_and_glb_bytes_keep_the_legacy_source() {
    let mut s = fixture(&RECTANGLE);
    let mut expected = s.doc.clone();
    let params = json!({"building":"b","kind":"pitched","pitch_deg":45});
    // The pre-existing native default shape: no new field or persistent requested height.
    expected.buildings[0].roof = serde_json::from_value(json!({"kind":"pitched","pitch_deg":45.,"gables":[],"straighten_mm":1200.})).unwrap();
    expected.journal.push(trigon_model::doc::JournalEntry { command: "building.set_roof".into(), params: params.clone() });
    s.execute("building.set_roof", params).unwrap();
    assert_eq!(serde_json::to_vec_pretty(&s.doc).unwrap(), serde_json::to_vec_pretty(&expected).unwrap());
    assert_eq!(
        trigon_io::glb::write(&trigon_model::build(&s.doc).unwrap(), &[]),
        trigon_io::glb::write(&trigon_model::build(&expected).unwrap(), &[])
    );
}
