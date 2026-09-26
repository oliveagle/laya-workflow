//! HTTP-service and tool-backed capabilities.
//!
//! * `s3`         — object storage: put / get / list / delete (S3-compatible HTTP)
//! * `prometheus` — scrape + parse the text exposition format; optional query
//! * `kafka`      — Kafka REST proxy (Confluent-style) produce/topics
//! * `pdf`        — text extraction via `pdftotext` (or plain-text passthrough)
//! * `sql`        — generic SQL over a CLI (psql / mysql / sqlite3)
//!
//! Network kinds are egress-gated by `policy.allow_hosts`; the CLI-backed kinds
//! (`pdf`, `sql`) require `policy.allow_exec`, and any file paths they touch are
//! checked against `policy.allow_paths`.

use anyhow::{anyhow, bail, Result};
use serde_json::{json, Value};

use super::{bounded_timeout, check_host, expand, host_of, stringify, truncate, ExecCap, Policy, effective_op};

// ── s3 ──────────────────────────────────────────────────────────────

#[derive(Clone, Debug, Default)]
pub struct S3Cap {
    /// Endpoint root, e.g. http://127.0.0.1:9000
    pub endpoint: String,
    pub bucket: String,
    /// Optional key prefix used by `list`.
    pub prefix: String,
    pub timeout_ms: u64,
    /// Auth headers, values expanded per call (use ${env.…}).
    pub headers: serde_json::Map<String, Value>,
}

pub fn call_s3(c: &S3Cap, with: &Value, state: &Value, policy: &Policy) -> Result<Value> {
    let endpoint = stringify(&expand(&Value::String(c.endpoint.clone()), state, with));
    if endpoint.is_empty() {
        bail!("s3 needs 'endpoint'");
    }
    let bucket = stringify(&expand(&Value::String(c.bucket.clone()), state, with));
    if bucket.is_empty() {
        bail!("s3 needs 'bucket'");
    }
    check_host(&endpoint, policy)?;
    let key = with.get("key").map(stringify).unwrap_or_default();
    let op = if c.prefix.is_empty() && key.is_empty() {
        "list".to_string()
    } else if key.is_empty() {
        "list".to_string()
    } else {
        with.get("op").map(stringify).unwrap_or_else(|| "get".to_string())
    };
    // Two S3 addressing styles exist and the endpoint tells us which one to use:
    //   * path-style         http://127.0.0.1:9000        + /bucket/key
    //   * virtual-host-style https://<bucket>.s3.<reg>... + /key
    // Appending the bucket unconditionally broke the second: an AWS-style
    // endpoint already carries the bucket in its host, so the URL became
    // `https://bucket.s3.amazonaws.com/bucket/` and every request 404'd.
    let endpoint_trimmed = endpoint.trim_end_matches('/');
    let bucket_in_host = host_of(endpoint_trimmed).starts_with(&format!("{bucket}."));
    let base = if bucket_in_host {
        endpoint_trimmed.to_string()
    } else {
        format!("{endpoint_trimmed}/{bucket}")
    };
    let url = match op.as_str() {
        "list" => {
            // `?list-type=2` makes AWS-compatible endpoints answer with XML;
            // a bare prefix also works on MinIO.
            let mut q: Vec<String> = Vec::new();
            if !c.prefix.is_empty() {
                q.push(format!("prefix={}", c.prefix));
            }
            if bucket_in_host {
                q.insert(0, "list-type=2".to_string());
            }
            if q.is_empty() {
                format!("{base}/")
            } else {
                format!("{base}/?{}", q.join("&"))
            }
        }
        _ => format!("{base}/{key}"),
    };
    check_host(&url, policy)?;

    let agent = ureq::AgentBuilder::new().timeout(bounded_timeout(if c.timeout_ms == 0 { 10_000 } else { c.timeout_ms }, policy)).build();
    let mut req = match op.as_str() {
        "put" => agent.put(&url),
        "delete" => agent.delete(&url),
        _ => agent.get(&url),
    };
    for (k, v) in &c.headers {
        req = req.set(k, &stringify(&expand(v, state, with)));
    }
    let resp = match op.as_str() {
        "put" => {
            let body = with.get("body").map(stringify).unwrap_or_default();
            req.send_string(&body)
        }
        _ => req.call(),
    };
    let resp = resp.map_err(|e| anyhow!("s3 {op} {url} failed: {e}"))?;
    let status = resp.status() as i64;
    let text = truncate(resp.into_string().unwrap_or_default(), policy.max_output);
    let keys: Vec<String> = if op == "list" {
        let re = regex_lite::Regex::new(r"<Key>([^<]+)</Key>").map_err(|e| anyhow!("{e}"))?;
        re.captures_iter(&text).filter_map(|c| c.get(1).map(|m| m.as_str().to_string())).collect()
    } else {
        Vec::new()
    };
    Ok(json!({
        "capability": "s3", "op": op, "bucket": bucket, "key": key,
        "status": status, "keys": keys, "count": keys.len(),
        "body": if op == "get" { json!(text) } else { Value::Null },
        "raw_len": text.len(),
    }))
}

