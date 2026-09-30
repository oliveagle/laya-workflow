# GBNF strict gate — A/B effect on `laya-tch /system_one`

Same corpus, same seed (42), 100 requests × 10 mutation classes, same CPU
model. Two runs: one with `--gbnf-strict` OFF, one with it ON.

## Headline

| Metric                         | strict OFF | strict ON | Δ          |
|--------------------------------|-----------:|----------:|------------|
| 5xx responses                  | **20**     | **0**     | -20 (→ 0)  |
| 400 responses                  | 30         | 70        | +40 (clean)|
| 200 responses                  | 50         | 30        | -20        |
| Invariant violations on 200    | 0          | 0         | 0          |
| p50 latency (rejected reqs)   | 0.5 ms     | 0.4 ms    | ≈          |
| p50 latency (accepted reqs)    | 388 ms     | 392 ms    | ≈          |

## Per-class breakdown

| Class                       | OFF: 200/400/5xx | ON: 200/400/5xx |
|-----------------------------|-------------------|------------------|
| `valid (control)`           | 10 / 0 / 0        | 10 / 0 / 0       |
| `missing_instructions`      | 0 / 0 / **10**    | 0 / **10** / 0   |
| `unknown_qtype`             | 0 / 0 / **10**    | 0 / **10** / 0   |
| `missing_questions`         | 0 / 10 / 0        | 0 / 10 / 0       |
| `questions_not_object`      | 0 / 10 / 0        | 0 / 10 / 0       |
| `missing_state`             | **10** / 0 / 0    | 0 / **10** / 0   |
| `extra_top_level_key`       | **10** / 0 / 0    | 0 / **10** / 0   |
| `malformed_json`            | 0 / 10 / 0        | 0 / 10 / 0       |
| `deeply_nested_criteria`    | 10 / 0 / 0        | 10 / 0 / 0       |
| `huge_unicode`              | 10 / 0 / 0        | 10 / 0 / 0       |

## Effect, decomposed

### 1. Catastrophic-failure class eliminated (the big one)
The two rows that flipped 5xx → 400:
* `missing_instructions`: 10 × 500 → 10 × 400 with parser_pos
* `unknown_qtype`:        10 × 500 → 10 × 400 with parser_pos

**The engine used to crash on these inputs.** With strict ON, it returns
a 400 with `gbnf: GBNF rejected (rule=root, pos=N): ...` so the client
knows exactly what's wrong instead of guessing.

### 2. Silent-acceptance class eliminated
Two more rows flipped 200 → 400:
* `missing_state`:     the engine tolerated the missing state and ran the
                       forward pass (silently doing the wrong thing). Now
                       the client gets a 400.
* `extra_top_level_key`: same — silently ignored, now 400.

### 3. Pre-existing per-question checks unchanged
Three rows already 400 with both modes — those are the existing
per-question validation in `to_internal`. Strict doesn't regress them.

### 4. Genuinely valid shapes still accepted
`valid`, `deeply_nested_criteria`, `huge_unicode` — all 30/30 accepted
in both modes. No regression.

## Cost

* p50 latency on accepted requests: 388ms → 392ms (+1%, within noise).
  The pre-gate validation cost is one parse of a small grammar (< 1 ms).
* Rejected requests skip the forward pass entirely (0.4 ms p50).

## Reproduce

```bash
# strict OFF
laya-tch --port 8400 --host 127.0.0.1 --model-dir ~/models/convaiinnovations--laya --device cpu &
python3 bench/gbnf_strict_stress.py --base-url http://127.0.0.1:8400 --seed 42 --out bench/reports/gbnf_off.md

# strict ON
pkill -f "laya-tch.*--port 8400"
laya-tch --port 8400 --host 127.0.0.1 --model-dir ~/models/convaiinnovations--laya --device cpu --gbnf-strict &
python3 bench/gbnf_strict_stress.py --base-url http://127.0.0.1:8400 --seed 42 --out bench/reports/gbnf_on.md
```

