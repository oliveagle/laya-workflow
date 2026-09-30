#!/usr/bin/env python3
"""Synthetic eval datasets for laya-mem production evaluation.

Each row is `(state, expected)` where the oracle + ground truth agree.

Categories:
  * memory_type  → 4-class type classification
  * admission    → 3-class gate (ALLOW/CONFIRM/BLOCK)
  * stopping     → STOP_EVIDENCE_OK / CONTINUE_*
  * recall       → (persist content, query, expected_substring_in_recall)
"""
from __future__ import annotations

# 30 memory-type observations, balanced across the 4 types + OTHER
MEMORY_TYPE_SET: list[tuple[dict, str]] = [
    # TYPE_EPISODIC (8)
    ({"observation": "Mira planted basil on the balcony last Saturday"}, "TYPE_EPISODIC"),
    ({"observation": "Bob started his new job on Monday"}, "TYPE_EPISODIC"),
    ({"observation": "Carol presented her research at the conference"}, "TYPE_EPISODIC"),
    ({"observation": "I visited Tokyo in spring 2024"}, "TYPE_EPISODIC"),
    ({"observation": "Alice met Bob at the cafe"}, "TYPE_EPISODIC"),
    ({"observation": "We began the project in January"}, "TYPE_EPISODIC"),
    ({"observation": "The product launched yesterday"}, "TYPE_EPISODIC"),
    ({"observation": "An accident happened on the highway"}, "TYPE_EPISODIC"),
    # TYPE_SEMANTIC (7)
    ({"observation": "Alice is a senior engineer at the company"}, "TYPE_SEMANTIC"),
    ({"observation": "Bob lives in Berlin"}, "TYPE_SEMANTIC"),
    ({"observation": "Carol works at the hospital"}, "TYPE_SEMANTIC"),
    ({"observation": "David was born in 1990"}, "TYPE_SEMANTIC"),
    ({"observation": "Facts about giraffes: they sleep 30 minutes a day"}, "TYPE_SEMANTIC"),
    ({"observation": "The Eiffel Tower is located in Paris"}, "TYPE_SEMANTIC"),
    ({"observation": "Python is a programming language with dynamic typing"}, "TYPE_SEMANTIC"),
    # TYPE_PROCEDURAL (6)
    ({"observation": "How to brew pour-over coffee: steps for a balanced cup"}, "TYPE_PROCEDURAL"),
    ({"observation": "Recipe for sourdough bread with detailed instructions"}, "TYPE_PROCEDURAL"),
    ({"observation": "Tutorial: deploy a Kubernetes cluster"}, "TYPE_PROCEDURAL"),
    ({"observation": "A guide to writing effective pull requests"}, "TYPE_PROCEDURAL"),
    ({"observation": "Procedure for setting up a new development environment"}, "TYPE_PROCEDURAL"),
    ({"observation": "Steps to migrate a database without downtime"}, "TYPE_PROCEDURAL"),
    # TYPE_PREFERENCE (6)
    ({"observation": "Alice prefers concise explanations"}, "TYPE_PREFERENCE"),
    ({"observation": "Bob likes dark mode in his editor"}, "TYPE_PREFERENCE"),
    ({"observation": "Carol's favorite color is blue"}, "TYPE_PREFERENCE"),
    ({"observation": "Express a preference for short meetings"}, "TYPE_PREFERENCE"),
    ({"observation": "Mira wants a weekly reminder for watering plants"}, "TYPE_PREFERENCE"),
    ({"observation": "The user prefers no animations in the UI"}, "TYPE_PREFERENCE"),
    # TYPE_OTHER (3 — should NOT match any class)
    ({"observation": "the weather forecast looks reasonable"}, "TYPE_OTHER"),
    ({"observation": "ok"}, "TYPE_OTHER"),
    ({"observation": "traffic was heavy this morning"}, "TYPE_OTHER"),
]


