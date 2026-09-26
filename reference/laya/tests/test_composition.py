"""Tests for Laya workflow composition, persistence, and fan-out.

Run: python3.12 -m pytest code/laya/tests/test_composition.py -v
"""
from __future__ import annotations

import json
import os
import tempfile
import sys
from pathlib import Path
from dataclasses import dataclass, field
from typing import Any, Dict

sys.path.insert(0, str(Path(__file__).resolve().parents[2]))

from code.laya.workflow_engine import (
    Edge, NodeAction, NodeResult, ResilientWorkflow, WorkflowNode,
)
from code.laya.composition import (
    SubWorkflowNode, FanOutNode, BranchResult, Checkpoint, ResilientLoop,
)


# ──────────────────────────────────────────────────────────────────────
# Mock backend
# ──────────────────────────────────────────────────────────────────────

@dataclass
class _D:
    answer: Any
    confidence: float = 0.9
    probabilities: dict = field(default_factory=dict)

class _V:
    def __init__(self, answers):
        self.answers = answers

class MockBackend:
    def __init__(self, presets):
        self.presets = presets if isinstance(presets, list) else [presets]
        self.idx = 0
    def decide(self, state, questions):
        p = self.presets[min(self.idx, len(self.presets)-1)]
        self.idx += 1
        answers = {}
        for qid in questions:
            if qid in p:
                a, c = p[qid]
                answers[qid] = _D(answer=a, confidence=c, probabilities={str(a): c})
            else:
                answers[qid] = _D(answer="default", confidence=0.5)
        return _V(answers)


def _make_linear_wf(start="a", end="b"):
    """Simple two-node workflow: a → b → STOP."""
    node_a = WorkflowNode(
        name="a",
        questions={"q": {"type": "choice", "instructions": "t", "criteria": {"go": "y"}}},
        edge=Edge(condition={"go": "b"}),
        primary_q="q",
    )
    node_b = WorkflowNode(
        name="b",
        questions={"q": {"type": "choice", "instructions": "t", "criteria": {"done": "y"}}},
        edge=Edge(condition={"done": "STOP"}),
        primary_q="q",
    )
    return ResilientWorkflow(nodes={"a": node_a, "b": node_b}, start="a")


# ──────────────────────────────────────────────────────────────────────
# SubWorkflowNode tests
# ──────────────────────────────────────────────────────────────────────

class TestSubWorkflowNode:
    def test_execute_inner_workflow(self):
        inner = _make_linear_wf()
        sub = SubWorkflowNode("inner", inner)
        backend = MockBackend([{"q": ("go", 0.9)}, {"q": ("done", 0.95)}])
        result = sub.execute({"x": 1}, backend)

        assert result["_subworkflow"] == "inner"
        assert result["_subworkflow_trace"]["final_action"] == "stop"
        assert result["_subworkflow_iterations"] == 2

    def test_no_backend_raises(self):
        inner = _make_linear_wf()
        sub = SubWorkflowNode("inner", inner)
        try:
            sub.execute({})
            assert False, "should have raised"
        except ValueError as e:
            assert "no backend" in str(e)

    def test_with_preset_backend(self):
        inner = _make_linear_wf()
        backend = MockBackend([{"q": ("go", 0.9)}, {"q": ("done", 0.95)}])
        sub = SubWorkflowNode("inner", inner, backend=backend)
        result = sub.execute({"x": 1})
        assert result["_subworkflow_trace"]["final_action"] == "stop"


# ──────────────────────────────────────────────────────────────────────
# FanOutNode tests
# ──────────────────────────────────────────────────────────────────────

class TestFanOutNode:
    def test_run_all_branches(self):
        wf1 = _make_linear_wf("a1", "b1")
        wf2 = _make_linear_wf("a2", "b2")
        fanout = FanOutNode("test", {"branch1": wf1, "branch2": wf2})

        backend = MockBackend([
            {"q": ("go", 0.9)}, {"q": ("done", 0.95)},  # branch1
            {"q": ("go", 0.8)}, {"q": ("done", 0.9)},   # branch2
        ])
        result = fanout.execute({"x": 1}, backend)

        assert result["_fanout"] == "test"
        assert len(result["_fanout_results"]) == 2
        assert result["_fanout_results"][0]["branch"] == "branch1"
        assert result["_fanout_results"][1]["branch"] == "branch2"

    def test_custom_merge(self):
        wf1 = _make_linear_wf()
        wf2 = _make_linear_wf()

        def my_merge(results):
            return {"count": len(results), "branches": [r.branch_name for r in results]}

        fanout = FanOutNode("test", {"a": wf1, "b": wf2}, merge_fn=my_merge)
        backend = MockBackend([{"q": ("go", 0.9)}, {"q": ("done", 0.95)}] * 2)
        result = fanout.execute({}, backend)

        assert result["_merged"]["count"] == 2
        assert set(result["_merged"]["branches"]) == {"a", "b"}

    def test_default_merge_picks_best(self):
        wf1 = _make_linear_wf()
        fanout = FanOutNode("test", {"single": wf1})
        backend = MockBackend([{"q": ("go", 0.9)}, {"q": ("done", 0.95)}])
        result = fanout.execute({}, backend)

        assert result["_merged"]["best_branch"] == "single"
        assert result["_merged"]["total_iterations"] == 2


