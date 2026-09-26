# optimize — Laya-driven optimizer loop

`laya-workflow optimize --steps N [--strategies s1,s2]`

Runs a propose → evaluate → Laya `continue`/`strategy` loop for `N` steps. The
Laya node decides whether to keep optimizing and which strategy to switch to.

* `--strategies` lists the strategy names the strategy node may select
  (default `aggressive_step,conservative_step,random_restart`).
* Use the offline backend for plumbing, `--base-url` for real decisions.

Next: `skill --section improve` (the accuracy-learning sibling).
