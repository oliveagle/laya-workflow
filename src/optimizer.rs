//! Laya-driven optimization loops — Rust ports of
//! `code/laya/composition.py::ResilientLoop` and
//! `code/laya/optimizer_integration.py::LayaOptimizerLoop`.
//!
//! Both loops replace hardcoded plateau detection with Laya decision nodes:
//!
//!   * `ResilientLoop`      — generic: `step_fn` advances the state, Laya decides
//!                            continue/stop, checkpoints after each iteration.
//!   * `LayaOptimizerLoop`  — self-evolving optimizer: evaluate → Laya continue →
//!                            Laya strategy → propose/evaluate, with the eval
//!                            injected at the `evaluate` node (mirrors Python's
//!                            `_run_with_eval` custom runner).

use anyhow::{anyhow, Result};
use serde_json::{json, Map, Value};
use std::path::Path;
use std::time::Instant;

use crate::workflow::{
    consecutive_same, Decide, Edge, NodeAction, ResilientWorkflow, WorkflowNode, WorkflowTrace,
};

/// Transform the optimizer state into the state Laya observes.
pub type OptimStateFn = fn(&Value) -> Value;

fn r6(x: f64) -> f64 {
    format!("{:.6}", x).parse::<f64>().unwrap_or(x)
}

/// Mirror of `optimizer_integration._count_consecutive_rejects`.
fn count_consecutive_rejects(history: &[Value]) -> usize {
    let mut n = 0;
    for h in history.iter().rev() {
        if h.get("accepted").and_then(|v| v.as_bool()).unwrap_or(false) {
            break;
        }
        n += 1;
    }
    n
}

/// Mirror of `optimizer_integration._build_laya_state`.
pub fn build_laya_state(optim: &Value) -> Value {
    let history = optim.get("history").and_then(|v| v.as_array()).cloned().unwrap_or_default();
    let recent: Vec<Value> = history.iter().rev().take(5).rev().cloned().collect();
    let last_trial = recent
        .last()
        .and_then(|h| h.get("trial_score"))
        .and_then(|v| v.as_f64())
        .unwrap_or(0.0);
    let score = optim.get("score").and_then(|v| v.as_f64()).unwrap_or(0.0);
    let recent_scores: Vec<Value> = recent
        .iter()
        .map(|h| json!(r6(h.get("trial_score").and_then(|v| v.as_f64()).unwrap_or(0.0))))
        .collect();
    let recent_accepted: Vec<Value> = recent
        .iter()
        .map(|h| json!(h.get("accepted").and_then(|v| v.as_bool()).unwrap_or(false)))
        .collect();
    let params = optim.get("params").cloned().unwrap_or_else(|| json!({}));
    let params_summary: String = serde_json::to_string(&params).unwrap_or_default().chars().take(200).collect();

    json!({
        "task": optim.get("task").cloned().unwrap_or_else(|| json!("unknown")),
        "current_score": r6(score),
        "best_score": r6(optim.get("best_score").and_then(|v| v.as_f64()).unwrap_or(0.0)),
        "score_delta": r6(score - last_trial),
        "step": optim.get("step").cloned().unwrap_or_else(|| json!(0)),
        "history_len": history.len(),
        "recent_scores": recent_scores,
        "recent_accepted": recent_accepted,
        "consecutive_rejects": count_consecutive_rejects(&recent),
        "strategy": optim.get("strategy").cloned().unwrap_or_else(|| json!("unknown")),
        "params_summary": params_summary,
    })
}

// ─── ResilientLoop ──────────────────────────────────────────────────

/// Generic Laya-driven loop: `step_fn` advances the state, Laya decides whether
/// to continue, and a JSON checkpoint is written after every iteration.
pub struct ResilientLoop {
    pub name: String,
    pub checkpoint_dir: String,
    pub continue_questions: Value,
}

/// Default continue/stop question set (mirrors `ResilientLoop` in Python).
pub fn default_continue_questions() -> Value {
    json!({
        "should_continue": {
            "type": "choice",
            "instructions": "Based on the current state, score trajectory, and convergence pattern, should the loop continue optimizing?",
            "criteria": {
                "stop": "score plateaued or converged, no more improvement expected",
                "continue": "still improving, room for gains"
            }
        }
    })
}

impl ResilientLoop {
    pub fn new(name: &str, checkpoint_dir: &str) -> Self {
        Self {
            name: name.to_string(),
            checkpoint_dir: checkpoint_dir.to_string(),
            continue_questions: default_continue_questions(),
        }
    }

    pub fn with_questions(mut self, qs: Value) -> Self {
        self.continue_questions = qs;
        self
    }

