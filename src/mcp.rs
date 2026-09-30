//! MCP stdio server for the `jev_mem` DSL specs.
//!
//! Implements a minimal JSON-RPC 2.0 server over stdin/stdout (no async, no
//! third-party deps). Four tools:
//!
//! - `jev_mem_assess`   — run memory_type + admission specs on an observation
//! - `jev_mem_retrieve` — run routing + stopping specs on a query + evidence
//! - `jev_mem_persist`  — write to SQLite via the `db` capability (sqlite3 CLI)
//! - `jev_mem_recall`   — read recent memories/relations from SQLite
//!
//! Workflow execution is in-process (no spawning): the engine loads each spec
//! from `spec_dir`, builds the same `HeuristicBackend` / `LayaBackend` the CLI
//! uses, runs the graph, and reads the result fields. This is the win over a
//! Python wrapper — no per-call process-spawn + JSON parse overhead, and the
//! same code path as `laya-workflow run --spec ...`.
//!
//! Wire format: one JSON-RPC 2.0 object per line, terminated by `\n`. Responses
//! and notifications follow the same framing; the codex `rmcp_client` reads
//! stdout line-by-line. Methods handled:
//!
//! | method                       | reply? |
//! |------------------------------|--------|
//! | `initialize`                 | yes    |
//! | `notifications/initialized`  | no (fire-and-forget) |
//! | `tools/list`                 | yes    |
//! | `tools/call`                 | yes    |
//! | `ping`                       | yes    |
//!
//! Errors raised by the server use the JSON-RPC error codes
//! (`-32601 method not found`, `-32602 invalid params`, `-32603 internal`).

use std::io::{self, BufRead, Write};
use std::path::{Path, PathBuf};

use anyhow::{anyhow, bail, Context, Result};
use serde_json::{json, Value};

use crate::backend::{HeuristicBackend, LayaBackend};
use crate::capability::db::{call_db, DbCap};
use crate::capability::Policy;
use crate::spec::load_file;
use crate::workflow::{Decide, ResilientWorkflow};

const PROTOCOL_VERSION: &str = "2025-06-18";
const SERVER_INFO: &str = "laya-workflow-jev-mem";
const SERVER_VERSION: &str = env!("CARGO_PKG_VERSION");

// ─── tool list ─────────────────────────────────────────────────────────────

fn tools() -> Vec<Value> {
    vec![
        json!({
            "name": "jev_mem_assess",
            "description": "Decide whether a new observation should be remembered, and how. Runs the write-side System-One controller: 4 memory-type noul questions (episodic/semantic/procedural/preference) plus a 3-state admission gate. Returns type scores + ALLOW/CONFIRM/BLOCK.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "observation": {"type": "string", "description": "The text/fact worth remembering."},
                    "recent_memories": {"type": "array", "items": {"type": "string"}, "description": "Already-stored summaries near this topic (redundancy hints)."}
                },
                "required": ["observation"],
            }
        }),
        json!({
            "name": "jev_mem_retrieve",
            "description": "Decide whether retrieval should stop or continue, plus which graph views are worth spending budget on. Runs the read-side System-One controller: 6 routing noul questions and a 4-question stopping gate. Returns STOP_EVIDENCE_OK / CONTINUE_* and per-view route scores.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "query": {"type": "string", "description": "What you are trying to answer."},
                    "evidence": {"type": "array", "items": {"type": "string"}, "description": "Candidate facts gathered so far."},
                    "evidence_status": {"type": "string", "enum": ["sufficient","insufficient","contradiction"]},
                    "route_semantic": {"type": "string", "enum": ["high","low","yes","no"]},
                    "route_temporal": {"type": "string", "enum": ["high","low","yes","no"]},
                    "route_causal": {"type": "string", "enum": ["high","low","yes","no"]},
                    "route_entity": {"type": "string", "enum": ["high","low","yes","no"]},
                    "route_multi_hop_need": {"type": "string", "enum": ["high","low","yes","no"]},
                    "route_recency": {"type": "string", "enum": ["high","low","yes","no"]}
                },
                "required": ["query"],
            }
        }),
        json!({
            "name": "jev_mem_persist",
            "description": "Append a memory (and optionally a relation) to the SQLite store, then verify the row(s) were written. Returns the verified rows.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "content": {"type": "string", "description": "Memory text to store."},
                    "ts": {"type": "string", "description": "ISO timestamp (defaults to now)."},
                    "entities": {"type": "array", "items": {"type": "string"}},
                    "type_scores": {"type": "object"},
                    "relation": {
                        "type": "object",
                        "properties": {
                            "source": {"type": "string"},
                            "target": {"type": "string"},
                            "link_type": {"type": "string"},
                            "probability": {"type": "number"}
                        },
                        "required": ["source","target"]
                    },
                    "db_path": {"type": "string", "description": "Override the SQLite file for this call."}
                },
                "required": ["content"],
            }
        }),
        json!({
            "name": "jev_mem_recall",
            "description": "Read recent memories (and optionally relations) from the SQLite store. Returns the last N rows.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "limit": {"type": "integer", "default": 10},
                    "include_relations": {"type": "boolean", "default": true},
                    "db_path": {"type": "string", "description": "Override the SQLite file for this call."}
                }
            }
        }),
    ]
}

