#!/usr/bin/env python3
"""Local mock servers for the protocol/service capabilities (stdlib only).

Starts any of these on demand (used by tests and DSL demos so nothing touches
the public network):

  --redis PORT     minimal RESP2 server: PING / GET / SET / DEL / INCR / TTL / KEYS
  --nats PORT      NATS text protocol: sends INFO, accepts PUB and echoes +OK
  --mqtt PORT      MQTT 3.1.1: CONNACK on CONNECT, swallows PUBLISH
  --smtp PORT      SMTP session: 220/250/354/221 flow (AUTH LOGIN supported)
  --s3 PORT        S3-ish HTTP: PUT/GET/DELETE object, GET ?list-type=2 XML
  --prom PORT      Prometheus text exposition endpoint
  --kafka PORT     Confluent-style REST proxy POST /topics/<t>
  --udp PORT       UDP echo server
  --web PORT       web research: /search?q=... (JSON results), /page?name=...
                   (HTML), /redirect (to prove redirects are not followed)

A JSON status line is printed once every requested listener is bound, so a
launcher can wait for readiness instead of sleeping and hoping::

    {"ready": true, "listeners": {"redis": 6380, ...}}

The process stays in the foreground (it is meant to be supervised). Use
`bench/mock_services.sh start|stop|status` to run it in the background.

Examples::

    $PYTHON code/laya-tch/bench/mock_services.py --redis 6380 --nats 4223
    $PYTHON code/laya-tch/bench/mock_services.py --smtp 2525 --s3 9000
"""
from __future__ import annotations

import argparse
import json
import socket
import sys
import threading
import time
import urllib.parse
from http.server import BaseHTTPRequestHandler, HTTPServer

# ── shared helpers ──────────────────────────────────────────────────


def bind_tcp(port: int, *, retries: int = 40, delay: float = 0.25) -> socket.socket:
    """Bind a listening TCP socket on 127.0.0.1, retrying briefly.

    A previous run that exited moments ago can leave the port in TIME_WAIT, and
    a concurrent test can still be shutting its listener down. Failing hard on
    the first EADDRINUSE would make the whole set flaky, so retry and only then
    raise — the caller turns that into a non-zero exit instead of a dead thread.
    """
    last: OSError | None = None
    for _ in range(max(1, retries)):
        srv = socket.socket(socket.AF_INET, socket.SOCK_STREAM)
        srv.setsockopt(socket.SOL_SOCKET, socket.SO_REUSEADDR, 1)
        try:
            srv.bind(("127.0.0.1", port))
            srv.listen(16)
            return srv
        except OSError as e:  # port busy / not yet released
            last = e
            srv.close()
            time.sleep(delay)
    raise OSError(f"cannot bind 127.0.0.1:{port} after {retries} tries: {last}")


def _bind_udp_retry(s: socket.socket, port: int, *, retries: int = 40, delay: float = 0.25) -> None:
    """Same retry discipline as `bind_tcp`, for the UDP echo listener."""
    last: OSError | None = None
    for _ in range(max(1, retries)):
        try:
            s.bind(("127.0.0.1", port))
            return
        except OSError as e:
            last = e
            time.sleep(delay)
    raise OSError(f"cannot bind udp 127.0.0.1:{port} after {retries} tries: {last}")

# ── TCP protocol servers ────────────────────────────────────────────


def serve_redis(port: int) -> None:
    store: dict[bytes, bytes] = {}

    def handle(conn: socket.socket) -> None:
        buf = b""
        while True:
            try:
                data = conn.recv(4096)
            except OSError:
                return
            if not data:
                return
            buf += data
            while b"\r\n" in buf:
                # parse one RESP array: *N $len val ...
                if not buf.startswith(b"*"):
                    buf = b""
                    break
                end = buf.find(b"\r\n")
                n = int(buf[1:end])
                rest = buf[end + 2:]
                args = []
                ok = True
                for _ in range(n):
                    if not rest.startswith(b"$"):
                        ok = False
                        break
                    e2 = rest.find(b"\r\n")
                    ln = int(rest[1:e2])
                    val = rest[e2 + 2:e2 + 2 + ln]
                    args.append(val)
                    rest = rest[e2 + 2 + ln + 2:]
                if not ok:
                    break
                buf = rest
                cmd = (args[0].upper() if args else b"")
                if cmd == b"PING":
                    conn.sendall(b"+PONG\r\n")
                elif cmd == b"SET" and len(args) >= 3:
                    store[args[1]] = args[2]
                    conn.sendall(b"+OK\r\n")
                elif cmd == b"GET" and len(args) >= 2:
                    v = store.get(args[1])
                    if v is None:
                        conn.sendall(b"$-1\r\n")
                    else:
                        conn.sendall(b"$%d\r\n%s\r\n" % (len(v), v))
                elif cmd == b"DEL" and len(args) >= 2:
                    conn.sendall(b":%d\r\n" % (1 if store.pop(args[1], None) is not None else 0))
                elif cmd == b"INCR" and len(args) >= 2:
                    cur = int(store.get(args[1], b"0"))
                    store[args[1]] = str(cur + 1).encode()
                    conn.sendall(b":%d\r\n" % (cur + 1))
                elif cmd == b"TTL":
                    conn.sendall(b":-1\r\n")
                elif cmd == b"KEYS":
                    keys = list(store.keys())
                    conn.sendall(b"*%d\r\n" % len(keys))
                    for k in keys:
                        conn.sendall(b"$%d\r\n%s\r\n" % (len(k), k))
                elif cmd == b"AUTH":
                    conn.sendall(b"+OK\r\n")
                else:
                    conn.sendall(b"-ERR unknown command\r\n")

    srv = bind_tcp(port)
    print(f"redis mock on 127.0.0.1:{port}", flush=True)
    while True:
        c, _ = srv.accept()
        threading.Thread(target=handle, args=(c,), daemon=True).start()


