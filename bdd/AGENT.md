# BDD 维护指南（agent 版）

你是这个仓库的 **BDD 维护者**。`.feature` 是唯一事实来源（single source of
truth）——workflow、测试、生产集成测试全部由它生成。**准确率是最高标准**：宁可
编译失败，也不让一个意思不明确的步骤通过。

## 一、这个方案是什么

```
                         ┌─ workflow  spec/          （laya-workflow JSON，跑真实 Chrome）
bdd/*.feature  ── build ─┼─ 测试      manifest.json    （本地 hermetic，CI 用）
（Gherkin 源）   ──>     └─ 生产集成   it.manifest.json  （@production，外部 base_url）
                          + coverage.json（准确率账本：每步覆盖率 + 未匹配清单）
```

一条命令（accuracy gate 内置）：
```bash
laya-workflow bdd build                     # 本地：编译 + validate + 100% 覆盖
laya-workflow bdd build --assist            # + 词汇表外步骤的 needle 建议
laya-workflow bdd build --profile production \
  --base-url https://app.example.com             # 生产集成测试计划（@production 场景）
laya-workflow bdd run --profile production \
  --base-url https://app.example.com             # 真正执行生产 IT（需要 Chrome/CDP）
laya-workflow bdd compare                        # 离线：BDD vs 手写 JSON 的体积 + 造册门禁
laya-workflow bdd live                           # 实战：29 个真实网站（需要 Chrome）
```

**门禁是硬性的**：词汇表外步骤 = 编译错误（exit 1）；生成的 spec 必须过
`laya-workflow validate`；每 feature 覆盖率必须 100%；选了 0 个场景 = 错误。

## 二、写 / 改 feature 的规则

1. **只用标准词汇表**（`laya-workflow bdd vocabulary-check` 里的规则）。当前支持：
   - 导航/动作：`the browser is ready`、`I am on "<url>"`、`I open/navigate to "<url>"`、
     `I wait for the element "<sel>"`、`I click the element "<sel>"`、
     `I click the element "<sel>" if it is present`、`I type "<text>" into the element "<sel>"`、
     `I select "<v>" in the element "<sel>"`、`I press the key "<key>"`、
     `I run javascript "<expr>"`、`I release the page`。
   - 工作流取值（给 agent 当工具用、产出 collected state，而非 pass/fail）：
     `I extract the text of the element "<sel>" into <key>`、
     `I extract the attribute "<attr>" of the element "<sel>" into <key>`、
     `I extract the page url into <key>`、`I extract the page title into <key>`、
     `I wait until the element "<sel>" becomes visible|invisible|absent|enabled|disabled`。
   - 断言 11 种：`the page title contains` / `the page url contains` /
     `the element ... is visible|absent` / `javascript ... is true|false|equals|contains|equals text` /
     `the saved value "<key>" equals text "<text>"` / `the saved value "<key>" contains text "<text>"`。
   > 做"工作流"（不是测试）时优先用 extract/key/wait_until/click-if-present + state 断言：
   > 它是把没有接口的 UI 变成 agent 可调用工具的主路径。
2. **一个 tag 一行**（Gherkin 解析器一行只认一个 `@tag`）：
   ```
   @production
   @smoke
   Scenario: ...
   ```
3. **外部地址用 `<base_url>`**：`Given I am on "<base_url>/"`——本地 profile 注入
   fixture 服务器地址，生产 profile 注入 `--base-url`。绝不硬编码生产域名。
4. **加新 step 必须先扩词汇表**，再在 feature 里用它：
   `laya-workflow bdd vocabulary-check` 的 `_GIVEN/_WHEN/_THEN` 表加正则 + `_args()` 加参数映射，
   然后 `laya-workflow bdd vocabulary-check` 加一行断言它映射到正确 op。否则
   `laya-workflow bdd build` 会以"unknown step"拒绝——这不是 bug，是门禁。
5. **生产集成测试场景打 `@production`**，只跑真实目标，绝不引用本地 fixture
   （`htmx.html` 这类）。
6. **工作流场景用 `@outputs(<key>, ...)` 声明产物**：它把"这个工作流产出哪些 state key"
   变成编译期合同——`laya-workflow bdd build` 会拒绝"声明了但没有任何 step 产出"的输出。
   每个 `@outputs` 里的 key 必须由某个 `I extract ... into <key>` 产出（或 seed 进
   initial_state）。参考 `bdd/features/workflow_capture.feature` + `bdd/fixtures/app.html`。

## 三、维护循环（每次改动都走）

```bash
# 1. 改 .feature / plugins/bdd/main.rhai
# 2. 本地门禁（不需要 Chrome）
laya-workflow bdd build --assist
# 3. 有词汇表外步骤？先看 --assist 的建议（带置信度），
#    把可接受的规则写进 plugins/bdd/main.rhai，再重跑。建议永远不直接编译。
# 4. 需要真跑浏览器（有 Chrome 的机器 / CI）
laya-workflow bdd run
scripts/bdd/check.sh            # 或 scripts/verify.sh，含本门禁
# 5. 生产 IT（可选，有部署目标时）
laya-workflow bdd build --profile production --base-url <real>
laya-workflow bdd run --profile production --base-url <real>
```

## 四、准确率策略（为什么这样分）

