//! `db` — one wrapper over two engines that play to their strengths:
//!
//!   * **SQLite** — the ACID system of record: row-level writes, explicit
//!     `BEGIN IMMEDIATE … COMMIT` batches, constraints, one file.
//!   * **DuckDB** — the analytics engine: columnar scans, joins and
//!     aggregations run **directly over that same SQLite file** through DuckDB's
//!     `sqlite` extension (<https://github.com/duckdb/duckdb-sqlite>). No ETL
//!     step and no second copy to keep in sync: DuckDB attaches the live SQLite
//!     database as a schema.
//!
//! The DuckDB prelude attaches the SQLite file and `USE`s it as the **default**
//! catalog, so a workflow's plain SQL (`SELECT * FROM sales`) hits the live
//! SQLite data; warehouse objects stay reachable by their catalog name.
//!
//! DuckDB can also *write back* through the attachment (`INSERT … SELECT`,
//! `CREATE [OR REPLACE] TABLE … AS SELECT`), which gives analytics results an
//! ACID landing zone: the write is committed by SQLite, not by DuckDB.
//!
//! Ops (`with.op`, else the capability's own `op`):
//!
//! | op | engine | behaviour |
//! |----|--------|-----------|
//! | `query` | sqlite | read-only SELECT (`with.sql`) → `rows` |
//! | `exec` | sqlite | write; `with.statements: [...]` runs as **one** `BEGIN IMMEDIATE … COMMIT` transaction (`readonly=false`), `with.sql` runs verbatim |
//! | `analytics` | duckdb | `with.sql` with the SQLite file attached as `<alias>`; `with.format` = `json` \| `csv` \| `md` |
//! | `sync` | duckdb → sqlite | `with.sql` (DuckDB SELECT) → `with.into` (SQLite table); `with.create` switches INSERT to `CREATE OR REPLACE TABLE` |
//! | `tables` | both | list tables/views in each engine |
//!
//! Both engines are driven through their real CLIs (`sqlite3`, `duckdb`), so
//! this needs `policy.allow_exec`; the two database files are validated against
//! `policy.allow_paths` before anything runs. No Rust driver dependency.

use anyhow::{bail, Result};
use serde_json::{json, Value};

use super::store::resolve_store_path;
use super::{call_exec, expand, stringify, ExecCap, Policy};

#[derive(Clone, Debug, Default)]
pub struct DbCap {
    /// SQLite database file — the ACID system of record.
    pub sqlite: String,
    /// Optional DuckDB warehouse file. Empty ⇒ analytics run in memory and only
    /// the attached SQLite file survives.
    pub duckdb: String,
    /// Schema alias the SQLite file gets inside DuckDB (default `sqlite`).
    pub alias: String,
    /// Default op when `with.op` is absent (default `query`).
    pub op: String,
    /// Allow writes. Default true-for-reads means `exec`/`sync` require `false`.
    pub readonly: bool,
    /// Analytics output format: json | csv | md (default json).
    pub format: String,
    pub timeout_ms: u64,
}

