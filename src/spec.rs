//! Generic workflow spec (JSON) ↔ `ResilientWorkflow`.
//!
//! A workflow can be authored as data — no Rust changes, no recompilation:
//!
//! ```json
//! {
//!   "name": "ticket_router",
//!   "start": "classify",
//!   "max_iterations": 20,
//!   "convergence_window": 5,
//!   "convergence_eps": 0.0001,
//!   "nodes": [
//!     {
//!       "name": "classify",
//!       "primary_q": "category",
//!       "questions": {
//!         "category": {"type": "choice", "instructions": "Which team?",
//!                      "criteria": {"billing": "money", "technical": "bugs"}}
//!       },
//!       "edge": {"condition": {"billing": "STOP", "technical": "escalate"}, "default": "STOP"},
//!       "min_confidence": 0.4,
//!       "max_retries": 2,
//!       "action": {"kind": "merge_open_probs"}
//!     }
//!   ]
//! }
//! ```
//!
//! `action.kind` selects a **built-in, data-parameterised** action:
//!
//! | kind | behaviour |
//! |------|-----------|
//! | `none` | no app payload |
//! | `copy_keys` | copy `action.keys` from the verdict answer into the payload |
//! | `merge_open_probs` | emit all answer keys + confidence + the option probability spread |
//! | `threshold` | emit a decision label from `action.rules` (ordered `{when, label}`) |
//! | `gate` | 3-way ALLOW/CONFIRM/BLOCK from a probability + thresholds |
//!
//! Custom Rust actions stay available via `WorkflowNode::with_action`; a spec may
//! mix built-in and hand-written nodes.

use anyhow::{anyhow, bail, Result};
use serde_json::{json, Map, Value};

use crate::capability::Registry;
use crate::workflow::{Edge, ResilientWorkflow, Verdict, WorkflowNode};

/// Current DSL schema version. Specs may declare `"dsl_version": N`.
///
/// Compatibility rules:
///   * absent          → accepted as `1` (legacy specs keep working)
///   * `< 1` or `> DSL_VERSION` → rejected with a clear message
///   * equal           → normal parse
///   * `< DSL_VERSION` (but ≥ 1) → parsed, and `validate` reports it as *upgradable*
pub const DSL_VERSION: u64 = 2;

/// Result of a version check.
#[derive(Debug, PartialEq)]
pub enum VersionCheck {
    /// Spec declared this version (1 when absent).
    Ok(u64),
    /// Older but still supported; `to` is the current version.
    Upgradable { from: u64, to: u64 },
}

/// Read and check `"dsl_version"` without parsing the rest of the spec.
pub fn check_version(spec: &Value) -> Result<VersionCheck> {
    let declared = spec.get("dsl_version").and_then(|v| v.as_u64()).unwrap_or(1);
    if declared == 0 {
        bail!("spec 'dsl_version' must be >= 1 (got 0)");
    }
    if declared > DSL_VERSION {
        bail!(
            "spec requires dsl_version {declared} but this engine supports up to {DSL_VERSION} \
             (upgrade the engine, or lower the spec's dsl_version)"
        );
    }
    if declared < DSL_VERSION {
        return Ok(VersionCheck::Upgradable { from: declared, to: DSL_VERSION });
    }
    Ok(VersionCheck::Ok(declared))
}

/// Parse a workflow spec (JSON value) into a runnable `ResilientWorkflow`.
pub fn from_spec(spec: &Value) -> Result<ResilientWorkflow> {
    from_spec_in(spec, None)
}

/// Like `from_spec` but resolves nested references relative to `base_dir`.
pub fn from_spec_in(spec: &Value, base_dir: Option<&std::path::Path>) -> Result<ResilientWorkflow> {
    from_spec_with_dir(spec, None, base_dir)
}

/// Resolve a nested `workflow` reference to a spec.
///
/// Accepted forms:
///   * `"workflow": "support_ticket_router"`        → `<dsl_dir>/support_ticket_router.json`
///   * `"workflow": "./sub/triage.json"`            → relative to the `dsl_dir`
///   * `"workflow": {"inline": { …spec… }}`         → an inline sub-spec
///   * a registry entry registered via `register`
pub type SpecRegistry = std::collections::HashMap<String, Value>;

/// Process-wide registry of named specs (populated by `register_load` / CLI).
pub fn registry() -> &'static std::sync::Mutex<SpecRegistry> {
    static R: std::sync::OnceLock<std::sync::Mutex<SpecRegistry>> = std::sync::OnceLock::new();
    R.get_or_init(|| std::sync::Mutex::new(SpecRegistry::new()))
}

