//! Search rerank comparison harness (jev-search 场景).
//!
//! Re-runs bm25 + laya_ft noul against the same 20-item candidate pools, loads
//! the committed Qwen3-Reranker-0.6B per-row scores (real CPU fp32 run), and
//! writes a fresh aggregated markdown report.
//!
//! Usage:
//!   cargo build --release -p search-rerank
//!   ./target/release/search-rerank [--dataset PATH] [--prev-reports PATH] [--laya-url URL] [--out DIR]

use anyhow::{bail, Context, Result};
use regex_lite::Regex;
use serde::{Deserialize, Serialize};
use std::path::PathBuf;
use std::time::Instant;

// ─── data types ───────────────────────────────────────────────────────

#[derive(Deserialize)]
struct PoolItem {
    source: String,
    title: String,
    snippet: String,
}

#[derive(Deserialize)]
struct QueryRow {
    q: String,
    ans: String,
    gold_index: usize,
    items: Vec<PoolItem>,
}

#[derive(Deserialize)]
struct Dataset {
    n: usize,
    pool_size: usize,
    seed: u64,
    queries: Vec<QueryRow>,
}

#[derive(Deserialize)]
struct LayaAnswer {
    #[serde(default)]
    noul: Option<f64>,
}

#[derive(Deserialize)]
struct LayaResponse {
    answers: serde_json::Map<String, serde_json::Value>,
}

#[derive(Deserialize, Clone, Debug)]
struct PrevPerRow {
    #[serde(default)]
    rank_of_gold: Option<usize>,
    #[serde(default)]
    lat_ms: f64,
    #[serde(default)]
    ndcg5: f64,
    #[serde(default)]
    p1: serde_json::Value,
}

#[derive(Deserialize)]
struct PrevReport {
    results: serde_json::Map<String, serde_json::Value>,
}

#[derive(Serialize, Deserialize)]
struct AggRow {
    q: String,
    gold_rank: Option<usize>,
    p1: bool,
    p3: bool,
    p5: bool,
    rr: f64,
    ndcg5: f64,
    lat_ms: f64,
}

#[derive(Serialize, Deserialize)]
struct Agg {
    n: usize,
    p1: f64,
    p3: f64,
    p5: f64,
    mrr5: f64,
    ndcg5: f64,
    avg_ms: f64,
    p95_ms: f64,
    per_row: Vec<AggRow>,
}

// ─── bm25 (term overlap proxy) ─────────────────────────────────────────

fn norm_tokens(s: &str) -> Vec<String> {
    let re = Regex::new(r"[^a-z0-9 ]").unwrap();
    let lower = s.to_lowercase();
    re.replace_all(&lower, " ")
        .split_whitespace()
        .map(|w| w.to_string())
        .collect()
}

fn bm25_scores(query: &str, docs: &[&PoolItem]) -> Vec<f64> {
    let q: std::collections::HashSet<String> = norm_tokens(query).into_iter().collect();
    docs.iter()
        .map(|d| {
            let toks: std::collections::HashSet<String> =
                norm_tokens(&format!("{} {}", d.title, d.snippet))
                    .into_iter()
                    .collect();
            (q.intersection(&toks).count()) as f64
        })
        .collect()
}

// ─── laya noul rerank ─────────────────────────────────────────────────

fn laya_rank(query: &str, docs: &[PoolItem], url: &str) -> Result<(Vec<f64>, f64)> {
    let mut questions = serde_json::Map::new();
    for i in 0..docs.len() {
        questions.insert(
            format!("r{}", i),
            serde_json::json!({
                "type": "noul",
                "instructions": format!("Is `results[{}]` about the subject the user asked for in `request`?", i),
                "criteria": {
                    "true": "The title or snippet discusses the same subject the user asked about, even briefly or as one of several topics",
                    "false": "The result is about something else that only shares words with the request (a different meaning of the same word, a different product, a person with the same name) or is unrelated",
                },
            }),
        );
    }
    let results: Vec<serde_json::Value> = docs
        .iter()
        .map(|d| {
            serde_json::json!({
                "source": d.source,
                "title": d.title,
                "snippet": d.snippet,
            })
        })
        .collect();
    let body = serde_json::json!({
        "state": { "request": query, "results": results },
        "model": "laya-ft",
        "questions": questions,
    });
    let t0 = Instant::now();
    let resp: LayaResponse = ureq::post(url)
        .timeout(std::time::Duration::from_secs(60))
        .send_json(body)
        .context("laya /v1/systemone request failed")?
        .into_json()?;
    let dt = t0.elapsed().as_secs_f64() * 1000.0;
    let mut scores = Vec::with_capacity(docs.len());
    for i in 0..docs.len() {
        let key = format!("r{}", i);
        let a: Option<LayaAnswer> = resp
            .answers
            .get(&key)
            .and_then(|v| serde_json::from_value(v.clone()).ok());
        scores.push(a.and_then(|a| a.noul).unwrap_or(0.0));
    }
    Ok((scores, dt))
}

