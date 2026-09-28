//! External capabilities ("tools") for Laya workflows.
//!
//! A *capability* is any external effect a workflow node can invoke — an HTTP
//! call, a local command, or a session-oriented agent/app-server. Capabilities
//! are declared as **data** in the workflow spec, so wiring a new endpoint or
//! swapping a protocol needs no Rust changes:
//!
//! ```jsonc
//! {
//!   "capabilities": {
//!     "fraud_check": { "kind": "http", "method": "POST",
//!                      "url": "http://127.0.0.1:9j/score",
//!                      "headers": { "x-api-key": "${env.FRAUD_KEY}" },
//!                      "body": { "text": "${state.text}" },
//!                      "timeout_ms": 2000 }
//!   },
//!   "nodes": [ { … "action": { "kind": "call", "capability": "fraud_check" } } ]
//! }
//! ```
//!
//! Templates: `"${state.<dotted.path>}"` pulls from the workflow state and
//! `"${env.<NAME>}"` from the environment (never hard-code secrets in a spec).
//!
//! Safety (see `Policy`): capabilities are **deny-by-default**; `exec` requires
//! an explicit opt-in, hosts must pass the allow-list, and every call is bounded
//! by a timeout and an output-size cap.

pub mod data;
pub mod goal;
pub mod local;
pub mod math;
pub mod net;
mod parse;
pub mod proto;
pub mod secret;
pub mod service;
pub mod store;
pub mod sys;
pub mod web;

use anyhow::{anyhow, bail, Result};
use serde_json::{json, Map, Value};
use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;

/// One resolved capability call target.
#[derive(Clone, Debug)]
pub enum Capability {
    Http(HttpCap),
    Exec(ExecCap),
    /// Session-oriented JSON request/response agent (codex app-server style).
    Agent(AgentCap),
    // ── local / pure ──
    Datetime(local::DatetimeCap),
    Text(local::TextCap),
    File(local::FileCap),
    Sqlite(local::SqliteCap),
    Shell(local::ShellCap),
    // ── network ──
    Rpc(net::RpcCap),
    Graphql(net::GraphqlCap),
    Llm(net::LlmCap),
    Mcp(net::McpCap),
    Vector(net::VectorCap),
    Webhook(net::WebhookCap),
    Sse(net::SseCap),
    /// No-op capability: returns the expanded `with` payload (useful as a
    /// placeholder and for testing the wiring without side effects).
    Passthrough,
    // ── structure / data (pure) ──
    Json(data::JsonCap),
    Csv(data::CsvCap),
    Xml(data::XmlCap),
    Markdown(data::MarkdownCap),
    Diff(data::DiffCap),
    Validate(data::ValidateCap),
    Math(data::MathCap),
    Hash(data::HashCap),
    Graph(data::GraphCap),
    Tokenize(data::TokenizeCap),
    Cron(data::CronCap),
    // ── stateful local stores (path-scoped) ──
    KeyValue(store::KeyValueCap),
    Cache(store::CacheCap),
    Queue(store::QueueCap),
    // ── host / system ──
    Metrics(sys::MetricsCap),
    NotifyLocal(sys::NotifyLocalCap),
    // ── raw socket / plaintext protocols ──
    Tcp(proto::TcpCap),
    Udp(proto::UdpCap),
    Redis(proto::RedisCap),
    Nats(proto::NatsCap),
    Mqtt(proto::MqttCap),
    Smtp(proto::SmtpCap),
    Archive(proto::ArchiveCap),
    // ── HTTP services / CLI-backed tools ──
    S3(service::S3Cap),
    Prometheus(service::PrometheusCap),
    Kafka(service::KafkaCap),
    Pdf(service::PdfCap),
    Sql(service::SqlCap),
    // ── web research ──
    /// Query a search endpoint and return structured results.
    WebSearch(web::WebSearchCap),
    /// Fetch a URL and return readable text / markdown / raw body.
    WebFetch(web::WebFetchCap),
    // ── external agent harnesses ──
    /// Run an external agent's goal loop (`cxgo` / `cmdgo`) on a target doc.
    GoalRunner(goal::GoalRunnerCap),
}

