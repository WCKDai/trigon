//! Trigon Studio's window: one session, run by the owner (toolbar, palette, undo) and by agents
//! (the control channel), with the model in 3D or as a plan, the activity feed, and what each
//! change did highlighted for a few seconds.

use std::path::PathBuf;
use std::sync::Arc;
use std::time::{Duration, Instant};

use eframe::egui;
use serde_json::{Value, json};
use trigon_control::auth::{LaunchFiles, Listen};
use trigon_control::{Presence, Responder, Server};
use trigon_data::{Credentials, Live};
use trigon_engine::{Session, commands};
use trigon_model::build::{PhaseFilter, View};

use crate::activity::{Action, Changes, Feed, Who};
use crate::authoring::{Authoring, Tab};
use crate::camera::Orbit;
use crate::drafting::{Action as DrawingAction, Drafting};
use crate::palette::Palette;
use crate::plan_view::PlanView;
use crate::render::{self, Viewport3d};
use crate::scene::{self, Scene};
use crate::theme;
use crate::worker::DrawingRequest;
use crate::worker::{Job, Worker};

/// How long a change stays highlighted, and how much of that is the fade.
const HIGHLIGHT: Duration = Duration::from_millis(4000);
const FADE: Duration = Duration::from_millis(1500);
/// An agent counts as working this long after its latest request.
const WORKING: Duration = Duration::from_millis(1500);
/// Control requests handled per frame, so a flood cannot starve drawing.
const REQUESTS_PER_FRAME: usize = 64;
/// How long a screenshot waits for a rebuild in progress, or for the window's frame.
pub(crate) const SHOT_WAIT: Duration = Duration::from_secs(30);

const CHANGED: [f32; 4] = [1.0, 0.55, 0.05, 0.55];
const REMOVED: [f32; 4] = [0.95, 0.10, 0.10, 0.45];
const SELECTED: [f32; 4] = [0.15, 0.45, 1.0, 0.40];
pub const AGENT_BLUE: egui::Color32 = egui::Color32::from_rgb(70, 140, 230);

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Mode {
    ThreeD,
    Plan,
    Drawing,
}

impl Mode {
    pub fn name(self) -> &'static str {
        match self {
            Mode::ThreeD => "3d",
            Mode::Plan => "plan",
            Mode::Drawing => "drawing",
        }
    }
}

pub fn filter_name(f: PhaseFilter) -> &'static str {
    match f {
        PhaseFilter::Complete => "complete",
        PhaseFilter::Demolition => "demolition",
        PhaseFilter::New => "new",
        PhaseFilter::All => "all",
    }
}

pub fn parse_filter(s: &str) -> Option<PhaseFilter> {
    Some(match s {
        "complete" => PhaseFilter::Complete,
        "demolition" => PhaseFilter::Demolition,
        "new" => PhaseFilter::New,
        "all" => PhaseFilter::All,
        _ => return None,
    })
}

/// How Studio was started.
pub struct Launch {
    pub file: Option<PathBuf>,
    /// Where the control channel listens; `None`: it is off.
    pub control: Option<Listen>,
}

struct Highlight {
    started: Instant,
}

pub enum ShotTarget {
    Viewport,
    Window,
}

/// A screenshot waiting for a rebuild to land, or for the window's next frame.
pub struct Shot {
    pub responder: Responder,
    pub target: ShotTarget,
    pub path: Option<String>,
    pub deadline: Instant,
    pub requested: bool,
    /// Bring the window to the front first (it takes focus from the owner: only when asked).
    pub raise: bool,
}

pub struct StudioApp {
    pub(crate) session: Session,
    pub(crate) feed: Feed,
    palette: Palette,
    pub(crate) server: Option<Server>,
    pub(crate) presence: Option<Arc<Presence>>,
    _launch_files: Option<LaunchFiles>,
    pub(crate) control_note: String,
    pub(crate) shots: Vec<Shot>,

    worker: Worker,
    generation: u64,
    /// The latest job that rebuilds the scene, and the one whose scene is shown.
    scene_wanted: u64,
    scene_generation: u64,
    /// The same for the plan sheet.
    plan_wanted: u64,
    plan_generation: u64,
    /// The feed entry whose change the rebuild of this generation shows.
    cause: Option<(u64, u64)>,
    pub(crate) scene: Arc<Scene>,
    previous: Option<Arc<Scene>>,
    scene_uploaded: bool,
    build_error: Option<String>,
    timings: Option<(f64, f64)>,
    plan_ms: Option<f64>,

    pub(crate) mode: Mode,
    /// 3D: show up to this level (cut 1.2 m above its floor); `None`: everything.
    pub(crate) level: Option<usize>,
    pub(crate) plan_level: usize,
    pub(crate) drawing: String,
    drafting: Drafting,
    pub(crate) phase: usize,
    pub(crate) filter: PhaseFilter,
    pub(crate) camera: Orbit,
    fit_pending: bool,
    pub(crate) render_state: Option<eframe::egui_wgpu::RenderState>,
    viewport: Option<Viewport3d>,
    texture: Option<egui::TextureId>,
    texture_size: [u32; 2],
    /// Screen pixels per point, as of the last frame.
    pub(crate) ppp: f32,
    /// Minimised or covered: eframe draws no frames, so the window can't be captured.
    pub(crate) window_hidden: bool,
    /// Input sent with `ui.click` and `ui.key`, fed to egui one event per frame.
    pub(crate) synthetic: std::collections::VecDeque<egui::Event>,
    pub(crate) viewport_rect: Option<egui::Rect>,
    redraw: bool,
    highlight: Option<Highlight>,
    highlight_dirty: bool,
    diff: scene::Diff,
    pub(crate) selection: Option<usize>,
    authoring: Authoring,
    pending_selection: Option<String>,
    selection_dirty: bool,
    pub(crate) plan: PlanView,
    cut_faces_dark: bool,
    confirm_close: bool,
    allow_close: bool,
    status: Option<(String, bool)>,
    title: String,
    /// Where Open and Save as… go on systems without native dialogs here.
    #[cfg(not(any(target_os = "macos", target_os = "windows")))]
    path_text: String,
}

