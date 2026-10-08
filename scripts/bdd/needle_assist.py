#!/usr/bin/env python3
"""Needle 3 assist for BDD step compilation — SUGGESTIONS ONLY, never auto-write.

The deterministic compiler (steps.py) is the accuracy path: a step it cannot
compile is a hard error, by design. This module is the narrow, gated slot where
a small on-device model may help an agent that is *extending the vocabulary*:
given a step the regexes do not know, it asks Needle 3 (multi-tool + triggers,
the vendor-recommended layout) what op/assertion the step probably means, and
returns a suggestion with the engine's confidence.

Accuracy rules (bench/bdd_to_needle.py measured the ceiling):
  * in-vocabulary  full-record ~60%, op routing ~90%
  * novel phrasings full-record ~40%, 1/10 above the 0.1 confidence floor
So a suggestion is an *input to a human/agent decision*, never a compile.
`accept(conf, floor)` implements the policy: only confidence above `floor`
(default 0.5) is worth showing as "likely"; everything else is "guess".

Usage:
    from needle_assist import suggest, ACCEPT_FLOOR
    s = suggest("Then the heading \\"#hero\\" should be displayed", bin)
    # -> {"op": "assert", "assertion": "visible", "value": "#hero",
    #     "confidence": 0.37, "matched_tool": "assert"} or None when the
    #     engine/weights are absent or nothing was extracted
"""
from __future__ import annotations

import json
import os
import subprocess

# One tool per op + triggers regexes: the layout Needle's own guide recommends
# for routing, and the only layout the bench found usable (op ~90% in-vocab).
TOOLS = [
    {"type": "function", "name": "open_page", "description": "Open a URL and wait until it is loaded.",
     "triggers": [r"\b(am on|open)\b"],
     "parameters": {"type": "object", "properties": {"url": {"type": "string"}}, "required": ["url"]}},
    {"type": "function", "name": "navigate", "description": "Move an open tab to a new URL.",
     "triggers": [r"\bnavigate\b"],
     "parameters": {"type": "object", "properties": {"url": {"type": "string"}}, "required": ["url"]}},
    {"type": "function", "name": "wait_for", "description": "Poll until a CSS selector matches.",
     "triggers": [r"\bwait for\b"],
     "parameters": {"type": "object", "properties": {"selector": {"type": "string"}}, "required": ["selector"]}},
    {"type": "function", "name": "click", "description": "Click a CSS selector.",
     "triggers": [r"\bclick\b"],
     "parameters": {"type": "object", "properties": {"selector": {"type": "string"}}, "required": ["selector"]}},
    {"type": "function", "name": "type_text", "description": "Type text into a CSS selector.",
     "triggers": [r"\btype\b"],
     "parameters": {"type": "object", "properties": {"text": {"type": "string"}, "selector": {"type": "string"}},
                    "required": ["text", "selector"]}},
    {"type": "function", "name": "select_option", "description": "Select a value in a CSS selector.",
     "triggers": [r"\bselect\b"],
     "parameters": {"type": "object", "properties": {"value": {"type": "string"}, "selector": {"type": "string"}},
                    "required": ["value", "selector"]}},
    {"type": "function", "name": "run_js", "description": "Evaluate a javascript expression in the page.",
     "triggers": [r"\brun javascript\b"],
     "parameters": {"type": "object", "properties": {"expression": {"type": "string"}}, "required": ["expression"]}},
    {"type": "function", "name": "assert", "description": "Run a named page assertion: title_contains, url_contains, visible, absent, is_true, is_false, equals, contains, equals_text.",
     "triggers": [r"\b(title|url) contains|is visible|is absent|is true|is false|equals|contains\b"],
     "parameters": {"type": "object",
                    "properties": {"assertion": {"type": "string"}, "value": {"type": "string"},
                                   "expected": {"type": "string"}},
                    "required": ["assertion"]}},
    {"type": "function", "name": "release_page", "description": "Close the current page.",
     "triggers": [r"\brelease\b"],
     "parameters": {"type": "object", "properties": {"target_id": {"type": "string"}}, "required": []}},
]

# The bench's measured model: below 0.5 even the "right" routing is too often a
# guess. 0.5 is the floor for "likely"; anything below prints as "guess".
ACCEPT_FLOOR = 0.5

SYSTEM = "You are a Gherkin-to-workflow compiler. Call exactly one tool per step."

# The op/tool names differ between the plugin and the tool set; keep the map in
# one place so a suggestion's `op` is always a plugin op.
TOOL_TO_OP = {"open_page": "open", "navigate": "navigate", "wait_for": "wait_for",
              "click": "click", "type_text": "type", "select_option": "select",
              "run_js": "evaluate", "assert": "assert", "release_page": "release"}


def _mcp_tool(bin: str, name: str, args: dict) -> dict | None:
    req = {"jsonrpc": "2.0", "id": 1, "method": "tools/call",
           "params": {"name": name, "arguments": args}}
    try:
        p = subprocess.run([bin, "mcp", "serve"], input=json.dumps(req),
                           capture_output=True, text=True, timeout=120)
    except (OSError, subprocess.SubprocessError):
        return None
    if p.returncode != 0:
        return None
    try:
        d = json.loads(p.stdout)
        return json.loads(d["result"]["content"][0]["text"])
    except (KeyError, ValueError, json.JSONDecodeError):
        return None


def suggest(step: str, bin: str = "laya-workflow") -> dict | None:
    """Return {op, assertion?, value?, expected?, confidence, matched_tool} or None."""
    r = _mcp_tool(bin, "needle_complete",
                  {"prompt": step, "tools": TOOLS, "system": SYSTEM})
    if not r:
        return None
    calls = r.get("function_calls") or []
    supp = r.get("suppressed_calls") or []
    item = calls[0] if calls else (supp[0] if supp else None)
    if not item:
        return None
    args = item.get("arguments") or {}
    op = TOOL_TO_OP.get(item.get("name"))
    if not op:
        return None
    out = {"op": op, "confidence": r.get("confidence")}
    for k in ("assertion", "value", "expected"):
        if args.get(k):
            out[k] = args[k]
    out["matched_tool"] = item.get("name")
    return out
