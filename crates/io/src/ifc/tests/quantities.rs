use std::collections::{BTreeMap, BTreeSet};

use trigon_model::build::{BuiltElement, PhaseFilter, View, build_view};
use trigon_model::component::Side;
use trigon_model::{Built, ComponentKind, ComponentType, Document, ElementClass};

use crate::ifc::{fills, part21::gid, sample, write};

#[path = "gable_windows.rs"]
mod gable_windows;
#[path = "stepped_walls.rs"]
mod stepped_walls;

#[derive(Debug)]
struct Entity<'a> {
    kind: &'a str,
    attributes: Vec<&'a str>,
}

fn attributes(text: &str) -> Vec<&str> {
    let mut result = Vec::new();
    let mut start = 0;
    let mut depth = 0;
    let mut quoted = false;
    for (index, character) in text.char_indices() {
        match character {
            '\'' => quoted = !quoted,
            '(' if !quoted => depth += 1,
            ')' if !quoted => depth -= 1,
            ',' if !quoted && depth == 0 => {
                result.push(&text[start..index]);
                start = index + 1;
            }
            _ => {}
        }
    }
    assert!(!quoted && depth == 0, "unbalanced STEP attributes: {text}");
    result.push(&text[start..]);
    result
}

fn entities(text: &str) -> BTreeMap<usize, Entity<'_>> {
    let mut result = BTreeMap::new();
    for line in text.lines().filter(|line| line.starts_with('#')) {
        let (number, body) = line.split_once('=').unwrap();
        let number = reference(number);
        let (kind, body) = body.split_once('(').unwrap();
        let entity = Entity { kind, attributes: attributes(body.strip_suffix(");").unwrap()) };
        assert!(result.insert(number, entity).is_none(), "duplicate STEP id #{number}");
    }
    result
}

fn reference(text: &str) -> usize {
    text.strip_prefix('#').unwrap().parse().unwrap()
}

fn references(text: &str) -> Vec<usize> {
    let body = text.strip_prefix('(').unwrap().strip_suffix(')').unwrap();
    if body.is_empty() { Vec::new() } else { attributes(body).into_iter().map(reference).collect() }
}

fn filling_elements(built: &[Built]) -> impl Iterator<Item = &BuiltElement> {
    built.iter().flat_map(|building| {
        building
            .exterior_walls()
            .flat_map(|wall| &wall.openings)
            .chain(building.interior.iter().flat_map(|wall| &wall.doors))
            .map(|opening| &opening.fill)
    })
}

fn quantity_key(fill: &BuiltElement, occurrence: usize) -> String {
    format!("fill:{}:{}:{}:{}:{occurrence}", fill.id.len(), fill.id, fill.type_id.len(), fill.type_id)
}

fn quantity_guid(fill: &BuiltElement) -> String {
    gid(&format!("{}.qto", quantity_key(fill, 0)))
}

