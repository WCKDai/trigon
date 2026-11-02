//! Display-only detail reduction. Export scenes and all semantic picking geometry stay exact.
use super::{Fill, Item, Layer, Path, Scene, Sheet, svg};
use std::collections::BTreeSet;

pub(crate) fn svg_at(sheet: &Sheet, detail: u8) -> String {
    if detail == 0 {
        return sheet.svg();
    }
    let representative_zoom = if detail == 1 { 0.6 } else { 0.3 };
    let cell = if detail == 1 { 1.5 } else { 2.5 };
    let mut scene = Scene::new(sheet.scene.scale);
    let mut occupied = BTreeSet::new();
    for item in &sheet.scene.items {
        match item {
            Item::Text(t) => {
                if t.layer == Layer::Annotation || t.size / super::MM_PER_PX * representative_zoom >= 4.0 {
                    scene.text(t.clone());
                }
            }
            Item::Path(p) => {
                let mut stroke = p.stroke;
                let protected = matches!(p.layer, Layer::WallCut | Layer::Wall | Layer::Roof | Layer::Floor | Layer::Ground | Layer::Annotation);
                if let Some(s) = &mut stroke {
                    s.pen *= if protected { 0.9 } else { 0.7 };
                }
                let fill = match p.fill {
                    Some(Fill::Masonry) => Some(Fill::Solid([225; 3])),
                    other => other,
                };
                if !p.closed && !protected && p.stroke.is_some_and(|s| s.pen <= 0.25) {
                    for [a, b] in p.segments() {
                        let length = (b[0] - a[0]).hypot(b[1] - a[1]) / sheet.vp.mm * representative_zoom;
                        if length < 0.75 {
                            continue;
                        }
                        let angle = (b[1] - a[1]).atan2(b[0] - a[0]).rem_euclid(std::f64::consts::PI);
                        // Sheet-anchored cells never change with viewport pan. Different directions
                        // are independent, retaining crossings while decimating dense parallel detail.
                        let key = (
                            p.layer,
                            (angle * 16.0 / std::f64::consts::PI).round() as i64,
                            ((a[0] + b[0]) * 0.5 / sheet.vp.mm * representative_zoom / cell).floor() as i64,
                            ((a[1] + b[1]) * 0.5 / sheet.vp.mm * representative_zoom / cell).floor() as i64,
                        );
                        if occupied.insert(key) {
                            scene.items.push(Item::Path(Path {
                                layer: p.layer,
                                rings: vec![vec![a, b]],
                                closed: false,
                                fill: None,
                                stroke,
                                element_segments: Vec::new(),
                            }));
                        }
                    }
                } else {
                    scene.items.push(Item::Path(Path {
                        layer: p.layer,
                        rings: p.rings.clone(),
                        closed: p.closed,
                        fill,
                        stroke,
                        element_segments: Vec::new(),
                    }));
                }
            }
        }
    }
    svg::sheet(
        &svg::Frame { title: &sheet.title, project: &sheet.project, caption: &sheet.caption, footer: &sheet.title, note: sheet.note.clone() },
        &scene,
        &sheet.vp,
        sheet.clip,
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::sheet::{Stroke, Viewport};
    #[test]
    fn display_reduces_dense_subpixel_detail_but_keeps_cut_silhouettes_and_manual_marks() {
        let mut scene = Scene::new(100.0);
        for i in 0..100 {
            scene.line(Layer::Balcony, [i as f64 * 2.0, 0.0], [i as f64 * 2.0, 1000.0], Stroke::new(0.13));
        }
        scene.line(Layer::WallCut, [0.0, 0.0], [1000.0, 0.0], Stroke::new(0.5));
        scene.line(Layer::Annotation, [0.0, 200.0], [1000.0, 200.0], Stroke { colour: [184, 65, 44], ..Stroke::new(0.5) });
        let sheet = Sheet {
            scene,
            vp: Viewport { x0: 0.0, y1: 1000.0, mm: 10.0 },
            clip: [80.0, 140.0, 1440.0, 650.0],
            title: "Fixture".into(),
            project: String::new(),
            caption: String::new(),
            note: None,
            metrics: serde_json::json!({}),
        };
        let before = sheet.scene.clone();
        let exact = sheet.svg();
        let overview = svg_at(&sheet, 2);
        assert!(overview.matches('M').count() < exact.matches('M').count() / 2);
        assert!(overview.contains("#b8412c"));
        assert!(overview.contains("stroke-width=\"2.126\""), "cut silhouette remains heavier than detail");
        assert_eq!(svg_at(&sheet, 0), exact, "zoom-in restores the exact exported detail");
        assert_eq!(sheet.scene, before, "display LOD cannot modify export strokes or picking metadata");
        assert_eq!(svg_at(&sheet, 2), overview, "display reduction is deterministic and independent of pan");
    }
}
