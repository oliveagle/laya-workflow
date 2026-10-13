//! DSL smoke: run every spec under `dsl/` through the real engine, checking
//! `validate` on all and running labelled sample states against `EXPECT`.
//! Rust replacement for `laya-workflow dsl smoke` — same logic, no Python.

use anyhow::{bail, Context, Result};
use serde_json::{json, Value};
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

/// Make the scratch env vars every spec may reference; create the dirs.
/// Directory vars get a scratch dir; plain secret-like vars get a dummy value.
/// Only set when absent, so a real environment (CI, live) is never clobbered.
pub(crate) fn ensure_env_dirs() -> Result<BTreeMap<String, String>> {
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
        let b: Box<dyn crate::workflow::Decide> =
            Box::new(crate::backend::HeuristicBackend);
        (b, "offline heuristic".to_string())
    };

    let mut total: u32 = 0;
    let mut fails: u32 = 0;
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
        for (label, st) in state_map {
            let out = match wf.run(backend.as_ref(), st) {
                Ok(o) => o,
                Err(e) => {
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
                    match want {
                        Value::String(want_str) => {
                            let result = res.get("result").cloned().unwrap_or(Value::Null);
                            let got = result
                                .get("label")
                                .and_then(Value::as_str)
                                .or_else(|| result.get("gate_action").and_then(Value::as_str))
                                .unwrap_or("");
                            if got != want_str {
                                fails += 1;
                                eprintln!(
                                    "       [FAIL] {label}: expected label {want_str:?}, got {got:?}"
                                );
                            }
                        }
                        Value::Object(asserts) => {
                            for (path, want_v) in asserts {
                                let got = resolve_path(&res, path).unwrap_or(&Value::Null);
                                let ok = match want_v {
                                    Value::String(want_str) => got.as_str() == Some(want_str),
                                    Value::Number(want_num) => match (got.as_f64(), want_num.as_f64()) {
                                        (Some(g), Some(w)) => {
                                            (g - w).abs() <= 1e-9 * w.abs().max(1.0)
                                        }
                                        _ => false,
                                    },
                                    _ => false,
                                };
                                if !ok {
                                    fails += 1;
                                    eprintln!(
                                        "       [FAIL] {label}: expected {path}={want_v}, got {got}"
                                    );
                                }
                            }
                        }
                        other => {
                            fails += 1;
                            eprintln!(
                                "       [FAIL] {label}: unsupported expect value {other:?} (use a string label or an object of dotted result paths)"
                            );
                        }
                    }
                }
            }
        }
    }
    let ok = total - fails;
    println!("\n{ok}/{total} specs OK");
    if fails > 0 {
        // callers (CI) want a non-zero exit
        bail!("{fails} spec run(s) failed");
    }
    Ok(fails)
}
