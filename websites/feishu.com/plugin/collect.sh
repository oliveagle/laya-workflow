#!/bin/sh
# Collect unread Feishu (飞书) messages across every chat, for the unread digest.
#
#   collect.sh [chats_limit] [page_size]      defaults: 15 chats, 20 messages
#
# Delegates to `laya-workflow feishu-collect`, which does the whole fetch.
# Emits one JSON object on stdout: {"ok":true,"chats":[{chat_id,chat_name,
# chat_type,unread:[message...]}]} - chats with nothing unread are omitted, so
# a quiet account prints {"ok":true,"chats":[]}.
#
# There is no Feishu unread API to ask ("GET /open-apis/im/v1/message_badge" is a
# 404). Unread is therefore *derived*, in two calls per chat:
#   1. +chat-messages-list --order desc   recent messages, newest first
#   2. +messages-read-status               is_read per message id, 50 at a time
# and the digest keeps the intersection. Cost is two CLI spawns per chat, so
# chats_limit is the knob that trades coverage for latency (~1.3s per chat
# against a cold token; a full 20-chat sweep measures ~22s).
#
# Failures are *not* swallowed: `ok:false` with the CLI's own error is printed
# and the exit code is non-zero, so the classifier reports "collect failed"
# rather than digesting an empty list as "you are all clear" - the failure mode
# that makes a triage list dangerous to trust.

set -u
# Thin shim: all collection logic lives in the Rust binary so the plugin path
# needs no Python. `--print-knobs` is passed through for `laya-workflow plugin
# check` to assert the defaulting against the real code, not a copy.
HERE="$(cd "$(dirname "$0")" && pwd)"
REPO_ROOT="$(cd "$HERE/../../.." && pwd)"
BIN="${LAYA_WORKFLOW_BIN:-}"
# Prefer the repo's own build (this plugin ships with the repo and the gate
# runs from the repo); fall back to a PATH install for deployed setups.
if [ -z "$BIN" ] && [ -x "$REPO_ROOT/target/release/laya-workflow" ]; then
  BIN="$REPO_ROOT/target/release/laya-workflow"
fi
if [ -z "$BIN" ]; then
  BIN="$(command -v laya-workflow 2>/dev/null || true)"
fi
if [ -z "$BIN" ] || [ ! -x "$BIN" ]; then
  echo "laya-workflow binary not found (set LAYA_WORKFLOW_BIN)" >&2
  exit 2
fi
if [ "${1:-}" = "--print-knobs" ]; then
  exec "$BIN" feishu-collect --print-knobs "${2:-}" "${3:-}"
fi
exec "$BIN" feishu-collect "${1:-}" "${2:-}"
