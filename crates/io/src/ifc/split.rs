//! How Trigon's built geometry divides into IFC products, shared by the writer and by
//! [`model_shapes`] (which `ifc.check` compares the readers against): balconies into deck, railing,
//! beams and tie rods; a pitched roof into its planes and gable walls; stairs into flights and
//! landings.

use std::collections::BTreeMap;

use trigon_model::build::{BuiltElement, BuiltRoof, BuiltWall};
use trigon_model::stair::{self, Part, PartKind};
use trigon_model::{Building, Built, ComponentKind, Document, ElementClass, Material, Mesh};

use super::part21::{Frame, cross, unit};

/// Thickness of a pitched roof plane below its surface (tiles, battens and rafters), mm.
pub const ROOF_PLANE: f64 = 250.0;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Class {
    Slab,
    Railing,
    Beam,
    Member,
}

/// One IFC product made from part of a Trigon element.
#[derive(Debug, Clone, PartialEq)]
pub struct Product {
    pub key: String,
    pub class: Class,
    pub name: String,
    pub parts: Vec<(Material, Mesh)>,
}

/// Disjoint pieces of a mesh. With `weld`, triangles touching at the same coordinates belong
/// together (boxes are written face by face); without, only those sharing vertex indices.
pub fn components(m: &Mesh, weld: bool) -> Vec<Mesh> {
    let n = m.positions.len();
    let mut parent: Vec<usize> = (0..n).collect();
    fn find(parent: &mut [usize], mut i: usize) -> usize {
        while let Some(&p) = parent.get(i) {
            if p == i {
                break;
            }
            let gp = parent.get(p).copied().unwrap_or(p);
            if let Some(slot) = parent.get_mut(i) {
                *slot = gp;
            }
            i = p;
        }
        i
    }
    let union = |parent: &mut Vec<usize>, a: usize, b: usize| {
        let (ra, rb) = (find(parent, a), find(parent, b));
        if ra != rb
            && let Some(slot) = parent.get_mut(ra)
        {
            *slot = rb;
        }
    };
    if weld {
        let mut seen: BTreeMap<[i64; 3], usize> = BTreeMap::new();
        for (i, p) in m.positions.iter().enumerate() {
            let key = p.map(|v| (v * 1000.0).round() as i64);
            match seen.get(&key) {
                Some(&j) => union(&mut parent, i, j),
                None => {
                    seen.insert(key, i);
                }
            }
        }
    }
    for t in &m.triangles {
        let [a, b, c] = t.map(|i| i as usize);
        if a < n && b < n && c < n {
            union(&mut parent, a, b);
            union(&mut parent, a, c);
        }
    }
    let mut groups: BTreeMap<usize, Vec<[u32; 3]>> = BTreeMap::new();
    for t in &m.triangles {
        let a = t[0] as usize;
        if a < n {
            let r = find(&mut parent, a);
            groups.entry(r).or_default().push(*t);
        }
    }
    groups
        .into_values()
        .map(|tris| {
            let mut map: BTreeMap<u32, u32> = BTreeMap::new();
            let mut out = Mesh::default();
            for t in tris {
                let mut nt = [0u32; 3];
                for (k, &i) in t.iter().enumerate() {
                    let next = out.positions.len() as u32;
                    let j = *map.entry(i).or_insert_with(|| {
                        out.positions.push(m.positions.get(i as usize).copied().unwrap_or_default());
                        next
                    });
                    nt[k] = j;
                }
                out.triangles.push(nt);
            }
            out
        })
        .collect()
}

/// The boxes of a mesh built from boxes only (`Mesh::hexahedron`: 24 corners and 12 triangles
/// each), one mesh per box; otherwise its disjoint pieces.
pub fn boxes(m: &Mesh) -> Vec<Mesh> {
    let n = m.positions.len();
    let fits = n > 0
        && n.is_multiple_of(24)
        && m.triangles.len() * 2 == n
        && m.triangles.iter().enumerate().all(|(k, t)| t.iter().all(|&i| i as usize / 24 == k / 12));
    if !fits {
        return components(m, true);
    }
    m.positions
        .chunks(24)
        .zip(m.triangles.chunks(12))
        .enumerate()
        .map(|(k, (p, t))| {
            let base = (k * 24) as u32;
            Mesh { positions: p.to_vec(), triangles: t.iter().map(|t| t.map(|i| i - base)).collect() }
        })
        .collect()
}

