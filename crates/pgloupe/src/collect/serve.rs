//! The local HTTP server behind `watch`: the current report, its JSON, and a status document.

use std::{
    net::{IpAddr, SocketAddr, ToSocketAddrs as _},
    sync::{Arc, RwLock},
    thread,
};

use serde::Serialize;
use tiny_http::{Header, Method, Request, Response, Server};

/// What the server hands out; replaced whole each time a report is built.
#[derive(Debug, Clone, Default)]
pub struct Published {
    /// Counts reports built; `0` until the first one exists.
    pub generation: u64,
    /// When the current report was built (RFC 3339).
    pub updated_at: Option<String>,
    /// When the next update is due.
    pub next_at: Option<String>,
    /// The dashboard.
    pub html: Arc<String>,
    /// The report as JSON.
    pub json: Arc<String>,
    /// What went wrong in the last cycle, if anything.
    pub last_error: Option<String>,
    /// Seconds between updates.
    pub interval_secs: u64,
}

/// The status document at `/status`.
#[derive(Debug, Serialize)]
struct Status<'a> {
    generation: u64,
    updated_at: Option<&'a str>,
    next_at: Option<&'a str>,
    last_error: Option<&'a str>,
    interval_secs: u64,
}

/// Shared between the loop that builds reports and the server that serves them.
pub type Shared = Arc<RwLock<Published>>;

/// Why an address cannot be served.
#[derive(Debug, thiserror::Error)]
pub enum ListenError {
    /// The address could not be parsed or resolved.
    #[error("--listen {0:?}: not an address like 127.0.0.1:8080")]
    Address(String),
    /// A non-loopback address without `--allow-remote`.
    #[error(
        "--listen {0}: that is reachable from other machines, and the page holds statement text; pass --allow-remote if that is what you want"
    )]
    Remote(SocketAddr),
    /// The port is taken or not allowed.
    #[error("could not listen on {0}: {1}")]
    Bind(SocketAddr, String),
}

/// Resolves `text` and refuses non-loopback addresses unless `allow_remote`.
///
/// # Errors
///
/// Fails if the address is not valid, or is remote without permission.
pub fn parse_listen(text: &str, allow_remote: bool) -> Result<SocketAddr, ListenError> {
    let addr = text
        .to_socket_addrs()
        .ok()
        .and_then(|mut a| a.next())
        .ok_or_else(|| ListenError::Address(text.to_owned()))?;
    let loopback = match addr.ip() {
        IpAddr::V4(ip) => ip.is_loopback(),
        IpAddr::V6(ip) => ip.is_loopback(),
    };
    if !loopback && !allow_remote {
        return Err(ListenError::Remote(addr));
    }
    Ok(addr)
}

type Reply = tiny_http::ResponseBox;

/// `response` with one more header (skipped if the name or value is not valid, which fixed ASCII
/// strings never are).
fn with(response: Reply, name: &str, value: &str) -> Reply {
    match Header::from_bytes(name.as_bytes(), value.as_bytes()) {
        Ok(header) => response.with_header(header).boxed(),
        Err(()) => response,
    }
}

/// A bound server, ready to be run on a thread.
pub struct Bound {
    server: Arc<Server>,
    /// The address actually bound (useful with port 0).
    pub addr: SocketAddr,
}

impl std::fmt::Debug for Bound {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Bound")
            .field("addr", &self.addr)
            .finish_non_exhaustive()
    }
}

/// Binds `addr`.
///
/// # Errors
///
/// Fails if the address cannot be bound.
pub fn bind(addr: SocketAddr) -> Result<Bound, ListenError> {
    let server = Server::http(addr).map_err(|e| ListenError::Bind(addr, e.to_string()))?;
    let bound = server
        .server_addr()
        .to_ip()
        .ok_or_else(|| ListenError::Bind(addr, "not an IP address".to_owned()))?;
    Ok(Bound {
        server: Arc::new(server),
        addr: bound,
    })
}

impl Bound {
    /// Serves requests from `shared` on a background thread until the process ends.
    pub fn spawn(&self, shared: Shared) {
        let server = Arc::clone(&self.server);
        // A loopback server answers only names that mean this machine, so a web page cannot
        // reach it by pointing its own domain at 127.0.0.1 (DNS rebinding).
        let local_only = self.addr.ip().is_loopback();
        thread::spawn(move || {
            for request in server.incoming_requests() {
                handle(request, &shared, local_only);
            }
        });
    }

