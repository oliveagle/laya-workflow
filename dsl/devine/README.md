# dsl/devine/ — devine_utils 的 release / test / integration 流移植

这 8 个 spec 把 **devine_utils** 里 `devine-utils-ci workflow` 的
二进制发布 / 测试 / 集成流程（Go 实现）移植成 laya-workflow DSL（声明式 JSON），
仿照 `dsl/ole_eval/` 的做法：**不改 Rust 即可增减规则**。

> 原始 Go 实现保留在 devine_utils 仓库
> （`cmd/devine-utils-ci/internal/workflow*.go`，文档 `docs/workflow.md`），
> 本目录是语义等价的**声明式镜像**，用于对照与后续迭代。
> 冻结在二进制里的 `devine-utils-ci workflow` 仍是**可执行的权威实现**。

## 目录

| spec | Go 原始实现 | 语义 |
|------|-------------|------|
| `release_gate.json` | `workflow_gate.go: gateCheckIntegration01BeforePromote` | 发布门禁（规则 1）：integration-01 `/version` 必须 HTTP 200 且 `app_version == rc`，否则 **BLOCK** |
| `post_promote_check.json` | `workflow_gate.go: runIntegration02AfterPromote` | 规则 2：stable 上线后必须**立刻**用 integration-02 测试并确认 `/version == stable`，否则 **FAIL** |
| `deploy_prod.json` | `workflow_deploy_prod.go` | FAT 最新 SUCCESS 镜像 → PROD group `auto_deploy` → `rollout` → 要求 **SUCCESS** + PROD `/spec` 工具存在（spec-drift 防护） |
| `release_rc.json` | `workflow_release.go: WorkflowReleaseRC` + `workflow_util.go: pollArtifactory` | bump rc → push main + tag → 校验 UAT / PROD artifactory 出现该版本（UAT → PROD） |
| `status_snapshot.json` | `workflow_status.go: WorkflowStatus` | 只读快照：binary repo 的 VERSION/tags + 四个 app 的 FAT `/version` |
| `test_rc.json` | `workflow_apps.go: WorkflowTestRC` + `workflow_gate.go: triggerAndPollApp` | 触发 dev-test app（devine-test-004-function）的 main pipeline（`--wait`）→ 读 `/version`，要求 `app_version == rc` |
| `integration_rc.json` | `workflow_apps.go: WorkflowIntegrationRC` + `triggerAndPollApp` | 参数化：触发任意 app（01/02/任一 dev-test）的 main pipeline → 读其 `/version`，要求 `app_version == rc` |
| `promote.json` | `workflow_release.go: WorkflowPromote` | 复合判定：规则 1 门禁（integration-01）→ 委托 CLI `workflow promote`（bump/push/tag + PROD artifactory）→ 规则 2 post-check（integration-02） |

## 映射方式

原 Go 实现里**确定性的"取证据 → 比大小 / 比字符串 → 出结论"**，在 DSL 里拆成两步：

1. **取证节点**：`action.kind = "call"`，用 `http` / `exec` capability 拿到事实，
   通过 `project` 把关键字段（如 `app_version`、`http_status`、release `status`、`exit_code`）
   投影进 workflow state（`merge_payload` 会把 action payload 合并进 state）。
2. **判定节点**：`action.kind = "threshold"`，用一个 `choice` question 让模型结合
   state 里的证据做结论，再由**有序 rules**决定最终 label —— **顺序即优先级**。

```jsonc
"action": {
  "kind": "threshold",
  "question": "gate_verdict",
  "rules": [
    { "when": { "answer": "block" }, "label": "BLOCK" },
    { "when": { "always": true },    "label": "ALLOW" }
  ]
}
```

命令式的**变更动作**（bump / push / checkout main）委托给冻结的 CLI，
用 `exec` capability 调用 `devine-utils-ci workflow ...`，
使"变更"仍走已测试的 Go 实现，而"判定 / 策略"搬到 DSL 里可见可改。

## 内置目录（与 `DefaultWorkflowConfig()` 对齐）

binary repo：`devine/devine-golang-mcp-app`，`VERSION`，remote `origin`。
artifactory base：`http://artifactory.release.ctripcorp.com/artifactory`
（UAT/PROD path 见 `release_rc.json`）。

| app | role | debug | appId | FAT group | PROD groups | spec_tool |
|-----|------|-------|-------|-----------|-------------|-----------|
| devine-test-004-function | dev-test | on | 310011825 | 1050478 | 1050772 SHAXY | http_self_ping |
| devine-test-001-function | dev-test | on | 310013281 | 1053904 | 1053920 SHAXY | http_self_ping |
| devine-app-integration-01-function | integration | on | 310012871 | 1042011 | 1053332 SHAXY, 1042473 SHA-ALI, 1053330 SGP(禁用) | http_self_ping |
| devine-app-integration-02-function | integration | off | 310013220 | 1042845 | 1053776 SHAXY, 1054162 SHA-ALI(禁用), 1054164 SGP | hello_tengo |