fn verify_quantities(document: &Document, built: &[Built], text: &str) -> BTreeMap<String, BTreeMap<String, f64>> {
    let records = entities(text);
    let mut expected = BTreeMap::<_, Vec<_>>::new();
    for fill in filling_elements(built)
        .filter(|fill| !matches!(document.component(&fill.type_id).ok().map(|component| &component.kind), Some(ComponentKind::Loggia { .. })))
    {
        expected.entry((gid(&fill.id), gid(&format!("type:{}", fill.type_id)))).or_default().push(fill);
    }
    let expected_count = expected.values().map(Vec::len).sum::<usize>();
    let mut seen_occurrences = BTreeMap::new();
    let project = records.values().find(|entity| entity.kind == "IFCPROJECT").unwrap();
    let units = &records[&reference(project.attributes[8])];
    assert_eq!(units.kind, "IFCUNITASSIGNMENT");
    let units: Vec<_> = references(units.attributes[0]).into_iter().map(|number| &records[&number]).collect();
    assert!(units.iter().any(|unit| unit.kind == "IFCSIUNIT" && unit.attributes == ["*", ".LENGTHUNIT.", ".MILLI.", ".METRE."]));
    assert!(units.iter().any(|unit| unit.kind == "IFCSIUNIT" && unit.attributes == ["*", ".AREAUNIT.", "$", ".SQUARE_METRE."]));
    let mut result = BTreeMap::new();
    let mut quantity_guids = BTreeSet::new();
    for (quantity_id, quantity_set) in records.iter().filter(|(_, entity)| {
        entity.kind == "IFCELEMENTQUANTITY" && matches!(entity.attributes[2], "'Qto_WindowBaseQuantities'" | "'Qto_DoorBaseQuantities'")
    }) {
        let relationships: Vec<_> =
            records.values().filter(|entity| entity.kind == "IFCRELDEFINESBYPROPERTIES" && reference(entity.attributes[5]) == *quantity_id).collect();
        assert_eq!(relationships.len(), 1, "one relationship per quantity set");
        let relationship = relationships[0];
        let occurrences = references(relationship.attributes[4]);
        assert_eq!(occurrences.len(), 1, "quantities belong to an occurrence, not a shared type");
        let occurrence_id = occurrences[0];
        let occurrence = &records[&occurrence_id];
        let typing: Vec<_> = records
            .values()
            .filter(|entity| entity.kind == "IFCRELDEFINESBYTYPE" && references(entity.attributes[4]).contains(&occurrence_id))
            .collect();
        assert_eq!(typing.len(), 1);
        let element_type = &records[&reference(typing[0].attributes[5])];
        let occurrence_key = (occurrence.attributes[0].to_string(), element_type.attributes[0].to_string());
        let occurrence_index = seen_occurrences.entry(occurrence_key.clone()).or_insert(0);
        let fill = expected
            .get(&occurrence_key)
            .and_then(|fills| fills.get(*occurrence_index))
            .expect("quantity set must target an exported rectangular fill");
        let quantity_key = quantity_key(fill, *occurrence_index);
        *occurrence_index += 1;
        let door = fill.class == ElementClass::Door;
        assert_eq!(occurrence.kind, if door { "IFCDOOR" } else { "IFCWINDOW" });
        assert_eq!(quantity_set.attributes[2], if door { "'Qto_DoorBaseQuantities'" } else { "'Qto_WindowBaseQuantities'" });
        assert_eq!(quantity_set.attributes[0], gid(&format!("{quantity_key}.qto")));
        assert_eq!(relationship.attributes[0], gid(&format!("{quantity_key}.qto.rel")));
        assert!(quantity_guids.insert(quantity_set.attributes[0]));
        assert!(quantity_guids.insert(relationship.attributes[0]));
        assert_eq!(occurrence.attributes[8].parse::<f64>().unwrap(), fill.height);
        assert_eq!(occurrence.attributes[9].parse::<f64>().unwrap(), fill.width);
        let mut quantities = BTreeMap::new();
        for number in references(quantity_set.attributes[5]) {
            let quantity = &records[&number];
            let (name, value, kind) = match quantity.attributes[0] {
                "'Height'" => ("Height", fill.height, "IFCQUANTITYLENGTH"),
                "'Width'" => ("Width", fill.width, "IFCQUANTITYLENGTH"),
                "'Area'" => (
                    "Area",
                    document
                        .component(&fill.type_id)
                        .map_or(fill.width * fill.height, |ty| trigon_model::profile::area(&ty.kind, fill.width, fill.height))
                        / 1_000_000.0,
                    "IFCQUANTITYAREA",
                ),
                other => panic!("unsupported fill quantity {other}"),
            };
            assert_eq!(quantity.kind, kind);
            assert_eq!(quantity.attributes.len(), 5);
            assert_eq!((quantity.attributes[1], quantity.attributes[2], quantity.attributes[4]), ("$", "$", "$"));
            let actual = quantity.attributes[3].parse::<f64>().unwrap();
            assert!((actual - value).abs() < 1e-9, "{} {name}: {actual} != {value}", fill.id);
            assert!(quantities.insert(name.to_string(), actual).is_none());
        }
        assert_eq!(quantities.len(), 3);
        assert!(result.insert(quantity_set.attributes[0].to_string(), quantities).is_none());
        assert_eq!(element_type.kind, if door { "IFCDOORTYPE" } else { "IFCWINDOWTYPE" });
        assert_eq!(element_type.attributes[0], gid(&format!("type:{}", fill.type_id)));
        let filling: Vec<_> =
            records.values().filter(|entity| entity.kind == "IFCRELFILLSELEMENT" && reference(entity.attributes[5]) == occurrence_id).collect();
        assert_eq!(filling.len(), 1);
        let opening = &records[&reference(filling[0].attributes[4])];
        assert_eq!(opening.kind, "IFCOPENINGELEMENT");
        let placement = &records[&reference(occurrence.attributes[5])];
        assert_eq!(placement.kind, "IFCLOCALPLACEMENT");
        assert_eq!(placement.attributes[0], opening.attributes[5]);
        let containment: Vec<_> = records
            .values()
            .filter(|entity| entity.kind == "IFCRELCONTAINEDINSPATIALSTRUCTURE" && references(entity.attributes[4]).contains(&occurrence_id))
            .collect();
        assert_eq!(containment.len(), 1);
        assert_eq!(records[&reference(containment[0].attributes[5])].kind, "IFCBUILDINGSTOREY");
    }
    assert_eq!(result.len(), expected_count, "one quantity set per exported rectangular fill");
    for (key, fills) in expected {
        assert_eq!(seen_occurrences.get(&key), Some(&fills.len()));
    }
    assert_eq!(text, write(document, built, "quantities.ifc", "2026-10-31T00:00:00Z"));
    result
}

