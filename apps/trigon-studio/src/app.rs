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
use trigon_model::annotation::DrawingViewKind;
use trigon_model::build::{PhaseFilter, View};

use crate::activity::{Action, Changes, Feed, Who};
use crate::authoring::{Authoring, Tab};
use crate::camera::Orbit;
use crate::drafting::{Action as DrawingAction, Drafting};
use crate::icons::{self, Icon};
use crate::palette::Palette;
use crate::plan_view::PlanView;
use crate::properties::{self, Mapping, ProjectedHandle, Properties};
use crate::render::{self, Viewport3d};
use crate::scene::{self, Scene};
use crate::schedules::{self, Schedules};
use crate::theme;
use crate::worker::DrawingRequest;
use crate::worker::{Done, Job, Worker};

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
const SELECTED: [f32; 4] = [1.0, 0.42, 0.21, 0.45];
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

#[derive(Clone, Debug, PartialEq, Eq)]
enum ViewTarget {
    ThreeD(Option<usize>),
    Plan(usize),
    Drawing(String),
}

struct OpenView {
    target: ViewTarget,
    navigation: Option<crate::plan_view::Navigation>,
}

#[derive(Clone, Copy, Default, PartialEq)]
enum Inspector {
    #[default]
    Elements,
    Entrance,
    Terrain,
    Outline,
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
    pub(crate) model_export: crate::model_export::ModelExport,
    pub(crate) phase: usize,
    pub(crate) filter: PhaseFilter,
    pub(crate) phase_colors: bool,
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
    inspector: Inspector,
    entrances: crate::entrances::Editor,
    terrain_panel: crate::terrain_panel::TerrainPanel,
    outline_panel: crate::outline_panel::OutlinePanel,
    precision_review: crate::precision_review::Review,
    open_views: Vec<OpenView>,
    active_view: Option<ViewTarget>,
    view_ribbon: bool,
    ribbon_expanded: bool,
    focus_canvas: bool,
    dock_open: bool,
    dock_properties: bool,
    dock_rect: Option<egui::Rect>,
    last_property_id: Option<String>,
    show_command_strip: bool,
    model_tools: crate::model_tools::ModelTools,
    empty_pan: egui::Vec2,
    empty_mm_per_point: f64,
    browser_query: String,
    pub(crate) browser_rect: Option<egui::Rect>,
    command_query: String,
    show_activity: bool,
    schedules: Schedules,
    properties: Properties,
    pending_selection: Option<String>,
    pending_family_targets: Option<Vec<String>>,
    family_text_owned_on_input: bool,
    selection_dirty: bool,
    pub(crate) plan: PlanView,
    cut_faces_dark: bool,
    confirm_close: bool,
    allow_close: bool,
    status: Option<(String, bool)>,
    title: String,
    /// Where Open and Save as… go on systems without native dialogs here.
    path_text: String,
}

