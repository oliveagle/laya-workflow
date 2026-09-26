"""Tests for Laya workflow engine + optimizer integration.

Run: python -m pytest code/laya/tests/test_workflow_engine.py -v
"""
from __future__ import annotations

import json
import pytest
from dataclasses import dataclass, field
from typing import Any, Dict, List, Optional
from unittest.mock import MagicMock

import sys
from pathlib import Path
sys.path.insert(0, str(Path(__file__).resolve().parents[2]))

from code.laya.workflow_engine import (
    Edge, NodeAction, NodeResult, ResilientWorkflow, WorkflowNode, WorkflowTrace,
)


# ──────────────────────────────────────────────────────────────────────
# Mock Laya backend
# ──────────────────────────────────────────────────────────────────────

class MockVerdict:
    """Mimics LayaEngine.Verdict without loading the model."""
    def __init__(self, answers: dict):
        self.answers = answers  # {qid: MockDecision}

    def to_jsonable(self):
        return {k: {"answer": v.answer} for k, v in self.answers.items()}


@dataclass
class MockDecision:
    answer: Any
    confidence: float = 0.9
    probabilities: dict = field(default_factory=dict)


class MockLayaBackend:
    """Deterministic mock that returns preset answers."""

    def __init__(self, preset: Dict[str, Any]):
        """preset maps question_key -> (answer, confidence)."""
        self.preset = preset
        self.call_count = 0
        self.calls: List[Dict] = []

    def decide(self, state: Any, questions: Dict[str, Any]) -> MockVerdict:
        self.call_count += 1
        self.calls.append({"state": state, "questions": list(questions.keys())})
        answers = {}
        for qid in questions:
            if qid in self.preset:
                ans, conf = self.preset[qid]
                answers[qid] = MockDecision(
                    answer=ans,
                    confidence=conf,
                    probabilities={str(ans): conf, "other": 1 - conf},
                )
            else:
                answers[qid] = MockDecision(
                    answer="default", confidence=0.5,
                    probabilities={"default": 0.5},
                )
        return MockVerdict(answers)


# ──────────────────────────────────────────────────────────────────────
# Edge tests
# ──────────────────────────────────────────────────────────────────────

class TestEdge:
    def test_exact_match(self):
        edge = Edge(condition={"yes": "next_node", "no": "stop"})
        assert edge.resolve("yes", 0.9) == "next_node"
        assert edge.resolve("no", 0.9) == "stop"

    def test_default_fallback(self):
        edge = Edge(condition={"yes": "go"}, default="fallback")
        assert edge.resolve("maybe", 0.9) == "fallback"

    def test_no_match_no_default(self):
        edge = Edge(condition={"yes": "go"})
        assert edge.resolve("maybe", 0.9) is None

    def test_min_confidence_gate(self):
        edge = Edge(condition={"yes": "go"}, min_confidence=0.7)
        assert edge.resolve("yes", 0.9) == "go"
        assert edge.resolve("yes", 0.5) is None  # confidence too low

    def test_self_loop(self):
        edge = Edge(condition={"retry": "same_node"})
        assert edge.resolve("retry", 0.9) == "same_node"


# ──────────────────────────────────────────────────────────────────────
# WorkflowNode tests
# ──────────────────────────────────────────────────────────────────────

class TestWorkflowNode:
    def _make_node(self, **kwargs):
        defaults = {
            "name": "test_node",
            "questions": {
                "q1": {
                    "type": "choice",
                    "instructions": "test question",
                    "criteria": {"A": "option A", "B": "option B"},
                }
            },
            "edge": Edge(condition={"A": "next", "B": "stop"}),
            "primary_q": "q1",
        }
        defaults.update(kwargs)
        return WorkflowNode(**defaults)

    def test_route_on_confident_answer(self):
        node = self._make_node()
        backend = MockLayaBackend({"q1": ("A", 0.95)})
        result = node.run(backend, {"data": "test"})

        assert result.action == NodeAction.ROUTE
        assert result.next_node == "next"
        assert result.edge_answer == "A"
        assert result.confidence == 0.95
        assert result.latency_ms >= 0

    def test_escalate_on_low_confidence(self):
        edge = Edge(condition={"A": "next"}, min_confidence=0.8)
        node = self._make_node(edge=edge)
        backend = MockLayaBackend({"q1": ("A", 0.3)})
        result = node.run(backend, {})

        assert result.action == NodeAction.ESCALATE
        assert result.confidence == 0.3

    def test_retry_on_self_loop(self):
        node = self._make_node(
            edge=Edge(condition={"retry": "test_node"}),
        )
        backend = MockLayaBackend({"q1": ("retry", 0.9)})

        result = node.run(backend, {})
        assert result.action == NodeAction.RETRY
        assert result.next_node == "test_node"  # self-loop

    def test_action_fn_called(self):
        action_fn = MagicMock(return_value={"modified": True})
        node = self._make_node(action_fn=action_fn)
        backend = MockLayaBackend({"q1": ("A", 0.9)})
        state = {"data": "test"}
        node.run(backend, state)

        action_fn.assert_called_once()
        call_args = action_fn.call_args[0]
        assert call_args[0] == state  # state passed

    def test_state_fn_transforms_input(self):
        state_fn = lambda s: {"transformed": s.get("x", 0) * 2}
        node = self._make_node(state_fn=state_fn)
        backend = MockLayaBackend({"q1": ("A", 0.9)})
        node.run(backend, {"x": 21})

        # backend should see transformed state
        assert backend.calls[0]["state"]["transformed"] == 42


