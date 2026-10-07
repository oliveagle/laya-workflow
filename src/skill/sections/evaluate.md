# evaluate — labeled-dataset decision evaluation

`laya-workflow evaluate --spec <file> --dataset <file.jsonl> [--split all|development|holdout] [--departments csv] [--output <path>] [--freeze]`

Port of the awesome-jev `evaluations/run.py` pattern: measure a workflow's
*decision quality* on tickets you label first — the step between "the graph
runs" and "the decisions are right".

## What it measures
* **department accuracy** — did the workflow route to the right team?
* **urgency accuracy** — when the workflow decided an urgency (not "review"),
  was it right? Cases left in "review" are counted separately, not dropped.
* **review rate / automatic coverage** — what fraction the workflow chose not
  to decide, and what it committed to.
* **unsafe automatic** — automatic assignments that turned out wrong or that
  the dataset flagged `require_review`.
* **required_review_missed** — dataset cases the spec *should* have queued for
  human review but committed to instead (the dangerous direction).
* **confusion matrix** — expected → observed route, so you can see *which* way
  decisions drift.
* **slice breakdown** — per labelled slice (e.g. `clear-technical`,
  `ambiguous`), each with its own denominator.
* **review queue** — every case that landed in review or was an unsafe
  automatic assignment, for targeted follow-up.

## The JSONL dataset
One object per line:

```json
{"id":"t-1","split":"development","slice":"clear-technical",
 "message":"CSV export crashes, report due today",
 "expected_department":"technical","expected_urgency":"high","require_review":false}
```

Validation is strict: duplicate `id`, unknown `expected_department`, invalid
urgency (only `high` | `ordinary`), invalid split (only `development` |
`holdout`) and non-bool `require_review` are all hard errors. `id`, `split`,
`slice`, expected labels, and `require_review` never reach the workflow state —
a leak cannot inflate a score.

## The evaluation block
The spec declares where to read the decision out of the result state:

```json
{"evaluation": {"route": "route", "urgency": "urgency", "confidence": "confidence"}}
```

The engine reads `result.<route>` / `result.<urgency>` / `result.<confidence>`
from the finished workflow's `to_json()` payload.

## Holdout freeze
With `--freeze` on a holdout run, the spec's sha256 fingerprint is recorded
beside the output. A later holdout run against a *changed* spec refuses to
execute:

```
holdout evaluation refused: spec hash changed (recorded <fp> vs now <fp2>);
tune only on development cases, then author a fresh holdout
```

This mirrors awesome-jev's fingerprint rule: holdout numbers are only
meaningful against a frozen configuration.

## Output
Each aggregate carries `{"numerator": n, "denominator": d, "value": r}` —
so a missing decision is counted, never silently dropped. Pass `--output`
to write the summary to a JSON file (refuses to overwrite) plus a
`<output>.fingerprint` file.

## Typical flow
1. Label 20–100 tickets into `dataset.jsonl`, marking ~30% `holdout`.
2. `laya-workflow evaluate --spec s.json --dataset d.jsonl --split development --output runs/dev.json`
3. Inspect `slices` + `review_queue`; tune the spec's questions / thresholds.
4. `--split holdout --freeze --output runs/hold.json` to record a frozen score.
5. After spec edits: re-freeze requires authoring a fresh holdout split.

Next: `skill --section improve` (continuous self-improvement on top of evaluation).
