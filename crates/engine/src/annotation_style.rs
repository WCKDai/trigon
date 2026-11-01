//! Saved annotation geometry and portable lettering styles. Inputs are validated by the model.

use trigon_model::annotation::{Annotation, AnnotationKind, TextStyle};

use crate::sheet::{Anchor, Fill, Halo, Layer, Scene, Stroke, Text, level_text};

fn lettering(scene: &mut Scene, at: [f64; 2], value: &str, size: f64, rotation: f64, style: TextStyle) {
    let mut t = Text::new(Layer::Annotation, at, value, size);
    t.anchor = Anchor::Start;
    t.middle = false;
    t.deg = rotation;
    t.halo = Halo::Outline;
    t.style = style;
    scene.text(t);
}

pub(crate) fn draw(scene: &mut Scene, a: &Annotation) {
    let s = scene.scale;
    let pen = Stroke::new(0.18);
    match &a.kind {
        AnnotationKind::Line { from, to, stroke } => {
            scene.line(Layer::Annotation, *from, *to, Stroke { pen: stroke.width_mm, colour: stroke.colour, pattern: stroke.pattern, dashed: false });
        }
        AnnotationKind::Text { at, text, rotation_deg } => lettering(scene, *at, text, a.size_mm, *rotation_deg, a.style),
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
            t.style = a.style;
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
            lettering(scene, [at[0] + s, at[1] + s], text, a.size_mm, 0.0, a.style);
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
            lettering(scene, [at[0] + 2.2 * s, at[1] + 1.0 * s], &text, a.size_mm, 0.0, a.style);
        }
        AnnotationKind::Region { points, fill, border } => {
            scene.polygon(Layer::Annotation, vec![points.clone()], Some(Fill::Solid(*fill)), border.then_some(pen));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::sheet::{Item, Viewport};
    use serde_json::json;
    use trigon_model::annotation::FontFamily;

    #[test]
    fn annotation_styles_reach_svg_text_without_changing_measured_dimension_content() {
        let style = TextStyle { family: FontFamily::Serif, bold: true, italic: true };
        for kind in [
            json!({"annotation":"text","at":[1000,2000],"text":"Køkken","rotation_deg":20}),
            json!({"annotation":"dimension","from":[0,0],"to":[3000,4000],"offset_mm":100}),
            json!({"annotation":"leader","tip":[0,0],"elbow":[2000,1000],"at":[3000,1000],"text":"Facade"}),
            json!({"annotation":"level","at":[1000,3000],"label":"Etage"}),
        ] {
            let mut annotation: Annotation = serde_json::from_value(json!({"id":"a","view":"v","kind":kind})).unwrap();
            annotation.style = style;
            annotation.validate().unwrap();
            let before = annotation.clone();
            let mut scene = Scene::new(100.0);
            draw(&mut scene, &annotation);
            let texts: Vec<_> = scene.items.iter().filter_map(|item| if let Item::Text(text) = item { Some(text) } else { None }).collect();
            assert_eq!(texts.len(), 1);
            assert_eq!(texts[0].style, style);
            if matches!(&annotation.kind, AnnotationKind::Dimension { .. }) {
                assert_eq!(texts[0].text, "5000");
            }
            let mut svg = String::new();
            crate::sheet::svg::items(&mut svg, &scene, &Viewport { x0: 0.0, y1: 10000.0, mm: 10.0 });
            assert!(svg.contains("font-family=\"Liberation Serif\" font-weight=\"bold\" font-style=\"italic\""));
            assert_eq!(annotation, before);
        }
    }

    #[test]
    fn filled_regions_keep_exact_boundaries_colour_and_optional_outline() {
        for border in [false, true] {
            let annotation:Annotation=serde_json::from_value(json!({"id":"region","view":"v","kind":{"annotation":"region","points":[[0,0],[3000,0],[3000,1000],[0,1000]],"fill":[220,230,240],"border":border}})).unwrap();
            annotation.validate().unwrap();
            let mut scene = Scene::new(100.0);
            draw(&mut scene, &annotation);
            let [Item::Path(path)] = scene.items.as_slice() else { panic!("one filled region path expected") };
            assert!(path.closed);
            assert_eq!(path.rings, vec![vec![[0.0, 0.0], [3000.0, 0.0], [3000.0, 1000.0], [0.0, 1000.0]]]);
            assert_eq!(path.fill, Some(Fill::Solid([220, 230, 240])));
            assert_eq!(path.stroke.is_some(), border);
            assert!(scene.element_regions.is_empty(), "annotation fill is not a model element");
        }
    }
}
