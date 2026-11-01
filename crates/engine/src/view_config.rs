//! Typed, atomic saved-view configuration. Templates are stored with a view, then copied from
//! that view by command; no external template file or paid service is required.
use crate::sheet::{Item, Layer, MM_PER_PX, PAGE_MM, Sheet, Text, Viewport, scale_text};
use crate::{Result, Session};
use schemars::JsonSchema;
use serde::Deserialize;
use serde_json::{Value, json};
use trigon_model::{
    ModelError,
    annotation::{DrawingTemplate, DrawingView, DrawingViewKind, LineWeightProfile, RoomLabels, ViewSizing},
};

#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub(crate) struct Configure {
    pub id: String,
    pub template: Option<DrawingTemplate>,
}
#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub(crate) struct Apply {
    pub source: String,
    pub targets: Vec<String>,
}

pub(crate) fn configure(s: &mut Session, p: Configure) -> Result<Value> {
    let mut view = s.doc.views.iter().find(|v| v.id == p.id).cloned().ok_or_else(|| ModelError::NotFound(format!("drawing view {}", p.id)))?;
    view.template = p.template;
    crate::views::define_view(s, view)
}
pub(crate) fn apply(s: &mut Session, p: Apply) -> Result<Value> {
    if p.targets.is_empty() || p.targets.len() > 256 {
        return Err(ModelError::Invalid("apply a template to 1 to 256 distinct views".into()).into());
    }
    let template = s
        .doc
        .views
        .iter()
        .find(|v| v.id == p.source)
        .and_then(|v| v.template.clone())
        .ok_or_else(|| ModelError::NotFound(format!("configured template view {}", p.source)))?;
    let mut seen = std::collections::BTreeSet::new();
    let views = p
        .targets
        .iter()
        .map(|id| {
            if !seen.insert(id) {
                return Err(ModelError::Invalid("duplicate template target".into()));
            }
            let mut view = s.doc.views.iter().find(|v| &v.id == id).cloned().ok_or_else(|| ModelError::NotFound(format!("drawing view {id}")))?;
            view.template = Some(template.clone());
            view.validate_source(&s.doc)?;
            Ok(view)
        })
        .collect::<std::result::Result<Vec<_>, ModelError>>()?;
    for view in views {
        crate::views::define_view(s, view)?;
    }
    Ok(json!({"template":template.name,"views":p.targets}))
}

pub(crate) fn prepare(sheet: &mut Sheet, template: &DrawingTemplate, annotation_bounds: Option<[f64; 4]>) {
    if let Some(crop) = template.crop {
        clip(sheet, crop);
    }
    let bounds = template.crop.or(annotation_bounds).or_else(|| sheet.scene.bounds()).unwrap_or([0.0, 0.0, 1000.0, 1000.0]);
    sheet.vp = match template.sizing {
        ViewSizing::Fit => Viewport::fit(bounds, sheet.clip),
        ViewSizing::Fixed { denominator, centre } => {
            let mm = denominator * MM_PER_PX;
            Viewport { x0: centre[0] - (sheet.clip[0] + sheet.clip[2] / 2.0) * mm, y1: centre[1] + (sheet.clip[1] + sheet.clip[3] / 2.0) * mm, mm }
        }
    };
    sheet.scene.scale = sheet.vp.scale();
    let factor = match template.line_weights {
        LineWeightProfile::Fine => 0.75,
        LineWeightProfile::Standard => 1.0,
        LineWeightProfile::Bold => 1.4,
    };
    for item in &mut sheet.scene.items {
        if let Item::Path(path) = item
            && path.layer != Layer::Annotation
            && let Some(stroke) = &mut path.stroke
        {
            stroke.pen = (stroke.pen * factor).clamp(0.05, 2.11);
        }
    }
    let crop_note = if matches!(template.sizing, ViewSizing::Fixed { .. }) {
        Some("Fixed scale · cropped to paper")
    } else if template.crop.is_some() {
        Some("Drawing crop")
    } else {
        None
    };
    if let Some(note) = crop_note {
        sheet.note = Some(match sheet.note.take() {
            Some(old) => format!("{old} · {note}"),
            None => note.into(),
        });
    }
    sheet.metrics["template"] = json!(template.name);
    sheet.metrics["line_weights"] = json!(template.line_weights);
    sheet.metrics["sizing"] = json!(if matches!(template.sizing, ViewSizing::Fit) { "fit" } else { "fixed" });
    sheet.metrics["paper_mm"] = json!(PAGE_MM);
    sheet.metrics["scale"] = json!(scale_text(sheet.scene.scale));
    sheet.metrics["mm_per_px"] = json!(sheet.vp.mm);
}

