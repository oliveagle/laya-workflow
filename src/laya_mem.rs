//! `laya-mem` — one MCP tool set registered with the generic MCP server.
//!
//! The JSON-RPC / stdio transport lives in `crate::mcp`. This module
//! only implements the **tools**: how to assess, retrieve, persist, and
//! recall Laya-Mem memories via the 8 DSL specs in `dsl/laya_mem/`.
//!
//! The public shape is a single [`LayaMemTools`] struct that implements
//! [`McpToolSet`]. A host (the `laya-workflow mcp serve` subcommand) constructs one and
//! passes it to `crate::mcp::serve_stdio`. Future tool groups follow the
//! same pattern: implement `McpToolSet`, register with the host.
//!
//! Tool state (spec directory, backend choice, SQLite path) is captured in
//! the `LayaMemTools` struct at construction time; each `McpTool` has
//! read-only access to those fields.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use anyhow::{anyhow, bail, Context, Result};
use serde_json::{json, Value};

use crate::backend::{HeuristicBackend, LayaBackend};
use crate::capability::db::{call_db, DbCap};
use crate::capability::Policy;
use crate::mcp::{DynTool, McpTool, McpToolSet};
use crate::spec::load_file;
use crate::workflow::{Decide, ResilientWorkflow};

pub const GROUP_NAME: &str = "laya-mem";
pub const DEFAULT_SPEC_SUBDIR: &str = "dsl/laya_mem";

/// Builtin spec root: `<crate>/dsl/laya_mem` shipped in the binary.
pub const PLUGIN_MANIFEST_DIR: &str = env!("CARGO_MANIFEST_DIR");

/// Construction-time state shared by all four tools.
#[derive(Clone)]
pub struct LayaMemTools {
    pub spec_dir: PathBuf,
    pub db_path: PathBuf,
    pub base_url: Option<String>,
}

impl LayaMemTools {
    /// Default spec_dir: `<plugin-manifest>/dsl` (next to the binary),
    /// falling back to `<cwd>/dsl/laya_mem` for in-repo use.
    pub fn default_spec_dir() -> PathBuf {
        let bundled = PathBuf::from(PLUGIN_MANIFEST_DIR).join("dsl");
        if bundled.is_dir() {
            return bundled;
        }
        PathBuf::from(DEFAULT_SPEC_SUBDIR)
    }

    pub fn default_db_path() -> PathBuf {
        let home = std::env::var("HOME").unwrap_or_else(|_| ".".to_string());
        PathBuf::from(home)
            .join("tmp")
            .join("laya_mem")
            .join("codex.sqlite")
    }

    pub fn from_env() -> Self {
        let spec_dir = std::env::var("LAYA_MEM_SPEC_DIR")
            .ok()
            .filter(|v| !v.is_empty())
            .map(PathBuf::from)
            .unwrap_or_else(Self::default_spec_dir);
        let db_path = std::env::var("LAYA_MEM_SQLITE")
            .ok()
            .filter(|v| !v.is_empty())
            .map(PathBuf::from)
            .unwrap_or_else(Self::default_db_path);
        let base_url = std::env::var("LAYA_BASE_URL").ok().filter(|v| !v.is_empty());
        Self { spec_dir, db_path, base_url }
    }

    fn backend(&self) -> Box<dyn Decide> {
        match self.base_url.as_deref() {
            Some(u) => Box::new(LayaBackend::new(u)),
            None => Box::new(HeuristicBackend),
        }
    }
}

impl McpToolSet for LayaMemTools {
    fn group_name(&self) -> &str {
        GROUP_NAME
    }

    fn tools(&self) -> Vec<DynTool> {
        let me = Arc::new(self.clone());
        vec![
            Arc::new(AssessTool(me.clone())),
            Arc::new(RetrieveTool(me.clone())),
            Arc::new(PersistTool(me.clone())),
            Arc::new(RecallTool(me)),
        ]
    }
}

// ─── tool: assess ─────────────────────────────────────────────────────────

struct AssessTool(Arc<LayaMemTools>);