// ─── helpers ──────────────────────────────────────────────────────────

fn order_from_scores(scores: &[f64]) -> Vec<usize> {
    let mut idx: Vec<usize> = (0..scores.len()).collect();
    idx.sort_by(|a, b| scores[*b].partial_cmp(&scores[*a]).unwrap());
    idx
}

fn dcg(labels: &[f64], k: usize) -> f64 {
    labels
        .iter()
        .take(k)
        .enumerate()
        .map(|(i, &l)| l / ((i + 2) as f64).log2())
        .sum()
}

fn eval(gold_idx: usize, order: &[usize], k: usize, lat_ms: f64, q: &str) -> AggRow {
    let rank_of_gold = order.iter().position(|&i| i == gold_idx).map(|p| p + 1);
    let p1 = rank_of_gold == Some(1);
    let p3 = rank_of_gold.map(|r| r <= 3).unwrap_or(false);
    let p5 = rank_of_gold.map(|r| r <= 5).unwrap_or(false);
    let rr = match rank_of_gold {
        Some(r) if r <= k => 1.0 / r as f64,
        _ => 0.0,
    };
    let got: Vec<f64> = order
        .iter()
        .map(|&i| if i == gold_idx { 1.0 } else { 0.0 })
        .collect();
    let ideal: Vec<f64> = (0..order.len())
        .map(|i| if i == 0 { 1.0 } else { 0.0 })
        .collect();
    let idcg = dcg(&ideal, k);
    let ndcg = if idcg > 0.0 { dcg(&got, k) / idcg } else { 0.0 };
    AggRow {
        q: q.chars().take(60).collect(),
        gold_rank: rank_of_gold,
        p1,
        p3,
        p5,
        rr,
        ndcg5: ndcg,
        lat_ms,
    }
}

fn aggregate(rows: Vec<AggRow>, n: usize) -> Agg {
    let p1 = rows.iter().filter(|r| r.p1).count() as f64 / n as f64;
    let p3 = rows.iter().filter(|r| r.p3).count() as f64 / n as f64;
    let p5 = rows.iter().filter(|r| r.p5).count() as f64 / n as f64;
    let mrr5: f64 = rows.iter().map(|r| r.rr).sum::<f64>() / n as f64;
    let ndcg5: f64 = rows.iter().map(|r| r.ndcg5).sum::<f64>() / n as f64;
    let lats: Vec<f64> = rows.iter().map(|r| r.lat_ms).collect();
    let avg = lats.iter().sum::<f64>() / n as f64;
    let mut sorted = lats.clone();
    sorted.sort_by(|a, b| a.partial_cmp(b).unwrap());
    let p95 = sorted.get((n * 95 / 100).saturating_sub(1)).copied().unwrap_or(avg);
    Agg {
        n,
        p1,
        p3,
        p5,
        mrr5,
        ndcg5,
        avg_ms: avg,
        p95_ms: p95,
        per_row: rows,
    }
}

// ─── main ─────────────────────────────────────────────────────────────

fn parse_args() -> Args {
    let mut dataset = PathBuf::from("bench/search_rerank/data/jev_rerank_dataset.json");
    let mut prev_reports = PathBuf::from("bench/search_rerank/reports/compare.json");
    let mut laya_url = String::from("http://127.0.0.1:18405/v1/systemone");
    let mut out = PathBuf::from("bench/search_rerank/reports");
    let mut i = 1;
    while i < std::env::args().len() {
        match std::env::args().nth(i).as_deref() {
            Some("--dataset") => {
                dataset = PathBuf::from(std::env::args().nth(i + 1).unwrap());
                i += 2;
            }
            Some("--prev-reports") => {
                prev_reports = PathBuf::from(std::env::args().nth(i + 1).unwrap());
                i += 2;
            }
            Some("--laya-url") => {
                laya_url = std::env::args().nth(i + 1).unwrap();
                i += 2;
            }
            Some("--out") => {
                out = PathBuf::from(std::env::args().nth(i + 1).unwrap());
                i += 2;
            }
            _ => i += 1,
        }
    }
    Args {
        dataset,
        prev_reports,
        laya_url,
        out,
    }
}

struct Args {
    dataset: PathBuf,
    prev_reports: PathBuf,
    laya_url: String,
    out: PathBuf,
}

