# laya-mem evaluation report (2026-09-30 16:10)

- binary: `<repo>/target/debug/laya-workflow`
- spec dir: `<repo>/dsl/laya_mem`

## Gate metrics

| task | n | want_acc | oracle_agree | avg_ms | p50_ms | p95_ms |
|------|---|----------|--------------|--------|--------|--------|
| admission | 30 | 100.0% | 100.0% | 1.8 | 1.8 | 1.9 |
| memory_type | 30 | 100.0% | 100.0% | 1.7 | 1.7 | 1.8 |
| stopping | 29 | 96.6% | 100.0% | 1.3 | 1.3 | 1.4 |

**Overall: PASS**