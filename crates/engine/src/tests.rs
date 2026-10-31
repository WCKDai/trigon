//! Engine tests against a canned provider: no network.

use serde_json::{Value, json};
use trigon_data::{Address, DataError, Footprint, Heights, Photo, Provider};
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
        Ok(Footprint { ring_utm: ring, tags: Default::default(), evidence: ev("osm:way/1") })
    }
    fn heights(&self, _: &[[f64; 2]]) -> trigon_data::Result<Heights> {
        Ok(Heights { terrain: 12.0, median: 15.2, top: 17.9, samples: 1800, evidence: ev("dhm:test") })
    }
    fn photos(&self, _: [f64; 2]) -> trigon_data::Result<Vec<Photo>> {
        Ok(vec![Photo { id: "p-north".into(), collection: "c".into(), direction: "north".into(), datetime: "2025".into() }])
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
