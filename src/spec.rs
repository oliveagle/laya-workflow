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
    let declared = spec
        .get("dsl_version")
        .and_then(|v| v.as_u64())
        .unwrap_or(1);
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
        return Ok(VersionCheck::Upgradable {
            from: declared,
            to: DSL_VERSION,
        });
    }
    Ok(VersionCheck::Ok(declared))
}

/// P148: a spec object is read by name too. `from_spec_with_dir_cycle` /
/// `Registry::from_spec` / `check_version` read only the keys below (plus the
/// derived `_bdd` metadata the compiler writes and `bdd` tooling reads back),
/// and the `policy` object is read only for its six keys. Anything else —
/// `satrt`, `policiy`, `allow_exex` — is never touched, so the declared value is
/// silently ignored: the workflow starts elsewhere / the policy never applies.
/// Same class as P142's `state` and P145-P147's node / question / action keys.
pub fn validate_spec_keys(spec: &Value) -> Result<()> {
    const KNOWN_SPEC_KEYS: &[&str] = &[
        "name", "dsl_version", "description", "start", "nodes",
        "max_iterations", "convergence_window", "convergence_eps",
        "capabilities", "policy", "_bdd",
    ];
    const KNOWN_POLICY_KEYS: &[&str] = &[
        "allow_exec", "allow_hosts", "allow_paths", "max_timeout_ms",
        "max_output", "retries",
    ];
    const KNOWN_BDD_META_KEYS: &[&str] = &[
        "feature", "feature_path", "mode", "scenario", "tags", "initial_state", "outputs",
    ];
    // A non-object spec is reported by the parser below ("spec missing 'start'"),
    // so this check stays silent on it rather than pre-empting that message.
    let Some(obj) = spec.as_object() else {
        return Ok(());
    };
    for k in obj.keys() {
        if !KNOWN_SPEC_KEYS.contains(&k.as_str()) {
            bail!(
                "spec carries an unrecognised key {k:?}{} — the engine never reads it, so it is silently ignored",
                suggest(k, KNOWN_SPEC_KEYS)
            );
        }
    }
    if let Some(p) = obj.get("policy") {
        let Some(po) = p.as_object() else {
            bail!(
                "spec `policy` must be an object (allow_exec / allow_hosts / allow_paths / max_timeout_ms / max_output / retries), not {} — a non-object policy is silently ignored and every default applies",
                json_kind_name(p)
            );
        };
        for k in po.keys() {
            if !KNOWN_POLICY_KEYS.contains(&k.as_str()) {
                bail!(
                    "spec `policy` carries an unrecognised key {k:?}{} — the engine never reads it, so it is silently ignored",
                    suggest(k, KNOWN_POLICY_KEYS)
                );
            }
        }
        // P168: a policy value of the wrong JSON type is silently replaced by the
        // engine's default — `{"allow_exec":"yes"}` does *not* allow exec (it
        // stays off), `{"allow_hosts":"a.com"}` becomes the empty list (*no*
        // restriction), `{"max_timeout_ms":"60000"}` becomes 60000. Same
        // "a declared value the engine throws away" class as P152/P154/P164; the
        // compiler always emits well-typed values (a `Given the policy` table is
        // checked as it is read), so this only ever fires on a hand-written or
        // sidecar spec — which is exactly where it was silent.
        for (k, v) in po {
            if let Some(msg) = policy_value_error(k, v) {
                bail!("spec {msg}");
            }
        }
    }
    if let Some(b) = obj.get("_bdd").and_then(Value::as_object) {
        for k in b.keys() {
            if !KNOWN_BDD_META_KEYS.contains(&k.as_str()) {
                bail!(
                    "spec `_bdd` carries an unrecognised key {k:?}{} — it is compiler metadata and the tooling reads only the known keys",
                    suggest(k, KNOWN_BDD_META_KEYS)
                );
            }
        }
    }
    Ok(())
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
/// A bare name is looked up across the layered spec roots (see [`spec_roots`]),
/// highest priority first: the explicit pin (`set_dsl_dir` / `--dsl-dir`), the
/// repo `.laya-workflow/dsl` / `dsl` tree, the user root, then `<crate>/dsl` —
/// the first root that has the name wins.
///
/// Accepted forms:
///   * `"workflow": "support_ticket_router"`        → first layered root that has it
///   * `"workflow": "./sub/triage.json"`            → relative to the referring file
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

/// The pinned spec root. Only consulted when a root was pinned via
/// [`set_dsl_dir`]; otherwise resolution is layered (see [`spec_roots`]).
pub fn dsl_dir() -> &'static std::sync::Mutex<String> {
    static D: std::sync::OnceLock<std::sync::Mutex<String>> = std::sync::OnceLock::new();
    D.get_or_init(|| std::sync::Mutex::new("dsl".to_string()))
}

/// Explicitly set the spec root and **pin** it: layered discovery collapses to
/// this single root (the legacy single-root behaviour). The CLI pins here for
/// `--dsl-dir` / `LAYA_DSL_DIR`. Use [`clear_dsl_dir`] to restore layering.
pub fn set_dsl_dir(dir: &str) {
    *dsl_dir().lock().unwrap() = dir.to_string();
    DSL_DIR_PINNED.store(true, std::sync::atomic::Ordering::SeqCst);
}

/// Drop the explicit pin set by [`set_dsl_dir`] and return to layered lookup.
pub fn clear_dsl_dir() {
    DSL_DIR_PINNED.store(false, std::sync::atomic::Ordering::SeqCst);
}

/// True when the spec root was pinned explicitly (single-root mode).
pub fn dsl_dir_pinned() -> bool {
    DSL_DIR_PINNED.load(std::sync::atomic::Ordering::SeqCst)
}

/// Which layer a spec root belongs to. Later layers override earlier ones.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SpecLayer {
    /// `<crate>/dsl` — the specs shipped with the engine.
    Builtin,
    /// `~/.laya-workflow/dsl` (or `$LAYA_HOME` / `$LAYA_USER_DSL_DIR`).
    User,
    /// `.laya-workflow/dsl` or `dsl/`, found by walking up to the git root.
    /// Committed with the repo, so it travels with the code.
    Repo,
    /// `--dsl-dir` / `LAYA_DSL_DIR` / `set_dsl_dir` — replaces every other layer.
    Explicit,
}

impl SpecLayer {
    pub fn as_str(self) -> &'static str {
        match self {
            SpecLayer::Builtin => "builtin",
            SpecLayer::User => "user",
            SpecLayer::Repo => "repo",
            SpecLayer::Explicit => "explicit",
        }
    }
}

/// One resolved spec root and the layer it came from.
#[derive(Debug, Clone)]
pub struct SpecRoot {
    pub path: std::path::PathBuf,
    pub layer: SpecLayer,
}

/// Repo-relative spec directories, searched upward from the cwd (nearest first).
///
/// `.laya-workflow/dsl` wins over a bare `dsl/` so a repo can keep a dedicated,
/// tool-namespaced spec folder without colliding with other `dsl/` uses.
pub const REPO_SPEC_REL_PATHS: [&str; 2] = [".laya-workflow/dsl", "dsl"];

static DSL_DIR_PINNED: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

/// `<crate>/dsl` — the specs compiled into the engine's source tree.
pub fn builtin_spec_dir() -> std::path::PathBuf {
    std::path::PathBuf::from(concat!(env!("CARGO_MANIFEST_DIR"), "/dsl"))
}

/// Per-user spec root: `$LAYA_USER_DSL_DIR` → `~/.laya-workflow/dsl`.
///
/// Lives under the tool's single state root, alongside the installed plugins and
/// the laya-mem store — see [`crate::state`].
pub fn user_spec_dir() -> Option<std::path::PathBuf> {
    crate::state::user_dsl_dir()
}

/// The explicitly pinned root, if any.
pub fn explicit_dsl_dir() -> Option<std::path::PathBuf> {
    if dsl_dir_pinned() {
        Some(std::path::PathBuf::from(dsl_dir().lock().unwrap().clone()))
    } else {
        None
    }
}

/// Walk up from `start` looking for a repo-local spec dir, stopping at the git
/// root.
///
/// A `.git` entry (directory, or a file for worktrees/submodules) marks the
/// repo boundary: the walk checks that directory and then stops, so a `dsl/`
/// belonging to an unrelated parent directory is never adopted.
pub fn discover_repo_spec_dir(start: &std::path::Path) -> Option<std::path::PathBuf> {
    let absolute = if start.is_absolute() {
        start.to_path_buf()
    } else {
        std::env::current_dir().ok()?.join(start)
    };
    let mut dir: Option<&std::path::Path> = Some(absolute.as_path());
    while let Some(d) = dir {
        for rel in REPO_SPEC_REL_PATHS {
            let cand = d.join(rel);
            if cand.is_dir() {
                return Some(cand);
            }
        }
        if d.join(".git").exists() {
            break;
        }
        dir = d.parent();
    }
    None
}

/// Spec roots **high → low** priority.
///
/// When the root is pinned explicitly the result is exactly that one root;
/// otherwise it is `repo → user → builtin`. Duplicate directories (e.g. a repo
/// that *is* the checkout) are collapsed, keeping the higher-priority layer.
pub fn spec_roots() -> Vec<SpecRoot> {
    if let Some(ex) = explicit_dsl_dir() {
        return vec![SpecRoot {
            path: ex,
            layer: SpecLayer::Explicit,
        }];
    }
    let mut roots: Vec<SpecRoot> = Vec::new();
    if let Ok(cwd) = std::env::current_dir() {
        if let Some(repo) = discover_repo_spec_dir(&cwd) {
            roots.push(SpecRoot {
                path: repo,
                layer: SpecLayer::Repo,
            });
        }
    }
    if let Some(user) = user_spec_dir() {
        roots.push(SpecRoot {
            path: user,
            layer: SpecLayer::User,
        });
    }
    roots.push(SpecRoot {
        path: builtin_spec_dir(),
        layer: SpecLayer::Builtin,
    });
    dedup_by_real_path(roots)
}

/// Spec roots **low → high** priority — the order a reader should apply them in,
/// since each later root overrides the ones before it.
pub fn spec_roots_low_to_high() -> Vec<SpecRoot> {
    let mut roots = spec_roots();
    roots.reverse();
    roots
}

/// Directory used for secrets and as the implicit working root: the explicit
/// pin, else the highest-priority root that exists on disk, else the builtin
/// root.
pub fn primary_spec_dir() -> std::path::PathBuf {
    if let Some(ex) = explicit_dsl_dir() {
        return ex;
    }
    for r in spec_roots() {
        if r.path.is_dir() {
            return r.path;
        }
    }
    builtin_spec_dir()
}

/// Drop duplicate roots that resolve to the same directory, keeping the
/// highest-priority occurrence.
fn dedup_by_real_path(roots: Vec<SpecRoot>) -> Vec<SpecRoot> {
    let mut seen: Vec<std::path::PathBuf> = Vec::new();
    let mut out: Vec<SpecRoot> = Vec::new();
    for r in roots {
        let real = r.path.canonicalize().unwrap_or_else(|_| r.path.clone());
        if seen.iter().any(|s| *s == real) {
            continue;
        }
        seen.push(real);
        out.push(r);
    }
    out
}

/// One spec found by [`discover_layered`].
#[derive(Debug, Clone)]
pub struct DiscoveredSpec {
    pub name: String,
    pub path: std::path::PathBuf,
    /// Root directory the spec was found under.
    pub root: std::path::PathBuf,
    /// Layer that root belongs to.
    pub layer: SpecLayer,
    /// Declared `dsl_version` (missing → 1).
    pub version: u64,
    /// Present on a lower-priority spec whose name is already claimed by a
    /// higher-priority `(layer, root)`.
    pub shadowed_by: Option<(SpecLayer, std::path::PathBuf)>,
}

