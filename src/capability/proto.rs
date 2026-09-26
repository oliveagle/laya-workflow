//! Raw-socket + plaintext-protocol capabilities, implemented on `std::net`.
//!
//! * `tcp`  — connect / send / recv (short timeout)
//! * `udp`  — send / recv datagrams
//! * `redis`— RESP2 client: get / set / del / incr / ttl / ping / keys
//! * `nats` — NATS text protocol: publish
//! * `mqtt` — MQTT 3.1.1 CONNECT/PUBLISH/DISCONNECT
//! * `smtp` — SMTP session: EHLO / (AUTH LOGIN) / MAIL / RCPT / DATA
//!
//! All calls are egress-gated by `policy.allow_hosts` and bounded by the shared
//! timeout/output caps. No third-party crates are used — every protocol is
//! hand-rolled over `TcpStream`/`UdpSocket`.

use anyhow::{anyhow, bail, Result};
use serde_json::{json, Value};
use std::io::{Read, Write};
use std::net::{TcpStream, ToSocketAddrs, UdpSocket};
use std::time::Duration;

use super::{check_host, expand, stringify, truncate, Policy, effective_op};

fn tcp_connect(host: &str, port: u16, timeout_ms: u64, policy: &Policy) -> Result<TcpStream> {
    let url = format!("tcp://{host}:{port}");
    check_host(&url, policy)?;
    let addr = (host, port)
        .to_socket_addrs()
        .map_err(|e| anyhow!("resolve {host}:{port} failed: {e}"))?
        .next()
        .ok_or_else(|| anyhow!("no address for {host}:{port}"))?;
    let t = Duration::from_millis(timeout_ms.min(policy.max_timeout_ms).max(1));
    TcpStream::connect_timeout(&addr, t).map_err(|e| anyhow!("connect {host}:{port} failed: {e}"))
}

// ── tcp ─────────────────────────────────────────────────────────────

#[derive(Clone, Debug, Default)]
pub struct TcpCap {
    pub host: String,
    pub port: u16,
    pub timeout_ms: u64,
    /// Read until this marker (e.g. "\r\n\r\n"); empty ⇒ single read.
    pub until: String,
}

pub fn call_tcp(c: &TcpCap, with: &Value, state: &Value, policy: &Policy) -> Result<Value> {
    let host = stringify(&expand(&Value::String(c.host.clone()), state, with));
    let payload = with.get("send").map(stringify).unwrap_or_default();
    if host.is_empty() || c.port == 0 {
        bail!("tcp needs 'host' and 'port'");
    }
    let timeout = if c.timeout_ms == 0 { 5000 } else { c.timeout_ms };
    let mut s = tcp_connect(&host, c.port, timeout, policy)?;
    let _ = s.set_read_timeout(Some(Duration::from_millis(timeout)));
    let sent = if payload.is_empty() {
        0
    } else {
        s.write_all(payload.as_bytes())?;
        s.flush()?;
        payload.len()
    };
    let mut buf = vec![0u8; 8192];
    let n = match s.read(&mut buf) {
        Ok(n) => n,
        Err(e) if e.kind() == std::io::ErrorKind::WouldBlock || e.kind() == std::io::ErrorKind::TimedOut => 0,
        Err(e) => return Err(anyhow!("tcp read failed: {e}")),
    };
    let reply = truncate(String::from_utf8_lossy(&buf[..n]).to_string(), policy.max_output);
    Ok(json!({
        "capability": "tcp", "host": host, "port": c.port,
        "sent_bytes": sent, "recv_bytes": n, "reply": reply,
        "hit_until": if c.until.is_empty() { Value::Null } else { json!(reply.contains(&c.until)) },
    }))
}

// ── udp ─────────────────────────────────────────────────────────────

#[derive(Clone, Debug, Default)]
pub struct UdpCap {
    pub host: String,
    pub port: u16,
    pub timeout_ms: u64,
}

