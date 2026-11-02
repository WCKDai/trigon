//! Local measured-footprint authoring: no provider or import shortcut is needed to start BIM.
use crate::{Result, Session};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use trigon_model::{Building, ModelError, checked};

#[derive(Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub(crate) struct Add {
    pub id: String,
    pub name: String,
    /// Outer line in shared site-local mm; open or closed, either winding. No holes.
    pub footprint: Vec<[f64; 2]>,
    pub storeys: usize,
    /// Explicit initial floor spacing; use building.set_levels for measured unequal floors.
    pub storey_height_mm: f64,
    #[serde(default)]
    pub ground_floor_mm: f64,
    #[serde(default = "wall")]
    pub wall_thickness_mm: f64,
    /// False opts into the legacy 200 mm near-collinear context-footprint simplification.
    #[serde(default = "yes")]
    pub preserve_vertices: bool,
    #[serde(default)]
    pub evidence: Vec<trigon_model::Evidence>,
}
fn wall() -> f64 {
    400.
}
fn yes() -> bool {
    true
}

pub(crate) fn add(s: &mut Session, p: Add) -> Result<Value> {
    if p.id.is_empty()
        || p.id.len() > 64
        || !p.id.chars().all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-'))
        || p.name.trim().is_empty()
        || p.name.len() > 200
        || p.name.chars().any(char::is_control)
    {
        return Err(ModelError::Invalid("building needs a 1..64 byte simple id and 1..200 byte name".into()).into());
    }
    if s.doc.buildings.len() >= 100 || s.doc.buildings.iter().any(|b| b.id == p.id) {
        return Err(ModelError::Invalid("building id already exists or the 100-building limit is reached".into()).into());
    }
    if p.evidence.len() > 32
        || p.evidence.iter().any(|e| [&e.source, &e.supplies, &e.attribution].iter().any(|s| s.len() > 2048 || s.chars().any(char::is_control)))
    {
        return Err(ModelError::Invalid("at most 32 evidence records with bounded text are allowed".into()).into());
    }
    checked("wall thickness", p.wall_thickness_mm, 100., 1500.)?;
    let mut building = Building::new(&p.id, &p.name, &p.footprint, p.storeys, p.storey_height_mm)?;
    if p.preserve_vertices {
        let mut ring = p.footprint;
        if ring.len() > 3 && ring.first() == ring.last() {
            ring.pop();
        }
        if ring.len() < 3 || ring.len() > 2000 {
            return Err(ModelError::Invalid("measured footprint needs 3..2000 corners".into()).into());
        }
        for i in 0..ring.len() {
            let (a, b) = (ring[i], ring[(i + 1) % ring.len()]);
            checked("measured footprint edge", (b[0] - a[0]).hypot(b[1] - a[1]), 1.001, 200_000.)?;
            for j in (i + 2)..ring.len() {
                if i == 0 && j == ring.len() - 1 {
                    continue;
                }
                if intersects(a, b, ring[j], ring[(j + 1) % ring.len()]) {
                    return Err(ModelError::Invalid("measured footprint crosses or touches a non-adjacent edge".into()).into());
                }
            }
        }
        let area = trigon_model::footprint::signed_area2(&ring);
        if !area.is_finite() || area.abs() < 8e6 {
            return Err(ModelError::Invalid("measured footprint area must be at least 4 m²".into()).into());
        }
        if area < 0. {
            ring.reverse();
        }
        // All new facades have the constructor's own defaults; only identity/edge changes.
        let prototype = building.facades.first().cloned().ok_or_else(|| ModelError::Invalid("building has no facade".into()))?;
        building.facades = (0..ring.len())
            .map(|i| {
                let mut f = prototype.clone();
                f.id = format!("{}.f{}", p.id, i + 1);
                f.edge = i;
                f
            })
            .collect();
        building.footprint = ring;
    }
    building.set_storeys_from(p.storeys, p.storey_height_mm, p.ground_floor_mm)?;
    building.wall_thickness = p.wall_thickness_mm;
    building.evidence = p.evidence;
    let result = json!({"building":building.id,"footprint":building.footprint,"preserve_vertices":p.preserve_vertices,
        "facades":building.facades.iter().map(|f|&f.id).collect::<Vec<_>>(),"storeys":building.storeys});
    s.doc.buildings.push(building);
    trigon_model::build(&s.doc)?;
    Ok(result)
}

fn intersects(a: [f64; 2], b: [f64; 2], c: [f64; 2], d: [f64; 2]) -> bool {
    let cross = |a: [f64; 2], b: [f64; 2], c: [f64; 2]| (b[0] - a[0]) * (c[1] - a[1]) - (b[1] - a[1]) * (c[0] - a[0]);
    let on = |a: [f64; 2], b: [f64; 2], p: [f64; 2]| {
        cross(a, b, p).abs() < 1e-6
            && p[0] >= a[0].min(b[0]) - 1e-6
            && p[0] <= a[0].max(b[0]) + 1e-6
            && p[1] >= a[1].min(b[1]) - 1e-6
            && p[1] <= a[1].max(b[1]) + 1e-6
    };
    (cross(a, b, c) * cross(a, b, d) < 0. && cross(c, d, a) * cross(c, d, b) < 0.) || on(a, b, c) || on(a, b, d) || on(c, d, a) || on(c, d, b)
}
