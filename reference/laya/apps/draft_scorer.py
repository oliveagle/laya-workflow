"""App 4 — Draft quality scorer (mirrors undertone, Vibe Check for X, tg-crush).

Scores a draft on tone, clarity, professionalism; suggests send/polish/sleep/rewrite.
Fix: neutral-key choice for boolean checks.
"""
from __future__ import annotations
import json, sys
from typing import Any, Dict

from code.laya.engine import LayaEngine

QUESTIONS = {
    "tone":           {"type": "score", "instructions": "Overall tone?",
                       "criteria": ["angry / hostile", "tense", "neutral", "warm / friendly"]},
    "clarity":        {"type": "score", "instructions": "How clearly does the message convey its point?",
                       "criteria": ["confusing", "ambiguous", "clear", "crystal clear"]},
    "professionalism":{"type": "score", "instructions": "How professional is the register?",
                       "criteria": ["unprofessional", "casual", "professional", "executive-grade"]},
    "has_typo":       {"type": "choice",
                       "instructions": "Does the message contain obvious typos, broken sentences, or autocorrect errors?",
                       "criteria": {"A": "no, it reads cleanly",
                                    "B": "yes, there are typos or broken sentences"}},
    "is_sensitive":   {"type": "choice",
                       "instructions": "Is the message on a sensitive topic (layoffs, legal, money conflict, breakup, politics)?",
                       "criteria": {"A": "no, it is a normal business or social message",
                                    "B": "yes, it touches layoffs / legal / money conflict / politics"}},
    "send_now":       {"type": "choice",
                       "instructions": "Best next action?",
                       "criteria": {"send_now": "send as-is",
                                    "polish":   "light edits recommended",
                                    "sleep":    "sleep on it / get a second opinion",
                                    "rewrite":  "major rewrite required"}},
}


def decide(text: str, audience: str = "team") -> Dict[str, Any]:
    v = LayaEngine.shared().decide({"audience": audience, "text": text}, QUESTIONS)
    a = v.answers
    typo = a["has_typo"].probabilities.get("B", 0.0) >= 0.5
    sens = a["is_sensitive"].probabilities.get("B", 0.0) >= 0.5
    if typo and a["clarity"].answer < 1.5:
        suggestion = "polish"
    elif sens and a["tone"].answer < 1.0:
        suggestion = "sleep"
    elif a["clarity"].answer < 0.5:
        suggestion = "rewrite"
    else:
        suggestion = a["send_now"].answer
    return {"suggestion": suggestion,
            "tone": a["tone"].answer, "clarity": a["clarity"].answer,
            "professionalism": a["professionalism"].answer,
            "typo_likely": typo, "sensitive": sens,
            "latency_ms": round(v.latency_ms, 1), "input_tokens": v.input_tokens}


if __name__ == "__main__":
    cases = [
        ("Hey everyone, I'll be OOO next week. Back on 15th. ping me if urgent.",    "team",    "send_now"),
        ("This is UNACCEPTABLE. I've told you THREE times. Fix NOW.",                "support", "sleep|rewrite"),
        ("Hi! Thanks so much for the kind words — made my morning.",                  "mentor",  "send_now"),
        ("Per our prev discusion, pls find the attched doc.",                         "client",  "polish"),
        ("I've decided to leave. Here's my 2-week plan and handoffs.",                "manager", "sleep|rewrite"),
        ("ok",                                                                        "manager", "polish|rewrite"),
    ]
    print(f"{'SUGGEST':<8}  {'TONE':>4}  {'CLAR':>4}  {'PROF':>4}  {'LAT':>6}  TEXT")
    print("-" * 100)
    for t, aud, exp in cases:
        r = decide(t, aud)
        ok = "✅" if r["suggestion"] in exp.split("|") else "⚠️"
        print(f"{r['suggestion']:<8}  {r['tone']:>4.1f}  {r['clarity']:>4.1f}  {r['professionalism']:>4.1f}  {r['latency_ms']:>5.1f}ms  {ok}{exp:<13}  {t[:45]}")
