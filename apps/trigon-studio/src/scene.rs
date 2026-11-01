//! The model as the viewport draws it: the built buildings turned into flat-shaded triangles with
//! per-material colours, the feature edges (creases and borders, not the triangulation), one
//! entry per element for picking, properties and before/after diffs. Pure data, no GPU.
//!
//! Coordinates are metres relative to [`Scene::origin`] (site mm), so f32 keeps sub-millimetre
//! precision; Z is up as in the model.

use std::collections::HashMap;
use std::collections::hash_map::DefaultHasher;
use std::hash::{Hash, Hasher};
use std::ops::Range;

use trigon_model::build::Status;
use trigon_model::{Built, BuiltElement, Material, Mesh, SlabKind};

/// Floats per triangle vertex: position (3), normal (3), linear RGBA (4), masonry UV/enable (3).
pub const VERTEX_FLOATS: usize = 13;
/// Floats per edge-quad vertex: both ends (3 + 3), along (0 or 1), side (-1 or 1).
pub const EDGE_FLOATS: usize = 8;
/// Two faces meeting at more than this angle draw an edge between them.
const CREASE_DEG: f64 = 20.0;
/// Welding grid for edge detection, mm.
const WELD_MM: f64 = 0.5;

const NEW_TINT: [f32; 3] = [0.20, 0.62, 0.22];
const PARTITION: [f32; 4] = [0.80, 0.79, 0.76, 1.0];
const BEARING: [f32; 4] = [0.56, 0.55, 0.52, 1.0];

/// One pickable element.
#[derive(Debug, Clone)]
pub struct Element {
    pub id: String,
    pub kind: &'static str,
    pub level: Option<usize>,
    pub props: Vec<(&'static str, String)>,
    /// Its triangles' vertex ranges in [`Scene::vertices`] (opaque and translucent parts).
    pub ranges: Vec<Range<u32>>,
    /// Changes whenever its geometry or colour does.
    pub fingerprint: u64,
    /// A wall or slab: it changes when something is placed in it, so a diff names it only when
    /// nothing else changed.
    pub host: bool,
    /// Its box, scene metres.
    pub bounds: Option<([f32; 3], [f32; 3])>,
    /// The outward direction (x, y) of the facade it is in or on.
    pub facing: Option<[f32; 2]>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Level {
    pub name: String,
    /// Floor level, site mm.
    pub elevation: f64,
    pub height: f64,
}

#[derive(Debug, Clone, Default)]
pub struct Scene {
    /// Subtracted from site coordinates (mm) before scaling to metres.
    pub origin: [f64; 3],
    /// Triangle vertices, [`VERTEX_FLOATS`] each: the opaque ones first.
    pub vertices: Vec<f32>,
    /// How many of the vertices are opaque.
    pub opaque: u32,
    /// Edge-quad vertices, [`EDGE_FLOATS`] each, six per edge.
    pub edges: Vec<f32>,
    pub elements: Vec<Element>,
    /// The element of each triangle.
    pub tri_element: Vec<u32>,
    pub bounds: Option<([f32; 3], [f32; 3])>,
    pub levels: Vec<Level>,
}

/// A part of an element: its mesh and colour.
struct Part<'a> {
    mesh: &'a Mesh,
    rgba: [f32; 4],
    material: Material,
}

fn status_rgba(m: Material, status: Status, phases: bool) -> [f32; 4] {
    match status {
        Status::Demolished => Material::Demolished.rgba(),
        Status::New if phases => {
            let c = m.rgba();
            let mix = |a: f32, b: f32| a * 0.45 + b * 0.55;
            [mix(c[0], NEW_TINT[0]), mix(c[1], NEW_TINT[1]), mix(c[2], NEW_TINT[2]), c[3]]
        }
        _ => m.rgba(),
    }
}

fn element_kind(e: &BuiltElement) -> &'static str {
    match e.class {
        trigon_model::ElementClass::Window => "window",
        trigon_model::ElementClass::Door => "door",
        trigon_model::ElementClass::Railing => "railing",
        trigon_model::ElementClass::BalconySlab => "balcony",
        trigon_model::ElementClass::Stair => "cellar stair",
    }
}

fn mm(v: f64) -> String {
    format!("{v:.0} mm")
}

fn polygon_area(p: &[[f64; 2]]) -> f64 {
    let n = p.len();
    (0..n).map(|i| p[i][0] * p[(i + 1) % n][1] - p[(i + 1) % n][0] * p[i][1]).sum::<f64>().abs() / 2.0
}

struct Builder {
    origin: [f64; 3],
    opaque: Vec<f32>,
    clear: Vec<f32>,
    opaque_tris: Vec<u32>,
    clear_tris: Vec<u32>,
    edges: Vec<f32>,
    elements: Vec<Element>,
    /// Per element: ranges in `opaque` and in `clear` (vertex indices), fixed up at the end.
    raw: Vec<(Vec<Range<u32>>, Vec<Range<u32>>)>,
    lo: [f32; 3],
    hi: [f32; 3],
}

impl Builder {
    fn local(&self, p: [f64; 3]) -> [f32; 3] {
        [((p[0] - self.origin[0]) / 1000.0) as f32, ((p[1] - self.origin[1]) / 1000.0) as f32, ((p[2] - self.origin[2]) / 1000.0) as f32]
    }

