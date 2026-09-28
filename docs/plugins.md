# Script plugins (Rhai)

The Rust code in this crate is the **base**: CDP transport, policy gates,
resource lifecycle, secret redaction, page→Markdown rendering, the capability
registry. Anything that is *site-* or *task-specific* — which selector to read,
which endpoint to page, how a natural-language query maps to a mode, how a
vendor's JSON maps to workflow state — belongs in a **plugin**.

Plugins are Rhai scripts. That keeps the interesting, fast-moving part out of the
compiled binary while the engine stays the single place where security and
resource ownership are enforced.

## Declaring and calling a plugin

```jsonc
{
  "capabilities": {
    "chrome":   { "kind": "chrome_cdp", "endpoint": "http://127.0.0.1:9222", "launch": true },
    "alphaxiv": { "kind": "plugin", "plugin": "websites/alphaxiv", "browser": "chrome" }
  },
  "nodes": [
    {
      "name": "fetch_papers",
      "action": {
        "kind": "call",
        "capability": "alphaxiv",
        "with":    { "query": "${state.query}", "count": "${state.count}" },
        "project": { "mode": "/mode", "saved_count": "/saved_count" }
      }
    }
  ]
}
```

`script` is an accepted alias for `kind: "plugin"`. Fields:

| field | meaning |
|-------|---------|
| `plugin` | plugin id, `group/name` (or a bare name); resolved from the layers below |
| `dir` | explicit plugin **directory**, or a single `.rhai` **file** run on its own (no manifest needed) |
| `entry` | entry file: overrides the manifest inside `dir`, or — with no `dir`/`plugin` — names a `.rhai` file to run directly |
| `op` | entry function (default `run`) |
| `browser` | name of a `chrome_cdp` capability in the same spec that the plugin may drive |
| `max_operations` | Rhai instruction budget (default from the manifest) |
| `timeout_ms` | timeout for the plugin's browser/CDP work |

The result of the call is whatever the entry function returns (a JSON object), so
`project`, `chain` and downstream nodes work exactly as for any other capability.

### Single-file plugins

A plugin needs neither a directory nor a `plugin.json` when it is one script:
point the capability straight at the file — via `dir` **or** `entry` — and it
runs with `entry_op: "run"`, the file stem as its name, and any sibling `page/`
still available to `host.js`. A `plugin.json` is only required for a multi-file
directory plugin and for `plugin install`.

```jsonc
{ "capabilities": { "greet": { "kind": "plugin", "dir": "scripts/greet.rhai" } } }
```

A runnable copy ships in the repo: `dsl/capabilities/single_file_plugin.json`
points `dir` at the bare `dsl/capabilities/hello.rhai`. Run it from the repo root
(`laya-workflow run --spec dsl/capabilities/single_file_plugin.json --state
'{"who":"laya"}'`) — no directory, no `plugin.json`, no browser, no network.

## Installing a plugin

`plugin install` fetches **only the one directory** you name — never the whole
repo (`git clone --depth 1 --filter=blob:none --sparse` + `sparse-checkout set`)
— then copies it into the plugin root:

```sh
laya-workflow plugin install <owner/repo> --path <dir> [--name N] [--git-ref R] [--force] [--root D]
laya-workflow plugin list     # every plugin the engine can see, and its layer
laya-workflow plugin dir      # the install root + the search path
```

`--path` is the plugin directory *inside* the repo (e.g.
`websites/alphaxiv.org/plugin`); naming the site folder instead
(`websites/alphaxiv`) installs its `plugin/` subdir too. The installed name
defaults to the source `plugin.json` `name` (falling back to the last path
segment), and `--name` overrides it. `--root` overrides the install root
(default: `$LAYA_PLUGIN_DIR`, else `~/.config/laya-workflow/plugins`). Install
is refused unless the directory (or a `plugin/` subdir of it) has a parseable
`plugin.json` (and no `--force` is needed only when the name is not already
present); credentials in a URL are stripped before anything is printed.

## Layout and resolution layers