#[derive(Clone, Debug, Default)]
pub struct HttpCap {
    pub method: String,
    pub url: String,
    pub headers: Map<String, Value>,
    /// Body template: object (JSON) or string (raw), expanded per call.
    pub body: Option<Value>,
    pub timeout_ms: u64,
    pub expect_json: bool,
}

#[derive(Clone, Debug, Default)]
pub struct ExecCap {
    pub argv: Vec<String>,
    pub cwd: Option<String>,
    pub env: Map<String, Value>,
    pub timeout_ms: u64,
    pub max_output: usize,
}

/// Agent/app-server capability.
///
/// Two transports are supported, both JSON in / JSON out per call:
///   * `http`  — POST the request object, read a JSON response
///   * `stdio` — spawn a long-lived process and speak line-delimited JSON-RPC
///               (the codex app-server shape: one JSON object per line on
///               stdin, one JSON object per line on stdout)
#[derive(Clone, Debug, Default)]
pub struct AgentCap {
    pub transport: String,
    pub url: Option<String>,
    pub command: Option<Vec<String>>,
    pub session: Option<String>,
    pub timeout_ms: u64,
    pub max_output: usize,
}

/// Runtime safety envelope, overridable from the spec (with hard ceilings).
#[derive(Clone, Debug)]
pub struct Policy {
    /// Whether `exec` capabilities may run at all (default: off).
    pub allow_exec: bool,
    /// Host allow-list for `http`/`agent-http` (empty ⇒ any host allowed).
    pub allow_hosts: Vec<String>,
    /// Path allow-list for `file` capabilities (empty ⇒ file access denied).
    pub allow_paths: Vec<String>,
    /// Hard ceiling for any capability timeout.
    pub max_timeout_ms: u64,
    /// Hard ceiling for captured output bytes.
    pub max_output: usize,
    /// Max retries on transport errors (0 ⇒ single attempt).
    pub retries: usize,
}

impl Default for Policy {
    fn default() -> Self {
        Self {
            allow_exec: false,
            allow_hosts: Vec::new(),
            allow_paths: Vec::new(),
            max_timeout_ms: 60_000,
            max_output: 1 << 20, // 1 MiB
            retries: 0,
        }
    }
}

/// Registry of named capabilities, built from a spec's `"capabilities"` block.
#[derive(Clone, Default)]
pub struct Registry {
    caps: HashMap<String, Capability>,
    policy: Policy,
}

impl Registry {
    /// Parse `"capabilities"` (and optional `"policy"`) out of a spec.
    pub fn from_spec(spec: &Value) -> Result<Self> {
        let mut policy = Policy::default();
        if let Some(p) = spec.get("policy") {
            if let Some(v) = p.get("allow_exec").and_then(|v| v.as_bool()) {
                policy.allow_exec = v;
            }
            if let Some(a) = p.get("allow_hosts").and_then(|v| v.as_array()) {
                policy.allow_hosts = a
                    .iter()
                    .filter_map(|x| x.as_str().map(str::to_string))
                    .collect();
            }
            if let Some(a) = p.get("allow_paths").and_then(|v| v.as_array()) {
                // allow_paths may use ${env.NAME} / ${secret.NAME} (paths must
                // not be hard-coded in a spec); they are resolved once at load
                // time, and a missing reference is an error here rather than an
                // empty string that silently redirects writes elsewhere.
                let mut paths = Vec::with_capacity(a.len());
                for x in a.iter().filter_map(|x| x.as_str()) {
                    let resolved = try_expand_env_only(x).map_err(|e| {
                        anyhow!("policy.allow_paths entry {x:?} could not be resolved: {e}")
                    })?;
                    paths.push(resolved);
                }
                policy.allow_paths = paths;
            }
            if let Some(v) = p.get("max_timeout_ms").and_then(|v| v.as_u64()) {
                policy.max_timeout_ms = v.min(600_000);
            }
            if let Some(v) = p.get("max_output").and_then(|v| v.as_u64()) {
                policy.max_output = (v as usize).min(64 << 20);
            }
            if let Some(v) = p.get("retries").and_then(|v| v.as_u64()) {
                policy.retries = (v as usize).min(5);
            }
        }

        let mut caps = HashMap::new();
        if let Some(obj) = spec.get("capabilities").and_then(|v| v.as_object()) {
            for (name, def) in obj {
                caps.insert(name.clone(), parse::parse_cap(name, def)?);
            }
        }
        Ok(Self { caps, policy })
    }