/// Layered discovery over [`spec_roots`] (high → low). The first root to define
/// a name wins; later definitions are reported with `shadowed_by` set so
/// callers can surface the shadowing instead of silently hiding it.
pub fn discover_layered() -> Result<Vec<DiscoveredSpec>> {
    let mut winner: std::collections::HashMap<String, (SpecLayer, std::path::PathBuf)> =
        std::collections::HashMap::new();
    let mut out: Vec<DiscoveredSpec> = Vec::new();
    for root in spec_roots() {
        let root_str = root.path.to_string_lossy().to_string();
        for (name, path) in discover(&root_str)? {
            let version = read_version(&path).unwrap_or(1);
            let shadowed_by = match winner.get(&name) {
                None => {
                    winner.insert(name.clone(), (root.layer, root.path.clone()));
                    None
                }
                Some(hit) => Some(hit.clone()),
            };
            out.push(DiscoveredSpec {
                name,
                path,
                root: root.path.clone(),
                layer: root.layer,
                version,
                shadowed_by,
            });
        }
    }
    Ok(out)
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
        Value::Object(o) if o.contains_key("inline") => {
            Ok((o["inline"].clone(), from_dir.map(|d| d.to_path_buf())))
        }
        Value::String(raw) => {
            let (name, pinned) = split_version(raw);
            if pinned.is_none() {
                if let Some(s) = registry().lock().unwrap().get(&name) {
                    return Ok((s.clone(), None));
                }
            }
            // 1. relative to the referring file's own directory (highest priority)
            if let Some(dir) = from_dir {
                for c in [dir.join(&name), dir.join(format!("{name}.json"))] {
                    if c.is_file() {
                        return Ok((read_spec_file(&c)?, c.parent().map(|d| d.to_path_buf())));
                    }
                }
            }
            // 2. layered roots, high → low; the first hit wins
            let roots = spec_roots();
            for root in &roots {
                let base = root.path.as_path();
                if !base.is_dir() {
                    continue;
                }
                for c in [base.join(&name), base.join(format!("{name}.json"))] {
                    if c.is_file() {
                        return Ok((read_spec_file(&c)?, c.parent().map(|d| d.to_path_buf())));
                    }
                }
                // tree search, honouring the version pin (or picking the highest)
                if let Some(found) = find_in_tree_versioned(base, &name, pinned)? {
                    return Ok((
                        read_spec_file(&found)?,
                        found.parent().map(|d| d.to_path_buf()),
                    ));
                }
            }
            let hint = match pinned {
                Some(v) => format!(" (version {v} pinned)"),
                None => String::new(),
            };
            let searched: Vec<String> = roots
                .iter()
                .map(|r| format!("{} [{}]", r.path.display(), r.layer.as_str()))
                .collect();
            bail!(
                "workflow reference {raw:?} not found{hint} (searched referring dir, {})",
                searched.join(", ")
            )
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
                        hits.push((
                            declared_version(&p.join("workflow.json")).unwrap_or(1),
                            p.join("workflow.json"),
                        ));
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
        return Ok(hits
            .into_iter()
            .filter(|(v, _)| *v == w)
            .map(|(_, p)| p)
            .min());
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

/// "did you mean ...?" hint helpers for the semantic validation errors below.
/// P34: every *unknown-reference* error (node / question / answer / action-kind
/// / gate-option / combine-fallback) appends the closest known identifier so an
/// author sees the fix at the error site. The metric is best-effort guidance
/// only — it never gates a build.
///
/// Optimal-string-alignment (Damerau-Levenshtein restricted to adjacent
/// transpositions) edit distance. An adjacent swap ("at" -> "ta") costs one
/// edit, which is what lets a transposition typo count as a single mistake.
/// O(n*m) but only ever called on short identifiers by `str_similarity`.
fn osa_distance(a: &str, b: &str) -> usize {
    let a: Vec<char> = a.chars().collect();
    let b: Vec<char> = b.chars().collect();
    let (la, lb) = (a.len(), b.len());
    let mut prev2: Vec<usize> = vec![0; lb + 1]; // row i-2
    let mut prev: Vec<usize> = (0..=lb).collect(); // row i-1
    let mut cur: Vec<usize> = vec![0; lb + 1];
    for i in 1..=la {
        cur[0] = i;
        for j in 1..=lb {
            let cost = if a[i - 1] == b[j - 1] { 0 } else { 1 };
            let mut d = (prev[j] + 1).min(cur[j - 1] + 1).min(prev[j - 1] + cost);
            if i > 1 && j > 1 && a[i - 1] == b[j - 2] && a[i - 2] == b[j - 1] {
                d = d.min(prev2[j - 2] + 1);
            }
            cur[j] = d;
        }
        std::mem::swap(&mut prev2, &mut prev);
        std::mem::swap(&mut prev, &mut cur);
        for x in cur.iter_mut() {
            *x = 0;
        }
    }
    prev[lb]
}

/// Character-level similarity in [0,1] between two short identifiers.
/// Combines a bigram Dice coefficient with a longest-common-prefix bonus so a
/// single typo ("biling" -> "billing") ranks well above unrelated names. Used
/// only to build "did you mean ...?" hints in validation errors — never a gate.
pub fn str_similarity(a: &str, b: &str) -> f64 {
    let a = a.trim().to_lowercase();
    let b = b.trim().to_lowercase();
    if a == b {
        return 1.0;
    }
    if a.is_empty() || b.is_empty() {
        return 0.0;
    }
    // Deduplicated bigram sets: repeated characters ("is_risky" has "is"
    // twice) must not double-count toward the Dice coefficient.
    let bigrams = |s: &str| -> std::collections::HashSet<(char, char)> {
        let cs: Vec<char> = s.chars().collect();
        if cs.len() < 2 {
            return std::collections::HashSet::new();
        }
        cs.windows(2).map(|w| (w[0], w[1])).collect()
    };
    let ba = bigrams(&a);
    let bb = bigrams(&b);
    let inter = ba.intersection(&bb).count();
    let dice = if ba.is_empty() && bb.is_empty() {
        0.0
    } else {
        2.0 * inter as f64 / (ba.len() + bb.len()) as f64
    };
    let lcp = a.chars().zip(b.chars()).take_while(|(x, y)| x == y).count() as f64;
    let denom = a.chars().count().max(b.chars().count()).max(1) as f64;
    let base = (dice + 0.35 * lcp / denom).min(1.0);
    // P155: a transposition typo ("satrt" for "start") shares almost no adjacent
    // bigrams, so the Dice+LCP score alone leaves it invisible to `closest_match`.
    // Fall back to a normalized transposition-aware (OSA) edit similarity that
    // sees the swap as one edit. Take the larger of the two so the new term can
    // only ever add a hint, never remove one the bigram metric already found.
    let edit = 1.0 - osa_distance(&a, &b) as f64 / denom;
    base.max(edit)
}

/// Best candidate for a misspelled `target` when one is close enough to be a
/// plausible "did you mean" hint. Returns None when nothing is close — the typo
/// is too far from every known identifier to guess safely.
pub fn closest_match<'a>(target: &str, candidates: &[&'a str]) -> Option<&'a str> {
    let mut best: Option<(f64, &'a str)> = None;
    for c in candidates {
        if c.is_empty() {
            continue;
        }
        let sim = str_similarity(target, c);
        if sim >= 0.5 && best.map(|(b, _)| sim > b).unwrap_or(true) {
            best = Some((sim, c));
        }
    }
    best.map(|(_, c)| c)
}

/// `" (did you mean \"closest\"?)"` appended to a validation error, or `""`
/// when no close candidate exists. Keeps errors readable when there is nothing
/// sensible to guess.
fn suggest(target: &str, candidates: &[&str]) -> String {
    match closest_match(target, candidates) {
        Some(c) => format!(" (did you mean \"{c}\"?)"),
        None => String::new(),
    }
}

/// Recursively validate a threshold rule `when`: every `question` it names must
/// be a declared question (or the action's own), and every operator key must be
/// one the runtime understands. A conjunction `{"all": [..]}` is checked through
/// each conjunct, so a bad question or a typo'd operator inside a conjunction is
/// caught at validate time instead of silently never matching at runtime.
fn validate_threshold_when(
    when: &Value,
    node: &str,
    has_q: &dyn Fn(&str) -> bool,
    aq: Option<&str>,
    q_candidates: &[&str],
    known_keys: &[&str],
    q_types: &std::collections::HashMap<String, String>,
) -> Result<()> {
    if let Some(all) = when.get("all").and_then(Value::as_array) {
        if all.len() < 2 {
            bail!(
                "node {node}: threshold rule `when.all` needs at least two conditions to conjoin"
            );
        }
        for sub in all {
            validate_threshold_when(sub, node, has_q, aq, q_candidates, known_keys, q_types)?;
        }
    }
    let w = when
        .as_object()
        .ok_or_else(|| anyhow!("node {node}: threshold rule `when` must be an object"))?;
    if let Some(q) = w.get("question").and_then(Value::as_str) {
        if !(has_q(q) || aq == Some(q)) {
            bail!(
                "node {node}: threshold rule references unknown question {q:?}{}",
                suggest(q, q_candidates)
            );
        }
    }
    for k in w.keys() {
        if !known_keys.contains(&k.as_str()) {
            bail!(
                "node {node}: threshold rule uses unknown when operator {k:?}{}",
                suggest(k.as_str(), known_keys)
            );
        }
    }
    // P122: a leaf may only read the answer the way its question's *declared
    // type* produces it. The operators read the `Decision` three different ways:
    //   * `option` / `gte` / `lt` -> `d.prob(key)`
    //   * `value_gte` / `value_lt` -> `d.as_f64()`
    //   * `answer` / `answer_in` -> `d.as_str()`
    // A `choice` answer is an option-key string with keyed probabilities; a
    // `score` answer is a number whose probabilities are ordinal ("0","1",…); a
    // `noul` answer is a probability number exposing only "true"/"false". So a
    // leaf whose operator reads a shape its question never produces is not an
    // error at runtime — it silently compares against 0.0 / the number's text and
    // takes a branch nobody chose. Same bug class as a dead `copy_keys` key or a
    // dangling edge; caught here instead of misrouting in production.
    let q_scope = w.get("question").and_then(Value::as_str).or(aq);
    if let Some(q) = q_scope {
        if let Some(t) = q_types.get(q).map(String::as_str) {
            let has = |k: &str| w.contains_key(k);
            if (has("gte") || has("lt")) && !has("option") {
                bail!(
                    "node {node}: threshold rule `gte`/`lt` without `option` — the engine reads them only inside an `option` block, so they are silently ignored and the rule matches on its other keys"
                );
            }
            if (has("value_gte") || has("value_lt")) && t == "choice" {
                bail!(
                    "node {node}: threshold rule applies `value_gte`/`value_lt` to the choice question {q:?} — a choice answer is an option-key string, so `as_f64()` reads 0.0 every time and the comparison never sees the answer"
                );
            }
            if has("option") && t == "score" {
                bail!(
                    "node {node}: threshold rule applies `option` to the score question {q:?} — a score carries ordinal probabilities keyed \"0\",\"1\",…, so `prob` is always 0.0 and the branch is decided by the operator's polarity, not the answer"
                );
            }
            if (has("answer") || has("answer_in")) && t == "noul" {
                bail!(
                    "node {node}: threshold rule applies `answer`/`answer_in` to the noul question {q:?} — a noul answer is the probability text (e.g. \"0.9\"), never \"true\"/\"false\"; compare with `option` + `gte`/`lt`"
                );
            }
        }
    }
    Ok(())
}

/// P141: every `answer`/`answer_in` value in a `when` (recursing through `all`)
/// that reads a *choice* question must be one of that question's options —
/// otherwise the leaf can never match (a dead rule). The runtime compares the
/// answer text to `answer`/`answer_in` (`rule_matches`), so a typo silently
/// produces a rule that never fires, the same bug class as a dangling edge or a
/// dead `copy_keys` key. `default_q` is the question a leaf reads when it names
/// none (the action's resolved question).
fn validate_when_answers(
    w: &Value,
    node: &str,
    questions: &Option<serde_json::Map<String, Value>>,
    default_q: Option<&str>,
) -> Result<()> {
    if let Some(all) = w.get("all").and_then(Value::as_array) {
        for sub in all {
            validate_when_answers(sub, node, questions, default_q)?;
        }
        return Ok(());
    }
    let q = match w.get("question").and_then(Value::as_str).or(default_q) {
        Some(q) => q,
        None => return Ok(()),
    };
    let Some(qdef) = questions.as_ref().and_then(|m| m.get(q)) else {
        return Ok(());
    };
    // Only a `choice` question produces option-key answers.
    if qdef.get("type").and_then(Value::as_str) != Some("choice") {
        return Ok(());
    }
    let space: Option<Vec<String>> = match qdef.get("criteria") {
        Some(Value::Object(m)) if !m.contains_key("from_state") => {
            Some(m.keys().cloned().collect())
        }
        Some(Value::Array(a)) => Some(a.iter().filter_map(|v| v.as_str().map(str::to_string)).collect()),
        _ => None, // from_state → dynamic answer space, cannot check statically
    };
    let Some(space) = space else { return Ok(()) };
    let mut vals: Vec<&str> = Vec::new();
    if let Some(a) = w.get("answer").and_then(Value::as_str) {
        vals.push(a);
    }
    if let Some(arr) = w.get("answer_in").and_then(Value::as_array) {
        vals.extend(arr.iter().filter_map(Value::as_str));
    }
    for v in vals {
        if !space.iter().any(|o| o == v) {
            bail!(
                "node {node}: threshold rule matches {q:?} on answer {v:?}, which is not one of its options {space:?} — the rule can never fire"
            );
        }
    }
    Ok(())
}

/// A threshold rule `when` that matches everything — `{"always": true}`.
/// (`apply_rules_multi` / `rule_matches_multi` return true on `always`
/// before looking at any other operator.)
fn thr_when_is_catch_all(w: &Value) -> bool {
    w.get("always").and_then(Value::as_bool).unwrap_or(false)
}

/// True when a threshold rule matches unconditionally: no `when` at all,
/// or a `when` carrying `always: true`.
fn thr_rule_is_catch_all(r: &Value) -> bool {
    r.get("when").map(thr_when_is_catch_all).unwrap_or(true)
}

/// Core parser: `base_dir` is the directory used to resolve relative refs.
/// Semantic graph validation beyond the structural parse. The engine would
/// otherwise only fail (or silently misroute) at *runtime*, hiding the fault
/// from `validate` / `bdd build` / the dsl smoke gate.
///
/// Checks (P24: edges/action-kind/threshold-question; P25: the rest):
///   * node names are unique across the (expanded) graph
///   * every edge target (condition + default) names an existing node or STOP/START
///   * every non-START node is reachable from `start` through static edges
///   * every action `kind` is one the runtime knows how to run
///   * `primary_q` names a question declared on the node
///   * `action.question` (threshold/gate/merge_open_probs) names a declared
///     question (or the node's own primary_q)
///   * `gate.option` is one of the question's choice criteria
///   * `rubric.dimensions[].question` names a declared question
///   * `combine.choice` / `combine.signal` name declared questions
///   * every threshold rule `when` object only uses known operator keys
///     (unknown keys silently never match at runtime → wrong label)
///   * `copy_keys.keys` name questions declared on the node (an unknown key
///     makes the copy a silent no-op at runtime)
///   * `combine.fallback` is a possible answer of the `combine.choice` question
///     (an unknown fallback can never fire → silently dead branch)
///   * (P39) every capability a node calls — `action.capability` and each
///     `action.chain[].capability` — is declared in the spec's `capabilities`
///     map (an undeclared name passes `validate` today but hard-fails at
///     runtime: `capability "X" is not declared in this spec`; same bug class as
///     a dangling edge or a dead `copy_keys` key)
///   * (P45) `min_confidence` (on the node or its `edge`), when present, is
///     within [0, 1] — > 1 makes every route fail (the node always escalates)
///     and < 0 is a typo; both silently misroute at runtime
///   * (P45) `max_iterations`, when present, is a positive integer — 0 (or a
///     negative value) runs the workflow zero times and a non-integer silently
///     falls back to the default 50, so a spec that claims to bound the loop
///     does not
///   * (P122) a threshold rule leaf's operator is coherent with its question's
///     declared type: `option`/`gte`/`lt` on a `score`, `value_gte`/`value_lt` on
///     a `choice`, `answer`/`answer_in` on a `noul`, and `gte`/`lt` outside an
///     `option` block all compare against a shape the question never produces
///     (0.0 or a number's text), so the rule silently takes a branch nobody chose
///   * (P45) a threshold ladder is total: every rule carries a `label`, the last
///     rule is an unconditional catch-all (else some score emits the literal
///     "unmatched"), no rule follows a catch-all (dead code), and no `when`
///     repeats (first match wins, so the repeat can never fire)
/// The JSON kind of a value, for error messages ("a string", "an array", …).
/// (P142: the `state` projection is validated by kind, so the message names the
/// kind the author actually wrote.)
/// P168: the message for a `policy` value whose JSON type the engine will not
/// read, or `None` when the value is the type its accessor wants. Every key is
/// read leniently (`as_bool` / `as_array` / `as_u64`) and the wrong type silently
/// keeps the engine's default, so the policy that runs is not the one written.
fn policy_value_error(k: &str, v: &Value) -> Option<String> {
    match k {
        "allow_exec" if !v.is_boolean() => Some(format!(
            "`policy` key {k:?} is {}, but it must be a boolean (`true` / `false`) — the engine reads it with `as_bool()` and silently keeps its default (false) on any other type, so a capability that spawns a process stays disabled",
            json_kind_name(v)
        )),
        "allow_hosts" | "allow_paths" => match v.as_array() {
            None => Some(format!(
                "`policy` key {k:?} is {}, but it must be an array of strings — the engine reads it with `as_array()` and silently falls back to the empty list, which for `allow_hosts` means *no* host restriction at all",
                json_kind_name(v)
            )),
            Some(a) => a.iter().find(|x| !x.is_string()).map(|bad| {
                format!(
                    "`policy` key {k:?} must be an array of strings, but it holds {} — the engine's `filter_map(|x| x.as_str())` silently drops a non-string element, so part of the list never applies",
                    json_kind_name(bad)
                )
            }),
        },
        "max_timeout_ms" | "max_output" | "retries" if v.as_u64().is_none() => Some(format!(
            "`policy` key {k:?} is {}, but it must be a non-negative integer — the engine reads it with `as_u64()` and silently keeps its default on any other value",
            json_kind_name(v)
        )),
        // P173: the type above is the *shape*; this is the *range*. The runtime
        // clamps each of these to a hard ceiling at load (`capability::Registry`),
        // so `{"max_timeout_ms": 999999999}` runs at 600000 and prints it — the
        // budget that runs is not the one written. The ceiling comes from the same
        // constant the clamp uses, so the two cannot drift.
        "max_timeout_ms" | "max_output" | "retries" => v.as_u64().and_then(|n| {
            let (ceil, what) = policy_ceiling(k)?;
            (n > ceil).then(|| {
                format!(
                    "`policy` key {k:?} is {n}, but the runtime clamps it to {ceil} ({what}) — a value above the ceiling is silently reduced at load, so the budget that runs is not the one written; lower it to {ceil} or below"
                )
            })
        }),
        _ => None,
    }
}