/// Register a named spec (available to every `"workflow": "<name>"` reference).
pub fn register(name: &str, spec: Value) {
    registry().lock().unwrap().insert(name.to_string(), spec);
}

/// Directory used to resolve bare `"workflow": "name"` references.
pub fn dsl_dir() -> &'static std::sync::Mutex<String> {
    static D: std::sync::OnceLock<std::sync::Mutex<String>> = std::sync::OnceLock::new();
    D.get_or_init(|| std::sync::Mutex::new("dsl".to_string()))
}

pub fn set_dsl_dir(dir: &str) {
    *dsl_dir().lock().unwrap() = dir.to_string();
}

/// Like `resolve_ref_in` but also returns the directory the sub-spec came from,
/// so *its* relative refs resolve against the correct folder.
/// Resolve a `workflow` reference value into `(spec, version, dir)`.
///
/// A reference may pin a version with `name@2` or `./x.json@2`. When no version
/// is pinned, the **highest version found** wins, so multiple versions can
/// coexist in the tree (`refund.v1.json` / `refund.v2.json`, or
/// `refund/1.json` / `refund/2.json`).
fn resolve_ref_dir(
    r: &Value,
    from_dir: Option<&std::path::Path>,
) -> Result<(Value, Option<std::path::PathBuf>)> {
    match r {
        Value::Object(o) if o.contains_key("inline") => Ok((o["inline"].clone(), from_dir.map(|d| d.to_path_buf()))),
        Value::String(raw) => {
            let (name, pinned) = split_version(raw);
            if pinned.is_none() {
                if let Some(s) = registry().lock().unwrap().get(&name) {
                    return Ok((s.clone(), None));
                }
            }
            let root = dsl_dir().lock().unwrap().clone();
            let mut cands: Vec<std::path::PathBuf> = Vec::new();
            if let Some(dir) = from_dir {
                cands.push(dir.join(&name));
                cands.push(dir.join(format!("{name}.json")));
            }
            let base = std::path::Path::new(&root);
            cands.push(base.join(&name));
            cands.push(base.join(format!("{name}.json")));
            for c in cands {
                if c.is_file() {
                    return Ok((read_spec_file(&c)?, c.parent().map(|d| d.to_path_buf())));
                }
            }
            // tree search, honouring the version pin (or picking the highest)
            if let Some(found) = find_in_tree_versioned(base, &name, pinned)? {
                return Ok((read_spec_file(&found)?, found.parent().map(|d| d.to_path_buf())));
            }
            let hint = match pinned {
                Some(v) => format!(" (version {v} pinned)"),
                None => String::new(),
            };
            bail!("workflow reference {raw:?} not found{hint} (searched referring dir, {root}/ and its subtree)")
        }
        other => bail!("workflow reference must be a string or {{\"inline\": …}}, got {other}"),
    }
}

/// Split `"name@2"` / `"./x.json@2"` into (`name`, Some(2)).
fn split_version(raw: &str) -> (String, Option<u64>) {
    match raw.rsplit_once('@') {
        Some((n, v)) if !n.is_empty() => match v.parse::<u64>() {
            Ok(nv) => (n.to_string(), Some(nv)),
            Err(_) => (raw.to_string(), None),
        },
        _ => (raw.to_string(), None),
    }
}

