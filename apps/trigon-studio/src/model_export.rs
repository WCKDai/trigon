//! Model exchange from the same typed commands used by CLI/MCP. Each job owns a
//! document snapshot; geometry generation and file writing stay off the UI thread.

use std::path::{Path, PathBuf};
use std::sync::mpsc::{Receiver, TryRecvError, channel};
use std::time::Instant;

use eframe::egui;
use serde_json::{Value, json};
use trigon_model::build::{PhaseFilter, View};
use trigon_model::{Document, Storey};

use crate::app::filter_name;
use crate::theme;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Format {
    #[default]
    Ifc,
    Glb,
}

impl Format {
    pub fn label(self) -> &'static str {
        match self {
            Self::Ifc => "IFC4 BIM (.ifc)",
            Self::Glb => "GLB 3D model (.glb)",
        }
    }

    fn extension(self) -> &'static str {
        match self {
            Self::Ifc => "ifc",
            Self::Glb => "glb",
        }
    }

    fn command(self) -> &'static str {
        match self {
            Self::Ifc => "export.ifc",
            Self::Glb => "export.gltf",
        }
    }
}

#[derive(Clone)]
struct Cut {
    building: String,
    level: usize,
    storeys: Vec<Storey>,
}

#[derive(Clone, Debug)]
pub struct Request {
    pub command: &'static str,
    pub params: Value,
    pub scope: String,
}

#[derive(Clone, Debug)]
pub struct Completed {
    pub request: Request,
    pub result: Result<Value, String>,
    pub ms: f64,
}

pub struct ModelExport {
    pub open: bool,
    scope_valid: bool,
    format: Format,
    phase: String,
    filter: PhaseFilter,
    use_cut: bool,
    cut: Option<Cut>,
    source: String,
    destination: String,
    running: Option<Receiver<Completed>>,
    pending: Option<Request>,
    last: Option<Completed>,
    error: Option<String>,
}

impl Default for ModelExport {
    fn default() -> Self {
        Self {
            open: false,
            scope_valid: false,
            format: Format::Ifc,
            phase: String::new(),
            filter: PhaseFilter::Complete,
            use_cut: false,
            cut: None,
            source: String::new(),
            destination: String::new(),
            running: None,
            pending: None,
            last: None,
            error: None,
        }
    }
}

impl ModelExport {
    pub fn show(&mut self, format: Format, doc: &Document, view: View, level: Option<usize>, path: Option<&Path>) {
        self.open = true;
        if self.busy() {
            return;
        }
        self.scope_valid = true;
        self.format = format;
        self.phase = doc.phases.get(view.phase).map_or_else(String::new, |phase| phase.id.clone());
        self.filter = view.filter;
        self.use_cut = false;
        self.cut = level.and_then(|level| {
            let building = (doc.buildings.len() == 1).then(|| &doc.buildings[0])?;
            building.storeys.get(level)?;
            Some(Cut { building: building.id.clone(), level, storeys: building.storeys.clone() })
        });
        let stem = path.and_then(Path::file_stem).and_then(|name| name.to_str()).unwrap_or("model");
        self.source = path.and_then(Path::file_name).map(|name| name.to_string_lossy().into_owned()).unwrap_or_else(|| "Unsaved project".into());
        self.destination = format!("{stem}.{}", format.extension());
        self.last = None;
        self.error = None;
    }

    pub fn busy(&self) -> bool {
        self.running.is_some()
    }

    pub fn project_replaced(&mut self) {
        // An active job keeps its own source/scope. A pending dialog must not
        // export a newly opened project under the previous project's name.
        self.scope_valid = false;
        if !self.busy() {
            self.open = false;
            self.phase.clear();
            self.cut = None;
            self.last = None;
            self.source.clear();
            self.error = None;
        }
    }

    pub fn feedback(&self) -> Value {
        json!({
            "open": self.open,
            "running": self.busy(),
            "scope_valid": self.scope_valid,
            "format": self.format.extension(),
            "phase": self.phase,
            "filter": filter_name(self.filter),
            "cut_level": self.cut.as_ref().filter(|_| self.use_cut).map(|cut| cut.level),
            "source": self.source,
            "scope": self.pending.as_ref().or(self.last.as_ref().map(|done| &done.request)).map(|request| &request.scope),
            "written": self.last.as_ref().and_then(|done| done.result.as_ref().ok()).and_then(|result| result.get("written")),
            "error": self.error,
        })
    }

