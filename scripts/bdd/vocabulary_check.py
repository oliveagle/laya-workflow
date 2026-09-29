#!/usr/bin/env python3
"""Prove the step vocabulary maps each step to the op it is supposed to run.

A Gherkin step that matches the *wrong* rule is the quietest failure in this
whole system: the scenario still compiles, the run still goes green, and the
document has stopped saying what it says. It already happened once - `equals
text "x"` was swallowed by the `equals` rule, whose `.+` was happy to eat
`text "x"`, so the step silently became a strict comparison against the
literal string `text "x"`. Only an @expected_failure scenario in
bdd/features/assertions.feature caught it, and that needs Chrome.

These checks need nothing but Python, so they run in the default gate and in
CI. Each row is (kind, text, expected op, expected assertion-or-None).
"""

from __future__ import annotations

import os
import re
import sys

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))

import gherkin  # noqa: E402
import steps as stepdefs  # noqa: E402

# `has_page` is True for everything but `open`, so the steps that need a tab
# take the has-a-tab path, which is the one a scenario actually uses.
CASES: list[tuple[str, str, str, str | None, bool]] = [
    # "the browser is ready" is a declaration, not an action: it must compile
    # to a node with no action at all, which is a different thing to get wrong
    # (an empty action would call the plugin with no op).
    ("given", "the browser is ready", None, None, False),
    ("given", 'I am on "http://x/"', "open", None, False),
    ("when", 'I open "http://x/"', "open", None, True),
    ("when", 'I navigate to "http://x/"', "navigate", None, True),
    ("when", 'I wait for the element "#a"', "wait_for", None, True),
    ("when", 'I click the element "#a"', "click", None, True),
    ("when", 'I type "hi" into the element "#a"', "type", None, True),
    ("when", 'I select "blue" in the element "#a"', "select", None, True),
    ("when", 'I run javascript "1+1"', "evaluate", None, True),
    ("when", "I release the page", "release", None, True),
    ("then", 'the page title contains "x"', "assert", "title_contains", True),
    ("then", 'the page url contains "x"', "assert", "url_contains", True),
    ("then", 'the element "#a" is visible', "assert", "visible", True),
    ("then", 'the element "#a" is absent', "assert", "absent", True),
    ('then', 'javascript "a" is true', "assert", "is_true", True),
    ('then', 'javascript "a" is false', "assert", "is_false", True),
    ('then', 'javascript "a" equals 3', "assert", "equals", True),
    ('then', 'javascript "a" contains "x"', "assert", "contains", True),
    # The two spellings of the same step must land on the same assertion, and
    # neither may fall through to `equals`. This row is the regression guard.
    ('then', 'javascript "a" equals text "x"', "assert", "equals_text", True),
    ('then', 'javascript "a" equals_text "x"', "assert", "equals_text", True),
    # The last op to get a step. It is also the only one whose *result* the
    # vocabulary throws away, so pin what it must carry - a release with no
    # target_id is a no-op that compiles clean.
    ("when", "I release the page", "release", None, True),
]

# The arguments a step produces, not just the op it names. Checking only the
# op is what let `I run javascript "1+1"` sit in the table for a whole release
# cycle compiling to `expression: ""` - the right call, carrying nothing, which
# could only ever throw. No scenario used the step, so nothing went red.
ARGUMENT_CASES: list[tuple[str, str, dict]] = [
    ("given", 'I am on "http://x/"', {"url": "http://x/"}),
    ("when", 'I navigate to "http://y/"', {"url": "http://y/"}),
    ("when", 'I wait for the element "#a"', {"selector": "#a"}),
    ("when", 'I click the element "#a"', {"selector": "#a"}),
    ("when", 'I type "hi" into the element "#a"',
     {"selector": "#a", "text": "hi"}),
    ("when", 'I select "blue" in the element "#a"',
     {"selector": "#a", "value": "blue"}),
    # The one that shipped broken.
    ("when", 'I run javascript "1+1"', {"expression": "1+1"}),
    ("when", 'I run javascript "document.title = 1"', {"expression": "document.title = 1"}),
    ("then", 'the page title contains "x"', {"assertion": "title_contains", "value": "x"}),
    ("then", 'the page url contains "x"', {"assertion": "url_contains", "value": "x"}),
    ("then", 'the element "#a" is visible', {"assertion": "visible", "value": "#a"}),
    ("then", 'the element "#a" is absent', {"assertion": "absent", "value": "#a"}),
    ('then', 'javascript "a" is true', {"assertion": "is_true", "value": "a"}),
    ('then', 'javascript "a" is false', {"assertion": "is_false", "value": "a"}),
    ('then', 'javascript "a" equals 3', {"assertion": "equals", "value": "a", "expected": 3}),
    ('then', 'javascript "a" contains "x"', {"assertion": "contains", "expected": "x"}),
    ('then', 'javascript "a" equals text "x"',
     {"assertion": "equals_text", "expected": "x"}),
]

