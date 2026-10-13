# dsl/ole_eval/ — ole-eval 场景的 DSL 移植

这 13 个 spec 把 **ole-eval**（`code/qwen_vm/app_scenarios/*`）里的策略式工作流
从 Python 移植成 laya-workflow DSL（声明式 JSON），无需改 Rust 即可增减规则。

> 原始 Python 实现保留在 ole-eval 仓库（`code/qwen_vm/app_scenarios/`），本目录
> 是语义等价的配置化移植，用于对照与后续迭代。

## 目录

| spec | ole-eval 原始实现 | 策略语义 |
|------|-------------------|----------|
| `content_safety_guard.json` | `content_safety_guard/` | 语义分类 + PII/prohibited 标志 + 严重度阈值，有序 **BLOCK → REVIEW → ALLOW** |
| `adaptive_risk_control.json` | `adaptive_risk_control/` | 意图/行为/风险分，有序 **DENY → REVIEW → CHALLENGE → ALLOW** |
| `deployment_canary_guard.json` | `deployment_canary_guard/` | 危险 token / 容量 / 形状校验，有序 **REJECT → ESCALATE → APPROVE** |
| `aml_screener.json` | `aml_screener/` | 制裁国 / 风险分，有序 **BLOCK → REVIEW → CLEAR**（R001 制裁国强制 BLOCK） |
| `intrusion_signal_guard.json` | `intrusion_signal_guard/` | 签名 / 速率 / 端口 / 地理时间信号 → 4 级裁决 **block → challenge → monitor → allow** |
| `dialogue_policy.json` | `dialogue_policy/` | Intent→State→Action DFA（8 节点）：edge routing 驱动 **greeting → identifying → authenticated → collecting → confirm → done / escalated / aborted** 状态迁移 |
| `order_tracker.json` | `order_tracker/` | 订单生命周期状态机（9 节点）：**draft → created → paid → fulfilling → shipped → delivered**、`created/paid/fulfilling → cancelled_unpaid/cancelled_paid → refunded`（refund 仅 paid 取消或已发货可退）；模型只提 typed event，合法跳转/非法拒绝由 edge routing + `label accept/reject` 本地裁决 |
| `approval_workflow.json` | `approval_workflow/` | 多角色法定人数审批状态机（15 节点）：**submitted →（按金额分档）reviewing → approved / rejected / cancelled**；金额档 small（经理）/ medium（经理+财务）/ large（经理+财务+安全），角色集合以乘积状态节点保真（medium 3、large 7）；模型只提 typed proposal，法定人数/单角色单票/自批/重复票/越权由 edge routing + `label`（`start_review`/`record_approval`/`approve_request`/`reject_request`/`cancel_request`/`duplicate_role_vote`/`requester_cannot_vote`/`self_approval_forbidden`/`role_not_required`/`reject_proposal`）本地裁决 |
| `alert_noise_reduction.json` | `alert_noise_reduction/` | 告警降噪单节点判定：模型只提 typed context（`cause_group` / `release_correlation` / `business_period`），阈值（每指标可配）/ 趋势 / 去重窗口（300s）/ 冷却窗口（900s）/ 严重度合成全部由本地规则引擎裁决 → action `page` / `ticket`（升级）或 `clear` / `duplicate` / `cooldown` / `low_severity` / `reject_context`（抑制）|
| `self_healing_gate.json` | `self_healing_gate/` | 自愈决策闸门单节点判定：模型只提 typed diagnosis（`fault_class` / `evidence` / `candidate_remediation`），确定性分类链 / 证据核验 / action 权限 / cooldown / rate limit / blast radius / rollback / 终态全部由本地规则引擎裁决 → `reject_diagnosis` / `deny_terminal` / `escalate_human` / `defer_cooldown` / `defer_rate_limit` / `execute_stabilizing` / `execute_recovered` / `execute_rolled_back` / `execute_terminal` |
| `loan_dialogue.json` | `loan_dialogue/` | 贷款申请 slot-filling 对话 DFA（5 节点）+ 承保：**awaiting_income → awaiting_debt → awaiting_amount → awaiting_term → complete**；模型只提 typed facts（annual_income/monthly_debt/loan_amount/term_months），每轮须恰好填期望槽位（skip-step/多字段/越界/非法值/完成后重复均本地拒绝）；Decimal 摊销与 DTI>43.00 / payment-to-income>28.00 阈值折算进终态 `outcome` → `APPROVE`/`REJECT`（+ `reject_duplicate`/`reject_value`/`reject_skip`）|
| `sla_checker.json` | `sla_checker/` | 确定性 UTC SLA 评估单节点判定：模型只提 typed facts（`proposal` 解析态 / `rule` schema 态 / `event` schema 态 / `sla` 总体态 / `severity` 超限比档），schema 校验（rule_id / `P1..P99` / `response<=resolution` / 窗口不重叠 / 六键事件 / 仅 UTC `Z` 时间戳 / 时间戳先后）、冲突、重复（rule registry / event ledger）、SLO 状态与严重度全部由本地有序 rules 裁决 → `reject_proposal` / `reject_rule` / `reject_rule_conflict` / `reject_duplicate_rule` / `reject_event` / `reject_duplicate_event` / `breach_critical` / `breach_high` / `breach_medium` / `breach_low` / `met` / `pending`；UTC 业务窗口分钟累计由本地算出（仅以 `sla`/`severity` 呈现），JSON 解析细节折进 `proposal` choice |
| `api_quota_guard.json` | `api_quota_guard/` | API 配额闸门单节点判定：模型只提 typed request intent（`principal` / `endpoint` / `requested_units` / `context_intent`），schema 校验（枚举字段 / units∈[1,1000] / 无重复 JSON key / 禁 authority·side-effect 字段）、request-id 重放台账、时钟单调性、scope 准入、per-endpoint 单请求上限、并发上限、令牌桶、日/月单位配额与花费预算全部由本地有序 rules 裁决 → `reject_proposal` / `reject_replay` / `reject_clock_regression` / `reject_scope` / `reject_units_above_max` / `throttle_concurrency` / `throttle_bucket` / `throttle_daily` / `reject_daily_no_retry` / `reject_monthly` / `reject_spend` / `allow`；令牌桶 refill 算术与 `retry_after_ms` 由本地算出（仅以 `bucket` = ok/insufficient 呈现），JSON 解析细节折进 `proposal` choice |

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

