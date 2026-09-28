# plugins — extending the engine with Rhai scripts

The Rust engine is the **base**: CDP transport, policy gates, resource
lifecycle, secret redaction, page→Markdown rendering, the capability registry.
Anything *site-* or *task-specific* — which selector to read, which endpoint to
page, how a natural-language query maps to a mode, how a vendor's JSON maps to
state — belongs in a **plugin** written in Rhai — a site under `websites/<domain>/`
(a tool under `plugins/<name>/`).

A plugin runs on the same host as the rest of the engine, so policy
(`allow_hosts` / `allow_paths` / `allow_exec`), secret redaction and the
"the engine owns tab cleanup" invariant keep applying. That is the whole point:
Rust stays the trusted base, Rhai is where the fast-moving logic lives and can
change **without rebuilding the binary**.

## Try it in 60 seconds

```sh
# 1. the smallest possible extension: one .rhai file, no directory, no manifest,
#    no browser, no network. Run from the repo root so the relative `dir` resolves.
laya-workflow run --spec dsl/capabilities/single_file_plugin.json \
  --state '{"who":"laya"}'

# 2. a tiny, fully offline named plugin (no browser, no network)
laya-workflow run --spec dsl/capabilities/script_plugin.json \
  --state '{"text":"the quick brown fox the fox"}'

# 3. install your own copy of a plugin from any git repo (sparse: only that dir)
laya-workflow plugin install oliveagle/laya-workflow \
  --path plugins/textdigest

laya-workflow plugin list      # what the engine can see, and from where
laya-workflow plugin dir       # the install root + the search path
```

## Installing a plugin

`plugin install` fetches **only the one directory** you name, never the whole
repo (`git clone --depth 1 --filter=blob:none --sparse` + `sparse-checkout set`),
then copies it into the plugin root:

```sh
laya-workflow plugin install <owner/repo> --path <dir> [--name N] [--git-ref R] [--force] [--root D]
```

| flag | meaning |
|------|---------|
| `<owner/repo>` | `owner/repo`, or a full git URL (`https://`, `ssh://`, `git@`) |
| `--path` | the plugin directory **inside** the repo, e.g. `websites/alphaxiv.org/plugin` (or the site folder, whose `plugin/` subdir is used) |
| `--name` | installed name (default: the source `plugin.json` `name`, else the last segment of `--path`) |
| `--git-ref` | branch / tag / commit to check out |
| `--force` | overwrite an existing install of the same name |
| `--root` | install root (default: `$LAYA_PLUGIN_DIR`, else `~/.config/laya-workflow/plugins`) |

Install is **refused** unless the directory (or its `plugin/` subdir) has a
parseable `plugin.json`, and credentials in a URL are stripped before anything
is printed.

## Layout and resolution layers

Two directories hold plugins: a **site** gets a folder `websites/<domain>/`
that keeps its plugin under `websites/<domain>/plugin/` (e.g.
`websites/news.ycombinator.com/plugin/`; the site folder itself is free for
docs, fixtures, notes). General-purpose / tool plugins live flat under
`plugins/<name>/`. Both are ordinary plugin directories — the folder is the
author's filing choice, and a site folder is matched by the `name` its
`plugin/plugin.json` declares, so `websites/news.ycombinator.com/` answers to
`hackernews`.

Highest priority first — a name in a higher layer shadows the same name below:

1. an explicit `dir` on the capability;
2. `$LAYA_PLUGIN_DIR/<name>/`;
3. `plugins/<name>/` and `websites/*/plugin/`, walking up from the cwd and
   stopping at the git root (the layer a repository commits — a site folder is
   matched by its manifest `name`);
4. `~/.config/laya-workflow/plugins/<name>/` and
   `~/.config/laya-workflow/websites/*/plugin/` (the `plugin install` default —
   `$LAYA_USER_PLUGIN_DIR` / `$XDG_CONFIG_HOME` override it);
5. the copy compiled into the binary (`include_str!`), so a bundled plugin
   still works after `sudo install`-ing a single binary.

## Declaring and calling a plugin

