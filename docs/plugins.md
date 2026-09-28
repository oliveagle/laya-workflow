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

`dsl/browser/alphaxiv_paper.json` is the plugin-driven spec:
`laya-workflow run --spec dsl/browser/alphaxiv_paper.json --query "trending" --state '{"count":3}'`.

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

Local orchestration helpers live in `scripts/` (both are the generic,
repo-agnostic half of the devine_int workflow; the devine-specific server build
stays in `devine_int`):

* `scripts/laya-ensure-chrome.sh` — ensure a CDP Chrome on `127.0.0.1:<port>`
  (idempotent; `LAYA_CDP_PORT` / `CHROME_BIN` override). Point the spec's
  `chrome_cdp` endpoint at it and run via a `shell` capability
  (`policy.allow_exec: true`) before the browser nodes.
* `scripts/laya-ensure-server.py` — generic local HTTP server lifecycle
  (`ensure` / `start [--daemon]` / `stop` / `status`) for any command and port.
  Liveness = "the port answers HTTP at all" (2xx–5xx), so a plain
  `python3 -m http.server` counts as up even on a default `/healthz` 404; pass
  `--health-path` when the server has a real health endpoint. State files under
  `/tmp/laya-ensure-server-<port>.{pid,base,log}`.

`dsl/browser/browser_orchestrate_probe.json` is the end-to-end demo that consumes
*both* helpers plus the `browser_base` plugin in one graph: `ensure_chrome` →
`ensure_server` (cold-starts `state.server_cmd` as a daemon when the port is
dead, idempotent otherwise) → open → wait_htmx → assert → done. It is
repo-agnostic — point `state.server_cmd` / `state.url` at any local server and
`state.repo` at this checkout.
