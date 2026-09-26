"""Laya Workflow Engine — HTTP API server.

Exposes workflow graphs as HTTP endpoints for remote execution.
Each request carries the workflow state, and the engine runs the
Laya-node graph synchronously, returning the full trace.

Usage:
    python -m code.laya.server --port 8100
    python -m code.laya.server --port 8100 --model-dir /path/to/laya
"""
from __future__ import annotations

import argparse
import json
import sys
import time
from pathlib import Path
from typing import Any, Dict

# ensure repo root on path
_REPO = str(Path(__file__).resolve().parents[2])
if _REPO not in sys.path:
    sys.path.insert(0, _REPO)

from code.laya.workflow_engine import (
    Edge, NodeAction, ResilientWorkflow, WorkflowNode, WorkflowTrace,
)


# ──────────────────────────────────────────────────────────────────────
# Pre-built workflow graphs
# ──────────────────────────────────────────────────────────────────────

def _build_continue_check_graph(max_iterations: int = 50) -> ResilientWorkflow:
    """The continue_check + strategy_check graph for optimization loops."""
    from code.laya.optimizer_integration import make_continue_node, make_strategy_node

    continue_node = make_continue_node()
    strategy_node = make_strategy_node(["aggressive", "conservative", "random_restart"])

    # Override: all strategy answers route back to continue_check (loop)
    strategy_node.edge = Edge(
        condition={"keep": "continue_check"},
        default="continue_check",
    )

    # Override continue edge: "continue" goes to strategy_check (not "evaluate")
    # In the full optimizer integration, "evaluate" is an action node.
    # Here in the standalone graph, we connect continue → strategy directly.
    continue_node.edge = Edge(
        condition={"stop": "STOP", "continue": "strategy_check"},
        default="strategy_check",
    )

    return ResilientWorkflow(
        nodes={
            "continue_check": continue_node,
            "strategy_check": strategy_node,
        },
        start="continue_check",
        max_iterations=max_iterations,
    )


def _build_triage_graph() -> ResilientWorkflow:
    """Email/ticket triage: classify → route → validate → done."""
    classify = WorkflowNode(
        name="classify",
        questions={
            "category": {
                "type": "choice",
                "instructions": "Which category does this item belong to?",
                "criteria": {
                    "billing": "invoices, payments, refunds",
                    "technical": "bugs, outages, system errors",
                    "security": "security incidents, phishing, threats",
                    "other": "everything else",
                },
            },
            "urgency": {
                "type": "score",
                "instructions": "How urgent is this?",
                "criteria": ["low", "medium", "critical"],
            },
        },
        edge=Edge(
            condition={"security": "security_gate", "billing": "route_billing",
                       "technical": "route_tech", "other": "route_general"},
            default="route_general",
        ),
        primary_q="category",
        state_fn=lambda s: {"text": s.get("text", ""), "from": s.get("from", "")},
    )

    security_gate = WorkflowNode(
        name="security_gate",
        questions={
            "is_threat": {
                "type": "choice",
                "instructions": "Is this a genuine security threat?",
                "criteria": {"A": "no, likely false alarm", "B": "yes, genuine threat"},
            },
            "severity": {
                "type": "score",
                "instructions": "Severity level?",
                "criteria": ["info", "warning", "critical"],
            },
        },
        edge=Edge(
            condition={"B": "STOP"},  # genuine threat → block
            default="STOP",           # false alarm → also stop (with low severity)
            min_confidence=0.6,
        ),
        primary_q="is_threat",
        state_fn=lambda s: {"text": s.get("text", ""), "from": s.get("from", "")},
        action_fn=lambda s, v: {
            **s,
            "_verdict": {
                "threat": v.answers["is_threat"].answer,
                "severity": v.answers["severity"].answer,
            },
        },
    )

    route_billing = WorkflowNode(
        name="route_billing",
        questions={
            "action": {
                "type": "choice",
                "instructions": "What billing action is needed?",
                "criteria": {
                    "refund": "process refund",
                    "invoice": "send invoice",
                    "none": "no action needed",
                },
            }
        },
        edge=Edge(condition={"refund": "STOP", "invoice": "STOP", "none": "STOP"}),
        primary_q="action",
        state_fn=lambda s: {"text": s.get("text", "")},
    )

    route_tech = WorkflowNode(
        name="route_tech",
        questions={
            "action": {
                "type": "choice",
                "instructions": "What technical action?",
                "criteria": {
                    "investigate": "needs investigation",
                    "ticket": "create ticket",
                    "none": "no action needed",
                },
            }
        },
        edge=Edge(condition={"investigate": "STOP", "ticket": "STOP", "none": "STOP"}),
        primary_q="action",
        state_fn=lambda s: {"text": s.get("text", "")},
    )

    route_general = WorkflowNode(
        name="route_general",
        questions={"action": {"type": "choice", "instructions": "Action?",
                               "criteria": {"reply": "reply", "archive": "archive", "none": "none"}}},
        edge=Edge(condition={"reply": "STOP", "archive": "STOP", "none": "STOP"}),
        primary_q="action",
        state_fn=lambda s: {"text": s.get("text", "")},
    )

    return ResilientWorkflow(
        nodes={
            "classify": classify,
            "security_gate": security_gate,
            "route_billing": route_billing,
            "route_tech": route_tech,
            "route_general": route_general,
        },
        start="classify",
        max_iterations=10,
    )


