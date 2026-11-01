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
    #[serde(rename = "building.add")]
    AddBuilding(crate::local_building::Add),
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
        Action::AddBuilding(p) => crate::local_building::add(s, p),
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
    Ok(json!({"preview_token":preview_token,"action":action,"changes":diff,"result":result,
        "validation":{"model_build":"passed","annotations":"passed"},"applied":false}))
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