fn main() -> Result<()> {
    let args = parse_args();
    let dataset: Dataset =
        serde_json::from_str(&std::fs::read_to_string(&args.dataset).with_context(|| {
            format!("cannot read dataset at {}", args.dataset.display())
        })?)
        .context("cannot parse dataset JSON")?;
    let n = dataset.n;
    let k = 5;
    println!(
        "dataset: n={} pool={} seed={} from {}",
        n,
        dataset.pool_size,
        dataset.seed,
        args.dataset.display()
    );

    // ── prev reports (committed qwen scores) ──
    let prev_raw =
        std::fs::read_to_string(&args.prev_reports).context("cannot read prev reports")?;
    let prev: PrevReport = serde_json::from_str(&prev_raw).context("cannot parse prev reports")?;

    // ── three rankers ──
    let mut all = serde_json::Map::new();

    // bm25
    {
        let mut rows = Vec::with_capacity(n);
        for q in &dataset.queries {
            let docs: Vec<&PoolItem> = q.items.iter().collect();
            let scores = bm25_scores(&q.q, &docs);
            let order = order_from_scores(&scores);
            rows.push(eval(q.gold_index, &order, k, 0.0, &q.q));
        }
        all.insert("bm25".into(), serde_json::to_value(aggregate(rows, n))?);
    }

    // laya
    {
        let mut rows = Vec::with_capacity(n);
        for q in &dataset.queries {
            let (scores, dt) = laya_rank(&q.q, &q.items, &args.laya_url)?;
            let order = order_from_scores(&scores);
            rows.push(eval(q.gold_index, &order, k, dt, &q.q));
        }
        all.insert("laya".into(), serde_json::to_value(aggregate(rows, n))?);
    }

    // qwen (load from committed report)
    {
        let entry = prev
            .results
            .get("qwen")
            .context("prev reports missing 'qwen' key")?;
        let per_row: Vec<PrevPerRow> = serde_json::from_value(
            entry
                .get("per_row")
                .cloned()
                .context("prev qwen report missing per_row")?,
        )?;
        if per_row.len() != n {
            bail!(
                "prev qwen per_row len {} != dataset n {}",
                per_row.len(),
                n
            );
        }
        let rows: Vec<AggRow> = per_row
            .iter()
            .zip(dataset.queries.iter())
            .map(|(p, q)| AggRow {
                q: q.q.chars().take(60).collect(),
                gold_rank: p.rank_of_gold,
                p1: p.rank_of_gold == Some(1),
                p3: p.rank_of_gold.map(|r| r <= 3).unwrap_or(false),
                p5: p.rank_of_gold.map(|r| r <= 5).unwrap_or(false),
                rr: match p.rank_of_gold {
                    Some(r) if r <= k => 1.0 / r as f64,
                    _ => 0.0,
                },
                ndcg5: p.ndcg5,
                lat_ms: p.lat_ms,
            })
            .collect();
        all.insert("qwen".into(), serde_json::to_value(aggregate(rows, n))?);
    }

    // ── write JSON + markdown ──
    let out_dir = &args.out;
    std::fs::create_dir_all(out_dir)?;
    let json_path = out_dir.join("compare_rust.json");
    std::fs::write(&json_path, serde_json::to_string_pretty(&all)?)?;
    println!("wrote {}", json_path.display());

    let md = format_md(&all, n);
    let md_path = out_dir.join("compare_rust.md");
    std::fs::write(&md_path, &md)?;
    println!("wrote {}", md_path.display());
    println!("{}", md);
    Ok(())
}

fn format_md(all: &serde_json::Map<String, serde_json::Value>, n: usize) -> String {
    let mut s = format!("# search rerank comparison (Rust harness, n={})\n\n", n);
    s.push_str("| ranker | P@1 | P@3 | P@5 | MRR@5 | nDCG@5 | avg ms | p95 ms |\n");
    s.push_str("|---|---|---|---|---|---|---|---|\n");
    for name in ["bm25", "laya", "qwen"] {
        let agg: std::result::Result<Agg, _> = serde_json::from_value(all.get(name).cloned().unwrap());
        if let Ok(a) = agg {
            s.push_str(&format!(
                "| {} | {:.3} | {:.3} | {:.3} | {:.3} | {:.3} | {:.0} | {:.0} |\n",
                name, a.p1, a.p3, a.p5, a.mrr5, a.ndcg5, a.avg_ms, a.p95_ms
            ));
        }
    }
    s.push_str("\n");
    s.push_str("- bm25: term-overlap baseline (no rerank)\n");
    s.push_str("- laya: laya_ft_server `/v1/systemone` noul batch rerank (same prompt as laya-search/src/judge.js)\n");
    s.push_str("- qwen: Qwen3-Reranker-0.6B cross-encoder yes/no logit (real CPU fp32 run, loaded from committed per-row data)\n");
    s
}
