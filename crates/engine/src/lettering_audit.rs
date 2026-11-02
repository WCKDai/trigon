//! Potential lettering collisions measured with the shared SVG shaper at final physical scale.
use crate::{
    Result, Session,
    sheet::{Halo, Item, MM_PER_PX},
};
use schemars::JsonSchema;
use serde::Deserialize;
use serde_json::{Value, json};
use trigon_model::{ModelError, build::View, checked};

#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub(crate) struct Audit {
    pub id: String,
    #[serde(default = "clearance")]
    pub clearance_mm_paper: f64,
    #[serde(default = "limit")]
    pub max_results: usize,
    /// Plan only: compare the visible glyph halo with actual wall-material regions.
    /// Potential collisions require review; this never moves or hides a label.
    #[serde(default)]
    pub check_wall_overlap: bool,
}
fn clearance() -> f64 {
    0.5
}
fn limit() -> usize {
    1000
}

pub(crate) fn audit(s: &Session, p: Audit) -> Result<Value> {
    checked("lettering clearance (paper mm)", p.clearance_mm_paper, 0., 10.)?;
    if !(1..=10_000).contains(&p.max_results) {
        return Err(ModelError::Invalid("request 1..10000 lettering findings".into()).into());
    }
    if p.check_wall_overlap {
        let view = s.doc.views.iter().find(|v| v.id == p.id).ok_or_else(|| ModelError::NotFound(format!("drawing view {}", p.id)))?;
        if !matches!(view.kind, trigon_model::annotation::DrawingViewKind::Plan { .. }) {
            return Err(ModelError::Invalid("wall-overlap lettering checks require a plan view".into()).into());
        }
    }
    let sheet = crate::views::sheet_unclipped(&s.doc, &p.id, View::latest(&s.doc))?;
    let texts: Vec<_> = sheet
        .scene
        .items
        .iter()
        .filter_map(|i| match i {
            Item::Text(t) => Some(t.clone()),
            _ => None,
        })
        .collect();
    if texts.len() > 2000 {
        return Err(ModelError::Invalid("lettering audit exceeds 2000 labels; use a smaller view/crop".into()).into());
    }
    let bounds = crate::plan_output::text_ink_bounds(&texts)?;
    let scale = sheet.scene.scale;
    let walls = if p.check_wall_overlap { visible_wall_material(&sheet.scene, texts.len())? } else { Vec::new() };
    let [x, y, w, h] = sheet.clip;
    let (lo, hi) = (sheet.vp.at([x, y + h]), sheet.vp.at([x + w, y]));
    let mut crop = [lo[0] / scale, lo[1] / scale, hi[0] / scale, hi[1] / scale];
    let template = s.doc.views.iter().find(|v| v.id == p.id).and_then(|v| v.template.as_ref());
    if let Some(bounds) = template.and_then(|t| t.crop) {
        crop = [crop[0].max(bounds[0] / scale), crop[1].max(bounds[1] / scale), crop[2].min(bounds[2] / scale), crop[3].min(bounds[3] / scale)];
    }
    let mut labels = Vec::new();
    let mut polygons: Vec<(String, Option<crate::sheet::TextSource>, Vec<[f64; 2]>)> = Vec::new();
    let mut findings = Vec::new();
    let mut counts = std::collections::BTreeMap::<String, usize>::new();
    let mut count = 0usize;
    let mut finding = |value: Value| {
        count += 1;
        *counts.entry(value["kind"].as_str().unwrap_or("unknown").to_string()).or_default() += 1;
        if findings.len() < p.max_results {
            findings.push(value);
        }
    };
    for (index, (text, bounds)) in texts.iter().zip(bounds).enumerate() {
        let Some(b) = bounds else { continue };
        let corners = |b: [f64; 4]| {
            let (sin, cos) = text.deg.to_radians().sin_cos();
            [[b[0], b[1]], [b[2], b[1]], [b[2], b[3]], [b[0], b[3]]]
                .map(|[x, y]| [text.at[0] / scale + x * cos - y * sin, text.at[1] / scale + x * sin + y * cos])
        };
        let ink = corners(b);
        // Include the actual exported white halo; a box halo uses the shared SVG rectangle.
        let halo = match text.halo {
            Halo::Outline => {
                let pad = 2.0 * MM_PER_PX;
                [b[0] - pad, b[1] - pad, b[2] + pad, b[3] + pad]
            }
            Halo::Box => {
                let width = 0.56 * text.size * text.text.chars().count() as f64 + 6.0 * MM_PER_PX;
                let left = match text.anchor {
                    crate::sheet::Anchor::Start => -3.0 * MM_PER_PX,
                    crate::sheet::Anchor::Middle => -width / 2.,
                    crate::sheet::Anchor::End => -width + 3.0 * MM_PER_PX,
                };
                let bottom = if text.middle { -0.62 * text.size } else { -0.34 * text.size };
                [b[0].min(left), b[1].min(bottom), b[2].max(left + width), b[3].max(bottom + 1.24 * text.size)]
            }
            Halo::None => b,
        };
        let natural = corners(halo);
        let at = [text.at[0] / scale, text.at[1] / scale];
        let outside_anchor = at[0] < crop[0] || at[0] > crop[2] || at[1] < crop[1] || at[1] > crop[3];
        let clipped = natural.iter().any(|q| q[0] < crop[0] || q[0] > crop[2] || q[1] < crop[1] || q[1] > crop[3]);
        let visible = if template.is_some() && outside_anchor { Vec::new() } else { clip_rect(natural.to_vec(), crop) };
        let key = format!("{}:text:{index}", p.id);
        let measurement = text
            .source
            .as_ref()
            .filter(|source| source.kind == "annotation")
            .and_then(|source| s.doc.annotations.iter().find(|a| a.id == source.id))
            .and_then(|a| match &a.kind {
                trigon_model::annotation::AnnotationKind::Dimension { from, to, .. } => Some((to[0] - from[0]).hypot(to[1] - from[1])),
                trigon_model::annotation::AnnotationKind::Level { at, datum_mm, .. } => Some(at[1] - datum_mm),
                _ => None,
            });
        labels.push(json!({"key":key,"source":text.source,"text":text.text,"at_mm":text.at,"size_mm_paper":text.size,
            "rotation_deg":text.deg,"style":text.style,"measurement_mm":measurement,
            "ink_corners_mm":ink.map(|q|q.map(|v|v*scale)),"halo_corners_mm":natural.map(|q|q.map(|v|v*scale)),
            "visible_polygon_mm":visible.iter().map(|q|q.map(|v|v*scale)).collect::<Vec<_>>(),"clipped":clipped||outside_anchor}));
        if clipped || outside_anchor {
            finding(json!({"kind":"clipped_label","key":key,"source":text.source,"anchor_outside":outside_anchor}));
        }
        for (other, source, poly) in &polygons {
            if overlaps(&visible, poly, p.clearance_mm_paper) {
                finding(
                    json!({"kind":"potential_text_overlap","keys":[other,&key],"sources":[source,&text.source],"clearance_mm_paper":p.clearance_mm_paper}),
                );
            }
        }
        let mut wall_overlaps = std::collections::BTreeMap::<&str, f64>::new();
        for (id, rings) in &walls {
            let area = material_overlap(&visible, rings);
            if area > 1e-10 {
                *wall_overlaps.entry(id).or_default() += area * scale * scale;
            }
        }
        for (wall_id, area) in wall_overlaps {
            finding(json!({"kind":"text_over_wall","key":key,"source":text.source,"wall_id":wall_id,
                "halo_overlap_mm2_drawing":area,"review_required":true,
                "inspect_commands":[{"command":"annotation.list","params":{"view":p.id}},
                    {"command":"element.get","params":{"id":wall_id}}]}));
        }
        polygons.push((key, text.source.clone(), visible));
    }
    let mut report = json!({"view":p.id,"scale":scale,"status":if count==0 {"no_findings_in_scope"} else {"needs_review"},
        "labels":labels,"findings":findings,"total_findings":count,"finding_counts":counts,"truncated":count>p.max_results,
        "inspect_commands":[{"command":"annotation.list","params":{"view":p.id}}],
        "mutation_workflow":["precision.preview","precision.apply"],
        "clearance_mm_paper":p.clearance_mm_paper,"visible_bounds_mm":crop.map(|v|v*scale),
        "scope":"bundled SVG glyph ink and exported halos; generated labels present in the view plus saved annotations",
        "limitations":["Box intersections are potential collisions, not a pixel-level glyph collision test.","DXF consumers may substitute fonts. Suppressed generated labels and generic text/model-line clashes are not audited."]});
    if p.check_wall_overlap {
        report["wall_overlap_check"] = json!({"status":"checked","wall_sources":walls.len(),"scope":"visible glyph halo boxes against filled WallCut material, unioned per source; excludes picking-only faces, wall strokes and other model lines"});
    }
    Ok(report)
}

