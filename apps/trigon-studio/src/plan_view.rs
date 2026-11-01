//! The plan view: the engine's presentation sheet for a level (the same SVG `plan.sheet` writes),
//! rasterized by resvg for exactly the region on screen at the screen's resolution, so it stays
//! crisp at any zoom. Rasterizing runs on a thread of its own; while it works, the previous
//! picture is shown moved and scaled to the current view.

use std::sync::Arc;
use std::sync::mpsc::{Receiver, Sender, channel};

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
    key: Key,
}

pub struct PlanView {
    tree: Option<(u64, Arc<usvg::Tree>)>,
    geometry: Option<DrawingPreview>,
    /// Sheet pixel at the middle of the view, and screen points per sheet pixel.
    centre: [f32; 2],
    zoom: f32,
    fitted: bool,
    picture: Option<(egui::TextureHandle, Key)>,
    asked: Option<Key>,
    jobs: Sender<Job>,
    done: Receiver<(Key, egui::ColorImage, drawing_raster::Times)>,
    pub error: Option<String>,
    last_times: Option<drawing_raster::Times>,
    uploads: u64,
    discarded: u64,
    enqueue_ms: f64,
}

impl PlanView {
    pub fn new(ctx: egui::Context) -> PlanView {
        let (jobs, rx) = channel::<Job>();
        let (tx, done) = channel();
        let _ = std::thread::Builder::new().name("trigon-plan-raster".into()).spawn(move || {
            while let Ok(mut job) = rx.recv() {
                // Only the latest view matters.
                while let Ok(newer) = rx.try_recv() {
                    job = newer;
                }
                if let Some((img, times)) = drawing_raster::profiled(&job.tree, job.key.origin, job.key.scale, job.key.size) {
                    if tx.send((job.key, img, times)).is_err() {
                        break;
                    }
                    ctx.request_repaint();
                }
            }
        });
        PlanView {
            tree: None,
            geometry: None,
            centre: [800.0, 450.0],
            zoom: 1.0,
            fitted: false,
            picture: None,
            asked: None,
            jobs,
            done,
            error: None,
            last_times: None,
            uploads: 0,
            discarded: 0,
            enqueue_ms: 0.0,
        }
    }

    /// A new sheet (another level, a changed model). The view stays where it is.
    pub fn set_tree(&mut self, generation: u64, tree: Arc<usvg::Tree>) {
        self.tree = Some((generation, tree));
        self.error = None;
    }

    pub fn set_drawing(&mut self, generation: u64, drawing: crate::worker::PreparedDrawing) {
        self.set_tree(generation, drawing.tree);
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
        serde_json::json!({"uploads":self.uploads,"discarded_results":self.discarded,"last_allocate_ms":self.last_times.map(|t|t.allocate_ms),"last_render_ms":self.last_times.map(|t|t.render_ms),"last_convert_ms":self.last_times.map(|t|t.convert_ms),"last_texture_enqueue_ms":self.enqueue_ms,"gpu_upload_measured":false})
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

    /// A PNG-ready picture of what the view shows, made now.
    pub fn snapshot(&self, rect: egui::Rect, ppp: f32) -> Option<egui::ColorImage> {
        let generation = self.tree.as_ref()?.0;
        let key = drawing_raster::exact_key(generation, self.centre, self.zoom, [rect.width(), rect.height()], ppp)?;
        let (_, tree) = self.tree.as_ref()?;
        rasterize(tree, key.origin, key.scale, key.size)
    }

    /// Parsing the SVG finishes before the on-screen raster job. Interaction waits for current-generation pixels; pan/zoom reuses them with exact vector
    /// picking. Snapshots independently rasterize the precise viewport at screen resolution.
    pub fn ready(&self, rect: egui::Rect, ppp: f32) -> bool {
        self.key(rect, ppp).is_some_and(|wanted| self.picture.as_ref().is_some_and(|(_, shown)| drawing_raster::ready(*shown, wanted)))
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
        if let Some((key, img, times)) = latest {
            if self.key(rect, ppp).is_some_and(|wanted| drawing_raster::accept_result(key, wanted)) {
                let start = std::time::Instant::now();
                let options = egui::TextureOptions::LINEAR;
                match &mut self.picture {
                    Some((tex, k)) => {
                        tex.set(img, options);
                        *k = key;
                    }
                    None => self.picture = Some((ui.ctx().load_texture("trigon-plan", img, options), key)),
                }
                self.enqueue_ms = start.elapsed().as_secs_f64() * 1000.0;
                self.last_times = Some(times);
                self.uploads += 1;
            } else {
                self.discarded += 1;
            }
        }
        if let Some(key) = self.key(rect, ppp) {
            let shown = self.picture.as_ref().map(|(_, k)| *k);
            if shown != Some(key)
                && self.asked != Some(key)
                && let Some((_, tree)) = &self.tree
            {
                let _ = self.jobs.send(Job { tree: tree.clone(), key });
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
        resp
    }
}

#[cfg(test)]
mod tests {
    use super::*;

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
        view.jobs.send(Job { tree: tree.clone(), key }).unwrap();
        let (key, img, _) = view.done.recv_timeout(std::time::Duration::from_secs(3)).unwrap();
        assert!(img.pixels.iter().any(|p| p.r() < 10));
        view.picture = Some((ctx.load_texture("ready-plan", img, egui::TextureOptions::LINEAR), key));
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
