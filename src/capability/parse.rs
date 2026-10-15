//! Spec -> `Capability` parsing.
//!
//! Kept separate from `mod.rs` so the registry/dispatch logic stays readable:
//! every kind's accepted fields live here, next to each other.

use anyhow::{anyhow, bail, Result};
use serde_json::{json, Value};

use super::{
    browser, computer_use, data, db, goal, local, net, plugin, proto, service, store, sys,
    web, AgentCap, Capability, ExecCap, HttpCap,
};

/// Records which option keys a kind's parser actually read, so [`parse_cap`] can
/// refuse a declared option nobody reads.
///
/// An option the engine never consumes is a silent no-op: `max_operatons`
/// leaves a plugin at the default cap and `chorme_binary` leaves Chrome to
/// `$PATH`, and nothing says so. Same rule as P126's "a declared heuristic field
/// with no value is a clean miss": a declaration nobody reads is a comment, not
/// config (P128).
struct Fields<'a> {
    def: &'a Value,
    seen: Vec<String>,
}

impl<'a> Fields<'a> {
    fn new(def: &'a Value) -> Self {
        Self { def, seen: Vec::new() }
    }

    /// `def.get(key)`, recording `key` as read. The returned reference borrows
    /// the spec (`'a`), not `self`, so call sites keep their
    /// `and_then`/`unwrap_or` chains unchanged.
    fn get(&mut self, key: &str) -> Option<&'a Value> {
        self.seen.push(key.to_string());
        self.def.get(key)
    }

    fn read(&self, key: &str) -> bool {
        self.seen.iter().any(|k| k == key)
    }
}

/// Option keys the engine reads *outside* a kind's parser, so they are not
/// "unread": `platforms` drives the platform gate (`dsl_smoke`'s platform-bound
/// skip and the BDD transpiler), and `description` is an annotation the
/// transpiler and the docs carry through.
const OPTIONS_READ_ELSEWHERE: &[&str] = &["platforms", "description"];

/// Kinds whose *definition* must carry a non-empty target field to ever run.
///
/// Mirrors each kind\'s own runtime `bail!("<kind> … needs \'<field>\'")`: a
/// capability declared without the field still `validate`s, but every call fails
/// at run — and the target can never be supplied later, because the runtime only
/// `expand`s *into* the configured string (an empty template stays empty). A
/// field the runtime instead lets a call\'s `with` supply (`graphql.query`,
/// `kafka.topic`, `pdf.path`, `archive.path`, `db.*`, `cron.expr`,
/// `computer_use.allowed_surface`, `web_fetch`) is deliberately **not** required
/// here — such a capability *is* runnable via `with`. This is the capability
/// analogue of P128\'s "a declared option nobody reads": a capability whose
/// definition can never run is the same latent bug, one level up (P149).
fn check_required_options(name: &str, kind: &str, def: &Value) -> Result<()> {
    let nonempty_str = |k: &str| {
        def.get(k)
            .and_then(Value::as_str)
            .map(|v| !v.trim().is_empty())
            .unwrap_or(false)
    };
    let port_set =
        |k: &str| def.get(k).and_then(Value::as_u64).map(|p| p != 0).unwrap_or(false);
    let command_set = || {
        def.get("command")
            .and_then(Value::as_array)
            .map(|a| !a.is_empty())
            .unwrap_or(false)
    };
    let mut missing: Vec<&str> = Vec::new();
    match kind {
        "rpc" | "graphql" | "llm" | "chat" | "vector" | "webhook" | "sse" | "prometheus"
        | "prom" | "kafka" => {
            if !nonempty_str("url") {
                missing.push("url");
            }
        }
        "web_search" | "search" => {
            if !nonempty_str("endpoint") {
                missing.push("endpoint");
            }
        }
        "s3" | "object_store" => {
            for k in ["endpoint", "bucket"] {
                if !nonempty_str(k) {
                    missing.push(k);
                }
            }
        }
        "sql" | "database" => {
            if !nonempty_str("dsn") {
                missing.push("dsn");
            }
        }
        "sqlite" => {
            if !nonempty_str("db") {
                missing.push("db");
            }
        }
        "tcp" | "udp" => {
            if !nonempty_str("host") {
                missing.push("host");
            }
            if !port_set("port") {
                missing.push("port");
            }
        }
        "agent" | "app_server" | "mcp" => {
            if kind == "mcp" && !nonempty_str("tool") {
                missing.push("tool");
            }
            let transport = def
                .get("transport")
                .and_then(Value::as_str)
                .map(str::to_string)
                .unwrap_or_else(|| {
                    if def.get("command").is_some() {
                        "stdio".to_string()
                    } else {
                        "http".to_string()
                    }
                });
            if transport == "http" {
                if !nonempty_str("url") {
                    missing.push("url");
                }
            } else if !command_set() {
                missing.push("command");
            }
        }
        _ => {}
    }
    if !missing.is_empty() {
        let list = missing
            .iter()
            .map(|k| format!("{k:?}"))
            .collect::<Vec<_>>()
            .join(", ");
        bail!(
            "capability {name:?} (kind {kind:?}) needs {list} (non-empty) — the engine reads it as an empty value, so the capability `validate`s but every call fails at run (`{kind} needs {list}`)"
        );
    }
    Ok(())
}

