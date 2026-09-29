//! Laya workflow engine — resilient decision graphs driven by `laya-tch`.
//!
//! Rust port of `code/laya/workflow_engine.py` + `code/laya/composition.py`, with
//! the same semantics so workflows can run on the libtorch backend:
//!
//!   * `WorkflowNode`     — one Laya decision junction (questions + edge + action)
//!   * `Edge`             — answer → destination routing with `min_confidence` gate
//!   * `ResilientWorkflow`— graph runner: ROUTE / RETRY / ESCALATE / STOP / EXECUTE,
//!                          retry budget, convergence window, loop detection,
//!                          max_iterations safety valve, full audit trace
//!   * `Checkpoint`       — JSON state persistence + resume
//!   * `SubWorkflow`      — nested workflow as a node action
//!   * `FanOut`           — run N sub-workflows and merge their results
//!
//! The backend is abstracted over `Decide`, so workflows work with the real
//! engine (`backend::LayaBackend`) or a deterministic mock (tests).

use anyhow::{anyhow, Result};
use serde_json::{json, Map, Value};

use crate::persist::{NodeRecord, NodeStore};
use std::collections::HashMap;
use std::path::Path;
use std::time::Instant;

// ─── decision types (mirror code/laya/engine.py) ────────────────────

/// One typed answer for one question.
#[derive(Clone, Debug)]
pub struct Decision {
    pub answer: Value,
    pub probabilities: Map<String, Value>,
    pub confidence: f64,
}

impl Decision {
    /// Probability of an option key ("A"/"B"/"true"/"false"/"0"/…), 0.0 if absent.
    pub fn prob(&self, key: &str) -> f64 {
        self.probabilities
            .get(key)
            .and_then(|v| v.as_f64())
            .unwrap_or(0.0)
    }

    pub fn as_f64(&self) -> f64 {
        self.answer.as_f64().unwrap_or(0.0)
    }

    pub fn as_str(&self) -> String {
        match &self.answer {
            Value::String(s) => s.clone(),
            other => other.to_string(),
        }
    }
}

/// Aggregated result of one Laya forward pass.
#[derive(Clone, Debug, Default)]
pub struct Verdict {
    pub answers: HashMap<String, Decision>,
    pub input_tokens: usize,
    pub latency_ms: f64,
}

impl Verdict {
    fn answer(&self, key: &str) -> Result<&Decision> {
        self.answers
            .get(key)
            .ok_or_else(|| anyhow!("verdict missing answer {key:?}"))
    }
    pub fn answer_value(&self, key: &str) -> Result<Value> {
        Ok(self.answer(key)?.answer.clone())
    }
    pub fn confidence(&self, key: &str) -> Result<f64> {
        Ok(self.answer(key)?.confidence)
    }
    pub fn prob(&self, key: &str, opt: &str) -> Result<f64> {
        Ok(self.answer(key)?.prob(opt))
    }
}

/// Anything that can answer a Laya request. Implemented by the HTTP client and
/// by test mocks.
pub trait Decide {
    fn decide(&self, state: &Value, questions: &Value) -> Result<Verdict>;
}

// ─── edge / node ────────────────────────────────────────────────────

/// A weighted edge: `condition` maps answer values to destination node names,
/// `default` is the fallback, `min_confidence` gates the edge (below → ESCALATE).
#[derive(Clone, Debug, Default)]
pub struct Edge {
    pub condition: HashMap<String, String>,
    pub default: Option<String>,
    pub min_confidence: f64,
}

impl Edge {
    pub fn new(condition: &[(&str, &str)], default: Option<&str>, min_confidence: f64) -> Self {
        Self {
            condition: condition
                .iter()
                .map(|(k, v)| (k.to_string(), v.to_string()))
                .collect(),
            default: default.map(|s| s.to_string()),
            min_confidence,
        }
    }