    fn request(&self, doc: &Document, path: &Path) -> Result<Request, String> {
        if !self.scope_valid {
            return Err("The project changed. Reopen Export to choose the new project's scope.".into());
        }
        let path = path.to_str().ok_or("Choose a destination with a valid Unicode file name")?;
        if Path::new(path).extension().and_then(|ext| ext.to_str()) != Some(self.format.extension()) {
            return Err(format!("The destination must end in .{}", self.format.extension()));
        }
        if doc.phases.iter().filter(|phase| phase.id == self.phase).count() != 1 {
            return Err("The selected phase is no longer available. Choose a phase before exporting.".into());
        }
        let mut params = json!({"path":path,"phase":self.phase,"filter":filter_name(self.filter)});
        let mut scope = format!("{} · phase {} · {}", self.source, self.phase, filter_name(self.filter));
        if self.use_cut {
            if self.format != Format::Glb {
                return Err("A storey cut is available for GLB only; IFC exports the complete storey geometry.".into());
            }
            let cut = self.cut.as_ref().ok_or("Choose a single-building 3D cut before opening Export.")?;
            let building = doc.buildings.first().filter(|building| doc.buildings.len() == 1 && building.id == cut.building);
            let building = building.ok_or("The cut's building has changed. Reopen Export to choose the current 3D cut.")?;
            if building.storeys != cut.storeys {
                return Err(format!("{}: storeys changed since Export was opened. Reopen Export to choose the current 3D cut.", cut.building));
            }
            let storey = building.storeys.get(cut.level).ok_or("The chosen cut storey no longer exists.")?;
            params["cut_level"] = json!(cut.level);
            scope.push_str(&format!(" · cut 1200 mm above {} ({})", storey.name, cut.building));
        } else {
            scope.push_str(" · full model");
        }
        Ok(Request { command: self.format.command(), params, scope })
    }

    pub fn start(&mut self, doc: &Document, path: PathBuf, ctx: &egui::Context) -> Result<(), String> {
        let result = self.start_inner(doc, path, ctx);
        if let Err(error) = &result {
            self.error = Some(error.clone());
        }
        result
    }

    fn start_inner(&mut self, doc: &Document, path: PathBuf, ctx: &egui::Context) -> Result<(), String> {
        if self.busy() {
            return Err("An export is already running. Wait for it to finish.".into());
        }
        let request = self.request(doc, &path)?;
        let snapshot = doc.clone();
        let (sender, receiver) = channel();
        let wake = ctx.clone();
        let pending = request.clone();
        std::thread::Builder::new()
            .name("trigon-model-export".into())
            .spawn(move || {
                let result = execute_snapshot(snapshot, request);
                let _ = sender.send(result);
                wake.request_repaint();
            })
            .map_err(|error| format!("Could not start export: {error}"))?;
        self.running = Some(receiver);
        self.pending = Some(pending);
        self.last = None;
        self.error = None;
        Ok(())
    }

    pub fn poll(&mut self) -> Option<Completed> {
        let result = match self.running.as_ref()?.try_recv() {
            Ok(done) => done,
            Err(TryRecvError::Empty) => return None,
            Err(TryRecvError::Disconnected) => {
                Completed { request: self.pending.clone()?, result: Err("The export worker stopped before returning a result.".into()), ms: 0.0 }
            }
        };
        self.running = None;
        self.pending = None;
        self.error = result.result.as_ref().err().cloned();
        self.last = Some(result.clone());
        Some(result)
    }