    pub fn policy(&self) -> &Policy {
        &self.policy
    }

    pub fn names(&self) -> Vec<String> {
        let mut v: Vec<String> = self.caps.keys().cloned().collect();
        v.sort();
        v
    }

    pub fn get(&self, name: &str) -> Result<&Capability> {
        self.caps
            .get(name)
            .ok_or_else(|| anyhow!("capability {name:?} is not declared in this spec"))
    }

    /// Invoke a capability with `with` arguments plus the current workflow state.
    pub fn call(&self, name: &str, with: &Value, state: &Value) -> Result<Value> {
        let cap = self.get(name)?.clone();
        // Fail closed when any string the spec will put on the wire still carries
        // an unresolved `secret.`/`env.` reference. Without this a literal like
        // `"Bearer null"` (or the sentinel) reaches the request, i.e. the call is
        // made with the wrong credential instead of erroring.
        for s in unresolved_in(&cap) {
            bail!("capability {name:?} has an unresolved reference: {s}");
        }
        if let Value::Object(o) = with {
            for (k, v) in o {
                if let Some(s) = v.as_str() {
                    if has_unresolved(s) {
                        bail!(
                            "capability {name:?} argument {k:?} has an unresolved reference: {s}"
                        );
                    }
                }
            }
        }
        let attempts = self.policy.retries + 1;
        let mut last: Option<anyhow::Error> = None;
        for _ in 0..attempts {
            let r = match &cap {
                Capability::Http(c) => call_http(c, with, state, &self.policy),
                Capability::Exec(c) => call_exec(c, with, state, &self.policy),
                Capability::Agent(c) => call_agent(c, with, state, &self.policy),
                Capability::Datetime(c) => local::call_datetime(c, with, state),
                Capability::Text(c) => local::call_text(c, with, state),
                Capability::File(c) => local::call_file(c, with, state, &self.policy),
                Capability::Sqlite(c) => local::call_sqlite(c, with, state, &self.policy),
                Capability::Shell(c) => local::call_shell(c, with, state, &self.policy),
                Capability::Rpc(c) => net::call_rpc(c, with, state, &self.policy),
                Capability::Graphql(c) => net::call_graphql(c, with, state, &self.policy),
                Capability::Llm(c) => net::call_llm(c, with, state, &self.policy),
                Capability::Mcp(c) => net::call_mcp(c, with, state, &self.policy),
                Capability::Vector(c) => net::call_vector(c, with, state, &self.policy),
                Capability::Webhook(c) => net::call_webhook(c, with, state, &self.policy),
                Capability::Sse(c) => net::call_sse(c, with, state, &self.policy),
                Capability::Passthrough => Ok(json!({
                    "capability": "passthrough", "value": expand(with, state, with)
                })),
                Capability::Json(c) => data::call_json(c, with, state),
                Capability::Csv(c) => data::call_csv(c, with, state),
                Capability::Xml(c) => data::call_xml(c, with, state),
                Capability::Markdown(c) => data::call_markdown(c, with, state),
                Capability::Diff(c) => data::call_diff(c, with, state),
                Capability::Validate(c) => data::call_validate(c, with, state),
                Capability::Math(c) => data::call_math(c, with, state),
                Capability::Hash(c) => data::call_hash(c, with, state),
                Capability::Graph(c) => data::call_graph(c, with, state),
                Capability::Tokenize(c) => data::call_tokenize(c, with, state),
                Capability::Cron(c) => data::call_cron(c, with, state),
                Capability::KeyValue(c) => store::call_keyvalue(c, with, state, &self.policy),
                Capability::Cache(c) => store::call_cache(c, with, state, &self.policy),
                Capability::Queue(c) => store::call_queue(c, with, state, &self.policy),
                Capability::Metrics(c) => sys::call_metrics(c, with, state),
                Capability::NotifyLocal(c) => sys::call_notify_local(c, with, state, &self.policy),
                Capability::Tcp(c) => proto::call_tcp(c, with, state, &self.policy),
                Capability::Udp(c) => proto::call_udp(c, with, state, &self.policy),
                Capability::Redis(c) => proto::call_redis(c, with, state, &self.policy),
                Capability::Nats(c) => proto::call_nats(c, with, state, &self.policy),
                Capability::Mqtt(c) => proto::call_mqtt(c, with, state, &self.policy),
                Capability::Smtp(c) => proto::call_smtp(c, with, state, &self.policy),
                Capability::Archive(c) => proto::call_archive(c, with, state, &self.policy),
                Capability::S3(c) => service::call_s3(c, with, state, &self.policy),
                Capability::Prometheus(c) => service::call_prometheus(c, with, state, &self.policy),
                Capability::Kafka(c) => service::call_kafka(c, with, state, &self.policy),
                Capability::Pdf(c) => service::call_pdf(c, with, state, &self.policy),
                Capability::Sql(c) => service::call_sql(c, with, state, &self.policy),
                Capability::WebSearch(c) => web::call_web_search(c, with, state, &self.policy),
                Capability::WebFetch(c) => web::call_web_fetch(c, with, state, &self.policy),
                Capability::GoalRunner(c) => goal::call_goal_runner(c, with, state, &self.policy),
            };
            match r {
                Ok(v) => return Ok(v),
                Err(e) => last = Some(e),
            }
        }
        Err(last.unwrap_or_else(|| anyhow!("capability {name:?} failed")))
    }
}

