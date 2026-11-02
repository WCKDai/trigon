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
    let r = rpc(&mut s, 3, "tools/call", json!({"name": "studio_screenshot", "arguments": {}}));
    assert!(r["error"]["message"].as_str().unwrap().contains("unknown tool"), "{r}");
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
fn studio() -> (u16, Arc<Mutex<Vec<String>>>, Arc<trigon_control::Presence>) {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let server = Server::start_with(listener, TOKEN.into(), Arc::new(|| {}), Options::default()).unwrap();
    let (port, presence) = (server.port(), server.presence());
    let seen = Arc::new(Mutex::new(Vec::new()));
    let log = seen.clone();
    std::thread::spawn(move || {
        let mut s = session();
        while let Some(req) = server.recv_timeout(Duration::from_secs(60)) {
            let what = match req.params.get("command").and_then(Value::as_str) {
                Some(c) => format!("{} {c}", req.method),
                None => req.method.clone(),
            };
            log.lock().unwrap().push(what);
            let r = match req.method.as_str() {
                "ui.view" => Ok(json!({"mode": req.params.get("mode").cloned().unwrap_or(json!("3d")), "level": null})),
                "ui.screenshot" => Ok(json!({"width": 2, "height": 1, "target": req.params["target"], "png_base64": "iVBORw0KGgo="})),
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
        ["command.run document.info", "command.run facade.place", "ui.view", "ui.screenshot"],
        "every call ran in the Studio's session"
    );
    assert_eq!(presence.clients().values().cloned().collect::<Vec<_>>(), ["claude-code"], "the owner sees who is working");
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
