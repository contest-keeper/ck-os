//! `ck-capture`: the ICAP service the stick's Squid hands requests and responses to (ADR-003).
//!
//! It never changes traffic: every reply is "unchanged". What it learns goes to a [`Rule`].
//! [`Observer`] writes the *shape* of the traffic (method, host, path, status, header names, form
//! field names) and never a value. [`Codeforces`] tells the Hub who logged in and what was
//! submitted, from public facts only. Either way a password, a cookie or a session id never leaves
//! the memory of this process.

mod codeforces;
mod fields;
mod form;
mod http;
mod icap;
mod network;
mod observe;
mod server;

pub use codeforces::Codeforces;
pub use http::{Head, parse_request_head, parse_response_head};
pub use network::follow as follow_network;
pub use observe::Observer;
pub use server::serve;

/// A request the proxy saw, reduced to what the rules need. The body is only its first part.
pub struct Request {
    pub head: Head,
    pub body: Vec<u8>,
    pub body_truncated: bool,
}

/// A response with the request it answers.
pub struct Response {
    pub status: u16,
    pub head: Head,
    pub body: Vec<u8>,
    pub body_truncated: bool,
}

/// What the Hub is told about. The payload never holds a password, a cookie or a session id.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EventKind {
    Login,
    Submission,
    /// A batch of what the proxy carried or refused.
    Network,
}

/// What a rule makes of one request or response.
#[derive(Debug, Clone, PartialEq)]
pub enum Output {
    /// A line for the observation log.
    Note(String),
    /// Something the Hub should hear about.
    Event {
        kind: EventKind,
        payload: serde_json::Value,
    },
}

/// Turns traffic into outputs.
pub trait Rule: Send {
    fn on_request(&mut self, request: &Request) -> Vec<Output>;
    fn on_response(&mut self, request: &Head, response: &Response) -> Vec<Output>;
}

/// Where events go. On the stick it is the queue the uplink sends from.
pub trait EventSink: Send + Sync {
    fn send(&self, kind: EventKind, payload: serde_json::Value) -> Result<(), String>;
}

/// For a machine that has nowhere to send events: they are dropped.
pub struct NoEvents;

impl EventSink for NoEvents {
    fn send(&self, _kind: EventKind, _payload: serde_json::Value) -> Result<(), String> {
        Ok(())
    }
}

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("{context}: {source}")]
    Io {
        context: &'static str,
        #[source]
        source: std::io::Error,
    },
    #[error("bad ICAP message: {0}")]
    Icap(String),
}
