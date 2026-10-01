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

pub(crate) fn count_rows(db_path: &Path, sql: &str) -> i64 {
    let res = call_db_for_db(db_path, json!({ "op": "query", "sql": sql }));
    match res {
        Ok(r) => r
            .get("rows")
            .and_then(|a| a.as_array())
            .and_then(|a| a.first())
            .and_then(|row| row.as_object().and_then(|o| o.values().next().cloned()))
            .and_then(|v| v.as_i64())
            .unwrap_or(0),
        Err(_) => 0,
    }
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

    // FTS5: create the virtual table + sync triggers only AFTER the memories
    // table exists. Creating them before the table would cause the very first
    // INSERT to run with no trigger in place, so row 1 is never indexed and
    // `find_similar_memories` silently returns an empty candidate list.
    // We use the default (internal-content) FTS5 mode — the external-content
    // form (`content='memories'`) requires `INSERT INTO ft(ft, rowid, …)
    // VALUES('insert', …)` trigger syntax, and mixing it with plain
    // `INSERT INTO ft(rowid, content)` silently skips indexing.
    call_db_for_db(
        db_path,
        json!({
            "op": "exec",
            "statements": [
                "CREATE VIRTUAL TABLE IF NOT EXISTS memories_fts USING fts5(content, tokenize='unicode61 remove_diacritics 2')",
                "CREATE TRIGGER IF NOT EXISTS memories_ai AFTER INSERT ON memories BEGIN INSERT INTO memories_fts(rowid, content) VALUES (new.id, new.content); END",
                "CREATE TRIGGER IF NOT EXISTS memories_ad AFTER DELETE ON memories BEGIN INSERT INTO memories_fts(memories_fts, rowid, content) VALUES('delete', old.id, old.content); END",
                "CREATE TRIGGER IF NOT EXISTS memories_au AFTER UPDATE ON memories BEGIN INSERT INTO memories_fts(memories_fts, rowid, content) VALUES('delete', old.id, old.content); INSERT INTO memories_fts(rowid, content) VALUES (new.id, new.content); END"
            ]
        }),
    )?;

    // Repair stale FTS indexes (e.g. DBs created before the trigger-order fix,
    // or an external-content `memories_fts` whose first row was never indexed).
    // `INSERT INTO memories_fts(memories_fts) VALUES('rebuild')` re-syncs the
    // index from the memories table; a count mismatch is a cheap signal.
    let mem_count = count_rows(db_path, "SELECT COUNT(*) AS c FROM memories");
    let fts_count = count_rows(db_path, "SELECT COUNT(*) AS c FROM memories_fts");
    if mem_count != fts_count {
        let _ = call_db_for_db(
            db_path,
            json!({ "op": "exec", "statements": ["INSERT INTO memories_fts(memories_fts) VALUES('rebuild')"] }),
        );
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

/// Apply one consolidation decision to the SQLite store.
///
/// Shared by the manual `laya_mem_consolidate` MCP tool and the auto-trigger
/// (`auto_consolidate`). Non-destructive by construction:
/// - writes a typed relation (`SEMANTIC`/`TEMPORAL` × `REDUNDANT_WITH`/`CONTRADICTS`/...)
/// - when `representation` ∈ {merge, promote} and `summary_content` is non-empty
///   AND `contradiction < threshold`, creates a SUMMARY memory with
///   `consolidation_key=fnv1a(target|candidate)` and `source_memory_ids=[target, candidate]`
/// - skips the summary write if the same key already exists (dedupe)
/// - always emits an `audit_log` row (op=`consolidation` or op=`auto_consolidation`)
pub(crate) fn apply_consolidation(
    db_path: &Path,
    target: &str,
    candidate: &str,
    scores: &Value,
    threshold: f64,
    representation: &str,
    summary_content: &str,
    summary_type_scores: &Value,
    summary_entities: &Value,
    audit_op: &str,
) -> Result<Value> {
    ensure_schema(db_path)?;

    let redundant = scores.get("redundant").and_then(|v| v.as_f64()).unwrap_or(0.0);
    let contradiction = scores.get("contradiction").and_then(|v| v.as_f64()).unwrap_or(0.0);
    let obsolete = scores.get("obsolete").and_then(|v| v.as_f64()).unwrap_or(0.0);
    let link = scores.get("link").and_then(|v| v.as_f64()).unwrap_or(0.0);

    let (link_sub_type, link_type_label, prob) = if contradiction >= threshold {
        ("CONTRADICTS", "SEMANTIC", contradiction)
    } else if obsolete >= threshold {
        ("OBSOLETE", "TEMPORAL", obsolete)
    } else if redundant >= threshold {
        ("REDUNDANT_WITH", "SEMANTIC", redundant)
    } else if link >= threshold {
        ("RELATED_TO", "SEMANTIC", link)
    } else {
        ("", "", 0.0)
    };

    let mut written_relations: Vec<i64> = Vec::new();

    if !link_sub_type.is_empty() {
        let rel_sql = format!(
            "INSERT INTO relations (source, target, link_type, link_sub_type, probability, status) VALUES ('{s}', '{t}', '{lt}', '{lst}', {p}, 'ACTIVE')",
            s = sql_escape(target),
            t = sql_escape(candidate),
            lt = sql_escape(link_type_label),
            lst = sql_escape(link_sub_type),
            p = prob,
        );
        let res = call_db_for_db(
            db_path,
            json!({ "op": "exec", "statements": [rel_sql, "SELECT last_insert_rowid() AS last_insert_rowid".to_string()] }),
        )?;
        if let Some(rows) = res.get("rows").and_then(|r| r.as_array()) {
            for row in rows {
                if let Some(id) = row.get("last_insert_rowid").and_then(|v| v.as_i64()) {
                    written_relations.push(id);
                }
            }
        }
    }

    let mut summary_memory_id: Option<i64> = None;
    let mut summary_blocked_reason: Option<String> = None;
    let allow_summarize = (representation == "merge" || representation == "promote")
        && contradiction < threshold
        && !summary_content.trim().is_empty();
    if representation == "merge" || representation == "promote" {
        if contradiction >= threshold {
            summary_blocked_reason = Some(
                "contradiction>=threshold forces keep_separate (non-destructive)".to_string()
            );
        } else if summary_content.trim().is_empty() {
            summary_blocked_reason = Some(
                "summary_content required for merge/promote".to_string()
            );
        }
    }

    if allow_summarize {
        let key = fnv1a_hex(&format!("{}|{}", target, candidate));
        let dup_q = format!(
            "SELECT id FROM memories WHERE consolidation_key = '{}' LIMIT 1",
            sql_escape(&key)
        );
        let dup = call_db_for_db(db_path, json!({ "op": "query", "sql": dup_q }))?;
        let existing: Option<i64> = dup
            .get("rows").and_then(|r| r.as_array())
            .and_then(|a| a.first())
            .and_then(|row| row.get("id"))
            .and_then(|v| v.as_i64());
        if let Some(id) = existing {
            summary_memory_id = Some(id);
            summary_blocked_reason = Some(format!(
                "consolidation_key={} already produced summary id={}", key, id
            ));
        } else {
            let entities_json = serde_json::to_string(summary_entities)?;
            let type_scores_json = serde_json::to_string(summary_type_scores)?;
            let summary_esc = sql_escape(summary_content);
            let ts_esc = sql_escape(&now_iso());
            let entities_esc = sql_escape(&entities_json);
            let type_scores_esc = sql_escape(&type_scores_json);
            let key_esc = sql_escape(&key);
            let action_esc = sql_escape(representation);
            let sources_esc = sql_escape(
                &serde_json::to_string(&[target, candidate])
                    .unwrap_or_else(|_| "[]".to_string())
            );
            let summary_sql = format!(
                "INSERT INTO memories (content, ts, entities, type_scores, node_type, source_memory_ids, consolidation_key, consolidation_action, consolidated_at) VALUES ('{summary_esc}', '{ts_esc}', '{entities_esc}', '{type_scores_esc}', 'SUMMARY', '{sources_esc}', '{key_esc}', '{action_esc}', '{ts_esc}')"
            );
            let res = call_db_for_db(
                db_path,
                json!({ "op": "exec", "statements": [summary_sql, "SELECT last_insert_rowid() AS last_insert_rowid".to_string()] }),
            )?;
            summary_memory_id = res
                .get("rows").and_then(|r| r.as_array())
                .and_then(|a| a.first())
                .and_then(|row| row.get("last_insert_rowid"))
                .and_then(|v| v.as_i64());
            // A SUMMARY row must be retrievable through recall too (Phase 4):
            // index its vector like an observation.
            if let Some(sid) = summary_memory_id {
                let _ = crate::laya_mem_vec::upsert_vector(
                    db_path,
                    &sid.to_string(),
                    &crate::laya_mem_vec::encode_mock(summary_content, crate::laya_mem_vec::MOCK_DIM),
                );
            }
        }
    }

    let audit_details = json!({
        "target": target,
        "candidate": candidate,
        "scores": scores,
        "representation": representation,
        "threshold": threshold,
        "link_sub_type": link_sub_type,
        "summary_memory_id": summary_memory_id,
        "summary_blocked_reason": summary_blocked_reason,
    });
    let audit_id = audit_log(db_path, audit_op, target, &audit_details)?;

    let verify_rels = call_db_for_db(
        db_path,
        json!({ "op": "query", "sql": format!(
            "SELECT id, source, target, link_type, link_sub_type, probability, status FROM relations WHERE source IN ('{t}', '{c}') AND target IN ('{t}', '{c}') ORDER BY id DESC LIMIT 10",
            t = sql_escape(target), c = sql_escape(candidate)) }),
    )?;

    Ok(json!({
        "decision": representation,
        "link_sub_type": link_sub_type,
        "link_type": link_type_label,
        "probability": prob,
        "relation_ids": written_relations,
        "summary_memory_id": summary_memory_id,
        "summary_blocked_reason": summary_blocked_reason,
        "audit_id": audit_id,
        "op": audit_op,
        "recent_relations": verify_rels.get("rows").cloned().unwrap_or(Value::Null),
        "db_path": db_path.display().to_string(),
    }))
}

/// Find the top-k rows of the memory index most similar to `content`, using
/// FTS5 bm25 over `memories_fts`. Excludes `exclude_id` and SUMMARY rows
/// (consolidation output rows would always be "self-similar").
pub(crate) fn find_similar_memories(
    db_path: &Path,
    content: &str,
    exclude_id: &str,
    top_k: usize,
) -> Result<Vec<(String, String)>> {
    ensure_schema(db_path)?;
    let tokens: Vec<String> = content
        .split_whitespace()
        .map(|w| w.trim_matches(|c: char| !c.is_alphanumeric()))
        .filter(|w| w.len() >= 3)
        .map(|w| format!("\"{}\"", sql_escape(w)))
        .collect();
    if tokens.is_empty() {
        return Ok(Vec::new());
    }
    let match_expr = tokens.join(" OR ");
    let sql = format!(
        "SELECT m.id, m.content FROM memories_fts f JOIN memories m ON m.id = f.rowid \
         WHERE memories_fts MATCH '{match_expr}' AND m.id <> {exclude} AND m.node_type <> 'SUMMARY' \
         ORDER BY bm25(memories_fts) LIMIT {k}",
        exclude = sql_escape(exclude_id),
        k = (top_k.max(1) * 4).to_string(),
    );
    // Phase 4: the candidate list for consolidation is now hybrid — BM25 +
    // dense cosine, fused with RRF (Jev-Mem `query_engine._rrf_fusion`). Pure
    // token overlap misses paraphrases ("bought a car" vs "purchased a
    // vehicle"); the dense stream catches them when the mock encoder shares
    // slot mass on a shared token, and an OpenAI-compatible endpoint when
    // configured makes it genuinely semantic. FTS still supplies lexical
    // precision, so both streams are fused rather than replaced.
    let ids = crate::laya_mem_vec::hybrid_top_ids(db_path, &sql, content, exclude_id, top_k.max(1))?;
    if ids.is_empty() {
        // No tokens at all (query too short) → nothing to fuse.
        if tokens.is_empty() {
            return Ok(Vec::new());
        }
        // Fall back to pure BM25 (dual-check, e.g. zero vectors + empty fts).
        let res = call_db_for_db(db_path, json!({ "op": "query", "sql": sql }))?;
        let mut out = Vec::new();
        if let Some(rows) = res.get("rows").and_then(|r| r.as_array()) {
            for row in rows {
                let id = row.get("id").and_then(|v| v.as_i64()).map(|n| n.to_string()).unwrap_or_default();
                let content = row.get("content").and_then(|v| v.as_str()).unwrap_or("").to_string();
                if !id.is_empty() {
                    out.push((id, content));
                }
            }
        }
        return Ok(out);
    }
    let id_list = ids
        .iter()
        .map(|id| format!("'{}'", sql_escape(id)))
        .collect::<Vec<_>>()
        .join(",");
    let sql2 = format!(
        "SELECT m.id, m.content FROM memories m WHERE m.id IN ({id_list}) AND m.node_type <> 'SUMMARY'"
    );
    let res = call_db_for_db(db_path, json!({ "op": "query", "sql": sql2 }))?;
    let mut row_map: std::collections::HashMap<String, String> = std::collections::HashMap::new();
    if let Some(rows) = res.get("rows").and_then(|r| r.as_array()) {
        for row in rows {
            let id = row.get("id").and_then(|v| v.as_i64()).map(|n| n.to_string()).unwrap_or_default();
            let c = row.get("content").and_then(|v| v.as_str()).unwrap_or("").to_string();
            if !id.is_empty() {
                row_map.insert(id, c);
            }
        }
    }
    // Preserve fused order.
    let mut out = Vec::new();
    for id in ids {
        if let Some(c) = row_map.remove(&id) {
            out.push((id, c));
        }
    }
    Ok(out)
}

/// Fetch one memory row by id as a JSON object (node_state shape).
pub(crate) fn fetch_memory_row(db_path: &Path, id: &str) -> Result<Value> {
    ensure_schema(db_path)?;
    let sql = format!(
        "SELECT id, content, ts, entities, type_scores, node_type, consolidation_key, consolidation_action, source_memory_ids FROM memories WHERE id = {} LIMIT 1",
        sql_escape(id)
    );
    let res = call_db_for_db(db_path, json!({ "op": "query", "sql": sql }))?;
    Ok(res
        .get("rows")
        .and_then(|r| r.as_array())
        .and_then(|a| a.first().cloned())
        .unwrap_or(Value::Null))
}

/// Run the consolidation DSL spec for one (new_memory, candidate) pair and
/// translate the result into (scores, representation, summary_content).
/// Cheap deterministic pair scoring used to populate the consolidation spec's
/// heuristic matchers when no Laya decision backend is configured. Mirrors
/// Jev-Mem's `find_candidates` + noul semantics with a token-level proxy:
/// - redundant: Jaccard token overlap >= 0.80 → high
/// - contradiction: opposite-claim pair (word + negation/antonym pattern) → high
/// - obsolete: update/supersede/replaces verb + overlap >= 0.5 → high
/// - link: Jaccard overlap >= 0.3 (corroborating / related) → high
///
/// Returns four status strings — `"redundant duplicate paraphrase"`,
/// `"contradict conflict incompatible"`, `"obsolete supersede replaced"`,
/// `"related corroborat"` — or the empty string. The consolidation spec's
/// match_any patterns match these; empty string produces p_miss.
pub(crate) fn heuristic_pair_status(
    new_content: &str,
    cand_content: &str,
) -> (String, String, String, String) {
    let tokenize = |s: &str| -> Vec<String> {
        s.to_lowercase()
            .split(|c: char| !c.is_alphanumeric())
            .filter(|w| !w.is_empty() && w.len() > 2)
            .map(|w| w.to_string())
            .collect()
    };
    let a = tokenize(new_content);
    let b = tokenize(cand_content);
    if a.is_empty() || b.is_empty() {
        return (String::new(), String::new(), String::new(), String::new());
    }
    let a_set: std::collections::HashSet<_> = a.iter().cloned().collect();
    let b_set: std::collections::HashSet<_> = b.iter().cloned().collect();
    let inter = a_set.intersection(&b_set).count();
    let union = a_set.union(&b_set).count();
    let jaccard = if union == 0 { 0.0 } else { inter as f64 / union as f64 };
    // Containment: overlap relative to the smaller set. Paraphrase pairs with
    // near-identical tokens but different vocabulary sizes score higher here
    // than under plain Jaccard, so 0.80 catches them.
    let min_len = a_set.len().min(b_set.len());
    let containment = if min_len == 0 { 0.0 } else { inter as f64 / min_len as f64 };

    let lower = format!("{}\n{}", new_content.to_lowercase(), cand_content.to_lowercase());
    let has_negation = ["not ", "n't ", "never ", "cannot ", "opposite ", "no longer "]
        .iter().any(|w| lower.contains(w));
    let has_update = ["updated ", "superseded ", "replaced ", "replaces ", "supersedes "]
        .iter().any(|w| lower.contains(w));

    // Contradiction outranks redundancy (mirrors the spec's threshold rules:
    // CONTRADICTS is matched first). A negation flips a near-duplicate pair
    // into a contradiction instead of a merge.
    let con = if (jaccard >= 0.40 || containment >= 0.50) && has_negation {
        "contradict conflict incompatible".to_string()
    } else { String::new() };
    let red = if con.is_empty() && (jaccard >= 0.80 || containment >= 0.85) {
        "redundant duplicate paraphrase".to_string()
    } else { String::new() };
    let obs = if (jaccard >= 0.40 || containment >= 0.50) && has_update {
        "obsolete supersede replaced".to_string()
    } else { String::new() };
    let lnk = if (jaccard >= 0.30 || containment >= 0.35) && red.is_empty() && con.is_empty() && obs.is_empty() {
        "related corroborat".to_string()
    } else { String::new() };

    (red, con, obs, lnk)
}

pub(crate) fn evaluate_consolidation(
    spec_dir: &Path,
    backend: &dyn Decide,
    new_memory_row: &Value,
    candidate_row: &Value,
    threshold: f64,
) -> Result<(Value, String, String)> {
    let new_content = new_memory_row.get("content").and_then(|v| v.as_str()).unwrap_or("");
    let cand_content = candidate_row.get("content").and_then(|v| v.as_str()).unwrap_or("");

    // Pre-populate the `pair_0_*_status` fields consumed by the consolidation
    // DSL spec's heuristic matchers. The HeuristicBackend reads
    // `state[pair_0_redundant_status]` (or similar) and matches against the
    // `match_any` patterns; without a populated field, every noul falls back
    // to p_miss and the auto-trigger never produces a non-trivial decision.
    let (red_st, con_st, obs_st, lnk_st) = heuristic_pair_status(new_content, cand_content);
    let state = json!({
        "new_memory": new_memory_row,
        "candidates": [candidate_row],
        "pair_0_redundant_status": red_st,
        "pair_0_contradiction_status": con_st,
        "pair_0_obsolete_status": obs_st,
        "pair_0_link_status": lnk_st,
    });
    let out = run_spec(spec_dir, backend, "consolidation", &state)?;

    // The DSL runner returns `out.result` (the spec's terminal JSON) plus
    // `out.trace` and `out.history`. For a single-node consolidation spec, the
    // answers are exposed as `result.<question_name>` (per-question probability)
    // and the typed label as `result.label`. The runner also flattens some
    // fields onto the top level for legacy callers.
    let res = out.get("result").cloned().unwrap_or_else(|| out.clone());

    // The consolidation DSL spec returns the relation subtype via
    // `result.label` (REDUNDANT_WITH / CONTRADICTS / OBSOLETE / RELATED_TO /
    // KEEP_SEPARATE). The runner only surfaces the *action* question's answer
    // (`result.action_answer`), not every per-question probability, so we
    // derive the four scores from the label — they are the evidence record
    // `apply_consolidation` persists into the relation row. The high value
    // (0.9) mirrors `p_hit` in the spec's heuristic matchers.
    let label = res.get("label").and_then(|v| v.as_str()).unwrap_or("KEEP_SEPARATE");
    let (redundant, contradiction, obsolete, link) = match label {
        "REDUNDANT_WITH" => (0.9, 0.0, 0.0, 0.0),
        "CONTRADICTS" => (0.0, 0.9, 0.0, 0.0),
        "OBSOLETE" => (0.0, 0.0, 0.9, 0.0),
        "RELATED_TO" => (0.0, 0.0, 0.0, 0.9),
        _ => (0.0, 0.0, 0.0, 0.0),
    };

    // Translate the label into the representation action.
    // REDUNDANT_WITH and OBSOLETE create a merged summary; the rest keep
    // evidence separate (Jev-Mem non-destructive invariant — contradiction
    // never merges).
    let representation = match label {
        "REDUNDANT_WITH" | "OBSOLETE" => "merge".to_string(),
        "CONTRADICTS" | "RELATED_TO" | "KEEP_SEPARATE" => "keep_separate".to_string(),
        _other => "keep_separate".to_string(),
    };

    let scores = json!({
        "redundant": redundant,
        "contradiction": contradiction,
        "obsolete": obsolete,
        "link": link,
    });

    let new_content = new_memory_row.get("content").and_then(|v| v.as_str()).unwrap_or("");
    let cand_content = candidate_row.get("content").and_then(|v| v.as_str()).unwrap_or("");
    let summary_content = match representation.as_str() {
        "merge" => format!("{} {}", new_content, cand_content),
        "promote" => format!("Repeated: {} | {}", new_content, cand_content),
        _ => String::new(),
    };

    let final_repr = if contradiction >= threshold
        && (representation == "merge" || representation == "promote")
    {
        "keep_separate".to_string()
    } else {
        representation.clone()
    };

    Ok((scores, final_repr, summary_content))
}

/// Continuous, non-destructive consolidation trigger. After a new memory is
/// written, finds the top-k most similar existing memories and runs the
/// `consolidation` DSL spec on each pair. Per-pair decisions are applied via
/// [`apply_consolidation`] with `audit_op = "auto_consolidation"`.
pub(crate) fn auto_consolidate(
    db_path: &Path,
    spec_dir: &Path,
    backend: &dyn Decide,
    new_memory_id: &str,
    top_k: usize,
    threshold: f64,
) -> Result<Vec<Value>> {
    let new_row = fetch_memory_row(db_path, new_memory_id)?;
    if new_row.is_null() {
        return Ok(Vec::new());
    }
    let new_content = new_row
        .get("content")
        .and_then(|v| v.as_str())
        .unwrap_or("");
    if new_content.trim().is_empty() {
        return Ok(Vec::new());
    }
    let candidates = find_similar_memories(db_path, new_content, new_memory_id, top_k)?;
    if candidates.is_empty() {
        return Ok(Vec::new());
    }

    let mut decisions: Vec<Value> = Vec::new();
    for (cand_id, _cand_content) in candidates {
        let cand_row = match fetch_memory_row(db_path, &cand_id) {
            Ok(r) if !r.is_null() => r,
            _ => continue,
        };
        let (scores, representation, summary_content) = match evaluate_consolidation(
            spec_dir, backend, &new_row, &cand_row, threshold,
        ) {
            Ok(t) => t,
            Err(e) => {
                decisions.push(json!({
                    "candidate_id": cand_id,
                    "error": format!("evaluate_consolidation: {e:#}"),
                }));
                continue;
            }
        };
        match apply_consolidation(
            db_path,
            new_memory_id,
            &cand_id,
            &scores,
            threshold,
            &representation,
            &summary_content,
            &json!({}),
            &Value::Array(Vec::new()),
            "auto_consolidation",
        ) {
            Ok(v) => decisions.push(v),
            Err(e) => decisions.push(json!({
                "candidate_id": cand_id,
                "error": format!("apply_consolidation: {e:#}"),
            })),
        }
    }
    Ok(decisions)
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
        for name in ["memory_type", "admission", "routing", "stopping", "consolidation"] {
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
        let _lock = crate::state::ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
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
        let _lock = crate::state::ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
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
