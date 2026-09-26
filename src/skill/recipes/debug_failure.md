# recipe: debug-failure — a run produced a wrong result

Do **not** re-run blindly. Follow this order; each step narrows the cause.

## Step 1 — was it the graph or the decision?

Run the same spec **offline**:

```
laya-workflow run --spec S --state '…'
```

* If offline also routes wrong → the spec (edges / conditions / actions) is
  wrong. Fix topology: `validate` + `skill --section dsl`.
* If offline routes right but live is wrong → the model decision or the
  backend connection is the problem. Check `trace.steps[*].confidence`.

## Step 2 — what did each node actually see and return?

For a persisted run:

```
laya-workflow state --dir D --json | jq '.[] | {iteration,node,action,confidence,edge_answer,state_after}'
```

Compare the `state_after` where things went bad with the expectation. The record
contains the full state snapshot, so you can tell whether the *input* was
already corrupted or the *decision* was wrong.

## Step 3 — reproduce just that iteration

```
laya-workflow replay --spec S --dir D --iter <N>
```

Change one thing (different env, `--base-url`, different seed) and replay only
that node — everything else stays untouched.

## Step 4 — if you need to roll the tail back

`rewind` (engine API) materialises the state at `N` and truncates later records,
then `resume --from N` continues. See `skill --recipe rollback-bad-iter`.

Next: `skill --section state`, `skill --section replay`.