// ── template expansion ──────────────────────────────────────────────

/// Expand `"${state.a.b}"` / `"${env.NAME}"` / `"${with.k}"` inside a JSON value.
pub fn expand(t: &Value, state: &Value, with: &Value) -> Value {
    match t {
        Value::String(s) => match expand_str(s, state, with) {
            Some(v) => v,
            None => Value::String(s.clone()),
        },
        Value::Array(a) => Value::Array(a.iter().map(|x| expand(x, state, with)).collect()),
        Value::Object(o) => Value::Object(
            o.iter()
                .map(|(k, v)| (k.clone(), expand(v, state, with)))
                .collect(),
        ),
        other => other.clone(),
    }
}

/// Resolve `${env.NAME}` / `${secret.NAME}` in a policy-level string.
///
/// A **missing** name is a hard error, not an empty string. Silently expanding
/// to `""` used to turn `"${env.LAYA_STORE_DIR}/queue.json"` into `"/queue.json"`
/// — or, when the whole path was the reference, into a directory literally named
/// `null` — which writes state to an unintended location instead of reporting a
/// misconfiguration. Policy paths are security-relevant, so failing closed here
/// matches how `${secret.X}` is treated everywhere else.
fn try_expand_env_only(s: &str) -> Result<String> {
    if !s.contains("${") {
        return Ok(s.to_string());
    }
    let mut out = String::new();
    let mut rest = s;
    while let Some(i) = rest.find("${") {
        out.push_str(&rest[..i]);
        let after = &rest[i + 2..];
        match after.find('}') {
            Some(j) => {
                let path = &after[..j];
                match path
                    .strip_prefix("env.")
                    .or_else(|| path.strip_prefix("secret."))
                {
                    Some(name) => out.push_str(&secret::get(name)?),
                    None => out.push_str(&format!("${{{path}}}")),
                }
                rest = &after[j + 1..];
            }
            None => break,
        }
    }
    out.push_str(rest);
    Ok(out)
}

/// A string that is exactly one placeholder becomes the referenced JSON value
/// (so numbers/objects survive); otherwise placeholders are string-interpolated.
fn expand_str(s: &str, state: &Value, with: &Value) -> Option<Value> {
    if let Some(path) = whole_placeholder(s) {
        return Some(lookup(path, state, with));
    }
    if !s.contains("${") {
        return None;
    }
    let mut out = String::new();
    let mut rest = s;
    while let Some(i) = rest.find("${") {
        out.push_str(&rest[..i]);
        let after = &rest[i + 2..];
        match after.find('}') {
            Some(j) => {
                let path = &after[..j];
                let v = lookup(path, state, with);
                // An unresolved `secret.`/`env.` reference inside a larger string
                // used to stringify to the literal "null", so `"Bearer ${env.X}"`
                // silently sent `Bearer null` instead of failing. Emit a marker
                // the caller can detect rather than fabricating a credential.
                if v.is_null() && (path.starts_with("secret.") || path.starts_with("env.")) {
                    out.push_str("${unresolved:");
                    out.push_str(path);
                    out.push('}');
                } else {
                    out.push_str(&stringify(&v));
                }
                rest = &after[j + 1..];
            }
            None => {
                break;
            }
        }
    }
    out.push_str(rest); // trailing literal segment
    Some(Value::String(out))
}