/// Keep generated dimension text clear using the final physical scale and actual glyph ink.
/// Legacy unconfigured exports retain their layout; configured sheets use paper-mm clearance.
pub(crate) fn paper_clearances(sheet: &mut Sheet) -> Result<()> {
    let labels: Vec<_> = sheet
        .scene
        .items
        .iter()
        .filter_map(|item| match item {
            Item::Text(t) if t.clearance.is_some() => Some(t.clone()),
            _ => None,
        })
        .collect();
    if labels.is_empty() {
        return Ok(());
    }
    let bounds = crate::plan_output::text_ink_bounds(&labels)?;
    let mut bounds = bounds.into_iter();
    for item in &mut sheet.scene.items {
        let Item::Text(text) = item else { continue };
        let Some(guide) = &text.clearance else { continue };
        let Some(b) = bounds.next().flatten() else { continue };
        let (sin, cos) = text.deg.to_radians().sin_cos();
        let near = [[b[0], b[1]], [b[2], b[1]], [b[2], b[3]], [b[0], b[3]]]
            .into_iter()
            .map(|[x, y]| (x * cos - y * sin) * guide.outward[0] + (x * sin + y * cos) * guide.outward[1])
            .fold(f64::INFINITY, f64::min);
        // 0.5 mm visible air, half the dimension pen, and the text's white outline.
        let required = 0.5 + 0.13 / 2.0 + 2.0 * MM_PER_PX - near;
        let current = ((text.at[0] - guide.anchor[0]) * guide.outward[0] + (text.at[1] - guide.anchor[1]) * guide.outward[1]) / sheet.scene.scale;
        let shift = (required - current).max(0.0) * sheet.scene.scale;
        for (coordinate, direction) in text.at.iter_mut().zip(guide.outward) {
            *coordinate += direction * shift;
        }
    }
    Ok(())
}

/// Crop geometry as well as the SVG viewport, so DXF and exact picking agree with the sheet.
pub(crate) fn finish(sheet: &mut Sheet, template: &DrawingTemplate) {
    if let Some(crop) = template.crop {
        clip(sheet, crop);
    }
    let [x, y, w, h] = sheet.clip;
    let a = sheet.vp.at([x, y + h]);
    let b = sheet.vp.at([x + w, y]);
    let crop = [a[0], a[1], b[0], b[1]];
    clip(sheet, crop);
    sheet.metrics["visible_bounds_mm"] = json!(crop);
}

