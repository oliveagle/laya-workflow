//! Bench / diagnostic subcommands. Rust replacements for the `bench/*.py`
//! tools — no Python on the tooling path.
//!
//! `bench backend-comparison` — side-by-side: offline heuristic backend vs a
//! live laya-tch @ /v1/systemone, on the same specs. Rust port of
//! `bench/backend_comparison.py`.

use anyhow::Result;
use serde_json::{json, Value};

pub struct BackendComparisonOptions {
    /// laya-tch base URL; empty string skips the live run.
    pub base_url: Option<String>,
    pub skip_heuristic: bool,
    pub skip_live: bool,
}

/// A single decision point: (spec file, state, expected label, description).
struct Case {
    spec: &'static str,
    state: Value,
    expected: &'static str,
    desc: &'static str,
}

fn cases() -> Vec<Case> {
    vec![
        Case {
            spec: "admission.json",
            state: json!({"observation": "Alice planted basil and wants a weekly reminder.", "recent_memories": []}),
            expected: "ALLOW",
            desc: "specific, recallable detail (preference)",
        },
        Case {
            spec: "admission.json",
            state: json!({"observation": "OK thanks", "recent_memories": []}),
            expected: "BLOCK",
            desc: "trivial ack ('ok', 'thanks')",
        },
        Case {
            spec: "admission.json",
            state: json!({"observation": "Mira prefers concise explanations.", "recent_memories": []}),
            expected: "ALLOW",
            desc: "preference signal",
        },
        Case {
            spec: "admission.json",
            state: json!({"observation": "trivial ack noted", "recent_memories": []}),
            expected: "BLOCK",
            desc: "trivial phrase ('trivial', 'noted')",
        },
        Case {
            spec: "admission.json",
            state: json!({"observation": "I was charged twice for March. Please refund the duplicate ASAP.", "recent_memories": []}),
            expected: "BLOCK",
            desc: "no obvious trivial needles (no ack words); heuristic blocks by absence",
        },
    ]
}

fn run_one(spec_path: &std::path::Path, state: &Value, live: bool, base_url: &Option<String>) -> (String, f64, f64, String) {
    let t0 = std::time::Instant::now();
    let wf = match crate::spec::load_file(&spec_path.to_string_lossy()) {
        Ok(w) => w,
        Err(e) => return ("ERR".into(), 0.0, 0.0, e.to_string()),
    };
    let backend: Box<dyn crate::workflow::Decide> = if live {
        match base_url {
            Some(u) => Box::new(crate::backend::LayaBackend::new(u)),
            None => Box::new(crate::backend::HeuristicBackend),
        }
    } else {
        Box::new(crate::backend::HeuristicBackend)
    };
    match wf.run(backend.as_ref(), state) {
        Ok(out) => {
            let v = out.to_json();
            let r = v.get("result").cloned().unwrap_or(Value::Null);
            let label = r.get("label").and_then(Value::as_str).unwrap_or("?").to_string();
            let conf = r.get("confidence").and_then(Value::as_f64).unwrap_or(0.0);
            let ans = r.get("action_answer").and_then(Value::as_str).unwrap_or("?").to_string();
            (label, conf, t0.elapsed().as_secs_f64() * 1000.0, ans)
        }
        Err(e) => ("ERR".into(), 0.0, t0.elapsed().as_secs_f64() * 1000.0, e.to_string()),
    }
}

pub fn backend_comparison(opts: &BackendComparisonOptions) -> Result<()> {
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("dsl/laya_mem");
    println!("laya-workflow CLI: in-process (lib)");
    println!("laya-tch base url: {}", opts.base_url.clone().unwrap_or_else(|| "(none)".into()));
    println!("laya-work_dir: /tmp/laya_work_dir");
    println!();

    let header = format!(
        "{:<55} {:<10} {:<8} {:<10} {:<22} {:<10}",
        "case", "expected", "heuristic", "h_ms", "laya-tch", "t_ms"
    );
    println!("{header}");
    println!("{}", "-".repeat(130));
    for c in cases() {
        let spec_path = root.join(c.spec);
        let (h_label, _, h_ms, _h_ans) = if opts.skip_heuristic {
            (String::new(), 0.0, 0.0, String::new())
        } else {
            run_one(&spec_path, &c.state, false, &opts.base_url)
        };
        let (t_label, _, t_ms, _t_ans) = if opts.skip_live || opts.base_url.is_none() {
            (String::new(), 0.0, 0.0, String::new())
        } else {
            run_one(&spec_path, &c.state, true, &opts.base_url)
        };
        let row = format!(
            "{:<55} {:<10} {:<8} {:<10} {:<22} {:<10}",
            &c.desc[..c.desc.len().min(55)],
            c.expected,
            h_label,
            format!("{h_ms:.0}"),
            t_label,
            format!("{t_ms:.0}"),
        );
        println!("{row}");
    }
    println!();
    println!("Latencies include in-process run + (live) HTTP roundtrip + one forward pass.");
    println!("Note: heuristic latency is *not* a useful baseline — it's a substring scan.");
    Ok(())
}

