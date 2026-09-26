//! Decision backends for the workflow engine.
//!
//! * `LayaBackend`  — talks to a running `laya-tch` server over HTTP
//!   (`POST /v1/systemone`), parsing the Jev-compatible response into a `Verdict`.
//! * `ScriptedBackend` — deterministic canned answers for tests / dry runs.
//! * `DecisionMap` — a tiny helper to build a `Verdict` from raw answers.

use anyhow::{anyhow, Result};
use serde_json::{Map, Value};
use std::collections::HashMap;
use std::time::Instant;

use crate::workflow::{Decide, Decision, Verdict};
/// HTTP client for `laya-tch`'s `/v1/systemone`.
pub struct LayaBackend {
    base_url: String,
    agent: ureq::Agent,
}

impl LayaBackend {
    pub fn new(base_url: &str) -> Self {
        let agent = ureq::AgentBuilder::new()
            .timeout(std::time::Duration::from_secs(600))
            .build();
        Self {
            base_url: base_url.trim_end_matches('/').to_string(),
            agent,
        }
    }
}

fn json_to_map(v: &Value) -> Map<String, Value> {
    v.as_object().cloned().unwrap_or_default()
}

impl Decide for LayaBackend {
    fn decide(&self, state: &Value, questions: &Value) -> Result<Verdict> {
        let t0 = Instant::now();
        let body = serde_json::json!({ "state": state, "questions": questions });
        let resp = self
            .agent
            .post(&format!("{}/v1/systemone", self.base_url))
            .set("Content-Type", "application/json")
            .send_string(&body.to_string())
            .map_err(|e| anyhow!("laya backend request failed: {e}"))?;
        let text = resp
            .into_string()
            .map_err(|e| anyhow!("laya backend read failed: {e}"))?;
        let raw: Value = serde_json::from_str(&text)?;
        let mut verdict = verdict_from_response(&raw)?;
        verdict.latency_ms = t0.elapsed().as_secs_f64() * 1000.0;
        Ok(verdict)
    }
}

/// Parse a Jev-compatible `{"answers": …, "usage": …}` payload into a `Verdict`.
pub fn verdict_from_response(raw: &Value) -> Result<Verdict> {
    let answers = raw
        .get("answers")
        .and_then(|v| v.as_object())
        .ok_or_else(|| anyhow!("response missing 'answers'"))?;
    let mut out = HashMap::new();
    for (qid, ans) in answers {
        let qtype = ans.get("type").and_then(|v| v.as_str()).unwrap_or("");
        let (answer, probabilities, confidence) = match qtype {
            "choice" => (
                ans.get("choice").cloned().unwrap_or(Value::Null),
                json_to_map(ans.get("probabilities").unwrap_or(&Value::Null)),
                ans.get("confidence").and_then(|v| v.as_f64()).unwrap_or(0.0),
            ),
            "score" => (
                ans.get("score").cloned().unwrap_or(Value::Null),
                json_to_map(ans.get("probabilities").unwrap_or(&Value::Null)),
                ans.get("confidence").and_then(|v| v.as_f64()).unwrap_or(0.0),
            ),
            "noul" => {
                let p = ans.get("noul").and_then(|v| v.as_f64()).unwrap_or(0.0);
                let mut probs = Map::new();
                probs.insert("false".to_string(), serde_json::json!(1.0 - p));
                probs.insert("true".to_string(), serde_json::json!(p));
                let conf = ans
                    .get("confidence")
                    .and_then(|v| v.as_f64())
                    .unwrap_or(p.max(1.0 - p));
                (serde_json::json!(p), probs, conf)
            }
            other => return Err(anyhow!("unknown answer type {other:?} for {qid}")),
        };
        out.insert(
            qid.clone(),
            Decision { answer, probabilities, confidence },
        );
    }
    let n_tokens = raw
        .get("usage")
        .and_then(|u| u.get("input_tokens"))
        .and_then(|v| v.as_u64())
        .unwrap_or(0) as usize;
    Ok(Verdict { answers: out, input_tokens: n_tokens, latency_ms: 0.0 })
}

