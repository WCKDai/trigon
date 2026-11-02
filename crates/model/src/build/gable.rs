//! Real attic gable windows, hosted by bounded masonry below the unopened roof.
use super::dormer::{local, plane};
use super::roof_opening;
use super::*;

const EPS: f64 = 1e-6;
const MAX_WINDOWS: usize = 32;

/// A source facade's closed, roof-clipped attic masonry and its normal window openings.
#[derive(Debug, Clone, PartialEq)]
pub struct BuiltGable {
    pub source_facade: String,
    pub front: BuiltWall,
    /// Unopened, closed host for exact gross quantities and exchange metadata.
    pub gross: Mesh,
    /// Convex gross front outline in facade (u, global Z) millimetres.
    pub profile: Vec<[f64; 2]>,
    pub thickness: f64,
    pub level: usize,
}

/// Low attic knee-wall openings retain the ordinary wall construction unchanged.
pub(super) fn candidate(b: &Building, level: usize, rect: Rect) -> bool {
    b.attic_level() == Some(level) && rect.v1 > b.height() - LINTEL
}

pub(super) fn validate_request(b: &Building, f: &Facade, p: &Placement, ty: &ComponentType, rect: Rect) -> Result<bool> {
    if !candidate(b, p.level, rect) {
        return Ok(false);
    }
    let crate::Roof::HalfHipped { gables, .. } = &b.roof else { return Ok(false) };
    let slot = format!("{} level {} bay {}", f.id, p.level, p.bay);
    if !gables.contains(&f.id) {
        return Err(ModelError::Invalid(format!("{slot}: above-eaves windows require an explicit source gable")));
    }
    if b.footprint.len() != 4 || gables.len() != 2 {
        return Err(ModelError::Invalid(format!(
            "{slot}: gable windows require a convex four-source-edge footprint with two opposite source gables"
        )));
    }
    let edges = gables.iter().map(|id| b.facade(id).map(|g| g.edge)).collect::<Result<Vec<_>>>()?;
    if (edges[0] + 2) % 4 != edges[1] {
        return Err(ModelError::Invalid(format!("{slot}: gable windows require opposite source gables")));
    }
    for k in 0..4 {
        let (a, b, c) = (b.footprint[k], b.footprint[(k + 1) % 4], b.footprint[(k + 2) % 4]);
        if (b[0] - a[0]) * (c[1] - b[1]) - (b[1] - a[1]) * (c[0] - b[0]) <= EPS {
            return Err(ModelError::Invalid(format!("{slot}: gable windows require a strictly convex counter-clockwise source footprint")));
        }
    }
    if p.spans_storeys {
        return Err(ModelError::Invalid(format!("{slot}: an attic gable window cannot span the knee-wall/eaves seam")));
    }
    if !matches!(
        ty.kind,
        ComponentKind::Window { .. }
            | ComponentKind::FixedWindow
            | ComponentKind::HistoricWindow { shape: crate::component::WindowShape::Rectangular, .. }
    ) {
        return Err(ModelError::Invalid(format!("{slot}: gable windows support only rectangular Window, FixedWindow or HistoricWindow types")));
    }
    let deficit = b.height() + LINTEL - rect.v0;
    if deficit > EPS {
        return Err(ModelError::Invalid(format!(
            "{slot}: gable window must stay {LINTEL} mm above the knee-wall/eaves seam at {:.6} mm; sill {:.6} mm has deficit {deficit:.6} mm",
            b.height(),
            rect.v0
        )));
    }
    Ok(true)
}

fn head_limit(cover: &Mesh, fr: &FacadeFrame, r: &Resolved, thickness: f64, source: &str) -> Result<f64> {
    let slot = format!("{source} level {} bay {}", r.placement.level, r.placement.bay);
    let mut limit = f64::INFINITY;
    // The hypograph of the eligible convex roof is convex: checking all front/back
    // corners bounds the whole rectangular aperture through the masonry thickness.
    for u in [r.rect.u0, r.rect.u1] {
        for w in [0., thickness] {
            let under = plane(cover, fr, [u, w], false)
                .map_err(|e| ModelError::Invalid(format!("{slot}: unsupported gable roof underside at u={u:.6} mm, w={w:.6} mm: {e}")))?
                .z(u, w);
            let deficit = r.rect.v1 + LINTEL - under;
            if !under.is_finite() || deficit > EPS {
                return Err(ModelError::Invalid(format!(
                    "{slot}: gable window head {:.6} mm needs {LINTEL} mm below the roof underside {:.6} mm at u={u:.6} mm, w={w:.6} mm; deficit {deficit:.6} mm",
                    r.rect.v1, under
                )));
            }
            limit = limit.min(under);
        }
    }
    Ok(limit)
}

