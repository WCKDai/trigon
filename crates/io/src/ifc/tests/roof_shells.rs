//! Public analytic roofs: source covers, not reconstructed fragment bounds, are authoritative.
use super::*;

fn source(opening: &str, rotation: f64) -> (Document, Vec<Built>) {
    let (sin, cos) = rotation.to_radians().sin_cos();
    let outline = [[0., 0.], [24000., 0.], [24000., 12000.], [0., 12000.]].map(|[x, y]| [150000. + x * cos - y * sin, -270000. + x * sin + y * cos]);
    let mut b = trigon_model::Building::new("b", "Public exact IFC roof", &outline, 3, 3400.).unwrap();
    let mut roof = serde_json::json!({"kind":"pitched","pitch_deg":46,"thickness_mm":200,"gables":["b.f2","b.f4"],"straighten_mm":0});
    if opening == "dormer" {
        roof["dormers"] = serde_json::json!([{"id":"shed","facade":"b.f1","u_mm":10000,
            "window_type":"window.casement.2","width_mm":1400,"height_mm":1000,"setback_mm":350,
            "cheek_mm":100,"head_mm":130,"sill_clearance_mm":80,"roof_pitch_deg":6,"roof_thickness_mm":65}]);
    } else if opening == "rooflight" || opening == "mansard" {
        roof["rooflights"] = serde_json::json!([{"id":"light","facade":"b.f1","u_mm":10000,"run_mm":500,
            "window_type":"window.casement.2","width_mm":800,"height_mm":1000,"stand_off_mm":100,"flashing_mm":100}]);
    }
    if opening == "mansard" {
        roof["kind"] = serde_json::json!("mansard");
        roof.as_object_mut().unwrap().remove("pitch_deg");
    }
    b.roof = serde_json::from_value(roof).unwrap();
    let mut doc = Document::default();
    doc.buildings.push(b);
    let built = trigon_model::build(&doc).unwrap();
    (doc, built)
}

fn records(text: &str) -> BTreeMap<usize, Entity<'_>> {
    text.lines()
        .filter(|l| l.starts_with('#'))
        .map(|l| {
            let (id, body) = l.split_once('=').unwrap();
            let (kind, body) = body.split_once('(').unwrap();
            (reference(id), Entity { kind, args: args(body.strip_suffix(");").unwrap()) })
        })
        .collect()
}

fn roof_id(records: &BTreeMap<usize, Entity<'_>>) -> usize {
    *records.iter().find(|(_, e)| e.kind == "IFCROOF" && e.args[0] == gid("b.roof")).unwrap().0
}

fn property_sets<'a>(records: &'a BTreeMap<usize, Entity<'a>>, object: usize) -> Vec<&'a Entity<'a>> {
    records
        .values()
        .filter(|e| e.kind == "IFCRELDEFINESBYPROPERTIES" && references(e.args[4]).contains(&object))
        .map(|e| &records[&reference(e.args[5])])
        .collect()
}

fn properties<'a>(records: &'a BTreeMap<usize, Entity<'a>>, object: usize) -> BTreeMap<&'a str, &'a str> {
    let sets = property_sets(records, object);
    let pset = sets.iter().find(|e| e.kind == "IFCPROPERTYSET" && e.args[2] == "'Trigon_RoofGeometry'").unwrap();
    references(pset.args[4]).into_iter().map(|i| (records[&i].args[0], records[&i].args[2])).collect()
}

fn vertical_hits(mesh: &Mesh, q: [f64; 2]) -> Vec<f64> {
    let mut hits = Vec::new();
    for tri in &mesh.triangles {
        let [a, b, c] = tri.map(|i| mesh.positions[i as usize]);
        let det = (b[0] - a[0]) * (c[1] - a[1]) - (b[1] - a[1]) * (c[0] - a[0]);
        if det.abs() < 1e-8 {
            continue;
        }
        let u = ((q[0] - a[0]) * (c[1] - a[1]) - (q[1] - a[1]) * (c[0] - a[0])) / det;
        let v = ((b[0] - a[0]) * (q[1] - a[1]) - (b[1] - a[1]) * (q[0] - a[0])) / det;
        if u >= -1e-9 && v >= -1e-9 && u + v <= 1. + 1e-9 {
            hits.push(a[2] + u * (b[2] - a[2]) + v * (c[2] - a[2]));
        }
    }
    hits.sort_by(f64::total_cmp);
    hits.dedup_by(|a, b| (*a - *b).abs() < 0.01);
    hits
}

