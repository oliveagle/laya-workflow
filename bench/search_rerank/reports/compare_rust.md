# search rerank comparison (Rust harness, n=11)

| ranker | P@1 | P@3 | P@5 | MRR@5 | nDCG@5 | avg ms | p95 ms |
|---|---|---|---|---|---|---|---|
| bm25 | 0.273 | 0.364 | 0.455 | 0.341 | 0.369 | 0 | 0 |
| laya | 0.091 | 0.182 | 0.364 | 0.162 | 0.211 | 296 | 305 |
| qwen | 0.455 | 0.727 | 0.818 | 0.598 | 0.654 | 7511 | 15854 |

- bm25: term-overlap baseline (no rerank)
- laya: laya_ft_server `/v1/systemone` noul batch rerank (same prompt as laya-search/src/judge.js)
- qwen: Qwen3-Reranker-0.6B cross-encoder yes/no logit (real CPU fp32 run, loaded from committed per-row data)
