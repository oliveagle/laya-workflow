# laya-workflow

Pure-Rust port of the Laya workflow engine — validate, run, and optimize
decision-graph workflows (DSL v2). This crate ships two binaries:

- `laya-workflow` — CLI: `validate`, `run`, `list`, `apps`, `describe`, `demo`,
  `optimize`, `improve`, `export`, `secrets`, `skill`.
- `laya-workflow-tests` — the embedded test harness (426 cases).

## Install

Download the latest release for your platform:

```bash
# macOS arm64
curl -L https://github.com/oliveagle/laya-workflow/releases/latest/download/laya-workflow-aarch64-apple-darwin.tar.gz | tar -xz
# Linux amd64
curl -L https://github.com/oliveagle/laya-workflow/releases/latest/download/laya-workflow-x86_64-unknown-linux-gnu.tar.gz | tar -xz
sudo mv laya-workflow laya-workflow-tests /usr/local/bin/
```

## Build from source

```bash
cargo build --release
./target/release/laya-workflow --help
./target/release/laya-workflow-tests
```

## DSL

Specs live under `dsl/`. See `bench/dsl_smoke.py` for end-to-end smoke tests
(`python3 bench/dsl_smoke.py`).

## License

Dual-licensed: MIT OR Apache-2.0.
