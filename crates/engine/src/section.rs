//! Sections: the model cut by a vertical plane along a line, looking to one side.
//!
//! The plane cuts every mesh; the cut faces are assembled into closed loops and filled as poche
//! (masonry hatched, concrete grey, partitions light grey, steel black), outlined with the heaviest
//! pen. What lies beyond the cut, up to `depth_mm` away, is drawn as in an elevation: hidden lines
//! removed, lighter and thinner further back. Storey levels are marked at the side ("+0,00" at the
//! ground storey's floor, metres with a decimal comma) with the storey names, and the ground line
//! runs outside the building.

use std::path::PathBuf;

use serde_json::{Value, json};
use trigon_model::build::{View, build_view};
use trigon_model::mesh::Material;
use trigon_model::{Document, ModelError};

use crate::Result;
use crate::plan::write_output;
use crate::sheet::cut::{self, Shape};
use crate::sheet::project::{Class, Look, Near, project};
use crate::sheet::{Anchor, Fill, Layer, PAGE_MM, Rgb, Scene, Stroke, Text, Viewport, dxf, level_text, scale_text, style, svg};

/// Drawing area on the 1600 × 900 sheet, px: room at the sides for the level marks.
const AREA: [f64; 4] = [230.0, 150.0, 1140.0, 640.0];

/// How a cut solid is filled.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
enum Poche {
    Concrete,
    Masonry,
    Partition,
    Frame,
    Timber,
    Steel,
    Glass,
    Roof,
}

impl Poche {
    fn of(class: Class, m: Material) -> Poche {
        match m {
            Material::Glass => Poche::Glass,
            Material::Steel | Material::Railing => Poche::Steel,
            Material::Deck => Poche::Timber,
            Material::Frame | Material::Door | Material::Sill => Poche::Frame,
            Material::Roof => Poche::Roof,
            Material::Wall if class == Class::Partition => Poche::Partition,
            Material::Wall => Poche::Masonry,
            _ => Poche::Concrete,
        }
    }

    /// Layer, fill and outline pen (paper mm).
    fn look(self) -> (Layer, Option<Fill>, f64) {
        const WHITE: Rgb = [0xff, 0xff, 0xff];
        match self {
            Poche::Masonry => (Layer::WallCut, Some(Fill::Masonry), 0.5),
            Poche::Concrete => (Layer::WallCut, Some(Fill::Solid([0xa8, 0xa8, 0xa8])), 0.5),
            Poche::Partition => (Layer::WallCut, Some(Fill::Solid([0xdc, 0xdc, 0xdc])), 0.35),
            Poche::Roof => (Layer::WallCut, Some(Fill::Solid([0x70, 0x70, 0x70])), 0.5),
            Poche::Frame => (Layer::Glazing, Some(Fill::Solid(WHITE)), 0.25),
            Poche::Timber => (Layer::Balcony, Some(Fill::Solid(WHITE)), 0.25),
            Poche::Steel => (Layer::Balcony, Some(Fill::Solid([0x1a, 0x1a, 0x1a])), 0.25),
            Poche::Glass => (Layer::Glazing, None, 0.18),
        }
    }
}

fn bbox(shapes: &[Shape]) -> Option<[f64; 4]> {
    let mut b: Option<[f64; 4]> = None;
    for p in shapes.iter().flatten().flatten() {
        let r = b.get_or_insert([p[0], p[1], p[0], p[1]]);
        *r = [r[0].min(p[0]), r[1].min(p[1]), r[2].max(p[0]), r[3].max(p[1])];
    }
    b
}

fn overlaps(a: [f64; 4], b: [f64; 4]) -> bool {
    a[0] <= b[2] && b[0] <= a[2] && a[1] <= b[3] && b[1] <= a[3]
}

/// Where along `from → to` (mm from `from`) the line is outside every footprint.
fn outside(doc: &Document, from: [f64; 2], to: [f64; 2]) -> Vec<(f64, f64)> {
    let d = [to[0] - from[0], to[1] - from[1]];
    let len = d[0].hypot(d[1]);
    let mut ts = vec![0.0, 1.0];
    for b in &doc.buildings {
        let n = b.footprint.len();
        for i in 0..n {
            let (p, q) = (b.footprint[i], b.footprint[(i + 1) % n]);
            let e = [q[0] - p[0], q[1] - p[1]];
            let den = d[0] * e[1] - d[1] * e[0];
            if den.abs() < 1e-12 {
                continue;
            }
            let w = [p[0] - from[0], p[1] - from[1]];
            let (t, s) = ((w[0] * e[1] - w[1] * e[0]) / den, (w[0] * d[1] - w[1] * d[0]) / den);
            if (0.0..=1.0).contains(&t) && (0.0..=1.0).contains(&s) {
                ts.push(t);
            }
        }
    }
    ts.sort_by(f64::total_cmp);
    ts.dedup_by(|a, b| (*a - *b).abs() < 1e-12);
    ts.windows(2)
        .filter(|w| {
            let m = (w[0] + w[1]) / 2.0;
            let p = [from[0] + d[0] * m, from[1] + d[1] * m];
            !doc.buildings.iter().any(|b| crate::drawings::inside(&b.footprint, p))
        })
        .map(|w| (w[0] * len, w[1] * len))
        .collect()
}

