# dsl/agents/ — agent-toolbox workflow 的 DSL 移植

把 **agent 工具箱仓库**（shell + Python 组成的 ops 脚本集）里的决策型 workflow
移植成 laya-workflow DSL（声明式 JSON）。未来项目**只改 JSON，不改 Rust**。

## 目录

| spec | 原始实现（agent 工具箱） | 决策语义 |
|------|--------------------------|----------|
| `quality_gate.json` | `.githooks/quality-gate.sh` (141 行) | 5 项检查 → **FAIL > WARN > NOTE > PASS** 有序规则 |
| `security_scan.json` | `code/scripts/security-scan.sh` (234 行) | 9 个恶意 pattern + whitelist + binary 跳过 → **QUARANTINE_CRITICAL > QUARANTINE_HIGH > CLEAN / SKIP** |

## 通用化：spec-declared heuristic（零 Rust）

之前新增 ole_eval 场景要给每个 question 加 Rust handler，违反了「新项目只写
JSON」的目标。现在 HeuristicBackend 支持**在 question 里声明启发式**：

```jsonc
"staged_placeholder": {
  "type": "choice",
  "instructions": "…",
  "criteria": { "A": "clean", "B": "violation" },
  "heuristic": {
    "match_any": ["staged_placeholder", "placeholder_in_staged"],
    "p_hit": 0.95, "p_miss": 0.05
  }
}
```

两个字段：

- `match_any` — 字面 substring needles（word-boundary aware，同原有 handler）
- `match_regex` — 正则数组（任一命中即 hit），可直接搬运 shell `grep -E` 规则

二者可同时声明（先 literal，再 regex），没有声明则走原有路径（内建 handler
或 `default_choice`），不破坏任何已有 spec。choice 走 `p_hit/p_miss`，
score 走 `score_hit/score_miss`。

## 映射方式

与 `dsl/ole_eval/` 相同：原始 shell 的**规则表 / 阈值阶梯**压成
choice question（A=干净 / B=命中），最终 verdict 由 `threshold` action 的
**有序 rules** 决定（`rule_matches_multi`）。规则顺序即优先级。

关键差异：shell 脚本里的**副作用**（`cp` 到隔离目录、`mv` 重命名、`sed` 注入
警告头部、`git mv`、`find` 遍历）不在 DSL 里做 —— DSL 只负责**决策**，副作用
由调用方的 probe 层执行（拿 `label` 后分支）。这个「决策和副作用分离」与
`dsl/ole_eval/` 的 decision-shape 相同，只是输入从「模型判断」变成「声明式
regex/needle 命中」。

## 离线覆盖

`bench/dsl_smoke.py` 的 `STATES` 注册了 10 个样本，覆盖每个 verdict label：

- quality_gate: FAIL / WARN / NOTE / PASS
- security_scan: QUARANTINE_CRITICAL / QUARANTINE_HIGH / CLEAN / SKIP

```bash
python3 bench/dsl_smoke.py     # 10 agents 样本 + 426 回归 = 全绿
```

## 运行

```bash
laya-workflow validate --spec dsl/agents/quality_gate.json
laya-workflow validate --spec dsl/agents/security_scan.json

# 离线（HeuristicBackend）
laya-workflow run --spec dsl/agents/quality_gate.json \
    --state '{"repo":"agents","branch":"main","text":"staged_placeholder"}'

# 连真模型
laya-workflow --base-url http://127.0.0.1:8400 run --spec dsl/agents/… \
    --state '…'
```

`state.keep` 保留调用方需要的上下文（`repo/branch` / `file/scan_dir` / `text`），
其余中间判定不污染最终状态。

## 边界说明

- **literal + regex**：只支持 substring / 正则，不支持全文 semantic 理解 —— 那是
  真 LLM backend 的活。DSL 的启发式适合「确定的规则表」类决策（quality gate、
  安全扫描、ACL、灰度阈值），不适合「需要语义」的场景。
- **非流式**：`bin/monitor-port-live.sh` 这类 tcpdump 实时监控不在本目录；DSL
  不是为 live stream 设计的。
- **过程型 pipeline**：`ole-commit-push` 多阶段（split → secret → commit →
  leftover → rebase/push）要走 `dsl/pipelines/` 或 `dsl/capabilities/` 组合，
  不适合单 spec 表达。
