//! Typed site commands and Properties descriptors, shared by Studio, CLI and MCP.

use std::collections::BTreeMap;

use schemars::JsonSchema;
use serde::Deserialize;
use serde_json::{Value, json};
use trigon_model::build::View;
use trigon_model::site::{self, SiteGeometry, SiteObject};
use trigon_model::{Document, ModelError};

use crate::{Result, Session};

#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub(crate) struct Add {
    pub object: SiteObject,
}
#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub(crate) struct Set {
    pub id: String,
    pub object: SiteObject,
}
#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub(crate) struct Id {
    pub id: String,
}

fn invalid(s: &str) -> crate::EngineError {
    ModelError::Invalid(s.into()).into()
}
fn validate(doc: &Document) -> Result<()> {
    site::build(doc, View::latest(doc), None)?;
    Ok(())
}

pub(crate) fn add(s: &mut Session, mut p: Add) -> Result<Value> {
    if s.doc.site_objects.iter().any(|o| o.id == p.object.id) {
        return Err(invalid("site ID already exists"));
    }
    if p.object.created.is_none() {
        p.object.created = s.doc.phases.get(s.doc.active_index()).map(|p| p.id.clone());
    }
    let id = p.object.id.clone();
    s.doc.site_objects.push(p.object);
    validate(&s.doc)?;
    Ok(json!({"id":id}))
}
pub(crate) fn set(s: &mut Session, p: Set) -> Result<Value> {
    if p.id != p.object.id {
        return Err(invalid("site.set preserves the source ID"));
    }
    let target = s.doc.site_objects.iter_mut().find(|o| o.id == p.id).ok_or_else(|| invalid("site object not found"))?;
    *target = p.object;
    validate(&s.doc)?;
    Ok(json!({"id":p.id}))
}
pub(crate) fn delete(s: &mut Session, p: Id) -> Result<Value> {
    let i = s.doc.site_objects.iter().position(|o| o.id == p.id).ok_or_else(|| invalid("site object not found"))?;
    s.doc.site_objects.remove(i);
    Ok(json!({"deleted":p.id}))
}

fn field(key: &str, value: Value) -> Value {
    json!({"key":key,"label":key.replace('_'," "),"value":value,"editable":true})
}
fn number(key: &str, value: Value, min: f64, max: f64) -> Value {
    let mut f = field(key, value);
    f["min"] = json!(min);
    f["max"] = json!(max);
    f["unit"] = json!(if key == "rotation_deg" { "deg" } else { "mm" });
    f
}
fn handle(key: &str, p: [f64; 3], d: [f64; 3], value: f64, min: f64, max: f64, gain: f64) -> Value {
    json!({"key":key,"label":key.replace('_'," "),"position_mm":p,"direction":d,"value":value,"min":min,"max":max,"gain":gain})
}