/// The products of one balcony: its slab or deck, its railing and, for a steel balcony, the
/// frame beams and tie rods. `elements` are the balcony's built elements (slab and rail).
pub fn balcony(elements: &[&BuiltElement], kind: Option<&ComponentKind>) -> Vec<Product> {
    let mut out = Vec::new();
    for e in elements {
        match e.class {
            ElementClass::Railing => out.push(Product { key: e.id.clone(), class: Class::Railing, name: e.name.clone(), parts: e.parts.clone() }),
            ElementClass::BalconySlab => {
                let mut parts = e.parts.clone();
                if let Some(ComponentKind::SteelBalcony { tie_rods, .. }) = kind {
                    // The steel balcony's slab is built as: frame, deck, underside, then the tie rods.
                    let rods = if *tie_rods && parts.len() > 3 { parts.pop() } else { None };
                    if !parts.is_empty() {
                        let (metal, frame) = parts.remove(0);
                        for (k, beam) in boxes(&frame).into_iter().enumerate() {
                            out.push(Product {
                                key: format!("{}.beam{k}", e.id),
                                class: Class::Beam,
                                name: "UPE 160".into(),
                                parts: vec![(metal, beam)],
                            });
                        }
                    }
                    if let Some((metal, rods)) = rods {
                        for (k, rod) in boxes(&rods).into_iter().enumerate() {
                            out.push(Product {
                                key: format!("{}.rod{k}", e.id),
                                class: Class::Member,
                                name: "Trækstang".into(),
                                parts: vec![(metal, rod)],
                            });
                        }
                    }
                }
                out.push(Product { key: e.id.clone(), class: Class::Slab, name: e.name.clone(), parts });
            }
            ElementClass::Window | ElementClass::Door | ElementClass::Stair => {}
        }
    }
    out
}

/// A pitched roof plane or a gable, as a polygon in site mm.
#[derive(Debug, Clone, PartialEq)]
pub struct RoofPiece {
    pub key: String,
    /// A plane's underside outline, counter-clockwise seen from above; a gable's outline.
    pub polygon: Vec<[f64; 3]>,
    /// The same as triangles, as Trigon built it (a plane next to a gable may be warped).
    pub surface: Mesh,
    /// A plane's solid runs from its underside along this to its top face, mm.
    pub rise: [f64; 3],
    /// For a gable: the facade wall it stands on and its thickness.
    pub gable: Option<(String, f64)>,
}

/// A roof plane further than this from flat is written as a tessellated solid, mm.
pub const WARP: f64 = 1.0;

impl RoofPiece {
    /// How far the outline strays from its best plane, mm.
    pub fn warp(&self) -> f64 {
        let Some(n) = unit(normal(&self.polygon)) else { return f64::INFINITY };
        let Some(&o) = self.polygon.first() else { return f64::INFINITY };
        let d: Vec<f64> = self.polygon.iter().map(|p| (p[0] - o[0]) * n[0] + (p[1] - o[1]) * n[1] + (p[2] - o[2]) * n[2]).collect();
        let (lo, hi) = d.iter().fold((f64::INFINITY, f64::NEG_INFINITY), |(lo, hi), v| (lo.min(*v), hi.max(*v)));
        hi - lo
    }

    /// The plane's thickness square to it, mm.
    pub fn thickness(&self) -> f64 {
        unit(normal(&self.polygon)).map_or(0.0, |n| (self.rise[0] * n[0] + self.rise[1] * n[1] + self.rise[2] * n[2]).abs())
    }
}

/// The boundary of a set of triangles that form one planar polygon, as a loop of points.
fn outline(m: &Mesh) -> Option<Vec<[f64; 3]>> {
    let mut count: BTreeMap<(u32, u32), usize> = BTreeMap::new();
    for t in &m.triangles {
        for k in 0..3 {
            let (a, b) = (t[k], t[(k + 1) % 3]);
            *count.entry((a.min(b), a.max(b))).or_default() += 1;
        }
    }
    let mut next: BTreeMap<u32, Vec<u32>> = BTreeMap::new();
    for (&(a, b), &c) in &count {
        if c == 1 {
            next.entry(a).or_default().push(b);
            next.entry(b).or_default().push(a);
        }
    }
    if next.values().any(|v| v.len() != 2) {
        return None;
    }
    let (&start, _) = next.iter().next()?;
    let mut ring = vec![start];
    let (mut prev, mut at) = (start, *next.get(&start)?.first()?);
    while at != start {
        if ring.len() > next.len() {
            return None;
        }
        ring.push(at);
        let n = next.get(&at)?;
        let go = if n.first() == Some(&prev) { *n.get(1)? } else { *n.first()? };
        (prev, at) = (at, go);
    }
    (ring.len() == next.len()).then(|| ring.iter().filter_map(|&i| m.positions.get(i as usize).copied()).collect())
}

