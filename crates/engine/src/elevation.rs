//! Elevations: the model seen square-on to one facade, as a presentation sheet.
//!
//! Every visible mesh (walls, windows, doors, balconies, roof) is projected orthographically onto
//! the facade's plane and painted back to front, so returns, reveals and the roof behind the eaves
//! appear as in a drawn elevation. Balconies get their size ("Ca. depth x width") under the slab
//! and staircases their entrance above the roof.

use std::fmt::Write as _;
use std::path::PathBuf;

use serde_json::{Value, json};
use trigon_model::build::{FacadeFrame, Status, View, build_view, resolve_facade};
use trigon_model::mesh::{Material, Mesh};
use trigon_model::{ComponentKind, Document, ModelError};

use crate::Result;
use crate::plan::{Label, escape, write_output};

/// Drawing area on the 1600 × 900 sheet, px.
const LEFT: f64 = 80.0;
const WIDTH: f64 = 1440.0;
const TOP: f64 = 190.0;
const HEIGHT: f64 = 560.0;
/// Things further in front of the facade than this (other wings) are left out, mm.
const NEAR: f64 = 3000.0;
/// Deepest wing whose staircases are named on an elevation, mm.
const WING: f64 = 16000.0;

struct Tri {
    depth: f64,
    pts: [[f64; 2]; 3],
    fill: [u8; 3],
}

fn base(m: Material) -> [f64; 3] {
    match m {
        // Glass reads as dark panes in an elevation; the roof as a dark band.
        Material::Glass => [0.30, 0.34, 0.38],
        Material::Roof => [0.17, 0.17, 0.18],
        Material::Railing | Material::Steel => [0.16, 0.16, 0.17],
        other => {
            let c = other.rgba();
            [f64::from(c[0]), f64::from(c[1]), f64::from(c[2])]
        }
    }
}

/// Projects `mesh` into the facade's (u, v) plane with each triangle's depth (outward, mm).
fn project(out: &mut Vec<Tri>, mesh: &Mesh, m: Material, fr: &FacadeFrame, u_range: (f64, f64)) {
    let light = {
        let l = [fr.n[0] * 0.6 - fr.t[0] * 0.35, fr.n[1] * 0.6 - fr.t[1] * 0.35, 0.55];
        let n = (l[0] * l[0] + l[1] * l[1] + l[2] * l[2]).sqrt();
        [l[0] / n, l[1] / n, l[2] / n]
    };
    let colour = base(m);
    for t in &mesh.triangles {
        let Some(p) = t.iter().map(|&i| mesh.positions.get(i as usize).copied()).collect::<Option<Vec<_>>>() else { continue };
        let uvw: Vec<[f64; 3]> = p
            .iter()
            .map(|q| {
                let (dx, dy) = (q[0] - fr.origin[0], q[1] - fr.origin[1]);
                [dx * fr.t[0] + dy * fr.t[1], q[2], dx * fr.n[0] + dy * fr.n[1]]
            })
            .collect();
        if uvw.iter().all(|q| q[0] < u_range.0)
            || uvw.iter().all(|q| q[0] > u_range.1)
            || uvw.iter().map(|q| q[2]).fold(f64::INFINITY, f64::min) > NEAR
        {
            continue;
        }
        let area = (uvw[1][0] - uvw[0][0]) * (uvw[2][1] - uvw[0][1]) - (uvw[2][0] - uvw[0][0]) * (uvw[1][1] - uvw[0][1]);
        if area.abs() < 1.0 {
            continue; // edge-on to the viewer
        }
        let (a, b) = ([p[1][0] - p[0][0], p[1][1] - p[0][1], p[1][2] - p[0][2]], [p[2][0] - p[0][0], p[2][1] - p[0][1], p[2][2] - p[0][2]]);
        let n = [a[1] * b[2] - a[2] * b[1], a[2] * b[0] - a[0] * b[2], a[0] * b[1] - a[1] * b[0]];
        let len = (n[0] * n[0] + n[1] * n[1] + n[2] * n[2]).sqrt().max(1e-9);
        let shade = 0.72 + 0.28 * ((n[0] * light[0] + n[1] * light[1] + n[2] * light[2]) / len).abs();
        out.push(Tri {
            depth: (uvw[0][2] + uvw[1][2] + uvw[2][2]) / 3.0,
            pts: [[uvw[0][0], uvw[0][1]], [uvw[1][0], uvw[1][1]], [uvw[2][0], uvw[2][1]]],
            fill: colour.map(|c| (c * shade * 255.0).round().clamp(0.0, 255.0) as u8),
        });
    }
}

