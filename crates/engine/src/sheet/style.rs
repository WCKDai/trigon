//! How model geometry looks on a sheet: which meshes are drawn as what, their pens, tones and
//! layers, and the depth cue (further back is lighter and thinner).

use std::collections::HashMap;

use trigon_model::build::{Built, ElementClass, Status};
use trigon_model::mesh::{Material, Mesh};

use super::project::{Class, Projection, Source};
use super::{Element, Fill, INK, Layer, Rgb, Scene, Stroke, thinner};
use crate::views::DrawingElementKind;

/// What an element's part is drawn as.
pub(crate) fn class_of(element: ElementClass, m: Material) -> Class {
    match (element, m) {
        (_, Material::Glass) => Class::Glass,
        (ElementClass::Railing, Material::Deck) | (ElementClass::BalconySlab, _) => Class::Balcony,
        (ElementClass::Railing, _) => Class::Railing,
        (_, Material::Door) => Class::Door,
        (_, Material::Wall) => Class::Wall,
        _ => Class::Frame,
    }
}

/// Which drawings a mesh takes part in.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Role {
    /// Seen and cut.
    Both,
    /// Only seen (the whole facade wall as one surface).
    Seen,
    /// Only cut (the facade wall's closed piece on each storey).
    Cut,
}

/// A mesh to draw with the material it is made of (sections hatch by material).
pub(crate) struct Part<'a> {
    pub(crate) source: Source<'a>,
    pub(crate) material: Material,
    pub(crate) role: Role,
}

fn part(mesh: &Mesh, class: Class, group: usize, material: Material, role: Role, element: Option<Element>) -> Part<'_> {
    Part { source: Source { mesh, class, group, element }, material, role }
}

/// The meshes of the buildings as a view shows them. A facade wall is seen as one surface and
/// cut storey by storey (each piece is closed); walls and gables of a building form one group, so
/// joints between facades in line don't show. `inside` adds slabs, interior walls with their doors
/// and stairs (for sections).
pub(crate) fn parts(built: &[Built], inside: bool) -> Vec<Part<'_>> {
    let mut out = Vec::new();
    let mut group = 0;
    let mut next = || {
        group += 1;
        group
    };
    for b in built {
        let walls = next();
        for w in b.exterior_walls() {
            let element = Element::new(&w.id, DrawingElementKind::FacadeWall);
            out.push(part(&w.mesh, Class::Wall, walls, Material::Wall, Role::Seen, Some(element.clone())));
            for seg in &w.segments {
                out.push(part(&seg.mesh, Class::Wall, next(), Material::Wall, Role::Cut, Some(element.clone())));
            }
            out.push(part(&w.plinth, Class::Plinth, next(), Material::Plinth, Role::Both, Some(element)));
            for e in w.openings.iter().flat_map(|o| std::iter::once(&o.fill).chain(&o.attached)).chain(&w.attachments) {
                if e.status == Status::Demolished {
                    continue;
                }
                for (m, mesh) in &e.parts {
                    let kind = match e.class {
                        ElementClass::Window | ElementClass::Door => DrawingElementKind::Opening,
                        ElementClass::Railing => DrawingElementKind::Railing,
                        ElementClass::BalconySlab => DrawingElementKind::Balcony,
                        ElementClass::Stair => DrawingElementKind::Stair,
                    };
                    let element = Some(Element::new(&e.id, kind));
                    out.push(part(mesh, class_of(e.class, *m), next(), *m, Role::Both, element));
                }
            }
        }
        if let Some(r) = &b.roof {
            let element = Some(Element::new(&r.id, DrawingElementKind::Roof));
            out.push(part(&r.cover, Class::Roof, next(), Material::Roof, Role::Both, element.clone()));
            if r.gable_parts.is_empty() {
                out.push(part(&r.gables, Class::Wall, walls, Material::Wall, Role::Both, element.clone()));
            } else {
                for (material, mesh) in &r.gable_parts {
                    out.push(part(mesh, Class::Wall, walls, *material, Role::Both, element.clone()));
                }
            }
            for (material, mesh) in &r.covering {
                out.push(part(mesh, Class::Roof, next(), *material, Role::Both, element.clone()));
            }
            out.push(part(&r.gutters, Class::Roof, next(), Material::Steel, Role::Both, element));
            for d in &r.dormers {
                out.push(part(
                    &d.roof,
                    Class::Roof,
                    next(),
                    Material::Steel,
                    Role::Both,
                    Some(Element::new(&format!("{}.roof", d.id), DrawingElementKind::Roof)),
                ));
                out.push(part(
                    &d.cheeks,
                    Class::Wall,
                    next(),
                    Material::Steel,
                    Role::Both,
                    Some(Element::new(&format!("{}.cheeks", d.id), DrawingElementKind::FacadeWall)),
                ));
            }
        }
        if inside {
            for s in &b.slabs {
                let kind = if s.kind == trigon_model::SlabKind::Roof { DrawingElementKind::Roof } else { DrawingElementKind::Slab };
                out.push(part(&s.mesh, Class::Slab, next(), Material::Slab, Role::Both, Some(Element::new(&s.id, kind))));
            }
            for iw in &b.interior {
                let class = if iw.load_bearing { Class::Wall } else { Class::Partition };
                out.push(part(&iw.mesh, class, next(), Material::Plaster, Role::Both, Some(Element::new(&iw.id, DrawingElementKind::InteriorWall))));
                for d in iw.doors.iter().filter(|d| d.status != Status::Demolished) {
                    for (m, mesh) in &d.fill.parts {
                        out.push(part(
                            mesh,
                            class_of(d.fill.class, *m),
                            next(),
                            *m,
                            Role::Both,
                            Some(Element::new(&d.fill.id, DrawingElementKind::Opening)),
                        ));
                    }
                }
            }
            for st in &b.stairs {
                let element = Some(Element::new(&st.id, DrawingElementKind::Stair));
                out.push(part(&st.mesh, Class::Stair, next(), Material::Slab, Role::Both, element.clone()));
                out.push(part(&st.railing, Class::Railing, next(), Material::Railing, Role::Both, element));
            }
        }
    }
    out.retain(|p| !p.source.mesh.is_empty());
    out
}

