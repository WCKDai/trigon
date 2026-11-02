//! Local, typed placement preflights. The same edit functions power Studio, CLI and MCP;
//! previews run on a separate document and apply records exactly one atomic command.
use std::collections::hash_map::DefaultHasher;
use std::hash::Hasher;

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use trigon_model::{ModelError, annotation::AnnotationKind, checked};

use crate::{Result, Session, edit};

#[derive(Serialize, Deserialize, JsonSchema)]
#[serde(tag = "command", content = "params", deny_unknown_fields)]
pub(crate) enum Action {
    #[serde(rename = "view.define")]
    DefineView(trigon_model::annotation::DrawingView),
    #[serde(rename = "view.configure")]
    ConfigureView(crate::view_config::Configure),
    #[serde(rename = "annotation.define")]
    DefineAnnotation(trigon_model::annotation::Annotation),
    #[serde(rename = "building.add")]
    AddBuilding(crate::local_building::Add),
    #[serde(rename = "building.set_roof")]
    SetRoof(crate::commands::SetRoof),
    #[serde(rename = "rooflight.add")]
    AddRooflight(crate::rooflights::Add),
    #[serde(rename = "rooflight.remove")]
    RemoveRooflight(crate::rooflights::Remove),
    #[serde(rename = "interior.add_walls")]
    AddWalls(crate::commands::AddWalls),
    #[serde(rename = "interior.add_rooms")]
    AddRooms(crate::commands::AddRooms),
    #[serde(rename = "edit.move")]
    Move(edit::Move),
    #[serde(rename = "edit.align")]
    Align(edit::Align),
    #[serde(rename = "edit.align_walls")]
    AlignWalls(edit::AlignWalls),
    #[serde(rename = "edit.connect_walls")]
    ConnectWalls(edit::ConnectWalls),
    #[serde(rename = "element.set")]
    Set(crate::elements::Set),
    #[serde(rename = "annotation.move_label")]
    MoveLabel(MoveLabel),
    #[serde(rename = "edit.delete")]
    Delete(edit::Selection),
}

#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub(crate) struct Preview {
    pub action: Action,
    /// Optional bounded geometric audit of the candidate, before applying. Findings need
    /// source review; unavailable checks are explicitly unverified. No files are read.
    pub sanity: Option<crate::precision_audit::Audit>,
}

#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub(crate) struct Apply {
    /// Opaque same-build conflict token from precision.preview. Not an authorization token.
    pub preview_token: String,
    pub action: Action,
}

fn token(s: &Session, action: &Action) -> Result<String> {
    let mut hash = DefaultHasher::new();
    // JSON serialization is deterministic for these ordered sources. Include the entire
    // journal: even a later edit followed by an equal geometric state invalidates a preview.
    hash.write(&serde_json::to_vec(&s.doc).map_err(|_| ModelError::Invalid("cannot fingerprint document".into()))?);
    hash.write(&[0]);
    hash.write(&serde_json::to_vec(action).map_err(|_| ModelError::Invalid("cannot fingerprint placement action".into()))?);
    Ok(format!("precision/1-{:016x}", hash.finish()))
}

fn run(s: &mut Session, action: Action) -> Result<Value> {
    let result = match action {
        Action::DefineView(p) => crate::views::define_view(s, p),
        Action::ConfigureView(p) => crate::view_config::configure(s, p),
        Action::DefineAnnotation(p) => crate::views::define_annotation(s, p),
        Action::AddBuilding(p) => crate::local_building::add(s, p),
        Action::SetRoof(p) => crate::commands::set_roof(s, p),
        Action::AddRooflight(p) => crate::rooflights::add(s, p),
        Action::RemoveRooflight(p) => crate::rooflights::remove(s, p),
        Action::AddWalls(p) => crate::commands::add_interior_walls(s, p),
        Action::AddRooms(p) => crate::commands::add_interior_rooms(s, p),
        Action::Move(p) => edit::move_elements(s, p),
        Action::Align(p) => edit::align(s, p),
        Action::AlignWalls(p) => edit::align_walls(s, p),
        Action::ConnectWalls(p) => edit::connect_walls(s, p),
        Action::Set(p) => crate::elements::set(s, p),
        Action::MoveLabel(p) => move_label(s, p),
        Action::Delete(p) => edit::delete(s, p),
    }?;
    trigon_model::annotation::validate_document(&s.doc)?;
    trigon_model::build(&s.doc)?;
    Ok(result)
}