def serve_nats(port: int) -> None:
    def handle(conn: socket.socket) -> None:
        conn.sendall(b'INFO {"server_id":"mock","version":"2.10.0"}\r\n')
        while True:
            try:
                data = conn.recv(4096)
            except OSError:
                return
            if not data:
                return
            if data.startswith(b"PUB") or data.startswith(b"SUB") or data.startswith(b"CONNECT"):
                conn.sendall(b"+OK\r\n")
            elif data.startswith(b"PING"):
                conn.sendall(b"PONG\r\n")

    srv = bind_tcp(port)
    print(f"nats mock on 127.0.0.1:{port}", flush=True)
    while True:
        c, _ = srv.accept()
        threading.Thread(target=handle, args=(c,), daemon=True).start()


def serve_mqtt(port: int) -> None:
    def handle(conn: socket.socket) -> None:
        while True:
            try:
                data = conn.recv(4096)
            except OSError:
                return
            if not data:
                return
            ptype = data[0] >> 4
            if ptype == 1:  # CONNECT -> CONNACK (accepted)
                conn.sendall(bytes([0x20, 0x02, 0x00, 0x00]))
            elif ptype == 12:  # PINGREQ -> PINGRESP
                conn.sendall(bytes([0xD0, 0x00]))
            # PUBLISH / DISCONNECT need no reply at QoS 0

    srv = bind_tcp(port)
    print(f"mqtt mock on 127.0.0.1:{port}", flush=True)
    while True:
        c, _ = srv.accept()
        threading.Thread(target=handle, args=(c,), daemon=True).start()


def serve_smtp(port: int) -> None:
    def handle(conn: socket.socket) -> None:
        conn.sendall(b"220 mock ESMTP\r\n")
        in_data = False
        auth_stage = 0  # 1 = expecting base64 username, 2 = expecting base64 password
        while True:
            try:
                data = conn.recv(4096)
            except OSError:
                return
            if not data:
                return
            text = data.decode(errors="replace")
            if in_data:
                if "\r\n.\r\n" in text or text.strip() == ".":
                    in_data = False
                    conn.sendall(b"250 OK queued\r\n")
                continue
            for line in text.splitlines():
                cmd = line.strip().upper()
                # AUTH LOGIN continuation lines are base64 blobs, not commands.
                # Answer stage 1 with the password challenge and stage 2 with
                # success; otherwise the session desynchronises and the client
                # waits forever for a reply that never comes.
                if auth_stage == 1:
                    auth_stage = 2
                    conn.sendall(b"334 UGFzc3dvcmQ6\r\n")
                    continue
                if auth_stage == 2:
                    auth_stage = 0
                    conn.sendall(b"235 Authentication successful\r\n")
                    continue
                if cmd.startswith("EHLO") or cmd.startswith("HELO"):
                    conn.sendall(b"250-mock\r\n250 SIZE 10485760\r\n")
                elif cmd.startswith("AUTH LOGIN"):
                    auth_stage = 1
                    conn.sendall(b"334 VXNlcm5hbWU6\r\n")
                elif cmd == "MAIL FROM" or cmd.startswith("MAIL FROM"):
                    conn.sendall(b"250 OK\r\n")
                elif cmd.startswith("RCPT TO"):
                    conn.sendall(b"250 OK\r\n")
                elif cmd == "DATA":
                    in_data = True
                    conn.sendall(b"354 End data with <CR><LF>.<CR><LF>\r\n")
                elif cmd == "QUIT":
                    conn.sendall(b"221 Bye\r\n")
                    return

    srv = bind_tcp(port)
    print(f"smtp mock on 127.0.0.1:{port}", flush=True)
    while True:
        c, _ = srv.accept()
        threading.Thread(target=handle, args=(c,), daemon=True).start()


