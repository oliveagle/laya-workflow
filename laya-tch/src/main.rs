//! Laya inference engine — tch-rs (libtorch / PyTorch C++ bindings).
//!
//! Jev-compatible `POST /v1/systemone` server + a one-shot CLI for parity tests.
//! The input rendering (`render_options` / `build_sequence`), temperature scaling
//! and answer formatting deliberately mirror `rl_common.py` / `rl_agent_api.py`
//! byte-for-byte, so outputs match the PyTorch reference.

use anyhow::{anyhow, bail, Result};
use clap::Parser;
use serde_json::{json, Map, Value};
use std::sync::{Arc, Mutex};
use std::time::Instant;
use tokenizers::Tokenizer;

mod model;
use model::{LayaModel, SeqInput};

const CLS_ID: u32 = 50281;
const SEP_ID: u32 = 50282;
const _PAD_ID: u32 = 50283;
const MASK_ID: u32 = 50284;
const MAX_LEN: usize = 512;
const HEAD_MAX_LEN: usize = 192;
const TEMPERATURE: [f64; 3] = [1.6369030475616455, 1.2514300346374512, 1.983399510383606];

// ─── CLI ───────────────────────────────────────────────────────────

#[derive(clap::Parser)]
#[command(name = "laya-tch")]
struct Cli {
    #[arg(short, long)]
    model_dir: String,
    #[arg(short, long, default_value_t = 8400)]
    port: u16,
    #[arg(long, default_value = "0.0.0.0")]
    host: String,
    /// One-shot: read a request JSON ({state, questions}), print the response JSON, exit.
    #[arg(long)]
    once: Option<String>,
}

// ─── Python-compatible JSON serialization (json.dumps(ensure_ascii=False)) ──

fn json_dumps_python(v: &Value) -> String {
    let mut s = String::new();
    write_json(v, &mut s);
    s
}

fn write_json(v: &Value, out: &mut String) {
    match v {
        Value::Null => out.push_str("null"),
        Value::Bool(b) => out.push_str(if *b { "true" } else { "false" }),
        Value::Number(n) => out.push_str(&n.to_string()),
        Value::String(s) => {
            out.push('"');
            for c in s.chars() {
                match c {
                    '"' => out.push_str("\\\""),
                    '\\' => out.push_str("\\\\"),
                    '\n' => out.push_str("\\n"),
                    '\r' => out.push_str("\\r"),
                    '\t' => out.push_str("\\t"),
                    '\u{08}' => out.push_str("\\b"),
                    '\u{0c}' => out.push_str("\\f"),
                    c if (c as u32) < 0x20 => out.push_str(&format!("\\u{:04x}", c as u32)),
                    c => out.push(c),
                }
            }
            out.push('"');
        }
        Value::Array(a) => {
            out.push('[');
            for (i, e) in a.iter().enumerate() {
                if i > 0 {
                    out.push_str(", ");
                }
                write_json(e, out);
            }
            out.push(']');
        }
        Value::Object(o) => {
            out.push('{');
            for (i, (k, e)) in o.iter().enumerate() {
                if i > 0 {
                    out.push_str(", ");
                }
                write_json(&Value::String(k.clone()), out);
                out.push_str(": ");
                write_json(e, out);
            }
            out.push('}');
        }
    }
}

fn serialize_state(state: &Value) -> String {
    match state {
        Value::String(s) => s.clone(),
        other => json_dumps_python(other),
    }
}

// ─── question model ────────────────────────────────────────────────

#[derive(Clone, Copy, PartialEq)]
pub enum QType {
    Choice,
    Score,
    Noul,
}

impl QType {
    fn as_str(self) -> &'static str {
        match self {
            QType::Choice => "choice",
            QType::Score => "score",
            QType::Noul => "noul",
        }
    }
    fn index(self) -> i64 {
        match self {
            QType::Choice => 0,
            QType::Score => 1,
            QType::Noul => 2,
        }
    }
}

pub struct InternalQ {
    pub t: QType,
    pub ins: String,
    pub crit: Option<Value>,
}

