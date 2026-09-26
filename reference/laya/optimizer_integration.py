"""Self-evolving optimizer loop driven by Laya scheduling nodes.

Replaces the hardcoded plateau detection and strategy selection in
optimizer_loop.py with a Laya-node workflow graph.  Each iteration:

  1. Laya observes the current state (score, params, history)
  2. Decides: should we continue? switch strategy? converge?
  3. Routes to the appropriate action

This makes the loop adaptive (Laya can detect patterns the fixed epsilon
window misses) and auditable (full trace of why each decision was made).

Architecture::

    ┌─────────────┐
    │   evaluate   │  (run eval_fn, get score)
    └──────┬──────┘
           ▼
    ┌─────────────┐     converged / score good
    │  Laya:      │───────────────────────────▶ STOP
    │  continue?  │
    └──────┬──────┘
           │ continue
           ▼
    ┌─────────────┐     strategy changed
    │  Laya:      │───────────────────────────▶ switch strategy
    │  strategy?  │
    └──────┬──────┘
           │ same strategy
           ▼
    ┌─────────────┐
    │  propose +  │  (LLM proposes delta_params)
    │  prefilter  │  (Laya gates: skip eval if unlikely to help)
    └──────┬──────┘
           ▼
    ┌─────────────┐
    │  evaluate   │  (run eval_fn, get trial_score)
    └──────┬──────┘
           │
           └───▶ back to evaluate
"""
from __future__ import annotations

import copy
import json
import time
from dataclasses import dataclass, field
from typing import Any, Callable, Dict, List, Optional

import sys
from pathlib import Path
_REPO = str(Path(__file__).resolve().parents[2])
if _REPO not in sys.path:
    sys.path.insert(0, _REPO)

from code.laya.workflow_engine import (
    Edge, NodeAction, NodeResult, ResilientWorkflow, WorkflowNode, WorkflowTrace,
)
from code.laya.engine import LayaEngine, Verdict


# ──────────────────────────────────────────────────────────────────────
# State transformer — build Laya input from optimizer state
# ──────────────────────────────────────────────────────────────────────

def _build_laya_state(optim_state: dict) -> dict:
    """Convert optimizer working state into a Laya-friendly dict."""
    history = optim_state.get("history", [])
    recent = history[-5:] if history else []

    return {
        "task": optim_state.get("task", "unknown"),
        "current_score": round(optim_state.get("score", -999.0), 6),
        "best_score": round(optim_state.get("best_score", -999.0), 6),
        "score_delta": round(
            optim_state.get("score", 0) - (recent[-1]["trial_score"] if recent else 0), 6
        ),
        "step": optim_state.get("step", 0),
        "history_len": len(history),
        "recent_scores": [round(h["trial_score"], 6) for h in recent],
        "recent_accepted": [h.get("accepted", False) for h in recent],
        "consecutive_rejects": _count_consecutive_rejects(recent),
        "strategy": optim_state.get("strategy", "unknown"),
        "params_summary": json.dumps(optim_state.get("params", {}), default=str)[:200],
    }


def _count_consecutive_rejects(history: list) -> int:
    count = 0
    for h in reversed(history):
        if h.get("accepted", False):
            break
        count += 1
    return count


# ──────────────────────────────────────────────────────────────────────
# Node factory functions
# ──────────────────────────────────────────────────────────────────────

def make_continue_node() -> WorkflowNode:
    """Laya decides: continue optimizing or stop?"""
    questions = {
        "should_continue": {
            "type": "choice",
            "instructions": (
                "Based on the current score, best score, recent history, "
                "and convergence pattern, should the optimizer continue? "
            ),
            "criteria": {
                "stop": (
                    "the score has plateaued, we've converged, "
                    "or further optimization is unlikely to help"
                ),
                "continue": (
                    "there's room for improvement, the score is still "
                    "climbing, or recent changes show promise"
                ),
            },
        },
        "confidence_assessment": {
            "type": "score",
            "instructions": "How confident are you that continuing will improve the score?",
            "criteria": [
                "not confident — likely waste of compute",
                "somewhat confident — might find small gains",
                "very confident — clear improvement trajectory",
            ],
        },
    }

    edge = Edge(
        condition={"stop": "STOP", "continue": "evaluate"},
        default="evaluate",
        min_confidence=0.0,
    )

    def action(state: dict, verdict: Verdict) -> dict:
        """Record the continue decision in state."""
        state["_continue_decision"] = {
            "answer": verdict.answers["should_continue"].answer,
            "confidence": verdict.answers["should_continue"].confidence,
        }
        return state

    return WorkflowNode(
        name="continue_check",
        questions=questions,
        edge=edge,
        action_fn=action,
        primary_q="should_continue",
        state_fn=_build_laya_state,
    )


