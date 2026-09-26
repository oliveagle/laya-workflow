//! Network capabilities layered on the HTTP/stdio primitives.
//!
//! * `rpc`      — JSON-RPC 2.0 over HTTP (id/method/params + error mapping)
//! * `graphql`  — GraphQL over HTTP (`query` + `variables`)
//! * `llm`      — OpenAI-compatible `/chat/completions` call
//! * `mcp`      — MCP tool call (HTTP or stdio transport)
//! * `vector`   — vector DB upsert/search over HTTP
//! * `webhook`  — signed HTTP notification with retry/backoff
//! * `sse`      — Server-Sent Events reader with event/time bounds
//!
//! All of these are governed by `policy.allow_hosts` (network egress) plus the
//! shared timeout/output caps; `mcp` with `transport=stdio` additionally needs
//! `policy.allow_exec` because it spawns a process.

use anyhow::{anyhow, bail, Result};
use serde_json::{json, Map, Value};
use std::time::Duration;

use super::{bounded_timeout, check_host, expand, stringify, truncate, Policy};

// ── rpc ─────────────────────────────────────────────────────────────

#[derive(Clone, Debug, Default)]
pub struct RpcCap {
    pub url: String,
    pub method: String,
    pub timeout_ms: u64,
    pub id: Option<i64>,
}

pub fn call_rpc(c: &RpcCap, with: &Value, state: &Value, policy: &Policy) -> Result<Value> {
    let url = stringify(&expand(&Value::String(c.url.clone()), state, with));
    if url.is_empty() {
        bail!("rpc capability needs 'url'");
    }
    check_host(&url, policy)?;
    let method = if c.method.is_empty() {
        with.get("method").map(stringify).unwrap_or_default()
    } else {
        c.method.clone()
    };
    if method.is_empty() {
        bail!("rpc capability needs 'method' (in config or 'with')");
    }
    let params = with.get("params").cloned().unwrap_or_else(|| json!({}));
    let id = c.id.unwrap_or(1);
    let req = json!({ "jsonrpc": "2.0", "id": id, "method": method, "params": params });

    let agent = ureq::AgentBuilder::new().timeout(bounded_timeout(c.timeout_ms, policy)).build();
    let resp = agent
        .post(&url)
        .set("content-type", "application/json")
        .send_json(req)
        .map_err(|e| anyhow!("rpc {url} failed: {e}"))?;
    let status = resp.status() as i64;
    let text = truncate(resp.into_string().map_err(|e| anyhow!("rpc read: {e}"))?, policy.max_output);
    let body: Value = serde_json::from_str(&text).unwrap_or(Value::String(text));
    if let Some(err) = body.get("error") {
        bail!("rpc {method} returned error: {err}");
    }
    Ok(json!({
        "capability": "rpc", "status": status, "method": method,
        "result": body.get("result").cloned().unwrap_or(Value::Null),
        "body": body,
    }))
}

// ── graphql ─────────────────────────────────────────────────────────

#[derive(Clone, Debug, Default)]
pub struct GraphqlCap {
    pub url: String,
    pub query: String,
    /// Extra variables merged with `with.variables`.
    pub variables: Value,
    pub timeout_ms: u64,
    /// Header carrying the auth token (value comes from a template/env).
    pub auth_header: String,
    pub auth_value: String,
}

pub fn call_graphql(c: &GraphqlCap, with: &Value, state: &Value, policy: &Policy) -> Result<Value> {
    let url = stringify(&expand(&Value::String(c.url.clone()), state, with));
    if url.is_empty() {
        bail!("graphql capability needs 'url'");
    }
    check_host(&url, policy)?;
    let query = if c.query.is_empty() {
        with.get("query").map(stringify).unwrap_or_default()
    } else {
        stringify(&expand(&Value::String(c.query.clone()), state, with))
    };
    if query.is_empty() {
        bail!("graphql capability needs 'query' (in config or 'with')");
    }
    let mut vars: Map<String, Value> = c
        .variables
        .as_object()
        .cloned()
        .unwrap_or_default();
    if let Some(extra) = with.get("variables").and_then(|v| v.as_object()) {
        for (k, v) in extra {
            vars.insert(k.clone(), expand(v, state, with));
        }
    }
    let body = json!({ "query": query, "variables": Value::Object(vars) });

    let agent = ureq::AgentBuilder::new().timeout(bounded_timeout(c.timeout_ms, policy)).build();
    let mut req = agent.post(&url).set("content-type", "application/json");
    if !c.auth_header.is_empty() {
        let v = stringify(&expand(&Value::String(c.auth_value.clone()), state, with));
        req = req.set(&c.auth_header, &v);
    }
    let resp = req.send_json(body).map_err(|e| anyhow!("graphql {url} failed: {e}"))?;
    let status = resp.status() as i64;
    let text = truncate(resp.into_string().map_err(|e| anyhow!("graphql read: {e}"))?, policy.max_output);
    let body: Value = serde_json::from_str(&text).unwrap_or(Value::String(text));
    if let Some(errors) = body.get("errors").and_then(|v| v.as_array()) {
        if !errors.is_empty() {
            bail!("graphql returned errors: {}", Value::Array(errors.clone()));
        }
    }
    Ok(json!({
        "capability": "graphql", "status": status,
        "data": body.get("data").cloned().unwrap_or(Value::Null),
        "body": body,
    }))
}

