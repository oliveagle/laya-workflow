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
| `auto` | Default. CUDA when the linked libtorch reports it available, else CPU. |

`LAYA_TCH_DEVICE` provides the same value when the flag is omitted.

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

# CPU / auto (works for both build flavours)
./target/release/laya-tch --model-dir "$MODEL_DIR" --device auto --port 8400

# Explicitly on GPU — needs a CUDA build plus the runtime lib path
LD_LIBRARY_PATH="$TORCH/lib:$LD_LIBRARY_PATH" \
  ./target/release/laya-tch --model-dir "$MODEL_DIR" --device cuda:0
```

One-shot request (used by the parity harness):

```bash
LD_LIBRARY_PATH="$LIBDIR" \
  ./target/release/laya-tch --model-dir "$MODEL_DIR" \
    --device auto --once laya-tch/bench/ref_request.json
```

Where `$LIBDIR` is the `libtorch/lib` directory of whatever the build linked:
either the download cache under `target/release/build/torch-sys-*/out/libtorch/libtorch/lib`
(CPU build) or `$TORCH/lib` (CUDA build). For a release build installed into
`PATH`, make sure the matching libtorch is on `LD_LIBRARY_PATH`.

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
