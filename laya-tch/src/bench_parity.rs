//! Parity + performance harness: compares `laya-tch` against the captured
//! PyTorch reference (`bench/reference_trace.json` + `bench/reference_probe/`).
//!
//! Usage:
//!   LD_LIBRARY_PATH=... ./target/release/bench_parity <model_dir> [trace.json] [probe_dir]

use std::path::PathBuf;
use std::time::Instant;

use anyhow::Result;
use serde::Deserialize;

mod model;
use model::{LayaModel, SeqInput};

#[derive(Deserialize)]
struct Trace {
    rows: Vec<Row>,
}

#[derive(Deserialize)]
struct Row {
    qid: String,
    ids: Vec<i64>,
    markers: Vec<i64>,
    qtype: i64,
    single_logits: Vec<f64>,
    #[allow(dead_code)]
    logits: Vec<f64>,
    #[serde(default)]
    act_probability: Option<f64>,
}

fn read_f32(path: &std::path::Path) -> Result<Vec<f32>> {
    let b = std::fs::read(path)?;
    Ok(b.chunks_exact(4)
        .map(|c| f32::from_le_bytes([c[0], c[1], c[2], c[3]]))
        .collect())
}

fn read_i64(path: &std::path::Path) -> Result<Vec<i64>> {
    let b = std::fs::read(path)?;
    Ok(b.chunks_exact(8)
        .map(|c| i64::from_le_bytes([c[0], c[1], c[2], c[3], c[4], c[5], c[6], c[7]]))
        .collect())
}

fn max_abs_diff(a: &[f32], b: &[f32]) -> (f32, usize) {
    let mut m = 0f32;
    let mut at = 0usize;
    for (i, (x, y)) in a.iter().zip(b.iter()).enumerate() {
        let d = (x - y).abs();
        if d > m {
            m = d;
            at = i;
        }
    }
    (m, at)
}

