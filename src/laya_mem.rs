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
            Arc::new(RecallTool(me.clone())),
            Arc::new(ConsolidateTool(me.clone())),
            Arc::new(StatsTool(me.clone())),
            Arc::new(AuditTool(me)),
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

        // Ensure the laya-mem schema is current (memories + relations + audit_log;
        // FTS5 + triggers; consolidation columns). Idempotent on older DBs.
        call_db_for_db(
            &db_path,
            json!({
                "op": "exec",
                "statements": [
                    "CREATE VIRTUAL TABLE IF NOT EXISTS memories_fts USING fts5(content, content='memories', content_rowid='id', tokenize='unicode61 remove_diacritics 2')",
                    "CREATE TRIGGER IF NOT EXISTS memories_ai AFTER INSERT ON memories BEGIN INSERT INTO memories_fts(rowid, content) VALUES (new.id, new.content); END",
                    "CREATE TRIGGER IF NOT EXISTS memories_ad AFTER DELETE ON memories BEGIN INSERT INTO memories_fts(memories_fts, rowid, content) VALUES('delete', old.id, old.content); END",
                    "CREATE TRIGGER IF NOT EXISTS memories_au AFTER UPDATE ON memories BEGIN INSERT INTO memories_fts(memories_fts, rowid, content) VALUES('delete', old.id, old.content); INSERT INTO memories_fts(rowid, content) VALUES (new.id, new.content); END"
                ]
            }),
        )?;
        ensure_schema(&db_path)?;

        let content_esc = sql_escape(content);
        let ts_esc = if ts.is_empty() { sql_escape(&now_iso()) } else { sql_escape(ts) };
        let entities_esc = sql_escape(&entities_json);
        let type_scores_esc = sql_escape(&type_scores_json);
        let node_type_esc = sql_escape(
            args.get("node_type").and_then(|v| v.as_str()).unwrap_or("OBSERVATION")
        );
        let source_memory_ids_esc = sql_escape(
            &args.get("source_memory_ids")
                .map(|v| serde_json::to_string(v).unwrap_or_else(|_| "[]".to_string()))
                .unwrap_or_else(|| "[]".to_string())
        );
        let insert_sql = format!(
            "INSERT INTO memories (content, ts, entities, type_scores, node_type, source_memory_ids) VALUES ('{content_esc}', '{ts_esc}', '{entities_esc}', '{type_scores_esc}', '{node_type_esc}', '{source_memory_ids_esc}')"
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
            let link_sub_type = rel.get("link_sub_type").and_then(|v| v.as_str()).unwrap_or("");
            let status = rel.get("status").and_then(|v| v.as_str()).unwrap_or("ACTIVE");
            let rel_sql = format!(
                "INSERT INTO relations (source, target, link_type, link_sub_type, probability, status) VALUES ('{s}', '{t}', '{lt}', '{lst}', {p}, '{st}')",
                s = sql_escape(source), t = sql_escape(target),
                lt = sql_escape(link_type),
                lst = sql_escape(link_sub_type),
                p = probability,
                st = sql_escape(status),
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
            json!({ "op": "query", "sql": "SELECT id, content, ts, entities, type_scores, node_type, consolidation_key, consolidation_action, source_memory_ids FROM memories ORDER BY id DESC LIMIT 5" }),
        )?;
        let relations_q = call_db_for_db(
            &db_path,
            json!({ "op": "query", "sql": "SELECT id, source, target, link_type, link_sub_type, probability, status FROM relations ORDER BY id DESC LIMIT 5" }),
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
        ensure_schema(&db_path)?;

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
                format!("SELECT id, content, ts, entities, type_scores, node_type, consolidation_key, consolidation_action, source_memory_ids FROM memories ORDER BY id DESC LIMIT {limit}")
            } else {
                // OR semantics: an AND of every question term almost never
                // matches natural phrasing ("what type of action figure"
                // needs all 4 tokens in one memory). OR lets BM25 rank —
                // memories matching more/rarer terms float to the top.
                let fts_query = tokens.join(" OR ");
                format!(
                    "SELECT m.id, m.content, m.ts, m.entities, m.type_scores, m.node_type, m.consolidation_key, m.consolidation_action, m.source_memory_ids FROM memories_fts f JOIN memories m ON m.id = f.rowid WHERE memories_fts MATCH '{}' ORDER BY bm25(memories_fts), m.id DESC LIMIT {limit}",
                    fts_query.replace('\'', "''")
                )
            }
        } else {
            format!("SELECT id, content, ts, entities, type_scores, node_type, consolidation_key, consolidation_action, source_memory_ids FROM memories ORDER BY id DESC LIMIT {limit}")
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
                json!({ "op": "query", "sql": "SELECT id, source, target, link_type, link_sub_type, probability, status FROM relations ORDER BY id DESC LIMIT 5" }),
            )?;
            out.as_object_mut().unwrap().insert(
                "relations".into(),
                rels.get("rows").cloned().unwrap_or(Value::Null),
            );
        }
        Ok(out)
    }
}