    /// Stops accepting requests.
    pub fn unblock(&self) {
        self.server.unblock();
    }
}

/// Whether a `Host` header names this machine (`localhost`, `127.0.0.1`, `[::1]`, any port).
fn host_is_local(host: &str) -> bool {
    let name = match host.strip_prefix('[') {
        Some(rest) => rest.split(']').next().map(|ip| format!("[{ip}]")),
        None => host.split(':').next().map(str::to_owned),
    };
    matches!(
        name.as_deref().map(str::to_ascii_lowercase).as_deref(),
        Some("localhost" | "127.0.0.1" | "[::1]")
    )
}

fn handle(request: Request, shared: &Shared, local_only: bool) {
    if local_only {
        let host = request
            .headers()
            .iter()
            .find(|h| h.field.equiv("Host"))
            .map(|h| h.value.as_str().to_owned());
        if host.is_some_and(|h| !host_is_local(&h)) {
            drop(request.respond(Response::from_string("unknown host").with_status_code(421)));
            return;
        }
    }
    let method = request.method().clone();
    let path = request.url().split('?').next().unwrap_or("/").to_owned();
    if method != Method::Get && method != Method::Head {
        drop(request.respond(Response::from_string("method not allowed").with_status_code(405)));
        return;
    }
    let published = match shared.read() {
        Ok(guard) => guard.clone(),
        Err(poisoned) => poisoned.into_inner().clone(),
    };
    let etag = format!("\"{}\"", published.generation);
    let ready = published.generation > 0;
    let response: Reply = match path.as_str() {
        "/" | "/index.html" if ready => with(
            with(
                Response::from_string(published.html.as_str()).boxed(),
                "Content-Type",
                "text/html; charset=utf-8",
            ),
            "Cache-Control",
            "no-store",
        ),
        "/report.json" if ready => {
            let not_modified = request
                .headers()
                .iter()
                .any(|h| h.field.equiv("If-None-Match") && h.value.as_str() == etag);
            if not_modified {
                with(Response::empty(304).boxed(), "ETag", &etag)
            } else {
                with(
                    with(
                        with(
                            Response::from_string(published.json.as_str()).boxed(),
                            "Content-Type",
                            "application/json",
                        ),
                        "ETag",
                        &etag,
                    ),
                    "Cache-Control",
                    "no-store",
                )
            }
        }
        "/status" => {
            let status = Status {
                generation: published.generation,
                updated_at: published.updated_at.as_deref(),
                next_at: published.next_at.as_deref(),
                last_error: published.last_error.as_deref(),
                interval_secs: published.interval_secs,
            };
            let body = serde_json::to_string(&status).unwrap_or_else(|_| "{}".to_owned());
            with(
                with(
                    Response::from_string(body).boxed(),
                    "Content-Type",
                    "application/json",
                ),
                "Cache-Control",
                "no-store",
            )
        }
        "/healthz" if ready => Response::from_string("ok").boxed(),
        "/" | "/index.html" | "/report.json" | "/healthz" => {
            let why = published
                .last_error
                .as_deref()
                .map_or(String::new(), |e| format!("\nlast error: {e}"));
            Response::from_string(format!("the first report is still being built{why}"))
                .with_status_code(503)
                .boxed()
        }
        _ => Response::from_string("not found")
            .with_status_code(404)
            .boxed(),
    };
    drop(request.respond(response));
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_names_for_this_machine_pass_the_host_check() {
        for ok in [
            "localhost:8080",
            "127.0.0.1:9",
            "[::1]:80",
            "LOCALHOST",
            "[::1]",
        ] {
            assert!(host_is_local(ok), "{ok}");
        }
        for bad in [
            "evil.example:8080",
            "127.0.0.1.evil.example",
            "localhost.evil.example:1",
            "",
        ] {
            assert!(!host_is_local(bad), "{bad}");
        }
    }

    #[test]
    fn only_loopback_addresses_are_served_without_permission() {
        assert_eq!(parse_listen("127.0.0.1:8080", false).unwrap().port(), 8080);
        parse_listen("[::1]:9000", false).unwrap();
        assert!(matches!(
            parse_listen("0.0.0.0:8080", false),
            Err(ListenError::Remote(_))
        ));
        assert!(matches!(
            parse_listen("192.168.1.5:80", false),
            Err(ListenError::Remote(_))
        ));
        parse_listen("0.0.0.0:8080", true).unwrap();
        assert!(matches!(
            parse_listen("not an address", false),
            Err(ListenError::Address(_))
        ));
    }
}
