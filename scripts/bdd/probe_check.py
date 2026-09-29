#!/usr/bin/env python3
"""Check the hand-written BDD probe specs without Chrome.

`laya-workflow validate` is the wrong tool for these, and saying so is the
point of this file. Measured, a probe with

  * an `edge.condition` naming a node that does not exist, and
  * an `action.with` argument of `null`

both **pass** `validate` and both fail at run time. The first is not
hypothetical: it is exactly how bdd_release_probe.json was born broken, where
the run stopped after the first node with `final_action: error_node_missing`
and a zero exit code - so the runner called it a pass until the probe was
rewritten to invert its verdict.

So the probes are gated by this instead. Four checks, none of which needs a
browser:

  1. every node has a unique name, and the start node exists
  2. every `edge.condition` target is a real node or the terminal STOP
  3. every node is reachable from the start node (an unreachable node is a
     node whose assertion is never made)
  4. every `${state.X}` a node reads is either kept by some node or supplied by
     the runner, and every `action.capability` is declared in `capabilities`

The spec list is imported from run.py rather than globbed, so a probe cannot be
added to one and not the other.
"""

from __future__ import annotations

import json
import os
import re
import sys

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))

import run as runner  # noqa: E402

TERMINAL = "STOP"
STATE_REF = re.compile(r"\$\{state\.([A-Za-z_][A-Za-z0-9_]*)\}")

# What scripts/bdd/run.py injects into state for every probe it runs. Anything
# else has to be kept by a node.
RUNNER_STATE = {"url", "cdp_port", "cdp_profile"}


def probes() -> list[str]:
    paths = [runner.HANDWRITTEN_SPEC, runner.RELEASE_PROBE,
             runner.WAIT_PROBE, runner.RETRY_PROBE,
             runner.BROWSER_BASE_PROBE]
    return [p for p in paths if os.path.isfile(p)]


def check(path: str) -> list[str]:
    problems: list[str] = []
    rel = os.path.relpath(path, runner.ROOT)
    with open(path, encoding="utf-8") as f:
        spec = json.load(f)

    nodes = spec.get("nodes") or []
    if not nodes:
        return [f"{rel}: no nodes"]

    names: list[str] = []
    for n in nodes:
        name = n.get("name")
        if not name:
            problems.append(f"{rel}: a node has no name")
        elif name in names:
            problems.append(f"{rel}: duplicate node name {name!r} - an edge "
                            "naming it is ambiguous")
        else:
            names.append(name)

    start = spec.get("start")
    if start not in names:
        problems.append(f"{rel}: start {start!r} is not a node; the run would "
                        "have nothing to do")
        return problems

    capabilities = set((spec.get("capabilities") or {}).keys())

    # (1) edge targets must exist
    adjacency: dict[str, set[str]] = {}
    for n in nodes:
        name = n.get("name")
        targets: set[str] = set()
        edge = n.get("edge") or {}
        for key, dest in (edge.get("condition") or {}).items():
            if dest != TERMINAL:
                targets.add(dest)
                if dest not in names:
                    problems.append(
                        f"{rel}: node {name!r} routes {key} -> {dest!r}, which is "
                        "not a node. `validate` accepts this and the run stops "
                        "early with error_node_missing and a zero exit code, "
                        "which a green-only suite reads as a pass")
        if edge.get("default") and edge["default"] != TERMINAL:
            dest = edge["default"]
            targets.add(dest)
            if dest not in names:
                problems.append(f"{rel}: node {name!r} default -> {dest!r}, "
                                "which is not a node")
        if name:
            adjacency[name] = targets

        action = n.get("action")
        if action:
            cap = action.get("capability")
            if cap and cap not in capabilities:
                problems.append(
                    f"{rel}: node {name!r} calls capability {cap!r}, which is "
                    f"not declared (have: {', '.join(sorted(capabilities)) or 'none'})")

    # (2) reachability - an unreachable node is an assertion that never runs
    seen: set[str] = set()
    stack = [start]
    while stack:
        cur = stack.pop()
        if cur in seen or cur not in adjacency:
            continue
        seen.add(cur)
        stack.extend(adjacency[cur])
    for name in names:
        if name not in seen:
            problems.append(
                f"{rel}: node {name!r} is unreachable from {start!r}, so "
                "whatever it checks is never checked")

    # (3) state keys
    kept: set[str] = set(RUNNER_STATE)
    for n in nodes:
        kept.update((n.get("state") or {}).get("keep") or [])
    for n in nodes:
        name = n.get("name")
        blob = json.dumps({k: v for k, v in n.items() if k != "state"})
        for key in sorted(set(STATE_REF.findall(blob))):
            if key not in kept:
                problems.append(
                    f"{rel}: node {name!r} reads ${{state.{key}}}, which no node "
                    "keeps and the runner does not supply")

    return problems


def main() -> int:
    paths = probes()
    if not paths:
        print("bdd probes: none found - skipped")
        return 0
    problems: list[str] = []
    for path in paths:
        problems.extend(check(path))
    if problems:
        print(f"bdd probes: {len(problems)} problem(s):", file=sys.stderr)
        for p in problems:
            print(f"  {p}", file=sys.stderr)
        return 1
    print(f"bdd probes: {len(paths)} hand-written spec(s) are connected, "
          "reachable and fully wired")
    return 0


if __name__ == "__main__":
    sys.exit(main())
