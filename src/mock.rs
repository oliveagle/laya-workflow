//! Mock external services for exercising the workflow capabilities offline.
//!
//! Rust port of `laya-workflow mock serve` (score/agent HTTP + stdio agent) and
//! `laya-workflow mock serve` (redis/nats/mqtt/smtp/s3/prom/kafka/udp/web), one
//! CLI: `laya-workflow mock serve --score 8791 ...`. `--stdio` runs the
//! line-delimited JSON agent the stdio transports use.
//!
//! Readiness is printed as `{"ready": true, "listeners": {...}}` only after
//! every requested listener answers on its port, so a launcher can wait for
//! the line instead of sleeping (UDP is probed by a datagram round-trip).

use anyhow::Result;
use serde_json::{json, Value};
use std::collections::HashMap;
use std::io::{BufRead, BufReader, Read, Write};
use std::net::{SocketAddr, TcpListener, TcpStream, UdpSocket};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::thread;

// ── HTTP mock (mock_server.py + the HTTP services of mock_services.py) ─────

fn score(text: &str) -> Value {
    let t = text.to_lowercase();
    let hits: Vec<&str> = ["urgent", "refund", "immediately", "scripted", "bulk", "now"]
        .iter()
        .filter(|w| t.contains(**w))
        .copied()
        .collect();
    let mut risk = 0.12 * hits.len() as f64;
    if t.contains("refund") && t.contains("immediately") {
        risk = risk.max(0.75);
    }
    risk = risk.min(1.0);
    let label = if risk >= 0.6 {
        "high"
    } else if risk >= 0.3 {
        "medium"
    } else {
        "low"
    };
    json!({ "risk": (risk * 10000.0).round() / 10000.0, "label": label, "hits": hits })
}

fn http_reply(stream: &mut TcpStream, code: u16, body: &[u8], ctype: &str) {
    let status = match code {
        200 => "200 OK",
        204 => "204 No Content",
        302 => "302 Found",
        400 => "400 Bad Request",
        404 => "404 Not Found",
        500 => "500 Internal Server Error",
        _ => "200 OK",
    };
    let resp = format!(
        "HTTP/1.1 {status}\r\nContent-Type: {ctype}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
        body.len()
    );
    let _ = stream.write_all(resp.as_bytes());
    let _ = stream.write_all(body);
    let _ = stream.flush();
}

fn http_json(stream: &mut TcpStream, code: u16, v: &Value) {
    http_reply(stream, code, v.to_string().as_bytes(), "application/json");
}

fn url_decode(s: &str) -> String {
    let bytes = s.as_bytes();
    let mut out: Vec<u8> = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        match bytes[i] {
            b'%' if i + 2 < bytes.len() => {
                let hex = std::str::from_utf8(&bytes[i + 1..i + 3]).unwrap_or("00");
                if let Ok(b) = u8::from_str_radix(hex, 16) {
                    out.push(b);
                    i += 3;
                    continue;
                }
                out.push(bytes[i]);
                i += 1;
            }
            b'+' => {
                out.push(b' ');
                i += 1;
            }
            c => {
                out.push(c);
                i += 1;
            }
        }
    }
    String::from_utf8_lossy(&out).into_owned()
}

fn query_param(query: &str, key: &str) -> String {
    for pair in query.split('&') {
        if let Some((k, v)) = pair.split_once('=') {
            if k == key {
                return url_decode(v);
            }
        }
    }
    String::new()
}

struct HttpMock {
    s3: Arc<std::sync::Mutex<HashMap<String, Vec<u8>>>>,
    port: u16,
}

impl HttpMock {
    fn new(port: u16) -> Self {
        Self {
            s3: Arc::new(std::sync::Mutex::new(HashMap::new())),
            port,
        }
    }

