//! Laya inference engine — tch-rs (libtorch / PyTorch C++ bindings).
//!
//! This is a from-scratch, numerically faithful re-implementation of the Laya
//! `DecisionModel` (ModernBERT-large encoder + 2-layer Transformer decision head
//! + marker scorer), matching the PyTorch reference in `rl_common.py`.
//!
//! Correctness-critical details (all verified against the PyTorch reference):
//!   * encoder LayerNorm: mean/variance over the LAST dim (weight only, eps=1e-5)
//!   * pre-attention norm is `Identity` for layer 0, LayerNorm for layers 1..27
//!   * RoPE on q/k with per-layer-type theta (full=160000, sliding=10000), head_dim=64
//!   * sliding attention band `|i-j| <= 65` (config.sliding_window=64, +1)
//!   * GeGLU MLP: `Wo(gelu(first_half) * second_half)`
//!   * decision head: `nn.TransformerEncoderLayer(norm_first=True, relu)` x2
//!   * scorer: LayerNorm(bias) -> Linear(bias) -> GELU -> Linear(bias)
//!
//! Performance: all weights are converted to f32 once at load and kept in a flat
//! struct (no per-forward HashMap copies / re-reads); RoPE tables and the sliding
//! mask are cached per sequence length.

use anyhow::{anyhow, Result};
use std::collections::HashMap;
use std::sync::Mutex;
use tch::{Device, Kind, Tensor};

pub const HIDDEN: i64 = 1024;
pub const HEADS: i64 = 16;
pub const HEAD_DIM: i64 = 64;
pub const N_LAYERS: usize = 28;
pub const INTERMEDIATE: i64 = 2624;
pub const N_HEAD_LAYERS: usize = 2;
const LN_EPS: f64 = 1e-5;
/// config.sliding_window (64) — inclusive bidirectional band used by HF's
/// `create_bidirectional_sliding_window_mask` (the `+1` in attention is only
/// for the flash-attention inclusive boundary and does not affect the mask).
const SLIDING_WINDOW: i64 = 64;
const ROPE_THETA_FULL: f64 = 160_000.0;
const ROPE_THETA_SLIDING: f64 = 10_000.0;

struct EncLayer {
    attn_norm: Option<Tensor>, // [HIDDEN] weight; None for layer 0 (Identity)
    wqkv: Tensor,              // [3*HIDDEN, HIDDEN]
    wo: Tensor,                // [HIDDEN, HIDDEN]
    mlp_norm: Tensor,          // [HIDDEN]
    wi: Tensor,                // [2*INTERMEDIATE, HIDDEN]
    mlp_wo: Tensor,            // [HIDDEN, INTERMEDIATE]
    is_full: bool,
}

struct HeadLayer {
    norm1_w: Tensor,
    norm1_b: Tensor,
    in_proj_w: Tensor, // [3*HIDDEN, HIDDEN]
    in_proj_b: Tensor, // [3*HIDDEN]
    out_proj_w: Tensor,
    out_proj_b: Tensor,
    norm2_w: Tensor,
    norm2_b: Tensor,
    lin1_w: Tensor, // [4*HIDDEN, HIDDEN]
    lin1_b: Tensor,
    lin2_w: Tensor, // [HIDDEN, 4*HIDDEN]
    lin2_b: Tensor,
}

pub struct LayaModel {
    tok_emb: Tensor,    // [V, HIDDEN]
    emb_norm_w: Tensor, // [HIDDEN]
    layers: Vec<EncLayer>,
    final_norm_w: Tensor,
    type_emb: Tensor, // [3, HIDDEN]
    head: Vec<HeadLayer>,
    scorer_ln_w: Tensor,
    scorer_ln_b: Tensor,
    scorer_l1_w: Tensor,
    scorer_l1_b: Tensor,
    scorer_l2_w: Tensor,
    scorer_l2_b: Tensor,
    act0_w: Tensor, // [256, 1028]
    act0_b: Tensor,
    act2_w: Tensor, // [2, 256]
    act2_b: Tensor,
    // cached RoPE tables keyed by (is_full, seq_len)
    rope_cache: Mutex<HashMap<(bool, i64), (Tensor, Tensor)>>,
    // cached sliding attention mask keyed by seq_len
    mask_cache: Mutex<HashMap<i64, Tensor>>,
    // cached sliding band mask `[1,1,L,L]` keyed by seq_len
    band_cache: Mutex<HashMap<i64, Tensor>>,
    pub n_layers_seen: usize,
    /// Device that all weight tensors live on (CPU or CUDA).
    pub device: Device,
}