/// Add live room tags only after the actual physical sheet scale has been established.
/// A conservative lettering box must fit the room's inscribed clearance and visible paper.
pub(crate) fn room_labels(sheet: &mut Sheet, doc: &trigon_model::Document, view: &DrawingView, template: &DrawingTemplate) {
    if template.room_labels == RoomLabels::Hidden {
        return;
    }
    let DrawingViewKind::Plan { level } = view.kind else { return };
    let [x, y, w, h] = sheet.clip;
    let lo = sheet.vp.at([x, y + h]);
    let hi = sheet.vp.at([x + w, y]);
    let mut bounds = [lo[0], lo[1], hi[0], hi[1]];
    if let Some(crop) = template.crop {
        bounds = [bounds[0].max(crop[0]), bounds[1].max(crop[1]), bounds[2].min(crop[2]), bounds[3].min(crop[3])];
    }
    let scale = sheet.scene.scale;
    let size = 2.5;
    let stairs: Vec<[f64; 4]> = sheet
        .scene
        .element_regions
        .iter()
        .filter(|region| region.kind == crate::DrawingElementKind::Stair)
        .filter_map(|region| {
            region
                .rings
                .iter()
                .flatten()
                .map(|p| [p[0], p[1], p[0], p[1]])
                .reduce(|a, b| [a[0].min(b[0]), a[1].min(b[1]), a[2].max(b[2]), a[3].max(b[3])])
        })
        .collect();
    let mut count = 0;
    for building in &doc.buildings {
        let Some(storey) = building.storeys.get(level) else { continue };
        if template.range.is_some_and(|range| range.cut_mm < 0.0 || range.cut_mm >= storey.height) {
            continue;
        }
        for (index, room) in building.rooms.iter().enumerate().filter(|(_, room)| room.levels.contains(&level)) {
            let Some((at, clearance)) = crate::plan::room_label_anchor(&room.polygon) else { continue };
            let area = format!("{:.1} m²", trigon_model::doc::polygon_area(&room.polygon) / 1e6);
            let fits = |chars: usize, lines: usize| {
                let half_width = chars as f64 * size * scale / 2.0;
                let half_height = if lines == 2 { (1.75 + size) * scale } else { size * scale };
                half_width.hypot(half_height) <= clearance
                    && at[0] - half_width >= bounds[0]
                    && at[0] + half_width <= bounds[2]
                    && at[1] - half_height >= bounds[1]
                    && at[1] + half_height <= bounds[3]
                    && !stairs.iter().any(|b| {
                        at[0] + half_width >= b[0] && at[0] - half_width <= b[2] && at[1] + half_height >= b[1] && at[1] - half_height <= b[3]
                    })
            };
            let named = template.room_labels == RoomLabels::NameAndArea
                && !room.name.trim().is_empty()
                && fits(room.name.chars().count().max(area.chars().count()), 2);
            if !named && !fits(area.chars().count(), 1) {
                continue;
            }
            let id = crate::room_element_id(building, index, level);
            if named {
                let mut text = Text::new(Layer::Annotation, [at[0], at[1] + 1.75 * scale], &room.name, size);
                text.source = Some(crate::sheet::TextSource { kind: "element", id: id.clone(), role: "room_name" });
                sheet.scene.items.push(Item::Text(text));
            }
            let area_at = if named { [at[0], at[1] - 1.75 * scale] } else { at };
            let mut text = Text::new(Layer::Annotation, area_at, area, size);
            text.source = Some(crate::sheet::TextSource { kind: "element", id: id.clone(), role: "room_area" });
            sheet.scene.items.push(Item::Text(text));
            if !sheet.scene.element_regions.iter().any(|region| region.id == id) {
                sheet.scene.element_regions.push(crate::DrawingElementRegion {
                    id,
                    kind: crate::DrawingElementKind::Room,
                    rings: vec![room.polygon.clone()],
                    depth_plane: template.range.map(|range| [-storey.elevation - range.cut_mm, 0.0, 0.0]),
                });
            }
            count += 1;
        }
    }
    sheet.metrics["room_labels"] = json!(template.room_labels);
    sheet.metrics["room_tags_drawn"] = json!(count);
}
fn clip(sheet: &mut Sheet, crop: [f64; 4]) {
    sheet.scene.clip(crop);
    sheet.scene.items.retain(|i| match i {
        Item::Text(t) => t.at[0] >= crop[0] && t.at[0] <= crop[2] && t.at[1] >= crop[1] && t.at[1] <= crop[3],
        _ => true,
    });
}

#[cfg(test)]
mod room_label_tests {
    use super::*;
    use trigon_model::build::View;

    fn session() -> Session {
        let mut s = Session::new(Box::new(trigon_data::Live::new(trigon_data::Credentials::default())));
        let mut b = trigon_model::Building::new("b", "Room tag fixture", &[[0., 0.], [12000., 0.], [12000., 8000.], [0., 8000.]], 2, 3000.).unwrap();
        b.rooms.push(trigon_model::Room {
            id: "room".into(),
            name: "Living room".into(),
            levels: vec![0],
            polygon: vec![[1000., 1000.], [8000., 1000.], [8000., 5000.], [1000., 5000.]],
        });
        s.doc.buildings.push(b);
        s.execute("view.define", json!({"id":"plan","name":"Plan","kind":{"view":"plan","level":0}})).unwrap();
        s
    }

    #[test]
    fn saved_plan_room_tags_use_live_room_names_and_areas_with_physical_paper_text() {
        let mut s = session();
        s.execute(
            "view.configure",
            json!({"id":"plan","template":{"name":"Room tags","room_labels":"name_and_area",
            "sizing":{"mode":"fixed","denominator":100,"centre":[6000,4000]}}}),
        )
        .unwrap();
        let sheet = crate::views::sheet(&s.doc, "plan", View::latest(&s.doc)).unwrap();
        let tags: Vec<_> = sheet.scene.items.iter().filter_map(|i| if let Item::Text(t) = i { Some(t) } else { None }).collect();
        assert!(tags.iter().any(|t| t.text == "Living room" && t.size == 2.5));
        assert!(tags.iter().any(|t| t.text == "28.0 m²" && t.size == 2.5));
        let before = s.doc.clone();
        let svg = sheet.svg();
        s.execute(
            "element.set",
            json!({"id":"room.space.L0","properties":{"name":"Dining room","polygon":[[1000,1000],[9000,1000],[9000,5000],[1000,5000]]}}),
        )
        .unwrap();
        let changed = crate::views::sheet(&s.doc, "plan", View::latest(&s.doc)).unwrap().svg();
        assert!(changed.contains("Dining room") && changed.contains("32.0 m²"));
        assert!(!changed.contains("Living room") && !changed.contains("28.0 m²"));
        s.execute("edit.undo", json!({})).unwrap();
        assert_eq!(s.doc, before);
        assert_eq!(crate::views::sheet(&s.doc, "plan", View::latest(&s.doc)).unwrap().svg(), svg);
    }

