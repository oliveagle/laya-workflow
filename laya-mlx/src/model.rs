//! Native MLX (Rust) implementation of the Laya decision model.
//!
//! A faithful port of `laya-tch/mlx/native/laya_mlx/model.py` (ModernBERT-large
//! encoder + 2-layer decision head + marker scorer + action head) onto `mlx-rs`.
//! Weights are read straight from the FP16 MLX checkpoint (MLX parameter names).
//!
//! Correctness notes carried over from the Python/`model.rs` implementations:
//!   * encoder LayerNorm: weight only (no bias), eps = 1e-5
//!   * pre-attention norm is Identity for layer 0, LayerNorm for layers 1..27
//!   * RoPE on q/k, per-layer base (full = 160000, sliding = 10000), head_dim = 64
//!   * sliding band `|i - j| <= local_attention / 2`
//!   * GeGLU MLP: `Wo(gelu(first_half) * second_half)`
//!   * decision head: 2 x (norm_first, ReLU) transformer layers
//!   * scorer: LayerNorm(bias) -> Linear(bias) -> GELU -> Linear(bias)

use std::collections::HashMap;

use anyhow::{anyhow, Result};
use mlx_rs::fast::scaled_dot_product_attention;
use mlx_rs::fast::rope;
use mlx_rs::nn::relu;
use mlx_rs::ops;
use mlx_rs::{Array, Dtype};

/// GELU that preserves the input dtype.
///
/// `mlx_rs::nn::gelu` funnels through `mlx_gelu`, which **returns float32** even
/// for a float16 input. Left unchecked that upcast leaks into every downstream
/// op (mul/matmul/add …), turning the whole model into an f32 graph and roughly
/// doubling the runtime. The Python reference computes
/// `x * (1 + erf(x / sqrt(2))) / 2` with weak scalars, which stays float16 — so
/// we reproduce that exactly (with correctly-typed constants).
pub fn gelu(x: &Array) -> Result<Array> {
    let dt = x.dtype();
    let one = Array::from_f32(1.0).as_dtype(dt)?;
    let two = Array::from_f32(2.0).as_dtype(dt)?;
    let sqrt2 = Array::from_f32(std::f32::consts::SQRT_2).as_dtype(dt)?;
    let inner = ops::erf(&x.divide(&sqrt2)?)?;
    Ok(x.multiply(&one.add(&inner)?)?.divide(&two)?)
}

pub const HIDDEN: i32 = 1024;
pub const HEADS: i32 = 16;
pub const HEAD_DIM: i32 = 64;
pub const N_LAYERS: usize = 28;
const EPS: f32 = 1e-5;
const ROPE_FULL: f32 = 160_000.0;
const ROPE_SLIDING: f32 = 10_000.0;

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum AttnKind {
    Full,
    Sliding,
}

struct EncLayer {
    attn_norm: Option<Array>,
    wqkv: Array,
    wo: Array,
    mlp_norm: Array,
    wi: Array,
    mlp_wo: Array,
    kind: AttnKind,
}

struct HeadLayer {
    norm1_w: Array,
    norm1_b: Array,
    in_proj_w: Array,
    in_proj_b: Array,
    out_proj_w: Array,
    out_proj_b: Array,
    norm2_w: Array,
    norm2_b: Array,
    lin1_w: Array,
    lin1_b: Array,
    lin2_w: Array,
    lin2_b: Array,
}

pub struct DecisionModel {
    tok_emb: Array,
    emb_norm: Array,
    layers: Vec<EncLayer>,
    final_norm: Array,
    type_emb: Array,
    head: Vec<HeadLayer>,
    scorer_ln_w: Array,
    scorer_ln_b: Array,
    scorer_l1_w: Array,
    scorer_l1_b: Array,
    scorer_l2_w: Array,
    scorer_l2_b: Array,
    act0_w: Array,
    act0_b: Array,
    act2_w: Array,
    act2_b: Array,
    local_window: i32,
}

// ── tensor helpers ──────────────────────────────────────────────────────────

fn dtype() -> Dtype {
    Dtype::Float16
}

fn f32_of(x: &Array) -> Result<Array> {
    Ok(x.as_dtype(Dtype::Float32)?)
}

fn linear(x: &Array, w: &Array, b: Option<&Array>) -> Result<Array> {
    let y = x.matmul(&w.transpose_axes(&[1, 0])?)?;
    match b {
        Some(b) => Ok(y.add(b)?),
        None => Ok(y),
    }
}