def make_strategy_node(strategies: List[str]) -> WorkflowNode:
    """Laya decides: switch strategy or keep the current one?"""
    criteria = {s: f"use strategy {s}" for s in strategies}
    criteria["keep"] = "keep the current strategy, don't switch"

    questions = {
        "strategy_choice": {
            "type": "choice",
            "instructions": (
                "Given the current score trajectory, recent acceptance rate, "
                "and consecutive rejections, should we switch optimization "
                "strategy or keep the current one?"
            ),
            "criteria": criteria,
        },
        "switch_urgency": {
            "type": "score",
            "instructions": "How urgently should we switch strategy?",
            "criteria": [
                "no rush — current strategy is working",
                "moderate — current strategy is slowing down",
                "urgent — current strategy is stuck, switch now",
            ],
        },
    }

    edge = Edge(
        condition={s: s for s in strategies},
        default="keep",
        min_confidence=0.3,
    )

    def action(state: dict, verdict: Verdict) -> dict:
        choice = verdict.answers["strategy_choice"].answer
        if choice != "keep":
            state["_strategy_switch"] = {
                "from": state.get("strategy", "unknown"),
                "to": choice,
                "confidence": verdict.answers["strategy_choice"].confidence,
            }
            state["strategy"] = choice
        return state

    return WorkflowNode(
        name="strategy_check",
        questions=questions,
        edge=edge,
        action_fn=action,
        primary_q="strategy_choice",
        state_fn=_build_laya_state,
    )


def make_evaluate_node() -> WorkflowNode:
    """Evaluate node — just marks that evaluation happened.

    The actual eval is done by the workflow's on_action callback.
    This node is a routing placeholder that always goes to continue_check.
    """
    return WorkflowNode(
        name="evaluate",
        questions={},  # no Laya questions — this is an action node
        edge=Edge(condition={}, default="continue_check"),
        max_retries=0,
    )


# ──────────────────────────────────────────────────────────────────────
# LayaOptimizerLoop — the full optimizer
# ──────────────────────────────────────────────────────────────────────

