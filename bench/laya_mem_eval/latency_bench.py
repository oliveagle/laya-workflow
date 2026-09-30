#!/usr/bin/env python3
"""Round-12: retrieval latency micro-benchmark — how fast is laya-mem recall?

Measures wall-clock per operation at increasing memory-store sizes:
  persist       — laya_mem_persist MCP call (ingest one fact)
  recall_bm25   — laya_mem_recall MCP call (FTS5 bm25+OR, no LLM)
  recall_bge    — BGE-small-en-v1.5 cosine over all embeddings (fastembed)
  recall_wemm   — WeMM-Embedding-2B cosine over all embeddings (GPU)

Traditional baselines:
  full_ctx      — LLM call cost to re-answer with whole transcript in
                  prompt (measured separately via codex_compare).
The point: laya-mem recall is a DB query (<ms), not an LLM round trip.

Stores sizes: 100 / 1k / 10k / 50k synthetic memories.
"""
from __future__ import annotations

import argparse
import json
import os
import statistics
import sys
import tempfile
import time
from pathlib import Path

HERE = Path(__file__).parent
sys.path.insert(0, str(HERE))
from run import McpClient, _tool_payload

REPO = HERE.parent.parent
BIN = REPO / "target" / "debug" / "laya-workflow"
SPEC_DIR = REPO / "dsl" / "laya_mem"

WORDS = ["deploy", "rollback", "shard", "cache", "gateway", "broker", "cluster",
         "pipeline", "incident", "rotation", "release", "feature", "flag", "audit"]
QUERIES = [
    "what is the deployment rollback procedure",
    "who is on the incident rotation",
    "how does the cache gateway work",
    "what was the last release incident",
    "which cluster holds the shard cache",
]


def synth_memories(n: int) -> list[str]:
    out = []
    for i in range(n):
        a = WORDS[i % len(WORDS)]
        b = WORDS[(i * 7 + 3) % len(WORDS)]
        c = WORDS[(i * 13 + 5) % len(WORDS)]
        out.append(f"fact {i}: the {a} {b} {c} was reviewed on day {i % 365} in cycle {i // 50}.")
    return out


def bench_store(n: int, skip_embed: bool):
    db = tempfile.mktemp(suffix=".sqlite")
    try:
        os.remove(db)
    except FileNotFoundError:
        pass
    client = McpClient(BIN, SPEC_DIR, db)
    mems = synth_memories(n)

    # persist latency (measure last 50)
    p_times = []
    for i, m in enumerate(mems):
        t = time.perf_counter()
        _tool_payload(client.tool("laya_mem_persist", {"content": m, "type_scores": {}}))
        if i >= n - 50:
            p_times.append((time.perf_counter() - t) * 1000)

    # recall latency (50 runs)
    r_times = []
    for i in range(50):
        q = QUERIES[i % len(QUERIES)]
        t = time.perf_counter()
        _tool_payload(client.tool("laya_mem_recall", {"query": q, "limit": 5,
                                                       "include_relations": False}))
        r_times.append((time.perf_counter() - t) * 1000)
    client.close()

    res = {
        "n": n,
        "persist_p50_ms": round(statistics.median(p_times), 3),
        "persist_p95_ms": round(sorted(p_times)[int(len(p_times) * 0.95)], 3),
        "recall_bm25_p50_ms": round(statistics.median(r_times), 3),
        "recall_bm25_p95_ms": round(sorted(r_times)[int(len(r_times) * 0.95)], 3),
    }

    if not skip_embed:
        # BGE-small encode-once + query latency
        try:
            from fastembed import TextEmbedding
            import numpy as np
            em = TextEmbedding("BAAI/bge-small-en-v1.5",
                               cache_dir="/home/oliveagle/models/embedding/fastembed")
            # warmup
            list(em.embed(["warmup"]))
            t0 = time.perf_counter()
            vecs = np.vstack(list(em.embed(mems)))
            enc_total = time.perf_counter() - t0
            q_times = []
            for i in range(20):
                q = QUERIES[i % len(QUERIES)]
                t = time.perf_counter()
                qv = np.vstack(list(em.embed([q])))
                _ = (vecs @ qv.T).ravel()
                q_times.append((time.perf_counter() - t) * 1000)
            res["bge_encode_total_s"] = round(enc_total, 3)
            res["bge_query_p50_ms"] = round(statistics.median(q_times), 3)
        except Exception as e:
            res["bge_error"] = str(e)[:120]
    return res


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--sizes", default="100,1000,10000")
    ap.add_argument("--skip-embed", action="store_true")
    args = ap.parse_args()
    rows = []
    for n in [int(x) for x in args.sizes.split(",")]:
        print(f"benchmarking N={n} ...", flush=True)
        r = bench_store(n, args.skip_embed)
        rows.append(r)
        print("  " + json.dumps(r))
    out = HERE / "reports" / "latency_bench.json"
    out.write_text(json.dumps(rows, indent=2))
    print(f"wrote {out}")


if __name__ == "__main__":
    main()
