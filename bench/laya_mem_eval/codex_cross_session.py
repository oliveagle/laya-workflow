#!/usr/bin/env python3
"""Round-11: decisive cross-session A/B — does laya-mem help a codex-style agent?

The R8 replay (single session) could NOT show a difference: qwen3.8-flash-
next has a 262K context so even the 46-turn long scenario fits in the
window, and no_tools still scored 100%. Cross-session is where memory is
the ONLY mechanism that works:

  Session 1  — user shares N facts (same transcript in both modes).
               tool mode: agent persists each fact to laya-mem.
               no_tools:  agent just discusses (facts die with the session).
  Session 2  — NEW empty context (simulates a fresh codex session / new
               repo checkout). User asks about Session-1 facts.
               no_tools: 0% (nothing in context — cannot know the fact).
               tool:     recall retrieves persisted facts -> answers.

This is the production scenario laya-mem exists for: a coding agent
starting a new session MUST recover durable context that lives outside
its prompt window.

Scoring: deterministic fact-substring check (no LLM judge).
"""
from __future__ import annotations

import argparse
import json
import os
import sys
import tempfile
import urllib.request
from pathlib import Path

HERE = Path(__file__).parent
sys.path.insert(0, str(HERE))
from run import McpClient, _tool_payload

REPO = HERE.parent.parent
BIN = REPO / "target" / "debug" / "laya-workflow"
SPEC_DIR = REPO / "dsl" / "laya_mem"
MODEL_URL = "http://127.0.0.1:11032/v1"

SYS_TOOL = (
    "You are a coding agent with long-term memory. Use laya_mem_persist "
    "to store durable user facts (names, locations, roles, preferences). "
    "When asked about something from earlier, call laya_mem_recall first, "
    "then answer from what it returns. Keep answers short and factual."
)
SYS_NO_TOOLS = (
    "You are a coding agent answering questions. No memory tools available; "
    "answer from the conversation context only. Keep answers short."
)

# Session-1 facts (user -> assistant turns). Each has a fact that Session-2 will probe.
# 20 durable facts across infra, people, preferences, tooling.
# Each fact includes a distinctive token that no paraphrasing can collapse
# (Postgres, JIRA, Acme, okta, Grafana, etc.) so substring scoring is fair.
FACTS = [
    ("the deployment pipeline for repo A is Jenkins on the staging box", "Jenkins"),
    ("the on-call rotation this week is Priya and Marcus", "Priya"),
    ("the production DB is Postgres 16 on the shared cluster, read replica on port 5433", "Postgres"),
    ("the API gateway was switched to Kong last sprint", "Kong"),
    ("the release process requires a signed commit and a JIRA ticket", "JIRA"),
    ("internal services authenticate via Okta SSO; legacy auth was decommissioned", "Okta"),
    ("the metrics dashboard we watch is Grafana at the observability stack", "Grafana"),
    ("the message broker was migrated from RabbitMQ to NATS in Q2", "NATS"),
    ("the new feature flag service is Acme Flags, replacing the in-house toggle system", "Acme Flags"),
    ("our company switched to Linear for issue tracking from Jira last month", "Linear"),
    ("the mobile build pipeline uses Fastlane under the mobile-team org", "Fastlane"),
    ("the staging environment is hosted on Kubernetes in the us-west-2 region", "Kubernetes"),
    ("the secret manager is Vault with the kv-v2 backend enabled", "Vault"),
    ("the search backend is Meilisearch on the data plane", "Meilisearch"),
    ("our CDN provider is Cloudflare with Argo smart routing enabled", "Cloudflare"),
    ("the frontend build system is Vite with pnpm workspaces", "Vite"),
    ("the team uses Slack for chat and Notion for documentation", "Notion"),
    ("the payments provider is Stripe with 3DS required for EU cards", "Stripe"),
    ("the data warehouse is Snowflake on AWS us-east-1", "Snowflake"),
    ("the analytics events go through Segment then Kafka", "Segment"),
]

QUESTIONS = [
    "What CI system does the deployment pipeline for repo A use?",
    "Who is on call this week?",
    "What database does production use?",
    "What API gateway is in front of the services?",
    "What does the release process require before merging?",
    "What SSO provider handles authentication for internal services?",
    "What metrics dashboard do we watch?",
    "What message broker was the system migrated to in Q2?",
    "What feature flag service replaced the in-house toggle system?",
    "What issue tracker did the company switch to last month?",
    "What does the mobile build pipeline use under the mobile-team org?",
    "Where is the staging environment hosted?",
    "What secret manager is in use, and which backend?",
    "What is the search backend on the data plane?",
    "What CDN provider is configured for the website?",
    "What build system does the frontend use, and which package manager?",
    "What tool does the team use for documentation?",
    "What payments provider is in use, and what's required for EU cards?",
    "Where is the data warehouse hosted?",
    "Where do analytics events go first before reaching Kafka?",
]
EXPECT = [f.lower() for f in ["Jenkins","Priya","Postgres","Kong","JIRA","Okta","Grafana",
                                "NATS","Acme Flags","Linear","Fastlane","Kubernetes","Vault",
                                "Meilisearch","Cloudflare","Vite","Notion","Stripe","Snowflake",
                                "Segment"]]


def llm(messages, tools=None, max_tokens=500):
    req = urllib.request.Request(
        f"{MODEL_URL}/chat/completions",
        data=json.dumps({
            "model": "qwen3.8-flash-next",
            "messages": messages, "tools": tools,
            "max_tokens": max_tokens, "temperature": 0,
        }).encode(),
        headers={"Content-Type": "application/json"},
    )
    for attempt in range(3):
        try:
            with urllib.request.urlopen(req, timeout=90) as r:
                body = json.load(r)
            return body["choices"][0]
        except Exception as e:
            import time as _t
            _t.sleep(2 + attempt)
    return {"message": {"role": "assistant", "content": "[llm error]"}, "finish_reason": "stop"}