    fn add(&mut self, id: &str, kind: &'static str, level: Option<usize>, props: Vec<(&'static str, String)>, host: bool, parts: &[Part<'_>]) {
        let index = self.elements.len() as u32;
        let (mut lo, mut hi) = ([f32::MAX; 3], [f32::MIN; 3]);
        let mut hasher = DefaultHasher::new();
        kind.hash(&mut hasher);
        let (mut opaque_ranges, mut clear_ranges) = (Vec::new(), Vec::new());
        for part in parts {
            let m = part.mesh;
            for c in part.rgba {
                c.to_bits().hash(&mut hasher);
            }
            part.material.hash(&mut hasher);
            let translucent = part.rgba[3] < 0.999;
            let start = if translucent { self.clear.len() } else { self.opaque.len() } / VERTEX_FLOATS;
            for t in &m.triangles {
                let (Some(a), Some(b), Some(c)) = (m.positions.get(t[0] as usize), m.positions.get(t[1] as usize), m.positions.get(t[2] as usize))
                else {
                    continue;
                };
                for p in [a, b, c] {
                    for k in p {
                        k.to_bits().hash(&mut hasher);
                    }
                }
                let (pa, pb, pc) = (self.local(*a), self.local(*b), self.local(*c));
                let n = normal(pa, pb, pc);
                // Texture coordinates stay anchored to original site millimetres, independent of
                // camera movement and the scene origin used for f32 rendering precision.
                let ab = [b[0] - a[0], b[1] - a[1], b[2] - a[2]];
                let ac = [c[0] - a[0], c[1] - a[1], c[2] - a[2]];
                let surface_normal = [ab[1] * ac[2] - ab[2] * ac[1], ab[2] * ac[0] - ab[0] * ac[2], ab[0] * ac[1] - ab[1] * ac[0]];
                let out = if translucent { &mut self.clear } else { &mut self.opaque };
                for (p, source) in [pa, pb, pc].into_iter().zip([a, b, c]) {
                    out.extend_from_slice(&p);
                    out.extend_from_slice(&n);
                    out.extend_from_slice(&part.rgba);
                    let uv = part.material.masonry_uv(*source, surface_normal);
                    let [u, v] = uv.unwrap_or([0.0; 2]);
                    out.extend_from_slice(&[u, v, if uv.is_some() && !translucent { 1.0 } else { 0.0 }]);
                    for (k, v) in p.iter().enumerate() {
                        lo[k] = lo[k].min(*v);
                        hi[k] = hi[k].max(*v);
                    }
                }
                if translucent { &mut self.clear_tris } else { &mut self.opaque_tris }.push(index);
            }
            let end = if translucent { self.clear.len() } else { self.opaque.len() } / VERTEX_FLOATS;
            if end > start {
                let r = start as u32..end as u32;
                if translucent { clear_ranges.push(r) } else { opaque_ranges.push(r) }
            }
            for (a, b) in feature_edges(&m.positions, &m.triangles, CREASE_DEG) {
                let (pa, pb) = (self.local(a), self.local(b));
                for (along, side) in [(0.0, -1.0), (1.0, -1.0), (1.0, 1.0), (0.0, -1.0), (1.0, 1.0), (0.0, 1.0)] {
                    self.edges.extend_from_slice(&pa);
                    self.edges.extend_from_slice(&pb);
                    self.edges.extend_from_slice(&[along, side]);
                }
            }
        }
        self.raw.push((opaque_ranges, clear_ranges));
        let bounds = (lo[0] <= hi[0]).then_some((lo, hi));
        if let Some((l, h)) = bounds {
            for k in 0..3 {
                self.lo[k] = self.lo[k].min(l[k]);
                self.hi[k] = self.hi[k].max(h[k]);
            }
        }
        let element =
            Element { id: id.to_string(), kind, level, props, ranges: Vec::new(), fingerprint: hasher.finish(), host, bounds, facing: None };
        self.elements.push(element);
    }

    /// Mark the elements added since `from` as facing `n`.
    fn facing(&mut self, from: usize, n: [f64; 2]) {
        for e in self.elements.iter_mut().skip(from) {
            e.facing = Some([n[0] as f32, n[1] as f32]);
        }
    }

    fn finish(mut self, levels: Vec<Level>) -> Scene {
        let opaque = (self.opaque.len() / VERTEX_FLOATS) as u32;
        for (e, (o, c)) in self.elements.iter_mut().zip(self.raw) {
            e.ranges = o.into_iter().chain(c.into_iter().map(|r| r.start + opaque..r.end + opaque)).collect();
        }
        let bounds = (self.lo[0] <= self.hi[0]).then_some((self.lo, self.hi));
        let mut vertices = self.opaque;
        vertices.extend_from_slice(&self.clear);
        let mut tri_element = self.opaque_tris;
        tri_element.extend_from_slice(&self.clear_tris);
        Scene { origin: self.origin, vertices, opaque, edges: self.edges, elements: self.elements, tri_element, bounds, levels }
    }
}

fn normal(a: [f32; 3], b: [f32; 3], c: [f32; 3]) -> [f32; 3] {
    let u = [b[0] - a[0], b[1] - a[1], b[2] - a[2]];
    let v = [c[0] - a[0], c[1] - a[1], c[2] - a[2]];
    let n = [u[1] * v[2] - u[2] * v[1], u[2] * v[0] - u[0] * v[2], u[0] * v[1] - u[1] * v[0]];
    let len = (n[0] * n[0] + n[1] * n[1] + n[2] * n[2]).sqrt();
    if len > 0.0 && len.is_finite() { [n[0] / len, n[1] / len, n[2] / len] } else { [0.0, 0.0, 1.0] }
}

