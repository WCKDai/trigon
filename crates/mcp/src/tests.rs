//! MCP over a headless session, and bridged to a stand-in Studio over a real control channel.

use std::net::TcpListener;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use serde_json::{Value, json};
use trigon_control::{Options, Server, methods};
use trigon_data::{Credentials, Live};
use trigon_engine::{Session, commands};

use super::*;

const TOKEN: &str = "fedcba9876543210fedcba9876543210fedcba9876543210fedcba9876543210";

fn session() -> Session {
    // Only local commands run in these tests; nothing reaches the network.
    Session::new(Box::new(Live::new(Credentials::default())))
}

fn rpc(b: &mut dyn Backend, id: u64, method: &str, params: Value) -> Value {
    handle(b, &json!({"jsonrpc": "2.0", "id": id, "method": method, "params": params})).expect("a reply")
}

fn tool_names(b: &mut dyn Backend) -> Vec<String> {
    let r = rpc(b, 1, "tools/list", json!({}));
    r["result"]["tools"].as_array().unwrap().iter().map(|t| t["name"].as_str().unwrap().to_string()).collect()
}

#[test]
fn base64_matches_rfc_4648() {
    assert_eq!(base64(b""), "");
    assert_eq!(base64(b"f"), "Zg==");
    assert_eq!(base64(b"fo"), "Zm8=");
    assert_eq!(base64(b"foo"), "Zm9v");
    assert_eq!(base64(b"foobar"), "Zm9vYmFy");
}

#[test]
fn headless_sessions_serve_every_command_and_no_studio_tools() {
    let mut s = session();
    let r = rpc(&mut s, 1, "initialize", json!({"protocolVersion": "2025-03-26", "clientInfo": {"name": "test"}}));
    assert_eq!(r["result"]["protocolVersion"], "2025-03-26");
    let names = tool_names(&mut s);
    assert!(names.contains(&"facade_place".to_string()));
    assert!(!names.iter().any(|n| n.starts_with("studio_")));
    let r = rpc(&mut s, 2, "tools/call", json!({"name": "document_info", "arguments": {}}));
    assert_eq!(r["result"]["isError"], false, "{r}");
    let before = s.doc.clone();
    for name in ["studio_view", "studio_screenshot", "studio_click", "studio_drag", "studio_key"] {
        let r = rpc(&mut s, 3, "tools/call", json!({"name": name, "arguments": {}}));
        assert_eq!(r["error"]["code"], -32602, "{r}");
        assert!(r["error"]["message"].as_str().unwrap().contains(&format!("unknown tool {name}")), "{r}");
    }
    assert_eq!(s.doc, before, "Studio requests cannot change a headless document");
    // Notifications get no reply.
    assert!(handle(&mut s, &json!({"jsonrpc": "2.0", "method": "notifications/initialized"})).is_none());
}

#[test]
fn studio_tool_names_never_shadow_a_command() {
    for c in commands() {
        assert!(!tool_name(c.id).starts_with("studio_"), "{}", c.id);
    }
}

#[test]
fn serve_reads_lines_and_answers_garbage() {
    let mut s = session();
    let input = "\n{not json\n{\"jsonrpc\":\"2.0\",\"id\":7,\"method\":\"ping\"}\n";
    let mut out = Vec::new();
    serve(&mut s, input.as_bytes(), &mut out).unwrap();
    let lines: Vec<Value> = String::from_utf8(out).unwrap().lines().map(|l| serde_json::from_str(l).unwrap()).collect();
    assert_eq!(lines.len(), 2);
    assert_eq!(lines[0]["error"]["code"], -32700);
    assert_eq!(lines[1]["id"], 7);
}

/// A stand-in Studio: a control server and an app thread that runs `command.run` through its own
/// session and answers the `ui.*` methods with canned replies.
fn studio() -> (u16, Arc<Mutex<Vec<Value>>>, Arc<trigon_control::Presence>) {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let server = Server::start_with(listener, TOKEN.into(), Arc::new(|| {}), Options::default()).unwrap();
    let (port, presence) = (server.port(), server.presence());
    let seen = Arc::new(Mutex::new(Vec::new()));
    let log = seen.clone();
    std::thread::spawn(move || {
        let mut s = session();
        while let Some(req) = server.recv_timeout(Duration::from_secs(60)) {
            log.lock().unwrap().push(json!({"method": req.method, "params": req.params}));
            let r = match req.method.as_str() {
                "ui.view" => Ok(json!({"mode": req.params.get("mode").cloned().unwrap_or(json!("3d")), "level": null})),
                "ui.screenshot" => Ok(json!({"width": 2, "height": 1, "target": req.params["target"], "png_base64": "iVBORw0KGgo="})),
                "ui.drag" if req.params.get("steps") == Some(&json!(0)) => Err("ui.drag: `steps` must be an integer from 1 to 60".into()),
                "ui.click" | "ui.drag" | "ui.key" => Ok(json!({"queued": 3, "window_hidden": false, "journal_len": s.doc.journal.len()})),
                m => methods::session_method(&mut s, m, &req.params).unwrap_or_else(|| Err(methods::unknown_method(m))),
            };
            req.respond(r);
        }
    });
    (port, seen, presence)
}

