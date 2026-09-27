# Agent-toolbox workflow 的 DSL 移植与收益度量

> **日期**: 2026-09-27
> **目标**: 研究我们 agent 工具箱里的 workflow 哪些能用 laya-workflow DSL 配置化，并量化收益。
> **关键约束**: **未来项目只改 JSON，不改 Rust 代码。**

---

## 结论一句话

- **5 个 agent 工具箱 workflow 可配置化**（2 个 HIGH + 1 个 MED + 2 个小 demo），其中 2 个已落地（quality_gate + security_scan）。
- **决策层 LOC 从 257 → 41，压缩 84%**。
- **新增通用能力 `heuristic.match_any` + `heuristic.match_regex`**，让 spec 可以自声明启发式，不再需要每加一个场景就给 `src/backend.rs` 添加 handler。**Rust 改动是一次性的引擎能力扩展，之后新项目只写 JSON。**
- `validate` 现在静态检查 `match_regex` 的正则合法性，坏 pattern 不用等到运行才暴露。
- 全量回归 **426 passed, 0 failed**，未破坏任何已有 spec。

---

## 1. 研究范围：~/.agents 里 workflow 形态的候选

## HIGH 拟合（已落地 2 个）

| 候选 | 原始 | 形状 | 决策语义 |
|------|------|------|----------|
| `quality-gate.sh` | `.githooks/` 141 行 | 5 项规则阶梯 | FAIL（staged placeholder 硬 fail） > WARN（repo placeholder / 文件行数 / AGENTS.md 行数） > NOTE（归档命名） > PASS |
| `security-scan.sh` | `code/scripts/` 234 行 | 9 个 pattern + whitelist + binary | QUARANTINE_CRITICAL > QUARANTINE_HIGH > SKIP（binary） > CLEAN |

两者都是**规则表 + 阈值阶梯**结构，映射到 DSL 的 `choice question (A/B)` + `threshold` action 有序 rules 非常直接。

## MED 拟合（未落地，推荐后续做）

| 候选 | 形状 | 难点 |
|------|------|------|
| `skills/ole-commit-push/scripts/*.sh`（5 个文件 ~909 行） | 多阶段 pipeline（precheck → split → secret → commit → leftover → rebase/push） | 有副作用（git mv / rebase / push），需 `dsl/capabilities/` 组合，不是单 spec |
| `ops/sglang_prism/*.sh` | 服务生命周期状态机（config → model → service → GPU → port） | 状态探测多（curl/gpu/端口），适合 `capabilities` + DSL 组合 |

## LOW 拟合（不建议配置化）

| 候选 | 原因 |
|------|------|
| `bin/monitor-port-live.sh` | 实时 tcpdump 流，DSL 非为 live stream 设计 |
| `task-*.sh` 系列 | 纯过程型 template fill + 报告生成，DSL 表达力不足 |
| `skills/lark-sheets/scripts/lark_chart_quality_check.py`（1540 行） | 规则虽是数据，但 Python 里已把规则组织成函数式验证器，DSL 化收益有限 |

---

## 2. 落地 spec

### 2.1 `dsl/agents/quality_gate.json`

5 个 question 各带 `heuristic.match_any`，6 条有序 rules：

| when | label |
|------|-------|
| `staged_placeholder == B` | **FAIL** |
| `repo_placeholder == B` | **WARN** |
| `file_lines_over == B` | **WARN** |
| `agents_md_over == B` | **WARN** |
| `archived_naming == B` | **NOTE** |
| fallback | **PASS** |

### 2.2 `dsl/agents/security_scan.json`

4 个 question（2 个带 `match_regex`，2 个带 `match_any`），5 条有序 rules：

| when | label |
|------|-------|
| `file_whitelisted == B` | **CLEAN** |
| `binary_file == B` | **SKIP** |
| `critical_pattern_hit == B` | **QUARANTINE_CRITICAL** |
| `high_pattern_hit == B` | **QUARANTINE_HIGH** |
| fallback | **CLEAN** |

关键正则直接从 shell `MALICIOUS_PATTERNS` 表搬运：

