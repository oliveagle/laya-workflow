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
  push the navigation out of the driven tab. With `stay_in_tab: true`, `find`
  normalizes all three routes back into this tab - a `target=_blank` link, a
  `window.open(url)` call, and a `<button type=submit>` inside a
  `<form target=_blank>` - so the tour keeps one tab (the click stays the engine's
  real CDP input; only the landing tab changes). See the sweep section below for
  why the form route needs its own handling.
* **Async pagination.** A pager click changes the URL a beat later, and a list
  still fetching renders a skeleton pager. `observe` with `prev_url` waits for
  the URL to actually change, and `find` with `retry_ms` re-scans while the list
  loads. `find` additionally falls back to a CSS `selector` when a list is
  virtualised and the numbered snapshot has no match.

A Laya quirk worth knowing: a string that is *only* `${state.x}` plus surrounding
whitespace is treated as one placeholder and the whitespace is dropped, so the
journal line is `"- ${state.step_rationale}\n"` - the leading `- ` keeps the
trailing newline alive.


## The comparison sweep: `taobao_ddr5_sweep`

`taobao_jev_tour` opens *one* product. `dsl/browser/taobao_ddr5_sweep.json` is its
**comparison** twin: fill the box, search, sort by 销量, walk three result pages,
then open **20 product details one by one in the same tab**, read each, score it
for 单条服务器内存条, and finally navigate to - and stay on - the single most
recommended listing.

```bash
laya-workflow run --spec dsl/browser/taobao_ddr5_sweep.json --progress
```

Every demo element is an engine node: **填选择框** (`TYPE_TEXT` into the search box,
addressed by number), 点 `搜索`, **弹出选择框的序号 / 筛选项选择** (`observe` then click
the `销量` tab by number), **多页浏览** (scroll, then `下一页` twice: page 1 → 2 → 3),
**打开商品详情** (20 × card → detail → re-observe). The `planner.sweep` op drives the
walk: `page/cards.js` reads the result list, `page/item.js` reads each detail and
returns one comparable row (title, price, 服务器/ECC/RDIMM flags, capacity, DDR5
frequency, and a single 0-20 score), and the op ranks them, tie-breaking toward a
32 GB single DIMM and then the lower price. The 20 rows land in
`~/tmp/jev-deep/sweep.jsonl`; the conclusion in `~/tmp/jev-deep/sweep_tour.jsonl`:

```text
- 比较了 20 个商品；最推荐：三星SK海力士镁光DDR5服务器内存4800B 5600REGECC 32G64G96G128G（score 16, ¥1000）
```

The scratch tab is opened with `keep_open: true`, so the run ends with that product
page still on screen instead of closing the tab it owned.

### Input that moves like a person

A real CDP click is a burst of `Input.dispatchMouseEvent`; raw, that burst is the
signature of a bot, and Taobao answers it with a wall. `src/capability/human.rs`
shapes every action the `chrome_cdp` capability issues (`"human": true` is the
default; a per-call `with.human` overrides it): a click is a bowed, eased mouse path
(9-24 points) with a hover beat and a press-hold, typed text arrives key by key with
longer pauses at spaces and punctuation, and a scroll is a run of 4-10 wheel notches
at the remembered cursor. Reaction (140-420 ms), hover (90-260 ms), hold (55-150 ms)
and inter-key gaps are all jittered by a small xorshift RNG. The click also brings
the page to the front first and then waits for `document.hasFocus()` - Chrome
silently drops synthetic mouse events sent to an unfocused page while still
reporting success, so a click that is not awaited behind focus is a click that never
happened.

### Anti-bot walls: wait them out

Taobao answers a client it distrusts with a `J_MIDDLEWARE_FRAME_WIDGET` overlay
(滑块 / click-the-image / drag-drop captcha, or "访问太频繁") or a whole-page
`punish` deny. `page/wall.js` detects it and `planner.guard` waits it out the way a
person backs off - escalating 6 s / 12 s / 18 s cooldowns, re-navigating to a clean
URL unless the page is a `punish` / `x5sec` / `_____tmd_____` trap, up to
`with.wait_ms`. The sweep runs the same wall check before it records a row, so a
blocked detail page is retried with a cooldown, never scored as a bogus product.

### `stay_in_tab` is a form problem, not just a `window.open` problem

`find`'s `stay_in_tab` normalizes the three ways a site can push the navigation out
of the driven tab: a link carrying `target=_blank`; a JS handler calling
`window.open` (Taobao's product cards); and - the one that actually bit us - a
`<button type=submit>` inside a `<form target=_blank>`, which is exactly what 搜索
is. The site's own JS rewrites that form's target to `_blank` after hydration, and
a form submit is a *browser-native* new-tab navigation that never runs
`window.open`, so patching `window.open` alone let the results open in a second tab
while the driven tab was left on a tracking page. `find` now also pins the owning
form's `target` to `_self` and re-asserts it on `submit` for the click window.

## Files

- `dsl/browser/jev_deep_dive.json` — the workflow.
- `plugins/jev-planner/` — the planner: `plugin.json`, `main.rhai`, `page/plan.js` (roam),
  `page/find.js` + `page/observe.js` + `page/dismiss.js` (the directed tour).
- `dsl/browser/taobao_jev_tour.json` — the directed Taobao tour.
- `dsl/browser/taobao_ddr5_sweep.json` — the directed Taobao **comparison sweep**
  (20 product details, ranked; `plugins/jev-planner/page/{cards,item,wall}.js`).
- `docs/browser_singleton.md` — the `chrome_cdp` operations and the singleton rules.
- `docs/plugins.md` — the plugin host API and layout.
