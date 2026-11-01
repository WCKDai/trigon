//! Saved 2D workspaces, annotation commands and the shared export/live drawing contract.

use schemars::JsonSchema;
use serde::Deserialize;
use serde_json::{Value, json};
use trigon_model::annotation::{self, Annotation, AnnotationKind, DrawingView, DrawingViewKind};
use trigon_model::build::View;
use trigon_model::{Document, ModelError};

use crate::sheet::{Item, Scene, Sheet};
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
        let same_frame = match (&old.kind, &view.kind) {
            (DrawingViewKind::Section { from: a, to: b, flip: c, .. }, DrawingViewKind::Section { from: d, to: e, flip: f, .. }) => {
                a == d && b == e && c == f
            }
            (a, b) => a == b,
        };
        if !same_frame && s.doc.annotations.iter().any(|a| a.view == view.id) {
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
        DrawingViewKind::Plan { level } => match view.template.as_ref().and_then(|t| t.range) {
            Some(range) => crate::plan_range::sheet(doc, *level, phase, range)?,
            None => crate::plan::drawing_sheet(doc, *level, phase)?,
        },
        DrawingViewKind::Facade { facade } => {
            crate::elevation::scene(doc, crate::elevation::Side::Facade(facade), phase, (None, None), &view.name, "")?
        }
        DrawingViewKind::Section { from, to, flip, depth_mm } => crate::section::scene(doc, [*from, *to], *flip, *depth_mm, phase, &view.name, "")?,
    };
    sheet.title = view.name.clone();
    if let Some(template) = &view.template {
        let mut extent = sheet.scene.clone();
        for a in doc.annotations.iter().filter(|a| a.view == id) {
            draw(&mut extent, a);
        }
        let mut bounds = extent.bounds();
        for item in &extent.items {
            if let Item::Text(t) = item {
                let b = bounds.get_or_insert([t.at[0], t.at[1], t.at[0], t.at[1]]);
                // A conservative lettering box keeps insertion points and complete labels in fit.
                let pad = t.size * extent.scale;
                b[0] = b[0].min(t.at[0] - pad);
                b[1] = b[1].min(t.at[1] - pad);
                b[2] = b[2].max(t.at[0] + pad * t.text.chars().count() as f64);
                b[3] = b[3].max(t.at[1] + pad);
            }
        }
        crate::view_config::prepare(&mut sheet, template, bounds);
    }
    let mut regions = Scene::new(sheet.scene.scale);
    for a in doc.annotations.iter().filter(|a| a.view == id && matches!(a.kind, AnnotationKind::Region { .. })) {
        draw(&mut regions, a);
    }
    regions.items.append(&mut sheet.scene.items);
    sheet.scene.items = regions.items;
    for a in doc.annotations.iter().filter(|a| a.view == id && !matches!(a.kind, AnnotationKind::Region { .. })) {
        draw(&mut sheet.scene, a);
    }
    if let Some(template) = &view.template {
        crate::view_config::finish(&mut sheet, template);
    }
    sheet.metrics["view"] = json!(id);
    sheet.metrics["annotations"] = json!(doc.annotations.iter().filter(|a| a.view == id).count());
    Ok(sheet)
}

use crate::annotation_style::draw;

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
    pub element_segments: Vec<DrawingElementSegment>,
    /// Filled semantic areas, used only after no visible boundary stroke is within tolerance.
    pub element_regions: Vec<DrawingElementRegion>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, serde::Serialize)]
#[serde(rename_all = "snake_case")]
pub enum DrawingElementKind {
    Opening,
    InteriorWall,
    FacadeWall,
    Room,
    Stair,
    Balcony,
    Railing,
    Roof,
    Slab,
}

#[derive(Clone, Debug, PartialEq, serde::Serialize)]
pub struct DrawingElementRegion {
    pub id: String,
    pub kind: DrawingElementKind,
    /// Drawing-plane mm; even-odd rings allow holes. This metadata does not change graphics.
    pub rings: Vec<Vec<[f64; 2]>>,
    /// Orthographic depth = c + gx*x + gy*y. None for plan-only semantic areas.
    pub depth_plane: Option<[f64; 3]>,
}

#[derive(Clone, Debug, PartialEq, serde::Serialize)]
pub struct DrawingElementSegment {
    pub id: String,
    pub kind: DrawingElementKind,
    pub segment: [[f64; 2]; 2],
}

#[derive(Clone, Debug, PartialEq, serde::Serialize)]
pub struct DrawingElementHit {
    pub id: String,
    pub kind: DrawingElementKind,
    pub point: [f64; 2],
    pub distance_mm: f64,
}

impl DrawingPreview {
    pub fn pick_element(&self, point: [f64; 2], tolerance_mm: f64) -> Option<DrawingElementHit> {
        if point.iter().any(|value| !value.is_finite() || value.abs() > 1_000_000.0)
            || !tolerance_mm.is_finite()
            || !(0.0..=100_000.0).contains(&tolerance_mm)
        {
            return None;
        }
        let boundary = self
            .element_segments
            .iter()
            .filter_map(|element| {
                let [start, end] = element.segment;
                if element.id.is_empty() || element.segment.iter().flatten().any(|value| !value.is_finite() || value.abs() > 1e8) {
                    return None;
                }
                let direction = [end[0] - start[0], end[1] - start[1]];
                let length2 = direction[0] * direction[0] + direction[1] * direction[1];
                if length2 <= 1e-16 {
                    return None;
                }
                let along = (((point[0] - start[0]) * direction[0] + (point[1] - start[1]) * direction[1]) / length2).clamp(0.0, 1.0);
                let nearest = [start[0] + along * direction[0], start[1] + along * direction[1]];
                let distance_mm = (point[0] - nearest[0]).hypot(point[1] - nearest[1]);
                (distance_mm <= tolerance_mm).then(|| DrawingElementHit { id: element.id.clone(), kind: element.kind, point: nearest, distance_mm })
            })
            .min_by(|left, right| {
                left.distance_mm
                    .total_cmp(&right.distance_mm)
                    .then(left.kind.cmp(&right.kind))
                    .then(left.id.cmp(&right.id))
                    .then(left.point[0].total_cmp(&right.point[0]))
                    .then(left.point[1].total_cmp(&right.point[1]))
            });
        if boundary.is_some() {
            return boundary;
        }
        self.element_regions
            .iter()
            .filter(|region| {
                !region.id.is_empty()
                    && !region.rings.is_empty()
                    && region.rings.len() <= 1024
                    && region
                        .rings
                        .iter()
                        .all(|ring| (3..=10_000).contains(&ring.len()) && ring.iter().flatten().all(|value| value.is_finite() && value.abs() <= 1e8))
                    && region
                        .depth_plane
                        .is_none_or(|plane| plane.iter().all(|v| v.is_finite()) && (plane[0] + plane[1] * point[0] + plane[2] * point[1]).is_finite())
                    && region.rings.iter().filter(|ring| crate::drawings::inside(ring, point)).count() % 2 == 1
            })
            .min_by(|left, right| {
                let area = |region: &DrawingElementRegion| {
                    region
                        .rings
                        .iter()
                        .map(|ring| {
                            ring.iter().zip(ring.iter().cycle().skip(1)).take(ring.len()).map(|(a, b)| a[0] * b[1] - b[0] * a[1]).sum::<f64>().abs()
                        })
                        .sum::<f64>()
                };
                let depth =
                    |region: &DrawingElementRegion| region.depth_plane.map_or(0.0, |plane| plane[0] + plane[1] * point[0] + plane[2] * point[1]);
                depth(left)
                    .total_cmp(&depth(right))
                    .then(area(left).total_cmp(&area(right)))
                    .then(left.kind.cmp(&right.kind))
                    .then(left.id.cmp(&right.id))
            })
            .map(|region| DrawingElementHit { id: region.id.clone(), kind: region.kind, point, distance_mm: 0.0 })
    }
}

