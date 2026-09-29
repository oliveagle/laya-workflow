"""Step definitions: Gherkin step text -> a laya-workflow node.

This is the *vocabulary*, not the semantics. A step is matched here and turned
into a call; what the call actually means is owned by the `bdd` plugin
(plugins/bdd/main.rhai). The split matters:

  * Assertion meaning lives in the plugin, so "the page title contains X" has
    exactly one definition, reusable from any spec - BDD-compiled or not. It
    used to be a JavaScript string built here in Python, which meant the meaning
    only existed if you went through this transpiler.
  * Here we only decide which op runs and with what arguments.

Which steps are plugin ops and which are capability ops:

  bdd plugin      navigate, wait_for, evaluate, and the whole Then family
  chrome_cdp op   open, click, type, select

`open` is a capability op because the engine closes any tab a plugin opened
before the next step can use it (see compile_step). The other three are there
because the plugin host exposes no
`browser_click` / `browser_type` / `browser_select`. The tempting substitute -
`browser_evaluate` with `element.click()` - was measured: it fires the handler
correctly, and it is a *weaker* test, because a synthetic JS click bypasses
hit-testing, so an overlay, a z-index bug, or a `pointer-events: none` mistake
would pass a scenario that a user could not click through. A BDD suite that
clicks with `.click()` and calls it an interaction test is worse than one that
does not click at all. So the gap stays visible instead of being papered over.

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


# ── Gherkin step text -> (op, capability, with-args) ────────────────────────
# Each table is (regex, op, capability). `with` is filled in by _args().
_GIVEN = [
    (re.compile(r'^the browser is ready$'), "open", "bdd"),
    (re.compile(r'^I am on (?P<url>.+)$'), "open", "bdd"),
]

_WHEN = [
    (re.compile(r'^I open (?P<url>.+)$'), "open", "bdd"),
    (re.compile(r'^I navigate to (?P<url>.+)$'), "navigate", "bdd"),
    (re.compile(r'^I wait for the element (?P<sel>.+)$'), "wait_for", "bdd"),
    (re.compile(r'^I click the element (?P<sel>.+)$'), "click", "chrome"),
    (re.compile(r'^I type (?P<text>.+?) into the element (?P<sel>.+)$'), "type", "chrome"),
    (re.compile(r'^I select (?P<value>.+?) in the element (?P<sel>.+)$'), "select", "chrome"),
    (re.compile(r'^I run javascript (?P<expr>.+)$'), "evaluate", "bdd"),
    # `release` was the one op a .feature could not say. It shipped, was
    # documented, and no scenario ever ran it - the same hole `I run javascript`
    # was in, one level down: a step nobody writes is a step nobody debugged.
    (re.compile(r'^I release the page$'), "release", "bdd"),
]

# (regex, op, assertion-name). `assertion` goes to the plugin, which owns what
# each name means; here it is only a name.
_THEN = [
    (re.compile(r'^the page title contains (?P<v>.+)$'), "assert", "title_contains"),
    (re.compile(r'^the page url contains (?P<v>.+)$'), "assert", "url_contains"),
    (re.compile(r'^the element (?P<v>.+?) is visible$'), "assert", "visible"),
    (re.compile(r'^the element (?P<v>.+?) is absent$'), "assert", "absent"),
    (re.compile(r'^javascript (?P<v>.+?) is true$'), "assert", "is_true"),
    (re.compile(r'^javascript (?P<v>.+?) is false$'), "assert", "is_false"),
    # Order is load-bearing. `equals text "x"` also matches the `equals` rule
    # (its `.+` happily eats `text "x"`), so the more specific pattern has to be
    # tried first. With the loose one first, the step compiled *silently* to
    # `equals` against the literal string `text "x"` - a step that quietly
    # becomes the wrong assertion, which is worse than a hard error.
    # Both spellings: prose reads better in a .feature, but the assertion is
    # *named* equals_text in the plugin, so that is what people actually type.
    (re.compile(r'^javascript (?P<v>.+?) equals[ _]text (?P<v2>.+)$'), "assert", "equals_text"),
    (re.compile(r'^javascript (?P<v>.+?) equals (?P<v2>.+)$'), "assert", "equals"),
    (re.compile(r'^javascript (?P<v>.+?) contains (?P<v2>.+)$'), "assert", "contains"),
]

_ALL = _GIVEN + _WHEN + _THEN


def _unquote(tok: str) -> str:
    tok = tok.strip()
    if len(tok) >= 2 and tok[0] == tok[-1] and tok[0] in "\"'":
        return tok[1:-1]
    return tok


def _typed_operand(raw: str):
    """A Gherkin operand as the JSON value it was *written* as.

    Quoted means string, full stop. Unquoted means "try to be a number or a
    bool, otherwise it is a bare string". `json.loads` on its own is wrong
    here: it would read the quotes in "3" as JSON syntax and hand back an int,
    which is precisely the type information the step was trying to preserve.
    """
    tok = raw.strip()
    if len(tok) >= 2 and tok[0] == tok[-1] and tok[0] in "\"'":
        return _unquote(tok)
    try:
        return json.loads(tok)
    except json.JSONDecodeError:
        return tok


def _match(kind: str, text: str, step: Step):
    table = {"given": _GIVEN, "when": _WHEN, "then": _THEN}[kind]
    for rx, op, extra in table:
        m = rx.match(text)
        if m:
            return m, op, extra
    known = sorted({n for _, _, n in _ALL} | {o for _, o, _ in _ALL})
    close = [n for n in known if any(w in text for w in re.split(r"[^a-z_]", n) if len(w) > 3)]
    hint = f" (did you mean: {', '.join(close)}?)" if close else ""
    raise UnknownStep(
        f"line {step.line}: unknown {kind} step: {text!r}{hint}\n"
        f"  supported: {', '.join(known)}"
    )


def _project(capability: str) -> dict:
    """What a node publishes back into state."""
    if capability == "bdd":
        return {"target_id": "/target_id"}
    return {}


def _args(op: str, extra: str, m: re.Match) -> tuple[dict, str, bool]:
    """Return (with-args, human instruction, needs-a-page)."""
    g = m.groupdict()
    v = _unquote(g.get("v", ""))

    if op == "open":
        if "url" not in g:  # `the browser is ready`
            return {}, "Is the CDP browser connected and ready to accept steps?", False
        url = _unquote(g["url"])
        return {"url": url}, f"Did Chrome open {url} and reach a loaded page?", False

    if op == "navigate":
        url = _unquote(g["url"])
        return ({"url": url}, f"Did the page navigate to {url}?", True)

    if op == "release":
        return {}, "Was the page closed?", True

    if op == "wait_for":
        sel = _unquote(g["sel"])
        return {"selector": sel}, f"Is {sel} present on the page?", True

    if op == "click":
        sel = _unquote(g["sel"])
        return {"selector": sel}, f"Was {sel} clicked?", True

    if op == "type":
        return ({"selector": _unquote(g["sel"]), "text": _unquote(g["text"])},
                f"Was {_unquote(g['text'])!r} typed into {_unquote(g['sel'])}?", True)

    if op == "select":
        return ({"value": _unquote(g["value"]), "selector": _unquote(g["sel"])},
                f"Was {_unquote(g['value'])!r} selected in {_unquote(g['sel'])}?", True)

    if op == "evaluate":
        # The capture group is `expr`, not `v`. Reading `v` handed the plugin an
        # empty expression, so `I run javascript "..."` compiled to a call that
        # could only ever throw - and nothing noticed for a whole release cycle
        # because no scenario had ever used the step. See
        # scripts/bdd/vocabulary_check.py, which now checks arguments, not just
        # the op, so a step cannot compile to a call with nothing in it.
        expr = _unquote(g["expr"])
        return ({"expression": expr},
                f"Did the script run on the page: {expr!r}?", True)

    if op == "assert":
        args = {"assertion": extra, "value": v}
        if extra in ("equals", "equals_text", "contains"):
            # `equals` needs the token *with* its quotes, because the quotes are
            # what say "this is a string". The other two want the bare text.
            raw = g["v2"].strip()
            text = _unquote(raw)
            if extra == "equals":
                # `equals` compares for strict identity, so the operand has to
                # keep the type the document gave it. What the author wrote is
                # the whole signal:
                #
                #   equals 3      -> int 3       (bare token, parses as a number)
                #   equals "3"    -> string "3"  (quoted, so it *is* a string)
                #   equals true   -> bool true
                #   equals "true" -> string "true"
                #   equals hello  -> string "hello" (bare, but not valid JSON)
                #
                # The obvious `json.loads(raw)` gets this backwards: it parses
                # the quoted "3" into int 3, so a step written to prove the
                # comparison is strict silently stopped testing it. An
                # @expected_failure scenario caught exactly that.
                args["expected"] = _typed_operand(raw)
                desc = f"({v}) === {json.dumps(args['expected'], ensure_ascii=False)}"
            else:
                args["expected"] = text
                desc = f"{'String'}({v}) {'==' if extra == 'equals_text' else 'contains'} {json.dumps(text, ensure_ascii=False)}"
        else:
            desc = f"{extra}({json.dumps(v, ensure_ascii=False)})"
        return args, f"Did the assertion hold: {desc}?", True

    raise UnknownStep(f"no builder for op {op!r}")  # pragma: no cover


def _target(has_page: bool) -> dict:
    return {"target_id": "${state.target_id}"} if has_page else {}


# A Scenario Outline column, or any hand-written `<name>` in a step, becomes a
# state key the engine interpolates at run time. The identifier shape is
# deliberate: `i < 10` and `a<b` in a JavaScript expression are not affected,
# so a `Then javascript ...` step can contain real comparison operators.
PARAM_RE = re.compile(r"<([A-Za-z_][A-Za-z0-9_]*)>")


def compile_step(step: Step, has_page: bool) -> tuple[CompiledNode, bool]:
    """Compile one step. Returns the node and whether the page exists after it."""
    text = PARAM_RE.sub(r"${state.\1}", step.text)
    m, op, extra = _match(step.kind, text, step)
    args, instruction, needs_page = _args(op, extra, m)

    if needs_page and not has_page:
        raise StepError(
            f"line {step.line}: step {text!r} needs a page, but no earlier step opened one. "
            f'Add `Given I am on "<url>"` (or `When I open "<url>"`) before it.'
        )

    capability = "chrome" if op in ("click", "type", "select") else "bdd"
    # `open` is handled above against the chrome capability; everything else
    # that touches the page is a bdd plugin op except real input, which stays on
    # the capability (see the module docstring).
    with_args = dict(args)
    with_args.update(_target(has_page))

    action = None
    now_has_page = has_page
    if op == "release":
        # After a release there is no page, and a later step that assumed one
        # should be a compile error, not a CDP 500 at run time. `has_page` is
        # already threaded through compile_step for exactly this.
        now_has_page = False
    if op == "open" and "url" in args:
        # The page is opened by the chrome_cdp *capability*, not by the plugin.
        # The engine closes every tab a plugin opened when that plugin call
        # returns (src/capability/plugin.rs: "The engine closes what the plugin
        # opened"), so a plugin-opened page is already closed by the time the
        # next step asks for its target_id - which showed up as an unhelpful
        # "connect Chrome page websocket failed: 500". Ownership of page
        # lifetime belongs to the capability; the plugin only borrows a target.
        action = {"kind": "call", "capability": "chrome",
                  "with": {"op": "open", **with_args},
                  "project": {"target_id": "/target_id"}}
        now_has_page = True
    elif op != "open":
        action = {"kind": "call", "capability": capability,
                  "with": {"op": op, **with_args},
                  "project": _project(capability)}

    slug = f"s{step.line}_{op}"
    return CompiledNode(slug, instruction, action, step.kind), now_has_page


def vocabulary() -> list[str]:
    return sorted({n for _, _, n in _ALL} | {o for _, o, _ in _ALL})
