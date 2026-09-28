#!/usr/bin/env python3
"""laya-ensure-server — generic local HTTP server lifecycle for Laya workflows.

This is the *generic* server half of local browser orchestration: keep a
long-running local HTTP server on a port, spawning it only when nothing healthy
is there yet (idempotent `ensure`), and cleaning it up on demand.  It has no
knowledge of *what* the server is — pass the exact shell command that starts it.
Site/server-specific setups (e.g. a repo that must compile its own binary first)
stay in that repo; this script is the reusable lifecycle scaffold they call into.

Subcommands:
    ensure          server healthy -> print base URL; else spawn + wait + print
    start           spawn in foreground (keep-alive; Ctrl-C / SIGTERM stops)
    start --daemon  spawn detached child, wait for health, print base URL
    stop            SIGTERM the daemon child, clean up pid/base files
    status          print running/stopped + base URL

State files (per port):  /tmp/laya-ensure-server-<port>.pid/.base
Server log:              /tmp/laya-ensure-server-<port>.log

No credentials: this script manages a local process only.
"""
from __future__ import annotations

import argparse
import os
import signal
import subprocess
import sys
import time
from pathlib import Path

DEFAULT_PORT = 18766
DEFAULT_HEALTH_PATH = "/healthz"
SPAWN_WAIT = 60.0  # how long to wait for a freshly spawned server to answer


def _paths(port: int) -> tuple[Path, Path, Path]:
    return (
        Path(f"/tmp/laya-ensure-server-{port}.pid"),
        Path(f"/tmp/laya-ensure-server-{port}.base"),
        Path(f"/tmp/laya-ensure-server-{port}.log"),
    )


def _healthy(base: str, health_path: str) -> bool:
    """A server is 'up' when the port answers HTTP at all — 2xx through 5xx all
    count, because a 404 still proves a server is listening and serving.  Only a
    connection refusal / timeout (nothing on the port) means down.  This is a
    liveness probe for orchestration, not an application health check: use
    `--health-path` for a real health endpoint when the server has one."""
    import http.client
    from urllib.parse import urlparse

    try:
        u = urlparse(base.rstrip("/") + (health_path or "/"))
        conn = http.client.HTTPConnection(u.hostname, u.port or 80, timeout=2)
        try:
            conn.request("GET", u.path or "/")
            resp = conn.getresponse()
            resp.read()
            return True  # any HTTP response => the port is serving
        finally:
            conn.close()
    except Exception:  # noqa: BLE001
        return False


def _serve(port: int, command: str, health_path: str) -> int:
    """Foreground server: run `command` in a child, keep alive, clean up."""
    import shlex

    pidf, basef, logf = _paths(port)
    base = f"http://127.0.0.1:{port}"

    def _term(_sig, _frm):  # SIGTERM -> clean context exit (kills server child)
        raise KeyboardInterrupt

    signal.signal(signal.SIGTERM, _term)
    log = open(logf, "a")
    child = subprocess.Popen(shlex.split(command), stdout=log, stderr=log)
    pidf.write_text(str(os.getpid()))
    basef.write_text(base)
    deadline = time.time() + SPAWN_WAIT
    while time.time() < deadline:
        if _healthy(base, health_path):
            print(f"BASE={base}", flush=True)
            break
        if child.poll() is not None:
            print(f"[laya-ensure-server] command exited early ({child.returncode}) — see {logf}",
                  file=sys.stderr)
            return 1
        time.sleep(1)
    else:
        print(f"[laya-ensure-server] timed out waiting for server on :{port} — see {logf}",
              file=sys.stderr)
        return 1
    try:
        while True:
            time.sleep(30)
    except KeyboardInterrupt:
        pass
    finally:
        child.terminate()
        for f in (pidf, basef):
            f.unlink(missing_ok=True)
    return 0


def _daemonize(port: int, command: str, health_path: str) -> int:
    pidf, basef, logf = _paths(port)
    if pidf.exists():
        try:
            os.kill(int(pidf.read_text().strip()), 0)
        except (ProcessLookupError, ValueError):
            pidf.unlink(missing_ok=True)
            basef.unlink(missing_ok=True)
    log = open(logf, "a")
    child = subprocess.Popen(
        [sys.executable, os.path.abspath(__file__), "start",
         "--port", str(port), "--command", command,
         "--health-path", health_path],
        stdout=log, stderr=log, start_new_session=True,
    )
    deadline = time.time() + SPAWN_WAIT
    while time.time() < deadline:
        if basef.exists() and _healthy(basef.read_text().strip(), health_path):
            base = basef.read_text().strip()
            print(f"BASE={base}")
            print(f"[laya-ensure-server] server up (pid {child.pid}) — log {logf}")
            return 0
        if child.poll() is not None:
            print(f"[laya-ensure-server] daemon child exited early ({child.returncode}) — see {logf}",
                  file=sys.stderr)
            return 1
        time.sleep(2)
    print(f"[laya-ensure-server] timed out waiting for server — see {logf}", file=sys.stderr)
    return 1


def _stop(port: int) -> int:
    pidf, basef, logf = _paths(port)
    if not pidf.exists():
        print(f"[laya-ensure-server] no server on :{port} (pid file absent)")
        return 0
    pid = int(pidf.read_text().strip())
    try:
        os.kill(pid, signal.SIGTERM)
    except ProcessLookupError:
        pass
    for _ in range(30):  # up to ~15s
        try:
            os.kill(pid, 0)
            time.sleep(0.5)
        except ProcessLookupError:
            break
    pidf.unlink(missing_ok=True)
    basef.unlink(missing_ok=True)
    print(f"[laya-ensure-server] stopped server pid {pid}")
    return 0


def _status(port: int, health_path: str) -> int:
    pidf, basef, logf = _paths(port)
    if basef.exists() and _healthy(basef.read_text().strip(), health_path):
        print(f"[laya-ensure-server] RUNNING  {basef.read_text().strip()}  (pid "
              f"{pidf.read_text().strip() if pidf.exists() else '?'})")
        return 0
    print(f"[laya-ensure-server] STOPPED  (no healthy server on :{port})")
    return 1


def main() -> int:
    ap = argparse.ArgumentParser(description=__doc__)
    ap.add_argument("cmd", choices=["ensure", "start", "stop", "status"])
    ap.add_argument("--port", type=int, default=DEFAULT_PORT)
    ap.add_argument("--command", default="", help="shell command that starts the server")
    ap.add_argument("--health-path", default=DEFAULT_HEALTH_PATH)
    ap.add_argument("--daemon", action="store_true", help="start in background")
    args = ap.parse_args()

    if args.cmd == "status":
        return _status(args.port, args.health_path)
    if args.cmd == "stop":
        return _stop(args.port)
    if args.cmd == "ensure":
        if _status(args.port, args.health_path) == 0:
            return 0
        if not args.command:
            print("[laya-ensure-server] ensure needs --command to spawn when nothing is running",
                  file=sys.stderr)
            return 1
        return _daemonize(args.port, args.command, args.health_path)
    if args.cmd == "start":
        if not args.command:
            print("[laya-ensure-server] start needs --command", file=sys.stderr)
            return 1
        if args.daemon:
            return _daemonize(args.port, args.command, args.health_path)
        return _serve(args.port, args.command, args.health_path)
    return 1


if __name__ == "__main__":
    raise SystemExit(main())
