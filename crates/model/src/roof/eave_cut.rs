//! A two-cut plumb eave: a vertical fascia and a horizontal underside return.

use crate::{Mesh, ModelError, Result, checked};

fn invalid(message: &str) -> ModelError {
    ModelError::Invalid(format!("Two Cut–Plumb: {message}"))
}

/// Vertical fascia-toe offset above the old eave soffit, retaining wall bearing.
pub(super) fn datum(pitch: f64, overhang: f64, thickness: f64, depth: f64) -> Result<f64> {
    let angle = checked("two-cut roof pitch", pitch, 5., 75.)?.to_radians();
    checked("two-cut roof overhang", overhang, 0., 2000.)?;
    checked("two-cut roof normal thickness", thickness, 1., 1000.)?;
    checked("two-cut vertical fascia depth", depth, 1., thickness)?;
    let offset = thickness / angle.cos() - depth;
    let minimum = offset / angle.tan();
    if overhang + 1e-6 < minimum {
        return Err(ModelError::Invalid(format!(
            "Two Cut–Plumb: fascia depth {depth:.6} mm with roof thickness {thickness:.6} mm at {pitch:.6} degrees needs overhang >= {minimum:.6} mm to retain wall bearing; supplied {overhang:.6} mm intrudes {:.6} mm inside it. Revise overhang and fascia together; roof thickness is not reduced",
            minimum - overhang
        )));
    }
    Ok(offset)
}

fn intersection(mut a: [f64; 3], mut b: [f64; 3], z: f64) -> [f64; 3] {
    if a[0].total_cmp(&b[0]).then(a[1].total_cmp(&b[1])).then(a[2].total_cmp(&b[2])).is_gt() {
        std::mem::swap(&mut a, &mut b);
    }
    let t = ((z - a[2]) / (b[2] - a[2])).clamp(0., 1.);
    [a[0] + t * (b[0] - a[0]), a[1] + t * (b[1] - a[1]), z]
}

fn clip(poly: &[[f64; 3]], z: f64, above: bool) -> Vec<[f64; 3]> {
    let mut out = Vec::new();
    for (&a, &b) in poly.iter().zip(poly.iter().cycle().skip(1)).take(poly.len()) {
        let inside = |p: [f64; 3]| if above { p[2] >= z } else { p[2] <= z };
        if inside(a) {
            out.push(a);
        }
        if inside(a) != inside(b) {
            out.push(intersection(a, b, z));
        }
    }
    out.dedup();
    if out.len() > 1 && out.first() == out.last() {
        out.pop();
    }
    out
}

fn triangle(mesh: &mut Mesh, t: [[f64; 3]; 3]) -> Result<()> {
    let [a, b, c] = t;
    let u = [b[0] - a[0], b[1] - a[1], b[2] - a[2]];
    let v = [c[0] - a[0], c[1] - a[1], c[2] - a[2]];
    if [u[1] * v[2] - u[2] * v[1], u[2] * v[0] - u[0] * v[2], u[0] * v[1] - u[1] * v[0]] == [0.; 3] {
        return Ok(());
    }
    let base = u32::try_from(mesh.positions.len()).ok().filter(|n| *n <= u32::MAX - 2).ok_or_else(|| invalid("roof mesh exceeds index capacity"))?;
    mesh.positions.extend(t);
    mesh.triangles.push([base, base + 1, base + 2]);
    Ok(())
}