fn truthy(v: &Value) -> bool {
    match v {
        Value::Null => false,
        Value::Bool(b) => *b,
        Value::Number(n) => n.as_f64().map(|x| x != 0.0).unwrap_or(false),
        Value::String(s) => !s.is_empty(),
        Value::Array(a) => !a.is_empty(),
        Value::Object(o) => !o.is_empty(),
    }
}

/// Approximates Python's `str(value)` used by the `"%s"` formatting in `rl_common`.
fn py_str(v: &Value) -> String {
    match v {
        Value::Null => "None".to_string(),
        Value::Bool(b) => if *b { "True" } else { "False" }.to_string(),
        Value::Number(n) => n.to_string(),
        Value::String(s) => s.clone(),
        Value::Array(a) => format!(
            "[{}]",
            a.iter().map(py_str).collect::<Vec<_>>().join(", ")
        ),
        Value::Object(_) => json_dumps_python(v),
    }
}

/// Mirrors `RLAgent._to_internal`.
fn to_internal(qdef: &Value) -> Result<InternalQ> {
    let obj = qdef.as_object().ok_or_else(|| anyhow!("question must be an object"))?;
    let t = match obj.get("type").and_then(|v| v.as_str()) {
        Some("choice") => QType::Choice,
        Some("score") => QType::Score,
        Some("noul") => QType::Noul,
        Some(other) => bail!("unknown question type {other}"),
        None => bail!("question missing 'type'"),
    };
    let ins_v = obj.get("instructions").ok_or_else(|| anyhow!("question missing 'instructions'"))?;
    let ins = match ins_v {
        Value::String(s) => s.clone(),
        other => json_dumps_python(other),
    };
    let mut crit = obj.get("criteria").cloned();
    if t == QType::Choice {
        if let Some(Value::Array(a)) = &crit {
            let mut m = Map::new();
            for c in a {
                m.insert(c.as_str().unwrap_or("").to_string(), Value::Null);
            }
            crit = Some(Value::Object(m));
        }
    }
    Ok(InternalQ { t, ins, crit })
}

/// Mirrors `rl_common.render_options`.
fn render_options(q: &InternalQ) -> Vec<String> {
    match q.t {
        QType::Choice => {
            let mut out = Vec::new();
            if let Some(Value::Object(m)) = &q.crit {
                for (k, v) in m {
                    if truthy(v) {
                        out.push(format!("{}: {}", k, py_str(v)));
                    } else {
                        out.push(k.clone());
                    }
                }
            }
            out
        }
        QType::Score => {
            let mut out = Vec::new();
            if let Some(Value::Array(a)) = &q.crit {
                for (i, c) in a.iter().enumerate() {
                    out.push(format!("level {}: {}", i, c.as_str().unwrap_or("")));
                }
            }
            out
        }
        QType::Noul => {
            let get = |key: &str, dflt: &str| -> String {
                q.crit
                    .as_ref()
                    .and_then(|c| c.as_object())
                    .and_then(|m| m.get(key))
                    .and_then(|v| v.as_str())
                    .filter(|s| !s.is_empty())
                    .unwrap_or(dflt)
                    .to_string()
            };
            vec![
                format!("false: {}", get("false", "no, the statement does not hold")),
                format!("true: {}", get("true", "yes, the statement holds")),
            ]
        }
    }
}