// ── tensor helpers ────────────────────────────────────────────────────

fn f16_to_f32(bytes: &[u8]) -> f32 {
    let bits = u16::from_le_bytes([bytes[0], bytes[1]]);
    half_bits_to_f32(bits)
}

fn bf16_to_f32(bytes: &[u8]) -> f32 {
    let bits = u16::from_le_bytes([bytes[0], bytes[1]]);
    let sign = if (bits >> 15) & 1 == 1 {
        -1.0f32
    } else {
        1.0f32
    };
    let exp = ((bits >> 7) & 0xFF) as i32;
    let man = (bits & 0x7F) as u32;
    if exp == 0xFF {
        if man == 0 {
            f32::INFINITY * sign
        } else {
            f32::NAN
        }
    } else if exp == 0 {
        (man as f32 / 128.0) * 2.0f32.powi(-126) * sign
    } else {
        f32::from_bits((((bits as u32) & 0x8000) << 16) | ((exp as u32) << 23) | (man << 16))
    }
}

fn half_bits_to_f32(bits: u16) -> f32 {
    let sign = if (bits >> 15) & 1 == 1 {
        -1.0f32
    } else {
        1.0f32
    };
    let exp = ((bits >> 10) & 0x1F) as i32;
    let man = (bits & 0x3FF) as f32 / 1024.0;
    if exp == 0x1F {
        if man == 0.0 {
            f32::INFINITY * sign
        } else {
            f32::NAN
        }
    } else if exp == 0 {
        man * 2.0f32.powi(-14) * sign
    } else {
        (1.0 + man) * 2.0f32.powi(exp - 15) * sign
    }
}

/// Read a safetensors tensor into a contiguous f32 `Tensor` with `shape`.
fn load_tensor(st: &safetensors::SafeTensors, name: &str, device: Device) -> Result<Tensor> {
    let tv = st
        .tensor(name)
        .map_err(|e| anyhow!("missing tensor {}: {}", name, e))?;
    let shape: Vec<i64> = tv.shape().iter().map(|&s| s as i64).collect();
    let data = tv.data();
    let flat: Vec<f32> = match tv.dtype() {
        safetensors::Dtype::F16 => data.chunks_exact(2).map(f16_to_f32).collect(),
        safetensors::Dtype::BF16 => data.chunks_exact(2).map(bf16_to_f32).collect(),
        safetensors::Dtype::F32 => data
            .chunks_exact(4)
            .map(|b| f32::from_le_bytes([b[0], b[1], b[2], b[3]]))
            .collect(),
        other => return Err(anyhow!("unsupported dtype {:?} for {}", other, name)),
    };
    let t = Tensor::from_slice(&flat)
        .view(shape.as_slice())
        .to_device(device);
    Ok(t.contiguous())
}

/// `nn.LayerNorm(dim, eps, weight, bias)` over the last dimension, using ATen's
/// fused kernel (identical op to PyTorch's `nn.LayerNorm`).
fn layer_norm(x: &Tensor, w: &Tensor, b: Option<&Tensor>) -> Tensor {
    x.layer_norm(&[HIDDEN], Some(w), b, LN_EPS, false)
}

/// `rotate_half`: split last dim in two halves, return `[-x2, x1]`.
fn rotate_half(x: &Tensor) -> Tensor {
    let d = x.size()[x.dim() - 1];
    let x1 = x.narrow(-1, 0, d / 2);
    let x2 = x.narrow(-1, d / 2, d / 2);
    Tensor::cat(&[&x2.neg(), &x1], -1)
}