/// Split the original upward heightfield before shaping either skin or its edge walls.
pub(super) fn shell(under: &Mesh, lift: f64, toe: f64) -> Result<Mesh> {
    let key = |p: [f64; 3]| p.map(|v| (v * 1e6).round() as i64);
    let mut edges = std::collections::BTreeMap::<_, ([f64; 3], [f64; 3], usize, i32)>::new();
    let mut mesh = Mesh::default();
    for indices in &under.triangles {
        let [Some(a), Some(b), Some(c)] = indices.map(|i| under.positions.get(i as usize).copied()) else {
            return Err(invalid("source triangle index is absent"));
        };
        for (a, b) in [(a, b), (b, c), (c, a)] {
            if a.iter().chain(&b).any(|v| !v.is_finite() || v.abs() > 1e9) {
                return Err(invalid("source coordinates are not finite site-local millimetres"));
            }
            let (ka, kb) = (key(a), key(b));
            if ka == kb {
                return Err(invalid("source edge collapses at 1e-6 mm"));
            }
            let (edge, sign) = if ka < kb { ((ka, kb), 1) } else { ((kb, ka), -1) };
            let entry = edges.entry(edge).or_insert((a, b, 0, 0));
            entry.2 += 1;
            entry.3 += sign;
        }
        for above in [false, true] {
            let poly = clip(&[a, b, c], toe, above);
            if let Some(&first) = poly.first() {
                for edge in poly.windows(2).skip(1) {
                    let t = [first, edge[0], edge[1]];
                    triangle(&mut mesh, t.map(|p| [p[0], p[1], p[2] + lift]))?;
                    let [a, b, c] = t.map(|p| [p[0], p[1], p[2].max(toe)]);
                    triangle(&mut mesh, [a, c, b])?;
                }
            }
        }
    }
    for (_, (a, b, count, sign)) in edges {
        if count == 2 && sign == 0 {
            continue;
        }
        if count != 1 {
            return Err(invalid("source roof heightfield is not conforming"));
        }
        let mut stations = vec![a];
        if (a[2] < toe && b[2] > toe) || (a[2] > toe && b[2] < toe) {
            stations.push(intersection(a, b, toe));
        }
        stations.push(b);
        for pair in stations.windows(2) {
            let [a, b] = [pair[0], pair[1]];
            let lo = |p: [f64; 3]| [p[0], p[1], p[2].max(toe)];
            let hi = |p: [f64; 3]| [p[0], p[1], p[2] + lift];
            triangle(&mut mesh, [lo(a), lo(b), hi(b)])?;
            triangle(&mut mesh, [lo(a), hi(b), hi(a)])?;
        }
    }
    super::halfhip::closed(&mesh).map_err(|e| invalid(&format!("shaped shell failed closure: {e}")))?;
    Ok(mesh)
}

#[cfg(test)]
mod tests {
    use crate::{Building, Document, Mesh, Roof};

    fn document(source: serde_json::Value) -> Document {
        let mut b = Building::new("b", "Two-cut fixture", &[[0., 0.], [16000., 0.], [16000., 8000.], [0., 8000.]], 3, 3000.).unwrap();
        b.roof = serde_json::from_value(source).unwrap();
        let mut doc = Document::default();
        doc.buildings.push(b);
        doc
    }

    fn source() -> serde_json::Value {
        serde_json::json!({"kind":"pitched","pitch_deg":46.,"gables":["b.f2","b.f4"],"straighten_mm":0.,"overhang_mm":400.,"verge_mm":135.,"thickness_mm":200.})
    }

    #[test]
    fn cut_shell_is_closed_for_hips_halfhips_skew_concavity_and_transformed_frames() {
        for ring in [
            vec![[0., 0.], [12000., 0.], [12000., 8000.], [0., 8000.]],
            vec![[0., 0.], [12000., 600.], [11500., 8600.], [-500., 8000.]],
            vec![[0., 0.], [12000., 0.], [12000., 6000.], [6000., 6000.], [6000., 12000.], [0., 12000.]],
        ] {
            for pitch in [10_f64, 46., 75.] {
                for yaw in [0_f64, 31., -118.] {
                    let (s, c) = yaw.to_radians().sin_cos();
                    let ring: Vec<_> = ring.iter().map(|p| [p[0] * c - p[1] * s + 12345.67, p[0] * s + p[1] * c - 7654.32]).collect();
                    for kind in ["hip", "gabled", "half_hipped"] {
                        if ring.len() != 4 && kind != "hip" {
                            continue;
                        }
                        let gables = if kind == "hip" { vec![] } else { vec![1, 3] };
                        let caps = if kind == "half_hipped" { vec![(3, 300.)] } else { vec![] };
                        let buildup = super::super::Buildup { overhang: 1800., verge: 135., thickness: 200., gutters: false };
                        let old = super::super::pitched_with_caps(&ring, 9000., pitch, &gables, &buildup, &caps).unwrap();
                        let new = super::super::pitched_with_eave_cut(&ring, 9000., pitch, &gables, &buildup, &caps, Some(20.))
                            .unwrap_or_else(|e| panic!("{kind}, {pitch}, {yaw}, {ring:?}: {e}"));
                        super::super::halfhip::closed(&new.cover).unwrap();
                        assert!((new.ridge - old.ridge).abs() < 1e-6);
                        for tri in &new.cover.triangles {
                            let [a, b, c] = tri.map(|i| new.cover.positions[i as usize]);
                            if (b[0] - a[0]) * (c[1] - a[1]) - (b[1] - a[1]) * (c[0] - a[0]) > 1e-5 {
                                let xy = [(a[0] + b[0] + c[0]) / 3., (a[1] + b[1] + c[1]) / 3.];
                                assert!((height(&old.cover, xy, true) - (a[2] + b[2] + c[2]) / 3.).abs() < 1e-5);
                            }
                        }
                    }
                }
            }
        }
    }

