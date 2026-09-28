# laya-workflow

Pure-Rust port of the Laya workflow engine — validate, run, and optimize
decision-graph workflows (DSL v2). This repository is a Cargo **workspace**
containing everything the workflow engine needs:

- `.` — **`laya-workflow`** crate (workflow DSL / engine / CLI)
  - `laya-workflow` — CLI: `validate`, `run`, `list`, `apps`, `describe`,
    `demo`, `optimize`, `improve`, `export`, `secrets`, `skill`.
  - `laya-workflow-tests` — the embedded test harness (500 cases with the local mocks; 445 offline).
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

Download the latest release for your platform. The workflow CLI is the only
runtime binary most users need; `laya-workflow-tests` is an optional offline
test harness.

```bash
# macOS arm64
curl -L https://github.com/oliveagle/laya-workflow/releases/latest/download/laya-workflow-aarch64-apple-darwin.tar.gz | tar -xz
# Linux amd64
curl -L https://github.com/oliveagle/laya-workflow/releases/latest/download/laya-workflow-x86_64-unknown-linux-gnu.tar.gz | tar -xz
sudo install -m 0755 laya-workflow /usr/local/bin/laya-workflow
# Optional repository regression harness:
# sudo install -m 0755 laya-workflow-tests /usr/local/bin/laya-workflow-tests
```

## Build from source

```bash
# workflow engine only (fast; no libtorch needed)
cargo build --release --locked -p laya-workflow
./target/release/laya-workflow --help
# Optional: ./target/release/laya-workflow-tests

# inference engine too (downloads libtorch on first build; heavy)
cargo build --release --locked -p laya-tch
MODEL_DIR="$HOME/models/convaiinnovations--laya" \
  ./target/release/laya-tch --model-dir "$MODEL_DIR" --port 8400
```

## DSL

Specs live under `dsl/`, organised by domain. See `bench/dsl_smoke.py` for
end-to-end smoke tests (`python3 bench/dsl_smoke.py`).

A bare `"workflow": "<name>"` reference — and `laya-workflow list` — resolves
through a **layered** set of spec roots (highest priority first):

1. **explicit** — `--dsl-dir <path>` / `$LAYA_DSL_DIR` *pins* the root and
   replaces the layers below (legacy single-root behaviour).
2. **repo** — `.laya-workflow/dsl/` (preferred) or `dsl/`, found by walking up
   from the cwd and stopping at the git root. Committed with the repo, so each
   repo's topology travels with its code.
3. **user** — `$LAYA_USER_DSL_DIR` → `$XDG_CONFIG_HOME/laya-workflow/dsl` →
   `~/.config/laya-workflow/dsl`. Personal, never committed.
4. **builtin** — `<crate>/dsl`, the specs shipped with the binary.

The first root that defines a name wins; a same-named spec in a lower-priority
root is reported by `list` as `(shadowed by …)`. `laya-workflow list` prints the
search path (low → high) it actually used.

For query-driven specs, `--query` is a concise alternative to JSON state
(`--query` overrides `state.query` when both are given):

```bash
laya-workflow run --spec dsl/browser/google_research.json --query "typesafe ai"
```

Browser research classifies each result URL **before** opening it and skips
video/streaming hosts (YouTube, Bilibili, …) and direct media files by default.
Opt out with `--state '{"skip_video":false}'`, or replace the set with
`--state '{"skip_kinds":[]}'`.

### Singleton Chrome CDP

The `chrome_cdp` capability drives a real Chrome profile with the bundled
MV3 observation extension while guaranteeing one browser/CDP endpoint for all
workflow threads. See `docs/browser_singleton.md`; a runnable localhost example
is `dsl/browser/browser_singleton.json`.

Its `save_article` op renders a JavaScript-heavy page (waiting for the SPA to
settle), converts it to Markdown — headings, lists, tables, code, links and
KaTeX math — and downloads its figures next to the Markdown. The `alphaxiv` op
is a natural-language downloader built on top of it. The bundled
`dsl/browser/alphaxiv_paper.json` infers what you want from a bare `--query`:

```bash
# search alphaXiv and save the top paper
laya-workflow run --spec dsl/browser/alphaxiv_paper.json --query "llm memory"

# save one paper by URL
laya-workflow run --spec dsl/browser/alphaxiv_paper.json \
  --query "https://www.alphaxiv.org/abs/2609.recurrent-looped-transformer"

# download the trending/explore feed (default 10 papers; page up to 500)
laya-workflow run --spec dsl/browser/alphaxiv_paper.json --query "trending"
laya-workflow run --spec dsl/browser/alphaxiv_paper.json --query "trending" \
  --state '{"count":100,"interval":"30 Days"}'
```