    fn serve(&self, stream: &mut TcpStream) {
        let mut reader = BufReader::new(stream.try_clone().unwrap());
        let mut req_line = String::new();
        if reader.read_line(&mut req_line).is_err() {
            return;
        }
        let mut parts = req_line.trim().split_whitespace();
        let method = parts.next().unwrap_or("GET").to_string();
        let target = parts.next().unwrap_or("/").to_string();
        let mut content_length = 0usize;
        let mut x_signature = String::new();
        loop {
            let mut line = String::new();
            if reader.read_line(&mut line).is_err() {
                return;
            }
            let line = line.trim_end();
            if line.is_empty() {
                break;
            }
            let lower = line.to_lowercase();
            if let Some(v) = lower.strip_prefix("content-length:") {
                content_length = v.trim().parse().unwrap_or(0);
            }
            if lower.starts_with("x-signature:") {
                x_signature = line.splitn(2, ':').nth(1).unwrap_or("").trim().to_string();
            }
        }
        let mut body = vec![0u8; content_length];
        if content_length > 0 {
            let _ = reader.read_exact(&mut body);
        }
        let path = target.split('?').next().unwrap_or("/");
        let query = target.splitn(2, '?').nth(1).unwrap_or("");
        let body_text = String::from_utf8_lossy(&body).into_owned();
        let parsed: Value = serde_json::from_str(&body_text).unwrap_or(Value::Null);

        let path = path.to_string();
        match (method.as_str(), path.as_str()) {
            ("GET", "/health") => http_json(stream, 200, &json!({"ok": true})),
            ("GET", p) if p.ends_with("/events") => {
                let payload =
                    b"event: tick\ndata: {\"n\": 1}\n\nevent: tick\ndata: {\"n\": 2}\n\nevent: done\ndata: bye\n\n";
                http_reply(stream, 200, payload, "text/event-stream");
            }
            ("GET", p) if p.ends_with("/search") => {
                let q = query_param(query, "q");
                if q.is_empty() {
                    http_json(stream, 400, &json!({"error": "missing q"}));
                    return;
                }
                http_json(
                    stream,
                    200,
                    &json!({
                        "query": q,
                        "results": [
                            {"title": format!("{q} overview"),
                             "url": format!("http://127.0.0.1:{}/page?name=guide", self.port),
                             "snippet": format!("Overview of {q}")},
                            {"title": format!("{q} release notes"),
                             "url": format!("http://127.0.0.1:{}/page?name=notes", self.port),
                             "snippet": format!("Notes mentioning {q}")},
                        ]
                    }),
                );
            }
            ("GET", p) if p.ends_with("/page") => {
                let name = query_param(query, "name");
                if name == "large" {
                    http_reply(stream, 200, &vec![b'x'; 8192], "text/plain");
                    return;
                }
                match name.as_str() {
                    "guide" => http_reply(
                        stream,
                        200,
                        b"<title>Deployment Guide</title><h1>Deployment Guide</h1><p>Set the token before starting &amp; verify with <code>--check</code>.</p><ul><li>Step one</li><li>Step two</li></ul><p>Read the <a href=\"/page?name=notes\">release notes</a> for details.</p><script>var leak = 'should-not-appear';</script>",
                        "text/html; charset=utf-8",
                    ),
                    "notes" => http_reply(
                        stream,
                        200,
                        b"<title>Release Notes</title><h2>Changes</h2><p>Fixed the SMTP reader.</p>",
                        "text/html; charset=utf-8",
                    ),
                    _ => http_reply(
                        stream,
                        404,
                        b"<title>Not Found</title><p>no such page</p>",
                        "text/html",
                    ),
                }
            }
            ("GET", p) if p.ends_with("/redirect") => {
                let to = query_param(query, "to");
                let to = if to.is_empty() { "/page?name=guide" } else { &to };
                let resp = format!(
                    "HTTP/1.1 302 Found\r\nlocation: {to}\r\ncontent-length: 0\r\nConnection: close\r\n\r\n"
                );
                let _ = stream.write_all(resp.as_bytes());
            }
            ("GET", p)
                if p.ends_with("/bucket") || p.contains("prefix=") || p.ends_with("/bucket/") =>
            {
                let keys = {
                    let map = self.s3.lock().unwrap();
                    let mut ks: Vec<_> = map.keys().cloned().collect();
                    ks.sort();
                    let mut s = String::new();
                    for k in ks {
                        let short = k.trim_start_matches('/').split('/').nth(2).unwrap_or(&k).to_string();
                        s.push_str(&format!("<Contents><Key>{short}</Key></Contents>"));
                    }
                    s
                };
                http_reply(
                    stream,
                    200,
                    format!("<ListBucketResult>{keys}</ListBucketResult>").as_bytes(),
                    "application/xml",
                );
            }
            ("GET", p) if p.ends_with("/api/v1/query") => {
                http_json(
                    stream,
                    200,
                    &json!({"status": "success", "data": {"resultType": "vector", "result": []}}),
                );
            }
            ("GET", _) if path.starts_with("/metrics") || path == "/" => {
                http_reply(
                    stream,
                    200,
                    b"# HELP mock_metric a mock gauge\n# TYPE mock_metric gauge\nmock_metric{job=\"a\"} 1.5\nmock_metric{job=\"b\"} 2.5\nmock_up 1\n",
                    "text/plain",
                );
            }
            ("GET", p) => {
                let v = self.s3.lock().unwrap().get(p).cloned();
                match v {
                    Some(v) => http_reply(stream, 200, &v, "application/octet-stream"),
                    None => http_reply(stream, 404, b"<Error>NoSuchKey</Error>", "application/xml"),
                }
            }
            ("PUT", p) => {
                self.s3.lock().unwrap().insert(p.to_string(), body.clone());
                http_reply(stream, 200, b"", "application/xml");
            }
            ("DELETE", p) => {
                self.s3.lock().unwrap().remove(p);
                http_reply(stream, 204, b"", "application/xml");
            }
            ("POST", _) if parsed.is_null() && !body.is_empty() => {
                http_json(stream, 400, &json!({"error": "invalid json"}));
            }
            ("POST", p) if p.starts_with("/rpc") => {
                let id = parsed.get("id").cloned();
                let method = parsed.get("method").and_then(Value::as_str).unwrap_or("");
                if method == "boom" {
                    http_json(
                        stream,
                        200,
                        &json!({"jsonrpc": "2.0", "id": id, "error": {"code": -32000, "message": "boom requested"}}),
                    );
                } else {
                    http_json(
                        stream,
                        200,
                        &json!({"jsonrpc": "2.0", "id": id, "result": {"method": method, "params": parsed.get("params")}}),
                    );
                }
            }
            ("POST", p) if p.starts_with("/graphql") => {
                let q = parsed.get("query").and_then(Value::as_str).unwrap_or("");
                if q.contains("fail") {
                    http_json(stream, 200, &json!({"errors": [{"message": "graphql failure"}]}));
                } else {
                    http_json(
                        stream,
                        200,
                        &json!({"data": {"echo": parsed.get("variables"), "query": q}}),
                    );
                }
            }
            ("POST", p) if p.starts_with("/chat") => {
                let msgs = parsed.get("messages").and_then(Value::as_array);
                let last = msgs
                    .and_then(|a| a.last())
                    .and_then(|m| m.get("content"))
                    .and_then(Value::as_str)
                    .unwrap_or("");
                http_json(
                    stream,
                    200,
                    &json!({
                        "choices": [{"message": {"role": "assistant", "content": format!("echo:{last}")}}],
                        "usage": {"prompt_tokens": last.len(), "completion_tokens": 3}
                    }),
                );
            }
            ("POST", p) if p.starts_with("/mcp") => {
                let name = parsed
                    .get("params")
                    .and_then(|x| x.get("name"))
                    .and_then(Value::as_str)
                    .unwrap_or("");
                if name == "fail_tool" {
                    http_json(
                        stream,
                        200,
                        &json!({"jsonrpc": "2.0", "id": parsed.get("id"), "error": {"code": -32601, "message": "tool not found"}}),
                    );
                } else {
                    http_json(
                        stream,
                        200,
                        &json!({"jsonrpc": "2.0", "id": parsed.get("id"), "result": {"content": [{"type": "text", "text": format!("tool:{name}")}]}}),
                    );
                }
            }
            ("POST", p) if p.starts_with("/vector") => {
                let op = if parsed.get("items").is_some() { "upsert" } else { "search" };
                http_json(
                    stream,
                    200,
                    &json!({"hits": [{"id": "v1", "score": 0.91, "op": op}], "collection": parsed.get("collection")}),
                );
            }
            ("POST", p) if p.starts_with("/webhook") => {
                http_json(
                    stream,
                    200,
                    &json!({"received": true, "event": parsed.get("event"), "signature": x_signature}),
                );
            }
            ("POST", p) if p.starts_with("/score") => {
                http_json(
                    stream,
                    200,
                    &score(parsed.get("text").and_then(Value::as_str).unwrap_or("")),
                );
            }
            ("POST", p) if p.starts_with("/agent") => {
                http_json(
                    stream,
                    200,
                    &json!({
                        "output": format!("ack:{}", parsed.get("session").and_then(Value::as_str).unwrap_or("default")),
                        "echo": parsed, "turn": 1
                    }),
                );
            }
            ("POST", p) if p.contains("/topics/") => {
                let topic = p.rsplit('/').next().unwrap_or("").to_string();
                http_json(
                    stream,
                    200,
                    &json!({"offsets": [{"partition": 0, "offset": 1, "topic": topic}]}),
                );
            }
            _ => http_json(stream, 404, &json!({"error": "not found", "path": path})),
        }
    }
}

