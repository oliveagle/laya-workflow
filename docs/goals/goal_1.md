# 目标：为 laya-tch 增加 macOS MLX 推理后端（Phase 1：MLX 设备路径 + 真实 GPU 计算冒烟 + 基准），在 Apple Silicon 上用 MLX(Metal GPU) 跑通真实前向计算并给出性能数据

## 背景

- 现状：`laya-tch` 只支持 CPU / CUDA（`tch-rs` / libtorch）。在 macOS Apple Silicon 上
  **没有可用的 libtorch 预编译包**（`torch-sys` 构建直接失败：`Pre-built version of
  libtorch for apple silicon are not available`），CUDA 也不存在。
- Apple Silicon 的原生高性能推理路径是 **MLX（Metal GPU）**。本机已具备：
  - Python `mlx` 0.31.1（`mlx.core` 默认设备为 `gpu`）；
  - 已转换好的 **FP16 MLX 检查点** `aac6fef/laya-mlx`（206 个张量，MLX 原生参数名，
    `encoder/config.json` + `model.safetensors`），位于本地 HF 缓存。
- 本目标交付 **Phase 1**：打通 MLX 设备解析 + 真实 MLX GPU 前向计算（基于真实检查点的
  一个编码器层）+ 基准，为后续"完整模型移植 + 端到端 parity"打基础。
- **不做（非目标）**：本阶段不要求完成完整 ModernBERT-large + 决策头的端到端移植，
  也不要求与 PyTorch 参考的端到端数值 parity；那是后续阶段。

## 任务

1. **设备解析（Rust）**：在 `laya-tch` CLI 的 `--device` 取值中新增 `mlx`：
   - 仅 macOS 有效；`auto` 在 Apple Silicon 且 MLX 可用时优先 `mlx`，否则回退 `cpu`；
   - 非 macOS 传入 `mlx` 时给出清晰错误（而不是静默退化成 cpu）；
   - 增加可被 `cargo test` 覆盖的单元测试（设备解析逻辑与平台判定）。
2. **MLX 运行时（自包含 Python，只用 `mlx.core`）**：新增 `laya-tch/mlx/`：
   - `mlx_smoke.py`：定位 FP16 MLX 检查点（优先 `$LAYA_MLX_MODEL_DIR`，其次
     `$LAYA_MODEL_DIR`，再其次本地 HF 缓存里的 `*laya-mlx*` 快照；均找不到则以非 0 退出），
     加载**一个真实编码器层**的权重，在 Apple GPU 上执行一次真实前向
     （LayerNorm + Linear/QKV + GeGLU），并断言：输出全部有限（非 NaN/Inf）、
     与同一份权重的 f32 参考实现（numpy）在容差内一致；打印 `PASS` 并 exit 0。
   - `bench.py`：在 Apple GPU 上测量并打印 MLX 的 linear 吞吐（GFLOPS）与单次
     编码器层前向耗时，输出确定性、可复现；exit 0。
3. **文档**：在 `laya-tch/README.md` 增补 "MLX (macOS)" 一节：如何运行 smoke / bench、
   本机依赖（Python + mlx）、以及 Phase 1 的范围与限制。
4. **不破坏既有路径**：CPU / CUDA 行为保持不变；`laya-tch` 在 macOS 上仍能构建
   （以本机安装的 libtorch 为 `LIBTORCH` 来源）。

## 验收命令

<!--
  由外部脚本在仓库根目录真实执行，必须全部 exit 0，goal 才算完成。
  说明：laya-tch 链接 libtorch，本机 libtorch 来自 pip 的 torch 包，故构建/测试命令
  内联解析 LIBTORCH / DYLD_LIBRARY_PATH。
-->

```accept
LIBTORCH="$(python3 -c 'import torch,pathlib;print(pathlib.Path(torch.__file__).parent)')" DYLD_LIBRARY_PATH="$(python3 -c 'import torch,pathlib;print(pathlib.Path(torch.__file__).parent)')/lib" cargo build -p laya-tch
LIBTORCH="$(python3 -c 'import torch,pathlib;print(pathlib.Path(torch.__file__).parent)')" DYLD_LIBRARY_PATH="$(python3 -c 'import torch,pathlib;print(pathlib.Path(torch.__file__).parent)')/lib" cargo test -p laya-tch --quiet
python3 laya-tch/mlx/mlx_smoke.py
python3 laya-tch/mlx/bench.py
```

## 完成情况说明要求（可选）

- 引用真实文件路径（`laya-tch/src/*.rs`、`laya-tch/mlx/*.py`、`laya-tch/README.md`）。
- 给出 `mlx_smoke.py` 的 `PASS` 输出与 `bench.py` 的 GFLOPS/耗时数字。
- 给出 `cargo test` 的通过条目（含新增的设备解析测试名）。

## 备注（可选）

- 工作目录：本仓库根目录。
- 预算建议：`--max-rounds 20 --round-timeout 1200`。
- MLX 检查点快照示例：
  `~/.cache/huggingface/hub/models--aac6fef--laya-mlx/snapshots/*/`。
