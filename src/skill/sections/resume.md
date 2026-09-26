# resume — continue a partially-completed run

`laya-workflow resume --spec <file> --dir <store-dir> [--from <iter>] [--state '<json>']`

Continues a run whose per-node state was persisted by a previous `resume`
(or `run_persistent`):

* Default: continues from the last committed iteration, materialising the state
  at that point and continuing at the stored `next_node`.
* `--from N`: re-executes from iteration `N+1` (N must already be committed).
* `--state '<json>'`: overrides the initial state (ignores what the store
  materialised) — for a brand-new run on an existing directory.

Every node it executes is written to `dir/runs/000N.json` atomically, so you can
kill it mid-run and resume again.

Next: `skill --section state`, `skill --recipe checkpoint-resume`.
