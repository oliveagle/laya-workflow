# recipe: checkpoint-resume — survive a process kill

Goal: prove that a run's per-node state survives a restart and resumes
correctly.

## Steps

1. Start a run against a store dir:

   ```
   laya-workflow resume --spec S --dir /tmp/run --state '{"…": …}'
   ```

   (For a *fresh* run this commits each node to `/tmp/run/runs/000N.json`
   atomically as it goes.)

2. Kill the process at any point (Ctrl-C / SIGKILL). This is the point of the
   feature: no record is half-written (temp + rename).

3. Inspect what survived:

   ```
   laya-workflow state --dir /tmp/run
   ```

4. Continue:

   ```
   laya-workflow resume --spec S --dir /tmp/run
   ```

   It materialises the state at the last committed iteration and continues at
   that node's `next_node`. Add `--from N` to deliberately re-execute from
   iteration `N+1`.

5. Confirm the final result matches a straight-through run (the `persist`
   test section asserts exactly this).

## When to use it

Long-running workflows, multi-agent hand-offs (the store dir *is* the hand-off
artifact), and any workflow where re-running from scratch is expensive.

Next: `skill --section persist`, `skill --section state`.
