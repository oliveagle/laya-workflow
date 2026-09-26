# apps — run the four built-in apps' reference cases

`laya-workflow apps` runs the reference cases for the four built-in apps
(agent gate, email triage, content moderation, draft scoring) against the
selected backend and reports pass/fail.

* Offline heuristic → plumbing checks only.
* `--base-url http://127.0.0.1:8400` → real decisions scored against the
  reference cases.

`laya-workflow describe <app>` prints the app's graph (nodes, edges, retries) —
useful before running or before exporting it.

Next: `skill --section export` (turn an app into a generic spec you can edit).
