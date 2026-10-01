#!/usr/bin/env bash
# The gate before you push, in about a second.
#
#   scripts/verify.sh              build + unit tests + plugin gate + fast suite   ~1s warm
#   scripts/verify.sh --full       ... and the timeout section too         +169s
#   scripts/verify.sh --no-build   tests only (you already built)
#   scripts/verify.sh spec engine  only these sections
#
# Why the split: `laya-workflow-tests` has 22 sections and 21 of them finish in
# 0.16s. The other one, `capability-timeouts`, takes 168.9s on its own - it is
# almost the entire runtime of a "full" run, and almost none of it is needed to
# answer "did I break something". Running the whole thing by reflex is how a
# 3-second question turns into a 3-minute one.
#
# There is deliberately no baseline of "known failures" here. A baseline is a
# fixture that rots: this repo shipped a test asserting a spec named
# `status_snapshot`, that spec moved to another repo, and the assertion read as
# a resolver bug for eight consecutive pushes - main stayed red the whole time
# because a failure nobody was willing to call "expected" was already known.
# Red here means red; fix it.
set -uo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
BIN="$ROOT/target/release/laya-workflow-tests"
# The CLI, not the test binary. `laya-workflow-tests` was being handed to
# scripts/bdd/check.sh for a long time, and it accepts `validate --spec` for a
# file that does not exist - printing "0 passed, 0 failed" and exiting 0 - so
# every validation in that script passed without reading a spec. Two names, one
# job each, so the next reader cannot mix them up again.
CLI="$ROOT/target/release/laya-workflow"
SLOW="capability-timeouts"
BUILD=1
FULL=0
SECTIONS=()

while [ $# -gt 0 ]; do
  case "$1" in
    --full)     FULL=1; shift ;;
    --no-build) BUILD=0; shift ;;
    -h|--help)  sed -n '2,20p' "$0"; exit 0 ;;
    *)          SECTIONS+=("$1"); shift ;;
  esac
done

stage() { printf '\n== %s\n' "$1"; }
rc=0
TMPERR="$(mktemp)"
trap 'rm -f "$TMPERR"' EXIT

# ── 1. does it build? ────────────────────────────────────────────────────────
# Incremental when src/** is unchanged (~1s), a full rebuild when it is. Either
# way it is the honest precondition: the test binary is what we are about to
# trust, and it has to be the current one.
if [ "$BUILD" = 1 ]; then
  stage "cargo build --release --locked"
  T0=$SECONDS
  BUILD_OUT="$(cargo build --release --locked --bins -p laya-workflow \
                 --manifest-path "$ROOT/Cargo.toml" 2>&1)"
  BRC=$?
  # Cargo's progress lines are noise once it is warm; its warnings are not.
  printf '%s\n' "$BUILD_OUT" | grep -Ev '^ *(Compiling|Finished|Downloaded|Updating|Locking|Adding)' | grep -v '^$' || true
  [ "$BRC" = 0 ] || { echo "   build FAILED" >&2; exit 1; }
  printf '   [%ss]\n' "$((SECONDS-T0))"
fi

[ -x "$BIN" ] || { echo "no $BIN - run without --no-build first" >&2; exit 1; }
[ -x "$CLI" ] || { echo "no $CLI - run without --no-build first" >&2; exit 1; }

