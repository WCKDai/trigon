use super::*;
use serde_json::{Value, json};
use std::collections::BTreeMap;

const MULTI: &str = include_str!("offset-three-flight-upper-continuity.json");
const FAN: &str = include_str!("rectangular-fan.json");
fn fixture(text: &str) -> (SequenceV1, Vec<PhysicalFloor>) {
    let v: Value = serde_json::from_str(text).unwrap();
    let s = SequenceV1::from_json(&serde_json::to_vec(&v["source"]).unwrap()).unwrap();
    let floors = v["building"]["storeys"]
        .as_array()
        .unwrap()
        .iter()
        .enumerate()
        .map(|(level, f)| PhysicalFloor { floor: FloorRef::Storey { level }, top_z_mm: f["elevation"].as_f64().unwrap(), thickness_mm: 220. })
        .collect();
    (s, floors)
}
fn error(s: &SequenceV1, floors: &[PhysicalFloor], message: &str) {
    let e = resolve_geometry(s, floors).unwrap_err().to_string();
    assert!(e.contains(message), "expected {message}, got {e}");
}
fn parse_error(bytes: &[u8], message: &str) {
    let e = SequenceV1::from_json(bytes).unwrap_err().to_string();
    assert!(e.contains(message), "expected {message}, got {e}");
}
fn close(a: f64, b: f64) {
    assert!((a - b).abs() < 0.02, "{a} != {b}");
}
fn subtract(a: [f64; 3], b: [f64; 3]) -> [f64; 3] {
    [a[0] - b[0], a[1] - b[1], a[2] - b[2]]
}
fn vector_cross(a: [f64; 3], b: [f64; 3]) -> [f64; 3] {
    [a[1] * b[2] - a[2] * b[1], a[2] * b[0] - a[0] * b[2], a[0] * b[1] - a[1] * b[0]]
}
fn vector_dot(a: [f64; 3], b: [f64; 3]) -> f64 {
    a.iter().zip(b).map(|(x, y)| x * y).sum()
}
/// Independent indexed half-edge audit and divergence-theorem integration. Does not use
/// resolver area/volume helpers or an implementation-derived expected quantity.
fn audit(p: &SolidPart, expected_volume: f64, expected_top: f64, expected_bottom: f64) -> f64 {
    let m = &p.mesh;
    assert!(!m.triangles.is_empty());
    assert!(m.positions.iter().flatten().all(|x| x.is_finite()));
    let mut edges: BTreeMap<(u32, u32), Vec<(u32, u32)>> = BTreeMap::new();
    let origin = m.positions[0];
    let mut volume = 0.;
    let mut top_area = 0.;
    for &[a, b, c] in &m.triangles {
        let (x, y, z) = (m.positions[a as usize], m.positions[b as usize], m.positions[c as usize]);
        let normal = vector_cross(subtract(y, x), subtract(z, x));
        assert!(vector_dot(normal, normal) > 1e-16, "degenerate face in {}", p.id);
        for (i, j) in [(a, b), (b, c), (c, a)] {
            edges.entry((i.min(j), i.max(j))).or_default().push((i, j));
        }
        volume += vector_dot(subtract(x, origin), vector_cross(subtract(y, origin), subtract(z, origin))) / 6.;
        if [x, y, z].iter().all(|v| (v[2] - expected_top).abs() < 1e-6) {
            assert!(normal[2] > 0.);
            top_area += normal[2] / 2.;
        } else if [x, y, z].iter().all(|v| (v[2] - expected_bottom).abs() < 1e-6) {
            assert!(normal[2] < 0.);
        } else {
            assert_eq!(normal[2], 0.);
            let center = [(x[0] + y[0] + z[0]) / 3., (x[1] + y[1] + z[1]) / 3.];
            let mut found = false;
            for (u, v) in p.outline.iter().zip(p.outline.iter().cycle().skip(1)) {
                let direction = [v[0] - u[0], v[1] - u[1]];
                let relative = [center[0] - u[0], center[1] - u[1]];
                let distance = (relative[0] * direction[1] - relative[1] * direction[0]).abs() / direction[0].hypot(direction[1]);
                let projection =
                    (relative[0] * direction[0] + relative[1] * direction[1]) / (direction[0] * direction[0] + direction[1] * direction[1]);
                if distance < 1e-6 && (0.0..=1.0).contains(&projection) {
                    assert!(normal[0] * direction[1] - normal[1] * direction[0] > 0., "inward side in {}", p.id);
                    found = true;
                    break;
                }
            }
            assert!(found, "side not on original boundary");
        }
    }
    for v in edges.values() {
        assert_eq!(v.len(), 2, "nonmanifold/open edge in {}", p.id);
        assert_eq!(v[0], (v[1].1, v[1].0), "inconsistent winding");
    }
    assert!(volume > 0.);
    close(volume, expected_volume);
    close(p.gross_volume_mm3, expected_volume);
    close(top_area * (expected_top - expected_bottom), expected_volume);
    assert!((m.positions.iter().map(|p| p[2]).fold(f64::NEG_INFINITY, f64::max) - expected_top).abs() < 1e-6);
    assert!((m.positions.iter().map(|p| p[2]).fold(f64::INFINITY, f64::min) - expected_bottom).abs() < 1e-6);
    volume
}
fn all_solids(r: &ResolvedSequence) -> impl Iterator<Item = &SolidPart> {
    r.flights.iter().flat_map(|f| &f.treads).chain(r.landings.iter().filter_map(|l| l.solid.as_ref()))
}

