use std::collections::BTreeMap;

use trigon_model::{Document, Mesh};

use super::*;
use crate::ifc::{guid, part21, write};

#[test]
fn clipped_gables_keep_one_complete_product_per_source_facade() {
    for rotation in [0.0_f64, 31.] {
        for caps in [
            serde_json::json!([{"facade":"b.f4","drop_mm":1200}]),
            serde_json::json!([{"facade":"b.f2","drop_mm":900},{"facade":"b.f4","drop_mm":1200}]),
        ] {
            let (sin, cos) = rotation.to_radians().sin_cos();
            let outline =
                [[0., 0.], [30000., 0.], [30000., 12000.], [0., 12000.]].map(|[x, y]| [142000. + x * cos - y * sin, -270000. + x * sin + y * cos]);
            let mut doc = Document::default();
            let mut b = trigon_model::Building::new("b", "Public connected gable", &outline, 3, 3000.).unwrap();
            b.roof=serde_json::from_value(serde_json::json!({"kind":"half_hipped","pitch_deg":45,"gables":["b.f2","b.f4"],"half_hips":caps,"thickness_mm":200,"overhang_mm":180,"verge_mm":135})).unwrap();
            doc.buildings.push(b);
            let built = trigon_model::build(&doc).unwrap();
            let roof = built[0].roof.as_ref().unwrap();
            let pieces: Vec<_> = split::roof(roof, &built[0].walls).into_iter().filter(|p| p.gable.is_some()).collect();
            assert_eq!(pieces.len(), 2, "each source gable must remain one physical wall: {:?}", pieces.iter().map(|p| &p.key).collect::<Vec<_>>());
            for id in ["b.f2.gable", "b.f4.gable"] {
                assert_eq!(pieces.iter().filter(|p| p.key == id).count(), 1);
            }
            let clipped = pieces.iter().find(|p| p.key == "b.f4.gable").unwrap();
            let frame = split::gable_frame(&clipped.polygon, &built[0].walls[3]).unwrap();
            let ring: Vec<_> = clipped
                .polygon
                .iter()
                .map(|p| {
                    let q = frame.local(*p);
                    [q[0], q[2]]
                })
                .collect();
            assert!((part21::area(&ring).abs() / 1e6 - 34.56).abs() < 1e-8, "clipped gable area must be complete");
            let text = write(&doc, &built, "halfhip.ifc", "2026-01-01T00:00:00");
            assert_eq!(text.lines().filter(|l| l.contains("=IFCWALL(") && l.contains("Gavltrekant")).count(), 2);
            assert!(!text.lines().any(|l| l.contains("=IFCWALL(") && l.contains("'b.roof.gable")));
        }
    }
}

#[test]
fn unopened_half_hip_exports_the_actual_shell_type_and_clipped_gable() {
    let mut doc = Document::default();
    let mut b =
        trigon_model::Building::new("b", "Public clipped gable", &[[0., 0.], [30000., 0.], [30000., 12000.], [0., 12000.]], 3, 3000.).unwrap();
    b.roof=serde_json::from_value(serde_json::json!({"kind":"half_hipped","pitch_deg":45,"gables":["b.f2","b.f4"],"half_hips":[{"facade":"b.f4","drop_mm":1200}],"thickness_mm":200,"overhang_mm":180,"verge_mm":135})).unwrap();
    doc.buildings.push(b);
    let built = trigon_model::build(&doc).unwrap();
    assert!(split::opening_shell(&doc, &built[0]), "a bare clipped gable still requires its real solid");
    let roof = built[0].roof.as_ref().unwrap();
    assert!(roof_shell_quantities(&roof.cover, &Frame::plan([0.; 3], [1., 0.]).unwrap()).is_some());
    let text = write(&doc, &built, "halfhip.ifc", "2026-01-01T00:00:00");
    let occurrence = text.lines().find(|l| l.contains("=IFCROOF(")).unwrap();
    assert!(occurrence.contains(".HIPPED_GABLE_ROOF."));
    assert!(text.lines().any(|l| l.contains("=IFCROOFTYPE(") && l.contains(".HIPPED_GABLE_ROOF.")));
    assert!(text.contains("Trigon_RoofQuantities"));
    assert!(!text.lines().any(|l| l.contains("=IFCSLAB(") && l.contains("b.roof.face")));
    let shapes = split::model_shapes(&doc, &built);
    let (_, gable) = shapes.iter().find(|(id, _)| id == "b.f4.gable").unwrap();
    assert!((gable.positions.iter().map(|p| p[2]).fold(f64::NEG_INFINITY, f64::max) - 13800.).abs() < 1e-6);
    assert!(text.lines().any(|l| l.contains("=IFCWALL(") && l.contains("'b.f4.gable'")));
}