    #[test]
    fn near_toe_source_vertices_keep_their_exact_boundary_stations() {
        let mut under = Mesh::default();
        for (a, b) in [(0., 1.01), (1.01, 4.)] {
            under.quad([a, 0., a], [b, 0., b], [b, 4., b], [a, 4., a]);
        }
        let shell = super::shell(&under, 3., 1.).unwrap();
        super::super::halfhip::closed(&shell).unwrap();
        assert!((height(&shell, [0.5, 2.], false) - 1.).abs() < 1e-9);
        assert!((height(&shell, [1.005, 2.], false) - 1.005).abs() < 1e-9);
        assert!(shell.positions.iter().any(|p| p[0] == 1.01 && p[2] == 1.01));
    }

    #[test]
    fn cut_refuses_invalid_dimensions_unsupported_bases_and_lost_wall_bearing() {
        for depth in [0., -1., 201., f64::NAN, f64::INFINITY, f64::MAX] {
            let doc = document(source());
            assert!(doc.buildings[0].roof.clone().with_two_cut_plumb(Some(depth)).is_err());
        }
        for (key, value, detail) in [("overhang_mm", 180., "258.718943"), ("thickness_mm", 0., "thickness"), ("pitch_deg", 0., "pitch")] {
            let mut roof = source();
            roof[key] = serde_json::json!(value);
            let doc = document(roof);
            let error = doc.buildings[0].roof.clone().with_two_cut_plumb(Some(20.)).unwrap_err().to_string();
            assert!(error.contains(detail), "{error}");
        }
        for roof in [Roof::Flat, serde_json::from_value(serde_json::json!({"kind":"mansard"})).unwrap()] {
            assert!(roof.with_two_cut_plumb(Some(20.)).is_err());
        }
        let mut doc = document(source());
        doc.buildings[0].roof = Roof::TwoCutPlumb(super::super::TwoCutPlumb {
            fascia_depth_mm: 20.,
            roof: Box::new(Roof::TwoCutPlumb(super::super::TwoCutPlumb { fascia_depth_mm: 20., roof: Box::new(doc.buildings[0].roof.clone()) })),
        });
        assert!(crate::build(&doc).unwrap_err().to_string().contains("nested"));
        let mut doc = document(source());
        doc.buildings[0].roof = doc.buildings[0].roof.clone().with_two_cut_plumb(Some(20.)).unwrap();
        let old = doc.buildings[0].roof.base().clone();
        let half = doc.buildings[0].roof.clone().with_half_hips(vec![super::super::HalfHip { facade: "b.f4".into(), drop_mm: 900. }]).unwrap();
        assert_eq!(half.fascia_depth(), Some(20.));
        assert_eq!(half.with_half_hips(vec![]).unwrap().with_two_cut_plumb(None).unwrap(), old);
    }

