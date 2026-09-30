#!/usr/bin/env python3
"""Side-by-side: offline heuristic backend vs live laya-tch @ /v1/systemone.

Runs the same workflow specs through both backends and reports per-case
(label, confidence, latency).  Useful for:

  * measuring the wire path (does laya-workflow talk to laya-tch correctly?)
  * comparing the deterministic offline heuristic to the real model
  * sanity-checking parity once release artefacts change

Setup::

    # 1. Build laya-tch once (downloads libtorch the first time)
    cargo build --release -p laya-tch

    # 2. Start the model server
    LD_LIBRARY_PATH=$(find target/release/build/torch-sys-* -type d -name lib \\
        | head -1 | sed 's|/lib$||')/lib \\
        target/release/laya-tch -m $HOME/models/convaiinnovations--laya --port 8400 &

    # 3. Run this script
    bench/backend_comparison.py
"""
import argparse
import json
import os
import subprocess
import sys
import time
from pathlib import Path

REPO = Path(__file__).resolve().parents[1]
CLI = Path(os.environ.get("LAYA_TCH_BIN_DIR", REPO / "target/release")) / "laya-workflow"
BASE_URL = os.environ.get("LAYA_BASE_URL", "http://127.0.0.1:8400")


# Spec -> (state, expected_label, brief description).  Each tuple is one decision
# point the laya_mem pipeline must make; the offline heuristic's labels are
# deterministic (the `match_any` needles in dsl/laya_mem/*.json).
CASES = [
    (
        "admission.json",
        {"observation": "Alice planted basil and wants a weekly reminder.", "recent_memories": []},
        "ALLOW",
        "specific, recallable detail (preference)",
    ),
    (
        "admission.json",
        {"observation": "OK thanks", "recent_memories": []},
        "BLOCK",
        "trivial ack ('ok', 'thanks')",
    ),
    (
        "admission.json",
        {"observation": "Mira prefers concise explanations.", "recent_memories": []},
        "ALLOW",
        "preference signal",
    ),
    (
        "admission.json",
        {"observation": "trivial ack noted", "recent_memories": []},
        "BLOCK",
        "trivial phrase ('trivial', 'noted')",
    ),
    (
        "admission.json",
        {"observation": "I was charged twice for March. Please refund the duplicate ASAP.", "recent_memories": []},
        "BLOCK",
        "no obvious trivial needles (no ack words); heuristic blocks by absence",
    ),
]


def _run(spec: str, state: dict, base_url: str | None) -> tuple[str, float, float, str]:
    """Invoke the CLI once and return (label, confidence, ms, raw_action_answer)."""
    cmd = [str(CLI)]
    if base_url:
        cmd += ["--base-url", base_url]
    cmd += ["run", "--spec", f"dsl/laya_mem/{spec}", "--state", json.dumps(state)]
    env = dict(os.environ)
    env.setdefault("LAYA_WORK_DIR", "/tmp/laya_work_dir")
    t0 = time.time()
    proc = subprocess.run(cmd, env=env, capture_output=True, text=True, timeout=60)
    elapsed_ms = (time.time() - t0) * 1000
    if proc.returncode != 0:
        err = (proc.stderr.strip().splitlines() or proc.stdout.strip().splitlines())[-1] if proc.stdout or proc.stderr else "no output"
        return "ERR", 0.0, elapsed_ms, err
    # Output starts with `backend: ...\n`, then JSON.
    body = proc.stdout
    idx = body.find("{")
    if idx < 0:
        return "NO_JSON", 0.0, elapsed_ms, "no JSON body"
    payload = json.loads(body[idx:])
    r = payload.get("result", {})
    return r.get("label", "?"), r.get("confidence", 0.0), elapsed_ms, r.get("action_answer", "?")


def main() -> None:
    p = argparse.ArgumentParser(description=__doc__.splitlines()[1])
    p.add_argument("--base-url", default=BASE_URL, help="laya-tch URL; empty string to skip the live run")
    p.add_argument("--skip-heuristic", action="store_true")
    p.add_argument("--skip-live", action="store_true")
    args = p.parse_args()

    if not CLI.exists():
        sys.exit(f"CLI not found at {CLI}; run `cargo build --release`")

    print(f"laya-workflow CLI: {CLI}")
    print(f"laya-tch base url: {args.base_url or '(none)'}")
    print(f"laya-work_dir: {os.environ.get('LAYA_WORK_DIR', '/tmp/laya_work_dir')}")
    print()

    header = ("case", "expected", "heuristic", "heuristic_ms", "laya-tch", "laya_tch_ms")
    rows = []
    for spec, state, expected, desc in CASES:
        h_label, h_conf, h_ms, h_ans = ("", 0.0, 0.0, "")
        if not args.skip_heuristic:
            h_label, h_conf, h_ms, h_ans = _run(spec, state, base_url=None)
        t_label, t_conf, t_ms, t_ans = ("", 0.0, 0.0, "")
        if args.base_url and not args.skip_live:
            t_label, t_conf, t_ms, t_ans = _run(spec, state, base_url=args.base_url)
        rows.append((desc, expected, h_label, f"{h_ms:.0f}", t_label, f"{t_ms:.0f}"))

    fmt = "{:<55} {:<10} {:<8} {:<10} {:<22} {:<10}"
    print(fmt.format(*header))
    print("-" * 130)
    for row in rows:
        print(fmt.format(*[str(c)[:55] for c in row]))
    print()
    print("Latencies include subprocess + CLI + (live) HTTP roundtrip + one forward pass.")
    print("Note: heuristic latency is *not* a useful baseline — it's a substring scan.")


if __name__ == "__main__":
    main()
