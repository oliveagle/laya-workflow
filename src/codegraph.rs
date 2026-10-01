//! `codegraph` — an MCP tool set that turns Laya's routing / stopping /
//! traversal controllers into a decision-driven loop over a codegraph DB.
//!
//! Four tools mirror the laya-mem pattern; each runs one or more of the
//! embedded System-One specs plus one view-adapter SQL query against the
//! codegraph SQLite file. Every tool runs both **offline** (heuristic
//! backend) and against a live `laya-tch` server — same inputs, same output
//! shape, so the caller can switch backends without changing call sites.
//!
//! Tools:
//!
//! | tool | inputs | what it does |
//! |------|--------|--------------|
//! | `codegraph_route`  | `query` (+ optional hint fields) | runs `routing_codegraph.json`, returns 6 view scores + largest-remainder budget |
//! | `codegraph_fetch`  | `view`, `query`, `seed`, `budget` | executes the view's SQL against the codegraph DB, returns ranked rows |
//! | `codegraph_traverse` | `candidates[]` (+ per-axis hints) | runs `traversal_codegraph.json`, returns per-candidate 4-axis scores ranked by sum |
//! | `codegraph_stop`   | `query`, `evidence[]` (+ status hints) | runs `stopping_codegraph.json`, returns `STOP_EVIDENCE_OK` or `CONTINUE_*` |
//! | `codegraph_status` | — | schema + row-count probe of the codegraph DB (sanity check) |
//!
//! `codegraph_answer` is deliberately left to the caller (the LLM): the
//! fetch / traverse / stop loop produces ranked, structured evidence and it
//! is the caller's job to turn that into prose.

use std::path::PathBuf;
use std::sync::Arc;

use anyhow::{bail, Result};
use serde_json::{json, Value};

use crate::backend::{HeuristicBackend, LayaBackend};
use crate::mcp::{DynTool, McpTool, McpToolSet};
use crate::workflow::Decide;

use crate::codegraph_util as util;

pub const GROUP_NAME: &str = "codegraph";

pub const EMBEDDED_SPECS: [(&str, &str); 3] = [
    ("routing_codegraph.json", include_str!("../dsl/codegraph/routing_codegraph.json")),
    ("stopping_codegraph.json", include_str!("../dsl/codegraph/stopping_codegraph.json")),
    ("traversal_codegraph.json", include_str!("../dsl/codegraph/traversal_codegraph.json")),
];

/// Shared construction-time state for the codegraph tool set.
#[derive(Clone)]
pub struct CodegraphTools {
    pub spec_dir: PathBuf,
    pub db_path: PathBuf,
    pub base_url: Option<String>,
}

