# recipe: agent-onboarding — you just got handed `laya-workflow`

Goal: get productive in the next five minutes without reading everything.

## Read in this order (stop when you have enough)

1. `skill` (top-level map) — you probably ran this already.
2. `skill --section overview` — what the tool is, which subcommand matches
   your intent.
3. `skill --section dsl` — the spec shape (this is the data model).
4. `skill --recipe first-run` — validate + run a real spec offline, end-to-end.

## Then, only if you need it

| Your task | Expand |
|-----------|--------|
| Run with a live model | `skill --recipe live-server` |
| Something looks wrong | `skill --recipe debug-failure` |
| Long/killed run | `skill --recipe checkpoint-resume` |
| Undo a bad iteration | `skill --recipe rollback-bad-iter` |
| Ship a spec safely | `skill --recipe harden-spec` |
| Which tests to run | `skill --section tests` (modular, pick 1–3 sections) |

## Ground rules

* `validate` before `run`; offline before `--base-url`.
* Never print secret values; `state --json` output may contain state — treat
  it as sensitive if the workflow handles secrets.
* Don't run the full test suite for every change — use `skill --section tests`.

Next: `skill --recipe first-run`.
