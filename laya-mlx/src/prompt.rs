//! Prompt construction, tokenizer and calibration.
//!
//! A Rust port of `laya-tch/mlx/native/laya_mlx/{common.py,tokenizer.py}`. The
//! tokenisation must match byte-for-byte, so `py_json_dumps` reproduces Python's
//! `json.dumps` default separators (`", "` / `": "`) rather than serde_json's
//! compact form.

use std::path::Path;

use anyhow::{anyhow, Result};
use serde_json::Value;

pub const QTYPES: [(&str, i32); 3] = [("choice", 0), ("score", 1), ("noul", 2)];
pub const TEMP_MIN: f32 = 0.5;
pub const TEMP_MAX: f32 = 5.0;

pub fn qtype_id(t: &str) -> Option<i32> {
    QTYPES.iter().find(|(k, _)| *k == t).map(|(_, v)| *v)
}
pub fn qtype_name(id: i32) -> &'static str {
    match id {
        0 => "choice",
        1 => "score",
        _ => "noul",
    }
}

/// `json.dumps(v, ensure_ascii=False)` with Python's default separators.
pub fn py_json_dumps(v: &Value) -> String {
    let mut s = String::new();
    dump(v, &mut s);
    s
}

fn dump(v: &Value, out: &mut String) {
    match v {
        Value::Null => out.push_str("null"),
        Value::Bool(b) => out.push_str(if *b { "true" } else { "false" }),
        Value::Number(n) => out.push_str(&py_number(n)),
        Value::String(s) => out.push_str(&json_escape(s)),
        Value::Array(a) => {
            out.push('[');
            for (i, e) in a.iter().enumerate() {
                if i > 0 {
                    out.push_str(", ");
                }
                dump(e, out);
            }
            out.push(']');
        }
        Value::Object(m) => {
            out.push('{');
            for (i, (k, val)) in m.iter().enumerate() {
                if i > 0 {
                    out.push_str(", ");
                }
                out.push_str(&json_escape(k));
                out.push_str(": ");
                dump(val, out);
            }
            out.push('}');
        }
    }
}

fn py_number(n: &serde_json::Number) -> String {
    if let Some(i) = n.as_i64() {
        i.to_string()
    } else if let Some(u) = n.as_u64() {
        u.to_string()
    } else {
        let f = n.as_f64().unwrap_or(f64::NAN);
        if f.is_finite() && f.fract() == 0.0 {
            format!("{:.1}", f)
        } else {
            format!("{}", f)
        }
    }
}

fn json_escape(s: &str) -> String {
    let mut out = String::with_capacity(s.len() + 2);
    out.push('"');
    for c in s.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            c if (c as u32) < 0x20 => out.push_str(&format!("\\u{:04x}", c as u32)),
            c => out.push(c),
        }
    }
    out.push('"');
    out
}

pub fn serialize_state(state: &Value) -> String {
    match state {
        Value::String(s) => s.clone(),
        other => py_json_dumps(other),
    }
}

/// `render_criterion`: strings pass through, everything else becomes JSON.
pub fn render_criterion(v: &Value) -> String {
    match v {
        Value::String(s) => s.clone(),
        other => py_json_dumps(other),
    }
}

/// One internal question (`q["t"]`, `q["ins"]`, `q["crit"]`).
pub struct Q {
    pub t: String,
    pub qtype: i32,
    pub ins: String,
    pub crit: Value,
}