    /// Destination node, or `None` when the confidence gate rejects the edge.
    pub fn resolve(&self, answer: &Value, confidence: f64) -> Option<String> {
        if confidence < self.min_confidence {
            return None;
        }
        let key = match answer {
            Value::String(s) => s.clone(),
            other => other.to_string(),
        };
        self.condition
            .get(&key)
            .cloned()
            .or_else(|| self.default.clone())
    }
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum NodeAction {
    Route,
    Execute,
    Stop,
    Escalate,
    Retry,
}

impl NodeAction {
    pub fn as_str(self) -> &'static str {
        match self {
            NodeAction::Route => "route",
            NodeAction::Execute => "execute",
            NodeAction::Stop => "stop",
            NodeAction::Escalate => "escalate",
            NodeAction::Retry => "retry",
        }
    }
}

/// Result of running one node.
#[derive(Clone, Debug)]
pub struct NodeResult {
    pub node_name: String,
    pub verdict: Verdict,
    pub action: NodeAction,
    pub next_node: Option<String>,
    pub edge_answer: Value,
    pub confidence: f64,
    pub latency_ms: f64,
    /// App-level decision payload produced by the node (set by the app runner).
    pub action_result: Option<Value>,
}

/// Transforms the workflow state into the state handed to the backend.
pub type StateFn = std::sync::Arc<dyn Fn(&Value) -> Value + Send + Sync>;

/// Applies a node to the workflow (app-level routing on top of the Laya verdict).
pub type ActionFn = std::sync::Arc<dyn Fn(&Value, &Verdict) -> Result<Value> + Send + Sync>;

/// Merges fan-out branch results into a single payload.
pub type MergeFn = std::sync::Arc<dyn Fn(&[BranchResult]) -> Value + Send + Sync>;

/// A Laya scheduling node: ask typed questions, then route by the primary answer.
#[derive(Clone)]
pub struct WorkflowNode {
    pub name: String,
    pub questions: Value,
    pub edge: Edge,
    pub action_fn: Option<ActionFn>,
    pub state_fn: Option<StateFn>,
    pub primary_q: Option<String>,
    pub max_retries: usize,
}

impl WorkflowNode {
    pub fn new(name: &str, questions: Value, edge: Edge) -> Self {
        Self {
            name: name.to_string(),
            questions,
            edge,
            action_fn: None,
            state_fn: None,
            primary_q: None,
            max_retries: 2,
        }
    }

    pub fn with_action<F>(mut self, f: F) -> Self
    where
        F: Fn(&Value, &Verdict) -> Result<Value> + Send + Sync + 'static,
    {
        self.action_fn = Some(std::sync::Arc::new(f));
        self
    }

    /// Transform the state before it is sent to the backend (mirrors `state_fn`).
    pub fn with_state_fn<F>(mut self, f: F) -> Self
    where
        F: Fn(&Value) -> Value + Send + Sync + 'static,
    {
        self.state_fn = Some(std::sync::Arc::new(f));
        self
    }

    pub fn with_primary(mut self, q: &str) -> Self {
        self.primary_q = Some(q.to_string());
        self
    }

    pub fn with_max_retries(mut self, n: usize) -> Self {
        self.max_retries = n;
        self
    }

    /// Node name used when building each step's state (the backend-facing state).
    pub fn backend_state(&self, state: &Value) -> Value {
        match self.state_fn.as_deref() {
            Some(f) => f(state),
            None => state.clone(),
        }
    }

    fn primary(&self) -> Result<String> {
        if let Some(q) = &self.primary_q {
            return Ok(q.clone());
        }
        self.questions
            .as_object()
            .and_then(|o| o.keys().next().cloned())
            .ok_or_else(|| anyhow!("node {}: empty questions", self.name))
    }