pub fn call_db(c: &DbCap, with: &Value, state: &Value, policy: &Policy) -> Result<Value> {
    let op = with
        .get("op")
        .and_then(|v| v.as_str())
        .map(str::to_string)
        .unwrap_or_else(|| {
            if c.op.is_empty() {
                "query".to_string()
            } else {
                c.op.clone()
            }
        });

    let sqlite = field(&c.sqlite, with, state);
    if sqlite.is_empty() {
        bail!("db capability needs 'sqlite' (the SQLite file — the ACID system of record)");
    }
    let sqlite = resolve_store_path(&sqlite, policy)?.display().to_string();

    let warehouse_raw = field(&c.duckdb, with, state);
    let warehouse = if warehouse_raw.is_empty() {
        None
    } else {
        Some(
            resolve_store_path(&warehouse_raw, policy)?
                .display()
                .to_string(),
        )
    };

    let alias = {
        let a = field(&c.alias, with, state);
        if a.is_empty() {
            "sqlite".to_string()
        } else {
            ident(&a, "attach alias")?
        }
    };

    let readonly = c.readonly;
    let timeout = if c.timeout_ms == 0 {
        60_000
    } else {
        c.timeout_ms
    };

    match op.as_str() {
        "query" => {
            let sql = sql_only(with, state, "query")?;
            let out = sqlite_run(&sqlite, &sql, true, timeout, policy)?;
            Ok(db_result(
                "query",
                "sqlite",
                &sqlite,
                warehouse.as_deref(),
                &alias,
                true,
                out,
                &sql,
            ))
        }
        "exec" => {
            if readonly {
                bail!("db exec writes to SQLite; set capability readonly=false to enable writes");
            }
            let script = sqlite_exec_script(with, state)?;
            let out = sqlite_run(&sqlite, &script, false, timeout, policy)?;
            Ok(db_result(
                "exec",
                "sqlite",
                &sqlite,
                warehouse.as_deref(),
                &alias,
                false,
                out,
                &script,
            ))
        }
        "analytics" => {
            let sql = sql_only(with, state, "analytics")?;
            let format = with
                .get("format")
                .and_then(|v| v.as_str())
                .map(str::to_string)
                .filter(|s| !s.is_empty())
                .unwrap_or_else(|| c.format.clone());
            let script = format!("{}{}", duckdb_prelude(&sqlite, &alias, readonly), sql);
            let out = duckdb_run(
                warehouse.as_deref(),
                readonly,
                &format,
                &script,
                timeout,
                policy,
            )?;
            Ok(db_result(
                "analytics",
                "duckdb",
                &sqlite,
                warehouse.as_deref(),
                &alias,
                readonly,
                out,
                &sql,
            ))
        }
        "sync" => {
            if readonly {
                bail!(
                    "db sync writes back to SQLite; set capability readonly=false to enable writes"
                );
            }
            let select = sql_only(with, state, "sync")?;
            let into = ident(
                &field(
                    &with.get("into").map(stringify).unwrap_or_default(),
                    with,
                    state,
                ),
                "sync target 'into'",
            )?;
            let create = with
                .get("create")
                .and_then(|v| v.as_bool())
                .unwrap_or(false);
            let stmt = if create {
                format!("CREATE OR REPLACE TABLE {alias}.{into} AS SELECT * FROM ({select});")
            } else {
                format!("INSERT INTO {alias}.{into} SELECT * FROM ({select});")
            };
            let script = format!("{}{}", duckdb_prelude(&sqlite, &alias, false), stmt);
            let out = duckdb_run(
                warehouse.as_deref(),
                false,
                "json",
                &script,
                timeout,
                policy,
            )?;
            Ok(db_result(
                "sync",
                "duckdb",
                &sqlite,
                warehouse.as_deref(),
                &alias,
                false,
                out,
                &script,
            ))
        }
        "tables" => {
            let sqlite_sql = "SELECT name, type FROM sqlite_schema \
                 WHERE type IN ('table','view') AND name NOT LIKE 'sqlite_%' ORDER BY name";
            let duckdb_sql = format!(
                "{}SELECT table_schema AS schema, table_name AS name FROM information_schema.tables \
                 WHERE table_schema NOT IN ('information_schema','pg_catalog') ORDER BY 1,2",
                duckdb_prelude(&sqlite, &alias, true)
            );
            let lite = parse_rows(
                sqlite_run(&sqlite, sqlite_sql, true, timeout, policy)?["stdout"]
                    .as_str()
                    .unwrap_or(""),
            );
            let duck = parse_rows(
                duckdb_run(
                    warehouse.as_deref(),
                    true,
                    "json",
                    &duckdb_sql,
                    timeout,
                    policy,
                )?["stdout"]
                    .as_str()
                    .unwrap_or(""),
            );
            Ok(json!({
                "capability": "db", "op": "tables", "sqlite": sqlite,
                "duckdb": warehouse.unwrap_or_else(|| ":memory:".to_string()),
                "alias": alias, "rows": { "sqlite": lite, "duckdb": duck },
            }))
        }
        other => bail!("db op {other:?} unsupported (query | exec | analytics | sync | tables)"),
    }
}

// ── engine runners (both go through `exec`, so policy.allow_exec applies) ──

fn sqlite_run(
    db: &str,
    sql: &str,
    readonly: bool,
    timeout_ms: u64,
    policy: &Policy,
) -> Result<Value> {
    let mut argv = vec!["sqlite3".to_string()];
    if readonly {
        argv.push("-readonly".to_string());
    }
    argv.push("-json".to_string());
    argv.push("-bail".to_string());
    argv.push(db.to_string());
    argv.push(sql.to_string());
    run(argv, timeout_ms, policy)
}