pub fn call_udp(c: &UdpCap, with: &Value, state: &Value, policy: &Policy) -> Result<Value> {
    let host = stringify(&expand(&Value::String(c.host.clone()), state, with));
    let payload = with.get("send").map(stringify).unwrap_or_default();
    if host.is_empty() || c.port == 0 {
        bail!("udp needs 'host' and 'port'");
    }
    check_host(&format!("udp://{host}:{}", c.port), policy)?;
    let timeout = if c.timeout_ms == 0 { 3000 } else { c.timeout_ms };
    let sock = UdpSocket::bind("0.0.0.0:0")?;
    sock.set_read_timeout(Some(Duration::from_millis(timeout.min(policy.max_timeout_ms))))?;
    sock.send_to(payload.as_bytes(), (host.as_str(), c.port))
        .map_err(|e| anyhow!("udp send failed: {e}"))?;
    let mut buf = vec![0u8; 8192];
    let (n, from) = match sock.recv_from(&mut buf) {
        Ok(v) => v,
        Err(_) => (0, "0.0.0.0:0".parse()?),
    };
    Ok(json!({
        "capability": "udp", "host": host, "port": c.port,
        "sent_bytes": payload.len(), "recv_bytes": n,
        "reply": truncate(String::from_utf8_lossy(&buf[..n]).to_string(), policy.max_output),
        "from": from.to_string(),
    }))
}

// ── redis (RESP2) ───────────────────────────────────────────────────

#[derive(Clone, Debug, Default)]
pub struct RedisCap {
    pub host: String,
    pub port: u16,
    pub timeout_ms: u64,
    pub password: String,
}

fn resp_write(args: &[String], s: &mut TcpStream) -> Result<()> {
    let mut out = format!("*{}\r\n", args.len());
    for a in args {
        out.push_str(&format!("${}\r\n{}\r\n", a.len(), a));
    }
    s.write_all(out.as_bytes())?;
    s.flush()?;
    Ok(())
}

fn resp_read(s: &mut TcpStream) -> Result<Value> {
    let mut byte = [0u8; 1];
    s.read_exact(&mut byte)?;
    let read_line = |s: &mut TcpStream| -> Result<String> {
        let mut line = Vec::new();
        loop {
            let mut b = [0u8; 1];
            s.read_exact(&mut b)?;
            if b[0] == b'\r' {
                let mut nl = [0u8; 1];
                s.read_exact(&mut nl)?;
                break;
            }
            line.push(b[0]);
        }
        Ok(String::from_utf8_lossy(&line).to_string())
    };
    match byte[0] {
        b'+' => Ok(Value::String(read_line(s)?)),
        b'-' => {
            let e = read_line(s)?;
            bail!("redis error: {e}")
        }
        b':' => Ok(json!(read_line(s)?.parse::<i64>().unwrap_or(0))),
        b'$' => {
            let n: i64 = read_line(s)?.parse().unwrap_or(-1);
            if n < 0 {
                return Ok(Value::Null);
            }
            let mut buf = vec![0u8; n as usize + 2];
            s.read_exact(&mut buf)?;
            buf.truncate(n as usize);
            Ok(Value::String(String::from_utf8_lossy(&buf).to_string()))
        }
        b'*' => {
            let n: i64 = read_line(s)?.parse().unwrap_or(0);
            let mut arr = Vec::new();
            for _ in 0..n.max(0) {
                arr.push(resp_read(s)?);
            }
            Ok(Value::Array(arr))
        }
        other => bail!("redis: unexpected reply byte {:?}", other as char),
    }
}

pub fn call_redis(c: &RedisCap, with: &Value, state: &Value, policy: &Policy) -> Result<Value> {
    let host = stringify(&expand(&Value::String(c.host.clone()), state, with));
    if host.is_empty() || c.port == 0 {
        bail!("redis needs 'host' and 'port'");
    }
    let timeout = if c.timeout_ms == 0 { 5000 } else { c.timeout_ms };
    let mut s = tcp_connect(&host, c.port, timeout, policy)?;
    s.set_read_timeout(Some(Duration::from_millis(timeout.min(policy.max_timeout_ms))))?;

    if !c.password.is_empty() {
        let pw = stringify(&expand(&Value::String(c.password.clone()), state, with));
        resp_write(&["AUTH".into(), pw], &mut s)?;
        let _ = resp_read(&mut s)?;
    }

    let op = with.get("op").map(stringify).unwrap_or_else(|| "ping".to_string());
    let key = with.get("key").map(stringify).unwrap_or_default();
    let args: Vec<String> = match op.as_str() {
        "ping" => vec!["PING".into()],
        "get" => vec!["GET".into(), key.clone()],
        "set" => vec!["SET".into(), key.clone(), with.get("value").map(stringify).unwrap_or_default()],
        "del" => vec!["DEL".into(), key.clone()],
        "incr" => vec!["INCR".into(), key.clone()],
        "ttl" => vec!["TTL".into(), key.clone()],
        "keys" => vec!["KEYS".into(), with.get("pattern").map(stringify).unwrap_or_else(|| "*".into())],
        other => bail!("redis op {other:?} unsupported (ping|get|set|del|incr|ttl|keys)"),
    };
    resp_write(&args, &mut s)?;
    let reply = resp_read(&mut s)?;
    Ok(json!({ "capability": "redis", "op": op, "key": key, "reply": reply, "host": host, "port": c.port }))
}