```jsonc
{
  "capabilities": {
    "chrome":   { "kind": "chrome_cdp", "endpoint": "http://127.0.0.1:9222", "launch": true },
    "alphaxiv": { "kind": "plugin", "plugin": "alphaxiv", "browser": "chrome" }
  },
  "nodes": [
    { "name": "fetch_papers",
      "action": { "kind": "call", "capability": "alphaxiv",
                  "with":    { "query": "${state.query}", "count": "${state.count}" },
                  "project": { "mode": "/mode", "saved_count": "/saved_count" } } }
  ]
}
```

`kind: "script"` is an accepted alias for `kind: "plugin"`. Fields:

| field | meaning |
|-------|---------|
| `plugin` | plugin name, resolved from the layers above |
| `dir` | explicit plugin **directory**, or a single `.rhai` **file** run on its own (no manifest needed) |
| `entry` | entry file: overrides the manifest inside `dir`, or — with no `dir`/`plugin` — names a `.rhai` file to run directly |
| `op` | entry function (default `run`) |
| `browser` | name of a `chrome_cdp` capability in the same spec the plugin may drive |
| `max_operations` | Rhai instruction budget (default from the manifest) |
| `timeout_ms` | timeout for the plugin's browser/CDP work |

The call returns whatever the entry function returns (a JSON object), so
`project` / `chain` / downstream nodes work exactly as for any other capability.
A browser capability's `op` may also forward to a named plugin
(`browser: "chrome"`, `op: "alphaxiv"`), which is how the bundled downloader is
reachable without a new capability.

One script, no directory: point `dir` (or `entry`) straight at a `.rhai` file and
it runs on `run` with the file stem as its name — no `plugin.json` needed.

```jsonc
{ "capabilities": { "greet": { "kind": "plugin", "dir": "scripts/greet.rhai" } } }
```

## Writing one

```
websites/<domain>/plugin/   (site plugins)   plugins/<name>/   (tools)
├── plugin.json   # name, version, api, entry, entry_op, max_operations, pages
├── main.rhai     # the logic
└── page/*.js     # optional page-side collectors, injected via host.js(name)
```

```json
{ "name": "textdigest", "version": "0.1.0", "api": 1,
  "entry": "main.rhai", "entry_op": "run", "max_operations": 2000000 }
```

```rhai
fn run(host, ctx) {
    // Constants must live INSIDE the function (see sharp edges below).
    let args  = ctx["with"];   // the expanded `with` of the call node
    let state = ctx["state"];  // the workflow state
    let text = ""; if args.contains("text") { text = "" + args["text"]; }
    #{ echo: text, at: host.now()["rfc3339"] }
}
```

The entry function may take `(host, ctx)`; a one-argument `fn run(ctx)` is also
accepted. `ctx` is `#{ plugin, op, with, state }`.

A **page script** is injected by building the expression yourself, so the plugin
controls exactly what the page sees:

```rhai
let js = "var __LAYA_OPTS__ = " + host.json_stringify(#{ limit: 10 }) + ";\n" + host.js("links.js");
let value = host.browser_evaluate(target, js, true);
```

## Host API (the whole of it)

A plugin can call **only** these. Everything funnels back through the engine's
own capability code, so policy still applies and the tab lifecycle still has one
owner.

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
| `host.browser_open(url)` | new background tab → `target_id` (policy-checked, engine-owned) |
| `host.browser_navigate(t, url)` | navigate an existing tab |
| `host.browser_wait_ready(t)` | best-effort readyState wait |
| `host.browser_evaluate(t, js, await_promise)` | evaluate in the page, get JSON back |
| `host.browser_release(t)` | ask the engine to close that tab now |
| `host.save_article(opts)` | render a page to Markdown + figures (the generic engine path) |
| `host.http_get(url)` | allow-listed, bounded, truncating GET (raises on any non-2xx) |
| `host.write_file(path, text)` | policy-gated write, creates parent dirs; returns `#{ path, bytes }` |
| `host.read_file(path)` | policy-gated read; returns `#{ path, exists, bytes, text }` |

## Invariants the host enforces

* A plugin **cannot** start Chrome; the engine launches (or attaches to) the
  singleton before the script runs.