/// Deterministic backend: returns canned `Verdict`s, one per call (then repeats
/// the last). Useful for tests and offline workflow dry runs.
pub struct ScriptedBackend {
    script: Vec<Verdict>,
    cursor: std::cell::Cell<usize>,
}

impl ScriptedBackend {
    pub fn new(script: Vec<Verdict>) -> Self {
        Self { script, cursor: std::cell::Cell::new(0) }
    }
    pub fn calls(&self) -> usize {
        self.cursor.get()
    }
}

impl Decide for ScriptedBackend {
    fn decide(&self, _state: &Value, _questions: &Value) -> Result<Verdict> {
        let i = self.cursor.get();
        let v = self
            .script
            .get(i)
            .or_else(|| self.script.last())
            .cloned()
            .ok_or_else(|| anyhow!("scripted backend has no verdicts"))?;
        self.cursor.set(i + 1);
        Ok(v)
    }
}

/// Deterministic rule-based backend for offline runs and CI.
///
/// It reproduces the *decisions* the neural model makes on the app reference
/// cases (same answer keys and rough orderings) so the whole workflow graph can
/// be exercised without a model or server. It is explicitly **not** the model —
/// parity tests use `LayaBackend`.
pub struct HeuristicBackend;

/// Count words in the **user-authored prose** of a state, not the JSON envelope.
///
/// Scoring `clarity` off the serialised state counted field names and quotes, so
/// even an empty message looked long. This reads the common text-bearing fields
/// and falls back to the raw value when the state is a bare string.
fn prose_words(state: &Value) -> usize {
    let mut text = String::new();
    if let Some(o) = state.as_object() {
        for k in ["text", "body", "draft", "message", "content"] {
            if let Some(v) = o.get(k).and_then(|v| v.as_str()) {
                text.push(' ');
                text.push_str(v);
            }
        }
    } else if let Some(s) = state.as_str() {
        text.push_str(s);
    }
    text.split_whitespace().count()
}

/// Case-insensitive substring test with **word-boundary awareness**.
///
/// Plain `contains` produced real false positives: `"need"` matches inside
/// `"No action needed."`, which flipped `needs_reply` to yes for a message that
/// explicitly says no reply is needed. Multi-word needles still match as
/// substrings (they carry their own context), but single words must sit on a
/// word boundary.
fn contains_any(s: &str, needles: &[&str]) -> bool {
    let l = s.to_lowercase();
    needles.iter().any(|n| {
        let n = n.to_lowercase();
        if n.contains(' ') || n.contains('-') || n.contains('/') || n.contains('@') || n.contains('.') {
            return l.contains(&n);
        }
        // Single token: require boundaries so "need" != "needed".
        let mut from = 0usize;
        while let Some(i) = l[from..].find(&n) {
            let start = from + i;
            let end = start + n.len();
            let before_ok = start == 0
                || !l[..start].chars().next_back().map(|c| c.is_alphanumeric()).unwrap_or(false);
            let after_ok = end >= l.len()
                || !l[end..].chars().next().map(|c| c.is_alphanumeric()).unwrap_or(false);
            if before_ok && after_ok {
                return true;
            }
            from = start + n.len();
        }
        false
    })
}