/// Numeric options the parsers read via `Value::as_u64`. A *present* value that
/// is not a non-negative integer (`-1`, `1.5`, `"9000"`, `true`) is silently
/// replaced by the option\'s default — the capability then runs with a timeout /
/// limit nobody chose. Same "a declared value the engine throws away" class as
/// P128/P149, now about the value\'s *domain* rather than its key (P150).
const U64_OPTIONS: &[&str] = &[
    "max_bytes",
    "max_events",
    "max_operations",
    "max_output",
    "max_owned_pages",
    "max_results",
    "max_text",
    "max_tokens",
    "owned_idle_ms",
    "startup_timeout_ms",
    "timeout_ms",
];

/// Numeric options the parsers read via `Value::as_f64`; a present value that is
/// not a JSON number is silently defaulted (e.g. `"0.9"` leaves a computer-use
/// node at a 0.8 floor).
const F64_OPTIONS: &[&str] = &["chars_per_token", "min_confidence", "temperature"];

/// Kinds whose `port` is stored as `u16`: any value above 65535 wraps, so
/// `70000` quietly connects to `4464`. The port must be a real port.
const PORT_KINDS: &[&str] = &["tcp", "udp", "redis", "nats", "mqtt", "smtp", "mail_send"];

fn check_option_domains(name: &str, kind: &str, def: &Value) -> Result<()> {
    let Some(obj) = def.as_object() else {
        return Ok(());
    };
    for k in U64_OPTIONS {
        if obj.contains_key(*k) && obj.get(*k).and_then(Value::as_u64).is_none() {
            bail!(
                "capability {name:?} (kind {kind:?}) option {k:?} must be a non-negative integer — the engine reads it as an unsigned number, so a fractional / negative / non-numeric value is silently replaced by the default"
            );
        }
    }
    for k in F64_OPTIONS {
        if obj.contains_key(*k) && obj.get(*k).and_then(Value::as_f64).is_none() {
            bail!(
                "capability {name:?} (kind {kind:?}) option {k:?} must be a number — the engine reads it as a float, so a non-numeric value is silently replaced by the default"
            );
        }
    }
    if PORT_KINDS.contains(&kind) {
        if let Some(v) = obj.get("port") {
            match v.as_u64() {
                Some(p) if (1..=65535).contains(&p) => {}
                _ => bail!(
                    "capability {name:?} (kind {kind:?}) option \"port\" must be a TCP/UDP port in 1..=65535 — it is cast to `u16`, so a larger value wraps to a different port (70000 -> 4464)"
                ),
            }
        }
    }
    Ok(())
}

pub(super) fn parse_cap(name: &str, def: &Value) -> Result<Capability> {
    let kind = def
        .get("kind")
        .and_then(|v| v.as_str())
        .ok_or_else(|| anyhow!("capability {name:?} missing 'kind'"))?;
    let mut fields = Fields::new(def);
    let cap = parse_kind(name, kind, &mut fields)?;
    check_required_options(name, kind, def)?;
    // Every option the spec declares must be one this kind reads.
    let unknown: Vec<String> = def
        .as_object()
        .map(|o| {
            o.keys()
                .filter(|k| k.as_str() != "kind")
                .filter(|k| !fields.read(k) && !OPTIONS_READ_ELSEWHERE.contains(&k.as_str()))
                .cloned()
                .collect()
        })
        .unwrap_or_default();
    if !unknown.is_empty() {
        let known: Vec<&str> = fields.seen.iter().map(String::as_str).collect();
        let hints: Vec<String> = unknown
            .iter()
            .map(|k| match crate::spec::closest_match(k, &known) {
                Some(c) => format!("{k} (did you mean \"{c}\"?)"),
                None => k.clone(),
            })
            .collect();
        bail!(
            "capability {name:?} (kind {kind:?}) declares option(s) this engine never reads: {}",
            hints.join(", ")
        );
    }
    check_option_domains(name, kind, def)?;
    Ok(cap)
}