// ─── tool: consolidate ─────────────────────────────────────────────────────────

/// Apply one consolidation decision between a target memory and one candidate.
/// Caller (agent) supplies the per-noul scores and a representation verdict;
/// this tool is the "apply" half of the System-One controller. Optional
/// `summary_content` triggers a non-destructive merge/promote (Jev-Mem:
/// only after System-Two summarizer returns text).
struct ConsolidateTool(Arc<LayaMemTools>);

impl McpTool for ConsolidateTool {
    fn name(&self) -> &str { "laya_mem_consolidate" }
    fn description(&self) -> &str {
        "Apply one periodic consolidation decision. Writes a typed consolidation relation (REDUNDANT_WITH / CONTRADICTS / OBSOLETE / RELATED_TO) between target_memory_id and candidate_id; when representation is merge/promote and a summary_content is supplied, creates a non-destructive summary memory (node_type=SUMMARY, consolidation_key=fnv1a(target+candidate), source_memory_ids=[target, candidate], consolidation_action=merge|promote). All operations emit audit_log rows. Raw evidence (target + candidate memories) is never modified — deletion of originals would violate Jev-Mem's non-destructive invariant."
    }
    fn input_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "target_memory_id": {"type": "string", "description": "Source memory id (typically the newly-written one)."},
                "candidate_id": {"type": "string", "description": "Existing memory id being compared."},
                "scores": {
                    "type": "object",
                    "properties": {
                        "redundant": {"type": "number", "description": "P(redundant), 0..1."},
                        "contradiction": {"type": "number", "description": "P(contradiction), 0..1."},
                        "obsolete": {"type": "number", "description": "P(obsolete), 0..1."},
                        "link": {"type": "number", "description": "P(link), 0..1."}
                    }
                },
                "representation": {
                    "type": "string",
                    "enum": ["keep_separate", "merge", "promote", "uncertain"],
                    "description": "System-One verdict (typically from laya_workflow run --spec dsl/laya_mem/consolidation.json)."
                },
                "consolidation_threshold": {"type": "number", "default": 0.85, "description": "Jev-Mem's threshold; noul >= threshold triggers the corresponding relation."},
                "summary_content": {"type": "string", "description": "System-Two synthesised text. Required when representation is merge/promote."},
                "summary_entities": {"type": "array", "items": {"type": "string"}},
                "summary_type_scores": {"type": "object"},
                "db_path": {"type": "string"}
            },
            "required": ["target_memory_id", "candidate_id", "scores", "representation"]
        })
    }

    fn call(&self, args: &Value) -> Result<Value> {
        let target = args.get("target_memory_id").and_then(|v| v.as_str())
            .ok_or_else(|| anyhow!("laya_mem_consolidate: missing 'target_memory_id'"))?;
        let candidate = args.get("candidate_id").and_then(|v| v.as_str())
            .ok_or_else(|| anyhow!("laya_mem_consolidate: missing 'candidate_id'"))?;
        let scores = args.get("scores").cloned().unwrap_or_else(|| json!({}));
        let representation = args.get("representation").and_then(|v| v.as_str())
            .ok_or_else(|| anyhow!("laya_mem_consolidate: missing 'representation'"))?;
        let threshold = args.get("consolidation_threshold").and_then(|v| v.as_f64()).unwrap_or(0.85);
        let summary_content = args.get("summary_content").and_then(|v| v.as_str()).unwrap_or("");
        let summary_type_scores = args.get("summary_type_scores").cloned().unwrap_or_else(|| json!({}));
        let summary_entities = args.get("summary_entities").cloned().unwrap_or_else(|| Value::Array(Vec::new()));
        let db_path: PathBuf = args
            .get("db_path")
            .and_then(|v| v.as_str())
            .filter(|s| !s.is_empty())
            .map(PathBuf::from)
            .unwrap_or_else(|| self.0.db_path.clone());
        ensure_parent(&db_path)?;
        ensure_schema(&db_path)?;

        let redundant = scores.get("redundant").and_then(|v| v.as_f64()).unwrap_or(0.0);
        let contradiction = scores.get("contradiction").and_then(|v| v.as_f64()).unwrap_or(0.0);
        let obsolete = scores.get("obsolete").and_then(|v| v.as_f64()).unwrap_or(0.0);
        let link = scores.get("link").and_then(|v| v.as_f64()).unwrap_or(0.0);

        // Determine the link sub_type + probability (priority: contradiction > obsolete > redundant > link).
        // Mirrors memory_builder.consolidate: 'CONTRADICTS' / 'REDUNDANT_WITH' / 'OBSOLETE' / 'RELATED_TO'.
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
        let mut stmts: Vec<String> = Vec::new();

        if !link_sub_type.is_empty() {
            let rel_sql = format!(
                "INSERT INTO relations (source, target, link_type, link_sub_type, probability, status) VALUES ('{s}', '{t}', '{lt}', '{lst}', {p}, 'ACTIVE')",
                s = sql_escape(target),
                t = sql_escape(candidate),
                lt = sql_escape(link_type_label),
                lst = sql_escape(link_sub_type),
                p = prob,
            );
            stmts.push(rel_sql);
            stmts.push("SELECT last_insert_rowid() AS last_insert_rowid".to_string());
            let res = call_db_for_db(&db_path, json!({ "op": "exec", "statements": stmts.clone() }))?;
            stmts.clear();
            if let Some(rows) = res.get("rows").and_then(|r| r.as_array()) {
                for row in rows {
                    if let Some(id) = row.get("last_insert_rowid").and_then(|v| v.as_i64()) {
                        written_relations.push(id);
                    }
                }
            }
        }

        // Non-destructive merge/promote: create a summary memory if requested.
        // Forced keep_separate when contradiction is high (>= threshold).
        let mut summary_memory_id: Option<i64> = None;
        let mut summary_blocked_reason: Option<String> = None;
        let allow_summarize = (representation == "merge" || representation == "promote")
            && contradiction < threshold
            && !summary_content.trim().is_empty();
        if representation == "merge" || representation == "promote" {
            if contradiction >= threshold {
                summary_blocked_reason = Some("contradiction>=threshold forces keep_separate (non-destructive)".to_string());
            } else if summary_content.trim().is_empty() {
                summary_blocked_reason = Some("summary_content required for merge/promote".to_string());
            }
        }

        if allow_summarize {
            // consolidation_key dedupe (Jev-Mem MemoryBuilder._consolidating + already_done check).
            let key = fnv1a_hex(&format!("{}|{}", target, candidate));
            let dup_q = format!(
                "SELECT id FROM memories WHERE consolidation_key = '{}' LIMIT 1",
                sql_escape(&key)
            );
            let dup = call_db_for_db(
                &db_path,
                json!({ "op": "query", "sql": dup_q }),
            )?;
            let existing_summary: Option<i64> = dup
                .get("rows").and_then(|r| r.as_array())
                .and_then(|a| a.first())
                .and_then(|row| row.get("id"))
                .and_then(|v| v.as_i64());
            if let Some(id) = existing_summary {
                summary_memory_id = Some(id);
                summary_blocked_reason = Some(format!("consolidation_key={} already produced summary id={}", key, id));
            } else {
                let entities_json = serde_json::to_string(&summary_entities)?;
                let type_scores_json = serde_json::to_string(&summary_type_scores)?;
                let summary_esc = sql_escape(summary_content);
                let ts_esc = sql_escape(&now_iso());
                let entities_esc = sql_escape(&entities_json);
                let type_scores_esc = sql_escape(&type_scores_json);
                let key_esc = sql_escape(&key);
                let action_esc = sql_escape(representation);
                let sources_esc = sql_escape(&serde_json::to_string(&[target, candidate]).unwrap_or_else(|_| "[]".to_string()));
                let summary_sql = format!(
                    "INSERT INTO memories (content, ts, entities, type_scores, node_type, source_memory_ids, consolidation_key, consolidation_action, consolidated_at) VALUES ('{summary_esc}', '{ts_esc}', '{entities_esc}', '{type_scores_esc}', 'SUMMARY', '{sources_esc}', '{key_esc}', '{action_esc}', '{ts_esc}')"
                );
                let res = call_db_for_db(
                    &db_path,
                    json!({ "op": "exec", "statements": [summary_sql, "SELECT last_insert_rowid() AS last_insert_rowid".to_string()] }),
                )?;
                summary_memory_id = res
                    .get("rows").and_then(|r| r.as_array())
                    .and_then(|a| a.first())
                    .and_then(|row| row.get("last_insert_rowid"))
                    .and_then(|v| v.as_i64());
            }
        }

        // Always emit an audit row, even when no relation written (keep_separate / uncertain path).
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
        let audit_id = audit_log(&db_path, "consolidation", target, &audit_details)?;

        // Read back recent relations + summary memory for verification.
        let verify_rels = call_db_for_db(
            &db_path,
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
            "recent_relations": verify_rels.get("rows").cloned().unwrap_or(Value::Null),
            "db_path": db_path.display().to_string(),
        }))
    }
}