// ── llm (OpenAI-compatible) ─────────────────────────────────────────

#[derive(Clone, Debug, Default)]
pub struct LlmCap {
    pub url: String,
    pub model: String,
    pub system: String,
    pub max_tokens: u64,
    pub temperature: f64,
    pub timeout_ms: u64,
    pub auth_header: String,
    pub auth_value: String,
}

pub fn call_llm(c: &LlmCap, with: &Value, state: &Value, policy: &Policy) -> Result<Value> {
    let base = stringify(&expand(&Value::String(c.url.clone()), state, with));
    if base.is_empty() {
        bail!("llm capability needs 'url' (OpenAI-compatible base or full endpoint)");
    }
    let url = if base.ends_with("/chat/completions") {
        base
    } else {
        format!("{}/v1/chat/completions", base.trim_end_matches('/'))
    };
    check_host(&url, policy)?;
    let model = if c.model.is_empty() {
        with.get("model").map(stringify).unwrap_or_else(|| "default".to_string())
    } else {
        c.model.clone()
    };
    let prompt = with
        .get("prompt")
        .or_else(|| with.get("text"))
        .map(stringify)
        .ok_or_else(|| anyhow!("llm capability needs 'prompt' in 'with'"))?;
    let mut messages = Vec::new();
    if !c.system.is_empty() {
        messages.push(json!({"role": "system", "content": c.system}));
    }
    messages.push(json!({"role": "user", "content": prompt}));
    let body = json!({
        "model": model,
        "messages": messages,
        "max_tokens": if c.max_tokens == 0 { 256 } else { c.max_tokens },
        "temperature": c.temperature,
    });

    let agent = ureq::AgentBuilder::new().timeout(bounded_timeout(c.timeout_ms, policy)).build();
    let mut req = agent.post(&url).set("content-type", "application/json");
    if !c.auth_header.is_empty() {
        let v = stringify(&expand(&Value::String(c.auth_value.clone()), state, with));
        req = req.set(&c.auth_header, &v);
    }
    let resp = req.send_json(body).map_err(|e| anyhow!("llm {url} failed: {e}"))?;
    let status = resp.status() as i64;
    let text = truncate(resp.into_string().map_err(|e| anyhow!("llm read: {e}"))?, policy.max_output);
    let body: Value = serde_json::from_str(&text).unwrap_or(Value::String(text));
    let content = body
        .get("choices")
        .and_then(|c| c.get(0))
        .and_then(|c| c.get("message"))
        .and_then(|m| m.get("content"))
        .cloned()
        .unwrap_or(Value::Null);
    let usage = body.get("usage").cloned().unwrap_or(Value::Null);
    Ok(json!({
        "capability": "llm", "status": status, "model": model,
        "content": content, "usage": usage, "body": body,
    }))
}

// ── mcp ─────────────────────────────────────────────────────────────

#[derive(Clone, Debug, Default)]
pub struct McpCap {
    pub transport: String,
    pub url: Option<String>,
    pub command: Option<Vec<String>>,
    pub tool: String,
    pub timeout_ms: u64,
    /// Extra request headers for the `http` transport, expanded per call.
    ///
    /// Required for any authenticated MCP server (e.g. an `x-api-key` header);
    /// without it such a server was unreachable even though the spec could name
    /// its URL. Use `${secret.NAME}` for the value so credentials stay out of
    /// the spec.
    pub headers: serde_json::Map<String, Value>,
}

