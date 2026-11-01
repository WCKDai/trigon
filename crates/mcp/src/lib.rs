//! Trigon's MCP server: JSON-RPC 2.0 over stdio, newline-delimited.
//!
//! Every engine command is one tool (`facade.place` → `facade_place`). The agent works through
//! the same commands a person uses; nothing here creates geometry on its own.
//!
//! The commands run on a [`Backend`]: a session of the server's own ([`Session`], headless), or
//! a running Trigon Studio reached over its control channel ([`Remote`]), where the owner watches
//! every call land. Bridged to Studio, two more tools let the agent steer and see the owner's
//! view (`studio_view`, `studio_screenshot`).
#![deny(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::unimplemented, clippy::todo, clippy::unreachable)]

use std::io::{BufRead, Write};

use serde_json::{Value, json};
use trigon_control::{CallError, Client};
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

const STUDIO_INSTRUCTIONS: &str = " You are connected to Trigon Studio: the owner watches every \
command land in the model and in the activity feed. studio_view shows them a level, a phase or the \
plan; studio_screenshot shows you what they see.";

/// Where tool calls run.
pub trait Backend {
    /// Runs one engine command.
    fn run_command(&mut self, command: &str, params: Value) -> Result<Value, String>;
    /// True when a Trigon Studio window is attached (the `studio_*` tools work).
    fn has_studio(&self) -> bool {
        false
    }
    /// Answers a Studio method (`ui.view`, `ui.screenshot`).
    fn studio(&mut self, method: &str, _params: Value) -> Result<Value, String> {
        Err(format!("{method} needs Trigon Studio; this MCP server runs a session of its own"))
    }
    /// The MCP client's name (from `initialize`), shown to the owner.
    fn hello(&mut self, _client: &str) {}
}

/// Headless: the server's own session.
impl Backend for Session {
    fn run_command(&mut self, command: &str, params: Value) -> Result<Value, String> {
        self.execute(command, params).map_err(|e| e.to_string())
    }
}

/// A running Trigon Studio, reached over its control channel (docs/control-protocol.md). It
/// connects on first use and again after the app restarts; the token is looked up each time,
/// since every launch makes a new one.
pub struct Remote {
    addr: String,
    explicit_token: Option<String>,
    client: Option<Client>,
    client_name: Option<String>,
}

impl Remote {
    pub fn new(addr: &str, token: Option<&str>) -> Self {
        Remote { addr: addr.to_string(), explicit_token: token.map(str::to_string), client: None, client_name: None }
    }

    pub fn addr(&self) -> &str {
        &self.addr
    }

    /// Connect now (otherwise the first call does).
    pub fn try_connect(&mut self) -> Result<(), String> {
        let port = self.addr.rsplit_once(':').and_then(|(_, p)| p.parse::<u16>().ok()).ok_or_else(|| format!("{} has no port", self.addr))?;
        let token = trigon_control::auth::client_token(port, self.explicit_token.as_deref()).ok_or_else(|| {
            format!("no control token for port {port}: is Trigon Studio running? (--token or {} give one)", trigon_control::auth::TOKEN_ENV)
        })?;
        let mut client = Client::connect(&self.addr, &token).map_err(|e| format!("Trigon Studio at {}: {e}", self.addr))?;
        if let Some(name) = &self.client_name {
            let _ = client.call("control.hello", json!({"client": name}));
        }
        self.client = Some(client);
        Ok(())
    }

    fn call(&mut self, method: &str, params: Value) -> Result<Value, String> {
        if self.client.is_none() {
            self.try_connect()?;
        }
        let Some(client) = self.client.as_mut() else { return Err(format!("not connected to Trigon Studio at {}", self.addr)) };
        match client.call(method, params) {
            Ok(v) => Ok(v),
            Err(CallError::App(e)) => Err(e),
            Err(e @ CallError::Connection(_)) => {
                // Not sent again: it may have run. The next call reconnects.
                self.client = None;
                Err(format!("{e}; the request may or may not have run (document_info shows the model)"))
            }
        }
    }
}

impl Backend for Remote {
    fn run_command(&mut self, command: &str, params: Value) -> Result<Value, String> {
        self.call("command.run", json!({"command": command, "params": params}))
    }
    fn has_studio(&self) -> bool {
        true
    }
    fn studio(&mut self, method: &str, params: Value) -> Result<Value, String> {
        self.call(method, params)
    }
    fn hello(&mut self, client: &str) {
        self.client_name = Some(client.to_string());
        if let Some(c) = self.client.as_mut() {
            let _ = c.call("control.hello", json!({"client": client}));
        }
    }
}

fn tool_name(id: &str) -> String {
    id.replace('.', "_")
}

