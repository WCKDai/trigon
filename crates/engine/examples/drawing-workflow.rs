//! Reproduce a local, offline model → drawings → pick → edit/undo → schedules → IFC workflow.
//! Run: cargo run -p trigon-engine --example drawing-workflow --locked -- out/drawing-workflow

use std::error::Error;
use std::path::PathBuf;

use serde_json::json;
use trigon_engine::{DrawingElementKind, Session, drawing_view};
use trigon_model::build::View;

fn require(condition: bool, message: &str) -> Result<(), Box<dyn Error>> {
    if condition { Ok(()) } else { Err(message.into()) }
}

fn main() -> Result<(), Box<dyn Error>> {
    let directory = std::env::args_os().nth(1).map_or_else(|| PathBuf::from("out/drawing-workflow"), PathBuf::from);
    std::fs::create_dir_all(&directory)?;
    // The existing sample is built entirely by deterministic model code. No account, imagery,
    // network call or private project is needed; all subsequent actions use normal commands.
    let mut session = Session::new(Box::new(trigon_data::Live::new(trigon_data::Credentials::default())));
    session.doc = trigon_io::ifc::sample::document()?;
    for (id, name, kind) in [
        ("ground", "Stueplan", json!({"view":"plan","level":0})),
        ("street", "Gadefacade", json!({"view":"facade","facade":"b1.f1"})),
        ("section", "Snit A–A", json!({"view":"section","from":[6000,-1000],"to":[6000,17000],"depth_mm":20000})),
    ] {
        session.execute("view.define", json!({"id":id,"name":name,"kind":kind}))?;
    }
    session.execute(
        "annotation.define",
        json!({
            "id":"front-width","view":"ground","kind":{"annotation":"dimension","from":[0,0],"to":[18000,0],"offset_mm":-1300}
        }),
    )?;
    let mut picks = Vec::new();
    let mut moved_target = None;
    for id in ["ground", "street", "section"] {
        let before = session.doc.clone();
        let drawing = drawing_view(&session.doc, id, View::latest(&session.doc))?;
        let line = drawing.element_segments.iter().find(|line| line.kind == DrawingElementKind::Opening).ok_or("view has no visible opening")?;
        let point = [(line.segment[0][0] + line.segment[1][0]) / 2.0, (line.segment[0][1] + line.segment[1][1]) / 2.0];
        let expected = drawing.pick_element(point, 1.0).ok_or("visible opening was not pickable")?;
        let result = session.execute("view.pick", json!({"id":id,"point":point,"tolerance_mm":1.0}))?;
        require(result["hit"] == serde_json::to_value(&expected)?, "command and preview picks differ")?;
        require(session.doc == before, "drawing or picking changed the document")?;
        if id == "ground" {
            moved_target = Some(expected.id);
        }
        picks.push(json!({"view":id,"query_mm":point,"result":result}));
        for extension in ["svg", "png", "pdf", "dxf"] {
            session.execute("view.export", json!({"id":id,"path":directory.join(format!("{id}.{extension}"))}))?;
        }
    }
    let before = session.doc.clone();
    let target = moved_target.ok_or("no edit target")?;
    session.execute("edit.move", json!({"targets":[target],"delta":[100,0,0]}))?;
    require(session.doc.buildings != before.buildings, "move did not change the building")?;
    require(session.execute("edit.undo", json!({}))?["undone"] == true, "move was not undoable")?;
    require(session.doc == before, "undo did not restore the document exactly")?;
    let schedule = session.execute("schedule.openings", json!({}))?;
    require(schedule["totals"]["instances"] == 55, "sample opening inventory changed")?;
    session.execute("edit.copy", json!({"targets":[target],"delta":[1600,0,0],"count":1}))?;
    let copied = session.execute("schedule.openings", json!({}))?;
    require(copied["totals"]["instances"] == 56, "schedule did not update after copying an opening")?;
    require(session.execute("edit.undo", json!({}))?["undone"] == true, "opening copy was not undoable")?;
    require(session.doc == before, "copy undo did not restore the document exactly")?;
    require(session.execute("schedule.openings", json!({}))? == schedule, "schedule did not restore after undo")?;
    session.execute("schedule.export", json!({"path":directory.join("opening-schedule.json")}))?;
    session.execute("schedule.export", json!({"path":directory.join("opening-schedule.csv")}))?;
    session.execute("document.save", json!({"path":directory.join("sample.trigon")}))?;
    session.execute("export.gltf", json!({"path":directory.join("sample.glb")}))?;
    session.execute("export.ifc", json!({"path":directory.join("sample.ifc")}))?;
    let exchange = session.execute("ifc.check", json!({"path":directory.join("sample.ifc")}))?;
    let readers = exchange["readers"].as_array().ok_or("missing IFC reader results")?;
    require(readers.len() == 2 && readers.iter().all(|reader| reader["failures"] == 0), "an IFC reader reported failures")?;
    let comparisons = exchange["versus_this_model"].as_array().ok_or("missing IFC/model comparisons")?;
    require(
        comparisons.len() == 2
            && comparisons.iter().all(|comparison| {
                comparison["matched"] == comparison["model_elements"]
                    && comparison["max_bounds_deviation_mm"].as_f64().is_some_and(|deviation| deviation < 0.01)
            }),
        "IFC reader geometry did not match the model within 0.01 mm",
    )?;
    require(
        readers.iter().all(|reader| {
            reader["by_class"]["IFCWINDOW"] == schedule["totals"]["windows"] && reader["by_class"]["IFCDOOR"] == schedule["totals"]["doors"]
        }),
        "schedule occurrence counts differ from independent IFC readers",
    )?;
    let report = json!({"picks":picks,"move_undo_exact":true,"copy_undo_schedule_exact":true,"opening_schedule_totals":schedule["totals"],"ifc":exchange,"native_mouse_input_verified":false});
    std::fs::write(directory.join("verification.json"), serde_json::to_vec_pretty(&report)?)?;
    println!("Verified local model, three drawing views, picking, opening schedule, exact edit/undo and two IFC readers in {}", directory.display());
    Ok(())
}
