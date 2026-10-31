//! Model → geometry. Deterministic: the same document always gives the same geometry.
//!
//! Walls are 2.5D: one prism per facade between the outer wall line and its mitred inward
//! offset, with openings cut as 2D holes in the wall faces. No 3D booleans are needed.

use crate::component::{ComponentKind, ComponentType};
use crate::doc::{Building, Document, Facade, FacadeKind, Placement, Storey};
use crate::mesh::{Material, Mesh};
use crate::{ModelError, Result};

/// Window and door frames sit this far behind the outer wall face, mm.
pub const REVEAL: f64 = 120.0;
const FRAME: f64 = 60.0;
const FRAME_DEPTH: f64 = 70.0;
/// Clear wall kept at facade ends and between openings, mm.
const END_MARGIN: f64 = 150.0;
const GAP: f64 = 100.0;
const LINTEL: f64 = 150.0;
const FLOOR_SLAB: f64 = 200.0;
const ROOF_SLAB: f64 = 300.0;
const BALCONY_SLAB: f64 = 180.0;

/// A facade's local frame. `u` runs along the outer wall line from its start, `w` is depth
/// measured inward from the outer face (negative = outside), `v` is height (world Z).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct FacadeFrame {
    pub origin: [f64; 2],
    pub t: [f64; 2],
    /// Outward normal.
    pub n: [f64; 2],
    pub length: f64,
}

impl FacadeFrame {
    pub fn new(a: [f64; 2], b: [f64; 2]) -> Option<Self> {
        let d = [b[0] - a[0], b[1] - a[1]];
        let length = (d[0] * d[0] + d[1] * d[1]).sqrt();
        if !(length > 1.0) {
            return None;
        }
        let t = [d[0] / length, d[1] / length];
        // Counter-clockwise footprint: the outside is on the right of the edge direction.
        Some(Self { origin: a, t, n: [t[1], -t[0]], length })
    }

    pub fn point(&self, u: f64, w: f64, v: f64) -> [f64; 3] {
        [self.origin[0] + u * self.t[0] - w * self.n[0], self.origin[1] + u * self.t[1] - w * self.n[1], v]
    }

    /// Outward compass bearing in degrees (0 = north, 90 = east).
    pub fn bearing(&self) -> f64 {
        self.n[0].atan2(self.n[1]).to_degrees().rem_euclid(360.0)
    }
}

/// A rectangle in a facade's (u, v) plane.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Rect {
    pub u0: f64,
    pub u1: f64,
    pub v0: f64,
    pub v1: f64,
}