impl CodegraphTools {
    pub fn from_env() -> Self {
        let spec_dir = std::env::var("LAYA_CODEGRAPH_SPECS")
            .ok()
            .filter(|v| !v.is_empty())
            .map(PathBuf::from)
            .unwrap_or_else(util::default_spec_dir);
        let db_path = std::env::var("LAYA_CODEGRAPH_DB")
            .ok()
            .filter(|v| !v.is_empty())
            .map(PathBuf::from)
            .unwrap_or_else(util::default_db_path);
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

impl McpToolSet for CodegraphTools {
    fn group_name(&self) -> &str { GROUP_NAME }

    fn tools(&self) -> Vec<DynTool> {
        let me = Arc::new(self.clone());
        vec![
            Arc::new(RouteTool(me.clone())),
            Arc::new(FetchTool(me.clone())),
            Arc::new(TraverseTool(me.clone())),
            Arc::new(StopTool(me.clone())),
            Arc::new(StatusTool(me)),
        ]
    }
}

// ─── tool: route ─────────────────────────────────────────────────────────────

struct RouteTool(Arc<CodegraphTools>);

impl McpTool for RouteTool {
    fn name(&self) -> &str { "codegraph_route" }
    fn description(&self) -> &str {
        "Decide how to spend codegraph retrieval budget across the six views (semantic / temporal / causal / entity / multi_hop / recency). Runs the routing System-One controller; returns 6 raw 0..1 scores and a largest-remainder integer budget summing to `total`. The caller MUST set all six hint fields explicitly — the offline heuristic falls back to substring-matching the whole serialised state when any is missing, which cross-fires on other questions."
    }
    fn input_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "query": {"type": "string", "description": "The user's code question."},
                "route_semantic": {"type": "string", "enum": ["high", "low", "yes", "no"]},
                "route_temporal": {"type": "string", "enum": ["high", "low", "yes", "no"]},
                "route_causal": {"type": "string", "enum": ["high", "low", "yes", "no"]},
                "route_entity": {"type": "string", "enum": ["high", "low", "yes", "no"]},
                "route_multi_hop_need": {"type": "string", "enum": ["high", "low", "yes", "no"]},
                "route_recency": {"type": "string", "enum": ["high", "low", "yes", "no"]},
                "total": {"type": "integer", "description": "Total budget to allocate (default 8)."}
            },
            "required": ["query"]
        })
    }
    fn call(&self, args: &Value) -> Result<Value> {
        let query = args.get("query").and_then(|v| v.as_str())
            .ok_or_else(|| anyhow::anyhow!("codegraph_route: missing 'query'"))?;
        let mut state = json!({ "query": query });
        for k in [
            "route_semantic", "route_temporal", "route_causal",
            "route_entity", "route_multi_hop_need", "route_recency",
        ] {
            if let Some(v) = args.get(k) { state[k] = v.clone(); }
        }
        let backend = self.0.backend();
        let routed = util::run_spec(&self.0.spec_dir, backend.as_ref(), "routing_codegraph", &state)?;
        let r = routed.get("result").cloned().unwrap_or(Value::Null);
        let mut scores = [0.0f64; 6];
        for (i, k) in ["semantic", "temporal", "causal", "entity", "multi_hop_need", "recency_importance"].iter().enumerate() {
            scores[i] = r.get(k).and_then(|x| x.as_f64()).unwrap_or(0.0);
        }
        let total = args.get("total").and_then(|v| v.as_i64()).unwrap_or(8).max(1);
        let budget = util::allocate_budgets(&scores, total);
        Ok(json!({
            "scores": {
                "semantic": scores[0], "temporal": scores[1], "causal": scores[2],
                "entity": scores[3], "multi_hop_need": scores[4], "recency_importance": scores[5],
            },
            "budget": {
                "semantic": budget[0], "temporal": budget[1], "causal": budget[2],
                "entity": budget[3], "multi_hop_need": budget[4], "recency_importance": budget[5],
            },
            "total": total,
        }))
    }
}

// ─── tool: fetch ─────────────────────────────────────────────────────────────

struct FetchTool(Arc<CodegraphTools>);

impl McpTool for FetchTool {
    fn name(&self) -> &str { "codegraph_fetch" }
    fn description(&self) -> &str {
        "Execute one codegraph view (semantic FTS / call-chain / entity lookup / recent files / multi-hop BFS) against the DB and return ranked rows. No LLM involved; the SQL is a static template per view with user data pre-escaped."
    }
    fn input_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "view": {"type": "string", "enum": ["semantic", "causal", "entity", "temporal", "multi_hop"]},
                "query": {"type": "string", "description": "Semantic: FTS query. Causal / entity / multi_hop: seed symbol name or qualified_name fragment."},
                "seed": {"type": "string", "description": "Seed symbol for causal / multi_hop (falls back to `query` when omitted)."},
                "inbound": {"type": "boolean", "description": "Causal: also return incoming edges (default false)."},
                "depth": {"type": "integer", "description": "Multi-hop depth (default 2, max 5)."},
                "budget": {"type": "integer", "description": "Row limit for this call (default 10, max 200)."},
                "db_path": {"type": "string"}
            },
            "required": ["view"]
        })
    }
    fn call(&self, args: &Value) -> Result<Value> {
        let view = args.get("view").and_then(|v| v.as_str())
            .ok_or_else(|| anyhow::anyhow!("codegraph_fetch: missing 'view'"))?;
        let db_path: PathBuf = args.get("db_path").and_then(|v| v.as_str()).filter(|s| !s.is_empty())
            .map(PathBuf::from).unwrap_or_else(|| self.0.db_path.clone());
        if !util::db_ready(&db_path).unwrap_or(false) {
            bail!(
                "codegraph DB missing required tables (need {:?}) at {}",
                util::REQUIRED_TABLES, db_path.display()
            );
        }
        let query = args.get("query").and_then(|v| v.as_str()).unwrap_or("");
        let seed = args.get("seed").and_then(|v| v.as_str()).filter(|s| !s.is_empty()).unwrap_or(query);
        let budget = args.get("budget").and_then(|v| v.as_i64()).unwrap_or(10).clamp(1, 200);
        let inbound = args.get("inbound").and_then(|v| v.as_bool()).unwrap_or(false);
        let depth = args.get("depth").and_then(|v| v.as_i64()).unwrap_or(2).clamp(1, 5);
        match view {
            "semantic" => util::view_semantic(&db_path, query, budget),
            "causal" => util::view_causal(&db_path, seed, inbound, budget),
            "entity" => util::view_entity(&db_path, seed),
            "temporal" => util::view_temporal(&db_path, budget),
            "multi_hop" => util::view_multi_hop(&db_path, seed, depth, budget),
            other => bail!("codegraph_fetch: unknown view {other:?} (semantic | causal | entity | temporal | multi_hop)"),
        }
    }
}