/// A section sheet along `line` (site mm), looking left of `from → to` (`flip`: right),
/// showing up to `depth` mm beyond the cut.
#[allow(clippy::too_many_arguments)]
pub(crate) fn sheet(
    doc: &Document,
    line: [[f64; 2]; 2],
    flip: bool,
    depth: f64,
    view: View,
    title: &str,
    caption: &str,
    path: PathBuf,
) -> Result<Value> {
    if title.len() > 200 || caption.len() > 500 {
        return Err(ModelError::Invalid("the title is limited to 200 bytes and the caption to 500".into()).into());
    }
    let ext = path.extension().and_then(|s| s.to_str()).unwrap_or_default();
    if !matches!(ext, "png" | "svg" | "pdf" | "dxf") {
        return Err(ModelError::Invalid("use .pdf, .png, .svg or .dxf for a section".into()).into());
    }
    if line.iter().flatten().any(|v| !v.is_finite() || v.abs() > 1e8) {
        return Err(ModelError::Invalid("the cut line needs two finite site points".into()).into());
    }
    let depth = trigon_model::checked("depth_mm", depth, 100.0, 500_000.0)?;
    let [from, to] = if flip { [line[1], line[0]] } else { line };
    let length = (to[0] - from[0]).hypot(to[1] - from[1]);
    if length < 1000.0 || length > 1_000_000.0 {
        return Err(ModelError::Invalid("the cut line must be between 1 m and 1 km long".into()).into());
    }
    let t = [(to[0] - from[0]) / length, (to[1] - from[1]) / length];
    // Looking to the left of from → to, so `from` is at the left of the drawing.
    let look = Look { origin: from, t, d: [-t[1], t[0]] };
    let built = build_view(doc, view)?;
    if built.is_empty() {
        return Err(ModelError::Invalid("there is no building to cut".into()).into());
    }
    let parts = style::parts(&built, true);

    // The cut, per welded group: closed loops become poche, open chains heavy lines.
    let mut groups: Vec<usize> = parts.iter().map(|p| p.source.group).collect();
    groups.sort_unstable();
    groups.dedup();
    let mut poche: Vec<(Poche, Vec<Shape>)> = Vec::new();
    let mut open: Vec<(Poche, Vec<[f64; 2]>)> = Vec::new();
    for g in groups {
        let mut segs = Vec::new();
        let mut kind = None;
        for p in parts.iter().filter(|p| p.source.group == g) {
            cut::segments(p.source.mesh, &|q| look.map(q), &mut segs);
            kind.get_or_insert(Poche::of(p.source.class, p.material));
        }
        let Some(kind) = kind.filter(|_| !segs.is_empty()) else { continue };
        let chains = cut::chain(&segs, 0.5, 5.0);
        let shapes = cut::even_odd(&chains.closed);
        match poche.iter_mut().find(|(k, _)| *k == kind) {
            Some((_, all)) => all.extend(shapes),
            None => poche.push((kind, shapes)),
        }
        open.extend(chains.open.into_iter().map(|c| (kind, c)));
    }
    poche.sort_by_key(|(k, _)| *k);
    let poche: Vec<(Poche, Vec<Shape>)> = poche.into_iter().map(|(k, s)| (k, cut::union(&s))).collect();
    let all_poche: Vec<Shape> = poche.iter().flat_map(|(_, s)| s.iter().cloned()).collect();

    // Beyond the cut.
    let sources: Vec<_> = parts.iter().map(|p| p.source.clone()).collect();
    let projection = project(&sources, &look, (0.0, depth), Near::Caps(&all_poche));

    // Fit: the cut line across, everything cut or seen up and down, the terrain included.
    let mut ys = vec![0.0_f64];
    ys.extend(all_poche.iter().flatten().flatten().map(|p| p[1]));
    ys.extend(projection.edges.iter().filter(|e| (0.0..=length).contains(&e.a[0])).flat_map(|e| [e.a[1], e.b[1]]));
    let (y0, y1) = ys.iter().fold((f64::INFINITY, f64::NEG_INFINITY), |(lo, hi), y| (lo.min(*y), hi.max(*y)));
    let vp = Viewport::fit([0.0, y0, length, y1.max(y0 + 1000.0)], AREA);
    let scale = vp.scale();
    let mut scene = Scene::new(scale);

    // Tones beyond, minus what the cut covers.
    let cover = bbox(&all_poche);
    let mut beyond = Scene::new(scale);
    style::fills(&mut beyond, &projection);
    for item in beyond.items {
        let crate::sheet::Item::Path(mut p) = item else { continue };
        let face = cut::even_odd(&p.rings);
        let b = bbox(&face);
        let kept = match (b, cover) {
            (Some(b), Some(c)) if overlaps(b, c) => cut::difference(&face, &all_poche),
            _ => face,
        };
        p.rings = kept.into_iter().flatten().collect();
        if !p.rings.is_empty() {
            scene.items.push(crate::sheet::Item::Path(p));
        }
    }
    style::edges(&mut scene, &projection);
    for (kind, shapes) in &poche {
        let (layer, fill, pen) = kind.look();
        for shape in shapes {
            scene.polygon(layer, shape.clone(), fill, Some(Stroke::new(pen)));
        }
    }
    for (kind, chain) in &open {
        let (layer, _, pen) = kind.look();
        scene.polyline(layer, chain.clone(), Stroke::new(pen));
    }
    scene.clip([0.0, f64::MIN, length, f64::MAX]);

    // The ground outside the building.
    for (a, b) in outside(doc, from, to) {
        scene.line(Layer::Ground, [a, 0.0], [b, 0.0], Stroke::new(0.7));
    }
    // Levels: a dashed line across, a mark and the level at the right, the name at the left.
    let s = scale;
    let mut levels: Vec<(f64, String)> = Vec::new();
    if let (Some(mb), Some(b)) = (doc.buildings.first(), built.first()) {
        let zero = b.storeys.get(mb.ground_level).map_or(0.0, |st| st.elevation);
        levels.extend(b.storeys.iter().map(|st| (st.elevation, st.name.clone())));
        levels.push((0.0, "Terræn".into()));
        if let Some(top) = b.storeys.last() {
            levels.push((top.elevation + top.height, "Tagfod".into()));
        }
        if let Some(r) = &b.roof {
            levels.push((r.ridge, "Kip".into()));
        }
        levels.sort_by(|a, b| a.0.total_cmp(&b.0));
        levels.dedup_by(|a, b| (a.0 - b.0).abs() < 1.0);
        for (k, (z, name)) in levels.iter().enumerate() {
            let (x0, x1) = (-3.0 * s, length + 3.0 * s);
            let dashed = Stroke { dashed: true, ..Stroke::new(0.18) };
            scene.line(Layer::Level, [x0, *z], [x1, *z], dashed);
            // Mark and lettering above the line, or below it when the next level up is too close.
            let crowded = levels.get(k + 1).is_some_and(|(next, _)| next - z < 4.0 * s);
            let (up, y) = if crowded { (-1.0, z - 3.2 * s) } else { (1.0, z + 0.8 * s) };
            let m = length + 7.0 * s;
            scene.polygon(
                Layer::Level,
                vec![vec![[m, *z], [m + 1.3 * s, *z + 2.2 * s * up], [m - 1.3 * s, *z + 2.2 * s * up]]],
                Some(Fill::Solid([0x1a, 0x1a, 0x1a])),
                None,
            );
            let mut level = Text::new(Layer::Level, [m + 2.2 * s, y], level_text(z - zero), 2.6);
            level.anchor = Anchor::Start;
            level.middle = false;
            scene.text(level);
            let mut label = Text::new(Layer::Level, [-5.0 * s, y], name.clone(), 2.6);
            label.anchor = Anchor::End;
            label.middle = false;
            scene.text(label);
        }
    }

    let project_name = doc.buildings.first().map(|b| b.name.as_str()).unwrap_or_default();
    let cut_area: f64 = all_poche.iter().flatten().map(|r| cut::area2(r)).sum::<f64>() / 2.0;
    let mut result = json!({
        "written": path,
        "scale": scale_text(scale),
        "mm_per_px": vp.mm,
        "looking": if flip { "right of from → to" } else { "left of from → to" },
        "cut_regions": poche.iter().map(|(_, s)| s.len()).sum::<usize>(),
        "cut_area_m2": (cut_area / 1e4).round() / 100.0,
        "open_cut_lines": open.len(),
        "lines": projection.edges.len(),
        "levels": levels.len(),
    });
    let bytes = match ext {
        "dxf" => {
            let text = dxf::write(&scene, &dxf::TitleBlock { title, project: project_name, caption, note: None });
            result["dxf"] = dxf::summary(&text);
            dxf::bytes(&text)
        }
        _ => {
            let page = svg::sheet(
                &svg::Frame { title, project: project_name, caption, footer: title, note: None },
                &scene,
                &vp,
                [0.0, 120.0, 1600.0, 700.0],
            );
            match ext {
                "pdf" => crate::plan_output::pdf(std::slice::from_ref(&page), PAGE_MM)?,
                "png" => crate::plan_output::png(&page)?,
                _ => page.into_bytes(),
            }
        }
    };
    write_output(&path, &bytes)?;
    if ext == "png" {
        result["image_path"] = json!(path);
    }
    Ok(result)
}