impl StudioApp {
    pub fn new(cc: &eframe::CreationContext<'_>, launch: Launch) -> StudioApp {
        let ctx = cc.egui_ctx.clone();
        theme::apply(&ctx);
        let wake = ctx.clone();
        if let Some(rs) = &cc.wgpu_render_state {
            // wgpu panics on a validation error by default; a driver quirk is reported instead.
            rs.device.on_uncaptured_error(Arc::new(|e| eprintln!("trigon-studio: GPU error: {e}")));
        }
        let mut app = StudioApp {
            session: Session::new(Box::new(Live::new(Credentials::from_env()))),
            feed: Feed::default(),
            palette: Palette::default(),
            server: None,
            presence: None,
            _launch_files: None,
            control_note: "agent channel off".into(),
            shots: Vec::new(),
            worker: Worker::start(move || wake.request_repaint()),
            generation: 0,
            scene_wanted: 0,
            scene_generation: 0,
            plan_wanted: 0,
            plan_generation: 0,
            cause: None,
            scene: Arc::new(Scene::default()),
            previous: None,
            scene_uploaded: false,
            build_error: None,
            timings: None,
            plan_ms: None,
            mode: Mode::ThreeD,
            level: None,
            plan_level: 0,
            drawing: String::new(),
            drafting: Drafting::default(),
            phase: 0,
            filter: PhaseFilter::Complete,
            camera: Orbit::default(),
            fit_pending: true,
            render_state: cc.wgpu_render_state.clone(),
            viewport: cc.wgpu_render_state.as_ref().map(|rs| Viewport3d::new(&rs.device, &rs.adapter)),
            texture: None,
            texture_size: [0, 0],
            ppp: cc.egui_ctx.pixels_per_point(),
            window_hidden: false,
            synthetic: std::collections::VecDeque::new(),
            viewport_rect: None,
            redraw: true,
            highlight: None,
            highlight_dirty: false,
            diff: scene::Diff::default(),
            selection: None,
            authoring: Authoring::default(),
            pending_selection: None,
            selection_dirty: false,
            plan: PlanView::new(ctx.clone()),
            cut_faces_dark: true,
            confirm_close: false,
            allow_close: false,
            status: None,
            title: String::new(),
            #[cfg(not(any(target_os = "macos", target_os = "windows")))]
            path_text: String::new(),
        };
        if let Some(listen) = launch.control {
            app.start_control(listen, &ctx);
        }
        match &launch.file {
            Some(path) if path.exists() => {
                let _ = app.run(Who::Owner, "document.open", json!({"path": path}));
            }
            Some(path) => {
                // A new file: saved there on the first save.
                app.session.path = Some(path.clone());
                app.status = Some((format!("{} does not exist yet; it is written on Save", path.display()), false));
                app.request_rebuild(None);
            }
            None => app.request_rebuild(None),
        }
        if app.viewport.is_none() {
            app.status = Some(("no GPU renderer: the 3D view is unavailable".into(), true));
        }
        app
    }

    fn start_control(&mut self, listen: Listen, ctx: &egui::Context) {
        let Some(token) = trigon_control::auth::env_token().or_else(trigon_control::auth::new_token) else {
            self.control_note = "agent channel off: no random token available".into();
            return;
        };
        let listener = match trigon_control::auth::bind(listen) {
            Ok(l) => l,
            Err(e) => {
                self.control_note = format!("agent channel off: {e}");
                eprintln!("trigon-studio: control channel not started: {e}");
                return;
            }
        };
        let wake = ctx.clone();
        match Server::start(listener, token.clone(), Arc::new(move || wake.request_repaint())) {
            Ok(server) => {
                let port = server.port();
                let (files, notes) = LaunchFiles::write(port, &token);
                eprintln!("trigon-studio: control channel on 127.0.0.1:{port} ({})", notes.join("; "));
                self.control_note = format!("127.0.0.1:{port}");
                self.presence = Some(server.presence());
                self.server = Some(server);
                self._launch_files = Some(files);
            }
            Err(e) => self.control_note = format!("agent channel off: {e}"),
        }
    }

    pub(crate) fn view(&self) -> View {
        View { phase: self.phase, filter: self.filter }
    }

    /// Runs one command, logs it in the feed and, if it may have changed the model, rebuilds.
    pub(crate) fn run(&mut self, who: Who, id: &str, params: Value) -> Result<Value, String> {
        let t = Instant::now();
        let r = self.session.execute(id, params.clone()).map_err(|e| e.to_string());
        let ms = t.elapsed().as_secs_f64() * 1000.0;
        let seq = self.feed.push(who, id, params, r.clone(), ms);
        let mutates = commands().iter().find(|c| c.id == id).is_some_and(|c| c.mutates);
        let undo_redo = matches!(id, "edit.undo" | "edit.redo");
        if let Ok(v) = &r
            && (mutates || undo_redo)
        {
            if undo_redo && (v.get("undone") == Some(&json!(false)) || v.get("redone") == Some(&json!(false))) {
                return r;
            }
            if matches!(id, "document.open" | "document.new" | "document.from_template") {
                self.fit_pending = true;
                self.selection = None;
                self.authoring = Authoring::default();
                self.drafting = Drafting::default();
                self.drawing.clear();
                if self.mode == Mode::Drawing {
                    self.mode = Mode::ThreeD;
                }
                self.plan.fail("Preparing drawing…".into());
                self.pending_selection = None;
                self.level = None;
                self.plan_level = self.session.doc.buildings.first().map_or(0, |b| b.ground_level);
                self.phase = self.session.doc.active_index();
                self.plan.fit();
            }
            if matches!(id, "phase.set" | "phase.activate") {
                self.phase = self.session.doc.active_index();
            }
            self.request_rebuild(Some(seq));
        }
        r
    }

    /// Build the model again (off the UI thread); `cause` is the feed entry it shows the change of.
    pub(crate) fn request_rebuild(&mut self, cause: Option<u64>) {
        let phases = self.session.doc.phases.len().max(1);
        if self.phase >= phases {
            self.phase = self.session.doc.active_index();
        }
        let levels = self.session.doc.buildings.first().map_or(0, |b| b.storeys.len());
        if self.level.is_some_and(|l| l >= levels) {
            self.level = None;
        }
        if self.plan_level >= levels.max(1) {
            self.plan_level = 0;
        }
        self.generation += 1;
        self.scene_wanted = self.generation;
        if let Some(seq) = cause {
            self.cause = Some((self.generation, seq));
        }
        let drawing = self.drawing_request();
        if drawing.is_some() {
            self.plan_wanted = self.generation;
        }
        self.worker.submit(Job {
            generation: self.generation,
            doc: Arc::new(self.session.doc.clone()),
            view: self.view(),
            want_scene: true,
            drawing,
        });
    }

    /// Draw the plan again (another level or phase; the model is the same).
    pub(crate) fn request_plan(&mut self) {
        self.generation += 1;
        self.plan_wanted = self.generation;
        self.worker.submit(Job {
            generation: self.generation,
            doc: Arc::new(self.session.doc.clone()),
            view: self.view(),
            want_scene: false,
            drawing: self.drawing_request(),
        });
    }

    /// True while a rebuild of the 3D scene is on its way.
    pub(crate) fn building(&self) -> bool {
        self.scene_wanted > self.scene_generation
    }

    /// True while the plan sheet for the current view is on its way.
    pub(crate) fn drawing_plan(&self) -> bool {
        self.plan_wanted > self.plan_generation
    }

