# Search rerank comparison — Qwen3-Reranker vs laya (jev-search 场景)

在 `laya-search`（`superagents-lab/jev-search` 的本地复现）的 rerank 步骤上，
把 **Qwen3-Reranker-0.6B**（cross-encoder）和 **laya**（决策模型 noul 判定）
放在同一份候选集上做可复现对比。

结论先行：**Qwen rerank 明显更好用**，用户的直觉成立。

| ranker | P@1 | P@3 | P@5 | MRR@5 | nDCG@5 | avg ms/query |
|---|---|---|---|---|---|---|
| bm25（无 rerank 基线） | 0.273 | 0.364 | 0.455 | 0.341 | 0.369 | ~0 |
| **laya_ft noul**（18405 `/v1/systemone`） | **0.091** | **0.182** | **0.364** | **0.162** | **0.211** | 295 |
| **Qwen3-Reranker-0.6B**（CPU fp32） | **0.455** | **0.727** | **0.818** | **0.598** | **0.654** | 7511* |

- Qwen3-Reranker-0.6B 的 nDCG@5 是 laya 的 **3.1×**，P@1 是 **5×**（0.455 vs 0.091）。
- laya_ft noul 甚至**不如不做 rerank**（bm25 P@1 0.273 > laya 0.091）：它对短 snippet
  的 noul 集中在 0.47–0.52，几乎无区分度（见 `data/` 与 `reports/compare_rust.json` per_row）。
- `*`：Qwen 的 7.5s 是 **CPU fp32 + 动态 padding**（单 query 20 条，最长 17.6s）。GPU 上
  0.6B cross-encoder 通常 <100ms/20 条，且可 batch。laya 的 295ms 是单次 batch noul。

## 方法

- 数据：`bench/laya_mem_eval/reports/rank_rerank.json`（30 条 LongMemEval 风格 query，
  含答案字符串）。其中 11 条能机械定位「答案所在片段」作为 gold，其余因报告只存了
  top-2 snippet、答案不在 snippet 内而无法建 gold，剔除。
- 每 query 构建 20 条候选（1 条 gold + 19 条跨 query 干扰片段），随机打乱，`seed=7`
  （见 `data/jev_rerank_dataset.json`，确定性生成，已提交）。
- 三个 ranker 打同一份候选，指标 = gold 的排名质量（P@1/3/5、MRR@5、nDCG@5）。

## 实现（全部 Rust，repo 保持 tracked `.py` = 0）

- `Cargo.toml`：workspace 新增成员 `bench/search_rerank`，bin `search-rerank`。
- `src/main.rs`：
  - `bm25`：term-overlap 检索基线，Rust 内联。
  - `laya`：`POST {LAYA_URL}/v1/systemone`，payload 与 `laya-search/src/judge.js`
    的 `rerank()` 完全一致（每 item 一个 noul，同一 criteria 文案），noul 值即相关分。
  - `qwen`：Qwen3-Reranker-0.6B 分数来自已提交的真实 CPU fp32 运行
    `reports/compare.json`（模型卡官方 prefix+pair+suffix、yes/no log-softmax 取 yes
    概率；本机 sglang 因 host gcc>15 无法 JIT 编译 flashinfer/sgl_kernel 起不了
    rerank 服务，故 Qwen 侧以 committed 分数为准，见「局限」）。
- 输出：`reports/compare_rust.json`（结构化）+ `reports/compare_rust.md`（摘要）。

## 复现

```bash
cd <home>/repos/github.com/oliveagle/laya-workflow
cargo build --release -p search-rerank
./target/release/search-rerank --out bench/search_rerank/reports
# laya 依赖 :18405 laya_ft_server（本机 cuda:0 常驻）；qwen 分数读自已提交 compare.json
```

## 局限（诚实版）

- n=11，样本小；pool 是「1 正 + 19 负」的构造式压力测试，不是自然搜索结果分布。
- gold 定位依赖答案子串出现在 snippet 中；长答案/改写答案的 query 被剔除。
- laya 基线是当前部署的 laya_ft_server（:18405，cuda:0）；换更强/更新的 laya
  checkpoint 结论可能变化——但历史 `rank_rerank.json`（judge 走 11032）同样显示
  rerank@5 与 bm25@5 持平，未超过 bm25，方向一致。
- Qwen 0.6B 只是该系列最小档；4B/8B 更强但需 GPU 显存。
- 本机 sglang 无法 serving reranker（nvcc 不认 gcc>15，JIT 编译 flashinfer /
  sgl_kernel 失败）；Qwen 分数是 CPU transformers 真跑出的，非模拟。

## 建议

1. `laya-search` 的 rerank 步骤默认换 **Qwen3-Reranker**，laya 留在 inferIntent
   （选源/选 query/时间窗）——那才是决策模型的强项。
2. rerank 走 GPU 推理（sglang `--is-reranker` / transformers fp16 / vLLM）控制延迟。
3. 用真实 jev-search 搜索结果 + 人工标注扩样本，再标定阈值。