#[test]
fn opened_roof_ifc_keeps_actual_200mm_cover_instead_of_fallback_prisms() {
    for rotation in [0.0_f64, 37.25, -118.] {
        let (doc, built) = source("dormer", rotation);
        let original = serde_json::to_vec(&doc).unwrap();
        let cover = &built[0].roof.as_ref().unwrap().cover;
        assert!(closed_outward(cover));
        let text = write(&doc, &built, "exact.ifc", "2026-01-01T00:00:00");
        assert!(!text.lines().any(|l| l.contains("=IFCSLAB(") && l.contains("b.roof.face")), "erroneous reconstructed face products must be retired");
        let model = split::model_shapes(&doc, &built);
        assert_eq!(model.iter().find(|(id, _)| id == "b.roof").unwrap().1, *cover);
        let (sin, cos) = rotation.to_radians().sin_cos();
        // Public point where the old writer substituted a 250 mm plane for a 200 mm source.
        let q = [150000. + 10_482.778_084_207_42 * cos - 7547.857572625772 * sin, -270000. + 10_482.778_084_207_42 * sin + 7547.857572625772 * cos];
        let expected = vertical_hits(cover, q);
        assert_eq!(expected.len(), 2);
        assert!(((expected[1] - expected[0]) * 46.0_f64.to_radians().cos() - 200.).abs() < 1e-4);
        for read in [trigon_ifcread::read_ifc_lite(&text).unwrap(), trigon_ifcread::read_bimifc(&text).unwrap()] {
            assert!(read.failures.is_empty());
            let target =
                read.elements.iter().find(|e| e.global_id == guid("trigon:b.roof")).expect("actual parent roof must carry its original cover");
            let actual = Mesh { positions: target.mesh.positions.clone(), triangles: target.mesh.triangles.clone() };
            let hits = vertical_hits(&actual, q);
            assert_eq!(hits.len(), 2);
            for (a, b) in expected.iter().zip(&hits) {
                assert!((a - b).abs() < 0.1, "{} changed source boundary {a} to {b}", read.reader.name());
            }
            assert!((signed_volume(&actual) - signed_volume(cover)).abs() < signed_volume(cover) * 1e-4);
        }
        assert_eq!(original, serde_json::to_vec(&doc).unwrap());
    }
}

#[test]
fn exact_roof_ifc_preserves_source_type_layers_and_provable_geometry_quantities() {
    for opening in ["dormer", "rooflight", "mansard"] {
        let (doc, built) = source(opening, 0.);
        let roof = &built[0].roof.as_ref().unwrap();
        let text = write(&doc, &built, "exact.ifc", "2026-01-01T00:00:00");
        let entities = records(&text);
        let id = roof_id(&entities);
        let typing = entities.values().find(|e| e.kind == "IFCRELDEFINESBYTYPE" && references(e.args[4]).contains(&id)).unwrap();
        let ty = &entities[&reference(typing.args[5])];
        assert_eq!(ty.kind, "IFCROOFTYPE");
        assert_eq!(ty.args[9], if opening == "mansard" { ".MANSARD_ROOF." } else { ".GABLE_ROOF." });
        for product in [id, reference(typing.args[5])] {
            let rel = entities.values().find(|e| e.kind == "IFCRELASSOCIATESMATERIAL" && references(e.args[4]).contains(&product)).unwrap();
            let set = &entities[&reference(rel.args[5])];
            assert_eq!(set.kind, "IFCMATERIALLAYERSET", "whole multi-plane roof must not get a fictitious AXIS3 usage");
            let layers = references(set.args[0]);
            assert_eq!(layers.len(), 2);
            let thickness: f64 = layers.iter().map(|i| entities[i].args[1].parse::<f64>().unwrap()).sum();
            assert!((thickness - 200.).abs() < 1e-6);
        }
        let props = properties(&entities, id);
        assert_eq!(props["'SourceNormalThickness'"], "IFCPOSITIVELENGTHMEASURE(200.)");
        assert!(props["'SolidCoverage'"].contains("Verified closed"));
        assert!(props["'OuterSkinAreas'"].contains("Unavailable"));
        let sets = property_sets(&entities, id);
        assert!(
            !sets.iter().any(|e| e.kind == "IFCELEMENTQUANTITY" && e.args[2] == "'Qto_RoofBaseQuantities'"),
            "no invented outer-skin area or nonstandard roof-base volume"
        );
        let qto = sets.iter().find(|e| e.kind == "IFCELEMENTQUANTITY" && e.args[2] == "'Trigon_RoofQuantities'").unwrap();
        let q: BTreeMap<_, _> =
            references(qto.args[5]).into_iter().map(|i| (entities[&i].args[0], entities[&i].args[3].parse::<f64>().unwrap())).collect();
        assert_eq!(q.len(), 2);
        assert!((q["'SolidVolume'"] * 1e9 - signed_volume(&roof.cover)).abs() < 0.51);
        let boundary: f64 = roof
            .cover
            .triangles
            .iter()
            .map(|t| {
                let [a, b, c] = t.map(|i| roof.cover.positions[i as usize]);
                let n = cross(sub(b, a), sub(c, a));
                dot(n, n).sqrt() / 2.
            })
            .sum();
        assert!((q["'TotalBoundaryArea'"] * 1e6 - boundary).abs() < 0.00051);
    }
}