    fn poll_worker(&mut self) {
        while let Some(done) = self.worker.try_recv() {
            if let Some(scene) = done.scene
                && done.generation > self.scene_generation
            {
                self.scene_generation = done.generation;
                match scene {
                    Ok(new) => {
                        self.timings = Some((done.build_ms, done.scene_ms));
                        self.build_error = None;
                        self.apply_scene(new, done.generation);
                    }
                    Err(e) => self.build_error = Some(e),
                }
            }
            if let Some((request, tree)) = done.drawing
                && done.generation >= self.plan_wanted
                && Some(request) == self.drawing_request()
            {
                self.plan_generation = done.generation;
                self.plan_ms = Some(done.plan_ms);
                match tree {
                    Ok(t) => self.plan.set_drawing(done.generation, t),
                    Err(e) => self.plan.fail(e),
                }
            }
        }
    }

    fn apply_scene(&mut self, new: Scene, generation: u64) {
        let old = self.scene.clone();
        let fresh = self.fit_pending || old.elements.is_empty();
        let diff = if fresh { scene::Diff::default() } else { scene::diff(&old, &new) };
        // Keep the view still when the scene's origin moves (a basement, a longer wall).
        let shift = old.shift_to(&new);
        if !fresh {
            for (t, d) in self.camera.target.iter_mut().zip(shift) {
                *t += d;
            }
        }
        if let Some((g, seq)) = self.cause
            && g <= generation
            && fresh
        {
            // A model opened or started anew: nothing to compare with.
            let _ = seq;
            self.cause = None;
        }
        if let Some((g, seq)) = self.cause
            && g <= generation
        {
            let ids = |s: &Scene, list: &[usize]| list.iter().filter_map(|i| s.elements.get(*i).map(|e| e.id.clone())).collect::<Vec<_>>();
            self.feed.set_changes(seq, Changes { changed: ids(&new, &diff.changed), removed: ids(&old, &diff.removed) });
            self.cause = None;
        }
        let selected = self.selection.and_then(|i| old.elements.get(i)).map(|e| e.id.clone());
        let new = Arc::new(new);
        self.selection = self
            .pending_selection
            .take()
            .and_then(|id| {
                new.find(&id).or_else(|| {
                    let root = id.split_once(':').map_or(id.as_str(), |(_, root)| root);
                    new.find(&format!("{root}.L{}", self.plan_level))
                        .or_else(|| new.elements.iter().position(|e| e.id.starts_with(&format!("{root}.L"))))
                })
            })
            .or_else(|| selected.and_then(|id| new.find(&id)));
        self.selection_dirty = true;
        if !diff.is_empty() {
            self.highlight = Some(Highlight { started: Instant::now() });
            self.highlight_dirty = true;
            self.previous = Some(old);
        }
        self.diff = diff;
        self.scene = new;
        self.scene_uploaded = false;
        if self.fit_pending {
            self.fit();
            self.fit_pending = false;
        }
        self.redraw = true;
    }

    pub(crate) fn fit(&mut self) {
        if let Some((lo, hi)) = self.scene.bounds {
            let aspect = self.viewport_rect.map_or(1.6, |r| r.width() / r.height().max(1.0));
            self.camera.fit(lo, hi, aspect);
            self.redraw = true;
        }
        self.plan.fit();
    }

    /// Frame one element, from outside its facade when it has one.
    pub(crate) fn focus(&mut self, i: usize) {
        if let Some(e) = self.scene.elements.get(i)
            && let Some((lo, hi)) = e.bounds
        {
            let aspect = self.viewport_rect.map_or(1.6, |r| r.width() / r.height().max(1.0));
            self.camera.focus(lo, hi, e.facing, aspect);
            self.mode = Mode::ThreeD;
            self.redraw = true;
        }
    }

    /// The change highlight's strength now (1, fading to 0), or `None` once it is over.
    fn highlight_alpha(&self) -> Option<f32> {
        let h = self.highlight.as_ref()?;
        let t = h.started.elapsed();
        if t >= HIGHLIGHT {
            return None;
        }
        let fade_from = HIGHLIGHT - FADE;
        Some(if t < fade_from { 1.0 } else { 1.0 - (t - fade_from).as_secs_f32() / FADE.as_secs_f32() })
    }

    /// Upload what changed to the GPU: the scene, the highlight, the selection.
    pub(crate) fn sync_gpu(&mut self, rs: &eframe::egui_wgpu::RenderState) {
        let expired = self.highlight.is_some() && self.highlight_alpha().is_none();
        let Some(vp) = self.viewport.as_mut() else { return };
        if !self.scene_uploaded {
            vp.set_scene(&rs.device, &self.scene);
            self.scene_uploaded = true;
        }
        if self.highlight_dirty {
            let mut data = self.scene.overlay(&self.diff.highlight, CHANGED, [0.0; 3]);
            if let Some(prev) = &self.previous {
                data.extend(prev.overlay(&self.diff.removed, REMOVED, prev.shift_to(&self.scene)));
            }
            vp.set_overlay(&rs.device, &data);
            self.highlight_dirty = false;
        }
        if expired {
            self.highlight = None;
            self.previous = None;
            vp.set_overlay(&rs.device, &[]);
            self.redraw = true;
        }
        if self.selection_dirty {
            let data = self.selection.map(|i| self.scene.overlay(&[i], SELECTED, [0.0; 3])).unwrap_or_default();
            vp.set_selection(&rs.device, &data);
            self.selection_dirty = false;
        }
    }

    pub(crate) fn clip_z(&self) -> Option<f32> {
        self.level.and_then(|l| self.scene.clip_z(l))
    }

    pub(crate) fn render_view(&self, size: [u32; 2], ppp: f32) -> render::View {
        let aspect = size[0] as f32 / size[1].max(1) as f32;
        render::View {
            view_proj: self.camera.view_proj(aspect),
            eye: self.camera.eye(),
            clip_z: self.clip_z(),
            size,
            half_line: 0.6 * ppp.max(1.0),
            highlight: self.highlight_alpha().unwrap_or(0.0),
            cut_faces_dark: self.cut_faces_dark,
        }
    }

    /// A PNG of the 3D view drawn offscreen at `size` pixels.
    pub(crate) fn capture_3d(&mut self, rs: &eframe::egui_wgpu::RenderState, size: [u32; 2], ppp: f32) -> Result<([u32; 2], Vec<u8>), String> {
        self.sync_gpu(rs);
        let view = self.render_view(size, ppp);
        let vp = self.viewport.as_mut().ok_or("no GPU renderer: the 3D view is unavailable")?;
        let (size, rgba) = vp.capture(rs, &view)?;
        Ok((size, render::png(size, &rgba)?))
    }

