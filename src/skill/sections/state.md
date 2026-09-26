# state — inspect every per-node record

`laya-workflow state --dir <store-dir> [--json]`

Reads a `NodeStore` created by `resume` (or `run_persistent`) and prints every
node execution record, in iteration order:

```
dir: /tmp/run
records: 2
last_iteration: 2
  iter=0001 node=        triage action=route    conf=0.500 next=Some("billing") detail=Some("routed to billing")
  iter=0002 node=       billing action=stop     conf=1.000 next=None detail=Some("workflow stopped")
```

`--json` dumps the full record including `state_before` / `state_after`
(complete state snapshots), `payload`, `edge_answer`, `confidence`,
`latency_ms`, `next_node`, `detail`, `error`.

## Why this is useful

* **Audit**: what did node X actually decide, and from what state?
* **Debug**: compare the `state_after` of a bad iteration with a good one.
* **Hand-off**: an agent can hand another agent the store dir; the next agent
  reads `state --dir D --json` and knows exactly where the run stands.

Next: `skill --section resume`, `skill --section replay`, `skill --recipe debug-failure`.