/// Newell's normal of a polygon (not normalised).
pub fn normal(p: &[[f64; 3]]) -> [f64; 3] {
    let mut n = [0.0; 3];
    for i in 0..p.len() {
        let (a, b) = (p[i], p[(i + 1) % p.len()]);
        n[0] += (a[1] - b[1]) * (a[2] + b[2]);
        n[1] += (a[2] - b[2]) * (a[0] + b[0]);
        n[2] += (a[0] - b[0]) * (a[1] + b[1]);
    }
    n
}

/// The area-weighted normal of a mesh's triangles (not normalised).
fn facing(m: &Mesh) -> [f64; 3] {
    let p = |i: u32| m.positions.get(i as usize).copied().unwrap_or_default();
    m.triangles.iter().fold([0.0; 3], |acc, t| {
        let (a, b, c) = (p(t[0]), p(t[1]), p(t[2]));
        let n = cross([b[0] - a[0], b[1] - a[1], b[2] - a[2]], [c[0] - a[0], c[1] - a[1], c[2] - a[2]]);
        [acc[0] + n[0], acc[1] + n[1], acc[2] + n[2]]
    })
}

fn shifted(m: &Mesh, d: [f64; 3]) -> Mesh {
    Mesh { positions: m.positions.iter().map(|p| [p[0] + d[0], p[1] + d[1], p[2] + d[2]]).collect(), triangles: m.triangles.clone() }
}

fn centroid(p: &[[f64; 3]]) -> [f64; 3] {
    let n = p.len().max(1) as f64;
    p.iter().fold([0.0; 3], |c, q| [c[0] + q[0] / n, c[1] + q[1] / n, c[2] + q[2] / n])
}

/// The planes of a pitched roof and its gables. Each face of the roof was triangulated on its
/// own, so faces are the groups of triangles that share vertices. A roof with a build-up is a
/// closed slab: each plane's top face lies straight above its soffit (the boards round the edge
/// are vertical), and the plane is the solid between them. A roof without one is a surface: the
/// plane hangs `ROOF_PLANE` below it.
pub fn roof(r: &BuiltRoof, walls: &[BuiltWall]) -> Vec<RoofPiece> {
    let mut up = Vec::new();
    let mut down = Vec::new();
    for face in components(&r.cover, false).into_iter().filter(|_| !r.compound) {
        let Some(polygon) = outline(&face) else { continue };
        // Which way the face looks, from its triangles; the outline is turned to run
        // counter-clockwise seen from above.
        let Some(n) = unit(facing(&face)) else { continue };
        let polygon = if normal(&polygon)[2] < 0.0 { polygon.into_iter().rev().collect() } else { polygon };
        if n[2] > 0.2 {
            up.push((polygon, face));
        } else if n[2] < -0.2 {
            down.push((polygon, face));
        }
    }
    let mut out = Vec::new();
    for (k, (top, face)) in up.into_iter().enumerate() {
        let key = format!("{}.face{k}", r.id);
        let c = centroid(&top);
        let soffit = down.iter().find(|(p, _)| {
            let d = centroid(p);
            p.len() == top.len() && (d[0] - c[0]).hypot(d[1] - c[1]) < 1.0 && d[2] < c[2]
        });
        let piece = match soffit {
            Some((polygon, surface)) => {
                let rise = [0.0, 0.0, c[2] - centroid(polygon)[2]];
                RoofPiece { key, polygon: polygon.clone(), surface: surface.clone(), rise, gable: None }
            }
            None => {
                let Some(n) = unit(normal(&top)) else { continue };
                let rise = n.map(|v| v * ROOF_PLANE);
                let below = rise.map(|v| -v);
                RoofPiece {
                    key,
                    polygon: top.iter().map(|p| [p[0] + below[0], p[1] + below[1], p[2] + below[2]]).collect(),
                    surface: shifted(&face, below),
                    rise,
                    gable: None,
                }
            }
        };
        out.push(piece);
    }
    for (k, face) in components(&r.gables, false).into_iter().enumerate() {
        let Some(polygon) = outline(&face) else { continue };
        // The facade whose outer face the gable stands in.
        let on = walls.iter().find(|w| {
            let fr = w.frame;
            polygon.iter().all(|p| {
                let d = [p[0] - fr.origin[0], p[1] - fr.origin[1]];
                let (u, off) = (d[0] * fr.t[0] + d[1] * fr.t[1], d[0] * fr.n[0] + d[1] * fr.n[1]);
                off.abs() < 2.0 && u > -2.0 && u < fr.length + 2.0
            })
        });
        let gable = on.map(|w| (w.id.clone(), w.segments.last().map_or(360.0, |s| s.thickness)));
        let key = match &gable {
            Some((id, _)) if !out.iter().any(|p: &RoofPiece| p.key == format!("{id}.gable")) => format!("{id}.gable"),
            _ => format!("{}.gable{k}", r.id),
        };
        out.push(RoofPiece { key, polygon, surface: face, rise: [0.0; 3], gable });
    }
    out
}

