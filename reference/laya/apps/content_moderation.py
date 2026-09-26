"""App 3 — Content moderation (mirrors Soter, Transcript Lens, Xtags).

Detects unsafe content. Returns APPROVE / WARN / BLOCK.
Fix: use neutral-key `choice` to avoid noul option-label bias.
"""
from __future__ import annotations
import json, sys
from typing import Any, Dict

from code.laya.engine import LayaEngine

QUESTIONS = {
    "is_hate":     {"type": "choice",
                    "instructions": "Does the text contain slurs, dehumanising language, or hate speech targeting a group?",
                    "criteria": {"A": "no, it is neutral or positive",
                                 "B": "yes, it dehumanises or attacks an identity group"}},
    "is_threat":   {"type": "choice",
                    "instructions": "Does the text contain a threat of violence, doxxing, or actionable harm against a person?",
                    "criteria": {"A": "no, it is safe or merely critical",
                                 "B": "yes, it threatens violence or harm against a specific person"}},
    "is_pii":      {"type": "choice",
                    "instructions": "Does the text contain personally identifiable information (full SSN, credit card, full address)?",
                    "criteria": {"A": "no, it does not expose personal data",
                                 "B": "yes, it leaks full SSN / card / home address"}},
    "is_spam":     {"type": "choice",
                    "instructions": "Is the text unsolicited commercial or promotional bulk content?",
                    "criteria": {"A": "no, it is a normal message",
                                 "B": "yes, it is spam or unsolicited promo"}},
    "severity":    {"type": "score",
                    "instructions": "Overall severity?",
                    "criteria": ["safe to publish", "needs light edit", "must be blocked"]},
    "category":    {"type": "choice",
                    "instructions": "Best single label?",
                    "criteria": {"safe": "no concerns",
                                 "hateful": "hate or harassment",
                                 "threatening": "violent threat",
                                 "leaky_pii": "leaks personal data",
                                 "spam": "spam or promo",
                                 "off_topic": "irrelevant or low-quality"}},
}


def decide(text: str, source: str = "user") -> Dict[str, Any]:
    v = LayaEngine.shared().decide({"source": source, "text": text}, QUESTIONS)
    a = v.answers
    sev = a["severity"].answer
    cat = a["category"].answer

    hate_p   = a["is_hate"].probabilities.get("B", 0.0)
    threat_p = a["is_threat"].probabilities.get("B", 0.0)
    pii_p    = a["is_pii"].probabilities.get("B", 0.0)
    spam_p   = a["is_spam"].probabilities.get("B", 0.0)

    if threat_p >= 0.7 or hate_p >= 0.7:
        action = "BLOCK"
    elif pii_p >= 0.6:
        action = "BLOCK"
    elif spam_p >= 0.7:
        action = "BLOCK"
    elif sev >= 1.5 or cat != "safe":
        action = "WARN"
    else:
        action = "APPROVE"

    return {"action": action, "severity": sev, "label": cat,
            "hate_p": round(hate_p, 2), "threat_p": round(threat_p, 2),
            "pii_p": round(pii_p, 2), "spam_p": round(spam_p, 2),
            "latency_ms": round(v.latency_ms, 1), "input_tokens": v.input_tokens}


if __name__ == "__main__":
    cases = [
        ("Thanks for the help! Looking forward to the release.",                                "APPROVE"),
        ("Get RICH QUICK with my crypto coin!!! Buy now limited offer",                         "BLOCK"),
        ("I'll find you and your family. Watch your back.",                                      "BLOCK"),
        ("Send to 123 Main St. SSN 123-45-6789",                                                "BLOCK"),
        ("All [group] are vermin who should be deported.",                                       "BLOCK"),
        ("Meeting agenda for next Tuesday. Three topics.",                                       "APPROVE"),
    ]
    print(f"{'ACTION':<8}  {'SEV':>3}  {'LABEL':<12}  {'LAT':>6}  TEXT")
    print("-" * 100)
    for t, exp in cases:
        r = decide(t)
        ok = "✅" if r["action"] == exp else "⚠️"
        print(f"{r['action']:<8}  {r['severity']:>3.1f}  {r['label']:<12}  {r['latency_ms']:>5.1f}ms  {ok}{exp:<10}  {t[:50]}")
