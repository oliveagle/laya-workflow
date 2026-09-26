# replay — re-execute a single iteration in place

`laya-workflow replay --spec <file> --dir <store-dir> --iter <N>`

Re-runs **only** iteration `N`, using that record's `state_before`, and atomically
overwrites the record with the new result.

* It does not touch any other iteration, so the rest of the run's history stays
  intact.
* Ideal for "that node made a bad call — let's see what happens with a different
  backend / different model / different env".

Combined with `rewind` (engine API) this is how you do **local re-entry**: roll
back the tail, replay one node, then resume.

Next: `skill --section state`, `skill --recipe rollback-bad-iter`.
