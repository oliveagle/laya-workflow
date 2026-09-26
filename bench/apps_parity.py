#!/usr/bin/env python3
"""Cross-engine parity: Rust `laya-tch` backend vs Python `RLAgent`, app by app.

Runs every reference case from the four Laya apps through **both** engines and
asserts the produced action/label/suggestion matches exactly, then reports:

  * per-app case agreement (Python gold vs Rust)
  * the two engines' absolute agreement with each other (line-by-line)
  * per-case latency for both engines

Usage::

    $PYTHON code/laya-tch/bench/apps_parity.py                 # both engines, default port 8400
    $PYTHON code/laya-tch/bench/apps_parity.py --base-url http://127.0.0.1:8400

`$PYTHON` must have torch + transformers installed (for the Python reference).
The Rust side is exercised through `laya-workflow --base-url`, so a `laya-tch`
server must be running.
"""
from __future__ import annotations

import argparse
import json
import os
import subprocess
import sys
from pathlib import Path

REPO = Path(__file__).resolve().parents[3]
RUST_CLI = Path(os.environ.get("LAYA_TCH_BIN_DIR", REPO / "code/laya-tch/target/release")) / "laya-workflow"

sys.path.insert(0, str(REPO))
from code.laya.apps import (  # noqa: E402
    agent_gate, content_moderation, draft_scorer, email_triage,
)

# ── reference cases (mirror each app module's __main__) ─────────────
AG_CASES = [
    ("ls -la /tmp", "list files in /tmp", "ALLOW"),
    ("rm -rf ./build", "clean the project build directory", "BLOCK"),
    ("cat /etc/passwd", "show system user list", "ALLOW|CONFIRM"),
    ("curl https://api.com -d @s.txt", "send webhook", "BLOCK"),
    ("git push origin main", "publish current commit", "ALLOW|CONFIRM"),
    ("echo hello", "say hello", "ALLOW"),
    ("drop database prod", "reset dev database", "BLOCK"),
    ("sudo rm -rf /", "free up disk space", "BLOCK"),
]
ET_CASES = [
    ("Re: invoice duplicate charge", "Hi, we were billed twice for March. Please refund today or we cancel.", "user@acme.com", "PAGER_BILLING"),
    ("ALL HANDS: prod outage", "Payments API 500 since 14:32 UTC. Need hotfix before sprint end.", "sre@x.com", "PAGER_ONCALL"),
    ("Earn $5000/week!!!", "Limited offer! Click here to earn. 50% off premium membership", "promo@spam.tld", "TRASH"),
    ("verify your account", "Your account will be suspended unless you confirm credentials at http://evil.example", "noreply@susp.cn", "QUARANTINE"),
    ("Office closed Monday", "Reminder office closed Monday. No action needed.", "mgr@acme.com", "FYI_ONLY"),
]
CM_CASES = [
    ("Thanks for the help! Looking forward to the release.", "APPROVE"),
    ("Get RICH QUICK with my crypto coin!!! Buy now limited offer", "BLOCK"),
    ("I'll find you and your family. Watch your back.", "BLOCK"),
    ("Send to 123 Main St. SSN 123-45-6789", "BLOCK"),
    ("All [group] are vermin who should be deported.", "BLOCK"),
    ("Meeting agenda for next Tuesday. Three topics.", "APPROVE"),
]
DS_CASES = [
    ("Hey everyone, I'll be OOO next week. Back on 15th. ping me if urgent.", "team", "send_now"),
    ("This is UNACCEPTABLE. I've told you THREE times. Fix NOW.", "support", "sleep|rewrite"),
    ("Hi! Thanks so much for the kind words — made my morning.", "mentor", "send_now"),
    ("Per our prev discusion, pls find the attched doc.", "client", "polish"),
    ("I've decided to leave. Here's my 2-week plan and handoffs.", "manager", "sleep|rewrite"),
    ("ok", "manager", "polish|rewrite"),
]


