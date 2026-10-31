//! A client of a running Trigon Studio's control channel (`trigon mcp` bridges through it).

use std::io::{BufRead, BufReader, Read, Write};
use std::net::{TcpStream, ToSocketAddrs};
use std::time::{Duration, Instant};

use serde_json::{Value, json};

use crate::server::{MAX_LINE, REPLY_WAIT};

/// How long a request that the app answered `running` is waited for in all.
const PATIENCE: Duration = Duration::from_secs(3600);

/// Why a call failed.
#[derive(Debug)]
pub enum CallError {
    /// The connection failed or closed: the app may have quit. Reconnecting may help.
    Connection(String),
    /// The app answered with an error.
    App(String),
}

impl std::fmt::Display for CallError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            CallError::Connection(e) => write!(f, "lost the connection to Trigon Studio: {e}"),
            CallError::App(e) => f.write_str(e),
        }
    }
}

pub struct Client {
    reader: BufReader<TcpStream>,
    out: TcpStream,
    token: String,
    next_id: u64,
}

impl Client {
    /// Connect to `addr` (`HOST:PORT`) and show `token`.
    pub fn connect(addr: &str, token: &str) -> std::io::Result<Client> {
        let mut last = std::io::Error::new(std::io::ErrorKind::NotFound, format!("cannot resolve {addr}"));
        for sa in addr.to_socket_addrs()? {
            match TcpStream::connect_timeout(&sa, Duration::from_millis(800)) {
                Ok(s) => {
                    // The app answers every request within REPLY_WAIT (at worst with `running`).
                    s.set_read_timeout(Some(REPLY_WAIT + Duration::from_secs(15)))?;
                    let _ = s.set_nodelay(true);
                    let out = s.try_clone()?;
                    let mut c = Client { reader: BufReader::new(s), out, token: token.to_string(), next_id: 0 };
                    // Authenticate now, so a wrong token shows at once and not on the first tool call.
                    c.call("control.hello", json!({"client": "trigon-mcp"}))
                        .map_err(|e| std::io::Error::new(std::io::ErrorKind::PermissionDenied, e.to_string()))?;
                    return Ok(c);
                }
                Err(e) => last = e,
            }
        }
        Err(last)
    }

    fn roundtrip(&mut self, method: &str, params: Value) -> Result<Value, CallError> {
        self.next_id += 1;
        let id = self.next_id;
        let line = json!({"id": id, "method": method, "params": params, "token": self.token});
        writeln!(self.out, "{line}").and_then(|()| self.out.flush()).map_err(|e| CallError::Connection(e.to_string()))?;
        loop {
            let mut buf = String::new();
            let n = (&mut self.reader).take(MAX_LINE as u64 + 1).read_line(&mut buf).map_err(|e| CallError::Connection(e.to_string()))?;
            if n == 0 {
                return Err(CallError::Connection("Trigon Studio closed the connection".into()));
            }
            let reply: Value = serde_json::from_str(&buf).map_err(|e| CallError::Connection(format!("bad reply: {e}")))?;
            // Replies come in order; a refusal (no id, or a null one) is ours too.
            match reply.get("id") {
                None | Some(Value::Null) => return Ok(reply),
                Some(v) if v == &json!(id) => return Ok(reply),
                Some(_) => continue,
            }
        }
    }

    /// Run one method; its result, or the app's error.
    pub fn call(&mut self, method: &str, params: Value) -> Result<Value, CallError> {
        let mut reply = self.roundtrip(method, params)?;
        if reply.get("running").and_then(Value::as_bool) == Some(true) {
            // Still running in the app: wait for its own reply.
            let start = Instant::now();
            reply = loop {
                std::thread::sleep(Duration::from_millis(250));
                let p = self.roundtrip("control.pending", json!({}))?;
                match p.pointer("/result/done").and_then(Value::as_bool) {
                    Some(true) => break p.pointer("/result/reply").cloned().unwrap_or(Value::Null),
                    Some(false) if start.elapsed() < PATIENCE => continue,
                    _ => return Err(CallError::App(format!("{method} is still running in Trigon Studio"))),
                }
            };
        }
        if reply.get("ok").and_then(Value::as_bool) == Some(true) {
            Ok(reply.get("result").cloned().unwrap_or(Value::Null))
        } else {
            Err(CallError::App(reply.get("error").and_then(Value::as_str).unwrap_or("Trigon Studio refused the request").to_string()))
        }
    }
}