/// The edges worth drawing: borders (one face) and creases (faces meeting at more than
/// `crease_deg`, either winding), never the diagonals of a triangulated flat face. Vertices are
/// welded on a fine grid first, since the model's meshes repeat corners per face.
pub fn feature_edges(positions: &[[f64; 3]], triangles: &[[u32; 3]], crease_deg: f64) -> Vec<([f64; 3], [f64; 3])> {
    let mut weld: HashMap<[i64; 3], u32> = HashMap::new();
    let mut points: Vec<[f64; 3]> = Vec::new();
    let welded: Vec<Option<u32>> = positions
        .iter()
        .map(|p| {
            if !p.iter().all(|v| v.is_finite()) {
                return None;
            }
            let key = [(p[0] / WELD_MM).round() as i64, (p[1] / WELD_MM).round() as i64, (p[2] / WELD_MM).round() as i64];
            Some(*weld.entry(key).or_insert_with(|| {
                points.push(*p);
                (points.len() - 1) as u32
            }))
        })
        .collect();
    let id = |i: u32| welded.get(i as usize).copied().flatten();
    let mut faces: Vec<[f64; 3]> = Vec::new();
    let mut sides: Vec<(u32, u32, u32)> = Vec::new();
    for t in triangles {
        let (Some(a), Some(b), Some(c)) = (id(t[0]), id(t[1]), id(t[2])) else { continue };
        if a == b || b == c || a == c {
            continue;
        }
        let (pa, pb, pc) = (points[a as usize], points[b as usize], points[c as usize]);
        let u = [pb[0] - pa[0], pb[1] - pa[1], pb[2] - pa[2]];
        let v = [pc[0] - pa[0], pc[1] - pa[1], pc[2] - pa[2]];
        let n = [u[1] * v[2] - u[2] * v[1], u[2] * v[0] - u[0] * v[2], u[0] * v[1] - u[1] * v[0]];
        let len = (n[0] * n[0] + n[1] * n[1] + n[2] * n[2]).sqrt();
        if !(len > 1e-9) {
            continue;
        }
        let f = faces.len() as u32;
        faces.push([n[0] / len, n[1] / len, n[2] / len]);
        for (p, q) in [(a, b), (b, c), (c, a)] {
            sides.push((p.min(q), p.max(q), f));
        }
    }
    sides.sort_unstable();
    let cos = crease_deg.to_radians().cos();
    let mut out = Vec::new();
    let mut i = 0;
    while i < sides.len() {
        let (p, q, f0) = sides[i];
        let mut j = i + 1;
        while j < sides.len() && sides[j].0 == p && sides[j].1 == q {
            j += 1;
        }
        let keep = match j - i {
            1 => true,
            2 => {
                let (n0, n1) = (faces[f0 as usize], faces[sides[i + 1].2 as usize]);
                (n0[0] * n1[0] + n0[1] * n1[1] + n0[2] * n1[2]).abs() < cos
            }
            _ => true,
        };
        if keep {
            out.push((points[p as usize], points[q as usize]));
        }
        i = j;
    }
    out
}

