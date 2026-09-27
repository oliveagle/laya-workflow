# dsl/ole_eval/ — ole-eval 场景的 DSL 移植

这 3 个 spec 把 **ole-eval**（`code/qwen_vm/app_scenarios/*`）里的策略式工作流
从 Python 移植成 laya-workflow DSL（声明式 JSON），无需改 Rust 即可增减规则。

> 原始 Python 实现保留在 ole-eval 仓库（`code/qwen_vm/app_scenarios/`），本目录
> 是语义等价的配置化移植，用于对照与后续迭代。

## 目录

| spec | ole-eval 原始实现 | 策略语义 |
|------|-------------------|----------|
| `content_safety_guard.json` | `content_safety_guard/` | 语义分类 + PII/prohibited 标志 + 严重度阈值，有序 **BLOCK → REVIEW → ALLOW** |
| `adaptive_risk_control.json` | `adaptive_risk_control/` | 意图/行为/风险分，有序 **DENY → REVIEW → CHALLENGE → ALLOW** |
| `deployment_canary_guard.json` | `deployment_canary_guard/` | 危险 token / 容量 / 形状校验，有序 **REJECT → ESCALATE → APPROVE** |

## 映射方式

原始 Python 里的**确定性表格查找**（taxonomy 基础严重度、audience 最小年龄、
上下文 allowlist、危险 token 表）在 DSL 里**折叠成模型直接判断的 choice question**
（如 `pii_public_high`、`age_blocking`、`canary_within_cap`）。每个问题只有
`A`（安全/不触发）/ `B`（触发）两个选项，把"查表 → 比较 → 归一"的步骤交给
LLM 一步判定。

最终放行与否由 `threshold` action 的**有序 rules** 决定（`rule_matches_multi`）：

- 命中 `answer_in` / `answer` 的问题（作用于该问题的判定结果）→ 高优先级 label
- 命中 `value_gte` / `value_lt` 的问题（作用于主问题的 severity/risk 分数）→ 兜底

```jsonc
"action": {
  "kind": "threshold",
  "question": "severity",
  "rules": [
    { "when": { "question": "is_prohibited", "answer": "B" }, "label": "BLOCK" },
    { "when": { "value_gte": 90.0 },                        "label": "BLOCK" },
    { "when": { "value_gte": 50.0 },                        "label": "REVIEW" },
    { "when": { "always": true },                           "label": "ALLOW" }
  ]
}
```

rules 按数组顺序匹配，第一条命中的 label 生效 —— 顺序即优先级。

## 运行

```bash
# 校验（应零 WARNING）
laya-workflow validate --spec dsl/ole_eval/content_safety_guard.json

# 列出所有 spec
laya-workflow list

# 离线/在线运行
laya-workflow --base-url http://127.0.0.1:8400 run \
    --spec dsl/ole_eval/content_safety_guard.json \
    --state '{"text": "..."}'
```

`state.keep` 保留请求上下文（如 `request_id` / `actor_id`），其余中间判定不
污染最终状态。

## 边界说明

- DSL `Edge` 目前只按 `answer → node` 路由（无分支式数值条件），所以这些
  单节点"判定 → 出结论"的策略最契合 DSL 现状；若未来要树状多级审核流，
  可把 `REVIEW` 语义拆成单独节点。
- DSL action kinds 仅 `none` / `copy_keys` / `merge_open_probs` / `threshold`
  / `gate`；确定性数值校验可加 `validate` / `math` cap，但当前 3 个 spec
  走纯 LLM 判定。

## 延伸

`reference/laya/` 是另一次迁移（`code/laya` 的 Python 参考实现），语义对照见
`reference/README.md`。未来可把更多 ole-eval 场景（use-cases 系列）按同样
方式配置化。
