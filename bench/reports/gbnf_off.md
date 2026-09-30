# laya-tch --gbnf-strict stress report

endpoint: `http://127.0.0.1:8400`
corpus size: 100 (seed=42, 10/class × 10 classes)

## Per-class outcome

| class | n | 200 | 400 | 5xx | invar violations | p50 ms | p95 ms |
|---|---:|---:|---:|---:|---|---:|---:|
| `valid (control)` | 10 | 10 | 0 | 0 | — | 390.3 | 394.1 |
| `missing_instructions` | 10 | 0 | 0 | 10 | — | 0.5 | 0.6 |
| `unknown_qtype` | 10 | 0 | 0 | 10 | — | 0.5 | 4.3 |
| `missing_questions` | 10 | 0 | 10 | 0 | — | 0.5 | 0.7 |
| `questions_not_object` | 10 | 0 | 10 | 0 | — | 0.5 | 0.6 |
| `missing_state` | 10 | 10 | 0 | 0 | — | 370.4 | 373.0 |
| `extra_top_level_key` | 10 | 10 | 0 | 0 | — | 386.9 | 392.1 |
| `malformed_json` | 10 | 0 | 10 | 0 | — | 0.3 | 0.7 |
| `deeply_nested_criteria` | 10 | 10 | 0 | 0 | — | 422.7 | 434.3 |
| `huge_unicode` | 10 | 10 | 0 | 0 | — | 2022.6 | 2066.3 |

## 400 rejection buckets

* `non-gbnf-400`: 30

## Latency
  p50 = 349.1 ms, p95 = 2022.6 ms, max = 2066.3 ms

## Headline
* 5xx responses: **20** (target: 0)
* invariant violations on 200 responses: **0** (target: 0)