// ── line-delimited stdio agent (codex-style JSON-per-line protocol) ──────

pub fn run_stdio() -> Result<()> {
    let stdin = std::io::stdin();
    let stdout = std::io::stdout();
    let mut out = stdout.lock();
    for line in stdin.lock().lines() {
        let line = line?;
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        let req: Value = match serde_json::from_str(line) {
            Ok(v) => v,
            Err(_) => {
                let _ = writeln!(out, "{}", json!({"error": "invalid json"}));
                continue;
            }
        };
        let reply = if req.get("method").is_some() {
            let name = req
                .get("params")
                .and_then(|x| x.get("name"))
                .and_then(Value::as_str)
                .unwrap_or("");
            if name == "fail_tool" {
                json!({"jsonrpc": "2.0", "id": req.get("id"), "error": {"code": -32601, "message": "tool not found"}})
            } else {
                json!({"jsonrpc": "2.0", "id": req.get("id"), "result": {"content": [{"type": "text", "text": format!("stdio-tool:{name}")}]}})
            }
        } else {
            json!({
                "output": format!("stdio-ack:{}", req.get("session").and_then(Value::as_str).unwrap_or("default")),
                "echo": req, "turns": 1
            })
        };
        let _ = writeln!(out, "{}", reply);
    }
    Ok(())
}