impl Scene {
    /// The viewport's scene of the built buildings.
    pub fn from_built(built: &[Built], phases: bool) -> Scene {
        // The first building's footprint centre at its ground, so coordinates stay small.
        let origin = built
            .iter()
            .flat_map(|b| b.walls.iter().flat_map(|w| w.mesh.bounds()))
            .reduce(|(a, b), (c, d)| ([a[0].min(c[0]), a[1].min(c[1]), a[2].min(c[2])], [b[0].max(d[0]), b[1].max(d[1]), b[2].max(d[2])]))
            .map(|(lo, hi)| [(lo[0] + hi[0]) / 2.0, (lo[1] + hi[1]) / 2.0, lo[2]])
            .unwrap_or_default();
        let mut b = Builder {
            origin,
            opaque: Vec::new(),
            clear: Vec::new(),
            opaque_tris: Vec::new(),
            clear_tris: Vec::new(),
            edges: Vec::new(),
            elements: Vec::new(),
            raw: Vec::new(),
            lo: [f32::MAX; 3],
            hi: [f32::MIN; 3],
        };
        let mut levels = Vec::new();
        for building in built {
            if let Some(terrain) = &building.terrain {
                b.add(
                    "site.terrain",
                    "terrain",
                    None,
                    vec![("units", "site-local mm; site datum".into())],
                    false,
                    &[Part { mesh: terrain, rgba: Material::Ground.rgba(), material: Material::Ground }],
                );
            }
            if levels.is_empty() {
                levels = building.storeys.iter().map(|s| Level { name: s.name.clone(), elevation: s.elevation, height: s.height }).collect();
            }
            for w in building.exterior_walls() {
                let first = b.elements.len();
                let length = w.frame.length;
                b.add(
                    &w.id,
                    "wall",
                    None,
                    vec![
                        ("facade", format!("{:?}", w.kind).to_lowercase()),
                        ("length", mm(length)),
                        ("thickness", w.segments.iter().map(|s| mm(s.thickness)).collect::<Vec<_>>().join(" / ")),
                        ("from", mm(w.bottom)),
                        ("to (eaves)", mm(w.height)),
                        ("openings", w.openings.len().to_string()),
                        ("bearing", format!("{:.0}°", w.frame.bearing())),
                    ],
                    true,
                    &[
                        Part { mesh: &w.mesh, rgba: w.finish.rgba(), material: w.finish },
                        Part { mesh: &w.plinth, rgba: Material::Plinth.rgba(), material: Material::Plinth },
                        Part { mesh: &w.trim, rgba: Material::Render.rgba(), material: Material::Render },
                    ],
                );
                let elements = w.openings.iter().flat_map(|o| std::iter::once(&o.fill).chain(&o.attached)).chain(&w.attachments);
                for e in elements {
                    let parts: Vec<Part<'_>> =
                        e.parts.iter().map(|(m, mesh)| Part { mesh, rgba: status_rgba(*m, e.status, phases), material: *m }).collect();
                    b.add(
                        &e.id,
                        element_kind(e),
                        Some(e.level),
                        vec![
                            ("building", building.building_id.clone()),
                            ("type", e.type_id.clone()),
                            ("name", e.name.clone()),
                            ("facade", w.id.clone()),
                            ("width", mm(e.width)),
                            ("height", mm(e.height)),
                            ("phase status", e.status.name().to_string()),
                        ],
                        false,
                        &parts,
                    );
                }
                b.facing(first, w.frame.n);
            }
            for s in &building.slabs {
                let (kind, m) = match s.kind {
                    SlabKind::Floor => ("floor slab", Material::Slab),
                    SlabKind::Roof => ("roof slab", Material::Roof),
                };
                let area = polygon_area(&s.polygon) - s.holes.iter().map(|h| polygon_area(h)).sum::<f64>();
                b.add(
                    &s.id,
                    kind,
                    Some(s.level),
                    vec![("thickness", mm(s.thickness)), ("underside", mm(s.z_bottom)), ("area", format!("{:.1} m²", area / 1e6))],
                    true,
                    &[Part { mesh: &s.mesh, rgba: m.rgba(), material: m }],
                );
            }
            if let Some(r) = &building.roof {
                let mut parts = vec![
                    Part { mesh: &r.cover, rgba: Material::Roof.rgba(), material: Material::Roof },
                    Part { mesh: &r.gutters, rgba: Material::Steel.rgba(), material: Material::Steel },
                ];
                if r.gable_parts.is_empty() {
                    parts.insert(1, Part { mesh: &r.gables, rgba: Material::Wall.rgba(), material: Material::Wall });
                } else {
                    parts.extend(r.gable_parts.iter().map(|(material, mesh)| Part { mesh, rgba: material.rgba(), material: *material }));
                }
                parts.extend(r.covering.iter().map(|(material, mesh)| Part { mesh, rgba: material.rgba(), material: *material }));
                b.add(
                    &r.id,
                    "roof",
                    None,
                    vec![("eaves", mm(r.eaves)), ("ridge", mm(r.ridge)), ("pitch", format!("{:.1}°", r.pitch_deg))],
                    true,
                    &parts,
                );
                for d in &r.dormers {
                    for (suffix, kind, mesh) in [("roof", "dormer roof", &d.roof), ("cheeks", "dormer cheeks", &d.cheeks)] {
                        b.add(
                            &format!("{}.{suffix}", d.id),
                            kind,
                            None,
                            Vec::new(),
                            true,
                            &[Part { mesh, rgba: Material::Steel.rgba(), material: Material::Steel }],
                        );
                    }
                }
            }
            for iw in &building.interior {
                let len = (iw.outline[1][0] - iw.outline[0][0]).hypot(iw.outline[1][1] - iw.outline[0][1]);
                b.add(
                    &iw.id,
                    "interior wall",
                    Some(iw.level),
                    vec![
                        ("load-bearing", if iw.load_bearing { "yes" } else { "no (partition)" }.to_string()),
                        ("length", mm(len)),
                        ("height", mm(iw.height)),
                        ("from", mm(iw.z_bottom)),
                    ],
                    false,
                    &[Part { mesh: &iw.mesh, rgba: if iw.load_bearing { BEARING } else { PARTITION }, material: Material::Plaster }],
                );
                for door in &iw.doors {
                    let e = &door.fill;
                    let parts: Vec<Part<'_>> =
                        e.parts.iter().map(|(m, mesh)| Part { mesh, rgba: status_rgba(*m, e.status, phases), material: *m }).collect();
                    b.add(
                        &e.id,
                        "door",
                        Some(e.level),
                        vec![
                            ("building", building.building_id.clone()),
                            ("type", e.type_id.clone()),
                            ("name", e.name.clone()),
                            ("host", iw.id.clone()),
                            ("width", mm(e.width)),
                            ("height", mm(e.height)),
                            ("phase status", e.status.name().to_string()),
                        ],
                        false,
                        &parts,
                    );
                }
            }
            for st in &building.stairs {
                b.add(
                    &st.id,
                    "stair",
                    Some(st.level),
                    vec![("layout", format!("{:?}", st.kind)), ("winding", if st.winding { "yes" } else { "no" }.to_string())],
                    false,
                    &[
                        Part { mesh: &st.mesh, rgba: Material::Sill.rgba(), material: Material::Sill },
                        Part { mesh: &st.railing, rgba: Material::Railing.rgba(), material: Material::Railing },
                    ],
                );
            }
        }
        b.finish(levels)
    }

