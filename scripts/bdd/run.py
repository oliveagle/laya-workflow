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
succeeded and every `checks` map matched. A red scenario is red because the page
did not do what the document says - not because a model thought otherwise.

Scenarios tagged `@expected_failure` must fail. They are the guard against
vacuous assertions: if a broken page ever made them pass, the suite reports it,
because a check that cannot fail is not a check.
"""

from __future__ import annotations

import argparse
import contextlib
import functools
import http.server
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


def main(argv: list[str] | None = None) -> int:
    ap = argparse.ArgumentParser(description=__doc__.split("\n")[0])
    ap.add_argument("features", nargs="*", help="feature files (default: all of bdd/features)")
    ap.add_argument("--filter", help="only features whose path contains this substring")
    ap.add_argument("--bin", help="path to the laya-workflow binary")
    ap.add_argument("--port", type=int, default=0, help="fixture port (0 = pick a free one)")
    ap.add_argument("--timeout", type=int, default=180, help="per-scenario timeout, seconds")
    ap.add_argument("--keep", action="store_true", help="keep the generated specs")
    ap.add_argument("--out", default=BUILD_DIR, help="where generated specs are written")
    args = ap.parse_args(argv)

    paths = args.features or sorted(
        os.path.join(FEATURE_DIR, f) for f in os.listdir(FEATURE_DIR) if f.endswith(".feature")
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

    cdp_port = args.port or free_port()
    # A private profile dir per run. Reusing a fixed one would collide with the
    # interactive Chrome's lock and owner marker; reusing it *within* a run is
    # what lets scenarios 2..N adopt the Chrome scenario 1 launched.
    cdp_profile = tempfile.mkdtemp(prefix="laya-bdd-chrome-")
    print(f"bdd: {len(plan)} scenario(s) from {len(paths)} feature file(s)")
    print(f"bdd: binary   {os.path.relpath(binary, ROOT)}")
    print(f"bdd: fixtures http://127.0.0.1:{cdp_port}  ({os.path.relpath(FIXTURE_DIR, ROOT)})")
    print(f"bdd: cdp      127.0.0.1:{cdp_port} headless  ({cdp_profile})")

    # No `browser ensure` here on purpose. Each `laya-workflow run` launches the
    # headless Chrome its spec asks for and tears it down when it exits, so the
    # runner never opens a window on the developer's screen.
    with fixture_server(FIXTURE_DIR) as base_url:
        print("bdd: headless chrome per scenario\n")

        passed = failed = 0
        failures: list[tuple[str, str]] = []

        for feature, scenario, spec, config in plan:
            meta = spec["_bdd"]
            xfail = XFAIL_TAG in (scenario.tags or [])
            spec_path = os.path.join(args.out, spec["name"] + ".json")
            with open(spec_path, "w", encoding="utf-8") as fh:
                fh.write(json.dumps(transpile.public_spec(spec), indent=2, ensure_ascii=False))

            state = {
                "base_url": base_url,
                "cdp_port": cdp_port,
                "cdp_profile": cdp_profile,
                **(config.get("initial_state") or {}),
            }
            rc, out = run_scenario(binary, spec_path, state, args.timeout)

            if xfail:
                ok = rc != 0
                label = "xfail" if ok else "XFAIL-BROKEN"
            else:
                ok = rc == 0
                label = "PASS" if ok else "FAIL"

            name = f"{os.path.basename(feature.path)} :: {scenario.name}"
            print(f"  {label:14s} {name}  [{rc}]")
            if not ok or xfail:
                # Show the assertion, not the whole run log: the useful part is
                # the deterministic FAIL the plugin threw.
                detail = extract_failure(out)
                if detail:
                    for line in detail.splitlines():
                        print(f"                 {line}")
            if ok and not xfail:
                passed += 1
            elif xfail and ok:
                passed += 1
            else:
                failed += 1
                failures.append((name, out))

        print(f"\nbdd: {passed} passed, {failed} failed")

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
        shutil.rmtree(cdp_profile, ignore_errors=True)

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
