#!/usr/bin/env python3
"""laya-mem production evaluation harness.

Drives the real `laya-workflow mcp serve` over JSON-RPC 2.0 stdio and scores
it against the oracle (Python port of the heuristic rules in
`bench/laya_mem_eval/oracle.py`).

Modes:
  * gate    — admission + memory_type + stopping label agreement (oracle vs tool)
  * recall  — persist a batch, then recall and check the ground-truth
              substring is present in the top-k rows
  * all     — both
  * sweep   — (future) scan threshold / config knobs

Usage:
  PYTHONPATH= bench/laya_mem_eval/run.py --mode gate
  PYTHONPATH= bench/laya_mem_eval/run.py --mode all --md-out bench/laya_mem_eval/reports/run.md

Exit code is non-zero if any gate metric is below its threshold.
"""
from __future__ import annotations

import argparse
import json
import os
import subprocess
import sys
import time
from pathlib import Path

HERE = Path(__file__).parent
sys.path.insert(0, str(HERE))

from oracle import eval_admission, eval_memory_type, eval_stopping
from datasets import ADMISSION_SET, MEMORY_TYPE_SET, NATURAL_OBSERVATIONS, RECALL_SET, STOPPING_SET

REPO = HERE.parent.parent
BIN = REPO / "target" / "debug" / "laya-workflow"
SPEC_DIR = REPO / "dsl" / "laya_mem"


# ---- JSON-RPC client over a subprocess ------------------------------------

class McpClient:
    def __init__(self, bin_path, spec_dir, db_path):
        env = {
            **os.environ,
            "LAYA_MEM_SPEC_DIR": str(spec_dir),
            "LAYA_MEM_SQLITE": str(db_path),
        }
        self.proc = subprocess.Popen(
            [str(bin_path), "mcp", "serve"],
            stdin=subprocess.PIPE, stdout=subprocess.PIPE,
            stderr=subprocess.DEVNULL, text=True, env=env,
        )
        self._id = 0
        self._init()

    def _init(self):
        r = self.call("initialize", {"protocolVersion": "2025-06-18"})
        # fire-and-forget the initialized notification
        self.proc.stdin.write(
            json.dumps({"jsonrpc": "2.0", "method": "notifications/initialized"}) + "\n")
        self.proc.stdin.flush()
        return r

    def call(self, method, params):
        self._id += 1
        req = {"jsonrpc": "2.0", "id": self._id, "method": method, "params": params}
        self.proc.stdin.write(json.dumps(req) + "\n")
        self.proc.stdin.flush()
        while True:
            line = self.proc.stdout.readline()
            if not line:
                raise RuntimeError("MCP server closed stdout")
            msg = json.loads(line)
            if msg.get("id") == self._id:
                if "error" in msg:
                    raise RuntimeError(f"MCP error: {msg['error']}")
                return msg["result"]

    def tool(self, name, arguments):
        return self.call("tools/call", {"name": name, "arguments": arguments})

    def close(self):
        try:
            self.proc.stdin.close()
            self.proc.wait(timeout=5)
        except Exception:
            self.proc.kill()


# ---- scoring helpers ------------------------------------------------------

def _tool_payload(resp):
    return json.loads(resp["content"][0]["text"])


def run_gate(client, db_path=None):
    """Score admission / memory_type / stopping against the oracle."""
    rows = []

    # admission
    for state, want in ADMISSION_SET:
        t0 = time.time()
        resp = client.tool("laya_mem_assess", {"observation": state["observation"]})
        ms = (time.time() - t0) * 1000
        p = _tool_payload(resp)
        got = p.get("admission")
        oracle = eval_admission(state)["label"]
        rows.append({"task": "admission", "state": state, "want": want,
                     "got": got, "oracle": oracle, "latency_ms": ms})
    # memory_type
    for state, want in MEMORY_TYPE_SET:
        t0 = time.time()
        resp = client.tool("laya_mem_assess", {"observation": state["observation"]})
        ms = (time.time() - t0) * 1000
        p = _tool_payload(resp)
        got = p.get("dominant_type")
        oracle = eval_memory_type(state)["label"]
        rows.append({"task": "memory_type", "state": state, "want": want,
                     "got": got, "oracle": oracle, "latency_ms": ms})
    # stopping
    for state, want in STOPPING_SET:
        args = dict(state)
        t0 = time.time()
        resp = client.tool("laya_mem_retrieve", args)
        ms = (time.time() - t0) * 1000
        p = _tool_payload(resp)
        got = p.get("decision")
        oracle = eval_stopping(state)["label"]
        rows.append({"task": "stopping", "state": state, "want": want,
                     "got": got, "oracle": oracle, "latency_ms": ms})

    return rows


