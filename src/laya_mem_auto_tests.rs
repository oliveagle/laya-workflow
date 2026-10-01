//! Unit tests for the continuous-consolidation helpers (`apply_consolidation`,
//! `find_similar_memories`, `heuristic_pair_status`). Kept in a sibling file so
//! `src/laya_mem_util.rs` stays under the 1000-line AGENTS.md limit.

use crate::laya_mem_util::*;
use serde_json::{json, Value};
use std::path::{Path, PathBuf};

fn tmpdb(tag: &str) -> PathBuf {
    let d = std::env::temp_dir().join(format!(
        "laya-mem-auto-{}-{:?}",
        std::process::id(),
        std::thread::current().id()
    ));
    let _ = std::fs::create_dir_all(&d);
    let p = d.join(format!("{tag}.sqlite"));
    let _ = std::fs::remove_file(&p);
    p
}

fn insert_memory(db: &Path, content: &str) -> i64 {
    ensure_schema(db).unwrap();
    let sql = format!(
        "INSERT INTO memories (content, ts, entities, type_scores, node_type) VALUES ('{}', '{}', '[]', '{}', 'OBSERVATION')",
        sql_escape(content),
        sql_escape("2026-10-01T00:00:00"),
        sql_escape("{}"),
    );
    let res = call_db_for_db(
        db,
        json!({ "op": "exec", "statements": [sql, "SELECT last_insert_rowid() AS last_insert_rowid".to_string()] }),
    )
    .unwrap();
    res.get("rows")
        .and_then(|r| r.as_array())
        .and_then(|a| a.first())
        .and_then(|row| row.get("last_insert_rowid"))
        .and_then(|v| v.as_i64())
        .expect("insert id")
}

/// Near-duplicate sentences must trip the redundant heuristic (containment),
/// a negation must trip contradiction, and unrelated text trips nothing.
#[test]
fn heuristic_pair_status_classifies_pairs() {
    let (red, con, obs, lnk) = heuristic_pair_status(
        "laya-mem stores memories in SQLite for persistence",
        "laya-mem stores memories in SQLite for durability",
    );
    assert!(!red.is_empty(), "near-duplicate should be redundant: {red:?}");
    assert!(con.is_empty() && obs.is_empty() && lnk.is_empty(), "{con:?} {obs:?} {lnk:?}");

    let (red2, con2, _, _) = heuristic_pair_status(
        "the cache is warm",
        "the cache is never warm",
    );
    assert!(red2.is_empty(), "negated pair is not redundant: {red2:?}");
    assert!(!con2.is_empty(), "negated pair should be contradictory: {con2:?}");

    let (r3, c3, o3, l3) = heuristic_pair_status(
        "release the build",
        "pomegranate juice recipe",
    );
    assert!(r3.is_empty() && c3.is_empty() && o3.is_empty() && l3.is_empty(),
        "unrelated pair must not match: {r3:?} {c3:?} {o3:?} {l3:?}");

    let (r4, _, o4, _) = heuristic_pair_status(
        "the deploy config supersedes the old one",
        "the deploy config replaced the old one",
    );
    assert!(!o4.is_empty(), "update verbs should mark obsolete: {o4:?}");
    assert!(r4.is_empty(), "update verbs are obsolete, not redundant: {r4:?}");
}

/// The FTS5 index must be created *after* the memories table so the very
/// first insert is indexed — the regression that made `find_similar_memories`
/// return nothing on a fresh database.
#[test]
fn first_memory_is_indexed_and_found_similar() {
    let db = tmpdb("fts");
    let id1 = insert_memory(&db, "laya-mem stores memories in SQLite at mem.sqlite");
    ensure_schema(&db).unwrap();
    let id2 = insert_memory(&db, "laya-mem stores memories in SQLite at mem.sqlite too");

    let found = find_similar_memories(
        &db,
        "laya-mem stores memories in SQLite at mem.sqlite too",
        &id2.to_string(),
        3,
    )
    .unwrap();
    assert!(
        found.iter().any(|(id, _)| id == &id1.to_string()),
        "row 1 must be found as a similar candidate, got {found:?}"
    );
    let _ = std::fs::remove_file(&db);
}

/// Merge with `apply_consolidation` creates a non-destructive SUMMARY with a
/// dedupe key; repeating the same decision reuses the existing summary
/// instead of creating a second one.
#[test]
fn apply_merge_is_non_destructive_and_deduped() {
    let db = tmpdb("merge");
    let t = insert_memory(&db, "fact A");
    let c = insert_memory(&db, "fact A (again)");
    let before = count_rows(&db, "SELECT COUNT(*) AS c FROM memories");

    let out = apply_consolidation(
        &db,
        &t.to_string(),
        &c.to_string(),
        &json!({"redundant": 0.95}),
        0.85,
        "merge",
        "fact A consolidated",
        &json!({}),
        &Value::Array(Vec::new()),
        "consolidation",
    )
    .unwrap();
    assert_eq!(out["decision"], "merge");
    assert_eq!(out["link_sub_type"], "REDUNDANT_WITH");
    let summary1 = out["summary_memory_id"].as_i64().expect("summary id");
    assert!(summary1 > 0);

    // Original evidence untouched, one new SUMMARY row.
    let after = count_rows(&db, "SELECT COUNT(*) AS c FROM memories");
    assert_eq!(after, before + 1, "merge must add exactly one row");
    let obs = count_rows(
        &db,
        "SELECT COUNT(*) AS c FROM memories WHERE node_type = 'OBSERVATION'",
    );
    assert_eq!(obs, 2, "original observations must survive");

    // Idempotent: same target/candidate → same summary id, no new row.
    let out2 = apply_consolidation(
        &db,
        &t.to_string(),
        &c.to_string(),
        &json!({"redundant": 0.95}),
        0.85,
        "merge",
        "fact A consolidated",
        &json!({}),
        &Value::Array(Vec::new()),
        "consolidation",
    )
    .unwrap();
    assert_eq!(out2["summary_memory_id"].as_i64(), Some(summary1));
    assert_eq!(
        count_rows(&db, "SELECT COUNT(*) AS c FROM memories"),
        after,
        "dedupe must not add a second summary"
    );

    let audit = count_rows(
        &db,
        "SELECT COUNT(*) AS c FROM audit_log WHERE op = 'consolidation'",
    );
    assert!(audit >= 2, "each decision writes an audit row, got {audit}");
    let _ = std::fs::remove_file(&db);
}

/// A contradiction never merges: no summary row, relation still recorded.
#[test]
fn contradiction_blocks_summary() {
    let db = tmpdb("contra");
    let t = insert_memory(&db, "status is green");
    let c = insert_memory(&db, "status is never green");
    let before = count_rows(&db, "SELECT COUNT(*) AS c FROM memories");

    let out = apply_consolidation(
        &db,
        &t.to_string(),
        &c.to_string(),
        &json!({"contradiction": 0.99}),
        0.85,
        "keep_separate",
        "",
        &json!({}),
        &Value::Array(Vec::new()),
        "consolidation",
    )
    .unwrap();
    assert_eq!(out["decision"], "keep_separate");
    assert_eq!(out["link_sub_type"], "CONTRADICTS");
    assert!(out["summary_memory_id"].is_null(), "contradiction must not produce a summary");
    assert_eq!(
        count_rows(&db, "SELECT COUNT(*) AS c FROM memories"),
        before,
        "keep_separate adds no rows"
    );
    assert_eq!(
        count_rows(&db, "SELECT COUNT(*) AS c FROM relations WHERE link_sub_type = 'CONTRADICTS'"),
        1
    );
    let _ = std::fs::remove_file(&db);
}
