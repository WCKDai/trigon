use std::collections::BTreeSet;

use super::*;
use trigon_model::build::{PhaseFilter, View, build_view};
use trigon_model::{Room, Stair};

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
fn the_sample_has_every_kind_of_building_element() {
    let (doc, built, text) = sample();
    let b = &built[0];
    let pieces: usize = b.walls.iter().map(|w| w.segments.len()).sum();
    // Every facade piece and interior wall is a wall; the two gables are walls too.
    assert_eq!(count(&text, "IFCWALLSTANDARDCASE") + count(&text, "IFCWALL"), pieces + b.interior.len() + 2);
    assert_eq!(count(&text, "IFCWALL"), 2, "only the gables are plain walls");
    assert_eq!(count(&text, "IFCWALLTYPE"), 6, "480 and 360 facades, the 360 fire gable, 240 and 120 masonry, 100 partitions");
    assert_eq!(count(&text, "IFCROOF"), 1);
    // Two storeys of a U-turn stair: two flights and a landing each.
    assert_eq!((count(&text, "IFCSTAIR"), count(&text, "IFCSTAIRFLIGHT")), (2, 4));
    assert_eq!(text.matches(".LANDING.)").count(), 2);
    let spaces: usize = doc.buildings[0].rooms.iter().map(|r| r.levels.len()).sum();
    assert_eq!(count(&text, "IFCSPACE"), spaces);
    // Two steel balconies (four UPE beams and two tie rods each) and one cantilever balcony.
    assert_eq!((count(&text, "IFCELEMENTASSEMBLY"), count(&text, "IFCBEAM"), count(&text, "IFCMEMBER")), (3, 8, 4));
    // Railings: three balconies and the French balcony.
    assert_eq!(count(&text, "IFCRAILING"), 4);
    let fills: usize = b.walls.iter().map(|w| w.openings.len()).sum();
    assert_eq!(count(&text, "IFCWINDOW") + count(&text, "IFCDOOR"), fills);
    assert_eq!(text.matches("IFCRELFILLSELEMENT(").count(), fills);
    // The stairwell windows cross a floor: they void two pieces each.
    assert_eq!(count(&text, "IFCOPENINGELEMENT"), fills + 2);
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
