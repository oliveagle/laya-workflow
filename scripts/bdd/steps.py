"""Step definitions: Gherkin step text -> a laya-workflow node.

This is the whole "Given/When/Then" vocabulary. Each definition turns one step
into exactly one node of the generated spec:

    Given  -> arrange: put the browser in a known state
    When   -> act: one CDP operation (open/navigate/click/type/select/evaluate)
    Then   -> assert: a *deterministic* `browser_base.assert` whose `checks`
              mismatch throws, which fails the whole run with rc=1

The Then half is the point of the exercise. Nothing in an assert is judged by
a language model: the expression runs in the page, `checks` compares the
returned object key-by-key, and a mismatch is a thrown error. A scenario is
green because the page really did what the BDD document says, not because a
heuristic guessed that it probably did.

Anything not matched here raises `UnknownStep`. A BDD step that quietly becomes
a no-op is a test that always passes, which is worse than a hard error.
"""

from __future__ import annotations

import json
import re
from dataclasses import dataclass

from gherkin import Step


class UnknownStep(Exception):
    """A step the vocabulary does not cover, with the suggestions that were close."""


class StepError(Exception):
    """A step that is recognised but cannot be compiled as written."""


@dataclass
class CompiledNode:
    name: str
    instruction: str
    action: dict | None
    kind: str


# ── JS helpers ───────────────────────────────────────────────────────────────
def js(value: object) -> str:
    """A JS literal for `value` (json.dumps output is valid JS for these types)."""
    return json.dumps(value, ensure_ascii=False)


def quoted(s: str) -> str:
    """The text captured from a Gherkin step, already unquoted by the caller."""
    return s


# ── When steps: one CDP operation each ───────────────────────────────────────
_WHEN = [
    # (regex, builder-name)
    (re.compile(r'^I open (?P<url>.+)$'), "open"),
    (re.compile(r'^I navigate to (?P<url>.+)$'), "navigate"),
    (re.compile(r'^I wait for the element (?P<sel>.+)$'), "wait_for"),
    (re.compile(r'^I click the element (?P<sel>.+)$'), "click"),
    (re.compile(r'^I type (?P<text>.+?) into the element (?P<sel>.+)$'), "type"),
    (re.compile(r'^I select (?P<value>.+?) in the element (?P<sel>.+)$'), "select"),
    (re.compile(r'^I run javascript (?P<expr>.+)$'), "evaluate"),
]

_GIVEN = [
    (re.compile(r'^the browser is ready$'), "browser_ready"),
    (re.compile(r'^I am on (?P<url>.+)$'), "open"),
]

_THEN = [
    (re.compile(r'^the page title contains (?P<want>.+)$'), "title_contains"),
    (re.compile(r'^the page url contains (?P<want>.+)$'), "url_contains"),
    (re.compile(r'^the element (?P<sel>.+?) is visible$'), "visible"),
    (re.compile(r'^javascript (?P<expr>.+?) is true$'), "is_true"),
    (re.compile(r'^javascript (?P<expr>.+?) is false$'), "is_false"),
    (re.compile(r'^javascript (?P<expr>.+?) equals (?P<want>.+)$'), "equals"),
    (re.compile(r'^javascript (?P<expr>.+?) contains (?P<want>.+)$'), "contains"),
]

# Every step name the vocabulary knows, across all three keyword tables.
_ALL = _GIVEN + _WHEN + _THEN


def _unquote(tok: str, step: Step) -> str:
    tok = tok.strip()
    if len(tok) >= 2 and tok[0] == tok[-1] and tok[0] in "\"'":
        return tok[1:-1]
    # Unquoted: accept anything, but a bare word that is clearly meant to be a
    # quoted literal is a typo worth naming rather than silently swallowing.
    return tok


def _match(kind: str, text: str, step: Step) -> tuple[re.Match, str]:
    table = {"given": _GIVEN, "when": _WHEN, "then": _THEN}[kind]
    for rx, name in table:
        m = rx.match(text)
        if m:
            return m, name
    known = sorted({n for _, n in _ALL})
    close = [n for n in known if any(w in text for w in n.split("_"))]
    hint = f" (did you mean: {', '.join(close)}?)" if close else ""
    raise UnknownStep(
        f"{step}: unknown {kind} step: {text!r}{hint}\n"
        f"  supported: {', '.join(known)}"
    )


