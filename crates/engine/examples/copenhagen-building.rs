//! Original apartment-block study; all dimensions are illustrative family parameters.
//! cargo run --locked -p trigon-engine --example copenhagen-building -- out/copenhagen-building

use serde_json::json;
use std::{error::Error, path::PathBuf};
use trigon_engine::Session;

fn main() -> Result<(), Box<dyn Error>> {
    let dir = std::env::args_os().nth(1).map_or_else(|| PathBuf::from("out/copenhagen-building"), PathBuf::from);
    std::fs::create_dir_all(&dir)?;
    let mut s = Session::new(Box::new(trigon_data::Live::new(trigon_data::Credentials::default())));
    s.doc.buildings.push(trigon_model::Building::new(
        "b",
        "Original Copenhagen apartment-block study",
        &[[0., 1500.], [1500., 0.], [30000., 0.], [30000., 12000.], [0., 12000.]],
        5,
        3400.,
    )?);
    s.execute("types.install_copenhagen", json!({}))?;
    let facades = s.doc.buildings[0].facades.iter().map(|f| f.id.clone()).collect::<Vec<_>>();
    for facade in &facades {
        let b = &s.doc.buildings[0];
        let f = b.facade(facade)?;
        let (a, b) = (b.footprint[f.edge], b.footprint[(f.edge + 1) % b.footprint.len()]);
        let length = (b[0] - a[0]).hypot(b[1] - a[1]);
        let count = (length / 3500.).floor().max(1.) as usize;
        s.execute("facade.set_bays", json!({"facade":facade,"count":count}))?;
        s.execute("facade.place", json!({"facade":facade,"type":"dk.window.dannebrog","levels":[0,1,2,3,4]}))?;
    }
    s.execute("facade.place", json!({"facade":"b.f2","type":"dk.door.double-panel","levels":[0],"bays":[3]}))?;
    s.execute("facade.set_finish", json!({"facades":facades,"finish":"yellow_brick"}))?;
    s.execute(
        "facade.set_bands",
        json!({"facades":facades,"bands":[
            {"levels":[0],"bottom":0,"top":650,"projection":25},
            {"levels":[1,2,3,4],"bottom":-130,"top":80,"projection":40},
            {"levels":[4],"bottom":3000,"top":3110,"projection":70},
            {"levels":[4],"bottom":3110,"top":3220,"projection":110},
            {"levels":[4],"bottom":3220,"top":3400,"projection":180}
        ]}),
    )?;
    s.execute("building.set_roof",json!({"kind":"mansard","lower_pitch_deg":68,"upper_pitch_deg":25,"break_run_mm":1300,"thickness_mm":220,"overhang_mm":250,"gables":["b.f3"],"gutters":true}))?;
    s.execute(
        "site.set_terrain",
        json!({"terrain":{"origin":[-10000,-10000],"spacing_mm":[50000,32000],"heights_mm":[[0,0],[0,0]],"source":"illustrative:flat-ground"}}),
    )?;
    s.execute("export.gltf", json!({"path":dir.join("building.glb")}))?;
    s.execute("export.ifc", json!({"path":dir.join("building.ifc")}))?;
    s.execute("document.save", json!({"path":dir.join("building.trigon")}))?;
    s.execute("schedule.export", json!({"path":dir.join("schedule.json")}))?;
    let before = s.doc.clone();
    s.execute("element.set", json!({"id":"b.roof","properties":{"break_run_mm":1500}}))?;
    s.execute("edit.undo", json!({}))?;
    if s.doc != before {
        return Err("roof edit/Undo changed the document".into());
    }
    s.execute("export.gltf", json!({"path":dir.join("undo.glb")}))?;
    if std::fs::read(dir.join("building.glb"))? != std::fs::read(dir.join("undo.glb"))? {
        return Err("roof edit/Undo changed mesh bytes".into());
    }
    let report = s.execute("ifc.check", json!({"path":dir.join("building.ifc")}))?;
    std::fs::write(dir.join("ifc-check.json"), serde_json::to_vec_pretty(&report)?)?;
    println!("Original apartment-block study and exact roof edit/Undo: {}", dir.display());
    Ok(())
}
