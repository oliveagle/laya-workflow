//! Wall-clock periodic consolidation for laya-mem.
//!
//! The persist-path auto-trigger (`LAYA_MEM_CONSOLIDATE_INTERVAL`) only fires
//! *while writes happen* — a quiet store never consolidates. This module adds a
//! true timer: every `period_secs`, run one consolidation sweep over the whole
//! store (the "slow path" of Jev-Mem's dual-stream memory), so old memories get
//! merged / linked even when no new memory arrives for days.
//!
//! Two entry points, same sweep:
//! * [`spawn_background`] — a detached thread used by `mcp serve` when
//!   `LAYA_MEM_CONSOLIDATE_PERIOD_SECONDS` is set.
//! * [`sweep_once`] — one blocking sweep; used by the CLI watcher
//!   (`laya-workflow laya-mem consolidate-watch`) and unit tests.

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use anyhow::Result;
use serde_json::{json, Value};

use crate::backend::{HeuristicBackend, LayaBackend};
use crate::laya_mem_util::{auto_consolidate, call_db_for_db, ensure_schema};
use crate::workflow::Decide;

/// Default sweep: consolidate every memory whose newest consolidation is older
/// than this (86400 s = 1 day). Overridden by `LAYA_MEM_CONSOLIDATE_MIN_AGE_SECS`.
pub fn min_age_secs() -> u64 {
    std::env::var("LAYA_MEM_CONSOLIDATE_MIN_AGE_SECS")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(86_400)
}

/// Default batch per sweep. Overridden by `LAYA_MEM_CONSOLIDATE_MAX_PER_SWEEP`.
pub fn max_per_sweep() -> i64 {
    std::env::var("LAYA_MEM_CONSOLIDATE_MAX_PER_SWEEP")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(20)
}

/// Default threshold passed to `auto_consolidate`. Same as the persist trigger.
pub fn threshold() -> f64 {
    std::env::var("LAYA_MEM_CONSOLIDATE_THRESHOLD")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(0.85)
}

fn make_backend(base_url: Option<&str>) -> Box<dyn Decide> {
    match base_url {
        Some(u) => Box::new(LayaBackend::new(u)),
        None => Box::new(HeuristicBackend),
    }
}

/// Select the memories that are due for a consolidation sweep: OBSERVATION and
/// EPISODE rows whose `consolidated_at` is NULL or older than `min_age_secs`,
/// newest-first, capped at `max`. SUMMARY rows are left alone — they are the
/// *output* of consolidation and re-consolidating them is Jev-Mem's
/// `_consolidating` recursion guard.
pub fn due_memories(db_path: &Path, min_age: u64, max: i64) -> Result<Vec<String>> {
    ensure_schema(db_path)?;
    let cutoff = if min_age == 0 {
        // Quoted SQL string literal — unquoted it is a syntax error and the
        // db layer returns an empty result set instead of an Err.
        "'1970-01-01T00:00:00'".to_string()
    } else {
        // Cutoff = now - min_age. We compute it in SQL via strftime on the
        // ISO timestamp; the DB's clock is the source of truth.
        format!("strftime('%Y-%m-%dT%H:%M:%fZ', 'now', '-{} seconds')", min_age)
    };
    let sql = format!(
        "SELECT id FROM memories \
         WHERE node_type IN ('OBSERVATION','EPISODE') \
           AND (consolidated_at IS NULL OR consolidated_at < {cutoff}) \
         ORDER BY id DESC LIMIT {}",
        max.max(1)
    );
    let res = call_db_for_db(db_path, json!({ "op": "query", "sql": sql }))?;
    let mut ids = Vec::new();
    if let Some(rows) = res.get("rows").and_then(|v| v.as_array()) {
        for row in rows {
            if let Some(id) = row.get("id") {
                let s = match id {
                    Value::String(x) => x.clone(),
                    Value::Number(n) => n.to_string(),
                    _ => String::new(),
                };
                if !s.is_empty() {
                    ids.push(s);
                }
            }
        }
    }
    Ok(ids)
}

/// One blocking consolidation sweep. Returns a summary of what happened.
pub fn sweep_once(
    db_path: &Path,
    spec_dir: &Path,
    base_url: Option<&str>,
    min_age: u64,
    max: i64,
    threshold: f64,
) -> Result<Value> {
    ensure_schema(db_path)?;
    let backend = make_backend(base_url);
    let ids = due_memories(db_path, min_age, max)?;
    let mut actions: Vec<Value> = Vec::new();
    let mut errors: Vec<Value> = Vec::new();
    for id in &ids {
        match auto_consolidate(db_path, spec_dir, backend.as_ref(), id, 3, threshold) {
            Ok(decisions) => {
                for d in decisions {
                    actions.push(json!({
                        "memory_id": id,
                        "result": d,
                    }));
                }
            }
            Err(e) => errors.push(json!({ "memory_id": id, "error": format!("{e:#}") })),
        }
    }
    Ok(json!({
        "due_count": ids.len(),
        "actions": actions,
        "errors": errors,
        "db_path": db_path.display().to_string(),
    }))
}