def serve_udp(port: int) -> None:
    s = socket.socket(socket.AF_INET, socket.SOCK_DGRAM)
    s.setsockopt(socket.SOL_SOCKET, socket.SO_REUSEADDR, 1)
    _bind_udp_retry(s, port)
    print(f"udp echo mock on 127.0.0.1:{port}", flush=True)
    while True:
        data, addr = s.recvfrom(4096)
        s.sendto(b"echo:" + data, addr)


# ── HTTP services ───────────────────────────────────────────────────


class _WebHandler(BaseHTTPRequestHandler):
    """Search + fetch endpoints for the `web_search` / `web_fetch` capabilities.

    `/search?q=...` returns a JSON envelope with a `results` array, and
    `/page?name=...` returns a small HTML document. Both are deterministic so
    tests never need the public internet.
    """

    documents = {
        "guide": (
            "<title>Deployment Guide</title>"
            "<h1>Deployment Guide</h1>"
            "<p>Set the token before starting &amp; verify with <code>--check</code>.</p>"
            "<ul><li>Step one</li><li>Step two</li></ul>"
            "<p>Read the <a href=\"/page?name=notes\">release notes</a> for details.</p>"
            "<script>var leak = 'should-not-appear';</script>"
        ),
        "notes": (
            "<title>Release Notes</title>"
            "<h2>Changes</h2><p>Fixed the SMTP reader.</p>"
        ),
    }

    def _send(self, code: int, body: bytes, ctype: str = "text/plain") -> None:
        self.send_response(code)
        self.send_header("content-type", ctype)
        self.send_header("content-length", str(len(body)))
        self.end_headers()
        self.wfile.write(body)

    def do_GET(self) -> None:  # noqa: N802
        path, _, qs = self.path.partition("?")
        params = {}
        for pair in qs.split("&"):
            if "=" in pair:
                k, _, v = pair.partition("=")
                params[k] = urllib.parse.unquote_plus(v)

        if path.rstrip("/").endswith("/search"):
            q = params.get("q", "")
            if not q:
                self._send(400, b'{"error":"missing q"}', "application/json")
                return
            body = json.dumps({
                "query": q,
                "results": [
                    {"title": f"{q} overview", "url": f"http://127.0.0.1:{self.server.server_port}/page?name=guide",
                     "snippet": f"Overview of {q}"},
                    {"title": f"{q} release notes", "url": f"http://127.0.0.1:{self.server.server_port}/page?name=notes",
                     "snippet": f"Notes mentioning {q}"},
                ],
            }).encode()
            self._send(200, body, "application/json")
            return

        if path.rstrip("/").endswith("/page"):
            name = params.get("name", "")
            if name == "large":
                self._send(200, b"x" * 8192, "text/plain")
                return
            doc = _WebHandler.documents.get(name)
            if doc is None:
                self._send(404, b"<title>Not Found</title><p>no such page</p>", "text/html")
                return
            self._send(200, doc.encode(), "text/html; charset=utf-8")
            return

        if path.rstrip("/").endswith("/redirect"):
            self.send_response(302)
            self.send_header("location", params.get("to", "/page?name=guide"))
            self.send_header("content-length", "0")
            self.end_headers()
            return

        self._send(404, b"not found", "text/plain")

    def log_message(self, *a) -> None:
        return


class _S3Handler(BaseHTTPRequestHandler):
    objects: dict[str, bytes] = {}

    def _send(self, code: int, body: bytes, ctype: str = "application/xml") -> None:
        self.send_response(code)
        self.send_header("content-type", ctype)
        self.send_header("content-length", str(len(body)))
        self.end_headers()
        self.wfile.write(body)

    def do_PUT(self) -> None:  # noqa: N802
        n = int(self.headers.get("content-length", 0) or 0)
        _S3Handler.objects[self.path] = self.rfile.read(n)
        self._send(200, b"")

    def do_GET(self) -> None:  # noqa: N802
        if self.path.rstrip("/").endswith("bucket") or "prefix=" in self.path or self.path.endswith("/bucket/"):
            keys = "".join(
                f"<Contents><Key>{k.lstrip('/').split('/', 2)[-1]}</Key></Contents>"
                for k in sorted(_S3Handler.objects)
            )
            self._send(200, f"<ListBucketResult>{keys}</ListBucketResult>".encode())
            return
        v = _S3Handler.objects.get(self.path)
        if v is None:
            self._send(404, b"<Error>NoSuchKey</Error>")
        else:
            self._send(200, v, "application/octet-stream")

    def do_DELETE(self) -> None:  # noqa: N802
        _S3Handler.objects.pop(self.path, None)
        self._send(204, b"")

    def log_message(self, *a) -> None:
        return


