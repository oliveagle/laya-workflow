//! DSL smoke: run every spec under `dsl/` through the real engine, checking
//! `validate` on all and running labelled sample states against `EXPECT`.
//! Rust replacement for `laya-workflow dsl smoke` — same logic, no Python.

use anyhow::{bail, Context, Result};
use serde_json::Value;
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

pub struct DslSmokeOptions {
    /// Pin the dsl root (defaults to `<manifest>/dsl`).
    pub dsl_dir: Option<String>,
    /// Live laya-tch base-url; omit for the offline heuristic backend.
    pub base_url: Option<String>,
    /// Only run specs whose stem contains this substring.
    pub filter: Option<String>,
    /// Quiet: only print failures and the final summary line.
    pub quiet: bool,
}

fn load_table(name: &str) -> Result<BTreeMap<String, Value>> {
    let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("bench").join(name);
    if !path.exists() {
        bail!("missing data table {}: {}", name, path.display());
    }
    let raw = std::fs::read_to_string(&path)?;
    let parsed: Value = serde_json::from_str(&raw)
        .with_context(|| format!("parse {}", path.display()))?;
    match parsed {
        Value::Object(map) => Ok(map
            .into_iter()
            .map(|(k, v)| (k, v))
            .collect()),
        _ => bail!("{} must be a JSON object", path.display()),
    }
}

fn discover_specs(root: &Path) -> Result<Vec<PathBuf>> {
    let mut out = Vec::new();
    let mut stack = vec![root.to_path_buf()];
    while let Some(dir) = stack.pop() {
        for entry in std::fs::read_dir(&dir).with_context(|| dir.display().to_string())? {
            let entry = entry?;
            let p = entry.path();
            if p.is_dir() {
                stack.push(p);
            } else if p.extension().and_then(|e| e.to_str()) == Some("json") {
                // `<name>.config.json` sidecars (feature policy/chrome overrides)
                // are config, not workflow specs - never validate or smoke them.
                if p.file_name()
                    .map(|n| n.to_string_lossy().ends_with(".config.json"))
                    .unwrap_or(false)
                {
                    continue;
                }
                out.push(p);
            }
        }
    }
    out.sort();
    Ok(out)
}

/// Strip the `backend:` banner and parse the JSON body that follows.
fn summarize(payload: &str) -> Option<Value> {
    let body = payload.splitn(2, "backend:").nth(1)?;
    let start = body.find('{')?;
    serde_json::from_str(&body[start..]).ok()
}


/// Resolve a dotted path ("result.route", "trace.final_action",
/// "trace.steps.0.node") against a value. A purely-numeric segment indexes an
/// array (serde_json `Value::get` dispatches on the key type).
fn resolve_path<'v>(root: &'v Value, path: &str) -> Option<&'v Value> {
    let mut cur = root;
    for part in path.split('.') {
        cur = match part.parse::<usize>() {
            Ok(i) => cur.get(i)?,
            Err(_) => cur.get(part)?,
        };
    }
    Some(cur)
}

/// Assert a completed run against an EXPECT value. A string value pins the
/// `result.label` (or `gate_action`); an object value pins decision outputs by
/// dotted path (e.g. `{"result.route": "billing"}`), where a number is compared
/// with 1e-9 relative tolerance. Returns a list of failure messages; empty
/// means the run satisfies the expectation. Shared by `dsl smoke` and
/// `bdd score --run`'s decision e2e, so the authoring gate enforces the same
/// decision-output semantics as the behavioural smoke.
pub fn check_expect(out: &crate::workflow::WorkflowOutcome, want: &Value) -> Vec<String> {
    let res = out.to_json();
    let mut problems = Vec::new();
    match want {
        Value::String(want_str) => {
            let result = res.get("result").cloned().unwrap_or(Value::Null);
            let got = result
                .get("label")
                .and_then(Value::as_str)
                .or_else(|| result.get("gate_action").and_then(Value::as_str))
                .unwrap_or("");
            if got != want_str {
                problems.push(format!("expected label {want_str:?}, got {got:?}"));
            }
        }
        Value::Object(asserts) => {
            for (path, want_v) in asserts {
                let got = resolve_path(&res, path).unwrap_or(&Value::Null);
                let ok = match want_v {
                    Value::String(want_str) => got.as_str() == Some(want_str),
                    Value::Number(want_num) => match (got.as_f64(), want_num.as_f64()) {
                        (Some(g), Some(w)) => (g - w).abs() <= 1e-9 * w.abs().max(1.0),
                        _ => false,
                    },
                    _ => false,
                };
                if !ok {
                    problems.push(format!("expected {path}={want_v}, got {got}"));
                }
            }
        }
        other => problems.push(format!(
            "unsupported expect value {other:?} (use a string label or an object of dotted result paths)"
        )),
    }
    problems
}

