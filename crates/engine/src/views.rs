//! Saved 2D workspaces, annotation commands and the shared export/live drawing contract.

use schemars::JsonSchema;
use serde::Deserialize;
use serde_json::{Value, json};
use trigon_model::annotation::{self, Annotation, AnnotationKind, DrawingView, DrawingViewKind};
use trigon_model::build::View;
use trigon_model::{Document, ModelError};

use crate::sheet::{Anchor, Fill, Halo, Item, Layer, Scene, Sheet, Stroke, Text, level_text};
use crate::{Result, Session};

#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub(crate) struct Id {
    pub id: String,
}

#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub(crate) struct ListAnnotations {
    pub view: Option<String>,
}

#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub(crate) struct Export {
    pub id: String,
    pub path: String,
    pub phase: Option<String>,
    pub filter: Option<String>,
}

pub(crate) fn define_view(s: &mut Session, view: DrawingView) -> Result<Value> {
    view.validate_source(&s.doc)?;
    if let Some(old) = s.doc.views.iter_mut().find(|v| v.id == view.id) {
        if old.kind != view.kind && s.doc.annotations.iter().any(|a| a.view == view.id) {
            return Err(ModelError::Invalid("remove the view's annotations before changing its coordinate frame".into()).into());
        }
        *old = view.clone();
    } else {
        if s.doc.views.len() >= annotation::MAX_VIEWS {
            return Err(ModelError::Invalid("the saved view limit is 256".into()).into());
        }
        s.doc.views.push(view.clone());
    }
    Ok(json!({"view":view}))
}

pub(crate) fn list_views(s: &Session) -> Value {
    json!({"views":s.doc.views.iter().map(|v| {
        let error = v.validate_source(&s.doc).err().map(|e|e.to_string());
        json!({"view":v, "valid":error.is_none(), "error":error, "annotations":s.doc.annotations.iter().filter(|a|a.view==v.id).count()})
    }).collect::<Vec<_>>()})
}

pub(crate) fn remove_view(s: &mut Session, id: &str) -> Result<Value> {
    let i = s.doc.views.iter().position(|v| v.id == id).ok_or_else(|| ModelError::NotFound(format!("drawing view {id}")))?;
    if s.doc.annotations.iter().any(|a| a.view == id) {
        return Err(ModelError::Invalid("remove the view's annotations before removing the view".into()).into());
    }
    s.doc.views.remove(i);
    Ok(json!({"removed":id}))
}

pub(crate) fn define_annotation(s: &mut Session, value: Annotation) -> Result<Value> {
    value.validate()?;
    find(&s.doc, &value.view)?.validate_source(&s.doc)?;
    if let Some(old) = s.doc.annotations.iter_mut().find(|a| a.id == value.id) {
        *old = value.clone();
    } else {
        if s.doc.annotations.len() >= annotation::MAX_ANNOTATIONS {
            return Err(ModelError::Invalid("the annotation limit is 5000".into()).into());
        }
        s.doc.annotations.push(value.clone());
    }
    annotation::validate_document(&s.doc)?;
    Ok(json!({"annotation":value}))
}

pub(crate) fn list_annotations(s: &Session, view: Option<&str>) -> Result<Value> {
    if let Some(id) = view {
        find(&s.doc, id)?;
    }
    Ok(json!({"annotations":s.doc.annotations.iter().filter(|a|view.is_none_or(|v|a.view==v)).collect::<Vec<_>>()}))
}

pub(crate) fn remove_annotation(s: &mut Session, id: &str) -> Result<Value> {
    let i = s.doc.annotations.iter().position(|a| a.id == id).ok_or_else(|| ModelError::NotFound(format!("annotation {id}")))?;
    s.doc.annotations.remove(i);
    Ok(json!({"removed":id}))
}

fn find<'a>(doc: &'a Document, id: &str) -> Result<&'a DrawingView> {
    doc.views.iter().find(|v| v.id == id).ok_or_else(|| ModelError::NotFound(format!("drawing view {id}")).into())
}

