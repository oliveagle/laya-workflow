#!/usr/bin/env python3
"""Build every artifact a BDD document drives, with accuracy as the hard gate.

One Gherkin `.feature` is the single source of truth. This is the one command
that turns it into everything downstream:

    spec/        the workflow specs (laya-workflow JSON) — the *workflow* artifact
    manifest.json  what each scenario became, per profile — feeds run.py and CI
    it.manifest.json (production profile only) — the *production integration test*
                 plan: @production-tagged scenarios, external base_url, no fixtures
    coverage.json  per-feature step-coverage report — the *accuracy ledger*

Accuracy is the gate, not a report:
  * every step the deterministic vocabulary cannot compile is a HARD ERROR
    (strict, on by default) — a silently-skipped step is how a document stops
    meaning what it says, and that is precisely the failure mode the previous
    research (docs/bdd_to_needle.md) ruled out as a needle-free path
  * every generated spec must pass `laya-workflow validate` — not just parse,
    the real engine's structural gate
  * per-feature step coverage must reach --coverage-min (default 100%)
  * out-of-vocabulary steps are *reported*, and with --assist a needle
    suggestion (with confidence) is printed for the agent to act on — a
    suggestion is never compiled

Profiles:
    local      hermetic: fixture server + headless Chrome (default)
    production real target: --base-url / $BDD_BASE_URL, no fixtures, only
               @production-tagged scenarios (--all to override)

    python3 scripts/bdd/build.py                      # local, all features
    python3 scripts/bdd/build.py --profile production --base-url https://app.example.com
    python3 scripts/bdd/build.py --assist             # + needle suggestions

Exit codes: 0 = everything compiled, validated, covered; 1 = any gate failed.
"""
from __future__ import annotations

import argparse
import json
import os
import subprocess
import sys

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))

import gherkin  # noqa: E402
import steps as stepdefs  # noqa: E402
import transpile  # noqa: E402
from needle_assist import suggest, ACCEPT_FLOOR  # noqa: E402

HERE = os.path.dirname(os.path.abspath(__file__))
ROOT = os.path.abspath(os.path.join(HERE, "..", ".."))
FEATURE_DIR = os.path.join(ROOT, "bdd", "features")
CHROME_WRAPPER = os.path.join(HERE, "chrome-headless.sh")

PROD_TAG = "production"   # scenario tag that selects a scenario for prod IT


class _FakeStep:
    def __init__(self, line: int):
        self.line = line


def pick_binary(explicit: str | None) -> str:
    if explicit:
        return os.path.abspath(explicit)
    for rel in ("target/release/laya-workflow", "target/debug/laya-workflow"):
        p = os.path.join(ROOT, rel)
        if os.path.isfile(p) and os.access(p, os.X_OK):
            return p
    return "laya-workflow"  # on PATH


def feature_coverage(feature: gherkin.Feature) -> dict:
    """Count vocabulary-matched vs total steps in a feature (background included).

    Uses the same `_match` the compiler uses, so "covered" means "the
    deterministic compiler knows this step" — the accuracy-first bar.
    """
    matched, total = 0, 0
    unmatched: list[str] = []
    steps = list(feature.background) + [s for sc in feature.scenarios for s in sc.steps]
    for st in steps:
        total += 1
        try:
            stepdefs._match(st.kind, st.text, _FakeStep(st.line))
            matched += 1
        except stepdefs.UnknownStep:
            unmatched.append(f"{st.kind} {st.text}")
    return {"total": total, "matched": matched,
            "coverage": round(100.0 * matched / total, 1) if total else 100.0,
            "unmatched": unmatched}


def validate_spec(bin: str, spec_path: str) -> tuple[bool, str]:
    p = subprocess.run([bin, "validate", "--spec", spec_path],
                       capture_output=True, text=True, timeout=60)
    return p.returncode == 0, (p.stdout + p.stderr).strip()[-600:]


def has_tag(tags: list[str], name: str) -> bool:
    return any(t == name or t.startswith(name + "(") for t in (tags or []))