/// Make the scratch env vars every spec may reference; create the dirs.
/// Directory vars get a scratch dir; plain secret-like vars get a dummy value.
/// Only set when absent, so a real environment (CI, live) is never clobbered.
pub fn ensure_env_dirs() -> Result<BTreeMap<String, String>> {
    let base = std::env::temp_dir().join("laya_dsl_smoke_rust");
    let mut map = BTreeMap::new();
    for var in ["LAYA_STORE_DIR", "LAYA_WORK_DIR", "LAYA_GOAL_DIR"] {
        if std::env::var(var).is_err() {
            let dir = base.join(var.to_lowercase());
            std::fs::create_dir_all(&dir)?;
            map.insert(var.to_string(), dir.to_string_lossy().into_owned());
        }
    }
    // Plain scratch values for the secret/webhook/search env refs the specs
    // use; the local mocks accept any value, so these are just placeholders.
    for (var, val) in [
        ("LAYA_SMTP_USER", "smoke@example.com"),
        ("LAYA_SMTP_PASS", "smoke-pass"),
        ("LAYA_WEBHOOK_SECRET", "smoke-webhook-secret"),
        ("LAYA_WEB_SEARCH_URL", "http://127.0.0.1:8792/search"),
        ("NAME", "laya-smoke"),
        ("NEEDLE", "laya-smoke-needle"),
    ] {
        if std::env::var(var).is_err() {
            map.insert(var.to_string(), val.to_string());
        }
    }
    Ok(map)
}

// ─── offline detection ────────────────────────────────────────────────────────
//
// A spec whose capabilities talk to an external listener (web_search, web_fetch,
// agent, rpc, http, webhook, ...) can only run its behavioural smoke states when
// the listener is up. The local mocks make that possible offline; in CI the
// integration job starts them and the plain `check` job does not. Rather than
// report a regression when the environment simply has no listener, we probe the
// spec's resolvable endpoints and skip the states (validate-only) when they are
// all unreachable — the same "skip, don't hang or fail" policy the rest of the
// harness already applies to network-dependent sections.

/// Expand `${env.X}` / `${secret.X}` for a probe URL. Unlike the engine's
/// strict resolver, a missing name leaves the reference literal (so the target
/// simply does not resolve); probing must never error.
fn expand_env_only_loose(s: &str) -> String {
    if !s.contains("${") {
        return s.to_string();
    }
    let mut out = String::new();
    let mut rest = s;
    while let Some(i) = rest.find("${") {
        out.push_str(&rest[..i]);
        let after = &rest[i + 2..];
        match after.find('}') {
            Some(j) => {
                let path = &after[..j];
                match path.strip_prefix("env.").or_else(|| path.strip_prefix("secret.")) {
                    Some(name) => {
                        if let Ok(v) = std::env::var(name) {
                            out.push_str(&v);
                        } else if let Ok(v) = crate::capability::secret::get(name) {
                            out.push_str(&v);
                        }
                        // unresolved → the URL stays partially literal, and
                        // parse_http_host_port below declines it.
                    }
                    None => out.push_str(&format!("${{{path}}}")),
                }
                rest = &after[j + 1..];
            }
            None => break,
        }
    }
    out.push_str(rest);
    out
}

