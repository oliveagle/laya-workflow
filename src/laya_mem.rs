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

use std::path::PathBuf;
use std::sync::Arc;

use anyhow::{anyhow, Result};

use serde_json::{json, Value};

use crate::backend::{HeuristicBackend, LayaBackend};
use crate::mcp::{DynTool, McpTool, McpToolSet};
use crate::workflow::Decide;


use crate::laya_mem_util::*;
pub use crate::laya_mem_util::{ensure_specs, restore_specs};


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
pub const EMBEDDED_SPECS: [(&str, &str); 9] = [
    ("admission.json", include_str!("../dsl/laya_mem/admission.json")),
    ("consolidation.json", include_str!("../dsl/laya_mem/consolidation.json")),
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
    /// Continuous consolidation: every N-th persist auto-runs the
    /// `consolidation` DSL spec against the top-k FTS5-matched candidates and
    /// writes relations / SUMMARY / audit_log. 0 disables the auto-trigger
    /// (the manual `laya_mem_consolidate` MCP tool always stays available).
    pub consolidation_interval: u64,
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
        let consolidation_interval = std::env::var("LAYA_MEM_CONSOLIDATE_INTERVAL")
            .ok()
            .filter(|v| !v.is_empty())
            .and_then(|v| v.parse::<u64>().ok())
            .unwrap_or(0);
        Self { spec_dir, db_path, base_url, consolidation_interval }
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
            Arc::new(AnswerTool(me.clone())),
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
        "Persist an observation into the laya-mem store. Phase 4 indexes its vector for hybrid recall; Phase 5 feeds it into the in-memory episode segmenter — a flushed EPISODE row (when one fires) is reported as `episode_memory_id`."
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

        // ensure_schema creates memories/relations/audit_log + FTS5 table + triggers
        // (in the right order so the first INSERT is indexed). Consolidation
        // columns and migrations are also applied here.
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

        // Phase 4: index the new memory's vector for semantic retrieval. Best
        // effort — a mock encoder never fails, and an OpenAI endpoint failure
        // must not lose the memory that was just persisted.
        let _ = embed_and_upsert(&db_path, &memory_id.to_string(), &content);
        // Phase 5: feed the segmenter; if a boundary just flushed an EPISODE,
        // the episode_memory_id is reported to the caller and indexed like any
        // other memory. Disabled with LAYA_MEM_EPISODES=0.
        let episode_memory_id = crate::laya_mem_episode::feed(&db_path, memory_id, content, &ts_esc);

        let verify = call_db_for_db(
            &db_path,
            json!({ "op": "query", "sql": "SELECT id, content, ts, entities, type_scores, node_type, consolidation_key, consolidation_action, source_memory_ids FROM memories ORDER BY id DESC LIMIT 5" }),
        )?;
        let relations_q = call_db_for_db(
            &db_path,
            json!({ "op": "query", "sql": "SELECT id, source, target, link_type, link_sub_type, probability, status FROM relations ORDER BY id DESC LIMIT 5" }),
        )?;

        // Continuous consolidation: every N-th persist (N = self.0.consolidation_interval)
        // runs the `consolidation` DSL spec against the top-k FTS5-matched
        // candidates and writes relations / SUMMARY / audit_log rows.
        let mut auto_consolidations: Vec<Value> = Vec::new();
        if self.0.consolidation_interval > 0 {
            {
                let mid = memory_id;
                if mid % (self.0.consolidation_interval as i64) == 0 {
                    let backend = self.0.backend();
                    let threshold = args
                        .get("consolidation_threshold")
                        .and_then(|v| v.as_f64())
                        .unwrap_or(0.85);
                    match auto_consolidate(
                        &db_path,
                        &self.0.spec_dir,
                        backend.as_ref(),
                        &mid.to_string(),
                        3,
                        threshold,
                    ) {
                        Ok(v) => auto_consolidations = v,
                        Err(e) => auto_consolidations.push(json!({
                            "error": format!("auto_consolidate: {e:#}"),
                        })),
                    }
                }
            }
        }

        Ok(json!({
            "memory_id": memory_id,
            "relation_id": relation_id,
            "episode_memory_id": episode_memory_id,
            "recent_memories": verify.get("rows").cloned().unwrap_or(Value::Null),
            "recent_relations": relations_q.get("rows").cloned().unwrap_or(Value::Null),
            "auto_consolidations": auto_consolidations,
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
        let memories = if let Some(q) = args.get("query").and_then(|v| v.as_str()).filter(|s| !s.is_empty()) {
            // Hybrid: FTS5 BM25 + dense cosine → RRF fusion (Phase 4, mirrors
            // Jev-Mem `query_engine._rrf_fusion`). The same `q` is encoded for
            // the dense side.
            hybrid_recall(&db_path, &sql, q, limit as usize)?
        } else {
            call_db_for_db(&db_path, json!({ "op": "query", "sql": sql }))?
        };
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


// ─── vector indexing (Phase 4) ───────────────────────────────────────────────

/// Embed `content` (mock by default, OpenAI-compatible if `LAYA_MEM_EMBEDDING_URL`
/// is set) and upsert the vector row for `memory_id`. Errors are swallowed by
/// callers that must not fail the memory write itself.
pub(crate) fn embed_and_upsert(db_path: &std::path::Path, memory_id: &str, content: &str) -> anyhow::Result<()> {
    let dim = crate::laya_mem_vec::MOCK_DIM;
    // Backend selection: `LAYA_MEM_EMBEDDING_BACKEND=needle` uses the on-device
    // Cactus engine (dim 3072, no network). The OpenAI endpoint path keeps its
    // precedence when `LAYA_MEM_EMBEDDING_URL` is set, so a caller can still
    // point at a hosted provider.
    if std::env::var("LAYA_MEM_EMBEDDING_BACKEND")
        .ok()
        .map(|v| v.eq_ignore_ascii_case("needle"))
        .unwrap_or(false)
    {
        match crate::laya_mem_vec::encode_needle(content) {
            Ok(vec) => {
                crate::laya_mem_vec::upsert_vector(db_path, memory_id, &vec)?;
                return Ok(());
            }
            Err(_) => {
                // fall through to OpenAI / mock, so a missing cact does not
                // kill the persist call.
            }
        }
    }
    let vec = match std::env::var("LAYA_MEM_EMBEDDING_URL")
        .ok()
        .filter(|s| !s.is_empty())
    {
        Some(url) => {
            let model = std::env::var("LAYA_MEM_EMBEDDING_MODEL")
                .unwrap_or_else(|_| "text-embedding-3-small".to_string());
            let key = std::env::var("LAYA_MEM_EMBEDDING_API_KEY").unwrap_or_default();
            match crate::laya_mem_vec::encode_openai(&url, &key, &model, &[content]) {
                Ok((mut vs, _)) if !vs.is_empty() => vs.remove(0),
                _ => crate::laya_mem_vec::encode_mock(content, dim),
            }
        }
        None => crate::laya_mem_vec::encode_mock(content, dim),
    };
    crate::laya_mem_vec::upsert_vector(db_path, memory_id, &vec)
}

/// Hybrid recall (Phase 4): FTS5 BM25 + dense cosine fused with RRF (k=60),
/// mirroring Jev-Mem's `query_engine._rrf_fusion`. Returns the top-`limit`
/// memory rows in fused order. `fts_sql` is the pre-built BM25 query (used to
/// short-circuit the FTS side); `query` is also encoded for the dense side.
fn hybrid_recall(
    db_path: &std::path::Path,
    fts_sql: &str,
    query: &str,
    limit: usize,
) -> anyhow::Result<Value> {
    let ids = crate::laya_mem_vec::hybrid_top_ids(db_path, fts_sql, query, "", limit)?;
    if ids.is_empty() {
        return Ok(json!({ "rows": Value::Null }));
    }
    // Preserve fused order via a SQLite `WITH ranked AS (VALUES ...)` CTE,
    // then join by id. `JOIN (VALUES ...)` is rejected by some sqlite3 builds
    // so we use a CTE explicitly.
    let rank_rows = ids
        .iter()
        .enumerate()
        .map(|(i, id)| format!("('{}', {})", crate::laya_mem_util::sql_escape(id), i))
        .collect::<Vec<_>>()
        .join(",");
    let in_list = ids
        .iter()
        .map(|id| format!("'{}'", crate::laya_mem_util::sql_escape(id)))
        .collect::<Vec<_>>()
        .join(",");
    let sql = format!(
        "WITH ranked(fid, f_rnk) AS (VALUES {rank_rows}) \
         SELECT m.id, m.content, m.ts, m.entities, m.type_scores, m.node_type, \
         m.consolidation_key, m.consolidation_action, m.source_memory_ids \
         FROM memories m \
         JOIN ranked ON m.id = ranked.fid \
         ORDER BY ranked.f_rnk"
    );
    let res = crate::laya_mem_util::call_db_for_db(db_path, json!({ "op": "query", "sql": sql }))?;
    Ok(res)
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
        "Apply one periodic consolidation decision. Writes a typed consolidation relation (REDUNDANT_WITH / CONTRADICTS / OBSOLETE / RELATED_TO) between target_memory_id and candidate_id; when representation is merge/promote and a summary_content is supplied, creates a non-destructive summary memory (node_type=SUMMARY, consolidation_key=fnv1a(min(target,candidate)+max(target,candidate)), source_memory_ids=[target, candidate], consolidation_action=merge|promote). All operations emit audit_log rows. Raw evidence (target + candidate memories) is never modified — deletion of originals would violate Jev-Mem's non-destructive invariant."
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

        apply_consolidation(
            &db_path,
            target,
            candidate,
            &scores,
            threshold,
            representation,
            summary_content,
            &summary_type_scores,
            &summary_entities,
            "consolidation",
        )
    }
}

// ─── tool: answer (System-Two) ─────────────────────────────────────────────

/// System-Two answer synthesis (Phase 6, Jev-Mem `longmemeval_jev.py`):
/// hybrid-recall the top-k memories for a question, then either call an
/// OpenAI-compatible chat endpoint (`LAYA_MEM_LLM_URL` + `_MODEL` + `_API_KEY`)
/// or fall back to a deterministic extractive answer. The full pipeline
/// (question, evidence, answer, latency) is written to audit_log as
/// `op='system_two_answer'`.
struct AnswerTool(Arc<LayaMemTools>);

impl McpTool for AnswerTool {
    fn name(&self) -> &str { "laya_mem_answer" }
    fn description(&self) -> &str {
        "System-Two: answer a question from the memory store. Retrieves the top-k memories via hybrid recall (BM25 + dense cosine, RRF-fused), then synthesizes a concise answer — via an OpenAI-compatible LLM when LAYA_MEM_LLM_URL is set, otherwise deterministically from the best-matching evidence. Emits a system_two_answer audit row. Answers 'Information not found' when no evidence matches."
    }
    fn input_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "question": {"type": "string", "description": "Natural-language question to answer from memory."},
                "top_k": {"type": "integer", "default": 6},
                "db_path": {"type": "string"}
            },
            "required": ["question"]
        })
    }
    fn call(&self, args: &Value) -> Result<Value> {
        let question = args
            .get("question")
            .and_then(|v| v.as_str())
            .filter(|s| !s.is_empty())
            .ok_or_else(|| anyhow!("laya_mem_answer: missing 'question'"))?;
        let top_k = args.get("top_k").and_then(|v| v.as_i64()).unwrap_or(6).max(1).min(20) as usize;
        let db_path: PathBuf = args
            .get("db_path")
            .and_then(|v| v.as_str())
            .filter(|s| !s.is_empty())
            .map(PathBuf::from)
            .unwrap_or_else(|| self.0.db_path.clone());
        ensure_schema(&db_path)?;

        // Build the same FTS5 query `laya_mem_recall` would, for the BM25 side
        // of hybrid_top_ids.
        let tokens: Vec<String> = question
            .split(|c: char| !c.is_alphanumeric())
            .filter(|t| !t.is_empty())
            .map(|t| format!("{}*", t))
            .collect();
        let fts_sql = if tokens.is_empty() {
            "SELECT m.id FROM memories m LIMIT 0".to_string()
        } else {
            let fts_query = tokens.join(" OR ");
            format!(
                "SELECT m.id FROM memories_fts f JOIN memories m ON m.id = f.rowid WHERE memories_fts MATCH '{}' ORDER BY bm25(memories_fts) LIMIT {}",
                fts_query.replace('\'', "''"),
                top_k * 4,
            )
        };

        let t0 = std::time::Instant::now();
        let (answer, evidence) = crate::laya_mem_answer::answer_query(&db_path, question, top_k, &fts_sql)?;
        let latency_ms = t0.elapsed().as_secs_f64() * 1000.0;

        // Audit (Jev-Mem: `audit.emit("system_two_answer", …)`).
        let _ = crate::laya_mem_util::audit_log(
            &db_path,
            "system_two_answer",
            "",
            &json!({
                "question": question,
                "top_k": top_k,
                "answer": answer,
                "evidence_rows": evidence.get("rows").and_then(|r| r.as_array()).map(|a| a.len()).unwrap_or(0),
                "latency_ms": latency_ms,
                "llm": std::env::var("LAYA_MEM_LLM_URL").ok().filter(|s| !s.is_empty()).is_some(),
            }),
        );

        Ok(json!({
            "answer": answer,
            "evidence": evidence,
            "latency_ms": latency_ms,
            "db_path": db_path.display().to_string(),
        }))
    }
}

