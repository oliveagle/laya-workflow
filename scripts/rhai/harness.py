#!/usr/bin/env python3
"""Run part of a plugin against the real engine, without the rest of it.

A Rhai plugin is one file with a `fn run(host, ctx)` at the bottom. Everything
above it is a library. This takes the library and swaps in your own `run`, then
writes the spec that runs it — which turns "does my helper do what I think"
from a 25-second browse into a 0.03-second call.

    harness.py plugin.rhai --body 'fn run(host, ctx) { #{ ok: helper(2) } }'
    harness.py plugin.rhai --body -            # body on stdin
    harness.py plugin.rhai --body b.rhai --out /tmp/h --run   # build, then run

Everything above `fn run(` is kept verbatim, so the harness tests the real
helpers rather than a copy that can drift.
"""
import argparse
import json
import os
import subprocess
import sys

HERE = os.path.dirname(os.path.abspath(__file__))
sys.path.insert(0, HERE)
from compile import Compiler, find_engine  # noqa: E402

SPEC = {
    "name": "h", "dsl_version": 2, "start": "p", "max_iterations": 1,
    "policy": {"allow_exec": True, "allow_hosts": [],
               "allow_paths": ["${env.HOME}/.laya-workflow", "${env.HOME}/tmp"],
               "max_timeout_ms": 120000, "max_output": 8388608, "retries": 0},
    "capabilities": {"p": {"kind": "plugin", "dir": "", "timeout_ms": 60000,
                           "max_operations": 200000000}},
    "nodes": [{"name": "p", "edge": {"condition": {}, "default": "STOP"},
               "action": {"kind": "call", "capability": "p", "with": {},
                          "project": {"result": "/result", "probe_error": "/probe_error"}},
               "primary_q": "done",
               "questions": {"done": {"type": "choice", "instructions": "ok?",
                                      "criteria": {"A": "yes", "B": "no"}}}}],
}


def build(plugin, body, out):
    lines = open(plugin).read().split("\n")
    starts = [i for i, l in enumerate(lines) if l.startswith("fn run(")]
    if not starts:
        raise SystemExit("%s has no top-level `fn run(host, ctx)`" % plugin)
    head = "\n".join(lines[:starts[0]])
    text = body if body.lstrip().startswith("fn ") else \
        "fn run(host, ctx) {\n" + body + "\n}\n"
    os.makedirs(out, exist_ok=True)
    rhai = os.path.join(out, "harness.rhai")
    with open(rhai, "w") as f:
        f.write(head + "\n" + text)
    spec = json.loads(json.dumps(SPEC))
    spec["capabilities"]["p"]["dir"] = rhai
    sp = os.path.join(out, "spec.json")
    with open(sp, "w") as f:
        json.dump(spec, f)
    return rhai, sp


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("plugin")
    ap.add_argument("--body", default='fn run(host, ctx) { #{ ok: true } }')
    ap.add_argument("--out", default="/tmp/rhai-harness")
    ap.add_argument("--engine")
    ap.add_argument("--run", action="store_true", help="run it after building")
    args = ap.parse_args()

    if args.body == "-":
        body = sys.stdin.read()
    elif os.path.exists(args.body):
        body = open(args.body).read()
    else:
        body = args.body
    rhai, spec = build(args.plugin, body, args.out)
    print("harness: %s\nspec:    %s" % (rhai, spec))
    if not args.run:
        return 0
    r = subprocess.run([find_engine(args.engine), "run", "--spec", spec],
                       capture_output=True, text=True)
    out = r.stdout + r.stderr
    i = out.find('"result"')
    print(out[i:i + 4000] if i >= 0 else out[:4000])
    return r.returncode


if __name__ == "__main__":
    sys.exit(main())
