#!/usr/bin/env python3
"""Round-10c: WeMM-Embedding-2B semantic retrieval (A/B vs bm25 & BGE-small).

Same harness as embedding_retrieval.py but with the WeMM-Embedding-2B
backend (run under the sglang-flashnext-venv which has torch + ST 6.1).
Documents encoded once per row; query encoded once; top-k by cosine.
"""
from __future__ import annotations

import argparse
import json
import os
import re
import sqlite3
import sys
import tempfile
from pathlib import Path

HERE = Path(__file__).parent
sys.path.append(str(HERE))
from longmemeval import load_sample
from run import McpClient, _tool_payload

REPO = HERE.parent.parent
BIN = REPO / "target" / "debug" / "laya-workflow"
SPEC_DIR = REPO / "dsl" / "laya_mem"
MODEL_DIR = os.environ.get("WEMM_DIR",
    "/home/oliveagle/.cache/modelscope/models/tencent-community--WeMM-Embedding-2B/snapshots/master")


def tokenize(q: str) -> list[str]:
    from longmemeval import STOPWORDS
    return [t for t in re.split(r"[^a-zA-Z0-9]+", q.lower()) if t and t not in STOPWORDS and len(t) > 1]


def find_evidence(row: dict, answer: str) -> tuple[int, str] | None:
    ans = answer.strip().lower()
    if not ans:
        return None
    for si, sess in enumerate(row.get("haystack_sessions", [])):
        for ti, turn in enumerate(sess):
            if ans in (turn.get("content") or "").lower():
                return si, turn.get("content") or ""
    return None


def bm25_top(tokens: list[str], limit: int, db: str) -> list[int]:
    if not tokens:
        return []
    match = " OR ".join(f'"{t}"*' for t in tokens)
    con = sqlite3.connect(db)
    try:
        return [r[0] for r in con.execute(
            "SELECT f.rowid FROM memories_fts f "
            f"WHERE memories_fts MATCH '{match}' "
            f"ORDER BY bm25(memories_fts), f.rowid DESC LIMIT {limit}"
        ).fetchall()]
    finally:
        con.close()


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--sample", type=int, default=30)
    ap.add_argument("--topk", type=int, default=5)
    ap.add_argument("--min-len", type=int, default=20)
    ap.add_argument("--limit-rows", type=int, default=0)
    ap.add_argument("--batch", type=int, default=64)
    args = ap.parse_args()

    import numpy as np
    from sentence_transformers import SentenceTransformer
    model = SentenceTransformer(MODEL_DIR, trust_remote_code=True, device="cuda")

    rows = load_sample(args.sample)
    tally = {k: {"hit": 0, "n": 0} for k in ["bm25", "wemm", "hyb_n1", "hyb_n2", "hyb_n3", "hyb_n4"]}
    per_row = []
    done = 0
    for row in rows:
        ev = find_evidence(row, row.get("answer", ""))
        if ev is None:
            continue
        si, ev_content = ev
        db = tempfile.mktemp(suffix=".sqlite")
        try:
            os.remove(db)
        except FileNotFoundError:
            pass
        client = McpClient(BIN, SPEC_DIR, db)
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
            client.close()
            continue

        con = sqlite3.connect(db)
        all_rows = con.execute("SELECT id, content FROM memories ORDER BY id").fetchall()
        con.close()
        ids = [r[0] for r in all_rows]
        texts = [r[1] for r in all_rows]
        qtokens = tokenize(row["question"])

        res = {"q": row["question"], "ans": row.get("answer", ""), "ev_id": ev_id}
        b_ids = bm25_top(qtokens, args.topk, db)
        res["bm25"] = (ev_id in b_ids)

        # WeMM: encode docs in batches, query once
        de = []
        for i in range(0, len(texts), args.batch):
            de.append(model.encode_document(texts[i:i+args.batch], normalize_embeddings=True))
        de = np.vstack(de) if de else np.zeros((0, model.get_sentence_embedding_dimension()))
        qe = np.asarray(model.encode_query([row["question"]], normalize_embeddings=True))[0]
        sims = (de @ qe).ravel()
        order = np.argsort(-sims)
        g_ids = [ids[i] for i in order[:args.topk].tolist()]
        res["wemm"] = (ev_id in g_ids)

        for n in (1, 2, 3, 4):
            b_n = b_ids[:n]
            hyb = b_n + [i for i in g_ids if i not in b_n]
            res[f"hyb_n{n}"] = (ev_id in hyb[:args.topk])

        for k, hit in res.items():
            if k in tally:
                tally[k]["n"] += 1
                tally[k]["hit"] += int(hit)
        per_row.append(res)
        done += 1
        flags = "".join(f"{k}{'H' if res[k] else 'M'}" for k in ("bm25", "wemm", "hyb_n2"))
        print(f"[{done}] {flags} n_mem={len(texts)} ev={ev_id} | {row['question'][:70]}", flush=True)
        client.close()
        if args.limit_rows and done >= args.limit_rows:
            break

    print(f"\n# WeMM-Embedding-2B retrieval (sample={args.sample}, answer-in-corpus n={tally['bm25']['n']})")
    print(f"{'method':10} | {'recall@5':>18}")
    print("-" * 32)
    for k, t in tally.items():
        if t["n"]:
            print(f"{k:10} | {t['hit']}/{t['n']} = {t['hit']/t['n']:>10.1%}")
    out = HERE / "reports" / "embedding_retrieval_wemm.json"
    out.write_text(json.dumps({"tally": {k: v for k, v in tally.items() if v["n"]},
                               "per_row": per_row, "args": vars(args)},
                              ensure_ascii=False, indent=2))
    print(f"wrote {out}")


if __name__ == "__main__":
    main()