/// The sources to project (everything seen).
pub(crate) fn seen<'a>(parts: &[Part<'a>]) -> Vec<Source<'a>> {
    parts.iter().filter(|p| p.role != Role::Cut).map(|p| p.source.clone()).collect()
}

pub(crate) fn layer(class: Class) -> Layer {
    match class {
        Class::Wall | Class::Partition | Class::Plinth => Layer::Wall,
        Class::Roof => Layer::Roof,
        Class::Slab => Layer::Floor,
        Class::Frame | Class::Glass => Layer::Glazing,
        Class::Door => Layer::Door,
        Class::Balcony | Class::Railing => Layer::Balcony,
        Class::Stair => Layer::Stair,
    }
}

/// The pen for a visible edge near the front, paper mm.
fn pen(class: Class) -> f64 {
    match class {
        Class::Wall | Class::Roof | Class::Plinth => 0.35,
        Class::Slab | Class::Stair | Class::Partition | Class::Frame | Class::Door | Class::Balcony => 0.25,
        Class::Glass => 0.18,
        Class::Railing => 0.13,
    }
}

/// The light tone a face of this class is filled with, if any.
fn tone(class: Class) -> Option<Rgb> {
    match class {
        Class::Wall => Some([0xf4, 0xef, 0xea]),
        Class::Roof => Some([0xdf, 0xdf, 0xdf]),
        Class::Plinth => Some([0xe4, 0xe2, 0xde]),
        Class::Slab | Class::Stair | Class::Balcony => Some([0xee, 0xee, 0xee]),
        Class::Partition => Some([0xf4, 0xf4, 0xf4]),
        Class::Frame => Some([0xff, 0xff, 0xff]),
        Class::Glass => Some([0xc9, 0xd2, 0xd9]),
        Class::Door => Some([0xe6, 0xdf, 0xd8]),
        Class::Railing => None,
    }
}

