//! Trigon's engine. Everything a person or an agent can do is a command: an id, a JSON schema for
//! its parameters and a function. The CLI, the MCP server and (later) the UI all run the same
//! commands through [`Session::execute`], so an agent can do exactly what a person can.
#![deny(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::unimplemented, clippy::todo, clippy::unreachable)]

use std::panic::{AssertUnwindSafe, catch_unwind};
use std::path::PathBuf;

use serde_json::Value;
use trigon_data::{DataError, Provider};
use trigon_model::doc::JournalEntry;
use trigon_model::{Document, ModelError};

mod commands;
mod drawings;
mod elevation;
mod facade_image;
mod plan;
mod plan_output;
mod rooms;
mod section;
mod sheet;
mod time;

pub use commands::{CommandSpec, commands};
/// For live views (Trigon Studio): one level's presentation plan as SVG text, the drawings' SVG
/// parser and the renderer they use, so a view rasterizes exactly what the sheets show.
pub use plan::sheet_svg_text as plan_sheet_svg;
pub use plan_output::parse_svg;
pub use resvg;

#[derive(Debug, thiserror::Error)]
pub enum EngineError {
    #[error("unknown command {0}; run `trigon commands` for the list")]
    Unknown(String),
    #[error("bad parameters for {command}: {message}")]
    Params { command: String, message: String },
    #[error(transparent)]
    Model(#[from] ModelError),
    #[error(transparent)]
    Data(#[from] DataError),
    #[error("{0}")]
    Io(String),
    #[error("internal error in {0}; the document was restored")]
    Internal(String),
}

pub type Result<T> = std::result::Result<T, EngineError>;

const UNDO_LIMIT: usize = 200;

pub struct Session {
    pub doc: Document,
    /// Where the document was opened from or last saved to.
    pub path: Option<PathBuf>,
    pub dirty: bool,
    undo: Vec<Document>,
    redo: Vec<Document>,
    provider: Box<dyn Provider>,
    /// The last decoded drawing page (path, page, rotation), since scans take a moment to decode.
    pub(crate) drawing_cache: Option<((String, u32, u32), std::sync::Arc<trigon_drawing::Gray>)>,
}

impl Session {
    pub fn new(provider: Box<dyn Provider>) -> Self {
        Self { doc: Document::default(), path: None, dirty: false, undo: Vec::new(), redo: Vec::new(), provider, drawing_cache: None }
    }

    pub fn provider(&self) -> &dyn Provider {
        self.provider.as_ref()
    }

    /// Runs one command. A command that fails, or panics, leaves the document as it was.
    pub fn execute(&mut self, id: &str, params: Value) -> Result<Value> {
        let spec = commands().iter().find(|c| c.id == id).ok_or_else(|| EngineError::Unknown(id.to_string()))?;
        let params = if params.is_null() { Value::Object(Default::default()) } else { params };
        let before = spec.mutates.then(|| self.doc.clone());
        let outcome = catch_unwind(AssertUnwindSafe(|| (spec.run)(self, &params)));
        let result = match outcome {
            Ok(r) => r,
            Err(_) => Err(EngineError::Internal(id.to_string())),
        };
        match (&result, before) {
            (Err(_), Some(before)) => self.doc = before,
            (Ok(_), Some(before)) if spec.journaled => {
                self.undo.push(before);
                if self.undo.len() > UNDO_LIMIT {
                    self.undo.remove(0);
                }
                self.redo.clear();
                self.doc.journal.push(JournalEntry { command: id.to_string(), params });
                self.dirty = true;
            }
            _ => {}
        }
        result
    }

    pub(crate) fn undo(&mut self) -> bool {
        match self.undo.pop() {
            Some(prev) => {
                self.redo.push(std::mem::replace(&mut self.doc, prev));
                self.dirty = true;
                true
            }
            None => false,
        }
    }

    pub(crate) fn redo(&mut self) -> bool {
        match self.redo.pop() {
            Some(next) => {
                self.undo.push(std::mem::replace(&mut self.doc, next));
                self.dirty = true;
                true
            }
            None => false,
        }
    }

    pub(crate) fn reset_history(&mut self) {
        self.undo.clear();
        self.redo.clear();
    }
}

#[cfg(test)]
mod tests;