# ──────────────────────────────────────────────────────────────────────
# WorkflowTrace tests
# ──────────────────────────────────────────────────────────────────────

class TestWorkflowTrace:
    def test_add_step(self):
        trace = WorkflowTrace()
        result = NodeResult(
            node_name="n1", verdict=None, action=NodeAction.ROUTE,
            next_node="n2", edge_answer="A", confidence=0.9, latency_ms=42.0,
        )
        trace.add(result)
        assert len(trace.steps) == 1
        assert trace.steps[0]["node"] == "n1"
        assert trace.steps[0]["confidence"] == 0.9

    def test_to_dict(self):
        trace = WorkflowTrace()
        trace.final_action = "stop"
        trace.total_latency_ms = 100.0
        d = trace.to_dict()
        assert d["final_action"] == "stop"
        assert d["total_latency_ms"] == 100.0


# ──────────────────────────────────────────────────────────────────────
# ResilientWorkflow tests
# ──────────────────────────────────────────────────────────────────────

class TestResilientWorkflow:
    def test_simple_linear_flow(self):
        """A → B → STOP"""
        node_a = WorkflowNode(
            name="a",
            questions={"q": {"type": "choice", "instructions": "test", "criteria": {"go": "yes"}}},
            edge=Edge(condition={"go": "b"}),
            primary_q="q",
        )
        node_b = WorkflowNode(
            name="b",
            questions={"q": {"type": "choice", "instructions": "test", "criteria": {"done": "yes"}}},
            edge=Edge(condition={"done": "STOP"}),
            primary_q="q",
        )
        wf = ResilientWorkflow(nodes={"a": node_a, "b": node_b}, start="a")

        # different answers per node
        call_count = [0]
        class PerNodeBackend:
            def decide(self, state, questions):
                call_count[0] += 1
                if call_count[0] == 1:  # node_a
                    return MockVerdict({"q": MockDecision("go", 0.95)})
                else:  # node_b
                    return MockVerdict({"q": MockDecision("done", 0.99)})

        result = wf.run(PerNodeBackend(), {"x": 1})

        assert result["trace"]["final_action"] == "stop"
        assert result["iterations"] == 2
        assert result["node_visits"] == {"a": 1, "b": 1}

    def test_loop_detection(self):
        """Node A loops to itself → detected after 3 visits."""
        node_a = WorkflowNode(
            name="a",
            questions={"q": {"type": "choice", "instructions": "test", "criteria": {"loop": "again"}}},
            edge=Edge(condition={"loop": "a"}),
            primary_q="q",
            max_retries=10,  # allow many retries
        )
        wf = ResilientWorkflow(nodes={"a": node_a}, start="a", max_iterations=10)

        backend = MockLayaBackend({"q": ("loop", 0.9)})
        result = wf.run(backend, {})

        # should detect loop and stop (via max_iterations or loop detection)
        assert result["trace"]["loop_detected"] or result["trace"]["final_action"] == "max_iterations"

    def test_escalation_on_low_confidence(self):
        """Low confidence → ESCALATE."""
        edge = Edge(condition={"yes": "next"}, min_confidence=0.8)
        node = WorkflowNode(
            name="uncertain",
            questions={"q": {"type": "choice", "instructions": "test", "criteria": {"yes": "y"}}},
            edge=edge,
            primary_q="q",
        )
        wf = ResilientWorkflow(nodes={"uncertain": node}, start="uncertain")

        backend = MockLayaBackend({"q": ("yes", 0.3)})  # low confidence
        result = wf.run(backend, {})

        assert result["trace"]["final_action"] == "escalate"

    def test_max_iterations_safety(self):
        """Workflow stops at max_iterations."""
        node = WorkflowNode(
            name="forever",
            questions={"q": {"type": "choice", "instructions": "test", "criteria": {"go": "y"}}},
            edge=Edge(condition={"go": "forever"}),  # self-loop
            primary_q="q",
            max_retries=100,  # high retries so ESCALATE doesn't trigger first
        )
        wf = ResilientWorkflow(nodes={"forever": node}, start="forever", max_iterations=5)

        backend = MockLayaBackend({"q": ("go", 0.9)})
        result = wf.run(backend, {})

        assert result["trace"]["final_action"] == "max_iterations"
        assert result["iterations"] == 5

    def test_describe(self):
        """describe() returns graph structure."""
        node = WorkflowNode(
            name="n",
            questions={"q": {"type": "choice", "instructions": "t", "criteria": {"a": "A"}}},
            edge=Edge(condition={"a": "next"}),
        )
        wf = ResilientWorkflow(nodes={"n": node}, start="n")
        desc = wf.describe()

        assert desc["start"] == "n"
        assert "n" in desc["nodes"]
        assert desc["nodes"]["n"]["primary_q"] == "q"


