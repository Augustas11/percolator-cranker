//! Tiny blocking HTTP/1.1 server on `std::net::TcpListener`. GET only, one
//! request per connection, one thread per connection (the monitor expects a
//! scraper and a dashboard, not public traffic: bind it to localhost).

use std::io::{BufRead, BufReader, Write};
use std::net::{TcpListener, TcpStream};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use super::metrics;
use super::snapshot::Snapshot;

pub const DASHBOARD_HTML: &str = include_str!(concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/monitor/dashboard.html"
));

pub type Shared = Arc<Mutex<Option<Snapshot>>>;

/// Response for a request path: (status, content type, body).
pub fn route(path: &str, snap: Option<&Snapshot>) -> (&'static str, &'static str, String) {
    let path = path.split('?').next().unwrap_or("");
    match (path, snap) {
        ("/" | "/index.html", _) => (
            "200 OK",
            "text/html; charset=utf-8",
            DASHBOARD_HTML.to_string(),
        ),
        ("/metrics", Some(s)) => (
            "200 OK",
            "text/plain; version=0.0.4; charset=utf-8",
            metrics::render(s),
        ),
        ("/snapshot.json", Some(s)) => ("200 OK", "application/json", s.to_json()),
        ("/metrics" | "/snapshot.json", None) => (
            "503 Service Unavailable",
            "text/plain; charset=utf-8",
            "no snapshot yet\n".to_string(),
        ),
        _ => (
            "404 Not Found",
            "text/plain; charset=utf-8",
            "not found\n".to_string(),
        ),
    }
}

fn handle(mut stream: TcpStream, shared: &Shared) -> std::io::Result<()> {
    stream.set_read_timeout(Some(Duration::from_secs(5)))?;
    stream.set_write_timeout(Some(Duration::from_secs(5)))?;
    let mut reader = BufReader::new(stream.try_clone()?);
    let mut request_line = String::new();
    reader.read_line(&mut request_line)?;
    // Drain headers (bounded).
    for _ in 0..100 {
        let mut h = String::new();
        if reader.read_line(&mut h)? == 0 || h == "\r\n" || h == "\n" {
            break;
        }
    }
    let mut parts = request_line.split_whitespace();
    let method = parts.next().unwrap_or("");
    let path = parts.next().unwrap_or("/");
    let (status, ctype, body) = if method != "GET" && method != "HEAD" {
        (
            "405 Method Not Allowed",
            "text/plain; charset=utf-8",
            "GET only\n".to_string(),
        )
    } else {
        let guard = shared.lock().unwrap_or_else(|e| e.into_inner());
        route(path, guard.as_ref())
    };
    let head = format!(
        "HTTP/1.1 {status}\r\nContent-Type: {ctype}\r\nContent-Length: {}\r\nCache-Control: no-store\r\nConnection: close\r\n\r\n",
        body.len()
    );
    stream.write_all(head.as_bytes())?;
    if method != "HEAD" {
        stream.write_all(body.as_bytes())?;
    }
    stream.flush()
}

/// Serve forever.
pub fn serve(listener: TcpListener, shared: Shared) {
    for conn in listener.incoming() {
        let Ok(stream) = conn else { continue };
        let shared = shared.clone();
        std::thread::spawn(move || {
            let _ = handle(stream, &shared);
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn routes() {
        assert_eq!(route("/", None).0, "200 OK");
        assert!(route("/", None).2.contains("snapshot.json"));
        assert_eq!(route("/metrics", None).0, "503 Service Unavailable");
        assert_eq!(route("/nope", None).0, "404 Not Found");
    }
}
