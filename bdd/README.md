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

Current state: **22 scenarios, all green in ~8s**, hermetic — the only HTTP
traffic is a local fixture server on a random port. `scripts/bdd/doc_check.py`
keeps every count on this page recomputed from the gates themselves.

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
executable as well as from a working tree. It is listed in the repo's own
`## Bundled plugins` registry in both `docs/plugins.md` and
`src/skill/sections/plugins.md`, and `scripts/plugin_registry_check.py` fails
the build if a plugin under `plugins/` is missing from either — see "The
registry is a gate" below. It is the reason the assertion
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

`release` reports the number of tabs the engine actually closed and **throws if
that is zero**, so releasing a target that was already closed — or that never
existed — reads as the mistake it is. It used to return `released: true`
unconditionally, which made it the one op in the set that could not fail, and
therefore had nothing in it worth testing.
`dsl/browser/bdd_release_probe.json` is the hand-written spec that holds it to
that, and `run.py` runs it as a *required failure* — see "A test that has to
fail" below.

That it is a *standard* plugin is checked, not asserted:
`dsl/browser/bdd_assert_probe.json` is a hand-written spec — no Gherkin, no
transpiler — that drives the same ops, and `scripts/bdd/run.py` runs it against
the same fixture as the scenarios. If the vocabulary ever stops being usable
outside the compiler, that row goes red.

Assertions: `title_contains`, `url_contains`, `visible`, `absent`, `is_true`,
`is_false`, `equals`, `contains`, `equals_text`.

Those two lists — the ops and the assertions — are not only documentation, they
are the *error messages*. `bdd: unknown op 'x' (open | navigate | …)` and
`bdd.assert: unknown assertion 'x' (…)` are how someone finds out what exists
after they have already got something wrong, so they drift silently when the
vocabulary grows. `vocabulary_check.py` reads the dispatch out of
`plugins/bdd/main.rhai` and compares it to both messages, in both directions:
an op the plugin handles but the message omits, and an op the message
advertises but the plugin does not handle. All three drift shapes were
confirmed to go red. Chrome-free, so CI sees it. A failure names the assertion
*and* the value it saw, e.g.

```
bdd.assert: FAIL visible with value "#definitely-not-here" | actual: "no such element"
```

`assert` runs **one** attempt by default. A deterministic check that is going to
fail should say so immediately; the way to absorb a page that is still settling
is an explicit `wait_for` before it, which is visible in the document. Pass
`with.attempts` to opt into retries.

`wait_for` takes `with.timeout_ms` (default 15000). Both knobs were dead until
Round 8, for one reason: `_num` tested `type_of(v) == "int"` and `"float"`, and
Rhai reports `"i64"` and `"f64"`, so *every* numeric argument silently became
its default. Measured with the broken `_num` compiled in, asking for 600ms:

```
bdd.wait_for: timed out after 15000ms (150 polls) waiting for #never-going-to-appear
```

25× the budget, and 16.5s instead of 1.8s. Every other plugin in the tree
already checks `"i64"`/`"f64"`; the alias is kept alongside so it cannot be the
failure again. `wait_for` also measures real elapsed milliseconds now — it used
to add a hardcoded 100 per iteration, ignoring both the sleep and the probe
round trip, so every number it reported under-counted, which is the direction
that makes a timeout look like it gave up early.

## Step vocabulary

Anything not in this table is a compile error. A BDD step that quietly becomes
a no-op is a test that always passes, which is worse than a hard error — and
so is a step that quietly becomes the *wrong* step. `scripts/bdd/vocabulary_check.py`
pins every step in the table to the op it is supposed to run, and runs in the
default gate with no Chrome:

```
bdd vocabulary: 21 steps map correctly, 17 step arguments survive, 8 operand types
                 survive, 8 steps still refuse to run without a page, 2 stay refused
                 after a release, every @expected_failure says what it disproves, the
                 plugin's own vocabulary messages are in sync, and its header documents
                 every op, assertion and default it has
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
`I run javascript "<expression>"` · `I release the page`

`I release the page` is the one step that changes what a *later* step may do:
it closes the tab, so the compiler moves to its has-no-page state and any
following step is a compile error rather than a CDP failure at run time. Both
halves of that are pinned in `vocabulary_check.py`.

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

It must also say **why**, in the tag:

```
@expected_failure(bdd.assert: FAIL equals)
```

Failing is not enough. The runner used to accept any non-zero exit, so a
scenario that died on a typo'd `Given`, a Chrome that would not start, or a 404
from the fixture counted exactly like one that failed for the reason it was
written to demonstrate — the tag was then a comment rather than a contract. Now
the declared text has to appear in the failure, or the scenario is reported
`XFAIL-WRONG-REASON`:

```
  PASS      page_smoke.feature :: A page that loads is assertable  [0]
  xfail     page_smoke.feature :: Asserting a missing element fails the run  [1]
  XFAIL-WRONG-REASON  assertions.feature :: equals must reject the wrong value  [1]
      it failed, but never said 'bdd.assert: FAIL title_contains', so it
      failed for some other reason