    /// A modal keeps sketch/annotation input behind the export controls. Closing
    /// this dialog leaves an already-running snapshot export alive.
    pub fn ui(&mut self, ctx: &egui::Context, doc: &Document) -> Option<Result<(), String>> {
        if !self.open {
            return None;
        }
        let mut export = false;
        let response = egui::Modal::new(egui::Id::new("model-export")).show(ctx, |ui| {
            ui.set_width((ctx.content_rect().width() - 48.0).clamp(240.0, 460.0));
            ui.heading("Export model");
            ui.label(&self.source);
            ui.separator();
            egui::ScrollArea::vertical().max_height((ctx.content_rect().height() - 210.0).max(120.0)).show(ui, |ui| {
                ui.add_enabled_ui(!self.busy() && self.scope_valid, |ui| {
                    ui.horizontal_wrapped(|ui| {
                        for format in [Format::Ifc, Format::Glb] {
                            if ui.selectable_value(&mut self.format, format, format.label()).changed() {
                                self.use_cut = false;
                                let path = PathBuf::from(&self.destination).with_extension(format.extension());
                                self.destination = path.to_string_lossy().into_owned();
                            }
                        }
                    });
                    ui.add_space(8.0);
                    let label = doc.phases.iter().find(|phase| phase.id == self.phase).map_or("Choose phase", |phase| phase.name.as_str());
                    egui::ComboBox::from_id_salt("model-export-phase").selected_text(label).show_ui(ui, |ui| {
                        for phase in &doc.phases {
                            ui.selectable_value(&mut self.phase, phase.id.clone(), &phase.name);
                        }
                    });
                    egui::ComboBox::from_id_salt("model-export-filter").selected_text(filter_label(self.filter)).show_ui(ui, |ui| {
                        for filter in [PhaseFilter::Complete, PhaseFilter::New, PhaseFilter::Demolition, PhaseFilter::All] {
                            ui.selectable_value(&mut self.filter, filter, filter_label(filter));
                        }
                    });
                    ui.add_space(8.0);
                    ui.add_enabled(self.format == Format::Glb && self.cut.is_some(), egui::Checkbox::new(&mut self.use_cut, "Use the 3D storey cut"));
                    if self.use_cut {
                        if let Some(cut) = &self.cut {
                            let label = cut.storeys.get(cut.level).map_or("Unavailable storey", |storey| storey.name.as_str());
                            ui.weak(format!("1200 mm above {label} · {}", cut.building));
                        }
                    } else {
                        ui.weak("Full model in the chosen phase and visibility filter.");
                    }
                    ui.weak(match self.format {
                        Format::Ifc => "BIM exchange: building and site elements. The viewport's storey cut and camera do not crop IFC.",
                        Format::Glb => "3D geometry and materials, including linked reference meshes read at export time.",
                    });
                    #[cfg(not(any(target_os = "macos", target_os = "windows")))]
                    {
                        ui.add_space(8.0);
                        ui.label("Destination");
                        ui.text_edit_singleline(&mut self.destination);
                    }
                });
                if let Some(request) = &self.pending {
                    ui.add_space(8.0);
                    ui.horizontal(|ui| {
                        ui.spinner();
                        ui.label("Exporting snapshot…");
                    });
                    ui.weak(&request.scope);
                }
                if !self.scope_valid {
                    ui.weak("The current project changed. Reopen Export before starting another export.");
                }
                if let Some(error) = &self.error {
                    ui.add_space(8.0);
                    ui.colored_label(ui.visuals().error_fg_color, error);
                } else if let Some(done) = &self.last
                    && let Ok(result) = &done.result
                {
                    ui.add_space(8.0);
                    ui.label("Export complete");
                    if let Some(path) = result.get("written").and_then(Value::as_str) {
                        ui.add(egui::Label::new(path).wrap());
                    }
                    ui.weak(&done.request.scope);
                }
            });
            ui.separator();
            ui.horizontal_wrapped(|ui| {
                if ui.add_enabled_ui(!self.busy() && self.scope_valid, |ui| theme::primary(ui, "Choose destination and export…")).inner.clicked() {
                    export = true;
                }
                if ui.button(if self.busy() { "Hide" } else { "Close" }).clicked() {
                    self.open = false;
                }
            });
        });
        if response.should_close() {
            self.open = false;
        }
        if export {
            #[cfg(any(target_os = "macos", target_os = "windows"))]
            let path = rfd::FileDialog::new()
                .set_title(format!("Export {}", self.format.label()))
                .add_filter(self.format.label(), &[self.format.extension()])
                .set_file_name(&self.destination)
                .save_file();
            #[cfg(not(any(target_os = "macos", target_os = "windows")))]
            let path = (!self.destination.trim().is_empty()).then(|| PathBuf::from(&self.destination));
            return path.map(|path| self.start(doc, path, ctx));
        }
        None
    }
}

fn filter_label(filter: PhaseFilter) -> &'static str {
    match filter {
        PhaseFilter::Complete => "Complete construction",
        PhaseFilter::New => "New construction",
        PhaseFilter::Demolition => "Demolition",
        PhaseFilter::All => "All elements",
    }
}