impl Q {
    pub fn from_json(def: &Value) -> Result<Self> {
        let t = def
            .get("type")
            .and_then(|v| v.as_str())
            .ok_or_else(|| anyhow!("question missing 'type'"))?
            .to_string();
        let qtype = qtype_id(&t).ok_or_else(|| anyhow!("unknown question type {t:?}"))?;
        let ins = match def.get("instructions") {
            Some(Value::String(s)) => s.clone(),
            Some(other) => py_json_dumps(other),
            None => return Err(anyhow!("question missing 'instructions'")),
        };
        let mut crit = def.get("criteria").cloned().unwrap_or(Value::Null);
        if t == "choice" {
            // Python `_to_internal` accepts a list of labels and turns it into a
            // dict preserving order (`dict.fromkeys(criteria)`).
            if let Value::Array(a) = &crit {
                let mut m = serde_json::Map::new();
                for c in a {
                    if let Some(s) = c.as_str() {
                        m.insert(s.to_string(), Value::Null);
                    }
                }
                crit = Value::Object(m);
            }
        }
        Ok(Self { t, qtype, ins, crit })
    }
}

/// `render_options`: option texts in label-index order.
pub fn render_options(q: &Q) -> Vec<String> {
    match q.t.as_str() {
        "choice" => {
            let mut out = Vec::new();
            // Preserve JSON insertion order (serde_json `preserve_order`), matching
            // Python's `dict.fromkeys(criteria)` / `crit.items()`.
            if let Value::Object(m) = &q.crit {
                for (k, v) in m {
                    if v.is_null() || v.as_str() == Some("") {
                        out.push(k.clone());
                    } else {
                        out.push(format!("{}: {}", k, render_criterion(v)));
                    }
                }
            }
            out
        }
        "score" => match &q.crit {
            Value::Array(a) => a
                .iter()
                .enumerate()
                .map(|(i, c)| format!("level {i}: {}", render_criterion(c)))
                .collect(),
            _ => Vec::new(),
        },
        _ => {
            let (mut f, mut t) = (None, None);
            if let Value::Object(m) = &q.crit {
                f = m.get("false").cloned();
                t = m.get("true").cloned();
            }
            let f = match f {
                Some(v) if !(v.is_null() || v.as_str() == Some("")) => render_criterion(&v),
                _ => "no, the statement does not hold".to_string(),
            };
            let t = match t {
                Some(v) if !(v.is_null() || v.as_str() == Some("")) => render_criterion(&v),
                _ => "yes, the statement holds".to_string(),
            };
            vec![format!("false: {f}"), format!("true: {t}")]
        }
    }
}

pub struct Tok {
    inner: tokenizers::Tokenizer,
    pub cls_id: u32,
    pub sep_id: u32,
    pub pad_id: u32,
    pub mask_id: u32,
    pub mask_token: String,
}

impl Tok {
    pub fn load(dir: &Path) -> Result<Self> {
        let inner = tokenizers::Tokenizer::from_file(dir.join("tokenizer.json"))
            .map_err(|e| anyhow!("tokenizer: {e}"))?;
        let cfg: Value = serde_json::from_str(&std::fs::read_to_string(dir.join("tokenizer_config.json"))?)?;
        let tok = |k: &str| -> Option<String> {
            cfg.get(k).and_then(|v| {
                if let Some(c) = v.get("content") {
                    c.as_str().map(String::from)
                } else {
                    v.as_str().map(String::from)
                }
            })
        };
        let cls_token = tok("cls_token").ok_or_else(|| anyhow!("no cls_token"))?;
        let sep_token = tok("sep_token").ok_or_else(|| anyhow!("no sep_token"))?;
        let pad_token = tok("pad_token").ok_or_else(|| anyhow!("no pad_token"))?;
        let mask_token = tok("mask_token").ok_or_else(|| anyhow!("no mask_token"))?;
        let id = |s: &str| -> Result<u32> {
            inner.token_to_id(s).ok_or_else(|| anyhow!("token {s:?} not in vocab"))
        };
        Ok(Self {
            cls_id: id(&cls_token)?,
            sep_id: id(&sep_token)?,
            pad_id: id(&pad_token)?,
            mask_id: id(&mask_token)?,
            mask_token,
            inner,
        })
    }

    pub fn encode(&self, text: &str, add_special: bool) -> Result<Vec<u32>> {
        Ok(self
            .inner
            .encode(text, add_special)
            .map_err(|e| anyhow!("encode: {e}"))?
            .get_ids()
            .to_vec())
    }
}

