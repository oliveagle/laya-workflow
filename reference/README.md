# reference/ — 原始 Python 实现

`laya/` 是 Rust 引擎的**原始参考实现**（Python / PyTorch），迁移自
`ole-eval/code/laya`。保留用于语义对照、行为回归和未来移植工作，**不是**
发布产物，也不在 CI 里构建或运行。

> 注意：该目录的代码仍硬编码了 `code/laya/…` 的旧仓库相对路径、依赖
> `rl_agent_api`（上游 convaiinnovations/laya 仓库）以及本地 ONNX 权重，
> 因此**不能直接运行**。需要时先修 import / 路径。

## 目录

| 文件 | 用途 |
|------|------|
| `laya/workflow_engine.py` | `ResilientWorkflow` 图 runner（ROUTE/RETRY/ESCALATE/STOP/EXECUTE） |
| `laya/composition.py`     | `ResilientLoop` 通用优化 loop |
| `laya/optimizer_integration.py` | `LayaOptimizerLoop` 自进化优化器 |
| `laya/apps/*.py`          | 四个 app：agent gate / email triage / content moderation / drafts |
| `laya/engine.py`          | Laya DecisionModel wrapper（加载 PyTorch checkpoint） |
| `laya/server.py`          | FastAPI 服务端（`/v1/systemone`） |
| `laya/onnx_server.py`     | ONNX Runtime 推理服务端 |
| `laya/export_onnx.py`     | DecisionModel → ONNX 导出脚本 |
| `laya/cli.py` / `cli_compose.py` / `__main__.py` | CLI 入口 |
| `laya/bench_rust.py`      | 原始 Rust 引擎对拍脚本（参考） |
| `laya/tests/*.py`         | Python 单测（对照用） |
| `laya/laya_english.config.json` | 导出 ONNX 的 tokenizer 配置 |

## Rust 对照

Rust 引擎在 `../src/`：

- `src/workflow.rs`   ← `workflow_engine.py` + `composition.py`
- `src/optimizer.rs`  ← `optimizer_integration.py`
- `src/apps.rs`       ← `apps/*.py`
- `src/backend.rs`    ← `engine.py` + `server.py`
- `src/workflow_tests/` ← `tests/*.py`

## 模型

ONNX 权重（`laya_english*.onnx`，~5GB）不入 git。需要时从原始
convaiinnovations/laya checkpoint 用 `export_onnx.py` 重新导出。