// Analytic, public fixtures: no surveyed geometry. The second triangle loses a corner to
// clean_ring's 0.01 mm weld even though its cap is larger than 1 mm².
fn fragment(long_thin: bool, built_up: bool) -> split::RoofPiece {
    let (length, width) = if long_thin { (400.0, 0.006) } else { (2.0, 0.5) };
    let polygon = [[0.0, 0.0], [length, 0.0], [length, width]].map(|[x, y]| [12000.0 + x, -2400.0 + y, 6500.0 + y * 0.75]).to_vec();
    let frame = split::plane_frame(&polygon).unwrap();
    split::RoofPiece {
        key: "fixture.roof.face0".into(),
        surface: Mesh { positions: polygon.clone(), triangles: vec![[0, 1, 2]] },
        polygon,
        rise: if built_up { [0.0, 0.0, 312.5] } else { frame.z.map(|v| v * 250.0) },
        gable: None,
    }
}

fn fixture(piece: &split::RoofPiece, built_up: bool) -> (Document, Vec<Built>) {
    let mut doc = Document::default();
    let mut building =
        trigon_model::Building::new("fixture", "Roof fragment", &[[0., 0.], [9000., 0.], [9000., 6000.], [0., 6000.]], 1, 3000.).unwrap();
    building.roof = serde_json::from_value(serde_json::json!({"kind":"pitched","pitch_deg":36.86989764584402})).unwrap();
    doc.buildings.push(building);
    let mut built = trigon_model::build(&doc).unwrap();
    let roof = built[0].roof.as_mut().unwrap();
    roof.cover = if built_up {
        split::plane_solid(piece)
    } else {
        let mut cap = piece.surface.clone();
        for p in &mut cap.positions {
            for (v, d) in p.iter_mut().zip(piece.rise) {
                *v += d;
            }
        }
        cap
    };
    roof.gables = Mesh::default();
    (doc, built)
}

fn signed_volume(mesh: &Mesh) -> f64 {
    let o = mesh.positions[0];
    mesh.triangles
        .iter()
        .map(|t| {
            let [a, b, c] = t.map(|i| part21::sub(mesh.positions[i as usize], o));
            part21::dot(a, part21::cross(b, c)) / 6.0
        })
        .sum()
}

fn closed_outward(mesh: &Mesh) -> bool {
    assert!(mesh.positions.iter().flatten().all(|v| v.is_finite()));
    let mut edges = BTreeMap::<_, (usize, i32)>::new();
    for t in &mesh.triangles {
        let points = t.map(|i| mesh.positions[i as usize]);
        let normal = part21::cross(part21::sub(points[1], points[0]), part21::sub(points[2], points[0]));
        assert!(part21::dot(normal, normal) > 0.0, "degenerate triangle");
        for i in 0..3 {
            // Both readers may return f32 positions; topology is checked by coincident points.
            let a = points[i].map(|v| (v * 1e6).round() as i64);
            let b = points[(i + 1) % 3].map(|v| (v * 1e6).round() as i64);
            assert_ne!(a, b);
            let e = edges.entry((a.min(b), a.max(b))).or_default();
            e.0 += 1;
            e.1 += if a < b { 1 } else { -1 };
        }
    }
    edges.values().all(|e| *e == (2, 0)) && signed_volume(mesh) > 0.0
}