    fn configure(s: &mut Session, mode: &str, denominator: f64, crop: Option<[f64; 4]>) {
        s.execute(
            "view.configure",
            json!({"id":"plan","template":{"name":"Tags","room_labels":mode,"crop":crop,
            "sizing":{"mode":"fixed","denominator":denominator,"centre":[6000,4000]}}}),
        )
        .unwrap();
    }

    #[test]
    fn hidden_room_tags_preserve_default_serialization_and_drawing_bytes() {
        let mut s = session();
        s.execute("view.configure", json!({"id":"plan","template":{"name":"Tags","sizing":{"mode":"fixed","denominator":100,"centre":[6000,4000]}}}))
            .unwrap();
        let before = s.doc.clone();
        let baseline = crate::views::sheet(&s.doc, "plan", View::latest(&s.doc)).unwrap().svg();
        configure(&mut s, "hidden", 100., None);
        assert_eq!(crate::views::sheet(&s.doc, "plan", View::latest(&s.doc)).unwrap().svg(), baseline);
        let template = serde_json::to_value(&s.doc.views[0].template).unwrap();
        assert!(template.get("room_labels").is_none());
        assert!(!baseline.contains("Living room") && !baseline.contains("28.0 m²"));
        configure(&mut s, "area", 100., None);
        assert!(crate::views::sheet(&s.doc, "plan", View::latest(&s.doc)).unwrap().svg().contains("28.0 m²"));
        assert_eq!(s.doc.buildings, before.buildings, "rendering/configuration never rewrites room polygons or names");
    }

    #[test]
    fn room_tags_fit_clearance_and_crop_and_blank_names_use_only_area() {
        let mut s = session();
        configure(&mut s, "name_and_area", 500., None);
        let sheet = crate::views::sheet(&s.doc, "plan", View::latest(&s.doc)).unwrap();
        assert_eq!(sheet.metrics["room_tags_drawn"], 0);
        assert!(!sheet.svg().contains("28.0 m²"));
        configure(&mut s, "name_and_area", 100., Some([0., 0., 4500., 8000.]));
        let sheet = crate::views::sheet(&s.doc, "plan", View::latest(&s.doc)).unwrap();
        assert_eq!(sheet.metrics["room_tags_drawn"], 0, "do not cut a label at the crop edge");
        s.doc.buildings[0].rooms[0].name.clear();
        configure(&mut s, "name_and_area", 100., None);
        let sheet = crate::views::sheet(&s.doc, "plan", View::latest(&s.doc)).unwrap();
        assert_eq!(sheet.metrics["room_tags_drawn"], 1);
        assert!(sheet.svg().contains("28.0 m²"));
        assert!(!sheet.svg().contains("Living room") && !sheet.svg().contains("Unnamed"));
    }

    #[test]
    fn room_tags_do_not_cover_visible_stair_symbols_inside_a_room_definition() {
        let mut s = session();
        s.doc.buildings[0].stairs.push(trigon_model::Stair {
            id: "stair".into(),
            levels: vec![0],
            outline: vec![[3500., 2000.], [5500., 2000.], [5500., 3900.], [3500., 3900.]],
            tread: None,
            kind: trigon_model::StairKind::UTurn,
            landing: None,
            well: None,
            rounded: true,
            winders: None,
            turn_depths: None,
        });
        configure(&mut s, "area", 100., None);
        let sheet = crate::views::sheet(&s.doc, "plan", View::latest(&s.doc)).unwrap();
        assert!(sheet.scene.element_regions.iter().any(|r| r.kind == crate::DrawingElementKind::Stair));
        assert_eq!(sheet.metrics["room_tags_drawn"], 0, "omit the tag rather than drawing it over stair treads");
        assert!(!sheet.svg().contains("28.0 m²"));
    }

