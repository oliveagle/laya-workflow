# BDD → needle → workflow JSON：可行性研究报告

**问题**：能否用 BDD（Gherkin `.feature`）写 workflow，然后用 Needle 3（端上小模型）
直接"转成真正的 JSON"？准确率有多高？

**方法**：全部用真实引擎跑（`laya-workflow mcp serve` + `libneedle.so`/`needle3.cact`，
base 121M/2-bit），不估算。复现：`python3 bench/bdd_to_needle.py`。对照组 = 仓库已有的
确定性编译器 `scripts/bdd/transpile.py`（46 个 step 模式，22 scenarios 全绿 ~8s，
0ms/步）。

## 结论（TL;DR）

**把"BDD 文本 → needle → 完整 workflow JSON"当主路径：不可行，准确率不可接受。**
但有一个可行子集：**确定性编译器做主路径，needle 只做"词汇表外步骤"的 op/参数抽取，
并且用 confidence 门禁 + `laya-workflow validate` 兜底**——这个混合架构有价值，
只是"整篇 JSON 由 needle 生成"这条路走不通。

## 实测数据（10 步，三种条件）

| 条件 | op 路由 | assertion | 参数 | 全字段 | conf≥0.1 |
|---|---|---|---|---|---|
| A 单工具 extract（枚举 schema） | 6/10 | 9/10 | 1/10 | **0/10** | 2/10 |
| B 多工具 complete + triggers（仓库内 46 步子集） | 9/10 | 7/10 | 7/10 | **6/10** | 6/10 |
| C 多工具 complete（生造措辞，正则编译器 0/10 覆盖） | 4/10 | 5/10 | 9/10 | **4/10** | 1/10 |

（B 条件重跑两轮 full 6/10 稳定；引擎有采样，单步会抖动。）

### 为什么 A 不可用
- 单一 `bdd_step` 工具 + `op`/`assertion` 枚举：模型无视枚举成员，输出 `op:"open"`
  之类的**语义错误**值；`title_contains` 被识别成 `open`，`is visible` 被识别成
  `evaluate`。grammar 只保证"JSON 形状合法"，**不保证语义正确**。
- 参数层全崩：`value`/`expected` 互相放错位（1/10）。置信度 2/10 过 0.1 floor——
  引擎自己都知道不可信。

### 为什么 B 是 needle 的天花板
- 按 vendor 的 "Design Tools for Needle 3"（one tool per action + `triggers` 正则），
  **op 路由 9/10** 是能用的；但"生成可直接执行的 JSON"要 op+assertion+参数全对：
  只有 **6/10**，且 6/10 过 0.1 置信度 floor。
- 就算全对，它也**只生成了 node 的动作片段**，不是完整 spec——graph、edge、
  policy、capabilities、`${state.target_id}` 传递这些结构性字段它完全不碰。

### 为什么 C 否定了"用 needle 补词汇表外"的期望
- 正则编译器 0/10 覆盖的那 10 个生造步骤，恰好是"模型才有用武之地"的场景，
  needle 却只有 **4/10 全字段、1/10 过置信度 floor**——比抛硬币差。
- `assert/visible → type_text`、`click → release_page`、`assert/absent → navigate`：
  这正是需要"读意图"的地方，121M/2-bit 基础模型扛不住。

## 与仓库既有基准的对照

`docs/needle_evidence.md`（同一 base 模型）早已标出边界：
- 强：**实体抽取 5/5**、**结构化抽取 total 8/8**（grammar 保证类型）、embedding 0.95。
- 弱：**routing 60%**、**workflow classify 20%**。
- 本次实测的 "step → op/assertion" 正是弱项那一类（分类/路由），所以 6/10 全字段、
  生造 4/10 一点也不意外。

## 可行架构（推荐）

1. **Gherkin 保持人的意图层**；标准词汇表走 `scripts/bdd/transpile.py`
   （确定性、0ms、100% 精确、可 `laya-workflow validate` + CI 门禁）。
2. **needle 只接词汇表外/口述意图**：自然语言 → 多工具 + triggers → `{op, assertion,
   value}`；**confidence ≥ 0.5 才接受**，低于就拒绝交给人工（仓库 routing 模式：
   heuristic 命中 0ms，miss 才升级到 needle）。
3. **产物永远是"片段"不是"整篇"**：needle 出的动作 JSON 由编译器模板套进完整 spec
   （graph/policy/capabilities 由模板保证），**生成结果必须过 `laya-workflow validate`**
   才算数——不合格即拒绝，绝不静默写入。
4. 别让 needle 直接产整个 workflow JSON：那是 200–500 行嵌套结构，超出
   `max_new_tokens` 80–512 的生成能力（8 字段发票都只有 62–88% 准确率）。

## 一句话

**思路方向对（BDD 写、机器转），但"转"这一步应该由确定性编译器完成；needle 只能在
"词汇表外 + 高置信度 + validate 兜底"的窄缝里帮忙，准确率全字段 6/10、生造 4/10，
不能当主路径。**
