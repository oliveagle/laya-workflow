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
> server / `--once`) runs on libtorch, and Phase 1's MLX path is the
> self-contained Python runtime under `laya-tch/mlx/`. So `auto` degrades to
> `cpu` (a notice is printed), and passing `--device mlx` explicitly exits with a
> clear message pointing at the MLX scripts. Use the MLX scripts below for
> Apple-GPU inference; serve on the tch engine with `--device cpu` (or
> `--device cuda` where available).

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
high-performance path is **MLX on the Metal GPU**. There are two implementations:

- **[`../laya-mlx/`](../laya-mlx) — the Rust implementation (primary).** The full
  model implemented directly on Apple MLX via `mlx-rs`; no Python in the loop.
  See its README for build (needs the Xcode Metal toolchain) and usage.
- `laya-tch/mlx/` — the earlier **Python** runtime (reference / legacy, used to
  generate the frozen parity golden). It also holds the shared request fixtures
  and the low-level op smoke/bench.

`laya-tch/mlx/` layout:

- `native/laya_mlx/` — the complete decision model in MLX (`ModernBert` encoder +
  decision head + scorer + act head), the prompt builder/tokenizer and the
  calibration logic. Vendored from [`mizorewww/laya-mlx`](https://github.com/mizorewww/laya-mlx)
  (Apache-2.0; see `native/laya_mlx/NOTICE` and `LICENSE`).
- `run_once.py` — end-to-end inference (`state` + `questions` → `answers`).
- `bench_model.py` — whole-request latency and Apple-GPU-vs-CPU speedup.
- `parity.py` / `parity_tch.py` — output parity (golden / tch CPU).
- `export_mlx.py` — re-export an upstream Laya checkpoint to the MLX format.
- `mlx_smoke.py` / `bench.py` — single-layer op-level smoke/bench.

### Requirements

- macOS on Apple Silicon.
- Python 3 with the `mlx` package (`python3 -m pip install mlx`), `numpy` and
  `tokenizers`. Verify with
  `python3 -c "import mlx.core as mx; print(mx.default_device())"` — it should
  print a `gpu` device when Metal is available.
- An FP16 MLX Laya checkpoint. Resolution order (first hit wins):
  1. `$LAYA_MLX_MODEL_DIR`
  2. `$LAYA_MODEL_DIR`
  3. the local Hugging Face cache, `models--*laya-mlx*/snapshots/*/`

  Each candidate must contain `model.safetensors`, `rl_agent_config.json`,
  `encoder/config.json` and `tokenizer/`. If none is found the scripts exit
  non-zero.

### End-to-end inference

```bash
python3 laya-tch/mlx/run_once.py --request laya-tch/mlx/examples/ref_request.json
# -> {"model": "laya-rl-agent", "answers": {...}, "usage": {...}}
```

`--device gpu|cpu`, `--dtype float16|float32|bfloat16`, `--compile` and
`--json` are supported. The vendored runtime is also importable:

```python
import sys; sys.path.insert(0, "laya-tch/mlx/native")
import laya_mlx
agent = laya_mlx.load("$LAYA_MLX_MODEL_DIR", device="gpu")
agent.system_one(state, questions)
```

### Parity, benchmark, export

```bash
python3 laya-tch/mlx/parity.py        # PASS: matches the upstream golden
python3 laya-tch/mlx/parity_tch.py    # PASS: MLX(GPU) == tch CPU engine (independent impls)
python3 laya-tch/mlx/bench_model.py   # whole-model CPU vs GPU + batch scaling
python3 laya-tch/mlx/export_mlx.py --source <upstream-dir-or-hf-id> --output <dir>
```

Measured on the FP16 MLX checkpoint (`laya-tch/mlx/bench_model.py`, ~590-token
requests, Apple GPU vs CPU, median of 15):

| device | per request |
|---|---|
| CPU | ~560 ms |
| GPU (Metal) | ~175 ms |

i.e. **≈3.2× faster on the GPU than on the CPU** running the identical model.
The cost is GEMM-bound at roughly 2–3 TFLOP/s fp16 (see `bench.py`), so the
whole-model number is consistent with the single-layer rate — batching amortizes
launch overhead (≈56 → ≈50 ms/question from 1 → 16 questions).

### `--device mlx` in the Rust CLI

The tch engine (HTTP server / `--once`) is a libtorch build and cannot execute
MLX, so `--device mlx` there exits with a message pointing at the scripts above;
`--device auto` on macOS degrades to CPU/CUDA with a notice. Use the MLX scripts
for Apple-GPU inference.

### Single-layer smoke (op-level)

```bash
python3 laya-tch/mlx/mlx_smoke.py   # one encoder layer vs fp32 NumPy reference
python3 laya-tch/mlx/bench.py       # per-op GFLOPS / per-layer latency
```

`mlx_smoke.py` prints a `parity` line (max abs diff, violations) and ends with
`PASS`.


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

- `bench/api_parity.py`, `bench/parity_check.py` — byte-for-byte output vs the
  PyTorch reference.
- `bench/bench_pytorch.py`, `bench/compare_perf.py` — performance comparison.
- `bench_parity` bin — Rust-side reference probe.

All of them honour `LAYA_MODEL_DIR`.
