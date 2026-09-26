#!/usr/bin/env python3
"""Minimal mock external services for exercising Laya workflow capabilities.

Two servers in one process (used by tests and the DSL demos):

  * POST /score          — a fake "fraud/risk scorer": reads {"text": …} and
                           returns {"risk": 0..1, "label": …}
  * POST /agent          — an app-server-shaped endpoint: accepts a JSON object
                           and replies with {"output": …, "echo": …}; also
                           supports line-delimited stdio mode when invoked with
                           ``--stdio`` so the `agent/stdio` transport can be
                           tested against a codex-style JSON-per-line protocol.

Usage::

    $PYTHON code/laya-tch/bench/mock_server.py --port 8791        # HTTP mode
    $PYTHON code/laya-tch/bench/mock_server.py --stdio            # stdio agent

No third-party dependencies (stdlib only), so it runs anywhere.
"""
from __future__ import annotations

import argparse
import json
import sys
from http.server import BaseHTTPRequestHandler, HTTPServer


def score(text: str) -> dict:
    """Deterministic pseudo-scorer: suspicious keywords raise the risk."""
    t = (text or "").lower()
    hits = [w for w in ("urgent", "refund", "immediately", "scripted", "bulk", "now")
            if w in t]
    risk = min(1.0, 0.12 * len(hits))
    if "refund" in t and "immediately" in t:
        risk = max(risk, 0.75)
    label = "high" if risk >= 0.6 else "medium" if risk >= 0.3 else "low"
    return {"risk": round(risk, 4), "label": label, "hits": hits}


class Handler(BaseHTTPRequestHandler):
    def _send(self, code: int, obj: dict) -> None:
        body = json.dumps(obj).encode()
        self.send_response(code)
        self.send_header("content-type", "application/json")
        self.send_header("content-length", str(len(body)))
        self.end_headers()
        self.wfile.write(body)

    def do_POST(self) -> None:  # noqa: N802 (stdlib naming)
        n = int(self.headers.get("content-length", 0) or 0)
        raw = self.rfile.read(n) if n else b"{}"
        try:
            req = json.loads(raw or b"{}")
        except json.JSONDecodeError:
            self._send(400, {"error": "invalid json"})
            return

        if self.path.startswith("/rpc"):
            # JSON-RPC 2.0 echo: returns the params back under result
            rid = req.get("id")
            method = req.get("method")
            if method == "boom":
                self._send(200, {"jsonrpc": "2.0", "id": rid,
                                 "error": {"code": -32000, "message": "boom requested"}})
            else:
                self._send(200, {"jsonrpc": "2.0", "id": rid,
                                 "result": {"method": method, "params": req.get("params")}})
        elif self.path.startswith("/graphql"):
            q = req.get("query", "")
            if "fail" in q:
                self._send(200, {"errors": [{"message": "graphql failure"}]})
            else:
                self._send(200, {"data": {"echo": req.get("variables", {}), "query": q}})
        elif self.path.startswith("/chat"):
            last = ""
            msgs = req.get("messages") or []
            if msgs:
                last = msgs[-1].get("content", "")
            self._send(200, {
                "choices": [{"message": {"role": "assistant", "content": f"echo:{last}"}}],
                "usage": {"prompt_tokens": len(last), "completion_tokens": 3},
            })
        elif self.path.startswith("/mcp"):
            if (req.get("params") or {}).get("name") == "fail_tool":
                self._send(200, {"jsonrpc": "2.0", "id": req.get("id"),
                                 "error": {"code": -32601, "message": "tool not found"}})
            else:
                self._send(200, {"jsonrpc": "2.0", "id": req.get("id"),
                                 "result": {"content": [{"type": "text",
                                            "text": f"tool:{req.get('params', {}).get('name')}"}]}})
        elif self.path.startswith("/vector"):
            op = "upsert" if "items" in req else "search"
            self._send(200, {"hits": [{"id": "v1", "score": 0.91, "op": op}],
                             "collection": req.get("collection")})
        elif self.path.startswith("/webhook"):
            self._send(200, {"received": True, "event": req.get("event"),
                             "signature": self.headers.get("x-signature")})
        elif self.path.startswith("/score"):
            self._send(200, score(req.get("text", "")))
        elif self.path.startswith("/agent"):
            self._send(200, {
                "output": f"ack:{req.get('session') or 'default'}",
                "echo": req,
                "turn": 1,
            })
        else:
            self._send(404, {"error": "not found", "path": self.path})

    def do_GET(self) -> None:  # noqa: N802
        if self.path.startswith("/health"):
            self._send(200, {"ok": True})
        elif self.path.startswith("/events"):
            # Server-Sent Events: stream a few events then close
            payload = ('event: tick\ndata: {"n": 1}\n\n'
                       'event: tick\ndata: {"n": 2}\n\n'
                       'event: done\ndata: bye\n\n').encode()
            self.send_response(200)
            self.send_header("content-type", "text/event-stream")
            self.send_header("content-length", str(len(payload)))
            self.end_headers()
            self.wfile.write(payload)
        else:
            self._send(404, {"error": "not found"})

    def log_message(self, *a) -> None:  # silence
        return


def run_stdio() -> None:
    """Line-delimited JSON agent: one request object per line, one reply line."""
    for line in sys.stdin:
        line = line.strip()
        if not line:
            continue
        try:
            req = json.loads(line)
        except json.JSONDecodeError:
            print(json.dumps({"error": "invalid json"}))
            sys.stdout.flush()
            continue
        # JSON-RPC-shaped requests get a JSON-RPC reply (MCP tools/call etc.);
        # anything else gets the generic echo used by the `agent` capability.
        if "method" in req:
            if (req.get("params") or {}).get("name") == "fail_tool":
                reply = {"jsonrpc": "2.0", "id": req.get("id"),
                         "error": {"code": -32601, "message": "tool not found"}}
            else:
                name = (req.get("params") or {}).get("name")
                reply = {"jsonrpc": "2.0", "id": req.get("id"),
                         "result": {"content": [{"type": "text", "text": f"stdio-tool:{name}"}]}}
        else:
            reply = {
                "output": f"stdio-ack:{req.get('session') or 'default'}",
                "echo": req,
                "turns": 1,
            }
        print(json.dumps(reply))
        sys.stdout.flush()


def main() -> int:
    ap = argparse.ArgumentParser()
    ap.add_argument("--port", type=int, default=8791)
    ap.add_argument("--host", default="127.0.0.1")
    ap.add_argument("--stdio", action="store_true", help="line-delimited JSON agent on stdio")
    args = ap.parse_args()
    if args.stdio:
        run_stdio()
        return 0
    srv = HTTPServer((args.host, args.port), Handler)
    print(f"mock server on http://{args.host}:{args.port}  (/score, /agent, /health)")
    srv.serve_forever()
    return 0


if __name__ == "__main__":
    sys.exit(main())
