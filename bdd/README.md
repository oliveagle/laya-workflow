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
| `When I open/navigate/click/type/select/run javascript` | act | one `chrome_cdp` op |
| `Then …` | assert | `browser_base.assert` with a `checks` map |

`target_id` is carried in state, so every step after the `Given` operates on
the *same tab* — a `When`/`Then` pair is a real interaction with one page, not
two independent page loads. A step that needs a tab but has none is a compile
error naming the step to add, never a silent fresh page.

The Gherkin text is copied into the spec's `description`, so a generated spec
points back at the document it came from.

## Step vocabulary

Anything not in this table is a compile error. A BDD step that quietly becomes
a no-op is a test that always passes, which is worse than a hard error.

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

* `scripts/verify.sh` (default, also CI) — compiles every `.feature` and checks
  each generated spec passes `laya-workflow validate`. No Chrome, no network,
  ~0.3s. This catches a step that stopped parsing or a scenario that compiles to
  a spec the engine rejects.
* `python3 scripts/bdd/run.py` — the CDP run. Needs a local Chrome, so it is an
  explicit local gate rather than part of the push gate. It is not wired into
  CI, which is offline and has no browser installed; adding one is a separate
  decision, not something to smuggle in here.

## Known limits

* Step definitions are not reusable across features. Copy-paste is the only
  sharing mechanism today; `$ref`/`include` would be the fix, and it is a
  bigger change than a transpiler.
* `checks` is a flat key → expected-value map, so an assertion is one
  expression. BDD's `Then` is more expressive; the mapping is lossy in that
  direction.
* The first mismatch throws, so a scenario reports one failure. BDD runners that
  aggregate soft failures per scenario would report more.
* One `Scenario` cannot branch mid-run. The DSL is a decision DAG and the
  transpiler uses that for the happy path only; a conditional step would need
  real edge routing.
