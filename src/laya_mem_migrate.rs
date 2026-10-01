//! Phase 7: one-shot schema migration for the laya-mem SQLite store.
//!
//! `laya-workflow laya-mem migrate` brings an older store (e.g. v0.4, written
//! before the Phase-4 vector index existed) up to the current schema: it
//! backfills a vector row for every memory that does not yet have one, stamps
//! `PRAGMA user_version`, and writes a `migrate_backfill` audit row. Idempotent
//! by construction — a second run finds nothing to backfill and just re-stamps.

use std::path::Path;

use anyhow::Result;
use serde_json::{json, Value};

use crate::laya_mem_util::{audit_log, call_db_for_db, ensure_schema};

/// Schema version stored in `user_version` after every successful migration.
/// Bump together with column migrations in [`ensure_schema`].
pub const CURRENT_SCHEMA_VERSION: i64 = 5;

/// Read the SQLite `user_version` PRAGMA. `0` means "fresh / pre-versioned".
pub fn schema_version(db_path: &Path) -> i64 {
    call_db_for_db(db_path, json!({ "op": "query", "sql": "PRAGMA user_version" }))
        .ok()
        .and_then(|r| r.get("rows").and_then(|a| a.as_array()).cloned())
        .and_then(|rows| rows.into_iter().next())
        .and_then(|row| {
            row.as_object()
                .and_then(|obj| obj.values().next().cloned())
        })
        .and_then(|v| v.as_i64())
        .unwrap_or(0)
}

/// Set the SQLite `user_version` PRAGMA. Returns the new value.
pub fn set_schema_version(db_path: &Path, version: i64) -> Result<i64> {
    call_db_for_db(
        db_path,
        json!({ "op": "exec", "statements": [format!("PRAGMA user_version = {version}")] }),
    )?;
    Ok(version)
}

/// Phase 7: backfill vectors for every memory that does not yet have one.
/// Returns `(total_memories, backfilled, already_indexed)`. Errors per row are
/// surfaced via the audit_log rather than aborting the whole batch — an HTTP
/// embedding endpoint can be flaky for one row and still leave the rest of the
/// store indexed.
pub fn backfill_vectors(db_path: &Path) -> Result<(usize, usize, usize)> {
    ensure_schema(db_path)?;
    crate::laya_mem_vec::ensure_vector_schema(db_path)?;
    let rows = call_db_for_db(
        db_path,
        json!({
            "op": "query",
            "sql": "SELECT id, content FROM memories WHERE id NOT IN (SELECT memory_id FROM memory_vectors) ORDER BY id"
        }),
    )?;
    let mem_rows: Vec<(String, String)> = rows
        .get("rows")
        .and_then(|r| r.as_array())
        .map(|arr| {
            arr.iter()
                .filter_map(|row| {
                    let id = match row.get("id") {
                        Some(Value::Number(n)) => n.to_string(),
                        Some(Value::String(s)) => s.clone(),
                        _ => return None,
                    };
                    let content = row
                        .get("content")
                        .and_then(|v| v.as_str())
                        .unwrap_or("")
                        .to_string();
                    Some((id, content))
                })
                .collect()
        })
        .unwrap_or_default();
    let total = call_db_for_db(
        db_path,
        json!({ "op": "query", "sql": "SELECT COUNT(*) AS c FROM memories" }),
    )
    .ok()
    .and_then(|r| r.get("rows").and_then(|a| a.as_array()).cloned())
    .and_then(|rows| rows.into_iter().next())
    .and_then(|row| {
        row.as_object()
            .and_then(|obj| obj.values().next().cloned())
    })
    .and_then(|v| v.as_i64())
    .unwrap_or(0) as usize;
    let mut backfilled = 0usize;
    for (id, content) in &mem_rows {
        let vec = crate::laya_mem_vec::encode_mock(content, crate::laya_mem_vec::MOCK_DIM);
        if crate::laya_mem_vec::upsert_vector(db_path, id, &vec).is_ok() {
            backfilled += 1;
        }
    }
    let already = total.saturating_sub(backfilled);
    audit_log(
        db_path,
        "migrate_backfill",
        "",
        &json!({
            "total_memories": total,
            "backfilled": backfilled,
            "already_indexed": already,
            "schema_version": CURRENT_SCHEMA_VERSION,
        }),
    )?;
    set_schema_version(db_path, CURRENT_SCHEMA_VERSION)?;
    Ok((total, backfilled, already))
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::{json, Value};
    use std::path::PathBuf;

    fn tmpdb(tag: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!(
            "laya-mem-migrate-{}-{:?}",
            std::process::id(),
            std::thread::current().id()
        ));
        let _ = std::fs::create_dir_all(&d);
        let p = d.join(format!("{tag}.sqlite"));
        let _ = std::fs::remove_file(&p);
        p
    }

    fn seed_memory(db: &Path, content: &str) {
        let sql = format!(
            "INSERT INTO memories (content, ts, entities, type_scores, node_type) VALUES ('{}', '', '[]', '{{}}', 'OBSERVATION')",
            crate::laya_mem_util::sql_escape(content)
        );
        call_db_for_db(db, json!({ "op": "exec", "statements": [sql] })).unwrap();
    }

    #[test]
    fn backfill_indexes_only_unindexed_rows() {
        let db = tmpdb("backfill");
        ensure_schema(&db).unwrap();
        seed_memory(&db, "alpha document");
        seed_memory(&db, "beta document");
        // Pre-index the first memory, as a Phase-4 persist would have done.
        crate::laya_mem_vec::upsert_vector(
            &db,
            "1",
            &crate::laya_mem_vec::encode_mock("alpha document", crate::laya_mem_vec::MOCK_DIM),
        ).unwrap();

        let (total, backfilled, already) = backfill_vectors(&db).unwrap();
        assert_eq!(total, 2);
        assert_eq!(backfilled, 1, "only memory 2 lacked a vector");
        assert_eq!(already, 1);

        // Both rows are now indexable.
        let vecs = crate::laya_mem_vec::load_vectors(&db).unwrap();
        assert_eq!(vecs.len(), 2);
        assert_eq!(schema_version(&db), CURRENT_SCHEMA_VERSION);
    }

    #[test]
    fn backfill_is_idempotent() {
        let db = tmpdb("idempotent");
        ensure_schema(&db).unwrap();
        seed_memory(&db, "one");
        let (total, backfilled, _) = backfill_vectors(&db).unwrap();
        assert_eq!((total, backfilled), (1, 1));
        // Second run: nothing left to do.
        let (total2, backfilled2, already2) = backfill_vectors(&db).unwrap();
        assert_eq!((total2, backfilled2, already2), (1, 0, 1));
    }

    #[test]
    fn schema_version_round_trips() {
        let db = tmpdb("version");
        ensure_schema(&db).unwrap();
        assert_eq!(schema_version(&db), 0, "fresh DB has no version stamp");
        set_schema_version(&db, 5).unwrap();
        assert_eq!(schema_version(&db), 5);
    }
}