# ──────────────────────────────────────────────────────────────────────
# Optimizer integration tests
# ──────────────────────────────────────────────────────────────────────

class TestOptimizerIntegration:
    def test_make_optimizer_workflow(self):
        from code.laya.optimizer_integration import make_optimizer_workflow
        wf = make_optimizer_workflow(
            strategies=["alpha", "beta"],
            max_iterations=20,
        )
        assert "continue_check" in wf.nodes
        assert "strategy_check" in wf.nodes
        assert wf.max_iterations == 20

    def test_continue_node_stop_decision(self):
        from code.laya.optimizer_integration import make_continue_node
        node = make_continue_node()
        backend = MockLayaBackend({
            "should_continue": ("stop", 0.95),
            "confidence_assessment": ("confident", 0.8),
        })
        result = node.run(backend, {
            "task": "test",
            "score": -5.0,
            "best_score": -5.0,
            "step": 20,
            "history": [],
            "params": {},
            "strategy": "default",
        })
        assert result.edge_answer == "stop"

    def test_continue_node_continue_decision(self):
        from code.laya.optimizer_integration import make_continue_node
        node = make_continue_node()
        backend = MockLayaBackend({
            "should_continue": ("continue", 0.9),
            "confidence_assessment": ("confident", 0.8),
        })
        result = node.run(backend, {
            "task": "test",
            "score": -3.0,
            "best_score": -5.0,
            "step": 5,
            "history": [],
            "params": {},
            "strategy": "default",
        })
        assert result.edge_answer == "continue"
        assert result.next_node == "evaluate"

    def test_strategy_node_switch(self):
        from code.laya.optimizer_integration import make_strategy_node
        node = make_strategy_node(["aggressive", "conservative"])
        backend = MockLayaBackend({
            "strategy_choice": ("aggressive", 0.9),
            "switch_urgency": ("urgent", 0.8),
        })
        state = {
            "task": "test", "score": -5.0, "best_score": -5.0,
            "step": 10, "history": [], "params": {}, "strategy": "conservative",
        }
        result = node.run(backend, state)
        assert result.edge_answer == "aggressive"

    def test_strategy_node_keep(self):
        from code.laya.optimizer_integration import make_strategy_node
        node = make_strategy_node(["aggressive", "conservative"])
        backend = MockLayaBackend({
            "strategy_choice": ("keep", 0.9),
            "switch_urgency": ("no rush", 0.8),
        })
        result = node.run(backend, {"strategy": "conservative"})
        assert result.edge_answer == "keep"


# ──────────────────────────────────────────────────────────────────────
# Edge case: empty workflow
# ──────────────────────────────────────────────────────────────────────

class TestEdgeCases:
    def test_empty_workflow(self):
        wf = ResilientWorkflow(nodes={}, start="missing")
        backend = MockLayaBackend({})
        result = wf.run(backend, {})
        assert result["trace"]["final_action"] == "error_node_missing"

    def test_single_node_stop(self):
        node = WorkflowNode(
            name="only",
            questions={"q": {"type": "choice", "instructions": "t", "criteria": {"done": "y"}}},
            edge=Edge(condition={"done": "STOP"}),
            primary_q="q",
        )
        wf = ResilientWorkflow(nodes={"only": node}, start="only")
        backend = MockLayaBackend({"q": ("done", 0.99)})
        result = wf.run(backend, {})
        assert result["trace"]["final_action"] == "stop"
        assert result["iterations"] == 1
