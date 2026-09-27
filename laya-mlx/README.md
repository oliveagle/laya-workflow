# laya-mlx

Native **Rust** MLX inference for the Laya decision model — a direct
implementation on Apple MLX via [`mlx-rs`](https://crates.io/crates/mlx-rs).

This is the Rust counterpart of the Python runtime under `../laya-tch/mlx/`:
the full **ModernBERT-large encoder + 2-layer decision head + marker scorer +
action head**, the tokenizer, the prompt builder and the temperature
calibration, all running on the **Metal GPU** with no Python in the loop.

## Requirements

- macOS on Apple Silicon with Xcode (the **Metal toolchain** component must be
  installed — Xcode 26 split it out):

  ```bash
  xcodebuild -downloadComponent MetalToolchain   # ~700 MB, once
  ```

- A Rust toolchain (MSRV 1.88 for `mlx-rs`).

`mlx-rs` builds MLX itself from source the first time (~4 minutes); subsequent
builds reuse the cache. The crate opts out of the repo workspace, so the heavy
MLX build never affects `cargo build` for `laya-workflow` / `laya-tch`.

## Build

```bash
cd laya-mlx
cargo build --release
```

## Run

```bash
# one-shot inference (same request/response shape as /v1/systemone)
./target/release/laya-mlx --request ../laya-tch/mlx/examples/ref_request.json

# benchmark (median / p90 latency + an f16 GEMM probe)
./target/release/laya-mlx --request ../laya-tch/mlx/examples/ref_request.json --bench 20
```

The checkpoint is resolved from `$LAYA_MLX_MODEL_DIR`, then `$LAYA_MODEL_DIR`,
then the local Hugging Face cache (`models--*laya-mlx*/snapshots/*`); or pass
`--model-dir DIR`.

## Correctness

`tests/parity.rs` compares whole-request outputs against a frozen golden
produced by the upstream Python package (`mizorewww/laya-mlx` 0.2.0) on the same
FP16 checkpoint. On the four golden cases (choice / score / noul):

```
parity test: ok — no discrete mismatches, max probability diff ≤ 0.001
```

Run it with `cargo test --release` (needs the checkpoint; skips otherwise).

## Performance

Measured on the FP16 MLX checkpoint, three-question request (~208 tokens),
median of 25, Apple M4 GPU:

| runtime | median per request |
|---|---|
| Python (`laya-tch/mlx`, MLX GPU) | ~71 ms |
| **Rust (`laya-mlx`, MLX GPU)** | **~71 ms** |

The Rust and Python runtimes are on par (ratio ≈ 1.0×).

> **Root cause of the earlier 1.6× regression.** The first Rust port ran at
> ~110 ms because `mlx_rs::nn::gelu` funnels through `mlx_gelu`, which returns
> **float32** even for a float16 input. The first MLP silently upcast its
> activation, and from there the *entire* graph (matmuls, adds, the next layers)
> ran in f32 — roughly doubling the runtime. `laya-mlx::model::gelu` now
> reproduces the reference formula (`x*(1+erf(x/sqrt2))/2`) with correctly-typed
> constants, so the model stays float16. `model::tests::gelu_preserves_dtype`
> guards against a regression.

`LAYA_MLX_TIMING=1` prints a per-request `build=` / `eval=` split (graph
construction vs GPU execution) when profiling.

## Attribution

The model architecture and prompt/calibration semantics are those of Laya
(Convai Innovations) and the `laya-mlx` project. See
`../laya-tch/mlx/native/laya_mlx/NOTICE`.
