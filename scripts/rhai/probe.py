#!/usr/bin/env python3
"""What this Rhai engine actually does — answered by running it, once.

Every one of these cost a round trip to discover at least once, because the
answer is not what the language documentation implies: this build registers a
reduced package set, sorts in place and returns unit, and copies a map when it
is pushed into an array. Guessing wrong costs minutes; this costs about a second.

    probe.py                 # the whole table
    probe.py --grep map      # only the questions about maps

Each question gets its own file on purpose: a construct the engine rejects would
take the whole probe down with it if they shared one.
"""
import argparse
import json
import os
import subprocess
import sys
import tempfile

HERE = os.path.dirname(os.path.abspath(__file__))
sys.path.insert(0, HERE)
from compile import Compiler, find_engine

PROBES = [
    ("arrays: does sort() return the array or unit?",
     'let a = [3, 1, 2]; let r = a.sort(); #{ ret: type_of(r), a: a }',
     "unit sorts in place; array is returned"),
    ("arrays: is join available?",
     'let a = ["x", "y"]; #{ r: a.join(",") }', "string, or a compile error"),
    ("arrays: is index assignment allowed?",
     'let a = [1, 2]; a[0] = 9; #{ a: a }', "[9, 2]"),
    ("arrays: does iteration yield a reference or a copy?",
     'let a = [#{ n: 1 }]; for x in a { x["n"] = 99; } #{ a: a[0]["n"] }',
     "99 is a reference; 1 is a copy"),
    ("arrays: is remove available?",
     'let a = [1, 2, 3]; a.remove(0); #{ a: a }', "[2, 3]"),
    ("maps: does += work on a key?",
     'let m = #{}; m["k"] = 1; m["k"] += 2; #{ v: m["k"] }', "3"),
    ("maps: does for k in map iterate keys?",
     'let m = #{ a: 1, b: 2 }; let n = 0; for k in m { n += 1; } #{ n: n }', "2"),
    ("maps: is contains the membership test?",
     'let m = #{ a: 1 }; #{ has: m.contains("a"), miss: m.contains("z") }', "true/false"),
    ("maps: is a map literal allowed as an if branch value?",
     'let m = if 1 > 0 { #{ ok: true } } else { #{ ok: false } }; #{ ok: m["ok"] }',
     "true"),
    ("maps: are CJK keys allowed unquoted?",
     'let m = #{ 功能完好: 1 }; #{ v: m["功能完好"] }', "1"),
    ("functions: may a fn be defined inside a fn?",
     'fn outer() { fn inner() { 1 } inner() } #{ v: outer() }', "1"),
    ("functions: are closures available?",
     'let f = |x| x + 1; #{ v: f(1) }', "2"),
    ("functions: does break exit the inner of two nested loops?",
     'let n = 0; for i in 0..3 { for j in 0..3 { n += 1; break; } } #{ n: n }', "3"),
    ("functions: is with usable as an identifier?",
     'let with = 1; #{ v: with }', "compiles, so not reserved here"),
    ("types: is type_of(v) equal to () the unit test?",
     'let m = #{}; let v = if m.contains("nope") { m["nope"] } else { () }; #{ u: type_of(v) }',
     'the unit type name'),
    ("types: does a unit value survive as a map value?",
     'let m = #{ a: () }; #{ t: type_of(m["a"]) }', "unit again"),
    ("types: is integer division integer?",
     'let a = 7 / 2; let b = 7.0 / 2.0; #{ i: a, f: b }', "3 and 3.5"),
    ("types: is int_of available?",
     'let v = int_of(7.9); #{ v: v }', "7"),
    ("types: is parse_int available?",
     'let v = parse_int("42"); #{ v: v }', "42"),
    ("strings: is replace available?",
     'let s = "a-b-c"; #{ r: s.replace("-", "+") }', "a+b+c"),
    ("strings: is index_of available, and -1 for a miss?",
     'let s = "abc"; #{ hit: s.index_of("b"), miss: s.index_of("z") }', "1 and -1"),
    ("strings: is starts_with available?",
     'let s = "abc"; #{ v: s.starts_with("ab") }', "true"),
    ("strings: does += concatenate?",
     'let s = "a"; s += "b"; #{ s: s }', "ab"),
    ("strings: is to_lower available?",
     'let s = "AbC"; #{ s: s.to_lower() }', "abc"),
    ("strings: is sub_string indexed in bytes or chars for CJK?",
     'let s = "价格进化"; #{ n: s.len(), head: s.sub_string(0, 2) }',
     "n=4 means chars; n=12 means bytes"),
    ("numbers: are abs, min, max available as free functions?",
     'let m = if abs(-2) > min(1, 2) { max(1, 2) } else { 0 }; #{ v: m }', "2"),
    ("numbers: is the method form available?",
     'let v = (-2).abs(); #{ v: v }', "2"),
    ("engine: is throw catchable as a string?",
     'let m = "none"; try { throw "boom" } catch (e) { m = e; } #{ m: m }', "boom"),
    ("engine: is try an expression or a statement?",
     'let v = try { 41 + 1 }; #{ v: v }',
     "a value means expression; unit means statement only, assign inside"),
    ("engine: what is the way to get a value out of a try?",
     'let v = "unset"; try { v = 42; } catch (e) { v = 0; } #{ v: v }',
     "42 means assign inside the block, the only form that works"),
    ("maps: can a CJK key be set by index?",
     'let m = #{}; m["功能完好"] = 1; #{ v: m["功能完好"] }', "1"),
    ("types: is int_of provided by the engine?",
     'let v = int_of(30.7); #{ v: v }',
     "not provided; a plugin that wants one defines it itself"),
    ("numbers: does round exist?",
     'let v = (30.7).round(); #{ v: v }', "31"),
    ("numbers: does floor exist?",
     'let v = (30.7).floor(); #{ v: v }', "30"),
    ("engine: does host.now give rfc3339 and unix_ms?",
     'let n = host.now(); #{ rfc: n.contains("rfc3339"), ms: n.contains("unix_ms") }',
     "true and true"),
]