fn preview(sheet: Sheet) -> DrawingPreview {
    let mut segments = Vec::new();
    let mut element_segments = Vec::new();
    let [left, top, width, height] = sheet.clip;
    let [x0, y1] = sheet.vp.at([left, top]);
    let [x1, y0] = sheet.vp.at([left + width, top + height]);
    for item in &sheet.scene.items {
        let Item::Path(path) = item else { continue };
        for ring in &path.rings {
            segments.extend(ring.windows(2).map(|w| [w[0], w[1]]));
            if path.closed && ring.len() > 2 {
                segments.push([ring[ring.len() - 1], ring[0]]);
            }
        }
        if path.stroke.is_some() {
            for element in &path.element_segments {
                for segment in path.segments() {
                    if let Some([start, end]) = crate::sheet::overlap(segment, element.segment, 0.5)
                        && let Some((start, end)) = crate::sheet::clip_segment(start, end, [x0, y0, x1, y1])
                    {
                        let segment = [start, end].map(|point| {
                            point.map(|coordinate| {
                                let coordinate = (coordinate * 100_000_000.0).round() / 100_000_000.0;
                                if coordinate == 0.0 { 0.0 } else { coordinate }
                            })
                        });
                        let segment = if segment[0] <= segment[1] { segment } else { [segment[1], segment[0]] };
                        if segment[0] != segment[1] {
                            element_segments.push(DrawingElementSegment { segment, ..element.clone() });
                        }
                    }
                }
            }
        }
    }
    element_segments.sort_by(|left, right| {
        left.kind.cmp(&right.kind).then(left.id.cmp(&right.id)).then_with(|| {
            left.segment
                .iter()
                .flatten()
                .zip(right.segment.iter().flatten())
                .map(|(left, right)| left.total_cmp(right))
                .find(|order| !order.is_eq())
                .unwrap_or(std::cmp::Ordering::Equal)
        })
    });
    element_segments.dedup();
    let clip = vec![[x0, y0], [x1, y0], [x1, y1], [x0, y1]];
    let element_regions = sheet
        .scene
        .element_regions
        .iter()
        .flat_map(|region| {
            crate::sheet::cut::intersect_rings(&region.rings, &clip).into_iter().map(|rings| DrawingElementRegion { rings, ..region.clone() })
        })
        .collect();
    DrawingPreview {
        svg: sheet.svg(),
        transform: DrawingTransform { x0: sheet.vp.x0, y1: sheet.vp.y1, mm_per_px: sheet.vp.mm },
        segments,
        element_segments,
        element_regions,
    }
}

/// Three immutable display variants per document generation. Only `exact` supplies picking data.
/// Reduced/overview styles are for screens; all exports continue to use the exact scene.
pub struct DrawingDisplay {
    pub exact: DrawingPreview,
    pub reduced_svg: String,
    pub overview_svg: String,
}
fn display(sheet: Sheet) -> DrawingDisplay {
    let reduced_svg = crate::sheet::display::svg_at(&sheet, 1);
    let overview_svg = crate::sheet::display::svg_at(&sheet, 2);
    DrawingDisplay { exact: preview(sheet), reduced_svg, overview_svg }
}
pub fn drawing_view_display(doc: &Document, id: &str, phase: View) -> Result<DrawingDisplay> {
    Ok(display(sheet(doc, id, phase)?))
}
pub fn plan_drawing_display(doc: &Document, level: usize, phase: View) -> Result<DrawingDisplay> {
    Ok(display(crate::plan::drawing_sheet(doc, level, phase)?))
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
            style: Default::default(),
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

#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "snake_case")]
pub enum SnapKind {
    Endpoint,
    Intersection,
    Midpoint,
    Grid,
    Alignment,
}

impl SnapKind {
    pub fn name(self) -> &'static str {
        match self {
            Self::Endpoint => "Endpoint",
            Self::Intersection => "Intersection",
            Self::Midpoint => "Midpoint",
            Self::Grid => "Grid",
            Self::Alignment => "Alignment",
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, serde::Serialize)]
pub struct DrawingSnap {
    pub point: [f64; 2],
    pub kind: SnapKind,
}

#[derive(Clone, Copy, Debug, serde::Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct SnapOptions {
    #[serde(default = "yes")]
    pub endpoints: bool,
    #[serde(default = "yes")]
    pub midpoints: bool,
    #[serde(default = "yes")]
    pub intersections: bool,
    pub grid_mm: Option<f64>,
}

fn yes() -> bool {
    true
}
impl Default for SnapOptions {
    fn default() -> Self {
        Self { endpoints: true, midpoints: true, intersections: true, grid_mm: Some(100.0) }
    }
}

/// Select the closest precise candidate within tolerance. Intersections are bounded to the 128
/// nearest segments crossing the cursor neighbourhood, avoiding quadratic work on whole facades.
pub fn snap_drawing(segments: &[[[f64; 2]; 2]], point: [f64; 2], tolerance_mm: f64, options: SnapOptions) -> Option<DrawingSnap> {
    if !point.iter().all(|v| v.is_finite() && v.abs() <= 1_000_000.0) || !tolerance_mm.is_finite() || !(0.0..=100_000.0).contains(&tolerance_mm) {
        return None;
    }
    let distance = |p: [f64; 2]| (p[0] - point[0]).hypot(p[1] - point[1]);
    let mut best: Option<(f64, DrawingSnap)> = None;
    let mut consider = |p: [f64; 2], kind: SnapKind| {
        if p.iter().any(|v| !v.is_finite() || v.abs() > 1_000_000.0) {
            return;
        }
        let d = distance(p);
        if d <= tolerance_mm && best.as_ref().is_none_or(|(b, s)| d < *b - 1e-8 || ((d - *b).abs() < 1e-8 && (kind as u8) < s.kind as u8)) {
            best = Some((d, DrawingSnap { point: p, kind }));
        }
    };
    let mut nearby = Vec::new();
    for &[a, b] in segments {
        if [a, b].iter().flatten().any(|v| !v.is_finite() || v.abs() > 10_000_000.0) {
            continue;
        }
        if options.endpoints {
            consider(a, SnapKind::Endpoint);
            consider(b, SnapKind::Endpoint);
        }
        if options.midpoints {
            consider([(a[0] + b[0]) / 2.0, (a[1] + b[1]) / 2.0], SnapKind::Midpoint);
        }
        if options.intersections {
            let d = [b[0] - a[0], b[1] - a[1]];
            let length2 = d[0] * d[0] + d[1] * d[1];
            if length2 > 1e-12 {
                let t = (((point[0] - a[0]) * d[0] + (point[1] - a[1]) * d[1]) / length2).clamp(0.0, 1.0);
                let dist = distance([a[0] + t * d[0], a[1] + t * d[1]]);
                if dist <= tolerance_mm {
                    nearby.push((dist, [a, b]));
                }
            }
        }
    }
    nearby.sort_by(|a, b| a.0.total_cmp(&b.0));
    nearby.truncate(128);
    for (i, (_, [a, b])) in nearby.iter().enumerate() {
        let r = [b[0] - a[0], b[1] - a[1]];
        for (_, [c, d]) in nearby.iter().skip(i + 1) {
            let s = [d[0] - c[0], d[1] - c[1]];
            let cross = |r: [f64; 2], s: [f64; 2]| r[0] * s[1] - r[1] * s[0];
            let denominator = cross(r, s);
            if denominator.abs() <= 1e-10 * r[0].hypot(r[1]) * s[0].hypot(s[1]) {
                continue;
            }
            let q = [c[0] - a[0], c[1] - a[1]];
            let (t, u) = (cross(q, s) / denominator, cross(q, r) / denominator);
            if (0.0..=1.0).contains(&t) && (0.0..=1.0).contains(&u) {
                consider([a[0] + t * r[0], a[1] + t * r[1]], SnapKind::Intersection);
            }
        }
    }
    if let Some((_, snap)) = best {
        return Some(snap);
    }
    if let Some(step) = options.grid_mm.filter(|s| s.is_finite() && (0.01..=1_000_000.0).contains(s)) {
        let p = [(point[0] / step).round() * step, (point[1] / step).round() * step];
        if p.iter().all(|v| v.is_finite() && v.abs() <= 1_000_000.0) && distance(p) <= tolerance_mm {
            return Some(DrawingSnap { point: p, kind: SnapKind::Grid });
        }
    }
    None
}