fn export(document: &Document, built: &[Built]) -> String {
    write(document, built, "quantities.ifc", "2026-10-31T00:00:00Z")
}

#[test]
fn dormer_roof_and_cheeks_have_external_common_properties_and_zinc_materials() {
    let mut document = Document::default();
    let mut building =
        trigon_model::Building::new("dormer", "Synthetic", &[[0., 0.], [24000., 0.], [24000., 12000.], [0., 12000.]], 1, 3000.).unwrap();
    building.roof = serde_json::from_value(serde_json::json!({
        "kind":"pitched","pitch_deg":46,"thickness_mm":220,"overhang_mm":180,
        "dormers":[{"id":"test","facade":"dormer.f1","u_mm":12000,
            "window_type":"window.casement.2","width_mm":1100,"height_mm":1200}]
    }))
    .unwrap();
    document.buildings.push(building);
    let built = trigon_model::build(&document).unwrap();
    let text = export(&document, &built);
    let records = entities(&text);
    let dormers = &built[0].roof.as_ref().unwrap().dormers;
    assert_eq!(dormers.len(), 1);
    for (suffix, kind, pset_name) in [("roof", "IFCROOF", "'Pset_RoofCommon'"), ("cheeks", "IFCCOVERING", "'Pset_CoveringCommon'")] {
        let guid = gid(&format!("{}.{}", dormers[0].id, suffix));
        let (&id, product) = records.iter().find(|(_, entity)| entity.kind == kind && entity.attributes[0] == guid).unwrap();
        assert!(product.attributes[6].starts_with('#'), "the product must retain represented geometry");
        let materials: Vec<_> =
            records.values().filter(|e| e.kind == "IFCRELASSOCIATESMATERIAL" && references(e.attributes[4]).contains(&id)).collect();
        assert_eq!(materials.len(), 1, "each dormer product needs a semantic material association");
        let material = &records[&reference(materials[0].attributes[5])];
        assert_eq!(material.kind, "IFCMATERIAL");
        assert_eq!(material.attributes[0], super::super::part21::string(super::super::catalog::ZINC.name));
        let common = records
            .values()
            .filter(|e| e.kind == "IFCRELDEFINESBYPROPERTIES" && references(e.attributes[4]).contains(&id))
            .map(|e| &records[&reference(e.attributes[5])])
            .find(|e| e.kind == "IFCPROPERTYSET" && e.attributes[2] == pset_name)
            .expect("dormer product needs its class's common property set");
        assert!(
            references(common.attributes[4])
                .iter()
                .map(|id| &records[id])
                .any(|e| e.kind == "IFCPROPERTYSINGLEVALUE" && e.attributes[0] == "'IsExternal'" && e.attributes[2] == "IFCBOOLEAN(.T.)")
        );
    }
}

