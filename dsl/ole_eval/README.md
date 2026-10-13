# dsl/ole_eval/ — ole-eval 场景的 DSL 移植

这 56 个 spec 把 **ole-eval**（`code/qwen_vm/app_scenarios/*`）里的策略式工作流
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
| `cache_policy_enforcer.json` | `cache_policy_enforcer/` | HTTP 缓存策略闸门单节点判定：模型只提 typed cache-policy change（request_id + routes[{path_pattern,cache_control,max_age}]），schema 校验（request_id 语法 / 非空 routes / 逐项字段类型 / 危险 token / max_age 非负整数）、routes 数上限、blocked path patterns、max_age 上限、require_approval 由本地有序 rules 裁决 → `reject_proposal` / `reject_too_many_routes` / `reject_blocked_pattern` / `reject_max_age` / `escalate_approval` / `approve`；本地 policy 表（max_routes/max_age_limit/blocked_patterns/require_approval）为固定 fixture |
| `log_redaction_guard.json` | `log_redaction_guard/` | 日志脱敏规则闸门单节点判定：模型只提 typed redaction change（request_id + rules[{field_pattern,action,replacement}]），schema 校验（action ∈ mask/hash/truncate/drop / 可编译 regex / 危险 token）、rules 数上限、blocked actions、policy-required patterns 缺失、require_approval 由本地有序 rules 裁决 → `reject_proposal` / `reject_too_many_rules` / `reject_blocked_action` / `reject_missing_required` / `escalate_approval` / `approve` |
| `dns_record_validator.json` | `dns_record_validator/` | DNS 记录变更闸门单节点判定：模型只提 typed DNS change（request_id + action add/modify/delete + records[{type,name,value,ttl?}]），schema 校验（type 枚举 / 非空 name·value / 非负 ttl / 危险 token）、records 数上限、blocked types、blocked domains、ttl < min_ttl、require_approval 由本地有序 rules 裁决 → `reject_proposal` / `reject_too_many_records` / `reject_blocked_type` / `reject_blocked_domain` / `reject_ttl_below_min` / `escalate_approval` / `approve` |
| `session_token_validator.json` | `session_token_validator/` | 会话令牌闸门单节点判定：模型只提 typed token change（request_id + action + token_type + scopes + expiry_seconds + metadata），**revoke 无条件放行**置于有序阶梯首位，其后 expiry_seconds 上限、scopes 数上限、blocked scopes、危险 metadata、require_approval 由本地有序 rules 裁决 → `reject_proposal` / `approve_revoke` / `reject_expiry` / `reject_scope_count` / `reject_blocked_scope` / `reject_metadata` / `escalate_approval` / `approve` |
| `webhook_signature_verifier.json` | `webhook_signature_verifier/` | webhook 签名校验配置闸门单节点判定：模型只提 typed webhook config（request_id/endpoint_url/hash_algorithm/encoding/header_name/secret_source/tolerance_seconds），schema 校验 + require_https + blocked algo + tolerance 上限 + approval 由本地有序 rules 裁决 → `reject_proposal`/`reject_insecure_url`/`reject_blocked_algo`/`reject_tolerance`/`escalate_approval`/`approve` |
| `endpoint_health_checker.json` | `endpoint_health_checker/` | 健康检查配置闸门单节点判定：模型只提 typed health-check config（request_id + endpoints[{url,method,timeout_seconds,expected_status,interval_seconds}]），schema 校验 + endpoints 数上限 + HTTPS + timeout 上限 + interval 下限 + blocked domains + approval 由本地有序 rules 裁决 → `reject_proposal`/`reject_too_many_endpoints`/`reject_insecure_url`/`reject_timeout`/`reject_interval_below_min`/`reject_blocked_domain`/`escalate_approval`/`approve` |
| `file_upload_scanner.json` | `file_upload_scanner/` | 文件上传闸门单节点判定：模型只提 typed upload（request_id + files[{filename,mime_type,size_bytes}] + destination + overwrite_policy），schema 校验 + files 数上限 + total·per-file size 上限 + blocked extensions + overwrite 升级 + approval 由本地有序 rules 裁决 → `reject_proposal`/`reject_too_many_files`/`reject_total_size`/`reject_file_size`/`reject_blocked_extension`/`escalate_overwrite`/`escalate_approval`/`approve` |
| `circuit_breaker_guard.json` | `circuit_breaker_guard/` | 熔断器状态迁移闸门单节点判定：模型只提 typed transition（request_id/service_id/current_state/proposed_state/error_rate/latency_p99_ms/open_duration_seconds），状态合法性 + 固定迁移表 + open 阈值 + half-open 时长 + close 错误率由本地有序 rules 裁决（Python 累积全部错误的语义以「首个失败检查」表达）→ `reject_proposal`/`reject_invalid_state`/`reject_illegal_transition`/`reject_open_threshold`/`reject_half_open_duration`/`reject_close_error_rate`/`approve` |
| `query_complexity_limiter.json` | `query_complexity_limiter/` | 查询复杂度闸门单节点判定：模型只提 typed query（request_id / query_type SELECT·INSERT·UPDATE·DELETE / tables / joins / subqueries / estimated_rows / query_text），schema 校验（request_id 语法 / 非负整数计数 / 危险 token）、blocked query_type、joins·subqueries·estimated_rows 上限、blocked tables、危险 query 文本、require_approval 由本地有序 rules 裁决 → `reject_proposal` / `reject_blocked_query_type` / `reject_too_many_joins` / `reject_too_many_subqueries` / `reject_row_estimate` / `reject_blocked_table` / `reject_dangerous_text` / `escalate_approval` / `approve` |
| `rate_limit_budget_guard.json` | `rate_limit_budget_guard/` | API 速率预算闸门单节点判定：模型只提 typed rate request（request_id / endpoint / requested_rate{rpm,tpm,burst}），schema 校验（request_id 语法 / 正值速率字段 / 危险 token）、endpoint 预算表准入、逐键 requested/limit 比值（>1 超限 REJECT / ≥0.8 近限 ESCALATE）由本地有序 rules 裁决 → `reject_proposal` / `reject_unknown_endpoint` / `reject_over_budget` / `escalate_near_budget` / `approve` |
| `rate_limiter_config_guard.json` | `rate_limiter_config_guard/` | 限流配置闸门单节点判定：模型只提 typed config（request_id / endpoint / algorithm / rate_per_second / burst_size / reason），算法枚举（token_bucket·sliding_window·fixed_window·leaky_bucket）、rate 上下界（常量 1..1e6 + policy band）、burst 上界（≥1 / ≤100×rate / ≤policy max_burst_size）、endpoint `..` 路径检查由本地有序 rules 裁决（Python 累积全部错误语义以「首个失败检查」表达）→ `reject_proposal` / `reject_algorithm` / `reject_rate` / `reject_burst` / `reject_endpoint` / `approve` |
| `network_bandwidth_allocation_guard.json` | `network_bandwidth_allocation_guard/` | 带宽分配闸门单节点判定：模型只提 typed allocations（request_id / allocations[{link_id,bandwidth_mbps,priority}]），schema 校验（非空列表 / 正值带宽 / priority 枚举 / 危险 token）、link catalog 准入、逐链路总量 ≤ capacity−reserved 由本地有序 rules 裁决 → `reject_proposal` / `reject_unknown_link` / `reject_over_capacity` / `approve` |
| `api_key_rotation_guard.json` | `api_key_rotation_guard/` | API key 生命周期闸门单节点判定：模型只提 typed action（request_id / key_id / action rotate·revoke·extend / age_days / new_key_length / extend_days），key_id 格式、age 硬限、新键长度≥32、提前轮换（age≥recommended 或 force）、扩展边界（extend_days ≤ max−age）由本地有序 rules 裁决（**revoke 无条件放行**；累积错误 → 首个失败检查）→ `reject_proposal` / `reject_key_id` / `reject_age` / `reject_new_key_length` / `reject_early_rotation` / `reject_extension` / `approve` |
| `credential_rotation_guard.json` | `credential_rotation_guard/` | 凭证轮换计划闸门单节点判定：模型只提 typed plan（request_id / credentials[{credential_id,service_name,days_since_last_rotation,max_rotation_days}] / rotation_plan{method,approval_required} / schedule_window），schema 校验（request_id 语法 / 非空凭据列表 / 非负整数计数 / 枚举 / 危险 token）、overdue（超过 min(凭据 max, policy max)）→ESCALATE **优先于** blocked service→REJECT、critical service 缺 approval→REJECT 由本地有序 rules 裁决 → `reject_proposal` / `escalate_overdue` / `reject_critical_approval` / `reject_blocked_service` / `approve` |
| `config_schema_migration_guard.json` | `config_schema_migration_guard/` | 配置 schema 迁移闸门单节点判定：模型只提 typed migration（request_id / schema_name / from_version / to_version / changes[{action,field_path,default_value,new_name}] / rollback_plan{steps}），schema 校验（action 枚举 / remove·rename 伴生字段 / 非空 rollback steps / 危险 token）、changes 上限、removals 上限、critical field 保护、removal default 非空、policy require_approval 由本地有序 rules 裁决 → `reject_proposal` / `reject_change_count` / `reject_removal_count` / `reject_critical_field` / `reject_removal_default` / `reject_dangerous` / `escalate_approval` / `approve` |
| `feature_flag_rollout_guard.json` | `feature_flag_rollout_guard/` | 特性开关灰度闸门单节点判定：模型只提 typed rollout（request_id / flag_name / rollout_plan{target_pct,strategy,target_env} / risk_level / staging_evidence），schema 校验（strategy·env·risk 枚举 / target_pct 0-100 / 危险 token）、env allowlist、prod 需 staging 证据、high-risk 需 canary、单步 pct 上限（high-risk 超限 REJECT、否则 ESCALATE）由本地有序 rules 裁决 → `reject_proposal` / `reject_env` / `reject_staging` / `reject_canary` / `reject_step_pct` / `escalate_step_pct` / `approve` |
| `cache_invalidation_guard.json` | `cache_invalidation_guard/` | 缓存失效闸门单节点判定：模型只提 typed invalidation（request_id / cache_keys[{key,namespace,current_ttl}] / invalidation_reason / ttl_override），schema 校验（非空 key·namespace / int ttl≥0 / 危险 token）、keys 数上限、protected namespace→ESCALATE、ttl 下限、require_reason 由本地有序 rules 裁决 → `reject_proposal` / `reject_key_count` / `escalate_namespace` / `reject_ttl` / `reject_reason` / `approve` |
| `model_drift_detector.json` | `model_drift_detector/` | 模型漂移检测闸门单节点判定：模型只提 typed assessment（request_id/model_id/metrics/baseline_metrics{accuracy,latency_p99_ms,throughput_rps}/drift_threshold），schema 校验（阈值 ∈(0,1] / 数值指标 / 危险 token）、accuracy 跌幅→REJECT、latency 增幅·throughput 跌幅→ESCALATE 由本地有序 rules 裁决 → `reject_proposal` / `reject_accuracy_drop` / `escalate_performance` / `approve` |
| `dead_letter_queue_guard.json` | `dead_letter_queue_guard/` | 死信队列闸门单节点判定：模型只提 typed action（request_id/messages[{message_id,queue_name,error_type,payload_size_bytes}]/action retry·discard·forward·archive/retry_count），schema 校验（action 枚举 / 非空 messages / 正值 payload / 危险 token）、retry 上限、payload 上限、blocked queue、forward 大包→ESCALATE 由本地有序 rules 裁决 → `reject_proposal` / `reject_retry` / `reject_payload` / `reject_queue` / `escalate_forward` / `approve` |
| `compliance_audit_evidence_guard.json` | `compliance_audit_evidence_guard/` | 合规审计证据闸门单节点判定：模型只提 typed checklist（request_id/audit_id/evidence_items[{evidence_id,description,status,category}]/compliance_framework），schema 校验（request_id·audit_id 语法 / 非空 items / status 枚举 / 危险 token）、缺项检查：missing+pending→ESCALATE（**优先于** REJECT）、必需类目覆盖、零采集、missing% 上限由本地有序 rules 裁决 → `reject_proposal` / `escalate_wip` / `reject_uncovered` / `reject_none_collected` / `reject_missing_pct` / `approve` |
| `workflow_dag_validator.json` | `workflow_dag_validator/` | 工作流 DAG 校验闸门单节点判定：模型只提 typed DAG（request_id/workflow_name/nodes[{id,type}]/edges[{from,to,condition}]），schema 校验（节点 id 语法·唯一 / type·condition 枚举 / 端点存在 / 危险 token）、节点·边上限、无环、单 start、有 end、blocked 节点类型、approval 由本地有序 rules 裁决 → `reject_proposal` / `reject_node_count` / `reject_edge_count` / `reject_cycle` / `reject_no_start` / `reject_multiple_starts` / `reject_no_end` / `reject_blocked_type` / `reject_dangerous` / `escalate_approval` / `approve` |