// ── nats (text protocol) ────────────────────────────────────────────

#[derive(Clone, Debug, Default)]
pub struct NatsCap {
    pub host: String,
    pub port: u16,
    pub timeout_ms: u64,
}

pub fn call_nats(c: &NatsCap, with: &Value, state: &Value, policy: &Policy) -> Result<Value> {
    let host = stringify(&expand(&Value::String(c.host.clone()), state, with));
    if host.is_empty() || c.port == 0 {
        bail!("nats needs 'host' and 'port'");
    }
    let subject = with.get("subject").map(stringify).ok_or_else(|| anyhow!("nats needs 'subject'"))?;
    let payload = with.get("message").map(stringify).unwrap_or_default();
    let timeout = if c.timeout_ms == 0 { 5000 } else { c.timeout_ms };
    let mut s = tcp_connect(&host, c.port, timeout, policy)?;
    s.set_read_timeout(Some(Duration::from_millis(timeout.min(policy.max_timeout_ms))))?;

    // server sends INFO first; read it (best effort) before publishing
    let mut info = vec![0u8; 4096];
    let n = s.read(&mut info).unwrap_or(0);
    let greeting = String::from_utf8_lossy(&info[..n]).to_string();

    let pub_cmd = format!("PUB {} {}\r\n{}\r\n", subject, payload.len(), payload);
    s.write_all(pub_cmd.as_bytes())?;
    s.flush()?;
    let mut echo = vec![0u8; 2048];
    let n2 = s.read(&mut echo).unwrap_or(0);
    let reply = truncate(String::from_utf8_lossy(&echo[..n2]).to_string(), policy.max_output);
    Ok(json!({
        "capability": "nats", "host": host, "port": c.port, "subject": subject,
        "sent_bytes": payload.len(), "greeting": truncate(greeting, 512), "reply": reply,
    }))
}

// ── mqtt 3.1.1 ──────────────────────────────────────────────────────

#[derive(Clone, Debug, Default)]
pub struct MqttCap {
    pub host: String,
    pub port: u16,
    pub client_id: String,
    pub timeout_ms: u64,
}

fn mqtt_string(s: &str) -> Vec<u8> {
    let b = s.as_bytes();
    let mut v = vec![(b.len() >> 8) as u8, (b.len() & 0xFF) as u8];
    v.extend_from_slice(b);
    v
}

fn mqtt_remaining_len(mut n: usize, out: &mut Vec<u8>) {
    loop {
        let mut byte = (n % 128) as u8;
        n /= 128;
        if n > 0 {
            byte |= 0x80;
        }
        out.push(byte);
        if n == 0 {
            break;
        }
    }
}

