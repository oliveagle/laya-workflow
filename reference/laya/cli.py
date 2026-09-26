"""Laya Workflow Engine — interactive CLI demo.

Shows the workflow graph execution with real-time trace visualization.
Run from repo root:

    python code/laya/cli.py                          # run all demos
    python code/laya/cli.py triage                   # run triage workflow
    python code/laya/cli.py review                   # run review loop
    python code/laya/cli.py optimizer                # run optimizer flow
    python code/laya/cli.py custom --nodes 3 --loops 2  # custom graph
"""
from __future__ import annotations

import argparse
import json
import sys
import time
from pathlib import Path
from dataclasses import dataclass, field
from typing import Any, Dict, List

_REPO = str(Path(__file__).resolve().parents[2])
if _REPO not in sys.path:
    sys.path.insert(0, _REPO)

from code.laya.workflow_engine import (
    Edge, NodeAction, NodeResult, ResilientWorkflow, WorkflowNode, WorkflowTrace,
)


# ──────────────────────────────────────────────────────────────────────
# Mock backend for demo (no model loading)
# ──────────────────────────────────────────────────────────────────────

@dataclass
class _Decision:
    answer: Any
    confidence: float = 0.9
    probabilities: dict = field(default_factory=dict)

class _Verdict:
    def __init__(self, answers):
        self.answers = answers

class ScriptedBackend:
    """Returns scripted answers in sequence, then repeats last."""
    def __init__(self, script: List[Dict[str, tuple]]):
        self.script = script
        self.call_idx = 0

    def decide(self, state, questions):
        preset = self.script[min(self.call_idx, len(self.script) - 1)]
        self.call_idx += 1
        answers = {}
        for qid in questions:
            if qid in preset:
                ans, conf = preset[qid]
                answers[qid] = _Decision(answer=ans, confidence=conf,
                                          probabilities={str(ans): conf, "other": 1-conf})
            else:
                answers[qid] = _Decision(answer="default", confidence=0.5)
        return _Verdict(answers)


# ──────────────────────────────────────────────────────────────────────
# ASCII trace renderer
# ──────────────────────────────────────────────────────────────────────

_COLORS = {
    "route":    "\033[36m",  # cyan
    "stop":     "\033[32m",  # green
    "escalate": "\033[31m",  # red
    "retry":    "\033[33m",  # yellow
    "execute":  "\033[35m",  # magenta
    "reset":    "\033[0m",
}
_B = "\033[1m"  # bold


def render_trace(result: Dict[str, Any], graph_desc: str = "") -> str:
    """Render a workflow execution as ASCII art with trace overlay."""
    lines = []
    trace = result["trace"]
    history = result["history"]
    visits = result.get("node_visits", {})

    lines.append(f"\n{_B}{'═'*60}{_COLORS['reset']}")
    lines.append(f"{_B}  LAYA WORKFLOW EXECUTION TRACE{_COLORS['reset']}")
    lines.append(f"{'═'*60}")

    if graph_desc:
        lines.append(f"\n{_B}Graph:{_COLORS['reset']}")
        for dl in graph_desc.split("\n"):
            lines.append(f"  {dl}")

    lines.append(f"\n{_B}Execution:{_COLORS['reset']}")

    for i, step in enumerate(history):
        node = step["node"]
        action = step["action"]
        color = _COLORS.get(action, "")
        conf = step.get("confidence", 0)
        lat = step.get("latency_ms", 0)
        detail = step.get("detail", "")
        visits_n = visits.get(node, 0)

        arrow = "  →" if i > 0 else "  ●"
        lines.append(
            f"  {arrow} {color}{_B}{node}{_COLORS['reset']}"
            f"  {color}[{action}]{_COLORS['reset']}"
            f"  conf={conf:.2f}  {lat:.1f}ms"
            f"  (visit #{visits_n})"
            f"  {detail}"
        )

    # summary
    lines.append(f"\n{_B}Summary:{_COLORS['reset']}")
    final = trace.get("final_action", "unknown")
    final_color = _COLORS.get(final, "")
    lines.append(f"  Final action:  {final_color}{_B}{final}{_COLORS['reset']}")
    lines.append(f"  Iterations:    {result.get('iterations', 0)}")
    lines.append(f"  Total latency: {trace.get('total_latency_ms', 0):.1f}ms")
    lines.append(f"  Node visits:   {visits}")
    if trace.get("loop_detected"):
        lines.append(f"  ⚠ Loop detected: {trace.get('loop_count', 0)} visits to same node")
    lines.append(f"{'═'*60}\n")

    return "\n".join(lines)