pub(crate) fn sheet(doc: &Document, id: &str, phase: View) -> Result<Sheet> {
    annotation::validate_document(doc)?;
    let view = find(doc, id)?;
    view.validate_source(doc)?;
    let mut sheet = match &view.kind {
        DrawingViewKind::Plan { level } => crate::plan::drawing_sheet(doc, *level, phase)?,
        DrawingViewKind::Facade { facade } => {
            crate::elevation::scene(doc, crate::elevation::Side::Facade(facade), phase, (None, None), &view.name, "")?
        }
        DrawingViewKind::Section { from, to, flip, depth_mm } => crate::section::scene(doc, [*from, *to], *flip, *depth_mm, phase, &view.name, "")?,
    };
    sheet.title = view.name.clone();
    for a in doc.annotations.iter().filter(|a| a.view == id) {
        draw(&mut sheet.scene, a);
    }
    sheet.metrics["view"] = json!(id);
    sheet.metrics["annotations"] = json!(doc.annotations.iter().filter(|a| a.view == id).count());
    Ok(sheet)
}

fn lettering(scene: &mut Scene, at: [f64; 2], value: &str, size: f64, rotation: f64) {
    let mut t = Text::new(Layer::Annotation, at, value, size);
    t.anchor = Anchor::Start;
    t.middle = false;
    t.deg = rotation;
    t.halo = Halo::Outline;
    scene.text(t);
}

fn draw(scene: &mut Scene, a: &Annotation) {
    let s = scene.scale;
    let pen = Stroke::new(0.18);
    match &a.kind {
        AnnotationKind::Text { at, text, rotation_deg } => lettering(scene, *at, text, a.size_mm, *rotation_deg),
        AnnotationKind::Dimension { from, to, offset_mm } => {
            let length = (to[0] - from[0]).hypot(to[1] - from[1]);
            let d = [(to[0] - from[0]) / length, (to[1] - from[1]) / length];
            let n = [-d[1], d[0]];
            let shift = |p: [f64; 2], u: f64, v: f64| [p[0] + d[0] * u + n[0] * v, p[1] + d[1] * u + n[1] * v];
            let sign = if *offset_mm < 0.0 { -1.0 } else { 1.0 };
            let (a0, b0) = (shift(*from, 0.0, *offset_mm), shift(*to, 0.0, *offset_mm));
            for (p, q) in [(*from, a0), (*to, b0)] {
                scene.line(Layer::Annotation, shift(p, 0.0, sign * 0.8 * s), shift(q, 0.0, sign * 1.5 * s), pen);
                scene.line(Layer::Annotation, shift(q, -1.2 * s, -1.2 * s), shift(q, 1.2 * s, 1.2 * s), pen);
            }
            scene.line(Layer::Annotation, a0, b0, pen);
            let value = if (length - length.round()).abs() < 0.05 { format!("{length:.0}") } else { format!("{length:.1}") };
            let mut t = Text::new(Layer::Annotation, shift([(a0[0] + b0[0]) / 2.0, (a0[1] + b0[1]) / 2.0], 0.0, 1.0 * s), value, a.size_mm);
            let angle = d[1].atan2(d[0]).to_degrees();
            t.deg = if angle > 90.0 {
                angle - 180.0
            } else if angle < -90.0 {
                angle + 180.0
            } else {
                angle
            };
            t.middle = false;
            t.halo = Halo::Box;
            scene.text(t);
        }
        AnnotationKind::Leader { tip, elbow, at, text } => {
            scene.polyline(Layer::Annotation, vec![*tip, *elbow, *at], pen);
            let length = (elbow[0] - tip[0]).hypot(elbow[1] - tip[1]);
            let d = [(elbow[0] - tip[0]) / length, (elbow[1] - tip[1]) / length];
            let size = (2.8 * s).min(length * 0.6);
            scene.polygon(
                Layer::Annotation,
                vec![vec![
                    *tip,
                    [tip[0] + d[0] * size - d[1] * size * 0.3, tip[1] + d[1] * size + d[0] * size * 0.3],
                    [tip[0] + d[0] * size + d[1] * size * 0.3, tip[1] + d[1] * size - d[0] * size * 0.3],
                ]],
                Some(Fill::Solid(crate::sheet::INK)),
                None,
            );
            lettering(scene, [at[0] + s, at[1] + s], text, a.size_mm, 0.0);
        }
        AnnotationKind::Level { at, datum_mm, label } => {
            scene.polygon(
                Layer::Annotation,
                vec![vec![*at, [at[0] + 1.3 * s, at[1] + 2.2 * s], [at[0] - 1.3 * s, at[1] + 2.2 * s]]],
                Some(Fill::Solid(crate::sheet::INK)),
                None,
            );
            scene.line(Layer::Annotation, [at[0] - 3.0 * s, at[1]], [at[0] + 12.0 * s, at[1]], pen);
            let value = level_text(at[1] - datum_mm);
            let text = if label.is_empty() { value } else { format!("{value} {label}") };
            lettering(scene, [at[0] + 2.2 * s, at[1] + 1.0 * s], &text, a.size_mm, 0.0);
        }
    }
}