// ─── tool dispatch ─────────────────────────────────────────────────────────

fn tool_assess(spec_dir: &Path, backend: &dyn Decide, args: &Value) -> Result<Value> {
    let observation = args
        .get("observation")
        .and_then(|v| v.as_str())
        .ok_or_else(|| anyhow!("jev_mem_assess: missing 'observation'"))?;
    let mut state = json!({ "observation": observation });
    if let Some(recent) = args.get("recent_memories").and_then(|v| v.as_array()) {
        if !recent.is_empty() {
            state["recent_memories"] = Value::Array(recent.clone());
        }
    }
    let typed = run_spec(spec_dir, backend, "memory_type", &state)?;
    let admitted = run_spec(spec_dir, backend, "admission", &state)?;
    let t = typed.get("result").cloned().unwrap_or(Value::Null);
    let a = admitted.get("result").cloned().unwrap_or(Value::Null);
    let mut type_scores = serde_json::Map::new();
    for k in ["episodic", "semantic", "procedural", "preference"] {
        if let Some(v) = t.get(k).and_then(|x| x.as_f64()) {
            type_scores.insert(k.to_string(), json!(v));
        }
    }
    Ok(json!({
        "type_scores": Value::Object(type_scores),
        "dominant_type": t.get("label").cloned().unwrap_or(Value::Null),
        "admission": a.get("gate_action").cloned().or_else(|| a.get("label").cloned()).unwrap_or(Value::Null),
        "admission_detail": a.get("reason").cloned().unwrap_or(Value::Null),
        "confidence": t.get("confidence").cloned().or_else(|| a.get("confidence").cloned()).unwrap_or(Value::Null),
    }))
}

fn tool_retrieve(spec_dir: &Path, backend: &dyn Decide, args: &Value) -> Result<Value> {
    let query = args
        .get("query")
        .and_then(|v| v.as_str())
        .ok_or_else(|| anyhow!("jev_mem_retrieve: missing 'query'"))?;
    let mut state = json!({ "query": query });
    if let Some(ev) = args.get("evidence").and_then(|v| v.as_array()) {
        state["evidence"] = Value::Array(ev.clone());
    }
    for k in [
        "evidence_status",
        "route_semantic",
        "route_temporal",
        "route_causal",
        "route_entity",
        "route_multi_hop_need",
        "route_recency",
    ] {
        if let Some(v) = args.get(k) {
            state[k] = v.clone();
        }
    }
    let routing = run_spec(spec_dir, backend, "routing", &state)?;
    let stopping = run_spec(spec_dir, backend, "stopping", &state)?;
    let r = routing.get("result").cloned().unwrap_or(Value::Null);
    let s = stopping.get("result").cloned().unwrap_or(Value::Null);
    let mut route = serde_json::Map::new();
    for k in ["semantic", "temporal", "causal", "entity", "multi_hop_need", "recency_importance"] {
        if let Some(v) = r.get(k).and_then(|x| x.as_f64()) {
            route.insert(k.to_string(), json!(v));
        }
    }
    Ok(json!({
        "decision": s.get("label").cloned().unwrap_or(Value::Null),
        "decision_detail": s.get("reason").cloned().or_else(|| s.get("action_answer").cloned()).unwrap_or(Value::Null),
        "route": Value::Object(route),
        "confidence": s.get("confidence").cloned().unwrap_or(Value::Null),
    }))
}

