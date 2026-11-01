//! Typed, atomic terrain commands shared by CLI/MCP/Studio.
use crate::{Result, Session};
use schemars::JsonSchema;
use serde::Deserialize;
use serde_json::{Value, json};
use trigon_model::{ModelError, terrain::Terrain};

#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub(crate) struct Set {
    /// null removes terrain; heights are mm above the site's terrain datum.
    terrain: Update,
}

// A required field that can explicitly contain null. `schemars(required)` on an
// Option removes null from the schema, despite the deserializer accepting it.
#[derive(Deserialize, JsonSchema)]
#[serde(untagged)]
enum Update {
    Grid(Terrain),
    Remove(()),
}

pub(crate) fn set(s: &mut Session, p: Set) -> Result<Value> {
    let terrain = match p.terrain {
        Update::Grid(t) => Some(t),
        Update::Remove(()) => None,
    };
    if let Some(t) = &terrain {
        t.validate()?;
    }
    s.doc.terrain = terrain;
    let built = trigon_model::build(&s.doc)?;
    let triangles = built.iter().filter_map(|b| b.terrain.as_ref()).map(|m| m.triangles.len()).sum::<usize>();
    Ok(json!({"terrain":s.doc.terrain,"triangles":triangles,"datum":"site-local mm"}))
}

#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub(crate) struct Sample {
    /// South-west model sample, site-local mm.
    origin: [f64; 2],
    spacing_mm: [f64; 2],
    columns: u32,
    rows: u32,
}

pub(crate) fn sample(s: &mut Session, p: Sample) -> Result<Value> {
    if !(2..=65).contains(&p.rows) || !(2..=65).contains(&p.columns) {
        return Err(ModelError::Invalid("terrain requires 2–65 rows and columns".into()).into());
    }
    for k in 0..2 {
        trigon_model::checked("terrain origin", p.origin[k], -1e8, 1e8)?;
        trigon_model::checked("terrain spacing", p.spacing_mm[k], 100.0, 200_000.0)?;
    }
    let site = s.doc.site.as_ref().ok_or_else(|| ModelError::NotFound("no site; run site.load_address first".into()))?;
    let datum = site.elevation;
    let origin = std::array::from_fn(|k| site.origin_utm[k] + p.origin[k] / 1000.0);
    let spacing = p.spacing_mm.map(|v| v / 1000.0);
    let samples = s.provider().terrain(origin, spacing, p.columns, p.rows)?;
    if samples.len() != p.rows as usize || samples.iter().any(|row| row.len() != p.columns as usize) {
        return Err(ModelError::Invalid("terrain provider returned the wrong grid dimensions".into()).into());
    }
    let t = Terrain {
        origin: p.origin,
        spacing_mm: p.spacing_mm,
        heights_mm: samples.iter().map(|row| row.iter().map(|h| (h - datum) * 1000.0).collect()).collect(),
        source: Some("dhm:dhm_terraen".into()),
        attribution: Some("Indeholder data fra Styrelsen for Dataforsyning og Infrastruktur (Danmarks Højdemodel)".into()),
    };
    set(s, Set { terrain: Update::Grid(t) })
}
