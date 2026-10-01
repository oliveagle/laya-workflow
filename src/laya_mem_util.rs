//! SQLite plumbing + schema migration + audit helpers for the laya-mem tools.
//! Split out of `laya_mem.rs` to keep every source file under the 1000-line
//! AGENTS.md limit.

use std::path::Path;

use anyhow::{anyhow, bail, Context, Result};
use serde_json::{json, Value};

use crate::workflow::Decide;
use crate::capability::db::{call_db, DbCap};
use crate::capability::Policy;
use crate::laya_mem::EMBEDDED_SPECS;
use crate::spec::load_file;
use crate::workflow::ResilientWorkflow;

// ─── helpers ──────────────────────────────────────────────────────────
// ─── helpers ──────────────────────────────────────────────────────────────

/// Write the embedded specs into `dir`, skipping any file that already exists.
///
/// Returns `Ok(())` when every embedded spec is present afterwards, so a
/// partially-unwritable directory fails here (at startup) rather than later as
/// a confusing `spec not found` in the middle of a tool call.
pub fn ensure_specs(dir: &Path) -> Result<()> {
    std::fs::create_dir_all(dir)
        .with_context(|| format!("cannot create spec dir {}", dir.display()))?;
    for (name, body) in EMBEDDED_SPECS {
        let path = dir.join(name);
        if path.exists() {
            continue;
        }
        std::fs::write(&path, body)
            .with_context(|| format!("cannot write spec {}", path.display()))?;
    }
    for (name, _) in EMBEDDED_SPECS {
        let path = dir.join(name);
        if !path.is_file() {
            bail!("spec missing after install: {}", path.display());
        }
    }
    Ok(())
}

/// Overwrite every spec in `dir` with the embedded copy, discarding local edits.
pub fn restore_specs(dir: &Path) -> Result<usize> {
    std::fs::create_dir_all(dir)
        .with_context(|| format!("cannot create spec dir {}", dir.display()))?;
    for (name, body) in EMBEDDED_SPECS {
        std::fs::write(dir.join(name), body)
            .with_context(|| format!("cannot write spec {}", dir.join(name).display()))?;
    }
    Ok(EMBEDDED_SPECS.len())
}

pub(crate) fn run_spec(spec_dir: &Path, backend: &dyn Decide, name: &str, state: &Value) -> Result<Value> {
    let path = spec_dir.join(format!("{name}.json"));
    if !path.is_file() {
        bail!("spec not found: {}", path.display());
    }
    let wf: ResilientWorkflow = load_file(
        path.to_str().ok_or_else(|| anyhow!("non-utf8 spec path"))?,
    )
    .with_context(|| format!("loading {}", path.display()))?;
    let outcome = wf
        .run(backend, state)
        .with_context(|| format!("running spec {name}"))?;
    Ok(outcome.to_json())
}

pub(crate) fn call_db_for_db(db_path: &Path, with: Value) -> Result<Value> {
    let cap = DbCap {
        sqlite: db_path.display().to_string(),
        duckdb: String::new(),
        alias: "sqlite".to_string(),
        op: String::new(),
        readonly: false,
        format: "json".to_string(),
        mode: "embed".to_string(),
        endpoint: String::new(),
        timeout_ms: 30_000,
    };
    let parent = db_path
        .parent()
        .unwrap_or_else(|| Path::new("."))
        .display()
        .to_string();
    let policy = Policy {
        allow_exec: true,
        allow_hosts: Vec::new(),
        allow_paths: vec![parent],
        max_timeout_ms: 60_000,
        max_output: 1 << 20,
        retries: 0,
    };
    let state = json!({});
    call_db(&cap, &with, &state, &policy)
        .with_context(|| format!("db call for {}", db_path.display()))
}

pub(crate) fn ensure_parent(p: &Path) -> Result<()> {
    if let Some(parent) = p.parent() {
        if !parent.as_os_str().is_empty() && !parent.exists() {
            std::fs::create_dir_all(parent)
                .with_context(|| format!("create_dir_all {}", parent.display()))?;
        }
    }
    Ok(())
}

pub(crate) fn sql_escape(s: &str) -> String {
    s.replace('\'', "''")
}

/// FNV-1a 64-bit hash, hex-formatted. Not cryptographic; only used to
/// dedupe consolidation summaries (Jev-Mem equivalent: sha256(node_id+other_id)).
pub(crate) fn fnv1a_hex(s: &str) -> String {
    let mut h: u64 = 0xcbf29ce484222325;
    for b in s.as_bytes() {
        h ^= *b as u64;
        h = h.wrapping_mul(0x100000001b3);
    }
    format!("{:016x}", h)
}

