//! The plan view: the engine's presentation sheet for a level (the same SVG `plan.sheet` writes),
//! rasterized by resvg for exactly the region on screen at the screen's resolution, so it stays
//! crisp at any zoom. Rasterizing runs on a thread of its own; while it works, the previous
//! picture is shown moved and scaled to the current view.

use std::sync::Arc;
use std::sync::mpsc::{Receiver, Sender, TrySendError, channel, sync_channel};

use eframe::egui;

use crate::theme;
use trigon_engine::resvg::usvg;
use trigon_engine::{DrawingElementHit, DrawingPreview, DrawingSnap, SnapOptions, snap_drawing};

#[cfg(test)]
use crate::drawing_raster::MAX_SIDE;
pub use crate::drawing_raster::rasterize;
use crate::drawing_raster::{self, Key};

struct Job {
    tree: Arc<usvg::Tree>,
    source: Option<Arc<str>>,
    key: Key,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct StyleKey {
    generation: u64,
    detail: u8,
    zoom_bits: u32,
}

impl StyleKey {
    fn from_raster(key: Key) -> Self {
        Self { generation: key.generation, detail: key.detail, zoom_bits: key.line_zoom.to_bits() }
    }
}

struct StyledTree {
    key: StyleKey,
    source: Arc<str>,
    tree: Arc<usvg::Tree>,
}

/// One parsed adaptive tree per raster worker, independent of pan, raster size and Retina ratio.
#[derive(Default)]
struct StyleCache {
    entry: Option<StyledTree>,
}

impl StyleCache {
    fn tree(&mut self, job: &Job) -> Result<(Arc<usvg::Tree>, bool), String> {
        let Some(source) = &job.source else {
            self.entry = None;
            return Ok((job.tree.clone(), false));
        };
        let key = StyleKey::from_raster(job.key);
        if let Some(entry) = &self.entry
            && entry.key == key
            && Arc::ptr_eq(&entry.source, source)
        {
            return Ok((entry.tree.clone(), true));
        }
        // Release the previous worker entry before constructing another potentially large tree.
        self.entry = None;
        let styled = crate::screen_lines::svg_at_zoom(source, job.key.line_zoom)?;
        let tree = if job.key.detail == 0 && job.key.line_zoom == 1.0 {
            job.tree.clone()
        } else {
            Arc::new(trigon_engine::parse_svg(&styled).map_err(|e| format!("screen line styling: {e}"))?)
        };
        self.entry = Some(StyledTree { key, source: source.clone(), tree: tree.clone() });
        Ok((tree, false))
    }
}

struct PreparedRaster {
    image: egui::ColorImage,
    tree: Arc<usvg::Tree>,
    times: drawing_raster::Times,
    style_ms: f64,
    style_cache_hit: bool,
}

struct RasterDone {
    key: Key,
    base: Arc<usvg::Tree>,
    result: Result<PreparedRaster, String>,
}

/// The currently uploaded tree shares storage with the worker cache; snapshots never parse SVG.
struct ScreenTree {
    base: Arc<usvg::Tree>,
    styled: Arc<usvg::Tree>,
}

/// Navigation belongs to a drawing tab, independently of its current raster generation.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct Navigation {
    centre: [f32; 2],
    zoom: f32,
    fitted: bool,
}

pub struct PlanView {
    tree: Option<(u64, Arc<usvg::Tree>)>,
    display_svgs: Option<[Arc<str>; 3]>,
    geometry: Option<DrawingPreview>,
    /// Sheet pixel at the middle of the view, and screen points per sheet pixel.
    centre: [f32; 2],
    zoom: f32,
    fitted: bool,
    picture: Option<(egui::TextureHandle, Key)>,
    picture_tree: Option<ScreenTree>,
    asked: Option<Key>,
    jobs: Sender<Job>,
    done: Receiver<RasterDone>,
    pub error: Option<String>,
    last_times: Option<drawing_raster::Times>,
    last_style_ms: Option<f64>,
    last_style_cache_hit: Option<bool>,
    uploads: u64,
    discarded: u64,
    enqueue_ms: f64,
}

impl PlanView {
    pub(crate) fn navigation(&self) -> Navigation {
        Navigation { centre: self.centre, zoom: self.zoom, fitted: self.fitted }
    }

    pub(crate) fn restore_navigation(&mut self, navigation: Navigation) {
        self.centre = navigation.centre;
        self.zoom = navigation.zoom;
        self.fitted = navigation.fitted;
    }

    pub fn new(ctx: egui::Context) -> PlanView {
        let (jobs, rx) = channel::<Job>();
        let (tx, done) = sync_channel(1);
        let _ = std::thread::Builder::new().name("trigon-plan-raster".into()).spawn(move || {
            let mut styles = StyleCache::default();
            while let Ok(mut job) = rx.recv() {
                // Only the latest view matters.
                while let Ok(newer) = rx.try_recv() {
                    job = newer;
                }
                let start = std::time::Instant::now();
                let result = styles.tree(&job).and_then(|(tree, style_cache_hit)| {
                    let style_ms = start.elapsed().as_secs_f64() * 1000.0;
                    let (image, times) = drawing_raster::profiled(&tree, job.key.origin, job.key.scale, job.key.size)
                        .ok_or_else(|| "drawing raster allocation or camera failed".to_owned())?;
                    Ok(PreparedRaster { image, tree, times, style_ms, style_cache_hit })
                });
                // A paused UI must not accumulate complete raster images or parsed style trees.
                // If the queued result is stale, the UI clears `asked` and resubmits its latest key.
                match tx.try_send(RasterDone { key: job.key, base: job.tree, result }) {
                    Ok(()) | Err(TrySendError::Full(_)) => {}
                    Err(TrySendError::Disconnected(_)) => break,
                }
                ctx.request_repaint();
            }
        });
        PlanView {
            display_svgs: None,
            tree: None,
            geometry: None,
            centre: [800.0, 450.0],
            zoom: 1.0,
            fitted: false,
            picture: None,
            picture_tree: None,
            asked: None,
            jobs,
            done,
            error: None,
            last_times: None,
            last_style_ms: None,
            last_style_cache_hit: None,
            uploads: 0,
            discarded: 0,
            enqueue_ms: 0.0,
        }
    }