Two directories hold plugins: a **site** gets a folder `websites/<domain>/`
that keeps its plugin under `websites/<domain>/plugin/` (e.g.
`websites/news.ycombinator.com/plugin/`; the site folder itself is free for
docs, fixtures, notes). General-purpose / tool plugins live flat under
`plugins/<name>/`. Both are ordinary plugin directories — the folder is the
author's filing choice, and a site folder is matched by the `name` its
`plugin/plugin.json` declares, so `websites/news.ycombinator.com/` answers to
`hackernews`.

### Plugin ids are `group/name`

Every plugin has a two-segment id: a **group** then its **name**, e.g.
`websites/hackernews`, `websites/github`, `plugins/textdigest`. The group comes
from the plugin's `plugin.json` `"group"` field, falling back to the root it was
found under (`websites` for site plugins, `plugins` for tools). A plugin id is
what `plugin list` prints, what the `plugin` capability field takes, and what
`ctx["plugin"]` reports to the script.

Resolution accepts the grouped id (`websites/hackernews`) *or* the bare name
(`hackernews`); both find the same plugin. A grouped id whose group does not
match is refused, so two plugins may share a name in different groups.

Highest priority first — an id in a higher layer shadows the same id below:

1. an explicit `dir`;
2. `$LAYA_PLUGIN_DIR/<name>/`;
3. `plugins/<name>/` and `websites/*/plugin/`, found by walking up from the
   cwd and stopping at the git root (this is the layer a repository commits —
   a site folder is matched by its manifest `name`);
4. `~/.config/laya-workflow/plugins/<name>/` and
   `~/.config/laya-workflow/websites/*/plugin/` — the `plugin install` default
   (`$LAYA_USER_PLUGIN_DIR` / `$XDG_CONFIG_HOME` override it);
5. the copy compiled into the binary (`include_str!`), so a plugin shipped with a
   release still works after `sudo install`-ing a single binary.

## Writing one

```
websites/<domain>/plugin/     (site plugins)     plugins/<name>/   (tools)
├── plugin.json   # name, entry, entry_op, max_operations, pages
├── main.rhai     # the logic
└── page/*.js     # optional page-side collectors, injected via host.js(name)
```

```json
{ "name": "textdigest", "group": "plugins", "entry": "main.rhai", "entry_op": "run", "max_operations": 2000000 }
```

`group` is optional; leave it out and the plugin id falls back to the bare
`name` (or to the root class if it is discovered under `websites/` / `plugins/`).

```rhai
fn run(host, ctx) {
    let args = ctx["with"];          // the expanded `with` of the call node
    let state = ctx["state"];        // the workflow state
    #{ echo: args["text"], at: host.now()["rfc3339"] }
}
```

The entry function may take `(host, ctx)`; a one-argument `fn run(ctx)` is also
accepted. `ctx` is `#{ plugin, op, with, state }`.

A **page script** is injected by building the expression yourself, so the plugin
controls exactly what the page sees:

```rhai
let opts = #{ limit: 10 };
let js = "var __LAYA_OPTS__ = " + host.json_stringify(opts) + ";\n" + host.js("links.js");
let value = host.browser_evaluate(target, js, true);
```

## Host API (the whole of it)

A plugin can call **only** these. Everything funnels back through the engine's own
capability code, so policy still applies and the tab lifecycle still has one owner.