#[test]
fn public_independent_elevations_actual_mesh_oracle() {
    let (s, floors) = fixture(MULTI);
    let r = resolve_geometry(&s, &floors).unwrap();
    assert_eq!(r.landings.iter().map(|l| l.top_z_mm).collect::<Vec<_>>(), [-2100., -120., 600., 1800., 3000.]);
    assert_eq!(r.flights.iter().map(|f| f.riser_count).collect::<Vec<_>>(), [11, 4, 6, 6]);
    assert_eq!(r.flights.iter().map(|f| f.riser_height_mm).collect::<Vec<_>>(), [180., 180., 200., 200.]);
    let top = [
        vec![-1920., -1740., -1560., -1380., -1200., -1020., -840., -660., -480., -300.],
        vec![60., 240., 420.],
        vec![800., 1000., 1200., 1400., 1600.],
        vec![2000., 2200., 2400., 2600., 2800.],
    ];
    let individual_volumes = [82_500_000., 99_000_000., 105_000_000., 105_000_000.];
    let heights = [330., 330., 350., 350.];
    let totals = [825_000_000., 297_000_000., 525_000_000., 525_000_000.];
    let mut total = 0.;
    for (i, f) in r.flights.iter().enumerate() {
        close(f.gross_volume_mm3, totals[i]);
        assert_eq!(f.treads.len(), top[i].len());
        for (j, t) in f.treads.iter().enumerate() {
            total += audit(t, individual_volumes[i], top[i][j], top[i][j] - heights[i]);
        }
    }
    for (i, l) in r.landings.iter().enumerate().take(4) {
        let thickness = if i == 0 { 180. } else { 220. };
        total += audit(l.solid.as_ref().unwrap(), 1_000_000. * thickness, l.top_z_mm, l.top_z_mm - thickness);
    }
    close(total, 3_012_000_000.);
    close(r.gross_volume_mm3, 3_012_000_000.);
    assert!(r.landings[4].solid.is_none());
    assert_eq!(r.landings[4].outline, s.landings[4].outline);
    assert_eq!(r.landings[4].surface_role, SurfaceRole::FloorOwnedSupportUnverified);
    let threshold = r.landings[1].access.as_ref().unwrap();
    assert_eq!(threshold.offset_mm, -120.);
    assert!(threshold.requires_source_review);
    write_artifact("multi", &s, &floors, &r);
    assert_eq!(r.proposed_openings.len(), 2);
    assert_eq!(r.building_clearance, Assessment::NotAssessed);
    assert_eq!(r.retained_floor_support, Assessment::NotAssessed);
}
#[test]
fn intermediate_edit_changes_only_adjacent_geometry_and_keeps_ids() {
    let (mut s, floors) = fixture(MULTI);
    let before = resolve_geometry(&s, &floors).unwrap();
    s.landings[2].elevation.offset_mm = 660.;
    let after = resolve_geometry(&s, &floors).unwrap();
    assert_eq!(after.flights[1].riser_height_mm, 195.);
    assert_eq!(after.flights[2].riser_height_mm, 190.);
    assert_eq!(before.flights[0], after.flights[0]);
    assert_eq!(before.flights[3], after.flights[3]);
    for i in [0, 1, 3, 4] {
        assert_eq!(before.landings[i], after.landings[i]);
    }
    assert_eq!(before.proposed_openings, after.proposed_openings);
    assert_eq!(before.part_ids(), after.part_ids());
    assert_eq!(after.identity_delta(&before).unwrap(), IdentityDelta { added_ids: vec![], removed_ids: vec![], count_changes: vec![] });
    close(after.gross_volume_mm3, 3_010_500_000.);
    let mut sum = 0.;
    for (i, f) in after.flights.iter().enumerate() {
        let (base, h, volume) =
            [(-2100., 180., 82_500_000.), (-120., 195., 103_500_000.), (660., 190., 102_000_000.), (1800., 200., 105_000_000.)][i];
        for (j, t) in f.treads.iter().enumerate() {
            let top = base + (j + 1) as f64 * h;
            sum += audit(t, volume, top, top - h - 150.);
        }
    }
    for (i, l) in after.landings.iter().enumerate().take(4) {
        let h = if i == 0 { 180. } else { 220. };
        sum += audit(l.solid.as_ref().unwrap(), 1_000_000. * h, l.top_z_mm, l.top_z_mm - h);
    }
    close(sum, 3_010_500_000.);
    write_artifact("multi-raised-rear", &s, &floors, &after);
}
#[test]
fn rectangular_fan_retains_real_corner_and_closed_solids() {
    let (s, floors) = fixture(FAN);
    let r = resolve_geometry(&s, &floors).unwrap();
    let f = &r.flights[0];
    assert_eq!(f.riser_count, 4);
    assert_eq!(f.riser_height_mm, 180.);
    let mut total = 0.;
    for (i, t) in f.treads.iter().enumerate() {
        total += audit(t, [82_500_000., 165_000_000., 82_500_000.][i], [180., 360., 540.][i], [-150., 30., 210.][i]);
    }
    let center = &f.treads[1];
    assert!(center.outline.contains(&[1000., 1000.]));
    assert!(center.mesh.positions.contains(&[1000., 1000., 360.]));
    assert_eq!(center.gross_volume_mm3 / 330., 500_000.);
    total += audit(r.landings[0].solid.as_ref().unwrap(), 200_000_000., 0., -200.);
    total += audit(r.landings[1].solid.as_ref().unwrap(), 200_000_000., 720., 520.);
    close(total, 730_000_000.);
    close(r.gross_volume_mm3, 730_000_000.);
    write_artifact("fan", &s, &floors, &r);
}
fn transform(s: &mut SequenceV1, angle: f64, offset: Point) {
    let (sin, cos) = angle.sin_cos();
    let change = |p: &mut Point| {
        let [x, y] = *p;
        *p = [cos * x - sin * y + offset[0], sin * x + cos * y + offset[1]];
    };
    for l in &mut s.landings {
        for p in &mut l.outline {
            change(p);
        }
        for p in &mut l.ports {
            for v in &mut p.edge {
                change(v);
            }
        }
        for p in &mut l.walk_via {
            change(p);
        }
    }
    for f in &mut s.flights {
        if let Profile::TreadChain { treads } = &mut f.profile {
            for t in treads {
                for p in &mut t.outline {
                    change(p);
                }
            }
        }
    }
    for o in &mut s.openings {
        for p in &mut o.outline {
            change(p);
        }
    }
}
#[test]
fn rigid_transforms_preserve_analytic_body_quantities_ids_and_datums() {
    for text in [MULTI, FAN] {
        for (angle, offset) in [(0., [12345., -87654.]), (37_f64.to_radians(), [245000., -190000.]), (-90_f64.to_radians(), [-900000., 900000.])] {
            let (mut s, mut floors) = fixture(text);
            let before = resolve_geometry(&s, &floors).unwrap();
            transform(&mut s, angle, offset);
            for f in &mut floors {
                f.top_z_mm += 321.;
            }
            let after = resolve_geometry(&s, &floors).unwrap();
            assert_eq!(before.part_ids(), after.part_ids());
            close(before.gross_volume_mm3, after.gross_volume_mm3);
            for (a, b) in all_solids(&before).zip(all_solids(&after)) {
                audit(b, a.gross_volume_mm3, a.top_z_mm + 321., a.bottom_z_mm + 321.);
            }
        }
    }
}
#[test]
fn gate_width_limits_survive_rigid_transforms_without_snapping() {
    let scaled = |width: f64| {
        let (mut source, floors) = fixture(MULTI);
        let change = |p: &mut Point| p[1] = (p[1] - 3000.) * width / 1000.;
        for landing in &mut source.landings {
            for p in &mut landing.outline {
                change(p);
            }
            for port in &mut landing.ports {
                for p in &mut port.edge {
                    change(p);
                }
            }
            for p in &mut landing.walk_via {
                change(p);
            }
        }
        for opening in &mut source.openings {
            for p in &mut opening.outline {
                change(p);
            }
        }
        (source, floors)
    };
    for width in [600., 4000.] {
        let (source, floors) = scaled(width);
        let before = resolve_geometry(&source, &floors).unwrap();
        for (angle, offset) in [(37_f64.to_radians(), [245000., -190000.]), (-73_f64.to_radians(), [-900000., 900000.])] {
            let mut placed = source.clone();
            transform(&mut placed, angle, offset);
            let authored = placed.clone();
            let after = resolve_geometry(&placed, &floors).unwrap();
            assert_eq!(placed, authored, "validation must not snap authored coordinates");
            assert_eq!(before.part_ids(), after.part_ids());
            close(before.gross_volume_mm3, after.gross_volume_mm3);
            for (a, b) in all_solids(&before).zip(all_solids(&after)) {
                audit(b, a.gross_volume_mm3, a.top_z_mm, a.bottom_z_mm);
            }
        }
    }
    // The existing geometric tolerance is bounded: a real excess still refuses,
    // both at the origin and after the same rigid placement as the valid case.
    for width in [600. - 2e-6, 4000. + 2e-6] {
        let (mut source, floors) = scaled(width);
        error(&source, &floors, "gate width");
        transform(&mut source, 37_f64.to_radians(), [245000., -190000.]);
        error(&source, &floors, "gate width");
    }
}

