#!/usr/bin/env python3
"""Round-12: three-method comparison — accuracy, time, token cost, long-horizon.

Answers the question "does laya-mem help codex, and how much faster/better
than traditional memory?" with hard numbers.

Methods (all answer the SAME 20 questions about Session-1 facts):
  laya_mem    — session1: persist every fact (tool call); session2 (fresh
                context): recall per question, answer from recall.
  full_ctx    — traditional "remember everything": session2 prompt = ALL
                session-1 turns + question. Context grows O(N) per question.
  no_memory   — session2 = empty + question. Baseline (refuses: 0%).

Long-horizon: D distractor facts are added to session-1 (N = 20 + D), so
full_ctx prefill grows linearly while laya_mem context stays ~constant.

Metrics per method: fact-accuracy, wall-clock, #LLM calls, prompt tokens
per question (from server usage), and per-question latency.
"""
from __future__ import annotations

import argparse
import json
import os
import sys
import tempfile
import time
import urllib.request
from pathlib import Path

HERE = Path(__file__).parent
sys.path.insert(0, str(HERE))
from run import McpClient, _tool_payload
from codex_cross_session import FACTS, QUESTIONS, EXPECT, llm, mcp_tools, invoke_tool

REPO = HERE.parent.parent
BIN = REPO / "target" / "debug" / "laya-workflow"
SPEC_DIR = REPO / "dsl" / "laya_mem"
MODEL_URL = "http://127.0.0.1:11032/v1"

SYS_TOOL = (
    "You are a coding agent with long-term memory. Use laya_mem_persist "
    "to store durable user facts. When asked about something, call "
    "laya_mem_recall first, then answer from what it returns. Keep "
    "answers short and factual — always output a non-empty answer."
)
SYS_FULL = (
    "You are a coding agent. All relevant facts from past sessions are "
    "listed below in the conversation history. Answer questions using "
    "them. Keep answers short. Always output a non-empty answer."
)

# Deterministic realistic distractor facts (infra/ops) to scale long-horizon.
DISTRACTOR_TMPL = [
    "the {i}th microservice is deployed to the {env} cluster with {n} replicas",
    "the {i}th team owns the {svc} service and its {queue} queue",
    "the {i}th alert routes to {pager} with {sev}-minute escalation",
    "the {i}th dashboard shows {metric} for the {svc} group",
    "the {i}th repo uses {ci} with {branch} as the protected branch",
    "the {i}th database shard lives on {host} with {n} GB storage",
]
_WORDS = ["api", "web", "auth", "search", "billing", "notify", "stream", "batch",
          "cache", "queue", "sync", "report", "ingest", "export", "config", "health"]
_CI = ["GitHub Actions", "GitLab CI", "CircleCI", "Drone"]
_PAGER = ["PagerDuty", "Opsgenie", "VictorOps"]


def distractor_facts(n: int) -> list[str]:
    out = []
    for i in range(n):
        tmpl = DISTRACTOR_TMPL[i % len(DISTRACTOR_TMPL)]
        s = tmpl.format(
            i=i + 1000, env="staging" if i % 2 else "prod",
            n=2 + (i % 5), svc=_WORDS[i % len(_WORDS)],
            queue=_WORDS[(i + 3) % len(_WORDS)],
            pager=_PAGER[i % 3], sev=5 + (i % 15),
            metric=_WORDS[(i + 7) % len(_WORDS)],
            ci=_CI[i % 4], branch="main" if i % 2 else "dev",
            host=f"srv{i % 7}", )
        out.append(s)
    return out


def llm_full(messages, tools=None, max_tokens=500):
    """Like llm() but returns (choice, usage)."""
    req = urllib.request.Request(
        f"{MODEL_URL}/chat/completions",
        data=json.dumps({
            "model": "qwen3.8-flash-next", "messages": messages, "tools": tools,
            "max_tokens": max_tokens, "temperature": 0,
        }).encode(),
        headers={"Content-Type": "application/json"},
    )
    for attempt in range(3):
        try:
            with urllib.request.urlopen(req, timeout=90) as r:
                body = json.load(r)
            return body["choices"][0], body.get("usage", {})
        except Exception as e:
            import time as _t
            _t.sleep(2 + attempt)
    return {"message": {"role": "assistant", "content": "[llm error]"}, "finish_reason": "stop"}, {}


