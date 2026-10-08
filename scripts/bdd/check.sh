#!/usr/bin/env bash
# Gate: every bdd/features/*.feature compiles into a spec the engine accepts.
#
#   scripts/bdd/check.sh [path-to-laya-workflow]
#
# bdd/features is hand-written; every spec under it is generated. This catches
# the failures that would otherwise surface minutes later as a red browser test
# - a step that no longer parses, a step that now compiles to the wrong op, and
# a scenario that compiles to a spec `validate` rejects. It needs no Chrome and no network, which is why it is
# safe in CI; the CDP run in `laya-workflow bdd run` is the separate local gate,
# because CI installs no browser.
#
# Both scripts/verify.sh and .github/workflows/ci.yml call this, so there is
# one definition of "the BDD documents are still buildable" rather than two
# that can drift.
#
# The checks live in the Rust binary (`laya-workflow bdd <check|build>`), the
# port of the former scripts/bdd/*.py toolchain — no Python involved.
set -euo pipefail

HERE="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
ROOT="$(cd "$HERE/../.." && pwd)"
BIN="${1:-}"

if [ -z "$BIN" ]; then
  for rel in target/release/laya-workflow target/debug/laya-workflow; do
    if [ -x "$ROOT/$rel" ]; then BIN="$ROOT/$rel"; break; fi
  done
fi
if [ -z "$BIN" ] || [ ! -x "$BIN" ]; then
  echo "bdd check: no laya-workflow binary; build it or pass the path" >&2
  exit 1
fi

# ── is $BIN actually the CLI? ──
# Not a nicety, and not paranoia: `laya-workflow-tests` accepts
# `validate --spec` for a file that does not exist, prints "0 passed, 0 failed"
# and exits 0. Hand it the test binary and every validation below passes without
# reading a single spec - which is exactly what scripts/verify.sh did, for as
# long as the bdd checks have existed. It surfaced only because the args probe
# asserts on an expected *failure*: all fifteen of its rows "succeeded" under
# verify.sh while refusing correctly on their own.
#
# So check the binary before trusting it, loudly, once.
_absent="$(mktemp -d)/absent.json"
if "$BIN" validate --spec "$_absent" >/dev/null 2>&1; then
  echo "bdd check: $BIN is not the laya-workflow CLI" >&2
  echo "  it accepted a spec that does not exist, so every validation in this" >&2
  echo "  script would pass without reading anything. Pass target/release/laya-workflow," >&2
  echo "  not laya-workflow-tests." >&2
  exit 1
fi

shopt -s nullglob
# Non-recursive on purpose: bdd/features/setup/ holds the step lists pulled in
# with `include:`, and those are a flat list of Given/When/Then with no
# `Feature:` header. Compiling one as a feature fails with "no Feature: header
# found", which reads like a broken feature rather than a directory that is not
# one. Changing this to **/*.feature needs the setup/ exclusion too.
features=("$ROOT"/bdd/features/*.feature)
if [ ${#features[@]} -eq 0 ]; then
  echo "bdd check: no bdd/features/*.feature - skipped"
  exit 0
fi

# First: does every known step still map to the op it is supposed to run? This
# needs no Chrome and catches the quietest failure there is - a step that
# compiles to a *different* assertion than the document says, which leaves the
# run green while the document stops meaning what it says.
"$BIN" bdd vocabulary-check

# The hand-written probe specs are gated separately, because `validate` is the
# wrong tool for them: measured, a probe whose edge names a node that does not
# exist *passes* `validate` and fails at run time with error_node_missing and a
# zero exit code. `laya-workflow validate` is still run over them below - it is
# cheap - but probe-check is what actually holds them together.
"$BIN" bdd probe-check

# The plugin's argument-validation errors, which the four browser probes above
# cannot reach. Needs no Chrome either, so it belongs in this file and not in
# the runner. 15 runs, ~25ms, and it fails if the plugin grows an error message
# that nothing pins.
"$BIN" bdd args-probe-check

# The numbers bdd/README.md quotes, recomputed from the gates above. A count
# nobody recomputes goes stale on the next edit - measured: the headline said
# "5 scenarios" when bdd/features/ transpiles to 22, and the quoted vocabulary
# block had lost four clauses to an edit. Same reason as the rest of this file:
# no Chrome, no network, and CI runs it.
"$BIN" bdd doc-check

out="$(mktemp -d)"
trap 'rm -rf "$out"' EXIT

# The unified accuracy gate: every scenario compiles, every generated spec
# passes the real `validate`, per-feature step coverage must be 100%, and an
# empty selection is an error (a typo'd tag must not report success). This is
# `laya-workflow bdd build` - the same command an agent runs before committing
# a .feature edit, so the gate and the daily loop cannot drift.
if ! "$BIN" bdd build "${features[@]}" --out "$out" >/dev/null 2> "$out/build.err"; then
  echo "bdd check: the build gate failed:" >&2
  cat "$out/build.err" >&2
  exit 1
fi

# Recover the scenario count from the build the cheap way: one spec file per
# selected scenario. (The manifest has the same count; counting spec files
# needs no JSON tooling.)
n="$(find "$out/spec" -maxdepth 1 -name '*.json' | wc -l)"

# The probes are not generated, so they are not in "$out" - but they are the
# specs the runner actually executes, and a broken one used to look green.
# The glob is `*probe*`, not `bdd_*probe*`: browser_base_probe.json is cited by
# four documents as the canonical multi-step shape and had no gate at all until
# round 17, which is how a cited example goes stale unnoticed.
probes=0
probe_bad=()
for spec in "$ROOT"/dsl/browser/*probe*.json; do
  probes=$((probes + 1))
  if ! "$BIN" validate --spec "$spec" >/dev/null 2>&1; then
    probe_bad+=("$(basename "$spec")")
  fi
done
if [ ${#probe_bad[@]} -gt 0 ]; then
  echo "bdd check: ${#probe_bad[@]}/$probes hand-written probe specs failed validation:" >&2
  for b in "${probe_bad[@]}"; do
    echo "  $b" >&2
    "$BIN" validate --spec "$ROOT/dsl/browser/$b" 2>&1 | head -5 | sed 's/^/    /' >&2 || true
  done
  exit 1
fi

echo "$n/$n scenarios compile, validate and are 100% covered, $probes/$probes probe specs validate"