fn visible_wall_material(scene: &crate::sheet::Scene, texts: usize) -> Result<Vec<(String, Vec<Vec<[f64; 2]>>)>> {
    use i_overlay::core::{fill_rule::FillRule, overlay_rule::OverlayRule};
    use i_overlay::float::overlay::FloatOverlay;
    let is_wall = |kind| matches!(kind, crate::DrawingElementKind::InteriorWall | crate::DrawingElementKind::FacadeWall);
    let paper = |rings: &[Vec<[f64; 2]>]| -> Vec<Vec<[f64; 2]>> {
        rings.iter().map(|ring| ring.iter().map(|p| [p[0] / scene.scale, p[1] / scene.scale]).collect()).collect()
    };
    let fills: Vec<_> = scene
        .items
        .iter()
        .filter_map(|item| {
            let Item::Path(path) = item else { return None };
            if path.layer != crate::sheet::Layer::WallCut || path.fill.is_none() {
                return None;
            }
            let ids: std::collections::BTreeSet<_> = path.element_segments.iter().filter(|e| is_wall(e.kind)).map(|e| e.id.as_str()).collect();
            Some((ids, paper(&path.rings)))
        })
        .collect();
    let regions: Vec<_> = scene.element_regions.iter().filter(|r| is_wall(r.kind)).collect();
    if regions.len().saturating_mul(texts.saturating_add(fills.len())) > 2_000_000 {
        return Err(ModelError::Invalid("lettering/wall comparison budget exceeded; use a smaller view/crop".into()).into());
    }
    let mut by_id = std::collections::BTreeMap::<String, Vec<Vec<[f64; 2]>>>::new();
    for region in regions {
        let rings = paper(&region.rings);
        for (ids, fill) in &fills {
            if ids.contains(region.id.as_str()) {
                by_id
                    .entry(region.id.clone())
                    .or_default()
                    .extend(FloatOverlay::with_subj_and_clip(&rings, fill).overlay(OverlayRule::Intersect, FillRule::EvenOdd).into_iter().flatten());
            }
        }
    }
    Ok(by_id
        .into_iter()
        .filter_map(|(id, rings)| {
            // Ranged-plan picking metadata may include coincident top/bottom faces.
            // Normalize their intersections to actual visible fill before measuring ink.
            let rings: Vec<_> = FloatOverlay::with_subj(&rings).overlay(OverlayRule::Union, FillRule::NonZero).into_iter().flatten().collect();
            (!rings.is_empty()).then_some((id, rings))
        })
        .collect())
}