fn main() -> Result<()> {
    let args: Vec<String> = std::env::args().collect();
    let model_dir = args.get(1).cloned().unwrap_or_else(|| {
        format!(
            "{}/models/convaiinnovations--laya",
            std::env::var("HOME").unwrap_or_default()
        )
    });
    let bench_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("bench");
    let trace_path = args
        .get(2)
        .map(PathBuf::from)
        .unwrap_or_else(|| bench_dir.join("reference_trace.json"));
    let probe_dir = args
        .get(3)
        .map(PathBuf::from)
        .unwrap_or_else(|| bench_dir.join("reference_probe"));

    println!("[parity] loading model from {}", model_dir);
    let t0 = Instant::now();
    let model = LayaModel::load(&model_dir)?;
    println!("[parity] loaded in {:.1}s", t0.elapsed().as_secs_f64());

    // ── probe: intermediate activations (row 0, true length) ─────────────
    let p_ids = read_i64(&probe_dir.join("probe_ids.i64"))?;
    let p_markers = read_i64(&probe_dir.join("probe_marker_pos.i64"))?;
    let meta: serde_json::Value =
        serde_json::from_slice(&std::fs::read(probe_dir.join("meta.json"))?)?;
    let qtype = meta["qtype"].as_i64().unwrap_or(0);
    let (enc, afe, head, logits) = model.run(&p_ids, &p_markers, qtype)?;
    let ref_enc = read_f32(&probe_dir.join("probe_encoder.f32"))?;
    let ref_afe = read_f32(&probe_dir.join("probe_after_type.f32"))?;
    let ref_head = read_f32(&probe_dir.join("probe_head.f32"))?;
    let ref_logits = read_f32(&probe_dir.join("probe_marker_logits.f32"))?;
    let our_enc: Vec<f32> = Vec::try_from(enc.view([-1]))?;
    let our_afe: Vec<f32> = Vec::try_from(afe.view([-1]))?;
    let our_head: Vec<f32> = Vec::try_from(head.view([-1]))?;
    let (de, _) = max_abs_diff(&our_enc, &ref_enc);
    let (da, _) = max_abs_diff(&our_afe, &ref_afe);
    let (dh, _) = max_abs_diff(&our_head, &ref_head);
    let (dl, _) = max_abs_diff(&logits, &ref_logits);
    println!("[probe] L={} markers={:?}", p_ids.len(), p_markers);
    println!("[probe] encoder_hidden   max|Δ| = {:.3e}", de);
    println!("[probe] after_type_emb   max|Δ| = {:.3e}", da);
    println!("[probe] head_hidden      max|Δ| = {:.3e}", dh);
    println!("[probe] marker_logits    max|Δ| = {:.3e}", dl);
    println!("[probe] rust logits  = {:?}", logits);
    println!("[probe] torch logits = {:?}", ref_logits);

    // ── end-to-end: all rows from the trace ──────────────────────────────
    let trace: Trace = serde_json::from_slice(&std::fs::read(&trace_path)?)?;
    println!(
        "\n[parity] {} rows from {}",
        trace.rows.len(),
        trace_path.display()
    );

    // warmup
    let _ = model.forward(
        &trace.rows[0].ids,
        &trace.rows[0].markers,
        trace.rows[0].qtype,
    )?;

    let mut worst = 0f32;
    let mut t_all = 0f64;
    for row in &trace.rows {
        let t = Instant::now();
        let logits = model.forward(&row.ids, &row.markers, row.qtype)?;
        t_all += t.elapsed().as_secs_f64();
        let expected: Vec<f32> = row.single_logits.iter().map(|&x| x as f32).collect();
        let (d, at) = max_abs_diff(&logits, &expected);
        worst = worst.max(d);
        println!(
            "  {:<18} L={:3} k={} max|Δ|={:.3e}  rust={:?} torch={:?}",
            row.qid,
            row.ids.len(),
            row.markers.len(),
            d,
            logits
                .iter()
                .map(|x| (x * 1e4).round() / 1e4)
                .collect::<Vec<_>>(),
            expected
                .iter()
                .map(|x| (x * 1e4).round() / 1e4)
                .collect::<Vec<_>>(),
        );
        let _ = at;
    }
    let n = trace.rows.len().max(1) as f64;
    println!(
        "\n[parity] worst max|Δ| = {:.3e}   (threshold 1e-3)   avg {:.1} ms/row",
        worst,
        1000.0 * t_all / n
    );

    // ── batched path (what the HTTP server uses) ─────────────────────────
    let inputs: Vec<SeqInput> = trace
        .rows
        .iter()
        .map(|r| SeqInput {
            ids: &r.ids,
            markers: &r.markers,
            qtype: r.qtype,
        })
        .collect();
    let _ = model.forward_batch(&inputs)?;
    let mut bt = f64::INFINITY;
    let mut outs = Vec::new();
    for _ in 0..5 {
        let t = Instant::now();
        let o = model.forward_batch(&inputs)?;
        bt = bt.min(t.elapsed().as_secs_f64());
        outs = o;
    }
    let mut bworst = 0f32;
    let mut aworst = 0f32;
    for (r, o) in trace.rows.iter().zip(outs.iter()) {
        let expected: Vec<f32> = r.logits.iter().map(|&x| x as f32).collect();
        let (d, _) = max_abs_diff(&o.logits, &expected);
        bworst = bworst.max(d);
        // act probability: softmax(act_logits)[0]
        let mx = o
            .act_logits
            .iter()
            .cloned()
            .fold(f32::NEG_INFINITY, f32::max);
        let ex: Vec<f32> = o.act_logits.iter().map(|v| (v - mx).exp()).collect();
        let sm: f32 = ex.iter().sum();
        let p0 = ex[0] / sm;
        if let Some(exp_act) = r.act_probability {
            aworst = aworst.max((p0 - exp_act as f32).abs());
        }
    }
    println!(
        "[parity] batched {} rows: worst max|Δ| (vs torch batched) = {:.3e}   {:.1} ms/request ({:.1} ms/question)",
        inputs.len(),
        bworst,
        1000.0 * bt,
        1000.0 * bt / n
    );
    println!("[parity] act_probability  worst max|Δ| = {:.3e}", aworst);

    if worst < 1e-3 && bworst < 1e-3 {
        println!("[parity] ✅ PASS");
    } else {
        println!("[parity] ❌ FAIL");
        std::process::exit(1);
    }
    Ok(())
}