/// `[..., L, D]` q/k rope application; `cos`/`sin` are `[L, D]` or `[..., L, D]`.
fn apply_rope(x: &Tensor, cos: &Tensor, sin: &Tensor) -> Tensor {
    let cos = if cos.dim() == x.dim() - 1 {
        cos.unsqueeze(0)
    } else {
        cos.shallow_clone()
    };
    let sin = if sin.dim() == x.dim() - 1 {
        sin.unsqueeze(0)
    } else {
        sin.shallow_clone()
    };
    x * &cos + rotate_half(x) * &sin
}

fn rope_table(l: i64, theta: f64, device: Device) -> (Tensor, Tensor) {
    let half = (HEAD_DIM / 2) as usize;
    let mut inv_freq = vec![0f64; half];
    for (j, v) in inv_freq.iter_mut().enumerate() {
        *v = 1.0 / theta.powf((2.0 * j as f64) / HEAD_DIM as f64);
    }
    let mut cos = vec![0f32; (l as usize) * HEAD_DIM as usize];
    let mut sin = vec![0f32; (l as usize) * HEAD_DIM as usize];
    for p in 0..l as usize {
        for j in 0..half {
            let ang = (p as f64) * inv_freq[j];
            let (c, s) = (ang.cos() as f32, ang.sin() as f32);
            cos[p * HEAD_DIM as usize + j] = c;
            cos[p * HEAD_DIM as usize + half + j] = c;
            sin[p * HEAD_DIM as usize + j] = s;
            sin[p * HEAD_DIM as usize + half + j] = s;
        }
    }
    (
        Tensor::from_slice(&cos)
            .view([l, HEAD_DIM])
            .to_device(device),
        Tensor::from_slice(&sin)
            .view([l, HEAD_DIM])
            .to_device(device),
    )
}

fn sliding_mask(l: i64, device: Device) -> Tensor {
    let lus = l as usize;
    let mut m = vec![0f32; lus * lus];
    for i in 0..lus {
        for j in 0..lus {
            if (i as i64 - j as i64).abs() > SLIDING_WINDOW {
                m[i * lus + j] = f32::NEG_INFINITY;
            }
        }
    }
    Tensor::from_slice(&m).view([l, l]).to_device(device)
}

/// One sequence for the batched forward path.
pub struct SeqInput<'a> {
    pub ids: &'a [i64],
    pub markers: &'a [i64],
    pub qtype: i64,
}

/// Per-sequence model output.
pub struct Output {
    /// raw marker logits (length = number of markers)
    pub logits: Vec<f32>,
    /// raw act-head logits (length 2)
    pub act_logits: Vec<f32>,
}

/// Encoder mask `[B, 1, L, L]` is no longer materialized: full layers add only
/// the key-padding mask, sliding layers add the cached band mask plus key pad.

/// Decision-head key padding mask `[B, 1, 1, L]` (broadcast over heads & queries).
fn build_key_pad_mask(b: i64, l: i64, attn: &[f32], device: Device) -> Tensor {
    let lus = l as usize;
    let mut m = vec![0f32; (b as usize) * lus];
    for bi in 0..b as usize {
        for j in 0..lus {
            if attn[bi * lus + j] == 0.0 {
                m[bi * lus + j] = f32::MIN;
            }
        }
    }
    Tensor::from_slice(&m).view([b, 1, 1, l]).to_device(device)
}

impl LayaModel {
    pub fn load(model_dir: &str) -> Result<Self> {
        Self::load_on(model_dir, Device::Cpu)
    }