    /// Ask the backend, resolve the edge, and classify the resulting action.
    pub fn run<B: Decide + ?Sized>(&self, backend: &B, state: &Value) -> Result<NodeResult> {
        let t0 = Instant::now();
        let verdict = backend.decide(&self.backend_state(state), &self.questions)?;
        let latency_ms = t0.elapsed().as_secs_f64() * 1000.0;

        let primary = self.primary()?;
        let d = verdict.answer(&primary)?;
        let answer = d.answer.clone();
        let confidence = d.confidence;

        let next_node = self.edge.resolve(&answer, confidence);
        // `EXECUTE:<node>` marks an edge that runs the node action then routes.
        let (action, next_node) = match next_node {
            None => (NodeAction::Escalate, None),
            Some(n) if n == "STOP" => (NodeAction::Stop, None),
            Some(n) if n == self.name => (NodeAction::Retry, Some(n)),
            Some(n) if n.strip_prefix("EXECUTE:").is_some() => (
                NodeAction::Execute,
                n.strip_prefix("EXECUTE:").map(str::to_string),
            ),
            Some(n) => (NodeAction::Route, Some(n)),
        };

        let action_result = match self.action_fn.as_deref() {
            Some(f) => Some(f(state, &verdict)?),
            None => None,
        };

        Ok(NodeResult {
            node_name: self.name.clone(),
            verdict,
            action,
            next_node,
            edge_answer: answer,
            confidence,
            latency_ms,
            action_result,
        })
    }
}

// ─── trace ──────────────────────────────────────────────────────────

#[derive(Default, Clone, Debug)]
pub struct WorkflowTrace {
    pub steps: Vec<Value>,
    pub total_latency_ms: f64,
    pub loop_detected: bool,
    pub loop_count: usize,
    pub final_action: String,
}

impl WorkflowTrace {
    pub fn add_step(&mut self, r: &NodeResult) {
        self.add(r)
    }

    fn add(&mut self, r: &NodeResult) {
        self.steps.push(json!({
            "node": r.node_name,
            "action": r.action.as_str(),
            "next": r.next_node,
            "answer": r.edge_answer,
            "confidence": r4(r.confidence),
            "latency_ms": (r.latency_ms * 10.0).round() / 10.0,
        }));
    }

    pub fn to_json(&self) -> Value {
        json!({
            "steps": self.steps,
            "total_latency_ms": (self.total_latency_ms * 10.0).round() / 10.0,
            "loop_detected": self.loop_detected,
            "loop_count": self.loop_count,
            "final_action": self.final_action,
            "step_count": self.steps.len(),
        })
    }
}

fn r4(x: f64) -> f64 {
    format!("{:.4}", x).parse::<f64>().unwrap_or(x)
}

/// Merge a node's app-level payload into the workflow state (top-level keys),
/// mirroring the Python `on_action` / `action_result` handling.
fn merge_payload(state: &mut Value, payload: &Option<Value>) {
    if let (Some(obj), Some(p)) = (
        state.as_object_mut(),
        payload.as_ref().and_then(|v| v.as_object()),
    ) {
        for (k, v) in p {
            obj.insert(k.clone(), v.clone());
        }
    }
}

// ─── workflow runner ────────────────────────────────────────────────

/// Result of a full workflow run.
#[derive(Clone, Debug)]
pub struct WorkflowOutcome {
    pub state: Value,
    pub trace: WorkflowTrace,
    pub history: Vec<Value>,
    pub node_visits: HashMap<String, usize>,
    pub iterations: usize,
}

impl WorkflowOutcome {
    pub fn final_action(&self) -> &str {
        &self.trace.final_action
    }
    pub fn to_json(&self) -> Value {
        json!({
            "result": self.state,
            "trace": self.trace.to_json(),
            "history": self.history,
            "node_visits": self.node_visits,
            "iterations": self.iterations,
        })
    }
}

/// Graph runner: same control flow as `ResilientWorkflow.run`.
pub struct ResilientWorkflow {
    pub nodes: HashMap<String, WorkflowNode>,
    pub start: String,
    pub max_iterations: usize,
    pub convergence_window: usize,
    pub convergence_eps: f64,
}

impl ResilientWorkflow {
    pub fn new(nodes: Vec<WorkflowNode>, start: &str) -> Self {
        Self {
            nodes: nodes.into_iter().map(|n| (n.name.clone(), n)).collect(),
            start: start.to_string(),
            max_iterations: 50,
            convergence_window: 5,
            convergence_eps: 1e-4,
        }
    }

    pub fn with_max_iterations(mut self, n: usize) -> Self {
        self.max_iterations = n;
        self
    }

