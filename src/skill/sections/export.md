# export — turn a built-in app into a generic spec

`laya-workflow export <app>` prints the app's workflow as a JSON spec you can
save and hand-edit.

**Read the stderr note.** Every built-in app (`agent_gate`, `email_triage`,
`content_moderation`, `draft_scorer`) routes inside a **native Rust closure** —
real code that cannot be serialized to data. `export` marks those nodes
`"action": {"kind": "rust"}`, and `from_spec` rejects that kind, so the emitted
spec is a **reference graph**: `laya-workflow validate --spec …` and
`laya-workflow run --spec …` will refuse it (`unknown action kind "rust"`). The
`export` command prints a `note:` to stderr saying exactly this, naming the
native nodes; stdout stays a clean JSON document.

So the spec is useful for *seeing* an app's questions, edges and thresholds —
not for re-running it as-is. To own the logic as data, author an equivalent
`@decision` workflow instead, where the routing is a built-in action
(`threshold` / `gate` / `combine` / `rubric` / …):

    laya-workflow bdd scaffold --name <wf> --cat <cat> --intent "..."

The generic-spec path is fully portable for workflows whose actions are all
built-in kinds (see `skill --section dsl`); only the four canned apps carry
native actions.

Next: `skill --section dsl` (spec shape), `skill --section validate`.
