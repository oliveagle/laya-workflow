"""Laya Workflow Engine — composition demos.

Shows nested workflows, fan-out/fan-in, and checkpoint + resume.
Run: python3.12 code/laya/cli_compose.py
"""
from __future__ import annotations

import json
import os
import sys
import tempfile
import time
from pathlib import Path
from dataclasses import dataclass, field
from typing import Any, Dict, List

_REPO = str(Path(__file__).resolve().parents[2])
if _REPO not in sys.path:
    sys.path.insert(0, _REPO)

from code.laya.workflow_engine import (
    Edge, NodeAction, NodeResult, ResilientWorkflow, WorkflowNode,
)
from code.laya.composition import (
    SubWorkflowNode, FanOutNode, Checkpoint, ResilientLoop,
)

_B = "\033[1m"
_C = {"cyan": "\033[36m", "green": "\033[32m", "red": "\033[31m",
      "yellow": "\033[33m", "magenta": "\033[35m", "dim": "\033[2m",
      "bold": "\033[1m", "reset": "\033[0m"}


# ──────────────────────────────────────────────────────────────────────
# Mock backend for demos
# ──────────────────────────────────────────────────────────────────────

@dataclass
class _D:
    answer: Any
    confidence: float = 0.9
    probabilities: dict = field(default_factory=dict)

class _V:
    def __init__(self, answers):
        self.answers = answers

class ScriptedBackend:
    def __init__(self, script):
        self.script = script if isinstance(script, list) else [script]
        self.idx = 0
    def decide(self, state, questions):
        p = self.script[min(self.idx, len(self.script)-1)]
        self.idx += 1
        answers = {}
        for qid in questions:
            if qid in p:
                a, c = p[qid]
                answers[qid] = _D(answer=a, confidence=c, probabilities={str(a): c})
            else:
                answers[qid] = _D(answer="default", confidence=0.5)
        return _V(answers)


# ──────────────────────────────────────────────────────────────────────
# Demo 1: SubWorkflowNode — nested triage inside a review
# ──────────────────────────────────────────────────────────────────────

def _build_triage_inner():
    """Inner triage workflow: classify → route → done."""
    classify = WorkflowNode(
        name="classify",
        questions={"cat": {"type": "choice", "instructions": "Category?",
                            "criteria": {"billing": "money", "tech": "bugs", "other": "misc"}}},
        edge=Edge(condition={"billing": "route_bill", "tech": "route_tech", "other": "done"}),
        primary_q="cat",
        state_fn=lambda s: {"text": s.get("text", "")},
    )
    route_bill = WorkflowNode(
        name="route_bill",
        questions={"act": {"type": "choice", "instructions": "Action?",
                            "criteria": {"refund": "R", "none": "N"}}},
        edge=Edge(condition={"refund": "done", "none": "done"}),
        primary_q="act",
        state_fn=lambda s: {"text": s.get("text", "")},
    )
    route_tech = WorkflowNode(
        name="route_tech",
        questions={"act": {"type": "choice", "instructions": "Action?",
                            "criteria": {"ticket": "T", "none": "N"}}},
        edge=Edge(condition={"ticket": "done", "none": "done"}),
        primary_q="act",
        state_fn=lambda s: {"text": s.get("text", "")},
    )
    done = WorkflowNode(
        name="done",
        questions={"ok": {"type": "choice", "instructions": "Done?",
                           "criteria": {"yes": "Y"}}},
        edge=Edge(condition={"yes": "STOP"}),
        primary_q="ok",
        state_fn=lambda s: {},
    )
    return ResilientWorkflow(
        nodes={"classify": classify, "route_bill": route_bill,
               "route_tech": route_tech, "done": done},
        start="classify", max_iterations=10,
    )