Papers are saved in Chinese by default (`lang` = `zh`); opt out with
`--state '{"lang":"en"}'`. Each `<slug>_meta.json` records a UTC
`downloaded_at` timestamp (plus the trending `rank`/`interval`), handy for
monitoring what is trending over time.

## Script plugins (Rhai)

Site- and task-specific logic lives in **Rhai plugins** under `plugins/`, not in
the compiled engine — the Rust base keeps the transport, policy gates, resource
lifecycle and rendering, and a plugin adds the site logic. A plugin can only call
a small, audited host API, so `allow_hosts` / `allow_paths` / `allow_exec` and the
"the engine owns tab cleanup" invariant still hold.

```bash
# fully offline: a workflow calling a local plugin
laya-workflow run --spec dsl/capabilities/script_plugin.json \
  --state '{"text": "the workflow engine runs the workflow"}'
```

Plugins resolve from an explicit `dir`, then `$LAYA_PLUGIN_DIR`, then
`plugins/<name>` walking up to the git root, then
`~/.config/laya-workflow/plugins` (the install default), then the copy compiled
into the binary. `plugins/alphaxiv` (natural-language alphaXiv downloader) and
`plugins/textdigest` (offline demo) ship with the repo. Install a single plugin
out of any git repo (sparse clone — not the whole repo):

```bash
laya-workflow plugin install <owner/repo> --path plugins/alphaxiv
laya-workflow plugin list       # what the engine can see, and from where
```

See [`docs/plugins.md`](./docs/plugins.md) for the host API and the DSL shape, or
run `laya-workflow skill --section plugins` for the embedded guide.

### HuggingFace model monitor

`plugins/hf-trending` is a second, browser-free example: given a phrase (or the
trending feed) it snapshots the current HuggingFace ranking and, per model,
records the rank, likes, downloads, trending score, card metadata and the full
model card — as JSON snapshots, a Markdown report, `cards/` and a `history.jsonl`
time series.

```bash
laya-workflow run --spec dsl/capabilities/hf_trending.json --query trending --state '{"limit":5}'
laya-workflow run --spec dsl/capabilities/hf_trending.json --query "llm memory"
```

See [`docs/hf_trending.md`](./docs/hf_trending.md).

### Browser reader plugins (Chrome/CDP)

Two more examples drive a real Chrome tab over CDP and keep all site logic in
Rhai + page JS, so the engine can stay generic:

`plugins/hackernews` reads Hacker News — a front page (`top`/`best`/`new`/`ask`/
`show`/`jobs`), a full-text search over story titles/URLs, or one discussion with
its comment tree; `plugins/arxiv` searches arXiv papers or reads a single paper's
abstract page into a Markdown digest.

```bash
laya-workflow run --spec dsl/browser/hackernews.json --query top --state '{"count":5}'
laya-workflow run --spec dsl/browser/hackernews.json --query "rust async" --state '{"count":10}'
laya-workflow run --spec dsl/browser/arxiv.json --query "rust async" --state '{"count":3}'
laya-workflow run --spec dsl/browser/arxiv.json --query "1706.03762"
```

See [`docs/hackernews.md`](./docs/hackernews.md) and
[`docs/arxiv.md`](./docs/arxiv.md).

Three more keep the same shape: `plugins/wikipedia` searches articles and reads a
whole article into Markdown (after stripping the site chrome in-page);
`plugins/mdn` searches MDN through its public API and reads one doc through a
tab; `plugins/bing` turns a phrase into Bing organic result rows.

```bash
laya-workflow run --spec dsl/browser/wikipedia.json --query "transformer neural network" --state '{"count":5}'
laya-workflow run --spec dsl/browser/wikipedia.json --query "Rust (programming language)" --state '{"mode":"page"}'
laya-workflow run --spec dsl/browser/mdn.json --query "fetch api" --state '{"count":5}'
laya-workflow run --spec dsl/browser/mdn.json --query "/en-US/docs/Web/API/Fetch_API"
laya-workflow run --spec dsl/browser/bing.json --query "typescript ai framework"
laya-workflow run --spec dsl/browser/v2ex.json --query hot --state '{"count":10}'
```

See [`docs/wikipedia.md`](./docs/wikipedia.md), [`docs/mdn.md`](./docs/mdn.md),
[`docs/bing.md`](./docs/bing.md) and [`docs/v2ex.md`](./docs/v2ex.md).

## License

Dual-licensed: MIT OR Apache-2.0.