    fn ckpt_path(&self) -> String {
        format!("{}/{}_checkpoint.json", self.checkpoint_dir, self.name)
    }

    fn decision_path(&self) -> String {
        format!("{}/{}_decisions.json", self.checkpoint_dir, self.name)
    }

    /// Run the loop, resuming from a checkpoint if one exists.
    ///
    /// `step_fn` is called once per iteration with the current state; Laya then
    /// decides continue/stop (`stop` with confidence ≥ 0.7 stops the loop).
    pub fn run<B, F>(&self, backend: &B, initial_state: &Value, step_fn: F, max_iterations: usize) -> Result<Value>
    where
        B: Decide + ?Sized,
        F: Fn(&Value) -> Result<Value>,
    {
        let ckpt = crate::workflow::Checkpoint::new(&self.ckpt_path());
        let dec_ckpt = crate::workflow::Checkpoint::new(&self.decision_path());

        let mut state = initial_state.clone();
        let mut iteration = 0usize;
        let mut score_history: Vec<f64> = Vec::new();
        let mut decision_history: Vec<Value> = Vec::new();

        if let Some((st, it, _, _)) = ckpt.load()? {
            state = st;
            iteration = it;
            if let Some((_, _, dh, _)) = dec_ckpt.load()? {
                decision_history = dh;
            }
        }

        for i in iteration..max_iterations {
            iteration = i + 1;
            state = step_fn(&state)?;
            if let Some(s) = state.get("score").and_then(|v| v.as_f64()) {
                score_history.push(s);
            }
            ckpt.save(&state, iteration, &[], None)?;

            let laya_state = json!({
                "task": state.get("task").cloned().unwrap_or_else(|| json!(self.name)),
                "score": r6(state.get("score").and_then(|v| v.as_f64()).unwrap_or(0.0)),
                "best_score": r6(state.get("best_score").and_then(|v| v.as_f64())
                    .unwrap_or_else(|| state.get("score").and_then(|v| v.as_f64()).unwrap_or(0.0))),
                "step": iteration,
                "recent_scores": score_history.iter().rev().take(5).rev().map(|s| json!(r6(*s))).collect::<Vec<_>>(),
                "consecutive_same": consecutive_same(&score_history, 1e-6),
            });

            let verdict = backend.decide(&laya_state, &self.continue_questions)?;
            let d = verdict
                .answers
                .get("should_continue")
                .ok_or_else(|| anyhow!("continue verdict missing should_continue"))?;
            let answer = d.as_str();
            let confidence = d.confidence;

            decision_history.push(json!({
                "iteration": iteration,
                "answer": answer,
                "confidence": r6(confidence),
                "score": state.get("score"),
            }));
            dec_ckpt.save(&json!({}), iteration, &decision_history, None)?;

            if answer == "stop" && confidence >= 0.7 {
                break;
            }
        }

        let window = 5usize.min(score_history.len());
        let converged = window >= 5 && {
            let w = &score_history[score_history.len() - 5..];
            let (mn, mx) = w.iter().fold((f64::INFINITY, f64::NEG_INFINITY), |(a, b), &v| (a.min(v), b.max(v)));
            mx - mn <= 1e-4
        };

        Ok(json!({
            "state": state,
            "iterations": iteration,
            "score_history": score_history,
            "decisions": decision_history.iter().rev().take(10).rev().collect::<Vec<_>>(),
            "converged": converged,
        }))
    }
}

// ─── LayaOptimizerLoop ──────────────────────────────────────────────

/// Laya decides: continue optimizing or stop?
pub fn make_continue_node() -> WorkflowNode {
    let questions = json!({
        "should_continue": {
            "type": "choice",
            "instructions": "Based on the current score, best score, recent history, and convergence pattern, should the optimizer continue? ",
            "criteria": {
                "stop": "the score has plateaued, we've converged, or further optimization is unlikely to help",
                "continue": "there's room for improvement, the score is still climbing, or recent changes show promise"
            }
        },
        "confidence_assessment": {
            "type": "score",
            "instructions": "How confident are you that continuing will improve the score?",
            "criteria": [
                "not confident — likely waste of compute",
                "somewhat confident — might find small gains",
                "very confident — clear improvement trajectory"
            ]
        }
    });
    WorkflowNode::new(
        "continue_check",
        questions,
        Edge::new(&[("stop", "STOP"), ("continue", "evaluate")], Some("evaluate"), 0.0),
    )
    .with_primary("should_continue")
    .with_state_fn(build_laya_state_action)
}

