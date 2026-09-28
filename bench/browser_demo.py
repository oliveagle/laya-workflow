#!/usr/bin/env python3
"""Run the singleton Chrome CDP demo with a temporary localhost page."""

import json
import os
import shutil
import socket
import subprocess
import sys
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]
SPEC = ROOT / "dsl/browser/browser_singleton.json"


def find_cli() -> str:
    override = os.environ.get("LAYA_WORKFLOW_BIN")
    if override:
        return override
    found = shutil.which("laya-workflow")
    if found:
        return found
    built = ROOT / "target/release/laya-workflow"
    if built.is_file():
        return str(built)
    raise SystemExit("laya-workflow not found; run cargo build --release --bins first")


def start_site() -> tuple[subprocess.Popen[bytes | str], int]:
    # A small deterministic range avoids a macOS Chrome issue with some
    # OS-assigned ephemeral loopback ports.
    for port in range(18777, 18787):
        server = subprocess.Popen(
            [sys.executable, "-m", "http.server", str(port), "--bind", "127.0.0.1"],
            cwd=ROOT / "bench/demo-site",
            stdout=subprocess.DEVNULL,
            stderr=subprocess.DEVNULL,
        )
        try:
            socket.create_connection(("127.0.0.1", port), timeout=0.5).close()
            return server, port
        except OSError:
            server.terminate()
            server.wait()
    raise SystemExit("demo ports 18777-18786 are busy")


def main() -> None:
    server, port = start_site()
    try:
        url = f"http://127.0.0.1:{port}/index.html"
        state = {"url": url, "text": "hello from Laya"}
        command = [
            find_cli(),
            "run",
            "--spec",
            str(SPEC),
            "--state",
            json.dumps(state),
        ]
        print(f"demo page: {url}")
        print(f"state: {json.dumps(state)}")
        print(f"command: {' '.join(command)}")
        result = subprocess.run(command)
    finally:
        server.terminate()
        server.wait()
    raise SystemExit(result.returncode)


if __name__ == "__main__":
    main()
