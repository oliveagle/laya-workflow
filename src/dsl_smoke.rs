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

pub fn discover_specs(root: &Path) -> Result<Vec<PathBuf>> {
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
/// P38 structural half of the decision-trace gate: a decision state that walks
/// more than one node must pin its **entire** execution path in the expect
/// table — `node`, the `action` verb and the routing `answer` at *every* step,
/// plus `trace.step_count` — so the gate proves "took the right path, via the
/// right action, on the right answer", not merely "produced the right final
/// label". A first-hop-only (or node-only) pin would let an authored feature
/// route through different intermediate nodes/answers/verbs and still pass.
/// Returns the missing keys (empty when the path is fully pinned).
pub fn missing_trace_pins(want: &Value, step_count: usize) -> Vec<String> {
    let mut missing: Vec<String> = Vec::new();
    let m = want.as_object();
    let has = |k: &str| m.map(|m| m.contains_key(k)).unwrap_or(false);
    if !has("trace.step_count") {
        missing.push("trace.step_count".into());
    }
    for i in 0..step_count {
        for field in ["node", "action", "answer"] {
            let k = format!("trace.steps.{i}.{field}");
            if !has(&k) {
                missing.push(k);
            }
        }
    }
    missing
}

/// P126: the *coverage* half of the decision-trace gate. `missing_trace_pins`
/// above proves a multi-node state pins its whole path — but only for states
/// whose expect entry **exists**. A single-node state with no expect entry was
/// silently passed over: the `if let Some(want)` block that runs `check_expect`
/// simply did not execute, so the state ran, and nothing about it was asserted.
/// Measured before this: `routing` (4 states), `traversal` (3) and one
/// `support_ticket_router` label ran 8 states against nothing while the gate
/// printed `163/163 specs OK` — a gate that names a state but asserts nothing
/// about it is a comment, not a check.
///
/// A state that ran is *covered* when its expect entry is a non-empty label
/// string or a non-empty object of dotted paths. Multi-node states are left to
/// `missing_trace_pins`, which already fails them harder (every path key is
/// required, `Null` included), so one hole is never named twice.
/// Returns the one-line failure message (None when the state is covered).
pub fn uncovered_state(stem: &str, label: &str, want: Option<&Value>, steps: usize) -> Option<String> {
    if steps > 1 {
        return None;
    }
    let covered = match want {
        Some(Value::String(s)) => !s.trim().is_empty(),
        Some(Value::Object(o)) => !o.is_empty(),
        _ => false,
    };
    if covered {
        return None;
    }
    Some(format!(
        "{stem}:{label}: the state ran {steps} node(s) and pins nothing — the labelled state is asserted against nothing; give it an expect entry (a label string, or an object of dotted `result.*` paths)"
    ))
}

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
                    // Booleans are a first-class decision output (a projected
                    // `truncated` / `blocked` / `seen` field). Before this the
                    // `_ => false` arm made every boolean expect *always* fail,
                    // so an author could not pin one — the gate silently had no
                    // vocabulary for a whole JSON type.
                    Value::Bool(want_b) => got.as_bool() == Some(*want_b),
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
    // `NEEDLE3_CACT` is the on-device Needle 3 activation blob that every
    // `kind: needle` capability interpolates (`cact`). Without it the offline
    // heuristic backend refuses the capability as an unresolved reference
    // (`${unresolved:env.NEEDLE3_CACT}`) before it can run, so the whole
    // needle-using spec was unreachable offline; a placeholder lets the
    // offline backend drive those graphs (needle_demo / needle_ticket /
    // support_ticket_router_hybrid's hybrid fallback).
    for (var, val) in [
        ("LAYA_SMTP_USER", "smoke@example.com"),
        ("LAYA_SMTP_PASS", "smoke-pass"),
        ("LAYA_WEBHOOK_SECRET", "smoke-webhook-secret"),
        ("LAYA_WEB_SEARCH_URL", "http://127.0.0.1:8792/search"),
        ("NAME", "laya-smoke"),
        ("NEEDLE", "laya-smoke-needle"),
        ("NEEDLE3_CACT", "laya-smoke-cact"),
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

/// The OS names a capability's `platforms` list may name, beyond the two
/// shorthands `unix` / `windows`. `current_platform_tags()` returns the exact
/// set this build satisfies.
fn current_platform_tags() -> Vec<&'static str> {
    if cfg!(windows) {
        vec!["windows"]
    } else if cfg!(target_os = "macos") {
        vec!["unix", "macos"]
    } else if cfg!(target_os = "linux") {
        vec!["unix", "linux"]
    } else {
        vec!["unix"]
    }
}

/// True when a spec *calls* a capability gated to a platform we are not on.
///
/// Some capabilities are inherently OS-specific: an `exec` argv of
/// `["/bin/sh","-c","df -Pk … | awk …"]` (or a `shell` command invoking `sh`)
/// is POSIX shell and cannot execute on Windows, where the same intent would
/// need a different command entirely. A spec that declares
/// `"platforms": ["unix"]` on such a capability is therefore skipped
/// validate-only on Windows instead of reported as a regression — the same
/// stance as the network-bound skip, and for the same reason: the run cannot
/// make its claim on this host, but it can still prove the spec validates.
pub fn platform_bound_here(spec: &Value) -> bool {
    let Some(caps) = spec.get("capabilities").and_then(Value::as_object) else {
        return false;
    };
    // Only capabilities a node actually calls matter; a declared-but-unused
    // unix-only capability must not knock an otherwise-portable spec out.
    let mut called: Vec<&str> = Vec::new();
    if let Some(nodes) = spec.get("nodes").and_then(Value::as_array) {
        for n in nodes {
            if let Some(cap) = n
                .get("action")
                .and_then(|a| a.get("capability"))
                .and_then(Value::as_str)
            {
                called.push(cap);
            }
        }
    }
    let here = current_platform_tags();
    for (name, cdef) in caps {
        if !called.contains(&name.as_str()) {
            continue;
        }
        let Some(list) = cdef.get("platforms").and_then(Value::as_array) else {
            continue;
        };
        let names: Vec<&str> = list.iter().filter_map(Value::as_str).collect();
        if names.is_empty() {
            continue;
        }
        if !names.iter().any(|n| here.contains(n)) {
            return true;
        }
    }
    false
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

/// P43 coverage ledger. `dsl smoke` behaviour-verifies only the specs that
/// carry sample states; a spec with no states used to be silently "validate
/// only" — a workflow nothing would ever flag as unverified. This makes the
/// accounting total: every discovered spec must be either offline-registered
/// (its stem is a key in `bench/dsl_smoke_states.json`) or declared live-only
/// in `bench/dsl_coverage_ledger.json` with the gate that covers it
/// (`covered_by`) and why the offline smoke cannot. A stale declaration — its
/// spec became offline-registered, or no longer exists — is an error too, so
/// the ledger cannot rot into a silent hole.
///
/// Returns `(offline_verified, live_declared, problems)`.
/// The `bench/dsl_coverage_ledger.json` `live_only` map: rel-spec (no `.json`)
/// → `{covered_by, reason}`. This is the authority for "this spec is *not*
/// behaviour-verified offline": `dsl smoke` counts its size as the live-declared
/// set, and `bdd_list` / `bdd_decide` report it per spec so all three agree.
pub fn coverage_ledger_live_only() -> Result<BTreeMap<String, Value>> {
    let ledger = load_table("dsl_coverage_ledger.json")?;
    let mut out: BTreeMap<String, Value> = BTreeMap::new();
    if let Some(o) = ledger.get("live_only").and_then(Value::as_object) {
        for (k, v) in o {
            out.insert(k.clone(), v.clone());
        }
    }
    Ok(out)
}

/// A repo-relative path with `/` separators on every OS.
///
/// `Path::display()` yields `\` on Windows, but every consumer of these strings
/// — the `bench/dsl_coverage_ledger.json` keys, the `[spec]` lines the doc-check
/// quotes, and the isolation-audit messages — is authored with `/`. Leaking the
/// host separator made the coverage-ledger lookup miss every entry on Windows
/// (`browser\github` vs `browser/github`), so `dsl smoke` reported each
/// live-declared spec as unaccounted. Normalise at the source.
pub fn slashed(p: &Path, root: &Path) -> String {
    p.strip_prefix(root)
        .unwrap_or(p)
        .to_string_lossy()
        .replace('\\', "/")
}

pub fn coverage_ledger_audit(
    specs: &[PathBuf],
    root: &Path,
    states: &BTreeMap<String, Value>,
) -> Result<(usize, usize, Vec<String>)> {
    let ledger = load_table("dsl_coverage_ledger.json")?;
    let live_only = ledger
        .get("live_only")
        .and_then(Value::as_object)
        .ok_or_else(|| anyhow::anyhow!("bench/dsl_coverage_ledger.json needs a `live_only` object"))?;
    let rel_of = |p: &Path| -> String {
        let r = slashed(p, root);
        r.strip_suffix(".json").unwrap_or(&r).to_string()
    };
    let mut problems = Vec::new();
    let mut offline = 0usize;
    let mut live = 0usize;
    // rel -> stem, so a stale declaration can tell "now offline-registered"
    // apart from "spec deleted".
    let stems: BTreeMap<String, String> = specs
        .iter()
        .map(|s| {
            (
                rel_of(s),
                s.file_stem().and_then(|x| x.to_str()).unwrap_or("").to_string(),
            )
        })
        .collect();
    for spec in specs {
        let rel = rel_of(spec);
        let stem = spec.file_stem().and_then(|s| s.to_str()).unwrap_or("");
        if states.contains_key(stem) {
            offline += 1;
            continue;
        }
        match live_only.get(&rel) {
            Some(entry) => {
                live += 1;
                if entry
                    .get("covered_by")
                    .and_then(Value::as_str)
                    .unwrap_or("")
                    .trim()
                    .is_empty()
                {
                    problems.push(format!("{rel}: ledger entry names no `covered_by` gate"));
                }
                if entry
                    .get("reason")
                    .and_then(Value::as_str)
                    .unwrap_or("")
                    .trim()
                    .is_empty()
                {
                    problems.push(format!("{rel}: ledger entry has no `reason`"));
                }
            }
            None => problems.push(format!(
                "{rel}: no offline smoke states in bench/dsl_smoke_states.json and no entry in bench/dsl_coverage_ledger.json — register a sample state, or declare the gate that verifies it and why offline smoke cannot"
            )),
        }
    }
    for rel in live_only.keys() {
        match stems.get(rel) {
            None => problems.push(format!(
                "{rel}: stale ledger entry — no such spec under dsl/; remove it"
            )),
            Some(stem) if states.contains_key(stem) => problems.push(format!(
                "{rel}: stale ledger entry — it is now offline-registered in bench/dsl_smoke_states.json; remove it"
            )),
            _ => {}
        }
    }
    Ok((offline, live, problems))
}

/// Recursively collect every file under `dir` into `out` (order irrelevant).
fn collect_files(dir: &Path, out: &mut Vec<PathBuf>) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let p = entry.path();
        if p.is_dir() {
            collect_files(&p, out);
        } else {
            out.push(p);
        }
    }
}

