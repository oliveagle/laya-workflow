#!/usr/bin/env python3
"""Run the BDD documents in bdd/features/ against real Chrome over CDP.

    python3 scripts/bdd/run.py                    # everything
    python3 scripts/bdd/run.py --filter outline   # one feature, by substring
    python3 scripts/bdd/run.py --keep             # leave the generated specs behind

For each Scenario this:

  1. serves bdd/fixtures on a free localhost port (hermetic: no internet),
  2. brings the CDP backend up (`laya-workflow browser ensure --backend chrome`),
  3. compiles the scenario to a spec with scripts/bdd/transpile.py,
  4. runs it: `laya-workflow run --spec <spec> --state <initial state>`.

A scenario is green when the run exits 0, which means every CDP operation
succeeded and every assertion held. A red scenario is red because the page did
not do what the document says - not because a model thought otherwise.

Scenarios run serially by default. `--jobs N` runs N at once, each with its own
Chrome; it works, but it is not the default because it is not *validated* as
faster - see the note in bdd/README.md.

Whatever happens - a failing scenario, a Ctrl-C, a crash - the runner kills the
Chrome processes holding its own profile dirs and removes them. This is not
cosmetic: the runner used to leak them, nine survivors were found holding
renderers, and the machine's load average was 58 with them and 31 without.

Scenarios tagged `@expected_failure` must fail. They are the guard against
vacuous assertions: if a broken page ever made them pass, the suite reports it,
because a check that cannot fail is not a check.
"""

from __future__ import annotations

import argparse
import contextlib
import functools
import http.server
import itertools
import signal
import time
import json
import os
import shutil
import socket
import subprocess
import sys
import tempfile
import threading

HERE = os.path.dirname(os.path.abspath(__file__))
ROOT = os.path.abspath(os.path.join(HERE, "..", ".."))
sys.path.insert(0, HERE)

import gherkin  # noqa: E402
import transpile  # noqa: E402

FEATURE_DIR = os.path.join(ROOT, "bdd", "features")
FIXTURE_DIR = os.path.join(ROOT, "bdd", "fixtures")
BUILD_DIR = os.path.join(ROOT, "target", "bdd-specs")
# Hands the real Chrome a --headless=new prefix. Resolved the same way the
# engine resolves it, so `chrome_binary` does not become a way to silently test
# a different browser than the one a developer would get by hand.
CHROME_WRAPPER = os.path.join(HERE, "chrome-headless.sh")

XFAIL_TAG = "expected_failure"

# A hand-written spec that drives the same plugin, with no Gherkin and no
# transpiler anywhere in it. Running it is what makes "the vocabulary is a
# standard plugin, not a transpiler accessory" a checked fact rather than a
# claim in a README.
HANDWRITTEN_SPEC = os.path.join(ROOT, "dsl", "browser", "bdd_assert_probe.json")

# A hand-written spec that is required to FAIL, and to name the thing it could
# not close. It exists because a green test proves a feature works, and says
# nothing about whether the feature can fail - and `bdd.release` used to return
# `released: true` for a target that never existed, so it could not. The
# Gherkin vocabulary cannot reach this state on purpose: `I release the page`
# clears the compiler's has-no-page flag, so a second release is a compile
# error rather than a run. Which is exactly the argument for having at least
# one spec written by hand.
RELEASE_PROBE = os.path.join(ROOT, "dsl", "browser", "bdd_release_probe.json")
# The message has to carry the target, or "no open target" is a shrug: the
# reader has no way to tell a typo'd id from a page that was already gone.
RELEASE_PROBE_MUST_CONTAIN = "no open target"

# A hand-written spec that waits for an element that never appears, required to
# FAIL *and* to say what it waited for. `bdd.wait_for` has two claims no other
# test could reach: that its budget is milliseconds on both sides of the
# comparison, and that it does not under-report how long it ran. Both were
# false - the budget was in seconds against a millisecond counter, and the
# elapsed time was a hardcoded `waited += 100` that ignored both the sleep and
# the probe round trip.
WAIT_PROBE = os.path.join(ROOT, "dsl", "browser", "bdd_wait_probe.json")
# A budget this small is what makes the probe cheap: the default is 15000ms, so
# a spec that relied on it would add 15s to every run. It is also what proves
# `with.timeout_ms` arrives - see RETRY_PROBE for the sibling case.
WAIT_PROBE_BUDGET_MS = 600
WAIT_PROBE_MUST_CONTAIN = (
    f"timed out after {WAIT_PROBE_BUDGET_MS}ms",
    "#never-going-to-appear",
    "ms elapsed",
)

