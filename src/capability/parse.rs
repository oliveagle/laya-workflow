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
        "plugin" | "script" => {
            // The plugin's identity is `plugin`/`name` (a search-layer id),
            // `dir` (an explicit path) or a script `entry`. `load_sources` needs
            // at least one; with none of the three the engine has nothing to
            // load -- every call bails "needs 'plugin' (a name), 'dir', or a
            // script 'entry'". A definition like that is dead the same way a
            // `rpc` without `url` is (P149): runnable only via the missing field,
            // which the definition never supplies.
            if !(nonempty_str("plugin")
                || nonempty_str("name")
                || nonempty_str("dir")
                || nonempty_str("entry"))
            {
                bail!(
                    "capability {name:?} (kind {kind:?}) names no plugin — none of `plugin`/`name` (a search-layer id), `dir` (a path) or a script `entry` is set, so the engine has nothing to load: the capability `validate`s but every call fails at run (`plugin capability needs 'plugin' (a name), 'dir', or a script 'entry'`)"
                );
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

/// Boolean options the parsers read via `Value::as_bool`. A present value that
/// is not a JSON boolean (`"false"`, `0`, `"yes"`) is silently replaced by the
/// option's default — the classic footgun where `"false"` (a truthy string in
/// most languages) silently keeps `readonly` / `human` at their defaults (P152).
/// `headers` is a boolean only for `csv` (elsewhere it is an object), so this
/// table is (kind, option), not a flat name list.
const BOOL_OPTIONS: &[(&str, &str)] = &[
    ("http", "expect_json"),
    ("http", "allow_error_status"),
    ("sqlite", "readonly"),
    ("db", "readonly"),
    ("csv", "headers"),
    ("notify", "bell"),
    ("notify", "timestamp"),
    ("browser", "launch"),
    ("browser", "attach"),
    ("browser", "human"),
    ("web_fetch", "body_urls_array"),
];

/// `metrics.what` is a comma-separated subset of this fixed vocabulary; the
/// runtime splits on `,` and `want.contains(&tok)` never matches an unknown
/// token, so a typo (`momery`, `cupy`) is silently dropped — the metric you
/// asked for is simply missing from the result (P153).
const METRICS_WHAT: &[&str] = &["mem", "load", "uptime", "cpu", "disk"];

/// Options the engine reads as a `Vec<String>` from a JSON array, keeping only
/// the string elements (`filter_map(|x| x.as_str())`). A non-string element — the
/// usual cause is an unquoted number (`"--max-turns", 8`) or bool — is silently
/// dropped, so the command runs without it; a non-array value drops the whole
/// list. `validate` refuses both (P154).
const STRING_ARRAY_OPTIONS: &[(&str, &str)] = &[
    ("exec", "argv"),
    ("agent", "command"),
    ("mcp", "command"),
    ("goal_runner", "args"),
    ("file", "allow_roots"),
    ("computer_use", "allowed_fields"),
];

/// Options the engine reads as a JSON object (`as_object`), keeping nothing on a
/// non-object value — the whole declaration (headers, env) is silently dropped
/// (P154).
const OBJECT_OPTIONS: &[(&str, &str)] = &[
    ("http", "headers"),
    ("exec", "env"),
    ("mcp", "headers"),
    ("s3", "headers"),
    ("web_search", "headers"),
    ("web_fetch", "headers"),
];

/// String options the parsers read with `Value::as_str` (every plain-string
/// option the engine has). A present value of any *other* JSON type is silently
/// replaced by the option's default (usually `""`), so a typo such as
/// `"format": 5` or `"separator": ["-"]` leaves the capability running with a
/// value nobody wrote — the declared value is thrown away and `validate` is
/// still green. This is the last face of the P128/P149/P150/P151/P152/P153/P154
/// family: those covered numeric domains, closed-set strings, booleans, string
/// arrays and objects, but a plain string option had never had its *type*
/// checked. Options whose value is validated as a closed set (P151) or
/// special-cased (`metrics.what`, `csv.delimiter`) are deliberately absent here;
/// (kind, option), compared through `canon_kind`.
const STRING_OPTIONS: &[(&str, &str)] = &[
    ("agent", "session"),
    ("agent", "url"),
    ("archive", "dest"),
    ("archive", "path"),
    ("browser", "chrome_binary"),
    ("browser", "endpoint"),
    ("browser", "extension_path"),
    ("browser", "profile_dir"),
    ("cache", "path"),
    ("chat", "auth_header"),
    ("chat", "auth_value"),
    ("chat", "model"),
    ("chat", "system"),
    ("chat", "url"),
    ("computer_use", "allowed_surface"),
    ("cron", "expr"),
    ("datetime", "format"),
    ("db", "alias"),
    ("db", "duckdb"),
    ("db", "endpoint"),
    ("db", "sqlite"),
    ("exec", "cwd"),
    ("goal_runner", "reports_dir"),
    ("graphql", "auth_header"),
    ("graphql", "auth_value"),
    ("graphql", "query"),
    ("graphql", "url"),
    ("http", "url"),
    ("kafka", "topic"),
    ("kafka", "url"),
    ("keyvalue", "key"),
    ("keyvalue", "path"),
    ("llm", "auth_header"),
    ("llm", "auth_value"),
    ("llm", "model"),
    ("llm", "system"),
    ("llm", "url"),
    ("mcp", "tool"),
    ("mcp", "url"),
    ("metrics", "disk_path"),
    ("mqtt", "client_id"),
    ("mqtt", "host"),
    ("nats", "host"),
    ("needle", "cact"),
    ("notify", "path"),
    ("notify", "sound"),
    ("notify", "subtitle"),
    ("notify", "title"),
    ("pdf", "path"),
    ("plugin", "browser"),
    ("plugin", "dir"),
    ("plugin", "entry"),
    ("plugin", "name"),
    ("plugin", "plugin"),
    ("prom", "url"),
    ("prometheus", "url"),
    ("queue", "path"),
    ("redis", "host"),
    ("redis", "password"),
    ("rpc", "method"),
    ("rpc", "url"),
    ("s3", "bucket"),
    ("s3", "endpoint"),
    ("s3", "prefix"),
    ("script", "browser"),
    ("script", "dir"),
    ("script", "entry"),
    ("script", "name"),
    ("script", "plugin"),
    ("shell", "command"),
    ("shell", "cwd"),
    ("sql", "dsn"),
    ("smtp", "from"),
    ("smtp", "host"),
    ("smtp", "password"),
    ("smtp", "username"),
    ("sqlite", "db"),
    ("sse", "url"),
    ("tcp", "host"),
    ("tcp", "until"),
    ("text", "pattern"),
    ("text", "replacement"),
    ("text", "separator"),
    ("udp", "host"),
    ("vector", "collection"),
    ("vector", "url"),
    ("web_fetch", "body_field"),
    ("web_fetch", "endpoint"),
    ("web_fetch", "result_path"),
    ("web_fetch", "text_field"),
    ("web_fetch", "title_field"),
    ("web_search", "endpoint"),
    ("web_search", "query_param"),
    ("web_search", "results_path"),
    ("web_search", "snippet_field"),
    ("web_search", "title_field"),
    ("web_search", "url_field"),
    ("webhook", "sign_header"),
    ("webhook", "sign_secret"),
    ("webhook", "url"),
    ("xml", "tag"),
];

/// Signed-integer options the parsers read with `Value::as_i64`; a present value
/// of any other type is silently defaulted, so `"offset_secs": "60"` quietly
/// means `0` (P159).
const I64_OPTIONS: &[(&str, &str)] =
    &[("cache", "ttl_secs"), ("datetime", "offset_secs"), ("rpc", "id")];

/// Every `(kind, option, type)` the per-kind option-*type* tables declare, with
/// `type` the JSON type the engine reads the option as. Exposed so the
/// regression can prove each entry is still live against the parser the tables
/// describe: a hand-maintained table that drifts from the code is exactly the
/// failure P128/P131 exist to catch — an entry for an option its kind no longer
/// reads would sit here silently and give the illusion of coverage.
pub(crate) fn option_type_sites() -> Vec<(&'static str, &'static str, &'static str)> {
    let mut v: Vec<(&'static str, &'static str, &'static str)> = Vec::new();
    for (k, o) in BOOL_OPTIONS {
        v.push((k, o, "bool"));
    }
    for (k, o) in STRING_ARRAY_OPTIONS {
        v.push((k, o, "str_array"));
    }
    for (k, o) in OBJECT_OPTIONS {
        v.push((k, o, "object"));
    }
    for (k, o) in STRING_OPTIONS {
        v.push((k, o, "string"));
    }
    for (k, o) in I64_OPTIONS {
        v.push((k, o, "i64"));
    }
    v
}

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
    let ckind = canon_kind(kind);
    for (k, opt) in BOOL_OPTIONS {
        if *k == ckind && obj.contains_key(*opt) && obj.get(*opt).and_then(Value::as_bool).is_none() {
            bail!(
                "capability {name:?} (kind {kind:?}) option {opt:?} must be true or false — the engine reads it as a boolean, so a non-boolean value (e.g. the string \"false\") is silently replaced by the default"
            );
        }
    }
    for (k, opt) in STRING_ARRAY_OPTIONS {
        if *k == ckind {
            if let Some(v) = obj.get(*opt) {
                match v.as_array() {
                    None => bail!(
                        "capability {name:?} (kind {kind:?}) option {opt:?} must be an array of strings — the engine reads it as a string list and ignores a non-array value, so the whole list is silently dropped"
                    ),
                    Some(arr) => {
                        if let Some(bad) = arr.iter().find(|x| !x.is_string()) {
                            bail!(
                                "capability {name:?} (kind {kind:?}) option {opt:?} has a non-string element {bad} — the engine keeps only the string elements, so this one is silently dropped (a quoted number such as 8 is the usual cause)"
                            );
                        }
                    }
                }
            }
        }
    }
    for (k, opt) in OBJECT_OPTIONS {
        if *k == ckind && obj.contains_key(*opt) && obj.get(*opt).and_then(Value::as_object).is_none() {
            bail!(
                "capability {name:?} (kind {kind:?}) option {opt:?} must be a JSON object — the engine reads it as a map and ignores a non-object value, so the whole declaration is silently dropped"
            );
        }
    }
    for (k, opt) in STRING_OPTIONS {
        if *k == ckind && obj.contains_key(*opt) && obj.get(*opt).and_then(Value::as_str).is_none() {
            bail!(
                "capability {name:?} (kind {kind:?}) option {opt:?} must be a string — the engine reads it with `as_str()` and falls back to its default on any other JSON type, so the declared value is silently discarded"
            );
        }
    }
    for (k, opt) in I64_OPTIONS {
        if *k == ckind
            && obj.contains_key(*opt)
            && obj.get(*opt).and_then(Value::as_i64).is_none()
        {
            bail!(
                "capability {name:?} (kind {kind:?}) option {opt:?} must be an integer — the engine reads it with `as_i64()` and falls back to its default on any other JSON type, so the declared value is silently discarded"
            );
        }
    }
    if ckind == "metrics" {
        if let Some(v) = obj.get("what") {
            let Some(s) = v.as_str() else {
                bail!(
                    "capability {name:?} (kind {kind:?}) option \"what\" must be a comma-separated string — the engine reads it as a string and ignores a non-string value"
                );
            };
            for tok in s.split(',').map(str::trim).filter(|t| !t.is_empty()) {
                if !METRICS_WHAT.contains(&tok) {
                    let list = METRICS_WHAT.iter().map(|m| format!("{m:?}")).collect::<Vec<_>>().join(" | ");
                    bail!(
                        "capability {name:?} (kind {kind:?}) option \"what\" has {tok:?}, which the engine never reads ({list}) — it is silently dropped, so the metric you asked for is missing from the result"
                    );
                }
            }
        }
    }
    if ckind == "csv" {
        if let Some(v) = obj.get("delimiter") {
            let Some(s) = v.as_str() else {
                bail!(
                    "capability {name:?} (kind {kind:?}) option \"delimiter\" must be a single-character string — the engine reads it via `chars().next()`, so a non-string value is silently replaced by the default (',')"
                );
            };
            let n = s.chars().count();
            if n > 1 {
                let first = s.chars().next().unwrap();
                bail!(
                    "capability {name:?} (kind {kind:?}) option \"delimiter\" is {s:?} ({n} characters) — the engine keeps only the first character ({first:?}) via `chars().next()`, so the rest is silently dropped"
                );
            }
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

/// Closed-set string options: the runtime's own dispatch accepts only a fixed
/// set of values, so a declared value outside it is never honoured. `validate`
/// refuses it instead — the same "a declared value the engine throws away"
/// class as P128/P149/P150, now about a *string* option's value.
///
/// `allow_empty` mirrors the engine exactly: `""` is accepted only where it
/// already resolves to a member (an explicit `if empty { default }` in the
/// runtime or an `effective_op` fallback equal to the kind's documented
/// default). Where `""` would bail, it is refused here too.
///
/// `fallback`: `Some(x)` means an out-of-set value does **not** bail — the
/// engine silently substitutes `x` (P152: `pdf.op` → `text`,
/// `web_fetch.format` → `text`, `web_search.method` → `GET`), so the typo runs
/// to completion with the wrong behaviour. `None` means the call bails.
///
/// (kind, option, accepted values, allow_empty, fallback)
const CLOSED_SETS: &[(&str, &str, &[&str], bool, Option<&str>)] = &[
    ("db", "op", &["query", "exec", "analytics", "sync", "tables"], true, None),
    ("db", "mode", &["embed", "server"], true, None),
    ("db", "format", &["json", "csv", "md", "markdown"], true, None),
    ("datetime", "op", &["now", "add", "format", "parse"], true, None),
    (
        "text",
        "op",
        &[
            "length", "upper", "lower", "trim", "split", "extract", "spans", "replace", "hash",
            "base64_encode", "base64_decode", "json_get",
        ],
        false,
        None,
    ),
    ("text", "hash", &["fnv1a64"], true, None),
    ("file", "op", &["read", "write", "append", "stat", "list"], false, None),
    (
        "json",
        "op",
        &["pick", "merge", "patch", "path_set", "flatten", "sort_keys", "pick_by"],
        true,
        None,
    ),
    ("csv", "op", &["parse", "generate"], false, None),
    ("xml", "op", &["tags", "text", "attrs"], false, None),
    ("markdown", "op", &["headings", "code_blocks", "links", "outline"], false, None),
    ("diff", "op", &["lines", "json"], false, None),
    ("hash", "op", &["sha256", "sha256_hex", "fnv1a64", "crc32"], false, None),
    ("graph", "op", &["reachable", "bfs", "toposort"], false, None),
    ("tokenize", "op", &["count", "split"], false, None),
    ("math", "op", &["eval", "stats"], true, None),
    ("vector", "op", &["upsert", "search"], true, None),
    ("keyvalue", "op", &["get", "has", "set", "del", "incr", "list"], false, None),
    ("cache", "op", &["get", "set", "ttl", "purge"], false, None),
    ("queue", "op", &["push", "pop", "peek", "length", "clear"], true, None),
    ("archive", "op", &["list", "extract"], false, None),
    ("sqlite", "op", &["query", "exec"], false, None),
    ("sql", "driver", &["sqlite3", "psql", "mysql"], true, None),
    (
        "needle",
        "op",
        &[
            "extract", "needle_extract", "embed", "needle_embed", "complete", "needle_complete",
        ],
        false,
        None,
    ),
    ("computer_use", "op", &["fingerprint", "step", "verify"], true, None),
    ("mcp", "transport", &["http", "stdio"], false, None),
    ("agent", "transport", &["http", "stdio"], false, None),
    (
        "notify",
        "channel",
        &["log", "file", "macos", "os", "osx", "notification", "both", "auto"],
        true,
        None,
    ),
    ("http", "method", &["GET", "POST", "PUT", "DELETE", "PATCH"], false, None),
    // P152: out-of-set values that silently fall back to a default instead of
    // bailing — the typo runs to completion with the wrong behaviour.
    ("pdf", "op", &["text", "info"], true, Some("text")),
    ("web_fetch", "format", &["text", "raw", "markdown"], true, Some("text")),
    ("web_search", "method", &["GET", "POST"], true, Some("GET")),
    // P153: `goal_runner.runner` is a fixed two-entry set; an unknown name is
    // rejected at run (`resolve_runner` bails), so validate refuses it here.
    ("goal_runner", "runner", &["cxgo", "cmdgo"], true, None),
];

/// Fold a kind alias to the canonical name the closed-set table uses.
fn canon_kind(k: &str) -> &str {
    match k {
        "md" => "markdown",
        "tokens" => "tokenize",
        "kv" => "keyvalue",
        "database" => "sql",
        "computer-use" => "computer_use",
        "notify_local" => "notify",
        "mail_send" => "smtp",
        "app_server" => "agent",
        "chrome_cdp" => "browser",
        "fetch_url" => "web_fetch",
        "search" => "web_search",
        "object_store" => "s3",
        "sysinfo" => "metrics",
        "agent_goal" => "goal_runner",
        other => other,
    }
}

fn check_option_values(name: &str, kind: &str, def: &Value) -> Result<()> {
    let Some(obj) = def.as_object() else {
        return Ok(());
    };
    let kind = canon_kind(kind);
    for (k, option, allowed, allow_empty, fallback) in CLOSED_SETS {
        if *k != kind {
            continue;
        }
        let Some(v) = obj.get(*option) else {
            continue;
        };
        let Some(s) = v.as_str() else {
            bail!(
                "capability {name:?} (kind {kind:?}) option {option:?} must be a string — the engine reads it as a string and ignores a non-string value, so `validate` would be green while the option is silently dropped"
            );
        };
        // `method` is upper-cased by the parser before it is stored (`get` ==
        // `GET`), so compare the same way here.
        let value = if *option == "method" {
            s.to_uppercase()
        } else {
            s.to_string()
        };
        if value.is_empty() && *allow_empty {
            continue;
        }
        if !allowed.contains(&value.as_str()) {
            let list = allowed
                .iter()
                .map(|a| format!("{a:?}"))
                .collect::<Vec<_>>()
                .join(" | ");
            match fallback {
                Some(fb) => bail!(
                    "capability {name:?} (kind {kind:?}) option {option:?} value {s:?} is not one this engine accepts ({list}) — it silently falls back to {fb:?}, so the typo runs to completion with the wrong behaviour"
                ),
                None => bail!(
                    "capability {name:?} (kind {kind:?}) option {option:?} value {s:?} is not one this engine accepts ({list}) — a value outside the set is never honoured: the call bails, or a default is silently substituted"
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
    check_option_values(name, kind, def)?;
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
            plugin: {
                // Read *both* keys unconditionally. The chain this replaced —
                // `f.get("plugin").or_else(|| f.get("name"))` — called
                // `get("name")` only when `plugin` was absent, so
                // `Fields::read` never saw it and a definition that set *both*
                // (the two names the docs give for the same thing) was refused
                // by the P128 "declares option(s) this engine never reads"
                // check, with `name` named as though it were a typo. The
                // semantics are unchanged: `plugin` decides when it is present.
                let present = f.get("plugin").is_some();
                let named = f.get("plugin").and_then(|v| v.as_str()).unwrap_or("").to_string();
                let alias = f.get("name").and_then(|v| v.as_str()).unwrap_or("").to_string();
                if present { named } else { alias }
            },
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