    pub fn with_convergence(mut self, window: usize, eps: f64) -> Self {
        self.convergence_window = window;
        self.convergence_eps = eps;
        self
    }

    /// Run the graph to completion (or to a safety valve).
    pub fn run<B: Decide + ?Sized>(&self, backend: &B, state: &Value) -> Result<WorkflowOutcome> {
        self.run_with_progress(backend, state, &mut |_| {})
    }

    /// Run the workflow, invoking `on_node` right after each node completes so
    /// a caller can surface live progress (e.g. the CLI `--progress` flag).
    /// The node name is available via `NodeResult::node_name`.
    pub fn run_with_progress<B: Decide + ?Sized>(
        &self,
        backend: &B,
        state: &Value,
        on_node: &mut dyn FnMut(&NodeResult),
    ) -> Result<WorkflowOutcome> {
        let mut trace = WorkflowTrace::default();
        let mut history: Vec<Value> = Vec::new();
        let mut state = state.clone();
        let mut current = Some(self.start.clone());
        let mut retry_count = 0usize;
        let mut iteration = 0usize;
        let mut score_history: Vec<f64> = Vec::new();
        let mut node_visits: HashMap<String, usize> = HashMap::new();
        let t0 = Instant::now();

        while let Some(name) = current.clone() {
            if iteration >= self.max_iterations {
                break;
            }
            iteration += 1;

            let node = match self.nodes.get(&name) {
                Some(n) => n.clone(),
                None => {
                    trace.final_action = "error_node_missing".to_string();
                    break;
                }
            };
            *node_visits.entry(name.clone()).or_insert(0) += 1;

            let result = node.run(backend, &state)?;
            on_node(&result);
            trace.add(&result);

            if let Some(s) = state.get("score").and_then(|v| v.as_f64()) {
                score_history.push(s);
            }

            let mut step = json!({
                "iteration": iteration,
                "node": name,
                "action": result.action.as_str(),
                "answer": result.edge_answer,
                "confidence": r4(result.confidence),
                "latency_ms": (result.latency_ms * 10.0).round() / 10.0,
            });

            match result.action {
                NodeAction::Escalate => {
                    merge_payload(&mut state, &result.action_result);
                    trace.final_action = "escalate".to_string();
                    step["detail"] = json!(format!(
                        "escalated from {name} (confidence={:.3})",
                        result.confidence
                    ));
                    history.push(step);
                    break;
                }
                NodeAction::Retry => {
                    merge_payload(&mut state, &result.action_result);
                    retry_count += 1;
                    if retry_count >= node.max_retries {
                        trace.final_action = "escalate".to_string();
                        step["detail"] = json!(format!("max retries ({retry_count}) on {name}"));
                        history.push(step);
                        break;
                    }
                    step["detail"] = json!(format!("retry {retry_count}/{}", node.max_retries));
                }
                NodeAction::Route => {
                    merge_payload(&mut state, &result.action_result);
                    retry_count = 0;
                    current = result.next_node.clone();
                    step["detail"] =
                        json!(format!("routed to {}", current.clone().unwrap_or_default()));
                }
                NodeAction::Execute => {
                    merge_payload(&mut state, &result.action_result);
                    retry_count = 0;
                    current = result.next_node.clone();
                    step["detail"] = json!(format!(
                        "executed + routed to {}",
                        current.clone().unwrap_or_default()
                    ));
                }
                NodeAction::Stop => {
                    merge_payload(&mut state, &result.action_result);
                    trace.final_action = "stop".to_string();
                    current = None;
                    step["detail"] = json!("workflow stopped");
                }
            }

            history.push(step);

            if score_history.len() >= self.convergence_window {
                let w = &score_history[score_history.len() - self.convergence_window..];
                let (mn, mx) = w
                    .iter()
                    .fold((f64::INFINITY, f64::NEG_INFINITY), |(a, b), &v| {
                        (a.min(v), b.max(v))
                    });
                if mx - mn <= self.convergence_eps {
                    trace.final_action = "converged".to_string();
                    trace.loop_detected = true;
                    break;
                }
            }

            if let Some(cur) = &current {
                if node_visits.get(cur).copied().unwrap_or(0) >= 3 && !trace.loop_detected {
                    trace.loop_detected = true;
                    trace.loop_count = node_visits[cur];
                }
            }
        }

        if iteration >= self.max_iterations {
            trace.final_action = "max_iterations".to_string();
        }
        trace.total_latency_ms = t0.elapsed().as_secs_f64() * 1000.0;
        trace.loop_count = node_visits.values().copied().max().unwrap_or(0);

        Ok(WorkflowOutcome {
            state,
            trace,
            history,
            node_visits,
            iterations: iteration,
        })
    }