def count_prompt_tokens(resp: dict) -> int:
    """resp may be a full body {'usage': {...}} or a bare usage dict."""
    try:
        if "usage" in resp:
            return resp["usage"].get("prompt_tokens", 0)
        return resp.get("prompt_tokens", 0)
    except Exception:
        return 0


def run_laya_mem(client, tools, all_facts):
    """Persist every fact (session1), recall per question (session2 fresh)."""
    t0 = time.time()
    calls = 0
    # session1: persist
    for fact in all_facts:
        msgs = [{"role": "system", "content": SYS_TOOL},
                {"role": "user", "content": f"For the record: {fact}."}]
        done = False
        while not done:
            ch = llm(msgs, tools)
            calls += 1
            msg = ch.get("message", {})
            tc = msg.get("tool_calls")
            if tc:
                for call in tc:
                    fn = call["function"]["name"]
                    try:
                        args = json.loads(call["function"]["arguments"] or "{}")
                    except json.JSONDecodeError:
                        args = {}
                    invoke_tool(client, fn, args)
                    msgs.append({"role": "assistant", "tool_calls": tc})
                    msgs.append({"role": "tool", "tool_call_id": call["id"],
                                 "content": json.dumps(invoke_tool(client, fn, args), ensure_ascii=False)[:4000]})
                continue
            msgs.append({"role": "assistant", "content": msg.get("content") or ""})
            done = True
    # session2: fresh context, recall per question
    answers = {}
    tok_sum = 0
    msgs = [{"role": "system", "content": SYS_TOOL}]
    for q in QUESTIONS:
        msgs.append({"role": "user", "content": q})
        done = False
        while not done:
            ch, usage = llm_full(msgs, tools)
            calls += 1
            tok_sum += count_prompt_tokens(usage)
            msg = ch.get("message", {})
            tc = msg.get("tool_calls")
            if tc:
                for call in tc:
                    fn = call["function"]["name"]
                    try:
                        args = json.loads(call["function"]["arguments"] or "{}")
                    except json.JSONDecodeError:
                        args = {}
                    r = invoke_tool(client, fn, args)
                    msgs.append({"role": "assistant", "tool_calls": tc})
                    msgs.append({"role": "tool", "tool_call_id": call["id"],
                                 "content": json.dumps(r, ensure_ascii=False)[:4000]})
                continue
            content = msg.get("content") or ""
            if content.strip():
                answers[q] = content
            msgs.append({"role": "assistant", "content": content})
            done = True
    return {"accuracy": score(answers), "answers": answers,
            "calls": calls, "wall": time.time() - t0,
            "prompt_tokens_per_q": tok_sum / len(QUESTIONS)}


def run_full_ctx(all_facts):
    """Traditional: stuff ALL session-1 turns into session-2 prompt."""
    t0 = time.time()
    calls = 0
    answers = {}
    tok_sum = 0
    hist = []
    for fact in all_facts:
        hist.append({"role": "user", "content": f"For the record: {fact}."})
        hist.append({"role": "assistant", "content": "Noted."})
    for q in QUESTIONS:
        msgs = [{"role": "system", "content": SYS_FULL}] + hist + [
            {"role": "user", "content": q}]
        ch, usage = llm_full(msgs, tools=None)
        calls += 1
        tok_sum += count_prompt_tokens(usage)
        content = ch.get("message", {}).get("content") or ""
        if content.strip():
            answers[q] = content
    return {"accuracy": score(answers), "answers": answers,
            "calls": calls, "wall": time.time() - t0,
            "prompt_tokens_per_q": tok_sum / len(QUESTIONS)}


