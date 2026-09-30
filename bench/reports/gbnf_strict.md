# laya-tch --gbnf-strict stress report

endpoint: `http://127.0.0.1:8400`
corpus size: 100 (seed=42, 10/class × 10 classes)

## Per-class outcome

| class | n | 200 | 400 | 5xx | invar violations | p50 ms | p95 ms |
|---|---:|---:|---:|---:|---|---:|---:|
| `valid (control)` | 10 | 10 | 0 | 0 | — | 338.5 | 375.5 |
| `missing_instructions` | 10 | 0 | 10 | 0 | — | 0.5 | 0.9 |
| `unknown_qtype` | 10 | 0 | 10 | 0 | — | 0.7 | 3.9 |
| `missing_questions` | 10 | 0 | 10 | 0 | — | 0.4 | 0.8 |
| `questions_not_object` | 10 | 0 | 10 | 0 | — | 0.6 | 0.9 |
| `missing_state` | 10 | 0 | 10 | 0 | — | 0.4 | 0.6 |
| `extra_top_level_key` | 10 | 0 | 10 | 0 | — | 0.6 | 0.9 |
| `malformed_json` | 10 | 0 | 10 | 0 | — | 0.6 | 1.1 |
| `deeply_nested_criteria` | 10 | 10 | 0 | 0 | — | 366.2 | 399.5 |
| `huge_unicode` | 10 | 10 | 0 | 0 | — | 1864.6 | 1983.8 |

## 400 rejection buckets

* `gbnf-structural`: 70

## Latency
  p50 = 0.7 ms, p95 = 1864.6 ms, max = 1983.8 ms

## Headline
* 5xx responses: **0** (target: 0)
* invariant violations on 200 responses: **0** (target: 0)