/// The hard ceiling the runtime clamps a numeric `policy` key to, and a short
/// phrase naming what it bounds. `None` for keys that are not clamped.
fn policy_ceiling(k: &str) -> Option<(u64, &'static str)> {
    match k {
        "max_timeout_ms" => Some((
            crate::capability::POLICY_MAX_TIMEOUT_MS,
            "the hard ceiling for any capability timeout",
        )),
        "max_output" => Some((
            crate::capability::POLICY_MAX_OUTPUT,
            "the hard ceiling for captured output bytes",
        )),
        "retries" => Some((
            crate::capability::POLICY_MAX_RETRIES,
            "the hard ceiling for transport retries",
        )),
        _ => None,
    }
}

/// P183: a question's `heuristic` block is a small *typed* contract. Each key is
/// read with a specific accessor — the offline backend (`backend.rs`), the rules
/// backend (`rules.rs`) or the BDD tooling (`bdd.rs` / `bdd_mcp.rs`) — and a value
/// of the wrong JSON type is silently replaced by that reader's default. Same
/// "a declared value the engine throws away" class as P168 (`policy`) and P179
/// (node / spec), one level deeper: *inside* a question's `heuristic`. The
/// compiler always emits well-typed values (it parses the `.feature` step), so
/// this only ever fires on a hand-written or sidecar spec — exactly where it was
/// silent.
fn heuristic_value_error(k: &str, v: &Value) -> Option<String> {
    // weights / probabilities: read with `as_f64()`; the `.unwrap_or(default)`
    // keeps the default on any other type, so the confidence that runs is not the
    // one written.
    const NUM: &[&str] = &[
        "p_hit", "p_mid", "p_miss", "p_fallback", "p_violated", "p_ok",
        "score_hit", "score_mid", "score_miss",
    ];
    // literal needles / regex patterns: read with `as_array()`; a non-array is an
    // *empty* needle set, so the signal never fires.
    const ARR: &[&str] = &["match_any", "match_mid_any", "match_regex"];
    // field names / fallbacks / literal answers: read with `as_str()`; a non-string
    // is silently dropped (for `field` that means the whole state envelope is
    // matched instead of the named field — the P126 cross-fire hazard).
    const STR: &[&str] = &["field", "extract_numeric", "fallback", "answer_field", "answer"];
    if NUM.contains(&k) {
        if v.as_f64().is_none() {
            return Some(format!(
                "`heuristic` key {k:?} is {}, but it must be a number — the offline backend reads it with `as_f64()` and silently keeps its default, so the confidence it was meant to set is not the one that runs",
                json_kind_name(v)
            ));
        }
        return None;
    }
    if ARR.contains(&k) {
        return match v.as_array() {
            None => Some(format!(
                "`heuristic` key {k:?} is {}, but it must be an array of strings — the backend reads it with `as_array()` and silently matches *no* token when it is not, so the question never fires its signal",
                json_kind_name(v)
            )),
            Some(a) => a.iter().find(|x| !x.is_string()).map(|bad| {
                format!(
                    "`heuristic` key {k:?} must be an array of strings, but it holds {} — the backend's `filter_map(|x| x.as_str())` silently drops a non-string needle, so that token never matches",
                    json_kind_name(bad)
                )
            }),
        };
    }
    if STR.contains(&k) {
        if v.as_str().is_none() {
            return Some(format!(
                "`heuristic` key {k:?} is {}, but it must be a string — the engine reads it with `as_str()` and silently ignores it on any other type, so the field / fallback it names is not the one used",
                json_kind_name(v)
            ));
        }
        return None;
    }
    // `match_rules` (multi-class routing): an object mapping each option to its
    // array of trigger tokens. Read with `as_object()`, then each value with
    // `filter_map(|x| x.as_str())`.
    if k == "match_rules" {
        let Some(m) = v.as_object() else {
            return Some(format!(
                "`heuristic` key {k:?} is {}, but it must be an object mapping each option to an array of trigger tokens — the backend reads it with `as_object()` and silently falls back to the empty rule set, so no option ever fires and the answer always falls to the fallback",
                json_kind_name(v)
            ));
        };
        for (opt, toks) in m {
            if !toks.as_array().map(|a| a.iter().all(Value::is_string)).unwrap_or(false) {
                return Some(format!(
                    "`heuristic` key {k:?} entry {opt:?} is {}, but it must be an array of strings — the backend's `filter_map(|x| x.as_str())` silently drops a non-string trigger, so that option can never win",
                    json_kind_name(toks)
                ));
            }
        }
    }
    None
}

fn json_kind_name(v: &Value) -> &'static str {
    match v {
        Value::Null => "null",
        Value::Bool(_) => "a boolean",
        Value::Number(_) => "a number",
        Value::String(_) => "a string",
        Value::Array(_) => "an array",
        Value::Object(_) => "an object",
    }
}

/// P177/P178: the `call` contract of every `call` step in a spec, for the paths
/// that never go through the BDD compiler (`validate --spec`,
/// `Registry::from_spec`, a side-loaded JSON).
///
/// P164/P166/P175 applied their three `with` checks only while compiling a
/// `.feature`, so a hand-written spec reached `validate` with none of them - a
/// `with.<key>` no call of the kind reads, a value of the wrong type, or a number
/// outside the read site's band all passed with exit 0. P165 did the same for the
/// `project` half: a pointer the engine skips (so the state key is never
/// written), an empty projection key, or a `chain` row keyed by an explicit
/// empty alias. Both decisions are shared with `bdd.rs`
/// ([`crate::capability::with_keys::with_step_issue`],
/// [`crate::capability::call_project::project_step_issue`]) so the two paths
/// cannot drift; only the locator differs (`node <name>:` here, `line N:` there).
fn guard_call_spec(caps: &Map<String, Value>, node_list: &[Value]) -> Result<()> {
    for n in node_list {
        let Some(obj) = n.as_object() else { continue };
        let Some(action) = obj.get("action").and_then(Value::as_object) else {
            continue;
        };
        if action.get("kind").and_then(Value::as_str) != Some("call") {
            continue;
        }
        let nm = obj.get("name").and_then(Value::as_str).unwrap_or("?");
        let cap = action.get("capability").and_then(Value::as_str).unwrap_or("");
        let with = action
            .get("with")
            .and_then(Value::as_object)
            .cloned()
            .unwrap_or_default();
        let chain: Vec<Value> = action
            .get("chain")
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default();
        if let Some(issue) = crate::capability::with_keys::with_step_issue(caps, cap, &with, &chain)
        {
            bail!(
                "node {nm}: {}",
                crate::capability::with_keys::with_issue_body(&issue)
            );
        }
        let project = action
            .get("project")
            .and_then(Value::as_object)
            .cloned()
            .unwrap_or_default();
        if let Some(issue) = crate::capability::call_project::project_step_issue(&project, &chain) {
            bail!(
                "node {nm}: {}",
                crate::capability::call_project::project_issue_body(&issue)
            );
        }
    }
    Ok(())
}

/// P179: a node/spec value whose *type* the parser silently discards.
///
/// `node_from_spec_with` / `from_spec_with_dir_cycle` read every structural value
/// with a typed accessor (`as_f64` / `as_u64` / `as_str` / `as_object`) and fall
/// back to a default whenever the accessor returns `None` — a *silently replaced*
/// declaration. Same "a declared value the engine throws away" class as P148's
/// keys and P168's policy values, but on the node/spec values themselves:
/// `min_confidence: "0.9"` reads 0.0 (every route below the intended floor now
/// passes), `max_retries: "5"` / `-1` / `2.5` reads the engine default (2),
/// `edge.default: 5` reads `None` (the node has **no** fallback route at all),
/// `edge: []` / `questions: "q"` / `action: "copy"` are dropped whole, and
/// `convergence_window: "3"` / `convergence_eps: "0.001"` are ignored. The
/// compiler always emits well-typed values, so this only ever fires on a
/// hand-written / side-loaded spec — which is exactly where it was silent.
///
/// Type only: a value's *range* is a separate axis (`min_confidence` ∈ [0, 1] is
/// checked in `validate_graph_semantics`), and JSON `null` or a `${...}` template
/// is not a wrong type — `null` is the describe/round-trip spelling of an absent
/// optional value and `${...}` is expanded by the engine at run time — so neither
/// is judged here.
fn guard_spec_value_types(spec: &Value, node_list: &[Value]) -> Result<()> {
    // `null` (an absent optional) and a `${...}` template (resolved by the engine
    // at run time, so its static JSON type is not what the author means) are never
    // judged as a wrong type.
    let unjudged = |v: &Value| v.is_null() || v.as_str().is_some_and(|s| s.contains("${"));

    if let Some(obj) = spec.as_object() {
        if let Some(v) = obj.get("name") {
            if !unjudged(v) && v.as_str().is_none() {
                bail!(
                    "spec `name` is {} — the engine reads it with `as_str()` and silently falls back to the placeholder `sub`, so the workflow is not named what you declared",
                    json_kind_name(v)
                );
            }
        }
        if let Some(v) = obj.get("convergence_window") {
            if !unjudged(v) && v.as_u64().is_none() {
                bail!(
                    "spec `convergence_window` is {} — the engine reads it with `as_u64()` and silently keeps the default 5, so the stall window is not what you declared",
                    json_kind_name(v)
                );
            }
        }
        if let Some(v) = obj.get("convergence_eps") {
            if !unjudged(v) && v.as_f64().is_none() {
                bail!(
                    "spec `convergence_eps` is {} — the engine reads it with `as_f64()` and silently keeps the default 1e-4, so the convergence threshold is not what you declared",
                    json_kind_name(v)
                );
            }
        }
    }

    for n in node_list {
        let Some(obj) = n.as_object() else { continue };
        let nm = obj.get("name").and_then(Value::as_str).unwrap_or("?");
        if let Some(v) = obj.get("questions") {
            if !unjudged(v) && v.as_object().is_none() {
                bail!(
                    "node {nm}: `questions` is {} — the engine reads it with `as_object()` and silently drops it, so the node asks nothing",
                    json_kind_name(v)
                );
            }
            if let Some(qs) = v.as_object() {
                for (qid, qv) in qs {
                    if qv.as_object().is_none() {
                        bail!(
                            "node {nm}: question {qid:?} is {} — a question must be an object (type / instructions / criteria / heuristic), so a non-object entry is silently dropped and the node never asks it",
                            json_kind_name(qv)
                        );
                    }
                }
            }
        }
        if let Some(v) = obj.get("edge") {
            if !unjudged(v) && v.as_object().is_none() {
                bail!(
                    "node {nm}: `edge` is {} — the engine reads it with `as_object()` and silently drops the whole edge, so the node has no routes at all",
                    json_kind_name(v)
                );
            }
            if let Some(e) = v.as_object() {
                if let Some(c) = e.get("condition") {
                    if !unjudged(c) && c.as_object().is_none() {
                        bail!(
                            "node {nm}: `edge.condition` is {} — the engine reads it with `as_object()` and silently drops every route, so no answer resolves",
                            json_kind_name(c)
                        );
                    }
                }
                if let Some(d) = e.get("default") {
                    if !unjudged(d) && d.as_str().is_none() {
                        bail!(
                            "node {nm}: `edge.default` is {} — the engine reads it with `as_str()` and silently falls back to *no* default route, so an unmatched answer ESCALATEs instead of falling back",
                            json_kind_name(d)
                        );
                    }
                }
                if let Some(mc) = e.get("min_confidence") {
                    if !unjudged(mc) && mc.as_f64().is_none() {
                        bail!(
                            "node {nm}: `edge.min_confidence` is {} — the engine reads it with `as_f64()` and silently falls back to 0.0, so every route passes",
                            json_kind_name(mc)
                        );
                    }
                }
            }
        }
        if let Some(v) = obj.get("min_confidence") {
            if !unjudged(v) && v.as_f64().is_none() {
                bail!(
                    "node {nm}: `min_confidence` is {} — the engine reads it with `as_f64()` and silently falls back to 0.0, so every route passes",
                    json_kind_name(v)
                );
            }
        }
        if let Some(v) = obj.get("primary_q") {
            if !unjudged(v) && v.as_str().is_none() {
                bail!(
                    "node {nm}: `primary_q` is {} — the engine reads it with `as_str()` and silently ignores it, so the routed question falls back to declaration order",
                    json_kind_name(v)
                );
            }
        }
        if let Some(v) = obj.get("max_retries") {
            if !unjudged(v) && v.as_u64().is_none() {
                bail!(
                    "node {nm}: `max_retries` is {} — the engine reads it with `as_u64()` and silently keeps the default 2, so the node retries a different number of times than you declared",
                    json_kind_name(v)
                );
            }
        }
        if let Some(v) = obj.get("action") {
            if !unjudged(v) && v.as_object().is_none() {
                bail!(
                    "node {nm}: `action` is {} — the engine reads it with `as_object()` inside `run_action`, so a non-object action is silently a no-op",
                    json_kind_name(v)
                );
            }
        }
    }
    Ok(())
}