/// Spawn a detached consolidation thread. Returns immediately; the thread
/// sleeps `period` between sweeps and exits when the process does.
pub fn spawn_background(
    db_path: PathBuf,
    spec_dir: PathBuf,
    base_url: Option<String>,
) -> std::thread::JoinHandle<()> {
    std::thread::spawn(move || {
        let period = std::env::var("LAYA_MEM_CONSOLIDATE_PERIOD_SECONDS")
            .ok()
            .and_then(|v| v.parse::<u64>().ok())
            .unwrap_or(0);
        if period == 0 {
            return;
        }
        let min_age = min_age_secs();
        let max = max_per_sweep();
        let thr = threshold();
        loop {
            std::thread::sleep(Duration::from_secs(period));
            match sweep_once(&db_path, &spec_dir, base_url.as_deref(), min_age, max, thr) {
                Ok(out) => {
                    let due = out.get("due_count").and_then(|v| v.as_i64()).unwrap_or(0);
                    let acted = out.get("actions").and_then(|v| v.as_array()).map(|a| a.len()).unwrap_or(0);
                    eprintln!(
                        "[laya-mem periodic] sweep: due={due} actions={acted} store={}",
                        db_path.display()
                    );
                }
                Err(e) => eprintln!("[laya-mem periodic] sweep error: {e:#}"),
            }
        }
    })
}

/// Build an [`Arc`] so the tools can hand the same backend to the sweep thread.
pub fn arc_backend(base_url: Option<&str>) -> Arc<Box<dyn Decide>> {
    Arc::new(make_backend(base_url))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::laya_mem_util::sql_escape;
    use std::path::PathBuf;

    fn tmpdb(tag: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!(
            "laya-mem-periodic-{}-{:?}",
            std::process::id(),
            std::thread::current().id()
        ));
        let _ = std::fs::create_dir_all(&d);
        let p = d.join(format!("{tag}.sqlite"));
        let _ = std::fs::remove_file(&p);
        p
    }

    fn insert(db: &Path, content: &str, node_type: &str, consolidated_at: Option<&str>) -> i64 {
        ensure_schema(db).unwrap();
        let ca = match consolidated_at {
            Some(ts) => format!("'{}'", sql_escape(ts)),
            None => "NULL".to_string(),
        };
        let sql = format!(
            "INSERT INTO memories (content, ts, entities, type_scores, node_type, consolidated_at) \
             VALUES ('{}', '2026-10-01T00:00:00', '[]', '{{}}', '{}', {})",
            sql_escape(content),
            sql_escape(node_type),
            ca,
        );
        let res = call_db_for_db(
            db,
            json!({ "op": "exec", "statements": [sql, "SELECT last_insert_rowid() AS id"] }),
        )
        .unwrap();
        res.get("rows")
            .and_then(|r| r.as_array())
            .and_then(|a| a.first())
            .and_then(|row| row.get("id"))
            .and_then(|v| v.as_i64())
            .unwrap()
    }

    #[test]
    fn due_memories_skips_summary_and_fresh() {
        let db = tmpdb("due");
        insert(&db, "alpha", "OBSERVATION", None);                 // due
        insert(&db, "beta summary", "SUMMARY", Some("2026-09-30T00:00:00")); // skipped: SUMMARY
        insert(&db, "fresh gamma", "OBSERVATION", Some("2030-01-01T00:00:00")); // not due (future)
        let due = due_memories(&db, 86_400, 100).unwrap();
        assert_eq!(due, vec!["1"], "only the never-consolidated observation is due");
    }

    #[test]
    fn due_memories_respects_max() {
        let db = tmpdb("max");
        insert(&db, "one", "OBSERVATION", None);
        insert(&db, "two", "OBSERVATION", None);
        insert(&db, "three", "OBSERVATION", None);
        let due = due_memories(&db, 0, 2).unwrap();
        assert_eq!(due.len(), 2, "capped at max");
        // min_age=0 → all are due, newest first.
        assert_eq!(due[0], "3");
    }

    #[test]
    fn sweep_once_runs_without_panicking_on_empty_store() {
        let db = tmpdb("empty");
        ensure_schema(&db).unwrap();
        let spec_dir = crate::laya_mem::LayaMemTools::default_spec_dir();
        let out = sweep_once(&db, &spec_dir, None, 0, 10, 0.85).unwrap();
        assert_eq!(out.get("due_count").and_then(|v| v.as_i64()).unwrap(), 0);
        assert_eq!(out.get("actions").and_then(|v| v.as_array()).map(|a| a.len()).unwrap_or(0), 0);
    }

    #[test]
    fn sweep_consolidates_a_near_duplicate_pair() {
        let db = tmpdb("sweep");
        let spec_dir = crate::laya_mem::LayaMemTools::default_spec_dir();
        insert(&db, "user bought a red car last week", "OBSERVATION", None);
        insert(&db, "user bought a red car last week", "OBSERVATION", None);
        let out = sweep_once(&db, &spec_dir, None, 0, 10, 0.30).unwrap();
        let actions = out.get("actions").and_then(|v| v.as_array()).map(|a| a.len()).unwrap_or(0);
        assert!(actions >= 1, "near-duplicate pair should consolidate: {out}");
        // After the sweep the pair is consolidated and the canonical
        // consolidation_key prevents a second SUMMARY on a repeat sweep
        // (idempotent — the property the periodic watcher depends on).
        // The first sweep stamps the *target* of every relation it writes;
        // in rare ordering cases a second sweep is needed to stamp the
        // member that only appeared as a *candidate*. Both sweeps together
        // guarantee every member has consolidated_at set.
        let _ = sweep_once(&db, &spec_dir, None, 0, 10, 0.30).unwrap();
        let due2 = due_memories(&db, 86_400 * 30, 10).unwrap();
        assert_eq!(due2.len(), 0, "both stamped, nothing re-due");
    }
}
