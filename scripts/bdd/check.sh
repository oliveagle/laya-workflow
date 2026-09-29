#!/usr/bin/env bash
# Gate: every bdd/features/*.feature compiles into a spec the engine accepts.
#
#   scripts/bdd/check.sh [path-to-laya-workflow]
#
# bdd/features is hand-written; every spec under it is generated. This catches
# the failures that would otherwise surface minutes later as a red browser test
# - a step that no longer parses, a step that now compiles to the wrong op, and
# a scenario that compiles to a spec `validate` rejects. It needs no Chrome and no network, which is why it is
# safe in CI; the CDP run in scripts/bdd/run.py is the separate local gate,
# because CI installs no browser.
#
# Both scripts/verify.sh and .github/workflows/ci.yml call this, so there is
# one definition of "the BDD documents are still buildable" rather than two
# that can drift.
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

shopt -s nullglob
features=("$ROOT"/bdd/features/*.feature)
if [ ${#features[@]} -eq 0 ]; then
  echo "bdd check: no bdd/features/*.feature - skipped"
  exit 0
fi

# First: does every known step still map to the op it is supposed to run? This
# needs no Chrome and catches the quietest failure there is - a step that
# compiles to a *different* assertion than the document says, which leaves the
# run green while the document stops meaning what it says.
python3 "$HERE/vocabulary_check.py"

out="$(mktemp -d)"
trap 'rm -rf "$out"' EXIT

python3 "$HERE/transpile.py" "${features[@]}" --out "$out" >/dev/null

n=0
bad=()
for spec in "$out"/*.json; do
  n=$((n + 1))
  if ! "$BIN" validate --spec "$spec" >/dev/null 2>&1; then
    bad+=("$(basename "$spec")")
  fi
done

if [ ${#bad[@]} -gt 0 ]; then
  echo "bdd check: ${#bad[@]}/$n generated specs failed validation:" >&2
  for b in "${bad[@]}"; do
    echo "  $b" >&2
    "$BIN" validate --spec "$out/$b" 2>&1 | head -5 | sed 's/^/    /' >&2 || true
  done
  exit 1
fi
echo "$n/$n scenarios compile and validate"