    /// Rooms are semantic spaces, not opaque boxes. Invisible floor polygons let a room be
    /// picked in a cut view; only its selection overlay colours the space.
    pub fn with_rooms(mut self, doc: &trigon_model::Document) -> Self {
        for building in &doc.buildings {
            for (room_index, room) in building.rooms.iter().enumerate() {
                for level in &room.levels {
                    let Some(storey) = building.storeys.get(*level) else { continue };
                    let mut mesh = Mesh::default();
                    if !mesh.polygon(&room.polygon, &[], |p| [p[0], p[1], storey.elevation + 0.5]) {
                        continue;
                    }
                    let index = self.elements.len() as u32;
                    let start = self.vertices.len() / VERTEX_FLOATS;
                    let (mut lo, mut hi) = ([f32::MAX; 3], [f32::MIN; 3]);
                    let mut fingerprint = DefaultHasher::new();
                    room.name.hash(&mut fingerprint);
                    for triangle in &mesh.triangles {
                        let [Some(a), Some(b), Some(c)] = triangle.map(|i| mesh.positions.get(i as usize)) else { continue };
                        for source in [a, b, c] {
                            let position = std::array::from_fn::<_, 3, _>(|axis| ((source[axis] - self.origin[axis]) / 1000.0) as f32);
                            self.vertices.extend_from_slice(&position);
                            self.vertices.extend_from_slice(&[0.0, 0.0, 1.0]);
                            self.vertices.extend_from_slice(&[0.0; 7]);
                            for axis in 0..3 {
                                source[axis].to_bits().hash(&mut fingerprint);
                                lo[axis] = lo[axis].min(position[axis]);
                                hi[axis] = hi[axis].max(position[axis]);
                            }
                        }
                        self.tri_element.push(index);
                    }
                    let end = self.vertices.len() / VERTEX_FLOATS;
                    if start == end {
                        continue;
                    }
                    let id = trigon_engine::room_element_id(building, room_index, *level);
                    self.elements.push(Element {
                        id,
                        kind: "room",
                        level: Some(*level),
                        props: vec![
                            ("building", building.id.clone()),
                            ("name", room.name.clone()),
                            ("area", format!("{:.2} m²", polygon_area(&room.polygon) / 1e6)),
                        ],
                        ranges: std::iter::once(start as u32..end as u32).collect(),
                        fingerprint: fingerprint.finish(),
                        host: false,
                        bounds: Some((lo, hi)),
                        facing: None,
                    });
                    self.bounds = Some(self.bounds.map_or((lo, hi), |(a, b)| {
                        (std::array::from_fn(|axis| a[axis].min(lo[axis])), std::array::from_fn(|axis| b[axis].max(hi[axis])))
                    }));
                }
            }
        }
        self
    }

    pub fn triangle_count(&self) -> usize {
        self.tri_element.len()
    }

    pub fn edge_count(&self) -> usize {
        self.edges.len() / (EDGE_FLOATS * 6)
    }

    /// The cut height (scene metres) that shows levels up to `level`: 1.2 m above its floor.
    pub fn clip_z(&self, level: usize) -> Option<f32> {
        let l = self.levels.get(level)?;
        Some(((l.elevation + 1200.0 - self.origin[2]) / 1000.0) as f32)
    }

    /// The nearest element hit by the ray `o + t·d` (scene metres), ignoring what lies above `clip`.
    pub fn pick(&self, o: [f32; 3], d: [f32; 3], clip: Option<f32>) -> Option<usize> {
        let mut best: Option<(f32, u32)> = None;
        for (t, tri) in self.vertices.as_chunks::<{ VERTEX_FLOATS * 3 }>().0.iter().enumerate() {
            let p = |k: usize| [tri[k * VERTEX_FLOATS], tri[k * VERTEX_FLOATS + 1], tri[k * VERTEX_FLOATS + 2]];
            let Some(hit) = ray_triangle(o, d, p(0), p(1), p(2)) else { continue };
            if best.is_some_and(|(b, _)| b <= hit) {
                continue;
            }
            if clip.is_some_and(|c| o[2] + d[2] * hit > c) {
                continue;
            }
            if let Some(e) = self.tri_element.get(t) {
                best = Some((hit, *e));
            }
        }
        best.map(|(_, e)| e as usize)
    }

    /// Copies of the triangles of `elements`, recoloured and moved by `shift` (metres; another
    /// scene's origin), for highlights.
    pub fn overlay(&self, elements: &[usize], rgba: [f32; 4], shift: [f32; 3]) -> Vec<f32> {
        let mut out = Vec::new();
        for e in elements.iter().filter_map(|i| self.elements.get(*i)) {
            for r in &e.ranges {
                let (a, b) = (r.start as usize * VERTEX_FLOATS, r.end as usize * VERTEX_FLOATS);
                for v in self.vertices.get(a..b).unwrap_or_default().as_chunks::<VERTEX_FLOATS>().0 {
                    out.extend_from_slice(&[v[0] + shift[0], v[1] + shift[1], v[2] + shift[2]]);
                    out.extend_from_slice(&v[3..6]);
                    out.extend_from_slice(&rgba);
                    out.extend_from_slice(&v[10..]);
                }
            }
        }
        out
    }

    /// Metres to add to this scene's coordinates to place them in `other`'s.
    pub fn shift_to(&self, other: &Scene) -> [f32; 3] {
        [0, 1, 2].map(|k| ((self.origin[k] - other.origin[k]) / 1000.0) as f32)
    }

    /// The element with this id, if any.
    pub fn find(&self, id: &str) -> Option<usize> {
        self.elements.iter().position(|e| e.id == id)
    }

    /// Semantic ids are not unique in every phase view. Never silently choose an occurrence.
    pub fn find_unique(&self, id: &str) -> Option<usize> {
        let mut matches = self.elements.iter().enumerate().filter(|(_, e)| e.id == id);
        let (index, _) = matches.next()?;
        matches.next().is_none().then_some(index)
    }

    pub fn ambiguous(&self, id: &str) -> bool {
        self.elements.iter().filter(|e| e.id == id).take(2).count() > 1
    }

