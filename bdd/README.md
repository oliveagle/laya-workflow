# BDD documents, executed by laya-workflow against real Chrome

A Gherkin `.feature` file is the source of truth for a browser scenario.
`scripts/bdd/transpile.py` compiles each `Scenario` into a laya-workflow
spec, and `scripts/bdd/run.py` runs those specs against a real headless Chrome
over CDP. Nothing in the assertion path is judged by a language model: the
expression runs in the page, the result is compared key-by-key, and a mismatch
throws and fails the run.

```bash
python3 scripts/bdd/run.py                    # every scenario, real Chrome
python3 scripts/bdd/run.py --filter outline   # one feature, by substring
python3 scripts/bdd/run.py --keep             # keep the generated specs
python3 scripts/bdd/transpile.py bdd/features/*.feature --list
```

Current state: **5 scenarios, all green in ~7s**, hermetic — the only HTTP
traffic is a local fixture server on a random port.

## How a Scenario becomes a spec

One `Scenario` is one spec, because one spec is one thing a laya-workflow run
can execute and pass or fail. Each step becomes exactly one node:

| Gherkin | Node | What runs |
|---|---|---|
| `Given the browser is ready` | routing only | declares the browser usable |
| `Given I am on "<url>"` | act | `chrome_cdp` `open`, keeps `target_id` |
| `When I open/click/type/select` | act | one `chrome_cdp` op (real input) |
| `When I navigate/wait for/run javascript` | act | one `bdd` plugin op |
| `Then …` | assert | one `bdd` plugin `assert` op, named assertion |

`target_id` is carried in state, so every step after the `Given` operates on
the *same tab* — a `When`/`Then` pair is a real interaction with one page, not
two independent page loads. A step that needs a tab but has none is a compile
error naming the step to add, never a silent fresh page.

The Gherkin text is copied into the spec's `description`, so a generated spec
points back at the document it came from.

## The `bdd` plugin

`plugins/bdd/` is a standard plugin — `plugin.json` + `main.rhai`, no
registration step, and no checkout needed: it is also compiled into the binary
as a built-in, so a spec that says `"plugin": "bdd"` resolves from a shipped
executable as well as from a working tree. It is the reason the assertion
vocabulary is reusable: the *meaning* of a `Then` lives in Rhai, not in the
Python transpiler, so a hand-written spec gets the same checks a compiled
`.feature` does:

```json
"capabilities": {
  "chrome": { "kind": "chrome_cdp", "…": "…" },
  "bdd":     { "kind": "plugin", "plugin": "bdd", "browser": "chrome" }
}
```

```json
{ "kind": "call", "capability": "bdd",
  "with": { "op": "assert", "assertion": "visible",
            "value": "#echo", "target_id": "${state.target_id}" } }
```

Ops: `open`, `navigate`, `wait_for`, `evaluate`, `assert`, `release`. The
capability leaves `op` unset so the engine prefers `with.op` per call.

That it is a *standard* plugin is checked, not asserted:
`dsl/browser/bdd_assert_probe.json` is a hand-written spec — no Gherkin, no
transpiler — that drives the same ops, and `scripts/bdd/run.py` runs it against
the same fixture as the scenarios. If the vocabulary ever stops being usable
outside the compiler, that row goes red.

Assertions: `title_contains`, `url_contains`, `visible`, `absent`, `is_true`,
`is_false`, `equals`, `contains`, `equals_text`. A failure names the assertion
*and* the value it saw, e.g.

```
bdd.assert: FAIL visible with value "#definitely-not-here" | actual: "no such element"
```

`assert` runs **one** attempt by default. A deterministic check that is going to
fail should say so immediately; the way to absorb a page that is still settling
is an explicit `wait_for` before it, which is visible in the document. Pass
`with.attempts` to opt into retries.

## Step vocabulary

