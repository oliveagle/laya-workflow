#!/usr/bin/env python3
"""Oracle = Python port of the laya-workflow heuristic backend.

This is the reference implementation used by `run.py` to score laya-mem's
actual output against the spec-declared heuristic rules. The match_any +
word-boundary logic mirrors `contains_any` in `src/backend.rs` exactly.

Each spec evaluates a state dict into the same dict shape the spec nodes
emit: `{"label": "...", "result": {...}}` with per-question answers.

Three specs are ported:
  * `memory_type` → dominant type label (TYPE_EPISODIC/...)
  * `admission`   → ALLOW / CONFIRM / BLOCK
  * `stopping`    → STOP_EVIDENCE_OK / CONTINUE_*
"""
from __future__ import annotations

import json
import re
from typing import Any


def contains_any(text: str, needles: list[str]) -> bool:
    """Mirror of `src/backend.rs::contains_any`.

    Tokens with whitespace/punctuation get substring match; single-word
    tokens require word boundaries so "need" does not match "needed".
    """
    l = text.lower()
    for n in needles:
        nl = n.lower()
        if any(c in nl for c in (" ", "-", "/", "@", ".")):
            if nl in l:
                return True
            continue
        # word-boundary
        for m in re.finditer(re.escape(nl), l):
            start, end = m.span()
            before_ok = start == 0 or not l[:start][-1].isalnum()
            after_ok = end >= len(l) or not l[end][0].isalnum()
            if before_ok and after_ok:
                return True
    return False


def serialize_state(state: dict) -> str:
    """Mirror of the heuristic's `serde_json::to_string(state)` text target."""
    return json.dumps(state, ensure_ascii=False, separators=(",", ":"))


# --- spec-declared heuristics (mirror dsl/laya_mem/*.json) ----------------

MEMORY_TYPE_QS = {
    "episodic":    ["planted", "started", "presented", "visited", "met", "began", "launched", "happened"],
    "semantic":    ["is a", "lives in", "works at", "born in", "facts about", "located in"],
    "procedural":  ["how to", "steps", "recipe", "instructions", "tutorial", "guide", "procedure"],
    "preference":  ["prefer", "prefers", "like", "likes", "favorite", "preference", "wants a weekly"],
}
MEMORY_TYPE_HIT = {"episodic": 0.8, "semantic": 0.8, "procedural": 0.8, "preference": 0.9}
MEMORY_TYPE_MISS = 0.1

ADMISSION_QS = {
    # choice: A = drop, B = store
    "should_store": {
        "kind": "choice",
        "match_any": ["thanks", "ok", "got it", "acknowledge", "noted", "trivial", "duplicate", "fine"],
        "p_hit": 0.1, "p_miss": 0.9,  # hit tokens → A (drop)
    },
    # noul
    "novelty":     {"kind": "noul", "match_any": ["new", "added", "changed", "updated"], "p_hit": 0.9, "p_miss": 0.1},
    "redundancy":  {"kind": "noul", "match_any": ["duplicate", "already", "same as", "repeat"], "p_hit": 0.9, "p_miss": 0.1},
    "borderline":  {"kind": "noul", "match_any": ["maybe", "might", "perhaps", "unsure", "borderline", "could be"], "p_hit": 0.85, "p_miss": 0.2},
}

# Spec declares per-question `field`; the heuristic only inspects that
# key. We mirror that here so oracle and tool agree exactly. The tool
# additionally folds the caller's `evidence_status` + boolean flags into
# those fields (see src/laya_mem.rs::RetrieveTool::call), so the oracle
# applies the same folding before evaluation.
STOPPING_QS = {
    "evidence_sufficient": {"kind": "noul", "field": "evidence_status", "match_any": ["sufficient"], "p_hit": 0.95, "p_miss": 0.05},
    "continue_useful":     {"kind": "noul", "field": "evidence_status", "match_any": ["sufficient"], "p_hit": 0.8,  "p_miss": 0.1},
    "missing_evidence":    {"kind": "noul", "field": "missing_evidence", "match_any": ["missing", "true"], "p_hit": 0.9, "p_miss": 0.05},
    "contradiction":       {"kind": "noul", "field": "contradiction",    "match_any": ["contradiction", "true"], "p_hit": 0.95, "p_miss": 0.05},
}


def _fold_stopping_state(state: dict) -> dict:
    """Mirror RetrieveTool's normalisation: fold evidence_status + booleans
    into the per-question trigger fields."""
    out = dict(state)
    ev = state.get("evidence_status")
    if isinstance(ev, str):
        if ev == "contradiction" or state.get("contradiction") is True:
            out["contradiction"] = "contradiction"
        if ev in ("insufficient", "missing") or state.get("missing_evidence") is True:
            out["missing_evidence"] = "missing"
    return out


