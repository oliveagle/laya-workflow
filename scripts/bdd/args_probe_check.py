#!/usr/bin/env python3
"""Run the bdd plugin's argument-validation errors, and hold the list complete.

`laya-workflow validate` cannot see any of this (round 9: it passes specs whose
`edge` names a node that does not exist), and the four hand-written probes under
`dsl/browser/` all need a real Chrome tab, so they cover the *browser* half of the
plugin and none of the argument half. Measured, of the 17 `throw` sites in
`plugins/bdd/main.rhai`, 5 were reachable from a probe and 12 had never been
executed by anything - and the 12 are exactly the messages a hand-written spec
author sees first, since they fire before the plugin touches CDP.

So they are tested here, from `bdd/args_probes.json`, and:

  1. every row's `with` is materialised into a spec, run, and required to exit
     non-zero with `must_contain` in the output;
  2. every `throw` in the plugin is claimed by a row or by an `elsewhere` entry
     carrying a reason, so a *new* error message fails the build;
  3. every row's and `elsewhere` entry's `source` still matches exactly one
     `throw`, so deleting a row fails too.

Checks 2 and 3 are the point. A list of assertions is not a gate; a list that
cannot notice the thing it exists to cover is decoration.

No Chrome, no network, no fixture server: 15 binary invocations in ~25ms. That
is what lets this live in check.sh, which CI runs - run.py is local-only
because CI installs no browser.

    scripts/bdd/args_probe_check.py [path-to-laya-workflow]
"""

from __future__ import annotations

import json
import os
import re
import subprocess
import sys
import tempfile

HERE = os.path.dirname(os.path.abspath(__file__))
ROOT = os.path.abspath(os.path.join(HERE, "..", ".."))
PLUGIN = os.path.join(ROOT, "plugins", "bdd", "main.rhai")
TABLE = os.path.join(ROOT, "bdd", "args_probes.json")

THROW_RE = re.compile(r"\bthrow\b")
LINE_COMMENT_RE = re.compile(r"//[^\n]*")


def throw_statements() -> list[str]:
    """Every `throw` in the plugin, one line each, comments stripped.

    Comments are stripped because the plugin's header documents that a failing
    assertion "throws", and counting prose as a throw site would make the
    completeness check meaningless. Multi-line throws are joined, because
    `rustfmt`-style wrapping means a site is rarely on one line.
    """
    with open(PLUGIN, encoding="utf-8") as f:
        src = f.read()
    # Only the code matters; the header comment is prose about the code.
    src = LINE_COMMENT_RE.sub("", src)
    out: list[str] = []
    for m in THROW_RE.finditer(src):
        end = src.find(";", m.start())
        stmt = src[m.start():end if end > 0 else m.start() + 200]
        out.append(re.sub(r"\s+", " ", stmt).strip())
    return out


def build_spec(table: dict, row: dict) -> dict:
    return {
        "name": f"bdd_args_{row['name']}",
        "dsl_version": table["spec_defaults"]["dsl_version"],
        "description": f"bdd must refuse this: {row['blurb']}",
        "start": table["spec_defaults"]["start"],
        "max_iterations": table["spec_defaults"]["max_iterations"],
        "convergence_window": table["spec_defaults"]["convergence_window"],
        "convergence_eps": table["spec_defaults"]["convergence_eps"],
        "policy": table["spec_defaults"]["policy"],
        "capabilities": table["capabilities"],
        "nodes": [{
            "name": table["spec_defaults"]["start"],
            "primary_q": "ok",
            "questions": {"ok": {
                "type": "choice",
                "instructions": "Did the plugin refuse this with the documented message?",
                "criteria": {"A": "yes", "B": "no"},
            }},
            "edge": {"condition": {"A": "STOP"}, "default": "STOP"},
            "state": {"keep": []},
            "action": {"kind": "call", "capability": "bdd", "with": row["with"]},
        }],
    }