/// Parse an `http(s)://host[:port]` URL into (host, port).
fn parse_http_host_port(url: &str) -> Option<(String, u16)> {
    let is_https = url.starts_with("https://");
    let rest = url
        .strip_prefix("https://")
        .or_else(|| url.strip_prefix("http://"))?;
    let authority = rest.split('/').next()?;
    let (host, port) = match authority.rsplit_once(':') {
        Some((h, p)) if !p.is_empty() && p.chars().all(|c| c.is_ascii_digit()) => {
            (h, p.parse::<u16>().ok()?)
        }
        _ => (authority, if is_https { 443 } else { 80 }),
    };
    if host.is_empty() {
        return None;
    }
    Some((host.to_string(), port))
}

/// Collect the unique (host, port) network targets a spec's capabilities
/// declare via their top-level `endpoint` / `url` fields. Web_search uses
/// `endpoint`; agent/rpc/graphql/http/webhook/... use `url`.
pub(crate) fn spec_network_targets(spec: &Value) -> Vec<(String, u16)> {
    let mut out: Vec<(String, u16)> = Vec::new();
    let Some(caps) = spec.get("capabilities").and_then(|v| v.as_object()) else {
        return out;
    };
    for cdef in caps.values() {
        let Some(obj) = cdef.as_object() else { continue };
        // A `plugin` capability declares no endpoint/url; its network targets
        // live inside the plugin script (P33). Static scan: http_get calls +
        // URL string literals, comments ignored.
        if obj.get("kind").and_then(Value::as_str) == Some("plugin") {
            if let Some(pid) = obj.get("plugin").and_then(Value::as_str) {
                for t in crate::capability::plugin::plugin_script_network_targets(pid) {
                    if !out.contains(&t) {
                        out.push(t);
                    }
                }
            }
            continue;
        }
        for field in ["endpoint", "url"] {
            let Some(raw) = obj.get(field).and_then(|v| v.as_str()) else {
                continue;
            };
            let url = expand_env_only_loose(raw);
            if let Some(t) = parse_http_host_port(&url) {
                if !out.contains(&t) {
                    out.push(t);
                }
            }
        }
    }
    out
}

fn tcp_reachable(host: &str, port: u16) -> bool {
    use std::net::ToSocketAddrs;
    let Ok(mut addrs) = (host, port).to_socket_addrs() else {
        return false;
    };
    let Some(addr) = addrs.next() else {
        return false;
    };
    std::net::TcpStream::connect_timeout(&addr, std::time::Duration::from_millis(500)).is_ok()
}

/// True when a spec is network-bound (it declares resolvable endpoint/url
/// targets) and none of those targets currently accepts a TCP connection —
/// i.e. the environment is offline with respect to this spec. Callers should
/// skip its behavioural states (validate-only) rather than report a regression.
/// A spec with no resolvable target (e.g. a web_fetch-only spec whose URL only
/// appears in state) returns false: we cannot prove it offline, so it keeps the
/// current run-and-report behaviour.
pub fn network_bound_offline(spec: &Value) -> bool {
    let targets = spec_network_targets(spec);
    if targets.is_empty() {
        return false;
    }
    !targets.iter().any(|(h, p)| tcp_reachable(h, *p))
}

/// True when a spec is network-bound and at least one declared target is an
/// *external* host (not loopback / RFC1918 private / link-local). The offline
/// smoke must never perform real internet I/O, so a spec that depends on the
/// external network has its states skipped (validate-only) there; the
/// integration / live job exercises them for real. Local mocks (127.0.0.1,
/// 10.x, 192.168.x, ...) stay "online" and keep running against the harness.
pub fn network_bound_external(spec: &Value) -> bool {
    let targets = spec_network_targets(spec);
    !targets.is_empty() && targets.iter().any(|(h, _)| !is_private_host(h))
}