@dataclass
class LayaOptimizerLoop:
    """Self-evolving optimizer with Laya scheduling nodes.

    Drop-in replacement for SelfEvolvingLoop that uses Laya's calibrated
    decisions instead of hardcoded plateau detection.

    Usage::

        loop = LayaOptimizerLoop(
            workflow=my_workflow,
            dataset=my_dataset,
            strategies=["aggressive_step", "conservative_step", "random_restart"],
        )
        result = loop.run(call_model, max_steps=100)
    """
    workflow: Any  # Workflow protocol
    dataset: Any
    strategies: List[str] = field(default_factory=lambda: ["default"])
    maximize: bool = True
    history_limit: int = 64
    max_iterations: int = 100
    convergence_window: int = 5
    convergence_eps: float = 1e-4
    prefilter: Any = None  # Optional LayaPrefilter
    meta: Any = None       # Optional MetaController
    verbose: bool = False

    def __post_init__(self):
        from ..workflow_state import WorkflowState
        self.state = WorkflowState(task=self.workflow.name if hasattr(self.workflow, "name") else "optimizer")
        self.state.score = float("-inf")
        self.history: List[Dict[str, Any]] = []

    def _build_workflow(self) -> ResilientWorkflow:
        """Construct the Laya-node workflow graph."""
        continue_node = make_continue_node()
        strategy_node = make_strategy_node(self.strategies)
        evaluate_node = make_evaluate_node()

        return ResilientWorkflow(
            nodes={
                "evaluate": evaluate_node,
                "continue_check": continue_node,
                "strategy_check": strategy_node,
            },
            start="evaluate",
            max_iterations=self.max_iterations,
            convergence_window=self.convergence_window,
            convergence_eps=self.convergence_eps,
            on_action=self._handle_action,
            on_escalate=self._handle_escalate,
        )

    def _handle_action(
        self, node_name: str, state: dict, action_result: Any
    ) -> dict:
        """Dispatch actions from node action functions."""
        if node_name == "evaluate" and action_result is not None:
            state.update(action_result)
        return state

    def _handle_escalate(self, result: Any, state: dict) -> dict:
        """Handle escalation — record and continue with best state."""
        state["_escalated"] = True
        state["_escalate_reason"] = f"confidence={result.confidence:.3f}"
        if self.verbose:
            print(f"[LAYA] ESCALATE from {result.node_name}: {result.verdict.answers}")
        return state

    def _call_model_and_eval(
        self, call_model: Callable[[str], str]
    ) -> Dict[str, Any]:
        """Run one iteration: propose + evaluate."""
        previous_score = self.state.score if self.state.score != float("-inf") else 0.0

        # build prompt from state
        prompt = self._build_prompt()
        model_output = call_model(prompt)

        # parse proposal
        proposal = self._parse_proposal(model_output)
        delta_params = proposal.get("delta_params", {})

        # apply delta
        candidate_params = {**self.state.params, **delta_params}

        # evaluate
        trial_score = self.workflow.evaluate(candidate_params, self.dataset)
        gain = trial_score - previous_score
        accepted = (
            trial_score >= previous_score if self.maximize
            else trial_score <= previous_score
        )

        # update state
        if accepted:
            self.state.params = candidate_params
            self.state.score = trial_score
            if self.maximize:
                if trial_score > self.state.best_score:
                    self.state.best_score = trial_score
                    self.state.best_params = copy.deepcopy(candidate_params)
            else:
                if trial_score < self.state.best_score:
                    self.state.best_score = trial_score
                    self.state.best_params = copy.deepcopy(candidate_params)

        self.state.step += 1
        self.state.score = trial_score

        record = {
            "step": self.state.step,
            "trial_score": trial_score,
            "best_score": self.state.best_score,
            "accepted": accepted,
            "gain": gain,
            "proposal": proposal,
        }
        self.history.append(record)
        self.state.history = self.history[-self.history_limit:]

        return record

    def _build_prompt(self) -> str:
        """Build a prompt for the LLM optimizer."""
        recent = self.history[-3:] if self.history else []
        recent_text = "\n".join(
            f"  step {h['step']}: score={h['trial_score']:.4f} "
            f"accepted={h['accepted']} gain={h['gain']:.4f}"
            for h in recent
        ) or "  (no history yet)"

        return f"""You are an optimization agent. Current state:
  task: {self.state.task}
  current_score: {self.state.score:.4f}
  best_score: {self.state.best_score:.4f}
  step: {self.state.step}
  strategy: {self.state.params.get('_strategy', 'default')}
  params: {json.dumps({k: v for k, v in self.state.params.items() if not k.startswith('_')}, default=str)[:300]}

Recent history:
{recent_text}

Propose a parameter change (JSON with "delta_params" and "rationale"):
"""

    def _parse_proposal(self, output: str) -> dict:
        """Parse LLM output into a proposal dict."""
        try:
            # try to find JSON in the output
            start = output.index("{")
            end = output.rindex("}") + 1
            return json.loads(output[start:end])
        except (ValueError, json.JSONDecodeError):
            return {"delta_params": {}, "rationale": output[:200]}

    def initial_score(self) -> float:
        """Evaluate default params to seed the state."""
        params = self.workflow.coerce_params(self.state.params)
        score = self.workflow.evaluate(params, self.dataset)
        self.state.score = score
        self.state.best_score = score
        self.state.best_params = copy.deepcopy(params)
        return score

    def run(
        self,
        call_model: Callable[[str], str],
        *,
        max_steps: int | None = None,
    ) -> Dict[str, Any]:
        """Run the Laya-optimized loop.

        Returns dict with: steps, best_score, best_params, final_score,
        trace, history, node_visits.
        """
        if max_steps is not None:
            self.max_iterations = max_steps

        if self.state.score == float("-inf"):
            self.initial_score()

        workflow = self._build_workflow()
        backend = LayaEngine.shared()

        # The workflow graph's evaluate node triggers our eval function.
        # We wrap the workflow run to inject the eval step.
        result = self._run_with_eval(backend, workflow)

        return {
            "steps": len(self.history),
            "best_score": self.state.best_score,
            "best_params": copy.deepcopy(self.state.best_params),
            "final_score": self.state.score,
            "trace": result.get("trace", {}),
            "history": self.history[-10:],
            "node_visits": result.get("node_visits", {}),
        }

    def _run_with_eval(self, backend, workflow: ResilientWorkflow) -> dict:
        """Run the workflow, injecting eval at the evaluate node.

        This is a custom runner because the evaluate node needs to run
        the actual eval_fn (not Laya), and the result feeds back into
        the state that Laya observes.
        """
        trace = WorkflowTrace()
        history: List[Dict[str, Any]] = []
        current = "evaluate"
        retry_count = 0
        iteration = 0
        node_visit_counts: Dict[str, int] = {}
        score_history: List[float] = []

        t0 = time.time()

        while current is not None and iteration < self.max_iterations:
            iteration += 1

            if current not in workflow.nodes:
                break

            node = workflow.nodes[current]
            node_visit_counts[current] = node_visit_counts.get(current, 0) + 1

            if current == "evaluate":
                # inject the actual eval
                record = self._call_model_and_eval(lambda p: "")
                action_result = record
                result = NodeResult(
                    node_name="evaluate",
                    verdict=None,
                    action=NodeAction.ROUTE,
                    next_node="continue_check",
                    edge_answer="continue",
                    confidence=1.0,
                    latency_ms=0.0,
                    action_result=action_result,
                )
                current = "continue_check"
            else:
                # Laya node
                state_dict = _build_laya_state({
                    "task": self.state.task,
                    "score": self.state.score,
                    "best_score": self.state.best_score,
                    "step": self.state.step,
                    "history": self.history,
                    "params": self.state.params,
                    "strategy": self.state.params.get("_strategy", "default"),
                })

                result = node.run(backend, state_dict)

                if result.action == NodeAction.RETRY:
                    retry_count = result.retry_count + 1
                elif result.action in (NodeAction.ROUTE, NodeAction.EXECUTE):
                    retry_count = 0
                    current = result.next_node
                elif result.action == NodeAction.ESCALATE:
                    trace.final_action = "escalate"
                    break
                elif result.action == NodeAction.STOP:
                    trace.final_action = "stop"
                    current = None
                    break

            trace.add(result)
            if self.state.score != float("-inf"):
                score_history.append(self.state.score)

            # convergence check
            if len(score_history) >= workflow.convergence_window:
                window = score_history[-workflow.convergence_window:]
                if max(window) - min(window) <= workflow.convergence_eps:
                    trace.final_action = "converged"
                    break

        if iteration >= self.max_iterations:
            trace.final_action = "max_iterations"

        trace.total_latency_ms = (time.time() - t0) * 1000

        return {
            "trace": trace.to_dict(),
            "node_visits": node_visit_counts,
            "iterations": iteration,
        }


# ──────────────────────────────────────────────────────────────────────
# Convenience: build a workflow for any optimizer
# ──────────────────────────────────────────────────────────────────────

def make_optimizer_workflow(
    strategies: List[str] = None,
    max_iterations: int = 50,
    convergence_window: int = 5,
    convergence_eps: float = 1e-4,
) -> ResilientWorkflow:
    """Create a reusable Laya-node workflow for optimization loops.

    This builds the graph without tying it to a specific workflow/dataset,
    so it can be composed with any optimizer that provides a `state` dict.
    """
    strategies = strategies or ["default"]

    continue_node = make_continue_node()
    strategy_node = make_strategy_node(strategies)

    return ResilientWorkflow(
        nodes={
            "continue_check": continue_node,
            "strategy_check": strategy_node,
        },
        start="continue_check",
        max_iterations=max_iterations,
        convergence_window=convergence_window,
        convergence_eps=convergence_eps,
    )