fn material_overlap(label: &[[f64; 2]], wall: &[Vec<[f64; 2]>]) -> f64 {
    use i_overlay::core::{fill_rule::FillRule, overlay_rule::OverlayRule};
    use i_overlay::float::overlay::FloatOverlay;
    if label.len() < 3 {
        return 0.0;
    }
    FloatOverlay::with_subj_and_clip(&label, &wall)
        .overlay(OverlayRule::Intersect, FillRule::EvenOdd)
        .iter()
        .map(|shape| {
            shape
                .iter()
                .enumerate()
                .map(|(i, ring)| if i == 0 { trigon_model::doc::polygon_area(ring) } else { -trigon_model::doc::polygon_area(ring) })
                .sum::<f64>()
        })
        .sum()
}

fn overlaps(a: &[[f64; 2]], b: &[[f64; 2]], clearance: f64) -> bool {
    if a.len() < 3 || b.len() < 3 {
        return false;
    }
    for poly in [a, b] {
        for i in 0..poly.len() {
            let (p, q) = (poly[i], poly[(i + 1) % poly.len()]);
            let d = [q[0] - p[0], q[1] - p[1]];
            let length = d[0].hypot(d[1]);
            if length < 1e-12 {
                continue;
            }
            let n = [-d[1] / length, d[0] / length];
            let span = |poly: &[[f64; 2]]| {
                poly.iter().map(|q| q[0] * n[0] + q[1] * n[1]).fold((f64::INFINITY, f64::NEG_INFINITY), |(lo, hi), v| (lo.min(v), hi.max(v)))
            };
            let ((a0, a1), (b0, b1)) = (span(a), span(b));
            if a1 + clearance <= b0 || b1 + clearance <= a0 {
                return false;
            }
        }
    }
    true
}
fn clip_rect(mut poly: Vec<[f64; 2]>, b: [f64; 4]) -> Vec<[f64; 2]> {
    if b[0] >= b[2] || b[1] >= b[3] {
        return Vec::new();
    }
    for (axis, edge, lower) in [(0, b[0], true), (0, b[2], false), (1, b[1], true), (1, b[3], false)] {
        let inside = |p: [f64; 2]| if lower { p[axis] >= edge } else { p[axis] <= edge };
        let mut clipped = Vec::new();
        for i in 0..poly.len() {
            let (a, c) = (poly[i], poly[(i + 1) % poly.len()]);
            let (ain, cin) = (inside(a), inside(c));
            if ain {
                clipped.push(a);
            }
            if ain != cin {
                let t = (edge - a[axis]) / (c[axis] - a[axis]);
                clipped.push([a[0] + t * (c[0] - a[0]), a[1] + t * (c[1] - a[1])]);
            }
        }
        poly = clipped;
    }
    poly
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn opt_in_plan_wall_check_reports_printed_label_halos_and_exact_wall_sources() {
        let mut s = Session::new(Box::new(trigon_data::Live::new(Default::default())));
        let mut b = trigon_model::Building::new("b", "Printed enclosure", &[[0., 0.], [12000., 0.], [12000., 8000.], [0., 8000.]], 2, 3000.).unwrap();
        b.interior_walls = serde_json::from_value(json!([{"id":"host","levels":[0],"a":[3000,1000],"b":[3000,7000],"thickness":200}])).unwrap();
        s.doc.buildings.push(b);
        for (id, level) in [("lower", 0), ("upper", 1)] {
            s.execute("view.define", json!({"id":id,"name":"Plan","kind":{"view":"plan","level":level}})).unwrap();
            s.execute("view.configure", json!({"id":id,"template":{"name":"100","sizing":{"mode":"fixed","denominator":100,"centre":[6000,4000]}}}))
                .unwrap();
            s.execute(
                "annotation.define",
                json!({"id":format!("tag-{id}"),"view":id,"size_mm":2.5,"kind":{"annotation":"text","at":[2950,3500],"text":"E","rotation_deg":37}}),
            )
            .unwrap();
        }
        let before = s.doc.clone();
        let history = (s.undo.len(), s.redo.len(), s.dirty);
        let legacy = s.execute("view.audit", json!({"id":"lower"})).unwrap();
        assert_eq!(legacy["total_findings"], 0);
        let report = s.execute("view.audit", json!({"id":"lower","check_wall_overlap":true})).unwrap();
        let hits: Vec<_> = report["findings"].as_array().unwrap().iter().filter(|f| f["kind"] == "text_over_wall").collect();
        assert_eq!(hits.len(), 1, "{report}");
        let hit = hits[0];
        assert_eq!(hit["source"]["id"], "tag-lower");
        assert_eq!(hit["wall_id"], "host.L0");
        assert!(hit["halo_overlap_mm2_drawing"].as_f64().unwrap() > 1.0);
        assert_eq!(report["labels"], legacy["labels"]);
        for inspect in hit["inspect_commands"].as_array().unwrap() {
            s.execute(inspect["command"].as_str().unwrap(), inspect["params"].clone()).unwrap();
        }
        let upper = s.execute("view.audit", json!({"id":"upper","check_wall_overlap":true})).unwrap();
        assert_eq!(upper["total_findings"], 0, "the wall is absent upstairs: {upper}");
        assert_eq!(s.doc, before);
        assert_eq!((s.undo.len(), s.redo.len(), s.dirty), history);
        let preview = s
            .execute(
                "precision.preview",
                json!({"action":{"command":"annotation.define","params":{
                    "id":"new-tag","view":"lower","kind":{"annotation":"text","at":[2950,5500],"text":"F"}
                }}}),
            )
            .unwrap();
        assert!(
            preview["drawing_feedback"][0]["findings"]
                .as_array()
                .unwrap()
                .iter()
                .any(|f| f["kind"] == "text_over_wall" && f["source"]["id"] == "new-tag")
        );
        assert_eq!(s.doc, before);
        assert_eq!((s.undo.len(), s.redo.len(), s.dirty), history);
        s.execute("view.define", json!({"id":"elevation","name":"Elevation","kind":{"view":"facade","facade":"b.f1"}})).unwrap();
        let before = s.doc.clone();
        assert!(s.execute("view.audit", json!({"id":"elevation","check_wall_overlap":true})).is_err());
        assert_eq!(s.doc, before);
    }

    #[test]
    fn lettering_material_intersection_preserves_door_and_shaft_voids() {
        let outer = vec![[0., 0.], [100., 0.], [100., 100.], [0., 100.]];
        let hole = vec![[25., 25.], [75., 25.], [75., 75.], [25., 75.]];
        let wall = vec![outer, hole];
        assert_eq!(material_overlap(&[[30., 30.], [40., 30.], [40., 40.], [30., 40.]], &wall), 0.);
        assert_eq!(material_overlap(&[[20., 20.], [30., 20.], [30., 30.], [20., 30.]], &wall), 75.);
    }

    #[test]
    fn ranged_wall_check_uses_visible_fills_once_and_excludes_below_range_faces() {
        for cut in [1000.0, 4000.0] {
            let mut s = Session::new(Box::new(trigon_data::Live::new(Default::default())));
            let mut b =
                trigon_model::Building::new("b", "Range material", &[[0., 0.], [12000., 0.], [12000., 8000.], [0., 8000.]], 2, 3000.).unwrap();
            b.interior_walls = serde_json::from_value(json!([{"id":"host","levels":[0],"a":[3000,1000],"b":[3000,7000],"thickness":1000}])).unwrap();
            s.doc.buildings.push(b);
            s.execute("view.define", json!({"id":"p","name":"Ranged plan","kind":{"view":"plan","level":0}})).unwrap();
            s.execute(
                "view.configure",
                json!({"id":"p","template":{"name":"Range","sizing":{"mode":"fixed","denominator":100,"centre":[6000,4000]},
                "range":{"cut_mm":cut,"top_mm":cut+500.,"bottom_mm":0.,"depth_mm":-1000.}}}),
            )
            .unwrap();
            s.execute("annotation.define", json!({"id":"tag","view":"p","kind":{"annotation":"text","at":[2950,3500],"text":"E"}})).unwrap();
            let before = s.doc.clone();
            let report = s.execute("view.audit", json!({"id":"p","check_wall_overlap":true})).unwrap();
            let hits: Vec<_> =
                report["findings"].as_array().unwrap().iter().filter(|f| f["kind"] == "text_over_wall" && f["source"]["id"] == "tag").collect();
            if cut == 4000.0 {
                assert!(hits.is_empty(), "below-range picking faces are not wall fill: {report}");
            } else {
                assert_eq!(hits.len(), 1, "{report}");
                let label = report["labels"].as_array().unwrap().iter().find(|l| l["source"]["id"] == "tag").unwrap();
                let ring: Vec<[f64; 2]> = serde_json::from_value(label["visible_polygon_mm"].clone()).unwrap();
                assert!(
                    (hits[0]["halo_overlap_mm2_drawing"].as_f64().unwrap() - trigon_model::doc::polygon_area(&ring)).abs() < 0.01,
                    "duplicate projected faces must not double the area: {report}"
                );
            }
            assert_eq!(s.doc, before);
        }
    }

    #[test]
    fn audit_feedback_keeps_lettering_counts_sources_and_read_only_next_steps_when_truncated() {
        let mut s = Session::new(Box::new(trigon_data::Live::new(Default::default())));
        s.doc.buildings.push(
            trigon_model::Building::new("b", "Lettering feedback fixture", &[[0., 0.], [12000., 0.], [12000., 8000.], [0., 8000.]], 1, 3000.)
                .unwrap(),
        );
        s.execute("view.define", json!({"id":"p","name":"Plan","kind":{"view":"plan","level":0}})).unwrap();
        s.execute("view.configure", json!({"id":"p","template":{"name":"100","sizing":{"mode":"fixed","denominator":100,"centre":[6000,4000]}}}))
            .unwrap();
        for (id, at) in [("wide-a", [2000, 4000]), ("wide-b", [3500, 4000]), ("outside", [6000, 100000])] {
            s.execute(
                "annotation.define",
                json!({"id":id,"view":"p","size_mm":2.5,"kind":{"annotation":"text","at":at,"text":"WWWWWWWW","rotation_deg":0}}),
            )
            .unwrap();
        }
        let before = s.doc.clone();
        let history = (s.undo.len(), s.redo.len(), s.dirty);
        let full = s.execute("view.audit", json!({"id":"p"})).unwrap();
        assert_eq!(full["total_findings"], 2);
        let limited = s.execute("view.audit", json!({"id":"p","max_results":1})).unwrap();
        assert_eq!(limited["truncated"], true);
        assert_eq!(limited["findings"].as_array().unwrap().len(), 1);
        assert_eq!(limited["finding_counts"], json!({"clipped_label":1,"potential_text_overlap":1}));
        assert_eq!(limited["finding_counts"], full["finding_counts"]);
        assert_eq!(limited["findings"][0]["sources"][0]["id"], "wide-a");
        assert_eq!(limited["findings"][0]["sources"][1]["id"], "wide-b");
        assert_eq!(limited["clearance_mm_paper"], 0.5);
        assert_eq!(limited["labels"], full["labels"], "bounded findings retain the shaped ink and halo evidence");
        assert_eq!(limited["mutation_workflow"], json!(["precision.preview", "precision.apply"]));
        assert_eq!(limited["inspect_commands"], json!([{"command":"annotation.list","params":{"view":"p"}}]));
        for command in limited["inspect_commands"].as_array().unwrap() {
            let id = command["command"].as_str().unwrap();
            assert!(!crate::commands().iter().find(|c| c.id == id).unwrap().mutates);
            assert_eq!(s.execute(id, command["params"].clone()).unwrap()["annotations"].as_array().unwrap().len(), 3);
        }
        assert_eq!(s.doc, before);
        assert_eq!((s.undo.len(), s.redo.len(), s.dirty), history);
    }
}