pub fn call_mqtt(c: &MqttCap, with: &Value, state: &Value, policy: &Policy) -> Result<Value> {
    let host = stringify(&expand(&Value::String(c.host.clone()), state, with));
    if host.is_empty() || c.port == 0 {
        bail!("mqtt needs 'host' and 'port'");
    }
    let topic = with.get("topic").map(stringify).ok_or_else(|| anyhow!("mqtt needs 'topic'"))?;
    let payload = with.get("message").map(stringify).unwrap_or_default();
    let timeout = if c.timeout_ms == 0 { 5000 } else { c.timeout_ms };
    let client_id = if c.client_id.is_empty() { "laya-workflow".to_string() } else { c.client_id.clone() };
    let mut s = tcp_connect(&host, c.port, timeout, policy)?;
    s.set_read_timeout(Some(Duration::from_millis(timeout.min(policy.max_timeout_ms))))?;

    // CONNECT
    let mut body = mqtt_string("MQTT");
    body.push(4); // protocol level 3.1.1
    body.push(0x02); // clean session
    body.extend_from_slice(&60u16.to_be_bytes()); // keepalive
    body.extend_from_slice(&mqtt_string(&client_id));
    let mut pkt = vec![0x10];
    mqtt_remaining_len(body.len(), &mut pkt);
    pkt.extend_from_slice(&body);
    s.write_all(&pkt)?;
    s.flush()?;
    let mut connack = [0u8; 4];
    let got_connack = s.read_exact(&mut connack).is_ok();
    let accepted = got_connack && connack[3] == 0;

    // PUBLISH (QoS 0)
    let mut pbody = mqtt_string(&topic);
    pbody.extend_from_slice(payload.as_bytes());
    let mut ppkt = vec![0x30];
    mqtt_remaining_len(pbody.len(), &mut ppkt);
    ppkt.extend_from_slice(&pbody);
    s.write_all(&ppkt)?;
    s.flush()?;

    // DISCONNECT
    let _ = s.write_all(&[0xE0, 0x00]);
    Ok(json!({
        "capability": "mqtt", "host": host, "port": c.port, "topic": topic,
        "connected": accepted, "connack": got_connack,
        "sent_bytes": payload.len(),
    }))
}

// ── smtp ────────────────────────────────────────────────────────────

#[derive(Clone, Debug, Default)]
pub struct SmtpCap {
    pub host: String,
    pub port: u16,
    pub from: String,
    pub timeout_ms: u64,
    pub username: String,
    pub password: String,
}

/// Buffered SMTP reply reader.
///
/// A raw `read()` returns a *chunk*, not a line: a server is free to coalesce
/// several reply lines into one TCP segment (e.g. `250-mock\r\n250 SIZE ...\r\n`)
/// or to split one line across segments. Parsing per-chunk therefore both
/// mis-detects continuation markers and can block waiting for data that has
/// already arrived. This reader keeps a leftover byte buffer and always yields
/// exactly one `\r\n`-terminated line, refilling only when it needs more bytes.
struct SmtpReader {
    buf: Vec<u8>,
    pos: usize,
    /// Wall-clock deadline for the **entire session**, from the capability's
    /// `timeout_ms`.
    ///
    /// Two earlier bugs lived here. First the retry loop used a hardcoded 10s
    /// ceiling that ignored `timeout_ms`. Then, once that was fixed, the budget
    /// was still counted *per read* — but an SMTP session issues many commands
    /// (EHLO/MAIL/RCPT/DATA…), so a hostile peer could burn the full budget on
    /// each one and a "2s" capability ran for ~86s. A single session deadline
    /// bounded the whole exchange instead.
    deadline: std::time::Instant,
}

impl SmtpReader {
    fn new(budget_ms: u64) -> Self {
        Self {
            buf: Vec::new(),
            pos: 0,
            deadline: std::time::Instant::now() + std::time::Duration::from_millis(budget_ms.max(1)),
        }
    }

    /// Read one line, without its trailing CRLF. Blocks (with retry tolerance
    /// for a slow server) until a newline is seen or EOF is reached.
    fn read_line(&mut self, s: &mut TcpStream) -> Result<String> {
        use std::io::{ErrorKind, Read as _};
        let mut waited = 0u64;
        loop {
            if let Some(i) = self.buf[self.pos..].iter().position(|&b| b == b'\n') {
                let end = self.pos + i;
                let mut line = String::from_utf8_lossy(&self.buf[self.pos..end]).into_owned();
                self.pos = end + 1;
                if line.ends_with('\r') {
                    line.pop();
                }
                if self.pos == self.buf.len() {
                    self.buf.clear();
                    self.pos = 0;
                }
                return Ok(line);
            }
            let mut chunk = [0u8; 4096];
            match s.read(&mut chunk) {
                Ok(0) => {
                    let rest = String::from_utf8_lossy(&self.buf[self.pos..]).into_owned();
                    self.buf.clear();
                    self.pos = 0;
                    return Ok(rest);
                }
                Ok(n) => {
                    self.buf.extend_from_slice(&chunk[..n]);
                    waited = 0;
                }
                Err(e) if e.kind() == ErrorKind::WouldBlock || e.kind() == ErrorKind::TimedOut => {
                    if std::time::Instant::now() >= self.deadline {
                        return Err(anyhow!(
                            "smtp session timed out after {waited}ms of waiting"
                        ));
                    }
                    std::thread::sleep(Duration::from_millis(50));
                    waited += 50;
                }
                Err(e) => return Err(anyhow!("smtp read failed: {e}")),
            }
        }
    }
}

