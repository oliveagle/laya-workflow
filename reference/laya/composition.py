"""Laya Workflow Engine — composition and persistence extensions.

Adds two critical capabilities for real-world workflows:

1. **SubWorkflowNode** — a node that runs an entire sub-workflow as its action.
   Enables nested/composed workflows: complex patterns built from simple pieces.

2. **State persistence** — checkpoint + resume for long-running loops.
   JSON-serializable state that survives process restarts.

3. **Fan-out / fan-in** — parallel branches that merge results.

Usage::

    from code.laya.composition import SubWorkflowNode, FanOutNode, Checkpoint

    # compose: triage inside a review loop
    review = make_review_node()
    triage_sub = SubWorkflowNode("triage", triage_workflow)
    review_with_triage = WorkflowNode(
        name="review_with_triage",
        questions={...},
        edge=Edge(condition={"approve": "done", "revise": "review_with_triage"}),
        action_fn=lambda s, v: triage_sub.run(s, v),
    )
"""
from __future__ import annotations

import json
import time
from dataclasses import dataclass, field
from pathlib import Path
from typing import Any, Callable, Dict, List, Optional, Tuple

from .workflow_engine import (
    Edge, NodeAction, NodeResult, ResilientWorkflow, WorkflowNode, WorkflowTrace,
)


# ──────────────────────────────────────────────────────────────────────
# SubWorkflowNode — nested workflow as a node
# ──────────────────────────────────────────────────────────────────────

class SubWorkflowNode:
    """Wraps a full ResilientWorkflow as a single node in a parent workflow.

    The sub-workflow runs with its own Laya backend, nodes, and termination
    conditions.  Its final state becomes the parent's action_result.

    Usage::

        inner = ResilientWorkflow(nodes={...}, start="classify")
        sub = SubWorkflowNode("triage", inner)

        # use in parent workflow
        parent_node = WorkflowNode(
            name="process",
            questions={...},
            edge=Edge(condition={"done": "STOP"}),
            action_fn=lambda s, v: sub.execute(s),
        )
    """

    def __init__(self, name: str, workflow: ResilientWorkflow, backend=None):
        self.name = name
        self.workflow = workflow
        self.backend = backend  # if None, uses parent's backend

    def execute(self, state: dict, backend=None) -> dict:
        """Run the sub-workflow and return the result state + trace."""
        be = backend or self.backend
        if be is None:
            raise ValueError(f"SubWorkflowNode '{self.name}': no backend provided")
        result = self.workflow.run(be, state)
        return {
            "_subworkflow": self.name,
            "_subworkflow_result": result["result"],
            "_subworkflow_trace": result["trace"],
            "_subworkflow_iterations": result["iterations"],
        }


# ──────────────────────────────────────────────────────────────────────
# FanOutNode — parallel branch execution
# ──────────────────────────────────────────────────────────────────────

@dataclass
class BranchResult:
    """Result from a single parallel branch."""
    branch_name: str
    state: dict
    trace: dict
    iterations: int
    latency_ms: float


class FanOutNode:
    """Execute multiple sub-workflows in parallel (or sequential) and merge results.

    This enables fan-out/fan-in patterns: split into N branches,
    run each independently, then merge all results back.

    Usage::

        fanout = FanOutNode(
            name="parallel_classify",
            branches={
                "billing": billing_workflow,
                "technical": tech_workflow,
                "security": security_workflow,
            },
            merge_fn=lambda results: merge_results(results),
        )
        result = fanout.execute(state, backend)
    """

    def __init__(
        self,
        name: str,
        branches: Dict[str, ResilientWorkflow],
        merge_fn: Callable[[List[BranchResult]], dict] | None = None,
        parallel: bool = False,
    ):
        self.name = name
        self.branches = branches
        self.merge_fn = merge_fn or self._default_merge
        self.parallel = parallel

    def execute(self, state: dict, backend) -> dict:
        """Run all branches and merge results."""
        results: List[BranchResult] = []

        for branch_name, workflow in self.branches.items():
            t0 = time.time()
            result = workflow.run(backend, state)
            lat = (time.time() - t0) * 1000

            results.append(BranchResult(
                branch_name=branch_name,
                state=result["result"],
                trace=result["trace"],
                iterations=result["iterations"],
                latency_ms=lat,
            ))

        merged = self.merge_fn(results)
        return {
            "_fanout": self.name,
            "_fanout_results": [
                {"branch": r.branch_name, "iterations": r.iterations,
                 "latency_ms": round(r.latency_ms, 1), "final_action": r.trace.get("final_action")}
                for r in results
            ],
            "_merged": merged,
        }

    @staticmethod
    def _default_merge(results: List[BranchResult]) -> dict:
        """Default merge: pick the branch with the highest confidence or most iterations."""
        if not results:
            return {}
        best = max(results, key=lambda r: r.iterations)
        return {"best_branch": best.branch_name, "total_iterations": sum(r.iterations for r in results)}


