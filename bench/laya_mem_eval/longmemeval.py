#!/usr/bin/env python3
"""Round-4/5: sample LongMemEval_s and evaluate laya-mem recall@k on real
user/assistant conversations, with two scoring modes:

  * exact   — ground-truth `answer` substring appears in the recalled
              memories (strict; LongMemEval answers are judge paraphrases,
              so this under-counts real retrieval quality).
  * judge   — LLM-as-judge (the LongMemEval / mem0 standard): give the
              judge the question + recalled memories only, ask YES/NO
              whether the memories suffice to answer. --judge-url off
              disables it (offline default).

Pipeline (mem0-style, 3 stages):
  Ingest   — persist every user+assistant turn (up to --max-turns) of ALL
             haystack sessions; no answer-session peeking.
  Search   — laya_mem_recall with a query built from the question, under
             several --strategies (comma list):
               top4   stop-word stripped, first 4 terms (default)
               full   all non-stop question terms
               raw    verbatim question
  Evaluate — exact substring + optional LLM judge, per strategy.

Usage:
    PYTHONPATH= bench/laya_mem_eval/longmemeval.py --sample 30 --topk 5
    PYTHONPATH= bench/laya_mem_eval/longmemeval.py --sample 10 \
        --strategies top4,full --judge-url http://127.0.0.1:11032/v1
"""
from __future__ import annotations

import argparse
import json
import os
import random
import sys
import tempfile
import urllib.request
from pathlib import Path

HERE = Path(__file__).parent
sys.path.insert(0, str(HERE))
from run import McpClient

REPO = HERE.parent.parent
BIN = REPO / "target" / "debug" / "laya-workflow"
SPEC_DIR = REPO / "dsl" / "laya_mem"
DEFAULT_DATA = "/tmp/longmemeval_s.json"


def load_sample(n: int, seed: int = 42) -> list[dict]:
    data = json.load(open(DEFAULT_DATA))
    rng = random.Random(seed)
    idx = list(range(len(data)))
    rng.shuffle(idx)
    return [data[i] for i in idx[:n]]


def _payload(resp):
    """Unwrap MCP tools/call response to the payload dict."""
    if "content" in resp and isinstance(resp["content"], list):
        return json.loads(resp["content"][0]["text"])
    return resp


def turn_text(turn: dict) -> str | None:
    role = turn.get("role", "")
    content = (turn.get("content") or "").strip()
    if not content:
        return None
    # Ingest both roles — evidence often appears in assistant turns
    # ("Congratulations on your recent collectible finds, including the rare
    # blue Snaggletooth action figure!"), and dropping assistants causes
    # recall misses that look like search failures but are ingestion gaps.
    if len(content) < 20:
        return None
    return content


STOPWORDS = {"what", "when", "where", "who", "which", "did", "does", "do",
             "the", "a", "an", "is", "are", "was", "were", "of", "in",
             "to", "with", "for", "on", "my", "i"}


def build_query(question: str, strategy: str) -> str:
    """Question -> FTS query per retrieval strategy."""
    terms = [w for w in question.lower().split() if w not in STOPWORDS]
    if strategy == "top4":
        return " ".join(terms[:4]) or question
    if strategy == "full":
        return " ".join(terms) or question
    return question