/// Read one SMTP reply, consuming continuation lines.
///
/// SMTP multi-line replies look like `250-first\r\n250 second\r\n250 last\r\n`
/// — the character after the 3-digit code is `-` while more lines follow and
/// ` ` on the final line. Because several lines may share one TCP segment, the
/// reply is assembled line by line (never per read chunk).
fn smtp_reply(r: &mut SmtpReader, s: &mut TcpStream, out: &mut String) -> Result<String> {
    let mut all = String::new();
    for _ in 0..64 {
        let line = r.read_line(s)?;
        if line.is_empty() {
            break;
        }
        out.push_str(&line);
        out.push_str("\r\n");
        all.push_str(&line);
        all.push('\n');
        if line.get(3..4) != Some("-") {
            break;
        }
    }
    Ok(all)
}

fn smtp_expect(r: &mut SmtpReader, s: &mut TcpStream, prefix: &str, out: &mut String) -> Result<()> {
    let reply = smtp_reply(r, s, out)?;
    if !reply.starts_with(prefix) {
        bail!("smtp expected {prefix:?}, got {:?}", reply.trim());
    }
    Ok(())
}

fn smtp_cmd(r: &mut SmtpReader, s: &mut TcpStream, cmd: &str, expect: &str, out: &mut String) -> Result<()> {
    s.write_all(format!("{cmd}\r\n").as_bytes())?;
    s.flush()?;
    smtp_expect(r, s, expect, out)
}

pub fn call_smtp(c: &SmtpCap, with: &Value, state: &Value, policy: &Policy) -> Result<Value> {
    let host = stringify(&expand(&Value::String(c.host.clone()), state, with));
    if host.is_empty() || c.port == 0 {
        bail!("smtp needs 'host' and 'port'");
    }
    let to: Vec<String> = with
        .get("to")
        .and_then(|v| v.as_array())
        .map(|a| a.iter().map(stringify).collect())
        .ok_or_else(|| anyhow!("smtp needs 'to' (array of addresses)"))?;
    if to.is_empty() {
        bail!("smtp 'to' must not be empty");
    }
    let subject = with.get("subject").map(stringify).unwrap_or_else(|| "(no subject)".to_string());
    let body_text = with.get("body").map(stringify).unwrap_or_default();
    let from = if c.from.is_empty() {
        with.get("from").map(stringify).unwrap_or_else(|| "noreply@localhost".to_string())
    } else {
        stringify(&expand(&Value::String(c.from.clone()), state, with))
    };
    let timeout = if c.timeout_ms == 0 { 8000 } else { c.timeout_ms };
    let mut s = tcp_connect(&host, c.port, timeout, policy)?;
    s.set_read_timeout(Some(Duration::from_millis(timeout.min(policy.max_timeout_ms))))?;

    let mut transcript = String::new();
    let mut r = SmtpReader::new(timeout.min(policy.max_timeout_ms));
    smtp_expect(&mut r, &mut s, "220", &mut transcript)?;
    smtp_cmd(&mut r, &mut s, &format!("EHLO {}", host), "250", &mut transcript)?;

    if !c.username.is_empty() {
        let user = stringify(&expand(&Value::String(c.username.clone()), state, with));
        let pass = stringify(&expand(&Value::String(c.password.clone()), state, with));
        smtp_cmd(&mut r, &mut s, "AUTH LOGIN", "334", &mut transcript)?;
        let user_b64 = super::local::b64_encode(user.as_bytes());
        // The server may either ask for the password (334) or accept straight
        // away (235) — both are valid per RFC 4954.
        s.write_all(format!("{user_b64}\r\n").as_bytes())?;
        s.flush()?;
        let line = smtp_reply(&mut r, &mut s, &mut transcript)?;
        if line.starts_with("334") {
            let pass_b64 = super::local::b64_encode(pass.as_bytes());
            smtp_cmd(&mut r, &mut s, &pass_b64, "235", &mut transcript)?;
        } else if !line.starts_with("235") {
            bail!("smtp auth failed: {:?}", line.trim());
        }
    }

    smtp_cmd(&mut r, &mut s, &format!("MAIL FROM:<{from}>"), "250", &mut transcript)?;
    for rcpt in &to {
        smtp_cmd(&mut r, &mut s, &format!("RCPT TO:<{rcpt}>"), "250", &mut transcript)?;
    }
    smtp_cmd(&mut r, &mut s, "DATA", "354", &mut transcript)?;
    let msg = format!(
        "From: {from}\r\nTo: {}\r\nSubject: {subject}\r\n\r\n{body_text}\r\n.",
        to.join(", ")
    );
    smtp_cmd(&mut r, &mut s, &msg, "250", &mut transcript)?;
    let _ = s.write_all(b"QUIT\r\n");
    let _ = s.flush();

    Ok(json!({
        "capability": "smtp", "host": host, "port": c.port,
        "from": from, "to": to, "subject": subject,
        "authed": !c.username.is_empty(),
        "transcript": truncate(transcript, 2048),
    }))
}