// ─── tool: stats ─────────────────────────────────────────────────────────────────

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
        // Phase 4: vectors in the semantic index. `memory_vectors` is created
        // lazily; if it isn't there yet, count_rows returns 0.
        let vector_count = count("SELECT COUNT(*) AS c FROM memory_vectors");
        let vector_dim = count("SELECT dim FROM memory_vectors ORDER BY rowid DESC LIMIT 1");
        // Phase 5: episode segmentation; on-disk EPISODE rows plus buffered ones.
        let episode_count = count("SELECT COUNT(*) AS c FROM memories WHERE node_type = 'EPISODE'");
        let buffered_turns = crate::laya_mem_episode::buffered_turns(&db_path) as i64;
        let audit_count = count("SELECT COUNT(*) AS c FROM audit_log");
        let consolidation_actions = count("SELECT COUNT(*) AS c FROM audit_log WHERE op = 'consolidation'");
        let auto_consolidation_actions = count("SELECT COUNT(*) AS c FROM audit_log WHERE op = 'auto_consolidation'");

        let recent_q = json!({ "op": "query", "sql": "SELECT id, ts, op, memory_id, substr(details, 1, 200) AS details FROM audit_log ORDER BY id DESC LIMIT 5" });
        let recent = call_db_for_db(&db_path, recent_q)
            .ok()
            .and_then(|r| r.get("rows").cloned())
            .unwrap_or(Value::Null);

        Ok(json!({
            "total_memories": total_memories,
            "summary_count": summary_count,
            "total_relations": total_relations,
            "vector_count": vector_count,
            "vector_dim": vector_dim,
            "episode_count": episode_count,
            "buffered_turns": buffered_turns,
            "audit_count": audit_count,
            "consolidation_actions": consolidation_actions,
            "auto_consolidation_actions": auto_consolidation_actions,
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