// ── gbnf_strict_stress: mutation corpus for the --gbnf-strict gate ─────────

pub struct GbnfStressOptions {
    pub base_url: String,
    pub n_per_class: usize,
    pub seed: u64,
    pub out: Option<String>,
}

const REALISTIC_BASE: &str = r#"{
  "state": "I was charged twice for March. Please refund the duplicate ASAP.",
  "questions": {
    "urgency": {
      "type": "score",
      "instructions": "How urgent is this?",
      "criteria": ["not urgent", "soon", "critical"]
    },
    "refund_requested": {
      "type": "noul",
      "instructions": "Does the user explicitly request a refund?"
    }
  }
}"#;

/// Simple deterministic PRNG (xorshift64), seedable.
struct Rng(u64);
impl Rng {
    fn new(seed: u64) -> Self {
        Self(seed.wrapping_mul(6364136223846793005).wrapping_add(1442695040888963407) | 1)
    }
    fn next_u64(&mut self) -> u64 {
        let mut x = self.0;
        x ^= x << 13;
        x ^= x >> 7;
        x ^= x << 17;
        self.0 = x;
        x
    }
    fn pick<'a, T>(&mut self, v: &'a [T]) -> &'a T {
        &v[(self.next_u64() % v.len() as u64) as usize]
    }
}

fn base_request() -> Value {
    serde_json::from_str(REALISTIC_BASE).unwrap()
}

fn mutations() -> Vec<(&'static str, fn(Value, &mut Rng) -> Value)> {
    vec![
        ("valid (control)", |p, _| p),
        ("missing_instructions", |mut p, rng| {
            let qid: Vec<String> = p["questions"].as_object().unwrap().keys().cloned().collect();
            let q = rng.pick(&qid).clone();
            p["questions"][&q].as_object_mut().unwrap().remove("instructions");
            p
        }),
        ("unknown_qtype", |mut p, rng| {
            let qid: Vec<String> = p["questions"].as_object().unwrap().keys().cloned().collect();
            let q = rng.pick(&qid).clone();
            let bad = ["choyce", "ranking", "", "wibble"];
            p["questions"][&q]["type"] = json!(*rng.pick(&bad));
            p
        }),
        ("missing_questions", |mut p, _| {
            p.as_object_mut().unwrap().remove("questions");
            p
        }),
        ("questions_not_object", |mut p, _| {
            p["questions"] = json!(["not", "an", "object"]);
            p
        }),
        ("missing_state", |mut p, _| {
            p.as_object_mut().unwrap().remove("state");
            p
        }),
        ("extra_top_level_key", |mut p, rng| {
            let chars: Vec<char> = "abc xyz?-_0123".chars().collect();
            let mut s = String::new();
            for _ in 0..8 { s.push(*rng.pick(&chars)); }
            p["surprise"] = json!(s);
            p
        }),
        ("malformed_json", |_, _| {
            json!({ "_malformed_": "{not: even: json" })
        }),
        ("deeply_nested_criteria", |mut p, _| {
            p["questions"]["weird"] = json!({
                "type": "choice", "instructions": "x",
                "criteria": {"a": {"deep": [1,2,3]}}
            });
            p
        }),
        ("huge_unicode", |mut p, _| {
            p["state"] = json!("🚨".repeat(200) + " test");
            p
        }),
    ]
}

