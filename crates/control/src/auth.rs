//! The control channel's per-launch token and discovery file (docs/control-protocol.md).
//!
//! Trigon Studio makes a random 256-bit token at start, writes it to a file only the user can
//! read ([`token_path`]) and refuses every connection whose first request doesn't carry it, so
//! other users on the same machine (loopback is shared) can't drive the app. Clients find the
//! port in the discovery file ([`read_current_port`]) and the token with [`client_token`].
//!
//! Adapted from Polykrom's `crates/engine/src/control_auth.rs` (MIT OR Apache-2.0, same owner),
//! so both apps behave alike.

use std::io::{Error, ErrorKind, Result, Write};
use std::net::TcpListener;
use std::path::{Path, PathBuf};

/// Shortest token accepted from [`TOKEN_ENV`] (hex characters).
pub const MIN_TOKEN_LEN: usize = 32;

/// The port tried first when no port is given. Below the ephemeral ranges of Linux (32768+) and
/// Windows/macOS (49152+), and clear of Polykrom's 27878–27893, so both apps can run at once.
pub const DEFAULT_PORT: u16 = 27900;

/// Ports tried from [`DEFAULT_PORT`] upward when it is taken (a second window, another program).
pub const PORT_TRIES: u16 = 16;

/// A token chosen on purpose (test harnesses, sandboxes whose data folder the client can't see).
pub const TOKEN_ENV: &str = "TRIGON_CONTROL_TOKEN";
/// Moves the folder of the token and discovery files.
pub const DIR_ENV: &str = "TRIGON_CONTROL_DIR";
/// Listen on exactly this port.
pub const PORT_ENV: &str = "TRIGON_CONTROL_PORT";

/// Where the channel listens.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Listen {
    /// Exactly this port (`--control PORT`, `TRIGON_CONTROL_PORT`); no fallback.
    Exact(u16),
    /// [`DEFAULT_PORT`], or the next free one of [`PORT_TRIES`].
    Default,
}

/// Bind a loopback-only listener as `listen` says.
pub fn bind(listen: Listen) -> Result<TcpListener> {
    match listen {
        Listen::Exact(port) => TcpListener::bind(("127.0.0.1", port)),
        Listen::Default => bind_with_fallback(DEFAULT_PORT, PORT_TRIES),
    }
}

/// Bind a loopback-only listener on `first`, or on the next free port when it is taken, up to
/// `tries` ports in all.
pub fn bind_with_fallback(first: u16, tries: u16) -> Result<TcpListener> {
    let mut last = Error::new(ErrorKind::AddrInUse, "no port to try");
    for port in (first..=u16::MAX).take(usize::from(tries.max(1))) {
        match TcpListener::bind(("127.0.0.1", port)) {
            Ok(l) => return Ok(l),
            // In use; Windows reports a port another program holds exclusively as denied.
            Err(e) if matches!(e.kind(), ErrorKind::AddrInUse | ErrorKind::PermissionDenied) => last = e,
            Err(e) => return Err(e),
        }
    }
    Err(last)
}

fn env_path(name: &str) -> Option<PathBuf> {
    std::env::var_os(name).filter(|v| !v.is_empty()).map(PathBuf::from)
}

/// The user's data folder for Trigon.
fn data_dir() -> Option<PathBuf> {
    if cfg!(target_os = "macos") {
        Some(env_path("HOME")?.join("Library/Application Support/Trigon"))
    } else if cfg!(windows) {
        Some(env_path("LOCALAPPDATA")?.join("Trigon"))
    } else {
        Some(env_path("XDG_DATA_HOME").or_else(|| Some(env_path("HOME")?.join(".local/share")))?.join("trigon"))
    }
}

/// The folder of the token and discovery files: `<data folder>/control` ([`DIR_ENV`] replaces it).
pub fn control_dir() -> Option<PathBuf> {
    env_path(DIR_ENV).or_else(|| Some(data_dir()?.join("control")))
}

/// The token file of the app listening on `port`: `<control folder>/<port>.token`.
pub fn token_path(port: u16) -> Option<PathBuf> {
    Some(control_dir()?.join(format!("{port}.token")))
}