#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub(crate) struct SnapRequest {
    pub id: String,
    pub point: [f64; 2],
    /// Search radius in drawing-plane mm (0.001 to 100000).
    pub tolerance_mm: f64,
    #[serde(default)]
    pub options: SnapOptions,
    pub alignment: Option<crate::AlignmentOptions>,
    pub phase: Option<String>,
    pub filter: Option<String>,
}

#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub(crate) struct PickRequest {
    pub id: String,
    pub point: [f64; 2],
    pub tolerance_mm: f64,
    pub phase: Option<String>,
    pub filter: Option<String>,
}

pub(crate) fn pick(doc: &Document, request: PickRequest, view: View) -> Result<Value> {
    for coordinate in request.point {
        trigon_model::checked("point", coordinate, -1_000_000.0, 1_000_000.0)?;
    }
    trigon_model::checked("tolerance_mm", request.tolerance_mm, 0.0, 100_000.0)?;
    let preview = drawing_view(doc, &request.id, view)?;
    Ok(json!({"hit":preview.pick_element(request.point, request.tolerance_mm)}))
}

pub(crate) fn snap(doc: &Document, p: SnapRequest, view: View) -> Result<Value> {
    for coordinate in p.point {
        trigon_model::checked("point", coordinate, -1_000_000.0, 1_000_000.0)?;
    }
    trigon_model::checked("tolerance_mm", p.tolerance_mm, 0.001, 100_000.0)?;
    if let Some(grid) = p.options.grid_mm {
        trigon_model::checked("grid_mm", grid, 0.01, 1_000_000.0)?;
    }
    if let Some(settings) = &p.alignment {
        settings.validate()?;
    }
    let preview = drawing_view(doc, &p.id, view)?;
    let snapped = crate::snap_drawing_aligned(&preview.segments, p.point, p.tolerance_mm, p.options, p.alignment.as_ref());
    Ok(
        json!({"point":snapped.as_ref().map_or(p.point,|s|s.snap.point),"snap":snapped.as_ref().map(|s|s.snap),"guides":snapped.map(|s|s.guides).unwrap_or_default()}),
    )
}

#[cfg(test)]
mod picking_tests {
    use super::*;
    use crate::sheet::{Layer, Stroke};

    fn session() -> Session {
        let mut session = Session::new(Box::new(trigon_data::Live::new(trigon_data::Credentials::default())));
        session.doc.buildings.push(
            trigon_model::Building::new("b", "Picking fixture", &[[0.0, 0.0], [12000.0, 0.0], [12000.0, 8000.0], [0.0, 8000.0]], 2, 3000.0).unwrap(),
        );
        for (command, params) in [
            ("facade.set_bays", json!({"facade":"b.f1","count":3})),
            ("facade.place", json!({"facade":"b.f1","type":"window.casement.2","levels":[0],"bays":[0]})),
            ("facade.place", json!({"facade":"b.f1","type":"door.entrance","levels":[0],"bays":[2]})),
            (
                "interior.add_walls",
                json!({"walls":[{"id":"partition","a":[2000,4000],"b":[10000,4000],"thickness":150,"levels":[0],"doors":[{"at":4000,"width":900}]}]}),
            ),
            ("view.define", json!({"id":"plan","name":"Plan","kind":{"view":"plan","level":0}})),
            ("view.define", json!({"id":"facade","name":"Facade","kind":{"view":"facade","facade":"b.f1"}})),
            ("view.define", json!({"id":"section","name":"Section","kind":{"view":"section","from":[0,2000],"to":[12000,2000]}})),
        ] {
            session.execute(command, params).unwrap();
        }
        session
    }

    #[test]
    fn manual_lines_keep_style_in_all_exports_round_trip_and_undo_exactly() {
        let mut session = session();
        for pattern in ["solid", "dashed", "dotted", "dash_dot"] {
            let before = session.doc.clone();
            let payload = json!({"id":"separation","view":"plan","kind":{"annotation":"line","from":[1500,1200],"to":[6500,1200],"stroke":{"colour":[184,65,44],"width_mm":0.5,"pattern":pattern}}});
            session.execute("annotation.define", payload.clone()).unwrap();
            let saved = serde_json::to_string(&session.doc).unwrap();
            let loaded: Document = serde_json::from_str(&saved).unwrap();
            assert_eq!(loaded, session.doc);
            let scene = sheet(&session.doc, "plan", View::latest(&session.doc)).unwrap();
            let svg = scene.svg();
            assert!(svg.contains("stroke=\"#b8412c\" stroke-width=\"2.362\""));
            assert_eq!(svg.contains("stroke-dasharray=\"0 4.72\""), pattern == "dotted");
            assert_eq!(svg.contains("stroke-dasharray=\"9.45 4.72 0 4.72\""), pattern == "dash_dot");
            let dxf = crate::sheet::dxf::write(
                &scene.scene,
                &crate::sheet::dxf::TitleBlock { title: "Fixture", project: "Synthetic", caption: "Manual separation", note: None },
            );
            assert!(dxf.contains("62\n1\n"));
            assert!(dxf.contains("370\n50\n"));
            if pattern == "dotted" {
                assert!(dxf.contains("6\nDOTTED\n"));
            }
            if pattern == "dash_dot" {
                assert!(dxf.contains("6\nDASHDOT\n"));
            }
            let png = crate::plan_output::png(&svg).unwrap();
            assert_eq!(&png[..8], b"\x89PNG\r\n\x1a\n");
            let pdf = crate::plan_output::pdf(&[svg], crate::sheet::PAGE_MM).unwrap();
            let parsed = lopdf::Document::load_mem(&pdf).unwrap();
            assert_eq!(parsed.get_pages().len(), 1);
            let preview = drawing_view(&session.doc, "plan", View::latest(&session.doc)).unwrap();
            assert!(preview.segments.contains(&[[1500.0, 1200.0], [6500.0, 1200.0]]));
            assert!(!preview.element_segments.iter().any(|s| s.id == "separation"));
            for bad in [json!(-0.1), json!(0), json!(f64::MAX)] {
                let mut bad_payload = payload.clone();
                bad_payload["kind"]["stroke"]["width_mm"] = bad;
                let valid_doc = session.doc.clone();
                assert!(session.execute("annotation.define", bad_payload).is_err());
                assert_eq!(session.doc, valid_doc);
            }
            session.execute("edit.undo", json!({})).unwrap();
            assert_eq!(session.doc, before);
        }
    }