fn hit(url: &str, payload: &Value, client: &ureq::Agent) -> (u16, String, f64) {
    let body = serde_json::to_vec(payload).unwrap_or_default();
    let url_full = format!("{url}/v1/systemone");
    let t0 = std::time::Instant::now();
    let resp = client
        .post(&url_full)
        .set("content-type", "application/json")
        .send_bytes(&body);
    let ms = t0.elapsed().as_secs_f64() * 1000.0;
    match resp {
        Ok(r) => {
            let status = r.status();
            let text = r.into_string().unwrap_or_default();
            (status, text, ms)
        }
        Err(ureq::Error::Status(code, r)) => {
            let text = r.into_string().unwrap_or_default();
            (code, text, ms)
        }
        Err(e) => (0, format!("transport: {e}"), ms),
    }
}

fn check_invariants(resp: &Value) -> Vec<String> {
    let mut bad = Vec::new();
    if let Some(answers) = resp.get("answers").and_then(Value::as_object) {
        for (qid, ans) in answers {
            let t = ans.get("type").and_then(Value::as_str).unwrap_or("");
            if t == "choice" {
                let keys: Vec<&str> = ans
                    .get("probabilities")
                    .and_then(Value::as_object)
                    .map(|m| m.keys().map(|s| s.as_str()).collect())
                    .unwrap_or_default();
                let chosen = ans.get("choice").and_then(Value::as_str).unwrap_or("");
                if chosen.is_empty() {
                    bad.push(format!("{qid}: choice is empty"));
                }
                if !chosen.is_empty() && !keys.contains(&chosen) {
                    bad.push(format!("{qid}: choice {chosen:?} not in declared keys {keys:?}"));
                }
                if let Some(ps) = ans.get("probabilities").and_then(Value::as_object) {
                    let s: f64 = ps.values().filter_map(Value::as_f64).sum();
                    if (s - 1.0).abs() > 0.01 {
                        bad.push(format!("{qid}: prob sum {s:.4} != 1.0"));
                    }
                }
            } else if t == "score" {
                let idx = ans
                    .get("score")
                    .and_then(Value::as_f64)
                    .map(|v| v.round() as i64)
                    .unwrap_or(-1);
                let legend_len = ans
                    .get("legend")
                    .and_then(Value::as_array)
                    .map(|a| a.len())
                    .unwrap_or(0);
                if idx as usize >= legend_len {
                    bad.push(format!("{qid}: score idx {idx} >= legend len {legend_len}"));
                }
                if idx < 0 {
                    bad.push(format!("{qid}: score {} out of range",
                        ans.get("score").map(|v| v.to_string()).unwrap_or_default()));
                }
            }
            if let Some(conf) = ans.get("confidence").and_then(Value::as_f64) {
                if !(0.0..=1.0).contains(&conf) {
                    bad.push(format!("{qid}: confidence {conf} not in [0,1]"));
                }
            }
        }
    }
    bad
}

fn rejection_bucket(error_msg: &str) -> &'static str {
    if !error_msg.contains("gbnf") && !error_msg.to_lowercase().contains("gbnf") {
        return "non-gbnf-400";
    }
    if error_msg.contains("root rule") {
        return "root_not_found";
    }
    "gbnf-structural"
}

fn percentile(sorted: &[f64], p: f64) -> f64 {
    if sorted.is_empty() { return 0.0; }
    let idx = ((sorted.len() as f64) * p).min((sorted.len() - 1) as f64) as usize;
    sorted[idx]
}

