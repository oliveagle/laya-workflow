# orchestrate — bring up a local browser + server before you drive it

A browser workflow needs two local resources to exist *before* the interesting
part: a Chrome with a CDP endpoint, and (usually) an HTTP server at a URL. Both
subcommands below are **idempotent** — "ensure" is a no-op when the resource is
already up — so a workflow can call them on every run without guarding. They are
the generic, repo-agnostic primitives; site/server specifics stay in the owning
repo.

## `browser ensure` — a browser backend on 127.0.0.1:<port>

```sh
laya-workflow browser ensure                        # default backend: chrome
laya-workflow browser ensure --backend chrome       # explicit (same thing)
laya-workflow browser ensure --backend chrome --port 9223
```

`--backend chrome` (the default, and the only one today) ensures a **CDP
Chrome** on an isolated profile: if something already listens on the port it
prints `ok Chrome CDP already listening on 127.0.0.1:<port>` and exits 0.
Otherwise it launches a dedicated Chrome (isolated `--user-data-dir`,
`--no-first-run`) and waits up to ~15s for the endpoint, printing
`ok Chrome CDP up …`. A missing Chrome prints
`fail Google Chrome not found at …` and exits 1.

The **backend selector is the point**: a more suitable backend can be added
later without changing the call shape a workflow already uses. Chrome-specific
knobs (`--chrome-bin`, `--profile`) and env fallbacks (`CHROME_BIN` /
`LAYA_CDP_PROFILE`; port via `LAYA_CDP_PORT`) apply to that backend — flags win
over env. `chrome ensure` remains as a hidden alias for the chrome backend.

Point a spec's `chrome_cdp` capability endpoint at `http://127.0.0.1:<port>` and
`ensure` it via an `exec` capability — or just set `launch: true` on the
`chrome_cdp` capability and let the engine launch it.

## `server ensure | start | stop | status` — a local HTTP server

```sh
# start on demand, capture the base URL, leave it running (daemon)
laya-workflow server ensure --port 18766 \
  --command 'laya-workflow mock serve --web 18766'

# same, but stay in the foreground (Ctrl-C / SIGTERM stops it)
laya-workflow server start --port 18766 \
  --command 'laya-workflow mock serve --web 18766'

laya-workflow server status --port 18766   # RUNNING <base> (pid N) | STOPPED
laya-workflow server stop   --port 18766   # SIGTERM + clean state files
```

* **Liveness** = "the port answers HTTP at all" (any 2xx–5xx), so a
  `laya-workflow mock serve --web` counts as up even on a default `/healthz` 404; pass
  `--health-path` when the server has a real health endpoint.
* `ensure` cold-starts `--command` as a **daemon** when nothing healthy answers,
  then prints `BASE=http://127.0.0.1:<port>`; when a server is already up it
  prints the `RUNNING` line + `BASE=…` and exits 0. `status` exits **1** when
  stopped (so a spec/shell can branch on it).
* State files: `/tmp/laya-ensure-server-<port>.{pid,base,log}` — identical to the
  old script, so old/new invocations interoperate.

## Putting it together

`dsl/browser/browser_orchestrate_probe.json` is the end-to-end demo:

```sh
laya-workflow run --spec dsl/browser/browser_orchestrate_probe.json \
  --state '{"cdp_port":9222,"port":18766,"server_cmd":"laya-workflow mock serve --web 18766","url":"http://127.0.0.1:18766/"}'
```

graph: `browser ensure` → `server ensure` (cold-starts `server_cmd` if the port is
dead) → open → wait_htmx → assert → done. `laya-workflow` must be on `PATH`
because the spec calls it through an `exec` capability.

The historical `scripts/laya-ensure-chrome.sh` / `laya-workflow server`
are now thin shims that `exec` these subcommands (kept for existing callers like
`devine_int`; override the target with `LAYA_WORKFLOW_BIN`).