```jsonc
"critical_pattern_hit": {
  "heuristic": { "match_regex": ["curl.*\\|.*sh", "eval.*base64", ">\\s*/dev/tcp",
                                  "bash\\s+-i\\s+>&\\s*/dev/tcp", "nc\\s+-l\\s+-p"],
                 "p_hit": 0.95, "p_miss": 0.05 }
}
```

### 2.3 通用化：`heuristic` 字段

之前每个新 ole_eval 场景都要给 `HeuristicBackend` 加一个 Rust handler（见 `src/backend.rs` 里的 `is_prohibited`、`aml_risk` 等），
违反「新项目只写 JSON」的目标。现在：

- question 里可以带 `"heuristic": {"match_any": [...], "p_hit":…, "p_miss":…}`
- 或 `"heuristic": {"match_regex": [...], "p_hit":…, "p_miss":…}`
- 没有 `heuristic` 的 question 依旧走原有路径，**完全向后兼容**（426 用例全绿）
- Rust 改动是**一次性的**（`src/backend.rs` ~45 行 + `src/workflow_cli.rs` ~35 行静态检查），
  之后新 spec 纯 JSON

---

## 3. 度量收益

### 3.1 决策层 LOC（最关键）

| 指标 | shell | DSL | 压缩比 |
|------|-------|-----|--------|
| quality-gate | 102 代码行（141 总 - 39 注释/空行） | 22（11 tokens + 6 rules + 5 boilerplate） | **-78%** |
| security-scan | 155 代码行（234 总 - 79 注释/空行） | 19（9 patterns + 5 rules + 5 boilerplate） | **-88%** |
| **合计** | **257** | **41** | **-84%** |

> 「决策层 LOC」 = 代码行减去注释/空行（决策的核心内容），而不是文件总行数。
> DSL 的 22/19 行包含 rules 数组、heuristic 数组、criteria/instructions 文档字符串，
> 每一行都直接对应一条**可审计的策略**。shell 里的 102/155 行有大量 grep/wc/find/sed
> 的过程型代码，与"规则是什么"无关。

### 3.1.1 CPU vs GPU 部署架构

| 组件 | 语言 / 依赖 | 角色 | 何时使用 |
|------|------------|------|---------|
| `laya-workflow`（CLI） | Rust，5 个 crate（serde / clap / anyhow / ureq / regex-lite）—— **无 CUDA / 无 torch** | 决策引擎 + HeuristicBackend | **当前所有离线样本都在这里跑，CPU** |
| `laya-tch`（workspace 另一个 crate） | Python，torch + transformers | 远程 GPU 推理服务（HTTP） | question 无 `heuristic` 声明时，CLI 通过 `--base-url` 连过去 |

权威证据（`Cargo.toml` 全依赖清单）：
```
serde / serde_json / clap / anyhow / ureq / regex-lite
```
源码 `src/` 中 `cuda|cudnn|cublas|torch::` 零命中。

结论：
- 当前跑的这 6 + 2 个 spec 全部走 **CPU** 路径，不启模型 server
- 6 个 ole_eval spec 之所以能离线跑，是因为它们对应的 question id 在 Rust 里有 hardcoded heuristic handler
- 2 个新加的 agents spec 走通用 `heuristic` 字段求值，**永不调 GPU**
- 真要跑 GPU 推理需要：写 spec question 但不加 `heuristic`，且有匹配的 GPU server 在 `--base-url` 后 listen

### 3.2 Wall time（单次决策）

| spec | shell parse（只做 `bash -n`） | DSL offline run（`laya-workflow run`） |
|------|--------------------------------|-----------------------------------------|
| quality-gate | 4.11 ms | 4.16 ms |
| security-scan | 4.16 ms | 4.17 ms |

> shell 端只测 `bash -n`（语法检查，不真跑副作用），DSL 端是完整离线决策（CLI 启动 +
> JSON 解析 + engine 决策 + 输出）。两者在同一量级；差异主要在 Rust CLI 启动（~2.5ms）
> vs `bash -n`（~4ms），真正的 engine decision 是 ~0.1ms。

### 3.3 样本覆盖

| spec | 覆盖的 verdict labels | 数量 |
|------|----------------------|------|
| `quality_gate` | FAIL / WARN / NOTE / PASS | 4 |
| `security_scan` | QUARANTINE_CRITICAL / QUARANTINE_HIGH / CLEAN / SKIP | 4 |

