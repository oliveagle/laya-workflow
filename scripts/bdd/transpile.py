#!/usr/bin/env python3
"""Compile a Gherkin `.feature` into a laya-workflow spec that drives real Chrome over CDP.

    python3 scripts/bdd/transpile.py bdd/features/page_smoke.feature -o out/

One Scenario becomes one spec, because one spec is one thing a laya-workflow
run can execute and pass or fail. The mapping is one node per step:

    Given  -> arrange node   (open a page, or just declare the browser ready)
    When   -> act node       (one chrome_cdp op: open/navigate/click/type/select/evaluate)
    Then   -> assert node    (browser_base.assert with a `checks` map)

Each node keeps the page's `target_id` in state, so a When/Then pair operates
on the same tab: the `Given` opens it, and every later step in the scenario
reuses it. A step that needs a tab but has none is a compile error naming the
step to add, never a silent fresh page.

The Gherkin text is copied into the spec `description`, so any generated spec
points back at the document it came from.

An optional sidecar `<feature>.config.json` next to the feature supplies
`policy` overrides and `initial_state` (this is where Scenario Outline columns
get their default values).
"""

from __future__ import annotations

import argparse
import json
import os
import re
import sys

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))

import gherkin  # noqa: E402
import steps as stepdefs  # noqa: E402

HERE = os.path.dirname(os.path.abspath(__file__))
ROOT = os.path.abspath(os.path.join(HERE, "..", ".."))
CHROME_WRAPPER = os.path.join(ROOT, "scripts", "bdd", "chrome-headless.sh")

PLACEHOLDER_RE = re.compile(r"\$\{state\.([A-Za-z_][A-Za-z0-9_]*)\}")

DEFAULT_POLICY = {
    "allow_exec": True,
    "allow_hosts": ["127.0.0.1", "localhost"],
    "max_timeout_ms": 120000,
    "max_output": 1048576,
    "retries": 0,
}

DEFAULT_CHROME = {
    "kind": "chrome_cdp",
    # Both are state-interpolated so the runner can hand out a genuinely free
    # port and a private profile dir per run. A hard-coded port is a lie: this
    # machine already had an unrelated chrome-headless-shell sitting on 9223,
    # and the capability quite correctly refused to adopt an instance it did
    # not launch. A fixed profile dir is worse - it collides with the
    # interactive Chrome on 9222, whose lock and owner marker belong to
    # somebody else's browser window.
    "endpoint": "http://127.0.0.1:${state.cdp_port}",
    "profile_dir": "${state.cdp_profile}",
    "launch": True,
    "startup_timeout_ms": 30000,
    "timeout_ms": 15000,
    "max_text": 8000,
    "max_owned_pages": 32,
    "owned_idle_ms": 300000,
    # Headless, by pointing the capability at a wrapper that execs the real
    # Chrome with --headless=new prepended. This one field is written
    # literally rather than interpolated: `endpoint` and `profile_dir` are run
    # through `expand()` against state, but `chrome_binary` is used verbatim,
    # so a `${state.x}` here would be spawned as a filename. Generated specs
    # live in a temp dir and are never committed, so an absolute path is safe.
    #
    # Chrome will not deliver synthetic
    # mouse events to a page reporting visibilityState "hidden", and a headful
    # window nobody is looking at reports exactly that - the click is accepted
    # by CDP and then dropped on the floor, so a scenario that clicks would be
    # testing nothing. See scripts/bdd/chrome-headless.sh.
    "chrome_binary": CHROME_WRAPPER,
    # Human-shaped input (random-walk cursor paths, per-key reaction pauses) is
    # the right default for driving someone's browser, and the wrong one for a
    # test: it makes every click a different gesture, so a scenario is not
    # reproducible and a timing flake looks like a product bug. Off by default
    # here; a feature that wants to exercise the humanised path can turn it back
    # on with `{"chrome": {"human": true}}` in its .config.json.
    "human": False,
}


class TranspileError(Exception):
    pass


