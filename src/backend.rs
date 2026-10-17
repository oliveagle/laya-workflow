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

/// A **live-backend** failure: the decision backend (`laya-tch` / soft-decider)
/// could not be reached, or answered with a transport / HTTP-status / parse
/// error instead of a `Verdict`.
///
/// This is deliberately a distinct error *type* rather than a string: the live
/// gate (`bdd live-decisions`) must tell "the deployed System-1 backend is
/// unreachable or broken" (inconclusive) from "the workflow graph regressed"
/// (a structural failure). `anyhow`'s error chain preserves the type across the
/// engine's `?` propagation, so one `downcast_ref` decides it.
#[derive(Debug)]
pub struct BackendError {
    msg: String,
}

impl BackendError {
    pub fn new(msg: impl Into<String>) -> Self {
        Self { msg: msg.into() }
    }

    /// Wrap a message into an `anyhow::Error` that preserves this type.
    pub fn err(msg: impl Into<String>) -> anyhow::Error {
        anyhow::Error::new(Self::new(msg))
    }
}

impl std::fmt::Display for BackendError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.msg)
    }
}

impl std::error::Error for BackendError {}

/// Is `e` (or anything in its `anyhow` cause chain) a [`BackendError`]? Pure, so
/// the live gate can pin the rule without a live backend.
pub fn is_backend_error(e: &anyhow::Error) -> bool {
    e.downcast_ref::<BackendError>().is_some()
}

/// HTTP client for `laya-tch`'s `/v1/systemone`.
pub struct LayaBackend {
    base_url: String,
    agent: ureq::Agent,
}

impl LayaBackend {
    pub fn new(base_url: &str) -> Self {
        Self::with_timeout(base_url, 600)
    }

    /// Like [`LayaBackend::new`], but with an explicit request timeout. The
    /// System-1 comparison's preflight probe (`bdd compare-systems`) must give up
    /// on a hung backend in seconds, not the 600 s the walk itself allows.
    pub fn with_timeout(base_url: &str, secs: u64) -> Self {
        let agent = ureq::AgentBuilder::new()
            .timeout(std::time::Duration::from_secs(secs))
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
            .map_err(|e| BackendError::err(format!("laya backend request failed: {e}")))?;
        let text = resp
            .into_string()
            .map_err(|e| BackendError::err(format!("laya backend read failed: {e}")))?;
        let raw: Value = serde_json::from_str(&text)
            .map_err(|e| BackendError::err(format!("laya backend returned malformed JSON: {e}")))?;
        let mut verdict = verdict_from_response(&raw).map_err(|e| {
            BackendError::err(format!("laya backend response was unusable: {e:#}"))
        })?;
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
                ans.get("confidence")
                    .and_then(|v| v.as_f64())
                    .unwrap_or(0.0),
            ),
            "score" => (
                ans.get("score").cloned().unwrap_or(Value::Null),
                json_to_map(ans.get("probabilities").unwrap_or(&Value::Null)),
                ans.get("confidence")
                    .and_then(|v| v.as_f64())
                    .unwrap_or(0.0),
            ),
            "boolean" => {
                let p = ans.get("probability").and_then(|v| v.as_f64()).unwrap_or(0.0);
                let mut probs = Map::new();
                probs.insert("false".to_string(), serde_json::json!(1.0 - p));
                probs.insert("true".to_string(), serde_json::json!(p));
                let conf = ans
                    .get("confidence")
                    .and_then(|v| v.as_f64())
                    .unwrap_or(p.max(1.0 - p));
                (serde_json::json!(p), probs, conf)
            }
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
            Decision {
                answer,
                probabilities,
                confidence,
            },
        );
    }
    let n_tokens = raw
        .get("usage")
        .and_then(|u| u.get("input_tokens"))
        .and_then(|v| v.as_u64())
        .unwrap_or(0) as usize;
    Ok(Verdict {
        answers: out,
        input_tokens: n_tokens,
        latency_ms: 0.0,
    })
}

/// Deterministic backend: returns canned `Verdict`s, one per call (then repeats
/// the last). Useful for tests and offline workflow dry runs.
pub struct ScriptedBackend {
    script: Vec<Verdict>,
    cursor: std::cell::Cell<usize>,
}