    #[test]
    fn configured_views_have_physical_scale_clipping_reusable_pens_and_exact_undo() {
        let mut session = session();
        session.execute("annotation.define",json!({"id":"manual","view":"plan","kind":{"annotation":"line","from":[0,2000],"to":[12000,2000],"stroke":{"colour":[184,65,44],"width_mm":0.5,"pattern":"dotted"}}})).unwrap();
        let before = session.doc.clone();
        let template = json!({"name":"Plan 100","line_weights":"fine","sizing":{"mode":"fixed","denominator":100,"centre":[6000,4000]},"crop":[-1000,-1000,10000,7000]});
        session.execute("view.configure", json!({"id":"plan","template":template})).unwrap();
        let configured = session.doc.clone();
        let plan = sheet(&session.doc, "plan", View::latest(&session.doc)).unwrap();
        let a = plan.vp.px([0.0, 0.0]);
        let b = plan.vp.px([10000.0, 0.0]);
        assert!(((b[0] - a[0]) * crate::sheet::MM_PER_PX - 100.0).abs() < 1e-8, "10m must print as 100mm at 1:100");
        assert_eq!(plan.scene.scale, 100.0);
        assert_eq!(plan.metrics["sizing"], "fixed");
        let pdf = crate::plan_output::pdf(&[plan.svg()], crate::sheet::PAGE_MM).unwrap();
        let parsed = lopdf::Document::load_mem(&pdf).unwrap();
        let page = parsed.get_dictionary(*parsed.get_pages().values().next().unwrap()).unwrap();
        let media = page.get(b"MediaBox").unwrap().as_array().unwrap();
        assert!((f64::from(media[2].as_float().unwrap()) * 25.4 / 72.0 - crate::sheet::PAGE_MM[0]).abs() < 0.001);

        assert!(
            plan.scene
                .items
                .iter()
                .filter_map(|i| if let Item::Path(p) = i { Some(p) } else { None })
                .flat_map(|p| p.rings.iter().flatten())
                .all(|p| p[0] >= -1000.0 - 1e-8 && p[0] <= 10000.0 + 1e-8 && p[1] >= -1000.0 - 1e-8 && p[1] <= 7000.0 + 1e-8)
        );
        let line = plan
            .scene
            .items
            .iter()
            .filter_map(|i| if let Item::Path(p) = i { Some(p) } else { None })
            .find(|p| p.stroke.is_some_and(|s| s.colour == [184, 65, 44]))
            .unwrap();
        assert_eq!(line.stroke.unwrap().pen, 0.5, "manual paper weights are explicit and preserved");
        assert!(
            plan.scene
                .items
                .iter()
                .any(|i| matches!(i,Item::Path(p) if p.layer==Layer::WallCut && p.stroke.is_some_and(|s|(s.pen-0.2625).abs()<1e-8)))
        );
        let loaded: Document = serde_json::from_str(&serde_json::to_string(&session.doc).unwrap()).unwrap();
        assert_eq!(loaded, session.doc);
        session.execute("view.apply_template", json!({"source":"plan","targets":["facade"]})).unwrap();
        assert_eq!(
            serde_json::to_value(&session.doc.views[0]).unwrap()["template"],
            serde_json::to_value(&session.doc.views[1]).unwrap()["template"]
        );
        session.execute("edit.undo", json!({})).unwrap();
        assert_eq!(session.doc, configured);
        let invalid = [
            json!({"name":"bad","sizing":{"mode":"fixed","denominator":0,"centre":[0,0]}}),
            json!({"name":"bad","crop":[10,0,0,10]}),
            json!({"name":"bad","range":{"top_mm":2300,"cut_mm":-10,"bottom_mm":0,"depth_mm":-1000}}),
        ];
        for template in invalid {
            assert!(session.execute("view.configure", json!({"id":"plan","template":template})).is_err());
            assert_eq!(session.doc, configured);
        }
        assert!(session.execute("view.apply_template", json!({"source":"plan","targets":["facade","missing"]})).is_err());
        assert_eq!(session.doc, configured);
        session.execute("edit.undo", json!({})).unwrap();
        assert_eq!(session.doc, before);
    }

    #[test]
    fn plan_ranges_change_cut_geometry_and_include_adjacent_overhead_only_within_top() {
        let mut s = session();
        s.execute("interior.add_walls", json!({"walls":[{"id":"upper","a":[2000,2000],"b":[10000,2000],"thickness":150,"levels":[1]}]})).unwrap();
        let before = s.doc.clone();
        let configure = |s: &mut Session, top, cut| {
            s.execute(
                "view.configure",
                json!({"id":"plan","template":{"name":"Range","range":{"top_mm":top,"cut_mm":cut,"bottom_mm":0,"depth_mm":-1000}}}),
            )
            .unwrap()
        };
        configure(&mut s, 2300, 1200);
        let low = sheet(&s.doc, "plan", View::latest(&s.doc)).unwrap();
        assert!(!drawing_view(&s.doc, "plan", View::latest(&s.doc)).unwrap().element_segments.iter().any(|e| e.id == "upper.L1"));
        configure(&mut s, 4500, 1200);
        let high = sheet(&s.doc, "plan", View::latest(&s.doc)).unwrap();
        assert!(drawing_view(&s.doc, "plan", View::latest(&s.doc)).unwrap().element_segments.iter().any(|e| e.id == "upper.L1"));
        assert!(
            high.scene
                .items
                .iter()
                .any(|i| matches!(i,Item::Path(p) if p.stroke.is_some_and(|s|s.dashed) && p.element_segments.iter().any(|e|e.id=="upper.L1")))
        );
        assert_ne!(low.svg(), high.svg());
        configure(&mut s, 2300, 500);
        let lower_cut = sheet(&s.doc, "plan", View::latest(&s.doc)).unwrap();
        assert_ne!(low.scene.items, lower_cut.scene.items, "cut plane must change actual wall/opening intersections");
        for _ in 0..3 {
            s.execute("edit.undo", json!({})).unwrap();
        }
        assert_eq!(s.doc, before);
    }

    #[test]
    fn fit_includes_far_manual_notes_and_range_settings_cannot_leak_into_facades() {
        let mut s = session();
        s.execute("annotation.define", json!({"id":"outside","view":"plan","kind":{"annotation":"text","at":[28000,20000],"text":"Proposal"}}))
            .unwrap();
        s.execute("view.configure", json!({"id":"plan","template":{"name":"Fit notes"}})).unwrap();
        let plan = sheet(&s.doc, "plan", View::latest(&s.doc)).unwrap();
        let p = plan.vp.px([28000.0, 20000.0]);
        assert!(p[0] >= plan.clip[0] && p[0] <= plan.clip[0] + plan.clip[2] && p[1] >= plan.clip[1] && p[1] <= plan.clip[1] + plan.clip[3]);
        let original = s.doc.clone();
        for bad in [
            json!({"top_mm":1000,"cut_mm":1200,"bottom_mm":0,"depth_mm":-1000}),
            json!({"top_mm":2300,"cut_mm":1200,"bottom_mm":0,"depth_mm":500}),
            json!({"top_mm":1e300,"cut_mm":1200,"bottom_mm":0,"depth_mm":-1000}),
        ] {
            assert!(s.execute("view.configure", json!({"id":"plan","template":{"name":"Bad","range":bad}})).is_err());
            assert_eq!(s.doc, original);
        }
        assert!(
            s.execute(
                "view.configure",
                json!({"id":"facade","template":{"name":"Bad","range":{"top_mm":2300,"cut_mm":1200,"bottom_mm":0,"depth_mm":-1000}}})
            )
            .is_err()
        );
        assert_eq!(s.doc, original);
        s.execute("view.configure", json!({"id":"plan","template":null})).unwrap();
        assert!(serde_json::to_value(&s.doc.views[0]).unwrap().get("template").is_none());
    }

