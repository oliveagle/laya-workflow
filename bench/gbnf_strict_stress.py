#!/usr/bin/env python3
"""Stress / mutation harness for `laya-tch --gbnf-strict`.

Generates a corpus of random /system_one-shaped payloads that exercise the
rejection + answer-set invariant surface of the GBNF gate, hits a running
laya-tch with each one, and reports:

  * status-code distribution (200 / 400 / 500)
  * rejection-position distribution (where parse failed, bucketed)
  * in-gate invariant violations (choice == "", score outside legend, prob sum drift)
  * latency p50 / p95 / max

Run::

    $PYTHON bench/gbnf_strict_stress.py --base-url http://127.0.0.1:8400
    $PYTHON bench/gbnf_strict_stress.py --base-url http://127.0.0.1:8400 --out bench/reports/gbnf_strict.md

Designed to run against any /v1/systemone endpoint; the binary has to expose
the same JSON shape (model, answers, ...).

The point isn't to find crashes (a small random corpus won't) — it's to
record the *shape* of the rejection surface so a future refactor that
regresses any of the four invariants will be caught immediately.
"""
from __future__ import annotations

import argparse
import json
import random
import statistics
import sys
import time
import urllib.error
import urllib.request
from collections import Counter
from pathlib import Path

REPO = Path(__file__).resolve().parents[1]

VALID_QTYPES = ("choice", "score", "noul")

# ---------------------------------------------------------------------------
# 1. Corpus generation. Each payload is tagged with a mutation class so we
#    can report per-class breakdown.
# ---------------------------------------------------------------------------

REALISTIC_BASE = {
    "state": "I was charged twice for March. Please refund the duplicate ASAP.",
    "questions": {
        "urgency": {
            "type": "score",
            "instructions": "How urgent is this?",
            "criteria": ["not urgent", "soon", "critical"],
        },
        "refund_requested": {
            "type": "noul",
            "instructions": "Does the user explicitly request a refund?",
        },
    },
}


def _rand_str(n: int) -> str:
    return "".join(random.choice("abc xyz?-_0123") for _ in range(n))


def _valid_request(rng: random.Random) -> dict:
    """A perfectly valid /system_one payload (control case)."""
    return json.loads(json.dumps(REALISTIC_BASE))  # deep copy


def _missing_instructions(rng: random.Random) -> dict:
    p = _valid_request(rng)
    qid = rng.choice(list(p["questions"].keys()))
    del p["questions"][qid]["instructions"]
    return p


def _unknown_qtype(rng: random.Random) -> dict:
    p = _valid_request(rng)
    qid = rng.choice(list(p["questions"].keys()))
    p["questions"][qid]["type"] = rng.choice(["choyce", "ranking", "", "wibble"])
    return p


def _missing_questions(rng: random.Random) -> dict:
    p = _valid_request(rng)
    del p["questions"]
    return p


def _questions_not_object(rng: random.Random) -> dict:
    p = _valid_request(rng)
    p["questions"] = ["not", "an", "object"]
    return p


def _missing_state(rng: random.Random) -> dict:
    p = _valid_request(rng)
    del p["state"]
    return p


def _extra_top_level_key(rng: random.Random) -> dict:
    p = _valid_request(rng)
    p["surprise"] = _rand_str(8)
    return p


def _malformed_json(rng: random.Random) -> dict:
    # Not a dict at all — the server must still 400 cleanly, not 500.
    return {"_malformed_": "{not: even: json"}


def _deeply_nested_criteria(rng: random.Random) -> dict:
    p = _valid_request(rng)
    p["questions"]["weird"] = {
        "type": "choice",
        "instructions": "x",
        "criteria": {"a": {"deep": [1, 2, 3]}},
    }
    return p


def _huge_unicode(rng: random.Random) -> dict:
    p = _valid_request(rng)
    p["state"] = "🚨" * 200 + " test"
    return p