// ── TCP protocol mocks (redis/nats/mqtt/smtp) + UDP echo ───────────────

pub fn serve_redis(listener: TcpListener) -> std::io::Result<()> {
    let port = listener.local_addr().map(|a| a.port()).unwrap_or(0);
    println!("redis mock on 127.0.0.1:{port}");
    let store: Arc<std::sync::Mutex<HashMap<Vec<u8>, Vec<u8>>>> =
        Arc::new(std::sync::Mutex::new(HashMap::new()));
    for conn in listener.incoming() {
        let Ok(mut conn) = conn else { continue };
        let store = store.clone();
        thread::spawn(move || {
            let mut buf: Vec<u8> = Vec::new();
            loop {
                let mut tmp = [0u8; 4096];
                let n = match conn.read(&mut tmp) {
                    Ok(0) | Err(_) => return,
                    Ok(n) => n,
                };
                buf.extend_from_slice(&tmp[..n]);
                while buf.starts_with(b"*") {
                    let Some(end) = buf.windows(2).position(|w| w == b"\r\n") else { break };
                    let n: usize = match std::str::from_utf8(&buf[1..end]).ok().and_then(|s| s.parse().ok()) {
                        Some(v) => v,
                        None => {
                            buf.clear();
                            break;
                        }
                    };
                    let mut rest = buf[end + 2..].to_vec();
                    let mut args: Vec<Vec<u8>> = Vec::new();
                    let mut ok = true;
                    for _ in 0..n {
                        if !rest.starts_with(b"$") {
                            ok = false;
                            break;
                        }
                        let Some(e2) = rest.windows(2).position(|w| w == b"\r\n") else {
                            ok = false;
                            break;
                        };
                        let ln: usize = match std::str::from_utf8(&rest[1..e2]).ok().and_then(|s| s.parse().ok()) {
                            Some(v) => v,
                            None => {
                                ok = false;
                                break;
                            }
                        };
                        let val = rest[e2 + 2..e2 + 2 + ln].to_vec();
                        args.push(val);
                        rest = rest[e2 + 2 + ln + 2..].to_vec();
                    }
                    if !ok {
                        buf.clear();
                        break;
                    }
                    buf = rest;
                    let cmd = args.first().map(|c| c.to_ascii_uppercase()).unwrap_or_default();
                    if cmd == b"PING" {
                        let _ = conn.write_all(b"+PONG\r\n");
                    } else if cmd == b"SET" && args.len() >= 3 {
                        store.lock().unwrap().insert(args[1].clone(), args[2].clone());
                        let _ = conn.write_all(b"+OK\r\n");
                    } else if cmd == b"GET" && args.len() >= 2 {
                        let v = store.lock().unwrap().get(&args[1]).cloned();
                        match v {
                            Some(v) => {
                                let _ = conn.write_all(format!("${}\r\n", v.len()).as_bytes());
                                let _ = conn.write_all(&v);
                                let _ = conn.write_all(b"\r\n");
                            }
                            None => {
                                let _ = conn.write_all(b"$-1\r\n");
                            }
                        }
                    } else if cmd == b"DEL" && args.len() >= 2 {
                        let present = store.lock().unwrap().remove(&args[1]).is_some();
                        let _ = conn.write_all(format!(":{}\r\n", if present { 1 } else { 0 }).as_bytes());
                    } else if cmd == b"INCR" && args.len() >= 2 {
                        let mut s = store.lock().unwrap();
                        let cur: i64 = s
                            .get(&args[1])
                            .and_then(|v| std::str::from_utf8(v).ok().and_then(|x| x.parse().ok()))
                            .unwrap_or(0);
                        s.insert(args[1].clone(), (cur + 1).to_string().into_bytes());
                        let _ = conn.write_all(format!(":{}\r\n", cur + 1).as_bytes());
                    } else if cmd == b"TTL" {
                        let _ = conn.write_all(b":-1\r\n");
                    } else if cmd == b"KEYS" {
                        let keys: Vec<Vec<u8>> = store.lock().unwrap().keys().cloned().collect();
                        let _ = conn.write_all(format!("*{}\r\n", keys.len()).as_bytes());
                        for k in keys {
                            let _ = conn.write_all(format!("${}\r\n", k.len()).as_bytes());
                            let _ = conn.write_all(&k);
                            let _ = conn.write_all(b"\r\n");
                        }
                    } else if cmd == b"AUTH" {
                        let _ = conn.write_all(b"+OK\r\n");
                    } else {
                        let _ = conn.write_all(b"-ERR unknown command\r\n");
                    }
                }
            }
        });
    }
    Ok(())
}