/// P180: a node/spec value in the wrong *range* is silently obeyed with a
/// nonsense outcome — the twin of P179 (wrong type), and the same "a declared
/// value does not mean what it says" class as P168/P173 for the policy numbers.
/// The two convergence knobs are the ones the engine reads without a floor:
///   * `convergence_window: 0` makes the convergence test slice an **empty**
///     window (`score_history[len - 0..]`), whose fold is `mn = +inf, mx = -inf`,
///     so `mx - mn = -inf <= eps` is always true: the run reports
///     `final_action: "converged"` and stops after the very **first** node,
///     whatever the workflow is. A window of at least 1 is the smallest thing
///     that can mean "look at the last N scores".
///   * `convergence_eps < 0` can never be met (`mx - mn` is never negative), so
///     the convergence guard never fires and the workflow runs to
///     `max_iterations`. A tolerance must be `>= 0`.
/// (`max_iterations >= 1` is checked next to the parse; `min_confidence ∈ [0, 1]`
/// in `validate_graph_semantics`; `state` / `policy` ranges have their own gates.)
fn guard_spec_value_ranges(spec: &Value) -> Result<()> {
    let Some(obj) = spec.as_object() else { return Ok(()) };
    if let Some(v) = obj.get("convergence_window").and_then(Value::as_u64) {
        if v == 0 {
            bail!(
                "spec `convergence_window` is 0 — a zero-length window folds an empty slice, so `mx - mn` is -inf and the run stops after the very first node claiming `final_action: \"converged\"`; use at least 1"
            );
        }
    }
    if let Some(v) = obj.get("convergence_eps").and_then(Value::as_f64) {
        if v < 0.0 {
            bail!(
                "spec `convergence_eps` is {v} — a negative tolerance can never be met (`mx - mn` is never negative), so the convergence guard never fires and the workflow runs to `max_iterations`; a tolerance must be >= 0"
            );
        }
    }
    Ok(())
}