/// Parse an MCP HTTP response body.
///
/// Streamable-HTTP servers may reply either with a bare JSON object or with an
/// SSE frame (`event: message\ndata: {...}\n`). Both carry the same JSON-RPC
/// payload, so pull the `data:` line out when the body is not plain JSON.
fn parse_mcp_body(raw: &str) -> Value {
    if let Ok(v) = serde_json::from_str::<Value>(raw.trim()) {
        return v;
    }
    for line in raw.lines() {
        if let Some(rest) = line.strip_prefix("data:") {
            if let Ok(v) = serde_json::from_str::<Value>(rest.trim()) {
                return v;
            }
        }
    }
    Value::String(raw.to_string())
}

pub fn call_mcp(c: &McpCap, with: &Value, state: &Value, policy: &Policy) -> Result<Value> {
    let tool = if c.tool.is_empty() {
        with.get("tool").map(stringify).unwrap_or_default()
    } else {
        c.tool.clone()
    };
    if tool.is_empty() {
        bail!("mcp capability needs 'tool' (in config or 'with')");
    }
    let arguments = with.get("arguments").cloned().unwrap_or_else(|| json!({}));
    // MCP tools/call request, as sent over either transport.
    let request = json!({
        "jsonrpc": "2.0", "id": 1, "method": "tools/call",
        "params": { "name": tool, "arguments": arguments }
    });
    match c.transport.as_str() {
        "http" => {
            let url = stringify(&expand(
                &Value::String(c.url.clone().unwrap_or_default()),
                state,
                with,
            ));
            if url.is_empty() {
                bail!("mcp transport=http needs 'url'");
            }
            check_host(&url, policy)?;
            let agent = ureq::AgentBuilder::new().timeout(bounded_timeout(c.timeout_ms, policy)).build();

            // Streamable-HTTP MCP servers are stateful: they require an
            // `initialize` handshake and then every subsequent request must carry
            // the `Mcp-Session-Id` they hand back. Posting `tools/call` straight
            // away gets a 400/405 ("Missing required Mcp-Session-Id header"), so
            // do the handshake first. Servers that do not return a session id are
            // stateless and still work, so a missing header is not an error.
            let init = json!({
                "jsonrpc": "2.0", "id": 1, "method": "initialize",
                "params": {
                    "protocolVersion": "2024-11-05",
                    "capabilities": {},
                    "clientInfo": {"name": "laya-tch", "version": "1"}
                }
            });
            let mut session: Option<String> = None;
            let mut hreq = agent
                .post(&url)
                .set("content-type", "application/json")
                .set("accept", "application/json, text/event-stream");
            for (k, v) in &c.headers {
                hreq = hreq.set(k, &stringify(&expand(v, state, with)));
            }
            match hreq.send_json(init) {
                Ok(r) => {
                    if let Some(id) = r.header("mcp-session-id") {
                        if !id.is_empty() {
                            session = Some(id.to_string());
                        }
                    }
                }
                Err(ureq::Error::Status(code, _)) => {
                    // A server may reject `initialize` when it is stateless; fall
                    // through and try the call anyway rather than failing here.
                    if code >= 500 {
                        bail!("mcp initialize failed with status {code}");
                    }
                }
                Err(e) => return Err(anyhow!("mcp initialize {url} failed: {e}")),
            }

            let mut req = agent
                .post(&url)
                .set("content-type", "application/json")
                .set("accept", "application/json, text/event-stream");
            for (k, v) in &c.headers {
                req = req.set(k, &stringify(&expand(v, state, with)));
            }
            if let Some(id) = &session {
                req = req.set("Mcp-Session-Id", id);
            }
            let resp = req
                .send_json(request)
                .map_err(|e| anyhow!("mcp {url} failed: {e}"))?;
            let status = resp.status() as i64;
            let raw = resp.into_string().map_err(|e| anyhow!("mcp read: {e}"))?;
            // A streamable-HTTP server may answer with an SSE frame rather than a
            // bare JSON body; unwrap the `data:` line before parsing.
            let body = parse_mcp_body(&raw);
            let text = truncate(raw, policy.max_output);
            if let Some(err) = body.get("error") {
                bail!("mcp tool {tool} returned error: {err}");
            }
            Ok(json!({
                "capability": "mcp", "transport": "http", "status": status, "tool": tool,
                "session": session.is_some(),
                "result": body.get("result").cloned().unwrap_or(Value::Null), "body": body,
                "raw": text,
            }))
        }
        "stdio" => {
            if !policy.allow_exec {
                bail!("mcp transport=stdio spawns a process; set policy.allow_exec = true to enable");
            }
            let cmd = c
                .command
                .as_ref()
                .ok_or_else(|| anyhow!("mcp transport=stdio needs 'command'"))?;
            let argv: Vec<String> = cmd
                .iter()
                .map(|a| stringify(&expand(&Value::String(a.clone()), state, with)))
                .collect();
            let reply = super::stdio_json_rpc(&argv, &request, c.timeout_ms, policy)?;
            if let Some(err) = reply.get("error") {
                bail!("mcp tool {tool} returned error: {err}");
            }
            Ok(json!({
                "capability": "mcp", "transport": "stdio", "tool": tool,
                "result": reply.get("result").cloned().unwrap_or(Value::Null), "body": reply,
            }))
        }
        other => bail!("mcp transport {other:?} unsupported (http | stdio)"),
    }
}