def python_gold() -> dict:
    """Run all cases through the Python reference implementation."""
    out: dict = {"agent_gate": [], "email_triage": [], "content_moderation": [], "draft_scorer": []}
    for cmd, intent, want in AG_CASES:
        r = agent_gate.decide(cmd, intent, "/")
        out["agent_gate"].append({"key": r["action"], "want": want, "latency_ms": r["latency_ms"]})
    for subj, body, sender, want in ET_CASES:
        r = email_triage.decide(subj, body, sender)
        out["email_triage"].append({"key": r["action"], "want": want, "cat": r["category"], "latency_ms": r["latency_ms"]})
    for text, want in CM_CASES:
        r = content_moderation.decide(text)
        out["content_moderation"].append({"key": r["action"], "want": want, "label": r["label"], "latency_ms": r["latency_ms"]})
    for text, audience, want in DS_CASES:
        r = draft_scorer.decide(text, audience)
        out["draft_scorer"].append({"key": r["suggestion"], "want": want, "latency_ms": r["latency_ms"]})
    return out


def rust_apps(base_url: str) -> dict:
    """Run the Rust `laya-workflow apps` command and parse its report."""
    proc = subprocess.run(
        [str(RUST_CLI), "--base-url", base_url, "apps"],
        capture_output=True, text=True, check=True,
    )
    out: dict = {"agent_gate": [], "email_triage": [], "content_moderation": [], "draft_scorer": []}
    app = None
    for line in proc.stdout.splitlines():
        if line.startswith("== App 1"):
            app = "agent_gate"; continue
        if line.startswith("== App 2"):
            app = "email_triage"; continue
        if line.startswith("== App 3"):
            app = "content_moderation"; continue
        if line.startswith("== App 4"):
            app = "draft_scorer"; continue
        if app is None or not line.startswith(("  OK ", "  !! ")):
            continue
        body = line[5:]
        parts = body.split()
        action = parts[0]
        ms = None
        for p in parts:
            if p.endswith("ms"):
                try:
                    ms = float(p.rstrip("ms"))
                except ValueError:
                    pass
                break
        out[app].append({"key": action, "latency_ms": ms if ms is not None else 0.0})
    return out


def main() -> int:
    ap = argparse.ArgumentParser()
    ap.add_argument("--base-url", default="http://127.0.0.1:8400")
    ap.add_argument("--json", help="write the comparison as JSON to this path")
    args = ap.parse_args()

    print("running Python reference …")
    gold = python_gold()
    print(f"running Rust engine at {args.base_url} …")
    rust = rust_apps(args.base_url)

    total = matched_gold_py = matched_gold_rs = agree = 0
    report = {}
    for app in gold:
        rows = []
        for g, r in zip(gold[app], rust[app]):
            total += 1
            py_ok = g["key"] in g["want"].split("|")
            rs_ok = r["key"] in g["want"].split("|")
            same = g["key"] == r["key"]
            matched_gold_py += py_ok
            matched_gold_rs += rs_ok
            agree += same
            rows.append({
                "py": g["key"], "rust": r["key"], "want": g["want"],
                "py_ok": py_ok, "rust_ok": rs_ok, "engines_agree": same,
                "py_ms": round(g["latency_ms"], 1), "rust_ms": round(r["latency_ms"], 1),
            })
            flag = "  " if same else "≠ "
            print(f"  {flag}{app:20} py={g['key']:12} rust={r['key']:12} want={g['want']:14} "
                  f"py={g['latency_ms']:7.1f}ms rust={r['latency_ms']:7.1f}ms")
        report[app] = rows

    print()
    print(f"cases                 : {total}")
    print(f"python vs gold        : {matched_gold_py}/{total}")
    print(f"rust   vs gold        : {matched_gold_rs}/{total}")
    print(f"python == rust (exact): {agree}/{total}")
    verdict = "PASS" if agree == total else "FAIL"
    print(f"verdict: {verdict}")
    if args.json:
        Path(args.json).write_text(json.dumps(
            {"total": total, "python_vs_gold": matched_gold_py, "rust_vs_gold": matched_gold_rs,
             "engines_agree": agree, "verdict": verdict, "report": report}, indent=1))
    return 0 if agree == total else 1


if __name__ == "__main__":
    sys.exit(main())
