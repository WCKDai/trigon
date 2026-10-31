//! A scene on the 1600 × 900 px presentation sheet: title box, the drawing through a viewport,
//! caption and footer. PNG and PDF are rendered from this SVG.

use std::fmt::Write as _;

use super::{Anchor, Fill, Halo, Item, LETTERING, Layer, MM_PER_PX, Path, Rgb, Scene, Stroke, Viewport, scale_text};
use crate::plan::escape;

/// What the sheet says around the drawing.
pub(crate) struct Frame<'a> {
    /// In the box at the top left.
    pub(crate) title: &'a str,
    /// Top right.
    pub(crate) project: &'a str,
    /// Under the drawing.
    pub(crate) caption: &'a str,
    /// Bottom right, after "Trigon · ".
    pub(crate) footer: &'a str,
    /// One line above the caption (for example the balcony list on a crowded plan).
    pub(crate) note: Option<String>,
}

const DEFS: &str = r##"<defs>
<pattern id="masonry" width="7" height="7" patternUnits="userSpaceOnUse"><rect width="7" height="7" fill="white"/><path d="M-2 2 L2 -2 M0 7 L7 0 M5 9 L9 5" fill="none" stroke="#777" stroke-width="0.65"/></pattern>
<pattern id="stair" width="8" height="8" patternUnits="userSpaceOnUse"><rect width="8" height="8" fill="#fafafa"/><circle cx="2" cy="2" r="0.6" fill="#aaa"/></pattern>
</defs>
"##;

pub(crate) fn hex(c: Rgb) -> String {
    format!("#{:02x}{:02x}{:02x}", c[0], c[1], c[2])
}

fn px(mm: f64) -> f64 {
    mm / MM_PER_PX
}