#[test]
fn unsupported_actual_roof_shell_is_reported_unverified_without_solid_quantities() {
    let (doc, original) = source("rooflight", 0.);
    for case in ["open", "reversed", "duplicate", "inverted-component", "nonfinite", "malformed-index"] {
        let mut built = original.clone();
        let roof = built[0].roof.as_mut().unwrap();
        match case {
            "open" => {
                roof.cover.triangles.pop();
            }
            "reversed" => {
                for t in &mut roof.cover.triangles {
                    t.swap(1, 2);
                }
            }
            "duplicate" => {
                roof.cover.triangles.extend(roof.cover.triangles.clone());
            }
            "nonfinite" => roof.cover.positions[0][0] = f64::NAN,
            "malformed-index" => roof.cover.triangles[0][0] = u32::MAX,
            _ => {
                let mut cube = Mesh::default();
                let lo = [[50000., 0., 0.], [50100., 0., 0.], [50100., 100., 0.], [50000., 100., 0.]];
                cube.hexahedron(lo, lo.map(|p| [p[0], p[1], p[2] + 100.]));
                for t in &mut cube.triangles {
                    t.swap(1, 2);
                }
                roof.cover.append(&cube);
            }
        }
        let text = write(&doc, &built, "unsupported.ifc", "2026-01-01T00:00:00");
        let entities = records(&text);
        let id = roof_id(&entities);
        let props = properties(&entities, id);
        assert!(props["'SolidCoverage'"].contains("Unverified"), "{case}");
        assert!(
            !property_sets(&entities, id).iter().any(|e| e.kind == "IFCELEMENTQUANTITY"),
            "{case} must not receive a successful solid certificate"
        );
        if matches!(case, "nonfinite" | "malformed-index") {
            assert_eq!(entities[&id].args[6], "$", "unrepresentable coordinates or indices must not be silently replaced with fabricated geometry");
        } else {
            assert_ne!(entities[&id].args[6], "$", "keep finite actual source geometry; do not silently substitute prisms");
        }
    }
}

#[test]
fn unrepresentable_roof_thickness_does_not_export_a_zero_positive_length() {
    let (mut doc, _) = source("plain", 0.);
    doc.buildings[0].roof = serde_json::from_value(serde_json::json!({"kind":"mansard","thickness_mm":0.00000001})).unwrap();
    let built = trigon_model::build(&doc).unwrap();
    let text = write(&doc, &built, "tiny.ifc", "2026-01-01T00:00:00");
    let entities = records(&text);
    let props = properties(&entities, roof_id(&entities));
    assert!(!props.contains_key("'SourceNormalThickness'"), "a positive source below written precision must not become IfcPositiveLengthMeasure(0.)");
    assert!(props["'ConstructionBasis'"].contains("Unverified"));
}

#[test]
fn exact_roof_layer_sets_do_not_alias_distinct_fractional_source_thicknesses() {
    let (mut doc, _) = source("rooflight", 0.);
    if let trigon_model::Roof::Pitched { thickness_mm, .. } = &mut doc.buildings[0].roof {
        *thickness_mm = 200.125;
    }
    let mut other = doc.buildings[0].clone();
    other.id = "other".into();
    for point in &mut other.footprint {
        point[0] += 30000.;
    }
    for facade in &mut other.facades {
        facade.id = facade.id.replacen("b.", "other.", 1);
    }
    if let trigon_model::Roof::Pitched { thickness_mm, gables, rooflights, .. } = &mut other.roof {
        *thickness_mm = 200.375;
        for id in gables {
            *id = id.replacen("b.", "other.", 1);
        }
        for light in rooflights {
            light.facade = light.facade.replacen("b.", "other.", 1);
        }
    }
    doc.buildings.push(other);
    let built = trigon_model::build(&doc).unwrap();
    let text = write(&doc, &built, "fractional.ifc", "2026-01-01T00:00:00");
    let entities = records(&text);
    let mut sets = Vec::new();
    for (key, expected) in [("b.roof", 200.125), ("other.roof", 200.375)] {
        let (&id, _) = entities.iter().find(|(_, e)| e.kind == "IFCROOF" && e.args[0] == gid(key)).unwrap();
        let rel = entities.values().find(|e| e.kind == "IFCRELASSOCIATESMATERIAL" && references(e.args[4]).contains(&id)).unwrap();
        let set = reference(rel.args[5]);
        sets.push(set);
        let thickness: f64 = references(entities[&set].args[0]).iter().map(|i| entities[i].args[1].parse::<f64>().unwrap()).sum();
        assert!((thickness - expected).abs() < 1e-6, "{key}: {thickness} versus {expected}");
    }
    assert_ne!(sets[0], sets[1]);
}
