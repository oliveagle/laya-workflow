//! SQLite plumbing + schema probing + view SQL strings for the codegraph
//! tools. Split out of `codegraph.rs` to keep every source file under the
//! 1000-line AGENTS.md limit.

use std::path::{Path, PathBuf};

use anyhow::{anyhow, bail, Context, Result};
use serde_json::{json, Value};

use crate::workflow::Decide;
use crate::capability::db::{call_db, DbCap};
use crate::capability::Policy;
use crate::codegraph::EMBEDDED_SPECS;
use crate::spec::load_file;
use crate::workflow::ResilientWorkflow;

// ─── escape + helpers ────────────────────────────────────────────────────────

/// SQLite identifier safety: codegraph DB identifiers come from the indexer
/// (table/column names, not user input), so we use them verbatim. We *do*
/// validate user-supplied query/key strings are not catastrophic before
/// interpolating them into SQL: a single-quote `'` in a user string would
/// otherwise break out of the literal. The escape doubles single quotes per
/// SQLite conventions (`'foo''bar'` → literal `foo'bar`).
pub fn sql_str(s: &str) -> String {
    let mut out = String::with_capacity(s.len() + 2);
    out.push('\'');
    for c in s.chars() {
        if c == '\'' {
            out.push_str("''");
        } else {
            out.push(c);
        }
    }
    out.push('\'');
    out
}

/// LIKE wildcards (`%` / `_`) in user-supplied keys would let them match
/// arbitrary content; codegraph callers don't want that — a key should match
/// a real name fragment only. Escape `%`, `_`, and the escape character `\`.
pub fn sql_like_escape(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        match c {
            '\\' | '%' | '_' => { out.push('\\'); out.push(c); }
            _ => out.push(c),
        }
    }
    out
}

// ─── schema probing ──────────────────────────────────────────────────────────

/// Tables the codegraph tool set requires.
pub const REQUIRED_TABLES: &[&str] = &["nodes", "edges", "files", "nodes_fts"];

/// Run a JSON `SELECT` against `db_path` via the local `sqlite3` CLI.
///
/// Inputs to this helper are static SQL strings built by the view adapters
/// below; user data is always pre-escaped through [`sql_str`] and
/// [`sql_like_escape`], so no further sanitisation happens.
pub fn sql_query_json(db_path: &Path, sql: &str) -> Result<Value> {
    let cap = DbCap {
        sqlite: db_path.display().to_string(),
        duckdb: String::new(),
        alias: "sqlite".to_string(),
        op: String::new(),
        readonly: true,
        format: "json".to_string(),
        mode: "embed".to_string(),
        endpoint: String::new(),
        timeout_ms: 30_000,
    };
    let pol = Policy {
        allow_exec: true,
        allow_paths: vec![db_path.display().to_string()],
        ..Default::default()
    };
    call_db(&cap, &json!({ "op": "query", "sql": sql }), &json!({}), &pol)
        .with_context(|| format!("sqlite3 query failed: {}", sql))
}

/// Look up the union of `(name, type)` from `sqlite_schema` to decide whether
/// the required codegraph tables are present. Returns a sorted set of names.
pub fn list_tables(db_path: &Path) -> Result<Vec<String>> {
    let v = sql_query_json(db_path,
        "SELECT name FROM sqlite_schema WHERE type IN ('table','view') \
         AND name NOT LIKE 'sqlite_%' ORDER BY name",
    )?;
    Ok(v.get("rows")
        .and_then(|r| r.as_array())
        .map(|arr| arr.iter().filter_map(|x| x.get("name").and_then(|n| n.as_str()).map(String::from)).collect())
        .unwrap_or_default())
}

pub fn db_ready(db_path: &Path) -> Result<bool> {
    let tables = list_tables(db_path)?;
    Ok(REQUIRED_TABLES.iter().all(|t| tables.iter().any(|x| x == t)))
}

