//! Typed roof-window authoring. Session owns transaction, journal and Undo/Redo boundaries.
use crate::{Result, Session};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use trigon_model::{ModelError, roof::Rooflight};
#[derive(Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub(crate) struct Add {
    building: Option<String>,
    rooflight: Rooflight,
}
#[derive(Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub(crate) struct Remove {
    building: Option<String>,
    /// Stable local rooflight source id, as supplied to rooflight.add.
    id: String,
}
pub(crate) fn add(s: &mut Session, p: Add) -> Result<Value> {
    p.rooflight.validate()?;
    let b = s.doc.building_mut(p.building.as_deref())?;
    let building = b.id.clone();
    let lights = b.roof.rooflights_mut().ok_or_else(|| ModelError::Invalid("rooflights require a pitched or mansard host".into()))?;
    if lights.len() >= 64 || lights.iter().any(|l| l.id == p.rooflight.id) {
        return Err(ModelError::Invalid("duplicate rooflight id or 64-instance limit reached".into()).into());
    }
    let id = format!("{building}.rooflight.{}", p.rooflight.id);
    lights.push(p.rooflight);
    trigon_model::build(&s.doc)?;
    Ok(json!({"id":id,"host_roof":format!("{building}.roof"),"opening_id":format!("{id}.opening")}))
}
pub(crate) fn remove(s: &mut Session, p: Remove) -> Result<Value> {
    let b = s.doc.building_mut(p.building.as_deref())?;
    let building = b.id.clone();
    let lights = b.roof.rooflights_mut().ok_or_else(|| ModelError::Invalid("rooflights require a pitched or mansard host".into()))?;
    let index = lights.iter().position(|l| l.id == p.id).ok_or_else(|| ModelError::NotFound(format!("rooflight {}", p.id)))?;
    lights.remove(index);
    trigon_model::build(&s.doc)?;
    Ok(json!({"removed":format!("{building}.rooflight.{}",p.id)}))
}
#[cfg(test)]
mod tests {
    use super::*;
    fn session() -> Session {
        let mut s = Session::new(Box::new(trigon_data::Live::new(trigon_data::Credentials::default())));
        s.execute("building.add",json!({"id":"b","name":"Public rooflight fixture","footprint":[[0,0],[24000,0],[24000,12000],[0,12000]],"storeys":3,"storey_height_mm":3400})).unwrap();
        s.execute("building.set_roof", json!({"kind":"pitched","pitch_deg":46,"thickness_mm":220})).unwrap();
        s
    }
    fn spec() -> Value {
        json!({"id":"light","facade":"b.f1","u_mm":10000,"run_mm":500,"window_type":"window.casement.2","width_mm":800,"height_mm":1000,"stand_off_mm":100,"flashing_mm":100})
    }

    #[test]
    fn clipped_roof_properties_preserve_sources_and_exact_atomic_history() {
        let mut s = session();
        s.execute(
            "building.set_roof",
            json!({"kind":"pitched","pitch_deg":45,"gables":["b.f2","b.f4"],"thickness_mm":200,"overhang_mm":180,"verge_mm":135,"gutters":true}),
        )
        .unwrap();
        let before = s.doc.clone();
        let glb = trigon_io::glb::write(&trigon_model::build(&s.doc).unwrap(), &[]);
        let action = json!({"command":"element.set","params":{"id":"b.roof","properties":{"half_hips":[{"facade":"b.f4","drop_mm":1200}]}}});
        let p = s.execute("precision.preview", json!({"action":action})).unwrap();
        assert_eq!(s.doc, before);
        s.execute("precision.apply", json!({"action":p["action"],"preview_token":p["preview_token"]})).unwrap();
        assert_eq!(s.doc.journal.len(), before.journal.len() + 1);
        assert_eq!(s.doc.buildings[0].roof.clone().with_half_hips(Vec::new()).unwrap(), before.buildings[0].roof);
        assert_eq!(s.execute("element.get", json!({"id":"b.roof"})).unwrap()["kind"], "roof");
        let accepted = s.doc.clone();
        s.execute("edit.undo", json!({})).unwrap();
        assert_eq!(s.doc, before);
        assert_eq!(trigon_io::glb::write(&trigon_model::build(&s.doc).unwrap(), &[]), glb);
        for caps in [
            json!([]),
            json!([{"facade":"absent","drop_mm":1200}]),
            json!([{"facade":"b.f4","drop_mm":0}]),
            json!([{"facade":"b.f4","drop_mm":6000}]),
        ] {
            assert!(
                s.execute(
                    "building.set_roof",
                    json!({"kind":"half_hipped","pitch_deg":45,"gables":["b.f2","b.f4"],"thickness_mm":200,"half_hips":caps})
                )
                .is_err()
            );
            assert_eq!(s.doc, before);
        }
        s.execute("edit.redo", json!({})).unwrap();
        assert_eq!(s.doc, accepted);
        let roundtrip: trigon_model::Document = serde_json::from_slice(&serde_json::to_vec(&s.doc).unwrap()).unwrap();
        assert_eq!(roundtrip, s.doc);
        let mut replay = Session::new(Box::new(trigon_data::Live::new(Default::default())));
        for entry in &s.doc.journal {
            replay.execute(&entry.command, entry.params.clone()).unwrap();
        }
        assert_eq!(s.doc, replay.doc);
        let mut params = serde_json::to_value(&s.doc.buildings[0].roof).unwrap();
        params["building"] = json!("b");
        params["source"] = json!("Public synthetic cap, nominal 1200 mm drop");
        let action = json!({"command":"building.set_roof","params":params});
        let preview = s.execute("precision.preview", json!({"action":action})).unwrap();
        assert_eq!(s.doc, accepted);
        s.execute("precision.apply", json!({"action":preview["action"],"preview_token":preview["preview_token"]})).unwrap();
        assert_eq!(s.doc.journal.len(), accepted.journal.len() + 1);
        assert_eq!(s.doc.buildings[0].evidence.last().unwrap().source, "Public synthetic cap, nominal 1200 mm drop");
        s.execute("edit.undo", json!({})).unwrap();
        assert_eq!(s.doc, accepted);
    }

