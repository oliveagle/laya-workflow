# Laya Workflow DSL

Declarative workflow specs for the Rust Laya engine. Add or change a workflow by
editing (or adding) a `.json` file here — **no Rust changes or rebuild required**.

Run / validate:

```bash
laya-workflow validate --spec dsl/<name>.json          # print the graph
laya-workflow --base-url http://127.0.0.1:8400 run \
    --spec dsl/<name>.json --state '{"text": "..."}'
laya-workflow export agent_gate                        # built-in Rust workflow -> spec
```

Layout: specs live in folders by domain (`guards/`, `routing/`, `loops/`,
`pipelines/`, `ole_eval/`, `devine/`). `laya-workflow list` walks the tree recursively.
See `ole_eval/README.md` for the ole-eval scenario mapping, and
`devine/README.md` for the devine_utils release/test/integration flow.

Nesting: a node may reference another workflow

  * by name        `"workflow": "support_ticket_router"`  (registry, or recursive tree lookup)
  * by relative path `"workflow": "./billing/refund.json"` (relative to the referring file)
  * from the DSL root `"workflow": "routing/triage_then_moderate"`
  * inline         `"workflow": {"inline": { ... }}`

Referenced workflows are expanded in place and namespaced (`<ref>::<node>`); edges
pointing at the reference node are rewritten to the sub-workflow's entry.
A folder named `<name>/` containing `workflow.json` is also addressable as `<name>`.

Full reference: `docs/benchmarks/laya_workflow_dsl_20260926.md`.

Specs in this directory:

| file | demonstrates |
|------|--------------|
| `agent_command_gate.json` | single node, `gate` (ALLOW/CONFIRM/BLOCK), state projection |
| `support_ticket_router.json` | multi-node routing, `min_confidence`, ordered `threshold` |
| `content_policy_gate.json` | 6-way choice, cross-question probability rules |
| `triage_then_moderate.json` | 3-hop chain with conditional jumps |
| `iterative_refine_loop.json` | self-loop + retry budget + convergence |
| `intake_pipeline_nested.json` | nested reference to `support_ticket_router` |
| `capabilities/refund_with_external_checks.json` | http + agent + exec capabilities |
| `capabilities/exec_preflight_guard.json` | opt-in local command (`policy.allow_exec`) |
| `capabilities/agent_session_probe.json` | external agent/app-server (http or stdio) |
| `capabilities/data_pipeline_local.json` | datetime + text + file + shell + chain |
| `capabilities/integration_hub.json` | rpc + graphql + llm + mcp + vector + webhook + sse |
| `capabilities/ticket_structuring.json` | csv + validate + hash + tokenize + metrics + chain |
| `capabilities/stateful_pipeline.json` | keyvalue + queue + cache(TTL) + cron + notify |
| `capabilities/protocol_services.json` | tcp/udp/redis/nats/mqtt/smtp/s3/prometheus/kafka |
| `versioned/refund_policy.v{1,2}.json` | multi-version coexistence + `name@N` pinning |
| `devine/release_gate.json` | http probe merged into state, then ordered `threshold` (BLOCK → ALLOW) |

External capabilities: declare them under `"capabilities"` and call them from a node
with `{"kind":"call","capability":"<name>","with":{…},"project":{…}}`. An action may
also list prerequisite `"chain": [{"capability":…, "as":…}]` whose results become
`${with.<as>…}` for later steps and the main call.

46 kinds (60 names incl. aliases), e.g. `tcp`, `udp`, `redis`, `nats`, `mqtt`, `smtp`, `s3`, `prometheus`,
`kafka`, `archive`*, `pdf`*, `sql`*, and the earlier 26: `http`, `exec`*, `agent`, `shell`*, `file` (path allow-list), `sqlite`*, `datetime`,
`text`, `rpc`, `graphql`, `llm`, `mcp`*, `vector`, `webhook`, `sse`, `passthrough`, `json`,
`csv`, `xml`, `markdown`, `diff`, `validate`, `math`, `hash`, `graph`, `tokenize`, `cron`,
`keyvalue`, `cache`, `queue`, `metrics`, `notify`  (* = spawns a process or touches the filesystem → gated by
`policy.allow_exec` / `policy.allow_paths`).

Secrets: never inline them. Reference as `${secret.NAME}` (legacy `${env.NAME}` also
works). Values load from the process environment, then `.env` files
(`$LAYA_SECRETS_FILE`, `$LAYA_SECRETS_DIR/*.env`, `<dsl_dir>/.env`, `./.env`), then a
JSON secrets file. A missing secret fails the run instead of sending an empty
credential, and every known value is redacted to `***` in all output.
`laya-workflow validate` prints the required secret *names* plus readiness.
`.env` / `*.secrets.json` are gitignored. See
`docs/benchmarks/laya_workflow_capabilities_20260926.md`.

Goal harnesses: `goal_runner` hands a whole goal doc to an external agent
harness and returns its **externally verified** verdict — `ok` comes from the
runner's acceptance reports / hash checks, never from the model's own claim.
`runner: cxgo` uses codex app-server, `runner: cmdgo` uses command-code headless.
Gated by `policy.allow_exec`, the runner must be a known harness, and the goal doc
must sit under `policy.allow_paths`. Example: `dsl/capabilities/goal_runner.json`.

Web research: `web_search` turns a query into `[{title,url,snippet}]` and `web_fetch`
turns a URL into readable text (`format: text|markdown|raw`). The endpoint is
configurable (`${env.LAYA_WEB_SEARCH_URL}`), so nothing is hard-coded to a public
provider. Only http/https URLs are accepted and redirects are **not** followed —
a `3xx` is reported (`status` + `location`) instead of chasing it to a host that
`allow_hosts` does not list. Example: `dsl/capabilities/web_research.json`.

Where to edit capabilities in a workflow? See the capability reference in
`docs/benchmarks/laya_workflow_capabilities_20260926.md`.

Does Laya get more accurate over time? `laya-workflow improve --dir <dir>` runs the
accuracy self-improvement loop: it scores the apps' reference cases, derives an
update from the misses, and adopts it only if a hold-out split does not regress
(otherwise the update is rejected and recorded). Policy, feedback samples and a
round history persist in `<dir>`, so the loop resumes across sessions. Design,
diagnostics and measured numbers: `docs/benchmarks/laya_accuracy_self_improvement_20260926.md`.

Which capabilities can reach a real service? See
`docs/benchmarks/laya_capability_live_readiness_20260926.md` — an audit of all 45
kinds, with live-tested endpoints for the network ones (web/http/graphql/rpc/llm/
webhook/sse/prometheus/s3/sql/tcp) and explicit ENV-LIMITED notes for the rest.
Note `allow_hosts` must list the API endpoint **and** every page host you fetch.

Local mocks: the protocol/service examples run against `bench/mock_services.py`
(redis/nats/mqtt/smtp/s3/prometheus/kafka/udp/web). Start them with
`bench/mock_services.sh start` (start/stop/status; it waits for the readiness
line, so the service outlives the calling shell). Tests take
`LAYA_MOCK3=host:redis:nats:mqtt:smtp:s3:prom:kafka:udp` and
`LAYA_WEB_PORT=<port>` for the web research cases; when a mock variable is set
but unreachable the suite fails instead of skipping.