pub fn db_stats(db_path: &Path) -> Result<Value> {
    // Counts + index info for an at-a-glance sanity probe.
    let sql = "\
        SELECT (SELECT COUNT(*) FROM nodes) AS nodes, \
               (SELECT COUNT(*) FROM edges) AS edges, \
               (SELECT COUNT(*) FROM files) AS files, \
               (SELECT COUNT(*) FROM unresolved_refs) AS unresolved_refs";
    let v = sql_query_json(db_path, sql)?;
    let row = v.get("rows")
        .and_then(|r| r.as_array())
        .and_then(|a| a.first())
        .cloned()
        .unwrap_or(Value::Null);
    let edge_kinds = sql_query_json(db_path,
        "SELECT kind, COUNT(*) AS n FROM edges GROUP BY kind ORDER BY n DESC"
    )?;
    let node_kinds = sql_query_json(db_path,
        "SELECT kind, COUNT(*) AS n FROM nodes GROUP BY kind ORDER BY n DESC"
    )?;
    Ok(json!({
        "counts": row,
        "edge_kinds": edge_kinds.get("rows").cloned().unwrap_or(Value::Null),
        "node_kinds": node_kinds.get("rows").cloned().unwrap_or(Value::Null),
        "db_path": db_path.display().to_string(),
    }))
}

fn rows_of(v: &Value) -> Vec<Value> {
    v.get("rows").and_then(|r| r.as_array()).cloned().unwrap_or_default()
}

fn pick(row: &Value, k: &str) -> Value {
    row.get(k).cloned().unwrap_or(Value::Null)
}

fn row_summary(row: &Value) -> Value {
    json!({
        "id":                  pick(row, "id"),
        "kind":                pick(row, "kind"),
        "name":                pick(row, "name"),
        "qualified_name":      pick(row, "qualified_name"),
        "file_path":           pick(row, "file_path"),
        "language":            pick(row, "language"),
        "start_line":          pick(row, "start_line"),
        "end_line":            pick(row, "end_line"),
        "signature":           pick(row, "signature"),
        "docstring":           pick(row, "docstring"),
        "score":               pick(row, "score"),
        "edge_kind":           pick(row, "edge_kind"),
        "edge_source":         pick(row, "edge_source"),
        "edge_target":         pick(row, "edge_target"),
        "other_id":            pick(row, "other_id"),
        "other_kind":          pick(row, "other_kind"),
        "other_qualified_name":pick(row, "other_qualified_name"),
        "depth":               pick(row, "depth"),
    })
}

// ─── view SQL adapters ───────────────────────────────────────────────────────

/// Semantic view: FTS5 over name / qualified_name / docstring / signature.
/// Falls back to a LIKE search when `nodes_fts` is unavailable.
pub fn view_semantic(db_path: &Path, query: &str, limit: i64) -> Result<Value> {
    // FTS5 contentless table over the nodes table: the hidden columns are
    // (id, name, qualified_name, docstring, signature); rowid ties back to
    // nodes.rowid. `bm25` needs the MATCH cursor, and the score is only
    // meaningful on the filtered row set, so we query the FTS table first.
    let q = sql_str(query);
    let sql = format!(
        "SELECT nodes.id, nodes.kind, nodes.qualified_name, nodes.file_path, \
         nodes.language, nodes.start_line, nodes.end_line, \
         nodes.signature, nodes.docstring, bm25(nodes_fts) AS score \
         FROM nodes_fts JOIN nodes ON nodes.rowid = nodes_fts.rowid \
         WHERE nodes_fts MATCH {q} \
         ORDER BY score LIMIT {limit}",
    );
    let raw = sql_query_json(db_path, &sql);
    match raw {
        Ok(v) => Ok(json!({
            "view": "semantic",
            "query": query,
            "rows": rows_of(&v).into_iter().map(|r| row_summary(&r)).collect::<Vec<_>>(),
            "db_path": db_path.display().to_string(),
            "engine": "fts5",
        })),
        Err(_) => {
            let like = sql_str(&format!("%{}%", sql_like_escape(query)));
            let fb = format!(
                "SELECT id, kind, qualified_name, file_path, language, start_line, end_line, \
                 signature, docstring, 0.0 AS score \
                 FROM nodes \
                 WHERE name LIKE {like} ESCAPE '\\' OR qualified_name LIKE {like} ESCAPE '\\' OR docstring LIKE {like} ESCAPE '\\' \
                 ORDER BY qualified_name LIMIT {limit}",
            );
            let v = sql_query_json(db_path, &fb)?;
            Ok(json!({
                "view": "semantic",
                "query": query,
                "rows": rows_of(&v).into_iter().map(|r| row_summary(&r)).collect::<Vec<_>>(),
                "db_path": db_path.display().to_string(),
                "engine": "like",
            }))
        }
    }
}

