//! Shared CPU raster region and cache-key mechanics, also used by the headless pan profile.
use eframe::egui;
use std::time::Instant;
use trigon_engine::resvg::{self, tiny_skia, usvg};

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Key {
    pub generation: u64,
    pub detail: u8,
    /// Logical window points per source SVG pixel, bucketed independently of Retina density.
    pub line_zoom: f32,
    pub origin: [f32; 2],
    pub scale: f32,
    pub size: [u32; 2],
}
pub const MAX_SIDE: u32 = 4096;
const TILE: f32 = 128.0;

/// Pen styling follows logical zoom, never physical pixels or the bounded raster resolution.
pub fn stroke_zoom(zoom: f32) -> Option<f32> {
    (zoom.is_finite() && zoom > 0.0).then(|| 2.0_f32.powf((zoom.log2() * 8.0).ceil() / 8.0).clamp(0.02, 200.0))
}

pub fn exact_key(generation: u64, centre: [f32; 2], zoom: f32, view: [f32; 2], ppp: f32) -> Option<Key> {
    if !zoom.is_finite()
        || zoom <= 0.0
        || !ppp.is_finite()
        || ppp <= 0.0
        || centre.iter().chain(view.iter()).any(|v| !v.is_finite())
        || view.iter().any(|v| *v <= 0.0)
    {
        return None;
    }
    let origin = [centre[0] - view[0] / 2.0 / zoom, centre[1] - view[1] / 2.0 / zoom];
    let size = [(view[0] * ppp).round().max(1.0) as u32, (view[1] * ppp).round().max(1.0) as u32];
    let band = stroke_zoom(zoom)?;
    let detail = if band < 0.4 {
        2
    } else if band < 0.8 {
        1
    } else {
        0
    };
    let scale = zoom * ppp;
    if origin.iter().any(|v| !v.is_finite()) || !scale.is_finite() || scale <= 0.0 {
        return None;
    }
    Some(Key { generation, detail, line_zoom: band, origin, scale, size })
}
/// Cache an overscanned region on a 128-pixel grid, with 1/8-octave upward resolution buckets.
/// The cached image remains at least screen resolution unless the bounded raster budget is hit.
pub fn key(generation: u64, centre: [f32; 2], zoom: f32, view: [f32; 2], ppp: f32) -> Option<Key> {
    let exact = exact_key(generation, centre, zoom, view, ppp)?;
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
    Some(Key { generation, detail: exact.detail, line_zoom: exact.line_zoom, origin, scale, size: size.map(|v| v.clamp(1, MAX_SIDE)) })
}

