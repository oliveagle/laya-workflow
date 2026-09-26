"""Capture a PyTorch reference trace for the Laya english checkpoint.

Produces `reference_trace.json` containing, for a fixed request:
  - the exact token ids + marker positions per question (from build_sequence)
  - the raw per-question logits (pre-temperature)
  - the calibrated answers (choice / score / noul / confidence / act_probability)
  - the encoder last_hidden_state for a small probe sequence (flattened, fp32)

The Rust implementation (laya-tch) must reproduce ids/logits/answers exactly.

Run with a python that has torch + transformers ($PYTHON):
  $PYTHON code/laya-tch/bench/ref_capture.py
"""
from __future__ import annotations

import json
import os
import sys
import time
from pathlib import Path

import numpy as np
import torch

MODEL_DIR = Path(os.environ.get("LAYA_MODEL_DIR", Path.home() / "models" / "convaiinnovations--laya"))
OUT = Path(__file__).resolve().parent / "reference_trace.json"
ENC_OUT = Path(__file__).resolve().parent / "reference_encoder.npz"
PROBE_DIR = Path(__file__).resolve().parent / "reference_probe"

sys.path.insert(0, str(MODEL_DIR))
from rl_agent_api import RLAgent  # noqa: E402
from rl_common import QTYPES, build_sequence, collate_items, temp_bucket, confidence_from_probs, render_options  # noqa: E402

STATE = {
    "from": "user@acme.com",
    "subject": "Duplicate charge on invoice #4411",
    "body": ("Hi, we were billed twice for March. Please refund the duplicate today "
             "or we will cancel our plan."),
}

QUESTIONS = {
    "department": {
        "type": "choice",
        "instructions": "Which department should handle this request?",
        "criteria": {
            "billing": "invoices, payments, refunds",
            "technical": "bugs, outages, system errors",
            "sales": "pricing, new contracts",
            "other": "everything else",
        },
    },
    "urgency": {
        "type": "score",
        "instructions": "How urgent is this request?",
        "criteria": ["not urgent", "soon", "critical deadline or blocking issue"],
    },
    "churn_risk": {
        "type": "noul",
        "instructions": "Does the user threaten to cancel or leave?",
    },
    "refund_requested": {
        "type": "noul",
        "instructions": "Does the user explicitly request a refund?",
    },
}


