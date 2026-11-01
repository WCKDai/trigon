//! Shared CPU raster region and cache-key mechanics, also used by the headless pan profile.
use eframe::egui;
use std::time::Instant;
use trigon_engine::resvg::{self, tiny_skia, usvg};

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Key {
    pub generation: u64,
    pub origin: [f32; 2],
    pub scale: f32,
    pub size: [u32; 2],
}
pub const MAX_SIDE: u32 = 4096;

pub fn exact_key(generation: u64, centre: [f32; 2], zoom: f32, view: [f32; 2], ppp: f32) -> Option<Key> {
    if !zoom.is_finite() || zoom <= 0.0 || !ppp.is_finite() || ppp <= 0.0 || centre.iter().chain(view.iter()).any(|v| !v.is_finite()) {
        return None;
    }
    let origin = [centre[0] - view[0] / 2.0 / zoom, centre[1] - view[1] / 2.0 / zoom];
    let size = [(view[0] * ppp).round().max(1.0) as u32, (view[1] * ppp).round().max(1.0) as u32];
    Some(Key { generation, origin, scale: zoom * ppp, size })
}
/// Cache an overscanned region on a 128-pixel grid, with 1/8-octave upward resolution buckets.
/// The cached image remains at least screen resolution unless the bounded raster budget is hit.
pub fn key(generation: u64, centre: [f32; 2], zoom: f32, view: [f32; 2], ppp: f32) -> Option<Key> {
    let exact = exact_key(generation, centre, zoom, view, ppp)?;
    const TILE: f32 = 128.0;
    let bucket = 2.0_f32.powf((exact.scale.log2() * 8.0).ceil() / 8.0);
    let limit = zoom * (MAX_SIDE as f32 - 3.0 * TILE) / view[0].max(view[1]).max(1.0);
    let scale = bucket.min(limit);
    if !scale.is_finite() || scale <= 0.0 {
        return None;
    }
    let origin = exact.origin.map(|v| (v * scale / TILE).floor() * TILE / scale - TILE / scale);
    // Reserve the whole resolution bucket, so fractional zoom ticks do not change raster size.
    let step = 2.0_f32.powf(1.0 / 8.0);
    let size = view.map(|v| ((v.max(1.0) * ppp * step).min(MAX_SIDE as f32 - 3.0 * TILE) + 3.0 * TILE) / TILE).map(|v| v.ceil() as u32 * 128);
    if origin.iter().any(|v| !v.is_finite()) {
        return None;
    }
    Some(Key { generation, origin, scale, size: size.map(|v| v.clamp(1, MAX_SIDE)) })
}
/// Interaction uses exact current-generation vector geometry, while its cached pixels move live.
pub fn ready(shown: Key, wanted: Key) -> bool {
    shown.generation == wanted.generation
}

#[derive(Clone, Copy, Default)]
pub struct Times {
    pub allocate_ms: f64,
    pub render_ms: f64,
    pub convert_ms: f64,
}

pub fn profiled(tree: &usvg::Tree, origin: [f32; 2], scale: f32, size: [u32; 2]) -> Option<(egui::ColorImage, Times)> {
    let (w, h) = (size[0].clamp(1, MAX_SIDE), size[1].clamp(1, MAX_SIDE));
    if !(scale > 0.0 && scale.is_finite()) || !origin.iter().all(|v| v.is_finite()) {
        return None;
    }
    let start = Instant::now();
    let mut pixmap = tiny_skia::Pixmap::new(w, h)?;
    pixmap.fill(tiny_skia::Color::WHITE);
    let allocate_ms = start.elapsed().as_secs_f64() * 1000.0;
    let t = tiny_skia::Transform::from_row(scale, 0.0, 0.0, scale, -origin[0] * scale, -origin[1] * scale);
    let start = Instant::now();
    resvg::render(tree, t, &mut pixmap.as_mut());
    let render_ms = start.elapsed().as_secs_f64() * 1000.0;
    let start = Instant::now();
    let image = egui::ColorImage::from_rgba_premultiplied([w as usize, h as usize], pixmap.data());
    let convert_ms = start.elapsed().as_secs_f64() * 1000.0;
    Some((image, Times { allocate_ms, render_ms, convert_ms }))
}
pub fn rasterize(tree: &usvg::Tree, origin: [f32; 2], scale: f32, size: [u32; 2]) -> Option<egui::ColorImage> {
    profiled(tree, origin, scale, size).map(|(image, _)| image)
}