# ──────────────────────────────────────────────────────────────────────
# Demo workflows
# ──────────────────────────────────────────────────────────────────────

def demo_triage():
    """Email/ticket triage: classify → route → done."""
    classify = WorkflowNode(
        name="classify",
        questions={
            "category": {
                "type": "choice",
                "instructions": "Which category?",
                "criteria": {
                    "billing": "invoices, payments",
                    "technical": "bugs, outages",
                    "security": "threats, phishing",
                    "other": "everything else",
                },
            },
            "urgency": {"type": "score", "instructions": "Urgency?",
                         "criteria": ["low", "medium", "critical"]},
        },
        edge=Edge(
            condition={"security": "sec_gate", "billing": "route_bill",
                       "technical": "route_tech", "other": "route_other"},
            default="route_other",
        ),
        primary_q="category",
        state_fn=lambda s: {"text": s.get("text", ""), "from": s.get("from", "")},
    )

    sec_gate = WorkflowNode(
        name="sec_gate",
        questions={
            "is_threat": {"type": "choice", "instructions": "Real threat?",
                          "criteria": {"A": "no", "B": "yes"}},
            "severity": {"type": "score", "instructions": "Severity?",
                          "criteria": ["info", "warning", "critical"]},
        },
        edge=Edge(condition={"B": "STOP"}, default="STOP", min_confidence=0.6),
        primary_q="is_threat",
        state_fn=lambda s: {"text": s.get("text", "")},
        action_fn=lambda s, v: {**s, "_verdict": "BLOCKED" if v.answers["is_threat"].answer == "B" else "ALLOWED"},
    )

    route_bill = WorkflowNode(
        name="route_bill",
        questions={"action": {"type": "choice", "instructions": "Action?",
                               "criteria": {"refund": "refund", "invoice": "invoice", "none": "none"}}},
        edge=Edge(condition={"refund": "STOP", "invoice": "STOP", "none": "STOP"}),
        primary_q="action",
        state_fn=lambda s: {"text": s.get("text", "")},
    )

    route_tech = WorkflowNode(
        name="route_tech",
        questions={"action": {"type": "choice", "instructions": "Action?",
                               "criteria": {"investigate": "investigate", "ticket": "ticket", "none": "none"}}},
        edge=Edge(condition={"investigate": "STOP", "ticket": "STOP", "none": "STOP"}),
        primary_q="action",
        state_fn=lambda s: {"text": s.get("text", "")},
    )

    route_other = WorkflowNode(
        name="route_other",
        questions={"action": {"type": "choice", "instructions": "Action?",
                               "criteria": {"reply": "reply", "archive": "archive", "none": "none"}}},
        edge=Edge(condition={"reply": "STOP", "archive": "STOP", "none": "STOP"}),
        primary_q="action",
        state_fn=lambda s: {"text": s.get("text", "")},
    )

    wf = ResilientWorkflow(
        nodes={"classify": classify, "sec_gate": sec_gate,
               "route_bill": route_bill, "route_tech": route_tech,
               "route_other": route_other},
        start="classify", max_iterations=10,
    )

    graph = """classify ──┬── security ──▶ sec_gate ──▶ STOP
             ├── billing  ──▶ route_bill ──▶ STOP
             ├── technical ─▶ route_tech ──▶ STOP
             └── other    ──▶ route_other ─▶ STOP"""

    cases = [
        ("Billing: duplicate charge", "user@acme.com", "charge me twice for March",
         [{"category": ("billing", 0.92), "urgency": (1, 0.7)},
          {"action": ("refund", 0.95)}]),
        ("Security: phishing attempt", "evil@phish.tld", "verify your account at http://evil",
         [{"category": ("security", 0.95), "urgency": (2, 0.9)},
          {"is_threat": ("B", 0.95), "severity": (2, 0.9)}]),
        ("Tech: login bug", "dev@co.com", "login button returns 500",
         [{"category": ("technical", 0.88), "urgency": (1, 0.6)},
          {"action": ("ticket", 0.9)}]),
        ("Other: lunch plans", "team@co.com", "where for lunch?",
         [{"category": ("other", 0.85), "urgency": (0, 0.5)},
          {"action": ("archive", 0.9)}]),
    ]

    print(f"\n{_B}╔══════════════════════════════════════════════════╗{_COLORS['reset']}")
    print(f"{_B}║  Demo 1: Email/Ticket Triage (5 nodes)          ║{_COLORS['reset']}")
    print(f"{_B}╚══════════════════════════════════════════════════╝{_COLORS['reset']}")

    for title, sender, text, script in cases:
        print(f"\n{_B}📧 {title}{_COLORS['reset']}")
        print(f"   From: {sender}")
        print(f"   Text: {text[:60]}...")
        backend = ScriptedBackend(script)
        result = wf.run(backend, {"text": text, "from": sender})
        print(render_trace(result, graph))