// ─── tool: traverse ──────────────────────────────────────────────────────────

struct TraverseTool(Arc<CodegraphTools>);

impl McpTool for TraverseTool {
    fn name(&self) -> &str { "codegraph_traverse" }
    fn description(&self) -> &str {
        "Score 2 candidate nodes on the 4 traversal axes (relevance / relation_usefulness / new_information / supports_current_evidence) via the traversal System-One controller and rank them by weight sum. Caller supplies per-candidate hint fields; the offline heuristic falls back to whole-state substring matching when a field is missing."
    }
    fn input_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "cand_0_relevance": {"type": "string", "enum": ["high", "low", "yes", "no"]},
                "cand_0_relation_usefulness": {"type": "string", "enum": ["high", "low", "yes", "no"]},
                "cand_0_new_information": {"type": "string", "enum": ["high", "low", "yes", "no"]},
                "cand_0_supports_current_evidence": {"type": "string", "enum": ["high", "low", "yes", "no"]},
                "cand_1_relevance": {"type": "string", "enum": ["high", "low", "yes", "no"]},
                "cand_1_relation_usefulness": {"type": "string", "enum": ["high", "low", "yes", "no"]},
                "cand_1_new_information": {"type": "string", "enum": ["high", "low", "yes", "no"]},
                "cand_1_supports_current_evidence": {"type": "string", "enum": ["high", "low", "yes", "no"]},
                "weights": {"type": "object", "description": "Per-axis weights (default 0.4 / 0.2 / 0.2 / 0.2)."}
            },
            "required": []
        })
    }
    fn call(&self, args: &Value) -> Result<Value> {
        let backend = self.0.backend();
        let t = util::run_spec(&self.0.spec_dir, backend.as_ref(), "traversal_codegraph", args)?;
        let r = t.get("result").cloned().unwrap_or(Value::Null);
        let weights = args.get("weights").cloned().unwrap_or(json!({
            "relevance": 0.4,
            "relation_usefulness": 0.2,
            "new_information": 0.2,
            "supports_current_evidence": 0.2,
        }));
        let w = |k: &str| weights.get(k).and_then(|x| x.as_f64()).unwrap_or(0.25);
        let mut out = Vec::new();
        for (idx, prefix) in ["cand_0", "cand_1"].iter().enumerate() {
            let get = |suffix: &str| r.get(format!("{prefix}_{suffix}")).and_then(|x| x.as_f64()).unwrap_or(0.0);
            let relevance = get("relevance");
            let rel_use = get("relation_usefulness");
            let new_info = get("new_information");
            let supports = get("supports_current_evidence");
            let total = relevance * w("relevance") + rel_use * w("relation_usefulness")
                + new_info * w("new_information") + supports * w("supports_current_evidence");
            out.push(json!({
                "candidate": idx,
                "relevance": relevance,
                "relation_usefulness": rel_use,
                "new_information": new_info,
                "supports_current_evidence": supports,
                "weighted": total,
            }));
        }
        out.sort_by(|a, b| b["weighted"].as_f64().partial_cmp(&a["weighted"].as_f64()).unwrap_or(std::cmp::Ordering::Equal));
        Ok(json!({ "ranked": out }))
    }
}

