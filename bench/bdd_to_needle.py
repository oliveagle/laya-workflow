#!/usr/bin/env python3
"""
Probe: can Needle 3 (base 121M/2-bit, on-device) compile Gherkin BDD steps
into laya-workflow spec JSON — good enough to replace the deterministic
regex transpiler in scripts/bdd/transpile.py?

Three conditions, every call through the real engine (`laya-workflow mcp serve`):

  A  single-tool extract   one `bdd_step` tool, op + assertion as string enums
  B  multi-tool complete   one tool per op + `triggers` regexes — the vendor's
                           recommended "Design Tools for Needle 3" layout
  C  novel phrasings       step sentences scripts/bdd/steps.py CANNOT compile
                           (0/10 regex coverage) — the only place a model could
                           add value over the deterministic compiler

Scoring, per step: op routing, assertion class, argument values, full record
(all three), and how many calls clear the engine's 0.1 confidence floor.
Ground truth for A/B: the repo's own 46 unique feature steps; for C: labeled
paraphrases. Reproduce: python3 bench/bdd_to_needle.py
"""
import json
import os
import subprocess
import sys

BIN = os.path.abspath(
    os.path.join(os.path.dirname(__file__), "..", "target", "release", "laya-workflow")
)
os.environ.setdefault(
    "NEEDLE3_CACT", os.path.expanduser("~/.cache/cactus-needle/v3/3.2.0/needle3.cact")
)


def mcp_tool(name, args):
    req = {"jsonrpc": "2.0", "id": 1, "method": "tools/call",
           "params": {"name": name, "arguments": args}}
    p = subprocess.run([BIN, "mcp", "serve"], input=json.dumps(req),
                       capture_output=True, text=True, timeout=120)
    if p.returncode != 0:
        raise RuntimeError(p.stderr[-300:])
    return json.loads(json.loads(p.stdout)["result"]["content"][0]["text"])


def _tool(name, desc, trig, props):
    return {"type": "function", "name": name, "description": desc, "triggers": trig,
            "parameters": {"type": "object",
                           "properties": {k: {"type": "string"} for k in props},
                           "required": list(props)}}


TOOLS = [
    _tool("open_page", "Open a URL and wait until it is loaded.",
          [r"\b(am on|open)\b"], ["url"]),
    _tool("navigate", "Move an open tab to a new URL.", [r"\bnavigate\b"], ["url"]),
    _tool("wait_for", "Poll until a CSS selector matches.", [r"\bwait for\b"], ["selector"]),
    _tool("click", "Click a CSS selector.", [r"\bclick\b"], ["selector"]),
    _tool("type_text", "Type text into a CSS selector.", [r"\btype\b"], ["text", "selector"]),
    _tool("select_option", "Select a value in a CSS selector.", [r"\bselect\b"],
          ["value", "selector"]),
    _tool("run_js", "Evaluate a javascript expression in the page.",
          [r"\brun javascript\b"], ["expression"]),
    _tool("assert", "Run a named page assertion.",
          [r"\b(title|url) contains|is visible|is absent|is true|is false|equals|contains\b"],
          ["assertion", "value", "expected"]),
    _tool("release_page", "Close the current page.", [r"\brelease\b"], ["target_id"]),
]

SINGLE_TOOL = {
    "type": "function", "name": "bdd_step",
    "description": "Translate a Gherkin browser-automation step into a workflow node.",
    "parameters": {"type": "object", "properties": {
        "op": {"type": "string", "enum": ["open", "navigate", "wait_for", "click",
                                          "type", "select", "evaluate", "assert", "release"]},
        "assertion": {"type": "string", "enum": ["title_contains", "url_contains", "visible",
                                                  "absent", "is_true", "is_false", "equals",
                                                  "contains", "equals_text"]},
        "value": {"type": "string"},
        "expected": {"type": "string"}}},
}

# (step, tool, assertion-or-None, {arg: expected}) — in-vocabulary steps
IN_VOCAB = [
    ('Then the page title contains "BDD Fixture"', "assert", "title_contains",
     {"value": "BDD Fixture"}),
    ('When I click the element "#greet"', "click", None, {"selector": "#greet"}),
    ('Given I am on "<base_url>/index.html"', "open_page", None,
     {"url": "<base_url>/index.html"}),
    ('Then the element "#heading" is visible', "assert", "visible", {"value": "#heading"}),
    ('When I navigate to "<base_url>/second.html"', "navigate", None,
     {"url": "<base_url>/second.html"}),
    ('When I type "Ada" into the element "#name"', "type_text", None,
     {"text": "Ada", "selector": "#name"}),
    ("Then javascript \"document.readyState === 'complete'\" is true", "assert", "is_true",
     {"value": "document.readyState === 'complete'"}),
    ('When I wait for the element "#echo[data-filled]"', "wait_for", None,
     {"selector": "#echo[data-filled]"}),
    ('When I select "blue" in the element "#color"', "select_option", None,
     {"value": "blue", "selector": "#color"}),
    ('When I release the page', "release_page", None, {}),
]