```

A bare `@expected_failure` is a hard error, checked **without Chrome** by
`vocabulary_check.py`, so it cannot be reintroduced in a commit that CI accepts.

```
  PASS           page_smoke.feature :: A page that loads is assertable  [0]
  xfail          page_smoke.feature :: Asserting a missing element fails the run  [1]
```

## The registry is a gate

"BDD is a standard plugin" was a claim in this file and nowhere else. The repo
keeps a registry for exactly that claim — the `## Bundled plugins` table in
`docs/plugins.md` and in `src/skill/sections/plugins.md` — and measured, that
table was missing three of the four directories under `plugins/`:

| plugin | docs/plugins.md | skill table |
|---|---|---|
| `plugins/bdd` | absent | absent |
| `plugins/jev-planner` | absent | absent |
| `plugins/browser_base` | listed | absent |

So a plugin could be written, compiled, gated, run and shipped while being
invisible to the one document whose job is to say what exists. `bdd` was the
one this file is about.

`scripts/plugin_registry_check.py` compares the directories under `plugins/`
against both tables, in both directions — a plugin that ships and is not
listed, and a row pointing at a directory that no longer exists. It runs in
`scripts/verify.sh` *and* as its own CI step, because CI does not invoke
`verify.sh`, and a check wired only into `verify.sh` is a check that runs on
one machine and nowhere else.

Scope is `plugins/` only, deliberately. Those four are the tool plugins, where
"a standard plugin" is the actual question, and they are all stable. The
`websites/` tree grows faster, some directories are named differently from the
table (`websites/developer.mozilla.org` is listed as `websites/mdn`,
`websites/huggingface.co` as `websites/hf-trending`), and some are mid-flight —
requiring all of them would make this a nuisance rather than a gate.

## A test that has to fail

`@expected_failure` covers *assertions*. It cannot cover an *op*, because an op
is not a document: to prove `release` refuses a target that is not open you have
to write a spec that releases twice, and the Gherkin vocabulary deliberately
refuses to compile that — after a release the compiler knows there is no page,
so the second release is a compile error, not a run.

So the probe is hand-written (`dsl/browser/bdd_release_probe.json`) and
`run.py` inverts the verdict for it: `rc == 0` is the *failure*, and the run
has to name the target it could not close or it does not count. A green-only
suite cannot express that, and here it is not hypothetical — measured with the
pre-fix `release` compiled in:

```
  PASS           release.feature :: Releasing closes the tab the scenario was using  [0]
  FAIL           hand-written dsl/browser/bdd_release_probe.json  [0]
      the second release was reported as success - bdd.release is claiming
      it closed a page that was already closed
```

The Gherkin scenario **passed in both builds**. It is the required failure that
carries this, and it cost one extra spec against a Chrome that was already
running.

Three of the four hand-written probes work this way. `dsl/browser/bdd_wait_probe.json`
waits for an element that never appears and must fail *and* name the budget, the
selector and the real elapsed time; `dsl/browser/bdd_retry_probe.json` asserts a
late element with `with.attempts` and must **succeed**, with the losing half of
that race as an `@expected_failure` scenario in `assertions.feature` (one attempt
probes at ~0ms and cannot win; eight attempts reach 2.1s and can). Neither half
means anything alone: a suite with only the winner cannot tell a working retry
from a lucky page, and one with only the loser cannot tell a working retry from
a missing knob.

The fixture's late element is a `setTimeout`, and this page is a background tab,
so Chrome throttles its timers into ~1s buckets. Measured, the note arrived at
944ms / 840ms / 998ms for timer delays of 300 / 600 / 1000ms, and 1674ms on
another run. **A sub-second timer delay in this fixture is indistinguishable
from a 1s one**, which is why the delay is 1000ms and the probe asks for eight
attempts rather than the five the arithmetic suggests. Don't tidy that margin
down to the nominal numbers.

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
build": check the step table, check the hand-written probe specs, run the
plugin's argument-error probes, compile every `.feature`, then
`laya-workflow validate` each generated spec. No Chrome, no network, ~0.4s. Two
callers, so they cannot drift:

* `scripts/verify.sh` — the pre-push gate.
* `.github/workflows/ci.yml` — a step in the offline job.

The probe specs get their own checker, `scripts/bdd/probe_check.py`, because
`laya-workflow validate` is the wrong tool for them. Measured, a probe whose
`edge` names a node that does not exist **passes** `validate` and then fails at
run time with `error_node_missing` and a zero exit code — which is precisely
how `bdd_release_probe.json` was born broken, and why the runner reported it as
a pass. The checker verifies that every edge target is a real node, every node
is reachable from the start, every `${state.X}` is kept by somebody, and every
capability is declared. All four were confirmed to go red on a deliberately
broken copy.

### The gate that was passing nothing

`scripts/verify.sh` handed `scripts/bdd/check.sh` the **test** binary
(`laya-workflow-tests`) where the **CLI** belongs. Measured, that binary accepts

```
$ target/release/laya-workflow-tests validate --spec /tmp/definitely-not-a-spec.json
0 passed, 0 failed
$ echo $?
0
```

so every `validate` in `check.sh` — all 22 transpiled scenarios and all 4 probe
specs — returned 0 without reading a spec. The line `22/22 scenarios compile and
validate` that `verify.sh` printed was true only in the sense that 22 things had
been counted. `verify.sh` now passes `target/release/laya-workflow`, which errors
on the same input, and `check.sh` refuses up front to run against a binary that
accepts a nonexistent spec.

CI was never affected: its step already passed `./target/debug/laya-workflow`.

It surfaced only because the argument probes assert on an *expected failure*.
Under `verify.sh` all 15 rows "succeeded"; on their own they all refuse
correctly. A gate that only ever checks things pass cannot notice when the
checker itself has been swapped for something that always passes.

## A cited example that was never checked

Four documents point at `dsl/browser/browser_base_probe.json` as the canonical
"chrome_cdp owns the page across nodes" shape — it is the only spec
`plugins/bdd/main.rhai`'s own header tells a reader to look at — and **no gate
touched it**. Round 16 documented that as a known hole; round 17 closed it, and
the spec did not survive being run.

It did not fail an assertion. It did something worse: it passed while the wait it
was written to demonstrate had timed out. Pointed at the shared
`bdd/fixtures/index.html`, whose page has no htmx at all, `wait_htmx` burned its
full 15000ms budget and reported `"htmx_loaded": false` — and nothing looked at
the result, so the workflow routed on to `done` and exited 0. Measured across
three runs: **16.38 / 15.80 / 16.52s**, with `htmx_loaded: false` in all three.
The demo cost more than the other five hand-written probes combined, to show
nothing.

Two changes, one per half of the problem:

* **the spec checks its own wait.** The `assert` node now requires
  `htmx_ok: typeof window.htmx === "object"`. Against the htmx-less fixture the
  run fails with
  `htmx_ok: expected true, got false` — the exact mismatch, by name.
* **the fixture has what the wait looks for.** `bdd/fixtures/htmx.html` defines
  `window.htmx`, so the wait succeeds on its first poll. It is a stand-in, not
  the library: the plugin only tests `typeof window.htmx === "object"`, and
  vendoring real htmx to satisfy a `typeof` would be a dependency with no
  behaviour under test.

`run.py` points it at that fixture and requires all three — exit 0, four nodes
reached, and `"htmx_loaded": true` in the report — so the runner cannot be
satisfied by a run where the wait quietly failed.

Measured after: the probe went **16.4s → 0.43s** (38×) and the whole
`run.py` suite **24.5s → ~9s**. Falsified both ways: against `index.html` the
run exits 1 with `htmx_ok: expected true, got false`; against `htmx.html` it
exits 0 with `htmx_loaded: true`, `htmx_ok: true` and all four nodes.

The spec also joins the Chrome-free gates — `probe_check.py` now lints it
(5 hand-written specs) and `check.sh`'s validate glob is `*probe*` rather than
`bdd_*probe*` (6/6 validate).

### And it caught a blind spot in the doc check