fn front_profile(gross: &Mesh, fr: &FacadeFrame) -> Result<Vec<[f64; 2]>> {
    let mut points: Vec<_> = gross
        .positions
        .iter()
        .filter_map(|&p| {
            let p = local(fr, p);
            (p[1].abs() <= EPS).then_some([p[0], p[2]])
        })
        .collect();
    points.sort_by(|a, b| a[0].total_cmp(&b[0]).then(a[1].total_cmp(&b[1])));
    points.dedup_by(|a, b| (a[0] - b[0]).abs() <= EPS && (a[1] - b[1]).abs() <= EPS);
    let cross = |a: [f64; 2], b: [f64; 2], c: [f64; 2]| (b[0] - a[0]) * (c[1] - b[1]) - (b[1] - a[1]) * (c[0] - b[0]);
    // Eligibility was explicitly checked before construction. This hull extracts the
    // convex front boundary from its triangulation stations; it never repairs a mesh.
    // Build the two chains separately so the reversed walk cannot pop the lower chain.
    let mut lower: Vec<[f64; 2]> = Vec::new();
    let mut upper: Vec<[f64; 2]> = Vec::new();
    for (chain, reverse) in [(&mut lower, false), (&mut upper, true)] {
        for k in 0..points.len() {
            let p = points[if reverse { points.len() - 1 - k } else { k }];
            while chain.len() >= 2 && cross(chain[chain.len() - 2], chain[chain.len() - 1], p) <= EPS {
                chain.pop();
            }
            chain.push(p);
        }
        chain.pop();
    }
    lower.extend(upper);
    if lower.len() < 3 || crate::doc::polygon_area(&lower) <= EPS {
        return Err(ModelError::Invalid("gable host has no convex gross front profile".into()));
    }
    Ok(lower)
}

fn remove_legacy(gables: &mut Mesh, fr: &FacadeFrame, source: &str) -> Result<()> {
    let old = gables.triangles.len();
    gables.triangles.retain(|tri| {
        !tri.iter().all(|&i| {
            gables.positions.get(i as usize).is_some_and(|&p| {
                let p = local(fr, p);
                p[1].abs() <= EPS && p[0] >= -EPS && p[0] <= fr.length + EPS
            })
        })
    });
    if gables.triangles.len() == old {
        return Err(ModelError::Invalid(format!("{source}: no legacy source gable faces match the real host")));
    }
    // Retain exact coordinates and source vertex order; only indices are compacted.
    let mut used = vec![false; gables.positions.len()];
    for tri in &gables.triangles {
        for &i in tri {
            if let Some(v) = used.get_mut(i as usize) {
                *v = true;
            }
        }
    }
    let mut indices = vec![0; used.len()];
    let mut positions = Vec::new();
    for (i, (&used, &point)) in used.iter().zip(&gables.positions).enumerate() {
        if used {
            indices[i] = positions.len() as u32;
            positions.push(point);
        }
    }
    for tri in &mut gables.triangles {
        for i in tri {
            *i = indices[*i as usize];
        }
    }
    gables.positions = positions;
    Ok(())
}

