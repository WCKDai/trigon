//! The loopback JSON-lines server: one request per line, one reply per line
//! (docs/control-protocol.md). Each connection has a thread that reads its lines, checks the
//! token and hands requests to the app through a bounded queue; the app answers them on its UI
//! thread, through its one session, and the connection's thread writes the reply.
//!
//! Adapted from Polykrom's `apps/solidcraft/src/control_server.rs` (MIT OR Apache-2.0, same
//! owner): the same token handshake, limits, timeouts and `control.pending`.

use std::collections::BTreeMap;
use std::io::{BufRead, BufReader, Read, Write};
use std::net::{Shutdown, TcpListener, TcpStream};
use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
use std::sync::mpsc::{Receiver, RecvTimeoutError, Sender, SyncSender, TryRecvError, TrySendError, channel, sync_channel};
use std::sync::{Arc, Mutex, PoisonError};
use std::time::{Duration, Instant};

use serde_json::{Value, json};

use crate::auth::tokens_match;

/// Longest request line, bytes; a longer one is refused and the connection closed.
pub const MAX_LINE: usize = 16 * 1024 * 1024;
/// Lines that are not JSON before the connection is closed, so garbage cannot loop forever.
pub const MAX_BAD_LINES: usize = 8;
/// Connections served at once, so a flood cannot spawn threads without end.
pub const MAX_CONNECTIONS: usize = 16;
/// Independent bounded refusal drains; silent refused sockets cannot stop new accepts.
const MAX_REFUSAL_DRAINS: usize = 4;
/// Requests waiting for the app at once.
pub const MAX_QUEUE: usize = 64;
/// How long a request waits for the app's reply before it is answered `running`.
pub const REPLY_WAIT: Duration = Duration::from_secs(60);
/// Longest client name kept from `control.hello`.
const MAX_CLIENT_NAME: usize = 64;

/// Limits, adjustable for tests.
#[derive(Clone, Copy, Debug)]
pub struct Options {
    pub max_line: usize,
    pub max_connections: usize,
    pub reply_wait: Duration,
}

impl Default for Options {
    fn default() -> Self {
        Options { max_line: MAX_LINE, max_connections: MAX_CONNECTIONS, reply_wait: REPLY_WAIT }
    }
}

/// One request for the app. Answer it with [`Request::respond`]; dropping it unanswered tells the
/// client the app ended it without a reply.
pub struct Request {
    /// The connection it came on (stable for the connection's life).
    pub connection: u64,
    pub method: String,
    pub params: Value,
    reply: Responder,
}

impl Request {
    /// Split off the responder, to answer in a later frame (a window screenshot).
    pub fn into_parts(self) -> (u64, String, Value, Responder) {
        (self.connection, self.method, self.params, self.reply)
    }

    pub fn respond(self, result: Result<Value, String>) {
        self.reply.send(result);
    }
}

/// Where a request's reply goes.
pub struct Responder(Sender<Value>);

impl Responder {
    pub fn send(self, result: Result<Value, String>) {
        let v = match result {
            Ok(v) => json!({"ok": true, "result": v}),
            Err(e) => json!({"ok": false, "error": e}),
        };
        let _ = self.0.send(v);
    }
}

/// Who is connected and what they are doing, for the app's agent indicator.
#[derive(Default)]
pub struct Presence {
    /// Connections that showed the token and are still open.
    agents: AtomicUsize,
    /// Requests sent to the app and not answered yet.
    in_flight: AtomicUsize,
    /// Milliseconds after `started` of the latest request, plus one (0: none yet).
    last_request: AtomicU64,
    started: Mutex<Option<Instant>>,
    clients: Mutex<BTreeMap<u64, String>>,
}

impl Presence {
    fn epoch(&self) -> Instant {
        *self.started.lock().unwrap_or_else(PoisonError::into_inner).get_or_insert_with(Instant::now)
    }

    /// Authenticated connections open now.
    pub fn agents(&self) -> usize {
        self.agents.load(Ordering::Relaxed)
    }

    /// True while a request waits for the app.
    pub fn busy(&self) -> bool {
        self.in_flight.load(Ordering::Relaxed) > 0
    }

    /// Time since the latest request arrived.
    pub fn since_last_request(&self) -> Option<Duration> {
        let at = self.last_request.load(Ordering::Relaxed).checked_sub(1)?;
        Some(self.epoch().elapsed().saturating_sub(Duration::from_millis(at)))
    }

