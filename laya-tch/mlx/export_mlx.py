#!/usr/bin/env python3
"""Re-export a Laya checkpoint to the native MLX format (`laya-mlx`).

Wraps the vendored `laya_mlx.convert.convert` so the export step is a first-class
entry point. The upstream source may be a local checkpoint directory (offline)
or a Hugging Face repo id (requires `huggingface_hub`, i.e. network access).

    # from a local checkout of the upstream checkpoint
    python3 laya-tch/mlx/export_mlx.py --source ~/models/convaiinnovations--laya \
        --output ~/models/laya-mlx-local --dtype float16

    # from the Hub (needs huggingface_hub + network)
    python3 laya-tch/mlx/export_mlx.py --source convaiinnovations/laya \
        --output ~/models/laya-mlx-local

Notes:
  * heavy weights are **never** written into Git — point `--output` outside the repo;
  * the destination must not exist (the converter refuses to overwrite);
  * the exported directory is directly consumable by `run_once.py` / `bench_model.py`.

Exit 0 on success.
"""

from __future__ import annotations

import argparse
import sys
from pathlib import Path

HERE = Path(__file__).resolve().parent
sys.path.insert(0, str(HERE))
from _common import add_native_path  # noqa: E402

add_native_path()


def parse_args() -> argparse.Namespace:
    p = argparse.ArgumentParser(description="Export a Laya checkpoint to MLX")
    p.add_argument("--source", required=True,
                   help="upstream local dir or HF repo id (e.g. convaiinnovations/laya)")
    p.add_argument("--output", required=True, help="destination dir (must not exist)")
    p.add_argument("--dtype", default="float16", choices=["float16", "float32", "bfloat16"])
    p.add_argument("--revision", default=None)
    p.add_argument("--subfolder", default=None)
    return p.parse_args()


def main() -> int:
    args = parse_args()
    import warnings

    warnings.filterwarnings("ignore")
    from laya_mlx.convert import convert

    out = Path(args.output).expanduser()
    if out.exists():
        print(f"error: output already exists: {out}", file=sys.stderr)
        return 2

    try:
        path = convert(args.source, out, dtype=args.dtype,
                       revision=args.revision, subfolder=args.subfolder)
    except FileNotFoundError as e:
        print(
            f"error: source checkpoint not found ({e}).\n"
            "  A Hugging Face repo id needs `pip install huggingface-hub` and network; "
            "a local path must contain model.safetensors + rl_agent_config.json + "
            "encoder/config.json + tokenizer/.",
            file=sys.stderr,
        )
        return 2

    print(f"[export_mlx] wrote {path}")
    print(f"[export_mlx] dtype {args.dtype}; run: "
          f"python3 laya-tch/mlx/run_once.py --model-dir {path} "
          f"--request laya-tch/mlx/examples/ref_request.json")
    return 0


if __name__ == "__main__":
    sys.exit(main())