/// The exact drawing-plane mm ↔ 1600 × 900 sheet-pixel mapping, shared with Studio.
#[derive(Clone, Copy, Debug, PartialEq, serde::Serialize)]
pub struct DrawingTransform {
    pub x0: f64,
    pub y1: f64,
    pub mm_per_px: f64,
}

impl DrawingTransform {
    pub fn to_sheet(self, p: [f64; 2]) -> [f64; 2] {
        [(p[0] - self.x0) / self.mm_per_px, (self.y1 - p[1]) / self.mm_per_px]
    }
    pub fn to_model(self, p: [f64; 2]) -> [f64; 2] {
        [self.x0 + p[0] * self.mm_per_px, self.y1 - p[1] * self.mm_per_px]
    }
}

/// Drawing metadata travels with its SVG, so picking never guesses a scale from the image.
#[derive(Clone, Debug)]
pub struct DrawingPreview {
    pub svg: String,
    pub transform: DrawingTransform,
    /// Visible path segments in drawing-plane mm, for precise endpoint/midpoint/intersection snaps.
    pub segments: Vec<[[f64; 2]; 2]>,
}

fn preview(sheet: Sheet) -> DrawingPreview {
    let mut segments = Vec::new();
    for item in &sheet.scene.items {
        let Item::Path(path) = item else { continue };
        for ring in &path.rings {
            segments.extend(ring.windows(2).map(|w| [w[0], w[1]]));
            if path.closed && ring.len() > 2 {
                segments.push([ring[ring.len() - 1], ring[0]]);
            }
        }
    }
    DrawingPreview { svg: sheet.svg(), transform: DrawingTransform { x0: sheet.vp.x0, y1: sheet.vp.y1, mm_per_px: sheet.vp.mm }, segments }
}

pub fn drawing_view(doc: &Document, id: &str, phase: View) -> Result<DrawingPreview> {
    Ok(preview(sheet(doc, id, phase)?))
}
pub fn plan_drawing(doc: &Document, level: usize, phase: View) -> Result<DrawingPreview> {
    Ok(preview(crate::plan::drawing_sheet(doc, level, phase)?))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dimensions_keep_measured_model_coordinates_and_scale_only_the_lettering() {
        let a = Annotation {
            id: "d".into(),
            view: "v".into(),
            size_mm: 2.5,
            kind: AnnotationKind::Dimension { from: [100.0, 200.0], to: [3100.0, 4200.0], offset_mm: 500.0 },
        };
        let mut small = Scene::new(50.0);
        let mut large = Scene::new(100.0);
        draw(&mut small, &a);
        draw(&mut large, &a);
        let main =
            |s: &Scene| s.items.iter().filter_map(|i| if let Item::Path(p) = i { Some(p) } else { None }).next_back().unwrap().rings[0].clone();
        assert_eq!(main(&small), vec![[-300.0, 500.0], [2700.0, 4500.0]]);
        assert_eq!(main(&small), main(&large));
        for s in [&small, &large] {
            let t = s.items.iter().find_map(|i| if let Item::Text(t) = i { Some(t) } else { None }).unwrap();
            assert_eq!(t.text, "5000");
            assert_eq!(t.size, 2.5);
            assert!((t.deg - 53.13010235415598).abs() < 1e-8);
        }
    }
}