    fn undo_redo(&mut self, action: Action) {
        let id = match action {
            Action::Undo => "edit.undo",
            Action::Redo => "edit.redo",
        };
        self.status = match self.run(Who::Owner, id, json!({})) {
            Ok(_) => Some((if id == "edit.undo" { "Undo completed" } else { "Redo completed" }.into(), false)),
            Err(error) => Some((error, true)),
        };
    }

    fn open_file(&mut self, path: PathBuf) {
        if let Err(e) = self.run(Who::Owner, "document.open", json!({"path": path})) {
            self.status = Some((e, true));
        }
    }

    fn save(&mut self, path: Option<PathBuf>) {
        let params = match path {
            Some(p) => json!({"path": p}),
            None => json!({}),
        };
        match self.run(Who::Owner, "document.save", params) {
            Ok(_) => self.status = Some(("saved".into(), false)),
            Err(e) => self.status = Some((e, true)),
        }
    }

    fn pick_open(&mut self) {
        #[cfg(any(target_os = "macos", target_os = "windows"))]
        if let Some(p) = rfd::FileDialog::new().add_filter("Trigon model", &["trigon"]).pick_file() {
            self.open_file(p);
        }
        #[cfg(not(any(target_os = "macos", target_os = "windows")))]
        if !self.path_text.is_empty() {
            let p = PathBuf::from(self.path_text.clone());
            self.open_file(p);
        }
    }

    fn pick_save_as(&mut self) {
        #[cfg(any(target_os = "macos", target_os = "windows"))]
        if let Some(p) = rfd::FileDialog::new().add_filter("Trigon model", &["trigon"]).set_file_name("model.trigon").save_file() {
            self.save(Some(p));
        }
        #[cfg(not(any(target_os = "macos", target_os = "windows")))]
        if !self.path_text.is_empty() {
            let p = PathBuf::from(self.path_text.clone());
            self.save(Some(p));
        }
    }

    fn shortcuts(&mut self, ctx: &egui::Context) {
        if ctx.text_edit_focused() {
            return;
        }
        let (undo, redo, palette, save, open, fit) = ctx.input_mut(|i| {
            let cmd = egui::Modifiers::COMMAND;
            let shift_cmd = egui::Modifiers::COMMAND | egui::Modifiers::SHIFT;
            (
                i.consume_key(cmd, egui::Key::Z),
                i.consume_key(shift_cmd, egui::Key::Z) || i.consume_key(cmd, egui::Key::Y),
                i.consume_key(cmd, egui::Key::K),
                i.consume_key(cmd, egui::Key::S),
                i.consume_key(cmd, egui::Key::O),
                i.consume_key(egui::Modifiers::NONE, egui::Key::F),
            )
        });
        if redo {
            self.undo_redo(Action::Redo);
        } else if undo {
            self.undo_redo(Action::Undo);
        }
        if palette {
            self.palette.show();
        }
        if save {
            if self.session.path.is_some() { self.save(None) } else { self.pick_save_as() }
        }
        if open {
            self.pick_open();
        }
        if fit {
            self.fit();
        }
    }

    fn update_title(&mut self, ctx: &egui::Context) {
        let name = self.session.path.as_ref().and_then(|p| p.file_name()).map_or("untitled".into(), |n| n.to_string_lossy().into_owned());
        let title = format!("{name}{} — Trigon Studio", if self.session.dirty { " •" } else { "" });
        if title != self.title {
            ctx.send_viewport_cmd(egui::ViewportCommand::Title(title.clone()));
            self.title = title;
        }
    }

    fn levels(&self) -> Vec<String> {
        self.session.doc.buildings.first().map(|b| b.storeys.iter().map(|s| s.name.clone()).collect()).unwrap_or_default()
    }