def _answer_noul(text: str, needles: list[str], p_hit: float, p_miss: float) -> float:
    """Noul returns a probability in [0,1] (the post-fix behavior)."""
    return p_hit if contains_any(text, needles) else p_miss


def _answer_choice(text: str, needles: list[str], p_hit: float, p_miss: float) -> str:
    """Choice uses bchoice(p_b): hit -> p_b = p_hit, miss -> p_b = p_miss.
    If p_b >= 0.5 returns 'B', else 'A'."""
    p_b = p_hit if contains_any(text, needles) else p_miss
    return "B" if p_b >= 0.5 else "A"


# --- spec evaluators -------------------------------------------------------

def eval_memory_type(state: dict) -> dict:
    """Return {label, type_scores} where type_scores has 4 probs and label is dominant."""
    text = serialize_state(state)
    scores: dict[str, float] = {}
    for qname, needles in MEMORY_TYPE_QS.items():
        scores[qname] = _answer_noul(text, needles, MEMORY_TYPE_HIT[qname], MEMORY_TYPE_MISS)
    # threshold rules (first-match-wins): any score >= 0.5 wins in declared order
    label = "TYPE_OTHER"
    for qname in ("episodic", "semantic", "procedural", "preference"):
        if scores[qname] >= 0.5:
            label = f"TYPE_{qname.upper()}"
            break
    confidence = max(scores.values())
    return {"label": label, "type_scores": scores, "confidence": confidence}


def eval_admission(state: dict) -> dict:
    """Return {label, gate_action}. Rules: BLOCK if should_store=A;
    CONFIRM if borderline>=0.5; ALLOW otherwise."""
    text = serialize_state(state)
    answers: dict[str, Any] = {}
    for qname, q in ADMISSION_QS.items():
        if q["kind"] == "choice":
            answers[qname] = _answer_choice(text, q["match_any"], q["p_hit"], q["p_miss"])
        else:
            answers[qname] = _answer_noul(text, q["match_any"], q["p_hit"], q["p_miss"])
    label = "ALLOW"
    if answers["should_store"] == "A":
        label = "BLOCK"
    elif answers["borderline"] >= 0.5:
        label = "CONFIRM"
    return {"label": label, "answers": answers}


def eval_stopping(state: dict) -> dict:
    """Return {label}. Rules: CONTINUE_CONTRADICTION / _INSUFFICIENT /
    _MISSING first-match-wins; else STOP_EVIDENCE_OK."""
    folded = _fold_stopping_state(state)
    answers: dict[str, float] = {}
    for qname, q in STOPPING_QS.items():
        target = folded.get(q["field"], "")
        if not isinstance(target, str):
            target = "" if target is None else str(target)
        answers[qname] = _answer_noul(target, q["match_any"], q["p_hit"], q["p_miss"])
    label = "STOP_EVIDENCE_OK"
    if answers["contradiction"] >= 0.4:
        label = "CONTINUE_CONTRADICTION"
    elif answers["evidence_sufficient"] < 0.85:
        label = "CONTINUE_EVIDENCE_INSUFFICIENT"
    elif answers["missing_evidence"] >= 0.4:
        label = "CONTINUE_MISSING"
    return {"label": label, "answers": answers}


if __name__ == "__main__":
    # Quick self-test against a few known cases
    cases = [
        ("memory_type", {"observation": "Mira planted basil"}, "TYPE_EPISODIC"),
        ("memory_type", {"observation": "Alice prefers concise explanations"}, "TYPE_PREFERENCE"),
        ("memory_type", {"observation": "How to bake sourdough: a step-by-step guide"}, "TYPE_PROCEDURAL"),
        ("memory_type", {"observation": "Mira lives in Dallas"}, "TYPE_SEMANTIC"),
        ("memory_type", {"observation": "weather is nice"}, "TYPE_OTHER"),
        ("admission",   {"observation": "this is trivial"}, "BLOCK"),
        ("admission",   {"observation": "ack"}, "ALLOW"),
        ("admission",   {"observation": "maybe we should do X"}, "CONFIRM"),
        ("admission",   {"observation": "Alice prefers concise explanations"}, "ALLOW"),
        ("stopping",    {"evidence_status": "sufficient"}, "STOP_EVIDENCE_OK"),
        ("stopping",    {"evidence_status": "insufficient"}, "CONTINUE_EVIDENCE_INSUFFICIENT"),
        ("stopping",    {"evidence_status": "contradiction"}, "CONTINUE_CONTRADICTION"),
    ]
    fn = {"memory_type": eval_memory_type, "admission": eval_admission, "stopping": eval_stopping}
    ok = True
    for spec, state, want in cases:
        got = fn[spec](state)["label"]
        line = f"{spec:13} | {str(state)[:50]:50} | want={want:32} got={got:32}"
        if got != want:
            ok = False
            print("FAIL", line)
        else:
            print("PASS", line)
    print("\nORACLE:", "PASS" if ok else "FAIL")
