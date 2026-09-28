#!/usr/bin/env bash
# The whole feedback loop for a plugin change, in one command, in about a second.
#
#   check.sh                 compile -> fold -> done            (~0.3s)
#   check.sh --cargo          ... then cargo test --release      (~1s warm)
#   check.sh --bisect         compile failure localised to a block
#
# Why this exists: the only other way to find out whether a plugin still parses
# is `laya-workflow run` on the real DSL, which browses live pages and takes
# 20+ seconds, and which tells you a parse error is on line 1965 without saying
# which of the 1965 lines is wrong. Both costs are avoidable.
#
# Stage 1  compile.py   compiles the plugin alone.        ~0.07s
# Stage 2  fold         mode=evolve over a scratch corpus: real learn(), real
#                       file writes, zero browsing.       ~0.13s
# Stage 3  cargo test   only with --cargo, and only once at the end.  ~1s warm
#
# Stage 2 runs against a copy of the corpus so that a syntax gate never edits
# the real out_dir, and it needs the scratch dir to sit inside the spec's
# policy.allow_paths -- the engine enforces that, and the failure is a wall of
# text. Hence: <allow_paths entry>/.check rather than /tmp.
set -uo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
SPEC="$ROOT/dsl/browser/goofish_item.json"
PLUGIN="$ROOT/websites/goofish.com/plugin/main.rhai"
CORPUS="$HOME/tmp/goofish"
BISECT=0
CARGO=0
BROWSE=0

while [ $# -gt 0 ]; do
  case "$1" in
    --spec)    SPEC="$2"; shift 2 ;;
    --plugin)  PLUGIN="$2"; shift 2 ;;
    --corpus)  CORPUS="$2"; shift 2 ;;
    --bisect)  BISECT=1; shift ;;
    --cargo)   CARGO=1; shift ;;
    --browse)  BROWSE=1; shift ;;
    -h|--help) sed -n '2,20p' "$0"; exit 0 ;;
    *) echo "unknown flag: $1" >&2; exit 2 ;;
  esac
done

stage() { printf '\n== %s\n' "$1"; }
clock() { local t0=$SECONDS; "$@"; local rc=$?
          printf '   [%ss] %s\n' "$((SECONDS-t0))" "$*"; return $rc; }

# ── 1. does it parse? ────────────────────────────────────────────────────────
stage "compile $PLUGIN"
if [ "$BISECT" = 1 ]; then
  clock python3 "$ROOT/scripts/rhai/compile.py" "$PLUGIN" --bisect || exit 1
else
  clock python3 "$ROOT/scripts/rhai/compile.py" "$PLUGIN" || exit 1
fi

# ── 1b. does the intent still route? ─────────────────────────────────────────
# plan() decides what the query means. A regression here is silent and expensive:
# an intent word that survives as a search term brings back a page of unrelated
# listings, files all of them in the corpus, and then feeds the vocabulary.
# ~0.15s, and it is the check that would have caught that.
stage "intent routing"
ROUTING='let a = plan(#{ query: "价格进化 CMP 170HX" });
let b = plan(#{ query: "价格进化" });
let c = plan(#{ query: "索尼 A7M4" });
#{ ok: a.mode == "evolve" && a.query == "CMP 170HX" && scope_of(a.query) == "cmp170hx"
   && b.mode == "evolve" && b.query == "价格进化" && strip_intent(b.query) == ""
   && c.mode == "search" && c.query == "索尼 A7M4" }'
GATE="$(python3 "$ROOT/scripts/rhai/harness.py" "$PLUGIN" --run --out "$HOME/tmp/rhai-gate" --body "$ROUTING" 2>&1)"
if echo "$GATE" | grep -q '"ok": true'; then
  echo "   ok: evolve keeps the noun, drops the verb; a plain search is untouched"
else
  echo "   intent routing regressed:"; echo "$GATE" | head -20; exit 1
fi

# ── 2. does it still learn? ─────────────────────────────────────────────────
# Scratch corpus: the real index.json/watch.json, copied in. Everything the
# fold writes lands in $SCRATCH, so the real out_dir is read-only here.
# The scratch corpus has to live inside the spec policy allow_paths (the engine
# enforces it, and the failure is a wall of text), so ask the spec where that is
# rather than guessing - and expand ${env.HOME} the way the engine does.
RAW="$(python3 -c 'import json,sys; print(json.load(open(sys.argv[1]))["policy"]["allow_paths"][-1])' "$SPEC")"
ALLOW="${RAW//\$\{env.HOME\}/$HOME}"
SCRATCH="$ALLOW/.check"
mkdir -p "$SCRATCH"
for f in index.json watch.json; do
  [ -f "$CORPUS/$f" ] && cp "$CORPUS/$f" "$SCRATCH/$f"
done

STATE="{\"mode\":\"evolve\",\"out_dir\":\"$SCRATCH\"}"
stage "fold (mode=evolve, no query, corpus -> $SCRATCH)"
if [ "$BROWSE" = 1 ]; then
  # the one expensive run: real pages, real prices. Do it last, once.
  OUT="$( "$ROOT/target/release/laya-workflow" run --spec "$SPEC" \
           --query "价格进化 CMP 170HX" --state "$STATE" 2>&1 )"
else
  OUT="$( "$ROOT/target/release/laya-workflow" run --spec "$SPEC" \
           --query "价格进化" --state "$STATE" 2>&1 )"
fi
echo "$OUT" | grep -E '"(scope|fair|next_feed_factor|deals|overpriced)"|Error|error:' | head -20 \
  || echo "$OUT" | tail -20
if echo "$OUT" | grep -q 'Error'; then
  echo "   fold FAILED" >&2; echo "$OUT" | tail -30; exit 1
fi
for f in index.json tags.json price_model.json; do
  printf '   wrote %s (%s bytes)\n' "$f" "$(wc -c < "$SCRATCH/$f" 2>/dev/null | tr -d ' ')"
done
# A fold must learn from the corpus it was handed, not add to it. New items mean
# the run went browsing - which costs 20s and, when the query is nothing but an
# intent word, files the site's junk answers into the corpus (30 "进化" games
# and ebooks once landed under the scope "价格进化" that way).
BEFORE="$(python3 -c 'import json,sys; print(len(json.load(open(sys.argv[1])).get("items",[])))' "$SCRATCH/index.json" 2>/dev/null || echo 0)"
AFTER="$(python3 -c 'import json,sys; print(len(json.load(open(sys.argv[1])).get("items",[])))' "$SCRATCH/index.json")"
printf '   corpus %s -> %s items%s\n' "$BEFORE" "$AFTER" \
  "$( [ "$BEFORE" = "$AFTER" ] && echo ' (no browsing)' || echo '  <-- IT BROWSED' )"
[ "$BEFORE" = "$AFTER" ] || exit 1

# ── 3. did the engine change? ────────────────────────────────────────────────
# src/** untouched means cargo test is testing nothing you just wrote. Run it
# once, here, at the end -- not four times spread across the edit.
if [ "$CARGO" = 1 ]; then
  stage "cargo test --release"
  clock cargo test --release --manifest-path "$ROOT/Cargo.toml" --quiet || exit 1
fi

printf '\nall green\n'