    #[test]
    fn screen_detail_variants_keep_exact_geometry_transforms_and_legacy_exports() {
        let s = session();
        for id in ["plan", "facade", "section"] {
            // Compare the same generated scene before/after LOD preparation. Separate projection
            // runs may order raw snap rings differently; semantic picks and export fingerprints
            // have their independent repeated-generation regressions below.
            let source = sheet(&s.doc, id, View::latest(&s.doc)).unwrap();
            let original = preview(source.clone());
            let display = display(source);
            assert_eq!(display.exact.svg, original.svg);
            assert_eq!(display.exact.transform, original.transform);
            assert_eq!(display.exact.segments, original.segments);
            assert_eq!(display.exact.element_segments, original.element_segments);
            assert_eq!(display.exact.element_regions, original.element_regions);
            for svg in [display.reduced_svg, display.overview_svg] {
                assert_eq!(crate::parse_svg(&svg).unwrap().size(), crate::parse_svg(&original.svg).unwrap().size());
            }
        }
    }

    #[test]
    fn ranged_plan_keeps_demolition_geometry_visible_only_in_requested_phase_filters() {
        let mut s = session();
        s.doc.phases.push(trigon_model::doc::Phase { id: "proposal".into(), name: "Proposal".into() });
        s.doc.active_phase = Some("proposal".into());
        let original = &mut s.doc.buildings[0].facades[0].placements[0];
        original.demolished = Some("proposal".into());
        let mut replacement = original.clone();
        replacement.created = Some("proposal".into());
        replacement.demolished = None;
        replacement.bay = 1;
        s.doc.buildings[0].facades[0].placements.push(replacement);
        s.execute(
            "view.configure",
            json!({"id":"plan","template":{"name":"Phases","range":{"top_mm":2300,"cut_mm":1200,"bottom_mm":0,"depth_mm":-1000}}}),
        )
        .unwrap();
        let before = s.doc.clone();
        let phase = s.doc.phase_index("proposal").unwrap();
        for (filter, old, new) in [
            (trigon_model::build::PhaseFilter::Complete, false, true),
            (trigon_model::build::PhaseFilter::All, true, true),
            (trigon_model::build::PhaseFilter::Demolition, true, false),
        ] {
            let drawing = drawing_view(&s.doc, "plan", View { phase, filter }).unwrap();
            assert_eq!(
                drawing.element_segments.iter().any(|e| e.id == "b.f1.L0.B0"),
                old,
                "{filter:?}: demolished source must follow requested visibility"
            );
            assert_eq!(drawing.svg.contains("#bd3030"), old);
            assert_eq!(drawing.element_segments.iter().any(|e| e.id == "b.f1.L0.B1"), new, "{filter:?}: new source must follow requested visibility");
        }
        assert_eq!(s.doc, before);
    }

    fn fingerprint(bytes: &[u8]) -> u64 {
        bytes.iter().fold(0xcbf29ce484222325, |hash, byte| (hash ^ u64::from(*byte)).wrapping_mul(0x100000001b3))
    }

    #[test]
    fn drawing_outputs_retain_baseline_bytes() {
        let session = session();
        for (id, expected) in [
            ("plan", [10244507659954298824, 11109581394211914227]),
            ("facade", [12304697737480228178, 7625715272030178447]),
            ("section", [13566637633045740336, 3463606591175843950]),
        ] {
            let sheet = sheet(&session.doc, id, View::latest(&session.doc)).unwrap();
            let dxf = crate::sheet::dxf::write(
                &sheet.scene,
                &crate::sheet::dxf::TitleBlock { title: &sheet.title, project: &sheet.project, caption: &sheet.caption, note: sheet.note.clone() },
            );
            let legacy_segments: Vec<_> = sheet
                .scene
                .items
                .iter()
                .filter_map(|item| match item {
                    Item::Path(path) => Some(path.rings.iter().flat_map(move |ring| {
                        ring.windows(2).map(|pair| [pair[0], pair[1]]).chain((path.closed && ring.len() > 2).then(|| [ring[ring.len() - 1], ring[0]]))
                    })),
                    _ => None,
                })
                .flatten()
                .collect();
            let preview = preview(sheet);
            assert_eq!(
                [fingerprint(preview.svg.as_bytes()), fingerprint(dxf.as_bytes())],
                expected,
                "{id} SVG and DXF must retain their baseline bytes"
            );
            assert_eq!(
                serde_json::to_vec(&preview.segments).unwrap(),
                serde_json::to_vec(&legacy_segments).unwrap(),
                "{id} preserves baseline segment extraction byte-for-byte"
            );
        }
    }

    fn assert_pick(preview: &DrawingPreview, point: [f64; 2], id: &str, kind: DrawingElementKind) {
        let hit = preview.pick_element(point, 1.0).unwrap_or_else(|| panic!("no hit at {point:?} for {id}"));
        assert_eq!((hit.id.as_str(), hit.kind), (id, kind), "at {point:?}");
        assert!(hit.distance_mm <= 1.0);
    }

    #[test]
    fn plans_pick_window_door_facade_and_interior_semantic_ids() {
        let session = session();
        let preview = plan_drawing(&session.doc, 0, View::latest(&session.doc)).unwrap();
        let facade = &session.doc.buildings[0].facades[0];
        for (point, id, kind) in [
            ([facade.bays[0], 140.0], "b.f1.L0.B0", DrawingElementKind::Opening),
            ([facade.bays[2], 200.0], "b.f1.L0.B2", DrawingElementKind::Opening),
            ([6000.0, 0.0], "b.f1", DrawingElementKind::FacadeWall),
            ([3000.0, 3925.0], "partition.L0", DrawingElementKind::InteriorWall),
            ([6000.0, 4000.0], "partition.L0.D0", DrawingElementKind::Opening),
        ] {
            assert_pick(&preview, point, id, kind);
        }
        let near = preview.pick_element([3000.0, 3920.0], 5.0).unwrap();
        assert_eq!(near.id, "partition.L0");
        assert_eq!(near.point, [3000.0, 3925.0]);
        assert_eq!(near.distance_mm, 5.0);
        assert_pick(&preview, [3000.0, 3920.0], "b.slab.L0", DrawingElementKind::Slab);
        assert_pick(&preview, [6000.0, 6500.0], "b.slab.L0", DrawingElementKind::Slab);
    }

    #[test]
    fn facades_and_sections_pick_visible_fill_and_host_ids() {
        let mut session = session();
        let phase = View::latest(&session.doc);
        let facade = drawing_view(&session.doc, "facade", phase).unwrap();
        let bays = &session.doc.buildings[0].facades[0].bays;
        assert_pick(&facade, [bays[0], 850.0], "b.f1.L0.B0", DrawingElementKind::Opening);
        assert_pick(&facade, [bays[2], 2300.0], "b.f1", DrawingElementKind::FacadeWall);
        assert_pick(&facade, [bays[2], 2240.0], "b.f1.L0.B2", DrawingElementKind::Opening);
        assert_pick(&facade, [0.0, 3000.0], "b.f1", DrawingElementKind::FacadeWall);
        assert!(!facade.element_segments.iter().any(|element| element.id.starts_with("partition")));
        let section = drawing_view(&session.doc, "section", phase).unwrap();
        assert_pick(&section, [0.0, 1500.0], "b.f4", DrawingElementKind::FacadeWall);
        assert_pick(&section, [2000.0, 1500.0], "partition.L0", DrawingElementKind::InteriorWall);
        assert_pick(&section, [6000.0, 2100.0], "partition.L0.D0", DrawingElementKind::Opening);
        session.execute("view.define", json!({"id":"cut","name":"Cut","kind":{"view":"section","from":[0,4000],"to":[12000,4000]}})).unwrap();
        let cut = drawing_view(&session.doc, "cut", phase).unwrap();
        assert_pick(&cut, [2000.0, 1500.0], "partition.L0", DrawingElementKind::InteriorWall);
        assert_pick(&cut, [6000.0, 2100.0], "partition.L0.D0", DrawingElementKind::Opening);
        assert_pick(&section, [6000.0, 4500.0], "b.f3", DrawingElementKind::FacadeWall);
    }