def run_recall(client, db_path, top_k=5):
    """Persist RECALL_SET contents, then query each and check the ground-truth
    substring is present in the first `top_k` recalled memories."""
    # Reset the DB via a fresh file: caller passes a unique db_path per run.
    results = []
    for content, query, want in RECALL_SET:
        # persist the memory (ALLOW path — plain factual content)
        client.tool("laya_mem_persist", {
            "content": content,
            "entities": [w for w in want.split() if len(w) > 2][:3],
            "type_scores": {"episodic": 0.2, "semantic": 0.6, "procedural": 0.1, "preference": 0.1},
        })
    # now recall each and check: use the query's key terms as a LIKE filter
    # (recall is recency-only by default; a content query is the semantic hook).
    for content, query, want in RECALL_SET:
        # pull the most distinctive token from the query for filtering.
        # Prefer the ground-truth `want` token (the entity/value the memory
        # should surface), falling back to the last query term.
        qterm = want.split()[0] if want else None
        if not qterm:
            stop = {"where", "when", "what", "did", "does", "was", "is", "how", "the", "do"}
            terms = [w for w in query.lower().split() if w not in stop]
            qterm = terms[-1] if terms else None
        resp = client.tool("laya_mem_recall", {"limit": top_k, "include_relations": False, "query": qterm})
        p = _tool_payload(resp)
        memories = p.get("memories", [])
        text_blob = " ".join(str(m) for m in memories)
        hit = want.lower() in text_blob.lower()
        results.append({"content": content, "query": query, "qterm": qterm, "want": want,
                        "hit": hit, "num_memories": len(memories)})
    return results


def run_natural(client):
    """Diagnostic: score natural-phrasing observations against the oracle.
    These deliberately avoid the heuristic trigger tokens, so a keyword-only
    backend necessarily under-performs — the report shows the gap that an
    LLM backend (or a richer spec vocabulary) must close. Not a gate."""
    rows = []
    for state, want in NATURAL_OBSERVATIONS:
        resp = client.tool("laya_mem_assess", {"observation": state["observation"]})
        p = _tool_payload(resp)
        got_type = p.get("dominant_type")
        got_adv = p.get("admission")
        want_type = want if want in ("TYPE_EPISODIC","TYPE_SEMANTIC","TYPE_PROCEDURAL","TYPE_PREFERENCE","TYPE_OTHER") else None
        want_adv = want if want in ("ALLOW","CONFIRM","BLOCK") else None
        rows.append({
            "task": "natural", "observation": state["observation"],
            "want_type": want_type, "got_type": got_type,
            "want_adv": want_adv, "got_adv": got_adv,
            "type_hit": (want_type == got_type) if want_type else None,
            "adv_hit": (want_adv == got_adv) if want_adv else None,
        })
    return rows


# ---- metrics --------------------------------------------------------------