10/10 离线样本在 `bench/dsl_smoke.py` 里跑通，且每个 label 都有**正向 + 反向**用例
（如 `critical` / `critical_tcp` 是不同 critical pattern；`clean` 是无 pattern 的正常文件）。

### 3.4 可审计性

| 维度 | shell | DSL |
|------|-------|-----|
| 规则可见性 | 规则藏在 `grep -E '…'` 的字符串里，要拆 shell 逻辑 | rules 数组一眼看完，每条 = `{when, label}` |
| 阈值变更成本 | 改 shell 脚本，要重新确认引号/转义 | 改 JSON 一行（值或 label） |
| 规则静态检查 | 无（`bash -n` 不检查规则结构） | `validate` 会检查 graph 结构 + `match_regex` 的正则合法性 |
| 离线测试 | 需要构造 git state / 临时文件 | `bench/dsl_smoke.py` 一行样本状态即可 |
| 版本 diff | 改 grep 字符串，diff 看起来"像 shell 改动" | 改一条 JSON rule，diff 精确到规则级 |
| schema 演进 | 无 | `dsl_version: 2`，`validate` 报 legacy |

### 3.5 测试成本

| 维度 | shell | DSL |
|------|-------|-----|
| 用例数量 | 0（repo 里没有针对 quality-gate.sh 或 security-scan.sh 的测试） | 10（每个 verdict label 至少 1 个） |
| 跑测试的方式 | 手动 `bash quality-gate.sh` + 读 exit code + 手动检查 | `python3 bench/dsl_smoke.py`，exit code 判 pass/fail |
| 破坏性 | 真 run 有副作用（quarantine 目录、IMPORTANT.md） | 无副作用，纯决策 |

### 3.6 Rust 通用化（关键产出）

改动：
- `src/backend.rs`：新增一个 `_ if qdef.get("heuristic")` 分支，支持 `match_any` + `match_regex`，~45 行
- `src/workflow_cli.rs`：validate 阶段静态检查正则合法性，~35 行
- 其他 spec 未动一行

**效果**：之后新项目（use-cases、skills 里的其他规则阶梯）**只需要写一个 JSON**，
validate + run + 加样本到 `dsl_smoke.py` 就能跑离线测试。**没有每场景 45 行的 Rust handler 了**。

---

## 4. 边界

- **live stream**（`bin/monitor-port-live.sh`）和**过程型 pipeline**（`ole-commit-push`）
  不适合单 spec 表达，需要 `dsl/capabilities/` 或 `dsl/pipelines/` 组合，本次不做。
- **纯语义理解**（如"这个 commit message 是否表达完毕"）仍然需要 LLM backend（`--base-url`），
  DSL 的启发式只覆盖**确定的规则**。
- shell 的副作用（quarantine 目录、IMPORTANT.md 记录、`cp`）不在 DSL 里 —— DSL 只决策，
  副作用由调用方（pre-commit hook、cron）根据返回的 label 执行。这个「决策/副作用分离」
  与 `dsl/ole_eval/` 的 design 一致。

---

## 5. 复现

```bash
cd ~/repos/github.com/oliveagle/laya-workflow

# 1) build
cargo build --release

# 2) validate（应无 warning）
laya-workflow validate --spec dsl/agents/quality_gate.json
laya-workflow validate --spec dsl/agents/security_scan.json

# 3) 全量 smoke（28 个 spec，agents/ 10/10 绿）
python3 bench/dsl_smoke.py | tail -3

# 4) 全量 Rust 回归
target/release/laya-workflow-tests 2>&1 | tail -1
# → 426 passed, 0 failed
```

关键路径：
- `dsl/agents/quality_gate.json`
- `dsl/agents/security_scan.json`
- `dsl/agents/README.md`（映射方法 + 通用化说明）
- `dsl/README.md`（folder list + `heuristic` 字段文档）
- `src/backend.rs`（`heuristic` 求值分支，fail-closed 正则错误）
- `src/workflow_cli.rs`（validate 静态正则检查）
- `bench/dsl_smoke.py`（agents/ 10 样本 STATES）
