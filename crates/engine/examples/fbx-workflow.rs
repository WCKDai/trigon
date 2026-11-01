//! Public, offline FBX exchange fixture. Requires a separately installed Blender.
//! cargo run --locked -p trigon-engine --example fbx-workflow -- out/fbx-workflow

use serde_json::json;
use std::{error::Error, path::PathBuf};
use trigon_engine::Session;

fn main() -> Result<(), Box<dyn Error>> {
    let directory = std::env::args_os().nth(1).map_or_else(|| PathBuf::from("out/fbx-workflow"), PathBuf::from);
    std::fs::create_dir_all(&directory)?;
    // Long IDs deliberately exercise application object-name limits. The
    // complete semantic identity is transported in GLB extras and FBX custom properties.
    let id = format!("original.copenhagen.{}rendering", "long-source.".repeat(28));
    let facade = format!("{id}.f1");
    let mut s = Session::new(Box::new(trigon_data::Live::new(trigon_data::Credentials::default())));
    s.doc.buildings.push(trigon_model::Building::new(
        &id,
        "Original exchange fixture",
        &[[0., 0.], [18000., 0.], [18000., 10000.], [0., 10000.]],
        2,
        3500.,
    )?);
    s.execute("types.install_copenhagen", json!({}))?;
    s.execute("facade.set_bays", json!({"facade":facade,"count":3}))?;
    for (bay, ty) in ["dk.window.segmental", "dk.window.applied-bars", "dk.door.glazed-panel"].iter().enumerate() {
        s.execute("facade.place", json!({"facade":facade,"type":ty,"levels":[0,1],"bays":[bay]}))?;
    }
    s.execute(
        "building.set_roof",
        json!({"kind":"pitched","pitch_deg":45,"gables":[format!("{id}.f2"),format!("{id}.f4")],"overhang_mm":250,"thickness_mm":250}),
    )?;
    s.execute(
        "site.set_terrain",
        json!({"terrain":{"origin":[-5000,-5000],"spacing_mm":[28000,20000],"heights_mm":[[0,0],[0,0]],"source":"illustrative:flat-ground"}}),
    )?;
    let before = s.doc.clone();
    s.execute("export.gltf", json!({"path":directory.join("original.glb")}))?;
    let report = s.execute("export.fbx", json!({"path":directory.join("original.fbx")}))?;
    if s.doc != before {
        return Err("FBX export changed the document".into());
    }
    s.execute("export.ifc", json!({"path":directory.join("original.ifc")}))?;
    s.execute("document.save", json!({"path":directory.join("original.trigon")}))?;
    std::fs::write(directory.join("verification.json"), serde_json::to_vec_pretty(&report)?)?;
    println!("Verified FBX rendering exchange, including long source identities, in {}", directory.display());
    Ok(())
}