#[test]
fn fill_base_quantities_use_occurrence_sizes_and_project_units() {
    let mut document = sample::document().unwrap();
    let placement = document.buildings[0].facades[3].placements.iter_mut().find(|placement| placement.type_id == "window.fixed").unwrap();
    placement.width = Some(1250.0);
    placement.height = Some(1800.0);
    let built = trigon_model::build(&document).unwrap();
    let text = export(&document, &built);
    let quantities = verify_quantities(&document, &built, &text);
    let window = filling_elements(&built).find(|fill| fill.type_id == "window.fixed" && fill.width == 1250.0).unwrap();
    assert_eq!(quantities[&quantity_guid(window)], BTreeMap::from([("Area".into(), 2.25), ("Height".into(), 1800.0), ("Width".into(), 1250.0)]));
    assert_ne!(document.component(&window.type_id).unwrap().width, window.width);
    let door = filling_elements(&built).find(|fill| fill.type_id == "door.interior").unwrap();
    assert_eq!(quantities[&quantity_guid(door)], BTreeMap::from([("Area".into(), 1.89), ("Height".into(), 2100.0), ("Width".into(), 900.0)]));
    let window_quantity_guid = quantity_guid(window);
    let placement = document.buildings[0].facades[3].placements.iter_mut().find(|placement| placement.type_id == "window.fixed").unwrap();
    placement.width = Some(1200.0);
    let resized = trigon_model::build(&document).unwrap();
    let resized_quantities = verify_quantities(&document, &resized, &export(&document, &resized));
    assert_eq!(resized_quantities[&window_quantity_guid]["Area"], 2.16);
}

#[test]
fn fill_base_quantities_cover_rectangular_families_but_not_loggia_openings() {
    for kind in [
        ComponentKind::Window { lights: 1, top_light: 0.0 },
        ComponentKind::Window { lights: 2, top_light: 300.0 },
        ComponentKind::Window { lights: 3, top_light: 0.0 },
        ComponentKind::FixedWindow,
        ComponentKind::EntranceDoor,
        ComponentKind::FrenchDoor { leaves: 1 },
        ComponentKind::FrenchDoor { leaves: 2 },
        ComponentKind::FrenchDoor { leaves: 3 },
        ComponentKind::FrenchBalcony { leaves: 2, rail_height: 1000.0 },
        ComponentKind::Balcony { leaves: 2, depth: 1000.0, slab_width: 2000.0, rail_height: 1000.0 },
        ComponentKind::DoorWindow { door_width: 650.0, door_side: Side::Right, window_sill: 700.0, lights: 1 },
        ComponentKind::Loggia { depth: 1000.0, rail_height: 1000.0, door_width: 650.0, door_side: Side::Left },
    ] {
        let mut document = sample::document().unwrap();
        document.types.push(ComponentType {
            id: "quantity.family".into(),
            name: "Lining, 'test'".into(),
            width: 1500.0,
            height: 1900.0,
            sill: 0.0,
            kind: kind.clone(),
        });
        let placement = &mut document.buildings[0].facades[0].placements[0];
        placement.type_id = "quantity.family".into();
        placement.width = Some(1250.0);
        placement.height = Some(1800.0);
        let built = trigon_model::build(&document).unwrap();
        let text = export(&document, &built);
        let quantities = verify_quantities(&document, &built, &text);
        let fill = filling_elements(&built).find(|fill| fill.type_id == "quantity.family").unwrap();
        if matches!(kind, ComponentKind::Loggia { .. }) {
            assert!(!quantities.contains_key(&quantity_guid(fill)), "a facade hole is not the recessed lining");
        } else {
            assert_eq!(quantities[&quantity_guid(fill)]["Area"], 2.25, "{kind:?}");
        }
        let records = entities(&text);
        let element_type = records.values().find(|entity| entity.attributes[0] == gid("type:quantity.family")).unwrap();
        assert_eq!(element_type.attributes[10], if kind.is_door() { fills::door_operation(&kind) } else { fills::window_partitioning(&kind) });
    }
}

