#!/usr/bin/env python3
"""End-to-end benchmark of the native MLX Laya model (Apple GPU vs CPU).

Measures whole-request latency (all questions in one batched forward), the
batch-scaling curve, and the GPU-vs-CPU speedup on the same weights — so the
"MLX on the Metal GPU" claim is backed by numbers, not by a single op.

    python3 laya-tch/mlx/bench_model.py
    python3 laya-tch/mlx/bench_model.py --reps 30 --state-repeats 4

Exit 0 on success.
"""

from __future__ import annotations

import argparse
import statistics
import sys
import time

sys.path.insert(0, __file__.rsplit("/", 1)[0])
from _common import DEMO_QUESTIONS, DEMO_STATE, add_native_path, find_model_dir  # noqa: E402

add_native_path()


def parse_args() -> argparse.Namespace:
    p = argparse.ArgumentParser(description="Native MLX Laya end-to-end benchmark")
    p.add_argument("--model-dir", default=None)
    p.add_argument("--dtype", default="float16", choices=["float16", "float32"])
    p.add_argument("--reps", type=int, default=20, help="timed repetitions (default 20)")
    p.add_argument("--warmup", type=int, default=3)
    p.add_argument("--state-repeats", type=int, default=1,
                   help="repeat the demo state N times to lengthen the sequence")
    p.add_argument("--batch-scaling", action=argparse.BooleanOptionalAction, default=True)
    return p.parse_args()


def timed(agent, state, questions, *, warmup, reps):
    for _ in range(warmup):
        agent.system_one(state, questions)
    samples = []
    for _ in range(reps):
        t0 = time.perf_counter()
        agent.system_one(state, questions)
        samples.append((time.perf_counter() - t0) * 1000.0)
    return samples


def main() -> int:
    args = parse_args()
    import warnings

    warnings.filterwarnings("ignore")
    import laya_mlx

    model_dir = args.model_dir or find_model_dir()
    state = DEMO_STATE * max(1, args.state_repeats)

    print(f"[bench_model] checkpoint : {model_dir}")
    print(f"[bench_model] questions  : {len(DEMO_QUESTIONS)}  dtype: {args.dtype}  reps: {args.reps}")

    results = {}
    for device in ("cpu", "gpu"):
        agent = laya_mlx.load(model_dir, device=device, dtype=args.dtype)
        samples = timed(agent, state, DEMO_QUESTIONS, warmup=args.warmup, reps=args.reps)
        med = statistics.median(samples)
        p90 = sorted(samples)[max(0, int(round(0.9 * (len(samples) - 1))))]
        inp = agent.system_one(state, DEMO_QUESTIONS)["usage"]["input_tokens"]
        results[device] = med
        print(
            f"[bench_model] {device.upper():3}  median {med:8.1f} ms   p90 {p90:8.1f} ms   "
            f"min {min(samples):7.1f} ms   ({inp} input tokens)"
        )

    speedup = results["cpu"] / results["gpu"] if results["gpu"] else float("nan")
    print(f"[bench_model] GPU vs CPU : {speedup:.2f}x  (median-of-{args.reps}, wider = faster)")

    if args.batch_scaling:
        print("[bench_model] batch scaling (GPU):")
        agent = laya_mlx.load(model_dir, device="gpu", dtype=args.dtype)
        for n in (1, 2, 4, 8, 16):
            qs = {
                f"q{i}": {
                    "type": "choice",
                    "instructions": "Which department should handle this request?",
                    "criteria": ["billing", "technical", "sales", "other"],
                }
                for i in range(n)
            }
            samples = timed(agent, state, qs, warmup=1, reps=max(3, args.reps // 4))
            med = statistics.median(samples)
            print(f"[bench_model]   {n:2} questions: {med:7.1f} ms total  {med / n:6.1f} ms/question")

    print("[bench_model] DONE")
    return 0


if __name__ == "__main__":
    sys.exit(main())