/// Sparse JSON-pointer changes are reviewable exact source values, not rounded mesh vertices.
fn changes(before: &Value, after: &Value, path: &str, out: &mut Vec<Value>) -> Result<()> {
    if before == after {
        return Ok(());
    }
    if out.len() >= 4096 {
        return Err(ModelError::Invalid("preview exceeds 4096 changed fields; split the edit into smaller steps".into()).into());
    }
    match (before, after) {
        (Value::Object(a), Value::Object(b)) => {
            let keys: std::collections::BTreeSet<_> = a.keys().chain(b.keys()).collect();
            for key in keys {
                let next = format!("{path}/{}", key.replace('~', "~0").replace('/', "~1"));
                changes(a.get(key).unwrap_or(&Value::Null), b.get(key).unwrap_or(&Value::Null), &next, out)?;
            }
        }
        (Value::Array(a), Value::Array(b)) => {
            for index in 0..a.len().max(b.len()) {
                changes(a.get(index).unwrap_or(&Value::Null), b.get(index).unwrap_or(&Value::Null), &format!("{path}/{index}"), out)?;
            }
        }
        _ => out.push(json!({"path":path,"before":before,"after":after})),
    }
    Ok(())
}

pub(crate) fn preview(s: &Session, p: Preview) -> Result<Value> {
    let preview_token = token(s, &p.action)?;
    let action = serde_json::to_value(&p.action).map_err(|_| ModelError::Invalid("cannot serialize placement action".into()))?;
    let mut drawings = std::collections::BTreeSet::new();
    match &p.action {
        Action::DefineView(p) => {
            drawings.insert(p.id.clone());
        }
        Action::ConfigureView(p) => {
            drawings.insert(p.id.clone());
        }
        Action::DefineAnnotation(p) => {
            drawings.insert(p.view.clone());
            if let Some(old) = s.doc.annotations.iter().find(|a| a.id == p.id) {
                drawings.insert(old.view.clone());
            }
        }
        Action::MoveLabel(p) => {
            if let Some(old) = s.doc.annotations.iter().find(|a| a.id == p.id) {
                drawings.insert(old.view.clone());
            }
        }
        _ => {}
    }
    // No environment credentials are loaded. The closed action list cannot access this
    // provider, files or the network; no command execution/journaling is nested here.
    let mut candidate = Session::new(Box::new(trigon_data::Live::new(Default::default())));
    candidate.doc = s.doc.clone();
    let result = run(&mut candidate, p.action)?;
    let mut diff = Vec::new();
    let before = serde_json::to_value(&s.doc).map_err(|_| ModelError::Invalid("cannot inspect source document".into()))?;
    let after = serde_json::to_value(&candidate.doc).map_err(|_| ModelError::Invalid("cannot inspect candidate document".into()))?;
    changes(&before, &after, "", &mut diff)?;
    if diff.is_empty() {
        return Err(ModelError::Invalid("placement action makes no source change".into()).into());
    }
    let mut response = json!({"preview_token":preview_token,"action":action,"changes":diff,"result":result,
        "validation":{"model_build":"passed","annotations":"passed"},"applied":false});
    if let Some(scope) = p.sanity {
        let feedback = match crate::precision_audit::audit(&candidate, scope) {
            Ok(audit) => audit,
            Err(error) => json!({"status":"unverified","error":error.to_string()}),
        };
        response["validation"]["sanity"] = feedback["status"].clone();
        response["sanity_feedback"] = feedback;
    }
    if !drawings.is_empty() {
        let mut rendered = true;
        let feedback: Vec<_> = drawings
            .into_iter()
            .map(|id| {
                match crate::lettering_audit::audit(
                    &candidate,
                    crate::lettering_audit::Audit { id: id.clone(), clearance_mm_paper: 0.5, max_results: 1000 },
                ) {
                    Ok(audit) => audit,
                    Err(error) => {
                        // A stale view or bounded audit can be unavailable during a repair.
                        // Keep structural/model validity separate and never report it as passed.
                        rendered = false;
                        json!({"view":id,"status":"unverified","error":error.to_string()})
                    }
                }
            })
            .collect();
        response["drawing_feedback"] = json!(feedback);
        response["validation"]["drawing_render"] = json!(if rendered { "passed" } else { "unverified" });
    }
    Ok(response)
}

pub(crate) fn apply(s: &mut Session, p: Apply) -> Result<Value> {
    if p.preview_token.len() > 64 || p.preview_token != token(s, &p.action)? {
        return Err(ModelError::Invalid("placement preview is stale or its action changed; run precision.preview again".into()).into());
    }
    run(s, p.action)
}

