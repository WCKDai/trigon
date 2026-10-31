//! The control server against a real session on an "app" thread, as Trigon Studio runs it.

use std::io::{BufRead, BufReader, Write};
use std::net::TcpStream;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use serde_json::{Value, json};
use trigon_data::{Address, DataError, Footprint, Heights, Photo, Provider, Region, Register};
use trigon_engine::Session;

use super::*;
use crate::methods;

const TOKEN: &str = "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef";

/// No data at all: tests never reach the network.
struct Offline;

impl Provider for Offline {
    fn geocode(&self, _: &str) -> trigon_data::Result<Address> {
        Err(DataError::Unavailable("offline".into()))
    }
    fn footprint(&self, _: &Address) -> trigon_data::Result<Footprint> {
        Err(DataError::Unavailable("offline".into()))
    }
    fn heights(&self, _: &[[f64; 2]]) -> trigon_data::Result<Heights> {
        Err(DataError::Unavailable("offline".into()))
    }
    fn photos(&self, _: [f64; 2]) -> trigon_data::Result<Vec<Photo>> {
        Err(DataError::Unavailable("offline".into()))
    }
    fn register(&self, _: &str) -> trigon_data::Result<Option<Register>> {
        Ok(None)
    }
    fn photo_region(&self, _: &Photo, _: [u32; 4]) -> trigon_data::Result<Region> {
        Err(DataError::Unavailable("offline".into()))
    }
}

struct App {
    port: u16,
    presence: Arc<Presence>,
    /// Methods that reached the app, in order.
    seen: Arc<Mutex<Vec<String>>>,
    /// Lets a held `test.slow` request go.
    gate: std::sync::mpsc::Sender<()>,
}

/// A server and an app thread that answers its requests through one session; `test.slow` waits
/// for the gate.
fn start(options: Options) -> App {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let server = Server::start_with(listener, TOKEN.into(), Arc::new(|| {}), options).unwrap();
    let (port, presence) = (server.port(), server.presence());
    let seen = Arc::new(Mutex::new(Vec::new()));
    let (gate, gate_rx) = std::sync::mpsc::channel::<()>();
    let log = seen.clone();
    std::thread::spawn(move || {
        let mut s = Session::new(Box::new(Offline));
        while let Some(req) = server.recv_timeout(Duration::from_secs(60)) {
            log.lock().unwrap().push(req.method.clone());
            if req.method == "test.slow" {
                let _ = gate_rx.recv();
                req.respond(Ok(json!("slow")));
                continue;
            }
            let r = methods::session_method(&mut s, &req.method, &req.params).unwrap_or_else(|| Err(methods::unknown_method(&req.method)));
            req.respond(r);
        }
    });
    App { port, presence, seen, gate }
}

struct Conn {
    out: TcpStream,
    lines: BufReader<TcpStream>,
}

impl Conn {
    fn open(port: u16) -> Conn {
        let s = TcpStream::connect(("127.0.0.1", port)).unwrap();
        s.set_read_timeout(Some(Duration::from_secs(20))).unwrap();
        Conn { out: s.try_clone().unwrap(), lines: BufReader::new(s) }
    }

    fn send(&mut self, bytes: &[u8]) {
        let _ = self.out.write_all(bytes);
    }

    /// The next reply, or `None` once the server closed the connection.
    fn read(&mut self) -> Option<Value> {
        let mut l = String::new();
        match self.lines.read_line(&mut l) {
            Ok(0) | Err(_) => None,
            Ok(_) => Some(serde_json::from_str(&l).unwrap()),
        }
    }

    fn ask(&mut self, v: Value) -> Value {
        self.send(format!("{v}\n").as_bytes());
        self.read().expect("a reply")
    }

    fn authed(port: u16) -> Conn {
        let mut c = Conn::open(port);
        let r = c.ask(json!({"id": 0, "method": "control.hello", "params": {"client": "test"}, "token": TOKEN}));
        assert_eq!(r["ok"], true, "{r}");
        c
    }
}

