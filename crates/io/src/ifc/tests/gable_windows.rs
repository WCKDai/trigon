//! Public analytic hosts: one shaped wall, true voids, typed fills and exact quantities.
use super::*;
use crate::ifc::{guid, part21, split};

#[test]
fn attic_gable_ifc_keeps_one_net_host_with_real_voids_fills_and_quantities() {
    for rotation in [0.0_f64, 31.] {
        for count in [1, 2] {
            let (sin, cos) = rotation.to_radians().sin_cos();
            let outline =
                [[0., 0.], [30000., 0.], [30000., 12000.], [0., 12000.]].map(|[x, y]| [142000. + x * cos - y * sin, -270000. + x * sin + y * cos]);
            let mut b = trigon_model::Building::new("b", "Public opened gable", &outline, 3, 3000.).unwrap();
            b.roof = serde_json::from_value(serde_json::json!({"kind":"half_hipped","pitch_deg":45,
                "gables":["b.f2","b.f4"],"half_hips":[{"facade":"b.f4","drop_mm":1200}],
                "thickness_mm":200,"overhang_mm":180,"verge_mm":135}))
            .unwrap();
            b.set_attic(Some(8250.), Some("Roof")).unwrap();
            b.facades[3].bays = vec![4000., 8000.];
            for (bay, u) in [4500., 7500.].into_iter().take(count).enumerate() {
                b.facades[3].placements.push(
                    serde_json::from_value(serde_json::json!({"level":3,"bay":bay,
                    "type":"window.casement.2","u":u,"width":1200,"height":1000,"sill":2700}))
                    .unwrap(),
                );
            }
            let mut doc = Document::default();
            doc.buildings.push(b);
            let built = trigon_model::build(&doc).unwrap();
            let g = &built[0].roof.as_ref().unwrap().gable_walls[0];
            let text = export(&doc, &built);
            let records = entities(&text);
            let host_guid = gid(&g.front.id);
            let hosts: Vec<_> = records.iter().filter(|(_, e)| e.kind == "IFCWALL" && e.attributes[0] == host_guid).collect();
            assert_eq!(hosts.len(), 1, "one real gable host, never a gross overlay or anonymous fragments");
            let (&host, wall) = hosts[0];
            let definition = &records[&reference(wall.attributes[6])];
            let representations = references(definition.attributes[2]);
            assert_eq!(representations.len(), 1);
            assert_eq!(records[&representations[0]].attributes[2], "'Tessellation'");
            let inventory = split::model_shapes(&doc, &built);
            assert_eq!(inventory.iter().filter(|(id, _)| id == &g.front.id).count(), 1);
            let actual = &inventory.iter().find(|(id, _)| id == &g.front.id).unwrap().1;
            assert_eq!(actual, &g.front.mesh);
            let quantities = verify_quantities(&doc, &built, &text);
            assert_eq!(quantities.len(), count);
            for opening in &g.front.openings {
                assert_eq!(quantities[&quantity_guid(&opening.fill)]["Area"], 1.2);
                let opening_guid = gid(&opening.id);
                let (&void, _) = records.iter().find(|(_, e)| e.kind == "IFCOPENINGELEMENT" && e.attributes[0] == opening_guid).unwrap();
                let voids: Vec<_> = records.values().filter(|e| e.kind == "IFCRELVOIDSELEMENT" && reference(e.attributes[5]) == void).collect();
                assert_eq!(voids.len(), 1);
                assert_eq!(reference(voids[0].attributes[4]), host);
                let fills: Vec<_> = records.values().filter(|e| e.kind == "IFCRELFILLSELEMENT" && reference(e.attributes[4]) == void).collect();
                assert_eq!(fills.len(), 1);
                let fill = &records[&reference(fills[0].attributes[5])];
                assert_eq!(fill.kind, "IFCWINDOW");
                assert_eq!(fill.attributes[0], gid(&opening.fill.id));
            }
            let qset = records
                .values()
                .filter(|e| e.kind == "IFCRELDEFINESBYPROPERTIES" && references(e.attributes[4]) == [host])
                .map(|e| &records[&reference(e.attributes[5])])
                .find(|e| e.kind == "IFCELEMENTQUANTITY" && e.attributes[2] == "'Qto_WallBaseQuantities'")
                .unwrap();
            let qs: BTreeMap<_, _> = references(qset.attributes[5])
                .into_iter()
                .map(|id| {
                    let q = &records[&id];
                    (q.attributes[0], q.attributes[3].parse::<f64>().unwrap())
                })
                .collect();
            let gross = part21::volume(&g.gross) / 1e9;
            let net = part21::volume(&g.front.mesh) / 1e9;
            let boundary: f64 = g
                .front
                .mesh
                .triangles
                .iter()
                .map(|t| {
                    let [a, b, c] = t.map(|i| g.front.mesh.positions[i as usize]);
                    let n = part21::cross(part21::sub(b, a), part21::sub(c, a));
                    part21::dot(n, n).sqrt() / 2.
                })
                .sum();
            assert!((qs["'GrossVolume'"] - gross).abs() < 1e-6);
            assert!((qs["'NetVolume'"] - net).abs() < 1e-6);
            assert!((gross - net - count as f64 * 1.2 * g.thickness / 1000.).abs() < 1e-6);
            assert!((qs["'GrossSideArea'"] - qs["'NetSideArea'"] - count as f64 * 1.2).abs() < 1e-6);
            for reader in [trigon_ifcread::read_ifc_lite(&text).unwrap(), trigon_ifcread::read_bimifc(&text).unwrap()] {
                assert!(reader.failures.is_empty(), "{}: {:?}", reader.reader.name(), reader.failures);
                let parsed: Vec<_> = reader.elements.iter().filter(|e| e.global_id == guid(&format!("trigon:{}", g.front.id))).collect();
                assert_eq!(parsed.len(), 1);
                let mesh = trigon_model::Mesh { positions: parsed[0].mesh.positions.clone(), triangles: parsed[0].mesh.triangles.clone() };
                assert!(trigon_ifcread::bounds_deviation(mesh.bounds().unwrap(), g.front.mesh.bounds().unwrap()) < 0.05);
                // At these translated coordinates the readers' f32 grid is up to
                // 0.03125 mm. Bound volume error by boundary area * 0.05 mm,
                // independently of the much stricter written-model quantities above.
                // A filled 1.2 m² aperture exceeds this allowance by two orders.
                let allowance = boundary * 0.05 / 1e9;
                assert!(
                    (part21::volume(&mesh) / 1e9 - net).abs() < allowance,
                    "{} rotation={rotation} count={count}: parsed={} m3 vs net={net} m3 (allowance={allowance})",
                    reader.reader.name(),
                    part21::volume(&mesh) / 1e9
                );
            }
        }
    }
}
