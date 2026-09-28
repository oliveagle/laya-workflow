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
    "alphaxiv": { "kind": "plugin", "plugin": "alphaxiv", "browser": "chrome" }
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
| `plugin` | plugin name; resolved from the layers below |
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

`--path` is the plugin directory *inside* the repo (e.g. `plugins/alphaxiv`);
the installed name defaults to its last segment. `--root` overrides the install
root (default: `$LAYA_PLUGIN_DIR`, else `~/.config/laya-workflow/plugins`).
Install is refused unless the directory has a parseable `plugin.json` (and no
`--force` is needed only when the name is not already present); credentials in a
URL are stripped before anything is printed.

## Resolution layers

Highest priority first — a name in a higher layer shadows the same name below:

1. an explicit `dir`;
2. `$LAYA_PLUGIN_DIR/<name>/`;
3. `plugins/<name>/`, found by walking up from the cwd and stopping at the git root
   (this is the layer a repository commits);
4. `~/.config/laya-workflow/plugins/<name>/` — the `plugin install` default
   (`$LAYA_USER_PLUGIN_DIR` / `$XDG_CONFIG_HOME` override it);
5. the copy compiled into the binary (`include_str!`), so a plugin shipped with a
   release still works after `sudo install`-ing a single binary.

## Writing one

```
plugins/<name>/
├── plugin.json   # name, entry, entry_op, max_operations, pages
├── main.rhai     # the logic
└── page/*.js     # optional page-side collectors, injected via host.js(name)
```

```json
{ "name": "textdigest", "entry": "main.rhai", "entry_op": "run", "max_operations": 2000000 }
```

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
| `plugins/alphaxiv` | the real thing: alphaXiv discovery (search / URL / trending), locale URL rewriting, feed-API paging, per-paper retry, `meta.json` provenance |
| `plugins/textdigest` | a tiny, fully offline plugin (`dsl/capabilities/script_plugin.json`) |
| `plugins/hf-trending` | a HuggingFace model monitor (no browser): per-model rank / likes / downloads / card metadata + the model card, written to snapshots, a report, `cards/` and `history.jsonl` (`dsl/capabilities/hf_trending.json`) |

`dsl/browser/alphaxiv_paper.json` is the plugin-driven spec:
`laya-workflow run --spec dsl/browser/alphaxiv_paper.json --query "trending" --state '{"count":3}'`.

The browser capability's `op: "alphaxiv"` is kept as a compatibility entry point
and simply forwards to the same plugin.
