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
            let (text, is_error, image) = match session.execute(spec.id, args) {
                Ok(v) => (serde_json::to_string_pretty(&v).unwrap_or_default(), false, image_of(&v)),
                Err(e) => (e.to_string(), true, None),
            };
            let mut content = vec![json!({"type": "text", "text": text})];
            if let Some(data) = image {
                content.push(json!({"type": "image", "data": data, "mimeType": "image/png"}));
            }
            ok(json!({"content": content, "isError": is_error}))
        }
        _ => err(-32601, &format!("method not found: {method}")),
    }
}

/// Largest image sent inline to the agent.
const MAX_IMAGE: u64 = 8 * 1024 * 1024;

/// A command result that names a PNG (`image_path`) is sent to the agent as an image too.
fn image_of(v: &Value) -> Option<String> {
    let path = v.get("image_path")?.as_str()?;
    if !path.ends_with(".png") || std::fs::metadata(path).ok()?.len() > MAX_IMAGE {
        return None;
    }
    std::fs::read(path).ok().map(|b| base64(&b))
}

fn base64(bytes: &[u8]) -> String {
    const T: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::with_capacity(bytes.len().div_ceil(3) * 4);
    for c in bytes.chunks(3) {
        let n = (u32::from(c[0]) << 16) | (u32::from(*c.get(1).unwrap_or(&0)) << 8) | u32::from(*c.get(2).unwrap_or(&0));
        for (i, shift) in [18, 12, 6, 0].into_iter().enumerate() {
            if i <= c.len() {
                out.push(char::from(T[((n >> shift) & 63) as usize]));
            } else {
                out.push('=');
            }
        }
    }
    out
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

#[cfg(test)]
mod tests {
    #[test]
    fn base64_matches_rfc_4648() {
        assert_eq!(super::base64(b""), "");
        assert_eq!(super::base64(b"f"), "Zg==");
        assert_eq!(super::base64(b"fo"), "Zm8=");
        assert_eq!(super::base64(b"foo"), "Zm9v");
        assert_eq!(super::base64(b"foobar"), "Zm9vYmFy");
    }
}