    /// The names clients gave in `control.hello`, by connection.
    pub fn clients(&self) -> BTreeMap<u64, String> {
        self.clients.lock().unwrap_or_else(PoisonError::into_inner).clone()
    }

    fn touch(&self) {
        let ms = u64::try_from(self.epoch().elapsed().as_millis()).unwrap_or(u64::MAX - 1);
        self.last_request.store(ms + 1, Ordering::Relaxed);
    }
}

/// Counts a request as in flight until dropped.
struct InFlight(Arc<Presence>);

impl InFlight {
    fn new(p: &Arc<Presence>) -> Self {
        p.in_flight.fetch_add(1, Ordering::Relaxed);
        p.touch();
        InFlight(p.clone())
    }
}

impl Drop for InFlight {
    fn drop(&mut self) {
        self.0.in_flight.fetch_sub(1, Ordering::Relaxed);
    }
}

/// Wakes the app's UI thread when a request arrives (`egui::Context::request_repaint`).
pub type Waker = Arc<dyn Fn() + Send + Sync>;

struct Shared {
    token: String,
    tx: SyncSender<Request>,
    waker: Waker,
    presence: Arc<Presence>,
    open: AtomicUsize,
    refusals: AtomicUsize,
    connections: AtomicU64,
    options: Options,
}

/// A running control server. Requests arrive on [`Server::try_recv`].
pub struct Server {
    rx: Receiver<Request>,
    port: u16,
    presence: Arc<Presence>,
}

impl Server {
    /// Serve `listener`, which must be bound to loopback, with `token`.
    pub fn start(listener: TcpListener, token: String, waker: Waker) -> std::io::Result<Server> {
        Self::start_with(listener, token, waker, Options::default())
    }

    pub fn start_with(listener: TcpListener, token: String, waker: Waker, options: Options) -> std::io::Result<Server> {
        let addr = listener.local_addr()?;
        check_loopback(addr)?;
        if token.len() < crate::auth::MIN_TOKEN_LEN {
            return Err(std::io::Error::new(std::io::ErrorKind::InvalidInput, "the control token is too short"));
        }
        let (tx, rx) = sync_channel(MAX_QUEUE);
        let presence = Arc::new(Presence::default());
        presence.epoch();
        let shared = Arc::new(Shared {
            token,
            tx,
            waker,
            presence: presence.clone(),
            open: AtomicUsize::new(0),
            refusals: AtomicUsize::new(0),
            connections: AtomicU64::new(0),
            options,
        });
        std::thread::Builder::new().name("trigon-control".into()).spawn(move || accept(listener, shared))?;
        Ok(Server { rx, port: addr.port(), presence })
    }

    pub fn port(&self) -> u16 {
        self.port
    }

    /// Who is connected, shared with the connections' threads.
    pub fn presence(&self) -> Arc<Presence> {
        self.presence.clone()
    }

    /// The next request, if one waits.
    pub fn try_recv(&self) -> Option<Request> {
        self.rx.try_recv().ok()
    }

    /// The next request, waiting up to `wait` for one.
    pub fn recv_timeout(&self, wait: Duration) -> Option<Request> {
        self.rx.recv_timeout(wait).ok()
    }
}

fn check_loopback(addr: std::net::SocketAddr) -> std::io::Result<()> {
    if addr.ip().is_loopback() {
        Ok(())
    } else {
        Err(std::io::Error::new(std::io::ErrorKind::InvalidInput, format!("the control channel listens on loopback only, not {addr}")))
    }
}

fn accept(listener: TcpListener, shared: Arc<Shared>) {
    for stream in listener.incoming().flatten() {
        if shared.open.fetch_add(1, Ordering::Relaxed) >= shared.options.max_connections {
            shared.open.fetch_sub(1, Ordering::Relaxed);
            if let Some(permit) = RefusalPermit::acquire(&shared) {
                // Dropping the closure on a spawn failure also releases the permit.
                let _ = std::thread::Builder::new().name("trigon-control-refused".into()).spawn(move || {
                    let _permit = permit;
                    drain_refused(stream);
                });
            }
            // No drain permit: close the excess socket; never spawn an unbounded worker.
            continue;
        }
        let shared2 = shared.clone();
        let spawned = std::thread::Builder::new().name("trigon-control-conn".into()).spawn(move || {
            serve(stream, &shared2);
            shared2.open.fetch_sub(1, Ordering::Relaxed);
        });
        if spawned.is_err() {
            shared.open.fetch_sub(1, Ordering::Relaxed);
        }
    }
}