| `incident_response_guard.json` | `incident_response_guard/` | 事故响应动作闸门单节点判定：模型只提 typed action（incident_id / incident_semantic / action_intent / target_service / reason），incident id·语义匹配 / 幂等指纹重放 / runbook allowlist / 目标合法性与 affected-service 范围 / RESOLVED 终态 / 未知状态 → REJECT；未先 page oncall / 审批法定人数缺失 / rollback·failover·scale·close 前置条件 / mitigation SLA 已破 → REVISE，否则 APPROVE；**首个三值（APPROVE/REVISE/REJECT）端口** → `reject_proposal` / `reject_incident_mismatch` / `reject_semantic_conflict` / `reject_replay` / `reject_runbook` / `reject_target` / `reject_target_scope` / `reject_resolved` / `reject_state` / `revise_ack_then_page` / `revise_missing_roles` / `revise_precondition` / `revise_mitigation_sla` / `approve` |
| `shipping_route_guard.json` | `shipping_route_guard/` | 运输路线闸门单节点判定：模型只提 typed route preference（request_id / shipment_id / preference_intent / preferred_mode），物流图求解（容量 / 温度 / 危险品 / 单据 / 模式约束）折叠成 choice、成本 vs 上限，由本地有序 rules 裁决 → `reject_proposal` / `reject_no_route` / `reject_over_cost` / `approve` |
| `credential_issuance_guard.json` | `credential_issuance_guard/` | 凭据签发闸门单节点判定：模型只提 typed request（request_id / subject_id / credential_kind / operation / requested_audience / device_binding_requested），subject/policy 解析 + proofing·assurance·MFA·device·risk·revocation·rotation 检查，引擎三值 **ISSUE / MODIFY / REJECT** → `reject_*`（proposal / duplicate / unknown_subject / unknown_kind / subject_type / audience / ial / proofing / mfa / phishing / trusted_device / device_binding / risk / subject_inactive / credential_inactive / rotation_early）/ `modify_*`（rotation_required / device_binding_required / risk_lifetime / risk_scope / step_up）/ `issue` |
| `refactor_plan_guard.json` | `refactor_plan_guard/` | 重构计划闸门单节点判定：模型只提 typed plan（request_id / intent{operation,target_module,objective} / steps[{step_id,action,source_module,target_module,test_ids,requires}] / rollback / proposed_risk），operation scope + step DAG + per-action payload·scope + 原子性 checkpoint + rollback + 行为等价测试 + risk 对齐，由本地有序 rules 裁决 → `reject_*`（proposal / duplicate / unknown_operation / intent_target / intent_objective / duplicate_step_id / dependency_cycle / unknown_dependency / missing_action / checkpoint_payload / create_payload / module_scope / move_payload / move_scope / update_payload / dependency_boundary / update_source / update_target / test_payload / step_order / checkpoint_count / rollback_strategy / rollback_checkpoint / checkpoint_order / behavior_equivalence）/ `modify_*`（optional_tests / risk_upgraded / risk_downgraded）/ `approve` |
| `data_retention_expiry_guard.json` | `data_retention_expiry_guard/` | 单节点 `threshold` 图 + 有序判定阶梯 | 6/6 | 模型只提 typed plan（request_id/data_records[{record_id,category,created_at,retention_days}]/retention_policy{max_retention_days,requires_approval_categories}）；retention_days 超 max → REJECT、未过期且需审批类目 → ESCALATE（优先于 REJECT）、过期仅报告不阻断 → `reject_proposal`/`reject_over_retention`/`escalate_approval`/`approve` |
| `subscription_churn_retention_guard.json` | `subscription_churn_retention_guard/` | 单节点 `threshold` 图 + 有序判定阶梯 | 9/9 | 模型只提 typed offer（request_id/customer_id/churn_risk_score/retention_offer{discount_pct,duration_months,tier}）；低风险<0.3 且折扣>20% → ESCALATE **优先于** 违规 REJECT；折扣·时长超客户档位上限 / 档位越级 → REJECT → `reject_proposal`/`escalate_low_risk_high_discount`/`reject_discount`/`reject_duration`/`reject_tier_upgrade`/`approve` |
| `pricing_discount_guard.json` | `pricing_discount_guard/` | 单节点 `threshold` 图 + 有序判定阶梯（**三值 APPROVE/MODIFY/REJECT**）| 17/17 | 模型只提 typed intent（sku/segment/discount_intent）；catalog 查找 / segment 准入 / 促销窗口 / 量级 / 优惠券 / 叠加 / 毛利底线 / 审批阈值由本地引擎算精确价 → 重复·segment 不合格·审批封顶破毛利底线 = REJECT，8 类 adjustment = MODIFY（一个 `adjustment` 问题承载），否则 APPROVE |
| `inventory_replenishment_guard.json` | `inventory_replenishment_guard/` | 单节点 `threshold` 图 + 有序判定阶梯（**三值 APPROVE/MODIFY/REJECT**）| 19/19 | 模型只提 typed intent（sku/demand_scenario/service_level/intent）；需求预测 / 再订点 / pack·最小起订量 / 预算约束由本地引擎算精确补货量 → 未知 sku·重复·reorder 未达再订点·不可行·非正量 = REJECT，hold/review 已在点之上 = APPROVE（不下单），5 类 adjustment = MODIFY（动态 capped_by 折成单 label）|
| `privacy_retention_guard.json` | `privacy_retention_guard/` | 单节点 `threshold` 图 + 有序判定阶梯（**三值 APPROVE/MODIFY/REJECT**）| 32/32 | 模型只提 typed proposal（data_class/retention_intent/deletion_intent）；记录台账·记录类·jurisdiction·data-class/jurisdiction policy 表·retention 时钟·lawful basis·legal hold·pseudonymization 资格·删除/保全优先级由本地引擎裁决，精确算 scheduled action 与时间戳 → `reject_*`(5) / `modify_*`(7) / `approve_*`(5)，gate 后优先级链折成单个 `outcome` 问题承载 13 个分支 |
| `shift_assignment_guard.json` | `shift_assignment_guard/` | 单节点 `threshold` 图 + 有序判定阶梯（**三值 FEASIBLE/REVISE/INFEASIBLE**）| 23/23 | 模型只提 typed 信封（request_id/plan_id/preferences/adjustments）；worker 技能·可用性·工时·休息·连续班次·role 覆盖·fairness·有界搜索由本地求解器裁决 → INFEASIBLE 首因(4) / REVISE 首因(6) / feasible |
| `settlement_allocation_guard.json` | `settlement_allocation_guard/` | 单节点 `threshold` 图 + 有序判定阶梯（**三值 APPROVE/MODIFY/REJECT**）| 23/23 | 模型只提 typed intent（request_id/settlement_case/preferences/priority_rationale）；case 台账·canonical order·tax/credit/waterfall·rounding·ledger+cash-control·allocation caps 本地精确算 → reject 首因(8) / modify 首因(2) / approve |
| `subscription_entitlement_guard.json` | `subscription_entitlement_guard/` | 单节点 `threshold` 图 + 有序判定阶梯（**三值 APPROVE/MODIFY/REJECT**）| 27/27 | 模型只提 typed intent（request_id/subscription_id/requested_effective_date/target_plan/target_seats/add_on_intents）；subscription 台账·plan/add-on 目录·term/billing 窗口·依赖·seat policy·commitment·proration+credit preview·control-total 本地裁决 → reject 首因(10) / modify 首因(4) / approve |
| `contract_conflict_guard.json` | `contract_conflict_guard/` | 单节点 `threshold` 图 + 有序判定阶梯（**三值 CONFLICT/NO_CONFLICT/REVIEW**）| 16/16 | 模型只提 typed clause（clause_type/party/role/modality/start/end/priority/exception）；固定合同 fixture·本地 clause id·区间重叠·modal/deadline 冲突·优先级覆盖·exception 引用由本地校验器裁决 → review(2) / conflict(2) / no_conflict |
| `accessibility_content_guard.json` | `accessibility_content_guard/` | 单节点 `threshold` 图 + 有序判定阶梯（**三值 ACCEPT/REVISE/REJECT**）| 20/20 | 模型只提 typed remediation（content_id/issue_intent/target_node/remediation_intent/reason）；不可变内容快照上的确定性 scoped checklist：意图·目标匹配·remediation allow-list·未解决项 → reject 首因(5) / revise 首因(2) / approve |
| `experiment_design_gate.json` | `experiment_design_gate/` | 单节点 `threshold` 图 + 有序判定阶梯（**三值 APPROVE/MODIFY/REJECT**）| 9/9 | 模型只提 typed proposal（factors/levels/budget）；权威 factor 域·typed-factor 校验·全因子组合数·power→replicates·Decimal 预算·确定性两因子缩减由本地裁决 → reject(4) / modify / approve |
| `crawler_schema_guard.json` | `crawler_schema_guard/` | 单节点 `threshold` 图 + 有序判定阶梯（**三值 ACCEPT/REPAIR/REJECT**）| 17/17 | 模型只提 typed extraction proposal（request_id/schema_name/page_id/field_mappings）；固定 schema·page fixture·URL 政策·selector 归属·required/normalizer·value 校验·cardinality·PII transform·请求台账本地裁决 → reject(10) / repair(4) / accept |
| `rpa_action_guard.json` | `rpa_action_guard/` | 单节点 `threshold` 图 + 有序判定阶梯（**三值 ALLOW/REVIEW/DENY**）| 24/24 | 模型只提 typed action（request_id/intent/action/selector/value/value_ref/page_state/attempt）；固定 intent 政策表·page/DOM fixture·URL 政策·selector 归属·target 能力·sensitive 规则·value_ref 规则·typing 规则·retry 台账本地裁决 → deny(20) / review / allow |
| `knowledge_graph_guard.json` | `knowledge_graph_guard/` | 单节点 `threshold` 图 + 有序判定阶梯（**三值 ACCEPT/REVISE/REJECT**）| 24/24 | 模型只提 typed graph op（add_entity/add_relation/merge_entities/retire_entity）；固定图状态·id/unique key·referential integrity·cycle·version/transaction·merge/retire 本地修订·不变量回滚本地裁决 → reject(17) / revise(3) / accept |
| `data_contract_schema_guard.json` | `data_contract_schema_guard/` | 单节点 `threshold` 图 + 有序判定阶梯（**三值 ACCEPT/REVISE/REJECT**）| 20/20 | 模型只提 typed contract proposal（field/source_field/semantic/migration_intent）；权威 target schema·source mapping 授权·type/historic/key/PII 兼容表·backfill coverage·确定性 default backfill 本地裁决 → reject(15) / revise(2) / accept |
| `resource_scheduler_guard.json` | `resource_scheduler_guard/` | 单节点 `threshold` 图 + 有序判定阶梯（**三值 APPROVE/MODIFY/REJECT**）| 11/11 | 模型只提 typed preference set（job_preferences/soft_objective）；固定 authority（jobs/resources/eligibility/DAG/window/capacity/preemption）·有界穷举 solver·soft preference 覆盖本地裁决 → reject(7) / modify / approve |
| `policy_as_code.json` | `policy_as_code/` | 单节点 `threshold` 图 + 有序判定阶梯（**三值 ALLOW/DENY/REVIEW**）| 9/9 | 模型只提 typed policy document（version/fields/rules）；field-schema·危险文本扫描·condition/action 编译·版本化 registry（原子激活/显式回滚）·首优先级规则匹配（默认 DENY）本地裁决 → reject(3) / allow / deny / review / deny_no_matching_rule |
| `test_case_generator.json` | `test_case_generator/` | 单节点 `threshold` 图 + 有序判定阶梯（**三值 ACCEPTED/REPAIRED/REJECTED**）| 13/13 | 模型只提 typed test case（target/test_intent/inputs/oracle_kind）；input boundary·安全扫描·纯 fixture oracle·duplicate fingerprint 台账·确定性修复目录·coverage 目录本地裁决 → reject(5) / repair(5) / accept |
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

