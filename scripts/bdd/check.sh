#!/usr/bin/env bash
# Gate: every bdd/features/*.feature compiles into a spec the engine accepts.
#
#   scripts/bdd/check.sh [path-to-laya-workflow]
#
# bdd/features is hand-written; every spec under it is generated. This catches
# the failures that would otherwise surface minutes later as a red browser test
# - a step that no longer parses, a step that now compiles to the wrong op, and
# a scenario that compiles to a spec `validate` rejects.
#
# The last stage runs `laya-workflow bdd run`, which drives a real headless
# Chrome. `bdd run` skips cleanly (exit 0, with a "skipped" line) where no
# browser is installed, so this file stays green on a browserless CI runner and
# green on a browserless box, while a *broken* scenario still fails wherever a
# browser does exist. It is the only gate that exercises the browser
# scenarios, and until it existed a real bug (release.feature bound no browser
# handle) sat in them unnoticed. Set LAYA_BDD_SKIP_BROWSER=1 to skip the stage
# for speed.
#
# Both scripts/verify.sh and .github/workflows/ci.yml call this, so there is
# one definition of "the BDD documents still build and still behave" rather
# than two that can drift.
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

# The decision side of the same idea: an `@decision` step shape the compiler
# accepts but the authoring vocabulary omits (or a template it advertises but
# no longer compiles) is the quietest drift there is - the did-you-mean hint
# and the grammar stop agreeing. Needs no Chrome.
"$BIN" bdd decision-vocab-check

# The hand-written probe specs are gated separately, because `validate` is the
# wrong tool for them: measured, a probe whose edge names a node that does not
# exist *passes* `validate` and fails at run time with error_node_missing and a
# zero exit code. `laya-workflow validate` is still run over them below - it is
# cheap - but probe-check is what actually holds them together.
"$BIN" bdd probe-check

# The plugin's argument-validation errors, which the four browser probes above
# cannot reach. Needs no Chrome either, so it belongs in this file and not in
# the runner. 16 runs, ~25ms, and it fails if the plugin grows an error message
# that nothing pins.
"$BIN" bdd args-probe-check

# The numbers bdd/README.md quotes, recomputed from the gates above. A count
# nobody recomputes goes stale on the next edit - measured: the headline said
# "5 scenarios" when bdd/features/ transpiles to 22, and the quoted vocabulary
# block had lost four clauses to an edit. Same reason as the rest of this file:
# no Chrome, no network, and CI runs it.
"$BIN" bdd doc-check

# The app-parity gate: every built-in app's reference case (`laya-workflow apps`,
# 32 cases across 5 apps: the 4 primitive apps plus the composed gateway)
# replayed end to end through the app's checked-in BDD port spec, and the label
# the engine projects must match the app's expectation. The port is the app's
# routing as *data*; the app is *code*. This holds the two together so a port
# that drifts from the app fails here. No Chrome, no network.
"$BIN" bdd app-parity

# The authoring corpus: can a workflow actually be expressed from an intent, and
# does the vocabulary catch what it should? Offline and Chrome-free - it compiles
# every reference workflow (expressiveness), checks the phrasings an author would
# want to write compile (recall) while the ones that are not in the vocabulary are
# refused by name (precision), and re-checks the @outputs contract on real
# features. `laya-workflow bdd score --run` adds the end-to-end half on a machine
# that has Chrome.
"$BIN" bdd score --check

# The live-fire corpus (bdd/workflows/browser/*.feature), compiled offline. `bdd compare`
# compiles all 29 real-site workflows and runs the seeded-fault gate - no Chrome,
# no network - so a live case that stops parsing is caught here, in CI, instead of
# on the machine that happens to have a browser.
"$BIN" bdd compare >/dev/null

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
  # `.config.json` sidecars moved into the json tree are config, not specs.
  [[ "$spec" == *.config.json ]] && continue
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

# The decision-graph corpus (bdd/workflows/**/*.feature tagged @decision):
# one whole workflow per feature, each Scenario one node. A decision step that
# stops parsing, a generated spec `validate` rejects, or per-feature coverage
# under the bar must fail CI here, the same way the browser corpus is gated.
#
# One `bdd build` over the whole selection, not one process per feature: the gate
# already names every feature it refuses (`<file>: line N: ...` for a step it
# cannot read, `<file>: step coverage N% < 100% (unmatched: ...)` for a typo), so
# a single invocation reports the same failures. The per-feature loop this
# replaces spawned 136 short-lived copies of the binary and was the single
# slowest stage of this script (10 s -> 0.4 s measured, P188).
decision_feats=()
while IFS= read -r feat; do
  decision_feats+=("$feat")