fn is_error(r: &Value, prefix: &str) -> bool {
    r["ok"] == false && r["error"].as_str().is_some_and(|e| e.starts_with(prefix))
}

#[test]
fn a_first_request_without_the_token_is_refused_and_nothing_runs() {
    let app = start(Options::default());
    let first_lines: [&[u8]; 6] = [
        b"{\"id\":1,\"method\":\"commands.list\"}\n",
        b"{\"id\":1,\"method\":\"commands.list\",\"token\":\"0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdee\"}\n",
        b"{\"id\":1,\"method\":\"commands.list\",\"token\":12345}\n",
        b"{\"id\":1,\"method\":\"commands.list\",\"token\":\"0123\"}\n",
        b"this is not json\n",
        b"[]\n",
    ];
    for line in first_lines {
        let mut c = Conn::open(app.port);
        c.send(line);
        let r = c.read().expect("a refusal");
        assert!(is_error(&r, "unauthorized"), "{r}");
        assert!(c.read().is_none(), "the connection is closed after a refusal");
    }
    assert!(app.seen.lock().unwrap().is_empty(), "nothing reached the app");
    assert_eq!(app.presence.agents(), 0);
}

#[test]
fn with_the_token_requests_run_through_the_session() {
    let app = start(Options::default());
    let mut c = Conn::open(app.port);
    let r = c.ask(json!({"id": 1, "method": "commands.list", "token": TOKEN}));
    assert_eq!(r["ok"], true, "{r}");
    assert!(r["result"].as_array().unwrap().iter().any(|c| c["id"] == "facade.place"));
    // Later requests on the connection need no token; ids of any JSON type come back.
    let r = c.ask(json!({"id": "two", "method": "command.run", "params": {"command": "document.info"}}));
    assert_eq!((&r["ok"], &r["id"]), (&json!(true), &json!("two")), "{r}");
    assert!(r["result"]["buildings"].is_array());
    let r = c.ask(json!({"id": 3, "method": "document.summary"}));
    assert_eq!(r["result"]["unsaved"], false, "{r}");
    let r = c.ask(json!({"id": 4, "method": "command.run", "params": {"command": "nope.nope"}}));
    assert!(is_error(&r, "unknown command"), "{r}");
    let r = c.ask(json!({"id": 5, "method": "ui.unknown"}));
    assert!(is_error(&r, "unknown method"), "{r}");
}

#[test]
fn malformed_lines_never_panic_and_eight_close_the_connection() {
    let app = start(Options::default());
    let mut c = Conn::authed(app.port);
    // Valid JSON that is not a proper request: answered, and the connection stays.
    for v in [json!([]), json!(42), json!("x"), json!(null), json!({"method": 5}), json!({"method": "command.run", "params": "x"})] {
        let r = c.ask(v.clone());
        assert_eq!(r["ok"], false, "{v} → {r}");
    }
    for i in 0..MAX_BAD_LINES - 1 {
        c.send(format!("{{not json {i}\n").as_bytes());
        let r = c.read().expect("an answer");
        assert!(is_error(&r, "bad JSON"), "{r}");
    }
    c.send(b"\xff\xfe\x00 invalid utf-8\n");
    let r = c.read().expect("an answer");
    assert!(is_error(&r, "bad JSON"), "{r}");
    assert!(c.read().is_none(), "the eighth bad line closes the connection");
    // The server lives on.
    let mut c = Conn::authed(app.port);
    assert_eq!(c.ask(json!({"id": 1, "method": "commands.list"}))["ok"], true);
}