# `release` changes the compile state: after it there is no page, so a later
# step must be a compile error rather than a CDP failure at run time. Without
# this the step is a silent no-op in any document that releases first and
# asserts second - which is exactly the document where a wrong `release` hurts.
POST_RELEASE_CASES: list[tuple[str, str]] = [
    ('when', "I release the page"),
    ('then', 'the element "#a" is visible'),
]

# The operand type is the author's signal, and getting it wrong makes a
# strictness test silently test nothing.
OPERAND_CASES: list[tuple[str, object]] = [
    ("equals 3", 3),
    ('equals "3"', "3"),
    ("equals true", True),
    ('equals "true"', "true"),
    ("equals hello", "hello"),
    ('equals "hello"', "hello"),
    ("equals -1.5", -1.5),
    ("equals null", None),
]

# An element that does not exist must be an error, never a silent pass.
NEEDS_PAGE_CASES = [
    'I click the element "#a"',
    'I type "hi" into the element "#a"',
    'I select "blue" in the element "#a"',
    'I wait for the element "#a"',
    'I navigate to "http://x/"',
    'I run javascript "1+1"',
    'the element "#a" is visible',
    'javascript "a" is true',
]


# Every @expected_failure has to say what it is disproving, and this is the
# Chrome-free half of that rule. The runtime half lives in run.py, which
# requires the reason to appear in the failure. Neither alone is enough: a bare
# tag satisfies nothing at parse time, and a reason nobody checks is a comment.
# This one runs in the default gate and in CI, where there is no browser.
XFAIL = "expected_failure"


def check_xfail_reasons(feature_dir: str) -> list[str]:
    import glob

    problems: list[str] = []
    paths = sorted(glob.glob(os.path.join(feature_dir, "*.feature")))
    if not paths:
        return [f"{feature_dir}: no .feature files found"]
    for path in paths:
        name = os.path.basename(path)
        with open(path, encoding="utf-8") as f:
            try:
                feature = gherkin.parse(f.read(), path)
            except gherkin.GherkinError as e:
                problems.append(f"{name}: {e}")
                continue
        for scenario in feature.scenarios:
            tagged = [t for t in (scenario.tags or [])
                      if t == XFAIL or t.startswith(XFAIL + "(")]
            if not tagged:
                continue
            if not gherkin.tag_reason(scenario, XFAIL):
                problems.append(
                    f"{name} :: {scenario.name}: @{XFAIL} declares no reason. "
                    "Without one, any non-zero exit counts - including a typo'd "
                    "Given, a Chrome that would not start, or a 404 from the "
                    f"fixture. Write @{XFAIL}(<text the failure must contain>)."
                )
    return problems


# The plugin's two "here is what you can say" error messages enumerate the whole
# vocabulary, and they are the only place a user learns it. Nothing kept them in
# step with the code: adding a seventh op or a tenth assertion leaves both
# messages quietly wrong, and the message is what someone reads when they have
# already got something wrong. Checked here because it needs only the plugin
# source - no Chrome, so CI sees it.
PLUGIN = os.path.join(
    os.path.dirname(os.path.abspath(__file__)), "..", "..",
    "plugins", "bdd", "main.rhai")

OP_DISPATCH_RE = re.compile(r'if op == "([a-z_]+)" \{ return _op_')
ASSERTION_RE = re.compile(r'if name == "([a-z_]+)"')
# The list inside `( ... )` at the end of each message.
ENUM_RE = re.compile(r"\(([a-z_ |]+)\)")


def check_plugin_messages() -> list[str]:
    problems: list[str] = []
    try:
        with open(PLUGIN, encoding="utf-8") as f:
            src = f.read()
    except OSError as e:
        return [f"cannot read {PLUGIN}: {e}"]

    ops = set(OP_DISPATCH_RE.findall(src))
    assertions = set(ASSERTION_RE.findall(src))
    if not ops or not assertions:
        return ["plugin dispatch not found - the patterns in "
                "vocabulary_check.py no longer match plugins/bdd/main.rhai"]

    for label, want, needle in (
        ("unknown op", ops, "bdd: unknown op"),
        ("unknown assertion", assertions, "bdd.assert: unknown assertion"),
    ):
        # The message is a `throw` spread over two source lines, because the
        # list is long enough that rustfmt-style wrapping kicked in. Search the
        # whole statement, not the line the needle is on, or a reformat silently
        # turns this check into a false alarm.
        at = src.find(needle)
        if at < 0 or "throw" not in src[max(0, at - 40):at]:
            problems.append(f"the {label} message is gone from {PLUGIN}")
            continue
        end = src.find(";", at)
        statement = src[at:end if end > 0 else at + 400]
        m = ENUM_RE.search(statement)
        if not m:
            problems.append(
                f"the {label} message no longer lists the vocabulary, so it is "
                f"no longer a way to learn it: {statement.strip()[:70]}")
            continue
        listed = {p.strip() for p in m.group(1).split("|") if p.strip()}
        for missing in sorted(want - listed):
            problems.append(
                f"the {label} message does not mention {missing!r}, which the "
                "plugin handles - someone hitting that error gets told it does "
                "not exist")
        for extra in sorted(listed - want):
            problems.append(
                f"the {label} message advertises {extra!r}, which the plugin "
                "does not handle - someone will try it and get no such op")
    return problems