/// True when an expanded string still carries an unresolved reference marker.
pub fn has_unresolved(s: &str) -> bool {
    s.contains("${unresolved:")
}

/// Collect the unresolved references inside a capability's own configuration.
///
/// Configured strings are stored **unexpanded** (e.g. `"Bearer ${env.X}"`), so
/// this expands each with empty state/with first and reports any that still
/// carry the sentinel. `with` is checked separately in `Registry::call` because
/// it is per-invocation.
fn unresolved_in(cap: &Capability) -> Vec<String> {
    fn walk(v: &Value, out: &mut Vec<String>) {
        match v {
            Value::String(s) => {
                if let Value::String(e) = expand(&Value::String(s.clone()), &json!({}), &json!({}))
                {
                    if has_unresolved(&e) {
                        out.push(e);
                    }
                }
            }
            Value::Array(a) => a.iter().for_each(|x| walk(x, out)),
            Value::Object(o) => o.values().for_each(|x| walk(x, out)),
            _ => {}
        }
    }
    // Serialise the capability to inspect every configured string at once; the
    // enum's derived Debug is not stable, so go through its declared fields by
    // re-expanding the common ones.
    let mut out = Vec::new();
    match cap {
        Capability::Http(c) => {
            walk(&Value::String(c.url.clone()), &mut out);
            walk(&Value::Object(c.headers.clone()), &mut out);
        }
        Capability::Exec(c) => walk(&Value::Object(c.env.clone()), &mut out),
        Capability::Agent(c) => {
            if let Some(u) = &c.url {
                walk(&Value::String(u.clone()), &mut out);
            }
        }
        Capability::Rpc(c) => walk(&Value::String(c.url.clone()), &mut out),
        Capability::Graphql(c) => {
            walk(&Value::String(c.url.clone()), &mut out);
            walk(&Value::String(c.auth_value.clone()), &mut out);
        }
        Capability::Llm(c) => {
            walk(&Value::String(c.url.clone()), &mut out);
            walk(&Value::String(c.auth_value.clone()), &mut out);
        }
        Capability::Mcp(c) => {
            if let Some(u) = &c.url {
                walk(&Value::String(u.clone()), &mut out);
            }
            walk(&Value::Object(c.headers.clone()), &mut out);
        }
        Capability::Vector(c) => walk(&Value::String(c.url.clone()), &mut out),
        Capability::Webhook(c) => {
            walk(&Value::String(c.url.clone()), &mut out);
            walk(&Value::String(c.sign_secret.clone()), &mut out);
        }
        Capability::Sse(c) => walk(&Value::String(c.url.clone()), &mut out),
        Capability::S3(c) => {
            walk(&Value::String(c.endpoint.clone()), &mut out);
            walk(&Value::Object(c.headers.clone()), &mut out);
        }
        Capability::Prometheus(c) => walk(&Value::String(c.url.clone()), &mut out),
        Capability::Kafka(c) => walk(&Value::String(c.url.clone()), &mut out),
        Capability::Redis(c) => walk(&Value::String(c.password.clone()), &mut out),
        Capability::Smtp(c) => {
            walk(&Value::String(c.username.clone()), &mut out);
            walk(&Value::String(c.password.clone()), &mut out);
        }
        Capability::WebSearch(c) => {
            walk(&Value::String(c.endpoint.clone()), &mut out);
            walk(&Value::Object(c.headers.clone()), &mut out);
        }
        Capability::WebFetch(c) => {
            walk(&Value::String(c.endpoint.clone()), &mut out);
            walk(&Value::Object(c.headers.clone()), &mut out);
            walk(&c.body, &mut out);
        }
        _ => {}
    }
    out
}

fn whole_placeholder(s: &str) -> Option<&str> {
    let t = s.trim();
    if t.starts_with("${") && t.ends_with('}') && t[2..t.len() - 1].find('}').is_none() {
        return Some(&t[2..t.len() - 1]);
    }
    None
}

