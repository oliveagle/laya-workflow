#!/usr/bin/env python3
"""Thin shim — the real logic now lives in the Rust subcommand
`laya-workflow server <cmd>` (see src/orchestrate.rs). Kept as a stable,
repo-relative entry point so existing callers keep working; new callers should
invoke the subcommand directly.

argv is forwarded verbatim: `ensure | start [--daemon] | stop | status`
plus `--port / --command / --health-path`.
"""
import os
import sys

bin_ = os.environ.get("LAYA_WORKFLOW_BIN", "laya-workflow")
os.execvp(bin_, [bin_, "server", *sys.argv[1:]])
