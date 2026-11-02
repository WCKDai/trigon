//! Typed, atomic saved-view configuration. Templates are stored with a view, then copied from
//! that view by command; no external template file or paid service is required.
use crate::sheet::{Item, Layer, MM_PER_PX, PAGE_MM, Sheet, Text, Viewport, scale_text};
use crate::{Result, Session};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use trigon_model::{
    ModelError,
    annotation::{DrawingTemplate, DrawingView, DrawingViewKind, LineWeightProfile, RoomLabels, ViewSizing},
};

#[derive(Serialize, Deserialize, JsonSchema)]
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
    if let Some(caption) = &template.caption {
        sheet.caption.clone_from(caption);
    }
    if let Some(note) = &template.note {
        sheet.note = (!note.is_empty()).then(|| note.clone());
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
/// Bundled glyph ink must fit the room's inscribed clearance and visible paper.
pub(crate) fn room_labels(sheet: &mut Sheet, doc: &trigon_model::Document, view: &DrawingView, template: &DrawingTemplate) -> Result<()> {
    if template.room_labels == RoomLabels::Hidden {
        return Ok(());
    }
    let DrawingViewKind::Plan { level } = view.kind else { return Ok(()) };
    let [x, y, w, h] = sheet.clip;
    let lo = sheet.vp.at([x, y + h]);
    let hi = sheet.vp.at([x + w, y]);
    let mut bounds = [lo[0], lo[1], hi[0], hi[1]];
    if let Some(crop) = template.crop {
        bounds = [bounds[0].max(crop[0]), bounds[1].max(crop[1]), bounds[2].min(crop[2]), bounds[3].min(crop[3])];
    }
    let scale = sheet.scene.scale;
    let size = 2.5;
    // Keep visible air around the actual ink, including a typical cut-line half-width.
    let padding = 0.75 * scale;
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
    let mut candidates = Vec::new();
    let mut texts = Vec::new();
    // Geometry already decides which room areas exist at the actual cut and crop.
    // Tags must not restore an omitted room when buildings have unequal floor elevations.
    let visible_rooms: std::collections::BTreeSet<_> =
        sheet.scene.element_regions.iter().filter(|region| region.kind == crate::DrawingElementKind::Room).map(|region| region.id.as_str()).collect();
    for building in &doc.buildings {
        let Some(storey) = building.storeys.get(level) else { continue };
        // Another building's roof level must not suppress this building's full-storey tags.
        // Roof rooms remain unsupported and are rejected by document/command validation.
        if storey.roof_level || template.range.is_some_and(|range| range.cut_mm < 0.0 || range.cut_mm >= storey.height) {
            continue;
        }
        for (index, room) in building.rooms.iter().enumerate().filter(|(_, room)| room.levels.contains(&level)) {
            let id = crate::room_element_id(building, index, level);
            if !visible_rooms.contains(id.as_str()) {
                continue;
            }
            let Some((at, clearance)) = crate::plan::room_label_anchor(&room.polygon) else { continue };
            let area = format!("{:.1} m²", trigon_model::doc::polygon_area(&room.polygon) / 1e6);
            texts.push(Text::new(Layer::Annotation, at, area, size));
            texts.push(Text::new(Layer::Annotation, at, &room.name, size));
            candidates.push((id, room, at, clearance));
        }
    }
    // Shape once with the same bundled fonts, anchoring and physical size as export.
    let ink = crate::plan_output::text_ink_bounds(&texts)?;
    let mut count = 0;
    for ((id, room, at, clearance), [area_ink, name_ink]) in candidates.into_iter().zip(ink.as_chunks::<2>().0) {
        let fits = |with_name: bool| {
            let Some(mut box_mm) = *area_ink else { return false };
            if with_name {
                let Some(name_mm) = *name_ink else { return false };
                box_mm = [
                    box_mm[0].min(name_mm[0]),
                    (box_mm[1] - 1.75).min(name_mm[1] + 1.75),
                    box_mm[2].max(name_mm[2]),
                    (box_mm[3] - 1.75).max(name_mm[3] + 1.75),
                ];
            }
            let [x0, y0, x1, y1] = box_mm;
            let lo = [at[0] + x0 * scale, at[1] + y0 * scale];
            let hi = [at[0] + x1 * scale, at[1] + y1 * scale];
            [[lo[0], lo[1]], [hi[0], lo[1]], [hi[0], hi[1]], [lo[0], hi[1]]].iter().all(|p| {
                (p[0] - at[0]).hypot(p[1] - at[1]) + padding <= clearance
                    && p[0] >= bounds[0] + padding
                    && p[0] <= bounds[2] - padding
                    && p[1] >= bounds[1] + padding
                    && p[1] <= bounds[3] - padding
            }) && !stairs.iter().any(|b| hi[0] + padding >= b[0] && lo[0] - padding <= b[2] && hi[1] + padding >= b[1] && lo[1] - padding <= b[3])
        };
        let named = template.room_labels == RoomLabels::NameAndArea && !room.name.trim().is_empty() && fits(true);
        if !named && !fits(false) {
            continue;
        }
        if named {
            let mut text = Text::new(Layer::Annotation, [at[0], at[1] + 1.75 * scale], &room.name, size);
            text.source = Some(crate::sheet::TextSource { kind: "element", id: id.clone(), role: "room_name" });
            sheet.scene.items.push(Item::Text(text));
        }
        let area_at = if named { [at[0], at[1] - 1.75 * scale] } else { at };
        let area = format!("{:.1} m²", trigon_model::doc::polygon_area(&room.polygon) / 1e6);
        let mut text = Text::new(Layer::Annotation, area_at, area, size);
        text.source = Some(crate::sheet::TextSource { kind: "element", id, role: "room_area" });
        sheet.scene.items.push(Item::Text(text));
        count += 1;
    }
    sheet.metrics["room_labels"] = json!(template.room_labels);
    sheet.metrics["room_tags_drawn"] = json!(count);
    Ok(())
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
    fn full_storey_room_tags_survive_a_neighbouring_roof_level() {
        let mut s = session();
        s.doc.buildings[0].rooms[0].levels = vec![1];
        s.execute("view.define", json!({"id":"plan","name":"Plan","kind":{"view":"plan","level":1}})).unwrap();
        configure(&mut s, "name_and_area", 100., None);
        let tags = |doc: &trigon_model::Document| {
            crate::views::sheet(doc, "plan", View::latest(doc))
                .unwrap()
                .scene
                .items
                .into_iter()
                .filter_map(|item| match item {
                    Item::Text(text) if text.source.as_ref().is_some_and(|source| matches!(source.role, "room_name" | "room_area")) => Some(text),
                    _ => None,
                })
                .collect::<Vec<_>>()
        };
        let before = tags(&s.doc);
        assert_eq!(before.len(), 2);
        s.execute(
            "building.add",
            json!({"id":"rear","name":"Lower neighbour","footprint":[[15000,0],[23000,0],[23000,8000],[15000,8000]],
                "storeys":1,"storey_height_mm":3000}),
        )
        .unwrap();
        s.execute("building.set_roof", json!({"building":"rear","kind":"pitched","pitch_deg":35})).unwrap();
        s.execute("building.set_attic", json!({"building":"rear","floor":2250})).unwrap();
        assert_eq!(tags(&s.doc), before);
        s.doc.buildings.swap(0, 1);
        assert_eq!(tags(&s.doc), before, "building order does not hide full-storey tags");
        let pick = crate::drawing_view(&s.doc, "plan", View::latest(&s.doc)).unwrap();
        assert_eq!(pick.pick_element([4500., 3000.], 1.).unwrap().id, "room.space.L1");
        let unchanged = s.doc.clone();
        assert!(
            s.execute(
                "interior.add_rooms",
                json!({"building":"rear","rooms":[{"id":"roof-room","levels":[1],"name":"Unsupported",
                    "polygon":[[16000,1000],[20000,1000],[20000,5000],[16000,5000]]}]}),
            )
            .is_err()
        );
        assert_eq!(s.doc, unchanged, "the unsupported roof-room guard remains atomic");
    }

    #[test]
    fn room_tags_fit_actual_bundled_glyphs_without_reducing_paper_size() {
        let mut s = session();
        s.doc.buildings[0].rooms[0].name = "WC".into();
        s.doc.buildings[0].rooms[0].polygon = vec![[5000., 1000.], [6200., 1000.], [6200., 4500.], [5000., 4500.]];
        configure(&mut s, "name_and_area", 100., None);
        let before = s.doc.clone();
        let sheet = crate::views::sheet(&s.doc, "plan", View::latest(&s.doc)).unwrap();
        assert_eq!(sheet.metrics["room_tags_drawn"], 1, "the current character-count guess incorrectly suppresses a fitting tag");
        let labels: Vec<_> = sheet
            .scene
            .items
            .iter()
            .filter_map(|item| match item {
                Item::Text(text) if text.source.as_ref().is_some_and(|source| source.id == "room.space.L0") => Some(text.clone()),
                _ => None,
            })
            .collect();
        assert_eq!(labels.len(), 2);
        assert!(labels.iter().any(|text| text.text == "WC" && text.size == 2.5));
        assert!(labels.iter().any(|text| text.text == "4.2 m²" && text.size == 2.5));
        let bounds = crate::plan_output::text_ink_bounds(&labels).unwrap();
        for (text, bounds) in labels.iter().zip(bounds) {
            let [x0, y0, x1, y1] = bounds.unwrap();
            for p in [[x0, y0], [x1, y0], [x1, y1], [x0, y1]] {
                let p = [text.at[0] + p[0] * sheet.scene.scale, text.at[1] + p[1] * sheet.scene.scale];
                assert!((5000.0..=6200.0).contains(&p[0]) && (1000.0..=4500.0).contains(&p[1]));
            }
        }
        assert_eq!(s.doc, before, "glyph fitting is read-only");
        configure(&mut s, "name_and_area", 500., None);
        assert_eq!(crate::views::sheet(&s.doc, "plan", View::latest(&s.doc)).unwrap().metrics["room_tags_drawn"], 0);
    }

    #[test]
    fn fitting_room_ink_keeps_physical_clearance_instead_of_touching_the_wall() {
        let label = Text::new(Layer::Annotation, [0., 0.], "4.2 m²", 2.5);
        let [x0, y0, x1, y1] = crate::plan_output::text_ink_bounds(&[label]).unwrap()[0].unwrap();
        let radius = [[x0, y0], [x1, y0], [x1, y1], [x0, y1]].into_iter().map(|[x, y]| x.hypot(y) * 100.0).fold(0.0_f64, f64::max);
        for (air, expected) in [(37.5, 0), (85.0, 1)] {
            let mut s = session();
            let width = 2.0 * (radius + air);
            let height = 4_200_000.0 / width;
            s.doc.buildings[0].rooms[0].name.clear();
            s.doc.buildings[0].rooms[0].polygon = vec![
                [6000. - width / 2., 4000. - height / 2.],
                [6000. + width / 2., 4000. - height / 2.],
                [6000. + width / 2., 4000. + height / 2.],
                [6000. - width / 2., 4000. + height / 2.],
            ];
            configure(&mut s, "area", 100., None);
            let sheet = crate::views::sheet(&s.doc, "plan", View::latest(&s.doc)).unwrap();
            assert_eq!(sheet.metrics["room_tags_drawn"], expected, "actual ink needs 0.75 mm paper air, not just mathematical containment");
        }
    }

    #[test]
    fn overview_room_tags_fit_actual_glyphs_with_full_size_and_room_clearance() {
        let mut s = session();
        configure(&mut s, "name_and_area", 250., None);
        let sheet = crate::views::sheet(&s.doc, "plan", View::latest(&s.doc)).unwrap();
        let labels: Vec<_> = sheet
            .scene
            .items
            .iter()
            .filter_map(|item| match item {
                Item::Text(text) if text.source.as_ref().is_some_and(|source| source.id == "room.space.L0") => Some(text.clone()),
                _ => None,
            })
            .collect();
        assert_eq!(labels.len(), 2);
        assert!(labels.iter().any(|text| text.text == "Living room" && text.size == 2.5));
        assert!(labels.iter().any(|text| text.text == "28.0 m²" && text.size == 2.5));
        let bounds = crate::plan_output::text_ink_bounds(&labels).unwrap();
        let (anchor, clearance) = crate::plan::room_label_anchor(&s.doc.buildings[0].rooms[0].polygon).unwrap();
        for (label, bounds) in labels.iter().zip(bounds) {
            let [x0, y0, x1, y1] = bounds.unwrap();
            for [x, y] in [[x0, y0], [x1, y0], [x1, y1], [x0, y1]] {
                let at = [label.at[0] + x * sheet.scene.scale, label.at[1] + y * sheet.scene.scale];
                assert!((at[0] - anchor[0]).hypot(at[1] - anchor[1]) + 0.75 * sheet.scene.scale <= clearance + 1e-6);
            }
        }
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

    #[test]
    fn empty_frame_overrides_hide_generated_text_and_invalid_text_is_atomic() {
        let mut s = session();
        let before = s.doc.clone();
        for (field, value) in [
            ("caption", "W".repeat(trigon_model::annotation::MAX_FRAME_CAPTION_CHARS + 1)),
            ("note", "W".repeat(trigon_model::annotation::MAX_FRAME_NOTE_CHARS + 1)),
            ("caption", "two\nlines".into()),
            ("note", "hidden\0control".into()),
        ] {
            let mut template = json!({"name":"Review"});
            template[field] = json!(value);
            assert!(s.execute("view.configure", json!({"id":"plan","template":template})).is_err());
            assert_eq!(s.doc, before);
        }
        s.execute("view.configure", json!({"id":"plan","template":{"name":"Review","caption":"","note":""}})).unwrap();
        let sheet = crate::views::sheet(&s.doc, "plan", View::latest(&s.doc)).unwrap();
        assert!(sheet.caption.is_empty());
        assert!(sheet.note.is_none());
        s.execute("edit.undo", json!({})).unwrap();
        assert_eq!(s.doc, before);
    }

    #[test]
    fn frame_text_overrides_save_and_undo_without_changing_model_or_picking() {
        let mut s = session();
        s.execute(
            "annotation.define",
            json!({"id":"manual","view":"plan","kind":{
                "annotation":"text","at":[6000,6500],"text":"Manual note stays"
            }}),
        )
        .unwrap();
        s.execute(
            "view.configure",
            json!({"id":"plan","template":{
                "name":"Review","room_labels":"name_and_area",
                "sizing":{"mode":"fixed","denominator":100,"centre":[6000,4000]}
            }}),
        )
        .unwrap();
        let before = s.doc.clone();
        let original = crate::views::sheet(&s.doc, "plan", View::latest(&s.doc)).unwrap();
        let picking = crate::views::drawing_view(&s.doc, "plan", View::latest(&s.doc)).unwrap();
        let mut template = serde_json::to_value(&s.doc.views[0].template).unwrap();
        assert!(template.get("caption").is_none() && template.get("note").is_none());
        template["caption"] = json!("Door detail");
        template["note"] = json!("Detail excerpt; areas are indicative.");
        s.execute("view.configure", json!({"id":"plan","template":template})).unwrap();
        let changed = crate::views::sheet(&s.doc, "plan", View::latest(&s.doc)).unwrap();
        let next_pick = crate::views::drawing_view(&s.doc, "plan", View::latest(&s.doc)).unwrap();
        assert_eq!(changed.caption, "Door detail");
        assert_eq!(changed.note.as_deref(), Some("Detail excerpt; areas are indicative."));
        assert!(changed.svg().contains("Manual note stays") && changed.svg().contains("28.0 m²"));
        assert_eq!(s.doc.buildings, before.buildings);
        assert_eq!(s.doc.types, before.types);
        assert_eq!(s.doc.annotations, before.annotations);
        assert_eq!(next_pick.element_segments, picking.element_segments);
        assert_eq!(next_pick.element_regions, picking.element_regions);
        let loaded: trigon_model::Document = serde_json::from_str(&serde_json::to_string(&s.doc).unwrap()).unwrap();
        assert_eq!(crate::views::sheet(&loaded, "plan", View::latest(&loaded)).unwrap().svg(), changed.svg());
        s.execute("edit.undo", json!({})).unwrap();
        assert_eq!(s.doc, before);
        assert_eq!(crate::views::sheet(&s.doc, "plan", View::latest(&s.doc)).unwrap().svg(), original.svg());
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
    fn room_tags_cannot_reintroduce_rooms_excluded_by_an_unequal_floor_range() {
        let mut s = session();
        let mut high =
            trigon_model::Building::new("high", "High floor", &[[15000., 0.], [27000., 0.], [27000., 8000.], [15000., 8000.]], 1, 3000.).unwrap();
        high.set_levels(&[4000.], 7000.).unwrap();
        high.rooms.push(trigon_model::Room {
            id: "high-room".into(),
            name: "High room".into(),
            levels: vec![0],
            polygon: vec![[16000., 1000.], [23000., 1000.], [23000., 5000.], [16000., 5000.]],
        });
        s.doc.buildings.push(high);
        for mode in ["hidden", "area", "name_and_area"] {
            s.execute(
                "view.configure",
                json!({"id":"plan","template":{"name":"Horizontal range","room_labels":mode,
                "sizing":{"mode":"fixed","denominator":100,"centre":[13500,4000]},
                "range":{"top_mm":3000,"cut_mm":1000,"bottom_mm":0,"depth_mm":-300}}}),
            )
            .unwrap();
            let before = s.doc.clone();
            let sheet = crate::views::sheet(&s.doc, "plan", View::latest(&s.doc)).unwrap();
            assert!(
                !sheet
                    .scene
                    .items
                    .iter()
                    .any(|item| matches!(item, Item::Text(text) if text.source.as_ref().is_some_and(|source| source.id == "high-room.space.L0"))),
                "a room 4000..7000 mm high cannot appear in the -300..3000 mm plan range, mode={mode}"
            );
            let preview = crate::drawing_view(&s.doc, "plan", View::latest(&s.doc)).unwrap();
            assert!(!preview.element_regions.iter().any(|room| room.id == "high-room.space.L0"));
            assert!(preview.pick_element([19500., 3000.], 1.).is_none(), "label fitting cannot create an invisible room pick target");
            if mode != "hidden" {
                assert_eq!(sheet.metrics["room_tags_drawn"], 1);
            }
            assert_eq!(s.doc, before);
        }
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
