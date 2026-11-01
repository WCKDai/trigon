//! Small, deterministic site objects. These have their own sources and identities: no building,
//! facade opening or imported asset is used to stand in for landscaping. Millimetres, Z up.

use std::collections::BTreeSet;
use std::f64::consts::{PI, TAU};

use serde::{Deserialize, Serialize};

use crate::build::{Status, View};
use crate::{Document, Material, Mesh, ModelError, Result, checked};

pub const MAX_OBJECTS: usize = 512;
pub const MAX_POLYGON_VERTICES: usize = 256;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum Intent {
    Existing,
    Proposal,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum GroundSurface {
    Paving,
    Gravel,
    Grass,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct SiteObject {
    /// Site-owned namespace: `site.` followed by 1–64 ASCII letters, digits, _ or -.
    pub id: String,
    pub name: String,
    /// Author intent is separate from phase status; a proposal is never survey evidence.
    pub intent: Intent,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub created: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub demolished: Option<String>,
    pub geometry: SiteGeometry,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum SiteGeometry {
    /// A constant-height area with a real build-up below its top, no terrain interpolation.
    Ground {
        polygon: Vec<[f64; 2]>,
        elevation_mm: f64,
        thickness_mm: f64,
        surface: GroundSurface,
    },
    /// A generic presentation tree, not a measured species or growth simulation.
    Tree {
        position_mm: [f64; 3],
        height_mm: f64,
        crown_radius_mm: f64,
        trunk_radius_mm: f64,
    },
    Bench {
        position_mm: [f64; 3],
        length_mm: f64,
        depth_mm: f64,
        seat_height_mm: f64,
        rotation_deg: f64,
    },
    Planter {
        position_mm: [f64; 3],
        length_mm: f64,
        width_mm: f64,
        height_mm: f64,
        wall_mm: f64,
        rotation_deg: f64,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SiteClass {
    Ground,
    Tree,
    Bench,
    Planter,
}
impl SiteClass {
    pub fn name(self) -> &'static str {
        match self {
            Self::Ground => "site ground",
            Self::Tree => "tree",
            Self::Bench => "bench",
            Self::Planter => "planter",
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct BuiltSiteObject {
    pub id: String,
    pub name: String,
    pub class: SiteClass,
    pub intent: Intent,
    pub status: Status,
    pub source: SiteObject,
    pub parts: Vec<(Material, Mesh)>,
    /// Exact footprint for plan drawing; tree uses its crown's deterministic plan silhouette.
    pub footprint: Vec<[f64; 2]>,
    pub base_mm: f64,
    pub top_mm: f64,
}

fn invalid(s: &str) -> ModelError {
    ModelError::Invalid(s.into())
}

pub fn identifier(id: &str) -> Result<()> {
    let Some(local) = id.strip_prefix("site.") else {
        return Err(invalid("site IDs must start with site."));
    };
    if local.is_empty() || local.len() > 64 || !local.bytes().all(|c| c.is_ascii_alphanumeric() || b"_-".contains(&c)) {
        return Err(invalid("site IDs need 1 to 64 ASCII letters, digits, _ or - after site."));
    }
    Ok(())
}

fn position(p: [f64; 3]) -> Result<()> {
    for v in &p[..2] {
        checked("site coordinate", *v, -1e8, 1e8)?;
    }
    checked("site elevation", p[2], -50000., 500000.)?;
    Ok(())
}

fn orient(a: [f64; 2], b: [f64; 2], c: [f64; 2]) -> f64 {
    (b[0] - a[0]) * (c[1] - a[1]) - (b[1] - a[1]) * (c[0] - a[0])
}
fn on(a: [f64; 2], b: [f64; 2], p: [f64; 2]) -> bool {
    orient(a, b, p).abs() < 1e-6 && (0..2).all(|k| p[k] >= a[k].min(b[k]) - 1e-6 && p[k] <= a[k].max(b[k]) + 1e-6)
}
fn intersect(a: [f64; 2], b: [f64; 2], c: [f64; 2], d: [f64; 2]) -> bool {
    let (u, v, w, x) = (orient(a, b, c), orient(a, b, d), orient(c, d, a), orient(c, d, b));
    (u * v < 0. && w * x < 0.) || on(a, b, c) || on(a, b, d) || on(c, d, a) || on(c, d, b)
}

/// Validate without moving, simplifying, winding or closing source coordinates.
pub fn polygon(points: &[[f64; 2]]) -> Result<()> {
    if !(3..=MAX_POLYGON_VERTICES).contains(&points.len()) {
        return Err(invalid("site polygon needs 3 to 256 unclosed vertices"));
    }
    for p in points {
        position([p[0], p[1], 0.])?;
    }
    let n = points.len();
    for i in 0..n {
        let (a, b, c) = (points[i], points[(i + 1) % n], points[(i + 2) % n]);
        checked("site polygon edge", (b[0] - a[0]).hypot(b[1] - a[1]), 1., 1e8)?;
        if orient(a, b, c).abs() < 1e-6 && (b[0] - a[0]) * (c[0] - b[0]) + (b[1] - a[1]) * (c[1] - b[1]) < 0. {
            return Err(invalid("site polygon doubles back"));
        }
        for j in i + 1..n {
            if j == i + 1 || (i == 0 && j == n - 1) {
                continue;
            }
            if intersect(a, b, points[j], points[(j + 1) % n]) {
                return Err(invalid("site polygon must be simple, without crossings or touching edges"));
            }
        }
    }
    checked("site area m²", crate::footprint::signed_area2(points).abs() / 2e6, 0.01, 1e6)?;
    Ok(())
}

impl SiteObject {
    pub fn lifetime(&self, doc: &Document) -> Result<(usize, usize)> {
        let c = self.created.as_deref().map(|s| doc.phase_index(s)).transpose()?.unwrap_or(0);
        let d = self.demolished.as_deref().map(|s| doc.phase_index(s)).transpose()?.unwrap_or(doc.phases.len().max(1));
        if d <= c {
            return Err(invalid("site demolition must be after creation"));
        }
        Ok((c, d))
    }
    pub fn validate(&self, doc: &Document) -> Result<()> {
        identifier(&self.id)?;
        if self.name.is_empty() || self.name.len() > 255 || self.name.chars().any(char::is_control) {
            return Err(invalid("site name needs 1 to 255 bytes without control characters"));
        }
        self.lifetime(doc)?;
        match &self.geometry {
            SiteGeometry::Ground { polygon: p, elevation_mm: z, thickness_mm: t, .. } => {
                polygon(p)?;
                position([0., 0., *z])?;
                checked("ground thickness", *t, 10., 1000.)?;
            }
            SiteGeometry::Tree { position_mm: p, height_mm: h, crown_radius_mm: r, trunk_radius_mm: t } => {
                position(*p)?;
                checked("tree height", *h, 1000., 20000.)?;
                checked("tree crown radius", *r, 300., h * 0.65)?;
                checked("tree trunk radius", *t, 25., r.min(500.) * 0.5)?;
            }
            SiteGeometry::Bench { position_mm: p, length_mm: l, depth_mm: d, seat_height_mm: h, rotation_deg: a } => {
                position(*p)?;
                checked("bench length", *l, 800., 4000.)?;
                checked("bench depth", *d, 350., 900.)?;
                checked("bench seat height", *h, 300., 650.)?;
                checked("site rotation", *a, -360., 360.)?;
            }
            SiteGeometry::Planter { position_mm: p, length_mm: l, width_mm: w, height_mm: h, wall_mm: t, rotation_deg: a } => {
                position(*p)?;
                checked("planter length", *l, 500., 5000.)?;
                checked("planter width", *w, 300., 5000.)?;
                checked("planter height", *h, 200., 1500.)?;
                checked("planter wall", *t, 15., l.min(*w).min(*h) / 4.)?;
                checked("site rotation", *a, -360., 360.)?;
            }
        }
        // Bound final extents as well as anchors, including rotated furniture.
        let (p, r, h) = match &self.geometry {
            SiteGeometry::Ground { elevation_mm: z, thickness_mm: t, .. } => {
                position([0., 0., z - t])?;
                ([0., 0., 0.], 0., 0.)
            }
            SiteGeometry::Tree { position_mm, height_mm, crown_radius_mm, .. } => (*position_mm, *crown_radius_mm, *height_mm),
            SiteGeometry::Bench { position_mm, length_mm, depth_mm, seat_height_mm, .. } => {
                (*position_mm, length_mm.hypot(*depth_mm) / 2., seat_height_mm + 410.)
            }
            SiteGeometry::Planter { position_mm, length_mm, width_mm, height_mm, .. } => (*position_mm, length_mm.hypot(*width_mm) / 2., *height_mm),
        };
        position([p[0] - r, p[1] - r, p[2]])?;
        position([p[0] + r, p[1] + r, p[2] + h])?;
        Ok(())
    }
}

/// All sources are validated even when hidden by a phase filter. Bound aggregate complexity
/// before allocation; invalid documents fail on load through the standard model build.
pub fn validate(doc: &Document) -> Result<()> {
    if doc.site_objects.len() > MAX_OBJECTS {
        return Err(invalid("at most 512 site objects"));
    }
    let mut vertices = 0usize;
    let mut ids = BTreeSet::new();
    for o in &doc.site_objects {
        if !ids.insert(&o.id) {
            return Err(invalid("site IDs must be unique"));
        }
        if doc.buildings.iter().any(|b| b.id == "site" || b.id == o.id || b.facades.iter().any(|f| f.id == o.id)) {
            return Err(invalid("site ID collides with a building source or its reserved site namespace"));
        }
        o.validate(doc)?;
        if let SiteGeometry::Ground { polygon, .. } = &o.geometry {
            vertices += polygon.len();
        }
        if vertices > 8192 {
            return Err(invalid("at most 8192 site polygon vertices in a document"));
        }
    }
    Ok(())
}

/// Build site objects separately from buildings, including a site-only document. No fake building
/// or storey is introduced. Exporters and Studio consume the same typed geometry.
pub fn build(doc: &Document, view: View, cut: Option<f64>) -> Result<Vec<BuiltSiteObject>> {
    validate(doc)?;
    if view.phase >= doc.phases.len().max(1) {
        return Err(invalid("site view phase does not exist"));
    }
    if let Some(z) = cut {
        checked("site cut", z, -50000., 500000.)?;
    }
    let mut out = Vec::new();
    for o in &doc.site_objects {
        let Some(status) = view.status(o.lifetime(doc)?) else {
            continue;
        };
        let mut built = geometry(o, status)?;
        if let Some(z) = cut {
            if built.base_mm > z {
                continue;
            }
            for (_, m) in &mut built.parts {
                *m = m.below(z);
            }
        }
        out.push(built);
    }
    Ok(out)
}

fn rect(l: f64, w: f64) -> Vec<[f64; 2]> {
    vec![[-l / 2., -w / 2.], [l / 2., -w / 2.], [l / 2., w / 2.], [-l / 2., w / 2.]]
}
fn transform(p: [f64; 3], a: f64, q: [f64; 3]) -> [f64; 3] {
    let (s, c) = a.to_radians().sin_cos();
    [p[0] + q[0] * c - q[1] * s, p[1] + q[0] * s + q[1] * c, p[2] + q[2]]
}
fn box_mesh(m: &mut Mesh, p: [f64; 3], angle: f64, lo: [f64; 3], hi: [f64; 3]) {
    let face = |z| [[lo[0], lo[1], z], [hi[0], lo[1], z], [hi[0], hi[1], z], [lo[0], hi[1], z]].map(|q| transform(p, angle, q));
    m.hexahedron(face(lo[2]), face(hi[2]));
}
fn cylinder(r: f64, h: f64) -> Mesh {
    let mut m = Mesh::default();
    let ring: Vec<[f64; 2]> = (0..16)
        .map(|i| {
            let a = TAU * i as f64 / 16.;
            [r * a.cos(), r * a.sin()]
        })
        .collect();
    m.polygon(&ring, &[], |p| [p[0], p[1], h]);
    let mut bottom = Mesh::default();
    bottom.polygon(&ring, &[], |p| [p[0], p[1], 0.]);
    for t in &mut bottom.triangles {
        t.swap(1, 2);
    }
    m.append(&bottom);
    for i in 0..16 {
        let a = ring[i];
        let b = ring[(i + 1) % 16];
        m.quad([a[0], a[1], 0.], [b[0], b[1], 0.], [b[0], b[1], h], [a[0], a[1], h]);
    }
    m
}
fn geometry(o: &SiteObject, status: Status) -> Result<BuiltSiteObject> {
    let (class, footprint, base, top, parts) = match &o.geometry {
        SiteGeometry::Ground { polygon, elevation_mm: z, thickness_mm: t, surface } => {
            let mut ring = polygon.clone();
            if crate::footprint::signed_area2(&ring) < 0. {
                ring.reverse();
            }
            let mesh = crate::build::prism(&ring, z - t, *t);
            let mat = match surface {
                GroundSurface::Paving => Material::Paving,
                GroundSurface::Gravel => Material::Gravel,
                GroundSurface::Grass => Material::Grass,
            };
            (SiteClass::Ground, ring, z - t, *z, vec![(mat, mesh)])
        }
        SiteGeometry::Tree { position_mm: p, height_mm: h, crown_radius_mm: r, trunk_radius_mm: t } => {
            let mut trunk = cylinder(*t, h * 0.65);
            for q in &mut trunk.positions {
                for k in 0..3 {
                    q[k] += p[k];
                }
            }
            // Seven latitude rings plus two poles, with consistent outward winding.
            let mut crown = Mesh::default();
            let point = |i: usize, j: usize| {
                let a = PI * i as f64 / 8.;
                let b = TAU * j as f64 / 16.;
                [p[0] + r * a.sin() * b.cos(), p[1] + r * a.sin() * b.sin(), p[2] + h * 0.675 + h * 0.325 * a.cos()]
            };
            for i in 0..8 {
                for j in 0..16 {
                    let (a, b, c, d) = (point(i, j), point(i + 1, j), point(i + 1, (j + 1) % 16), point(i, (j + 1) % 16));
                    if i == 0 {
                        let k = crown.positions.len() as u32;
                        crown.positions.extend([a, b, c]);
                        crown.triangles.push([k, k + 1, k + 2]);
                    } else if i == 7 {
                        let k = crown.positions.len() as u32;
                        crown.positions.extend([a, b, d]);
                        crown.triangles.push([k, k + 1, k + 2]);
                    } else {
                        crown.quad(a, b, c, d);
                    }
                }
            }
            let ring = (0..16)
                .map(|j| {
                    let q = point(4, j);
                    [q[0], q[1]]
                })
                .collect();
            (SiteClass::Tree, ring, p[2], p[2] + h, vec![(Material::Bark, trunk), (Material::Foliage, crown)])
        }
        SiteGeometry::Bench { position_mm: p, length_mm: l, depth_mm: d, seat_height_mm: h, rotation_deg: a } => {
            let mut timber = Mesh::default();
            let mut steel = Mesh::default();
            for i in 0..6 {
                let y = -d / 2. + i as f64 * d / 6.;
                box_mesh(&mut timber, *p, *a, [-l / 2., y, h - 40.], [l / 2., y + d / 6. - 8., *h]);
            }
            for i in 0..4 {
                let z = h + 70. + i as f64 * 90.;
                box_mesh(&mut timber, *p, *a, [-l / 2., d / 2. - 40., z], [l / 2., d / 2., z + 70.]);
            }
            for x in [-l * 0.32, l * 0.32] {
                for y in [-d * 0.32, d * 0.32] {
                    box_mesh(&mut steel, *p, *a, [x - 20., y - 20., 0.], [x + 20., y + 20., h - 40.]);
                }
                box_mesh(&mut steel, *p, *a, [x - 25., -d / 2., h - 70.], [x + 25., d / 2., h - 40.]);
                box_mesh(&mut steel, *p, *a, [x - 20., d / 2. - 45., h - 40.], [x + 20., d / 2. - 5., h + 410.]);
            }
            let ring = rect(*l, *d)
                .into_iter()
                .map(|q| {
                    let q = transform(*p, *a, [q[0], q[1], 0.]);
                    [q[0], q[1]]
                })
                .collect();
            (SiteClass::Bench, ring, p[2], p[2] + h + 410., vec![(Material::Deck, timber), (Material::Railing, steel)])
        }
        SiteGeometry::Planter { position_mm: p, length_mm: l, width_mm: w, height_mm: h, wall_mm: t, rotation_deg: a } => {
            let mut shell = Mesh::default();
            let mut soil = Mesh::default();
            box_mesh(&mut shell, *p, *a, [-l / 2., -w / 2., 0.], [l / 2., w / 2., *t]);
            for x in [-l / 2., l / 2. - t] {
                box_mesh(&mut shell, *p, *a, [x, -w / 2., *t], [x + t, w / 2., *h]);
            }
            for y in [-w / 2., w / 2. - t] {
                box_mesh(&mut shell, *p, *a, [-l / 2. + t, y, *t], [l / 2. - t, y + t, *h]);
            }
            box_mesh(&mut soil, *p, *a, [-l / 2. + t, -w / 2. + t, *t], [l / 2. - t, w / 2. - t, h - 80.]);
            let ring = rect(*l, *w)
                .into_iter()
                .map(|q| {
                    let q = transform(*p, *a, [q[0], q[1], 0.]);
                    [q[0], q[1]]
                })
                .collect();
            (SiteClass::Planter, ring, p[2], p[2] + h, vec![(Material::Planter, shell), (Material::Soil, soil)])
        }
    };
    for (_, m) in &parts {
        for p in &m.positions {
            position(*p)?;
        }
    }
    Ok(BuiltSiteObject {
        id: o.id.clone(),
        name: o.name.clone(),
        class,
        intent: o.intent,
        status,
        source: o.clone(),
        parts,
        footprint,
        base_mm: base,
        top_mm: top,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    fn ground() -> SiteObject {
        SiteObject {
            id: "site.court".into(),
            name: "Proposed courtyard".into(),
            intent: Intent::Proposal,
            created: None,
            demolished: None,
            geometry: SiteGeometry::Ground {
                polygon: vec![[0., 0.], [4000., 0.], [4000., 2000.], [2000., 2000.], [2000., 4000.], [0., 4000.]],
                elevation_mm: 0.,
                thickness_mm: 150.,
                surface: GroundSurface::Paving,
            },
        }
    }
    #[test]
    fn concave_ground_is_exact_and_closed_and_old_documents_stay_empty() {
        let mut doc = Document::default();
        let old = serde_json::to_value(&doc).unwrap();
        assert!(old.get("site_objects").is_none());
        doc = serde_json::from_value(old.clone()).unwrap();
        assert_eq!(serde_json::to_value(&doc).unwrap(), old);
        assert!(build(&doc, View::latest(&doc), None).unwrap().is_empty());
        doc.site_objects.push(ground());
        let source = doc.clone();
        let built = build(&doc, View::latest(&doc), None).unwrap();
        assert_eq!(doc, source);
        let m = &built[0].parts[0].1;
        let volume: f64 = m
            .triangles
            .iter()
            .map(|t| {
                let [a, b, c] = t.map(|i| m.positions[i as usize]);
                (a[0] * (b[1] * c[2] - b[2] * c[1]) + a[1] * (b[2] * c[0] - b[0] * c[2]) + a[2] * (b[0] * c[1] - b[1] * c[0])) / 6.
            })
            .sum();
        assert!((volume - 12e6 * 150.).abs() < 1e-3, "{volume}");
        assert_eq!(m.bounds().unwrap(), ([0., 0., -150.], [4000., 4000., 0.]));
        let copy: Document = serde_json::from_str(&serde_json::to_string(&doc).unwrap()).unwrap();
        assert_eq!(copy, doc);
        assert_eq!(build(&copy, View::latest(&copy), None).unwrap(), built);
    }
    #[test]
    fn site_polygons_reject_crossings_touching_and_hostile_coordinates() {
        for bad in [
            vec![],
            vec![[0., 0.]; 3],
            vec![[0., 0.], [5000., 5000.], [0., 5000.], [5000., 0.]],
            vec![[0., 0.], [5000., 0.], [5000., 5000.], [2500., 0.], [0., 5000.]],
            vec![[0., 0.], [1000., 0.], [1000., 1000.], [0., 0.]],
            vec![[f64::NAN; 2]; 4],
            vec![[1e300; 2]; 4],
            vec![[0., 0.]; 257],
        ] {
            assert!(polygon(&bad).is_err(), "{bad:?}");
        }
        let mut doc = Document { site_objects: vec![ground(); 513], ..Default::default() };
        assert!(validate(&doc).is_err());
        doc.site_objects = vec![ground(); 2];
        assert!(validate(&doc).is_err());
    }
    #[test]
    fn site_phases_and_cuts_follow_source_lifetimes() {
        let mut doc = Document::default();
        doc.phases
            .extend([crate::Phase { id: "proposal".into(), name: "Proposal".into() }, crate::Phase { id: "later".into(), name: "Later".into() }]);
        let mut o = ground();
        o.created = Some("proposal".into());
        o.demolished = Some("later".into());
        doc.site_objects.push(o);
        use crate::build::PhaseFilter;
        assert!(build(&doc, View { phase: 0, filter: PhaseFilter::Complete }, None).unwrap().is_empty());
        assert_eq!(build(&doc, View { phase: 1, filter: PhaseFilter::Complete }, None).unwrap()[0].status, Status::New);
        assert!(build(&doc, View::latest(&doc), None).unwrap().is_empty());
        assert_eq!(build(&doc, View { phase: 2, filter: PhaseFilter::Demolition }, None).unwrap()[0].status, Status::Demolished);
        assert!(build(&doc, View { phase: 1, filter: PhaseFilter::Complete }, Some(-1000.)).unwrap().is_empty());
    }
    #[test]
    fn trees_furniture_are_finite_bounded_and_outward_wound() {
        let mut doc = Document::default();
        let mut tree = ground();
        tree.id = "site.tree".into();
        tree.geometry = SiteGeometry::Tree { position_mm: [5000., 6000., 0.], height_mm: 4500., crown_radius_mm: 1500., trunk_radius_mm: 100. };
        doc.site_objects.push(tree);
        let b = build(&doc, View::latest(&doc), None).unwrap();
        assert!(b[0].parts.iter().map(|(_, m)| m.triangles.len()).sum::<usize>() < 500);
        let crown = &b[0].parts[1].1;
        for t in &crown.triangles {
            let [a, b, c] = t.map(|i| crown.positions[i as usize]);
            let u = [b[0] - a[0], b[1] - a[1], b[2] - a[2]];
            let v = [c[0] - a[0], c[1] - a[1], c[2] - a[2]];
            let n = [u[1] * v[2] - u[2] * v[1], u[2] * v[0] - u[0] * v[2], u[0] * v[1] - u[1] * v[0]];
            assert!(n[0] * (a[0] - 5000.) + n[1] * (a[1] - 6000.) + n[2] * (a[2] - 4500. * 0.675) > 0.);
        }
        for bad in [f64::NAN, f64::INFINITY, f64::MAX, -1., 0.] {
            if let SiteGeometry::Tree { height_mm, .. } = &mut doc.site_objects[0].geometry {
                *height_mm = bad;
            }
            assert!(build(&doc, View::latest(&doc), None).is_err());
        }
    }
}