/// The discovery file: the port of the most recently started app. It holds only the port.
pub fn current_port_path() -> Option<PathBuf> {
    Some(control_dir()?.join("current.port"))
}

/// Record `port` as the running app's, readable only by the user like the token files.
pub fn write_current_port(port: u16) -> Result<PathBuf> {
    let path = current_port_path().ok_or_else(|| Error::new(ErrorKind::NotFound, "no data folder"))?;
    write_private_file(&path, &port.to_string())?;
    Ok(path)
}

/// The port the discovery file names, if there is one.
pub fn read_current_port() -> Option<u16> {
    parse_port(&std::fs::read_to_string(current_port_path()?).ok().filter(|t| t.len() <= 64)?)
}

fn parse_port(text: &str) -> Option<u16> {
    text.trim().parse::<u16>().ok().filter(|p| *p != 0)
}

/// Remove the discovery file at exit, unless a later app has taken it over (it names another port).
pub fn remove_current_port(port: u16) {
    if read_current_port() == Some(port)
        && let Some(path) = current_port_path()
    {
        let _ = std::fs::remove_file(path);
    }
}

/// A token set on purpose with [`TOKEN_ENV`], if it is long enough.
pub fn env_token() -> Option<String> {
    std::env::var(TOKEN_ENV).ok().map(|t| t.trim().to_string()).filter(|t| t.len() >= MIN_TOKEN_LEN)
}

/// A new random token: 32 bytes from the operating system, as hex.
pub fn new_token() -> Option<String> {
    let mut bytes = [0u8; 32];
    getrandom::fill(&mut bytes).ok()?;
    Some(bytes.iter().map(|b| format!("{b:02x}")).collect())
}

/// The token a client sends to the app on `port`: `explicit` (`--token`), else [`TOKEN_ENV`],
/// else the app's token file.
pub fn client_token(port: u16, explicit: Option<&str>) -> Option<String> {
    if let Some(t) = explicit.map(str::trim).filter(|t| !t.is_empty()) {
        return Some(t.to_string());
    }
    if let Some(t) = env_token() {
        return Some(t);
    }
    let text = std::fs::read_to_string(token_path(port)?).ok().filter(|t| t.len() <= 4096)?;
    Some(text.trim().to_string()).filter(|t| !t.is_empty())
}

/// `a == b`, taking the same time wherever they differ.
pub fn tokens_match(a: &str, b: &str) -> bool {
    let (a, b) = (a.as_bytes(), b.as_bytes());
    a.len() == b.len() && a.iter().zip(b).fold(0u8, |acc, (x, y)| acc | (x ^ y)) == 0
}

/// Write `text` to `path`, readable and writable only by the user: the folder is made `0700` and
/// the file `0600` on Unix; on Windows the file's inherited permissions are replaced by one entry
/// for the user (`icacls`) before the text goes in. A file left by an earlier launch is replaced.
pub fn write_private_file(path: &Path, text: &str) -> Result<()> {
    let dir = path.parent().ok_or_else(|| Error::new(ErrorKind::InvalidInput, "no folder"))?;
    std::fs::create_dir_all(dir)?;
    let _ = std::fs::remove_file(path);
    let mut opts = std::fs::OpenOptions::new();
    opts.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};
        std::fs::set_permissions(dir, std::fs::Permissions::from_mode(0o700))?;
        opts.mode(0o600);
    }
    let mut f = opts.open(path)?;
    #[cfg(windows)]
    restrict_to_user(path)?;
    f.write_all(text.as_bytes())?;
    f.write_all(b"\n")?;
    f.sync_all()
}

/// Windows: drop inherited entries and grant only the current user full access.
#[cfg(windows)]
fn restrict_to_user(path: &Path) -> Result<()> {
    use std::os::windows::process::CommandExt;
    let user = std::env::var("USERNAME").map_err(|_| Error::other("USERNAME is not set"))?;
    let domain = std::env::var("USERDOMAIN").ok().filter(|d| !d.is_empty());
    let who = domain.map_or(user.clone(), |d| format!("{d}\\{user}"));
    let status = std::process::Command::new("icacls")
        .arg(path)
        .args(["/inheritance:r", "/grant:r", &format!("{who}:F")])
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .creation_flags(0x0800_0000)
        .status()?;
    if status.success() { Ok(()) } else { Err(Error::other(format!("icacls failed to restrict {}", path.display()))) }
}