impl StudioApp {
    pub fn new(cc: &eframe::CreationContext<'_>, launch: Launch) -> StudioApp {
        let ctx = cc.egui_ctx.clone();
        let session = Session::new(Box::new(Live::new(Credentials::from_env())));
        let mut app = Self::with_context(ctx.clone(), cc.wgpu_render_state.clone(), session);
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

    /// The workspace state is also usable without a GPU for state-transition tests.
    fn with_context(ctx: egui::Context, render_state: Option<eframe::egui_wgpu::RenderState>, session: Session) -> StudioApp {
        theme::apply(&ctx);
        let wake = ctx.clone();
        if let Some(rs) = &render_state {
            // wgpu panics on a validation error by default; a driver quirk is reported instead.
            rs.device.on_uncaptured_error(Arc::new(|e| eprintln!("trigon-studio: GPU error: {e}")));
        }
        StudioApp {
            session,
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
            model_export: crate::model_export::ModelExport::default(),
            phase: 0,
            filter: PhaseFilter::Complete,
            phase_colors: false,
            camera: Orbit::default(),
            fit_pending: true,
            viewport: render_state.as_ref().map(|rs| Viewport3d::new(&rs.device, &rs.adapter)),
            render_state,
            texture: None,
            texture_size: [0, 0],
            ppp: ctx.pixels_per_point(),
            window_hidden: false,
            synthetic: std::collections::VecDeque::new(),
            viewport_rect: None,
            redraw: true,
            highlight: None,
            highlight_dirty: false,
            diff: scene::Diff::default(),
            selection: None,
            authoring: Authoring::default(),
            inspector: Inspector::Elements,
            entrances: crate::entrances::Editor::default(),
            terrain_panel: crate::terrain_panel::TerrainPanel::default(),
            outline_panel: crate::outline_panel::OutlinePanel::default(),
            precision_review: crate::precision_review::Review::default(),
            open_views: Vec::new(),
            active_view: None,
            view_ribbon: false,
            ribbon_expanded: false,
            focus_canvas: false,
            dock_open: false,
            dock_properties: false,
            dock_rect: None,
            last_property_id: None,
            show_command_strip: false,
            model_tools: crate::model_tools::ModelTools::default(),
            empty_pan: egui::Vec2::ZERO,
            empty_mm_per_point: 20.0,
            browser_query: String::new(),
            browser_rect: None,
            command_query: String::new(),
            show_activity: false,
            schedules: Schedules::default(),
            properties: Properties::default(),
            pending_selection: None,
            pending_family_targets: None,
            family_text_owned_on_input: false,
            selection_dirty: false,
            plan: PlanView::new(ctx.clone()),
            cut_faces_dark: true,
            confirm_close: false,
            allow_close: false,
            status: None,
            title: String::new(),
            path_text: String::new(),
        }
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
        if id == "precision.preview" {
            self.precision_review.clear();
        }
        let t = Instant::now();
        let r = self.session.execute(id, params.clone()).map_err(|e| e.to_string());
        let ms = t.elapsed().as_secs_f64() * 1000.0;
        let seq = self.feed.push(who, id, params, r.clone(), ms);
        let mutates = commands().iter().find(|c| c.id == id).is_some_and(|c| c.mutates);
        let undo_redo = matches!(id, "edit.undo" | "edit.redo");
        if let Ok(value) = &r
            && id == "precision.preview"
        {
            self.precision_review.set(value.clone());
        }
        if let Ok(v) = &r
            && (mutates || undo_redo)
        {
            if undo_redo && (v.get("undone") == Some(&json!(false)) || v.get("redone") == Some(&json!(false))) {
                return r;
            }
            self.precision_review.clear();
            if matches!(id, "document.open" | "document.new" | "document.from_template") {
                self.fit_pending = true;
                self.selection = None;
                self.authoring = Authoring::default();
                self.model_tools.cancel();
                self.model_export.project_replaced();
                self.drafting.cancel_placement();
                self.empty_pan = egui::Vec2::ZERO;
                self.inspector = Inspector::Elements;
                self.entrances.clear();
                self.terrain_panel = crate::terrain_panel::TerrainPanel::default();
                self.outline_panel = crate::outline_panel::OutlinePanel::default();
                self.open_views.clear();
                self.active_view = None;
                self.browser_query.clear();
                self.view_ribbon = false;
                self.schedules = Schedules::default();
                self.properties = Properties::default();
                self.drafting = Drafting::default();
                self.drawing.clear();
                if self.mode == Mode::Drawing {
                    self.mode = Mode::ThreeD;
                }
                self.plan.fail("Preparing drawing…".into());
                self.pending_selection = None;
                self.pending_family_targets = None;
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
        let levels = self.levels().len();
        if self.level.is_some_and(|l| l >= levels) {
            self.level = None;
        }
        if self.plan_level >= levels.max(1) {
            self.plan_level = 0;
        }
        self.generation += 1;
        self.scene_wanted = self.generation;
        self.schedules.begin(self.generation);
        self.properties.begin(self.generation);
        self.build_error = None;
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
            phase_colors: self.phase_colors,
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
            phase_colors: self.phase_colors,
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
            self.apply_done(done);
        }
    }

    fn apply_done(&mut self, done: Done) {
        if let Some(scene) = done.scene
            && done.generation > self.scene_generation
            && done.generation >= self.scene_wanted
        {
            self.scene_generation = done.generation;
            match scene {
                Ok(prepared) => {
                    self.timings = Some((done.build_ms, done.scene_ms));
                    self.build_error = None;
                    self.schedules.finish(done.generation, Ok(prepared.schedule));
                    self.properties.finish(done.generation, prepared.properties);
                    self.apply_scene(prepared.scene, done.generation);
                }
                Err(e) => {
                    self.schedules.finish(done.generation, Err(e.clone()));
                    self.build_error = Some(e);
                    self.selection = None;
                    self.pending_selection = None;
                    self.pending_family_targets = None;
                    self.properties.begin(done.generation);
                    self.selection_dirty = true;
                }
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
        if let Some(targets) = self.pending_family_targets.take() {
            self.pending_selection = None;
            self.selection = targets.iter().find_map(|id| new.find_unique(id));
        } else {
            self.selection = self
                .pending_selection
                .take()
                .and_then(|id| {
                    new.find_unique(&id).or_else(|| {
                        let root = id.split_once(':').map_or(id.as_str(), |(_, root)| root);
                        new.find_unique(&format!("{root}.L{}", self.plan_level))
                            .or_else(|| new.elements.iter().position(|e| e.id.starts_with(&format!("{root}.L"))))
                    })
                })
                .or_else(|| selected.and_then(|id| new.find_unique(&id)));
        }
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
            if self.authoring.tab == Tab::Schedules {
                self.authoring.tab = Tab::Project;
            }
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

    fn show_model_export(&mut self, format: crate::model_export::Format) {
        let view = self.view();
        let level = if self.mode == Mode::ThreeD { self.level } else { None };
        self.model_export.show(format, &self.session.doc, view, level, self.session.path.as_deref());
    }

    fn poll_model_export(&mut self) {
        if let Some(done) = self.model_export.poll() {
            let seq = self.feed.push(Who::Owner, done.request.command, done.request.params, done.result.clone(), done.ms);
            self.feed.set_context(seq, done.request.scope.clone());
            self.status = Some(match done.result {
                Ok(result) => {
                    let path = result.get("written").and_then(Value::as_str).unwrap_or("destination");
                    (format!("Export complete: {path} · {}", done.request.scope), false)
                }
                Err(error) => (format!("Export failed: {error} · {}", done.request.scope), true),
            });
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
        if self.model_export.open
            || ctx.text_edit_focused()
            || (self.authoring.family_placement_active() && self.family_text_owned_on_input)
            || egui::Popup::is_any_open(ctx)
            || self.palette.open
            || self.confirm_close
        {
            return;
        }
        let (undo, redo, palette, save, open, fit) = ctx.input_mut(|i| {
            let cmd = egui::Modifiers::COMMAND;
            let shift_cmd = egui::Modifiers::COMMAND | egui::Modifiers::SHIFT;
            // egui matches command-Z with extra Shift too. Consume the more
            // specific Redo binding before trying the plain Undo binding.
            let redo = i.consume_key(shift_cmd, egui::Key::Z) || i.consume_key(cmd, egui::Key::Y);
            let undo = !redo && i.consume_key(cmd, egui::Key::Z);
            (
                undo,
                redo,
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
        if ctx.input_mut(|i| i.consume_key(egui::Modifiers::NONE, egui::Key::F11)) {
            self.focus_canvas = !self.focus_canvas;
        }
        if ctx.input_mut(|i| i.consume_key(egui::Modifiers::NONE, egui::Key::Escape)) {
            if self.model_tools.active() {
                self.model_tools.escape();
            }
            self.drafting.cancel_placement();
            self.authoring.cancel_family_placement();
        }
        if self.model_tools.active()
            && ctx.input_mut(|i| i.consume_key(egui::Modifiers::NONE, egui::Key::Enter))
            && let Some(run) = self.model_tools.finish(&self.session.doc)
        {
            self.run_model_tool(run);
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

    pub(crate) fn levels(&self) -> Vec<String> {
        let doc = &self.session.doc;
        let count = if doc.buildings.iter().any(|b| b.attic_level().is_some()) {
            doc.buildings.iter().map(|b| b.storeys.len()).max().unwrap_or(0)
        } else {
            doc.buildings.first().map_or(0, |b| b.storeys.len())
        };
        (0..count).filter_map(|level| doc.level_label(level)).collect()
    }

    fn app_bar(&mut self, ui: &mut egui::Ui) {
        let wide = ui.available_width() > 1100.0;
        let mac = ui.ctx().os() == egui::os::OperatingSystem::Mac;
        let search_hint = if mac { "⌘ K" } else { "Ctrl+K" };
        let undo_hint = if mac { "⌘Z" } else { "Ctrl+Z" };
        let redo_hint = if mac { "⇧⌘Z" } else { "Ctrl+Shift+Z" };
        let save_hint = if mac { "⌘S" } else { "Ctrl+S" };
        ui.horizontal(|ui| {
            theme::wordmark(ui);
            ui.separator();
            let project = self
                .session
                .path
                .as_ref()
                .and_then(|path| path.file_name())
                .map(|name| name.to_string_lossy().into_owned())
                .or_else(|| self.session.doc.buildings.first().map(|building| building.name.clone()))
                .unwrap_or_else(|| "Untitled project".into());
            let short = if project.chars().count() > 24 { format!("{}…", project.chars().take(24).collect::<String>()) } else { project.clone() };
            ui.menu_button(short, |ui| {
                ui.strong(&project);
                ui.separator();
                if ui.button("Open project…").clicked() {
                    self.pick_open();
                    ui.close();
                }
                if ui.button("Save as…").clicked() {
                    self.pick_save_as();
                    ui.close();
                }
                ui.separator();
                if ui.button("New from template…").clicked() {
                    self.pick_template(false);
                    ui.close();
                }
                if ui.button("Save project as template…").clicked() {
                    self.pick_template(true);
                    ui.close();
                }
            });
            if wide {
                ui.separator();
                if icons::button(ui, Icon::Undo, "", false).on_hover_text(format!("Undo the last model or annotation change ({undo_hint})")).clicked()
                {
                    self.undo_redo(Action::Undo);
                }
                if icons::button(ui, Icon::Redo, "", false).on_hover_text(format!("Redo ({redo_hint})")).clicked() {
                    self.undo_redo(Action::Redo);
                }
            }
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                let export = icons::button(ui, Icon::Export, "Export", false).on_hover_text("Export the BIM model, 3D geometry or a saved drawing");
                egui::Popup::menu(&export).show(|ui| {
                    ui.strong("Model exchange");
                    for format in [crate::model_export::Format::Ifc, crate::model_export::Format::Glb] {
                        if ui.add_enabled(!self.model_export.busy(), egui::Button::new(format.label())).clicked() {
                            self.show_model_export(format);
                            ui.close();
                        }
                    }
                    if self.model_export.busy() && ui.button("Export progress…").clicked() {
                        self.model_export.open = true;
                        ui.close();
                    }
                    ui.separator();
                    let saved = self.mode == Mode::Drawing && self.session.doc.views.iter().any(|view| view.id == self.drawing);
                    if ui.add_enabled(saved, egui::Button::new("Drawing · PDF, SVG, PNG, DXF…")).clicked() {
                        self.authoring.tab = Tab::Drawings;
                        self.drafting.show_export();
                        self.focus_canvas = false;
                        self.dock_open = true;
                        self.dock_properties = true;
                        ui.close();
                    }
                    if !saved {
                        ui.weak("Open a saved drawing to export its annotations and sheet settings.");
                    }
                });
                if icons::button(ui, Icon::Save, "Save", false).on_hover_text(format!("Save project ({save_hint})")).clicked() {
                    if self.session.path.is_some() { self.save(None) } else { self.pick_save_as() }
                }
                ui.separator();
                if icons::button(ui, Icon::Search, search_hint, false).on_hover_text(format!("Search and run any command ({search_hint})")).clicked()
                {
                    self.palette.show();
                }
            });
        });
    }

    fn workspace_tabs(&mut self, ui: &mut egui::Ui) {
        ui.horizontal_wrapped(|ui| {
            ui.spacing_mut().item_spacing.x = 3.0;
            for (tab, label) in
                [(Tab::Project, "Architecture"), (Tab::Families, "Components"), (Tab::Drawings, "Annotate"), (Tab::Schedules, "Schedules")]
            {
                if theme::workspace_tab(ui, label, self.authoring.tab == tab && !self.view_ribbon).clicked() {
                    self.model_tools.cancel();
                    self.inspect_elements();
                    self.authoring.tab = tab;
                    self.view_ribbon = false;
                    self.drafting.cancel_placement();
                    if tab == Tab::Families {
                        self.dock_open = true;
                        self.dock_properties = false;
                    }
                }
            }
            if theme::workspace_tab(ui, "View", self.view_ribbon).clicked() {
                self.view_ribbon = true;
            }
            ui.separator();
            if ui
                .selectable_label(self.dock_open && !self.dock_properties, "Browser")
                .on_hover_text("Project Browser · views, elements and families")
                .clicked()
            {
                self.dock_open = !(self.dock_open && !self.dock_properties);
                self.dock_properties = false;
            }
            if ui
                .selectable_label(self.dock_open && self.dock_properties, "Properties")
                .on_hover_text("Properties of the selected element, view or active tool")
                .clicked()
            {
                self.dock_open = !(self.dock_open && self.dock_properties);
                self.dock_properties = true;
            }
            if ui.selectable_label(self.ribbon_expanded, "Ribbon").on_hover_text("Expand or minimise the full ribbon").clicked() {
                self.ribbon_expanded = !self.ribbon_expanded;
            }
            if ui.selectable_label(self.focus_canvas, "Focus").on_hover_text("Focus canvas · F11").clicked() {
                self.focus_canvas = !self.focus_canvas;
            }
        });
    }

    fn compact_ribbon(&mut self, ui: &mut egui::Ui) {
        ui.horizontal_wrapped(|ui| {
            if self.view_ribbon {
                if ui.button("Fit view").clicked() {
                    self.fit();
                }
                ui.toggle_value(&mut self.show_activity, "Activity");
                ui.toggle_value(&mut self.show_command_strip, "Command bar");
                ui.toggle_value(&mut self.focus_canvas, "Focus canvas");
                return;
            }
            match self.authoring.tab {
                Tab::Project => {
                    if icons::button(ui, Icon::Select, "Select", !self.model_tools.active()).clicked() {
                        self.model_tools.cancel();
                        self.inspect_elements();
                    }
                    if icons::button(ui, Icon::Wall, "Wall", self.model_tools.active() && self.model_tools.feedback()["tool"] == "wall").clicked() {
                        self.begin_model_tool(false);
                    }
                    if icons::button(ui, Icon::Roof, "Roof", self.model_tools.active() && self.model_tools.feedback()["tool"] == "roof").clicked() {
                        self.begin_model_tool(true);
                    }
                    if icons::button(ui, Icon::Component, "Component", false).clicked() {
                        self.model_tools.cancel();
                        let selected = self.selection.and_then(|i| self.scene.elements.get(i));
                        self.authoring.begin_family_placement(&self.session.doc, selected);
                        self.dock_open = true;
                        self.dock_properties = true;
                    }
                    let modify = ui.menu_button("Modify", |ui| {
                        self.authoring.ribbon(ui);
                    });
                    if modify.response.clicked() {
                        self.model_tools.cancel();
                        self.dock_open = true;
                        self.dock_properties = true;
                    }
                    ui.menu_button("Set out", |ui| {
                        if self.coordination_ribbon(ui) {
                            self.dock_open = true;
                            self.dock_properties = true;
                            ui.close();
                        }
                    });
                    if self.model_tools.active() {
                        ui.separator();
                        ui.label(self.model_tools.title());
                        if ui.button("Finish").clicked()
                            && let Some(run) = self.model_tools.finish(&self.session.doc)
                        {
                            self.run_model_tool(run);
                        }
                        if ui.button("Cancel").clicked() {
                            self.model_tools.cancel();
                        }
                    }
                }
                Tab::Families => {
                    if ui.button("Browse types").clicked() {
                        self.dock_open = true;
                        self.dock_properties = false;
                    }
                    if ui.button("Edit type").clicked() {
                        if let Some(selected) = self.selection.and_then(|i| self.scene.elements.get(i)) {
                            self.authoring.choose_instance_type(&self.session.doc, selected);
                        }
                        self.dock_open = true;
                        self.dock_properties = true;
                    }
                    if ui.button("Duplicate type").clicked() {
                        self.authoring.duplicate_selected_type(&self.session.doc);
                        self.dock_open = true;
                        self.dock_properties = true;
                    }
                    if ui.button("Place component").clicked() {
                        let selected = self.selection.and_then(|i| self.scene.elements.get(i));
                        self.authoring.begin_family_placement(&self.session.doc, selected);
                        self.dock_open = true;
                        self.dock_properties = true;
                    }
                }
                Tab::Drawings => {
                    if self.mode != Mode::Drawing {
                        ui.label("Choose a saved drawing, or annotate the active floor plan.");
                        if ui.add_enabled(self.reference_level().is_some(), egui::Button::new("Annotate this plan")).clicked() {
                            self.ensure_annotation_view();
                        }
                    } else {
                        self.drafting.ribbon(ui, &self.session.doc, &self.drawing);
                    }
                }
                Tab::Schedules => {
                    ui.label("Select a row to inspect an instance, or open its shared type.");
                }
            }
        });
    }

    fn ensure_annotation_view(&mut self) {
        let Some(level) = self.reference_level() else { return };
        if let Some(view) =
            self.session.doc.views.iter().find(|v| matches!(v.kind,trigon_model::annotation::DrawingViewKind::Plan {level:l} if l==level))
        {
            self.open_drawing(view.id.clone());
            return;
        }
        let id = (0..256).map(|i| format!("plan-L{level}-{i}")).find(|id| !self.session.doc.views.iter().any(|v| v.id == *id));
        let Some(id) = id else {
            self.status = Some(("No free plan view ID.".into(), true));
            return;
        };
        let name = format!("Floor plan · {}", self.levels().get(level).map_or("Floor", String::as_str));
        match self.run(Who::Owner, "view.define", json!({"id":id,"name":name,"kind":{"view":"plan","level":level}})) {
            Ok(_) => self.open_drawing(id),
            Err(error) => self.status = Some((error, true)),
        }
    }

    fn ribbon(&mut self, ui: &mut egui::Ui) {
        let compact = ui.available_width() < 1200.0;
        egui::ScrollArea::horizontal().id_salt("ribbon-scroll").show(ui, |ui| {
            ui.spacing_mut().item_spacing = egui::vec2(8.0, 3.0);
            ui.horizontal(|ui| {
                ui.set_min_height(80.0);
                if self.view_ribbon {
                    ui.vertical(|ui| {
                        ui.horizontal(|ui| {
                            if icons::button(ui, Icon::Map, "Floor plan", self.mode == Mode::Plan).clicked() {
                                self.mode = Mode::Plan;
                                self.view_changed(false);
                            }
                            if icons::button(ui, Icon::Box, "3D view", self.mode == Mode::ThreeD).clicked() {
                                self.mode = Mode::ThreeD;
                                self.level = None;
                                self.view_changed(false);
                            }
                            if icons::button(ui, Icon::Bookmark, "Saved views", false).clicked() {
                                self.authoring.tab = Tab::Drawings;
                                self.view_ribbon = false;
                            }
                        });
                        theme::eyebrow(ui, "OPEN VIEWS");
                    });
                    ui.separator();
                    ui.vertical(|ui| {
                        ui.horizontal(|ui| {
                            if icons::button(ui, Icon::Align, "Fit view", false).clicked() {
                                self.fit();
                            }
                            ui.toggle_value(&mut self.show_activity, "Activity");
                        });
                        theme::eyebrow(ui, "WORKSPACE");
                    });
                } else {
                    match self.authoring.tab {
                        Tab::Project => {
                            ui.vertical(|ui| {
                                if ui.add_enabled_ui(!self.precision_review.is_pending(), |ui| self.authoring.ribbon(ui)).inner {
                                    self.inspect_elements();
                                    self.drafting.cancel_placement();
                                    self.dock_open = true;
                                    self.dock_properties = true;
                                }
                                theme::eyebrow(ui, "MODIFY");
                            });
                            ui.separator();
                            if compact {
                                ui.vertical(|ui| {
                                    ui.menu_button("Set out", |ui| {
                                        if self.coordination_ribbon(ui) {
                                            ui.close();
                                        }
                                    });
                                    theme::eyebrow(ui, "COORDINATION");
                                });
                                ui.vertical(|ui| {
                                    ui.menu_button("Components", |ui| {
                                        if self.components_ribbon(ui) {
                                            ui.close();
                                        }
                                    });
                                    theme::eyebrow(ui, "LIBRARY");
                                });
                                ui.vertical(|ui| {
                                    ui.menu_button("Build", |ui| {
                                        if self.build_ribbon(ui) {
                                            ui.close();
                                        }
                                    });
                                    theme::eyebrow(ui, "MODEL");
                                });
                                ui.vertical(|ui| {
                                    ui.menu_button("Site", |ui| {
                                        if self.site_ribbon(ui) {
                                            ui.close();
                                        }
                                    });
                                    theme::eyebrow(ui, "PROJECT");
                                });
                            } else {
                                ui.vertical(|ui| {
                                    self.coordination_ribbon(ui);
                                    theme::eyebrow(ui, "SET OUT");
                                });
                                ui.separator();
                                ui.vertical(|ui| {
                                    self.components_ribbon(ui);
                                    theme::eyebrow(ui, "COMPONENTS");
                                });
                                ui.separator();
                                ui.vertical(|ui| {
                                    self.build_ribbon(ui);
                                    theme::eyebrow(ui, "BUILD");
                                });
                                ui.separator();
                                ui.vertical(|ui| {
                                    self.site_ribbon(ui);
                                    theme::eyebrow(ui, "SITE");
                                });
                            }
                        }
                        Tab::Families => {
                            ui.vertical(|ui| {
                                icons::show(ui, Icon::Component, 24.0);
                                ui.label("Shared types · variants · placement · portable libraries");
                                theme::eyebrow(ui, "COMPONENT LIBRARY");
                            });
                        }
                        Tab::Drawings => {
                            ui.vertical(|ui| {
                                ui.horizontal_wrapped(|ui| {
                                    if icons::button(ui, Icon::Select, "Edit elements", false).clicked() {
                                        self.authoring.tab = Tab::Project;
                                        self.drafting.cancel_placement();
                                    }
                                    ui.add_enabled_ui(!self.precision_review.is_pending(), |ui| {
                                        self.drafting.ribbon(ui, &self.session.doc, if self.mode == Mode::Drawing { &self.drawing } else { "" })
                                    });
                                });
                                theme::eyebrow(ui, "ANNOTATION / DRAWING TOOLS");
                            });
                        }
                        Tab::Schedules => {
                            ui.label("Window & door inventory · select a row to edit in Properties");
                        }
                    }
                }
            });
        });
    }

    fn coordination_ribbon(&mut self, ui: &mut egui::Ui) -> bool {
        let mut chosen = false;
        egui::Grid::new("coordination-ribbon").num_columns(2).spacing(egui::vec2(4.0, 2.0)).show(ui, |ui| {
            let level = self.reference_level();
            if ui
                .add_enabled_ui(level.is_some(), |ui| icons::ribbon_button(ui, Icon::Guide, "Guide", false))
                .inner
                .on_hover_text("Create a red dotted plan reference with exact endpoints")
                .clicked()
                && let Some(level) = level
            {
                self.begin_reference_guide(level, None);
                chosen = true;
            }
            if icons::ribbon_button(ui, Icon::Outline, "Outline", self.inspector == Inspector::Outline)
                .on_hover_text("Inspect source edges and use them as alignment guides")
                .clicked()
            {
                self.inspect_project(Inspector::Outline);
                chosen = true;
            }
            ui.end_row();
            if icons::ribbon_button(ui, Icon::Align, "Wall face", false)
                .on_hover_text("Align selected walls to a guide or wall face, with a movement bound")
                .clicked()
            {
                self.inspect_elements();
                self.authoring.choose_wall_alignment();
                chosen = true;
            }
            if icons::ribbon_button(ui, Icon::Levels, "Copy levels", false).on_hover_text("Preview a transfer to named destination storeys").clicked()
            {
                self.inspect_elements();
                self.authoring.choose_copy_to_levels();
                chosen = true;
            }
        });
        chosen
    }

    fn components_ribbon(&mut self, ui: &mut egui::Ui) -> bool {
        let mut chosen = false;
        egui::Grid::new("components-ribbon").num_columns(2).spacing(egui::vec2(4.0, 2.0)).show(ui, |ui| {
            for (i, (icon, label, query)) in [
                (Icon::Window, "Windows", "window"),
                (Icon::Door, "Doors", "door"),
                (Icon::Railing, "Balconies", "balcony"),
                (Icon::Component, "All types", ""),
            ]
            .into_iter()
            .enumerate()
            {
                if icons::ribbon_button(ui, icon, label, false).on_hover_text("Open shared project types and placement controls").clicked() {
                    self.inspect_elements();
                    self.drafting.cancel_placement();
                    self.authoring.tab = Tab::Families;
                    self.authoring.search = query.into();
                    self.dock_open = true;
                    self.dock_properties = false;
                    chosen = true;
                }
                if i % 2 == 1 {
                    ui.end_row();
                }
            }
        });
        chosen
    }

    fn build_ribbon(&mut self, ui: &mut egui::Ui) -> bool {
        let mut chosen = false;
        egui::Grid::new("build-ribbon").num_columns(2).spacing(egui::vec2(4.0, 2.0)).show(ui, |ui| {
            for (i, (icon, label, query)) in
                [(Icon::Wall, "Walls", "interior"), (Icon::Stair, "Stairs", "stair"), (Icon::Roof, "Roof", "roof"), (Icon::Slab, "Slabs", "slab")]
                    .into_iter()
                    .enumerate()
            {
                if icons::ribbon_button(ui, icon, label, false).on_hover_text("Find modelling commands and their parameters").clicked() {
                    if query == "interior" {
                        self.begin_model_tool(false);
                    } else if query == "roof" {
                        self.begin_model_tool(true);
                    } else {
                        self.palette.show_query(query);
                    }
                    chosen = true;
                }
                if i % 2 == 1 {
                    ui.end_row();
                }
            }
        });
        chosen
    }

    fn site_ribbon(&mut self, ui: &mut egui::Ui) -> bool {
        let mut chosen = false;
        ui.vertical(|ui| {
            if icons::ribbon_button(ui, Icon::Entrance, "Entrances", self.inspector == Inspector::Entrance).clicked() {
                let id = self.session.doc.buildings.iter().find(|b| !b.entrances.is_empty()).map(|b| format!("{}.entrance0", b.id));
                if let Some(id) = id {
                    self.select_entrance(id);
                } else {
                    self.status = Some(("This project has no stored entrance records.".into(), true));
                }
                chosen = true;
            }
            if icons::ribbon_button(ui, Icon::Terrain, "Terrain", self.inspector == Inspector::Terrain).clicked() {
                self.inspect_project(Inspector::Terrain);
                chosen = true;
            }
        });
        chosen
    }

    fn command_strip(&mut self, ui: &mut egui::Ui) {
        ui.horizontal(|ui| {
            ui.label(egui::RichText::new("TRIGON").strong().size(11.0).color(theme::ACCENT));
            let response = ui.add(
                egui::TextEdit::singleline(&mut self.command_query)
                    .char_limit(256)
                    .hint_text("Find a modelling command…")
                    .desired_width((ui.available_width() * 0.45).clamp(140.0, 500.0)),
            );
            let enter = response.lost_focus() && ui.input(|input| input.key_pressed(egui::Key::Enter));
            if icons::button(ui, Icon::Search, "Commands", false).clicked() || enter {
                self.palette.show_query(&self.command_query);
            }
            ui.toggle_value(&mut self.show_activity, "Activity");
            if ui.available_width() > 250.0 {
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| self.presence_chip(ui));
            }
        });
    }

    fn view_target(&self) -> ViewTarget {
        match self.mode {
            Mode::ThreeD => ViewTarget::ThreeD(self.level),
            Mode::Plan => ViewTarget::Plan(self.plan_level),
            Mode::Drawing => ViewTarget::Drawing(self.drawing.clone()),
        }
    }

    fn sync_open_views(&mut self) {
        let target = self.view_target();
        if self.active_view.as_ref() == Some(&target) {
            return;
        }
        if let Some(previous) = &self.active_view
            && !matches!(previous, ViewTarget::ThreeD(_))
            && let Some(tab) = self.open_views.iter_mut().find(|tab| &tab.target == previous)
        {
            tab.navigation = Some(self.plan.navigation());
        }
        if let Some(navigation) = self.open_views.iter().find(|tab| tab.target == target).and_then(|tab| tab.navigation) {
            self.plan.restore_navigation(navigation);
        } else {
            if !matches!(target, ViewTarget::ThreeD(_)) {
                self.plan.fit();
            }
            if !self.open_views.iter().any(|tab| tab.target == target) {
                self.open_views.push(OpenView { target: target.clone(), navigation: None });
            }
        }
        self.active_view = Some(target);
    }

    fn activate_view(&mut self, target: ViewTarget) {
        match target {
            ViewTarget::ThreeD(level) => {
                self.mode = Mode::ThreeD;
                self.level = level;
            }
            ViewTarget::Plan(level) => {
                self.mode = Mode::Plan;
                self.plan_level = level;
            }
            ViewTarget::Drawing(id) => {
                self.open_drawing(id);
                return;
            }
        }
        self.drafting.cancel_placement();
        self.view_changed(false);
    }

    fn view_tabs(&mut self, ui: &mut egui::Ui) {
        self.sync_open_views();
        let levels = self.levels();
        self.open_views.retain(|tab| match &tab.target {
            ViewTarget::ThreeD(level) => level.is_none_or(|level| level < levels.len()),
            ViewTarget::Plan(level) => *level < levels.len() || (self.session.doc.buildings.is_empty() && *level == 0),
            ViewTarget::Drawing(id) => self.session.doc.views.iter().any(|view| &view.id == id),
        });
        if !self.open_views.iter().any(|tab| Some(&tab.target) == self.active_view.as_ref()) {
            self.activate_view(ViewTarget::ThreeD(None));
        }
        let targets = self.open_views.iter().map(|tab| tab.target.clone()).collect::<Vec<_>>();
        let mut close = None;
        egui::ScrollArea::horizontal().id_salt("view-tabs-scroll").show(ui, |ui| {
            ui.horizontal(|ui| {
                for target in targets {
                    let (icon, label) = match &target {
                        ViewTarget::ThreeD(level) => {
                            (Icon::Box, level.and_then(|level| levels.get(level)).map_or("3D / Coordination".into(), |name| format!("3D / {name}")))
                        }
                        ViewTarget::Plan(level) => (Icon::Map, levels.get(*level).cloned().unwrap_or_else(|| "Floor plan".into())),
                        ViewTarget::Drawing(id) => {
                            (Icon::Section, self.session.doc.views.iter().find(|v| &v.id == id).map_or(id.clone(), |v| v.name.clone()))
                        }
                    };
                    ui.push_id(format!("{target:?}"), |ui| {
                        let selected = self.active_view.as_ref() == Some(&target);
                        let response = icons::button(ui, icon, &label, selected);
                        if selected {
                            ui.painter()
                                .line_segment([response.rect.left_bottom(), response.rect.right_bottom()], egui::Stroke::new(2.0, theme::ACCENT));
                        }
                        if response.clicked() {
                            self.activate_view(target.clone());
                        }
                        if ui.small_button("×").on_hover_text("Close this view tab; the saved drawing stays in the project").clicked() {
                            close = Some(target.clone());
                        }
                    });
                }
            });
        });
        if let Some(target) = close {
            self.open_views.retain(|tab| tab.target != target);
            if self.active_view.as_ref() == Some(&target) {
                let next = self.open_views.last().map_or(ViewTarget::ThreeD(None), |tab| tab.target.clone());
                self.active_view = None;
                self.activate_view(next);
            }
        }
    }

    fn view_controls(&mut self, ui: &mut egui::Ui) {
        if !matches!(ui.ctx().os(), egui::os::OperatingSystem::Mac | egui::os::OperatingSystem::Windows) {
            // Keep the file-path fallback out of the wrapping view controls: its
            // text-edit width otherwise expands the canvas after a narrow resize.
            ui.add(egui::TextEdit::singleline(&mut self.path_text).hint_text("path/to/model.trigon").desired_width(ui.available_width()));
        }
        ui.horizontal_wrapped(|ui| {
            let before = (self.mode, self.level, self.plan_level, self.phase, self.filter, self.drawing.clone(), self.phase_colors);
            ui.selectable_value(&mut self.mode, Mode::ThreeD, "3D");
            ui.selectable_value(&mut self.mode, Mode::Plan, "Plan");
            if ui.add_enabled(!self.session.doc.views.is_empty(), egui::Button::selectable(self.mode == Mode::Drawing, "Drawing")).clicked() {
                self.mode = Mode::Drawing;
                self.authoring.tab = Tab::Drawings;
                if !self.session.doc.views.iter().any(|view| view.id == self.drawing) {
                    self.drawing = self.session.doc.views.first().map(|v| v.id.clone()).unwrap_or_default();
                }
            }
            let levels = self.levels();
            match self.mode {
                Mode::ThreeD => {
                    let label = self.level.and_then(|l| levels.get(l)).map_or("All levels".to_string(), |n| format!("Up to {n}"));
                    view_combo(ui, "level", &label, 160.0, |ui| {
                        ui.selectable_value(&mut self.level, None, "All levels");
                        for (i, n) in levels.iter().enumerate() {
                            ui.selectable_value(&mut self.level, Some(i), format!("Up to {n}"));
                        }
                    });
                }
                Mode::Drawing => {
                    let label = self.session.doc.views.iter().find(|v| v.id == self.drawing).map_or("Choose drawing", |v| v.name.as_str());
                    view_combo(ui, "saved-drawing", label, 190.0, |ui| {
                        for v in &self.session.doc.views {
                            ui.selectable_value(&mut self.drawing, v.id.clone(), &v.name);
                        }
                    });
                }
                Mode::Plan => {
                    let label = levels.get(self.plan_level).cloned().unwrap_or_else(|| "Level".into());
                    view_combo(ui, "plan-level", &label, 160.0, |ui| {
                        for (i, n) in levels.iter().enumerate() {
                            ui.selectable_value(&mut self.plan_level, i, n);
                        }
                    });
                }
            }
            let phases: Vec<(String, String)> = self.session.doc.phases.iter().map(|p| (p.id.clone(), p.name.clone())).collect();
            if phases.len() > 1 {
                let label = phases.get(self.phase).map_or("Phase".to_string(), |p| p.1.clone());
                view_combo(ui, "phase", &label, 190.0, |ui| {
                    for (i, (_, name)) in phases.iter().enumerate() {
                        ui.selectable_value(&mut self.phase, i, name);
                    }
                });
                view_combo(ui, "filter", filter_name(self.filter), 110.0, |ui| {
                    for f in [PhaseFilter::Complete, PhaseFilter::Demolition, PhaseFilter::New, PhaseFilter::All] {
                        ui.selectable_value(&mut self.filter, f, filter_name(f));
                    }
                });
            }
            if self.mode == Mode::ThreeD {
                view_combo(ui, "appearance", if self.phase_colors { "Phase colors" } else { "Materials" }, 120.0, |ui| {
                    ui.selectable_value(&mut self.phase_colors, false, "Materials");
                    ui.selectable_value(&mut self.phase_colors, true, "Phase colors");
                })
                .on_hover_text("Materials show actual finishes. Phase colors highlight new work. Phase visibility and status are independent.");
            }
            if ui.button("Fit").on_hover_text("Frame the whole model (F)").clicked() {
                self.fit();
            }
            let after = (self.mode, self.level, self.plan_level, self.phase, self.filter, self.drawing.clone(), self.phase_colors);
            if before != after {
                if before.5 != after.5 {
                    self.drafting.cancel_placement();
                }
                self.view_changed(before.3 != after.3 || before.4 != after.4 || before.6 != after.6);
            }
        });
    }

    /// The view settings changed: rebuild what the new view needs.
    pub(crate) fn view_changed(&mut self, phase_changed: bool) {
        if self.mode != Mode::Plan {
            self.model_tools.cancel();
        }
        if self.mode == Mode::Drawing
            && let Some(trigon_model::annotation::DrawingViewKind::Plan { level }) =
                self.session.doc.views.iter().find(|view| view.id == self.drawing).map(|view| &view.kind)
        {
            self.plan_level = *level;
        }
        self.sync_open_views();
        self.properties.gizmo.cancel();
        self.redraw = true;
        if !phase_changed && self.authoring.tab == Tab::Schedules {
            self.authoring.tab = Tab::Project;
        }
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
        egui::containers::Sides::new().shrink_left().truncate().show(
            ui,
            |ui| {
                let path = self.session.path.as_ref().map_or("untitled".into(), |p| p.display().to_string());
                let name =
                    self.session.path.as_ref().and_then(|p| p.file_name()).map_or("Untitled project".into(), |p| p.to_string_lossy().into_owned());
                ui.weak(name).on_hover_text(path);
                if self.session.dirty {
                    ui.label(egui::RichText::new("unsaved").color(egui::Color32::from_rgb(200, 130, 20)));
                }
                if let Some((text, error)) = &self.status {
                    if *error {
                        ui.colored_label(theme::ACCENT, text).on_hover_text(text);
                    } else {
                        ui.weak(text).on_hover_text(text);
                    }
                }
            },
            |ui| {
                ui.label(egui::RichText::new("Model units / mm").size(11.0).color(theme::MUTED));
                ui.separator();
                let timing = self.timings.map_or(String::new(), |(b, s)| format!("\nBuild {b:.0} ms; prepare {s:.0} ms"));
                ui.weak(format!("{} elements", self.scene.elements.len())).on_hover_text(format!(
                    "{} triangles; {} edges{timing}\nControl: {}",
                    self.scene.triangle_count(),
                    self.scene.edge_count(),
                    self.control_note
                ));
            },
        );
    }

    fn edit_block_reason(&self) -> Option<&'static str> {
        if self.model_export.open {
            return Some("Close the export dialog to resume editing.");
        }
        if self.authoring.tab == Tab::Families {
            if self.authoring.family_placement_active() {
                if self.building() || self.build_error.is_some() {
                    return Some("Wait for a valid current model before picking or placing a component.");
                }
                if self.phase != self.session.doc.active_index() || self.filter != PhaseFilter::Complete {
                    return Some("Component placement uses the active phase's complete view.");
                }
                if self.mode != Mode::ThreeD && (self.drawing_plan() || self.plan.error.is_some()) {
                    return Some("Wait for the current drawing before picking or placing a component.");
                }
            }
            return None;
        }
        if self.build_error.is_some() {
            return Some("The model build failed. Correct the model before editing this view.");
        }
        if self.building() || (self.authoring.tab == Tab::Schedules && self.schedules.data().is_none()) {
            return Some("Wait for the current model build before editing.");
        }
        if self.mode != Mode::ThreeD && (self.drawing_plan() || self.plan.error.is_some()) {
            return Some("Wait for a valid drawing of the current model before editing this view.");
        }
        let selected = self.selection.and_then(|i| self.scene.elements.get(i));
        let ambiguous = |id: &str| {
            if let Some(group) = id.strip_prefix("group:") {
                self.session.doc.groups.iter().find(|g| g.id == group).is_some_and(|g| g.targets.iter().any(|id| self.scene.ambiguous(id)))
            } else {
                self.scene.ambiguous(id)
            }
        };
        if self.authoring.selected(selected).iter().any(|id| ambiguous(id)) {
            return Some(schedules::AMBIGUOUS);
        }
        // Semantic edit commands target the document's active phase, not a historical view.
        if (self.authoring.tab == Tab::Schedules || selected.is_some())
            && (self.phase != self.session.doc.active_index()
                || selected.is_some_and(|e| e.props.iter().any(|(k, v)| *k == "phase status" && v == "demolished")))
        {
            return Some("Model edits use the active phase. Use its complete view to edit this element.");
        }
        None
    }

    fn authoring_controls(&mut self, ui: &mut egui::Ui, selected: Option<&crate::scene::Element>) {
        let enabled = self.edit_block_reason().is_none() && !self.precision_review.is_pending();
        let run = ui.add_enabled_ui(enabled, |ui| self.authoring.ui(ui, &self.session.doc, selected, self.plan_level)).inner;
        if let Some(run) = run
            && self.edit_block_reason().is_none()
        {
            self.run_form(run);
        }
    }

    fn run_form(&mut self, run: crate::authoring::Run) {
        let family_placement = matches!(run.command, "facade.place" | "facade.place_at");
        if self.edit_block_reason().is_some() {
            let error = "Wait for the active model view before applying this change.".to_owned();
            if family_placement {
                self.authoring.acknowledge_family_placement(Some(error.clone()));
            }
            self.status = Some((error, true));
            return;
        }
        match self.run(Who::Owner, run.command, run.params) {
            Ok(value) => {
                if family_placement {
                    self.authoring.family_placement_succeeded(&self.session.doc, &value);
                }
                if run.command == "precision.preview" {
                    self.status = Some(("Preview ready · review and apply in Properties".into(), false));
                    return;
                }
                if let Some(targets) = value.get("targets").and_then(Value::as_array) {
                    let ids = targets.iter().filter_map(Value::as_str).collect::<Vec<_>>();
                    self.pending_selection = ids.first().map(|s| s.to_string());
                    if run.command == "facade.place_at" {
                        self.pending_family_targets = Some(ids.iter().map(|id| (*id).to_owned()).collect());
                        self.authoring.targets = ids.join("\n");
                        if let Some(phase) = value["phase"].as_str().and_then(|id| self.session.doc.phases.iter().position(|p| p.id == id)) {
                            let changed = self.phase != phase || self.filter != PhaseFilter::Complete || self.level.is_some();
                            self.phase = phase;
                            self.filter = PhaseFilter::Complete;
                            self.level = None;
                            if changed {
                                self.request_rebuild(None);
                            }
                        }
                    }
                    if !self.authoring.targets.is_empty() {
                        self.authoring.targets = ids.join("\n");
                    }
                }
                if let Some(annotation) = value.get("annotation").and_then(|a| serde_json::from_value(a.clone()).ok()) {
                    self.drafting.select(&annotation);
                }
                self.status = Some((format!("{} completed", run.command), false));
            }
            Err(error) => {
                if family_placement {
                    self.authoring.acknowledge_family_placement(Some(error.clone()));
                }
                self.status = Some((error, true));
            }
        }
    }

    fn selection_panel(&mut self, ui: &mut egui::Ui) {
        if self.model_tools.active() {
            let first_footprint = self.session.doc.buildings.is_empty() && self.model_tools.feedback()["wall_mode"] == "exterior_footprint";
            let blocked = self.creation_block_reason(first_footprint);
            if let Some(reason) = blocked {
                ui.label(egui::RichText::new(reason).color(theme::ACCENT));
            }
            let run = ui.add_enabled_ui(blocked.is_none(), |ui| self.model_tools.properties(ui, &self.session.doc, self.plan_level)).inner;
            if let Some(run) = run {
                self.run_model_tool(run);
            }
            return;
        }
        let blocked = self.edit_block_reason();
        if self.precision_review.is_pending() {
            if let Some(run) = self.precision_review.ui(ui, blocked) {
                self.run_form(run);
            }
            ui.add_space(8.0);
        }
        let blocked =
            self.edit_block_reason().or_else(|| self.precision_review.is_pending().then_some("Apply or cancel the preview to revise the inputs."));
        if self.authoring.tab == Tab::Project {
            match self.inspector {
                Inspector::Entrance => {
                    if !self.entrances.is_selected() {
                        ui.weak("Choose an entrance from the Project Browser.");
                        return;
                    }
                    let run = ui.add_enabled_ui(blocked.is_none(), |ui| self.entrances.ui(ui, &self.session.doc)).inner;
                    if let Some(run) = run {
                        self.run_form(run);
                    }
                    return;
                }
                Inspector::Terrain => {
                    let run = ui.add_enabled_ui(blocked.is_none(), |ui| self.terrain_panel.ui(ui, &self.session.doc)).inner;
                    if let Some(run) = run {
                        self.run_form(run);
                    }
                    return;
                }
                Inspector::Outline => {
                    let level = self.reference_level();
                    let guide = ui.add_enabled_ui(blocked.is_none(), |ui| self.outline_panel.ui(ui, &self.session.doc, level)).inner;
                    if let Some(guide) = guide {
                        self.begin_reference_guide(guide.level, Some(guide.line));
                    }
                    return;
                }
                Inspector::Elements => {}
            }
        }
        if self.authoring.tab == Tab::Drawings {
            if self.mode == Mode::Drawing {
                let run =
                    ui.add_enabled_ui(!self.precision_review.is_pending(), |ui| self.drafting.properties(ui, &self.session.doc, &self.drawing)).inner;
                if let Some(run) = run {
                    self.run_drafting(run);
                }
            } else {
                ui.heading("Drawing properties");
                ui.weak("Open or create a saved drawing in the Drawings browser to add text, dimensions, leaders and levels.");
            }
            return;
        }
        ui.label(egui::RichText::new(if self.authoring.tab == Tab::Families { "Type properties" } else { "Properties" }).strong().size(14.0));
        let selected = self.selection.and_then(|i| self.scene.elements.get(i)).cloned();
        if let Some(reason) = blocked {
            ui.label(egui::RichText::new(reason).color(theme::ACCENT));
        }
        let controls_first = self.authoring.command_controls_first();
        if self.authoring.tab == Tab::Project
            && selected.is_none()
            && self.pending_selection.is_none()
            && self.authoring.targets.trim().is_empty()
            && !controls_first
        {
            self.idle_view_properties(ui);
            return;
        }
        if controls_first {
            self.authoring_controls(ui, selected.as_ref());
            ui.separator();
        }
        if self.authoring.tab != Tab::Families
            && let Some(element) = &selected
        {
            ui.add(egui::Label::new(egui::RichText::new(&element.id).monospace()).wrap());
            let edit = self.properties.ui(ui, &element.id, blocked);
            if let Some(edit) = edit {
                self.apply_property_edit(edit);
            }
            if ui.button("Edit type…").on_hover_text("Edit the shared family type used by this instance").clicked()
                && self.authoring.choose_instance_type(&self.session.doc, element)
            {
                self.dock_open = true;
                self.dock_properties = true;
            }
            ui.horizontal(|ui| {
                if ui.button("Show in 3D").clicked()
                    && let Some(index) = self.selection
                {
                    self.level = None;
                    self.focus(index);
                }
                if let Some(level) = element.level
                    && ui.button("Show plan").clicked()
                {
                    self.mode = Mode::Plan;
                    self.plan_level = level;
                    self.view_changed(false);
                }
            });
            ui.separator();
        }
        if !controls_first {
            self.authoring_controls(ui, selected.as_ref());
        }
        if self.authoring.family_placement_active() && selected.is_some() && ui.button("Inspect selected instance").clicked() {
            self.authoring.cancel_family_placement();
            self.authoring.tab = Tab::Project;
        }
        ui.add_space(6.0);
        ui.separator();
        let Some(e) = selected else {
            return;
        };
        theme::eyebrow(ui, "PICKED ELEMENT");
        egui::Grid::new("selection").num_columns(2).max_col_width(ui.available_width() * 0.55).striped(true).show(ui, |ui| {
            ui.weak("id");
            ui.add(egui::Label::new(egui::RichText::new(&e.id).monospace()).wrap());
            ui.end_row();
            ui.weak("category");
            ui.label(e.kind);
            ui.end_row();
            if let Some(l) = e.level {
                ui.weak("level");
                let building =
                    e.props.iter().find(|(k, _)| *k == "building").and_then(|(_, id)| self.session.doc.buildings.iter().find(|b| b.id == *id));
                ui.label(
                    building
                        .and_then(|b| b.storeys.get(l))
                        .map(|s| s.name.clone())
                        .or_else(|| self.levels().get(l).cloned())
                        .unwrap_or_else(|| l.to_string()),
                );
                ui.end_row();
            }
            for (k, v) in &e.props {
                ui.weak(*k);
                ui.label(v);
                ui.end_row();
            }
        });
    }

    fn idle_view_properties(&mut self, ui: &mut egui::Ui) {
        theme::eyebrow(ui, "View properties");
        let levels = self.levels();
        let name = match self.mode {
            Mode::ThreeD => "3D / Coordination".to_string(),
            Mode::Plan => format!("Floor plan · {}", levels.get(self.plan_level).map_or("New outline", String::as_str)),
            Mode::Drawing => self.session.doc.views.iter().find(|v| v.id == self.drawing).map_or("Drawing".to_string(), |v| v.name.clone()),
        };
        ui.strong(name);
        ui.add_space(8.0);
        ui.label("Select an element in the canvas or Project Browser to edit its properties.");
        ui.separator();
        if let Some(phase) = self.session.doc.phases.get(self.phase) {
            ui.label(format!("Phase · {}", phase.name));
        }
        ui.label(format!("Visibility · {}", filter_name(self.filter)));
        if self.mode == Mode::ThreeD {
            ui.label(if self.phase_colors { "Display · Phase colors" } else { "Display · Materials" });
        }
        if self.mode == Mode::Drawing && ui.button("Drawing settings…").clicked() {
            self.show_drawing_properties();
        }
        if ui.button("Fit view").clicked() {
            self.fit();
        }
    }

    pub(crate) fn authoring_feedback(&self) -> Value {
        json!({"model_tool":self.model_tools.feedback(), "annotation":self.drafting.placement_feedback(),
            "family_placement":self.authoring.family_placement_feedback(&self.session.doc),
            "canvas_focus":self.focus_canvas,"ribbon_expanded":self.ribbon_expanded,
            "dock":if !self.dock_open {"closed"} else if self.dock_properties {"properties"} else {"browser"},
            "empty_workplane":self.viewport_rect.filter(|_|self.session.doc.buildings.is_empty() && self.mode==Mode::Plan).map(|r|json!({"origin_window_points":[r.center().x+self.empty_pan.x,r.center().y+self.empty_pan.y],"mm_per_point":self.empty_mm_per_point,"units":"mm","axes":"x_right_y_up"}))})
    }

    fn begin_model_tool(&mut self, roof: bool) {
        self.drafting.cancel_placement();
        self.inspect_elements();
        self.mode = Mode::Plan;
        self.view_changed(false);
        if roof {
            self.model_tools.start_roof(&self.session.doc, self.plan_level);
        } else {
            self.model_tools.start_wall(&self.session.doc, self.plan_level);
        }
        self.dock_open = true;
        self.dock_properties = true;
        self.ribbon_expanded = false;
    }

    fn creation_block_reason(&self, first_footprint: bool) -> Option<&'static str> {
        if first_footprint {
            if self.building() {
                Some("Wait for the current model build before creating a footprint.")
            } else if self.build_error.is_some() {
                Some("Resolve the model build error before creating a footprint.")
            } else if self.precision_review.is_pending() {
                Some("Apply or cancel the current precision preview first.")
            } else {
                None
            }
        } else {
            self.edit_block_reason()
        }
    }

    fn run_model_tool(&mut self, run: crate::authoring::Run) {
        // An explicit first footprint uses the empty construction plane; there is
        // no existing floor drawing to be ready yet. Model builds and transactions
        // remain guarded, and Session still validates the complete new building.
        let first_footprint = self.session.doc.buildings.is_empty() && run.command == "building.add";
        let reason = self.creation_block_reason(first_footprint);
        let blocked = reason.map(str::to_string).or_else(|| {
            (self.phase != self.session.doc.active_index() || self.filter != PhaseFilter::Complete)
                .then(|| "Use the active phase's complete view to create elements.".to_string())
        });
        if let Some(reason) = blocked {
            self.model_tools.acknowledge(false, Some(reason.clone()));
            self.status = Some((reason, true));
            return;
        }
        match self.run(Who::Owner, run.command, run.params) {
            Ok(_) => {
                self.model_tools.acknowledge(true, None);
                self.status = Some((format!("{} completed", run.command), false));
            }
            Err(error) => {
                self.model_tools.acknowledge(false, Some(error.clone()));
                self.status = Some((error, true));
            }
        }
    }

    pub(crate) fn inspect_elements(&mut self) {
        self.model_tools.cancel();
        self.inspector = Inspector::Elements;
        self.entrances.clear();
        self.precision_review.clear();
        self.properties.gizmo.cancel();
    }

    fn inspect_project(&mut self, inspector: Inspector) {
        if inspector == Inspector::Outline {
            self.outline_panel.start(self.reference_level());
        }
        self.inspect_elements();
        self.authoring.tab = Tab::Project;
        self.view_ribbon = false;
        self.inspector = inspector;
        self.drafting.cancel_placement();
        self.selection = None;
        self.pending_selection = None;
        self.authoring.targets.clear();
        self.selection_dirty = true;
    }

    fn select_entrance(&mut self, id: String) {
        if self.inspector == Inspector::Entrance && self.entrances.selected_id() == Some(&id) {
            return;
        }
        self.inspect_project(Inspector::Entrance);
        self.entrances.select(id);
    }

    fn reference_level(&self) -> Option<usize> {
        match self.mode {
            Mode::Plan => Some(self.plan_level),
            Mode::ThreeD => self.level,
            Mode::Drawing => self.session.doc.views.iter().find(|v| v.id == self.drawing).and_then(|v| match v.kind {
                trigon_model::annotation::DrawingViewKind::Plan { level } => Some(level),
                _ => None,
            }),
        }
    }

    fn begin_reference_guide(&mut self, level: usize, line: Option<[[f64; 2]; 2]>) {
        if let Some(reason) = self.edit_block_reason() {
            self.status = Some((reason.into(), true));
            return;
        }
        if !self.session.doc.buildings.iter().any(|b| b.storeys.get(level).is_some()) {
            self.status = Some(("Choose a real floor before creating a plan guide.".into(), true));
            return;
        }
        let stem = format!("coordination-L{level}");
        let id = (0..256).map(|i| if i == 0 { stem.clone() } else { format!("{stem}-{i}") }).find(|id| {
            !self.session.doc.views.iter().any(|v| &v.id == id)
                || self
                    .session
                    .doc
                    .views
                    .iter()
                    .any(|v| &v.id == id && matches!(v.kind, trigon_model::annotation::DrawingViewKind::Plan { level: l } if l == level))
        });
        let Some(id) = id else {
            self.status = Some(("No free coordination view ID is available.".into(), true));
            return;
        };
        if !self.session.doc.views.iter().any(|v| v.id == id) {
            let params = json!({"id":id,"name":format!("Coordination · {}",self.levels().get(level).map_or("Floor", String::as_str)),"kind":{"view":"plan","level":level}});
            if let Err(error) = self.run(Who::Owner, "view.define", params) {
                self.status = Some((error, true));
                return;
            }
        }
        self.inspect_elements();
        self.open_drawing(id.clone());
        if self.drafting.reference_guide(&self.session.doc, &id, line) {
            self.status = Some(("Reference guide · set exact endpoints, then Preview guide".into(), false));
        }
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
        ui.label(
            egui::RichText::new(match self.authoring.tab {
                Tab::Project => "Project Browser",
                Tab::Families => "FAMILIES / COMPONENTS",
                Tab::Drawings => "VIEWS / DRAWINGS",
                Tab::Schedules => "SCHEDULES",
            })
            .strong()
            .size(13.0),
        );
        ui.separator();
        if self.authoring.tab == Tab::Schedules {
            self.schedules.browser(ui, &self.session.doc);
            return;
        }
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
            if let Some(run) = self.authoring.library(ui, &self.session.doc) {
                let _ = self.run(Who::Owner, run.command, run.params);
            }
            if ui.button("Type properties…").clicked() {
                self.dock_properties = true;
            }
            return;
        }
        ui.add(egui::TextEdit::singleline(&mut self.browser_query).char_limit(256).hint_text("Search project…").desired_width(f32::INFINITY));
        let query = self.browser_query.trim().to_lowercase();
        let project = self.session.doc.buildings.first().map_or("Untitled project", |b| b.name.as_str());
        ui.add(egui::Label::new(egui::RichText::new(project).color(theme::MUTED)).wrap());
        let before = (self.mode, self.level, self.plan_level);
        ui.add_space(8.0);
        if ui.selectable_label(self.mode == Mode::ThreeD && self.level.is_none(), "3D model").clicked() {
            self.mode = Mode::ThreeD;
            self.level = None;
        }
        let levels = self.levels();
        egui::CollapsingHeader::new("Floor plans").default_open(true).show(ui, |ui| {
            for (i, name) in levels.iter().enumerate().filter(|(_, name)| name.to_lowercase().contains(&query)) {
                if ui.selectable_label(self.mode == Mode::Plan && self.plan_level == i, name).clicked() {
                    self.mode = Mode::Plan;
                    self.plan_level = i;
                }
            }
        });
        egui::CollapsingHeader::new("3D cut views").show(ui, |ui| {
            for (i, name) in levels.iter().enumerate().filter(|(_, name)| name.to_lowercase().contains(&query)) {
                if ui.selectable_label(self.mode == Mode::ThreeD && self.level == Some(i), format!("{name} · cut")).clicked() {
                    self.mode = Mode::ThreeD;
                    self.level = Some(i);
                    self.plan_level = i;
                }
            }
        });
        let mut project_inspector = None;
        egui::CollapsingHeader::new("Set out & site").show(ui, |ui| {
            for (inspector, label) in [(Inspector::Outline, "Building outline / reference guides"), (Inspector::Terrain, "Terrain / survey context")]
            {
                if ui.add_sized([ui.available_width(), 28.0], egui::Button::selectable(self.inspector == inspector, label).truncate()).clicked() {
                    project_inspector = Some(inspector);
                }
            }
        });
        if let Some(inspector) = project_inspector {
            self.inspect_project(inspector);
        }
        let entrances = self
            .session
            .doc
            .buildings
            .iter()
            .flat_map(|b| {
                b.entrances
                    .iter()
                    .enumerate()
                    .map(|(index, e)| (format!("{}.entrance{index}", b.id), e.number.clone(), e.street.clone(), e.units.len()))
            })
            .collect::<Vec<_>>();
        let mut entrance = None;
        egui::CollapsingHeader::new(format!("Entrances / apartments ({})", entrances.len())).show(ui, |ui| {
            for (id, number, street, count) in
                entrances.iter().filter(|(id, number, street, _)| format!("{id} {number} {street}").to_lowercase().contains(&query))
            {
                let selected = self.inspector == Inspector::Entrance && self.entrances.selected_id() == Some(id);
                if ui
                    .add_sized([ui.available_width(), 28.0], egui::Button::selectable(selected, format!("{number} · {street}")).truncate())
                    .on_hover_text(format!("{street} {number} · {count} unit records"))
                    .clicked()
                {
                    entrance = Some(id.clone());
                }
            }
            if entrances.is_empty() {
                ui.weak("No stored entrance records in this project.");
            }
        });
        if let Some(id) = entrance {
            self.select_entrance(id);
        }
        let guides = self
            .session
            .doc
            .annotations
            .iter()
            .filter(|a| {
                matches!(a.kind, trigon_model::annotation::AnnotationKind::Line { .. })
                    && self
                        .session
                        .doc
                        .views
                        .iter()
                        .any(|v| v.id == a.view && matches!(v.kind, trigon_model::annotation::DrawingViewKind::Plan { .. }))
            })
            .map(|a| (a.id.clone(), a.view.clone()))
            .collect::<Vec<_>>();
        let mut open_guide = None;
        egui::CollapsingHeader::new(format!("Plan lines / guides ({})", guides.len())).show(ui, |ui| {
            for (id, view) in guides.iter().filter(|(id, view)| format!("{id} {view}").to_lowercase().contains(&query)) {
                if ui.add_sized([ui.available_width(), 28.0], egui::Button::selectable(false, id).truncate()).on_hover_text(view).clicked() {
                    open_guide = Some((id.clone(), view.clone()));
                }
            }
        });
        if let Some((id, view)) = open_guide {
            self.inspect_elements();
            self.open_drawing(view);
            if let Some(annotation) = self.session.doc.annotations.iter().find(|a| a.id == id) {
                self.drafting.select(annotation);
            }
            return;
        }
        let facades = self.session.doc.buildings.iter().flat_map(|b| b.facades.iter().map(|f| (f.id.clone(), f.kind))).collect::<Vec<_>>();
        let mut open_facade = None;
        egui::CollapsingHeader::new("Facade elevations").show(ui, |ui| {
            for (id, kind) in facades.into_iter().filter(|(id, _)| id.to_lowercase().contains(&query)) {
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
        for (id, name, n) in groups.into_iter().filter(|(id, name, _)| format!("{id} {name}").to_lowercase().contains(&query)) {
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
            .filter(|(_, e)| !e.host && e.level == Some(self.plan_level) && format!("{} {}", e.id, e.kind).to_lowercase().contains(&query))
            .map(|(i, e)| (i, e.id.clone(), e.kind))
            .collect::<Vec<_>>();
        for (i, id, kind) in elements {
            let response = ui.selectable_label(self.selection == Some(i), &id).on_hover_text(kind);
            if response.clicked() {
                self.inspect_elements();
                self.pending_selection = None;
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

    fn schedule_action(&mut self, action: schedules::Action) {
        if self.building() {
            return;
        }
        let (schedules::Action::Select(index) | schedules::Action::Show(index)) = action;
        let Some(_) = self.schedules.data().and_then(|s| s.rows.iter().find(|r| schedules::resolve_row(&self.scene, r) == Ok(index))) else {
            return;
        };
        self.selection = Some(index);
        self.inspect_elements();
        self.pending_selection = None;
        self.selection_dirty = true;
        self.redraw = true;
        self.authoring.targets.clear();
        if matches!(action, schedules::Action::Show(_)) {
            self.level = None;
            self.focus(index);
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
        if let Some(trigon_model::annotation::DrawingViewKind::Plan { level }) = self.session.doc.views.iter().find(|v| v.id == id).map(|v| &v.kind) {
            self.plan_level = *level;
        }
        self.mode = Mode::Drawing;
        self.drawing = id;
        self.authoring.tab = Tab::Drawings;
        self.drafting.opened(&self.session.doc, &self.drawing);
        self.view_changed(false);
    }

    fn run_drafting(&mut self, mut run: crate::authoring::Run) {
        if run.command == "annotation.define" && self.drafting.guide_active() {
            self.run_form(crate::authoring::Run {
                command: "precision.preview",
                params: json!({"action":{"command":run.command,"params":run.params}}),
            });
            return;
        }
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
                    self.drafting.annotation_applied(&self.session.doc, &a);
                }
                self.status = Some((format!("{} completed", run.command), false));
            }
            Err(error) => {
                self.drafting.placement_failed(&error);
                self.status = Some((error, true));
            }
        }
    }

    fn apply_property_edit(&mut self, edit: properties::Edit) {
        if self.edit_block_reason().is_some()
            || self.selection.and_then(|index| self.scene.elements.get(index)).is_none_or(|e| e.id != edit.id)
            || self.scene.find_unique(&edit.id) != self.selection
            || !self.properties.descriptor(&edit.id).is_some_and(|d| d["editable"] == true)
        {
            self.properties.gizmo.cancel();
            return;
        }
        let phase = self.session.doc.phases.get(self.phase).map(|p| p.id.clone());
        let params = json!({"id":edit.id,"phase":phase,"filter":filter_name(self.filter),"properties":edit.properties});
        match self.run(Who::Owner, "element.set", params) {
            Ok(result) => {
                self.pending_selection = result["targets"].as_array().and_then(|targets| targets.first()).and_then(Value::as_str).map(str::to_string);
                self.status = Some(("Model properties updated".into(), false));
            }
            Err(error) => self.status = Some((error, true)),
        }
    }

    fn drawing_point(&self, point: [f64; 3]) -> Option<[f64; 2]> {
        if self.mode == Mode::Plan {
            return Some([point[0], point[1]]);
        }
        let view = self.session.doc.views.iter().find(|view| view.id == self.drawing)?;
        match &view.kind {
            DrawingViewKind::Plan { .. } => Some([point[0], point[1]]),
            DrawingViewKind::Facade { facade } => {
                let building = self.session.doc.buildings.iter().find(|b| b.facades.iter().any(|f| &f.id == facade))?;
                let f = building.facade(facade).ok()?;
                let (a, b) = building.edge(f.edge)?;
                let frame = trigon_model::FacadeFrame::new(a, b)?;
                Some([(point[0] - a[0]) * frame.t[0] + (point[1] - a[1]) * frame.t[1], point[2]])
            }
            DrawingViewKind::Section { from, to, flip, .. } => {
                let (a, b) = if *flip { (*to, *from) } else { (*from, *to) };
                let length = (b[0] - a[0]).hypot(b[1] - a[1]);
                (length > 1e-9).then(|| [((point[0] - a[0]) * (b[0] - a[0]) + (point[1] - a[1]) * (b[1] - a[1])) / length, point[2]])
            }
        }
    }

    pub(crate) fn projected_handles(&self, rect: egui::Rect) -> Vec<ProjectedHandle> {
        if self.precision_review.is_pending()
            || self.inspector != Inspector::Elements
            || self.authoring.tab == Tab::Drawings
            || self.authoring.tab == Tab::Families
            || self.authoring.wall_alignment_active()
            || self.edit_block_reason().is_some()
        {
            return Vec::new();
        }
        let Some(element) = self.selection.and_then(|index| self.scene.elements.get(index)) else { return Vec::new() };
        self.properties
            .handles(&element.id)
            .into_iter()
            .filter_map(|handle| {
                let mapping = if self.mode == Mode::ThreeD {
                    let at = std::array::from_fn(|axis| ((handle.position[axis] - self.scene.origin[axis]) / 1000.0) as f32);
                    Mapping::Spatial { camera: self.camera, rect, at, direction: handle.direction.map(|v| v as f32) }
                } else {
                    let start = self.drawing_point(handle.position)?;
                    let end = self.drawing_point(std::array::from_fn(|axis| handle.position[axis] + handle.direction[axis] * 1000.0))?;
                    let at = self.plan.model_to_screen(rect, start)?;
                    let to = self.plan.model_to_screen(rect, end)?;
                    let per_mm = (to - at) / 1000.0;
                    if per_mm.length_sq() < 1e-10 {
                        return None;
                    }
                    Mapping::Drawing { at, per_mm }
                };
                let at = mapping.screen(0.0)?;
                mapping.parameter(at)?;
                Some(ProjectedHandle { handle, mapping })
            })
            .collect()
    }

    fn handle_hover(&self, ui: &egui::Ui, rect: egui::Rect) -> bool {
        if self.properties.gizmo.active() {
            return true;
        }
        let (hover, press) = ui.input(|i| (i.pointer.hover_pos(), i.pointer.primary_down().then(|| i.pointer.press_origin()).flatten()));
        self.projected_handles(rect).iter().any(|handle| {
            handle.mapping.screen(0.0).is_some_and(|at| rect.contains(at) && [hover, press].into_iter().flatten().any(|p| p.distance(at) <= 12.0))
        })
    }

    fn model_handles_ui(&mut self, ui: &mut egui::Ui, rect: egui::Rect) -> bool {
        let Some(id) = self.selection.and_then(|index| self.scene.elements.get(index)).map(|e| e.id.clone()) else {
            self.properties.gizmo.cancel();
            return false;
        };
        let handles = self.projected_handles(rect);
        let enabled = !handles.is_empty() && self.edit_block_reason().is_none() && self.scene.find_unique(&id) == self.selection;
        let (busy, edit) = self.properties.gizmo.ui(ui, rect, &id, self.scene_generation, &handles, enabled);
        if let Some(edit) = edit {
            self.apply_property_edit(edit);
        }
        busy
    }

    fn annotation_canvas_editor(&mut self, ctx: &egui::Context, keyboard_owned: bool) {
        if !self.drafting.placement_active() || self.mode != Mode::Drawing || self.palette.open || self.confirm_close || self.model_export.open {
            return;
        }
        let Some(rect) = self.viewport_rect else { return };
        let mut run = None;
        egui::Area::new(egui::Id::new("annotation-canvas-editor"))
            .order(egui::Order::Foreground)
            .fixed_pos(rect.left_top() + egui::vec2(12.0, 12.0))
            .constrain_to(rect)
            .show(ctx, |ui| {
                egui::Frame::popup(ui.style()).show(ui, |ui| {
                    ui.set_width((rect.width() - 32.0).clamp(220.0, 360.0));
                    run = self.drafting.canvas_editor(ui, &self.session.doc, &self.drawing, keyboard_owned);
                });
            });
        if let Some(run) = run {
            self.run_drafting(run);
        }
    }

    fn empty_model_canvas(&mut self, ui: &mut egui::Ui) {
        let rect = ui.available_rect_before_wrap();
        let response = ui.allocate_rect(rect, egui::Sense::click_and_drag());
        self.viewport_rect = Some(rect);
        let painter = ui.painter_at(rect);
        painter.rect_filled(rect, 0.0, theme::PAPER);
        if response.dragged_by(egui::PointerButton::Middle) {
            self.empty_pan += response.drag_delta();
        }
        if response.hovered() {
            let scroll = ui.input(|i| i.smooth_scroll_delta.y);
            if scroll != 0.0 {
                self.empty_mm_per_point = (self.empty_mm_per_point * (-f64::from(scroll) * 0.002).exp()).clamp(0.1, 2000.0);
            }
        }
        let origin = rect.center() + self.empty_pan;
        let mut step = 1000.0 / self.empty_mm_per_point as f32;
        while step < 30.0 {
            step *= 10.0;
        }
        while step > 300.0 {
            step /= 10.0;
        }
        let stroke = egui::Stroke::new(0.5, egui::Color32::from_gray(220));
        let mut x = rect.left() + (origin.x - rect.left()).rem_euclid(step);
        for _ in 0..100 {
            if x > rect.right() {
                break;
            }
            painter.line_segment([egui::pos2(x, rect.top()), egui::pos2(x, rect.bottom())], stroke);
            x += step;
        }
        let mut y = rect.top() + (origin.y - rect.top()).rem_euclid(step);
        for _ in 0..100 {
            if y > rect.bottom() {
                break;
            }
            painter.line_segment([egui::pos2(rect.left(), y), egui::pos2(rect.right(), y)], stroke);
            y += step;
        }
        painter.line_segment([origin - egui::vec2(12.0, 0.0), origin + egui::vec2(12.0, 0.0)], egui::Stroke::new(1.0, theme::ACCENT));
        painter.line_segment([origin - egui::vec2(0.0, 12.0), origin + egui::vec2(0.0, 12.0)], egui::Stroke::new(1.0, theme::ACCENT));
        if self.model_tools.active() {
            self.model_canvas_gesture(ui, &response, rect, true);
        } else {
            painter.text(
                rect.center(),
                egui::Align2::CENTER_CENTER,
                "Choose Wall to draw a building outline",
                egui::FontId::proportional(16.0),
                theme::DRAWING_INK,
            );
        }
    }

    fn model_canvas_gesture(&mut self, ui: &egui::Ui, response: &egui::Response, rect: egui::Rect, empty: bool) {
        let painter = ui.painter_at(rect);
        let bypass = ui.input(|input| input.modifiers.alt);
        let raw_point = response.hover_pos().and_then(|at| {
            if empty {
                let origin = rect.center() + self.empty_pan;
                let raw = [f64::from(at.x - origin.x) * self.empty_mm_per_point, -f64::from(at.y - origin.y) * self.empty_mm_per_point];
                let grid = (!bypass).then(|| self.model_tools.snap_options().grid_mm).flatten();
                Some(grid.filter(|g| g.is_finite() && *g > 0.0).map_or(raw, |g| [(raw[0] / g).round() * g, (raw[1] / g).round() * g]))
            } else {
                self.plan.aligned_cursor(rect, at, (!bypass).then(|| self.model_tools.snap_options()), None).map(|(point, _)| point)
            }
        });
        let point = raw_point.map(|p| self.model_tools.constrained(p));
        let screen = |p: [f64; 2]| -> Option<egui::Pos2> {
            if empty {
                Some(rect.center() + self.empty_pan + egui::vec2((p[0] / self.empty_mm_per_point) as f32, (-p[1] / self.empty_mm_per_point) as f32))
            } else {
                self.plan.model_to_screen(rect, p)
            }
        };
        for [a, b] in self.model_tools.preview(point) {
            if let (Some(a), Some(b)) = (screen(a), screen(b)) {
                painter.line_segment([a, b], egui::Stroke::new(1.5, theme::ACCENT));
            }
        }
        if let Some(point) = point {
            response.clone().on_hover_cursor(egui::CursorIcon::Crosshair);
            if let Some(at) = screen(point) {
                painter.circle_stroke(at, 5.0, egui::Stroke::new(1.5, theme::ACCENT));
                let length = self.model_tools.anchor().map(|a| (point[0] - a[0]).hypot(point[1] - a[1]));
                let label = length.map_or_else(|| format!("{:.0}, {:.0} mm", point[0], point[1]), |l| format!("{l:.1} mm"));
                painter.text(at + egui::vec2(12.0, -12.0), egui::Align2::LEFT_BOTTOM, label, egui::FontId::proportional(12.0), theme::DRAWING_INK);
            }
            if response.clicked()
                && let Some(raw_point) = raw_point
                && let Some(run) = self.model_tools.point(&self.session.doc, raw_point)
            {
                self.run_model_tool(run);
            }
        }
        if response.secondary_clicked() {
            self.model_tools.end_chain();
        }
        let message = self.model_tools.error().unwrap_or_else(|| self.model_tools.status());
        painter.text(
            rect.left_bottom() + egui::vec2(12.0, -14.0),
            egui::Align2::LEFT_BOTTOM,
            message,
            egui::FontId::proportional(12.0),
            theme::DRAWING_INK,
        );
    }

    fn sync_family_view(&mut self) {
        let view_id = (self.mode == Mode::Drawing).then_some(self.drawing.as_str());
        self.authoring.sync_family_plan(&self.session.doc, view_id);
    }

    fn family_pick_status(&mut self, accepted: bool) {
        self.dock_open = true;
        self.dock_properties = true;
        self.status = Some((
            if accepted {
                "Point captured · review dimensions, then Place component"
            } else {
                "Choose a host and current storey in Properties (or open its saved plan), then click a facade wall or component."
            }
            .into(),
            !accepted,
        ));
    }

    fn family_preview_ui(&self, ui: &egui::Ui, rect: egui::Rect) {
        if let Some(reason) = self.edit_block_reason() {
            ui.painter_at(rect).text(
                rect.left_top() + egui::vec2(12.0, 12.0),
                egui::Align2::LEFT_TOP,
                reason,
                egui::FontId::proportional(13.0),
                theme::ACCENT,
            );
            return;
        }
        let Some(preview) = self.authoring.family_preview(&self.session.doc) else { return };
        let plan_level = if self.mode == Mode::Plan {
            Some(self.plan_level)
        } else if self.mode == Mode::Drawing {
            self.session.doc.views.iter().find(|v| v.id == self.drawing).and_then(|v| match v.kind {
                DrawingViewKind::Plan { level } => Some(level),
                _ => None,
            })
        } else {
            None
        };
        let painter = ui.painter_at(rect);
        if plan_level.is_some_and(|level| level != preview.level) {
            painter.text(
                rect.left_top() + egui::vec2(12.0, 12.0),
                egui::Align2::LEFT_TOP,
                format!("Placement is on L{} · open that plan to review its preview", preview.level),
                egui::FontId::proportional(13.0),
                theme::ACCENT,
            );
            return;
        }
        if self.mode == Mode::Drawing && plan_level.is_none() {
            let matching = self
                .session
                .doc
                .views
                .iter()
                .find(|v| v.id == self.drawing)
                .is_some_and(|v| matches!(&v.kind, DrawingViewKind::Facade { facade } if facade == &preview.facade));
            if !matching {
                return;
            }
        }
        let project = |p: [f64; 3]| {
            if self.mode == Mode::ThreeD {
                let at = std::array::from_fn(|axis| ((p[axis] - self.scene.origin[axis]) / 1000.0) as f32);
                Mapping::Spatial { camera: self.camera, rect, at, direction: [1.0, 0.0, 0.0] }.screen(0.0)
            } else {
                self.drawing_point(p).and_then(|p| self.plan.model_to_screen(rect, p))
            }
        };
        let corners = if plan_level.is_some() { preview.plan } else { preview.face };
        let Some(points) = corners.into_iter().map(project).collect::<Option<Vec<_>>>() else { return };
        let color = if preview.captured { theme::ACCENT } else { egui::Color32::from_rgb(35, 130, 190) };
        painter.add(egui::Shape::convex_polygon(points.clone(), color.gamma_multiply(0.15), egui::Stroke::new(2.0, color)));
        let axis = if plan_level.is_some() {
            [
                std::array::from_fn(|i| (preview.plan[0][i] + preview.plan[1][i]) / 2.0),
                std::array::from_fn(|i| (preview.plan[2][i] + preview.plan[3][i]) / 2.0),
            ]
        } else {
            preview.axis
        };
        if let (Some(a), Some(b)) = (project(axis[0]), project(axis[1])) {
            painter.line_segment([a, b], egui::Stroke::new(1.0, color));
            painter.circle_stroke(a, 4.0, egui::Stroke::new(1.5, color));
        }
        let text = format!(
            "{} · L{} · u {} mm\nW {} × H {} · sill {} mm\n{} · depth {} mm\n{} · {}",
            if preview.captured { "Captured" } else { "Click to capture" },
            preview.level,
            preview.u_mm,
            preview.sizes[0],
            preview.sizes[1],
            preview.sizes[2],
            if preview.opening { "Opening" } else { "Attachment size" },
            preview.depth_mm,
            preview.guide,
            if preview.captured { "Place in Properties" } else { "Alt: exact" }
        );
        let galley = painter.layout_no_wrap(text, egui::FontId::proportional(12.0), theme::INK);
        let anchor = points.first().copied().unwrap_or(rect.left_top()) + egui::vec2(8.0, 8.0);
        let at = egui::pos2(
            anchor.x.clamp(rect.left() + 6.0, (rect.right() - galley.size().x - 12.0).max(rect.left() + 6.0)),
            anchor.y.clamp(rect.top() + 6.0, (rect.bottom() - galley.size().y - 12.0).max(rect.top() + 6.0)),
        );
        painter.rect_filled(egui::Rect::from_min_size(at - egui::vec2(4.0, 4.0), galley.size() + egui::vec2(8.0, 8.0)), 3.0, theme::PANEL);
        painter.galley(at, galley, theme::INK);
    }

    fn family_site_point(&self, host: &crate::scene::Element, point: [f64; 2]) -> Option<[f64; 2]> {
        if point.iter().any(|value| !value.is_finite()) {
            return None;
        }
        if self.mode == Mode::Plan {
            return Some(point);
        }
        let view = self.session.doc.views.iter().find(|view| view.id == self.drawing)?;
        match &view.kind {
            DrawingViewKind::Plan { .. } => Some(point),
            DrawingViewKind::Facade { facade } => {
                let host_facade = if host.host && host.kind == "wall" {
                    Some(host.id.as_str())
                } else {
                    host.props.iter().find(|(key, _)| *key == "facade").map(|(_, value)| value.as_str())
                }?;
                if host_facade != facade {
                    return None;
                }
                let mut matches = self
                    .session
                    .doc
                    .buildings
                    .iter()
                    .flat_map(|building| building.facades.iter().filter(|f| &f.id == facade).map(move |f| (building, f)));
                let (building, source) = matches.next()?;
                if matches.next().is_some() {
                    return None;
                }
                let (a, b) = building.edge(source.edge)?;
                let frame = trigon_model::FacadeFrame::new(a, b)?;
                Some([a[0] + frame.t[0] * point[0], a[1] + frame.t[1] * point[0]])
            }
            // A section projects surfaces at different depths onto one plane. That
            // projection cannot identify the picked facade's source point uniquely.
            DrawingViewKind::Section { .. } => None,
        }
    }

    fn viewport_drawing(&mut self, ui: &mut egui::Ui) {
        if self.session.doc.buildings.is_empty() {
            self.empty_model_canvas(ui);
            return;
        }
        if self.plan_wanted == 0 {
            self.request_plan();
        }
        let picking =
            self.model_tools.picking() || (self.mode == Mode::Drawing && self.authoring.tab == Tab::Drawings && self.drafting.picking.is_some());
        let handle_capture = self.viewport_rect.is_some_and(|rect| self.handle_hover(ui, rect));
        let response = self.plan.ui(ui, picking || handle_capture);
        let rect = response.rect;
        self.viewport_rect = Some(rect);
        if self.drawing_plan() || self.plan.error.is_some() || !self.plan.ready(rect, self.ppp) {
            return;
        }
        if self.model_tools.active() {
            self.model_canvas_gesture(ui, &response, rect, false);
            return;
        }
        if self.authoring.family_placement_active() {
            self.sync_family_view();
            if self.edit_block_reason().is_some() {
                self.authoring.hover_family_host(&self.session.doc, None, false);
                self.family_preview_ui(ui, rect);
                return;
            }
            response.clone().on_hover_cursor(egui::CursorIcon::Crosshair);
            let hit = response.hover_pos().and_then(|at| {
                let host = self
                    .plan
                    .pick_element(rect, at)
                    .and_then(|hit| self.scene.find_unique(&hit.id))
                    .and_then(|i| self.scene.elements.get(i))?
                    .clone();
                let point = self.plan.screen_to_model(rect, at).and_then(|point| self.family_site_point(&host, point))?;
                Some((host, point))
            });
            let alt = ui.input(|i| i.modifiers.alt);
            self.authoring.hover_family_host(&self.session.doc, hit.as_ref().map(|(host, point)| (host, *point)), alt);
            if response.clicked() {
                let accepted =
                    hit.as_ref().is_some_and(|(host, point)| self.authoring.pick_family_host_snapped(&self.session.doc, host, Some(*point), alt));
                self.family_pick_status(accepted);
            }
            self.family_preview_ui(ui, rect);
            return;
        }
        let handles_busy = self.model_handles_ui(ui, rect);
        if !picking
            && !handles_busy
            && response.clicked()
            && let Some(p) = response.interact_pointer_pos()
        {
            let annotation = (self.mode == Mode::Drawing && self.authoring.tab == Tab::Drawings)
                .then(|| self.plan.screen_to_model(rect, p))
                .flatten()
                .and_then(|point| Drafting::hit(&self.session.doc, &self.drawing, point, self.plan.tolerance_mm().unwrap_or(0.0)))
                .cloned();
            self.selection = None;
            self.pending_selection = None;
            self.inspect_elements();
            if let Some(annotation) = annotation {
                self.drafting.select(&annotation);
            } else {
                self.drafting.new_note(&self.session.doc);
                if let Some(hit) = self.plan.pick_element(rect, p) {
                    self.authoring.targets.clear();
                    self.pending_selection = Some(hit.id.clone());
                    self.selection = self.scene.find_unique(&hit.id);
                    if self.selection.is_some() {
                        self.authoring.tab = Tab::Project;
                    }
                    self.status = Some((format!("Selected {}", hit.id), false));
                }
            }
            self.selection_dirty = true;
            self.redraw = true;
        }
        if let Some(id) =
            self.pending_selection.clone().or_else(|| self.selection.and_then(|i| self.scene.elements.get(i).map(|element| element.id.clone())))
        {
            self.plan.highlight_element(&ui.painter_at(rect), rect, &id);
        }
        if self.mode == Mode::Drawing && self.authoring.tab == Tab::Drawings {
            let painter = ui.painter_at(rect);
            let snap = (!ui.input(|i| i.modifiers.alt)).then(|| self.drafting.options());
            let settings = self.drafting.alignment_options(&self.session.doc, &self.drawing);
            let alignment = (picking && self.drafting.alignment).then_some(&settings);
            let cursor = response.hover_pos().and_then(|at| self.plan.aligned_cursor(rect, at, snap, alignment));
            if picking || self.drafting.selected.is_some() {
                let point = cursor.as_ref().map(|(point, _)| *point);
                let preview = self.drafting.preview_points(point);
                for p in &preview {
                    if let Some(at) = self.plan.model_to_screen(rect, *p) {
                        painter.circle_stroke(at, 4.0, egui::Stroke::new(1.5, theme::ACCENT));
                    }
                }
                for [a, b] in self.drafting.preview_segments(point) {
                    if let (Some(a), Some(b)) = (self.plan.model_to_screen(rect, a), self.plan.model_to_screen(rect, b)) {
                        painter.line_segment([a, b], egui::Stroke::new(1.5, theme::ACCENT));
                    }
                }
            }
            if let Some((point, aligned)) = cursor {
                let snapped = aligned.as_ref().map(|a| a.snap);
                if picking && let Some(aligned) = &aligned {
                    for guide in &aligned.guides {
                        if let (Some(a), Some(b)) = (self.plan.model_to_screen(rect, guide[0]), self.plan.model_to_screen(rect, guide[1])) {
                            painter.line_segment([a, b], egui::Stroke::new(0.8, theme::ACCENT.gamma_multiply(0.6)));
                        }
                    }
                }
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
                    if response.clicked() {
                        self.drafting.picked(point);
                        let state = self.drafting.placement_feedback();
                        if self.drafting.placement_ready()
                            && matches!(state["kind"].as_str(), Some("dimension" | "line"))
                            && let Some(run) = self.drafting.finish_placement(&self.session.doc, &self.drawing)
                        {
                            self.run_drafting(run);
                        }
                    }
                }
                let text =
                    format!("x {:.3}  ·  y {:.3} mm{}", point[0], point[1], snapped.map_or(String::new(), |s| format!("  ·  {}", s.kind.name())));
                let at = rect.left_bottom() + egui::vec2(12.0, -12.0);
                painter.text(at, egui::Align2::LEFT_BOTTOM, text, egui::FontId::monospace(12.0), theme::DRAWING_INK);
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
        let handle_capture = self.handle_hover(ui, rect);
        // Orbit with the left button; pan with the right or middle button, or Shift.
        let shift = ui.input(|i| i.modifiers.shift);
        let d = resp.drag_delta();
        if d != egui::Vec2::ZERO && !handle_capture {
            if resp.dragged_by(egui::PointerButton::Primary) && !shift {
                self.camera.orbit(d.x * 0.008, d.y * 0.008);
            } else {
                self.camera.pan(d.x, d.y, rect.height());
            }
            self.redraw = true;
        }
        if resp.hovered() && !handle_capture {
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
        if resp.double_clicked() && !handle_capture {
            self.fit();
        } else if resp.clicked()
            && !handle_capture
            && (!self.authoring.family_placement_active() || self.edit_block_reason().is_none())
            && let Some(p) = resp.interact_pointer_pos()
        {
            let ndc = [(p.x - rect.left()) / rect.width() * 2.0 - 1.0, 1.0 - (p.y - rect.top()) / rect.height() * 2.0];
            let (o, dir) = self.camera.ray(ndc, aspect);
            self.inspect_elements();
            self.pending_selection = None;
            let placing = self.authoring.family_placement_active();
            self.sync_family_view();
            let hit = placing.then(|| self.scene.pick_hit(o, dir, self.clip_z())).flatten();
            self.selection = if placing { hit.map(|(index, _)| index) } else { self.scene.pick(o, dir, self.clip_z()) };
            if placing {
                if let Some((index, point)) = hit
                    && let Some(selected) = self.scene.elements.get(index)
                    && self.authoring.pick_family_host_snapped(&self.session.doc, selected, Some([point[0], point[1]]), ui.input(|i| i.modifiers.alt))
                {
                    self.family_pick_status(true);
                } else {
                    self.family_pick_status(false);
                }
            }
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
        if self.authoring.family_placement_active() {
            self.sync_family_view();
            let hit = resp.hover_pos().filter(|_| self.edit_block_reason().is_none()).and_then(|p| {
                let ndc = [(p.x - rect.left()) / rect.width() * 2.0 - 1.0, 1.0 - (p.y - rect.top()) / rect.height() * 2.0];
                let (origin, direction) = self.camera.ray(ndc, aspect);
                let (index, point) = self.scene.pick_hit(origin, direction, self.clip_z())?;
                Some((self.scene.elements.get(index)?.clone(), [point[0], point[1]]))
            });
            self.authoring.hover_family_host(&self.session.doc, hit.as_ref().map(|(host, point)| (host, *point)), ui.input(|i| i.modifiers.alt));
            self.family_preview_ui(ui, rect);
        }
        self.model_handles_ui(ui, rect);
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
        if ctx.input(|i| i.viewport().close_requested()) && self.model_export.busy() {
            ctx.send_viewport_cmd(egui::ViewportCommand::CancelClose);
            self.model_export.open = true;
            self.status = Some(("An export is running. Wait for it to finish before closing Trigon Studio.".into(), false));
            return;
        }
        if ctx.input(|i| i.viewport().close_requested()) {
            self.model_export.open = false;
        }
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

impl StudioApp {
    fn workspace_ui(&mut self, ui: &mut egui::Ui) {
        let ctx = ui.ctx().clone();
        let annotation_keyboard_owned = self.drafting.keyboard_owned(&ctx) && !egui::Popup::is_any_open(&ctx);
        self.ppp = ctx.pixels_per_point();
        self.collect_window_shots(&ctx);
        self.shortcuts(&ctx);
        self.update_title(&ctx);
        let selected_id = self.selection.and_then(|i| self.scene.elements.get(i)).map(|e| e.id.clone());
        if selected_id != self.last_property_id {
            if selected_id.is_some() {
                self.dock_open = true;
                self.dock_properties = true;
            }
            self.last_property_id = selected_id;
        }
        egui::Panel::top("studio-title")
            .frame(egui::Frame::new().fill(theme::CHROME).inner_margin(egui::Margin::symmetric(12, 4)))
            .show(ui, |ui| self.app_bar(ui));
        egui::Panel::top("workspace-tabs")
            .frame(egui::Frame::new().fill(theme::PANEL).inner_margin(egui::Margin::symmetric(12, 0)))
            .show(ui, |ui| self.workspace_tabs(ui));
        if !self.focus_canvas {
            egui::Panel::top("ribbon").frame(egui::Frame::new().fill(theme::PANEL).inner_margin(egui::Margin::symmetric(10, 3))).show(ui, |ui| {
                if self.ribbon_expanded {
                    self.ribbon(ui);
                } else {
                    self.compact_ribbon(ui);
                }
            });
            if self.show_command_strip {
                egui::Panel::top("command-strip")
                    .frame(egui::Frame::new().fill(theme::CHROME).inner_margin(egui::Margin::symmetric(10, 2)))
                    .show(ui, |ui| self.command_strip(ui));
            }
        }
        egui::Panel::bottom("status").show(ui, |ui| self.status_bar(ui));
        self.model_tools.sync_context(&self.session.doc, self.plan_level);
        self.drafting.sync_context(&self.session.doc);
        self.sync_family_view();
        self.browser_rect = None;
        self.dock_rect = None;
        if self.dock_open && !self.focus_canvas {
            let width = (ctx.content_rect().width() * 0.22).clamp(240.0, 320.0);
            let dock = egui::Panel::left("project-dock")
                .resizable(true)
                .default_size(260.0)
                .size_range(240.0..=width)
                .frame(egui::Frame::new().fill(theme::PANEL).inner_margin(10))
                .show(ui, |ui| {
                    ui.style_mut().wrap_mode = Some(egui::TextWrapMode::Wrap);
                    ui.spacing_mut().text_edit_width = ui.available_width();
                    ui.horizontal(|ui| {
                        ui.selectable_value(&mut self.dock_properties, false, "Browser");
                        ui.selectable_value(&mut self.dock_properties, true, "Properties");
                        if ui.small_button("×").on_hover_text("Close panel").clicked() {
                            self.dock_open = false;
                        }
                    });
                    ui.separator();
                    let body = egui::ScrollArea::vertical()
                        .id_salt(if self.dock_properties { "properties-scroll" } else { "project-scroll" })
                        .auto_shrink([false, false])
                        .show(ui, |ui| {
                            if self.dock_properties {
                                self.selection_panel(ui);
                            } else {
                                self.project_browser(ui);
                            }
                        });
                    if !self.dock_properties {
                        self.browser_rect = Some(body.inner_rect);
                    }
                });
            self.dock_rect = Some(dock.response.rect);
        }
        egui::CentralPanel::no_frame().show(ui, |ui| {
            if self.authoring.tab == Tab::Schedules {
                ui.painter().rect_filled(ui.max_rect(), 0.0, theme::PANEL);
                if let Some(action) = self.schedules.ui(ui, &self.scene, self.selection) {
                    self.schedule_action(action);
                }
                return;
            }
            egui::Frame::new().fill(theme::CHROME).inner_margin(egui::Margin::symmetric(10, 3)).show(ui, |ui| {
                ui.set_min_width(ui.available_width());
                let tabs_width = (ui.available_width() - 210.0).max(100.0);
                ui.horizontal(|ui| {
                    ui.allocate_ui_with_layout(egui::vec2(tabs_width, 28.0), egui::Layout::left_to_right(egui::Align::Center), |ui| {
                        self.view_tabs(ui)
                    });
                    ui.menu_button("View settings", |ui| self.view_controls(ui));
                    if ui.button("Fit").clicked() {
                        self.fit();
                    }
                });
            });
            match self.mode {
                Mode::ThreeD => self.viewport_3d(ui),
                Mode::Plan | Mode::Drawing => self.viewport_drawing(ui),
            }
        });
        if self.show_activity && !self.focus_canvas {
            let mut open = self.show_activity;
            let mut action = None;
            egui::Window::new("Activity")
                .open(&mut open)
                .default_width(300.0)
                .default_height(300.0)
                .max_height((ctx.content_rect().height() - 100.0).max(160.0))
                .show(&ctx, |ui| action = self.feed.ui(ui, true, true));
            self.show_activity = open;
            if let Some(action) = action {
                self.undo_redo(action);
            }
        }
        self.annotation_canvas_editor(&ctx, annotation_keyboard_owned);
        if !self.model_export.open
            && let Some(run) = self.palette.ui(&ctx)
        {
            let r = self.run(Who::Owner, run.command, run.params);
            self.palette.set_outcome(r.map(|v| crate::activity::pretty(&v)));
        }
        self.confirm_close_dialog(&ctx);
        if !self.confirm_close
            && let Some(result) = self.model_export.ui(&ctx, &self.session.doc)
        {
            self.status = Some(match result {
                Ok(()) => ("Exporting a model snapshot…".into(), false),
                Err(error) => (error, true),
            });
        }
    }
}

impl eframe::App for StudioApp {
    fn raw_input_hook(&mut self, ctx: &egui::Context, raw_input: &mut egui::RawInput) {
        // egui may surrender text focus on Escape at begin_pass, before shortcuts run.
        self.family_text_owned_on_input = self.authoring.family_placement_active() && ctx.text_edit_focused();
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
        self.poll_model_export();
        let _ = frame;
        for _ in 0..REQUESTS_PER_FRAME {
            let Some(req) = self.server.as_ref().and_then(Server::try_recv) else { break };
            self.handle_request(req, ctx);
        }
        self.serve_shots(ctx);
    }

    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        self.workspace_ui(ui);
    }
}

/// ComboBox::width is a minimum; constrain its child UI as well so long saved names
/// cannot expand the canvas. Allocating the whole control lets the parent wrap it.
fn view_combo(ui: &mut egui::Ui, id: &str, label: &str, width: f32, choices: impl FnOnce(&mut egui::Ui)) -> egui::Response {
    ui.allocate_ui_with_layout(egui::vec2(width, ui.spacing().interact_size.y), egui::Layout::left_to_right(egui::Align::Center), |ui| {
        egui::ComboBox::from_id_salt(id).width(width).truncate().selected_text(label).show_ui(ui, choices).response.on_hover_text(label)
    })
    .inner
}

#[cfg(test)]
mod tests {
    use super::*;

    fn app(phases: bool) -> StudioApp {
        let session = if phases { schedules::tests::phase_session() } else { schedules::tests::session() };
        let mut app = StudioApp::with_context(egui::Context::default(), None, session);
        app.phase = app.session.doc.active_index();
        app.request_rebuild(None);
        complete(&mut app);
        app.authoring.tab = Tab::Schedules;
        app
    }

    fn result(app: &StudioApp) -> Done {
        crate::worker::run(&Job {
            generation: app.generation,
            doc: Arc::new(app.session.doc.clone()),
            view: app.view(),
            want_scene: true,
            phase_colors: app.phase_colors,
            drawing: None,
        })
    }

    fn complete(app: &mut StudioApp) {
        app.apply_done(result(app));
        assert!(!app.building());
    }

    fn window(app: &StudioApp) -> usize {
        let row = app.schedules.data().unwrap().rows.iter().find(|r| r.kind == trigon_engine::ScheduleOpeningKind::Window).unwrap();
        schedules::resolve_row(&app.scene, row).unwrap()
    }

    #[test]
    fn model_exports_are_findable_from_the_3d_and_plan_workspaces() {
        for mode in [Mode::ThreeD, Mode::Plan] {
            let mut app = app(false);
            app.mode = mode;
            let document = app.session.doc.clone();
            let ctx = egui::Context::default();
            theme::apply(&ctx);
            let mut time = 0.0;
            let mut draw = |app: &mut StudioApp, events| {
                time += 0.05;
                let mut output = ctx.run_ui(
                    egui::RawInput {
                        screen_rect: Some(egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(1440.0, 250.0))),
                        time: Some(time),
                        events,
                        ..Default::default()
                    },
                    |ui| app.app_bar(ui),
                );
                output.textures_delta.clear();
                output.shapes
            };
            let shapes = draw(&mut app, Vec::new());
            let point = shapes
                .iter()
                .find_map(|shape| match &shape.shape {
                    egui::Shape::Text(text) if text.galley.text() == "Export" => Some(text.pos + text.galley.size() * 0.5),
                    _ => None,
                })
                .expect("Export must remain visible in the application bar");
            for event in [
                egui::Event::PointerMoved(point),
                egui::Event::PointerButton { pos: point, button: egui::PointerButton::Primary, pressed: true, modifiers: Default::default() },
                egui::Event::PointerButton { pos: point, button: egui::PointerButton::Primary, pressed: false, modifiers: Default::default() },
            ] {
                draw(&mut app, vec![event]);
            }
            let shapes = draw(&mut app, Vec::new());
            fn labels(shapes: &[egui::epaint::ClippedShape]) -> Vec<String> {
                shapes
                    .iter()
                    .flat_map(|shape| match &shape.shape {
                        egui::Shape::Text(text) => vec![text.galley.text().to_string()],
                        egui::Shape::Vec(shapes) => shapes
                            .iter()
                            .filter_map(|shape| match shape {
                                egui::Shape::Text(text) => Some(text.galley.text().to_string()),
                                _ => None,
                            })
                            .collect(),
                        _ => Vec::new(),
                    })
                    .collect()
            }
            let labels = labels(&shapes);
            ctx.data_mut(|data| data.clear());
            assert!(labels.iter().any(|label| label.contains("IFC")), "{mode:?}: model IFC export is inaccessible: {labels:?}");
            assert!(labels.iter().any(|label| label.contains("GLB")), "{mode:?}: model GLB export is inaccessible: {labels:?}");
            assert_eq!(app.session.doc, document, "opening Export must not edit the model");
        }
    }

    #[test]
    fn drawing_export_reveals_its_properties_controls_from_any_dock_state() {
        for (dock, properties, focus) in [(false, false, false), (true, false, false), (true, true, false), (true, false, true)] {
            let mut app = app(false);
            app.run(Who::Owner, "view.define", json!({"id":"export-drawing","name":"Export drawing","kind":{"view":"plan","level":0}})).unwrap();
            app.open_drawing("export-drawing".into());
            app.authoring.tab = Tab::Project;
            app.dock_open = dock;
            app.dock_properties = properties;
            app.focus_canvas = focus;
            complete(&mut app);
            let before = app.session.doc.clone();
            let ctx = egui::Context::default();
            theme::apply(&ctx);
            let mut time = 0.0;
            let mut draw = |app: &mut StudioApp, events| {
                time += 0.05;
                let mut output = ctx.run_ui(
                    egui::RawInput {
                        screen_rect: Some(egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(1440.0, 900.0))),
                        time: Some(time),
                        events,
                        ..Default::default()
                    },
                    |ui| app.workspace_ui(ui),
                );
                output.textures_delta.clear();
                output.shapes
            };
            let mut click = |app: &mut StudioApp, label: &str| {
                let shapes = draw(app, Vec::new());
                let point = shapes
                    .iter()
                    .find_map(|shape| match &shape.shape {
                        egui::Shape::Text(text) if text.galley.text() == label => Some(text.pos + text.galley.size() * 0.5),
                        _ => None,
                    })
                    .unwrap_or_else(|| panic!("missing action {label}"));
                for event in [
                    egui::Event::PointerMoved(point),
                    egui::Event::PointerButton { pos: point, button: egui::PointerButton::Primary, pressed: true, modifiers: Default::default() },
                    egui::Event::PointerButton { pos: point, button: egui::PointerButton::Primary, pressed: false, modifiers: Default::default() },
                ] {
                    draw(app, vec![event]);
                }
            };
            click(&mut app, "Export");
            click(&mut app, "Drawing · PDF, SVG, PNG, DXF…");
            let mut controls_visible = false;
            // Existing header/scroll animations need to settle before checking
            // the final visible controls; these are simulated frames, not sleeps.
            for _ in 0..20 {
                let shapes = draw(&mut app, Vec::new());
                controls_visible = shapes.iter().any(|shape| match &shape.shape {
                    egui::Shape::Text(text) if text.galley.text() == "Export annotated view" => {
                        shape.clip_rect.contains(text.pos + text.galley.size() * 0.5)
                    }
                    _ => false,
                });
            }
            ctx.data_mut(|data| data.clear());
            assert!(app.dock_open && app.dock_properties && !app.focus_canvas);
            assert!(controls_visible, "export controls are inaccessible from dock={dock}, properties={properties}, focus={focus}");
            assert_eq!(app.session.doc, before);
        }
    }

    #[test]
    fn export_modal_preserves_a_ready_annotation_and_its_keyboard_input() {
        let mut app = app(false);
        app.session.execute("view.define", json!({"id":"export-input","name":"Export input","kind":{"view":"plan","level":0}})).unwrap();
        app.open_drawing("export-input".into());
        complete(&mut app);
        app.drafting.start_placement(&app.session.doc, "export-input", 0);
        app.drafting.picked([1000.0, 1000.0]);
        let ctx = egui::Context::default();
        theme::apply(&ctx);
        let document = app.session.doc.clone();
        app.show_model_export(crate::model_export::Format::Ifc);
        for (frame, key) in [None, Some(egui::Key::Enter), Some(egui::Key::Escape)].into_iter().enumerate() {
            let events = key
                .map(|key| egui::Event::Key { key, physical_key: None, pressed: true, repeat: false, modifiers: Default::default() })
                .into_iter()
                .collect();
            let mut output = ctx.run_ui(
                egui::RawInput {
                    screen_rect: Some(egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(1440.0, 900.0))),
                    time: Some(frame as f64 * 0.05),
                    events,
                    ..Default::default()
                },
                |ui| app.workspace_ui(ui),
            );
            output.textures_delta.clear();
            assert_eq!(app.session.doc, document);
            assert!(app.drafting.placement_active(), "the export dialog must not finish or discard the annotation");
        }
        assert!(!app.model_export.open, "Escape should close only Export");
        ctx.data_mut(|data| data.clear());
    }

    #[test]
    fn export_completion_records_the_typed_command_without_a_rebuild_or_history_edit() {
        let mut app = app(false);
        let ctx = egui::Context::default();
        let path = std::env::temp_dir().join(format!("trigon-studio-export-{}.glb", std::process::id()));
        app.mode = Mode::ThreeD;
        app.phase = 0;
        app.show_model_export(crate::model_export::Format::Glb);
        let document = app.session.doc.clone();
        let dirty = app.session.dirty;
        let generation = app.generation;
        app.model_export.start(&app.session.doc, path.clone(), &ctx).unwrap();
        let deadline = Instant::now() + Duration::from_secs(30);
        while app.model_export.busy() && Instant::now() < deadline {
            app.poll_model_export();
            std::thread::sleep(Duration::from_millis(2));
        }
        assert!(!app.model_export.busy());
        let entry = app.feed.latest().unwrap();
        assert_eq!(entry.command, "export.gltf");
        assert!(entry.result.is_ok(), "{:?}", entry.result);
        assert_eq!(entry.params["phase"], "existing");
        assert_eq!(app.session.doc, document);
        assert_eq!(app.session.dirty, dirty);
        assert_eq!(app.generation, generation, "a read-only model export must not rebuild the viewport");
        assert!(app.view_state()["model_export"]["written"].is_string());
        assert_eq!(app.view_state()["model_export"]["error"], Value::Null);
        assert!(std::fs::read(&path).unwrap().starts_with(b"glTF"));
        std::fs::remove_file(path).unwrap();
    }

    #[test]
    fn activity_keeps_old_project_export_scope_after_switching_and_opening_another_export() {
        let mut app = app(false);
        let ctx = egui::Context::default();
        app.session.path = Some(PathBuf::from("original.trigon"));
        app.show_model_export(crate::model_export::Format::Glb);
        let path = std::env::temp_dir().join(format!("trigon-studio-export-context-{}.glb", std::process::id()));
        app.model_export.start(&app.session.doc, path.clone(), &ctx).unwrap();
        app.run(Who::Owner, "document.new", json!({})).unwrap();
        let new_document = app.session.doc.clone();
        let deadline = Instant::now() + Duration::from_secs(30);
        while app.model_export.busy() && Instant::now() < deadline {
            app.poll_model_export();
            std::thread::sleep(Duration::from_millis(2));
        }
        assert!(!app.model_export.busy());
        let original_entry = app.feed.latest().unwrap();
        let seq = original_entry.seq;
        assert_eq!(original_entry.command, "export.gltf");
        assert!(original_entry.result.is_ok());
        assert!(original_entry.context.as_deref().unwrap().contains("original.trigon"));
        assert!(original_entry.context.as_deref().unwrap().contains("full model"));
        assert!(original_entry.params.get("context").is_none() && original_entry.params.get("scope").is_none());
        app.show_model_export(crate::model_export::Format::Ifc);
        assert_eq!(app.feed.latest().unwrap().seq, seq);
        assert!(app.feed.latest().unwrap().context.as_deref().unwrap().contains("original.trigon"));
        assert_eq!(app.session.doc, new_document);
        std::fs::remove_file(path).unwrap();
    }

    #[test]
    fn normal_close_waits_for_a_running_export_even_when_the_project_is_clean() {
        let mut app = app(false);
        app.session.dirty = false;
        let ctx = egui::Context::default();
        let path = std::env::temp_dir().join(format!("trigon-studio-export-close-{}.glb", std::process::id()));
        app.show_model_export(crate::model_export::Format::Glb);
        app.model_export.start(&app.session.doc, path.clone(), &ctx).unwrap();
        let close_input = || {
            let mut input = egui::RawInput::default();
            input.viewports.entry(egui::ViewportId::ROOT).or_default().events.push(egui::ViewportEvent::Close);
            input
        };
        let mut output = ctx.run_ui(close_input(), |_| app.confirm_close_dialog(&ctx));
        output.textures_delta.clear();
        assert!(output.viewport_output[&egui::ViewportId::ROOT].commands.iter().any(|command| matches!(command, egui::ViewportCommand::CancelClose)));
        assert!(!app.confirm_close, "a clean document does not need a save prompt");
        assert!(app.model_export.open && app.model_export.busy());
        let deadline = Instant::now() + Duration::from_secs(30);
        while app.model_export.busy() && Instant::now() < deadline {
            app.poll_model_export();
            std::thread::sleep(Duration::from_millis(2));
        }
        assert!(!app.model_export.busy());
        let mut output = ctx.run_ui(close_input(), |_| app.confirm_close_dialog(&ctx));
        output.textures_delta.clear();
        let cancelled =
            output.viewport_output[&egui::ViewportId::ROOT].commands.iter().any(|command| matches!(command, egui::ViewportCommand::CancelClose));
        ctx.data_mut(|data| data.clear());
        assert!(!cancelled, "close can proceed after the export finishes");
        assert!(std::fs::read(&path).unwrap().starts_with(b"glTF"));
        std::fs::remove_file(path).unwrap();
    }

    #[test]
    fn repeated_escape_releases_model_tools_and_roof_cancels_immediately() {
        let mut app = app(false);
        app.begin_model_tool(false);
        app.model_tools.point(&app.session.doc, [1000.0, 1000.0]);
        let ctx = egui::Context::default();
        for _ in 0..2 {
            let mut output = ctx.run_ui(
                egui::RawInput {
                    events: vec![egui::Event::Key {
                        key: egui::Key::Escape,
                        physical_key: None,
                        pressed: true,
                        repeat: false,
                        modifiers: Default::default(),
                    }],
                    ..Default::default()
                },
                |_| app.shortcuts(&ctx),
            );
            output.textures_delta.clear();
        }
        let wall_exited = !app.model_tools.active();
        app.begin_model_tool(true);
        let mut output = ctx.run_ui(
            egui::RawInput {
                events: vec![egui::Event::Key {
                    key: egui::Key::Escape,
                    physical_key: None,
                    pressed: true,
                    repeat: false,
                    modifiers: Default::default(),
                }],
                ..Default::default()
            },
            |_| app.shortcuts(&ctx),
        );
        output.textures_delta.clear();
        let roof_exited = !app.model_tools.active();
        ctx.data_mut(|data| data.clear());
        assert!(wall_exited && roof_exited, "Escape must return control to selection: wall={wall_exited}, roof={roof_exited}");
    }

    #[test]
    fn shift_command_redo_is_not_consumed_as_plain_undo() {
        for mac in [true, false] {
            let mut app = app(false);
            app.run(Who::Owner, "view.define", json!({"id":"redo-key","name":"Redo key","kind":{"view":"plan","level":0}})).unwrap();
            let expected = app.session.doc.clone();
            app.undo_redo(Action::Undo);
            let ctx = egui::Context::default();
            ctx.set_os(if mac { egui::os::OperatingSystem::Mac } else { egui::os::OperatingSystem::Windows });
            let modifiers = egui::Modifiers { command: true, mac_cmd: mac, ctrl: !mac, shift: true, ..Default::default() };
            let mut output = ctx.run_ui(
                egui::RawInput {
                    events: vec![egui::Event::Key { key: egui::Key::Z, physical_key: None, pressed: true, repeat: false, modifiers }],
                    ..Default::default()
                },
                |_| app.shortcuts(&ctx),
            );
            output.textures_delta.clear();
            ctx.data_mut(|data| data.clear());
            assert_eq!(app.session.doc, expected, "Shift+command+Z must Redo on mac={mac}, status={:?}", app.status);
        }
    }

    #[test]
    fn palette_enter_cannot_submit_a_ready_canvas_annotation() {
        let mut app = app(false);
        app.session.execute("view.define", json!({"id":"annotation-input","name":"Input ownership","kind":{"view":"plan","level":0}})).unwrap();
        app.open_drawing("annotation-input".into());
        app.drafting.start_placement(&app.session.doc, "annotation-input", 0);
        app.drafting.picked([1000.0, 1000.0]);
        app.palette.show();
        let before = app.session.doc.clone();
        let ctx = egui::Context::default();
        theme::apply(&ctx);
        for pressed in [false, true] {
            let events = if pressed {
                vec![egui::Event::Key { key: egui::Key::Enter, physical_key: None, pressed: true, repeat: false, modifiers: Default::default() }]
            } else {
                vec![]
            };
            let mut output = ctx.run_ui(
                egui::RawInput {
                    screen_rect: Some(egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(1440.0, 900.0))),
                    events,
                    ..Default::default()
                },
                |ui| app.workspace_ui(ui),
            );
            output.textures_delta.clear();
        }
        ctx.data_mut(|data| data.clear());
        assert_eq!(app.session.doc, before, "Enter in Commands must not place the retained canvas note");
        assert!(app.drafting.placement_active());
        assert_eq!(app.drafting.placement_feedback()["pending"], false);
    }

    #[test]
    fn modal_escape_does_not_discard_an_unfinished_canvas_annotation() {
        for close_dialog in [false, true] {
            let mut app = app(false);
            app.session.execute("view.define", json!({"id":"escape-input","name":"Escape ownership","kind":{"view":"plan","level":0}})).unwrap();
            app.open_drawing("escape-input".into());
            app.drafting.start_placement(&app.session.doc, "escape-input", 4);
            app.drafting.picked([1000.0, 1000.0]);
            app.drafting.picked([3000.0, 1000.0]);
            if close_dialog {
                app.confirm_close = true;
            } else {
                app.palette.show();
            }
            let before = app.drafting.placement_feedback();
            let ctx = egui::Context::default();
            theme::apply(&ctx);
            let mut output = ctx.run_ui(
                egui::RawInput {
                    screen_rect: Some(egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(1440.0, 900.0))),
                    events: vec![egui::Event::Key {
                        key: egui::Key::Escape,
                        physical_key: None,
                        pressed: true,
                        repeat: false,
                        modifiers: Default::default(),
                    }],
                    ..Default::default()
                },
                |ui| app.workspace_ui(ui),
            );
            output.textures_delta.clear();
            ctx.data_mut(|data| data.clear());
            assert_eq!(app.drafting.placement_feedback()["picked_points"], before["picked_points"], "the modal owned Escape");
            assert!(app.drafting.placement_active());
        }
    }

    #[test]
    fn popup_keyboard_input_cannot_finish_or_cancel_a_canvas_draft() {
        for key in [egui::Key::Escape, egui::Key::Enter] {
            let mut app = app(false);
            app.session.execute("view.define", json!({"id":"popup-input","name":"Popup ownership","kind":{"view":"plan","level":0}})).unwrap();
            app.open_drawing("popup-input".into());
            app.drafting.start_placement(&app.session.doc, "popup-input", 4);
            for point in [[1000.0, 1000.0], [3000.0, 1000.0], [3000.0, 3000.0]] {
                app.drafting.picked(point);
            }
            let before = app.drafting.placement_feedback();
            let document = app.session.doc.clone();
            let ctx = egui::Context::default();
            theme::apply(&ctx);
            egui::Popup::open_id(&ctx, egui::Id::new("family-or-font-selector"));
            let mut output = ctx.run_ui(
                egui::RawInput {
                    screen_rect: Some(egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(1440.0, 900.0))),
                    events: vec![egui::Event::Key { key, physical_key: None, pressed: true, repeat: false, modifiers: Default::default() }],
                    ..Default::default()
                },
                |ui| app.workspace_ui(ui),
            );
            output.textures_delta.clear();
            ctx.data_mut(|data| data.clear());
            assert_eq!(app.drafting.placement_feedback()["picked_points"], before["picked_points"], "the popup owns {key:?}");
            assert!(app.drafting.placement_active(), "the popup owns {key:?}");
            assert_eq!(app.session.doc, document, "popup input cannot commit the canvas draft");
        }
    }

    #[test]
    fn elevation_host_picks_convert_u_to_the_correct_translated_facade_bay() {
        let mut app = app(false);
        let building = &mut app.session.doc.buildings[0];
        building.footprint = vec![[10000.0, 20000.0], [18000.0, 20000.0], [18000.0, 28000.0], [10000.0, 28000.0]];
        let facade = building.facades.iter_mut().find(|facade| facade.edge == 1).unwrap();
        facade.bays = vec![2000.0, 4000.0, 6000.0];
        let facade_id = facade.id.clone();
        let mut host = app.scene.elements.iter().find(|element| element.kind == "wall").unwrap().clone();
        host.id = facade_id.clone();
        host.level = Some(0);
        app.session
            .doc
            .views
            .push(serde_json::from_value(json!({"id":"elevation-pick","name":"Elevation","kind":{"view":"facade","facade":facade_id}})).unwrap());
        app.mode = Mode::Drawing;
        app.drawing = "elevation-pick".into();
        app.authoring.begin_family_placement(&app.session.doc, None);
        let point = app.family_site_point(&host, [6000.0, 2500.0]).unwrap();
        assert_eq!(point, [18000.0, 26000.0]);
        assert!(app.authoring.pick_family_host(&app.session.doc, &host, Some(point)));
        let document = app.session.doc.clone();
        app.session.doc.views[0].kind = DrawingViewKind::Section { from: [10000.0, 20000.0], to: [18000.0, 20000.0], flip: false, depth_mm: 30000.0 };
        assert!(app.family_site_point(&host, [6000.0, 2500.0]).is_none(), "ambiguous projected section depths must be refused");
        app.session.doc = document;
    }

    #[test]
    fn rejected_family_command_keeps_the_engine_error_in_authoring_feedback() {
        let mut app = app(false);
        app.authoring.begin_family_placement(&app.session.doc, None);
        let facade = app.session.doc.buildings[0].facades.iter().find(|facade| !facade.bays.is_empty()).unwrap().id.clone();
        let ty = app.session.doc.types.iter().find(|ty| ty.kind.is_opening()).unwrap().id.clone();
        let before = app.session.doc.clone();
        app.run_form(crate::authoring::Run {
            command: "facade.place",
            params: json!({"facade":facade,"type":ty,"levels":[0],"bays":[0],"width":800,"height":4000,"sill":500}),
        });
        let (error, failed) = app.status.as_ref().unwrap();
        assert!(*failed, "the opening cannot fit this storey");
        assert_eq!(app.session.doc, before, "rejected family placement is atomic");
        assert_eq!(app.authoring_feedback()["family_placement"]["command_error"], error.as_str());
        app.run_form(crate::authoring::Run {
            command: "facade.place",
            params: json!({"facade":facade,"type":ty,"levels":[0],"bays":[0],"width":800,"height":800,"sill":500}),
        });
        assert!(!app.status.as_ref().unwrap().1, "the corrected opening must be accepted");
        assert!(app.authoring_feedback()["family_placement"]["command_error"].is_null(), "success clears the retained rejection");
        app.session.execute("edit.undo", json!({})).unwrap();
        assert_eq!(app.session.doc, before, "retry remains undoable through the shared command");
    }

    fn point_app() -> StudioApp {
        let mut session = Session::new(Box::new(Live::new(Credentials::default())));
        session.doc.buildings.push(
            trigon_model::Building::new(
                "point",
                "Point placement fixture",
                &[[0.0, 0.0], [12000.0, 0.0], [12000.0, 8000.0], [0.0, 8000.0]],
                3,
                3000.0,
            )
            .unwrap(),
        );
        let mut app = StudioApp::with_context(egui::Context::default(), None, session);
        app.request_rebuild(None);
        complete(&mut app);
        app.authoring.choose_type(app.session.doc.types.iter().find(|t| t.id == "window.fixed").unwrap());
        app.authoring.begin_family_placement(&app.session.doc, None);
        app
    }

    fn capture_point(app: &mut StudioApp, u: f64) {
        let mut host = app.scene.elements.iter().find(|e| e.id == "point.f1").unwrap().clone();
        host.level = Some(1); // Explicit Storey selection, not an inferred global index.
        assert!(app.authoring.pick_family_host(&app.session.doc, &host, Some([u, 0.0])));
    }

    #[test]
    fn point_preview_legend_has_readable_contrast_against_its_painted_panel() {
        let mut app = point_app();
        capture_point(&mut app, 3200.125);
        let ctx = egui::Context::default();
        theme::apply(&ctx);
        let mut output = ctx.run_ui(
            egui::RawInput { screen_rect: Some(egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(1000.0, 700.0))), ..Default::default() },
            |ui| app.family_preview_ui(ui, ui.max_rect()),
        );
        output.textures_delta.clear();
        let text = output
            .shapes
            .iter()
            .find_map(|shape| match &shape.shape {
                egui::epaint::Shape::Text(text) if text.galley.job.text.starts_with("Captured · L1 · u ") => Some(text),
                _ => None,
            })
            .expect("a captured component must have a visible dimension legend");
        let background = output
            .shapes
            .iter()
            .rev()
            .find_map(|shape| match &shape.shape {
                egui::epaint::Shape::Rect(rect) if rect.rect.contains(text.pos) && rect.fill.a() > 0 => Some(rect.fill),
                _ => None,
            })
            .expect("the dimension legend must have a painted background");
        let luminance = |color: egui::Color32| {
            [color.r(), color.g(), color.b()]
                .into_iter()
                .zip([0.2126, 0.7152, 0.0722])
                .map(|(channel, weight)| {
                    let value = f64::from(channel) / 255.0;
                    weight * if value <= 0.04045 { value / 12.92 } else { ((value + 0.055) / 1.055).powf(2.4) }
                })
                .sum::<f64>()
        };
        assert_eq!(background.a(), 255);
        for section in &text.galley.job.sections {
            let color = text.override_text_color.unwrap_or(if section.format.color == egui::Color32::PLACEHOLDER {
                text.fallback_color
            } else {
                section.format.color
            });
            assert_eq!(color.a(), 255);
            let (a, b) = (luminance(color), luminance(background));
            let contrast = (a.max(b) + 0.05) / (a.min(b) + 0.05);
            assert!(contrast >= 4.5, "placement legend contrast is only {contrast:.3}:1");
        }
    }

    #[test]
    fn long_status_errors_do_not_paint_over_model_metrics() {
        let mut app = point_app();
        let error = "invalid: point.f1 level 1 bay 2: sill 800.125 + height 4000 leaves less than 150 mm below the storey above (storey height 3000); stairwell windows that cross a floor need spans_storeys";
        let count = format!("{} elements", app.scene.elements.len());
        for (width, message) in [(820.0, "saved"), (820.0, error), (1440.0, "saved"), (1440.0, error)] {
            app.status = Some((message.into(), message == error));
            let ctx = egui::Context::default();
            theme::apply(&ctx);
            let mut output = ctx.run_ui(
                egui::RawInput { screen_rect: Some(egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(width, 80.0))), ..Default::default() },
                |ui| app.status_bar(ui),
            );
            output.textures_delta.clear();
            let labels = output
                .shapes
                .iter()
                .filter_map(|shape| match &shape.shape {
                    egui::epaint::Shape::Text(text) => Some((shape.clip_rect, text)),
                    _ => None,
                })
                .collect::<Vec<_>>();
            let (clip, status) =
                labels.iter().find(|(_, text)| text.galley.job.text == message).expect("retain the complete status in its layout job");
            let status_ink = status.visual_bounding_rect().intersect(*clip);
            assert!(status_ink.is_positive(), "the error must remain visible at width {width}");
            for expected in ["Model units / mm", count.as_str()] {
                let (_, metric) = labels.iter().find(|(_, text)| text.galley.job.text == expected).expect("keep each model metric visible");
                let bounds = metric.visual_bounding_rect();
                assert!(bounds.left() >= 0.0 && bounds.right() <= width, "model metrics must remain inside the status bar at width {width}");
                assert!(!status_ink.intersects(bounds), "the error overlaps {expected} at width {width}");
            }
        }
    }

    #[test]
    fn point_command_error_survives_repaint_and_success_selects_exact_returned_products() {
        let mut app = point_app();
        capture_point(&mut app, 3200.125);
        let before = app.session.doc.clone();
        let mut run = app.authoring.place_type(&app.session.doc).unwrap();
        run.params["height"] = json!(4000.0);
        run.params["sill"] = json!(500.0);
        app.run_form(run);
        let error = app.feed.latest().unwrap().result.as_ref().unwrap_err().clone();
        assert_eq!(app.session.doc, before);
        let ctx = egui::Context::default();
        theme::apply(&ctx);
        for hidden in [true, false, true] {
            app.dock_open = !hidden;
            let mut output = ctx.run_ui(
                egui::RawInput { screen_rect: Some(egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(1440.0, 900.0))), ..Default::default() },
                |ui| app.workspace_ui(ui),
            );
            output.textures_delta.clear();
            assert_eq!(app.authoring_feedback()["family_placement"]["command_error"], error);
            assert_eq!(app.session.doc, before);
        }
        app.run_form(app.authoring.place_type(&app.session.doc).unwrap());
        let result = app.feed.latest().unwrap().result.as_ref().unwrap().clone();
        assert!(app.authoring_feedback()["family_placement"]["command_error"].is_null());
        assert_eq!(app.pending_selection.as_deref(), result["targets"][0].as_str());
        assert_eq!(app.session.doc.phases[app.phase].id, result["phase"]);
        assert!(app.building());
        complete(&mut app);
        let id = &app.scene.elements[app.selection.unwrap()].id;
        assert_eq!(id, result["targets"][0].as_str().unwrap());
        assert_eq!(app.properties.descriptor(id).unwrap()["editable"], true);
        assert_eq!(app.authoring_feedback()["family_placement"]["scope"]["level"], 1);
        assert_eq!(app.authoring_feedback()["family_placement"]["stale"], false);
        capture_point(&mut app, 7000.25);
        app.run_form(app.authoring.place_type(&app.session.doc).unwrap());
        complete(&mut app);
        let placed = app.session.doc.clone();
        app.undo_redo(Action::Undo);
        complete(&mut app);
        app.undo_redo(Action::Undo);
        complete(&mut app);
        assert_eq!(app.session.doc, before);
        app.undo_redo(Action::Redo);
        complete(&mut app);
        app.undo_redo(Action::Redo);
        complete(&mut app);
        assert_eq!(app.session.doc, placed);
    }

    #[test]
    fn point_attachment_selection_uses_returned_leaf_ids_and_never_falls_back_to_another_product() {
        let mut app = point_app();
        let guard:trigon_model::ComponentType=serde_json::from_value(json!({"id":"guard.test","name":"Guard",
            "width":1200,"height":1100,"sill":0,"kind":{"family":"guard_rail","projection":180,"railing":"frederiksberg","finish":"galvanised","handrail":false}})).unwrap();
        app.session.doc.types.push(guard.clone());
        app.authoring.choose_type(&guard);
        capture_point(&mut app, 4000.125);
        app.run_form(app.authoring.place_type(&app.session.doc).unwrap());
        let result = app.feed.latest().unwrap().result.as_ref().unwrap().clone();
        complete(&mut app);
        let id = &app.scene.elements[app.selection.unwrap()].id;
        assert_eq!(id, result["targets"][0].as_str().unwrap());
        assert!(id.ends_with(".balcony.rail"));
        assert_eq!(app.properties.descriptor(id).unwrap()["editable"], true);
        app.pending_family_targets = Some(vec!["missing.L1.B0".into()]);
        app.pending_selection = Some("missing.L1.B0".into());
        app.apply_scene((*app.scene).clone(), app.scene_generation);
        assert!(app.selection.is_none(), "an exact engine target may never fall back to the previous selected product");
    }

    #[test]
    fn point_escape_respects_modal_popup_and_foreign_text_editor_ownership() {
        for owner in 0..5 {
            let mut app = point_app();
            capture_point(&mut app, 3200.125);
            let before = app.session.doc.clone();
            let ctx = egui::Context::default();
            if owner == 1 {
                app.palette.show();
            }
            if owner == 2 {
                app.confirm_close = true;
            }
            if owner == 3 {
                egui::Popup::open_id(&ctx, egui::Id::new("placement-popup"));
            }
            let mut text = String::from("foreign editor");
            if owner == 4 {
                for _ in 0..2 {
                    let mut output = ctx.run_ui(egui::RawInput::default(), |ui| {
                        let response = ui.add(egui::TextEdit::singleline(&mut text).id(egui::Id::new("point-foreign-editor")));
                        response.request_focus();
                    });
                    output.textures_delta.clear();
                }
                assert!(ctx.text_edit_focused(), "the fixture must establish real foreign editor ownership");
            }
            let mut input = egui::RawInput {
                events: vec![egui::Event::Key {
                    key: egui::Key::Escape,
                    physical_key: None,
                    pressed: true,
                    repeat: false,
                    modifiers: Default::default(),
                }],
                ..Default::default()
            };
            eframe::App::raw_input_hook(&mut app, &ctx, &mut input);
            let mut output = ctx.run_ui(input, |_| app.shortcuts(&ctx));
            output.textures_delta.clear();
            assert_eq!(app.authoring.family_placement_active(), owner != 0, "keyboard owner {owner}");
            if owner == 0 {
                assert!(app.authoring_feedback()["family_placement"]["captured_u_mm"].is_null());
            }
            assert_eq!(app.session.doc, before);
        }
    }

    #[test]
    fn point_scope_is_invalidated_with_properties_hidden_before_real_bare_wall_pick() {
        let mut app = point_app();
        app.session.doc.views.push(serde_json::from_value(json!({"id":"saved-ground","name":"Ground","kind":{"view":"plan","level":0}})).unwrap());
        app.mode = Mode::Drawing;
        app.drawing = "saved-ground".into();
        app.sync_family_view();
        let host = app.scene.elements.iter().find(|e| e.id == "point.f1").unwrap().clone();
        assert_eq!(host.level, None);
        assert!(app.authoring.pick_family_host(&app.session.doc, &host, Some([3200.0, 0.0])));
        app.run(Who::Owner, "building.add_basement", json!({"building":"point","floor":-2500})).unwrap();
        complete(&mut app);
        app.dock_open = false;
        let ctx = egui::Context::default();
        theme::apply(&ctx);
        let mut output = ctx.run_ui(
            egui::RawInput { screen_rect: Some(egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(1440.0, 900.0))), ..Default::default() },
            |ui| app.workspace_ui(ui),
        );
        output.textures_delta.clear();
        let host = app.scene.elements.iter().find(|e| e.id == "point.f1").unwrap().clone();
        assert!(!app.authoring.pick_family_host(&app.session.doc, &host, Some([3200.0, 0.0])));
        assert!(app.authoring.place_type(&app.session.doc).is_none());
        assert!(app.authoring_feedback()["family_placement"]["scope"]["level"].is_null());
    }

    #[test]
    fn point_placement_waits_for_the_current_worker_and_complete_phase_view() {
        let mut app = point_app();
        capture_point(&mut app, 3200.0);
        let before = app.session.doc.clone();
        let run = app.authoring.place_type(&app.session.doc).unwrap();
        app.request_rebuild(None);
        app.run_form(run);
        assert_eq!(app.session.doc, before);
        assert!(app.authoring_feedback()["family_placement"]["command_error"].as_str().unwrap().contains("Wait"));
        complete(&mut app);
        app.filter = PhaseFilter::Demolition;
        assert!(app.edit_block_reason().is_some());
        app.run_form(app.authoring.place_type(&app.session.doc).unwrap());
        assert_eq!(app.session.doc, before);
    }

    #[test]
    fn expanded_modify_and_component_actions_release_the_wall_sketch() {
        let mut app = app(false);
        app.authoring.tab = Tab::Project;
        let document = app.session.doc.clone();
        let ctx = egui::Context::default();
        theme::apply(&ctx);
        let mut time = 0.0;
        let mut draw = |app: &mut StudioApp, events| {
            time += 0.05;
            let mut output = ctx.run_ui(
                egui::RawInput {
                    screen_rect: Some(egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(1440.0, 200.0))),
                    time: Some(time),
                    events,
                    ..Default::default()
                },
                |ui| {
                    app.ribbon(ui);
                },
            );
            output.textures_delta.clear();
            output.shapes
        };
        for label in ["Move", "Copy", "All types"] {
            app.begin_model_tool(false);
            app.model_tools.point(&app.session.doc, [1000.0, 1000.0]);
            let shapes = draw(&mut app, vec![]);
            let pos = shapes
                .iter()
                .find_map(|shape| match &shape.shape {
                    egui::Shape::Text(text) if text.galley.text() == label => Some(text.pos + text.galley.size() * 0.5),
                    _ => None,
                })
                .unwrap();
            for event in [
                egui::Event::PointerMoved(pos),
                egui::Event::PointerButton { pos, button: egui::PointerButton::Primary, pressed: true, modifiers: Default::default() },
                egui::Event::PointerButton { pos, button: egui::PointerButton::Primary, pressed: false, modifiers: Default::default() },
            ] {
                draw(&mut app, vec![event]);
            }
            assert!(!app.model_tools.active(), "{label} must take control from the sketch");
            app.authoring.tab = Tab::Project;
        }
        ctx.data_mut(|data| data.clear());
        assert_eq!(app.session.doc, document);
    }

    /// Exercise the actual control screenshot queue using a synthetic loopback request.
    /// No discovery files or real credentials are read, and no egui pass runs here.
    fn hidden_viewport_capture(app: &mut StudioApp, ctx: &egui::Context) -> serde_json::Value {
        use std::io::{BufRead, BufReader, Write};
        use std::net::{TcpListener, TcpStream};
        use std::time::Duration;
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let address = listener.local_addr().unwrap();
        let token = "synthetic-drawing-screenshot-test".repeat(2);
        let server = Server::start_with(
            listener,
            token.clone(),
            Arc::new(|| {}),
            trigon_control::Options { reply_wait: Duration::from_secs(4), ..Default::default() },
        )
        .unwrap();
        let client = std::thread::spawn(move || {
            let mut stream = TcpStream::connect(address).unwrap();
            stream.set_read_timeout(Some(Duration::from_secs(5))).unwrap();
            writeln!(stream, "{}", json!({"id":1,"method":"ui.screenshot","params":{"target":"viewport"},"token":token})).unwrap();
            stream.flush().unwrap();
            let mut reply = String::new();
            BufReader::new(stream).read_line(&mut reply).unwrap();
            serde_json::from_str::<serde_json::Value>(&reply).unwrap()
        });
        let request = server.recv_timeout(Duration::from_secs(2)).unwrap();
        app.handle_request(request, ctx);
        let deadline = Instant::now() + Duration::from_secs(2);
        while !app.shots.is_empty() && Instant::now() < deadline {
            app.poll_worker();
            app.serve_shots(ctx);
            std::thread::sleep(Duration::from_millis(1));
        }
        assert!(app.shots.is_empty(), "hidden drawing screenshots must finish through logic/control work without an egui pass");
        let reply = client.join().unwrap();
        assert_eq!(reply["ok"], true, "{reply}");
        assert_eq!(app.plan.diagnostics()["uploads"], 0, "offscreen preparation must not enqueue GPU texture uploads");
        reply
    }

    fn hidden_drawing_fixture(app: &mut StudioApp, request: DrawingRequest) {
        app.window_hidden = true;
        app.viewport_rect = Some(egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(320.0, 220.0)));
        app.ppp = 1.0;
        app.generation += 1;
        app.plan_wanted = app.generation;
        let done = crate::worker::run(&Job {
            generation: app.generation,
            doc: Arc::new(app.session.doc.clone()),
            view: app.view(),
            want_scene: false,
            phase_colors: false,
            drawing: Some(request),
        });
        app.apply_done(done);
        assert!(!app.building() && !app.drawing_plan());
    }

    #[test]
    fn hidden_plan_viewport_screenshot_progresses_without_a_ui_frame() {
        let mut app = app(false);
        app.mode = Mode::Plan;
        app.plan_level = 0;
        hidden_drawing_fixture(&mut app, DrawingRequest::Plan(0));
        let ctx = egui::Context::default();
        let reply = hidden_viewport_capture(&mut app, &ctx);
        assert_eq!(reply["result"]["width"], 320);
        assert_eq!(reply["result"]["height"], 220);
        assert!(app.window_hidden);
        assert!(app.plan.snapshot_ready(app.viewport_rect.unwrap(), 1.0));
    }

    #[test]
    fn hidden_saved_drawing_and_refit_prepare_the_matching_style_without_ui() {
        let mut app = app(false);
        app.session.execute("view.define", json!({"id":"hidden-upper","name":"Hidden upper","kind":{"view":"plan","level":1}})).unwrap();
        app.mode = Mode::Drawing;
        app.drawing = "hidden-upper".into();
        hidden_drawing_fixture(&mut app, DrawingRequest::Saved("hidden-upper".into()));
        let ctx = egui::Context::default();
        hidden_viewport_capture(&mut app, &ctx);
        let first = app.plan.diagnostics()["stroke_zoom_bucket"].clone();
        app.viewport_rect = Some(egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(640.0, 440.0)));
        app.fit();
        let reply = hidden_viewport_capture(&mut app, &ctx);
        assert_eq!(reply["result"]["width"], 640);
        assert_eq!(reply["result"]["height"], 440);
        assert_ne!(app.plan.diagnostics()["stroke_zoom_bucket"], first, "hidden refit changes its logical pen bucket");
        assert!(app.window_hidden);
    }

    #[test]
    fn optional_screen_style_failure_does_not_block_edits_but_drawing_failure_does() {
        let mut app = app(false);
        app.authoring.tab = Tab::Project;
        app.mode = Mode::Plan;
        app.plan_level = 0;
        hidden_drawing_fixture(&mut app, DrawingRequest::Plan(0));
        let mut done = crate::worker::run(&Job {
            generation: app.generation,
            doc: Arc::new(app.session.doc.clone()),
            view: app.view(),
            want_scene: false,
            phase_colors: false,
            drawing: Some(DrawingRequest::Plan(0)),
        });
        let drawing = done.drawing.as_mut().unwrap().1.as_mut().unwrap();
        // Keep the already parsed engine base and exact pick/snap metadata, but reject the
        // optional screen source. A failure here cannot become a model-editing lock.
        drawing.display_svgs = [Arc::from("<svg>"), Arc::from("<svg>"), Arc::from("<svg>")];
        app.apply_done(done);
        let ctx = egui::Context::default();
        hidden_viewport_capture(&mut app, &ctx);
        assert!(app.plan.error.is_none());
        assert!(app.plan.diagnostics()["last_style_warning"].as_str().unwrap().contains("using paper pens"));
        assert!(app.edit_block_reason().is_none());
        app.plan.fail("synthetic drawing generation failure".into());
        assert!(app.edit_block_reason().is_some(), "genuine drawing errors retain the editing guard");
        assert!(!app.plan.snapshot_ready(app.viewport_rect.unwrap(), 1.0));
    }

    #[test]
    fn local_metadata_and_precision_review_share_the_real_command_path() {
        let mut app = app(false);
        app.authoring.tab = Tab::Project;
        let id = app.session.doc.buildings[0].id.clone();
        app.session.doc.buildings[0].entrances.push(
            serde_json::from_value(json!({"street":"Synthetic street","number":"1A",
            "point":[0,0],"units":[{"floor":"st","door":"tv"}]}))
            .unwrap(),
        );
        let locator = format!("{id}.entrance0");
        app.select_entrance(locator.clone());
        assert!(app.inspector == Inspector::Entrance);
        assert_eq!(app.entrances.selected_id(), Some(locator.as_str()));
        assert!(app.selection.is_none());
        let info = app.session.execute("entrance.get", json!({"id":locator})).unwrap();
        let action = json!({"command":"entrance.set_unit","params":{"id":locator,"expected_address":info["expected_address"],
            "unit_index":0,"expected_unit":info["units"][0]["unit"],"unit":{"floor":"st","door":"th"}}});
        let before = app.session.doc.clone();
        app.run_form(crate::authoring::Run { command: "precision.preview", params: json!({"action":action}) });
        assert!(app.precision_review.is_pending());
        assert_eq!(app.session.doc, before, "review must not apply the metadata edit");
        app.inspect_elements();
        assert!(!app.precision_review.is_pending(), "switching selection cancels the old proposal");
        assert!(!app.entrances.is_selected());
        app.run_form(crate::authoring::Run { command: "precision.preview", params: json!({"action":action}) });
        assert!(app.precision_review.is_pending());
        let mut invalid_action = action.clone();
        invalid_action["params"]["unit"]["door"] = json!("W".repeat(65));
        app.run_form(crate::authoring::Run { command: "precision.preview", params: json!({"action":invalid_action}) });
        assert!(!app.precision_review.is_pending(), "a failed replacement preview must never leave the previous Apply action available");
        assert_eq!(app.session.doc, before);
        app.run_form(crate::authoring::Run { command: "precision.preview", params: json!({"action":action}) });
        assert!(app.precision_review.is_pending());
        app.run(Who::Owner, "view.define", json!({"id":"new-plan","name":"New plan","kind":{"view":"plan","level":0}})).unwrap();
        assert!(!app.precision_review.is_pending(), "a document mutation invalidates the displayed proposal");
        assert_eq!(app.session.doc.buildings[0].entrances[0].units[0].door, "tv");
    }

    #[test]
    fn outline_guides_preserve_exact_points_and_create_only_a_coordination_frame() {
        let mut app = app(false);
        app.authoring.tab = Tab::Project;
        let before = app.session.doc.buildings.clone();
        let line = [[1234.567891, -0.125678], [6543.210987, 99.123456]];
        app.begin_reference_guide(0, Some(line));
        assert_eq!(app.mode, Mode::Drawing);
        assert!(app.drafting.guide_active());
        assert!(app.session.doc.annotations.is_empty(), "a reference starts as an unsaved draft");
        assert!(
            app.session
                .doc
                .views
                .iter()
                .any(|v| v.id == app.drawing && matches!(v.kind, trigon_model::annotation::DrawingViewKind::Plan { level: 0 }))
        );
        assert_eq!(app.session.doc.buildings, before);
        assert_eq!(app.session.doc.journal.last().unwrap().command, "view.define");
    }

    #[test]
    fn guide_floor_follows_the_active_saved_plan_and_refuses_other_planes() {
        let mut app = app(false);
        app.session.execute("view.define", json!({"id":"upper","name":"Upper","kind":{"view":"plan","level":1}})).unwrap();
        app.plan_level = 0;
        app.open_drawing("upper".into());
        assert_eq!(app.plan_level, 1, "opening an upper saved plan synchronizes the active floor");
        assert_eq!(app.reference_level(), Some(1));
        app.plan_level = 0;
        app.drawing = "upper".into(); // Same state transition as the saved-view dropdown.
        assert_eq!(app.reference_level(), Some(1), "a guide resolves the active drawing plane, not the remembered plain plan");
        let facade = app.session.doc.buildings[0].facades[0].id.clone();
        app.session.execute("view.define", json!({"id":"front","name":"Front","kind":{"view":"facade","facade":facade}})).unwrap();
        app.drawing = "front".into();
        assert_eq!(app.reference_level(), None, "facade coordinates cannot silently become plan coordinates");
        app.mode = Mode::ThreeD;
        app.level = None;
        assert_eq!(app.reference_level(), None);
        app.level = Some(1);
        assert_eq!(app.reference_level(), Some(1));
    }

    #[test]
    fn control_and_dropdown_saved_plan_changes_report_the_actual_active_floor() {
        let mut app = app(false);
        app.session.execute("view.define", json!({"id":"upper-control","name":"Upper control","kind":{"view":"plan","level":1}})).unwrap();
        app.plan_level = 0;
        app.mode = Mode::Drawing;
        app.drawing = "upper-control".into();
        app.view_changed(false);
        let state = app.view_state();
        assert_eq!(state["plan_level"], 1, "MCP feedback must describe the active saved plan's floor");
        app.plan_level = 0;
        app.drawing = "upper-control".into();
        app.view_changed(false); // The same transition as the saved-view dropdown.
        assert_eq!(app.plan_level, 1);
        assert_eq!(app.reference_level(), Some(1));
    }

    #[test]
    fn view_tabs_preserve_plan_navigation_selection_and_shared_document() {
        let mut app = app(false);
        app.authoring.tab = Tab::Project;
        app.selection = Some(window(&app));
        let selected = app.selection;
        let document = app.session.doc.clone();
        app.sync_open_views();
        let camera = app.camera;
        app.activate_view(ViewTarget::Plan(0));
        let ctx = egui::Context::default();
        app.plan.set_tree(
            1,
            Arc::new(
                trigon_engine::parse_svg(
                    r#"<svg xmlns="http://www.w3.org/2000/svg" width="1600" height="900"><path d="M10 10L1500 800" stroke="black"/></svg>"#,
                )
                .unwrap(),
            ),
        );
        let mut frame = 0;
        let mut draw = |plan: &mut PlanView, events| {
            frame += 1;
            let mut output = ctx.run_ui(
                egui::RawInput {
                    screen_rect: Some(egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(1200.0, 800.0))),
                    time: Some(f64::from(frame) * 0.05),
                    events,
                    ..Default::default()
                },
                |ui| {
                    plan.ui(ui, false);
                },
            );
            output.textures_delta.clear();
        };
        draw(&mut app.plan, Vec::new());
        let initial = app.plan.navigation();
        for event in crate::control::parse_input("ui.drag", &json!({"from":[600,400],"to":[640,420],"steps":8})).unwrap() {
            draw(&mut app.plan, vec![event]);
        }
        let first = app.plan.navigation();
        assert_ne!(first, initial, "exercise a real drawing pan before switching tabs");
        app.activate_view(ViewTarget::ThreeD(None));
        app.plan.restore_navigation(crate::plan_view::PlanView::new(egui::Context::default()).navigation());
        app.activate_view(ViewTarget::Plan(0));
        assert_eq!(app.plan.navigation(), first);
        assert_eq!(app.camera, camera);
        assert_eq!(app.selection, selected);
        assert_eq!(app.session.doc, document, "view navigation must not create modelling transactions");
        assert_eq!(app.open_views.len(), 2);
        app.activate_view(ViewTarget::ThreeD(None));
        assert_eq!(app.open_views.len(), 2, "returning to a view reuses its tab");
        app.run(Who::Owner, "document.new", json!({})).unwrap();
        assert!(app.open_views.is_empty());
        assert!(app.active_view.is_none(), "views and their navigation cannot leak into another project");
    }

