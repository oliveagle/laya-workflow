"""Laya Workflow Engine — resilient workflow graphs with Laya scheduling nodes.

Core idea: replace hardcoded if/elif workflow branching with Laya decision
nodes that output calibrated probabilities.  Each node asks typed questions,
routes to the next node based on answers, and supports loops with convergence
detection.

Architecture::

    ┌──────────────┐
    │  Input State  │
    └──────┬───────┘
           ▼
    ┌──────────────┐     ┌──────────────┐     ┌──────────────┐
    │ Laya Node A  │────▶│ Laya Node B  │────▶│   Output     │
    │ (classify)   │     │ (validate)   │     │              │
    └──────┬───────┘     └──────┬───────┘     └──────────────┘
           │ retry               │ fail
           └───────────┐  ┌─────┘
                       ▼  ▼
                  (loop / fallback)

Design principles:
  - Every node is a Laya forward pass (~80ms CPU / ~38ms GPU)
  - Edges carry probability thresholds, not hard boolean
  - Loops have max_iter + convergence detection + confidence gate
  - Full execution history for audit / debugging
  - Node actions: ROUTE (go to next node), EXECUTE (run a function),
    STOP (workflow done), ESCALATE (needs human), RETRY (re-run current)
"""
from __future__ import annotations

import json
import time
import logging
from dataclasses import dataclass, field
from enum import Enum
from typing import Any, Callable, Dict, List, Optional, Protocol, Tuple, Union

logger = logging.getLogger(__name__)


# ──────────────────────────────────────────────────────────────────────
# Core types
# ──────────────────────────────────────────────────────────────────────

class NodeAction(str, Enum):
    """What a node tells the engine to do after deciding."""
    ROUTE = "route"           # follow an edge to the next node
    EXECUTE = "execute"       # run an action function, then route
    STOP = "stop"             # workflow finished successfully
    ESCALATE = "escalate"     # needs human intervention
    RETRY = "retry"           # re-run the current node (bounded)


class Edge:
    """A weighted edge from one node to another.

    `condition` is a dict mapping answer values to destination node names.
    `default` is the fallback if no condition matches.
    `min_confidence` gates the edge: if the node's confidence is below this
    threshold, the edge is not taken (ESCALATE instead).
    """

    def __init__(
        self,
        condition: Dict[str, str],
        default: str | None = None,
        min_confidence: float = 0.0,
    ):
        self.condition = condition
        self.default = default
        self.min_confidence = min_confidence

    def resolve(self, answer: Any, confidence: float) -> str | None:
        """Return the destination node name, or None if no match."""
        if confidence < self.min_confidence:
            return None
        return self.condition.get(str(answer), self.default)


@dataclass
class NodeResult:
    """Output from running a single node."""
    node_name: str
    verdict: Any  # Laya Verdict (not imported to avoid circular)
    action: NodeAction
    next_node: str | None
    edge_answer: Any
    confidence: float
    latency_ms: float
    action_result: Any = None  # if EXECUTE, the function's return value


@dataclass
class WorkflowTrace:
    """Full execution trace for debugging / audit."""
    steps: List[Dict[str, Any]] = field(default_factory=list)
    total_latency_ms: float = 0.0
    loop_detected: bool = False
    loop_count: int = 0
    final_action: str = ""

    def add(self, result: NodeResult) -> None:
        self.steps.append({
            "node": result.node_name,
            "action": result.action.value,
            "next": result.next_node,
            "answer": result.edge_answer,
            "confidence": round(result.confidence, 4),
            "latency_ms": round(result.latency_ms, 1),
        })

    def to_dict(self) -> Dict[str, Any]:
        return {
            "steps": self.steps,
            "total_latency_ms": round(self.total_latency_ms, 1),
            "loop_detected": self.loop_detected,
            "loop_count": self.loop_count,
            "final_action": self.final_action,
            "step_count": len(self.steps),
        }


# ──────────────────────────────────────────────────────────────────────
# Laya backend protocol (allows mock in tests)
# ──────────────────────────────────────────────────────────────────────