# A hand-written spec that must SUCCEED, and only does if `with.attempts`
# reaches the plugin. Its control - that one attempt loses the same race - is
# the @expected_failure scenario at the end of bdd/features/assertions.feature.
# Two specs, because one proves nothing on its own.
RETRY_PROBE = os.path.join(ROOT, "dsl", "browser", "bdd_retry_probe.json")
RETRY_PROBE_ATTEMPTS = 8


def free_port() -> int:
    with contextlib.closing(socket.socket()) as s:
        s.bind(("127.0.0.1", 0))
        return s.getsockname()[1]


@contextlib.contextmanager
def fixture_server(directory: str):
    """Serve `directory` on a free localhost port for the duration of the block."""
    handler = functools.partial(http.server.SimpleHTTPRequestHandler, directory=directory)
    handler.log_message = lambda *a, **k: None  # keep the test output readable
    httpd = http.server.ThreadingHTTPServer(("127.0.0.1", 0), handler)
    port = httpd.server_address[1]
    t = threading.Thread(target=httpd.serve_forever, daemon=True)
    t.start()
    try:
        yield f"http://127.0.0.1:{port}"
    finally:
        httpd.shutdown()
        httpd.server_close()


def pick_binary(explicit: str | None) -> str:
    if explicit:
        return explicit
    for rel in ("target/release/laya-workflow", "target/debug/laya-workflow"):
        p = os.path.join(ROOT, rel)
        if os.path.isfile(p) and os.access(p, os.X_OK):
            return p
    raise SystemExit("no laya-workflow binary; run `cargo build --release --bins -p laya-workflow`")


def run_scenario(binary: str, spec_path: str, state: dict, timeout: int) -> tuple[int, str]:
    cmd = [binary, "run", "--spec", spec_path, "--state", json.dumps(state)]
    try:
        proc = subprocess.run(cmd, capture_output=True, text=True, timeout=timeout)
    except subprocess.TimeoutExpired:
        return 124, f"timed out after {timeout}s"
    return proc.returncode, (proc.stdout + proc.stderr)


def reap_chrome(profiles: list[str]) -> int:
    """Kill Chrome processes still holding one of *our* profile dirs.

    Each `laya-workflow run` launches the headless Chrome its spec asks for, and
    that Chrome normally goes away with the run. It does not always: an
    interrupted run, a crash in this script, or a scenario that times out all
    leave one behind. Nine survivors were found on this machine, each holding a
    renderer and a slice of CPU - which then makes every later run slower, and
    makes timing measurements look worse than the code deserves.

    Matching on the profile dir is precise rather than broad: these are temp
    dirs this process just created, so nothing else can be named by them. No
    `pkill chrome`, which would take the developer's own browser with it.
    """
    killed = 0
    for profile in profiles:
        with contextlib.suppress(OSError, subprocess.SubprocessError):
            found = subprocess.run(["pgrep", "-f", profile], capture_output=True, text=True)
            for pid in found.stdout.split():
                with contextlib.suppress(OSError, ProcessLookupError, ValueError):
                    os.kill(int(pid), signal.SIGTERM)
                    killed += 1
    return killed


def dispose_profiles(profiles: list[str], attempts: int = 5) -> int:
    """Remove the profile dirs, retrying while Chrome is still letting go.

    SIGTERM is asynchronous, and a directory Chrome still has open will fail to
    be removed. `rmtree(ignore_errors=True)` alone leaves the dir behind and
    says nothing, which is how 47 of them accumulated unnoticed.
    """
    left = 0
    for profile in profiles:
        for _ in range(attempts):
            if not os.path.isdir(profile):
                break
            shutil.rmtree(profile, ignore_errors=True)
            if not os.path.isdir(profile):
                break
            time.sleep(0.2)
        if os.path.isdir(profile):
            left += 1
            print(f"bdd: warning: could not remove {profile}", file=sys.stderr)
    return left