/// Where an elevation looks from: square-on to one facade, or to a line along a stepped front
/// (from `a` to `b`, the outside on the right as on a counter-clockwise footprint).
pub(crate) enum Side<'a> {
    Facade(&'a str),
    Along([[f64; 2]; 2]),
}

/// An elevation sheet seen from `side`, between `from` and `to` mm along it (default: all of it).
#[allow(clippy::too_many_arguments)]
pub(crate) fn sheet(
    doc: &Document,
    side: Side<'_>,
    view: View,
    range: (Option<f64>, Option<f64>),
    title: &str,
    caption: &str,
    path: PathBuf,
) -> Result<Value> {
    if title.len() > 200 || caption.len() > 500 {
        return Err(ModelError::Invalid("the title is limited to 200 bytes and the caption to 500".into()).into());
    }
    let ext = path.extension().and_then(|s| s.to_str()).unwrap_or_default();
    if !matches!(ext, "png" | "svg" | "pdf") {
        return Err(ModelError::Invalid("use .pdf, .png or .svg for an elevation".into()).into());
    }
    let built = build_view(doc, view)?;
    let (b, fr) = match side {
        Side::Facade(facade) => built
            .iter()
            .find_map(|b| b.walls.iter().find(|w| w.id == facade).map(|w| (b, w.frame)))
            .ok_or_else(|| ModelError::Invalid(format!("facade {facade} doesn't exist")))?,
        Side::Along([a, e]) => {
            if [a, e].iter().flatten().any(|v| !v.is_finite() || v.abs() > 1e8) || (e[0] - a[0]).hypot(e[1] - a[1]) < 1000.0 {
                return Err(ModelError::Invalid("`along` needs two finite site points at least 1 m apart".into()).into());
            }
            let b = built.first().ok_or_else(|| ModelError::Invalid("there is no building to draw".into()))?;
            (b, FacadeFrame::new(a, e).ok_or_else(|| ModelError::Invalid("`along` needs two distinct points".into()))?)
        }
    };
    let (from, to) = (range.0.unwrap_or(-1500.0), range.1.unwrap_or(fr.length + 1500.0));
    for (name, v) in [("from_mm", from), ("to_mm", to)] {
        trigon_model::checked(name, v, -100_000.0, 1_000_000.0)?;
    }
    if to - from < 1000.0 {
        return Err(ModelError::Invalid("the elevation needs at least 1 m between from_mm and to_mm".into()).into());
    }
    let mut tris = Vec::new();
    for bb in &built {
        for w in &bb.walls {
            project(&mut tris, &w.mesh, Material::Wall, &fr, (from, to));
            for e in w.openings.iter().flat_map(|o| std::iter::once(&o.fill).chain(&o.attached)).chain(&w.attachments) {
                if e.status == Status::Demolished {
                    continue;
                }
                for (m, mesh) in &e.parts {
                    project(&mut tris, mesh, *m, &fr, (from, to));
                }
            }
        }
        if let Some(r) = &bb.roof {
            project(&mut tris, &r.cover, Material::Roof, &fr, (from, to));
            project(&mut tris, &r.gables, Material::Wall, &fr, (from, to));
        }
    }
    tris.sort_by(|a, b| a.depth.total_cmp(&b.depth));
    let top = tris.iter().flat_map(|t| t.pts.iter().filter(|p| (from..=to).contains(&p[0])).map(|p| p[1])).fold(1000.0_f64, f64::max);
    let mm = ((to - from) / WIDTH).max(top / HEIGHT);
    let x0 = LEFT + (WIDTH - (to - from) / mm) / 2.0;
    let ground = TOP + (HEIGHT + top / mm) / 2.0;
    let px = |u: f64, v: f64| [x0 + (u - from) / mm, ground - v / mm];

    let floor = doc.buildings.first().map(|b| b.name.as_str()).unwrap_or_default();
    let mut out = String::from(
        r##"<svg xmlns="http://www.w3.org/2000/svg" width="1600" height="900" viewBox="0 0 1600 900">
<rect width="1600" height="900" fill="white"/>
<g font-family="Liberation Sans, Helvetica, Arial, sans-serif" fill="#242424">
"##,
    );
    let (cx0, cx1) = (px(from, 0.0)[0], px(to, 0.0)[0]);
    let _ = writeln!(out, r#"<defs><clipPath id="model"><rect x="{cx0:.2}" y="0" width="{:.2}" height="{ground:.2}"/></clipPath></defs>"#, cx1 - cx0);
    let title_width = (title.chars().count() as f64 * 23.0 + 44.0).max(160.0);
    let _ = writeln!(
        out,
        r##"<rect x="80" y="42" width="{title_width:.1}" height="70" fill="white" stroke="#999" stroke-width="0.8"/><text x="102" y="91" font-size="40">{}</text><text x="1520" y="72" text-anchor="end" font-size="18">{}</text>"##,
        escape(title),
        escape(floor)
    );
    out.push_str("<g clip-path=\"url(#model)\" stroke-linejoin=\"round\">\n");
    // Consecutive triangles of one colour share a path; the thin same-colour stroke hides the
    // seams anti-aliasing would leave between them.
    let mut k = 0;
    while k < tris.len() {
        let fill = tris[k].fill;
        let mut d = String::new();
        while k < tris.len() && tris[k].fill == fill {
            for (i, p) in tris[k].pts.iter().enumerate() {
                let q = px(p[0], p[1]);
                let _ = write!(d, "{}{:.2} {:.2}", if i == 0 { "M" } else { "L" }, q[0], q[1]);
            }
            d.push('Z');
            k += 1;
        }
        let c = format!("#{:02x}{:02x}{:02x}", fill[0], fill[1], fill[2]);
        let _ = writeln!(out, r#"<path d="{d}" fill="{c}" stroke="{c}" stroke-width="0.35"/>"#);
    }
    out.push_str("</g>\n");
    let _ = writeln!(out, r##"<line x1="60" y1="{ground:.2}" x2="1540" y2="{ground:.2}" stroke="#222" stroke-width="2"/>"##);

    // Balcony sizes under each slab, with ticks at its ends; a label that would run into its
    // neighbour's drops one line.
    let mut labels: Vec<Label> = Vec::new();
    let mut ticks = Vec::new();
    // Balconies on every facade turned towards the viewer, placed along this view.
    let depth_of = |p: [f64; 2]| (p[0] - fr.origin[0]) * fr.n[0] + (p[1] - fr.origin[1]) * fr.n[1];
    let facing: Vec<(&str, FacadeFrame)> = b
        .walls
        .iter()
        .filter(|w| {
            let mid = w.frame.point(w.frame.length / 2.0, 0.0, 0.0);
            w.frame.n[0] * fr.n[0] + w.frame.n[1] * fr.n[1] > 0.7 && (-4000.0..=NEAR).contains(&depth_of([mid[0], mid[1]]))
        })
        .map(|w| (w.id.as_str(), w.frame))
        .collect();
    for (mb, (id, wf)) in doc.buildings.iter().filter(|x| x.id == b.building_id).flat_map(|mb| facing.iter().map(move |f| (mb, f))) {
        let Ok(f) = mb.facade(id) else { continue };
        for r in resolve_facade(doc, mb, f)? {
            let Some(depth) = r.ty.kind.depth() else { continue };
            if !matches!(view.status(r.lifetime), Some(Status::Existing | Status::New)) {
                continue;
            }
            let width = match r.ty.kind {
                ComponentKind::Balcony { slab_width, .. } => slab_width,
                _ => r.ty.width,
            };
            let p = wf.point((r.rect.u0 + r.rect.u1) / 2.0, 0.0, 0.0);
            let c = (p[0] - fr.origin[0]) * fr.t[0] + (p[1] - fr.origin[1]) * fr.t[1];
            if !(from..=to).contains(&c) {
                continue;
            }
            let at = px(c, r.rect.v0 - 330.0);
            let fs = 13.0;
            labels.push(Label {
                at,
                alt: Some([at[0], at[1] + fs * 1.6]),
                deg: 0.0,
                fs,
                text: format!("Ca. {depth:.0} × {width:.0}"),
                balcony: true,
            });
            ticks.push((px(c - width / 2.0, r.rect.v0 - 330.0), px(c + width / 2.0, r.rect.v0 - 330.0)));
        }
    }
    // Entrances above the roof, named as seen from this side: "TV 7 TH" from the street the stair
    // is entered from, "TH 7 TV" from behind.
    for e in &doc.buildings.iter().find(|x| x.id == b.building_id).map(|x| x.entrances.clone()).unwrap_or_default() {
        let Some(st) = e.stair.as_ref().and_then(|id| doc.buildings.iter().flat_map(|x| &x.stairs).find(|s| &s.id == id)) else { continue };
        let Ok(sf) = trigon_model::stair::frame(st) else { continue };
        let core = sf.at(sf.length / 2.0, sf.width / 2.0);
        let u = (core[0] - fr.origin[0]) * fr.t[0] + (core[1] - fr.origin[1]) * fr.t[1];
        // Only staircases in the wing behind this facade, not other wings seen past its ends.
        let behind = (core[0] - fr.origin[0]) * fr.n[0] + (core[1] - fr.origin[1]) * fr.n[1];
        if !(from + 500.0..=to - 500.0).contains(&u) || !(-WING..=0.0).contains(&behind) {
            continue;
        }
        // Seen from the street the stair is entered from: the facade it meets nearest the address
        // point, as on the plans.
        let Some(footprint) = doc.buildings.iter().find(|x| x.id == b.building_id).map(|x| &x.footprint) else { continue };
        let hit = |d: [f64; 2]| {
            crate::plan::facade_crossing(footprint, core, d).map(|t| (core[0] + d[0] * t - e.point[0]).hypot(core[1] + d[1] * t - e.point[1]))
        };
        let front = match (hit(fr.n), hit([-fr.n[0], -fr.n[1]])) {
            (Some(ahead), Some(behind)) => ahead <= behind,
            (ahead, _) => ahead.is_some(),
        };
        let text = if front { format!("TV {} TH", e.number) } else { format!("TH {} TV", e.number) };
        labels.push(Label { at: px(u, top + 900.0), alt: None, deg: 0.0, fs: 18.0, text, balcony: false });
    }
    let mut placed: Vec<Label> = Vec::new();
    for l in labels {
        let fits = |l: &Label| !placed.iter().any(|q| q.overlaps(l));
        if fits(&l) {
            placed.push(l);
        } else if let Some(at) = l.alt.filter(|at| fits(&Label { at: *at, alt: None, text: l.text.clone(), ..l })) {
            placed.push(Label { at, alt: None, ..l });
        }
    }
    for (a, b) in &ticks {
        for p in [a, b] {
            let _ = writeln!(
                out,
                r##"<line x1="{:.2}" y1="{:.2}" x2="{:.2}" y2="{:.2}" stroke="#222" stroke-width="0.9"/>"##,
                p[0] - 4.0,
                p[1] + 4.0,
                p[0] + 4.0,
                p[1] - 4.0
            );
        }
    }
    for l in &placed {
        let w = 0.56 * l.fs * l.text.chars().count() as f64 + 6.0;
        if l.balcony {
            let _ = writeln!(
                out,
                r##"<rect x="{:.2}" y="{:.2}" width="{w:.2}" height="{:.2}" fill="white" fill-opacity="0.92"/>"##,
                l.at[0] - w / 2.0,
                l.at[1] - l.fs * 0.62,
                l.fs * 1.24
            );
        }
        let _ = writeln!(
            out,
            r#"<text x="{:.2}" y="{:.2}" text-anchor="middle" dominant-baseline="middle" font-size="{}">{}</text>"#,
            l.at[0],
            l.at[1],
            l.fs,
            escape(&l.text)
        );
    }
    let _ = writeln!(
        out,
        r##"<text x="800" y="847" text-anchor="middle" font-size="28">{}</text><text x="80" y="886" font-size="12" fill="#777">Mål og tegninger er vejledende</text><text x="1520" y="886" text-anchor="end" font-size="12" fill="#777">Trigon · {}</text></g></svg>"##,
        escape(caption),
        escape(title)
    );
    let bytes = match ext {
        "pdf" => crate::plan_output::pdf(std::slice::from_ref(&out), [338.666_666_7, 190.5])?,
        "png" => crate::plan_output::png(&out)?,
        _ => out.into_bytes(),
    };
    write_output(&path, &bytes)?;
    let mut result =
        json!({"written": path, "triangles": tris.len(), "mm_per_px": mm, "balcony_labels": placed.iter().filter(|l| l.balcony).count()});
    if ext == "png" {
        result["image_path"] = json!(path);
    }
    Ok(result)
}
