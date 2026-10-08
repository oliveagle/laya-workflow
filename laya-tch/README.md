# laya-tch

Laya inference engine — `tch-rs` (PyTorch C++ bindings) implementation of the
`rl-agent` decision model. Serves `POST /v1/systemone` over HTTP for the
`laya-workflow` crate in this workspace, plus a `--once` mode used by the
parity/bench tooling.

The model weights are **not** part of this repo. Point `--model-dir` at a local
checkout that contains `model.safetensors` and a tokenizer, e.g.
`~/models/convaiinnovations--laya`.

## Compute device (CPU / GPU)

`laya-tch` runs on CPU and GPU. Which backend is available depends on the
**libtorch it was linked against** at build time, and is chosen at run time
with `--device`:

| `--device` | Behaviour |
|---|---|
| `cpu` | Always CPU. Works with any build. |
| `cuda` | First GPU (`cuda:0`). Fails fast with a clear error if the linked libtorch is CPU-only. |
| `cuda:N` | A specific GPU by index, e.g. `cuda:1`. Fails if `N >=` the number of visible devices. |
| `mlx` | Apple MLX (Metal GPU). **macOS only** — on any other platform this is a hard error, not a silent fallback to CPU. |
| `auto` | Default. Prefers `mlx` on Apple Silicon when an MLX runtime is present; but the tch engine can only *serve* `cpu`/`cuda`, so it degrades to CUDA when available, else CPU (with a notice). Elsewhere: CUDA when available, else CPU. |

The resolution rules live in `src/device.rs` (`laya_tch::device::resolve`) and
are covered by `cargo test -p laya-tch` (see the `device::tests` module).

`LAYA_TCH_DEVICE` provides the same value when the flag is omitted.

> On macOS, `auto` does **not** hand the tch engine to MLX: the engine (HTTP
> server / `--once`) runs on libtorch, while MLX compute lives in the native
> Rust `laya-mlx` crate. So `auto` degrades to `cpu` (a notice is printed), and
> passing `--device mlx` explicitly exits with a clear message pointing at
> `laya-mlx`. Use `laya-mlx` for Apple-GPU inference; serve on the tch engine
> with `--device cpu` (or `--device cuda` where available).

## Build

### CPU (default)

Downloads a CPU-only libtorch automatically on first build:

```bash
cargo build --release --locked -p laya-tch
```

### GPU (CUDA)

`tch` picks its libtorch from the `LIBTORCH` environment variable. Point it at
a CUDA-enabled PyTorch install (this machine uses the venv torch):

```bash
TORCH=$HOME/venvs/vllm/lib/python3.12/site-packages/torch

LIBTORCH="$TORCH" LIBTORCH_BYPASS_VERSION_CHECK=1 \
  cargo build --release --locked -p laya-tch
```

`LIBTORCH_BYPASS_VERSION_CHECK=1` skips the version gate when the PyTorch
version differs from the one `tch 0.26` expects (`2.13.0`).

Note: the build is keyed on the env vars — rebuilding without `LIBTORCH`
switches back to the downloaded CPU-only libtorch (and vice versa).

## Run

```bash
MODEL_DIR="$HOME/models/convaiinnovations--laya"

# CPU (works for both build flavours). On macOS use --device cpu (see the note
# above: `auto` prefers mlx, which the tch engine cannot serve).
./target/release/laya-tch --model-dir "$MODEL_DIR" --device cpu --port 8400

# Explicitly on GPU — needs a CUDA build plus the runtime lib path
LD_LIBRARY_PATH="$TORCH/lib:$LD_LIBRARY_PATH" \
  ./target/release/laya-tch --model-dir "$MODEL_DIR" --device cuda:0
```

One-shot request (used by the parity harness):

```bash
LD_LIBRARY_PATH="$LIBDIR" \
  ./target/release/laya-tch --model-dir "$MODEL_DIR" \
    --device cpu --once laya-tch/bench/ref_request.json
```

Where `$LIBDIR` is the `libtorch/lib` directory of whatever the build linked:
either the download cache under `target/release/build/torch-sys-*/out/libtorch/libtorch/lib`
(CPU build) or `$TORCH/lib` (CUDA build). For a release build installed into
`PATH`, make sure the matching libtorch is on `LD_LIBRARY_PATH`.

## MLX (macOS)

On Apple Silicon there is no usable prebuilt libtorch and no CUDA, so the native
high-performance path is **MLX on the Metal GPU**. It is implemented in Rust:

- **[`../laya-mlx/`](../laya-mlx) — the `laya-mlx` crate.** The full model
  (ModernBERT-large encoder + decision head + marker scorer + action head), the
  tokenizer, the prompt builder and the temperature calibration, implemented
  directly on Apple MLX via `mlx-rs`, with no Python anywhere in the loop.
  See its README for build (needs the Xcode Metal toolchain), usage and parity.

`laya-tch/mlx/native/` holds only the attribution for the architecture
  `laya-mlx` was written from. The request and parity fixtures live in the
  `laya-mlx` crate itself (`examples/`, `tests/`).

### Requirements

- macOS on Apple Silicon with Xcode (the Metal toolchain component).
- An FP16 MLX Laya checkpoint. Resolution order (first hit wins):
  1. `$LAYA_MLX_MODEL_DIR`
  2. `$LAYA_MODEL_DIR`
  3. the local Hugging Face cache, `models--*laya-mlx*/snapshots/*/`

  Each candidate must contain `model.safetensors`, `rl_agent_config.json`,
  `encoder/config.json` and `tokenizer/`.

### End-to-end inference

```bash
cd ../laya-mlx && cargo build --release
# one-shot inference (same request/response shape as /v1/systemone)
./target/release/laya-mlx --request ./examples/ref_request.json
# -> {"model": "laya-rl-agent", "answers": {...}, "usage": {...}}
```

Benchmark (median / p90 latency + an f16 GEMM probe):

```bash
./target/release/laya-mlx --request ./examples/ref_request.json --bench 20
```

### Parity and benchmark

```bash
cd ../laya-mlx && cargo test --release   # parity vs the frozen golden (skips without the checkpoint)
LAYA_MLX_TIMING=1 ./target/release/laya-mlx --request ./examples/ref_request.json
```

Measured on the FP16 MLX checkpoint (~590-token requests, Apple GPU vs CPU,
median of 15):

| device | per request |
|---|---|
| CPU | ~560 ms |
| GPU (Metal) | ~175 ms |

i.e. **≈3.2× faster on the GPU than on the CPU** running the identical model.

### `--device mlx` in the Rust CLI

The tch engine (HTTP server / `--once`) is a libtorch build and cannot execute
MLX, so `--device mlx` there exits with a message pointing at the `laya-mlx`
crate; `--device auto` on macOS degrades to CPU/CUDA with a notice. Use
`laya-mlx` for Apple-GPU inference.

## Tests

CPU-only smoke test:

```bash
cargo test -p laya-tch --lib
```

GPU smoke test (needs a CUDA libtorch build and `LD_LIBRARY_PATH`; skipped
automatically on CPU-only builds):

```bash
TORCH=$HOME/venvs/vllm/lib/python3.12/site-packages/torch
LIBTORCH="$TORCH" LIBTORCH_BYPASS_VERSION_CHECK=1 \
LD_LIBRARY_PATH="$TORCH/lib" \
  cargo test -p laya-tch --test cuda_smoke -- --nocapture
```

## Parity / bench

- `cargo run --release -p laya-tch --bin bench_parity` — Rust-side reference probe
  (byte-for-byte output vs the captured PyTorch reference).



All of them honour `LAYA_MODEL_DIR`.