| call | effect |
|------|--------|
| `host.log(msg)` | stderr, secret-redacted |
| `host.now()` | `#{ unix_ms, rfc3339, rfc3339_local, utc_offset_secs, tz, tz_abbrev }` |
| `host.parse_time(s)` | RFC 3339 (or a bare epoch) → Unix ms; `()` when unparseable |
| `host.time_format(ms, offset_secs)` | render ms at an explicit offset (`…+08:00`; `Z` for 0) |
| `host.timeout_ms()` | the effective timeout for this plugin run |
| `host.urlencode(s)` | percent-encode a query value |
| `host.slug(url, title)` | the filesystem-safe slug the engine would derive |
| `host.json_parse(s)` / `host.json_stringify(v)` | JSON ↔ script values |
| `host.js("links.js")` | read one of the plugin's `page/` scripts |
| `host.browser_open(url)` | new background tab → `target_id` (policy-checked, owned by the engine) |
| `host.browser_navigate(t, url)` | navigate an existing tab |
| `host.browser_wait_ready(t)` | best-effort readyState wait |
| `host.browser_evaluate(t, js, await_promise)` | evaluate in the page, get JSON back |
| `host.browser_release(t)` | ask the engine to close that tab now |
| `host.save_article(opts)` | render a page to Markdown + figures (the generic engine path) |
| `host.http_get(url)` | allow-listed, bounded, truncating GET (raises on any non-2xx) |
| `host.write_file(path, text)` | policy-gated write, creates parent dirs; returns `#{ path, bytes }` |
| `host.read_file(path)` | policy-gated read; returns `#{ path, exists, bytes, text }` |

### Invariants the host enforces

* A plugin **cannot** start Chrome; the engine launches (or attaches to) the
  singleton before the script runs.
* A plugin **cannot** close a target on its own: it asks through
  `host.browser_release`, and the engine performs the close. Whatever is still
  open when the script returns is closed by the engine's own sweep, so
  `keep_open`, `max_owned_pages` and the idle GC keep working.
* Every navigation passes `policy.allow_hosts`; every write path passes
  `policy.allow_paths`; `host.http_get` is bounded by the same timeout and
  output caps as any other capability.
* Results are returned through the engine, so they are stamped and redacted on
  the way out.
* The language itself has **no** file, network, `eval`, `import` or raw `print`
  access, and runs under an instruction budget, a call-depth limit, and
  string/array/map size caps.

## Sandbox notes

`Engine::new()` is used for the string/math/collection packages (Rhai's own
standard library has no I/O), then `eval`, `import`, `print`, `debug` and `Fn` are
disabled, and the only way to reach the outside world is the table above.

Two Rhai sharp edges are worth knowing when writing plugins:

* a script-level `const`/`let` is **not** visible inside functions when the host
  calls a function directly — declare constants inside the function that uses them;
* `trim()`, `replace()` and other methods that return a borrowed slice do not
  survive a `return`; do the string surgery with `sub_string`/`len` instead.

## Bundled plugins

| plugin | what it shows |
|--------|---------------|
| `websites/alphaxiv` | the real thing: alphaXiv discovery (search / URL / trending), locale URL rewriting, feed-API paging, per-paper retry, on-demand AI Overview generation, `meta.json` provenance |
| `plugins/textdigest` | a tiny, fully offline plugin (`dsl/capabilities/script_plugin.json`) |
| `plugins/browser_base` | generic single-page browser primitives (open / evaluate / wait_htmx / assert) with deterministic navigation-wait + retry — the shared base for browser verification plugins; `dsl/browser/browser_base_probe.json` demos it |
| `websites/hf-trending` | a HuggingFace model monitor (no browser): per-model rank / likes / downloads / card metadata + the model card, written to snapshots, a report, `cards/` and `history.jsonl` (`dsl/capabilities/hf_trending.json`) |
| `websites/hackernews` | a Hacker News reader over Chrome/CDP: front pages (top/best/new/ask/show/jobs), a full-text search (public Algolia index) and one discussion with its comment tree (`dsl/browser/hackernews.json`) |
| `websites/arxiv` | an arXiv reader over Chrome/CDP: search papers by phrase, or read one paper's abstract page into a Markdown digest + `meta.json` (`dsl/browser/arxiv.json`) |
| `websites/wikipedia` | a Wikipedia reader over Chrome/CDP: article search, and a whole article rendered to Markdown after the site chrome is stripped in-page (`dsl/browser/wikipedia.json`) |
| `websites/mdn` | an MDN Web Docs reader: search via the public search API, and one doc rendered to Markdown through a Chrome tab (`dsl/browser/mdn.json`) |
| `websites/bing` | a Bing web-search reader over Chrome/CDP: a phrase in, organic result rows out (`dsl/browser/bing.json`) |
| `websites/v2ex` | a V2EX reader over Chrome/CDP: a tab's topic list (latest/hot/tech/...), a node's topics, or one topic with its replies (`dsl/browser/v2ex.json`) |
| `websites/crates` | a crates.io reader over Chrome/CDP: a phrase into crate search rows (name / version / description / downloads / docs.rs), or one crate page into Markdown (`dsl/browser/crates.json`) |
| `websites/pypi` | a PyPI reader over Chrome/CDP: a phrase into package search rows, or one project page into Markdown (`dsl/browser/pypi.json`) |
| `websites/docsrs` | a docs.rs reader over Chrome/CDP: crate-release search rows, or one crate's rendered API docs into Markdown (`dsl/browser/docsrs.json`) |
| `websites/github` | a GitHub reader over Chrome/CDP: the trending page (day/week/month, optional language), or one repository — stars / forks / description + README as Markdown (`dsl/browser/github.json`) |
| `websites/goofish` | a 闲鱼 / goofish.com reader over Chrome/CDP: **search** the feed, **browse** one listing into a full product sheet, **collect** it to JSON + Markdown + pictures, **watch prices** durably in `watch.json`, and **learn** a tag vocabulary and a fair price per tag from everything it has seen (`dsl/browser/goofish_item.json`, `dsl/browser/goofish_sqlite.json`) |