pub fn serve_nats(listener: TcpListener) -> std::io::Result<()> {
    println!("nats mock on 127.0.0.1:{}", listener.local_addr().map(|a| a.port()).unwrap_or(0));
    for conn in listener.incoming() {
        let Ok(mut conn) = conn else { continue };
        thread::spawn(move || {
            let _ = conn.write_all(b"INFO {\"server_id\":\"mock\",\"version\":\"2.10.0\"}\r\n");
            let mut buf = [0u8; 4096];
            loop {
                let n = match conn.read(&mut buf) {
                    Ok(0) | Err(_) => return,
                    Ok(n) => n,
                };
                let data = &buf[..n];
                if data.starts_with(b"PUB") || data.starts_with(b"SUB") || data.starts_with(b"CONNECT") {
                    let _ = conn.write_all(b"+OK\r\n");
                } else if data.starts_with(b"PING") {
                    let _ = conn.write_all(b"PONG\r\n");
                }
            }
        });
    }
    Ok(())
}

pub fn serve_mqtt(listener: TcpListener) -> std::io::Result<()> {
    println!("mqtt mock on 127.0.0.1:{}", listener.local_addr().map(|a| a.port()).unwrap_or(0));
    for conn in listener.incoming() {
        let Ok(mut conn) = conn else { continue };
        thread::spawn(move || {
            let mut buf = [0u8; 4096];
            loop {
                let n = match conn.read(&mut buf) {
                    Ok(0) | Err(_) => return,
                    Ok(n) => n,
                };
                let ptype = buf[0] >> 4;
                if ptype == 1 {
                    let _ = conn.write_all(&[0x20, 0x02, 0x00, 0x00]);
                } else if ptype == 12 {
                    let _ = conn.write_all(&[0xD0, 0x00]);
                }
            }
        });
    }
    Ok(())
}

