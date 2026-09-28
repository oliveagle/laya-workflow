# run — execute a spec end-to-end

`laya-workflow run --spec <file> --state '<json>'`

* `--state` is the initial workflow state (default `{}`).
* `--query '<text>'` is shorthand for putting `query` into that state. If both
  flags are present, `--query` overrides `state.query`.
* Without `--base-url` it uses the **offline heuristic backend** — topology and
  routing logic run, but decisions are canned heuristics, so use it for plumbing
  checks, not for real accuracy.
* With `--base-url http://127.0.0.1:8400` it drives the live `laya-tch` server.

The output is the full `WorkflowOutcome` as JSON: `result` (final state),
`trace` (steps, final_action, latency), `history`, `node_visits`, `iterations`.

## Example

```
laya-workflow run --spec /tmp/persist_demo2.json --state '{"x":"A"}'
# equivalent shorthand when the spec reads state.query
laya-workflow run --spec dsl/browser/google_research.json --query "jev ai"
```

Browser research (`dsl/browser/google_research.json`) classifies every candidate
URL **before** opening it and skips video/streaming hosts (YouTube, Bilibili, …)
and direct media files by default. Opt out with `--state '{"skip_video":false}'`, or
replace the set with `--state '{"skip_kinds":[]}'`. Skipped rows appear in the
result as `skipped`/`skipped_found`, not silently dropped.

`dsl/browser/alphaxiv_paper.json` is a natural-language alphaXiv downloader
driven by the Rhai plugin `plugins/alphaxiv` (`kind: "plugin"`) — a bare `--query`
decides the mode:

```
laya-workflow run --spec dsl/browser/alphaxiv_paper.json --query "llm memory"   # search, save the top paper
laya-workflow run --spec dsl/browser/alphaxiv_paper.json --query "trending"     # trending feed (10 papers)
laya-workflow run --spec dsl/browser/alphaxiv_paper.json --query "trending" \
  --state '{"count":100,"interval":"30 Days"}'                                    # page the feed (up to 500)
laya-workflow run --spec dsl/browser/alphaxiv_paper.json --query "<paper URL>"  # save that paper
```

The site logic (mode inference, locale URLs, feed paging) lives in
`plugins/alphaxiv/main.rhai`, so it can change without rebuilding the engine; the
same plugin can be called from your own spec as
`{"kind": "plugin", "plugin": "alphaxiv", "browser": "chrome"}`. See
`skill --section plugins` (write/install your own), `skill --section dsl`, and
`docs/plugins.md`.

Each paper is written under `~/tmp/alphaxiv` as Markdown plus its figures (in a
per-paper `images/<slug>/` folder).
Papers are fetched in Chinese by default; opt out with `--state '{"lang":"en"}'`.
Each `<slug>_meta.json` carries a UTC `downloaded_at`, the same instant as
`downloaded_at_local` (rendered in the runner's zone, e.g. `+08:00`) and as
`downloaded_at_unix_ms`, plus trending `rank`/`interval`, so runs can be
monitored over time.

## When to use run vs resume

* Fresh start → `run`.
* Resume a killed/long run from its last committed node → `resume --dir D`
  (see `skill --section resume`).

## What if the result looks wrong?

Do **not** re-run blindly. Use `skill --recipe debug-failure` — it walks you
through checking the trace, then `state --json` to see per-node state snapshots.

Next: `skill --section state`, `skill --section safety`.
