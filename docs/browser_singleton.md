# Singleton Chrome CDP for Laya workflows

Laya now has a Jev-style real-browser path: observe a real Chrome page,
let a workflow/planner choose one typed operation, then execute it with real CDP
input events. The important deployment invariant is **one Chrome instance and one
extension for all concurrent Laya work**. Threads do not each launch a browser;
they use separate page targets through the same CDP endpoint.

## Components

1. **Rust capability** — `kind: "chrome_cdp"` (alias `browser`) in
   `src/capability/browser.rs`.
2. **Observation extension** — `extensions/laya-browser`, an MV3 content script.
   It exposes `window.__layaBrowser.snapshot()`, stable element indexes, and
   numbered badges. The toolbar action toggles badges.
3. **Execution transport** — Chrome DevTools Protocol. Clicks are
   `Input.dispatchMouseEvent`, text is `Input.dispatchKeyEvent`; pages are not
   controlled by brittle synthetic DOM events alone.

## Launch exactly once

```jsonc
{
  "capabilities": {
    "chrome": {
      "kind": "chrome_cdp",
      "endpoint": "http://127.0.0.1:9222",
      "launch": true,
      "timeout_ms": 10000
    }
  }
}
```

The first call:

- checks `/json/version` and attaches if Chrome is already running;
- otherwise requires `policy.allow_exec`, creates/locks one profile, and starts
  one foreground Chrome window with CDP;
- injects the unpacked plugin through browser-level CDP
  (`Extensions.loadUnpacked`), which works with current branded Chrome releases
  where the command-line extension flag is ignored;
- keeps a process-global binding to that endpoint. A second endpoint in the same
  process fails fast with `the Chrome CDP singleton is already bound...`;
- uses a profile lock to stop two Laya processes from racing the initial launch.

Subsequent capability calls reuse the browser. Workflow threads may operate on
different `target_id` values concurrently. The plugin is loaded once.

## Automatic resource lifetime

Workflow specs normally do not need a `close` node. Pages created by `open`,
`open_many`, and `research` are owned by the Laya process. They remain usable
across every node in that workflow; when the process exits normally, Laya
closes any owned scratch pages that have not already been released.

The browser capability also enforces `max_owned_pages` (default 32). Before it
creates another page, it reclaims owned auto-close pages that have been idle
longer than `owned_idle_ms` (default 5 minutes), then the oldest reclaimable
pages if the ceiling is still full. Explicitly pinned pages are never reclaimed,
but they count toward the ceiling.

Laya writes a `laya-owner.json` marker into the Chrome profile and verifies the
live Chrome command line before reusing an endpoint. An unmarked CDP listener is
rejected instead of being adopted accidentally. The process-wide endpoint binding
and the marked profile keep one Laya singleton.

Use `{"keep_open": true}` on `open` or `open_many` only when a page must outlive
the workflow process. Research uses `{"keep_open_pages": true}` for the same
purpose. Explicit `close` remains available for special cases, but should not
be required for ordinary resource hygiene.

## Operations

| `with.op` | purpose |
|---|---|
| `status` / `targets` | CDP version and visible page target list |
| `open` | create a process-scoped scratch target (`url`, optional `keep_open`) |
| `open_many` | create multiple background targets from a `urls` array; they close by default |
| `research` | search Google, classify ad/organic rows, open organic pages in background, extract text, and score relevance/effectiveness |
| `navigate` | navigate an existing target |
| `snapshot` | URL/title/text plus roles, names, values, hrefs, sections, rects, indexes |
| `highlight` | show/hide numbered extension badges |
| `agent_step` | execute a typed planner decision |
| `evaluate` | main-world JavaScript expression |
| `click` | click by snapshot `element` index or CSS `selector` |
| `type` | focus then send real keystrokes |
| `select` | select an `<option>` and dispatch input/change |
| `scroll` | page/element scrolling |
| `key` | send a named key such as `Enter` |
| `wait_for` | poll for a visible selector |
| `close` | explicitly close a target (rarely needed; kept for special cases) |
| `shutdown` | ask the singleton browser to exit |

## Example

```json
{
  "kind": "call",
  "capability": "chrome",
  "with": {
    "op": "click",
    "target_id": "${state.target_id}",
    "element": 4
  }
}
```

## Runnable demo

Run the bundled one-command smoke demo:

```bash
python3 bench/browser_demo.py
```

It starts a temporary localhost page, supplies the required state, opens a tab in
the singleton Chrome, observes elements, types with real CDP key events, and
shows the extension badges. It prints `typed: 15` and `badges: true`; the owned
tab is closed automatically when the helper exits.

For the same workflow without the helper, `state.url` is mandatory:

```bash
laya-workflow run --spec dsl/browser/browser_singleton.json \
  --state '{"url":"http://127.0.0.1:18777/index.html","text":"hello from Laya"}'
```

Omitting `--state` makes `${state.url}` resolve to null and the capability now
fails clearly instead of trying to open a null URL.

## Google search demo

Search Google for `内容` and open the first ten organic results:

```bash
laya-workflow run --spec dsl/browser/google_search_top10.json --state '{"query":"内容"}'
```

The workflow opens Google, types the query, navigates to the result page, and
merges results across pagination if Google returns fewer than ten entries on the
first page. It uses CDP `Target.createTarget`, so the result tabs are not blocked
as JavaScript popups.

## Laya Google research

