//! Floor plans: a storey cut 1 m above its floor, drawn from the model (walls, windows, closed door
//! leaves, balconies with their size, phase colours) over the registered archive plan, as a
//! PNG (for agents) or an SVG (vector, for drawing sets and offers).

use std::collections::BTreeMap;
use std::fmt::Write as _;
use std::path::PathBuf;

use serde_json::{Value, json};
use trigon_model::build::{FacadeFrame, Status, View, build_view};
use trigon_model::{ComponentKind, Document, ModelError};

use crate::sheet::{Element, Fill, Halo, Layer, MM_PER_PX, PAGE_MM, Scene, Stroke, Text, Viewport, dxf, scale_text, sheet_scale, svg};
use crate::views::DrawingElementKind;
use crate::{EngineError, Result, Session};

const CUT: f64 = 1000.0;
const WALL: [u8; 3] = [25, 25, 25];
const PARTITION: [u8; 3] = [110, 110, 110];
const ROOM: [u8; 3] = [90, 90, 90];
const LINE: [u8; 3] = [30, 30, 30];
const NEW: [u8; 3] = [40, 150, 30];
const DEMOLISHED: [u8; 3] = [220, 30, 30];

/// Choose a point with wall clearance inside a room, including concave room polygons.
pub(crate) fn room_label_anchor(poly: &[[f64; 2]]) -> Option<([f64; 2], f64)> {
    let first = *poly.first()?;
    let (lo, hi) = poly.iter().fold((first[1], first[1]), |(a, b), p| (a.min(p[1]), b.max(p[1])));
    let mut best: Option<([f64; 2], f64)> = None;
    for row in 0..33 {
        let y = lo + (hi - lo) * (row as f64 + 0.5) / 33.0;
        let mut xs = Vec::new();
        for i in 0..poly.len() {
            let (a, b) = (poly[i], poly[(i + 1) % poly.len()]);
            if (a[1] > y) != (b[1] > y) {
                xs.push(a[0] + (y - a[1]) * (b[0] - a[0]) / (b[1] - a[1]));
            }
        }
        xs.sort_by(f64::total_cmp);
        for pair in xs.as_chunks::<2>().0 {
            let p = [(pair[0] + pair[1]) / 2.0, y];
            let clearance = (0..poly.len())
                .map(|i| {
                    let (a, b) = (poly[i], poly[(i + 1) % poly.len()]);
                    let d = [b[0] - a[0], b[1] - a[1]];
                    let t = (((p[0] - a[0]) * d[0] + (p[1] - a[1]) * d[1]) / (d[0] * d[0] + d[1] * d[1]).max(1e-12)).clamp(0.0, 1.0);
                    (p[0] - a[0] - t * d[0]).hypot(p[1] - a[1] - t * d[1])
                })
                .fold(f64::INFINITY, f64::min);
            if clearance > 0.0 && best.is_none_or(|(_, d)| clearance > d) {
                best = Some((p, clearance));
            }
        }
    }
    best
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Surface {
    Exterior,
    Interior,
    Stair,
}

enum Prim {
    /// Independent site footprints, displayed only in the ground plan.
    Site {
        pts: Vec<[f64; 2]>,
        fill: [u8; 3],
    },
    /// Semantic filled areas; never emitted as graphical ink by either writer.
    PickRegion(crate::views::DrawingElementRegion),
    Poly {
        pts: Vec<[f64; 2]>,
        fill: [u8; 3],
        surface: Surface,
    },
    Line {
        a: [f64; 2],
        b: [f64; 2],
        color: [u8; 3],
        width: f64,
        dashed: bool,
    },
    /// Model-derived stair detail has its own layer and neutral direction-line styling.
    StairLine {
        a: [f64; 2],
        b: [f64; 2],
        width: f64,
        dashed: bool,
    },
    /// `away`: the unit direction a label may step out along when it collides with another.
    Text {
        at: [f64; 2],
        text: String,
        color: [u8; 3],
        size: f64,
        angle: Option<f64>,
        away: Option<[f64; 2]>,
        entrance: Option<crate::sheet::EntranceLabel>,
    },
}

fn status_color(s: Status, normal: [u8; 3]) -> [u8; 3] {
    match s {
        Status::Existing => normal,
        Status::New => NEW,
        Status::Demolished => DEMOLISHED,
    }
}

fn pt(fr: &FacadeFrame, u: f64, w: f64) -> [f64; 2] {
    let p = fr.point(u, w, 0.0);
    [p[0], p[1]]
}

/// Distance along the ray from `from` in direction `dir` (unit) to the first footprint edge it
/// crosses.
pub(crate) fn facade_crossing(ring: &[[f64; 2]], from: [f64; 2], dir: [f64; 2]) -> Option<f64> {
    let n = ring.len();
    (0..n)
        .filter_map(|i| {
            let (a, b) = (ring[i], ring[(i + 1) % n]);
            let e = [b[0] - a[0], b[1] - a[1]];
            let den = dir[0] * e[1] - dir[1] * e[0];
            if den.abs() < 1e-9 {
                return None;
            }
            let w = [a[0] - from[0], a[1] - from[1]];
            let t = (w[0] * e[1] - w[1] * e[0]) / den;
            let s = (w[0] * dir[1] - w[1] * dir[0]) / den;
            (t > 0.0 && (0.0..=1.0).contains(&s)).then_some(t)
        })
        .min_by(f64::total_cmp)
}

pub(crate) fn visible_stairs(b: &trigon_model::Building, level: usize) -> Vec<&trigon_model::Stair> {
    let outgoing: Vec<_> = b.stairs.iter().filter(|s| s.levels.contains(&level)).collect();
    b.stairs
        .iter()
        .filter(|s| {
            if s.levels.contains(&level) {
                return true;
            }
            if !level.checked_sub(1).is_some_and(|previous| s.levels.contains(&previous)) {
                return false;
            }
            let n = s.outline.len().max(1) as f64;
            let centre = s.outline.iter().fold([0.0; 2], |a, p| [a[0] + p[0] / n, a[1] + p[1] / n]);
            !outgoing.iter().any(|next| trigon_model::stair::opening(next).is_ok_and(|ring| crate::drawings::inside(&ring, centre)))
        })
        .collect()
}

/// Mixed plans include site geometry once when a full-storey source is at ground level.
/// Legacy plans keep their established first-building convention.
fn includes_site(doc: &Document, level: usize) -> bool {
    if doc.buildings.iter().any(|b| b.attic_level() == Some(level)) {
        doc.buildings.iter().any(|b| b.ground_level == level && b.storeys.get(level).is_some_and(|s| !s.roof_level))
    } else {
        level == doc.buildings.first().map_or(0, |b| b.ground_level)
    }
}

/// Full-storey symbolic cuts in site mm. Roof levels are composed from actual meshes instead.
fn scene(doc: &Document, level: usize, view: View, presentation: bool) -> Result<(Vec<Prim>, String, BTreeMap<usize, Element>)> {
    if !doc.buildings.iter().any(|b| b.storeys.get(level).is_some_and(|s| !s.roof_level)) {
        return Err(ModelError::Invalid(format!("level {level} doesn't exist")).into());
    }
    let mut prims = Vec::new();
    let mut elements = BTreeMap::new();
    let mut title = String::new();
    if includes_site(doc, level) {
        let mut site = trigon_model::site::build(doc, view, None)?;
        // Areas under furniture and canopies. The outlines here are the actual parametric
        // footprints, so selection never invents an opening or a building identity.
        site.sort_by_key(|o| if o.class == trigon_model::site::SiteClass::Ground { 0 } else { 1 });
        for o in site {
            let material = if o.class == trigon_model::site::SiteClass::Tree {
                trigon_model::Material::Foliage
            } else {
                o.parts.first().map_or(trigon_model::Material::Paving, |p| p.0)
            };
            let rgba = material.rgba();
            let material_colour =
                [((rgba[0].sqrt() * 190.) + 40.) as u8, ((rgba[1].sqrt() * 190.) + 40.) as u8, ((rgba[2].sqrt() * 190.) + 40.) as u8];
            let color = if presentation && o.status != Status::Demolished { material_colour } else { status_color(o.status, material_colour) };
            elements.insert(prims.len(), Element::new(&o.id, DrawingElementKind::Site));
            prims.push(Prim::Site { pts: o.footprint, fill: color });
        }
    }
    // Validate/build the complete document before selecting presentation sources. Global
    // component, site and terrain context must not depend on a filtered Document clone.
    let built: Vec<_> = build_view(doc, view)?.into_iter().filter(|b| b.storeys.get(level).is_some_and(|s| !s.roof_level)).collect();
    for b in &built {
        let Some(storey) = b.storeys.get(level) else { continue };
        title = format!("{} — {}", b.name, storey.name);
        let cut = storey.elevation + CUT;
        for slab in
            b.slabs.iter().filter(|slab| slab.level == level && slab.kind == trigon_model::SlabKind::Floor && slab.z_bottom + slab.thickness <= cut)
        {
            let mut rings = vec![slab.polygon.clone()];
            rings.extend(slab.holes.iter().cloned());
            prims.push(Prim::PickRegion(crate::views::DrawingElementRegion {
                id: slab.id.clone(),
                kind: DrawingElementKind::Slab,
                rings,
                depth_plane: None,
            }));
        }
        for w in &b.walls {
            let fr = &w.frame;
            // The facade wall as it is on this storey (thicker lower down).
            let Some(seg) = w.segment(level) else { continue };
            let d = seg.thickness;
            // Openings crossing the cut, by position along the wall.
            let mut cuts: Vec<_> = w
                .openings
                .iter()
                .filter_map(|o| {
                    if o.rect.v0 > cut || o.rect.v1 < cut {
                        return None;
                    }
                    let (u0, u1) = if o.profile.is_empty() { (o.rect.u0, o.rect.u1) } else { trigon_model::profile::width_at(&o.profile, cut)? };
                    (u1 - u0 > 1e-8).then_some((o, u0, u1))
                })
                .collect();
            cuts.sort_by(|x, y| x.1.total_cmp(&y.1));
            let [a, e, qe, qs] = seg.plan;
            let mut start = (a, qs);
            for &(o, u0, u1) in &cuts {
                elements.insert(prims.len(), Element::new(&w.id, DrawingElementKind::FacadeWall));
                prims.push(Prim::Poly { pts: vec![start.0, pt(fr, u0, 0.0), pt(fr, u0, d), start.1], fill: WALL, surface: Surface::Exterior });
                start = (pt(fr, u1, 0.0), pt(fr, u1, d));
                let opening_start = prims.len();
                let color = status_color(o.status, LINE);
                let kind = doc.component(&o.fill.type_id).map(|t| t.kind.clone()).ok();
                let french_rail = matches!(&kind, Some(ComponentKind::FrenchBalcony { .. }));
                // Window: frame and glass lines; door: leaf and swing; a door + window unit: both.
                let window = |prims: &mut Vec<Prim>, wu0: f64, wu1: f64| {
                    for depth in [d * 0.35, d * 0.55] {
                        prims.push(Prim::Line { a: pt(fr, wu0, depth), b: pt(fr, wu1, depth), color, width: 25.0, dashed: false });
                    }
                    prims.push(Prim::Line { a: pt(fr, wu0, 0.0), b: pt(fr, wu1, 0.0), color, width: 15.0, dashed: false });
                };
                // Doors as a leaf line in the closed position: no swing, no opening direction.
                let door = |prims: &mut Vec<Prim>, hinge: f64, other: f64| {
                    prims.push(Prim::Line { a: pt(fr, hinge, d * 0.5), b: pt(fr, other, d * 0.5), color, width: 45.0, dashed: false });
                };
                match kind {
                    Some(ComponentKind::Window { .. } | ComponentKind::HistoricWindow { .. } | ComponentKind::FixedWindow) => {
                        window(&mut prims, u0, u1)
                    }
                    Some(ComponentKind::Loggia { depth, door_width, door_side, .. }) => {
                        // Open to the cut: brick returns, the glazing set back, a rail across the parapet.
                        let back = depth.max(d + 300.0);
                        for (a0, a1) in [(u0 - 110.0, u0), (u1, u1 + 110.0)] {
                            prims.push(Prim::Poly {
                                pts: vec![pt(fr, a0, d), pt(fr, a1, d), pt(fr, a1, back + 110.0), pt(fr, a0, back + 110.0)],
                                fill: WALL,
                                surface: Surface::Exterior,
                            });
                        }
                        let (du0, du1) = if door_side == trigon_model::component::Side::Left { (u0, u0 + door_width) } else { (u1 - door_width, u1) };
                        prims.push(Prim::Line { a: pt(fr, du0, back - 35.0), b: pt(fr, du1, back - 35.0), color, width: 45.0, dashed: false });
                        let (wu0, wu1) = if door_side == trigon_model::component::Side::Left { (u0 + door_width, u1) } else { (u0, u1 - door_width) };
                        for depth in [back - 60.0, back - 15.0] {
                            prims.push(Prim::Line { a: pt(fr, wu0, depth), b: pt(fr, wu1, depth), color, width: 25.0, dashed: false });
                        }
                        prims.push(Prim::Line { a: pt(fr, u0, d * 0.4), b: pt(fr, u1, d * 0.4), color, width: 14.0, dashed: false });
                    }
                    Some(ComponentKind::DoorWindow { door_width, door_side, .. }) => {
                        if door_side == trigon_model::component::Side::Left {
                            door(&mut prims, u0, u0 + door_width);
                            window(&mut prims, u0 + door_width, u1);
                        } else {
                            door(&mut prims, u1, u1 - door_width);
                            window(&mut prims, u0, u1 - door_width);
                        }
                    }
                    Some(
                        ComponentKind::FrenchDoor { leaves, .. }
                        | ComponentKind::FrenchBalcony { leaves, .. }
                        | ComponentKind::Balcony { leaves, .. },
                    ) if leaves >= 2 => {
                        let m = (u0 + u1) / 2.0;
                        door(&mut prims, u0, m);
                        door(&mut prims, u1, m);
                    }
                    _ => door(&mut prims, u0, u1),
                }
                for index in opening_start..prims.len() {
                    elements.insert(index, Element::new(&o.fill.id, DrawingElementKind::Opening));
                }
                if french_rail {
                    for depth in [-110.0, -150.0] {
                        if let Some(rail) = o.attached.iter().find(|element| element.class == trigon_model::ElementClass::Railing) {
                            elements.insert(prims.len(), Element::new(&rail.id, DrawingElementKind::Railing));
                        }
                        prims.push(Prim::Line {
                            a: pt(fr, u0 - 50.0, depth),
                            b: pt(fr, u1 + 50.0, depth),
                            color,
                            width: 14.0,
                            dashed: o.status == Status::Demolished,
                        });
                    }
                }
            }
            elements.insert(prims.len(), Element::new(&w.id, DrawingElementKind::FacadeWall));
            prims.push(Prim::Poly { pts: vec![start.0, e, qe, start.1], fill: WALL, surface: Surface::Exterior });
            // Balconies on this storey, from the facade's resolved placements.
            let Some(model_b) = doc.buildings.iter().find(|x| x.id == b.building_id) else { continue };
            let Ok(f) = model_b.facade(&w.id) else { continue };
            for r in trigon_model::build::resolve_facade(doc, model_b, f)? {
                if (r.is_opening() && !matches!(r.ty.kind, ComponentKind::Balcony { .. })) || r.placement.level != level {
                    continue;
                }
                let Some(status) = view.status(r.lifetime) else { continue };
                let Some(depth) = r.ty.kind.depth() else { continue };
                let slab_width = match r.ty.kind {
                    ComponentKind::Balcony { slab_width, .. } => slab_width,
                    _ => r.ty.width,
                };
                let centre = (r.rect.u0 + r.rect.u1) / 2.0;
                let (u0, u1) = (centre - slab_width / 2.0, centre + slab_width / 2.0);
                let color = status_color(status, LINE);
                let shaped = matches!(r.ty.kind, ComponentKind::ShapedBalcony { .. });
                let corners = if let ComponentKind::ShapedBalcony { ref plan, .. } = r.ty.kind {
                    plan.outline(slab_width, depth)?.iter().map(|p| pt(fr, centre + p[0], -p[1])).collect::<Vec<_>>()
                } else {
                    vec![pt(fr, u0, 0.0), pt(fr, u1, 0.0), pt(fr, u1, -depth), pt(fr, u0, -depth)]
                };
                let prefix = if r.is_opening() {
                    format!("{}.L{}.B{}", w.id, level, r.placement.bay)
                } else {
                    format!("{}.L{}.B{}.balcony", w.id, level, r.placement.bay)
                };
                let parts: Vec<_> = w
                    .openings
                    .iter()
                    .flat_map(|opening| opening.attached.iter())
                    .chain(w.attachments.iter())
                    .filter(|element| element.level == level && element.type_id == r.ty.id && element.id.starts_with(&format!("{prefix}.")))
                    .collect();
                let owner = parts.iter().copied().min_by_key(|element| if element.class == trigon_model::ElementClass::BalconySlab { 0 } else { 1 });
                let owner = owner.map(|element| {
                    Element::new(
                        &element.id,
                        if element.class == trigon_model::ElementClass::BalconySlab {
                            DrawingElementKind::Balcony
                        } else {
                            DrawingElementKind::Railing
                        },
                    )
                });
                let rail = parts
                    .iter()
                    .find(|element| element.class == trigon_model::ElementClass::Railing)
                    .map(|element| Element::new(&element.id, DrawingElementKind::Railing));
                if let ComponentKind::GuardRail { projection, railing, handrail, .. } = r.ty.kind {
                    let half = if handrail { 35.0 } else { 20.0 };
                    let inset = if railing == trigon_model::component::Railing::Osterbro { 0.0 } else { 20.0 };
                    let outline = [
                        pt(fr, u0, -projection - half),
                        pt(fr, u1, -projection - half),
                        pt(fr, u1, -projection + half),
                        pt(fr, u0, -projection + half),
                    ];
                    let start = prims.len();
                    for k in 0..4 {
                        prims.push(Prim::Line { a: outline[k], b: outline[(k + 1) % 4], color, width: 14.0, dashed: status == Status::Demolished });
                    }
                    for u in [u0 + inset + 20.0, u1 - inset - 20.0] {
                        prims.push(Prim::Line {
                            a: pt(fr, u, -projection + 20.0),
                            b: pt(fr, u, 0.0),
                            color,
                            width: 14.0,
                            dashed: status == Status::Demolished,
                        });
                    }
                    if let Some(rail) = rail {
                        for index in start..prims.len() {
                            elements.insert(index, rail.clone());
                        }
                        prims.push(Prim::PickRegion(crate::views::DrawingElementRegion {
                            id: rail.id,
                            kind: DrawingElementKind::Railing,
                            rings: vec![outline.to_vec()],
                            depth_plane: None,
                        }));
                    }
                    continue;
                }
                let mut rail_indices = Vec::new();
                if let Some(element) = &owner {
                    prims.push(Prim::PickRegion(crate::views::DrawingElementRegion {
                        id: element.id.clone(),
                        kind: element.kind,
                        rings: vec![corners.to_vec()],
                        depth_plane: None,
                    }));
                }
                let detail_start = prims.len();
                let dashed = status == Status::Demolished;
                for k in 0..corners.len() {
                    prims.push(Prim::Line { a: corners[k], b: corners[(k + 1) % corners.len()], color, width: 30.0, dashed });
                }
                let mut detail_end = prims.len();
                if presentation && status != Status::Demolished && shaped {
                    if let ComponentKind::ShapedBalcony { ref plan, .. } = r.ty.kind {
                        let path = plan.guard_path(slab_width, depth)?;
                        for pair in path.windows(2) {
                            rail_indices.push(prims.len());
                            prims.push(Prim::Line {
                                a: pt(fr, centre + pair[0][0], -pair[0][1]),
                                b: pt(fr, centre + pair[1][0], -pair[1][1]),
                                color,
                                width: 14.0,
                                dashed: false,
                            });
                        }
                    }
                    detail_end = prims.len();
                }
                if presentation && status != Status::Demolished && !shaped {
                    // The deck and perimeter rail are separate from the cut wall.
                    for (edge, v) in [70.0, depth - 70.0].into_iter().enumerate() {
                        if edge == 1 {
                            rail_indices.push(prims.len());
                        }
                        prims.push(Prim::Line { a: pt(fr, u0 + 70.0, -v), b: pt(fr, u1 - 70.0, -v), color, width: 14.0, dashed: false });
                    }
                    for u in [u0 + 70.0, u1 - 70.0] {
                        rail_indices.push(prims.len());
                        prims.push(Prim::Line { a: pt(fr, u, -70.0), b: pt(fr, u, -depth + 70.0), color, width: 14.0, dashed: false });
                    }
                    let boards = if matches!(r.ty.kind, ComponentKind::SteelBalcony { deck, .. } if deck != trigon_model::component::Deck::FibreConcrete)
                    {
                        (depth / 140.0).round().clamp(2.0, 50.0) as usize
                    } else {
                        1
                    };
                    for k in 1..boards {
                        let v = -depth * k as f64 / boards as f64;
                        prims.push(Prim::Line { a: pt(fr, u0 + 70.0, v), b: pt(fr, u1 - 70.0, v), color, width: 7.0, dashed: false });
                    }
                    detail_end = prims.len();
                    let dim = -depth - 300.0;
                    prims.push(Prim::Line { a: pt(fr, u0, dim), b: pt(fr, u1, dim), color, width: 10.0, dashed: false });
                    for u in [u0, u1] {
                        prims.push(Prim::Line { a: pt(fr, u, -depth - 100.0), b: pt(fr, u, dim - 130.0), color, width: 7.0, dashed: false });
                        prims.push(Prim::Line {
                            a: pt(fr, u - 90.0, dim + 90.0),
                            b: pt(fr, u + 90.0, dim - 90.0),
                            color,
                            width: 18.0,
                            dashed: false,
                        });
                    }
                }
                if status != Status::Demolished {
                    let label_at = pt(fr, (u0 + u1) / 2.0, -depth - 450.0);
                    let out = pt(fr, (u0 + u1) / 2.0, -depth - 1450.0);
                    if let Some(element) = &owner {
                        elements.insert(prims.len(), element.clone());
                    }
                    prims.push(Prim::Text {
                        at: label_at,
                        text: format!("Ca. {:.0} x {:.0}", depth, slab_width),
                        color,
                        size: 280.0,
                        angle: presentation.then(|| fr.t[1].atan2(fr.t[0])),
                        away: Some([(out[0] - label_at[0]) / 1000.0, (out[1] - label_at[1]) / 1000.0]),
                        entrance: None,
                    });
                }
                if let Some(owner) = owner {
                    for index in detail_start..detail_end {
                        elements.insert(index, owner.clone());
                    }
                }
                if let Some(rail) = rail {
                    for index in rail_indices {
                        elements.insert(index, rail.clone());
                    }
                }
            }
        }
    }
    // Outdoor cellar stairs: cut in the basement, seen from above on the ground storey.
    for b in doc.buildings.iter().filter(|b| b.storeys.get(level).is_some_and(|s| !s.roof_level)) {
        if level != 0 && level != b.ground_level {
            continue;
        }
        for (index, c) in b.cellar_stairs.iter().enumerate() {
            let Ok(f) = b.facade(&c.facade) else { continue };
            let Some((a, e)) = b.edge(f.edge) else { continue };
            let Some(fr) = trigon_model::FacadeFrame::new(a, e) else { continue };
            let id = if c.id.is_empty() { format!("{}.cellar{index}", f.id) } else { c.id.clone() };
            let owner = Element::new(&id, DrawingElementKind::Stair);
            let start = prims.len();
            let (out, r) = (c.width, 150.0);
            let walls = [
                [(c.from - r, out), (c.to + r, out), (c.to + r, out + r), (c.from - r, out + r)],
                [(c.from - r, 0.0), (c.from, 0.0), (c.from, out), (c.from - r, out)],
                [(c.to, 0.0), (c.to + r, 0.0), (c.to + r, out), (c.to, out)],
            ];
            for q in walls {
                let pts: Vec<[f64; 2]> = q.iter().map(|&(u, w)| pt(&fr, u, -w)).collect();
                if level == 0 {
                    prims.push(Prim::Poly { pts, fill: WALL, surface: Surface::Exterior });
                } else {
                    for k in 0..4 {
                        prims.push(Prim::Line { a: pts[k], b: pts[(k + 1) % 4], color: LINE, width: 14.0, dashed: false });
                    }
                }
            }
            let risers = ((-c.floor) / 175.0).round().clamp(3.0, 40.0) as usize;
            for i in 0..=risers {
                let u = c.bottom + (c.top - c.bottom) * i as f64 / risers as f64;
                prims.push(Prim::Line { a: pt(&fr, u, 0.0), b: pt(&fr, u, -out), color: LINE, width: 9.0, dashed: false });
            }
            for index in start..prims.len() {
                elements.insert(index, owner.clone());
            }
            let rings = vec![vec![pt(&fr, c.from, 0.0), pt(&fr, c.to, 0.0), pt(&fr, c.to, -out), pt(&fr, c.from, -out)]];
            prims.push(Prim::PickRegion(crate::views::DrawingElementRegion { id, kind: DrawingElementKind::Stair, rings, depth_plane: None }));
        }
    }
    // Interior: walls cut at this storey (joined to each other and to the facade), stairs with
    // treads, room names and areas.
    for built in &built {
        for w in built.interior.iter().filter(|w| w.level == level) {
            for pts in &w.cut {
                elements.insert(prims.len(), Element::new(&w.id, DrawingElementKind::InteriorWall));
                prims.push(Prim::Poly { pts: pts.clone(), fill: if w.load_bearing { WALL } else { PARTITION }, surface: Surface::Interior });
            }
            // Doors as a closed leaf across the opening: no swing, no opening direction.
            for d in &w.doors {
                let mid = w.thickness / 2.0;
                elements.insert(prims.len(), Element::new(&d.fill.id, DrawingElementKind::Opening));
                prims.push(Prim::Line { a: pt(&w.frame, d.rect.u0, mid), b: pt(&w.frame, d.rect.u1, mid), color: LINE, width: 30.0, dashed: false });
            }
        }
    }
    for b in doc.buildings.iter().filter(|b| b.storeys.get(level).is_some_and(|s| !s.roof_level)) {
        let visible = visible_stairs(b, level);
        for st in &visible {
            let start = prims.len();
            let departure = if st.levels.contains(&level) { level } else { level.saturating_sub(1) };
            let Some(index) = b.stairs.iter().position(|source| std::ptr::eq(source, *st)) else { continue };
            let id = if st.id.is_empty() { format!("{}.stair{index}.L{departure}", b.id) } else { format!("{}.L{departure}", st.id) };
            let owner = Element::new(&id, DrawingElementKind::Stair);
            if presentation {
                prims.push(Prim::Poly { pts: trigon_model::stair::opening(st)?, fill: [245, 245, 245], surface: Surface::Stair });
            }
            let line = |a, b, width, dashed| {
                if presentation { Prim::StairLine { a, b, width, dashed } } else { Prim::Line { a, b, color: LINE, width, dashed } }
            };
            let h = b.storeys.get(departure).map_or(3000.0, |s| s.height);
            let Ok(sym) = trigon_model::stair::symbol(st, h) else { continue };
            for (a, e) in &sym.outline {
                prims.push(line(*a, *e, 18.0, false));
            }
            for (a, e) in &sym.treads {
                prims.push(line(*a, *e, 9.0, false));
            }
            for k in 1..sym.landing.len() {
                prims.push(line(sym.landing[k - 1], sym.landing[k], 18.0, false));
            }
            for k in 1..sym.walk.len() {
                prims.push(line(sym.walk[k - 1], sym.walk[k], 7.0, true));
            }
            for (a, b, width) in &sym.markings {
                prims.push(line(*a, *b, *width, false));
            }
            for index in start..prims.len() {
                elements.insert(index, owner.clone());
            }
            prims.push(Prim::PickRegion(crate::views::DrawingElementRegion {
                id,
                kind: DrawingElementKind::Stair,
                rings: vec![trigon_model::stair::opening(st)?],
                depth_plane: None,
            }));
        }
        // Entrances: house number on the staircase's floor landing, the apartments' doors either side
        // ("TV" to the left and "TH" to the right as you arrive up the stairs).
        for (entrance_index, e) in b.entrances.iter().enumerate() {
            let units: Vec<_> = e.units.iter().filter(|u| entrance_unit_level(u, b.ground_level) == Some(level)).collect();
            // Prefer the departing flight; on its receiving floor use the preceding departure.
            let Some(st) = [Some(level), level.checked_sub(1)].into_iter().flatten().find_map(|departure| {
                let id = e.stair_at(departure)?;
                visible.iter().copied().find(|s| s.id == id && s.levels.contains(&departure))
            }) else {
                if presentation && !units.is_empty() {
                    let mut tag = entrance_label(&b.id, entrance_index, e, &units, true, "  ");
                    tag.placement_note = Some("missing_explicit_stair_association");
                    prims.push(Prim::Text {
                        at: e.point,
                        text: tag.text.clone(),
                        color: LINE,
                        size: 380.0,
                        angle: None,
                        away: None,
                        entrance: Some(tag),
                    });
                }
                continue;
            };
            let Ok(f) = trigon_model::stair::frame(st) else { continue };
            let landing = f.at(-700.0, f.width / 2.0);
            // Arriving on the floor you walk back along the second flight: facing −run.
            let left = [f.run[1], -f.run[0]];
            if presentation {
                // On the stair's axis, just outside the facade it meets on the entrance's side, and
                // aligned with the wing (across the flights) so stepped facades don't tilt it.
                let core = f.at(f.length / 2.0, f.width / 2.0);
                let label = [f.run, [-f.run[0], -f.run[1]]]
                    .into_iter()
                    .filter_map(|dir| {
                        let t = facade_crossing(&b.footprint, core, dir)?;
                        let hit = [core[0] + dir[0] * t, core[1] + dir[1] * t];
                        Some(((hit[0] - e.point[0]).hypot(hit[1] - e.point[1]), [hit[0] + dir[0] * 1100.0, hit[1] + dir[1] * 1100.0], dir))
                    })
                    .min_by(|a, b| a.0.total_cmp(&b.0));
                let mut tag = entrance_label(&b.id, entrance_index, e, &units, true, "  ");
                let (at, outward) = match label {
                    Some((_, at, dir)) => (at, Some(dir)),
                    None => {
                        tag.placement_note = Some("no_facade_crossing_for_stair_axis");
                        (e.point, None)
                    }
                };
                prims.push(Prim::Text {
                    at,
                    text: tag.text.clone(),
                    color: LINE,
                    size: 380.0,
                    angle: Some(f.across[1].atan2(f.across[0])),
                    away: outward,
                    entrance: Some(tag),
                });
                continue;
            }
            prims.push(Prim::Text { at: landing, text: e.number.clone(), color: LINE, size: 380.0, angle: None, away: None, entrance: None });
            let mut others = Vec::new();
            let mut unnamed = 0;
            for u in &units {
                let side = match u.door.as_str() {
                    "tv" => Some(1.0),
                    "th" => Some(-1.0),
                    _ => None,
                };
                match side {
                    Some(k) => {
                        let at = [landing[0] + left[0] * 1700.0 * k, landing[1] + left[1] * 1700.0 * k];
                        prims.push(Prim::Text { at, text: u.door.to_uppercase(), color: LINE, size: 320.0, angle: None, away: None, entrance: None });
                    }
                    None if u.door.is_empty() => unnamed += 1,
                    None => others.push(u.door.to_uppercase()),
                }
            }
            if unnamed > 0 {
                others.push(format!("{unnamed} lejl."));
            }
            if !others.is_empty() {
                let at = [landing[0] - f.run[0] * 450.0, landing[1] - f.run[1] * 450.0];
                prims.push(Prim::Text { at, text: others.join(" "), color: LINE, size: 240.0, angle: None, away: None, entrance: None });
            }
        }
        for (index, r) in b.rooms.iter().enumerate() {
            if !r.levels.contains(&level) || r.polygon.is_empty() {
                continue;
            }
            prims.push(Prim::PickRegion(crate::views::DrawingElementRegion {
                id: crate::room_element_id(b, index, level),
                kind: DrawingElementKind::Room,
                rings: vec![r.polygon.clone()],
                depth_plane: None,
            }));
            if presentation {
                continue;
            }
            let Some((c, _)) = room_label_anchor(&r.polygon) else { continue };
            let area = trigon_model::doc::polygon_area(&r.polygon) / 1e6;
            if r.name.is_empty() {
                prims.push(Prim::Text { at: c, text: format!("{area:.1} m²"), color: ROOM, size: 200.0, angle: None, away: None, entrance: None });
            } else {
                prims.push(Prim::Text {
                    at: [c[0], c[1] + 220.0],
                    text: r.name.clone(),
                    color: LINE,
                    size: 240.0,
                    angle: None,
                    away: None,
                    entrance: None,
                });
                prims.push(Prim::Text {
                    at: [c[0], c[1] - 150.0],
                    text: format!("{area:.1} m²"),
                    color: ROOM,
                    size: 200.0,
                    angle: None,
                    away: None,
                    entrance: None,
                });
            }
        }
    }
    Ok((prims, title, elements))
}

/// Maps site mm to output pixels: rotated by `rot` (degrees, counter-clockwise) about the
/// bounds' centre, north-up when 0.
struct Frame {
    c: [f64; 2],
    cos: f64,
    sin: f64,
    x0: f64,
    y1: f64,
    mm: f64,
    w: u32,
    h: u32,
}

impl Frame {
    fn rot(&self, p: [f64; 2]) -> [f64; 2] {
        let (x, y) = (p[0] - self.c[0], p[1] - self.c[1]);
        [self.cos * x - self.sin * y, self.sin * x + self.cos * y]
    }

    fn px(&self, p: [f64; 2]) -> [f64; 2] {
        let r = self.rot(p);
        [(r[0] - self.x0) / self.mm, (self.y1 - r[1]) / self.mm]
    }

    /// Output pixel centre back to site mm.
    fn site(&self, i: u32, j: u32) -> [f64; 2] {
        let (rx, ry) = (self.x0 + (f64::from(i) + 0.5) * self.mm, self.y1 - (f64::from(j) + 0.5) * self.mm);
        [self.c[0] + self.cos * rx + self.sin * ry, self.c[1] - self.sin * rx + self.cos * ry]
    }
}

fn frame_for(doc: &Document, mm: f64, rot_deg: f64, level: usize, view: View) -> Result<Frame> {
    let pts = plan_extents(doc, level, view)?;
    let first = *pts.first().ok_or_else(|| ModelError::NotFound("no building yet".into()))?;
    let (lo, hi) = pts.iter().fold((first, first), |(lo, hi), p| ([lo[0].min(p[0]), lo[1].min(p[1])], [hi[0].max(p[0]), hi[1].max(p[1])]));
    let c = [(lo[0] + hi[0]) / 2.0, (lo[1] + hi[1]) / 2.0];
    let t = rot_deg.to_radians();
    let mut f = Frame { c, cos: t.cos(), sin: t.sin(), x0: 0.0, y1: 0.0, mm, w: 1, h: 1 };
    let r: Vec<[f64; 2]> = pts.iter().map(|p| f.rot(*p)).collect();
    let margin = 4000.0;
    let (rx0, rx1) = r.iter().fold((f64::MAX, f64::MIN), |(a, b), p| (a.min(p[0]), b.max(p[0])));
    let (ry0, ry1) = r.iter().fold((f64::MAX, f64::MIN), |(a, b), p| (a.min(p[1]), b.max(p[1])));
    f.x0 = rx0 - margin;
    f.y1 = ry1 + margin;
    f.w = ((rx1 - rx0 + 2.0 * margin) / mm).ceil().clamp(1.0, 6000.0) as u32;
    f.h = ((ry1 - ry0 + 2.0 * margin) / mm).ceil().clamp(1.0, 6000.0) as u32;
    Ok(f)
}

/// The registered archive plan under the cut, lightened, or white.
fn underlay(s: &mut Session, f: &Frame, level: usize, drawing: Option<&str>) -> Result<(Vec<u8>, Option<String>)> {
    let mut rgb = vec![255u8; (f.w * f.h * 3) as usize];
    let pick = s.doc.drawings.iter().find(|d| drawing.is_none_or(|n| n == d.name) && d.plans.iter().any(|p| p.levels.contains(&level))).cloned();
    let Some(d) = pick else {
        if let Some(n) = drawing {
            return Err(ModelError::NotFound(format!("drawing {n} has no plan registered for level {level}; run drawing.register_plan")).into());
        }
        return Ok((rgb, None));
    };
    let Some(reg) = d.plans.iter().find(|p| p.levels.contains(&level)).cloned() else { return Ok((rgb, None)) };
    let img = crate::drawings::image(s, &d.path, d.page, d.rotate)?;
    for j in 0..f.h {
        for i in 0..f.w {
            let [x, y] = reg.to_drawing(f.site(i, j));
            if let Some(g) = img.sample(x, y) {
                let v = (f64::from(g) * 0.55 + 255.0 * 0.45) as u8;
                let k = ((j * f.w + i) * 3) as usize;
                if let Some(dst) = rgb.get_mut(k..k + 3) {
                    dst.copy_from_slice(&[v, v, v]);
                }
            }
        }
    }
    Ok((rgb, Some(d.name)))
}

fn hex(c: [u8; 3]) -> String {
    format!("#{:02x}{:02x}{:02x}", c[0], c[1], c[2])
}

pub(crate) fn write_output(path: &std::path::Path, bytes: &[u8]) -> Result<()> {
    if let Some(dir) = path.parent().filter(|d| !d.as_os_str().is_empty()) {
        std::fs::create_dir_all(dir).map_err(|e| EngineError::Io(format!("{}: {e}", dir.display())))?;
    }
    use std::io::Write as _;
    use std::sync::atomic::{AtomicU64, Ordering};
    static NEXT: AtomicU64 = AtomicU64::new(0);
    let name = path.file_name().ok_or_else(|| EngineError::Io("the output needs a file name".into()))?;
    let mut temporary = name.to_os_string();
    temporary.push(format!(".{}-{}.tmp", std::process::id(), NEXT.fetch_add(1, Ordering::Relaxed)));
    let temporary = path.with_file_name(temporary);
    let mut file =
        std::fs::OpenOptions::new().write(true).create_new(true).open(&temporary).map_err(|e| EngineError::Io(format!("{}: {e}", path.display())))?;
    let write = (|| -> std::io::Result<()> {
        file.write_all(bytes)?;
        file.sync_all()?;
        drop(file);
        std::fs::rename(&temporary, path)
    })();
    if let Err(e) = write {
        let _ = std::fs::remove_file(&temporary);
        return Err(EngineError::Io(format!("{}: {e}", path.display())));
    }
    Ok(())
}

pub(crate) fn escape(s: &str) -> String {
    s.replace('&', "&amp;").replace('<', "&lt;").replace('>', "&gt;").replace('"', "&quot;")
}

fn region_corners(points: &[[f64; 2]]) -> Vec<[f64; 2]> {
    match points {
        [a, b] => vec![*a, [b[0], a[1]], *b, [a[0], b[1]]],
        _ => points.to_vec(),
    }
}

/// Current-document unit floors, with bounded arithmetic for hostile imported strings.
fn entrance_unit_level(unit: &trigon_model::Unit, ground: usize) -> Option<usize> {
    unit.level(ground)
}

/// One display formatter for plans and elevations. Absence is never filled from convention.
pub(crate) fn entrance_label(
    building: &str,
    index: usize,
    entrance: &trigon_model::Entrance,
    units: &[&trigon_model::Unit],
    front: bool,
    separator: &str,
) -> crate::sheet::EntranceLabel {
    let mut known = Vec::new();
    let mut unknown = 0;
    for unit in units {
        let door = unit.door.trim().to_uppercase();
        if trigon_model::Unit::is_unknown_door(&unit.door) {
            unknown += 1;
        } else if !known.contains(&door) {
            known.push(door);
        }
    }
    let (left, right) = if front { ("TV", "TH") } else { ("TH", "TV") };
    let number = if entrance.number.is_empty() { "(nummer ukendt)" } else { entrance.number.as_str() };
    let mut text = if entrance.number.is_empty() { format!("Opgang {number}") } else { number.to_owned() };
    if known.iter().any(|door| door == left) {
        text = format!("{left}{separator}{text}");
    }
    if known.iter().any(|door| door == right) {
        text.push_str(&format!("{separator}{right}"));
    }
    let others: Vec<_> = known.iter().filter(|door| !matches!(door.as_str(), "TV" | "TH")).cloned().collect();
    if !others.is_empty() {
        text.push_str(&format!(" · {}", others.join(" / ")));
    }
    let mut details = known;
    if unknown > 0 {
        details.push(format!("{unknown} ukendt dørbetegnelse"));
    }
    if details.is_empty() {
        details.push("ingen dørbetegnelser registreret".into());
    }
    crate::sheet::EntranceLabel {
        source: crate::sheet::TextSource { kind: "entrance", id: format!("{building}.entrance{index}"), role: "unit_register" },
        at: entrance.point,
        deg: 0.0,
        outward: [0.0, 0.0],
        text,
        legend: format!("Opgang {number} — {}", details.join(" / ")),
        unit_records: units.len(),
        unknown_door_units: unknown,
        placement_note: None,
    }
}

/// Repeat entrance placement after a saved template establishes the physical scale and crop.
/// Original inputs survive this pass, so default-scale layout is not compounded on each reflow.
pub(crate) fn layout_entrance_labels(sheet: &mut crate::sheet::Sheet, crop: Option<[f64; 4]>) -> Result<()> {
    let old_note = sheet.metrics.get("entrance_labels").and_then(|coverage| coverage["note"].as_str()).map(String::from);
    let coverage = layout_entrance_scene(&mut sheet.scene, &sheet.vp, sheet.clip, crop)?;
    if let Some(old) = old_note {
        sheet.note = sheet.note.take().and_then(|note| {
            let parts: Vec<_> = note.split(" · ").filter(|part| *part != old).collect();
            (!parts.is_empty()).then(|| parts.join(" · "))
        });
    }
    if let Some(note) = coverage["note"].as_str() {
        sheet.note = Some(match sheet.note.take() {
            Some(old) => format!("{old} · {note}"),
            None => note.into(),
        });
    }
    sheet.metrics["entrance_labels"] = coverage;
    Ok(())
}

/// Filled wall rings in sheet pixels, with cheap bounds before exact material tests.
struct EntranceWall {
    bounds: [f64; 4],
    rings: Vec<Vec<[f64; 2]>>,
}

fn entrance_bounds(points: &[[f64; 2]]) -> [f64; 4] {
    points.iter().fold([f64::INFINITY, f64::INFINITY, f64::NEG_INFINITY, f64::NEG_INFINITY], |b, p| {
        [b[0].min(p[0]), b[1].min(p[1]), b[2].max(p[0]), b[3].max(p[1])]
    })
}

fn entrance_boxes_overlap(a: &[[f64; 2]], b: &[[f64; 2]]) -> bool {
    for poly in [a, b] {
        for index in 0..poly.len() {
            let (p, q) = (poly[index], poly[(index + 1) % poly.len()]);
            let axis = [q[1] - p[1], p[0] - q[0]];
            let span = |poly: &[[f64; 2]]| {
                poly.iter()
                    .map(|p| p[0] * axis[0] + p[1] * axis[1])
                    .fold((f64::INFINITY, f64::NEG_INFINITY), |(lo, hi), value| (lo.min(value), hi.max(value)))
            };
            let ((a0, a1), (b0, b1)) = (span(a), span(b));
            if a1 <= b0 || b1 <= a0 {
                return false;
            }
        }
    }
    true
}

fn entrance_corners(text: &Text, mut bounds: [f64; 4], vp: &Viewport, padding: f64) -> [[f64; 2]; 4] {
    if text.halo == Halo::Box {
        // Match the exported white box, while retaining any glyph ink extending beyond it.
        let width = 0.56 * text.size * text.text.chars().count() as f64 + 6.0 * MM_PER_PX;
        let left = match text.anchor {
            crate::sheet::Anchor::Start => -3.0 * MM_PER_PX,
            crate::sheet::Anchor::Middle => -width / 2.0,
            crate::sheet::Anchor::End => -width + 3.0 * MM_PER_PX,
        };
        let bottom = if text.middle { -0.62 * text.size } else { -0.34 * text.size };
        bounds = [bounds[0].min(left), bounds[1].min(bottom), bounds[2].max(left + width), bounds[3].max(bottom + 1.24 * text.size)];
    }
    let pad = padding + if text.halo == Halo::Outline { 2.0 * MM_PER_PX } else { 0.0 };
    let b = [bounds[0] - pad, bounds[1] - pad, bounds[2] + pad, bounds[3] + pad];
    let at = vp.px(text.at);
    let (sin, cos) = text.deg.to_radians().sin_cos();
    [[b[0], b[1]], [b[2], b[1]], [b[2], b[3]], [b[0], b[3]]]
        .map(|[x, y]| [at[0] + (x * cos - y * sin) / MM_PER_PX, at[1] - (x * sin + y * cos) / MM_PER_PX])
}

fn entrance_hits_wall(corners: &[[f64; 2]; 4], wall: &EntranceWall, budget: &mut usize) -> bool {
    let b = entrance_bounds(corners);
    if b[2] <= wall.bounds[0] || b[0] >= wall.bounds[2] || b[3] <= wall.bounds[1] || b[1] >= wall.bounds[3] {
        return false;
    }
    let cost = wall.rings.iter().map(Vec::len).fold(0usize, usize::saturating_add).saturating_mul(5);
    if *budget < cost {
        *budget = 0;
        return true;
    }
    *budget -= cost;
    if corners.iter().any(|p| wall.rings.iter().filter(|ring| crate::drawings::inside(ring, *p)).count() % 2 == 1) {
        return true;
    }
    let cross = |a: [f64; 2], b: [f64; 2], p: [f64; 2]| (b[0] - a[0]) * (p[1] - a[1]) - (b[1] - a[1]) * (p[0] - a[0]);
    for ring in &wall.rings {
        if ring.iter().any(|p| crate::drawings::inside(corners, *p)) {
            return true;
        }
        for index in 0..ring.len() {
            if *budget < 4 {
                *budget = 0;
                return true;
            }
            *budget -= 4;
            let (a, b) = (ring[index], ring[(index + 1) % ring.len()]);
            for edge in 0..4 {
                let (p, q) = (corners[edge], corners[(edge + 1) % 4]);
                let ab = entrance_bounds(&[a, b]);
                let pq = entrance_bounds(&[p, q]);
                if ab[2] < pq[0] || ab[0] > pq[2] || ab[3] < pq[1] || ab[1] > pq[3] {
                    continue;
                }
                if cross(a, b, p) * cross(a, b, q) <= 0.0 && cross(p, q, a) * cross(p, q, b) <= 0.0 {
                    return true;
                }
            }
        }
    }
    false
}

fn entrance_fits(corners: &[[f64; 2]; 4], clip: [f64; 4], occupied: &[[[f64; 2]; 4]], walls: &[EntranceWall], budget: &mut usize) -> bool {
    let cost = occupied.len().saturating_mul(8).saturating_add(1);
    if *budget < cost {
        *budget = 0;
        return false;
    }
    *budget -= cost;
    corners.iter().all(|p| p[0] >= clip[0] && p[0] <= clip[2] && p[1] >= clip[1] && p[1] <= clip[3])
        && !occupied.iter().any(|other| entrance_boxes_overlap(corners, other))
        && !walls.iter().any(|wall| entrance_hits_wall(corners, wall, budget))
}

fn layout_entrance_scene(scene: &mut Scene, vp: &Viewport, area: [f64; 4], crop: Option<[f64; 4]>) -> Result<Value> {
    use crate::sheet::Item;
    let mut texts: Vec<_> = scene
        .items
        .iter()
        .enumerate()
        .filter_map(|(index, item)| match item {
            Item::Text(text) => Some((index, text.clone())),
            _ => None,
        })
        .collect();
    let entrances: Vec<_> = texts.iter().enumerate().filter_map(|(index, (_, text))| text.entrance.as_ref().map(|_| index)).collect();
    if entrances.is_empty() {
        return Ok(json!({"requested":0,"placed":0,"alternate":0,"legend":0,"unplaced":0,"records":[],"note":null}));
    }
    if texts.len() > 2000 {
        return Err(ModelError::Invalid("entrance lettering exceeds 2000 labels; use a smaller drawing scope".into()).into());
    }
    for (_, text) in &mut texts {
        if let Some(input) = &text.entrance {
            text.at = input.at;
            text.deg = input.deg;
            text.text.clone_from(&input.text);
        }
    }
    let mut measured: Vec<_> = texts.iter().map(|(_, text)| text.clone()).collect();
    for index in &entrances {
        let mut text = texts[*index].1.clone();
        if let Some(input) = &text.entrance {
            text.text.clone_from(&input.legend);
        }
        text.deg = 0.0;
        measured.push(text);
    }
    let ink = crate::plan_output::text_ink_bounds(&measured)?;
    let mut clip = [area[0], area[1], area[0] + area[2], area[1] + area[3]];
    if let Some(crop) = crop {
        let lo = vp.px([crop[0], crop[3]]);
        let hi = vp.px([crop[2], crop[1]]);
        clip = [clip[0].max(lo[0]), clip[1].max(lo[1]), clip[2].min(hi[0]), clip[3].min(hi[1])];
    }
    let mut occupied: Vec<_> = texts
        .iter()
        .zip(&ink)
        .filter_map(|((_, text), bounds)| {
            if text.entrance.is_some() {
                return None;
            }
            bounds.map(|bounds| entrance_corners(text, bounds, vp, 0.0))
        })
        .collect();
    let walls: Vec<_> = scene
        .items
        .iter()
        .filter_map(|item| {
            let Item::Path(path) = item else { return None };
            if path.layer != Layer::WallCut || path.fill.is_none() || !path.closed {
                return None;
            }
            let rings: Vec<Vec<_>> = path.rings.iter().map(|ring| ring.iter().map(|p| vp.px(*p)).collect()).collect();
            let points: Vec<_> = rings.iter().flatten().copied().collect();
            Some(EntranceWall { bounds: entrance_bounds(&points), rings })
        })
        .collect();
    let geometry = scene.bounds().map(|bounds| {
        let a = vp.px([bounds[0], bounds[3]]);
        let b = vp.px([bounds[2], bounds[1]]);
        [[a[0], a[1]], [b[0], a[1]], [b[0], b[1]], [a[0], b[1]]]
    });
    let mut records = Vec::new();
    let mut placed = 0;
    let mut alternate = 0;
    let mut legends = 0;
    // Bound all alternate/legend material comparisons; exhaustion is explicit coverage feedback.
    let mut budget = 2_000_000usize;
    for (rank, index) in entrances.iter().copied().enumerate() {
        let (scene_index, mut text) = texts[index].clone();
        let Some(input) = text.entrance.clone() else { continue };
        let mut state = "unplaced";
        let mut reason = "complete_glyph_halo_cannot_fit";
        let mut chosen = None;
        if let Some(bounds) = ink[index]
            && input.placement_note.is_none()
        {
            let origin = vp.px(input.at);
            let q = vp.px([input.at[0] + input.outward[0], input.at[1] + input.outward[1]]);
            let length = (q[0] - origin[0]).hypot(q[1] - origin[1]);
            let direction = if length > 1e-9 { [(q[0] - origin[0]) / length, (q[1] - origin[1]) / length] } else { [0.0, 0.0] };
            let corners = entrance_corners(&text, bounds, vp, 0.5);
            let span = corners
                .iter()
                .map(|p| p[0] * direction[0] + p[1] * direction[1])
                .fold((f64::INFINITY, f64::NEG_INFINITY), |(a, b), v| (a.min(v), b.max(v)));
            let step = (span.1 - span.0 + 0.5 / MM_PER_PX).max(1.0);
            for row in 0..=8 {
                if row > 0 && length <= 1e-9 {
                    break;
                }
                text.at = vp.at([origin[0] + direction[0] * step * row as f64, origin[1] + direction[1] * step * row as f64]);
                let corners = entrance_corners(&text, bounds, vp, 0.5);
                if entrance_fits(&corners, clip, &occupied, &walls, &mut budget) {
                    chosen = Some(corners);
                    state = if row == 0 {
                        "preferred"
                    } else {
                        alternate += 1;
                        "alternate"
                    };
                    break;
                }
            }
        } else if ink[index].is_none() {
            reason = "no_shaped_glyph_ink";
        }
        if chosen.is_none()
            && let Some(bounds) = ink[texts.len() + rank]
        {
            text.text.clone_from(&input.legend);
            text.deg = 0.0;
            text.at = vp.at([0.0, 0.0]);
            let local = entrance_bounds(&entrance_corners(&text, bounds, vp, 0.5));
            let step = [(local[2] - local[0] + 1.0 / MM_PER_PX).max(1.0), (local[3] - local[1] + 1.0 / MM_PER_PX).max(1.0)];
            // At most 512 deterministic whitespace candidates; no font shrinking or clipping-as-fit.
            for slot in 0..512 {
                let columns = ((clip[2] - clip[0]) / step[0]).floor().max(1.0) as usize;
                let at = [clip[0] - local[0] + (slot % columns) as f64 * step[0], clip[1] - local[1] + (slot / columns) as f64 * step[1]];
                if at[1] + local[3] > clip[3] || budget == 0 {
                    break;
                }
                text.at = vp.at(at);
                let corners = entrance_corners(&text, bounds, vp, 0.5);
                if geometry.as_ref().is_some_and(|geometry| entrance_boxes_overlap(&corners, geometry)) {
                    continue;
                }
                if entrance_fits(&corners, clip, &occupied, &walls, &mut budget) {
                    chosen = Some(corners);
                    state = "legend";
                    legends += 1;
                    break;
                }
            }
        }
        if let Some(corners) = chosen {
            occupied.push(corners);
            placed += 1;
        } else {
            if budget == 0 {
                reason = "lettering_comparison_budget_exceeded";
            }
            // Retain complete source text in the audit scene even when no complete box fits.
            text.text.clone_from(&input.text);
            text.deg = input.deg;
            // Keep provenance without inventing a new placement on the crop edge.
            // Ordinary drawing clipping excludes labels outside a detailed view.
            text.at = input.at;
        }
        records.push(json!({"source":input.source,"text":input.text,"legend_text":input.legend,
            "drawn_text":text.text,"state":state,"reason":if state=="unplaced" {Some(reason)} else {None},
            "at_mm":text.at,"rotation_deg":text.deg,"unit_records":input.unit_records,
            "unknown_door_units":input.unknown_door_units,"placement_note":input.placement_note,
            "inspect_commands":[{"command":"entrance.get","params":{"id":input.source.id}}]}));
        scene.items[scene_index] = Item::Text(text);
    }
    let unplaced = entrances.len() - placed;
    Ok(json!({"requested":entrances.len(),"placed":placed,"alternate":alternate,"legend":legends,"unplaced":unplaced,
        "records":records,"note":(unplaced>0).then(||format!("Opgange: {unplaced} mærkninger kræver placering"))}))
}

/// A sheet label in pixels: centre, rotation (degrees, clockwise) and font size.
#[derive(Clone)]
pub(crate) struct Label {
    pub(crate) at: [f64; 2],
    pub(crate) alt: Option<[f64; 2]>,
    pub(crate) deg: f64,
    pub(crate) fs: f64,
    pub(crate) text: String,
    pub(crate) balcony: bool,
    pub(crate) source: Option<crate::sheet::TextSource>,
    pub(crate) clearance: Option<crate::sheet::TextClearance>,
    pub(crate) entrance: Option<crate::sheet::EntranceLabel>,
}

impl Label {
    /// Corners of the text's box (Liberation Sans averages about 0.56 em per character), with a
    /// little air around it.
    fn corners(&self) -> [[f64; 2]; 4] {
        let (hw, hh) = (0.28 * self.fs * self.text.chars().count() as f64 + 4.0, 0.6 * self.fs + 2.0);
        let (s, c) = self.deg.to_radians().sin_cos();
        [[-hw, -hh], [hw, -hh], [hw, hh], [-hw, hh]].map(|[x, y]| [self.at[0] + x * c - y * s, self.at[1] + x * s + y * c])
    }

    /// Separating-axis test of the two rotated boxes.
    pub(crate) fn overlaps(&self, other: &Label) -> bool {
        let (a, b) = (self.corners(), other.corners());
        let axes = [
            [a[1][0] - a[0][0], a[1][1] - a[0][1]],
            [a[3][0] - a[0][0], a[3][1] - a[0][1]],
            [b[1][0] - b[0][0], b[1][1] - b[0][1]],
            [b[3][0] - b[0][0], b[3][1] - b[0][1]],
        ];
        axes.iter().all(|ax| {
            let span = |r: &[[f64; 2]; 4]| {
                r.iter().map(|p| p[0] * ax[0] + p[1] * ax[1]).fold((f64::INFINITY, f64::NEG_INFINITY), |(lo, hi), v| (lo.min(v), hi.max(v)))
            };
            let ((a0, a1), (b0, b1)) = (span(&a), span(&b));
            a0 < b1 && b0 < a1
        })
    }
}

fn plan_extents(doc: &Document, level: usize, view: View) -> Result<Vec<[f64; 2]>> {
    let mut points: Vec<_> = doc.buildings.iter().filter(|b| b.storeys.get(level).is_some()).flat_map(|b| b.footprint.iter().copied()).collect();
    if includes_site(doc, level) {
        for o in trigon_model::site::build(doc, view, None)? {
            points.extend(o.footprint);
        }
    }
    Ok(points)
}

/// A composed landscape sheet. Its crop and rotation affect the viewport only, never the model.
fn sheet_frame(doc: &Document, region: Option<&[[f64; 2]]>, rotation: f64, level: usize, view: View) -> Result<Frame> {
    trigon_model::checked("rotate_deg", rotation, -360.0, 360.0)?;
    let points = match region {
        Some(r) => region_corners(r),
        None => plan_extents(doc, level, view)?,
    };
    if points.len() < 2 || points.len() > 10_000 || points.iter().flatten().any(|x| !x.is_finite() || x.abs() > 1e8) {
        return Err(ModelError::Invalid("sheet region needs 2 to 10000 finite site points".into()).into());
    }
    let t = rotation.to_radians();
    let mut f = Frame { c: [0.0; 2], cos: t.cos(), sin: t.sin(), x0: 0.0, y1: 0.0, mm: 1.0, w: 1600, h: 900 };
    let mut lo = [f64::INFINITY; 2];
    let mut hi = [f64::NEG_INFINITY; 2];
    for p in points {
        let r = f.rot(p);
        for k in 0..2 {
            lo[k] = lo[k].min(r[k]);
            hi[k] = hi[k].max(r[k]);
        }
    }
    if hi[0] - lo[0] < 100.0 || hi[1] - lo[1] < 100.0 {
        return Err(ModelError::Invalid("sheet region needs a nonzero width and height".into()).into());
    }
    let margin = if region.is_some() { 0.0 } else { 2000.0 };
    let fit = ((hi[0] - lo[0] + 2.0 * margin) / 1440.0).max((hi[1] - lo[1] + 2.0 * margin) / 650.0);
    // At a standard scale when one fits well, so the sheet can say which.
    f.mm = sheet_scale(fit / MM_PER_PX) * MM_PER_PX;
    f.x0 = (lo[0] + hi[0]) / 2.0 - 800.0 * f.mm;
    f.y1 = (lo[1] + hi[1]) / 2.0 + 465.0 * f.mm;
    Ok(f)
}

/// The layer a presentation plan line from `a` to `b` belongs to, from the widths `scene` draws
/// each family with: door leaves 45 (and 30 inside the building: interior doors), balcony outlines
/// 30, window lines 25 and 15, rails and decks 14, boards 7, dimension lines 10 and ticks 18.
fn line_layer(doc: &Document, width: f64, a: [f64; 2], b: [f64; 2]) -> Layer {
    let mid = [(a[0] + b[0]) / 2.0, (a[1] + b[1]) / 2.0];
    // Well inside a footprint, away from its facades (where balconies hang).
    let indoors = || {
        doc.buildings.iter().any(|bd| {
            let ring = &bd.footprint;
            crate::drawings::inside(ring, mid)
                && (0..ring.len()).all(|i| {
                    let (p, q) = (ring[i], ring[(i + 1) % ring.len()]);
                    let d = [q[0] - p[0], q[1] - p[1]];
                    let t = (((mid[0] - p[0]) * d[0] + (mid[1] - p[1]) * d[1]) / (d[0] * d[0] + d[1] * d[1]).max(1e-12)).clamp(0.0, 1.0);
                    (mid[0] - p[0] - t * d[0]).hypot(mid[1] - p[1] - t * d[1]) > 150.0
                })
        })
    };
    match width.round() as i64 {
        40.. => Layer::Door,
        28..=39 if indoors() => Layer::Door,
        28..=39 => Layer::Balcony,
        20..=27 | 15 | 16 => Layer::Glazing,
        13 | 14 | ..=9 => Layer::Balcony,
        _ => Layer::Annotation,
    }
}

/// A presentation plan: the scene in drawing mm (site mm turned by the sheet rotation about the
/// site origin; unturned, site coordinates), and how it sits on the sheet.
struct PlanSheet {
    scene: Scene,
    vp: Viewport,
    /// Sheet px `[x, y, w, h]`.
    clip: [f64; 4],
    floor: String,
    project: String,
    caption: String,
    note: Option<String>,
    entrance_labels: Value,
}

/// A level-index plan can include or crop any building at that level. Retain the
/// legacy name only when one building supplies it; multi-building sheets use an
/// explicit caption instead of misidentifying the first building as the project.
pub(crate) fn project_name(doc: &Document, level: usize) -> String {
    let mut sources = doc.buildings.iter().filter(|b| b.storeys.get(level).is_some());
    let source = sources.next();
    if sources.next().is_none() { source.map(|b| b.name.clone()).unwrap_or_default() } else { String::new() }
}

fn sheet_scene(doc: &Document, level: usize, view: View, rotation: f64, region: Option<&[[f64; 2]]>, caption: &str) -> Result<PlanSheet> {
    let has_roof_floor = doc.buildings.iter().any(|b| b.attic_level() == Some(level));
    if has_roof_floor && (rotation != 0.0 || region.is_some()) {
        return Err(ModelError::Invalid(
            "roof floor plans use actual mesh cuts; use a saved plan view/template for crops; rotation is unsupported".into(),
        )
        .into());
    }
    // Keep the existing actual-cut route when this index has only roof floors. In a mixed
    // sheet, full storeys retain their plan symbols, including stair arrivals from below.
    if has_roof_floor && !doc.buildings.iter().any(|b| b.storeys.get(level).is_some_and(|s| !s.roof_level)) {
        let sheet = crate::plan_range::sheet(doc, level, view, Default::default())?;
        let caption = if caption.is_empty() { sheet.project.clone() } else { caption.into() };
        let note = Some(match sheet.note {
            Some(note) => format!("{note} · Roof-clipped interiors and habitable areas unsupported"),
            None => "Roof-clipped interiors and habitable areas unsupported".into(),
        });
        return Ok(PlanSheet {
            scene: sheet.scene,
            vp: sheet.vp,
            clip: sheet.clip,
            floor: sheet.title,
            project: sheet.project,
            caption,
            note,
            entrance_labels: json!(null),
        });
    }
    let (prims, _, elements) = scene(doc, level, view, true)?;
    let f = sheet_frame(doc, region, rotation, level, view)?;
    let vp = Viewport { x0: f.x0, y1: f.y1, mm: f.mm };
    let floor = doc.level_label(level).unwrap_or_default();
    let project = project_name(doc, level);
    let caption = if caption.is_empty() { project.clone() } else { caption.to_string() };
    let clip = if let Some(region) = region {
        let mut lo = [f64::INFINITY; 2];
        let mut hi = [f64::NEG_INFINITY; 2];
        for p in region_corners(region) {
            let q = f.px(p);
            for k in 0..2 {
                lo[k] = lo[k].min(q[k]);
                hi[k] = hi[k].max(q[k]);
            }
        }
        let x = lo[0].max(70.0);
        let y = lo[1].max(130.0);
        [x, y, hi[0].min(1530.0) - x, hi[1].min(800.0) - y]
    } else {
        [70.0, 130.0, 1460.0, 670.0]
    };
    let mut sc = Scene::new(vp.scale());
    for (index, p) in prims.iter().enumerate() {
        if let Prim::Site { pts, fill } = p {
            let points: Vec<_> = pts.iter().map(|p| f.rot(*p)).collect();
            sc.polygon(Layer::Ground, vec![points.clone()], Some(Fill::Solid(*fill)), Some(Stroke { colour: [80, 85, 75], ..Stroke::new(0.18) }));
            if let Some(element) = elements.get(&index) {
                let sources: Vec<_> = (0..points.len()).map(|i| element.segment([points[i], points[(i + 1) % points.len()]])).collect();
                sc.attribute_last(&sources);
                sc.element_regions.push(crate::views::DrawingElementRegion {
                    id: element.id.clone(),
                    kind: element.kind,
                    rings: vec![points],
                    depth_plane: None,
                });
            }
        }
    }
    // Union each family before stroking: no rectangles' internal seams or doubled junction edges.
    for surface in [Surface::Stair, Surface::Interior, Surface::Exterior] {
        let polys: Vec<_> = prims
            .iter()
            .filter_map(|p| match p {
                Prim::Poly { pts, surface: s, .. } if *s == surface => Some(pts.clone()),
                _ => None,
            })
            .collect();
        let mut sources = Vec::new();
        for (index, primitive) in prims.iter().enumerate() {
            if let Prim::Poly { pts, surface: family, .. } = primitive
                && *family == surface
                && let Some(element) = elements.get(&index)
            {
                sc.element_regions.push(crate::views::DrawingElementRegion {
                    id: element.id.clone(),
                    kind: element.kind,
                    rings: vec![pts.iter().map(|p| f.rot(*p)).collect()],
                    depth_plane: None,
                });
                for corner in 0..pts.len() {
                    sources.push(element.segment([f.rot(pts[corner]), f.rot(pts[(corner + 1) % pts.len()])]));
                }
            }
        }
        let (layer, fill, pen) = match surface {
            Surface::Exterior => (Layer::WallCut, Fill::Masonry, 0.35),
            Surface::Interior => (Layer::WallCut, Fill::Solid([0xd6, 0xd6, 0xd6]), 0.35),
            Surface::Stair => (Layer::Stair, Fill::Solid([250, 250, 250]), 0.18),
        };
        for shape in trigon_model::footprint::union(&polys)? {
            let rings = shape.iter().map(|ring| ring.iter().map(|p| f.rot(*p)).collect()).collect();
            sc.polygon(layer, rings, Some(fill), Some(Stroke { pen, colour: [0x33, 0x33, 0x33], dashed: false, ..Stroke::new(pen) }));
            sc.attribute_last(&sources);
        }
    }
    let mut labels: Vec<Label> = Vec::new();
    for (index, p) in prims.iter().enumerate() {
        match p {
            Prim::PickRegion(region) => {
                sc.element_regions.push(crate::views::DrawingElementRegion {
                    rings: region.rings.iter().map(|ring| ring.iter().map(|p| f.rot(*p)).collect()).collect(),
                    ..region.clone()
                });
            }
            Prim::Poly { .. } | Prim::Site { .. } => {}
            Prim::StairLine { a, b, width, dashed } => {
                let pen = if *width >= 14.0 { 0.18 } else { 0.13 };
                sc.line(Layer::Stair, f.rot(*a), f.rot(*b), Stroke { pen, colour: [0x33; 3], dashed: *dashed, ..Stroke::new(pen) });
                if let Some(element) = elements.get(&index) {
                    sc.attribute_last(&[element.segment([f.rot(*a), f.rot(*b)])]);
                }
            }
            Prim::Line { a, b, width, dashed, color } => {
                let colour = if *dashed || *color == DEMOLISHED { [0xbd, 0x30, 0x30] } else { [0x33, 0x33, 0x33] };
                let pen = if *width >= 25.0 {
                    0.25
                } else if *width >= 14.0 {
                    0.18
                } else {
                    0.13
                };
                sc.line(line_layer(doc, *width, *a, *b), f.rot(*a), f.rot(*b), Stroke { pen, colour, dashed: *dashed, ..Stroke::new(pen) });
                if let Some(element) = elements.get(&index) {
                    sc.attribute_last(&[element.segment([f.rot(*a), f.rot(*b)])]);
                }
            }
            Prim::Text { at, text, size, angle, away, entrance, .. } => {
                let p = f.px(*at);
                if entrance.is_none() && (!(80.0..=1520.0).contains(&p[0]) || !(140.0..=790.0).contains(&p[1])) {
                    continue;
                }
                let mut deg = angle.map_or(0.0, |a| -(a.to_degrees() + rotation)).rem_euclid(360.0);
                if deg > 90.0 && deg < 270.0 {
                    deg += 180.0;
                }
                let fs = if *size >= 380.0 { 18.0 } else { 14.0 };
                // One text line further out, for a label that collides where it is.
                let alt = away.and_then(|d| {
                    let q = f.px([at[0] + d[0] * 1000.0, at[1] + d[1] * 1000.0]);
                    let l = (q[0] - p[0]).hypot(q[1] - p[1]);
                    (l > 1e-6).then(|| [p[0] + (q[0] - p[0]) / l * fs * 1.7, p[1] + (q[1] - p[1]) / l * fs * 1.7])
                });
                let clearance = away
                    .filter(|d| {
                        let anchor = [at[0] - d[0] * 150.0, at[1] - d[1] * 150.0];
                        text.starts_with("Ca. ")
                            && prims.iter().any(|p| {
                                matches!(p,
                        Prim::Line { a,b,width,.. } if *width == 10.0 &&
                        ((a[0]+b[0])/2.0-anchor[0]).hypot((a[1]+b[1])/2.0-anchor[1]) < 1e-4)
                            })
                    })
                    .map(|d| {
                        let anchor = f.rot([at[0] - d[0] * 150.0, at[1] - d[1] * 150.0]);
                        let start = f.rot(*at);
                        let end = f.rot([at[0] + d[0], at[1] + d[1]]);
                        crate::sheet::TextClearance { anchor, outward: [end[0] - start[0], end[1] - start[1]] }
                    });
                labels.push(Label {
                    at: p,
                    alt,
                    deg,
                    fs,
                    text: text.replace(" x ", " × "),
                    balcony: text.starts_with("Ca. "),
                    source: entrance.as_ref().map(|tag| tag.source.clone()).or_else(|| {
                        elements.get(&index).map(|e| crate::sheet::TextSource { kind: "element", id: e.id.clone(), role: "balcony_size" })
                    }),
                    clearance,
                    entrance: entrance.as_ref().map(|input| {
                        let mut tag = input.clone();
                        tag.at = f.rot(*at);
                        tag.deg = -deg;
                        tag.outward = away.map_or([0.0, 0.0], |d| [f.cos * d[0] - f.sin * d[1], f.sin * d[0] + f.cos * d[1]]);
                        tag
                    }),
                });
            }
        }
    }
    // Retain every entrance input for actual-glyph placement below. Balcony sizes retain their
    // legacy collision/summary layout; an entrance is never discarded by this approximate pass.
    labels.sort_by_key(|l| l.balcony);
    let mut sizes: Vec<(String, usize)> = Vec::new();
    for l in labels.iter().filter(|l| l.balcony) {
        match sizes.iter_mut().find(|(t, _)| *t == l.text) {
            Some((_, n)) => *n += 1,
            None => sizes.push((l.text.clone(), 1)),
        }
    }
    let mut placed: Vec<Label> = Vec::new();
    let mut crowded = false;
    for l in labels {
        let fits = |l: &Label| !placed.iter().any(|q| q.overlaps(l));
        if l.entrance.is_some() || fits(&l) {
            placed.push(l);
            continue;
        }
        match l.alt {
            Some(at) if fits(&Label { at, alt: None, ..l.clone() }) => placed.push(Label { at, alt: None, ..l }),
            _ => crowded |= l.balcony,
        }
    }
    for l in placed.iter().filter(|l| !(crowded && l.balcony)) {
        let mut t = Text::new(Layer::Annotation, vp.at(l.at), l.text.clone(), l.fs * MM_PER_PX);
        t.deg = -l.deg;
        t.halo = Halo::Outline;
        t.source = l.source.clone();
        t.clearance = l.clearance.clone();
        t.entrance = l.entrance.clone().map(Box::new);
        sc.text(t);
    }
    let mut notes = Vec::new();
    if crowded {
        notes.push(format!("Altaner: {}", sizes.iter().map(|(t, n)| format!("{n} stk. {t}")).collect::<Vec<_>>().join(", ")));
    }
    if has_roof_floor {
        // Both scenes use site mm. Keep the shared frame/scale and append only each roof
        // building's geometry, cut relative to its own floor, with original source ownership.
        for building in doc.buildings.iter().filter(|b| b.attic_level() == Some(level)) {
            let mut roof = crate::plan_range::sheet_for_building(doc, level, view, Default::default(), &building.id)?;
            sc.items.append(&mut roof.scene.items);
            sc.element_regions.append(&mut roof.scene.element_regions);
            if let Some(note) = roof.note
                && !notes.contains(&note)
            {
                notes.push(note);
            }
        }
        notes.push("Roof-clipped interiors and habitable areas unsupported".into());
    }
    let entrance_labels = layout_entrance_scene(&mut sc, &vp, clip, None)?;
    if let Some(note) = entrance_labels["note"].as_str() {
        notes.push(note.into());
    }
    let note = (!notes.is_empty()).then(|| notes.join(" · "));
    Ok(PlanSheet { scene: sc, vp, clip, floor, project, caption, note, entrance_labels })
}

/// A presentation drawing set: vector PDF pages, or one matching SVG/PNG sheet or DXF drawing.
pub(crate) fn sheet(
    doc: &Document,
    levels: &[usize],
    view: View,
    rotation: f64,
    region: Option<&[[f64; 2]]>,
    caption: &str,
    path: PathBuf,
) -> Result<Value> {
    if levels.is_empty() || levels.len() > 100 || caption.len() > 500 {
        return Err(ModelError::Invalid("a drawing set needs 1 to 100 levels and a caption of at most 500 bytes".into()).into());
    }
    let ext = path.extension().and_then(|s| s.to_str()).unwrap_or_default();
    if !matches!(ext, "png" | "svg" | "pdf" | "dxf") || (ext != "pdf" && levels.len() != 1) {
        return Err(ModelError::Invalid("use .pdf for a drawing set, or .png/.svg/.dxf for a single sheet".into()).into());
    }
    let sheets: Vec<PlanSheet> = levels.iter().map(|level| sheet_scene(doc, *level, view, rotation, region, caption)).collect::<Result<_>>()?;
    let pages: Vec<String> = sheets
        .iter()
        .filter(|_| ext != "dxf")
        .map(|s| {
            let frame = svg::Frame { title: &s.floor, project: &s.project, caption: &s.caption, footer: &s.floor, note: s.note.clone() };
            svg::sheet(&frame, &s.scene, &s.vp, s.clip)
        })
        .collect();
    let mut result = json!({"written": path, "pages": levels.len(), "width_px": 1600, "height_px": 900, "underlay": null, "style": "presentation"});
    let bytes = match (ext, sheets.first(), pages.first()) {
        ("pdf", _, _) => crate::plan_output::pdf(&pages, PAGE_MM)?,
        ("png", _, Some(page)) => crate::plan_output::png(page)?,
        ("svg", _, Some(page)) => page.as_bytes().to_vec(),
        ("dxf", Some(s), _) => {
            let text = dxf::write(&s.scene, &dxf::TitleBlock { title: &s.floor, project: &s.project, caption: &s.caption, note: s.note.clone() });
            result["dxf"] = dxf::summary(&text);
            dxf::bytes(&text)
        }
        _ => return Err(EngineError::Io("nothing to write".into())),
    };
    write_output(&path, &bytes)?;
    if let Some(s) = sheets.first() {
        result["scale"] = json!(scale_text(s.scene.scale));
        result["entrance_labels"] = s.entrance_labels.clone();
    }
    if ext == "png" {
        result["image_path"] = json!(path);
    }
    Ok(result)
}

pub(crate) fn drawing_sheet(doc: &Document, level: usize, view: View) -> Result<crate::sheet::Sheet> {
    let s = sheet_scene(doc, level, view, 0.0, None, "")?;
    let metrics = json!({"pages":1, "width_px":1600, "height_px":900, "underlay":null, "style":"presentation", "scale":scale_text(s.scene.scale), "entrance_labels":s.entrance_labels});
    Ok(crate::sheet::Sheet { scene: s.scene, vp: s.vp, clip: s.clip, title: s.floor, project: s.project, caption: s.caption, note: s.note, metrics })
}

/// One level's presentation sheet as SVG text, for live views (Trigon Studio's plan view).
pub fn sheet_svg_text(doc: &Document, level: usize, view: View) -> Result<String> {
    let s = sheet_scene(doc, level, view, 0.0, None, "")?;
    let frame = svg::Frame { title: &s.floor, project: &s.project, caption: &s.caption, footer: &s.floor, note: s.note.clone() };
    Ok(svg::sheet(&frame, &s.scene, &s.vp, s.clip))
}

fn svg(prims: &[Prim], f: &Frame, background: Option<&[u8]>, title: &str) -> Result<String> {
    let mut out = String::new();
    let _ = writeln!(out, r#"<svg xmlns="http://www.w3.org/2000/svg" width="{w}" height="{h}" viewBox="0 0 {w} {h}">"#, w = f.w, h = f.h);
    let _ = writeln!(out, r#"<rect width="{}" height="{}" fill="white"/>"#, f.w, f.h);
    if let Some(bg) = background {
        let mut png = Vec::new();
        {
            let mut enc = png::Encoder::new(&mut png, f.w, f.h);
            enc.set_color(png::ColorType::Rgb);
            enc.set_depth(png::BitDepth::Eight);
            let mut wr = enc.write_header().map_err(|e| EngineError::Io(e.to_string()))?;
            wr.write_image_data(bg).map_err(|e| EngineError::Io(e.to_string()))?;
        }
        let _ = writeln!(out, r#"<image width="{}" height="{}" href="data:image/png;base64,{}"/>"#, f.w, f.h, crate::drawings::base64(&png));
    }
    for p in prims {
        match p {
            Prim::PickRegion(_) => {}
            Prim::Poly { pts, fill, .. } | Prim::Site { pts, fill } => {
                let d: Vec<String> = pts.iter().map(|p| f.px(*p)).map(|q| format!("{:.1},{:.1}", q[0], q[1])).collect();
                let _ = writeln!(out, r#"<polygon points="{}" fill="{}"/>"#, d.join(" "), hex(*fill));
            }
            Prim::Line { a, b, width, dashed, .. } | Prim::StairLine { a, b, width, dashed } => {
                let color = match p {
                    Prim::Line { color, .. } => *color,
                    _ => LINE,
                };
                let (pa, pb) = (f.px(*a), f.px(*b));
                let dash = if *dashed { r#" stroke-dasharray="6 6""# } else { "" };
                let _ = writeln!(
                    out,
                    r#"<line x1="{:.1}" y1="{:.1}" x2="{:.1}" y2="{:.1}" stroke="{}" stroke-width="{:.2}" stroke-linecap="round"{dash}/>"#,
                    pa[0],
                    pa[1],
                    pb[0],
                    pb[1],
                    hex(color),
                    (width / f.mm).max(0.6)
                );
            }
            Prim::Text { at, text, color, size, .. } => {
                let p = f.px(*at);
                let _ = writeln!(
                    out,
                    r#"<text x="{:.1}" y="{:.1}" font-family="Liberation Sans, Helvetica, Arial, sans-serif" font-size="{:.1}" fill="{}" text-anchor="middle" dominant-baseline="middle">{}</text>"#,
                    p[0],
                    p[1],
                    size / f.mm,
                    hex(*color),
                    text.replace('&', "&amp;").replace('<', "&lt;")
                );
            }
        }
    }
    let _ = writeln!(
        out,
        r#"<text x="20" y="40" font-family="Liberation Sans, Helvetica, Arial, sans-serif" font-size="28" fill="black">{}</text>"#,
        title.replace('&', "&amp;").replace('<', "&lt;")
    );
    out.push_str("</svg>\n");
    Ok(out)
}

pub(crate) fn render(
    s: &mut Session,
    level: usize,
    drawing: Option<&str>,
    show_scan: bool,
    view: View,
    mm: f64,
    rot_deg: f64,
    path: PathBuf,
) -> Result<Value> {
    let mm = trigon_model::checked("mm_per_px", mm, 5.0, 200.0)?;
    let rot_deg = trigon_model::checked("rotate_deg", rot_deg, -360.0, 360.0)?;
    if s.doc.buildings.iter().any(|b| b.attic_level() == Some(level)) {
        return Err(ModelError::Invalid(
            "roof floor plans need actual roof cuts; use plan.sheet or a saved ranged plan, without a scan overlay".into(),
        )
        .into());
    }
    let (mut prims, title, _) = scene(&s.doc, level, view, false)?;
    if show_scan {
        // Check views: the model in colour so it stands apart from the scan's greys.
        for p in &mut prims {
            match p {
                Prim::Poly { fill, .. } if *fill == WALL => *fill = [210, 20, 20],
                Prim::Poly { fill, .. } if *fill == PARTITION => *fill = [20, 90, 230],
                Prim::Line { color, .. } if *color == LINE => *color = [200, 0, 160],
                _ => {}
            }
        }
    }
    let f = frame_for(&s.doc, mm, rot_deg, level, view)?;
    let (rgb, used) = if show_scan { underlay(s, &f, level, drawing)? } else { (vec![255u8; (f.w * f.h * 3) as usize], None) };
    let ext = path.extension().and_then(|e| e.to_str()).unwrap_or_default().to_string();
    match ext.as_str() {
        "png" => {
            let text = svg(&prims, &f, used.as_ref().map(|_| rgb.as_slice()), &title)?;
            let bytes = crate::plan_output::png(&text)?;
            write_output(&path, &bytes)?;
        }
        "svg" => {
            let text = svg(&prims, &f, used.as_ref().map(|_| rgb.as_slice()), &title)?;
            write_output(&path, text.as_bytes())?;
        }
        _ => return Err(EngineError::Io(format!("{}: the file name must end in .png or .svg", path.display()))),
    }
    let balconies = prims.iter().filter(|p| matches!(p, Prim::Text { text, .. } if text.starts_with("Ca. "))).count();
    let mut out = json!({
        "title": title,
        "underlay": used,
        "width_px": f.w,
        "height_px": f.h,
        "mm_per_px": mm,
        "labelled_balconies": balconies,
        "legend": "black = load-bearing walls, grey = partitions; double lines = windows; single thick line = doors; rectangles = balconies (green new, red dashed demolished)",
    });
    if ext == "png" {
        out["image_path"] = json!(path);
    } else {
        out["written"] = json!(path);
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn mixed_plan_fixture() -> Session {
        let mut s = Session::new(Box::new(trigon_data::Live::new(trigon_data::Credentials::default())));
        for (command, params) in [
            (
                "building.add",
                json!({"id":"main","name":"Public main","footprint":[[0,0],[12000,0],[12000,8000],[0,8000]],"storeys":2,"storey_height_mm":3000}),
            ),
            (
                "interior.add_stairs",
                json!({"building":"main","stairs":[{"id":"main-stair","levels":[0],"kind":"u_turn","outline":[[3000,2000],[6000,2000],[6000,4200],[3000,4200]],"rounded":false,"landing":1100,"well":200}]}),
            ),
            ("view.define", json!({"id":"mixed","name":"Public plan","kind":{"view":"plan","level":1}})),
            ("view.configure", json!({"id":"mixed","template":{"name":"Fixed","sizing":{"mode":"fixed","denominator":100,"centre":[11500,4000]}}})),
        ] {
            s.execute(command, params).unwrap();
        }
        s
    }

    fn mixed_plan_add_roof_neighbour(s: &mut Session) {
        for (command, params) in [
            (
                "building.add",
                json!({"id":"rear","name":"Public lower neighbour","footprint":[[15000,0],[23000,0],[23000,8000],[15000,8000]],"storeys":1,"storey_height_mm":3000}),
            ),
            ("building.set_roof", json!({"building":"rear","kind":"pitched","pitch_deg":35})),
            ("building.set_attic", json!({"building":"rear","floor":2250})),
        ] {
            s.execute(command, params).unwrap();
        }
    }

    fn mixed_plan_owned_paths(scene: &Scene, owns: impl Fn(&str) -> bool) -> Vec<crate::sheet::Path> {
        scene
            .items
            .iter()
            .filter_map(|item| match item {
                crate::sheet::Item::Path(path) if path.element_segments.iter().any(|source| owns(&source.id)) => Some(path.clone()),
                _ => None,
            })
            .collect()
    }

    #[test]
    fn mixed_no_range_plans_preserve_full_storey_stair_arrivals_and_picking() {
        let mut s = mixed_plan_fixture();
        let original_main = s.doc.buildings[0].clone();
        assert_eq!(original_main.stairs[0].levels, vec![0], "there is no invented outgoing flight on selected level 1");
        let before = crate::views::sheet(&s.doc, "mixed", View::latest(&s.doc)).unwrap();
        let paths = mixed_plan_owned_paths(&before.scene, |id| id == "main-stair.L0");
        let regions: Vec<_> = before.scene.element_regions.iter().filter(|region| region.id == "main-stair.L0").cloned().collect();
        let symbol = trigon_model::stair::symbol(&original_main.stairs[0], original_main.storeys[0].height).unwrap();
        assert_eq!(symbol.treads.len(), 14);
        for (a, b) in &symbol.treads {
            assert!(
                paths.iter().any(|path| path.rings == vec![vec![*a, *b]] && path.stroke.is_some_and(|stroke| stroke.pen == 0.13 && !stroke.dashed))
            );
        }
        mixed_plan_add_roof_neighbour(&mut s);
        assert_eq!(s.doc.buildings[0], original_main);
        for reverse in [false, true] {
            if reverse {
                s.doc.buildings.swap(0, 1);
            }
            let source = s.doc.clone();
            let after = crate::views::sheet(&s.doc, "mixed", View::latest(&s.doc)).unwrap();
            assert_eq!(after.scene.scale, 100.0);
            assert_eq!(
                mixed_plan_owned_paths(&after.scene, |id| id == "main-stair.L0"),
                paths,
                "an unrelated attic must not replace a full-storey arrival symbol with depth-clipped mesh fragments"
            );
            assert_eq!(after.scene.element_regions.iter().filter(|region| region.id == "main-stair.L0").cloned().collect::<Vec<_>>(), regions);
            let drawing = crate::drawing_view(&s.doc, "mixed", View::latest(&s.doc)).unwrap();
            for point in [[4000.0, 2100.0], [4500.0, 3100.0]] {
                assert_eq!(drawing.pick_element(point, 2.0).unwrap().id, "main-stair.L0");
            }
            assert_eq!(s.doc, source, "rendering and picking never change source geometry or history");
        }
    }

    #[test]
    fn mixed_no_range_roof_components_match_their_single_building_actual_cut() {
        let mut s = mixed_plan_fixture();
        mixed_plan_add_roof_neighbour(&mut s);
        // This public fixture has no terrain, site objects, cross-building references or annotations.
        // Its independent single-building counterpart is safe as a test oracle; production keeps
        // the complete Document as validation/build context.
        let mut roof_only = s.doc.clone();
        roof_only.buildings.retain(|b| b.id == "rear");
        let mut reference = crate::plan_range::sheet(&roof_only, 1, View::latest(&roof_only), Default::default()).unwrap();
        assert_eq!(reference.metrics["floor_mm"], 2250.0);
        let only = drawing_sheet(&roof_only, 1, View::latest(&roof_only)).unwrap();
        assert_eq!(only.scene.items, reference.scene.items, "single-building roof plans keep their existing actual-cut route");
        let template = s.doc.views[0].template.as_ref().unwrap();
        let bounds = reference.scene.bounds();
        crate::view_config::prepare(&mut reference, template, bounds);
        crate::view_config::finish(&mut reference, template);
        let expected = mixed_plan_owned_paths(&reference.scene, |id| id.starts_with("rear."));
        assert!(!expected.is_empty());
        assert!(
            expected.iter().any(|path| path.layer == Layer::Roof && path.fill == Some(Fill::Solid([235; 3]))),
            "actual roof material cut styles remain present"
        );
        for reverse in [false, true] {
            if reverse {
                s.doc.buildings.swap(0, 1);
            }
            let original = s.doc.clone();
            let combined = crate::views::sheet(&s.doc, "mixed", View::latest(&s.doc)).unwrap();
            assert_eq!(
                mixed_plan_owned_paths(&combined.scene, |id| id.starts_with("rear.")),
                expected,
                "the attic is cut at its own floor, independent of the first building's datum; fill, stroke and ownership survive composition"
            );
            let preview = crate::drawing_view(&s.doc, "mixed", View::latest(&s.doc)).unwrap();
            for owned in &expected {
                for source in &owned.element_segments {
                    let point = [(source.segment[0][0] + source.segment[1][0]) / 2.0, (source.segment[0][1] + source.segment[1][1]) / 2.0];
                    assert!(
                        preview.pick_element(point, 0.01).is_some_and(|hit| hit.id.starts_with("rear.")),
                        "visible attic boundaries retain real owners"
                    );
                }
            }
            assert_eq!(s.doc, original);
        }
    }

    #[test]
    fn explicit_plan_ranges_remain_whole_view_actual_mesh_cuts() {
        let mut s = mixed_plan_fixture();
        mixed_plan_add_roof_neighbour(&mut s);
        let symbolic = crate::views::sheet(&s.doc, "mixed", View::latest(&s.doc)).unwrap();
        s.execute("view.configure", json!({"id":"mixed","template":{"name":"Explicit mesh range","range":{"top_mm":2300,"cut_mm":1200,"bottom_mm":0,"depth_mm":-1000},"sizing":{"mode":"fixed","denominator":100,"centre":[11500,4000]}}})).unwrap();
        let original = s.doc.clone();
        let ranged = crate::views::sheet(&s.doc, "mixed", View::latest(&s.doc)).unwrap();
        let template = s.doc.views[0].template.as_ref().unwrap();
        let mut expected = crate::plan_range::sheet(&s.doc, 1, View::latest(&s.doc), template.range.unwrap()).unwrap();
        let bounds = expected.scene.bounds();
        crate::view_config::prepare(&mut expected, template, bounds);
        crate::view_config::paper_clearances(&mut expected).unwrap();
        crate::view_config::finish(&mut expected, template);
        assert_eq!(ranged.scene.items, expected.scene.items, "Some(range) must still project all building meshes together");
        assert_eq!(ranged.scene.scale, expected.scene.scale);
        assert_ne!(
            mixed_plan_owned_paths(&symbolic.scene, |id| id == "main-stair.L0"),
            mixed_plan_owned_paths(&ranged.scene, |id| id == "main-stair.L0")
        );
        assert_eq!(s.doc, original);
    }

    #[test]
    fn mixed_no_range_plans_preserve_phase_filters_and_full_storey_opening_owners() {
        use trigon_model::build::PhaseFilter;
        let mut s = mixed_plan_fixture();
        s.execute("facade.set_bays", json!({"facade":"main.f1","count":2})).unwrap();
        s.execute("facade.place", json!({"facade":"main.f1","type":"window.casement.2","levels":[1],"bays":[0],"height":900,"sill":600})).unwrap();
        s.doc.phases.push(trigon_model::doc::Phase { id: "proposal".into(), name: "Proposal".into() });
        s.doc.active_phase = Some("proposal".into());
        let old = &mut s.doc.buildings[0].facades[0].placements[0];
        old.demolished = Some("proposal".into());
        let mut new = old.clone();
        new.bay = 1;
        new.created = Some("proposal".into());
        new.demolished = None;
        s.doc.buildings[0].facades[0].placements.push(new);
        let cases = [
            (PhaseFilter::Complete, false, true),
            (PhaseFilter::All, true, true),
            (PhaseFilter::Demolition, true, false),
            (PhaseFilter::New, false, true),
        ];
        let before: Vec<_> = cases
            .iter()
            .map(|(filter, _, _)| {
                mixed_plan_owned_paths(&crate::views::sheet(&s.doc, "mixed", View { phase: 1, filter: *filter }).unwrap().scene, |id| {
                    id.starts_with("main.f1.L1.B")
                })
            })
            .collect();
        mixed_plan_add_roof_neighbour(&mut s);
        let original = s.doc.clone();
        for ((filter, old_visible, new_visible), baseline) in cases.into_iter().zip(before) {
            let view = View { phase: 1, filter };
            let actual = crate::views::sheet(&s.doc, "mixed", view).unwrap();
            assert_eq!(
                mixed_plan_owned_paths(&actual.scene, |id| id.starts_with("main.f1.L1.B")),
                baseline,
                "{filter:?}: retain prior opening geometry, material styling and source ownership"
            );
            let preview = crate::drawing_view(&s.doc, "mixed", view).unwrap();
            for (id, visible) in [("main.f1.L1.B0", old_visible), ("main.f1.L1.B1", new_visible)] {
                let edge = preview.element_segments.iter().find(|edge| edge.id == id);
                assert_eq!(edge.is_some(), visible, "{filter:?}: {id}");
                if let Some(edge) = edge {
                    let point = [(edge.segment[0][0] + edge.segment[1][0]) / 2.0, (edge.segment[0][1] + edge.segment[1][1]) / 2.0];
                    assert_eq!(preview.pick_element(point, 0.01).unwrap().id, id);
                }
            }
        }
        assert_eq!(s.doc, original);
    }

    #[test]
    fn mixed_no_range_plans_keep_default_hidden_and_no_room_invariants_at_final_scale() {
        for with_room in [false, true] {
            let mut s = mixed_plan_fixture();
            if with_room {
                s.execute("interior.add_rooms", json!({"building":"main","rooms":[{"id":"main-room","name":"Room","levels":[1],"polygon":[[7500,1500],[11500,1500],[11500,6500],[7500,6500]]}]})).unwrap();
            }
            mixed_plan_add_roof_neighbour(&mut s);
            let hidden = crate::views::sheet(&s.doc, "mixed", View::latest(&s.doc)).unwrap();
            assert!(!hidden.scene.items.iter().any(|item| matches!(item, crate::sheet::Item::Text(text) if text.source.as_ref().is_some_and(|source| matches!(source.role, "room_name" | "room_area")))));
            assert_eq!(hidden.scene.element_regions.iter().any(|region| region.kind == DrawingElementKind::Room), with_room);
            for denominator in [50.0, 100.0, 200.0] {
                s.execute("view.configure", json!({"id":"mixed","template":{"name":"Room tags","room_labels":"name_and_area","sizing":{"mode":"fixed","denominator":denominator,"centre":[11500,4000]}}})).unwrap();
                let source = s.doc.clone();
                let sheet = crate::views::sheet(&s.doc, "mixed", View::latest(&s.doc)).unwrap();
                assert_eq!(sheet.scene.scale, denominator);
                let tags: Vec<_> = sheet
                    .scene
                    .items
                    .iter()
                    .filter_map(|item| match item {
                        crate::sheet::Item::Text(text) if text.source.as_ref().is_some_and(|source| source.id == "main-room.space.L1") => Some(text),
                        _ => None,
                    })
                    .collect();
                assert_eq!(tags.len(), if with_room { 2 } else { 0 });
                assert!(tags.iter().all(|text| text.size == 2.5), "text keeps its paper-mm size after scene composition and final template scale");
                assert_eq!(s.doc, source);
            }
            let source = s.doc.clone();
            assert!(s.execute("interior.add_rooms", json!({"building":"rear","rooms":[{"id":"unsupported","name":"Unsupported roof room","levels":[1],"polygon":[[16000,1000],[22000,1000],[22000,7000],[16000,7000]]}]})).is_err());
            assert_eq!(s.doc, source, "roof-room rejection remains atomic; no habitable quantities are invented");
        }
    }

    #[test]
    fn mixed_no_range_plans_do_not_duplicate_site_or_include_buildings_without_the_level() {
        let mut s = mixed_plan_fixture();
        s.execute("building.add_basement", json!({"building":"main","floor":-2500})).unwrap();
        s.execute("view.define", json!({"id":"mixed","name":"Public ground plan","kind":{"view":"plan","level":1}})).unwrap();
        s.execute(
            "view.configure",
            json!({"id":"mixed","template":{"name":"Fixed","sizing":{"mode":"fixed","denominator":100,"centre":[11500,4000]}}}),
        )
        .unwrap();
        s.execute("site.add", json!({"object":{"id":"site.paving","name":"Public paving","intent":"existing","geometry":{"kind":"ground","polygon":[[-3000,-3000],[-1000,-3000],[-1000,-1000],[-3000,-1000]],"elevation_mm":0,"thickness_mm":100,"surface":"paving"}}})).unwrap();
        let before = crate::views::sheet(&s.doc, "mixed", View::latest(&s.doc)).unwrap();
        let site = mixed_plan_owned_paths(&before.scene, |id| id == "site.paving");
        assert_eq!(site.len(), 1);
        mixed_plan_add_roof_neighbour(&mut s);
        s.execute("building.add", json!({"id":"short","name":"No selected storey","footprint":[[24000,0],[26000,0],[26000,5000],[24000,5000]],"storeys":1,"storey_height_mm":5000})).unwrap();
        for reverse in [false, true] {
            if reverse {
                s.doc.buildings.swap(0, 1);
            }
            let source = s.doc.clone();
            let after = crate::views::sheet(&s.doc, "mixed", View::latest(&s.doc)).unwrap();
            assert_eq!(
                mixed_plan_owned_paths(&after.scene, |id| id == "site.paving"),
                site,
                "the whole-sheet symbolic route owns site footprints exactly once, independent of building order"
            );
            assert!(mixed_plan_owned_paths(&after.scene, |id| id.starts_with("short.")).is_empty());
            assert!(!after.scene.element_regions.iter().any(|region| region.id.starts_with("short.")));
            let preview = crate::drawing_view(&s.doc, "mixed", View::latest(&s.doc)).unwrap();
            assert_eq!(preview.pick_element([-2000.0, -2000.0], 1.0).unwrap().id, "site.paving");
            assert!(preview.pick_element([25000.0, 2500.0], 1.0).is_none());
            assert_eq!(s.doc, source);
        }
    }

    #[test]
    fn round_opening_plan_cuts_and_picking_follow_the_actual_profile() {
        let mut doc = Document::default();
        let mut b = trigon_model::Building::new("b", "Round plan fixture", &[[0., 0.], [9000., 0.], [9000., 6000.], [0., 6000.]], 1, 3000.).unwrap();
        b.facades[0].bays = vec![4500.0];
        b.facades[0].placements = serde_json::from_value(json!([{"level":0,"bay":0,"type":"round"}])).unwrap();
        doc.buildings.push(b);
        doc.types.push(
            serde_json::from_value(json!({"id":"round","name":"Round window","width":700,"height":700,"sill":850,
            "kind":{"family":"historic_window","shape":{"kind":"round"},"operable":false}}))
            .unwrap(),
        );
        doc.views.push(serde_json::from_value(json!({"id":"plan","name":"Plan","kind":{"view":"plan","level":0}})).unwrap());
        for sill in [850.0, 1000.0, 1750.0, 300.0] {
            doc.types.last_mut().unwrap().sill = sill;
            let built = build_view(&doc, View::latest(&doc)).unwrap();
            let opening = &built[0].walls[0].openings[0];
            let (prims, _, _) = scene(&doc, 0, View::latest(&doc), true).unwrap();
            let windows: Vec<_> = prims
                .iter()
                .filter_map(|p| match p {
                    Prim::Line { a, b, width, .. } if *width == 25.0 => Some([*a, *b]),
                    _ => None,
                })
                .collect();
            let preview = crate::drawing_view(&doc, "plan", View::latest(&doc)).unwrap();
            if sill == 850.0 {
                assert_eq!(windows.len(), 2);
                let (left, right) = trigon_model::profile::width_at(&opening.profile, CUT).unwrap();
                let analytic = 2.0 * (350.0_f64.powi(2) - 200.0_f64.powi(2)).sqrt();
                assert!((right - left - analytic).abs() < 2.0, "48-chord profile must agree with the analytic circular opening");
                for end in [left, right] {
                    assert!(
                        prims.iter().any(|p| matches!(p, Prim::Poly {pts,surface:Surface::Exterior,..}
                        if pts.iter().any(|p|(p[0]-end).abs()<1e-8 && p[1].abs()<1e-8))),
                        "the actual masonry outline must end at the same horizontal cut as the symbol"
                    );
                }
                for [a, b] in windows {
                    assert!(
                        (a[0] - left).abs() < 1e-8 && (b[0] - right).abs() < 1e-8,
                        "plan window must use the actual horizontal cut span, not nominal width"
                    );
                }
                assert_eq!(preview.pick_element([4500.0, 0.0], 0.01).unwrap().id, "b.f1.L0.B0");
            } else {
                assert!(windows.is_empty(), "a tangent or noncrossing profile must not remove a full-width hole at sill {sill}");
                assert!(!preview.element_segments.iter().any(|e| e.id == "b.f1.L0.B0"));
            }
            assert_eq!(
                preview.pick_element([4160.0, 0.0], 0.01).unwrap().id,
                "b.f1",
                "masonry at the nominal corner must remain pickable at sill {sill}"
            );
        }
    }

    #[test]
    fn stair_direction_head_stays_on_the_first_walk_segment_in_both_plan_styles() {
        for kind in ["straight", "u_turn", "double_quarter_turn"] {
            for (angle, width) in [(0.0_f64, 600.0), (37.0, 620.0), (90.0, 1000.0)] {
                let (sin, cos) = angle.to_radians().sin_cos();
                let at = |run: f64, across: f64| [3500.0 + run * cos - across * sin, 3500.0 + run * sin + across * cos];
                let length = if kind == "double_quarter_turn" { 500.0 } else { 1000.0 };
                let stair: trigon_model::Stair = serde_json::from_value(json!({
                    "id":"direction", "levels":[0], "kind":kind, "rounded":false,
                    "outline":[at(0.0,0.0),at(length,0.0),at(length,width),at(0.0,width)]
                }))
                .unwrap();
                let walk = trigon_model::stair::symbol(&stair, 3000.0).unwrap().walk;
                let (start, end) = (walk[0], walk[1]);
                let distance = (end[0] - start[0]).hypot(end[1] - start[1]);
                let direction = [(end[0] - start[0]) / distance, (end[1] - start[1]) / distance];
                let mut building =
                    trigon_model::Building::new("b", "Direction fixture", &[[0., 0.], [8000., 0.], [8000., 8000.], [0., 8000.]], 2, 3000.).unwrap();
                building.stairs.push(stair);
                let mut doc = Document::default();
                doc.buildings.push(building);
                let before = doc.clone();
                for level in [0, 1] {
                    for presentation in [false, true] {
                        let (prims, _, elements) = scene(&doc, level, View::latest(&doc), presentation).unwrap();
                        let heads: Vec<_> = prims
                            .iter()
                            .enumerate()
                            .filter_map(|(index, p)| {
                                if !elements.get(&index).is_some_and(|e| e.id == "direction.L0") {
                                    return None;
                                }
                                match p {
                                    Prim::Line { a, b, width, .. } | Prim::StairLine { a, b, width, .. } if *width == 9.0 => Some((*a, *b)),
                                    _ => None,
                                }
                            })
                            .rev()
                            .take(2)
                            .collect();
                        assert_eq!(heads.len(), 2);
                        let tip = heads[0].1;
                        assert_eq!(tip, heads[1].1);
                        for point in [tip, heads[0].0, heads[1].0] {
                            let along = (point[0] - start[0]) * direction[0] + (point[1] - start[1]) * direction[1];
                            assert!(
                                along >= 0.0 && along <= distance + 1e-6,
                                "{kind}, {angle} degrees, level {level}, presentation {presentation}: arrow extends beyond its first flight ({along} > {distance} mm)"
                            );
                        }
                        let off_axis = (tip[0] - start[0]) * direction[1] - (tip[1] - start[1]) * direction[0];
                        assert!(off_axis.abs() < 1e-6);
                        if distance >= 450.0 {
                            assert!(
                                ((tip[0] - start[0]).hypot(tip[1] - start[1]) - 450.0).abs() < 1e-6,
                                "existing full-size symbols retain their exact arrow position"
                            );
                        }
                        let preview = crate::plan_drawing(&doc, level, View::latest(&doc)).unwrap();
                        assert_eq!(preview.pick_element(tip, 0.01).unwrap().id, "direction.L0");
                        for (base, tip) in heads {
                            let expected = if base <= tip { [base, tip] } else { [tip, base] };
                            assert!(
                                preview.element_segments.iter().any(|line| line.id == "direction.L0"
                                    && line.segment.iter().zip(expected).all(|(a, b)| (a[0] - b[0]).hypot(a[1] - b[1]) < 1e-6)),
                                "display/export and exact picking must share the direction head"
                            );
                        }
                        assert_eq!(doc, before);
                    }
                }
            }
        }
    }

    #[test]
    fn presentation_stairs_retain_model_detail_at_departure_and_receiving_storeys() {
        use crate::sheet::Item;

        for (kind, rounded, winders) in
            [("straight", false, None), ("u_turn", true, None), ("u_turn", false, Some(6)), ("double_quarter_turn", false, None)]
        {
            let mut doc = Document::default();
            let mut building = trigon_model::Building::new("b", "test", &[[0., 0.], [12000., 0.], [12000., 10000.], [0., 10000.]], 2, 3000.).unwrap();
            building.storeys[1].height = 3900.0;
            let stair: trigon_model::Stair = serde_json::from_value(json!({
                "id":"measured-stair", "levels":[0], "outline":[[3500.,3500.],[6000.,3500.],[6000.,5500.],[3500.,5500.]],
                "kind":kind, "rounded":rounded, "winders":winders
            }))
            .unwrap();
            let symbol = trigon_model::stair::symbol(&stair, building.storeys[0].height).unwrap();
            assert!(!symbol.treads.is_empty());
            building.stairs.push(stair);
            doc.buildings.push(building);
            let original = doc.clone();
            for level in [0, 1] {
                for rotation in [0.0, 27.5] {
                    let sheet = sheet_scene(&doc, level, View::latest(&doc), rotation, None, "").unwrap();
                    let frame = sheet_frame(&doc, None, rotation, level, View::latest(&doc)).unwrap();
                    let paths: Vec<_> = sheet
                        .scene
                        .items
                        .iter()
                        .filter_map(|item| match item {
                            Item::Path(path) if path.layer == Layer::Stair && !path.closed => Some(path),
                            _ => None,
                        })
                        .collect();
                    for (a, b) in &symbol.treads {
                        let expected = vec![vec![frame.rot(*a), frame.rot(*b)]];
                        // An end tread may coincide with the heavier stringer/landing outline.
                        let tread = paths.iter().find(|path| path.rings == expected && path.stroke.is_some_and(|stroke| stroke.pen == 0.13));
                        assert!(tread.is_some(), "{kind} at level {level}, rotation {rotation}: model tread is missing from the presentation plan");
                        let stroke = tread.unwrap().stroke.unwrap();
                        assert_eq!(stroke.pen, 0.13);
                        assert!(!stroke.dashed);
                    }
                    for points in symbol.walk.windows(2) {
                        let expected = vec![vec![frame.rot(points[0]), frame.rot(points[1])]];
                        let walk = paths.iter().find(|path| path.rings == expected).unwrap();
                        let stroke = walk.stroke.unwrap();
                        assert!(stroke.dashed);
                        assert_eq!(stroke.colour, [0x33; 3], "direction lines must not take demolition red");
                    }
                    for points in symbol.landing.windows(2) {
                        let expected = vec![vec![frame.rot(points[0]), frame.rot(points[1])]];
                        assert!(paths.iter().any(|path| path.rings == expected), "model landing outline is missing");
                    }
                    assert!(paths.iter().all(|path| path.stroke.unwrap().pen < 0.35), "stair detail must remain finer than cut walls");
                    assert_eq!(doc, original, "drawing must not change the model");
                }
            }
        }
    }

    #[test]
    fn receiving_floor_shows_its_stair_and_rotated_box_crop_is_valid() {
        let mut b = trigon_model::Building::new("b", "test", &[[0., 0.], [10000., 0.], [10000., 10000.], [0., 10000.]], 2, 3000.).unwrap();
        b.stairs.push(
            serde_json::from_value(json!({"id":"only-flight", "levels":[0], "outline":[[2000.,2000.],[4000.,2000.],[4000.,3200.],[2000.,3200.]]}))
                .unwrap(),
        );
        assert_eq!(visible_stairs(&b, 1).len(), 1);
        let mut doc = Document::default();
        doc.buildings.push(b);
        let frame = sheet_frame(&doc, Some(&[[0., 0.], [10000., 10000.]]), 45., 0, View::latest(&doc)).unwrap();
        assert!(frame.mm.is_finite());
        for p in [[0., 0.], [10000., 0.], [10000., 10000.], [0., 10000.]] {
            let q = frame.px(p);
            assert!((70.0..=1530.0).contains(&q[0]) && (130.0..=800.0).contains(&q[1]));
        }
        let (prims, _, _) = scene(&doc, 1, View::latest(&doc), true).unwrap();
        assert!(prims.iter().any(|p| matches!(p, Prim::Poly { surface: Surface::Stair, .. })));
    }

    #[test]
    fn plan_lines_find_their_layers() {
        let mut doc = Document::default();
        doc.buildings.push(trigon_model::Building::new("b", "test", &[[0., 0.], [10000., 0.], [10000., 8000.], [0., 8000.]], 2, 3000.).unwrap());
        // A balcony's outline hangs off the facade; an interior door leaf is well inside.
        assert_eq!(line_layer(&doc, 30.0, [2000., 0.], [2000., -1300.]), Layer::Balcony);
        assert_eq!(line_layer(&doc, 30.0, [2000., 0.], [4000., 0.]), Layer::Balcony);
        assert_eq!(line_layer(&doc, 30.0, [4000., 4000.], [4900., 4000.]), Layer::Door);
        assert_eq!(line_layer(&doc, 45.0, [0., 2000.], [0., 2900.]), Layer::Door);
        assert_eq!(line_layer(&doc, 25.0, [0., 2000.], [0., 2900.]), Layer::Glazing);
        assert_eq!(line_layer(&doc, 10.0, [0., -1600.], [900., -1600.]), Layer::Annotation);
    }

    #[test]
    fn labels_collide_as_rotated_boxes() {
        let label = |at: [f64; 2], deg: f64| Label {
            at,
            alt: None,
            deg,
            fs: 14.0,
            text: "Ca. 1300 × 2700".into(),
            balcony: true,
            source: None,
            clearance: None,
            entrance: None,
        };
        // Side by side 80 px apart: the 15-character labels overlap; 140 px apart they don't.
        assert!(label([100., 100.], 0.).overlaps(&label([180., 100.], 0.)));
        assert!(!label([100., 100.], 0.).overlaps(&label([240., 100.], 0.)));
        // One line further out clears it; turned 90° the boxes are narrow across the line.
        assert!(!label([100., 100.], 0.).overlaps(&label([180., 124.], 0.)));
        assert!(!label([100., 100.], 90.).overlaps(&label([130., 100.], 90.)));
    }

    #[test]
    fn entrance_labels_find_the_facade_on_a_stair_axis() {
        let square = [[0., 0.], [10000., 0.], [10000., 8000.], [0., 8000.]];
        assert_eq!(facade_crossing(&square, [4000., 3000.], [0., -1.]), Some(3000.));
        assert_eq!(facade_crossing(&square, [4000., 3000.], [1., 0.]), Some(6000.));
        assert_eq!(facade_crossing(&square, [-100., 3000.], [-1., 0.]), None);
    }

    #[test]
    fn concave_room_label_stays_inside_and_away_from_walls() {
        let p = [[0., 0.], [6000., 0.], [6000., 1500.], [1500., 1500.], [1500., 6000.], [0., 6000.]];
        let (at, clearance) = room_label_anchor(&p).unwrap();
        assert!(clearance > 500.0);
        assert!(crate::drawings::inside(&p, at));
        assert!(at[0] > 500. && at[1] > 500.);
        assert!(at[0] < 1000. || at[1] < 1000.);
    }
    /// Original public fixture: separate enclosures with a real 200 mm gap, but colliding tags.
    fn entrance_tag_fixture() -> Session {
        let mut s = Session::new(Box::new(trigon_data::Live::new(Default::default())));
        s.execute(
            "building.add",
            json!({"id":"b","name":"Synthetic entrance tags",
            "footprint":[[0,0],[20000,0],[20000,16000],[0,16000]],"storeys":2,"storey_height_mm":3000}),
        )
        .unwrap();
        s.execute("interior.add_stairs", json!({"building":"b","stairs":[
            {"id":"core-a","levels":[0],"outline":[[4000,3000],[4000,6000],[6000,6000],[6000,3000]],"kind":"u_turn","landing":1000,"well":150,"rounded":false},
            {"id":"core-b","levels":[0],"outline":[[6200,3000],[6200,6000],[8200,6000],[8200,3000]],"kind":"u_turn","landing":1000,"well":150,"rounded":false}
        ]})).unwrap();
        s.doc.buildings[0].entrances = serde_json::from_value(json!([
            {"street":"Synthetic street","number":"1A","point":[5000,0],"stair":"core-a","units":[{"floor":"st","door":"tv"},{"floor":"st","door":"th"},{"floor":"1","door":"tv"},{"floor":"1","door":"th"}]},
            {"street":"Synthetic street","number":"1B","point":[7200,0],"stair":"core-b","units":[{"floor":"st","door":"tv"},{"floor":"st","door":"th"},{"floor":"1","door":"tv"},{"floor":"1","door":"th"}]}
        ])).unwrap();
        s
    }

    fn entrance_tag_texts(scene: &Scene) -> Vec<&Text> {
        scene
            .items
            .iter()
            .filter_map(|item| match item {
                crate::sheet::Item::Text(text) => Some(text),
                _ => None,
            })
            .collect()
    }

    #[test]
    fn neighboring_entrances_keep_every_source_known_side_without_mutating_units() {
        let mut s = entrance_tag_fixture();
        let before = s.doc.clone();
        for reverse in [false, true] {
            if reverse {
                s.doc.buildings[0].entrances.reverse();
            }
            for level in [0, 1] {
                let sheet = drawing_sheet(&s.doc, level, View::latest(&s.doc)).unwrap();
                let texts = entrance_tag_texts(&sheet.scene);
                for address in ["1A", "1B"] {
                    assert!(
                        texts.iter().any(|text| text.text.contains(address) && text.text.contains("TV") && text.text.contains("TH")),
                        "source-known sides for {address} were lost on level {level}"
                    );
                }
                assert_eq!(sheet.metrics["entrance_labels"]["requested"], 2);
                assert_eq!(sheet.metrics["entrance_labels"]["placed"], 2);
                assert_eq!(sheet.metrics["entrance_labels"]["unplaced"], 0);
            }
        }
        s.doc.buildings[0].entrances.reverse();
        assert_eq!(s.doc, before);
    }

    #[test]
    fn configured_tags_use_final_scale_crop_and_real_glyph_clearance() {
        let mut s = entrance_tag_fixture();
        s.execute("view.define", json!({"id":"entrances","name":"Public entrance plan","kind":{"view":"plan","level":0}})).unwrap();
        for denominator in [100.0, 150.0, 200.0] {
            s.execute(
                "view.configure",
                json!({"id":"entrances","template":{"name":"Public fixed",
                "sizing":{"mode":"fixed","denominator":denominator,"centre":[10000,4000]}}}),
            )
            .unwrap();
            let before = s.doc.clone();
            let sheet = crate::views::sheet_unclipped(&s.doc, "entrances", View::latest(&s.doc)).unwrap();
            assert_eq!(sheet.scene.scale, denominator);
            assert_eq!(sheet.metrics["entrance_labels"]["placed"], 2);
            let report = s.execute("view.audit", json!({"id":"entrances"})).unwrap();
            let labels = report["labels"].as_array().unwrap();
            for id in ["b.entrance0", "b.entrance1"] {
                let label = labels.iter().find(|label| label["source"]["id"] == id).unwrap();
                assert_eq!(label["source"]["kind"], "entrance");
                assert_eq!(label["source"]["role"], "unit_register");
                assert_eq!(label["clipped"], false);
                assert!(
                    label["inspect_commands"]
                        .as_array()
                        .unwrap()
                        .iter()
                        .any(|command| command["command"] == "entrance.get" && command["params"]["id"] == id)
                );
            }
            assert!(!report["findings"].as_array().unwrap().iter().any(|finding| {
                finding["kind"] == "potential_text_overlap"
                    && finding["sources"]
                        .as_array()
                        .is_some_and(|sources| sources.len() == 2 && sources.iter().all(|source| source["kind"] == "entrance"))
            }));
            assert_eq!(s.doc, before);
        }
    }

    #[test]
    fn presentation_tags_keep_numeric_and_middle_doors_and_do_not_fill_unknown_sides() {
        let mut s = entrance_tag_fixture();
        s.doc.buildings[0].entrances.truncate(1);
        s.doc.buildings[0].entrances[0].units = serde_json::from_value(json!([
            {"floor":"st","door":"42"},{"floor":"st","door":" mf "},{"floor":"st","door":""},
            {"floor":"1","door":"th"}
        ]))
        .unwrap();
        let before = s.doc.clone();
        let ground = drawing_sheet(&s.doc, 0, View::latest(&s.doc)).unwrap();
        let tag = entrance_tag_texts(&ground.scene).into_iter().find(|text| text.text.contains("1A")).unwrap();
        assert!(tag.text.contains("42") && tag.text.contains("MF"));
        assert!(!tag.text.contains("TV") && !tag.text.contains("TH"));
        assert_eq!(ground.metrics["entrance_labels"]["records"][0]["unit_records"], 3);
        assert_eq!(ground.metrics["entrance_labels"]["records"][0]["unknown_door_units"], 1);
        let arrival = drawing_sheet(&s.doc, 1, View::latest(&s.doc)).unwrap();
        let tag = entrance_tag_texts(&arrival.scene).into_iter().find(|text| text.text.contains("1A")).unwrap();
        assert!(tag.text.contains("TH") && !tag.text.contains("TV") && !tag.text.contains("42"));
        assert_eq!(s.doc, before);
    }

    #[test]
    fn missing_stair_association_uses_an_explicit_register_legend() {
        let mut s = entrance_tag_fixture();
        s.doc.buildings[0].entrances.truncate(1);
        s.doc.buildings[0].entrances[0].stair = None;
        let before = s.doc.clone();
        let sheet = drawing_sheet(&s.doc, 0, View::latest(&s.doc)).unwrap();
        assert_eq!(sheet.metrics["entrance_labels"]["requested"], 1);
        assert_eq!(sheet.metrics["entrance_labels"]["placed"], 1);
        let record = &sheet.metrics["entrance_labels"]["records"][0];
        assert_eq!(record["state"], "legend");
        assert_eq!(record["placement_note"], "missing_explicit_stair_association");
        assert!(
            entrance_tag_texts(&sheet.scene)
                .iter()
                .any(|text| text.text.contains("Opgang 1A") && text.text.contains("TV") && text.text.contains("TH"))
        );
        assert_eq!(s.doc, before);
    }

    #[test]
    fn entrance_register_legend_keeps_an_explicit_unknown_number() {
        for number in ["", "1A"] {
            let mut s = entrance_tag_fixture();
            s.doc.buildings[0].entrances.truncate(1);
            let entrance = &mut s.doc.buildings[0].entrances[0];
            entrance.number = number.into();
            entrance.stair = None;
            let before = s.doc.clone();
            let sheet = drawing_sheet(&s.doc, 0, View::latest(&s.doc)).unwrap();
            let record = &sheet.metrics["entrance_labels"]["records"][0];
            let expected = format!("Opgang {} — TV / TH", if number.is_empty() { "(nummer ukendt)" } else { number });
            assert_eq!(record["state"], "legend");
            assert_eq!(record["legend_text"], expected);
            assert!(
                entrance_tag_texts(&sheet.scene).iter().any(|text| text.text == expected),
                "the printed fallback must retain the unknown-number notice"
            );
            assert_eq!(s.doc, before);
        }
    }

    #[test]
    fn impossible_complete_tag_remains_inspectable_and_is_reported_unplaced() {
        let mut s = entrance_tag_fixture();
        s.doc.buildings[0].entrances.truncate(1);
        s.doc.buildings[0].entrances[0].number = "W".repeat(300);
        s.execute("view.define", json!({"id":"too-wide","name":"Public too-wide tag","kind":{"view":"plan","level":0}})).unwrap();
        let before = s.doc.clone();
        let sheet = drawing_sheet(&s.doc, 0, View::latest(&s.doc)).unwrap();
        assert_eq!(sheet.metrics["entrance_labels"]["requested"], 1);
        assert_eq!(sheet.metrics["entrance_labels"]["placed"], 0);
        assert_eq!(sheet.metrics["entrance_labels"]["unplaced"], 1);
        assert!(sheet.note.as_ref().is_some_and(|note| note.contains("kræver placering")));
        let tags: Vec<_> = entrance_tag_texts(&sheet.scene)
            .into_iter()
            .filter(|text| text.source.as_ref().is_some_and(|source| source.kind == "entrance"))
            .collect();
        assert_eq!(tags.len(), 1, "unplaced text stays in the audit scene");
        assert!(tags[0].text.contains(&"W".repeat(300)) && tags[0].text.contains("TV") && tags[0].text.contains("TH"));
        let report = s.execute("view.audit", json!({"id":"too-wide"})).unwrap();
        assert!(
            report["findings"]
                .as_array()
                .unwrap()
                .iter()
                .any(|finding| finding["kind"] == "unplaced_entrance_tag" && finding["source"]["id"] == "b.entrance0")
        );
        assert_eq!(report["status"], "needs_review");
        assert_eq!(s.doc, before);
    }

    #[test]
    fn a_single_fitting_tag_retains_its_legacy_anchor_and_font_size() {
        let mut s = entrance_tag_fixture();
        s.doc.buildings[0].entrances.truncate(1);
        let frame = sheet_frame(&s.doc, None, 0.0, 0, View::latest(&s.doc)).unwrap();
        let sheet = drawing_sheet(&s.doc, 0, View::latest(&s.doc)).unwrap();
        let tag = entrance_tag_texts(&sheet.scene).into_iter().find(|text| text.text.contains("1A")).unwrap();
        assert_eq!(tag.text, "TV  1A  TH");
        assert_eq!(tag.size, 18.0 * MM_PER_PX);
        assert!((sheet.vp.px(tag.at)[0] - frame.px([5000.0, -1100.0])[0]).abs() < 1e-8);
        assert!((sheet.vp.px(tag.at)[1] - frame.px([5000.0, -1100.0])[1]).abs() < 1e-8);
    }

    #[test]
    fn unknown_register_designations_agree_between_labels_and_inspection() {
        for marker in ["", "?", "Unknown", "Ukendt", "uoplyst", "ikke oplyst"] {
            let mut s = entrance_tag_fixture();
            let e = &mut s.doc.buildings[0].entrances[0];
            e.units[0].door = "tv".into();
            e.units[1].door = marker.into();
            let before = s.doc.clone();
            let info = s.execute("entrance.get", json!({"id":"b.entrance0"})).unwrap();
            let sheet = drawing_sheet(&s.doc, 0, View::latest(&s.doc)).unwrap();
            let record = sheet.metrics["entrance_labels"]["records"].as_array().unwrap().iter().find(|r| r["source"]["id"] == "b.entrance0").unwrap();
            assert_eq!(record["unknown_door_units"], info["unknown_designations"], "{marker:?}");
            assert_eq!(record["unknown_door_units"], 1);
            assert!(!record["text"].as_str().unwrap().contains('?'));
            assert!(record["legend_text"].as_str().unwrap().contains("ukendt dørbetegnelse"));
            assert_eq!(s.doc, before);
        }
    }

    #[test]
    fn cropped_away_preferred_tags_stay_in_audit_coverage() {
        let mut s = entrance_tag_fixture();
        s.execute("view.define", json!({"id":"cropped-entrances","name":"Public cropped entrances","kind":{"view":"plan","level":0}})).unwrap();
        s.execute(
            "view.configure",
            json!({"id":"cropped-entrances","template":{"name":"Public clipped facade edge",
            "crop":[0,0,20000,12000],"sizing":{"mode":"fixed","denominator":100,"centre":[10000,6000]}}}),
        )
        .unwrap();
        let before = s.doc.clone();
        let sheet = crate::views::sheet_unclipped(&s.doc, "cropped-entrances", View::latest(&s.doc)).unwrap();
        assert_eq!(sheet.metrics["entrance_labels"]["requested"], 2);
        assert_eq!(sheet.metrics["entrance_labels"]["placed"], 0);
        assert_eq!(sheet.metrics["entrance_labels"]["unplaced"], 2);
        assert_eq!(
            entrance_tag_texts(&sheet.scene).into_iter().filter(|text| text.source.as_ref().is_some_and(|source| source.kind == "entrance")).count(),
            2
        );
        let report = s.execute("view.audit", json!({"id":"cropped-entrances"})).unwrap();
        assert_eq!(report["finding_counts"]["unplaced_entrance_tag"], 2);
        for text in entrance_tag_texts(&sheet.scene) {
            let input = text.entrance.as_ref().unwrap();
            assert_eq!(text.at, input.at, "unplaced source tags must not be moved onto the crop boundary");
        }
        assert_eq!(s.doc, before);
    }
}