def _build_review_loop_graph(max_iterations: int = 10) -> ResilientWorkflow:
    """Review loop: draft → Laya review → approve/revise → done.

    This demonstrates the loop pattern — Laya can send back for revision.
    """
    review = WorkflowNode(
        name="review",
        questions={
            "quality": {
                "type": "score",
                "instructions": "How is the quality of this draft?",
                "criteria": ["poor", "acceptable", "good", "excellent"],
            },
            "should_revise": {
                "type": "choice",
                "instructions": "Should this be sent back for revision?",
                "criteria": {"A": "yes, needs revision", "B": "no, approve as-is"},
            },
        },
        edge=Edge(
            condition={"A": "review"},  # self-loop: revise → re-review
            default="STOP",             # approve → done
            min_confidence=0.5,
        ),
        primary_q="should_revise",
        max_retries=10,  # allow revision loops (bounded by max_iterations)
        state_fn=lambda s: {"draft": s.get("draft", ""), "iteration": s.get("iteration", 0)},
        action_fn=lambda s, v: {
            **s,
            "iteration": s.get("iteration", 0) + 1,
            "_review": {
                "quality": v.answers["quality"].answer,
                "revise": v.answers["should_revise"].answer,
            },
        },
    )

    return ResilientWorkflow(
        nodes={"review": review},
        start="review",
        max_iterations=max_iterations,
    )


# ──────────────────────────────────────────────────────────────────────
# Workflow registry
# ──────────────────────────────────────────────────────────────────────

WORKFLOWS: Dict[str, lambda: ResilientWorkflow] = {
    "optimizer": lambda: _build_continue_check_graph(),
    "triage": lambda: _build_triage_graph(),
    "review_loop": lambda: _build_review_loop_graph(),
}


# ──────────────────────────────────────────────────────────────────────
# HTTP server (stdlib only, no deps)
# �parameter>
# ──────────────────────────────────────────────────────────────────────