fn duckdb_run(
    warehouse: Option<&str>,
    readonly: bool,
    format: &str,
    script: &str,
    timeout_ms: u64,
    policy: &Policy,
) -> Result<Value> {
    let mut argv = vec![
        "duckdb".to_string(),
        duckdb_format_flag(format)?.to_string(),
    ];
    // `-bail` makes the CLI stop at the first error and return non-zero.
    argv.push("-bail".to_string());
    if readonly && warehouse.is_some() {
        argv.push("-readonly".to_string());
    }
    if let Some(w) = warehouse {
        argv.push(w.to_string());
    }
    argv.push("-c".to_string());
    argv.push(script.to_string());
    run(argv, timeout_ms, policy)
}

fn run(argv: Vec<String>, timeout_ms: u64, policy: &Policy) -> Result<Value> {
    let cap = ExecCap {
        argv,
        cwd: None,
        env: serde_json::Map::new(),
        timeout_ms,
        max_output: policy.max_output,
    };
    call_exec(&cap, &json!({}), &json!({}), policy)
}

// ── SQL assembly (pure; unit-tested) ──

/// Attach the SQLite file inside DuckDB and make it the **default** catalog, so
/// unqualified table names in a workflow's SQL resolve to the live SQLite data
/// (e.g. `SELECT * FROM sales`). Warehouse/`information_schema` objects stay
/// reachable by their catalog name.
fn duckdb_prelude(sqlite: &str, alias: &str, readonly: bool) -> String {
    let path = sqlite.replace('\'', "''");
    let mode = if readonly { ", READ_ONLY" } else { "" };
    format!(
        "INSTALL sqlite;\nLOAD sqlite;\nATTACH '{path}' AS {alias} (TYPE SQLITE{mode});\nUSE {alias};\n"
    )
}

fn duckdb_format_flag(format: &str) -> Result<&'static str> {
    Ok(match format {
        "" | "json" => "-json",
        "csv" => "-csv",
        "md" | "markdown" => "-markdown",
        other => bail!("db analytics format {other:?} unsupported (json | csv | md)"),
    })
}

/// `statements: [...]` → one explicit SQLite transaction. Each statement is
/// wrapped by `BEGIN IMMEDIATE … COMMIT`, so a mid-batch error rolls the whole
/// batch back (the CLI exits non-zero via `-bail` and never reaches COMMIT).
fn sqlite_tx(statements: &[String]) -> String {
    let mut s = String::from("BEGIN IMMEDIATE;\n");
    for st in statements {
        s.push_str(st.trim().trim_end_matches(';'));
        s.push_str(";\n");
    }
    s.push_str("COMMIT;\n");
    s
}

fn db_result(
    op: &str,
    engine: &str,
    sqlite: &str,
    warehouse: Option<&str>,
    alias: &str,
    readonly: bool,
    out: Value,
    sql: &str,
) -> Value {
    let stdout = out["stdout"].as_str().unwrap_or("");
    json!({
        "capability": "db", "op": op, "engine": engine,
        "sqlite": sqlite, "duckdb": warehouse.unwrap_or(":memory:"), "alias": alias,
        "readonly": readonly, "ok": out["ok"], "exit_code": out["exit_code"],
        "rows": parse_rows(stdout), "raw": stdout, "stderr": out["stderr"], "sql": sql,
    })
}

/// `sqlite3 -json` and `duckdb -json` both emit a JSON **array** (pretty-printed
/// across several lines); several statements produce several such arrays. Newer
/// or streaming modes may instead emit line-delimited objects, so accept both.
/// Anything that is not JSON at all (csv / markdown output) is surfaced as
/// `null` rather than a bogus value.
fn parse_rows(stdout: &str) -> Value {
    let t = stdout.trim();
    if t.is_empty() {
        return json!([]);
    }
    if let Ok(v) = serde_json::from_str::<Value>(t) {
        if v.is_array() {
            return v;
        }
    }
    let mut rows = Vec::new();
    for line in t.lines() {
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        match serde_json::from_str::<Value>(line) {
            Ok(v) => rows.push(v),
            Err(_) => return Value::Null,
        }
    }
    json!(rows)
}

// ── config expansion helpers ──

fn field(raw: &str, with: &Value, state: &Value) -> String {
    if raw.is_empty() {
        return String::new();
    }
    stringify(&expand(&Value::String(raw.to_string()), state, with))
}

fn sql_only(with: &Value, state: &Value, op: &str) -> Result<String> {
    match with.get("sql") {
        Some(v) => Ok(stringify(&expand(v, state, with))),
        None => bail!("db {op} needs 'sql' in 'with'"),
    }
}