pub fn gbnf_stress(opts: &GbnfStressOptions) -> Result<u32> {
    let client = ureq::AgentBuilder::new().timeout(std::time::Duration::from_secs(30)).build();
    let mut rng = Rng::new(opts.seed);
    let muts = mutations();

    // Build the corpus: n_per_class of each mutation.
    let mut corpus: Vec<(&str, Value)> = Vec::new();
    for (label, gen) in &muts {
        for _ in 0..opts.n_per_class {
            let base = base_request();
            corpus.push((label, gen(base, &mut rng)));
        }
    }
    // Shuffle
    for i in (1..corpus.len()).rev() {
        let j = (rng.next_u64() % (i as u64 + 1)) as usize;
        corpus.swap(i, j);
    }
    println!(
        "corpus: {} requests across {} mutation classes",
        corpus.len(),
        muts.len()
    );

    #[derive(Default)]
    struct ClassStat {
        n: usize,
        status_400: usize,
        status_200: usize,
        status_5xx: usize,
        invar: Vec<String>,
        lat: Vec<f64>,
    }
    let mut stats: Vec<(String, ClassStat)> =
        muts.iter().map(|(l, _)| (l.to_string(), ClassStat::default())).collect();
    let mut buckets: std::collections::HashMap<String, usize> = Default::default();
    let mut all_lat: Vec<f64> = Vec::new();

    for (label, payload) in &corpus {
        let idx = stats.iter_mut().position(|(l, _)| l == label).unwrap();
        let s = &mut stats[idx].1;
        s.n += 1;
        let (status, body_text, ms) = hit(&opts.base_url, payload, &client);
        s.lat.push(ms);
        all_lat.push(ms);
        match status {
            200 => {
                s.status_200 += 1;
                let v: Value = serde_json::from_str(&body_text).unwrap_or(Value::Null);
                s.invar.extend(check_invariants(&v));
            }
            400 => {
                s.status_400 += 1;
                *buckets.entry(rejection_bucket(&body_text).to_string()).or_insert(0) += 1;
            }
            x if x >= 500 => s.status_5xx += 1,
            _ => s.invar.push(format!("unexpected status {status}")),
        }
    }

    // Report.
    let mut lines: Vec<String> = Vec::new();
    lines.push("# laya-tch --gbnf-strict stress report".into());
    lines.push(String::new());
    lines.push(format!("endpoint: `{}`", opts.base_url));
    lines.push(format!(
        "corpus size: {} (seed={}, {}/class × {} classes)",
        corpus.len(), opts.seed, opts.n_per_class, muts.len()
    ));
    lines.push(String::new());
    lines.push("## Per-class outcome".into());
    lines.push(String::new());
    lines.push("| class | n | 200 | 400 | 5xx | invar violations | p50 ms | p95 ms |".into());
    lines.push("|---|---:|---:|---:|---:|---|---:|---:|".into());
    for (label, s) in &stats {
        let mut lats = s.lat.clone();
        lats.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
        let p50 = if lats.is_empty() { 0.0 } else { lats[lats.len() / 2] };
        let p95 = percentile(&lats, 0.95);
        let mut inv = s.invar.iter().take(3).cloned().collect::<Vec<_>>().join("; ");
        if s.invar.len() > 3 {
            inv += &format!(" (+{} more)", s.invar.len() - 3);
        }
        lines.push(format!(
            "| `{label}` | {} | {} | {} | {} | {} | {p50:.1} | {p95:.1} |",
            s.n, s.status_200, s.status_400, s.status_5xx,
            if inv.is_empty() { "—".into() } else { inv }
        ));
    }
    lines.push(String::new());

    lines.push("## 400 rejection buckets".into());
    lines.push(String::new());
    if buckets.is_empty() {
        lines.push("(no 400s in this corpus)".into());
    } else {
        let mut kv: Vec<_> = buckets.iter().collect();
        kv.sort_by_key(|(_, v)| std::cmp::Reverse(**v));
        for (k, v) in kv {
            lines.push(format!("* `{k}`: {v}"));
        }
    }
    lines.push(String::new());

    if !all_lat.is_empty() {
        all_lat.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
        let p50 = all_lat[all_lat.len() / 2];
        let p95 = percentile(&all_lat, 0.95);
        let max = all_lat[all_lat.len() - 1];
        lines.push("## Latency".into());
        lines.push(format!("  p50 = {p50:.1} ms, p95 = {p95:.1} ms, max = {max:.1} ms"));
        lines.push(String::new());
    }

    let total_5xx: usize = stats.iter().map(|(_, s)| s.status_5xx).sum();
    let total_invar: usize = stats.iter().map(|(_, s)| s.invar.len()).sum();
    lines.push("## Headline".into());
    lines.push(format!("* 5xx responses: **{total_5xx}** (target: 0)"));
    lines.push(format!(
        "* invariant violations on 200 responses: **{total_invar}** (target: 0)"
    ));

    let text = lines.join("\n") + "\n";
    print!("{text}");
    if let Some(out) = &opts.out {
        let p = std::path::Path::new(out);
        if let Some(parent) = p.parent() {
            std::fs::create_dir_all(parent)?;
        }
        std::fs::write(p, &text)?;
        println!("wrote {out}");
    }
    if total_5xx > 0 || total_invar > 0 {
        anyhow::bail!("stress found {total_5xx} 5xx + {total_invar} invariant violations");
    }
    Ok(0)
}