    /// Run the graph with **per-node** persistence to `store`.
    ///
    /// * Every node execution writes one `NodeRecord` before continuing.
    /// * `resume_from` (1-based iteration) skips all stored iterations up to
    ///   and including it: the run restarts at `resume_from + 1` with the
    ///   materialised state, so an interrupted process can pick up exactly
    ///   where it stopped.
    /// * `initial_state` (if given) overrides whatever the store materialises
    ///   — useful when starting a brand-new run on an existing directory.
    pub fn run_persistent<B: Decide + ?Sized>(
        &self,
        backend: &B,
        store: &mut NodeStore,
        initial_state: Option<&Value>,
        resume_from: Option<u64>,
    ) -> Result<WorkflowOutcome> {
        let last = store.last_iteration()?;
        let mut state = match initial_state {
            Some(s) => s.clone(),
            None => match resume_from {
                Some(n) => store.materialise_state(n)?,
                None => match last {
                    Some(n) => store.materialise_state(n)?,
                    None => Value::Object(Map::new()),
                },
            },
        };

        // Decide where to resume: the next iteration after the stored run.
        let mut next_iter = match resume_from {
            Some(n) => n + 1,
            None => last.map(|n| n + 1).unwrap_or(1),
        };

        store.write_manifest(&self.start, &self.start, &Value::Object(Map::new()))?;
        let mut trace = WorkflowTrace::default();
        let mut history: Vec<Value> = Vec::new();
        // Resume from the last stored node's `next_node` (or `start` if the
        // store is empty) so partial runs continue exactly where they stopped.
        let mut current = match initial_state {
            Some(_) => self.start.clone(),
            None => match store.read_all()?.last() {
                Some(last_rec) if last_rec.iteration + 1 == next_iter => last_rec
                    .next_node
                    .clone()
                    .unwrap_or_else(|| self.start.clone()),
                _ => self.start.clone(),
            },
        };
        let mut retry_count = 0usize;
        let mut score_history: Vec<f64> = Vec::new();
        let mut node_visits: HashMap<String, usize> = HashMap::new();
        let t0 = Instant::now();

        loop {
            if next_iter.saturating_sub(1) as usize >= self.max_iterations {
                // Hit the per-run safety valve: report it like the non-
                // persistent runner does instead of leaving final_action empty.
                trace.final_action = "max_iterations".to_string();
                break;
            }
            let iter = next_iter;
            next_iter += 1;

            let node = match self.nodes.get(&current) {
                Some(n) => n.clone(),
                None => {
                    trace.final_action = "error_node_missing".to_string();
                    break;
                }
            };
            *node_visits.entry(current.clone()).or_insert(0) += 1;

            let result = match node.run(backend, &state) {
                Ok(r) => r,
                Err(e) => {
                    let rec = NodeRecord {
                        node: current.clone(),
                        iteration: iter,
                        timestamp_ms: std::time::SystemTime::now()
                            .duration_since(std::time::UNIX_EPOCH)
                            .map(|d| d.as_millis() as i64)
                            .unwrap_or(0),
                        state_before: state.clone(),
                        state_after: state.clone(),
                        payload: None,
                        action: "error".to_string(),
                        edge_answer: Value::Null,
                        confidence: 0.0,
                        latency_ms: 0.0,
                        next_node: None,
                        detail: None,
                        error: Some(e.to_string()),
                    };
                    store.write(&rec)?;
                    trace.final_action = "error".to_string();
                    history.push(json!({"iteration": iter, "node": current, "action": "error", "error": e.to_string()}));
                    break;
                }
            };

            trace.add(&result);
            if let Some(s) = state.get("score").and_then(|v| v.as_f64()) {
                score_history.push(s);
            }

            let state_before = state.clone();
            merge_payload(&mut state, &result.action_result);
            let state_after = state.clone();

            let mut step = json!({
                "iteration": iter,
                "node": current,
                "action": result.action.as_str(),
                "answer": result.edge_answer,
                "confidence": r4(result.confidence),
                "latency_ms": (result.latency_ms * 10.0).round() / 10.0,
            });

            let mut next: Option<String> = None;
            match result.action {
                NodeAction::Escalate => {
                    trace.final_action = "escalate".to_string();
                    step["detail"] = json!(format!(
                        "escalated from {current} (confidence={:.3})",
                        result.confidence
                    ));
                }
                NodeAction::Retry => {
                    retry_count += 1;
                    if retry_count >= node.max_retries {
                        trace.final_action = "escalate".to_string();
                        step["detail"] = json!(format!("max retries ({retry_count}) on {current}"));
                    } else {
                        next = Some(current.clone());
                        step["detail"] = json!(format!("retry {retry_count}/{}", node.max_retries));
                    }
                }
                NodeAction::Route => {
                    retry_count = 0;
                    next = result.next_node.clone();
                    step["detail"] =
                        json!(format!("routed to {}", next.clone().unwrap_or_default()));
                }
                NodeAction::Execute => {
                    retry_count = 0;
                    next = result.next_node.clone();
                    step["detail"] = json!(format!(
                        "executed + routed to {}",
                        next.clone().unwrap_or_default()
                    ));
                }
                NodeAction::Stop => {
                    trace.final_action = "stop".to_string();
                    step["detail"] = json!("workflow stopped");
                }
            }
            let step_detail = step
                .get("detail")
                .and_then(|d| d.as_str())
                .unwrap_or("")
                .to_string();
            history.push(step);

            let rec = NodeRecord {
                node: current.clone(),
                iteration: iter,
                timestamp_ms: std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .map(|d| d.as_millis() as i64)
                    .unwrap_or(0),
                state_before,
                state_after,
                payload: result.action_result.clone(),
                action: result.action.as_str().to_string(),
                edge_answer: result.edge_answer.clone(),
                confidence: result.confidence,
                latency_ms: result.latency_ms,
                next_node: next.clone(),
                detail: if step_detail.is_empty() {
                    None
                } else {
                    Some(step_detail)
                },
                error: None,
            };
            store.write(&rec)?;

            match (result.action, next) {
                (NodeAction::Stop, _) | (_, None) => break,
                (NodeAction::Escalate, _) => break,
                (NodeAction::Retry, Some(n)) => current = n,
                (NodeAction::Route, Some(n)) | (NodeAction::Execute, Some(n)) => current = n,
                _ => break,
            }

            if score_history.len() >= self.convergence_window {
                let w = &score_history[score_history.len() - self.convergence_window..];
                let (mn, mx) = w
                    .iter()
                    .fold((f64::INFINITY, f64::NEG_INFINITY), |(a, b), &v| {
                        (a.min(v), b.max(v))
                    });
                if mx - mn <= self.convergence_eps {
                    trace.final_action = "converged".to_string();
                    trace.loop_detected = true;
                    break;
                }
            }

            if node_visits.get(&current).copied().unwrap_or(0) >= 3 && !trace.loop_detected {
                trace.loop_detected = true;
                trace.loop_count = node_visits[&current];
            }
        }

        trace.total_latency_ms = t0.elapsed().as_secs_f64() * 1000.0;
        trace.loop_count = node_visits.values().copied().max().unwrap_or(0);

        Ok(WorkflowOutcome {
            state,
            trace,
            history,
            node_visits,
            iterations: store.last_iteration()?.unwrap_or(0) as usize,
        })
    }