fn lookup(path: &str, state: &Value, with: &Value) -> Value {
    // `${secret.NAME}` and the legacy `${env.NAME}` both resolve through the
    // secret store (env first, then .env files).
    //
    // A missing reference yields an explicit sentinel, not `Null`: `null` used to
    // stringify into the middle of a larger string and produce `"Bearer null"`,
    // i.e. a request sent with the wrong credential instead of an error.
    // `has_unresolved()` detects the sentinel and callers refuse to proceed.
    if let Some(name) = path.strip_prefix("secret.") {
        return match secret::get(name) {
            Ok(v) => Value::String(v),
            Err(_) => Value::String(format!("${{unresolved:{path}}}")),
        };
    }
    if let Some(name) = path.strip_prefix("env.") {
        return match secret::get(name) {
            Ok(v) => Value::String(v),
            Err(_) => match std::env::var(name) {
                Ok(v) => Value::String(v),
                Err(_) => Value::String(format!("${{unresolved:{path}}}")),
            },
        };
    }
    let (root, rest) = match path.split_once('.') {
        Some((r, rest)) => (r, rest),
        None => (path, ""),
    };
    let base = match root {
        "state" => state,
        "with" => with,
        "env" => return Value::Null,
        _ => state, // bare path ⇒ state
    };
    let mut cur = base;
    let path = if root == "state" || root == "with" {
        rest
    } else {
        path
    };
    for seg in path.split('.').filter(|s| !s.is_empty()) {
        cur = match cur {
            Value::Object(o) => match o.get(seg) {
                Some(v) => v,
                None => return Value::Null,
            },
            Value::Array(a) => match seg.parse::<usize>().ok().and_then(|i| a.get(i)) {
                Some(v) => v,
                None => return Value::Null,
            },
            _ => return Value::Null,
        };
    }
    cur.clone()
}

/// Resolve the operation a capability should perform.
///
/// `with.op` is a **per-call override**; the capability definition's `op` is the
/// default. Only honouring the definition meant `call("q", {"op": "length"})`
/// silently ran whatever op the spec declared — for a `queue` whose default is
/// `push`, asking for `length` pushed a `null` and returned the wrong count.
/// Specs already pass `"with": {"op": ...}` expecting it to take effect.
pub(crate) fn effective_op(configured: &str, with: &Value, fallback: &str) -> String {
    if let Some(op) = with.get("op").and_then(|v| v.as_str()) {
        if !op.is_empty() {
            return op.to_string();
        }
    }
    if !configured.is_empty() {
        return configured.to_string();
    }
    fallback.to_string()
}

fn stringify(v: &Value) -> String {
    match v {
        Value::String(s) => s.clone(),
        other => other.to_string(),
    }
}

// ── implementations ─────────────────────────────────────────────────

pub(crate) fn host_of(url: &str) -> String {
    url.split("://")
        .nth(1)
        .unwrap_or(url)
        .split('/')
        .next()
        .unwrap_or("")
        .split('@')
        .next_back()
        .unwrap_or("")
        .split(':')
        .next()
        .unwrap_or("")
        .to_string()
}

fn check_host(url: &str, policy: &Policy) -> Result<()> {
    if policy.allow_hosts.is_empty() {
        return Ok(());
    }
    let h = host_of(url);
    if policy.allow_hosts.iter().any(|a| a == &h) {
        Ok(())
    } else {
        bail!("host {h:?} is not in policy.allow_hosts (denied)")
    }
}

fn bounded_timeout(ms: u64, policy: &Policy) -> Duration {
    Duration::from_millis(ms.min(policy.max_timeout_ms).max(1))
}