/// Create / migrate the Laya-Mem schema. Idempotent: each ALTER TABLE ADD COLUMN
/// is guarded by a PRAGMA table_info check so re-runs on older DBs add only the
/// missing columns. Adds:
///   memories: source_memory_ids, consolidation_key, consolidation_action,
///             consolidated_at, node_type
///   relations: link_sub_type, status
///   new table audit_log: id, ts, op, memory_id, details
pub(crate) fn ensure_schema(db_path: &Path) -> Result<()> {
    let _ = call_db_for_db(
        db_path,
        json!({
            "op": "exec",
            "statements": [
                "CREATE TABLE IF NOT EXISTS memories (id INTEGER PRIMARY KEY, content TEXT NOT NULL, ts TEXT, entities TEXT, type_scores TEXT NOT NULL)",
                "CREATE TABLE IF NOT EXISTS relations (id INTEGER PRIMARY KEY, source TEXT, target TEXT, link_type TEXT, probability REAL)",
                "CREATE TABLE IF NOT EXISTS audit_log (id INTEGER PRIMARY KEY, ts TEXT, op TEXT NOT NULL, memory_id TEXT, details TEXT)"
            ]
        }),
    )?;

    let mem_info = call_db_for_db(
        db_path,
        json!({ "op": "query", "sql": "PRAGMA table_info(memories)" }),
    )?;
    let mut existing: Vec<String> = Vec::new();
    if let Some(rows) = mem_info.get("rows").and_then(|v| v.as_array()) {
        for row in rows {
            if let Some(name) = row.get("name").and_then(|v| v.as_str()) {
                existing.push(name.to_string());
            }
        }
    }
    let add_col = |col: &str, decl: &str| -> String {
        format!("ALTER TABLE memories ADD COLUMN {} {}", col, decl)
    };
    let mem_migrations: &[(&str, &str)] = &[
        ("source_memory_ids", "TEXT"),
        ("consolidation_key", "TEXT"),
        ("consolidation_action", "TEXT"),
        ("consolidated_at", "TEXT"),
        ("node_type", "TEXT DEFAULT 'OBSERVATION'"),
    ];
    let mut stmts: Vec<String> = Vec::new();
    for (col, decl) in mem_migrations {
        if !existing.iter().any(|n| n == col) {
            stmts.push(add_col(col, decl));
        }
    }

    let rel_info = call_db_for_db(
        db_path,
        json!({ "op": "query", "sql": "PRAGMA table_info(relations)" }),
    )?;
    let mut existing_rel: Vec<String> = Vec::new();
    if let Some(rows) = rel_info.get("rows").and_then(|v| v.as_array()) {
        for row in rows {
            if let Some(name) = row.get("name").and_then(|v| v.as_str()) {
                existing_rel.push(name.to_string());
            }
        }
    }
    let rel_migrations: &[(&str, &str)] = &[
        ("link_sub_type", "TEXT"),
        ("status", "TEXT DEFAULT 'ACTIVE'"),
    ];
    for (col, decl) in rel_migrations {
        if !existing_rel.iter().any(|n| n == col) {
            stmts.push(format!("ALTER TABLE relations ADD COLUMN {} {}", col, decl));
        }
    }
    if !stmts.is_empty() {
        call_db_for_db(db_path, json!({ "op": "exec", "statements": stmts }))?;
    }
    Ok(())
}

/// Append an audit_log row. Used by consolidate + future periodic hooks.
pub(crate) fn audit_log(db_path: &Path, op: &str, memory_id: &str, details: &Value) -> Result<i64> {
    let details_esc = sql_escape(&details.to_string());
    let mem_id_esc = sql_escape(memory_id);
    let op_esc = sql_escape(op);
    let ts_esc = sql_escape(&now_iso());
    let res = call_db_for_db(
        db_path,
        json!({
            "op": "exec",
            "statements": [
                format!(
                    "INSERT INTO audit_log (ts, op, memory_id, details) VALUES ('{ts_esc}', '{op_esc}', '{mem_id_esc}', '{details_esc}')"
                ),
                "SELECT last_insert_rowid() AS last_insert_rowid".to_string()
            ]
        }),
    )?;
    let id = res
        .get("rows")
        .and_then(|r| r.as_array())
        .and_then(|a| a.first())
        .and_then(|row| row.get("last_insert_rowid"))
        .and_then(|v| v.as_i64())
        .unwrap_or(-1);
    Ok(id)
}