/// Find `name` in the tree, optionally pinned to `want` version; otherwise the
/// highest declared `dsl_version` among the candidates wins.
fn find_in_tree_versioned(
    root: &std::path::Path,
    name: &str,
    want: Option<u64>,
) -> Result<Option<std::path::PathBuf>> {
    if !root.is_dir() {
        return Ok(None);
    }
    let target = format!("{name}.json");
    // also accept `name.v2.json` and `name/2.json` layouts
    let alt_prefix = format!("{name}.v");
    let mut dirs: Vec<std::path::PathBuf> = vec![root.to_path_buf()];
    let mut hits: Vec<(u64, std::path::PathBuf)> = Vec::new();
    while let Some(d) = dirs.pop() {
        let mut entries: Vec<_> = std::fs::read_dir(&d)?.filter_map(|e| e.ok()).collect();
        entries.sort_by_key(|e| e.file_name());
        for e in entries {
            let p = e.path();
            let fname = e.file_name().to_string_lossy().to_string();
            if p.is_dir() {
                // `name/<version>.json` convention
                if fname == name {
                    let mut vs: Vec<_> = std::fs::read_dir(&p)?
                        .filter_map(|x| x.ok())
                        .filter(|x| x.path().extension().and_then(|e| e.to_str()) == Some("json"))
                        .collect();
                    vs.sort_by_key(|x| x.file_name());
                    for v in vs {
                        let vn = v
                            .path()
                            .file_stem()
                            .and_then(|s| s.to_str())
                            .and_then(|s| s.parse::<u64>().ok());
                        if let Some(vn) = vn {
                            hits.push((vn, v.path()));
                        }
                    }
                    if p.join("workflow.json").is_file() {
                        hits.push((declared_version(&p.join("workflow.json")).unwrap_or(1), p.join("workflow.json")));
                    }
                }
                dirs.push(p);
            } else if fname == target {
                hits.push((declared_version(&p).unwrap_or(1), p));
            } else if let Some(rest) = fname.strip_prefix(&alt_prefix) {
                // `name.v2.json`
                if rest.ends_with(".json") {
                    if let Ok(vn) = rest.trim_end_matches(".json").parse::<u64>() {
                        hits.push((vn, p));
                    }
                }
            }
        }
    }
    if let Some(w) = want {
        return Ok(hits.into_iter().filter(|(v, _)| *v == w).map(|(_, p)| p).min());
    }
    hits.sort_by_key(|(v, p)| (*v, p.clone()));
    Ok(hits.pop().map(|(_, p)| p)) // highest version
}

/// Public peek at a spec file's declared `dsl_version` (1 when absent).
pub fn read_version(p: &std::path::Path) -> Option<u64> {
    declared_version(p)
}

/// Cheap peek at a spec file's declared `dsl_version` (1 when absent).
fn declared_version(p: &std::path::Path) -> Option<u64> {
    let raw = std::fs::read_to_string(p).ok()?;
    let v: Value = serde_json::from_str(&raw).ok()?;
    Some(v.get("dsl_version").and_then(|x| x.as_u64()).unwrap_or(1))
}
fn read_spec_file(p: &std::path::Path) -> Result<Value> {
    let raw = std::fs::read_to_string(p)?;
    Ok(serde_json::from_str(&raw)?)
}

/// Depth-first search for `<name>.json` (or `<name>/`) under `root`.
///
/// Parse a spec with an optional parent context (for nested resolution depth).
pub fn from_spec_with(spec: &Value, parent: Option<&Value>) -> Result<ResilientWorkflow> {
    from_spec_with_dir(spec, parent, None)
}