def mcp_tools(client):
    r = client.call("tools/list", {})
    out = []
    for t in r["tools"]:
        out.append({"type": "function", "function": {
            "name": t["name"], "description": t.get("description", ""),
            "parameters": t.get("inputSchema", {"type": "object"})}})
    return out


def invoke_tool(client, name, arguments):
    return _tool_payload(client.tool(name, arguments))


def session1_tool(client, tools):
    """Share facts; let the agent persist them."""
    stats = {"persist": 0, "recall": 0}
    for fact, _ in FACTS:
        msgs = [{"role": "system", "content": SYS_TOOL},
                {"role": "user", "content": f"For the record: {fact}."}]
        done = False
        while not done:
            ch = llm(msgs, tools)
            msg = ch.get("message", {})
            tc = msg.get("tool_calls")
            if tc:
                for call in tc:
                    fn = call["function"]["name"]
                    try:
                        args = json.loads(call["function"]["arguments"] or "{}")
                    except json.JSONDecodeError:
                        args = {}
                    result = invoke_tool(client, fn, args)
                    if fn == "laya_mem_persist":
                        stats["persist"] += 1
                    elif fn == "laya_mem_recall":
                        stats["recall"] += 1
                    msgs.append({"role": "assistant", "tool_calls": tc})
                    msgs.append({"role": "tool", "tool_call_id": call["id"],
                                 "content": json.dumps(result, ensure_ascii=False)[:4000]})
                continue
            msgs.append({"role": "assistant", "content": msg.get("content") or ""})
            done = True
    return stats


def session1_no_tools():
    """Share facts; agent just talks (nothing persisted)."""
    for fact, _ in FACTS:
        msgs = [{"role": "system", "content": SYS_NO_TOOLS},
                {"role": "user", "content": f"For the record: {fact}."}]
        ch = llm(msgs, tools=None)
        # discard — session is over, context gone
    return {"persist": 0, "recall": 0}


def session2_tool(client, tools):
    """Fresh empty context; ask questions, recall from memory."""
    stats = {"persist": 0, "recall": 0}
    answers = {}
    msgs = [{"role": "system", "content": SYS_TOOL}]
    for q in QUESTIONS:
        msgs.append({"role": "user", "content": q})
        done = False
        while not done:
            ch = llm(msgs, tools)
            msg = ch.get("message", {})
            tc = msg.get("tool_calls")
            if tc:
                for call in tc:
                    fn = call["function"]["name"]
                    try:
                        args = json.loads(call["function"]["arguments"] or "{}")
                    except json.JSONDecodeError:
                        args = {}
                    result = invoke_tool(client, fn, args)
                    if fn == "laya_mem_recall":
                        stats["recall"] += 1
                    msgs.append({"role": "assistant", "tool_calls": tc})
                    msgs.append({"role": "tool", "tool_call_id": call["id"],
                                 "content": json.dumps(result, ensure_ascii=False)[:4000]})
                continue
            content = msg.get("content") or ""
            if content.strip():
                answers[q] = content
            msgs.append({"role": "assistant", "content": content})
            done = True
    return stats, answers


def session2_no_tools():
    """Fresh empty context, no memory — must fail to know Session-1 facts."""
    answers = {}
    msgs = [{"role": "system", "content": SYS_NO_TOOLS}]
    for q in QUESTIONS:
        msgs.append({"role": "user", "content": q})
        ch = llm(msgs, tools=None)
        content = ch.get("message", {}).get("content") or ""
        if content.strip():
            answers[q] = content
        msgs.append({"role": "assistant", "content": content})
    return {"persist": 0, "recall": 0}, answers


def score(answers):
    rows = []
    for q, exp in zip(QUESTIONS, EXPECT):
        blob = (answers.get(q) or "").lower()
        hit = exp in blob
        rows.append({"question": q, "expected": exp, "hit": hit,
                     "answer": answers.get(q, "")[:150]})
    return rows


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--mode", default="tool", choices=["tool", "no_tools"])
    args = ap.parse_args()

    db = tempfile.mktemp(suffix=".sqlite")
    try:
        os.remove(db)
    except FileNotFoundError:
        pass
    client = McpClient(BIN, SPEC_DIR, db)
    tools = mcp_tools(client) if args.mode == "tool" else None
    try:
        if args.mode == "tool":
            s1 = session1_tool(client, tools)
            s2, answers = session2_tool(client, tools)
        else:
            s1 = session1_no_tools()
            s2, answers = session2_no_tools()
    finally:
        client.close()

    rows = score(answers)
    print(f"\n# Cross-session codex A/B ({args.mode}) — fresh Session-2 context")
    print(f"session1 usage: persist={s1['persist']} recall={s1['recall']} | session2: persist={s2['persist']} recall={s2['recall']}")
    for r in rows:
        print(f"  [{'PASS' if r['hit'] else 'FAIL'}] {r['question']}: expect {r['expected']!r}")
        print(f"        answer: {r['answer']}")
    n_pass = sum(1 for r in rows if r["hit"])
    print(f"\ncross-session recall: {n_pass}/{len(rows)} = {n_pass/len(rows):.0%}")
    out = HERE / "reports" / f"codex_cross_session_{args.mode}.json"
    out.write_text(json.dumps({"mode": args.mode, "s1": s1, "s2": s2, "rows": rows,
                               "answers": answers}, indent=2, ensure_ascii=False))
    print(f"wrote {out}")


if __name__ == "__main__":
    main()