def judge(judge_url: str, question: str, answer: str, memories: list) -> bool:
    """LLM-as-judge: can the recalled memories answer the question?

    Fixed prompt (no prompt-variant sweep within a run), temperature 0 —
    per the LLM-as-judge best practice of a frozen prompt for comparability.
    Returns True when the judge says the memories suffice (the judge never
    sees the gold answer — only question + memories).
    """
    blob = "\n---\n".join(
        str(m.get("content", m)) if isinstance(m, dict) else str(m)
        for m in memories
    )
    prompt = (
        "You are scoring whether retrieved memory snippets are sufficient to "
        "answer a question. Answer YES if the snippets contain enough "
        "information to answer it correctly, NO if they do not.\n\n"
        f"Question: {question}\n\nRetrieved memories:\n{blob}\n\n"
        "Answer with exactly YES or NO."
    )
    req = urllib.request.Request(
        f"{judge_url.rstrip('/')}/chat/completions",
        data=json.dumps({
            "model": os.environ.get("JUDGE_MODEL", "qwen3.8-flash-next"),
            "messages": [{"role": "user", "content": prompt}],
            "max_tokens": 150,
            "temperature": 0,
        }).encode(),
        headers={"Content-Type": "application/json"},
    )
    last_err = None
    for attempt in range(3):
        try:
            with urllib.request.urlopen(req, timeout=60) as r:
                body = json.load(r)
            content = body["choices"][0]["message"].get("content") or ""
            return content.strip().upper().startswith("YES")
        except Exception as e:
            last_err = e
            import time as _t
            _t.sleep(2 + attempt)
    print(f"  judge failed after 3 retries: {last_err}", file=sys.stderr)
    return False


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--sample", type=int, default=30, help="how many LongMemEval rows to test")
    ap.add_argument("--topk", type=int, default=5, help="recall limit")
    ap.add_argument("--max-turns", type=int, default=500,
                    help="max turns persisted per row (all sessions, both roles)")
    ap.add_argument("--db", default=None, help="sqlite path")
    ap.add_argument("--strategies", default="top4",
                    help="comma list of query strategies: top4,full,raw")
    ap.add_argument("--judge-url", default="",
                    help="OpenAI-compatible base URL for LLM-as-judge (empty = exact scoring only)")
    ap.add_argument("--judge-runs", type=int, default=1,
                    help="judge repetitions for stability (report mean)")
    args = ap.parse_args()

    data_path = Path(DEFAULT_DATA)
    if not data_path.is_file():
        print(f"LongMemEval data missing at {data_path}. Download:", file=sys.stderr)
        print(f"  wget {data_path.parent}/{data_path.name}", file=sys.stderr)
        sys.exit(1)

    rows = load_sample(args.sample)
    db = args.db or tempfile.mktemp(suffix=".sqlite")
    client = McpClient(BIN, SPEC_DIR, db)
    results = []
    try:
        for i, row in enumerate(rows):
            # Ingest ALL sessions (up to max-turns) — this is what mem0/BEAM
            # do, and the only realistic production scenario. The old harness
            # picked only answer_session_ids + neighbours (≤5 sessions), but
            # LongMemEval's answer string often appears in a distractor
            # session or as an assistant response, so recall was trivially
            # 0 even with a perfect search.
            ans_ids = set(row.get("answer_session_ids", []))
            all_ids = row.get("haystack_session_ids", [])
            if not ans_ids or not all_ids:
                continue

            turns = []
            for sess in row.get("haystack_sessions", []):
                for turn in sess:
                    t = turn_text(turn)
                    if t: turns.append(t)
            turns = turns[:args.max_turns]
            if not turns:
                continue

            # persist each turn (with admission — skip if BLOCKed)
            stored = 0
            for turn in turns:
                resp = client.tool("laya_mem_persist", {"content": turn, "type_scores": {"episodic": 0.25, "semantic": 0.45, "procedural": 0.15, "preference": 0.15}})
                payload = _payload(resp)
                mid = payload.get("memory_id", -1)
                if isinstance(mid, int) and mid >= 0:
                    stored += 1

            # Search: query with keywords from the question (stop-word stripped).
            stop = {"what", "when", "where", "who", "which", "did", "does", "do",
                    "the", "a", "an", "is", "are", "was", "were", "of", "in",
                    "to", "with", "for", "on", "my", "i"}
            strategies = [s.strip() for s in args.strategies.split(",") if s.strip()]
            per_strategy = []
            for strat in strategies:
                qterm = build_query(row["question"], strat)
                r = client.tool("laya_mem_recall", {"limit": args.topk, "include_relations": False, "query": qterm})
                p = _payload(r)
                if "memories" not in p:
                    p = {"memories": []}
                memories = p.get("memories") or []
                text_blob = json.dumps(memories, ensure_ascii=False).lower()
                exact_hit = row["answer"].strip().lower() in text_blob
                judge_hit = None
                judge_score = None
                if args.judge_url:
                    yes_votes = 0
                    for _ in range(args.judge_runs):
                        if judge(args.judge_url, row["question"], row["answer"], memories):
                            yes_votes += 1
                    judge_hit = yes_votes > 0
                    judge_score = yes_votes / args.judge_runs
                per_strategy.append({
                    "strategy": strat,
                    "qterm": qterm,
                    "exact_hit": exact_hit,
                    "judge_hit": judge_hit,
                    "judge_score": judge_score,
                    "num_memories": len(memories),
                })

            results.append({
                "question_id": row["question_id"],
                "qtype": row.get("question_type"),
                "question": row["question"][:60],
                "answer": row["answer"],
                "ingested_turns": len(turns),
                "stored_turns": stored,
                "per_strategy": per_strategy,
            })
    finally:
        client.close()

    # report
    n = len(results)
    total_stored = sum(r["stored_turns"] for r in results)
    total_ingested = sum(r["ingested_turns"] for r in results)
    print(f"\n# LongMemEval sample: {n} questions, {args.topk}-recall, {total_stored}/{total_ingested} turns persisted\n")
    # per-strategy summary
    summary = {"n": n, "topk": args.topk, "strategies": {}, "results": results}
    have_judge = bool(args.judge_url)
    if have_judge:
        print("Strategy        | exact@5        | judge@5         | qtype breakdown")
    else:
        print("Strategy        | exact@5        | qtype breakdown")
    strat_names = sorted({s["strategy"] for r in results for s in r["per_strategy"]})
    for strat in strat_names:
        rows_strat = [r for r in results if any(s["strategy"] == strat for s in r["per_strategy"])]
        exact = [next(s for s in r["per_strategy"] if s["strategy"] == strat)["exact_hit"] for r in rows_strat]
        exact_h = sum(1 for x in exact if x)
        line = f"{strat:15} | {exact_h}/{len(rows_strat)} = {exact_h/len(rows_strat):.1%}"
        if have_judge:
            scores = [next(s for s in r["per_strategy"] if s["strategy"] == strat)["judge_score"] or 0 for r in rows_strat]
            judge_h = sum(1 for r in rows_strat if next(s for s in r["per_strategy"] if s["strategy"] == strat)["judge_hit"])
            line += f" | {judge_h}/{len(rows_strat)} = {judge_h/len(rows_strat):.1%}  (mean={sum(scores)/len(scores):.2f})"
        print(line)
        by_type = {}
        for r in rows_strat:
            by_type.setdefault(r["qtype"], []).append(r)
        for qt, rs in sorted(by_type.items()):
            h = sum(1 for r in rs if next(s for s in r["per_strategy"] if s["strategy"] == strat)["exact_hit"])
            line = f"  {qt:24}  {h}/{len(rs)} = {h/len(rs):.1%}"
            if have_judge:
                jh = sum(1 for r in rs if next(s for s in r["per_strategy"] if s["strategy"] == strat)["judge_hit"])
                line += f"  (judge {jh}/{len(rs)} = {jh/len(rs):.1%})"
            print(line)
        summary["strategies"][strat] = {
            "exact_hits": exact_h, "judge_hits": judge_h if have_judge else None,
            "exact_at_k": exact_h / len(rows_strat) if rows_strat else 0,
        }
    out = HERE / "reports" / "longmemeval_sample.json"
    out.write_text(json.dumps(summary, indent=2))
    print(f"\nwrote {out}")


if __name__ == "__main__":
    main()
