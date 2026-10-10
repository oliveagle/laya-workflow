# BDD Quickstart

一份 `.feature` 文件是**单一事实来源**——同时驱动 workflow 规格、本地测试、生产集成测试。
准确率是硬性最高标准，gate 默认 100% 覆盖、未匹配 step 直接编译失败。

## 30 秒看懂

```
                          ┌─ workflow    spec/<name>.json        (laya-workflow JSON)
bdd/*.feature  ── build ──┼─ 测试        manifest.json           (本地 hermetic，CI)
  (Gherkin 源)            └─ 生产集成     it.manifest.json         (@production + 外部 base-url)
                           + coverage.json                        (准确率账本：每步是否被词汇表编译)
```

| 命令 | 干什么 |
|---|---|
| `laya-workflow bdd build <features...>` | 本地：编译所有场景 + validate 每个 spec + 100% 覆盖检查 |
| `laya-workflow bdd build --assist` | 同上，并对词汇表外的步骤打印 needle 建议（只建议，不编译） |
| `laya-workflow bdd build --profile production --base-url https://app.example.com` | 只跑 `@production` 场景，输出 `it.manifest.json` |
| `laya-workflow bdd run [--profile production] [--base-url URL]` | 真浏览器执行（需要 Chrome） |
| `laya-workflow bdd score --check` | 工作流造册打分（离线）：词汇表 recall/precision、`@outputs` 合同、expressiveness |
| `laya-workflow bdd score --run` | 同上 + 真实 Chrome e2e 通过率 |
| `laya-workflow bdd compare` | 离线：BDD vs 手写 JSON 的体积对比 + 造册门禁（不进 CI 会漏掉，见下） |
| `laya-workflow bdd live` | 实战：29 个真实网站语料（需要 Chrome） |
| `scripts/bdd/check.sh [laya-workflow]` | CI 门禁（vocabulary + probe + args + doc + `laya-workflow bdd build` 严格编译） |

准确率门禁：词汇表外 step = 编译错误；生成的 spec 必过 `laya-workflow validate`；每 feature
覆盖率必须 100%（默认）；空选择（0 场景被选）= 错误；needle 只做建议、永不自动编译。

## 前置条件

- `laya-workflow` 二进制（`cargo build --release --locked -p laya-workflow`，或下载 release tarball 装到 PATH）
- Chrome（`scripts/bdd/chrome-headless.sh` 会自己找，没有时本机 `laya-workflow bdd run` 起不来——但 `laya-workflow bdd build` 不需要 Chrome，所以**编译/validate/覆盖率本机就能跑**）

## 完整例子：用户登录冒烟测试

文件：`bdd/examples/login_smoke.feature`

```gherkin
Feature: User login smoke
  End-to-end smoke test for the user login flow. The @smoke scenario runs in
  every build; the @production scenario is selected by `--profile production`,
  running against a real deployment instead of the local fixture server.

  @smoke
  Scenario: successful login with valid credentials
    Given I am on "<base_url>/login"
    Then the page title contains "Login"
    When I type "alice@example.com" into the element "#email"
    And I type "secret123" into the element "#password"
    And I click the element "#login-button"
    Then the page url contains "/dashboard"
    And the element "#welcome-message" is visible
    And javascript "document.querySelector('#username').textContent" contains "Alice"

  @production
  Scenario: login form renders correctly on production
    Given I am on "<base_url>/login"
    Then the page title contains "Login"
    And the element "#login-form" is visible
    And the element "#email" is visible
    And the element "#password" is visible
    And the element "#login-button" is visible
```

每个 step 必须是 `laya-workflow bdd vocabulary-check` 里**已有**的词汇；`<base_url>` 是运行期注入
的占位符（本地 = 本地 fixture 服务地址，生产 = `--base-url`）。两个 tag： `@smoke`
让场景每次都跑；`@production` 让场景只在生产 IT 里跑。

## 跑一下

### 1) 本地构建（不需要 Chrome）

