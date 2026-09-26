"""App 2 — Email / Ticket auto-triage (mirrors Jev Mail Classifier, Jevmail).

Reuses the built-in email questions from the Laya repo's email_utils.py,
then maps raw answers into actionable routing decisions.
Fix: use neutral-key `choice` instead of `noul` to avoid option-label bias.
"""
from __future__ import annotations
import json, sys
from typing import Any, Dict, Optional

from code.laya.engine import LayaEngine

_DEFAULT_CATEGORIES = {
    "billing":     "invoices, payments, refunds",
    "technical":   "bugs, outages, integrations",
    "sales":       "pricing, demos, new purchases",
    "account":     "login, access, profile changes",
    "hr":          "hiring, leave, payroll",
    "other":       "none of the above",
}


def _clean(body: str, max_chars: int = 2500) -> str:
    out, lines = [], (body or "").replace("\r\n", "\n").split("\n")
    for line in lines:
        s = line.lstrip()
        if s.startswith(">"): continue
        if any(s.startswith(p) for p in ("On ", "From:", "----", "____")): break
        out.append(line.rstrip())
    return "\n".join(out)[:max_chars]


def _questions(categories: Optional[Dict[str, str]] = None) -> Dict[str, Any]:
    cats = categories or _DEFAULT_CATEGORIES
    return {
        "category":     {"type": "choice", "instructions": f"Which team should handle this email?",
                         "criteria": cats},
        "spam":         {"type": "choice",
                         "instructions": "Is this email unsolicited bulk marketing or spam?",
                         "criteria": {"A": "no, it is a legitimate one-to-one or business email",
                                      "B": "yes, it is spam, bulk marketing, or unsolicited promo"}},
        "phishing":     {"type": "choice",
                         "instructions": "Is this email a phishing or scam attempt to steal money, credentials, or personal data?",
                         "criteria": {"A": "no, it is a legitimate business email",
                                      "B": "yes, it tries to steal credentials, money, or personal data"}},
        "urgency":      {"type": "score", "instructions": "How urgent is the issue?",
                         "criteria": ["no time pressure", "needs attention soon", "blocking issue or hard deadline"]},
        "needs_reply":  {"type": "choice",
                         "instructions": "Does the sender expect a reply or follow-up action?",
                         "criteria": {"A": "no, FYI only or auto-notification",
                                      "B": "yes, the sender needs a response"}},
        "sentiment":    {"type": "score", "instructions": "Sender tone?",
                         "criteria": ["angry or very negative", "negative", "neutral", "positive"]},
    }


def decide(subject: str, body: str, sender: Optional[str] = None,
           categories: Optional[Dict[str, str]] = None) -> Dict[str, Any]:
    state = {"subject": (subject or "").strip(), "body": _clean(body), "from": sender or ""}
    v = LayaEngine.shared().decide(state, _questions(categories))
    a = v.answers

    is_spam = a["spam"].probabilities.get("B", 0.0) >= 0.5
    is_phish = a["phishing"].probabilities.get("B", 0.0) >= 0.5
    urgency = a["urgency"].answer
    needs_reply = a["needs_reply"].probabilities.get("B", 0.0) >= 0.5
    cat = a["category"].answer

    if is_phish:
        action = "QUARANTINE"
    elif is_spam:
        action = "TRASH"
    elif cat == "billing" and urgency >= 1.5:
        action = "PAGER_BILLING"
    elif cat == "technical" and urgency >= 1.5:
        action = "PAGER_ONCALL"
    elif needs_reply and urgency >= 1.5:
        action = "REPLY_TODAY"
    elif needs_reply:
        action = "REPLY_QUEUE"
    else:
        action = "FYI_ONLY"

    return {"category": cat, "spam": is_spam, "phishing": is_phish,
            "urgency": urgency, "needs_reply": needs_reply, "sentiment": a["sentiment"].answer,
            "action": action, "latency_ms": round(v.latency_ms, 1),
            "input_tokens": v.input_tokens}


if __name__ == "__main__":
    cases = [
        ("Re: invoice duplicate charge", "Hi, we were billed twice for March. Please refund today or we cancel.", "user@acme.com", "PAGER_BILLING"),
        ("ALL HANDS: prod outage", "Payments API 500 since 14:32 UTC. Need hotfix before sprint end.", "sre@x.com", "PAGER_ONCALL"),
        ("Earn $5000/week!!!", "Limited offer! Click here to earn. 50% off premium membership", "promo@spam.tld", "TRASH"),
        ("verify your account", "Your account will be suspended unless you confirm credentials at http://evil.example", "noreply@susp.cn", "QUARANTINE"),
        ("Office closed Monday", "Reminder office closed Monday. No action needed.", "mgr@acme.com", "FYI_ONLY"),
    ]
    print(f"{'CATEGORY':<10}  {'ACTION':<14}  {'URG':>3}  {'LAT':>6}  SUBJECT")
    print("-" * 90)
    for subj, body, sender, exp in cases:
        r = decide(subj, body, sender)
        ok = "✅" if r["action"] == exp else "⚠️"
        print(f"{ok}{r['category']:<9}  {r['action']:<14}  {r['urgency']:>3.1f}  {r['latency_ms']:>5.1f}ms  {subj[:40]}")
