# Multi-version workflows

Same workflow name, multiple versions on disk:

```
refund_policy.v1.json   dsl_version 1
refund_policy.v2.json   dsl_version 2
```

Resolution:

* `"workflow": "refund_policy"`      → **highest version** (v2)
* `"workflow": "refund_policy@1"`    → pinned to v1
* `"workflow": "refund_policy@2"`    → pinned to v2
* pin to a version that does not exist → error

Alternative layouts also work: `refund_policy/1.json` + `refund_policy/2.json`, or
`refund_policy/` containing `workflow.json`.

Engine support: `spec::DSL_VERSION` (currently 2). Specs may set `"dsl_version": N`;
omitting it is treated as 1 (legacy). A spec whose `dsl_version` exceeds the engine's
is rejected with a clear message; older specs are accepted and reported as upgradable
by `laya-workflow validate`.
