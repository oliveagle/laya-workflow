# recipe: rollback-bad-iter — undo a bad iteration, replay, resume

Scenario: iterations 1..N ran, but iteration K produced a wrong `state_after`.

## Steps

1. Locate K:

   ```
   laya-workflow state --dir D --json | jq '.[] | {iteration,node,action,state_after}'
   ```

2. **Rewind** to K (engine API): materialise the state as of K and truncate all
   records after K:

   ```rust
   let state = workflow.rewind(&mut store, K - 1)?;   // state as of before K
   ```

   Everything after K is gone; nothing re-executed yet.

3. **Replay** node K (CLI) — re-executes only K from its own `state_before`:

   ```
   laya-workflow replay --spec S --dir D --iter K
   ```

   (Or change the conditions first: different `--base-url`, env, spec edit.)

4. **Resume** the rest:

   ```
   laya-workflow resume --spec S --dir D --from K
   ```

   Now K+1… run on top of the corrected state.

## Why this is "local re-entry"

Only the affected node (and its tail) is re-executed; the prefix 1..K-1 stays
byte-identical, so you never pay for (or mask errors in) the whole run.

Next: `skill --section replay`, `skill --section persist`.
