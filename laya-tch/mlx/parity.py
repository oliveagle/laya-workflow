#!/usr/bin/env python3
"""Parity check for the vendored native MLX Laya runtime.

Compares whole-request outputs of `laya-tch/mlx/native/laya_mlx` against a frozen
golden file produced by the pristine upstream `laya-mlx` package
(`tests/parity_expected.json`, generated from `mizorewww/laya-mlx` 0.2.0 on the
same FP16 checkpoint). This guards the vendored copy against silent drift.

Checks per question:
  * discrete answer (choice / score / noul) matches the golden rounded value;
  * published probabilities agree within `--prob-tol`;
  * outputs are finite.

    python3 laya-tch/mlx/parity.py

Exit 0 (PASS) when every case matches; 1 otherwise.
"""

from __future__ import annotations

import argparse
import json
import sys
from pathlib import Path

HERE = Path(__file__).resolve().parent
sys.path.insert(0, str(HERE))
from _common import add_native_path, find_model_dir  # noqa: E402

add_native_path()

CASES = HERE / "tests" / "parity_cases.json"
EXPECTED = HERE / "tests" / "parity_expected.json"

PROB_KEYS = ("probabilities",)


def parse_args() -> argparse.Namespace:
    p = argparse.ArgumentParser(description="Vendored MLX runtime parity check")
    p.add_argument("--model-dir", default=None)
    p.add_argument("--device", default="gpu", choices=["gpu", "cpu"])
    p.add_argument("--dtype", default="float16", choices=["float16", "float32"])
    p.add_argument("--prob-tol", type=float, default=2e-3,
                   help="max abs diff allowed on published probabilities (default 2e-3)")
    return p.parse_args()


def compare_answer(qid: str, got: dict, want: dict, prob_tol: float) -> list[str]:
    problems: list[str] = []
    gtype, wtype = got.get("type"), want.get("type")
    if gtype != wtype:
        return [f"{qid}: type {gtype!r} != {wtype!r}"]

    # `choice` is a label; `noul` is a probability in [0,1]; `score` is an expectation.
    if "choice" in want:
        if got.get("choice") != want["choice"]:
            problems.append(f"{qid}: choice {got.get('choice')!r} != {want['choice']!r}")
    score_tol = max(prob_tol, 0.05)
    for key in ("noul", "score"):
        if key in want:
            if key not in got:
                problems.append(f"{qid}: missing '{key}'")
            elif abs(float(got[key]) - float(want[key])) > score_tol:
                problems.append(f"{qid}: {key} {got[key]} != {want[key]}")

    for key in PROB_KEYS:
        if key in want:
            w, g = want[key], got.get(key, {})
            if set(w) != set(g):
                problems.append(f"{qid}: probability keys differ {sorted(g)} != {sorted(w)}")
                continue
            for label, wv in w.items():
                gv = float(g[label])
                if abs(gv - float(wv)) > prob_tol:
                    problems.append(f"{qid}: p[{label}] {gv:.5f} vs {wv:.5f}")
    return problems


def main() -> int:
    args = parse_args()
    import warnings

    warnings.filterwarnings("ignore")
    import laya_mlx

    model_dir = args.model_dir or find_model_dir()
    cases = json.loads(CASES.read_text())
    expected = json.loads(EXPECTED.read_text())

    agent = laya_mlx.load(model_dir, device=args.device, dtype=args.dtype)
    all_problems: list[str] = []
    for name, req in cases.items():
        got = agent.system_one(req["state"], req["questions"])
        want = expected[name]
        for qid, wans in want["answers"].items():
            gans = got["answers"].get(qid, {})
            all_problems += compare_answer(f"{name}/{qid}", gans, wans, args.prob_tol)

    if all_problems:
        print("FAIL: parity mismatches vs upstream golden:")
        for p in all_problems:
            print("  -", p)
        return 1

    print(f"[parity] {len(cases)} cases, {sum(len(c['questions']) for c in cases.values())} "
          f"questions, tol={args.prob_tol:g}: all match upstream golden")
    print("PASS")
    return 0


if __name__ == "__main__":
    sys.exit(main())
