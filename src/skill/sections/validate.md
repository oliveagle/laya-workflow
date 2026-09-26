# validate — lint a spec before running it

`laya-workflow validate --spec <file>` parses the spec and reports:

* `dsl_version` (current vs. legacy; an older spec is "upgradable", a newer one
  is rejected)
* the `nodes` graph: `start`, each node's `primary_q`, `edge_condition`,
  `edge_default`, `min_confidence`, `has_action`, `max_retries`
* the `capabilities` it references, the `policy` it declares (`allow_exec`,
  `allow_hosts`, `max_timeout_ms`, `max_output`, `retries`)
* required secret **names** and whether they're ready (never their values)
* a warning if the spec hard-codes what looks like a secret

## Example

```
laya-workflow validate --spec dsl/capabilities/goal_runner.json
```

Output (abridged):

```
# dsl_version: 2 (current)
# capabilities: cmd_goal, codex_goal
# policy: allow_exec=true allow_hosts=[] max_timeout_ms=600000 max_output=262144 retries=0
# secrets: none required
```

## When to use it

* Before every `run`/`resume` of a spec you hand-edited.
* In CI / agent onboarding: **validate is the cheapest gate**; it catches
  broken JSON, missing nodes, bad `allow_paths`, unreadable secrets, and
  unknown capability kinds without invoking a model.

Note: `validate` may fail fast on an unresolvable `${env.X}` (fail-closed).
Set the env var (or a `.env`) and re-run.

Next: `skill --section run`, `skill --section safety`.
