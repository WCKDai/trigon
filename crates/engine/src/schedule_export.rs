//! Portable schedule files. CSV text is quoted and formula-like cells are made literal.

use std::path::Path;

use schemars::JsonSchema;
use serde::Deserialize;
use serde_json::{Value, json};

use crate::schedules::{OpeningScheduleRequest, query};
use crate::{EngineError, OpeningQuantityBasis, OpeningSchedule, Result, ScheduleOpeningKind, Session};

#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub(crate) struct Export {
    path: String,
    building: Option<String>,
    levels: Option<Vec<usize>>,
    phase: Option<String>,
    filter: Option<String>,
}

/// Quoting alone does not stop spreadsheet formulas. Prefix dangerous text with a literal
/// apostrophe, including formulas obscured by whitespace/BOM; retain exact original text in JSON.
fn cell(text: &str) -> String {
    let trimmed = text.trim_start_matches(|character: char| character.is_whitespace() || character.is_control() || character == '\u{feff}');
    let formula = trimmed.starts_with(['=', '+', '-', '@']) || text.starts_with(['\t', '\r', '\n']);
    format!("\"{}{}\"", if formula { "'" } else { "" }, text.replace('"', "\"\""))
}

fn csv(schedule: &OpeningSchedule, phase: &str, filter: &str) -> String {
    let mut text = String::from(
        "Id,Building id,Building,Host,Level,Storey,Kind,Type,Name,Width (mm),Height (mm),Area (m2),Quantity basis,Status,External,Phase,Filter\r\n",
    );
    for row in &schedule.rows {
        let fields = [
            cell(&row.id),
            cell(&row.building_id),
            cell(&row.building_name),
            cell(&row.host_id),
            row.level.to_string(),
            cell(&row.level_name),
            cell(match row.kind {
                ScheduleOpeningKind::Window => "window",
                ScheduleOpeningKind::Door => "door",
            }),
            cell(&row.type_id),
            cell(&row.name),
            row.width_mm.to_string(),
            row.height_mm.to_string(),
            row.area_m2.map_or(String::new(), |area| area.to_string()),
            cell(match row.quantity_basis {
                OpeningQuantityBasis::OuterLining => "outer_lining",
                OpeningQuantityBasis::FacadeOpening => "facade_opening",
            }),
            cell(&row.status),
            row.external.to_string(),
            cell(phase),
            cell(filter),
        ];
        text.push_str(&fields.join(","));
        text.push_str("\r\n");
    }
    text
}

/// Write a completed query atomically; failed validation cannot replace an existing report.
pub(crate) fn export(session: &Session, params: Export) -> Result<Value> {
    let path = Path::new(&params.path);
    let extension = path.extension().and_then(|extension| extension.to_str()).unwrap_or_default();
    if !matches!(extension, "csv" | "json") {
        return Err(trigon_model::ModelError::Invalid("use .csv or .json for an opening schedule".into()).into());
    }
    let request = OpeningScheduleRequest { building: params.building, levels: params.levels, phase: params.phase, filter: params.filter };
    let (bytes, instances) = if extension == "json" {
        let report = crate::schedules::openings(session, request)?;
        let bytes = serde_json::to_vec_pretty(&report).map_err(|error| EngineError::Io(format!("schedule JSON: {error}")))?;
        let count = report["rows"].as_array().ok_or_else(|| EngineError::Internal("schedule.export: rows".into()))?.len();
        (bytes, count)
    } else {
        let (schedule, view) = query(session, &request)?;
        let phase = session.doc.phases.get(view.phase).map_or("", |phase| phase.id.as_str());
        (csv(&schedule, phase, request.filter.as_deref().unwrap_or("complete")).into_bytes(), schedule.rows.len())
    };
    crate::plan::write_output(path, &bytes)?;
    Ok(json!({"written":params.path,"format":extension,"instances":instances}))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn csv_text_is_quoted_and_formula_prefixes_are_literal() {
        assert_eq!(cell("Køkken, \"nord\"\r\nfløj"), "\"Køkken, \"\"nord\"\"\r\nfløj\"");
        for formula in ["=1+1", "+SUM(A1:A2)", "-1+2", "@SUM(A1)", " \t=1+1", "\0=1+1", "\u{feff}=1+1", "\ttext", "\rtext", "\ntext"] {
            assert!(cell(formula).starts_with("\"'"), "{formula:?}");
        }
        for text in ["b1.f1.L0.B0", "Window - south", "Dør 1", "  ordinary text"] {
            assert!(!cell(text).starts_with("\"'"));
        }
    }

    #[test]
    fn exports_keep_query_data_and_history_and_refuse_invalid_replacements() {
        let directory = std::env::temp_dir().join(format!("trigon-schedule-export-{}", std::process::id()));
        std::fs::create_dir_all(&directory).unwrap();
        let mut session = Session::new(Box::new(trigon_data::Live::new(trigon_data::Credentials::default())));
        session.doc = trigon_io::ifc::sample::document().unwrap();
        session.doc.buildings[0].name = "=not a formula".into();
        let before = session.doc.clone();
        let expected = session.execute("schedule.openings", json!({"levels":[0]})).unwrap();
        for extension in ["csv", "json"] {
            let path = directory.join(format!("openings.{extension}"));
            let result = session.execute("schedule.export", json!({"path":path,"levels":[0]})).unwrap();
            assert_eq!(result["instances"], expected["totals"]["instances"]);
            let original = std::fs::read(&path).unwrap();
            if extension == "json" {
                assert_eq!(serde_json::from_slice::<Value>(&original).unwrap(), expected);
            } else {
                let text = String::from_utf8(original.clone()).unwrap();
                assert!(text.starts_with("Id,Building id,"));
                assert!(text.contains("\"'=not a formula\""));
                assert!(text.ends_with("\r\n"));
            }
            assert!(session.execute("schedule.export", json!({"path":path,"levels":[999]})).is_err());
            assert_eq!(std::fs::read(&path).unwrap(), original);
            std::fs::remove_file(&path).unwrap();
        }
        assert!(session.execute("schedule.export", json!({"path":directory.join("unsupported.html")})).is_err());
        assert_eq!(session.doc, before);
        assert!(!session.dirty);
        assert_eq!(session.execute("edit.undo", json!({})).unwrap()["undone"], false);
        std::fs::remove_dir(&directory).unwrap();
    }
}