done < <(grep -rl --include="*.feature" "^@decision" "$ROOT/bdd/workflows" | sort)
decision_n=${#decision_feats[@]}
if [ "$decision_n" -gt 0 ]; then
  dout="$(mktemp -d)"
  if ! "$BIN" bdd build "${decision_feats[@]}" --out "$dout" >/dev/null 2> "$dout/build.err"; then
    echo "bdd check: decision features failed to build:" >&2
    cat "$dout/build.err" >&2
    rm -rf "$dout"
    exit 1
  fi
  rm -rf "$dout"
fi

# The round-trip gate: every @decision feature must also reproduce its checked-in
# dsl/<cat>/<name>.json counterpart. The build gate above proves the features
# *compile*; this proves the compiled spec is the same workflow the dsl says it
# is (max_iterations, truthy/falsy annotations, policy/capabilities blocks, ...).
# It needs no Chrome and no network.
if ! "$BIN" bdd round-trip >/dev/null 2> "$out/roundtrip.err"; then
  echo "bdd check: a decision feature drifted from its dsl spec:" >&2
  cat "$out/roundtrip.err" >&2
  exit 1
fi

# The import gate: the reverse of the round-trip above. `bdd import` turns a
# spec back into a `.feature`; this proves every checked-in spec is *recoverable*
# from the feature that import emits, so the two spellings are interchangeable
# and the BDD surface is at least as expressive as the JSON DSL. Needs only this
# repo (no Chrome, no network).
if ! "$BIN" bdd import-check > "$out/import.log" 2> "$out/import.err"; then
  echo "bdd check: a checked-in spec could not be re-imported from a feature:" >&2
  cat "$out/import.err" >&2
  cat "$out/import.log" >&2
  exit 1
fi
grep -E "^bdd import:" "$out/import.log" || true

# The corpus-completeness gate: every checked-in `.feature` under bdd/ compiles,
# and every one is in a gated tree. `bdd/workflows/examples/` - the two features
# docs/bdd_quickstart.md tells a new author to copy - is read by NO other gate,
# so before this a vocabulary change could break a documented example while
# every check stayed green. A `.feature` dropped in a directory no gate compiles
# is named here instead of shipping unverified. Needs no Chrome, no network.
if ! "$BIN" bdd features-check > "$out/features.log" 2> "$out/features.err"; then
  echo "bdd check: a checked-in feature is unverified:" >&2
  cat "$out/features.err" >&2
  cat "$out/features.log" >&2
  exit 1
fi
grep -E "^bdd features-check:" "$out/features.log" || true

# The behavioural smoke gate: every spec under dsl/ must validate, and where a
# labelled sample state is registered (bench/dsl_smoke_states.json) it must
# route/decide to the expected label (bench/dsl_smoke_expect.json) through the
# real engine — offline heuristic backend, so no model, no Chrome, no network.
# Structural round-trip above proves the .feature == dsl spec; this proves the
# dsl specs actually *decide correctly* on the states they are given.
if ! "$BIN" dsl smoke > "$out/smoke.log" 2> "$out/smoke.err"; then
  echo "bdd check: dsl behavioural smoke failed:" >&2
  cat "$out/smoke.err" >&2
  cat "$out/smoke.log" >&2
  exit 1
fi
# Cross-repo conversion gate, in two halves. Every upstream scenario must have a
# BDD port (a @decision feature and a checked-in spec) and every port an upstream
# source. The upstream corpus lives in another repo (ole-eval
# code/qwen_vm/app_scenarios), so its source *names* are sealed under
# bench/bdd/port_pins/<cat>.json; the first half runs against that seal and needs
# no second checkout, so a normal run proves the closure. Point
# LAYA_OLE_EVAL_SOURCES at a live checkout to run the second half, which puts the
# seal itself on trial: the live names must equal the seal's, and any fingerprint
# the seal carries must still match the source.
# Put the seal on trial automatically when the sibling ole-eval checkout sits
# next to this one (`../ole-eval`, the layout both repos are developed in), so a
# dev box compares against the live upstream by default instead of only against
# the checked-in seal. An explicit LAYA_OLE_EVAL_* still wins, and a machine
# without the sibling keeps the offline behaviour and the message below.
if [ -z "${LAYA_OLE_EVAL_ROOT:-}" ] && [ -d "$ROOT/../ole-eval" ]; then
  LAYA_OLE_EVAL_ROOT="$(cd "$ROOT/../ole-eval" && pwd)"
fi
if [ -z "${LAYA_OLE_EVAL_SOURCES:-}" ] && [ -d "${LAYA_OLE_EVAL_ROOT:-}/code/qwen_vm/app_scenarios" ]; then
  LAYA_OLE_EVAL_SOURCES="$LAYA_OLE_EVAL_ROOT/code/qwen_vm/app_scenarios"
fi
PORT_PIN="$ROOT/bench/bdd/port_pins/ole_eval.json"
if [ ! -f "$PORT_PIN" ]; then
  echo "bdd check: the port seal is missing: $PORT_PIN" >&2
  exit 1
fi
if ! "$BIN" bdd port-check --pin "$PORT_PIN" > "$out/port.log" 2>&1; then
  echo "bdd check: the port closure failed against the seal:" >&2
  tail -40 "$out/port.log" | sed 's/^/  /' >&2
  exit 1
fi
if [ -n "${LAYA_OLE_EVAL_SOURCES:-}" ]; then
  if ! "$BIN" bdd port-check --sources "$LAYA_OLE_EVAL_SOURCES" --pin "$PORT_PIN" \
      > "$out/port_live.log" 2>&1; then
    echo "bdd check: the port closure failed against the live checkout:" >&2
    tail -40 "$out/port_live.log" | sed 's/^/  /' >&2
    exit 1
  fi
  echo "port check: closed against the live checkout and the seal is intact"
else
  echo "port check: closed against the seal (set LAYA_OLE_EVAL_SOURCES=<ole-eval code/qwen_vm/app_scenarios> to also put the seal on trial against a live checkout)"
fi

# The scope census (bench/bdd/scope_census.json): the bounded form of "convert
# every known workflow". Each upstream workflow universe in the sibling ole-eval
# repo is either ported in full (proven above by `bdd port-check`) or named with a
# reason, so no universe can be quietly skipped. Offline this judges the census on
# its own; point LAYA_OLE_EVAL_ROOT at an ole-eval *checkout root* to also require
# that the census names exactly the live upstream workflows - a new one nobody
# ported then fails here rather than going unnoticed.
if ! "$BIN" bdd scope-check > "$out/scope.log" 2>&1; then
  echo "bdd check: the scope census is not green:" >&2
  tail -40 "$out/scope.log" | sed 's/^/  /' >&2
  exit 1
fi
if [ -n "${LAYA_OLE_EVAL_ROOT:-}" ]; then
  if ! "$BIN" bdd scope-check --sources "$LAYA_OLE_EVAL_ROOT" > "$out/scope_live.log" 2>&1; then
    echo "bdd check: the scope census does not match the live checkout:" >&2
    tail -40 "$out/scope_live.log" | sed 's/^/  /' >&2
    exit 1
  fi
  echo "scope check: the census matches the live checkout"
else
  echo "scope check: every upstream universe accounted for offline (set LAYA_OLE_EVAL_ROOT=<ole-eval checkout> to match the live upstream workflows)"
fi

# The browser run: the bdd/features scenarios and the hand-written probes
# actually drive a real headless Chrome over CDP. `bdd run` skips cleanly when
# no browser is present, so this stage is green on a browserless box; it fails
# only when a scenario genuinely fails while a browser *is* available.
if [ "${LAYA_BDD_SKIP_BROWSER:-}" = "1" ]; then
  echo "browser run: skipped (LAYA_BDD_SKIP_BROWSER=1)"
else
  if ! "$BIN" bdd run > "$out/browsers.log" 2>&1; then
    echo "bdd check: the browser run failed:" >&2
    tail -40 "$out/browsers.log" | sed 's/^/  /' >&2
    exit 1
  fi
  if grep -q "bdd: skipped - no Chrome" "$out/browsers.log"; then
    echo "browser run: skipped (no Chrome/Chromium on this box)"
  else
    grep -E "bdd: [0-9]+ passed" "$out/browsers.log" | tail -1 | sed 's/^bdd: /browser run: /' || true
  fi
fi

smoke_line="$(grep -oE "[0-9]+/[0-9]+ specs OK" "$out/smoke.log" | tail -1)"
echo "$n/$n scenarios compile, validate and are 100% covered, $probes/$probes probe specs validate, $decision_n/$decision_n decision workflows compile at 100% coverage and round-trip to their dsl spec; behavioural smoke: $smoke_line"
# Every dsl spec is accounted for: either it carries offline smoke states, or it
# is declared live-only in bench/dsl_coverage_ledger.json. `dsl smoke` fails on
# an unaccounted spec or a stale declaration, so this line cannot overstate the
# coverage it prints.
grep -oE "coverage ledger: .*specs" "$out/smoke.log" | tail -1 || true
# The two-tree separation the user requires: bdd/ holds no json, dsl/ holds no
# feature, both hierarchical. `dsl smoke` fails on any violation, so this line
# only prints when the invariant holds.
grep -oE "isolation: .*hierarchical" "$out/smoke.log" | tail -1 || true