```bash
laya-workflow bdd build bdd/examples/login_smoke.feature \
  --bin ./target/release/laya-workflow --out /tmp/bdd-example
```

输出：

```
  coverage 100.0%  14/14  ok   bdd/examples/login_smoke.feature
  profile local  base_url (local fixtures)
  specs 2  validated 2  assist-suggestions 0
  OK — every scenario compiled, every spec validated, coverage at the bar
```

### 2) 生产 IT 计划

```bash
laya-workflow bdd build bdd/examples/login_smoke.feature \
  --profile production --base-url https://app.example.com \
  --bin ./target/release/laya-workflow --out /tmp/bdd-example-it
```

输出：

```
  coverage 100.0%  14/14  ok   bdd/examples/login_smoke.feature
  profile production  base_url https://app.example.com
  specs 1  validated 1  production-IT 1  assist-suggestions 0
```

注意 `specs 1`：**只有 `@production` 场景入选**；`@smoke` 自动被生产 profile 跳过。

## 四个产物长什么样

### 产物 1：`spec/<name>.json` —— workflow 规格（laya-workflow 可执行）

完整文件在 `/tmp/bdd-example/spec/bdd_user_login_smoke_successful_login_with_valid_credentials.json`。
结构：

```jsonc
{
  "name": "bdd_user_login_smoke_successful_login_with_valid_credentials",
  "dsl_version": 2,
  "description": "Generated from login_smoke.feature by laya-workflow bdd transpile. ...",
  "start": "s0",
  "max_iterations": 10,
  "policy": { "allow_exec": true, "allow_hosts": ["127.0.0.1","localhost"], ... },
  "capabilities": {
    "chrome": { "kind": "chrome_cdp", "endpoint": "http://127.0.0.1:${state.cdp_port}", ... },
    "bdd":    { "kind": "plugin", "plugin": "bdd", "browser": "chrome", "timeout_ms": 60000 }
  },
  "nodes": [
    // 一个 Gherkin step 对应一个 node。`state.keep` 把 base_url / target_id 一路传到 done。
    // 关键模式：s0 调 chrome 打开 URL → s1 调 bdd 断言 title → s2 调 chrome 输 email → ...
    {
      "name": "s0", "primary_q": "ok",
      "action": { "kind": "call", "capability": "chrome",
                  "with": { "op": "open", "url": "${state.base_url}/login" },
                  "project": { "target_id": "/target_id" } },
      "edge": { "condition": { "A": "s1" }, "default": "STOP" }
    },
    {
      "name": "s1", "primary_q": "ok",
      "action": { "kind": "call", "capability": "bdd",
                  "with": { "op": "assert", "assertion": "title_contains",
                            "value": "Login", "target_id": "${state.target_id}" } },
      "edge": { "condition": { "A": "s2" }, "default": "STOP" }
    },
    {
      "name": "s2", "primary_q": "ok",
      "action": { "kind": "call", "capability": "chrome",
                  "with": { "op": "type", "selector": "#email",
                            "text": "alice@example.com", "target_id": "${state.target_id}" } },
      "edge": { "condition": { "A": "s3" }, "default": "STOP" }
    },
    // ... s3..s7 同模式，最后 s8 是 done 节点
  ]
}
```

要直接看完整 spec：`cat /tmp/bdd-example/spec/<name>.json`。

### 产物 2：`manifest.json` —— 本地测试清单（CI 用）

```json
{
  "profile": "local",
  "base_url": null,
  "scenarios": [
    {
      "feature": "bdd/examples/login_smoke.feature",
      "scenario": "successful login with valid credentials",
      "spec": "spec/bdd_user_login_smoke_successful_login_with_valid_credentials.json",
      "tags": ["smoke"],
      "profile": "local", "base_url": null,
      "nodes": 9, "validated": true
    },
    {
      "feature": "bdd/examples/login_smoke.feature",
      "scenario": "login form renders correctly on production",
      "spec": "spec/bdd_user_login_smoke_login_form_renders_correctly_on_production.json",
      "tags": ["production"],
      "profile": "local", "base_url": null,
      "nodes": 7, "validated": true
    }
  ]
}
```