    /// A new sheet (another level, a changed model). The view stays where it is.
    pub fn set_tree(&mut self, generation: u64, tree: Arc<usvg::Tree>) {
        self.tree = Some((generation, tree));
        self.display_svgs = None;
        self.asked = None;
        self.error = None;
    }

    pub fn set_drawing(&mut self, generation: u64, drawing: crate::worker::PreparedDrawing) {
        self.set_tree(generation, drawing.tree);
        self.display_svgs = Some(drawing.display_svgs);
        self.geometry = Some(DrawingPreview {
            svg: String::new(),
            transform: drawing.transform,
            segments: drawing.segments,
            element_segments: drawing.element_segments,
            element_regions: drawing.element_regions,
        });
    }

    pub fn fail(&mut self, error: String) {
        self.tree = None;
        self.display_svgs = None;
        self.picture_tree = None;
        self.geometry = None;
        self.picture = None;
        self.asked = None;
        self.error = Some(error);
    }

    pub fn screen_to_model(&self, rect: egui::Rect, p: egui::Pos2) -> Option<[f64; 2]> {
        let transform = self.geometry.as_ref()?.transform;
        let sheet = [
            f64::from(self.centre[0]) + f64::from(p.x - rect.center().x) / f64::from(self.zoom),
            f64::from(self.centre[1]) + f64::from(p.y - rect.center().y) / f64::from(self.zoom),
        ];
        Some(transform.to_model(sheet))
    }

    pub fn model_to_screen(&self, rect: egui::Rect, p: [f64; 2]) -> Option<egui::Pos2> {
        let transform = self.geometry.as_ref()?.transform;
        let q = transform.to_sheet(p);
        Some(egui::pos2(rect.center().x + (q[0] as f32 - self.centre[0]) * self.zoom, rect.center().y + (q[1] as f32 - self.centre[1]) * self.zoom))
    }

    /// Affine model-mm → window-point mapping for remote point picking. y points up in the
    /// drawing and down in the window; `origin` is where drawing [0,0] lands on screen.
    pub fn screen_mapping(&self, rect: egui::Rect) -> Option<serde_json::Value> {
        let t = self.geometry.as_ref()?.transform;
        let scale = f64::from(self.zoom) / t.mm_per_px;
        let origin = [
            f64::from(rect.center().x) - t.x0 * scale - f64::from(self.centre[0] * self.zoom),
            f64::from(rect.center().y) + t.y1 * scale - f64::from(self.centre[1] * self.zoom),
        ];
        Some(serde_json::json!({"origin":origin,"points_per_mm":scale,"viewport":[rect.left(),rect.top(),rect.width(),rect.height()]}))
    }

    pub fn tolerance_mm(&self) -> Option<f64> {
        self.geometry.as_ref().map(|drawing| (10.0 * drawing.transform.mm_per_px / f64::from(self.zoom)).min(100_000.0))
    }

    pub fn cursor(&self, rect: egui::Rect, p: egui::Pos2, options: Option<SnapOptions>) -> Option<([f64; 2], Option<DrawingSnap>)> {
        let point = self.screen_to_model(rect, p)?;
        let segments = &self.geometry.as_ref()?.segments;
        let snap = options.and_then(|settings| snap_drawing(segments, point, self.tolerance_mm()?, settings));
        Some((snap.map_or(point, |s| s.point), snap))
    }

    pub fn aligned_cursor(
        &self,
        rect: egui::Rect,
        p: egui::Pos2,
        options: Option<SnapOptions>,
        alignment: Option<&trigon_engine::AlignmentOptions>,
    ) -> Option<([f64; 2], Option<trigon_engine::AlignedSnap>)> {
        let point = self.screen_to_model(rect, p)?;
        let segments = &self.geometry.as_ref()?.segments;
        if alignment.is_none() {
            let (point, snap) = self.cursor(rect, p, options)?;
            return Some((point, snap.map(|snap| trigon_engine::AlignedSnap { snap, guides: Vec::new() })));
        }
        let snapped = options.and_then(|settings| trigon_engine::snap_drawing_aligned(segments, point, self.tolerance_mm()?, settings, alignment));
        Some((snapped.as_ref().map_or(point, |s| s.snap.point), snapped))
    }

    /// Pick the actual visible model linework within ten window points of the cursor.
    /// The caller waits for the current generation and raster before exposing interaction.
    pub fn pick_element(&self, rect: egui::Rect, p: egui::Pos2) -> Option<DrawingElementHit> {
        if !rect.contains(p) {
            return None;
        }
        self.geometry.as_ref()?.pick_element(self.screen_to_model(rect, p)?, self.tolerance_mm()?)
    }