pub(crate) fn now_iso() -> String {
    extern "C" {
        fn time(t: *mut i64) -> i64;
    }
    unsafe {
        let mut t: i64 = 0;
        time(&mut t as *mut i64);
        let mut tm = std::mem::MaybeUninit::<libc::tm>::uninit();
        let ok = libc::localtime_r(&t, tm.as_mut_ptr());
        if ok.is_null() {
            return String::new();
        }
        let tm = tm.assume_init();
        format!(
            "{:04}-{:02}-{:02}T{:02}:{:02}:{:02}",
            tm.tm_year + 1900, tm.tm_mon + 1, tm.tm_mday,
            tm.tm_hour, tm.tm_min, tm.tm_sec
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;
    use crate::laya_mem::{EMBEDDED_SPECS, LayaMemTools};

    fn tmpdir(tag: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!(
            "laya-mem-{tag}-{}-{:?}",
            std::process::id(),
            std::thread::current().id()
        ));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    /// The embedded specs are the behavioural contract of the four tools. A
    /// silent `include_str!` typo would compile fine and only surface as
    /// `spec not found` at the first tool call, so parse them here.
    #[test]
    fn embedded_specs_are_valid_dsl() {
        for (name, body) in EMBEDDED_SPECS {
            let v: Value = serde_json::from_str(body)
                .unwrap_or_else(|e| panic!("{name} is not valid JSON: {e}"));
            assert_eq!(v.get("dsl_version").and_then(Value::as_u64), Some(2), "{name}");
            assert!(v.get("start").is_some(), "{name} has no start node");
            assert!(v.get("nodes").and_then(Value::as_array).is_some_and(|n| !n.is_empty()),
                "{name} has no nodes");
        }
    }

    /// Every spec the tools actually run must be embedded — a name referenced in
    /// `run_spec` but missing from the table is a runtime-only failure.
    #[test]
    fn every_referenced_spec_is_embedded() {
        for name in ["memory_type", "admission", "routing", "stopping"] {
            let embedded = EMBEDDED_SPECS.iter().any(|(f, _)| *f == format!("{name}.json"));
            assert!(embedded, "{name}.json is run but not embedded");
        }
    }

    #[test]
    fn ensure_specs_writes_all_and_is_idempotent() {
        let dir = tmpdir("ensure");
        ensure_specs(&dir).unwrap();
        for (name, body) in EMBEDDED_SPECS {
            let got = std::fs::read_to_string(dir.join(name)).unwrap();
            assert_eq!(got, body, "{name} content mismatch");
        }
        // A second call must not clobber a local edit.
        let edited = dir.join("routing.json");
        std::fs::write(&edited, "{\"dsl_version\":2,\"start\":\"EDITED\"}").unwrap();
        ensure_specs(&dir).unwrap();
        assert!(std::fs::read_to_string(&edited).unwrap().contains("EDITED"));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn restore_specs_overwrites_edits() {
        let dir = tmpdir("restore");
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("routing.json"), "{\"dsl_version\":2,\"start\":\"EDITED\"}").unwrap();
        let n = restore_specs(&dir).unwrap();
        assert_eq!(n, EMBEDDED_SPECS.len());
        assert!(std::fs::read_to_string(dir.join("routing.json")).unwrap().contains("\"nodes\""));
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// The regression this whole change exists for: a binary whose source tree is
    /// gone must still be able to serve. `default_spec_dir` has to land on a
    /// directory holding all 8 specs, not on a compile-time path.
    #[test]
    fn default_spec_dir_resolves_without_the_source_tree() {
        let dir = tmpdir("default-spec");
        // Emulate an installed binary in a state root, with no $HOME override
        // reachable from the checkout.
        std::env::set_var("LAYA_MEM_SPEC_DIR", &dir);
        let resolved = LayaMemTools::default_spec_dir();
        assert_eq!(resolved, dir);
        for (name, _) in EMBEDDED_SPECS {
            assert!(resolved.join(name).is_file(), "{name} not materialized");
        }
        std::env::remove_var("LAYA_MEM_SPEC_DIR");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn db_path_defaults_under_the_state_root() {
        let saved: Vec<_> = ["LAYA_MEM_SQLITE", "LAYA_HOME", "HOME"]
            .iter()
            .map(|v| (v.to_string(), std::env::var_os(v)))
            .collect();
        for (v, _) in &saved {
            std::env::remove_var(v);
        }
        std::env::set_var("LAYA_HOME", "/tmp/laya-state-x");
        assert_eq!(
            LayaMemTools::default_db_path(),
            PathBuf::from("/tmp/laya-state-x/laya-mem/codex.sqlite")
        );
        for (name, value) in saved {
            match value {
                Some(v) => std::env::set_var(&name, v),
                None => std::env::remove_var(&name),
            }
        }
    }
}