    /// Load weights onto `device` (CPU or CUDA). All cached RoPE/mask tables
    /// are also created on that device, so no per-forward copies are needed.
    pub fn load_on(model_dir: &str, device: Device) -> Result<Self> {
        let weights_path = format!("{}/model.safetensors", model_dir);
        let data = std::fs::read(&weights_path)?;
        let st = safetensors::SafeTensors::deserialize(&data)?;

        let load = |name: &str| -> Result<Tensor> { load_tensor(&st, name, device) };

        let mut layers = Vec::with_capacity(N_LAYERS);
        for i in 0..N_LAYERS {
            let p = |s: &str| format!("encoder.layers.{}.{}", i, s);
            let attn_norm = if i == 0 {
                None
            } else {
                Some(load(&p("attn_norm.weight"))?)
            };
            layers.push(EncLayer {
                attn_norm,
                wqkv: load(&p("attn.Wqkv.weight"))?,
                wo: load(&p("attn.Wo.weight"))?,
                mlp_norm: load(&p("mlp_norm.weight"))?,
                wi: load(&p("mlp.Wi.weight"))?,
                mlp_wo: load(&p("mlp.Wo.weight"))?,
                is_full: i % 3 == 0,
            });
        }

        let mut head = Vec::with_capacity(N_HEAD_LAYERS);
        for i in 0..N_HEAD_LAYERS {
            let p = |s: &str| format!("head.layers.{}.{}", i, s);
            head.push(HeadLayer {
                norm1_w: load(&p("norm1.weight"))?,
                norm1_b: load(&p("norm1.bias"))?,
                in_proj_w: load(&p("self_attn.in_proj_weight"))?,
                in_proj_b: load(&p("self_attn.in_proj_bias"))?,
                out_proj_w: load(&p("self_attn.out_proj.weight"))?,
                out_proj_b: load(&p("self_attn.out_proj.bias"))?,
                norm2_w: load(&p("norm2.weight"))?,
                norm2_b: load(&p("norm2.bias"))?,
                lin1_w: load(&p("linear1.weight"))?,
                lin1_b: load(&p("linear1.bias"))?,
                lin2_w: load(&p("linear2.weight"))?,
                lin2_b: load(&p("linear2.bias"))?,
            });
        }

        Ok(Self {
            tok_emb: load("encoder.embeddings.tok_embeddings.weight")?,
            emb_norm_w: load("encoder.embeddings.norm.weight")?,
            layers,
            final_norm_w: load("encoder.final_norm.weight")?,
            type_emb: load("type_emb.weight")?,
            head,
            scorer_ln_w: load("scorer.0.weight")?,
            scorer_ln_b: load("scorer.0.bias")?,
            scorer_l1_w: load("scorer.1.weight")?,
            scorer_l1_b: load("scorer.1.bias")?,
            scorer_l2_w: load("scorer.3.weight")?,
            scorer_l2_b: load("scorer.3.bias")?,
            act0_w: load("act_head.0.weight")?,
            act0_b: load("act_head.0.bias")?,
            act2_w: load("act_head.2.weight")?,
            act2_b: load("act_head.2.bias")?,
            rope_cache: Mutex::new(HashMap::new()),
            mask_cache: Mutex::new(HashMap::new()),
            band_cache: Mutex::new(HashMap::new()),
            n_layers_seen: N_LAYERS,
            device,
        })
    }

    fn rope(&self, l: i64, is_full: bool) -> (Tensor, Tensor) {
        let key = (is_full, l);
        let mut cache = self.rope_cache.lock().unwrap();
        if let Some(v) = cache.get(&key) {
            return (v.0.shallow_clone(), v.1.shallow_clone());
        }
        let theta = if is_full {
            ROPE_THETA_FULL
        } else {
            ROPE_THETA_SLIDING
        };
        let tables = rope_table(l, theta, self.device);
        let out = (tables.0.shallow_clone(), tables.1.shallow_clone());
        cache.insert(key, tables);
        out
    }

    fn mask(&self, l: i64) -> Tensor {
        let mut cache = self.mask_cache.lock().unwrap();
        if let Some(v) = cache.get(&l) {
            return v.shallow_clone();
        }
        let m = sliding_mask(l, self.device);
        cache.insert(l, m.shallow_clone());
        m
    }

