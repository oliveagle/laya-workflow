#!/usr/bin/env python3
"""Compile-check a Rhai plugin, and localise a compile error to a line range.

`laya-workflow run` is the only compiler, but running a whole workflow to learn
"does this parse" wastes the whole run. This compiles the plugin alone, in
~10ms, and with --bisect narrows a parse error to the block that causes it.

    compile.py plugin.rhai              # exit 0 ok / 1 error, message on stderr
    compile.py plugin.rhai --bisect     # ... and print the offending window
    compile.py plugin.rhai --engine ./target/release/laya-workflow

Why bisect is needed at all: this engine reports "Expecting '{' to start a
statement block (line 1965, position 34)" for a stray `)` three hundred lines
away from anything suspicious, because the parser recovers at the next token it
can use. Reading the reported line first is how you end up debugging the wrong
line for ten minutes.
"""
import argparse
import json
import os
import re
import subprocess
import sys
import tempfile

SPEC = {
    "name": "compile", "dsl_version": 2, "start": "p", "max_iterations": 1,
    "policy": {"allow_exec": False, "allow_hosts": [],
               "allow_paths": ["${env.HOME}/.laya-workflow", "${env.HOME}/tmp"],
               "max_timeout_ms": 120000, "max_output": 8388608, "retries": 0},
    "capabilities": {"p": {"kind": "plugin", "dir": "", "timeout_ms": 60000,
                           "max_operations": 200000000}},
    "nodes": [{"name": "p", "edge": {"condition": {}, "default": "STOP"},
               "action": {"kind": "call", "capability": "p", "with": {},
                          "project": {"ok": "/ok", "probe_error": "/probe_error"}},
               "primary_q": "done",
               "questions": {"done": {"type": "choice", "instructions": "ok?",
                                      "criteria": {"A": "yes", "B": "no"}}}}],
}


def find_engine(explicit=None):
    if explicit:
        return explicit
    root = os.path.dirname(os.path.dirname(os.path.dirname(os.path.abspath(__file__))))
    cand = os.path.join(root, "target", "release", "laya-workflow")
    return cand if os.path.exists(cand) else "laya-workflow"


class Compiler:
    def __init__(self, engine, workdir):
        self.engine = engine
        self.dir = workdir
        self.calls = 0

    def check(self, path):
        """Return the compile error message, or None when the plugin compiles."""
        self.calls += 1
        spec = json.loads(json.dumps(SPEC))
        spec["capabilities"]["p"]["dir"] = os.path.abspath(path)
        sp = os.path.join(self.dir, "spec.json")
        with open(sp, "w") as f:
            json.dump(spec, f)
        r = subprocess.run([self.engine, "run", "--spec", sp],
                           capture_output=True, text=True, cwd=os.path.dirname(self.engine) or ".")
        out = r.stdout + r.stderr
        m = re.search(r"did not compile: (.*)", out)
        return m.group(1).strip() if m else None

    def check_lines(self, lines, tag="bs"):
        p = os.path.join(self.dir, tag + ".rhai")
        with open(p, "w") as f:
            f.write("\n".join(lines))
        return self.check(p)


def comment_out(lines, a, b):
    """Blank lines a..b (1-based, inclusive). Comments cannot unbalance braces,
    which is what makes this a safe thing to do at an arbitrary cut point."""
    out = list(lines)
    for i in range(a - 1, min(b, len(out))):
        if out[i].strip():
            out[i] = "//" + out[i]
    return out


def enclosing_fn(lines, line):
    for i in range(min(line, len(lines)) - 1, -1, -1):
        if lines[i].startswith("fn "):
            return lines[i].split("(")[0][3:], i + 1
    return "<top level>", 1


def bisect(comp, lines, err):
    """Smallest window around the reported line whose *removal* compiles clean.

    The parser recovers, so the line it names is where it gave up, not where the
    mistake is. The useful question is therefore not "what is wrong on line N"
    but "what is the least I can take out to make it parse" — so the predicate
    is a clean compile, and the search doubles outward from the reported line
    then halves in. Returns None when no window works, i.e. there is more than
    one thing wrong.
    """
    m = re.search(r"line (\d+)", err)
    if not m:
        return None
    line = int(m.group(1))
    line = max(1, min(line, len(lines)))

    def clean(a, b):
        return comp.check_lines(comment_out(lines, a, b), "bis") is None

    span = 8
    while span <= len(lines):
        if clean(max(1, line - span), min(len(lines), line + span)):
            break
        span *= 2
    else:
        return None
    lo, hi = 0, span                     # halve the window that worked
    while lo < hi:
        mid = (lo + hi) // 2
        if clean(max(1, line - mid), min(len(lines), line + mid)):
            hi = mid
        else:
            lo = mid + 1
    a, b = max(1, line - lo), min(len(lines), line + lo)
    return a, b, line, span


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("plugin")
    ap.add_argument("--bisect", action="store_true")
    ap.add_argument("--engine")
    args = ap.parse_args()

    engine = find_engine(args.engine)
    lines = open(args.plugin).read().split("\n")
    with tempfile.TemporaryDirectory() as tmp:
        comp = Compiler(engine, tmp)
        err = comp.check_lines(lines, "orig")
        if err is None:
            print("ok: %s compiles (%d compile call)" % (args.plugin, comp.calls))
            return 0
        print("error: %s" % err, file=sys.stderr)
        if not args.bisect:
            return 1

        fn, fnline = enclosing_fn(lines, int(re.search(r"line (\d+)", err).group(1)))
        print("\nreported at: %s" % err, file=sys.stderr)
        print("inside fn:  %s (line %d)" % (fn, fnline), file=sys.stderr)
        got = bisect(comp, lines, err)
        if not got:
            print("\nno window around it compiles clean: more than one problem,"
                  "\nor the error is not a syntax error. Read the message.",
                  file=sys.stderr)
            return 1
        a, b, line, span = got
        width = b - a + 1
        print("\nreported line %d; the smallest block whose removal compiles"
              "\nclean is %d-%d (%d lines, needed a +/-%d window, %d compiles)"
              % (line, a, b, width, span, comp.calls), file=sys.stderr)
        show = range(a - 1, min(b, len(lines)))
        if width > 41:                    # keep the output readable
            lo = max(a, line - 12)
            hi = min(b, line + 12)
            show = list(range(a - 1, lo - 1)) + list(range(lo - 1, hi)) + \
                list(range(hi, min(b, len(lines))))
        for i in show:
            mark = ">>" if i + 1 == line else "  "
            print("%s %5d %s" % (mark, i + 1, lines[i]), file=sys.stderr)
        return 1


if __name__ == "__main__":
    sys.exit(main())