impl ScriptedBackend {
    pub fn new(script: Vec<Verdict>) -> Self {
        Self {
            script,
            cursor: std::cell::Cell::new(0),
        }
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

/// A caller-supplied answer script. This is how an agent (or any non-heuristic
/// caller) drives a graph — the BDD graph stays the deterministic decision core,
/// and the caller supplies the perceptions the offline `HeuristicBackend` would
/// otherwise invent.
///
/// Two shapes are accepted (see [`AnswersBackend::from_json`]):
///
///   * an **array** of `{<qid>: <answer>}` objects, one per node the walk
///     visits, in visit order; or
///   * an **object** `{"<node>": {<qid>: <answer>}}` keyed by node name, whose
///     entry is reused on every visit to that node.
///
/// Each entry is an object in the same shape `laya-tch` returns (see
/// [`verdict_from_response`]): a `choice` needs `choice` + `probabilities` +
/// `confidence`, a `score` a `score`, a `noul` a `noul` probability. The backend
/// answers every question the current node asks from that node's entry, and
/// refuses — naming the question and the entry — when an entry is missing a
/// question the node asks, when a choice answer is one the question does not
/// accept, or when the walk needs an answer the script did not supply
/// (`exhausted()` is then true). That lets a caller drive the graph one question
/// at a time instead of guessing the whole walk up front.
pub struct AnswersBackend {
    /// Visit-order mode: one entry per node the walk visits, consumed in order,
    /// paired with the caller's raw per-question answer objects so `accept` can
    /// check that each answer's declared `type` is the one the question asks.
    script: Vec<(Verdict, Map<String, Value>)>,
    /// Node-keyed mode: one entry per node name, reused on every visit to that
    /// node. Populated (and `keyed` set) when `answers` is an object.
    by_node: std::collections::HashMap<String, (Verdict, Map<String, Value>)>,
    keyed: bool,
    cursor: std::cell::Cell<usize>,
    exhausted: std::cell::Cell<bool>,
}

impl AnswersBackend {
    /// Parse the `answers` argument. An array is visit-order mode (an empty array
    /// starts a step-wise walk); an object is node-keyed mode (an empty object
    /// starts a step-wise walk keyed by node).
    pub fn from_json(answers: &Value) -> Result<Self> {
        let (script, by_node, keyed) = if let Some(arr) = answers.as_array() {
            let mut script = Vec::with_capacity(arr.len());
            for (i, entry) in arr.iter().enumerate() {
                let wrapped = serde_json::json!({ "answers": entry });
                let v = verdict_from_response(&wrapped)
                    .map_err(|e| anyhow!("answers[{i}]: {e:#}"))?;
                if v.answers.is_empty() {
                    return Err(anyhow!(
                        "answers[{i}] is empty — each entry must answer the question(s) its node asks"
                    ));
                }
                let raw = entry.as_object().cloned().unwrap_or_default();
                script.push((v, raw));
            }
            (script, std::collections::HashMap::new(), false)
        } else if let Some(obj) = answers.as_object() {
            let mut by_node = std::collections::HashMap::new();
            for (node, entry) in obj {
                let wrapped = serde_json::json!({ "answers": entry });
                let v = verdict_from_response(&wrapped)
                    .map_err(|e| anyhow!("answers[{node}]: {e:#}"))?;
                if v.answers.is_empty() {
                    return Err(anyhow!(
                        "answers[{node}] is empty — each node entry must answer the question(s) that node asks"
                    ));
                }
                let raw = entry.as_object().cloned().unwrap_or_default();
                by_node.insert(node.clone(), (v, raw));
            }
            (Vec::new(), by_node, true)
        } else {
            return Err(anyhow!(
                "answers must be an array of per-node answer objects (one per node the walk visits; [] starts a step-wise walk) or an object keyed by node name ({{<node>: {{<qid>: <answer>}}}})"
            ));
        };
        Ok(Self {
            script,
            by_node,
            keyed,
            cursor: std::cell::Cell::new(0),
            exhausted: std::cell::Cell::new(false),
        })
    }

    /// How many answer entries the caller supplied (the walk's consumed count in
    /// visit-order mode; the number of keyed nodes otherwise).
    pub fn calls(&self) -> usize {
        if self.keyed {
            self.by_node.len()
        } else {
            self.cursor.get()
        }
    }

    /// True once the walk needed an answer the `answers` argument did not supply —
    /// a visit past the array, or a node with no keyed entry.
    pub fn exhausted(&self) -> bool {
        self.exhausted.get()
    }

    /// The shared per-entry checks: every question the node asks must be answered,
    /// every answer must carry the `type` the question declares, and every choice
    /// answer must be one the question accepts (the engine would otherwise silently
    /// take the edge's `default` branch). Returns the entry on success; `label`
    /// names the entry for the error text (`answers[i]` / `answers[node]`).
    fn accept(
        &self,
        label: &str,
        v: &Verdict,
        raw: &Map<String, Value>,
        questions: &Value,
    ) -> Result<Verdict> {
        let asked: Vec<String> = questions
            .as_object()
            .map(|o| o.keys().cloned().collect())
            .unwrap_or_default();
        let missing: Vec<String> = asked
            .iter()
            .filter(|q| !v.answers.contains_key(*q))
            .cloned()
            .collect();
        if !missing.is_empty() {
            return Err(anyhow!(
                "{label} does not answer question(s) {} asked by this node (it answers {})",
                serde_json::to_string(&missing).unwrap_or_default(),
                serde_json::to_string(&v.answers.keys().collect::<Vec<_>>()).unwrap_or_default(),
            ));
        }
        // Fail closed on an answer the question does not accept. The engine would
        // otherwise silently take the edge's `default` branch, so a typo would read
        // as a deliberate "any other answer". Choice questions only (score/`noul`/
        // boolean carry no criteria); `none` stays the DSL's fail-closed answer.
        for q in &asked {
            let Some(d) = v.answers.get(q) else { continue };
            let qdef = questions.get(q.as_str());
            let qtype = qdef
                .and_then(|d| d.get("type"))
                .and_then(Value::as_str)
                .unwrap_or("choice");
            // P111: the answer must be the type the question declares. A
            // wrong-typed answer (e.g. a `score` object for a `choice` question - a
            // plausible model slip) carries no `criteria` key, so the engine would
            // silently take the edge's `default` branch. Fail closed, naming the
            // entry, the question and both types.
            if let Some(sent) = raw.get(q).and_then(|a| a.get("type")).and_then(Value::as_str) {
                if sent != qtype {
                    return Err(anyhow!(
                        "{label} answers question {q:?} with a {sent:?} answer, but the question is a {qtype:?} one — a {sent:?} answer cannot be matched against its criteria, so the walk would otherwise silently take the default branch"
                    ));
                }
            }
            if qtype != "choice" {
                continue;
            }
            let Some(ans) = d.answer.as_str() else { continue };
            if ans == "none" {
                continue;
            }
            let Some(criteria) = qdef.and_then(|d| d.get("criteria")).and_then(Value::as_object)
            else {
                continue;
            };
            if !criteria.contains_key(ans) {
                let mut allowed: Vec<&String> = criteria.keys().collect();
                allowed.sort();
                return Err(anyhow!(
                    "{label} answers question {q:?} with {ans:?}, which is not one of its accepted answers {allowed:?} — the walk would otherwise silently take the default branch"
                ));
            }
        }
        Ok(v.clone())
    }
}

impl Decide for AnswersBackend {
    fn decide(&self, _state: &Value, questions: &Value) -> Result<Verdict> {
        let i = self.cursor.get();
        let asked: Vec<String> = questions
            .as_object()
            .map(|o| o.keys().cloned().collect())
            .unwrap_or_default();
        let Some((v, raw)) = self.script.get(i) else {
            self.exhausted.set(true);
            return Err(anyhow!(
                "the walk needs another step but `answers` has only {} entr{}; the node asks question(s) {} — append one answer object for this step",
                self.script.len(),
                if self.script.len() == 1 { "y" } else { "ies" },
                serde_json::to_string(&asked).unwrap_or_default(),
            ));
        };
        let out = self.accept(&format!("answers[{i}]"), v, raw, questions)?;
        self.cursor.set(i + 1);
        Ok(out)
    }

    fn decide_at(&self, node: &str, state: &Value, questions: &Value) -> Result<Verdict> {
        if !self.keyed {
            return self.decide(state, questions);
        }
        let Some((v, raw)) = self.by_node.get(node) else {
            self.exhausted.set(true);
            let mut keys: Vec<&String> = self.by_node.keys().collect();
            keys.sort();
            let asked: Vec<String> = questions
                .as_object()
                .map(|o| o.keys().cloned().collect())
                .unwrap_or_default();
            return Err(anyhow!(
                "the walk is at node {node:?}, which `answers` has no entry for (it is keyed by node; supplied {}); the node asks question(s) {} — add an entry for this node",
                serde_json::to_string(&keys).unwrap_or_default(),
                serde_json::to_string(&asked).unwrap_or_default(),
            ));
        };
        self.accept(&format!("answers[{node}]"), v, raw, questions)
    }
}

/// Deterministic rule-based backend for offline runs and CI.
///
/// It reproduces the *decisions* the neural model makes on the app reference
/// cases (same answer keys and rough orderings) so the whole workflow graph can
/// be exercised without a model or server. It is explicitly **not** the model —
/// parity tests use `LayaBackend`.
pub struct HeuristicBackend;

/// P244: the question ids the built-in heuristic backend answers by a
/// **hand-written handler**. The match in `Decide for HeuristicBackend` keys on
/// the question id (`qid.as_str()`), not on the declared type, so this is the
/// list of ids that have a real answer in Rust.
///
/// A question id that is not listed here *and* carries no spec-declared
/// `heuristic` falls through to the type-generic default (`default_choice` /
/// `bscore(1.0)` / `noul` at 0.5) — that is a *shape*, not an answer. Nothing but
/// a model answers such a question, which is what `bdd::AnswerSource` reads this
/// list to say.
///
/// The list is duplicated from the match arms on purpose:
/// `workflow_tests::test_bdd_answer_source` scans `src/backend.rs` and fails when
/// the two drift, so a new arm cannot land without updating the list — the same
/// rule the docs follow, that a count nobody recomputes rots on the next edit.
pub const BUILTIN_QUESTION_HANDLERS: &[&str] = &[
    "is_destructive",
    "is_exfiltration",
    "is_hate",
    "is_threat",
    "is_pii",
    "is_spam",
    "spam",
    "phishing",
    "severity",
    "has_typo",
    "is_sensitive",
    "tone",
    "clarity",
    "professionalism",
    "urgency",
    "sentiment",
    "needs_reply",
    "intent_match",
    "category",
    "risk",
    "is_prohibited",
    "pii_public_high",
    "pii_public_low",
    "age_blocking",
    "age_review",
    "context_blocking",
    "context_review",
    "effective_severity",
    "intent",
    "behaviour",
    "mfa_failed_pending",
    "composite_risk",
    "service_name_safe",
    "health_path_safe",
    "canary_within_cap",
    "rollback_threshold_ok",
    "stage_shape_ok",
    "hold_within_cap",
    "canary_risk",
    "sanctioned_country",
    "crypto_category",
    "crypto_exchange_counterparty",
    "high_risk_country",
    "aml_risk",
    "known_signature",
    "failed_login_velocity",
    "port_scan_window",
    "impossible_travel",
    "unusual_country",
    "off_hours_access",
    "intrusion_verdict",
];

/// P244: does the built-in heuristic backend answer this question id by name
/// (as opposed to falling through to the type-generic default)?
pub fn has_builtin_handler(qid: &str) -> bool {
    BUILTIN_QUESTION_HANDLERS.contains(&qid)
}

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
pub(crate) fn contains_any(s: &str, needles: &[&str]) -> bool {
    let l = s.to_lowercase();
    needles.iter().any(|n| {
        let n = n.to_lowercase();
        if n.contains(' ')
            || n.contains('-')
            || n.contains('/')
            || n.contains('@')
            || n.contains('.')
        {
            return l.contains(&n);
        }
        // Single token: require boundaries so "need" != "needed".
        let mut from = 0usize;
        while let Some(i) = l[from..].find(&n) {
            let start = from + i;
            let end = start + n.len();
            let before_ok = start == 0
                || !l[..start]
                    .chars()
                    .next_back()
                    .map(|c| c.is_alphanumeric())
                    .unwrap_or(false);
            let after_ok = end >= l.len()
                || !l[end..]
                    .chars()
                    .next()
                    .map(|c| c.is_alphanumeric())
                    .unwrap_or(false);
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
            let qtype = qdef
                .get("type")
                .and_then(|v| v.as_str())
                .unwrap_or("choice");
            let (answer, probabilities, confidence) = match qid.as_str() {
                // Spec-declared offline determinism — evaluated FIRST, before any
                // named handler: a question whose definition carries
                // `heuristic.answer_field` is answered straight from that state
                // field, so a spec can drive a *shared* question name (e.g. the
                // dialogue-policy "intent") deterministically without any Rust
                // change. The state carries a previously-computed answer id (e.g.
                // a `selected_id` / `intent` field populated by a prior action or
                // a state_fn). Fail closed: an unknown id or a missing field stays
                // "none" with low confidence rather than silently picking the first
                // candidate.
                _ if qdef.get("heuristic").and_then(|h| h.get("answer_field")).is_some() => {
                    let h = qdef.get("heuristic").unwrap();
                    let field = h.get("answer_field").and_then(|v| v.as_str()).unwrap_or("");
                    let chosen = state.get(field).and_then(|v| v.as_str()).unwrap_or("none");
                    let keys: Vec<String> = qdef
                        .get("criteria")
                        .and_then(|c| c.as_object())
                        .map(|o| o.keys().cloned().collect())
                        .unwrap_or_default();
                    let known = keys.iter().any(|k| k == chosen);
                    // A `criteria.from_state` question's options do not live in the
                    // literal criteria — they are materialised from a state list
                    // (`from_state`, ids from `id_key`, default "id"). When the engine
                    // asks (it expands the criteria first, so `keys` already holds the
                    // candidate ids) `known` is the test. When a caller drives the
                    // backend *directly* with the raw criteria, materialise the ids
                    // here so a state-supplied id is accepted only when the state
                    // actually offers it: an id outside the list — like a missing
                    // field — fails closed to `none` at low confidence instead of
                    // silently picking it (P41's documented contract, made strict; P103).
                    let dyn_ids: Vec<String> =
                        match qdef.pointer("/criteria/from_state").and_then(Value::as_str) {
                            Some(src) => {
                                let id_key = qdef
                                    .pointer("/criteria/id_key")
                                    .and_then(Value::as_str)
                                    .unwrap_or("id");
                                state
                                    .get(src)
                                    .and_then(Value::as_array)
                                    .map(|arr| {
                                        arr.iter()
                                            .enumerate()
                                            .map(|(i, it)| {
                                                it.get(id_key)
                                                    .and_then(Value::as_str)
                                                    .map(str::to_string)
                                                    .unwrap_or_else(|| format!("candidate_{i}"))
                                            })
                                            .collect()
                                    })
                                    .unwrap_or_default()
                            }
                            None => Vec::new(),
                        };
                    if known || dyn_ids.iter().any(|i| i == chosen) {
                        let mut m = Map::new();
                        for k in &keys {
                            m.insert(k.clone(), serde_json::json!(if k == chosen { 0.92 } else { 0.02 }));
                        }
                        m.insert(chosen.to_string(), serde_json::json!(0.92));
                        (serde_json::json!(chosen), m, 0.92)
                    } else {
                        // Not a valid option — fall back to `none` at low
                        // confidence so the threshold's review branch fires.
                        let mut m = Map::new();
                        for k in &keys {
                            m.insert(k.clone(), serde_json::json!(if k == "none" { 0.55 } else { 0.2 }));
                        }
                        (serde_json::json!("none"), m, 0.55)
                    }
                }
                // P245: the spec's own declaration is read **before** the named-handler
                // arms below. A spec that declares its own rule must not be shadowed by a
                // Rust handler that happens to use the same question id — otherwise
                // "new project == new JSON only" would be false exactly when a spec
                // migrates a built-in question. (All existing corpora spell these two ways
                // apart: no checked-in spec declares a consumed heuristic on an id that also
                // has a named arm, so this reordering is a no-op for them; a regression
                // (`p245:`) pins that a declared `match_any` beats the named handler.)
                // Generic, **spec-declared** heuristic: if the question definition
                // carries `"heuristic": {"match_any": [...], "p_hit":…, "p_miss":…}`
                // (choice) or `{"match_any": [...], "score_hit":…, "score_miss":…}`
                // (score), evaluate it right here — no per-question Rust code.
                // `match_mid_any` adds the ambiguous middle band (the review zone):
                // strong-positive wins outright, otherwise a mid token yields
                // `p_mid` (default 0.5), and only an outright miss gets `p_miss`.
                // That three-band read-out is what lets a threshold rule's middle
                // fallback mean "uncertain, hand to a human" rather than "no".
                // This keeps the "new project == new JSON only" promise: the DSL
                // author declares the tokens that indicate a hit, the engine
                // evaluates them uniformly. Longest-first substring match on the
                // serialised state, case-insensitive (same helper as the named
                // handlers below).
                // multi-class routing: `heuristic.match_rules` maps each
                // choice option to a list of trigger tokens; the first option
                // whose tokens appear in the field/text wins (option at p_hit,
                // the rest at p_miss). A `field` declared but absent from the
                // state is an empty match space (same P126 rule as `match_any`
                // below), so a missing hint can never be satisfied by a token
                // that happens to sit elsewhere in the state. No hit at all fails safe to the option
                // named by `fallback` (default "other"), mirroring awesome-jev's
                // support-routing "choose other if unclear". This is the
                // declarative offline analog of a named per-question handler:
                // new project == new JSON, no Rust.
                _ if qdef.get("heuristic").and_then(|h| h.get("match_rules")).is_some() => {
                    let h = qdef.get("heuristic").unwrap();
                    let rules = h.get("match_rules").and_then(|m| m.as_object()).cloned().unwrap_or_default();
                    let p_hit = h.get("p_hit").and_then(|x| x.as_f64()).unwrap_or(0.9);
                    let p_miss = h.get("p_miss").and_then(|x| x.as_f64()).unwrap_or(0.02);
                    let fallback = h.get("fallback").and_then(|x| x.as_str()).unwrap_or("other");
                    let target_text: String = match h.get("field").and_then(|f| f.as_str()) {
                        // `field` declared but absent from the state: a clean miss
                        // (empty needle space), never the whole envelope.
                        Some(k) => state.get(k).map(|v| match v {
                            Value::String(sv) => sv.clone(),
                            other => other.to_string(),
                        }).unwrap_or_default(),
                        None => text.clone(),
                    };
                    let mut pick: Option<String> = None;
                    for (opt, toks_v) in &rules {
                        let toks: Vec<&str> = toks_v
                            .as_array()
                            .map(|a| a.iter().filter_map(|x| x.as_str()).collect())
                            .unwrap_or_default();
                        if !toks.is_empty() && contains_any(&target_text, &toks) {
                            pick = Some(opt.clone());
                            break;
                        }
                    }
                    let matched = pick.is_some();
                    let chosen = pick.unwrap_or_else(|| fallback.to_string());
                    let conf = if matched {
                        p_hit
                    } else {
                        // Failing safe to `fallback` is a *no-match* verdict, not
                        // a confident one: confidences it below a 0.8 floor so a
                        // `combine`/threshold rule sends it to review instead of
                        // routing it as though the message clearly fit `other`.
                        h.get("p_fallback")
                            .and_then(|x| x.as_f64())
                            .unwrap_or(0.6)
                    };
                    let mut m = Map::new();
                    for opt in rules.keys() {
                        m.insert(opt.clone(), serde_json::json!(if *opt == chosen { conf } else { p_miss }));
                    }
                    (serde_json::json!(chosen), m, conf)
                }
                _ if qdef.get("heuristic").and_then(|h| h.get("score_rules")).is_some() => {
                    let h = qdef.get("heuristic").unwrap();
                    let rules = h.get("score_rules").and_then(|m| m.as_object()).cloned().unwrap_or_default();
                    let miss = h.get("score_miss").and_then(|x| x.as_f64()).unwrap_or(0.5);
                    let target_text: String = match h.get("field").and_then(|f| f.as_str()) {
                        Some(k) => state.get(k).map(|v| match v {
                            Value::String(sv) => sv.clone(),
                            other => other.to_string(),
                        }).unwrap_or_default(),
                        None => text.clone(),
                    };
                    let mut value = miss;
                    for (score, toks_v) in &rules {
                        let score: f64 = score.parse().map_err(|_| {
                            anyhow!("heuristic score_rules score {score:?} for {qid} is not a number")
                        })?;
                        let toks: Vec<&str> = toks_v
                            .as_array()
                            .map(|a| a.iter().filter_map(|x| x.as_str()).collect())
                            .unwrap_or_default();
                        if !toks.is_empty() && contains_any(&target_text, &toks) {
                            value = score;
                            break;
                        }
                    }
                    bscore(value)
                }
                _ if qdef.get("heuristic").and_then(|h| h.get("match_any").or_else(|| h.get("match_all")).or_else(|| h.get("match_regex")).or_else(|| h.get("extract_numeric"))).is_some() => {
                    let h = qdef.get("heuristic").unwrap();
                    // `match_any`: literal substring needles (word-boundary aware).
                    // `match_regex`: full regex patterns (any match counts as a hit),
                    // letting a spec port a shell `grep -E` rule verbatim.
                    // `heuristic.field` narrows matching to one state key (e.g.
                    // {"field":"message"} so a regex checks the message value
                    // itself, not the serialised JSON envelope). With `field`
                    // **declared but absent** from the state the match space is
                    // empty: a clean miss, never a match. Only a question that
                    // declares no `field` at all matches the whole serialised
                    // state. P126 — the absent-field case used to fall back to
                    // the envelope, which silently cross-fired: a state that set
                    // only `route_semantic: "high"` scored *every* routing noul
                    // at its `p_hit` (the token "high" was somewhere in the
                    // JSON), and the smoke gate's own expect table had pinned
                    // that artefact. Absent means absent.
                    let target_text: String = match h.get("field").and_then(|f| f.as_str()) {
                        // `field` declared but absent from the state: a clean miss
                        // (empty needle space), never the whole envelope.
                        Some(k) => state.get(k).map(|v| match v {
                            Value::String(sv) => sv.clone(),
                            other => other.to_string(),
                        }).unwrap_or_default(),
                        None => text.clone(),
                    };
                    let mut hit = false;
                    if let Some(any) = h.get("match_any").and_then(|a| a.as_array()) {
                        let toks: Vec<&str> = any.iter().filter_map(|x| x.as_str()).collect();
                        hit = contains_any(&target_text, &toks);
                    }
                    // `match_all`: each row is an OR-alternative group; *every* row
                    // must contribute a hit. This is the conjunction the one-group
                    // `match_any` cannot spell (P248).
                    if !hit {
                        if let Some(groups) = h.get("match_all").and_then(|a| a.as_array()) {
                            hit = groups.iter().all(|group| {
                                group.as_array().is_some_and(|needles| {
                                    let toks: Vec<&str> =
                                        needles.iter().filter_map(|x| x.as_str()).collect();
                                    contains_any(&target_text, &toks)
                                })
                            });
                        }
                    }
                    // `match_mid_any`: ambiguous tokens (mid / unclear / …) that are
                    // neither a strong hit nor a strong miss. Only consulted when the
                    // strong signal did not fire, so the bands stay strictly ordered.
                    let mut mid = false;
                    if !hit {
                        if let Some(any) = h.get("match_mid_any").and_then(|a| a.as_array()) {
                            let toks: Vec<&str> = any.iter().filter_map(|x| x.as_str()).collect();
                            mid = contains_any(&target_text, &toks);
                        }
                    }
                    if !hit {
                        if let Some(rxs) = h.get("match_regex").and_then(|a| a.as_array()) {
                            for rx in rxs.iter().filter_map(|x| x.as_str()) {
                                // Fail closed: a broken regex is a spec error, not a
                                // silent miss (a security scanner that skips a bad
                                // pattern could let a real threat through).
                                let re = regex_lite::Regex::new(rx)
                                    .map_err(|e| anyhow!("heuristic match_regex {rx:?} for {qid}: {e}"))?;
                                if re.is_match(&target_text) { hit = true; break; }
                            }
                        }
                    }
                    match qtype {
                        "score" => {
                            // `heuristic.extract_numeric: "field"` pulls the raw
                            // numeric value out of a state field and uses it as the
                            // score, so threshold rules can compare the real value
                            // (days_left, error_rate, byte size, …) instead of a
                            // two-constant hit/miss pair. Field missing/unparsable
                            // falls back to `score_miss` (fail-closed for scores
                            // that gate on a number).
                            let v = if let Some(field) = h.get("extract_numeric").and_then(|x| x.as_str()) {
                                state.get(field)
                                    .and_then(|x| x.as_f64().or_else(|| x.as_str().and_then(|sv| sv.parse::<f64>().ok())))
                                    .unwrap_or_else(|| h.get("score_miss").and_then(|x| x.as_f64()).unwrap_or(0.0))
                            } else if hit { h.get("score_hit").and_then(|x| x.as_f64()).unwrap_or(2.0) }
                            else if mid {
                                // middle band defaults to the midpoint of the two
                                // declared anchors (2.0/0.5 -> 1.25), so an explicit
                                // `score_mid` is an override, not a requirement.
                                h.get("score_mid").and_then(|x| x.as_f64()).unwrap_or_else(|| {
                                    let lo = h.get("score_miss").and_then(|x| x.as_f64()).unwrap_or(0.5);
                                    let hi = h.get("score_hit").and_then(|x| x.as_f64()).unwrap_or(2.0);
                                    (lo + hi) / 2.0
                                })
                            }
                            else { h.get("score_miss").and_then(|x| x.as_f64()).unwrap_or(0.5) };
                            bscore(v)
                        }
                        "noul" => {
                            // Noul returns a *probability* (0..1 = p(true)), not an A/B choice.
                            // Returning the bchoice shape made threshold rules on value_gte/value_lt
                            // compare against the string "A"/"B" (as_f64 -> 0.0), which silently
                            // triggered CONTINUE in stop-check-style specs.
                            let p = if hit { h.get("p_hit").and_then(|x| x.as_f64()).unwrap_or(0.9) }
                                    else if mid { h.get("p_mid").and_then(|x| x.as_f64()).unwrap_or(0.5) }
                                    else { h.get("p_miss").and_then(|x| x.as_f64()).unwrap_or(0.1) };
                            let mut m = Map::new();
                            m.insert("false".to_string(), serde_json::json!(1.0 - p));
                            m.insert("true".to_string(), serde_json::json!(p));
                            (serde_json::json!(p), m, p.max(1.0 - p))
                        }
                        _ => {
                            let p_b = if hit { h.get("p_hit").and_then(|x| x.as_f64()).unwrap_or(0.9) }
                                      else if mid { h.get("p_mid").and_then(|x| x.as_f64()).unwrap_or(0.5) }
                                      else { h.get("p_miss").and_then(|x| x.as_f64()).unwrap_or(0.1) };
                            bchoice(p_b)
                        }
                    }
                }
                "is_destructive" => {
                    let p = if contains_any(
                        &text,
                        &[
                            "rm ", "drop ", "delete", "truncate", "wipe", "format", "kill",
                        ],
                    ) {
                        0.95
                    } else {
                        0.1
                    };
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
                    let outward = contains_any(
                        &text,
                        &["curl ", "wget ", "scp ", "@s.txt", "evil.example", "upload"],
                    );
                    let sensitive_read =
                        contains_any(&text, &["/etc/passwd", "/etc/shadow", "id_rsa", ".ssh/"]);
                    let p = if outward {
                        0.9
                    } else if sensitive_read {
                        0.45
                    } else {
                        0.1
                    };
                    bchoice(p)
                }
                "is_hate" => bchoice(if contains_any(&text, &["vermin", "deport", "slur"]) {
                    0.9
                } else {
                    0.05
                }),
                "is_threat" => bchoice(
                    if contains_any(&text, &["find you", "watch your back", "kill you"]) {
                        0.9
                    } else {
                        0.05
                    },
                ),
                "is_pii" => bchoice(
                    if contains_any(&text, &["ssn", "123-45-6789", "credit card"]) {
                        0.9
                    } else {
                        0.05
                    },
                ),
                "is_spam" | "spam" => bchoice(
                    if contains_any(
                        &text,
                        &["get rich", "limited offer", "buy now", "5000/week"],
                    ) {
                        0.9
                    } else {
                        0.1
                    },
                ),
                "phishing" => bchoice(
                    if contains_any(
                        &text,
                        &[
                            "confirm credentials",
                            "suspended",
                            "verify your account",
                            "http://evil",
                        ],
                    ) {
                        0.9
                    } else {
                        0.05
                    },
                ),
                "severity" => bscore(
                    if contains_any(
                        &text,
                        &["vermin", "watch your back", "get rich", "123-45-6789"],
                    ) {
                        2.0
                    } else {
                        0.5
                    },
                ),
                "has_typo" => bchoice(if contains_any(&text, &["discusion", "attched", "pls"]) {
                    0.8
                } else {
                    0.1
                }),
                "is_sensitive" => bchoice(
                    if contains_any(
                        &text,
                        &["i've decided to leave", "unacceptable", "2-week plan"],
                    ) {
                        0.8
                    } else {
                        0.2
                    },
                ),
                // `tone` is a 4-level score (angry/tense/neutral/warm). Only
                // looking for two hostile phrases left a resignation letter at
                // "neutral", so a high-stakes message was never routed to
                // "sleep". Fold in the other negative-affect signals this
                // backend already extracts.
                "tone" => {
                    let hostile =
                        contains_any(&text, &["unacceptable", "fix now", "angry", "furious"]);
                    let heavy = contains_any(
                        &text,
                        &[
                            "decided to leave",
                            "resign",
                            "i quit",
                            "termination",
                            "layoff",
                        ],
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
                "urgency" => bscore(
                    if contains_any(&text, &["outage", "refund today", "before sprint end"]) {
                        2.0
                    } else {
                        0.8
                    },
                ),
                "sentiment" => bscore(if contains_any(&text, &["unacceptable", "angry"]) {
                    0.3
                } else {
                    2.0
                }),
                "needs_reply" => bchoice(if contains_any(&text, &["please", "need", "refund"]) {
                    0.7
                } else {
                    0.2
                }),
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
                            "free up disk space", // `sudo rm -rf /`
                            "reset dev database", // `drop database prod`
                            "clean the project",  // `rm -rf ./build`
                            "send webhook",       // `curl ... -d @s.txt`
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
                        } else if contains_any(&text, &["find you", "watch your back", "kill you"])
                        {
                            "threatening"
                        } else if contains_any(&text, &["ssn", "123-45-6789", "credit card"]) {
                            "leaky_pii"
                        } else if contains_any(
                            &text,
                            &["get rich", "limited offer", "buy now", "5000/week"],
                        ) {
                            "spam"
                        } else {
                            "safe"
                        };
                        let keys = [
                            "safe",
                            "hateful",
                            "threatening",
                            "leaky_pii",
                            "spam",
                            "off_topic",
                        ];
                        let mut m = Map::new();
                        for k in keys {
                            m.insert(
                                k.to_string(),
                                serde_json::json!(if k == pick { 0.9 } else { 0.02 }),
                            );
                        }
                        (serde_json::json!(pick), m, 0.9)
                    } else {
                        let pick = if contains_any(
                            &text,
                            &["outage", "500", "hotfix", "incident", "prod", "unavailable"],
                        ) {
                            "technical"
                        } else if contains_any(
                            &text,
                            &["refund", "invoice", "billed", "payment", "duplicate charge"],
                        ) {
                            "billing"
                        } else if contains_any(
                            &text,
                            &["pricing", "demo", "purchase", "quote", "trial"],
                        ) {
                            "sales"
                        } else if contains_any(
                            &text,
                            &["package", "parcel", "delivery", "deliver", "courier",
                              "shipment", "shipping", "arrived", "arrive", "customs",
                              "tracking"],
                        ) {
                            "shipping"
                        } else if contains_any(
                            &text,
                            &["login", "password", "access", "profile", "reset"],
                        ) {
                            "account"
                        } else if contains_any(
                            &text,
                            &["leave", "payroll", "hiring", "vacation", "pto"],
                        ) {
                            "hr"
                        } else {
                            "other"
                        };
                        let keys = ["billing", "technical", "sales", "shipping", "account", "hr", "other"];
                        let mut m = Map::new();
                        for k in keys {
                            m.insert(
                                k.to_string(),
                                serde_json::json!(if k == pick { 0.9 } else { 0.02 }),
                            );
                        }
                        (serde_json::json!(pick), m, 0.9)
                    }
                }
                "risk" => bscore(if contains_any(&text, &["rm ", "drop ", "delete"]) {
                    2.0
                } else {
                    0.2
                }),
                // --- ole_eval / dsl/ole_eval/ question handlers ---
                // Content safety guard: choice questions are A=no / B=yes.
                "is_prohibited" => bchoice(if contains_any(&text, &["targeted_abuse", "prohibited", "harassment"]) { 0.95 } else { 0.05 }),
                "pii_public_high" => bchoice(if contains_any(&text, &["personal_address", "ssn", "contact_information"]) && contains_any(&text, &["public_feed", "public"]) { 0.9 } else { 0.05 }),
                "pii_public_low" => bchoice(if contains_any(&text, &["contact_information", "financial_promotion"]) && contains_any(&text, &["public_feed", "public"]) { 0.7 } else { 0.05 }),
                "age_blocking" => bchoice(if contains_any(&text, &["graphic_violence", "self_harm", "artistic_nudity"]) && contains_any(&text, &["child", "teen", "audience_age"]) { 0.9 } else { 0.05 }),
                "age_review" => bchoice(if contains_any(&text, &["violence_simulation", "medical_information", "contact_information"]) && contains_any(&text, &["child", "teen"]) { 0.7 } else { 0.05 }),
                "context_blocking" => bchoice(if contains_any(&text, &["graphic_violence", "self_harm", "artistic_nudity"]) && contains_any(&text, &["public_feed", "not_allowed"]) { 0.9 } else { 0.05 }),
                "context_review" => bchoice(if contains_any(&text, &["violence_simulation", "medical_information", "financial_promotion"]) && contains_any(&text, &["public_feed", "not_allowed"]) { 0.7 } else { 0.05 }),
                "effective_severity" => {
                    let v = if contains_any(&text, &["targeted_abuse", "prohibited"]) { 100.0 }
                        else if contains_any(&text, &["graphic_violence", "self_harm"]) { 95.0 }
                        else if contains_any(&text, &["personal_address"]) { 80.0 }
                        else if contains_any(&text, &["artistic_nudity"]) { 75.0 }
                        else if contains_any(&text, &["violence_simulation"]) { 60.0 }
                        else if contains_any(&text, &["contact_information"]) { 45.0 }
                        else if contains_any(&text, &["financial_promotion"]) { 35.0 }
                        else if contains_any(&text, &["medical_information"]) { 30.0 }
                        else { 10.0 };
                    // Use a small-ordinal helper to keep the value f64 clean for value_gte rules.
                    let mut m = Map::new();
                    m.insert("0".to_string(), serde_json::json!(if v < 50.0 { 0.9 } else { 0.02 }));
                    m.insert("1".to_string(), serde_json::json!(if v >= 50.0 && v < 90.0 { 0.9 } else { 0.02 }));
                    m.insert("2".to_string(), serde_json::json!(if v >= 90.0 { 0.9 } else { 0.02 }));
                    (serde_json::json!(v), m, 0.9)
                }
                // Adaptive risk control: intent/behaviour are multi-choice (not A/B).
                "intent" => {
                    // Criteria-aware: the same question id serves two specs with
                    // different criteria sets (same trick as "category" below).
                    let crit = qdef.get("criteria").and_then(|c| c.as_object());
                    let has_aml = crit.map(|o| o.contains_key("legitimate_purchase")).unwrap_or(false);
                    if has_aml {
                        let pick = if contains_any(&text, &["credential_stuffing", "credential stuffing"]) { "credential_stuffing" }
                            else if contains_any(&text, &["card_testing", "card testing"]) { "card_testing" }
                            else if contains_any(&text, &["account_takeover", "account takeover"]) { "account_takeover" }
                            else if contains_any(&text, &["recovery"]) { "legitimate_recovery" }
                            else { "legitimate_purchase" };
                        let keys = ["legitimate_purchase", "legitimate_recovery", "credential_stuffing", "card_testing", "account_takeover", "unknown"];
                        let mut m = Map::new();
                        for k in keys { m.insert(k.to_string(), serde_json::json!(if k == pick { 0.9 } else { 0.02 })); }
                        (serde_json::json!(pick), m, 0.9)
                    } else if crit.map(|o| o.contains_key("greet")).unwrap_or(false) {
                        // Dialogue policy: pick by the keywords present for each state.
                        let pick = if contains_any(&text, &["stop_path", "confirm execute", "confirm_transfer"]) { "confirm" }
                            else if contains_any(&text, &["provide_slots", "provide slot", "store_slots", "fill slots", "amount recipient"]) { "provide_slot" }
                            else if contains_any(&text, &["request_confirm", "all slots", "ask confirm"]) { "request_confirm" }
                            else if contains_any(&text, &["authenticate", "mfa_code", "auth_method"]) { "authenticate" }
                            else if contains_any(&text, &["identify", "user_id"]) { "identify" }
                            else if contains_any(&text, &["start_collection", "start collection", "operation transfer"]) { "start_collection" }
                            else if contains_any(&text, &["escalate", "handoff", "hand off"]) { "escalate" }
                            else if contains_any(&text, &["abort", "cancel session", "discard"]) { "abort" }
                            else if crit.map(|o| o.contains_key("noop")).unwrap_or(false) { "noop" }
                            else { "greet" };
                        let keys: Vec<String> = crit
                            .map(|o| o.keys().cloned().collect())
                            .unwrap_or_else(|| vec![pick.to_string()]);
                        let mut m = Map::new();
                        for k in &keys { m.insert(k.clone(), serde_json::json!(if k == pick { 0.9 } else { 0.02 })); }
                        (serde_json::json!(pick), m, 0.9)
                    } else {
                        default_choice(qdef)
                    }
                }
                "behaviour" => {
                    let pick = if contains_any(&text, &["repeated_failure", "repeated failure"]) { "repeated_failures" }
                        else if contains_any(&text, &["micro_transaction", "micro transaction", "burst"]) { "micro_transaction_burst" }
                        else if contains_any(&text, &["new_device", "new device"]) { "new_device_payment" }
                        else if contains_any(&text, &["recovery_then_withdrawal", "recovery then withdrawal", "withdrawal"]) { "recovery_then_withdrawal" }
                        else if contains_any(&text, &["inconsistent", "session signals"]) { "inconsistent_session" }
                        else { "normal_usage" };
                    let keys = ["normal_usage", "repeated_failures", "micro_transaction_burst", "new_device_payment", "recovery_then_withdrawal", "inconsistent_session"];
                    let mut m = Map::new();
                    for k in keys { m.insert(k.to_string(), serde_json::json!(if k == pick { 0.9 } else { 0.02 })); }
                    (serde_json::json!(pick), m, 0.9)
                }
                "mfa_failed_pending" => bchoice(if contains_any(&text, &["mfa_failed", "mfa failed", "mfa-failed", "challenge_pending"]) { 0.9 } else { 0.05 }),
                "composite_risk" => {
                    let v = if contains_any(&text, &["credential_stuffing", "card_testing", "account_takeover"]) { 80.0 }
                        else if contains_any(&text, &["recovery_then_withdrawal"]) { 60.0 }
                        else if contains_any(&text, &["micro_transaction_burst", "new_device_payment"]) { 45.0 }
                        else if contains_any(&text, &["repeated_failures", "inconsistent_session"]) { 25.0 }
                        else { 10.0 };
                    let mut m = Map::new();
                    m.insert("0".to_string(), serde_json::json!(if v < 20.0 { 0.9 } else { 0.02 }));
                    m.insert("1".to_string(), serde_json::json!(if v >= 20.0 && v < 40.0 { 0.9 } else { 0.02 }));
                    m.insert("2".to_string(), serde_json::json!(if v >= 40.0 { 0.9 } else { 0.02 }));
                    (serde_json::json!(v), m, 0.9)
                }
                // Deployment canary guard: all choice questions are A=ok / B=violated.
                "service_name_safe" => bchoice(if contains_any(&text, &["eval(", "exec(", "subprocess", "rm -rf", "javascript:", "os.system"]) { 0.95 } else { 0.05 }),
                "health_path_safe" => bchoice(if contains_any(&text, &["eval(", "javascript:", "data:", "os.system"]) { 0.9 } else { 0.05 }),
                "canary_within_cap" => bchoice(if contains_any(&text, &["exceed_cap", "over_cap", "canary_percent_high"]) { 0.9 } else { 0.05 }),
                "rollback_threshold_ok" => bchoice(if contains_any(&text, &["error_rate_high", "latency_high", "rollback_breach"]) { 0.9 } else { 0.05 }),
                "stage_shape_ok" => bchoice(if contains_any(&text, &["no_stages", "empty_stages", "too_many_stages", "negative_percent"]) { 0.9 } else { 0.05 }),
                "hold_within_cap" => bchoice(if contains_any(&text, &["hold_too_long", "hold_exceeds", "extended_hold"]) { 0.9 } else { 0.05 }),
                "canary_risk" => {
                    let v = if contains_any(&text, &["eval(", "rm -rf", "javascript:", "subprocess"]) { 90.0 }
                        else if contains_any(&text, &["error_rate_high", "latency_high", "rollback_breach"]) { 60.0 }
                        else if contains_any(&text, &["exceed_cap", "hold_too_long"]) { 40.0 }
                        else if contains_any(&text, &["stages_wide"]) { 45.0 }
                        else { 10.0 };
                    let mut m = Map::new();
                    m.insert("0".to_string(), serde_json::json!(if v < 30.0 { 0.9 } else { 0.02 }));
                    m.insert("1".to_string(), serde_json::json!(if v >= 30.0 && v < 70.0 { 0.9 } else { 0.02 }));
                    m.insert("2".to_string(), serde_json::json!(if v >= 70.0 { 0.9 } else { 0.02 }));
                    (serde_json::json!(v), m, 0.9)
                }
                // AML screener: choice questions A=no / B=yes; aml_risk is 0..100.
                "sanctioned_country" => bchoice(if contains_any(&text, &["cuba", "iran", "north korea", "syria", " country_cu", "country_ir", "country_kp", "country_sy", "\"CU\"", "\"IR\"", "\"KP\"", "\"SY\""]) { 0.95 } else { 0.05 }),
                "crypto_category" => bchoice(if contains_any(&text, &["crypto", "bitcoin", "ethereum"]) { 0.9 } else { 0.1 }),
                "crypto_exchange_counterparty" => bchoice(if contains_any(&text, &["crypto_exchange", "exchange"]) { 0.9 } else { 0.1 }),
                "high_risk_country" => bchoice(if contains_any(&text, &["belarus", "myanmar", "nigeria", "russia", "belarus", "country_by", "country_mm", "country_ng", "country_ru"]) { 0.9 } else { 0.1 }),
                "aml_risk" => {
                    let mut v = 0.0f64;
                    if contains_any(&text, &["crypto", "bitcoin", "ethereum"]) { v += 30.0; }
                    if contains_any(&text, &["large_amount", "large amount", "above threshold"]) { v += 25.0; }
                    if contains_any(&text, &["high_risk", "high risk", "belarus", "russia", "country_by", "country_ru"]) { v += 35.0; }
                    if contains_any(&text, &["crypto_exchange", "exchange"]) { v += 20.0; }
                    if contains_any(&text, &["near_threshold", "near threshold"]) { v += 15.0; }
                    if contains_any(&text, &["charity_high_risk", "charity"]) { v += 20.0; }
                    if contains_any(&text, &["sanctioned", "country_cu", "country_ir", "country_kp", "country_sy"]) { v = 100.0; }
                    let v = v.min(100.0);
                    let mut m = Map::new();
                    m.insert("0".to_string(), serde_json::json!(if v < 25.0 { 0.9 } else { 0.02 }));
                    m.insert("1".to_string(), serde_json::json!(if v >= 25.0 && v < 70.0 { 0.9 } else { 0.02 }));
                    m.insert("2".to_string(), serde_json::json!(if v >= 70.0 { 0.9 } else { 0.02 }));
                    (serde_json::json!(v), m, 0.9)
                }
                // Intrusion signal guard: signal flags + the derived verdict.
                "known_signature" => bchoice(if contains_any(&text, &["sql_injection", "union select", " or 1=1", "sleep(", "../", "/etc/passwd", "${jndi:", "() { :;", "sqlmap", "nikto", "masscan", "zgrab"]) { 0.95 } else { 0.05 }),
                "failed_login_velocity" => bchoice(if contains_any(&text, &["failed_login_velocity", "failed login velocity", "5 failed", "repeated_failures"]) { 0.9 } else { 0.1 }),
                "port_scan_window" => bchoice(if contains_any(&text, &["port_scan", "port scan", "unique_ports", "masscan", "zgrab"]) { 0.9 } else { 0.1 }),
                "impossible_travel" => bchoice(if contains_any(&text, &["impossible_travel", "impossible travel", "fast travel"]) { 0.9 } else { 0.1 }),
                "unusual_country" => bchoice(if contains_any(&text, &["unusual_country", "unusual country", "unexpected country"]) { 0.8 } else { 0.2 }),
                "off_hours_access" => bchoice(if contains_any(&text, &["off_hours", "off hours", "night access"]) { 0.8 } else { 0.2 }),
                "intrusion_verdict" => {
                    // Recompute the Python _decide ladder from the same state text.
                    let strong_count = [
                        contains_any(&text, &["failed_login_velocity", "failed login velocity"]),
                        contains_any(&text, &["port_scan", "port scan", "masscan", "zgrab"]),
                        contains_any(&text, &["impossible_travel", "impossible travel"]),
                    ].iter().filter(|x| **x).count();
                    let weak_count = [
                        contains_any(&text, &["unusual_country", "unusual country"]),
                        contains_any(&text, &["off_hours", "off hours"]),
                    ].iter().filter(|x| **x).count();
                    let signature = contains_any(&text, &["sql_injection", "union select", " or 1=1", "sleep(", "../", "/etc/passwd", "${jndi:", "() { :;", "sqlmap", "nikto"]);
                    let pick = if signature || strong_count >= 2 || (strong_count >= 1 && weak_count >= 2) { "block" }
                        else if strong_count == 1 || weak_count >= 2 { "challenge" }
                        else if weak_count == 1 { "monitor" }
                        else { "allow" };
                    let keys = ["allow", "monitor", "challenge", "block"];
                    let mut m = Map::new();
                    for k in keys { m.insert(k.to_string(), serde_json::json!(if k == pick { 0.9 } else { 0.02 })); }
                    (serde_json::json!(pick), m, 0.9)
                }
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
            // The handlers above are keyed by question id, but two specs may reuse an
            // id with a different declared type (email_triage's `urgency` is a score,
            // notify_macos' `urgency` is a choice). A handler that returns the wrong
            // shape for the declared type would have the engine read it as the wrong
            // thing (`Edge::resolve` stringifies a number, `as_f64()` reads a string as
            // 0.0) — and `WorkflowNode::hold_in_contract` now fails closed on exactly
            // that mismatch. So the heuristic must uphold the same contract: when the
            // named handler's shape does not match the question's declared type, fall
            // back to the type-generic default.
            let (answer, probabilities, confidence) = match qtype {
                "choice" if !answer.is_string() => default_choice(qdef),
                "score" if !answer.is_number() => bscore(1.0),
                "noul" if !answer.is_number() => {
                    let p = 0.5;
                    let mut m = Map::new();
                    m.insert("false".to_string(), serde_json::json!(1.0 - p));
                    m.insert("true".to_string(), serde_json::json!(p));
                    (serde_json::json!(p), m, 0.5)
                }
                _ => (answer, probabilities, confidence),
            };
            answers.insert(
                qid.clone(),
                Decision {
                    answer,
                    probabilities,
                    confidence,
                },
            );
        }
        Ok(Verdict {
            answers,
            input_tokens: 0,
            latency_ms: 0.0,
        })
    }
}

/// binary choice from a "true-ish" probability: A = no, B = yes.
fn bchoice(p_b: f64) -> (Value, Map<String, Value>, f64) {
    let mut m = Map::new();
    m.insert("A".to_string(), serde_json::json!(1.0 - p_b));
    m.insert("B".to_string(), serde_json::json!(p_b));
    let ans = if p_b >= 0.5 { "B" } else { "A" };
    // 3-decimal confidence: `1.0 - 0.6/3.0` etc. produce 0.7999999999999999,
    // which then compared against a 0.8 confidence floor as "uncertain" in
    // composite-scoring specs. Rounding makes the clean boundary a boundary.
    let conf = (p_b.max(1.0 - p_b) * 1000.0).round() / 1000.0;
    (serde_json::json!(ans), m, conf)
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
    // Same 3-decimal confidence as bchoice — see the comment there.
    let conf = (best.1 * 1000.0).round() / 1000.0;
    (serde_json::json!(value), m, conf)
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
        m.insert(
            k.clone(),
            serde_json::json!(1.0 / n as f64 + if i == 0 { 0.05 } else { 0.0 }),
        );
    }
    (
        serde_json::json!(keys.first().cloned().unwrap_or_default()),
        m,
        0.5,
    )
}

/// Builder helper for hand-written verdicts in tests.
pub fn verdict(entries: &[(&str, Decision)]) -> Verdict {
    Verdict {
        answers: entries
            .iter()
            .map(|(k, d)| (k.to_string(), d.clone()))
            .collect(),
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