impl Rect {
    fn overlaps(&self, o: &Rect, gap: f64) -> bool {
        self.u0 < o.u1 + gap && o.u0 < self.u1 + gap && self.v0 < o.v1 + gap && o.v0 < self.v1 + gap
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ElementClass {
    Window,
    Door,
    Railing,
    BalconySlab,
}

#[derive(Debug, Clone, PartialEq)]
pub struct BuiltElement {
    pub id: String,
    pub name: String,
    pub class: ElementClass,
    pub type_id: String,
    pub level: usize,
    pub width: f64,
    pub height: f64,
    pub parts: Vec<(Material, Mesh)>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct BuiltOpening {
    pub id: String,
    pub level: usize,
    pub rect: Rect,
    /// The window or door filling the opening.
    pub fill: BuiltElement,
    /// Railings and balcony slabs that belong to the opening.
    pub attached: Vec<BuiltElement>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct BuiltWall {
    pub id: String,
    pub frame: FacadeFrame,
    pub kind: FacadeKind,
    pub thickness: f64,
    pub height: f64,
    /// Plan outline: outer start, outer end, inner end, inner start.
    pub plan: [[f64; 2]; 4],
    pub mesh: Mesh,
    pub openings: Vec<BuiltOpening>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SlabKind {
    Floor,
    Roof,
}

#[derive(Debug, Clone, PartialEq)]
pub struct BuiltSlab {
    pub id: String,
    pub kind: SlabKind,
    pub level: usize,
    pub polygon: Vec<[f64; 2]>,
    pub z_bottom: f64,
    pub thickness: f64,
    pub mesh: Mesh,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Built {
    pub building_id: String,
    pub name: String,
    pub storeys: Vec<Storey>,
    pub walls: Vec<BuiltWall>,
    pub slabs: Vec<BuiltSlab>,
}

/// Builds every building in the document.
pub fn build(doc: &Document) -> Result<Vec<Built>> {
    doc.buildings.iter().map(|b| build_building(doc, b)).collect()
}

/// The inner wall line: each footprint vertex moved inward by `d` along the mitre of its two
/// edges. Very sharp corners fall back to a plain offset so the mitre can't run away.
pub fn inner_ring(ring: &[[f64; 2]], d: f64) -> Vec<[f64; 2]> {
    let n = ring.len();
    (0..n)
        .map(|i| {
            let p = ring[i];
            let prev = FacadeFrame::new(ring[(i + n - 1) % n], p);
            let next = FacadeFrame::new(p, ring[(i + 1) % n]);
            let (Some(e0), Some(e1)) = (prev, next) else { return p };
            let plain = [p[0] - e1.n[0] * d, p[1] - e1.n[1] * d];
            // Intersect p0 + s·t0 and p1 + r·t1, both offset inward by d.
            let p0 = [p[0] - e0.n[0] * d, p[1] - e0.n[1] * d];
            let cross = e0.t[0] * e1.t[1] - e0.t[1] * e1.t[0];
            if cross.abs() < 1e-6 {
                return plain;
            }
            let dp = [plain[0] - p0[0], plain[1] - p0[1]];
            let s = (dp[0] * e1.t[1] - dp[1] * e1.t[0]) / cross;
            let q = [p0[0] + s * e0.t[0], p0[1] + s * e0.t[1]];
            let miter = ((q[0] - p[0]).powi(2) + (q[1] - p[1]).powi(2)).sqrt();
            if miter > 5.0 * d { plain } else { q }
        })
        .collect()
}

/// One resolved opening: the placement, its type and its rectangle.
#[derive(Debug, Clone, PartialEq)]
pub struct Resolved {
    pub placement: Placement,
    pub ty: ComponentType,
    pub rect: Rect,
}

/// Resolves and checks a facade's placements: each must fit its storey and the wall, and none
/// may overlap another. The errors say which slot is wrong and why.
pub fn resolve_facade(doc: &Document, b: &Building, f: &Facade) -> Result<Vec<Resolved>> {
    let (a, e) = b.edge(f.edge).ok_or_else(|| ModelError::Invalid(format!("facade {} has no edge", f.id)))?;
    let frame = FacadeFrame::new(a, e).ok_or_else(|| ModelError::Invalid(format!("facade {} has zero length", f.id)))?;
    if f.kind == FacadeKind::Party && !f.placements.is_empty() {
        return Err(ModelError::Invalid(format!("facade {} is a party wall and can't have openings", f.id)));
    }
    let inner = inner_ring(&b.footprint, b.wall_thickness);
    let n = b.footprint.len();
    let along = |q: [f64; 2]| (q[0] - a[0]) * frame.t[0] + (q[1] - a[1]) * frame.t[1];
    let (in0, in1) = match (inner.get(f.edge), inner.get((f.edge + 1) % n)) {
        (Some(&s), Some(&t)) => (along(s), along(t)),
        _ => (0.0, frame.length),
    };
    let lo = in0.max(0.0) + END_MARGIN;
    let hi = in1.min(frame.length) - END_MARGIN;

    let mut out: Vec<Resolved> = Vec::with_capacity(f.placements.len());
    for p in &f.placements {
        let slot = format!("{} level {} bay {}", f.id, p.level, p.bay);
        let ty = doc.component(&p.type_id)?.clone();
        let storey = b.storeys.get(p.level).ok_or_else(|| ModelError::Invalid(format!("{slot}: the building has {} storeys", b.storeys.len())))?;
        let c = *f.bays.get(p.bay).ok_or_else(|| ModelError::Invalid(format!("{slot}: the facade has {} bays", f.bays.len())))?;
        let mut ty_eff = ty.clone();
        ty_eff.width = p.width.unwrap_or(ty.width);
        ty_eff.height = p.height.unwrap_or(ty.height);
        ty_eff.sill = p.sill.unwrap_or(ty.sill);
        // The sill may go below the floor (down to the terrain); everything else as for the type.
        let mut check = ty_eff.clone();
        check.sill = check.sill.max(0.0);
        check.validate().map_err(|e| ModelError::Invalid(format!("{slot}: {e}")))?;
        if !ty_eff.sill.is_finite() || ty_eff.sill < -storey.elevation {
            return Err(ModelError::Invalid(format!(
                "{slot}: sill {} mm would put the opening below the terrain (the floor is {:.0} mm above it)",
                ty_eff.sill, storey.elevation
            )));
        }
        let rect = Rect {
            u0: c - ty_eff.width / 2.0,
            u1: c + ty_eff.width / 2.0,
            v0: storey.elevation + ty_eff.sill,
            v1: storey.elevation + ty_eff.sill + ty_eff.height,
        };
        if rect.u0 < lo || rect.u1 > hi {
            return Err(ModelError::Invalid(format!(
                "{slot}: {} mm wide at {c:.0} mm doesn't fit the wall ({lo:.0} to {hi:.0} mm along the facade)",
                ty_eff.width
            )));
        }
        let top = if p.spans_storeys { b.height() } else { storey.elevation + storey.height };
        if rect.v1 > top - LINTEL {
            return Err(ModelError::Invalid(if p.spans_storeys {
                format!("{slot}: the opening runs to {:.0} mm, less than {LINTEL} mm below the eaves ({:.0} mm)", rect.v1, b.height())
            } else {
                format!(
                    "{slot}: sill {} + height {} leaves less than {LINTEL} mm below the storey above (storey height {}); \
                     stairwell windows that cross a floor need spans_storeys",
                    ty_eff.sill, ty_eff.height, storey.height
                )
            }));
        }
        if let ComponentKind::Balcony { slab_width, .. } = ty_eff.kind
            && (c - slab_width / 2.0 < 0.0 || c + slab_width / 2.0 > frame.length)
        {
            return Err(ModelError::Invalid(format!("{slot}: the {slab_width} mm balcony slab runs past the facade end")));
        }
        if let Some(other) = out.iter().find(|o| o.rect.overlaps(&rect, GAP)) {
            return Err(ModelError::Invalid(format!(
                "{slot}: overlaps the opening at level {} bay {} (keep {GAP} mm between openings)",
                other.placement.level, other.placement.bay
            )));
        }
        out.push(Resolved { placement: p.clone(), ty: ty_eff, rect });
    }
    Ok(out)
}

fn build_building(doc: &Document, b: &Building) -> Result<Built> {
    let h = b.height();
    let d = b.wall_thickness;
    let inner = inner_ring(&b.footprint, d);
    let n = b.footprint.len();
    let mut walls = Vec::with_capacity(b.facades.len());
    for f in &b.facades {
        let (a, e) = b.edge(f.edge).ok_or_else(|| ModelError::Invalid(format!("facade {} has no edge", f.id)))?;
        let fr = FacadeFrame::new(a, e).ok_or_else(|| ModelError::Invalid(format!("facade {} has zero length", f.id)))?;
        let (qs, qe) = match (inner.get(f.edge), inner.get((f.edge + 1) % n)) {
            (Some(&s), Some(&t)) => (s, t),
            _ => return Err(ModelError::Invalid(format!("facade {} has no inner line", f.id))),
        };
        let along = |q: [f64; 2]| (q[0] - a[0]) * fr.t[0] + (q[1] - a[1]) * fr.t[1];
        let (in0, in1) = (along(qs), along(qe));
        let resolved = resolve_facade(doc, b, f)?;

        let holes: Vec<Vec<[f64; 2]>> =
            resolved.iter().map(|r| vec![[r.rect.u0, r.rect.v0], [r.rect.u0, r.rect.v1], [r.rect.u1, r.rect.v1], [r.rect.u1, r.rect.v0]]).collect();
        let mut mesh = Mesh::default();
        let outer_ok = mesh.polygon(&[[0.0, 0.0], [fr.length, 0.0], [fr.length, h], [0.0, h]], &holes, |p| fr.point(p[0], 0.0, p[1]));
        let inner_ok = mesh.polygon(&[[in0, 0.0], [in1, 0.0], [in1, h], [in0, h]], &holes, |p| fr.point(p[0], d, p[1]));
        if !(outer_ok && inner_ok) {
            return Err(ModelError::Invalid(format!("facade {}: the wall faces could not be triangulated", f.id)));
        }
        mesh.quad(fr.point(0.0, 0.0, h), fr.point(fr.length, 0.0, h), fr.point(in1, d, h), fr.point(in0, d, h));
        mesh.quad(fr.point(0.0, 0.0, 0.0), fr.point(in0, d, 0.0), fr.point(in1, d, 0.0), fr.point(fr.length, 0.0, 0.0));
        for r in &resolved {
            let Rect { u0, u1, v0, v1 } = r.rect;
            mesh.quad(fr.point(u0, 0.0, v0), fr.point(u1, 0.0, v0), fr.point(u1, d, v0), fr.point(u0, d, v0));
            mesh.quad(fr.point(u0, 0.0, v1), fr.point(u0, d, v1), fr.point(u1, d, v1), fr.point(u1, 0.0, v1));
            mesh.quad(fr.point(u0, 0.0, v0), fr.point(u0, d, v0), fr.point(u0, d, v1), fr.point(u0, 0.0, v1));
            mesh.quad(fr.point(u1, 0.0, v0), fr.point(u1, 0.0, v1), fr.point(u1, d, v1), fr.point(u1, d, v0));
        }

        let openings = resolved
            .iter()
            .map(|r| {
                let id = format!("{}.L{}.B{}", f.id, r.placement.level, r.placement.bay);
                let elevation = b.storeys.get(r.placement.level).map_or(0.0, |s| s.elevation);
                component(&fr, &id, r, elevation)
            })
            .collect();
        walls.push(BuiltWall { id: f.id.clone(), frame: fr, kind: f.kind, thickness: d, height: h, plan: [a, e, qe, qs], mesh, openings });
    }

    let mut slabs = Vec::with_capacity(b.storeys.len() + 1);
    for (i, s) in b.storeys.iter().enumerate() {
        let z0 = s.elevation - FLOOR_SLAB;
        slabs.push(BuiltSlab {
            id: format!("{}.slab.L{i}", b.id),
            kind: SlabKind::Floor,
            level: i,
            polygon: inner.clone(),
            z_bottom: z0,
            thickness: FLOOR_SLAB,
            mesh: prism(&inner, z0, FLOOR_SLAB),
        });
    }
    slabs.push(BuiltSlab {
        id: format!("{}.roof", b.id),
        kind: SlabKind::Roof,
        level: b.storeys.len().saturating_sub(1),
        polygon: b.footprint.clone(),
        z_bottom: h,
        thickness: ROOF_SLAB,
        mesh: prism(&b.footprint, h, ROOF_SLAB),
    });
    Ok(Built { building_id: b.id.clone(), name: b.name.clone(), storeys: b.storeys.clone(), walls, slabs })
}

/// A vertical prism on a counter-clockwise polygon.
pub fn prism(poly: &[[f64; 2]], z0: f64, height: f64) -> Mesh {
    let mut m = Mesh::default();
    let z1 = z0 + height;
    m.polygon(poly, &[], |p| [p[0], p[1], z1]);
    let mut bottom = Mesh::default();
    bottom.polygon(poly, &[], |p| [p[0], p[1], z0]);
    for t in &mut bottom.triangles {
        t.swap(1, 2);
    }
    m.append(&bottom);
    let n = poly.len();
    for i in 0..n {
        let (a, b) = (poly[i], poly[(i + 1) % n]);
        m.quad([a[0], a[1], z0], [b[0], b[1], z0], [b[0], b[1], z1], [a[0], a[1], z1]);
    }
    m
}

/// A box in facade coordinates.
fn fbox(fr: &FacadeFrame, u: (f64, f64), w: (f64, f64), v: (f64, f64)) -> Mesh {
    let lo = [fr.point(u.0, w.0, v.0), fr.point(u.1, w.0, v.0), fr.point(u.1, w.1, v.0), fr.point(u.0, w.1, v.0)];
    let hi = [fr.point(u.0, w.0, v.1), fr.point(u.1, w.0, v.1), fr.point(u.1, w.1, v.1), fr.point(u.0, w.1, v.1)];
    let mut m = Mesh::default();
    m.hexahedron(lo, hi);
    m
}

/// Vertical bars at even spacing between `u0` and `u1` (`count` bars, not at the ends).
fn dividers(u0: f64, u1: f64, count: u32) -> Vec<f64> {
    (1..count).map(|k| u0 + (u1 - u0) * f64::from(k) / f64::from(count)).collect()
}

/// Frame, sashes or leaves and glazing for a window or door in `rect`.
fn fill_parts(fr: &FacadeFrame, rect: Rect, kind: &ComponentKind) -> Vec<(Material, Mesh)> {
    let Rect { u0, u1, v0, v1 } = rect;
    let w = (REVEAL, REVEAL + FRAME_DEPTH);
    let mut frame = Mesh::default();
    frame.append(&fbox(fr, (u0, u0 + FRAME), w, (v0, v1)));
    frame.append(&fbox(fr, (u1 - FRAME, u1), w, (v0, v1)));
    frame.append(&fbox(fr, (u0 + FRAME, u1 - FRAME), w, (v1 - FRAME, v1)));
    let (iu0, iu1) = (u0 + FRAME, u1 - FRAME);
    let mut glass = Mesh::default();
    let glass_w = (REVEAL + 30.0, REVEAL + 36.0);
    let mut parts = Vec::new();
    match kind {
        ComponentKind::Window { lights } => {
            frame.append(&fbox(fr, (iu0, iu1), w, (v0, v0 + FRAME)));
            for c in dividers(iu0, iu1, *lights) {
                frame.append(&fbox(fr, (c - FRAME / 2.0, c + FRAME / 2.0), w, (v0 + FRAME, v1 - FRAME)));
            }
            glass.append(&fbox(fr, (iu0, iu1), glass_w, (v0 + FRAME, v1 - FRAME)));
            parts.push((Material::Sill, fbox(fr, (u0 - 30.0, u1 + 30.0), (-40.0, REVEAL), (v0 - 40.0, v0))));
        }
        ComponentKind::FixedWindow => {
            frame.append(&fbox(fr, (iu0, iu1), w, (v0, v0 + FRAME)));
            glass.append(&fbox(fr, (iu0, iu1), glass_w, (v0 + FRAME, v1 - FRAME)));
            parts.push((Material::Sill, fbox(fr, (u0 - 30.0, u1 + 30.0), (-40.0, REVEAL), (v0 - 40.0, v0))));
        }
        ComponentKind::EntranceDoor => {
            parts.push((Material::Door, fbox(fr, (iu0, iu1), (REVEAL + 15.0, REVEAL + 60.0), (v0, v1 - FRAME))));
        }
        ComponentKind::FrenchDoor { leaves } | ComponentKind::FrenchBalcony { leaves, .. } | ComponentKind::Balcony { leaves, .. } => {
            let panel = 400.0_f64.min((v1 - v0) / 3.0);
            frame.append(&fbox(fr, (iu0, iu1), w, (v0, v0 + panel)));
            for c in dividers(iu0, iu1, *leaves) {
                frame.append(&fbox(fr, (c - FRAME / 2.0, c + FRAME / 2.0), w, (v0 + panel, v1 - FRAME)));
            }
            glass.append(&fbox(fr, (iu0, iu1), glass_w, (v0 + panel, v1 - FRAME)));
        }
    }
    parts.push((Material::Frame, frame));
    if !glass.is_empty() {
        parts.push((Material::Glass, glass));
    }
    parts
}

/// A guard rail along a straight run in facade coordinates: posts at the ends, top and
/// bottom rails, balusters at 110 mm centres.
fn rail_run(fr: &FacadeFrame, from: (f64, f64), to: (f64, f64), z0: f64, height: f64) -> Mesh {
    let mut m = Mesh::default();
    let (du, dw) = (to.0 - from.0, to.1 - from.1);
    let len = (du * du + dw * dw).sqrt();
    if !(len > 1.0) {
        return m;
    }
    let at = |s: f64| (from.0 + du * s / len, from.1 + dw * s / len);
    let bar = |m: &mut Mesh, s: f64, size: f64, v: (f64, f64)| {
        let (u, w) = at(s);
        m.append(&fbox(fr, (u - size / 2.0, u + size / 2.0), (w - size / 2.0, w + size / 2.0), v));
    };
    bar(&mut m, 20.0, 40.0, (z0, z0 + height));
    bar(&mut m, len - 20.0, 40.0, (z0, z0 + height));
    let count = (len / 110.0).floor() as u32;
    for k in 1..count {
        bar(&mut m, len * f64::from(k) / f64::from(count), 16.0, (z0 + 100.0, z0 + height - 40.0));
    }
    // Rails as thin boxes along the run (axis-aligned in u or w; runs here always are).
    let (ua, ub) = (from.0.min(to.0) - 20.0, from.0.max(to.0) + 20.0);
    let (wa, wb) = (from.1.min(to.1) - 20.0, from.1.max(to.1) + 20.0);
    m.append(&fbox(fr, (ua, ub), (wa, wb), (z0 + height - 40.0, z0 + height)));
    m.append(&fbox(fr, (ua, ub), (wa, wb), (z0 + 100.0, z0 + 130.0)));
    m
}

fn component(fr: &FacadeFrame, id: &str, r: &Resolved, elevation: f64) -> BuiltOpening {
    let ty = &r.ty;
    let class = if ty.kind.is_door() { ElementClass::Door } else { ElementClass::Window };
    let level = r.placement.level;
    let fill = BuiltElement {
        id: id.to_string(),
        name: ty.name.clone(),
        class,
        type_id: ty.id.clone(),
        level,
        width: ty.width,
        height: ty.height,
        parts: fill_parts(fr, r.rect, &ty.kind),
    };
    let mut attached = Vec::new();
    let Rect { u0, u1, v0, .. } = r.rect;
    match ty.kind {
        ComponentKind::FrenchBalcony { rail_height, .. } => {
            let rail = rail_run(fr, (u0 - 50.0, -130.0), (u1 + 50.0, -130.0), v0, rail_height);
            attached.push(element(id, "rail", "French balcony rail", ElementClass::Railing, ty, level, rail));
        }
        ComponentKind::Balcony { depth, slab_width, rail_height, .. } => {
            let c = (u0 + u1) / 2.0;
            let (s0, s1) = (c - slab_width / 2.0, c + slab_width / 2.0);
            let slab = fbox(fr, (s0, s1), (-depth, 0.0), (elevation - BALCONY_SLAB, elevation));
            attached.push(element(id, "slab", "Balcony slab", ElementClass::BalconySlab, ty, level, slab));
            let mut rail = rail_run(fr, (s0 + 40.0, -depth + 40.0), (s1 - 40.0, -depth + 40.0), elevation, rail_height);
            rail.append(&rail_run(fr, (s0 + 40.0, -depth + 40.0), (s0 + 40.0, -60.0), elevation, rail_height));
            rail.append(&rail_run(fr, (s1 - 40.0, -depth + 40.0), (s1 - 40.0, -60.0), elevation, rail_height));
            attached.push(element(id, "rail", "Balcony rail", ElementClass::Railing, ty, level, rail));
        }
        _ => {}
    }
    BuiltOpening { id: format!("{id}.opening"), level, rect: r.rect, fill, attached }
}

fn element(id: &str, suffix: &str, name: &str, class: ElementClass, ty: &ComponentType, level: usize, mesh: Mesh) -> BuiltElement {
    let material = if class == ElementClass::Railing { Material::Railing } else { Material::Slab };
    BuiltElement {
        id: format!("{id}.{suffix}"),
        name: name.to_string(),
        class,
        type_id: ty.id.clone(),
        level,
        width: ty.width,
        height: ty.height,
        parts: vec![(material, mesh)],
    }
}

/// Evenly spaced bay centres along a facade: `count` bays in the usable length.
pub fn even_bays(length: f64, count: usize, margin: f64) -> Vec<f64> {
    let usable = (length - 2.0 * margin).max(0.0);
    let step = usable / count.max(1) as f64;
    (0..count).map(|i| margin + step * (i as f64 + 0.5)).collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::doc::Building;

    fn doc_with_block() -> Document {
        let mut doc = Document::default();
        let fp = [[0.0, 0.0], [20000.0, 0.0], [20000.0, 12000.0], [0.0, 12000.0]];
        let mut b = Building::new("b1", "Test", &fp, 5, 3000.0).unwrap();
        let f = &mut b.facades[0];
        f.bays = even_bays(20000.0, 6, 600.0);
        for level in 0..5 {
            for bay in 0..6 {
                let ty = match (level, bay) {
                    (0, 0) => "door.entrance",
                    (0, _) => "window.casement.2",
                    (_, 2) => "balcony.cantilever",
                    (_, 4) => "balcony.french",
                    _ => "window.casement.2",
                };
                f.placements.push(Placement { level, bay, type_id: ty.into(), width: None, height: None, sill: None, spans_storeys: false });
            }
        }
        doc.buildings.push(b);
        doc
    }

    #[test]
    fn block_builds_with_openings_and_balconies() {
        let doc = doc_with_block();
        let built = build(&doc).unwrap();
        let b = &built[0];
        assert_eq!(b.walls.len(), 4);
        assert_eq!(b.walls[0].openings.len(), 30);
        assert_eq!(b.slabs.len(), 6);
        let balconies = b.walls[0].openings.iter().filter(|o| o.attached.iter().any(|a| a.class == ElementClass::BalconySlab)).count();
        assert_eq!(balconies, 4);
        // Wall geometry stays inside the building's extent plus nothing outward.
        let (lo, hi) = b.walls[0].mesh.bounds().unwrap();
        assert!(lo[1] >= -1e-6 && hi[1] <= 400.0 + 1e-6, "{lo:?} {hi:?}");
    }

    #[test]
    fn overlapping_and_oversized_openings_are_refused() {
        let mut doc = doc_with_block();
        doc.buildings[0].facades[0].placements[1].width = Some(6000.0);
        let err = build(&doc).unwrap_err();
        assert!(format!("{err}").contains("overlaps"), "{err}");
        let mut doc = doc_with_block();
        doc.buildings[0].facades[0].placements[1].height = Some(2900.0);
        assert!(format!("{}", build(&doc).unwrap_err()).contains("storey above"));
    }

    #[test]
    fn raised_ground_floor_stairwells_and_openings_below_the_floor() {
        let mut doc = doc_with_block();
        let b = &mut doc.buildings[0];
        b.set_storeys_from(5, 3000.0, 900.0).unwrap();
        assert_eq!(b.height(), 15900.0);
        assert_eq!(b.storeys[1].elevation, 3900.0);
        let f = &mut b.facades[0];
        f.placements.clear();
        // A landing window: 2580 mm above the ground storey's floor, 950 high, crosses the floor above.
        let landing = Placement {
            level: 0,
            bay: 5,
            type_id: "window.fixed".into(),
            width: Some(1000.0),
            height: Some(950.0),
            sill: Some(2580.0),
            spans_storeys: false,
        };
        f.placements.push(landing.clone());
        let err = build(&doc).unwrap_err().to_string();
        assert!(err.contains("spans_storeys"), "{err}");
        doc.buildings[0].facades[0].placements[0].spans_storeys = true;
        let built = build(&doc).unwrap();
        assert_eq!(built[0].walls[0].openings[0].rect.v0, 900.0 + 2580.0);
        // A door down to the courtyard: below the floor is fine, below the terrain is not.
        let door = Placement { level: 0, bay: 0, type_id: "door.french".into(), width: None, height: None, sill: Some(-500.0), spans_storeys: false };
        doc.buildings[0].facades[0].placements.push(door);
        assert_eq!(build(&doc).unwrap()[0].walls[0].openings[1].rect.v0, 400.0);
        doc.buildings[0].facades[0].placements[1].sill = Some(-1000.0);
        assert!(build(&doc).unwrap_err().to_string().contains("below the terrain"));
    }

    #[test]
    fn party_walls_refuse_openings() {
        let mut doc = doc_with_block();
        doc.buildings[0].facades[0].kind = FacadeKind::Party;
        assert!(build(&doc).is_err());
    }

    #[test]
    fn inner_ring_of_a_rectangle_is_offset_by_the_thickness() {
        let r = inner_ring(&[[0.0, 0.0], [10000.0, 0.0], [10000.0, 5000.0], [0.0, 5000.0]], 400.0);
        assert_eq!(r[0], [400.0, 400.0]);
        assert_eq!(r[2], [9600.0, 4600.0]);
    }

    #[test]
    fn bearing_of_a_south_facing_facade_is_180() {
        let f = FacadeFrame::new([0.0, 0.0], [10.0, 0.0]).unwrap();
        assert!((f.bearing() - 180.0).abs() < 1e-9);
    }
}