Two counts in this file went stale the instant the probe set widened, and
`doc_check.py` did not notice, because it matches labelled numbers in the gates'
own output and counts written as *words* — "all 4 probe specs", "the four probe
specs" — are prose. One of them was also past tense, describing the old
behaviour as if current. Both corrected by hand, and the lesson is written down
here rather than pretended away: a number-checker that only knows its own labels
will go quiet about every other number on the page.
## The numbers on this page are checked

This README quotes its own gates: the scenario count up top, and the output of
`vocabulary_check.py` and `args_probe_check.py`. Those quotes drift silently,
because nothing recomputed them. Measured on the first run of
`scripts/bdd/doc_check.py`:

* the headline said **`Current state: 5 scenarios`** while
  `bdd/features/*.feature` transpiles to **22** — the number was never wrong
  when it was written, it just stopped being updated;
* the quoted `bdd vocabulary:` block had lost four clauses to an edit and still
  looked like a complete line of output.

`doc_check.py` recomputes every labelled count from the gate that owns it and
compares it with what the README says, and requires each quoted output block to
be **exactly** what the gate prints today — not a prefix of it. That last
distinction is the whole check: a truncated quote is a *prefix* of the real
output, so containment would accept the exact failure that caused this round.
Confirmed red on six deliberate edits — the truncated quote, a reworded quoted
line, a stale scenario count, a stale `17 throw sites` number, a dropped label,
and a quoted line the gate no longer prints — and green on the restored file.

Two details that the first version got wrong, and that mattered:

1. **keyed by label, not by source.** The first version collected every match
   from one checker under one key, so each label compared itself against every
   number on the checker's line: all nine labels reported
   "the README says 21 where the truth is [21, 17, 8, 8, 2]". The single
   genuinely stale count drowned in eight false alarms.
2. **whitespace-insensitive in the right places only.** The page wraps output to
   fit, so both the quoted block and the labels are matched with whitespace
   collapsed — otherwise the next edit that wraps a line makes the check go
   quiet rather than red.

It runs in `check.sh`, so CI runs it: no Chrome, no network.

### What this does not check

The prose *around* the numbers is not machine-checked, and the counts it
recomputes are only as trustworthy as the gates they come from. Counts written as
words ("four probe specs") are not labels it recognises either, which is how two
of them went stale the moment round 17 widened the probe set — see below.
## The header is the contract, so it is checked

`plugins/bdd/main.rhai` opens with a table of its six ops, its nine assertions and
the defaults it takes. That header is what someone reads before writing a spec
against the plugin, and **nothing held it to the code** — `vocabulary_check.py`
checked the two "here is what you can say" *error messages* against the
dispatcher, and the table right next to them was prose.

It had already drifted. The header said of `release`:

> close an owned target. Best effort, never masks a real error.

That is the behaviour round 7 deleted. `_op_release` now throws when the engine
closed nothing, and `dsl/browser/bdd_release_probe.json` exists specifically to
*require* that refusal — so the plugin documented, in its own contract, the exact
behaviour its own probe forbids. A second one sat in `_op_open`, whose comment
promised to "say so here rather than letting a later click fail with no
explanation" about a hidden page, above a return map that carries no visibility
field and never did.

`check_plugin_header()` now compares the header's tables to the code:

* every op the dispatcher handles is documented, and every op documented is
  handled;
* same for the nine assertions;
* every `_num(ctx, "x", N)` default is quoted in the header, with the same number.

All three directions were confirmed red on purpose. Adding an op to the
dispatcher fails; documenting an op that does not exist fails; changing the
header's `15000` to `5000` fails; documenting an assertion the plugin lacks
fails.

What it found, on the first run against the real header: **`with.attempts` had a
default the header never mentioned.** It is the opt-in retry knob, it is
documented in this README, and the plugin's own contract omitted it.

Prose is still not checkable, and the round does not pretend otherwise: the
*wording* of each op is review-held, while the shape and the numbers are machine-
checked. The refusal behaviour the old wording denied is pinned by the release
probe; that pairing is the whole reason the drift was survivable for a while.

## Is `release` allowed at all?

`src/skill/sections/plugins.md` says, under the invariants the host enforces:
**"Never put explicit cleanup in a workflow."** And `bdd.release` is a
user-facing close op. That reads like a contradiction, so it is now resolved in
the invariant itself rather than left for a reader to guess at.

The distinction is `pinned`, and it is in the engine, not the plugin:

```rust
// Enforce a hard ceiling for Laya-owned pages. Stale auto-close pages are
// reclaimed first; pinned pages are never reclaimed, but they still count
// against the ceiling so a pin leak cannot grow without a policy error.
```

`keep_open` pins a tab; the idle-GC filter skips `pinned`; a pinned tab still
counts against `max_owned_pages`; and when the ceiling is hit the engine's own
error says *"close pinned tabs or raise max_owned_pages"*.

So there are two different acts, and only one of them is the forbidden one:

* **tidying up after yourself** — the sweep at plugin-call return and the idle GC
  already do it. Writing a step for it is redundant and can fail for no useful
  reason. That is what the invariant forbids.
* **reclaiming a pinned tab mid-workflow** — the automatic mechanisms
  deliberately do *not* do this, and it is the only in-band way to free a slot
  against a hard ceiling. That is what `release` is.

`release` also still goes through `host.browser_release`, so the engine performs
the close — which is the invariant's actual normative content, and the one that
matters for `keep_open` and the idle GC continuing to work.

## The plugin's own error messages

The four **`bdd_*`** probe specs under `dsl/browser/` all need a real Chrome tab,
so between them they cover the plugin's *browser* half. They cover none of its
argument half — and that half is what a hand-written spec author hits first,
because every one of those checks fires before the plugin touches CDP.

Measured, of the 17 `throw` sites in `plugins/bdd/main.rhai`, 5 were reachable
from a probe and **12 had never been executed by anything**. The messages
included `bdd.navigate: with.url is required`, the shared
`an earlier step must open a page first` guard, and the three `needs
with.expected` variants.

`bdd/args_probes.json` pins them, and `scripts/bdd/args_probe_check.py` runs it:

```
$ python3 scripts/bdd/args_probe_check.py
bdd args probes: 15/15 argument errors refuse with the message they claim,
and all 17 throw sites are accounted for (15 pinned here, 5 declared elsewhere)
```

It is a **table**, not 15 near-identical spec files, because the useful content
of those specs is one `(with, message)` pair each and the rest is boilerplate.

**The part that makes it a gate** is the second clause. Each row carries a
`source` — a substring of the throw statement in `main.rhai` — and the checker
requires every `throw` in the plugin to be claimed by a row or by an `elsewhere`
entry that gives a reason. So this fails:

| what someone does | what the checker says |
|---|---|
| adds an 18th error message | `the plugin throws '...' and nothing claims it` |
| deletes a row | the throw it pinned is now unclaimed |
| rewords a message | that row's `source` now matches 0 throw sites |
| a row claims a message the plugin does not emit | `the run failed without saying ...` |

All four were confirmed red on purpose, and the last two also confirm the rows
are falsifiable rather than merely consistent.

`elsewhere` is not an escape hatch — each entry has to say where the throw *is*
covered, or why it cannot be:

| throw | covered by |
|---|---|
| `bdd.wait_for: timed out after` | `dsl/browser/bdd_wait_probe.json` |
| `bdd.release: no open target` | `dsl/browser/bdd_release_probe.json` |
| `throw last` (the assert mismatch) | `bdd_assert_probe.json` + an `@expected_failure` scenario |
| `bdd: target never reached` | **not covered** — needs a tab that never finishes loading |
| `bdd.open: " + e` (the load-failure catch) | **not covered** — same reason |

Those last two are the honest gap: the fixture server serves whatever is asked
for, so there is no way to make a real tab hang.

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
  the transpiler), and shared setup is reusable too
  (`include: setup/fixture-page.feature`, below). Arbitrary step *text* is
  not: there is no `$ref` with parameters, conditionals or nesting. That is a
  deliberate omission, not a missing feature — measured over all six features
  there are 46 step lines and 37 unique ones, and 14 of the 9 duplicates were
  two boilerplate lines in four files. A general reference mechanism would be a
  larger language to keep honest than the duplication it removes. Includes are
  also one level deep on purpose: a cycle would be a hang rather than an error.

  ```
  Background:
    include: setup/fixture-page.feature
  ```

  Refactoring those four features to the include produced **22 of 22
  byte-identical specs**, so the sharing changed no behaviour. Every failure
  mode is a parse error rather than a silent drop — measured, all eight of
  these are refused with a message naming the file and line: an empty file, a
  file of comments, a missing file, an absolute path, a `..` climb, a path
  escaping to a non-feature file, a file that is a whole feature rather than a
  step list, and a file that itself includes.
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
