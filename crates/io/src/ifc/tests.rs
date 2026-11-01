use std::collections::BTreeSet;

use super::*;
use trigon_model::build::{PhaseFilter, View, build_view};
use trigon_model::{Room, Stair};

mod quantities;

fn sample() -> (Document, Vec<Built>, String) {
    let doc = sample::document().unwrap();
    let built = trigon_model::build(&doc).unwrap();
    let text = write(&doc, &built, "sample.ifc", "2026-01-01T00:00:00");
    (doc, built, text)
}

fn count(text: &str, entity: &str) -> usize {
    text.matches(&format!("={entity}(")).count()
}

#[test]
fn painted_balcony_metal_is_classified_as_steel_instead_of_timber() {
    let mut doc = sample::document().unwrap();
    for ty in &mut doc.types {
        if let trigon_model::ComponentKind::SteelBalcony { finish, .. } = &mut ty.kind {
            *finish = trigon_model::component::Finish::White;
        }
    }
    let built = trigon_model::build(&doc).unwrap();
    let white_metals: Vec<_> = built
        .iter()
        .flat_map(|b| &b.walls)
        .flat_map(|w| &w.attachments)
        .filter(|e| matches!(doc.component(&e.type_id).unwrap().kind, trigon_model::ComponentKind::SteelBalcony { .. }))
        .flat_map(|e| &e.parts)
        .filter(|(m, _)| m.rgba() == trigon_model::Material::Frame.rgba())
        .collect();
    assert!(!white_metals.is_empty());
    assert!(white_metals.iter().all(|(m, _)| catalog::of(*m).category == "steel"));
}

#[test]
fn detailed_glazing_seals_have_rubber_material_semantics() {
    let doc = sample::document().unwrap();
    let built = trigon_model::build(&doc).unwrap();
    let seals: Vec<_> = built
        .iter()
        .flat_map(|b| &b.walls)
        .flat_map(|w| &w.openings)
        .flat_map(|o| &o.fill.parts)
        .filter(|(m, _)| m.rgba() == trigon_model::Material::Railing.rgba())
        .collect();
    assert!(!seals.is_empty());
    assert!(seals.iter().all(|(m, _)| catalog::of(*m).category == "rubber"));
}

#[test]
fn opening_material_constituents_are_exported_and_linked_to_named_shape_aspects() {
    let (_, _, text) = sample();
    assert!(text.contains(&format!("IFCMATERIAL({},$,'rubber')", string(catalog::GASKET.name))));
    assert!(count(&text, "IFCMATERIALCONSTITUENTSET") > 0);
    assert!(text.contains("IFCMATERIALCONSTITUENT('Gasket',$,"));
    assert!(text.lines().any(|line| line.contains("IFCSHAPEASPECT(") && line.contains(",'Gasket',$,.T.,")));
}

#[test]
fn independent_guard_exports_as_one_ifc_railing_without_an_assembly_deck_or_opening() {
    let mut doc = Document::default();
    let mut building =
        trigon_model::Building::new("guard-b", "Guard fixture", &[[0., 0.], [9000., 0.], [9000., 6000.], [0., 6000.]], 1, 3000.).unwrap();
    building.facades[0].bays = vec![4500.];
    building.facades[0].placements = serde_json::from_value(serde_json::json!([{"level":0,"bay":0,"type":"guard"}])).unwrap();
    doc.buildings.push(building);
    doc.types.push(
        serde_json::from_value(serde_json::json!({
            "id":"guard","name":"Independent guard","width":1500,"height":1100,"sill":200,
            "kind":{"family":"guard_rail","projection":150,"railing":"norrebro","finish":"white","handrail":true}
        }))
        .unwrap(),
    );
    let built = trigon_model::build(&doc).unwrap();
    let text = write(&doc, &built, "guard.ifc", "2026-01-01T00:00:00");
    assert_eq!(count(&text, "IFCRAILING"), 1);
    for entity in ["IFCELEMENTASSEMBLY", "IFCOPENINGELEMENT", "IFCWINDOW", "IFCDOOR", "IFCBEAM"] {
        assert_eq!(count(&text, entity), 0, "{entity}");
    }
    assert_eq!(count(&text, "IFCSLAB"), built[0].slabs.len());
    let guard = &built[0].walls[0].attachments[0];
    let mut mesh = trigon_model::Mesh::default();
    for (_, part) in &guard.parts {
        mesh.append(part);
    }
    let expected = mesh.bounds().unwrap();
    for read in [trigon_ifcread::read_ifc_lite(&text).unwrap(), trigon_ifcread::read_bimifc(&text).unwrap()] {
        let actual = read.elements.iter().find(|e| e.global_id == guid(&format!("trigon:{}", guard.id))).unwrap();
        assert!(trigon_ifcread::bounds_deviation(expected, actual.mesh.bounds().unwrap()) < 0.01, "{}", read.reader.name());
    }
}

