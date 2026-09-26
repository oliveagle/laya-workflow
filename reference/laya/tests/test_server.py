"""Tests for Laya workflow server + pre-built workflow graphs.

Run: python3.12 -m pytest code/laya/tests/test_server.py -v
"""
from __future__ import annotations

import json
import sys
from pathlib import Path
from dataclasses import dataclass, field
from typing import Any, Dict

sys.path.insert(0, str(Path(__file__).resolve().parents[2]))

from code.laya.workflow_engine import (
    Edge, NodeAction, NodeResult, ResilientWorkflow, WorkflowNode,
)
from code.laya.server import (
    _build_triage_graph, _build_review_loop_graph, _build_continue_check_graph,
    WORKFLOWS,
)


# ──────────────────────────────────────────────────────────────────────
# Mock backend (reuse from test_workflow_engine)
# ──────────────────────────────────────────────────────────────────────

@dataclass
class MockDecision:
    answer: Any
    confidence: float = 0.9
    probabilities: dict = field(default_factory=dict)


class MockVerdict:
    def __init__(self, answers: dict):
        self.answers = answers
    def to_jsonable(self):
        return {k: {"answer": v.answer} for k, v in self.answers.items()}


class MockLayaBackend:
    """Returns preset answers, cycling through a list if multiple rounds."""
    def __init__(self, presets):
        """presets: list of dicts, each mapping qid -> (answer, confidence)."""
        self.presets = presets if isinstance(presets, list) else [presets]
        self.call_count = 0

    def decide(self, state, questions):
        idx = min(self.call_count, len(self.presets) - 1)
        preset = self.presets[idx]
        self.call_count += 1
        answers = {}
        for qid in questions:
            if qid in preset:
                ans, conf = preset[qid]
                answers[qid] = MockDecision(answer=ans, confidence=conf,
                                             probabilities={str(ans): conf})
            else:
                answers[qid] = MockDecision(answer="default", confidence=0.5)
        return MockVerdict(answers)


# ──────────────────────────────────────────────────────────────────────
# Triage graph tests
# ──────────────────────────────────────────────────────────────────────

class TestTriageGraph:
    def test_billing_route(self):
        wf = _build_triage_graph()
        backend = MockLayaBackend([
            {"category": ("billing", 0.9), "urgency": (1, 0.8)},   # classify
            {"action": ("refund", 0.9)},                             # route_billing
        ])
        result = wf.run(backend, {"text": "charge me twice", "from": "user@x.com"})
        assert result["trace"]["final_action"] == "stop"
        assert result["node_visits"]["classify"] == 1
        assert result["node_visits"]["route_billing"] == 1

    def test_security_gate_blocks(self):
        wf = _build_triage_graph()
        backend = MockLayaBackend([
            {"category": ("security", 0.95), "urgency": (2, 0.9)},  # classify
            {"is_threat": ("B", 0.95), "severity": (2, 0.9)},       # security_gate
        ])
        result = wf.run(backend, {"text": "phishing attack detected"})
        assert result["trace"]["final_action"] == "stop"
        assert result["node_visits"]["security_gate"] == 1

    def test_security_gate_low_confidence_escalates(self):
        wf = _build_triage_graph()
        backend = MockLayaBackend([
            {"category": ("security", 0.95), "urgency": (2, 0.9)},
            {"is_threat": ("B", 0.3), "severity": (1, 0.3)},  # low confidence
        ])
        result = wf.run(backend, {"text": "maybe phishing?"})
        assert result["trace"]["final_action"] == "escalate"

    def test_technical_route(self):
        wf = _build_triage_graph()
        backend = MockLayaBackend([
            {"category": ("technical", 0.9), "urgency": (1, 0.7)},
            {"action": ("ticket", 0.9)},
        ])
        result = wf.run(backend, {"text": "bug in login"})
        assert result["trace"]["final_action"] == "stop"
        assert "route_tech" in result["node_visits"]

    def test_other_route(self):
        wf = _build_triage_graph()
        backend = MockLayaBackend([
            {"category": ("other", 0.8), "urgency": (0, 0.6)},
            {"action": ("archive", 0.9)},
        ])
        result = wf.run(backend, {"text": "lunch plans"})
        assert result["trace"]["final_action"] == "stop"
        assert "route_general" in result["node_visits"]


