//! The model's stepped material, rather than its legacy quad reference, must reach IFC.
use super::*;
use trigon_model::{Building, InteriorWall, Mesh, WallDoor};

fn fixture(perpendicular: bool, reversed: bool, door_height: Option<f64>) -> Document {
    let mut doc = Document::default();
    let mut b =
        Building::new("junction", "Public stepped-wall fixture", &[[0., 0.], [10000., 0.], [10000., 10000.], [0., 10000.]], 1, 3000.).unwrap();
    for (id, mut a, mut c, thickness) in [
        ("north", [5000., 5000.], [5000., 6500.], 120.),
        ("south", [5000., 5000.], [5000., 3700.], 240.),
        ("branch", if perpendicular { [3200., 5000.] } else { [4300., 4250.] }, [5000., 5000.], 120.),
    ] {
        if reversed {
            std::mem::swap(&mut a, &mut c);
        }
        let doors = if id == "branch" {
            door_height.map(|height| WallDoor { at: if reversed { 150. } else { 1650. }, width: 300., height }).into_iter().collect()
        } else {
            Vec::new()
        };
        b.interior_walls.push(InteriorWall { id: id.into(), levels: vec![0], a, b: c, thickness, load_bearing: false, doors });
    }
    doc.buildings.push(b);
    doc
}

fn volume(mesh: &Mesh) -> f64 {
    let origin = mesh.positions[0];
    mesh.triangles
        .iter()
        .map(|t| {
            let [a, b, c] = t.map(|i| crate::ifc::part21::sub(mesh.positions[i as usize], origin));
            crate::ifc::part21::dot(a, crate::ifc::part21::cross(b, c)) / 6.
        })
        .sum::<f64>()
        .abs()
}

fn reader_volume(mesh: &trigon_ifcread::Mesh) -> f64 {
    volume(&Mesh { positions: mesh.positions.clone(), triangles: mesh.triangles.clone() })
}

fn inside(ring: &[[f64; 2]], p: [f64; 2]) -> bool {
    let mut yes = false;
    for (a, b) in ring.iter().zip(ring.iter().cycle().skip(1)).take(ring.len()) {
        if (a[1] > p[1]) != (b[1] > p[1]) && p[0] < (b[0] - a[0]) * (p[1] - a[1]) / (b[1] - a[1]) + a[0] {
            yes = !yes;
        }
    }
    yes
}

fn cap_contains(mesh: &trigon_ifcread::Mesh, p: [f64; 2], z: f64) -> bool {
    mesh.triangles.iter().any(|t| {
        let points = t.map(|i| mesh.positions[i as usize]);
        if points.iter().any(|q| (q[2] - z).abs() > 0.01) {
            return false;
        }
        let sides: Vec<_> = points
            .iter()
            .zip(points.iter().cycle().skip(1))
            .take(3)
            .map(|(a, b)| (b[0] - a[0]) * (p[1] - a[1]) - (b[1] - a[1]) * (p[0] - a[0]))
            .collect();
        sides.iter().all(|s| *s >= 0.) || sides.iter().all(|s| *s <= 0.)
    })
}

fn quantities(text: &str, id: &str) -> BTreeMap<String, f64> {
    let records = entities(text);
    let qto = records.values().find(|e| e.kind == "IFCELEMENTQUANTITY" && e.attributes[0] == gid(&format!("{id}.qto"))).unwrap();
    references(qto.attributes[5])
        .into_iter()
        .map(|i| {
            let q = &records[&i];
            (q.attributes[0].trim_matches('\'').into(), q.attributes[3].parse().unwrap())
        })
        .collect()
}