fn assert_fragment(long_thin: bool, built_up: bool) {
    let piece = fragment(long_thin, built_up);
    let (doc, built) = fixture(&piece, built_up);
    let before = built.clone();
    let text = write(&doc, &built, "fragment.ifc", "2026-01-01T00:00:00");
    if let Some(directory) = std::env::var_os("TRIGON_WRITE_ROOF_FIXTURES") {
        let name = format!("{}-{}", if long_thin { "thin" } else { "small" }, if built_up { "built" } else { "bare" });
        std::fs::write(std::path::Path::new(&directory).join(format!("{name}.ifc")), &text).unwrap();
    }
    assert_eq!(built, before, "export must not mutate built geometry");
    let pieces = split::roof(built[0].roof.as_ref().unwrap(), &built[0].walls);
    assert_eq!(pieces.len(), 1);
    let piece = &pieces[0];
    let frame = split::plane_frame(&piece.polygon).unwrap();
    let raw: Vec<_> = piece.polygon.iter().map(|p| frame.local(*p)).map(|p| [p[0], p[1]]).collect();
    if long_thin {
        assert!(area(&raw) > 1.0);
        assert_eq!(clean_ring(&raw).len(), 2, "exercise cleanup, not just area rejection");
    } else {
        assert!(area(&raw) > 0.0 && area(&raw) < 1.0);
        assert_eq!(clean_ring(&raw).len(), 3);
    }
    let mesh = split::plane_solid(piece);
    assert!(closed_outward(&mesh), "source must be closed and outward");
    let product = text.lines().find(|line| line.contains("=IFCSLAB(") && line.contains(&gid(&piece.key)));
    assert!(product.is_some(), "finite positive roof fragment must have an IFC product: {}", piece.key);
    assert!(product.unwrap().ends_with(",.ROOF.);"));
    assert_metadata(&text, piece, "Tessellation");
    assert_eq!(text, write(&doc, &built, "fragment.ifc", "2026-01-01T00:00:00"), "stable IDs and deterministic output");
    for read in [trigon_ifcread::read_ifc_lite(&text).unwrap(), trigon_ifcread::read_bimifc(&text).unwrap()] {
        assert!(read.failures.is_empty(), "{}: {:?}", read.reader.name(), read.failures);
        let id = guid(&format!("trigon:{}", piece.key));
        let element = read.elements.iter().find(|e| e.global_id == id).expect("reader must build the fragment");
        assert_eq!(element.class, "IFCSLAB");
        assert!(trigon_ifcread::bounds_deviation(mesh.bounds().unwrap(), element.mesh.bounds().unwrap()) < 0.01, "{}", read.reader.name());
        let intact = closed_outward(&Mesh { positions: element.mesh.positions.clone(), triangles: element.mesh.triangles.clone() });
        if long_thin && read.reader == trigon_ifcread::Reader::IfcLite {
            // Characterize incomplete geometry, not successful solid coverage: this reader's
            // 1/65536 m cleanup drops the caps below its 0.015259 mm triangle-height grid.
            assert!(!intact, "reader limitation changed; re-evaluate thin-solid coverage");
            assert_eq!(element.mesh.triangles.len(), 4, "only the long side faces survive");
        } else {
            assert!(intact, "{}: reader must preserve the closed outward solid", read.reader.name());
            assert_eq!(element.mesh.triangles.len(), mesh.triangles.len());
        }
    }
}

#[derive(Debug)]
struct Entity<'a> {
    kind: &'a str,
    args: Vec<&'a str>,
}