impl McpTool for AssessTool {
    fn name(&self) -> &str { "laya_mem_assess" }
    fn description(&self) -> &str {
        "Decide whether a new observation should be remembered, and how. Runs the write-side System-One controller: 4 memory-type noul questions plus a 3-state admission gate. Returns type scores + ALLOW/CONFIRM/BLOCK."
    }
    fn input_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "observation": {"type": "string", "description": "The text/fact worth remembering."},
                "recent_memories": {"type": "array", "items": {"type": "string"}, "description": "Already-stored summaries near this topic (redundancy hints)."}
            },
            "required": ["observation"]
        })
    }
    fn call(&self, args: &Value) -> Result<Value> {
        let observation = args
            .get("observation")
            .and_then(|v| v.as_str())
            .ok_or_else(|| anyhow!("laya_mem_assess: missing 'observation'"))?;
        let mut state = json!({ "observation": observation });
        if let Some(recent) = args.get("recent_memories").and_then(|v| v.as_array()) {
            if !recent.is_empty() {
                state["recent_memories"] = Value::Array(recent.clone());
            }
        }
        let backend = self.0.backend();
        let typed = run_spec(&self.0.spec_dir, backend.as_ref(), "memory_type", &state)?;
        let admitted = run_spec(&self.0.spec_dir, backend.as_ref(), "admission", &state)?;
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
}

// ─── tool: retrieve ───────────────────────────────────────────────────────

struct RetrieveTool(Arc<LayaMemTools>);