`validated:true` 表示这个 spec 已经过了真实 `laya-workflow validate`。CI 直接拿这个文件读出
要跑什么场景。

### 产物 3：`it.manifest.json` —— 生产集成测试清单（只含 `@production`）

```json
{
  "profile": "production",
  "base_url": "https://app.example.com",
  "scenarios": [
    {
      "feature": "bdd/examples/login_smoke.feature",
      "scenario": "login form renders correctly on production",
      "spec": "spec/bdd_user_login_smoke_login_form_renders_correctly_on_production.json",
      "tags": ["production"],
      "profile": "production",
      "base_url": "https://app.example.com",
      "nodes": 7,
      "validated": true
    }
  ]
}
```

只有一个场景入选——`@smoke` 那个被生产 profile 跳过了。

### 产物 4：`coverage.json` —— 准确率账本

```json
[
  { "feature": "bdd/examples/login_smoke.feature",
    "total": 14, "matched": 14, "coverage": 100.0, "unmatched": [] }
]
```

`unmatched: []` 意味着 14 步全部被词汇表编译；只要有一行不空，整个 build 就 exit 1。

## 准确率门禁（实测的硬约束）

| 场景 | 行为 |
|---|---|
| 词汇表外的 step | **编译错误** exit 1 + 列出所有支持的 step |
| 生成的 spec 没过 `validate` | exit 1 + 引擎的拒绝原因 |
| 任何 feature 覆盖率 < 100% | exit 1 + 列出未匹配 step |
| `--profile production` 但 0 个 `@production` 场景 | exit 1 + 提示加 tag |
| 生产 profile 缺 `--base-url` | exit 1（不许静默用本地 fixture） |
| `--tags @no-such-tag` 选 0 个场景 | exit 1（"empty plan must not report success"） |
| `--assist` 给出的 needle 建议 | **绝不自动编译**；`conf ≥ 0.5` 标 LIKELY，< 0.5 标 guess |

示例：把 `Then the heading "#hero" should be displayed` 加到 feature 里，`laya-workflow bdd build` 会说：

```
errors:
  bdd/examples/login_smoke.feature: line 7: unknown then step: 'the heading "#hero" should be displayed'
  supported: absent, assert, bdd, chrome, click, contains, equals, ...
  coverage  0.0%  0/1  LOW
specs 0  validated 0
```

## 怎么跑（有 Chrome 的机器 / CI）

```bash
# 本地：fixture server + headless Chrome
laya-workflow bdd run                                    # 全部 bdd/features/*.feature
laya-workflow bdd run bdd/examples/login_smoke.feature    # 单个 feature
laya-workflow bdd run --tags @smoke                       # 只跑带 @smoke tag 的
laya-workflow bdd run --filter page_smoke                 # 按 feature 文件名子串

# 生产集成测试（真目标，无本地 fixture）
laya-workflow bdd run bdd/examples/login_smoke.feature \
  --profile production --base-url https://app.example.com
```

完整门禁（无 Chrome 也跑）：

```bash
scripts/bdd/check.sh ./target/release/laya-workflow
# 22/22 scenarios compile, validate and are 100% covered, 6/6 probe specs validate
```

## 想加一个新 step？

1. 写 `.feature`，先 ``laya-workflow bdd build`` 看它是否已经认识——绝大多数情况都认识。
2. 不认识时 ``laya-workflow bdd build` --assist`：对每个未匹配 step 打印 `op + 置信度`。
3. 真正正确的做法是**扩展 `laya-workflow bdd vocabulary-check`**（`laya-workflow bdd vocabulary-check` 自动检查 op 映射）。
4. 不正确的做法：把 needle 建议直接 paste 到 spec 里——研究（`docs/bdd_to_needle.md`）实测基础模型对生造措辞全字段准确率 ~40%，强自动编译会静默通过不正确的 spec。