// ── prometheus ──────────────────────────────────────────────────────

#[derive(Clone, Debug, Default)]
pub struct PrometheusCap {
    pub url: String,
    pub timeout_ms: u64,
}

pub fn call_prometheus(c: &PrometheusCap, with: &Value, state: &Value, policy: &Policy) -> Result<Value> {
    let url = stringify(&expand(&Value::String(c.url.clone()), state, with));
    if url.is_empty() {
        bail!("prometheus needs 'url' (the /metrics or /api/v1/query endpoint)");
    }
    check_host(&url, policy)?;
    let agent = ureq::AgentBuilder::new()
        .timeout(bounded_timeout(if c.timeout_ms == 0 { 8000 } else { c.timeout_ms }, policy))
        .build();

    // Query API mode when a query is provided, else scrape the text endpoint.
    let query = with.get("query").map(stringify).unwrap_or_default();
    if !query.is_empty() {
        let sep = if url.contains('?') { '&' } else { '?' };
        let qurl = format!("{url}{sep}query={}", urlencode(&query));
        let resp = agent.get(&qurl).call().map_err(|e| anyhow!("prometheus query failed: {e}"))?;
        let status = resp.status() as i64;
        let text = truncate(resp.into_string().unwrap_or_default(), policy.max_output);
        let body: Value = serde_json::from_str(&text).unwrap_or(Value::String(text));
        return Ok(json!({ "capability": "prometheus", "mode": "query", "status": status, "query": query, "body": body }));
    }

    let resp = agent.get(&url).call().map_err(|e| anyhow!("prometheus scrape failed: {e}"))?;
    let status = resp.status() as i64;
    let text = truncate(resp.into_string().unwrap_or_default(), policy.max_output);
    let mut samples = Vec::new();
    for line in text.lines() {
        let l = line.trim();
        if l.is_empty() || l.starts_with('#') {
            continue;
        }
        if let Some((name, val)) = l.rsplit_once(' ') {
            // Prometheus exposition format permits `+Inf`, `-Inf` and `NaN`, and
            // Rust's f64 parser accepts all of them. serde_json then serialises a
            // non-finite float as `null`, so the caller would read a *silent*
            // wrong value. Report the value in a parseable form instead of
            // letting it collapse to null.
            if let Ok(v) = val.trim().parse::<f64>() {
                let value = if v.is_finite() {
                    json!(v)
                } else {
                    json!(val.trim()) // keep the literal: "NaN" / "+Inf" / "-Inf"
                };
                samples.push(json!({ "metric": name.trim(), "value": value }));
            }
        }
    }
    let count = samples.len();
    Ok(json!({
        "capability": "prometheus", "mode": "scrape", "status": status,
        "samples": samples, "count": count,
    }))
}

fn urlencode(s: &str) -> String {
    s.bytes()
        .map(|b| match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => (b as char).to_string(),
            b' ' => "%20".to_string(),
            other => format!("%{other:02X}"),
        })
        .collect()
}

// ── kafka (REST proxy) ──────────────────────────────────────────────

#[derive(Clone, Debug, Default)]
pub struct KafkaCap {
    /// Kafka REST proxy root, e.g. http://127.0.0.1:8082
    pub url: String,
    pub topic: String,
    pub timeout_ms: u64,
}

pub fn call_kafka(c: &KafkaCap, with: &Value, state: &Value, policy: &Policy) -> Result<Value> {
    let url = stringify(&expand(&Value::String(c.url.clone()), state, with));
    if url.is_empty() {
        bail!("kafka needs 'url' (the REST proxy)");
    }
    check_host(&url, policy)?;
    let topic = if c.topic.is_empty() { with.get("topic").map(stringify).unwrap_or_default() } else { c.topic.clone() };
    if topic.is_empty() {
        bail!("kafka needs 'topic'");
    }
    let value = with.get("value").cloned().unwrap_or_else(|| json!({}));
    let body = json!({ "records": [{ "value": value }] });
    let agent = ureq::AgentBuilder::new()
        .timeout(bounded_timeout(if c.timeout_ms == 0 { 10_000 } else { c.timeout_ms }, policy))
        .build();
    let url = format!("{}/topics/{}", url.trim_end_matches('/'), topic);
    let resp = agent
        .post(&url)
        .set("content-type", "application/vnd.kafka.json.v2+json")
        .send_json(body)
        .map_err(|e| anyhow!("kafka produce {url} failed: {e}"))?;
    let status = resp.status() as i64;
    let text = truncate(resp.into_string().unwrap_or_default(), policy.max_output);
    let parsed: Value = serde_json::from_str(&text).unwrap_or(Value::String(text));
    Ok(json!({
        "capability": "kafka", "topic": topic, "status": status,
        "offsets": parsed.get("offsets").cloned().unwrap_or(Value::Null), "body": parsed,
    }))
}

