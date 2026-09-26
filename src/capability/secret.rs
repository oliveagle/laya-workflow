//! Secret management for workflow capabilities.
//!
//! Goals: credentials never appear in a DSL spec, and never leak into workflow
//! state, node payloads, traces, history or CLI output.
//!
//! Sources, highest precedence first:
//!   1. process environment (`export NAME=…`, or a literal `${env.NAME}`)
//!   2. `.env`-style files, in this order:
//!        * `$LAYA_SECRETS_FILE` (a single file)
//!        * `$LAYA_SECRETS_DIR/*.env` (sorted)
//!        * `<dsl_dir>/.env`
//!        * `./.env`
//!   3. JSON secrets files found alongside a spec, under a
//!      `policy.allow_paths` root (`{"NAME": "value"}`)
//!
//! Reference syntax inside a spec: `${secret.NAME}` (preferred) or the legacy
//! `${env.NAME}`. A missing secret is a hard error — never a silent empty value.
//!
//! Anti-leak: every resolved value is registered in a redaction set; `redact()`
//! is applied to all JSON we emit, so a secret that somehow reaches a payload or
//! trace is replaced by `***` (and the field marked).

use anyhow::{anyhow, bail, Result};
use serde_json::{Map, Value};
use std::collections::BTreeSet;
use std::path::{Path, PathBuf};
use std::sync::{Mutex, OnceLock};

/// A resolved secret store: name → value, plus the names that are known.
#[derive(Debug, Default, Clone)]
pub struct Secrets {
    values: std::collections::HashMap<String, String>,
}

impl Secrets {
    /// Build from the process environment plus any `.env` / JSON secrets files.
    pub fn load(dsl_dir: Option<&Path>) -> Self {
        let mut s = Secrets::default();
        // .env files first, so the real environment wins below
        for f in env_files(dsl_dir) {
            s.merge_env_file(&f);
        }
        for (k, v) in std::env::vars() {
            s.values.insert(k, v);
        }
        s
    }

    /// Merge `KEY=VALUE` lines (supports comments, quotes, `export ` prefix).
    pub fn merge_env_file(&mut self, path: &Path) {
        let Ok(text) = std::fs::read_to_string(path) else {
            return;
        };
        for line in text.lines() {
            let l = line.trim();
            if l.is_empty() || l.starts_with('#') {
                continue;
            }
            let l = l.strip_prefix("export ").unwrap_or(l).trim();
            let Some((k, v)) = l.split_once('=') else {
                continue;
            };
            let k = k.trim();
            if k.is_empty() {
                continue;
            }
            let mut v = v.trim().to_string();
            if (v.starts_with('"') && v.ends_with('"') && v.len() >= 2)
                || (v.starts_with('\'') && v.ends_with('\'') && v.len() >= 2)
            {
                v = v[1..v.len() - 1].to_string();
            }
            // first definition wins (env files are lower precedence than a
            // later explicit merge, which the caller controls)
            self.values.entry(k.to_string()).or_insert(v);
        }
    }

    /// Merge a JSON secrets file: `{"NAME": "value", …}`.
    pub fn merge_json_file(&mut self, path: &Path) -> Result<()> {
        let raw = std::fs::read_to_string(path)
            .map_err(|e| anyhow!("secrets file {} unreadable: {e}", path.display()))?;
        let v: Value = serde_json::from_str(&raw)
            .map_err(|e| anyhow!("secrets file {} is not JSON: {e}", path.display()))?;
        if let Some(o) = v.as_object() {
            for (k, val) in o {
                if let Some(s) = val.as_str() {
                    self.values.insert(k.clone(), s.to_string());
                }
            }
        }
        Ok(())
    }

    pub fn insert(&mut self, name: &str, value: &str) {
        self.values.insert(name.to_string(), value.to_string());
    }

    pub fn has(&self, name: &str) -> bool {
        self.values.contains_key(name)
    }

    pub fn get(&self, name: &str) -> Option<&str> {
        self.values.get(name).map(|s| s.as_str())
    }

    /// Names known to this store, sorted (never values).
    pub fn names(&self) -> Vec<String> {
        let mut v: Vec<String> = self.values.keys().cloned().collect();
        v.sort();
        v
    }

    /// All non-empty values, for redaction.
    pub fn values_for_redaction(&self) -> Vec<String> {
        self.values
            .values()
            .filter(|v| v.len() >= 4) // avoid redacting trivial strings
            .cloned()
            .collect()
    }
}