/// Core parser: `base_dir` is the directory used to resolve relative refs.
pub fn from_spec_with_dir(
    spec: &Value,
    _parent: Option<&Value>,
    base_dir: Option<&std::path::Path>,
) -> Result<ResilientWorkflow> {
    // Version gate: rejects forward-incompatible specs, accepts legacy ones.
    check_version(spec)?;
    // Capabilities + safety policy declared by this spec (may be empty).
    let registry = Registry::from_spec(spec)?;
    let start = spec
        .get("start")
        .and_then(|v| v.as_str())
        .ok_or_else(|| anyhow!("spec missing 'start'"))?
        .to_string();
    let node_list = spec
        .get("nodes")
        .and_then(|v| v.as_array())
        .ok_or_else(|| anyhow!("spec missing 'nodes' array"))?;
    if node_list.is_empty() {
        bail!("spec has no nodes");
    }

    let mut nodes: Vec<WorkflowNode> = Vec::with_capacity(node_list.len());
    for n in node_list {
        // A node may be a `{"workflow": …}` *reference node*: it inlines the
        // referenced workflow's nodes into this graph (namespaced) so nesting
        // needs no special runner support.
        if let Some(wref) = n.get("workflow") {
            let (sub_spec, sub_dir) = resolve_ref_dir(wref, base_dir)?;
            let sub_name = n
                .get("name")
                .and_then(|v| v.as_str())
                .or_else(|| sub_spec.get("name").and_then(|v| v.as_str()))
                .unwrap_or("sub");
            let prefix = format!("{sub_name}::");
            let sub = from_spec_with_dir(&sub_spec, Some(spec), sub_dir.as_deref())?;
            // `sub.start` is the sub's own start (unprefixed); the expanded nodes
            // above get `prefix`, so inbound edges must point at the prefixed name.
            let sub_start = format!("{prefix}{}", sub.start);
            for existing in nodes.iter_mut() {
                for dest in existing.edge.condition.values_mut() {
                    if dest == sub_name {
                        *dest = sub_start.clone();
                    }
                }
                if existing.edge.default.as_deref() == Some(&sub_name) {
                    existing.edge.default = Some(sub_start.clone());
                }
            }
            for node in sub.nodes.values() {
                let mut nn = node.clone();
                nn.name = format!("{prefix}{}", node.name);
                for dest in nn.edge.condition.values_mut() {
                    if dest != "STOP" && sub.nodes.contains_key(dest) {
                        *dest = format!("{prefix}{dest}");
                    }
                }
                if let Some(d) = nn.edge.default.clone() {
                    if d != "STOP" && sub.nodes.contains_key(&d) {
                        nn.edge.default = Some(format!("{prefix}{d}"));
                    }
                }
                nodes.push(nn);
            }
            continue;
        }
        nodes.push(node_from_spec_with(n, Some(registry.clone()))?);
    }

    // If `start` names a node that was expanded from a reference, redirect it to
    // that sub-workflow's own `start` (also namespaced).
    let mut start = start;
    let names: std::collections::HashSet<String> = nodes.iter().map(|n| n.name.clone()).collect();
    if !names.contains(&start) {
        if let Some(rest) = node_list.iter().find(|n| {
            n.get("name").and_then(|v| v.as_str()) == Some(start.as_str())
                || n.get("workflow").and_then(|v| v.as_str()) == Some(start.as_str())
        }) {
            if let Some(wref) = rest.get("workflow") {
                let (sub_spec, _) = resolve_ref_dir(wref, base_dir)?;
                let sub_name = rest
                    .get("name")
                    .and_then(|v| v.as_str())
                    .or_else(|| sub_spec.get("name").and_then(|v| v.as_str()))
                    .unwrap_or("sub");
                let sub_start = sub_spec.get("start").and_then(|v| v.as_str()).unwrap_or("start");
                start = format!("{sub_name}::{sub_start}");
            }
        }
    }

    let mut wf = ResilientWorkflow::new(nodes, &start);
    if let Some(v) = spec.get("max_iterations").and_then(|v| v.as_u64()) {
        wf = wf.with_max_iterations(v as usize);
    }
    let mut window = 5usize;
    let mut eps = 1e-4;
    if let Some(v) = spec.get("convergence_window").and_then(|v| v.as_u64()) {
        window = v as usize;
    }
    if let Some(v) = spec.get("convergence_eps").and_then(|v| v.as_f64()) {
        eps = v;
    }
    Ok(wf.with_convergence(window, eps))
}

/// Parse one node entry from a spec.
pub fn node_from_spec(n: &Value) -> Result<WorkflowNode> {
    node_from_spec_with(n, None)
}

/// Node parser with an optional capability registry (enables `{"kind":"call"}`).
pub fn node_from_spec_with(n: &Value, registry: Option<Registry>) -> Result<WorkflowNode> {
    let name = n
        .get("name")
        .and_then(|v| v.as_str())
        .ok_or_else(|| anyhow!("node missing 'name'"))?;
    let questions = n.get("questions").cloned().unwrap_or_else(|| json!({}));

    let edge_v = n.get("edge").cloned().unwrap_or_else(|| json!({}));
    let mut condition: Vec<(String, String)> = Vec::new();
    if let Some(Value::Object(cond)) = edge_v.get("condition") {
        for (k, v) in cond {
            let dest = v
                .as_str()
                .ok_or_else(|| anyhow!("node {name}: edge condition {k:?} must be a string"))?;
            condition.push((k.clone(), dest.to_string()));
        }
    }
    let default = edge_v.get("default").and_then(|v| v.as_str());
    let min_confidence = n
        .get("min_confidence")
        .or_else(|| edge_v.get("min_confidence"))
        .and_then(|v| v.as_f64())
        .unwrap_or(0.0);
    let refs: Vec<(&str, &str)> = condition.iter().map(|(a, b)| (a.as_str(), b.as_str())).collect();
    let edge = Edge::new(&refs, default, min_confidence);

    let mut node = WorkflowNode::new(name, questions.clone(), edge);
    if let Some(pq) = n.get("primary_q").and_then(|v| v.as_str()) {
        node = node.with_primary(pq);
    }
    if let Some(r) = n.get("max_retries").and_then(|v| v.as_u64()) {
        node = node.with_max_retries(r as usize);
    }
    if let Some(state_v) = n.get("state") {
        let state = state_v.clone();
        node = node.with_state_fn(move |s: &Value| project_state(s, &state));
    }
    if let Some(action) = n.get("action") {
        let action = action.clone();
        let primary = n
            .get("primary_q")
            .and_then(|v| v.as_str())
            .map(str::to_string);
        let reg = registry.clone();
        node = node.with_action(move |state: &Value, v: &Verdict| {
            match (&action, &reg) {
                // capability-backed action: external effect, needs the registry
                (a, Some(r)) if a.get("kind").and_then(|k| k.as_str()) == Some("call") => {
                    run_call_action(a, state, v, r)
                }
                _ => run_action(&action, state, v, primary.as_deref()),
            }
        });
    }
    Ok(node)
}

