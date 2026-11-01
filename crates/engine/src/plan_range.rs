//! Explicit floor-relative plan ranges use the same deterministic meshes, plane cutter and
//! hidden-line projector as sections. Legacy symbolic plans do not enter this path.
use crate::sheet::project::{Class, Look, Near, Source, project};
use crate::sheet::{Fill, Layer, Scene, Sheet, Stroke, Viewport, cut, style};
use crate::{DrawingElementRegion, Result};
use serde_json::json;
use trigon_model::annotation::PlanRange;
use trigon_model::build::{View, build_view};
use trigon_model::mesh::{Material, Mesh};
use trigon_model::{Document, ModelError};

pub(crate) fn sheet(doc: &Document, level: usize, phase: View, range: PlanRange) -> Result<Sheet> {
    range.validate()?;
    let built = build_view(doc, phase)?;
    let first = built.first().ok_or_else(|| ModelError::Invalid("a plan needs a building".into()))?;
    let storey = first.storeys.get(level).ok_or_else(|| ModelError::Invalid(format!("plan level {level} does not exist")))?;
    let floor = storey.elevation;
    let (top, plane, bottom, depth) = (floor + range.top_mm, floor + range.cut_mm, floor + range.bottom_mm, floor + range.depth_mm);
    let parts = style::parts(&built, true);
    let mut caps = Vec::new();
    let mut cuts = Vec::new();
    let mut cut_ids = std::collections::BTreeSet::new();
    let mut cut_regions = Vec::new();
    for part in parts.iter().filter(|p| p.role != style::Role::Seen) {
        let mut segments = Vec::new();
        cut::segments(part.source.mesh, &|q| [q[0], q[1], q[2] - plane], &mut segments);
        if segments.is_empty() {
            continue;
        }
        let chains = cut::chain(&segments, 0.5, 0.5);
        let shapes = cut::even_odd(&chains.closed);
        caps.extend(shapes.iter().cloned());
        if let Some(element) = &part.source.element {
            cut_ids.insert(element.id.clone());
            for shape in &shapes {
                cut_regions.push(DrawingElementRegion {
                    id: element.id.clone(),
                    kind: element.kind,
                    rings: shape.clone(),
                    depth_plane: Some([-plane, 0.0, 0.0]),
                });
            }
        }
        cuts.push((part, segments, shapes, chains.open));
    }
    // Rotate a private mesh copy so the existing projector sees x/y with depth=-z. Model data
    // and source IDs remain untouched. Both range boundaries clip actual triangles/feature edges.
    let meshes: Vec<Mesh> = parts
        .iter()
        .map(|p| Mesh {
            positions: p.source.mesh.positions.iter().map(|q| [q[0], -q[2], q[1]]).collect(),
            triangles: p.source.mesh.triangles.clone(),
        })
        .collect();
    let sources: Vec<_> =
        parts.iter().zip(&meshes).filter(|(p, _)| p.role != style::Role::Cut).map(|(p, m)| Source { mesh: m, ..p.source.clone() }).collect();
    let look = Look { origin: [0.0, 0.0], t: [1.0, 0.0], d: [0.0, 1.0] };
    let below = project(&sources, &look, (-plane, -depth), Near::Caps(&caps));
    let overhead: Vec<_> = sources.iter().filter(|p| p.element.as_ref().is_none_or(|e| !cut_ids.contains(&e.id))).cloned().collect();
    let above = project(&overhead, &look, (-top, -plane), Near::Cut);
    let mut scene = Scene::new(100.0);
    scene.element_regions.extend(below.element_regions.iter().cloned());
    scene.element_regions.extend(cut_regions);
    for face in &below.faces {
        if let Some(e) = &face.element {
            scene.element_regions.push(DrawingElementRegion {
                id: e.id.clone(),
                kind: e.kind,
                rings: face.rings.clone(),
                depth_plane: Some(face.depth_plane),
            });
        }
    }
    let depth_edges = draw_below(&mut scene, &below, bottom);
    // The cut is opaque and carries the strongest outlines. Source segment ownership survives.
    for (part, segments, shapes, open) in cuts {
        let wall = matches!(part.source.class, Class::Wall | Class::Partition);
        let layer = if wall { Layer::WallCut } else { style::layer(part.source.class) };
        let (fill, pen) = match part.material {
            Material::Glass => (None, 0.18),
            Material::Wall => (Some(Fill::Masonry), 0.5),
            Material::Plaster => (Some(Fill::Solid([220; 3])), if wall { 0.35 } else { 0.25 }),
            _ => (Some(Fill::Solid([235; 3])), 0.25),
        };
        let owners: Vec<_> = part.source.element.iter().flat_map(|e| segments.iter().map(|s| e.segment(*s))).collect();
        for shape in shapes {
            scene.polygon(layer, shape, fill, Some(Stroke::new(pen)));
            scene.attribute_last(&owners);
        }
        for chain in open {
            scene.polyline(layer, chain, Stroke::new(pen));
            scene.attribute_last(&owners);
        }
    }
    for e in &above.edges {
        scene.line(style::layer(e.class), e.a, e.b, Stroke { colour: [130; 3], dashed: true, ..Stroke::new(0.13) });
        scene.attribute_last(&e.element_segments);
    }
    for building in &doc.buildings {
        let Some(storey) = building.storeys.get(level) else { continue };
        if storey.elevation < depth || storey.elevation > plane {
            continue;
        }
        for (index, room) in building.rooms.iter().enumerate().filter(|(_, r)| r.levels.contains(&level)) {
            scene.element_regions.push(DrawingElementRegion {
                id: crate::room_element_id(building, index, level),
                kind: crate::DrawingElementKind::Room,
                rings: vec![room.polygon.clone()],
                depth_plane: Some([-storey.elevation, 0.0, 0.0]),
            });
        }
    }
    // Keep semantic regions only for shapes actually shown; overhead has visible edges but no
    // opaque pick area that could cover a floor or room under it.
    let bounds = scene.bounds().unwrap_or([0.0, 0.0, 1000.0, 1000.0]);
    let clip = [80.0, 140.0, 1440.0, 650.0];
    let vp = Viewport::fit(bounds, clip);
    scene.scale = vp.scale();
    let metrics = json!({"range":range,"floor_mm":floor,"scale":crate::sheet::scale_text(scene.scale),"cut_regions":caps.len(),"depth_edges":depth_edges,"overhead_edges":above.edges.len()});
    // Construction above is read-only; the shared template stage applies paper sizing/crop/pens.
    Ok(Sheet {
        scene,
        vp,
        clip,
        title: storey.name.clone(),
        project: first.name.clone(),
        caption: String::new(),
        note: Some("Dashed: overhead · Dotted: below primary range".into()),
        metrics,
    })
}

