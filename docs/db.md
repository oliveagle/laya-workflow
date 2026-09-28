# `db` — SQLite (ACID) + DuckDB (analytics) over one file

`kind: "db"` is one wrapper over two engines that play to their strengths:

* **SQLite** — the *system of record*. Row-level writes, constraints, explicit
  `BEGIN IMMEDIATE … COMMIT` batches, one portable file.
* **DuckDB** — the *analytics engine*. Columnar scans, joins and aggregations —
  computed **directly over that same SQLite file** through DuckDB's `sqlite`
  extension (<https://github.com/duckdb/duckdb-sqlite>).

There is no ETL step and no second copy to keep in sync: DuckDB *attaches* the
live SQLite database as a schema and reads it in place. DuckDB can also **write
back** through the attachment (`INSERT … SELECT`, `CREATE OR REPLACE TABLE … AS
SELECT`), and that write is committed **by SQLite**, not by DuckDB — so
analytics results get an ACID landing zone for free.

```text
        ┌─────────── your workflow ────────────┐
        │  op: exec / query   op: analytics    │
        ▼                        ▼             │
   ┌─────────┐  attach+USE  ┌──────────┐        │
   │ SQLite  │◀────────────▶│  DuckDB  │        │
   │ (ACID)  │  same file   │(columnar)│        │
   └─────────┘              └──────────┘        │
        ▲                        │             │
        └────── op: sync ────────┘             │
              (write back)                     │
```

Both engines are driven through their **real CLIs** (`sqlite3`, `duckdb`), so
this capability needs `policy.allow_exec = true`, and both database files are
validated against `policy.allow_paths` before anything runs. No Rust driver
dependency.

## Fields

| field | default | meaning |
|-------|---------|---------|
| `sqlite` | — (required) | SQLite file — the ACID system of record |
| `duckdb` | `""` (in-memory) | optional DuckDB warehouse file; empty ⇒ analytics run in memory and only the attached SQLite file survives |
| `alias` | `sqlite` | schema alias the SQLite file gets inside DuckDB |
| `op` | `query` | default op (a node's `with.op` overrides it) |
| `readonly` | `true` | writes require `false` (`exec` / `sync`) |
| `format` | `json` | analytics output: `json` \| `csv` \| `md` |
| `timeout_ms` | `60000` | per-invocation timeout |

## Ops

| op | engine | behaviour |
|----|--------|-----------|
| `query` | SQLite | read-only `SELECT` from `with.sql` → `rows` |
| `exec` | SQLite | write; `with.statements: [...]` runs as **one** `BEGIN IMMEDIATE … COMMIT` transaction (`readonly: false`), or `with.sql` runs verbatim |
| `analytics` | DuckDB | `with.sql` with the SQLite file attached; `with.format` = `json` \| `csv` \| `md` |
| `sync` | DuckDB → SQLite | `with.sql` (a DuckDB `SELECT`) → `with.into` (SQLite table); `with.create: true` ⇒ `CREATE OR REPLACE TABLE`, else `INSERT INTO` |
| `tables` | both | list tables/views per engine |

`analytics` attaches the SQLite file and runs `USE <alias>`, so unqualified
table names resolve to the live SQLite data. Warehouse / `information_schema`
objects stay reachable by their catalog name.

Every result carries `{capability:"db", op, engine, sqlite, duckdb, alias,
readonly, ok, exit_code, rows, raw, stderr, sql}` — `rows` is the parsed JSON
array (so a workflow can route or assert on data), and `raw` is the untouched
CLI output (which is the CSV/Markdown text when `format` is `csv`/`md`).

## Read-only by default

A `db` capability is **read-only** unless it sets `readonly: false`, and even
then only `exec` / `sync` will write. Asking `exec` on a read-only capability is
a hard error:

```text
Error: db exec writes to SQLite; set capability readonly=false to enable writes
```

Define two capabilities over the same file — one reader, one writer — when a
workflow should read widely but write narrowly.

## Example

`dsl/capabilities/db_analytics.json` runs the whole loop: write rows into
SQLite, aggregate them with DuckDB, write the aggregate back as an ACID table,
read it back, and list what each engine sees.

```sh
export LAYA_WORK_DIR=/tmp/laya-db-demo      # fail-closed: required by the spec's allow_paths
mkdir -p "$LAYA_WORK_DIR"
laya-workflow run --spec dsl/capabilities/db_analytics.json
```

The equivalent `db` capability call:

```jsonc
{
  "capabilities": {
    "store_ro": { "kind": "db", "sqlite": "${env.LAYA_WORK_DIR}/shop.sqlite" },
    "store_rw": { "kind": "db", "sqlite": "${env.LAYA_WORK_DIR}/shop.sqlite", "readonly": false }
  },
  "nodes": [
    {
      "name": "ingest",
      "action": {
        "kind": "call",
        "capability": "store_rw",
        "with": {
          "op": "exec",
          "statements": [
            "CREATE TABLE IF NOT EXISTS sales (id INTEGER PRIMARY KEY, region TEXT NOT NULL, amount REAL NOT NULL)",
            "INSERT INTO sales (region, amount) VALUES ('east', 10.5), ('west', 20.0)"
          ]
        }
      }
    }
  ]
}
```

## What this is *not*

* Not a new query language — it is plain SQLite SQL and plain DuckDB SQL.
* Not a connection pool — each call is a short-lived CLI process, so batch your
  writes with `statements` (one transaction) rather than many tiny `exec` calls.
* Not a replacement for the `sqlite` capability: `kind: "sqlite"` is a bare
  one-engine file store; `kind: "db"` is the *two-engine* pairing.