impl Decide for HeuristicBackend {
    fn decide(&self, state: &Value, questions: &Value) -> Result<Verdict> {
        let text = serde_json::to_string(state).unwrap_or_default();
        let qs = questions.as_object().cloned().unwrap_or_default();
        let mut answers = HashMap::new();
        for (qid, qdef) in &qs {
            let qtype = qdef.get("type").and_then(|v| v.as_str()).unwrap_or("choice");
            let (answer, probabilities, confidence) = match qid.as_str() {
                "is_destructive" => {
                    let p = if contains_any(&text, &["rm ", "drop ", "delete", "truncate", "wipe", "format", "kill"]) { 0.95 } else { 0.1 };
                    bchoice(p)
                }
                "is_exfiltration" => {
                    // Two flavours of the same risk:
                    //   * sending local data outward (curl/wget/scp/upload);
                    //   * reading a credential-bearing local file, which is the
                    //     precursor to exfiltration. `cat /etc/passwd` matches
                    //     its stated intent, so it must NOT be an intent
                    //     mismatch — it belongs here, at a caution level that
                    //     routes to CONFIRM rather than BLOCK.
                    let outward = contains_any(&text, &["curl ", "wget ", "scp ", "@s.txt", "evil.example", "upload"]);
                    let sensitive_read = contains_any(&text, &["/etc/passwd", "/etc/shadow", "id_rsa", ".ssh/"]);
                    let p = if outward {
                        0.9
                    } else if sensitive_read {
                        0.45
                    } else {
                        0.1
                    };
                    bchoice(p)
                }
                "is_hate" => bchoice(if contains_any(&text, &["vermin", "deport", "slur"]) { 0.9 } else { 0.05 }),
                "is_threat" => bchoice(if contains_any(&text, &["find you", "watch your back", "kill you"]) { 0.9 } else { 0.05 }),
                "is_pii" => bchoice(if contains_any(&text, &["ssn", "123-45-6789", "credit card"]) { 0.9 } else { 0.05 }),
                "is_spam" | "spam" => bchoice(if contains_any(&text, &["get rich", "limited offer", "buy now", "5000/week"]) { 0.9 } else { 0.1 }),
                "phishing" => bchoice(if contains_any(&text, &["confirm credentials", "suspended", "verify your account", "http://evil"]) { 0.9 } else { 0.05 }),
                "severity" => bscore(if contains_any(&text, &["vermin", "watch your back", "get rich", "123-45-6789"]) { 2.0 } else { 0.5 }),
                "has_typo" => bchoice(if contains_any(&text, &["discusion", "attched", "pls"]) { 0.8 } else { 0.1 }),
                "is_sensitive" => bchoice(if contains_any(&text, &["i've decided to leave", "unacceptable", "2-week plan"]) { 0.8 } else { 0.2 }),
                // `tone` is a 4-level score (angry/tense/neutral/warm). Only
                // looking for two hostile phrases left a resignation letter at
                // "neutral", so a high-stakes message was never routed to
                // "sleep". Fold in the other negative-affect signals this
                // backend already extracts.
                "tone" => {
                    let hostile = contains_any(&text, &["unacceptable", "fix now", "angry", "furious"]);
                    let heavy = contains_any(
                        &text,
                        &["decided to leave", "resign", "i quit", "termination", "layoff"],
                    );
                    bscore(if hostile {
                        0.3
                    } else if heavy {
                        0.8
                    } else {
                        2.0
                    })
                }
                // `clarity` is a 4-level score (confusing/ambiguous/clear/crystal
                // clear). Keying only off typo words reported "crystal clear" for
                // a one-word "ok" reply. Brevity is the signal that a message
                // does not convey enough — measure the prose, not the JSON.
                "clarity" => {
                    let typo = contains_any(&text, &["discusion", "attched"]);
                    let prose = prose_words(state);
                    let v = if typo {
                        1.2
                    } else if prose < 5 {
                        0.4 // too short to be clear: "ok"
                    } else if prose < 12 {
                        1.6
                    } else {
                        2.5
                    };
                    bscore(v)
                }
                "professionalism" => bscore(1.5),
                "urgency" => bscore(if contains_any(&text, &["outage", "refund today", "before sprint end"]) { 2.0 } else { 0.8 }),
                "sentiment" => bscore(if contains_any(&text, &["unacceptable", "angry"]) { 0.3 } else { 2.0 }),
                "needs_reply" => bchoice(if contains_any(&text, &["please", "need", "refund"]) { 0.7 } else { 0.2 }),
                // `intent_match` asks "does the command match the stated intent?".
                // Unlike the other questions its **A** pole is the positive one
                // ("yes, it does what the user asked"), so a plain `bchoice(0.9)`
                // was inverted: it emitted "no, it does something dangerous" for
                // every command, and agent_gate's `intent_p < 0.5` branch then
                // blocked everything. Default to A (matches) and downgrade only
                // when the command is a known mismatch pattern, where the stated
                // intent is benign but the command is not.
                // `intent_match` asks "does the command match the stated intent?".
                // Unlike the other questions its **A** pole is the positive one
                // ("yes, it does what the user asked"), so a plain `bchoice(0.9)`
                // was inverted: it emitted "no, it does something dangerous" for
                // every command, and agent_gate's `intent_p < 0.5` branch then
                // blocked everything. Default to A (matches) and downgrade only
                // when the command is a known mismatch pattern, where the stated
                // intent is benign but the command is not.
                "intent_match" => {
                    let mismatch = contains_any(
                        &text,
                        &[
                            "free up disk space",     // `sudo rm -rf /`
                            "reset dev database",     // `drop database prod`
                            "clean the project",      // `rm -rf ./build`
                            "send webhook",           // `curl ... -d @s.txt`
                        ],
                    );
                    // A = matches intent, B = diverges/dangerous.
                    bchoice(if mismatch { 0.9 } else { 0.1 })
                }
                // Both `email_triage` and `content_moderation` name their routing
                // question `category`, but with *different* criteria sets. With no
                // branch here `default_choice` always returned the first key
                // ("billing" / "safe"), collapsing every case to one label. The
                // criteria tell us which app we are answering for.
                "category" => {
                    let is_moderation = qdef
                        .get("criteria")
                        .and_then(|c| c.as_object())
                        .map(|o| o.contains_key("hateful"))
                        .unwrap_or(false);
                    if is_moderation {
                        let pick = if contains_any(&text, &["vermin", "deport", "slur"]) {
                            "hateful"
                        } else if contains_any(&text, &["find you", "watch your back", "kill you"]) {
                            "threatening"
                        } else if contains_any(&text, &["ssn", "123-45-6789", "credit card"]) {
                            "leaky_pii"
                        } else if contains_any(&text, &["get rich", "limited offer", "buy now", "5000/week"]) {
                            "spam"
                        } else {
                            "safe"
                        };
                        let keys = ["safe", "hateful", "threatening", "leaky_pii", "spam", "off_topic"];
                        let mut m = Map::new();
                        for k in keys {
                            m.insert(k.to_string(), serde_json::json!(if k == pick { 0.9 } else { 0.02 }));
                        }
                        (serde_json::json!(pick), m, 0.9)
                    } else {
                        let pick = if contains_any(&text, &["outage", "500", "hotfix", "incident", "prod", "unavailable"]) {
                            "technical"
                        } else if contains_any(&text, &["refund", "invoice", "billed", "payment", "duplicate charge"]) {
                            "billing"
                        } else if contains_any(&text, &["pricing", "demo", "purchase", "quote", "trial"]) {
                            "sales"
                        } else if contains_any(&text, &["login", "password", "access", "profile", "reset"]) {
                            "account"
                        } else if contains_any(&text, &["leave", "payroll", "hiring", "vacation", "pto"]) {
                            "hr"
                        } else {
                            "other"
                        };
                        let keys = ["billing", "technical", "sales", "account", "hr", "other"];
                        let mut m = Map::new();
                        for k in keys {
                            m.insert(k.to_string(), serde_json::json!(if k == pick { 0.9 } else { 0.02 }));
                        }
                        (serde_json::json!(pick), m, 0.9)
                    }
                }
                "risk" => bscore(if contains_any(&text, &["rm ", "drop ", "delete"]) { 2.0 } else { 0.2 }),
                _ => match qtype {
                    "score" => bscore(1.0),
                    "noul" => {
                        let p = 0.5;
                        let mut m = Map::new();
                        m.insert("false".to_string(), serde_json::json!(1.0 - p));
                        m.insert("true".to_string(), serde_json::json!(p));
                        (serde_json::json!(p), m, 0.5)
                    }
                    _ => default_choice(qdef),
                },
            };
            answers.insert(qid.clone(), Decision { answer, probabilities, confidence });
        }
        Ok(Verdict { answers, input_tokens: 0, latency_ms: 0.0 })
    }
}