/// LayerNorm over the last dim, using MLX's fused kernel — the same op the
/// Python reference reaches through `nn.LayerNorm`. `weight` is always present;
/// `bias` is optional.
fn layer_norm(x: &Array, w: &Array, b: Option<&Array>) -> Result<Array> {
    Ok(mlx_rs::fast::layer_norm(x, Some(w), b, EPS)?)
}

/// Boolean key masks. Mirrors `attention_masks` in the Python runtime.
fn attention_masks(attention_mask: &Array, window: i32) -> Result<(Array, Array)> {
    let valid = attention_mask.as_dtype(Dtype::Bool)?;
    let full = valid.expand_dims(1)?.expand_dims(2)?; // (b,1,1,l)
    let l = valid.shape()[1];
    let positions = ops::arange::<i32, i32>(None, l, None)?;
    let pi = positions.expand_dims(1)?; // (l,1)
    let pj = positions.expand_dims(0)?; // (1,l)
    let dist = ops::abs(&pi.subtract(&pj)?)?;
    let local = dist.le(&Array::from_int(window / 2))?; // (l,l)
    let local = local.expand_dims(0)?.expand_dims(1)?; // (1,1,l,l)
    let not_valid = valid.expand_dims(1)?.expand_dims(3)?.logical_not()?; // (b,1,l,1)
    let local = ops::logical_or(&local, &not_valid)?; // (b,1,l,l)
    let local = ops::logical_and(&local, &full)?; // (b,1,l,l)
    Ok((full, local))
}

// ── model parts ─────────────────────────────────────────────────────────────

impl DecisionModel {
    pub fn load(weights: HashMap<String, Array>, local_window: i32, n_head_layers: usize) -> Result<Self> {
        let get = |n: &str| -> Result<Array> {
            weights
                .get(n)
                .cloned()
                .ok_or_else(|| anyhow!("missing tensor {n}"))
                .map(|a| a.as_dtype(dtype()).unwrap_or(a))
        };

        let mut layers = Vec::with_capacity(N_LAYERS);
        for i in 0..N_LAYERS {
            let p = |s: &str| format!("encoder.layers.{i}.{s}");
            layers.push(EncLayer {
                attn_norm: if i == 0 { None } else { Some(get(&p("attn_norm.weight"))?) },
                wqkv: get(&p("attn.Wqkv.weight"))?,
                wo: get(&p("attn.Wo.weight"))?,
                mlp_norm: get(&p("mlp_norm.weight"))?,
                wi: get(&p("mlp.Wi.weight"))?,
                mlp_wo: get(&p("mlp.Wo.weight"))?,
                kind: if i % 3 == 0 { AttnKind::Full } else { AttnKind::Sliding },
            });
        }

        let mut head = Vec::with_capacity(n_head_layers);
        for i in 0..n_head_layers {
            let p = |s: &str| format!("head.layers.{i}.{s}");
            head.push(HeadLayer {
                norm1_w: get(&p("norm1.weight"))?,
                norm1_b: get(&p("norm1.bias"))?,
                in_proj_w: get(&p("self_attn.in_proj.weight"))?,
                in_proj_b: get(&p("self_attn.in_proj.bias"))?,
                out_proj_w: get(&p("self_attn.out_proj.weight"))?,
                out_proj_b: get(&p("self_attn.out_proj.bias"))?,
                norm2_w: get(&p("norm2.weight"))?,
                norm2_b: get(&p("norm2.bias"))?,
                lin1_w: get(&p("linear1.weight"))?,
                lin1_b: get(&p("linear1.bias"))?,
                lin2_w: get(&p("linear2.weight"))?,
                lin2_b: get(&p("linear2.bias"))?,
            });
        }

        Ok(Self {
            tok_emb: get("encoder.embeddings.tok_embeddings.weight")?,
            emb_norm: get("encoder.embeddings.norm.weight")?,
            layers,
            final_norm: get("encoder.final_norm.weight")?,
            type_emb: get("type_emb.weight")?,
            head,
            scorer_ln_w: get("scorer.layers.0.weight")?,
            scorer_ln_b: get("scorer.layers.0.bias")?,
            scorer_l1_w: get("scorer.layers.1.weight")?,
            scorer_l1_b: get("scorer.layers.1.bias")?,
            scorer_l2_w: get("scorer.layers.3.weight")?,
            scorer_l2_b: get("scorer.layers.3.bias")?,
            act0_w: get("act_head.layers.0.weight")?,
            act0_b: get("act_head.layers.0.bias")?,
            act2_w: get("act_head.layers.2.weight")?,
            act2_b: get("act_head.layers.2.bias")?,
            local_window,
        })
    }

