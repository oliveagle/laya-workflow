# run — execute a spec end-to-end

`laya-workflow run --spec <file> --state '<json>'`

* `--state` is the initial workflow state (default `{}`).
* `--query '<text>'` is shorthand for putting `query` into that state. If both
  flags are present, `--query` overrides `state.query`.
* Without `--base-url` it uses the **offline heuristic backend** — topology and
  routing logic run, but decisions are canned heuristics, so use it for plumbing
  checks, not for real accuracy.
* With `--base-url http://127.0.0.1:8400` it drives the live `laya-tch` server.

The output is the full `WorkflowOutcome` as JSON: `result` (final state),
`trace` (steps, final_action, latency), `history`, `node_visits`, `iterations`.

## Example

```
laya-workflow run --spec /tmp/persist_demo2.json --state '{"x":"A"}'
# equivalent shorthand when the spec reads state.query
laya-workflow run --spec dsl/browser/google_research.json --query "jev ai"
```

## When to use run vs resume

* Fresh start → `run`.
* Resume a killed/long run from its last committed node → `resume --dir D`
  (see `skill --section resume`).

## What if the result looks wrong?

Do **not** re-run blindly. Use `skill --recipe debug-failure` — it walks you
through checking the trace, then `state --json` to see per-node state snapshots.

Next: `skill --section state`, `skill --section safety`.