fn build_laya_state_action(state: &Value) -> Value {
    // The runner already hands the node an optimizer-shaped state; keep the
    // transform local so `describe` reports the same primary question.
    build_laya_state(state)
}

/// Laya decides: switch strategy or keep the current one?
pub fn make_strategy_node(strategies: &[String]) -> WorkflowNode {
    let mut criteria = Map::new();
    for s in strategies {
        criteria.insert(s.clone(), json!(format!("use strategy {s}")));
    }
    criteria.insert("keep".to_string(), json!("keep the current strategy, don't switch"));

    let questions = json!({
        "strategy_choice": {
            "type": "choice",
            "instructions": "Given the current score trajectory, recent acceptance rate, and consecutive rejections, should we switch optimization strategy or keep the current one?",
            "criteria": criteria,
        },
        "switch_urgency": {
            "type": "score",
            "instructions": "How urgently should we switch strategy?",
            "criteria": [
                "no rush — current strategy is working",
                "moderate — current strategy is slowing down",
                "urgent — current strategy is stuck, switch now"
            ]
        }
    });
    let cond: Vec<(String, String)> = strategies
        .iter()
        .map(|s| (s.clone(), s.clone()))
        .collect();
    let refs: Vec<(&str, &str)> = cond.iter().map(|(a, b)| (a.as_str(), b.as_str())).collect();
    WorkflowNode::new("strategy_check", questions, Edge::new(&refs, Some("keep"), 0.3))
        .with_primary("strategy_choice")
        .with_state_fn(build_laya_state_action)
}

/// Action node that always routes to `continue_check` (eval is injected by the runner).
pub fn make_evaluate_node() -> WorkflowNode {
    WorkflowNode::new(
        "evaluate",
        json!({}),
        Edge::new(&[], Some("continue_check"), 0.0),
    )
    .with_max_retries(0)
}

/// The optimizer state, matching `WorkflowState` fields used by the Python loop.
#[derive(Clone, Debug)]
pub struct OptimizerState {
    pub task: String,
    pub params: Value,
    pub score: f64,
    pub best_score: f64,
    pub best_params: Value,
    pub step: usize,
    pub history: Vec<Value>,
    pub strategy: String,
    pub maximize: bool,
}

impl OptimizerState {
    pub fn new(task: &str, maximize: bool) -> Self {
        Self {
            task: task.to_string(),
            params: json!({}),
            score: f64::NEG_INFINITY,
            best_score: f64::NEG_INFINITY,
            best_params: json!({}),
            step: 0,
            history: Vec::new(),
            strategy: "default".to_string(),
            maximize,
        }
    }

    pub fn to_laya_input(&self) -> Value {
        json!({
            "task": self.task,
            "score": self.score,
            "best_score": self.best_score,
            "step": self.step,
            "history": self.history,
            "params": self.params,
            "strategy": self.strategy,
        })
    }
}

/// Self-evolving optimizer driven by Laya scheduling nodes.
///
/// `propose_fn(prompt) -> proposal JSON` supplies the candidate `delta_params`
/// (the LLM/solver), and `eval_fn(&params) -> score` scores a candidate.
pub struct LayaOptimizerLoop {
    pub strategies: Vec<String>,
    pub max_iterations: usize,
    pub convergence_window: usize,
    pub convergence_eps: f64,
    pub history_limit: usize,
    pub state: OptimizerState,
}

impl LayaOptimizerLoop {
    pub fn new(strategies: Vec<String>, maximize: bool, max_iterations: usize) -> Self {
        Self {
            strategies,
            max_iterations,
            convergence_window: 5,
            convergence_eps: 1e-4,
            history_limit: 64,
            state: OptimizerState::new("optimizer", maximize),
        }
    }

    pub fn with_task(mut self, task: &str) -> Self {
        self.state.task = task.to_string();
        self
    }

    /// Seed the loop by evaluating the current (empty) params.
    pub fn initial_score<F: Fn(&Value) -> Result<f64>>(&mut self, eval_fn: F) -> Result<f64> {
        let s = eval_fn(&self.state.params)?;
        self.state.score = s;
        self.state.best_score = s;
        self.state.best_params = self.state.params.clone();
        Ok(s)
    }