#[test]
fn fill_base_quantities_follow_phase_filters_without_orphans() {
    let mut exported_counts = BTreeSet::new();
    for same_type_replacement in [false, true] {
        let mut document = sample::document().unwrap();
        if same_type_replacement {
            for placement in &mut document.buildings[0].facades[0].placements {
                if placement.type_id == "door.french" && placement.created.is_some() {
                    placement.type_id = "window.casement.2".into();
                }
            }
        }
        for phase in 0..2 {
            for filter in [PhaseFilter::Complete, PhaseFilter::Demolition, PhaseFilter::New, PhaseFilter::All] {
                let built = build_view(&document, View { phase, filter }).unwrap();
                let quantities = verify_quantities(&document, &built, &export(&document, &built));
                exported_counts.insert(quantities.len());
            }
        }
    }
    assert!(exported_counts.len() >= 3, "the fixture exercises different visible sets");
    let empty = Document::default();
    assert!(verify_quantities(&empty, &[], &export(&empty, &[])).is_empty());
}

#[test]
fn curved_void_axes_and_wall_quantities_match_the_real_profiles() {
    let mut doc = Document::default();
    doc.types.extend(trigon_model::component::copenhagen_types());
    let mut b =
        trigon_model::Building::new("b", "Original curved-void fixture", &[[0., 0.], [12000., 0.], [12000., 8000.], [0., 8000.]], 1, 3500.).unwrap();
    b.facades[0].bays = vec![2500., 5500., 8500.];
    for (bay, ty) in ["dk.window.segmental", "dk.window.arched-fixed", "dk.window.round"].iter().enumerate() {
        b.facades[0].placements.push(serde_json::from_value(serde_json::json!({"level":0,"bay":bay,"type":ty})).unwrap());
    }
    doc.buildings.push(b);
    let built = trigon_model::build(&doc).unwrap();
    let text = export(&doc, &built);
    verify_quantities(&doc, &built, &text);
    let records = entities(&text);
    let vector =
        |id| attributes(records[&id].attributes[0].trim_matches(['(', ')'])).into_iter().map(|x| x.parse::<f64>().unwrap()).collect::<Vec<_>>();
    for opening in &built[0].walls[0].openings {
        let product = records.values().find(|e| e.kind == "IFCOPENINGELEMENT" && e.attributes[0] == gid(&opening.id)).unwrap();
        let product_shape = &records[&reference(product.attributes[6])];
        let shape = &records[&references(product_shape.attributes[2])[0]];
        let solid = &records[&references(shape.attributes[3])[0]];
        assert_eq!(solid.kind, "IFCEXTRUDEDAREASOLID");
        let position = &records[&reference(solid.attributes[1])];
        let origin = vector(reference(position.attributes[0]));
        let z = vector(reference(position.attributes[1]));
        let x = vector(reference(position.attributes[2]));
        let y = [z[1] * x[2] - z[2] * x[1], z[2] * x[0] - z[0] * x[2], z[0] * x[1] - z[1] * x[0]];
        let profile = &records[&reference(solid.attributes[0])];
        assert_eq!(profile.kind, "IFCARBITRARYCLOSEDPROFILEDEF");
        let line = &records[&reference(profile.attributes[2])];
        let heights: Vec<_> = references(line.attributes[0])
            .into_iter()
            .map(|id| {
                let p = vector(id);
                origin[2] + x[2] * p[0] + y[2] * p[1]
            })
            .collect();
        let lo = heights.iter().copied().fold(f64::INFINITY, f64::min);
        let hi = heights.iter().copied().fold(f64::NEG_INFINITY, f64::max);
        assert!(lo.abs() < 1e-6 && (hi - opening.fill.height).abs() < 1e-6, "void {} must rise from its bottom, got {lo}..{hi}", opening.id);
    }
    for wall in &built[0].walls {
        for seg in &wall.segments {
            let mesh_volume = seg
                .mesh
                .triangles
                .iter()
                .map(|t| {
                    let [a, b, c] = t.map(|i| seg.mesh.positions[i as usize]);
                    (a[0] * (b[1] * c[2] - b[2] * c[1]) - a[1] * (b[0] * c[2] - b[2] * c[0]) + a[2] * (b[0] * c[1] - b[1] * c[0])) / 6.0
                })
                .sum::<f64>();
            let quantity = crate::ifc::walls::net_volume(wall, seg, trigon_model::doc::polygon_area(&seg.plan));
            // The wall boolean uses a quantised 2D overlay; 1 mm³ is the existing wall-volume test tolerance.
            assert!((mesh_volume - quantity).abs() < 1.0, "curved-void wall quantity {quantity} vs mesh {mesh_volume}");
        }
    }
}

