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
# 2. 本地门禁（编译 / 词汇 / 离线 smoke；check.sh 末段真跑浏览器，无浏览器自动跳过）
laya-workflow bdd build --assist
# 3. 有词汇表外步骤？先看 --assist 的建议（带置信度），
#    把可接受的规则写进 plugins/bdd/main.rhai，再重跑。建议永远不直接编译。
# 4. 真跑浏览器（有 Chrome 的机器）：
laya-workflow bdd run           # 无浏览器时打印 skipped 并 exit 0；--require-browser 则改判 error
scripts/bdd/check.sh            # 或 scripts/verify.sh：编译 + 离线 smoke + 浏览器真跑（无浏览器自动跳过）
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
- `laya-workflow bdd run` — 执行器（本地 hermetic / 生产 IT）；无浏览器时 skip（exit 0），`--require-browser` 改判 error，`LAYA_BDD_SKIP_BROWSER=1` 关掉 `check.sh` 里的这一段
- `laya-workflow bdd build --assist` — needle 建议模块（只建议，不编译）
- `bench/bdd/` — bdd 工具链 JSON 数据（authoring 造册 `tasks/*.json`、实战结果 `live/`、`args_probes.json`）；`bdd/` 内无任何 `.json`
- `bench/bdd/live/REPORT.md` — 实战结果（29 passed / 842 steps）+ BDD vs 手写 JSON 效率对比
- `laya-workflow bdd live` — 跑实战语料（需要 Chrome，**绝不在 CI**）
- `laya-workflow bdd compare` — BDD vs 手写 JSON 的体积/门禁对比（离线，可进 CI）
- `laya-workflow bdd port-check --sources <dir>` — 跨仓转换闭合性（每个上游 scenario 都有 BDD 端口、每个端口都有上游来源）；`LAYA_OLE_EVAL_SOURCES` 指向 ole-eval `code/qwen_vm/app_scenarios` 时由 `check.sh` 真跑（未设则跳过）
- `laya-workflow bdd import <spec.json> [--out <f.feature> | --into <cat>]` / `laya-workflow bdd import --dir <DIR> [--out <dir> | --into <cat>] [--json <f>] [--force]` — 正向搬运：把已有 workflow spec 转成 `@decision` `.feature`（transpile 的逆）。单文件默认打 stdout；`--into` 归入本仓 `bdd/workflows/<cat>/` 并把 spec 抄进 `dsl/<cat>/`（`name` 钉成端口名，保证落仓即过 `round-trip`/`import-check`）。`--dir` 递归批量：只收「解析成功且 `nodes` 非空」的 json，按名跳过 `*.config.json`、其余非 workflow json 记为 skipped，**一个都没命中即报错**（绝不 false-green）；每个 feature 落盘前先验证「重新解析 + 重新编译 == 原 spec（`same_graph`）」；目标已存在默认报冲突（`--force` 覆盖）。`bdd import-check` 门禁证明每个已入库 spec 都能被 import 出的 feature 编译回同一张图（`check.sh` 真跑）

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
  `label "L" when "Q" is at least N` / `label "L" when "Q" is below N` / `label "L" otherwise`；
  **合取**：把任意两个叶子条件用 ` and ` 连起来（`label "L" when <cond> and <cond>`，
  如 `label "PAGER_BILLING" when the answer to "category" is "billing" and "urgency" is at least 1.5`），
  编译成 `{"all": [...]}`（每个叶子仍可各自点名 `question`）。用于「两个信号同时成立」的场景。
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
- **行为 smoke（P7 起）**：`dsl smoke` 用离线 HeuristicBackend 跑每个 dsl spec 注册的样本状态（`bench/dsl_smoke_states.json`），按 `bench/dsl_smoke_expect.json` 断言决策输出。160/160 specs OK。
- **决策输出断言（P21-P30）**：expect 表三种形态 —— ① string 钉最终 label/gate_action；② object 按 dotted-path 钉决策输出（`result.route`/`result.score`/数值 1e-9 容差）；③ P30 起 `bdd score --run` 的决策 e2e 编译 **authored feature** 跑同样状态、用同一个 `check_expect` 断言（修「跑通但决策错」的空洞 e2e，10/10 authoring task 全钉）。
- **决策 trace 全钉（P31 → P38 收紧）**：一个 state 走 >1 个节点时，它的 expect **必须钉完整条路径**——每一步的 `trace.steps.N.node` / `.action` / `.answer`，再加上 `trace.step_count`。最终 label 对但中间走错节点、或某步跑了不同 action、或某步用的是不同路由 answer 的 feature 都会被拦（`walks N nodes but expect pins an incomplete trace path — missing ...`）。P38 之前只强制钉 step 0，现在**首步钉死也不再够**（首步-only / 仅 node 都拒）。21/21 multi-node stem × 60 个 multi-node state 全钉（dialogue_policy 状态机 retry 自环、persist_memory 5 节点链、triage_then_moderate gate_action 等）。回归测试 `bdd-smoke-trace-path-floor` 让这条要求是结构性的，并带「篡改任意一步 answer → 被 check_expect 抓住」的负例。
- **状态覆盖广度（P40）**：`bench/dsl_smoke_states.json` 里每个决策 spec 的**每条可达分支**都应有注册 state 走到，否则那条边的行为从没被验证过（"能编译" ≠ "那条路走对了"）。P40 补了 integration_hub 的 4 条 `via_*` 分支、protocol_services 的 `do_redis`，并把 triage_then_moderate 三份**一模一样**的 expect 拆成三条真实路径（billing 走满 `triage → billing_gate → refund_amount`），靠的是给 routing 问题加 `And the answer comes from the field "<f>"`（answer_field，只影响离线 HeuristicBackend）。
- **answer_field 权威性 + 动态 criteria（P41）**：带 `heuristic.answer_field` 的问题**一律先从该 state 字段作答** —— 即使该问题名（如共享的 dialogue-policy `intent`）另有具名 handler，`answer_field` guard arm 也排在 `match qid` 首位、优先生效，因此一个 spec 能让**共享问题名**离线确定性地走分支、无需改 Rust。`criteria.from_state`（动态候选）的问题，state 给出的候选 id 即合法取值（offline 复刻「模型从实时候选里选了一个」）。缺 id / 未知 id **fail closed 到 `none`**（低置信），绝不静默取第一个候选。dialogue_policy 全 DFA（greeting→identifying→authenticated→collecting→confirm→done）与 span_selection `pick` 分支靠它离线走通。回归测试 `heuristic-answer-field`（4 断言：answer_field 胜具名 handler / 动态 criteria 接受 state id / 缺 id fail closed / multi-node 缺 expect 被拒）。
- **multi-node state 缺 expect = 硬失败（P41）**：一个 state 走 >1 节点却**完全没有** expect 条目时，过去被 `if let Some(want)` 静默跳过 —— 现在直接判 FAIL（`missing_trace_pins(Null, n)` 返回全部键）。smoke 门禁与 P38 回归测试因此永远不可能不一致。
- **分支覆盖门禁（P42，结构性）**：P40 的「状态覆盖广度」从一次性人工审计升级为**结构性不变量** —— 回归 `bdd-smoke-branch-coverage` 对每个带注册 state 的 spec，跑全部 state（真实引擎）取并集，断言**图里从 `start` 可达的每个节点**都被某个 state 走到；新加一条可达边却没 state 走它 = 硬失败。白名单每条都断言「仍是可达节点」，杜绝腐烂成静默空洞（P43 后只剩 1 条：`cua_desktop.exec` 要真 cua-driver；原来那两条 `intake_pipeline_nested.intake` / `support_ticket_router_hybrid.needle_fallback` 已按下方 P43 结构性修掉）。本轮补全 `web_research` 的 `read`/`markdown` 分支（给 `route.mode` 加 `answer_field`，离线按 state 分流）。门禁自报覆盖规模（现 **68/69** 个 state-registered spec；余下 1 个外连目标 spec 离线跳过）。
- **expect 支持布尔输出（P42，易用性）**：`check_expect` 原本只认 string/number，`_ => false` 让**任何布尔断言永远失败** —— 作者无法钉 `truncated`/`blocked`/`seen` 这类布尔决策输出。现支持 `Value::Bool`（`got.as_bool() == Some(want)`），`dsl smoke` 得以钉 `result.truncated`。回归 `bdd-smoke-branch-coverage` 双向证明（钉 true 通过 / 钉 false 失败）。
- **needle 离线可达 + 容器节点结构性修正（P43，正确性）**：① `ensure_env_dirs` 现补种 `${env.NEEDLE3_CACT}` —— 每个 `kind: needle` capability 都插值 `cact`，缺它时离线后端在跑之前就以 `unresolved reference: ${unresolved:env.NEEDLE3_CACT}` 拒掉整个 spec，于是**所有 needle workflow 离线不可达**。补种占位值后，`needle_demo` / `needle_ticket` / `support_ticket_router_hybrid` 的 hybrid fallback 都能被离线引擎驱动；本轮给前两个各注册一个 state、给 hybrid 补一条 `sales` state（`classify` 默认边 → `needle_fallback`），`needle_fallback` 于是从白名单变真覆盖。② 分支覆盖门禁原把**内联子工作流容器**当普通节点要求「必被访问」，可容器是引擎就地展开的——它的子步骤在 trace 里表现为 `intake::classify` / `intake::severity_probe`，容器名 `intake` 按构造永远不是 trace step。门禁现在识别节点的 `workflow` 字段、把容器从「必被访问」集合里排除（仍沿它的 `edge` 继续走，所以下游 `audit` 照样要求被覆盖），`intake_pipeline_nested.intake` 不再需要白名单。**白名单 3 → 1**。
- **覆盖台账门禁（P43，正确性/可审计）**：`dsl smoke` 过去只行为验证「有注册 state」的 spec，没 state 的 spec 静默 `validate only` —— 一个没人会标为「未验证」的 workflow。现在台账是**全量**的：每个被发现的 spec 要么离线注册（`bench/dsl_smoke_states.json`），要么在 `bench/dsl_coverage_ledger.json` 的 `live_only` 里声明**覆盖它的门禁**（`covered_by`）和**为什么离线 smoke 覆盖不了**（`reason`）。两者皆非 = 硬失败（`register a sample state, or declare ...`）；**声明腐烂**同样硬失败（该 spec 已离线注册、或已不存在 → `stale ledger entry`）。`scripts/bdd/check.sh` 现在把 `coverage ledger: N offline-verified + M live-declared = T specs` 打出来。回归测试 `bdd-coverage-ledger`（含两条负例：未声明的 spec 被抓、声明了不存在的 spec 判 stale）。实测 **125 offline-verified + 35 live-declared = 160 specs**（35 = 27 browser，走 `bdd live` / `bdd run` 需 Chrome；+ 8 capability，需真 driver / 凭据 / duckdb / HTAP daemon / 租户 / 真 runner / 外网）。
- **bdd/json 两树隔离不变量（P44，结构性/可审计）**：把标准约束机器化 —— `dsl smoke` 末尾对它所在的仓库根跑 `isolation_audit`，并作为回归测试 `bdd-json-isolation`。以下**任一**即硬失败：`bdd/` 内出现**任何** `.json`（spec 属于 `dsl/`）；`dsl/` 内出现**任何** `.feature`（源属于 `bdd/`）；`bdd/` 树**之外**出现 `.feature`；`dsl/` 树**之外**出现 **workflow spec** `.json`（spec 以编译产物特征识别：`_bdd` 溯源键或 `nodes`+`start` 对，故 plugin 清单 / npm `package.json` / 评测数据等非 spec json 不受影响）；任一树根下**裸放**源文件（`bdd/*.feature` 或 `dsl/*.json`，即没归入类别文件夹、破坏层级）。非源非 spec 的支撑文件（`bdd/fixtures/*.html`、各 `README.md`、`dsl/capabilities/hello.rhai`、`*.config.json` 侧车）不在检查范围。`scripts/bdd/check.sh` 现打印 `isolation: bdd/ holds no json, dsl/ holds no feature; both trees are hierarchical`；`--dsl-dir` 覆盖（指向别处）时跳过本检查。回归 8 断言：真仓 0 problem + 良构两树通过 + 5 条负例（bdd 混入 json / dsl 混入 feature / 树根裸放 / bdd 外 feature / dsl 外 spec json，附一条「非 spec json 不误报」控制组），所以门禁不可能在空树或错形状树上假绿。
- **validate 图语义硬化（P45，正确性）**：`validate` 现编译期拒掉 4 类「**能编译过、运行期静默 mis-route / 永不执行**」的 spec（此前全部放行）：① 节点 `min_confidence`（或 `edge.min_confidence`）∉ [0,1]（>1 让每条 route 都失败 → 节点永远 ESCALATE；<0 是笔误）；② `max_iterations` 存在但非正整数（0/负数 → 图跑 0 次；非整数 → 静默回落默认 50）；③ threshold 阶梯缺终局 catch-all（某些 score 无规则命中 → 输出字面量 `"unmatched"` label）；④ threshold 规则重复 `when`（首匹配胜出 → 重复规则永不触发）/ catch-all 不在末尾（其后规则是死代码）/ 规则缺 `label`（输出字面量 `"unknown"`）。回归测试 5 项共 18 断言：`validate-min-conf-range` / `validate-max-iterations` / `validate-threshold-not-exhaustive` / `validate-threshold-dead-rules` + `validate-repo-specs-strict`（**真仓不变式**：`dsl/**/*.json` 全部 **160** 个 spec 在更严规则下仍 parse 成功，把「零假阳性」机器化）；每个负例都断言错误信息含关键字（`min_confidence` / `max_iterations` / `unmatched` / `repeats`），并配合法 spec 仍 `is_ok()` 的控制组防假阳性。
- **ole-eval port #9 `alert_noise_reduction` BDD 化（P48，适配性）**：告警降噪单节点判定移植到 `dsl/ole_eval/alert_noise_reduction.json`（`bdd/workflows/ole_eval/alert_noise_reduction.feature` 为源，`bdd transpile` 生成）。模型只提 typed context（`cause_group` / `release_correlation` / `business_period`）；阈值（每指标可配：cpu 85 / p95 500ms / error_rate 5% / disk 85 / queue 1000 / memory 90）、趋势（最小二乘斜率 vs 阈值 2%）、去重窗口（300s）、冷却窗口（900s）、严重度合成与最终 suppress/escalate 全部由本地规则引擎裁决——`breach`（ratio≥1 折叠成 band）、`stream`（dedupe/cooldown 时钟状态折叠成 first_seen/duplicate/cooldown）、`severity`（base ratio band + correlated/rising 与 business_peak 提升、off_peak/falling 下调折叠成一个 choice）三个折叠问题都**文档化**。action 有序阶梯复刻引擎早返回链：`reject_context` → `clear` → `duplicate` → `cooldown` → `low_severity` → `page`(critical/high) → `ticket`(warning)。8 个离线样本（`ev_*` answer_field 驱动，零新 Rust handler）覆盖全部 7 个 label；单节点，全节点被分支覆盖门禁走到。dsl smoke 102/102、round-trip 102/102、ledger 67+35=102。
- **ole-eval port #10 `self_healing_gate` BDD 化（P49，适配性）**：自愈决策闸门移植到 `dsl/ole_eval/self_healing_gate.json`（`bdd/workflows/ole_eval/self_healing_gate.feature` 为源，`bdd transpile` 生成）。模型只提 typed diagnosis（`fault_class` / `evidence` / `candidate_remediation`）；确定性分类链（`crash_loop` > `deployment_changed`+`error_rate_high` > `dependency_timeout` > `traffic_above_capacity` > `zone_unhealthy` > `memory_growth` > unknown）、证据核验、action 权限（restart→process_crash / drain_bad_instance→memory_leak / rollback_deployment→deployment_regression / failover→{dependency_latency,zone_isolation} / scale_out→traffic_overload）、优先级、cooldown、rate limit、blast radius、rollback 与终态全部由本地规则引擎裁决。四个折叠问题**文档化**：`proposal_schema`（malformed / 注入 / 未知 action 折叠成 rejected）、`service_state`（DEGRADED→STABILIZING/RECOVERED/ROLLED_BACK/TERMINAL 状态机；终态集 {recovered,rolled_back,terminal} 拒重入）、`stream`（per service+action cooldown 与 per window rate limit 折叠成 idle/cooldown/rate_limited）、`effect`（in-memory action_success/recovered/rollback_success 折叠成 stabilized/recovered/rolled_back/terminal）。action 有序阶梯**严格复刻引擎早返回链**：`reject_diagnosis` → `deny_terminal` → `escalate_human`(evidence/fault/permission fail) → `defer_cooldown` → `defer_rate_limit` → `escalate_human`(blast) → `execute_rolled_back` / `execute_terminal` / `execute_recovered` / `execute_stabilizing`。12 个离线样本（`ev_*` answer_field 驱动，零新 Rust handler）覆盖全部 9 个 label；单节点，全节点被分支覆盖门禁走到。dsl smoke 103/103、round-trip 103/103、ledger 68+35=103。
- **ole-eval port #11 `loan_dialogue` BDD 化（P50，适配性）**：贷款申请 slot-filling 对话 + 承保移植到 `dsl/ole_eval/loan_dialogue.json`（`bdd/workflows/ole_eval/loan_dialogue.feature` 为源，`bdd transpile` 生成）。5 节点 DFA：`awaiting_income` → `awaiting_debt` → `awaiting_amount` → `awaiting_term` → `complete`。模型只提 typed facts（`annual_income`/`monthly_debt`/`loan_amount`/`term_months`）；每轮必须恰好填期望槽位——skip-step / 多字段轮、越界 / 非法值、完成后再次提交，全部本地拒绝。`bdd build` **42/42 = 100%**；11 个离线样本覆盖全部 6 个终态 label（`APPROVE`/`REJECT`/`reject_duplicate`/`reject_value`/`reject_skip` + 中间槽位 label），9 个 multi-node 样本逐步钉 trace（`trace.step_count` + 每步 node/action/answer/next）。**折叠/文档化边界**：Decimal 精确摊销、DTI > 43.00 / payment-to-income > 28.00 阈值折算进终态 `outcome` choice；COMPLETE 后重复轮折叠为 `duplicate`。states 68→69 spec/240→251 state、expect 65→66 spec/231→242 state/60→69 multi-node、ledger 69+35=104；dsl smoke 104/104、round-trip 104/104、check.sh 全绿。
- **ole-eval port #12 `sla_checker` BDD 化（P51，适配性）**：确定性 UTC SLA 评估移植到 `dsl/ole_eval/sla_checker.json`（`bdd/workflows/ole_eval/sla_checker.feature` 为源，`bdd transpile` 生成）。单节点判定：模型只提 typed facts（`proposal` 解析态 / `rule` schema 态 / `event` schema 态 / `sla` 总体态 / `severity` 超限比档），schema 校验（rule_id 格式 / `P1..P99` / `response_minutes <= resolution_minutes` / 窗口非空不重叠 / 六键事件 / 仅 UTC `Z` 时间戳 / 时间戳先后）、冲突、重复（rule registry / event ledger）、SLO 状态与严重度全部由本地有序 rules 裁决。**折叠/文档化边界**：`add_business_minutes` / `business_minutes_between` 的 UTC 业务窗口分钟累计（next-opening / 日窗重叠 / 无 DST 算术）不进 DSL —— 由本地算出，仅以 `sla` + `severity` 两个 choice 呈现；JSON 解析细节（```json 围栏剥离、重复 JSON key 拒绝、恰 `{rule,event}` 形状）折叠进 `proposal` choice；`_MAX_SLO_MINUTES` / `_MAX_TIMELINE_DAYS` 边界折进同一 schema 桶。12 个 label 的有序阶梯：`reject_proposal` → `reject_rule` → `reject_rule_conflict`（conflict / overlap）→ `reject_duplicate_rule` → `reject_event`（bad_schema / bad_time / unknown_priority）→ `reject_duplicate_event` → `breach_critical` → `breach_high` → `breach_medium` → `breach_low` → `met` → `pending`（catch-all）。15 个离线样本（`ev_*` answer_field 驱动，零新 Rust handler）覆盖全部 12 个 label；单节点，全节点被分支覆盖门禁走到。`bdd build` **27/27 = 100%**；states 69→70 spec/251→266 state、expect 66→67 spec/242→257 state（multi-node 仍 69）、ledger 70+35=105；dsl smoke 105/105、round-trip 105/105、check.sh 全绿。
- **ole-eval port #13 `api_quota_guard` BDD 化（P52，适配性）**：确定性 API scope/quota/rate/concurrency/budget 闸门移植到 `dsl/ole_eval/api_quota_guard.json`（`bdd/workflows/ole_eval/api_quota_guard.feature` 为源，`bdd transpile` 生成）。单节点判定：模型只提 typed request intent（`principal` / `endpoint` / `requested_units` / `context_intent`）；schema 校验（枚举 principal/endpoint/context_intent、整数 units ∈ [1,1000]、无重复 JSON key、禁 authority/side-effect 字段）、request-id 重放台账、时钟单调性、scope 准入、per-endpoint 单请求上限、并发上限、令牌桶、日/月单位配额与花费预算全部由本地有序 rules 裁决。**折叠/文档化边界**：JSON 解析细节折进 `proposal` choice；令牌桶 refill 算术（`elapsed_ms * refill_units_per_second / 1000`，封顶容量）与其隐含的 `retry_after_ms` 是确定性 Decimal 时钟算术、不进 DSL（桶仅以 `bucket` = ok/insufficient 呈现）；`daily_reset_after_ms > 0` 的「重置提示 → throttle-with-retry vs 直接 reject」折进 `quota` 的 `daily_retry` / `daily_no_retry` 两个答案；per-principal/per-endpoint 固定策略表（scopes/limits/budgets/costs/capacities/refill）是本地 fixture 非模型输入。12 个 label 的有序阶梯：`reject_proposal` → `reject_replay` → `reject_clock_regression` → `reject_scope` → `reject_units_above_max` → `throttle_concurrency` → `throttle_bucket` → `throttle_daily` → `reject_daily_no_retry` → `reject_monthly` → `reject_spend` → `allow`（catch-all）。15 个离线样本（`ev_*` answer_field 驱动，零新 Rust handler）覆盖全部 12 个 label，含 3 例跨 label 优先级（proposal-first / replay-before-scope / quota-before-budget）；单节点，全节点被分支覆盖门禁走到。`bdd build` **35/35 = 100%**；states 70→71 spec/266→281 state、expect 67→68 spec/257→272 state、ledger 71+35=**106**；dsl smoke 106/106、round-trip 106/106、check.sh 全绿。
- **ole-eval port #14–#17 `cache_policy_enforcer` / `log_redaction_guard` / `dns_record_validator` / `session_token_validator` BDD 化（P53，适配性）**：四个同构「LLM 提案 → 确定性闸门」guard 移植到 `dsl/ole_eval/{cache_policy_enforcer,log_redaction_guard,dns_record_validator,session_token_validator}.json`（`bdd/workflows/ole_eval/<name>.feature` 为源，`bdd transpile` 生成）。四个都是单节点判定，共用同一阶梯骨架：**schema 校验 → 数量上限 → 黑名单 → 专项检查 → 审批升级 → 放行**。逐个：`cache_policy_enforcer`（routes 数上限 / blocked path patterns / max_age 上限 / require_approval；label reject_proposal→reject_too_many_routes→reject_blocked_pattern→reject_max_age→escalate_approval→approve）；`log_redaction_guard`（rules 数上限 / blocked actions / required patterns 缺失 / require_approval；reject_too_many_rules→reject_blocked_action→reject_missing_required→…）；`dns_record_validator`（records 数上限 / blocked types / blocked domains / ttl < min_ttl / require_approval；reject_too_many_records→reject_blocked_type→reject_blocked_domain→reject_ttl_below_min→…）；`session_token_validator`（**revoke 无条件放行**置于阶梯首位 → expiry 上限 → scope 数上限 → blocked scopes → 危险 metadata → require_approval；approve_revoke→reject_expiry→reject_scope_count→reject_blocked_scope→reject_metadata→…）。**折叠/文档化边界**：JSON 解析细节与 per-item 字段类型折进 `proposal` choice；固定 policy 表（max_*/blocked_*/required_*/min_ttl/require_approval）是本地 fixture 非模型输入，各确定性检查折成一个 verdict choice。`bdd build` 21/21、21/21、24/24、27/27 均 = 100%（各 8/8/9/10 离线样本，`ev_*` answer_field 驱动，零新 Rust handler，含跨 label 优先级样本）。states 71→75 spec/281→316 state、expect 68→72 spec/272→307 state、ledger 75+35=**110**；dsl smoke 110/110、round-trip 110/110、check.sh 全绿；dsl 侧回归 629/0。
- **ole-eval port #18–#21 `webhook_signature_verifier` / `endpoint_health_checker` / `file_upload_scanner` / `circuit_breaker_guard` BDD 化（P54，适配性）**：四个同构单节点 guard 移植到 `dsl/ole_eval/<name>.json`（`bdd/workflows/ole_eval/<name>.feature` 为源，`bdd transpile` 生成）。`webhook_signature_verifier`（require_https / blocked hash algo / tolerance 上限 / approval；reject_proposal→reject_insecure_url→reject_blocked_algo→reject_tolerance→escalate_approval→approve）；`endpoint_health_checker`（endpoints 数上限 / HTTPS / timeout 上限 / interval 下限 / blocked domains / approval）；`file_upload_scanner`（files 数上限 / total size / per-file size / blocked extensions / overwrite 升级 / approval）；`circuit_breaker_guard`（状态合法性 / 状态机合法迁移 / open 阈值 / half-open 时长 / close 错误率——Python 累积全部错误的语义在端口里以「首个失败检查」表达，**文档化**）。**折叠/文档化边界**：JSON 解析细节 + 固定 policy 表（max_*/min_*/blocked_*/require_*/require_https）折进各 choice。`bdd build` 21/21、27/27、27/27、24/24 = 100%（各 8/8/9/8 离线样本，`ev_*` answer_field 驱动，零新 Rust handler，含跨 label 优先级）。states 75→79 spec/316→350 state、expect 72→76 spec/307→341 state、ledger 79+35=**114**；dsl smoke 114/114、round-trip 114/114、check.sh 全绿；dsl 侧回归 667/0。
- **ole-eval port #22 `query_complexity_limiter` BDD 化（P55，适配性）**：单节点查询复杂度闸门移植到 `dsl/ole_eval/query_complexity_limiter.json`（`bdd/workflows/ole_eval/query_complexity_limiter.feature` 为源，`bdd transpile` 生成）。模型只提 typed query（request_id / query_type SELECT·INSERT·UPDATE·DELETE / tables / joins / subqueries / estimated_rows / query_text）；schema 校验 + blocked query_type + joins·subqueries·estimated_rows 上限 + blocked tables + 危险 query 文本 + approval，全部由本地有序 rules 裁决 → `reject_proposal`/`reject_blocked_query_type`/`reject_too_many_joins`/`reject_too_many_subqueries`/`reject_row_estimate`/`reject_blocked_table`/`reject_dangerous_text`/`escalate_approval`/`approve`。**折叠/文档化边界**：JSON 解析细节 + 固定 policy 表（max_joins / max_subqueries / max_estimated_rows / blocked_tables / blocked_query_types / require_approval）折进各 choice。`bdd build` 30/30 = 100%（11 个离线样本，`ev_*` answer_field 驱动，零新 Rust handler，含跨 label 优先级）。states 79→80 spec/350→361 state、expect 76→77 spec/341→352 state、ledger 80+35=**115**；dsl smoke 115/115、round-trip 115/115、check.sh 全绿；dsl 侧回归 679/0。
- **ole-eval port #23–#26 `rate_limit_budget_guard` / `rate_limiter_config_guard` / `network_bandwidth_allocation_guard` / `api_key_rotation_guard` BDD 化（P56，适配性）**：四个同构单节点 guard 移植到 `dsl/ole_eval/<name>.json`（`bdd/workflows/ole_eval/<name>.feature` 为源，`bdd transpile` 生成）。`rate_limit_budget_guard`（endpoint 预算表准入 / 每键 ratio 超限→REJECT·近限≥80%→ESCALATE）；`rate_limiter_config_guard`（算法枚举 / rate 上下界 / burst≤100×率·≤policy 上界 / endpoint `..` 路径）；`network_bandwidth_allocation_guard`（link catalog 准入 / 每链路总量 ≤ capacity−reserved）；`api_key_rotation_guard`（key_id 格式 / age 硬限 / 新键长度 / 提前轮换 / 扩展边界，**revoke 无条件放行**）。**折叠/文档化边界**：JSON 解析细节 + 固定 policy 表折进各 verdict choice；Python「累积全部错误」语义在端口里以「首个失败检查」（Python 源序：algorithm→rate→burst→endpoint；key_id→age→rotate→early→extend）**文档化**。`bdd build` 16/16 · 22/22 · 15/15 · 24/24 = 100%（各 6/8/5/8 离线样本，`ev_*` answer_field 驱动，零新 Rust handler，含跨 label 优先级）。states 80→84 spec/361→388 state、expect 77→81 spec/352→379 state、ledger 84+35=**119**；dsl smoke 119/119、round-trip 119/119、doc-check 9/9、check.sh 全绿；dsl 侧回归 710/0。
- **ole-eval port #27–#30 `credential_rotation_guard` / `config_schema_migration_guard` / `feature_flag_rollout_guard` / `cache_invalidation_guard` BDD 化（P57，适配性）**：四个同构单节点 guard 移植到 `dsl/ole_eval/<name>.json`（`bdd/workflows/ole_eval/<name>.feature` 为源，`bdd transpile` 生成）。`credential_rotation_guard`（overdue→ESCALATE 优先于 blocked→REJECT、critical-approval 规则、service 黑名单）；`config_schema_migration_guard`（changes 上限 / removals 上限 / critical field 保护 / removal default / 危险 token / approval）；`feature_flag_rollout_guard`（env allowlist / prod staging 证据 / high-risk canary / 单步 pct 上限——high-risk 超限 REJECT、否则 ESCALATE）；`cache_invalidation_guard`（keys 上限 / protected namespace→ESCALATE / ttl 下限 / reason 必填）。**折叠/文档化边界**：JSON 解析细节 + 固定 policy 表折进各 verdict choice；`ev_*` answer_field 驱动、零新 Rust handler。`bdd build` 18/18 · 27/27 · 22/22 · 21/21 = 100%（各 6/9/8/7 离线样本，含跨 label 优先级）。states 84→88 spec/388→418 state、expect 81→85 spec/379→409 state、ledger 88+35=**123**；dsl smoke 123/123、round-trip 123/123、doc-check 9/9、check.sh 全绿；dsl 侧回归 744/0。
- **ole-eval port #31–#34 `model_drift_detector` / `dead_letter_queue_guard` / `compliance_audit_evidence_guard` / `workflow_dag_validator` BDD 化（P58，适配性）**：四个同构单节点 guard 移植到 `dsl/ole_eval/<name>.json`（`bdd/workflows/ole_eval/<name>.feature` 为源，`bdd transpile` 生成）。`model_drift_detector`（accuracy 跌幅→REJECT、latency 增幅·throughput 跌幅→ESCALATE）；`dead_letter_queue_guard`（retry 上限 / payload 上限 / blocked queue / forward 大包→ESCALATE）；`compliance_audit_evidence_guard`（missing+pending→ESCALATE **优先于** REJECT、必需类目覆盖 / 零采集 / missing% 上限）；`workflow_dag_validator`（节点·边上限 / 无环 / 单 start / 有 end / blocked 节点类型 / 危险 token / approval）。**折叠/文档化边界**：JSON 解析细节 + 固定 policy 表折进各 verdict choice；`ev_*` answer_field 驱动、零新 Rust handler。`bdd build` 15/15 · 21/21 · 21/21 · 34/34 = 100%（各 5/7/7/12 离线样本，含跨 label 优先级）。states 88→92 spec/418→449 state、expect 85→89 spec/409→440 state、ledger 92+35=**127**；dsl smoke 127/127、round-trip 127/127、doc-check 9/9、check.sh 全绿；dsl 侧回归 779/0。
- **ole-eval port #35–#38 `incident_response_guard` / `shipping_route_guard` / `credential_issuance_guard` / `refactor_plan_guard` BDD 化（P59，适配性）**：四个同构单节点 guard 移植到 `dsl/ole_eval/<name>.json`（`bdd/workflows/ole_eval/<name>.feature` 为源，`bdd transpile` 生成）。**首批三值决策词**：`incident_response_guard` 与 `refactor_plan_guard` 保真引擎的 APPROVE / REVISE / REJECT（`reject_*` 硬门 → `revise_*` 软门 → `approve`），`credential_issuance_guard` 用引擎自己的 ISSUE / MODIFY / REJECT（`reject_*` → `modify_*` → `issue`）。`incident_response_guard`（incident id/语义匹配 / 幂等指纹重放 / runbook allowlist / 目标合法性与 affected-service 范围 / RESOLVED 终态 / 未知状态 → REJECT；未先 page oncall / 审批法定人数缺失 / rollback·failover·scale·close 前置条件 / mitigation SLA 已破 → REVISE）；`shipping_route_guard`（物流图求解折叠成 choice：无可行路线 / 成本超限 → REJECT，否则 APPROVE）；`credential_issuance_guard`（未知 subject·kind / 主体类型·audience / IAL·proofing / MFA·phishing / 可信设备·设备绑定 / 风险过高 / 主体·凭据非 active / 轮换过早 → REJECT；隐式轮换 / 设备绑定必填 / 中等风险降寿命·缩 scope / recover 步进升级 → MODIFY）；`refactor_plan_guard`（重复 request / 未知 operation / intent target·objective 越界 / 重复 step id / 依赖环 / 未知依赖 / 缺必需 action / 11 类 per-step payload·scope·order / 原子性 checkpoint / rollback 策略·checkpoint·顺序 / 行为等价性 → REJECT；可选行为测试缺失 / 风险升·降级 → REVISE）。**折叠/文档化边界**：JSON 解析细节 + 固定 registry/policy 表折进各 choice；`credential_issuance_guard` 的 preview 复检（proofing/IAL）在同输入下可证为空 → 折进 `proofing`/`ial` 而不新增 label；`refactor_plan_guard` 的 11 条 per-step 规则用一个 `step_shape` 问题、每规则一个 answer/label 承载。`bdd build` 45/45 · 15/15 · 69/69 · 68/68 = 100%（各 19/6/27/34 离线样本，`ev_*` answer_field 驱动、零新 Rust handler，含跨 label 优先级）。states 92→96 spec/449→535 state、expect 89→93 spec/440→526 state、ledger 96+35=**131**；dsl smoke 131/131、round-trip 131/131、doc-check 9/9、check.sh 全绿；dsl 侧回归 869/0。
- **ole-eval port #39–#42 `data_retention_expiry_guard` / `subscription_churn_retention_guard` / `pricing_discount_guard` / `inventory_replenishment_guard` BDD 化（P60，适配性）**：四个同构单节点 guard 移植到 `dsl/ole_eval/<name>.json`（`bdd/workflows/ole_eval/<name>.feature` 为源，`bdd transpile` 生成）。`data_retention_expiry_guard`（记录 retention_days 超 policy.max → REJECT、未过期且属 requires-approval 类目 → ESCALATE，过期只报告不阻断）；`subscription_churn_retention_guard`（低风险<0.3 且折扣>20% → ESCALATE **优先于** 违规 REJECT；折扣 / 时长超客户档位上限、档位越级 → REJECT；parse 已拒 discount<=0 故该违规不可达 → 折进 `proposal`）；`pricing_discount_guard`（**三值 APPROVE/MODIFY/REJECT**：重复 request / segment 不合格 / 审批阈值封顶后破毛利底线 → REJECT；促销窗口·量级·优惠券·叠加·毛利底线·审批封顶等 8 类 adjustment → MODIFY，每类一个 label 由单个 `adjustment` 问题承载，按引擎 append 序输出；无 adjustment → APPROVE）；`inventory_replenishment_guard`（**三值 APPROVE/MODIFY/REJECT**：未知 sku / 重复提案 / reorder 未达再订点 / 约束致不可行 / 数量非正 → REJECT；hold·review 已在再订点之上 → APPROVE（不下单）；intent 提升·服务水平回退·凑整到 pack·最小起订量·硬约束封顶等 5 类 adjustment → MODIFY，动态 `capped_by_<breach>` 折成单个 `capped_by_constraint`）。**折叠/文档化边界**：JSON 解析细节 + 固定 catalog/coupon/promotion/item/budget 表折进各 choice；`ev_*` answer_field 驱动、零新 Rust handler。`bdd build` 15/15 · 21/21 · 28/28 · 34/34 = 100%（各 6/9/17/19 离线样本，含跨 label 优先级）。states 96→100 spec/535→586 state、expect 93→97 spec/526→577 state、ledger 100+35=**135**；dsl smoke 135/135、round-trip 135/135、doc-check 9/9、check.sh EXIT=0；dsl 侧回归 926/0。
- **ole-eval port #43 `privacy_retention_guard` BDD 化（P61，适配性轴）**：把 `code/qwen_vm/app_scenarios/privacy_retention_guard/` 的保留/保全/删除决策闸门移植成单节点 `threshold` 决策图：模型只提 typed proposal（`data_class` / `retention_intent` / `deletion_intent`，保真引擎自己的 APPROVE / MODIFY / REJECT 三值）；记录台账（已评估记录 id）、authoritative 记录类、jurisdiction 注册、data-class policy 表（retention days · retention clock · allowed lawful bases · pseudonymization eligibility）与 jurisdiction policy 表（deletion grace · maximum extension · pseudonymization permission）由本地引擎解析，retention 时钟/截止、lawful-basis 检查、legal hold、pseudonymization 资格与删除/保全优先级全部本地裁决，精确算出 scheduled action 与其时间戳。**折叠/文档化边界**：JSON 解析细节（4 KiB 上限 / 精确 `{proposal:{data_class,retention_intent,deletion_intent}}` 形状 / 枚举 / 记录 id 语法 / 重复键扫描 / 非有限数拒绝 / 禁止 authority·副作用字段）折进 `proposal`；记录台账 / 记录类 / jurisdiction 与整条 gate 之后的优先级链折成 `record`/`data_class`/`jurisdiction`/`outcome` 四个 choice（`outcome` 一个问题承载余下 13 个引擎分支，每分支一个 label，按引擎首匹配序出结论，末支 `modify_early_deletion_not_permitted` 复刻引擎自己的 final else）；`ev_*` answer_field 驱动、零新 Rust handler。`bdd build` 32/32 = 100%；21 离线样本（17 label 全覆盖 + 4 跨 label 优先级）。states 100→101 spec/586→607 state、expect 97→98 spec/577→598 state、ledger 101+35=**136**；dsl smoke 136/136、round-trip 136/136、doc-check 9/9、check.sh EXIT=0；dsl 侧回归 948/0。
- **ole-eval port #44–#46 `shift_assignment_guard` / `settlement_allocation_guard` / `subscription_entitlement_guard` BDD 化（P62，适配性轴）**：三个「LLM 提案 → 确定性求解/分配闸门」单节点 `threshold` 图（保真引擎各自的三值：shift = FEASIBLE/REVISE/INFEASIBLE(+REJECTED)，settlement/subscription = APPROVE/MODIFY/REJECT(+REJECTED)）。`shift_assignment_guard`（排班求解器：worker 技能·可用性·工时·休息·连续班次上限·role 覆盖·fairness·有界搜索；INFEASIBLE 首因 DUPLICATE_REQUEST_ID>UNKNOWN_PLAN>SEARCH_COMBINATION_LIMIT_EXCEEDED>NO_FEASIBLE_ROLE_COVERING_ASSIGNMENT，REVISE 首因取引擎 append 序：UNKNOWN_WORKER/UNKNOWN_SHIFT/UNQUALIFIED/UNAVAILABLE/FAIRNESS_ASSIGNMENT_ADJUSTED/AVOID_PREFERENCE_OVERRIDDEN_BY_COVERAGE）；`settlement_allocation_guard`（结算分配：case 台账·canonical order·tax/credit/waterfall·rounding·ledger+cash-control 校验·allocation caps → REJECT 首因 DUPLICATE/UNKNOWN_SETTLEMENT_CASE/UNKNOWN_INVOICE_PREFERENCE/RANK_OUT_OF_RANGE/INCOMPLETE_PREFERENCES/LEDGER_NOT_BALANCED/CASH_CONTROL_TOTAL_INVALID/ALLOCATION_EXCEEDS_CAP；MODIFY 首因 PRIORITY_RATIONALE_MODIFIED/ALLOCATION_ORDER_MODIFIED）；`subscription_entitlement_guard`（订阅权益变更：subscription 台账·plan/add-on 目录·term/billing-cycle 窗口·add-on plan 依赖·seat policy·commitment·proration+credit preview·control-total → REJECT 首因 10 类，MODIFY 首因 DOWNGRADE_DEFERRED_TO_TERM_END/SEAT_REDUCTION_CLAMPED_TO_COMMITMENT/MISSING_ADD_ON_DISABLE_IGNORED/INCOMPATIBLE_ADD_ON_REMOVED）。**折叠/文档化边界**：JSON 解析细节（balanced-JSON 提取 / 重复键 / 精确信封形状 / 标识符·日期语法 / action·rationale 白名单 / item 上限与唯一对 / 危险 token 扫描）折进 `proposal`；固定 fixture 表（plan·worker·case·add-on 目录与求解上界）由 harness 解析；台账·求解·分配·预览与优先级链折成 `infeasible`/`reject_reason`/`revision` 各一问题承载引擎首因（引擎可能 append 多因，取首因并按引擎 REJECT/INFEASIBLE→REVISE/MODIFY 优先级出结论）；`ev_*` answer_field 驱动、零新 Rust handler。`bdd build` 23/23 · 23/23 · 27/27 = 100%（各 15/15/19 离线样本：label 全覆盖 + 跨 label 优先级）。states 101→104 spec/607→656 state、expect 98→101 spec/598→647 state、ledger 104+35=**139**；dsl smoke 139/139、round-trip 139/139、doc-check 9/9、check.sh EXIT=0；dsl 侧回归 1000/0。
- **ole-eval port #47–#48 `contract_conflict_guard` / `accessibility_content_guard` BDD 化（P63，适配性轴）**：两个「LLM 提案 → 确定性校验/审查」单节点 `threshold` 图，各保真引擎自己的多值：`contract_conflict_guard` = CONFLICT/NO_CONFLICT/REVIEW（固定合同 fixture：本地 clause id·区间重叠·modal/deadline 冲突·优先级覆盖·exception 引用；REVIEW 两类 + CONFLICT 首因两类 + no_conflict）；`accessibility_content_guard` = ACCEPT/REVISE/REJECT（不可变内容快照上的确定性 scoped checklist：意图·目标匹配·remediation allow-list·未解决项；reject 首因 5 类 + revise 首因 2 类 + approve）。**折叠/文档化边界**：解析细节（精确信封形状 / 枚举与标识符·日期语法 / reason 长度与 unsafe-text 扫描）折进 `proposal`；固定 fixture（合同条款表 / 内容快照与 issue 列表）由 harness 解析；早期返回式校验折成 `exception`/`reject_reason`/`revision`/`conflict` 各一问题承载引擎首因，按引擎 REVIEW/REJECT→CONFLICT/REVISE→NO_CONFLICT/ACCEPT 顺序出结论；`ev_*` answer_field 驱动、零新 Rust handler。`bdd build` 16/16 · 20/20 = 100%；8/12 离线样本（label 全覆盖 + 跨 label 优先级）。states 104→106 spec/656→676 state、expect 101→103 spec/647→667 state、ledger 106+35=**141**；dsl smoke 141/141、round-trip 141/141、doc-check 9/9、check.sh EXIT=0；dsl 侧回归 1022/0。
- **ole-eval port #49–#52 `experiment_design_gate` / `crawler_schema_guard` / `rpa_action_guard` / `knowledge_graph_guard` BDD 化（P64，适配性轴）**：四个「LLM 提案 → 确定性 guard」单节点三值 `threshold` 图：实验设计 APPROVE/MODIFY/REJECT、爬虫 schema ACCEPT/REPAIR/REJECT、RPA 动作 ALLOW/REVIEW/DENY、知识图谱 ACCEPT/REVISE/REJECT。解析折进 `proposal`、固定 fixture 由 harness 解析、引擎早期返回式检查链折成 `reject_reason`/`deny_reason`/`authority`+`design` 承载引擎首因；`bdd build` 17/17·26/26·33/33·32/32 = 100%；9/17/24/24 离线样本（label 全覆盖 + 跨 label 优先级）。states 106→110 spec/676→750 state、expect 103→107 spec/667→741 state、ledger 110+35=**145**；dsl smoke 145/145、round-trip 145/145、doc-check 9/9、check.sh EXIT=0；dsl 侧回归 1100/0。
- **ole-eval port #53–#56 `data_contract_schema_guard` / `resource_scheduler_guard` / `policy_as_code` / `test_case_generator` BDD 化（P65，适配性轴）**：四个「LLM 提案 → 确定性 guard」单节点三值 `threshold` 图：数据契约 ACCEPT/REVISE/REJECT（14 reject 首因 + 2 确定性 default-backfill revise 首因）、资源调度 APPROVE/MODIFY/REJECT（顺序 preference 域检查 + 有界搜索拒绝 + 不可行 + 可行但覆盖 soft preference）、policy-as-code ALLOW/DENY/REVIEW（编译 / 版本原子激活·回滚 / 首优先级规则匹配，默认 DENY「no matching policy rule」当 catch-all）、测试用例 ACCEPTED/REPAIRED/REJECTED（4 reject 首因 + inputs/intent/oracle 确定性修复）。多字段聚合 guard（数据契约）折成单 `reject_reason`/`revision` 承载引擎首因；policy-as-code 的 `effect` 阶梯以 `deny otherwise` 复刻引擎默认 DENY。解析折进 `proposal`、固定 fixture 由 harness 解析、`ev_*` answer_field 驱动、零新 Rust handler。`bdd build` 29/29·20/20·19/19·22/22 = 100%；20/11/9/13 离线样本（label 全覆盖 + 跨 label 优先级）。states 110→114 spec/750→803 state、expect 107→111 spec/741→794 state、ledger 114+35=**149**；dsl smoke 149/149、round-trip 149/149、doc-check 9/9、check.sh EXIT=0；dsl 侧回归 1157/0（bdd-smoke-expect-coverage 850→907）。
- **ole-eval port #57–#60 `bracket_matcher` / `indentation_guard` / `keyword_order_guard` / `protocol_parser` BDD 化（P66，正确性轴）**：四个「LLM 提案 → 确定性语法/协议 guard」单节点 `threshold` 图：括号匹配 accepted/rejected（CPython tokenizer + 手工 fallback：lexical_error / depth_exceeded / extra_close / wrong_order / unclosed 首因）、缩进 raw_accepted/repaired_accepted/rejected（缩进栈 + 确定性缩进修复 + ast.parse：mixed_tab_space / non_unit_width / leading_indented_statement / unexpected_indent / unindent_no_level / python_indent_error / python_syntax_error 首因 + repair_rescaled/repair_realigned）、关键字顺序 ok/not-ok（tokenize + 块 DFA + ast.parse：clause_order / import_order / not_tokenizable / python_syntax 首因）、协议解析 accepted/error(protocol)（JSON/SQL 提取 + 规范化：reject_json / reject_sql / reject_unsupported_protocol）。提取/解析细节折进 `proposal`，固定 fixture（深度上限 32、四空格缩进、DFA 相位、SQL 语法白名单）由 harness 解析、`ev_*` answer_field 驱动、零新 Rust handler。`bdd build` 16/16·22/22·15/15·14/14 = 100%；8/13/7/7 离线样本（label 全覆盖 + 跨 label 优先级）；states 114→118 spec/803→838 state、expect 111→115 spec/794→829 state、ledger 118+35=**153**；dsl smoke 153/153、round-trip 153/153、doc-check 9/9、check.sh EXIT=0；dsl 侧回归 1196/0（bdd-smoke-expect-coverage 907→946）。
- **ole-eval port #61 `verifiable_cot` BDD 化（P67，正确性轴）**：把 `verifiable_cot`（可验证 CoT 的确定性 trace VM）移植成三节点 DFA `proposal → contract → execution`（`bdd/workflows/ole_eval/verifiable_cot.feature` 为源）。模型只提 typed steps（`{"trace":[...]}`，opcode 白名单 load/const/arith/compare/branch/assert）；解析（信封形状 + 逐步 schema + 32 步上限）、控制流契约（重复 step id / 未定义跳转目标 / 环 / 不可达步 / input 契约）、确定性执行（未定义 input·变量 / 类型错误 / 除零 / 数值越界 / assert 前终止 / 步数上限 / assert 冲突 / PASS）三层各折成一个 choice，16 个 label 由引擎首因出结论；proposal 失败不达 contract，contract 失败不达 execution（跨节点优先级靠 DFA 的 `When` 路由强制，已测 contract≻execution）。**折叠/文档化边界**：JSON 恢复 / 信封形状 / 逐步 schema / 算子白名单 / 数值界折进 `proposal`；确定性 VM（parse → 契约校验 → typed 执行）由 harness 跑并交给 DSL 每检查一个答案；`ev_*` answer_field 驱动、零新 Rust handler。`bdd build` 32/32 = 100%；16 离线样本（15 个终局 label 全覆盖 + 跨节点优先级；`contract_ok` 是 contract 节点的中途 label，随路由被 execution 覆盖，不作终局断言）；states 118→119 spec/838→854 state、expect 115→116 spec/829→845 state（trace-pinned stems 22→23）、ledger 119+35=**154**；dsl smoke 154/154、round-trip 154/154、doc-check 9/9、check.sh EXIT=0；dsl 侧回归 1243/0（bdd-smoke-expect-coverage 946→963）。
- **ole-eval port #62 `formal_checker` BDD 化（P68，正确性轴）**：把有界 Thompson-NFA→DFA 形式化 checker 移植成两节点 DFA `proposal → verify`（`bdd/workflows/ole_eval/formal_checker.feature` 为源）。模型只提 fixture 对象（`{regex, accepted, rejected}`，≤12 候选）；`proposal` 折叠 balanced-JSON / fenced 提取 + fixture schema（恰好三键、pattern 为字符串、两个候选串数组、≤12 候选）；`verify` 承载 checker 权威（有界正则子集：锚点仅两端、码点字符类、预定义转义、算子集、深度/长度上限 → Thompson NFA → 惰性子集构造 DFA → 逐候选匹配）。15 个 label 由引擎首因出结论：parse 失败 → `reject_proposal`；12 类正则错误 → `reject_regex_too_long`/`reject_nesting_exceeded`/`reject_unexpected_char`/`reject_anchor_position`/`reject_group`/`reject_charclass`/`reject_quantifier`/`reject_escape`/`reject_nfa_limit`/`reject_dfa_limit`/`reject_input_too_long`/`reject_transition_limit`；无错误但分类与声明不符 → `reject_mismatch`；全部候选按声明分类 → `accept`。proposal 失败不达 verify（跨节点优先级，已测）。**折叠/文档化边界**：正则错误与「自不一致」折进 verify 路由的同一个 `check` 答案（`mismatch` = 无错误但错；错误码 = 引擎首个 raise 原因）；错误码取引擎首个 raise 的原因，故 reject 阶梯复刻引擎的 raise 顺序（先解析期语法/上限，再 NFA 上限，再惰性 DFA 上限，再逐 input 长度/转移上限）；`ev_*` answer_field 驱动、**零新 Rust handler**。`bdd build` 26/26 = 100%；16 离线样本（15 个终局 label 全覆盖 + 跨节点优先级；`proposal_ok` 为 proposal 节点中途 label）；states 119→120 spec/854→870 state、expect 116→117 spec/845→861 state（trace-pinned stems 23→24）、ledger 120+35=**155**；dsl smoke 155/155、round-trip 155/155、doc-check 9/9、check.sh EXIT=0；dsl 侧回归 1288/0（bdd-smoke-expect-coverage 963→980）。
- **ole-eval port #63 `reasoning_audit` BDD 化（P69，正确性轴）**：把不可变推理轨迹 + 确定性审计重放移植成三节点 DFA `proposal → build → replay`（`bdd/workflows/ole_eval/reasoning_audit.feature` 为源）。模型只提轨迹（`request_id`/`claimed_result`/`steps`），永不创建权威；引擎执行微型 opcode 集、逐步 SHA-256 哈希成链、追加到 append-only 台账、并重放以检测篡改/缺步/乱序/重复 request_id/结论不一致。`proposal` 折叠保守 JSON 恢复（fenced 或首个 balanced 对象）+ 提案 schema（恰好 request_id/claimed_result/steps 三键）；`build` 折叠 request_id 校验、候选步 schema、确定性 opcode 执行、台账重复拒绝；`replay` 折叠重执行、序列/哈希链校验、claim/conclusion 校验、trusted-root 校验。30 个 label 由引擎首因出结论：parse 失败 → `reject_proposal`；7 类 build 首因 → `reject_request_id`/`reject_step_count`/`reject_step_schema`/`reject_opcode`/`reject_operands`/`reject_execution`/`reject_duplicate_request_id`；21 类重放违规 → `reject_trace_not_object`/`reject_unexpected_trace_field`/`reject_unsupported_schema_version`/`reject_invalid_request_id`/`reject_invalid_previous_hash`/`reject_invalid_step_count`/`reject_invalid_step_schema`/`reject_sequence_mismatch`/`reject_missing_or_out_of_order_step`/`reject_request_id_mismatch`/`reject_hash_chain_break`/`reject_opcode_replay_failed`/`reject_resolved_operand_mismatch`/`reject_replayed_result_mismatch`/`reject_entry_hash_mismatch`/`reject_root_hash_mismatch`/`reject_final_result_mismatch`/`reject_conclusion_mismatch`/`reject_conclusion_inconsistent`/`reject_trusted_root_mismatch`/`reject_ledger_hash_chain_break`；全部通过 → `accept`。proposal 失败不达 build、build 失败不达 replay（跨节点优先级，均已测）。**折叠/文档化边界**：`proposal` 折叠 JSON 恢复 + schema；opcode 集/操作数规则/数值安全上限/64 步上限/canonical-JSON SHA-256 链/append-only 台账由 harness 跑并交给 DSL 每检查一个答案；引擎首个 raise 的原因决定 build 阶梯，有序去重的 `violations` 元组取首个决定 replay 阶梯；`ev_*` answer_field 驱动、**零新 Rust handler**。`bdd build` 48/48 = 100%；33 离线样本（30 个终局 label 全覆盖 + 跨节点优先级 + 防御性 catch-all）；states 120→121 spec/870→903 state、expect 117→118 spec/861→894 state（trace-pinned stems 24→25）、ledger 121+35=**156**；dsl smoke 156/156、round-trip 156/156、doc-check 9/9、check.sh EXIT=0；dsl 侧回归 1384/0（bdd-smoke-expect-coverage 980→1014）。
- **ole-eval port #64 `code_review_gate` BDD 化（P70，正确性轴）**：把 diff/token/AST 确定性的代码评审闸门移植成两节点 DFA `proposal → review`（`bdd/workflows/ole_eval/code_review_gate.feature` 为源）。模型只提 typed issue（`issue_type`/`file`/`hunk_id`/`line`/`reason`/`suggestion`）；diff/hunk 存在性、token/AST 证据、issue 有效性、重复、test impact、严重度与最终 ACCEPT/REVISE/REJECT 全由固定 fixture 本地裁决，闸门从不读仓库、不跑代码。`proposal` 折叠 balanced-JSON/fenced 提取 + 提案 schema（恰好 `proposal` 一键 → 恰好 `issues` 一键 → ≤12 条 typed issue，每条恰好六键，`issue_type` ∈ 七枚举、`file`≤160、`hunk_id`≤32、`line` 1..10000、`reason`/`suggestion` 各一行 8..240）；`review` 折叠逐 issue 的有序早返回链（file-in-diff→hunk-exists→line-in-hunk→line-changed→duplicate→local-evidence）与终局裁决（对 accepted ∪ locally-missed 的权威 issue 集算严重度：CRITICAL→REJECT、HIGH/MEDIUM→REVISE、否则 ACCEPT）。10 个 label：parse 失败 → `reject_proposal`；终局 `reject`/`revise`/`accept`；被拒 issue 在改动仍被采纳时以首因出 `accept_issue_file_not_in_diff`/`accept_issue_hunk_not_found`/`accept_issue_line_not_in_hunk`/`accept_issue_line_not_changed`/`accept_issue_duplicate_issue`/`accept_issue_no_local_evidence`。proposal 失败不达 review（已测）；decision 主导于 gate（已测）。**折叠/文档化边界**：固定 diff fixture、token/AST 证据、issue 本地扫描、test-impact 目录、严重度表由 harness 跑并交给 DSL 每检查一个答案；被拒 issue 不改变终局裁决，故折成 `gate` 首因；`line_not_changed` 分支随公开 fixture 无 context 行而声明但未行使。`ev_*` answer_field 驱动、**零新 Rust handler**。`bdd build` 25/25 = 100%；17 离线样本（5 个 parse 失败 + 终局 3 label + 5 个 `accept_issue_*` + decision≻gate 优先级）；states 121→122 spec/903→920 state、expect 118→119 spec/894→911 state（trace-pinned stems 25→26）、ledger 122+35=**157**；dsl smoke 157/157、round-trip 157/157、doc-check 9/9、check.sh EXIT=0；dsl 侧回归 1426/0（bdd-smoke-expect-coverage 1014→1032、trace-path-floor 265→289）。
- **ole-eval port #65 `code_self_repair` BDD 化（P71，正确性轴）**：把「LLM 提案补丁 → 确定性修复闸门」移植成单节点 `guard` 决策图（`bdd/workflows/ole_eval/code_self_repair.feature` 为源）。模型只提 typed 补丁提案（balanced-object/fenced 提取后恰好 `target`+`patches` 两键；`target` ∈ 六个白名单文件；`patches` 为非空片段列表，每片段恰好 `old`/`new` 两键字符串；输出 ≤4096 字符、片段 ≤2048 字符、每片段 ≤24 新行、单束 ≤48 新行）；补丁应用的精确匹配、CPython 词法、Python AST、静态安全、固定单元测试五道有序闸门全由固定 fixture 本地裁决，闸门从不读仓库、不跑代码。8 个 label：parse/schema 失败 → `reject_proposal`；逐 gate 首因 `reject_bounded_patch`/`reject_exact_match`/`reject_tokenize`/`reject_ast`/`reject_malicious_pattern`/`reject_unit_tests`；全过 → `accept`。**折叠/文档化边界**：`validate_boundaries` 的 assigned-path 与 patch-size 检查在 parser 期已拒（未指派 target / 超尺寸 → malformed），parser 之后永不触发，故折进 `proposal`，`gate` 仅剩 6 个可达值 + `none`；`MAX_RETRIES` 的 re-propose 循环属于 agent 不属于确定性 gate，故单节点。`ev_*` answer_field 驱动、**零新 Rust handler**。`bdd build` 17/17 = 100%；15 离线样本（6 个 parse/schema 失败 + 7 个 gate 首因样本，含 `bounded_patch` 两例 + 2 个 `accept`）——9 个 accepted-path 样本的 gate/label 由真跑 `repair.py::evaluate_proposal` 得到、6 个 malformed 由 `parse_patch_proposal` 抛 `PatchProposalError` 得到；states 122→123 spec/920→935 state、expect 119→120 spec/911→926 state（trace-pinned stems 26 不变，纯字符串 label）、ledger 123+35=**158**；dsl smoke 158/158、round-trip 158/158、doc-check 9/9、check.sh EXIT=0；dsl 侧回归 1442/0（bdd-smoke-expect-coverage 1032→1048）。
- **ole-eval port #66 `literature_synthesis_guard` BDD 化（P72，正确性轴）**：把「LLM 提案综述 → 确定性 claim/evidence 闸门」移植成单节点 `guard`（`bdd/workflows/ole_eval/literature_synthesis_guard.feature` 为源）。模型只提 typed 综述提案（balanced-object/fenced 提取 ≤48KB、拒重复键、危险 token 扫描、恰好 request_id/review_id/theme/claims 四键、标识符/枚举校验、1..8 条 claim、每条恰好 claim_id/claim_text/proposed_strength/evidence 四键、1..16 条 evidence、每条恰好 paper_id/relation/quote 三键、paper_id 格式 `p\d{4}-\d{3}`、relation ∈ {supports,refutes,limits}、quote 8..512 字符）；固定 citation graph + abstract 语料、主题 scope 与 core-paper 集、coverage 与 computed strength 全由 harness 本地裁决，闸门从不检索/下载/打开论文。18 label：parse/schema 失败 → `reject_proposal`；15 个逐阶段首因 `reject_duplicate_request_id`/`reject_unknown_theme`/`reject_theme_scope_mismatch`/`reject_duplicate_claim_id`/`reject_duplicate_claim_text`/`reject_paper_id_not_found`/`reject_paper_outside_theme`/`reject_evidence_relation_mismatch`/`reject_evidence_not_in_abstract_corpus`/`reject_fabricated_citation`/`reject_contradictory_evidence_for_paper`/`reject_contradictory_evidence`/`reject_citation_graph_disconnected`/`reject_core_coverage_below_threshold`/`reject_no_supporting_evidence`；4 个仅可修复原因（重复证据 / 支撑过薄 / 强度高估 / 强度低估）→ `revise`；全过 → `accept`。**折叠/文档化边界**：引擎的线性 stage machine （START→PROPOSAL_PARSED→THEME_CHECKED→CLAIMS_CHECKED→EVIDENCE_CHECKED→GRAPH_CHECKED→COVERAGE_CHECKED→STRENGTH_CHECKED→DECIDED）无分支，故单节点；in-memory request ledger（DUPLICATE_REQUEST_ID 重放闸）不在 DSL 内复刻，由 harness 拥有。`ev_*` answer_field 驱动、**零新 Rust handler**。`bdd build` 30/30 = 100%；27 离线样本（7 个 parse/schema 失败 + 19 个 accepted-path 逐 gate 首因 + `accept`），每条的 gate/label 均由真跑 `literature_synthesis_guard.py::parse_proposal` + `ValidatorSession.evaluate` 得到、并断言观察到的首因 == 预期首因。states 123→124 spec/935→962 state、expect 120→121 spec/926→953 state（trace-pinned stems 26 不变，纯字符串 label）、ledger 124+35=**159**；dsl smoke 159/159、round-trip 159/159、doc-check 9/9、check.sh EXIT=0；dsl 侧回归 1470/0（bdd-smoke-expect-coverage 1048→1076）。**至此 `app_scenarios/` 已知 66 个场景 66/66 全部 BDD 化（66 feature ↔ 66 spec 双向零孤儿）。**
- **self_evolving port `agent_permission_gate` BDD 化（P79，正确性轴）**：把 ole-eval `code/qwen_vm/self_evolving/workflows/agent_permission.py`（Python 权限状态机 + RiskModel 校准门禁）移植成 5 节点 `@decision` 图（`bdd/workflows/guards/agent_permission_gate.feature` 为源）。参考的 状态机 `unauthenticated→authenticated→authorized`（`blocked` 为死锁）、事件 `login/authorize/request/violation/logout`、布尔 `allowed`，与 `PermissionGate` 用 `RiskModel` 校准概率给 `BLOCK/ESCALATE/EXECUTE` 合并成：每节点两问 —— `event`（choice，含 `halt` = 参考的「trace 结束」）与 `risk`（`low`/`uncertain/`high` 三档），两者都由 `answer_field` 离线驱动；判决 `risk=high→BLOCK` / `uncertain→ESCALATE` / 否则 `EXECUTE`，而状态机拒绝（`refused`）优先级高于风险。**折叠/文档化边界**：`RiskModel` 的连续校准概率折成 3 档 typed choice；「首因」按 Python 源序；`blocked` 死锁语义 = 全事件→`refused`（故 `blocked` 节点必须保留 真实入边 `violation`，否则 `validate` 报 unreachable/dead code）。`ev_*` answer_field 驱动、**零新 Rust handler**。`bdd build` 56/56 = 100%；5 离线样本（`auth_execute`→EXECUTE / `violation_blocked`→BLOCK / `refused_request`→BLOCK （SM 拒绝优先）/ `escalate_uncertain`→ESCALATE / `block_high`→BLOCK）。states 124→125 spec/962→967 state、expect 121→122 spec/953→958 state、ledger 125+35=**160**；dsl smoke 160/160、round-trip 160/160、import-check 160/160、doc-check 9/9、check.sh EXIT=0。
- **内建 app ↔ BDD 端口行为等价 + 决策词汇表补 `with at most`（P82，正确性/表达力轴）**：用真实执行证据审计 `src/apps.rs` 的两个内建 app 与它们的 BDD 端口 —— `agent_gate`↔`dsl/guards/agent_command_gate.json`、`content_moderation`↔`dsl/guards/content_policy_gate.json`。① `agent_command_gate` 原为单问 `gate`（只看 `is_destructive`），与 app 的 5 阶梯 `route()` **不等价**：8 个参考用例里 `cat /etc/passwd` 应 CONFIRM、`curl -d @s.txt` 应 BLOCK，端口却给 ALLOW；改成多信号 `threshold` 阶梯（is_destructive/is_exfiltration/intent_match/risk），8/8 与 app 一致。② `content_policy_gate` 原有 4 条 `when.answer`（hateful/threatening/leaky_pii/spam）BLOCK 规则是**死规则** —— 运行时按 action question（`severity`）比较，永不命中；且缺 app 的「`category != safe` → WARN」。改成 概率规则在前（BLOCK）→ severity/category（WARN）→ APPROVE，与 app 逐点相等。③ 决策词汇表新增 `label "L" when the answer to "Q" is "B" with at most N confidence` → `{question,option,lt}`（此前只有 `at least`/`gte`，**无法表达「某选项概率低于阈值」**，正是复刻 `intent_safe < 0.5/0.7` 分支所需）：`dec_then_table` 加正则+tag、codegen 加 `label_opt_lt` 臂、`imp_rule` 让 `option` 分支接受 `gte` xor `lt`（`lt` 需显式 question）、`decision_templates` 加模板。**折叠/文档化边界**：app 的 `exfil verb ∧ exfil_p>=0.6` 前置守卫依赖命令行*动词*，BDD 无 pre-model 输入槽 → 折进 exfiltration 概率（高概率单独即 BLOCK，fail-closed 方向）。Rust 回归 `bdd-app-port-parity` 钉死：content 1458 verdicts 逐点相等；agent 300 verdicts 满足「端口永不比 app 少 BLOCK / 不在 app 允许处 ALLOW」，且端口多出的 BLOCK **恰好**落在折叠带 `exfil>=0.6`；两 app 的 8+6 参考用例经离线 heuristic 端到端一致。新增 2 个离线样本（agent_command_gate `exfil`→BLOCK / `sensitive_read`→CONFIRM）。states 125 spec/967→**969** state、expect 122 spec/958→**960** state、ledger 125+35=**160**；`bdd build` 100%、round-trip 160/160、import-check 160/160、dsl smoke 160/160、doc-check 9/9、check.sh EXIT=0；全量 `laya-workflow-tests` **3016/0**（109→110 sections）。



- **offline-detection（P29）**：网络型 stem（web_search/web_fetch/agent/rpc/http/webhook…）在本地 mocks 全不可达时 state 判 skip（validate-only），不误报回归；mocks up 时（`bench/mock_services.sh start` + 8791/8792 `mock serve`）全量强断言。
- **offline-external skip（P33，效率轴）**：离线 smoke **绝不对外网做真实 I/O**。`plugin` 型 capability 没有 `endpoint`/`url` 字段，改从插件脚本静态扫描 `http_get` 调用 + URL 字面量（去注释、识别字符串）判定网络依赖；spec 的网络目标只要含外部 host（非 loopback/RFC1918/link-local/CGNAT）就在离线 smoke 里整 state skip（validate-only，集成/live 任务真跑）。插件 `http_get` 在离线 smoke 里再叠一层 1.5s 超时上限兜底动态 URL。效果：`hf_trending`（huggingface.co 插件）从每次门禁 **16s → 0.2s**，check.sh 从 **15.5s → 1.8s**；注释里提 URL 的插件（如 cua 的 github 链接）不会误判。回归测试 `bdd-smoke-offline-external-skip`。
- **did-you-mean（P28 + P34）**：拼错有两层兜底 —— ① **步骤层（P28）**：决策步骤拼错/放错关键词时，`bdd build` 覆盖门禁逐条给 `did you mean the decision <kind> step: <tmpl>?`；② **语义层（P34）**：`validate_graph_semantics` 的**每个未知引用**（routing 目标 / start / primary_q / action kind / 问题引用 / answer key / gate option / combine fallback / when 运算符 / call capability）都追加 `(did you mean "..."?)`，候选来自图内真实标识符（节点名 + answer 值 / 问题名 / 已知 kind），相似度 <0.5 直接不给（绝不误报）。作者在 build 报错处就能看到该改成什么。
- **错误定位线号（P35）**：语义校验错误**自带 `.feature` 行号**（`line N: node X: ...`）—— `validate_graph_semantics` 在 `spec.rs` 里跑、离 feature 文本很远，`bdd build` / `bdd transpile` / MCP `bdd_validate` 三处出口都用 `locate_semantic_error_line` 按错误语法（`unknown "X"` / `"X" is not ...` / `... capability "X"`）从 parsed step 里反查最可能的出错行（P39 起 `call the capability` 也精确定位到该行）。agent 拿到的是「哪一行 + 改成什么」，不是一句裸拒绝。

expect 表现况：`bench/dsl_smoke_expect.json` 覆盖 **122** 个 spec / **960** 个 state（多节点 spec 的每个样本按 P38 逐步钉 `trace.step_count` + 每步 node/action/answer；trace-pinned stems **26**）；`bench/dsl_smoke_states.json` 注册 **125** 个 spec / **969** 个 state；**分支覆盖门禁** 断言每个 state-registered spec 的可达节点全集被走到（白名单仅 1 条，容器节点已结构性排除）；**覆盖台账** `125 offline-verified + 35 live-declared = 160 specs`。门禁实测：`bdd check.sh` 全绿、`dsl smoke` 160/160、`bdd round-trip` 160/160、`bdd doc-check` 9/9；dsl 侧回归套件（bdd-smoke-expect-coverage / -trace-path-floor / -branch-coverage / -offline-external-skip + bdd-coverage-ledger / bdd-json-isolation / bdd-score-corpus-floor / bdd-score-e2e-decision / bdd-decision-roundtrip-vocab / bdd-transpile-decision / validate-repo-specs-strict）**1470/0**。`laya-workflow-tests` 全量（mocks up）**2069/0**；offline 全量在当前环境会卡在 `capability-timeouts` 的 resource-leak soak（socket 读取不返回；单跑该 section 亦复现，与 BDD 改动无关，已登记待查），故本轮 dsl 侧回归以 section 直跑为准。

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
7. 反向搬运：**MCP `bdd_import`**（已有 JSON workflow spec → `@decision` feature，附 `graph_matches` 往返证明；批量走 CLI
   `laya-workflow bdd import --dir <DIR> --into <cat>`）—— 让「把已知 workflow 转成 BDD」也能在 MCP 里一把完成。
8. 运行决策：先 **MCP `bdd_list`** 发现可用 workflow（返回每个 `@decision` 图的 `cat`/`name`/`description`/`start`/`nodes`/
   每问 `questions`（id+type+`answer_field`）/ `inputs`（离线调用要喂的 state 字段＝每问 heuristic 的源字段 `answer_field`/`field`/`extract_numeric` ＋ 动作里
   所有 `${state.X}` 插值 ＋ 类型隐含读（如 `ui_proposal` 的 `surface`/`snapshot_hash`））/ `outputs`（该图能返回的全部
   决策 label 闭集＝`bdd_decide` 能回的全部 label：阶梯 label / gate 的 BLOCK·ALLOW·CONFIRM / combine 的 `route`（choice criteria 去掉
   fallback，再加 review_label）/ rubric 的 `status`（scored·human_review）；单节点图即全量、多节点图取 trace 末节点）/ `offline`（true＝该图经 `dsl smoke`
   离线行为验证，离线 `bdd_decide` 可信；false＝只在 live 验证过——`verified_by` 指 gate、`why_live` 说明原因，离线调用只是默认走查）+ 顶层 `offline_count`/`live_count`）
   —— `bdd_list` 递归扫 `dsl/` 子树（与 `bdd_decide {name}` 的解析同源，`cat` 取相对子路径，如 `routing/sub`）；再用 **MCP `bdd_decide`**
   （`name` 或 `feature_text`/`feature_path`/`spec_path` + `state` → `label`（终态节点的决策投影：threshold label / gate_action / combine `route` / rubric `status`，都没有才回落 `stop`）+ 全 `trace` + `result` + 决策契约
   `inputs`/`missing_inputs`/`offline`/`verified_by`/`why_live`，与 `dsl smoke` 同一离线确定性引擎）跑它：`missing_inputs` 非空说明有声明字段没喂、
   label 可能来自默认值；`inputs` 为空说明离线引擎确实不读 state（该图需真实模型后端驱动）——
   让 BDD 图成为 agent **可发现、可调用、且诚实**的决策核心，而不只是可写的产物。
9. 全量门禁：`laya-workflow bdd round-trip`（160/160 复现）+ `scripts/bdd/check.sh`。
10. 提交时 `bdd round-trip` 会拦：dsl 有 spec 但没 `@decision` 源 = FAIL。

### 为什么这样是 agentic workflow 的核心

- **确定性**：`@decision` 词汇表是确定性编译器（正则 → 决策图），0ms、无歧义；validate 双保险。
- **正确性可证**：语义图校验 + 决策输出断言 + **决策 trace 全钉** + offline-detection，让"决策对没对"是
  可测、可复现、可回归的（P24-P31），不只是"能编译"。
- **单一事实来源**：`.feature` 是源，`dsl/*.json` 是编译产物；round-trip 证明等价，杜绝双份漂移。
- **可直接运行 / 可被 agent 调用**：`laya-workflow run --spec dsl/<cat>/<name>.json` 用 HeuristicBackend / 真实 LLM 跑决策图；MCP `bdd_decide` 让同一张图在进程内被 agent 直接调用（feature/spec + state → label + trace）。
- **完整性可审计**：`bdd_catalog` 让"所有已知 workflow 都 BDD 化"这件事可测（孤儿即报）。