    /// Element keys (ids, made unique) and fingerprints.
    fn keyed(&self) -> HashMap<String, (usize, u64)> {
        let mut out = HashMap::with_capacity(self.elements.len());
        for (i, e) in self.elements.iter().enumerate() {
            let mut key = e.id.clone();
            let mut n = 1;
            while out.contains_key(&key) {
                n += 1;
                key = format!("{}#{n}", e.id);
            }
            out.insert(key, (i, e.fingerprint));
        }
        out
    }
}

/// What a change did to the scene.
#[derive(Debug, Default, Clone, PartialEq)]
pub struct Diff {
    /// Elements of the new scene that are new or changed.
    pub changed: Vec<usize>,
    /// Elements of the old scene that are gone.
    pub removed: Vec<usize>,
    /// The changed elements worth highlighting: walls and slabs only when nothing else changed
    /// or went (a window placed in a wall changes the wall too; a removed one shows as a ghost).
    pub highlight: Vec<usize>,
}

impl Diff {
    pub fn is_empty(&self) -> bool {
        self.changed.is_empty() && self.removed.is_empty()
    }
}

pub fn diff(old: &Scene, new: &Scene) -> Diff {
    let (a, b) = (old.keyed(), new.keyed());
    let mut changed: Vec<usize> = b.iter().filter(|(k, (_, f))| a.get(*k).is_none_or(|(_, g)| g != f)).map(|(_, (i, _))| *i).collect();
    let mut removed: Vec<usize> = a.iter().filter(|(k, _)| !b.contains_key(*k)).map(|(_, (i, _))| *i).collect();
    changed.sort_unstable();
    removed.sort_unstable();
    let guests: Vec<usize> = changed.iter().copied().filter(|i| new.elements.get(*i).is_some_and(|e| !e.host)).collect();
    let highlight = if guests.is_empty() && removed.is_empty() { changed.clone() } else { guests };
    Diff { changed, removed, highlight }
}

/// Möller–Trumbore: the ray parameter of the hit, if the ray meets the triangle in front.
fn ray_triangle(o: [f32; 3], d: [f32; 3], a: [f32; 3], b: [f32; 3], c: [f32; 3]) -> Option<f32> {
    let sub = |p: [f32; 3], q: [f32; 3]| [p[0] - q[0], p[1] - q[1], p[2] - q[2]];
    let cross = |p: [f32; 3], q: [f32; 3]| [p[1] * q[2] - p[2] * q[1], p[2] * q[0] - p[0] * q[2], p[0] * q[1] - p[1] * q[0]];
    let dot = |p: [f32; 3], q: [f32; 3]| p[0] * q[0] + p[1] * q[1] + p[2] * q[2];
    let (e1, e2) = (sub(b, a), sub(c, a));
    let p = cross(d, e2);
    let det = dot(e1, p);
    if det.abs() < 1e-12 {
        return None;
    }
    let inv = 1.0 / det;
    let s = sub(o, a);
    let u = dot(s, p) * inv;
    if !(0.0..=1.0).contains(&u) {
        return None;
    }
    let q = cross(s, e1);
    let v = dot(d, q) * inv;
    if v < 0.0 || u + v > 1.0 {
        return None;
    }
    let t = dot(e2, q) * inv;
    (t > 1e-6).then_some(t)
}

#[cfg(test)]
mod tests {
    use super::*;
    use trigon_model::{Building, Document, build};

    fn cube() -> Mesh {
        let mut m = Mesh::default();
        let lo = [[0.0, 0.0, 0.0], [1000.0, 0.0, 0.0], [1000.0, 1000.0, 0.0], [0.0, 1000.0, 0.0]];
        let hi = lo.map(|p| [p[0], p[1], 1000.0]);
        m.hexahedron(lo, hi);
        m
    }

    #[test]
    fn a_box_has_twelve_edges_and_no_diagonals() {
        let m = cube();
        assert_eq!(m.triangles.len(), 12);
        assert_eq!(feature_edges(&m.positions, &m.triangles, CREASE_DEG).len(), 12);
    }

    #[test]
    fn a_flat_quad_draws_its_border_only_whatever_the_winding() {
        let mut m = Mesh::default();
        m.quad([0.0, 0.0, 0.0], [1000.0, 0.0, 0.0], [1000.0, 1000.0, 0.0], [0.0, 1000.0, 0.0]);
        assert_eq!(feature_edges(&m.positions, &m.triangles, CREASE_DEG).len(), 4);
        // The second triangle turned over: still coplanar, still no diagonal.
        m.triangles[1] = [m.triangles[1][0], m.triangles[1][2], m.triangles[1][1]];
        assert_eq!(feature_edges(&m.positions, &m.triangles, CREASE_DEG).len(), 4);
    }

    #[test]
    fn a_shallow_fold_is_no_crease_but_a_sharp_one_is() {
        let fold = |z: f64| {
            let mut m = Mesh::default();
            m.quad([0.0, 0.0, 0.0], [1000.0, 0.0, 0.0], [1000.0, 1000.0, 0.0], [0.0, 1000.0, 0.0]);
            m.quad([1000.0, 0.0, 0.0], [2000.0, 0.0, z], [2000.0, 1000.0, z], [1000.0, 1000.0, 0.0]);
            feature_edges(&m.positions, &m.triangles, CREASE_DEG).len()
        };
        assert_eq!(fold(50.0), 6, "3° stays smooth");
        assert_eq!(fold(1000.0), 7, "45° draws the fold");
    }