/// A roof plane's frame: z up its normal, x level along the plane.
pub fn plane_frame(polygon: &[[f64; 3]]) -> Option<Frame> {
    let mut n = unit(normal(polygon))?;
    if n[2] < 0.0 {
        n = n.map(|v| -v);
    }
    let o = *polygon.first()?;
    let x = unit(cross([0.0, 0.0, 1.0], n)).unwrap_or([1.0, 0.0, 0.0]);
    Frame::new(o, n, x)
}

/// A gable's frame: the facade's (x along it, y into the building), at the gable's foot.
pub fn gable_frame(polygon: &[[f64; 3]], wall: &BuiltWall) -> Option<Frame> {
    let fr = wall.frame;
    let z0 = polygon.iter().map(|p| p[2]).fold(f64::INFINITY, f64::min);
    Frame::plan([fr.origin[0], fr.origin[1], if z0.is_finite() { z0 } else { 0.0 }], fr.t)
}

/// A prism on a planar polygon, offset by `d` (site mm): the solid IFC extrudes.
fn slab_mesh(polygon: &[[f64; 3]], d: [f64; 3]) -> Mesh {
    let mut m = Mesh::default();
    let n = polygon.len();
    if n < 3 {
        return m;
    }
    let low: Vec<[f64; 3]> = polygon.iter().map(|p| [p[0] + d[0], p[1] + d[1], p[2] + d[2]]).collect();
    m.positions.extend_from_slice(polygon);
    m.positions.extend_from_slice(&low);
    let n32 = n as u32;
    for i in 1..n32 - 1 {
        m.triangles.push([0, i, i + 1]);
        m.triangles.push([n32, n32 + i + 1, n32 + i]);
    }
    for i in 0..n32 {
        let j = (i + 1) % n32;
        m.triangles.push([i, n32 + i, n32 + j]);
        m.triangles.push([i, n32 + j, j]);
    }
    m
}

/// A roof plane as a closed solid: its underside as Trigon built it, the same moved by the
/// rise on top, and sides along the outline, all facing out.
pub fn plane_solid(p: &RoofPiece) -> Mesh {
    let Some(f) = plane_frame(&p.polygon) else { return Mesh::default() };
    let mut under = p.surface.clone();
    for t in &mut under.triangles {
        let [a, b, c] = t.map(|i| under.positions.get(i as usize).copied().unwrap_or_default());
        let n = cross([b[0] - a[0], b[1] - a[1], b[2] - a[2]], [c[0] - a[0], c[1] - a[1], c[2] - a[2]]);
        if n[0] * f.z[0] + n[1] * f.z[1] + n[2] * f.z[2] > 0.0 {
            t.swap(1, 2);
        }
    }
    let mut top = shifted(&under, p.rise);
    for t in &mut top.triangles {
        t.swap(1, 2);
    }
    let mut m = Mesh::default();
    m.append(&under);
    m.append(&top);
    let d = p.rise;
    let n = p.polygon.len();
    for i in 0..n {
        let (a, b) = (p.polygon[i], p.polygon[(i + 1) % n]);
        m.quad(a, b, [b[0] + d[0], b[1] + d[1], b[2] + d[2]], [a[0] + d[0], a[1] + d[1], a[2] + d[2]]);
    }
    m
}