`laya-workflow dsl smoke` 的 `STATES` 已为 56 个 spec 注册样本状态，每个决策 label
至少一例（BLOCK / REVIEW / ALLOW、DENY / CHALLENGE、CLEAR、REJECT /
ESCALATE / APPROVE、block / challenge / monitor / allow）。这些样本走
`HeuristicBackend`（`src/backend.rs` 中每个 question id 有对应 handler），
在不启模型、不连服务的条件下验证「有序 rules → 最终 label」整条链路。

```bash
laya-workflow dsl smoke   # 56 个 ole_eval spec 全部样本输出预期 label
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
  单节点"判定 → 出结论"的策略（44 个）和多节点 DFA 状态机（`dialogue_policy` / `order_tracker` / `approval_workflow` / `loan_dialogue`）都已有落地案例；`dialogue_policy` 展示了 `edge.condition` 的 answer→next_node 路由如何映射 Python 的意图状态机；若未来要树状多级审核流，
  可把 `REVIEW` 语义拆成单独节点。
- DSL action kinds 仅 `none` / `copy_keys` / `merge_open_probs` / `threshold`
  / `gate`；确定性数值校验可加 `validate` / `math` cap，但当前 44 个单节点 spec
  走纯 LLM 判定（阈值与优先级仍由 `threshold` action 的有序 rules 编码，
  数值由模型给出）。

## 延伸

