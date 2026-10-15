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

/// The JSON type a read site reads an option as. When a declared value has a
/// different type, the engine silently falls back to a default and the
/// declaration becomes a no-op — the whole P128/P149/P150/P151/P152/P153/P154
/// family. Declaring the type *at the read site* (P160) is what keeps the check
/// from drifting from the parser: there is no separate table to stay complete,
/// and a key may legitimately need a different type per kind (`csv.headers` is a
/// boolean, `http.headers` an object) because the read site knows which one it
/// is.
#[derive(Clone, Copy, PartialEq, Eq)]
enum OptType {
    Str,
    U64,
    I64,
    F64,
    Bool,
    StrArray,
    Object,
}

impl OptType {
    fn accepts(self, v: &Value) -> bool {
        match self {
            OptType::Str => v.is_string(),
            OptType::U64 => v.as_u64().is_some(),
            OptType::I64 => v.as_i64().is_some(),
            OptType::F64 => v.as_f64().is_some(),
            OptType::Bool => v.is_boolean(),
            OptType::StrArray => v.as_array().is_some(),
            OptType::Object => v.is_object(),
        }
    }

    /// Why a mismatch is refused, in the same words the P150–P159 tables used:
    /// the message names the read (`as_str()` …) and the silent fallback.
    fn mismatch(self) -> &'static str {
        match self {
            OptType::Str => "must be a string — the engine reads it with `as_str()` and falls back to its default on any other JSON type, so the declared value is silently discarded",
            OptType::U64 => "must be a non-negative integer — the engine reads it as an unsigned number, so a fractional / negative / non-numeric value is silently replaced by the default",
            OptType::I64 => "must be an integer — the engine reads it with `as_i64()` and falls back to its default on any other JSON type, so the declared value is silently discarded",
            OptType::F64 => "must be a number — the engine reads it as a float, so a non-numeric value is silently replaced by the default",
            OptType::Bool => "must be true or false — the engine reads it as a boolean, so a non-boolean value (e.g. the string \"false\") is silently replaced by the default",
            OptType::StrArray => "must be an array of strings — the engine reads it as a string list and ignores a non-array value, so the whole list is silently dropped",
            OptType::Object => "must be a JSON object — the engine reads it as a map and ignores a non-object value, so the whole declaration is silently dropped",
        }
    }

    /// The order the P150–P159 tables checked in, kept so that a spec with
    /// several wrong-typed options still reports the same one first.
    fn rank(self) -> u8 {
        match self {
            OptType::U64 => 0,
            OptType::F64 => 1,
            OptType::Bool => 2,
            OptType::StrArray => 3,
            OptType::Object => 4,
            OptType::Str => 5,
            OptType::I64 => 6,
        }
    }
}

/// A declared value whose JSON type is not the one its read site expects.
/// Recorded while the kind is parsed; [`report_wrong_types`] refuses the spec.
enum Wrong {
    Type { key: String, want: OptType },
    Elem { key: String, bad: String },
}

/// Records which option keys a kind's parser actually read, so [`parse_cap`] can
/// refuse a declared option nobody reads.
///
/// An option the engine never consumes is a silent no-op: `max_operatons`
/// leaves a plugin at the default cap and `chorme_binary` leaves Chrome to
/// `$PATH`, and nothing says so. Same rule as P126's "a declared heuristic field
/// with no value is a clean miss": a declaration nobody reads is a comment, not
/// config (P128).
///
/// Every read goes through a typed accessor (P160), so a declared value of the
/// wrong JSON type is refused as well — see [`OptType`].
struct Fields<'a> {
    def: &'a Value,
    seen: Vec<String>,
    wrong: Vec<Wrong>,
}

impl<'a> Fields<'a> {
    fn new(def: &'a Value) -> Self {
        Self { def, seen: Vec::new(), wrong: Vec::new() }
    }