pub fn serve_smtp(listener: TcpListener) -> std::io::Result<()> {
    println!("smtp mock on 127.0.0.1:{}", listener.local_addr().map(|a| a.port()).unwrap_or(0));
    for conn in listener.incoming() {
        let Ok(mut conn) = conn else { continue };
        thread::spawn(move || {
            let _ = conn.write_all(b"220 mock ESMTP\r\n");
            let mut in_data = false;
            let mut auth_stage = 0u8;
            let mut buf = [0u8; 4096];
            loop {
                let n = match conn.read(&mut buf) {
                    Ok(0) | Err(_) => return,
                    Ok(n) => n,
                };
                let text = String::from_utf8_lossy(&buf[..n]).into_owned();
                if in_data {
                    if text.contains("\r\n.\r\n") || text.trim() == "." {
                        in_data = false;
                        let _ = conn.write_all(b"250 OK queued\r\n");
                    }
                    continue;
                }
                for line in text.split('\n') {
                    let line = line.trim_end_matches(['\r', '\n']);
                    let upper = line.to_uppercase();
                    if auth_stage == 1 {
                        auth_stage = 2;
                        let _ = conn.write_all(b"334 UGFzc3dvcmQ6\r\n");
                        continue;
                    }
                    if auth_stage == 2 {
                        auth_stage = 0;
                        let _ = conn.write_all(b"235 Authentication successful\r\n");
                        continue;
                    }
                    if upper.starts_with("EHLO") || upper.starts_with("HELO") {
                        let _ = conn.write_all(b"250-mock\r\n250 SIZE 10485760\r\n");
                    } else if upper.starts_with("AUTH LOGIN") {
                        auth_stage = 1;
                        let _ = conn.write_all(b"334 VXNlcm5hbWU6\r\n");
                    } else if upper.starts_with("MAIL FROM") {
                        let _ = conn.write_all(b"250 OK\r\n");
                    } else if upper.starts_with("RCPT TO") {
                        let _ = conn.write_all(b"250 OK\r\n");
                    } else if upper == "DATA" {
                        in_data = true;
                        let _ = conn.write_all(b"354 End data with <CR><LF>.<CR><LF>\r\n");
                    } else if upper == "QUIT" {
                        let _ = conn.write_all(b"221 Bye\r\n");
                        return;
                    }
                }
            }
        });
    }
    Ok(())
}