def main(argv: list[str] | None = None) -> int:
    ap = argparse.ArgumentParser(description=__doc__.split("\n")[0])
    ap.add_argument("features", nargs="*", help="feature files (default: all of bdd/features)")
    ap.add_argument("--filter", help="only features whose path contains this substring")
    ap.add_argument("--bin", help="path to the laya-workflow binary")
    ap.add_argument("--port", type=int, default=0,
                    help="CDP port (0 = pick a free one; only set this with --jobs 1)")
    ap.add_argument("--jobs", "-j", type=int, default=1,
                    help="scenarios to run at once (default 1: parallel was measured "
                         "and lost here, see --help notes in this file)")
    ap.add_argument("--timeout", type=int, default=180, help="per-scenario timeout, seconds")
    ap.add_argument("--keep", action="store_true", help="keep the generated specs")
    ap.add_argument("--out", default=BUILD_DIR, help="where generated specs are written")
    args = ap.parse_args(argv)

    paths = args.features or sorted(
        os.path.join(FEATURE_DIR, f) for f in os.listdir(FEATURE_DIR)
        # Non-recursive on purpose. bdd/features/setup/ holds step lists pulled
        # in with `include:`, and one of those is a flat list of Given/When/Then
        # with no `Feature:` header - so a walk here would try to compile it as
        # a feature and fail with "no Feature: header found", which reads like
        # a broken feature rather than a directory that is not one.
        if f.endswith(".feature") and os.path.isfile(os.path.join(FEATURE_DIR, f))
    )
    if args.filter:
        paths = [p for p in paths if args.filter in p]
    if not paths:
        print("no feature files selected", file=sys.stderr)
        return 1

    # Compile everything first: a broken document should cost zero browser time.
    plan: list[tuple[gherkin.Feature, gherkin.Scenario, dict, dict]] = []
    for path in paths:
        try:
            feature = gherkin.load(path)
        except gherkin.GherkinError as e:
            print(f"error: {path}: {e}", file=sys.stderr)
            return 1
        config = transpile.load_config(path)
        for scenario in feature.scenarios:
            try:
                spec = transpile.compile_scenario(feature, scenario, config, CHROME_WRAPPER)
            except Exception as e:  # noqa: BLE001 - surface the step error verbatim
                print(f"error: {os.path.basename(path)} :: {scenario.name}: {e}", file=sys.stderr)
                return 1
            plan.append((feature, scenario, spec, config))

    binary = pick_binary(args.bin)
    os.makedirs(args.out, exist_ok=True)

    jobs = max(1, min(args.jobs, len(plan)))
    if args.port and jobs != 1:
        print("error: --port pins one CDP endpoint, so it only works with --jobs 1",
              file=sys.stderr)
        return 1

    print(f"bdd: {len(plan)} scenario(s) from {len(paths)} feature file(s)")
    print(f"bdd: binary   {os.path.relpath(binary, ROOT)}")
    print(f"bdd: jobs     {jobs}")

    # One CDP endpoint per worker. Each worker keeps its own port and profile
    # for every scenario it runs, which is what lets scenario 2..N adopt the
    # Chrome that scenario 1 launched instead of paying for a fresh launch:
    # measured 1.9s per scenario with a fresh profile, 0.75s with a reused one.
    # A private profile dir per *worker* is still required: a shared fixed one
    # would collide with the interactive Chrome's lock and owner marker.
    endpoints: list[tuple[int, str]] = []
    for w in range(jobs):
        endpoints.append((args.port or free_port(),
                          tempfile.mkdtemp(prefix=f"laya-bdd-chrome-w{w}-")))

    # No `browser ensure` here on purpose. Each `laya-workflow run` launches the
    # headless Chrome its spec asks for, so the runner never opens a window on
    # the developer's screen.
    #
    # The finally matters more than it looks: the browser work is what leaks
    # (a killed run leaves a Chrome behind), and leaking it makes every later
    # run slower, so cleanup cannot depend on reaching the end of the script.
    try:
        with fixture_server(FIXTURE_DIR) as base_url:
            print(f"bdd: fixtures {base_url}  ({os.path.relpath(FIXTURE_DIR, ROOT)})")
            for w, (port, prof) in enumerate(endpoints):
                print(f"bdd: cdp[{w}]   127.0.0.1:{port} headless  ({prof})")
            print("bdd: headless chrome\n")

            # Scenarios are independent: their own process, their own tab, their
            # own state. So run them across a small pool of CDP endpoints, each of
            # which reuses one Chrome for every scenario it handles. Measured on
            # this machine: serial 7.4s for 18 scenarios, because ~0.4s of every
            # scenario is Chrome/CDP, not page work (a validate run costs 3ms).
            #
            # The report is printed in plan order, not completion order, so the
            # output is identical run to run and a diff between two runs means
            # something changed.
            results: list[tuple[str, bool, str, int, str, bool, float]] = []
            lock = threading.Lock()
            counter = itertools.count()

            def one(index: int, worker: int) -> None:
                feature, scenario, spec, config = plan[index]
                port, profile = endpoints[worker]
                xfail = any(t == XFAIL_TAG or t.startswith(XFAIL_TAG + "(")
                            for t in (scenario.tags or []))
                spec_path = os.path.join(args.out, spec["name"] + ".json")
                with open(spec_path, "w", encoding="utf-8") as fh:
                    fh.write(json.dumps(transpile.public_spec(spec), indent=2, ensure_ascii=False))

                state = {
                    "base_url": base_url,
                    "cdp_port": port,
                    "cdp_profile": profile,
                    **(config.get("initial_state") or {}),
                }
                started = time.monotonic()
                rc, out = run_scenario(binary, spec_path, state, args.timeout)
                elapsed = time.monotonic() - started
                name = f"{os.path.basename(feature.path)} :: {scenario.name}"
                if xfail:
                    # Failing is not enough - the run has to fail *for the
                    # stated reason*. Any non-zero exit used to count, so a
                    # scenario that died on a typo'd Given, a Chrome that would
                    # not start, or a 404 from the fixture was reported as a
                    # passing demonstration of the thing it was written to
                    # demonstrate. The reason lives in the tag:
                    #   @expected_failure(bdd.assert: FAIL equals)
                    want = gherkin.tag_reason(scenario, XFAIL_TAG)
                    if rc == 0:
                        ok, why = False, "the scenario passed, so the check it " \
                                         "was written to disprove no longer fails"
                    elif not want:
                        ok, why = False, "the tag declares no reason, so any " \
                                         "failure at all would count"
                    elif want not in out:
                        ok, why = False, f"it failed, but never said {want!r}, " \
                                         "so it failed for some other reason"
                    else:
                        ok, why = True, ""
                    label = "xfail" if ok else "XFAIL-WRONG-REASON"
                    if not ok:
                        print(f"  {name}: {why}", file=sys.stderr, flush=True)
                else:
                    ok = rc == 0
                    label = "PASS" if ok else "FAIL"
                with lock:
                    done = next(counter)
                    results.append((label, ok, name, rc, out, xfail, elapsed))
                    # Progress only: the verdict table is printed at the end, in
                    # plan order. Interleaved lines from N threads are unreadable.
                    print(f"  ... {label:14s} {name}  [{rc}]  ({done + 1}/{len(plan)})",
                          flush=True)

            if jobs == 1:
                for i in range(len(plan)):
                    one(i, 0)
            else:
                threads = [
                    threading.Thread(target=one, args=(i, i % jobs), daemon=True)
                    for i in range(len(plan))
                ]
                for t in threads:
                    t.start()
                for t in threads:
                    t.join()

                # The hand-written spec runs on the same endpoint and the same fixture,
            # so it costs one scenario's worth of Chrome and proves the plugin is
            # usable without the transpiler.
            if os.path.isfile(HANDWRITTEN_SPEC):
                started = time.monotonic()
                rc, out = run_scenario(
                    binary, HANDWRITTEN_SPEC,
                    {"url": f"{base_url}/index.html",
                     "cdp_port": endpoints[0][0],
                     "cdp_profile": endpoints[0][1]},
                    args.timeout,
                )
                elapsed = time.monotonic() - started
                ok = rc == 0
                results.append(("PASS" if ok else "FAIL", ok,
                                "hand-written dsl/browser/bdd_assert_probe.json "
                                "(no transpiler)",
                                rc, out, False, elapsed))

            # The opposite shape: this spec must NOT pass. Two exit codes mean
            # "the assertion did not hold" here, so `rc == 0` is the failure
            # being reported as a pass - the exact inversion a green-only
            # suite cannot express.
            if os.path.isfile(RELEASE_PROBE):
                started = time.monotonic()
                rc, out = run_scenario(
                    binary, RELEASE_PROBE,
                    {"url": f"{base_url}/index.html",
                     "cdp_port": endpoints[0][0],
                     "cdp_profile": endpoints[0][1]},
                    args.timeout,
                )
                elapsed = time.monotonic() - started
                names_target = RELEASE_PROBE_MUST_CONTAIN in out
                ok = rc != 0 and names_target
                results.append(("PASS" if ok else "FAIL", ok,
                                "hand-written dsl/browser/bdd_release_probe.json "
                                "(must refuse to release twice)",
                                rc, out, not ok, elapsed))
                if rc == 0:
                    detail = ("the second release was reported as success - "
                              "bdd.release is claiming it closed a page that "
                              "was already closed")
                elif not names_target:
                    detail = (f"the run failed but never said {RELEASE_PROBE_MUST_CONTAIN!r}, "
                              "so the reader cannot tell a typo'd target from an "
                              "already-closed one")
                else:
                    detail = ""
                if detail:
                    print(f"  bdd_release_probe: {detail}", file=sys.stderr)
                    for line in extract_failure(out).splitlines():
                        print(f"                 {line}", file=sys.stderr)

            for probe, required_rc_zero, must_contain, blurb in (
                (WAIT_PROBE, False, WAIT_PROBE_MUST_CONTAIN,
                 "must time out and say what it waited for"),
                (RETRY_PROBE, True, (),
                 "must win a race it cannot win in one attempt"),
            ):
                if not os.path.isfile(probe):
                    continue
                started = time.monotonic()
                rc, out = run_scenario(
                    binary, probe,
                    {"url": f"{base_url}/index.html",
                     "cdp_port": endpoints[0][0],
                     "cdp_profile": endpoints[0][1]},
                    args.timeout,
                )
                elapsed = time.monotonic() - started
                problems: list[str] = []
                if required_rc_zero and rc != 0:
                    problems.append("the spec failed, but a working "
                                    f"{os.path.basename(probe)} must succeed")
                if not required_rc_zero and rc == 0:
                    problems.append("the spec succeeded, but it exists because "
                                    "the op is supposed to refuse")
                if not required_rc_zero:
                    missing = [t for t in must_contain if t not in out]
                    if missing:
                        problems.append(
                            "the run failed without saying "
                            + ", ".join(repr(m) for m in missing)
                            + " - a reader cannot tell a mistyped selector from "
                              "a budget that was never applied")
                results.append(("PASS" if not problems else "FAIL", not problems,
                                f"hand-written {os.path.relpath(probe, ROOT)} ({blurb})",
                                rc, out, bool(problems), elapsed))
                for problem in problems:
                    print(f"  {os.path.basename(probe)}: {problem}", file=sys.stderr)
                    for line in extract_failure(out).splitlines():
                        print(f"                 {line}", file=sys.stderr)

            results.sort(key=lambda r: r[2])
            passed = sum(1 for r in results if r[1])
            failures: list[tuple[str, str]] = []
            print()
            for label, ok, name, rc, out, xfail, elapsed in results:
                print(f"  {label:14s} {name}  [{rc}]  {elapsed:.2f}s")
                if not ok or xfail:
                    # Show the assertion, not the whole run log: the useful part is
                    # the deterministic FAIL the plugin threw.
                    detail = extract_failure(out)
                    if detail:
                        for line in detail.splitlines():
                            print(f"                 {line}")
                if not ok:
                    failures.append((name, out))

            failed = len(failures)
            slowest = sorted(results, key=lambda r: -r[6])[:3]
            print("\nbdd: slowest scenarios (Chrome/CDP, not page work):")
            for _, _, name, _, _, _, elapsed in slowest:
                print(f"  {elapsed:5.2f}s  {name}")
            print(f"\nbdd: {passed} passed, {failed} failed")
    finally:
        reap_chrome([prof for _, prof in endpoints])
        dispose_profiles([prof for _, prof in endpoints])

    if failures:
        print("\n--- first failure, full output ---")
        name, out = failures[0]
        print(f"{name}\n{out}")
    if not args.keep:
        for f in os.listdir(args.out):
            with contextlib.suppress(OSError):
                os.remove(os.path.join(args.out, f))
        with contextlib.suppress(OSError):
            os.rmdir(args.out)

    print("\nall green" if failed == 0 else "\nRED")
    return 1 if failed else 0


def extract_failure(output: str) -> str:
    """The deterministic assert/cdp error lines, without the run's noise."""
    keep = []
    for line in output.splitlines():
        low = line.lower()
        if ("fail" in low or "error" in low or "not found" in low
                or "mismatch" in low or "timed out" in low):
            keep.append(line.strip())
        if len(keep) >= 6:
            break
    return "\n".join(keep)


if __name__ == "__main__":
    sys.exit(main())