    #[test]
    fn fallback_gutter_rim_stays_below_the_twenty_mm_fascia_top() {
        let mut source = source();
        source["gutters"] = true.into();
        let mut doc = document(source);
        let old = crate::build(&doc).unwrap().remove(0).roof.unwrap();
        doc.buildings[0].roof = doc.buildings[0].roof.clone().with_two_cut_plumb(Some(20.)).unwrap();
        let new = crate::build(&doc).unwrap().remove(0).roof.unwrap();
        let toe = 9000. - 400. * 46_f64.to_radians().tan() + 200. / 46_f64.to_radians().cos() - 20.;
        let gutter_top = new.gutters.positions.iter().map(|p| p[2]).fold(f64::NEG_INFINITY, f64::max);
        assert!((gutter_top - toe).abs() < 1e-6);
        assert_eq!(new.gutters.triangles, old.gutters.triangles);
        assert!(new.gutters.positions.iter().all(|p| p[2] <= toe + 1e-6));
    }

    #[test]
    fn fascia_coordinates_detailed_drainage_and_supports_the_original_cornice() {
        let mut roof = source();
        roof["gutters"] = true.into();
        roof["drainage"] = serde_json::json!({"downpipes":[{"id":"pipe","facade":"b.f1","along_mm":3000.,"diameter_mm":100.,"wall_offset_mm":80.,"bottom_mm":0.,"shoe_mm":150.}]});
        let mut doc = document(roof);
        for index in [0, 2] {
            doc.buildings[0].facades[index].bands.push(crate::doc::Band {
                levels: vec![2],
                bottom: 2820.,
                top: 3000.,
                projection: 135.,
                finish: None,
            });
        }
        let old = crate::build(&doc).unwrap().remove(0);
        doc.buildings[0].roof = doc.buildings[0].roof.clone().with_two_cut_plumb(Some(20.)).unwrap();
        let new = crate::build(&doc).unwrap().remove(0);
        assert_eq!(new.walls, old.walls, "cornice remains supported and its original solid stays exact");
        let (old, new) = (old.roof.unwrap(), new.roof.unwrap());
        assert_eq!(old.drainage.iter().map(|p| &p.id).collect::<Vec<_>>(), new.drainage.iter().map(|p| &p.id).collect::<Vec<_>>());
        let dz = 200. / 46_f64.to_radians().cos() - 20.;
        for (a, b) in old.drainage.iter().zip(&new.drainage).filter(|(p, _)| p.id.contains("gutter")) {
            assert_eq!(a.mesh.triangles, b.mesh.triangles);
            for (a, b) in a.mesh.positions.iter().zip(&b.mesh.positions) {
                assert_eq!([a[0], a[1]], [b[0], b[1]]);
                assert!((b[2] - a[2] - dz).abs() < 1e-6);
            }
        }
        let pipe = new.drainage.iter().find(|p| p.id.contains("pipe")).unwrap();
        assert!(pipe.mesh.positions.iter().any(|p| p[2].abs() < 1e-6));
        if let Roof::Pitched { overhang_mm, .. } = doc.buildings[0].roof.base_mut() {
            *overhang_mm = 360.;
        }
        let error = crate::build(&doc).unwrap_err().to_string();
        assert!(error.contains("b.f1") && error.contains("393.718943"), "{error}");
    }

    #[test]
    fn inboard_and_wall_line_openings_keep_real_closed_voids_clear_of_the_return() {
        let mut source = source();
        source["dormers"] = serde_json::json!([{"id":"d","facade":"b.f1","u_mm":8000.,"window_type":"window.casement.2","width_mm":1100.,"height_mm":1200.,"setback_mm":350.}]);
        source["rooflights"] = serde_json::json!([{"id":"r","facade":"b.f1","u_mm":3000.,"run_mm":1000.,"window_type":"window.casement.1","width_mm":800.,"height_mm":1100.,"stand_off_mm":100.,"flashing_mm":100.}]);
        let mut doc = document(source);
        let old = crate::build(&doc).unwrap().remove(0).roof.unwrap();
        doc.buildings[0].roof = doc.buildings[0].roof.clone().with_two_cut_plumb(Some(20.)).unwrap();
        let new = crate::build(&doc).unwrap().remove(0).roof.unwrap();
        assert_eq!(new.dormers, old.dormers);
        assert_eq!(new.rooflights, old.rooflights);
        super::super::halfhip::closed(&new.cover).unwrap();
        // The wall-bearing guard keeps the return outboard; a normal through-void
        // travels inward from even a wall-line window and remains fully supported.
        if let Roof::Pitched { overhang_mm, .. } = doc.buildings[0].roof.base_mut() {
            *overhang_mm = 270.;
        }
        doc.buildings[0].roof.rooflights_mut().unwrap()[0].run_mm = 0.;
        let near_wall = crate::build(&doc).unwrap().remove(0).roof.unwrap();
        super::super::halfhip::closed(&near_wall.cover).unwrap();
        assert_eq!(near_wall.rooflights.len(), 1);
    }

