//! `laya-mlx` — native Rust MLX inference for the Laya decision model.
//!
//! A Rust reimplementation (on `mlx-rs` / Apple MLX) of the full Laya
//! `DecisionModel` — ModernBERT-large encoder + decision head + scorer + action
//! head — plus the tokenizer, prompt construction and calibration. It replaces
//! the Python runtime under `laya-tch/mlx/native/`.

pub mod model;
pub mod prompt;
pub mod runtime;