def run_laya_mem_script(client, all_facts):
    """Production-fast path: deterministic persist (direct MCP call, no LLM
    round-trip per fact), then per-question recall (MCP) + one LLM answer.
    Same #LLM calls as full_ctx (1 answer per Q) but context stays O(1)."""
    t0 = time.time()
    calls = 0
    tok_sum = 0
    for fact in all_facts:
        invoke_tool(client, "laya_mem_persist",
                    {"content": fact, "entities": [],
                     "type_scores": {"episodic": 0.5, "semantic": 0.3,
                                     "procedural": 0.1, "preference": 0.1}})
    answers = {}
    msgs = [{"role": "system", "content": SYS_TOOL}]
    for q in QUESTIONS:
        r = invoke_tool(client, "laya_mem_recall",
                        {"query": q, "limit": 5, "include_relations": False})
        blob = json.dumps(r.get("memories") or [], ensure_ascii=False)[:4000]
        m2 = msgs + [{"role": "user", "content": f"Recall result:\n{blob}\n\nQuestion: {q}"}]
        ch, usage = llm_full(m2, tools=None)
        calls += 1
        tok_sum += count_prompt_tokens(usage)
        content = ch.get("message", {}).get("content") or ""
        if content.strip():
            answers[q] = content
    return {"accuracy": score(answers), "answers": answers,
            "calls": calls, "wall": time.time() - t0,
            "prompt_tokens_per_q": tok_sum / len(QUESTIONS)}


def run_no_memory():
    t0 = time.time()
    answers = {}
    tok_sum = 0
    msgs = [{"role": "system", "content": SYS_FULL}]
    for q in QUESTIONS:
        m2 = msgs + [{"role": "user", "content": q}]
        ch, usage = llm_full(m2, tools=None)
        tok_sum += count_prompt_tokens(usage)
        content = ch.get("message", {}).get("content") or ""
        if content.strip():
            answers[q] = content
    return {"accuracy": score(answers), "answers": answers,
            "calls": len(QUESTIONS), "wall": time.time() - t0,
            "prompt_tokens_per_q": tok_sum / len(QUESTIONS)}


def score(answers):
    hits = 0
    for q, exp in zip(QUESTIONS, EXPECT):
        if exp in (answers.get(q) or "").lower():
            hits += 1
    return hits / len(QUESTIONS)


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--distractors", type=int, default=0, help="extra facts in session-1")
    ap.add_argument("--methods", default="laya_mem,full_ctx,no_memory")
    args = ap.parse_args()
    methods = [m.strip() for m in args.methods.split(",") if m.strip()]
    dist = distractor_facts(args.distractors)
    all_facts = [f for f, _ in FACTS] + dist
    print(f"session-1 facts: {len(all_facts)} (20 real + {len(dist)} distractor)")

    db = tempfile.mktemp(suffix=".sqlite")
    try:
        os.remove(db)
    except FileNotFoundError:
        pass
    client = McpClient(BIN, SPEC_DIR, db)
    tools = mcp_tools(client) if "laya_mem" in methods else None
    results = {}
    try:
        if "laya_mem" in methods:
            print("running laya_mem (tool loop) ...", flush=True)
            results["laya_mem"] = run_laya_mem(client, tools, all_facts)
        if "laya_mem_script" in methods:
            print("running laya_mem_script ...", flush=True)
            results["laya_mem_script"] = run_laya_mem_script(client, all_facts)
        if "full_ctx" in methods:
            print("running full_ctx ...", flush=True)
            results["full_ctx"] = run_full_ctx(all_facts)
        if "no_memory" in methods:
            print("running no_memory ...", flush=True)
            results["no_memory"] = run_no_memory()
    finally:
        client.close()

    print(f"\n# Three-method comparison (N={len(all_facts)} session-1 facts, {len(QUESTIONS)} questions)")
    print(f"{'method':10} | {'accuracy':>9} | {'wall':>7} | {'calls':>6} | {'tok/q':>8}")
    print("-" * 56)
    for m, r in results.items():
        print(f"{m:10} | {r['accuracy']:>8.0%} | {r['wall']:>6.1f}s | {r['calls']:>6} | {r['prompt_tokens_per_q']:>7.0f}")

    out = HERE / "reports" / "codex_compare.json"
    out.write_text(json.dumps({"args": vars(args), "n_facts": len(all_facts),
                               "results": results}, indent=2, ensure_ascii=False))
    print(f"wrote {out}")


if __name__ == "__main__":
    main()
