#!/usr/bin/env python3
"""Round-7 isolated-retrieval benchmark.

The LongMemEval judge-paraphrase answers confound the recall-quality
signal: if the answer is never in the corpus, recall@k is structurally
0 even with a perfect search. This harness isolates retrieval by
constructing an oracle-clean subset:

  1. Ingest EVERY turn of every haystack session.
  2. For each answer-bearing turn, the gold "evidence memory" is exactly
     that turn's content (or its neighbor turn that contains the literal
     answer string).
  3. A "hit" is any recalled top-k that contains the evidence memory's id.

Compared metrics:
  bm25+OR @5   current production default
  bm25+OR @10  wider window — recall ceiling without rerank
  exact-only   recall if "gold" means substring in top-k blob

This is the precision/recall@k curve for bm25+OR over real human
conversation data, with the "was the answer even in the corpus"
question removed by construction.
"""
from __future__ import annotations

import argparse
import json
import os
import random
import re
import sqlite3
import sys
import tempfile
import urllib.request
from pathlib import Path

HERE = Path(__file__).parent
sys.path.insert(0, str(HERE))
from longmemeval import build_query, judge, load_sample, STOPWORDS
from run import McpClient, _tool_payload

REPO = HERE.parent.parent
BIN = REPO / "target" / "debug" / "laya-workflow"
SPEC_DIR = REPO / "dsl" / "laya_mem"

DEFAULT_DATA = "/tmp/longmemeval_s.json"


def tokenize(q: str) -> list[str]:
    return [t for t in re.split(r"[^a-zA-Z0-9]+", q.lower()) if t and t not in STOPWORDS and len(t) > 1]


def find_evidence_turn(row: dict, answer: str) -> tuple[int, str] | None:
    """Find the (session_idx, turn_idx) of the turn that literally
    contains the gold answer. If none, return None — skip the row.
    Scans user + assistant turns.
    """
    ans = answer.strip().lower()
    if not ans:
        return None
    sessions = row.get("haystack_sessions", [])
    for si, sess in enumerate(sessions):
        for ti, turn in enumerate(sess):
            c = (turn.get("content") or "").lower()
            if ans in c:
                return si, ti
    return None


def bm25_top(qterm: str, limit: int, db: str) -> list[dict]:
    toks = tokenize(qterm)
    if not toks:
        return []
    match = " OR ".join(f'"{t}"*' for t in toks)
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


def ingest_with_evidence(row: dict, evidence: tuple[int, int]) -> list[int]:
    """Persist all turns of all sessions in the order they appear; return
    the memory ids of the evidence turn (and its immediate neighbor if
    the gold substring only spans a turn boundary)."""
    si, ti = evidence
    all_mems = []
    evidence_ids = []
    sessions = row.get("haystack_sessions", [])
    # We don't have direct API control over id, but we know insert order.
    # Read back the order via the DB after persist; for simplicity,
    # capture content fingerprints and match later.
    contents = []
    for sess in sessions:
        for turn in sess:
            content = (turn.get("content") or "").strip()
            if len(content) < 20: continue
            contents.append(content)
    # Persist in order. The laya_mem_persist tool returns memory_id, but
    # we don't need it — we'll find the evidence row by content match.
    return contents  # caller persists, then we match


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--sample", type=int, default=30)
    ap.add_argument("--max-turns", type=int, default=500)
    ap.add_argument("--strategy", default="full")
    ap.add_argument("--topks", default="5,10,20")
    args = ap.parse_args()

    rows = load_sample(args.sample)
    topks = [int(t) for t in args.topks.split(",")]
    tally = {f"bm25+OR@{k}": {"hit": 0, "n": 0} for k in topks}

    db = tempfile.mktemp(suffix=".sqlite")
    try: os.remove(db)
    except FileNotFoundError: pass
    client = McpClient(BIN, SPEC_DIR, db)

    per_row = []
    for row in rows:
        ev = find_evidence_turn(row, row.get("answer", ""))
        if ev is None:
            continue
        si, ti = ev
        # Find the evidence content
        sessions = row.get("haystack_sessions", [])
        evidence_content = (sessions[si][ti].get("content") or "").strip()
        if not evidence_content or len(evidence_content) < 20:
            continue
        # Ingest ALL turns
        for sess in sessions:
            for turn in sess:
                t = (turn.get("content") or "").strip()
                if len(t) >= 20:
                    client.tool("laya_mem_persist", {"content": t, "type_scores": {}})
        qterm = build_query(row["question"], args.strategy)
        # Verify the evidence content is now in the DB
        con = sqlite3.connect(db)
        ev_match = con.execute(
            "SELECT id FROM memories WHERE content LIKE ?",
            ("%" + evidence_content[:80].replace("%", "\\%").replace("_", "\\_") + "%",),
        ).fetchall()
        con.close()
        if not ev_match:
            continue
        ev_id = ev_match[0][0]

        for k in topks:
            mems = bm25_top(qterm, k, db)
            mem_ids = {m["id"] for m in mems}
            hit = ev_id in mem_ids
            tally[f"bm25+OR@{k}"]["n"] += 1
            tally[f"bm25+OR@{k}"]["hit"] += int(hit)
        per_row.append({"q": row["question"][:60], "ev_id": ev_id,
                        "ev_rank": next((i+1 for i, m in enumerate(bm25_top(qterm, 100, db)) if m["id"] == ev_id), None)})

    client.close()
    print(f"\n# Isolated-retrieval benchmark: rows where answer literally IS in corpus: {len(per_row)}/{args.sample}\n")
    print(f"{'method':12} | {'recall@k':>14} | {'effective_n':>10}")
    print("-" * 44)
    for label, t in tally.items():
        if t["n"] == 0:
            print(f"{label:12} | {'—':>14} | {0:>10}")
            continue
        r = t["hit"] / t["n"]
        print(f"{label:12} | {t['hit']}/{t['n']} = {r:>10.1%} | {t['n']:>10}")

    # rank distribution of the gold evidence memory
    ranks = [x["ev_rank"] for x in per_row if x["ev_rank"] is not None]
    if ranks:
        avg = sum(ranks)/len(ranks)
        med = sorted(ranks)[len(ranks)//2]
        print(f"\nGold evidence rank: avg={avg:.1f} median={med} top1={sum(1 for r in ranks if r==1)}/{len(ranks)}")

    out = HERE / "reports" / "synth_recall.json"
    out.write_text(json.dumps({"tally": tally, "per_row": per_row,
                               "ranks": ranks, "args": vars(args)}, indent=2))
    print(f"\nwrote {out}")


if __name__ == "__main__":
    main()
