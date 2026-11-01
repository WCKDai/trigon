//! Offline, original geometry; no client imagery, network access or external account.
//! cargo run --locked -p trigon-engine --example copenhagen-openings -- out/copenhagen-openings

use serde_json::json;
use std::{error::Error, path::PathBuf};
use trigon_engine::Session;

fn main() -> Result<(), Box<dyn Error>> {
    let directory = std::env::args_os().nth(1).map_or_else(|| PathBuf::from("out/copenhagen-openings"), PathBuf::from);
    std::fs::create_dir_all(&directory)?;
    let mut s = Session::new(Box::new(trigon_data::Live::new(trigon_data::Credentials::default())));
    s.doc.buildings.push(trigon_model::Building::new(
        "b",
        "Original joinery gallery",
        &[[0., 0.], [42000., 0.], [42000., 8000.], [0., 8000.]],
        1,
        3500.,
    )?);
    s.execute("types.install_copenhagen", json!({}))?;
    s.execute("facade.set_bays", json!({"facade":"b.f1","count":12}))?;
    for (i, ty) in trigon_model::component::copenhagen_types().iter().enumerate() {
        s.execute("facade.place", json!({"facade":"b.f1","type":ty.id,"levels":[0],"bays":[i]}))?;
    }
    s.execute("site.set_terrain",json!({"terrain":{"origin":[-8000,-8000],"spacing_mm":[58000,24000],"heights_mm":[[0,0],[0,0]],"source":"illustrative:flat-ground","attribution":"Original public-safe presentation assumption"}}))?;
    s.execute("view.define", json!({"id":"street","name":"Original family gallery","kind":{"view":"facade","facade":"b.f1"}}))?;
    for ext in ["png", "svg", "dxf"] {
        s.execute("view.export", json!({"id":"street","path":directory.join(format!("gallery.{ext}"))}))?;
    }
    s.execute("export.gltf", json!({"path":directory.join("gallery.glb")}))?;
    s.execute("export.ifc", json!({"path":directory.join("gallery.ifc")}))?;
    s.execute("document.save", json!({"path":directory.join("gallery.trigon")}))?;
    s.execute("schedule.export", json!({"path":directory.join("schedule.json")}))?;
    let before = s.doc.clone();
    s.execute("element.set", json!({"id":"b.f1.L0.B5","properties":{"width_mm":1300,"height_mm":2100}}))?;
    s.execute("edit.undo", json!({}))?;
    if s.doc != before {
        return Err("edit/undo did not restore the source document".into());
    }
    s.execute("export.gltf", json!({"path":directory.join("undo.glb")}))?;
    if std::fs::read(directory.join("gallery.glb"))? != std::fs::read(directory.join("undo.glb"))? {
        return Err("edit/undo changed portable geometry".into());
    }
    let check = s.execute("ifc.check", json!({"path":directory.join("gallery.ifc")}))?;
    std::fs::write(
        directory.join("verification.json"),
        serde_json::to_vec_pretty(&json!({"exact_undo":true,"ifc":check,"family_dimensions":"unmeasured illustrative defaults"}))?,
    )?;
    println!("Original joinery gallery: {}", directory.display());
    Ok(())
}
