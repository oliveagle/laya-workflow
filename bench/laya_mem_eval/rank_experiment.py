#!/usr/bin/env python3
"""Round-4 ranking experiment: compare FTS5 ranking strategies directly on
the ingested LongMemEval SQLite DBs (no Rust rebuild needed).

For each sampled row: ingest all turns into laya-mem, then for each ranking
SQL variant evaluate:
  * exact@5  — gold answer substring present in the top-5 memories
  * judge@5  — LLM judge says top-5 memories suffice to answer

Ranking variants (all FTS5 MATCH on the OR-expanded question tokens):
  bm25          current: ORDER BY bm25(memories_fts), id DESC
  recency       recency-first: ORDER BY id DESC (freshness)
  hybrid0.5     bm25 + 0.5 * recency term
  hybrid1.0     bm25 + 1.0 * recency term

Usage:
    PYTHONPATH= bench/laya_mem_eval/rank_experiment.py --sample 10 \\
        --judge-url http://127.0.0.1:11032/v1
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

VARIANTS = ["bm25", "recency", "hybrid0.25", "hybrid0.5", "hybrid1.0", "hybrid2.0"]


def fts_query_tokens(q: str) -> list[str]:
    toks = [t for t in q.split() if t]
    return [f"{t}*" for t in toks]


def variant_sql(variant: str, tokens: list[str], limit: int) -> str:
    match = " OR ".join(tokens)
    base = (
        "SELECT m.id, m.content, m.ts, m.entities, m.type_scores "
        "FROM memories_fts f JOIN memories m ON m.id = f.rowid "
        f"WHERE memories_fts MATCH '{match}' "
    )
    if variant == "bm25":
        return base + f"ORDER BY bm25(memories_fts), m.id DESC LIMIT {limit}"
    if variant == "recency":
        return base + f"ORDER BY m.id DESC LIMIT {limit}"
    w = float(variant.replace("hybrid", ""))
    # hybrid: normalize recency to ~[0,1] via id/max(id); scale by w.
    # bm25 returns negative-ish numbers (more negative = better), so we
    # SUBTRACT the recency term (higher id = more recent = smaller term).
    return base + (
        f"ORDER BY bm25(memories_fts) + {w} * (1.0 - m.id / (SELECT MAX(id) FROM memories)) "
        f", m.id DESC LIMIT {limit}"
    )


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--sample", type=int, default=10)
    ap.add_argument("--topk", type=int, default=5)
    ap.add_argument("--max-turns", type=int, default=500)
    ap.add_argument("--judge-url", default="")
    ap.add_argument("--strategy", default="full")
    ap.add_argument("--variants", default=",".join(VARIANTS))
    args = ap.parse_args()

    rows = load_sample(args.sample)
    variants = [v.strip() for v in args.variants.split(",") if v.strip()]

    # tally per variant
    tally = {v: {"n": 0, "exact": 0, "judge": 0} for v in variants}
    per_row = []

    db = args_db = tempfile.mktemp(suffix=".sqlite")
    try:
        os.remove(db)
    except FileNotFoundError:
        pass
    client = McpClient(BIN, SPEC_DIR, db)

    for row in rows:
        if not row.get("answer_session_ids") or not row.get("haystack_session_ids"):
            continue
        # ingest all turns
        for sess in row.get("haystack_sessions", []):
            for turn in sess:
                t = turn_text(turn)
                if t:
                    client.tool("laya_mem_persist", {"content": t, "type_scores": {}})
        # build the query tokens (reuse the Rust tokenizer semantics: split non-alnum)
        qterm = build_query(row["question"], args.strategy)
        import re
        tokens = [f"{t}*" for t in re.split(r"[^a-zA-Z0-9]+", qterm) if t]
        if not tokens:
            continue
        match = " OR ".join(tokens)
        ans = row["answer"].strip().lower()

        con = sqlite3.connect(db)
        row_res = {}
        for v in variants:
            sql = variant_sql(v, tokens, args.topk)
            try:
                cursor = con.execute(sql)
                cols = [d[0] for d in cursor.description]
                mems = [dict(zip(cols, r)) for r in cursor.fetchall()]
            except Exception as e:
                print(f"  variant {v} SQL error: {e}", file=sys.stderr)
                continue
            blob = json.dumps(mems, ensure_ascii=False).lower()
            exact = ans in blob
            judge_hit = None
            if args.judge_url:
                judge_hit = judge(args.judge_url, row["question"], row["answer"], mems)
            tally[v]["n"] += 1
            tally[v]["exact"] += int(exact)
            tally[v]["judge"] += int(bool(judge_hit))
            row_res[v] = {"exact": exact, "judge": judge_hit,
                          "q": row["question"][:50], "ans": row["answer"][:40]}
        con.close()
        per_row.append(row_res)

    print(f"\n# Ranking experiment: {args.sample} rows, top{args.topk}, judge={'on' if args.judge_url else 'off'}\n")
    print(f"{'variant':12} | {'exact@5':>12} | {'judge@5':>12}")
    print("-" * 42)
    for v in variants:
        t = tally[v]
        if t["n"] == 0:
            continue
        e = f"{t['exact']}/{t['n']} = {t['exact']/t['n']:.1%}"
        j = f"{t['judge']}/{t['n']} = {t['judge']/t['n']:.1%}" if args.judge_url else "—"
        print(f"{v:12} | {e:>12} | {j:>12}")

    # save raw
    out = HERE / "reports" / "rank_experiment.json"
    out.write_text(json.dumps({"variants": variants, "tally": tally,
                               "per_row": per_row, "args": vars(args)}, indent=2))
    print(f"\nwrote {out}")
    client.close()


if __name__ == "__main__":
    main()
