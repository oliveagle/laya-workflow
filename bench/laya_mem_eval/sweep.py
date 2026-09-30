#!/usr/bin/env python3
"""Parameter sweep: enumerate spec tuning candidates and score each against
the oracle. Currently sweeps the two production knobs we actually control:

  A. memory_type vocabulary richness — how many per-class match_any tokens
     each type question uses. More tokens catch more phrasings.
  B. admission should_store vocabulary — the block tokens.

For each candidate, we mutate a copy of the spec, run the tool against a
fresh DB, and score against the oracle port (re-implemented in python here
with the mutated vocabulary, so the oracle tracks the sweep).

Usage:
    PYTHONPATH= bench/laya_mem_eval/sweep.py
"""
from __future__ import annotations

import json
import subprocess
import sys
import tempfile
from pathlib import Path

HERE = Path(__file__).parent
sys.path.insert(0, str(HERE))
from run import McpClient
from oracle import contains_any, serialize_state

REPO = HERE.parent.parent
BIN = REPO / "target" / "debug" / "laya-workflow"
SPEC_DIR = REPO / "dsl" / "laya_mem"


# candidate vocabularies (natural phrasings added on top of the spec defaults)
VOCAB_CANDIDATES = {
    "episodic": {
        "v1_base":      ["planted", "started", "presented", "visited", "met", "began", "launched", "happened", "on friday"],
        "v2_natural":   ["planted", "started", "presented", "visited", "met", "began", "launched", "happened", "on friday",
                         "went", "joined", "got a", "deploy"],
        "v3_natural":   ["planted", "started", "presented", "visited", "met", "began", "launched", "happened", "on friday",
                         "went", "joined", "got a", "deploy", "came", "did", "arrived"],
    },
    "semantic": {
        "v1_base":      ["is a", "lives in", "works at", "born in", "facts about", "located in"],
        "v2_natural":   ["is a", "lives in", "works at", "born in", "facts about", "located in",
                         "hometown", "capital", "was released"],
        "v3_natural":   ["is a", "lives in", "works at", "born in", "facts about", "located in",
                         "hometown", "capital", "was released", "is the", "is one of"],
    },
    "procedural": {
        "v1_base":      ["how to", "steps", "recipe", "instructions", "tutorial", "guide", "procedure"],
        "v2_natural":   ["how to", "steps", "recipe", "instructions", "tutorial", "guide", "procedure",
                         "first", "then", "click", "follow"],
        "v3_natural":   ["how to", "steps", "recipe", "instructions", "tutorial", "guide", "procedure",
                         "first", "then", "click", "follow", "by tagging", "reset"],
    },
    "preference": {
        "v1_base":      ["prefer", "prefers", "like", "likes", "favorite", "preference", "wants a weekly"],
        "v2_natural":   ["prefer", "prefers", "like", "likes", "favorite", "preference", "wants a weekly",
                         "would rather", "always orders", "enjoys", "dislikes"],
        "v3_natural":   ["prefer", "prefers", "like", "likes", "favorite", "preference", "wants a weekly",
                         "would rather", "always orders", "enjoys", "dislikes", "rather not", "would not"],
    },
    "should_store": {
        "v1_base":      ["thanks", "ok", "got it", "acknowledge", "noted", "trivial", "duplicate", "fine"],
        "v2_natural":   ["thanks", "ok", "got it", "acknowledge", "noted", "trivial", "duplicate", "fine",
                         "acknowledged", "no need", "ack"],
        "v3_natural":   ["thanks", "ok", "got it", "acknowledge", "noted", "trivial", "duplicate", "fine",
                         "acknowledged", "no need", "ack", "nothing to", "not worth"],
    },
    "borderline": {
        "v1_base":      ["maybe", "might", "perhaps", "unsure", "borderline", "could be"],
        "v2_natural":   ["maybe", "might", "perhaps", "unsure", "borderline", "could be",
                         "not sure", "either way", "debating", "could go"],
        "v3_natural":   ["maybe", "might", "perhaps", "unsure", "borderline", "could be",
                         "not sure", "either way", "debating", "could go", "we could"],
    },
}


def oracle_with_vocab(text: str, vocab: dict[str, list[str]]) -> tuple[str, str]:
    """Score (type_label, admission) for the given text using the given
    vocabulary (mirror of eval_memory_type + eval_admission with per-call
    vocab)."""
    # type
    scores = {}
    for q in ("episodic", "semantic", "procedural", "preference"):
        needles = vocab[q]
        scores[q] = 0.8 if contains_any(text, needles) else 0.1
    if scores["episodic"] >= 0.5: t_label = "TYPE_EPISODIC"
    elif scores["semantic"] >= 0.5: t_label = "TYPE_SEMANTIC"
    elif scores["procedural"] >= 0.5: t_label = "TYPE_PROCEDURAL"
    elif scores["preference"] >= 0.5: t_label = "TYPE_PREFERENCE"
    else: t_label = "TYPE_OTHER"

    # admission
    hit_ss = contains_any(text, vocab["should_store"])
    hit_bl = contains_any(text, vocab["borderline"])
    a_label = "ALLOW"
    if hit_ss: a_label = "BLOCK"     # should_store hits → A → BLOCK
    elif hit_bl: a_label = "CONFIRM" # borderline hits
    return t_label, a_label


