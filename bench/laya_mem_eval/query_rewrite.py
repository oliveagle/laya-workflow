#!/usr/bin/env python3
"""Round-6 query rewrite experiment.

Pass the user's question through an LLM 'rewrite to search keywords' step
before handing it to the bm25+OR search. This is the standard mem0/Letta
"query expansion" technique. Hypothesis: the rewritten query matches the
specific vocabulary actually present in the corpus (e.g. 'model kit',
'scale', 'Revell F-15 Eagle'), while the raw question contains generic
question-words (how, what, did, many) that only add noise.

Compare on LongMemEval_s n=30:
  baseline     bm25+OR with full non-stop question tokens (current)
  rewrites     bm25+OR with LLM-extracted entity/keyword tokens
  combined     bm25+OR with (question tokens) OR (rewrites)
"""
from __future__ import annotations

import argparse
import json
import os
import re
import sqlite3
import sys
import tempfile
import urllib.request
from pathlib import Path

HERE = Path(__file__).parent
sys.path.insert(0, str(HERE))
from longmemeval import STOPWORDS, judge, load_sample, turn_text
from run import McpClient, _tool_payload

REPO = HERE.parent.parent
BIN = REPO / "target" / "debug" / "laya-workflow"
SPEC_DIR = REPO / "dsl" / "laya_mem"


def tokenize_question(q: str) -> list[str]:
    return [t for t in re.split(r"[^a-zA-Z0-9]+", q.lower()) if t and t not in STOPWORDS and len(t) > 1]


def rewrite(question: str, url: str) -> list[str]:
    """Ask the LLM to extract 3-8 specific search keywords from a question."""
    prompt = (
        "Given a user's question, list 3-8 specific search keywords or "
        "named entities that would appear in the conversation history "
        "answering it. Lower-case, one per line, no numbering, no extra "
        "explanation.\\n\\nQuestion: " + question
    )
    req = urllib.request.Request(
        f"{url.rstrip('/')}/chat/completions",
        data=json.dumps({
            "model": "qwen3.8-flash-next",
            "messages": [{"role": "user", "content": prompt}],
            "max_tokens": 600, "temperature": 0,
        }).encode(),
        headers={"Content-Type": "application/json"},
    )
    for attempt in range(3):
        try:
            with urllib.request.urlopen(req, timeout=60) as r:
                body = json.load(r)
            content = body["choices"][0]["message"].get("content") or ""
            kws = []
            for line in content.replace("\n", " ").split():
                w = line.strip(".,!?:;\"'()[]{}").lower()
                if w and w not in STOPWORDS and len(w) > 1 and not w.isspace():
                    kws.append(w)
                if len(kws) >= 8: break
            return kws[:8]
        except Exception as e:
            import time as _t
            _t.sleep(2 + attempt)
    return []


def bm25_query(tokens: list[str], limit: int, db: str) -> list[dict]:
    if not tokens:
        return []
    # double-quote each token so FTS5 never mis-parses it as a column
    # reference ("no such column: aware" happens on unquoted bare words)
    match = " OR ".join(f'\"{t}\"*' for t in tokens)
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


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--sample", type=int, default=30)
    ap.add_argument("--max-turns", type=int, default=500)
    ap.add_argument("--topk", type=int, default=5)
    ap.add_argument("--judge-url", default="http://127.0.0.1:11032/v1")
    args = ap.parse_args()

    rows = load_sample(args.sample)
    tally = {
        "baseline@5": {"exact": 0, "judge": 0, "n": 0},
        "rewrites@5": {"exact": 0, "judge": 0, "n": 0},
        "combined@5": {"exact": 0, "judge": 0, "n": 0},
    }

    db = tempfile.mktemp(suffix=".sqlite")
    try: os.remove(db)
    except FileNotFoundError: pass
    client = McpClient(BIN, SPEC_DIR, db)

    rewrites_log = []
    per_row_out = []

    for row in rows:
        if not row.get("answer_session_ids") or not row.get("haystack_session_ids"):
            continue
        for sess in row.get("haystack_sessions", []):
            for turn in sess:
                t = turn_text(turn)
                if t:
                    client.tool("laya_mem_persist", {"content": t, "type_scores": {}})
        base_toks = tokenize_question(row["question"])
        rw_toks = rewrite(row["question"], args.judge_url)
        combined = list(dict.fromkeys(base_toks + rw_toks))  # preserve order, dedupe
        rewrites_log.append({"q": row["question"][:80], "rw": rw_toks})
        ans = row["answer"].strip().lower()

        for label, toks in [("baseline@5", base_toks), ("rewrites@5", rw_toks), ("combined@5", combined)]:
            mems = bm25_query(toks, args.topk, db)
            blob = json.dumps(mems, ensure_ascii=False).lower()
            exact = ans in blob
            j = judge(args.judge_url, row["question"], row["answer"], mems) if args.judge_url else None
            tally[label]["n"] += 1
            tally[label]["exact"] += int(exact)
            tally[label]["judge"] += int(bool(j))
            per_row_out.append({"q": row["question"][:50], "label": label,
                                "rw": rw_toks, "exact": exact, "judge": bool(j)})

    client.close()
    print(f"\n# Query-rewrite experiment: n={args.sample}, topk={args.topk}\n")
    print(f"{'method':14} | {'exact':>12} | {'judge':>12}")
    print("-" * 46)
    for label in tally:
        t = tally[label]
        if t["n"] == 0: continue
        e = f"{t['exact']}/{t['n']} = {t['exact']/t['n']:.1%}"
        j = f"{t['judge']}/{t['n']} = {t['judge']/t['n']:.1%}" if args.judge_url else "—"
        print(f"{label:14} | {e:>12} | {j:>12}")

    out = HERE / "reports" / "query_rewrite.json"
    out.write_text(json.dumps({"tally": tally, "rewrites": rewrites_log,
                               "per_row": per_row_out, "args": vars(args)}, indent=2))
    print(f"\nwrote {out}")


if __name__ == "__main__":
    main()