/// Loopback, RFC1918 private, link-local and CGNAT address families — hosts
/// that do not leave the machine / local network and are safe for the offline
/// smoke to touch.
fn is_private_host(host: &str) -> bool {
    if host.eq_ignore_ascii_case("localhost") || host == "::1" || host == "0.0.0.0" {
        return true;
    }
    let parts: Vec<&str> = host.split('.').collect();
    if parts.len() != 4 {
        return false;
    }
    let o0: Option<u16> = parts[0].parse().ok();
    let o1: Option<u16> = parts[1].parse().ok();
    match (o0, o1) {
        (Some(10), _) => true,                                  // 10/8
        (Some(127), _) => true,                                 // loopback
        (Some(169), Some(254)) => true,                         // link-local
        (Some(172), Some(o1)) if (16..=31).contains(&o1) => true, // 172.16/12
        (Some(192), Some(168)) => true,                         // 192.168/16
        (Some(100), Some(o1)) if (64..=127).contains(&o1) => true, // CGNAT 100.64/10
        _ => false,
    }
}

pub fn run(opts: &DslSmokeOptions) -> Result<u32> {
    let root: PathBuf = match &opts.dsl_dir {
        Some(d) => PathBuf::from(d),
        None => PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("dsl"),
    };
    let specs = discover_specs(&root)?;
    if specs.is_empty() {
        bail!("no specs found under {}", root.display());
    }
    let states = load_table("dsl_smoke_states.json")?;
    let expect = load_table("dsl_smoke_expect.json")?;
    let env = ensure_env_dirs()?;
    for (k, v) in &env {
        std::env::set_var(k, v);
    }
    // The secrets store snapshots the environment once at CLI startup, so a
    // fresh snapshot (with the dirs we just created) must replace it before
    // specs that reference ${env.LAYA_STORE_DIR} & friends can validate.
    crate::capability::secret::init(None)?;

    let mode = match &opts.base_url {
        Some(u) => format!("live @ {u}"),
        None => "offline heuristic".to_string(),
    };
    if !opts.quiet {
        println!("engine backend: {mode}");
        println!("specs: {}", specs.len());
        println!();
    }

    let (backend, _label) = if let Some(u) = &opts.base_url {
        let b: Box<dyn crate::workflow::Decide> =
            Box::new(crate::backend::LayaBackend::new(u));
        (b, format!("laya-tch @ {u}"))
    } else {
        // Offline smoke: bound plugin `http_get`/external I/O to a short
        // timeout so a network call that slips past static detection fails
        // fast (and is then skipped as external/offline) instead of hanging
        // for the capability's full timeout. Local mocks answer in ms, so
        // they are unaffected.
        std::env::set_var("LAYA_SMOKE_NET_TIMEOUT_MS", "1500");
        let b: Box<dyn crate::workflow::Decide> =
            Box::new(crate::backend::HeuristicBackend);
        (b, "offline heuristic".to_string())
    };

    let mut total: u32 = 0;
    let mut fails: u32 = 0;
    let mut skipped: u32 = 0;
    for spec in &specs {
        let stem = spec.file_stem().and_then(|s| s.to_str()).unwrap_or("").to_string();
        if let Some(f) = &opts.filter {
            if !stem.contains(f) {
                continue;
            }
        }
        total += 1;

        // validate (cheap check: load_file parses + checks version)
        let wf = match crate::spec::load_file(&spec.to_string_lossy()) {
            Ok(w) => w,
            Err(e) => {
                fails += 1;
                eprintln!("[FAIL] {}: validate failed\n  {e}", spec.display());
                continue;
            }
        };
        let rel = spec.strip_prefix(&root).unwrap_or(spec).display().to_string();
        if !opts.quiet {
            println!("[spec] {rel}  start={} nodes={}", wf.start, wf.nodes.len());
        }

        let Some(sample_states) = states.get(&stem) else {
            if !opts.quiet {
                println!("       (no sample states registered; validate only)");
            }
            continue;
        };
        let Some(state_map) = sample_states.as_object() else {
            continue;
        };
        // The raw spec drives offline detection: when a network-bound spec's
        // declared endpoints are all unreachable the environment is offline, and
        // a state that *errors* because it needs one of those listeners is
        // skipped (validate-only) instead of reported as a regression. States
        // that run fine still run and assert, preserving offline coverage for
        // specs whose sample states never touch the network.
        let raw_spec: Value = std::fs::read_to_string(&spec)
            .ok()
            .and_then(|t| serde_json::from_str(&t).ok())
            .unwrap_or(Value::Null);
        for (label, st) in state_map {
            // P33: the offline smoke must not perform real internet I/O. A
            // spec that is network-bound to an external (non-local) host has
            // its states skipped here (validate-only) — e.g. the
            // `hf_trending` plugin reaching huggingface.co was costing ~16 s
            // per run before the static plugin-script scan classified it.
            // The integration / live job runs these states for real.
            if opts.base_url.is_none() && network_bound_external(&raw_spec) {
                if !opts.quiet {
                    eprintln!(
                        "       {label}: external network target(s) — skipped, validate only (runs in the integration job)"
                    );
                }
                skipped += 1;
                continue;
            }
            let out = match wf.run(backend.as_ref(), st) {
                Ok(o) => o,
                Err(e) => {
                    if network_bound_offline(&raw_spec)
                        || (opts.base_url.is_none() && network_bound_external(&raw_spec))
                    {
                        if !opts.quiet {
                            eprintln!(
                                "       {label}: offline (network endpoint(s) unreachable) — skipped, validate only"
                            );
                        }
                        skipped += 1;
                        continue;
                    }
                    fails += 1;
                    eprintln!("       [FAIL] {label}: run error: {e}");
                    continue;
                }
            };
            let res = out.to_json();
            let steps = res
                .pointer("/trace/steps")
                .and_then(Value::as_array)
                .cloned()
                .unwrap_or_default();
            let chain = steps
                .iter()
                .filter_map(|s| {
                    let node = s.get("node")?.as_str()?;
                    let action = s.get("action")?.as_str()?;
                    Some(format!("{node}:{action}"))
                })
                .collect::<Vec<_>>()
                .join(" -> ");
            let final_action = res
                .pointer("/trace/final_action")
                .and_then(Value::as_str)
                .unwrap_or("?");
            if !opts.quiet {
                println!("       {label:10} final={final_action}  {chain}");
            }
            // compare against EXPECT: a string value pins the label (or
            // gate_action); an object value pins decision outputs by dotted
            // path (e.g. {"result.route": "billing"}).
            if let Some(expect_map) = expect.get(&stem).and_then(Value::as_object) {
                if let Some(want) = expect_map.get(label) {
                    for msg in check_expect(&out, want) {
                        fails += 1;
                        eprintln!("       [FAIL] {label}: {msg}");
                    }
                    // P31 structural half of the decision-trace gate: a state
                    // that walks more than one node must pin its route
                    // (trace.steps.N.node/.action + trace.step_count) in the
                    // expect table. A final label alone would let an authored
                    // feature route through wrong intermediate nodes and still
                    // pass — this turns "runs to the right answer" into "takes
                    // the right path". Runs in check.sh via dsl smoke, and
                    // applies to bdd score --run's decision e2e (shared table).
                    if steps.len() > 1 {
                        let trace_pinned = want
                            .as_object()
                            .map(|m| {
                                m.contains_key("trace.steps.0.node")
                                    && m.contains_key("trace.step_count")
                            })
                            .unwrap_or(false);
                        if !trace_pinned {
                            fails += 1;
                            eprintln!(
                                "       [FAIL] {label}: walks {} nodes but expect pins no trace path (add trace.steps.N.node/.action + trace.step_count)",
                                steps.len()
                            );
                        }
                    }
                }
            }
        }
    }
    let ok = total - fails;
    println!("\n{ok}/{total} specs OK");
    if skipped > 0 {
        println!("({skipped} state(s) skipped offline because a network endpoint was unreachable; they run in the integration job / with the local mocks up)");
    }
    if fails > 0 {
        // callers (CI) want a non-zero exit
        bail!("{fails} spec run(s) failed");
    }
    Ok(fails)
}