    #[test]
    fn ranged_plan_room_tags_pick_exact_room_ids_and_respect_storeys_and_cut_height() {
        let mut s = session();
        let mut upper = s.doc.buildings[0].rooms[0].clone();
        upper.id = "upper".into();
        upper.name = "Upper only".into();
        upper.levels = vec![1];
        s.doc.buildings[0].rooms.push(upper);
        s.execute(
            "view.configure",
            json!({"id":"plan","template":{"name":"Tags","room_labels":"name_and_area",
            "sizing":{"mode":"fixed","denominator":100,"centre":[6000,4000]},
            "range":{"top_mm":2500,"cut_mm":1000,"bottom_mm":0,"depth_mm":-200}}}),
        )
        .unwrap();
        let sheet = crate::views::sheet(&s.doc, "plan", View::latest(&s.doc)).unwrap();
        assert_eq!(sheet.metrics["room_tags_drawn"], 1);
        assert!(!sheet.svg().contains("Upper only"));
        let preview = crate::drawing_view(&s.doc, "plan", View::latest(&s.doc)).unwrap();
        assert_eq!(preview.pick_element([4500., 3000.], 1.).unwrap().id, "room.space.L0");
        s.execute(
            "view.configure",
            json!({"id":"plan","template":{"name":"Below floor","room_labels":"area",
            "range":{"top_mm":0,"cut_mm":-100,"bottom_mm":-300,"depth_mm":-500}}}),
        )
        .unwrap();
        let sheet = crate::views::sheet(&s.doc, "plan", View::latest(&s.doc)).unwrap();
        assert_eq!(sheet.metrics["room_tags_drawn"], 0);
    }

    #[test]
    fn room_label_configuration_rejects_unknown_modes_and_nonplan_targets_atomically() {
        let mut s = session();
        let facade = s.doc.buildings[0].facades[0].id.clone();
        s.execute("view.define", json!({"id":"facade","name":"Facade","kind":{"view":"facade","facade":facade}})).unwrap();
        let before = s.doc.clone();
        let history = s.undo.len();
        for params in [
            json!({"id":"plan","template":{"name":"Invalid","room_labels":"invented"}}),
            json!({"id":"facade","template":{"name":"Invalid","room_labels":"area"}}),
        ] {
            assert!(s.execute("view.configure", params).is_err());
            assert_eq!(s.doc, before);
            assert_eq!(s.undo.len(), history);
        }
    }
}

#[cfg(test)]
mod precision_clearance_tests {
    use super::*;
    use trigon_model::build::View;

    #[test]
    fn fixed_plan_balcony_lettering_clears_dimension_line_at_paper_scale() {
        let mut s = Session::new(Box::new(trigon_data::Live::new(Default::default())));
        let mut b = trigon_model::Building::new("b", "Balcony clearance", &[[0., 0.], [12000., 0.], [12000., 8000.], [0., 8000.]], 2, 3000.).unwrap();
        b.facades[0].bays = vec![6000.];
        let facade = b.facades[0].id.clone();
        s.doc.buildings.push(b);
        s.execute("facade.place", json!({"facade":facade,"type":"balcony.cantilever","levels":[0],"bays":[0]})).unwrap();
        s.execute("view.define", json!({"id":"p","name":"Plan","kind":{"view":"plan","level":0}})).unwrap();
        for scale in [100., 200., 500.] {
            s.execute(
                "view.configure",
                json!({"id":"p","template":{"name":"Physical scale","sizing":{"mode":"fixed","denominator":scale,"centre":[6000,2500]}}}),
            )
            .unwrap();
            let sheet = crate::views::sheet(&s.doc, "p", View::latest(&s.doc)).unwrap();
            let text = sheet
                .scene
                .items
                .iter()
                .find_map(|item| match item {
                    Item::Text(t) if t.text.starts_with("Ca. ") => Some(t),
                    _ => None,
                })
                .expect("balcony label");
            let bounds = crate::plan_output::text_ink_bounds(std::slice::from_ref(text)).unwrap()[0].unwrap();
            // First facade faces south: the dimension is 300 mm outside a 1300 mm slab.
            let dim_y = -s.doc.component("balcony.cantilever").unwrap().kind.depth().unwrap() - 300.;
            let top = text.at[1] + bounds[3] * scale;
            let ink_gap = (dim_y - top) / scale;
            assert!(ink_gap >= 0.49, "scale 1:{scale}: ink clearance {ink_gap} paper mm");
        }
    }
}
