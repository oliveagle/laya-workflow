# recipe: harden-spec — before a spec touches anything real

Goal: verify a spec is safe to run with process/network/filesystem powers.

## Checklist

1. **Lint**:

   ```
   laya-workflow validate --spec S
   ```

   Confirm: `dsl_version` current, `capabilities` expected, `policy` explicit.

2. **Policy is intentional** — read the printed line:
   `allow_exec`, `allow_hosts`, `max_timeout_ms`, `max_output`, `retries`.
   An empty `allow_paths` with path capabilities is fail-closed (denied);
   `allow_exec: true` means the spec can spawn processes — say so out loud.

3. **Secrets**: `validate` prints required secret *names* + readiness. Missing
   secrets fail closed. Never embed a secret in the spec (it warns on anything
   that looks like one).

4. **goal_runner / exec capabilities**: runner must be a known harness; goal
   docs must live under `allow_paths`; output is capped by `max_output`.

5. **Smoke it offline first**:

   ```
   laya-workflow run --spec S --state '{…}'
   ```

   Only add `--base-url` after the offline run does what you expect.

Next: `skill --section safety`, `skill --recipe debug-failure`.