/// The per-kind parser: every option it reads goes through [`Fields::get`], so
/// [`parse_cap`] can prove a declared option was consumed.
fn parse_kind(name: &str, kind: &str, f: &mut Fields) -> Result<Capability> {
    match kind {
        "http" => Ok(Capability::Http(HttpCap {
            method: f.get("method").and_then(|v| v.as_str()).unwrap_or("POST").to_uppercase(),
            url: f
                .get("url")
                .and_then(|v| v.as_str())
                .ok_or_else(|| anyhow!("capability {name:?} (http) missing 'url'"))?
                .to_string(),
            headers: f.get("headers").and_then(|v| v.as_object()).cloned().unwrap_or_default(),
            body: f.get("body").cloned(),
            timeout_ms: f.get("timeout_ms").and_then(|v| v.as_u64()).unwrap_or(15_000),
            expect_json: f.get("expect_json").and_then(|v| v.as_bool()).unwrap_or(true),
            allow_error_status: f
                .get("allow_error_status")
                .and_then(|v| v.as_bool())
                .unwrap_or(false),
        })),
        "exec" => Ok(Capability::Exec(ExecCap {
            argv: f
                .get("argv")
                .and_then(|v| v.as_array())
                .map(|a| a.iter().filter_map(|x| x.as_str().map(str::to_string)).collect())
                .ok_or_else(|| anyhow!("capability {name:?} (exec) missing 'argv'"))?,
            cwd: f.get("cwd").and_then(|v| v.as_str()).map(str::to_string),
            env: f.get("env").and_then(|v| v.as_object()).cloned().unwrap_or_default(),
            timeout_ms: f.get("timeout_ms").and_then(|v| v.as_u64()).unwrap_or(30_000),
            max_output: f.get("max_output").and_then(|v| v.as_u64()).unwrap_or(256 << 10) as usize,
        })),
        "agent" | "app_server" => Ok(Capability::Agent(AgentCap {
            transport: f
                .get("transport")
                .and_then(|v| v.as_str())
                .unwrap_or(if f.get("command").is_some() { "stdio" } else { "http" })
                .to_string(),
            url: f.get("url").and_then(|v| v.as_str()).map(str::to_string),
            command: f
                .get("command")
                .and_then(|v| v.as_array())
                .map(|a| a.iter().filter_map(|x| x.as_str().map(str::to_string)).collect()),
            session: f.get("session").and_then(|v| v.as_str()).map(str::to_string),
            timeout_ms: f.get("timeout_ms").and_then(|v| v.as_u64()).unwrap_or(30_000),
            max_output: f.get("max_output").and_then(|v| v.as_u64()).unwrap_or(256 << 10) as usize,
        })),
        // ── local / pure ──
        "datetime" => Ok(Capability::Datetime(local::DatetimeCap {
            op: f.get("op").and_then(|v| v.as_str()).unwrap_or("now").to_string(),
            format: f.get("format").and_then(|v| v.as_str()).unwrap_or("").to_string(),
            offset_secs: f.get("offset_secs").and_then(|v| v.as_i64()).unwrap_or(0),
        })),
        "text" => Ok(Capability::Text(local::TextCap {
            op: f.get("op").and_then(|v| v.as_str()).unwrap_or("length").to_string(),
            pattern: f.get("pattern").and_then(|v| v.as_str()).unwrap_or("").to_string(),
            replacement: f.get("replacement").and_then(|v| v.as_str()).unwrap_or("").to_string(),
            separator: f.get("separator").and_then(|v| v.as_str()).unwrap_or("").to_string(),
            hash: f.get("hash").and_then(|v| v.as_str()).unwrap_or("").to_string(),
        })),
        "file" => Ok(Capability::File(local::FileCap {
            op: f.get("op").and_then(|v| v.as_str()).unwrap_or("read").to_string(),
            allow_roots: f
                .get("allow_roots")
                .and_then(|v| v.as_array())
                .map(|a| a.iter().filter_map(|x| x.as_str().map(std::path::PathBuf::from)).collect())
                .unwrap_or_default(),
            max_bytes: f.get("max_bytes").and_then(|v| v.as_u64()).unwrap_or(1 << 20) as usize,
        })),
        "sqlite" => Ok(Capability::Sqlite(local::SqliteCap {
            db: f.get("db").and_then(|v| v.as_str()).unwrap_or("").to_string(),
            op: f.get("op").and_then(|v| v.as_str()).unwrap_or("query").to_string(),
            readonly: f.get("readonly").and_then(|v| v.as_bool()).unwrap_or(true),
            timeout_ms: f.get("timeout_ms").and_then(|v| v.as_u64()).unwrap_or(10_000),
        })),
        "db" => Ok(Capability::Db(db::DbCap {
            sqlite: f.get("sqlite").and_then(|v| v.as_str()).unwrap_or("").to_string(),
            duckdb: f.get("duckdb").and_then(|v| v.as_str()).unwrap_or("").to_string(),
            alias: f.get("alias").and_then(|v| v.as_str()).unwrap_or("").to_string(),
            op: f.get("op").and_then(|v| v.as_str()).unwrap_or("query").to_string(),
            readonly: f.get("readonly").and_then(|v| v.as_bool()).unwrap_or(true),
            format: f.get("format").and_then(|v| v.as_str()).unwrap_or("json").to_string(),
            mode: f.get("mode").and_then(|v| v.as_str()).unwrap_or("embed").to_string(),
            endpoint: f.get("endpoint").and_then(|v| v.as_str()).unwrap_or("").to_string(),
            timeout_ms: f.get("timeout_ms").and_then(|v| v.as_u64()).unwrap_or(60_000),
        })),
        "shell" => Ok(Capability::Shell(local::ShellCap {
            command: f
                .get("command")
                .and_then(|v| v.as_str())
                .ok_or_else(|| anyhow!("capability {name:?} (shell) missing 'command'"))?
                .to_string(),
            cwd: f.get("cwd").and_then(|v| v.as_str()).map(str::to_string),
            timeout_ms: f.get("timeout_ms").and_then(|v| v.as_u64()).unwrap_or(30_000),
        })),
        // ── network ──
        "rpc" => Ok(Capability::Rpc(net::RpcCap {
            url: f.get("url").and_then(|v| v.as_str()).unwrap_or("").to_string(),
            method: f.get("method").and_then(|v| v.as_str()).unwrap_or("").to_string(),
            timeout_ms: f.get("timeout_ms").and_then(|v| v.as_u64()).unwrap_or(15_000),
            id: f.get("id").and_then(|v| v.as_i64()),
        })),
        "graphql" => Ok(Capability::Graphql(net::GraphqlCap {
            url: f.get("url").and_then(|v| v.as_str()).unwrap_or("").to_string(),
            query: f.get("query").and_then(|v| v.as_str()).unwrap_or("").to_string(),
            variables: f.get("variables").cloned().unwrap_or_else(|| json!({})),
            timeout_ms: f.get("timeout_ms").and_then(|v| v.as_u64()).unwrap_or(15_000),
            auth_header: f.get("auth_header").and_then(|v| v.as_str()).unwrap_or("").to_string(),
            auth_value: f.get("auth_value").and_then(|v| v.as_str()).unwrap_or("").to_string(),
        })),
        "llm" | "chat" => Ok(Capability::Llm(net::LlmCap {
            url: f.get("url").and_then(|v| v.as_str()).unwrap_or("").to_string(),
            model: f.get("model").and_then(|v| v.as_str()).unwrap_or("").to_string(),
            system: f.get("system").and_then(|v| v.as_str()).unwrap_or("").to_string(),
            max_tokens: f.get("max_tokens").and_then(|v| v.as_u64()).unwrap_or(256),
            temperature: f.get("temperature").and_then(|v| v.as_f64()).unwrap_or(0.0),
            timeout_ms: f.get("timeout_ms").and_then(|v| v.as_u64()).unwrap_or(30_000),
            auth_header: f.get("auth_header").and_then(|v| v.as_str()).unwrap_or("authorization").to_string(),
            auth_value: f.get("auth_value").and_then(|v| v.as_str()).unwrap_or("").to_string(),
        })),
        "mcp" => Ok(Capability::Mcp(net::McpCap {
            headers: f.get("headers").and_then(|v| v.as_object()).cloned().unwrap_or_default(),
            transport: f
                .get("transport")
                .and_then(|v| v.as_str())
                .unwrap_or(if f.get("command").is_some() { "stdio" } else { "http" })
                .to_string(),
            url: f.get("url").and_then(|v| v.as_str()).map(str::to_string),
            command: f
                .get("command")
                .and_then(|v| v.as_array())
                .map(|a| a.iter().filter_map(|x| x.as_str().map(str::to_string)).collect()),
            tool: f.get("tool").and_then(|v| v.as_str()).unwrap_or("").to_string(),
            timeout_ms: f.get("timeout_ms").and_then(|v| v.as_u64()).unwrap_or(30_000),
        })),
        "vector" => Ok(Capability::Vector(net::VectorCap {
            url: f.get("url").and_then(|v| v.as_str()).unwrap_or("").to_string(),
            op: f.get("op").and_then(|v| v.as_str()).unwrap_or("search").to_string(),
            collection: f.get("collection").and_then(|v| v.as_str()).unwrap_or("").to_string(),
            timeout_ms: f.get("timeout_ms").and_then(|v| v.as_u64()).unwrap_or(15_000),
        })),
        "webhook" => Ok(Capability::Webhook(net::WebhookCap {
            url: f.get("url").and_then(|v| v.as_str()).unwrap_or("").to_string(),
            timeout_ms: f.get("timeout_ms").and_then(|v| v.as_u64()).unwrap_or(10_000),
            sign_header: f.get("sign_header").and_then(|v| v.as_str()).unwrap_or("").to_string(),
            sign_secret: f.get("sign_secret").and_then(|v| v.as_str()).unwrap_or("").to_string(),
        })),
        "sse" => Ok(Capability::Sse(net::SseCap {
            url: f.get("url").and_then(|v| v.as_str()).unwrap_or("").to_string(),
            timeout_ms: f.get("timeout_ms").and_then(|v| v.as_u64()).unwrap_or(15_000),
            max_events: f.get("max_events").and_then(|v| v.as_u64()).unwrap_or(100) as usize,
        })),
        "passthrough" | "noop" => Ok(Capability::Passthrough),
        "json" => Ok(Capability::Json(data::JsonCap {
            op: f.get("op").and_then(|v| v.as_str()).unwrap_or("pick").to_string(),
        })),
        "csv" => Ok(Capability::Csv(data::CsvCap {
            op: f.get("op").and_then(|v| v.as_str()).unwrap_or("parse").to_string(),
            delimiter: f
                .get("delimiter")
                .and_then(|v| v.as_str())
                .and_then(|s| s.chars().next())
                .unwrap_or(','),
            headers: f.get("headers").and_then(|v| v.as_bool()).unwrap_or(true),
        })),
        "xml" => Ok(Capability::Xml(data::XmlCap {
            tag: f.get("tag").and_then(|v| v.as_str()).unwrap_or("").to_string(),
            op: f.get("op").and_then(|v| v.as_str()).unwrap_or("tags").to_string(),
        })),
        "markdown" | "md" => Ok(Capability::Markdown(data::MarkdownCap {
            op: f.get("op").and_then(|v| v.as_str()).unwrap_or("headings").to_string(),
        })),
        "diff" => Ok(Capability::Diff(data::DiffCap {
            op: f.get("op").and_then(|v| v.as_str()).unwrap_or("lines").to_string(),
        })),
        "validate" | "schema" => Ok(Capability::Validate(data::ValidateCap)),
        "math" => Ok(Capability::Math(data::MathCap {
            op: f.get("op").and_then(|v| v.as_str()).unwrap_or("eval").to_string(),
        })),
        "hash" => Ok(Capability::Hash(data::HashCap {
            op: f.get("op").and_then(|v| v.as_str()).unwrap_or("sha256").to_string(),
        })),
        "graph" => Ok(Capability::Graph(data::GraphCap {
            op: f.get("op").and_then(|v| v.as_str()).unwrap_or("reachable").to_string(),
        })),
        "tokenize" | "tokens" => Ok(Capability::Tokenize(data::TokenizeCap {
            op: f.get("op").and_then(|v| v.as_str()).unwrap_or("count").to_string(),
            chars_per_token: f.get("chars_per_token").and_then(|v| v.as_f64()).unwrap_or(4.0),
        })),
        "cron" => Ok(Capability::Cron(data::CronCap {
            expr: f.get("expr").and_then(|v| v.as_str()).unwrap_or("").to_string(),
        })),
        "keyvalue" | "kv" => Ok(Capability::KeyValue(store::KeyValueCap {
            path: f.get("path").and_then(|v| v.as_str()).unwrap_or("").to_string(),
            op: f.get("op").and_then(|v| v.as_str()).unwrap_or("get").to_string(),
            key: f.get("key").and_then(|v| v.as_str()).unwrap_or("").to_string(),
        })),
        "cache" => Ok(Capability::Cache(store::CacheCap {
            path: f.get("path").and_then(|v| v.as_str()).unwrap_or("").to_string(),
            op: f.get("op").and_then(|v| v.as_str()).unwrap_or("get").to_string(),
            ttl_secs: f.get("ttl_secs").and_then(|v| v.as_i64()).unwrap_or(0),
        })),
        "queue" => Ok(Capability::Queue(store::QueueCap {
            path: f.get("path").and_then(|v| v.as_str()).unwrap_or("").to_string(),
            op: f.get("op").and_then(|v| v.as_str()).unwrap_or("length").to_string(),
        })),
        "metrics" | "sysinfo" => Ok(Capability::Metrics(sys::MetricsCap {
            what: f.get("what").and_then(|v| v.as_str()).unwrap_or("").to_string(),
            disk_path: f.get("disk_path").and_then(|v| v.as_str()).unwrap_or("").to_string(),
        })),
        "notify" | "notify_local" => Ok(Capability::NotifyLocal(sys::NotifyLocalCap {
            path: f.get("path").and_then(|v| v.as_str()).unwrap_or("").to_string(),
            bell: f.get("bell").and_then(|v| v.as_bool()).unwrap_or(false),
            timestamp: f.get("timestamp").and_then(|v| v.as_bool()).unwrap_or(true),
            channel: f.get("channel").and_then(|v| v.as_str()).unwrap_or("").to_string(),
            title: f.get("title").and_then(|v| v.as_str()).unwrap_or("").to_string(),
            subtitle: f.get("subtitle").and_then(|v| v.as_str()).unwrap_or("").to_string(),
            sound: f.get("sound").and_then(|v| v.as_str()).unwrap_or("").to_string(),
            timeout_ms: f.get("timeout_ms").and_then(|v| v.as_u64()).unwrap_or(10_000),
        })),
        "tcp" => Ok(Capability::Tcp(proto::TcpCap {
            host: f.get("host").and_then(|v| v.as_str()).unwrap_or("").to_string(),
            port: f.get("port").and_then(|v| v.as_u64()).unwrap_or(0) as u16,
            timeout_ms: f.get("timeout_ms").and_then(|v| v.as_u64()).unwrap_or(5000),
            until: f.get("until").and_then(|v| v.as_str()).unwrap_or("").to_string(),
        })),
        "udp" => Ok(Capability::Udp(proto::UdpCap {
            host: f.get("host").and_then(|v| v.as_str()).unwrap_or("").to_string(),
            port: f.get("port").and_then(|v| v.as_u64()).unwrap_or(0) as u16,
            timeout_ms: f.get("timeout_ms").and_then(|v| v.as_u64()).unwrap_or(3000),
        })),
        "redis" => Ok(Capability::Redis(proto::RedisCap {
            host: f.get("host").and_then(|v| v.as_str()).unwrap_or("127.0.0.1").to_string(),
            port: f.get("port").and_then(|v| v.as_u64()).unwrap_or(6379) as u16,
            timeout_ms: f.get("timeout_ms").and_then(|v| v.as_u64()).unwrap_or(5000),
            password: f.get("password").and_then(|v| v.as_str()).unwrap_or("").to_string(),
        })),
        "nats" => Ok(Capability::Nats(proto::NatsCap {
            host: f.get("host").and_then(|v| v.as_str()).unwrap_or("127.0.0.1").to_string(),
            port: f.get("port").and_then(|v| v.as_u64()).unwrap_or(4222) as u16,
            timeout_ms: f.get("timeout_ms").and_then(|v| v.as_u64()).unwrap_or(5000),
        })),
        "mqtt" => Ok(Capability::Mqtt(proto::MqttCap {
            host: f.get("host").and_then(|v| v.as_str()).unwrap_or("127.0.0.1").to_string(),
            port: f.get("port").and_then(|v| v.as_u64()).unwrap_or(1883) as u16,
            client_id: f.get("client_id").and_then(|v| v.as_str()).unwrap_or("").to_string(),
            timeout_ms: f.get("timeout_ms").and_then(|v| v.as_u64()).unwrap_or(5000),
        })),
        "smtp" | "mail_send" => Ok(Capability::Smtp(proto::SmtpCap {
            host: f.get("host").and_then(|v| v.as_str()).unwrap_or("127.0.0.1").to_string(),
            port: f.get("port").and_then(|v| v.as_u64()).unwrap_or(25) as u16,
            from: f.get("from").and_then(|v| v.as_str()).unwrap_or("").to_string(),
            timeout_ms: f.get("timeout_ms").and_then(|v| v.as_u64()).unwrap_or(8000),
            username: f.get("username").and_then(|v| v.as_str()).unwrap_or("").to_string(),
            password: f.get("password").and_then(|v| v.as_str()).unwrap_or("").to_string(),
        })),
        "archive" => Ok(Capability::Archive(proto::ArchiveCap {
            path: f.get("path").and_then(|v| v.as_str()).unwrap_or("").to_string(),
            op: f.get("op").and_then(|v| v.as_str()).unwrap_or("list").to_string(),
            dest: f.get("dest").and_then(|v| v.as_str()).unwrap_or("").to_string(),
            timeout_ms: f.get("timeout_ms").and_then(|v| v.as_u64()).unwrap_or(30_000),
        })),
        "s3" | "object_store" => Ok(Capability::S3(service::S3Cap {
            endpoint: f.get("endpoint").and_then(|v| v.as_str()).unwrap_or("").to_string(),
            bucket: f.get("bucket").and_then(|v| v.as_str()).unwrap_or("").to_string(),
            prefix: f.get("prefix").and_then(|v| v.as_str()).unwrap_or("").to_string(),
            timeout_ms: f.get("timeout_ms").and_then(|v| v.as_u64()).unwrap_or(10_000),
            headers: f.get("headers").and_then(|v| v.as_object()).cloned().unwrap_or_default(),
        })),
        "prometheus" | "prom" => Ok(Capability::Prometheus(service::PrometheusCap {
            url: f.get("url").and_then(|v| v.as_str()).unwrap_or("").to_string(),
            timeout_ms: f.get("timeout_ms").and_then(|v| v.as_u64()).unwrap_or(8000),
        })),
        "kafka" => Ok(Capability::Kafka(service::KafkaCap {
            url: f.get("url").and_then(|v| v.as_str()).unwrap_or("").to_string(),
            topic: f.get("topic").and_then(|v| v.as_str()).unwrap_or("").to_string(),
            timeout_ms: f.get("timeout_ms").and_then(|v| v.as_u64()).unwrap_or(10_000),
        })),
        "pdf" => Ok(Capability::Pdf(service::PdfCap {
            path: f.get("path").and_then(|v| v.as_str()).unwrap_or("").to_string(),
            op: f.get("op").and_then(|v| v.as_str()).unwrap_or("text").to_string(),
            timeout_ms: f.get("timeout_ms").and_then(|v| v.as_u64()).unwrap_or(30_000),
        })),
        "sql" | "database" => Ok(Capability::Sql(service::SqlCap {
            driver: f.get("driver").and_then(|v| v.as_str()).unwrap_or("sqlite3").to_string(),
            dsn: f.get("dsn").and_then(|v| v.as_str()).unwrap_or("").to_string(),
            timeout_ms: f.get("timeout_ms").and_then(|v| v.as_u64()).unwrap_or(30_000),
        })),
        // ── singleton Chrome / CDP ──
        "browser" | "chrome_cdp" => Ok(Capability::Browser(browser::BrowserCap {
            endpoint: f.get("endpoint").and_then(|v| v.as_str()).unwrap_or("").to_string(),
            chrome_binary: f.get("chrome_binary").and_then(|v| v.as_str()).unwrap_or("").to_string(),
            profile_dir: f.get("profile_dir").and_then(|v| v.as_str()).unwrap_or("").to_string(),
            extension_path: f.get("extension_path").and_then(|v| v.as_str()).unwrap_or("").to_string(),
            launch: f.get("launch").and_then(|v| v.as_bool()).unwrap_or(false),
            attach: f.get("attach").and_then(|v| v.as_bool()).unwrap_or(false),
            startup_timeout_ms: f.get("startup_timeout_ms").and_then(|v| v.as_u64()).unwrap_or(15_000),
            timeout_ms: f.get("timeout_ms").and_then(|v| v.as_u64()).unwrap_or(20_000),
            max_text: f.get("max_text").and_then(|v| v.as_u64()).unwrap_or(6_000) as usize,
            max_owned_pages: f
                .get("max_owned_pages")
                .and_then(|v| v.as_u64())
                .unwrap_or(32)
                .clamp(1, 500) as usize,
            owned_idle_ms: f
                .get("owned_idle_ms")
                .and_then(|v| v.as_u64())
                .unwrap_or(5 * 60_000)
                .clamp(1_000, 24 * 60 * 60_000),
            human: f.get("human").and_then(|v| v.as_bool()).unwrap_or(true),
        })),
        "web_search" | "search" => Ok(Capability::WebSearch(web::WebSearchCap {
            endpoint: f.get("endpoint").and_then(|v| v.as_str()).unwrap_or("").to_string(),
            method: f.get("method").and_then(|v| v.as_str()).unwrap_or("GET").to_uppercase(),
            headers: f.get("headers").and_then(|v| v.as_object()).cloned().unwrap_or_default(),
            results_path: f.get("results_path").and_then(|v| v.as_str()).unwrap_or("").to_string(),
            query_param: f.get("query_param").and_then(|v| v.as_str()).unwrap_or("").to_string(),
            title_field: f.get("title_field").and_then(|v| v.as_str()).unwrap_or("").to_string(),
            url_field: f.get("url_field").and_then(|v| v.as_str()).unwrap_or("").to_string(),
            snippet_field: f.get("snippet_field").and_then(|v| v.as_str()).unwrap_or("").to_string(),
            timeout_ms: f.get("timeout_ms").and_then(|v| v.as_u64()).unwrap_or(15_000),
            max_results: f.get("max_results").and_then(|v| v.as_u64()).unwrap_or(10) as usize,
            max_bytes: f.get("max_bytes").and_then(|v| v.as_u64()).unwrap_or(256 << 10) as usize,
        })),
        "goal_runner" | "agent_goal" => Ok(Capability::GoalRunner(goal::GoalRunnerCap {
            runner: f.get("runner").and_then(|v| v.as_str()).unwrap_or("cxgo").to_string(),
            args: f
                .get("args")
                .and_then(|v| v.as_array())
                .map(|a| a.iter().filter_map(|x| x.as_str().map(str::to_string)).collect())
                .unwrap_or_default(),
            timeout_ms: f.get("timeout_ms").and_then(|v| v.as_u64()).unwrap_or(1_800_000),
            max_output: f.get("max_output").and_then(|v| v.as_u64()).unwrap_or(256 << 10) as usize,
            reports_dir: f.get("reports_dir").and_then(|v| v.as_str()).unwrap_or("").to_string(),
        })),
        // ── workflow-authored script plugins ──
        "plugin" | "script" => Ok(Capability::Plugin(plugin::PluginCap {
            plugin: f
                .get("plugin")
                .or_else(|| f.get("name"))
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_string(),
            dir: f.get("dir").and_then(|v| v.as_str()).unwrap_or("").to_string(),
            entry: f.get("entry").and_then(|v| v.as_str()).unwrap_or("").to_string(),
            op: f.get("op").and_then(|v| v.as_str()).unwrap_or("").to_string(),
            browser: f
                .get("browser")
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_string(),
            max_operations: f
                .get("max_operations")
                .and_then(|v| v.as_u64())
                .unwrap_or(0),
            timeout_ms: f.get("timeout_ms").and_then(|v| v.as_u64()).unwrap_or(0),
        })),
        "needle" => Ok(Capability::Needle(super::needle::NeedleCap {
            op: f.get("op").and_then(|v| v.as_str()).unwrap_or("extract").to_string(),
            cact: f.get("cact").and_then(|v| v.as_str()).map(|s| s.to_string()),
        })),
        "computer_use" | "computer-use" => Ok(Capability::ComputerUse(
            computer_use::ComputerUseCap {
                op: f
                    .get("op")
                    .and_then(|v| v.as_str())
                    .unwrap_or("fingerprint")
                    .to_string(),
                allowed_surface: f
                    .get("allowed_surface")
                    .and_then(|v| v.as_str())
                    .unwrap_or("")
                    .to_string(),
                allowed_fields: f
                    .get("allowed_fields")
                    .and_then(|v| v.as_array())
                    .map(|a| {
                        a.iter().filter_map(|x| x.as_str().map(str::to_string)).collect()
                    })
                    .unwrap_or_default(),
                expected: f.get("expected").cloned().unwrap_or(Value::Null),
                min_confidence: f
                    .get("min_confidence")
                    .and_then(|v| v.as_f64())
                    .unwrap_or(0.8),
            },
        )),
        "web_fetch" | "fetch_url" => Ok(Capability::WebFetch(web::WebFetchCap {
            format: f.get("format").and_then(|v| v.as_str()).unwrap_or("text").to_string(),
            headers: f.get("headers").and_then(|v| v.as_object()).cloned().unwrap_or_default(),
            timeout_ms: f.get("timeout_ms").and_then(|v| v.as_u64()).unwrap_or(20_000),
            max_bytes: f.get("max_bytes").and_then(|v| v.as_u64()).unwrap_or(256 << 10) as usize,
            endpoint: f.get("endpoint").and_then(|v| v.as_str()).unwrap_or("").to_string(),
            method: f.get("method").and_then(|v| v.as_str()).unwrap_or("POST").to_uppercase(),
            body_field: f.get("body_field").and_then(|v| v.as_str()).unwrap_or("url").to_string(),
            body_urls_array: f.get("body_urls_array").and_then(|v| v.as_bool()).unwrap_or(false),
            body: f.get("body").cloned().unwrap_or(Value::Null),
            result_path: f.get("result_path").and_then(|v| v.as_str()).unwrap_or("").to_string(),
            text_field: f.get("text_field").and_then(|v| v.as_str()).unwrap_or("").to_string(),
            title_field: f.get("title_field").and_then(|v| v.as_str()).unwrap_or("").to_string(),
        })),
        other => bail!(
            "capability {name:?}: unknown kind {other:?} (see docs/benchmarks/laya_workflow_capabilities_*.md)"
        ),
    }
}
