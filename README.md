# laya-workflow

Pure-Rust port of the Laya workflow engine — validate, run, and optimize
decision-graph workflows (DSL v2). This repository is a Cargo **workspace**
containing everything the workflow engine needs:

- `.` — **`laya-workflow`** crate (workflow DSL / engine / CLI)
  - `laya-workflow` — CLI: `validate`, `run`, `list`, `apps`, `describe`,
    `demo`, `optimize`, `improve`, `export`, `secrets`, `skill`.
  - `laya-workflow-tests` — the embedded test harness (492 cases).
- [`laya-tch/`](./laya-tch) — **`laya-tch`** inference engine crate
  (`tch-rs` / PyTorch bindings). Serves the Laya model over
  `POST /v1/systemone` for real decisions; `laya-workflow --base-url`
  points at it. The model weights themselves are **not** part of the repo —
  point `--model-dir` at a local checkout
  (e.g. `~/models/convaiinnovations--laya`).
- [`laya-mlx/`](./laya-mlx) — **`laya-mlx`** native MLX (Apple GPU) inference in
  Rust, via `mlx-rs`. The macOS high-performance path for the decision model
  (see also `laya-tch/mlx/`). Separate crate (its own workspace): `cd laya-mlx &&
  cargo build --release`.

## Install

Download the latest release for your platform (workflow CLI + offline tests):

```bash
# macOS arm64
curl -L https://github.com/oliveagle/laya-workflow/releases/latest/download/laya-workflow-aarch64-apple-darwin.tar.gz | tar -xz
# Linux amd64
curl -L https://github.com/oliveagle/laya-workflow/releases/latest/download/laya-workflow-x86_64-unknown-linux-gnu.tar.gz | tar -xz
sudo mv laya-workflow laya-workflow-tests /usr/local/bin/
```

## Build from source

```bash
# workflow engine only (fast; no libtorch needed)
cargo build --release --locked -p laya-workflow
./target/release/laya-workflow --help
./target/release/laya-workflow-tests

# inference engine too (downloads libtorch on first build; heavy)
cargo build --release --locked -p laya-tch
MODEL_DIR="$HOME/models/convaiinnovations--laya" \
  ./target/release/laya-tch --model-dir "$MODEL_DIR" --port 8400
```

## DSL

Specs live under `dsl/`. See `bench/dsl_smoke.py` for end-to-end smoke tests
(`python3 bench/dsl_smoke.py`).

## License

Dual-licensed: MIT OR Apache-2.0.
