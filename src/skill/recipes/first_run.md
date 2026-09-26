# recipe: first-run — run a workflow spec end-to-end

Goal: take a workflow JSON you (or someone) wrote and see it run, without a
model server. This is the fastest possible loop.

## Steps

1. Find a spec:

   ```
   laya-workflow list
   ```

2. Lint it (cheapest gate, catches broken JSON / graph / policy / secrets):

   ```
   laya-workflow validate --spec dsl/routing/support_ticket_router.json
   ```

3. Run it on the offline heuristic backend (topology + routing only):

   ```
   laya-workflow run --spec dsl/routing/support_ticket_router.json --state '{"label":"outage"}'
   ```

4. Read the output: `result` (final state), `trace.final_action`
   (`stop` / `escalate` / `converged` / `max_iterations` / `error…`),
   `trace.steps` (per-iteration answer + confidence), `iterations`.

## If it fails

* Validate errors → fix the spec (see `skill --section dsl`).
* `final_action = error_node_missing` → an edge routes to a node name that
  doesn't exist; run `validate` to list the real node names.
* Looks wrong but ran → `skill --recipe debug-failure`.

Next: `skill --section run`, `skill --recipe debug-failure`.
