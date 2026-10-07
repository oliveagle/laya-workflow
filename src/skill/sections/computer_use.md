# computer-use — bounded UI decision cycle

`dsl/capabilities/computer_use.json` · capability `kind: "computer_use"`

Port of awesome-jev `examples/computer-use`: one bounded observation→decision→execution
cycle for filling a form field, with four stages kept strictly apart.

## The four stages

| Stage | Where | Code / model |
|---|---|---|
| 1. **fingerprint** | `computer_use` op `fingerprint` | code — sha256 of the canonical (keys sorted) observation, captured **before** inference |
| 2. **prepare** | action `kind: "ui_proposal"` | code — validated answers become a bounded proposal; below `min_confidence` → `human_review`; `wait`/`blocked` stop the cycle |
| 3. **step** | `computer_use` op `step` | code — the executor rechecks: ready → surface allow-list → **stale fingerprint** → field permission scope → target enabled → at most one `fill` |
| 4. **verify** | `computer_use` op `verify` | code — an independent oracle; expected IDs/values live in the capability config and are **never** part of the state the model sees |

```
snapshot ─fingerprint→ [model: operation/field/amount] ─ui_proposal→ proposal
                                                                    │
                        step ◄── rechecks fingerprint+scope+enabled ┘
                          │ (at most one field changes, nothing submits)
                       verify ── independent oracle ──► verified | failed
```

## Why the fingerprint matters (TOCTOU guard)

A model decides on an observation; the UI can change between observation and
execution. `step` re-hashes the **fresh** snapshot and refuses with
`stale observation; choose again using fresh state` if it no longer matches the
proposal's captured hash. Re-order keys freely: the hash is canonical
(sorted at every depth), matching awesome-jev's `sort_keys=True`.

## What the executor cannot do

`step` has exactly one effect: set `value` on one enabled textbox inside
`allowed_fields` on `allowed_surface`, then bump `revision`. There is no op
that can navigate, click, submit, run a script or generate text; `submitted`
stays `false` and the oracle checks it. A `done` proposal (no action) still
runs the oracle — a premature "I'm done" fails the checks.

## Offline demo

```bash
laya-workflow run --spec dsl/capabilities/computer_use.json --state '{
  "goal": "Fill accounts payable with billing_contact, leave delivery, read amount due. Do not submit.",
  "values": {"billing_contact": "billing@example.invalid"},
  "snapshot": {"surface": "https://invoice.example.invalid/fixture", "revision": 1, "submitted": false,
    "elements": [
      {"id":"e1","role":"textbox","label":"Delivery contact","enabled":true,"value":"shipping@example.invalid"},
      {"id":"e2","role":"textbox","label":"Accounts payable","enabled":true,"value":""},
      {"id":"e3","role":"button","label":"Pay invoice","enabled":true}],
    "texts": [{"id":"t1","label":"Subtotal","text":"€100.00"},
              {"id":"t2","label":"Amount due (tax included)","text":"€120.00"}]}}'
```

Result: `e2` filled, `e1` untouched, `revision` 1→2, `submitted` false,
`verify_status: verified` (5/5 checks). Heuristic classifies via
`heuristic.match_rules`; a real backend answers the same three questions.

Tests: `laya-workflow-tests computer-use` (39 checks) cover the happy path,
stale refusal, surface/field scope refusal, disabled target, non-fill action,
non-ready proposal, oracle failures, and the `ui_proposal` confidence floor.

Next: `skill --section evaluate` (measure decision quality on labeled tickets).