fn sqlite_exec_script(with: &Value, state: &Value) -> Result<String> {
    if let Some(stmts) = with.get("statements").and_then(|v| v.as_array()) {
        let list: Vec<String> = stmts
            .iter()
            .map(|s| stringify(&expand(s, state, with)))
            .filter(|s| !s.trim().is_empty())
            .collect();
        if list.is_empty() {
            bail!("db exec: 'statements' is empty");
        }
        return Ok(sqlite_tx(&list));
    }
    match with.get("sql") {
        Some(v) => Ok(stringify(&expand(v, state, with))),
        None => bail!("db exec needs 'sql' or 'statements' in 'with'"),
    }
}

fn ident(s: &str, what: &str) -> Result<String> {
    let first = s.chars().next();
    let ok = matches!(first, Some(c) if c.is_ascii_alphabetic() || c == '_')
        && s.chars().all(|c| c.is_ascii_alphanumeric() || c == '_');
    if !ok {
        bail!("db {what} {s:?} must be a simple identifier ([A-Za-z_][A-Za-z0-9_]*)");
    }
    Ok(s.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn prelude_attaches_sqlite_with_readonly_option() {
        let p = duckdb_prelude("/tmp/a.sqlite", "sqlite", true);
        assert!(p.contains("INSTALL sqlite;"));
        assert!(p.contains("LOAD sqlite;"));
        assert!(p.contains("ATTACH '/tmp/a.sqlite' AS sqlite (TYPE SQLITE, READ_ONLY);"));
        assert!(p.contains("USE sqlite;"));
        let w = duckdb_prelude("/tmp/a.sqlite", "db", false);
        assert!(w.contains("ATTACH '/tmp/a.sqlite' AS db (TYPE SQLITE);"));
        assert!(w.contains("USE db;"));
    }

    #[test]
    fn prelude_escapes_single_quotes_in_path() {
        let p = duckdb_prelude("/tmp/o'brien/x.sqlite", "sqlite", false);
        assert!(p.contains("'/tmp/o''brien/x.sqlite'"));
    }

    #[test]
    fn format_flag_maps_known_formats() {
        assert_eq!(duckdb_format_flag("").unwrap(), "-json");
        assert_eq!(duckdb_format_flag("json").unwrap(), "-json");
        assert_eq!(duckdb_format_flag("csv").unwrap(), "-csv");
        assert_eq!(duckdb_format_flag("md").unwrap(), "-markdown");
        assert_eq!(duckdb_format_flag("markdown").unwrap(), "-markdown");
        assert!(duckdb_format_flag("parquet").is_err());
    }

    #[test]
    fn statements_become_one_transaction() {
        let tx = sqlite_tx(&[
            "INSERT INTO t VALUES (1)".to_string(),
            "  INSERT INTO t VALUES (2);  ".to_string(),
        ]);
        assert_eq!(
            tx,
            "BEGIN IMMEDIATE;\nINSERT INTO t VALUES (1);\nINSERT INTO t VALUES (2);\nCOMMIT;\n"
        );
    }

    #[test]
    fn empty_output_parses_as_empty_array() {
        assert_eq!(parse_rows(""), json!([]));
        assert_eq!(parse_rows("  \n "), json!([]));
    }

    #[test]
    fn json_arrays_and_ndjson_both_parse() {
        // `sqlite3 -json` / `duckdb -json` emit a JSON array, pretty-printed
        // across lines.
        assert_eq!(parse_rows("[{\"a\":1}]"), json!([{"a": 1}]));
        assert_eq!(
            parse_rows("[{\"a\":1},\n{\"a\":2}]"),
            json!([{"a": 1}, {"a": 2}])
        );
        // Streaming/NDJSON shape: one object per line.
        assert_eq!(
            parse_rows("{\"a\":1}\n{\"a\":2}"),
            json!([{"a": 1}, {"a": 2}])
        );
        // non-JSON output (csv/markdown) is surfaced as null, not a bogus value.
        assert_eq!(parse_rows("a,b\n1,2"), Value::Null);
    }

    #[test]
    fn identifiers_are_validated() {
        assert_eq!(ident("items", "table").unwrap(), "items");
        assert_eq!(ident("_x9", "table").unwrap(), "_x9");
        assert!(ident("", "table").is_err());
        assert!(ident("9x", "table").is_err());
        assert!(ident("a.b", "table").is_err());
        assert!(ident("a; DROP TABLE t", "table").is_err());
    }
}
