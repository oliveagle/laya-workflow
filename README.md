# laya-workflow

Pure-Rust port of the Laya workflow engine — validate, run, and optimize
decision-graph workflows (DSL v2). This repository is a Cargo **workspace**
containing everything the workflow engine needs:

- `.` — **`laya-workflow`** crate (workflow DSL / engine / CLI)
  - `laya-workflow` — CLI: `validate`, `run`, `list`, `apps`, `describe`,
    `demo`, `optimize`, `improve`, `export`, `secrets`, `skill`.
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
runtime binary most users need.

```bash
# macOS arm64
curl -L https://github.com/oliveagle/laya-workflow/releases/latest/download/laya-workflow-aarch64-apple-darwin.tar.gz | tar -xz
# Linux amd64
curl -L https://github.com/oliveagle/laya-workflow/releases/latest/download/laya-workflow-x86_64-unknown-linux-gnu.tar.gz | tar -xz
sudo install -m 0755 laya-workflow /usr/local/bin/laya-workflow

# lay down the state root + every bundled plugin + the laya-mem specs
laya-workflow install
```

## Where it keeps its files

One root, `~/.laya-workflow` (override with `$LAYA_HOME`):

```text
~/.laya-workflow/
  dsl/          per-user specs          (LAYA_USER_DSL_DIR)
  plugins/      installed plugins       (LAYA_USER_PLUGIN_DIR, `plugin install`)
  websites/     site-scoped plugins, sibling of plugins/
  laya-mem/     memory gate: specs/ + codex.sqlite
  chrome/       Chrome profile for chrome_cdp
```

`laya-workflow install` creates all of it and installs every bundled plugin, so
nothing needs a checkout afterwards. `laya-mem info` reports where the memory
gate's specs and store resolved — the failure that matters is a server that
starts and then answers every call with `spec not found`.

## Build from source

```bash
# workflow engine only (fast; no libtorch needed)
cargo build --release --locked -p laya-workflow
./target/release/laya-workflow --help

# inference engine too (downloads libtorch on first build; heavy)
cargo build --release --locked -p laya-tch
MODEL_DIR="$HOME/models/convaiinnovations--laya" \
  ./target/release/laya-tch --model-dir "$MODEL_DIR" --port 8400
```

## Verify before you push

```bash
scripts/verify.sh            # build + plugin gate + fast suite   ~1s warm
scripts/verify.sh --full     # ... and the timeout section        +170s
```

`laya-workflow-tests` has 22 sections. Twenty-one of them finish in **0.16s**;
`capability-timeouts` alone takes **168.9s**, because it waits on real timeouts.
Running the whole binary by reflex turns a sub-second question into a
three-minute one, which is long enough that you stop doing it before every
change. `verify.sh` runs the fast twenty-one by default, always runs the plugin
gate, and prints what it skipped so the cost is visible rather than surprising.

The plugin gate compiles **all 14** `websites/*/plugin/main.rhai`, not a sample:
the Rust suite executes none of them, so a gate that compiled one would report
a broken taobao plugin as green. It also runs `scripts/rhai/check.sh`, which
executes goofish to assert routing and the price-model fold — behaviour, not
syntax, which is why it stays a separate step.

There is no baseline of known-failing tests, on purpose: a baseline is a fixture
that rots. This repo shipped a test asserting a spec named `status_snapshot`;
that spec moved to another repo, and the assertion then read as a resolver bug
for eight consecutive pushes. Red means red.

Editing a plugin (`websites/*/plugin/main.rhai`)? Start at
[`scripts/rhai/README.md`](./scripts/rhai/README.md) — the parse/route/probe
loop there costs 0.33s.

That loop compiles plugins by *running* them through the engine, which is why
`compile.py` names a host that cannot resolve in `policy.allow_hosts`: an empty
list means "allow everything" (`check_host`, `src/capability/mod.rs`), so
huggingface's `run()` really did make 11 live HTTP calls — 4.9s of a 5.6s gate,
and a red gate whenever the network was down. Syntax errors are raised by
`build_engine()` before any op runs, so denying the network costs no detection.

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
3. **user** — `$LAYA_USER_DSL_DIR` → `~/.laya-workflow/dsl`. Personal, never
   committed.
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

### SQLite + DuckDB (`db`)

