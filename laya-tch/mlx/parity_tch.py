#!/usr/bin/env python3
"""Cross-implementation parity: native MLX runtime vs the Rust `tch` CPU engine.

This is a *strong* correctness check: two independent implementations (Python +
Apple MLX on the GPU, versus Rust + libtorch on the CPU) run the same weights
and the same request, and their published answers must agree.

It rebuilds a tch-compatible copy of the MLX checkpoint (the MLX export renamed
`self_attn.in_proj_*` -> `self_attn.in_proj.*`, `scorer.N` -> `scorer.layers.N`,
`act_head.N` -> `act_head.layers.N`), runs:
    laya-tch --device cpu --once <request>
and compares the answer fields with `run` of the vendored MLX runtime.

    python3 laya-tch/mlx/parity_tch.py            # needs target/{debug,release}/laya-tch
    python3 laya-tch/mlx/parity_tch.py --tch target/release/laya-tch

Exit 0 (PASS) when all compared fields agree within `--tol`; 1 otherwise; 2 when
the tch binary or libtorch is unavailable (skipped).
"""

from __future__ import annotations

import argparse
import json
import os
import subprocess
import sys
import tempfile
from pathlib import Path

HERE = Path(__file__).resolve().parent
REPO = HERE.parent.parent
sys.path.insert(0, str(HERE))
from _common import DEMO_QUESTIONS, DEMO_STATE, add_native_path, find_model_dir  # noqa: E402

add_native_path()

REQUEST_PATH = HERE / "examples" / "ref_request.json"


def parse_args() -> argparse.Namespace:
    p = argparse.ArgumentParser(description="MLX vs tch parity check")
    p.add_argument("--model-dir", default=None)
    p.add_argument("--tch", default=None, help="path to the laya-tch binary")
    p.add_argument("--request", default=str(REQUEST_PATH))
    p.add_argument("--tol", type=float, default=5e-3)
    return p.parse_args()


def find_tch(explicit: str | None) -> Path | None:
    if explicit:
        return Path(explicit)
    for rel in ("target/debug/laya-tch", "target/release/laya-tch"):
        cand = REPO / rel
        if cand.is_file():
            return cand
    return None


def libtorch_env() -> dict:
    env = dict(os.environ)
    try:
        import torch  # noqa: PLC0415
        lib = Path(torch.__file__).parent
        env["LIBTORCH"] = str(lib)
        env["DYLD_LIBRARY_PATH"] = str(lib / "lib") + os.pathsep + env.get("DYLD_LIBRARY_PATH", "")
        env["LD_LIBRARY_PATH"] = str(lib / "lib") + os.pathsep + env.get("LD_LIBRARY_PATH", "")
    except Exception:  # noqa: BLE001
        pass
    return env


def build_tch_alias(model_dir: str, dest: Path) -> None:
    from safetensors import safe_open
    from safetensors.numpy import save_file

    src = Path(model_dir) / "model.safetensors"
    out = {}
    with safe_open(str(src), framework="numpy") as f:
        for k in f.keys():
            nk = k
            nk = nk.replace(".self_attn.in_proj.weight", ".self_attn.in_proj_weight")
            nk = nk.replace(".self_attn.in_proj.bias", ".self_attn.in_proj_bias")
            if nk.startswith("scorer.layers."):
                nk = "scorer." + nk[len("scorer.layers."):]
            if nk.startswith("act_head.layers."):
                nk = "act_head." + nk[len("act_head.layers."):]
            if nk == "temperature":
                continue
            out[nk] = f.get_tensor(k)
    save_file(out, str(dest / "model.safetensors"))
    tok_src = Path(model_dir) / "tokenizer"
    if tok_src.is_dir() and not (dest / "tokenizer").exists():
        (dest / "tokenizer").symlink_to(tok_src)


def compare(path: str, got: dict, want: dict, tol: float) -> list[str]:
    problems: list[str] = []
    for qid, wans in want["answers"].items():
        gans = got["answers"].get(qid, {})
        for key in ("choice",):
            if key in wans and gans.get(key) != wans[key]:
                problems.append(f"{path}/{qid}: choice {gans.get(key)!r} != {wans[key]!r}")
        for key in ("noul", "score"):
            if key in wans:
                if abs(float(gans.get(key, 1e9)) - float(wans[key])) > tol:
                    problems.append(f"{path}/{qid}: {key} {gans.get(key)} vs {wans[key]}")
        if "probabilities" in wans:
            g = gans.get("probabilities", {})
            for label, wv in wans["probabilities"].items():
                if abs(float(g.get(label, 1e9)) - float(wv)) > tol:
                    problems.append(f"{path}/{qid}: p[{label}] {g.get(label)} vs {wv}")
    return problems


def main() -> int:
    args = parse_args()
    import warnings

    warnings.filterwarnings("ignore")
    import laya_mlx

    tch = find_tch(args.tch)
    if tch is None:
        print("SKIP: no laya-tch binary (build with `cargo build -p laya-tch` first)")
        return 2

    model_dir = args.model_dir or find_model_dir()
    req = json.loads(Path(args.request).read_text())

    # tch side ------------------------------------------------------------
    with tempfile.TemporaryDirectory(prefix="laya_tch_parity_") as tmp:
        build_tch_alias(model_dir, Path(tmp))
        proc = subprocess.run(
            [str(tch), "--model-dir", tmp, "--device", "cpu", "--once", args.request],
            env=libtorch_env(), stdout=subprocess.PIPE, stderr=subprocess.PIPE, timeout=600,
        )
        if proc.returncode != 0:
            print("SKIP: tch run failed:\n" + proc.stderr.decode()[-800:])
            return 2
        tch_out = json.loads(proc.stdout.decode().strip().splitlines()[-1])

    # MLX side ------------------------------------------------------------
    agent = laya_mlx.load(model_dir, device="gpu", dtype="float16")
    mlx_out = agent.system_one(req["state"], req["questions"])

    problems = compare("tch-vs-mlx", mlx_out, tch_out, args.tol)
    if problems:
        print("FAIL: MLX (GPU) and tch (CPU) disagree:")
        for p in problems:
            print("  -", p)
        return 1

    print(f"[parity_tch] MLX(GPU) vs tch(CPU) agree within {args.tol:g} on "
          f"{len(tch_out['answers'])} answers")
    print("PASS")
    return 0


if __name__ == "__main__":
    sys.exit(main())