    #[test]
    fn hostile_meshes_never_panic() {
        let positions = vec![[f64::NAN, 0.0, 0.0], [1.0, f64::INFINITY, 0.0], [0.0, 0.0, 0.0], [1.0, 0.0, 0.0]];
        let triangles = vec![[0, 1, 2], [2, 3, 99], [2, 2, 3], [u32::MAX, 0, 1]];
        assert!(feature_edges(&positions, &triangles, CREASE_DEG).is_empty());
    }

    fn doc() -> Document {
        let mut doc = Document::default();
        doc.buildings.push(Building::new("b1", "T", &[[0.0, 0.0], [9000.0, 0.0], [9000.0, 8000.0], [0.0, 8000.0]], 2, 3000.0).unwrap());
        doc
    }

    #[test]
    fn terrain_uses_the_shared_source_mesh_in_the_headless_scene() {
        let mut d = doc();
        d.terrain = Some(trigon_model::terrain::Terrain {
            origin: [-5000., -5000.],
            spacing_mm: [20000., 20000.],
            heights_mm: vec![vec![0., 100.], vec![200., 300.]],
            source: None,
            attribution: None,
        });
        let built = build(&d).unwrap();
        let scene = Scene::from_built(&built, false);
        let element = &scene.elements[scene.find_unique("site.terrain").unwrap()];
        assert_eq!(element.kind, "terrain");
        assert!(!element.ranges.is_empty());
        assert_eq!(scene.elements.iter().filter(|e| e.id == "site.terrain").count(), 1);
        let without = Scene::from_built(&build(&doc()).unwrap(), false);
        assert_eq!(scene.elements.len(), without.elements.len() + 1);
    }

    #[test]
    fn rooms_are_pickable_floor_spaces_without_idle_edges_or_opaque_boxes() {
        let mut d = doc();
        d.buildings[0].rooms.push(trigon_model::doc::Room {
            id: "living".into(),
            levels: vec![0],
            name: "Living".into(),
            polygon: vec![[1000.0, 1000.0], [5000.0, 1000.0], [5000.0, 5000.0], [1000.0, 5000.0]],
        });
        let built = build(&d).unwrap();
        let plain = Scene::from_built(&built, false);
        let edges = plain.edges.clone();
        let opaque = plain.opaque;
        let scene = plain.with_rooms(&d);
        let room = scene.find_unique("living.space.L0").unwrap();
        assert_eq!(scene.edges, edges);
        assert_eq!(scene.opaque, opaque);
        let local = |p: [f64; 3]| std::array::from_fn(|axis| ((p[axis] - scene.origin[axis]) / 1000.0) as f32);
        assert_eq!(scene.pick(local([2000.0, 2000.0, 1000.0]), [0.0, 0.0, -1.0], scene.clip_z(0)), Some(room));
        for range in &scene.elements[room].ranges {
            assert!(
                scene.vertices[range.start as usize * VERTEX_FLOATS..range.end as usize * VERTEX_FLOATS]
                    .as_chunks::<VERTEX_FLOATS>()
                    .0
                    .iter()
                    .all(|v| v[9] == 0.0)
            );
        }
        assert!(!scene.overlay(&[room], [1.0, 0.4, 0.0, 0.4], [0.0; 3]).is_empty());
    }

    #[test]
    fn facade_finishes_and_plinths_reach_the_viewport() {
        let mut d = doc();
        let original = Scene::from_built(&build(&d).unwrap(), false);
        d.buildings[0].plinth = Some(trigon_model::doc::Plinth { height: 600.0, projection: 50.0 });
        let plinth = Scene::from_built(&build(&d).unwrap(), false);
        assert_ne!(original.elements[0].fingerprint, plinth.elements[0].fingerprint, "the plinth must be visible");
        d.buildings[0].finish = trigon_model::doc::WallFinish::Render;
        let rendered = Scene::from_built(&build(&d).unwrap(), false);
        assert_ne!(plinth.elements[0].fingerprint, rendered.elements[0].fingerprint, "the facade finish must be visible");
    }

    #[test]
    fn masonry_uvs_stay_at_site_scale_when_scene_origin_changes_and_overlays_keep_the_vertex_layout() {
        let mut d = doc();
        d.buildings[0].finish = trigon_model::doc::WallFinish::YellowBrick;
        let a = Scene::from_built(&build(&d).unwrap(), false);
        d.buildings
            .push(Building::new("annex", "Annex", &[[100000.0, 0.0], [109000.0, 0.0], [109000.0, 8000.0], [100000.0, 8000.0]], 1, 3000.0).unwrap());
        let b = Scene::from_built(&build(&d).unwrap(), false);
        assert_ne!(a.origin, b.origin);
        let uvs = |scene: &Scene, id: &str| {
            let element = &scene.elements[scene.find(id).unwrap()];
            element
                .ranges
                .iter()
                .flat_map(|range| {
                    scene.vertices[range.start as usize * VERTEX_FLOATS..range.end as usize * VERTEX_FLOATS]
                        .as_chunks::<VERTEX_FLOATS>()
                        .0
                        .iter()
                        .map(|vertex| [vertex[10], vertex[11], vertex[12]])
                })
                .collect::<Vec<_>>()
        };
        let wall = uvs(&a, "b1.f1");
        assert!(!wall.is_empty());
        assert!(wall.iter().all(|uv| uv.iter().all(|value| value.is_finite()) && uv[2] == 1.0));
        assert_eq!(wall, uvs(&b, "b1.f1"));
        let overlay = a.overlay(&[a.find("b1.f1").unwrap()], [1.0, 0.5, 0.0, 0.6], [3.0, 0.0, 0.0]);
        let overlay_uvs: Vec<_> = overlay.as_chunks::<VERTEX_FLOATS>().0.iter().map(|vertex| [vertex[10], vertex[11], vertex[12]]).collect();
        assert_eq!(overlay_uvs, wall);
        for element in a.elements.iter().filter(|e| !matches!(e.kind, "wall")) {
            for range in &element.ranges {
                assert!(
                    a.vertices[range.start as usize * VERTEX_FLOATS..range.end as usize * VERTEX_FLOATS]
                        .as_chunks::<VERTEX_FLOATS>()
                        .0
                        .iter()
                        .all(|vertex| vertex[12] == 0.0)
                );
            }
        }
    }