/// Same as [`collect_files`] but skips the usual heavy/uninteresting roots so
/// the whole-repo scan for stray `.feature` files stays cheap.
fn collect_files_shallow_skip(dir: &Path, out: &mut Vec<PathBuf>) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let name = entry.file_name().to_string_lossy().into_owned();
        let p = entry.path();
        if p.is_dir() {
            if matches!(name.as_str(), ".git" | "target" | "node_modules" | ".venv") {
                continue;
            }
            collect_files_shallow_skip(&p, out);
        } else {
            out.push(p);
        }
    }
}

/// P44 standing-constraint gate: BDD and JSON are two fully separate trees.
///
/// The user's standing requirement (2026-10-13) is that the `.feature` sources
/// (BDD = single source of truth) and the workflow `.json` specs live in **two
/// different folders**, that **all** BDD is in one of them and **all** specs in
/// the other, and that both are **hierarchical** — nothing loose at a tree root,
/// everything filed under a category folder. This is that constraint as a
/// machine check, so it can never rot back into a manual "we eyeballed it"
/// verification:
///
///   * no `.json` anywhere under `bdd/` — a spec belongs under `dsl/`;
///   * no `.feature` anywhere under `dsl/` — a source belongs under `bdd/`;
///   * no `.feature` anywhere else in the repo (outside the one `bdd/` tree);
///   * no `.feature` anywhere under the `dsl/` tree, and the mirror of that —
///     no workflow `.json` spec anywhere outside the one `dsl/` tree (a spec is
///     recognised by its transpiled shape, see [`is_workflow_spec`]);
///   * no source loose at a tree root — `bdd/*.feature` and `dsl/*.json` are
///     both rejected (file it under `bdd/features/`, `bdd/workflows/<category>/`
///     or `dsl/<category>/`).
///
/// Support files that are neither source nor spec (browser fixtures, `README.md`,
/// `.html`, a plugin `.rhai`) are not the cross-type leak this guards, so they
/// are left alone. Returns the problems found (empty = the invariant holds).
/// True when `p` is a transpiled workflow `.json` spec — the mirror of a
/// `.feature` source: either the BDD provenance key `_bdd`, or the engine's
/// mandatory `nodes` + `start` pair, is present. A cheap content sniff so the
/// whole-repo json scan only ever flags real specs, never a plugin manifest,
/// npm `package.json` or an eval-result file.
fn is_workflow_spec(p: &Path) -> bool {
    let Ok(text) = std::fs::read_to_string(p) else {
        return false;
    };
    let Ok(v) = serde_json::from_str::<serde_json::Value>(&text) else {
        return false;
    };
    let Some(obj) = v.as_object() else {
        return false;
    };
    obj.contains_key("_bdd") || (obj.contains_key("nodes") && obj.contains_key("start"))
}