#[test]
fn bridged_tool_calls_run_in_the_studio_session() {
    let (port, seen, presence) = studio();
    let mut remote = Remote::new(&format!("127.0.0.1:{port}"), Some(TOKEN));
    remote.try_connect().unwrap();
    let r = rpc(&mut remote, 1, "initialize", json!({"protocolVersion": "2025-06-18", "clientInfo": {"name": "claude-code"}}));
    assert!(r["result"]["instructions"].as_str().unwrap().contains("Trigon Studio"));
    let names = tool_names(&mut remote);
    assert!(names.contains(&"studio_view".to_string()) && names.contains(&"studio_screenshot".to_string()));
    assert!(names.contains(&"interior_list".to_string()));

    let r = rpc(&mut remote, 2, "tools/call", json!({"name": "document_info", "arguments": {}}));
    assert_eq!(r["result"]["isError"], false, "{r}");
    let r = rpc(&mut remote, 3, "tools/call", json!({"name": "facade_place", "arguments": {"facade": "nope"}}));
    assert_eq!(r["result"]["isError"], true, "{r}");
    let r = rpc(&mut remote, 4, "tools/call", json!({"name": "studio_view", "arguments": {"mode": "plan"}}));
    assert_eq!(r["result"]["isError"], false, "{r}");
    let r = rpc(&mut remote, 5, "tools/call", json!({"name": "studio_screenshot", "arguments": {"target": "window"}}));
    let content = r["result"]["content"].as_array().unwrap();
    assert_eq!(content[1]["type"], "image", "{r}");
    assert_eq!(content[1]["data"], "iVBORw0KGgo=");
    assert!(!content[0]["text"].as_str().unwrap().contains("iVBOR"), "the image goes once, as an image");

    assert_eq!(
        *seen.lock().unwrap(),
        [
            json!({"method": "command.run", "params": {"command": "document.info", "params": {}}}),
            json!({"method": "command.run", "params": {"command": "facade.place", "params": {"facade": "nope"}}}),
            json!({"method": "ui.view", "params": {"mode": "plan"}}),
            json!({"method": "ui.screenshot", "params": {"target": "window", "raise": false}}),
        ],
        "every call ran in the Studio's session"
    );
    assert_eq!(presence.clients().values().cloned().collect::<Vec<_>>(), ["claude-code"], "the owner sees who is working");
}

#[test]
fn studio_input_tools_have_bounded_typed_schemas() {
    let mut remote = Remote::new("127.0.0.1:1", None);
    let listed = rpc(&mut remote, 1, "tools/list", json!({}));
    let tools = listed["result"]["tools"].as_array().unwrap();
    let schema = |name: &str| &tools.iter().find(|tool| tool["name"] == name).unwrap()["inputSchema"];
    let coord = json!({"type": "number", "minimum": 0, "maximum": 20000});
    let click = schema("studio_click");
    assert_eq!(click["required"], json!(["x", "y"]));
    assert_eq!(click["properties"]["x"], coord);
    assert_eq!(click["properties"]["y"], coord);
    assert_eq!(click["properties"]["button"]["enum"], json!(["left", "right", "middle"]));
    assert_eq!(click["properties"]["button"]["default"], "left");
    assert_eq!(click["properties"]["double"]["type"], "boolean");
    let drag = schema("studio_drag");
    assert_eq!(drag["required"], json!(["from", "to"]));
    for point in ["from", "to"] {
        assert_eq!(drag["properties"][point]["type"], "array");
        assert_eq!(drag["properties"][point]["minItems"], 2);
        assert_eq!(drag["properties"][point]["maxItems"], 2);
        assert_eq!(drag["properties"][point]["items"], coord);
    }
    assert_eq!(drag["properties"]["steps"], json!({"type": "integer", "minimum": 1, "maximum": 60, "default": 8}));
    let key = schema("studio_key");
    assert_eq!(key["required"], json!(["key"]));
    assert_eq!(key["properties"]["key"]["type"], "string");
    for name in ["studio_click", "studio_drag", "studio_key"] {
        let tool = tools.iter().find(|tool| tool["name"] == name).unwrap();
        assert!(!tool["description"].as_str().unwrap().is_empty());
        assert_eq!(tool["inputSchema"]["additionalProperties"], false);
        for modifier in ["command", "shift", "alt", "ctrl"] {
            assert_eq!(tool["inputSchema"]["properties"][modifier]["type"], "boolean");
        }
    }
}