- **确定性编译器 = 100% 精度**：step 正则 → op/assertion/参数，0ms，无歧义。
  一切能靠规则表达的步骤都走这里。
- **needle 只做"建议"**：`--assist` 对词汇表外步骤跑端上小模型（多工具+triggers），
  打印 `op + 置信度`，`≥0.5` 标 LIKELY、`<0.5` 标 guess。它**绝不自动编译**——
  研究（`docs/bdd_to_needle.md`）实测：基础模型对生造措辞全字段准确率约 40%、
  只有 ~10% 过 0.1 置信度 floor，所以建议只是给 agent 的一个输入，最终以
  `plugins/bdd/main.rhai` 里的确定规则为准。
- **validate 双保险**：build 生成的每个 spec 都跑真实 `laya-workflow validate`，
  结构不合法即失败。

## 五、常见错误 → 正确动作

| 报错 | 含义 | 动作 |
|---|---|---|
| `unknown then step: '...'` | 词汇表不认这个 step | 用 `--assist` 看建议 → 加 `plugins/bdd/main.rhai` 规则 |
| `step coverage 40% < 100%` | feature 里有未匹配步骤 | 同上，或改写成标准措辞 |
| `no @production scenarios selected` | 生产计划为空 | 给场景加 `@production` tag |
| `--profile production needs --base-url` | 生产没给目标 | 传 `--base-url` 或设 `$BDD_BASE_URL` |
| `validate rejected the generated spec` | 生成的 spec 结构不合法 | 改 feature 让它编译出合法图 |

## 六、目录速查

> **隔离原则**：BDD 源与 JSON 完全隔离在两个根目录——所有 `.feature` 源在 `bdd/workflows/<类别>/`（层级镜像 `dsl/`），所有 JSON 工件在 `dsl/` + `bench/`。浏览器 feature 的侧车配置 `<name>.config.json` 归入 json 树（`dsl/browser/`），authoring 造册/args 门禁等工具数据归入 `bench/bdd/`。因此 **`bdd/` 内 0 个 `.json`**（含 bench/args_probes），`dsl/` 内 0 个 `.feature`。BDD 是源、JSON 是编译产物（单向生成）。这条隔离现在由门禁**机器化**（P44）：`dsl smoke` 末尾内置 `isolation_audit` 并作为回归 `bdd-json-isolation`，见下方「bdd/json 两树隔离不变量」。

- `bdd/features/*.feature` — 正式场景（CI 门禁的 glob）
- `bdd/features/setup/` — `include:` 拉入的步骤列表（无 Feature 头）
- `bdd/workflows/` — 全部 BDD workflow 源（单一层级，镜像 `dsl/`）：`browser/`（29 实战站点 live 语料 + `_probe`）、`examples/`、`tasks/`、`routing/`、`agents/`（决策图 `@decision` 类别，随转换持续扩展）
- `bdd/fixtures/` — 本地 hermetic 夹具页（`app.html` 是"无接口 UI 转工作流"的示例目标）
- `bdd/features/workflow_capture.feature` — 工作流语义的端到端示例（extract/key/wait_until/条件点击/@outputs）
- `laya-workflow bdd vocabulary-check` — 词汇表（step → op/assertion）
- `laya-workflow bdd transpile` — 编译器（Gherkin → spec）
- `laya-workflow bdd build` — 统一 accuracy gate + 多产物（本指南的核心命令）
- `laya-workflow bdd run` — 执行器（本地 hermetic / 生产 IT）
- `laya-workflow bdd build --assist` — needle 建议模块（只建议，不编译）
- `bench/bdd/` — bdd 工具链 JSON 数据（authoring 造册 `tasks/*.json`、实战结果 `live/`、`args_probes.json`）；`bdd/` 内无任何 `.json`
- `bench/bdd/live/REPORT.md` — 实战结果（29 passed / 842 steps）+ BDD vs 手写 JSON 效率对比
- `laya-workflow bdd live` — 跑实战语料（需要 Chrome，**绝不在 CI**）
- `laya-workflow bdd compare` — BDD vs 手写 JSON 的体积/门禁对比（离线，可进 CI）

## 七、决策 workflow（@decision）—— agentic 决策图的 BDD 写法

> 这是把 **agentic 决策 workflow** 变成 BDD 的主路径（也是把 `dsl/<cat>/<name>.json`
> 里 99 个决策 spec 全量转成 `.feature` 的做法）。一条 `@decision` feature = 一张决策图：
> **Feature = 整张图，每个 Scenario = 一个决策节点**。编译产物是决策图 spec
> （`nodes/start/max_iterations/policy/capabilities`），不是浏览器步骤 spec。

### 文件放哪 / 来源

- **绿场起点**：`laya-workflow bdd scaffold --name <wf> --cat <cat> --intent "<intent>"`
  一条命令开出最小合法 `@decision` 骨架（自带 authoring 词汇注释、自验证能过 `bdd build`）。
  默认是**典型两节点图**（choice → score → STOP，带 threshold label + copy_keys 节点），
  复制 Scenario 加节点、改 edge 即可长成真实多节点 workflow；`--single` 退化为单节点起点；
  `--nested` 开 prefilter → 子工作流 → audit 管线起点；`--pattern gate` 开单节点门禁骨架
  （choice → `Then gate "<q>" on option "B" with block 0.6 allow 0.7`，适合风险/审批）；
  `--pattern loop` 开迭代精修自环（Background `at most N iterations` + verdict 节点
  `revise` 回自身 + `allow up to 3 retries`）；`--pattern` 取值见 `bdd scaffold --help`；
  `--stdout` 只打印不写文件，重复写需 `--force`