```bash
# 1. 写 feature
# 2. 跑带 --assist 的 build
laya-workflow bdd build bdd/examples/my.feature --assist --out /tmp/x
# 3. 看建议（例：Then the heading "#hero" should be displayed -> op=type conf=0.06 guess）
# 4. 在 `plugins/bdd/main.rhai` 加规则：_THEN 表加一行 (re.compile(r'^the heading (?P<sel>.+) should be (visible|displayed)$'), 'assert', 'visible')
# 5. 重跑：`laya-workflow bdd build` exit 0，coverage 100%
```

## 下一步

- **更深入维护说明**：`bdd/AGENT.md`（agent 维护循环、规则、错误→动作表）
- **BDD skill**：已装到 codex + opencode（`~/.codex/skills/bdd/SKILL.md`），新会话里说"维护 BDD"会自动加载
- **可行性研究**：`docs/bdd_to_needle.md` + `laya-workflow bench bdd-to-needle`（为什么 needle 不当主路径）
- **CI 门禁**：`scripts/bdd/check.sh` 已经被 CI 跑；`scripts/verify.sh` 调用它

## 工作流模式（把没有接口的 UI 变成 agent 可调用的工具）

同一份 `.feature` 既能写测试，也能**描述工作流**——一串动作 + 采集 state 作为产物。
工作流专用 step：

```gherkin
Given I am on "<base_url>/app.html"
When I extract the page title into page_title          # 采集：表达式结果 → state.<key>
When I extract the attribute "data-app" of the element "#app" into app_version
When I click the element "#consent" if it is present   # 条件点击：不在这里就 no-op
When I type "ACME" into the element "#search"
When I press the key "Enter"                           # 真实按键
When I wait until the element "#export" becomes enabled # 等状态（wait for 只等存在）
When I extract the text of the element "#results .row" into first_row
Then the saved value "first_row" contains text "ACME"   # 读 state，不需要页面
```

用 tag 声明产物，`bdd build` 会把它当编译期合同（声明了却无人产出 = 编译失败）：

```
@outputs(page_title, app_version, first_row)
Scenario: Search the no-API app and capture its record
```

完整示例：`bdd/features/workflow_capture.feature`（目标页 `bdd/fixtures/app.html`）。

打分（离线 / 含 e2e）：

```bash
laya-workflow bdd score --check     # 词汇表 recall/precision、@outputs 合同、expressiveness
laya-workflow bdd score --run       # 再加真实 Chrome e2e（需要 CHROME_BIN / 系统 Chrome）
```

## 实战演练：29 个真实网站（`bdd live` / `bdd compare`）

词汇表只在夹具上跑通说明不了什么。实战语料是 `bdd/bench/live/*.feature`——
**29 个工作流打 29 个真实外部网站**，每个 ≥20 步，无夹具服务器、无 probe
（`laya-workflow bdd live`）。一台 headless Chrome 串行跑：**29 passed, 0 failed / 842 steps**。
完整报告（每站表格、BDD vs JSON 对比、首次跑暴露的坑）见 `bdd/bench/live/REPORT.md`。

```bash
laya-workflow bdd compare                # 离线：体积 + 造册门禁
laya-workflow bdd live                   # 打真实网站（设置 CHROME_BIN）
laya-workflow bdd run <feature> --live   # 单个实战用例
```

`bdd compare` 把每个 `.feature` 编译出来，量出手写 JSON 的等价规格：**JSON 是 BDD 的
~16 倍字节、~38 倍行数**。它还会给每种表示各埋一个"造册错误"，问各自的 gate 能否**离线**
抓到：BDD 的封闭词汇表把未知步骤直接编译报错（29/29）；JSON DSL 的 `validate` 只做结构校验，
对语义错误——写错的 op、悬空的 edge、拼错的 assertion——一个都抓不到，只能在页面上运行时暴
露（对照：同一个 `validate` 能抓出删掉的 `start`，所以这个 0 是真的门禁缺口，不是坏探针）。