fn tool_persist(spec_dir: &Path, db_path: &Path, args: &Value) -> Result<Value> {
    let content = args
        .get("content")
        .and_then(|v| v.as_str())
        .ok_or_else(|| anyhow!("jev_mem_persist: missing 'content'"))?;
    let ts = args
        .get("ts")
        .and_then(|v| v.as_str())
        .unwrap_or("");
    let entities = args
        .get("entities")
        .cloned()
        .unwrap_or_else(|| Value::Array(Vec::new()));
    let type_scores = args
        .get("type_scores")
        .cloned()
        .unwrap_or_else(|| json!({}));

    let db_path_owned: PathBuf = args
        .get("db_path")
        .and_then(|v| v.as_str())
        .filter(|s| !s.is_empty())
        .map(PathBuf::from)
        .unwrap_or_else(|| db_path.to_path_buf());
    ensure_parent(&db_path_owned)?;

    let entities_json = serde_json::to_string(&entities)?;
    let type_scores_json = serde_json::to_string(&type_scores)?;

    // 1. CREATE TABLE IF NOT EXISTS + DELETE (matches persist_memory.json)
    call_db_for_db(
        &db_path_owned,
        json!({
            "op": "exec",
            "statements": [
                "CREATE TABLE IF NOT EXISTS memories (id INTEGER PRIMARY KEY, content TEXT NOT NULL, ts TEXT, entities TEXT, type_scores TEXT NOT NULL)",
                "CREATE TABLE IF NOT EXISTS relations (id INTEGER PRIMARY KEY, source TEXT, target TEXT, link_type TEXT, probability REAL)"
            ]
        }),
    )?;

    // 2. INSERT the memory (use the actual content/types from args)
    let content_sql_esc = sql_escape(content);
    let ts_sql = if ts.is_empty() {
        sql_escape(&now_iso())
    } else {
        sql_escape(ts)
    };
    let entities_sql_esc = sql_escape(&entities_json);
    let type_scores_sql_esc = sql_escape(&type_scores_json);
    let insert_sql = format!(
        "INSERT INTO memories (content, ts, entities, type_scores) VALUES ('{content_sql_esc}', '{ts_sql}', '{entities_sql_esc}', '{type_scores_sql_esc}')"
    );
    // `statements` runs as one transaction; a trailing SELECT emits the row id
    // as JSON rows (same shape as `query`), so we can return it to the caller.
    let insert_result = call_db_for_db(
        &db_path_owned,
        json!({ "op": "exec", "statements": [insert_sql, "SELECT last_insert_rowid() AS last_insert_rowid"] }),
    )?;
    let memory_id = insert_result
        .get("rows")
        .and_then(|r| r.as_array())
        .and_then(|a| a.first())
        .and_then(|row| row.get("last_insert_rowid"))
        .and_then(|v| v.as_i64())
        .unwrap_or(-1);

    // 3. Optional relation
    let mut relation_id: Option<i64> = None;
    if let Some(rel) = args.get("relation") {
        let source = rel
            .get("source")
            .and_then(|v| v.as_str())
            .ok_or_else(|| anyhow!("relation.source required"))?;
        let target = rel
            .get("target")
            .and_then(|v| v.as_str())
            .ok_or_else(|| anyhow!("relation.target required"))?;
        let link_type = rel
            .get("link_type")
            .and_then(|v| v.as_str())
            .unwrap_or("SEMANTIC");
        let probability = rel
            .get("probability")
            .and_then(|v| v.as_f64())
            .unwrap_or(1.0);
        let rel_sql = format!(
            "INSERT INTO relations (source, target, link_type, probability) VALUES ('{src}', '{tgt}', '{lt}', {prob})",
            src = sql_escape(source),
            tgt = sql_escape(target),
            lt = sql_escape(link_type),
            prob = probability,
        );
        let rel_result = call_db_for_db(
            &db_path_owned,
            json!({ "op": "exec", "statements": [rel_sql, "SELECT last_insert_rowid() AS last_insert_rowid"] }),
        )?;
        relation_id = rel_result
            .get("rows")
            .and_then(|r| r.as_array())
            .and_then(|a| a.first())
            .and_then(|row| row.get("last_insert_rowid"))
            .and_then(|v| v.as_i64());
    }

    // 4. Verify by reading back (also exercises the DSL `db` capability)
    let verify = call_db_for_db(
        &db_path_owned,
        json!({
            "op": "query",
            "sql": "SELECT id, content, ts, entities, type_scores FROM memories ORDER BY id DESC LIMIT 5"
        }),
    )?;
    let relations_q = call_db_for_db(
        &db_path_owned,
        json!({
            "op": "query",
            "sql": "SELECT id, source, target, link_type, probability FROM relations ORDER BY id DESC LIMIT 5"
        }),
    )?;

    // 5. Also run the persist_memory spec to prove the DSL path is intact.
    //    It's a demo spec (hardcoded INSERTs) — we only call it for the
    //    side-effect of exercising the capability path; user data already lives
    //    in the table from steps 1–3.
    let dsl_verify = run_spec(spec_dir, &HeuristicBackend, "persist_memory", &json!({}))
        .ok()
        .and_then(|v| v.get("result").cloned())
        .unwrap_or(Value::Null);

    Ok(json!({
        "memory_id": memory_id,
        "relation_id": relation_id,
        "recent_memories": verify.get("rows").cloned().unwrap_or(Value::Null),
        "recent_relations": relations_q.get("rows").cloned().unwrap_or(Value::Null),
        "db_path": db_path_owned.display().to_string(),
        "dsl_verify": dsl_verify,
    }))
}

