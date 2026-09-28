#!/usr/bin/env bash
# Thin shim — the real logic now lives in the Rust subcommand
# `laya-workflow browser ensure --backend chrome` (see src/orchestrate.rs). Kept
# as a stable, repo-relative entry point so existing callers that shell out to
# this path keep working; new callers should invoke the subcommand directly.
#
# Env: LAYA_CDP_PORT / CHROME_BIN / LAYA_CDP_PROFILE (read by the subcommand).
# Extra args are forwarded, e.g. `... --port 9223`.
set -euo pipefail
BIN="${LAYA_WORKFLOW_BIN:-laya-workflow}"
exec "$BIN" browser ensure --backend chrome "$@"