    pub fn highlight_element(&self, painter: &egui::Painter, rect: egui::Rect, id: &str) {
        let Some(drawing) = &self.geometry else { return };
        for line in drawing.element_segments.iter().filter(|line| line.id == id) {
            if let (Some(a), Some(b)) = (self.model_to_screen(rect, line.segment[0]), self.model_to_screen(rect, line.segment[1])) {
                painter.line_segment([a, b], egui::Stroke::new(2.0, theme::ACCENT));
            }
        }
        for region in drawing.element_regions.iter().filter(|region| region.id == id) {
            for ring in &region.rings {
                for (a, b) in ring.iter().zip(ring.iter().cycle().skip(1)).take(ring.len()) {
                    if let (Some(a), Some(b)) = (self.model_to_screen(rect, *a), self.model_to_screen(rect, *b)) {
                        painter.line_segment([a, b], egui::Stroke::new(2.0, theme::ACCENT));
                    }
                }
            }
        }
    }

    /// CPU diagnostics for native profiling; enqueue timing does not measure GPU upload time.
    pub fn diagnostics(&self) -> serde_json::Value {
        serde_json::json!({"uploads":self.uploads,"discarded_results":self.discarded,"logical_zoom":self.zoom,"stroke_zoom_bucket":drawing_raster::stroke_zoom(self.zoom),"shown_stroke_zoom_bucket":self.picture.as_ref().map(|(_,k)|k.line_zoom),"last_style_ms":self.last_style_ms,"last_style_cache_hit":self.last_style_cache_hit,"style_cache_capacity":1,"last_allocate_ms":self.last_times.map(|t|t.allocate_ms),"last_render_ms":self.last_times.map(|t|t.render_ms),"last_convert_ms":self.last_times.map(|t|t.convert_ms),"last_texture_enqueue_ms":self.enqueue_ms,"gpu_upload_measured":false})
    }

    pub fn fit(&mut self) {
        self.fitted = false;
    }

    fn fit_to(&mut self, view: egui::Vec2) {
        if let Some((_, t)) = &self.tree {
            let s = t.size();
            self.centre = [s.width() / 2.0, s.height() / 2.0];
            self.zoom = (view.x / s.width()).min(view.y / s.height()).max(0.01);
            self.fitted = true;
        }
    }

    /// The sheet region on screen now, as a raster job's key.
    fn key(&self, rect: egui::Rect, ppp: f32) -> Option<Key> {
        let (generation, _) = self.tree.as_ref()?;
        drawing_raster::key(*generation, self.centre, self.zoom, [rect.width(), rect.height()], ppp)
    }

    fn accepts_raster(&self, key: Key, base: &Arc<usvg::Tree>, rect: egui::Rect, ppp: f32) -> bool {
        self.key(rect, ppp).is_some_and(|wanted| drawing_raster::accept_result(key, wanted))
            && self.tree.as_ref().is_some_and(|(_, current)| Arc::ptr_eq(current, base))
    }

    /// Capture readiness is stricter than picking readiness: current source, LOD and pen bucket.
    pub fn snapshot_ready(&self, rect: egui::Rect, ppp: f32) -> bool {
        let Some((generation, base)) = &self.tree else { return false };
        let Some(key) = drawing_raster::exact_key(*generation, self.centre, self.zoom, [rect.width(), rect.height()], ppp) else { return false };
        self.picture_tree
            .as_ref()
            .zip(self.picture.as_ref())
            .is_some_and(|(tree, (_, shown))| Arc::ptr_eq(base, &tree.base) && StyleKey::from_raster(*shown) == StyleKey::from_raster(key))
    }

    /// A PNG-ready picture of the current style and exact viewport. A pending style returns None;
    /// SVG adaptation and parsing happen exclusively on the raster worker.
    pub fn snapshot(&self, rect: egui::Rect, ppp: f32) -> Option<egui::ColorImage> {
        if !self.snapshot_ready(rect, ppp) {
            return None;
        }
        let generation = self.tree.as_ref()?.0;
        let key = drawing_raster::exact_key(generation, self.centre, self.zoom, [rect.width(), rect.height()], ppp)?;
        rasterize(&self.picture_tree.as_ref()?.styled, key.origin, key.scale, key.size)
    }

    /// Parsing the SVG finishes before the on-screen raster job. Interaction waits for current-generation pixels; pan/zoom reuses them with exact vector
    /// picking. Snapshots rasterize the precise viewport once its current style is prepared.
    pub fn ready(&self, rect: egui::Rect, ppp: f32) -> bool {
        self.key(rect, ppp).is_some_and(|wanted| {
            self.picture.as_ref().is_some_and(|(_, shown)| drawing_raster::ready(*shown, wanted))
                && self.tree.as_ref().zip(self.picture_tree.as_ref()).is_some_and(|((_, base), shown)| Arc::ptr_eq(base, &shown.base))
        })
    }

