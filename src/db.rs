//! The **server** mode of the `db` HTAP wrapper.
//!
//! The embedded `db` capability ([`crate::capability::db`]) runs the local
//! `sqlite3` / `duckdb` CLIs per call, which is perfect for a single workflow
//! but has two limits: every call re-opens the files, and DuckDB refuses to open
//! one database from two processes. `laya-workflow db serve` starts **one**
//! long-lived process that owns the SQLite(+DuckDB) pair and answers ops over
//! HTTP, so many workflows (and non-workflow tools) share a single writer.
//!
//! This is deliberately dependency-free: a small HTTP/1.1 server built on
//! `std::net`, reusing the exact same engine as embed mode (`call_db` with a
//! server-owned [`DbCap`]). Requests are handled **sequentially** — the store is
//! the shared resource, so serialising it keeps SQLite's single-writer model and
//! avoids two CLI processes racing on one file.
//!
//! Routes:
//!
//! | method | path | body | response |
//! |--------|------|------|----------|
//! | `GET`  | `/health`, `/healthz`, `/` | — | `{ok, service:"laya-db", sqlite, duckdb, alias, uptime_ms}` |
//! | `POST` | `/db` | the `with` object of any `db` op (`query`/`exec`/`analytics`/`sync`/`tables`) | the `call_db` result JSON |

use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};
use std::path::Path;
use std::time::{Duration, Instant};

use anyhow::{Context, Result};
use serde_json::{json, Value};

use crate::capability::db::{call_db, DbCap};
use crate::capability::Policy;

/// Default port for the HTAP daemon (`LAYA_DB_PORT` overrides at the CLI layer).
pub const DEFAULT_DB_PORT: u16 = 18767;
/// Default liveness path (matches the generic `server` lifecycle helpers).
pub const DEFAULT_DB_HEALTH_PATH: &str = "/healthz";
/// Default bind host — localhost only; the store is never exposed off-box.
pub const DEFAULT_DB_HOST: &str = "127.0.0.1";
/// Largest request body we will buffer (a spec's SQL is small; this stops a
/// rogue client from making the daemon allocate without bound).
const MAX_BODY: usize = 4 << 20;
/// Largest request head we will buffer before giving up.
const MAX_HEAD: usize = 64 << 10;

/// What the daemon is bound to. Empty `duckdb` ⇒ analytics run in memory.
#[derive(Clone, Debug)]
pub struct DbServerConfig {
    pub sqlite: String,
    pub duckdb: String,
    pub alias: String,
    pub host: String,
    pub port: u16,
}

impl Default for DbServerConfig {
    fn default() -> Self {
        Self {
            sqlite: String::new(),
            duckdb: String::new(),
            alias: "sqlite".to_string(),
            host: DEFAULT_DB_HOST.to_string(),
            port: DEFAULT_DB_PORT,
        }
    }
}

impl DbServerConfig {
    /// The server-owned capability: always writable, because the daemon *is* the
    /// writer and the per-call read-only guard lives on the client capability.
    fn capability(&self) -> DbCap {
        DbCap {
            sqlite: self.sqlite.clone(),
            duckdb: self.duckdb.clone(),
            alias: self.alias.clone(),
            op: "query".to_string(),
            readonly: false,
            format: "json".to_string(),
            mode: "embed".to_string(),
            endpoint: String::new(),
            timeout_ms: 60_000,
        }
    }

    /// The policy the daemon runs under: exec is required (we spawn the CLIs) and
    /// the two files' parent directories are the allowed roots.
    fn policy(&self) -> Policy {
        let mut allow_paths = Vec::new();
        for p in [self.sqlite.as_str(), self.duckdb.as_str()] {
            if p.is_empty() {
                continue;
            }
            if let Some(dir) = Path::new(p).parent() {
                allow_paths.push(dir.to_string_lossy().to_string());
            }
        }
        Policy {
            allow_exec: true,
            allow_paths,
            ..Policy::default()
        }
    }

    pub fn base(&self) -> String {
        format!("http://{}:{}", self.host, self.port)
    }
}