#[derive(Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub(crate) struct MoveLabel {
    pub id: String,
    /// Drawing-plane [dx,dy] mm. Dimension motion must be perpendicular to its witnesses;
    /// level marks may move horizontally only, so their measured values do not change.
    pub delta: [f64; 2],
}

pub(crate) fn move_label(s: &mut Session, p: MoveLabel) -> Result<Value> {
    for value in p.delta {
        checked("label delta", value, -100_000.0, 100_000.0)?;
    }
    let mut matches = s.doc.annotations.iter().enumerate().filter(|(_, a)| a.id == p.id);
    let (index, original) = matches.next().ok_or_else(|| ModelError::NotFound(format!("annotation {}", p.id)))?;
    if matches.next().is_some() {
        return Err(ModelError::Invalid("ambiguous annotation id".into()).into());
    }
    let mut next = original.clone();
    match &mut next.kind {
        AnnotationKind::Text { at, .. } | AnnotationKind::Leader { at, .. } => {
            at[0] += p.delta[0];
            at[1] += p.delta[1];
        }
        AnnotationKind::Dimension { from, to, offset_mm } => {
            let length = (to[0] - from[0]).hypot(to[1] - from[1]);
            let d = [(to[0] - from[0]) / length, (to[1] - from[1]) / length];
            if !length.is_finite() || length <= 1e-6 || (p.delta[0] * d[0] + p.delta[1] * d[1]).abs() > 1e-6 {
                return Err(ModelError::Invalid("dimension label motion must be perpendicular to the measured line".into()).into());
            }
            *offset_mm += -p.delta[0] * d[1] + p.delta[1] * d[0];
        }
        AnnotationKind::Level { at, .. } => {
            if p.delta[1].abs() > 1e-6 {
                return Err(ModelError::Invalid("move a level label horizontally; vertical movement changes its measured elevation".into()).into());
            }
            at[0] += p.delta[0];
        }
        _ => return Err(ModelError::Invalid("only text, leader, dimension and level annotations have movable labels".into()).into()),
    }
    // Validate on the final source; the outer command restores the document on failure.
    s.doc.annotations[index] = next;
    trigon_model::annotation::validate_document(&s.doc)?;
    Ok(json!({"moved":p.id,"annotation":s.doc.annotations[index]}))
}

#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub(crate) struct MeasureStair {
    pub name: String,
    pub level: usize,
    pub rough: [[f64; 2]; 4],
    pub template: Option<crate::drawings::StairTemplate>,
}

pub(crate) fn measure_stair(s: &mut Session, p: MeasureStair) -> Result<Value> {
    let fit = crate::drawings::fit_stair(s, &p.name, p.level, p.rough, p.template)?;
    let reg = s
        .doc
        .drawings
        .iter()
        .find(|d| d.name == p.name)
        .and_then(|d| d.plans.iter().find(|r| r.levels.contains(&p.level)))
        .ok_or_else(|| ModelError::Invalid("missing plan registration".into()))?;
    let a = reg.to_site([0., 0.]);
    let b = reg.to_site([1., 0.]);
    Ok(json!({"candidate":fit.stair,"treads_found":fit.treads,"tread_mm":fit.tread_mm,
        "run_mm":fit.length_mm,"width_mm":fit.width_mm,"well_mm":fit.well_mm,"lines_across_mm":fit.lines_across,
        "mm_per_pixel":(b[0]-a[0]).hypot(b[1]-a[1]),"applied":false,
        "basis":"registered source pixels; full-precision calculation is not a claim of physical or registration accuracy"}))
}

#[cfg(test)]
mod drawing_preview_tests {
    use super::*;

    fn session() -> Session {
        let mut s = Session::new(Box::new(trigon_data::Live::new(Default::default())));
        s.execute(
            "building.add",
            json!({"id":"b","name":"Public preview fixture","footprint":[[0,0],[12000,0],[12000,8000],[0,8000]],"storeys":1,"storey_height_mm":3000}),
        )
        .unwrap();
        s
    }