/// The sheet with `scene` drawn through `vp`, clipped to `clip` (sheet px `[x, y, w, h]`).
pub(crate) fn sheet(frame: &Frame<'_>, scene: &Scene, vp: &Viewport, clip: [f64; 4]) -> String {
    let mut out = String::from(
        r##"<svg xmlns="http://www.w3.org/2000/svg" width="1600" height="900" viewBox="0 0 1600 900">
<rect width="1600" height="900" fill="white"/>
"##,
    );
    out.push_str(DEFS);
    out.push_str("<g font-family=\"Liberation Sans, Helvetica, Arial, sans-serif\" fill=\"#242424\">\n");
    let _ = writeln!(
        out,
        r#"<defs><clipPath id="viewport"><rect x="{:.2}" y="{:.2}" width="{:.2}" height="{:.2}"/></clipPath></defs>"#,
        clip[0], clip[1], clip[2], clip[3]
    );
    let title_width = (frame.title.chars().count() as f64 * 23.0 + 44.0).max(160.0);
    let _ = writeln!(
        out,
        r##"<rect x="80" y="42" width="{title_width:.1}" height="70" fill="white" stroke="#999" stroke-width="0.8"/><text x="102" y="91" font-size="40">{}</text><text x="1520" y="72" text-anchor="end" font-size="18">{}</text>"##,
        escape(frame.title),
        escape(frame.project)
    );
    out.push_str("<g clip-path=\"url(#viewport)\">\n");
    items(&mut out, scene, vp);
    out.push_str("</g>\n");
    if let Some(note) = &frame.note {
        let _ = writeln!(out, r##"<text x="80" y="812" font-size="15">{}</text>"##, escape(note));
    }
    let _ = writeln!(
        out,
        r##"<text x="800" y="847" text-anchor="middle" font-size="28">{}</text><text x="80" y="886" font-size="12" fill="#777">Mål og tegninger er vejledende · {}</text><text x="1520" y="886" text-anchor="end" font-size="12" fill="#777">Trigon · {}</text></g></svg>"##,
        escape(frame.caption),
        scale_text(scene.scale),
        escape(frame.footer)
    );
    out
}

fn stroke_attrs(s: &Stroke, cap: &str) -> String {
    let dash = if s.dashed { format!(r#" stroke-dasharray="{:.2} {:.2}""#, px(2.0), px(1.0)) } else { String::new() };
    let cap = if s.dashed { "butt" } else { cap };
    format!(r#"stroke="{}" stroke-width="{:.3}" stroke-linecap="{cap}" stroke-linejoin="round"{dash}"#, hex(s.colour), px(s.pen))
}

fn path_data(d: &mut String, p: &Path, vp: &Viewport) {
    for ring in &p.rings {
        for (k, q) in ring.iter().enumerate() {
            let q = vp.px(*q);
            let _ = write!(d, "{}{:.2} {:.2}", if k == 0 { "M" } else { "L" }, q[0], q[1]);
        }
        if p.closed {
            d.push('Z');
        }
    }
}

/// The scene's items in order. Consecutive lines with the same stroke share one path.
pub(crate) fn items(out: &mut String, scene: &Scene, vp: &Viewport) {
    let mut k = 0;
    while k < scene.items.len() {
        match &scene.items[k] {
            Item::Path(p) if !p.closed => {
                let Some(stroke) = p.stroke else {
                    k += 1;
                    continue;
                };
                let mut d = String::new();
                while let Some(Item::Path(q)) = scene.items.get(k) {
                    if q.closed || q.stroke != Some(stroke) {
                        break;
                    }
                    path_data(&mut d, q, vp);
                    k += 1;
                }
                let _ = writeln!(out, r#"<path d="{d}" fill="none" {}/>"#, stroke_attrs(&stroke, "round"));
            }
            Item::Path(p) if p.layer == Layer::Tone && p.stroke.is_none() => {
                // Tones come oriented; consecutive ones of one colour are one nonzero path, so the
                // edges they share are never anti-aliased twice (no seams).
                let fill = p.fill;
                let mut d = String::new();
                while let Some(Item::Path(q)) = scene.items.get(k) {
                    if q.layer != Layer::Tone || q.stroke.is_some() || q.fill != fill || !q.closed {
                        break;
                    }
                    path_data(&mut d, q, vp);
                    k += 1;
                }
                let colour = match fill {
                    Some(Fill::Solid(c)) => hex(c),
                    _ => "none".into(),
                };
                let _ = writeln!(
                    out,
                    r#"<path d="{d}" fill="{colour}" fill-rule="nonzero" stroke="{colour}" stroke-width="0.3" stroke-linejoin="round"/>"#
                );
            }
            Item::Path(p) => {
                let mut d = String::new();
                path_data(&mut d, p, vp);
                let fill = match p.fill {
                    Some(Fill::Solid(c)) => hex(c),
                    Some(Fill::Masonry) => "url(#masonry)".into(),
                    Some(Fill::Dots) => "url(#stair)".into(),
                    None => "none".into(),
                };
                let stroke = match (p.stroke, p.fill) {
                    (Some(s), _) => stroke_attrs(&s, "round"),
                    // A thin stroke of the fill's own colour hides anti-aliasing seams between
                    // neighbouring faces.
                    (None, Some(Fill::Solid(c))) => format!(r#"stroke="{}" stroke-width="0.6" stroke-linejoin="round""#, hex(c)),
                    (None, _) => String::new(),
                };
                let _ = writeln!(out, r#"<path d="{d}" fill="{fill}" fill-rule="evenodd" {stroke}/>"#);
                k += 1;
            }
            Item::Text(t) => {
                let at = vp.px(t.at);
                let fs = px(t.size);
                let deg = -t.deg;
                let rotate = if deg.abs() > 1e-9 { format!(r#" transform="rotate({deg:.3} {:.2} {:.2})""#, at[0], at[1]) } else { String::new() };
                let anchor = match t.anchor {
                    Anchor::Start => "start",
                    Anchor::Middle => "middle",
                    Anchor::End => "end",
                };
                if t.halo == Halo::Box {
                    let w = 0.56 * fs * t.text.chars().count() as f64 + 6.0;
                    let x = match t.anchor {
                        Anchor::Start => at[0] - 3.0,
                        Anchor::Middle => at[0] - w / 2.0,
                        Anchor::End => at[0] - w + 3.0,
                    };
                    let y = if t.middle { at[1] - fs * 0.62 } else { at[1] - fs * 0.9 };
                    let _ = writeln!(
                        out,
                        r#"<rect x="{x:.2}" y="{y:.2}" width="{w:.2}" height="{:.2}" fill="white" fill-opacity="0.92"{rotate}/>"#,
                        fs * 1.24
                    );
                }
                let halo = if t.halo == Halo::Outline {
                    r#" paint-order="stroke fill" stroke="white" stroke-width="4" stroke-linejoin="round""#
                } else {
                    ""
                };
                let baseline = if t.middle { r#" dominant-baseline="middle""# } else { "" };
                let fill = if t.colour == LETTERING { String::new() } else { format!(r#" fill="{}""#, hex(t.colour)) };
                let _ = writeln!(
                    out,
                    r#"<text x="{:.2}" y="{:.2}" text-anchor="{anchor}"{baseline} font-size="{fs:.2}"{fill}{rotate}{halo}>{}</text>"#,
                    at[0],
                    at[1],
                    escape(&t.text)
                );
                k += 1;
            }
        }
    }
}