    #[test]
    fn unioned_wall_boundaries_preserve_owners_without_pickable_internal_seams() {
        let mut session = session();
        session
            .execute(
                "interior.add_walls",
                json!({"walls":[
                    {"id":"left","a":[2000,6000],"b":[6000,6000],"thickness":150,"levels":[0]},
                    {"id":"right","a":[6000,6000],"b":[10000,6000],"thickness":150,"levels":[0]}
                ]}),
            )
            .unwrap();
        let preview = plan_drawing(&session.doc, 0, View::latest(&session.doc)).unwrap();
        assert_pick(&preview, [4000.0, 5925.0], "left.L0", DrawingElementKind::InteriorWall);
        assert_pick(&preview, [8000.0, 5925.0], "right.L0", DrawingElementKind::InteriorWall);
        let mut boundary_only = preview.clone();
        boundary_only.element_regions.clear();
        assert!(boundary_only.pick_element([6000.0, 6000.0], 10.0).is_none(), "the union seam must not become a stroke");
        assert_pick(&preview, [6000.0, 6000.0], "left.L0", DrawingElementKind::InteriorWall);
    }

    #[test]
    fn picks_are_order_independent_nearest_first_with_explicit_ties() {
        let segment = [[0.0, 0.0], [100.0, 0.0]];
        let mut preview = DrawingPreview {
            svg: String::new(),
            transform: DrawingTransform { x0: 0.0, y1: 0.0, mm_per_px: 1.0 },
            segments: Vec::new(),
            element_regions: Vec::new(),
            element_segments: vec![
                DrawingElementSegment { id: "a-host".into(), kind: DrawingElementKind::FacadeWall, segment },
                DrawingElementSegment { id: "a-interior".into(), kind: DrawingElementKind::InteriorWall, segment },
                DrawingElementSegment { id: "z-opening".into(), kind: DrawingElementKind::Opening, segment },
                DrawingElementSegment { id: "a-opening".into(), kind: DrawingElementKind::Opening, segment },
            ],
        };
        let hit = preview.pick_element([50.0, 5.0], 5.0).unwrap();
        assert_eq!(hit.id, "a-opening");
        assert_eq!(hit.point, [50.0, 0.0]);
        assert_eq!(hit.distance_mm, 5.0);
        preview.element_segments.reverse();
        for element in &mut preview.element_segments {
            element.segment.reverse();
        }
        assert_eq!(preview.pick_element([50.0, 5.0], 5.0), Some(hit));
        preview.element_segments.retain(|element| element.kind != DrawingElementKind::Opening);
        assert_eq!(preview.pick_element([50.0, 0.0], 0.0).unwrap().id, "a-interior");
        preview.element_segments.push(DrawingElementSegment {
            id: "near-host".into(),
            kind: DrawingElementKind::FacadeWall,
            segment: [[0.0, 4.0], [100.0, 4.0]],
        });
        assert_eq!(preview.pick_element([50.0, 5.0], 5.0).unwrap().id, "near-host");
        assert_eq!(preview.pick_element([-3.0, 0.0], 3.0).unwrap().point, [0.0, 0.0]);
        assert!(preview.pick_element([-3.0, 0.0], 2.99).is_none());
    }

    #[test]
    fn picking_ignores_hostile_geometry_and_refuses_invalid_queries() {
        let session = session();
        let mut preview = plan_drawing(&session.doc, 0, View::latest(&session.doc)).unwrap();
        let point = [3000.0, 3925.0];
        let expected = preview.pick_element(point, 0.0);
        for segment in [[[f64::NAN, 0.0], point], [[f64::INFINITY, 0.0], point], [[f64::MAX, 0.0], point], [point, point]] {
            preview.element_segments.push(DrawingElementSegment { id: "hostile".into(), kind: DrawingElementKind::Opening, segment });
        }
        preview.element_segments.push(DrawingElementSegment {
            id: String::new(),
            kind: DrawingElementKind::Opening,
            segment: [[0.0, 3925.0], [4000.0, 3925.0]],
        });
        assert_eq!(preview.pick_element(point, 0.0), expected);
        for coordinate in [f64::NAN, f64::INFINITY, f64::NEG_INFINITY, f64::MAX, 1_000_001.0, -1_000_001.0] {
            assert!(preview.pick_element([coordinate, 0.0], 10.0).is_none());
            assert!(preview.pick_element([0.0, coordinate], 10.0).is_none());
        }
        for tolerance in [f64::NAN, f64::INFINITY, f64::NEG_INFINITY, f64::MAX, -0.001, 100_000.001] {
            assert!(preview.pick_element(point, tolerance).is_none());
        }
    }

    #[test]
    fn pick_command_is_additive_read_only_and_checks_hostile_params() {
        let mut session = session();
        let before = session.doc.clone();
        let history = (session.undo.len(), session.redo.len(), session.dirty);
        let spec = crate::commands().iter().find(|command| command.id == "view.pick").unwrap();
        assert!(!spec.mutates && !spec.journaled);
        let result = session.execute("view.pick", json!({"id":"plan","point":[3000,3920],"tolerance_mm":5})).unwrap();
        assert_eq!(result, json!({"hit":{"id":"partition.L0","kind":"interior_wall","point":[3000.0,3925.0],"distance_mm":5.0}}));
        assert_eq!(
            session.execute("view.pick", json!({"id":"plan","point":[6000,6500],"tolerance_mm":1})).unwrap(),
            json!({"hit":{"id":"b.slab.L0","kind":"slab","point":[6000.0,6500.0],"distance_mm":0.0}})
        );
        assert_eq!(session.execute("view.pick", json!({"id":"plan","point":[6000,15000],"tolerance_mm":1})).unwrap(), json!({"hit":null}));
        for params in [
            json!({"id":"plan","point":[1e300,0],"tolerance_mm":5}),
            json!({"id":"plan","point":[0,1e300],"tolerance_mm":5}),
            json!({"id":"plan","point":[0,0],"tolerance_mm":-1}),
            json!({"id":"plan","point":[0,0],"tolerance_mm":100001}),
            json!({"id":"plan","point":[0,0],"tolerance_mm":1e300}),
            json!({"id":"missing","point":[0,0],"tolerance_mm":1}),
            json!({"id":"plan","point":[0,0],"tolerance_mm":1,"phase":"missing"}),
            json!({"id":"plan","point":[0,0],"tolerance_mm":1,"filter":"missing"}),
            json!({"id":"plan","point":[0,0],"tolerance_mm":1,"extra":true}),
        ] {
            let error = session.execute("view.pick", params).unwrap_err();
            assert!(!matches!(error, crate::EngineError::Internal(_)));
        }
        assert_eq!(session.doc, before);
        assert_eq!((session.undo.len(), session.redo.len(), session.dirty), history);
    }

