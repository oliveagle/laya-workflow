#!/usr/bin/env python3
"""Keep the numbers bdd/README.md quotes honest.

The README quotes its own gates: the scenario count in the "Current state"
line, and the output of vocabulary_check.py and args_probe_check.py. Those
quotes drift silently, because nothing recomputed them. Measured on the first
run: the headline said **5 scenarios** when bdd/features/ transpiles to 22,
and the quoted `bdd vocabulary:` block stopped four clauses short of what the
checker actually prints - it had been truncated by an edit that never came back
to update the quote.

A quote that nobody recomputes is decoration. So every count the README attaches
to a label is matched against the same label in the live output of the thing
that owns it:

  * `N scenarios`            <- how many specs bdd/features/ actually transpiles to
  * `N steps map correctly`  <- vocabulary_check.py's stdout
  * ... and the rest, by the same rule

Matching by *label* rather than by position is what makes truncation survivable:
the README may quote half a line, but every number in it still has to be the
number the checker prints. A label that appears in the README and no longer
appears in the output is stale, and so is one that appears in the output and
never made it into the README.

Needs no Chrome and no network, so it runs in check.sh and therefore in CI.
"""

from __future__ import annotations

import os
import re
import subprocess
import sys
import tempfile

HERE = os.path.dirname(os.path.abspath(__file__))
ROOT = os.path.abspath(os.path.join(HERE, "..", ".."))
README = os.path.join(ROOT, "bdd", "README.md")
FEATURES = os.path.join(ROOT, "bdd", "features")

# label regex -> which source owns the truth. `None` means "count it here".
LABELS: list[tuple[re.Pattern, str | None]] = [
    (re.compile(r"Current state: \*\*(\d+) scenario"), "transpile"),
    (re.compile(r"(\d+) steps map correctly"), "vocabulary"),
    (re.compile(r"(\d+) step arguments survive"), "vocabulary"),
    (re.compile(r"(\d+) operand types survive"), "vocabulary"),
    (re.compile(r"(\d+) steps still refuse to run without a page"), "vocabulary"),
    (re.compile(r"(\d+) stay refused after a release"), "vocabulary"),
    (re.compile(r"(\d+)/(\d+) argument errors refuse"), "args"),
    (re.compile(r"all (\d+) throw sites are accounted for"), "args"),
    (re.compile(r"\((\d+) pinned here, (\d+) declared elsewhere\)"), "args"),
]


# Fenced blocks in the README that quote one gate's output. Group 1 is the body
# of the block; group 2 (in `src`) names which checker's output must contain
# those lines. The vocab one is indented as a continuation in the README, so the
# leading space is stripped before comparison rather than expected verbatim.
QUOTED_BLOCKS: list[tuple[str, str]] = [
    (r"```\n(bdd vocabulary:[\s\S]*?)```", "vocabulary"),
    (r"```\n\$ python3 scripts/bdd/args_probe_check\.py\n"
     r"(bdd args probes:[\s\S]*?)```", "args"),
]


def run_checker(name: str) -> str:
    proc = subprocess.run([sys.executable, os.path.join(HERE, name)],
                          capture_output=True, text=True, cwd=ROOT)
    return proc.stdout + proc.stderr


def transpiled_count(binary: str) -> int:
    """How many specs bdd/features/ actually compiles to.

    Counted the way check.sh counts, by transpiling, rather than by grepping
    `Scenario:` out of the sources: a Scenario Outline with two examples is one
    line and two specs, and the README means the number of specs.
    """
    import glob
    out = tempfile.mkdtemp()
    try:
        subprocess.run([sys.executable, os.path.join(HERE, "transpile.py")]
                       + sorted(glob.glob(os.path.join(FEATURES, "*.feature")))
                       + ["--out", out],
                       capture_output=True, text=True, cwd=ROOT, check=True)
        return len(glob.glob(os.path.join(out, "*.json")))
    except subprocess.CalledProcessError as e:
        print(f"bdd doc check: transpile failed: {e.stderr[:200]}", file=sys.stderr)
        return -1