    #[test]
    fn returned_rooflight_opening_id_resolves_to_its_editable_source() {
        let mut s = session();
        let added = s.execute("rooflight.add", json!({"rooflight":spec()})).unwrap();
        let id = added["opening_id"].as_str().unwrap();
        assert_eq!(id, "b.rooflight.light.opening");
        let source = s.doc.clone();
        let properties = s.execute("element.get", json!({"id":id})).unwrap();
        assert_eq!(s.doc, source, "opening inspection is read-only");
        assert_eq!(properties["kind"], "rooflight");
        let glb = trigon_io::glb::write(&trigon_model::build(&s.doc).unwrap(), &[]);
        let action = json!({"command":"element.set","params":{"id":id,"properties":{"width_mm":900}}});
        let preview = s.execute("precision.preview", json!({"action":action})).unwrap();
        assert_eq!(s.doc, source);
        s.execute("precision.apply", json!({"action":preview["action"],"preview_token":preview["preview_token"]})).unwrap();
        assert_eq!(s.doc.buildings[0].roof.rooflights()[0].width_mm, 900.);
        let updated = s.doc.clone();
        s.execute("edit.undo", json!({})).unwrap();
        assert_eq!(s.doc, source);
        assert_eq!(trigon_io::glb::write(&trigon_model::build(&s.doc).unwrap(), &[]), glb);
        assert!(s.execute("element.set", json!({"id":id,"properties":{"width_mm":0}})).is_err());
        assert_eq!(s.doc, source);
        s.execute("edit.redo", json!({})).unwrap();
        assert_eq!(s.doc, updated);
        s.execute("rooflight.remove", json!({"id":"light"})).unwrap();
        assert!(s.execute("element.get", json!({"id":id})).is_err());
        s.execute("edit.undo", json!({})).unwrap();
        assert_eq!(s.doc, updated);
        assert_eq!(s.execute("element.get", json!({"id":id})).unwrap()["kind"], "rooflight");
    }
    #[test]
    fn roof_opening_ids_cannot_alias_generated_opening_or_flashing_ids() {
        let mut s = session();
        s.execute("rooflight.add", json!({"rooflight":spec()})).unwrap();
        let before = s.doc.clone();
        for id in ["light.flashing", "light.opening"] {
            let mut alias = spec();
            alias["id"] = json!(id);
            alias["u_mm"] = json!(14000.);
            assert!(s.execute("rooflight.add", json!({"rooflight":alias})).is_err(), "source id {id} aliases another generated product");
            assert_eq!(before, s.doc);
        }
    }
    #[test]
    fn public_rooflights_are_typed_editable_atomic_and_exactly_undoable() {
        let mut s = session();
        let before = s.doc.clone();
        s.execute("rooflight.add", json!({"rooflight":spec()})).unwrap();
        let accepted = s.doc.clone();
        let built = trigon_model::build(&accepted).unwrap();
        let glb = trigon_io::glb::write(&built, &[]);
        s.execute("edit.undo", json!({})).unwrap();
        assert_eq!(before, s.doc);
        for (field, value) in [
            ("u_mm", json!(1e300)),
            ("run_mm", json!(-1)),
            ("width_mm", json!(null)),
            ("stand_off_mm", json!(0)),
            ("yaw_deg", json!(91)),
            ("window_type", json!("missing")),
        ] {
            let mut bad = spec();
            bad[field] = value;
            let failure = s.doc.clone();
            assert!(s.execute("rooflight.add", json!({"rooflight":bad})).is_err());
            assert_eq!(s.doc, failure);
        }
        s.execute("edit.redo", json!({})).unwrap();
        assert_eq!(accepted, s.doc);
        assert_eq!(trigon_io::glb::write(&trigon_model::build(&s.doc).unwrap(), &[]), glb);
        let properties = s.execute("element.get", json!({"id":"b.rooflight.light"})).unwrap();
        assert!(properties.to_string().contains("height_mm"));
        s.execute("element.set", json!({"id":"b.rooflight.light.flashing","properties":{"width_mm":950}})).unwrap();
        assert_eq!(s.doc.buildings[0].roof.rooflights()[0].width_mm, 950.);
        s.execute("edit.undo", json!({})).unwrap();
        assert_eq!(s.doc, accepted);
        s.execute("rooflight.remove", json!({"id":"light"})).unwrap();
        assert!(s.doc.buildings[0].roof.rooflights().is_empty());
        s.execute("edit.undo", json!({})).unwrap();
        assert_eq!(s.doc, accepted);
        let reopened: trigon_model::Document = serde_json::from_slice(&serde_json::to_vec(&s.doc).unwrap()).unwrap();
        assert_eq!(s.doc, reopened);
        assert_eq!(built, trigon_model::build(&reopened).unwrap());
    }
}