    #[test]
    fn picking_follows_each_drawings_phase_and_filter_visibility() {
        use trigon_model::build::PhaseFilter;
        let mut session = session();
        session
            .execute(
                "view.define",
                json!({"id":"front-section","name":"Front section","kind":{"view":"section","from":[0,-1000],"to":[12000,-1000]}}),
            )
            .unwrap();
        session.execute("phase.set", json!({"phases":[{"id":"existing","name":"Existing"},{"id":"proposal","name":"Proposal"}]})).unwrap();
        session.execute("facade.demolish", json!({"facade":"b.f1","levels":[0],"bays":[0],"what":"openings"})).unwrap();
        session.execute("facade.place", json!({"facade":"b.f1","type":"window.fixed","levels":[0],"bays":[1]})).unwrap();
        for id in ["plan", "facade", "front-section"] {
            for (phase, filter, old_visible, new_visible) in [
                (0, PhaseFilter::Complete, true, false),
                (1, PhaseFilter::Complete, false, true),
                (1, PhaseFilter::New, false, true),
                (1, PhaseFilter::All, id == "plan", true),
                (1, PhaseFilter::Demolition, id == "plan", false),
            ] {
                let preview = drawing_view(&session.doc, id, View { phase, filter }).unwrap();
                let has = |element_id: &str| preview.element_segments.iter().any(|element| element.id == element_id);
                assert_eq!(has("b.f1.L0.B0"), old_visible, "{id}, {phase}, {filter:?}");
                assert_eq!(has("b.f1.L0.B1"), new_visible, "{id}, {phase}, {filter:?}");
            }
            let old = session.execute("view.pick", json!({"id":id,"point":[session.doc.buildings[0].facades[0].bays[0],if id == "plan" {140.0} else {850.0}],"tolerance_mm":1,"phase":"existing"})).unwrap();
            assert_eq!(old["hit"]["id"], "b.f1.L0.B0");
        }
    }

    #[test]
    fn picking_metadata_is_deterministic_clipped_and_lies_on_stroked_paths() {
        let session = session();
        for id in ["plan", "facade", "section"] {
            let drawing = sheet(&session.doc, id, View::latest(&session.doc)).unwrap();
            let strokes: Vec<_> = drawing
                .scene
                .items
                .iter()
                .filter_map(|item| match item {
                    Item::Path(path) if path.stroke.is_some() => Some(path.segments()),
                    _ => None,
                })
                .flatten()
                .collect();
            let first = preview(drawing);
            for _ in 0..8 {
                let repeated = drawing_view(&session.doc, id, View::latest(&session.doc)).unwrap();
                assert_eq!(first.element_segments, repeated.element_segments, "{id}");
                assert!(first.element_regions == repeated.element_regions, "{id} semantic areas must be deterministic");
            }
            assert!(!first.element_segments.is_empty(), "{id}");
            for element in &first.element_segments {
                assert!(
                    strokes.iter().any(|stroke| crate::sheet::overlap(element.segment, *stroke, 1e-6).is_some_and(|overlap| {
                        overlap.iter().flatten().zip(element.segment.iter().flatten()).all(|(actual, expected)| (actual - expected).abs() < 1e-6)
                    })),
                    "{id}: {element:?}"
                );
            }
        }
        let mut cropped = sheet(&session.doc, "plan", View::latest(&session.doc)).unwrap();
        let top_left = cropped.vp.px([2000.0, 5000.0]);
        let bottom_right = cropped.vp.px([4000.0, 3000.0]);
        cropped.clip = [top_left[0], top_left[1], bottom_right[0] - top_left[0], bottom_right[1] - top_left[1]];
        let cropped = preview(cropped);
        assert_pick(&cropped, [3000.0, 3925.0], "partition.L0", DrawingElementKind::InteriorWall);
        assert!(cropped.pick_element([8000.0, 3925.0], 1.0).is_none());
        assert!(
            cropped
                .element_segments
                .iter()
                .flat_map(|element| element.segment)
                .all(|point| (1999.999..=4000.001).contains(&point[0]) && (2999.999..=5000.001).contains(&point[1]))
        );
    }

    #[test]
    fn annotations_levels_and_section_direction_do_not_invent_owners() {
        let mut session = session();
        let phase = View::latest(&session.doc);
        let before = drawing_view(&session.doc, "plan", phase).unwrap();
        session.execute("annotation.define", json!({"id":"note","view":"plan","kind":{"annotation":"leader","tip":[9000,6000],"elbow":[9500,6500],"at":[10000,6500],"text":"Only annotation"}})).unwrap();
        let after = drawing_view(&session.doc, "plan", phase).unwrap();
        assert_eq!(before.element_segments, after.element_segments);
        assert!(after.segments.len() > before.segments.len());
        assert_eq!(before.element_regions, after.element_regions, "annotations must not change semantic model areas");
        assert_pick(&after, [9000.0, 6000.0], "b.slab.L0", DrawingElementKind::Slab);
        let upper = plan_drawing(&session.doc, 1, phase).unwrap();
        assert!(upper.element_segments.iter().all(|element| element.kind == DrawingElementKind::FacadeWall));
        session
            .execute("view.define", json!({"id":"flipped","name":"Flipped","kind":{"view":"section","from":[0,2000],"to":[12000,2000],"flip":true}}))
            .unwrap();
        let flipped = drawing_view(&session.doc, "flipped", phase).unwrap();
        assert_pick(&flipped, [0.0, 1500.0], "b.f2", DrawingElementKind::FacadeWall);
        assert!(!flipped.element_segments.iter().any(|element| element.id.starts_with("partition")));
    }

    #[test]
    fn metadata_precision_removes_projection_noise_without_rewriting_paths() {
        let session = session();
        for offset in [0.0, 4e-12, -4e-12] {
            let mut drawing = sheet(&session.doc, "plan", View::latest(&session.doc)).unwrap();
            drawing.scene.items.clear();
            let segment = [[1000.0 + offset, 2000.0 + offset], [2000.0 + offset, 2000.0 + offset]];
            drawing.scene.line(Layer::Wall, segment[0], segment[1], Stroke::new(0.25));
            drawing.scene.attribute_last(&[DrawingElementSegment { id: "wall".into(), kind: DrawingElementKind::FacadeWall, segment }]);
            let Item::Path(path) = &drawing.scene.items[0] else { panic!("expected a stroked path") };
            assert_eq!(path.rings[0], segment);
            let preview = preview(drawing);
            assert_eq!(preview.segments, vec![segment]);
            assert_eq!(
                preview.element_segments,
                vec![DrawingElementSegment {
                    id: "wall".into(),
                    kind: DrawingElementKind::FacadeWall,
                    segment: [[1000.0, 2000.0], [2000.0, 2000.0]]
                }]
            );
            assert_eq!(
                preview.pick_element([1500.0, 2005.0], 5.0),
                Some(DrawingElementHit { id: "wall".into(), kind: DrawingElementKind::FacadeWall, point: [1500.0, 2000.0], distance_mm: 5.0 })
            );
        }
    }

    #[test]
    fn filled_picking_respects_holes_depth_and_existing_boundary_priority() {
        let square = |lo, hi| vec![[lo, lo], [hi, lo], [hi, hi], [lo, hi]];
        let region = |id: &str, kind, rings, depth_plane| DrawingElementRegion { id: id.into(), kind, rings, depth_plane };
        let mut preview = DrawingPreview {
            svg: String::new(),
            transform: DrawingTransform { x0: 0.0, y1: 0.0, mm_per_px: 1.0 },
            segments: vec![],
            element_segments: vec![],
            element_regions: vec![
                region("floor", DrawingElementKind::Slab, vec![square(0.0, 100.0), square(40.0, 60.0)], None),
                region("room", DrawingElementKind::Room, vec![square(10.0, 30.0)], None),
            ],
        };
        assert_pick(&preview, [20.0, 20.0], "room", DrawingElementKind::Room);
        assert_pick(&preview, [80.0, 80.0], "floor", DrawingElementKind::Slab);
        assert!(preview.pick_element([50.0, 50.0], 0.0).is_none(), "a floor opening has no floor pick");
        preview.element_regions.push(region("behind", DrawingElementKind::Opening, vec![square(12.0, 28.0)], Some([10.0, 0.0, 0.0])));
        assert_pick(&preview, [20.0, 20.0], "room", DrawingElementKind::Room);
        preview.element_regions.push(region("front", DrawingElementKind::Roof, vec![square(0.0, 100.0)], Some([-10.0, 0.0, 0.0])));
        assert_pick(&preview, [20.0, 20.0], "front", DrawingElementKind::Roof);
        preview.element_segments.push(DrawingElementSegment {
            id: "visible-line".into(),
            kind: DrawingElementKind::FacadeWall,
            segment: [[19.0, 0.0], [19.0, 100.0]],
        });
        assert_pick(&preview, [20.0, 20.0], "visible-line", DrawingElementKind::FacadeWall);
        preview.element_regions.reverse();
        assert_pick(&preview, [20.0, 20.0], "visible-line", DrawingElementKind::FacadeWall);
    }

