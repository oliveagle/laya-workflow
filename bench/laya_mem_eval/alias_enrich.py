#!/usr/bin/env python3
"""Round-9: memory-side alias enrichment (mem0 approach).

Round-6 showed query-side LLM rewrite HURTS. mem0 instead enriches the
MEMORY side at ingest time: each memory gets extracted entities /
synonyms / answer-bearing aliases, and search matches query tokens
against content OR aliases. This is conceptually different from query
rewrite because the enrichment is per-memory, so it can only help
candidate generation (never hurt precision by adding distractor terms
to the query).

Experiment (small scale first): for each ingested memory, ask the LLM
to produce 3-6 search aliases. Store in an `aliases` column. Then:
  * bm25+OR @5 on content only          (current production default)
  * bm25+OR @5 on content OR aliases    (enriched)

Measured on the Round-7 isolated-retrieval subset (answer literal IS
in corpus, n=11 at sample=30). Cost: one LLM call per ~8 memories
(batched), only for the alias-enrichment arm.
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
from longmemeval import STOPWORDS, load_sample, turn_text
from run import McpClient, _tool_payload

REPO = HERE.parent.parent
BIN = REPO / "target" / "debug" / "laya-workflow"
SPEC_DIR = REPO / "dsl" / "laya_mem"
MODEL_URL = "http://127.0.0.1:11032/v1"


def tokenize(q: str) -> list[str]:
    return [t for t in re.split(r"[^a-zA-Z0-9]+", q.lower()) if t and t not in STOPWORDS and len(t) > 1]


def llm_alias_batch(contents: list[str]) -> list[list[str]]:
    """One LLM call for a batch of memories -> per-memory alias list."""
    block = "\n\n".join(f"[{i+1}] {c[:300]}" for i, c in enumerate(contents))
    prompt = (
        "For each memory below, list 3-6 short search aliases: alternate "
        "words, synonyms, named entities, or keyphrases someone might search "
        "for to find it. Format strictly as one line per memory:\n"
        "M1: alias1, alias2, alias3\nM2: alias1, ...\n\nMemories:\n" + block
    )
    req = urllib.request.Request(
        f"{MODEL_URL}/chat/completions",
        data=json.dumps({
            "model": "qwen3.8-flash-next",
            "messages": [{"role": "user", "content": prompt}],
            "max_tokens": 900, "temperature": 0, "reasoning_effort": "none",
        }).encode(),
        headers={"Content-Type": "application/json"},
    )
    for attempt in range(3):
        try:
            with urllib.request.urlopen(req, timeout=120) as r:
                body = json.load(r)
            content = body["choices"][0]["message"].get("content") or ""
            per = {i: [] for i in range(len(contents))}
            for line in content.splitlines():
                m = re.match(r"\s*M(\d+)\s*:\s*(.*)", line)
                if not m:
                    continue
                idx = int(m.group(1)) - 1
                if 0 <= idx < len(contents):
                    al = [a.strip().lower() for a in re.split(r"[;,]", m.group(2)) if a.strip()]
                    per[idx] = [a for a in al if a and a not in STOPWORDS]
            return [per[i] for i in range(len(contents))]
        except Exception as e:
            import time as _t
            _t.sleep(3 + attempt)
    return [[] for _ in contents]


def bm25_top(tokens: list[str], limit: int, db: str, alias_table: bool = False) -> list[dict]:
    """bm25+OR top-k; optionally merge in alias-FTS hits (Python-side union)."""
    if not tokens:
        return []
    match = " OR ".join(f'"{t}"*' for t in tokens)
    con = sqlite3.connect(db)
    cols = ["id", "content", "ts", "entities", "type_scores"]
    try:
        sql = (
            "SELECT m.id, m.content, m.ts, m.entities, m.type_scores "
            "FROM memories_fts f JOIN memories m ON m.id = f.rowid "
            f"WHERE memories_fts MATCH '{match}' "
            f"ORDER BY bm25(memories_fts), m.id DESC LIMIT {limit}"
        )
        rows = con.execute(sql).fetchall()
        out = [dict(zip(cols, r)) for r in rows]
        if not alias_table:
            return out
        seen = {m["id"] for m in out}
        try:
            sql_a = (
                "SELECT rowid FROM memory_aliases_fts "
                f"WHERE memory_aliases_fts MATCH '{match}' "
                f"LIMIT {limit}"
            )
            alias_ids = [r[0] for r in con.execute(sql_a).fetchall()]
        except sqlite3.OperationalError:
            alias_ids = []
        for aid in alias_ids:
            if aid in seen:
                continue
            r = con.execute(
                "SELECT id, content, ts, entities, type_scores FROM memories WHERE id=?", (aid,)
            ).fetchone()
            if r is None:
                continue
            seen.add(aid)
            out.append(dict(zip(cols, r)))
            if len(out) >= limit:
                break
        return out
    finally:
        con.close()

def find_evidence(row: dict, answer: str) -> tuple[int, str] | None:
    ans = answer.strip().lower()
    if not ans:
        return None
    for si, sess in enumerate(row.get("haystack_sessions", [])):
        for ti, turn in enumerate(sess):
            if ans in (turn.get("content") or "").lower():
                return si, turn.get("content") or ""
    return None


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--sample", type=int, default=30)
    ap.add_argument("--max-turns", type=int, default=500)
    ap.add_argument("--topk", type=int, default=5)
    ap.add_argument("--enrich-n", type=int, default=0,
                    help="0 = content-only; >0 = enrich first N memories only (fast debug)")
    args = ap.parse_args()

    rows = load_sample(args.sample)
    db = tempfile.mktemp(suffix=".sqlite")
    try: os.remove(db)
    except FileNotFoundError: pass
    client = McpClient(BIN, SPEC_DIR, db)

    tally = {"content@5": {"hit": 0, "n": 0}, "alias@5": {"hit": 0, "n": 0}}
    enrich_used = 0

    for row in rows:
        ev = find_evidence(row, row.get("answer", ""))
        if ev is None:
            continue
        si, ev_content = ev
        # ingest all turns, capture memory ids per content
        content_to_id = {}
        for sess in row.get("haystack_sessions", []):
            for turn in sess:
                t = (turn.get("content") or "").strip()
                if len(t) < 20:
                    continue
                p = _tool_payload(client.tool("laya_mem_persist", {"content": t, "type_scores": {}}))
                content_to_id[t] = p.get("memory_id")
        # find evidence memory id
        ev_id = content_to_id.get(ev_content)
        if ev_id is None:
            continue

        qterm_tokens = tokenize(row["question"])
        if not qterm_tokens:
            continue
        # content-only arm
        mems = bm25_top(qterm_tokens, args.topk, db, alias_table=False)
        tally["content@5"]["n"] += 1
        tally["content@5"]["hit"] += int(ev_id in {m["id"] for m in mems})

        # alias arm: enrich up to enrich_n memories (or all if enrich_n<=0)
        con = sqlite3.connect(db)
        try:
            con.execute("CREATE TABLE IF NOT EXISTS memory_aliases (memory_id INTEGER PRIMARY KEY, content TEXT)")
            con.execute("CREATE VIRTUAL TABLE IF NOT EXISTS memory_aliases_fts USING fts5(content, content='memory_aliases', content_rowid='memory_id')")
            ids = [r[0] for r in con.execute("SELECT id FROM memories ORDER BY id").fetchall()]
        finally:
            con.close()
        if args.enrich_n > 0:
            ids = ids[:args.enrich_n]
        todo = [i for i in ids if i >= 0]
        # batch by 8
        for b in range(0, len(todo), 8):
            batch_ids = todo[b:b+8]
            batch_contents = []
            con = sqlite3.connect(db)
            try:
                for i in batch_ids:
                    r = con.execute("SELECT content FROM memories WHERE id=?", (i,)).fetchone()
                    batch_contents.append(r[0] if r else "")
            finally:
                con.close()
            aliases = llm_alias_batch(batch_contents)
            con = sqlite3.connect(db)
            try:
                for i, al in zip(batch_ids, aliases):
                    if al:
                        con.execute("INSERT OR REPLACE INTO memory_aliases (memory_id, content) VALUES (?, ?)",
                                    (i, " ".join(al)))
                con.commit()
            finally:
                con.close()
            enrich_used += len(batch_ids)

        # alias arm query
        amems = bm25_top(qterm_tokens, args.topk, db, alias_table=True)
        tally["alias@5"]["n"] += 1
        tally["alias@5"]["hit"] += int(ev_id in {m["id"] for m in amems})

    client.close()
    print(f"\n# Alias-enrichment experiment: rows with answer-in-corpus={tally['content@5']['n']}, enriched={enrich_used}\n")
    print(f"{'method':12} | {'recall@5':>14}")
    print("-" * 30)
    for label, t in tally.items():
        if t["n"] == 0:
            continue
        print(f"{label:12} | {t['hit']}/{t['n']} = {t['hit']/t['n']:>10.1%}")

    out = HERE / "reports" / "alias_enrich.json"
    out.write_text(json.dumps({"tally": tally, "enrich_used": enrich_used, "args": vars(args)}, indent=2))
    print(f"\nwrote {out}")


if __name__ == "__main__":
    main()