    /// **Rewind**: materialise the exact state as of `iter`, delete every record
    /// after it, and return the state. Does **not** re-execute anything.
    pub fn rewind(&self, store: &mut NodeStore, iter: u64) -> Result<Value> {
        let state = store.materialise_state(iter)?;
        store.delete_from(iter + 1)?;
        Ok(state)
    }

    /// **Replay**: materialise the state at `iter - 1` (its `state_before`)
    /// and re-execute just that node against the real backend. The old record
    /// is overwritten atomically.
    pub fn replay<B: Decide + ?Sized>(
        &self,
        backend: &B,
        store: &mut NodeStore,
        iter: u64,
    ) -> Result<NodeRecord> {
        let old = store
            .read(iter)?
            .ok_or_else(|| anyhow!("cannot replay iteration {iter}: no record"))?;
        let node = self.nodes.get(&old.node).ok_or_else(|| {
            anyhow!(
                "cannot replay iteration {iter}: node {:?} not in spec",
                old.node
            )
        })?;
        let result = node.run(backend, &old.state_before)?;
        let mut state_after = old.state_before.clone();
        merge_payload(&mut state_after, &result.action_result);
        let rec = NodeRecord {
            node: old.node.clone(),
            iteration: iter,
            timestamp_ms: std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_millis() as i64)
                .unwrap_or(0),
            state_before: old.state_before.clone(),
            state_after,
            payload: result.action_result.clone(),
            action: result.action.as_str().to_string(),
            edge_answer: result.edge_answer.clone(),
            confidence: result.confidence,
            latency_ms: result.latency_ms,
            next_node: result.next_node.clone(),
            detail: None,
            error: None,
        };
        store.write(&rec)?;
        Ok(rec)
    }

    /// Human-readable graph description (mirrors `ResilientWorkflow.describe`).
    pub fn describe(&self) -> Value {
        let mut nodes = Map::new();
        let mut names: Vec<&String> = self.nodes.keys().collect();
        names.sort();
        for name in names {
            let n = &self.nodes[name];
            let questions: Vec<String> = n
                .questions
                .as_object()
                .map(|o| o.keys().cloned().collect())
                .unwrap_or_default();
            let cond: Map<String, Value> = n
                .edge
                .condition
                .iter()
                .map(|(k, v)| (k.clone(), json!(v)))
                .collect();
            nodes.insert(
                name.clone(),
                json!({
                    "questions": questions,
                    "primary_q": n.primary_q,
                    "edge_condition": cond,
                    "edge_default": n.edge.default,
                    "min_confidence": n.edge.min_confidence,
                    "has_action": n.action_fn.is_some(),
                    "max_retries": n.max_retries,
                }),
            );
        }
        json!({
            "start": self.start,
            "max_iterations": self.max_iterations,
            "nodes": nodes,
        })
    }
}