    /// Records `key` as read and asserts its value is the JSON type the read
    /// site declares. On a mismatch the site keeps its own `unwrap_or` fallback
    /// (the engine's behaviour is unchanged), but the mismatch is recorded so
    /// [`report_wrong_types`] refuses the spec. The returned reference borrows
    /// the spec (`'a`), not `self`, so call sites keep their
    /// `and_then`/`unwrap_or` chains unchanged.
    fn typed(&mut self, key: &str, want: OptType) -> Option<&'a Value> {
        self.seen.push(key.to_string());
        let v = self.def.get(key)?;
        if want.accepts(v) {
            Some(v)
        } else {
            self.wrong.push(Wrong::Type { key: key.to_string(), want });
            None
        }
    }

    fn s(&mut self, key: &str) -> Option<&'a str> {
        self.typed(key, OptType::Str)?.as_str()
    }

    fn u64(&mut self, key: &str) -> Option<u64> {
        self.typed(key, OptType::U64)?.as_u64()
    }

    fn i64(&mut self, key: &str) -> Option<i64> {
        self.typed(key, OptType::I64)?.as_i64()
    }

    fn f64(&mut self, key: &str) -> Option<f64> {
        self.typed(key, OptType::F64)?.as_f64()
    }

    fn b(&mut self, key: &str) -> Option<bool> {
        self.typed(key, OptType::Bool)?.as_bool()
    }

    fn obj(&mut self, key: &str) -> Option<&'a serde_json::Map<String, Value>> {
        self.typed(key, OptType::Object)?.as_object()
    }

    /// A string list. A non-string element is recorded too, because the engine's
    /// `filter_map(|x| x.as_str())` silently drops it (P154).
    fn arr(&mut self, key: &str) -> Option<&'a Vec<Value>> {
        let a = self.typed(key, OptType::StrArray)?.as_array()?;
        if let Some(bad) = a.iter().find(|x| !x.is_string()) {
            self.wrong.push(Wrong::Elem { key: key.to_string(), bad: format!("{bad}") });
        }
        Some(a)
    }

    /// A read whose type is *not* asserted here: the value is consumed opaquely
    /// (`body`, `expected`, `variables`), or a value check owns it and already
    /// refuses a non-string (`port`, `metrics.what`, `csv.delimiter`). Recorded
    /// as read so the P128 "never reads" check still sees it.
    ///
    /// The closed-set options (`op`, `method`, `format`, `mode`, `hash`,
    /// `channel`, `driver`, `runner`, `transport`) used to be read here too;
    /// P167 typed them, because the value table reads the option with `as_str()`
    /// and so a `{"kind":"datetime","op":4}` slipped past it and compiled to the
    /// default `op:"now"`.
    fn any(&mut self, key: &str) -> Option<&'a Value> {
        self.seen.push(key.to_string());
        self.def.get(key)
    }

    /// Whether the spec declares `key` — a presence test, not a read. The key's
    /// type is asserted where it is actually read (`command`, `plugin`).
    fn has(&mut self, key: &str) -> bool {
        self.seen.push(key.to_string());
        self.def.get(key).is_some()
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

/// Kinds whose `port` is stored as `u16`: any value above 65535 wraps, so
/// `70000` quietly connects to `4464`. The port must be a real port.
const PORT_KINDS: &[&str] = &["tcp", "udp", "redis", "nats", "mqtt", "smtp", "mail_send"];

/// `metrics.what` is a comma-separated subset of this fixed vocabulary; the
/// runtime splits on `,` and `want.contains(&tok)` never matches an unknown
/// token, so a typo (`momery`, `cupy`) is silently dropped — the metric you
/// asked for is simply missing from the result (P153).
const METRICS_WHAT: &[&str] = &["mem", "load", "uptime", "cpu", "disk"];

/// Refuses the first declared option whose value has the wrong JSON type at its
/// read site (P160). Ordered like the P150–P159 tables it replaces (`U64`
/// first, then `F64`, …) so a spec with several wrong values still reports the
/// same option first.
fn report_wrong_types(name: &str, kind: &str, wrong: &[Wrong]) -> Result<()> {
    fn rank(w: &Wrong) -> u8 {
        match w {
            Wrong::Type { want, .. } => want.rank(),
            Wrong::Elem { .. } => OptType::StrArray.rank(),
        }
    }
    let mut best: Option<&Wrong> = None;
    for w in wrong {
        if best.is_none_or(|b| rank(w) < rank(b)) {
            best = Some(w);
        }
    }
    let Some(w) = best else {
        return Ok(());
    };
    match w {
        Wrong::Type { key, want } => bail!(
            "capability {name:?} (kind {kind:?}) option {key:?} {}",
            (*want).mismatch()
        ),
        Wrong::Elem { key, bad } => bail!(
            "capability {name:?} (kind {kind:?}) option {key:?} has a non-string element {bad} — the engine keeps only the string elements, so this one is silently dropped (a quoted number such as 8 is the usual cause)"
        ),
    }
}

/// The option *domains* no plain type check can express: `metrics.what` has a
/// fixed vocabulary, `csv.delimiter` must be one character, and `port` must fit
/// in a `u16`. The types themselves are asserted at each read site (P160).
fn check_option_domains(name: &str, kind: &str, def: &Value) -> Result<()> {
    let Some(obj) = def.as_object() else {
        return Ok(());
    };
    let ckind = canon_kind(kind);
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
    report_wrong_types(name, kind, &fields.wrong)?;
    check_option_domains(name, kind, def)?;
    check_option_values(name, kind, def)?;
    Ok(cap)
}

/// The per-kind parser: every option it reads goes through [`Fields::get`], so
/// [`parse_cap`] can prove a declared option was consumed.
fn parse_kind(name: &str, kind: &str, f: &mut Fields) -> Result<Capability> {
    match kind {
        "http" => Ok(Capability::Http(HttpCap {
            method: f.s("method").unwrap_or("POST").to_uppercase(),
            url: f
                .s("url")
                .ok_or_else(|| anyhow!("capability {name:?} (http) missing 'url'"))?
                .to_string(),
            headers: f.obj("headers").cloned().unwrap_or_default(),
            body: f.any("body").cloned(),
            timeout_ms: f.u64("timeout_ms").unwrap_or(15_000),
            expect_json: f.b("expect_json").unwrap_or(true),
            allow_error_status: f
                .b("allow_error_status")
                .unwrap_or(false),
        })),
        "exec" => Ok(Capability::Exec(ExecCap {
            argv: f
                .arr("argv")
                .map(|a| a.iter().filter_map(|x| x.as_str().map(str::to_string)).collect())
                .ok_or_else(|| anyhow!("capability {name:?} (exec) missing 'argv'"))?,
            cwd: f.s("cwd").map(str::to_string),
            env: f.obj("env").cloned().unwrap_or_default(),
            timeout_ms: f.u64("timeout_ms").unwrap_or(30_000),
            max_output: f.u64("max_output").unwrap_or(256 << 10) as usize,
        })),
        "agent" | "app_server" => Ok(Capability::Agent(AgentCap {
            transport: f.s("transport")
                .unwrap_or(if f.has("command") { "stdio" } else { "http" })
                .to_string(),
            url: f.s("url").map(str::to_string),
            command: f
                .arr("command")
                .map(|a| a.iter().filter_map(|x| x.as_str().map(str::to_string)).collect()),
            session: f.s("session").map(str::to_string),
            timeout_ms: f.u64("timeout_ms").unwrap_or(30_000),
            max_output: f.u64("max_output").unwrap_or(256 << 10) as usize,
        })),
        // ── local / pure ──
        "datetime" => Ok(Capability::Datetime(local::DatetimeCap {
            op: f.s("op").unwrap_or("now").to_string(),
            format: f.s("format").unwrap_or("").to_string(),
            offset_secs: f.i64("offset_secs").unwrap_or(0),
        })),
        "text" => Ok(Capability::Text(local::TextCap {
            op: f.s("op").unwrap_or("length").to_string(),
            pattern: f.s("pattern").unwrap_or("").to_string(),
            replacement: f.s("replacement").unwrap_or("").to_string(),
            separator: f.s("separator").unwrap_or("").to_string(),
            hash: f.s("hash").unwrap_or("").to_string(),
        })),
        "file" => Ok(Capability::File(local::FileCap {
            op: f.s("op").unwrap_or("read").to_string(),
            allow_roots: f
                .arr("allow_roots")
                .map(|a| a.iter().filter_map(|x| x.as_str().map(std::path::PathBuf::from)).collect())
                .unwrap_or_default(),
            max_bytes: f.u64("max_bytes").unwrap_or(1 << 20) as usize,
        })),
        "sqlite" => Ok(Capability::Sqlite(local::SqliteCap {
            db: f.s("db").unwrap_or("").to_string(),
            op: f.s("op").unwrap_or("query").to_string(),
            readonly: f.b("readonly").unwrap_or(true),
            timeout_ms: f.u64("timeout_ms").unwrap_or(10_000),
        })),
        "db" => Ok(Capability::Db(db::DbCap {
            sqlite: f.s("sqlite").unwrap_or("").to_string(),
            duckdb: f.s("duckdb").unwrap_or("").to_string(),
            alias: f.s("alias").unwrap_or("").to_string(),
            op: f.s("op").unwrap_or("query").to_string(),
            readonly: f.b("readonly").unwrap_or(true),
            format: f.s("format").unwrap_or("json").to_string(),
            mode: f.s("mode").unwrap_or("embed").to_string(),
            endpoint: f.s("endpoint").unwrap_or("").to_string(),
            timeout_ms: f.u64("timeout_ms").unwrap_or(60_000),
        })),
        "shell" => Ok(Capability::Shell(local::ShellCap {
            command: f
                .s("command")
                .ok_or_else(|| anyhow!("capability {name:?} (shell) missing 'command'"))?
                .to_string(),
            cwd: f.s("cwd").map(str::to_string),
            timeout_ms: f.u64("timeout_ms").unwrap_or(30_000),
        })),
        // ── network ──
        "rpc" => Ok(Capability::Rpc(net::RpcCap {
            url: f.s("url").unwrap_or("").to_string(),
            method: f.s("method").unwrap_or("").to_string(),
            timeout_ms: f.u64("timeout_ms").unwrap_or(15_000),
            id: f.i64("id"),
        })),
        "graphql" => Ok(Capability::Graphql(net::GraphqlCap {
            url: f.s("url").unwrap_or("").to_string(),
            query: f.s("query").unwrap_or("").to_string(),
            variables: f.any("variables").cloned().unwrap_or_else(|| json!({})),
            timeout_ms: f.u64("timeout_ms").unwrap_or(15_000),
            auth_header: f.s("auth_header").unwrap_or("").to_string(),
            auth_value: f.s("auth_value").unwrap_or("").to_string(),
        })),
        "llm" | "chat" => Ok(Capability::Llm(net::LlmCap {
            url: f.s("url").unwrap_or("").to_string(),
            model: f.s("model").unwrap_or("").to_string(),
            system: f.s("system").unwrap_or("").to_string(),
            max_tokens: f.u64("max_tokens").unwrap_or(256),
            temperature: f.f64("temperature").unwrap_or(0.0),
            timeout_ms: f.u64("timeout_ms").unwrap_or(30_000),
            auth_header: f.s("auth_header").unwrap_or("authorization").to_string(),
            auth_value: f.s("auth_value").unwrap_or("").to_string(),
        })),
        "mcp" => Ok(Capability::Mcp(net::McpCap {
            headers: f.obj("headers").cloned().unwrap_or_default(),
            transport: f.s("transport")
                .unwrap_or(if f.has("command") { "stdio" } else { "http" })
                .to_string(),
            url: f.s("url").map(str::to_string),
            command: f
                .arr("command")
                .map(|a| a.iter().filter_map(|x| x.as_str().map(str::to_string)).collect()),
            tool: f.s("tool").unwrap_or("").to_string(),
            timeout_ms: f.u64("timeout_ms").unwrap_or(30_000),
        })),
        "vector" => Ok(Capability::Vector(net::VectorCap {
            url: f.s("url").unwrap_or("").to_string(),
            op: f.s("op").unwrap_or("search").to_string(),
            collection: f.s("collection").unwrap_or("").to_string(),
            timeout_ms: f.u64("timeout_ms").unwrap_or(15_000),
        })),
        "webhook" => Ok(Capability::Webhook(net::WebhookCap {
            url: f.s("url").unwrap_or("").to_string(),
            timeout_ms: f.u64("timeout_ms").unwrap_or(10_000),
            sign_header: f.s("sign_header").unwrap_or("").to_string(),
            sign_secret: f.s("sign_secret").unwrap_or("").to_string(),
        })),
        "sse" => Ok(Capability::Sse(net::SseCap {
            url: f.s("url").unwrap_or("").to_string(),
            timeout_ms: f.u64("timeout_ms").unwrap_or(15_000),
            max_events: f.u64("max_events").unwrap_or(100) as usize,
        })),
        "passthrough" | "noop" => Ok(Capability::Passthrough),
        "json" => Ok(Capability::Json(data::JsonCap {
            op: f.s("op").unwrap_or("pick").to_string(),
        })),
        "csv" => Ok(Capability::Csv(data::CsvCap {
            op: f.s("op").unwrap_or("parse").to_string(),
            delimiter: f
                .any("delimiter").and_then(|v| v.as_str())
                .and_then(|s| s.chars().next())
                .unwrap_or(','),
            headers: f.b("headers").unwrap_or(true),
        })),
        "xml" => Ok(Capability::Xml(data::XmlCap {
            tag: f.s("tag").unwrap_or("").to_string(),
            op: f.s("op").unwrap_or("tags").to_string(),
        })),
        "markdown" | "md" => Ok(Capability::Markdown(data::MarkdownCap {
            op: f.s("op").unwrap_or("headings").to_string(),
        })),
        "diff" => Ok(Capability::Diff(data::DiffCap {
            op: f.s("op").unwrap_or("lines").to_string(),
        })),
        "validate" | "schema" => Ok(Capability::Validate(data::ValidateCap)),
        "math" => Ok(Capability::Math(data::MathCap {
            op: f.s("op").unwrap_or("eval").to_string(),
        })),
        "hash" => Ok(Capability::Hash(data::HashCap {
            op: f.s("op").unwrap_or("sha256").to_string(),
        })),
        "graph" => Ok(Capability::Graph(data::GraphCap {
            op: f.s("op").unwrap_or("reachable").to_string(),
        })),
        "tokenize" | "tokens" => Ok(Capability::Tokenize(data::TokenizeCap {
            op: f.s("op").unwrap_or("count").to_string(),
            chars_per_token: f.f64("chars_per_token").unwrap_or(4.0),
        })),
        "cron" => Ok(Capability::Cron(data::CronCap {
            expr: f.s("expr").unwrap_or("").to_string(),
        })),
        "keyvalue" | "kv" => Ok(Capability::KeyValue(store::KeyValueCap {
            path: f.s("path").unwrap_or("").to_string(),
            op: f.s("op").unwrap_or("get").to_string(),
            key: f.s("key").unwrap_or("").to_string(),
        })),
        "cache" => Ok(Capability::Cache(store::CacheCap {
            path: f.s("path").unwrap_or("").to_string(),
            op: f.s("op").unwrap_or("get").to_string(),
            ttl_secs: f.i64("ttl_secs").unwrap_or(0),
        })),
        "queue" => Ok(Capability::Queue(store::QueueCap {
            path: f.s("path").unwrap_or("").to_string(),
            op: f.s("op").unwrap_or("length").to_string(),
        })),
        "metrics" | "sysinfo" => Ok(Capability::Metrics(sys::MetricsCap {
            what: f.any("what").and_then(|v| v.as_str()).unwrap_or("").to_string(),
            disk_path: f.s("disk_path").unwrap_or("").to_string(),
        })),
        "notify" | "notify_local" => Ok(Capability::NotifyLocal(sys::NotifyLocalCap {
            path: f.s("path").unwrap_or("").to_string(),
            bell: f.b("bell").unwrap_or(false),
            timestamp: f.b("timestamp").unwrap_or(true),
            channel: f.s("channel").unwrap_or("").to_string(),
            title: f.s("title").unwrap_or("").to_string(),
            subtitle: f.s("subtitle").unwrap_or("").to_string(),
            sound: f.s("sound").unwrap_or("").to_string(),
            timeout_ms: f.u64("timeout_ms").unwrap_or(10_000),
        })),
        "tcp" => Ok(Capability::Tcp(proto::TcpCap {
            host: f.s("host").unwrap_or("").to_string(),
            port: f.any("port").and_then(|v| v.as_u64()).unwrap_or(0) as u16,
            timeout_ms: f.u64("timeout_ms").unwrap_or(5000),
            until: f.s("until").unwrap_or("").to_string(),
        })),
        "udp" => Ok(Capability::Udp(proto::UdpCap {
            host: f.s("host").unwrap_or("").to_string(),
            port: f.any("port").and_then(|v| v.as_u64()).unwrap_or(0) as u16,
            timeout_ms: f.u64("timeout_ms").unwrap_or(3000),
        })),
        "redis" => Ok(Capability::Redis(proto::RedisCap {
            host: f.s("host").unwrap_or("127.0.0.1").to_string(),
            port: f.any("port").and_then(|v| v.as_u64()).unwrap_or(6379) as u16,
            timeout_ms: f.u64("timeout_ms").unwrap_or(5000),
            password: f.s("password").unwrap_or("").to_string(),
        })),
        "nats" => Ok(Capability::Nats(proto::NatsCap {
            host: f.s("host").unwrap_or("127.0.0.1").to_string(),
            port: f.any("port").and_then(|v| v.as_u64()).unwrap_or(4222) as u16,
            timeout_ms: f.u64("timeout_ms").unwrap_or(5000),
        })),
        "mqtt" => Ok(Capability::Mqtt(proto::MqttCap {
            host: f.s("host").unwrap_or("127.0.0.1").to_string(),
            port: f.any("port").and_then(|v| v.as_u64()).unwrap_or(1883) as u16,
            client_id: f.s("client_id").unwrap_or("").to_string(),
            timeout_ms: f.u64("timeout_ms").unwrap_or(5000),
        })),
        "smtp" | "mail_send" => Ok(Capability::Smtp(proto::SmtpCap {
            host: f.s("host").unwrap_or("127.0.0.1").to_string(),
            port: f.any("port").and_then(|v| v.as_u64()).unwrap_or(25) as u16,
            from: f.s("from").unwrap_or("").to_string(),
            timeout_ms: f.u64("timeout_ms").unwrap_or(8000),
            username: f.s("username").unwrap_or("").to_string(),
            password: f.s("password").unwrap_or("").to_string(),
        })),
        "archive" => Ok(Capability::Archive(proto::ArchiveCap {
            path: f.s("path").unwrap_or("").to_string(),
            op: f.s("op").unwrap_or("list").to_string(),
            dest: f.s("dest").unwrap_or("").to_string(),
            timeout_ms: f.u64("timeout_ms").unwrap_or(30_000),
        })),
        "s3" | "object_store" => Ok(Capability::S3(service::S3Cap {
            endpoint: f.s("endpoint").unwrap_or("").to_string(),
            bucket: f.s("bucket").unwrap_or("").to_string(),
            prefix: f.s("prefix").unwrap_or("").to_string(),
            timeout_ms: f.u64("timeout_ms").unwrap_or(10_000),
            headers: f.obj("headers").cloned().unwrap_or_default(),
        })),
        "prometheus" | "prom" => Ok(Capability::Prometheus(service::PrometheusCap {
            url: f.s("url").unwrap_or("").to_string(),
            timeout_ms: f.u64("timeout_ms").unwrap_or(8000),
        })),
        "kafka" => Ok(Capability::Kafka(service::KafkaCap {
            url: f.s("url").unwrap_or("").to_string(),
            topic: f.s("topic").unwrap_or("").to_string(),
            timeout_ms: f.u64("timeout_ms").unwrap_or(10_000),
        })),
        "pdf" => Ok(Capability::Pdf(service::PdfCap {
            path: f.s("path").unwrap_or("").to_string(),
            op: f.s("op").unwrap_or("text").to_string(),
            timeout_ms: f.u64("timeout_ms").unwrap_or(30_000),
        })),
        "sql" | "database" => Ok(Capability::Sql(service::SqlCap {
            driver: f.s("driver").unwrap_or("sqlite3").to_string(),
            dsn: f.s("dsn").unwrap_or("").to_string(),
            timeout_ms: f.u64("timeout_ms").unwrap_or(30_000),
        })),
        // ── singleton Chrome / CDP ──
        "browser" | "chrome_cdp" => Ok(Capability::Browser(browser::BrowserCap {
            endpoint: f.s("endpoint").unwrap_or("").to_string(),
            chrome_binary: f.s("chrome_binary").unwrap_or("").to_string(),
            profile_dir: f.s("profile_dir").unwrap_or("").to_string(),
            extension_path: f.s("extension_path").unwrap_or("").to_string(),
            launch: f.b("launch").unwrap_or(false),
            attach: f.b("attach").unwrap_or(false),
            startup_timeout_ms: f.u64("startup_timeout_ms").unwrap_or(15_000),
            timeout_ms: f.u64("timeout_ms").unwrap_or(20_000),
            max_text: f.u64("max_text").unwrap_or(6_000) as usize,
            max_owned_pages: f
                .u64("max_owned_pages")
                .unwrap_or(32)
                .clamp(1, 500) as usize,
            owned_idle_ms: f
                .u64("owned_idle_ms")
                .unwrap_or(5 * 60_000)
                .clamp(1_000, 24 * 60 * 60_000),
            human: f.b("human").unwrap_or(true),
        })),
        "web_search" | "search" => Ok(Capability::WebSearch(web::WebSearchCap {
            endpoint: f.s("endpoint").unwrap_or("").to_string(),
            method: f.s("method").unwrap_or("GET").to_uppercase(),
            headers: f.obj("headers").cloned().unwrap_or_default(),
            results_path: f.s("results_path").unwrap_or("").to_string(),
            query_param: f.s("query_param").unwrap_or("").to_string(),
            title_field: f.s("title_field").unwrap_or("").to_string(),
            url_field: f.s("url_field").unwrap_or("").to_string(),
            snippet_field: f.s("snippet_field").unwrap_or("").to_string(),
            timeout_ms: f.u64("timeout_ms").unwrap_or(15_000),
            max_results: f.u64("max_results").unwrap_or(10) as usize,
            max_bytes: f.u64("max_bytes").unwrap_or(256 << 10) as usize,
        })),
        "goal_runner" | "agent_goal" => Ok(Capability::GoalRunner(goal::GoalRunnerCap {
            runner: f.s("runner").unwrap_or("cxgo").to_string(),
            args: f
                .arr("args")
                .map(|a| a.iter().filter_map(|x| x.as_str().map(str::to_string)).collect())
                .unwrap_or_default(),
            timeout_ms: f.u64("timeout_ms").unwrap_or(1_800_000),
            max_output: f.u64("max_output").unwrap_or(256 << 10) as usize,
            reports_dir: f.s("reports_dir").unwrap_or("").to_string(),
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
                let present = f.has("plugin");
                let named = f.s("plugin").unwrap_or("").to_string();
                let alias = f.s("name").unwrap_or("").to_string();
                if present { named } else { alias }
            },
            dir: f.s("dir").unwrap_or("").to_string(),
            entry: f.s("entry").unwrap_or("").to_string(),
            op: f.s("op").unwrap_or("").to_string(),
            browser: f
                .s("browser")
                .unwrap_or("")
                .to_string(),
            max_operations: f
                .u64("max_operations")
                .unwrap_or(0),
            timeout_ms: f.u64("timeout_ms").unwrap_or(0),
        })),
        "needle" => Ok(Capability::Needle(super::needle::NeedleCap {
            op: f.s("op").unwrap_or("extract").to_string(),
            cact: f.s("cact").map(|s| s.to_string()),
        })),
        "computer_use" | "computer-use" => Ok(Capability::ComputerUse(
            computer_use::ComputerUseCap {
                op: f.s("op")
                    .unwrap_or("fingerprint")
                    .to_string(),
                allowed_surface: f
                    .s("allowed_surface")
                    .unwrap_or("")
                    .to_string(),
                allowed_fields: f
                    .arr("allowed_fields")
                    .map(|a| {
                        a.iter().filter_map(|x| x.as_str().map(str::to_string)).collect()
                    })
                    .unwrap_or_default(),
                expected: f.any("expected").cloned().unwrap_or(Value::Null),
                min_confidence: f
                    .f64("min_confidence")
                    .unwrap_or(0.8),
            },
        )),
        "web_fetch" | "fetch_url" => Ok(Capability::WebFetch(web::WebFetchCap {
            format: f.s("format").unwrap_or("text").to_string(),
            headers: f.obj("headers").cloned().unwrap_or_default(),
            timeout_ms: f.u64("timeout_ms").unwrap_or(20_000),
            max_bytes: f.u64("max_bytes").unwrap_or(256 << 10) as usize,
            endpoint: f.s("endpoint").unwrap_or("").to_string(),
            method: f.s("method").unwrap_or("POST").to_uppercase(),
            body_field: f.s("body_field").unwrap_or("url").to_string(),
            body_urls_array: f.b("body_urls_array").unwrap_or(false),
            body: f.any("body").cloned().unwrap_or(Value::Null),
            result_path: f.s("result_path").unwrap_or("").to_string(),
            text_field: f.s("text_field").unwrap_or("").to_string(),
            title_field: f.s("title_field").unwrap_or("").to_string(),
        })),
        other => bail!(
            "capability {name:?}: unknown kind {other:?} (see docs/benchmarks/laya_workflow_capabilities_*.md)"
        ),
    }
}