def main(argv: list[str]) -> int:
    binary = ""
    if len(argv) > 1:
        binary = argv[1]
    if not binary:
        for rel in ("target/release/laya-workflow", "target/debug/laya-workflow"):
            cand = os.path.join(ROOT, rel)
            if os.access(cand, os.X_OK):
                binary = cand
                break

    with open(README, encoding="utf-8") as f:
        readme = f.read()

    # Run each checker exactly once, then search *its* output for every label
    # owned by it. Keying the collected matches by label rather than by source
    # is what makes the comparison mean anything: keyed by source, every
    # vocabulary label ended up comparing itself against every vocabulary number
    # on one line, so all nine reported "the README says 21 where the truth is
    # [21, 17, 8, 8, 2]" and the one genuinely stale count was lost in noise.
    outputs = {
        "vocabulary": run_checker("vocabulary_check.py"),
        "args": run_checker("args_probe_check.py"),
        "transpile": "",
    }
    n_scenarios = transpiled_count(binary) if binary else -1

    truth: dict[int, list[tuple[int, ...]]] = {}
    for i, (rx, source) in enumerate(LABELS):
        if source == "transpile":
            truth[i] = [(n_scenarios,)] if binary else []
        else:
            truth[i] = [tuple(int(g) for g in m.groups())
                        for m in rx.finditer(outputs[source])]

    problems: list[str] = []
    checked = 0
    # Collapse whitespace in the README before searching labels: wrapping
    # "8 operand types survive" across a line break must not make the label
    # invisible, or the check would go quiet exactly when the page is edited.
    flat_readme = re.sub(r"\s+", " ", readme)
    for i, (rx, source) in enumerate(LABELS):
        in_doc = [tuple(int(g) for g in m.groups())
                  for m in rx.finditer(flat_readme)]
        if not in_doc:
            problems.append(
                f"the README no longer says anything matching {rx.pattern!r}, so "
                "the number it used to quote is no longer checked - either restore "
                "it or drop the label")
            continue
        want = truth.get(i, [])
        if not want:
            problems.append(
                f"the README quotes {rx.pattern!r} but the gate no longer prints "
                "anything matching it - the quote is stale")
            continue
        checked += 1
        if in_doc != want:
            problems.append(
                f"the README says {in_doc[0] if len(in_doc) == 1 else in_doc} "
                f"where the truth is {want[0] if len(want) == 1 else want} for "
                f"{rx.pattern!r} - a number nobody recomputes goes stale on the "
                "next edit")

    # A quoted block can be truncated without any single number going wrong, so
    # also require every line the README shows as gate output to still be a line
    # the gate prints. Matched on the leading label, not on indentation: the
    # README wraps long output across lines.
    for block, src in QUOTED_BLOCKS:
        m = re.search(block, readme)
        if not m:
            problems.append(
                f"the README's quoted block for {block!r} is gone - it was the "
                "evidence that this label is checked")
            continue
        # Collapse all whitespace before comparing: the README wraps the output
        # to fit the page, and comparing line-by-line would flag the wrapping
        # rather than the staleness it is meant to catch.
        def squash(text: str) -> str:
            return re.sub(r"\s+", " ", text).strip()

        # Equality, not containment: a truncated quote is a *prefix* of the real
        # output, so `in` would pass the very failure this checks for - which is
        # how the vocabulary block lost four clauses and stayed green.
        quoted = squash(m.group(1))
        shown_is_full_line = any(
            squash(ln) == quoted for ln in outputs[src].splitlines())
        if not shown_is_full_line:
            problems.append(
                "the README quotes gate output that is not exactly what the gate "
                f"prints: {quoted[:70]!r} - stale or truncated")

    for p in problems:
        print(f"bdd doc check: {p}", file=sys.stderr)
    if problems:
        print(f"bdd doc check: {len(problems)} problem(s)", file=sys.stderr)
        return 1
    print(f"bdd doc check: {checked}/{len(LABELS)} counts the README quotes match "
          f"what the gates print, and every quoted gate line is still printed")
    return 0


if __name__ == "__main__":
    sys.exit(main(sys.argv))