#[test]
fn multi_plane_dormer_front_exports_its_real_underside_and_net_volume() {
    let mut doc = Document::default();
    let mut b =
        trigon_model::Building::new("b", "Synthetic chamfer", &[[0., 0.], [12000., 0.], [12000., 8000.], [10000., 10000.], [0., 10000.]], 3, 3400.)
            .unwrap();
    b.roof = serde_json::from_value(serde_json::json!({"kind":"pitched","pitch_deg":46,"thickness_mm":200,"overhang_mm":180,"straighten_mm":0,
        "dormers":[{"id":"corner","facade":"b.f3","u_mm":1414.213562373095,"window_type":"window.casement.2",
        "width_mm":1400,"height_mm":1000,"setback_mm":350,"multi_plane":true}]}))
    .unwrap();
    doc.buildings.push(b);
    let built = trigon_model::build(&doc).unwrap();
    let text = export(&doc, &built);
    let records = entities(&text);
    let roof = built[0].roof.as_ref().unwrap();
    let source_volume = crate::ifc::part21::volume(&roof.cover);
    let exported_volume: f64 = crate::ifc::model_shapes(&doc, &built)
        .iter()
        .filter(|(key, _)| key == &roof.id || key.starts_with(&format!("{}.face", roof.id)))
        .map(|(_, m)| crate::ifc::part21::volume(m))
        .sum();
    assert!(
        (exported_volume - source_volume).abs() < source_volume * 0.001,
        "multi-plane host shell volume changed: {source_volume} -> {exported_volume} mm³"
    );
    for read in [trigon_ifcread::read_ifc_lite, trigon_ifcread::read_bimifc] {
        let result = read(&text).unwrap();
        let gid = crate::ifc::guid(&format!("trigon:{}", roof.id));
        let e = result.elements.iter().find(|e| e.global_id == gid).unwrap();
        let mesh = trigon_model::Mesh { positions: e.mesh.positions.clone(), triangles: e.mesh.triangles.clone() };
        let volume = crate::ifc::part21::volume(&mesh);
        assert!((volume - source_volume).abs() < source_volume * 1e-4, "reader host shell volume changed: {source_volume} -> {volume} mm³");
    }
    let front = &built[0].roof.as_ref().unwrap().dormers[0].front;
    let seg = &front.segments[0];
    let (&id, wall) = records.iter().find(|(_, e)| e.kind == "IFCWALL" && e.attributes[0] == gid(&seg.id)).unwrap();
    let shape = &records[&reference(wall.attributes[6])];
    let body = &records[&references(shape.attributes[2])[1]];
    assert_eq!(body.attributes[2], "'Tessellation'");
    let qset = records
        .values()
        .filter(|e| e.kind == "IFCRELDEFINESBYPROPERTIES" && references(e.attributes[4]).contains(&id))
        .map(|e| &records[&reference(e.attributes[5])])
        .find(|e| e.kind == "IFCELEMENTQUANTITY")
        .unwrap();
    let quantity = references(qset.attributes[5]).into_iter().map(|k| &records[&k]).find(|e| e.attributes[0] == "'NetVolume'").unwrap();
    assert!((quantity.attributes[3].parse::<f64>().unwrap() - crate::ifc::part21::volume(&seg.mesh) / 1e9).abs() < 1e-8);
}

