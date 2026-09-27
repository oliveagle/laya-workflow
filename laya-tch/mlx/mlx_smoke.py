#!/usr/bin/env python3
"""MLX (Apple Metal GPU) smoke test for the Laya encoder — Phase 1.

Locates an FP16 MLX Laya checkpoint, loads **one real encoder layer**
(`encoder.layers.<LAYER>`), runs a real forward pass on the Apple GPU and checks
the result against an independent float32 NumPy reference of the same weights.

The forward covers the operations named in the Phase 1 scope:

    LayerNorm -> Linear (QKV) -> Linear (MLP up/gate) -> GeGLU -> Linear (MLP out)
    with a residual connection.

Checkpoint resolution order (first hit wins):

    1. ``$LAYA_MLX_MODEL_DIR``   (explicit, must contain ``model.safetensors``)
    2. ``$LAYA_MODEL_DIR``       (same expectation)
    3. the local Hugging Face cache: ``models--*laya-mlx*/snapshots/*``

If none contains ``model.safetensors`` the script prints ``FAIL`` and exits
non-zero.

On success it prints ``PASS`` and exits 0. Only ``mlx.core`` + NumPy are used.
"""

from __future__ import annotations

import glob
import math
import os
import sys

import numpy as np

try:
    import mlx.core as mx
except Exception as exc:  # pragma: no cover - environment dependent
    print(f"FAIL: could not import mlx.core: {exc}", file=sys.stderr)
    sys.exit(2)

# ── Model geometry (ModernBERT-large, from encoder/config.json) ──────────────
HIDDEN = 1024
INTER = 2624          # mlp intermediate size
QKV = 3 * HIDDEN      # Wqkv output = 3072
EPS = 1e-5            # layer_norm_eps / norm_eps
LAYER = 0             # encoder layer to exercise
SEQ_LEN = 64          # tokens in the synthetic batch
SEED = 0

# ── Parity tolerances (FP16 GPU vs FP32 reference) ──────────────────────────
RTOL = 2e-2
ATOL = 2e-2

LAYER_PREFIX = f"encoder.layers.{LAYER}."
REQUIRED = (
    "mlp_norm.weight",
    "attn.Wqkv.weight",
    "mlp.Wi.weight",
    "mlp.Wo.weight",
)


def fail(msg: str) -> "None":
    print(f"FAIL: {msg}", file=sys.stderr)
    sys.exit(1)


def _usable(path: str) -> bool:
    return os.path.isfile(os.path.join(path, "model.safetensors"))


def find_model_dir() -> str:
    """Return a directory that holds ``model.safetensors`` (or fail)."""
    for var in ("LAYA_MLX_MODEL_DIR", "LAYA_MODEL_DIR"):
        val = os.environ.get(var)
        if val:
            cand = os.path.expanduser(val)
            if not _usable(cand):
                fail(
                    f"${var} is set to {cand!r} but it has no model.safetensors"
                )
            return cand

    hf_home = os.environ.get("HF_HOME", os.path.join("~", ".cache", "huggingface"))
    hub = os.path.join(os.path.expanduser(hf_home), "hub")
    pattern = os.path.join(hub, "models--*laya-mlx*", "snapshots", "*")
    for cand in sorted(glob.glob(pattern)):
        if _usable(cand):
            return cand

    fail(
        "no MLX Laya checkpoint found. Set $LAYA_MLX_MODEL_DIR (or "
        "$LAYA_MODEL_DIR) to a directory containing model.safetensors, or "
        f"place one under {hub}/models--*laya-mlx*/snapshots/*/"
    )
    raise SystemExit(1)  # unreachable, keeps type checkers happy


def select_gpu() -> str:
    """Force the Apple GPU when Metal is present; report the device in use."""
    if hasattr(mx, "metal") and mx.metal.is_available():
        mx.set_default_device(mx.gpu)
    dev = mx.default_device()
    return str(dev)


def layer_norm(x: mx.array, weight: mx.array) -> mx.array:
    mean = mx.mean(x, axis=-1, keepdims=True)
    var = mx.var(x, axis=-1, keepdims=True)
    return (x - mean) / mx.sqrt(var + EPS) * weight


def gelu(x: mx.array) -> mx.array:
    """Exact (erf-based) GELU, matching torch's ``gelu(approximate='none')``."""
    return 0.5 * x * (1.0 + mx.erf(x / math.sqrt(2.0)))