def main(argv: list[str] | None = None) -> int:
    ap = argparse.ArgumentParser(description=__doc__.split("\n")[0])
    ap.add_argument("features", nargs="*", help="feature files (default: all of bdd/features)")
    ap.add_argument("--out", default=os.path.join(ROOT, "target", "bdd-build"),
                    help="output dir for generated artifacts")
    ap.add_argument("--profile", choices=["local", "production"], default="local",
                    help="local = hermetic fixtures; production = real target (--base-url)")
    ap.add_argument("--filter", help="keep only scenarios with this tag (e.g. @production)")
    ap.add_argument("--all", action="store_true",
                    help="(production) run every scenario, not just @production-tagged ones")
    ap.add_argument("--bin", help="path to the laya-workflow binary used for `validate`")
    ap.add_argument("--emit", choices=["all", "spec", "manifest", "coverage"], default="all")
    ap.add_argument("--coverage-min", type=float, default=100.0,
                    help="minimum per-feature step coverage %% (default 100 = accuracy-first)")
    ap.add_argument("--no-validate", action="store_true",
                    help="skip the `laya-workflow validate` gate (for docs/demo only)")
    ap.add_argument("--assist", action="store_true",
                    help="for out-of-vocabulary steps, print a needle suggestion + confidence")
    ap.add_argument("--base-url", help="production base URL (default $BDD_BASE_URL)")
    args = ap.parse_args(argv)

    bin_ = pick_binary(args.bin)
    if not args.no_validate and not os.path.isfile(bin_):
        print(f"build: no laya-workflow binary at {bin_} (--bin to point at one)", file=sys.stderr)
        return 1

    paths = args.features or sorted(
        os.path.join(FEATURE_DIR, f) for f in os.listdir(FEATURE_DIR)
        if f.endswith(".feature") and os.path.isfile(os.path.join(FEATURE_DIR, f))
    )
    if not paths:
        print("build: no feature files selected", file=sys.stderr)
        return 1

    # Production requires an external base_url: this is the entire point of the
    # profile — the scenario must run against a real target, never a fixture.
    profile = args.profile
    base_url = args.base_url or os.environ.get("BDD_BASE_URL") if profile == "production" else None
    if profile == "production" and not base_url:
        print("build: --profile production needs --base-url (or $BDD_BASE_URL) — "
              "production integration tests run against a real target", file=sys.stderr)
        return 1

    spec_dir = os.path.join(args.out, "spec")
    os.makedirs(spec_dir, exist_ok=True)

    manifest: list[dict] = []
    coverage_report: list[dict] = []
    errors: list[str] = []
    assist_calls: list[tuple[str, str]] = []

    for path in paths:
        try:
            feature = gherkin.load(path)
        except gherkin.GherkinError as e:
            errors.append(f"{os.path.relpath(path, ROOT)}: {e}")
            continue
        cov = feature_coverage(feature)
        coverage_report.append({
            "feature": os.path.relpath(path, ROOT),
            **{k: cov[k] for k in ("total", "matched", "coverage")},
            "unmatched": cov["unmatched"],
        })

        config = transpile.load_config(path)
        for scenario in feature.scenarios:
            tags = scenario.tags or []
            # Production selects @production scenarios; a --filter selects tags.
            if profile == "production" and not args.all and not has_tag(tags, PROD_TAG):
                continue
            if args.filter:
                fname = args.filter[1:] if args.filter.startswith("@") else args.filter
                if not has_tag(tags, fname):
                    continue
            try:
                spec = transpile.compile_scenario(feature, scenario, config, CHROME_WRAPPER)
            except (stepdefs.UnknownStep, stepdefs.StepError, transpile.TranspileError,
                    json.JSONDecodeError) as e:
                errors.append(f"{os.path.relpath(path, ROOT)} :: {scenario.name}: {e}")
                continue

            spec_file = spec["name"] + ".json"
            spec_path = os.path.join(spec_dir, spec_file)
            with open(spec_path, "w", encoding="utf-8") as fh:
                fh.write(json.dumps(transpile.public_spec(spec), indent=2, ensure_ascii=False) + "\n")

            validated, vmsg = (True, "") if args.no_validate else \
                validate_spec(bin_, spec_path)
            if not validated:
                errors.append(f"{os.path.relpath(path, ROOT)} :: {scenario.name}: "
                              f"`validate` rejected the generated spec: {vmsg}")
            manifest.append({
                "feature": os.path.relpath(path, ROOT),
                "scenario": scenario.name,
                "spec": f"spec/{spec_file}",
                "tags": tags,
                "profile": profile,
                "base_url": base_url,
                "nodes": len(spec["nodes"]),
                "validated": validated,
            })

    # Per-feature coverage gate (strict = 100% by default).
    for cov in coverage_report:
        if cov["coverage"] < args.coverage_min:
            errors.append(f"{cov['feature']}: step coverage {cov['coverage']}% < "
                          f"{args.coverage_min}% (unmatched: {', '.join(cov['unmatched'])[:200]})")

    # Needle assist for out-of-vocabulary steps — suggestions only.
    if args.assist:
        for cov in coverage_report:
            for step in cov["unmatched"]:
                assist_calls.append((cov["feature"], step))
        for feature, step in assist_calls:
            s = suggest(step, bin_)
            if not s:
                print(f"  assist: {feature}: {step!r} -> (needle unavailable)", file=sys.stderr)
                continue
            label = "LIKELY" if s["confidence"] >= ACCEPT_FLOOR else "guess"
            core = {k: s[k] for k in ("op", "assertion", "value", "expected") if s.get(k)}
            print(f"  assist [{label} conf={s['confidence']:.2f}] {feature}: {step!r}")
            print(f"        -> {json.dumps(core, ensure_ascii=False)}  "
                  "(suggestion only — add a rule to scripts/bdd/steps.py to compile it)")

    # Emit the manifests.
    if args.emit in ("all", "manifest"):
        with open(os.path.join(args.out, "manifest.json"), "w", encoding="utf-8") as fh:
            json.dump({"profile": profile, "base_url": base_url, "scenarios": manifest},
                      fh, indent=2, ensure_ascii=False)
    if args.emit in ("all", "coverage"):
        with open(os.path.join(args.out, "coverage.json"), "w", encoding="utf-8") as fh:
            json.dump(coverage_report, fh, indent=2, ensure_ascii=False)
    if profile == "production":
        with open(os.path.join(args.out, "it.manifest.json"), "w", encoding="utf-8") as fh:
            json.dump({"profile": "production", "base_url": base_url, "scenarios": manifest},
                      fh, indent=2, ensure_ascii=False)

    # An empty selection is an error, not a success: a typo in the --filter tag
    # or a `@production` tag nobody wrote yet must not silently produce a
    # "0 production integration tests" plan that exits 0. This is the same
    # failure shape as "all tests pass because none ran", and it has to be
    # louder than a green line.
    if not manifest and not errors:  # compile errors already explain an empty plan
        if profile == "production" and not args.all:
            errors.append("no @production scenarios selected — tag some `Scenario:` "
                          f"blocks with @{PROD_TAG}, or pass --all to override")
        elif args.filter:
            errors.append(f"--filter {args.filter}: no scenario carries that tag")
        elif args.features:
            errors.append("the selected feature file(s) contain no scenario")
        else:
            errors.append("no scenario was selected at all")
    # Report.
    n_spec = len(manifest)
    n_prod = sum(1 for m in manifest if m["profile"] == "production")
    for cov in coverage_report:
        tag = "ok" if cov["coverage"] >= args.coverage_min else "LOW"
        print(f"  coverage {cov['coverage']:5.1f}%  {cov['matched']}/{cov['total']}  {tag:3s}  "
              f"{cov['feature']}"
              + (f"   unmatched: {'; '.join(cov['unmatched'])[:120]}" if cov["unmatched"] else ""))
    print(f"  profile {profile}  base_url {base_url or '(local fixtures)'}")
    print(f"  specs {n_spec}  validated {sum(1 for m in manifest if m['validated'])}"
          + (f"  production-IT {n_prod}" if profile == "production" else "")
          + f"  assist-suggestions {len(assist_calls)}")
    if errors:
        print("\nerrors:", file=sys.stderr)
        for e in errors:
            print(f"  {e}", file=sys.stderr)
        return 1
    print("  OK — every scenario compiled, every spec validated, coverage at the bar")
    return 0


if __name__ == "__main__":
    sys.exit(main())