/// `state` projection: `{"keep": [...], "rename": {out: in}}` — a data-level
/// `state_fn` so a spec can shape what the backend sees.
fn project_state(state: &Value, spec: &Value) -> Value {
    if spec.is_null() {
        return state.clone();
    }
    let mut out = Map::new();
    if let Some(keep) = spec.get("keep").and_then(|v| v.as_array()) {
        for k in keep.iter().filter_map(|v| v.as_str()) {
            if let Some(v) = state.get(k) {
                out.insert(k.to_string(), v.clone());
            }
        }
    }
    if let Some(renames) = spec.get("rename").and_then(|v| v.as_object()) {
        for (to, from) in renames {
            if let Some(src) = from.as_str() {
                if let Some(v) = state.get(src) {
                    out.insert(to.clone(), v.clone());
                }
            }
        }
    }
    if out.is_empty() {
        state.clone()
    } else {
        Value::Object(out)
    }
}

/// Capability-backed action: `{"kind":"call","capability":"x","with":{…}}`.
///
/// The result is written back into the node payload under `result` (and its
/// `body`/`status`/`exit_code` keys are hoisted for convenience), so downstream
/// nodes and the workflow state can use it.
pub fn run_call_action(
    action: &Value,
    state: &Value,
    v: &Verdict,
    registry: &Registry,
) -> Result<Value> {
    let name = action
        .get("capability")
        .and_then(|c| c.as_str())
        .ok_or_else(|| anyhow!("action kind=call needs 'capability'"))?;
    // `with` may be a literal object or a template resolved against
    // {state, with:{question answers}}.
    let answers = verdict_answers(v);

    // Optional `chain`: run prerequisite capabilities first; their results are
    // exposed to later steps and to the main call as `with.<step_name>`.
    let mut with = action.get("with").cloned().unwrap_or_else(|| json!({}));
    let mut chain_results = Map::new();
    if let Some(steps) = action.get("chain").and_then(|c| c.as_array()) {
        for (i, step) in steps.iter().enumerate() {
            let step_cap = step
                .get("capability")
                .and_then(|c| c.as_str())
                .ok_or_else(|| anyhow!("chain step {i} needs 'capability'"))?;
            let step_name = step
                .get("as")
                .and_then(|a| a.as_str())
                .unwrap_or(step_cap)
                .to_string();
            // a step may carry its own `with`, else inherit the action's
            let mut step_with = step.get("with").cloned().unwrap_or_else(|| with.clone());
            if let Some(obj) = step_with.as_object_mut() {
                for (k, val) in &chain_results {
                    obj.insert(k.clone(), val.clone());
                }
            }
            let step_with = crate::capability::expand(&step_with, state, &answers);
            let res = registry.call(step_cap, &step_with, state)?;
            chain_results.insert(step_name.clone(), res);
        }
    }

    // Merge chain results into `with` BEFORE expanding, so a template like
    // "${with.seed.epoch}" resolves against a context that already contains the
    // chain output.
    if !chain_results.is_empty() {
        if let Some(obj) = with.as_object_mut() {
            for (k, val) in &chain_results {
                obj.insert(k.clone(), val.clone());
            }
        }
    }
    // Expand against {state, with: merged-with + question answers}, so templates
    // can read `${with.<chain_step>}` as well as `${with.<question>}`.
    let mut ctx = with.as_object().cloned().unwrap_or_default();
    if let Some(ans) = answers.as_object() {
        for (k, v) in ans {
            ctx.entry(k.clone()).or_insert_with(|| v.clone());
        }
    }
    let with = crate::capability::expand(&with, state, &Value::Object(ctx));
    // Fail loudly when a referenced secret is missing, instead of sending an
    // empty credential. Any `${secret.X}` that resolved to null is an error.
    if let Some(missing) = missing_secret_in(&with, &action) {
        anyhow::bail!(
            "capability {name:?}: secret {missing:?} is not available \
             (set it in the environment or a .env / secrets file; see the secrets docs)"
        );
    }

    let result = registry.call(name, &with, state)?;

    let mut payload = Map::new();
    payload.insert("capability".to_string(), json!(name));
    payload.insert("result".to_string(), result.clone());
    if let Some(o) = result.as_object() {
        for (k, val) in o {
            payload.entry(k.clone()).or_insert_with(|| val.clone());
        }
    }
    // optional: project values out of the capability result into named keys.
    // Pointers resolve against the result, falling back to its `body` (so
    // "/label" finds `{"status":200,"body":{"label":"high"}}.body.label`).
    if let Some(proj) = action.get("project").and_then(|p| p.as_object()) {
        // Root for pointers: the main call's result, augmented with chain
        // outputs under their `as` name, so `/queued/length` works alongside
        // `/hex`. `body` fallback covers http-style envelopes.
        let mut roots = vec![result.clone()];
        if !chain_results.is_empty() {
            let mut merged = result.as_object().cloned().unwrap_or_default();
            for (k, v) in &chain_results {
                merged.insert(k.clone(), v.clone());
            }
            roots.push(Value::Object(merged));
        }
        for (out_key, ptr) in proj {
            if let Some(ptr) = ptr.as_str() {
                let segs: Vec<&str> = ptr.trim_start_matches('/').split('/').filter(|s| !s.is_empty()).collect();
                let mut found: Option<Value> = None;
                for root in &roots {
                    if let Some(v) = json_pointer(root, &segs).or_else(|| {
                        root.get("body").and_then(|b| json_pointer(b, &segs))
                    }) {
                        found = Some(v.clone());
                        break;
                    }
                }
                if let Some(v) = found {
                    payload.insert(out_key.clone(), v.clone());
                }
            }
        }
    }
    Ok(Value::Object(payload))
}