def _build_review_with_triage():
    """Outer: review → (if revise needed, run triage sub-workflow) → re-review."""
    triage_wf = _build_triage_inner()
    triage_sub = SubWorkflowNode("triage", triage_wf)

    review = WorkflowNode(
        name="review",
        questions={
            "quality": {"type": "score", "instructions": "Quality?",
                          "criteria": ["poor", "ok", "good"]},
            "verdict": {"type": "choice", "instructions": "Verdict?",
                         "criteria": {"A": "needs triage then revise", "B": "approve"}},
        },
        edge=Edge(condition={"A": "review"}, default="STOP", min_confidence=0.5),
        primary_q="verdict",
        max_retries=6,
        state_fn=lambda s: {"text": s.get("text", ""), "round": s.get("round", 0)},
        action_fn=lambda s, v: {
            **s,
            "round": s.get("round", 0) + 1,
            "_review_verdict": v.answers["verdict"].answer,
        },
    )

    return ResilientWorkflow(
        nodes={"review": review},
        start="review", max_iterations=8,
    )


def demo_subworkflow():
    print(f"\n{_B}╔══════════════════════════════════════════════════════╗{_C['reset']}")
    print(f"{_B}║  Demo 5: SubWorkflow — triage inside review loop   ║{_C['reset']}")
    print(f"{_B}╚══════════════════════════════════════════════════════╝{_C['reset']}")
    print(f"""
{_C['dim']}Graph:
  review ──(revise)──▶ review ──(approve)──▶ STOP
     │
     └── each revision triggers triage sub-workflow
         classify → route → done{_C['reset']}
""")

    wf = _build_review_with_triage()
    backend = ScriptedBackend([
        {"quality": (0, 0.8), "verdict": ("A", 0.9)},   # round 1: needs triage
        {"quality": (1, 0.8), "verdict": ("A", 0.85)},  # round 2: still needs work
        {"quality": (2, 0.95), "verdict": ("B", 0.95)}, # round 3: approve
    ])
    result = wf.run(backend, {"text": "draft with billing issues", "round": 0})

    print(f"{_B}Execution:{_C['reset']}")
    for step in result["history"]:
        node = step["node"]
        action = step["action"]
        color = _C.get("green" if action == "stop" else "yellow" if action == "retry" else "cyan", "")
        print(f"  → {color}{_B}{node}{_C['reset']} [{action}] conf={step['confidence']:.2f}  {step.get('detail','')}")

    print(f"\n{_B}Result:{_C['reset']}")
    print(f"  Final: {result['trace']['final_action']}")
    print(f"  Iterations: {result['iterations']}")
    print(f"  Node visits: {result['node_visits']}")


# ──────────────────────────────────────────────────────────────────────
# Demo 2: FanOutNode — parallel classify + merge
# ──────────────────────────────────────────────────────────────────────

def _build_billing_wf():
    q = {"act": {"type": "choice", "instructions": "Billing action?",
                  "criteria": {"refund": "refund", "invoice": "invoice"}}}
    return ResilientWorkflow(
        nodes={
            "start": WorkflowNode(name="start", questions=q,
                                   edge=Edge(condition={"refund": "done", "invoice": "done"}),
                                   primary_q="act", state_fn=lambda s: {"text": s.get("text","")}),
            "done": WorkflowNode(name="done", questions={"ok": {"type":"choice","instructions":"?","criteria":{"y":"Y"}}},
                                  edge=Edge(condition={"y":"STOP"}), primary_q="ok", state_fn=lambda s: {}),
        },
        start="start", max_iterations=5,
    )

def _build_tech_wf():
    q = {"act": {"type": "choice", "instructions": "Tech action?",
                  "criteria": {"ticket": "ticket", "ignore": "ignore"}}}
    return ResilientWorkflow(
        nodes={
            "start": WorkflowNode(name="start", questions=q,
                                   edge=Edge(condition={"ticket": "done", "ignore": "done"}),
                                   primary_q="act", state_fn=lambda s: {"text": s.get("text","")}),
            "done": WorkflowNode(name="done", questions={"ok": {"type":"choice","instructions":"?","criteria":{"y":"Y"}}},
                                  edge=Edge(condition={"y":"STOP"}), primary_q="ok", state_fn=lambda s: {}),
        },
        start="start", max_iterations=5,
    )