def main() -> None:
    torch.set_grad_enabled(False)
    agent = RLAgent(str(MODEL_DIR), device="cpu")
    cfg = agent.cfg

    # --- exact per-question encoding (ids + markers) ---------------------
    rows = []
    qids = list(QUESTIONS.keys())
    for qid in qids:
        q = agent._to_internal(QUESTIONS[qid])
        ids, markers = build_sequence(agent.tok, STATE, q, cfg["max_len"], cfg["head_max_len"])
        rows.append({"qid": qid, "ids": list(map(int, ids)), "markers": list(map(int, markers)),
                     "qtype": int(QTYPES[q["t"]]), "type": q["t"], "n_options": len(render_options(q))})

    # --- one batched forward (mirrors RLAgent.system_one) ----------------
    items = []
    for r in rows:
        items.append({"ids": r["ids"], "markers": r["markers"], "qtype": r["qtype"],
                      "target": [0.0] * len(r["markers"]), "label": -1, "episode": 0,
                      "ep_step": 0, "ep_len": 1, "src": "apibench"})
    b = collate_items([items], agent.tok.pad_token_id)
    t0 = time.perf_counter()
    logits, act = agent.model(b["input_ids"], b["attention_mask"], b["marker_pos"],
                              b["marker_mask"], b["qtype"])
    t1 = time.perf_counter()
    logits = logits.float().numpy()
    act = torch.softmax(act.float(), -1).numpy()

    for r, row in enumerate(rows):
        k = len(row["markers"])
        row["logits"] = [float(x) for x in logits[r, :k]]
        row["act_probability"] = float(act[r, 0])

    answers = agent.system_one(STATE, QUESTIONS)
    forward_ms = (t1 - t0) * 1000.0

    gold = {"model_dir": str(MODEL_DIR), "state": STATE, "questions": QUESTIONS,
            "max_len": cfg["max_len"], "head_max_len": cfg["head_max_len"],
            "rows": rows, "answers": answers["answers"], "usage": answers["usage"],
            "forward_ms": forward_ms}

    # --- per-row forward at TRUE length (no padding) ---------------------
    # The Rust engine processes each question at its true length; with proper
    # key masking this equals the padded batched result for valid positions.
    with torch.no_grad():
        for r, row in enumerate(rows):
            ids = torch.tensor([row["ids"]], dtype=torch.long)
            am = torch.ones_like(ids)
            mp = torch.tensor([row["markers"]], dtype=torch.long)
            mm = torch.ones_like(mp, dtype=torch.bool)
            lg, _ = agent.model(ids, am, mp, mm, torch.tensor([row["qtype"]]))
            row["single_logits"] = [float(x) for x in lg.float()[0, :len(row["markers"])]]

    # --- encoder probe: last_hidden_state for row 0 at TRUE length -------
    with torch.no_grad():
        p_ids = torch.tensor([rows[0]["ids"]], dtype=torch.long)
        p_am = torch.ones_like(p_ids)
        enc = agent.model.encoder(p_ids, attention_mask=p_am).last_hidden_state
        h0 = enc[0].float().numpy()  # [L, 1024]
        te = agent.model.type_emb(torch.tensor([rows[0]["qtype"]]))[:, None, :]
        h_te = (enc + te)[0].float().numpy()
        hh = enc + te
        pad = torch.zeros_like(p_am, dtype=torch.bool)
        for layer in agent.model.head.layers:
            hh = layer(hh, src_key_padding_mask=pad)
        hh3 = hh  # [1, L, H] post-head
        hh = hh3[0].float().numpy()
        mk = torch.tensor([rows[0]["markers"]], dtype=torch.long)
        idx = mk.clamp(min=0)[:, :, None].expand(-1, -1, hh3.shape[-1])
        m = torch.gather(hh3, 1, idx)
        sc = agent.model.scorer(m).squeeze(-1).float().numpy()
    np.savez(ENC_OUT,
             probe_ids=p_ids.numpy().astype(np.int64),
             probe_attn=p_am.numpy().astype(np.int64),
             encoder_hidden=h0.astype(np.float32),
             after_type_emb=h_te.astype(np.float32),
             head_hidden=hh.astype(np.float32),
             marker_pos=mk.numpy().astype(np.int64),
             marker_logits=sc.astype(np.float32))

    OUT.write_text(json.dumps(gold, indent=2, ensure_ascii=False))

    # plain (non-npz) probe files so the Rust parity binary can read them
    PROBE_DIR.mkdir(exist_ok=True)
    (PROBE_DIR / "probe_ids.i64").write_bytes(p_ids.numpy().astype("<i8").tobytes())
    (PROBE_DIR / "probe_attn.i64").write_bytes(p_am.numpy().astype("<i8").tobytes())
    (PROBE_DIR / "probe_encoder.f32").write_bytes(h0.astype("<f4").tobytes())
    (PROBE_DIR / "probe_after_type.f32").write_bytes(h_te.astype("<f4").tobytes())
    (PROBE_DIR / "probe_head.f32").write_bytes(hh.astype("<f4").tobytes())
    (PROBE_DIR / "probe_marker_pos.i64").write_bytes(mk.numpy().astype("<i8").tobytes())
    (PROBE_DIR / "probe_marker_logits.f32").write_bytes(sc.astype("<f4").tobytes())
    (PROBE_DIR / "meta.json").write_text(json.dumps({
        "seq_len": int(h0.shape[0]), "hidden": int(h0.shape[1]),
        "n_markers": int(mk.numel()), "marker_pos": [int(x) for x in mk.numpy().tolist()[0]],
        "qtype": int(rows[0]["qtype"]),
        "pad_token_id": int(agent.tok.pad_token_id),
    }))

    print(f"wrote {OUT}")
    print(f"wrote {ENC_OUT}  (encoder_hidden {h0.shape}, head_hidden {hh.shape})")
    print(f"single batched forward: {forward_ms:.1f} ms for {len(rows)} questions")
    for r in rows:
        a = answers["answers"][r["qid"]]
        d = max(abs(x - y) for x, y in zip(r["logits"], r["single_logits"]))
        print(f"  {r['qid']:18s} L={len(r['ids']):3d} k={r['n_options']:2d}  "
              f"logits={['%.4f' % x for x in r['single_logits']]}  batched-vs-single maxdiff={d:.2e}  -> "
              f"{json.dumps({kk: vv for kk, vv in a.items() if kk != 'rl_agent'}, default=str)}")


if __name__ == "__main__":
    main()