// ─── tool: stop ──────────────────────────────────────────────────────────────

struct StopTool(Arc<CodegraphTools>);

impl McpTool for StopTool {
    fn name(&self) -> &str { "codegraph_stop" }
    fn description(&self) -> &str {
        "Decide whether the retrieval loop should stop. Runs the stopping System-One controller: 4 noul (evidence_sufficient, continue_useful, missing_evidence, contradiction) via first-fail-on-CONTINUE. Returns STOP_EVIDENCE_OK or CONTINUE_* with the reason."
    }
    fn input_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "query": {"type": "string"},
                "evidence": {"type": "array", "items": {"type": "string"}},
                "evidence_status": {"type": "string", "enum": ["sufficient", "insufficient", "contradiction"]},
                "missing_evidence": {"type": "boolean"},
                "contradiction": {"type": "boolean"},
                "continue_useful": {"type": "boolean"}
            },
            "required": ["query"]
        })
    }
    fn call(&self, args: &Value) -> Result<Value> {
        let query = args.get("query").and_then(|v| v.as_str())
            .ok_or_else(|| anyhow::anyhow!("codegraph_stop: missing 'query'"))?;
        let mut state = json!({ "query": query });
        if let Some(ev) = args.get("evidence").and_then(|v| v.as_array()) {
            state["evidence"] = Value::Array(ev.clone());
        }
        // Normalise caller signals into the DSL trigger fields.
        let ev_status = args.get("evidence_status").and_then(|v| v.as_str()).unwrap_or("");
        if ev_status == "contradiction" || args.get("contradiction").and_then(|v| v.as_bool()) == Some(true) {
            state["contradiction"] = json!("contradiction");
        }
        if matches!(ev_status, "insufficient" | "missing") || args.get("missing_evidence").and_then(|v| v.as_bool()) == Some(true) {
            state["missing_evidence"] = json!("missing");
        }
        if ev_status == "sufficient" { state["evidence_status"] = json!("sufficient"); }
        if args.get("continue_useful").and_then(|v| v.as_bool()) == Some(true) {
            state["continue_useful"] = json!("continue");
        }
        let backend = self.0.backend();
        let s = util::run_spec(&self.0.spec_dir, backend.as_ref(), "stopping_codegraph", &state)?;
        let r = s.get("result").cloned().unwrap_or(Value::Null);
        Ok(json!({
            "decision": r.get("label").cloned().unwrap_or(Value::Null),
            "detail": r.get("reason").cloned().or_else(|| r.get("action_answer").cloned()).unwrap_or(Value::Null),
            "confidence": r.get("confidence").cloned().unwrap_or(Value::Null),
        }))
    }
}

// ─── tool: status ────────────────────────────────────────────────────────────

struct StatusTool(Arc<CodegraphTools>);

impl McpTool for StatusTool {
    fn name(&self) -> &str { "codegraph_status" }
    fn description(&self) -> &str {
        "Probe the codegraph DB: required tables present? node / edge / file / unresolved-ref counts? edge and node kind histograms? Cheap sanity check before running the retrieval loop."
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
        let db_path: PathBuf = args.get("db_path").and_then(|v| v.as_str()).filter(|s| !s.is_empty())
            .map(PathBuf::from).unwrap_or_else(|| self.0.db_path.clone());
        let ready = util::db_ready(&db_path).unwrap_or(false);
        let stats = if ready { util::db_stats(&db_path).ok() } else { None };
        Ok(json!({
            "ready": ready,
            "required_tables": util::REQUIRED_TABLES,
            "db_path": db_path.display().to_string(),
            "stats": stats,
        }))
    }
}