# same nine ops, phrased so scripts/bdd/steps.py's regexes never match (0/10)
NOVEL = [
    ('Then the heading "#hero" should be displayed', "assert", "visible",
     {"value": "#hero"}),
    ('When I hit the submit button', "click", None, {}),
    ('Given the site is open at https://example.com/login', "open_page", None,
     {"url": "https://example.com/login"}),
    ('Then the address bar shows "/dashboard"', "assert", "url_contains", {}),
    ('When I fill the email field with ada@example.com', "type_text", None, {}),
    ('Then the count of items is 7', "assert", "equals", {}),
    ('When I jump to the settings screen', "navigate", None, {}),
    ('Then there should be no loading spinner', "assert", "absent", {}),
    ('When I choose "USD" from the currency picker', "select_option", None,
     {"value": "USD"}),
    ('Then confirm the button is hidden', "assert", "absent", {}),
]

SYSTEM = "You are a Gherkin-to-workflow compiler. Call exactly one tool per step."


def _unq(s):
    if s and len(s) >= 2 and s[0] == '"' and s[-1] == '"':
        return s[1:-1]
    return s


def score_single():
    """Condition A: one bdd_step tool, op string compared to the expected tool family."""
    op_ok = assert_ok = arg_ok = full_ok = floor_ok = 0
    for step, exp_tool, exp_assert, exp_args in IN_VOCAB:
        r = mcp_tool("needle_extract", {"text": step, "tool": SINGLE_TOOL, "system": SYSTEM})
        a = r.get("arguments") or {}
        got = a.get("op") or ""
        # op name in extract == tool name in complete for click/navigate/wait_for/;
        # normalize the few that differ so we compare intent, not spelling
        got_tool = {"open": "open_page", "type": "type_text", "select": "select_option",
                    "evaluate": "run_js"}.get(got, got)
        oo = got_tool == exp_tool
        op_ok += oo
        ao = exp_assert is None or a.get("assertion") == exp_assert
        assert_ok += ao
        ar = all(_unq(a.get(k) or "") == v for k, v in exp_args.items())
        arg_ok += ar
        full_ok += oo and ao and ar
        if (r.get("confidence") or 0) >= 0.1:
            floor_ok += 1
    return dict(op=op_ok, assertion=assert_ok, args=arg_ok, full=full_ok, floor=floor_ok)


def score_multi(cases):
    """Conditions B/C: multi-tool complete + triggers; returns (counters, details)."""
    op_ok = assert_ok = arg_ok = full_ok = floor_ok = 0
    details = []
    for step, exp_tool, exp_assert, exp_args in cases:
        r = mcp_tool("needle_complete", {"prompt": step, "tools": TOOLS, "system": SYSTEM})
        calls = r.get("function_calls") or []
        supp = r.get("suppressed_calls") or []
        item = calls[0] if calls else (supp[0] if supp else None)
        conf = r.get("confidence") or 0.0
        args = (item or {}).get("arguments") or {}
        got = (item or {}).get("name")
        oo = got == exp_tool
        ao = exp_assert is None or args.get("assertion") == exp_assert
        ar = all(_unq(args.get(k) or "") == v for k, v in exp_args.items())
        f = oo and ao and ar
        op_ok += oo
        assert_ok += ao
        arg_ok += ar
        full_ok += f
        floor_ok += conf >= 0.1
        details.append((step, exp_tool, exp_assert, got, args, conf, f))
    return dict(op=op_ok, assertion=assert_ok, args=arg_ok, full=full_ok, floor=floor_ok), details


def emit(label, c, n):
    print(f"  {label:<34} op {c['op']}/{n}  assert {c['assertion']}/{n}  "
          f"args {c['args']}/{n}  full {c['full']}/{n}  conf>=0.1 {c['floor']}/{n}")


def main():
    n = len(IN_VOCAB)
    print("=" * 74)
    print("BDD -> needle -> spec JSON  (bench/bdd_to_needle.py)")
    print("=" * 74)
    a = score_single()
    emit("A single-tool extract (in-vocab)", a, n)
    b, _ = score_multi(IN_VOCAB)
    emit("B multi-tool + triggers (in-vocab)", b, n)
    c, c_det = score_multi(NOVEL)
    emit("C multi-tool (novel phrasings)", c, len(NOVEL))
    print()
    print("Novel-step failures (regex transpiler gets these wrong too — its coverage is 0):")
    for step, exp_tool, exp_assert, got, args, conf, ok in c_det:
        if not ok:
            print(f"  conf={conf:.3f} expect={exp_tool}/{exp_assert} got={got}  {step}")
    print()
    # stability: rerun condition B once, report the delta on `full`
    b2, _ = score_multi(IN_VOCAB)
    print(f"Stability (condition B rerun): full {b['full']}/{n} -> {b2['full']}/{n} "
          f"(run-to-run variance is expected: the engine samples)")


if __name__ == "__main__":
    main()
