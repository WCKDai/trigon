//! CPU-only reproduction of Studio pan/zoom raster work; never opens a window or uses the GPU.
//! cargo run --release --locked -p trigon-studio --example drawing-pan-profile
#[path = "../src/drawing_raster.rs"]
mod drawing_raster;
use serde_json::json;
use std::{error::Error, time::Instant};
use trigon_engine::{Session, drawing_view};
use trigon_model::build::View;

fn percentile(values: &[f64], q: f64) -> f64 {
    let mut v = values.to_vec();
    v.sort_by(f64::total_cmp);
    v.get(((v.len().saturating_sub(1)) as f64 * q).ceil() as usize).copied().unwrap_or(0.0)
}
fn main() -> Result<(), Box<dyn Error>> {
    let ppp = std::env::args().nth(1).map(|v| v.parse::<f32>()).transpose()?.unwrap_or(1.0);
    if !ppp.is_finite() || !(0.5..=4.0).contains(&ppp) {
        return Err("pixels per point must be 0.5 to 4".into());
    }
    let mut s = Session::new(Box::new(trigon_data::Live::new(trigon_data::Credentials::default())));
    s.doc.buildings.push(trigon_model::Building::new(
        "b1",
        "Synthetic pan profile",
        &[[0.0, 0.0], [24000.0, 0.0], [24000.0, 16000.0], [0.0, 16000.0]],
        5,
        3000.0,
    )?);
    for facade in ["b1.f1", "b1.f2", "b1.f3", "b1.f4"] {
        s.execute("facade.set_bays", json!({"facade":facade,"count":8}))?;
        s.execute("facade.place", json!({"facade":facade,"type":"window.casement.2","levels":[0,1,2,3,4]}))?;
    }
    for (id, kind) in [("plan", json!({"view":"plan","level":0})), ("elevation", json!({"view":"facade","facade":"b1.f1"}))] {
        s.execute("view.define", json!({"id":id,"name":id,"kind":kind}))?;
        let drawing = drawing_view(&s.doc, id, View::latest(&s.doc))?;
        let start = Instant::now();
        let tree = trigon_engine::parse_svg(&drawing.svg)?;
        let parse_ms = start.elapsed().as_secs_f64() * 1000.0;
        // Warm up raster code/fonts/allocator before the five measured repetitions.
        std::hint::black_box(
            drawing_raster::rasterize(&tree, [0.0, 0.0], 0.65 * ppp, [(1000.0 * ppp) as u32, (700.0 * ppp) as u32]).ok_or("raster warmup failed")?,
        );
        for gesture in ["pan", "zoom"] {
            for repeat in 0..5 {
                let mut shown = None;
                let mut costs = Vec::new();
                let mut renders = Vec::new();
                let mut alloc = Vec::new();
                let mut converts = Vec::new();
                let mut bytes = 0u64;
                let mut stalls = 0;
                let start = Instant::now();
                for frame in 0..96 {
                    let zoom = if gesture == "zoom" { 0.65 * 1.005_f32.powi(frame) } else { 0.65 };
                    let centre =
                        if gesture == "pan" { [800.0 + frame as f32 * 0.5 / zoom, 450.0 + frame as f32 * 0.25 / zoom] } else { [800.0, 450.0] };
                    let key = drawing_raster::key(1, centre, zoom, [1000.0, 700.0], ppp).ok_or("bad raster key")?;
                    if shown.is_none_or(|previous| !drawing_raster::ready(previous, key)) {
                        stalls += 1;
                    }
                    let before = Instant::now();
                    if shown != Some(key) {
                        let (image, times) = drawing_raster::profiled(&tree, key.origin, key.scale, key.size).ok_or("raster failed")?;
                        if !drawing_raster::accept_result(key, key) {
                            return Err("current raster rejected".into());
                        }
                        bytes += image.pixels.len() as u64 * 4;
                        std::hint::black_box(image);
                        renders.push(times.render_ms);
                        alloc.push(times.allocate_ms);
                        converts.push(times.convert_ms);
                        shown = Some(key);
                    }
                    costs.push(before.elapsed().as_secs_f64() * 1000.0);
                }
                println!(
                    "{}",
                    json!({"view":id,"gesture":gesture,"repeat":repeat,"frames":96,"pixels_per_point":ppp,"viewport_points":[1000,700],"parse_ms":parse_ms,"gesture_ms":start.elapsed().as_secs_f64()*1000.0,"raster_jobs":renders.len(),"readiness_stalls":stalls,"texture_bytes_candidate":bytes,"frame_cpu_p50_ms":percentile(&costs,0.5),"frame_cpu_p95_ms":percentile(&costs,0.95),"raster_render_p50_ms":percentile(&renders,0.5),"raster_render_p95_ms":percentile(&renders,0.95),"allocation_p50_ms":percentile(&alloc,0.5),"conversion_p50_ms":percentile(&converts,0.5)})
                );
            }
        }
    }
    Ok(())
}