/// `build_prefix`: `[CLS] <type> question: <ins> [SEP] [MASK] opt [MASK] opt ... [SEP]`.
pub fn build_prefix(tok: &Tok, q: &Q, head_max_len: usize) -> Result<(Vec<u32>, Vec<usize>)> {
    let opts = render_options(q);
    let ins = q.ins.replace(&tok.mask_token, " ");
    let head_text = format!("{} question: {}", q.t, ins);
    let mut head_ids = tok.encode(&head_text, false)?;

    let mut opt_ids: Vec<Vec<u32>> = Vec::new();
    for o in &opts {
        let body = o.replace(&tok.mask_token, " ");
        let mut ids = vec![tok.mask_id];
        let body = tok.encode(&format!(" {body}"), false)?;
        ids.extend_from_slice(&body[..body.len().min(48)]);
        opt_ids.push(ids);
    }
    let sum = |v: &Vec<Vec<u32>>| v.iter().map(|x| x.len()).sum::<usize>();
    let mut opt_budget = head_max_len.saturating_sub(sum(&opt_ids));
    if opt_budget < 16 {
        let per = std::cmp::max(4, (head_max_len.saturating_sub(16)) / std::cmp::max(1, opt_ids.len()));
        for o in opt_ids.iter_mut() {
            o.truncate(per);
        }
        opt_budget = head_max_len.saturating_sub(sum(&opt_ids));
    }
    head_ids.truncate(std::cmp::max(8, opt_budget));

    let mut ids = vec![tok.cls_id];
    ids.extend_from_slice(&head_ids);
    ids.push(tok.sep_id);
    let mut markers = Vec::new();
    for o in &opt_ids {
        markers.push(ids.len());
        ids.extend_from_slice(o);
    }
    ids.push(tok.sep_id);
    Ok((ids, markers))
}

/// `build_sequence`: prefix + (state tokens) + `[SEP]`, truncated to `max_len`.
pub fn build_sequence(
    tok: &Tok,
    state: &Value,
    q: &Q,
    max_len: usize,
    head_max_len: usize,
) -> Result<(Vec<u32>, Vec<usize>)> {
    let (mut ids, markers) = build_prefix(tok, q, head_max_len)?;
    let room = max_len.saturating_sub(ids.len() + 1);
    let st_text = serialize_state(state).replace(&tok.mask_token, " ");
    let mut st = tok.encode(&st_text, false)?;
    st.truncate(room);
    ids.extend_from_slice(&st);
    ids.push(tok.sep_id);
    ids.truncate(max_len);
    let markers = markers.into_iter().filter(|m| *m < max_len).collect();
    Ok((ids, markers))
}

// ── calibration ─────────────────────────────────────────────────────────────

pub fn confidence_from_probs(p: &[f32]) -> f32 {
    let k = p.len();
    if k < 2 {
        return 1.0;
    }
    let ent: f32 = p.iter().map(|x| -x * x.max(1e-12).ln()).sum();
    (1.0 - ent / (k as f32).ln()).clamp(0.0, 1.0)
}

pub fn temp_bucket(qtype: i32, k: usize) -> String {
    let size = if k <= 2 {
        "2"
    } else if k <= 5 {
        "3-5"
    } else if k <= 10 {
        "6-10"
    } else {
        "11+"
    };
    format!("{}:{}", qtype_name(qtype), size)
}

pub fn clamp_temperature(t: f32) -> f32 {
    if !t.is_finite() {
        return 1.0;
    }
    t.clamp(TEMP_MIN, TEMP_MAX)
}

pub fn softmax(v: &[f32]) -> Vec<f32> {
    let max = v.iter().cloned().fold(f32::NEG_INFINITY, f32::max);
    let exps: Vec<f32> = v.iter().map(|x| (x - max).exp()).collect();
    let sum: f32 = exps.iter().sum();
    exps.into_iter().map(|e| e / sum).collect()
}