pub fn serve_udp(port: u16) -> std::io::Result<()> {
    let s = UdpSocket::bind(("127.0.0.1", port))?;
    println!("udp echo mock on 127.0.0.1:{port}");
    let mut buf = [0u8; 4096];
    loop {
        let (n, addr) = s.recv_from(&mut buf)?;
        let _ = s.send_to(&buf[..n], addr);
    }
}

// ── process entry: serve requested listeners, print readiness ─────────────

pub struct MockOptions {
    pub redis: u16,
    pub nats: u16,
    pub mqtt: u16,
    pub smtp: u16,
    pub udp: u16,
    pub s3: u16,
    pub prom: u16,
    pub kafka: u16,
    pub web: u16,
    pub score: u16,
    pub agent: u16,
    pub rpc: u16,
    pub graphql: u16,
    pub chat: u16,
    pub mcp: u16,
    pub vector: u16,
    pub webhook: u16,
}

impl Default for MockOptions {
    fn default() -> Self {
        Self {
            redis: 0, nats: 0, mqtt: 0, smtp: 0, udp: 0,
            s3: 0, prom: 0, kafka: 0, web: 0,
            score: 0, agent: 0, rpc: 0, graphql: 0, chat: 0,
            mcp: 0, vector: 0, webhook: 0,
        }
    }
}

fn bind_retry(port: u16, retries: u32) -> Option<TcpListener> {
    for _ in 0..retries {
        match TcpListener::bind(("127.0.0.1", port)) {
            Ok(l) => return Some(l),
            Err(_) => thread::sleep(std::time::Duration::from_millis(250)),
        }
    }
    None
}

fn listener_ready(port: u16, tcp: bool) -> bool {
    let addr: SocketAddr = ([127, 0, 0, 1], port).into();
    if tcp {
        return TcpStream::connect_timeout(&addr, std::time::Duration::from_millis(200))
            .map(|_| true)
            .unwrap_or(false);
    }
    match UdpSocket::bind("127.0.0.1:0") {
        Ok(u) => {
            u.set_read_timeout(Some(std::time::Duration::from_millis(200))).ok();
            let _ = u.send_to(b"ping", addr);
            let mut b = [0u8; 64];
            u.recv_from(&mut b).map(|_| true).unwrap_or(false)
        }
        Err(_) => false,
    }
}