    #[test]
    fn properties_dock_recovers_from_legacy_splits_and_window_resize() {
        let mut app = app(false);
        app.authoring.tab = Tab::Project;
        app.dock_open = true;
        app.dock_properties = true;
        let document = app.session.doc.clone();
        let ctx = egui::Context::default();
        ctx.set_os(egui::os::OperatingSystem::Mac);
        theme::apply(&ctx);
        ctx.data_mut(|data| {
            data.insert_persisted(
                egui::Id::new("properties-pane"),
                egui::containers::panel::PanelState { outer_rect: egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(300.0, 80.0)) },
            );
        });
        let mut failures = Vec::new();
        for size in [egui::vec2(1440.0, 900.0), egui::vec2(820.0, 520.0), egui::vec2(1440.0, 900.0)] {
            for selected in [None, Some(window(&app)), None] {
                app.selection = selected;
                for _ in 0..3 {
                    let mut output = ctx.run_ui(
                        egui::RawInput { screen_rect: Some(egui::Rect::from_min_size(egui::Pos2::ZERO, size)), ..Default::default() },
                        |ui| app.workspace_ui(ui),
                    );
                    output.textures_delta.clear();
                }
                let properties = app.dock_rect.unwrap();
                let minimum = if size.y >= 800.0 { 240.0 } else { 100.0 };
                if properties.height() < minimum {
                    failures.push(format!("Properties collapsed at {size:?}: {properties:?}"));
                }
                if properties.bottom() > size.y || properties.right() > size.x * 0.5 {
                    failures.push(format!("properties dock leaves its bounds: {properties:?}"));
                }
                if app.browser_rect.is_some() {
                    failures.push("closed Browser reported a visible hit region".to_string());
                }
            }
        }
        ctx.data_mut(|data| data.clear());
        assert!(failures.is_empty(), "{}", failures.join("\n"));
        assert_eq!(app.session.doc, document, "resizing/selection cannot edit the building");
    }

    #[test]
    fn architecture_actions_are_visible_inside_the_ribbon() {
        let mut app = app(false);
        app.authoring.tab = Tab::Project;
        let ctx = egui::Context::default();
        theme::apply(&ctx);
        let mut output = ctx.run_ui(
            egui::RawInput { screen_rect: Some(egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(1440.0, 900.0))), ..Default::default() },
            |ui| {
                egui::Panel::top("ribbon-test").show(ui, |ui| app.ribbon(ui));
            },
        );
        let mut failures = Vec::new();
        for label in [
            "Move",
            "Copy",
            "Align",
            "To levels",
            "Guide",
            "Outline",
            "Wall face",
            "Copy levels",
            "Windows",
            "Doors",
            "Balconies",
            "All types",
            "Walls",
            "Stairs",
            "Roof",
            "Slabs",
            "Entrances",
            "Terrain",
        ] {
            let matches = output
                .shapes
                .iter()
                .filter_map(|shape| {
                    if let egui::Shape::Text(text) = &shape.shape
                        && text.galley.text() == label
                    {
                        Some((shape.clip_rect, egui::Rect::from_min_size(text.pos, text.galley.size())))
                    } else {
                        None
                    }
                })
                .collect::<Vec<_>>();
            if matches.is_empty() || !matches.iter().all(|(clip, rect)| clip.contains_rect(*rect)) {
                failures.push(format!("missing or clipped ribbon action {label}: {matches:?}"));
            }
        }
        output.textures_delta.clear();
        ctx.data_mut(|data| data.clear());
        assert!(failures.is_empty(), "{}", failures.join("\n"));
    }

    #[test]
    fn long_project_labels_and_activity_leave_room_for_the_canvas() {
        let mut app = app(true);
        app.authoring.tab = Tab::Project;
        app.session.doc.phases[app.phase].name = "Existing building and proposed works ".repeat(7);
        app.session.doc.buildings[0].storeys[0].name = "Ground floor reference level ".repeat(7);
        app.level = Some(0);
        let index = window(&app);
        Arc::make_mut(&mut app.scene).elements[index].id = "selected-opening-with-a-long-source-identity-".repeat(5);
        app.selection = Some(index);
        app.show_activity = true;
        let document = app.session.doc.clone();
        let ctx = egui::Context::default();
        ctx.set_os(egui::os::OperatingSystem::Mac);
        theme::apply(&ctx);
        ctx.data_mut(|data| {
            for id in ["project-dock", "activity"] {
                data.insert_persisted(
                    egui::Id::new(id),
                    egui::containers::panel::PanelState { outer_rect: egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(700.0, 500.0)) },
                );
            }
        });
        let mut failures = Vec::new();
        for size in [egui::vec2(1440.0, 900.0), egui::vec2(820.0, 520.0), egui::vec2(1440.0, 900.0)] {
            for _ in 0..3 {
                let mut output = ctx
                    .run_ui(egui::RawInput { screen_rect: Some(egui::Rect::from_min_size(egui::Pos2::ZERO, size)), ..Default::default() }, |ui| {
                        app.workspace_ui(ui)
                    });
                output.textures_delta.clear();
            }
            let canvas = app.viewport_rect.unwrap();
            if canvas.width() < 450.0 || canvas.height() < 100.0 || canvas.right() > size.x + 1.0 || canvas.bottom() > size.y + 1.0 {
                failures.push(format!("crowded or overflowing canvas at {size:?}: {canvas:?}"));
            }
        }
        ctx.data_mut(|data| data.clear());
        assert!(failures.is_empty(), "{}", failures.join("\n"));
        assert_eq!(app.session.doc, document);
    }

    #[test]
    fn compact_ribbon_menus_open_the_same_editors_without_editing_the_model() {
        let mut app = app(false);
        app.authoring.tab = Tab::Project;
        app.ribbon_expanded = true;
        let document = app.session.doc.clone();
        let ctx = egui::Context::default();
        ctx.set_os(egui::os::OperatingSystem::Mac);
        theme::apply(&ctx);
        let draw = |app: &mut StudioApp, events| {
            let mut output = ctx.run_ui(
                egui::RawInput {
                    screen_rect: Some(egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(820.0, 520.0))),
                    events,
                    ..Default::default()
                },
                |ui| app.workspace_ui(ui),
            );
            output.textures_delta.clear();
            output.shapes
        };
        let click = |app: &mut StudioApp, label: &str| -> bool {
            let shapes = draw(app, Vec::new());
            // Prefer the ribbon/menu action over a workspace tab with the same label.
            let point = shapes.iter().rev().find_map(|shape| {
                if let egui::Shape::Text(text) = &shape.shape
                    && text.galley.text() == label
                {
                    let point = text.pos + text.galley.size() * 0.5;
                    shape.clip_rect.contains(point).then_some(point)
                } else {
                    None
                }
            });
            let Some(point) = point else { return false };
            for event in [
                egui::Event::PointerMoved(point),
                egui::Event::PointerButton { pos: point, button: egui::PointerButton::Primary, pressed: true, modifiers: Default::default() },
                egui::Event::PointerButton { pos: point, button: egui::PointerButton::Primary, pressed: false, modifiers: Default::default() },
            ] {
                draw(app, vec![event]);
            }
            true
        };
        let terrain = click(&mut app, "Site") && click(&mut app, "Terrain") && app.inspector == Inspector::Terrain;
        let components =
            click(&mut app, "Components") && click(&mut app, "Windows") && app.authoring.tab == Tab::Families && app.authoring.search == "window";
        app.authoring.tab = Tab::Project;
        let outline = click(&mut app, "Set out") && click(&mut app, "Outline") && app.inspector == Inspector::Outline;
        ctx.data_mut(|data| data.clear());
        assert!(terrain && components && outline, "compact menu routes: terrain={terrain}, components={components}, outline={outline}");
        assert_eq!(app.session.doc, document);
    }

    #[test]
    fn idle_properties_follow_the_view_and_selection_restores_element_editing() {
        let mut app = app(false);
        app.authoring.tab = Tab::Project;
        let document = app.session.doc.clone();
        let ctx = egui::Context::default();
        theme::apply(&ctx);
        let draw = |app: &mut StudioApp| {
            let mut output = ctx.run_ui(
                egui::RawInput { screen_rect: Some(egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(320.0, 800.0))), ..Default::default() },
                |ui| app.selection_panel(ui),
            );
            let texts = output
                .shapes
                .iter()
                .filter_map(|shape| if let egui::Shape::Text(text) = &shape.shape { Some(text.galley.text().to_string()) } else { None })
                .collect::<Vec<_>>();
            output.textures_delta.clear();
            texts
        };
        let idle = draw(&mut app);
        app.mode = Mode::Plan;
        let plan = draw(&mut app);
        app.selection = Some(window(&app));
        let element = draw(&mut app);
        app.selection = None;
        app.authoring.choose_wall_alignment();
        let alignment = draw(&mut app);
        ctx.data_mut(|data| data.clear());
        assert!(idle.iter().any(|t| t == "View properties"), "idle Properties must describe the view");
        assert!(!idle.iter().any(|t| t == "X offset"), "disabled Move fields should not dominate an empty selection");
        assert!(plan.iter().any(|t| t.starts_with("Floor plan")), "switching views must update the context");
        assert!(element.iter().any(|t| t == "MODEL PARAMETERS"), "selected elements retain direct editable properties");
        assert!(!alignment.iter().any(|t| t == "View properties"), "an explicitly chosen precision tool retains its form");
        assert_eq!(app.session.doc, document);
    }

    #[test]
    fn command_search_hint_matches_the_desktop_platform() {
        let mut app = app(false);
        let mut found = Vec::new();
        for (os, expected) in [(egui::os::OperatingSystem::Mac, "⌘ K"), (egui::os::OperatingSystem::Windows, "Ctrl+K")] {
            let ctx = egui::Context::default();
            ctx.set_os(os);
            theme::apply(&ctx);
            let mut output = ctx.run_ui(
                egui::RawInput { screen_rect: Some(egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(1440.0, 80.0))), ..Default::default() },
                |ui| app.app_bar(ui),
            );
            let matches = output.shapes.iter().any(|shape| matches!(&shape.shape, egui::Shape::Text(text) if text.galley.text() == expected));
            output.textures_delta.clear();
            ctx.data_mut(|data| data.clear());
            found.push((os, matches));
        }
        assert!(found.iter().all(|(_, found)| *found), "wrong platform shortcut hints: {found:?}");
    }

    #[test]
    fn canvas_dominates_and_optional_panels_stay_inside_the_window() {
        for os in [egui::os::OperatingSystem::Mac, egui::os::OperatingSystem::Windows, egui::os::OperatingSystem::Nix] {
            let mut app = app(false);
            app.authoring.tab = Tab::Project;
            let document = app.session.doc.clone();
            let ctx = egui::Context::default();
            ctx.set_os(os);
            theme::apply(&ctx);
            let mut failures = Vec::new();
            for size in [egui::vec2(1440.0, 900.0), egui::vec2(820.0, 520.0), egui::vec2(1600.0, 1076.0)] {
                for (dock, properties, focus) in [(false, false, false), (true, false, false), (true, true, false), (true, true, true)] {
                    app.dock_open = dock;
                    app.dock_properties = properties;
                    app.focus_canvas = focus;
                    let mut output = ctx.run_ui(
                        egui::RawInput { screen_rect: Some(egui::Rect::from_min_size(egui::Pos2::ZERO, size)), ..Default::default() },
                        |ui| app.workspace_ui(ui),
                    );
                    output.textures_delta.clear();
                    let canvas = app.viewport_rect.unwrap();
                    if canvas.width() < 450.0 || canvas.height() < 100.0 || canvas.right() > size.x + 1.0 || canvas.bottom() > size.y + 1.0 {
                        failures.push(format!("unusable {os:?} canvas at {size:?}: {canvas:?}"));
                    }
                    if !dock || focus {
                        let area = canvas.area() / (size.x * size.y);
                        let minimum = if size.y >= 800.0 { 0.73 } else { 0.60 };
                        if area < minimum {
                            failures.push(format!("chrome dominates at {size:?}: canvas area {area:.3}"));
                        }
                        if app.dock_rect.is_some() || app.browser_rect.is_some() {
                            failures.push("hidden panel retained a hit rectangle".into());
                        }
                    }
                    if dock && !properties && !focus && app.browser_rect.is_none_or(|r| r.height() < 150.0 || r.bottom() > size.y) {
                        failures.push("visible Browser is unusable".into());
                    }
                }
            }
            ctx.data_mut(|data| data.clear());
            assert!(failures.is_empty(), "{}", failures.join("\n"));
            assert_eq!(app.session.doc, document);
        }
    }

    #[test]
    fn human_canvas_outline_creates_a_real_building_and_undo_restores_the_document() {
        let session = Session::new(Box::new(trigon_data::Live::new(trigon_data::Credentials::default())));
        let ctx = egui::Context::default();
        ctx.set_os(egui::os::OperatingSystem::Mac);
        theme::apply(&ctx);
        let mut app = StudioApp::with_context(ctx.clone(), None, session);
        complete(&mut app);
        let before = app.session.doc.clone();
        app.begin_model_tool(false);
        complete(&mut app);
        let mut time = 0.0;
        let mut draw = |app: &mut StudioApp, events| {
            time += 0.05;
            let mut output = ctx.run_ui(
                egui::RawInput {
                    screen_rect: Some(egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(1440.0, 900.0))),
                    events,
                    time: Some(time),
                    ..Default::default()
                },
                |ui| app.workspace_ui(ui),
            );
            output.textures_delta.clear();
        };
        draw(&mut app, Vec::new());
        for point in [[-4000.0, -3000.0], [4000.0, -3000.0], [4000.0, 3000.0], [-4000.0, 3000.0]] {
            let rect = app.viewport_rect.unwrap();
            let pos =
                rect.center() + app.empty_pan + egui::vec2((point[0] / app.empty_mm_per_point) as f32, (-point[1] / app.empty_mm_per_point) as f32);
            for event in [
                egui::Event::PointerMoved(pos),
                egui::Event::PointerButton { pos, button: egui::PointerButton::Primary, pressed: true, modifiers: Default::default() },
                egui::Event::PointerButton { pos, button: egui::PointerButton::Primary, pressed: false, modifiers: Default::default() },
            ] {
                draw(&mut app, vec![event]);
            }
        }
        let sketch_feedback = app.authoring_feedback();
        let uncommitted = app.session.doc == before;
        draw(
            &mut app,
            vec![egui::Event::Key { key: egui::Key::Enter, physical_key: None, pressed: true, repeat: false, modifiers: Default::default() }],
        );
        ctx.data_mut(|data| data.clear());
        assert!(uncommitted, "partial sketch/repaint edited the document");
        assert_eq!(sketch_feedback["model_tool"]["draft_points"].as_array().unwrap().len(), 4);
        assert_eq!(app.session.doc.buildings.len(), 1, "human clicks + Enter must reach building.add: {:?}", app.status);
        assert_eq!(app.session.doc.journal.last().unwrap().command, "building.add");
        assert_eq!(app.session.doc.buildings[0].footprint.len(), 4);
        app.run(Who::Owner, "edit.undo", json!({})).unwrap();
        assert_eq!(app.session.doc, before);
    }

    #[test]
    fn drawing_shortcut_recovers_after_the_active_saved_view_is_removed() {
        let mut app = app(false);
        for id in ["a", "b"] {
            app.run(Who::Owner, "view.define", json!({"id":id,"name":id,"kind":{"view":"plan","level":0}})).unwrap();
        }
        app.open_drawing("a".into());
        app.run(Who::Owner, "view.remove", json!({"id":"a"})).unwrap();
        let ctx = egui::Context::default();
        theme::apply(&ctx);
        let mut frame = 0;
        let mut draw = |app: &mut StudioApp, events| {
            frame += 1;
            let mut output = ctx.run_ui(
                egui::RawInput {
                    screen_rect: Some(egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(1200.0, 300.0))),
                    time: Some(f64::from(frame) * 0.05),
                    events,
                    ..Default::default()
                },
                |ui| {
                    egui::CentralPanel::default().show(ui, |ui| {
                        app.view_tabs(ui);
                        app.view_controls(ui);
                    });
                },
            );
            output.textures_delta.clear();
            output.shapes.into_iter().map(|shape| shape.shape).collect::<Vec<_>>()
        };
        let shapes = draw(&mut app, Vec::new());
        assert_eq!(app.mode, Mode::ThreeD);
        fn find(shapes: &[egui::Shape]) -> Option<egui::Pos2> {
            shapes.iter().find_map(|shape| match shape {
                egui::Shape::Text(text) if text.galley.text() == "Drawing" => Some(text.pos + text.galley.size() * 0.5),
                egui::Shape::Vec(shapes) => find(shapes),
                _ => None,
            })
        }
        let point = find(&shapes).unwrap();
        for event in [
            egui::Event::PointerMoved(point),
            egui::Event::PointerButton { pos: point, button: egui::PointerButton::Primary, pressed: true, modifiers: Default::default() },
            egui::Event::PointerButton { pos: point, button: egui::PointerButton::Primary, pressed: false, modifiers: Default::default() },
        ] {
            draw(&mut app, vec![event]);
        }
        assert_eq!(app.drawing, "b", "the shortcut must choose a remaining saved drawing rather than reopen a deleted ID");
        assert_eq!(app.mode, Mode::Drawing);
        ctx.data_mut(|data| data.clear());
    }

    #[test]
    fn viewport_pointer_drag_reserves_the_handle_and_commits_one_source_edit() {
        let mut app = app(false);
        app.authoring.tab = Tab::Project;
        app.selection = Some(window(&app));
        app.focus(app.selection.unwrap());
        let before = app.session.doc.clone();
        let camera = app.camera.view_proj(1.0);
        let ctx = egui::Context::default();
        let mut frame = 0;
        let mut draw = |app: &mut StudioApp, events: Vec<egui::Event>| {
            frame += 1;
            let input = egui::RawInput {
                screen_rect: Some(egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(1200.0, 800.0))),
                time: Some(frame as f64 * 0.05),
                events,
                ..Default::default()
            };
            let mut output = ctx.run_ui(input, |ui| app.viewport_3d(ui));
            output.textures_delta.clear();
        };
        draw(&mut app, vec![]);
        let handle = app.projected_handles(app.viewport_rect.unwrap()).into_iter().find(|h| h.handle.key == "width_mm").unwrap();
        let from = handle.mapping.screen(0.0).unwrap();
        let to = handle.mapping.screen(120.0).unwrap();
        let expected = handle.handle.value + 120.0 * handle.handle.gain;
        let events = crate::control::parse_input("ui.drag", &json!({"from":[from.x,from.y],"to":[to.x,to.y],"steps":4})).unwrap();
        for event in events {
            draw(&mut app, vec![event]);
        }
        assert_eq!(app.camera.view_proj(1.0), camera, "a handle drag must not orbit the model");
        assert_eq!(app.session.doc.journal.len(), before.journal.len() + 1, "release must make exactly one command");
        complete(&mut app);
        let selected = app.scene.elements[app.selection.unwrap()].id.clone();
        let width = app.properties.descriptor(&selected).unwrap()["properties"].as_array().unwrap().iter().find(|p| p["key"] == "width_mm").unwrap()
            ["value"]
            .as_f64()
            .unwrap();
        assert!((width - expected).abs() < 0.1, "{width} expected {expected}");
        app.run(Who::Owner, "edit.undo", json!({})).unwrap();
        assert_eq!(app.session.doc, before);
    }

    #[test]
    fn one_property_edit_updates_the_shared_model_scene_and_plan_and_undo_restores_them() {
        let mut app = app(false);
        app.authoring.tab = Tab::Project;
        let index = window(&app);
        let id = app.scene.elements[index].id.clone();
        app.selection = Some(index);
        let original = app.session.doc.clone();
        let bounds = app.scene.elements[index].bounds;
        let drawing = trigon_engine::plan_drawing(&original, 0, app.view()).unwrap().svg;
        let descriptor = app.properties.descriptor(&id).unwrap();
        assert_eq!(descriptor["editable"], true);
        let width = descriptor["properties"].as_array().unwrap().iter().find(|p| p["key"] == "width_mm").unwrap()["value"].as_f64().unwrap();
        app.apply_property_edit(properties::Edit { id: id.clone(), properties: json!({"width_mm":width+200.0}) });
        assert_eq!(app.session.doc.journal.len(), original.journal.len() + 1, "one completed edit is one command");
        assert!(app.building());
        assert!(app.properties.descriptor(&id).is_none());
        complete(&mut app);
        let selected = app.selection.unwrap();
        assert_eq!(app.scene.elements[selected].id, id);
        assert_ne!(app.scene.elements[selected].bounds, bounds);
        let updated = trigon_engine::plan_drawing(&app.session.doc, 0, app.view()).unwrap().svg;
        assert_ne!(updated, drawing);
        app.run(Who::Owner, "edit.undo", json!({})).unwrap();
        complete(&mut app);
        assert_eq!(app.session.doc, original);
        assert_eq!(trigon_engine::plan_drawing(&app.session.doc, 0, app.view()).unwrap().svg, drawing);
        assert_eq!(app.scene.elements[app.selection.unwrap()].bounds, bounds);
        app.run(Who::Owner, "edit.redo", json!({})).unwrap();
        complete(&mut app);
        assert_eq!(trigon_engine::plan_drawing(&app.session.doc, 0, app.view()).unwrap().svg, updated);
        let before = app.session.doc.clone();
        app.apply_property_edit(properties::Edit { id, properties: json!({"width_mm":f64::MAX}) });
        assert_eq!(app.session.doc, before);
        assert!(app.status.as_ref().is_some_and(|(_, error)| *error));
    }

    #[test]
    fn historical_and_pending_views_cannot_commit_property_edits_or_offer_handles() {
        let mut app = app(true);
        app.authoring.tab = Tab::Project;
        let index = window(&app);
        let id = app.scene.elements[index].id.clone();
        app.selection = Some(index);
        app.request_rebuild(None);
        let before = app.session.doc.clone();
        app.apply_property_edit(properties::Edit { id: id.clone(), properties: json!({"width_mm":1400.0}) });
        assert_eq!(app.session.doc, before);
        complete(&mut app);
        app.phase = 0;
        app.view_changed(true);
        complete(&mut app);
        app.selection = Some(window(&app));
        let selected = app.scene.elements[app.selection.unwrap()].id.clone();
        assert!(app.projected_handles(egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(1000.0, 700.0))).is_empty());
        app.apply_property_edit(properties::Edit { id: selected, properties: json!({"width_mm":1400.0}) });
        assert_eq!(app.session.doc, before);
    }

    #[test]
    fn schedule_selection_uses_existing_properties_and_refreshes_after_edit_undo_redo() {
        let mut app = app(false);
        let before = app.schedules.data().unwrap().clone();
        app.authoring.targets = "previous selection".into();
        let index = window(&app);
        let id = app.scene.elements[index].id.clone();
        app.schedule_action(schedules::Action::Select(index));
        assert_eq!(app.selection, Some(index));
        assert!(app.authoring.tab == Tab::Schedules, "the table must remain visible while editing");
        assert!(app.authoring.targets.is_empty());
        assert!(app.edit_block_reason().is_none());
        assert_eq!(app.authoring.selected(app.scene.elements.get(index)), vec![id.clone()]);

        app.run(Who::Owner, "edit.copy", json!({"targets":[id],"delta":[1800,0,0],"count":1})).unwrap();
        assert!(app.building());
        assert!(app.schedules.data().is_none());
        assert!(app.edit_block_reason().is_some());
        complete(&mut app);
        let changed = app.schedules.data().unwrap().clone();
        assert_eq!(changed.totals.instances, before.totals.instances + 1);
        assert!((changed.totals.area_m2 - before.totals.area_m2 - 2.16).abs() < 1e-9);
        assert!(app.authoring.tab == Tab::Schedules);
        assert!(app.edit_block_reason().is_none());

        app.run(Who::Owner, "edit.undo", json!({})).unwrap();
        assert!(app.schedules.data().is_none());
        complete(&mut app);
        assert_eq!(app.schedules.data(), Some(&before));
        app.run(Who::Owner, "edit.redo", json!({})).unwrap();
        assert!(app.schedules.data().is_none());
        complete(&mut app);
        assert_eq!(app.schedules.data(), Some(&changed));
        let generation = app.generation;
        app.request_plan();
        assert_eq!(app.schedules.data(), Some(&changed), "a drawing-only request does not invalidate quantities");
        assert!(app.generation > generation);
    }

    #[test]
    fn selecting_an_upper_storey_schedule_row_preserves_the_ready_or_pending_plan() {
        for pending in [false, true] {
            let mut app = app(false);
            app.mode = Mode::Plan;
            app.plan_level = 0;
            app.request_plan();
            let mut done = Some(crate::worker::run(&Job {
                generation: app.generation,
                doc: Arc::new(app.session.doc.clone()),
                view: app.view(),
                want_scene: false,
                phase_colors: app.phase_colors,
                drawing: app.drawing_request(),
            }));
            if !pending {
                app.apply_done(done.take().unwrap());
            }
            let row = app.schedules.data().unwrap().rows.iter().find(|row| row.level == 1).unwrap();
            let index = schedules::resolve_row(&app.scene, row).unwrap();
            app.schedule_action(schedules::Action::Select(index));
            app.authoring.tab = Tab::Project; // Return to Model without changing the view controls.
            assert_eq!(app.selection, Some(index));
            assert_eq!(app.plan_level, 0, "selection must not relabel a cached plan or abandon a pending plan");
            assert_eq!(app.drawing_request(), Some(DrawingRequest::Plan(0)));
            if let Some(done) = done {
                app.apply_done(done);
            }
            assert!(!app.drawing_plan());
            assert_eq!(app.plan_generation, app.plan_wanted);
            assert!(app.plan.screen_mapping(egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(800.0, 600.0))).is_some());
        }
    }

    #[test]
    fn obsolete_phase_results_and_errors_are_rejected_and_new_documents_clear_inventory() {
        let mut app = app(true);
        app.request_rebuild(None);
        let stale = result(&app);
        let displayed = app.scene_generation;
        app.filter = PhaseFilter::New;
        app.view_changed(true);
        app.apply_done(stale);
        assert_eq!(app.scene_generation, displayed, "an obsolete model must not replace the displayed scene");
        assert!(app.schedules.data().is_none());
        assert!(app.building());
        complete(&mut app);
        assert!(app.schedules.data().unwrap().rows.iter().all(|r| r.status == "new" || !r.external));

        app.schedule_action(schedules::Action::Select(window(&app)));
        app.request_rebuild(None);
        let mut failure = result(&app);
        failure.scene = Some(Err("test build error".into()));
        app.apply_done(failure);
        assert!(app.schedules.data().is_none());
        assert_eq!(app.build_error.as_deref(), Some("test build error"));
        assert!(app.selection.is_none());
        assert!(app.edit_block_reason().is_some());

        app.request_rebuild(None);
        app.request_plan();
        complete(&mut app); // A coalesced scene/plan result may be newer than scene_wanted.
        assert!(app.schedules.data().is_some());
        app.request_rebuild(None);
        let old_document = result(&app);
        app.schedules.filters.search = "window".into();
        app.run(Who::Owner, "document.new", json!({})).unwrap();
        assert!(app.schedules.data().is_none());
        assert!(app.schedules.filters.search.is_empty());
        app.apply_done(old_document);
        assert!(app.schedules.data().is_none());
        assert!(app.selection.is_none());
        complete(&mut app);
        assert_eq!(app.schedules.data(), Some(&trigon_engine::OpeningSchedule::default()));
    }

    #[test]
    fn ambiguous_same_type_phase_ids_cannot_select_focus_or_edit_even_through_a_group() {
        let mut app = app(true);
        app.filter = PhaseFilter::All;
        app.view_changed(true);
        complete(&mut app);
        let index = app.scene.find("b.f1.L0.B0").unwrap();
        let target = app.camera.target;
        app.schedule_action(schedules::Action::Select(index));
        assert!(app.selection.is_none());
        app.schedule_action(schedules::Action::Show(index));
        assert_eq!(app.camera.target, target);
        assert!(app.authoring.tab == Tab::Schedules);
        app.selection = Some(index); // A viewport pick or an older selection must not bypass the guard.
        assert_eq!(app.edit_block_reason(), Some(schedules::AMBIGUOUS));
        app.selection = None;
        app.session.doc.groups.push(trigon_model::doc::ModelGroup {
            id: "duplicate".into(),
            name: "Duplicate".into(),
            targets: vec!["b.f1.L0.B0".into()],
        });
        app.authoring.targets = "group:duplicate".into();
        assert_eq!(app.edit_block_reason(), Some(schedules::AMBIGUOUS));
        app.authoring.targets.clear();
        app.filter = PhaseFilter::Complete;
        app.view_changed(true);
        complete(&mut app);
        let index = window(&app);
        app.schedule_action(schedules::Action::Select(index));
        assert_eq!(app.selection, Some(index));
        assert!(app.edit_block_reason().is_none());
    }

    #[test]
    fn historical_and_demolished_openings_are_read_only_and_show_in_3d_uses_existing_focus() {
        let mut app = app(true);
        app.phase = 0;
        app.view_changed(true);
        complete(&mut app);
        let index = window(&app);
        app.schedule_action(schedules::Action::Select(index));
        assert!(app.edit_block_reason().unwrap().contains("active phase"));
        app.phase = 1;
        app.filter = PhaseFilter::Demolition;
        app.view_changed(true);
        complete(&mut app);
        let index = window(&app);
        app.schedule_action(schedules::Action::Select(index));
        assert!(app.edit_block_reason().unwrap().contains("active phase"));
        app.filter = PhaseFilter::Complete;
        app.view_changed(true);
        complete(&mut app);
        let index = window(&app);
        app.mode = Mode::Plan;
        app.level = Some(0);
        app.camera.target = [500.0; 3];
        app.schedule_action(schedules::Action::Show(index));
        assert_eq!(app.mode, Mode::ThreeD);
        assert!(app.authoring.tab == Tab::Project);
        assert_eq!(app.selection, Some(index));
        assert!(app.level.is_none(), "the previously active cut must not hide the focused row");
        assert_ne!(app.camera.target, [500.0; 3]);
        assert!(app.redraw && app.selection_dirty);
    }
}