impl McpTool for RetrieveTool {
    fn name(&self) -> &str { "laya_mem_retrieve" }
    fn description(&self) -> &str {
        "Decide whether retrieval should stop or continue, plus which graph views are worth spending budget on. Runs the read-side System-One controller: 6 routing noul questions and a 4-question stopping gate. Returns STOP_EVIDENCE_OK / CONTINUE_* and per-view route scores."
    }
    fn input_schema(&self) -> Value {
        json!({
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
            "required": ["query"]
        })
    }
    fn call(&self, args: &Value) -> Result<Value> {
        let query = args
            .get("query")
            .and_then(|v| v.as_str())
            .ok_or_else(|| anyhow!("laya_mem_retrieve: missing 'query'"))?;
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
        let backend = self.0.backend();
        let routing = run_spec(&self.0.spec_dir, backend.as_ref(), "routing", &state)?;
        let stopping = run_spec(&self.0.spec_dir, backend.as_ref(), "stopping", &state)?;
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
}

// ─── tool: persist ────────────────────────────────────────────────────────

struct PersistTool(Arc<LayaMemTools>);

impl McpTool for PersistTool {
    fn name(&self) -> &str { "laya_mem_persist" }
    fn description(&self) -> &str {
        "Append a memory (and optionally a relation) to the SQLite store, then verify the row(s) were written. Returns the verified rows."
    }
    fn input_schema(&self) -> Value {
        json!({
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
            "required": ["content"]
        })
    }
    fn call(&self, args: &Value) -> Result<Value> {
        let content = args
            .get("content")
            .and_then(|v| v.as_str())
            .ok_or_else(|| anyhow!("laya_mem_persist: missing 'content'"))?;
        let ts = args.get("ts").and_then(|v| v.as_str()).unwrap_or("");
        let entities = args
            .get("entities")
            .cloned()
            .unwrap_or_else(|| Value::Array(Vec::new()));
        let type_scores = args
            .get("type_scores")
            .cloned()
            .unwrap_or_else(|| json!({}));
        let db_path: PathBuf = args
            .get("db_path")
            .and_then(|v| v.as_str())
            .filter(|s| !s.is_empty())
            .map(PathBuf::from)
            .unwrap_or_else(|| self.0.db_path.clone());
        ensure_parent(&db_path)?;

        let entities_json = serde_json::to_string(&entities)?;
        let type_scores_json = serde_json::to_string(&type_scores)?;

        call_db_for_db(
            &db_path,
            json!({
                "op": "exec",
                "statements": [
                    "CREATE TABLE IF NOT EXISTS memories (id INTEGER PRIMARY KEY, content TEXT NOT NULL, ts TEXT, entities TEXT, type_scores TEXT NOT NULL)",
                    "CREATE TABLE IF NOT EXISTS relations (id INTEGER PRIMARY KEY, source TEXT, target TEXT, link_type TEXT, probability REAL)"
                ]
            }),
        )?;

        let content_esc = sql_escape(content);
        let ts_esc = if ts.is_empty() { sql_escape(&now_iso()) } else { sql_escape(ts) };
        let entities_esc = sql_escape(&entities_json);
        let type_scores_esc = sql_escape(&type_scores_json);
        let insert_sql = format!(
            "INSERT INTO memories (content, ts, entities, type_scores) VALUES ('{content_esc}', '{ts_esc}', '{entities_esc}', '{type_scores_esc}')"
        );
        let insert_result = call_db_for_db(
            &db_path,
            json!({ "op": "exec", "statements": [insert_sql, "SELECT last_insert_rowid() AS last_insert_rowid"] }),
        )?;
        let memory_id = insert_result
            .get("rows")
            .and_then(|r| r.as_array())
            .and_then(|a| a.first())
            .and_then(|row| row.get("last_insert_rowid"))
            .and_then(|v| v.as_i64())
            .unwrap_or(-1);

        let mut relation_id: Option<i64> = None;
        if let Some(rel) = args.get("relation") {
            let source = rel.get("source").and_then(|v| v.as_str())
                .ok_or_else(|| anyhow!("relation.source required"))?;
            let target = rel.get("target").and_then(|v| v.as_str())
                .ok_or_else(|| anyhow!("relation.target required"))?;
            let link_type = rel.get("link_type").and_then(|v| v.as_str()).unwrap_or("SEMANTIC");
            let probability = rel.get("probability").and_then(|v| v.as_f64()).unwrap_or(1.0);
            let rel_sql = format!(
                "INSERT INTO relations (source, target, link_type, probability) VALUES ('{s}', '{t}', '{lt}', {p})",
                s = sql_escape(source), t = sql_escape(target),
                lt = sql_escape(link_type), p = probability,
            );
            let rel_result = call_db_for_db(
                &db_path,
                json!({ "op": "exec", "statements": [rel_sql, "SELECT last_insert_rowid() AS last_insert_rowid"] }),
            )?;
            relation_id = rel_result
                .get("rows")
                .and_then(|r| r.as_array())
                .and_then(|a| a.first())
                .and_then(|row| row.get("last_insert_rowid"))
                .and_then(|v| v.as_i64());
        }

        let verify = call_db_for_db(
            &db_path,
            json!({ "op": "query", "sql": "SELECT id, content, ts, entities, type_scores FROM memories ORDER BY id DESC LIMIT 5" }),
        )?;
        let relations_q = call_db_for_db(
            &db_path,
            json!({ "op": "query", "sql": "SELECT id, source, target, link_type, probability FROM relations ORDER BY id DESC LIMIT 5" }),
        )?;

        Ok(json!({
            "memory_id": memory_id,
            "relation_id": relation_id,
            "recent_memories": verify.get("rows").cloned().unwrap_or(Value::Null),
            "recent_relations": relations_q.get("rows").cloned().unwrap_or(Value::Null),
            "db_path": db_path.display().to_string(),
        }))
    }
}

// ─── tool: recall ─────────────────────────────────────────────────────────

struct RecallTool(Arc<LayaMemTools>);

impl McpTool for RecallTool {
    fn name(&self) -> &str { "laya_mem_recall" }
    fn description(&self) -> &str {
        "Read recent memories (and optionally relations) from the SQLite store. Returns the last N rows."
    }
    fn input_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "limit": {"type": "integer", "default": 10},
                "include_relations": {"type": "boolean", "default": true},
                "db_path": {"type": "string", "description": "Override the SQLite file for this call."}
            }
        })
    }
    fn call(&self, args: &Value) -> Result<Value> {
        let limit = args.get("limit").and_then(|v| v.as_i64()).unwrap_or(10).max(1);
        let include_relations = args
            .get("include_relations")
            .and_then(|v| v.as_bool())
            .unwrap_or(true);
        let db_path: PathBuf = args
            .get("db_path")
            .and_then(|v| v.as_str())
            .filter(|s| !s.is_empty())
            .map(PathBuf::from)
            .unwrap_or_else(|| self.0.db_path.clone());

        let memories = call_db_for_db(
            &db_path,
            json!({ "op": "query", "sql": format!("SELECT id, content, ts, entities, type_scores FROM memories ORDER BY id DESC LIMIT {limit}") }),
        )?;
        let mut out = json!({
            "memories": memories.get("rows").cloned().unwrap_or(Value::Null),
            "db_path": db_path.display().to_string(),
        });
        if include_relations {
            let rels = call_db_for_db(
                &db_path,
                json!({ "op": "query", "sql": "SELECT id, source, target, link_type, probability FROM relations ORDER BY id DESC LIMIT 5" }),
            )?;
            out.as_object_mut().unwrap().insert(
                "relations".into(),
                rels.get("rows").cloned().unwrap_or(Value::Null),
            );
        }
        Ok(out)
    }
}

// ─── helpers ──────────────────────────────────────────────────────────────

fn run_spec(spec_dir: &Path, backend: &dyn Decide, name: &str, state: &Value) -> Result<Value> {
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