# ──────────────────────────────────────────────────────────────────────
# Checkpoint tests
# ──────────────────────────────────────────────────────────────────────

class TestCheckpoint:
    def test_save_and_load(self):
        with tempfile.TemporaryDirectory() as tmp:
            ckpt = Checkpoint(f"{tmp}/test.json")
            state = {"score": -3.5, "params": {"lr": 0.01}}
            ckpt.save(state, iteration=10, history=[{"step": 1}])

            loaded_state, iteration, history, extra = ckpt.load()
            assert loaded_state["score"] == -3.5
            assert iteration == 10
            assert len(history) == 1

    def test_exists(self):
        with tempfile.TemporaryDirectory() as tmp:
            ckpt = Checkpoint(f"{tmp}/test.json")
            assert not ckpt.exists()
            ckpt.save({"x": 1}, iteration=1)
            assert ckpt.exists()

    def test_delete(self):
        with tempfile.TemporaryDirectory() as tmp:
            ckpt = Checkpoint(f"{tmp}/test.json")
            ckpt.save({"x": 1}, iteration=1)
            assert ckpt.exists()
            ckpt.delete()
            assert not ckpt.exists()

    def test_file_not_found_raises(self):
        ckpt = Checkpoint("/nonexistent/path/ckpt.json")
        try:
            ckpt.load()
            assert False, "should have raised"
        except FileNotFoundError:
            pass

    def test_nested_state(self):
        with tempfile.TemporaryDirectory() as tmp:
            ckpt = Checkpoint(f"{tmp}/test.json")
            state = {
                "nested": {"a": [1, 2, 3], "b": {"c": True}},
                "score": -1.0,
            }
            ckpt.save(state, iteration=5)
            loaded, _, _, _ = ckpt.load()
            assert loaded["nested"]["a"] == [1, 2, 3]
            assert loaded["nested"]["b"]["c"] is True


# ──────────────────────────────────────────────────────────────────────
# ResilientLoop tests
# ──────────────────────────────────────────────────────────────────────

class TestResilientLoop:
    def test_run_to_convergence(self):
        with tempfile.TemporaryDirectory() as tmp:
            scores = iter([-5.0, -4.0, -3.5, -3.1, -3.01, -3.001])

            def step(state):
                s = state.copy()
                s["score"] = next(scores)
                s["best_score"] = min(s.get("best_score", 999), s["score"])
                return s

            backend = MockBackend([
                {"should_continue": ("continue", 0.9)},  # iter 1
                {"should_continue": ("continue", 0.85)}, # iter 2
                {"should_continue": ("continue", 0.8)},  # iter 3
                {"should_continue": ("continue", 0.75)}, # iter 4
                {"should_continue": ("stop", 0.95)},     # iter 5: Laya says stop
            ])

            loop = ResilientLoop("test", backend, checkpoint_dir=tmp)
            result = loop.run(
                initial_state={"score": -999, "task": "test"},
                step_fn=step,
                max_iterations=10,
            )

            assert result["iterations"] >= 5
            assert len(result["score_history"]) >= 5
            assert len(result["decisions"]) >= 1

    def test_checkpoint_created(self):
        with tempfile.TemporaryDirectory() as tmp:
            def step(state):
                s = state.copy()
                s["score"] = state.get("score", 0) - 1
                return s

            backend = MockBackend({"should_continue": ("stop", 0.9)})
            loop = ResilientLoop("ckpt_test", backend, checkpoint_dir=tmp)
            loop.run(
                initial_state={"score": 0, "task": "test"},
                step_fn=step,
                max_iterations=5,
            )

            assert os.path.exists(f"{tmp}/ckpt_test_checkpoint.json")
            assert os.path.exists(f"{tmp}/ckpt_test_decisions.json")

    def test_resume_from_checkpoint(self):
        with tempfile.TemporaryDirectory() as tmp:
            call_count = [0]

            def step(state):
                call_count[0] += 1
                s = state.copy()
                s["score"] = state.get("score", 0) - 1
                return s

            backend = MockBackend({"should_continue": ("continue", 0.9)})
            loop = ResilientLoop("resume_test", backend, checkpoint_dir=tmp)

            # run 3 iterations then "crash"
            result1 = loop.run(
                initial_state={"score": 0, "task": "test"},
                step_fn=step,
                max_iterations=3,
            )
            first_iterations = result1["iterations"]

            # resume — should start from checkpoint
            backend2 = MockBackend({"should_continue": ("stop", 0.95)})
            loop2 = ResilientLoop("resume_test", backend2, checkpoint_dir=tmp)
            result2 = loop2.run(
                initial_state={"score": 0, "task": "test"},
                step_fn=step,
                max_iterations=6,
            )

            # should have resumed, not started over
            assert result2["iterations"] >= first_iterations

    def test_convergence_stops_loop(self):
        with tempfile.TemporaryDirectory() as tmp:
            def step(state):
                s = state.copy()
                s["score"] = -3.0  # constant score → convergence
                return s

            backend = MockBackend({"should_continue": ("continue", 0.9)})
            loop = ResilientLoop("conv_test", backend, checkpoint_dir=tmp)
            result = loop.run(
                initial_state={"score": -3.0, "task": "test"},
                step_fn=step,
                max_iterations=20,
                convergence_window=3,
                convergence_eps=0.01,
            )

            assert result["converged"] is True
            assert result["iterations"] <= 8  # should stop early