fn validate_graph_semantics(
    nodes: &[WorkflowNode],
    node_list: &[Value],
    start: &str,
    declared_caps: &std::collections::HashSet<String>,
) -> Result<()> {
    let names: std::collections::HashSet<&str> =
        nodes.iter().map(|n| n.name.as_str()).collect();
    let name_candidates: Vec<&str> = names.iter().copied().collect();
    // Routing-target hints: a node name, but also any *answer value* — authors
    // routinely route to an answer spelling they just typed (a node that does
    // not exist yet, or a mistyped answer). Including both lets the hint point
    // at the typo instead of silently saying "nothing close".
    let mut answer_candidates: Vec<&str> = Vec::new();
    for n in nodes {
        if let Some(qs) = n.questions.as_object() {
            for qv in qs.values() {
                match qv.get("criteria") {
                    Some(Value::Object(m)) => {
                        answer_candidates.extend(m.keys().map(|k| k.as_str()))
                    }
                    Some(Value::Array(a)) => {
                        answer_candidates.extend(a.iter().filter_map(|v| v.as_str()))
                    }
                    _ => {}
                }
            }
        }
    }
    let mut dest_candidates = name_candidates.clone();
    dest_candidates.extend(answer_candidates);
    if !names.contains(start) {
        bail!(
            "spec start node {start:?} is not in the graph{}",
            suggest(start, &name_candidates)
        );
    }
    // unique node names (a duplicate makes edges ambiguous)
    let mut seen: std::collections::HashSet<&str> = std::collections::HashSet::new();
    for n in nodes {
        if !seen.insert(n.name.as_str()) {
            bail!("duplicate node name {:?} in graph", n.name);
        }
    }
    for n in nodes {
        for dest in n.edge.condition.values() {
            if dest != "STOP" && dest != "START" && !names.contains(dest.as_str()) {
                bail!(
                    "node {}: edge condition targets unknown node {dest:?}{}",
                    n.name,
                    suggest(dest, &dest_candidates)
                );
            }
        }
        if let Some(d) = &n.edge.default {
            if d != "STOP" && d != "START" && !names.contains(d.as_str()) {
                bail!(
                    "node {}: edge default targets unknown node {d:?}{}",
                    n.name,
                    suggest(d, &dest_candidates)
                );
            }
        }
    }
    // edge condition *keys* must be possible answers of the node's primary
    // question. Routing matches the primary answer against these keys
    // (`Edge::resolve`), so a key that is not a criterion can never fire —
    // silently dead routing. Skipped for `from_state` criteria, whose answer
    // space is populated dynamically at runtime.
    for n in nodes {
        let pq = n
            .primary_q
            .clone()
            .or_else(|| n.questions.as_object().and_then(|m| m.keys().next().cloned()));
        let Some(pq) = pq else { continue };
        let Some(qdef) = n.questions.get(&pq) else { continue };
        if qdef.get("criteria").and_then(|c| c.get("from_state")).is_some() {
            continue;
        }
        let answer_space: Option<Vec<String>> = match qdef.get("criteria") {
            Some(Value::Object(m)) if !m.contains_key("from_state") => {
                Some(m.keys().cloned().collect())
            }
            Some(Value::Array(a)) => {
                Some(a.iter().filter_map(|v| v.as_str().map(str::to_string)).collect())
            }
            _ => None,
        };
        if let Some(space) = answer_space {
            let ans_candidates: Vec<&str> = space.iter().map(|s| s.as_str()).collect();
            for k in n.edge.condition.keys() {
                if !space.iter().any(|a| a == k) {
                    bail!(
                        "node {}: edge condition key {k:?} is not an answer of primary question {pq:?}{}",
                        n.name,
                        suggest(k, &ans_candidates)
                    );
                }
            }
        }
    }
    // A dynamic question (`criteria.from_state`) materialises its options from a
    // state *list* at decide time; every other key in its `criteria` is therefore
    // a recognised control knob, not an answer. A mistyped knob (`id_keey`) would
    // otherwise be silently ignored and the question would materialise with the
    // default id key, hiding an authoring bug behind "it happens to work". So when
    // `from_state` is present, the sibling keys must be exactly the known ones.
    {
        const DYN_KEYS: [&str; 4] = ["from_state", "id_key", "value_key", "none"];
        for n in nodes {
            let Some(qs) = n.questions.as_object() else { continue };
            for (qid, q) in qs {
                let Some(m) = q.get("criteria").and_then(Value::as_object) else {
                    continue;
                };
                if !m.contains_key("from_state") {
                    continue;
                }
                for k in m.keys() {
                    if !DYN_KEYS.contains(&k.as_str()) {
                        bail!(
                            "node {}: question {qid:?} declares `criteria.from_state` but also carries the unrecognised key {k:?} — a dynamic question's only criteria keys are {DYN_KEYS:?}{}",
                            n.name,
                            suggest(k.as_str(), &DYN_KEYS)
                        );
                    }
                }
                if m.get("from_state").and_then(Value::as_str).map(str::is_empty) != Some(false)
                {
                    bail!(
                        "node {}: question {qid:?} has `criteria.from_state` that is not a non-empty string (it must name the state list the options are materialised from)",
                        n.name
                    );
                }
            }
        }
    }
    // reachability: every non-start node must be reachable through static edges.
    // Routing is purely edge-driven (no dynamic goto), so an unreachable node is
    // dead code — it will never run, silently hiding misrouted decisions.
    {
        let mut reachable: std::collections::HashSet<&str> =
            std::collections::HashSet::from([start]);
        loop {
            let mut grew = false;
            for n in nodes {
                if !reachable.contains(n.name.as_str()) {
                    continue;
                }
                for dest in n.edge.condition.values() {
                    if dest != "STOP" && dest != "START" && reachable.insert(dest) {
                        grew = true;
                    }
                }
                if let Some(d) = &n.edge.default {
                    if d != "STOP" && d != "START" && reachable.insert(d.as_str()) {
                        grew = true;
                    }
                }
                // a bare "START" destination loops back to the workflow start
                if n.edge.condition.values().any(|d| d == "START")
                    || n.edge.default.as_deref() == Some("START")
                {
                    if reachable.insert(start) {
                        grew = true;
                    }
                }
            }
            if !grew {
                break;
            }
        }
        for n in nodes {
            if n.name != start && !reachable.contains(n.name.as_str()) {
                bail!(
                    "node {} is unreachable from start {start:?} (dead code — it never runs)",
                    n.name
                );
            }
        }
    }
    const KNOWN_KINDS: &[&str] = &[
        "none", "copy_keys", "merge_open_probs", "threshold", "rubric",
        "combine", "gate", "ui_proposal", "call",
    ];
    const KNOWN_WHEN_KEYS: &[&str] = &[
        "question", "always", "all", "answer", "answer_in", "option", "gte",
        "lt", "value_gte", "value_lt",
    ];
    // P145: the keys `node_from_spec_with` actually reads off a node object and
    // its `edge`. Anything else is never touched by the parser or the engine, so a
    // misspelling — `questoins`, `actoin`, `primmary_q`, a stray `condtion` on the
    // edge — is silently dropped: `validate` passes yet the declared behaviour is
    // gone (no questions / no action / no routing). Same class as P142's `state`
    // (there a silent fallback sent the whole state); here a silent drop *deletes*
    // the node's questions, action or routing. `has_action` is the one derived key
    // (emitted by describe / round-trip), so it is recognised but must *agree* with
    // the presence of `action`.
    const KNOWN_NODE_KEYS: &[&str] = &[
        "name", "questions", "edge", "min_confidence", "primary_q",
        "max_retries", "state", "action", "workflow", "has_action",
    ];
    const KNOWN_EDGE_KEYS: &[&str] = &["condition", "default", "min_confidence"];
    for n in node_list {
        if let Some(obj) = n.as_object() {
            let nm = obj.get("name").and_then(Value::as_str).unwrap_or("?");
            for k in obj.keys() {
                if !KNOWN_NODE_KEYS.contains(&k.as_str()) {
                    bail!(
                        "node {nm}: unrecognised key {k:?}{} — the engine never reads it, so it is silently ignored",
                        suggest(k, KNOWN_NODE_KEYS)
                    );
                }
            }
            if let Some(ha) = obj.get("has_action") {
                let has = obj.contains_key("action");
                if ha.as_bool() != Some(has) {
                    bail!(
                        "node {nm}: `has_action` is {ha} but the node {} — `has_action` is derived from `action` and must agree",
                        if has { "declares an `action`" } else { "declares no `action`" }
                    );
                }
            }
            if let Some(edge) = obj.get("edge").and_then(Value::as_object) {
                for k in edge.keys() {
                    if !KNOWN_EDGE_KEYS.contains(&k.as_str()) {
                        bail!(
                            "node {nm}: edge carries an unrecognised key {k:?}{} — only `condition`, `default` and `min_confidence` are read, so it is silently ignored",
                            suggest(k, KNOWN_EDGE_KEYS)
                        );
                    }
                }
            }
        }
        // P146: a question object and its `heuristic` are read by name too. The
        // engine reads a question's `type` / `instructions` / `criteria` /
        // `heuristic` (the rules engine also reads `violating` / `violating_from`),
        // and the offline backend reads the heuristic keys listed below. Any *other*
        // key is never touched, so a typo — `critera`, `answer_feild`, `matc_any` —
        // silently drops the question's options or the offline read-out: the same
        // class as P145's node/edge keys, one level deeper. `truthy` / `falsy` are
        // compiler-emitted `noul` annotations (accepted, like `has_action`); `answer`
        // is the compiler's `heur_answer` step (round-tripped by `bdd import`) though
        // today only `default_choice`'s first-key fallback answers such a question.
        const KNOWN_QUESTION_KEYS: &[&str] = &[
            "type", "instructions", "criteria", "heuristic",
            "truthy", "falsy", "violating", "violating_from",
        ];
        const KNOWN_HEURISTIC_KEYS: &[&str] = &[
            "answer_field", "field", "extract_numeric", "match_any", "match_mid_any",
            "match_regex", "match_rules", "fallback", "answer", "p_hit", "p_mid",
            "p_miss", "p_fallback", "p_violated", "p_ok", "score_hit", "score_mid",
            "score_miss",
        ];
        if let Some(qs) = n.get("questions").and_then(Value::as_object) {
            let qn = n.get("name").and_then(Value::as_str).unwrap_or("?");
            for (qid, q) in qs {
                let Some(qo) = q.as_object() else { continue };
                // P181: a question's `type` is read with `unwrap_or("choice")`
                // (backend.rs / workflow.rs / bdd.rs / bdd_mcp.rs) and
                // `unwrap_or("boolean")` (rules.rs), so an unrecognised or missing
                // value is silently replaced by `choice`: the question is then
                // answered and routed as a choice (its answer keys the node's edge
                // condition) and the tooling reports the type as `choice` too, so
                // the author never sees the slip. Measured: a `score`-shaped question
                // mis-typed `scor` made `run` route on the *answer string* (`a` -> `b`)
                // where the real `score` answer (a number) matched no edge and stopped
                // at `a`. Both the unknown value and the missing key are rejected.
                const KNOWN_QUESTION_TYPES: &[&str] = &["choice", "score", "noul", "boolean"];
                match qo.get("type") {
                    None => bail!(
                        "node {qn}: question {qid:?} declares no `type` — the engine reads a missing type as `choice`, so the question is silently answered and routed as a choice; add \"type\": \"choice\" / \"score\" / \"noul\""
                    ),
                    Some(v) if v.as_str().is_some_and(|t| KNOWN_QUESTION_TYPES.contains(&t)) => {}
                    Some(v) => bail!(
                        "node {qn}: question {qid:?} declares type {v} — the engine reads only `choice` / `score` / `noul` (and `boolean` in the rules backend), and any other value is silently replaced by `choice` (the answer then keys the node's edge condition and the tooling reports a choice){}",
                        v.as_str().map(|t| suggest(t, KNOWN_QUESTION_TYPES)).unwrap_or_default()
                    ),
                }
                // P182: `criteria` is not free-form — its *shape* is read with a
                // specific accessor per type, and a wrong shape is silently dropped.
                // `choice` reads it with `as_object()` (the option table, or the
                // `from_state` control object); anything else makes the offline
                // backend fabricate a `{A: ..., B: ...}` pair (`default_choice`) and
                // makes the answer-space check below skip *entirely*, so a branch
                // keyed on the real options silently dies while `validate` stays
                // green. Measured: a choice question whose `criteria` was the string
                // "approve or deny" left `validate` at exit 0 (edge keys
                // `approve`/`deny` unchecked) and `run` answered the fabricated "A"
                // that matched no edge, so the `approve -> b` branch never fired.
                // `score` reads its level names with `as_array()`; a non-array (or a
                // non-string / empty one) drops the levels the same way.
                match qo.get("type").and_then(Value::as_str) {
                    Some("choice") => match qo.get("criteria") {
                        Some(Value::Object(m)) if !m.is_empty() => {}
                        Some(Value::Object(_)) => bail!(
                            "node {qn}: question {qid:?} is a `choice` with an empty `criteria` object — it offers no option, so no answer can key its edge and the offline backend fabricates an `{{A, B}}` pair instead"
                        ),
                        None => bail!(
                            "node {qn}: question {qid:?} is a `choice` with no `criteria` — the options *are* `criteria` (an object `{{\"<answer>\": \"<what it means>\"}}`, or `{{\"from_state\": …}}` for a dynamic question); without it the engine reads no option and the offline backend fabricates an `{{A, B}}` pair, so a branch keyed on the real options silently dies"
                        ),
                        Some(v) => bail!(
                            "node {qn}: question {qid:?} is a `choice` but its `criteria` is {} — the engine reads a choice's options with `as_object()`, so this is silently dropped (the offline backend then fabricates an `{{A, B}}` pair) and the edge-condition-key check is skipped; write `criteria` as an object",
                            json_kind_name(v)
                        ),
                    },
                    Some("score") => {
                        if let Some(v) = qo.get("criteria") {
                            let ok = v.as_array().is_some_and(|a| {
                                !a.is_empty() && a.iter().all(Value::is_string)
                            });
                            if !ok {
                                bail!(
                                    "node {qn}: question {qid:?} is a `score` but its `criteria` is {} — a score's levels are an array of level names (e.g. [\"poor\", \"ok\", \"good\"]), so anything else is silently dropped and the levels disappear from the question contract",
                                    json_kind_name(v)
                                );
                            }
                        }
                    }
                    _ => {}
                }
                for k in qo.keys() {
                    if !KNOWN_QUESTION_KEYS.contains(&k.as_str()) {
                        bail!(
                            "node {qn}: question {qid:?} carries an unrecognised key {k:?}{} — the engine never reads it, so it is silently ignored",
                            suggest(k, KNOWN_QUESTION_KEYS)
                        );
                    }
                }
                if let Some(h) = qo.get("heuristic").and_then(Value::as_object) {
                    for k in h.keys() {
                        if !KNOWN_HEURISTIC_KEYS.contains(&k.as_str()) {
                            bail!(
                                "node {qn}: question {qid:?} heuristic carries an unrecognised key {k:?}{} — the offline backend never reads it, so it is silently ignored",
                                suggest(k, KNOWN_HEURISTIC_KEYS)
                            );
                        }
                    }
                    // P183: a heuristic *value* of the wrong JSON type is read
                    // with a typed accessor and silently replaced by its default
                    // (the P168 policy / P179 node-spec mirror, one level deeper).
                    for (k, v) in h {
                        if let Some(msg) = heuristic_value_error(k, v) {
                            bail!("node {qn}: question {qid:?} {msg}");
                        }
                    }
                }
            }
        }
        if n.get("workflow").is_some() {
            continue; // reference node; its expanded nodes are validated above
        }
        let name = n.get("name").and_then(|v| v.as_str()).unwrap_or("?");
        let questions = n.get("questions").and_then(|v| v.as_object()).cloned();
        let q_candidates: Vec<&str> = questions
            .as_ref()
            .map(|m| m.keys().map(|k| k.as_str()).collect())
            .unwrap_or_default();
        // P122: each question's declared type, so a rule leaf's operator can be
        // matched against the shape that question actually produces.
        let q_types: std::collections::HashMap<String, String> = questions
            .as_ref()
            .map(|m| {
                m.iter()
                    .filter_map(|(k, v)| {
                        v.get("type")
                            .and_then(Value::as_str)
                            .map(|t| (k.clone(), t.to_string()))
                    })
                    .collect()
            })
            .unwrap_or_default();
        let has_q = |q: &str| {
            questions.as_ref().map(|m| m.contains_key(q)).unwrap_or(false)
        };
        // primary_q must name a declared question
        if let Some(pq) = n.get("primary_q").and_then(|v| v.as_str()) {
            if !has_q(pq) {
                bail!(
                    "node {name}: primary_q {pq:?} is not a declared question{}",
                    suggest(pq, &q_candidates)
                );
            }
        }
        // P136: with several questions, routing on the primary is only well-defined
        // when the primary is named. `Edge::resolve` matches the *primary* answer
        // against the edge condition keys, and `WorkflowNode::primary()` falls back to
        // the first-declared question — so a node that declares >=2 questions, carries
        // edge condition keys and names no `primary_q` would silently route on
        // whichever question happens to be declared first (a hand-written spec has no
        // compiler to have made that choice explicit).
        let n_qs = questions.as_ref().map(|m| m.len()).unwrap_or(0);
        let cond_keys: Vec<&str> = n
            .get("edge")
            .and_then(|e| e.get("condition"))
            .and_then(|c| c.as_object())
            .map(|m| m.keys().map(String::as_str).collect())
            .unwrap_or_default();
        if n_qs >= 2
            && !cond_keys.is_empty()
            && n.get("primary_q").and_then(Value::as_str).is_none()
        {
            bail!(
                "node {name}: declares {n_qs} questions and routes on a question (edge condition keys {cond_keys:?}) but names no `primary_q` — the routed question would otherwise be inferred from declaration order, so name it explicitly"
            );
        }
        // edge `min_confidence` gates every route (an answer below it resolves
        // to no edge → the node ESCALATEs). A value outside [0, 1] is
        // meaningless: > 1 makes every route fail (the node always escalates),
        // < 0 is a typo, and both silently mis-route instead of failing.
        let mc = n
            .get("min_confidence")
            .or_else(|| n.get("edge").and_then(|e| e.get("min_confidence")))
            .and_then(|v| v.as_f64());
        if let Some(mc) = mc {
            if !(0.0..=1.0).contains(&mc) {
                bail!(
                    "node {name}: min_confidence {mc} is outside [0, 1] — routes below it escalate, and > 1 makes every route fail"
                );
            }
        }
        // P142: the node's `state` projection must mean what it says. `project_state`
        // reads only `{"keep": [<state key>…], "rename": {<out>: <state key>}}`;
        // anything else — a non-object `state`, a `keep` that is not a string array,
        // a `rename` that is not a string→string map, an unrecognised key, or a
        // `state` object that declares neither — is *silently ignored*, so the node
        // sends the WHOLE state to the backend: a projection that never projects.
        // Same class as P139/P141 — `validate` must reject a shape that can only
        // quietly disagree with its declared intent.
        if let Some(state) = n.get("state") {
            if !state.is_null() {
                let Some(st) = state.as_object() else {
                    bail!(
                        "node {name}: `state` must be an object (`{{\"keep\": […], \"rename\": {{…}}}}`) or null, not {} — a non-object `state` is silently ignored and the whole state is sent to the backend",
                        json_kind_name(state)
                    );
                };
                for k in st.keys() {
                    if k != "keep" && k != "rename" {
                        bail!(
                            "node {name}: `state` carries the unrecognised key {k:?}{} — only `keep` and `rename` are read, so it is silently ignored (the whole state is sent)",
                            suggest(k, &["keep", "rename"])
                        );
                    }
                }
                if let Some(keep) = st.get("keep") {
                    match keep.as_array() {
                        None => bail!(
                            "node {name}: `state.keep` must be an array of state-key strings, not {} — a non-array `keep` is silently ignored",
                            json_kind_name(keep)
                        ),
                        Some(a) => {
                            for (i, v) in a.iter().enumerate() {
                                if v.as_str().is_none() {
                                    bail!(
                                        "node {name}: `state.keep[{i}]` is {}, not a string — a non-string key is silently dropped from the projection",
                                        json_kind_name(v)
                                    );
                                }
                            }
                        }
                    }
                }
                if let Some(rename) = st.get("rename") {
                    match rename.as_object() {
                        None => bail!(
                            "node {name}: `state.rename` must be an object mapping each output key to a source state key, not {} — a non-object `rename` is silently ignored",
                            json_kind_name(rename)
                        ),
                        Some(m) => {
                            for (to, from) in m {
                                if from.as_str().is_none() {
                                    bail!(
                                        "node {name}: `state.rename[{to:?}]` is {}, not a string — a non-string source key is silently dropped",
                                        json_kind_name(from)
                                    );
                                }
                            }
                        }
                    }
                }
                if !st.contains_key("keep") && !st.contains_key("rename") {
                    bail!(
                        "node {name}: `state` declares neither `keep` nor `rename` — it can never project anything, so the whole state is sent; omit `state` or add a projection"
                    );
                }
            }
        }
        if let Some(action) = n.get("action") {
            if let Some(kind) = action.get("kind").and_then(|v| v.as_str()) {
                if !KNOWN_KINDS.contains(&kind) {
                    bail!(
                        "node {name}: unknown action kind {kind:?}{}",
                        suggest(kind, KNOWN_KINDS)
                    );
                }
                // P147: the action object, its `rules[]` / `dimensions[]` and its
                // `chain[]` steps are read by name too. `run_action` /
                // `run_call_action` read only the keys below — anything else
                // (`capabilty`, `rulez`, `dimenions`, `wieght`) is never touched, so
                // the action silently loses its capability / rules / dimensions
                // behind a green `validate`. Same class as P145/P146, one level
                // deeper still. `question` is read for every kind; a ui_proposal's
                // `surface` / `snapshot_hash` (the state keys it reads) and a chain
                // step's `project` are emitted by the compiler and round-tripped by
                // `bdd import`.
                const COMMON_ACTION_KEYS: &[&str] = &["kind", "question"];
                let kind_keys: &[&str] = match kind {
                    "none" | "merge_open_probs" => &[],
                    "copy_keys" => &["keys"],
                    "threshold" => &["rules"],
                    "rubric" => &["dimensions", "min_confidence"],
                    "combine" => &[
                        "choice", "signal", "min_confidence", "fallback", "review_label",
                        "high_threshold", "low_threshold",
                    ],
                    "gate" => &["option", "block_at", "allow_at"],
                    "ui_proposal" => &[
                        "operation", "field", "amount", "min_confidence", "surface",
                        "snapshot_hash", "value_key",
                    ],
                    "call" => &["capability", "with", "chain", "project"],
                    _ => &[],
                };
                if let Some(ao) = action.as_object() {
                    let mut allowed: Vec<&str> = COMMON_ACTION_KEYS.to_vec();
                    allowed.extend_from_slice(kind_keys);
                    for k in ao.keys() {
                        if !allowed.contains(&k.as_str()) {
                            bail!(
                                "node {name}: action {kind:?} carries an unrecognised key {k:?}{} — the engine never reads it, so it is silently ignored",
                                suggest(k, &allowed)
                            );
                        }
                    }
                }
                if kind == "threshold" {
                    if let Some(rules) = action.get("rules").and_then(Value::as_array) {
                        for (i, r) in rules.iter().enumerate() {
                            let Some(ro) = r.as_object() else { continue };
                            for k in ro.keys() {
                                if k != "when" && k != "label" {
                                    bail!(
                                        "node {name}: threshold rule #{i} carries an unrecognised key {k:?}{} — the engine never reads it, so it is silently ignored",
                                        suggest(k, &["when", "label"])
                                    );
                                }
                            }
                        }
                    }
                }
                if kind == "rubric" {
                    if let Some(dims) = action.get("dimensions").and_then(Value::as_array) {
                        for (i, d) in dims.iter().enumerate() {
                            let Some(dim) = d.as_object() else { continue };
                            for k in dim.keys() {
                                if k != "question" && k != "levels" && k != "weight" {
                                    bail!(
                                        "node {name}: rubric dimension #{i} carries an unrecognised key {k:?}{} — the engine never reads it, so it is silently ignored",
                                        suggest(k, &["question", "levels", "weight"])
                                    );
                                }
                            }
                        }
                    }
                }
                if kind == "call" {
                    if let Some(steps) = action.get("chain").and_then(Value::as_array) {
                        for (i, st) in steps.iter().enumerate() {
                            let Some(so) = st.as_object() else { continue };
                            for k in so.keys() {
                                if k != "capability" && k != "as" && k != "with" && k != "project" {
                                    bail!(
                                        "node {name}: chain step #{i} carries an unrecognised key {k:?}{} — the engine never reads it, so it is silently ignored",
                                        suggest(k, &["capability", "as", "with", "project"])
                                    );
                                }
                            }
                        }
                    }
                }
                let aq = action.get("question").and_then(|v| v.as_str());
                match kind {
                    "threshold" | "gate" | "merge_open_probs" => {
                        // action.question must be a declared question (or primary_q)
                        if let Some(q) = aq {
                            if !has_q(q)
                                && n.get("primary_q").and_then(|v| v.as_str()) != Some(q)
                            {
                                bail!(
                                    "node {name}: action {kind:?} references unknown question {q:?}{}",
                                    suggest(q, &q_candidates)
                                );
                            }
                        } else if n.get("primary_q").and_then(|v| v.as_str()).is_none() {
                            // P139: the walk reads its answer from `action.question`,
                            // else the node `primary_q`. With neither, `run_action`
                            // bails `action {kind} needs 'question' or a primary_q`
                            // — but that is only after `validate` said the spec was
                            // fine. Reject it here instead: the same "validate must
                            // imply runnable" rule P39 applies to an undeclared
                            // capability. (The compiler always emits a primary_q, so
                            // this only guards hand-written / imported specs.)
                            bail!(
                                "node {name}: action {kind:?} reads an answer but names no question — add `\"question\": \"<Q>\"` to the action or `primary_q` to the node (otherwise the walk bails `action {kind} needs 'question' or a primary_q`)"
                            );
                        }
                    }
                    _ => {}
                }
                if kind == "threshold" {
                    if let Some(rules) = action.get("rules").and_then(|v| v.as_array()) {
                        for r in rules {
                            if let Some(w) = r.get("when") {
                                validate_threshold_when(
                                    w,
                                    name,
                                    &has_q,
                                    aq,
                                    &q_candidates,
                                    KNOWN_WHEN_KEYS,
                                    &q_types,
                                )?;
                            }
                        }
                    }
                }
                if kind == "threshold" {
                    // The score ladder must be total: `apply_rules_multi` emits the
                    // first rule whose `when` matches, else an unconditional
                    // catch-all, else the literal label "unmatched". A ladder that
                    // does not end in a catch-all, carries a rule with no `label`,
                    // keeps a dead rule after a catch-all, or repeats a `when`,
                    // quietly decides wrong at runtime — the same bug class as a
                    // dangling edge or a dead `copy_keys` key. Every checked-in
                    // spec already satisfies this.
                    match action.get("rules").and_then(|v| v.as_array()) {
                        None => bail!(
                            "node {name}: threshold action has no `rules` array — a score question with no rules can never emit a label"
                        ),
                        Some(rules) if rules.is_empty() => bail!(
                            "node {name}: threshold action has an empty `rules` array"
                        ),
                        Some(rules) => {
                            let mut seen: Vec<&Value> = Vec::new();
                            for (i, r) in rules.iter().enumerate() {
                                if r.get("label").and_then(|v| v.as_str()).is_none() {
                                    bail!(
                                        "node {name}: threshold rule #{i} has no `label` (it emits the literal \"unknown\" at runtime)"
                                    );
                                }
                                if thr_rule_is_catch_all(r) && i + 1 != rules.len() {
                                    bail!(
                                        "node {name}: threshold rule #{i} is a catch-all but is not last — the {} rule(s) after it are dead code",
                                        rules.len() - i - 1
                                    );
                                }
                                if let Some(w) = r.get("when") {
                                    validate_when_answers(
                                        w,
                                        name,
                                        &questions,
                                        aq.or_else(|| n.get("primary_q").and_then(|v| v.as_str())),
                                    )?;
                                    if !thr_when_is_catch_all(w) {
                                        if seen.iter().any(|s| *s == w) {
                                            bail!(
                                                "node {name}: threshold rule #{i} repeats an earlier rule's `when` — first match wins, so it can never fire"
                                            );
                                        }
                                        seen.push(w);
                                    }
                                }
                            }
                            if !rules.last().map(thr_rule_is_catch_all).unwrap_or(true) {
                                bail!(
                                    "node {name}: threshold rules are not exhaustive — the last rule is conditioned, so some scores match no rule and emit the literal \"unmatched\" label; end the ladder with an unconditional rule (no `when`, or `when: {{\"always\": true}}`)"
                                );
                            }
                        }
                    }
                }
                if kind == "gate" {
                    let q = aq
                        .or_else(|| n.get("primary_q").and_then(|v| v.as_str()))
                        .unwrap_or("?");
                    // P141: a gate reads `prob(option)` from a *choice* question's
                    // keyed option probabilities. On a score/noul the option key is
                    // absent, so `prob` is 0.0 and the verdict comes from the
                    // thresholds' polarity, not the answer — silently wrong.
                    if let Some(t) = q_types.get(q).map(String::as_str) {
                        if t == "score" || t == "noul" {
                            bail!(
                                "node {name}: gate reads question {q:?} of type {t:?} but a gate needs a `choice` question — `prob(option)` reads keyed option probabilities, so a score/noul answer always reads 0.0"
                            );
                        }
                    }
                    if let Some(opt) = action.get("option").and_then(|v| v.as_str()) {
                        let crit = questions
                            .as_ref()
                            .and_then(|m| m.get(q))
                            .and_then(|qv| qv.get("criteria"))
                            .and_then(|cv| cv.as_object());
                        if let Some(crit) = crit {
                            if !crit.contains_key(opt) {
                                let opt_candidates: Vec<&str> =
                                    crit.keys().map(|k| k.as_str()).collect();
                                bail!(
                                    "node {name}: gate option {opt:?} is not a choice of question {q:?}{}",
                                    suggest(opt, &opt_candidates)
                                );
                            }
                        }
                    }
                }
                if kind == "merge_open_probs" {
                    // P141: merging reads `probabilities`, which is only meaningful
                    // for a *choice* question (keyed option probabilities). A
                    // score/noul answer's probabilities are ordinal, not option
                    // probabilities, so merging them is silently meaningless.
                    let q = aq
                        .or_else(|| n.get("primary_q").and_then(|v| v.as_str()))
                        .unwrap_or("?");
                    if let Some(t) = q_types.get(q).map(String::as_str) {
                        if t == "score" || t == "noul" {
                            bail!(
                                "node {name}: merge_open_probs reads question {q:?} of type {t:?} but merging needs a `choice` question — a score/noul answer's probabilities are ordinal (`\"0\",\"1\",…`), not option probabilities"
                            );
                        }
                    }
                }
                if kind == "rubric" {
                    // P139: the walk needs `dimensions` (a non-empty array) and, in
                    // each, a declared `question` and a numeric `levels`; a missing
                    // one only bails at runtime (`action rubric: missing
                    // 'dimensions'` / `rubric dimension needs 'question'` /
                    // `rubric dimension {q} needs 'levels'`) after `validate` lied.
                    match action.get("dimensions").and_then(|v| v.as_array()) {
                        None => bail!(
                            "node {name}: rubric action has no `dimensions` array — the walk bails `action rubric: missing 'dimensions'`"
                        ),
                        Some(dims) if dims.is_empty() => bail!(
                            "node {name}: rubric action has an empty `dimensions` array"
                        ),
                        Some(dims) => {
                            for (i, dim) in dims.iter().enumerate() {
                                let Some(q) = dim.get("question").and_then(|v| v.as_str()) else {
                                    bail!(
                                        "node {name}: rubric dimension #{i} has no `question` — the walk bails `rubric dimension needs 'question'`"
                                    );
                                };
                                if !has_q(q)
                                    && n.get("primary_q").and_then(|v| v.as_str()) != Some(q)
                                {
                                    bail!(
                                        "node {name}: rubric dimension references unknown question {q:?}{}",
                                        suggest(q, &q_candidates)
                                    );
                                }
                                if dim.get("levels").and_then(|v| v.as_f64()).is_none() {
                                    bail!(
                                        "node {name}: rubric dimension {q:?} has no `levels` — the walk bails `rubric dimension {q:?} needs 'levels'`"
                                    );
                                }
                            }
                        }
                    }
                }
                if kind == "copy_keys" {
                    // `copy_keys` copies `action.keys` from the verdict answers of
                    // THIS node. Each key must be a question declared on the node —
                    // otherwise the copy silently no-ops at runtime (the key never
                    // appears in `v.answers`).
                    if let Some(keys) = action.get("keys").and_then(|v| v.as_array()) {
                        for k in keys.iter().filter_map(|v| v.as_str()) {
                            if !has_q(k)
                                && n.get("primary_q").and_then(|v| v.as_str()) != Some(k)
                            {
                                bail!(
                                    "node {name}: copy_keys copies {k:?}, which is not a question declared on this node — the runtime copy is a silent no-op{}",
                                    suggest(k, &q_candidates)
                                );
                            }
                        }
                    }
                }
                if kind == "call" {
                    // P39: a `call` node names a capability. An undeclared name
                    // compiles and `validate`s clean but hard-fails at runtime
                    // (`capability "X" is not declared in this spec`) — the same
                    // dangling-reference class as P24 edges / P27 copy_keys.
                    let mut cap_candidates: Vec<&str> =
                        declared_caps.iter().map(|s| s.as_str()).collect();
                    cap_candidates.sort_unstable();
                    let Some(cap) = action.get("capability").and_then(|v| v.as_str()) else {
                        // P140: `run_call_action` bails `action kind=call needs
                        // 'capability'` — a missing name is a spec that validates
                        // but can never run.
                        bail!(
                            "node {name}: `call` action has no `capability` — the walk bails `action kind=call needs 'capability'`"
                        );
                    };
                    if !declared_caps.contains(cap) {
                        bail!(
                            "node {name}: action calls undeclared capability {cap:?}{}",
                            suggest(cap, &cap_candidates)
                        );
                    }
                    if let Some(chain) = action.get("chain").and_then(|v| v.as_array()) {
                        for (i, step) in chain.iter().enumerate() {
                            let Some(step) = step.as_object() else {
                                bail!("node {name}: action chain step #{i} is not an object");
                            };
                            let Some(cap) = step.get("capability").and_then(|v| v.as_str()) else {
                                // P140: `run_call_action` bails `chain step {i} needs
                                // 'capability'`.
                                bail!(
                                    "node {name}: action chain step #{i} has no `capability` — the walk bails `chain step {i} needs 'capability'`"
                                );
                            };
                            if !declared_caps.contains(cap) {
                                bail!(
                                    "node {name}: action chain step calls undeclared capability {cap:?}{}",
                                    suggest(cap, &cap_candidates)
                                );
                            }
                        }
                    }
                }
                if kind == "ui_proposal" {
                    // P140: the walk reads the operation/field/amount answers by
                    // name (`ui_proposal: no answer {name:?}`), defaulting the
                    // names to "operation"/"field"/"amount" when unset. A node that
                    // does not declare them validates but can never run.
                    for (fld, dflt) in [("operation", "operation"), ("field", "field"), ("amount", "amount")] {
                        let q = action.get(fld).and_then(|v| v.as_str()).unwrap_or(dflt);
                        if !has_q(q) && n.get("primary_q").and_then(|v| v.as_str()) != Some(q) {
                            bail!(
                                "node {name}: ui_proposal reads question {q:?} (from `.{fld}`), which is not declared on this node — the walk bails `ui_proposal: no answer {q:?}`"
                            );
                        }
                    }
                }
                if kind == "combine" {
                    for f in ["choice", "signal"] {
                        // P139: `choice` and `signal` are read directly at runtime
                        // (`action combine: needs 'choice'` / `needs 'signal'`), so
                        // a missing one is a spec that validates but can never run.
                        let Some(q) = action.get(f).and_then(|v| v.as_str()) else {
                            bail!(
                                "node {name}: combine action has no `{f}` question — the walk bails `action combine: needs '{f}'`"
                            );
                        };
                        if !has_q(q) {
                            bail!(
                                "node {name}: combine {f:?} references unknown question {q:?}{}",
                                suggest(q, &q_candidates)
                            );
                        }
                        // P141: `choice` routes on an option-key answer (`as_str`),
                        // `signal` is read as a number (`as_f64`). Feeding each the
                        // other's question type silently routes on the wrong shape.
                        if let Some(t) = q_types.get(q).map(String::as_str) {
                            if f == "choice" && (t == "score" || t == "noul") {
                                bail!(
                                    "node {name}: combine.choice names question {q:?} of type {t:?} but the choice must be a `choice` question — combine routes on its option-key answer"
                                );
                            }
                            if f == "signal" && t == "choice" {
                                bail!(
                                    "node {name}: combine.signal names question {q:?} of type {t:?} but the signal must be a score/noul question — combine reads it as a number (`as_f64`), and a choice answer is a string that reads 0.0"
                                );
                            }
                        }
                    }
                    // `fallback` is a value of the choice question; when the choice
                    // answer equals it, routing goes to `review_label`. A fallback
                    // that is not a possible choice answer can never fire — silently
                    // dead branch.
                    if let (Some(cq), Some(fb)) = (
                        action.get("choice").and_then(|v| v.as_str()),
                        action.get("fallback").and_then(|v| v.as_str()),
                    ) {
                        let ok = match questions
                            .as_ref()
                            .and_then(|m| m.get(cq))
                            .and_then(|qv| qv.get("criteria"))
                        {
                            Some(Value::Object(m)) if !m.contains_key("from_state") => {
                                m.contains_key(fb)
                            }
                            Some(Value::Array(a)) => a.iter().any(|v| v.as_str() == Some(fb)),
                            _ => true, // from_state or absent → dynamic at runtime
                        };
                        if !ok {
                            let fb_candidates: Vec<&str> = match questions
                                .as_ref()
                                .and_then(|m| m.get(cq))
                                .and_then(|qv| qv.get("criteria"))
                            {
                                Some(Value::Object(m)) => m.keys().map(|k| k.as_str()).collect(),
                                Some(Value::Array(a)) => {
                                    a.iter().filter_map(|v| v.as_str()).collect()
                                }
                                _ => Vec::new(),
                            };
                            bail!(
                                "node {name}: combine fallback {fb:?} is not a possible answer of choice question {cq:?} (the fallback branch can never fire){}",
                                suggest(fb, &fb_candidates)
                            );
                        }
                    }
                }
            }
        }
    }
    Ok(())
}