#[test]
fn count_changes_are_explicit_and_keep_authored_id_order() {
    let (mut s, floors) = fixture(MULTI);
    let old = resolve_geometry(&s, &floors).unwrap();
    if let Profile::Straight { tread_ids } = &mut s.flights[1].profile {
        *tread_ids = vec!["z".into(), "a".into(), "t03".into(), "b".into()];
    }
    let new = resolve_geometry(&s, &floors).unwrap();
    let d = new.identity_delta(&old).unwrap();
    assert_eq!(d.added_ids, ["public.route.flight.f1.tread.z", "public.route.flight.f1.tread.a", "public.route.flight.f1.tread.b"]);
    assert_eq!(d.removed_ids, ["public.route.flight.f1.tread.t01", "public.route.flight.f1.tread.t02"]);
    assert_eq!(
        d.count_changes,
        [FlightCountChange { flight_id: "public.route.flight.f1".into(), old_treads: 3, new_treads: 4, old_risers: 4, new_risers: 5 }]
    );
    s.id = "other".into();
    let another = resolve_geometry(&s, &floors).unwrap();
    assert!(another.identity_delta(&old).unwrap_err().to_string().contains("preserve source ID"));
}

#[test]
fn strict_unknown_duplicate_nested_fields_and_required_nulls() {
    let (s, _) = fixture(FAN);
    let original = serde_json::to_value(&s).unwrap();
    for pointer in [
        "",
        "/landings/0",
        "/landings/0/elevation",
        "/landings/0/elevation/floor",
        "/landings/0/surface",
        "/landings/0/ports/0",
        "/flights/0",
        "/flights/0/from",
        "/flights/0/profile",
        "/flights/0/profile/treads/0",
        "/flights/0/guards",
    ] {
        let mut v = original.clone();
        v.pointer_mut(pointer).unwrap().as_object_mut().unwrap().insert("mystery".into(), json!(0));
        parse_error(&serde_json::to_vec(&v).unwrap(), "unknown field");
        let mut v = original.clone();
        v.pointer_mut(pointer).unwrap().as_object_mut().unwrap().insert("DUPLICATE_MARKER".into(), json!(0));
        let obj = original.pointer(pointer).unwrap().as_object().unwrap();
        let (key, value) = obj.iter().next().unwrap();
        let text =
            serde_json::to_string(&v).unwrap().replace("\"DUPLICATE_MARKER\":0", &format!("{}:{}", serde_json::to_string(key).unwrap(), value));
        parse_error(text.as_bytes(), "duplicate field");
    }
    for (pointer, key) in [("/landings/0", "access_level"), ("/flights/0/guards", "left"), ("/flights/0/guards", "right")] {
        let mut v = original.clone();
        v.pointer_mut(pointer).unwrap().as_object_mut().unwrap().remove(key);
        parse_error(&serde_json::to_vec(&v).unwrap(), "missing field");
    }
    let text = serde_json::to_string(&s).unwrap();
    for (from, to) in [
        ("\"left\":null", "\"left\":null,\"left\":null"),
        ("\"right\":null", "\"right\":null,\"right\":null"),
        ("\"access_level\":null", "\"access_level\":null,\"access_level\":null"),
    ] {
        parse_error(text.replace(from, to).as_bytes(), "duplicate field");
    }
    assert_eq!(s, SequenceV1::from_json(text.as_bytes()).unwrap());
}
#[test]
fn tagged_alternatives_reject_wrong_fields_and_invalid_kind_without_fallback() {
    let (s, _) = fixture(FAN);
    let orig = serde_json::to_value(s).unwrap();
    for (pointer, key, value, msg) in [
        ("", "kind", json!("sequence_v2"), "unknown variant"),
        ("/landings/0/elevation/floor", "level", Value::Null, "invalid type"),
        ("/landings/0/elevation/floor", "kind", json!("attic"), "without level"),
        ("/flights/0/profile", "tread_ids", json!(["t"]), "profile requires"),
        ("/landings/0/surface", "floor", json!({"kind":"attic"}), "surface requires"),
    ] {
        let mut v = orig.clone();
        v.pointer_mut(pointer).unwrap()[key] = value;
        parse_error(&serde_json::to_vec(&v).unwrap(), msg);
    }
}
#[test]
fn bounded_deserialization_rejects_excess_before_reading_excess_item() {
    let (s, _) = fixture(FAN);
    let orig = serde_json::to_value(s).unwrap();
    for (pointer, max) in [
        ("/landings", 33),
        ("/flights", 32),
        ("/landings/0/outline", 64),
        ("/landings/0/ports", 2),
        ("/flights/0/profile/treads", 80),
        ("/flights/0/profile/treads/0/outline", 16),
    ] {
        let mut v = orig.clone();
        let a = v.pointer_mut(pointer).unwrap().as_array_mut().unwrap();
        let item = a[0].clone();
        *a = vec![item; max];
        a.push(json!("EXCESS"));
        let text = serde_json::to_string(&v).unwrap().replace("\"EXCESS\"", "{\"incomplete");
        parse_error(text.as_bytes(), "collection exceeds its limit");
    }
    let mut v = orig.clone();
    v["landings"][0]["walk_via"] = json!(vec![[0, 0]; 9]);
    parse_error(&serde_json::to_vec(&v).unwrap(), "collection exceeds its limit");
    parse_error(&vec![b' '; MAX_JSON_BYTES + 1], "sequence JSON bytes");
}
#[test]
fn hostile_scalar_indices_and_text_fail_before_geometry() {
    let (original, floors) = fixture(FAN);
    for value in [f64::NAN, f64::INFINITY, f64::NEG_INFINITY, 1e100] {
        let mut s = original.clone();
        s.landings[0].elevation.offset_mm = value;
        error(&s, &floors, "landing offset");
        let mut s = original.clone();
        s.flights[0].soffit_mm = value;
        error(&s, &floors, "flight soffit");
        let mut s = original.clone();
        s.landings[0].outline[0][0] = value;
        error(&s, &floors, "sequence X");
        let mut f = floors.clone();
        f[0].top_z_mm = value;
        error(&original, &f, "physical floor top");
    }
    let mut s = original.clone();
    if let Profile::TreadChain { treads } = &mut s.flights[0].profile {
        treads[0].exit_edge = [usize::MAX, 0];
    }
    error(&s, &floors, "edge index out of range");
    let mut v = serde_json::to_value(&original).unwrap();
    v["flights"][0]["profile"]["treads"][0]["entry_edge"] = json!([18446744073709551615u64, 0]);
    parse_error(&serde_json::to_vec(&v).unwrap(), "edge index out of range");
    let mut s = original.clone();
    s.landings[0].elevation.floor = FloorRef::Storey { level: usize::MAX };
    error(&s, &floors, "storey level");
    let mut s = original.clone();
    s.id = "bad:id".into();
    error(&s, &floors, "ID must contain");
    let mut s = original.clone();
    s.name = "x".repeat(201);
    error(&s, &floors, "name bytes");
    let mut s = original.clone();
    s.name = "control\n".into();
    error(&s, &floors, "control characters");
    let text = serde_json::to_string(&original).unwrap().replace("\"offset_mm\":1000.0", "\"offset_mm\":1e309");
    parse_error(text.as_bytes(), "number out of range");
}
#[test]
fn collection_and_aggregate_caps_apply_to_direct_rust_callers() {
    let (original, floors) = fixture(FAN);
    let mut s = original.clone();
    s.flights = vec![s.flights[0].clone(); 33];
    error(&s, &floors, "flights: count");
    let mut s = original.clone();
    s.landings = vec![s.landings[0].clone(); 34];
    error(&s, &floors, "landings: count");
    let mut s = original.clone();
    let flight = s.flights[0].clone();
    s.flights = vec![flight; 13];
    for f in &mut s.flights {
        f.profile = Profile::Straight { tread_ids: vec!["t".into(); 80] };
    }
    error(&s, &floors, "sequence treads: count");
    let mut s = original.clone();
    s.flights = vec![s.flights[0].clone(); 13];
    for (i, f) in s.flights.iter_mut().enumerate() {
        if let Profile::TreadChain { treads } = &mut f.profile {
            let mut t = treads[0].clone();
            t.outline = vec![[0., 0.]; 16];
            *treads = vec![t; if i == 12 { 64 } else { 80 }];
        }
    }
    s.landings = vec![s.landings[0].clone(); 33];
    for l in &mut s.landings {
        l.outline = vec![[0., 0.]; 64];
    }
    s.openings = vec![Opening { id: "o".into(), floor: FloorRef::Storey { level: 0 }, outline: vec![[0., 0.]; 64] }; 64];
    error(&s, &floors, "sequence vertices: count");
    let mut work = Work::new();
    work.take(geometry::MAX_WORK).unwrap();
    assert!(work.take(1).unwrap_err().to_string().contains("work limit exceeded"));
}
#[test]
fn duplicate_dangling_and_misordered_identities_refuse() {
    let (original, floors) = fixture(MULTI);
    let mut s = original.clone();
    s.landings[1].id = s.landings[0].id.clone();
    error(&s, &floors, "duplicate landing ID");
    let mut s = original.clone();
    s.flights[1].id = s.flights[0].id.clone();
    error(&s, &floors, "duplicate flight ID");
    let mut s = original.clone();
    s.openings[1].id = s.openings[0].id.clone();
    error(&s, &floors, "duplicate opening ID");
    let mut s = original.clone();
    s.landings[1].ports[1].id = "in".into();
    error(&s, &floors, "duplicate port ID");
    let mut s = original.clone();
    if let Profile::Straight { tread_ids } = &mut s.flights[0].profile {
        tread_ids[1] = tread_ids[0].clone();
    }
    error(&s, &floors, "duplicate tread ID");
    let mut s = original.clone();
    s.flights[0].from.port = "missing".into();
    error(&s, &floors, "missing port");
    let mut s = original.clone();
    s.flights[0].to.landing = "missing".into();
    error(&s, &floors, "path ordering");
    let mut s = original.clone();
    s.flights.swap(0, 1);
    error(&s, &floors, "path ordering");
    let mut s = original.clone();
    s.landings[1].ports.remove(1);
    error(&s, &floors, "unused or missing port");
    let mut s = original.clone();
    s.flights[1].from.port = "in".into();
    error(&s, &floors, "port used twice");
}
#[test]
fn floor_references_access_arrival_thickness_and_attic_are_explicit() {
    let (original, floors) = fixture(MULTI);
    error(&original, &floors[..2], "missing physical floor");
    let mut f = floors.clone();
    f.push(floors[0]);
    error(&original, &f, "duplicate physical floor datum");
    let mut s = original.clone();
    s.landings[2].access_level = Some(1);
    error(&s, &floors, "duplicate access level");
    let mut s = original.clone();
    s.landings[2].access_level = Some(0);
    error(&s, &floors, "access must match");
    let mut s = original.clone();
    s.landings[4].elevation.offset_mm = 0.0000001;
    error(&s, &floors, "same floor and zero offset");
    let mut s = original.clone();
    s.landings[1].surface = Surface::Slab { thickness_mm: 180. };
    error(&s, &floors, "arrival thickness");
    let mut s = original.clone();
    s.landings[2].elevation.offset_mm = -120.;
    error(&s, &floors, "flight total rise");
    let mut s = original.clone();
    s.landings[4].elevation.floor = FloorRef::Attic;
    s.landings[4].access_level = None;
    s.landings[4].surface = Surface::Floor { floor: FloorRef::Attic };
    error(&s, &floors, "missing physical floor Attic");
    let mut f = floors;
    f.push(PhysicalFloor { floor: FloorRef::Attic, top_z_mm: 3000., thickness_mm: 220. });
    let r = resolve_geometry(&s, &f).unwrap();
    assert_eq!(r.landings[4].datum.floor, FloorRef::Attic);
    assert!(r.landings[4].solid.is_none());
}
#[test]
fn requested_guards_are_refused_and_absence_is_not_safety_approval() {
    let (original, floors) = fixture(FAN);
    let r = resolve_geometry(&original, &floors).unwrap();
    assert_eq!(r.guard_safety, Assessment::NotAssessed);
    for left in [true, false] {
        let mut s = original.clone();
        let g = Some(Guard { id: "g".into(), height_mm: 1000., thickness_mm: 20. });
        if left {
            s.flights[0].guards.left = g;
        } else {
            s.flights[0].guards.right = g;
        }
        error(&s, &floors, "non-null guards are unsupported");
    }
    let mut s = original;
    s.landings[0].guards.push(LandingGuard { id: "g".into(), height_mm: 1000., thickness_mm: 20., edge: [[0., 0.], [1000., 0.]] });
    error(&s, &floors, "non-null guards are unsupported");
}
#[test]
fn oriented_gates_full_width_joins_and_boundary_tolerance_are_enforced() {
    let (original, floors) = fixture(FAN);
    let mut s = original.clone();
    s.landings[0].ports[0].edge.swap(0, 1);
    error(&s, &floors, "gate orientation");
    let mut s = original.clone();
    if let Profile::TreadChain { treads } = &mut s.flights[0].profile {
        treads[1].entry_edge = [1, 0];
    }
    error(&s, &floors, "oriented boundary edges");
    let mut s = original.clone();
    if let Profile::TreadChain { treads } = &mut s.flights[0].profile {
        treads[1].outline[1][1] += 0.01;
    }
    error(&s, &floors, "disconnected full-width gate");
    let mut s = original.clone();
    s.landings[0].ports[0].edge[1][0] = 500.;
    error(&s, &floors, "gate width");
    let mut s = original.clone();
    for p in &mut s.landings[0].ports[0].edge {
        p[1] += 2e-6;
    }
    error(&s, &floors, "one boundary edge");
    let (mut s, f) = fixture(MULTI);
    for p in &mut s.landings[1].outline {
        p[1] += 1.;
    }
    for p in &mut s.landings[1].ports {
        for q in &mut p.edge {
            q[1] += 1.;
        }
    }
    error(&s, &f, "equal parallel widths");
}
#[test]
fn simple_ccw_concave_rings_and_collinear_vertices_keep_real_material() {
    let (original, floors) = fixture(FAN);
    let mut s = original.clone();
    s.landings[0].outline.reverse();
    error(&s, &floors, "CCW");
    let mut s = original.clone();
    s.landings[0].outline.swap(1, 2);
    error(&s, &floors, "simple without crossing");
    let mut s = original.clone();
    s.landings[0].outline[1] = s.landings[0].outline[0];
    error(&s, &floors, "edge shorter");
    let mut s = original.clone();
    if let Profile::TreadChain { treads } = &mut s.flights[0].profile {
        treads[1].outline = vec![[0., 0.], [1000., 500.], [1000., 750.], [1000., 800.], [800., 800.], [800., 1000.], [500., 1000.]];
        treads[1].exit_edge = [0, 6];
    }
    let r = resolve_geometry(&s, &floors).unwrap();
    audit(&r.flights[0].treads[1], 460_000. * 330., 360., 30.);
    assert!(r.flights[0].treads[1].mesh.positions.contains(&[1000., 750., 360.]));
}
#[test]
fn concave_walk_checks_every_interval_and_overlap_checks_actual_interiors() {
    let r = vec![[0., 0.], [1000., 0.], [1000., 1000.], [600., 1000.], [600., 200.], [400., 200.], [400., 1000.], [0., 1000.]];
    let mut w = Work::new();
    ring(&r, &mut w).unwrap();
    assert!(!covers(&r, [[200., 500.], [800., 500.]], &mut w).unwrap());
    assert!(covers(&r, [[200., 100.], [800., 100.]], &mut w).unwrap());
    let square = vec![[0., 0.], [1000., 0.], [1000., 1000.], [0., 1000.]];
    let shifted = vec![[500., -100.], [1500., -100.], [1500., 1100.], [500., 1100.]];
    for other in [&square, &shifted, &r] {
        assert!(disjoint(&square, other, &mut w).unwrap_err().to_string().contains("interiors overlap"));
    }
    let notch = vec![[450., 300.], [550., 300.], [550., 900.], [450., 900.]];
    assert!(disjoint(&r, &notch, &mut w).unwrap().is_empty());
}
#[test]
fn arbitrary_nonadjacent_contact_is_not_misclassified_as_fan_pivot() {
    let (s, _) = fixture(FAN);
    let mut f = s.flights[0].clone();
    f.profile = Profile::TreadChain {
        treads: vec![
            Tread { id: "a".into(), outline: vec![[0., 0.], [1000., 0.], [1000., 1000.], [0., 1000.]], entry_edge: [0, 1], exit_edge: [2, 1] },
            Tread { id: "b".into(), outline: vec![[1000., 0.], [2000., 0.], [2000., 1000.], [1000., 1000.]], entry_edge: [3, 0], exit_edge: [3, 2] },
            Tread {
                id: "c".into(),
                outline: vec![[1000., 1000.], [2000., 1000.], [2000., 2000.], [1000., 2000.]],
                entry_edge: [0, 1],
                exit_edge: [3, 2],
            },
        ],
    };
    let e = cells(&f, [[0., 0.], [1000., 0.]], [[1000., 2000.], [2000., 2000.]], &mut Work::new()).err().unwrap().to_string();
    assert!(e.contains("not a shared inner pivot"), "{e}");
}
#[test]
fn openings_are_geometric_proposals_not_floor_application() {
    let (mut s, floors) = fixture(MULTI);
    s.openings[1].outline[1][0] = 12300.;
    s.openings[1].outline[2][0] = 12300.;
    let r = resolve_geometry(&s, &floors).unwrap();
    assert_eq!(r.proposed_openings[1].outline[1][0], 12300.);
    assert_eq!(r.retained_floor_support, Assessment::NotAssessed);
    s.openings[1].floor = FloorRef::Storey { level: 0 };
    s.openings[0].floor = FloorRef::Storey { level: 0 };
    error(&s, &floors, "interiors overlap");
    s.openings[1].floor = FloorRef::Storey { level: 59 };
    error(&s, &floors, "missing physical floor");
}
#[test]
fn legacy_document_and_stair_reader_still_refuse_sequence() {
    let (s, _) = fixture(FAN);
    let bytes = serde_json::to_vec(&s).unwrap();
    let e = serde_json::from_slice::<crate::Stair>(&bytes).unwrap_err().to_string();
    assert!(e.contains("explicit_sequence_v1") && e.contains("unknown variant"), "{e}");
    let legacy = r#"{"id":"old","levels":[0],"outline":[[0,0],[2700,0],[2700,1000],[0,1000]],"kind":"straight","tread":300}"#;
    let stair: crate::Stair = serde_json::from_str(legacy).unwrap();
    let before = serde_json::to_vec(&stair).unwrap();
    let symbol = super::super::symbol(&stair, 3000.).unwrap();
    let mesh = super::super::mesh(&stair, 0., 3000.).unwrap();
    resolve_geometry(&s, &fixture(FAN).1).unwrap();
    assert_eq!(before, serde_json::to_vec(&stair).unwrap());
    assert_eq!(symbol.treads, super::super::symbol(&stair, 3000.).unwrap().treads);
    assert_eq!(mesh, super::super::mesh(&stair, 0., 3000.).unwrap());
    let mut doc: Value = serde_json::from_str(include_str!("../../../tests/fixtures/courtyard.trigon")).unwrap();
    doc["buildings"] = json!([crate::Building::new("public", "Public", &[[0., 0.], [5000., 0.], [5000., 5000.], [0., 5000.]], 2, 3000.).unwrap()]);
    doc["buildings"][0]["stairs"] = json!([s]);
    let e = serde_json::from_value::<crate::Document>(doc).unwrap_err().to_string();
    assert!(e.contains("unknown field `flights`"), "{e}");
}