/// The solid of a roof piece as the IFC writes it.
pub fn roof_solid(p: &RoofPiece, walls: &[BuiltWall]) -> Mesh {
    match &p.gable {
        Some((wall, t)) => {
            let inward = walls.iter().find(|w| &w.id == wall).map_or([0.0; 3], |w| [-w.frame.n[0] * t, -w.frame.n[1] * t, 0.0]);
            slab_mesh(&p.polygon, inward)
        }
        None => plane_solid(p),
    }
}

/// Explicit pipe assemblies, separate from gutter segments in exchange.
pub fn downpipes(r: &BuiltRoof) -> Vec<(String, Mesh)> {
    r.drainage.iter().filter(|p| p.class == trigon_model::roof::drainage::DrainClass::Downpipe).map(|p| (p.id.clone(), p.mesh.clone())).collect()
}

/// The gutters along the eaves, one per eave.
pub fn gutters(r: &BuiltRoof) -> Vec<(String, Mesh)> {
    if !r.drainage.is_empty() {
        return r
            .drainage
            .iter()
            .filter(|p| p.class == trigon_model::roof::drainage::DrainClass::Gutter)
            .map(|p| (p.id.clone(), p.mesh.clone()))
            .collect();
    }
    if r.gutters.is_empty() {
        return Vec::new();
    }
    boxes(&r.gutters).into_iter().enumerate().map(|(k, m)| (format!("{}.gutter{k}", r.id), m)).collect()
}

/// One storey's staircase: its id as built, level, the doc's stair, and its parts with keys.
pub struct StairPieces<'d> {
    pub id: String,
    pub level: usize,
    pub stair: &'d trigon_model::Stair,
    pub parts: Vec<(String, Part)>,
}

/// The building's staircases storey by storey, named as `build` names them.
pub fn stairs(bd: &Building) -> Vec<StairPieces<'_>> {
    let mut out = Vec::new();
    for (k, st) in bd.stairs.iter().enumerate() {
        for &level in &st.levels {
            let Some(s) = bd.storeys.get(level) else { continue };
            let Ok(parts) = stair::parts(st, s.elevation, s.height) else { continue };
            let base = if st.id.is_empty() { format!("{}.stair{k}", bd.id) } else { st.id.clone() };
            let id = format!("{base}.L{level}");
            let parts = parts
                .into_iter()
                .enumerate()
                .map(|(i, p)| {
                    let what = match p.kind {
                        PartKind::Flight => "flight",
                        PartKind::Winders => "winders",
                        PartKind::Landing => "landing",
                    };
                    (format!("{id}.{what}{i}"), p)
                })
                .collect();
            out.push(StairPieces { id, level, stair: st, parts });
        }
    }
    out
}

/// Wall attachments grouped per balcony (`<slot>.balcony.slab`, `<slot>.balcony.rail` →
/// `<slot>.balcony`); a cellar stair is a group of its own.
pub fn attachment_groups(wall: &BuiltWall) -> Vec<(String, Vec<&BuiltElement>)> {
    let mut groups: Vec<(String, Vec<&BuiltElement>)> = Vec::new();
    for a in &wall.attachments {
        let key = if a.class == ElementClass::Stair { a.id.clone() } else { a.id.rsplit_once('.').map_or(a.id.as_str(), |(k, _)| k).to_string() };
        match groups.iter_mut().find(|(k, _)| *k == key) {
            Some((_, v)) => v.push(a),
            None => groups.push((key, vec![a])),
        }
    }
    groups
}

/// A cellar stair's pieces: the pit's retaining walls, the flight and the railing, by key.
pub fn cellar(e: &BuiltElement) -> Vec<(String, Material, Mesh)> {
    e.parts
        .iter()
        .filter_map(|(m, mesh)| {
            let what = match m {
                Material::Slab => "pit",
                Material::Sill => "flight",
                Material::Railing => "railing",
                _ => return None,
            };
            Some((format!("{}.{what}", e.id), *m, mesh.clone()))
        })
        .collect()
}

