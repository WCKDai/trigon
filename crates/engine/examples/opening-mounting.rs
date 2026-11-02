//! Public thin-wall mounting reproduction and exact Undo check; no external data.
//! cargo run --locked -p trigon-engine --example opening-mounting -- out/opening-mounting

use serde_json::json;
use std::{error::Error, path::PathBuf};
use trigon_engine::Session;

fn main() -> Result<(), Box<dyn Error>> {
    let directory = std::env::args_os().nth(1).map_or_else(|| PathBuf::from("out/opening-mounting"), PathBuf::from);
    std::fs::create_dir_all(&directory)?;
    let mut s = Session::new(Box::new(trigon_data::Live::new(trigon_data::Credentials::default())));
    s.doc.buildings.push(trigon_model::Building::new(
        "b",
        "Original mounting fixture",
        &[[0., 0.], [10000., 0.], [10000., 5000.], [0., 5000.]],
        1,
        3500.,
    )?);
    s.execute("types.install_copenhagen", json!({}))?;
    s.execute("building.set_wall_thickness", json!({"thickness":100}))?;
    s.execute("facade.set_bays", json!({"facade":"b.f1","count":3}))?;
    for (bay, ty) in ["dk.window.dannebrog", "dk.window.segmental", "dk.door.six-panel"].iter().enumerate() {
        s.execute("facade.place", json!({"facade":"b.f1","type":ty,"levels":[0],"bays":[bay]}))?;
    }
    s.execute("export.gltf", json!({"path":directory.join("mounting.glb")}))?;
    s.execute("export.gltf", json!({"path":directory.join("section.glb"),"cut_level":0,"cut_mm":1500}))?;
    s.execute("export.ifc", json!({"path":directory.join("mounting.ifc")}))?;
    s.execute("document.save", json!({"path":directory.join("mounting.trigon")}))?;
    s.execute("schedule.export", json!({"path":directory.join("schedule.json")}))?;
    let before = s.doc.clone();
    s.execute("building.set_wall_thickness", json!({"thickness":360}))?;
    s.execute("edit.undo", json!({}))?;
    if s.doc != before {
        return Err("wall edit/Undo changed source document".into());
    }
    s.execute("export.gltf", json!({"path":directory.join("undo.glb")}))?;
    if std::fs::read(directory.join("mounting.glb"))? != std::fs::read(directory.join("undo.glb"))? {
        return Err("wall edit/Undo changed mesh bytes".into());
    }
    println!("Thin-wall reproduction and exact Undo verified in {}", directory.display());
    Ok(())
}