    pub fn ui(&mut self, ui: &mut egui::Ui, picking: bool) -> egui::Response {
        let (rect, resp) = ui.allocate_exact_size(ui.available_size(), egui::Sense::click_and_drag());
        let painter = ui.painter_at(rect);
        painter.rect_filled(rect, 0.0, theme::PAPER);
        if self.tree.is_none() {
            let text = self.error.as_deref().unwrap_or("Drawing the workspace…");
            painter.text(rect.center(), egui::Align2::CENTER_CENTER, text, egui::FontId::proportional(15.0), egui::Color32::from_gray(90));
            return resp;
        }
        if !self.fitted {
            self.fit_to(rect.size());
        }
        // Pan with any drag, zoom with the wheel or a pinch around the pointer.
        if resp.dragged_by(egui::PointerButton::Middle) || resp.dragged_by(egui::PointerButton::Secondary) || (resp.dragged() && !picking) {
            let d = resp.drag_delta();
            self.centre[0] -= d.x / self.zoom;
            self.centre[1] -= d.y / self.zoom;
        }
        if resp.double_clicked() && !picking {
            self.fit_to(rect.size());
        }
        if resp.hovered() && !(picking && ui.input(|i| i.pointer.primary_down())) {
            let (scroll, pinch) = ui.input(|i| (i.smooth_scroll_delta.y, i.zoom_delta()));
            let factor = pinch * (scroll / 300.0).exp();
            if (factor - 1.0).abs() > 1e-4
                && let Some(p) = resp.hover_pos()
            {
                let before = [self.centre[0] + (p.x - rect.center().x) / self.zoom, self.centre[1] + (p.y - rect.center().y) / self.zoom];
                self.zoom = (self.zoom * factor).clamp(0.02, 200.0);
                self.centre = [before[0] - (p.x - rect.center().x) / self.zoom, before[1] - (p.y - rect.center().y) / self.zoom];
            }
        }
        let ppp = ui.ctx().pixels_per_point();
        let mut latest = None;
        while let Ok(result) = self.done.try_recv() {
            if latest.replace(result).is_some() {
                self.discarded += 1;
            }
        }
        if let Some(RasterDone { key, base, result }) = latest {
            if self.accepts_raster(key, &base, rect, ppp) {
                match result {
                    Ok(PreparedRaster { image, tree, times, style_ms, style_cache_hit }) => {
                        let start = std::time::Instant::now();
                        let options = egui::TextureOptions::LINEAR;
                        match &mut self.picture {
                            Some((tex, k)) => {
                                tex.set(image, options);
                                *k = key;
                            }
                            None => self.picture = Some((ui.ctx().load_texture("trigon-plan", image, options), key)),
                        }
                        self.picture_tree = Some(ScreenTree { base, styled: tree });
                        self.enqueue_ms = start.elapsed().as_secs_f64() * 1000.0;
                        self.last_times = Some(times);
                        self.last_style_ms = Some(style_ms);
                        self.last_style_cache_hit = Some(style_cache_hit);
                        self.uploads += 1;
                        self.asked = None;
                        self.error = None;
                    }
                    Err(error) => {
                        self.asked = Some(key);
                        self.error = Some(error);
                    }
                }
            } else {
                self.discarded += 1;
                // The bounded completion slot may have discarded a more recent result.
                self.asked = None;
            }
        }
        if let Some(key) = self.key(rect, ppp) {
            let shown = self.picture.as_ref().map(|(_, k)| *k);
            let current_source = self.tree.as_ref().zip(self.picture_tree.as_ref()).is_some_and(|((_, base), shown)| Arc::ptr_eq(base, &shown.base));
            if (shown != Some(key) || !current_source)
                && self.asked != Some(key)
                && let Some((_, tree)) = &self.tree
            {
                let source = self.display_svgs.as_ref().and_then(|sources| sources.get(usize::from(key.detail))).cloned();
                if self.jobs.send(Job { tree: tree.clone(), source, key }).is_err() {
                    self.error = Some("drawing raster worker is unavailable".into());
                }
                self.asked = Some(key);
            }
        }
        if let Some((tex, k)) = &self.picture {
            // Where the picture's sheet region lies in the current view.
            let to_screen = |sx: f32, sy: f32| {
                egui::pos2(rect.center().x + (sx - self.centre[0]) * self.zoom, rect.center().y + (sy - self.centre[1]) * self.zoom)
            };
            let w = k.size[0] as f32 / k.scale;
            let h = k.size[1] as f32 / k.scale;
            let r = egui::Rect::from_min_max(to_screen(k.origin[0], k.origin[1]), to_screen(k.origin[0] + w, k.origin[1] + h));
            painter.image(tex.id(), r, egui::Rect::from_min_max(egui::pos2(0.0, 0.0), egui::pos2(1.0, 1.0)), theme::PAPER);
        }
        if let Some(error) = &self.error {
            painter.text(rect.left_top() + egui::vec2(12.0, 12.0), egui::Align2::LEFT_TOP, error, egui::FontId::proportional(13.0), theme::ACCENT);
        }
        resp
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const SCREEN_LINE: &str = r#"<svg xmlns="http://www.w3.org/2000/svg" width="100" height="100"><path d="M10 50L90 50" fill="none" stroke="black" stroke-width="2.362"/></svg>"#;

    fn screen_drawing() -> crate::worker::PreparedDrawing {
        let source: Arc<str> = Arc::from(SCREEN_LINE);
        crate::worker::PreparedDrawing {
            tree: Arc::new(trigon_engine::parse_svg(SCREEN_LINE).unwrap()),
            display_svgs: [source.clone(), source.clone(), source],
            transform: trigon_engine::DrawingTransform { x0: 0.0, y1: 1000.0, mm_per_px: 10.0 },
            segments: vec![[[100.0, 500.0], [900.0, 500.0]]],
            element_segments: vec![trigon_engine::DrawingElementSegment {
                id: "wall.L0".into(),
                kind: trigon_engine::DrawingElementKind::InteriorWall,
                segment: [[100.0, 500.0], [900.0, 500.0]],
            }],
            element_regions: vec![],
        }
    }

    #[test]
    fn style_cache_reuses_pan_retina_and_bucket_ticks_but_invalidates_every_style_input() {
        let source: Arc<str> = Arc::from(SCREEN_LINE);
        let tree = Arc::new(trigon_engine::parse_svg(SCREEN_LINE).unwrap());
        let make = |generation, zoom, ppp, centre| Job {
            tree: tree.clone(),
            source: Some(source.clone()),
            key: drawing_raster::key(generation, centre, zoom, [3000.0, 1500.0], ppp).unwrap(),
        };
        let mut cache = StyleCache::default();
        let (first, hit) = cache.tree(&make(1, 4.01, 1.0, [50.0; 2])).unwrap();
        assert!(!hit);
        for ppp in [0.5, 1.0, 1.5, 2.0, 3.0, 4.0] {
            let (same, hit) = cache.tree(&make(1, 4.02, ppp, [4000.0, -2000.0])).unwrap();
            assert!(hit, "pan, Retina and sub-bucket ticks must not reparse");
            assert!(Arc::ptr_eq(&first, &same));
        }
        for (generation, zoom) in [(1, 8.0), (2, 8.0), (1, 4.01)] {
            let (next, hit) = cache.tree(&make(generation, zoom, 1.0, [50.0; 2])).unwrap();
            assert!(!hit, "the one-entry cache must invalidate generation/zoom and evict old zooms");
            assert!(!Arc::ptr_eq(&first, &next));
        }
        let mut changed = make(1, 4.01, 1.0, [50.0; 2]);
        changed.key.detail = 1;
        assert!(!cache.tree(&changed).unwrap().1, "LOD is part of the style key");
        changed.source = Some(Arc::from(SCREEN_LINE));
        assert!(!cache.tree(&changed).unwrap().1, "a newly prepared source invalidates even equal generation/zoom");
        changed.source = Some(Arc::from("<svg>"));
        assert!(cache.tree(&changed).is_err());
        assert!(cache.entry.is_none(), "failed adaptation must not retain a misleading cached style");
        changed.source = Some(source);
        changed.key.line_zoom = f32::NAN;
        assert!(cache.tree(&changed).is_err());
        assert!(cache.entry.is_none());
    }

    #[test]
    fn worker_styled_pixels_and_snapshot_match_without_moving_second_frame_picking() {
        let ctx = egui::Context::default();
        let mut view = PlanView::new(ctx.clone());
        let rect = egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(120.0, 80.0));
        view.set_drawing(7, screen_drawing());
        view.centre = [50.0; 2];
        view.fitted = true;
        let before = view.geometry.clone().unwrap();
        for zoom in [0.02, 0.3, 0.6, 1.0, 3.5, 8.0, 200.0] {
            view.zoom = zoom;
            for ppp in [0.5, 1.0, 1.5, 2.0, 3.0, 4.0] {
                let key = view.key(rect, ppp).unwrap();
                let base = view.tree.as_ref().unwrap().1.clone();
                let source = view.display_svgs.as_ref().unwrap()[usize::from(key.detail)].clone();
                view.jobs.send(Job { tree: base.clone(), source: Some(source), key }).unwrap();
                let RasterDone { key: completed, base, result } = view.done.recv_timeout(std::time::Duration::from_secs(3)).unwrap();
                assert!(view.accepts_raster(completed, &base, rect, ppp));
                let PreparedRaster { image, tree, .. } = result.unwrap();
                view.picture_tree = Some(ScreenTree { base, styled: tree.clone() });
                view.picture = Some((ctx.load_texture("styled-plan", image, egui::TextureOptions::LINEAR), completed));
                let snapshot = view.snapshot(rect, ppp).unwrap();
                let exact = drawing_raster::exact_key(7, view.centre, zoom, [rect.width(), rect.height()], ppp).unwrap();
                let source = &view.display_svgs.as_ref().unwrap()[usize::from(key.detail)];
                let adapted = crate::screen_lines::svg_at_zoom(source, key.line_zoom).unwrap();
                let expected_tree = trigon_engine::parse_svg(&adapted).unwrap();
                let expected = rasterize(&expected_tree, exact.origin, exact.scale, exact.size).unwrap();
                assert_eq!(snapshot.pixels, expected.pixels, "snapshots must use the actual worker-prepared adaptive style");
                if zoom == 8.0 {
                    let ink = (0..snapshot.size[1])
                        .map(|row| 1.0 - f32::from(snapshot.pixels[row * snapshot.size[0] + snapshot.size[0] / 2].r()) / 255.0)
                        .sum::<f32>()
                        / ppp;
                    assert!(ink < 3.3, "the pipeline must not rasterize an 18.9-point paper pen at 8x: {ink}");
                }
                let point = view.model_to_screen(rect, [500.0, 500.0]).unwrap();
                let hit = view.pick_element(rect, point).unwrap();
                assert_eq!(hit.id, "wall.L0");
                assert_eq!(hit.point, [500.0, 500.0]);
                assert_eq!(view.screen_to_model(rect, point).unwrap(), [500.0, 500.0]);
                assert!(view.ready(rect, ppp));
                let after = view.geometry.as_ref().unwrap();
                assert_eq!(before.transform, after.transform);
                assert_eq!(before.segments, after.segments);
                assert_eq!(before.element_segments, after.element_segments);
                let mut output = ctx.run_ui(egui::RawInput::default(), |_| {});
                output.textures_delta.clear();
            }
        }
    }

