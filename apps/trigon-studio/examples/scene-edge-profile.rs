//! CPU-only scene preparation profile, excluding document parsing and model generation.
//! cargo run --release --locked -p trigon-studio --example scene-edge-profile -- [project.trigon] [samples]
// The application tests this module; the profiler must not register those tests a second time.
#[cfg(not(test))]
#[path = "../src/scene.rs"]
pub mod scene;

#[cfg(not(test))]
fn main() -> Result<(), Box<dyn std::error::Error>> {
    use serde_json::json;
    use std::hash::{Hash, Hasher};
    use std::time::Instant;
    use trigon_engine::Session;
    use trigon_model::build::{View, build_view};

    let mut args = std::env::args().skip(1);
    let path = args.next();
    if path.as_deref() == Some("--help") {
        println!(
            "scene-edge-profile [project.trigon] [samples=9]\nNo project uses a synthetic 160-window building. CPU scene preparation only; no GPU or frame pacing measurement."
        );
        return Ok(());
    }
    let samples = args.next().map(|value| value.parse::<usize>()).transpose()?.unwrap_or(9);
    if !(5..=100).contains(&samples) || args.next().is_some() {
        return Err("give a project path and optionally 5 to 100 samples".into());
    }
    let mut session = Session::new(Box::new(trigon_data::Live::new(trigon_data::Credentials::default())));
    if let Some(path) = path {
        session.execute("document.open", json!({"path":path}))?;
    } else {
        session.doc.buildings.push(trigon_model::Building::new(
            "b1",
            "Synthetic scene profile",
            &[[0., 0.], [24000., 0.], [24000., 16000.], [0., 16000.]],
            5,
            3000.,
        )?);
        for facade in ["b1.f1", "b1.f2", "b1.f3", "b1.f4"] {
            session.execute("facade.set_bays", json!({"facade":facade,"count":8}))?;
            session.execute("facade.place", json!({"facade":facade,"type":"window.casement.2","levels":[0,1,2,3,4]}))?;
        }
    }
    let start = Instant::now();
    let built = build_view(&session.doc, View::latest(&session.doc))?;
    let model_ms = start.elapsed().as_secs_f64() * 1000.;
    let warm = scene::Scene::from_built(&built, false);
    let mut faces_and_picking = std::collections::hash_map::DefaultHasher::new();
    faces_and_picking.write(bytemuck::cast_slice(&warm.vertices));
    warm.opaque.hash(&mut faces_and_picking);
    warm.tri_element.hash(&mut faces_and_picking);
    for element in &warm.elements {
        element.id.hash(&mut faces_and_picking);
        element.fingerprint.hash(&mut faces_and_picking);
        element.ranges.hash(&mut faces_and_picking);
    }
    let (triangles, edges, elements) = (warm.triangle_count(), warm.edge_count(), warm.elements.len());
    let signature = format!("{:016x}", faces_and_picking.finish());
    drop(warm);
    let mut costs = Vec::with_capacity(samples);
    for _ in 0..samples {
        let start = Instant::now();
        let scene = std::hint::black_box(scene::Scene::from_built(&built, false));
        costs.push(start.elapsed().as_secs_f64() * 1000.);
        if (scene.triangle_count(), scene.edge_count(), scene.elements.len()) != (triangles, edges, elements) {
            return Err("scene geometry/counts changed between repetitions".into());
        }
        drop(scene);
    }
    let mut sorted = costs.clone();
    sorted.sort_by(f64::total_cmp);
    let at = |q: f64| sorted.get(((sorted.len() - 1) as f64 * q).ceil() as usize).copied().unwrap_or(0.);
    println!(
        "{}",
        json!({"warmups":1,"samples_ms":costs,"median_ms":at(0.5),"p95_ms":at(0.95),"model_build_ms_excluded":model_ms,
        "triangles":triangles,"feature_edges":edges,"elements":elements,"faces_and_picking_signature":signature,
        "scope":"CPU Scene::from_built only; geometry generated once; GPU, textures, site/room append and frame pacing excluded. Signature comparable only with the same Rust toolchain."})
    );
    Ok(())
}