    fn encoder(&self, input_ids: &Array, attention_mask: &Array) -> Result<(Array, Array)> {
        let (full_mask, local_mask) = attention_masks(attention_mask, self.local_window)?;
        let shape = input_ids.shape(); // (b,l)
        let (b, l) = (shape[0], shape[1]);
        let flat = input_ids.reshape(&[-1])?;
        let mut x = self.tok_emb.take_axis(&flat, 0)?.reshape(&[b, l, HIDDEN])?;
        x = layer_norm(&x, &self.emb_norm, None)?;

        for layer in &self.layers {
            let normed = match &layer.attn_norm {
                Some(w) => layer_norm(&x, w, None)?,
                None => x.clone(),
            };
            let mask = match layer.kind {
                AttnKind::Full => &full_mask,
                AttnKind::Sliding => &local_mask,
            };
            let attn = self.attention(&normed, &layer.wqkv, &layer.wo, layer.kind, mask)?;
            x = x.add(&attn)?;

            let normed = layer_norm(&x, &layer.mlp_norm, None)?;
            let gv = linear(&normed, &layer.wi, None)?;
            let parts = gv.split_equal(2, Some(-1))?;
            let act = gelu(&parts[0])?.multiply(&parts[1])?;
            let y = linear(&act, &layer.mlp_wo, None)?;
            x = x.add(&y)?;
        }
        let x = layer_norm(&x, &self.final_norm, None)?;
        Ok((x, local_mask))
    }

    #[allow(clippy::too_many_arguments)]
    fn attention(
        &self,
        x: &Array,
        wqkv: &Array,
        wo: &Array,
        kind: AttnKind,
        mask: &Array,
    ) -> Result<Array> {
        let s = x.shape();
        let (b, l) = (s[0], s[1]);
        let qkv = linear(x, wqkv, None)?;
        let qkv = qkv.reshape(&[b, l, 3, HEADS, HEAD_DIM])?;
        let parts = qkv.split_equal(3, Some(2))?; // 3 x (b,l,1,H,hd)
        let head = |p: &Array| -> Result<Array> {
            Ok(p.reshape(&[b, l, HEADS, HEAD_DIM])?.transpose_axes(&[0, 2, 1, 3])?)
        };
        let q = head(&parts[0])?;
        let k = head(&parts[1])?;
        let v = head(&parts[2])?;
        let base = match kind {
            AttnKind::Full => ROPE_FULL,
            AttnKind::Sliding => ROPE_SLIDING,
        };
        let q = rope(&q, HEAD_DIM, false, Some(base), 1.0, 0, None)?;
        let k = rope(&k, HEAD_DIM, false, Some(base), 1.0, 0, None)?;
        let out = scaled_dot_product_attention(
            &q,
            &k,
            &v,
            (HEAD_DIM as f32).powf(-0.5),
            mask,
            None,
        )?; // (b,H,l,hd)
        let out = out.transpose_axes(&[0, 2, 1, 3])?.reshape(&[b, l, HIDDEN])?;
        linear(&out, wo, None)
    }

