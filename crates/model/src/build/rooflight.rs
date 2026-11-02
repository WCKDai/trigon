//! Inclined windows, normal through-roof voids and fitted flashing.
use super::*;
#[derive(Debug, Clone, PartialEq)]
pub struct RooflightFrame {
    pub origin: [f64; 3],
    pub x: [f64; 3],
    pub up: [f64; 3],
    pub normal: [f64; 3],
}
#[derive(Debug, Clone, PartialEq)]
pub struct BuiltRooflight {
    pub id: String,
    pub opening_id: String,
    pub frame: RooflightFrame,
    pub stand_off: f64,
    pub opening: Mesh,
    pub fill: BuiltElement,
    pub flashing: Mesh,
}
impl RooflightFrame {
    pub fn point(&self, p: [f64; 3]) -> [f64; 3] {
        std::array::from_fn(|k| self.origin[k] + p[0] * self.x[k] + p[1] * self.up[k] + p[2] * self.normal[k])
    }
    pub fn local(&self, p: [f64; 3]) -> [f64; 3] {
        let d = std::array::from_fn::<_, 3, _>(|k| p[k] - self.origin[k]);
        [self.x, self.up, self.normal].map(|a| a.iter().zip(d).map(|(a, b)| a * b).sum())
    }
    fn mesh(&self, mesh: &Mesh, world: bool) -> Mesh {
        Mesh {
            positions: mesh.positions.iter().map(|&p| if world { self.point(p) } else { self.local(p) }).collect(),
            triangles: mesh.triangles.clone(),
        }
    }
}
fn flashing(fr: &RooflightFrame, w: f64, h: f64, width: f64, finish: f64, stand: f64) -> Mesh {
    // A single closed stepped metal curb/apron. The frame's rear seats 1 mm into its top.
    let rows =
        [(width, finish + 2.), (width, finish + 5.), (25., finish + 5.), (25., stand - 70. + 1.), (-40., stand - 70. + 1.), (-40., finish + 2.)];
    let rings: Vec<_> = rows.iter().map(|&(d, z)| [[-d, -d, z], [w + d, -d, z], [w + d, h + d, z], [-d, h + d, z]].map(|p| fr.point(p))).collect();
    let mut mesh = Mesh::default();
    for k in 0..rings.len() {
        for j in 0..4 {
            mesh.quad(rings[k][j], rings[k][(j + 1) % 4], rings[(k + 1) % rings.len()][(j + 1) % 4], rings[(k + 1) % rings.len()][j]);
        }
    }
    mesh
}
pub(super) fn build(doc: &Document, b: &Building, cover: &mut Mesh, dormers: &[BuiltDormer]) -> Result<Vec<BuiltRooflight>> {
    use super::dormer::{Plane, plane};
    use super::roof_opening as cut;
    use std::collections::BTreeSet;
    let specs = b.roof.rooflights();
    if specs.is_empty() {
        return Ok(Vec::new());
    }
    if specs.len() > 64 || b.footprint.len() > 64 {
        return Err(ModelError::Invalid("rooflights support at most 64 instances and 64 footprint corners".into()));
    }
    let original = cover.clone();
    let mut ids = BTreeSet::new();
    let mut reserved: Vec<(String, Vec<[f64; 2]>)> = Vec::new();
    let mut built = Vec::new();
    let identity = FacadeFrame::new([0., 0.], [1000., 0.]).ok_or_else(|| ModelError::Invalid("empty rooflight frame".into()))?;
    for spec in specs {
        spec.validate()?;
        for key in [spec.id.clone(), format!("{}.opening", spec.id), format!("{}.flashing", spec.id)] {
            if !ids.insert(key.clone()) {
                return Err(ModelError::Invalid(format!("rooflight {} collides with generated window/opening/flashing id {key}", spec.id)));
            }
        }
        let f = b.facade(&spec.facade)?;
        let (a, e) = b.edge(f.edge).ok_or_else(|| ModelError::Invalid("rooflight facade has no edge".into()))?;
        let source = FacadeFrame::new(a, e).ok_or_else(|| ModelError::Invalid("rooflight facade is too short".into()))?;
        let (sin, cos) = spec.yaw_deg.to_radians().sin_cos();
        let t = [source.t[0] * cos - source.t[1] * sin, source.t[0] * sin + source.t[1] * cos];
        let anchor = source.point(spec.u_mm, 0., 0.);
        let fr = FacadeFrame { origin: [anchor[0] - spec.u_mm * t[0], anchor[1] - spec.u_mm * t[1]], t, n: [t[1], -t[0]], length: source.length };
        let sample = [spec.u_mm, spec.run_mm];
        let top = plane(&original, &fr, sample, true).map_err(|e| {
            ModelError::Invalid(format!("rooflight {} on {} at u={:.6} mm, run={:.6} mm: {e}", spec.id, spec.facade, spec.u_mm, spec.run_mm))
        })?;
        if top.0[0].abs() > 1e-6 || top.0[1] <= 0. {
            let required = spec.yaw_deg + (-top.0[0]).atan2(top.0[1]).to_degrees();
            return Err(ModelError::Invalid(format!(
                "rooflight {} yaw_deg={:.9} does not align with a rising host plane: along slope={:.9} mm/mm; required yaw_deg={required:.9} deg",
                spec.id, spec.yaw_deg, top.0[0]
            )));
        }
        let scale = (1. + top.0[1] * top.0[1]).sqrt();
        let up = [-fr.n[0] / scale, -fr.n[1] / scale, top.0[1] / scale];
        let normal = [-top.0[1] * up[0], -top.0[1] * up[1], 1. / scale];
        let origin = fr.point(spec.u_mm - spec.width_mm / 2., spec.run_mm, top.z(spec.u_mm - spec.width_mm / 2., spec.run_mm));
        let frame = RooflightFrame { origin, x: [fr.t[0], fr.t[1], 0.], up, normal };
        let host = frame.mesh(&original, false);
        let outer = cut::rect(-spec.flashing_mm, spec.width_mm + spec.flashing_mm, -spec.flashing_mm, spec.height_mm + spec.flashing_mm);
        let bottom = plane(&host, &identity, [spec.width_mm / 2., spec.height_mm / 2.], false).map_err(|e| {
            ModelError::Invalid(format!(
                "rooflight {} on {} at u={:.6} mm, run={:.6} mm: missing underside at slope-plane centre ({:.6}, {:.6}) mm; {e}",
                spec.id,
                spec.facade,
                spec.u_mm,
                spec.run_mm,
                spec.width_mm / 2.,
                spec.height_mm / 2.
            ))
        })?;
        let thickness = -bottom.z(spec.width_mm / 2., spec.height_mm / 2.);
        checked_host(&spec.id, thickness, cut::limits(&host, &identity, &outer, true)?, cut::limits(&host, &identity, &outer, false)?)?;
        let footprint: Vec<_> = outer
            .iter()
            .map(|p| {
                let q = frame.point([p[0], p[1], 0.]);
                [q[0], q[1]]
            })
            .collect();
        for (id, other) in &reserved {
            let area = cut::overlap(&footprint, other)?;
            if area > 1e-6 {
                return Err(ModelError::Invalid(format!("rooflight {} overlaps rooflight {id}: flashing intersection {area:.6} mm²", spec.id)));
            }
        }
        for d in dormers {
            for tri in &d.roof.triangles {
                let p = tri.map(|i| d.roof.positions.get(i as usize).copied().unwrap_or_default());
                let p = p.map(|p| [p[0], p[1]]);
                if cut::area(&p) > 1e-6 {
                    let area = cut::overlap(&footprint, &p)?;
                    if area > 1e-6 {
                        return Err(ModelError::Invalid(format!(
                            "rooflight {} overlaps dormer {}: projected flashing intersection {area:.6} mm²",
                            spec.id, d.id
                        )));
                    }
                }
            }
        }
        reserved.push((spec.id.clone(), footprint));
        let mut ty = doc.component(&spec.window_type)?.clone();
        ty.width = spec.width_mm;
        ty.height = spec.height_mm;
        ty.validate()?;
        let (lights, transom, operable) = match ty.kind {
            ComponentKind::Window { lights, top_light } => (lights, top_light, true),
            ComponentKind::FixedWindow => (1, 0., false),
            _ => return Err(ModelError::Invalid("rooflight fill requires a rectangular Window or FixedWindow family".into())),
        };
        let finish = match &b.roof {
            crate::Roof::Mansard { parameters: p } => {
                p.covering.as_ref().map(|c| c.offset(top.0[1].atan().to_degrees(), p.lower_pitch_deg, p.upper_pitch_deg)).transpose()?.unwrap_or(0.)
            }
            _ => 0.,
        };
        let stand = finish + spec.stand_off_mm;
        let rect = Rect { u0: 0., u1: ty.width, v0: 0., v1: ty.height };
        let mut parts = opening::window(&identity, rect, (0., 70.), lights, transom, operable, false).finish();
        // Family coordinates are (across, inward depth, up); roof-surface coordinates are
        // (across, up-slope, outward normal), a rigid right-handed rotation.
        for (_, mesh) in &mut parts {
            for p in &mut mesh.positions {
                *p = frame.point([p[0], p[2], stand - p[1]]);
            }
        }
        let id = format!("{}.rooflight.{}", b.id, spec.id);
        let opening_id = format!("{id}.opening");
        let aperture = cut::rect(0., ty.width, 0., ty.height);
        let local_cover = frame.mesh(cover, false);
        *cover = frame.mesh(&cut::cut(&local_cover, &host, &identity, &aperture)?, true);
        let opening = frame.mesh(&cut::prism(&identity, &aperture, Plane([0., 0., -thickness]), Plane([0., 0., finish + 2.]))?, true);
        let fill = BuiltElement {
            id: id.clone(),
            name: ty.name,
            type_id: ty.id,
            class: ElementClass::Window,
            level: b.storeys.len().saturating_sub(1),
            width: ty.width,
            height: ty.height,
            parts,
            status: Status::Existing,
        };
        let flashing = flashing(&frame, ty.width, ty.height, spec.flashing_mm, finish, stand);
        built.push(BuiltRooflight { id, opening_id, frame, stand_off: stand, opening, fill, flashing });
    }
    Ok(built)
}
fn checked_host(id: &str, thickness: f64, top: (f64, f64), bottom: (f64, f64)) -> Result<()> {
    if !thickness.is_finite() || thickness < 50. {
        return Err(ModelError::Invalid(format!("rooflight {id} requires at least 50 mm normal host build-up (got {thickness:.6} mm)")));
    }
    if top.0.abs() > 0.01 || top.1.abs() > 0.01 || (bottom.0 + thickness).abs() > 0.01 || (bottom.1 + thickness).abs() > 0.01 {
        return Err(ModelError::Invalid(format!(
            "rooflight {id} crosses a hip or slope break: outer flashing top residual=[{:.6},{:.6}] mm, underside residual=[{:.6},{:.6}] mm exceeds 0.010000 mm",
            top.0,
            top.1,
            bottom.0 + thickness,
            bottom.1 + thickness
        )));
    }
    Ok(())
}
#[cfg(test)]
mod tests {
    use super::*;
    use crate::build::dormer::tests::closed;
    #[test]
    fn invalid_hip_outside_thin_and_hostless_rooflights_are_rejected_with_feedback() {
        let mut b = Building::new("b", "Invalid host checks", &[[0., 0.], [24000., 0.], [24000., 12000.], [0., 12000.]], 3, 3400.).unwrap();
        b.roof=serde_json::from_value(serde_json::json!({"kind":"pitched","pitch_deg":46,"thickness_mm":220,
            "rooflights":[{"id":"light","facade":"b.f1","u_mm":10000,"run_mm":500,"window_type":"window.casement.2","width_mm":800,"height_mm":1000,"stand_off_mm":100,"flashing_mm":100}]})).unwrap();
        let mut doc = Document::default();
        doc.buildings.push(b);
        let valid = doc.clone();
        for (u, run) in [(500., 500.), (10000., 5700.), (10000., 1e8)] {
            doc = valid.clone();
            let d = &mut doc.buildings[0].roof.rooflights_mut().unwrap()[0];
            d.u_mm = u;
            d.run_mm = run;
            let before = doc.clone();
            let error = crate::build(&doc).unwrap_err().to_string();
            assert!(error.contains("support") || error.contains("residual") || error.contains("run="), "{error}");
            assert_eq!(before, doc);
        }
        doc = valid.clone();
        if let crate::Roof::Pitched { thickness_mm, .. } = &mut doc.buildings[0].roof {
            *thickness_mm = 20.;
        }
        assert!(crate::build(&doc).unwrap_err().to_string().contains("50 mm normal host build-up"));
        let mut s = valid.buildings[0].roof.rooflights()[0].clone();
        s.facade = "missing".into();
        doc = valid.clone();
        doc.buildings[0].roof.rooflights_mut().unwrap()[0] = s;
        assert!(crate::build(&doc).is_err());
    }
    #[test]
    fn rooflight_source_is_optional_and_hostile_numbers_are_rejected() {
        for kind in ["pitched", "mansard"] {
            let mut b = Building::new("b", "Legacy roof", &[[0., 0.], [24000., 0.], [24000., 14000.], [0., 14000.]], 3, 3400.).unwrap();
            let mut source = serde_json::json!({"kind":kind,"thickness_mm":220});
            if kind == "pitched" {
                source["pitch_deg"] = serde_json::json!(46);
            }
            b.roof = serde_json::from_value(source.clone()).unwrap();
            let mut doc = Document::default();
            doc.buildings.push(b);
            let legacy = serde_json::to_vec(&doc).unwrap();
            let built = crate::build(&doc).unwrap();
            source["rooflights"] = serde_json::json!([]);
            doc.buildings[0].roof = serde_json::from_value(source).unwrap();
            assert_eq!(serde_json::to_vec(&doc).unwrap(), legacy);
            assert_eq!(crate::build(&doc).unwrap(), built);
        }
        let spec: crate::roof::Rooflight = serde_json::from_value(serde_json::json!({"id":"light","facade":"b.f1","u_mm":10000,"run_mm":500,
            "window_type":"window.casement.2","width_mm":800,"height_mm":1000,"stand_off_mm":100,"flashing_mm":100}))
        .unwrap();
        for field in ["u", "run", "yaw", "width", "height", "stand", "flashing"] {
            for n in [f64::NAN, f64::INFINITY, f64::NEG_INFINITY, f64::MAX, -1e300] {
                let mut d = spec.clone();
                match field {
                    "u" => d.u_mm = n,
                    "run" => d.run_mm = n,
                    "yaw" => d.yaw_deg = n,
                    "width" => d.width_mm = n,
                    "height" => d.height_mm = n,
                    "stand" => d.stand_off_mm = n,
                    _ => d.flashing_mm = n,
                }
                assert!(d.validate().unwrap_err().to_string().contains("rooflight"));
            }
        }
    }
    #[test]
    fn rooflights_keep_normal_cuts_on_transformed_hosts_with_both_yaw_signs() {
        for positive in [false, true] {
            for rotation in [0.0_f64, 37.25] {
                let raw = if positive {
                    [[12000., 0.], [24000., 0.], [24000., 11000.], [0., 11000.], [0., -1000.]]
                } else {
                    [[0., 0.], [12000., 0.], [24000., -1000.], [24000., 11000.], [0., 11000.]]
                };
                let (sin, cos) = rotation.to_radians().sin_cos();
                let outline = raw.map(|[x, y]| [150000. + x * cos - y * sin, -270000. + x * sin + y * cos]);
                let mut b = Building::new("b", "Skew rooflight", &outline, 3, 3400.).unwrap();
                let yaw = if positive { 1. } else { -1. } * (1000.0_f64 / 24000.).atan().to_degrees();
                b.roof = serde_json::from_value(serde_json::json!({"kind":"pitched","pitch_deg":46,"thickness_mm":220,
                "rooflights":[{"id":"skew","facade":"b.f1","u_mm":6000,"run_mm":600,"yaw_deg":yaw,
                "window_type":"window.casement.1","width_mm":800,"height_mm":1000,"stand_off_mm":100,"flashing_mm":100}]}))
                .unwrap();
                let mut doc = Document::default();
                doc.buildings.push(b);
                let built = crate::build(&doc).unwrap();
                let r = built[0].roof.as_ref().unwrap();
                closed(&r.cover);
                closed(&r.rooflights[0].flashing);
                closed(&r.rooflights[0].opening);
                let light = &r.rooflights[0];
                let q = light.frame.point([400., 500., 400.]);
                let back = light.frame.local(q);
                for (a, b) in back.into_iter().zip([400., 500., 400.]) {
                    assert!((a - b).abs() < 1e-6);
                }
                doc.buildings[0].roof.rooflights_mut().unwrap()[0].yaw_deg = 0.;
                let failed = doc.clone();
                assert!(crate::build(&doc).unwrap_err().to_string().contains("required yaw_deg"));
                assert_eq!(failed, doc);
            }
        }
    }
    #[test]
    fn inclined_rooflights_cut_both_skins_and_keep_family_glazing_clear() {
        for kind in ["pitched", "mansard"] {
            let mut b = Building::new("b", "Rooflight fixture", &[[0., 0.], [24000., 0.], [24000., 14000.], [0., 14000.]], 3, 3400.).unwrap();
            let mut source = serde_json::json!({"kind":kind,"thickness_mm":220});
            if kind == "pitched" {
                source["pitch_deg"] = serde_json::json!(46);
            }
            source["rooflights"] = serde_json::json!([{"id":"light","facade":"b.f1","u_mm":10000,"run_mm":500,
                "window_type":"window.casement.2","width_mm":800,"height_mm":1000,"stand_off_mm":100,"flashing_mm":100}]);
            b.roof = serde_json::from_value(source).unwrap();
            let mut doc = Document::default();
            doc.buildings.push(b);
            let mut uncut = doc.clone();
            uncut.buildings[0].roof.rooflights_mut().unwrap().clear();
            let original = crate::build(&uncut).unwrap();
            let built = crate::build(&doc).unwrap();
            let roof = built[0].roof.as_ref().unwrap();
            assert_eq!(roof.rooflights.len(), 1, "source rooflights must generate real parametric assemblies");
            let light = &roof.rooflights[0];
            for mesh in [&roof.cover, &light.opening, &light.flashing] {
                closed(mesh);
            }
            for (mat, mesh) in &light.fill.parts {
                assert!(!mesh.is_empty());
                if *mat != Material::Steel {
                    closed(mesh);
                }
            }
            let fr = FacadeFrame::new([0., 0.], [1000., 0.]).unwrap();
            let before = light.frame.mesh(&original[0].roof.as_ref().unwrap().cover, false);
            let after = light.frame.mesh(&roof.cover, false);
            for top in [true, false] {
                for q in [[2., 2.], [400., 500.], [798., 998.]] {
                    assert!(super::super::dormer::plane(&before, &fr, q, top).is_ok());
                    assert!(super::super::dormer::plane(&after, &fr, q, top).is_err(), "both inclined skins need a real void");
                }
            }
            for (_, glass) in light.fill.parts.iter().filter(|(m, _)| *m == Material::Glass) {
                for &p in &glass.positions {
                    let q = light.frame.local(p);
                    assert!(q[2] > 20., "glazing must clear the actual host");
                }
            }
            let source = serde_json::to_vec(&doc).unwrap();
            let reopened: Document = serde_json::from_slice(&source).unwrap();
            assert_eq!(built, crate::build(&reopened).unwrap());
            let mut overlap = doc.buildings[0].roof.rooflights()[0].clone();
            overlap.id = "overlap".into();
            overlap.u_mm += 100.;
            doc.buildings[0].roof.rooflights_mut().unwrap().push(overlap);
            assert!(crate::build(&doc).unwrap_err().to_string().contains("overlaps"));
        }
    }
}