// ── pdf / sql (CLI-backed) ──────────────────────────────────────────

#[derive(Clone, Debug, Default)]
pub struct PdfCap {
    pub path: String,
    /// `text` (pdftotext) | `info` (pdfinfo)
    pub op: String,
    pub timeout_ms: u64,
}

pub fn call_pdf(c: &PdfCap, with: &Value, state: &Value, policy: &Policy) -> Result<Value> {
    if !policy.allow_exec {
        bail!("pdf shells out to pdftotext/pdfinfo; set policy.allow_exec = true to enable");
    }
    let path = stringify(&expand(
        &Value::String(if c.path.is_empty() { with.get("path").map(stringify).unwrap_or_default() } else { c.path.clone() }),
        state,
        with,
    ));
    if path.is_empty() {
        bail!("pdf needs 'path'");
    }
    super::store::resolve_store_path(&path, policy)?;
    let op = effective_op(&c.op, with, "text");
    let argv = match op.as_str() {
        "info" => vec!["pdfinfo".to_string(), path.clone()],
        _ => vec!["pdftotext".to_string(), "-layout".to_string(), path.clone(), "-".to_string()],
    };
    let cap = ExecCap {
        argv,
        cwd: None,
        env: serde_json::Map::new(),
        timeout_ms: if c.timeout_ms == 0 { 30_000 } else { c.timeout_ms },
        max_output: policy.max_output,
    };
    let out = super::call_exec(&cap, &json!({}), &json!({}), policy)?;
    let stdout = out["stdout"].as_str().unwrap_or("").to_string();
    let pages = stdout.matches('\u{0c}').count();
    Ok(json!({
        "capability": "pdf", "op": c.op, "path": path,
        "exit_code": out["exit_code"], "text": stdout.clone(),
        "chars": stdout.chars().count(), "pages": pages,
        "stderr": out["stderr"],
    }))
}

#[derive(Clone, Debug, Default)]
pub struct SqlCap {
    /// `sqlite3` | `psql` | `mysql`
    pub driver: String,
    /// Connection string / database path.
    pub dsn: String,
    pub timeout_ms: u64,
}

pub fn call_sql(c: &SqlCap, with: &Value, state: &Value, policy: &Policy) -> Result<Value> {
    if !policy.allow_exec {
        bail!("sql shells out to a database CLI; set policy.allow_exec = true to enable");
    }
    let dsn = stringify(&expand(&Value::String(c.dsn.clone()), state, with));
    if dsn.is_empty() {
        bail!("sql needs 'dsn'");
    }
    let sql = with.get("sql").map(stringify).ok_or_else(|| anyhow!("sql needs 'sql'"))?;
    let sql = stringify(&expand(&Value::String(sql), state, with));
    let driver = if c.driver.is_empty() { "sqlite3".to_string() } else { c.driver.clone() };
    let argv: Vec<String> = match driver.as_str() {
        "sqlite3" => {
            super::store::resolve_store_path(&dsn, policy)?;
            vec!["sqlite3".into(), "-json".into(), dsn.clone(), sql.clone()]
        }
        "psql" => vec!["psql".into(), dsn.clone(), "-c".into(), sql.clone(), "--no-align".into(), "--tuples-only".into()],
        "mysql" => vec!["mysql".into(), dsn.clone(), "-e".into(), sql.clone()],
        other => bail!("sql driver {other:?} unsupported (sqlite3 | psql | mysql)"),
    };
    let cap = ExecCap {
        argv,
        cwd: None,
        env: serde_json::Map::new(),
        timeout_ms: if c.timeout_ms == 0 { 30_000 } else { c.timeout_ms },
        max_output: policy.max_output,
    };
    let out = super::call_exec(&cap, &json!({}), &json!({}), policy)?;
    let stdout = out["stdout"].as_str().unwrap_or("").to_string();
    let parsed: Value = serde_json::from_str(&stdout).unwrap_or(Value::Null);
    Ok(json!({
        "capability": "sql", "driver": driver, "exit_code": out["exit_code"],
        "rows": parsed, "raw": stdout, "stderr": out["stderr"],
    }))
}