/// Find the first `${secret.X}` in the action's config that is unavailable in
/// the secret store, so the caller can hard-fail instead of sending an empty
/// credential. Values supplied via `with` (e.g. chain output) are not secrets.
fn missing_secret_in(_expanded: &Value, action: &Value) -> Option<String> {
    crate::capability::secret::referenced_names(action)
        .into_iter()
        .find(|name| !crate::capability::secret::has(name))
}

/// Walk a JSON value by path segments; `None` when any segment is missing.
fn json_pointer<'a>(root: &'a Value, segs: &[&str]) -> Option<&'a Value> {
    let mut cur = root;
    for seg in segs {
        cur = match cur {
            Value::Object(o) => o.get(*seg)?,
            Value::Array(a) => a.get(seg.parse::<usize>().ok()?)?,
            _ => return None,
        };
    }
    Some(cur)
}

/// Condense a verdict into `{question: answer}` so capability templates can read
/// `"${with.<question>}"` alongside `"${state.<field>}"`.
fn verdict_answers(v: &Verdict) -> Value {
    let mut m = Map::new();
    for (k, d) in &v.answers {
        m.insert(k.clone(), d.answer.clone());
    }
    Value::Object(m)
}

/// Dispatch a spec action descriptor to the matching built-in action.
pub fn run_action(
    action: &Value,
    _state: &Value,
    v: &Verdict,
    primary: Option<&str>,
) -> Result<Value> {
    let kind = action
        .get("kind")
        .and_then(|k| k.as_str())
        .ok_or_else(|| anyhow!("action missing 'kind'"))?;
    let q = action
        .get("question")
        .and_then(|q| q.as_str())
        .or(primary);
    let q = match q {
        Some(q) => q,
        None => {
            // only actions that read a specific answer need a question
            if kind == "copy_keys" || kind == "none" {
                ""
            } else {
                bail!("action {kind:?} needs 'question' or a primary_q");
            }
        }
    };

    match kind {
        "none" => Ok(json!({})),
        "copy_keys" => {
            let keys: Vec<String> = action
                .get("keys")
                .and_then(|v| v.as_array())
                .map(|a| a.iter().filter_map(|v| v.as_str().map(str::to_string)).collect())
                .unwrap_or_default();
            let mut out = Map::new();
            for k in keys {
                if let Some(vv) = v.answers.get(&k) {
                    out.insert(k.clone(), vv.answer.clone());
                }
            }
            Ok(Value::Object(out))
        }
        "merge_open_probs" => {
            let d = v
                .answers
                .get(q)
                .ok_or_else(|| anyhow!("action merge_open_probs: no answer {q:?}"))?;
            let mut probs = Map::new();
            for (k, pv) in &d.probabilities {
                probs.insert(k.clone(), pv.clone());
            }
            let spread = prob_spread(&d.probabilities);
            Ok(json!({
                "action_question": q,
                "action_answer": d.answer,
                "confidence": d.confidence,
                "probabilities": probs,
                "merged": true,
                "spread": spread,
            }))
        }
        "threshold" => {
            let d = v
                .answers
                .get(q)
                .ok_or_else(|| anyhow!("action threshold: no answer {q:?}"))?;
            let rules = action
                .get("rules")
                .and_then(|r| r.as_array())
                .ok_or_else(|| anyhow!("action threshold: missing 'rules'"))?;
            let label = apply_rules_multi(rules, v, d);
            Ok(json!({
                "action_question": q,
                "action_answer": d.answer,
                "label": label,
                "confidence": d.confidence,
            }))
        }
        "gate" => {
            // Gate semantics (fail-closed): `option` is the *unsafe* answer
            // (default "B"). High p(option) → BLOCK; high p(not option) → ALLOW;
            // the middle band → CONFIRM.
            let d = v
                .answers
                .get(q)
                .ok_or_else(|| anyhow!("action gate: no answer {q:?}"))?;
            let block_at = action.get("block_at").and_then(|x| x.as_f64()).unwrap_or(0.85);
            let allow_at = action.get("allow_at").and_then(|x| x.as_f64()).unwrap_or(0.85);
            let option = action.get("option").and_then(|x| x.as_str()).unwrap_or("B");
            let p_unsafe = d.prob(option);
            let p_safe = 1.0 - p_unsafe;
            let decision = if p_unsafe >= block_at {
                "BLOCK"
            } else if p_safe >= allow_at {
                "ALLOW"
            } else {
                "CONFIRM"
            };
            Ok(json!({
                "action_question": q,
                "gate_action": decision,
                "p": p_unsafe,
                "p_unsafe": p_unsafe,
                "confidence": d.confidence,
                "reason": format!("{q}: p({option})={p_unsafe:.3} block_at={block_at} allow_at={allow_at}"),
            }))
        }
        other => bail!("unknown action kind {other:?}"),
    }
}