#[test]
fn the_sample_has_every_kind_of_building_element() {
    let (doc, built, text) = sample();
    let b = &built[0];
    let pieces: usize = b.walls.iter().map(|w| w.segments.len()).sum();
    // Every facade piece and interior wall is a wall; the two gables are walls too.
    assert_eq!(count(&text, "IFCWALLSTANDARDCASE") + count(&text, "IFCWALL"), pieces + b.interior.len() + 2);
    assert_eq!(count(&text, "IFCWALL"), 2, "only the gables are plain walls");
    assert_eq!(count(&text, "IFCWALLTYPE"), 8, "480 and 360 facades, rendered too, the 360 fire gable, 240 and 120 masonry, 100 partitions");
    assert_eq!(count(&text, "IFCROOF"), 1);
    // Two storeys of a U-turn stair: two flights and a landing each.
    assert_eq!((count(&text, "IFCSTAIR"), count(&text, "IFCSTAIRFLIGHT")), (2, 4));
    assert_eq!(text.matches(".LANDING.)").count(), 2);
    let spaces: usize = doc.buildings[0].rooms.iter().map(|r| r.levels.len()).sum();
    assert_eq!(count(&text, "IFCSPACE"), spaces);
    // Two steel balconies (four UPE beams and two tie rods each) and one cantilever balcony.
    assert_eq!((count(&text, "IFCELEMENTASSEMBLY"), count(&text, "IFCBEAM"), count(&text, "IFCMEMBER")), (3, 8, 4));
    // Railings: three balconies, the French balcony and the balustrade of each U-turn stair.
    assert_eq!(count(&text, "IFCRAILING"), 4 + 2);
    let fills: usize = b.walls.iter().map(|w| w.openings.len()).sum();
    let doors: usize = b.interior.iter().map(|w| w.doors.len()).sum();
    assert_eq!(doors, 3 * 2 + 3 + 2, "two in the spine, one in the stairwell wall, one in the partition, per storey");
    assert_eq!(count(&text, "IFCWINDOW") + count(&text, "IFCDOOR"), fills + doors);
    assert_eq!(text.matches("IFCRELFILLSELEMENT(").count(), fills + doors);
    // The stairwell windows cross a floor: they void two pieces each.
    assert_eq!(count(&text, "IFCOPENINGELEMENT"), fills + 2 + doors);
    // A plinth on every facade and the street front's bands, a gutter on every eave, roof planes
    // with the roof's build-up, the rendered courtyard side as its own wall type.
    assert_eq!(count(&text, "IFCCOVERING"), b.walls.len() + 1);
    assert_eq!(text.matches(".MOLDING.)").count(), 1);
    assert!(text.contains("'Yderv\\X2\\00E6\\X0\\g, pudset mur 1\\X2\\00BD\\X0\\ sten, 360 mm'"));
    assert_eq!(count(&text, "IFCPIPESEGMENT"), 4);
    assert!(text.contains("'Tag, tegl p\\X2\\00E5\\X0\\ sp\\X2\\00E6\\X0\\r 250 mm'"));
    for layer in [
        catalog::L_FACADE,
        catalog::L_INTERIOR,
        catalog::L_FLOOR,
        catalog::L_ROOF,
        catalog::L_WINDOW,
        catalog::L_DOOR,
        catalog::L_STAIR,
        catalog::L_SPACE,
        catalog::L_BALCONY,
    ] {
        assert!(text.contains(&format!("IFCPRESENTATIONLAYERASSIGNMENT({},", string(layer))), "{layer}");
    }
    assert!(text.contains("'Qto_WallBaseQuantities'") && text.contains("'Qto_SpaceBaseQuantities'") && text.contains("'Pset_StairCommon'"));
    assert!(text.contains("IFCLABEL('NEW')") && text.contains("IFCLABEL('EXISTING')"));
}

#[test]
fn global_ids_are_unique_and_stable() {
    let (doc, built, text) = sample();
    let mut seen = BTreeSet::new();
    for line in text.lines() {
        let Some((_, rest)) = line.split_once('=') else { continue };
        let Some((_, args)) = rest.split_once("('") else { continue };
        if args.len() > 23 && args.as_bytes()[22] == b'\'' && rest.starts_with("IFC") && !rest.starts_with("IFCCARTESIAN") {
            let id = &args[..22];
            if id.bytes().all(|c| c.is_ascii_alphanumeric() || c == b'_' || c == b'$') {
                assert!(seen.insert(id.to_string()), "duplicate GlobalId in {line}");
            }
        }
    }
    assert!(seen.len() > 500);
    assert_eq!(text, write(&doc, &built, "sample.ifc", "2026-01-01T00:00:00"), "the export is deterministic");
    let seg = &built[0].walls[0].segments[0];
    assert!(text.contains(&format!("IFCWALLSTANDARDCASE({}", string(&guid(&format!("trigon:{}", seg.id))))));
}

#[test]
fn a_piece_s_net_volume_is_the_volume_of_trigon_s_closed_piece() {
    let (_, built, _) = sample();
    let mut checked = 0;
    for w in &built[0].walls {
        for seg in &w.segments {
            let net = walls::net_volume(w, seg, part21::area(&seg.plan));
            let mesh = part21::volume(&seg.mesh);
            assert!((net - mesh).abs() < 1e-6 * mesh, "{}: {net} vs {mesh}", seg.id);
            checked += 1;
        }
    }
    assert_eq!(checked, 18);
}