    #[test]
    fn bounded_completion_slot_retries_the_latest_view_when_its_result_was_dropped() {
        let ctx = egui::Context::default();
        let mut view = PlanView::new(ctx.clone());
        view.set_drawing(7, screen_drawing());
        view.centre = [50.0; 2];
        view.fitted = true;
        let (jobs, received) = channel();
        view.jobs = jobs;
        let (completed, done) = sync_channel(1);
        view.done = done;
        let input = egui::RawInput { screen_rect: Some(egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(200.0, 120.0))), ..Default::default() };
        let mut output = ctx.run_ui(input, |ui| {
            let rect = ui.available_rect_before_wrap();
            view.zoom = 4.0;
            let old = view.key(rect, ui.ctx().pixels_per_point()).unwrap();
            view.zoom = 8.0;
            let wanted = view.key(rect, ui.ctx().pixels_per_point()).unwrap();
            view.asked = Some(wanted);
            let base = view.tree.as_ref().unwrap().1.clone();
            assert!(completed.try_send(RasterDone { key: old, base: base.clone(), result: Err("obsolete error".into()) }).is_ok());
            assert!(matches!(
                completed.try_send(RasterDone { key: wanted, base, result: Err("dropped newer result".into()) }),
                Err(TrySendError::Full(_))
            ));
            view.ui(ui, false);
            assert_eq!(received.try_recv().unwrap().key, wanted, "rejecting the stale occupied slot must retry the dropped latest key");
            assert!(view.error.is_none(), "an obsolete error must not replace the current drawing");
            assert_eq!(view.discarded, 1);
        });
        output.textures_delta.clear();
    }

    #[test]
    fn accepted_wheel_reversal_clears_a_dropped_later_zoom_request() {
        let ctx = egui::Context::default();
        let mut view = PlanView::new(ctx.clone());
        view.set_drawing(7, screen_drawing());
        view.centre = [50.0; 2];
        view.fitted = true;
        let (jobs, received) = channel();
        view.jobs = jobs;
        let (completed, done) = sync_channel(1);
        view.done = done;
        let input = |time| egui::RawInput {
            screen_rect: Some(egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(200.0, 120.0))),
            time: Some(time),
            ..Default::default()
        };
        let mut output = ctx.run_ui(input(0.0), |ui| {
            let rect = ui.available_rect_before_wrap();
            let ppp = ui.ctx().pixels_per_point();
            view.zoom = 4.0;
            let a = view.key(rect, ppp).unwrap();
            view.zoom = 8.0;
            let b = view.key(rect, ppp).unwrap();
            view.asked = Some(b);
            let base = view.tree.as_ref().unwrap().1.clone();
            let prepared = PreparedRaster {
                image: rasterize(&base, [0.0; 2], 1.0, [1, 1]).unwrap(),
                tree: base.clone(),
                times: drawing_raster::Times::default(),
                style_ms: 0.0,
                style_cache_hit: false,
            };
            assert!(completed.try_send(RasterDone { key: a, base: base.clone(), result: Ok(prepared) }).is_ok());
            assert!(matches!(
                completed.try_send(RasterDone { key: b, base, result: Err("B cannot occupy A's full completion slot".into()) }),
                Err(TrySendError::Full(_))
            ));
            view.zoom = 4.0;
            view.ui(ui, false);
            assert!(view.ready(rect, ppp), "A is the current visible result after the wheel reversal");
            assert!(received.try_recv().is_err(), "accepted A needs no duplicate raster");
        });
        output.textures_delta.clear();
        view.zoom = 8.0;
        let mut output = ctx.run_ui(input(0.1), |ui| {
            let rect = ui.available_rect_before_wrap();
            let wanted = view.key(rect, ui.ctx().pixels_per_point()).unwrap();
            view.ui(ui, false);
            assert_eq!(
                received.try_recv().ok().map(|job| job.key),
                Some(wanted),
                "accepting A must clear the dropped B marker so a later return to B cannot wait forever"
            );
        });
        output.textures_delta.clear();
    }

    #[test]
    fn pending_zoom_and_replaced_sources_cannot_snapshot_or_accept_stale_styles() {
        let ctx = egui::Context::default();
        let mut view = PlanView::new(ctx.clone());
        let rect = egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(120.0, 80.0));
        view.set_drawing(7, screen_drawing());
        view.centre = [50.0; 2];
        view.zoom = 4.0;
        view.fitted = true;
        let key = view.key(rect, 2.0).unwrap();
        let base = view.tree.as_ref().unwrap().1.clone();
        assert!(!view.snapshot_ready(rect, 2.0));
        assert!(view.snapshot(rect, 2.0).is_none());
        view.jobs.send(Job { tree: base.clone(), source: Some(view.display_svgs.as_ref().unwrap()[0].clone()), key }).unwrap();
        let RasterDone { base: completed_base, result, .. } = view.done.recv_timeout(std::time::Duration::from_secs(3)).unwrap();
        let PreparedRaster { image, tree, .. } = result.unwrap();
        view.picture_tree = Some(ScreenTree { base: completed_base, styled: tree });
        view.picture = Some((ctx.load_texture("old-style", image, egui::TextureOptions::LINEAR), key));
        assert!(view.snapshot_ready(rect, 2.0));
        assert!(view.snapshot(rect, 2.0).is_some());
        view.zoom = 8.0;
        assert!(!view.snapshot_ready(rect, 2.0));
        assert!(view.snapshot(rect, 2.0).is_none());
        assert!(!view.accepts_raster(key, &base, rect, 2.0));
        assert!(view.ready(rect, 2.0), "exact current-generation picking stays live during style preparation");
        view.zoom = 4.0;
        assert!(view.accepts_raster(key, &base, rect, 2.0), "rapid wheel reversal can reuse the original style");
        view.set_drawing(7, screen_drawing());
        assert!(!view.accepts_raster(key, &base, rect, 2.0), "same generation cannot alias a different prepared drawing");
        assert!(!view.ready(rect, 2.0));
        assert!(view.snapshot(rect, 2.0).is_none());
    }

    #[test]
    fn pointer_drag_pans_the_live_canvas_without_changing_its_zoom() {
        let ctx = egui::Context::default();
        let mut view = PlanView::new(ctx.clone());
        let svg = r#"<svg xmlns="http://www.w3.org/2000/svg" width="1600" height="900"><path d="M10 10L1500 800" stroke="black"/></svg>"#;
        view.set_tree(1, Arc::new(trigon_engine::parse_svg(svg).unwrap()));
        let mut frame = 0;
        let mut draw = |view: &mut PlanView, events| {
            frame += 1;
            let input = egui::RawInput {
                screen_rect: Some(egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(1200.0, 800.0))),
                time: Some(f64::from(frame) * 0.05),
                events,
                ..Default::default()
            };
            let mut output = ctx.run_ui(input, |ui| {
                view.ui(ui, false);
            });
            output.textures_delta.clear();
        };
        draw(&mut view, Vec::new());
        let before = view.centre;
        let zoom = view.zoom;
        let events = crate::control::parse_input("ui.drag", &serde_json::json!({"from":[600,400],"to":[640,420],"steps":8})).unwrap();
        for event in events {
            draw(&mut view, vec![event]);
        }
        assert!(view.centre[0] < before[0] - 20.0 / zoom, "a real drag must move the drawing centre");
        assert_eq!(view.zoom, zoom);
    }

    #[test]
    fn alignment_remains_in_drawing_mm_after_pan_and_zoom_and_alt_bypasses_it() {
        let mut view = PlanView::new(egui::Context::default());
        view.geometry = Some(DrawingPreview {
            svg: String::new(),
            transform: trigon_engine::DrawingTransform { x0: -24000.0, y1: 18000.0, mm_per_px: 40.0 },
            segments: vec![],
            element_segments: vec![],
            element_regions: vec![],
        });
        let rect = egui::Rect::from_min_size(egui::pos2(200.0, 80.0), egui::vec2(1000.0, 700.0));
        let settings = trigon_engine::AlignmentOptions { anchors: vec![[1000.0, 3000.0], [4000.0, 2000.0]], offset_mm: 500.0 };
        for zoom in [0.3, 1.0, 3.5] {
            view.zoom = zoom;
            view.centre = [200.0 * zoom, 300.0 / zoom];
            let screen = view.model_to_screen(rect, [1503.0, 2504.0]).unwrap();
            let (point, snap) = view.aligned_cursor(rect, screen, Some(SnapOptions::default()), Some(&settings)).unwrap();
            assert_eq!(point, [1500.0, 2500.0]);
            assert_eq!(snap.unwrap().guides.len(), 2);
            let (point, snap) = view.aligned_cursor(rect, screen, None, Some(&settings)).unwrap();
            assert!((point[0] - 1503.0).abs() < 0.05 && (point[1] - 2504.0).abs() < 0.05);
            assert!(snap.is_none());
        }
    }

    #[test]
    fn room_region_interiors_are_pickable_through_the_live_drawing_transform() {
        let mut view = PlanView::new(egui::Context::default());
        view.geometry = Some(DrawingPreview {
            svg: String::new(),
            transform: trigon_engine::DrawingTransform { x0: 0.0, y1: 5000.0, mm_per_px: 10.0 },
            segments: vec![],
            element_segments: vec![],
            element_regions: vec![trigon_engine::DrawingElementRegion {
                id: "living.space.L0".into(),
                kind: trigon_engine::DrawingElementKind::Room,
                rings: vec![vec![[1000.0, 1000.0], [4000.0, 1000.0], [4000.0, 4000.0], [1000.0, 4000.0]]],
                depth_plane: None,
            }],
        });
        view.centre = [250.0, 250.0];
        view.zoom = 1.0;
        let rect = egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(800.0, 600.0));
        let point = view.model_to_screen(rect, [2200.0, 2300.0]).unwrap();
        assert_eq!(view.pick_element(rect, point).unwrap().id, "living.space.L0");
    }

    #[test]
    fn picking_inverts_the_real_drawing_transform_after_pan_and_zoom() {
        let mut view = PlanView::new(egui::Context::default());
        view.geometry = Some(DrawingPreview {
            svg: String::new(),
            transform: trigon_engine::DrawingTransform { x0: -24000.0, y1: 18000.0, mm_per_px: 40.0 },
            segments: vec![],
            element_segments: vec![],
            element_regions: vec![],
        });
        view.centre = [860.0, 380.0];
        view.zoom = 3.5;
        let rect = egui::Rect::from_min_size(egui::pos2(240.0, 80.0), egui::vec2(1000.0, 700.0));
        let p = [11234.5, 3000.25];
        let screen = view.model_to_screen(rect, p).unwrap();
        let actual = view.screen_to_model(rect, screen).unwrap();
        let mapping = view.screen_mapping(rect).unwrap();
        let scale = mapping["points_per_mm"].as_f64().unwrap();
        let origin = [mapping["origin"][0].as_f64().unwrap(), mapping["origin"][1].as_f64().unwrap()];
        assert!((origin[0] + p[0] * scale - f64::from(screen.x)).abs() < 0.001);
        assert!((origin[1] - p[1] * scale - f64::from(screen.y)).abs() < 0.001);
        assert!((p[0] - actual[0]).abs() < 0.01 && (p[1] - actual[1]).abs() < 0.01);
        let (point, snap) = view.cursor(rect, screen, None).unwrap();
        assert_eq!(actual, point);
        assert!(snap.is_none());
        let (_, snap) = view.cursor(rect, view.model_to_screen(rect, [11001.0, 3001.0]).unwrap(), Some(SnapOptions::default())).unwrap();
        assert_eq!(snap.unwrap().point, [11000.0, 3000.0]);
        view.fail("invalid drawing".into());
        assert!(view.screen_to_model(rect, screen).is_none(), "a failed view must not pick stale geometry");
    }

    #[test]
    fn semantic_picking_keeps_a_screen_sized_tolerance_after_pan_and_zoom() {
        use trigon_engine::{DrawingElementKind, DrawingElementSegment, DrawingTransform};
        let mut view = PlanView::new(egui::Context::default());
        view.geometry = Some(DrawingPreview {
            svg: String::new(),
            transform: DrawingTransform { x0: 0.0, y1: 1000.0, mm_per_px: 10.0 },
            segments: vec![],
            element_segments: vec![DrawingElementSegment {
                id: "b1.f1.L0.B0".into(),
                kind: DrawingElementKind::Opening,
                segment: [[200.0, 200.0], [700.0, 200.0]],
            }],
            element_regions: vec![],
        });
        view.centre = [45.0, 55.0];
        view.zoom = 4.0;
        let rect = egui::Rect::from_min_size(egui::pos2(240.0, 80.0), egui::vec2(1000.0, 700.0));
        let near = view.model_to_screen(rect, [400.0, 224.0]).unwrap();
        let hit = view.pick_element(rect, near).unwrap();
        assert_eq!(hit.id, "b1.f1.L0.B0");
        assert!((hit.point[1] - 200.0).abs() < 1e-8);
        let far = view.model_to_screen(rect, [400.0, 226.0]).unwrap();
        assert!(view.pick_element(rect, far).is_none());
        assert!(view.pick_element(rect, rect.min - egui::vec2(1.0, 1.0)).is_none());
        view.zoom = 8.0;
        let zoomed = view.model_to_screen(rect, [400.0, 224.0]).unwrap();
        assert!(view.pick_element(rect, zoomed).is_none(), "zooming in narrows the model-space tolerance");
        let closer = view.model_to_screen(rect, [400.0, 212.0]).unwrap();
        assert!(view.pick_element(rect, closer).is_some());
        view.fail("view changed".into());
        assert!(view.pick_element(rect, closer).is_none(), "a failed drawing cannot expose old element IDs");
    }

    #[test]
    fn a_parsed_plan_is_not_ready_until_its_raster_has_arrived() {
        let ctx = egui::Context::default();
        let mut view = PlanView::new(ctx.clone());
        let tree = Arc::new(
            trigon_engine::parse_svg(
                r#"<svg xmlns="http://www.w3.org/2000/svg" width="100" height="50"><rect width="50" height="50" fill="black"/></svg>"#,
            )
            .unwrap(),
        );
        let rect = egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(200.0, 100.0));
        view.set_tree(1, tree.clone());
        view.fit_to(rect.size());
        assert!(!view.ready(rect, 1.0));
        let key = view.key(rect, 1.0).unwrap();
        view.jobs.send(Job { tree: tree.clone(), source: None, key }).unwrap();
        let RasterDone { key, base, result } = view.done.recv_timeout(std::time::Duration::from_secs(3)).unwrap();
        let PreparedRaster { image, tree: styled, .. } = result.unwrap();
        assert!(image.pixels.iter().any(|p| p.r() < 10));
        view.picture_tree = Some(ScreenTree { base, styled });
        view.picture = Some((ctx.load_texture("ready-plan", image, egui::TextureOptions::LINEAR), key));
        assert!(view.ready(rect, 1.0));
        view.set_tree(2, tree);
        assert!(!view.ready(rect, 1.0), "an older texture cannot stand in for a changed plan");
    }

    #[test]
    fn a_region_of_the_sheet_rasterizes_at_the_asked_scale() {
        let svg = r#"<svg xmlns="http://www.w3.org/2000/svg" width="100" height="50"><rect x="50" y="0" width="50" height="50" fill="black"/></svg>"#;
        let tree = trigon_engine::parse_svg(svg).unwrap();
        // The right half at 2×: black everywhere.
        let img = rasterize(&tree, [50.0, 0.0], 2.0, [100, 100]).unwrap();
        assert_eq!(img.size, [100, 100]);
        assert!(img.pixels.iter().all(|p| p.r() < 10));
        // The left half: white.
        let img = rasterize(&tree, [0.0, 0.0], 1.0, [50, 50]).unwrap();
        assert!(img.pixels.iter().all(|p| p.r() > 245));
        assert!(rasterize(&tree, [f32::NAN, 0.0], 1.0, [10, 10]).is_none());
        assert!(rasterize(&tree, [0.0, 0.0], 0.0, [10, 10]).is_none());
        assert_eq!(rasterize(&tree, [0.0, 0.0], 1.0, [u32::MAX, 0]).unwrap().size, [MAX_SIDE as usize, 1]);
    }
}