class LayaBackend(Protocol):
    """Anything that can decide(state, questions) -> Verdict."""
    def decide(self, state: Any, questions: Dict[str, Any]) -> Any: ...


# ──────────────────────────────────────────────────────────────────────
# WorkflowNode — a single Laya scheduling junction
# ──────────────────────────────────────────────────────────────────────

class WorkflowNode:
    """A Laya scheduling node in a workflow graph.

    Each node:
      1. Builds a state dict from the current workflow state
      2. Asks Laya a set of typed questions
      3. Picks an edge based on the primary answer + confidence
      4. Optionally runs an action function before routing

    Parameters:
      name:        unique node identifier
      questions:   Laya-style questions dict
      edge:        Edge object for routing
      action_fn:   optional callable(state, verdict) -> transformed_state
      primary_q:   which question key drives the edge routing (first by default)
      max_retries: how many times RETRY is allowed before ESCALATE
      state_fn:    callable(full_state) -> laya_state dict (default: pass-through)
    """

    def __init__(
        self,
        name: str,
        questions: Dict[str, Any],
        edge: Edge,
        action_fn: Callable[[dict, Any], dict] | None = None,
        primary_q: str | None = None,
        max_retries: int = 2,
        state_fn: Callable[[dict], dict] | None = None,
    ):
        self.name = name
        self.questions = questions
        self.edge = edge
        self.action_fn = action_fn
        self.primary_q = primary_q or next(iter(questions))
        self.max_retries = max_retries
        self.state_fn = state_fn or (lambda s: s)

    def run(
        self,
        backend: LayaBackend,
        state: dict[str, Any],
    ) -> NodeResult:
        """Execute this node: ask Laya, resolve edge, optionally run action.

        Note: retry counting is handled by the workflow runner, not here.
        This method just returns RETRY when the edge points to self.
        """
        laya_state = self.state_fn(state)
        t0 = time.time()
        verdict = backend.decide(laya_state, self.questions)
        latency_ms = (time.time() - t0) * 1000

        primary_answer = verdict.answers[self.primary_q].answer
        confidence = verdict.answers[self.primary_q].confidence

        # resolve edge
        next_node = self.edge.resolve(primary_answer, confidence)

        if next_node is None:
            # confidence too low → ESCALATE
            action = NodeAction.ESCALATE
        elif next_node == "STOP":
            # explicit terminal → STOP
            action = NodeAction.STOP
        elif next_node == self.name:
            # self-loop → RETRY (runner manages the count)
            action = NodeAction.RETRY
        else:
            action = NodeAction.ROUTE

        # run action function if present
        action_result = None
        if self.action_fn is not None:
            action_result = self.action_fn(state, verdict)

        return NodeResult(
            node_name=self.name,
            verdict=verdict,
            action=action,
            next_node=next_node,
            edge_answer=primary_answer,
            confidence=confidence,
            latency_ms=latency_ms,
            action_result=action_result,
        )


# ──────────────────────────────────────────────────────────────────────
# ResilientWorkflow — graph runner with loop detection
# ──────────────────────────────────────────────────────────────────────