    /// Full forward. Returns `(logits (b,K), action (b,A))` in float32.
    pub fn forward(
        &self,
        input_ids: &Array,
        attention_mask: &Array,
        marker_pos: &Array,
        marker_mask: &Array,
        qtype: &Array,
    ) -> Result<(Array, Array)> {
        let (mut h, _local) = self.encoder(input_ids, attention_mask)?;

        // + type embedding (broadcast over the sequence)
        let te = self.type_emb.take_axis(qtype, 0)?.expand_dims(1)?; // (b,1,H)
        h = h.add(&te)?;

        // decision head: attention mask (b,1,1,l)
        let head_mask = attention_mask.as_dtype(Dtype::Bool)?.expand_dims(1)?.expand_dims(2)?;
        for layer in &self.head {
            let x = layer_norm(&h, &layer.norm1_w, Some(&layer.norm1_b))?;
            let s = x.shape();
            let (b, l) = (s[0], s[1]);
            let qkv = linear(&x, &layer.in_proj_w, Some(&layer.in_proj_b))?;
            let qkv = qkv.reshape(&[b, l, 3, HEADS, HEAD_DIM])?;
            let parts = qkv.split_equal(3, Some(2))?;
            let hd = |p: &Array| -> Result<Array> {
                Ok(p.reshape(&[b, l, HEADS, HEAD_DIM])?.transpose_axes(&[0, 2, 1, 3])?)
            };
            let attn = scaled_dot_product_attention(
                &hd(&parts[0])?,
                &hd(&parts[1])?,
                &hd(&parts[2])?,
                (HEAD_DIM as f32).powf(-0.5),
                &head_mask,
                None,
            )?;
            let attn = attn.transpose_axes(&[0, 2, 1, 3])?.reshape(&[b, l, HIDDEN])?;
            let attn = linear(&attn, &layer.out_proj_w, Some(&layer.out_proj_b))?;
            h = h.add(&attn)?;

            let x = layer_norm(&h, &layer.norm2_w, Some(&layer.norm2_b))?;
            let y = relu(&linear(&x, &layer.lin1_w, Some(&layer.lin1_b))?)?;
            let y = linear(&y, &layer.lin2_w, Some(&layer.lin2_b))?;
            h = h.add(&y)?;
        }

        // gather marker rows: h[b, marker_pos] -> (b,K,H)
        let idx = ops::maximum(marker_pos, &Array::from_int(0))?.expand_dims(2)?; // (b,K,1)
        let markers = h.take_along_axis(&idx, 1)?; // (b,K,H)

        let m = layer_norm(&markers, &self.scorer_ln_w, Some(&self.scorer_ln_b))?;
        let y = linear(&m, &self.scorer_l1_w, Some(&self.scorer_l1_b))?;
        let y = gelu(&y)?;
        let y = linear(&y, &self.scorer_l2_w, Some(&self.scorer_l2_b))?; // (b,K,1)
        let logits = f32_of(&y.squeeze_axes(&[-1])?)?; // (b,K)

        let mask_b = marker_mask.as_dtype(Dtype::Bool)?;
        let neg = Array::from_f32(-1e4);
        let logits = ops::select(&mask_b, &logits, &neg)?;
        let p = ops::softmax_axis(&logits, -1, None)?; // f32

        // entropy / top-2 features
        let kf = mask_b.as_dtype(Dtype::Int32)?.sum_axes(&[-1], false)?.as_dtype(Dtype::Float32)?;
        let kf = ops::maximum(&kf, &Array::from_f32(2.0))?;
        let logp = ops::maximum(&p, &Array::from_f32(1e-9))?.log()?;
        let ent = p.multiply(&logp)?.sum_axes(&[-1], false)?.negative()?;
        let entropy = ent.divide(&kf.log()?)?; // (b,)

        let sorted = ops::sort_axis(&p, -1)?; // (b,K) ascending
        let klen = sorted.shape()[1];
        let idx = Array::from_slice(&[klen - 2, klen - 1], &[2]);
        let top = sorted.take_axis(&idx, 1)?; // (b,2)
        let top1 = top.take_axis(&Array::from_int(1), 1)?; // (b,) highest
        let top0 = top.take_axis(&Array::from_int(0), 1)?; // (b,) second
        let feats = ops::stack(
            &[
                top1.clone(),
                top1.subtract(&top0)?,
                entropy.clone(),
                kf.divide(&Array::from_f32(255.0))?,
            ],
            -1,
        )?; // (b,4)

        let pooled_h = h.take_axis(&Array::from_int(0), 1)?; // (b,H)
        let pooled = ops::concatenate(&[&f32_of(&pooled_h)?, &feats], -1)?; // (b,H+4)
        let hidden = linear(&pooled, &self.act0_w, Some(&self.act0_b))?;
        let hidden = gelu(&hidden)?;
        let action = linear(&hidden, &self.act2_w, Some(&self.act2_b))?;
        Ok((logits, f32_of(&action)?))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Regression guard for the f32-upcast bug: `mlx_rs::nn::gelu` returns
    /// float32 for a float16 input, which silently turned the whole model into
    /// an f32 graph (~1.6x slower). Our `gelu` must preserve dtype.
    #[test]
    fn gelu_preserves_dtype() {
        let x = Array::from_slice(&[0.5f32, -1.0, 2.0], &[3]).as_dtype(Dtype::Float16).unwrap();
        let y = gelu(&x).unwrap();
        assert_eq!(y.dtype(), Dtype::Float16, "gelu must not upcast float16");
        let y32 = gelu(&x.as_dtype(Dtype::Float32).unwrap()).unwrap();
        assert_eq!(y32.dtype(), Dtype::Float32);
    }
}