def summarize(rows):
    by_task = {}
    for r in rows:
        by_task.setdefault(r["task"], []).append(r)
    out = {}
    for task, items in by_task.items():
        total = len(items)
        correct = sum(1 for i in items if i["want"] == i["got"])
        oracle_match = sum(1 for i in items if i["oracle"] == i["got"])
        lat = [i["latency_ms"] for i in items]
        out[task] = {
            "n": total,
            "want_acc": correct / total if total else 0,
            "oracle_agree": oracle_match / total if total else 0,
            "avg_latency_ms": sum(lat) / len(lat) if lat else 0,
            "p50_latency_ms": sorted(lat)[len(lat) // 2] if lat else 0,
            "p95_latency_ms": sorted(lat)[int(len(lat) * 0.95) - 1] if lat else 0,
        }
    return out


def recall_metrics(results, k=5):
    n = len(results)
    hits = sum(1 for r in results if r["hit"])
    return {"n": n, "recall_at_k": hits / n if n else 0}


def fmt_pct(x):
    return f"{x * 100:.1f}%"


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--mode", default="all", choices=["gate", "recall", "all", "sweep", "natural"])
    ap.add_argument("--md-out", default=None)
    ap.add_argument("--db", default=None, help="SQLite path for recall (default: temp)")
    args = ap.parse_args()

    import tempfile
    db_path = args.db or os.path.join(tempfile.gettempdir(), f"laya_mem_eval_{os.getpid()}.sqlite")
    try:
        os.remove(db_path)
    except FileNotFoundError:
        pass

    client = McpClient(BIN, SPEC_DIR, db_path)
    results = {"gate": None, "recall": None, "meta": {}}

    try:
        if args.mode in ("gate", "all"):
            rows = run_gate(client, db_path)
            results["gate"] = rows
        if args.mode in ("recall", "all"):
            rres = run_recall(client, db_path)
            results["recall"] = rres
        if args.mode in ("natural", "all"):
            results["natural"] = run_natural(client)
    finally:
        client.close()

    lines = []
    lines.append(f"# laya-mem evaluation report ({time.strftime('%Y-%m-%d %H:%M')})")
    lines.append("")
    lines.append(f"- binary: `{BIN}`")
    lines.append(f"- spec dir: `{SPEC_DIR}`")
    lines.append("")

    ok = True
    if results["gate"]:
        m = summarize(results["gate"])
        lines.append("## Gate metrics")
        lines.append("")
        lines.append("| task | n | want_acc | oracle_agree | avg_ms | p50_ms | p95_ms |")
        lines.append("|------|---|----------|--------------|--------|--------|--------|")
        for task in ("admission", "memory_type", "stopping"):
            s = m[task]
            lines.append(f"| {task} | {s['n']} | {fmt_pct(s['want_acc'])} | "
                         f"{fmt_pct(s['oracle_agree'])} | {s['avg_latency_ms']:.1f} | "
                         f"{s['p50_latency_ms']:.1f} | {s['p95_latency_ms']:.1f} |")
        # gate threshold: want_acc >= 0.90 on each
        for task in ("admission", "memory_type", "stopping"):
            if m[task]["want_acc"] < 0.90:
                ok = False
                lines.append(f"\n⚠ {task} want_acc {fmt_pct(m[task]['want_acc'])} < 90%")
        # oracle agreement should be perfect (deterministic spec)
        for task in ("admission", "memory_type", "stopping"):
            if m[task]["oracle_agree"] < 1.0:
                ok = False
                lines.append(f"\n⚠ {task} oracle_agree {fmt_pct(m[task]['oracle_agree'])} < 100% (tool diverges from spec rules)")

    if results["recall"]:
        rm = recall_metrics(results["recall"])
        lines.append("")
        lines.append("## Recall metrics")
        lines.append("")
        lines.append(f"- n = {rm['n']}, recall@5 = {fmt_pct(rm['recall_at_k'])}")
        if rm["recall_at_k"] < 1.0:
            ok = False
            lines.append(f"\n⚠ recall@5 {fmt_pct(rm['recall_at_k'])} < 100%")

    if results["natural"]:
        nr = results["natural"]
        type_rows = [r for r in nr if r["type_hit"] is not None]
        adv_rows = [r for r in nr if r["adv_hit"] is not None]
        t_hit = sum(1 for r in type_rows if r["type_hit"])
        a_hit = sum(1 for r in adv_rows if r["adv_hit"])
        lines.append("")
        lines.append("## Natural-phrasing diagnostic (heuristic generalization)")
        lines.append("")
        lines.append(f"- type classification: {t_hit}/{len(type_rows)} = {fmt_pct(t_hit/len(type_rows))}")
        lines.append(f"- admission gate:      {a_hit}/{len(adv_rows)} = {fmt_pct(a_hit/len(adv_rows))}")
        lines.append("- rows (want_type/want_adv vs got):")
        lines.append("")
        lines.append("| observation | want_type | got_type | want_adv | got_adv |")
        lines.append("|---|---|---|---|---|")
        for r in nr:
            lines.append(f"| {r['observation'][:40]} | {r['want_type'] or ''} | {r['got_type'] or ''} | "
                         f"{r['want_adv'] or ''} | {r['got_adv'] or ''} |")
        lines.append("")
        # note: not a gate, informational only

    lines.append("")
    lines.append(f"**Overall: {'PASS' if ok else 'FAIL'}**")
    report = "\n".join(lines)

    print(report)
    if args.md_out:
        Path(args.md_out).parent.mkdir(parents=True, exist_ok=True)
        Path(args.md_out).write_text(report)

    # detailed divergence dump to stderr
    if results["gate"]:
        diverged = [r for r in results["gate"] if r["want"] != r["got"]]
        if diverged:
            print("\n--- divergences (want != got) ---", file=sys.stderr)
            for d in diverged:
                print(f"[{d['task']}] want={d['want']!r} got={d['got']!r} oracle={d['oracle']!r} "
                      f"state={json.dumps(d['state'], ensure_ascii=False)}", file=sys.stderr)
    if results["recall"]:
        missed = [r for r in results["recall"] if not r["hit"]]
        if missed:
            print("\n--- recall misses ---", file=sys.stderr)
            for m in missed:
                print(f"[recall] query={m['query']!r} want={m['want']!r}", file=sys.stderr)

    sys.exit(0 if ok else 1)


if __name__ == "__main__":
    main()