Anything not in this table is a compile error. A BDD step that quietly becomes
a no-op is a test that always passes, which is worse than a hard error — and
so is a step that quietly becomes the *wrong* step. `scripts/bdd/vocabulary_check.py`
pins every step in the table to the op it is supposed to run, and runs in the
default gate with no Chrome:

```
bdd vocabulary: 19 steps map correctly, 8 operand types survive,
                 8 steps still refuse to run without a page
```

It exists because that failure is invisible otherwise. `Then javascript "a"
equals text "x"` used to be swallowed by the `equals` rule — whose `.+` was
happy to eat `text "x"` — so it compiled into a strict comparison against the
literal string `text "x"`. Green run, meaningless document.

`equals` is strict about types, and the operand keeps the type the document
gave it: `equals 3` is a number, `equals "3"` is a string. That distinction is
load-bearing enough to have its own `@expected_failure` scenario, because
`json.loads` on the quoted `"3"` used to quietly turn it back into a number.

**Given** — `the browser is ready` · `I am on "<url>"`

**When** — `I open "<url>"` · `I navigate to "<url>"` ·
`I wait for the element "<selector>"` · `I click the element "<selector>"` ·
`I type "<text>" into the element "<selector>"` ·
`I select "<value>" in the element "<selector>"` ·
`I run javascript "<expression>"`

**Then** — `the page title contains "<text>"` · `the page url contains "<text>"` ·
`the element "<selector>" is visible` ·
`javascript "<expression>" is true|false` ·
`javascript "<expression>" equals <json>` ·
`javascript "<expression>" contains "<text>"`

`equals` takes JSON, so `equals 42`, `equals true` and `equals "complete"` all
mean what they look like.

## `@expected_failure`

A scenario tagged `@expected_failure` must fail. It is the guard against vacuous
assertions: if a broken page ever made it pass, the checks above would report
success no matter what the page did, and the suite reports that as a failure.

```
  PASS           page_smoke.feature :: A page that loads is assertable  [0]
  xfail          page_smoke.feature :: Asserting a missing element fails the run  [1]
```

## Scenario Outlines

Each `Examples:` row becomes its own spec and its own Chrome run; the
placeholder is substituted into the step text *and* the scenario name, so the
run log reads `Greet Ada in blue` rather than `Greet <person> in <colour>`.

## State and configuration

`<name>` in a step becomes a state key the engine interpolates at run time
(`<base_url>` → `${state.base_url}`). The identifier shape is deliberate:
`i < 10` inside a `Then javascript` step is not mistaken for a placeholder.

The runner supplies `base_url`, `cdp_port` and `cdp_profile`; the port is
chosen free at run time and the profile is a private temp dir, so a BDD run
never collides with a browser somebody else is using. (A hard-coded port is a
lie — this machine already had an unrelated `chrome-headless-shell` on 9223,
and the capability correctly refused to adopt an instance it had not launched.)
A sidecar
`<feature>.config.json` next to a feature adds `initial_state` (the default for
an outline column), `policy` overrides, and `chrome` overrides — for example
`{"chrome": {"human": true}}` to exercise the humanised input path.

## Two settings that are not defaults, on purpose

Both are set in the generated spec, and both have a measured failure behind
them rather than a hunch.

**Headless, via `"chrome_binary"`** pointing at `scripts/bdd/chrome-headless.sh`.
Chrome does not deliver synthetic `Input.dispatchMouseEvent` to a page whose
`visibilityState` is `"hidden"`, and a headful window that was never brought to
the front reports exactly that. The CDP call still returns success, so a
scripted click appears to work while the page never sees it — a scenario that
clicks and asserts is testing nothing. Measured, not assumed:
`document.visibilityState` was `"hidden"` while `elementFromPoint` still
returned `BUTTON#greet` at the very coordinates being clicked.

