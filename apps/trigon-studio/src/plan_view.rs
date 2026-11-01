//! The plan view: the engine's presentation sheet for a level (the same SVG `plan.sheet` writes),
//! rasterized by resvg for exactly the region on screen at the screen's resolution, so it stays
//! crisp at any zoom. Rasterizing runs on a thread of its own; while it works, the previous
//! picture is shown moved and scaled to the current view.

use std::sync::Arc;
use std::sync::mpsc::{Receiver, Sender, channel};

use eframe::egui;

use crate::theme;
use trigon_engine::resvg::{self, tiny_skia, usvg};
use trigon_engine::{DrawingSnap, DrawingTransform, SnapOptions, snap_drawing};

/// The part of the sheet a picture shows: its top-left in sheet pixels and its scale.
#[derive(Clone, Copy, Debug, PartialEq)]
struct Key {
    generation: u64,
    origin: [f32; 2],
    /// Picture pixels per sheet pixel.
    scale: f32,
    size: [u32; 2],
}

struct Job {
    tree: Arc<usvg::Tree>,
    key: Key,
}

const MAX_SIDE: u32 = 4096;

/// Sheet region → RGBA pixels on white.
pub fn rasterize(tree: &usvg::Tree, origin: [f32; 2], scale: f32, size: [u32; 2]) -> Option<egui::ColorImage> {
    let (w, h) = (size[0].clamp(1, MAX_SIDE), size[1].clamp(1, MAX_SIDE));
    if !(scale > 0.0 && scale.is_finite()) || !origin.iter().all(|v| v.is_finite()) {
        return None;
    }
    let mut pixmap = tiny_skia::Pixmap::new(w, h)?;
    pixmap.fill(tiny_skia::Color::WHITE);
    let t = tiny_skia::Transform::from_row(scale, 0.0, 0.0, scale, -origin[0] * scale, -origin[1] * scale);
    resvg::render(tree, t, &mut pixmap.as_mut());
    Some(egui::ColorImage::from_rgba_premultiplied([w as usize, h as usize], pixmap.data()))
}

