"""Fair head-to-head: laya-tch (Rust/libtorch) vs PyTorch on identical requests.

Alternates the two engines (so CPU frequency / cache state is comparable) and
reports the median of N runs for:
  * single-row forward (per question, true length, no padding)
  * batched request forward (all questions in one forward, like RLAgent.system_one)

Run: $PYTHON (a python that has torch + transformers installed)code/laya-tch/bench/compare_perf.py
"""
from __future__ import annotations

import json
import os
import re
import statistics
import subprocess
import sys
from pathlib import Path

import torch

HERE = Path(__file__).resolve().parent
CRATE = HERE.parent
MODEL_DIR = Path(os.environ.get("LAYA_MODEL_DIR", Path.home() / "models" / "convaiinnovations--laya"))
BIN = Path(os.environ.get("LAYA_TCH_BIN", CRATE / "target" / "release" / "bench_parity"))

sys.path.insert(0, str(MODEL_DIR))
from rl_agent_api import RLAgent  # noqa: E402
from rl_common import QTYPES, build_sequence, collate_items  # noqa: E402

TRACE = json.loads((HERE / "reference_trace.json").read_text())
STATE, QUESTIONS = TRACE["state"], TRACE["questions"]
N = int(os.environ.get("N", "5"))


def rust_once() -> tuple[float, float, float]:
    out = subprocess.run([str(BIN), str(MODEL_DIR)], capture_output=True, text=True, check=True).stdout
    single = float(re.search(r"worst max\|Δ\| = [\d.eE+-]+.*?avg ([\d.]+) ms/row", out).group(1))
    batched = float(re.search(r"batched \d+ rows:.*?([\d.]+) ms/request", out).group(1))
    perc = float(re.search(r"batched \d+ rows:.*?\(([\d.]+) ms/question\)", out).group(1))
    delta = float(re.search(r"worst max\|Δ\| \(vs torch batched\) = ([\d.eE+-]+)", out).group(1))
    return single, batched, perc, delta


def main() -> None:
    torch.set_grad_enabled(False)
    agent = RLAgent(str(MODEL_DIR), device="cpu")

    rows = []
    for qid in QUESTIONS:
        q = agent._to_internal(QUESTIONS[qid])
        ids, markers = build_sequence(agent.tok, STATE, q, agent.cfg["max_len"], agent.cfg["head_max_len"])
        rows.append({"ids": ids, "markers": markers, "qtype": QTYPES[q["t"]]})

    # torch single-row
    def torch_single() -> float:
        ts = []
        for r in rows:
            ids = torch.tensor([r["ids"]]); am = torch.ones_like(ids)
            mp = torch.tensor([r["markers"]]); mm = torch.ones_like(mp, dtype=torch.bool)
            qt = torch.tensor([r["qtype"]])
            with torch.no_grad():
                for _ in range(2):
                    agent.model(ids, am, mp, mm, qt)
                t0 = torch.cuda.Event(enable_timing=False) if False else None
                import time
                t = time.perf_counter()
                agent.model(ids, am, mp, mm, qt)
                ts.append((time.perf_counter() - t) * 1000)
        return statistics.median(ts)

    items = [{"ids": r["ids"], "markers": r["markers"], "qtype": r["qtype"], "target": [0.0] * len(r["markers"]),
              "label": -1, "episode": 0, "ep_step": 0, "ep_len": 1, "src": "bench"} for r in rows]
    b = collate_items([items], agent.tok.pad_token_id)
    args = (b["input_ids"], b["attention_mask"], b["marker_pos"], b["marker_mask"], b["qtype"])

    def torch_batched() -> float:
        import time
        with torch.no_grad():
            for _ in range(2):
                agent.model(*args)
            t = time.perf_counter()
            agent.model(*args)
            return (time.perf_counter() - t) * 1000

    r_single: list[float] = []
    r_batch: list[float] = []
    t_single: list[float] = []
    t_batch: list[float] = []
    delta = 0.0
    for _ in range(N):
        s, ba, _, d = rust_once()
        r_single.append(s); r_batch.append(ba); delta = max(delta, d)
        t_single.append(torch_single())
        t_batch.append(torch_batched())

    print(f"\nN={N} iterations (alternating)")
    print(f"  single-row per question :  rust {statistics.median(r_single):7.1f} ms   torch {statistics.median(t_single):7.1f} ms   "
          f"ratio {statistics.median(r_single)/statistics.median(t_single):.3f}")
    print(f"  batched request (4 q)   :  rust {statistics.median(r_batch):7.1f} ms   torch {statistics.median(t_batch):7.1f} ms   "
          f"ratio {statistics.median(r_batch)/statistics.median(t_batch):.3f}")
    print(f"  per-question (batched)  :  rust {statistics.median(r_batch)/len(rows):7.1f} ms   torch {statistics.median(t_batch)/len(rows):7.1f} ms")
    print(f"  worst logit |Δ| vs torch:  {delta:.3e}")
    verdict = "PASS" if (statistics.median(r_batch)/statistics.median(t_batch) < 1.15 and delta < 1e-3) else "FAIL"
    print(f"  verdict: {verdict}  (perf within 15%, logits within 1e-3)")


if __name__ == "__main__":
    main()