## 离线覆盖

`laya-workflow dsl smoke` 的 `STATES` 已为 13 个 spec 注册样本状态，每个决策 label
至少一例（BLOCK / REVIEW / ALLOW、DENY / CHALLENGE、CLEAR、REJECT /
ESCALATE / APPROVE、block / challenge / monitor / allow）。这些样本走
`HeuristicBackend`（`src/backend.rs` 中每个 question id 有对应 handler），
在不启模型、不连服务的条件下验证「有序 rules → 最终 label」整条链路。

```bash
laya-workflow dsl smoke   # 13 个 ole_eval spec 全部样本输出预期 label
```

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
  单节点"判定 → 出结论"的策略（9 个）和多节点 DFA 状态机（`dialogue_policy` / `order_tracker` / `approval_workflow` / `loan_dialogue`）都已有落地案例；`dialogue_policy` 展示了 `edge.condition` 的 answer→next_node 路由如何映射 Python 的意图状态机；若未来要树状多级审核流，
  可把 `REVIEW` 语义拆成单独节点。
- DSL action kinds 仅 `none` / `copy_keys` / `merge_open_probs` / `threshold`
  / `gate`；确定性数值校验可加 `validate` / `math` cap，但当前 9 个单节点 spec
  走纯 LLM 判定（阈值与优先级仍由 `threshold` action 的有序 rules 编码，
  数值由模型给出）。

## 延伸