pub struct PlanView {
    tree: Option<(u64, Arc<usvg::Tree>)>,
    geometry: Option<(DrawingTransform, Vec<[[f64; 2]; 2]>)>,
    /// Sheet pixel at the middle of the view, and screen points per sheet pixel.
    centre: [f32; 2],
    zoom: f32,
    fitted: bool,
    picture: Option<(egui::TextureHandle, Key)>,
    asked: Option<Key>,
    jobs: Sender<Job>,
    done: Receiver<(Key, egui::ColorImage)>,
    pub error: Option<String>,
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
                if let Some(img) = rasterize(&job.tree, job.key.origin, job.key.scale, job.key.size) {
                    if tx.send((job.key, img)).is_err() {
                        break;
                    }
                    ctx.request_repaint();
                }
            }
        });
        PlanView { tree: None, geometry: None, centre: [800.0, 450.0], zoom: 1.0, fitted: false, picture: None, asked: None, jobs, done, error: None }
    }

    /// A new sheet (another level, a changed model). The view stays where it is.
    pub fn set_tree(&mut self, generation: u64, tree: Arc<usvg::Tree>) {
        self.tree = Some((generation, tree));
        self.error = None;
    }

    pub fn set_drawing(&mut self, generation: u64, drawing: crate::worker::PreparedDrawing) {
        self.set_tree(generation, drawing.tree);
        self.geometry = Some((drawing.transform, drawing.segments));
    }

    pub fn fail(&mut self, error: String) {
        self.tree = None;
        self.geometry = None;
        self.picture = None;
        self.asked = None;
        self.error = Some(error);
    }

    pub fn screen_to_model(&self, rect: egui::Rect, p: egui::Pos2) -> Option<[f64; 2]> {
        let (transform, _) = self.geometry.as_ref()?;
        let sheet = [
            f64::from(self.centre[0]) + f64::from(p.x - rect.center().x) / f64::from(self.zoom),
            f64::from(self.centre[1]) + f64::from(p.y - rect.center().y) / f64::from(self.zoom),
        ];
        Some(transform.to_model(sheet))
    }

    pub fn model_to_screen(&self, rect: egui::Rect, p: [f64; 2]) -> Option<egui::Pos2> {
        let (transform, _) = self.geometry.as_ref()?;
        let q = transform.to_sheet(p);
        Some(egui::pos2(rect.center().x + (q[0] as f32 - self.centre[0]) * self.zoom, rect.center().y + (q[1] as f32 - self.centre[1]) * self.zoom))
    }

    /// Affine model-mm → window-point mapping for remote point picking. y points up in the
    /// drawing and down in the window; `origin` is where drawing [0,0] lands on screen.
    pub fn screen_mapping(&self, rect: egui::Rect) -> Option<serde_json::Value> {
        let (t, _) = self.geometry.as_ref()?;
        let scale = f64::from(self.zoom) / t.mm_per_px;
        let origin = [
            f64::from(rect.center().x) - t.x0 * scale - f64::from(self.centre[0] * self.zoom),
            f64::from(rect.center().y) + t.y1 * scale - f64::from(self.centre[1] * self.zoom),
        ];
        Some(serde_json::json!({"origin":origin,"points_per_mm":scale,"viewport":[rect.left(),rect.top(),rect.width(),rect.height()]}))
    }

    pub fn tolerance_mm(&self) -> Option<f64> {
        self.geometry.as_ref().map(|(t, _)| (10.0 * t.mm_per_px / f64::from(self.zoom)).min(100_000.0))
    }

    pub fn cursor(&self, rect: egui::Rect, p: egui::Pos2, options: Option<SnapOptions>) -> Option<([f64; 2], Option<DrawingSnap>)> {
        let point = self.screen_to_model(rect, p)?;
        let (_, segments) = self.geometry.as_ref()?;
        let snap = options.and_then(|settings| snap_drawing(segments, point, self.tolerance_mm()?, settings));
        Some((snap.map_or(point, |s| s.point), snap))
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
        let origin = [self.centre[0] - rect.width() / 2.0 / self.zoom, self.centre[1] - rect.height() / 2.0 / self.zoom];
        let size = [(rect.width() * ppp).round().max(1.0) as u32, (rect.height() * ppp).round().max(1.0) as u32];
        Some(Key { generation: *generation, origin, scale: self.zoom * ppp, size })
    }

    /// A PNG-ready picture of what the view shows, made now.
    pub fn snapshot(&self, rect: egui::Rect, ppp: f32) -> Option<egui::ColorImage> {
        let key = self.key(rect, ppp)?;
        let (_, tree) = self.tree.as_ref()?;
        rasterize(tree, key.origin, key.scale, key.size)
    }

    /// Parsing the SVG finishes before the on-screen raster job. Window captures must wait
    /// for the texture of this generation and view, rather than capture a blank/older sheet.
    pub fn ready(&self, rect: egui::Rect, ppp: f32) -> bool {
        self.key(rect, ppp).is_some_and(|wanted| self.picture.as_ref().is_some_and(|(_, shown)| *shown == wanted))
    }

    pub fn ui(&mut self, ui: &mut egui::Ui, picking: bool) -> egui::Response {
        let (rect, resp) = ui.allocate_exact_size(ui.available_size(), egui::Sense::click_and_drag());
        let painter = ui.painter_at(rect);
        painter.rect_filled(rect, 0.0, theme::PAPER);
        while let Ok((key, img)) = self.done.try_recv() {
            let options = egui::TextureOptions::LINEAR;
            match &mut self.picture {
                Some((tex, k)) => {
                    tex.set(img, options);
                    *k = key;
                }
                None => self.picture = Some((ui.ctx().load_texture("trigon-plan", img, options), key)),
            }
        }
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
        if resp.hovered() {
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
    fn picking_inverts_the_real_drawing_transform_after_pan_and_zoom() {
        let mut view = PlanView::new(egui::Context::default());
        view.geometry = Some((DrawingTransform { x0: -24000.0, y1: 18000.0, mm_per_px: 40.0 }, vec![]));
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
        let (key, img) = view.done.recv_timeout(std::time::Duration::from_secs(3)).unwrap();
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
