//! Native MLX (Rust) inference runtime for the Laya decision model.
//!
//! Loads the FP16 MLX checkpoint (`model.safetensors` + encoder/agent configs +
//! tokenizer) and runs whole-request inference, mirroring
//! `laya-tch/mlx/native/laya_mlx/runtime.py` (`Agent.system_one`).

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use anyhow::{anyhow, Result};
use half::f16;
use mlx_rs::{Array, Dtype};
use serde_json::{json, Map, Value};

use crate::model::DecisionModel;
use crate::prompt::{
    build_sequence, clamp_temperature, confidence_from_probs, softmax, temp_bucket, Q, Tok,
};

pub struct Agent {
    model: DecisionModel,
    tok: Tok,
    max_len: usize,
    head_max_len: usize,
    temperature: Vec<f32>,
    temperature_by_options: HashMap<String, f32>,
}

fn sanitize(name: &str) -> String {
    let mut n = name.to_string();
    if n.contains(".self_attn.in_proj_weight") {
        n = n.replace(".self_attn.in_proj_weight", ".self_attn.in_proj.weight");
    }
    if n.contains(".self_attn.in_proj_bias") {
        n = n.replace(".self_attn.in_proj_bias", ".self_attn.in_proj.bias");
    }
    if n.starts_with("scorer.") && !n.starts_with("scorer.layers.") {
        n = format!("scorer.layers.{}", &n["scorer.".len()..]);
    }
    if n.starts_with("act_head.") && !n.starts_with("act_head.layers.") {
        n = format!("act_head.layers.{}", &n["act_head.".len()..]);
    }
    n
}

fn load_safetensors(path: &Path) -> Result<HashMap<String, Array>> {
    let data = std::fs::read(path)?;
    let st = safetensors::SafeTensors::deserialize(&data)?;
    let mut out = HashMap::new();
    for name in st.names() {
        let tv = st.tensor(name)?;
        let shape: Vec<i32> = tv.shape().iter().map(|&x| x as i32).collect();
        let raw = tv.data();
        let vals: Vec<f32> = match tv.dtype() {
            safetensors::Dtype::F16 => raw
                .chunks_exact(2)
                .map(|b| f16::from_bits(u16::from_le_bytes([b[0], b[1]])).to_f32())
                .collect(),
            safetensors::Dtype::BF16 => raw
                .chunks_exact(2)
                .map(|b| half::bf16::from_bits(u16::from_le_bytes([b[0], b[1]])).to_f32())
                .collect(),
            safetensors::Dtype::F32 => raw
                .chunks_exact(4)
                .map(|b| f32::from_le_bytes([b[0], b[1], b[2], b[3]]))
                .collect(),
            other => return Err(anyhow!("unsupported dtype {other:?} for {name}")),
        };
        out.insert(sanitize(name), Array::from_slice(&vals, &shape));
    }
    Ok(out)
}

fn read_json(path: &Path) -> Result<Value> {
    Ok(serde_json::from_str(&std::fs::read_to_string(path)?)?)
}

impl Agent {
    pub fn load(model_dir: &Path) -> Result<Self> {
        let agent_cfg = read_json(&model_dir.join("rl_agent_config.json"))?;
        let enc_cfg = read_json(&model_dir.join("encoder/config.json"))?;
        let local_window = enc_cfg
            .get("local_attention")
            .and_then(|v| v.as_i64())
            .unwrap_or(128) as i32;
        let head_layers = agent_cfg
            .get("head_layers")
            .and_then(|v| v.as_u64())
            .unwrap_or(2) as usize;
        let max_len = agent_cfg.get("max_len").and_then(|v| v.as_u64()).unwrap_or(512) as usize;
        let head_max_len = agent_cfg
            .get("head_max_len")
            .and_then(|v| v.as_u64())
            .unwrap_or(192) as usize;

        let mut temperature: Vec<f32> = agent_cfg
            .get("temperature")
            .and_then(|v| v.as_array())
            .map(|a| a.iter().map(|x| x.as_f64().unwrap_or(1.0) as f32).collect())
            .unwrap_or_else(|| vec![1.0, 1.0, 1.0]);
        if temperature.len() != 3 {
            temperature = vec![1.0, 1.0, 1.0];
        }
        let temperature: Vec<f32> = temperature.into_iter().map(clamp_temperature).collect();
        let temperature_by_options = agent_cfg
            .get("temperature_by_options")
            .and_then(|v| v.as_object())
            .map(|m| {
                m.iter()
                    .map(|(k, v)| (k.clone(), clamp_temperature(v.as_f64().unwrap_or(1.0) as f32)))
                    .collect()
            })
            .unwrap_or_default();

        let weights = load_safetensors(&model_dir.join("model.safetensors"))?;
        let model = DecisionModel::load(weights, local_window, head_layers)?;
        let tok = Tok::load(&model_dir.join("tokenizer"))?;
        Ok(Self { model, tok, max_len, head_max_len, temperature, temperature_by_options })
    }