/// Mirrors `rl_common.build_sequence`.
fn build_sequence(
    tok: &Tokenizer,
    state: &Value,
    q: &InternalQ,
    max_len: usize,
    head_max_len: usize,
) -> Result<(Vec<i64>, Vec<i64>)> {
    let cls_id = tok.token_to_id("[CLS]").unwrap_or(CLS_ID) as i64;
    let sep_id = tok.token_to_id("[SEP]").unwrap_or(SEP_ID) as i64;
    let mask_id = tok.token_to_id("[MASK]").unwrap_or(MASK_ID) as i64;
    let mask_tok = tok.id_to_token(mask_id as u32).unwrap_or_else(|| "[MASK]".to_string());

    let encode = |text: &str| -> Result<Vec<i64>> {
        Ok(tok
            .encode(text, false)
            .map_err(|e| anyhow!("encode: {e}"))?
            .get_ids()
            .iter()
            .map(|&t| t as i64)
            .collect())
    };

    let opts = render_options(q);
    let ins = q.ins.replace(&mask_tok, " ");
    let mut head_ids = encode(&format!("{} question: {}", q.t.as_str(), ins))?;

    let mut opt_ids: Vec<Vec<i64>> = Vec::with_capacity(opts.len());
    for opt in &opts {
        let mut o = vec![mask_id];
        let mut t = encode(&format!(" {}", opt.replace(&mask_tok, " ")))?;
        t.truncate(48);
        o.extend(t);
        opt_ids.push(o);
    }

    let sum = |v: &Vec<Vec<i64>>| v.iter().map(|x| x.len()).sum::<usize>();
    let mut opt_budget = head_max_len as i64 - sum(&opt_ids) as i64;
    if opt_budget < 16 {
        let per = std::cmp::max(4, (head_max_len as i64 - 16) / std::cmp::max(1, opt_ids.len() as i64));
        for o in opt_ids.iter_mut() {
            o.truncate(per as usize);
        }
        opt_budget = head_max_len as i64 - sum(&opt_ids) as i64;
    }
    head_ids.truncate(std::cmp::max(8, opt_budget) as usize);

    let mut ids = vec![cls_id];
    ids.extend_from_slice(&head_ids);
    ids.push(sep_id);
    let mut markers = Vec::new();
    for o in &opt_ids {
        markers.push(ids.len() as i64);
        ids.extend_from_slice(o);
    }
    ids.push(sep_id);

    let room = max_len.saturating_sub(ids.len() + 1);
    let st_text = serialize_state(state).replace(&mask_tok, " ");
    let mut st = encode(&st_text)?;
    st.truncate(room);
    ids.extend_from_slice(&st);
    ids.push(sep_id);
    ids.truncate(max_len);
    let markers: Vec<i64> = markers.into_iter().filter(|&m| (m as usize) < max_len).collect();
    Ok((ids, markers))
}

fn temp_bucket(qt: QType, k: usize) -> String {
    let size = if k <= 2 {
        "2"
    } else if k <= 5 {
        "3-5"
    } else if k <= 10 {
        "6-10"
    } else {
        "11+"
    };
    format!("{}:{}", qt.as_str(), size)
}

fn temperature_for(qt: QType, k: usize) -> f64 {
    let bucket = temp_bucket(qt, k);
    let overrides: &[(&str, f64)] = &[
        ("choice:2", 1.9063563346862793),
        ("choice:3-5", 1.7601518630981445),
        ("choice:6-10", 1.0000158548355103),
        ("choice:11+", 0.10058280825614929),
        ("score:3-5", 1.2514300346374512),
        ("noul:2", 1.983399510383606),
    ];
    for (k2, v) in overrides {
        if *k2 == bucket {
            return *v;
        }
    }
    TEMPERATURE[qt.index() as usize]
}

/// Softmax in f32 — the reference (`rl_agent_api.system_one`) computes
/// `np.exp(z - z.max()) / sum` in float32, so we match its numerics bit-for-bit.
fn softmax_f32(z: &[f32]) -> Vec<f32> {
    let m = z.iter().cloned().fold(f32::NEG_INFINITY, f32::max);
    let e: Vec<f32> = z.iter().map(|v| (v - m).exp()).collect();
    let s: f32 = e.iter().sum();
    e.into_iter().map(|v| v / s).collect()
}

fn confidence_from_probs(p: &[f32], k: usize) -> f32 {
    if k < 2 {
        return 1.0;
    }
    let ent: f32 = p.iter().map(|&x| -x * x.clamp(1e-12, 1.0).ln()).sum();
    1.0 - ent / (k as f32).ln()
}

/// Round to 4 decimals the way Python's `round(x, 4)` does (half-to-even on the
/// shortest decimal), so probabilities match the reference exactly.
fn r4(x: f64) -> f64 {
    format!("{:.4}", x).parse::<f64>().unwrap_or(x)
}

// ─── engine ────────────────────────────────────────────────────────

struct Engine {
    tok: Tokenizer,
    model: LayaModel,
}

impl Engine {
    fn load(model_dir: &str) -> Result<Self> {
        let tok_path = if std::path::Path::new(&format!("{model_dir}/tokenizer.json")).exists() {
            format!("{model_dir}/tokenizer.json")
        } else {
            format!("{model_dir}/tokenizer/tokenizer.json")
        };
        let tok = Tokenizer::from_file(&tok_path).map_err(|e| anyhow!("tokenizer: {e}"))?;
        let model = LayaModel::load(model_dir)?;
        Ok(Self { tok, model })
    }

