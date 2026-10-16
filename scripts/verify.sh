# The gate before you push, in about half a minute.
#
#   scripts/verify.sh              build + unit tests + plugin gate + bdd + fast suite   ~30s warm
#                                  (~14s of that is the real-Chrome bdd run; export
#                                   LAYA_BDD_SKIP_BROWSER=1 to skip it, ~18s)
#   scripts/verify.sh --full       ... and the timeout section too        +169s
#   scripts/verify.sh --no-build   tests only (you already built)
#   scripts/verify.sh spec engine  only these sections
#
# Why the split: all but one of `laya-workflow-tests`'s sections finish in about
# 10s (the run prints the exact counts, so none is pinned here - the "103" this
# comment used to carry had long since rotted). The one exception,
# `capability-timeouts`, takes ~169s on its own - it is almost the entire runtime
# of a "full" run, and almost none of it is needed to answer "did I break
# something". Running the whole thing by reflex is how a 3-second question turns
# into a 3-minute one.
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
# executes Rhai. Every plugin, not a sample: 20 shipped `.rhai` files, plus the
# goofish intent routing, the fold guard and the feishu classifier fixtures —
# all in-process in the binary, one command, no Python. The fold guard drives a
# real browser; where none is installed it is skipped, not failed, so the gate
# stays honest on a browserless box (CI installs no browser).
T0=$SECONDS
stage "plugin gate"
if ! "$ROOT/target/release/laya-workflow" plugin check 2>&1 | sed 's/^/   /'; then
  rc=1
fi

# Compiling a plugin says nothing about whether anyone can find it. The
# `## Bundled plugins` table is the registry, and it had drifted: three of the
# four directories under plugins/ were missing from one registry or the other.
# Wired into CI as well, because verify.sh is not what CI runs.
"$ROOT/target/release/laya-workflow" plugin registry-check || rc=1
printf '   [%ss]\n' "$((SECONDS-T0))"
# ── 4. do the BDD documents still compile into runnable specs? ───────────────
# bdd/features/*.feature is hand-written; every spec under it is generated.
# This catches a step that no longer parses, or a scenario that compiles to a
# spec the engine rejects - both of which would otherwise surface minutes later
# as a red browser test. The logic lives in scripts/bdd/check.sh so that CI and
# this script cannot drift apart. That script ends with `laya-workflow bdd run`,
# which drives a real headless Chrome; it skips cleanly (exit 0) where no browser
# is installed, so this gate is green on a browserless box while still exercising
# the browser scenarios wherever a browser exists. Set LAYA_BDD_SKIP_BROWSER=1 to
# skip that stage for a faster gate.
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