    fn toolbar(&mut self, ui: &mut egui::Ui) {
        ui.horizontal_wrapped(|ui| {
            if ui.button("Open…").on_hover_text("Open a .trigon model (⌘O)").clicked() {
                self.pick_open();
            }
            let save = ui.add_enabled(true, egui::Button::new("Save")).on_hover_text("Save (⌘S)");
            if save.clicked() {
                if self.session.path.is_some() { self.save(None) } else { self.pick_save_as() }
            }
            if ui.button("Save as…").clicked() {
                self.pick_save_as();
            }
            ui.menu_button("Templates", |ui| {
                if ui.button("New from template…").clicked() {
                    self.pick_template(false);
                    ui.close();
                }
                if ui.button("Save project as template…").clicked() {
                    self.pick_template(true);
                    ui.close();
                }
            });
            #[cfg(not(any(target_os = "macos", target_os = "windows")))]
            ui.add(egui::TextEdit::singleline(&mut self.path_text).hint_text("path/to/model.trigon").desired_width(180.0));
            ui.separator();
            let before = (self.mode, self.level, self.plan_level, self.phase, self.filter, self.drawing.clone());
            ui.selectable_value(&mut self.mode, Mode::ThreeD, "3D");
            ui.selectable_value(&mut self.mode, Mode::Plan, "Plan");
            if ui.add_enabled(!self.session.doc.views.is_empty(), egui::Button::selectable(self.mode == Mode::Drawing, "Drawing")).clicked() {
                self.mode = Mode::Drawing;
                self.authoring.tab = Tab::Drawings;
                if self.drawing.is_empty() {
                    self.drawing = self.session.doc.views.first().map(|v| v.id.clone()).unwrap_or_default();
                }
            }
            let levels = self.levels();
            match self.mode {
                Mode::ThreeD => {
                    let label = self.level.and_then(|l| levels.get(l)).map_or("All levels".to_string(), |n| format!("Up to {n}"));
                    egui::ComboBox::from_id_salt("level").selected_text(label).show_ui(ui, |ui| {
                        ui.selectable_value(&mut self.level, None, "All levels");
                        for (i, n) in levels.iter().enumerate() {
                            ui.selectable_value(&mut self.level, Some(i), format!("Up to {n}"));
                        }
                    });
                }
                Mode::Drawing => {
                    let label = self.session.doc.views.iter().find(|v| v.id == self.drawing).map_or("Choose drawing", |v| v.name.as_str());
                    egui::ComboBox::from_id_salt("saved-drawing").selected_text(label).show_ui(ui, |ui| {
                        for v in &self.session.doc.views {
                            ui.selectable_value(&mut self.drawing, v.id.clone(), &v.name);
                        }
                    });
                }
                Mode::Plan => {
                    let label = levels.get(self.plan_level).cloned().unwrap_or_else(|| "Level".into());
                    egui::ComboBox::from_id_salt("plan-level").selected_text(label).show_ui(ui, |ui| {
                        for (i, n) in levels.iter().enumerate() {
                            ui.selectable_value(&mut self.plan_level, i, n);
                        }
                    });
                }
            }
            let phases: Vec<(String, String)> = self.session.doc.phases.iter().map(|p| (p.id.clone(), p.name.clone())).collect();
            if phases.len() > 1 {
                let label = phases.get(self.phase).map_or("Phase".to_string(), |p| p.1.clone());
                egui::ComboBox::from_id_salt("phase").selected_text(label).show_ui(ui, |ui| {
                    for (i, (_, name)) in phases.iter().enumerate() {
                        ui.selectable_value(&mut self.phase, i, name);
                    }
                });
                egui::ComboBox::from_id_salt("filter").selected_text(filter_name(self.filter)).show_ui(ui, |ui| {
                    for f in [PhaseFilter::Complete, PhaseFilter::Demolition, PhaseFilter::New, PhaseFilter::All] {
                        ui.selectable_value(&mut self.filter, f, filter_name(f));
                    }
                });
            }
            if ui.button("Fit").on_hover_text("Frame the whole model (F)").clicked() {
                self.fit();
            }
            ui.separator();
            if ui.button("Commands…").on_hover_text("Search and run any command (⌘K)").clicked() {
                self.palette.show();
            }
            let after = (self.mode, self.level, self.plan_level, self.phase, self.filter, self.drawing.clone());
            if before != after {
                if before.5 != after.5 {
                    self.plan.fit();
                    self.drafting.picking = None;
                }
                self.view_changed(before.3 != after.3 || before.4 != after.4);
            }
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| self.presence_chip(ui));
        });
    }

    /// The view settings changed: rebuild what the new view needs.
    pub(crate) fn view_changed(&mut self, phase_changed: bool) {
        self.redraw = true;
        if phase_changed {
            self.request_rebuild(None);
        } else if self.mode != Mode::ThreeD {
            self.request_plan();
        }
    }

    /// Names of the connected agents, and whether one is working now.
    fn agents(&self) -> Option<(String, bool)> {
        let p = self.presence.as_ref()?;
        if p.agents() == 0 {
            return None;
        }
        let names: Vec<String> = p.clients().into_values().filter(|n| n != "trigon-mcp").collect();
        let names = if names.is_empty() { "agent".to_string() } else { names.join(", ") };
        let working = p.busy() || p.since_last_request().is_some_and(|d| d < WORKING);
        Some((names, working))
    }

    fn presence_chip(&mut self, ui: &mut egui::Ui) {
        match self.agents() {
            None => {
                ui.weak(format!("no agent · {}", self.control_note)).on_hover_text("Point an MCP client at `trigon mcp` to let an agent work here.");
                crate::activity::dot(ui, egui::Color32::from_gray(170));
            }
            Some((names, working)) => {
                let t = ui.input(|i| i.time) as f32;
                let (text, colour) = if working {
                    let pulse = 0.55 + 0.45 * (t * 6.0).sin().abs();
                    (format!("{names} is working"), AGENT_BLUE.gamma_multiply(pulse))
                } else {
                    (format!("{names} connected"), egui::Color32::from_rgb(60, 170, 90))
                };
                // Right to left: the text, then its dot before it.
                ui.label(egui::RichText::new(text).strong().color(colour)).on_hover_text(format!("control channel {}", self.control_note));
                crate::activity::dot(ui, colour);
                if working {
                    ui.ctx().request_repaint_after(Duration::from_millis(50));
                } else {
                    ui.ctx().request_repaint_after(WORKING);
                }
            }
        }
    }

    fn status_bar(&mut self, ui: &mut egui::Ui) {
        ui.horizontal(|ui| {
            let path = self.session.path.as_ref().map_or("untitled".into(), |p| p.display().to_string());
            ui.weak(path);
            if self.session.dirty {
                ui.label(egui::RichText::new("unsaved").color(egui::Color32::from_rgb(200, 130, 20)));
            }
            if let Some((text, error)) = &self.status {
                if *error {
                    ui.colored_label(egui::Color32::from_rgb(200, 70, 60), text);
                } else {
                    ui.weak(text);
                }
            }
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                ui.weak(&self.control_note);
                if let (Mode::Plan, Some(ms)) = (self.mode, self.plan_ms) {
                    ui.weak(format!("plan {ms:.0} ms"));
                }
                if let Some((b, s)) = self.timings {
                    ui.weak(format!("rebuild {b:.0} + {s:.0} ms"))
                        .on_hover_text("build_view, then the viewport's triangles and edges (off the UI thread)");
                }
                ui.weak(format!(
                    "{} elements · {} triangles · {} edges",
                    self.scene.elements.len(),
                    self.scene.triangle_count(),
                    self.scene.edge_count()
                ));
            });
        });
    }

    fn selection_panel(&mut self, ui: &mut egui::Ui) {
        if self.authoring.tab == Tab::Drawings {
            if self.mode == Mode::Drawing {
                if let Some(run) = self.drafting.properties(ui, &self.session.doc, &self.drawing) {
                    self.run_drafting(run);
                }
            } else {
                ui.heading("Drawing properties");
                ui.weak("Open or create a saved drawing in the Drawings browser to add text, dimensions, leaders and levels.");
            }
            return;
        }
        ui.heading(if self.authoring.tab == Tab::Families { "Type properties" } else { "Properties" });
        let selected = self.selection.and_then(|i| self.scene.elements.get(i)).cloned();
        if let Some(run) = self.authoring.ui(ui, &self.session.doc, selected.as_ref(), self.plan_level) {
            match self.run(Who::Owner, run.command, run.params) {
                Ok(value) => {
                    if let Some(targets) = value.get("targets").and_then(Value::as_array) {
                        let ids = targets.iter().filter_map(Value::as_str).collect::<Vec<_>>();
                        self.pending_selection = ids.first().map(|s| s.to_string());
                        if !self.authoring.targets.is_empty() {
                            self.authoring.targets = ids.join("\n");
                        }
                    }
                    self.status = Some((format!("{} completed", run.command), false));
                }
                Err(error) => self.status = Some((error, true)),
            }
        }
        ui.add_space(6.0);
        ui.separator();
        let Some(e) = selected else {
            return;
        };
        theme::eyebrow(ui, "PICKED ELEMENT");
        egui::Grid::new("selection").num_columns(2).striped(true).show(ui, |ui| {
            ui.weak("id");
            ui.monospace(&e.id);
            ui.end_row();
            ui.weak("category");
            ui.label(e.kind);
            ui.end_row();
            if let Some(l) = e.level {
                ui.weak("level");
                ui.label(self.levels().get(l).cloned().unwrap_or_else(|| l.to_string()));
                ui.end_row();
            }
            for (k, v) in &e.props {
                ui.weak(*k);
                ui.label(v);
                ui.end_row();
            }
        });
    }

    fn pick_template(&mut self, save: bool) {
        #[cfg(any(target_os = "macos", target_os = "windows"))]
        let path = {
            let picker = rfd::FileDialog::new().add_filter("Trigon template", &["trigon"]);
            if save { picker.set_file_name("template.trigon").save_file() } else { picker.pick_file() }
        };
        #[cfg(not(any(target_os = "macos", target_os = "windows")))]
        let path = (!self.path_text.is_empty()).then(|| PathBuf::from(&self.path_text));
        if let Some(path) = path {
            let command = if save { "document.save_template" } else { "document.from_template" };
            match self.run(Who::Owner, command, json!({"path":path})) {
                Ok(_) => self.status = Some((if save { "Template saved" } else { "Unsaved copy created from template" }.into(), false)),
                Err(error) => self.status = Some((error, true)),
            }
        }
    }

    fn project_browser(&mut self, ui: &mut egui::Ui) {
        ui.horizontal(|ui| {
            ui.selectable_value(&mut self.authoring.tab, Tab::Project, "Project");
            ui.selectable_value(&mut self.authoring.tab, Tab::Families, "Families");
            ui.selectable_value(&mut self.authoring.tab, Tab::Drawings, "Drawings");
        });
        ui.separator();
        if self.authoring.tab == Tab::Drawings {
            if let Some(action) = self.drafting.library(ui, &self.session.doc, if self.mode == Mode::Drawing { &self.drawing } else { "" }) {
                match action {
                    DrawingAction::Open(id) => self.open_drawing(id),
                    DrawingAction::Run(run) => self.run_drafting(run),
                }
            }
            return;
        }
        if self.authoring.tab == Tab::Families {
            self.authoring.library(ui, &self.session.doc);
            return;
        }
        theme::eyebrow(ui, "PROJECT BROWSER");
        let project = self.session.doc.buildings.first().map_or("Untitled project", |b| b.name.as_str());
        ui.heading(project);
        let before = (self.mode, self.level, self.plan_level);
        ui.add_space(8.0);
        if ui.selectable_label(self.mode == Mode::ThreeD && self.level.is_none(), "3D model").clicked() {
            self.mode = Mode::ThreeD;
            self.level = None;
        }
        let levels = self.levels();
        egui::CollapsingHeader::new("Floor plans").default_open(true).show(ui, |ui| {
            for (i, name) in levels.iter().enumerate() {
                if ui.selectable_label(self.mode == Mode::Plan && self.plan_level == i, name).clicked() {
                    self.mode = Mode::Plan;
                    self.plan_level = i;
                }
            }
        });
        egui::CollapsingHeader::new("3D cut views").default_open(true).show(ui, |ui| {
            for (i, name) in levels.iter().enumerate() {
                if ui.selectable_label(self.mode == Mode::ThreeD && self.level == Some(i), format!("{name} · cut")).clicked() {
                    self.mode = Mode::ThreeD;
                    self.level = Some(i);
                    self.plan_level = i;
                }
            }
        });
        let facades = self.session.doc.buildings.iter().flat_map(|b| b.facades.iter().map(|f| (f.id.clone(), f.kind))).collect::<Vec<_>>();
        let mut open_facade = None;
        egui::CollapsingHeader::new("Facade elevations").show(ui, |ui| {
            for (id, kind) in facades {
                if ui.selectable_label(false, format!("{id} · {kind:?}")).clicked() {
                    open_facade = Some(id);
                }
            }
        });
        if let Some(facade) = open_facade {
            let existing = self
                .session
                .doc
                .views
                .iter()
                .find(|v| matches!(&v.kind,trigon_model::annotation::DrawingViewKind::Facade {facade:f} if f==&facade))
                .map(|v| v.id.clone());
            if let Some(id) = existing {
                self.open_drawing(id);
            } else {
                let stem = format!("facade-{facade}");
                let id = (0..256)
                    .map(|i| if i == 0 { stem.clone() } else { format!("{stem}-{i}") })
                    .find(|id| !self.session.doc.views.iter().any(|v| &v.id == id))
                    .unwrap_or(stem);
                self.run_drafting(crate::authoring::Run {
                    command: "view.define",
                    params: json!({"id":id,"name":format!("Facade {facade}"),"kind":{"view":"facade","facade":facade}}),
                });
            }
            return;
        }
        theme::eyebrow(ui, "GROUPS");
        let groups = self.session.doc.groups.iter().map(|g| (g.id.clone(), g.name.clone(), g.targets.len())).collect::<Vec<_>>();
        if groups.is_empty() {
            ui.weak("Collect elements in Properties to save a group.");
        }
        for (id, name, n) in groups {
            if ui.selectable_label(self.authoring.targets == format!("group:{id}"), format!("{name} · {n}")).clicked() {
                self.authoring.targets = format!("group:{id}");
            }
        }
        theme::eyebrow(ui, "ELEMENTS ON THIS STOREY");
        let elements = self
            .scene
            .elements
            .iter()
            .enumerate()
            .filter(|(_, e)| !e.host && e.level == Some(self.plan_level))
            .map(|(i, e)| (i, e.id.clone(), e.kind))
            .collect::<Vec<_>>();
        for (i, id, kind) in elements {
            let response = ui.selectable_label(self.selection == Some(i), &id).on_hover_text(kind);
            if response.clicked() {
                self.selection = Some(i);
                self.selection_dirty = true;
                self.redraw = true;
                self.authoring.targets.clear();
            }
            if response.double_clicked() {
                self.mode = Mode::ThreeD;
                self.focus(i);
            }
        }
        if before != (self.mode, self.level, self.plan_level) {
            self.view_changed(false);
        }
    }

    fn drawing_request(&self) -> Option<DrawingRequest> {
        match self.mode {
            Mode::ThreeD => None,
            Mode::Plan => Some(DrawingRequest::Plan(self.plan_level)),
            Mode::Drawing => Some(DrawingRequest::Saved(self.drawing.clone())),
        }
    }

    pub(crate) fn show_drawing_properties(&mut self) {
        self.authoring.tab = Tab::Drawings;
        self.drafting.opened(&self.session.doc, &self.drawing);
    }

    pub(crate) fn open_drawing(&mut self, id: String) {
        self.mode = Mode::Drawing;
        self.drawing = id;
        self.authoring.tab = Tab::Drawings;
        self.drafting.opened(&self.session.doc, &self.drawing);
        self.plan.fit();
        self.request_plan();
    }

    fn run_drafting(&mut self, mut run: crate::authoring::Run) {
        if run.command == "view.export" {
            if let Some(p) = self.session.doc.phases.get(self.phase) {
                run.params["phase"] = json!(p.id);
            }
            run.params["filter"] = json!(filter_name(self.filter));
        }
        match self.run(Who::Owner, run.command, run.params) {
            Ok(value) => {
                if let Some(id) = value.get("view").and_then(|v| v.get("id")).and_then(Value::as_str) {
                    self.open_drawing(id.into());
                }
                if let Some(a) = value.get("annotation").and_then(|a| serde_json::from_value(a.clone()).ok()) {
                    self.drafting.select(&a);
                }
                self.status = Some((format!("{} completed", run.command), false));
            }
            Err(error) => self.status = Some((error, true)),
        }
    }

    fn viewport_drawing(&mut self, ui: &mut egui::Ui) {
        if self.plan_wanted == 0 {
            self.request_plan();
        }
        if ui.input(|i| i.key_pressed(egui::Key::Escape)) {
            self.drafting.picking = None;
        }
        let picking = self.mode == Mode::Drawing && self.authoring.tab == Tab::Drawings && self.drafting.picking.is_some();
        let response = self.plan.ui(ui, picking);
        let rect = response.rect;
        self.viewport_rect = Some(rect);
        if self.drawing_plan() || self.plan.error.is_some() || !self.plan.ready(rect, self.ppp) {
            return;
        }
        if self.mode == Mode::Drawing && self.authoring.tab == Tab::Drawings {
            let painter = ui.painter_at(rect);
            if picking || self.drafting.selected.is_some() {
                for p in self.drafting.points() {
                    if let Some(at) = self.plan.model_to_screen(rect, *p) {
                        painter.circle_stroke(at, 4.0, egui::Stroke::new(1.5, theme::ACCENT));
                    }
                }
            }
            if let Some(p) = response.hover_pos() {
                let snap = (!ui.input(|i| i.modifiers.alt)).then(|| self.drafting.options());
                if let Some((point, snapped)) = self.plan.cursor(rect, p, snap) {
                    if picking {
                        response.clone().on_hover_cursor(egui::CursorIcon::Crosshair);
                        if let Some(at) = self.plan.model_to_screen(rect, point) {
                            painter.rect_stroke(
                                egui::Rect::from_center_size(at, egui::vec2(10.0, 10.0)),
                                0.0,
                                egui::Stroke::new(1.5, theme::ACCENT),
                                egui::StrokeKind::Middle,
                            );
                            if let Some(s) = snapped {
                                painter.text(
                                    at + egui::vec2(10.0, -12.0),
                                    egui::Align2::LEFT_BOTTOM,
                                    s.kind.name(),
                                    egui::FontId::proportional(12.0),
                                    theme::ACCENT,
                                );
                            }
                        }
                        if let Some(i) = self.drafting.picking.filter(|i| *i > 0)
                            && let Some(from) = self.drafting.points().get(i - 1).and_then(|q| self.plan.model_to_screen(rect, *q))
                            && let Some(to) = self.plan.model_to_screen(rect, point)
                        {
                            painter.line_segment([from, to], egui::Stroke::new(1.0, theme::ACCENT));
                        }
                        if response.clicked() {
                            self.drafting.picked(point);
                        }
                    } else if response.clicked()
                        && let Some(raw) = self.plan.screen_to_model(rect, p)
                        && let Some(a) = Drafting::hit(&self.session.doc, &self.drawing, raw, self.plan.tolerance_mm().unwrap_or(0.0))
                    {
                        self.drafting.select(a);
                    }
                    let text =
                        format!("x {:.3}  ·  y {:.3} mm{}", point[0], point[1], snapped.map_or(String::new(), |s| format!("  ·  {}", s.kind.name())));
                    let at = rect.left_bottom() + egui::vec2(12.0, -12.0);
                    painter.text(at, egui::Align2::LEFT_BOTTOM, text, egui::FontId::monospace(12.0), theme::INK);
                }
            }
        }
    }

    fn viewport_3d(&mut self, ui: &mut egui::Ui) {
        let (rect, resp) = ui.allocate_exact_size(ui.available_size(), egui::Sense::click_and_drag());
        if self.viewport_rect != Some(rect) {
            self.viewport_rect = Some(rect);
            self.redraw = true;
        }
        let aspect = rect.width() / rect.height().max(1.0);
        // Orbit with the left button; pan with the right or middle button, or Shift.
        let shift = ui.input(|i| i.modifiers.shift);
        let d = resp.drag_delta();
        if d != egui::Vec2::ZERO {
            if resp.dragged_by(egui::PointerButton::Primary) && !shift {
                self.camera.orbit(d.x * 0.008, d.y * 0.008);
            } else {
                self.camera.pan(d.x, d.y, rect.height());
            }
            self.redraw = true;
        }
        if resp.hovered() {
            let (scroll, pinch) = ui.input(|i| (i.smooth_scroll_delta.y, i.zoom_delta()));
            let factor = (1.0 / pinch) * (-scroll / 300.0).exp();
            if (factor - 1.0).abs() > 1e-4
                && let Some(p) = resp.hover_pos()
            {
                let ndc = [(p.x - rect.left()) / rect.width() * 2.0 - 1.0, 1.0 - (p.y - rect.top()) / rect.height() * 2.0];
                let (_, dir) = self.camera.ray(ndc, aspect);
                self.camera.zoom_at(factor, dir);
                self.redraw = true;
            }
        }
        if resp.double_clicked() {
            self.fit();
        } else if resp.clicked()
            && let Some(p) = resp.interact_pointer_pos()
        {
            let ndc = [(p.x - rect.left()) / rect.width() * 2.0 - 1.0, 1.0 - (p.y - rect.top()) / rect.height() * 2.0];
            let (o, dir) = self.camera.ray(ndc, aspect);
            self.selection = self.scene.pick(o, dir, self.clip_z());
            self.selection_dirty = true;
            self.redraw = true;
        }
        let ppp = ui.ctx().pixels_per_point();
        let size = [(rect.width() * ppp).round().max(1.0) as u32, (rect.height() * ppp).round().max(1.0) as u32];
        if self.texture_size != size {
            self.texture_size = size;
            self.redraw = true;
        }
        let painter = ui.painter_at(rect);
        let animating = self.highlight.is_some();
        if let Some(rs) = self.render_state.clone() {
            self.sync_gpu(&rs);
            let view = self.render_view(size, ppp);
            if (self.redraw || animating)
                && let Some(vp) = self.viewport.as_mut()
                && let Some(id) = vp.render(&rs, &view)
            {
                self.texture = Some(id);
                self.redraw = false;
            }
        }
        if let Some(id) = self.texture {
            painter.image(id, rect, egui::Rect::from_min_max(egui::pos2(0.0, 0.0), egui::pos2(1.0, 1.0)), egui::Color32::WHITE);
        } else {
            painter.rect_filled(rect, 0.0, egui::Color32::from_gray(200));
        }
        if animating {
            ui.ctx().request_repaint();
        }
        self.viewport_overlay(&painter, rect);
    }

    /// Words over the view: what is building, errors, the agent at work, the change just made.
    fn viewport_overlay(&self, painter: &egui::Painter, rect: egui::Rect) {
        let font = egui::FontId::proportional(13.0);
        let mut y = rect.bottom() - 10.0;
        let mut chip = |text: String, colour: egui::Color32| {
            let galley = painter.layout_no_wrap(text, font.clone(), egui::Color32::WHITE);
            let r = egui::Rect::from_min_size(egui::pos2(rect.left() + 10.0, y - galley.size().y - 8.0), galley.size() + egui::vec2(16.0, 8.0));
            painter.rect_filled(r, 6.0, colour);
            painter.galley(r.min + egui::vec2(8.0, 4.0), galley, egui::Color32::WHITE);
            y = r.top() - 6.0;
        };
        if let Some(e) = &self.build_error {
            chip(format!("The model can't be built: {e}"), egui::Color32::from_rgb(170, 50, 40));
        }
        if let Some((names, true)) = self.agents() {
            let doing = self.feed.latest().filter(|e| matches!(e.who, Who::Agent(_))).map(|e| format!(" · {}", e.command)).unwrap_or_default();
            chip(format!("{names} is working{doing}"), AGENT_BLUE.gamma_multiply(0.9));
        }
        if self.highlight.is_some() && !self.diff.is_empty() {
            let plural = |n: usize| if n == 1 { "element" } else { "elements" };
            let (n, removed) = (self.diff.highlight.len(), self.diff.removed.len());
            let text = match (n, removed) {
                (n, 0) => format!("{n} {} changed", plural(n)),
                (0, r) => format!("{r} {} removed", plural(r)),
                (n, r) => format!("{n} {} changed, {r} removed", plural(n)),
            };
            chip(text, egui::Color32::from_rgb(190, 110, 10));
        }
        if self.building() {
            chip("Rebuilding…".into(), egui::Color32::from_black_alpha(140));
        }
        if let Some(l) = self.level
            && let Some(name) = self.levels().get(l)
        {
            painter.text(
                rect.right_top() + egui::vec2(-10.0, 10.0),
                egui::Align2::RIGHT_TOP,
                format!("Cut 1.2 m above {name}"),
                font,
                egui::Color32::from_gray(30),
            );
        }
    }

    fn confirm_close_dialog(&mut self, ctx: &egui::Context) {
        if ctx.input(|i| i.viewport().close_requested()) && self.session.dirty && !self.allow_close {
            ctx.send_viewport_cmd(egui::ViewportCommand::CancelClose);
            self.confirm_close = true;
        }
        if !self.confirm_close {
            return;
        }
        egui::Window::new("Save changes?").collapsible(false).resizable(false).anchor(egui::Align2::CENTER_CENTER, [0.0, 0.0]).show(ctx, |ui| {
            ui.label("The model has unsaved changes.");
            ui.horizontal(|ui| {
                if ui.button("Save").clicked() {
                    if self.session.path.is_some() {
                        self.save(None)
                    } else {
                        self.pick_save_as()
                    }
                    if !self.session.dirty {
                        self.allow_close = true;
                        ctx.send_viewport_cmd(egui::ViewportCommand::Close);
                    }
                    self.confirm_close = false;
                }
                if ui.button("Don't save").clicked() {
                    self.allow_close = true;
                    self.confirm_close = false;
                    ctx.send_viewport_cmd(egui::ViewportCommand::Close);
                }
                if ui.button("Cancel").clicked() {
                    self.confirm_close = false;
                }
            });
        });
    }
}

