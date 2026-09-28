#!/usr/bin/env bash
# Ensure a Chrome with CDP (remote-debugging) listening on 127.0.0.1:<port>.
#
# This is the generic Chrome/CDP half of local browser orchestration for Laya
# workflows: if a CDP endpoint is already up it does nothing (idempotent);
# otherwise it launches a dedicated Chrome with an isolated temp profile and
# waits for the endpoint.  Point a workflow's `chrome_cdp` capability at
# http://127.0.0.1:<port> and run this (via a `shell` capability with
# policy.allow_exec=true) before the browser nodes.
#
# Usage:
#   LAYA_CDP_PORT=9222 ./scripts/laya-ensure-chrome.sh
#
# Env:
#   LAYA_CDP_PORT   CDP port (default 9222)
#   CHROME_BIN      Chrome executable (default macOS branded path; override
#                   for Linux/headless setups)
set -euo pipefail

PORT="${LAYA_CDP_PORT:-9222}"

if nc -z 127.0.0.1 "$PORT" 2>/dev/null; then
  echo "ok Chrome CDP already listening on 127.0.0.1:${PORT}"
  exit 0
fi

CHROME="${CHROME_BIN:-/Applications/Google Chrome.app/Contents/MacOS/Google Chrome}"
if [ ! -x "$CHROME" ]; then
  echo "fail Google Chrome not found at ${CHROME} (set CHROME_BIN)" >&2
  exit 1
fi

PROFILE="${LAYA_CDP_PROFILE:-/tmp/laya-chrome-cdp-profile}"
mkdir -p "$PROFILE"
"$CHROME" --remote-debugging-port="$PORT" --user-data-dir="$PROFILE" \
  --no-first-run --no-default-browser-check --no-service-autorun \
  about:blank >/dev/null 2>&1 &
disown

for _ in $(seq 1 30); do
  if nc -z 127.0.0.1 "$PORT" 2>/dev/null; then
    echo "ok Chrome CDP up on 127.0.0.1:${PORT} (profile ${PROFILE})"
    exit 0
  fi
  sleep 0.5
done
echo "fail Chrome CDP did not come up on :${PORT}" >&2
exit 1
