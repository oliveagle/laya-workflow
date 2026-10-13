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

> **隔离原则**：BDD 源与 JSON 完全隔离在两个根目录——所有 `.feature` 源在 `bdd/workflows/<类别>/`（层级镜像 `dsl/`），所有 JSON 工件在 `dsl/` + `bench/`。浏览器 feature 的侧车配置 `<name>.config.json` 归入 json 树（`dsl/browser/`），authoring 造册/args 门禁等工具数据归入 `bench/bdd/`。因此 **`bdd/` 内 0 个 `.json`**（含 bench/args_probes），`dsl/` 内 0 个 `.feature`。BDD 是源、JSON 是编译产物（单向生成）。

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

- **语义图校验（P24-P27）**：`from_spec` 阶段跑 `validate_graph_semantics` —— edge 目标 / action kind / threshold question 引用 / 节点名唯一 / 死节点可达性 / primary_q 存在 / copy_keys 键 ∈ 本节点 question / combine fallback ∈ criteria / workflow 引用环（A→B→A 拒绝并命名链条，菱形不误报）/ edge 条件键 ∈ 主问题 criteria。任何一条违反在 build 阶段就拒绝，而不是运行时静默错（真实抓过 3 处死代码/死路由）。
- **行为 smoke（P7 起）**：`dsl smoke` 用离线 HeuristicBackend 跑每个 dsl spec 注册的样本状态（`bench/dsl_smoke_states.json`），按 `bench/dsl_smoke_expect.json` 断言决策输出。99/99 specs OK。
- **决策输出断言（P21-P30）**：expect 表三种形态 —— ① string 钉最终 label/gate_action；② object 按 dotted-path 钉决策输出（`result.route`/`result.score`/数值 1e-9 容差）；③ P30 起 `bdd score --run` 的决策 e2e 编译 **authored feature** 跑同样状态、用同一个 `check_expect` 断言（修「跑通但决策错」的空洞 e2e，10/10 authoring task 全钉）。
- **决策 trace 全钉（P31）**：一个 state 走 >1 个节点时，它的 expect **必须**钉 `trace.steps.N.node`/`.action` + `trace.step_count`——最终 label 对但中间走错节点的 feature 会被拦（`walks N nodes but expect pins no trace path`）。17/17 multi-node stem 已钉死（dialogue_policy 状态机 retry 自环、persist_memory 5 节点链、triage_then_moderate gate_action 等）。回归测试 `bdd-smoke-trace-path-floor` 让这条要求是结构性的。
- **offline-detection（P29）**：网络型 stem（web_search/web_fetch/agent/rpc/http/webhook…）在本地 mocks 全不可达时 state 判 skip（validate-only），不误报回归；mocks up 时（`bench/mock_services.sh start` + 8791/8792 `mock serve`）全量强断言。
- **offline-external skip（P33，效率轴）**：离线 smoke **绝不对外网做真实 I/O**。`plugin` 型 capability 没有 `endpoint`/`url` 字段，改从插件脚本静态扫描 `http_get` 调用 + URL 字面量（去注释、识别字符串）判定网络依赖；spec 的网络目标只要含外部 host（非 loopback/RFC1918/link-local/CGNAT）就在离线 smoke 里整 state skip（validate-only，集成/live 任务真跑）。插件 `http_get` 在离线 smoke 里再叠一层 1.5s 超时上限兜底动态 URL。效果：`hf_trending`（huggingface.co 插件）从每次门禁 **16s → 0.2s**，check.sh 从 **15.5s → 1.8s**；注释里提 URL 的插件（如 cua 的 github 链接）不会误判。回归测试 `bdd-smoke-offline-external-skip`。
- **did-you-mean（P28 + P34）**：拼错有两层兜底 —— ① **步骤层（P28）**：决策步骤拼错/放错关键词时，`bdd build` 覆盖门禁逐条给 `did you mean the decision <kind> step: <tmpl>?`；② **语义层（P34）**：`validate_graph_semantics` 的**每个未知引用**（routing 目标 / start / primary_q / action kind / 问题引用 / answer key / gate option / combine fallback / when 运算符）都追加 `(did you mean "..."?)`，候选来自图内真实标识符（节点名 + answer 值 / 问题名 / 已知 kind），相似度 <0.5 直接不给（绝不误报）。作者在 build 报错处就能看到该改成什么。
- **错误定位线号（P35）**：语义校验错误**自带 `.feature` 行号**（`line N: node X: ...`）—— `validate_graph_semantics` 在 `spec.rs` 里跑、离 feature 文本很远，`bdd build` / `bdd transpile` / MCP `bdd_validate` 三处出口都用 `locate_semantic_error_line` 按错误语法（`unknown "X"` / `"X" is not ...`）从 parsed step 里反查最可能的出错行。agent 拿到的是「哪一行 + 改成什么」，不是一句裸拒绝。

expect 表现况：59 个 stem（28 个 object 形态 dotted-path 断言、17 个钉死 trace 路径）；`laya-workflow-tests` 1834/0（mocks up）/ 1757/0（mocks down）。

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
7. 全量门禁：`laya-workflow bdd round-trip`（99/99 复现）+ `scripts/bdd/check.sh`。
6. 提交时 `bdd round-trip` 会拦：dsl 有 spec 但没 `@decision` 源 = FAIL。

### 为什么这样是 agentic workflow 的核心

- **确定性**：`@decision` 词汇表是确定性编译器（正则 → 决策图），0ms、无歧义；validate 双保险。
- **正确性可证**：语义图校验 + 决策输出断言 + **决策 trace 全钉** + offline-detection，让"决策对没对"是
  可测、可复现、可回归的（P24-P31），不只是"能编译"。
- **单一事实来源**：`.feature` 是源，`dsl/*.json` 是编译产物；round-trip 证明等价，杜绝双份漂移。
- **可直接运行**：`laya-workflow run --spec dsl/<cat>/<name>.json` 用 HeuristicBackend / 真实 LLM 跑决策图。
- **完整性可审计**：`bdd_catalog` 让"所有已知 workflow 都 BDD 化"这件事可测（孤儿即报）。