def apply_vocab(vocab: dict[str, list[str]]) -> str:
    """Mutate a fresh copy of dsl/laya_mem/ into a temp dir with the given
    vocab and return its path."""
    tmp = Path(tempfile.mkdtemp(prefix="laya_mem_sweep_"))
    for f in ("admission.json", "memory_type.json", "stopping.json", "persist_memory.json"):
        src = SPEC_DIR / f
        dst = tmp / f
        dst.write_text(src.read_text())
    # patch memory_type.json
    mt = json.loads((tmp / "memory_type.json").read_text())
    for n in mt.get("nodes", []):
        for qname, q in (n.get("questions") or {}).items():
            if qname in VOCAB_CANDIDATES and qname in vocab:
                if q.get("heuristic"):
                    q["heuristic"]["match_any"] = vocab[qname]
    (tmp / "memory_type.json").write_text(json.dumps(mt, indent=2, ensure_ascii=False))
    # patch admission.json
    ad = json.loads((tmp / "admission.json").read_text())
    for n in ad.get("nodes", []):
        for qname, q in (n.get("questions") or {}).items():
            if qname in ("should_store", "borderline") and qname in vocab:
                if q.get("heuristic"):
                    q["heuristic"]["match_any"] = vocab[qname]
    (tmp / "admission.json").write_text(json.dumps(ad, indent=2, ensure_ascii=False))
    return str(tmp)


def score_oracle(vocab: dict[str, list[str]], natural_rows) -> dict:
    """Pure-oracle scoring: no server spawn, evaluates the natural rows
    through the python port with the given vocab."""
    t_hit = 0; t_total = 0
    a_hit = 0; a_total = 0
    for state, want in natural_rows:
        text = serialize_state(state)
        otype, oadv = oracle_with_vocab(text, vocab)
        want_type = want if want.startswith("TYPE_") else None
        want_adv = want if want in ("ALLOW", "CONFIRM", "BLOCK") else None
        if want_type:
            t_total += 1
            if otype == want_type: t_hit += 1
        if want_adv:
            a_total += 1
            if oadv == want_adv: a_hit += 1
    return {
        "type_acc": t_hit / t_total if t_total else 0,
        "admission_acc": a_hit / a_total if a_total else 0,
    }


def main():
    from laya_datasets import NATURAL_OBSERVATIONS
    groups = list(VOCAB_CANDIDATES.keys())
    # dict keys are v1_base / v2_natural / v3_natural; combo key is short v1/v2/v3
    variant_keys = {"v1": "v1_base", "v2": "v2_natural", "v3": "v3_natural"}
    variants = ["v1", "v2", "v3"]
    import itertools
    print(f"natural observations: {len(NATURAL_OBSERVATIONS)}; sweep space {len(variants)**len(groups)} combos\n")
    results = []
    for combo in itertools.product(variants, repeat=len(groups)):
        merged = {g: VOCAB_CANDIDATES[g][variant_keys[v]] for g, v in zip(groups, combo)}
        key = "_".join(combo)
        r = score_oracle(merged, NATURAL_OBSERVATIONS)
        r["vocab"] = key
        results.append(r)
    # print per-combo, sorted
    results.sort(key=lambda r: -(r["type_acc"] + r["admission_acc"]))
    for r in results[:40]:
        print(f"  {r['vocab']}: type={r['type_acc']:.1%} admission={r['admission_acc']:.1%}")
    print()
    best = results[0]
    print(f"BEST: {best['vocab']} type={best['type_acc']:.1%} admission={best['admission_acc']:.1%}")
    # report
    md_lines = [
        "# laya-mem vocab sweep (Round-2)",
        "",
        f"Natural-phrasing accuracy under different match_any vocabularies. Space: {len(results)} combos (v1/v2/v3 x 6 groups).",
        "",
        "Top 25 by combined score:",
        "",
        "| combo | type_acc | admission_acc |",
        "|---|---|---|",
    ]
    for r in results[:25]:
        md_lines.append(f"| {r['vocab']} | {r['type_acc']:.1%} | {r['admission_acc']:.1%} |")
    md_lines.append("")
    md_lines.append(f"**Best: `{best['vocab']}` type={best['type_acc']:.1%} admission={best['admission_acc']:.1%}**")
    (HERE / "reports" / "sweep.md").write_text("\n".join(md_lines))
    print("\nwrote bench/laya_mem_eval/reports/sweep.md")



if __name__ == "__main__":
    main()