pub(super) fn build(doc: &Document, b: &Building, view: View, cut: Option<f64>, cover: &Mesh, legacy: &mut Mesh) -> Result<Vec<BuiltGable>> {
    if !matches!(b.roof, crate::Roof::HalfHipped { .. }) {
        return Ok(Vec::new());
    }
    let Some(level) = b.attic_level() else { return Ok(Vec::new()) };
    let mut built = Vec::new();
    for f in &b.facades {
        let all = resolve_facade(doc, b, f)?;
        let requests: Vec<_> = all.iter().filter(|r| r.is_opening() && candidate(b, r.placement.level, r.rect)).collect();
        if requests.is_empty() {
            continue;
        }
        let (a, e) = b.edge(f.edge).ok_or_else(|| ModelError::Invalid(format!("{}: gable source edge is absent", f.id)))?;
        let fr = FacadeFrame::new(a, e).ok_or_else(|| ModelError::Invalid(format!("{}: gable source edge has zero length", f.id)))?;
        let thickness = b.edge_thickness(f.edge, level);
        let requests = requests.into_iter().map(|r| head_limit(cover, &fr, r, thickness, &f.id).map(|head| (r, head))).collect::<Result<Vec<_>>>()?;
        let shown: Vec<_> = requests
            .into_iter()
            .filter(|(r, _)| !cut.is_some_and(|z| r.rect.v0 >= z - 1.))
            .filter_map(|(r, head)| view.status(r.lifetime).map(|status| (r, head, status)))
            .collect();
        if shown.is_empty() {
            continue;
        }
        if shown.len() > MAX_WINDOWS {
            return Err(ModelError::Invalid(format!("{}: at most {MAX_WINDOWS} visible gable windows are supported", f.id)));
        }
        let inner = b.inner_line(level);
        let (Some(&start), Some(&end)) = (inner.get(f.edge), inner.get((f.edge + 1) % b.footprint.len())) else {
            return Err(ModelError::Invalid(format!("{}: gable mitred inner line is absent", f.id)));
        };
        let u = |p: [f64; 2]| (p[0] - a[0]) * fr.t[0] + (p[1] - a[1]) * fr.t[1];
        let band = [[0., 0.], [fr.length, 0.], [u(end), thickness], [u(start), thickness]];
        let gross = roof_opening::below_roof(cover, &fr, &band, b.height()).map_err(|e| ModelError::Invalid(format!("{}: {e}", f.id)))?;
        let profile = front_profile(&gross, &fr)?;
        let height = profile.iter().map(|p| p[1]).fold(b.height(), f64::max);
        let finish = f.finish.unwrap_or(b.finish).material();
        let rectangles: Vec<_> = shown.iter().map(|(r, _, _)| r.rect).collect();
        let net = if let [rect] = rectangles.as_slice() {
            roof_opening::front_window(&gross, &fr, *rect, 0., thickness)?
        } else {
            roof_opening::front_windows(&gross, &fr, &rectangles, 0., thickness)?
        };
        let mut openings = Vec::new();
        for (r, head, status) in shown {
            let id = format!("{}.L{}.B{}", f.id, level, r.placement.bay);
            let elevation = b.storeys[level].elevation;
            let mut opening = component(&fr, &id, r, elevation, status, Fill { wall: thickness, finish, floor: elevation, ceiling: head });
            if cut.is_some() {
                opening.fill = clip_element(opening.fill, cut);
            }
            openings.push(opening);
        }
        crate::roof::halfhip::closed(&net).map_err(|e| ModelError::Invalid(format!("{}: net gable host: {e}", f.id)))?;
        remove_legacy(legacy, &fr, &f.id)?;
        let mesh = cut.map_or_else(|| net.clone(), |z| net.below(z));
        built.push(BuiltGable {
            source_facade: f.id.clone(),
            gross,
            profile,
            thickness,
            level,
            front: BuiltWall {
                id: format!("{}.gable", f.id),
                frame: fr,
                kind: f.kind,
                height,
                bottom: b.height(),
                mesh,
                segments: Vec::new(),
                plinth: Mesh::default(),
                trim: Mesh::default(),
                trim_parts: Vec::new(),
                finish,
                openings,
                attachments: Vec::new(),
            },
        });
    }
    Ok(built)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn document(shear: f64, rotation: f64, offset: [f64; 2]) -> Document {
        let (sin, cos) = rotation.to_radians().sin_cos();
        let outline = [[0., 0.], [30000., 0.], [30000., 12000.], [0., 12000.]].map(|[x, y]| {
            let x = x + shear * y;
            [offset[0] + x * cos - y * sin, offset[1] + x * sin + y * cos]
        });
        let mut b = Building::new("b", "Synthetic attic gable", &outline, 3, 3000.).unwrap();
        b.roof = serde_json::from_value(serde_json::json!({"kind":"half_hipped","pitch_deg":45,
            "gables":["b.f2","b.f4"],"half_hips":[{"facade":"b.f4","drop_mm":1200}],
            "thickness_mm":200,"overhang_mm":180,"verge_mm":135}))
        .unwrap();
        b.set_attic(Some(8250.), Some("Roof")).unwrap();
        b.facades[3].bays = vec![4000., 8000.];
        let mut doc = Document::default();
        doc.buildings.push(b);
        doc
    }

    fn window(bay: usize, u: f64) -> Placement {
        serde_json::from_value(serde_json::json!({"level":3,"bay":bay,"type":"window.casement.2", "u":u,
            "width":1200,"height":1000,"sill":2700}))
        .unwrap()
    }

    fn volume(mesh: &Mesh) -> f64 {
        let origin = mesh.positions[0];
        mesh.triangles
            .iter()
            .map(|t| {
                let [a, b, c] = t.map(|i| std::array::from_fn::<_, 3, _>(|k| mesh.positions[i as usize][k] - origin[k]));
                (a[0] * (b[1] * c[2] - b[2] * c[1]) + a[1] * (b[2] * c[0] - b[0] * c[2]) + a[2] * (b[0] * c[1] - b[1] * c[0])) / 6.
            })
            .sum()
    }

    fn normal_hits(mesh: &Mesh, fr: &FacadeFrame, u: f64, z: f64) -> usize {
        mesh.triangles
            .iter()
            .filter(|t| {
                let [a, b, c] = t.map(|i| local(fr, mesh.positions[i as usize]));
                let det = (b[0] - a[0]) * (c[2] - a[2]) - (b[2] - a[2]) * (c[0] - a[0]);
                if det.abs() < 1e-8 {
                    return false;
                }
                let s = ((u - a[0]) * (c[2] - a[2]) - (z - a[2]) * (c[0] - a[0])) / det;
                let t = ((b[0] - a[0]) * (z - a[2]) - (b[2] - a[2]) * (u - a[0])) / det;
                s >= -1e-8 && t >= -1e-8 && s + t <= 1. + 1e-8
            })
            .count()
    }

    #[test]
    fn rotated_skew_gables_follow_mitres_and_roof_underside_without_changing_legacy_walls() {
        for (shear, rotation, offset) in [(0., 0., [0., 0.]), (0.17, 37.25, [150000., -270000.]), (-0.12, -118., [-30000., 41000.])] {
            let mut doc = document(shear, rotation, offset);
            doc.buildings[0].facades[1].finish = Some(crate::doc::WallFinish::YellowBrick);
            doc.buildings[0].facades[3].finish = Some(crate::doc::WallFinish::Render);
            let baseline = crate::build(&doc).unwrap();
            let legacy = baseline[0].roof.as_ref().unwrap();
            assert!(legacy.gable_walls.is_empty());
            doc.buildings[0].facades[3].placements.push(window(0, 6000.));
            let source = serde_json::to_vec(&doc).unwrap();
            let built = crate::build(&doc).unwrap();
            let roof = built[0].roof.as_ref().unwrap();
            assert_eq!(source, serde_json::to_vec(&doc).unwrap());
            assert_eq!(built[0].walls, baseline[0].walls);
            assert_eq!(roof.cover, legacy.cover);
            assert_eq!(roof.gutters, legacy.gutters);
            assert_eq!(roof.ridge, legacy.ridge);
            assert_eq!(roof.gable_walls.len(), 1);
            let host = &roof.gable_walls[0];
            assert_eq!(host.source_facade, "b.f4");
            assert_eq!(host.level, 3);
            assert_eq!(host.thickness, 400.);
            assert_eq!(host.front.finish, Material::Render);
            assert!(host.front.segments.is_empty());
            crate::roof::halfhip::closed(&host.gross).unwrap();
            crate::roof::halfhip::closed(&host.front.mesh).unwrap();
            assert!((volume(&host.gross) - volume(&host.front.mesh) - 1200. * 1000. * host.thickness).abs() < 1.);
            assert_eq!(normal_hits(&host.front.mesh, &host.front.frame, 6077., 11511.), 0);
            assert!(normal_hits(&host.front.mesh, &host.front.frame, 5100., 11511.) >= 2);
            let fr = &host.front.frame;
            let inner = doc.buildings[0].inner_line(3);
            let u = |p: [f64; 2]| (p[0] - fr.origin[0]) * fr.t[0] + (p[1] - fr.origin[1]) * fr.t[1];
            let (start, end) = (u(inner[3]), u(inner[0]));
            for &p in &host.gross.positions {
                let [u, w, z] = local(fr, p);
                assert!(w >= -EPS && w <= host.thickness + EPS);
                assert!(u >= start * w / host.thickness - EPS);
                assert!(u <= fr.length + (end - fr.length) * w / host.thickness + EPS);
                assert!(z >= roof.eaves - EPS);
                let high = plane(&roof.cover, fr, [u, w], false).unwrap().z(u, w);
                assert!(z <= high + EPS, "masonry must stay below the actual soffit");
            }
            let front_area: f64 = host
                .gross
                .triangles
                .iter()
                .filter_map(|t| {
                    let p = t.map(|i| local(fr, host.gross.positions[i as usize]));
                    p.iter().all(|p| p[1].abs() <= EPS).then(|| crate::doc::polygon_area(&p.map(|p| [p[0], p[2]])))
                })
                .sum();
            assert!((crate::doc::polygon_area(&host.profile) - front_area).abs() < 1e-3);
            for point in &host.profile {
                assert!(host.gross.positions.iter().any(|&p| {
                    let p = local(fr, p);
                    p[1].abs() <= EPS && (p[0] - point[0]).abs() <= EPS && (p[2] - point[1]).abs() <= EPS
                }));
            }
            assert!(roof.gable_parts.iter().all(|(m, _)| *m == Material::BrickYellow));
            assert!(roof.gables.positions.iter().all(|&p| local(fr, p)[1].abs() > 1.));
            let reopened: Document = serde_json::from_slice(&source).unwrap();
            assert_eq!(built, crate::build(&reopened).unwrap());
            doc.buildings[0].facades[3].placements.clear();
            assert_eq!(crate::build(&doc).unwrap(), baseline, "removal restores the full legacy roof bytes");
        }
    }

    #[test]
    fn multiple_windows_keep_distinct_voids_and_normal_fills() {
        let mut doc = document(0., 0., [0., 0.]);
        doc.buildings[0].facades[3].placements = vec![window(0, 4500.), window(1, 7500.)];
        doc.buildings[0].facades[3].placements[1].type_id = "window.fixed".into();
        let built = crate::build(&doc).unwrap();
        let gable = &built[0].roof.as_ref().unwrap().gable_walls[0];
        assert_eq!(gable.front.openings.len(), 2);
        assert_eq!(gable.front.openings[0].fill.id, "b.f4.L3.B0");
        assert_eq!(gable.front.openings[1].fill.id, "b.f4.L3.B1");
        crate::roof::halfhip::closed(&gable.front.mesh).unwrap();
        for u in [4507., 7507.] {
            assert_eq!(normal_hits(&gable.front.mesh, &gable.front.frame, u, 11511.), 0);
        }
        assert!(normal_hits(&gable.front.mesh, &gable.front.frame, 6007., 11511.) >= 2);
        assert!((volume(&gable.gross) - volume(&gable.front.mesh) - 2. * 1200. * 1000. * gable.thickness).abs() < 1.);
        assert_eq!(built[0].exterior_walls().filter(|w| w.id == "b.f4.gable").count(), 1);
    }

    #[test]
    fn ordinary_and_lower_knee_wall_windows_stay_on_their_original_hosts() {
        let mut doc = document(0., 0., [0., 0.]);
        let mut knee = window(0, 6000.);
        knee.height = Some(400.);
        knee.sill = Some(100.);
        let mut ordinary = window(1, 8000.);
        ordinary.level = 0;
        ordinary.sill = Some(900.);
        doc.buildings[0].facades[3].placements = vec![knee, ordinary];
        let baseline = crate::build(&doc).unwrap();
        assert!(baseline[0].roof.as_ref().unwrap().gable_walls.is_empty());
        assert_eq!(baseline[0].walls[3].openings.len(), 2);
        doc.buildings[0].facades[3].placements.push(window(1, 6000.));
        let built = crate::build(&doc).unwrap();
        assert_eq!(built[0].walls, baseline[0].walls);
        let host = &built[0].roof.as_ref().unwrap().gable_walls[0];
        assert_eq!(host.front.openings.len(), 1);
        assert_eq!(host.front.openings[0].fill.id, "b.f4.L3.B1");
        assert_eq!(built[0].exterior_walls().flat_map(|w| &w.openings).filter(|o| o.fill.id == "b.f4.L3.B0").count(), 1);
        doc.buildings[0].facades[3].placements.pop();
        assert_eq!(crate::build(&doc).unwrap(), baseline);
    }

    #[test]
    fn gable_window_seams_types_and_source_scope_are_explicit() {
        for (change, expected) in [(0, "deficit"), (1, "rectangular"), (2, "cannot span"), (3, "explicit source gable"), (4, "party wall")] {
            let mut doc = document(0., 0., [0., 0.]);
            let mut p = window(0, 6000.);
            if change == 0 {
                p.sill = Some(600.);
            }
            if change == 1 {
                p.type_id = "door.french".into();
            }
            if change == 2 {
                p.spans_storeys = true;
            }
            let f = if change == 3 { 0 } else { 3 };
            doc.buildings[0].facades[f].bays = vec![6000.];
            if change == 4 {
                doc.buildings[0].facades[f].kind = FacadeKind::Party;
            }
            doc.buildings[0].facades[f].placements.push(p);
            let err = crate::build(&doc).unwrap_err().to_string();
            assert!(err.contains(expected) && err.contains(&format!("b.f{}", f + 1)), "{err}");
        }
        let mut doc = document(0., 0., [0., 0.]);
        let mut family = doc.component("window.casement.2").unwrap().clone();
        family.id = "window.round".into();
        family.kind = ComponentKind::HistoricWindow {
            shape: crate::component::WindowShape::Round,
            lights: 1,
            top_light: 0.,
            panes_across: 1,
            panes_down: 1,
            muntins: crate::component::Muntins::TrueDivided,
            muntin_width_mm: 22.,
            operable: false,
        };
        doc.types.push(family);
        let mut p = window(0, 6000.);
        p.type_id = "window.round".into();
        p.width = Some(1000.);
        doc.buildings[0].facades[3].placements.push(p);
        let err = crate::build(&doc).unwrap_err().to_string();
        assert!(err.contains("rectangular"), "{err}");
        doc.types.last_mut().unwrap().kind = ComponentKind::HistoricWindow {
            shape: crate::component::WindowShape::Rectangular,
            lights: 1,
            top_light: 0.,
            panes_across: 1,
            panes_down: 1,
            muntins: crate::component::Muntins::TrueDivided,
            muntin_width_mm: 22.,
            operable: false,
        };
        assert_eq!(crate::build(&doc).unwrap()[0].roof.as_ref().unwrap().gable_walls[0].front.openings.len(), 1);
    }

    #[test]
    fn gable_head_clearance_checks_the_back_of_a_skew_wall() {
        let mut doc = document(0.17, 0., [0., 0.]);
        let baseline = crate::build(&doc).unwrap();
        let cover = &baseline[0].roof.as_ref().unwrap().cover;
        let fr = baseline[0].walls[3].frame;
        let mut p = window(0, 8000.);
        let front = [7400., 8600.].map(|u| plane(cover, &fr, [u, 0.], false).unwrap().z(u, 0.)).into_iter().fold(f64::INFINITY, f64::min);
        let back = [7400., 8600.].map(|u| plane(cover, &fr, [u, 400.], false).unwrap().z(u, 400.)).into_iter().fold(f64::INFINITY, f64::min);
        assert!(back < front - 10., "this synthetic skew must expose a back-only collision");
        p.height = Some(front - LINTEL - 10. - (8250. + 2700.));
        doc.buildings[0].facades[3].placements.push(p);
        let err = crate::build(&doc).unwrap_err().to_string();
        assert!(err.contains("b.f4 level 3 bay 0") && err.contains("w=400.000000") && err.contains("deficit"), "{err}");
        doc.buildings[0].facades[3].placements[0].height = Some(back - LINTEL - 10. - (8250. + 2700.));
        assert!(crate::build(&doc).is_ok());
    }

    #[test]
    fn selected_four_edge_roof_does_not_expand_the_gable_window_source_scope() {
        let mut doc = document(0., 0., [0., 0.]);
        let ring = [[0., 0.], [4000., 0.], [4000., 800.], [10000., 800.], [10000., 0.], [24000., 0.], [24000., 10000.], [0., 10000.]];
        let mut b = Building::new("b", "Synthetic source scope", &ring, 3, 3000.).unwrap();
        b.roof = serde_json::from_value(serde_json::json!({"kind":"half_hipped","pitch_deg":45,
            "eave_facades":["b.f1","b.f6","b.f7","b.f8"],"gables":["b.f6","b.f8"],"half_hips":[{"facade":"b.f8","drop_mm":1200}],
            "thickness_mm":200,"overhang_mm":180,"verge_mm":135}))
        .unwrap();
        b.set_attic(Some(8250.), Some("Roof")).unwrap();
        b.facades[7].bays = vec![5000.];
        doc.buildings[0] = b;
        assert!(crate::build(&doc).is_ok(), "half-hip caps keep their existing selected-eave support");
        doc.buildings[0].facades[7].placements.push(window(0, 5000.));
        let err = crate::build(&doc).unwrap_err().to_string();
        assert!(err.contains("b.f8") && err.contains("four-source-edge footprint"), "{err}");
    }

    #[test]
    fn phase_views_cut_the_union_of_identical_nested_and_differing_replacements() {
        for (u, width, height, sill) in
            [(6000., 1200., 1000., 2700.), (6000., 800., 700., 2800.), (6500., 1400., 1100., 2600.), (6500., 1200., 1000., 2800.)]
        {
            let mut doc = document(0., 0., [0., 0.]);
            let legacy = crate::build(&doc).unwrap();
            doc.phases.push(crate::doc::Phase { id: "new".into(), name: "New".into() });
            let mut old = window(0, 6000.);
            old.demolished = Some("new".into());
            let mut new = window(1, u);
            new.width = Some(width);
            new.height = Some(height);
            new.sill = Some(sill);
            new.created = Some("new".into());
            doc.buildings[0].facades[3].placements = vec![old, new];
            for (phase, filter, count) in
                [(0, PhaseFilter::Complete, 1), (1, PhaseFilter::Complete, 1), (1, PhaseFilter::All, 2), (1, PhaseFilter::Demolition, 1)]
            {
                let built = build_view(&doc, View { phase, filter }).unwrap();
                let host = &built[0].roof.as_ref().unwrap().gable_walls[0];
                crate::roof::halfhip::closed(&host.gross).unwrap();
                crate::roof::halfhip::closed(&host.front.mesh).unwrap();
                assert_eq!(host.front.openings.len(), count);
                let openings = &host.front.openings;
                for o in openings {
                    assert_eq!(o.fill.status, o.status);
                    assert_eq!(o.fill.id, format!("b.f4.L3.B{}", if o.status == Status::New { 1 } else { 0 }));
                    assert_eq!(
                        normal_hits(&host.front.mesh, &host.front.frame, (o.rect.u0 + o.rect.u1) / 2. + 7., (o.rect.v0 + o.rect.v1) / 2. + 11.),
                        0
                    );
                }
                let mut area: f64 = openings.iter().map(|o| (o.rect.u1 - o.rect.u0) * (o.rect.v1 - o.rect.v0)).sum();
                if openings.len() == 2 {
                    let (a, b) = (openings[0].rect, openings[1].rect);
                    area -= (a.u1.min(b.u1) - a.u0.max(b.u0)).max(0.) * (a.v1.min(b.v1) - a.v0.max(b.v0)).max(0.);
                    assert_eq!(openings[0].status, Status::Demolished);
                    assert_eq!(openings[1].status, Status::New);
                }
                assert!((volume(&host.gross) - volume(&host.front.mesh) - area * host.thickness).abs() < 1.);
            }
            let hidden = build_view(&doc, View { phase: 0, filter: PhaseFilter::New }).unwrap();
            assert_eq!(hidden[0].roof.as_ref().unwrap().gables, legacy[0].roof.as_ref().unwrap().gables);
            assert!(hidden[0].roof.as_ref().unwrap().gable_walls.is_empty());
            doc.buildings[0].facades[3].placements[1].height = Some(3000.);
            let err = build_view(&doc, View { phase: 0, filter: PhaseFilter::Complete }).unwrap_err().to_string();
            assert!(err.contains("head") && err.contains("deficit"), "even hidden sources must validate: {err}");
        }
    }

    #[test]
    fn cut_display_keeps_full_gross_profile_and_clips_the_visible_host_and_fill() {
        let mut doc = document(0., 0., [0., 0.]);
        doc.buildings[0].facades[3].placements.push(window(0, 6000.));
        let full = crate::build(&doc).unwrap();
        let original = &full[0].roof.as_ref().unwrap().gable_walls[0];
        let cut = build_cut(&doc, View::latest(&doc), Some(11500.)).unwrap();
        let host = &cut[0].roof.as_ref().unwrap().gable_walls[0];
        assert_eq!(host.gross, original.gross);
        assert_eq!(host.profile, original.profile);
        assert!(host.front.mesh.positions.iter().all(|p| p[2] <= 11500.));
        assert!(host.front.openings[0].fill.parts.iter().flat_map(|(_, m)| &m.positions).all(|p| p[2] <= 11500.));
        assert!(build_cut(&doc, View::latest(&doc), Some(8500.)).unwrap()[0].roof.is_none());
    }
}
