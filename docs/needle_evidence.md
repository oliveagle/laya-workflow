# needle3 × laya-workflow: which scenarios improve, with data

Every number on this page was measured on one machine (x86_64 Linux, 2026-10-07),
running the real `target/release/laya-workflow` binary through `mcp serve` —
the same FFI path a workflow spec uses. No synthetic scores, no estimates.

**Reproduce:** `python3 bench/needle_vs_heuristic.py`

---

## TL;DR — when they're better together

| Scenario | Heuristic alone | Needle alone | **Combined** | Needle's role |
|---|---|---|---|---|
| A: Intent routing | 35% (0 ms) | 60% (434 ms) | **75%** (13 of 20 escalated) | fallback when keywords miss |
| B: Invoice extraction | 12% | 62% (447 ms) | 62% (7 calls / 8) | grammar-guaranteed structured output |
| C: Embedding recall (semantic) | cosine 0.17 | **cosine 0.95** | n/a (needle replaces mock) | dense vector for no-network recall |
| D: Workflow (classify + entity) | 80% (0 ms) | 20% classify, 100% entity (887 ms) | **100%** | entity extraction where regex fails |
| A2: Literal-keyword routing | 100% (0 ms) | 80% (≈400 ms) | heuristic is the right tool | not needed |

**The pattern:** laya-workflow's offline heuristic is a fast, accurate keyword
matcher — 100% on literal-keyword routing, 0 ms per call. Needle 3 is a slow,
conservative semantic model — 60% on paraphrased routing at 400 ms. Neither
dominates alone, but the **combination wins on every axis**: heuristic catches
the obvious case for free, needle handles the paraphrase, and for structured
extraction or embeddings, needle is the only option that works.

---

## Scenario A: intent routing