struct RefusalPermit(Arc<Shared>);
impl RefusalPermit {
    fn acquire(shared: &Arc<Shared>) -> Option<Self> {
        let mut active = shared.refusals.load(Ordering::Relaxed);
        loop {
            if active >= MAX_REFUSAL_DRAINS {
                return None;
            }
            match shared.refusals.compare_exchange_weak(active, active + 1, Ordering::Relaxed, Ordering::Relaxed) {
                Ok(_) => return Some(Self(shared.clone())),
                Err(next) => active = next,
            }
        }
    }
}
impl Drop for RefusalPermit {
    fn drop(&mut self) {
        self.0.refusals.fetch_sub(1, Ordering::Relaxed);
    }
}

fn drain_refused(mut refused: TcpStream) {
    let _ = refused.set_write_timeout(Some(Duration::from_millis(100)));
    let _ = writeln!(refused, "{}", json!({"ok":false,"error":"too many connections"}));
    let _ = refused.shutdown(Shutdown::Write);
    let deadline = Instant::now() + Duration::from_millis(500);
    let mut left = 64 * 1024;
    let mut buffer = [0_u8; 4096];
    while left > 0 {
        let wait = deadline.saturating_duration_since(Instant::now());
        if wait.is_zero() || refused.set_read_timeout(Some(wait)).is_err() {
            break;
        }
        let limit = left.min(buffer.len());
        match refused.read(&mut buffer[..limit]) {
            Ok(0) | Err(_) => break,
            Ok(n) => left -= n,
        }
    }
}

/// One line without its newline, or `None` at the end of the stream. A client that sends
/// gigabytes with no newline would otherwise make this thread allocate until the process is
/// killed, so the line is refused as soon as it passes `max`.
fn read_line(r: &mut impl BufRead, max: usize) -> std::io::Result<Option<Vec<u8>>> {
    let mut line = Vec::new();
    loop {
        let (done, used) = {
            let available = r.fill_buf()?;
            if available.is_empty() {
                return Ok((!line.is_empty()).then_some(line));
            }
            match available.iter().position(|b| *b == b'\n') {
                Some(i) => {
                    line.extend_from_slice(available.get(..i).unwrap_or(&[]));
                    (true, i + 1)
                }
                None => {
                    line.extend_from_slice(available);
                    (false, available.len())
                }
            }
        };
        r.consume(used);
        if line.len() > max {
            return Err(std::io::Error::new(std::io::ErrorKind::InvalidData, "request too large"));
        }
        if done {
            return Ok(Some(line));
        }
    }
}

/// Refuse the connection: the reply, then the end of what we send, then what the client had
/// already sent read and dropped (a little, briefly). Closing a socket with unread input resets
/// the connection, and a reset can reach the client before it has read the refusal.
fn refuse(out: &mut TcpStream, reader: &mut BufReader<TcpStream>) {
    let _ = writeln!(out, "{}", json!({"ok": false, "error": "unauthorized: send the control token as \"token\" (see docs/control-protocol.md)"}));
    let _ = out.shutdown(Shutdown::Write);
    let _ = reader.get_ref().set_read_timeout(Some(Duration::from_millis(500)));
    let _ = std::io::copy(&mut reader.by_ref().take(64 * 1024), &mut std::io::sink());
}

/// A request that timed out but still runs in the app: its id, method and where its reply lands.
struct Pending {
    id: Value,
    method: String,
    reply: Receiver<Value>,
    _in_flight: InFlight,
}

/// A client name made safe to show: printable, short.
fn client_name(v: Option<&Value>) -> String {
    let raw = v.and_then(Value::as_str).unwrap_or("agent");
    let name: String = raw.chars().filter(|c| !c.is_control()).take(MAX_CLIENT_NAME).collect();
    if name.trim().is_empty() { "agent".into() } else { name }
}