def _handle_request(handler, laya_backend):
    """Handle a single HTTP request."""
    from http.server import BaseHTTPRequestHandler
    import io

    # read body
    content_length = int(handler.headers.get("Content-Length", 0))
    body = handler.rfile.read(content_length) if content_length > 0 else b""

    # parse path
    path = handler.path.split("?")[0]

    try:
        if path == "/health":
            response = {"status": "ok", "workflows": list(WORKFLOWS.keys())}
            status = 200

        elif path == "/workflows":
            response = {name: {"description": f"Laya workflow: {name}"}
                        for name in WORKFLOWS}
            status = 200

        elif path.startswith("/run/"):
            workflow_name = path[len("/run/"):]
            if workflow_name not in WORKFLOWS:
                response = {"error": f"unknown workflow: {workflow_name}",
                            "available": list(WORKFLOWS.keys())}
                status = 404
            else:
                data = json.loads(body) if body else {}
                state = data.get("state", {})
                max_iter = data.get("max_iterations")

                workflow = WORKFLOWS[workflow_name]()
                if max_iter:
                    workflow.max_iterations = max_iter

                t0 = time.time()
                result = workflow.run(laya_backend, state)
                result["server_latency_ms"] = round((time.time() - t0) * 1000, 1)
                response = result
                status = 200

        elif path == "/trace":
            # demo: run triage workflow with sample input
            data = json.loads(body) if body else {}
            sample = data or {
                "text": "URGENT: Production database is down, payments failing",
                "from": "sre@company.com",
            }
            workflow = _build_triage_graph()
            result = workflow.run(laya_backend, sample)
            response = result
            status = 200

        else:
            response = {"error": "not found",
                        "endpoints": ["/health", "/workflows", "/run/<name>", "/trace"]}
            status = 404

    except json.JSONDecodeError as e:
        response = {"error": f"invalid JSON: {e}"}
        status = 400
    except Exception as e:
        response = {"error": str(e), "type": type(e).__name__}
        status = 500

    # send response
    output = json.dumps(response, ensure_ascii=False, indent=2, default=str).encode()
    handler.send_response(status)
    handler.send_header("Content-Type", "application/json")
    handler.send_header("Content-Length", str(len(output)))
    handler.end_headers()
    handler.wfile.write(output)


def run_server(host: str = "0.0.0.0", port: int = 8100, model_dir: str | None = None):
    """Start the HTTP server."""
    from http.server import HTTPServer, BaseHTTPRequestHandler

    # load Laya backend
    if model_dir:
        import os
        os.environ["LAYA_MODEL_DIR"] = model_dir

    from code.laya.engine import LayaEngine
    print(f"[server] Loading Laya engine...")
    laya_backend = LayaEngine.shared()
    print(f"[server] Laya loaded on {laya_backend.device} in {laya_backend.load_s:.1f}s")

    class Handler(BaseHTTPRequestHandler):
        def do_POST(self):
            _handle_request(self, laya_backend)

        def do_GET(self):
            _handle_request(self, laya_backend)

        def log_message(self, format, *args):
            # quiet logging
            pass

    server = HTTPServer((host, port), Handler)
    print(f"[server] Listening on http://{host}:{port}")
    print(f"[server] Workflows: {list(WORKFLOWS.keys())}")
    print(f"[server] POST /run/triage  with {{'state': {{'text': '...', 'from': '...'}}}}")
    print(f"[server] POST /run/optimizer with {{'state': {{'score': -5.0, ...}}}}")
    print(f"[server] POST /run/review_loop with {{'state': {{'draft': '...', 'iteration': 0}}}}")

    try:
        server.serve_forever()
    except KeyboardInterrupt:
        print("\n[server] Shutting down.")
        server.shutdown()


# ──────────────────────────────────────────────────────────────────────
# CLI
# ──────────────────────────────────────────────────────────────────────

def main():
    p = argparse.ArgumentParser(description=__doc__)
    p.add_argument("--host", default="0.0.0.0")
    p.add_argument("--port", type=int, default=8100)
    p.add_argument("--model-dir", type=str, default=None,
                   help="Path to Laya model directory (or set LAYA_MODEL_DIR env)")
    p.add_argument("--list", action="store_true", help="List available workflows and exit")
    args = p.parse_args()

    if args.list:
        print("Available workflows:")
        for name in WORKFLOWS:
            print(f"  {name}")
        return

    run_server(host=args.host, port=args.port, model_dir=args.model_dir)


if __name__ == "__main__":
    main()