pub fn isolation_audit(repo_root: &Path) -> Vec<String> {
    let bdd = repo_root.join("bdd");
    let dsl = repo_root.join("dsl");
    let mut problems = Vec::new();

    let rel = |p: &Path| slashed(p, repo_root);
    let ext = |p: &Path| p.extension().and_then(|e| e.to_str()).unwrap_or("").to_string();

    // (1) the BDD tree carries sources, never json; its sources are filed under
    //     a category folder, never loose at the bdd/ root.
    if bdd.is_dir() {
        let mut files = Vec::new();
        collect_files(&bdd, &mut files);
        for f in &files {
            match ext(f).as_str() {
                "json" => problems.push(format!(
                    "{}: json in the BDD tree — a workflow spec belongs under dsl/ (bdd/ holds .feature sources only)",
                    rel(f)
                )),
                "feature" if f.parent() == Some(bdd.as_path()) => problems.push(format!(
                    "{}: loose .feature at the bdd/ root — file it under bdd/features/ or bdd/workflows/<category>/",
                    rel(f)
                )),
                _ => {}
            }
        }
    }

    // (2) the json tree carries specs, never a .feature; no spec loose at root.
    if dsl.is_dir() {
        let mut files = Vec::new();
        collect_files(&dsl, &mut files);
        for f in &files {
            match ext(f).as_str() {
                "feature" => problems.push(format!(
                    "{}: .feature in the json tree — a BDD source belongs under bdd/ (dsl/ holds .json specs only)",
                    rel(f)
                )),
                "json" if f.parent() == Some(dsl.as_path()) => problems.push(format!(
                    "{}: loose .json at the dsl/ root — file it under dsl/<category>/",
                    rel(f)
                )),
                _ => {}
            }
        }
    }

    // (3) every .feature in the repo lives in the single bdd/ tree.
    let mut all = Vec::new();
    collect_files_shallow_skip(repo_root, &mut all);
    for f in &all {
        if ext(f) == "feature" && !f.starts_with(&bdd) {
            problems.push(format!(
                "{}: .feature outside bdd/ — all BDD sources live in the single bdd/ tree",
                rel(f)
            ));
        }
    }

    // (4) the mirror of (3) for the json side: every workflow spec lives in the
    //     single dsl/ tree. Non-spec json (plugin manifests, eval data) is not
    //     the cross-type leak this guards, so `is_workflow_spec` filters it out.
    for f in &all {
        if ext(f) == "json" && !f.starts_with(&dsl) && is_workflow_spec(f) {
            problems.push(format!(
                "{}: workflow spec json outside dsl/ — all workflow specs live in the single dsl/ tree",
                rel(f)
            ));
        }
    }

    problems.sort();
    problems.dedup();
    problems
}