    #[test]
    fn interior_previews_report_candidate_sanity_and_apply_one_replayable_edit() {
        let mut s = session();
        s.execute("interior.add_walls",json!({"building":"b","walls":[{"id":"host","levels":[0],"a":[1000,4000],"b":[11000,4000],"thickness":240,"doors":[{"at":5000,"width":900,"height":2100}]}]})).unwrap();
        let actions = [
            json!({"command":"interior.add_walls","params":{"building":"b","walls":[{"id":"branch","levels":[0],"a":[6000,4000],"b":[6000,6500],"thickness":100}]}}),
            json!({"command":"interior.add_rooms","params":{"building":"b","rooms":[{"id":"room","name":"Measured room","levels":[0],"polygon":[[1000,1000],[4000,1000],[4000,3000],[1000,3000]]}]}}),
        ];
        for action in actions {
            let before = s.doc.clone();
            let history = (s.undo.len(), s.redo.len(), s.dirty);
            let before_glb = trigon_io::glb::write(&trigon_model::build(&s.doc).unwrap(), &[]);
            let plain = s.execute("precision.preview", json!({"action":action})).unwrap();
            assert!(plain.get("sanity_feedback").is_none());
            let preview = s.execute("precision.preview", json!({"action":action,"sanity":{"building":"b","levels":[0]}})).unwrap();
            assert_eq!(s.doc, before);
            assert_eq!((s.undo.len(), s.redo.len(), s.dirty), history);
            assert_eq!(plain["preview_token"], preview["preview_token"], "read-only audit options do not change an edit");
            assert_eq!(preview["validation"]["sanity"], "needs_review");
            assert_eq!(preview["sanity_feedback"]["finding_counts"]["wall_end_in_door_opening"], 1);
            assert_eq!(preview["sanity_feedback"]["truncated"], false);
            s.execute("precision.apply", json!({"action":preview["action"],"preview_token":preview["preview_token"]})).unwrap();
            assert_eq!(s.doc.journal.len(), before.journal.len() + 1);
            assert_eq!(s.doc.journal.last().unwrap().command, action["command"].as_str().unwrap());
            assert_eq!(preview["sanity_feedback"], s.execute("precision.audit", json!({"building":"b","levels":[0]})).unwrap());
            let accepted = s.doc.clone();
            assert!(s.execute("precision.apply", json!({"action":preview["action"],"preview_token":preview["preview_token"]})).is_err());
            assert_eq!(s.doc, accepted);
            s.execute("edit.undo", json!({})).unwrap();
            assert_eq!(s.doc, before);
            assert_eq!(trigon_io::glb::write(&trigon_model::build(&s.doc).unwrap(), &[]), before_glb);
            for bad in [
                json!({"command":"interior.add_walls","params":{"walls":[{"a":[1,1],"b":[1,1],"levels":[0],"thickness":100}]}}),
                json!({"command":"interior.add_rooms","params":{"rooms":[{"name":"bad","levels":[999],"polygon":[[0,0],[1000,0],[1000,1000]]}]}}),
            ] {
                assert!(s.execute("precision.preview", json!({"action":bad})).is_err());
                assert_eq!(s.doc, before);
            }
            s.execute("edit.redo", json!({})).unwrap();
            assert_eq!(s.doc, accepted);
        }
        let mut replay = Session::new(Box::new(trigon_data::Live::new(Default::default())));
        for entry in &s.doc.journal {
            replay.execute(&entry.command, entry.params.clone()).unwrap();
        }
        assert_eq!(s.doc, replay.doc);
    }

    #[test]
    fn unavailable_candidate_sanity_is_explicit_and_read_only() {
        let mut s = session();
        let before = s.doc.clone();
        let action = json!({"command":"view.define","params":{"id":"p","name":"Plan","kind":{"view":"plan","level":0}}});
        let preview = s.execute("precision.preview", json!({"action":action,"sanity":{"building":"absent"}})).unwrap();
        assert_eq!(s.doc, before);
        assert_eq!(preview["validation"]["model_build"], "passed");
        assert_eq!(preview["validation"]["sanity"], "unverified");
        assert_eq!(preview["sanity_feedback"]["status"], "unverified");
        assert!(preview["sanity_feedback"]["error"].as_str().unwrap().contains("absent"));
    }

    fn plan(s: &mut Session) {
        s.execute(
            "view.define",
            json!({"id":"p","name":"Plan","kind":{"view":"plan","level":0},
            "template":{"name":"100","sizing":{"mode":"fixed","denominator":100,"centre":[6000,4000]}}}),
        )
        .unwrap();
        s.execute(
            "annotation.define",
            json!({"id":"a","view":"p","size_mm":2.5,
            "kind":{"annotation":"text","at":[2000,4000],"text":"WWWWWWWW"}}),
        )
        .unwrap();
    }