    /// Cached band mask `[1, 1, L, L]` for the sliding-attention layers.
    fn band(&self, l: i64) -> Tensor {
        let mut cache = self.band_cache.lock().unwrap();
        if let Some(v) = cache.get(&l) {
            return v.shallow_clone();
        }
        let m = sliding_mask(l, self.device).view([1, 1, l, l]);
        cache.insert(l, m.shallow_clone());
        m
    }

    /// Encoder + type embedding + decision head + marker scorer for one sequence.
    ///
    /// `ids`: token ids; `marker_pos`: per-option `[MASK]` positions; `qtype`: 0/1/2.
    /// Returns raw marker logits (length = marker_pos.len()).
    pub fn forward(&self, ids: &[i64], marker_pos: &[i64], qtype: i64) -> Result<Vec<f32>> {
        Ok(self.run(ids, marker_pos, qtype)?.3)
    }

    /// Like `forward` but also returns intermediate activations
    /// `(encoder_out, after_type_emb, head_out)` for parity debugging.
    pub fn run(
        &self,
        ids: &[i64],
        marker_pos: &[i64],
        qtype: i64,
    ) -> Result<(Tensor, Tensor, Tensor, Vec<f32>)> {
        let l = ids.len() as i64;
        let ids_t = Tensor::from_slice(ids).to_device(self.device);
        let mut h = self.tok_emb.index_select(0, &ids_t); // [L, HIDDEN]
        h = layer_norm(&h, &self.emb_norm_w, None);

        for layer in &self.layers {
            let normed = match &layer.attn_norm {
                Some(w) => layer_norm(&h, w, None),
                None => h.shallow_clone(),
            };
            let qkv = normed.linear(&layer.wqkv, None::<&Tensor>); // [L, 3H]
            let q = qkv
                .narrow(1, 0, HIDDEN)
                .view([l, HEADS, HEAD_DIM])
                .transpose(0, 1)
                .contiguous();
            let k = qkv
                .narrow(1, HIDDEN, HIDDEN)
                .view([l, HEADS, HEAD_DIM])
                .transpose(0, 1)
                .contiguous();
            let v = qkv
                .narrow(1, 2 * HIDDEN, HIDDEN)
                .view([l, HEADS, HEAD_DIM])
                .transpose(0, 1)
                .contiguous();
            let (cos, sin) = self.rope(l, layer.is_full);
            let q = apply_rope(&q, &cos, &sin);
            let k = apply_rope(&k, &cos, &sin);

            let scaling = 1.0f64 / (HEAD_DIM as f64).sqrt();
            let mut scores = q.matmul(&k.transpose(-2, -1)) * scaling; // [HEADS, L, L]
            if !layer.is_full {
                scores = scores + self.mask(l).unsqueeze(0);
            }
            let probs = scores.softmax(-1, Kind::Float);
            let ao = probs
                .matmul(&v)
                .transpose(0, 1)
                .contiguous()
                .view([l, HIDDEN])
                .linear(&layer.wo, None::<&Tensor>);
            h = h + ao;

            let normed = layer_norm(&h, &layer.mlp_norm, None);
            let gv = normed.linear(&layer.wi, None::<&Tensor>); // [L, 2*INTER]
            let input = gv.narrow(1, 0, INTERMEDIATE);
            let gate = gv.narrow(1, INTERMEDIATE, INTERMEDIATE);
            let act = input.gelu("none") * gate;
            h = h + act.linear(&layer.mlp_wo, None::<&Tensor>);
        }

        h = layer_norm(&h, &self.final_norm_w, None);
        let encoder_out = h.shallow_clone();

        // add type embedding (broadcast over sequence)
        let te = self
            .type_emb
            .index_select(0, &Tensor::from_slice(&[qtype]).to_device(self.device)); // [1, HIDDEN]
        h = h + te;
        let after_type = h.shallow_clone();

        // decision head: 2 x TransformerEncoderLayer(norm_first=True, relu)
        for layer in &self.head {
            // self-attention
            let x = layer_norm(&h, &layer.norm1_w, Some(&layer.norm1_b));
            let qkv = x.linear(&layer.in_proj_w, Some(&layer.in_proj_b)); // [L, 3H]
            let q = qkv
                .narrow(1, 0, HIDDEN)
                .view([l, HEADS, HEAD_DIM])
                .transpose(0, 1)
                .contiguous();
            let k = qkv
                .narrow(1, HIDDEN, HIDDEN)
                .view([l, HEADS, HEAD_DIM])
                .transpose(0, 1)
                .contiguous();
            let v = qkv
                .narrow(1, 2 * HIDDEN, HIDDEN)
                .view([l, HEADS, HEAD_DIM])
                .transpose(0, 1)
                .contiguous();
            let scaling = 1.0f64 / (HEAD_DIM as f64).sqrt();
            let scores = q.matmul(&k.transpose(-2, -1)) * scaling;
            let probs = scores.softmax(-1, Kind::Float);
            let attn = probs
                .matmul(&v)
                .transpose(0, 1)
                .contiguous()
                .view([l, HIDDEN])
                .linear(&layer.out_proj_w, Some(&layer.out_proj_b));
            h = h + attn;

            // feed-forward (relu)
            let x = layer_norm(&h, &layer.norm2_w, Some(&layer.norm2_b));
            let y = x.linear(&layer.lin1_w, Some(&layer.lin1_b)).relu();
            let y = y.linear(&layer.lin2_w, Some(&layer.lin2_b));
            h = h + y;
        }
        let head_out = h.shallow_clone();

        // gather marker rows
        let idx = Tensor::from_slice(marker_pos).to_device(self.device);
        let m = h.index_select(0, &idx); // [K, HIDDEN]

        let m = layer_norm(&m, &self.scorer_ln_w, Some(&self.scorer_ln_b));
        let y = m.linear(&self.scorer_l1_w, Some(&self.scorer_l1_b));
        let y = y.gelu("none");
        let y = y.linear(&self.scorer_l2_w, Some(&self.scorer_l2_b)); // [K, 1]
        let logits: Vec<f32> = Vec::try_from(y.view([-1]))?;
        Ok((encoder_out, after_type, head_out, logits))
    }