class ResilientWorkflow:
    """A workflow graph with Laya nodes at each junction.

    Usage::

        engine = ResilientWorkflow(
            nodes={"start": node_a, "validate": node_b, "done": node_c},
            start="start",
            max_iterations=20,
        )
        result = engine.run(backend, initial_state)
        print(result["trace"])
    """

    def __init__(
        self,
        nodes: Dict[str, WorkflowNode],
        start: str,
        max_iterations: int = 50,
        convergence_window: int = 5,
        convergence_eps: float = 1e-4,
        on_escalate: Callable[[NodeResult, dict], dict] | None = None,
        on_action: Callable[[str, dict, Any], dict] | None = None,
    ):
        self.nodes = nodes
        self.start = start
        self.max_iterations = max_iterations
        self.convergence_window = convergence_window
        self.convergence_eps = convergence_eps
        self.on_escalate = on_escalate  # custom escalate handler
        self.on_action = on_action      # custom action dispatcher

    def run(
        self,
        backend: LayaBackend,
        state: Dict[str, Any],
    ) -> Dict[str, Any]:
        """Run the workflow graph to completion.

        Returns dict with keys: result, trace, history, loop_info.
        """
        trace = WorkflowTrace()
        history: List[Dict[str, Any]] = []
        current = self.start
        retry_count = 0
        iteration = 0
        score_history: List[float] = []
        node_visit_counts: Dict[str, int] = {}

        t0 = time.time()

        while current is not None and iteration < self.max_iterations:
            iteration += 1

            if current not in self.nodes:
                logger.error(f"Node '{current}' not found in workflow")
                trace.final_action = "error_node_missing"
                break

            node = self.nodes[current]
            node_visit_counts[current] = node_visit_counts.get(current, 0) + 1

            result = node.run(backend, state)
            trace.add(result)

            # record score if available (for convergence detection)
            if "score" in state:
                score_history.append(state["score"])

            step_record = {
                "iteration": iteration,
                "node": current,
                "action": result.action.value,
                "answer": result.edge_answer,
                "confidence": round(result.confidence, 4),
                "latency_ms": round(result.latency_ms, 1),
            }

            if result.action == NodeAction.RETRY:
                retry_count += 1
                if retry_count >= node.max_retries:
                    # too many retries on same node → ESCALATE
                    trace.final_action = "escalate"
                    step_record["detail"] = f"max retries ({retry_count}) on {current}"
                    history.append(step_record)
                    break
                # stay on same node
                step_record["detail"] = f"retry {retry_count}/{node.max_retries}"

            elif result.action == NodeAction.ROUTE:
                retry_count = 0
                current = result.next_node
                step_record["detail"] = f"routed to {current}"

            elif result.action == NodeAction.ESCALATE:
                trace.final_action = "escalate"
                if self.on_escalate:
                    state = self.on_escalate(result, state)
                step_record["detail"] = f"escalated from {current} (confidence={result.confidence:.3f})"
                history.append(step_record)
                break

            elif result.action == NodeAction.STOP:
                trace.final_action = "stop"
                current = None
                step_record["detail"] = "workflow stopped"

            elif result.action == NodeAction.EXECUTE:
                retry_count = 0
                if self.on_action and result.action_result:
                    state = self.on_action(current, state, result.action_result)
                current = result.next_node
                step_record["detail"] = f"executed + routed to {current}"

            history.append(step_record)

            # convergence detection (score-based)
            if len(score_history) >= self.convergence_window:
                window = score_history[-self.convergence_window:]
                if max(window) - min(window) <= self.convergence_eps:
                    trace.final_action = "converged"
                    trace.loop_detected = True
                    logger.info(f"Converged after {iteration} iterations "
                                f"(score delta {max(window)-min(window):.6f} <= {self.convergence_eps})")
                    break

            # loop detection (visit-based)
            if current and node_visit_counts.get(current, 0) >= 3 and not trace.loop_detected:
                trace.loop_detected = True
                trace.loop_count = node_visit_counts[current]
                logger.warning(f"Loop detected: node '{current}' visited {trace.loop_count} times")

        if iteration >= self.max_iterations:
            trace.final_action = "max_iterations"

        trace.total_latency_ms = (time.time() - t0) * 1000
        trace.loop_count = max(node_visit_counts.values()) if node_visit_counts else 0

        return {
            "result": state,
            "trace": trace.to_dict(),
            "history": history,
            "node_visits": node_visit_counts,
            "iterations": iteration,
        }

    def describe(self) -> Dict[str, Any]:
        """Return a human-readable graph description."""
        edges = {}
        for name, node in self.nodes.items():
            edges[name] = {
                "questions": list(node.questions.keys()),
                "primary_q": node.primary_q,
                "edge_condition": node.edge.condition,
                "edge_default": node.edge.default,
                "min_confidence": node.edge.min_confidence,
                "has_action": node.action_fn is not None,
                "max_retries": node.max_retries,
            }
        return {
            "start": self.start,
            "max_iterations": self.max_iterations,
            "nodes": edges,
        }