    #[test]
    fn drawing_actions_preview_real_lettering_apply_once_and_replay() {
        let mut s = session();
        let actions = [
            json!({"command":"view.define","params":{"id":"p","name":"Plan","kind":{"view":"plan","level":0}}}),
            json!({"command":"view.configure","params":{"id":"p","template":{"name":"100","caption":"Public preview fixture",
                "sizing":{"mode":"fixed","denominator":100,"centre":[6000,4000]}}}}),
            json!({"command":"annotation.define","params":{"id":"label","view":"p","size_mm":2.5,
                "kind":{"annotation":"text","at":[3000,4000],"text":"Public label"}}}),
        ];
        for action in actions {
            let before = s.doc.clone();
            let history = (s.undo.len(), s.redo.len(), s.dirty);
            let preview = s.execute("precision.preview", json!({"action":action})).unwrap();
            assert_eq!(s.doc, before);
            assert_eq!((s.undo.len(), s.redo.len(), s.dirty), history);
            assert_eq!(preview, s.execute("precision.preview", json!({"action":action})).unwrap());
            assert_eq!(preview["validation"]["drawing_render"], "passed");
            assert_eq!(preview["drawing_feedback"].as_array().unwrap().len(), 1);
            assert_eq!(preview["drawing_feedback"][0]["view"], "p");
            s.execute("precision.apply", json!({"action":preview["action"],"preview_token":preview["preview_token"]})).unwrap();
            assert_eq!(s.doc.buildings, before.buildings, "drawing changes cannot alter geometry sources");
            assert_eq!(s.doc.journal.len(), before.journal.len() + 1);
            assert_eq!(preview["drawing_feedback"][0], s.execute("view.audit", json!({"id":"p"})).unwrap());
            let applied = s.doc.clone();
            s.execute("edit.undo", json!({})).unwrap();
            assert_eq!(s.doc, before);
            s.execute("edit.redo", json!({})).unwrap();
            assert_eq!(s.doc, applied);
        }
        let label = s.execute("view.audit", json!({"id":"p"})).unwrap()["labels"]
            .as_array()
            .unwrap()
            .iter()
            .find(|label| label["source"]["id"] == "label")
            .unwrap()
            .clone();
        assert_eq!(label["text"], "Public label");
        assert_eq!(label["size_mm_paper"], 2.5);
        assert_eq!(label["ink_corners_mm"].as_array().unwrap().len(), 4);
        let mut replay = Session::new(Box::new(trigon_data::Live::new(Default::default())));
        for entry in &s.doc.journal {
            assert_ne!(entry.command, "precision.apply");
            replay.execute(&entry.command, entry.params.clone()).unwrap();
        }
        assert_eq!(replay.doc, s.doc);
    }

    #[test]
    fn drawing_previews_report_overlap_and_crop_without_claiming_layout_acceptance() {
        let mut s = session();
        plan(&mut s);
        let before = s.doc.clone();
        for (id, at, expected) in [("b", [2000, 4000], "potential_text_overlap"), ("outside", [6000, 100000], "clipped_label")] {
            let action = json!({"command":"annotation.define","params":{"id":id,"view":"p","size_mm":2.5,
                "kind":{"annotation":"text","at":at,"text":"WWWWWWWW"}}});
            let preview = s.execute("precision.preview", json!({"action":action})).unwrap();
            let audit = &preview["drawing_feedback"][0];
            assert_eq!(preview["validation"]["model_build"], "passed");
            assert_eq!(audit["status"], "needs_review");
            assert!(audit["finding_counts"][expected].as_u64().unwrap() > 0);
            assert!(audit["labels"].as_array().unwrap().iter().any(|label| label["source"]["id"] == id));
            assert_eq!(s.doc, before);
        }
        let preview =
            s.execute("precision.preview", json!({"action":{"command":"annotation.move_label","params":{"id":"a","delta":[0,100000]}}})).unwrap();
        assert!(preview["drawing_feedback"][0]["finding_counts"]["clipped_label"].as_u64().unwrap() > 0);
        assert_eq!(s.doc, before);
    }

