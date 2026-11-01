//! Offline, public-safe presentation evidence: legacy vs configured plan, all four manual line
//! patterns and portable SVG/PNG/PDF/DXF. No credentials, network, native app or GPU.
use serde_json::json;
use std::{error::Error, path::PathBuf};
use trigon_engine::Session;

fn main() -> Result<(), Box<dyn Error>> {
    let directory = std::env::args_os().nth(1).map_or_else(|| PathBuf::from("out/drafting-templates"), PathBuf::from);
    std::fs::create_dir_all(&directory)?;
    let mut s = Session::new(Box::new(trigon_data::Live::new(trigon_data::Credentials::default())));
    s.doc = trigon_io::ifc::sample::document()?;
    s.execute("view.define", json!({"id":"plan","name":"Ground floor · presentation study","kind":{"view":"plan","level":0}}))?;
    s.execute("view.export", json!({"id":"plan","path":directory.join("legacy.png")}))?;
    for (i, pattern) in ["solid", "dashed", "dotted", "dash_dot"].into_iter().enumerate() {
        let y = 2000.0 + i as f64 * 1000.0;
        s.execute("annotation.define",json!({"id":format!("line-{i}"),"view":"plan","kind":{"annotation":"line","from":[4000,y],"to":[13000,y],"stroke":{"colour":[184,65,44],"width_mm":0.5,"pattern":pattern}}}))?;
        s.execute("annotation.define",json!({"id":format!("label-{i}"),"view":"plan","kind":{"annotation":"text","at":[4000,y+200.0],"text":format!("Manual {pattern} · 0.50 mm")}}))?;
    }
    let baseline = s.doc.clone();
    s.execute("view.configure",json!({"id":"plan","template":{"name":"Presentation fit","line_weights":"fine","sizing":{"mode":"fit"},"range":{"top_mm":2300,"cut_mm":1200,"bottom_mm":0,"depth_mm":-1000}}}))?;
    let configured = s.doc.clone();
    let display = trigon_engine::drawing_view_display(&s.doc, "plan", trigon_model::build::View::latest(&s.doc))?;
    for (name, svg) in [("screen-full", display.exact.svg), ("screen-reduced", display.reduced_svg), ("screen-overview", display.overview_svg)] {
        std::fs::write(directory.join(format!("{name}.svg")), &svg)?;
        let tree = trigon_engine::parse_svg(&svg)?;
        let mut pixels = trigon_engine::resvg::tiny_skia::Pixmap::new(480, 270).ok_or("cannot allocate display evidence")?;
        pixels.fill(trigon_engine::resvg::tiny_skia::Color::WHITE);
        trigon_engine::resvg::render(&tree, trigon_engine::resvg::tiny_skia::Transform::from_scale(0.3, 0.3), &mut pixels.as_mut());
        std::fs::write(directory.join(format!("{name}.png")), pixels.encode_png()?)?;
    }
    for ext in ["svg", "png", "pdf", "dxf"] {
        s.execute("view.export", json!({"id":"plan","path":directory.join(format!("fit.{ext}"))}))?;
    }
    s.execute("view.configure",json!({"id":"plan","template":{"name":"Fixed 100 crop","line_weights":"standard","sizing":{"mode":"fixed","denominator":100,"centre":[9000,8000]},"crop":[1000,1500,17000,14500],"range":{"top_mm":2300,"cut_mm":1200,"bottom_mm":0,"depth_mm":-1000}}}))?;
    for ext in ["svg", "png", "pdf", "dxf"] {
        s.execute("view.export", json!({"id":"plan","path":directory.join(format!("fixed-100.{ext}"))}))?;
    }
    s.execute("edit.undo", json!({}))?;
    if s.doc != configured {
        return Err("fixed scale Undo did not restore the configured document".into());
    }
    s.execute("edit.undo", json!({}))?;
    if s.doc != baseline {
        return Err("template Undo did not restore the original document".into());
    }
    s.execute("document.save", json!({"path":directory.join("synthetic.trigon")}))?;
    println!("Portable drawing evidence and exact template Undo verified in {}", directory.display());
    Ok(())
}
