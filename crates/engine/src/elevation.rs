//! Elevations: the model seen square-on to one facade, as a line drawing.
//!
//! Walls, windows, doors, balconies and the roof are projected orthographically onto the facade's
//! plane with hidden lines removed (see [`crate::sheet::project`]): feature edges only, the outline
//! heavier, things further back lighter and thinner, light tonal fills with glass a little darker,
//! the ground line heaviest. Balconies get their size ("Ca. depth x width") under the slab and
//! staircases their entrance above the roof.

use std::path::PathBuf;

use serde_json::{Value, json};
use trigon_model::build::{FacadeFrame, Status, View, build_view, resolve_facade};
use trigon_model::{ComponentKind, Document, ModelError};

use crate::Result;
use crate::plan::Label;
use crate::sheet::project::{Look, Near, project};
use crate::sheet::{Halo, Layer, MM_PER_PX, Scene, Sheet, Stroke, Text, Viewport, scale_text, sheet_scale, style};

/// Drawing area on the 1600 × 900 sheet, px.
const LEFT: f64 = 80.0;
const WIDTH: f64 = 1440.0;
const TOP: f64 = 190.0;
const HEIGHT: f64 = 560.0;
/// Things further in front of the facade than this (other wings) are cut away, mm.
const NEAR: f64 = 3000.0;
/// Nothing further back than this is drawn, mm.
const FAR: f64 = 300_000.0;
/// Deepest wing whose staircases are named on an elevation, mm.
const WING: f64 = 16000.0;

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
    let ext = path.extension().and_then(|s| s.to_str()).unwrap_or_default();
    if !matches!(ext, "png" | "svg" | "pdf" | "dxf") {
        return Err(ModelError::Invalid("use .pdf, .png, .svg or .dxf for an elevation".into()).into());
    }
    scene(doc, side, view, range, title, caption, false)?.write(path)
}

pub(crate) fn scene(
    doc: &Document,
    side: Side<'_>,
    view: View,
    range: (Option<f64>, Option<f64>),
    title: &str,
    caption: &str,
    hide_balcony_dimensions: bool,
) -> Result<Sheet> {
    if title.len() > 200 || caption.len() > 500 {
        return Err(ModelError::Invalid("the title is limited to 200 bytes and the caption to 500".into()).into());
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
    // Looking at the facade from outside: x along it, depth into the building.
    let look = Look { origin: fr.origin, t: fr.t, d: [-fr.n[0], -fr.n[1]] };
    let parts = style::parts(&built, false);
    let sources = style::seen(&parts);
    let projection = project(&sources, &look, (-NEAR, FAR), Near::Cut);
    let top = projection.edges.iter().flat_map(|e| [e.a, e.b]).filter(|p| (from..=to).contains(&p[0])).map(|p| p[1]).fold(1000.0_f64, f64::max);
    let fit = ((to - from) / WIDTH).max(top / HEIGHT) / MM_PER_PX;
    let mm = sheet_scale(fit) * MM_PER_PX;
    let x0 = LEFT + (WIDTH - (to - from) / mm) / 2.0;
    let ground = TOP + (HEIGHT + top / mm) / 2.0;
    let vp = Viewport { x0: from - x0 * mm, y1: ground * mm, mm };
    let px = |u: f64, v: f64| vp.px([u, v]);

    let mut scene = Scene::new(vp.scale());
    style::fills(&mut scene, &projection);
    style::edges(&mut scene, &projection);
    // Below the terrain and beyond the ends is not part of the elevation.
    scene.clip([from, 0.0, to, f64::MAX]);
    let (g0, g1) = (vp.at([60.0, ground])[0], vp.at([1540.0, ground])[0]);
    scene.line(Layer::Ground, [g0, 0.0], [g1, 0.0], Stroke::new(0.7));

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
        if hide_balcony_dimensions {
            continue;
        }
        let Ok(f) = mb.facade(id) else { continue };
        for r in resolve_facade(doc, mb, f)? {
            if matches!(r.ty.kind, ComponentKind::GuardRail { .. }) {
                continue;
            }
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
        let Some(source) = doc.buildings.iter().find(|x| x.id == b.building_id) else { continue };
        let Some(st) = e
            .stair_at(source.ground_level)
            .and_then(|id| source.stairs.iter().find(|s| s.id == id))
            .or_else(|| e.stair.as_ref().and_then(|id| source.stairs.iter().find(|s| &s.id == id)))
            .or_else(|| e.stair_by_level.iter().find_map(|(level, id)| source.stairs.iter().find(|s| &s.id == id && s.levels.contains(level))))
        else {
            continue;
        };
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
    // Ticks at the balconies' ends, 45° like dimension ticks.
    for (a, b) in &ticks {
        for p in [a, b] {
            let (p0, p1) = (vp.at([p[0] - 4.0, p[1] + 4.0]), vp.at([p[0] + 4.0, p[1] - 4.0]));
            scene.line(Layer::Annotation, p0, p1, Stroke::new(0.18));
        }
    }
    for l in &placed {
        let mut t = Text::new(Layer::Annotation, vp.at(l.at), l.text.clone(), l.fs * MM_PER_PX);
        if l.balcony {
            t.halo = Halo::Box;
        }
        scene.text(t);
    }
    let floor = doc.buildings.first().map(|b| b.name.as_str()).unwrap_or_default();
    let metrics = json!({"triangles": projection.triangles, "lines": projection.edges.len(), "mm_per_px": mm, "scale": scale_text(scene.scale), "balcony_labels": placed.iter().filter(|l| l.balcony).count()});
    Ok(Sheet {
        scene,
        vp,
        clip: [0.0, 120.0, 1600.0, 700.0],
        title: title.into(),
        project: floor.into(),
        caption: caption.into(),
        note: None,
        metrics,
    })
}
