# laya-mem evaluation report (2026-09-30 16:05)

- binary: `<repo>/target/debug/laya-workflow`
- spec dir: `<repo>/dsl/laya_mem`

## Gate metrics

| task | n | want_acc | oracle_agree | avg_ms | p50_ms | p95_ms |
|------|---|----------|--------------|--------|--------|--------|
| admission | 30 | 100.0% | 100.0% | 1.8 | 1.8 | 1.8 |
| memory_type | 30 | 100.0% | 100.0% | 1.7 | 1.7 | 1.7 |
| stopping | 29 | 89.7% | 79.3% | 1.3 | 1.3 | 1.3 |

⚠ stopping want_acc 89.7% < 90%

⚠ stopping oracle_agree 79.3% < 100% (tool diverges from spec rules)

**Overall: FAIL**