- 源：`bdd/workflows/<cat>/<name>.feature`（层级镜像 `dsl/<cat>/`，如 routing/、agents/、capabilities/…）
- 编译产物：`dsl/<cat>/<name>.json` —— **这是权威 spec，由 `bdd round-trip` 证明等价，不许手改**
- 门禁：`laya-workflow bdd round-trip` —— 每个 `@decision` feature 必须逐字段复现其 dsl spec；
  **反向完整性**：任何有 dsl spec 但没有 `@decision` 源的，round-trip 直接 FAIL（"convert it"）。

### 结构：Feature = 图，Scenario = 节点

```gherkin
@decision
Feature: support_ticket_router

  Background:                        # 整张图的设置（可选）
    Given the workflow runs at most 20 iterations

  Scenario: classify                 # 一个决策节点
    Given a choice question "category" asks "Which team should handle this ticket?"
      | answer    | meaning          |   # choice 表：第一行是表头 | answer | meaning |
      | billing   | invoices, ...    |   # 其余行是 criteria
    When the answer "technical" goes to "severity_probe"   # edge 条件 → 目标节点
    And any other answer goes to "STOP"                    # edge 默认
    Then keep the answer to "category"                     # copy_keys
    And require confidence at least 0.4                    # min_confidence
    And allow up to 3 retries                              # max_retries

  Scenario: severity_probe           # 第二个节点
    Given a score question "severity" asks "How severe is the impact?"
      | level    |                   # score 表：| level |（criteria 是 level 列表）
    When any other answer goes to "STOP"
    Then label "PAGE_ONCALL" when "severity" is at least 1.5   # 有序 threshold 规则
    And label "BACKLOG" otherwise                               # 兜底
```

### 决策词汇表（Given / When / Then）

**Background（整图级）**：`Given the workflow runs at most N iterations`（max_iterations）、
`Given the policy`（| key | value | 表 → policy 块）、`Given the capabilities`（| name | config-JSON | 表）。

**Given 问题（每节点 ≥1 个问题）**：
- `a choice question "Q" asks "..."` + `| answer | meaning |` 表
- `a score question "Q" asks "..."` + `| level |` 表
- `a noul question "Q" asks "..."` + 可选 `| truthy | ... |` / `| falsy | ... |` 表
- `the sub workflow "NAME"` —— 节点只引子工作流（不能混节点内容）
- `And when the sub workflow completes continue at "NODE"` —— 子工作流完成后的**交接点**：子内部 STOP 边被重定向到调用方节点（P25；参考 `bdd/workflows/pipelines/intake_pipeline_nested.feature`）
- 启发式（跟在问题后、配置**最近一个**问题）：
  - `answer "X" when the field "F" contains <needles> with hit N [and miss M]`（field_any）
  - `answer "X" when the text contains <needles> with hit N [and miss M]`（any）
  - `score "X" from the numeric field "F" [with miss M]`（numeric）
  - `answer "X" when the field "F" matches <patterns> with hit N [and miss M]`（regex）
  - `answer "X" with mid N when the field "F" contains <needles>`（mid）
  - `answer by rules on the field "F" with hit N [and fallback "X"]`（match_rules + | label | needles | 表）
  - `score "X" when the field "F" contains "HIGH" scoring H and "MID" scoring M miss L`（score_bands）
  - `the answer is always "X"`、`the answer comes from the field "F"`

**When 边**：`the answer "X" goes to "NODE"`（edge 条件）、`any other answer goes to "NODE"`（默认）。

**Then 动作**：
- 阈值规则（**有序**，round-trip 断言顺序）：`label "L" when the answer to "Q" is "X"` /
  `... is one of "X","Y"` / `... when the answer is "X"`（implicit）/ `... with at least C confidence` /
  `label "L" when "Q" is at least N` / `label "L" when "Q" is below N` / `label "L" otherwise`
- state：`keep the answer to "Q"`（copy_keys）、`keep the state key "K"`
- 节点设置：`the primary question is "Q"`、`the action reads the question "Q"`、
  `require confidence at least N`、`allow up to N retries`
- 能力调用：`call the capability "NAME"` + `| with.<k> | ... |` / `| chain.<as>.capability | ... |` 表
- 高级动作：`gate "Q" on option "O" with block B allow A`、`merge open probabilities on "Q"`、
  `combine "C" with signal "S" fallback "F" review "R" requiring MIN high H low L`、
  `rubric "Q1" at L1 levels weight W1 and "Q2" at L2 levels weight W2 requiring M`、
  `prepare a UI proposal from operation "OP" field "F" amount "A" surface "S" hash "H" requiring M [value "K"]`

> 单元格里要写字面 `|`（如 shell 管道）时转义成 `\|`；JSON 值里的 `\r\n` 等按 JSON 转义。

### 正确性 & 行为门禁（决策路径怎么被证明对）

写出来的 `.feature` 不仅要能编译，还要被证明**真的按你写的意图决策**。这些门禁全部离线、进 `scripts/bdd/check.sh`：