/// binary choice from a "true-ish" probability: A = no, B = yes.
fn bchoice(p_b: f64) -> (Value, Map<String, Value>, f64) {
    let mut m = Map::new();
    m.insert("A".to_string(), serde_json::json!(1.0 - p_b));
    m.insert("B".to_string(), serde_json::json!(p_b));
    let ans = if p_b >= 0.5 { "B" } else { "A" };
    (serde_json::json!(ans), m, p_b.max(1.0 - p_b))
}

/// ordinal score with a triangular-ish distribution around the value.
fn bscore(value: f64) -> (Value, Map<String, Value>, f64) {
    let k = 3usize;
    let mut m = Map::new();
    let mut best = (0usize, 0.0f64);
    for i in 0..k {
        let w = (1.0 - (i as f64 - value).abs() / k as f64).max(0.02);
        if w > best.1 {
            best = (i, w);
        }
        m.insert(i.to_string(), serde_json::json!(w));
    }
    (serde_json::json!(value), m, best.1)
}

/// Default for choice questions not explicitly handled: first criteria key.
fn default_choice(qdef: &Value) -> (Value, Map<String, Value>, f64) {
    let keys: Vec<String> = qdef
        .get("criteria")
        .and_then(|c| c.as_object())
        .map(|o| o.keys().cloned().collect())
        .unwrap_or_else(|| vec!["A".to_string(), "B".to_string()]);
    let mut m = Map::new();
    let n = keys.len().max(1);
    for (i, k) in keys.iter().enumerate() {
        m.insert(k.clone(), serde_json::json!(1.0 / n as f64 + if i == 0 { 0.05 } else { 0.0 }));
    }
    (serde_json::json!(keys.first().cloned().unwrap_or_default()), m, 0.5)
}