`dsl/browser/alphaxiv_paper.json` is the plugin-driven spec:
`laya-workflow run --spec dsl/browser/alphaxiv_paper.json --query "trending" --state '{"count":3}'`.

### 闲鱼 / goofish.com: search, browse, collect, watch, learn

`websites/goofish.com/plugin` covers the five things you do on 闲鱼, and one
query string picks between them:

```bash
# search — a keyword, a count, optional full-detail opens
laya-workflow run --spec dsl/browser/goofish_item.json --query "索尼 A7M4" \
  --state '{"count":30,"pages":2,"browse":3,"price_min":15000,"price_max":22000}'

# browse + collect — one listing as a product sheet (Markdown, JSON, pictures)
laya-workflow run --spec dsl/browser/goofish_item.json \
  --query "https://www.goofish.com/item?id=1085216610239"
# a pasted /search URL works too, percent-encoding and all

# price monitoring — add, then check what moved
laya-workflow run --spec dsl/browser/goofish_item.json --query "添加监控 1085216610239"
laya-workflow run --spec dsl/browser/goofish_item.json --query "价格监控" \
  --state '{"watch_query":"A7M4","watch_limit":20,"drop_pct":5}'

# self-evolution — collect, learn a vocabulary, price against it
laya-workflow run --spec dsl/browser/goofish_item.json --query "价格进化 索尼 A7M4" \
  --state '{"count":30,"browse":6}'
# no query at all: re-derive the model from index.json, without browsing
laya-workflow run --spec dsl/browser/goofish_item.json --query "价格进化"
```

`plan()` infers the mode: a keyword is a search, an item URL or a bare 12–13
digit id is one listing, `价格监控` / `watch price` is monitoring,
`添加监控 <url>` adds to it, and `价格进化` / `fair price` / `evolve` is the
learning mode. An explicit `state.mode` always wins. Everything lands in
`out_dir` (default `~/tmp/goofish`): `items/<id>.json` per listing,
`item-<id>.md` + downloaded `images/<id>/` for an opened listing, and
`watch.json` — one entry per tracked item with every price ever seen, so a
later `watch` run reports `new` / `up` / `down` / `same` / `gone` rather than
just the current number.

#### Self-evolution: tags and prices that learn

Every mode feeds the learning pass, not just `evolve` — watching a price or
browsing a listing is exactly the fresh evidence the model needs, and making
the caller opt in separately would mean it only ever learns when someone
remembered to ask. The result gains an `evolve` block:

| key | meaning |
| --- | --- |
| `items_indexed` / `new_this_run` | corpus size, and what this run added |
| `tags` / `vocabulary` | the learned tag vocabulary and how many tag keys back it |
| `fair` | per-tag band: median (`fair`), `low`, `high`, and `n` listings behind it |
| `deals` / `overpriced` | items outside their tag's band, split by direction, ranked by distance |
| `next_feed_factor` | the learned search-card-vs-detail price ratio, `0` when there is not enough evidence |
| `index_file` / `tags_file` / `model_file` | the three files it wrote |