    #[test]
    fn the_scene_has_every_element_and_picks_them() {
        let built = build(&doc()).unwrap();
        let s = Scene::from_built(&built, false);
        let kinds: Vec<&str> = s.elements.iter().map(|e| e.kind).collect();
        assert_eq!(kinds.iter().filter(|k| **k == "wall").count(), 4);
        assert!(kinds.contains(&"floor slab"));
        assert_eq!(s.tri_element.len() * 3 * VERTEX_FLOATS, s.vertices.len());
        assert!(s.edge_count() >= 12);
        let (lo, hi) = s.bounds.unwrap();
        assert!((hi[0] - lo[0] - 9.0).abs() < 0.01 && (hi[1] - lo[1] - 8.0).abs() < 0.01, "{lo:?} {hi:?}");
        // Straight down onto the middle: the roof (or top slab) is hit first.
        let top = s.pick([0.0, 0.0, 100.0], [0.0, 0.0, -1.0], None).unwrap();
        assert!(s.elements[top].kind.contains("roof") || s.elements[top].kind.contains("slab"), "{}", s.elements[top].kind);
        // Cut at level 0: the roof is ignored and the ground floor slab is found.
        let cut = s.pick([0.0, 0.0, 100.0], [0.0, 0.0, -1.0], s.clip_z(0)).unwrap();
        assert_ne!(cut, top);
        // From the south, horizontally: a wall.
        let wall = s.pick([0.0, -50.0, 1.5], [0.0, 1.0, 0.0], None).unwrap();
        assert_eq!(s.elements[wall].kind, "wall");
        // It faces south, towards the ray's origin, and its box is the 9 m south wall.
        let f = s.elements[wall].facing.unwrap();
        assert!(f[1] < -0.99, "{f:?}");
        let (lo, hi) = s.elements[wall].bounds.unwrap();
        assert!((hi[0] - lo[0] - 9.0).abs() < 0.01 && hi[1] - lo[1] < 1.0, "{lo:?} {hi:?}");
        assert!(s.pick([0.0, -50.0, 1.5], [0.0, -1.0, 0.0], None).is_none(), "nothing behind the camera");
    }

    #[test]
    fn a_diff_names_what_changed_and_highlights_guests_over_hosts() {
        let mut d = doc();
        let before = Scene::from_built(&build(&d).unwrap(), false);
        assert!(diff(&before, &before).is_empty());
        d.buildings[0].wall_thickness += 100.0;
        let thicker = Scene::from_built(&build(&d).unwrap(), false);
        let walls = diff(&before, &thicker);
        assert!(walls.changed.iter().any(|i| thicker.elements[*i].kind == "wall"));
        assert_eq!(walls.highlight, walls.changed, "only hosts changed: they are highlighted");
        assert!(walls.removed.is_empty());

        let mut before2 = thicker.clone();
        before2.elements.push(Element {
            id: "gone".into(),
            kind: "window",
            level: Some(0),
            props: vec![],
            ranges: vec![],
            fingerprint: 1,
            host: false,
            bounds: None,
            facing: None,
        });
        let mut after2 = thicker.clone();
        after2.elements[0].fingerprint ^= 1;
        after2.elements.push(Element {
            id: "new".into(),
            kind: "window",
            level: Some(0),
            props: vec![],
            ranges: vec![],
            fingerprint: 2,
            host: false,
            bounds: None,
            facing: None,
        });
        let g = diff(&before2, &after2);
        assert_eq!(g.removed, vec![before2.elements.len() - 1]);
        assert_eq!(g.highlight, vec![after2.elements.len() - 1], "the new window, not its wall");
        assert_eq!(g.changed.len(), 2);
        // An undo that takes the window out again: its ghost is the news, not its wall.
        let undo = diff(&after2, &thicker);
        assert_eq!(undo.removed.len(), 1);
        assert!(undo.highlight.is_empty(), "{undo:?}");
    }

    #[test]
    fn overlays_copy_positions_with_the_highlight_colour() {
        let s = Scene::from_built(&build(&doc()).unwrap(), false);
        let o = s.overlay(&[0], [1.0, 0.5, 0.0, 0.6], [0.0; 3]);
        let n: u32 = s.elements[0].ranges.iter().map(|r| r.end - r.start).sum();
        assert_eq!(o.len(), n as usize * VERTEX_FLOATS);
        assert_eq!(&o[6..10], &[1.0, 0.5, 0.0, 0.6]);
        assert!(s.overlay(&[usize::MAX], [1.0; 4], [0.0; 3]).is_empty());
        let moved = s.overlay(&[0], [1.0; 4], [1.0, 2.0, 3.0]);
        assert_eq!([moved[0] - o[0], moved[1] - o[1], moved[2] - o[2]], [1.0, 2.0, 3.0]);
    }
}