# 30 admission cases, balanced across the 3 outcomes
ADMISSION_SET: list[tuple[dict, str]] = [
    # BLOCK (8 — should_store=A via trivial/duplicate/fine/etc.)
    ({"observation": "this is trivial"}, "BLOCK"),
    ({"observation": "thanks"}, "BLOCK"),
    ({"observation": "ok"}, "BLOCK"),
    ({"observation": "got it"}, "BLOCK"),
    ({"observation": "noted"}, "BLOCK"),
    ({"observation": "fine"}, "BLOCK"),
    ({"observation": "duplicate entry, already exists"}, "BLOCK"),
    ({"observation": "acknowledge the message"}, "BLOCK"),
    # CONFIRM (8 — borderline tokens present)
    ({"observation": "maybe we should call John"}, "CONFIRM"),
    ({"observation": "might be useful later"}, "CONFIRM"),
    ({"observation": "perhaps store this?"}, "CONFIRM"),
    ({"observation": "I'm unsure if this matters"}, "CONFIRM"),
    ({"observation": "this feels borderline"}, "CONFIRM"),
    ({"observation": "it could be important"}, "CONFIRM"),
    ({"observation": "maybe Alice prefers tea"}, "CONFIRM"),
    ({"observation": "might be a duplicate, unsure"}, "BLOCK"),
    # ALLOW (14 — default, no admission tokens)
    ({"observation": "Alice lives in Dallas"}, "ALLOW"),
    ({"observation": "Bob started a new project on Tuesday"}, "ALLOW"),
    ({"observation": "Carol presented her research at the venue"}, "ALLOW"),
    ({"observation": "How to bake bread: a step-by-step recipe"}, "ALLOW"),
    ({"observation": "The Eiffel Tower is located in Paris"}, "ALLOW"),
    ({"observation": "Mira planted basil last weekend"}, "ALLOW"),
    ({"observation": "User prefers concise explanations"}, "ALLOW"),
    ({"observation": "weather is nice today"}, "ALLOW"),
    ({"observation": "no relevant content here"}, "ALLOW"),
    ({"observation": "I want to track my reading list"}, "ALLOW"),
    ({"observation": "The new feature is documented at /docs/x"}, "ALLOW"),
    ({"observation": "Bob's birthday is March 15"}, "ALLOW"),
    ({"observation": "Carol launched a new app"}, "ALLOW"),
    ({"observation": "team meeting on Friday"}, "ALLOW"),
]