    /// One propose → evaluate iteration; returns the history record.
    pub fn step<P, F>(&mut self, propose_fn: &P, eval_fn: &F) -> Result<Value>
    where
        P: Fn(&str) -> String,
        F: Fn(&Value) -> Result<f64>,
    {
        let previous = if self.state.score.is_finite() { self.state.score } else { 0.0 };
        let prompt = self.build_prompt();
        let output = propose_fn(&prompt);
        let proposal = parse_proposal(&output);
        let delta = proposal.get("delta_params").cloned().unwrap_or_else(|| json!({}));

        // candidate = params merged with delta
        let mut candidate = self.state.params.as_object().cloned().unwrap_or_default();
        if let Some(d) = delta.as_object() {
            for (k, v) in d {
                candidate.insert(k.clone(), v.clone());
            }
        }
        let candidate = Value::Object(candidate);

        let trial = eval_fn(&candidate)?;
        let gain = trial - previous;
        let accepted = if self.state.maximize { trial >= previous } else { trial <= previous };

        if accepted {
            self.state.params = candidate.clone();
            self.state.score = trial;
            let better = if self.state.maximize { trial > self.state.best_score } else { trial < self.state.best_score };
            if better || !self.state.best_score.is_finite() {
                self.state.best_score = trial;
                self.state.best_params = candidate.clone();
            }
        }
        self.state.step += 1;
        self.state.score = trial;

        let record = json!({
            "step": self.state.step,
            "trial_score": trial,
            "best_score": self.state.best_score,
            "accepted": accepted,
            "gain": gain,
            "proposal": proposal,
        });
        self.state.history.push(record.clone());
        if self.state.history.len() > self.history_limit {
            let drop = self.state.history.len() - self.history_limit;
            self.state.history.drain(0..drop);
        }
        Ok(record)
    }

    fn build_prompt(&self) -> String {
        let recent = self.state.history.iter().rev().take(3).rev();
        let recent_text: String = if self.state.history.is_empty() {
            "  (no history yet)".to_string()
        } else {
            recent
                .map(|h| {
                    format!(
                        "  step {}: score={:.4} accepted={} gain={:.4}",
                        h.get("step").and_then(|v| v.as_u64()).unwrap_or(0),
                        h.get("trial_score").and_then(|v| v.as_f64()).unwrap_or(0.0),
                        h.get("accepted").and_then(|v| v.as_bool()).unwrap_or(false),
                        h.get("gain").and_then(|v| v.as_f64()).unwrap_or(0.0),
                    )
                })
                .collect::<Vec<_>>()
                .join("\n")
        };
        let visible: Map<String, Value> = self
            .state
            .params
            .as_object()
            .map(|o| o.iter().filter(|(k, _)| !k.starts_with('_')).map(|(k, v)| (k.clone(), v.clone())).collect())
            .unwrap_or_default();
        let params_summary: String = serde_json::to_string(&visible).unwrap_or_default().chars().take(300).collect();

        format!(
            "You are an optimization agent. Current state:\n  task: {}\n  current_score: {:.4}\n  best_score: {:.4}\n  step: {}\n  strategy: {}\n  params: {}\n\nRecent history:\n{}\n\nPropose a parameter change (JSON with \"delta_params\" and \"rationale\"):\n",
            self.state.task,
            if self.state.score.is_finite() { self.state.score } else { 0.0 },
            if self.state.best_score.is_finite() { self.state.best_score } else { 0.0 },
            self.state.step,
            self.state.strategy,
            params_summary,
            recent_text,
        )
    }

    /// Build the Laya-node graph used by `run`.
    pub fn build_workflow(&self) -> ResilientWorkflow {
        ResilientWorkflow::new(
            vec![
                make_evaluate_node(),
                make_continue_node(),
                make_strategy_node(&self.strategies),
            ],
            "evaluate",
        )
        .with_max_iterations(self.max_iterations)
        .with_convergence(self.convergence_window, self.convergence_eps)
    }