#[test]
fn rejected_oversized_id_does_not_expand_error_output() {
    let (mut s, floors) = fixture(FAN);
    s.id = "x".repeat(1_000_000);
    let e = resolve_geometry(&s, &floors).unwrap_err().to_string();
    assert!(e.contains("ID bytes"));
    assert!(e.len() < 1000, "invalid identifier was copied into diagnostic");
}
#[test]
fn real_polygon_work_exhaustion_refuses_without_partial_result() {
    let (mut s, mut floors) = fixture(FAN);
    for level in 0..16 {
        if level > 0 {
            floors.push(PhysicalFloor { floor: FloorRef::Storey { level }, top_z_mm: 3000. * level as f64, thickness_mm: 220. });
        }
        for strip in 0..4 {
            let corners = [[0., 0.], [10000., 0.], [10000., 100.], [0., 100.]];
            let mut outline = Vec::new();
            for (a, b) in corners.iter().zip(corners.iter().cycle().skip(1)) {
                for k in 0..16 {
                    let x = a[0] + (b[0] - a[0]) * k as f64 / 16.;
                    let y = a[1] + (b[1] - a[1]) * k as f64 / 16. + strip as f64 * 200.;
                    outline.push([10000. + (x - y) * std::f64::consts::FRAC_1_SQRT_2, 10000. + (x + y) * std::f64::consts::FRAC_1_SQRT_2]);
                }
            }
            s.openings.push(Opening { id: format!("cut-{level}-{strip}"), floor: FloorRef::Storey { level }, outline });
        }
    }
    error(&s, &floors, "geometry work limit exceeded; result unverified");
}