- **语义图校验（P24-P27, P39）**：`from_spec` 阶段跑 `validate_graph_semantics` —— edge 目标 / action kind / threshold question 引用 / 节点名唯一 / 死节点可达性 / primary_q 存在 / copy_keys 键 ∈ 本节点 question / combine fallback ∈ criteria / workflow 引用环（A→B→A 拒绝并命名链条，菱形不误报）/ edge 条件键 ∈ 主问题 criteria / **`call` action 的 `capability` 以及每个 `chain[].capability` ∈ 本 spec 的 `capabilities`（P39）**。任何一条违反在 build 阶段就拒绝，而不是运行时静默错（真实抓过 3 处死代码/死路由）。
- **行为 smoke（P7 起）**：`dsl smoke` 用离线 HeuristicBackend 跑每个 dsl spec 注册的样本状态（`bench/dsl_smoke_states.json`），按 `bench/dsl_smoke_expect.json` 断言决策输出。106/106 specs OK。
- **决策输出断言（P21-P30）**：expect 表三种形态 —— ① string 钉最终 label/gate_action；② object 按 dotted-path 钉决策输出（`result.route`/`result.score`/数值 1e-9 容差）；③ P30 起 `bdd score --run` 的决策 e2e 编译 **authored feature** 跑同样状态、用同一个 `check_expect` 断言（修「跑通但决策错」的空洞 e2e，10/10 authoring task 全钉）。
- **决策 trace 全钉（P31 → P38 收紧）**：一个 state 走 >1 个节点时，它的 expect **必须钉完整条路径**——每一步的 `trace.steps.N.node` / `.action` / `.answer`，再加上 `trace.step_count`。最终 label 对但中间走错节点、或某步跑了不同 action、或某步用的是不同路由 answer 的 feature 都会被拦（`walks N nodes but expect pins an incomplete trace path — missing ...`）。P38 之前只强制钉 step 0，现在**首步钉死也不再够**（首步-only / 仅 node 都拒）。21/21 multi-node stem × 60 个 multi-node state 全钉（dialogue_policy 状态机 retry 自环、persist_memory 5 节点链、triage_then_moderate gate_action 等）。回归测试 `bdd-smoke-trace-path-floor` 让这条要求是结构性的，并带「篡改任意一步 answer → 被 check_expect 抓住」的负例。
- **状态覆盖广度（P40）**：`bench/dsl_smoke_states.json` 里每个决策 spec 的**每条可达分支**都应有注册 state 走到，否则那条边的行为从没被验证过（"能编译" ≠ "那条路走对了"）。P40 补了 integration_hub 的 4 条 `via_*` 分支、protocol_services 的 `do_redis`，并把 triage_then_moderate 三份**一模一样**的 expect 拆成三条真实路径（billing 走满 `triage → billing_gate → refund_amount`），靠的是给 routing 问题加 `And the answer comes from the field "<f>"`（answer_field，只影响离线 HeuristicBackend）。
- **answer_field 权威性 + 动态 criteria（P41）**：带 `heuristic.answer_field` 的问题**一律先从该 state 字段作答** —— 即使该问题名（如共享的 dialogue-policy `intent`）另有具名 handler，`answer_field` guard arm 也排在 `match qid` 首位、优先生效，因此一个 spec 能让**共享问题名**离线确定性地走分支、无需改 Rust。`criteria.from_state`（动态候选）的问题，state 给出的候选 id 即合法取值（offline 复刻「模型从实时候选里选了一个」）。缺 id / 未知 id **fail closed 到 `none`**（低置信），绝不静默取第一个候选。dialogue_policy 全 DFA（greeting→identifying→authenticated→collecting→confirm→done）与 span_selection `pick` 分支靠它离线走通。回归测试 `heuristic-answer-field`（4 断言：answer_field 胜具名 handler / 动态 criteria 接受 state id / 缺 id fail closed / multi-node 缺 expect 被拒）。
- **multi-node state 缺 expect = 硬失败（P41）**：一个 state 走 >1 节点却**完全没有** expect 条目时，过去被 `if let Some(want)` 静默跳过 —— 现在直接判 FAIL（`missing_trace_pins(Null, n)` 返回全部键）。smoke 门禁与 P38 回归测试因此永远不可能不一致。
- **分支覆盖门禁（P42，结构性）**：P40 的「状态覆盖广度」从一次性人工审计升级为**结构性不变量** —— 回归 `bdd-smoke-branch-coverage` 对每个带注册 state 的 spec，跑全部 state（真实引擎）取并集，断言**图里从 `start` 可达的每个节点**都被某个 state 走到；新加一条可达边却没 state 走它 = 硬失败。白名单每条都断言「仍是可达节点」，杜绝腐烂成静默空洞（P43 后只剩 1 条：`cua_desktop.exec` 要真 cua-driver；原来那两条 `intake_pipeline_nested.intake` / `support_ticket_router_hybrid.needle_fallback` 已按下方 P43 结构性修掉）。本轮补全 `web_research` 的 `read`/`markdown` 分支（给 `route.mode` 加 `answer_field`，离线按 state 分流）。门禁自报覆盖规模（现 **68/69** 个 state-registered spec；余下 1 个外连目标 spec 离线跳过）。
- **expect 支持布尔输出（P42，易用性）**：`check_expect` 原本只认 string/number，`_ => false` 让**任何布尔断言永远失败** —— 作者无法钉 `truncated`/`blocked`/`seen` 这类布尔决策输出。现支持 `Value::Bool`（`got.as_bool() == Some(want)`），`dsl smoke` 得以钉 `result.truncated`。回归 `bdd-smoke-branch-coverage` 双向证明（钉 true 通过 / 钉 false 失败）。
- **needle 离线可达 + 容器节点结构性修正（P43，正确性）**：① `ensure_env_dirs` 现补种 `${env.NEEDLE3_CACT}` —— 每个 `kind: needle` capability 都插值 `cact`，缺它时离线后端在跑之前就以 `unresolved reference: ${unresolved:env.NEEDLE3_CACT}` 拒掉整个 spec，于是**所有 needle workflow 离线不可达**。补种占位值后，`needle_demo` / `needle_ticket` / `support_ticket_router_hybrid` 的 hybrid fallback 都能被离线引擎驱动；本轮给前两个各注册一个 state、给 hybrid 补一条 `sales` state（`classify` 默认边 → `needle_fallback`），`needle_fallback` 于是从白名单变真覆盖。② 分支覆盖门禁原把**内联子工作流容器**当普通节点要求「必被访问」，可容器是引擎就地展开的——它的子步骤在 trace 里表现为 `intake::classify` / `intake::severity_probe`，容器名 `intake` 按构造永远不是 trace step。门禁现在识别节点的 `workflow` 字段、把容器从「必被访问」集合里排除（仍沿它的 `edge` 继续走，所以下游 `audit` 照样要求被覆盖），`intake_pipeline_nested.intake` 不再需要白名单。**白名单 3 → 1**。
- **覆盖台账门禁（P43，正确性/可审计）**：`dsl smoke` 过去只行为验证「有注册 state」的 spec，没 state 的 spec 静默 `validate only` —— 一个没人会标为「未验证」的 workflow。现在台账是**全量**的：每个被发现的 spec 要么离线注册（`bench/dsl_smoke_states.json`），要么在 `bench/dsl_coverage_ledger.json` 的 `live_only` 里声明**覆盖它的门禁**（`covered_by`）和**为什么离线 smoke 覆盖不了**（`reason`）。两者皆非 = 硬失败（`register a sample state, or declare ...`）；**声明腐烂**同样硬失败（该 spec 已离线注册、或已不存在 → `stale ledger entry`）。`scripts/bdd/check.sh` 现在把 `coverage ledger: N offline-verified + M live-declared = T specs` 打出来。回归测试 `bdd-coverage-ledger`（含两条负例：未声明的 spec 被抓、声明了不存在的 spec 判 stale）。实测 **71 offline-verified + 35 live-declared = 106 specs**（35 = 27 browser，走 `bdd live` / `bdd run` 需 Chrome；+ 8 capability，需真 driver / 凭据 / duckdb / HTAP daemon / 租户 / 真 runner / 外网）。
- **bdd/json 两树隔离不变量（P44，结构性/可审计）**：把标准约束机器化 —— `dsl smoke` 末尾对它所在的仓库根跑 `isolation_audit`，并作为回归测试 `bdd-json-isolation`。以下**任一**即硬失败：`bdd/` 内出现**任何** `.json`（spec 属于 `dsl/`）；`dsl/` 内出现**任何** `.feature`（源属于 `bdd/`）；`bdd/` 树**之外**出现 `.feature`；任一树根下**裸放**源文件（`bdd/*.feature` 或 `dsl/*.json`，即没归入类别文件夹、破坏层级）。非源非 spec 的支撑文件（`bdd/fixtures/*.html`、各 `README.md`、`dsl/capabilities/hello.rhai`、`*.config.json` 侧车）不在检查范围。`scripts/bdd/check.sh` 现打印 `isolation: bdd/ holds no json, dsl/ holds no feature; both trees are hierarchical`；`--dsl-dir` 覆盖（指向别处）时跳过本检查。回归 6 断言：真仓 0 problem + 良构两树通过 + 4 条负例（bdd 混入 json / dsl 混入 feature / 树根裸放 / bdd 外 feature），所以门禁不可能在空树或错形状树上假绿。
- **validate 图语义硬化（P45，正确性）**：`validate` 现编译期拒掉 4 类「**能编译过、运行期静默 mis-route / 永不执行**」的 spec（此前全部放行）：① 节点 `min_confidence`（或 `edge.min_confidence`）∉ [0,1]（>1 让每条 route 都失败 → 节点永远 ESCALATE；<0 是笔误）；② `max_iterations` 存在但非正整数（0/负数 → 图跑 0 次；非整数 → 静默回落默认 50）；③ threshold 阶梯缺终局 catch-all（某些 score 无规则命中 → 输出字面量 `"unmatched"` label）；④ threshold 规则重复 `when`（首匹配胜出 → 重复规则永不触发）/ catch-all 不在末尾（其后规则是死代码）/ 规则缺 `label`（输出字面量 `"unknown"`）。回归测试 5 项共 18 断言：`validate-min-conf-range` / `validate-max-iterations` / `validate-threshold-not-exhaustive` / `validate-threshold-dead-rules` + `validate-repo-specs-strict`（**真仓不变式**：`dsl/**/*.json` 全部 **106** 个 spec 在更严规则下仍 parse 成功，把「零假阳性」机器化）；每个负例都断言错误信息含关键字（`min_confidence` / `max_iterations` / `unmatched` / `repeats`），并配合法 spec 仍 `is_ok()` 的控制组防假阳性。
- **ole-eval port #9 `alert_noise_reduction` BDD 化（P48，适配性）**：告警降噪单节点判定移植到 `dsl/ole_eval/alert_noise_reduction.json`（`bdd/workflows/ole_eval/alert_noise_reduction.feature` 为源，`bdd transpile` 生成）。模型只提 typed context（`cause_group` / `release_correlation` / `business_period`）；阈值（每指标可配：cpu 85 / p95 500ms / error_rate 5% / disk 85 / queue 1000 / memory 90）、趋势（最小二乘斜率 vs 阈值 2%）、去重窗口（300s）、冷却窗口（900s）、严重度合成与最终 suppress/escalate 全部由本地规则引擎裁决——`breach`（ratio≥1 折叠成 band）、`stream`（dedupe/cooldown 时钟状态折叠成 first_seen/duplicate/cooldown）、`severity`（base ratio band + correlated/rising 与 business_peak 提升、off_peak/falling 下调折叠成一个 choice）三个折叠问题都**文档化**。action 有序阶梯复刻引擎早返回链：`reject_context` → `clear` → `duplicate` → `cooldown` → `low_severity` → `page`(critical/high) → `ticket`(warning)。8 个离线样本（`ev_*` answer_field 驱动，零新 Rust handler）覆盖全部 7 个 label；单节点，全节点被分支覆盖门禁走到。dsl smoke 102/102、round-trip 102/102、ledger 67+35=102。
- **ole-eval port #10 `self_healing_gate` BDD 化（P49，适配性）**：自愈决策闸门移植到 `dsl/ole_eval/self_healing_gate.json`（`bdd/workflows/ole_eval/self_healing_gate.feature` 为源，`bdd transpile` 生成）。模型只提 typed diagnosis（`fault_class` / `evidence` / `candidate_remediation`）；确定性分类链（`crash_loop` > `deployment_changed`+`error_rate_high` > `dependency_timeout` > `traffic_above_capacity` > `zone_unhealthy` > `memory_growth` > unknown）、证据核验、action 权限（restart→process_crash / drain_bad_instance→memory_leak / rollback_deployment→deployment_regression / failover→{dependency_latency,zone_isolation} / scale_out→traffic_overload）、优先级、cooldown、rate limit、blast radius、rollback 与终态全部由本地规则引擎裁决。四个折叠问题**文档化**：`proposal_schema`（malformed / 注入 / 未知 action 折叠成 rejected）、`service_state`（DEGRADED→STABILIZING/RECOVERED/ROLLED_BACK/TERMINAL 状态机；终态集 {recovered,rolled_back,terminal} 拒重入）、`stream`（per service+action cooldown 与 per window rate limit 折叠成 idle/cooldown/rate_limited）、`effect`（in-memory action_success/recovered/rollback_success 折叠成 stabilized/recovered/rolled_back/terminal）。action 有序阶梯**严格复刻引擎早返回链**：`reject_diagnosis` → `deny_terminal` → `escalate_human`(evidence/fault/permission fail) → `defer_cooldown` → `defer_rate_limit` → `escalate_human`(blast) → `execute_rolled_back` / `execute_terminal` / `execute_recovered` / `execute_stabilizing`。12 个离线样本（`ev_*` answer_field 驱动，零新 Rust handler）覆盖全部 9 个 label；单节点，全节点被分支覆盖门禁走到。dsl smoke 103/103、round-trip 103/103、ledger 68+35=103。
- **ole-eval port #11 `loan_dialogue` BDD 化（P50，适配性）**：贷款申请 slot-filling 对话 + 承保移植到 `dsl/ole_eval/loan_dialogue.json`（`bdd/workflows/ole_eval/loan_dialogue.feature` 为源，`bdd transpile` 生成）。5 节点 DFA：`awaiting_income` → `awaiting_debt` → `awaiting_amount` → `awaiting_term` → `complete`。模型只提 typed facts（`annual_income`/`monthly_debt`/`loan_amount`/`term_months`）；每轮必须恰好填期望槽位——skip-step / 多字段轮、越界 / 非法值、完成后再次提交，全部本地拒绝。`bdd build` **42/42 = 100%**；11 个离线样本覆盖全部 6 个终态 label（`APPROVE`/`REJECT`/`reject_duplicate`/`reject_value`/`reject_skip` + 中间槽位 label），9 个 multi-node 样本逐步钉 trace（`trace.step_count` + 每步 node/action/answer/next）。**折叠/文档化边界**：Decimal 精确摊销、DTI > 43.00 / payment-to-income > 28.00 阈值折算进终态 `outcome` choice；COMPLETE 后重复轮折叠为 `duplicate`。states 68→69 spec/240→251 state、expect 65→66 spec/231→242 state/60→69 multi-node、ledger 69+35=104；dsl smoke 104/104、round-trip 104/104、check.sh 全绿。
- **ole-eval port #12 `sla_checker` BDD 化（P51，适配性）**：确定性 UTC SLA 评估移植到 `dsl/ole_eval/sla_checker.json`（`bdd/workflows/ole_eval/sla_checker.feature` 为源，`bdd transpile` 生成）。单节点判定：模型只提 typed facts（`proposal` 解析态 / `rule` schema 态 / `event` schema 态 / `sla` 总体态 / `severity` 超限比档），schema 校验（rule_id 格式 / `P1..P99` / `response_minutes <= resolution_minutes` / 窗口非空不重叠 / 六键事件 / 仅 UTC `Z` 时间戳 / 时间戳先后）、冲突、重复（rule registry / event ledger）、SLO 状态与严重度全部由本地有序 rules 裁决。**折叠/文档化边界**：`add_business_minutes` / `business_minutes_between` 的 UTC 业务窗口分钟累计（next-opening / 日窗重叠 / 无 DST 算术）不进 DSL —— 由本地算出，仅以 `sla` + `severity` 两个 choice 呈现；JSON 解析细节（```json 围栏剥离、重复 JSON key 拒绝、恰 `{rule,event}` 形状）折叠进 `proposal` choice；`_MAX_SLO_MINUTES` / `_MAX_TIMELINE_DAYS` 边界折进同一 schema 桶。12 个 label 的有序阶梯：`reject_proposal` → `reject_rule` → `reject_rule_conflict`（conflict / overlap）→ `reject_duplicate_rule` → `reject_event`（bad_schema / bad_time / unknown_priority）→ `reject_duplicate_event` → `breach_critical` → `breach_high` → `breach_medium` → `breach_low` → `met` → `pending`（catch-all）。15 个离线样本（`ev_*` answer_field 驱动，零新 Rust handler）覆盖全部 12 个 label；单节点，全节点被分支覆盖门禁走到。`bdd build` **27/27 = 100%**；states 69→70 spec/251→266 state、expect 66→67 spec/242→257 state（multi-node 仍 69）、ledger 70+35=105；dsl smoke 105/105、round-trip 105/105、check.sh 全绿。
- **ole-eval port #13 `api_quota_guard` BDD 化（P52，适配性）**：确定性 API scope/quota/rate/concurrency/budget 闸门移植到 `dsl/ole_eval/api_quota_guard.json`（`bdd/workflows/ole_eval/api_quota_guard.feature` 为源，`bdd transpile` 生成）。单节点判定：模型只提 typed request intent（`principal` / `endpoint` / `requested_units` / `context_intent`）；schema 校验（枚举 principal/endpoint/context_intent、整数 units ∈ [1,1000]、无重复 JSON key、禁 authority/side-effect 字段）、request-id 重放台账、时钟单调性、scope 准入、per-endpoint 单请求上限、并发上限、令牌桶、日/月单位配额与花费预算全部由本地有序 rules 裁决。**折叠/文档化边界**：JSON 解析细节折进 `proposal` choice；令牌桶 refill 算术（`elapsed_ms * refill_units_per_second / 1000`，封顶容量）与其隐含的 `retry_after_ms` 是确定性 Decimal 时钟算术、不进 DSL（桶仅以 `bucket` = ok/insufficient 呈现）；`daily_reset_after_ms > 0` 的「重置提示 → throttle-with-retry vs 直接 reject」折进 `quota` 的 `daily_retry` / `daily_no_retry` 两个答案；per-principal/per-endpoint 固定策略表（scopes/limits/budgets/costs/capacities/refill）是本地 fixture 非模型输入。12 个 label 的有序阶梯：`reject_proposal` → `reject_replay` → `reject_clock_regression` → `reject_scope` → `reject_units_above_max` → `throttle_concurrency` → `throttle_bucket` → `throttle_daily` → `reject_daily_no_retry` → `reject_monthly` → `reject_spend` → `allow`（catch-all）。15 个离线样本（`ev_*` answer_field 驱动，零新 Rust handler）覆盖全部 12 个 label，含 3 例跨 label 优先级（proposal-first / replay-before-scope / quota-before-budget）；单节点，全节点被分支覆盖门禁走到。`bdd build` **35/35 = 100%**；states 70→71 spec/266→281 state、expect 67→68 spec/257→272 state、ledger 71+35=**106**；dsl smoke 106/106、round-trip 106/106、check.sh 全绿。
- **offline-detection（P29）**：网络型 stem（web_search/web_fetch/agent/rpc/http/webhook…）在本地 mocks 全不可达时 state 判 skip（validate-only），不误报回归；mocks up 时（`bench/mock_services.sh start` + 8791/8792 `mock serve`）全量强断言。
- **offline-external skip（P33，效率轴）**：离线 smoke **绝不对外网做真实 I/O**。`plugin` 型 capability 没有 `endpoint`/`url` 字段，改从插件脚本静态扫描 `http_get` 调用 + URL 字面量（去注释、识别字符串）判定网络依赖；spec 的网络目标只要含外部 host（非 loopback/RFC1918/link-local/CGNAT）就在离线 smoke 里整 state skip（validate-only，集成/live 任务真跑）。插件 `http_get` 在离线 smoke 里再叠一层 1.5s 超时上限兜底动态 URL。效果：`hf_trending`（huggingface.co 插件）从每次门禁 **16s → 0.2s**，check.sh 从 **15.5s → 1.8s**；注释里提 URL 的插件（如 cua 的 github 链接）不会误判。回归测试 `bdd-smoke-offline-external-skip`。
- **did-you-mean（P28 + P34）**：拼错有两层兜底 —— ① **步骤层（P28）**：决策步骤拼错/放错关键词时，`bdd build` 覆盖门禁逐条给 `did you mean the decision <kind> step: <tmpl>?`；② **语义层（P34）**：`validate_graph_semantics` 的**每个未知引用**（routing 目标 / start / primary_q / action kind / 问题引用 / answer key / gate option / combine fallback / when 运算符 / call capability）都追加 `(did you mean "..."?)`，候选来自图内真实标识符（节点名 + answer 值 / 问题名 / 已知 kind），相似度 <0.5 直接不给（绝不误报）。作者在 build 报错处就能看到该改成什么。
- **错误定位线号（P35）**：语义校验错误**自带 `.feature` 行号**（`line N: node X: ...`）—— `validate_graph_semantics` 在 `spec.rs` 里跑、离 feature 文本很远，`bdd build` / `bdd transpile` / MCP `bdd_validate` 三处出口都用 `locate_semantic_error_line` 按错误语法（`unknown "X"` / `"X" is not ...` / `... capability "X"`）从 parsed step 里反查最可能的出错行（P39 起 `call the capability` 也精确定位到该行）。agent 拿到的是「哪一行 + 改成什么」，不是一句裸拒绝。

