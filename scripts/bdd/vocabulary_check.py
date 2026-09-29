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
          f"{len(NEEDS_PAGE_CASES)} steps still refuse to run without a page")
    return 0


if __name__ == "__main__":
    sys.exit(main())