MUTATIONS = [
    ("valid (control)", _valid_request),
    ("missing_instructions", _missing_instructions),
    ("unknown_qtype", _unknown_qtype),
    ("missing_questions", _missing_questions),
    ("questions_not_object", _questions_not_object),
    ("missing_state", _missing_state),
    ("extra_top_level_key", _extra_top_level_key),
    ("malformed_json", _malformed_json),
    ("deeply_nested_criteria", _deeply_nested_criteria),
    ("huge_unicode", _huge_unicode),
]


def generate_corpus(n_per_class: int, seed: int = 0) -> list[tuple[str, dict]]:
    rng = random.Random(seed)
    out = []
    for label, gen in MUTATIONS:
        for _ in range(n_per_class):
            try:
                p = gen(rng)
                out.append((label, p))
            except Exception as e:
                out.append((label, {"_gen_error": str(e)}))
    rng.shuffle(out)
    return out


# ---------------------------------------------------------------------------
# 2. HTTP + invariant checking.
# ---------------------------------------------------------------------------


def hit(url: str, payload: dict, timeout: float = 30.0) -> tuple[int, dict | str, float]:
    """POST a payload to /v1/systemone. Returns (status, body_or_err, ms)."""
    body = json.dumps(payload).encode("utf-8")
    req = urllib.request.Request(
        f"{url}/v1/systemone",
        data=body,
        headers={"Content-Type": "application/json"},
        method="POST",
    )
    t0 = time.perf_counter()
    try:
        with urllib.request.urlopen(req, timeout=timeout) as r:
            data = r.read()
            status = r.status
    except urllib.error.HTTPError as e:
        data = e.read()
        status = e.code
    except Exception as e:
        return 0, f"transport: {e!r}", (time.perf_counter() - t0) * 1000
    ms = (time.perf_counter() - t0) * 1000
    try:
        return status, json.loads(data), ms
    except Exception:
        return status, data.decode("utf-8", errors="replace")[:200], ms


def check_invariants(resp: dict) -> list[str]:
    """Return a list of invariant violations (empty == all good)."""
    bad = []
    for qid, ans in resp.get("answers", {}).items():
        t = ans.get("type")
        if t == "choice":
            keys = list(ans.get("probabilities", {}).keys())
            chosen = ans.get("choice", "")
            if not chosen:
                bad.append(f"{qid}: choice is empty")
            if chosen not in keys and chosen != "":
                bad.append(f"{qid}: choice {chosen!r} not in declared keys {keys}")
            ps = ans.get("probabilities", {})
            s = sum(float(v) for v in ps.values())
            if abs(s - 1.0) > 0.01:
                bad.append(f"{qid}: prob sum {s:.4f} != 1.0")
        elif t == "score":
            try:
                idx = int(round(float(ans.get("score", -1))))
            except Exception:
                idx = -1
            legend = ans.get("legend", {})
            if idx >= len(legend):
                bad.append(f"{qid}: score idx {idx} >= legend len {len(legend)}")
            if idx < 0:
                bad.append(f"{qid}: score {ans.get('score')} out of range")
        conf = ans.get("confidence")
        if conf is not None and not (0.0 <= float(conf) <= 1.0):
            bad.append(f"{qid}: confidence {conf} not in [0,1]")
    return bad


def rejection_bucket(error_msg: str) -> str:
    """Map a 400 'gbnf: ...' message to a coarse bucket."""
    if not isinstance(error_msg, str):
        return "other"
    if "root rule" in error_msg:
        return "root_not_found"
    if "gbnf" not in error_msg.lower():
        return "non-gbnf-400"
    return "gbnf-structural"


# ---------------------------------------------------------------------------
# 3. Reporting.
# ---------------------------------------------------------------------------