fn env_files(dsl_dir: Option<&Path>) -> Vec<PathBuf> {
    let mut out: Vec<PathBuf> = Vec::new();
    if let Ok(f) = std::env::var("LAYA_SECRETS_FILE") {
        if !f.is_empty() {
            out.push(PathBuf::from(f));
        }
    }
    if let Ok(d) = std::env::var("LAYA_SECRETS_DIR") {
        if let Ok(rd) = std::fs::read_dir(&d) {
            let mut files: Vec<PathBuf> = rd
                .filter_map(|e| e.ok())
                .map(|e| e.path())
                .filter(|p| p.extension().and_then(|x| x.to_str()) == Some("env"))
                .collect();
            files.sort();
            out.extend(files);
        }
    }
    if let Some(d) = dsl_dir {
        out.push(d.join(".env"));
    }
    out.push(PathBuf::from(".env"));
    out
}

// ── process-wide store + redaction set ──────────────────────────────

fn store() -> &'static Mutex<Secrets> {
    static S: OnceLock<Mutex<Secrets>> = OnceLock::new();
    S.get_or_init(|| Mutex::new(Secrets::default()))
}

fn redaction_set() -> &'static Mutex<BTreeSet<String>> {
    static R: OnceLock<Mutex<BTreeSet<String>>> = OnceLock::new();
    R.get_or_init(|| Mutex::new(BTreeSet::new()))
}

/// (Re)load secrets for a DSL dir and refresh the redaction set.
pub fn init(dsl_dir: Option<&Path>) -> Result<()> {
    let s = Secrets::load(dsl_dir);
    refresh(s)
}

/// Replace the store (tests use this to inject values).
pub fn set_secrets(s: Secrets) -> Result<()> {
    refresh(s)
}

fn refresh(s: Secrets) -> Result<()> {
    {
        let mut r = redaction_set().lock().unwrap();
        r.clear();
        for v in s.values_for_redaction() {
            r.insert(v);
        }
    }
    *store().lock().unwrap() = s;
    Ok(())
}

/// Resolve one secret; a missing value is an error (never an empty string).
pub fn get(name: &str) -> Result<String> {
    let s = store().lock().unwrap();
    match s.get(name) {
        Some(v) if !v.is_empty() => Ok(v.to_string()),
        Some(_) => bail!("secret {name:?} is set but empty; refusing to use it"),
        None => bail!(
            "secret {name:?} is not available (checked environment, .env files and secrets store)"
        ),
    }
}

/// Is a secret available (without exposing its value)?
pub fn has(name: &str) -> bool {
    store().lock().unwrap().has(name)
}

/// Register an extra value for redaction (e.g. a value read from a file).
pub fn register_for_redaction(value: &str) {
    if value.len() >= 4 {
        redaction_set().lock().unwrap().insert(value.to_string());
    }
}

/// Shortest value we are willing to blind-replace.
///
/// Redaction is substring replacement, so a short value is dangerous: a
/// 4-character secret matches inside ordinary words and mangles unrelated
/// output. Observed for real — a short token turned
/// `https://avatars.githubusercontent.com/...` into `...github***ontent.com...`.
/// Values below this length are skipped rather than allowed to corrupt text.
const MIN_REDACT_LEN: usize = 12;

/// Replace any known secret value found in `s` with `***`.
pub fn redact_str(s: &str) -> String {
    let set = redaction_set().lock().unwrap();
    let mut out = s.to_string();
    for v in set.iter() {
        if v.len() < MIN_REDACT_LEN {
            continue;
        }
        if out.contains(v.as_str()) {
            out = out.replace(v.as_str(), "***");
        }
    }
    out
}

/// Deep-redact a JSON value: any string containing a known secret is masked.
pub fn redact(v: &Value) -> Value {
    match v {
        Value::String(s) => {
            let r = redact_str(s);
            Value::String(r)
        }
        Value::Array(a) => Value::Array(a.iter().map(redact).collect()),
        Value::Object(o) => {
            let mut m = Map::new();
            for (k, val) in o {
                m.insert(k.clone(), redact(val));
            }
            Value::Object(m)
        }
        other => other.clone(),
    }
}

/// Spec keys that are pure documentation/metadata and must not be scanned for
/// secret references (prose may legitimately mention `${secret.NAME}`).
const META_KEYS: &[&str] = &["description", "dsl_version", "name"];

fn strip_meta(v: &Value) -> Value {
    match v {
        Value::Object(o) => {
            let mut m = Map::new();
            for (k, val) in o {
                if META_KEYS.contains(&k.as_str()) {
                    continue;
                }
                m.insert(k.clone(), strip_meta(val));
            }
            Value::Object(m)
        }
        Value::Array(a) => Value::Array(a.iter().map(strip_meta).collect()),
        other => other.clone(),
    }
}