def _node(name: str, instruction: str, action: dict | None, keep: list[str], nxt: str) -> dict:
    return {
        "name": name,
        "primary_q": "ok",
        "questions": {
            "ok": {
                "type": "choice",
                "instructions": instruction,
                "criteria": {"A": "yes", "B": "no"},
            }
        },
        # The offline heuristic answers a choice question with its first key, so
        # a healthy scenario walks A -> A -> ... -> done. Anything that throws
        # (a CDP bail or a failed `checks`) fails the run before routing matters.
        "edge": {"condition": {"A": nxt}, "default": "STOP"},
        "state": {"keep": keep},
        **({"action": action} if action else {}),
    }


def _render(feature: gherkin.Feature, scenario: gherkin.Scenario) -> str:
    lines = [f"Feature: {feature.name}"]
    if feature.description:
        lines += ["", feature.description]
    for s in feature.background:
        lines.append(f"    {s.kind.capitalize()} {s.text}")
    lines += ["", f"  Scenario: {scenario.name}"]
    for s in scenario.steps:
        lines.append(f"    {s.kind.capitalize()} {s.text}")
    return "\n".join(lines)


def compile_scenario(
    feature: gherkin.Feature,
    scenario: gherkin.Scenario,
    config: dict | None = None,
    chrome_wrapper: str = CHROME_WRAPPER,
) -> dict:
    config = config or {}
    merged_steps = list(feature.background) + list(scenario.steps)

    nodes: list[dict] = []
    keep: list[str] = []
    has_page = False

    for st in merged_steps:
        compiled, has_page = stepdefs.compile_step(st, has_page)
        idx = len(nodes)
        # Nodes are named by position so the edge targets are predictable:
        # s0 -> s1 -> ... -> sN(done) -> STOP.
        node = _node(f"s{idx}", compiled.instruction, compiled.action, list(keep), f"s{idx + 1}")
        nodes.append(node)
        # A state key must survive node to node if any later step reads it.
        blob = json.dumps(compiled.action or {})
        for key in PLACEHOLDER_RE.findall(blob):
            if key not in keep:
                keep.append(key)
        if compiled.action and compiled.action.get("capability") == "chrome":
            for key in (compiled.action.get("project") or {}):
                if key not in keep:
                    keep.append(key)

    n = len(nodes)
    # keep is only complete now, so stamp it onto every node.
    for node in nodes:
        node["state"]["keep"] = list(keep)

    done = {
        "name": f"s{n}",
        "primary_q": "ok",
        "questions": {
            "ok": {
                "type": "choice",
                "instructions": (
                    f"Scenario {scenario.name!r} completed every step "
                    "without a failed assertion?"
                ),
                "criteria": {"A": "yes", "B": "no"},
            }
        },
        "edge": {"condition": {"A": "STOP"}, "default": "STOP"},
        "state": {"keep": list(keep)},
    }

    policy = dict(DEFAULT_POLICY)
    policy.update(config.get("policy") or {})

    chrome = dict(DEFAULT_CHROME)
    chrome["chrome_binary"] = chrome_wrapper
    chrome.update(config.get("chrome") or {})

    spec = {
        "name": f"bdd_{feature.slug}_{scenario.slug}",
        "dsl_version": 2,
        "description": (
            f"Generated from {os.path.basename(feature.path)} by "
            "scripts/bdd/transpile.py. Do not edit: change the .feature and "
            "recompile.\n\n" + _render(feature, scenario)
        ),
        "start": "s0",
        "max_iterations": n + 2,
        "convergence_window": 3,
        "convergence_eps": 0.001,
        "policy": policy,
        "capabilities": {
            "chrome": chrome,
            "bd_assert": {
                "kind": "plugin",
                "plugin": "browser_base",
                "op": "assert",
                "browser": "chrome",
                "timeout_ms": 60000,
            },
        },
        "nodes": nodes + [done],
    }
    spec["_bdd"] = {
        "feature": feature.name,
        "feature_path": os.path.relpath(feature.path),
        "scenario": scenario.name,
        "tags": scenario.tags,
        "initial_state": config.get("initial_state") or {},
    }
    return spec