/// Inline every `{"workflow": …}` reference node into a plain-node spec — the
/// node list the engine actually *walks*. The engine inlines the same way
/// (namespaced `"{ref}::"` node names; edges and `start` redirected; a
/// reference node's `edge.default` becomes the sub-graph's completion
/// handoff), so this JSON view is the surface `bdd_list` reports and the
/// contract `bdd_decide` names for an inlined node. It binds no capability, so
/// it is for readers (`bdd_list`, the step-wise `ask`), never for `from_spec`.
/// Returns a spec of the same shape when there is no reference node.
pub fn expand_spec_nodes(spec: &Value, base_dir: Option<&std::path::Path>) -> Result<Value> {
    let mut ref_stack: Vec<String> = Vec::new();
    expand_spec_nodes_cycle(spec, base_dir, &mut ref_stack)
}

fn expand_spec_nodes_cycle(
    spec: &Value,
    base_dir: Option<&std::path::Path>,
    ref_stack: &mut Vec<String>,
) -> Result<Value> {
    check_version(spec)?;
    let start = spec
        .get("start")
        .and_then(|v| v.as_str())
        .ok_or_else(|| anyhow!("spec missing 'start'"))?
        .to_string();
    let node_list = spec
        .get("nodes")
        .and_then(|v| v.as_array())
        .ok_or_else(|| anyhow!("spec missing 'nodes' array"))?;

    let mut out_nodes: Vec<Value> = Vec::with_capacity(node_list.len());
    // (sub_name, prefixed_start) pairs, so nodes emitted *after* the reference
    // are redirected too (order-independent) — mirrors the engine.
    let mut ref_redirects: Vec<(String, String)> = Vec::new();
    for n in node_list {
        let Some(wref) = n.get("workflow") else {
            out_nodes.push(n.clone());
            continue;
        };
        let (sub_spec, sub_dir) = resolve_ref_dir(wref, base_dir)?;
        // Cycle guard: a workflow must not reference itself transitively.
        let ident = sub_spec
            .get("name")
            .and_then(|v| v.as_str())
            .map(str::to_string)
            .or_else(|| match wref {
                Value::String(s) => Some(s.clone()),
                _ => None,
            });
        if let Some(id) = &ident {
            if ref_stack.iter().any(|s| s == id) {
                let mut chain = ref_stack.clone();
                chain.push(id.clone());
                bail!("workflow reference cycle detected: {}", chain.join(" -> "));
            }
            ref_stack.push(id.clone());
        }
        let sub_name = n
            .get("name")
            .and_then(|v| v.as_str())
            .or_else(|| sub_spec.get("name").and_then(|v| v.as_str()))
            .unwrap_or("sub")
            .to_string();
        let prefix = format!("{sub_name}::");
        // Expand the sub-graph's own nodes first (recursively), so a nested
        // reference inside the sub is already flattened when we re-prefix.
        let sub = expand_spec_nodes_cycle(&sub_spec, sub_dir.as_deref(), ref_stack)?;
        if ident.is_some() {
            ref_stack.pop();
        }
        let continue_to = n
            .get("edge")
            .and_then(|e| e.get("default"))
            .and_then(|v| v.as_str())
            .map(str::to_string);
        let sub_start = format!(
            "{prefix}{}",
            sub.get("start").and_then(|v| v.as_str()).unwrap_or("start")
        );
        ref_redirects.push((sub_name.clone(), sub_start.clone()));
        // Nodes emitted before the reference still point at the bare sub-name.
        for existing in out_nodes.iter_mut() {
            redirect_node_dests(existing, &sub_name, &sub_start);
        }
        let sub_nodes = sub
            .get("nodes")
            .and_then(|v| v.as_array())
            .cloned()
            .unwrap_or_default();
        // Names of the (already-expanded) sub nodes: only a dest that *is* one of
        // them is re-prefixed; a `STOP` becomes the completion handoff.
        let sub_names: std::collections::HashSet<String> = sub_nodes
            .iter()
            .filter_map(|x| x.get("name").and_then(|v| v.as_str()).map(str::to_string))
            .collect();
        for sn in &sub_nodes {
            let mut nn = sn.clone();
            let sn_name = nn
                .get("name")
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_string();
            if let Some(o) = nn.as_object_mut() {
                o.insert("name".to_string(), json!(format!("{prefix}{sn_name}")));
            }
            if let Some(e) = nn.get_mut("edge").and_then(|e| e.as_object_mut()) {
                if let Some(cond) = e.get_mut("condition").and_then(|c| c.as_object_mut()) {
                    for dest in cond.values_mut() {
                        let Some(d) = dest.as_str() else { continue };
                        if d == "STOP" {
                            if let Some(ct) = &continue_to {
                                *dest = json!(ct);
                            }
                        } else if sub_names.contains(d) {
                            *dest = json!(format!("{prefix}{d}"));
                        }
                    }
                }
                if let Some(d) = e.get("default").and_then(|v| v.as_str()).map(str::to_string) {
                    if d == "STOP" {
                        if let Some(ct) = &continue_to {
                            e.insert("default".to_string(), json!(ct));
                        }
                    } else if sub_names.contains(&d) {
                        e.insert("default".to_string(), json!(format!("{prefix}{d}")));
                    }
                }
            }
            out_nodes.push(nn);
        }
    }
    // Order-independent reference redirect for nodes emitted after a reference.
    for existing in out_nodes.iter_mut() {
        for (sub_name, sub_start) in &ref_redirects {
            redirect_node_dests(existing, sub_name, sub_start);
        }
    }
    // If `start` names a reference node, redirect it to that sub-workflow's own
    // start (also namespaced).
    let mut new_start = start;
    let names: std::collections::HashSet<String> = out_nodes
        .iter()
        .filter_map(|n| n.get("name").and_then(|v| v.as_str()).map(str::to_string))
        .collect();
    if !names.contains(&new_start) {
        if let Some(rest) = node_list.iter().find(|n| {
            n.get("name").and_then(|v| v.as_str()) == Some(new_start.as_str())
                || n.get("workflow").and_then(|v| v.as_str()) == Some(new_start.as_str())
        }) {
            if let Some(wref) = rest.get("workflow") {
                let (sub_spec, _) = resolve_ref_dir(wref, base_dir)?;
                let sub_name = rest
                    .get("name")
                    .and_then(|v| v.as_str())
                    .or_else(|| sub_spec.get("name").and_then(|v| v.as_str()))
                    .unwrap_or("sub");
                let sub_start = sub_spec
                    .get("start")
                    .and_then(|v| v.as_str())
                    .unwrap_or("start");
                new_start = format!("{sub_name}::{sub_start}");
            }
        }
    }
    let mut out = spec.clone();
    if let Some(o) = out.as_object_mut() {
        o.insert("nodes".to_string(), Value::Array(out_nodes));
        o.insert("start".to_string(), json!(new_start));
    }
    Ok(out)
}