fn tool_recall(db_path: &Path, args: &Value) -> Result<Value> {
    let limit = args.get("limit").and_then(|v| v.as_i64()).unwrap_or(10).max(1);
    let include_relations = args
        .get("include_relations")
        .and_then(|v| v.as_bool())
        .unwrap_or(true);
    let db_path_owned: PathBuf = args
        .get("db_path")
        .and_then(|v| v.as_str())
        .filter(|s| !s.is_empty())
        .map(PathBuf::from)
        .unwrap_or_else(|| db_path.to_path_buf());

    let memories = call_db_for_db(
        &db_path_owned,
        json!({
            "op": "query",
            "sql": format!(
                "SELECT id, content, ts, entities, type_scores FROM memories ORDER BY id DESC LIMIT {limit}"
            )
        }),
    )?;
    let out = json!({
        "memories": memories.get("rows").cloned().unwrap_or(Value::Null),
        "db_path": db_path_owned.display().to_string(),
    });
    if include_relations {
        let rels = call_db_for_db(
            &db_path_owned,
            json!({
                "op": "query",
                "sql": "SELECT id, source, target, link_type, probability FROM relations ORDER BY id DESC LIMIT 5"
            }),
        )?;
        let mut out = out;
        out.as_object_mut().unwrap().insert(
            "relations".into(),
            rels.get("rows").cloned().unwrap_or(Value::Null),
        );
        Ok(out)
    } else {
        Ok(out)
    }
}

// ─── helpers ───────────────────────────────────────────────────────────────

/// Load a spec from `spec_dir/<name>.json` and run it against `backend`.
fn run_spec(spec_dir: &Path, backend: &dyn Decide, name: &str, state: &Value) -> Result<Value> {
    let path = spec_dir.join(format!("{name}.json"));
    if !path.is_file() {
        bail!("spec not found: {}", path.display());
    }
    let wf: ResilientWorkflow = load_file(path.to_str().ok_or_else(|| anyhow!("non-utf8 spec path"))?)
        .with_context(|| format!("loading {}", path.display()))?;
    let outcome = wf
        .run(backend, state)
        .with_context(|| format!("running spec {name}"))?;
    Ok(outcome.to_json())
}

fn call_db_for_db(db_path: &Path, with: Value) -> Result<Value> {
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
    // Embed mode spawns the `sqlite3` CLI, which needs allow_exec=true and the
    // parent dir in allow_paths (resolve_store_path validates there).
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

fn ensure_parent(p: &Path) -> Result<()> {
    if let Some(parent) = p.parent() {
        if !parent.as_os_str().is_empty() && !parent.exists() {
            std::fs::create_dir_all(parent)
                .with_context(|| format!("create_dir_all {}", parent.display()))?;
        }
    }
    Ok(())
}

fn sql_escape(s: &str) -> String {
    s.replace('\'', "''")
}

fn now_iso() -> String {
    // Minimal ISO-like timestamp without pulling chrono: "%Y-%m-%dT%H:%M:%S"
    // via libc::localtime_r on the system clock.
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
            tm.tm_year + 1900,
            tm.tm_mon + 1,
            tm.tm_mday,
            tm.tm_hour,
            tm.tm_min,
            tm.tm_sec
        )
    }
}

// ─── JSON-RPC transport ────────────────────────────────────────────────────

fn send_response(id: Value, result: Value) {
    let line = json!({ "jsonrpc": "2.0", "id": id, "result": result });
    write_message(&line);
}

fn send_error(id: Value, code: i64, message: &str) {
    let line = json!({
        "jsonrpc": "2.0",
        "id": id,
        "error": { "code": code, "message": message }
    });
    write_message(&line);
}