The optimized research operation searches in a background tab, classifies
explicit Google advertisement rows, opens only organic results as background
targets, extracts readable text, and applies `laya-browser-heuristic-v1` scoring
for relevance and effectiveness. Each scored page also contains a deterministic
`classification` with `content_type`, matched query labels, a `relevance_class`,
and the signals used for the decision.

```bash
laya-workflow run \
  --spec dsl/browser/google_research.json \
  --state '{"query":"jev ai","result_count":5,"max_open":10,"max_text":4000}'
```

For the common query-only case, use the CLI shorthand:

```bash
laya-workflow run \
  --spec dsl/browser/google_research.json \
  --query "jev ai"
```

Research opens and extracts pages concurrently. The default is **5 tabs**;
control it with `tab_concurrency` (alias: `concurrency`), bounded to 1–10:

```bash
laya-workflow run \
  --spec dsl/browser/google_research.json \
  --query "jev ai" \
  --state '{"result_count":5,"max_open":10,"tab_concurrency":5,"max_text":4000}'
```

Resource safety is preserved: resource reclamation and target creation are
serialized, but page navigation, text extraction, scoring, and classification run
in parallel across independent page websockets. The response includes
`tab_concurrency`, and cleanup still closes every non-pinned research tab.

`query` is required. Readable page text is returned in
`result.scored_pages[*].text`; its size is `content_chars`, and
`content_truncated` records whether the configured `max_text` limit clipped it.
The best matches are in `accepted_pages`; `classification` describes whether the
page is documentation, source code, a paper, tutorial, discussion, product, blog,
or news, and how relevant it is to the query.

Research does not leave its scratch tabs behind. It closes the Google search
target and every page it opened after text extraction and scoring; the page text
and scores remain in the workflow output. Set `"keep_open_pages": true` in the
research `with` object only when a page must outlive the workflow process.

The resource order is deliberately **reclaim before open, then register each new
target for release**: before each page, `enforce_owned_target_limit` may close old
unpinned Laya-owned idle tabs; the new background target is then tracked either by
the research cleanup guard or, for `keep_open_pages`, as a pinned owned target.
When the research operation returns, its guard closes the Google tab and all
non-pinned research tabs, even if some page extraction fails.

For explicit cleanup of earlier disposable tabs, use a policy-gated cleanup call.
It only selects CDP targets whose `type` is `page`, always preserves protected
`chrome://`/extension/DevTools targets unless they are explicitly requested, and
honors target-ID and URL-prefix keeps:

```json
{
  "op": "cleanup_tabs",
  "close_url_prefixes": [
    "http://127.0.0.1",
    "https://www.google.com/search"
  ],
  "keep_url_prefixes": [],
  "keep_target_ids": [],
  "limit": 200
}
```

`open_many` is intentionally short-lived by default. Pass `"keep_open": true`
when opening result tabs for the user is the actual task (as
`dsl/browser/google_search_top10.json` does).

Both `research` and `open_many` open pages concurrently. The default is **5
tabs**; set `tab_concurrency` (alias: `concurrency`) to change it, bounded to
`1..=10`. Chrome target creation and Laya-owned-page reclamation are serialized to
keep the ownership registry consistent, while the page work runs in parallel:

```bash
laya-workflow run \
  --spec dsl/browser/google_search_top10.json \
  --query "内容" \
  --state '{"tab_concurrency":5}'
```

As an explicit, conservative sweep, `"dedupe_urls": true` keeps one tab for each
exact URL and closes only later duplicates. It still skips non-page targets and
protected schemes unless explicitly requested.

## Jev-style planner step

A model should receive `state.goal`, the snapshot URL/title/text and element
table, then return strict JSON:

```json
{
  "operation": "CLICK",
  "element": 4,
  "goal_probability": 0.31,
  "stuck_probability": 0.05
}
```

Then pass that object to the execution capability:

```json
{
  "kind": "call",
  "capability": "chrome",
  "with": {
    "op": "agent_step",
    "target_id": "${state.target_id}",
    "decision": "${state.model_decision}"
  }
}
```

Supported operations are `CLICK`, `TYPE_TEXT`, `SELECT`, `SCROLL_DOWN`,
`SCROLL_UP`, `PRESS_ENTER`, `KEY`, `WAIT`, `DONE`, and `BLOCKED`. `DONE` with
`goal_probability < 0.5`, or `BLOCKED` with `stuck_probability < 0.5`, is
withheld instead of acted on.

The planner can be any OpenAI-compatible `llm` capability. Keeping planning in
the workflow and execution in `chrome_cdp` makes the decision auditable and lets
multiple workflow threads share the one browser safely.

## Safety

- `policy.allow_hosts` governs the CDP endpoint, newly opened URLs, and the URL
  of every target inspected by this capability.
- Launching Chrome requires `policy.allow_exec`.
- If `policy.allow_paths` is non-empty, the configured/default profile must be
  under one of those roots.
- Do not expose port 9222 beyond loopback. The local Chrome profile has your
  authenticated sessions.

## Verified round-trip

The repository includes an opt-in real-browser check. It launches one foreground
Chrome with the bundled extension, types with CDP key events, clicks with CDP
mouse events, then starts two concurrent workers on separate targets through the
same CDP endpoint:

```bash
cargo test --lib capability::browser::real_browser_tests::real_chrome_singleton_observes_and_drives_page -- --ignored --nocapture
```

## Manual extension use

If you prefer an existing profile, start Chrome once with
`--remote-debugging-port=9222` and load `extensions/laya-browser` through
`chrome://extensions`. Then set `launch: false`; Laya will reuse that exact
instance and refuse to launch a second one.