fn write_artifact(name: &str, source: &SequenceV1, floors: &[PhysicalFloor], result: &ResolvedSequence) {
    let Ok(path) = std::env::var("TRIGON_SEQUENCE_ARTIFACT_DIR") else {
        return;
    };
    let path = std::path::PathBuf::from(path);
    std::fs::create_dir_all(&path).unwrap();
    let parts:Vec<_>=all_solids(result).map(|p|json!({"id":p.id,"role":format!("{:?}",p.role),"outline":p.outline,"top_z_mm":p.top_z_mm,"bottom_z_mm":p.bottom_z_mm,"gross_volume_mm3":p.gross_volume_mm3,"positions":p.mesh.positions,"triangles":p.mesh.triangles})).collect();
    let datums: Vec<_> = floors.iter().map(|f| json!({"floor":f.floor,"top_z_mm":f.top_z_mm,"thickness_mm":f.thickness_mm})).collect();
    let artifact = json!({"provenance":"Actual Rust resolve_geometry output; public synthetic oracle; geometry only, no floor/support/building acceptance", "source":source,"physical_floors":datums,"parts":parts,"gross_volume_mm3":result.gross_volume_mm3});
    std::fs::write(path.join(format!("{name}.mesh.json")), serde_json::to_vec_pretty(&artifact).unwrap()).unwrap();
}