// ── vector ──────────────────────────────────────────────────────────

#[derive(Clone, Debug, Default)]
pub struct VectorCap {
    pub url: String,
    /// `upsert` | `search`
    pub op: String,
    pub collection: String,
    pub timeout_ms: u64,
}

pub fn call_vector(c: &VectorCap, with: &Value, state: &Value, policy: &Policy) -> Result<Value> {
    let url = stringify(&expand(&Value::String(c.url.clone()), state, with));
    if url.is_empty() {
        bail!("vector capability needs 'url'");
    }
    check_host(&url, policy)?;
    let op = if c.op.is_empty() { "search" } else { c.op.as_str() };
    let collection = stringify(&expand(&Value::String(c.collection.clone()), state, with));
    let body = match op {
        "search" => {
            let vector = with
                .get("vector")
                .cloned()
                .ok_or_else(|| anyhow!("vector.search needs 'vector' in 'with'"))?;
            json!({ "collection": collection, "vector": vector,
                    "top_k": with.get("top_k").cloned().unwrap_or(json!(5)) })
        }
        "upsert" => {
            let items = with
                .get("items")
                .cloned()
                .ok_or_else(|| anyhow!("vector.upsert needs 'items' in 'with'"))?;
            json!({ "collection": collection, "items": items })
        }
        other => bail!("vector op {other:?} unsupported (upsert | search)"),
    };
    let agent = ureq::AgentBuilder::new().timeout(bounded_timeout(c.timeout_ms, policy)).build();
    let resp = agent
        .post(&url)
        .set("content-type", "application/json")
        .send_json(body)
        .map_err(|e| anyhow!("vector {url} failed: {e}"))?;
    let status = resp.status() as i64;
    let text = truncate(resp.into_string().map_err(|e| anyhow!("vector read: {e}"))?, policy.max_output);
    let body: Value = serde_json::from_str(&text).unwrap_or(Value::String(text));
    Ok(json!({
        "capability": "vector", "op": op, "collection": collection,
        "status": status, "hits": body.get("hits").cloned().unwrap_or(Value::Null), "body": body,
    }))
}

// ── webhook (signed notify with retry) ──────────────────────────────

#[derive(Clone, Debug, Default)]
pub struct WebhookCap {
    pub url: String,
    pub timeout_ms: u64,
    /// Header used for the signature, e.g. "x-signature".
    pub sign_header: String,
    /// Secret template (use `${env.…}`); signs `timestamp.body`.
    pub sign_secret: String,
}

pub fn call_webhook(c: &WebhookCap, with: &Value, state: &Value, policy: &Policy) -> Result<Value> {
    let url = stringify(&expand(&Value::String(c.url.clone()), state, with));
    if url.is_empty() {
        bail!("webhook capability needs 'url'");
    }
    check_host(&url, policy)?;
    let payload = with.get("payload").cloned().unwrap_or_else(|| json!({}));
    let event = with.get("event").map(stringify).unwrap_or_else(|| "notify".to_string());
    let ts = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    let body = json!({ "event": event, "timestamp": ts, "payload": payload });
    let body_text = serde_json::to_string(&body)?;

    let agent = ureq::AgentBuilder::new().timeout(bounded_timeout(c.timeout_ms, policy)).build();
    let attempts = policy.retries + 1;
    let mut last_err = String::new();
    for attempt in 0..attempts {
        let mut req = agent.post(&url).set("content-type", "application/json");
        if !c.sign_header.is_empty() && !c.sign_secret.is_empty() {
            let secret = stringify(&expand(&Value::String(c.sign_secret.clone()), state, with));
            let sig = hmac_sha256_hex(secret.as_bytes(), format!("{ts}.{body_text}").as_bytes());
            req = req.set(&c.sign_header, &format!("t={ts},v1={sig}"));
        }
        match req.send_string(&body_text) {
            Ok(resp) => {
                let status = resp.status() as i64;
                let text = truncate(resp.into_string().unwrap_or_default(), policy.max_output);
                return Ok(json!({
                    "capability": "webhook", "event": event, "status": status,
                    "attempts": attempt + 1, "signed": !c.sign_header.is_empty(), "body": text,
                }));
            }
            Err(e) => {
                last_err = e.to_string();
                if attempt + 1 < attempts {
                    std::thread::sleep(Duration::from_millis(50 * (attempt as u64 + 1)));
                }
            }
        }
    }
    bail!("webhook {url} failed after {attempts} attempt(s): {last_err}")
}