#[test]
fn stepped_wall_ifc_preserves_material_volumes_quantities_and_hosted_doors() {
    for reversed in [false, true] {
        for (perpendicular, door_height) in [(false, None), (true, Some(2100.)), (true, Some(4000.))] {
            let doc = fixture(perpendicular, reversed, door_height);
            let original = serde_json::to_vec(&doc).unwrap();
            let built = trigon_model::build(&doc).unwrap();
            let mut gross_doc = doc.clone();
            for wall in &mut gross_doc.buildings[0].interior_walls {
                wall.doors.clear();
            }
            let gross = trigon_model::build(&gross_doc).unwrap();
            let text = write(&doc, &built, "stepped.ifc", "2026-01-01T00:00:00");
            assert_eq!(serde_json::to_vec(&doc).unwrap(), original);
            for reader in [trigon_ifcread::read_ifc_lite(&text).unwrap(), trigon_ifcread::read_bimifc(&text).unwrap()] {
                assert!(reader.failures.is_empty());
                for wall in &built[0].interior {
                    let product = reader.elements.iter().find(|e| e.global_id == crate::ifc::guid(&format!("trigon:{}", wall.id))).unwrap();
                    let full = gross[0].interior.iter().find(|w| w.id == wall.id).unwrap();
                    let expected_mesh = if reader.voids_applied { &wall.mesh } else { &full.mesh };
                    let expected = volume(expected_mesh);
                    let actual = reader_volume(&product.mesh);
                    assert!(
                        (actual - expected).abs() < expected * 0.00001 + 1.,
                        "{} {}: volume {actual} vs actual material {expected}",
                        reader.reader.name(),
                        wall.id
                    );
                    assert!(
                        trigon_ifcread::bounds_deviation(expected_mesh.bounds().unwrap(), product.mesh.bounds().unwrap()) < 0.01,
                        "{} {} bounds; reversed={reversed}, door={door_height:?}",
                        reader.reader.name(),
                        wall.id
                    );
                    let top = wall.z_bottom + wall.height;
                    let rings = if reader.voids_applied && wall.doors.iter().any(|d| d.rect.v1 > top) { &wall.cut } else { &full.cut };
                    for x in 0..21 {
                        for y in 0..21 {
                            let point = [4750. + (f64::from(x) + 0.137) * 15., 4800. + (f64::from(y) + 0.271) * 20.];
                            assert_eq!(
                                cap_contains(&product.mesh, point, top),
                                rings.iter().any(|r| inside(r, point)),
                                "{} {} material coverage at {point:?}; door={door_height:?}",
                                reader.reader.name(),
                                wall.id
                            );
                        }
                    }
                }
            }
            let branch = built[0].interior.iter().find(|w| w.id == "branch.L0").unwrap();
            let full = gross[0].interior.iter().find(|w| w.id == branch.id).unwrap();
            let qs = quantities(&text, &branch.id);
            assert!((qs["GrossVolume"] * 1e9 - volume(&full.mesh)).abs() <= 0.5);
            assert!((qs["NetVolume"] * 1e9 - volume(&branch.mesh)).abs() <= 0.5);
            let gross_area: f64 = full.cut.iter().map(|r| trigon_model::doc::polygon_area(r)).sum();
            let net_area: f64 = branch.cut.iter().map(|r| trigon_model::doc::polygon_area(r)).sum();
            assert!((qs["GrossFootprintArea"] * 1e6 - gross_area).abs() <= 0.0005);
            assert!((qs["NetFootprintArea"] * 1e6 - net_area).abs() <= 0.0005);
            assert!(qs["GrossSideArea"] > 0. && qs["NetSideArea"] >= 0.);
            if perpendicular {
                // Source bands own x>=4880 on one half and x>=4940 on the other. The
                // silhouette is1740mm long; the end doorway removes240mm of that projection.
                assert!((qs["GrossSideArea"] * 1e6 - 1740. * branch.height).abs() <= 0.0005);
                assert!((qs["NetSideArea"] * 1e6 - (1740. * branch.height - 240. * door_height.unwrap().min(branch.height))).abs() <= 0.0005);
                assert!((gross_area - 205200.).abs() < 0.01 && (net_area - 180000.).abs() < 0.01);
            }
            let records = entities(&text);
            let (&id, wall) = records.iter().find(|(_, e)| e.kind == "IFCWALL" && e.attributes[0] == gid(&branch.id)).unwrap();
            assert_eq!(wall.attributes[7], crate::ifc::string(&branch.id));
            assert!(records.values().any(|e| e.kind == "IFCRELDEFINESBYTYPE" && references(e.attributes[4]).contains(&id)));
            assert!(records.values().any(|e| e.kind == "IFCRELASSOCIATESMATERIAL" && references(e.attributes[4]).contains(&id)));
            let voids: Vec<_> = records.values().filter(|e| e.kind == "IFCRELVOIDSELEMENT" && reference(e.attributes[4]) == id).collect();
            assert_eq!(voids.len(), usize::from(door_height.is_some()));
            for void in voids {
                assert!(records.values().any(|e| e.kind == "IFCRELFILLSELEMENT" && reference(e.attributes[4]) == reference(void.attributes[5])));
            }
        }
    }
}

#[test]
fn nearby_stepped_stations_have_single_material_coverage_in_both_readers() {
    let sources = [
        ([1000., 5000.], [3000., 5000.], 240.),
        ([3000., 5000.], [3120., 5000.], 120.),
        ([3120., 5000.], [9000., 5000.], 400.),
        ([3000., 2000.], [3000., 5000.], 300.),
        ([3120., 8000.], [3120., 5000.], 120.),
    ];
    for reversed in [false, true] {
        let mut doc = fixture(false, false, None);
        doc.buildings[0].interior_walls = sources
            .iter()
            .enumerate()
            .map(|(i, &(a, b, thickness))| InteriorWall {
                id: format!("wall{i}"),
                levels: vec![0],
                a: if reversed { b } else { a },
                b: if reversed { a } else { b },
                thickness,
                load_bearing: false,
                doors: Vec::new(),
            })
            .collect();
        let built = trigon_model::build(&doc).unwrap();
        let text = write(&doc, &built, "nearby.ifc", "2026-01-01T00:00:00");
        for reader in [trigon_ifcread::read_ifc_lite(&text).unwrap(), trigon_ifcread::read_bimifc(&text).unwrap()] {
            assert!(reader.failures.is_empty());
            let products: Vec<_> = built[0]
                .interior
                .iter()
                .map(|wall| {
                    let product = reader.elements.iter().find(|e| e.global_id == crate::ifc::guid(&format!("trigon:{}", wall.id))).unwrap();
                    let expected = volume(&wall.mesh);
                    assert!((reader_volume(&product.mesh) - expected).abs() < expected * 0.00001 + 1.);
                    product
                })
                .collect();
            assert_eq!(products.iter().filter(|p| cap_contains(&p.mesh, [3130., 4900.], 2800.)).count(), 1, "formerly duplicated material");
            for x in 0..41 {
                for y in 0..41 {
                    let point = [2800. + (f64::from(x) + 0.137) * 10., 4760. + (f64::from(y) + 0.271) * 10.];
                    let expected = built[0].interior.iter().any(|wall| wall.cut.iter().any(|ring| inside(ring, point)));
                    assert_eq!(
                        products.iter().filter(|p| cap_contains(&p.mesh, point, 2800.)).count(),
                        usize::from(expected),
                        "{} material coverage at {point:?}, reversed={reversed}",
                        reader.reader.name()
                    );
                }
            }
        }
    }
}