The wrapper `exec`s the real Chrome with `--headless=new` prepended. It uses
`exec` deliberately: the pid is preserved, so the capability's owner marker and
process-command-line check still identify the instance, and no wrapper process
lingers holding the profile lock. It resolves the real binary from
`$LAYA_BDD_REAL_CHROME`, then `$CHROME_BIN`, then the platform paths the engine
uses. A `headless` field on `BrowserCap` would be the tidier home for this; the
wrapper is what avoided reaching into the capability's launch path.

**`"human": false`**. Human-shaped input is the right default for driving
someone's browser and the wrong one for a test: a random-walk cursor makes
every click a different gesture, so a scenario is not reproducible and a timing
flake reads as a product bug. The humanised path also pushed a 6-command click
sequence past the 15s capability timeout.

## What is gated, and where

**`scripts/bdd/check.sh`** is the one definition of "the BDD documents still
build": compile every `.feature`, then `laya-workflow validate` each generated
spec. No Chrome, no network, ~0.3s. Two callers, so they cannot drift:

* `scripts/verify.sh` — the pre-push gate.
* `.github/workflows/ci.yml` — a step in the offline job.

### Cost, and why it is serial

`run.py` prints the slowest scenarios, because the cost is not where you would
guess. Process startup and spec parsing are ~3ms (`validate` × 20 = 62ms).
Almost all of a scenario's ~0.3s is Chrome/CDP round-trips.

Two things that were measured rather than assumed:

* **Chrome reuse is the whole ballgame.** A scenario costs ~1.9s when it
  launches a fresh Chrome and ~0.75s when one is already running on the same
  port and profile. The runner therefore hands every scenario the *same* CDP
  endpoint, so only the first one pays for a launch. A private profile dir per
  run is still required — a fixed one collides with the interactive Chrome.
* **Parallel execution is not the win it looks like.** `--jobs N` works and is
  clean, but on the machine this was measured on the box was at load 20–40 from
  other work, and serial runs ranged 7.8s–33.9s on identical code. No honest
  speed claim can be made from that, and one `--jobs 2` run failed where serial
  never did. So the default stays 1 and the flag is opt-in and unvalidated.

The real efficiency bug was a leak. An interrupted run left its headless Chrome
alive; nine survivors were found, each holding a renderer, and they were enough
to push the machine's load average from 31 to 58. Every run after that was
slower than it should have been, and the timing noise made the numbers above
harder to read. The runner now kills and removes what it started in a
`finally`, and says so if it cannot:

```
after a normal run: 0 dirs, 0 chrome procs
after SIGINT mid-run: 0 dirs, 0 chrome procs
```

**`python3 scripts/bdd/run.py`** is the CDP run. It needs a local Chrome, so it
is an explicit local gate and deliberately *not* in CI: the runners install no
browser, and adding one is a separate decision rather than something to smuggle
in here.

## Known limits

* Step *meanings* are reusable (they live in `plugins/bdd/main.rhai`, not in
  the transpiler), but step *text* is not: the same scenario still has to be
  written out per feature. `$ref`/`include` would be the fix, and it is a
  bigger change than a transpiler.
* An assertion is one named check (`title_contains`, `visible`, `equals`, …),
  not an arbitrary expression. `Then javascript "…"` covers the general case,
  but a vocabulary you can enumerate is easier to keep honest than one you
  cannot.
* The first mismatch throws, so a scenario reports one failure. BDD runners that
  aggregate soft failures per scenario would report more.
* The `bdd` plugin's `open` op is only usable **within one plugin call**: the
  engine closes any tab a plugin opened as soon as that call returns
  (`src/capability/plugin.rs`, "the engine closes what the plugin opened"). A
  multi-step scenario therefore lets the `chrome_cdp` capability own the page
  and passes the plugin a `target_id` to borrow — which is what the transpiler
  emits, and what `dsl/browser/browser_base_probe.json` does.
* One `Scenario` cannot branch mid-run. The DSL is a decision DAG and the
  transpiler uses that for the happy path only; a conditional step would need
  real edge routing.
