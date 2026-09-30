# laya-tch --gbnf-strict stress report

endpoint: `http://127.0.0.1:8400`
corpus size: 100 (seed=42, 10/class × 10 classes)

## Per-class outcome

| class | n | 200 | 400 | 5xx | invar violations | p50 ms | p95 ms |
|---|---:|---:|---:|---:|---|---:|---:|
| `valid (control)` | 10 | 10 | 0 | 0 | — | 382.9 | 393.4 |
| `missing_instructions` | 10 | 0 | 10 | 0 | — | 0.4 | 0.6 |
| `unknown_qtype` | 10 | 0 | 10 | 0 | — | 0.4 | 3.8 |
| `missing_questions` | 10 | 0 | 10 | 0 | — | 0.4 | 0.8 |
| `questions_not_object` | 10 | 0 | 10 | 0 | — | 0.5 | 0.8 |
| `missing_state` | 10 | 0 | 10 | 0 | — | 0.4 | 0.6 |
| `extra_top_level_key` | 10 | 0 | 10 | 0 | — | 0.4 | 0.6 |
| `malformed_json` | 10 | 0 | 10 | 0 | — | 0.4 | 0.6 |
| `deeply_nested_criteria` | 10 | 10 | 0 | 0 | — | 403.6 | 426.6 |
| `huge_unicode` | 10 | 10 | 0 | 0 | — | 1974.5 | 2080.0 |

## 400 rejection buckets

* `gbnf-structural`: 70

## Latency
  p50 = 0.6 ms, p95 = 1974.5 ms, max = 2080.0 ms

## Headline
* 5xx responses: **0** (target: 0)
* invariant violations on 200 responses: **0** (target: 0)
