# HuggingFace model monitor (`plugins/hf-trending` + `dsl/capabilities/hf_trending.json`)

A plugin-driven workflow that snapshots the **current HuggingFace model ranking**
and, for every model, records the data you would otherwise click through the UI
for:

* **rank** in *this query* (1 = top), plus a **rank delta** vs the previous run;
* **likes**, **downloads**, **trending score**;
* card metadata: `tags`, `pipeline_tag`, `library_name`, **license**, file count,
  `createdAt` / `lastModified`, `gated`, `private`;
* the **full model card** (`README.md`) saved to disk.

It needs no browser: it calls the public models API and the `raw` card endpoint
over plain HTTP, so the only host it touches is `huggingface.co`.

The Rust engine stays the base (transport, policy, path gate); the *site* logic —
how a phrase maps to a mode, which fields matter, how the ranking is written out —
lives entirely in `plugins/hf-trending/main.rhai`.

## Run it

```bash
# trending feed (bare query, or "trending" / "热门" / "最新" / "排行")
laya-workflow run --spec dsl/capabilities/hf_trending.json --query trending

# a search phrase → search mode
laya-workflow run --spec dsl/capabilities/hf_trending.json --query "llm memory" --state '{"limit":5}'

# filter / author / pipeline + a custom sort, no cards
laya-workflow run --spec dsl/capabilities/hf_trending.json --state '{
  "query":"qwen","author":"Qwen","limit":10,"fetch_cards":false}'
```

State knobs (all optional): `query`, `mode` (`auto`|`trending`|`search`),
`limit` (1–100, default 10), `sort` (default `trendingScore`), `direction`
(default `-1`), `filter`, `author`, `pipeline_tag`, `out_dir`
(default `~/tmp/hf-trending`), `fetch_cards` (default `true`).

`mode: auto` (the default) treats an empty / trending-word query as the trending
feed and anything else as a search.

## What lands on disk

Everything is written under `out_dir` (`~/tmp/hf-trending` by default), and every
path is checked against the spec's `policy.allow_paths` before a byte is written:

```
~/tmp/hf-trending/
├── snapshot_<key>_<unix_ms>.json   full structured snapshot for this run
├── latest_<key>.json               the same, overwritten each run (rank-delta source)
├── report_<key>_<unix_ms>.md       a Markdown ranking table
├── history.jsonl                   one compact line per run (time series)
└── cards/<author>_<name>.md        each model's README (the model card)
```

`<key>` is `trending` or `search_<slug>`, so different queries keep their own
`latest_*` baseline and their own rank deltas.

## Per-model record

```jsonc
{
  "rank": 1, "id": "convaiinnovations/laya", "author": "convaiinnovations",
  "likes": 4174, "downloads": 0, "trending_score": 2652,
  "pipeline_tag": "text-classification", "library_name": "transformers",
  "license": "apache-2.0", "tags": ["transformers", "...", "license:apache-2.0"],
  "created_at": "2026-09-18T05:05:55.000Z", "last_modified": "2026-09-24T05:39:22.000Z",
  "gated": false, "private": false, "files": 38,
  "url": "https://huggingface.co/convaiinnovations/laya",
  "card_url": "https://huggingface.co/convaiinnovations/laya/raw/main/README.md",
  "card_path": "~/tmp/hf-trending/cards/convaiinnovations_laya.md", "card_bytes": 23187,
  "card_error": "", "is_new": true, "rank_delta": null
}
```

`rank_delta` is `prev_rank - rank` (positive = moved up); `is_new: true` means the
model was not in the previous snapshot for this key. A model with no card at all
(HTTP 404) is still recorded, with `card_error` set — one missing card never sinks
the run.

## Monitoring over time

Each run appends one line to `history.jsonl`:

```jsonc
{"fetched_at":"2026-09-28T08:16:57.609Z","mode":"trending","query":"trending",
 "limit":5,"count":5,"ranks":{"convaiinnovations/laya":1,"Qwen/Qwen-Image-2.1":3}}
```

Schedule it and the deltas turn into a change feed, e.g. with the `ole-cron`
skill: run `laya-workflow run --spec dsl/capabilities/hf_trending.json --query
trending --state '{"limit":20}'` every few hours and diff `latest_trending.json`.

## Policy

The spec declares:

```jsonc
"policy": {
  "allow_exec": false,
  "allow_hosts": ["huggingface.co"],
  "allow_paths": ["${env.HOME}/tmp/hf-trending"],
  "max_timeout_ms": 120000, "max_output": 16777216, "retries": 0
}
```

The plugin can only reach the outside world through the audited host API, so
`allow_hosts` gates the API calls and `allow_paths` gates every file write (the
host's `write_file` / `read_file` both go through the same fail-closed path check
as `save_article`).
