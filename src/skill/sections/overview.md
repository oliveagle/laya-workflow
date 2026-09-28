# overview — what laya-workflow is

`laya-workflow` is the Rust CLI for the **Laya workflow engine**: a JSON spec of
decision nodes ("ask typed questions → route by the primary answer") run against
either:

* **offline heuristic backend** (default; graph/plumbing only, no model) — great
  for validating a spec, iterating on topology, and running the test suite;
* **a live `laya-tch` HTTP server** (`--base-url http://127.0.0.1:8400`) for real
  model decisions.

Think of it as: `validate` before you run, `run` to execute, `state`/`resume`/
`replay` to inspect and control a run, `skill` to learn any part on demand.

## The six things it can do

1. **Built-in apps** — `apps`, `describe`, `demo`: the four canned workflows
   (agent gate, email triage, content moderation, draft scoring) plus a
   composition demo.
2. **Run a spec** — `validate` then `run --spec S --state '{"…"}'`.
3. **Optimize / improve** — `optimize` (Laya-driven proposal loop) and
   `improve` (accuracy self-improvement under a hold-out gate).
4. **Per-node persistence** — `state`, `resume`, `replay` against a
   `NodeStore` (`<dir>/runs/0001.json` + `manifest.json`).
5. **Extend it** — `plugin install | list | dir`: site logic lives in sandboxed
   **Rhai plugins** (`kind: "plugin"`), not in the binary. See
   `skill --section plugins`.
6. **Learn it** — `skill` (this command).

## Which subcommand do I want?

| You want to… | Use |
|--------------|-----|
| Check a spec is well-formed before touching a model | `validate --spec S` |
| Execute a spec once, see the result | `run --spec S --state '{}'` |
| See the graph of a built-in app | `describe <app>` |
| Re-run a partially-completed run | `resume --spec S --dir D` |
| Inspect what each node returned | `state --dir D --json` |
| Re-execute just one iteration | `replay --spec S --dir D --iter N` |
| Drive real model decisions | add `--base-url http://127.0.0.1:8400` |

Next: `skill --section dsl` (spec shape) → `skill --section validate` →
`skill --section run`.
