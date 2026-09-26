#!/usr/bin/env python3
"""Run the Rust-vs-PyTorch numeric parity check for `LayaModel`.

`bench_parity` compares the Rust forward pass against a captured PyTorch
reference (`reference_trace.json` + `reference_probe/`). That is the only check
that can catch a reshape / index / rope / mask / dtype mistake in the model —
unit tests over the surrounding code cannot.

The gate is on the **decision output** (marker logits): `max|Δ| < 1e-3`. The
intermediate-probe numbers are printed for diagnosis but are not the pass/fail
criterion, so a small drift in a hidden state is not a failure by itself.

Usage::

    $PYTHON laya-tch/bench/parity_check.py
    $PYTHON laya-tch/bench/parity_check.py --model-dir /path/to/model
    $PYTHON laya-tch/bench/parity_check.py --json out.json

Exits 0 on PASS, 1 on FAIL, 2 when it cannot run (and says exactly what is
missing rather than silently succeeding).
"""
from __future__ import annotations

import argparse
import json
import os
import re
import subprocess
import sys
from pathlib import Path

CRATE = Path(__file__).resolve().parents[1]
DEFAULT_BIN = CRATE / "target" / "release" / "bench_parity"
DEFAULT_TRACE = CRATE / "bench" / "reference_trace.json"
DEFAULT_PROBE = CRATE / "bench" / "reference_probe"


def trace_model_dir(trace: Path) -> str | None:
    """The trace records which model produced it; prefer that over guessing."""
    try:
        return json.loads(trace.read_text()).get("model_dir")
    except Exception:
        return None


def main() -> int:
    ap = argparse.ArgumentParser()
    ap.add_argument("--bin", default=str(DEFAULT_BIN))
    ap.add_argument("--model-dir", default=None)
    ap.add_argument("--trace", default=str(DEFAULT_TRACE))
    ap.add_argument("--probe", default=str(DEFAULT_PROBE))
    ap.add_argument("--json", default=None, help="also write a JSON result here")
    args = ap.parse_args()

    trace = Path(args.trace)
    if not Path(args.bin).exists():
        print(f"parity: cannot run — {args.bin} is not built. "
              f"Build it with: cargo build --release --bin bench_parity", file=sys.stderr)
        return 2
    if not trace.exists():
        print(f"parity: cannot run — reference trace {trace} missing", file=sys.stderr)
        return 2

    model_dir = args.model_dir or trace_model_dir(trace)
    if not model_dir:
        print("parity: cannot run — no --model-dir and the trace does not name one",
              file=sys.stderr)
        return 2
    if not Path(model_dir, "model.safetensors").exists():
        print(f"parity: cannot run — {model_dir}/model.safetensors missing", file=sys.stderr)
        return 2

    p = subprocess.run(
        [args.bin, model_dir, str(trace), str(args.probe)],
        capture_output=True, text=True,
    )
    out = p.stdout + p.stderr
    print(out, end="" if out.endswith("\n") else "\n")

    passed = p.returncode == 0 and "PASS" in out
    result = {
        "ok": passed,
        "exit_code": p.returncode,
        "model_dir": model_dir,
        "threshold": 1e-3,
    }
    # Pull the headline numbers so callers do not have to re-parse prose.
    m = re.search(r"worst max\|\u0394\| = ([0-9.eE+-]+)", out)
    if m:
        result["worst_logit_delta"] = float(m.group(1))
    m = re.search(r"batched 4 rows: worst max\|\u0394\| \(vs torch batched\) = ([0-9.eE+-]+)", out)
    if m:
        result["worst_batched_delta"] = float(m.group(1))
    m = re.search(r"encoder_hidden\s+max\|\u0394\| = ([0-9.eE+-]+)", out)
    if m:
        result["encoder_hidden_delta"] = float(m.group(1))

    if args.json:
        Path(args.json).write_text(json.dumps(result, indent=2) + "\n")
    print(f"[parity_check] {'PASS' if passed else 'FAIL'}  {json.dumps(result)}")
    return 0 if passed else 1


if __name__ == "__main__":
    sys.exit(main())
