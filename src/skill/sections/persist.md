# persist — how per-node persistence works

Runs are **not** persisted by default (`run` is in-memory). To persist, use a
`NodeStore`:

* API: `ResilientWorkflow::run_persistent(backend, &mut store, initial_state, resume_from)`
* Disk: `<dir>/manifest.json` + `<dir>/runs/0001.json`, `0002.json`, …

Each record holds the **complete observable state of one node execution**:

| field | meaning |
|-------|---------|
| `node` / `iteration` | which node, which step |
| `timestamp_ms` | when |
| `state_before` / `state_after` | full state snapshot around the execution |
| `payload` | node action result |
| `action` / `edge_answer` / `confidence` / `latency_ms` | the decision |
| `next_node` / `detail` / `error` | routing + context |

Operations:

* `read_all()` → all records, iteration order (tracking)
* `materialise_state(n)` → rebuild state as of iteration `n`
* `delete_from(n)` → truncate the tail (rollback)
* `rewind(store, n)` → materialise + truncate in one call
* `replay(backend, store, n)` → re-run just node `n`

Writes are atomic (temp + rename), so a crash never corrupts a record.

Next: `skill --section state`, `skill --section resume`, `skill --recipe checkpoint-resume`.
