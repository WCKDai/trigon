//! Rebuilding off the UI thread: the model's geometry (`build_view`), the viewport's scene (its
//! triangles and feature edges) and the plan sheet, for a copy of the document. Only the latest
//! request is worked on, so a burst of changes costs one rebuild.

use std::panic::{AssertUnwindSafe, catch_unwind};
use std::sync::Arc;
use std::sync::mpsc::{Receiver, Sender, channel};
use std::time::Instant;

use trigon_engine::resvg::usvg;
use trigon_engine::{
    DrawingElementRegion, DrawingElementSegment, DrawingPreview, DrawingTransform, OpeningSchedule, element_properties_from_built,
    opening_schedule_from_built,
};
use trigon_model::Document;
use trigon_model::build::View;

use crate::scene::Scene;

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum DrawingRequest {
    Plan(usize),
    Saved(String),
}

pub struct PreparedDrawing {
    pub tree: Arc<usvg::Tree>,
    pub transform: DrawingTransform,
    pub segments: Vec<[[f64; 2]; 2]>,
    /// Visible model linework, in drawing-plane mm, with the built element id that owns each stroke.
    pub element_segments: Vec<DrawingElementSegment>,
    pub element_regions: Vec<DrawingElementRegion>,
}

pub struct Job {
    pub generation: u64,
    pub doc: Arc<Document>,
    pub view: View,
    pub want_scene: bool,
    /// One drawing workspace, prepared with its exact picking geometry.
    pub drawing: Option<DrawingRequest>,
}

pub struct Done {
    pub generation: u64,
    pub scene: Option<Result<PreparedScene, String>>,
    pub drawing: Option<(DrawingRequest, Result<PreparedDrawing, String>)>,
    /// Milliseconds in `build_view`, in making the scene, in drawing and parsing the plan.
    pub build_ms: f64,
    pub scene_ms: f64,
    pub plan_ms: f64,
}

/// One built view, delivered atomically so its inventory and picking geometry cannot diverge.
pub struct PreparedScene {
    pub scene: Scene,
    pub schedule: OpeningSchedule,
    pub properties: Vec<serde_json::Value>,
}

pub struct Worker {
    jobs: Sender<Job>,
    done: Receiver<Done>,
}

fn ms(since: Instant) -> f64 {
    since.elapsed().as_secs_f64() * 1000.0
}

/// Runs `f`, turning a panic (a bug, never expected) into an error.
fn guarded<T>(what: &str, f: impl FnOnce() -> Result<T, String>) -> Result<T, String> {
    catch_unwind(AssertUnwindSafe(f)).unwrap_or_else(|_| Err(format!("internal error while {what}")))
}

pub fn run(job: &Job) -> Done {
    let (mut build_ms, mut scene_ms, mut plan_ms) = (0.0, 0.0, 0.0);
    let scene = job.want_scene.then(|| {
        let t = Instant::now();
        let built = guarded("building the model", || trigon_model::build::build_view(&job.doc, job.view).map_err(|e| e.to_string()));
        build_ms = ms(t);
        let t = Instant::now();
        let phases = job.doc.phases.len() > 1;
        let scene = built.and_then(|b| {
            guarded("preparing the view", || {
                let schedule = opening_schedule_from_built(&job.doc, &b).map_err(|e| e.to_string())?;
                let properties = element_properties_from_built(&job.doc, &b, job.view).map_err(|e| e.to_string())?;
                let site = trigon_model::site::build(&job.doc, job.view, None).map_err(|e| e.to_string())?;
                Ok(PreparedScene { scene: Scene::from_built(&b, phases).with_site(&site).with_rooms(&job.doc), schedule, properties })
            })
        });
        scene_ms = ms(t);
        scene
    });
    let drawing = job.drawing.as_ref().map(|request| {
        let t = Instant::now();
        let tree = guarded("drawing the workspace", || {
            let DrawingPreview { svg, transform, segments, element_segments, element_regions } = match request {
                DrawingRequest::Plan(level) => trigon_engine::plan_drawing(&job.doc, *level, job.view),
                DrawingRequest::Saved(id) => trigon_engine::drawing_view(&job.doc, id, job.view),
            }
            .map_err(|e| e.to_string())?;
            let tree = trigon_engine::parse_svg(&svg).map(Arc::new).map_err(|e| e.to_string())?;
            Ok(PreparedDrawing { tree, transform, segments, element_segments, element_regions })
        });
        plan_ms = ms(t);
        (request.clone(), tree)
    });
    Done { generation: job.generation, scene, drawing, build_ms, scene_ms, plan_ms }
}