#[test]
fn tread_cannot_use_same_physical_gate_for_entry_and_exit() {
    let (s, _) = fixture(FAN);
    let mut f = s.flights[0].clone();
    f.profile = Profile::TreadChain {
        treads: vec![Tread {
            id: "turn-back".into(),
            outline: vec![[0., 0.], [1000., 0.], [1000., 1000.], [0., 1000.]],
            entry_edge: [0, 1],
            exit_edge: [1, 0],
        }],
    };
    let error = cells(&f, [[0., 0.], [1000., 0.]], [[1000., 0.], [0., 0.]], &mut Work::new()).err();
    assert!(error.is_some(), "one tread entered and left through the same physical gate");
    assert!(error.unwrap().to_string().contains("distinct entry and exit gates"));
}

#[test]
fn positive_owned_body_overlap_refuses_but_separated_elevations_do_not() {
    let (mut s, floors) = fixture(FAN);
    s.landings[1].outline = vec![[-1000., -1000.], [500., -1000.], [500., 0.], [0., 0.], [0., 1000.], [-1000., 1000.]];
    resolve_geometry(&s, &floors).unwrap(); // Overlapping XY, but disjoint Z intervals.
    s.landings[1].surface = Surface::Slab { thickness_mm: 1000. };
    error(&s, &floors, "parts public.winder.landing.low and public.winder.landing.high");
    error(&s, &floors, "polygon interiors overlap");
}
#[test]
fn nested_guard_opening_fields_and_all_remaining_collection_limits_are_strict() {
    let (s, _) = fixture(MULTI);
    let mut original = serde_json::to_value(s).unwrap();
    original["landings"][0]["guards"] = json!([{"id":"lg","edge":[[2000,3000],[3000,3000]],"height_mm":1000,"thickness_mm":20}]);
    original["flights"][0]["guards"]["left"] = json!({"id":"fg","height_mm":1000,"thickness_mm":20});
    for pointer in ["/openings/0", "/openings/0/floor", "/landings/0/guards/0", "/flights/0/guards/left"] {
        let mut v = original.clone();
        v.pointer_mut(pointer).unwrap()["mystery"] = json!(0);
        parse_error(&serde_json::to_vec(&v).unwrap(), "unknown field");
        let (key, value) = original.pointer(pointer).unwrap().as_object().unwrap().iter().next().unwrap();
        let text = serde_json::to_string(&v).unwrap().replace("\"mystery\":0", &format!("{}:{}", serde_json::to_string(key).unwrap(), value));
        parse_error(text.as_bytes(), "duplicate field");
    }
    for (pointer, max) in [("/openings", 64), ("/openings/0/outline", 64), ("/landings/0/guards", 16), ("/flights/0/profile/tread_ids", 80)] {
        let mut v = original.clone();
        let a = v.pointer_mut(pointer).unwrap().as_array_mut().unwrap();
        let element = a[0].clone();
        *a = vec![element; max + 1];
        parse_error(&serde_json::to_vec(&v).unwrap(), "collection exceeds its limit");
    }
    for pointer in ["/landings", "/flights", "/landings/0/outline", "/landings/0/ports", "/flights/0/profile/tread_ids"] {
        let mut v = original.clone();
        *v.pointer_mut(pointer).unwrap() = json!([]);
        parse_error(&serde_json::to_vec(&v).unwrap(), "collection below its minimum");
    }
}

#[test]
fn gate_tolerances_cannot_accumulate_into_disconnected_actual_bodies() {
    let (mut s, floors) = fixture(FAN);
    s.landings[1].outline = vec![[0., 1000.], [1000., 1000.], [1000., 2000.], [0., 2000.]];
    s.landings[1].ports[0].edge = [[0., 1000.], [1000., 1000.]];
    s.landings[1].elevation.offset_mm = 1400.;
    s.landings[1].surface = Surface::Slab { thickness_mm: 220. };
    s.flights[0].profile = Profile::TreadChain {
        treads: vec![Tread {
            id: "single".into(),
            outline: vec![[0., 0.], [1000., 0.], [1000., 1000.], [0., 1000.]],
            entry_edge: [0, 1],
            exit_edge: [3, 2],
        }],
    };
    resolve_geometry(&s, &floors).unwrap();
    for p in &mut s.landings[0].ports[0].edge {
        p[1] += 0.75e-6;
    }
    if let Profile::TreadChain { treads } = &mut s.flights[0].profile {
        treads[0].outline[0][1] += 1.5e-6;
        treads[0].outline[1][1] += 1.5e-6;
    }
    error(&s, &floors, "actual boundaries must share the full declared gate");
}
