# improve — accuracy self-improvement

`laya-workflow improve --dir <dir> [--rounds N] [--score-only]`

Measures decisions, learns from misses under a **hold-out gate** (adopt an
update only if it doesn't regress the held-out split), and persists the accepted
policy in `<dir>/policy.json` with `samples.jsonl` + `rounds.jsonl` history.

* `--score-only` just reports current accuracy, changes nothing.
* The loop resumes across sessions because everything is persisted in `dir`.
* This is the engine's built-in "learn from feedback" path.

Next: `skill --section safety` (what the gate enforces).