/// Run the daemon in the foreground (blocks). Prints `BASE=<url>` once bound so
/// the generic `laya-workflow server` lifecycle can capture it, exactly like
/// `server start`.
pub fn serve(cfg: &DbServerConfig) -> Result<()> {
    if cfg.sqlite.trim().is_empty() {
        anyhow::bail!("db serve needs --sqlite (the ACID system of record)");
    }
    // Create the parent dirs up front so the CLIs can open the files and so the
    // allow-list roots canonicalise.
    for p in [cfg.sqlite.as_str(), cfg.duckdb.as_str()] {
        if p.is_empty() {
            continue;
        }
        if let Some(dir) = Path::new(p).parent() {
            if !dir.as_os_str().is_empty() {
                std::fs::create_dir_all(dir)
                    .with_context(|| format!("create db dir {dir:?} failed"))?;
            }
        }
    }

    let cap = cfg.capability();
    let policy = cfg.policy();
    let addr = format!("{}:{}", cfg.host, cfg.port);
    let listener = TcpListener::bind(&addr).with_context(|| format!("bind {addr} failed"))?;
    let local = listener
        .local_addr()
        .map(|a| a.to_string())
        .unwrap_or_else(|_| addr.clone());
    let started = Instant::now();
    eprintln!(
        "[laya-db] serving sqlite={} duckdb={} on {local} (mode=server)",
        cfg.sqlite,
        if cfg.duckdb.is_empty() {
            ":memory:"
        } else {
            cfg.duckdb.as_str()
        }
    );
    println!("BASE={}", cfg.base());
    let _ = std::io::stdout().flush();

    for stream in listener.incoming() {
        match stream {
            Ok(mut s) => {
                let _ = s.set_read_timeout(Some(Duration::from_secs(30)));
                let _ = s.set_write_timeout(Some(Duration::from_secs(30)));
                if let Err(e) = handle(&mut s, &cap, &policy, started) {
                    eprintln!("[laya-db] request error: {e}");
                }
            }
            Err(e) => eprintln!("[laya-db] accept error: {e}"),
        }
    }
    Ok(())
}

/// Handle one connection: parse the request, route it, write a JSON response.
fn handle(stream: &mut TcpStream, cap: &DbCap, policy: &Policy, started: Instant) -> Result<()> {
    let Some(req) = read_request(stream)? else {
        return Ok(());
    };
    let (status, body) = match (req.method.as_str(), req.path.as_str()) {
        ("GET", "/health") | ("GET", "/healthz") | ("GET", "/") => (200, health_json(cap, started)),
        ("POST", "/db") => {
            let status_and = |code: u16, v: Value| (code, v);
            match serde_json::from_slice::<Value>(&req.body) {
                Ok(with) if with.is_object() => match call_db(cap, &with, &json!({}), policy) {
                    Ok(v) => status_and(200, v),
                    // A capability/policy error (bad op, unknown key, …) is the
                    // caller's fault → 400 with the message, still JSON.
                    Err(e) => status_and(
                        400,
                        json!({ "ok": false, "error": e.to_string(), "mode": "server" }),
                    ),
                },
                Ok(_) => status_and(
                    400,
                    json!({ "ok": false, "error": "POST /db body must be a JSON object" }),
                ),
                Err(e) => status_and(
                    400,
                    json!({ "ok": false, "error": format!("POST /db body is not JSON: {e}") }),
                ),
            }
        }
        ("GET", _) | ("POST", _) => (404, json!({ "ok": false, "error": "no such route" })),
        _ => (405, json!({ "ok": false, "error": "method not allowed" })),
    };
    let bytes = serde_json::to_vec(&body).unwrap_or_else(|_| b"{}".to_vec());
    respond(stream, status, &bytes)
}

fn health_json(cap: &DbCap, started: Instant) -> Value {
    json!({
        "ok": true,
        "service": "laya-db",
        "mode": "server",
        "sqlite": cap.sqlite,
        "duckdb": if cap.duckdb.is_empty() { ":memory:" } else { cap.duckdb.as_str() },
        "alias": cap.alias,
        "uptime_ms": started.elapsed().as_millis() as u64,
    })
}

/// Fetch `<endpoint>/health` from a running daemon (short timeout). `None` when
/// nothing healthy answers. `db ensure` uses this to refuse attaching to a
/// daemon that serves a *different* database.
pub fn health(endpoint: &str) -> Option<Value> {
    let url = format!("{}/health", endpoint.trim_end_matches('/'));
    let agent = ureq::AgentBuilder::new()
        .timeout(Duration::from_millis(1500))
        .build();
    let text = agent.get(&url).call().ok()?.into_string().ok()?;
    serde_json::from_str(&text).ok()
}

// ───────────────────────────── HTTP plumbing ─────────────────────────────

/// A parsed request head, plus the offset at which its body begins.
#[derive(Debug, PartialEq)]
struct Head {
    method: String,
    path: String,
    content_len: usize,
    body_at: usize,
}

