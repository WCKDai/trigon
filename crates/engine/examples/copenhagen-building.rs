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
    for ty in trigon_model::balcony::copenhagen_types() {
        s.execute("types.define", serde_json::to_value(ty)?)?;
    }
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
    for (facade, bays, ty) in [("b.f2", vec![1, 5], "dk.balcony.bowed"), ("b.f4", vec![1, 3, 5], "dk.balcony.chamfered")] {
        s.execute("facade.place", json!({"facade":facade,"type":"door.french","levels":[1,2,3,4],"bays":bays}))?;
        s.execute("facade.place", json!({"facade":facade,"type":ty,"levels":[1,2,3,4],"bays":bays}))?;
    }
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
    let mut dormers = Vec::new();
    for (facade, indices) in [("b.f2", vec![1, 2, 3, 4, 5, 6]), ("b.f4", vec![1, 3, 5])] {
        let bays = s.doc.buildings[0].facade(facade)?.bays.clone();
        for bay in indices {
            let u = bays.get(bay).ok_or("dormer bay is absent")?;
            dormers.push(
                json!({"id":format!("{facade}-{bay}"),"facade":facade,"u_mm":u,"window_type":"dk.window.dannebrog","width_mm":1100,"height_mm":1200}),
            );
        }
    }
    s.execute("building.set_roof",json!({"kind":"mansard","lower_pitch_deg":68,"upper_pitch_deg":25,"break_run_mm":1300,"thickness_mm":220,"overhang_mm":250,"gables":["b.f3"],"gutters":true,"dormers":dormers}))?;
    s.execute(
        "site.set_terrain",
        json!({"terrain":{"origin":[-10000,-10000],"spacing_mm":[50000,32000],"heights_mm":[[0,0],[0,0]],"source":"illustrative:flat-ground"}}),
    )?;
    s.execute("export.gltf", json!({"path":dir.join("building.glb")}))?;
    s.execute("export.ifc", json!({"path":dir.join("building.ifc")}))?;
    s.execute("document.save", json!({"path":dir.join("building.trigon")}))?;
    s.execute("schedule.export", json!({"path":dir.join("schedule.json")}))?;
    s.execute("view.define", json!({"id":"balcony-plan","name":"Original balcony plan","kind":{"view":"plan","level":1}}))?;
    for ext in ["svg", "png", "dxf"] {
        s.execute("view.export", json!({"id":"balcony-plan","path":dir.join(format!("plan.{ext}"))}))?;
    }
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