#[test]
fn hostile_command_run_params_never_panic() {
    let app = start(Options::default());
    let mut c = Conn::authed(app.port);
    let long = "x".repeat(100_000);
    let hostile = [
        json!(null),
        json!([]),
        json!({"command": null}),
        json!({"command": 5}),
        json!({"command": long}),
        json!({"command": "facade.place", "params": [1, 2]}),
        json!({"command": "facade.place", "params": {"facade": "b9.f9", "type": "window.fixed", "levels": [usize::MAX]}}),
        json!({"command": "building.set_storeys", "params": {"count": usize::MAX, "height": f64::MAX}}),
        json!({"command": "document.open", "params": {"path": "/etc/passwd"}}),
        json!({"command": "document.save", "params": {"path": "/dev/null/x.trigon"}}),
        json!({"command": "site.load_address", "params": {"address": ""}}),
        json!({"command": "edit.undo", "params": {"extra": true}}),
    ];
    for (i, p) in hostile.iter().enumerate() {
        let r = c.ask(json!({"id": i, "method": "command.run", "params": p}));
        assert!(r["ok"].is_boolean() && r["id"] == json!(i), "{r}");
        if r["ok"] == false {
            let e = r["error"].as_str().unwrap();
            assert!(!e.contains("internal error"), "a command panicked: {e}");
            assert!(e.len() < 2000, "errors stay short: {}", e.len());
        }
    }
    assert_eq!(c.ask(json!({"id": 99, "method": "commands.list"}))["ok"], true);
}

#[test]
fn an_oversized_line_is_refused_and_closes_the_connection() {
    let app = start(Options { max_line: 1024, ..Options::default() });
    for authed in [false, true] {
        let mut c = if authed { Conn::authed(app.port) } else { Conn::open(app.port) };
        let mut big = vec![b'a'; 4096];
        big.push(b'\n');
        c.send(&big);
        let r = c.read().expect("a refusal");
        assert!(is_error(&r, "request too large"), "{r}");
        assert!(c.read().is_none());
    }
}

#[test]
fn connections_beyond_the_limit_are_turned_away() {
    let app = start(Options { max_connections: 2, ..Options::default() });
    let _a = Conn::authed(app.port);
    let _b = Conn::authed(app.port);
    let mut c = Conn::open(app.port);
    let r = c.read().expect("a refusal");
    assert!(is_error(&r, "too many connections"), "{r}");
    drop(_a);
    // A slot frees once a connection closes.
    let deadline = std::time::Instant::now() + Duration::from_secs(5);
    loop {
        let mut d = Conn::open(app.port);
        let r = d.ask(json!({"id": 1, "method": "commands.list", "token": TOKEN}));
        if r["ok"] == true {
            break;
        }
        assert!(std::time::Instant::now() < deadline, "{r}");
        std::thread::sleep(Duration::from_millis(20));
    }
}

/// A request that outlasts its wait says it is still running; the connection keeps its reply for
/// `control.pending`, and refuses other requests as `busy` until it lands.
#[test]
fn a_timed_out_request_can_be_asked_for_later() {
    let app = start(Options { reply_wait: Duration::from_millis(100), ..Options::default() });
    let mut c = Conn::authed(app.port);
    let r = c.ask(json!({"id": 1, "method": "test.slow"}));
    assert!(r["running"] == true && is_error(&r, "timeout"), "{r}");
    assert!(app.presence.busy());
    let r = c.ask(json!({"id": 2, "method": "commands.list"}));
    assert!(r["busy"] == true && r["id"] == 2, "{r}");
    let r = c.ask(json!({"id": 3, "method": "control.pending"}));
    assert_eq!((&r["result"]["pending"], &r["result"]["done"]), (&json!(1), &json!(false)), "{r}");
    app.gate.send(()).unwrap();
    let deadline = std::time::Instant::now() + Duration::from_secs(10);
    let r = loop {
        let r = c.ask(json!({"id": 4, "method": "control.pending"}));
        if r["result"]["done"] == true || std::time::Instant::now() > deadline {
            break r;
        }
        std::thread::sleep(Duration::from_millis(10));
    };
    assert_eq!((&r["result"]["done"], &r["result"]["reply"]["result"]), (&json!(true), &json!("slow")), "{r}");
    assert_eq!(c.ask(json!({"id": 5, "method": "control.pending"}))["result"]["pending"], Value::Null);
    assert_eq!(c.ask(json!({"id": 6, "method": "commands.list"}))["ok"], true);
    assert_eq!(*app.seen.lock().unwrap(), ["test.slow", "commands.list"], "the busy request never reached the app");
}

