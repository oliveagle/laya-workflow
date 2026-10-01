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

/// Build-time crate root, kept only as a fallback for running the test binary
/// straight out of a checkout. A binary installed to `/usr/local/bin` has no
/// guarantee this path still exists, which is why the specs below are compiled
/// in rather than read from here.
pub const PLUGIN_MANIFEST_DIR: &str = env!("CARGO_MANIFEST_DIR");

/// The System-One specs, compiled into the binary: `(file name, contents)`.
///
/// These are the whole behavioural contract of the four tools, and an MCP server
/// that cannot find them answers every call with `spec not found`. Reading them
/// from `CARGO_MANIFEST_DIR` made that outcome depend on the *source tree*
/// outliving the binary — `cargo install` from a checkout that is later deleted,
/// or a release tarball, both leave an installed server with no specs at all.
/// Embedding makes `laya-workflow mcp serve` self-sufficient wherever it is
/// installed from.
pub const EMBEDDED_SPECS: [(&str, &str); 8] = [
    ("admission.json", include_str!("../dsl/laya_mem/admission.json")),
    ("memory_type.json", include_str!("../dsl/laya_mem/memory_type.json")),
    ("persist_memory.json", include_str!("../dsl/laya_mem/persist_memory.json")),
    ("relation_pair.json", include_str!("../dsl/laya_mem/relation_pair.json")),
    ("retrieve_loop.json", include_str!("../dsl/laya_mem/retrieve_loop.json")),
    ("routing.json", include_str!("../dsl/laya_mem/routing.json")),
    ("stopping.json", include_str!("../dsl/laya_mem/stopping.json")),
    ("traversal.json", include_str!("../dsl/laya_mem/traversal.json")),
];

/// Construction-time state shared by all four tools.
#[derive(Clone)]
pub struct LayaMemTools {
    pub spec_dir: PathBuf,
    pub db_path: PathBuf,
    pub base_url: Option<String>,
}

impl LayaMemTools {
    /// Default spec_dir: `~/.laya-workflow/laya-mem/specs`, materialized from
    /// [`EMBEDDED_SPECS`] on first use.
    ///
    /// The specs are written out rather than read straight from the embedded
    /// copy so they stay inspectable and editable: they are the System-One
    /// policy, and a user tuning it should be able to open the JSON. An
    /// existing file is never overwritten, so local edits survive restarts —
    /// `laya-workflow laya-mem specs --restore` is the way back to the shipped
    /// versions.
    pub fn default_spec_dir() -> PathBuf {
        if let Some(dir) = crate::state::laya_mem_spec_dir() {
            if ensure_specs(&dir).is_ok() {
                return dir;
            }
        }
        // No writable state root (no `$HOME`): fall back to the checkout, which
        // is also what running the test binary in-tree wants.
        let bundled = PathBuf::from(PLUGIN_MANIFEST_DIR).join(DEFAULT_SPEC_SUBDIR);
        if bundled.is_dir() {
            return bundled;
        }
        PathBuf::from(DEFAULT_SPEC_SUBDIR)
    }

    pub fn default_db_path() -> PathBuf {
        crate::state::laya_mem_db()
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
                "contradiction": {"type": "boolean", "description": "Evidence set has a contradiction. Adds the DSL trigger token so stopping rules can fire."},
                "missing_evidence": {"type": "boolean", "description": "Caller explicitly flags missing evidence."},
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
        // Fold caller signals into the DSL trigger fields the stopping spec
        // reads via `heuristic.field`. Three inputs participate:
        //   * `evidence_status` ("contradiction" / "insufficient" / "missing")
        //   * `contradiction` (bool)
        //   * `missing_evidence` (bool)
        // The spec questions each `field` one of {evidence_status,
        // contradiction, missing_evidence} and match_any the trigger word, so
        // we normalise all three signals into those fields. A bare `true`
        // would serialise to `true` and never match — the normalisation maps
        // it to the spec's trigger token.
        let ev_status = args.get("evidence_status").and_then(|v| v.as_str()).unwrap_or("");
        if ev_status == "contradiction"
            || args.get("contradiction").and_then(|v| v.as_bool()) == Some(true)
        {
            state["contradiction"] = json!("contradiction");
        }
        if matches!(ev_status, "insufficient" | "missing")
            || args.get("missing_evidence").and_then(|v| v.as_bool()) == Some(true)
        {
            state["missing_evidence"] = json!("missing");
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
                    "CREATE TABLE IF NOT EXISTS relations (id INTEGER PRIMARY KEY, source TEXT, target TEXT, link_type TEXT, probability REAL)",
                    "CREATE VIRTUAL TABLE IF NOT EXISTS memories_fts USING fts5(content, content='memories', content_rowid='id', tokenize='unicode61 remove_diacritics 2')",
                    "CREATE TRIGGER IF NOT EXISTS memories_ai AFTER INSERT ON memories BEGIN INSERT INTO memories_fts(rowid, content) VALUES (new.id, new.content); END",
                    "CREATE TRIGGER IF NOT EXISTS memories_ad AFTER DELETE ON memories BEGIN INSERT INTO memories_fts(memories_fts, rowid, content) VALUES('delete', old.id, old.content); END",
                    "CREATE TRIGGER IF NOT EXISTS memories_au AFTER UPDATE ON memories BEGIN INSERT INTO memories_fts(memories_fts, rowid, content) VALUES('delete', old.id, old.content); INSERT INTO memories_fts(rowid, content) VALUES (new.id, new.content); END"
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
                "query": {"type": "string", "description": "Optional content filter (SQLite LIKE pattern without % wildcards; escaped)."},
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

        let sql = if let Some(q) = args.get("query").and_then(|v| v.as_str()).filter(|s| !s.is_empty()) {
            // FTS5 with prefix wildcards on every token: "graduat*" matches
            // "graduated" / "graduating" / "graduate". This is the semantic-ish
            // retrieval the real dataset (LongMemEval) needs — plain LIKE
            // can't bridge "What degree did I graduate with?" to "Business
            // Administration". Tokenise on non-alnum, escape FTS5 punctuation.
            let tokens: Vec<String> = q.split(|c: char| !c.is_alphanumeric())
                .filter(|t| !t.is_empty())
                .map(|t| format!("{}*", t))
                .collect();
            if tokens.is_empty() {
                format!("SELECT id, content, ts, entities, type_scores FROM memories ORDER BY id DESC LIMIT {limit}")
            } else {
                // OR semantics: an AND of every question term almost never
                // matches natural phrasing ("what type of action figure"
                // needs all 4 tokens in one memory). OR lets BM25 rank —
                // memories matching more/rarer terms float to the top.
                let fts_query = tokens.join(" OR ");
                format!(
                    "SELECT m.id, m.content, m.ts, m.entities, m.type_scores FROM memories_fts f JOIN memories m ON m.id = f.rowid WHERE memories_fts MATCH '{}' ORDER BY bm25(memories_fts), m.id DESC LIMIT {limit}",
                    fts_query.replace('\'', "''")
                )
            }
        } else {
            format!("SELECT id, content, ts, entities, type_scores FROM memories ORDER BY id DESC LIMIT {limit}")
        };
        let memories = call_db_for_db(
            &db_path,
            json!({ "op": "query", "sql": sql }),
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

#[cfg(test)]
mod tests {
    use super::*;

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
        for name in ["memory_type", "admission", "routing", "stopping"] {
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
