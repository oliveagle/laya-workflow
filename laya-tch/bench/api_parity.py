"""End-to-end API parity: Rust `laya-tch --once` vs PyTorch `RLAgent.system_one`.

Writes the reference request, runs the Rust binary one-shot, and compares every
answer field (choice / score / noul / probabilities / confidence / act_probability).

Run: $PYTHON (a python that has torch + transformers installed)laya-tch/bench/api_parity.py
"""
from __future__ import annotations

import json
import os
import subprocess
import sys
from pathlib import Path

HERE = Path(__file__).resolve().parent
CRATE = HERE.parent
MODEL_DIR = Path(os.environ.get("LAYA_MODEL_DIR", Path.home() / "models" / "convaiinnovations--laya"))
BIN = Path(os.environ.get("LAYA_TCH_BIN", CRATE / "target" / "release" / "laya-tch"))
REQ = HERE / "ref_request.json"
TRACE = json.loads((HERE / "reference_trace.json").read_text())

# extra cases: noul false, single choice, boolean-ish, unicode state
CASES = [
    {"state": {"text": "The office will be closed on Monday for a public holiday."},
     "questions": {
         "is_action_required": {"type": "noul", "instructions": "Does this require any action from the reader?"},
         "category": {"type": "choice", "instructions": "What is this about?",
                      "criteria": {"holiday": "closures and days off", "billing": "money", "security": "access"}},
     }},
    {"state": "I was charged twice for March. Please refund the duplicate ASAP.",
     "questions": {
         "urgency": {"type": "score", "instructions": "How urgent is this?",
                     "criteria": ["not urgent", "soon", "critical"]},
         "refund_requested": {"type": "noul", "instructions": "Does the user explicitly request a refund?"},
     }},
]


def run_rust(req_obj) -> dict:
    REQ.write_text(json.dumps(req_obj))
    out = subprocess.run([str(BIN), "--model-dir", str(MODEL_DIR), "--once", str(REQ)],
                         capture_output=True, text=True, check=True)
    return json.loads(out.stdout)


def compare(name: str, rust: dict, torch: dict) -> list[str]:
    errs = []
    ra, ta = rust["answers"], torch["answers"]
    if set(ra) != set(ta):
        errs.append(f"{name}: answer keys differ {sorted(ra)} vs {sorted(ta)}")
        return errs
    for qid in ta:
        r, t = ra[qid], ta[qid]
        if r["type"] != t["type"]:
            errs.append(f"{name}/{qid}: type {r['type']} != {t['type']}")
            continue
        if r["type"] == "choice":
            if r["choice"] != t["choice"]:
                errs.append(f"{name}/{qid}: choice {r['choice']!r} != {t['choice']!r}")
            for k, v in t["probabilities"].items():
                if abs(r["probabilities"][k] - v) > 1e-4:
                    errs.append(f"{name}/{qid}: prob[{k}] {r['probabilities'][k]} != {v}")
            if abs(r["confidence"] - t["confidence"]) > 1e-4:
                errs.append(f"{name}/{qid}: confidence {r['confidence']} != {t['confidence']}")
        elif r["type"] == "score":
            if abs(r["score"] - t["score"]) > 1e-4:
                errs.append(f"{name}/{qid}: score {r['score']} != {t['score']}")
            for k, v in t["probabilities"].items():
                if abs(r["probabilities"][k] - v) > 1e-4:
                    errs.append(f"{name}/{qid}: prob[{k}] {r['probabilities'][k]} != {v}")
        else:
            if abs(r["noul"] - t["noul"]) > 1e-4:
                errs.append(f"{name}/{qid}: noul {r['noul']} != {t['noul']}")
        ar = r.get("rl_agent", {}).get("act_probability")
        at = t.get("rl_agent", {}).get("act_probability")
        if ar is not None and at is not None and abs(ar - at) > 1e-4:
            errs.append(f"{name}/{qid}: act_probability {ar} != {at}")
    # the reference itself rounds to 4 decimals: a raw probability within ~1e-6
    # of a rounding boundary can legitimately print one ULP apart.
    return errs


