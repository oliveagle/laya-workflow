# Continuous deep use of one page (`jev_deep_dive`)

`dsl/browser/jev_deep_dive.json` is the [Jev-for-Chrome](https://github.com/chy4pro/jev-for-chrome)
idea driven from Laya: **one tab, many steps, a typed decision per step**. Every
round the workflow observes the page, asks for one
`{operation, element | selector, goal_probability, stuck_probability}` decision,
executes it with *real* CDP input, journals it, and goes again. The tab is never
replaced: Laya keeps the same target the whole run.

The demo behaviour is deliberately generic and works on most article-like pages:
**read the page you are on (scroll), then follow one in-content link to go
deeper, then read that one** — and keep going until the workflow's iteration
budget runs out (or the planner returns `DONE` because the page has no
unfollowed links left).

## Run it

```bash
# friendly form: --query is the seed URL
laya-workflow run --spec dsl/browser/jev_deep_dive.json \
  --progress --query "https://en.wikipedia.org/wiki/Jevons_paradox"

# explicit form: --state carries url (and an optional goal)
laya-workflow run --spec dsl/browser/jev_deep_dive.json \
  --progress \
  --state '{"url":"https://en.wikipedia.org/wiki/Jevons_paradox",
            "goal":"read this page through, then go one level deeper"}'
```

`--progress` streams one line per node (`step`, `route`, `act`, `journal`) to
stderr, so you can watch the loop advance while the owned Chrome window scrolls
and clicks. The final JSON verdict on stdout carries the last decision plus the
accumulated state.

## What you get

Every round appends to `~/tmp/jev-deep/rounds.jsonl` — two JSON objects per
round: the **plan** (what the planner saw and decided) and the **act** result
(what `chrome_cdp` actually did).

```jsonc
{"round":14,"goal":"…","url":"https://en.wikipedia.org/wiki/Jevons_paradox",
 "title":"Jevons paradox - Wikipedia","operation":"CLICK",
 "selector":"[data-laya-pick=\"1\"]","element":null,
 "goal_probability":0.55,"stuck_probability":0.1,
 "rationale":"round 14: page read to the bottom; go deeper through \"19th-century Manchester\"",
 "page":{"scroll_y":7800,"scroll_height":8598,"viewport":800,"at_bottom":true,
         "text_len":28840,"headings":21,"links_total":60,"links_fresh":60,"links_followed":0},
 "next_url":"https://en.wikipedia.org/wiki/Cottonopolis","next_label":"19th-century Manchester",
 "line":"{…same object, used as the journal line…}"}
{"capability":"chrome_cdp","op":"click","clicked":true,"planned":{"operation":"CLICK",…}}
```

Read it back:

```bash
# what did each round decide?
jq -rc 'select(.operation) | "r\(.round) \(.operation) y=\(.page.scroll_y)/\(.page.scroll_height) \(.title) | \(.rationale)"' \
  ~/tmp/jev-deep/rounds.jsonl

# which pages did it actually visit?
jq -rc 'select(.operation=="CLICK") | "\(.title) -> \(.next_label) (\(.next_url))"' \
  ~/tmp/jev-deep/rounds.jsonl
```

`~/tmp/jev-deep/rounds.jsonl` is appended to, so delete it to start a clean
transcript. The round counter (and the "already followed" link set) live in the
tab's `localStorage` and are cleared whenever a run visits its seed URL, so round
numbers are per run.

## The decision contract

`agent_step` (see [`browser_singleton.md`](./browser_singleton.md)) accepts
`CLICK`, `TYPE_TEXT`, `SELECT`, `SCROLL_DOWN`, `SCROLL_UP`, `PRESS_ENTER`, `KEY`,
`WAIT`, `DONE`, `BLOCKED`; an element is addressed by a snapshot `element` index
*or* a CSS `selector`. `DONE` with `goal_probability < 0.5` and `BLOCKED` with
`stuck_probability < 0.5` are withheld rather than executed — the probabilities
are a real gate, not decoration.

This demo runs without the observation extension (it addresses the tab through a
CSS selector it marks itself: `[data-laya-pick="1"]`), so it needs nothing beyond
the singleton Chrome. With `extensions/laya-browser` loaded, a planner can click
by `element` index from `snapshot` instead.

## Where the planning lives

| piece | file | why there |
|---|---|---|
| the loop (observe → decide → execute → journal) | `dsl/browser/jev_deep_dive.json` | data: node graph, edges, policy |
| wait-for-settle + observe plumbing | `plugins/jev-planner/main.rhai` | script: it is policy, and it changes often |
| "what counts as a content link", "when is a page read" | `plugins/jev-planner/page/plan.js` | page-side, readable, no rebuild |
| directed "intent -> numbered element -> decision" (`find`) | `plugins/jev-planner/page/find.js` | page-side; one file serves every site |
| numbered table + overlay (`observe`), modal close (`dismiss`) | `plugins/jev-planner/page/observe.js`, `page/dismiss.js` | page-side helpers the tour calls |

The plugin is called through `kind: "plugin"` with `browser: "chrome"`, so it can
only *ask* the engine to evaluate JS on the tab; the engine still owns the
singleton, the host allow-list and the tab lifecycle, and `chrome_cdp` still owns
the execution of the decision. `main.rhai`'s `_settle` exists because the
engine's own `wait_ready` can return on the pre-navigation `about:blank` target;
polling for a stable URL is what makes the loop deterministic across clicks.

## Swap in a model

The demo's planner is deterministic so the run is reproducible offline. To let a
model make the same decisions, keep the `step` node's shape and replace the
`planner` capability with an `llm` capability whose prompt carries the snapshot
(`url`, `title`, `text`, element table) and whose response is parsed into the
same object:

```jsonc
"planner": {
  "kind": "llm",
  "model": "${env.LAYA_MODEL}",
  "prompt": "You are the decision half of a Jev loop. Page: ${state.step_url} …",
  "response_format": "json"
}
```

`project: {"decision": "/json", "step_action": "/json/operation"}` then feeds
`agent_step` exactly as the plugin does today, and the `route` node's heuristic
(`field: "step_action"`, `match_any: ["DONE","BLOCKED"]`) still stops the loop.
Nothing else in the graph changes — the point of the split is that planning and
execution can be swapped independently.

## The directed tour: `taobao_jev_tour`

`jev_deep_dive` **roams** - the page-side policy decides what to click.
`dsl/browser/taobao_jev_tour.json` is its **directed** twin: one ordered *intent*
per round, resolved to a numbered element and executed the same way, in the same
one tab. It is the concrete demo of 持续深度使用一个网页 on Taobao (淘宝):

1. **填选择框** - type `DDR5 内存 16G` into the search box (`TYPE_TEXT`), addressed by number.
2. 点 `搜索` -> the results page.
3. **弹出选择框的序号 / 筛选项选择** - draw the numbered overlay (`observe`) and click the
   `销量` tab by number.
4. **多页浏览** - scroll the list, then click `下一页` twice: page 1 -> 2 -> 3.
5. **打开单个商品详情** - pick the first product card by number (a link whose href is
   `item.taobao.com` / `detail.tmall.com`) and click it; the tour follows into the
   item page and keeps reading it (scroll + re-observe).

```bash
laya-workflow run --spec dsl/browser/taobao_jev_tour.json --progress
```

Each action is one engine node whose `chain` runs `planner.find` first and feeds
`${with.dec}` to `chrome.agent_step`, so resolution and execution stay separate
capabilities while the graph stays one node per step. `planner.observe` draws the
numbered overlay and returns the numbered table; `planner.dismiss` closes a
full-viewport promo modal that would otherwise swallow every click. The run
journals to `~/tmp/jev-deep/tour.jsonl`:

```text
- 填选择框：把关键词打进搜索框 — round 0: TYPE_TEXT #13 (combobox "请输入搜索文字") with text "DDR5 内存 16G"
- 点搜索 — round 0: CLICK #12 (button "搜索")
- 多页浏览：翻到第 2 页 — round 0: CLICK #16 (button "下一页，当前第1页")
- 多页浏览：翻到第 3 页 — round 0: CLICK #16 (button "下一页，当前第2页")
- 序号选商品：打开第一个商品详情 — round 0: CLICK #27 (link "海力士16G 1R8 PC5 4800RECC ECC REG DDR5服务器内存 5代内存条")
```

Three problems a "click #N" driver hits on a real site, all solved generically in
`page/find.js` rather than in the spec:

* **A number is not a click target.** The extension's snapshot also lists
  off-screen and shadowed elements - Taobao numbers a tiny pager arrow that lives
  beyond the right edge, and clicking it is a silent no-op. `find` scrolls each
  match into view and requires its centre to hit the element itself (or a child),
  so a stale number turns into "pick the next match", never into a stalled run.
* **Some sites force a new tab.** Taobao's product cards *and* its `搜索` button
  call `window.open`. With `stay_in_tab: true`, `find` drops `target` and routes
  that one `window.open(url)` back into the driven tab, so the tour keeps one tab
  (the click stays the engine's real CDP input - only the landing tab changes).
* **Async pagination.** A pager click changes the URL a beat later, and a list
  still fetching renders a skeleton pager. `observe` with `prev_url` waits for
  the URL to actually change, and `find` with `retry_ms` re-scans while the list
  loads. `find` additionally falls back to a CSS `selector` when a list is
  virtualised and the numbered snapshot has no match.

A Laya quirk worth knowing: a string that is *only* `${state.x}` plus surrounding
whitespace is treated as one placeholder and the whitespace is dropped, so the
journal line is `"- ${state.step_rationale}\n"` - the leading `- ` keeps the
trailing newline alive.


## Files

- `dsl/browser/jev_deep_dive.json` — the workflow.
- `plugins/jev-planner/` — the planner: `plugin.json`, `main.rhai`, `page/plan.js` (roam),
  `page/find.js` + `page/observe.js` + `page/dismiss.js` (the directed tour).
- `dsl/browser/taobao_jev_tour.json` — the directed Taobao tour.
- `docs/browser_singleton.md` — the `chrome_cdp` operations and the singleton rules.
- `docs/plugins.md` — the plugin host API and layout.