fn args(text: &str) -> Vec<&str> {
    let (mut depth, mut quoted, mut start) = (0, false, 0);
    let mut result = Vec::new();
    for (i, ch) in text.char_indices() {
        match ch {
            '\'' => quoted = !quoted,
            '(' if !quoted => depth += 1,
            ')' if !quoted => depth -= 1,
            ',' if !quoted && depth == 0 => {
                result.push(&text[start..i]);
                start = i + 1;
            }
            _ => {}
        }
    }
    assert!(depth == 0 && !quoted);
    result.push(&text[start..]);
    result
}

fn reference(text: &str) -> usize {
    text.strip_prefix('#').unwrap().parse().unwrap()
}

fn references(text: &str) -> Vec<usize> {
    args(text.strip_prefix('(').unwrap().strip_suffix(')').unwrap()).into_iter().map(reference).collect()
}

fn assert_metadata(text: &str, piece: &split::RoofPiece, representation: &str) {
    let records: BTreeMap<_, _> = text
        .lines()
        .filter(|l| l.starts_with('#'))
        .map(|l| {
            let (id, body) = l.split_once('=').unwrap();
            let (kind, body) = body.split_once('(').unwrap();
            (reference(id), Entity { kind, args: args(body.strip_suffix(");").unwrap()) })
        })
        .collect();
    let (&id, slab) = records.iter().find(|(_, e)| e.kind == "IFCSLAB" && e.args[0] == gid(&piece.key)).unwrap();
    assert_eq!(slab.args[7], string(&piece.key));
    assert_eq!(slab.args[8], ".ROOF.");
    let up = catalog::pitched_roof((piece.thickness() * 10.0).round() / 10.0);
    assert_eq!(slab.args[2], string(&up.name));
    let shape = &records[&reference(slab.args[6])];
    assert_eq!(shape.kind, "IFCPRODUCTDEFINITIONSHAPE");
    let body_id = references(shape.args[2])[0];
    let body = &records[&body_id];
    assert_eq!(body.kind, "IFCSHAPEREPRESENTATION");
    assert_eq!(body.args[1], "'Body'");
    assert_eq!(body.args[2], string(representation));
    let item = references(body.args[3])[0];
    assert_eq!(records[&item].kind, if representation == "Tessellation" { "IFCTRIANGULATEDFACESET" } else { "IFCEXTRUDEDAREASOLID" });
    assert!(records.values().any(|e| e.kind == "IFCSTYLEDITEM" && e.args[0] == format!("#{item}")));
    assert!(
        records.values().any(|e| {
            e.kind == "IFCPRESENTATIONLAYERASSIGNMENT" && e.args[0] == string(catalog::L_ROOF) && references(e.args[2]).contains(&body_id)
        })
    );
    let typing = records.values().find(|e| e.kind == "IFCRELDEFINESBYTYPE" && references(e.args[4]).contains(&id)).unwrap();
    let ty = &records[&reference(typing.args[5])];
    assert_eq!(ty.kind, "IFCSLABTYPE");
    assert_eq!(ty.args[0], gid(&format!("slabtype:{}", up.name)));
    assert_eq!(ty.args[9], ".ROOF.");
    let material = records.values().find(|e| e.kind == "IFCRELASSOCIATESMATERIAL" && references(e.args[4]).contains(&id)).unwrap();
    let usage = &records[&reference(material.args[5])];
    assert_eq!(usage.kind, "IFCMATERIALLAYERSETUSAGE");
    assert_eq!(&usage.args[1..4], &[".AXIS3.", ".POSITIVE.", "0."]);
    let layers = &records[&reference(usage.args[0])];
    assert_eq!(layers.kind, "IFCMATERIALLAYERSET");
    let layers: Vec<_> = references(layers.args[0]).into_iter().map(|i| &records[&i]).collect();
    assert_eq!(layers.len(), up.layers.len());
    for (layer, expected) in layers.iter().zip(&up.layers) {
        assert_eq!(layer.kind, "IFCMATERIALLAYER");
        assert_eq!(records[&reference(layer.args[0])].args[0], string(expected.mat.name));
        assert!((layer.args[1].parse::<f64>().unwrap() - expected.thickness).abs() < 1e-6);
    }
    let properties: Vec<_> = records
        .values()
        .filter(|e| e.kind == "IFCRELDEFINESBYPROPERTIES" && references(e.args[4]).contains(&id))
        .map(|e| &records[&reference(e.args[5])])
        .collect();
    let common = properties.iter().find(|e| e.kind == "IFCPROPERTYSET" && e.args[2] == "'Pset_SlabCommon'").unwrap();
    let props: BTreeMap<_, _> = references(common.args[4]).into_iter().map(|i| (records[&i].args[0], records[&i].args[2])).collect();
    assert_eq!(props["'Reference'"], format!("IFCIDENTIFIER({})", string(&up.name)));
    assert_eq!(props["'IsExternal'"], "IFCBOOLEAN(.T.)");
    assert_eq!(props["'LoadBearing'"], "IFCBOOLEAN(.T.)");
    assert!(props["'PitchAngle'"].starts_with("IFCPLANEANGLEMEASURE("));
    let qto = properties.iter().find(|e| e.kind == "IFCELEMENTQUANTITY").unwrap();
    assert_eq!(qto.args[0], gid(&format!("{}.qto", piece.key)));
    assert_eq!(qto.args[2], "'Qto_SlabBaseQuantities'");
    let quantities: BTreeMap<_, _> =
        references(qto.args[5]).into_iter().map(|i| (records[&i].args[0], records[&i].args[3].parse::<f64>().unwrap())).collect();
    let frame = split::plane_frame(&piece.polygon).unwrap();
    let ring: Vec<_> = piece.polygon.iter().map(|p| frame.local(*p)).map(|p| [p[0], p[1]]).collect();
    assert!((quantities["'Width'"] - piece.thickness()).abs() <= 0.000001);
    for name in ["'GrossArea'", "'NetArea'"] {
        assert!(quantities[name] > 0.0);
        assert!((quantities[name] * 1e6 - area(&ring)).abs() <= 0.0005);
    }
    for name in ["'GrossVolume'", "'NetVolume'"] {
        assert!(quantities[name] > 0.0);
        assert!((quantities[name] * 1e9 - signed_volume(&split::plane_solid(piece))).abs() <= 0.5);
    }
    let aggregate = records.values().find(|e| e.kind == "IFCRELAGGREGATES" && references(e.args[5]).contains(&id)).unwrap();
    assert_eq!(records[&reference(aggregate.args[4])].kind, "IFCROOF");
}

