//! Rebuilding off the UI thread: the model's geometry (`build_view`), the viewport's scene (its
//! triangles and feature edges) and the plan sheet, for a copy of the document. Only the latest
//! request is worked on, so a burst of changes costs one rebuild.

use std::panic::{AssertUnwindSafe, catch_unwind};
use std::sync::Arc;
use std::sync::mpsc::{Receiver, Sender, channel};
use std::time::Instant;

use trigon_engine::resvg::usvg;
use trigon_model::Document;
use trigon_model::build::View;

use crate::scene::Scene;

pub struct Job {
    pub generation: u64,
    pub doc: Arc<Document>,
    pub view: View,
    pub want_scene: bool,
    /// The plan sheet of this level, too.
    pub plan_level: Option<usize>,
}

pub struct Done {
    pub generation: u64,
    pub scene: Option<Result<Scene, String>>,
    pub plan: Option<(usize, Result<Arc<usvg::Tree>, String>)>,
    /// Milliseconds in `build_view`, in making the scene, in drawing and parsing the plan.
    pub build_ms: f64,
    pub scene_ms: f64,
    pub plan_ms: f64,
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
        let scene = built.and_then(|b| guarded("preparing the view", || Ok(Scene::from_built(&b, phases))));
        scene_ms = ms(t);
        scene
    });
    let plan = job.plan_level.map(|level| {
        let t = Instant::now();
        let tree = guarded("drawing the plan", || {
            let svg = trigon_engine::plan_sheet_svg(&job.doc, level, job.view).map_err(|e| e.to_string())?;
            trigon_engine::parse_svg(&svg).map(Arc::new).map_err(|e| e.to_string())
        });
        plan_ms = ms(t);
        (level, tree)
    });
    Done { generation: job.generation, scene, plan, build_ms, scene_ms, plan_ms }
}

impl Worker {
    pub fn start(wake: impl Fn() + Send + 'static) -> Worker {
        let (jobs, rx) = channel::<Job>();
        let (tx, done) = channel();
        let _ = std::thread::Builder::new().name("trigon-build".into()).spawn(move || {
            while let Ok(mut job) = rx.recv() {
                while let Ok(newer) = rx.try_recv() {
                    job = Job { want_scene: job.want_scene || newer.want_scene, plan_level: newer.plan_level.or(job.plan_level), ..newer };
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
        Job { generation: 1, doc: Arc::new(doc), view: View { phase: 0, filter: PhaseFilter::Complete }, want_scene: true, plan_level }
    }

    #[test]
    fn empty_and_hostile_requests_give_errors_not_panics() {
        let done = run(&job(Document::default(), Some(0)));
        assert!(matches!(done.scene, Some(Ok(ref s)) if s.elements.is_empty()));
        assert!(done.plan.is_some());
        let mut doc = Document::default();
        doc.buildings.push(trigon_model::Building::new("b1", "T", &[[0.0, 0.0], [9000.0, 0.0], [9000.0, 8000.0], [0.0, 8000.0]], 2, 3000.0).unwrap());
        let done = run(&job(doc.clone(), Some(usize::MAX)));
        assert!(matches!(done.plan, Some((_, Err(_)))));
        let mut bad_view = job(doc.clone(), Some(0));
        bad_view.view.phase = usize::MAX;
        assert!(matches!(run(&bad_view).scene, Some(Err(_))));
        let done = run(&job(doc, Some(1)));
        assert!(matches!(done.plan, Some((1, Ok(_)))), "a real plan sheet parses");
        assert!(matches!(done.scene, Some(Ok(ref s)) if !s.elements.is_empty()));
    }
}
