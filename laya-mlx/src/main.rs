//! `laya-mlx` — native Rust MLX inference for the Laya decision model.
//!
//! Replaces the Python MLX runtime (`laya-tch/mlx/native/laya_mlx`) with a Rust
//! implementation built directly on Apple MLX (`mlx-rs`). Reads a request JSON
//! (`{"state": ..., "questions": {...}}`) and prints the same answer JSON.

use std::path::Path;

use anyhow::{anyhow, Result};
use clap::Parser;
use serde_json::Value;

use laya_mlx::runtime;

#[derive(Parser)]
#[command(name = "laya-mlx", about = "Native Rust MLX inference for Laya decisions")]
struct Cli {
    /// Checkpoint directory (default: $LAYA_MLX_MODEL_DIR / $LAYA_MODEL_DIR / HF cache).
    #[arg(long)]
    model_dir: Option<String>,
    /// Request JSON path ('-' reads stdin).
    #[arg(long)]
    request: Option<String>,
    /// Print only the response JSON.
    #[arg(long)]
    json: bool,
    /// Run as a benchmark: repeat the request N times and report timing.
    #[arg(long, value_name = "N")]
    bench: Option<usize>,
}

fn main() -> Result<()> {
    let cli = Cli::parse();
    let raw = match &cli.request {
        Some(p) if p != "-" => std::fs::read_to_string(p)?,
        _ => {
            use std::io::Read;
            let mut s = String::new();
            std::io::stdin().read_to_string(&mut s)?;
            s
        }
    };
    let req: Value = serde_json::from_str(&raw)?;
    let state = req.get("state").ok_or_else(|| anyhow!("request missing 'state'"))?.clone();
    let questions = req.get("questions").ok_or_else(|| anyhow!("request missing 'questions'"))?;

    let dir = runtime::resolve_model_dir(cli.model_dir.as_deref())?;
    if let Some(reps) = cli.bench {
        bench(&dir, &state, questions, reps.max(1))?;
        return Ok(());
    }
    let agent = runtime::Agent::load(Path::new(&dir))?;
    let result = agent.system_one(&state, questions)?;

    if cli.json {
        println!("{}", serde_json::to_string(&result)?);
    } else {
        eprintln!("[laya-mlx] model-dir : {}", dir.display());
        eprintln!("[laya-mlx] device    : mlx (gpu)");
        if let Some(ans) = result.get("answers").and_then(|a| a.as_object()) {
            for (qid, a) in ans {
                let summary = a
                    .get("choice")
                    .or_else(|| a.get("score"))
                    .or_else(|| a.get("noul"))
                    .cloned()
                    .unwrap_or(Value::Null);
                eprintln!("[laya-mlx] {qid:12} -> {summary}");
            }
        }
        println!("{}", serde_json::to_string(&result)?);
    }
    Ok(())
}

fn bench(dir: &Path, state: &Value, questions: &Value, reps: usize) -> Result<()> {
    use std::time::Instant;
    mlx_rs::Device::set_default(&mlx_rs::Device::gpu());
    eprintln!("[laya-mlx] default device: {:?}", mlx_rs::Device::default());
    gemm_probe();
    let agent = runtime::Agent::load(dir)?;
    let mut samples = Vec::with_capacity(reps);
    for i in 0..reps {
        let t = Instant::now();
        let _ = agent.system_one(state, questions)?;
        let ms = t.elapsed().as_secs_f64() * 1000.0;
        if i >= reps / 10 {
            samples.push(ms); // drop warm-up
        }
    }
    samples.sort_by(|a, b| a.partial_cmp(b).unwrap());
    let med = samples[samples.len() / 2];
    let p90 = samples[(samples.len() as f64 * 0.9) as usize % samples.len()];
    eprintln!(
        "[laya-mlx] bench: {} reps, median {:.1} ms, p90 {:.1} ms, min {:.1} ms",
        reps,
        med,
        p90,
        samples[0]
    );
    println!("{}\"median_ms\": {:.2}, \"p90_ms\": {:.2}, \"min_ms\": {:.2}}}", "{", med, p90, samples[0]);
    Ok(())
}

fn gemm_probe() {
    use std::time::Instant;
    use mlx_rs::{Array, Dtype};
    let n = 1024usize;
    let a = Array::from_slice(&vec![1.0f32; n * n], &[n as i32, n as i32]).as_dtype(Dtype::Float16).unwrap();
    let c = a.matmul(&a).unwrap();
    c.eval().unwrap();
    let iters = 30;
    let t = Instant::now();
    for _ in 0..iters {
        let c = a.matmul(&a).unwrap();
        c.eval().unwrap();
    }
    let ms = t.elapsed().as_secs_f64() * 1000.0 / iters as f64;
    let gflops = 2.0 * (n as f64).powi(3) / (ms / 1000.0) / 1e9;
    eprintln!("[laya-mlx] gemm {n}^3 f16: {ms:.2} ms, {gflops:.1} GFLOPS");
}