    fn scale_for(&self, qtype: i32, k: usize) -> f32 {
        *self
            .temperature_by_options
            .get(&temp_bucket(qtype, k))
            .unwrap_or(&self.temperature[qtype as usize])
    }

    pub fn system_one(&self, state: &Value, questions: &Value) -> Result<Value> {
        let qmap = questions
            .as_object()
            .ok_or_else(|| anyhow!("questions must be an object"))?;

        let mut ids_all: Vec<Vec<u32>> = Vec::new();
        let mut markers_all: Vec<Vec<usize>> = Vec::new();
        let mut qtypes: Vec<i32> = Vec::new();
        let mut parsed: Vec<Q> = Vec::new();
        for (_qid, def) in qmap.iter() {
            let q = Q::from_json(def)?;
            let (ids, markers) = build_sequence(&self.tok, state, &q, self.max_len, self.head_max_len)?;
            ids_all.push(ids);
            markers_all.push(markers);
            qtypes.push(q.qtype);
            parsed.push(q);
        }

        let n = ids_all.len();
        let length = ids_all.iter().map(|v| v.len()).max().unwrap_or(0);
        let count = markers_all.iter().map(|v| v.len()).max().unwrap_or(0).max(2);

        let mut input_ids = vec![self.tok.pad_id as i32; n * length];
        let mut attn = vec![0i32; n * length];
        let mut marker_pos = vec![0i32; n * count];
        let mut marker_mask = vec![0i32; n * count];
        let mut input_tokens = 0usize;
        for i in 0..n {
            input_tokens += ids_all[i].len();
            for (j, &v) in ids_all[i].iter().enumerate() {
                input_ids[i * length + j] = v as i32;
                attn[i * length + j] = 1;
            }
            for (j, &m) in markers_all[i].iter().enumerate() {
                marker_pos[i * count + j] = m as i32;
                marker_mask[i * count + j] = 1;
            }
        }

        let input_ids = Array::from_slice(&input_ids, &[n as i32, length as i32]);
        let attention_mask = Array::from_slice(&attn, &[n as i32, length as i32]).as_dtype(Dtype::Bool)?;
        let marker_pos = Array::from_slice(&marker_pos, &[n as i32, count as i32]);
        let marker_mask = Array::from_slice(&marker_mask, &[n as i32, count as i32]).as_dtype(Dtype::Bool)?;
        let qtype = Array::from_slice(&qtypes, &[n as i32]);

        let t0 = std::time::Instant::now();
        let (logits, action) =
            self.model.forward(&input_ids, &attention_mask, &marker_pos, &marker_mask, &qtype)?;
        let build_ms = t0.elapsed().as_secs_f64() * 1000.0;
        logits.eval()?;
        action.eval()?;
        if std::env::var("LAYA_MLX_TIMING").is_ok() {
            let total = t0.elapsed().as_secs_f64() * 1000.0;
            eprintln!("[timing] build={build_ms:.1} ms eval={:.1} ms", total - build_ms);
        }
        let logits: Vec<f32> = logits.to_vec_cast::<f32>()?;
        let action: Vec<f32> = action.to_vec_cast::<f32>()?;
        let n_actions = if n > 0 { action.len() / n } else { 0 };

        let mut answers = Map::new();
        for (i, (qid, _)) in qmap.iter().enumerate() {
            let q = &parsed[i];
            let k = markers_all[i].len();
            let row = &logits[i * count..i * count + k];
            let scale = self.scale_for(q.qtype, k);
            let z: Vec<f32> = row.iter().map(|x| x / scale).collect();
            let p = softmax(&z);
            let act = softmax(&action[i * n_actions..(i + 1) * n_actions]);
            let act_prob = round4(act[0]);

            let mut ans = Map::new();
            ans.insert("type".into(), json!(q.t));
            match q.t.as_str() {
                "choice" => {
                    let labels: Vec<String> = match &q.crit {
                        Value::Object(m) => m.keys().cloned().collect(),
                        _ => vec![],
                    };
                    let mut prob = Map::new();
                    for (l, v) in labels.iter().zip(p.iter()) {
                        prob.insert(l.clone(), json!(round4(*v)));
                    }
                    let best = p
                        .iter()
                        .enumerate()
                        .max_by(|a, b| a.1.partial_cmp(b.1).unwrap())
                        .map(|(idx, _)| idx)
                        .unwrap_or(0);
                    ans.insert("confidence".into(), json!(round4(confidence_from_probs(&p))));
                    ans.insert("action".into(), json!({"act_probability": act_prob}));
                    ans.insert("choice".into(), json!(labels.get(best).cloned().unwrap_or_default()));
                    ans.insert("probabilities".into(), Value::Object(prob));
                }
                "score" => {
                    let mut prob = Map::new();
                    let mut score = 0.0f32;
                    for (idx, v) in p.iter().enumerate() {
                        score += idx as f32 * v;
                        prob.insert(idx.to_string(), json!(round4(*v)));
                    }
                    let legend: Map<String, Value> = match &q.crit {
                        Value::Array(a) => a
                            .iter()
                            .enumerate()
                            .map(|(i, c)| (i.to_string(), c.clone()))
                            .collect(),
                        _ => Map::new(),
                    };
                    ans.insert("confidence".into(), json!(round4(confidence_from_probs(&p))));
                    ans.insert("action".into(), json!({"act_probability": act_prob}));
                    ans.insert("score".into(), json!(round4(score)));
                    ans.insert("legend".into(), Value::Object(legend));
                    ans.insert("probabilities".into(), Value::Object(prob));
                }
                _ => {
                    let p1 = *p.get(1).unwrap_or(&0.0);
                    ans.insert("action".into(), json!({"act_probability": act_prob}));
                    ans.insert("noul".into(), json!(round4(p1)));
                    ans.insert("confidence".into(), json!(round4(p1.max(1.0 - p1))));
                }
            }
            answers.insert(qid.clone(), Value::Object(ans));
        }

        Ok(json!({
            "model": "laya-rl-agent",
            "answers": Value::Object(answers),
            "usage": {"input_tokens": input_tokens, "output_tokens": 0},
        }))
    }
}