def run_probe(engine, tmp, body):
    """Three outcomes, kept apart on purpose: a construct that will not compile
    and one that compiles but throws are different facts, and collapsing them
    into "no" is how a probe ends up asserting something false."""
    path = os.path.join(tmp, "probe.rhai")
    fh = open(path, "w")
    fh.write("fn run(host, ctx) {\n" + body + "\n}\n")
    fh.close()
    err = Compiler(engine, tmp).check(path)
    if err:
        return "NO-COMPILE", err.split("(")[0].strip()
    r = subprocess.run([engine, "run", "--spec", os.path.join(tmp, "spec.json")],
                       capture_output=True, text=True)
    out = r.stdout + r.stderr
    for line in out.split("\n"):
        if line.startswith("Error:"):
            return "NO-RUNTIME", line[7:].strip()[:140]
    start = out.find("{")
    end = out.find('"trace"')
    if start < 0:
        return "NO-RUNTIME", (out.strip().split("\n") or ["?"])[-1][:140]
    tail = out[start:end] if end > start else out[start:]
    tail = tail.rstrip().rstrip(",") + "}"
    try:
        blob = json.loads(tail)
    except ValueError:
        return "NO-RUNTIME", tail[:140]
    got = blob.get("result", {})
    got = got.get("result", got)
    got.pop("capability", None)
    return "OK", json.dumps(got, ensure_ascii=False)


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--grep")
    ap.add_argument("--engine")
    args = ap.parse_args()
    engine = find_engine(args.engine)
    probes = [p for p in PROBES if not args.grep or args.grep in p[0]]
    print("# Rhai engine facts (%s)\n" % os.path.basename(engine))
    for q, body, want in probes:
        with tempfile.TemporaryDirectory() as tmp:
            status, got = run_probe(engine, tmp, body)
        print("- %s **%s**\n  got `%s`  (expected: %s)" % (status, q, got[:150], want))
    print("\n%d probes" % len(probes))


if __name__ == "__main__":
    main()