def _build_security_wf():
    q = {"act": {"type": "choice", "instructions": "Security action?",
                  "criteria": {"block": "block", "allow": "allow"}}}
    return ResilientWorkflow(
        nodes={
            "start": WorkflowNode(name="start", questions=q,
                                   edge=Edge(condition={"block": "done", "allow": "done"}),
                                   primary_q="act", state_fn=lambda s: {"text": s.get("text","")}),
            "done": WorkflowNode(name="done", questions={"ok": {"type":"choice","instructions":"?","criteria":{"y":"Y"}}},
                                  edge=Edge(condition={"y":"STOP"}), primary_q="ok", state_fn=lambda s: {}),
        },
        start="start", max_iterations=5,
    )


def demo_fanout():
    print(f"\n{_B}╔══════════════════════════════════════════════════════╗{_C['reset']}")
    print(f"{_B}║  Demo 6: FanOut — 3 parallel branches + merge      ║{_C['reset']}")
    print(f"{_B}╚══════════════════════════════════════════════════════╝{_C['reset']}")
    print(f"""
{_C['dim']}Graph:
  input ──┬──▶ billing_wf  ──▶ done
          ├──▶ tech_wf     ──▶ done
          └──▶ security_wf ──▶ done
                         ↓
                    merge results{_C['reset']}
""")

    fanout = FanOutNode(
        name="parallel_classify",
        branches={
            "billing": _build_billing_wf(),
            "technical": _build_tech_wf(),
            "security": _build_security_wf(),
        },
        merge_fn=lambda results: {
            "branches_completed": len(results),
            "total_iterations": sum(r.iterations for r in results),
            "branch_details": [
                {"name": r.branch_name, "action": r.trace.get("final_action"),
                 "iterations": r.iterations, "latency_ms": round(r.latency_ms, 1)}
                for r in results
            ],
        },
    )

    backend = ScriptedBackend([
        # billing branch
        {"act": ("refund", 0.9)}, {"ok": ("y", 0.95)},
        # tech branch
        {"act": ("ticket", 0.85)}, {"ok": ("y", 0.9)},
        # security branch
        {"act": ("block", 0.92)}, {"ok": ("y", 0.95)},
    ])

    t0 = time.time()
    result = fanout.execute({"text": "urgent: billing error + login bug + phishing"}, backend)
    total_ms = (time.time() - t0) * 1000

    print(f"{_B}Results:{_C['reset']}")
    for branch in result["_fanout_results"]:
        color = _C["green"] if branch["final_action"] == "stop" else _C["red"]
        print(f"  {color}✓{_C['reset']} {branch['branch']:<12} {branch['iterations']} iterations  {branch['latency_ms']:.1f}ms  [{branch['final_action']}]")

    merged = result["_merged"]
    print(f"\n{_B}Merged:{_C['reset']}")
    print(f"  Branches completed: {merged['branches_completed']}")
    print(f"  Total iterations: {merged['total_iterations']}")
    print(f"  Wall clock: {total_ms:.1f}ms")


# ──────────────────────────────────────────────────────────────────────
# Demo 3: Checkpoint + Resume — crash recovery
# ──────────────────────────────────────────────────────────────────────

def demo_checkpoint():
    print(f"\n{_B}╔══════════════════════════════════════════════════════╗{_C['reset']}")
    print(f"{_B}║  Demo 7: Checkpoint — crash recovery               ║{_C['reset']}")
    print(f"{_B}╚══════════════════════════════════════════════════════╝{_C['reset']}")

    with tempfile.TemporaryDirectory() as tmp:
        ckpt = Checkpoint(f"{tmp}/optimizer.json")

        # simulate 3 iterations of an optimizer
        state = {"score": -10.0, "step": 0, "params": {"lr": 0.01}}
        for i in range(3):
            state = {**state, "score": state["score"] + 1.5, "step": i + 1}
            ckpt.save(state, iteration=i + 1)
            print(f"  {_C['dim']}iter {i+1}: score={state['score']:.1f} (saved){_C['reset']}")

        print(f"\n  {_C['red']}⚡ CRASH! Process killed at iteration 3{_C['reset']}")
        print(f"  {_C['dim']}state lost... but checkpoint exists{_C['reset']}")

        # resume from checkpoint
        loaded_state, iteration, history, extra = ckpt.load()
        print(f"\n  {_C['green']}✓ Resumed from checkpoint:{_C['reset']}")
        print(f"    iteration: {iteration}")
        print(f"    score: {loaded_state['score']:.1f}")
        print(f"    params: {loaded_state['params']}")

        # continue from where we left off
        for i in range(iteration, iteration + 2):
            loaded_state = {**loaded_state, "score": loaded_state["score"] + 1.0, "step": i + 1}
            ckpt.save(loaded_state, iteration=i + 1)
            print(f"  {_C['dim']}iter {i+1}: score={loaded_state['score']:.1f} (resumed){_C['reset']}")

        print(f"\n  {_B}Final score: {loaded_state['score']:.1f}{_C['reset']}")