    #[test]
    fn invalid_and_stale_drawing_preflights_leave_document_dirty_state_and_redo_intact() {
        let mut s = session();
        plan(&mut s);
        s.execute("annotation.move_label", json!({"id":"a","delta":[100,0]})).unwrap();
        let edited = s.doc.clone();
        s.execute("edit.undo", json!({})).unwrap();
        let before = s.doc.clone();
        let history = (s.undo.len(), s.redo.len(), s.dirty);
        let action = json!({"command":"view.configure","params":{"id":"p","template":{"name":"Changed","caption":"Changed"}}});
        let preview = s.execute("precision.preview", json!({"action":action})).unwrap();
        for invalid in [
            json!({"command":"document.save","params":{"path":"forbidden.trigon"}}),
            json!({"command":"view.define","params":{"id":"bad","name":"Missing floor","kind":{"view":"plan","level":9}}}),
            json!({"command":"view.configure","params":{"id":"p","template":{"name":"Invalid","sizing":{"mode":"fixed","denominator":0,"centre":[0,0]}}}}),
            json!({"command":"annotation.define","params":{"id":"bad","view":"missing","kind":{"annotation":"text","at":[0,0],"text":"Absent view"}}}),
        ] {
            assert!(s.execute("precision.preview", json!({"action":invalid})).is_err());
            assert!(s.execute("precision.apply", json!({"action":invalid,"preview_token":preview["preview_token"]})).is_err());
            assert_eq!(s.doc, before);
            assert_eq!((s.undo.len(), s.redo.len(), s.dirty), history);
        }
        let mut changed_action = preview["action"].clone();
        changed_action["params"]["template"]["caption"] = json!("Changed action");
        assert!(s.execute("precision.apply", json!({"action":changed_action,"preview_token":preview["preview_token"]})).is_err());
        assert_eq!(s.doc, before);
        assert_eq!((s.undo.len(), s.redo.len(), s.dirty), history);
        s.execute("edit.redo", json!({})).unwrap();
        assert_eq!(s.doc, edited);
        let after = s.doc.clone();
        let history = (s.undo.len(), s.redo.len(), s.dirty);
        assert!(s.execute("precision.apply", json!({"action":preview["action"],"preview_token":preview["preview_token"]})).is_err());
        assert_eq!(s.doc, after);
        assert_eq!((s.undo.len(), s.redo.len(), s.dirty), history);
    }

    #[test]
    fn moving_an_annotation_between_views_audits_both_final_drawings() {
        let mut s = session();
        plan(&mut s);
        s.execute("view.define", json!({"id":"q","name":"Other plan","kind":{"view":"plan","level":0}})).unwrap();
        let before = s.doc.clone();
        let action = json!({"command":"annotation.define","params":{"id":"a","view":"q","size_mm":2.5,
            "kind":{"annotation":"text","at":[2000,4000],"text":"Moved label"}}});
        let preview = s.execute("precision.preview", json!({"action":action})).unwrap();
        let feedback = preview["drawing_feedback"].as_array().unwrap();
        assert_eq!(feedback.iter().map(|audit| audit["view"].as_str().unwrap()).collect::<Vec<_>>(), ["p", "q"]);
        assert!(!feedback[0]["labels"].as_array().unwrap().iter().any(|label| label["source"]["id"] == "a"));
        assert!(feedback[1]["labels"].as_array().unwrap().iter().any(|label| label["source"]["id"] == "a"));
        assert_eq!(s.doc, before);
    }

    #[test]
    fn unavailable_lettering_feedback_is_explicit_without_changing_existing_repair_contracts() {
        let mut s = session();
        plan(&mut s);
        let annotation = s.doc.annotations[0].clone();
        s.doc.annotations.extend((0..2000).map(|i| trigon_model::annotation::Annotation { id: format!("extra{i}"), ..annotation.clone() }));
        let before = s.doc.clone();
        let history = (s.undo.len(), s.redo.len(), s.dirty);
        let preview =
            s.execute("precision.preview", json!({"action":{"command":"annotation.move_label","params":{"id":"a","delta":[100,0]}}})).unwrap();
        assert_eq!(preview["validation"]["model_build"], "passed");
        assert_eq!(preview["validation"]["annotations"], "passed");
        assert_eq!(preview["validation"]["drawing_render"], "unverified");
        assert_eq!(preview["drawing_feedback"][0]["status"], "unverified");
        assert!(preview["drawing_feedback"][0]["error"].as_str().unwrap().contains("2000 labels"));
        assert_eq!(s.doc, before);
        assert_eq!((s.undo.len(), s.redo.len(), s.dirty), history);
        s.execute("precision.apply", json!({"action":preview["action"],"preview_token":preview["preview_token"]})).unwrap();
        assert_eq!(s.doc.journal.len(), before.journal.len() + 1);
    }
}