/// Every product the IFC export gives its own body, by Trigon id (the GlobalId seed), with
/// its geometry in site mm as Trigon built it. `ifc.check` compares what IFC readers make of
/// the file with these.
pub fn model_shapes(doc: &Document, built: &[Built]) -> Vec<(String, Mesh)> {
    let mut out = Vec::new();
    let merged = |parts: &[(Material, Mesh)]| {
        let mut m = Mesh::default();
        for (_, p) in parts {
            m.append(p);
        }
        m
    };
    let kind_of = |e: &BuiltElement| doc.component(&e.type_id).ok().map(|t| t.kind.clone());
    for b in built {
        if let Some(terrain) = &b.terrain {
            out.push(("site.terrain".into(), terrain.clone()));
        }
        for w in b.exterior_walls() {
            for seg in &w.segments {
                out.push((seg.id.clone(), seg.mesh.clone()));
            }
            for o in &w.openings {
                out.push((o.fill.id.clone(), merged(&o.fill.parts)));
                let attached: Vec<&BuiltElement> = o.attached.iter().collect();
                for p in balcony(&attached, kind_of(&o.fill).as_ref()) {
                    out.push((p.key, merged(&p.parts)));
                }
            }
            for (_, group) in attachment_groups(w) {
                for e in group.iter().filter(|e| e.class == ElementClass::Stair) {
                    out.extend(cellar(e).into_iter().map(|(k, _, m)| (k, m)));
                }
                let kind = group.first().and_then(|e| kind_of(e));
                for p in balcony(&group, kind.as_ref()) {
                    out.push((p.key, merged(&p.parts)));
                }
            }
        }
        for s in &b.slabs {
            out.push((s.id.clone(), s.mesh.clone()));
        }
        if let Some(r) = &b.roof {
            if r.compound {
                out.push((r.id.clone(), r.cover.clone()));
            }
            for p in roof(r, &b.walls) {
                out.push((p.key.clone(), roof_solid(&p, &b.walls)));
            }
            out.extend(gutters(r));
            out.extend(downpipes(r));
            for (material, mesh) in &r.covering {
                out.push((format!("{}.covering.{}", r.id, material.name()), mesh.clone()));
            }
            for d in &r.dormers {
                out.push((format!("{}.roof", d.id), d.roof.clone()));
                out.push((format!("{}.cheeks", d.id), d.cheeks.clone()));
            }
        }
        for iw in &b.interior {
            out.push((iw.id.clone(), iw.mesh.clone()));
            for d in &iw.doors {
                out.push((d.fill.id.clone(), merged(&d.fill.parts)));
            }
        }
        for w in b.exterior_walls() {
            out.push((format!("{}.plinth", w.id), w.plinth.clone()));
            if w.trim_parts.is_empty() {
                out.push((format!("{}.trim", w.id), w.trim.clone()));
            } else {
                out.extend(w.trim_parts.iter().map(|(material, mesh)| (format!("{}.trim.{}", w.id, material.name()), mesh.clone())));
            }
        }
        if let Some(bd) = doc.buildings.iter().find(|x| x.id == b.building_id) {
            for st in stairs(bd) {
                if let Some(built) = b.stairs.iter().find(|s| s.id == st.id) {
                    out.extend(st.parts.into_iter().map(|(k, p)| (k, p.mesh)));
                    out.push((format!("{}.railing", st.id), built.railing.clone()));
                }
            }
        }
    }
    out.retain(|(_, m)| !m.is_empty());
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn boxes_written_face_by_face_split_into_boxes() {
        let mut m = Mesh::default();
        for x0 in [0.0, 100.0] {
            let lo = [[x0, 0.0, 0.0], [x0 + 50.0, 0.0, 0.0], [x0 + 50.0, 50.0, 0.0], [x0, 50.0, 0.0]];
            let hi = lo.map(|p| [p[0], p[1], 50.0]);
            m.hexahedron(lo, hi);
        }
        assert_eq!(components(&m, true).len(), 2);
        assert_eq!(boxes(&m).len(), 2);
        // Without welding, each quad is on its own.
        assert_eq!(components(&m, false).len(), 12);
    }

    #[test]
    fn a_triangulated_polygon_has_one_outline() {
        let mut m = Mesh::default();
        let sq = [[0.0, 0.0], [10.0, 0.0], [10.0, 10.0], [5.0, 12.0], [0.0, 10.0]];
        assert!(m.polygon(&sq, &[], |p| [p[0], p[1], 3.0]));
        let ring = outline(&m).unwrap();
        assert_eq!(ring.len(), 5);
        let n = unit(normal(&ring)).unwrap();
        assert!(n[2].abs() > 0.999);
    }
}
