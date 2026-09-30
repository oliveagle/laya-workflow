#!/usr/bin/env python3
"""Round-5 LLM-rerank experiment: bm25 top-20 -> LLM picks top-K.

A RAG reranking pattern: lexical bm25 gives a wide candidate set (top-20),
the LLM judge picks the best K of them. This bridges paraphrase /
semantic gaps that pure bm25 misses, without requiring any new
embedding dependency.

Compares (on LongMemEval_s n=10 with judge):
  bm25@5     current production default
  rerank@5   bm25@20 -> LLM pick best 5
  bm25@20    wider candidate (no rerank) — upper bound on bm25 alone
"""
from __future__ import annotations

import argparse
import json
import os
import random
import sqlite3
import sys
import tempfile
import urllib.request
from pathlib import Path

HERE = Path(__file__).parent
sys.path.insert(0, str(HERE))
from longmemeval import build_query, judge, load_sample, turn_text
from run import McpClient, _tool_payload

REPO = HERE.parent.parent
BIN = REPO / "target" / "debug" / "laya-workflow"
SPEC_DIR = REPO / "dsl" / "laya_mem"


def fts_tokens(q: str) -> list[str]:
    import re
    return [f"{t}*" for t in re.split(r"[^a-zA-Z0-9]+", q) if t]


def bm25_top(qterm: str, limit: int, db: str) -> list[dict]:
    toks = fts_tokens(qterm)
    if not toks:
        return []
    match = " OR ".join(toks)
    sql = (
        "SELECT m.id, m.content, m.ts, m.entities, m.type_scores "
        "FROM memories_fts f JOIN memories m ON m.id = f.rowid "
        f"WHERE memories_fts MATCH '{match}' "
        f"ORDER BY bm25(memories_fts), m.id DESC LIMIT {limit}"
    )
    con = sqlite3.connect(db)
    try:
        cur = con.execute(sql)
        cols = [d[0] for d in cur.description]
        return [dict(zip(cols, r)) for r in cur.fetchall()]
    finally:
        con.close()


def rerank(question: str, candidates: list[dict], k: int, judge_url: str) -> list[dict]:
    """LLM rerank: pick top-K ids for `question` from candidates."""
    if len(candidates) <= k:
        return candidates
    options = "\\n".join(f"[{i+1}] {(c['content'] or '')[:200]}" for i, c in enumerate(candidates))
    prompt = (
        "Select the most relevant memories for answering the question. "
        "Return the IDs (one per line) of the top options. Include only the IDs.\\n\\n"
        f"Question: {question}\\n\\nOptions:\\n{options}"
    )
    req = urllib.request.Request(
        f"{judge_url.rstrip('/')}/chat/completions",
        data=json.dumps({
            "model": "qwen3.8-flash-next",
            "messages": [{"role": "user", "content": prompt}],
            "max_tokens": 200, "temperature": 0,
        }).encode(),
        headers={"Content-Type": "application/json"},
    )
    for attempt in range(3):
        try:
            with urllib.request.urlopen(req, timeout=60) as r:
                body = json.load(r)
            content = body["choices"][0]["message"].get("content") or ""
            picked = []
            for tok in content.replace(",", " ").split():
                tok = tok.strip("[]").rstrip(".)")
                if tok.isdigit():
                    idx = int(tok) - 1
                    if 0 <= idx < len(candidates):
                        picked.append(candidates[idx])
                if len(picked) >= k: break
            if picked:
                return picked[:k]
            return candidates[:k]
        except Exception as e:
            import time as _t
            _t.sleep(2 + attempt)
    return candidates[:k]


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--sample", type=int, default=10)
    ap.add_argument("--max-turns", type=int, default=500)
    ap.add_argument("--strategy", default="full")
    ap.add_argument("--judge-url", default="http://127.0.0.1:11032/v1")
    ap.add_argument("--candidates", type=int, default=20)
    ap.add_argument("--topk", type=int, default=5)
    args = ap.parse_args()

    rows = load_sample(args.sample)
    tally = {"bm25@5": {"exact": 0, "judge": 0, "n": 0},
             "rerank@5": {"exact": 0, "judge": 0, "n": 0},
             "bm25@20": {"exact": 0, "judge": 0, "n": 0}}

    db = tempfile.mktemp(suffix=".sqlite")
    try: os.remove(db)
    except FileNotFoundError: pass
    client = McpClient(BIN, SPEC_DIR, db)

    per_row = []
    for row in rows:
        if not row.get("answer_session_ids") or not row.get("haystack_session_ids"):
            continue
        for sess in row.get("haystack_sessions", []):
            for turn in sess:
                t = turn_text(turn)
                if t:
                    client.tool("laya_mem_persist", {"content": t, "type_scores": {}})
        qterm = build_query(row["question"], args.strategy)
        ans = row["answer"].strip().lower()

        bm5 = bm25_top(qterm, args.topk, db)
        bm20 = bm25_top(qterm, args.candidates, db)
        rr5 = rerank(row["question"], bm20, args.topk, args.judge_url)

        for label, mems in [("bm25@5", bm5), ("rerank@5", rr5), ("bm25@20", bm20)]:
            blob = json.dumps(mems, ensure_ascii=False).lower()
            exact = ans in blob
            j_hit = judge(args.judge_url, row["question"], row["answer"], mems) if args.judge_url else None
            tally[label]["n"] += 1
            tally[label]["exact"] += int(exact)
            tally[label]["judge"] += int(bool(j_hit))

        per_row.append({"q": row["question"][:60], "ans": row["answer"][:40],
                        "bm5": [m["content"][:60] for m in bm5[:2]],
                        "rr5": [m["content"][:60] for m in rr5[:2]]})

    client.close()

    print(f"\n# Rerank experiment: n={args.sample}, topk={args.topk}, candidates={args.candidates}\n")
    print(f"{'method':12} | {'exact':>12} | {'judge':>12}")
    print("-" * 44)
    for label in tally:
        t = tally[label]
        if t["n"] == 0: continue
        e = f"{t['exact']}/{t['n']} = {t['exact']/t['n']:.1%}"
        j = f"{t['judge']}/{t['n']} = {t['judge']/t['n']:.1%}" if args.judge_url else "—"
        print(f"{label:12} | {e:>12} | {j:>12}")

    out = HERE / "reports" / "rank_rerank.json"
    out.write_text(json.dumps({"tally": tally, "per_row": per_row,
                               "args": vars(args)}, indent=2))
    print(f"\nwrote {out}")


if __name__ == "__main__":
    main()