`kind: "db"` is a small **HTAP wrapper** over **one file**: **SQLite** is the
ACID system of record (constraints, `BEGIN IMMEDIATE … COMMIT` batches),
**DuckDB** is the analytics engine and attaches that same live SQLite file
through its `sqlite` extension
([duckdb-sqlite](https://github.com/duckdb/duckdb-sqlite)) — no ETL, no second
copy. DuckDB can also write aggregates back into SQLite (`op: sync`), so results
land in an ACID table. Read-only by default; writes need `readonly: false`.

Both **modes** are supported:

```bash
export LAYA_WORK_DIR=/tmp/laya-db-demo
mkdir -p "$LAYA_WORK_DIR"

# embed (default): the workflow drives the local sqlite3/duckdb CLIs per call
laya-workflow run --spec dsl/capabilities/db_analytics.json

# server: one daemon owns the file; workflows POST ops to it (shared writer)
laya-workflow db serve --sqlite "$LAYA_WORK_DIR/shop.sqlite" --port 18767 --daemon
laya-workflow run --spec dsl/capabilities/db_server_analytics.json
```

`embed` spawns the CLIs, so it needs `policy.allow_exec` and both files must sit
under `policy.allow_paths`; `server` spawns nothing client-side and needs neither
— only the daemon does. See [`docs/db.md`](./docs/db.md).

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

alphaXiv only generates a paper's AI Overview on request, and until it does the
page's overview section reads "No overview yet…", which a plain render would save
as though it were the overview. When you ask for a single paper by URL the
plugin clicks the site's own **Generate overview** button and waits for the
result (alphaXiv says about five minutes) before capturing, so the Markdown
carries the real overview. It is bounded and opt-out:

```bash
# skip the generation round trip (and its few minutes) entirely
laya-workflow run --spec dsl/browser/alphaxiv_paper.json \
  --query "https://www.alphaxiv.org/abs/2609.31048" \
  --state '{"generate_overview":false}'

# cap how long to wait for it (default 420000 ms)
laya-workflow run --spec dsl/browser/alphaxiv_paper.json \
  --query "https://www.alphaxiv.org/abs/2609.31048" \
  --state '{"overview_wait_ms":600000}'
```

Search and trending runs do **not** generate overviews unless you pass
`"generate_overview":true`, and then only for the first
`overview_max_papers` (default 3) — generation costs minutes per paper, so a
500-paper trending run would otherwise stall for days. What happened is recorded
per paper as `overview_state`/`overview_ready`/`overview_waited_ms` in the run
result and in each `<slug>_meta.json`.

### Desktop notifications (`notify`)

`kind: "notify"` (alias `notify_local`) turns a workflow step into a **real
desktop notification**. On macOS it posts a Notification Center banner through
`osascript`'s `display notification`; on any host it can also append a line to a
log file. No extra crate — the banner is an `osascript` call run through the
`exec` capability, so it is governed by the same policy gates.

`channel` picks the target: `log` (default; path-allow-listed file), `macos`
(Notification Center banner), `both`, or `auto` (**macOS on a Mac, log
elsewhere**). `title` / `subtitle` / `sound` (e.g. `Glass`) shape the banner, and
each is overridable per call via `with`. Posting a banner needs
`policy.allow_exec`; the log channel needs its file under `policy.allow_paths`.

```bash
export LAYA_WORK_DIR=/tmp/laya-notify-demo && mkdir -p "$LAYA_WORK_DIR"
laya-workflow run --spec dsl/capabilities/notify_macos.json \
  --state '{"text":"build finished","topic":"ci"}'
laya-workflow notify --message "build finished" --title Laya --sound Glass
```

A `macos`-only spec fails closed on Linux/CI — prefer `auto`, which degrades to
the log file. See [`docs/notify.md`](./docs/notify.md).

### Feishu / Lark (`飞书`)

Feishu automation is not compiled in — it is driven through the `lark-cli`
command (the `lark-im` skill, `~/.agents/skills/lark-im/`) via the `exec`
capability. With an authorized user identity you can list chats, **read a
conversation's message history**, search across chats, read threads, download
attachments, and send/reply. `dsl/capabilities/feishu_chat_history.json` reads a
chat's recent history and runs a model decision over it:

```bash
export PATH="$HOME/.local/bin:$PATH"
laya-workflow run --spec dsl/capabilities/feishu_chat_history.json \
  --state '{"chat_id":"oc_xxx"}'
```

Reading needs `policy.allow_exec: true` (it spawns the CLI) and an authorized
`lark-cli auth login`. See [`docs/feishu.md`](./docs/feishu.md).

### DingTalk (`钉钉`)

DingTalk has **no** public API to read conversation history (a bot only receives
messages **@-mentioned to it**; the message-menu JSAPI returns **one** message the
user acted on; the data APIs return **aggregate counts only**, and the official
MCP can only *send*). Since the macOS app renders messages as accessibility
elements, we read the client directly instead — "computer use", no database
decryption:

```bash
./scripts/dingtalk-ax sessions        # conversations (name / last message / time)
./scripts/dingtalk-ax chat            # the open conversation's messages
laya-workflow run --spec dsl/capabilities/dingtalk_chat_history.json \
  --state "{\"tool\":\"$PWD/scripts/dingtalk-ax\"}"
```

Needs Accessibility permission for the caller (macOS only); read-only. Full
findings, API doc links and caveats: [`docs/dingtalk.md`](./docs/dingtalk.md).
## Script plugins (Rhai)

Site- and task-specific logic lives in **Rhai plugins**, not in the compiled
engine — the Rust base keeps the transport, policy gates, resource lifecycle and
rendering, and a plugin adds the site logic. A plugin can only call a small,
audited host API, so `allow_hosts` / `allow_paths` / `allow_exec` and the
"the engine owns tab cleanup" invariant still hold. A **site** gets a folder
`websites/<domain>/` and keeps its plugin under `websites/<domain>/plugin/`
(e.g. `websites/news.ycombinator.com/plugin/`); **tool** plugins stay flat under
`plugins/<name>/`. A site folder is matched by the `name` its `plugin.json`
declares.

Every plugin has a **`group/name` id** — `websites/hackernews`,
`websites/github`, `plugins/textdigest` — with the group coming from the
manifest `"group"` (falling back to the root it lives under). `plugin list`
prints the id, a capability takes it in its `plugin` field, and a bare name
still resolves as an alias.

```bash
# fully offline: a workflow calling a local plugin
laya-workflow run --spec dsl/capabilities/script_plugin.json \
  --state '{"text": "the workflow engine runs the workflow"}'
```

Plugins resolve from an explicit `dir`, then `$LAYA_PLUGIN_DIR`, then
`plugins/<name>` / `websites/*/plugin` walking up to the git root, then
`~/.laya-workflow/plugins` / `~/.laya-workflow/websites` (the
install default), then the copy compiled into the binary.
`websites/alphaxiv` (natural-language alphaXiv downloader) and
`plugins/textdigest` (offline demo) ship with the repo. Install a single plugin
out of any git repo (sparse clone — not the whole repo):

```bash
laya-workflow plugin install <owner/repo> --path websites/alphaxiv.org/plugin
laya-workflow plugin list       # what the engine can see, and from where
```

See [`docs/plugins.md`](./docs/plugins.md) for the host API and the DSL shape, or
run `laya-workflow skill --section plugins` for the embedded guide.

### HuggingFace model monitor

`websites/huggingface.co` is a second, browser-free example: given a phrase (or the
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

`websites/news.ycombinator.com` reads Hacker News — a front page (`top`/`best`/`new`/`ask`/
`show`/`jobs`), a full-text search over story titles/URLs, or one discussion with
its comment tree; `websites/arxiv.org` searches arXiv papers or reads a single paper's
abstract page into a Markdown digest.

```bash
laya-workflow run --spec dsl/browser/hackernews.json --query top --state '{"count":5}'
laya-workflow run --spec dsl/browser/hackernews.json --query "rust async" --state '{"count":10}'
laya-workflow run --spec dsl/browser/arxiv.json --query "rust async" --state '{"count":3}'
laya-workflow run --spec dsl/browser/arxiv.json --query "1706.03762"
```

See [`docs/hackernews.md`](./docs/hackernews.md) and
[`docs/arxiv.md`](./docs/arxiv.md).

Three more keep the same shape: `websites/wikipedia.org` searches articles and reads a
whole article into Markdown (after stripping the site chrome in-page);
`websites/developer.mozilla.org` searches MDN through its public API and reads one doc through a
tab; `websites/bing.com` turns a phrase into Bing organic result rows.

```bash
laya-workflow run --spec dsl/browser/wikipedia.json --query "transformer neural network" --state '{"count":5}'
laya-workflow run --spec dsl/browser/wikipedia.json --query "Rust (programming language)" --state '{"mode":"page"}'
laya-workflow run --spec dsl/browser/mdn.json --query "fetch api" --state '{"count":5}'
laya-workflow run --spec dsl/browser/mdn.json --query "/en-US/docs/Web/API/Fetch_API"
laya-workflow run --spec dsl/browser/bing.json --query "typescript ai framework"
laya-workflow run --spec dsl/browser/v2ex.json --query hot --state '{"count":10}'
```

Four more read the developer registries and GitHub: `websites/crates.io` and
`websites/pypi.org` search their package indexes (or read one crate / project
page into Markdown); `websites/docs.rs` searches crate releases or reads a
crate's rendered API docs; `websites/github.com` lists the trending page
(day/week/month, optional language) or reads one repository's stars / forks /
description and its README as Markdown.

```bash
laya-workflow run --spec dsl/browser/crates.json --query "serde"
laya-workflow run --spec dsl/browser/crates.json --query "https://crates.io/crates/serde"
laya-workflow run --spec dsl/browser/pypi.json --query "requests"
laya-workflow run --spec dsl/browser/docsrs.json --query "serde"
laya-workflow run --spec dsl/browser/github.json --query trending --state '{"since":"daily","count":5}'
laya-workflow run --spec dsl/browser/github.json --query "BurntSushi/ripgrep"
```

### 淘宝 / taobao.com: DDR5 商品价格监控

`websites/taobao.com` mirrors the 闲鱼 reader for the Taobao search feed and detail
pages: search a keyword, browse one listing, and keep every sighting in a durable
`watch.json` so later runs report `new`/`up`/`down`/`same`/`gone`. The first target
is DDR5 memory; an omitted query defaults to `DDR5 内存`, and search results
enter `watch.json` by default (`"track": false` opts out).

```bash
# search (8 DDR5 cards are written to ~/tmp/taobao/items and watch.json)
laya-workflow run --spec dsl/browser/taobao_item.json --query 'DDR5 内存' \
  --state '{"count":8,"browse":0}'

# re-read every tracked item; prices are persisted even when all are unchanged
laya-workflow run --spec dsl/browser/taobao_item.json \
  --state '{"mode":"watch","watch_limit":20,"drop_pct":1}'
```

The parser keeps price and sales in separate nodes (`priceInt`/`realSales`), reads
the detail page's `highlightPrice` (优惠后) and `subPrice` (优惠前), and separates Taobao's
temporary `bixi` wait page from a permanent read failure. A watch run loads one
DDR5 search snapshot (up to five pages) first and reuses matching card prices.
Taobao rotates promotion ids for the same listing, so an exact-title match reuses
the card price while keeping the tracked id/url unchanged; only ids absent from
that snapshot open detail pages. Search results are stored
under `~/tmp/taobao` (policy-gated); `watch.json` records every price sighting and
the watch run always saves it, including `same`.

See [`docs/wikipedia.md`](./docs/wikipedia.md), [`docs/mdn.md`](./docs/mdn.md),
[`docs/bing.md`](./docs/bing.md), [`docs/v2ex.md`](./docs/v2ex.md),
[`docs/crates.md`](./docs/crates.md), [`docs/pypi.md`](./docs/pypi.md),
[`docs/docsrs.md`](./docs/docsrs.md) and [`docs/github.md`](./docs/github.md).

### 闲鱼 / goofish.com: search, browse, collect, watch prices, learn prices

`websites/goofish.com` is a marketplace reader — search a feed, open a listing
into a full product sheet, keep it as JSON + Markdown + pictures, watch the
price of anything you saved, and learn from everything it has seen. One query
picks the behaviour:

```bash
# search (multi-page; `browse` opens the first N into full detail)
laya-workflow run --spec dsl/browser/goofish_item.json --query "索尼 A7M4" \
  --state '{"count":30,"pages":2,"browse":3,"price_min":15000,"price_max":22000}'

# browse one listing → item-<id>.md, items/<id>.json and the downloaded pictures
laya-workflow run --spec dsl/browser/goofish_item.json \
  --query "https://www.goofish.com/item?id=1085216610239"

# price monitoring: add to the watch list, then ask what moved
laya-workflow run --spec dsl/browser/goofish_item.json --query "添加监控 1085216610239"
laya-workflow run --spec dsl/browser/goofish_item.json --query "价格监控" \
  --state '{"watch_query":"A7M4","watch_limit":20,"drop_pct":5}'
```

Results go to `~/tmp/goofish` (policy-gated): `items/<id>.json` per listing,
`item-<id>.md` + `images/<id>/` for an opened one, and `watch.json` holding every
price ever seen per tracked item, so a later run reports
`new`/`up`/`down`/`same`/`gone` instead of just the current number.
`watch_query` narrows that re-read — it matches the title *or* the item id,
case-insensitively, so `a7m4` and `A7M4` both find the item. The monitor also
guards its own store: once an item has three priced sightings behind it, a
reading more than 5× its own median is kept as a sighting *without* a price
(and the refused number is kept beside it). goofish abbreviates prices with a
万/千 character sitting beside the number, and one of those picked up by mistake
turns a ¥9,180 body into ¥91,800,000 — which would then sit in the history
forever, poisoning the median and every trend drawn from it. Real moves pass
through untouched.

**Prices and tags get better with use.** Every run — search, browse or watch —
also feeds a learning pass, and the result gains an `evolve` block:

```bash
# learn from the tags sellers actually write, and price an item against them
laya-workflow run --spec dsl/browser/goofish_item.json --query "价格进化 索尼 A7M4" \
  --state '{"count":30,"browse":6}'

# or re-derive the model from everything already collected (no browsing at all)
laya-workflow run --spec dsl/browser/goofish_item.json --query "价格进化"
```

`价格进化` is an instruction to the plugin, not a search term, so it never
reaches goofish: the site weighs it as heavily as a model number, and
`--query "价格进化 CMP 170HX"` used to come back as a page of cameras. Only
`CMP 170HX` is searched.

It mines the listing attributes into a vocabulary (`品牌: sony` and `品牌: 索尼`
fold into one tag once two listings declare them the same thing), then gives
every tag a robust price band — median ± σ·1.4826·MAD — and reports what sits
outside it:

| `evolve` key | what it is |
| --- | --- |
| `tags` / `vocabulary` | the learned tag vocabulary, with spellings merged and co-occurrence |
| `scope` | the product these bands were drawn from — `"cmp170hx"`, or `""` when the run named none |
| `scopes` | one band per product in the whole corpus, so a mixed index stays legible |
| `fair` | per-tag price band: `fair` (median), `low`, `high`, and how many listings back it |
| `deals` / `overpriced` | items below / above their tag's band, ranked by how far out they are |
| `next_feed_factor` | the learned search-card-vs-detail-page price ratio, or `0` when there is not enough agreeing evidence to correct anything |

**A price is only a price against the same product.** `index.json` accumulates
every product ever looked up, and `功能完好无维修` means exactly as much on a
¥9,200 camera as on a ¥13,400 graphics card — so a band that pooled them
answered a question nobody asked. Vocabulary stays shared across products (the
concept is the same concept everywhere), but prices are compared only inside the
product the run named, and `evolve.scopes` reports every market at once:

```
cmp170hx    items=30 priced=30 median=13899.5 min=9900 max=62000 fair=13899.5 band=[10935.78, 16863.22]
(unnamed)   items=52 priced=52 median=8450.0 min=4.9 max=19800 fair=8450.0  band=[4446.98, 12453.02]
```

A listing collected outside any named search keeps no scope rather than guessing
one, and a later run that meets the same item without naming a product inherits
the market it already had.

Three files carry it, all under `out_dir`: `index.json` (one row per item ever
collected, plus the tags mined from it), `tags.json` (the vocabulary) and
`price_model.json` (the price bands). Only *accumulated* evidence grows between
runs — every statistic is re-derived from `index.json`, so re-running changes
nothing but the counter and the new evidence.

**…or hand it to SQLite.** With `emit_sql: true` the plugin also returns the DDL
(`sql_schema`) and idempotent upserts (`sql_statements`) for six `goofish_`
tables (including `goofish_scopes`, one row per product), so what lands in the
database is exactly the numbers the plugin priced with:

```bash
laya-workflow run --spec dsl/browser/goofish_sqlite.json --state '{"query":"索尼 A7M4","browse":6}'
```

That spec writes one SQLite file, then reads it back for fair price per tag,
price moves per item, tag-level trends, and the below/above-fair report.

Worth knowing before changing the page scripts: `/search` ignores `?page=` and
`?priceMin=` (paging is clicked, and the price band is applied locally); feed
prices are abbreviated (`¥1.98万`, and the 万 lives outside the price block);
and a sold listing renders no item block at all, so `status` is explicit.
[`docs/plugins.md`](./docs/plugins.md#闲鱼--goofishcom-search-browse-collect-watch)
has the details.

## License

Dual-licensed: MIT OR Apache-2.0.