/// Causal view: edges by kind (calls / extends / implements / instantiates /
/// references) starting from `seed` (matched against `qualified_name` /
/// `name`).
pub fn view_causal(db_path: &Path, seed: &str, inbound: bool, limit: i64) -> Result<Value> {
    let like = sql_str(&format!("%{}%", sql_like_escape(seed)));
    // Direction: inbound => edges pointing TO seed (others call seed); outbound => edges FROM seed (seed calls others).
    // The other-side endpoint (the caller for inbound, the callee for outbound) is joined back so callers / callees can be named in the row.
    let (join_clause, other_id_expr) = if inbound {
        ("e.target = n.id", "e.source")
    } else {
        ("e.source = n.id", "e.target")
    };
    let sql = format!(
        "SELECT e.kind AS edge_kind, e.source AS edge_source, e.target AS edge_target, \
         e.line, n.id, n.kind, n.qualified_name, n.file_path, n.start_line, n.signature, \
         other.id AS other_id, other.kind AS other_kind, other.qualified_name AS other_qualified_name \
         FROM nodes n \
         JOIN edges e ON ({join_clause}) \
         LEFT JOIN nodes other ON other.id = {other_id_expr} \
         WHERE n.qualified_name LIKE {like} ESCAPE '\\' OR n.name LIKE {like} ESCAPE '\\' \
         ORDER BY e.kind, other.qualified_name LIMIT {limit}",
    );
    let raw = sql_query_json(db_path, &sql)?;
    Ok(json!({
        "view": "causal",
        "seed": seed,
        "rows": rows_of(&raw).into_iter().map(|r| row_summary(&r)).collect::<Vec<_>>(),
        "db_path": db_path.display().to_string(),
    }))
}

/// Entity view: exact `qualified_name` lookup, falling back to `unresolved_refs`
/// candidate expansion.
pub fn view_entity(db_path: &Path, key: &str) -> Result<Value> {
    let exact = sql_str(key);
    let sql = format!(
        "SELECT id, kind, qualified_name, file_path, language, start_line, end_line, \
         signature, docstring \
         FROM nodes WHERE qualified_name = {exact} OR name = {exact} LIMIT 5",
    );
    let raw = sql_query_json(db_path, &sql)?;
    let rows = rows_of(&raw);
    if rows.is_empty() {
        let fb = sql_str(key);
        let soft = format!(
            "SELECT candidates, file_path, language, reference_kind \
             FROM unresolved_refs WHERE reference_name = {fb} LIMIT 5",
        );
        let v = sql_query_json(db_path, &soft)?;
        Ok(json!({
            "view": "entity",
            "key": key,
            "rows": rows_of(&v).into_iter().map(|r| json!({
                "soft_match": true,
                "candidates": r.get("candidates"),
                "file_path": r.get("file_path"),
                "language": r.get("language"),
                "edge_kind": r.get("reference_kind"),
            })).collect::<Vec<_>>(),
            "db_path": db_path.display().to_string(),
        }))
    } else {
        Ok(json!({
            "view": "entity",
            "key": key,
            "rows": rows.into_iter().map(|r| row_summary(&r)).collect::<Vec<_>>(),
            "db_path": db_path.display().to_string(),
        }))
    }
}

/// Temporal view: files ordered by `modified_at`, with their node counts.
pub fn view_temporal(db_path: &Path, limit: i64) -> Result<Value> {
    let sql = format!(
        "SELECT path, modified_at, indexed_at, node_count, language \
         FROM files ORDER BY modified_at DESC LIMIT {limit}",
    );
    let raw = sql_query_json(db_path, &sql)?;
    Ok(json!({
        "view": "temporal",
        "rows": rows_of(&raw),
        "db_path": db_path.display().to_string(),
    }))
}

