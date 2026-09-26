"""Capture the raw (unrounded) PyTorch probabilities for every api_parity case.

The reference API (`rl_agent_api.system_one`) rounds to 4 decimals before
returning, which hides the true numerical agreement between engines. This script
reproduces the same computation *without* the final `round()` and writes
`bench/reference_raw.json`, so the Rust engine's raw probabilities can be
compared like-for-like.

Run: $PYTHON (a python that has torch + transformers installed)code/laya-tch/bench/ref_raw_capture.py
"""
from __future__ import annotations

import json
import os
import sys
from pathlib import Path

import numpy as np
import torch

HERE = Path(__file__).resolve().parent
MODEL_DIR = Path(os.environ.get("LAYA_MODEL_DIR", Path.home() / "models" / "convaiinnovations--laya"))
OUT = HERE / "reference_raw.json"

sys.path.insert(0, str(MODEL_DIR))
from rl_agent_api import RLAgent  # noqa: E402
from rl_common import QTYPES, build_sequence, collate_items, render_options, temp_bucket  # noqa: E402

TRACE = json.loads((HERE / "reference_trace.json").read_text())

CASES = [
    {"state": TRACE["state"], "questions": TRACE["questions"]},
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


def main() -> None:
    torch.set_grad_enabled(False)
    agent = RLAgent(str(MODEL_DIR), device="cpu")
    out = []
    for case in CASES:
        state, questions = case["state"], case["questions"]
        ids, items = list(questions.keys()), []
        for qid in ids:
            q = agent._to_internal(questions[qid])
            seq, markers = build_sequence(agent.tok, state, q, agent.cfg["max_len"], agent.cfg["head_max_len"])
            items.append({"ids": seq, "markers": markers, "qtype": QTYPES[q["t"]],
                          "target": [0.0] * len(markers), "label": -1, "episode": 0,
                          "ep_step": 0, "ep_len": 1, "src": "raw"})
        b = collate_items([items], agent.tok.pad_token_id)
        with torch.no_grad():
            logits, act = agent.model(b["input_ids"], b["attention_mask"], b["marker_pos"],
                                      b["marker_mask"], b["qtype"])
        lg = logits.float().cpu().numpy()
        act = torch.softmax(act.float(), -1).cpu().numpy()
        answers = {}
        for r, qid in enumerate(ids):
            q = agent._to_internal(questions[qid])
            k = len(items[r]["markers"])
            qt = QTYPES[q["t"]]
            temp = agent.temperature_by_options.get(temp_bucket(qt, k), agent.temperature[qt])
            z = lg[r, :k] / temp
            p = np.exp(z - z.max())
            p = p / p.sum()
            rec = {"type": q["t"], "logits": [float(x) for x in lg[r, :k]],
                   "probs": [float(x) for x in p],
                   "act_probability": float(act[r, 0])}
            if q["t"] == "choice":
                rec["keys"] = list(q["crit"].keys())
            answers[qid] = rec
        out.append({"answers": answers})

    OUT.write_text(json.dumps(out, indent=1))
    print(f"wrote {OUT} ({len(out)} cases)")
    for i, c in enumerate(out):
        for qid, a in c["answers"].items():
            print(f"  case{i}/{qid}: probs={['%.8f' % v for v in a['probs']]}")


if __name__ == "__main__":
    main()
