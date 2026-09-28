# dsl — the workflow spec shape

A spec is a JSON file:

```json
{
  "name": "refund_policy",
  "dsl_version": 2,
  "description": "…",
  "start": "band",
  "max_iterations": 6,
  "convergence_window": 3,
  "convergence_eps": 0.0001,
  "policy": { "allow_exec": true, "allow_hosts": [], "max_timeout_ms": 600000,
              "max_output": 262144, "retries": 0, "allow_paths": ["${env.X}"] },
  "capabilities": { "name": { "kind": "…", … } },
  "nodes": [
    { "name": "route", "primary_q": "q", "questions": {…},
      "edge": {"condition": {"A": "next"}, "default": "STOP"},
      "min_confidence": 0.0, "max_retries": 1,
      "action": {"kind": "call", "capability": "…", "with": {…}, "project": {…}} }
  ]
}
```

* `dsl_version` 2 is current; missing → legacy 1 (accepted, reported upgradable).
* Version pinning in references: `"workflow": "name@2"`.
* Folder layouts work: `name/1.json`, `name/workflow.json`, `name.vN.json`.
* Actions include `copy_keys`, `threshold`, `gate`, `call`, `capability` kinds —
  see `docs/benchmarks/laya_workflow_capabilities_20260926.md` for the catalogue.

Next: `skill --section plugins` (the `kind: "plugin"` extension seam),
`skill --section validate`, `skill --section safety`.
