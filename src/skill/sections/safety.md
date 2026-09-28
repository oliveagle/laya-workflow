# safety — gates every spec goes through

A spec's `policy` object controls what its capabilities may do:

| field | meaning |
|-------|---------|
| `allow_exec` | may spawn processes (goal_runner, exec, shell, agent) |
| `allow_paths` | filesystem roots a path-based capability may touch |
| `allow_hosts` | hosts a network capability may reach |
| `max_timeout_ms` | hard ceiling on any capability timeout |
| `max_output` | cap on captured stdout/stderr |
| `retries` | default capability retry budget |

Plus:

* **Secrets**: `.env` / `*.secrets.json` are gitignored; `validate` prints only
  secret *names* + readiness; errors never leak values (they're redacted).
  Your home directory is rendered as `$HOME` in output paths (not leaked as a
  raw username and no longer masked to `***`).
* **goal_runner**: `runner` must be a known harness (`cxgo`/`cmdgo`); the goal
  doc must be under `allow_paths`; output is capped.
* **Fail-closed**: unresolvable `${env.X}` or missing secret is an error, not a
  silent fallback.

Next: `skill --section validate`, `skill --recipe harden-spec`.
