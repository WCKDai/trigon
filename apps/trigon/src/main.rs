//! `trigon`: run commands, scripts and the MCP server.
#![deny(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::unimplemented, clippy::todo, clippy::unreachable)]

use std::path::PathBuf;
use std::process::ExitCode;

use serde_json::{Value, json};
use trigon_data::{Credentials, Live};
use trigon_engine::{Session, commands};

const USAGE: &str = "\
trigon — AI-driven building modelling

USAGE
  trigon commands                         list the commands
  trigon schema <command>                 the JSON schema of a command's parameters
  trigon run [--doc FILE] <command> [JSON]  run one command; with --doc the document is opened
                                          first and saved after a change
  trigon script <FILE.json> [--doc FILE]  run a list of {\"command\", \"params\"} in one session
  trigon mcp                              serve MCP over stdio, bridged to the running Trigon Studio
                                          (found through its discovery file), else a session of its own
  trigon mcp --connect HOST:PORT [--token T]  bridged to the Studio listening there
  trigon mcp --doc FILE | --headless      a session of its own (on FILE)

CREDENTIALS (environment)
  TRIGON_DATAFORSYNINGEN_TOKEN[_FILE]     addresses, elevation model, Skråfoto
  TRIGON_DATAFORDELER_KEY[_FILE]          GeoDanmark and BBR
  TRIGON_CONTROL_TOKEN                    the Studio's control token (else read from its token file)";

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    match run(&args) {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("trigon: {e}");
            ExitCode::FAILURE
        }
    }
}

fn session(doc: Option<&PathBuf>) -> Result<Session, String> {
    let mut s = Session::new(Box::new(Live::new(Credentials::from_env())));
    if let Some(p) = doc.filter(|p| p.exists()) {
        s.execute("document.open", json!({"path": p})).map_err(|e| e.to_string())?;
    }
    Ok(s)
}

/// Pulls `--doc FILE` out of the arguments.
fn take_doc(args: &mut Vec<String>) -> Result<Option<PathBuf>, String> {
    match args.iter().position(|a| a == "--doc") {
        Some(i) => {
            args.remove(i);
            if i < args.len() { Ok(Some(PathBuf::from(args.remove(i)))) } else { Err("--doc needs a file".into()) }
        }
        None => Ok(None),
    }
}

/// Pulls `--flag VALUE` out of the arguments.
fn take_flag(args: &mut Vec<String>, flag: &str) -> Result<Option<String>, String> {
    match args.iter().position(|a| a == flag) {
        Some(i) => {
            args.remove(i);
            if i < args.len() { Ok(Some(args.remove(i))) } else { Err(format!("{flag} needs a value")) }
        }
        None => Ok(None),
    }
}

/// The port of the running Trigon Studio: the one its discovery file names, if something listens
/// there (a file left by an app that crashed names a closed port).
fn discover_studio() -> Option<u16> {
    let port = trigon_control::auth::read_current_port()?;
    let addr = std::net::SocketAddr::from(([127, 0, 0, 1], port));
    std::net::TcpStream::connect_timeout(&addr, std::time::Duration::from_millis(500)).ok().map(|_| port)
}