// ─── checkpoint ─────────────────────────────────────────────────────

/// JSON checkpoint for long-running loops (save/resume across process restarts).
pub struct Checkpoint {
    pub path: String,
}

impl Checkpoint {
    pub fn new(path: &str) -> Self {
        Self {
            path: path.to_string(),
        }
    }

    pub fn exists(&self) -> bool {
        Path::new(&self.path).exists()
    }

    pub fn save(
        &self,
        state: &Value,
        iteration: usize,
        history: &[Value],
        extra: Option<&Value>,
    ) -> Result<()> {
        if let Some(parent) = Path::new(&self.path).parent() {
            std::fs::create_dir_all(parent)?;
        }
        let mut data = json!({
            "state": state,
            "iteration": iteration,
            "history": history,
            "timestamp": std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_secs_f64())
                .unwrap_or(0.0),
        });
        if let Some(e) = extra {
            data["extra"] = e.clone();
        }
        std::fs::write(&self.path, serde_json::to_string_pretty(&data)?)?;
        Ok(())
    }

    /// Returns `(state, iteration, history, extra)`; `None` when no checkpoint exists.
    pub fn load(&self) -> Result<Option<(Value, usize, Vec<Value>, Value)>> {
        if !self.exists() {
            return Ok(None);
        }
        let raw = std::fs::read_to_string(&self.path)?;
        let data: Value = serde_json::from_str(&raw)?;
        Ok(Some((
            data.get("state")
                .cloned()
                .unwrap_or(Value::Object(Map::new())),
            data.get("iteration").and_then(|v| v.as_u64()).unwrap_or(0) as usize,
            data.get("history")
                .and_then(|v| v.as_array())
                .cloned()
                .unwrap_or_default(),
            data.get("extra")
                .cloned()
                .unwrap_or(Value::Object(Map::new())),
        )))
    }

    pub fn delete(&self) -> Result<()> {
        if self.exists() {
            std::fs::remove_file(&self.path)?;
        }
        Ok(())
    }
}

