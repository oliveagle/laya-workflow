#!/usr/bin/env python3
"""End-to-end one-shot inference with the native MLX Laya runtime.

Reads a request (a JSON file with ``{"state": ..., "questions": {...}}``) and
prints the full answer JSON — the same shape as ``POST /v1/systemone``:

    python3 laya-tch/mlx/run_once.py --request laya-tch/mlx/examples/ref_request.json

Options:
    --model-dir DIR   checkpoint dir (default: $LAYA_MLX_MODEL_DIR /
                      $LAYA_MODEL_DIR / local HF cache)
    --device gpu|cpu  MLX device (default: gpu)
    --dtype float16|float32|bfloat16 (default: float16)
    --compile         wrap the model in mx.compile (static-shape runs)
    --request PATH    request JSON ('-' reads stdin)
    --json            print only the response JSON (no summary lines)

Exit codes: 0 ok; 2 bad usage / missing checkpoint; 1 inference failure.
"""

from __future__ import annotations

import argparse
import json
import sys

sys.path.insert(0, __file__.rsplit("/", 1)[0])
from _common import add_native_path, find_model_dir  # noqa: E402

add_native_path()


def parse_args() -> argparse.Namespace:
    p = argparse.ArgumentParser(description="Native MLX Laya one-shot inference")
    p.add_argument("--model-dir", default=None)
    p.add_argument("--device", default="gpu", choices=["gpu", "cpu"])
    p.add_argument("--dtype", default="float16", choices=["float16", "float32", "bfloat16"])
    p.add_argument("--compile", action="store_true")
    p.add_argument("--batch-size", type=int, default=16)
    p.add_argument("--request", default=None, help="request JSON path ('-' = stdin)")
    p.add_argument("--json", action="store_true", help="print only the response JSON")
    return p.parse_args()


def load_request(path: str | None) -> dict:
    raw = sys.stdin.read() if path in (None, "-") else open(path, encoding="utf-8").read()
    req = json.loads(raw)
    if not isinstance(req, dict) or "state" not in req or "questions" not in req:
        raise SystemExit("request must be a JSON object with 'state' and 'questions'")
    return req


def main() -> int:
    args = parse_args()
    if args.request is None and sys.stdin.isatty():
        print("error: pass --request <file> (or pipe JSON on stdin)", file=sys.stderr)
        return 2

    import warnings

    warnings.filterwarnings("ignore")
    import laya_mlx

    model_dir = args.model_dir or find_model_dir()
    req = load_request(args.request)

    agent = laya_mlx.load(model_dir, device=args.device, dtype=args.dtype,
                          batch_size=args.batch_size, compile=args.compile)
    result = agent.system_one(req["state"], req["questions"])

    if args.json:
        print(json.dumps(result, ensure_ascii=False))
    else:
        print(f"[run_once] model-dir : {model_dir}")
        print(f"[run_once] device    : {agent.device}  dtype: {args.dtype}")
        for qid, ans in result["answers"].items():
            print(f"[run_once] {qid:12} -> {json.dumps(ans, ensure_ascii=False)}")
        print(json.dumps(result, ensure_ascii=False))
    return 0


if __name__ == "__main__":
    sys.exit(main())
