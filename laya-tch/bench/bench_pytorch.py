"""PyTorch latency baseline for the Laya english checkpoint (CPU).

Reports warm per-row forward latency and the batched `system_one` latency for the
same request used by `ref_capture.py`, so the Rust engine can be compared fairly.

Run: $PYTHON (a python that has torch + transformers installed)laya-tch/bench/bench_pytorch.py
"""
from __future__ import annotations

import json
import os
import sys
import time
from pathlib import Path

import torch

HERE = Path(__file__).resolve().parent
MODEL_DIR = Path(os.environ.get("LAYA_MODEL_DIR", Path.home() / "models" / "convaiinnovations--laya"))
sys.path.insert(0, str(MODEL_DIR))
from rl_agent_api import RLAgent  # noqa: E402
from rl_common import build_sequence, collate_items, QTYPES  # noqa: E402

TRACE = json.loads((HERE / "reference_trace.json").read_text())
STATE, QUESTIONS = TRACE["state"], TRACE["questions"]


def main() -> None:
    print(f"torch threads: {torch.get_num_threads()}")
    agent = RLAgent(str(MODEL_DIR), device="cpu")
    cfg = agent.cfg

    rows = []
    for qid in QUESTIONS:
        q = agent._to_internal(QUESTIONS[qid])
        ids, markers = build_sequence(agent.tok, STATE, q, cfg["max_len"], cfg["head_max_len"])
        rows.append({"qid": qid, "ids": ids, "markers": markers, "qtype": QTYPES[q["t"]]})

    # warmup
    with torch.no_grad():
        for _ in range(2):
            for r in rows:
                agent.model(torch.tensor([r["ids"]]), torch.ones(1, len(r["ids"]), dtype=torch.long),
                            torch.tensor([r["markers"]]), torch.ones(1, len(r["markers"]), dtype=torch.bool),
                            torch.tensor([r["qtype"]]))

    # per-row warm latency
    with torch.no_grad():
        for r in rows:
            ids = torch.tensor([r["ids"]])
            am = torch.ones_like(ids)
            mp = torch.tensor([r["markers"]])
            mm = torch.ones_like(mp, dtype=torch.bool)
            qt = torch.tensor([r["qtype"]])
            ts = []
            for _ in range(5):
                t0 = time.perf_counter()
                agent.model(ids, am, mp, mm, qt)
                ts.append((time.perf_counter() - t0) * 1000)
            ts.sort()
            print(f"  row {r['qid']:18s} L={len(r['ids']):3d}  single-row warm median {ts[len(ts)//2]:7.1f} ms")

    # system_one (batched, what the API actually does)
    ts = []
    for _ in range(5):
        t0 = time.perf_counter()
        agent.system_one(STATE, QUESTIONS)
        ts.append((time.perf_counter() - t0) * 1000)
    ts.sort()
    print(f"  system_one (batch {len(rows)} q)  warm median {ts[len(ts)//2]:7.1f} ms  "
          f"per-question {ts[len(ts)//2]/len(rows):.1f} ms")

    # model-only batched forward (excludes tokenization), min of 10
    items = []
    for r in rows:
        items.append({"ids": r["ids"], "markers": r["markers"], "qtype": r["qtype"],
                      "target": [0.0] * len(r["markers"]), "label": -1, "episode": 0,
                      "ep_step": 0, "ep_len": 1, "src": "bench"})
    b = collate_items([items], agent.tok.pad_token_id)
    args = (b["input_ids"], b["attention_mask"], b["marker_pos"], b["marker_mask"], b["qtype"])
    with torch.no_grad():
        for _ in range(3):
            agent.model(*args)
        ts = []
        for _ in range(10):
            t0 = time.perf_counter()
            agent.model(*args)
            ts.append((time.perf_counter() - t0) * 1000)
    print(f"  model-only batch {len(rows)} q      warm min {min(ts):7.1f} ms  median {sorted(ts)[len(ts)//2]:7.1f} ms")


if __name__ == "__main__":
    main()
