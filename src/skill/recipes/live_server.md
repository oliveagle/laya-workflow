# recipe: live-server — get real decisions

Goal: run the same spec against a running `laya-tch` HTTP server so the
decisions come from the real model instead of the offline heuristic.

## Steps

1. Make sure the server is up (its `/v1/systemone` endpoint). Nothing in this
   CLI starts the server — it is an external dependency.

2. Add `--base-url` to any execution command:

   ```
   laya-workflow --base-url http://127.0.0.1:8400 run \
       --spec dsl/routing/support_ticket_router.json --state '{"label":"outage"}'
   ```

3. Verify you are actually live: the CLI prints `backend: laya-tch @ <url>`
   (offline prints `backend: offline heuristic`).

4. Re-run `apps` or `describe` against it for the built-in reference cases.

## Notes

* Graph logic is identical online/offline; only the decisions differ.
* A connection error means the server isn't listening — fall back to offline
  (`--base-url` omitted) to keep iterating on topology.
* Never trust heuristic results as "the model says so".

Next: `skill --recipe debug-failure`, `skill --section state`.
