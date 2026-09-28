# db — an HTAP wrapper: SQLite (ACID) + DuckDB (analytics) over one file

`kind: "db"` is one capability over two engines that play to their strengths:
**SQLite** is the transactional system of record (`BEGIN IMMEDIATE … COMMIT`
batches, constraints, one file) and **DuckDB** is the analytics engine. DuckDB
attaches the *same live SQLite file* through its `sqlite` extension
(<https://github.com/duckdb/duckdb-sqlite>) — no ETL, no second copy — and can
write aggregates back into SQLite (`op: sync`), which SQLite commits. That pairing
is the **HTAP** story: OLTP in SQLite, OLAP in DuckDB, one file.

It ships in **two modes**, and both run the same ops against the same file.

## embed (default) — the workflow drives the local CLIs

```jsonc
{ "kind": "db", "sqlite": "${env.LAYA_WORK_DIR}/shop.sqlite" }                              // read-only
{ "kind": "db", "sqlite": "${env.LAYA_WORK_DIR}/shop.sqlite", "readonly": false }           // may write
```

Needs `policy.allow_exec = true` (it spawns `sqlite3` / `duckdb`) and both files
under `policy.allow_paths`. Demo: `dsl/capabilities/db_analytics.json`:

```sh
export LAYA_WORK_DIR=/tmp/laya-db-demo && mkdir -p "$LAYA_WORK_DIR"
laya-workflow run --spec dsl/capabilities/db_analytics.json
```

## server — one daemon owns the file; clients POST ops to it

```sh
laya-workflow db serve  --sqlite "$LAYA_WORK_DIR/shop.sqlite" --port 18767 --daemon  # start (own session)
laya-workflow db ensure --sqlite "$LAYA_WORK_DIR/shop.sqlite" --port 18767           # idempotent
laya-workflow db status --port 18767                                                 # RUNNING | STOPPED (exit 1)
laya-workflow db stop   --port 18767                                                 # SIGTERM + clean state
```

```jsonc
{ "kind": "db", "mode": "server", "endpoint": "http://127.0.0.1:18767" }
{ "kind": "db", "mode": "server", "endpoint": "http://127.0.0.1:18767", "readonly": false }
```

The daemon is a dependency-free HTTP/1.1 server; it binds localhost only, prints
`BASE=<url>`, and keeps its state in the same
`/tmp/laya-ensure-server-<port>.{pid,base,log}` files as `server ensure`.
`db ensure` refuses to attach to a daemon already serving a *different* file.
Because server mode speaks HTTP, the client capability needs **no**
`policy.allow_exec` and **no** `policy.allow_paths` — only the daemon does.
Demo: `dsl/capabilities/db_server_analytics.json`.

## Ops (identical in both modes)

| op | engine | behaviour |
|----|--------|-----------|
| `query` | SQLite | read-only `SELECT` from `with.sql` → `result.rows` |
| `exec` | SQLite | write; `with.statements: [...]` = one `BEGIN IMMEDIATE … COMMIT` tx (needs `readonly:false`) |
| `analytics` | DuckDB | `with.sql` over the attached SQLite file; `with.format` = `json` \| `csv` \| `md` |
| `sync` | DuckDB → SQLite | `with.sql` → `with.into`; `with.create:true` ⇒ `CREATE OR REPLACE`, else `INSERT` |
| `tables` | both | list tables/views per engine |

Defaults: `alias: sqlite`, `format: json`, `timeout_ms: 60000`, `readonly: true`.
Read-only is enforced per capability — `exec`/`sync` are refused unless
`readonly: false`, in both modes.

Next: `skill --section orchestrate` (the generic local-resource lifecycle), or
read `docs/db.md` for the full model.