#[test]
fn every_view_and_an_empty_document_export() {
    let (doc, _, _) = sample();
    for phase in 0..2 {
        for filter in [PhaseFilter::Complete, PhaseFilter::Demolition, PhaseFilter::New, PhaseFilter::All] {
            let built = build_view(&doc, View { phase, filter }).unwrap();
            let text = write(&doc, &built, "v.ifc", "t");
            assert!(text.ends_with("END-ISO-10303-21;\n"));
        }
    }
    let empty = Document::default();
    assert!(write(&empty, &[], "e.ifc", "t").contains("IFCPROJECT("));
}

#[test]
fn hostile_rooms_and_stairs_are_skipped_not_fatal() {
    let (mut doc, built, _) = sample();
    let b = &mut doc.buildings[0];
    b.rooms.push(Room { id: String::new(), levels: vec![0, 99], name: "x".into(), polygon: vec![[f64::NAN, 0.0], [1.0, 1.0]] });
    b.rooms.push(Room { id: "flat".into(), levels: vec![1], name: String::new(), polygon: vec![[0.0, 0.0], [1e9, 0.0], [2e9, 0.0]] });
    b.stairs.push(Stair { outline: vec![[0.0, 0.0]], ..b.stairs[0].clone() });
    let text = write(&doc, &built, "h.ifc", "t");
    assert_eq!(count(&text, "IFCSPACE"), 15 - 1, "the broken rooms are left out");
}

#[test]
fn a_built_up_roof_is_one_slab_per_plane_as_thick_as_its_build_up() {
    let (_, built, text) = sample();
    let r = built[0].roof.as_ref().unwrap();
    let pieces = split::roof(r, &built[0].walls);
    let planes: Vec<_> = pieces.iter().filter(|p| p.gable.is_none()).collect();
    // An L with two gables: four planes, each its soffit raised straight up to the top face.
    assert_eq!(planes.len(), 4);
    for p in &planes {
        assert!((p.thickness() - 250.0).abs() < 1e-6, "{}: {}", p.key, p.thickness());
        assert!(p.rise[0].abs() < 1e-9 && p.rise[1].abs() < 1e-9);
    }
    assert_eq!(text.lines().filter(|l| l.contains("=IFCSLAB(") && l.ends_with(".ROOF.);")).count(), 4);
    // Without a build-up the roof is a surface; its planes hang square below it.
    let mut doc = sample::document().unwrap();
    if let trigon_model::Roof::Pitched { thickness_mm, overhang_mm, verge_mm, gutters, .. } = &mut doc.buildings[0].roof {
        (*thickness_mm, *overhang_mm, *verge_mm, *gutters) = (0.0, 0.0, 0.0, false);
    }
    let bare = trigon_model::build(&doc).unwrap();
    let r = bare[0].roof.as_ref().unwrap();
    let planes: Vec<_> = split::roof(r, &bare[0].walls).into_iter().filter(|p| p.gable.is_none()).collect();
    assert_eq!(planes.len(), 4);
    assert!(planes.iter().all(|p| (p.thickness() - split::ROOF_PLANE).abs() < 1e-6));
}

#[test]
fn explicit_drainage_reads_back_with_stable_ids_and_correct_pipe_classes() {
    use trigon_model::roof::drainage::{Downpipe, Drainage};
    let mut doc = sample::document().unwrap();
    let facade = doc.buildings[0].facades[0].id.clone();
    if let trigon_model::Roof::Pitched { drainage, gutters, .. } = &mut doc.buildings[0].roof {
        *gutters = true;
        *drainage = Some(Drainage {
            downpipes: vec![Downpipe {
                id: "rain-a".into(),
                facade,
                along_mm: 3000.0,
                diameter_mm: 80.0,
                wall_offset_mm: 100.0,
                bottom_mm: 150.0,
                shoe_mm: 200.0,
            }],
            ..Default::default()
        });
    }
    let built = trigon_model::build(&doc).unwrap();
    let text = write(&doc, &built, "drainage.ifc", "2026-01-01T00:00:00");
    assert!(text.contains(".RIGIDSEGMENT.)"));
    assert!(text.contains(".GUTTER.)"));
    let shapes = model_shapes(&doc, &built);
    for read in [trigon_ifcread::read_ifc_lite(&text).unwrap(), trigon_ifcread::read_bimifc(&text).unwrap()] {
        assert!(read.failures.is_empty(), "{:?}", read.failures);
        for (id, mesh) in shapes.iter().filter(|(id, _)| id.contains(".gutter") || id.contains(".downpipe.")) {
            let guid = guid(&format!("trigon:{id}"));
            let e = read.elements.iter().find(|e| e.global_id == guid).unwrap();
            assert!(e.class.eq_ignore_ascii_case("IfcPipeSegment"));
            assert!(trigon_ifcread::bounds_deviation(mesh.bounds().unwrap(), e.mesh.bounds().unwrap()) < 0.01);
        }
    }
}