fn execute_snapshot(doc: Document, request: Request) -> Completed {
    let since = Instant::now();
    let mut session = trigon_engine::Session::new(Box::new(trigon_data::Live::new(trigon_data::Credentials::default())));
    session.doc = doc;
    let result = session.execute(request.command, request.params.clone()).map_err(|error| error.to_string());
    Completed { request, result, ms: since.elapsed().as_secs_f64() * 1000.0 }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicU64, Ordering};
    use std::time::Duration;

    struct Files(PathBuf);

    impl Files {
        fn new() -> Self {
            static NEXT: AtomicU64 = AtomicU64::new(0);
            let path = std::env::temp_dir().join(format!("trigon-model-export-{}-{}", std::process::id(), NEXT.fetch_add(1, Ordering::Relaxed)));
            std::fs::create_dir_all(&path).unwrap();
            Self(path)
        }

        fn path(&self, name: &str) -> PathBuf {
            self.0.join(name)
        }
    }

    impl Drop for Files {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    fn open(format: Format, doc: &Document) -> ModelExport {
        let mut export = ModelExport::default();
        export.show(format, doc, View { phase: 0, filter: PhaseFilter::Complete }, Some(0), Some(Path::new("source.trigon")));
        export
    }

    #[test]
    fn export_uses_the_selected_view_phase_and_only_an_explicit_glb_cut() {
        let session = crate::schedules::tests::phase_session();
        assert_eq!(session.doc.active_index(), 1);
        for format in [Format::Ifc, Format::Glb] {
            let mut export = open(format, &session.doc);
            export.filter = PhaseFilter::Demolition;
            let request = export.request(&session.doc, Path::new(&format!("file.{}", format.extension()))).unwrap();
            assert_eq!(request.command, format.command());
            assert_eq!(request.params["phase"], "existing");
            assert_eq!(request.params["filter"], "demolition");
            assert!(request.params.get("cut_level").is_none());
            export.use_cut = true;
            if format == Format::Ifc {
                assert!(export.request(&session.doc, Path::new("file.ifc")).unwrap_err().contains("GLB only"));
            } else {
                let request = export.request(&session.doc, Path::new("file.glb")).unwrap();
                assert_eq!(request.params["cut_level"], 0);
                assert!(request.scope.contains("1200 mm above Stueetage"));
            }
        }
    }

    #[test]
    fn pending_cut_refuses_basement_insertion_and_a_replaced_building() {
        let mut session = crate::schedules::tests::session();
        let mut export = open(Format::Glb, &session.doc);
        export.use_cut = true;
        session.execute("building.add_basement", json!({"building":"b","floor":-2500})).unwrap();
        let before = session.doc.clone();
        let error = export.request(&session.doc, Path::new("file.glb")).unwrap_err();
        assert!(error.contains("b: storeys changed"), "{error}");
        assert_eq!(session.doc, before);
        export.use_cut = false;
        assert!(export.request(&session.doc, Path::new("file.glb")).is_ok(), "full-model export has no captured storey scope");
        export.use_cut = true;
        session.doc.buildings[0].id = "replacement".into();
        assert!(export.request(&session.doc, Path::new("file.glb")).unwrap_err().contains("building has changed"));
    }

    #[test]
    fn removed_phase_and_wrong_extension_refuse_before_writing() {
        let mut session = crate::schedules::tests::session();
        let mut export = open(Format::Ifc, &session.doc);
        let files = Files::new();
        let path = files.path("keep.trigon");
        std::fs::write(&path, b"original project sentinel").unwrap();
        let ctx = egui::Context::default();
        assert!(export.start(&session.doc, path.clone(), &ctx).unwrap_err().contains(".ifc"));
        assert_eq!(std::fs::read(path).unwrap(), b"original project sentinel");
        assert!(!export.busy());
        assert!(export.feedback()["error"].as_str().unwrap().contains(".ifc"));
        session.doc.phases[0].id = "changed".into();
        let path = files.path("missing.ifc");
        assert!(export.start(&session.doc, path.clone(), &ctx).unwrap_err().contains("selected phase"));
        assert!(!path.exists());
        assert!(!export.busy());
    }

    #[test]
    fn replacing_a_project_invalidates_pending_scope_but_keeps_a_running_snapshot() {
        let session = crate::schedules::tests::session();
        let mut export = open(Format::Glb, &session.doc);
        export.project_replaced();
        assert!(!export.open);
        assert!(export.request(&session.doc, Path::new("model.glb")).is_err());
        export = open(Format::Glb, &session.doc);
        let request = export.request(&session.doc, Path::new("model.glb")).unwrap();
        let (sender, receiver) = channel();
        export.running = Some(receiver);
        export.pending = Some(request.clone());
        export.project_replaced();
        assert!(export.busy());
        assert_eq!(export.feedback()["source"], "source.trigon");
        sender.send(Completed { request, result: Ok(json!({"written":"model.glb"})), ms: 1.0 }).unwrap();
        assert_eq!(export.poll().unwrap().result.unwrap()["written"], "model.glb");
        assert!(export.request(&session.doc, Path::new("second.glb")).unwrap_err().contains("project changed"));
        export.show(Format::Glb, &session.doc, View { phase: 0, filter: PhaseFilter::Complete }, None, Some(Path::new("new.trigon")));
        assert!(export.request(&session.doc, Path::new("second.glb")).is_ok());
        assert_eq!(export.feedback()["source"], "new.trigon");
    }

    #[test]
    fn background_job_exports_its_snapshot_without_changing_the_live_history() {
        let mut session = crate::schedules::tests::phase_session();
        session.doc.buildings[0].facades[0].placements.last_mut().unwrap().width = Some(1600.0);
        let before = session.doc.clone();
        let files = Files::new();
        let mut export = open(Format::Glb, &session.doc);
        let expected_request = export.request(&session.doc, &files.path("expected.glb")).unwrap();
        let expected = execute_snapshot(session.doc.clone(), expected_request);
        assert!(expected.result.is_ok());
        let ctx = egui::Context::default();
        export.start(&session.doc, files.path("snapshot.glb"), &ctx).unwrap();
        assert_eq!(session.doc, before);
        assert!(export.start(&session.doc, files.path("second.glb"), &ctx).unwrap_err().contains("already running"));
        assert!(!files.path("second.glb").exists());
        // The live project can continue changing while the worker owns its copy.
        session.execute("building.set_wall_thickness", json!({"building":"b","thickness":450})).unwrap();
        let after_edit = session.doc.clone();
        let done = export.running.take().unwrap().recv_timeout(Duration::from_secs(30)).unwrap();
        assert!(done.result.is_ok(), "{:?}", done.result);
        assert_eq!(std::fs::read(files.path("snapshot.glb")).unwrap(), std::fs::read(files.path("expected.glb")).unwrap());
        assert_eq!(session.doc, after_edit, "background export cannot replace the live document");
        session.execute("edit.undo", json!({})).unwrap();
        assert_eq!(session.doc, before, "the live history still undoes the actual owner edit");
        session.execute("edit.redo", json!({})).unwrap();
        assert_eq!(session.doc, after_edit);
    }

    #[test]
    fn actual_ifc_snapshot_is_readable_and_preserves_the_original_document() {
        let mut session = crate::schedules::tests::session();
        let before = session.doc.clone();
        let dirty = session.dirty;
        let files = Files::new();
        let export = open(Format::Ifc, &session.doc);
        let request = export.request(&session.doc, &files.path("model.ifc")).unwrap();
        let done = execute_snapshot(session.doc.clone(), request);
        assert!(done.result.is_ok(), "{:?}", done.result);
        let result = session.execute("ifc.check", json!({"path":files.path("model.ifc")})).unwrap();
        for reader in result["readers"].as_array().unwrap() {
            assert_eq!(reader["failures"], 0, "{reader}");
        }
        assert_eq!(session.doc, before);
        assert_eq!(session.dirty, dirty);
        assert!(std::fs::read_to_string(files.path("model.ifc")).unwrap().contains("IFCWALL"));
    }

    #[test]
    fn worker_failures_and_disconnects_remain_in_feedback() {
        let session = crate::schedules::tests::session();
        let files = Files::new();
        let path = files.path("directory.ifc");
        std::fs::create_dir(&path).unwrap();
        let mut export = open(Format::Ifc, &session.doc);
        let request = export.request(&session.doc, &path).unwrap();
        let failed = execute_snapshot(session.doc.clone(), request.clone());
        assert!(failed.result.is_err());
        let (sender, receiver) = channel();
        sender.send(failed).unwrap();
        export.running = Some(receiver);
        export.pending = Some(request.clone());
        assert!(export.poll().unwrap().result.is_err());
        assert!(!export.busy());
        assert!(export.feedback()["error"].is_string());
        assert_eq!(export.feedback()["written"], Value::Null);
        let (sender, receiver) = channel();
        drop(sender);
        export.running = Some(receiver);
        export.pending = Some(request);
        let done = export.poll().unwrap();
        assert!(done.result.unwrap_err().contains("worker stopped"));
        assert!(!export.busy());
        assert!(export.feedback()["error"].as_str().unwrap().contains("worker stopped"));
    }
}