def layer_forward(x: mx.array, w: dict) -> tuple:
    """One encoder layer: LayerNorm + QKV linear + GeGLU MLP + residual."""
    n = layer_norm(x, w["mlp_norm.weight"])
    qkv = n @ w["attn.Wqkv.weight"].T                       # [L, 3H]
    gv = n @ w["mlp.Wi.weight"].T                           # [L, 2*INTER]
    val, gate = gv[..., :INTER], gv[..., INTER:]
    act = gelu(val) * gate                                   # GeGLU
    y = act @ w["mlp.Wo.weight"].T                          # [L, H]
    return x + y, qkv


# ── float32 NumPy reference ─────────────────────────────────────────────────
_erf = np.frompyfunc(math.erf, 1, 1)


def np_layer_norm(x: np.ndarray, w: np.ndarray) -> np.ndarray:
    mean = x.mean(-1, keepdims=True)
    var = x.var(-1, keepdims=True)
    return (x - mean) / np.sqrt(var + EPS) * w


def np_gelu(x: np.ndarray) -> np.ndarray:
    return (0.5 * x * (1.0 + _erf(x / math.sqrt(2.0)))).astype(np.float64)


def np_layer_forward(x: np.ndarray, w: dict) -> np.ndarray:
    n = np_layer_norm(x, w["mlp_norm.weight"])
    gv = n @ w["mlp.Wi.weight"].T
    val, gate = gv[..., :INTER], gv[..., INTER:]
    act = np_gelu(val) * gate
    y = (act @ w["mlp.Wo.weight"].T).astype(np.float32)
    return x + y


def main() -> int:
    model_dir = find_model_dir()
    weights_path = os.path.join(model_dir, "model.safetensors")
    print(f"[mlx_smoke] checkpoint : {weights_path}")

    device = select_gpu()
    print(f"[mlx_smoke] device     : {device}")

    all_tensors = mx.load(weights_path)
    layer = {k[len(LAYER_PREFIX):]: v for k, v in all_tensors.items() if k.startswith(LAYER_PREFIX)}
    missing = [k for k in REQUIRED if k not in layer]
    if missing:
        fail(f"layer {LAYER} is missing tensors {missing} in {weights_path}")
    print(f"[mlx_smoke] layer      : {LAYER_PREFIX} ({len(layer)} tensors)")

    # Deterministic synthetic activations entering the layer.
    rng = np.random.default_rng(SEED)
    x_np = (rng.standard_normal((SEQ_LEN, HIDDEN), dtype=np.float32) * 0.5)
    x = mx.array(x_np).astype(layer["mlp_norm.weight"].dtype)

    out, qkv = layer_forward(x, layer)
    mx.eval(out, qkv)
    got = np.array(out.astype(mx.float32))

    finite = bool(np.isfinite(got).all()) and bool(mx.all(mx.isfinite(qkv)).item())
    if not finite:
        fail("forward produced non-finite values (NaN/Inf)")
    print(
        f"[mlx_smoke] forward    : L={SEQ_LEN} H={HIDDEN} "
        f"qkv={tuple(qkv.shape)} out={tuple(got.shape)} finite=yes"
    )

    # float32 NumPy reference over the same fp16 weights.
    ref_w = {k: np.array(v.astype(mx.float32)) for k, v in layer.items()}
    ref = np_layer_forward(x_np, ref_w)

    diff = np.abs(got - ref)
    tol = ATOL + RTOL * np.abs(ref)
    bad = int((diff > tol).sum())
    max_abs = float(diff.max())
    scale = float(np.abs(ref).max())
    rel = max_abs / scale if scale else 0.0
    print(
        f"[mlx_smoke] parity     : max_abs_diff={max_abs:.6g} "
        f"rel={rel:.3e} tol=(atol={ATOL:g}, rtol={RTOL:g}) violations={bad}"
    )
    if bad:
        fail(f"{bad} element(s) exceed tolerance (max_abs_diff={max_abs:.6g})")

    print(
        f"[mlx_smoke] OK         : {SEQ_LEN}x{HIDDEN} fp16 forward on {device} "
        f"matches fp32 reference within tolerance"
    )
    print("PASS")
    return 0


if __name__ == "__main__":
    sys.exit(main())
