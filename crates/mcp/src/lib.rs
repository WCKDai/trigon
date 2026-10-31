//! Trigon's MCP server: JSON-RPC 2.0 over stdio, newline-delimited.
//!
//! Every engine command is one tool (`facade.place` → `facade_place`). The agent works through
//! the same commands a person uses; nothing here creates geometry on its own.
#![deny(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::unimplemented, clippy::todo, clippy::unreachable)]

use std::io::{BufRead, Write};

use serde_json::{Value, json};
use trigon_engine::{Session, commands};

const PROTOCOL_VERSIONS: [&str; 3] = ["2025-06-18", "2025-03-26", "2024-11-05"];
const MAX_LINE: usize = 4 * 1024 * 1024;

const INSTRUCTIONS: &str = "Trigon models existing Danish buildings. Typical session: \
site_load_address (outline, storeys and heights from public data) → facade_list → evidence_photos \
(which oblique photos show each facade) → facade_set_kind (street, courtyard, party) → \
facade_set_bays → facade_place with component types from types_list (windows, doors, French and \
cantilever balconies) → export_ifc / export_gltf → document_save. Lengths are millimetres. \
Every change is checked; a refused change leaves the model as it was and the error says why. \
edit_undo reverts the last change.";

fn tool_name(id: &str) -> String {
    id.replace('.', "_")
}

/// Handles one JSON-RPC message; `None` for notifications.
pub fn handle(session: &mut Session, msg: &Value) -> Option<Value> {
    let id = msg.get("id").cloned();
    let method = msg.get("method").and_then(Value::as_str).unwrap_or_default();
    let id = id?; // Notifications get no reply.
    let ok = |result: Value| Some(json!({"jsonrpc": "2.0", "id": id, "result": result}));
    let err = |code: i64, message: &str| Some(json!({"jsonrpc": "2.0", "id": id, "error": {"code": code, "message": message}}));
    match method {
        "initialize" => {
            let asked = msg.pointer("/params/protocolVersion").and_then(Value::as_str).unwrap_or_default();
            let version = PROTOCOL_VERSIONS.iter().find(|v| **v == asked).unwrap_or(&PROTOCOL_VERSIONS[0]);
            ok(json!({
                "protocolVersion": version,
                "capabilities": {"tools": {"listChanged": false}},
                "serverInfo": {"name": "trigon", "version": env!("CARGO_PKG_VERSION")},
                "instructions": INSTRUCTIONS,
            }))
        }
        "ping" => ok(json!({})),
        "tools/list" => ok(json!({
            "tools": commands().iter().map(|c| json!({
                "name": tool_name(c.id),
                "description": c.summary,
                "inputSchema": c.params_schema(),
            })).collect::<Vec<_>>()
        })),
        "tools/call" => {
            let name = msg.pointer("/params/name").and_then(Value::as_str).unwrap_or_default();
            let Some(spec) = commands().iter().find(|c| tool_name(c.id) == name) else {
                return err(-32602, &format!("unknown tool {name}"));
            };
            let args = msg.pointer("/params/arguments").cloned().unwrap_or(Value::Null);
            let (text, is_error) = match session.execute(spec.id, args) {
                Ok(v) => (serde_json::to_string_pretty(&v).unwrap_or_default(), false),
                Err(e) => (e.to_string(), true),
            };
            ok(json!({"content": [{"type": "text", "text": text}], "isError": is_error}))
        }
        _ => err(-32601, &format!("method not found: {method}")),
    }
}

/// Serves until the input closes.
pub fn serve(session: &mut Session, input: impl BufRead, mut output: impl Write) -> std::io::Result<()> {
    for line in input.lines() {
        let line = line?;
        if line.trim().is_empty() {
            continue;
        }
        let reply = if line.len() > MAX_LINE {
            Some(json!({"jsonrpc": "2.0", "id": null, "error": {"code": -32600, "message": "message too large"}}))
        } else {
            match serde_json::from_str::<Value>(&line) {
                Ok(msg) => handle(session, &msg),
                Err(_) => Some(json!({"jsonrpc": "2.0", "id": null, "error": {"code": -32700, "message": "parse error"}})),
            }
        };
        if let Some(r) = reply {
            writeln!(output, "{r}")?;
            output.flush()?;
        }
    }
    Ok(())
}