#[test]
fn random_bytes_never_take_the_server_down() {
    let app = start(Options { max_line: 4096, ..Options::default() });
    let mut seed = 0x2545_f491_4f6c_dd1du64;
    let mut next = || {
        seed ^= seed << 13;
        seed ^= seed >> 7;
        seed ^= seed << 17;
        seed
    };
    for round in 0..20 {
        let mut c = if round % 2 == 0 { Conn::authed(app.port) } else { Conn::open(app.port) };
        for _ in 0..6 {
            let len = (next() % 300) as usize;
            let mut line: Vec<u8> = (0..len).map(|_| (next() % 256) as u8).filter(|b| *b != b'\n').collect();
            if next() % 3 == 0 {
                // Something that parses: a request with odd members.
                line = format!(
                    r#"{{"id":{},"method":"command.run","params":{{"command":"facade.place","params":{{"facade":{}}}}}}}"#,
                    next() % 1000,
                    next() % 7
                )
                .into_bytes();
            }
            line.push(b'\n');
            c.send(&line);
            if c.read().is_none() {
                break;
            }
        }
    }
    let mut c = Conn::authed(app.port);
    assert_eq!(c.ask(json!({"id": 1, "method": "commands.list"}))["ok"], true);
}

#[test]
fn presence_counts_authenticated_connections_and_their_names() {
    let app = start(Options::default());
    let mut c = Conn::open(app.port);
    std::thread::sleep(Duration::from_millis(50));
    assert_eq!(app.presence.agents(), 0, "not before the token");
    let r = c.ask(json!({"id": 1, "method": "control.hello", "params": {"client": "claude\u{7}-code\n"}, "token": TOKEN}));
    assert_eq!(r["result"]["client"], "claude-code", "{r}");
    assert_eq!(app.presence.agents(), 1);
    assert_eq!(app.presence.clients().values().cloned().collect::<Vec<_>>(), ["claude-code"]);
    assert!(app.presence.since_last_request().is_some_and(|d| d < Duration::from_secs(5)));
    drop(c);
    let deadline = std::time::Instant::now() + Duration::from_secs(5);
    while app.presence.agents() != 0 {
        assert!(std::time::Instant::now() < deadline, "the agent count drops when the connection closes");
        std::thread::sleep(Duration::from_millis(10));
    }
    assert!(app.presence.clients().is_empty());
}

#[test]
fn only_loopback_and_long_tokens_are_served() {
    assert!(check_loopback("127.0.0.1:27900".parse().unwrap()).is_ok());
    assert!(check_loopback("[::1]:27900".parse().unwrap()).is_ok());
    assert!(check_loopback("0.0.0.0:27900".parse().unwrap()).is_err());
    assert!(check_loopback("192.168.1.5:27900".parse().unwrap()).is_err());
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    assert!(Server::start(listener, "short".into(), Arc::new(|| {})).is_err());
}

#[test]
fn the_client_authenticates_and_calls() {
    let app = start(Options::default());
    let addr = format!("127.0.0.1:{}", app.port);
    let mut client = crate::Client::connect(&addr, TOKEN).unwrap();
    let v = client.call("command.run", json!({"command": "types.list"})).unwrap();
    assert!(v.is_array() || v.is_object(), "{v}");
    match client.call("command.run", json!({"command": "nope"})) {
        Err(crate::CallError::App(e)) => assert!(e.starts_with("unknown command"), "{e}"),
        other => panic!("{other:?}"),
    }
    let wrong = format!("{}0", &TOKEN[..TOKEN.len() - 1]);
    assert!(crate::Client::connect(&addr, &wrong).is_err(), "a wrong token fails at connect");
}