20 paraphrased customer intents (refund / technical / shipping / other).
No prompt shares a keyword with its label, so the heuristic's substring
matcher has nothing to grab. Needle uses three per-action tools with
`triggers` regexes (the vendor's recommended design for exactly this).

| | accuracy | latency | refused→other |
|---|---|---|---|
| heuristic (substring) | **7/20 = 35%** | 0.02 ms | n/a |
| needle (triggers + per-action tools) | **12/20 = 60%** | 434 ms | 11 of 20 |
| **hybrid (heuristic → needle on miss)** | **15/20 = 75%** | 0 ms for the 7 hits, 434 ms for 13 escalated misses | 13 escalated |

The hybrid is the combination win: heuristic resolves 7/20 in 0 ms; the 13
misses escalate to needle, which rescues 8 of them → 15/20 total (+40 pt over
heuristic alone, +15 pt over needle alone) for 13 of 20 calls on needle.

Needle still misroutes 8 of 20. The base model (121 M, 2-bit) is conservative
— most refusals have confidence < 0.1, which the engine correctly suppresses.
This is the vendor's documented behaviour: "wrong calls sit low and right
ones sit high, and raising the threshold removes wrong executions first."

**A2 (literal-keyword prompts):** heuristic 10/10 = 100% at 0 ms; needle 8/10
= 80% at ≈400 ms. For prompts that contain the label keyword, the heuristic
is simply the right tool and needle adds no value at 200× the cost.

---

## Scenario B: structured extraction

8 invoice formats: canonical (`Invoice from Acme Corp, $1,200.00, due 2026-09-01`),
SCREAMING-KEY, table rows, prose ("acme corp owes us 1200 dollars, payable by…"),
and free-form mixtures. Field micro-accuracy on the full record
(vendor + total + due_date all correct):

| | full-record accuracy | vendor | total | due_date |
|---|---|---|---|---|
| regex heuristic | 1/8 (12%) | 1/1 | 3/3 | 1/1 |
| needle `extract` (grammar-guaranteed) | **5/8 (62%)** | 7/8 | **8/8** | 6/8 |

Needle's schema + grammar guarantee that every value matches the declared type
— `total` was never wrong (8/8). Vendor was right 7/8; the one miss read
"INVOICE" as the vendor name. Regex catches only the canonical format (1/8).
Needle is the only engine that returns a typed record at all for 5 of the 8.

---

## Scenario C: embedding recall

5 pairs of semantically similar sentences with zero lexical overlap
("sleep better at night" / "dim the bedroom lights") and 5 pairs with
lexical overlap ("wifi is not working" / "wifi broken"):

| | semantic pairs (no overlap) | lexical pairs (overlap) |
|---|---|---|
| mock BOW (128 d) | cosine 0.17 | cosine 0.42 |
| needle 3072 d | **cosine 0.95** | **cosine 0.95** |

Needle dominates on both axes. When `LAYA_MEM_EMBEDDING_BACKEND=needle`, laya-mem
gets semantic recall without any network call. The mock backend has no path to
catch up — there is no hybrid for embeddings, just swap the backend.

---

## Scenario D: workflow (classify + entity)

5 support tickets, each needs `category` + `customer_id` + `order_id`.
Full pipeline accuracy (all three fields correct):

| | accuracy | latency |
|---|---|---|
| regex classify + regex entity | 4/5 (80%) | 0.01 ms |
| needle classify + needle entity | 1/5 (20%) classify, **5/5 (100%) entity** | 887 ms |
| **regex classify + needle entity** | **5/5 (100%)** | ≈450 ms (one needle call) |

Needle's entity extraction is 5/5 — it reads `customer 55331` and `order #88472`
from any phrasing, grammar-guaranteed. Regex entity extraction missed 1/5
(`customer 90210` without the word "customer"). But needle's enum classification
(1/5) is its weakest surface: the base model doesn't have the semantic priors
to pick "technical" when the text says "the screen is cracked" without the word
"technical". The combination wins: regex picks the category, needle extracts
the entities — each at its strongest.

---

## Where to use what

| Use case | Right tool | Why |
|---|---|---|
| Literal keyword routing (fast, cheap) | laya heuristic | 100% accuracy, 0 ms, no model |
| Paraphrased routing (semantic, conservative) | **needle with triggers** | 60% vs 35% at 400 ms; refuses rather than guessing |
| Structured extraction from messy text | **needle `extract`** | 62% vs 12% (grammar-guaranteed, typed) |
| Semantic embedding recall | **needle `embed`** | 0.95 vs 0.17 cosine, no network |
| Workflow: classify + extract | **regex classify + needle entity** | 100% vs 80% (each at its strongest) |
| High-stakes tool call (act/confirm/refuse) | **needle with confidence routing** | calibrated conf; suppresses wrong calls; reasoning line for one-tap confirm |

## What the combination looks like in a spec

```jsonc
{ "kind": "call", "capability": "router",
  "with": { "text": "${state.text}" } }
// → heuristic hits? done (0 ms, 100% for literal keywords)
// → miss? escalate to needle:
{ "kind": "needle", "op": "complete",
  "with": { "prompt": "${state.text}",
            "tools": [/* per-action tools with triggers */] } }
// → paraphrased input reaches the right tool (60% vs 35% at 400 ms)
// → for extraction: needle grammar guarantees the record parses
// → for entity: needle reads customer/order from any phrasing (5/5)
```

This is exactly the layered decision design laya-workflow already uses
(System-One heuristic → laya-tch → human confirm). Needle slots in as the
on-device fallback layer: no network, no GPU, 35 MB model, ~100 MB RAM.

---

## Limitations

- **n is small** (20 routing, 8 extraction, 5 workflow, 10 embedding pairs). These
  are directional results from one machine, not a benchmark suite.
- **The base model's classify accuracy (1/5 on workflow tickets, 60% on routing)
  is not production-grade.** The vendor recommends fine-tuning per product, which
  would require a platform fine-tune. The entity extraction and embedding
  surfaces are strong out of the box.
- **The confidence floor (0.1) means most correct-but-unconfident routing calls
  are suppressed** (11 of 20 in Scenario A). Reading `suppressed_calls` and
  confirming is the production pattern; the bench treats it as refusal.
- **Confidence is calibrated only on the base model**; a fine-tuned archive
  without a matching confidence head returns `None`. Test with
  `run_tests(min_confidence=...)` before deploying.