/// Split a per-spec failure tally into `(specs_ok, specs_failed, failing_checks)`.
///
/// The `N/M specs OK` line counts spec *documents* that passed, so it must come
/// from the specs, never from `total - failing_checks`: a spec has many states and
/// checks, so subtracting per-check failures from a per-spec total underflows the
/// u32 the moment any state fails (a live run once printed `4294967265/1` instead
/// of how many specs passed). Pure, so the regression can pin it without a corpus.
pub fn tally(per_spec_fail_counts: &[u32]) -> (u32, u32, u32) {
    let specs_ok = per_spec_fail_counts.iter().filter(|c| **c == 0).count() as u32;
    let specs_failed = per_spec_fail_counts.len() as u32 - specs_ok;
    let failing_checks: u32 = per_spec_fail_counts.iter().sum();
    (specs_ok, specs_failed, failing_checks)
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
    let mut per_spec_fail_counts: Vec<u32> = Vec::new();
    let mut skipped: u32 = 0;
    for spec in &specs {
        let stem = spec.file_stem().and_then(|s| s.to_str()).unwrap_or("").to_string();
        if let Some(f) = &opts.filter {
            if !stem.contains(f) {
                continue;
            }
        }
        total += 1;
        let spec_fails_before = fails;

        // validate (cheap check: load_file parses + checks version)
        let wf = match crate::spec::load_file(&spec.to_string_lossy()) {
            Ok(w) => w,
            Err(e) => {
                fails += 1;
                eprintln!("[FAIL] {}: validate failed\n  {e}", spec.display());
                per_spec_fail_counts.push(1);
                continue;
            }
        };
        let rel = slashed(spec, &root);
        if !opts.quiet {
            println!("[spec] {rel}  start={} nodes={}", wf.start, wf.nodes.len());
        }

        let Some(sample_states) = states.get(&stem) else {
            if !opts.quiet {
                println!("       (no sample states registered; validate only)");
            }
            // validate-only is a pass for this spec: there was nothing to run.
            per_spec_fail_counts.push(0);
            continue;
        };
        let Some(state_map) = sample_states.as_object() else {
            per_spec_fail_counts.push(0);
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
            // A capability gated to another platform (e.g. a POSIX
            // `exec`/`shell` command on Windows) cannot run here; skip
            // validate-only rather than report a regression the host cannot
            // actually demonstrate.
            if platform_bound_here(&raw_spec) {
                if !opts.quiet {
                    eprintln!(
                        "       {label}: platform-bound capability (not available on {}) — skipped, validate only",
                        current_platform_tags().join("/")
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
                        || platform_bound_here(&raw_spec)
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
            let want = expect
                .get(&stem)
                .and_then(Value::as_object)
                .and_then(|m| m.get(label))
                .cloned();
            if let Some(want) = &want {
                for msg in check_expect(&out, want) {
                    fails += 1;
                    eprintln!("       [FAIL] {label}: {msg}");
                }
            }
            // P31/P38 structural half of the decision-trace gate: a state that
            // walks more than one node must pin its entire route — node +
            // action + answer at every step, plus trace.step_count (see
            // `missing_trace_pins`). A final label alone, or a first-hop-only
            // pin, would let an authored feature route through wrong
            // intermediate nodes / actions / answers and still pass — this
            // turns "runs to the right answer" into "takes the right path on
            // the right evidence". Runs in check.sh via dsl smoke, and applies
            // to bdd score --run's decision e2e (shared table). A multi-node
            // state with **no** expect entry at all is a hard failure here too
            // — `missing_trace_pins(Null, n)` returns every key — so the smoke
            // gate and the P38 regression test can never silently disagree
            // (a state whose expect entry is missing used to be skipped).
            if steps.len() > 1 {
                let want_v = want.clone().unwrap_or(Value::Null);
                let missing = missing_trace_pins(&want_v, steps.len());
                if !missing.is_empty() {
                    fails += 1;
                    eprintln!(
                        "       [FAIL] {label}: walks {} nodes but expect pins an incomplete trace path — missing {} (pin node/action/answer for every step + trace.step_count)",
                        steps.len(),
                        missing.join(", ")
                    );
                }
            }
            // P126: the coverage half — did this state assert anything at all?
            if let Some(msg) = uncovered_state(&stem, label, want.as_ref(), steps.len()) {
                fails += 1;
                eprintln!("       [FAIL] {msg}");
            }
        }
        // Count a spec as OK only when none of its states/checks failed. The old
        // `total - fails` subtracted *per-state* failures from a *per-spec* total,
        // so any failing state underflowed the u32 and a live run printed a
        // garbage count like `4294967265/1` instead of how many specs passed.
        per_spec_fail_counts.push(fails - spec_fails_before);
    }
    let (ok, failed_specs, _failing_checks) = tally(&per_spec_fail_counts);
    println!("\n{ok}/{total} specs OK");
    if skipped > 0 {
        println!("({skipped} state(s) skipped offline because a network endpoint was unreachable; they run in the integration job / with the local mocks up)");
    }
    // P43: the coverage ledger — every discovered spec accounted for, no
    // stale declaration. Printed so `check.sh` / CI show the split.
    let (ledger_offline, ledger_live, ledger_problems) =
        coverage_ledger_audit(&specs, &root, &states)?;
    println!(
        "coverage ledger: {ledger_offline} offline-verified + {ledger_live} live-declared = {} specs",
        ledger_offline + ledger_live
    );
    for p in &ledger_problems {
        eprintln!("[FAIL] coverage ledger: {p}");
    }
    // P44: the bdd/json separation invariant (see `isolation_audit`). Only
    // meaningful against the repo's own two-tree layout, so a `--dsl-dir`
    // override — which points the smoke at some other tree — skips it.
    let iso_problems = if opts.dsl_dir.is_none() {
        isolation_audit(Path::new(env!("CARGO_MANIFEST_DIR")))
    } else {
        Vec::new()
    };
    if iso_problems.is_empty() && opts.dsl_dir.is_none() {
        println!("isolation: bdd/ holds no json, dsl/ holds no feature; both trees are hierarchical");
    }
    for p in &iso_problems {
        eprintln!("[FAIL] isolation: {p}");
    }
    if fails > 0 || !ledger_problems.is_empty() || !iso_problems.is_empty() {
        // callers (CI) want a non-zero exit
        bail!(
            "{} spec(s) failed ({fails} failing check(s)); {} coverage-ledger problem(s); {} isolation problem(s)",
            failed_specs,
            ledger_problems.len(),
            iso_problems.len()
        );
    }
    Ok(fails)
}