fn call_http(c: &HttpCap, with: &Value, state: &Value, policy: &Policy) -> Result<Value> {
    let url = stringify(&expand(&Value::String(c.url.clone()), state, with));
    check_host(&url, policy)?;
    let agent = ureq::AgentBuilder::new()
        .timeout(bounded_timeout(c.timeout_ms, policy))
        .build();

    let mut req = match c.method.as_str() {
        "GET" => agent.get(&url),
        "POST" => agent.post(&url),
        "PUT" => agent.put(&url),
        "DELETE" => agent.delete(&url),
        "PATCH" => agent.request("PATCH", &url),
        m => bail!("unsupported http method {m:?}"),
    };
    for (k, v) in &c.headers {
        req = req.set(k, &stringify(&expand(v, state, with)));
    }
    let resp = match &c.body {
        None => req.call(),
        Some(body) => {
            let b = expand(body, state, with);
            match &b {
                Value::Object(_) => req.send_json(b),
                _ => req.send_string(&stringify(&b)),
            }
        }
    };
    let resp = resp.map_err(|e| anyhow!("http {url} failed: {e}"))?;
    let status = resp.status() as i64;
    let text = resp
        .into_string()
        .map_err(|e| anyhow!("http read failed: {e}"))?;
    let text = truncate(text, policy.max_output);
    let body = if c.expect_json {
        serde_json::from_str::<Value>(&text).unwrap_or(Value::String(text))
    } else {
        Value::String(text)
    };
    Ok(json!({ "status": status, "body": body, "capability": "http", "url": url }))
}

fn call_exec(c: &ExecCap, with: &Value, state: &Value, policy: &Policy) -> Result<Value> {
    if !policy.allow_exec {
        bail!("exec capabilities are disabled (set policy.allow_exec = true to enable)");
    }
    let argv: Vec<String> = c
        .argv
        .iter()
        .map(|a| stringify(&expand(&Value::String(a.clone()), state, with)))
        .collect();
    if argv.is_empty() {
        bail!("exec capability has empty argv");
    }
    let mut cmd = std::process::Command::new(&argv[0]);
    cmd.args(&argv[1..]);
    if let Some(d) = &c.cwd {
        cmd.current_dir(d);
    }
    for (k, v) in &c.env {
        cmd.env(k, stringify(&expand(v, state, with)));
    }
    cmd.stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped());
    let child = cmd
        .spawn()
        .map_err(|e| anyhow!("spawn {:?} failed: {e}", argv[0]))?;

    // Bounded wait: poll with a deadline, then kill.
    let deadline = std::time::Instant::now() + bounded_timeout(c.timeout_ms, policy);
    let mut child = child;
    loop {
        match child.try_wait() {
            Ok(Some(_)) => break,
            Ok(None) => {
                if std::time::Instant::now() >= deadline {
                    let _ = child.kill();
                    bail!("exec {:?} timed out after {}ms", argv[0], c.timeout_ms);
                }
                std::thread::sleep(Duration::from_millis(10));
            }
            Err(e) => bail!("exec wait failed: {e}"),
        }
    }
    let out = child
        .wait_with_output()
        .map_err(|e| anyhow!("exec collect failed: {e}"))?;
    let cap = c.max_output.min(policy.max_output);
    let stdout = truncate(String::from_utf8_lossy(&out.stdout).to_string(), cap);
    let stderr = truncate(String::from_utf8_lossy(&out.stderr).to_string(), cap);
    let code = out.status.code().unwrap_or(-1);
    let exit_ok = out.status.success();
    Ok(json!({
        "exit_code": code,
        "ok": exit_ok,
        "stdout": stdout,
        "stderr": stderr,
        "capability": "exec",
        "argv": argv,
    }))
}

fn call_agent(c: &AgentCap, with: &Value, state: &Value, policy: &Policy) -> Result<Value> {
    let request = json!({
        "session": c.session,
        "input": with.clone(),
        "state": state.clone(),
    });
    match c.transport.as_str() {
        "http" => {
            let url = stringify(&expand(
                &Value::String(c.url.clone().unwrap_or_default()),
                state,
                with,
            ));
            if url.is_empty() {
                bail!("agent capability needs 'url' for transport=http");
            }
            check_host(&url, policy)?;
            let agent = ureq::AgentBuilder::new()
                .timeout(bounded_timeout(c.timeout_ms, policy))
                .build();
            let resp = agent
                .post(&url)
                .set("content-type", "application/json")
                .send_json(request)
                .map_err(|e| anyhow!("agent http {url} failed: {e}"))?;
            let status = resp.status() as i64;
            let text = truncate(
                resp.into_string().map_err(|e| anyhow!("agent read: {e}"))?,
                policy.max_output,
            );
            let body = serde_json::from_str::<Value>(&text).unwrap_or(Value::String(text));
            Ok(
                json!({ "status": status, "body": body, "capability": "agent", "transport": "http" }),
            )
        }
        "stdio" => {
            if !policy.allow_exec {
                bail!("agent transport=stdio spawns a process; set policy.allow_exec = true to enable");
            }
            let cmd = c
                .command
                .as_ref()
                .ok_or_else(|| anyhow!("agent capability needs 'command' for transport=stdio"))?;
            let argv: Vec<String> = cmd
                .iter()
                .map(|a| stringify(&expand(&Value::String(a.clone()), state, with)))
                .collect();
            let reply = stdio_json_rpc(&argv, &request, c.timeout_ms, policy)?;
            Ok(json!({ "body": reply, "capability": "agent", "transport": "stdio" }))
        }
        other => bail!("agent transport {other:?} unsupported (http | stdio)"),
    }
}

