#!/usr/bin/env python3
"""Assert the Feishu unread classifier buckets each fixture message correctly.

Every assertion here is a runtime one. `compile.py` cannot catch any of it: a
missing Rhai builtin (this build has no `replace`, `replace_all`, `strip` or
array `clone`) is a *runtime* "Function not found", so a plugin that compiles
clean can still fail on the first real message. Routing has to be executed.

The fixture is scripts/rhai/fixtures/feishu_unread.json; each chat carries the
bucket it must land in under `_expect`, and a `_note` saying why. Real payloads
from `lark-cli` shaped the cases - a `post` body that arrives as an HTML
fragment, and the literal "[Invalid text JSON]" the CLI emits for a payload it
could not decode.
"""
import json
import os
import subprocess
import sys

ROOT = os.path.dirname(os.path.dirname(os.path.dirname(os.path.abspath(__file__))))
FIXTURE = os.path.join(ROOT, "scripts", "rhai", "fixtures", "feishu_unread.json")
PLUGIN = os.path.join(ROOT, "websites", "feishu.com", "plugin", "classify.rhai")
COLLECT = os.path.join(ROOT, "websites", "feishu.com", "plugin", "collect.sh")
ENGINE = os.path.join(ROOT, "target", "release", "laya-workflow")

SPEC = {
    "name": "feishu_classify_check", "dsl_version": 2, "start": "p",
    "max_iterations": 1,
    "policy": {"allow_exec": False, "allow_hosts": [],
               "allow_paths": ["${env.HOME}/tmp"],
               "max_timeout_ms": 60000, "max_output": 8388608, "retries": 0},
    "capabilities": {"p": {"kind": "plugin", "dir": PLUGIN, "timeout_ms": 60000,
                           "max_operations": 200000}},
    "nodes": [{"name": "p", "edge": {"condition": {}, "default": "STOP"},
               "action": {"kind": "call", "capability": "p",
                          "with": {"collected": "${state.collected}"},
                          "project": {"ok": "/ok", "total": "/total",
                                      "buckets": "/buckets",
                                      "digest": "/digest"}},
               "primary_q": "done",
               "questions": {"done": {"type": "choice", "instructions": "ok?",
                                      "criteria": {"A": "yes", "B": "no"}}}}],
}


def run(collected):
    import tempfile
    with tempfile.TemporaryDirectory() as tmp:
        sp = os.path.join(tmp, "spec.json")
        json.dump(SPEC, open(sp, "w"))
        # --state takes the JSON inline (workflow_cli.rs parses the string
        # directly; there is no @file form), so it goes through argv, not a
        # shell - a fixture containing quotes and CJK is safe here.
        r = subprocess.run([ENGINE, "run", "--spec", sp, "--state",
                            json.dumps({"collected": collected}, ensure_ascii=False)],
                           capture_output=True, text=True)
    out = r.stdout
    i = out.find("{")
    if i < 0:
        raise RuntimeError("no JSON in engine output: %r / %r" % (out[:200], r.stderr[:200]))
    return json.loads(out[i:])


def main():
    if not os.path.exists(ENGINE):
        print("   feishu: skipped (no %s - build first)" % ENGINE)
        return 0
    # A failed collection must say so. The dangerous outcome is not a crash -
    # it is a digest that reports "you have nothing unread" because the token
    # had expired, and the user reads that as all clear.
    bad = run(json.dumps({"ok": False, "err": "token expired"}))["result"]["result"]
    if bad.get("ok") is not False or "token expired" not in (bad.get("error") or ""):
        print("   feishu: FAIL a failed collection was not reported (got %s)"
              % (bad.get("ok"),))
        return 1
    if bad.get("digest") is not None:
        print("   feishu: FAIL a failed collection still produced a digest")
        return 1

    # collect.sh receives its two knobs as $1/$2. The engine expands an unset
    # ${state.x} through serde_json, and Value::Null.to_string() is the literal
    # "null" - so a spec that leaves page_size out hands the script the string
    # "null" unless the script guards for it. Assert the guard directly: it
    # costs nothing and needs no lark-cli token, unlike running the collector.
    for given, want in (("null", "15/20"), ("", "15/20"), ("20", "20/20"),
                        ("20", "20/50")):
        args = ["--print-knobs", given] + (["50"] if want.endswith("/50") else [given if want == "20/20" else ""])
        guard = subprocess.run(["sh", COLLECT] + args,
                               capture_output=True, text=True)
        if guard.stdout.strip() != want:
            print("   feishu: FAIL collect.sh knobs %r resolved to %r, want %r"
                  % (args[1:], guard.stdout.strip(), want))
            return 1

    fx = json.load(open(FIXTURE))
    res = run(json.dumps(fx, ensure_ascii=False))["result"]["result"]
    if not res.get("ok"):
        print("   feishu: FAIL classifier returned %s" % res.get("error"))
        return 1

    got = {}
    for bucket, items in (res.get("buckets") or {}).items():
        for it in items:
            got[it["message_id"]] = bucket

    fails = []
    for c in fx["chats"]:
        m = c["unread"][0]
        mid, want = m["message_id"], c["_expect"]
        have = got.get(mid)
        if have != want:
            fails.append("   %s (%s): want %s, got %s\n      %s"
                         % (mid, c["_note"], want, have, m["content"][:60]))
    if res.get("total") != len(fx["chats"]):
        fails.append("   total: want %d, got %s" % (len(fx["chats"]), res.get("total")))

    # The digest is capped, so it must be a prefix of the buckets in rank order.
    dig = res.get("digest") or []
    if len(dig) > 5:
        fails.append("   digest has %d entries, cap is 5" % len(dig))
    if dig and dig[0]["bucket"] != "mention":
        fails.append("   digest does not lead with the mention: %s" % dig[0]["bucket"])
    # Every digest line needs a jump-to-message link, or the triage list tells
    # you what needs you but not where to go.
    for x in dig:
        if not x.get("link"):
            fails.append("   digest entry has no link: %s" % (x.get("text", "")[:40]))
            break

    if fails:
        print("   feishu routing: %d FAIL" % len(fails))
        for f in fails:
            print(f)
        return 1
    print("   ok: feishu unread routing - %d messages into %d buckets, digest leads with the mention,"
          " failed collection refused, unset knobs default"
          % (len(fx["chats"]), len([b for b in (res.get("buckets") or {}).values() if b])))
    return 0


if __name__ == "__main__":
    sys.exit(main())