impl Worker {
    pub fn start(wake: impl Fn() + Send + 'static) -> Worker {
        let (jobs, rx) = channel::<Job>();
        let (tx, done) = channel();
        let _ = std::thread::Builder::new().name("trigon-build".into()).spawn(move || {
            while let Ok(mut job) = rx.recv() {
                while let Ok(newer) = rx.try_recv() {
                    job = Job { want_scene: job.want_scene || newer.want_scene, drawing: newer.drawing.or(job.drawing), ..newer };
                }
                if tx.send(run(&job)).is_err() {
                    break;
                }
                wake();
            }
        });
        Worker { jobs, done }
    }

    pub fn submit(&self, job: Job) {
        let _ = self.jobs.send(job);
    }

    pub fn try_recv(&self) -> Option<Done> {
        self.done.try_recv().ok()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use trigon_model::build::PhaseFilter;

    fn job(doc: Document, plan_level: Option<usize>) -> Job {
        Job {
            generation: 1,
            doc: Arc::new(doc),
            view: View { phase: 0, filter: PhaseFilter::Complete },
            want_scene: true,
            drawing: plan_level.map(DrawingRequest::Plan),
        }
    }

    #[test]
    fn empty_and_hostile_requests_give_errors_not_panics() {
        let done = run(&job(Document::default(), Some(0)));
        assert!(matches!(done.scene, Some(Ok(ref s)) if s.scene.elements.is_empty() && s.schedule.rows.is_empty()));
        assert!(done.drawing.is_some());
        let mut doc = Document::default();
        doc.buildings.push(trigon_model::Building::new("b1", "T", &[[0.0, 0.0], [9000.0, 0.0], [9000.0, 8000.0], [0.0, 8000.0]], 2, 3000.0).unwrap());
        let done = run(&job(doc.clone(), Some(usize::MAX)));
        assert!(matches!(done.drawing, Some((_, Err(_)))));
        let mut bad_view = job(doc.clone(), Some(0));
        bad_view.view.phase = usize::MAX;
        assert!(matches!(run(&bad_view).scene, Some(Err(_))));
        let done = run(&job(doc, Some(1)));
        let Some((DrawingRequest::Plan(1), Ok(drawing))) = done.drawing else {
            panic!("a real plan sheet parses");
        };
        assert!(drawing.element_segments.iter().any(|line| line.id.starts_with("b1.f")), "plan strokes keep facade wall ids");
        assert!(drawing.element_segments.iter().all(|line| !line.id.ends_with(".opening")));
        assert!(matches!(done.scene, Some(Ok(ref s)) if !s.scene.elements.is_empty()));
    }

    #[test]
    fn worker_inventory_and_scene_share_the_built_occurrences_in_every_phase_filter() {
        let mut s = crate::schedules::tests::phase_session();
        s.execute("building.add_basement", serde_json::json!({"floor":-2400})).unwrap();
        s.execute("building.set_cellar_stairs", serde_json::json!({"stairs":[{"id":"cellar","facade":"b.f2","from":1000,"to":5000,"width":1100,"top":4800,"bottom":2000,"floor":-2400,"door_at":1450,"door_width":900}]})).unwrap();
        for filter in [PhaseFilter::Complete, PhaseFilter::New, PhaseFilter::Demolition, PhaseFilter::All] {
            let mut job = job(s.doc.clone(), None);
            job.view = View { phase: 1, filter };
            let prepared = run(&job).scene.unwrap().unwrap();
            let openings = prepared.scene.elements.iter().filter(|e| matches!(e.kind, "window" | "door")).count();
            assert_eq!(openings, prepared.schedule.totals.instances);
            for row in &prepared.schedule.rows {
                if prepared.scene.ambiguous(&row.id) {
                    assert_eq!(crate::schedules::resolve_row(&prepared.scene, row), Err(crate::schedules::AMBIGUOUS));
                } else {
                    assert!(crate::schedules::resolve_row(&prepared.scene, row).is_ok(), "{filter:?}: {}", row.id);
                }
            }
            let inherited: Vec<_> = prepared.schedule.rows.iter().filter(|r| !r.external || r.type_id == "door.cellar").collect();
            assert_eq!(inherited.len(), 3);
            assert!(inherited.iter().all(|r| r.status == "existing"));
            assert_eq!(prepared.schedule.totals.area_excluded, usize::from(filter != PhaseFilter::New));
            let mut plan_only = job;
            plan_only.want_scene = false;
            assert!(run(&plan_only).scene.is_none(), "a plan-only job must not replace the model inventory");
        }
    }

    #[test]
    fn drawing_picks_resolve_in_the_scene_and_drive_the_shared_undoable_edit() {
        use serde_json::json;
        use trigon_engine::{DrawingElementKind, Session};
        let mut session = Session::new(Box::new(trigon_data::Live::new(trigon_data::Credentials::default())));
        session.doc.buildings.push(
            trigon_model::Building::new("b", "Studio picking", &[[0.0, 0.0], [12000.0, 0.0], [12000.0, 8000.0], [0.0, 8000.0]], 2, 3000.0).unwrap(),
        );
        for (command, params) in [
            ("facade.set_bays", json!({"facade":"b.f1","count":3})),
            ("facade.place", json!({"facade":"b.f1","type":"window.casement.2","levels":[0,1],"bays":[0]})),
            ("interior.add_walls", json!({"walls":[{"id":"partition","a":[2000,4000],"b":[10000,4000],"thickness":150,"levels":[0,1]}]})),
            ("view.define", json!({"id":"plan","name":"Plan","kind":{"view":"plan","level":0}})),
            ("view.define", json!({"id":"facade","name":"Facade","kind":{"view":"facade","facade":"b.f1"}})),
            ("view.define", json!({"id":"section","name":"Section","kind":{"view":"section","from":[0,2000],"to":[12000,2000]}})),
        ] {
            session.execute(command, params).unwrap();
        }
        for request in [
            DrawingRequest::Plan(0),
            DrawingRequest::Saved("plan".into()),
            DrawingRequest::Saved("facade".into()),
            DrawingRequest::Saved("section".into()),
        ] {
            let before = session.doc.clone();
            let mut request_job = job(session.doc.clone(), None);
            request_job.drawing = Some(request.clone());
            let result = run(&request_job);
            let scene = result.scene.unwrap().unwrap().scene;
            let (actual, drawing) = result.drawing.unwrap();
            assert_eq!(actual, request);
            let drawing = drawing.unwrap();
            assert!(!drawing.element_segments.is_empty());
            for line in &drawing.element_segments {
                assert!(scene.find(&line.id).is_some(), "{request:?}: {} must resolve to a real scene element", line.id);
            }
            let preview = DrawingPreview {
                svg: String::new(),
                transform: drawing.transform,
                segments: drawing.segments,
                element_segments: drawing.element_segments,
                element_regions: drawing.element_regions,
            };
            let (point, expected) = match &request {
                DrawingRequest::Saved(id) if id == "facade" => ([session.doc.buildings[0].facades[0].bays[0], 896.0], "b.f1.L0.B0"),
                DrawingRequest::Saved(id) if id == "section" => ([2000.0, 1500.0], "partition.L0"),
                _ => ([3000.0, 3925.0], "partition.L0"),
            };
            let hit = preview.pick_element(point, 2.0).unwrap_or_else(|| panic!("{request:?}: expected {expected} at {point:?}"));
            assert_eq!(hit.id, expected, "{request:?}");
            assert_eq!(session.doc, before, "drawing and picking must leave the document unchanged");
            let selected = &scene.elements[scene.find(&hit.id).unwrap()];
            assert_eq!(selected.id, expected);
            assert_eq!(selected.kind, if hit.kind == DrawingElementKind::Opening { "window" } else { "interior wall" });
            session.execute("edit.move", json!({"targets":[hit.id],"delta":[100,0,0]})).unwrap();
            assert_ne!(session.doc.buildings, before.buildings);
            assert_eq!(session.execute("edit.undo", json!({})).unwrap()["undone"], true);
            assert_eq!(session.doc, before, "{request:?}: undo must restore the whole document");
        }
    }
}