// ─── composition ────────────────────────────────────────────────────

/// Nested workflow executed as a node action.
pub struct SubWorkflow {
    pub name: String,
    pub workflow: ResilientWorkflow,
}

impl SubWorkflow {
    pub fn new(name: &str, workflow: ResilientWorkflow) -> Self {
        Self {
            name: name.to_string(),
            workflow,
        }
    }

    pub fn execute<B: Decide + ?Sized>(&self, backend: &B, state: &Value) -> Result<Value> {
        let out = self.workflow.run(backend, state)?;
        Ok(json!({
            "_subworkflow": self.name,
            "_subworkflow_result": out.state,
            "_subworkflow_trace": out.trace.to_json(),
            "_subworkflow_iterations": out.iterations,
        }))
    }
}

/// One branch result inside a `FanOut`.
#[derive(Clone)]
pub struct BranchResult {
    pub branch_name: String,
    pub state: Value,
    pub trace: Value,
    pub iterations: usize,
    pub latency_ms: f64,
}

/// Run N sub-workflows and merge their results (fan-out / fan-in).
pub struct FanOut {
    pub name: String,
    pub branches: Vec<(String, ResilientWorkflow)>,
    pub merge_fn: Option<MergeFn>,
}

impl FanOut {
    pub fn new(name: &str, branches: Vec<(String, ResilientWorkflow)>) -> Self {
        Self {
            name: name.to_string(),
            branches,
            merge_fn: None,
        }
    }

    pub fn with_merge<F>(mut self, f: F) -> Self
    where
        F: Fn(&[BranchResult]) -> Value + Send + Sync + 'static,
    {
        self.merge_fn = Some(std::sync::Arc::new(f));
        self
    }

    pub fn execute<B: Decide + ?Sized>(&self, backend: &B, state: &Value) -> Result<Value> {
        let mut results = Vec::with_capacity(self.branches.len());
        for (branch, wf) in &self.branches {
            let t0 = Instant::now();
            let out = wf.run(backend, state)?;
            results.push(BranchResult {
                branch_name: branch.clone(),
                state: out.state,
                trace: out.trace.to_json(),
                iterations: out.iterations,
                latency_ms: t0.elapsed().as_secs_f64() * 1000.0,
            });
        }
        let merged = match self.merge_fn.as_deref() {
            Some(f) => f(&results),
            None => default_merge(&results),
        };
        let summary: Vec<Value> = results
            .iter()
            .map(|r| {
                json!({
                    "branch": r.branch_name,
                    "iterations": r.iterations,
                    "latency_ms": (r.latency_ms * 10.0).round() / 10.0,
                    "final_action": r.trace.get("final_action"),
                })
            })
            .collect();
        Ok(json!({
            "_fanout": self.name,
            "_fanout_results": summary,
            "_merged": merged,
        }))
    }
}

/// Default merge: the branch that iterated the most.
pub fn default_merge(results: &[BranchResult]) -> Value {
    match results.iter().max_by_key(|r| r.iterations) {
        Some(best) => json!({
            "best_branch": best.branch_name,
            "total_iterations": results.iter().map(|r| r.iterations).sum::<usize>(),
        }),
        None => json!({}),
    }
}

/// Count trailing values equal (within `tol`) to the last one — used by
/// `ResilientLoop` decisions in the Python side; exposed here for app reuse.
pub fn consecutive_same(values: &[f64], tol: f64) -> usize {
    if values.is_empty() {
        return 0;
    }
    let last = *values.last().unwrap();
    values
        .iter()
        .rev()
        .take_while(|&&v| (v - last).abs() <= tol)
        .count()
}