// ─── tool: stats ───────────────────────────────────────────────────────────────────────

struct StatsTool(Arc<LayaMemTools>);

impl McpTool for StatsTool {
    fn name(&self) -> &str { "laya_mem_stats" }
    fn description(&self) -> &str {
        "Return memory-system statistics: total memories, summaries created via consolidation, total relations, total audit rows, and the last 5 consolidation actions. Use this to monitor consolidation activity."
    }
    fn input_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "db_path": {"type": "string"}
            }
        })
    }
    fn call(&self, args: &Value) -> Result<Value> {
        let db_path: PathBuf = args
            .get("db_path")
            .and_then(|v| v.as_str())
            .filter(|s| !s.is_empty())
            .map(PathBuf::from)
            .unwrap_or_else(|| self.0.db_path.clone());
        ensure_schema(&db_path)?;

        let count = |sql: &str| -> i64 {
            let res = call_db_for_db(&db_path, json!({ "op": "query", "sql": sql }));
            let rows = match res {
                Ok(r) => r.get("rows").and_then(|a| a.as_array()).cloned().unwrap_or_default(),
                Err(_) => Vec::new(),
            };
            rows.first()
                .and_then(|row| row.as_object().and_then(|obj| obj.values().next().cloned()))
                .and_then(|v| v.as_i64())
                .unwrap_or(0)
        };

        let total_memories = count("SELECT COUNT(*) AS c FROM memories");
        let summary_count = count("SELECT COUNT(*) AS c FROM memories WHERE node_type = 'SUMMARY'");
        let total_relations = count("SELECT COUNT(*) AS c FROM relations");
        let audit_count = count("SELECT COUNT(*) AS c FROM audit_log");
        let consolidation_actions = count("SELECT COUNT(*) AS c FROM audit_log WHERE op = 'consolidation'");

        let recent_q = json!({ "op": "query", "sql": "SELECT id, ts, op, memory_id, substr(details, 1, 200) AS details FROM audit_log ORDER BY id DESC LIMIT 5" });
        let recent = call_db_for_db(&db_path, recent_q)
            .ok()
            .and_then(|r| r.get("rows").cloned())
            .unwrap_or(Value::Null);

        Ok(json!({
            "total_memories": total_memories,
            "summary_count": summary_count,
            "total_relations": total_relations,
            "audit_count": audit_count,
            "consolidation_actions": consolidation_actions,
            "recent_audit": recent,
            "db_path": db_path.display().to_string(),
        }))
    }
}

