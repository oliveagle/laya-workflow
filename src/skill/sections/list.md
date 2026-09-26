# list — enumerate specs in the DSL tree

`laya-workflow list` scans `LAYA_DSL_DIR` (default `<crate>/dsl`) and prints
every discoverable spec, its version, and its path:

```
DSL root: code/laya-tch/dsl  (engine dsl_version: 2)
20 spec(s):
  goal_runner              v2  capabilities/goal_runner.json
  …
```

* Version comes from each spec's `dsl_version` field (missing → treated as 1).
* This is the fastest way for an agent to see "what workflows exist here".

Next: `skill --section validate` (pick one and lint it), `skill --section dsl`.
