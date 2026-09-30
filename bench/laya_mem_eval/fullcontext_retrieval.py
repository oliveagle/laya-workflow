#!/usr/bin/env python3
"""Round-10a: LLM full-context retrieval — quantify the semantic ceiling.

Every lexical method (bm25+OR / query-rewrite / LLM-rerank / alias-
enrichment) is stuck at 45.5% recall@5 on LongMemEval_s answer-in-
corpus subset because the evidence turn carries the ANSWER LITERAL, not
the question topic. This experiment asks: if we give the LLM the ENTIRE
memory store (all ~500 turns) and ask it to find the evidence, can it?
If yes, a semantic embedding retriever SHOULD be able to as well —
proving the gap is semantic, not lexical.

Uses the local qwen3.8-flash-next (262K ctx) via OpenAI-compatible API.
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
from longmemeval import load_sample
from run import McpClient, _tool_payload

REPO = HERE.parent.parent
BIN = REPO / "target" / "debug" / "laya-workflow"
SPEC_DIR = REPO / "dsl" / "laya_mem"
MODEL_URL = "http://127.0.0.1:11032/v1"


def find_evidence(row: dict, answer: str) -> tuple[int, str] | None:
    ans = answer.strip().lower()
    if not ans:
        return None
    for si, sess in enumerate(row.get("haystack_sessions", [])):
        for ti, turn in enumerate(sess):
            if ans in (turn.get("content") or "").lower():
                return si, turn.get("content") or ""
    return None


def llm_retrieve(question: str, memories: list[dict], timeout: int = 300) -> tuple[int | None, str]:
    """One call: full memory store + question -> evidence memory_id."""
    # memory_id is memory DB id; we present as index to keep numbers small
    idx_to_db = []
    chunks = []
    for db_id, m in memories:
        idx_to_db.append(db_id)
        chunks.append(f"[{len(idx_to_db)}] {m[:400]}")
    store = "\n\n".join(chunks)
    prompt = (
        "You are a memory retrieval engine for a personal assistant. Below "
        "is the full conversation memory store (each entry has a numeric id "
        "and content). Given a user question, find the SINGLE memory entry "
        "that contains the information needed to answer it. The answer "
        "literal is present verbatim in exactly one entry.\n\n"
        f"Question: {question}\n\n"
        f"Memory store:\n{store}\n\n"
        "Reply with exactly the numeric id in brackets, e.g. [42]. If no "
        "entry suffices, reply [NONE]."
    )
    req = urllib.request.Request(
        f"{MODEL_URL}/chat/completions",
        data=json.dumps({
            "model": "qwen3.8-flash-next",
            "messages": [{"role": "user", "content": prompt}],
            "max_tokens": 32, "temperature": 0, "reasoning_effort": "none",
        }).encode(),
        headers={"Content-Type": "application/json"},
    )
    with urllib.request.urlopen(req, timeout=timeout) as r:
        body = json.load(r)
    content = (body["choices"][0]["message"].get("content") or "").strip()
    m = re.search(r"\[(\d+|NONE)\]", content, re.IGNORECASE)
    if not m:
        return None, content
    tok = m.group(1).upper()
    if tok == "NONE":
        return None, content
    idx = int(m.group(1))
    if 0 <= idx - 1 < len(idx_to_db):
        return idx_to_db[idx - 1], content
    return None, content


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--sample", type=int, default=30)
    ap.add_argument("--max-turns", type=int, default=500)
    ap.add_argument("--limit-rows", type=int, default=0, help="0=all, else first N with evidence")
    ap.add_argument("--min-len", type=int, default=20)
    ap.add_argument("--max-len", type=int, default=600, help="truncate each memory to N chars")
    args = ap.parse_args()

    rows = load_sample(args.sample)
    db = tempfile.mktemp(suffix=".sqlite")
    try:
        os.remove(db)
    except FileNotFoundError:
        pass
    client = McpClient(BIN, SPEC_DIR, db)

    per_row = []
    done = 0
    for row in rows:
        ev = find_evidence(row, row.get("answer", ""))
        if ev is None:
            continue
        si, ev_content = ev
        # ingest all turns -> memory ids
        content_to_id = {}
        for sess in row.get("haystack_sessions", []):
            for turn in sess:
                t = (turn.get("content") or "").strip()
                if len(t) < args.min_len:
                    continue
                p = _tool_payload(client.tool("laya_mem_persist", {"content": t, "type_scores": {}}))
                content_to_id[t] = p.get("memory_id")
        ev_id = content_to_id.get(ev_content)
        if ev_id is None:
            continue

        con = sqlite3.connect(db)
        mems = con.execute(
            "SELECT id, content FROM memories ORDER BY id"
        ).fetchall()
        con.close()
        mems = [(i, c[:args.max_len]) for i, c in mems if c]

        got_id, raw = llm_retrieve(row["question"], mems)
        hit = got_id == ev_id
        per_row.append({
            "q": row["question"], "ans": row.get("answer", ""),
            "ev_id": ev_id, "pred_id": got_id, "hit": hit, "raw": raw[:120],
            "n_memories": len(mems),
        })
        done += 1
        print(f"[{done}] {'HIT ' if hit else 'MISS'} n_mem={len(mems)} "
              f"ev={ev_id} pred={got_id}")
        if args.limit_rows and done >= args.limit_rows:
            break

    client.close()
    n = len(per_row)
    hits = sum(1 for r in per_row if r["hit"])
    print(f"\n# Full-context LLM retrieval: {hits}/{n} = {hits/n:.1%} (answer-in-corpus subset)")
    out = HERE / "reports" / "fullcontext_retrieval.json"
    out.write_text(json.dumps({
        "tally": {"hit": hits, "n": n},
        "per_row": per_row,
        "args": vars(args),
    }, ensure_ascii=False, indent=2))
    print(f"wrote {out}")


if __name__ == "__main__":
    main()
