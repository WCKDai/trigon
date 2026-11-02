//! The control methods that need only the session, shared by Trigon Studio and the tests. The
//! `ui.*` methods belong to the app and are answered there.

use serde_json::{Value, json};
use trigon_engine::{Session, commands};

/// Every method the control channel answers, with a one-line description (docs/control-protocol.md).
pub const METHODS: &[(&str, &str)] = &[
    ("control.hello", "name this client (`client`), shown in the app's activity feed; answered by the server"),
    ("control.pending", "the reply of this connection's request that timed out, once it has one"),
    ("command.run", "run any engine command: `command` (dotted id), `params` (object)"),
    ("commands.list", "every engine command: id, summary, whether it changes the model; `schemas: true` adds parameter schemas"),
    ("document.summary", "the open document (document.info) with its file and unsaved state"),
    (
        "ui.view",
        "what the owner sees: `mode` (3d, plan, drawing), `drawing` (saved view id), `level`, `phase`, `filter`, `appearance` (materials, phases; 3D only), `fit`, `focus` (an element id); returns the view",
    ),
    (
        "ui.screenshot",
        "a PNG of the viewport (`target: viewport`, default) or the whole window (`window`; `raise: true` brings it to the front first); `path` writes it, else base64",
    ),
    ("ui.click", "click at window points `x`, `y`; optional `button`, `double` and modifiers"),
    ("ui.drag", "primary-button drag between window points `from: [x,y]` and `to: [x,y]`; optional `steps` (1..60) and modifiers"),
    ("ui.key", "press an egui `key`; optional command, shift, alt and ctrl modifiers"),
];

/// Longest command id accepted.
const MAX_COMMAND_ID: usize = 128;

/// The `command` and `params` of a `command.run` request.
pub fn command_run_params(params: &Value) -> Result<(String, Value), String> {
    let id = params.get("command").and_then(Value::as_str).ok_or("command.run needs `command`: a command id such as \"facade.list\"")?;
    if id.len() > MAX_COMMAND_ID || !commands().iter().any(|c| c.id == id) {
        let shown: String = id.chars().take(MAX_COMMAND_ID).collect();
        return Err(format!("unknown command {shown}; commands.list gives the list"));
    }
    let p = params.get("params").cloned().unwrap_or(Value::Null);
    Ok((id.to_string(), p))
}

/// `commands.list {schemas?}`.
pub fn commands_list(params: &Value) -> Value {
    let schemas = params.get("schemas").and_then(Value::as_bool).unwrap_or(false);
    Value::Array(
        commands()
            .iter()
            .map(|c| {
                let mut v = json!({"id": c.id, "summary": c.summary, "changes_model": c.mutates});
                if schemas {
                    v["params_schema"] = c.params_schema();
                }
                v
            })
            .collect(),
    )
}

/// `document.summary`: document.info plus where the document lives and whether it is saved.
pub fn document_summary(session: &mut Session) -> Result<Value, String> {
    let mut info = session.execute("document.info", Value::Null).map_err(|e| e.to_string())?;
    info["path"] = json!(session.path);
    info["unsaved"] = json!(session.dirty);
    Ok(info)
}

/// Answers the session methods (`command.run`, `commands.list`, `document.summary`); `None` for
/// any other method. An app that shows its work runs `command.run` itself, through its own log.
pub fn session_method(session: &mut Session, method: &str, params: &Value) -> Option<Result<Value, String>> {
    Some(match method {
        "command.run" => command_run_params(params).and_then(|(id, p)| session.execute(&id, p).map_err(|e| e.to_string())),
        "commands.list" => Ok(commands_list(params)),
        "document.summary" => document_summary(session),
        _ => return None,
    })
}

/// The reply to a method nobody answers.
pub fn unknown_method(method: &str) -> String {
    let shown: String = method.chars().take(128).collect();
    let known: Vec<&str> = METHODS.iter().map(|(m, _)| *m).collect();
    format!("unknown method `{shown}`; methods: {}", known.join(", "))
}