/// Multi-hop view: recursive CTE walking the edge graph from `seed` to depth
/// `depth`.
pub fn view_multi_hop(db_path: &Path, seed: &str, depth: i64, limit: i64) -> Result<Value> {
    let like = sql_str(&format!("%{}%", sql_like_escape(seed)));
    let sql = format!(
        "WITH RECURSIVE walk(seed, id, depth) AS ( \
            SELECT id AS seed, id, 0 FROM nodes WHERE qualified_name LIKE {like} ESCAPE '\\' OR name LIKE {like} ESCAPE '\\' \
            UNION ALL \
            SELECT walk.seed, e.target, walk.depth + 1 \
            FROM walk JOIN edges e ON e.source = walk.id \
            WHERE walk.depth < {depth} \
        ) \
        SELECT n.id, n.kind, n.qualified_name, n.file_path, n.start_line, n.signature, \
               walk.depth AS depth, walk.seed AS edge_source \
        FROM walk JOIN nodes n ON n.id = walk.id \
        ORDER BY walk.depth, n.qualified_name LIMIT {limit}",
    );
    let raw = sql_query_json(db_path, &sql)?;
    Ok(json!({
        "view": "multi_hop",
        "seed": seed,
        "depth": depth,
        "rows": rows_of(&raw).into_iter().map(|r| row_summary(&r)).collect::<Vec<_>>(),
        "db_path": db_path.display().to_string(),
    }))
}

/// Largest-remainder allocation: turn six 0..1 view scores into integer
/// per-view budgets summing to `total`.
pub fn allocate_budgets(scores: &[f64; 6], total: i64) -> [i64; 6] {
    if total <= 0 {
        return [0; 6];
    }
    let s: f64 = scores.iter().copied().sum();
    if s <= 0.0 {
        return [0; 6];
    }
    let exact: Vec<f64> = scores.iter().map(|x| x * (total as f64) / s).collect();
    let floors: Vec<i64> = exact.iter().map(|x| x.floor() as i64).collect();
    let mut out = [0i64; 6];
    for i in 0..6 { out[i] = floors[i]; }
    let mut remaining = total - out.iter().sum::<i64>();
    let mut idxs: Vec<usize> = (0..6).collect();
    idxs.sort_by(|&a, &b| (exact[b] - exact[b].floor()).partial_cmp(&(exact[a] - exact[a].floor())).unwrap_or(std::cmp::Ordering::Equal));
    for i in idxs {
        if remaining <= 0 { break; }
        out[i] += 1;
        remaining -= 1;
    }
    out
}

// ─── spec runner (mirrors laya_mem_util) ──────────────────────────────────────

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

pub fn default_spec_dir() -> PathBuf {
    if let Some(dir) = crate::state::codegraph_spec_dir() {
        if ensure_specs(&dir).is_ok() {
            return dir;
        }
    }
    let bundled = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("dsl/codegraph");
    if bundled.is_dir() {
        return bundled;
    }
    PathBuf::from("dsl/codegraph")
}

/// The codegraph SQLite DB path, in preference order:
/// 1. `$LAYA_CODEGRAPH_DB` (explicit user override)
/// 2. `<state>/codegraph/codegraph.sqlite` (installed state root)
/// 3. `~/.agents/.codegraph/codegraph.db` (existing codegraph daemon output)
/// 4. `<state>/codegraph/codegraph.sqlite` (fallback)
pub fn default_db_path() -> PathBuf {
    if let Some(p) = std::env::var_os("LAYA_CODEGRAPH_DB").filter(|v| !v.is_empty()) {
        return PathBuf::from(p);
    }
    let state_path = crate::state::codegraph_db();
    if state_path.is_file() {
        return state_path;
    }
    let daemon_path = std::env::var_os("HOME")
        .filter(|v| !v.is_empty())
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("/"))
        .join(".agents/.codegraph/codegraph.db");
    if daemon_path.is_file() {
        return daemon_path;
    }
    state_path
}