fn draw_below(scene: &mut Scene, projection: &crate::sheet::project::Projection, bottom: f64) -> usize {
    let mut deep_count = 0;
    for e in &projection.edges {
        let [za, zb] = e.end_depths.map(|d| -d);
        let mut cuts = vec![0.0, 1.0];
        if (za - bottom) * (zb - bottom) < 0.0 {
            cuts.insert(1, (bottom - za) / (zb - za));
        }
        for pair in cuts.windows(2) {
            let point = |t: f64| [e.a[0] + (e.b[0] - e.a[0]) * t, e.a[1] + (e.b[1] - e.a[1]) * t];
            let deep = za + (zb - za) * (pair[0] + pair[1]) / 2.0 < bottom - 0.001;
            deep_count += usize::from(deep);
            let pen = if deep {
                0.13
            } else if e.outline {
                0.35
            } else {
                0.18
            };
            let stroke = Stroke {
                colour: if deep { [150; 3] } else { [51; 3] },
                pattern: if deep { trigon_model::annotation::LinePattern::Dotted } else { Default::default() },
                ..Stroke::new(pen)
            };
            scene.line(style::layer(e.class), point(pair[0]), point(pair[1]), stroke);
            scene.attribute_last(&e.element_segments);
        }
    }
    deep_count
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::sheet::Item;
    #[test]
    fn sloped_mesh_edges_clip_to_depth_and_split_at_exact_bottom_with_dotted_deep_strokes() {
        let mut mesh = Mesh::default();
        // Already oriented for the XY projector: depth=-z. A rising ramp has its visible top
        // from z=-1000 to z=1000 across x=0..2000, not a drawing-space synthetic line.
        mesh.quad([0.0, 1000.0, 0.0], [2000.0, -1000.0, 0.0], [2000.0, -1000.0, 1000.0], [0.0, 1000.0, 1000.0]);
        let look = Look { origin: [0.0, 0.0], t: [1.0, 0.0], d: [0.0, 1.0] };
        let sources = [Source { mesh: &mesh, class: Class::Stair, group: 0, element: None }];
        let projection = project(&sources, &look, (-1200.0, 500.0), Near::Cut);
        let mut scene = Scene::new(100.0);
        let count = draw_below(&mut scene, &projection, 0.0);
        assert!(count > 0);
        let paths: Vec<_> = scene.items.iter().filter_map(|i| if let Item::Path(p) = i { Some(p) } else { None }).collect();
        assert!(
            paths.iter().flat_map(|p| p.rings.iter().flatten()).all(|p| p[0] >= 500.0 - 0.01),
            "geometry below depth=-500 must be clipped at x=500"
        );
        assert!(paths.iter().any(|p| p.stroke.is_some_and(|s| s.pattern() == trigon_model::annotation::LinePattern::Dotted)
            && p.rings.iter().flatten().any(|p| (p[0] - 1000.0).abs() < 0.01)));
        assert!(paths.iter().any(|p| p.stroke.is_some_and(|s| s.pattern() == trigon_model::annotation::LinePattern::Solid)
            && p.rings.iter().flatten().any(|p| (p[0] - 1000.0).abs() < 0.01)));
        let shallow = project(&sources, &look, (-1200.0, -500.0), Near::Cut);
        assert!(shallow.edges.iter().all(|e| e.a[0] >= 1500.0 - 0.01 && e.b[0] >= 1500.0 - 0.01));
    }
}