    fn predict(&self, state: &Value, questions: &Map<String, Value>) -> Result<(Value, usize)> {
        self.predict_impl(state, questions, false)
    }

    /// Same as `predict` but includes raw (unrounded) probabilities for debugging
    /// mismatches against the PyTorch reference.
    fn predict_debug(&self, state: &Value, questions: &Map<String, Value>) -> Result<(Value, usize)> {
        self.predict_impl(state, questions, true)
    }

    fn predict_impl(
        &self,
        state: &Value,
        questions: &Map<String, Value>,
        debug: bool,
    ) -> Result<(Value, usize)> {
        let mut id_store: Vec<Vec<i64>> = Vec::new();
        let mut marker_store: Vec<Vec<i64>> = Vec::new();
        let mut metas: Vec<(String, InternalQ, usize, QType)> = Vec::new();

        for (qid, qdef) in questions {
            let q = to_internal(qdef)?;
            let (ids, markers) = build_sequence(&self.tok, state, &q, MAX_LEN, HEAD_MAX_LEN)?;
            let k = render_options(&q).len();
            if markers.len() != k {
                bail!("question {qid:?}: options do not fit in head_max_len={HEAD_MAX_LEN} tokens");
            }
            id_store.push(ids);
            marker_store.push(markers);
            let qt = q.t;
            metas.push((qid.clone(), q, k, qt));
        }
        if id_store.is_empty() {
            return Ok((json!({ "model": "rl-agent", "answers": {}, "usage": {"input_tokens": 0, "output_tokens": 0} }), 0));
        }

        let seqs: Vec<SeqInput> = (0..id_store.len())
            .map(|i| SeqInput {
                ids: &id_store[i],
                markers: &marker_store[i],
                qtype: metas[i].3.index(),
            })
            .collect();
        let n_tokens: usize = id_store.iter().map(|v| v.len()).sum();
        let outs = self.model.forward_batch(&seqs)?;

        let mut answers = Map::new();
        for (i, (qid, q, k, qt)) in metas.iter().enumerate() {
            let logits = &outs[i].logits;
            let temp = temperature_for(*qt, *k) as f32;
            let z: Vec<f32> = logits.iter().map(|&v| v / temp).collect();
            let p = softmax_f32(&z);
            let conf = r4(confidence_from_probs(&p, *k) as f64);
            // act probability = softmax(act_logits)[0], computed in f32 like torch
            let am = outs[i].act_logits.iter().cloned().fold(f32::NEG_INFINITY, f32::max);
            let ae: Vec<f32> = outs[i].act_logits.iter().map(|&v| (v - am).exp()).collect();
            let act_probability = (ae[0] / ae.iter().sum::<f32>()) as f64;

            if debug {
                eprintln!(
                    "[dbg] qid={} k={} temp={} logits={:?} raw_p={:?} raw_act={}",
                    qid, k, temp,
                    logits.iter().map(|v| *v as f64).collect::<Vec<_>>(),
                    p.iter().map(|v| *v as f64).collect::<Vec<_>>(),
                    act_probability
                );
            }

            let ans = match q.t {
                QType::Choice => {
                    let keys: Vec<String> = match &q.crit {
                        Some(Value::Object(m)) => m.keys().cloned().collect(),
                        _ => (0..*k).map(|i| i.to_string()).collect(),
                    };
                    let best = p
                        .iter()
                        .enumerate()
                        .max_by(|a, b| a.1.partial_cmp(b.1).unwrap_or(std::cmp::Ordering::Equal))
                        .map(|(i, _)| i)
                        .unwrap_or(0);
                    let probs: Map<String, Value> = keys
                        .iter()
                        .zip(p.iter())
                        .map(|(kk, vv)| (kk.clone(), json!(r4(*vv as f64))))
                        .collect();
                    json!({
                        "type": "choice",
                        "choice": keys.get(best).cloned().unwrap_or_default(),
                        "probabilities": Value::Object(probs),
                        "confidence": conf,
                        "rl_agent": {"act_probability": act_probability},
                    })
                }
                QType::Score => {
                    let score: f64 = p.iter().enumerate().map(|(i, &pi)| pi as f64 * i as f64).sum();
                    let legend: Map<String, Value> = match &q.crit {
                        Some(Value::Array(a)) => a
                            .iter()
                            .enumerate()
                            .map(|(i, c)| (i.to_string(), c.clone()))
                            .collect(),
                        _ => Map::new(),
                    };
                    let probs: Map<String, Value> = p
                        .iter()
                        .enumerate()
                        .map(|(i, &pi)| (i.to_string(), json!(r4(pi as f64))))
                        .collect();
                    json!({
                        "type": "score",
                        "score": r4(score),
                        "legend": Value::Object(legend),
                        "probabilities": Value::Object(probs),
                        "confidence": conf,
                        "rl_agent": {"act_probability": act_probability},
                    })
                }
                QType::Noul => json!({
                    "type": "noul",
                    "noul": r4(if p.len() >= 2 { p[1] as f64 } else { p[0] as f64 }),
                    "rl_agent": {"act_probability": act_probability},
                }),
            };
            answers.insert(qid.clone(), ans);
        }

        Ok((
            json!({
                "model": "rl-agent",
                "answers": Value::Object(answers),
                "usage": {"input_tokens": n_tokens, "output_tokens": 0},
            }),
            n_tokens,
        ))
    }
}