* A plugin **cannot** close a target itself: it asks via `host.browser_release`,
  and the engine performs the close. Whatever is still open when the script
  returns is closed by the engine's sweep, so `keep_open`, `max_owned_pages` and
  the idle GC keep working. **Never** put explicit cleanup in a workflow.
* Every navigation passes `policy.allow_hosts`; every write path passes
  `policy.allow_paths`; `host.http_get` is bounded by the same timeout/output
  caps as any other capability.
* Results are stamped and redacted on the way out.
* The language has **no** file, network, `eval`, `import` or raw `print` access,
  and runs under an instruction budget, a call-depth limit and string/array/map
  caps.

## Rhai sharp edges (read before writing)

* A script-level `const`/`let` is **not** visible inside a function when the host
  calls it directly — declare constants **inside** the function that uses them.
* `trim()`, `replace()` and other methods that return a *borrowed* slice do not
  survive a `return`; do string surgery with `sub_string` / `len` / `+=`.
* There is no `let mut`: a plain `let` is already mutable (the language is
  dynamic). Reassigning is fine.
* `with` is a reserved word — never name a variable `with`; read the call's
  arguments as `ctx["with"]`.
* `"" + value` stringifies an int; useful methods: `to_lower`, `contains`,
  `starts_with`, `index_of`, `sub_string`, `split`, `len`, `==`.
* The engine is `Engine::new()` (string/math/collection packages) with `eval`,
  `import`, `print`, `debug` and `Fn` disabled.

## Bundled plugins

| plugin | what it shows |
|--------|---------------|
| `websites/alphaxiv.org` | the real thing: alphaXiv discovery (search / URL / trending), locale URL rewriting, feed-API paging, per-paper retry, `meta.json` provenance |
| `plugins/textdigest` | a tiny, fully offline plugin (`dsl/capabilities/script_plugin.json`) |
| `websites/huggingface.co` | a HuggingFace model monitor: rank + likes + downloads + card metadata per model, saved as snapshots / report / cards (`dsl/capabilities/hf_trending.json`) |
| `websites/news.ycombinator.com` | a Hacker News reader over Chrome/CDP: front pages (top/best/new/ask/show/jobs), full-text search, and a discussion + comment tree (`dsl/browser/hackernews.json`) |
| `websites/arxiv.org` | an arXiv reader over Chrome/CDP: search papers, or read one paper into a Markdown digest (`dsl/browser/arxiv.json`) |
| `websites/wikipedia.org` | a Wikipedia reader over Chrome/CDP: article search, and a whole article rendered to Markdown after the site chrome is stripped in-page (`dsl/browser/wikipedia.json`) |
| `websites/developer.mozilla.org` | an MDN Web Docs reader: search via the public search API, and one doc rendered to Markdown through a Chrome tab (`dsl/browser/mdn.json`) |
| `websites/bing.com` | a Bing web-search reader over Chrome/CDP: a phrase in, organic result rows out (`dsl/browser/bing.json`) |
| `websites/v2ex.com` | a V2EX reader over Chrome/CDP: a tab's topic list (latest/hot/tech/...), a node's topics, or one topic with its replies (`dsl/browser/v2ex.json`) |
| `websites/crates.io` | a crates.io reader: crate search rows (name / version / description / downloads / docs.rs), or one crate page into Markdown (`dsl/browser/crates.json`) |
| `websites/pypi.org` | a PyPI reader: package search rows, or one project page into Markdown (`dsl/browser/pypi.json`) |
| `websites/docs.rs` | a docs.rs reader: crate-release search rows, or one crate's rendered API docs into Markdown (`dsl/browser/docsrs.json`) |
| `websites/github.com` | a GitHub reader: the trending page (day/week/month, optional language), or one repository — stars / forks / description + README as Markdown (`dsl/browser/github.json`) |

```sh
laya-workflow run --spec dsl/browser/alphaxiv_paper.json \
  --query "trending" --state '{"count":3}'

# a plugin with no browser: fetch + persist HuggingFace rankings (needs network)
laya-workflow run --spec dsl/capabilities/hf_trending.json \
  --query "llm memory" --state '{"limit":5}'
```

Next: `skill --section dsl` (spec shape), `skill --section safety` (the gates a
plugin's effects still pass through), `docs/plugins.md` (the long form).