def _action(name: str, m: re.Match, has_page: bool, line: int) -> tuple[dict | None, bool, str]:
    """Return (action, produces_page, instruction)."""
    g = m.groupdict()

    if name == "browser_ready":
        return None, has_page, "Is the CDP browser connected and ready to accept steps?"

    if name == "open":
        url = _unquote(g["url"], None)  # type: ignore[arg-type]
        return (
            {"kind": "call", "capability": "chrome",
             "with": {"op": "open", "url": url},
             "project": {"target_id": "/target_id"}},
            True,
            f"Did Chrome open {url} and reach a loaded page?",
        )

    if not has_page:
        raise StepError(
            f"line {line}: step {g!r} needs a page, but no earlier step opened one. "
            f'Add `Given I am on "<url>"` (or `When I open "<url>"`) before it.'
        )

    t = "${state.target_id}"

    if name == "navigate":
        url = _unquote(g["url"], None)  # type: ignore[arg-type]
        return ({"kind": "call", "capability": "chrome",
                 "with": {"op": "navigate", "target_id": t, "url": url},
                 "project": {"ready": "/ready"}},
                has_page, f"Did the page navigate to {url}?")

    if name == "wait_for":
        sel = _unquote(g["sel"], None)  # type: ignore[arg-type]
        return ({"kind": "call", "capability": "chrome",
                 "with": {"op": "wait_for", "target_id": t, "selector": sel, "timeout_ms": 15000},
                 "project": {"ready": "/ready"}},
                has_page, f"Is {sel} present on the page?")

    if name == "click":
        sel = _unquote(g["sel"], None)  # type: ignore[arg-type]
        return ({"kind": "call", "capability": "chrome",
                 "with": {"op": "click", "target_id": t, "selector": sel},
                 "project": {"clicked": "/clicked"}},
                has_page, f"Was {sel} clicked?")

    if name == "type":
        text = _unquote(g["text"], None)  # type: ignore[arg-type]
        sel = _unquote(g["sel"], None)  # type: ignore[arg-type]
        return ({"kind": "call", "capability": "chrome",
                 "with": {"op": "type", "target_id": t, "selector": sel, "text": text},
                 "project": {"typed": "/typed"}},
                has_page, f"Was {text!r} typed into {sel}?")

    if name == "select":
        val = _unquote(g["value"], None)  # type: ignore[arg-type]
        sel = _unquote(g["sel"], None)  # type: ignore[arg-type]
        return ({"kind": "call", "capability": "chrome",
                 "with": {"op": "select", "target_id": t, "selector": sel, "value": val},
                 "project": {"selected": "/selected"}},
                has_page, f"Was {val!r} selected in {sel}?")

    if name == "evaluate":
        expr = _unquote(g["expr"], None)  # type: ignore[arg-type]
        return ({"kind": "call", "capability": "chrome",
                 "with": {"op": "evaluate", "target_id": t, "expression": expr},
                 "project": {"value": "/value"}},
                has_page, "Did the script run on the page?")

    # ── Then steps: browser_base.assert ──────────────────────────────────────
    def assertion(expr: str, expect_desc: str) -> tuple[dict | None, bool, str]:
        # The parentheses are load-bearing. CDP evaluates `expression` as an
        # *expression*, and a bare `{ ok: ... }` parses as a block statement
        # with a label - the page answers "SyntaxError: Unexpected token ':'".
        # Wrapping makes it the object literal the `checks` map expects.
        wrapped = f"({expr})"
        return (
            {"kind": "call", "capability": "bd_assert",
             "with": {"target_id": t, "expression": wrapped, "checks": {"ok": True}},
             "project": {"assert_pass": "/pass", "assert_actual": "/actual"}},
            has_page,
            f"Did the assertion hold: {expect_desc}?",
        )

    if name == "title_contains":
        want = _unquote(g["want"], None)  # type: ignore[arg-type]
        return assertion(
            '{ ok: (document.title || "").includes(%s), actual: document.title }' % js(want),
            f'the page title contains {js(want)}',
        )

    if name == "url_contains":
        want = _unquote(g["want"], None)  # type: ignore[arg-type]
        return assertion(
            '{ ok: (location.href || "").includes(%s), actual: location.href }' % js(want),
            f'the url contains {js(want)}',
        )

    if name == "visible":
        sel = _unquote(g["sel"], None)  # type: ignore[arg-type]
        return assertion(
            "(() => { const e = document.querySelector(%s);"
            " if (!e) return { ok: false, actual: 'no such element' };"
            " const r = e.getBoundingClientRect();"
            " const vis = !!(r.width || r.height) && getComputedStyle(e).visibility !== 'hidden';"
            " return { ok: vis, actual: e.tagName + (vis ? ' visible' : ' not visible') }; })()"
            % js(sel),
            f"{sel} is visible",
        )

    if name in ("is_true", "is_false"):
        expr = _unquote(g["expr"], None)  # type: ignore[arg-type]
        want = name == "is_true"
        return assertion(
            "{ ok: (!!(%s)) === %s, actual: (%s) }" % (expr, "true" if want else "false", expr),
            f"({expr}) is {'true' if want else 'false'}",
        )

    if name == "equals":
        expr = _unquote(g["expr"], None)  # type: ignore[arg-type]
        raw = _unquote(g["want"], None)  # type: ignore[arg-type]
        try:
            expected = json.loads(raw)
            lit = json.dumps(expected, ensure_ascii=False)
        except json.JSONDecodeError:
            lit = js(raw)
        return assertion(
            "{ ok: (%s) === %s, actual: (%s) }" % (expr, lit, expr),
            f"({expr}) === {lit}",
        )

    if name == "contains":
        expr = _unquote(g["expr"], None)  # type: ignore[arg-type]
        want = _unquote(g["want"], None)  # type: ignore[arg-type]
        return assertion(
            "{ ok: String(%s).includes(%s), actual: String(%s) }" % (expr, js(want), expr),
            f"String({expr}) contains {js(want)}",
        )

    raise UnknownStep(f"no builder named {name!r}")  # pragma: no cover


# A Scenario Outline column, or any hand-written `<name>` in a step, becomes a
# state key the engine interpolates at run time. The identifier shape is
# deliberate: `i < 10` and `a<b` in a JavaScript expression are not affected,
# so a `Then javascript ...` step can contain real comparison operators.
PARAM_RE = re.compile(r"<([A-Za-z_][A-Za-z0-9_]*)>")


def compile_step(step: Step, has_page: bool) -> tuple[CompiledNode, bool]:
    """Compile one step. Returns the node and whether the page exists after it."""
    text = PARAM_RE.sub(r"${state.\1}", step.text)
    m, name = _match(step.kind, text, step)
    action, now_has_page, instruction = _action(name, m, has_page, step.line)
    slug = f"s{step.line}_{name}"
    return CompiledNode(slug, instruction, action, step.kind), now_has_page


def vocabulary() -> list[str]:
    return sorted({n for _, n in _ALL})