# 30 stopping cases: contradiction / sufficient / missing
STOPPING_SET: list[tuple[dict, str]] = [
    # STOP_EVIDENCE_OK (8 — evidence_status sufficient)
    ({"query": "x", "evidence": ["a"], "evidence_status": "sufficient"}, "STOP_EVIDENCE_OK"),
    ({"query": "x", "evidence": ["a", "b"], "evidence_status": "sufficient"}, "STOP_EVIDENCE_OK"),
    ({"query": "x", "evidence": [], "evidence_status": "sufficient"}, "STOP_EVIDENCE_OK"),
    ({"query": "x", "evidence": ["a"], "evidence_status": "sufficient", "contradiction": True}, "CONTINUE_CONTRADICTION"),
    ({"query": "x", "evidence": ["a"], "evidence_status": "sufficient", "missing_evidence": True}, "CONTINUE_MISSING"),
    ({"query": "x", "evidence": ["a"], "evidence_status": "insufficient"}, "CONTINUE_EVIDENCE_INSUFFICIENT"),
    ({"query": "x", "evidence": ["a"], "evidence_status": "contradiction"}, "CONTINUE_CONTRADICTION"),
    ({"query": "x", "evidence": ["a"], "evidence_status": "missing"}, "CONTINUE_EVIDENCE_INSUFFICIENT"),
    # CONTINUE_CONTRADICTION priority over missing/insufficient
    ({"query": "x", "evidence": ["a"], "evidence_status": "contradiction", "missing_evidence": True}, "CONTINUE_CONTRADICTION"),
    ({"query": "x", "evidence": [], "evidence_status": "contradiction"}, "CONTINUE_CONTRADICTION"),
    # CONTINUE_EVIDENCE_INSUFFICIENT (default insufficient/missing)
    ({"query": "x", "evidence": [], "evidence_status": "insufficient"}, "CONTINUE_EVIDENCE_INSUFFICIENT"),
    ({"query": "x", "evidence": ["a"], "evidence_status": "insufficient", "missing_evidence": True}, "CONTINUE_EVIDENCE_INSUFFICIENT"),
    ({"query": "x", "evidence": [], "evidence_status": "missing"}, "CONTINUE_EVIDENCE_INSUFFICIENT"),
    ({"query": "x", "evidence": [], "evidence_status": "unknown"}, "CONTINUE_EVIDENCE_INSUFFICIENT"),
    # empty evidence default → insufficient
    ({"query": "x"}, "CONTINUE_EVIDENCE_INSUFFICIENT"),
    ({"query": "x", "evidence": []}, "CONTINUE_EVIDENCE_INSUFFICIENT"),
    # contradiction wins
    ({"query": "x", "evidence": ["a"], "contradiction": True}, "CONTINUE_CONTRADICTION"),
    ({"query": "x", "evidence": [], "contradiction": True}, "CONTINUE_CONTRADICTION"),
    ({"query": "x", "evidence": ["a"], "evidence_status": "sufficient", "contradiction": True}, "CONTINUE_CONTRADICTION"),
    ({"query": "x", "evidence": ["a"], "evidence_status": "sufficient", "contradiction": True, "missing_evidence": True}, "CONTINUE_CONTRADICTION"),
    # pure sufficient cases
    ({"query": "x", "evidence": ["a"], "evidence_status": "sufficient"}, "STOP_EVIDENCE_OK"),
    ({"query": "x", "evidence": ["a", "b", "c"], "evidence_status": "sufficient"}, "STOP_EVIDENCE_OK"),
    ({"query": "x", "evidence": ["a"], "evidence_status": "sufficient", "missing_evidence": False}, "STOP_EVIDENCE_OK"),
    ({"query": "x", "evidence": ["a"], "evidence_status": "sufficient", "contradiction": False}, "STOP_EVIDENCE_OK"),
    # missing_evidence alone is insufficient → CONTINUE_EVIDENCE_INSUFFICIENT
    ({"query": "x", "evidence": [], "evidence_status": "missing", "contradiction": False}, "CONTINUE_EVIDENCE_INSUFFICIENT"),
    ({"query": "x", "evidence": ["a"], "evidence_status": "missing", "contradiction": False}, "CONTINUE_EVIDENCE_INSUFFICIENT"),
    # contradicting only the evidence_status but the rule chain still resolves
    ({"query": "x", "evidence": ["a"], "evidence_status": "insufficient", "missing_evidence": False}, "CONTINUE_EVIDENCE_INSUFFICIENT"),
    ({"query": "x", "evidence": [], "evidence_status": "missing", "missing_evidence": True}, "CONTINUE_EVIDENCE_INSUFFICIENT"),
    # edge: nothing explicit, default no_hit, evidence_sufficient=0.05 < 0.85 → CONTINUE_EVIDENCE_INSUFFICIENT
    ({"query": "x", "evidence": ["a"], "evidence_status": "unknown", "missing_evidence": False}, "CONTINUE_EVIDENCE_INSUFFICIENT"),
]


# 20 recall round-trips: persist X, then query for X; the recall should
# contain the persisted content (substring).
RECALL_SET: list[tuple[str, str, str]] = [
    ("Alice lives in Dallas",       "Where does Alice live",             "Dallas"),
    ("Bob started a new job",       "When did Bob start",                "Bob"),
    ("Carol prefers dark mode",     "What does Carol prefer",            "dark mode"),
    ("David was born in 1990",      "When was David born",               "1990"),
    ("The Eiffel Tower is in Paris","Where is the Eiffel Tower",         "Paris"),
    ("Mira planted basil",          "What did Mira do",                  "basil"),
    ("How to bake sourdough",       "How to bake",                       "sourdough"),
    ("The team launched v2",        "What was launched",                 "v2"),
    ("Project started in January",  "When did the project start",        "January"),
    ("Tokyo trip in spring 2024",   "When was the Tokyo trip",           "Tokyo"),
    ("recipe with detailed steps",  "Show the recipe",                   "steps"),
    ("Alice prefers concise text",  "Alice's preference",                "concise"),
    ("Bob likes hiking",            "Bob's hobby",                       "hiking"),
    ("Carol lives in Berlin",       "Carol's city",                      "Berlin"),
    ("user wants weekly reminders", "What does the user want",           "weekly"),
    ("favorite color is blue",      "favorite color",                    "blue"),
    ("facts about giraffes",        "facts about",                       "giraffes"),
    ("the meeting was productive",  "the meeting",                       "productive"),
    ("the report was filed",        "the report",                        "filed"),
]