    /// Batched forward over several sequences (one per question in a request).
    ///
    /// Rows are padded to `Lmax`; padding is masked out of both the encoder and
    /// the decision head exactly like HF's `attention_mask` / `src_key_padding_mask`.
    /// Returns per-sequence raw marker logits.
    pub fn forward_batch(&self, seqs: &[SeqInput]) -> Result<Vec<Output>> {
        let b = seqs.len();
        if b == 0 {
            return Ok(Vec::new());
        }
        let bf = b as i64;
        let lmax = seqs.iter().map(|s| s.ids.len() as i64).max().unwrap();

        let profile = std::env::var("LAYA_PROFILE").is_ok();
        let mut acc = [0f64; 8];
        #[allow(unused_assignments)]
        let mut tp = std::time::Instant::now();
        macro_rules! lap {
            ($i:expr) => {
                if profile {
                    acc[$i] += tp.elapsed().as_secs_f64();
                    tp = std::time::Instant::now();
                }
            };
        }

        let mut ids_v = vec![0i64; (bf * lmax) as usize];
        let mut attn_v = vec![0f32; (bf * lmax) as usize];
        for (bi, s) in seqs.iter().enumerate() {
            for (i, &v) in s.ids.iter().enumerate() {
                ids_v[bi * lmax as usize + i] = v;
                attn_v[bi * lmax as usize + i] = 1.0;
            }
        }
        let ids_t = Tensor::from_slice(&ids_v)
            .view([bf, lmax])
            .to_device(self.device);
        let mut h = self
            .tok_emb
            .index_select(0, &ids_t.view([-1]))
            .view([bf, lmax, HIDDEN]);
        h = layer_norm(&h, &self.emb_norm_w, None);
        lap!(0);

        let has_pad = attn_v.iter().any(|&v| v == 0.0);
        let key_pad = if has_pad {
            Some(build_key_pad_mask(bf, lmax, &attn_v, self.device))
        } else {
            None
        };

        for layer in &self.layers {
            let normed = match &layer.attn_norm {
                Some(w) => layer_norm(&h, w, None),
                None => h.shallow_clone(),
            };
            lap!(1);
            let qkv = normed.linear(&layer.wqkv, None::<&Tensor>); // [B,L,3H]
            lap!(2);
            let q = qkv
                .narrow(2, 0, HIDDEN)
                .view([bf, lmax, HEADS, HEAD_DIM])
                .transpose(1, 2)
                .contiguous();
            let k = qkv
                .narrow(2, HIDDEN, HIDDEN)
                .view([bf, lmax, HEADS, HEAD_DIM])
                .transpose(1, 2)
                .contiguous();
            let v = qkv
                .narrow(2, 2 * HIDDEN, HIDDEN)
                .view([bf, lmax, HEADS, HEAD_DIM])
                .transpose(1, 2)
                .contiguous();
            let (cos, sin) = self.rope(lmax, layer.is_full);
            let cos = cos.view([1, 1, lmax, HEAD_DIM]);
            let sin = sin.view([1, 1, lmax, HEAD_DIM]);
            let q = apply_rope(&q, &cos, &sin);
            let k = apply_rope(&k, &cos, &sin);

            let scaling = 1.0f64 / (HEAD_DIM as f64).sqrt();
            let mut scores = q.matmul(&k.transpose(-2, -1)) * scaling; // [B,H,L,L]
            if !layer.is_full {
                scores = scores + self.band(lmax);
            }
            if let Some(kp) = &key_pad {
                scores = scores + kp;
            }
            let probs = scores.softmax(-1, Kind::Float);
            let ao = probs
                .matmul(&v)
                .transpose(1, 2)
                .contiguous()
                .view([bf, lmax, HIDDEN])
                .linear(&layer.wo, None::<&Tensor>);
            lap!(3);
            h = h + ao;

            let normed = layer_norm(&h, &layer.mlp_norm, None);
            let gv = normed.linear(&layer.wi, None::<&Tensor>);
            let input = gv.narrow(2, 0, INTERMEDIATE);
            let gate = gv.narrow(2, INTERMEDIATE, INTERMEDIATE);
            let act = input.gelu("none") * gate;
            h = h + act.linear(&layer.mlp_wo, None::<&Tensor>);
            lap!(4);
        }

        h = layer_norm(&h, &self.final_norm_w, None);
        lap!(5);

        let qt: Vec<i64> = seqs.iter().map(|s| s.qtype).collect();
        let te = self
            .type_emb
            .index_select(0, &Tensor::from_slice(&qt).to_device(self.device))
            .unsqueeze(1); // [B,1,H]
        h = h + te;

        let pad_mask = match &key_pad {
            Some(kp) => kp.shallow_clone(),
            None => build_key_pad_mask(bf, lmax, &attn_v, self.device),
        }; // [B,1,1,L]

        for layer in &self.head {
            let x = layer_norm(&h, &layer.norm1_w, Some(&layer.norm1_b));
            let qkv = x.linear(&layer.in_proj_w, Some(&layer.in_proj_b));
            let q = qkv
                .narrow(2, 0, HIDDEN)
                .view([bf, lmax, HEADS, HEAD_DIM])
                .transpose(1, 2)
                .contiguous();
            let k = qkv
                .narrow(2, HIDDEN, HIDDEN)
                .view([bf, lmax, HEADS, HEAD_DIM])
                .transpose(1, 2)
                .contiguous();
            let v = qkv
                .narrow(2, 2 * HIDDEN, HIDDEN)
                .view([bf, lmax, HEADS, HEAD_DIM])
                .transpose(1, 2)
                .contiguous();
            let scaling = 1.0f64 / (HEAD_DIM as f64).sqrt();
            let scores = q.matmul(&k.transpose(-2, -1)) * scaling + &pad_mask;
            let probs = scores.softmax(-1, Kind::Float);
            let attn = probs
                .matmul(&v)
                .transpose(1, 2)
                .contiguous()
                .view([bf, lmax, HIDDEN])
                .linear(&layer.out_proj_w, Some(&layer.out_proj_b));
            h = h + attn;

            let x = layer_norm(&h, &layer.norm2_w, Some(&layer.norm2_b));
            let y = x.linear(&layer.lin1_w, Some(&layer.lin1_b)).relu();
            let y = y.linear(&layer.lin2_w, Some(&layer.lin2_b));
            h = h + y;
        }

        let mut out = Vec::with_capacity(b);
        for (bi, s) in seqs.iter().enumerate() {
            let mp = Tensor::from_slice(s.markers).to_device(self.device);
            let hb = h.get(bi as i64); // [L,H] post-head
            let m = hb.index_select(0, &mp); // [K,H]
            let m = layer_norm(&m, &self.scorer_ln_w, Some(&self.scorer_ln_b));
            let y = m.linear(&self.scorer_l1_w, Some(&self.scorer_l1_b));
            let y = y.gelu("none");
            let y = y.linear(&self.scorer_l2_w, Some(&self.scorer_l2_b));
            let logits: Vec<f32> = Vec::try_from(y.view([-1]))?;

            // act head: pooled CLS + detached summary of the answer distribution
            let act_logits = self.act(&hb, &logits);
            out.push(Output { logits, act_logits });
        }
        lap!(6);
        if profile {
            let names = [
                "embed",
                "enc_norm",
                "enc_qkv_linear",
                "enc_attn(rope+sdpa)",
                "enc_mlp",
                "final_norm",
                "head(type+2L)",
                "scorer",
            ];
            for (i, n) in names.iter().enumerate() {
                eprintln!("[prof] {:<20} {:8.1} ms", n, acc[i] * 1000.0);
            }
        }
        Ok(out)
    }