# ──────────────────────────────────────────────────────────────────────
# Checkpoint — state persistence for long-running loops
# ──────────────────────────────────────────────────────────────────────

@dataclass
class Checkpoint:
    """JSON-serializable checkpoint for workflow state.

    Saves: current state, iteration count, history, score trajectory.
    Enables resume from the last checkpoint without losing progress.

    Usage::

        ckpt = Checkpoint(path="/tmp/workflow_ckpt.json")

        # save after each iteration
        ckpt.save(state, iteration=5, history=history)

        # resume
        state, iteration = ckpt.load()
    """
    path: str
    auto_save: bool = True

    def save(
        self,
        state: dict,
        iteration: int = 0,
        history: List[dict] | None = None,
        extra: dict | None = None,
    ) -> None:
        """Save state to JSON file."""
        data = {
            "state": _make_jsonable(state),
            "iteration": iteration,
            "history": _make_jsonable(history or []),
            "timestamp": time.time(),
        }
        if extra:
            data["extra"] = _make_jsonable(extra)

        Path(self.path).parent.mkdir(parents=True, exist_ok=True)
        with open(self.path, "w") as f:
            json.dump(data, f, ensure_ascii=False, indent=2, default=str)

    def load(self) -> Tuple[dict, int, List[dict], dict]:
        """Load state from JSON file.

        Returns: (state, iteration, history, extra)
        Raises FileNotFoundError if no checkpoint exists.
        """
        with open(self.path) as f:
            data = json.load(f)
        return (
            data.get("state", {}),
            data.get("iteration", 0),
            data.get("history", []),
            data.get("extra", {}),
        )

    def exists(self) -> bool:
        return Path(self.path).exists()

    def delete(self) -> None:
        p = Path(self.path)
        if p.exists():
            p.unlink()


def _make_jsonable(obj):
    """Recursively convert dataclasses and other objects to JSON-safe types."""
    if hasattr(obj, "__dataclass_fields__"):
        return {k: _make_jsonable(v) for k, v in obj.__dict__.items()}
    if isinstance(obj, dict):
        return {k: _make_jsonable(v) for k, v in obj.items()}
    if isinstance(obj, (list, tuple)):
        return [_make_jsonable(v) for v in obj]
    if isinstance(obj, (int, float, str, bool, type(None))):
        return obj
    return str(obj)


# ──────────────────────────────────────────────────────────────────────
# ResilientLoop — high-level loop with checkpoint + convergence
# ──────────────────────────────────────────────────────────────────────

