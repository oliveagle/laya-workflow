#!/usr/bin/env python3
"""Round-8: end-to-end codex-style replay through the real laya-mem MCP server.

Drives a qwen3.8-flash-next "codex surrogate" that has the four laya_mem
tools exposed as OpenAI function-calling tools. The conversation is a
realistic multi-turn session: the user shares facts about Carol's
relocation + job, then asks questions that REQUIRE the agent to recall
previously persisted facts.

Two controller modes:
  * tool    — full tool-calling loop (persist/recall/assess/retrieve);
              the model decides what to store and what to fetch.
  * script  — deterministic script (persist every turn, recall before
              each question) as a no-brain sanity baseline.

Scoring per question:
  * fact_hit   — the expected fact substring appears in the agent's
                 final answer (no LLM judge needed; deterministic).
  * tool usage — persist/recall counts (instrumentation).

Usage:
    python3 bench/laya_mem_eval/codex_replay.py --mode tool
    python3 bench/laya_mem_eval/codex_replay.py --mode script
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

SYSTEM = (
    "You are a coding agent with a long-term memory toolset. Use the "
    "available tools to persist important user facts and to recall them "
    "when answering later questions. Prefer persist for new durable facts "
    "(names, moves, jobs, preferences) and recall before answering any "
    "question that references earlier conversation. Keep answers short."
)


def llm(messages, tools=None, max_tokens=500):
    req = urllib.request.Request(
        f"{MODEL_URL}/chat/completions",
        data=json.dumps({
            "model": "qwen3.8-flash-next",
            "messages": messages,
            "tools": tools,
            "max_tokens": max_tokens,
            "temperature": 0,
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
    """Build OpenAI function-calling tool list from the MCP server."""
    r = client.call("tools/list", {})
    out = []
    for t in r["tools"]:
        out.append({
            "type": "function",
            "function": {
                "name": t["name"],
                "description": t.get("description", ""),
                "parameters": t.get("inputSchema", {"type": "object"}),
            },
        })
    return out


def invoke_tool(client, name, arguments):
    resp = client.tool(name, arguments)
    return _tool_payload(resp)


# ——— scenarios (user message, expected fact to recall later) ———
SCENARIO = [
    {"role": "user", "content": "Hi! Quick update: Carol just told me she moved to Berlin for a new job."},
    {"role": "user", "content": "Also Carol's new role is Senior Engineer at Acme Robotics. She's been there since March."},
    {"role": "user", "content": "One more thing — Carol said she prefers async communication over long meetings."},
    {"role": "user", "content": "By the way, David is now the tech lead on our team, replacing Ana."},
    {"role": "user", "content": "Where does Carol live now, and what is her role and employer?"},
    {"role": "user", "content": "And how does Carol prefer to communicate?"},
    {"role": "user", "content": "Who is the current tech lead on our team?"},
]

EXPECT = {
    "Where does Carol live now, and what is her role and employer?":
        ["Berlin", "Senior Engineer", "Acme"],
    "And how does Carol prefer to communicate?":
        ["async"],
    "Who is the current tech lead on our team?":
        ["David"],
}


def run_script(client):
    """Deterministic baseline: persist every turn, recall before Q."""
    turns = SCENARIO
    q_turns = [t for t in turns if t["content"].endswith("?")]
    stats = {"persist": 0, "recall": 0, "assess": 0}
    answers = {}
    for turn in turns:
        content = turn["content"]
        if content.endswith("?"):
            mems = invoke_tool(client, "laya_mem_recall", {"limit": 5, "include_relations": False, "query": content})
            stats["recall"] += 1
            blob = " ".join(str(m) for m in (mems.get("memories") or []))
            answers[content] = blob
        else:
            invoke_tool(client, "laya_mem_persist", {"content": content, "entities": [], "type_scores": {"episodic": 0.5, "semantic": 0.3, "procedural": 0.1, "preference": 0.1}})
            stats["persist"] += 1
    return stats, answers


def run_tool(client, tools):
    """Full tool-calling loop with the LLM deciding what to do."""
    stats = {"persist": 0, "recall": 0, "assess": 0}
    answers = {}
    # conversation history: system + all scenario turns (questions asked at end)
    messages = [{"role": "system", "content": SYSTEM}]
    for turn in SCENARIO:
        messages.append({"role": "user", "content": turn["content"]})
        # let the agent act on this turn (tools + any reply) before next turn
        done = False
        while not done:
            ch = llm(messages, tools)
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
                    if fn == "laya_mem_persist": stats["persist"] += 1
                    elif fn == "laya_mem_recall": stats["recall"] += 1
                    elif fn == "laya_mem_assess": stats["assess"] += 1
                    messages.append({"role": "assistant", "tool_calls": tc})
                    messages.append({"role": "tool", "tool_call_id": call["id"],
                                     "content": json.dumps(result, ensure_ascii=False)[:4000]})
                continue  # loop: model decides next step after tool results
            # no tool call -> final assistant reply for this turn
            content = msg.get("content") or ""
            if turn["content"].endswith("?") and content.strip():
                answers[turn["content"]] = content
            messages.append({"role": "assistant", "content": content})
            done = True
    return stats, answers


def score(answers):
    rows = []
    for q, expect in EXPECT.items():
        blob = (answers.get(q) or "").lower()
        hits = [e.lower() in blob for e in expect]
        rows.append({"question": q[:50], "expected": expect,
                     "hit": all(hits), "hits": hits, "answer": answers.get(q, "")[:120]})
    return rows


LONG_SCENARIO = json.load(open('/tmp/long_scenario.json'))


def run_no_tools():
    """A/B control: same conversation but NO laya-mem tools — the agent
    must remember everything in its own context window. Measures how
    much laya-mem adds for a short session (baseline)."""
    stats = {"persist": 0, "recall": 0, "assess": 0}
    answers = {}
    messages = [{"role": "system", "content":
                 "You are a coding agent answering questions about the "
                 "conversation history. No memory tools available — rely "
                 "on the conversation itself. Keep answers short."}]
    for turn in SCENARIO:
        messages.append({"role": "user", "content": turn["content"]})
        ch = llm(messages, tools=None)
        content = ch.get("message", {}).get("content") or ""
        if turn["content"].endswith("?"):
            answers[turn["content"]] = content
        messages.append({"role": "assistant", "content": content})
    return stats, answers


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--mode", default="tool", choices=["tool", "script", "no_tools"])
    ap.add_argument("--long", action="store_true", help="use the 46-turn long-horizon scenario")
    args = ap.parse_args()
    global SCENARIO, EXPECT
    if args.long:
        SCENARIO = LONG_SCENARIO["turns"]
        EXPECT = LONG_SCENARIO["expect"]

    db = tempfile.mktemp(suffix=".sqlite")
    try: os.remove(db)
    except FileNotFoundError: pass
    client = McpClient(BIN, SPEC_DIR, db)
    try:
        tools = mcp_tools(client) if args.mode == "tool" else None
        if args.mode == "tool":
            stats, answers = run_tool(client, tools)
        elif args.mode == "script":
            stats, answers = run_script(client)
        else:
            stats, answers = run_no_tools()
    finally:
        client.close()

    rows = score(answers)
    print(f"\n# Codex-style replay ({args.mode} mode)")
    print(f"tool usage: persist={stats['persist']} recall={stats['recall']} assess={stats['assess']}")
    for r in rows:
        mark = "PASS" if r["hit"] else "FAIL"
        print(f"  [{mark}] {r['question']}: expected {r['expected']} -> hits {r['hits']}")
        print(f"        answer: {r['answer']}")
    n_pass = sum(1 for r in rows if r["hit"])
    print(f"\nfact recall: {n_pass}/{len(rows)} = {n_pass/len(rows):.0%}")

    out = HERE / "reports" / f"codex_replay_{args.mode}.json"
    out.write_text(json.dumps({"mode": args.mode, "stats": stats, "rows": rows,
                               "answers": answers}, indent=2))
    print(f"wrote {out}")


if __name__ == "__main__":
    main()