/// How far back something is: 0 within 8 m of the drawing plane (a wing and its roof), 1 within
/// 20 m, 2 beyond.
fn band(depth: f64) -> usize {
    if depth < 8000.0 {
        0
    } else if depth < 20_000.0 {
        1
    } else {
        2
    }
}

fn towards_white(c: Rgb, k: f64) -> Rgb {
    c.map(|v| (f64::from(v) + (255.0 - f64::from(v)) * k).round().clamp(0.0, 255.0) as u8)
}

/// The projection's faces (back to front) as tonal fills, each face's rings oriented (outlines
/// counter-clockwise, holes clockwise) so neighbouring faces of one tone can be filled as one.
pub(crate) fn fills(scene: &mut Scene, p: &Projection) {
    scene.element_regions.extend(p.element_regions.iter().cloned());
    for f in &p.faces {
        if let Some(element) = &f.element {
            scene.element_regions.push(crate::views::DrawingElementRegion {
                id: element.id.clone(),
                kind: element.kind,
                rings: f.rings.clone(),
                depth_plane: Some(f.depth_plane),
            });
        }
        let Some(c) = tone(f.class) else { continue };
        let c = towards_white(c, [0.0, 0.45, 0.7][band(f.depth)]);
        let rings: Vec<Vec<[f64; 2]>> = super::cut::even_odd(&f.rings).into_iter().flatten().collect();
        scene.polygon(Layer::Tone, rings, Some(Fill::Solid(c)), None);
    }
}

/// The projection's visible edges: heavier on the outline, lighter and thinner further back. Where
/// pieces of edges lie on one line they are merged, the heaviest winning, so nothing is drawn twice.
pub(crate) fn edges(scene: &mut Scene, p: &Projection) {
    let mut styled: Vec<([f64; 2], [f64; 2], Layer, Stroke)> = Vec::with_capacity(p.edges.len());
    for e in &p.edges {
        let b = band(e.depth);
        let base = if e.outline { pen(e.class).max(0.5) } else { pen(e.class) };
        // Balusters are many fine lines: grey, so a railing reads as one, not as a black block.
        let b = if e.class == Class::Railing { (b + 1).min(2) } else { b };
        let colour = [INK, [0x60, 0x60, 0x60], [0x96, 0x96, 0x96]][b];
        let pen = if e.class == Class::Railing { base } else { thinner(base, b) };
        styled.push((e.a, e.b, layer(e.class), Stroke { pen, colour, dashed: false, pattern: Default::default() }));
    }
    let elements: Vec<_> = p.edges.iter().flat_map(|edge| edge.element_segments.iter().cloned()).collect();
    for (a, b, layer, stroke) in merge(styled) {
        scene.line(layer, a, b, stroke);
        scene.attribute_last(&elements);
    }
}

/// Heavier first, then darker.
fn weight(l: &Layer, s: &Stroke) -> (i64, i64, Layer) {
    ((s.pen * 1000.0).round() as i64, -i64::from(s.colour.iter().map(|v| u32::from(*v)).sum::<u32>()), *l)
}