#[test]
fn invalid_or_unrepresentable_roof_fragments_stay_rejected() {
    let good = fragment(false, false);
    let mut cases = Vec::new();
    for rise in [[0.0; 3], good.rise.map(|v| -v), [f64::INFINITY, 0.0, 1.0]] {
        cases.push(split::RoofPiece { rise, ..good.clone() });
    }
    let mut bad = good.clone();
    bad.surface.triangles[0][2] = 99;
    cases.push(bad);
    let mut bad = good.clone();
    bad.surface.triangles.push([0, 2, 1]); // cancelling/overlapping caps are not one disk
    cases.push(bad);
    let mut bad = good.clone();
    bad.surface.positions[1][0] += 1.0; // cap and boundary disagree
    cases.push(bad);
    for width in [0.0, f64::NAN, 0.0000001, 0.0000022] {
        let mut bad = good.clone();
        bad.polygon[1][0] += 10000.0;
        bad.polygon[2] = [bad.polygon[1][0], bad.polygon[1][1] + width, bad.polygon[1][2] + width * 0.75];
        bad.surface.positions.clone_from(&bad.polygon);
        cases.push(bad);
    }
    let doc = Document::default();
    for piece in cases {
        let mut writer = Ifc::new(&doc);
        let parent = writer.w.place(None, Frame::at([0.0; 3]));
        assert_eq!(writer.roof_plane(&piece, &catalog::pitched_roof(250.0), parent, 36.87), None, "{piece:?}");
        assert!(!writer.w.lines.iter().any(|l| l.starts_with("IFCSLAB(")), "rejection must not create a bogus product");
    }
}

