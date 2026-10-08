//! `laya-mlx` — native Rust MLX inference for the Laya decision model.
//!
//! Replaces the Python MLX runtime (`laya-tch/mlx/native/laya_mlx`) with a Rust
//! implementation built directly on Apple MLX (`mlx-rs`). Reads a request JSON
//! (`{"state": ..., "questions": {...}}`) and prints the same answer JSON.
//!
//! `--serve` starts a Jev-compatible HTTP server (`POST /v1/systemone`,
//! `GET /health`) with the model loaded once and kept in memory.

use std::io::{BufRead, BufReader, Read, Write};
use std::net::{TcpListener, TcpStream};
use std::path::{Path, PathBuf};
use std::io::ErrorKind;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc;
use std::sync::Arc;
use std::time::{Duration, Instant};

use anyhow::{anyhow, Result};
use clap::Parser;
use serde_json::{json, Value};

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
    /// Serve HTTP instead of one-shot inference.
    #[arg(long)]
    serve: bool,
    /// Listen address for --serve (default: 127.0.0.1).
    #[arg(long, default_value = "127.0.0.1")]
    host: String,
    /// Listen port for --serve (default: 8400).
    #[arg(long, default_value_t = 8400)]
    port: u16,
}

fn main() -> Result<()> {
    let cli = Cli::parse();
    let dir = runtime::resolve_model_dir(cli.model_dir.as_deref())?;

    if cli.serve {
        return serve(&dir, &cli.host, cli.port);
    }

    if let Some(reps) = cli.bench {
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
        return bench(Path::new(&dir), &state, questions, reps.max(1));
    }

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
    samples.sort_by(|a, b| a.partial_cmp(&b).unwrap());
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

// ─── HTTP server ───────────────────────────────────────────────────────────
//
// The model lives in one dedicated worker thread (created and used there, so
// no MLX type ever crosses a thread boundary). Connection threads parse HTTP
// and hand requests over a channel; one-shot channels carry answers back.

struct Job {
    state: Value,
    questions: Value,
    respond: mpsc::Sender<Result<Value, String>>,
    /// Set when the client hangs up before the answer is ready; the worker
    /// then skips the forward pass entirely (aborted keystrokes would
    /// otherwise clog the serial queue for seconds each).
    cancelled: Arc<AtomicBool>,
}

fn serve(dir: &PathBuf, host: &str, port: u16) -> Result<()> {
    let addr = format!("{host}:{port}");
    let listener = TcpListener::bind(&addr)?;
    eprintln!("[laya-mlx] model-dir : {}", dir.display());
    eprintln!("[laya-mlx] device    : mlx (gpu)");
    eprintln!("[laya-mlx] listening : http://{addr}");
    eprintln!("[laya-mlx] endpoints : POST /v1/systemone · GET /health");

    let (job_tx, job_rx) = mpsc::channel::<Job>();
    let (ready_tx, ready_rx) = mpsc::channel::<Result<(), String>>();
    let dir_str = dir.display().to_string();

    std::thread::Builder::new()
        .name("laya-mlx-agent".to_string())
        .spawn(move || {
            let agent = match runtime::Agent::load(Path::new(&dir_str)) {
                Ok(a) => {
                    let _ = ready_tx.send(Ok(()));
                    a
                }
                Err(e) => {
                    let _ = ready_tx.send(Err(format!("{e:#}")));
                    return;
                }
            };
            for job in job_rx {
                if job.cancelled.load(Ordering::Relaxed) {
                    continue; // client went away (e.g. the panel aborted a stale keystroke)
                }
                let t0 = Instant::now();
                let out = agent
                    .system_one(&job.state, &job.questions)
                    .map_err(|e| format!("{e:#}"));
                let ms = t0.elapsed().as_secs_f64() * 1000.0;
                eprintln!(
                    "[laya-mlx] {ms:.0}ms  questions={}",
                    job.questions.as_object().map(|m| m.len()).unwrap_or(0)
                );
                let out = out.map(|mut v| {
                    if let Some(o) = v.as_object_mut() {
                        o.insert("ms".into(), json!(ms));
                    }
                    v
                });
                let _ = job.respond.send(out);
            }
        })?;

    match ready_rx.recv_timeout(Duration::from_secs(120)) {
        Ok(Ok(())) => {}
        Ok(Err(e)) => return Err(anyhow!("failed to load model: {e}")),
        Err(_) => return Err(anyhow!("model worker did not start")),
    }

    for stream in listener.incoming() {
        let stream = match stream {
            Ok(s) => s,
            Err(e) => {
                eprintln!("[laya-mlx] accept error: {e}");
                continue;
            }
        };
        let tx = job_tx.clone();
        std::thread::spawn(move || {
            if let Err(e) = handle(stream, &tx) {
                eprintln!("[laya-mlx] connection error: {e}");
            }
        });
    }
    Ok(())
}

fn handle(mut stream: TcpStream, job_tx: &mpsc::Sender<Job>) -> std::io::Result<()> {
    stream.set_read_timeout(Some(Duration::from_secs(30)))?;
    stream.set_write_timeout(Some(Duration::from_secs(30)))?;

    let mut reader = BufReader::new(&mut stream);
    let mut request_line = String::new();
    if reader.read_line(&mut request_line)? == 0 {
        return Ok(());
    }
    let mut parts = request_line.split_whitespace();
    let method = parts.next().unwrap_or("").to_uppercase();
    let target = parts.next().unwrap_or("").to_string();
    let path = target.split('?').next().unwrap_or("").to_string();

    let mut content_length = 0usize;
    loop {
        let mut line = String::new();
        let n = reader.read_line(&mut line)?;
        if n == 0 || line == "\r\n" || line == "\n" {
            break;
        }
        let lower = line.to_ascii_lowercase();
        if let Some(v) = lower.strip_prefix("content-length:") {
            content_length = v.trim().parse().unwrap_or(0);
        }
    }
    let mut body = vec![0u8; content_length];
    if content_length > 0 {
        reader.read_exact(&mut body)?;
    }

    match (method.as_str(), path.as_str()) {
        ("OPTIONS", _) => respond(&mut stream, 204, "No Content", ""),
        ("GET", "/health") => respond(
            &mut stream,
            200,
            "OK",
            &serde_json::to_string(&json!({
                "ok": true,
                "model": "laya-mlx",
                "device": "mlx (gpu)"
            }))
            .unwrap(),
        ),
        ("POST", "/v1/systemone") => {
            let req: Value = match serde_json::from_slice(&body) {
                Ok(v) => v,
                Err(e) => return respond(&mut stream, 400, "Bad Request", &error_body(&format!("invalid JSON body: {e}"))),
            };
            let state = req.get("state").cloned().unwrap_or(Value::Null);
            let questions = match req.get("questions").and_then(|v| v.as_object()) {
                Some(_) => req.get("questions").cloned().unwrap(),
                None => return respond(&mut stream, 400, "Bad Request", &error_body("missing questions")),
            };
            let (tx, rx) = mpsc::channel::<Result<Value, String>>();
            let cancelled = Arc::new(AtomicBool::new(false));
            let flag = cancelled.clone();
            let _watcher = stream.try_clone().and_then(|mut peek| {
                peek.set_nonblocking(true)?;
                Ok(std::thread::spawn(move || {
                    let mut buf = [0u8; 1];
                    loop {
                        match peek.read(&mut buf) {
                            Ok(0) => {
                                flag.store(true, Ordering::Relaxed);
                                return;
                            }
                            Ok(_) => continue,
                            Err(e) if e.kind() == ErrorKind::WouldBlock => {
                                std::thread::sleep(Duration::from_millis(120));
                            }
                            Err(_) => {
                                flag.store(true, Ordering::Relaxed);
                                return;
                            }
                        }
                    }
                }))
            });
            if job_tx.send(Job { state, questions, respond: tx, cancelled }).is_err() {
                return respond(&mut stream, 503, "Service Unavailable", &error_body("model worker is not running"));
            }
            match rx.recv_timeout(Duration::from_secs(120)) {
                Ok(Ok(v)) => respond(&mut stream, 200, "OK", &serde_json::to_string(&v).unwrap()),
                Ok(Err(e)) => respond(&mut stream, 500, "Internal Server Error", &error_body(&e)),
                Err(_) => respond(&mut stream, 504, "Gateway Timeout", &error_body("inference timed out")),
            }
        }
        ("GET", "/v1/systemone") | ("POST", "/health") => respond(&mut stream, 405, "Method Not Allowed", &error_body("method not allowed")),
        _ => respond(&mut stream, 404, "Not Found", &error_body("not found")),
    }
}

fn error_body(detail: &str) -> String {
    serde_json::to_string(&json!({ "detail": detail })).unwrap()
}

fn respond(stream: &mut TcpStream, status: u16, reason: &str, body: &str) -> std::io::Result<()> {
    let response = format!(
        "HTTP/1.1 {status} {reason}\r\nContent-Type: application/json\r\nAccess-Control-Allow-Origin: *\r\nAccess-Control-Allow-Methods: GET, POST, OPTIONS\r\nAccess-Control-Allow-Headers: Content-Type, Authorization\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
        body.len()
    );
    stream.write_all(response.as_bytes())?;
    stream.flush()
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
