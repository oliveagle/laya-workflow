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
python3 scripts/bdd/build.py                     # 本地：编译 + validate + 100% 覆盖
python3 scripts/bdd/build.py --assist            # + 词汇表外步骤的 needle 建议
python3 scripts/bdd/build.py --profile production \
  --base-url https://app.example.com             # 生产集成测试计划（@production 场景）
python3 scripts/bdd/run.py --profile production \
  --base-url https://app.example.com             # 真正执行生产 IT（需要 Chrome/CDP）
```

**门禁是硬性的**：词汇表外步骤 = 编译错误（exit 1）；生成的 spec 必须过
`laya-workflow validate`；每 feature 覆盖率必须 100%；选了 0 个场景 = 错误。

## 二、写 / 改 feature 的规则

1. **只用标准词汇表**（`scripts/bdd/steps.py` 里的规则）。当前支持：
   `the browser is ready`、`I am on "<url>"`、`I open/navigate to "<url>"`、
   `I wait for the element "<sel>"`、`I click the element "<sel>"`、
   `I type "<text>" into the element "<sel>"`、`I select "<v>" in the element "<sel>"`、
   `I run javascript "<expr>"`、`I release the page`；断言 9 种：
   `the page title contains` / `the page url contains` / `the element ... is visible|absent` /
   `javascript ... is true|false|equals|contains|equals text`。
2. **一个 tag 一行**（Gherkin 解析器一行只认一个 `@tag`）：
   ```
   @production
   @smoke
   Scenario: ...
   ```
3. **外部地址用 `<base_url>`**：`Given I am on "<base_url>/"`——本地 profile 注入
   fixture 服务器地址，生产 profile 注入 `--base-url`。绝不硬编码生产域名。
4. **加新 step 必须先扩词汇表**，再在 feature 里用它：
   `scripts/bdd/steps.py` 的 `_GIVEN/_WHEN/_THEN` 表加正则 + `_args()` 加参数映射，
   然后 `scripts/bdd/vocabulary_check.py` 加一行断言它映射到正确 op。否则
   `build.py` 会以"unknown step"拒绝——这不是 bug，是门禁。
5. **生产集成测试场景打 `@production`**，只跑真实目标，绝不引用本地 fixture
   （`htmx.html` 这类）。

## 三、维护循环（每次改动都走）

```bash
# 1. 改 .feature / steps.py
# 2. 本地门禁（不需要 Chrome）
python3 scripts/bdd/build.py --assist
# 3. 有词汇表外步骤？先看 --assist 的建议（带置信度），
#    把可接受的规则写进 steps.py，再重跑。建议永远不直接编译。
# 4. 需要真跑浏览器（有 Chrome 的机器 / CI）
scripts/bdd/run.py
scripts/bdd/check.sh            # 或 scripts/verify.sh，含本门禁
# 5. 生产 IT（可选，有部署目标时）
python3 scripts/bdd/build.py --profile production --base-url <real>
python3 scripts/bdd/run.py --profile production --base-url <real>
```

## 四、准确率策略（为什么这样分）

- **确定性编译器 = 100% 精度**：step 正则 → op/assertion/参数，0ms，无歧义。
  一切能靠规则表达的步骤都走这里。
- **needle 只做"建议"**：`--assist` 对词汇表外步骤跑端上小模型（多工具+triggers），
  打印 `op + 置信度`，`≥0.5` 标 LIKELY、`<0.5` 标 guess。它**绝不自动编译**——
  研究（`docs/bdd_to_needle.md`）实测：基础模型对生造措辞全字段准确率约 40%、
  只有 ~10% 过 0.1 置信度 floor，所以建议只是给 agent 的一个输入，最终以
  `steps.py` 里的确定规则为准。
- **validate 双保险**：build 生成的每个 spec 都跑真实 `laya-workflow validate`，
  结构不合法即失败。

## 五、常见错误 → 正确动作

| 报错 | 含义 | 动作 |
|---|---|---|
| `unknown then step: '...'` | 词汇表不认这个 step | 用 `--assist` 看建议 → 加 `steps.py` 规则 |
| `step coverage 40% < 100%` | feature 里有未匹配步骤 | 同上，或改写成标准措辞 |
| `no @production scenarios selected` | 生产计划为空 | 给场景加 `@production` tag |
| `--profile production needs --base-url` | 生产没给目标 | 传 `--base-url` 或设 `$BDD_BASE_URL` |
| `validate rejected the generated spec` | 生成的 spec 结构不合法 | 改 feature 让它编译出合法图 |

## 六、目录速查

- `bdd/features/*.feature` — 正式场景（CI 门禁的 glob）
- `bdd/features/setup/` — `include:` 拉入的步骤列表（无 Feature 头）
- `bdd/examples/` — 模板/示例（不进 CI glob），如 `production_it.feature`
- `bdd/fixtures/` — 本地 hermetic 夹具页
- `scripts/bdd/steps.py` — 词汇表（step → op/assertion）
- `scripts/bdd/transpile.py` — 编译器（Gherkin → spec）
- `scripts/bdd/build.py` — 统一 accuracy gate + 多产物（本指南的核心命令）
- `scripts/bdd/run.py` — 执行器（本地 hermetic / 生产 IT）
- `scripts/bdd/needle_assist.py` — needle 建议模块（只建议，不编译）