/// Redirect a node's edge destinations that name the bare reference `from` to
/// the sub-graph's prefixed `to` — condition values and the default, leaving
/// `STOP` and any non-sub destination alone.
fn redirect_node_dests(node: &mut Value, from: &str, to: &str) {
    let Some(e) = node.get_mut("edge").and_then(|e| e.as_object_mut()) else {
        return;
    };
    if let Some(cond) = e.get_mut("condition").and_then(|c| c.as_object_mut()) {
        for dest in cond.values_mut() {
            if dest.as_str() == Some(from) {
                *dest = json!(to);
            }
        }
    }
    if e.get("default").and_then(|v| v.as_str()) == Some(from) {
        e.insert("default".to_string(), json!(to));
    }
}
pub fn from_spec_with_dir(
    spec: &Value,
    parent: Option<&Value>,
    base_dir: Option<&std::path::Path>,
) -> Result<ResilientWorkflow> {
    // Cycle guard: nested `{"workflow": …}` references must not expand into
    // themselves (directly or transitively) — that would recurse forever.
    // Identity is the referenced spec's own name (or its ref string), so a
    // diamond (two branches referencing the same leaf) is fine while a cycle
    // (A → B → A) is rejected up front instead of overflowing the stack.
    let mut ref_stack: Vec<String> = Vec::new();
    from_spec_with_dir_cycle(spec, parent, base_dir, &mut ref_stack)
}

fn from_spec_with_dir_cycle(
    spec: &Value,
    _parent: Option<&Value>,
    base_dir: Option<&std::path::Path>,
    ref_stack: &mut Vec<String>,
) -> Result<ResilientWorkflow> {
    // P148: every top-level key (and each `policy` / `_bdd` key) must be one the
    // engine actually reads; a misspelling silently drops a declaration.
    validate_spec_keys(spec)?;
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
    // (sub_name, prefixed_start) pairs collected while expanding reference
    // nodes, so edges that appear *after* the reference are redirected too.
    let mut ref_redirects: Vec<(String, String)> = Vec::new();
    for n in node_list {
        // A node may be a `{"workflow": …}` *reference node*: it inlines the
        // referenced workflow's nodes into this graph (namespaced) so nesting
        // needs no special runner support.
        if let Some(wref) = n.get("workflow") {
            let (sub_spec, sub_dir) = resolve_ref_dir(wref, base_dir)?;
            // Cycle guard: a workflow must not reference itself transitively.
            // Identity is the referenced spec's own name (or the ref string),
            // so a diamond (two branches → same leaf) is fine while a cycle
            // (A → B → A) is rejected up front instead of overflowing the stack.
            let ident = sub_spec
                .get("name")
                .and_then(|v| v.as_str())
                .map(str::to_string)
                .or_else(|| match wref {
                    Value::String(s) => Some(s.clone()),
                    _ => None,
                });
            if let Some(id) = &ident {
                if ref_stack.iter().any(|s| s == id) {
                    let mut chain = ref_stack.clone();
                    chain.push(id.clone());
                    bail!(
                        "workflow reference cycle detected: {} (a workflow must not reference itself, directly or transitively)",
                        chain.join(" -> ")
                    );
                }
                ref_stack.push(id.clone());
            }
            let sub_name = n
                .get("name")
                .and_then(|v| v.as_str())
                .or_else(|| sub_spec.get("name").and_then(|v| v.as_str()))
                .unwrap_or("sub");
            let prefix = format!("{sub_name}::");
            let sub = from_spec_with_dir_cycle(&sub_spec, Some(spec), sub_dir.as_deref(), ref_stack)?;
            if ident.is_some() {
                ref_stack.pop();
            }
            // A reference node may declare a *completion handoff*: when the sub
            // workflow terminates (an internal edge hits STOP), routing continues
            // at the caller's continuation node instead. This is what lets a
            // nested pipeline run sub-graph THEN a local downstream step.
            let continue_to = n
                .get("edge")
                .and_then(|e| e.get("default"))
                .and_then(|v| v.as_str())
                .map(str::to_string);
            // `sub.start` is the sub's own start (unprefixed); the expanded nodes
            // above get `prefix`, so inbound edges must point at the prefixed name.
            let sub_start = format!("{prefix}{}", sub.start);
            ref_redirects.push((sub_name.to_string(), sub_start.clone()));
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
                    } else if dest == "STOP" {
                        if let Some(ct) = &continue_to {
                            *dest = ct.clone();
                        }
                    }
                }
                if let Some(d) = nn.edge.default.clone() {
                    if d != "STOP" && sub.nodes.contains_key(&d) {
                        nn.edge.default = Some(format!("{prefix}{d}"));
                    } else if d == "STOP" {
                        if let Some(ct) = &continue_to {
                            nn.edge.default = Some(ct.clone());
                        }
                    }
                }
                nodes.push(nn);
            }
            continue;
        }
        nodes.push(node_from_spec_with(n, Some(registry.clone()))?);
    }

    // Order-independent reference redirect: a node that appears *after* the
    // reference in the spec still points at the bare sub-name, which only
    // exists as the prefixed expansion.
    for existing in nodes.iter_mut() {
        for (sub_name, sub_start) in &ref_redirects {
            for dest in existing.edge.condition.values_mut() {
                if dest == sub_name {
                    *dest = sub_start.clone();
                }
            }
            if existing.edge.default.as_deref() == Some(sub_name.as_str()) {
                existing.edge.default = Some(sub_start.clone());
            }
        }
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
                let sub_start = sub_spec
                    .get("start")
                    .and_then(|v| v.as_str())
                    .unwrap_or("start");
                start = format!("{sub_name}::{sub_start}");
            }
        }
    }

    let declared_caps: std::collections::HashSet<String> = spec
        .get("capabilities")
        .and_then(|v| v.as_object())
        .map(|m| m.keys().cloned().collect())
        .unwrap_or_default();
    // `max_iterations: 0` (or negative) runs the graph zero times — a spec that
    // can never execute — and a non-integer silently falls back to the default
    // 50. Reject the present-but-invalid case instead.
    if let Some(v) = spec.get("max_iterations") {
        if v.as_u64().map(|n| n >= 1) != Some(true) {
            bail!(
                "spec max_iterations must be a positive integer (got {v}) — 0 runs the workflow zero times"
            );
        }
    }
    validate_graph_semantics(&nodes, node_list, &start, &declared_caps)?;
    // P177/P178: hold a hand-written / side-loaded JSON spec to the same `call`
    // contract the BDD compile path enforces - the `with` half (P164 key /
    // P166 type / P175 range) and the `project` half (P165 pointer / empty key /
    // empty chain alias).
    let cap_defs: Map<String, Value> = spec
        .get("capabilities")
        .and_then(|v| v.as_object())
        .cloned()
        .unwrap_or_default();
    guard_call_spec(&cap_defs, node_list)?;
    // P179: a node/spec value of the wrong type is silently replaced by the engine
    // default (see `guard_spec_value_types`) — reject it instead.
    guard_spec_value_types(spec, node_list)?;
    // P180: a value in the wrong *range* is the twin of the wrong *type* above.
    guard_spec_value_ranges(spec)?;

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
    let refs: Vec<(&str, &str)> = condition
        .iter()
        .map(|(a, b)| (a.as_str(), b.as_str()))
        .collect();
    let edge = Edge::new(&refs, default, min_confidence);

    let mut node = WorkflowNode::new(name, questions.clone(), edge);
    if let Some(pq) = n.get("primary_q").and_then(|v| v.as_str()) {
        node = node.with_primary(pq);
    }
    if let Some(r) = n.get("max_retries").and_then(|v| v.as_u64()) {
        node = node.with_max_retries(r as usize);
    }
    // P144: a node's `state` projection must retain the keys the node *reads*.
    // The backend answers a question from `heuristic.answer_field` — a state key —
    // so that key must survive the projection even when the author did not list it
    // under `keep`. Pre-P144 the runtime fell back to the whole state whenever the
    // projection matched nothing, which hid the gap (and leaked every other key).
    // Fold the read keys into `keep` here so the projection is complete and never
    // has to fall back. (`criteria.from_state` is expanded from the *raw* state by
    // the engine before the backend sees the projected state, so it is not folded.)
    let read_keys = answer_field_keys(n.get("questions"));
    if let Some(state_v) = n.get("state") {
        if state_v.is_null() {
            node = node.with_state_fn(move |s: &Value| project_state(s, &Value::Null));
        } else {
            let mut state = state_v.clone();
            if !read_keys.is_empty() {
                if let Some(o) = state.as_object_mut() {
                    let keep = o.entry("keep").or_insert_with(|| json!([]));
                    if let Some(arr) = keep.as_array_mut() {
                        for k in &read_keys {
                            if !arr.iter().any(|v| v.as_str() == Some(k.as_str())) {
                                arr.push(json!(k));
                            }
                        }
                    }
                }
            }
            node = node.with_state_fn(move |s: &Value| project_state(s, &state));
        }
    } else if !read_keys.is_empty() {
        // The node reads state keys but declared no projection: retain exactly
        // those keys — never the whole state.
        let mut st = Map::new();
        st.insert("keep".into(), json!(read_keys));
        let state = Value::Object(st);
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

/// The state keys a node's questions read directly: every
/// `heuristic.answer_field`. Deduplicated, in question order (serde_json keys
/// are sorted, so the order is deterministic). `project_state` folds these into
/// the projection so a node always sees what it reads (P144).
fn answer_field_keys(questions: Option<&Value>) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    if let Some(qs) = questions.and_then(Value::as_object) {
        for q in qs.values() {
            if let Some(f) = q.pointer("/heuristic/answer_field").and_then(Value::as_str) {
                if !out.iter().any(|k| k == f) {
                    out.push(f.to_string());
                }
            }
        }
    }
    out
}

