//! Typed, atomic saved-view configuration. Templates are stored with a view, then copied from
//! that view by command; no external template file or paid service is required.
use crate::sheet::{Item, Layer, MM_PER_PX, PAGE_MM, Sheet, Viewport, scale_text};
use crate::{Result, Session};
use schemars::JsonSchema;
use serde::Deserialize;
use serde_json::{Value, json};
use trigon_model::{
    ModelError,
    annotation::{DrawingTemplate, LineWeightProfile, ViewSizing},
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
fn clip(sheet: &mut Sheet, crop: [f64; 4]) {
    sheet.scene.clip(crop);
    sheet.scene.items.retain(|i| match i {
        Item::Text(t) => t.at[0] >= crop[0] && t.at[0] <= crop[2] && t.at[1] >= crop[1] && t.at[1] <= crop[3],
        _ => true,
    });
}