/// Builder helper for hand-written verdicts in tests.
pub fn verdict(entries: &[(&str, Decision)]) -> Verdict {
    Verdict {
        answers: entries.iter().map(|(k, d)| (k.to_string(), d.clone())).collect(),
        input_tokens: 0,
        latency_ms: 0.0,
    }
}

/// A choice decision with probabilities for the given option keys.
pub fn choice(answer: &str, probs: &[(&str, f64)]) -> Decision {
    let probabilities: Map<String, Value> = probs
        .iter()
        .map(|(k, v)| (k.to_string(), serde_json::json!(v)))
        .collect();
    let confidence = probs.iter().map(|(_, v)| *v).fold(0.0f64, f64::max);
    Decision {
        answer: serde_json::json!(answer),
        probabilities,
        confidence,
    }
}

/// A score decision (ordinal answer with per-level probabilities).
pub fn score(value: f64, probs: &[f64]) -> Decision {
    let probabilities: Map<String, Value> = probs
        .iter()
        .enumerate()
        .map(|(i, v)| (i.to_string(), serde_json::json!(v)))
        .collect();
    let confidence = probs.iter().cloned().fold(0.0f64, f64::max);
    Decision {
        answer: serde_json::json!(value),
        probabilities,
        confidence,
    }
}

/// A noul decision (`true` probability) exposing false/true probabilities.
pub fn noul(p_true: f64) -> Decision {
    let mut probabilities = Map::new();
    probabilities.insert("false".to_string(), serde_json::json!(1.0 - p_true));
    probabilities.insert("true".to_string(), serde_json::json!(p_true));
    Decision {
        answer: serde_json::json!(p_true),
        probabilities,
        confidence: p_true.max(1.0 - p_true),
    }
}