// ─── HTTP ──────────────────────────────────────────────────────────

async fn system_one(
    axum::extract::State(state): axum::extract::State<Arc<Mutex<Engine>>>,
    axum::extract::Json(req): axum::extract::Json<Value>,
) -> Result<axum::Json<Value>, (axum::http::StatusCode, String)> {
    let t0 = Instant::now();
    let st = req.get("state").cloned().unwrap_or(Value::Null);
    let qs = req
        .get("questions")
        .and_then(|v| v.as_object())
        .cloned()
        .ok_or_else(|| (axum::http::StatusCode::BAD_REQUEST, "missing questions".to_string()))?;
    let engine = state.lock().map_err(|_| {
        (axum::http::StatusCode::INTERNAL_SERVER_ERROR, "engine lock poisoned".to_string())
    })?;
    let (resp, _n) = engine
        .predict(&st, &qs)
        .map_err(|e| (axum::http::StatusCode::INTERNAL_SERVER_ERROR, format!("{e}")))?;
    let ms = t0.elapsed().as_secs_f64() * 1000.0;
    eprintln!("[laya-tch] {ms:.0}ms  questions={}", qs.len());
    Ok(axum::Json(resp))
}

async fn health() -> &'static str {
    "ok"
}

#[tokio::main(flavor = "current_thread")]
async fn main() -> Result<()> {
    let cli = Cli::parse();
    let t0 = Instant::now();
    eprintln!("[laya-tch] loading model from {}", cli.model_dir);
    let engine = Engine::load(&cli.model_dir)?;
    eprintln!("[laya-tch] loaded in {:.1}s", t0.elapsed().as_secs_f64());

    if let Some(path) = &cli.once {
        let raw = std::fs::read_to_string(path)?;
        let req: Value = serde_json::from_str(&raw)?;
        let st = req.get("state").cloned().unwrap_or(Value::Null);
        let qs = req
            .get("questions")
            .and_then(|v| v.as_object())
            .cloned()
            .ok_or_else(|| anyhow!("missing questions"))?;
        if std::env::var("LAYA_DEBUG_PROBS").is_ok() {
            let (resp, _) = engine.predict_debug(&st, &qs)?;
            println!("{}", serde_json::to_string(&resp)?);
        } else {
            let (resp, _) = engine.predict(&st, &qs)?;
            println!("{}", serde_json::to_string(&resp)?);
        }
        return Ok(());
    }

    let app = axum::Router::new()
        .route("/v1/systemone", axum::routing::post(system_one))
        .route("/health", axum::routing::get(health))
        .with_state(Arc::new(Mutex::new(engine)));

    let addr = format!("{}:{}", cli.host, cli.port);
    println!("[laya-tch] Listening on http://{addr}");
    let listener = tokio::net::TcpListener::bind(&addr).await?;
    axum::serve(listener, app).await?;
    Ok(())
}