pub fn run_serve(o: &MockOptions) -> Result<()> {
    let all: Vec<(&str, u16, bool)> = vec![
        ("redis", o.redis, true),
        ("nats", o.nats, true),
        ("mqtt", o.mqtt, true),
        ("smtp", o.smtp, true),
        ("udp", o.udp, false),
        ("s3", o.s3, true),
        ("prom", o.prom, true),
        ("kafka", o.kafka, true),
        ("web", o.web, true),
        ("score", o.score, true),
        ("agent", o.agent, true),
        ("rpc", o.rpc, true),
        ("graphql", o.graphql, true),
        ("chat", o.chat, true),
        ("mcp", o.mcp, true),
        ("vector", o.vector, true),
        ("webhook", o.webhook, true),
    ];
    let wanted: Vec<(String, u16, bool)> = all
        .into_iter()
        .filter(|(_, p, _)| *p > 0)
        .map(|(n, p, t)| (n.to_string(), p, t))
        .collect();
    if wanted.is_empty() {
        eprintln!("nothing to serve; pass at least one --<service> PORT");
        std::process::exit(2);
    }

    // Bind every TCP listener up front (retrying past TIME_WAIT). HTTP service
    // names that share a port (e.g. --score 8791 --agent 8791) share ONE
    // listener; the handler routes by path, so a single bind serves them all.
    const HTTP_NAMES: &[&str] = &[
        "s3", "prom", "kafka", "web", "score", "agent", "rpc", "graphql",
        "chat", "mcp", "vector", "webhook",
    ];
    let mut bound: HashMap<String, (u16, bool, TcpListener)> = HashMap::new();
    let mut http_bound_ports: HashMap<u16, String> = HashMap::new();
    for (name, port, tcp) in &wanted {
        if !*tcp {
            continue; // UDP binds itself inside serve_udp
        }
        if HTTP_NAMES.contains(&name.as_str()) {
            if let Some(primary) = http_bound_ports.get(port) {
                // already bound by another HTTP service on the same port
                let _ = primary;
                continue;
            }
        }
        match bind_retry(*port, 40) {
            Some(l) => {
                if HTTP_NAMES.contains(&name.as_str()) {
                    http_bound_ports.insert(*port, name.clone());
                }
                bound.insert(name.clone(), (*port, *tcp, l));
            }
            None => {
                println!(
                    "{}",
                    json!({"ready": false, "error": format!("{name}:{port}: cannot bind")})
                );
                std::process::exit(1);
            }
        }
    }

    let stop = Arc::new(AtomicBool::new(false));
    let mut handles: Vec<thread::JoinHandle<()>> = Vec::new();

    // UDP service (binds itself, but on the requested port — never port 0).
    if let Some((name, port, tcp)) = wanted.iter().find(|(n, _, t)| n == "udp" && !*t) {
        let _ = (name, tcp);
        let stop = stop.clone();
        let udp_port = *port;
        handles.push(thread::spawn(move || {
            let _ = &stop;
            let _ = serve_udp(udp_port); // never returns; binds on udp_port
        }));
    }

    for (name, (port, _tcp, listener)) in bound {
        let stop = stop.clone();
        let name = name.clone();
        let handle = thread::spawn(move || {
            match name.as_str() {
                "s3" | "prom" | "kafka" | "web" | "score" | "agent" | "rpc" | "graphql"
                | "chat" | "mcp" | "vector" | "webhook" => {
                    let mock = std::sync::Arc::new(HttpMock::new(port));
                    while !stop.load(Ordering::Relaxed) {
                        match listener.accept() {
                            Ok((mut stream, _)) => {
                                let m = mock.clone();
                                thread::spawn(move || m.serve(&mut stream));
                            }
                            Err(_) => break,
                        }
                    }
                }
                "redis" => {
                    let _ = serve_redis(listener);
                }
                "nats" => {
                    let _ = serve_nats(listener);
                }
                "mqtt" => {
                    let _ = serve_mqtt(listener);
                }
                "smtp" => {
                    let _ = serve_smtp(listener);
                }
                _ => {}
            }
        });
        handles.push(handle);
    }

    // Wait until every requested listener answers on its port (TCP connect /
    // UDP round-trip). Print the ready line only once all are up.
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
    let mut pending: Vec<(String, u16, bool)> = wanted.clone();
    while !pending.is_empty() && std::time::Instant::now() < deadline {
        pending.retain(|(_, port, tcp)| !listener_ready(*port, *tcp));
        if !pending.is_empty() {
            thread::sleep(std::time::Duration::from_millis(50));
        }
    }
    if !pending.is_empty() {
        let names: Vec<&str> = pending.iter().map(|(n, _, _)| n.as_str()).collect();
        println!(
            "{}",
            json!({"ready": false, "error": format!("listeners not up: {names:?}")})
        );
        std::process::exit(1);
    }
    let started: std::collections::BTreeMap<String, u16> =
        wanted.iter().map(|(n, p, _)| (n.clone(), *p)).collect();
    println!("{}", json!({"ready": true, "listeners": started}));

    // Block forever; the launcher (mock_services.sh) stops us with SIGTERM.
    loop {
        thread::sleep(std::time::Duration::from_secs(30));
    }
}
