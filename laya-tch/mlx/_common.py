#!/usr/bin/env python3
"""Shared helpers for the `laya-tch/mlx/` scripts.

Locates the FP16 MLX checkpoint and puts the vendored native MLX runtime on
`sys.path`. Kept stdlib-only so every script can import it cheaply.
"""

from __future__ import annotations

import glob
import os
import sys
from pathlib import Path

NATIVE_DIR = Path(__file__).resolve().parent / "native"


def add_native_path() -> None:
    """Make `import laya_mlx` resolve to the vendored runtime under `native/`."""
    p = str(NATIVE_DIR)
    if p not in sys.path:
        sys.path.insert(0, p)


def find_model_dir(*, required: bool = True) -> str | None:
    """Return a directory holding a Laya MLX checkpoint (`model.safetensors`).

    Resolution order (first hit wins):
      1. ``$LAYA_MLX_MODEL_DIR``
      2. ``$LAYA_MODEL_DIR``
      3. the local Hugging Face cache: ``models--*laya-mlx*/snapshots/*``
    """
    for var in ("LAYA_MLX_MODEL_DIR", "LAYA_MODEL_DIR"):
        val = os.environ.get(var)
        if val:
            cand = os.path.expanduser(val)
            if _usable(cand):
                return cand
            if required:
                raise SystemExit(
                    f"${var} is set to {cand!r} but it has no model.safetensors"
                )
            return None

    hf_home = os.environ.get("HF_HOME", os.path.join("~", ".cache", "huggingface"))
    hub = os.path.join(os.path.expanduser(hf_home), "hub")
    for cand in sorted(glob.glob(os.path.join(hub, "models--*laya-mlx*", "snapshots", "*"))):
        if _usable(cand):
            return cand

    if required:
        raise SystemExit(
            "no MLX Laya checkpoint found. Set $LAYA_MLX_MODEL_DIR (or $LAYA_MODEL_DIR) "
            f"to a directory containing model.safetensors, or place one under "
            f"{hub}/models--*laya-mlx*/snapshots/*/"
        )
    return None


def _usable(path: str) -> bool:
    return os.path.isfile(os.path.join(path, "model.safetensors"))


# ── a small, deterministic request used across the scripts ─────────────────────
DEMO_STATE = (
    "Customer writes: I was billed twice this month for the same subscription and "
    "I need the duplicate refunded today. Account email a@b.com; invoices INV-1, INV-2."
)
DEMO_QUESTIONS = {
    "department": {
        "type": "choice",
        "instructions": "Which department should handle this request?",
        "criteria": ["billing", "technical", "sales"],
    },
    "refund": {
        "type": "noul",
        "instructions": "Does the customer ask for money back?",
    },
    "urgency": {
        "type": "score",
        "instructions": "How urgent is this request?",
        "criteria": ["low", "normal", "high"],
    },
}