expect 表现况：`bench/dsl_smoke_expect.json` 覆盖 **68** 个 spec / **272** 个 state（多节点 spec 的每个样本按 P38 逐步钉 `trace.step_count` + 每步 node/action/answer；trace-pinned stems **22**）；`bench/dsl_smoke_states.json` 注册 **71** 个 spec / **281** 个 state；**分支覆盖门禁** 断言每个 state-registered spec 的可达节点全集被走到（白名单仅 1 条，容器节点已结构性排除）；**覆盖台账** `71 offline-verified + 35 live-declared = 106 specs`。门禁实测：`bdd check.sh` 全绿、`dsl smoke` 106/106、`bdd round-trip` 106/106、`bdd doc-check` 9/9；dsl 侧回归套件（bdd-smoke-expect-coverage / -trace-path-floor / -branch-coverage / -offline-external-skip + bdd-coverage-ledger / bdd-json-isolation / bdd-score-corpus-floor / bdd-score-e2e-decision / bdd-decision-roundtrip-vocab / bdd-transpile-decision / validate-repo-specs-strict）**590/0**。`laya-workflow-tests` 全量（mocks up）**2069/0**；offline 全量在当前环境会卡在 `capability-timeouts` 的 resource-leak soak（socket 读取不返回；单跑该 section 亦复现，与 BDD 改动无关，已登记待查），故本轮 dsl 侧回归以 section 直跑为准。

