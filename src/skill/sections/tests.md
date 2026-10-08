# tests — the offline test runner is modular

`laya-workflow-tests` is the offline test binary. It is **modular**: each
`[section]` is selectable by prefix match:

```
./target/release/laya-workflow-tests            # full suite
./target/release/laya-workflow-tests --list     # list sections
./target/release/laya-workflow-tests edge node  # just those (prefix match)
./target/release/laya-workflow-tests persist    # persistence section
./target/release/laya-workflow-tests capability-timeouts
LAYA_TEST_SECTIONS=edge,spec ./target/release/laya-workflow-tests
```

* Pick 1–3 sections most relevant to your change; **don't run the full suite for
  every PR** (that's only for release / big refactors).
* `laya-workflow dsl` validates + runs every spec in the DSL tree.
* `cargo run -p laya-tch --bin bench_parity` compares model logits against the frozen PyTorch reference (only when touching `LayaModel`).

Next: `skill --section overview`, `skill --section run`.