Three files under `out_dir` carry it:

- **`index.json`** — the corpus: one row per item ever collected, with the tags
  mined from it. Every statistic is re-derived from this file, which is what
  makes re-running idempotent instead of cumulative.
- **`tags.json`** — the vocabulary: canonical tag, the spellings folded into it,
  the raw forms seen on the page, co-occurrence, and the synonym pairs that
  justify the folds.
- **`price_model.json`** — per-tag price distribution (median, MAD, min/max/mean,
  the band), the per-item price history summary, and the learned correction.

Only *accumulated* state grows: spelling counts, synonym evidence (pair
sightings), correction ratios, the run counter. Folding runs into statistics is
how a counter ends up counting the same listing twice and pricing a camera by
how often it was looked at.

Three decisions worth knowing before changing this code:

- **Synonyms are conservative, and deliberately so.** Two spellings merge on
  containment, across scripts (`Sony` ↔ `索尼`, the real goofish pattern
  `品牌: "Sony/索尼"`), or on alphanumeric squeeze (`a7m4` ↔ `a7-m4`) — and only
  once two independent listings declare them together. Prefix matching is
  explicitly *not* used: it would merge `全新` into `几乎全新` and `A7M4` into
  `A7M3`. `detail.seller.tags` is not mined either — those badges describe the
  *seller* ("来闲鱼5年"), and folding them in would end up pricing a camera by
  how long its owner has been a member.
- **The price band is robust, not mean ± sd.** One ¥1 body listing in a
  thousand should not move the answer, so it is median ± σ·1.4826·MAD, with a
  10% fallback band when the MAD is zero. `sigma` defaults to 2.0 and a tag
  needs `min_samples` (3) priced listings before it is allowed to judge a
  price at all.
- **The card/detail correction has four gates** — enough independent sightings
  (`feed_min_hits`, 5), a tight distribution (p75/p25 within `feed_max_spread`,
  1.10), a plausible magnitude (0.5×–2×), and a real difference from 1 — before
  it touches a price. It is only ever applied to a card that no detail page
  confirmed, and the uncorrected price is kept beside it.

#### …and into SQLite

With `emit_sql: true` the plugin also returns `sql_schema` (six `CREATE TABLE IF
NOT EXISTS` statements) and `sql_statements` (idempotent `INSERT OR REPLACE`
upserts, capped by `sql_max` / `sql_points`). Nothing is executed by the plugin —
it has no way to reach a database. `dsl/browser/goofish_sqlite.json` hands both
arrays to a `kind: "db"` capability and then reads the file back:

```bash
laya-workflow run --spec dsl/browser/goofish_sqlite.json \
  --state '{"query":"索尼 A7M4","browse":6}'
```

The tables are `goofish_items`, `goofish_price_points`, `goofish_tags`,
`goofish_item_tags`, `goofish_tag_price_stats` and
`goofish_price_corrections`, all prefixed because the SQLite file is a shared
resource. The spec's later nodes are plain SQL over them: fair price per tag,
price moves per item (via `LAG`), tag-level trends, and the below/above-fair
report. `goofish_items.price` is the price the item's tags were priced *by* —
the same number the tag statistics came from — while `last_price` is the watch
history's latest sighting; keeping both is what lets "compare this listing
against its tag" be answerable in SQL. `goofish_price_points.implausible_price`
is the reading the monitor refused (see the abbreviated-prices note below), so a
hole in the series is legible as a hole rather than as a million-percent jump.

One DuckDB detail the spec pays for: `at` is a keyword, so the price-moves node
quotes it and aliases the result to `price_at`. Unquoted it parses fine in
SQLite and fails in DuckDB — which is exactly the half of the system a
`kind: "db"` analytics op runs on.

Three site facts shape the plugin, all verified against the live site and worth
knowing before changing the page scripts:

- **`/search` ignores `?page=`, `?sort=`, `?priceMin=` and `?priceMax=`.** Every
  value returns the same 30 ids; paging is in-page React state, so
  `page/search.js` clicks the site's own page boxes. The price band is therefore
  applied to what came back (and says so in `notes`) — it is a shorter list, not
  a different one.
- **Feed prices are abbreviated.** A ¥19,800 camera renders as `¥1.98万`, with
  the 万 in a *sibling* of the price block. Missing that character reads 1.98,
  which a price monitor then reports as a 99.99% drop. It is matched by element,
  never by searching the row's text: `4万浏览` in the want slot is not a
  magnitude. One stray character in the other direction — a 万 picked up by a
  ¥9,180 body — reads ¥91,800,000, and that number poisons the median, the high
  and every trend built on them *permanently*, because a price history is
  append-only. So `observe()` also guards the store: once an item has three
  priced sightings behind it, a reading more than 5× its own median (up or down)
  is recorded as a sighting with no price plus `implausible_price`, and the item
  keeps its last believed price. Real moves are unaffected — a 9180 → 6900 drop
  is accepted and reported as `down` — and so is an item with too little history
  to have a baseline, which is never second-guessed.
- **A sold listing renders no item block at all** — no "sold" page, just the
  footer and "看看下面为你推荐". `status` is therefore explicit
  (`on_sale` / `gone` / `login_required`), and a `watch` run that finds `gone`
  records the change and drops the stale price rather than reporting it again.

The detail page is a `<span>`-soup SPA that the generic article renderer cannot
use — a live capture with the item container as the selector produced 432 bytes
of Markdown: ten pictures, the "为你推荐" heading, and not one character of
title, price, description or attributes (it drops any div without block-level
children). So the plugin writes its own product sheet from the structured
record, while still calling `host.save_article` first, because that is the call
that downloads the pictures. Matching a downloaded file back to its listing URL
goes through the alicdn *object key*, not the whole URL: the carousel and the
downloader spell the same picture differently.

### Generating a missing AI Overview

alphaXiv generates a paper's AI Overview on request. Until it exists, the page's
overview section reads "No overview yet…", and a plain render saves that string
as though it were the overview — so the Markdown looks complete while carrying
nothing. `page/overview.js` reads the section's real state (`ready`,
`placeholder`, `generating`, `absent`, `unknown`), and in `url` mode
`main.rhai` clicks the site's own **Generate overview** button and waits for the
result before capturing. The capture then reuses the tab it waited on, via
`save_article`'s `target_id`, instead of navigating to the paper twice.

The wait is bounded on purpose: one CDP poll per round (kept under the
capability's per-call timeout), a wall-clock budget (`overview_wait_ms`, default
7 minutes — the site itself says about five), and a bounded number of
re-entries. Re-entering the paper is what makes the wait work at all: the
generation is server-side and survives a reload, but the page's live channel is
not, so the tab that clicked may never hear about the run it started, while a
page loaded afterwards re-reads what the server already has.

`generate_overview` (default on for `url` mode, off for `search`/`trending`
because generation costs minutes per paper) turns it off;
`overview_max_papers` caps how many papers of a listing run generate one.
Every paper's outcome lands in the run result and in `<slug>_meta.json` as
`overview_state`, `overview_ready`, `overview_clicked` and `overview_waited_ms`.

The browser capability's `op: "alphaxiv"` is kept as a compatibility entry point
and simply forwards to the same plugin.


## Bundled base browser plugin: `plugins/browser_base`

The binary ships one **generic** browser plugin so a checkout can drive a real
Chrome page (via a `chrome_cdp` capability) without shipping any site logic.
It exists to hold the browser mechanics every verification plugin would
otherwise re-implement, factored out of `devine_int`'s `devine_console_probe`:

* **`_wait_loaded(host, target, prefix)`** — poll short evaluates until the
  target URL matches `prefix` **and** `readyState` is `complete`. This is
  deliberate: `host.browser_wait_ready` skips the URL check and can return on
  the pre-navigation `about:blank` target, so a long evaluate issued right
  after races the real navigation commit (CDP `-32000 Inspected target
  navigated or closed`). 100 × 0.15s ≈ 15s worst case.