    /// `act_head` forward, mirroring `DecisionModel.forward`'s act branch.
    fn act(&self, h_post_head: &Tensor, logits: &[f32]) -> Vec<f32> {
        let k = logits.len();
        if k < 2 {
            return vec![0.0, 0.0];
        }
        let lf = Tensor::from_slice(logits);
        let p = lf.softmax(-1, Kind::Float);
        let kf = k as f64;
        let ent = -(p.shallow_clone() * p.shallow_clone().clamp_min(1e-9).log()).sum(Kind::Float)
            / kf.max(2.0).ln();
        let entropy = ent.double_value(&[]) as f32;
        let top2 = p.topk(2, -1, true, true).0; // [2] sorted desc
        let t0 = top2.double_value(&[0]) as f32;
        let t1 = top2.double_value(&[1]) as f32;
        let feats = Tensor::from_slice(&[t0, t0 - t1, entropy, (k as f32) / 255.0]);
        let pooled = h_post_head.select(0, 0); // [H]
        let inp = Tensor::cat(&[&pooled, &feats], -1); // [H+4]
        let a = inp
            .linear(&self.act0_w, Some(&self.act0_b))
            .gelu("none")
            .linear(&self.act2_w, Some(&self.act2_b));
        Vec::try_from(a.view([-1])).unwrap_or_else(|_| vec![0.0, 0.0])
    }

    /// Full pipeline producing final softmax probabilities for `marker_pos`
    /// (mirrors `RLAgent.system_one`'s logits -> temperature -> softmax).
    pub fn probs(&self, ids: &[i64], marker_pos: &[i64], qtype: i64) -> Result<Vec<f32>> {
        let logits = self.forward(ids, marker_pos, qtype)?;
        let max = logits.iter().cloned().fold(f32::NEG_INFINITY, f32::max);
        let exps: Vec<f32> = logits.iter().map(|v| (v - max).exp()).collect();
        let sum: f32 = exps.iter().sum();
        Ok(exps.into_iter().map(|v| v / sum).collect())
    }
}
