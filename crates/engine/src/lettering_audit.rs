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
    let [x, y, w, h] = sheet.clip;
    let (lo, hi) = (sheet.vp.at([x, y + h]), sheet.vp.at([x + w, y]));
    let mut crop = [lo[0] / scale, lo[1] / scale, hi[0] / scale, hi[1] / scale];
    let template = s.doc.views.iter().find(|v| v.id == p.id).and_then(|v| v.template.as_ref());
    if let Some(bounds) = template.and_then(|t| t.crop) {
        crop = [crop[0].max(bounds[0] / scale), crop[1].max(bounds[1] / scale), crop[2].min(bounds[2] / scale), crop[3].min(bounds[3] / scale)];
    }
    let mut labels = Vec::new();
    let mut polygons: Vec<(String, Vec<[f64; 2]>)> = Vec::new();
    let mut findings = Vec::new();
    let mut count = 0usize;
    let mut finding = |value: Value| {
        count += 1;
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
        for (other, poly) in &polygons {
            if overlaps(&visible, poly, p.clearance_mm_paper) {
                finding(json!({"kind":"potential_text_overlap","keys":[other,&key],"clearance_mm_paper":p.clearance_mm_paper}));
            }
        }
        polygons.push((key, visible));
    }
    Ok(json!({"view":p.id,"scale":scale,"status":if count==0 {"no_findings_in_scope"} else {"needs_review"},
        "labels":labels,"findings":findings,"total_findings":count,"truncated":count>p.max_results,
        "clearance_mm_paper":p.clearance_mm_paper,"visible_bounds_mm":crop.map(|v|v*scale),
        "scope":"bundled SVG glyph ink and exported halos; generated labels present in the view plus saved annotations",
        "limitations":["Box intersections are potential collisions, not a pixel-level glyph collision test.","DXF consumers may substitute fonts. Suppressed generated labels and generic text/model-line clashes are not audited."]}))
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