    #[test]
    fn semantic_areas_follow_logical_scene_clip_before_sheet_clip() {
        let mut scene = Scene::new(100.0);
        scene.element_regions.push(DrawingElementRegion {
            id: "wall".into(),
            kind: DrawingElementKind::FacadeWall,
            rings: vec![vec![[-100.0, -100.0], [200.0, -100.0], [200.0, 200.0], [-100.0, 200.0]]],
            depth_plane: Some([12.0, 0.0, 0.0]),
        });
        scene.clip([0.0, 0.0, 100.0, f64::MAX]);
        let preview = DrawingPreview {
            svg: String::new(),
            transform: DrawingTransform { x0: 0.0, y1: 0.0, mm_per_px: 1.0 },
            segments: vec![],
            element_segments: vec![],
            element_regions: scene.element_regions,
        };
        assert_pick(&preview, [50.0, 50.0], "wall", DrawingElementKind::FacadeWall);
        for point in [[-20.0, 50.0], [120.0, 50.0], [50.0, -20.0]] {
            assert!(preview.pick_element(point, 0.0).is_none(), "clipped area at {point:?}");
        }
    }

    #[test]
    fn top_floor_plan_picks_its_floor_instead_of_the_attic_above_the_cut() {
        let mut s = session();
        s.execute("element.set", json!({"id":"b.roof","properties":{"roof_kind":"pitched"}})).unwrap();
        let preview = plan_drawing(&s.doc, 1, View::latest(&s.doc)).unwrap();
        assert_pick(&preview, [6000.0, 6500.0], "b.slab.L1", DrawingElementKind::Slab);
        assert!(!preview.element_regions.iter().any(|r| r.id == "b.attic"));
    }

    #[test]
    fn plan_areas_and_rail_strokes_resolve_to_actual_scene_elements() {
        let mut s = session();
        for (bay, ty) in [(0, "balcony.french"), (1, "balcony.cantilever")] {
            s.execute("facade.place", json!({"facade":"b.f1","type":ty,"levels":[0],"bays":[bay]})).unwrap();
        }
        let b = &mut s.doc.buildings[0];
        b.rooms =
            serde_json::from_value(json!([{"id":"living","name":"Living","levels":[0],"polygon":[[1000,1000],[5000,1000],[5000,3500],[1000,3500]]}]))
                .unwrap();
        b.stairs = serde_json::from_value(
            json!([{"id":"stair","levels":[0],"outline":[[7500,5000],[11000,5000],[11000,7000],[7500,7000]],"kind":"u_turn"}]),
        )
        .unwrap();
        let phase = View::latest(&s.doc);
        let preview = plan_drawing(&s.doc, 0, phase).unwrap();
        assert_pick(&preview, [3000.0, 2500.0], "living.space.L0", DrawingElementKind::Room);
        assert_pick(&preview, [9000.0, 5900.0], "stair.L0", DrawingElementKind::Stair);
        let built = trigon_model::build::build_view(&s.doc, phase).unwrap();
        let descriptors = crate::element_properties_from_built(&s.doc, &built, phase).unwrap();
        for id in ["b.f1.L0.B0.rail", "b.f1.L0.B1.rail"] {
            assert!(
                preview.element_segments.iter().any(|segment| segment.id == id && segment.kind == DrawingElementKind::Railing),
                "visible rail {id}"
            );
        }
        assert!(preview.element_regions.iter().any(|region| region.id == "b.f1.L0.B1.slab" && region.kind == DrawingElementKind::Balcony));
        for id in preview.element_segments.iter().map(|s| &s.id).chain(preview.element_regions.iter().map(|r| &r.id)) {
            assert!(descriptors.iter().any(|descriptor| descriptor["id"] == *id), "picked ID {id} must resolve to a real editable occurrence");
        }
        s.doc.buildings[0].add_basement(-2480.0, "Basement").unwrap();
        s.doc.buildings[0].cellar_stairs = serde_json::from_value(
            json!([{"facade":"b.f2","from":1000,"to":6000,"width":1500,"top":1200,"bottom":4000,"floor":-2480,"door_at":5000}]),
        )
        .unwrap();
        let preview = plan_drawing(&s.doc, 1, View::latest(&s.doc)).unwrap();
        assert!(preview.element_regions.iter().any(|region| region.id == "b.f2.cellar0" && region.kind == DrawingElementKind::Stair));
    }
}

#[cfg(test)]
mod snap_tests {
    use super::*;
    #[test]
    fn snapping_uses_real_segments_and_prioritizes_exact_geometry_on_ties() {
        let lines = [[[0.0, 0.0], [1000.0, 0.0]], [[500.0, -500.0], [500.0, 500.0]]];
        let options = SnapOptions { grid_mm: None, ..SnapOptions::default() };
        let hit = snap_drawing(&lines, [501.0, 1.0], 10.0, options).unwrap();
        assert_eq!(hit, DrawingSnap { point: [500.0, 0.0], kind: SnapKind::Intersection });
        let hit = snap_drawing(&lines, [2.0, 0.0], 10.0, options).unwrap();
        assert_eq!(hit.kind, SnapKind::Endpoint);
        let hit = snap_drawing(&lines[..1], [501.0, 1.0], 10.0, options).unwrap();
        assert_eq!(hit.kind, SnapKind::Midpoint);
        let hit = snap_drawing(&[], [1040.0, 1030.0], 51.0, SnapOptions::default()).unwrap();
        assert_eq!(hit, DrawingSnap { point: [1000.0, 1000.0], kind: SnapKind::Grid });
        assert!(snap_drawing(&lines, [5000.0, 5000.0], 10.0, options).is_none());
        let extensions = [[[0.0, 0.0], [100.0, 0.0]], [[200.0, -100.0], [200.0, 100.0]]];
        assert!(snap_drawing(&extensions, [200.0, 0.0], 1.0, SnapOptions { endpoints: false, midpoints: false, ..options }).is_none());
    }
    #[test]
    fn snapping_ignores_hostile_geometry_and_checks_query_numbers() {
        for v in [f64::NAN, f64::INFINITY, f64::MAX] {
            assert!(snap_drawing(&[], [v, 0.0], 10.0, SnapOptions::default()).is_none());
            assert!(snap_drawing(&[], [0.0, 0.0], v, SnapOptions::default()).is_none());
            assert!(snap_drawing(&[[[v, 0.0], [0.0, 0.0]]], [0.0, 0.0], 10.0, SnapOptions { grid_mm: None, ..SnapOptions::default() }).is_none());
        }
        assert!(snap_drawing(&[[[0.0, 0.0], [0.0, 0.0]]], [0.0, 0.0], 10.0, SnapOptions::default()).is_some());
    }
}