def run(url: str, n_per_class: int, seed: int, out: Path | None) -> int:
    corpus = generate_corpus(n_per_class, seed)
    print(f"corpus: {len(corpus)} requests across {len(MUTATIONS)} mutation classes")

    by_class: dict[str, dict] = {
        label: {"n": 0, "status_400": 0, "status_200": 0, "status_5xx": 0,
                "invar_violations": [], "latencies_ms": []}
        for label, _ in MUTATIONS
    }
    buckets: Counter = Counter()
    all_latencies: list[float] = []

    for label, payload in corpus:
        if "_gen_error" in payload:
            by_class[label]["invar_violations"].append(f"gen_error: {payload['_gen_error']}")
            by_class[label]["n"] += 1
            continue
        status, body, ms = hit(url, payload)
        by_class[label]["n"] += 1
        by_class[label]["latencies_ms"].append(ms)
        all_latencies.append(ms)
        if status == 200:
            by_class[label]["status_200"] += 1
            inv = check_invariants(body if isinstance(body, dict) else {})
            by_class[label]["invar_violations"].extend(inv)
        elif status == 400:
            by_class[label]["status_400"] += 1
            buckets[rejection_bucket(body if isinstance(body, str) else json.dumps(body))] += 1
        elif status >= 500:
            by_class[label]["status_5xx"] += 1
        else:
            by_class[label]["invar_violations"].append(f"unexpected status {status}")

    # ── report ────────────────────────────────────────────────────────
    lines = ["# laya-tch --gbnf-strict stress report", ""]
    lines.append(f"endpoint: `{url}`")
    lines.append(f"corpus size: {len(corpus)} (seed={seed}, {n_per_class}/class × {len(MUTATIONS)} classes)")
    lines.append("")

    lines.append("## Per-class outcome")
    lines.append("")
    lines.append("| class | n | 200 | 400 | 5xx | invar violations | p50 ms | p95 ms |")
    lines.append("|---|---:|---:|---:|---:|---|---:|---:|")
    for label, _ in MUTATIONS:
        s = by_class[label]
        lats = sorted(s["latencies_ms"])
        p50 = lats[len(lats) // 2] if lats else 0
        p95 = lats[int(len(lats) * 0.95)] if lats else 0
        inv = "; ".join(s["invar_violations"][:3])
        if len(s["invar_violations"]) > 3:
            inv += f" (+{len(s['invar_violations']) - 3} more)"
        lines.append(f"| `{label}` | {s['n']} | {s['status_200']} | {s['status_400']} | "
                     f"{s['status_5xx']} | {inv or '—'} | {p50:.1f} | {p95:.1f} |")
    lines.append("")

    lines.append("## 400 rejection buckets")
    lines.append("")
    if buckets:
        for k, v in buckets.most_common():
            lines.append(f"* `{k}`: {v}")
    else:
        lines.append("(no 400s in this corpus)")
    lines.append("")

    if all_latencies:
        s = sorted(all_latencies)
        lines.append("## Latency")
        lines.append(f"  p50 = {s[len(s) // 2]:.1f} ms, "
                     f"p95 = {s[int(len(s) * 0.95)]:.1f} ms, "
                     f"max = {s[-1]:.1f} ms")
        lines.append("")

    # ── summary assertion ─────────────────────────────────────────────
    total_5xx = sum(s["status_5xx"] for s in by_class.values())
    total_invar = sum(len(s["invar_violations"]) for s in by_class.values())
    lines.append("## Headline")
    lines.append(f"* 5xx responses: **{total_5xx}** (target: 0)")
    lines.append(f"* invariant violations on 200 responses: **{total_invar}** (target: 0)")

    text = "\n".join(lines) + "\n"
    print(text)
    if out:
        out.parent.mkdir(parents=True, exist_ok=True)
        out.write_text(text)
        print(f"wrote {out}")
    return 0 if (total_5xx == 0 and total_invar == 0) else 1


def main() -> int:
    ap = argparse.ArgumentParser()
    ap.add_argument("--base-url", default="http://127.0.0.1:8400")
    ap.add_argument("--n-per-class", type=int, default=10)
    ap.add_argument("--seed", type=int, default=42)
    ap.add_argument("--out", type=Path, default=None)
    args = ap.parse_args()
    return run(args.base_url.rstrip("/"), args.n_per_class, args.seed, args.out)


if __name__ == "__main__":
    sys.exit(main())