> 禁用的 PROD group（如 integration-01 的 SGP）在 CLI 里"跳过并报告"。
> `deploy_prod.json` 每次只处理**一个** group（由 `state.prod_group_id` 指定），
> 多 group 请按 enabled 列表分别运行。

## Secrets

绝不内联。`deploy_prod.json` 需要 Captain UI 会话 token：

```bash
export CAPTAIN_USER_TOKEN=...      # 或写进 .env / secrets json
```

`release_rc.json` / `post_promote_check.json` 通过 `exec` 调用
`devine-utils-ci`，GitLab token 由该 CLI 自己解析（dotvault），spec 不需要再声明。

## 运行

```bash
# 校验（应零 WARNING）
laya-workflow validate --spec dsl/devine/release_gate.json

# 列出所有 spec（递归遍历 dsl/ 树）
laya-workflow list

# 规则 1：release gate（rc 由 state 注入）
laya-workflow --base-url http://127.0.0.1:8400 run \
    --spec dsl/devine/release_gate.json \
    --state '{"rc": "1.2.0-rc.1"}'

# 规则 2：post-promote check
laya-workflow --base-url http://127.0.0.1:8400 run \
    --spec dsl/devine/post_promote_check.json \
    --state '{"stable": "1.2.0"}'

# deploy-prod：参数化，一次一个 app + 一个 PROD group
laya-workflow --base-url http://127.0.0.1:8400 run \
    --spec dsl/devine/deploy_prod.json \
    --state '{"app_name":"devine-test-004-function","fat_group_id":1050478,"prod_group_id":1050772,"prod_url":"http://devine-test-004-function.faas.ctripcorp.com","spec_tool":"http_self_ping"}'

# test-rc：触发 dev-test app（004）并校验 /version == rc
laya-workflow --base-url http://127.0.0.1:8400 run \
    --spec dsl/devine/test_rc.json \
    --state '{"rc": "1.2.0-rc.1"}'

# integration-rc：参数化指定 app（01/02/任一 dev-test）
laya-workflow --base-url http://127.0.0.1:8400 run \
    --spec dsl/devine/integration_rc.json \
    --state '{"app_name":"devine-app-integration-01-function","project":"faas/devine-app-integration-01-function","version_url":"http://devine-app-integration-01-function.fws.faas.qa.nt.ctripcorp.com/version","rc":"1.2.0-rc.1"}'

# promote：规则 1 门禁 + 委托 CLI + 规则 2 post-check（rc / stable 由 state 注入）
laya-workflow --base-url http://127.0.0.1:8400 run \
    --spec dsl/devine/promote.json \
    --state '{"rc":"1.2.0-rc.1","stable":"1.2.0"}'
```

## 边界说明

- **轮询**：`pollArtifactory` / `pollAppVersion` / `captainWaitRelease` 都是"每 N 秒重试到超时"的循环。
  DSL 的 `Edge` 目前只按 `answer → node` 路由，无循环计数，所以这些 spec 做的是**单次快照判定**；
  真正的轮询仍由 `devine-utils-ci workflow ...` 负责（也是 `release_rc.json` 委托它的原因）。
- **allow_hosts 精确匹配**：`check_host` 是比较完整 host 的字符串相等（无通配符）。
  固定主机的 spec 已显式列出；`deploy_prod.json` 因为 `prod_url` 由 `state` 注入，
  已把四个已知 PROD host 全部列出。
- **`deploy_prod.json` 的 `chain`**：`auto_deploy` 先创建 release（PENDING），
  紧接着 `rollout` 用 `${with.deploy.body.id}` 取到 release id 再推进；
  与 Go 实现"PENDING 时自动点 rollout"语义一致（Go 是轮询中触发，这里是一次）。
- **只读 vs 写**：`status_snapshot.json` / `release_gate.json` 只读（`allow_exec=false`）；
  `release_rc.json` / `post_promote_check.json` 含写操作，故开 `allow_exec` 并限定 `allow_paths`。

- **600s 引擎硬上限**：`policy.max_timeout_ms` 在引擎里被硬夹到 `600000`（10 分钟），
  spec 里写更大也会被截断。因此委托 CLI 的 spec 显式把 CLI 自身的轮询窗口收进这个上限：
  `release_rc.json` 传 `--timeout 270`（UAT + PROD 各一次），`promote.json` 传
  `--timeout 360 --poll-timeout 180`。CLI 默认的 1800s artifactory 轮询在 DSL 内**无法**完整表达；
  需要完整窗口时直接跑 `devine-utils-ci workflow ...`（权威实现）。
- **`apps` / `list` 不建 spec**：`workflow apps`（打印内置目录）与 `workflow list`（枚举命名 workflow）
  是注册表 / 自省命令，不含任何"取证 → 判定"的流——前者是静态数据（见本页"内置目录"表，并已固化进各 spec 的
  `allow_hosts` / capability），后者的 DSL 对应物就是 `laya-workflow list`。故不为它们建退化 spec。

## 延伸

- 权威流程文档：devine_utils `docs/workflow.md`。
- 同类迁移先例：`dsl/ole_eval/README.md`（ole-eval 的 Python 策略 → DSL）。