// ── archive (tar/zip via system tools) ──────────────────────────────

#[derive(Clone, Debug, Default)]
pub struct ArchiveCap {
    pub path: String,
    /// `list` | `extract`
    pub op: String,
    pub dest: String,
    pub timeout_ms: u64,
}

pub fn call_archive(c: &ArchiveCap, with: &Value, state: &Value, policy: &Policy) -> Result<Value> {
    if !policy.allow_exec {
        bail!("archive shells out to tar/unzip; set policy.allow_exec = true to enable");
    }
    let path = stringify(&expand(
        &Value::String(if c.path.is_empty() { with.get("path").map(stringify).unwrap_or_default() } else { c.path.clone() }),
        state,
        with,
    ));
    if path.is_empty() {
        bail!("archive needs 'path'");
    }
    // path must be allow-listed (we read the archive / write extracted files)
    super::store::resolve_store_path(&path, policy)?;
    let argv: Vec<String> = if path.ends_with(".zip") {
        let op = effective_op(&c.op, with, "ping");
        match op.as_str() {
            "list" => vec!["unzip".into(), "-l".into(), path.clone()],
            "extract" => {
                let dest = if c.dest.is_empty() { ".".to_string() } else { c.dest.clone() };
                super::store::resolve_store_path(&dest, policy)?;
                vec!["unzip".into(), "-o".into(), path.clone(), "-d".into(), dest]
            }
            other => bail!("archive op {other:?} unsupported (list | extract)"),
        }
    } else {
        let op = effective_op(&c.op, with, "ping");
        match op.as_str() {
            "list" => vec!["tar".into(), "-tf".into(), path.clone()],
            "extract" => {
                let dest = if c.dest.is_empty() { ".".to_string() } else { c.dest.clone() };
                super::store::resolve_store_path(&dest, policy)?;
                vec!["tar".into(), "-xf".into(), path.clone(), "-C".into(), dest]
            }
            other => bail!("archive op {other:?} unsupported (list | extract)"),
        }
    };
    let cap = super::ExecCap {
        argv,
        cwd: None,
        env: serde_json::Map::new(),
        timeout_ms: if c.timeout_ms == 0 { 30_000 } else { c.timeout_ms },
        max_output: policy.max_output,
    };
    let out = super::call_exec(&cap, &json!({}), &json!({}), policy)?;
    let stdout = out["stdout"].as_str().unwrap_or("").to_string();
    let entries: Vec<String> = stdout.lines().map(str::to_string).filter(|l| !l.trim().is_empty()).collect();
    Ok(json!({
        "capability": "archive", "op": c.op, "path": path,
        "exit_code": out["exit_code"], "entries": entries, "count": entries.len(),
        "stderr": out["stderr"],
    }))
}