/// Line-delimited JSON over stdio: write one request line, read one reply line.
///
/// This is the shape used by codex-style app-servers (JSON-RPC-ish, one JSON
/// object per line). A long-lived server can be used by keeping it as a
/// capability per node invocation.
fn stdio_json_rpc(
    argv: &[String],
    request: &Value,
    timeout_ms: u64,
    policy: &Policy,
) -> Result<Value> {
    use std::io::{BufRead, BufReader, Write};
    if argv.is_empty() {
        bail!("stdio agent has empty command");
    }
    let mut child = std::process::Command::new(&argv[0])
        .args(&argv[1..])
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::null())
        .spawn()
        .map_err(|e| anyhow!("spawn {:?} failed: {e}", argv[0]))?;

    let line = serde_json::to_string(request)?;
    {
        let mut stdin = child.stdin.take().ok_or_else(|| anyhow!("no stdin"))?;
        writeln!(stdin, "{line}")?;
        stdin.flush()?;
        // drop stdin → signals EOF for one-shot servers
    }
    let stdout = child.stdout.take().ok_or_else(|| anyhow!("no stdout"))?;
    let reader = BufReader::new(stdout);

    // Read the first non-empty line, bounded by the timeout via a watchdog kill.
    let deadline = std::time::Instant::now() + bounded_timeout(timeout_ms, policy);
    let (tx, rx) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        for l in reader.lines() {
            match l {
                Ok(s) if !s.trim().is_empty() => {
                    let _ = tx.send(Ok(s));
                    return;
                }
                Ok(_) => continue,
                Err(e) => {
                    let _ = tx.send(Err(e.to_string()));
                    return;
                }
            }
        }
        let _ = tx.send(Err("agent closed stdout without a reply".to_string()));
    });
    let remaining = deadline.saturating_duration_since(std::time::Instant::now());
    let got = rx.recv_timeout(remaining);
    let _ = child.kill();
    let _ = child.wait();
    match got {
        Ok(Ok(line)) => {
            let line = truncate(line, policy.max_output);
            Ok(serde_json::from_str::<Value>(&line).unwrap_or(Value::String(line)))
        }
        Ok(Err(e)) => bail!("agent stdio error: {e}"),
        Err(_) => bail!("agent stdio timed out after {timeout_ms}ms"),
    }
}

fn truncate(mut s: String, max: usize) -> String {
    if s.len() <= max {
        return s;
    }
    // keep it valid UTF-8 at the boundary
    let mut cut = max;
    while cut > 0 && !s.is_char_boundary(cut) {
        cut -= 1;
    }
    s.truncate(cut);
    s.push_str("…[truncated]");
    s
}

/// Convenience for building a registry programmatically (library users/tests).
pub fn registry_from(entries: &[(&str, Value)], policy: Option<Policy>) -> Result<Registry> {
    let mut spec = json!({ "capabilities": {} });
    {
        let caps = spec["capabilities"].as_object_mut().unwrap();
        for (k, v) in entries {
            caps.insert(k.to_string(), v.clone());
        }
    }
    let mut r = Registry::from_spec(&spec)?;
    if let Some(p) = policy {
        r.policy = p;
    }
    Ok(r)
}

/// Shareable handle (used by `spec::from_spec_with_dir` when building nodes).
pub type SharedRegistry = Arc<Registry>;