# ──────────────────────────────────────────────────────────────────────
# Review loop graph tests
# ──────────────────────────────────────────────────────────────────────

class TestReviewLoopGraph:
    def test_approve_on_first_try(self):
        wf = _build_review_loop_graph()
        backend = MockLayaBackend({"quality": (3, 0.9), "should_revise": ("B", 0.95)})
        result = wf.run(backend, {"draft": "good draft", "iteration": 0})
        assert result["trace"]["final_action"] == "stop"
        assert result["iterations"] == 1

    def test_revision_loop(self):
        """Laya sends back for revision twice, then approves."""
        wf = _build_review_loop_graph()
        backend = MockLayaBackend([
            {"quality": (1, 0.8), "should_revise": ("A", 0.9)},  # round 1: revise
            {"quality": (2, 0.8), "should_revise": ("A", 0.85)}, # round 2: revise
            {"quality": (3, 0.9), "should_revise": ("B", 0.95)}, # round 3: approve
        ])
        result = wf.run(backend, {"draft": "bad draft", "iteration": 0})
        assert result["trace"]["final_action"] == "stop"
        assert result["iterations"] == 3

    def test_max_iterations_safety(self):
        """Loop can't go forever."""
        wf = _build_review_loop_graph(max_iterations=3)
        backend = MockLayaBackend({"quality": (1, 0.9), "should_revise": ("A", 0.95)})
        result = wf.run(backend, {"draft": "terrible", "iteration": 0})
        assert result["trace"]["final_action"] == "max_iterations"
        assert result["iterations"] == 3


# ──────────────────────────────────────────────────────────────────────
# Optimizer workflow graph tests
# ──────────────────────────────────────────────────────────────────────

class TestOptimizerGraph:
    def test_stop_on_convergence(self):
        wf = _build_continue_check_graph()
        backend = MockLayaBackend({"should_continue": ("stop", 0.95), "confidence_assessment": (2, 0.8)})
        result = wf.run(backend, {
            "task": "outlier_detect", "score": -3.0, "best_score": -3.0,
            "step": 20, "history": [], "params": {}, "strategy": "default",
        })
        assert result["trace"]["final_action"] == "stop"

    def test_continue_to_strategy(self):
        wf = _build_continue_check_graph()
        backend = MockLayaBackend([
            {"should_continue": ("continue", 0.9), "confidence_assessment": (2, 0.8)},
            {"strategy_choice": ("keep", 0.9), "switch_urgency": (0, 0.7)},
        ])
        result = wf.run(backend, {
            "task": "test", "score": -5.0, "best_score": -3.0,
            "step": 5, "history": [], "params": {}, "strategy": "default",
        })
        # continue → strategy_check → keep → (no more nodes → stop)
        assert result["iterations"] >= 2


# ──────────────────────────────────────────────────────────────────────
# Workflow registry tests
# ──────────────────────────────────────────────────────────────────────

class TestWorkflowRegistry:
    def test_all_workflows_buildable(self):
        for name, factory in WORKFLOWS.items():
            wf = factory()
            assert isinstance(wf, ResilientWorkflow), f"{name} didn't build"
            assert len(wf.nodes) > 0, f"{name} has no nodes"
            assert wf.start in wf.nodes, f"{name} start '{wf.start}' not in nodes"

    def test_describe_all(self):
        for name, factory in WORKFLOWS.items():
            wf = factory()
            desc = wf.describe()
            assert "start" in desc
            assert "nodes" in desc