#[test]
fn studio_input_tools_forward_ui_methods_and_params_without_command_run() {
    let (port, seen, _) = studio();
    let mut remote = Remote::new(&format!("127.0.0.1:{port}"), Some(TOKEN));
    remote.try_connect().unwrap();
    let cases = [
        (
            "studio_click",
            "ui.click",
            json!({"x": 10.25, "y": 20.5, "button": "right", "double": true, "command": true, "shift": false, "alt": true, "ctrl": false}),
        ),
        ("studio_click", "ui.click", json!({"x": 0, "y": 20000, "button": "middle"})),
        ("studio_click", "ui.click", json!({"x": 15, "y": 30})),
        (
            "studio_drag",
            "ui.drag",
            json!({"from": [10.25, 20.5], "to": [110.75, 220.5], "steps": 60, "command": false, "shift": true, "alt": false, "ctrl": true}),
        ),
        ("studio_drag", "ui.drag", json!({"from": [0, 0], "to": [20000, 20000], "steps": 1})),
        ("studio_drag", "ui.drag", json!({"from": [10, 20], "to": [30, 40]})),
        ("studio_key", "ui.key", json!({"key": "Z", "command": true, "shift": true, "alt": false, "ctrl": false})),
        ("studio_key", "ui.key", json!({"key": "Escape"})),
    ];
    for (name, _, params) in &cases {
        let r = rpc(&mut remote, 2, "tools/call", json!({"name": name, "arguments": params}));
        assert_eq!(r["result"]["isError"], false, "{r}");
        let body: Value = serde_json::from_str(r["result"]["content"][0]["text"].as_str().unwrap()).unwrap();
        assert_eq!(body["journal_len"], 0, "input reaches UI without a command journal entry");
    }
    assert_eq!(
        *seen.lock().unwrap(),
        cases.iter().map(|(_, method, params)| json!({"method": method, "params": params})).collect::<Vec<_>>(),
        "only the named UI methods reach the authenticated Studio, with unchanged arguments"
    );
    let before = seen.lock().unwrap().len();
    for name in ["studio_delete", "studio_ui_click", "ui.click"] {
        let r = rpc(&mut remote, 3, "tools/call", json!({"name": name, "arguments": {"x": 10, "y": 20}}));
        assert_eq!(r["error"]["code"], -32602, "{r}");
    }
    assert_eq!(seen.lock().unwrap().len(), before, "unknown tool names never become arbitrary UI methods");
}

#[test]
fn studio_input_refusals_reach_mcp_without_retry() {
    let (port, seen, _) = studio();
    let mut remote = Remote::new(&format!("127.0.0.1:{port}"), Some(TOKEN));
    let params = json!({"from": [0, 0], "to": [1, 1], "steps": 0});
    let r = rpc(&mut remote, 1, "tools/call", json!({"name": "studio_drag", "arguments": params}));
    assert_eq!(r["result"]["isError"], true, "{r}");
    assert_eq!(r["result"]["content"][0]["text"], "ui.drag: `steps` must be an integer from 1 to 60");
    assert_eq!(*seen.lock().unwrap(), [json!({"method": "ui.drag", "params": params})]);
}

#[test]
fn a_studio_that_is_gone_gives_tool_errors_not_a_crash() {
    let port = {
        let l = TcpListener::bind("127.0.0.1:0").unwrap();
        l.local_addr().unwrap().port()
    };
    let mut remote = Remote::new(&format!("127.0.0.1:{port}"), Some(TOKEN));
    assert!(remote.try_connect().is_err());
    let r = rpc(&mut remote, 1, "tools/call", json!({"name": "document_info", "arguments": {}}));
    assert_eq!(r["result"]["isError"], true, "{r}");
    assert!(r["result"]["content"][0]["text"].as_str().unwrap().contains("Trigon Studio"), "{r}");
}