/// Minimal HMAC-SHA256 (no external crate) for webhook signing.
fn hmac_sha256_hex(key: &[u8], msg: &[u8]) -> String {
    const BLOCK: usize = 64;
    let mut k = [0u8; BLOCK];
    if key.len() > BLOCK {
        let d = sha256(key);
        k[..32].copy_from_slice(&d);
    } else {
        k[..key.len()].copy_from_slice(key);
    }
    let mut ipad = [0x36u8; BLOCK];
    let mut opad = [0x5cu8; BLOCK];
    for i in 0..BLOCK {
        ipad[i] ^= k[i];
        opad[i] ^= k[i];
    }
    let mut inner = Vec::with_capacity(BLOCK + msg.len());
    inner.extend_from_slice(&ipad);
    inner.extend_from_slice(msg);
    let ih = sha256(&inner);
    let mut outer = Vec::with_capacity(BLOCK + 32);
    outer.extend_from_slice(&opad);
    outer.extend_from_slice(&ih);
    sha256(&outer).iter().map(|b| format!("{b:02x}")).collect()
}

pub fn sha256_hex(data: &[u8]) -> String {
    sha256(data).iter().map(|b| format!("{b:02x}")).collect()
}

fn sha256(data: &[u8]) -> [u8; 32] {
    const K: [u32; 64] = [
        0x428a2f98, 0x71374491, 0xb5c0fbcf, 0xe9b5dba5, 0x3956c25b, 0x59f111f1, 0x923f82a4,
        0xab1c5ed5, 0xd807aa98, 0x12835b01, 0x243185be, 0x550c7dc3, 0x72be5d74, 0x80deb1fe,
        0x9bdc06a7, 0xc19bf174, 0xe49b69c1, 0xefbe4786, 0x0fc19dc6, 0x240ca1cc, 0x2de92c6f,
        0x4a7484aa, 0x5cb0a9dc, 0x76f988da, 0x983e5152, 0xa831c66d, 0xb00327c8, 0xbf597fc7,
        0xc6e00bf3, 0xd5a79147, 0x06ca6351, 0x14292967, 0x27b70a85, 0x2e1b2138, 0x4d2c6dfc,
        0x53380d13, 0x650a7354, 0x766a0abb, 0x81c2c92e, 0x92722c85, 0xa2bfe8a1, 0xa81a664b,
        0xc24b8b70, 0xc76c51a3, 0xd192e819, 0xd6990624, 0xf40e3585, 0x106aa070, 0x19a4c116,
        0x1e376c08, 0x2748774c, 0x34b0bcb5, 0x391c0cb3, 0x4ed8aa4a, 0x5b9cca4f, 0x682e6ff3,
        0x748f82ee, 0x78a5636f, 0x84c87814, 0x8cc70208, 0x90befffa, 0xa4506ceb, 0xbef9a3f7,
        0xc67178f2,
    ];
    let mut h: [u32; 8] = [
        0x6a09e667, 0xbb67ae85, 0x3c6ef372, 0xa54ff53a, 0x510e527f, 0x9b05688c, 0x1f83d9ab,
        0x5be0cd19,
    ];
    let mut msg = data.to_vec();
    let bitlen = (data.len() as u64) * 8;
    msg.push(0x80);
    while msg.len() % 64 != 56 {
        msg.push(0);
    }
    msg.extend_from_slice(&bitlen.to_be_bytes());

    for chunk in msg.chunks(64) {
        let mut w = [0u32; 64];
        for i in 0..16 {
            w[i] = u32::from_be_bytes([chunk[i * 4], chunk[i * 4 + 1], chunk[i * 4 + 2], chunk[i * 4 + 3]]);
        }
        for i in 16..64 {
            let s0 = w[i - 15].rotate_right(7) ^ w[i - 15].rotate_right(18) ^ (w[i - 15] >> 3);
            let s1 = w[i - 2].rotate_right(17) ^ w[i - 2].rotate_right(19) ^ (w[i - 2] >> 10);
            w[i] = w[i - 16]
                .wrapping_add(s0)
                .wrapping_add(w[i - 7])
                .wrapping_add(s1);
        }
        let (mut a, mut b, mut c, mut d, mut e, mut f, mut g, mut hh) =
            (h[0], h[1], h[2], h[3], h[4], h[5], h[6], h[7]);
        for i in 0..64 {
            let s1 = e.rotate_right(6) ^ e.rotate_right(11) ^ e.rotate_right(25);
            let ch = (e & f) ^ ((!e) & g);
            let t1 = hh
                .wrapping_add(s1)
                .wrapping_add(ch)
                .wrapping_add(K[i])
                .wrapping_add(w[i]);
            let s0 = a.rotate_right(2) ^ a.rotate_right(13) ^ a.rotate_right(22);
            let maj = (a & b) ^ (a & c) ^ (b & c);
            let t2 = s0.wrapping_add(maj);
            hh = g; g = f; f = e; e = d.wrapping_add(t1);
            d = c; c = b; b = a; a = t1.wrapping_add(t2);
        }
        h[0] = h[0].wrapping_add(a);
        h[1] = h[1].wrapping_add(b);
        h[2] = h[2].wrapping_add(c);
        h[3] = h[3].wrapping_add(d);
        h[4] = h[4].wrapping_add(e);
        h[5] = h[5].wrapping_add(f);
        h[6] = h[6].wrapping_add(g);
        h[7] = h[7].wrapping_add(hh);
    }
    let mut out = [0u8; 32];
    for (i, v) in h.iter().enumerate() {
        out[i * 4..i * 4 + 4].copy_from_slice(&v.to_be_bytes());
    }
    out
}