/// `state` projection: `{"keep": [...], "rename": {out: in}}` — a data-level
/// `state_fn` so a spec can shape what the backend sees.
///
/// P144 — a projection that names keys but matches none must project *nothing*,
/// not the whole state. The pre-P144 fallback was `if out.is_empty() { state.clone() }`,
/// so a `keep: ["missing_key"]` (or a `rename` whose source is absent) silently
/// sent every *other* key — secrets, unrelated blobs — to the backend. The empty
/// case is now split: a spec that named ≥1 key but found none yields `{}` (fail
/// closed); a spec that named no keys at all (an absent/null `state`, or the
/// compiler's `{"keep": []}` "keeps nothing in particular" encoding) stays identity.
fn project_state(state: &Value, spec: &Value) -> Value {
    if spec.is_null() {
        return state.clone();
    }
    let mut out = Map::new();
    // Did the spec *name* any state key? An empty declaration (`keep: []`,
    // `rename: {}`) is the compiler's encoding of "no projection to apply", so
    // it must not be conflated with a declaration that named keys the state
    // does not carry.
    let mut named = false;
    if let Some(keep) = spec.get("keep").and_then(|v| v.as_array()) {
        named |= !keep.is_empty();
        for k in keep.iter().filter_map(|v| v.as_str()) {
            if let Some(v) = state.get(k) {
                out.insert(k.to_string(), v.clone());
            }
        }
    }
    if let Some(renames) = spec.get("rename").and_then(|v| v.as_object()) {
        named |= !renames.is_empty();
        for (to, from) in renames {
            if let Some(src) = from.as_str() {
                if let Some(v) = state.get(src) {
                    out.insert(to.clone(), v.clone());
                }
            }
        }
    }
    if !out.is_empty() {
        Value::Object(out)
    } else if named {
        // Named keys, none present: project nothing (never the whole state).
        Value::Object(Map::new())
    } else {
        // No keys named at all: identity.
        state.clone()
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
            // P147: a chain step may project its own result (the same pointer /
            // `body` fallback as the main `call`), so `with.<as>` carries the
            // chosen keys instead of the whole envelope.
            let res = match step.get("project").and_then(|p| p.as_object()) {
                Some(proj) if !proj.is_empty() => Value::Object(project_result(&res, proj)),
                _ => res,
            };
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
                let segs: Vec<&str> = ptr
                    .trim_start_matches('/')
                    .split('/')
                    .filter(|s| !s.is_empty())
                    .collect();
                let mut found: Option<Value> = None;
                for root in &roots {
                    if let Some(v) = json_pointer(root, &segs)
                        .or_else(|| root.get("body").and_then(|b| json_pointer(b, &segs)))
                    {
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

/// Project named keys out of a capability result: each `out_key` reads the JSON
/// pointer `ptr` against `root`, falling back to `root.body` (http-style
/// envelopes). The single-root half of a `call`'s `project`, reused for each
/// `chain` step so its declared `project` is honoured (P147).
fn project_result(root: &Value, project: &Map<String, Value>) -> Map<String, Value> {
    let mut out = Map::new();
    for (key, ptr) in project {
        let Some(ptr) = ptr.as_str() else { continue };
        let segs: Vec<&str> = ptr
            .trim_start_matches('/')
            .split('/')
            .filter(|s| !s.is_empty())
            .collect();
        if let Some(v) = json_pointer(root, &segs)
            .or_else(|| root.get("body").and_then(|b| json_pointer(b, &segs)))
        {
            out.insert(key.clone(), v.clone());
        }
    }
    out
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
    state: &Value,
    v: &Verdict,
    primary: Option<&str>,
) -> Result<Value> {
    let kind = action
        .get("kind")
        .and_then(|k| k.as_str())
        .ok_or_else(|| anyhow!("action missing 'kind'"))?;
    let q = action.get("question").and_then(|q| q.as_str()).or(primary);
    let q = match q {
        Some(q) => q,
        None => {
            // only actions that read a specific answer need a question
            if kind == "copy_keys" || kind == "none" || kind == "rubric" || kind == "combine" || kind == "ui_proposal" {
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
                .map(|a| {
                    a.iter()
                        .filter_map(|v| v.as_str().map(str::to_string))
                        .collect()
                })
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
        "rubric" => {
            // Composite scoring (awesome-jev quality-rubric): several ordered
            // score dimensions, each normalised to 0..1 by dividing by
            // `levels - 1` (a Score is a probability-weighted position in its
            // rubric — NOT a probability itself), then combined with per-dimension
            // weights, all in code. If any dimension's confidence is below the
            // floor, the composite is withheld (`status: human_review`) instead
            // of reporting a number built on uncertain inputs.
            let dims = action
                .get("dimensions")
                .and_then(|d| d.as_array())
                .ok_or_else(|| anyhow!("action rubric: missing 'dimensions'"))?;
            let conf_floor = action
                .get("min_confidence")
                .and_then(|x| x.as_f64())
                .unwrap_or(0.8);
            let mut normalized = Map::new();
            let mut uncertain = Vec::new();
            let mut score_sum = 0.0f64;
            let mut weight_sum = 0.0f64;
            let mut low_conf = false;
            for dim in dims.iter().filter_map(|d| d.as_object()) {
                let qname = dim
                    .get("question")
                    .and_then(|x| x.as_str())
                    .ok_or_else(|| anyhow!("rubric dimension needs 'question'"))?;
                let weight = dim.get("weight").and_then(|x| x.as_f64()).unwrap_or(1.0);
                let levels = dim
                    .get("levels")
                    .and_then(|x| x.as_u64())
                    .ok_or_else(|| anyhow!("rubric dimension {qname:?} needs 'levels'"))?
                    .max(2) as f64;
                let d = v
                    .answers
                    .get(qname)
                    .ok_or_else(|| anyhow!("rubric: no answer {qname:?}"))?;
                if d.confidence < conf_floor {
                    low_conf = true;
                    uncertain.push(qname.to_string());
                }
                let raw = d.as_f64();
                let norm = (raw / (levels - 1.0)).clamp(0.0, 1.0);
                normalized.insert(qname.to_string(), json!({ "raw": raw, "normalized": norm }));
                score_sum += norm * weight;
                weight_sum += weight;
            }
            let composite = if low_conf || weight_sum <= 0.0 {
                None
            } else {
                Some(score_sum / weight_sum)
            };
            Ok(json!({
                "status": if low_conf { "human_review" } else { "scored" },
                "normalized": normalized,
                "weighted_score": composite,
                "uncertain_dimensions": uncertain,
                "min_confidence": conf_floor,
            }))
        }
        "combine" => {
            // Route + signal composition (awesome-jev support-routing):
            // `choice` names a choice answer and `signal` a noul answer; code
            // combines them. The choice routes unless its confidence is below
            // `min_confidence` (default 0.8) or it picked `fallback`
            // (default "other") — then it routes to `review_label` (default
            // human_review). The noul is bucketed by dual thresholds:
            // >= high_threshold -> high, <= low_threshold -> ordinary,
            // otherwise -> review. Neither question depends on the other's
            // result; all combination lives here, in code.
            let cq = action
                .get("choice")
                .and_then(|x| x.as_str())
                .ok_or_else(|| anyhow!("action combine: needs 'choice'"))?;
            let sq = action
                .get("signal")
                .and_then(|x| x.as_str())
                .ok_or_else(|| anyhow!("action combine: needs 'signal'"))?;
            let c = v
                .answers
                .get(cq)
                .ok_or_else(|| anyhow!("action combine: no answer {cq:?}"))?;
            let s = v
                .answers
                .get(sq)
                .ok_or_else(|| anyhow!("action combine: no answer {sq:?}"))?;
            let floor = action.get("min_confidence").and_then(|x| x.as_f64()).unwrap_or(0.8);
            let fallback = action.get("fallback").and_then(|x| x.as_str()).unwrap_or("other");
            let review_label = action.get("review_label").and_then(|x| x.as_str()).unwrap_or("human_review");
            let hi = action.get("high_threshold").and_then(|x| x.as_f64()).unwrap_or(0.85);
            let lo = action.get("low_threshold").and_then(|x| x.as_f64()).unwrap_or(0.15);
            let choice = c.as_str();
            let needs_review = c.confidence < floor || choice == fallback;
            let route = if needs_review { review_label.to_string() } else { choice.clone() };
            let p = s.as_f64();
            let urgency = if p >= hi {
                "high".to_string()
            } else if p <= lo {
                "ordinary".to_string()
            } else {
                "review".to_string()
            };
            Ok(json!({
                "route": route,
                "urgency": urgency,
                "reason": if needs_review {
                    format!("{cq}: confidence {:.3} below {floor} or picked {fallback:?}", c.confidence)
                } else {
                    format!("{cq} clears the confidence floor ({floor})")
                },
                "choice_confidence": c.confidence,
                "signal_value": p,
                "combined": true,
            }))
        }
        "ui_proposal" => {
            // Prepare stage (awesome-jev computer-use `prepare()`): turn
            // validated answers into a bounded proposal. Code — not the model —
            // decides what constitutes an action. `operation` picks
            // fill/done/wait/blocked under a confidence floor; `field` and
            // `amount` are independent answers. The observation fingerprint and
            // surface come from state (never authored by the model), and the
            // extracted value is the *source text* looked up by id, not the id
            // itself. Anything below the floor yields `human_review`; wait /
            // blocked stop the cycle. The proposal nests under "proposal" so a
            // downstream executor node reads `${state.proposal}`.
            let op_q = action
                .get("operation")
                .and_then(|x| x.as_str())
                .unwrap_or("operation");
            let fld_q = action.get("field").and_then(|x| x.as_str()).unwrap_or("field");
            let amt_q = action.get("amount").and_then(|x| x.as_str()).unwrap_or("amount");
            let floor = action
                .get("min_confidence")
                .and_then(|x| x.as_f64())
                .unwrap_or(0.8);
            // P147: `surface` / `snapshot_hash` name the STATE keys the
            // observation was fingerprinted under (like `operation` / `field` /
            // `amount` name question keys). The defaults keep every existing spec
            // byte-identical; a declared name is now honoured instead of ignored.
            let surface_key = action
                .get("surface")
                .and_then(|x| x.as_str())
                .unwrap_or("surface");
            let surface = state
                .get(surface_key)
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_string();
            let snapshot_hash_key = action
                .get("snapshot_hash")
                .and_then(|x| x.as_str())
                .unwrap_or("snapshot_hash");
            let snapshot_hash = state
                .get(snapshot_hash_key)
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_string();
            let prepare = |status: &str, reason: &str, action_out: Value, extracted: Value| {
                json!({
                    "prepare_status": status,
                    "proposal": {
                        "status": status,
                        "reason": reason,
                        "snapshot_hash": snapshot_hash,
                        "surface": surface,
                        "action": action_out,
                        "extracted": extracted,
                    },
                })
            };
            let need = |name: &str| -> Result<&crate::workflow::Decision> {
                v.answers
                    .get(name)
                    .ok_or_else(|| anyhow!("ui_proposal: no answer {name:?}"))
            };
            let op = need(op_q)?;
            if op.confidence < floor {
                return Ok(prepare("human_review", "Uncertain operation", Value::Null, Value::Null));
            }
            let choice = op.as_str();
            let choice: &str = choice.as_str();
            if choice == "wait" || choice == "blocked" {
                return Ok(prepare(
                    choice,
                    "A fresh observation or intervention is needed",
                    Value::Null,
                    Value::Null,
                ));
            }
            let amt = need(amt_q)?;
            let amt_choice = amt.as_str();
            if amt.confidence < floor || amt_choice == "none" {
                return Ok(prepare(
                    "human_review",
                    "Amount due is missing or uncertain",
                    Value::Null,
                    Value::Null,
                ));
            }
            // Look up the extracted source text by id (the model names the
            // source; code reads the value). Absent id → review.
            let extracted_text = state
                .get("snapshot")
                .and_then(|s| s.get("texts"))
                .and_then(|t| t.as_array())
                .and_then(|arr| {
                    arr.iter()
                        .find(|e| e.get("id").and_then(|v| v.as_str()) == Some(amt_choice.as_str()))
                        .and_then(|e| e.get("text"))
                })
                .cloned();
            let Some(extracted_text) = extracted_text else {
                return Ok(prepare(
                    "human_review",
                    "Extracted source id is absent from the observation",
                    Value::Null,
                    Value::Null,
                ));
            };
            let extracted = json!({
                "source_id": amt_choice,
                "value": extracted_text,
                "surface": surface,
            });
            if choice == "done" {
                return Ok(prepare("ready", "", Value::Null, extracted));
            }
            // choice == "fill"
            let fld = need(fld_q)?;
            let fld_choice = fld.as_str();
            if fld.confidence < floor || fld_choice == "none" {
                return Ok(prepare(
                    "human_review",
                    "Field is missing or uncertain",
                    Value::Null,
                    Value::Null,
                ));
            }
            let fill = json!({
                "kind": "fill",
                "target_id": fld_choice,
                "value_key": action
                    .get("value_key")
                    .and_then(|x| x.as_str())
                    .unwrap_or("billing_contact"),
            });
            Ok(prepare("ready", "", fill, extracted))
        }
        "gate" => {
            // Gate semantics (fail-closed): `option` is the *unsafe* answer
            // (default "B"). High p(option) → BLOCK; high p(not option) → ALLOW;
            // the middle band → CONFIRM.
            let d = v
                .answers
                .get(q)
                .ok_or_else(|| anyhow!("action gate: no answer {q:?}"))?;
            let block_at = action
                .get("block_at")
                .and_then(|x| x.as_f64())
                .unwrap_or(0.85);
            let allow_at = action
                .get("allow_at")
                .and_then(|x| x.as_f64())
                .unwrap_or(0.85);
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
        let label = r
            .get("label")
            .and_then(|l| l.as_str())
            .unwrap_or("unknown")
            .to_string();
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
        let label = r
            .get("label")
            .and_then(|l| l.as_str())
            .unwrap_or("unknown")
            .to_string();
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
    if when
        .get("always")
        .and_then(|v| v.as_bool())
        .unwrap_or(false)
    {
        return true;
    }
    // A conjunction: every sub-condition must hold. Each sub-condition is a
    // leaf `when` (no nested `question` here — `rule_matches_multi` resolves
    // the question scope and then delegates the leaves to this function).
    if let Some(all) = when.get("all").and_then(|v| v.as_array()) {
        return all.iter().all(|w| rule_matches(w, d));
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
    if when
        .get("always")
        .and_then(|x| x.as_bool())
        .unwrap_or(false)
    {
        return true;
    }
    // A conjunction: resolve each conjunct against its own question scope.
    if let Some(all) = when.get("all").and_then(|x| x.as_array()) {
        return all.iter().all(|w| rule_matches_multi(w, v, default_d));
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

/// Node names in a spec that carry a **native** (non-serializable) action.
/// `"kind": "rust"` marks a node whose behaviour lived in a Rust closure when it
/// was exported; such a spec is a *reference* graph, because `from_spec` rejects
/// it — `validate` / `run` cannot load it. Every built-in app (`agent_gate`,
/// `email_triage`, `content_moderation`, `draft_scorer`) routes in a Rust
/// closure, so this is non-empty for all of them. Exposed so `export` can say so
/// out loud instead of emitting a spec that silently fails to reload.
pub fn native_action_nodes(spec: &Value) -> Vec<String> {
    spec.get("nodes")
        .and_then(Value::as_array)
        .map(|ns| {
            ns.iter()
                .filter(|n| n.pointer("/action/kind").and_then(Value::as_str) == Some("rust"))
                .filter_map(|n| n.get("name").and_then(Value::as_str).map(str::to_string))
                .collect()
        })
        .unwrap_or_default()
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
                let stem = p
                    .file_stem()
                    .and_then(|x| x.to_str())
                    .unwrap_or("")
                    .to_string();
                out.push((stem, p));
            }
        }
    }
    out.sort_by(|a, b| a.1.cmp(&b.1));
    Ok(out)
}