def find_binary(argv: list[str]) -> str:
    if len(argv) > 1:
        return argv[1]
    for rel in ("target/release/laya-workflow", "target/debug/laya-workflow"):
        cand = os.path.join(ROOT, rel)
        if os.access(cand, os.X_OK):
            return cand
    return ""


def main() -> int:
    problems: list[str] = []
    binary = find_binary(sys.argv)
    if not binary:
        print("bdd args probes: no laya-workflow binary; build it or pass the path",
              file=sys.stderr)
        return 1
    with open(TABLE, encoding="utf-8") as f:
        table = json.load(f)

    stmts = throw_statements()
    if not stmts:
        print(f"bdd args probes: no `throw` found in {PLUGIN} - the patterns in "
              "args_probe_check.py no longer match the plugin", file=sys.stderr)
        return 1

    # ── 2 and 3: is the table still a complete, accurate map of the throws? ──
    claimed: set[str] = set()
    for row in table["rows"]:
        hits = [s for s in stmts if row["source"] in s]
        if len(hits) != 1:
            problems.append(
                f"row {row['name']!r}: its source {row['source']!r} matches "
                f"{len(hits)} of the plugin's {len(stmts)} throw sites, expected 1 - "
                "the message was reworded, or the anchor is now ambiguous")
            continue
        claimed.add(hits[0])
    for entry in table["elsewhere"]:
        hits = [s for s in stmts if entry["source"] in s]
        if len(hits) != 1:
            problems.append(
                f"elsewhere entry {entry['source']!r} matches {len(hits)} throw "
                f"sites, expected 1 - it no longer describes a real one")
            continue
        claimed.add(hits[0])
        if not entry.get("why"):
            problems.append(
                f"elsewhere entry {entry['source']!r} has no reason; a throw the "
                "table skips has to say why it is not skipped here")
    for stmt in stmts:
        if stmt not in claimed:
            problems.append(
                f"the plugin throws {stmt!r} and nothing in bdd/args_probes.json "
                "claims it - add a row, or an elsewhere entry saying why it is not "
                "pinned here. An unpinned error message is one nobody reads twice.")

    # ── 1: does each row actually produce its message? ──
    ran = 0
    with tempfile.TemporaryDirectory() as tmp:
        for row in table["rows"]:
            spec_path = os.path.join(tmp, f"{row['name']}.json")
            with open(spec_path, "w", encoding="utf-8") as f:
                json.dump(build_spec(table, row), f)
            try:
                proc = subprocess.run(
                    [binary, "run", "--spec", spec_path, "--state", "{}"],
                    capture_output=True, text=True, timeout=30)
            except subprocess.TimeoutExpired:
                problems.append(f"row {row['name']!r} ({row['blurb']}): the run "
                                "timed out; this path is supposed to refuse before "
                                "touching a browser, so a hang is a real defect")
                continue
            ran += 1
            out = proc.stdout + proc.stderr
            if proc.returncode == 0:
                problems.append(
                    f"row {row['name']!r} ({row['blurb']}): the run exited 0, but "
                    f"the plugin is supposed to refuse with {row['must_contain']!r}")
            elif row["must_contain"] not in out:
                first = next((l for l in out.splitlines() if l.startswith("Error:")), "")
                problems.append(
                    f"row {row['name']!r} ({row['blurb']}): the run failed without "
                    f"saying {row['must_contain']!r} - got: {first[:120]}")

    for p in problems:
        print(f"bdd args probes: {p}", file=sys.stderr)

    rows, elsewhere = len(table["rows"]), len(table["elsewhere"])
    if problems:
        print(f"bdd args probes: {len(problems)} problem(s)", file=sys.stderr)
        return 1
    print(f"bdd args probes: {ran}/{rows} argument errors refuse with the message "
          f"they claim, and all {len(stmts)} throw sites are accounted for "
          f"({rows} pinned here, {elsewhere} declared elsewhere)")
    return 0


if __name__ == "__main__":
    sys.exit(main())