/// The tools only a Trigon Studio answers.
fn studio_tools() -> Value {
    json!([
        {
            "name": "studio_view",
            "description": "Change what the owner sees in Trigon Studio and return the view: mode (3d, plan or drawing), drawing (a saved drawing view id), level (the plan's \
                            level; in 3D the top level shown, cut 1.2 m above its floor; null shows everything), phase (a phase id), \
                            filter (complete, demolition, new, all), fit (true frames the whole model), focus (an element id such as a \
                            facade's wall \"b1.f3\" or a placement: frames it, seen from outside its facade).",
            "inputSchema": {"type": "object", "properties": {
                "mode": {"type": "string", "enum": ["3d", "plan", "drawing"]},
                "drawing": {"type":"string", "maxLength":80},
                "level": {"type": ["integer", "null"], "minimum": 0},
                "phase": {"type": "string"},
                "filter": {"type": "string", "enum": ["complete", "demolition", "new", "all"]},
                "fit": {"type": "boolean"},
                "focus": {"type": "string"}
            }, "additionalProperties": false}
        },
        {
            "name": "studio_screenshot",
            "description": "See what the owner sees in Trigon Studio: a PNG of the 3D or plan viewport (default) or of the whole \
                            window with the activity feed (target: window). A covered window can't be captured; raise: true brings it \
                            to the front first, taking focus from the owner, so only when they asked for it.",
            "inputSchema": {"type": "object", "properties": {
                "target": {"type": "string", "enum": ["viewport", "window"]},
                "raise": {"type": "boolean"}
            }, "additionalProperties": false}
        }
    ])
}

fn text_content(v: &Value) -> Value {
    let text = match v {
        Value::String(s) => s.clone(),
        other => serde_json::to_string_pretty(other).unwrap_or_default(),
    };
    json!({"type": "text", "text": text})
}

/// A Studio tool call's MCP result.
fn call_studio_tool(backend: &mut dyn Backend, name: &str, args: Value) -> Value {
    let (method, args) = match name {
        "studio_view" => ("ui.view", args),
        _ => (
            "ui.screenshot",
            json!({"target": args.get("target").cloned().unwrap_or(json!("viewport")), "raise": args.get("raise").cloned().unwrap_or(json!(false))}),
        ),
    };
    match backend.studio(method, args) {
        Ok(mut v) => {
            let image = v.as_object_mut().and_then(|o| o.remove("png_base64"));
            let mut content = vec![text_content(&v)];
            if let Some(Value::String(data)) = image {
                content.push(json!({"type": "image", "data": data, "mimeType": "image/png"}));
            }
            json!({"content": content, "isError": false})
        }
        Err(e) => json!({"content": [text_content(&Value::String(e))], "isError": true}),
    }
}

/// Handles one JSON-RPC message; `None` for notifications.
pub fn handle(backend: &mut dyn Backend, msg: &Value) -> Option<Value> {
    let id = msg.get("id").cloned();
    let method = msg.get("method").and_then(Value::as_str).unwrap_or_default();
    let id = id?; // Notifications get no reply.
    let ok = |result: Value| Some(json!({"jsonrpc": "2.0", "id": id, "result": result}));
    let err = |code: i64, message: &str| Some(json!({"jsonrpc": "2.0", "id": id, "error": {"code": code, "message": message}}));
    match method {
        "initialize" => {
            let asked = msg.pointer("/params/protocolVersion").and_then(Value::as_str).unwrap_or_default();
            let version = PROTOCOL_VERSIONS.iter().find(|v| **v == asked).unwrap_or(&PROTOCOL_VERSIONS[0]);
            if let Some(name) = msg.pointer("/params/clientInfo/name").and_then(Value::as_str) {
                backend.hello(name);
            }
            let instructions = if backend.has_studio() { format!("{INSTRUCTIONS}{STUDIO_INSTRUCTIONS}") } else { INSTRUCTIONS.to_string() };
            ok(json!({
                "protocolVersion": version,
                "capabilities": {"tools": {"listChanged": false}},
                "serverInfo": {"name": "trigon", "version": env!("CARGO_PKG_VERSION")},
                "instructions": instructions,
            }))
        }
        "ping" => ok(json!({})),
        "tools/list" => {
            let mut tools: Vec<Value> =
                commands().iter().map(|c| json!({"name": tool_name(c.id), "description": c.summary, "inputSchema": c.params_schema()})).collect();
            if backend.has_studio()
                && let Value::Array(extra) = studio_tools()
            {
                tools.extend(extra);
            }
            ok(json!({"tools": tools}))
        }
        "tools/call" => {
            let name = msg.pointer("/params/name").and_then(Value::as_str).unwrap_or_default();
            let args = msg.pointer("/params/arguments").cloned().unwrap_or(Value::Null);
            if backend.has_studio() && matches!(name, "studio_view" | "studio_screenshot") {
                return ok(call_studio_tool(backend, name, args));
            }
            let Some(spec) = commands().iter().find(|c| tool_name(c.id) == name) else {
                return err(-32602, &format!("unknown tool {name}"));
            };
            let (text, is_error, image) = match backend.run_command(spec.id, args) {
                Ok(v) => (serde_json::to_string_pretty(&v).unwrap_or_default(), false, image_of(&v)),
                Err(e) => (e, true, None),
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
pub fn serve(backend: &mut dyn Backend, input: impl BufRead, mut output: impl Write) -> std::io::Result<()> {
    for line in input.lines() {
        let line = line?;
        if line.trim().is_empty() {
            continue;
        }
        let reply = if line.len() > MAX_LINE {
            Some(json!({"jsonrpc": "2.0", "id": null, "error": {"code": -32600, "message": "message too large"}}))
        } else {
            match serde_json::from_str::<Value>(&line) {
                Ok(msg) => handle(backend, &msg),
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
mod tests;