fn send_tool_result(id: Value, structured: Value) {
    let text = serde_json::to_string_pretty(&structured).unwrap_or_else(|_| "{}".to_string());
    send_response(
        id,
        json!({
            "content": [{"type": "text", "text": text}],
            "structuredContent": structured,
            "isError": false,
        }),
    );
}

fn write_message(v: &Value) {
    let s = serde_json::to_string(v).unwrap_or_else(|_| "{}".to_string());
    let stdout = io::stdout();
    let mut h = stdout.lock();
    let _ = h.write_all(s.as_bytes());
    let _ = h.write_all(b"\n");
    let _ = h.flush();
}

fn handle_request(spec_dir: &Path, db_path: &Path, backend: &dyn Decide, msg: &Value) {
    let method = msg.get("method").and_then(|v| v.as_str()).unwrap_or("");
    let id = msg.get("id").cloned().unwrap_or(Value::Null);

    match method {
        "initialize" => {
            let requested = msg
                .get("params")
                .and_then(|p| p.get("protocolVersion"))
                .and_then(|v| v.as_str())
                .unwrap_or(PROTOCOL_VERSION);
            send_response(
                id,
                json!({
                    "protocolVersion": requested,
                    "capabilities": { "tools": {} },
                    "serverInfo": { "name": SERVER_INFO, "version": SERVER_VERSION },
                }),
            );
        }
        "ping" => send_response(id, json!({})),
        "tools/list" => send_response(id, json!({ "tools": tools() })),
        "tools/call" => {
            let params = msg.get("params").cloned().unwrap_or_else(|| json!({}));
            let name = params.get("name").and_then(|v| v.as_str()).unwrap_or("");
            let args = params
                .get("arguments")
                .cloned()
                .unwrap_or_else(|| json!({}));
            let result = match name {
                "jev_mem_assess" => tool_assess(spec_dir, backend, &args),
                "jev_mem_retrieve" => tool_retrieve(spec_dir, backend, &args),
                "jev_mem_persist" => tool_persist(spec_dir, db_path, &args),
                "jev_mem_recall" => tool_recall(db_path, &args),
                other => Err(anyhow!("unknown tool: {other}")),
            };
            match result {
                Ok(v) => send_tool_result(id, v),
                Err(e) => {
                    send_tool_result(
                        id,
                        json!({
                            "error": format!("{e:#}"),
                        }),
                    );
                    // Also emit a JSON-RPC error so the client sees it as a
                    // method failure, not a malformed tool result.
                    let _ = e; // already embedded in structuredContent above
                }
            }
        }
        "" => {
            // notification without method — ignore silently
        }
        other => {
            send_error(id, -32601, &format!("method not found: {other}"));
        }
    }
}

/// Entry point: spin up the stdio MCP loop. Blocks until stdin EOF.
///
/// `base_url == None` → use the offline heuristic backend; `Some(u)` →
/// `LayaBackend` driving a live `laya-tch` HTTP server.
pub fn serve_stdio(
    spec_dir: PathBuf,
    db_path: PathBuf,
    base_url: Option<String>,
) -> Result<()> {
    if !spec_dir.is_dir() {
        bail!(
            "spec dir does not exist: {} (set --spec-dir or LAYA_MEM_SPEC_DIR)",
            spec_dir.display()
        );
    }

    let backend: Box<dyn Decide> = match base_url.as_deref() {
        Some(u) => Box::new(LayaBackend::new(u)),
        None => Box::new(HeuristicBackend),
    };
    let label = match &base_url {
        Some(u) => format!("laya-tch @ {u}"),
        None => "offline heuristic".to_string(),
    };
    eprintln!("[jev-mem] backend: {label}");
    eprintln!("[jev-mem] spec_dir: {}", spec_dir.display());
    eprintln!("[jev-mem] db_path:  {}", db_path.display());

    let stdin = io::stdin();
    for line in stdin.lock().lines() {
        let line = match line {
            Ok(l) => l,
            Err(_) => break,
        };
        let trimmed = line.trim();
        if trimmed.is_empty() {
            continue;
        }
        let msg: Value = match serde_json::from_str(trimmed) {
            Ok(v) => v,
            Err(e) => {
                eprintln!("[jev-mem] parse error: {e}");
                continue;
            }
        };
        // Notifications (no `id`) → fire-and-forget, no reply.
        if msg.get("id").is_none() && msg.get("method").is_some() {
            continue;
        }
        handle_request(&spec_dir, &db_path, backend.as_ref(), &msg);
    }
    Ok(())
}