    fn height(mesh: &Mesh, xy: [f64; 2], top: bool) -> f64 {
        for tri in &mesh.triangles {
            let [a, b, c] = tri.map(|i| mesh.positions[i as usize]);
            let det = (b[0] - a[0]) * (c[1] - a[1]) - (b[1] - a[1]) * (c[0] - a[0]);
            if (top && det <= 1e-6) || (!top && det >= -1e-6) {
                continue;
            }
            let u = ((xy[0] - a[0]) * (c[1] - a[1]) - (xy[1] - a[1]) * (c[0] - a[0])) / det;
            let v = ((b[0] - a[0]) * (xy[1] - a[1]) - (b[1] - a[1]) * (xy[0] - a[0])) / det;
            if u >= -1e-9 && v >= -1e-9 && u + v <= 1. + 1e-9 {
                return a[2] + u * (b[2] - a[2]) + v * (c[2] - a[2]);
            }
        }
        panic!("missing roof skin at {xy:?}, top={top}");
    }

    #[test]
    fn two_cut_plumb_has_a_twenty_mm_fascia_and_flat_return_without_moving_the_top() {
        let base = serde_json::json!({"kind":"pitched","pitch_deg":46.,"gables":["b.f2","b.f4"],"straighten_mm":0.,"overhang_mm":400.,"verge_mm":80.,"thickness_mm":200.});
        let source = serde_json::json!({"kind":"two_cut_plumb","fascia_depth_mm":20.,"roof":base});
        let mut b = Building::new("b", "Two-cut plumb", &[[0., 0.], [6000., 0.], [6000., 4000.], [0., 4000.]], 3, 3000.).unwrap();
        b.roof = serde_json::from_value::<Roof>(base.clone()).unwrap();
        let mut doc = Document::default();
        doc.buildings.push(b);
        let old = crate::build(&doc).unwrap().remove(0).roof.unwrap();
        doc.buildings[0].roof = serde_json::from_value(source.clone()).unwrap();
        assert_eq!(serde_json::to_value(&doc.buildings[0].roof).unwrap(), source);
        let new = crate::build(&doc).unwrap().remove(0).roof.unwrap();
        let slope = 46_f64.to_radians().tan();
        let lift = 200. / 46_f64.to_radians().cos();
        let toe = 9000. - 400. * slope + lift - 20.;
        for y in [-400., -350., -250., -200., -100., 0., 500., 1000.] {
            assert!((height(&new.cover, [3000., y], true) - height(&old.cover, [3000., y], true)).abs() < 1e-6);
            assert!((height(&new.cover, [3000., y], false) - (9000. + y * slope).max(toe)).abs() < 1e-6);
        }
        assert!((height(&new.cover, [3000., -400.], true) - height(&new.cover, [3000., -400.], false) - 20.).abs() < 1e-6);
        assert!((height(&new.cover, [3000., -350.], false) - height(&new.cover, [3000., -250.], false)).abs() < 1e-6);
        assert!((height(&new.cover, [3000., 0.], false) - 9000.).abs() < 1e-6);
        assert!((new.ridge - old.ridge).abs() < 1e-6);
        super::super::halfhip::closed(&new.cover).unwrap();
        doc.buildings[0].roof = serde_json::from_value(base).unwrap();
        assert_eq!(crate::build(&doc).unwrap().remove(0).roof.unwrap(), old);
    }
}