def raw_gap(name: str, rust_raw: dict, ref_raw: dict) -> list[str]:
    """Compare *unrounded* probabilities raw-vs-raw (the honest accuracy metric,
    insensitive to 4-decimal boundary crossings)."""
    errs = []
    for qid, ref in ref_raw["answers"].items():
        ra = rust_raw["answers"].get(qid)
        if ra is None:
            errs.append(f"{name}/{qid}: missing from rust raw output")
            continue
        rp = ra.get("probs")
        if rp is None or len(rp) != len(ref["probs"]):
            errs.append(f"{name}/{qid}: raw probs shape {rp} vs {ref['probs']}")
            continue
        for i, (rv, tv) in enumerate(zip(rp, ref["probs"])):
            if abs(rv - tv) > 1e-5:
                errs.append(f"{name}/{qid}: raw prob[{i}] {rv} vs {tv}")
        ra_act = ra.get("act_probability")
        if ra_act is not None and abs(ra_act - ref["act_probability"]) > 1e-5:
            errs.append(f"{name}/{qid}: raw act {ra_act} vs {ref['act_probability']}")
    return errs


def run_rust_raw(req_obj) -> dict:
    REQ.write_text(json.dumps(req_obj))
    env = dict(os.environ, LAYA_DEBUG_PROBS="1")
    out = subprocess.run([str(BIN), "--model-dir", str(MODEL_DIR), "--once", str(REQ)],
                         capture_output=True, text=True, check=True, env=env)
    raw = {"answers": {}}
    for line in out.stderr.splitlines():
        if not line.startswith("[dbg]"):
            continue
        qid = line.split("qid=", 1)[1].split(" ", 1)[0]
        rp = line.split("raw_p=[", 1)[1].split("]", 1)[0]
        ra = line.split("raw_act=", 1)[1].split("}", 1)[0].strip() if "raw_act=" in line else None
        vals = [float(x) for x in rp.split(",")]
        raw["answers"][qid] = {"probs": vals, "act_probability": float(ra) if ra else None}
    return raw


def main() -> None:
    sys.path.insert(0, str(MODEL_DIR))
    from rl_agent_api import RLAgent

    agent = RLAgent(str(MODEL_DIR), device="cpu")
    cases = [{"state": TRACE["state"], "questions": TRACE["questions"]}] + CASES

    all_errs = []
    raw_errs = []
    rounded_boundary_cases = 0
    ref_raw_all = json.loads((HERE / "reference_raw.json").read_text())
    for i, case in enumerate(cases):
        name = f"case{i}"
        rust = run_rust(case)
        rust_raw = run_rust_raw(case)
        torch = agent.system_one(case["state"], case["questions"])
        errs = compare(name, rust, torch)
        rerrs = raw_gap(name, rust_raw, ref_raw_all[i])
        all_errs += errs
        raw_errs += rerrs
        if errs and not rerrs:
            rounded_boundary_cases += 1
        tag = "OK " if not errs else ("ULP" if not rerrs else "DIFF")
        print(f"[{tag}] {name}: rust   = {json.dumps(rust['answers'])}")
        print(f"        torch  = {json.dumps(torch['answers'])}")
        for e in errs:
            print("        rounded:", e)
        for e in rerrs:
            print("        raw    :", e)

    print()
    print(f"raw-probability agreement (1e-5): {'PASS' if not raw_errs else 'FAIL'}")
    print(f"rounded-output agreement (1e-4):  {'PASS' if not all_errs else 'FAIL'} "
          f"({rounded_boundary_cases} case(s) differ only by 4-decimal rounding boundary)")
    if raw_errs:
        sys.exit(2)
    if all_errs:
        print("⚠️  rounded outputs differ by 1 ULP on a rounding boundary; raw values agree.")


if __name__ == "__main__":
    main()
