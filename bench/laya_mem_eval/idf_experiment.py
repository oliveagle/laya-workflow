#!/usr/bin/env python3
"""Round-5 IDF experiment: does dropping common question terms help?

For each question, split tokens into two classes by global IDF over the
ingested corpus: rare (top 3 by idf) vs all. Compare:
  bm25@5 all    current default (all non-stop tokens as OR prefix)
  bm25@5 rare   only top-3-idf tokens as OR prefix
  bm25@5 tfidf  all tokens, weighted by bm25 (same as all — baseline)
"""
from __future__ import annotations

import argparse
import json
import math
import os
import re
import sqlite3
import sys
import tempfile
from pathlib import Path

HERE = Path(__file__).parent
sys.path.insert(0, str(HERE))
from longmemeval import judge, load_sample, turn_text, STOPWORDS
from run import McpClient, _tool_payload

REPO = HERE.parent.parent
BIN = REPO / "target" / "debug" / "laya-workflow"
SPEC_DIR = REPO / "dsl" / "laya_mem"


def tokenize(q: str) -> list[str]:
    return [t for t in re.split(r"[^a-zA-Z0-9]+", q.lower()) if t and t not in STOPWORDS and len(t) > 1]


def bm25_query(tokens: list[str], limit: int, db: str) -> list[dict]:
    if not tokens:
        return []
    match = " OR ".join(f"{t}*" for t in tokens)
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


def idf_scores(db: str) -> dict[str, float]:
    """Global IDF over ingested memories. idf = log(N / (1 + df)) with smoothing."""
    con = sqlite3.connect(db)
    try:
        n = con.execute("SELECT COUNT(*) FROM memories").fetchone()[0]
        df = {}
        for (content,) in con.execute("SELECT content FROM memories"):
            for t in set(tokenize(content or "")):
                df[t] = df.get(t, 0) + 1
        return {t: math.log(n / (1 + c)) for t, c in df.items()}
    finally:
        con.close()


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--sample", type=int, default=30)
    ap.add_argument("--max-turns", type=int, default=500)
    ap.add_argument("--topk", type=int, default=5)
    ap.add_argument("--rare-n", type=int, default=3)
    ap.add_argument("--judge-url", default="http://127.0.0.1:11032/v1")
    args = ap.parse_args()

    rows = load_sample(args.sample)
    tally = {"all@5": {"exact": 0, "judge": 0, "n": 0},
             f"rare{args.rare_n}@5": {"exact": 0, "judge": 0, "n": 0}}

    db = tempfile.mktemp(suffix=".sqlite")
    try: os.remove(db)
    except FileNotFoundError: pass
    client = McpClient(BIN, SPEC_DIR, db)

    # pre-ingest all rows so idf_scores sees the full corpus
    per_row_data = []
    for row in rows:
        if not row.get("answer_session_ids") or not row.get("haystack_session_ids"):
            continue
        for sess in row.get("haystack_sessions", []):
            for turn in sess:
                t = turn_text(turn)
                if t:
                    client.tool("laya_mem_persist", {"content": t, "type_scores": {}})
        per_row_data.append(row)
    idf = idf_scores(db)

    for row in per_row_data:
        toks = tokenize(row["question"])
        if not toks:
            continue
        # rare = top-3 by idf (highest idf = most distinctive)
        ranked = sorted(set(toks), key=lambda t: idf.get(t, 0.0), reverse=True)
        rare_toks = ranked[:args.rare_n]

        for label, toks_use in [("all@5", toks), (f"rare{args.rare_n}@5", rare_toks)]:
            mems = bm25_query(toks_use, args.topk, db)
            blob = json.dumps(mems, ensure_ascii=False).lower()
            exact = row["answer"].strip().lower() in blob
            j = judge(args.judge_url, row["question"], row["answer"], mems) if args.judge_url else None
            tally[label]["n"] += 1
            tally[label]["exact"] += int(exact)
            tally[label]["judge"] += int(bool(j))

    client.close()
    print(f"\n# IDF experiment: n={args.sample}, topk={args.topk}\n")
    print(f"{'method':12} | {'exact':>12} | {'judge':>12}")
    print("-" * 44)
    for label in tally:
        t = tally[label]
        if t["n"] == 0: continue
        e = f"{t['exact']}/{t['n']} = {t['exact']/t['n']:.1%}"
        j = f"{t['judge']}/{t['n']} = {t['judge']/t['n']:.1%}" if args.judge_url else "—"
        print(f"{label:12} | {e:>12} | {j:>12}")

    out = HERE / "reports" / "idf_experiment.json"
    out.write_text(json.dumps({"tally": tally, "args": vars(args)}, indent=2))
    print(f"\nwrote {out}")


if __name__ == "__main__":
    main()