/// Parse the request line and headers out of a (possibly partial) buffer.
/// Returns `None` until the terminating `\r\n\r\n` has arrived.
fn parse_head(buf: &[u8]) -> Option<Head> {
    let end = find(buf, b"\r\n\r\n")? + 4;
    let head = String::from_utf8_lossy(&buf[..end]);
    let mut lines = head.split("\r\n");
    let mut parts = lines.next().unwrap_or("").split_whitespace();
    let method = parts.next().unwrap_or("").to_ascii_uppercase();
    let path = parts.next().unwrap_or("/").to_string();
    let mut content_len = 0usize;
    for line in lines {
        if let Some((k, v)) = line.split_once(':') {
            if k.trim().eq_ignore_ascii_case("content-length") {
                content_len = v.trim().parse().unwrap_or(0);
            }
        }
    }
    Some(Head {
        method,
        path,
        content_len: content_len.min(MAX_BODY),
        body_at: end,
    })
}

struct Request {
    method: String,
    path: String,
    body: Vec<u8>,
}

/// Read one full request (head + `Content-Length` bytes) from the stream.
fn read_request(stream: &mut TcpStream) -> Result<Option<Request>> {
    let mut buf: Vec<u8> = Vec::with_capacity(4096);
    let mut chunk = [0u8; 4096];
    let head = loop {
        if let Some(h) = parse_head(&buf) {
            break h;
        }
        if buf.len() > MAX_HEAD {
            anyhow::bail!("request head too large");
        }
        let n = stream.read(&mut chunk)?;
        if n == 0 {
            return Ok(None); // client closed before a full request
        }
        buf.extend_from_slice(&chunk[..n]);
    };
    let mut body = buf[head.body_at.min(buf.len())..].to_vec();
    while body.len() < head.content_len {
        let n = stream.read(&mut chunk)?;
        if n == 0 {
            break;
        }
        body.extend_from_slice(&chunk[..n]);
    }
    body.truncate(head.content_len);
    Ok(Some(Request {
        method: head.method,
        path: head.path,
        body,
    }))
}

fn respond(stream: &mut TcpStream, status: u16, body: &[u8]) -> Result<()> {
    let reason = match status {
        200 => "OK",
        400 => "Bad Request",
        404 => "Not Found",
        405 => "Method Not Allowed",
        _ => "Error",
    };
    let head = format!(
        "HTTP/1.1 {status} {reason}\r\nContent-Type: application/json\r\n\
         Content-Length: {}\r\nConnection: close\r\n\r\n",
        body.len()
    );
    stream.write_all(head.as_bytes())?;
    stream.write_all(body)?;
    stream.flush()?;
    Ok(())
}

/// Index of the first occurrence of `needle` in `haystack`.
fn find(haystack: &[u8], needle: &[u8]) -> Option<usize> {
    if needle.is_empty() || haystack.len() < needle.len() {
        return None;
    }
    haystack.windows(needle.len()).position(|w| w == needle)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_head_reads_request_line_and_length() {
        let raw = b"POST /db HTTP/1.1\r\nHost: x\r\nContent-Length: 5\r\n\r\nhello";
        let h = parse_head(raw).unwrap();
        assert_eq!(h.method, "POST");
        assert_eq!(h.path, "/db");
        assert_eq!(h.content_len, 5);
        assert_eq!(&raw[h.body_at..h.body_at + 5], b"hello");
    }

    #[test]
    fn parse_head_is_case_insensitive_and_defaults_length_zero() {
        let h = parse_head(b"GET /healthz HTTP/1.1\r\nhost: x\r\n\r\n").unwrap();
        assert_eq!(h.method, "GET");
        assert_eq!(h.path, "/healthz");
        assert_eq!(h.content_len, 0);
        // Missing terminator ⇒ not ready yet.
        assert!(parse_head(b"GET / HTTP/1.1\r\n").is_none());
    }

    #[test]
    fn find_locates_delimiters() {
        assert_eq!(find(b"ab\r\n\r\ncd", b"\r\n\r\n"), Some(2));
        assert_eq!(find(b"abcd", b"\r\n\r\n"), None);
        assert_eq!(find(b"x", b""), None);
    }

    #[test]
    fn config_policy_allows_both_file_roots() {
        let cfg = DbServerConfig {
            sqlite: "/tmp/layadb/shop.sqlite".to_string(),
            duckdb: "/tmp/layadb/wh.duckdb".to_string(),
            alias: "sqlite".to_string(),
            host: DEFAULT_DB_HOST.to_string(),
            port: DEFAULT_DB_PORT,
        };
        let p = cfg.policy();
        assert!(p.allow_exec);
        assert!(p.allow_paths.contains(&"/tmp/layadb".to_string()));
        assert_eq!(cfg.base(), "http://127.0.0.1:18767");
    }
}