#[test]
fn small_positive_roof_caps_have_real_solids_in_both_readers() {
    for built_up in [false, true] {
        assert_fragment(false, built_up);
    }
}

#[test]
fn long_thin_roof_caps_keep_ifc_solids_but_characterize_reader_cleanup_loss() {
    for built_up in [false, true] {
        assert_fragment(true, built_up);
    }
}

#[test]
fn normal_roofs_keep_extrusions_and_existing_tessellations() {
    for built_up in [false, true] {
        let mut piece = fragment(false, built_up);
        for p in &mut piece.polygon {
            for (v, o) in p.iter_mut().zip([12000.0, -2400.0, 6500.0]) {
                *v = o + (*v - o) * 10.0;
            }
        }
        piece.surface.positions.clone_from(&piece.polygon);
        let (doc, built) = fixture(&piece, built_up);
        let text = write(&doc, &built, "normal.ifc", "2026-01-01T00:00:00");
        assert_metadata(&text, &piece, if built_up { "Tessellation" } else { "SweptSolid" });
        if let Some(directory) = std::env::var_os("TRIGON_WRITE_ROOF_FIXTURES") {
            let name = if built_up { "normal-built.ifc" } else { "normal-bare.ifc" };
            std::fs::write(std::path::Path::new(&directory).join(name), text).unwrap();
        }
    }
    let mut doc = crate::ifc::sample::document().unwrap();
    for bare in [false, true] {
        if bare && let trigon_model::Roof::Pitched { thickness_mm, overhang_mm, verge_mm, gutters, .. } = &mut doc.buildings[0].roof {
            (*thickness_mm, *overhang_mm, *verge_mm, *gutters) = (0.0, 0.0, 0.0, false);
        }
        let built = trigon_model::build(&doc).unwrap();
        let text = write(&doc, &built, "sample.ifc", "2026-01-01T00:00:00");
        if let Some(directory) = std::env::var_os("TRIGON_WRITE_ROOF_FIXTURES") {
            let name = if bare { "sample-bare.ifc" } else { "sample-built.ifc" };
            std::fs::write(std::path::Path::new(&directory).join(name), text).unwrap();
        }
    }
}

#[test]
fn shallow_positive_roof_fragments_use_solids_instead_of_short_extrusions() {
    let mut piece = fragment(false, false);
    piece.rise = piece.rise.map(|v| v / 500.0);
    let doc = Document::default();
    let mut writer = Ifc::new(&doc);
    let parent = writer.w.place(None, Frame::at([0.0; 3]));
    assert!(writer.roof_plane(&piece, &catalog::pitched_roof(0.5), parent, 36.87).is_none(), "sub-quantity-resolution volume is uncovered");
    // A larger cap makes the same 0.5 mm depth a representable positive solid.
    piece.polygon[1][0] += 20.0;
    piece.polygon[2][0] += 20.0;
    piece.surface.positions.clone_from(&piece.polygon);
    let mut writer = Ifc::new(&doc);
    let parent = writer.w.place(None, Frame::at([0.0; 3]));
    assert!(writer.roof_plane(&piece, &catalog::pitched_roof(0.5), parent, 36.87).is_some());
    assert!(writer.w.lines.iter().any(|l| l.starts_with("IFCTRIANGULATEDFACESET(")));
    assert!(!writer.w.lines.iter().any(|l| l.starts_with("IFCEXTRUDEDAREASOLID(")));
}

#[path = "roof_shells.rs"]
mod roof_shells;
