# list — enumerate specs across the layered roots

`laya-workflow list` resolves its spec roots (highest priority first):

1. `--dsl-dir <path>` / `$LAYA_DSL_DIR` — a **pin** that replaces the layers;
2. **repo** — `.laya-workflow/dsl/` (preferred) or `dsl/`, walking up from the
   cwd and stopping at the git root (so it follows the repo's commits);
3. **user** — `$LAYA_USER_DSL_DIR` → `$XDG_CONFIG_HOME/laya-workflow/dsl` →
   `~/.config/laya-workflow/dsl`;
4. **builtin** — `<crate>/dsl` (shipped with the binary).

It prints the search path (low → high) plus every discoverable spec with its
layer, version, and path. The first root to define a name wins; a same-named
spec in a lower-priority root is listed with `(shadowed by …)`:

```
DSL search path (low → high; later overrides earlier):
  builtin  <crate>/dsl
  user     ~/.config/laya-workflow/dsl  (missing)
  repo     /path/to/repo/dsl

31 spec(s)  (engine dsl_version: 2):
  goal_runner    repo  v2   /path/to/repo/dsl/capabilities/goal_runner.json
  …
```

* Version comes from each spec's `dsl_version` field (missing → treated as 1).
* This is the fastest way for an agent to see "what workflows exist here".

Next: `skill --section validate` (pick one and lint it), `skill --section dsl`.