// ── sse ─────────────────────────────────────────────────────────────

#[derive(Clone, Debug, Default)]
pub struct SseCap {
    pub url: String,
    pub timeout_ms: u64,
    /// Stop after this many events (0 ⇒ until timeout/EOF).
    pub max_events: usize,
}

pub fn call_sse(c: &SseCap, with: &Value, state: &Value, policy: &Policy) -> Result<Value> {
    use std::io::{BufRead, BufReader};
    let url = stringify(&expand(&Value::String(c.url.clone()), state, with));
    if url.is_empty() {
        bail!("sse capability needs 'url'");
    }
    check_host(&url, policy)?;
    let agent = ureq::AgentBuilder::new().timeout(bounded_timeout(c.timeout_ms, policy)).build();
    let resp = agent
        .get(&url)
        .set("accept", "text/event-stream")
        .call()
        .map_err(|e| anyhow!("sse {url} failed: {e}"))?;
    let status = resp.status() as i64;
    let reader = BufReader::new(resp.into_reader());
    let max_events = if c.max_events == 0 { 100 } else { c.max_events };
    let mut events: Vec<Value> = Vec::new();
    let mut cur_data = String::new();
    let mut cur_event = String::new();
    let mut total = 0usize;
    for line in reader.lines() {
        let line = line.map_err(|e| anyhow!("sse read: {e}"))?;
        total += line.len() + 1;
        if total > policy.max_output {
            break;
        }
        if let Some(rest) = line.strip_prefix("event:") {
            cur_event = rest.trim().to_string();
        } else if let Some(rest) = line.strip_prefix("data:") {
            if !cur_data.is_empty() {
                cur_data.push('\n');
            }
            cur_data.push_str(rest.trim());
        } else if line.trim().is_empty() && !cur_data.is_empty() {
            events.push(json!({ "event": cur_event, "data": cur_data }));
            cur_data.clear();
            cur_event.clear();
            if events.len() >= max_events {
                break;
            }
        }
    }
    if !cur_data.is_empty() && events.len() < max_events {
        events.push(json!({ "event": cur_event, "data": cur_data }));
    }
    let count = events.len();
    Ok(json!({
        "capability": "sse", "status": status, "events": events,
        "count": count, "truncated": count >= max_events,
    }))
}