pub fn accept_result(completed: Key, wanted: Key) -> bool {
    completed == wanted
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn small_pan_keeps_interaction_ready_with_current_generation_pixels() {
        let shown = key(1, [800.0, 450.0], 0.65, [1000.0, 700.0], 1.0).unwrap();
        let moved = key(1, [1004.0, 703.0], 0.65, [1000.0, 700.0], 1.0).unwrap();
        assert_ne!(shown, moved);
        assert!(ready(shown, moved), "a transformed current-generation picture must not stall exact overlays/picking");
        let changed = key(2, [804.0, 453.0], 0.65, [1000.0, 700.0], 1.0).unwrap();
        assert!(!ready(shown, changed), "changed documents still wait for their own pixels");
    }
    #[test]
    fn raster_keys_bound_memory_cover_the_view_and_reuse_small_pan_and_zoom_steps() {
        let first = key(1, [800.0, 450.0], 0.65, [1000.0, 700.0], 2.0).unwrap();
        let mut unique = Vec::new();
        for frame in 0..96 {
            let zoom = 0.65 * 1.005_f32.powi(frame);
            let centre = [800.0 + frame as f32 * 0.5, 450.0];
            let k = key(1, centre, zoom, [1000.0, 700.0], 2.0).unwrap();
            let exact = exact_key(1, centre, zoom, [1000.0, 700.0], 2.0).unwrap();
            assert!(k.size.iter().all(|v| *v <= MAX_SIDE));
            assert!(k.origin[0] <= exact.origin[0] && k.origin[1] <= exact.origin[1]);
            assert!(k.origin[0] + k.size[0] as f32 / k.scale >= exact.origin[0] + 1000.0 / zoom);
            assert!(k.origin[1] + k.size[1] as f32 / k.scale >= exact.origin[1] + 700.0 / zoom);
            assert!(k.scale >= zoom * 2.0);
            if unique.last() != Some(&k) {
                unique.push(k);
            }
        }
        assert!(unique.len() < 20, "96 pointer ticks should share bounded raster regions");
        assert!(first.size[0] > 2000);
        assert!(key(1, [0.0; 2], f32::NAN, [1000.0; 2], 2.0).is_none());
    }
    #[test]
    fn large_retina_viewports_stay_inside_the_raster_budget_without_losing_coverage() {
        for zoom in [0.02, 0.3, 1.0, 3.5, 200.0] {
            for ppp in [1.0, 1.5, 2.0, 3.0] {
                let view = [3000.0, 1500.0];
                let centre = [-400.0, 600.0];
                let k = key(1, centre, zoom, view, ppp).unwrap();
                let exact = exact_key(1, centre, zoom, view, ppp).unwrap();
                assert!(k.size.iter().all(|v| *v <= MAX_SIDE));
                for (axis, extent) in view.iter().enumerate() {
                    assert!(k.origin[axis] + k.size[axis] as f32 / k.scale >= exact.origin[axis] + extent / zoom - 0.1);
                }
            }
        }
    }
    #[test]
    fn obsolete_generations_and_superseded_rasters_are_rejected_before_upload() {
        let old = key(1, [800.0, 450.0], 0.65, [1000.0, 700.0], 1.0).unwrap();
        let current = key(2, [800.0, 450.0], 0.65, [1000.0, 700.0], 1.0).unwrap();
        assert!(!accept_result(old, current));
        assert!(accept_result(current, current));
    }
}