/// max−min of an option distribution (a cheap uncertainty read-out).
fn prob_spread(probs: &Map<String, Value>) -> f64 {
    let vals: Vec<f64> = probs.iter().filter_map(|(_, v)| v.as_f64()).collect();
    if vals.is_empty() {
        return 0.0;
    }
    let mx = vals.iter().cloned().fold(f64::NEG_INFINITY, f64::max);
    let mn = vals.iter().cloned().fold(f64::INFINITY, f64::min);
    let r4 = |x: f64| format!("{:.4}", x).parse::<f64>().unwrap_or(x);
    r4(mx - mn)
}

/// Ordered rule list with per-rule `when.question` support.
fn apply_rules_multi(
    rules: &[Value],
    v: &crate::workflow::Verdict,
    default_d: &crate::workflow::Decision,
) -> String {
    for r in rules {
        let label = r.get("label").and_then(|l| l.as_str()).unwrap_or("unknown").to_string();
        match r.get("when") {
            Some(w) if rule_matches_multi(w, v, default_d) => return label,
            Some(_) => continue,
            None => return label,
        }
    }
    "unmatched".to_string()
}

/// Ordered rule list: first rule whose `when` matches wins; `when` supports
/// `{"option": "B", "gte": 0.8}`, `{"answer_in": ["billing"]}`, `{"always": true}`.
#[allow(dead_code)]
fn apply_rules(rules: &[Value], d: &crate::workflow::Decision) -> String {
    for r in rules {
        let label = r.get("label").and_then(|l| l.as_str()).unwrap_or("unknown").to_string();
        let when = match r.get("when") {
            Some(w) => w,
            None => return label,
        };
        if rule_matches(when, d) {
            return label;
        }
    }
    "unmatched".to_string()
}