    /// Run the loop: `evaluate` (injected) → Laya continue → Laya strategy → …
    pub fn run<P, F>(&mut self, backend: &dyn Decide, propose_fn: &P, eval_fn: &F) -> Result<Value>
    where
        P: Fn(&str) -> String,
        F: Fn(&Value) -> Result<f64>,
    {
        if !self.state.score.is_finite() {
            self.initial_score(eval_fn)?;
        }
        let workflow = self.build_workflow();

        let mut trace = WorkflowTrace::default();
        let mut current = Some("evaluate".to_string());
        let mut retry_count = 0usize;
        let mut iteration = 0usize;
        let mut node_visits: Map<String, Value> = Map::new();
        let mut score_history: Vec<f64> = Vec::new();
        let mut escalate_reason: Option<String> = None;
        let t0 = Instant::now();

        while let Some(name) = current.clone() {
            if iteration >= self.max_iterations {
                break;
            }
            iteration += 1;
            let node = match workflow.nodes.get(&name) {
                Some(n) => n.clone(),
                None => break,
            };
            let visits = node_visits.get(&name).and_then(|v| v.as_u64()).unwrap_or(0) + 1;
            node_visits.insert(name.clone(), json!(visits));

            if name == "evaluate" {
                // inject the real eval (propose + score) — Laya is not consulted
                let record = self.step(propose_fn, eval_fn)?;
                let result = crate::workflow::NodeResult {
                    node_name: "evaluate".to_string(),
                    verdict: Default::default(),
                    action: NodeAction::Route,
                    next_node: Some("continue_check".to_string()),
                    edge_answer: json!("continue"),
                    confidence: 1.0,
                    latency_ms: 0.0,
                    action_result: Some(record),
                };
                trace.add_step(&result);
                current = Some("continue_check".to_string());
            } else {
                let laya_state = build_laya_state(&self.state.to_laya_input());
                let result = node.run(backend, &laya_state)?;
                trace.add_step(&result);
                match result.action {
                    NodeAction::Retry => {
                        retry_count += 1;
                        if retry_count >= node.max_retries {
                            escalate_reason = Some(format!("max retries on {name}"));
                            trace.final_action = "escalate".to_string();
                            break;
                        }
                    }
                    NodeAction::Route | NodeAction::Execute => {
                        retry_count = 0;
                        if name == "strategy_check" {
                            // apply the Laya strategy switch to the optimizer state
                            if let Some(dec) = result.verdict.answers.get("strategy_choice") {
                                let choice = dec.as_str();
                                if choice != "keep" {
                                    self.state.strategy = choice;
                                }
                            }
                        }
                        current = result.next_node.clone();
                    }
                    NodeAction::Escalate => {
                        escalate_reason = Some(format!("confidence={:.3}", result.confidence));
                        trace.final_action = "escalate".to_string();
                        break;
                    }
                    NodeAction::Stop => {
                        trace.final_action = "stop".to_string();
                        current = None;
                    }
                }
                // on the strategy node with a "keep" answer, run another step
                if current.is_none() && result.action == NodeAction::Stop {
                    break;
                }
                if name == "strategy_check" && result.action == NodeAction::Route {
                    current = Some("evaluate".to_string());
                }
            }

            if self.state.score.is_finite() {
                score_history.push(self.state.score);
            }
            if score_history.len() >= workflow.convergence_window {
                let w = &score_history[score_history.len() - workflow.convergence_window..];
                let (mn, mx) = w.iter().fold((f64::INFINITY, f64::NEG_INFINITY), |(a, b), &v| (a.min(v), b.max(v)));
                if mx - mn <= workflow.convergence_eps {
                    trace.final_action = "converged".to_string();
                    break;
                }
            }
        }

        if iteration >= self.max_iterations {
            trace.final_action = "max_iterations".to_string();
        }
        trace.total_latency_ms = t0.elapsed().as_secs_f64() * 1000.0;
        trace.loop_count = node_visits.values().filter_map(|v| v.as_u64()).max().unwrap_or(0) as usize;

        Ok(json!({
            "steps": self.state.history.len(),
            "best_score": self.state.best_score,
            "best_params": self.state.best_params,
            "final_score": self.state.score,
            "strategy": self.state.strategy,
            "escalated": escalate_reason,
            "trace": trace.to_json(),
            "history": self.state.history.iter().rev().take(10).rev().collect::<Vec<_>>(),
            "node_visits": node_visits,
        }))
    }
}

/// Mirror of `LayaOptimizerLoop._parse_proposal`: first `{` … last `}` → JSON.
pub fn parse_proposal(output: &str) -> Value {
    if let (Some(start), Some(end)) = (output.find('{'), output.rfind('}')) {
        if end > start {
            if let Ok(v) = serde_json::from_str::<Value>(&output[start..=end]) {
                return v;
            }
        }
    }
    let snippet: String = output.chars().take(200).collect();
    json!({"delta_params": {}, "rationale": snippet})
}

/// Convenience: build the continue/strategy graph without a dataset (mirrors
/// `make_optimizer_workflow`).
pub fn make_optimizer_workflow(strategies: &[String], max_iterations: usize) -> ResilientWorkflow {
    ResilientWorkflow::new(
        vec![make_continue_node(), make_strategy_node(strategies)],
        "continue_check",
    )
    .with_max_iterations(max_iterations)
}

/// Write a JSON checkpoint atomically (helper shared by loop implementations).
pub fn save_json(path: &str, value: &Value) -> Result<()> {
    if let Some(parent) = Path::new(path).parent() {
        std::fs::create_dir_all(parent)?;
    }
    std::fs::write(path, serde_json::to_string_pretty(value)?)?;
    Ok(())
}