#[test]
fn rooflights_export_window_opening_host_identities_and_inclined_quantities() {
    let mut doc = Document::default();
    let mut b = trigon_model::Building::new("b", "Inclined windows", &[[0., 0.], [24000., 0.], [24000., 12000.], [0., 12000.]], 3, 3400.).unwrap();
    b.roof=serde_json::from_value(serde_json::json!({"kind":"pitched","pitch_deg":46,"thickness_mm":220,
        "rooflights":[{"id":"light","facade":"b.f1","u_mm":10000,"run_mm":500,"window_type":"window.casement.2","width_mm":800,"height_mm":1000,"stand_off_mm":100,"flashing_mm":100}]})).unwrap();
    let existing_type_keys = ["rooflight:window.casement.2", "rooflight:17:window.casement.2"];
    b.facades[0].bays = vec![5000., 7000.];
    for (bay, id) in existing_type_keys.into_iter().enumerate() {
        doc.types.push(ComponentType {
            id: id.into(),
            name: "Existing facade door namespace fixture".into(),
            width: 900.,
            height: 2100.,
            sill: 0.,
            kind: ComponentKind::PanelDoor { leaves: 1, panel_columns: 1, panel_rows: 2, glazed_rows: 0 },
        });
        b.facades[0].placements.push(serde_json::from_value(serde_json::json!({"level":0,"bay":bay,"type":id})).unwrap());
    }
    doc.buildings.push(b);
    let built = trigon_model::build(&doc).unwrap();
    let text = export(&doc, &built);
    let records = entities(&text);
    let light = &built[0].roof.as_ref().unwrap().rooflights[0];
    let (&window_id, window) = records.iter().find(|(_, e)| e.kind == "IFCWINDOW" && e.attributes[0] == gid(&light.id)).unwrap();
    assert_eq!(window.attributes[10], ".SKYLIGHT.");
    assert_eq!(window.attributes[8].parse::<f64>().unwrap(), 1000.);
    assert_eq!(window.attributes[9].parse::<f64>().unwrap(), 800.);
    let (&opening_id, _) = records.iter().find(|(_, e)| e.kind == "IFCOPENINGELEMENT" && e.attributes[0] == gid(&light.opening_id)).unwrap();
    let (&roof_id, _) = records.iter().find(|(_, e)| e.kind == "IFCROOF" && e.attributes[0] == gid("b.roof")).unwrap();
    assert!(
        records.values().any(|e| e.kind == "IFCRELVOIDSELEMENT" && reference(e.attributes[4]) == roof_id && reference(e.attributes[5]) == opening_id)
    );
    assert!(
        records
            .values()
            .any(|e| e.kind == "IFCRELFILLSELEMENT" && reference(e.attributes[4]) == opening_id && reference(e.attributes[5]) == window_id)
    );
    assert!(records.values().any(|e| e.kind == "IFCWINDOWTYPE" && e.attributes[9] == ".SKYLIGHT."));
    let qset = records
        .values()
        .filter(|e| e.kind == "IFCRELDEFINESBYPROPERTIES" && references(e.attributes[4]).contains(&window_id))
        .map(|e| &records[&reference(e.attributes[5])])
        .find(|e| e.kind == "IFCELEMENTQUANTITY")
        .unwrap();
    let area = references(qset.attributes[5]).into_iter().map(|k| &records[&k]).find(|e| e.attributes[0] == "'Area'").unwrap();
    assert_eq!(area.attributes[3].parse::<f64>().unwrap(), 0.8);
    let bytes = crate::glb::write(&built, &[]);
    let length = u32::from_le_bytes(bytes[12..16].try_into().unwrap()) as usize;
    let glb: serde_json::Value = serde_json::from_slice(&bytes[20..20 + length]).unwrap();
    let nodes = glb["nodes"].as_array().unwrap();
    let window = nodes.iter().find(|n| n["name"] == light.id).unwrap();
    assert_eq!(window["extras"]["ifc_class"], "IfcWindow");
    assert_eq!(window["extras"]["host_roof"], "b.roof");
    assert_eq!(window["extras"]["quantities"]["opening_area_m2"], 0.8);
    let opening = nodes.iter().find(|n| n["name"] == light.opening_id).unwrap();
    assert_eq!(opening["extras"]["fill"], light.id);
    for read in [trigon_ifcread::read_ifc_lite, trigon_ifcread::read_bimifc] {
        let result = read(&text).unwrap();
        for key in [&light.id, &format!("{}.flashing", light.id)] {
            let guid = crate::ifc::guid(&format!("trigon:{key}"));
            let element = result.elements.iter().find(|e| e.global_id == guid).unwrap();
            assert!(!element.mesh.triangles.is_empty(), "reader must build the actual positive product mesh");
        }
        let roof_guids: BTreeSet<_> = crate::ifc::model_shapes(&doc, &built)
            .iter()
            .filter(|(key, _)| key == "b.roof" || key.starts_with("b.roof."))
            .map(|(key, _)| crate::ifc::guid(&format!("trigon:{key}")))
            .collect();
        let roofs: Vec<_> = result.elements.iter().filter(|e| roof_guids.contains(&e.global_id)).collect();
        assert!(!roofs.is_empty());
        let hits = |q: [f64; 2]| {
            roofs
                .iter()
                .map(|e| {
                    e.mesh
                        .triangles
                        .iter()
                        .filter(|t| {
                            let p = t.map(|i| light.frame.local(e.mesh.positions[i as usize]));
                            let det = (p[1][0] - p[0][0]) * (p[2][1] - p[0][1]) - (p[1][1] - p[0][1]) * (p[2][0] - p[0][0]);
                            if det.abs() < 1e-8 {
                                return false;
                            }
                            let s = ((q[0] - p[0][0]) * (p[2][1] - p[0][1]) - (q[1] - p[0][1]) * (p[2][0] - p[0][0])) / det;
                            let t = ((p[1][0] - p[0][0]) * (q[1] - p[0][1]) - (p[1][1] - p[0][1]) * (q[0] - p[0][0])) / det;
                            s >= 0. && t >= 0. && s + t <= 1.
                        })
                        .count()
                })
                .sum::<usize>()
        };
        for q in [[400., 2.], [400., 500.], [400., 998.]] {
            assert_eq!(hits(q), 0, "the exported roof must retain its true normal reveal at {q:?}");
        }
        assert!(hits([-2., 500.]) > 0, "roof next to the cut must remain present");
        let q = [-2., 500.];
        let mut depths = Vec::new();
        for roof in &roofs {
            for t in &roof.mesh.triangles {
                let p = t.map(|i| light.frame.local(roof.mesh.positions[i as usize]));
                let det = (p[1][0] - p[0][0]) * (p[2][1] - p[0][1]) - (p[1][1] - p[0][1]) * (p[2][0] - p[0][0]);
                if det.abs() < 1e-8 {
                    continue;
                }
                let s = ((q[0] - p[0][0]) * (p[2][1] - p[0][1]) - (q[1] - p[0][1]) * (p[2][0] - p[0][0])) / det;
                let t = ((p[1][0] - p[0][0]) * (q[1] - p[0][1]) - (p[1][1] - p[0][1]) * (q[0] - p[0][0])) / det;
                if s >= 0. && t >= 0. && s + t <= 1. {
                    depths.push(p[0][2] + s * (p[1][2] - p[0][2]) + t * (p[2][2] - p[0][2]));
                }
            }
        }
        depths.sort_by(f64::total_cmp);
        depths.dedup_by(|a, b| (*a - *b).abs() < 0.01);
        assert_eq!(depths.len(), 2, "exactly two roof skins at the adjacent normal probe: {depths:?}");
        assert!((depths[0] + 220.).abs() < 0.01 && depths[1].abs() < 0.01, "normal roof build-up must remain 220 mm, got {depths:?}");
    }
}