// ─── tool: audit ─────────────────────────────────────────────────────────────────────

struct AuditTool(Arc<LayaMemTools>);

impl McpTool for AuditTool {
    fn name(&self) -> &str { "laya_mem_audit" }
    fn description(&self) -> &str {
        "Read audit_log entries. op filter (e.g. 'consolidation'), limit (default 50)."
    }
    fn input_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "op": {"type": "string", "description": "Filter to one op (e.g. 'consolidation', 'persist')."},
                "limit": {"type": "integer", "default": 50},
                "db_path": {"type": "string"}
            }
        })
    }
    fn call(&self, args: &Value) -> Result<Value> {
        let db_path: PathBuf = args
            .get("db_path")
            .and_then(|v| v.as_str())
            .filter(|s| !s.is_empty())
            .map(PathBuf::from)
            .unwrap_or_else(|| self.0.db_path.clone());
        ensure_schema(&db_path)?;
        let limit = args.get("limit").and_then(|v| v.as_i64()).unwrap_or(50).max(1).min(1000);
        let op_filter = args.get("op").and_then(|v| v.as_str()).filter(|s| !s.is_empty());

        let sql = match op_filter {
            Some(op) => format!(
                "SELECT id, ts, op, memory_id, details FROM audit_log WHERE op = '{}' ORDER BY id DESC LIMIT {}",
                sql_escape(op), limit
            ),
            None => format!(
                "SELECT id, ts, op, memory_id, details FROM audit_log ORDER BY id DESC LIMIT {}",
                limit
            ),
        };
        let res = call_db_for_db(&db_path, json!({ "op": "query", "sql": sql }))?;
        Ok(json!({
            "rows": res.get("rows").cloned().unwrap_or(Value::Null),
            "db_path": db_path.display().to_string(),
        }))
    }
}

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

/// FNV-1a 64-bit hash, hex-formatted. Not cryptographic; only used to
/// dedupe consolidation summaries (Jev-Mem equivalent: sha256(node_id+other_id)).
fn fnv1a_hex(s: &str) -> String {
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
fn ensure_schema(db_path: &Path) -> Result<()> {
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
    Ok(())
}

/// Append an audit_log row. Used by consolidate + future periodic hooks.
fn audit_log(db_path: &Path, op: &str, memory_id: &str, details: &Value) -> Result<i64> {
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