fn round4(x: f32) -> f64 {
    ((x as f64) * 1e4).round() / 1e4
}

/// Resolve a checkpoint directory: `$LAYA_MLX_MODEL_DIR` / `$LAYA_MODEL_DIR` /
/// the local Hugging Face cache (`models--*laya-mlx*/snapshots/*`).
pub fn resolve_model_dir(explicit: Option<&str>) -> Result<PathBuf> {
    if let Some(p) = explicit {
        let p = expand(p);
        if usable(&p) {
            return Ok(p);
        }
        return Err(anyhow!("{} has no model.safetensors", p.display()));
    }
    for var in ["LAYA_MLX_MODEL_DIR", "LAYA_MODEL_DIR"] {
        if let Ok(v) = std::env::var(var) {
            let p = expand(&v);
            if usable(&p) {
                return Ok(p);
            }
        }
    }
    let hub = std::env::var("HF_HOME")
        .map(|h| expand(&format!("{h}/hub")))
        .unwrap_or_else(|_| expand("~/.cache/huggingface/hub"));
    if let Ok(entries) = std::fs::read_dir(&hub) {
        let mut cands: Vec<PathBuf> = Vec::new();
        for e in entries.flatten() {
            let name = e.file_name().to_string_lossy().to_string();
            if name.contains("laya-mlx") {
                let snaps = e.path().join("snapshots");
                if let Ok(s) = std::fs::read_dir(&snaps) {
                    for snap in s.flatten() {
                        if usable(&snap.path()) {
                            cands.push(snap.path());
                        }
                    }
                }
            }
        }
        cands.sort();
        if let Some(p) = cands.pop() {
            return Ok(p);
        }
    }
    Err(anyhow!(
        "no MLX Laya checkpoint found; set $LAYA_MLX_MODEL_DIR or pass --model-dir"
    ))
}

fn usable(p: &Path) -> bool {
    p.join("model.safetensors").is_file()
}

fn expand(p: &str) -> PathBuf {
    if let Some(rest) = p.strip_prefix("~") {
        if let Ok(home) = std::env::var("HOME") {
            return PathBuf::from(format!("{home}{rest}"));
        }
    }
    PathBuf::from(p)
}