### 作者循环（agent 怎么写一个决策 workflow）

1. 想清楚决策图：几个节点？每个节点问什么问题、答什么路由、产出什么 label/动作。
2. 绿场起点：**MCP `bdd_new`**（`name`+`cat`+`intent` → 最小合法 `@decision` `.feature` 骨架，
   自验证能过 `bdd build`；`write=true` 落盘、`root` 指定工作区）或 CLI `laya-workflow bdd scaffold`。
3. 在 `bdd/workflows/<cat>/<name>.feature` 扩展（先查词汇表，生造措辞会被拒）。
4. 校验：MCP `bdd_validate`（喂 feature 文本/路径 → per-scenario compile + step coverage +
   validate 逐行错误）或 CLI `laya-workflow bdd build`。拼错会收到 **`line N:` + `did you mean ...?`**
   （P28 步骤层 + P34 语义层 + P35 行号），照建议在对应行改即可。
5. 出 spec：**MCP `bdd_transpile`**（feature → dsl spec，`@decision` 单图 / browser 逐 scenario；
   dsl_version 剥离对齐 dsl 约定；`write=true` 落盘 dsl/<cat>/ 带 exists 防覆盖）或 CLI
   `laya-workflow bdd transpile <file> --out dsl/<cat>`。
6. 造册审计：MCP `bdd_catalog` —— 双向完整性（dsl↔feature），报 orphan_specs / features_without_spec。
7. 全量门禁：`laya-workflow bdd round-trip`（106/106 复现）+ `scripts/bdd/check.sh`。
6. 提交时 `bdd round-trip` 会拦：dsl 有 spec 但没 `@decision` 源 = FAIL。

### 为什么这样是 agentic workflow 的核心

- **确定性**：`@decision` 词汇表是确定性编译器（正则 → 决策图），0ms、无歧义；validate 双保险。
- **正确性可证**：语义图校验 + 决策输出断言 + **决策 trace 全钉** + offline-detection，让"决策对没对"是
  可测、可复现、可回归的（P24-P31），不只是"能编译"。
- **单一事实来源**：`.feature` 是源，`dsl/*.json` 是编译产物；round-trip 证明等价，杜绝双份漂移。
- **可直接运行**：`laya-workflow run --spec dsl/<cat>/<name>.json` 用 HeuristicBackend / 真实 LLM 跑决策图。
- **完整性可审计**：`bdd_catalog` 让"所有已知 workflow 都 BDD 化"这件事可测（孤儿即报）。