/// The files a launch wrote (token, discovery), removed when it ends.
pub struct LaunchFiles {
    port: u16,
    token: Option<PathBuf>,
}

impl LaunchFiles {
    /// Write the token file and the discovery file for `port`. Errors are reported and skipped:
    /// a client can still be given the token with [`TOKEN_ENV`].
    pub fn write(port: u16, token: &str) -> (Self, Vec<String>) {
        let mut notes = Vec::new();
        let token_file = match token_path(port) {
            Some(path) => match write_private_file(&path, token) {
                Ok(()) => {
                    notes.push(format!("token in {}", path.display()));
                    Some(path)
                }
                Err(e) => {
                    notes.push(format!("the token file {} could not be written ({e}); clients need {TOKEN_ENV}", path.display()));
                    None
                }
            },
            None => {
                notes.push(format!("no data folder for the token; clients need {TOKEN_ENV}"));
                None
            }
        };
        if let Err(e) = write_current_port(port) {
            notes.push(format!("the discovery file could not be written ({e}); clients need --connect 127.0.0.1:{port}"));
        }
        (Self { port, token: token_file }, notes)
    }
}

impl Drop for LaunchFiles {
    fn drop(&mut self) {
        if let Some(p) = &self.token {
            let _ = std::fs::remove_file(p);
        }
        remove_current_port(self.port);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tokens_compare_whole() {
        assert!(tokens_match("abc123", "abc123"));
        assert!(!tokens_match("abc123", "abc124"));
        assert!(!tokens_match("abc123", "abc12"));
        assert!(!tokens_match("", "a"));
        assert!(tokens_match("", ""));
    }

    #[test]
    fn new_tokens_are_256_bit_hex_and_differ() {
        let (a, b) = (new_token().unwrap(), new_token().unwrap());
        assert_eq!(a.len(), 64);
        assert!(a.bytes().all(|c| c.is_ascii_hexdigit()));
        assert_ne!(a, b);
    }

    /// A taken port falls through to the next free one, on loopback only; a run of taken ports
    /// is an error, not a hang.
    #[test]
    fn a_taken_port_falls_back_to_the_next_free_one() {
        let probe = TcpListener::bind("127.0.0.1:0").unwrap();
        let start = probe.local_addr().unwrap().port().clamp(1024, u16::MAX - 64);
        drop(probe);
        let first = bind_with_fallback(start, 32).unwrap();
        let a = first.local_addr().unwrap();
        assert!(a.ip().is_loopback(), "{a}");
        let second = bind_with_fallback(a.port(), 32).unwrap();
        assert!(second.local_addr().unwrap().port() > a.port());
        assert!(bind_with_fallback(a.port(), 1).is_err());
    }

    #[test]
    fn the_discovery_file_holds_a_port() {
        assert_eq!(parse_port("27901\n"), Some(27901));
        assert_eq!(parse_port("0"), None);
        assert_eq!(parse_port("70000"), None);
        assert_eq!(parse_port("nope"), None);
        assert_eq!(parse_port(""), None);
    }

    /// The token file is the user's alone, a stale file is replaced, and it can be read back.
    #[cfg(unix)]
    #[test]
    fn the_token_file_is_private_to_the_user() {
        use std::os::unix::fs::PermissionsExt;
        let dir = std::env::temp_dir().join(format!("trigon-control-auth-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let path = dir.join("control/27900.token");
        std::fs::create_dir_all(dir.join("control")).unwrap();
        std::fs::write(&path, "stale").unwrap();
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o644)).unwrap();
        write_private_file(&path, "0123456789abcdef0123456789abcdef").unwrap();
        let mode = |p: &Path| std::fs::metadata(p).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode(&path), 0o600);
        assert_eq!(mode(path.parent().unwrap()), 0o700);
        assert_eq!(std::fs::read_to_string(&path).unwrap().trim(), "0123456789abcdef0123456789abcdef");
        let _ = std::fs::remove_dir_all(&dir);
    }
}