#[test]
fn precision_workflow_is_discoverable_and_atomic_through_mcp() {
    let mut s = session();
    s.doc.buildings.push(
        serde_json::from_value(json!({"id":"b","name":"MCP precision",
        "footprint":[[0,0],[12000,0],[12000,8000],[0,8000]],"wall_thickness":350,
        "storeys":[{"name":"Ground","elevation":0,"height":3000},{"name":"First","elevation":3000,"height":3000}],
        "facades":[],"evidence":[],"entrances":[]}))
        .unwrap(),
    );
    s.execute("interior.add_walls", json!({"walls":[{"id":"wall","levels":[0,1],"a":[1000,3000],"b":[5000,3000],"thickness":100}]})).unwrap();
    let names = tool_names(&mut s);
    for name in [
        "precision_audit",
        "precision_preview",
        "precision_apply",
        "view_audit",
        "edit_connect_walls",
        "annotation_move_label",
        "drawing_measure_stair",
    ] {
        assert!(names.iter().any(|actual| actual == name));
    }
    let before = s.doc.clone();
    let action = json!({"command":"edit.move","params":{"targets":["wall.L0"],"delta":[0.125,0.375,0]}});
    let preview = rpc(&mut s, 1, "tools/call", json!({"name":"precision_preview","arguments":{"action":action}}));
    assert_eq!(preview["result"]["isError"], false, "{preview}");
    let body: Value = serde_json::from_str(preview["result"]["content"][0]["text"].as_str().unwrap()).unwrap();
    assert_eq!(s.doc, before);
    let applied = rpc(&mut s, 2, "tools/call", json!({"name":"precision_apply","arguments":{"preview_token":body["preview_token"],"action":action}}));
    assert_eq!(applied["result"]["isError"], false, "{applied}");
    assert_eq!(s.doc.journal.len(), before.journal.len() + 1);
    let stale = rpc(&mut s, 3, "tools/call", json!({"name":"precision_apply","arguments":{"preview_token":body["preview_token"],"action":action}}));
    assert_eq!(stale["result"]["isError"], true);
    let undone = rpc(&mut s, 4, "tools/call", json!({"name":"edit_undo","arguments":{}}));
    assert_eq!(undone["result"]["isError"], false);
    assert_eq!(s.doc, before);
}

#[test]
fn drawing_preflight_schema_and_shaped_feedback_reach_mcp_clients() {
    let mut s = session();
    s.execute(
        "building.add",
        json!({"id":"b","name":"MCP drawing fixture","footprint":[[0,0],[12000,0],[12000,8000],[0,8000]],"storeys":1,"storey_height_mm":3000}),
    )
    .unwrap();
    let listed = rpc(&mut s, 1, "tools/list", json!({}));
    let schema =
        listed["result"]["tools"].as_array().unwrap().iter().find(|tool| tool["name"] == "precision_preview").unwrap()["inputSchema"].to_string();
    for action in ["view.define", "view.configure", "annotation.define"] {
        assert!(schema.contains(action), "discoverable action {action}");
    }
    let actions = [
        json!({"command":"view.define","params":{"id":"p","name":"Plan","kind":{"view":"plan","level":0}}}),
        json!({"command":"view.configure","params":{"id":"p","template":{"name":"100","sizing":{"mode":"fixed","denominator":100,"centre":[6000,4000]}}}}),
        json!({"command":"annotation.define","params":{"id":"contract-note","view":"p","kind":{"annotation":"text","at":[2000,4000],"text":"MCP label"}}}),
    ];
    for action in actions {
        let before = s.doc.clone();
        let reply = rpc(&mut s, 2, "tools/call", json!({"name":"precision_preview","arguments":{"action":action}}));
        assert_eq!(reply["result"]["isError"], false, "{reply}");
        let preview: Value = serde_json::from_str(reply["result"]["content"][0]["text"].as_str().unwrap()).unwrap();
        assert_eq!(s.doc, before);
        assert_eq!(preview["validation"]["drawing_render"], "passed");
        assert_eq!(preview["drawing_feedback"][0]["view"], "p");
        if action["command"] == "annotation.define" {
            let label =
                preview["drawing_feedback"][0]["labels"].as_array().unwrap().iter().find(|label| label["source"]["id"] == "contract-note").unwrap();
            assert_eq!(label["text"], "MCP label");
            assert_eq!(label["ink_corners_mm"].as_array().unwrap().len(), 4);
        }
        let reply = rpc(
            &mut s,
            3,
            "tools/call",
            json!({"name":"precision_apply","arguments":{"action":preview["action"],"preview_token":preview["preview_token"]}}),
        );
        assert_eq!(reply["result"]["isError"], false, "{reply}");
        assert_eq!(s.doc.journal.len(), before.journal.len() + 1);
        assert_eq!(s.doc.buildings, before.buildings);
    }
}
