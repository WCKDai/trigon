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
  trigon mcp [--doc FILE]                 serve MCP over stdio

CREDENTIALS (environment)
  TRIGON_DATAFORSYNINGEN_TOKEN[_FILE]     addresses, elevation model, Skråfoto
  TRIGON_DATAFORDELER_KEY[_FILE]          GeoDanmark and BBR";

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
        "mcp" => {
            let mut s = session(doc.as_ref())?;
            if let Some(p) = &doc {
                s.path = Some(p.clone());
            }
            let stdin = std::io::stdin();
            trigon_mcp::serve(&mut s, stdin.lock(), std::io::stdout()).map_err(|e| e.to_string())
        }
        "help" | "--help" | "-h" => {
            println!("{USAGE}");
            Ok(())
        }
        other => Err(format!("unknown subcommand {other}\n\n{USAGE}")),
    }
}
