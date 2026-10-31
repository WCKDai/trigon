//! Trigon's control channel: a running Trigon Studio listens on loopback for JSON lines
//! (`{"id", "method", "params", "token"}` → `{"id", "ok", "result" | "error"}`), so an agent's
//! commands run in the owner's session, where the owner watches them land. See
//! docs/control-protocol.md.
//!
//! - [`auth`]: the per-launch token, its user-only file and the discovery file with the port
//! - [`server`]: the listener, with its limits; requests reach the app's UI thread through a queue
//! - [`client`]: what `trigon mcp --connect` uses to talk to the app
//! - [`methods`]: the methods that need only the session
#![deny(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::unimplemented, clippy::todo, clippy::unreachable)]

pub mod auth;
pub mod client;
pub mod methods;
pub mod server;

pub use client::{CallError, Client};
pub use server::{Options, Presence, Request, Responder, Server, Waker};