/// Collinear overlapping pieces become one, styled as the heaviest piece covering each part.
fn merge(lines: Vec<([f64; 2], [f64; 2], Layer, Stroke)>) -> Vec<([f64; 2], [f64; 2], Layer, Stroke)> {
    // Lines keyed by direction (to 1e-5 rad) and offset from the origin (to 0.05 mm).
    let mut by_line: HashMap<(i64, i64), Vec<(f64, f64, usize)>> = HashMap::new();
    let mut frames: HashMap<(i64, i64), ([f64; 2], [f64; 2], f64)> = HashMap::new();
    for (k, (a, b, _, _)) in lines.iter().enumerate() {
        let mut theta = (b[1] - a[1]).atan2(b[0] - a[0]);
        if theta < 0.0 {
            theta += std::f64::consts::PI;
        }
        if theta >= std::f64::consts::PI - 1e-9 {
            theta = 0.0;
        }
        let q = (theta / 1e-5).round() as i64;
        let th = q as f64 * 1e-5;
        let rho = -th.sin() * a[0] + th.cos() * a[1];
        let key = (q, (rho / 0.05).round() as i64);
        // Measured along the first piece found on the line, so its ends come back exactly.
        let (u, _, _) = *frames.entry(key).or_insert_with(|| {
            let (u, n) = ([theta.cos(), theta.sin()], [-theta.sin(), theta.cos()]);
            (u, n, n[0] * a[0] + n[1] * a[1])
        });
        let (s0, s1) = (u[0] * a[0] + u[1] * a[1], u[0] * b[0] + u[1] * b[1]);
        by_line.entry(key).or_default().push((s0.min(s1), s0.max(s1), k));
    }
    let mut keys: Vec<(i64, i64)> = by_line.keys().copied().collect();
    keys.sort_unstable();
    let mut out = Vec::new();
    for key in keys {
        let (Some(pieces), Some((u, n, rho))) = (by_line.get(&key), frames.get(&key)) else { continue };
        let point = |s: f64| [n[0] * rho + u[0] * s, n[1] * rho + u[1] * s];
        let mut cuts: Vec<f64> = pieces.iter().flat_map(|p| [p.0, p.1]).collect();
        cuts.sort_by(f64::total_cmp);
        cuts.dedup_by(|a, b| (*a - *b).abs() < 1e-6);
        let mut run: Option<(f64, f64, usize)> = None;
        for w in cuts.windows(2) {
            let m = (w[0] + w[1]) / 2.0;
            let best = pieces
                .iter()
                .filter(|p| p.0 <= m && m <= p.1)
                .map(|p| p.2)
                .max_by_key(|&k| lines.get(k).map(|(_, _, l, s)| weight(l, s)).unwrap_or((0, 0, Layer::Wall)));
            match (best, run) {
                (Some(k), Some((s, _, rk))) if lines.get(k).map(|l| (l.2, l.3)) == lines.get(rk).map(|l| (l.2, l.3)) => run = Some((s, w[1], rk)),
                (best, prev) => {
                    if let Some((s, e, rk)) = prev
                        && let Some(l) = lines.get(rk)
                    {
                        out.push((point(s), point(e), l.2, l.3));
                    }
                    run = best.map(|k| (w[0], w[1], k));
                }
            }
        }
        if let Some((s, e, rk)) = run
            && let Some(l) = lines.get(rk)
        {
            out.push((point(s), point(e), l.2, l.3));
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn collinear_pieces_merge_and_the_heaviest_wins() {
        let thin = Stroke { pen: 0.18, ..Stroke::new(0.18) };
        let heavy = Stroke::new(0.5);
        let out = merge(vec![
            ([0.0, 0.0], [1000.0, 0.0], Layer::Glazing, thin),
            ([500.0, 0.0], [1500.0, 0.0], Layer::Wall, heavy),
            ([1500.0, 0.0], [2000.0, 0.0], Layer::Wall, heavy),
            // Drawn the other way along the same line, and exactly on top of another piece.
            ([3000.0, 0.0], [2500.0, 0.0], Layer::Glazing, thin),
            ([2500.0, 0.0], [3000.0, 0.0], Layer::Glazing, thin),
            // Another line.
            ([0.0, 100.0], [1000.0, 100.0], Layer::Wall, thin),
        ]);
        let on_axis: Vec<_> = out.iter().filter(|l| l.0[1].abs() < 1e-6).collect();
        assert_eq!(on_axis.len(), 3, "{on_axis:?}");
        assert!(on_axis.iter().any(|l| (l.0[0] - 500.0).abs() < 1e-6 && (l.1[0] - 2000.0).abs() < 1e-6 && l.3.pen == 0.5));
        assert!(on_axis.iter().any(|l| l.0[0].abs() < 1e-6 && (l.1[0] - 500.0).abs() < 1e-6 && l.3.pen == 0.18));
        assert_eq!(out.len(), 4);
    }
}