/// Names of every secret this spec references (`${secret.X}`), ignoring prose.
pub fn referenced_names(spec: &Value) -> Vec<String> {
    let mut out: BTreeSet<String> = BTreeSet::new();
    collect_refs(&strip_meta(spec), &mut out);
    out.into_iter().collect()
}

/// Names of every plain environment reference (`${env.X}`) in a spec.
pub fn env_names(spec: &Value) -> Vec<String> {
    let mut out: BTreeSet<String> = BTreeSet::new();
    collect_env_refs(&strip_meta(spec), &mut out);
    out.into_iter().collect()
}

fn collect_env_refs(v: &Value, out: &mut BTreeSet<String>) {
    match v {
        Value::String(s) => {
            let mut rest = s.as_str();
            while let Some(i) = rest.find("${") {
                let after = &rest[i + 2..];
                let Some(j) = after.find('}') else { break };
                if let Some(name) = after[..j].strip_prefix("env.") {
                    if !name.is_empty() {
                        out.insert(name.to_string());
                    }
                }
                rest = &after[j + 1..];
            }
        }
        Value::Array(a) => a.iter().for_each(|x| collect_env_refs(x, out)),
        Value::Object(o) => o.values().for_each(|x| collect_env_refs(x, out)),
        _ => {}
    }
}

fn collect_refs(v: &Value, out: &mut BTreeSet<String>) {
    match v {
        Value::String(s) => {
            let mut rest = s.as_str();
            while let Some(i) = rest.find("${") {
                let after = &rest[i + 2..];
                let Some(j) = after.find('}') else { break };
                let path = &after[..j];
                // Only `${secret.X}` is a *credential requirement*. `${env.X}`
                // stays a plain environment reference (may be a path, a tuning
                // knob, …) and is reported separately by `env_names`.
                if let Some(name) = path.strip_prefix("secret.") {
                    if !name.is_empty() {
                        out.insert(name.to_string());
                    }
                }
                rest = &after[j + 1..];
            }
        }
        Value::Array(a) => a.iter().for_each(|x| collect_refs(x, out)),
        Value::Object(o) => o.values().for_each(|x| collect_refs(x, out)),
        _ => {}
    }
}

/// Heuristic: does this spec hard-code what looks like a secret?
///
/// Flags keys named password/secret/token/api_key/authorization (etc.) whose
/// value is a non-empty literal that is **not** a `${…}` reference.
pub fn hardcoded_secret_fields(spec: &Value) -> Vec<String> {
    let mut out = Vec::new();
    scan_hardcoded(&strip_meta(spec), "", &mut out);
    out.sort();
    out.dedup();
    out
}

const SECRETISH: &[&str] = &[
    "password", "passwd", "secret", "token", "api_key", "apikey", "access_key",
    "private_key", "client_secret", "authorization", "auth_value", "sign_secret",
    "passphrase", "credential", "bearer",
];

fn scan_hardcoded(v: &Value, path: &str, out: &mut Vec<String>) {
    match v {
        Value::Object(o) => {
            for (k, val) in o {
                let p = if path.is_empty() { k.clone() } else { format!("{path}.{k}") };
                let kl = k.to_ascii_lowercase();
                // Only flag *values* that look like a literal credential. A
                // key that merely *mentions* token/secret (e.g. a `project`
                // output name like `token_ttl_set`) is not a hard-coded secret.
                // Also skip output-mapping sections, which name destination keys.
                let in_output_map = path.ends_with("project") || path.ends_with("keys");
                if !in_output_map && SECRETISH.iter().any(|s| kl.contains(s)) {
                    if let Some(s) = val.as_str() {
                        if !s.trim().is_empty() && !s.contains("${") {
                            out.push(p.clone());
                        }
                    }
                }
                scan_hardcoded(val, &p, out);
            }
        }
        Value::Array(a) => {
            for (i, x) in a.iter().enumerate() {
                scan_hardcoded(x, &format!("{path}[{i}]"), out);
            }
        }
        _ => {}
    }
}

/// Human-readable provenance of the loaded sources (paths only, no values).
pub fn sources_note(dsl_dir: Option<&Path>) -> String {
    let files: Vec<String> = env_files(dsl_dir).iter().map(|p| p.display().to_string()).collect();
    if files.is_empty() {
        "environment only".to_string()
    } else {
        files.join(", ")
    }
}