def _step(kind: str, text: str) -> gherkin.Step:
    return gherkin.Step(keyword=kind.capitalize() + " ", kind=kind, text=text, line=1)


def main() -> int:
    failures: list[str] = []

    for kind, text, want_op, want_assertion, has_page in CASES:
        try:
            node, _ = stepdefs.compile_step(_step(kind, text), has_page)
        except (stepdefs.UnknownStep, stepdefs.StepError) as e:
            failures.append(f"{kind} {text!r}: {e}")
            continue
        action = node.action or {}
        got_op = action.get("with", {}).get("op") if action else None
        if got_op != want_op:
            failures.append(f"{kind} {text!r}: op {got_op!r}, want {want_op!r}")
            continue
        got_assertion = action.get("with", {}).get("assertion") if action else None
        if got_assertion != want_assertion:
            failures.append(
                f"{kind} {text!r}: assertion {got_assertion!r}, want {want_assertion!r}"
            )

    for kind, text, want_args in ARGUMENT_CASES:
        try:
            node, _ = stepdefs.compile_step(_step(kind, text), True)
        except (stepdefs.UnknownStep, stepdefs.StepError) as e:
            failures.append(f"{kind} {text!r}: {e}")
            continue
        with_args = (node.action or {}).get("with", {})
        for key, want in want_args.items():
            got = with_args.get(key)
            if got != want:
                failures.append(
                    f"{kind} {text!r}: with.{key} is {got!r}, want {want!r}"
                )
        # A step must never compile to a call with an empty required argument:
        # that is a call that can only throw, and it looks fine until someone
        # finally writes the scenario that uses it.
        for key in ("url", "selector", "text", "value", "expression", "assertion"):
            if key in want_args and not with_args.get(key):
                failures.append(f"{kind} {text!r}: with.{key} compiled empty")

    for text, want in OPERAND_CASES:
        try:
            node, _ = stepdefs.compile_step(_step("then", f'javascript "a" {text}'), True)
        except (stepdefs.UnknownStep, stepdefs.StepError) as e:
            failures.append(f"then javascript \"a\" {text}: {e}")
            continue
        got = (node.action or {}).get("with", {}).get("expected")
        if got != want or type(got) is not type(want):
            failures.append(
                f"then javascript \"a\" {text}: expected {want!r} ({type(want).__name__}), "
                f"got {got!r} ({type(got).__name__})"
            )

    # The `release` step must leave the compiler in the has-no-page state.
    try:
        _, still_has_page = stepdefs.compile_step(_step("when", "I release the page"), True)
        if still_has_page:
            failures.append(
                'when "I release the page": the compiler still believes a page is open, '
                "so any later step would run against a closed target"
            )
    except (stepdefs.UnknownStep, stepdefs.StepError) as e:
        failures.append(f'when "I release the page": {e}')

    for kind, text in POST_RELEASE_CASES:
        try:
            stepdefs.compile_step(_step(kind, text), False)
        except stepdefs.StepError:
            continue
        except stepdefs.UnknownStep as e:
            failures.append(f"post-release {kind} {text!r}: {e}")
            continue
        failures.append(
            f"post-release {kind} {text!r}: compiled with no page, want a StepError"
        )

    failures += check_plugin_messages()
    failures += check_xfail_reasons(
        os.path.join(os.path.dirname(os.path.abspath(__file__)),
                     "..", "..", "bdd", "features"))

    for text in NEEDS_PAGE_CASES:
        try:
            stepdefs.compile_step(_step("then" if text.startswith(("the ", "javascript")) else "when", text), False)
        except stepdefs.StepError:
            continue
        except stepdefs.UnknownStep as e:
            failures.append(f"needs-page {text!r}: {e}")
            continue
        failures.append(f"needs-page {text!r}: compiled without a page, want a StepError")

    if failures:
        print(f"bdd vocabulary: {len(failures)} problem(s):", file=sys.stderr)
        for f in failures:
            print(f"  {f}", file=sys.stderr)
        return 1
    print(f"bdd vocabulary: {len(CASES)} steps map correctly, "
          f"{len(ARGUMENT_CASES)} step arguments survive, "
          f"{len(OPERAND_CASES)} operand types survive, "
          f"{len(NEEDS_PAGE_CASES)} steps still refuse to run without a page, "
          f"{len(POST_RELEASE_CASES)} stay refused after a release, "
          f"every @{XFAIL} says what it disproves, "
          f"the plugin's own vocabulary messages are in sync")
    return 0


if __name__ == "__main__":
    sys.exit(main())
