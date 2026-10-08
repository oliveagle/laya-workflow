#!/usr/bin/env bash
# mock_services.sh — start/stop/status for the local protocol mocks.
#
# Why this exists: running `laya-workflow mock serve ... &` from a shell that
# then exits leaves the child tied to that shell's process group, so it dies
# with the shell. This launcher detaches the server into its own session, waits
# for the `{"ready": true, ...}` line before returning, and records a PID file
# so the process can be stopped or inspected later. It replaces the old
# laya-workflow mock serve (the Rust binary now owns the mocks).
#
# Usage:
#   bench/mock_services.sh start [--redis 6380 --nats 4223 ...]
#   bench/mock_services.sh stop
#   bench/mock_services.sh status
#
# With no ports given, `start` brings up the full default set used by the
# tests and DSL demos.
set -euo pipefail

HERE="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
# Prefer an explicit binary (CI builds target/debug), else the release build,
# else whatever `laya-workflow` resolves to on PATH.
BIN="${LAYA_WORKFLOW_BIN:-}"
if [ -z "$BIN" ] && [ -x "$HERE/../target/release/laya-workflow" ]; then
    BIN="$HERE/../target/release/laya-workflow"
fi
if [ -z "$BIN" ]; then
    BIN="$(command -v laya-workflow || true)"
fi
if [ -z "$BIN" ] || [ ! -x "$BIN" ]; then
    echo "laya-workflow binary not found (set LAYA_WORKFLOW_BIN)" >&2
    exit 2
fi
RUNDIR="${LAYA_MOCK_RUNDIR:-$HOME/tmp/laya-mocks}"
PIDFILE="$RUNDIR/mock_services.pid"
PORTFILE="$RUNDIR/mock_services.ports"
LOGFILE="$RUNDIR/mock_services.log"
DEFAULT_PORTS=(--redis 6380 --nats 4223 --mqtt 1884 --smtp 2526 \
               --s3 9000 --prom 9091 --kafka 8083 --udp 9999 --web 8793)

port_open() {  # tcp only; udp is verified via the ready line
    local port="$1"
    timeout 0.5 bash -c "exec 3<>/dev/tcp/127.0.0.1/$port" 2>/dev/null
}

cmd_start() {
    mkdir -p "$RUNDIR"
    if [ -f "$PIDFILE" ] && kill -0 "$(cat "$PIDFILE")" 2>/dev/null; then
        echo "mock services already running (pid $(cat "$PIDFILE"))"
        cmd_status
        return 0
    fi
    local args=("$@")
    if [ "${#args[@]}" -eq 0 ]; then args=("${DEFAULT_PORTS[@]}"); fi

    # Fresh log per start so the readiness grep cannot match a stale line.
    : >"$LOGFILE"
    # Detach into a new session so it survives the calling shell, then wait for
    # the server's own readiness line instead of sleeping an arbitrary amount.
    setsid "$BIN" mock serve "${args[@]}" \
        >>"$LOGFILE" 2>&1 &
    local pid=$!
    echo "$pid" >"$PIDFILE"
    # Remember which TCP ports this start requested, so `status` reports the
    # truth instead of a hard-coded default list.
    : >"$PORTFILE"
    local i=0
    while [ "$i" -lt "${#args[@]}" ]; do
        if [ "${args[$i]}" = "--udp" ] || [ "${args[$i]}" = "--web" ] || \
           [ "${args[$i]}" = "--redis" ] || [ "${args[$i]}" = "--nats" ] || \
           [ "${args[$i]}" = "--mqtt" ] || [ "${args[$i]}" = "--smtp" ] || \
           [ "${args[$i]}" = "--s3" ] || [ "${args[$i]}" = "--prom" ] || \
           [ "${args[$i]}" = "--kafka" ] || [ "${args[$i]}" = "--score" ] || \
           [ "${args[$i]}" = "--agent" ] || [ "${args[$i]}" = "--rpc" ] || \
           [ "${args[$i]}" = "--graphql" ] || [ "${args[$i]}" = "--chat" ] || \
           [ "${args[$i]}" = "--mcp" ] || [ "${args[$i]}" = "--vector" ] || \
           [ "${args[$i]}" = "--webhook" ]; then
            i=$((i + 1))
            [ "$i" -lt "${#args[@]}" ] && echo "${args[$i]}" >>"$PORTFILE"
        fi
        i=$((i + 1))
    done

    local waited=0
    while [ "$waited" -lt 150 ]; do
        if ! kill -0 "$pid" 2>/dev/null; then
            echo "mock services exited during startup; log tail:" >&2
            tail -5 "$LOGFILE" >&2 || true
            rm -f "$PIDFILE"
            return 1
        fi
        if grep -q '"ready":true' "$LOGFILE" 2>/dev/null; then
            echo "mock services ready (pid $pid)"
            cmd_status
            return 0
        fi
        if grep -q '"ready":false' "$LOGFILE" 2>/dev/null; then
            echo "mock services reported a startup failure; log tail:" >&2
            tail -5 "$LOGFILE" >&2 || true
            kill "$pid" 2>/dev/null || true
            rm -f "$PIDFILE"
            return 1
        fi
        sleep 0.2
        waited=$((waited + 1))
    done
    echo "mock services did not become ready within 30s; log tail:" >&2
    tail -5 "$LOGFILE" >&2 || true
    return 1
}

cmd_stop() {
    if [ ! -f "$PIDFILE" ]; then
        echo "no pid file; nothing to stop"
        return 0
    fi
    local pid; pid="$(cat "$PIDFILE")"
    if kill -0 "$pid" 2>/dev/null; then
        # Kill the whole session (the pid is the session leader thanks to setsid).
        kill -TERM "-$pid" 2>/dev/null || kill -TERM "$pid" 2>/dev/null || true
        local waited=0
        while kill -0 "$pid" 2>/dev/null && [ "$waited" -lt 25 ]; do
            sleep 0.2; waited=$((waited + 1))
        done
        kill -KILL "-$pid" 2>/dev/null || true
    fi
    rm -f "$PIDFILE"
    echo "mock services stopped"
}

cmd_status() {
    local pid=""
    [ -f "$PIDFILE" ] && pid="$(cat "$PIDFILE")"
    if [ -n "$pid" ] && kill -0 "$pid" 2>/dev/null; then
        echo "running: pid $pid"
    else
        echo "running: no"
    fi
    local ports=()
    if [ -f "$PORTFILE" ]; then
        while read -r p; do [ -n "$p" ] && ports+=("$p"); done <"$PORTFILE"
    fi
    if [ "${#ports[@]}" -eq 0 ]; then
        ports=(6380 4223 1884 2526 9000 9091 8083 8793)
    fi
    for p in "${ports[@]}"; do
        if port_open "$p"; then echo "  tcp $p: up"; else echo "  tcp $p: down"; fi
    done
}

case "${1:-}" in
    start) shift; cmd_start "$@" ;;
    stop) cmd_stop ;;
    status) cmd_status ;;
    *) echo "usage: $0 {start|stop|status} [--redis PORT ...]" >&2; exit 2 ;;
esac
