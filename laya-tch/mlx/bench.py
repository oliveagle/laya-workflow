#!/usr/bin/env python3
"""MLX (Apple Metal GPU) benchmark for the Laya encoder — Phase 1.

Measures, on the Apple GPU via ``mlx.core``:

* linear (matmul) throughput in **GFLOPS** for the three projections of one
  encoder layer (QKV, MLP up/gate, MLP out), and
* the wall-clock latency of a **single encoder layer forward** pass.

Methodology (kept deterministic and reproducible): fixed shapes derived from the
model config, a fixed RNG seed for the synthetic weights, a fixed warmup and a
fixed number of timed iterations. Each timed call is forced to complete with
``mx.eval`` before the clock is read. Reported numbers are the median of the
timed iterations to damp scheduler jitter.

Exits 0 on success. Uses only ``mlx.core``.
"""

from __future__ import annotations

import math
import sys
import time

try:
    import mlx.core as mx
except Exception as exc:  # pragma: no cover - environment dependent
    print(f"FAIL: could not import mlx.core: {exc}", file=sys.stderr)
    sys.exit(2)

# ── Geometry (ModernBERT-large) and benchmark knobs ──────────────────────────
HIDDEN = 1024
QKV = 3 * HIDDEN        # 3072
INTER = 2624            # mlp intermediate size
SEQ_LEN = 512           # tokens per forward
SEED = 0
WARMUP = 10
ITERS = 50
EPS = 1e-5


def select_gpu() -> str:
    if hasattr(mx, "metal") and mx.metal.is_available():
        mx.set_default_device(mx.gpu)
    return str(mx.default_device())


def layer_norm(x: mx.array, weight: mx.array) -> mx.array:
    mean = mx.mean(x, axis=-1, keepdims=True)
    var = mx.var(x, axis=-1, keepdims=True)
    return (x - mean) / mx.sqrt(var + EPS) * weight


def gelu(x: mx.array) -> mx.array:
    return 0.5 * x * (1.0 + mx.erf(x / math.sqrt(2.0)))


def make_weights():
    key = mx.random.key(SEED)
    k1, k2 = mx.random.split(key, 2)
    scale = lambda fan_in: (1.0 / math.sqrt(fan_in))
    w = {
        "norm": mx.ones((HIDDEN,), mx.float16),
        "wqkv": (mx.random.normal((QKV, HIDDEN), key=k1) * scale(HIDDEN)).astype(mx.float16),
        "wi": (mx.random.normal((2 * INTER, HIDDEN), key=k2) * scale(HIDDEN)).astype(mx.float16),
    }
    ki = mx.random.key(SEED + 1)
    w["wo"] = (mx.random.normal((HIDDEN, INTER), key=ki) * scale(INTER)).astype(mx.float16)
    if hasattr(mx, "eval"):
        mx.eval(w["norm"], w["wqkv"], w["wi"], w["wo"])
    return w


def timeit(fn, iters: int = ITERS, warmup: int = WARMUP) -> float:
    """Median seconds per call of ``fn`` (which returns an array to eval)."""
    for _ in range(warmup):
        mx.eval(fn())
    samples = []
    for _ in range(iters):
        t0 = time.perf_counter()
        mx.eval(fn())
        samples.append(time.perf_counter() - t0)
    samples.sort()
    return samples[len(samples) // 2]


def gflops(flops: float, seconds: float) -> float:
    return flops / seconds / 1e9


def main() -> int:
    device = select_gpu()
    print(f"[mlx_bench] device     : {device}")
    print(
        f"[mlx_bench] config     : L={SEQ_LEN} H={HIDDEN} qkv={QKV} "
        f"inter={INTER} seed={SEED} warmup={WARMUP} iters={ITERS}"
    )

    w = make_weights()
    x = (mx.random.normal((SEQ_LEN, HIDDEN), key=mx.random.key(SEED + 2))).astype(mx.float16)

    # ── Linear throughput ────────────────────────────────────────────────────
    linears = [
        ("linear qkv     ", lambda: x @ w["wqkv"].T, 2 * SEQ_LEN * HIDDEN * QKV),
        ("linear mlp_in  ", lambda: x @ w["wi"].T, 2 * SEQ_LEN * HIDDEN * (2 * INTER)),
        (
            "linear mlp_out ",
            lambda: mx.zeros((SEQ_LEN, INTER), mx.float16) @ w["wo"].T,
            2 * SEQ_LEN * INTER * HIDDEN,
        ),
    ]
    total_flops = 0.0
    total_time = 0.0
    for name, fn, flops in linears:
        dt = timeit(fn)
        total_flops += flops
        total_time += dt
        print(
            f"[mlx_bench] {name}: {dt * 1e3:8.3f} ms   {gflops(flops, dt):8.2f} GFLOPS"
        )
    print(
        f"[mlx_bench] linear total : {total_time * 1e3:8.3f} ms   "
        f"{gflops(total_flops, total_time):8.2f} GFLOPS (sum of the three projections)"
    )

    # ── Single encoder-layer forward ─────────────────────────────────────────
    def layer_forward():
        n = layer_norm(x, w["norm"])
        qkv = n @ w["wqkv"].T
        gv = n @ w["wi"].T
        val, gate = gv[..., :INTER], gv[..., INTER:]
        act = gelu(val) * gate
        y = act @ w["wo"].T
        return x + y + qkv[..., :HIDDEN]

    layer_dt = timeit(layer_forward)
    layer_flops = (
        2 * SEQ_LEN * HIDDEN * QKV
        + 2 * SEQ_LEN * HIDDEN * (2 * INTER)
        + 2 * SEQ_LEN * INTER * HIDDEN
    )
    print(
        f"[mlx_bench] encoder layer: {layer_dt * 1e3:8.3f} ms/forward   "
        f"{gflops(layer_flops, layer_dt):8.2f} GFLOPS  (L={SEQ_LEN})"
    )
    print("[mlx_bench] median of per-call wall time; timings from mx.eval-synced calls")
    print("[mlx_bench] DONE")
    return 0


if __name__ == "__main__":
    sys.exit(main())