impl eframe::App for StudioApp {
    fn raw_input_hook(&mut self, ctx: &egui::Context, raw_input: &mut egui::RawInput) {
        if let Some(event) = self.synthetic.pop_front() {
            raw_input.events.push(event);
        }
        if !self.synthetic.is_empty() {
            ctx.request_repaint();
        }
    }

    fn logic(&mut self, ctx: &egui::Context, frame: &mut eframe::Frame) {
        self.window_hidden = ctx.input(|i| i.viewport().minimized == Some(true) || i.viewport().occluded == Some(true));
        self.poll_worker();
        let _ = frame;
        for _ in 0..REQUESTS_PER_FRAME {
            let Some(req) = self.server.as_ref().and_then(Server::try_recv) else { break };
            self.handle_request(req, ctx);
        }
        self.serve_shots(ctx);
    }

    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        let ctx = ui.ctx().clone();
        self.ppp = ctx.pixels_per_point();
        self.collect_window_shots(&ctx);
        self.shortcuts(&ctx);
        self.update_title(&ctx);
        egui::Panel::top("studio-title").frame(egui::Frame::new().fill(theme::INK).inner_margin(egui::Margin::symmetric(16, 10))).show(ui, |ui| {
            ui.horizontal(|ui| {
                ui.label(egui::RichText::new("TRIGON").size(20.0).strong().color(egui::Color32::WHITE));
                ui.label(egui::RichText::new("STUDIO").size(11.0).color(egui::Color32::from_gray(170)));
                ui.add_space(18.0);
                let name =
                    self.session.path.as_ref().and_then(|p| p.file_name()).map_or("Untitled project".into(), |s| s.to_string_lossy().into_owned());
                ui.label(
                    egui::RichText::new(format!("{name}{}", if self.session.dirty { " · unsaved" } else { "" })).color(egui::Color32::from_gray(220)),
                );
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    ui.label(egui::RichText::new("mm   ·   Z up").size(11.0).color(egui::Color32::from_rgb(151, 207, 196)));
                });
            });
        });
        egui::Panel::top("toolbar").show(ui, |ui| {
            ui.add_space(4.0);
            self.toolbar(ui);
            ui.add_space(2.0);
        });
        egui::Panel::bottom("status").show(ui, |ui| self.status_bar(ui));
        egui::Panel::left("project-browser").resizable(true).default_size(235.0).min_size(180.0).show(ui, |ui| {
            egui::ScrollArea::vertical().id_salt("project-scroll").show(ui, |ui| self.project_browser(ui));
        });
        egui::Panel::right("side").resizable(true).default_size(330.0).min_size(280.0).show(ui, |ui| {
            egui::Panel::bottom("activity-panel").resizable(true).default_size(230.0).min_size(120.0).show(ui, |ui| {
                if let Some(a) = self.feed.ui(ui, true, true) {
                    self.undo_redo(a);
                }
            });
            egui::ScrollArea::vertical().id_salt("properties-scroll").show(ui, |ui| self.selection_panel(ui));
        });
        egui::CentralPanel::no_frame().show(ui, |ui| match self.mode {
            Mode::ThreeD => self.viewport_3d(ui),
            Mode::Plan | Mode::Drawing => self.viewport_drawing(ui),
        });
        if let Some(run) = self.palette.ui(&ctx) {
            let r = self.run(Who::Owner, run.command, run.params);
            self.palette.set_outcome(r.map(|v| crate::activity::pretty(&v)));
        }
        self.confirm_close_dialog(&ctx);
    }
}