# ── 2. do the #[cfg(test)] unit tests still pass? ──────────────────────────────
# `laya-workflow-tests` is the offline suite, but the crate also carries
# `#[cfg(test)]` tests (state-dir resolution, the embedded laya-mem specs, plugin
# installation). Nothing else runs them, so a change there can only be checked by
# someone remembering `cargo test` by hand - which is how they rot.
#
# 100+ tests, no network, no browser: the lib target alone.
T0=$SECONDS
if [ "$BUILD" = 1 ] || [ ${#SECTIONS[@]} -eq 0 ]; then
  stage "unit tests"
  UNIT_OUT="$(cd "$ROOT" && cargo test --release -p laya-workflow --lib 2>&1)"
  URC=$?
  printf '%s\n' "$UNIT_OUT" | grep -E '^test result|^error' | sed 's/^/   /'
  [ "$URC" = 0 ] || rc=1
  printf '   [%ss]\n' "$((SECONDS-T0))"
else
  stage "unit tests"
  echo "   skipped (section filter given; unit tests run only for a full gate)"
fi

# ── 3. do the plugins still compile and still route? ─────────────────────────
# It is the only stage that can catch a plugin edit: the Rust suite never
# executes Rhai.
#
# Every plugin, not the first one found. There are 14 of them and the Rust
# suite executes none of them, so a gate that compiles goofish alone would call
# the repo green with a broken taobao plugin sitting in it - which is precisely
# the shape of failure this file exists to prevent. With compile.py's allow_hosts
# fix the whole set costs ~0.3s, so there is no reason to sample.
T0=$SECONDS
NPLUG=0
BADPLUG=""
while IFS= read -r p; do
  [ -n "$p" ] || continue
  NPLUG=$((NPLUG+1))
  if ! python3 "$ROOT/scripts/rhai/compile.py" "$p" >/dev/null 2>"$TMPERR"; then
    BADPLUG="$BADPLUG ${p#"$ROOT"/}"
    echo "   FAILED ${p#"$ROOT"/}"
    sed 's/^/     /' "$TMPERR" | head -3
  fi
done < <(find "$ROOT/websites" -name '*.rhai' 2>/dev/null | sort)

if [ "$NPLUG" -gt 0 ]; then
  stage "plugin gate"
  # check.sh additionally *runs* goofish to assert routing and the fold, which
  # compile.py cannot: those are behaviour, not syntax.
  "$ROOT/scripts/rhai/check.sh" 2>&1 | grep -E '^(ok:|   ok|   corpus|all green|error)' || rc=1
  # Compiling a plugin says nothing about whether anyone can find it. The
  # `## Bundled plugins` table is the registry, and it had drifted: three of
  # the four directories under plugins/ were missing from one registry or the
  # other. Wired into CI as well, because verify.sh is not what CI runs.
  python3 "$ROOT/scripts/plugin_registry_check.py" || rc=1
  if [ -z "$BADPLUG" ]; then
    printf '   %d/%d plugins compiled [%ss]\n' "$NPLUG" "$NPLUG" "$((SECONDS-T0))"
  else
    printf '   %d/%d plugins compiled; FAILED:%s [%ss]\n' \
      "$((NPLUG - $(printf '%s' "$BADPLUG" | wc -w | tr -d ' ')))" "$NPLUG" "$BADPLUG" "$((SECONDS-T0))"
    rc=1
  fi
else
  stage "plugin gate"
  echo "   no plugins under websites/ - skipped"
fi

# ── 4. do the BDD documents still compile into runnable specs? ───────────────
# bdd/features/*.feature is hand-written; every spec under it is generated.
# This catches a step that no longer parses, or a scenario that compiles to a
# spec the engine rejects - both of which would otherwise surface minutes later
# as a red browser test. No Chrome, no network, so it is cheap enough for the
# default gate. The logic lives in scripts/bdd/check.sh so that CI and this
# script cannot drift apart; the CDP run is a separate, explicit gate
# (scripts/bdd/run.py).
stage "bdd transpile"
if [ -d "$ROOT/bdd/features" ] && compgen -G "$ROOT/bdd/features/*.feature" >/dev/null; then
  if ! "$ROOT/scripts/bdd/check.sh" "$CLI" 2>"$TMPERR" | sed 's/^/   /'; then
    sed 's/^/   /' "$TMPERR" >&2
    rc=1
  fi
else
  echo "   no bdd/features/*.feature - skipped"
fi

# ── 5. the tests ─────────────────────────────────────────────────────────────
# One section per line: read it as lines, not as one multi-line word.
ALL=()
while IFS= read -r line; do
  [ -n "$line" ] && ALL+=("$line")
done < <("$BIN" --list)
RUN=()
if [ ${#SECTIONS[@]} -gt 0 ]; then
  RUN=("${SECTIONS[@]}")
elif [ "$FULL" = 1 ]; then
  RUN=("${ALL[@]}")
  printf '\n== full suite (%d sections, ~170s - %s sleeps on real timeouts)\n' "${#ALL[@]}" "$SLOW"
else
  for s in "${ALL[@]}"; do [ "$s" = "$SLOW" ] || RUN+=("$s"); done
  printf '\n== fast suite (%d of %d sections; skipping %s - rerun with --full)\n' \
    "${#RUN[@]}" "${#ALL[@]}" "$SLOW"
fi

T0=$SECONDS
OUT="$("$BIN" "${RUN[@]}" 2>&1)"
TRC=$?
printf '%s\n' "$OUT" | grep -E '^  FAIL|passed,'
printf '   [%ss]\n' "$((SECONDS-T0))"
[ "$TRC" = 0 ] || rc=1

[ "$rc" = 0 ] && printf '\nall green\n' || printf '\nRED\n'
exit $rc