# ──────────────────────────────────────────────────────────────────────
# Demo 4: ResilientLoop — Laya-powered loop with checkpoint
# ──────────────────────────────────────────────────────────────────────

def demo_resilient_loop():
    print(f"\n{_B}╔══════════════════════════════════════════════════════╗{_C['reset']}")
    print(f"{_B}║  Demo 8: ResilientLoop — Laya decides when to stop ║{_C['reset']}")
    print(f"{_B}╚══════════════════════════════════════════════════════╝{_C['reset']}")
    print(f"""
{_C['dim']}Pattern:
  while True:
      state = step_fn(state)           # one optimization step
      verdict = laya.decide(state)     # Laya: continue or stop?
      if verdict == "stop": break
      checkpoint.save(state)           # survive crashes{_C['reset']}
""")

    with tempfile.TemporaryDirectory() as tmp:
        scores = [-10.0, -8.5, -7.2, -6.1, -5.3, -4.8, -4.5, -4.3, -4.2, -4.15]
        score_iter = iter(scores)

        def step(state):
            s = state.copy()
            s["score"] = next(score_iter)
            s["best_score"] = min(s.get("best_score", 999), s["score"])
            s["step"] = state.get("step", 0) + 1
            return s

        backend = ScriptedBackend([
            {"should_continue": ("continue", 0.9)},
            {"should_continue": ("continue", 0.85)},
            {"should_continue": ("continue", 0.8)},
            {"should_continue": ("continue", 0.75)},
            {"should_continue": ("continue", 0.7)},
            {"should_continue": ("continue", 0.65)},
            {"should_continue": ("stop", 0.95)},  # Laya says: enough
        ])

        loop = ResilientLoop("optimizer_demo", backend, checkpoint_dir=tmp)
        result = loop.run(
            initial_state={"score": -999, "task": "outlier_detect", "step": 0},
            step_fn=step,
            max_iterations=10,
        )

        print(f"{_B}Execution trace:{_C['reset']}")
        for d in result["decisions"]:
            color = _C["green"] if d["answer"] == "stop" else _C["cyan"]
            score_str = f"{d['score']:.1f}" if d["score"] is not None else "?"
            print(f"  {color}iter {d['iteration']:<3} score={score_str:<6} "
                  f"laya→{_B}{d['answer']:<9}{_C['reset']} conf={d['confidence']:.2f}")

        print(f"\n{_B}Summary:{_C['reset']}")
        print(f"  Iterations: {result['iterations']}")
        print(f"  Final score: {result['state']['score']:.2f}")
        print(f"  Converged: {result['converged']}")

        # verify checkpoint exists
        ckpt_exists = os.path.exists(f"{tmp}/optimizer_demo_checkpoint.json")
        print(f"  Checkpoint saved: {ckpt_exists}")


# ──────────────────────────────────────────────────────────────────────
# Main
# ──────────────────────────────────────────────────────────────────────

def main():
    demo_subworkflow()
    demo_fanout()
    demo_checkpoint()
    demo_resilient_loop()
    print(f"\n{_B}{'═'*60}{_C['reset']}")
    print(f"{_B}  All composition demos complete!{_C['reset']}")
    print(f"{'═'*60}\n")


if __name__ == "__main__":
    main()