def demo_review_loop():
    """Review loop with revision cycles."""
    review = WorkflowNode(
        name="review",
        questions={
            "quality": {"type": "score", "instructions": "Quality?",
                          "criteria": ["poor", "acceptable", "good", "excellent"]},
            "should_revise": {"type": "choice", "instructions": "Revise?",
                               "criteria": {"A": "yes, revise", "B": "no, approve"}},
        },
        edge=Edge(condition={"A": "review"}, default="STOP", min_confidence=0.5),
        primary_q="should_revise",
        max_retries=10,
        state_fn=lambda s: {"draft": s.get("draft", ""), "iteration": s.get("iteration", 0)},
        action_fn=lambda s, v: {
            **s,
            "iteration": s.get("iteration", 0) + 1,
            "_quality": v.answers["quality"].answer,
        },
    )

    wf = ResilientWorkflow(
        nodes={"review": review}, start="review", max_iterations=10,
    )

    graph = """review ──(revise)──▶ review ──(revise)──▶ review ──(approve)──▶ STOP"""

    cases = [
        ("Perfect on first try", [
            {"quality": (3, 0.95), "should_revise": ("B", 0.95)},
        ]),
        ("Two revision rounds", [
            {"quality": (0, 0.85), "should_revise": ("A", 0.9)},
            {"quality": (1, 0.8), "should_revise": ("A", 0.85)},
            {"quality": (3, 0.95), "should_revise": ("B", 0.95)},
        ]),
        ("Hits max iterations", [
            {"quality": (0, 0.9), "should_revise": ("A", 0.95)},
        ]),
    ]

    print(f"\n{_B}╔══════════════════════════════════════════════════╗{_COLORS['reset']}")
    print(f"{_B}║  Demo 2: Review Loop (self-loop pattern)        ║{_COLORS['reset']}")
    print(f"{_B}╚══════════════════════════════════════════════════╝{_COLORS['reset']}")

    for title, script in cases:
        print(f"\n{_B}📝 {title}{_COLORS['reset']}")
        backend = ScriptedBackend(script)
        result = wf.run(backend, {"draft": "draft v1", "iteration": 0})
        print(render_trace(result, graph))


def demo_optimizer():
    """Optimizer loop: continue → strategy → (loop)."""
    from code.laya.optimizer_integration import make_continue_node, make_strategy_node

    continue_node = make_continue_node()
    strategy_node = make_strategy_node(["aggressive", "conservative", "random_restart"])

    # Override: all strategy answers route back to continue_check (loop)
    strategy_node.edge = Edge(
        condition={"keep": "continue_check"},  # keep current → loop back
        default="continue_check",               # switch strategy → loop back
    )

    # Override: "continue" routes to strategy_check (not "evaluate")
    continue_node.edge = Edge(
        condition={"stop": "STOP", "continue": "strategy_check"},
        default="strategy_check",
    )

    wf = ResilientWorkflow(
        nodes={"continue_check": continue_node, "strategy_check": strategy_node},
        start="continue_check", max_iterations=20,
    )

    graph = """continue_check ──(stop)──▶ STOP
                   │
                (continue)
                   │
                   ▼
             strategy_check ──(keep/switch)──▶ (next iteration)"""

    cases = [
        ("Converged: stop early", [
            {"should_continue": ("stop", 0.95), "confidence_assessment": (2, 0.9)},
        ]),
        ("Keep optimizing", [
            {"should_continue": ("continue", 0.9), "confidence_assessment": (2, 0.8)},
            {"strategy_choice": ("keep", 0.9), "switch_urgency": (0, 0.7)},
        ]),
        ("Switch strategy", [
            {"should_continue": ("continue", 0.85), "confidence_assessment": (1, 0.6)},
            {"strategy_choice": ("aggressive", 0.9), "switch_urgency": (2, 0.85)},
        ]),
    ]

    print(f"\n{_B}╔══════════════════════════════════════════════════╗{_COLORS['reset']}")
    print(f"{_B}║  Demo 3: Optimizer Loop (strategy switching)    ║{_COLORS['reset']}")
    print(f"{_B}╚══════════════════════════════════════════════════╝{_COLORS['reset']}")

    for title, script in cases:
        print(f"\n{_B}🔄 {title}{_COLORS['reset']}")
        backend = ScriptedBackend(script)
        result = wf.run(backend, {
            "task": "outlier_detect", "score": -5.0, "best_score": -3.0,
            "step": 10, "history": [], "params": {}, "strategy": "default",
        })
        print(render_trace(result, graph))


