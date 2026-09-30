# Round-3: heuristic vs real-model backend comparison

Date: 2026-09-30. Data: NATURAL_OBSERVATIONS (27 rows, no keyword triggers).

| backend | type acc | admission acc | notes |
|---|---|---|---|
| heuristic (v1, Round-1 spec) | 17.6% (3/17) | 50% (5/10) | keyword-tuned baseline |
| **heuristic (v2, Round-2 spec)** | **88.2% (15/17)** | **100% (10/10)** | +natural vocab |
| LLM (laya-tch :8400, first-match-wins) | 41.2% (7/17) | 50% (5/10) | spec rule order bias |
| LLM (laya-tch :8400, argmax) | 58.8% (10/17) | 50% (5/10) | argmax helps type |

## Findings

1. **Optimised heuristic beats the current LLM backend on this set.** The
   RL-agent model returns flat/low noul probabilities for the 4-way type
   question (many < 0.5), and its admission `should_store` choice over-
   blocks factual observations ("Alice moved to Berlin in 2018" → A/drop).
2. **argmax helps the LLM type classification** (41.2% → 58.8%) but not
   admission. Rule order (first-match-wins) biased toward earlier types.
3. **Production recommendation: default heuristic v2.** Deterministic,
   predictable, ~1.8ms/call vs ~2-3s/call for the LLM backend, zero cost,
   and higher accuracy on this diagnostic. The LLM backend needs either
   threshold/prompt tuning or fine-tuning before it can be the default.

## Remaining type misses (heuristic v2)
- "Python was first released in 1991" → TYPE_PROCEDURAL (procedural "first" token)
- "Deploy by tagging commit and pushing to main" → TYPE_EPISODIC ("deploy" token)

Both are vocabulary collisions: the added tokens help recall but can
mis-fire. A production fix is a backend **ensemble** (heuristic for
high-confidence, LLM for low-confidence) — see goals doc.

## Latency
- heuristic: ~1.3-1.9 ms/call (all tools)
- LLM backend: ~2-3 s/call (per systemone request), 600s timeout configured