class _PromHandler(BaseHTTPRequestHandler):
    def do_GET(self) -> None:  # noqa: N802
        if self.path.startswith("/api/v1/query"):
            body = json.dumps({"status": "success", "data": {"resultType": "vector", "result": []}}).encode()
            self.send_response(200)
            self.send_header("content-type", "application/json")
        else:
            body = (b"# HELP mock_metric a mock gauge\n"
                    b"# TYPE mock_metric gauge\n"
                    b'mock_metric{job="a"} 1.5\n'
                    b'mock_metric{job="b"} 2.5\n'
                    b'mock_up 1\n')
            self.send_response(200)
            self.send_header("content-type", "text/plain")
        self.send_header("content-length", str(len(body)))
        self.end_headers()
        self.wfile.write(body)

    def log_message(self, *a) -> None:
        return


class _KafkaHandler(BaseHTTPRequestHandler):
    def do_POST(self) -> None:  # noqa: N802
        n = int(self.headers.get("content-length", 0) or 0)
        self.rfile.read(n)
        topic = self.path.rsplit("/", 1)[-1]
        body = json.dumps({"offsets": [{"partition": 0, "offset": 1, "topic": topic}]}).encode()
        self.send_response(200)
        self.send_header("content-type", "application/json")
        self.send_header("content-length", str(len(body)))
        self.end_headers()
        self.wfile.write(body)

    def log_message(self, *a) -> None:
        return


def serve_http(port: int, handler) -> None:
    class _Server(HTTPServer):
        allow_reuse_address = True

    srv = _Server(("127.0.0.1", port), handler)
    print(f"http mock on 127.0.0.1:{port} ({handler.__name__})", flush=True)
    srv.serve_forever()


def main() -> int:
    ap = argparse.ArgumentParser()
    for flag in ("redis", "nats", "mqtt", "smtp", "s3", "prom", "kafka", "udp", "web"):
        ap.add_argument(f"--{flag}", type=int, default=0)
    args = ap.parse_args()

    specs = [
        ("redis", args.redis, serve_redis, True),
        ("nats", args.nats, serve_nats, True),
        ("mqtt", args.mqtt, serve_mqtt, True),
        ("smtp", args.smtp, serve_smtp, True),
        ("udp", args.udp, serve_udp, False),
        ("s3", args.s3, lambda p: serve_http(p, _S3Handler), True),
        ("prom", args.prom, lambda p: serve_http(p, _PromHandler), True),
        ("kafka", args.kafka, lambda p: serve_http(p, _KafkaHandler), True),
        ("web", args.web, lambda p: serve_http(p, _WebHandler), True),
    ]
    wanted = [(name, port, fn, tcp) for name, port, fn, tcp in specs if port]

    if not wanted:
        print("nothing to serve; pass at least one --<service> PORT", file=sys.stderr)
        return 2

    errors: list[str] = []
    started: dict[str, int] = {}

    def launch(name: str, port: int, fn) -> None:
        try:
            fn(port)
        except Exception as e:  # bind failure etc. — record instead of vanishing
            errors.append(f"{name}:{port}: {e}")

    for name, port, fn, _tcp in wanted:
        threading.Thread(target=launch, args=(name, port, fn), daemon=True).start()
        started[name] = port

    # Wait until every requested listener answers on its port. This is what
    # makes the process safe to use from a launcher: once the ready line is
    # printed, clients are guaranteed to be able to connect. UDP has no connect
    # handshake, so it is probed by being sendable (a datagram round-trip).
    checks = {name: (port, tcp) for name, port, _fn, tcp in wanted}
    deadline = time.time() + 10
    pending = dict(started)
    while pending and time.time() < deadline:
        if errors:
            print(json.dumps({"ready": False, "error": errors[0]}), flush=True)
            return 1
        for name in list(pending):
            port, tcp = checks[name]
            if _listener_ready(port, tcp):
                pending.pop(name)
        if pending:
            time.sleep(0.05)

    if errors:
        print(json.dumps({"ready": False, "error": errors[0]}), flush=True)
        return 1
    if pending:
        print(json.dumps({"ready": False, "error": f"listeners not up: {sorted(pending)}"}), flush=True)
        return 1

    print(json.dumps({"ready": True, "listeners": started}, sort_keys=True), flush=True)
    threading.Event().wait()
    return 0


def _listener_ready(port: int, tcp: bool) -> bool:
    """True once the listener accepts traffic (TCP connect or UDP round-trip)."""
    if tcp:
        try:
            with socket.create_connection(("127.0.0.1", port), 0.2):
                return True
        except OSError:
            return False
    try:
        with socket.socket(socket.AF_INET, socket.SOCK_DGRAM) as u:
            u.settimeout(0.2)
            u.sendto(b"ping", ("127.0.0.1", port))
            u.recvfrom(64)
        return True
    except OSError:
        return False


if __name__ == "__main__":
    sys.exit(main())