def demo_custom(nodes: int, loops: int):
    """Build and run a custom chain of nodes with N nodes and N loops."""
    node_names = [f"step_{i}" for i in range(nodes)]
    workflow_nodes = {}

    for i, name in enumerate(node_names):
        is_last = (i == nodes - 1)
        is_loop_point = (loops > 0 and i == nodes - 1)

        if is_loop_point:
            # last node loops back to step_0
            edge = Edge(condition={"retry": node_names[0]}, default="STOP")
            max_ret = loops + 2
        elif is_last:
            edge = Edge(condition={}, default="STOP")
            max_ret = 0
        else:
            edge = Edge(condition={"go": node_names[i + 1]}, default="STOP")
            max_ret = 0

        workflow_nodes[name] = WorkflowNode(
            name=name,
            questions={"q": {"type": "choice", "instructions": "next?",
                              "criteria": {"go": "continue", "retry": "retry", "stop": "stop"}}},
            edge=edge,
            primary_q="q",
            max_retries=max_ret,
            state_fn=lambda s, idx=i: {**s, "_step": idx},
        )

    wf = ResilientWorkflow(nodes=workflow_nodes, start=node_names[0], max_iterations=nodes * 3)

    # script: go through all nodes, then loop back, then stop
    script = []
    for i in range(nodes):
        if i == nodes - 1 and loops > 0:
            script.append({"q": ("retry", 0.9)})  # loop back
        else:
            script.append({"q": ("go", 0.9)})
    # on second pass, stop
    for i in range(nodes):
        script.append({"q": ("stop", 0.9)})

    graph = " → ".join(node_names)
    if loops > 0:
        graph += f" → (loop to {node_names[0]}) × {loops}"

    print(f"\n{_B}╔══════════════════════════════════════════════════╗{_COLORS['reset']}")
    print(f"{_B}║  Demo 4: Custom Chain ({nodes} nodes, {loops} loops)          ║{_COLORS['reset']}")
    print(f"{_B}╚══════════════════════════════════════════════════╝{_COLORS['reset']}")
    print(f"\n{_B}⛓  Custom graph:{_COLORS['reset']}")
    print(f"   {graph}")

    backend = ScriptedBackend(script)
    result = wf.run(backend, {"custom": True})
    print(render_trace(result, graph))


# ──────────────────────────────────────────────────────────────────────
# CLI
# ──────────────────────────────────────────────────────────────────────

def main():
    p = argparse.ArgumentParser(description=__doc__,
                                formatter_class=argparse.RawDescriptionHelpFormatter)
    p.add_argument("demo", nargs="?", default="all",
                   choices=["all", "triage", "review", "optimizer", "custom"],
                   help="Which demo to run")
    p.add_argument("--nodes", type=int, default=5, help="Custom: number of nodes")
    p.add_argument("--loops", type=int, default=2, help="Custom: number of loops")
    args = p.parse_args()

    if args.demo in ("all", "triage"):
        demo_triage()
    if args.demo in ("all", "review"):
        demo_review_loop()
    if args.demo in ("all", "optimizer"):
        demo_optimizer()
    if args.demo in ("all", "custom"):
        demo_custom(args.nodes, args.loops)


if __name__ == "__main__":
    main()
