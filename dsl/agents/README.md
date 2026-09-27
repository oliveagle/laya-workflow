# dsl/agents/ — agent-toolbox workflow 的 DSL 移植

把 **agent 工具箱仓库**（shell + Python 组成的 ops 脚本集）里的决策型 workflow
移植成 laya-workflow DSL（声明式 JSON）。未来项目**只改 JSON，不改 Rust**。

## 目录

| spec | 原始实现（agent 工具箱） | 决策语义 |
|------|--------------------------|----------|
| `quality_gate.json` | `.githooks/quality-gate.sh` (141 行) | 5 项检查 → **FAIL > WARN > NOTE > PASS** 有序规则 |
| `security_scan.json` | `code/scripts/security-scan.sh` (234 行) | 9 个恶意 pattern + whitelist + binary 跳过 → **QUARANTINE_CRITICAL > QUARANTINE_HIGH > CLEAN / SKIP** |
| `commit_msg_gate.json` | `.githooks/commit-msg` (40 行) | 3 条规则 → **FAIL_EMPTY > FAIL_TOO_SHORT > FAIL_PLACEHOLDER > PASS**（`heuristic.field: message` 检查 message 值本身） |
| `version_gate.json` | `skills/ole-release/scripts/check_version.sh` (165 行) | 5 项检查 → **FAIL_INVALID_VERSION > FAIL_TAG_UNREACHABLE > FAIL_ORIGIN_DIVERGED > FAIL_DIRTY_TREE > PASS_RC / PASS_STABLE** |
| `skill_publish_gate.json` | `skills/devine-gen-mcp/scripts/quality-gate.sh` (207 行) | 4 项检查（结构 / 行数 / placeholder / 链接）→ **FAIL_STRUCTURE > FAIL_LINE_COUNT > FAIL_PLACEHOLDER > FAIL_LINKS > PASS** |
| `workflow_guardian.json` | `code/agents/workflow-guardian/scripts/scan-all.sh` (216 行) | 3 层（任务 / 知识 / 协作）→ **FAIL_TASK_LAYER > FAIL_COLLABORATION_LAYER > WARN_KNOWLEDGE_LAYER > OK** |
| `ssl_certificate_expiry.json` | `code/scripts/ssl-expiry-check.sh` + `ssl-renew-olehome.sh` | 剩余天数阶梯 → **EXPIRED > WARN_RENEW > RENEW_FORCED > OK / FAIL_UNREADABLE**（`extract_numeric` 读真实天数） |

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

**`heuristic.field`**：把匹配范围从「整个 state JSON 信封」缩到**一个字段值**
（如 `{"field":"message"}` 时正则直接作用于 `state.message` 的原始值，而不是
信封字符串）。字段不存在按无命中处理，不报错。这对 commit-msg 长度 / 单 token
判断、VERSION 格式检查这类「针对字段值而非整包」的场景是必要的。

**`heuristic.extract_numeric`**：score 类型问题可从字段里取**真实数值**做阈值
比较（如 `{"field":"days_left","default":0}` → 取 `state.days_left` 的数值）。
没有它时 score 的离线默认值是常量 0.5，所有阈值都会落到同一分支；有了它，
`95 → HIGH / 50 → MEDIUM / 10 → LOW`（含 `80`、`29.9` 等边界）才能真正区分。
字段缺失或非数值时取 `default`。

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

`bench/dsl_smoke.py` 的 `STATES` 注册了 42 个样本（7 个 spec），覆盖每个 verdict label：

- quality_gate: FAIL / WARN / NOTE / PASS
- security_scan: QUARANTINE_CRITICAL / QUARANTINE_HIGH / CLEAN / SKIP
- commit_msg_gate: FAIL_EMPTY / FAIL_TOO_SHORT / FAIL_PLACEHOLDER / PASS
- version_gate: PASS_STABLE / PASS_RC / FAIL_INVALID_VERSION / FAIL_TAG_UNREACHABLE / FAIL_ORIGIN_DIVERGED / FAIL_DIRTY_TREE
- skill_publish_gate: PASS / FAIL_STRUCTURE / FAIL_LINE_COUNT / FAIL_PLACEHOLDER / FAIL_LINKS
- workflow_guardian: OK / FAIL_TASK_LAYER / WARN_KNOWLEDGE_LAYER / FAIL_COLLABORATION_LAYER
- ssl_certificate_expiry: OK / WARN_RENEW / EXPIRED / RENEW_FORCED / FAIL_UNREADABLE（含 0 / 30 天边界）

```bash
python3 bench/dsl_smoke.py     # 42 agents 样本 + 全量回归 = 全绿
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