def required_state(spec: dict) -> list[str]:
    """State keys the spec interpolates but does not produce itself.

    Scans the whole spec, not just the nodes: `endpoint` and `profile_dir` are
    interpolated too, and a state key the runner forgets to supply shows up as
    an opaque "CDP endpoint ${state.cdp_port} is not running" much later.
    """
    produced = set()
    for node in spec["nodes"]:
        act = node.get("action") or {}
        produced.update((act.get("project") or {}).keys())
    blob = json.dumps(spec["nodes"]) + json.dumps(spec.get("capabilities"))
    return sorted({k for k in PLACEHOLDER_RE.findall(blob) if k not in produced})


def public_spec(spec: dict) -> dict:
    out = {k: v for k, v in spec.items() if not k.startswith("_")}
    return out


def load_config(feature_path: str) -> dict:
    side = os.path.splitext(feature_path)[0] + ".config.json"
    if not os.path.exists(side):
        return {}
    with open(side, "r", encoding="utf-8") as fh:
        return json.load(fh)


def compile_feature(feature_path: str, chrome_wrapper: str = CHROME_WRAPPER) -> list[dict]:
    feature = gherkin.load(feature_path)
    config = load_config(feature_path)
    return [compile_scenario(feature, s, config, chrome_wrapper) for s in feature.scenarios]


def main(argv: list[str] | None = None) -> int:
    ap = argparse.ArgumentParser(description=__doc__.split("\n")[0])
    ap.add_argument("features", nargs="+", help=".feature files")
    ap.add_argument("-o", "--out", help="write generated specs into this directory")
    ap.add_argument("--list", action="store_true", help="list scenarios, generate nothing")
    ap.add_argument("--check", action="store_true",
                    help="generate, verify each spec is well-formed, write nothing")
    ap.add_argument("--chrome-bin", default=CHROME_WRAPPER,
                    help="Chrome wrapper that forces headless (see scripts/bdd/chrome-headless.sh)")
    args = ap.parse_args(argv)

    total = 0
    for fp in args.features:
        try:
            specs = compile_feature(fp, args.chrome_bin)
        except (gherkin.GherkinError, stepdefs.UnknownStep, stepdefs.StepError,
                TranspileError, json.JSONDecodeError) as e:
            print(f"error: {fp}: {e}", file=sys.stderr)
            return 1

        for spec in specs:
            total += 1
            meta = spec["_bdd"]
            need = required_state(spec)
            if args.list:
                print(f"{os.path.basename(fp)} :: {meta['scenario']}"
                      + (f"   [state: {', '.join(need)}]" if need else ""))
                continue

            payload = public_spec(spec)
            body = json.dumps(payload, indent=2, ensure_ascii=False) + "\n"

            if args.check:
                try:
                    json.loads(body)
                except json.JSONDecodeError as e:  # pragma: no cover
                    print(f"error: {meta['scenario']}: {e}", file=sys.stderr)
                    return 1
                if not spec["nodes"] or not payload["start"]:
                    print(f"error: {meta['scenario']}: empty spec", file=sys.stderr)
                    return 1
                print(f"ok: {os.path.basename(fp)} :: {meta['scenario']} "
                      f"({len(spec['nodes'])} nodes"
                      + (f", state: {', '.join(need)}" if need else "") + ")")
                continue

            if not args.out:
                print(body, end="")
                continue
            os.makedirs(args.out, exist_ok=True)
            dest = os.path.join(args.out, spec["name"] + ".json")
            with open(dest, "w", encoding="utf-8") as fh:
                fh.write(body)
            print(f"wrote {dest}")

    if not total:
        print("no scenarios found", file=sys.stderr)
        return 1
    return 0


if __name__ == "__main__":
    sys.exit(main())