fn serve(stream: TcpStream, shared: &Shared) {
    let connection = shared.connections.fetch_add(1, Ordering::Relaxed) + 1;
    let Ok(read) = stream.try_clone() else { return };
    let mut out = stream;
    let mut reader = BufReader::new(read);
    let mut bad = 0usize;
    let mut authenticated = false;
    let mut pending: Option<Pending> = None;
    let presence = &shared.presence;
    loop {
        let line = match read_line(&mut reader, shared.options.max_line) {
            Ok(Some(l)) => l,
            Ok(None) => break,
            Err(e) => {
                let _ = writeln!(out, "{}", json!({"ok": false, "error": e.to_string()}));
                break;
            }
        };
        let text = String::from_utf8_lossy(&line);
        if text.trim().is_empty() {
            continue;
        }
        let msg = match serde_json::from_str::<Value>(&text) {
            Ok(msg) => msg,
            Err(_) if !authenticated => {
                refuse(&mut out, &mut reader);
                break;
            }
            Err(e) => {
                bad += 1;
                if writeln!(out, "{}", json!({"ok": false, "id": null, "error": format!("bad JSON: {e}")})).is_err() || bad >= MAX_BAD_LINES {
                    break;
                }
                continue;
            }
        };
        // The first request must carry the token; later ones on the same connection may.
        if !authenticated {
            match msg.get("token").and_then(Value::as_str) {
                Some(t) if tokens_match(t, &shared.token) => {
                    authenticated = true;
                    presence.agents.fetch_add(1, Ordering::Relaxed);
                    presence.touch();
                    (shared.waker)();
                }
                _ => {
                    refuse(&mut out, &mut reader);
                    break;
                }
            }
        }
        let id = msg.get("id").cloned().unwrap_or(Value::Null);
        let method = msg.get("method").and_then(Value::as_str).unwrap_or_default().to_string();
        let params = msg.get("params").cloned().unwrap_or_else(|| json!({}));
        // A request that timed out may have finished since.
        let late = pending.as_ref().and_then(|p| match p.reply.try_recv() {
            Ok(reply) => Some(reply),
            Err(TryRecvError::Disconnected) => Some(json!({"ok": false, "error": "the app ended the request without a reply"})),
            Err(TryRecvError::Empty) => None,
        });
        let mut reply = if method == "control.hello" {
            let name = client_name(params.get("client"));
            presence.clients.lock().unwrap_or_else(PoisonError::into_inner).insert(connection, name.clone());
            (shared.waker)();
            json!({"ok": true, "result": {"app": "trigon-studio", "version": env!("CARGO_PKG_VERSION"), "client": name, "connection": connection, "methods": crate::methods::METHODS.iter().map(|(m, _)| *m).collect::<Vec<_>>()}})
        } else if method == "control.pending" {
            match (pending.take(), late) {
                (None, _) => json!({"ok": true, "result": {"pending": null}}),
                (Some(p), Some(reply)) => json!({"ok": true, "result": {"pending": p.id, "method": p.method, "done": true, "reply": reply}}),
                (Some(p), None) => {
                    let r = json!({"ok": true, "result": {"pending": p.id, "method": p.method, "done": false}});
                    pending = Some(p);
                    r
                }
            }
        } else if let Some(p) = pending.as_ref().filter(|_| late.is_none()) {
            // Nothing more is queued behind it, to run unseen when it ends.
            let method: String = method.chars().take(128).collect();
            json!({"ok": false, "busy": true, "error": format!("busy: request {} ({}) is still running, so `{method}` was not run; ask `control.pending` for its result, then send this one again", p.id, p.method)})
        } else {
            if late.is_some() {
                // Finished, and the client moved on without asking: it is dropped.
                pending = None;
            }
            let (rtx, rrx) = channel();
            let in_flight = InFlight::new(presence);
            let req = Request { connection, method: method.clone(), params, reply: Responder(rtx) };
            match shared.tx.try_send(req) {
                Ok(()) => {
                    (shared.waker)();
                    match rrx.recv_timeout(shared.options.reply_wait) {
                        Ok(r) => r,
                        Err(RecvTimeoutError::Timeout) => {
                            let what: String = method.chars().take(128).collect();
                            let r = json!({"ok": false, "running": true, "error": format!("timeout: {what} is still running in the app (it was not cancelled: don't send it again); ask `control.pending` for its result")});
                            pending = Some(Pending { id: id.clone(), method, reply: rrx, _in_flight: in_flight });
                            r
                        }
                        Err(RecvTimeoutError::Disconnected) => json!({"ok": false, "error": "the app ended the request without a reply"}),
                    }
                }
                Err(TrySendError::Full(_)) => {
                    json!({"ok": false, "busy": true, "error": "busy: too many requests wait for the app; send it again shortly"})
                }
                Err(TrySendError::Disconnected(_)) => break,
            }
        };
        if let Some(o) = reply.as_object_mut() {
            o.insert("id".into(), id);
        }
        if writeln!(out, "{reply}").is_err() {
            break;
        }
    }
    if authenticated {
        presence.agents.fetch_sub(1, Ordering::Relaxed);
        presence.clients.lock().unwrap_or_else(PoisonError::into_inner).remove(&connection);
        (shared.waker)();
    }
}

#[cfg(test)]
mod tests;