#[cfg(test)]
mod assembly_tests {
    use super::*;
    use crate::build::dormer::tests::closed;
    #[test]
    fn repeated_rooflights_on_opposite_slopes_preserve_a_closed_gabled_host() {
        let mut b = Building::new("b", "Analytic skew gabled roof", &[[0., 0.], [16000., 900.], [15450., 9800.], [-500., 8900.]], 2, 3570.).unwrap();
        let mut lights = Vec::new();
        for (facade, stations) in [("b.f1", vec![2500., 4800., 12800.]), ("b.f3", vec![8200., 10700., 13900., 1600., 6300.])] {
            for (i, u) in stations.into_iter().enumerate() {
                lights.push(serde_json::json!({"id":format!("{facade}-{i}"),"facade":facade,"u_mm":u,"run_mm":1800.,"window_type":"window.casement.1","width_mm":800.,"height_mm":1100.,"stand_off_mm":100.,"flashing_mm":100.}));
            }
        }
        b.roof=serde_json::from_value(serde_json::json!({"kind":"pitched","pitch_deg":46,"thickness_mm":200.,"overhang_mm":180.,"verge_mm":135.,"gables":["b.f2","b.f4"],"straighten_mm":0.,"rooflights":lights,
            "dormers":[{"id":"large","facade":"b.f1","u_mm":8300.,"window_type":"window.casement.3","width_mm":2600.,"height_mm":1300.,"setback_mm":350.,"cheek_mm":100.,"head_mm":130.,"sill_clearance_mm":80.,"roof_pitch_deg":6.,"roof_thickness_mm":65.}]})).unwrap();
        let mut doc = Document::default();
        doc.buildings.push(b);
        let built = crate::build(&doc).unwrap();
        let r = built[0].roof.as_ref().unwrap();
        assert_eq!(r.rooflights.len(), 8);
        closed(&r.cover);
    }
}