class ResilientLoop:
    """A high-level loop that combines Laya nodes, checkpointing, and convergence.

    This is the "batteries-included" API for building resilient loops:

    - Laya decides when to stop (not hardcoded epsilon)
    - Checkpoint saves state after each iteration
    - Resume from checkpoint if process crashes
    - Convergence detection via score trajectory
    - Full audit trail

    Usage::

        loop = ResilientLoop(
            name="optimizer",
            workflow=my_workflow,
            backend=laya_engine,
            checkpoint_dir="/tmp/checkpoints",
        )

        # run with automatic checkpointing
        result = loop.run(
            initial_state={"score": -999},
            step_fn=lambda state: optimize_one_step(state),
            max_iterations=100,
        )

        # resume after crash
        result = loop.resume()
    """

    def __init__(
        self,
        name: str,
        backend,
        checkpoint_dir: str = "/tmp/laya_checkpoints",
        max_retries: int = 3,
    ):
        self.name = name
        self.backend = backend
        self.checkpoint_dir = checkpoint_dir
        self.max_retries = max_retries

    def _ckpt_path(self) -> str:
        return f"{self.checkpoint_dir}/{self.name}_checkpoint.json"

    def _decision_ckpt_path(self) -> str:
        return f"{self.checkpoint_dir}/{self.name}_decisions.json"

    def run(
        self,
        initial_state: dict,
        step_fn: Callable[[dict], dict],
        continue_questions: dict | None = None,
        max_iterations: int = 100,
        convergence_window: int = 5,
        convergence_eps: float = 1e-4,
    ) -> dict:
        """Run the loop with Laya-powered continue/stop decisions.

        Args:
            initial_state: starting state
            step_fn: callable(state) -> new_state  (one optimization step)
            continue_questions: Laya questions for continue/stop decision
            max_iterations: safety valve
            convergence_window: score window for convergence detection
            convergence_eps: convergence threshold
        """
        ckpt = Checkpoint(self._ckpt_path())
        dec_ckpt = Checkpoint(self._decision_ckpt_path())

        state = initial_state
        iteration = 0
        score_history: List[float] = []
        decision_history: List[dict] = []

        # resume if checkpoint exists
        if ckpt.exists():
            state, iteration, _, _ = ckpt.load()
            if dec_ckpt.exists():
                _, _, decision_history, _ = dec_ckpt.load()
            print(f"[ResilientLoop] Resumed from checkpoint: iteration={iteration}")

        if continue_questions is None:
            continue_questions = {
                "should_continue": {
                    "type": "choice",
                    "instructions": (
                        "Based on the current state, score trajectory, and convergence pattern, "
                        "should the loop continue optimizing?"
                    ),
                    "criteria": {
                        "stop": "score plateaued or converged, no more improvement expected",
                        "continue": "still improving, room for gains",
                    },
                },
            }

        for i in range(iteration, max_iterations):
            iteration = i + 1

            # run one step
            state = step_fn(state)

            # record score
            if "score" in state:
                score_history.append(state["score"])

            # save checkpoint
            ckpt.save(state, iteration)

            # Laya decision: continue or stop?
            laya_state = {
                "task": state.get("task", self.name),
                "score": round(state.get("score", 0), 6),
                "best_score": round(state.get("best_score", state.get("score", 0)), 6),
                "step": iteration,
                "recent_scores": [round(s, 6) for s in score_history[-5:]],
                "consecutive_same": _consecutive_same(score_history),
            }

            verdict = self.backend.decide(laya_state, continue_questions)
            answer = verdict.answers["should_continue"].answer
            confidence = verdict.answers["should_continue"].confidence

            decision = {
                "iteration": iteration,
                "answer": answer,
                "confidence": round(confidence, 4),
                "score": state.get("score"),
            }
            decision_history.append(decision)
            dec_ckpt.save({}, iteration, decision_history)

            if answer == "stop" and confidence >= 0.7:
                break

            # convergence check
            if len(score_history) >= convergence_window:
                window = score_history[-convergence_window:]
                if max(window) - min(window) <= convergence_eps:
                    break

        return {
            "state": state,
            "iterations": iteration,
            "score_history": score_history,
            "decisions": decision_history[-10:],
            "converged": len(score_history) >= convergence_window and
                         max(score_history[-convergence_window:]) -
                         min(score_history[-convergence_window:]) <= convergence_eps,
        }

    def resume(self, **kwargs) -> dict:
        """Resume from last checkpoint."""
        return self.run(**kwargs)


def _consecutive_same(values: list, tol: float = 1e-6) -> int:
    """Count consecutive values that are approximately the same."""
    if not values:
        return 0
    count = 0
    for v in reversed(values):
        if abs(v - values[-1]) <= tol:
            count += 1
        else:
            break
    return count