* **`_safe_release`** — best-effort page close that never masks the primary
  error.
* **`_retry(host, ctx, op)`** — up to 3 attempts, a fresh page per attempt for
  self-contained ops, as a secondary net for any other transient CDP failure.
* **`wait_htmx_helper()`** — the in-page JS that waits for `window.htmx`.

Ops (pick via the capability `op` or `with.op`):

| op | `with` | behaviour |
|----|--------|-----------|
| `open` | `url`, `prefix` (default `url`), `keep_open` | open + `_wait_loaded`; reports `{url, title, ready, sample}`; self-contained (page closed) unless `keep_open: true`, which returns a live `target_id` for later nodes |
| `evaluate` | `expression` (JS), `url`/`target_id`, `prefix`, `await_promise` | run arbitrary JS on a page; returns the evaluate result |
| `wait_htmx` | `url`/`target_id`, `prefix` | wait up to 15s for `window.htmx`; reports `{htmx_loaded, url, title, text}` |
| `assert` | `expression`, `checks` (map of key → expected), `url`/`target_id` | run the expression and require every check to equal its expected value; **throws a deterministic FAIL otherwise** — a throwing plugin fails the workflow (rc=1), so a spec routes PASS/FAIL without an LLM |

`run()` is a guard that tells you to pick an op. `dsl/browser/browser_base_probe.json`
is a runnable demo (open → wait_htmx → assert → done) against any localhost URL.
Site/specific assertions (e.g. devine console's hub/shell/pong checks) stay in
the owning repo — `devine_int` keeps its own `devine_console_probe` on top.

Local orchestration is built into the engine as two subcommands (the generic,
repo-agnostic half of the devine_int workflow; the devine-specific server build
stays in `devine_int`). Nothing here is devine- or site-specific:

* `laya-workflow browser ensure [--backend chrome] [--port N] [--chrome-bin P]
  [--profile D]` — ensure a local browser backend on `127.0.0.1:<port>`
  (idempotent: exits 0 immediately when it is already up). Only `chrome` (a CDP
  endpoint) exists today; the `--backend` selector keeps the call shape stable
  if a better backend arrives. Chrome env fallbacks: `LAYA_CDP_PORT` /
  `CHROME_BIN` / `LAYA_CDP_PROFILE` (`chrome ensure` is a hidden alias). Point
  the spec's `chrome_cdp` endpoint at it and run it via an `exec` capability
  (`policy.allow_exec: true`) before the browser nodes.
* `laya-workflow server ensure | start [--daemon] | stop | status [--port N]
  [--command C] [--health-path P]` — generic local HTTP server lifecycle for any
  command and port. Liveness = "the port answers HTTP at all" (2xx–5xx), so a
  plain `python3 -m http.server` counts as up even on a default `/healthz` 404;
  pass `--health-path` when the server has a real health endpoint. `ensure` /
  `start` print `BASE=<url>` for the workflow to capture (and `start` without
  `--daemon` stays in the foreground until Ctrl-C / SIGTERM); state files live
  under `/tmp/laya-ensure-server-<port>.{pid,base,log}`.

`scripts/laya-ensure-chrome.sh` and `scripts/laya-ensure-server.py` are now thin
shims that `exec` those two subcommands — kept only as stable, repo-relative
entry points for existing callers (e.g. `devine_int`). New callers should use
the subcommands directly; that requires `laya-workflow` on `PATH` (override the
shim's target with `LAYA_WORKFLOW_BIN`).

`dsl/browser/browser_orchestrate_probe.json` is the end-to-end demo that drives
*both* subcommands plus the `browser_base` plugin in one graph: `browser ensure` →
`ensure_server` (cold-starts `state.server_cmd` as a daemon when the port is
dead, idempotent otherwise) → open → wait_htmx → assert → done. It is
repo-agnostic — point `state.server_cmd` / `state.url` at any local server.