fn rule_matches(when: &Value, d: &crate::workflow::Decision) -> bool {
    if when.get("always").and_then(|v| v.as_bool()).unwrap_or(false) {
        return true;
    }
    if let Some(ans) = when.get("answer_in").and_then(|v| v.as_array()) {
        let a = d.as_str();
        return ans.iter().filter_map(|x| x.as_str()).any(|x| x == a);
    }
    let mut ok = true;
    if let Some(ans) = when.get("answer").and_then(|v| v.as_str()) {
        ok &= d.as_str() == ans;
    }
    if let Some(opt) = when.get("option").and_then(|v| v.as_str()) {
        let p = d.prob(opt);
        if let Some(g) = when.get("gte").and_then(|v| v.as_f64()) {
            ok &= p >= g;
        }
        if let Some(l) = when.get("lt").and_then(|v| v.as_f64()) {
            ok &= p < l;
        }
    }
    if let Some(g) = when.get("value_gte").and_then(|v| v.as_f64()) {
        ok &= d.as_f64() >= g;
    }
    if let Some(l) = when.get("value_lt").and_then(|v| v.as_f64()) {
        ok &= d.as_f64() < l;
    }
    ok
}

/// Like `rule_matches`, but `when.question` may name a *different* question than
/// the rule list's primary one (so one node can combine several signals).
fn rule_matches_multi(
    when: &Value,
    v: &crate::workflow::Verdict,
    default_d: &crate::workflow::Decision,
) -> bool {
    if when.get("always").and_then(|x| x.as_bool()).unwrap_or(false) {
        return true;
    }
    match when.get("question").and_then(|x| x.as_str()) {
        Some(q) => match v.answers.get(q) {
            Some(d) => {
                // only answer/option/value operators are meaningful here
                let mut probe = when.clone();
                if let Some(o) = probe.as_object_mut() {
                    o.remove("question");
                }
                rule_matches(&probe, d)
            }
            None => false,
        },
        None => rule_matches(when, default_d),
    }
}

/// Serialize a workflow back to a **round-trippable** spec.
///
/// Hand-written `with_action` closures cannot be serialized; such nodes are
/// marked `"action": {"kind": "rust"}` and `from_spec` rejects them, so a spec
/// exchange format only ever carries built-in actions.
pub fn to_spec(wf: &ResilientWorkflow, name: &str) -> Value {
    let mut nodes: Vec<Value> = wf
        .nodes
        .values()
        .map(|n| {
            let cond: Map<String, Value> = n
                .edge
                .condition
                .iter()
                .map(|(k, v)| (k.clone(), json!(v)))
                .collect();
            json!({
                "name": n.name,
                "primary_q": n.primary_q,
                "questions": n.questions,
                "edge": {"condition": cond, "default": n.edge.default},
                "min_confidence": n.edge.min_confidence,
                "max_retries": n.max_retries,
                "action": if n.action_fn.is_some() { json!({"kind": "rust"}) } else { Value::Null },
            })
        })
        .collect();
    nodes.sort_by(|a, b| a["name"].as_str().cmp(&b["name"].as_str()));
    json!({
        "name": name,
        "start": wf.start,
        "max_iterations": wf.max_iterations,
        "convergence_window": wf.convergence_window,
        "convergence_eps": wf.convergence_eps,
        "nodes": nodes,
    })
}

/// Convenience: load a spec file from disk.
///
/// Relative `workflow` references inside the file are resolved against the
/// file's own directory first, so folder layouts work naturally.
pub fn load_file(path: &str) -> Result<ResilientWorkflow> {
    let p = std::path::Path::new(path);
    let raw = std::fs::read_to_string(p)?;
    let spec: Value = serde_json::from_str(&raw)?;
    from_spec_with_dir(&spec, None, p.parent())
}

/// Discover every spec in the DSL tree (`name` → path), sorted by path.
pub fn discover(root: &str) -> Result<Vec<(String, std::path::PathBuf)>> {
    let base = std::path::Path::new(root);
    if !base.is_dir() {
        return Ok(Vec::new());
    }
    let mut out: Vec<(String, std::path::PathBuf)> = Vec::new();
    let mut dirs = vec![base.to_path_buf()];
    while let Some(d) = dirs.pop() {
        let mut entries: Vec<_> = std::fs::read_dir(&d)?.filter_map(|e| e.ok()).collect();
        entries.sort_by_key(|e| e.file_name());
        for e in entries {
            let p = e.path();
            if p.is_dir() {
                dirs.push(p);
            } else if p.extension().and_then(|x| x.to_str()) == Some("json") {
                let stem = p.file_stem().and_then(|x| x.to_str()).unwrap_or("").to_string();
                out.push((stem, p));
            }
        }
    }
    out.sort_by(|a, b| a.1.cmp(&b.1));
    Ok(out)
}