/// Reuse a cached image only at the current raster resolution and screen style, and only when
/// the actual visible sheet rectangle is strictly inside it. On a changed grid axis keep a full
/// overscan tile for replacement rendering, plus one physical pixel for filtering.
/// `wanted` includes the MAX_SIDE resolution cap; comparing against zoom * Retina would reject
/// valid capped images. Requiring the same scale is deliberately more conservative than merely
/// permitting a higher-resolution image. Source/base Arc identities are checked by PlanView.
pub fn covers_viewport(shown: Key, wanted: Key, centre: [f32; 2], zoom: f32, view: [f32; 2]) -> bool {
    let valid = |key: Key| {
        key.detail <= 2
            && key.line_zoom.is_finite()
            && (0.02..=200.0).contains(&key.line_zoom)
            && key.scale.is_finite()
            && key.scale > 0.0
            && key.origin.iter().all(|v| v.is_finite())
            && key.size.iter().all(|v| (1..=MAX_SIDE).contains(v))
    };
    if !valid(shown)
        || !valid(wanted)
        || shown.generation != wanted.generation
        || shown.detail != wanted.detail
        || shown.line_zoom.to_bits() != wanted.line_zoom.to_bits()
        || shown.scale.to_bits() != wanted.scale.to_bits()
        || !zoom.is_finite()
        || zoom <= 0.0
        || centre.iter().any(|v| !v.is_finite())
        || view.iter().any(|v| !v.is_finite() || *v <= 0.0)
    {
        return false;
    }
    for axis in 0..2 {
        // An unchanged axis still has its original canonical extent. Its exact 128-pixel
        // margin must not prevent reuse on the other axis, just as for an unchanged key.
        let unchanged = shown.origin[axis] == wanted.origin[axis] && shown.size[axis] == wanted.size[axis];
        let guard = if unchanged { 1.0 } else { f64::from(TILE) + 1.0 } / f64::from(shown.scale);
        // Match the image endpoint arithmetic used by the painter. Use the true point extent
        // for the viewport, rather than the rounded snapshot size or wanted overscan bounds.
        let image_end = shown.origin[axis] + shown.size[axis] as f32 / shown.scale;
        let half = f64::from(view[axis]) / (2.0 * f64::from(zoom));
        let first = f64::from(centre[axis]) - half;
        let last = f64::from(centre[axis]) + half;
        if !image_end.is_finite() || first >= last || first <= f64::from(shown.origin[axis]) + guard || last >= f64::from(image_end) - guard {
            return false;
        }
    }
    true
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
    fn coverage_refreshes_before_a_pan_exhausts_one_tile_of_prefetch_headroom() {
        let zoom = 0.65;
        let view = [1000.0, 700.0];
        let ppp = 2.0;
        let shown = key(7, [800.0, 450.0], zoom, view, ppp).unwrap();
        for axis in 0..2 {
            for margin in [130.0, 64.0] {
                let mut centre = shown.origin;
                for (dimension, coordinate) in centre.iter_mut().enumerate() {
                    *coordinate += shown.size[dimension] as f32 / (2.0 * shown.scale);
                }
                centre[axis] = shown.origin[axis] + shown.size[axis] as f32 / shown.scale - view[axis] / (2.0 * zoom) - margin / shown.scale;
                let wanted = key(7, centre, zoom, view, ppp).unwrap();
                assert_ne!(shown, wanted, "the gesture must leave the original snapped key");
                assert_eq!(
                    covers_viewport(shown, wanted, centre, zoom, view),
                    margin > 129.0,
                    "axis {axis}: keep a full overscan tile and filtering pixel while a replacement renders"
                );
            }
        }
    }

    #[test]
    fn covering_images_survive_grid_crossings_but_never_uncovered_edges() {
        let zoom = 4.0;
        let view = [120.0, 80.0];
        let start = [60.0, 50.0];
        let shown = key(7, start, zoom, view, 2.0).unwrap();
        let moved = [72.0, 50.0];
        let wanted = key(7, moved, zoom, view, 2.0).unwrap();
        assert_ne!(shown, wanted);
        assert!(covers_viewport(shown, wanted, moved, zoom, view), "coverage, not the new overscan rectangle, decides reuse");
        let far = [moved[0] + shown.size[0] as f32 / shown.scale, moved[1]];
        assert!(!covers_viewport(shown, key(7, far, zoom, view, 2.0).unwrap(), far, zoom, view));
        let image = Key { generation: 7, detail: 0, line_zoom: 1.0, origin: [0.0; 2], scale: 2.0, size: [200; 2] };
        assert!(covers_viewport(image, image, [50.0; 2], 1.0, [98.0; 2]));
        assert!(!covers_viewport(image, image, [50.0; 2], 1.0, [99.0; 2]), "equality at the one-pixel filtering guard is conservatively rejected");
        assert!(!covers_viewport(image, image, [50.0; 2], 1.0, [99.25, 98.0]), "fractional viewport points cannot be rounded down to imply coverage");
        assert!(!covers_viewport(image, image, [50.0; 2], 1.0, [98.0, 99.25]));
        let boundary = key(7, [31.0, 26.0], 4.0, [120.0, 80.0], 2.0).unwrap();
        assert_eq!(boundary.origin, [0.0; 2]);
        assert!(
            covers_viewport(boundary, boundary, [31.0, 26.0], 4.0, [120.0, 80.0]),
            "a fresh key with exactly 128 pixels of margin must remain usable"
        );
        let horizontal = key(7, [49.0, 26.0], 4.0, [120.0, 80.0], 2.0).unwrap();
        assert_ne!(boundary.origin[0], horizontal.origin[0]);
        assert_eq!(boundary.origin[1], horizontal.origin[1]);
        assert!(
            covers_viewport(boundary, horizontal, [49.0, 26.0], 4.0, [120.0, 80.0]),
            "an unchanged vertical key can retain its original margin during a covered horizontal pan"
        );
    }

    #[test]
    fn coverage_requires_current_finite_style_and_full_desired_resolution() {
        for zoom in [0.02, 0.3, 0.6, 1.0, 3.02, 8.0, 200.0] {
            for ppp in [0.5, 1.0, 1.5, 2.0, 3.0, 4.0] {
                for view in [[1000.0, 700.0], [3000.0, 1500.0]] {
                    let centre = [-400.0, 600.0];
                    let shown = key(7, centre, zoom, view, ppp).unwrap();
                    assert!(covers_viewport(shown, shown, centre, zoom, view));
                    let mut changed = shown;
                    changed.generation += 1;
                    assert!(!covers_viewport(shown, changed, centre, zoom, view));
                    changed = shown;
                    changed.detail = (changed.detail + 1) % 3;
                    assert!(!covers_viewport(shown, changed, centre, zoom, view));
                    changed = shown;
                    changed.line_zoom = if shown.line_zoom < 100.0 { 200.0 } else { 1.0 };
                    assert!(!covers_viewport(shown, changed, centre, zoom, view));
                    changed = shown;
                    changed.scale *= 1.01;
                    assert!(
                        !covers_viewport(shown, changed, centre, zoom, view),
                        "a lower-resolution old image cannot bypass the new resolution requirement"
                    );
                    assert!(
                        !covers_viewport(changed, shown, centre, zoom, view),
                        "the first implementation deliberately rejects even a different higher-resolution bucket"
                    );
                }
            }
        }
        let view = [3000.0, 1500.0];
        let capped = key(7, [0.0; 2], 4.0, view, 4.0).unwrap();
        assert!(capped.scale < 16.0);
        assert!(covers_viewport(capped, capped, [0.0; 2], 4.0, view), "MAX_SIDE-capped Retina still reuses its full allowed resolution");
        let image = key(7, [50.0; 2], 1.0, [120.0, 80.0], 2.0).unwrap();
        for bad in [f32::NAN, f32::INFINITY, f32::NEG_INFINITY, -1.0, 0.0] {
            assert!(!covers_viewport(image, image, [50.0; 2], bad, [120.0, 80.0]));
            assert!(!covers_viewport(image, image, [50.0; 2], 1.0, [bad, 80.0]));
            let mut changed = image;
            changed.scale = bad;
            assert!(!covers_viewport(changed, image, [50.0; 2], 1.0, [120.0, 80.0]));
            assert!(!covers_viewport(image, changed, [50.0; 2], 1.0, [120.0, 80.0]));
        }
        for bad in [f32::NAN, f32::INFINITY, f32::NEG_INFINITY] {
            assert!(!covers_viewport(image, image, [bad, 50.0], 1.0, [120.0, 80.0]));
            let mut changed = image;
            changed.origin[0] = bad;
            assert!(!covers_viewport(changed, image, [50.0; 2], 1.0, [120.0, 80.0]));
            changed = image;
            changed.line_zoom = bad;
            assert!(!covers_viewport(image, changed, [50.0; 2], 1.0, [120.0, 80.0]));
        }
        for size in [0, MAX_SIDE + 1, u32::MAX] {
            let mut changed = image;
            changed.size[0] = size;
            assert!(!covers_viewport(changed, image, [50.0; 2], 1.0, [120.0, 80.0]));
            assert!(!covers_viewport(image, changed, [50.0; 2], 1.0, [120.0, 80.0]));
        }
    }

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
    fn display_detail_tracks_zoom_buckets_and_returns_to_full_detail() {
        let k = |zoom, ppp| key(1, [800.0, 450.0], zoom, [1000.0, 700.0], ppp).unwrap();
        assert_eq!(k(0.3, 1.0).detail, 2);
        assert_eq!(k(0.6, 1.0).detail, 1);
        assert_eq!(k(1.0, 1.0).detail, 0);
        assert_eq!(k(0.3, 1.0).detail, k(0.3, 2.0).detail, "density follows window scale, independently of Retina pixel ratio");
        assert_eq!(k(0.6001, 1.0), k(0.6002, 1.0), "tiny zoom increments share raster resolution and display detail");
    }
    #[test]
    fn logical_pen_zoom_is_pan_density_and_raster_budget_independent() {
        for zoom in [0.02, 0.3, 0.6, 1.0, 3.5, 200.0] {
            let reference = key(4, [0.0; 2], zoom, [1000.0, 700.0], 1.0).unwrap();
            for ppp in [0.5, 1.0, 1.5, 2.0, 3.0, 4.0] {
                let other = key(4, [900.0, -300.0], zoom, [3000.0, 1500.0], ppp).unwrap();
                assert_eq!(reference.line_zoom, other.line_zoom);
                assert_eq!(reference.detail, other.detail);
                assert!((0.02..=200.0).contains(&other.line_zoom));
            }
        }
        let low = key(1, [0.0; 2], 4.0, [3000.0; 2], 4.0).unwrap();
        let high = key(1, [0.0; 2], 4.01, [3000.0; 2], 4.0).unwrap();
        assert_ne!(low.line_zoom, high.line_zoom, "style must invalidate even when the raster budget caps resolution");
        assert!(!accept_result(low, high), "old zoom pens cannot upload as current pixels");
    }
    #[test]
    fn hostile_raster_inputs_never_produce_nonfinite_style_or_camera_values() {
        for value in [f32::NAN, f32::INFINITY, f32::NEG_INFINITY, -1.0, 0.0] {
            assert!(key(1, [0.0; 2], value, [1000.0; 2], 1.0).is_none());
            assert!(key(1, [0.0; 2], 1.0, [1000.0; 2], value).is_none());
        }
        assert!(exact_key(1, [f32::INFINITY, 0.0], 1.0, [1000.0; 2], 1.0).is_none());
        assert!(exact_key(1, [0.0; 2], f32::MAX, [1000.0; 2], 2.0).is_none());
    }
    #[test]
    fn obsolete_generations_and_superseded_rasters_are_rejected_before_upload() {
        let old = key(1, [800.0, 450.0], 0.65, [1000.0, 700.0], 1.0).unwrap();
        let current = key(2, [800.0, 450.0], 0.65, [1000.0, 700.0], 1.0).unwrap();
        assert!(!accept_result(old, current));
        assert!(accept_result(current, current));
    }
}