pub(crate) fn records(doc: &Document, view: View) -> Result<Vec<(usize, Value)>> {
    let built = site::build(doc, view, None)?;
    let mut out = Vec::new();
    for o in built {
        let (index, source) = doc.site_objects.iter().enumerate().find(|(_, s)| s.id == o.id).ok_or_else(|| invalid("site source missing"))?;
        let raw = serde_json::to_value(source).map_err(|_| invalid("site source cannot be described"))?;
        let mut props = vec![
            field("name", raw["name"].clone()),
            field("intent", raw["intent"].clone()),
            field("created", raw["created"].clone()),
            field("demolished", raw["demolished"].clone()),
        ];
        props[1]["choices"] = json!(["existing", "proposal"]);
        let mut handles = Vec::new();
        let p = match &source.geometry {
            SiteGeometry::Ground { .. } => None,
            SiteGeometry::Tree { position_mm, .. } | SiteGeometry::Bench { position_mm, .. } | SiteGeometry::Planter { position_mm, .. } => {
                Some(*position_mm)
            }
        };
        if let Some(p) = p {
            for (i, key) in ["x_mm", "y_mm", "z_mm"].iter().enumerate() {
                let (min, max) = if i == 2 { (-50000., 500000.) } else { (-1e8, 1e8) };
                props.push(number(key, json!(p[i]), min, max));
                let mut d = [0.; 3];
                d[i] = 1.;
                handles.push(handle(key, p, d, p[i], min, max, 1.));
            }
        }
        if let Some(values) = raw["geometry"].as_object() {
            for (key, value) in values {
                if key == "kind" || key == "position_mm" {
                    continue;
                }
                let bounds = match (&source.geometry, key.as_str()) {
                    (_, "elevation_mm") => (-50000., 500000.),
                    (_, "thickness_mm") => (10., 1000.),
                    (_, "rotation_deg") => (-360., 360.),
                    (SiteGeometry::Tree { height_mm, .. }, "crown_radius_mm") => (300., height_mm * 0.65),
                    (SiteGeometry::Tree { crown_radius_mm, .. }, "trunk_radius_mm") => (25., crown_radius_mm.min(500.) * 0.5),
                    (SiteGeometry::Tree { .. }, "height_mm") => (1000., 20000.),
                    (SiteGeometry::Bench { .. }, "length_mm") => (800., 4000.),
                    (SiteGeometry::Bench { .. }, "depth_mm") => (350., 900.),
                    (SiteGeometry::Bench { .. }, "seat_height_mm") => (300., 650.),
                    (SiteGeometry::Planter { .. }, "length_mm") => (500., 5000.),
                    (SiteGeometry::Planter { .. }, "width_mm") => (300., 5000.),
                    (SiteGeometry::Planter { .. }, "height_mm") => (200., 1500.),
                    (SiteGeometry::Planter { length_mm, width_mm, height_mm, .. }, "wall_mm") => (15., length_mm.min(*width_mm).min(*height_mm) / 4.),
                    _ => {
                        props.push(field(key, value.clone()));
                        continue;
                    }
                };
                props.push(number(key, value.clone(), bounds.0, bounds.1));
            }
        }
        // Height and radius handles edit source dimensions, independent of tessellation.
        if let SiteGeometry::Tree { position_mm: p, height_mm: h, crown_radius_mm: r, .. } = &source.geometry {
            handles.push(handle("height_mm", [p[0], p[1], p[2] + h], [0., 0., 1.], *h, 1000., 20000., 1.));
            handles.push(handle("crown_radius_mm", [p[0] + r, p[1], p[2] + h * 0.675], [1., 0., 0.], *r, 300., h * 0.65, 1.));
        }
        if let Some(surface) = props.iter_mut().find(|f| f["key"] == "surface") {
            surface["choices"] = json!(["paving", "gravel", "grass"]);
        }
        out.push((index,json!({"id":o.id,"kind":o.class.name(),"building":null,"level":null,"editable":true,"reason":null,"status":o.status.name(),"properties":props,"handles":handles})));
    }
    Ok(out)
}

pub(crate) fn apply(doc: &mut Document, index: usize, properties: &BTreeMap<String, Value>) -> Result<String> {
    let source = doc.site_objects.get_mut(index).ok_or_else(|| invalid("site source missing"))?;
    let mut raw = serde_json::to_value(&*source).map_err(|_| invalid("invalid site source"))?;
    for (key, value) in properties {
        match key.as_str() {
            "name" | "intent" | "created" | "demolished" => raw[key] = value.clone(),
            "x_mm" | "y_mm" | "z_mm" => {
                if raw["geometry"]["position_mm"].is_null() {
                    return Err(invalid("this site object has no position anchor"));
                }
                let axis = if key == "x_mm" {
                    0
                } else if key == "y_mm" {
                    1
                } else {
                    2
                };
                raw["geometry"]["position_mm"][axis] = value.clone();
            }
            "kind" | "position_mm" | "id" => return Err(invalid("use typed site.set to replace site geometry; ID stays stable")),
            _ => {
                if raw["geometry"].get(key).is_none() {
                    return Err(invalid("unknown site property"));
                }
                raw["geometry"][key] = value.clone();
            }
        }
    }
    *source = serde_json::from_value(raw).map_err(|_| invalid("invalid typed site properties"))?;
    let id = source.id.clone();
    validate(doc)?;
    Ok(id)
}