/// `trigon mcp`: bridged to Trigon Studio (`--connect`, or the one running), else headless.
fn mcp(mut args: Vec<String>, doc: Option<&PathBuf>) -> Result<(), String> {
    let connect = take_flag(&mut args, "--connect")?;
    let token = take_flag(&mut args, "--token")?;
    let headless = args.iter().any(|a| a == "--headless");
    let stdin = std::io::stdin();
    let serve_remote = |remote: &mut trigon_mcp::Remote| trigon_mcp::serve(remote, stdin.lock(), std::io::stdout()).map_err(|e| e.to_string());
    match (connect, doc) {
        (Some(_), Some(_)) => Err("mcp: use either --doc (a session of its own) or --connect, not both".into()),
        (Some(addr), None) => {
            if addr.rsplit_once(':').and_then(|(_, p)| p.parse::<u16>().ok()).is_none_or(|p| p == 0) {
                return Err(format!("mcp: --connect wants HOST:PORT, such as 127.0.0.1:{}, not `{addr}`", trigon_control::auth::DEFAULT_PORT));
            }
            let mut remote = trigon_mcp::Remote::new(&addr, token.as_deref());
            match remote.try_connect() {
                Ok(()) => eprintln!("trigon mcp: bridged to Trigon Studio at {addr}"),
                Err(e) => eprintln!("trigon mcp: {e}; tool calls connect once Trigon Studio runs"),
            }
            serve_remote(&mut remote)
        }
        (None, _) => {
            if doc.is_none()
                && !headless
                && let Some(port) = discover_studio()
            {
                let mut remote = trigon_mcp::Remote::new(&format!("127.0.0.1:{port}"), token.as_deref());
                match remote.try_connect() {
                    Ok(()) => {
                        eprintln!("trigon mcp: bridged to the running Trigon Studio at 127.0.0.1:{port} (--headless starts a session of its own)");
                        return serve_remote(&mut remote);
                    }
                    Err(e) => {
                        eprintln!("trigon mcp: found Trigon Studio at 127.0.0.1:{port} but could not connect ({e}); running a session of its own")
                    }
                }
            }
            let mut s = session(doc)?;
            if let Some(p) = doc {
                s.path = Some(p.clone());
            }
            trigon_mcp::serve(&mut s, stdin.lock(), std::io::stdout()).map_err(|e| e.to_string())
        }
    }
}

fn save_if_changed(s: &mut Session, doc: Option<&PathBuf>) -> Result<(), String> {
    if let (true, Some(p)) = (s.dirty, doc) {
        s.execute("document.save", json!({"path": p})).map_err(|e| e.to_string())?;
    }
    Ok(())
}

fn run(args: &[String]) -> Result<(), String> {
    let mut args = args.to_vec();
    let doc = take_doc(&mut args)?;
    let Some(cmd) = args.first().cloned() else {
        println!("{USAGE}");
        return Ok(());
    };
    match cmd.as_str() {
        "commands" => {
            for c in commands() {
                println!("{:<28} {}", c.id, c.summary);
            }
            Ok(())
        }
        "schema" => {
            let id = args.get(1).ok_or("schema needs a command id")?;
            let c = commands().iter().find(|c| c.id == id).ok_or_else(|| format!("unknown command {id}"))?;
            println!("{}", serde_json::to_string_pretty(&c.params_schema()).unwrap_or_default());
            Ok(())
        }
        "run" => {
            let id = args.get(1).ok_or("run needs a command id")?;
            let params: Value = match args.get(2) {
                Some(j) => serde_json::from_str(j).map_err(|e| format!("parameters are not JSON: {e}"))?,
                None => Value::Null,
            };
            let mut s = session(doc.as_ref())?;
            let out = s.execute(id, params).map_err(|e| e.to_string())?;
            println!("{}", serde_json::to_string_pretty(&out).unwrap_or_default());
            save_if_changed(&mut s, doc.as_ref())
        }
        "script" => {
            let file = args.get(1).ok_or("script needs a file")?;
            let text = std::fs::read_to_string(file).map_err(|e| format!("{file}: {e}"))?;
            let steps: Vec<Value> = serde_json::from_str(&text).map_err(|e| format!("{file}: {e}"))?;
            let mut s = session(doc.as_ref())?;
            for (i, step) in steps.iter().enumerate() {
                let id = step.get("command").and_then(Value::as_str).ok_or_else(|| format!("step {i}: no command"))?;
                let params = step.get("params").cloned().unwrap_or(Value::Null);
                let out = s.execute(id, params).map_err(|e| format!("step {i} ({id}): {e}"))?;
                println!("# {id}\n{}", serde_json::to_string_pretty(&out).unwrap_or_default());
            }
            save_if_changed(&mut s, doc.as_ref())
        }
        "mcp" => mcp(args[1..].to_vec(), doc.as_ref()),
        "help" | "--help" | "-h" => {
            println!("{USAGE}");
            Ok(())
        }
        other => Err(format!("unknown subcommand {other}\n\n{USAGE}")),
    }
}
