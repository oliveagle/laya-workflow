//! Labeled-dataset decision evaluation (port of awesome-jev `evaluations/run.py`).
//!
//! The four pattern specs (rag-triage, span-selection, quality-rubric,
//! support-routing) each decide on unstructured input. This module measures a
//! workflow's *decision quality* on tickets you label first — the step between
//! "the graph runs" and "the decisions are right".
//!
//! A JSONL dataset is one object per line, mirroring awesome-jev:
//!
//! ```json
//! {"id":"t-1","split":"development","slice":"clear-technical",
//!  "message":"CSV export crashes, report due today",
//!  "expected_department":"technical","expected_urgency":"high","require_review":false}
//! ```
//!
//! The model/backend receives only the ticket message (plus whatever the spec's
//! own state projection keeps); `id`/`split`/`slice`/expected labels are never
//! fed into the workflow state, so a leak cannot inflate a score.
//!
//! Rules:
//! * development cases are for inspecting mistakes and tuning questions;
//!   holdout cases must be evaluated against a **frozen** configuration — the
//!   spec's sha256 is recorded and a holdout run refuses a changed spec.
//! * every aggregate carries its own denominator (awesome-jev's `rate` shape),
//!   so a missing decision is counted, not silently dropped.
//! * a per-case review queue is exported for cases that routed to human review
//!   or got an unsafe automatic assignment.

use std::collections::BTreeMap;
use std::fs;

use anyhow::{anyhow, Result};
use serde::Serialize;
use serde_json::{json, Value};

/// One labelled case from the JSONL dataset.
#[derive(Clone, Debug, Serialize)]
pub struct Case {
    pub id: String,
    pub split: String, // development | holdout
    pub slice: String,
    pub message: String,
    pub expected_department: String,
    pub expected_urgency: String,
    #[serde(default)]
    pub require_review: bool,
}

/// One decision outcome, with the expected vs observed labels.
#[derive(Clone, Debug, Serialize)]
pub struct Record {
    pub id: String,
    pub split: String,
    pub slice: String,
    pub expected_department: String,
    pub expected_urgency: String,
    pub require_review: bool,
    pub route: String,
    pub urgency: String,
    pub confidence: Option<f64>,
    pub department_correct: bool,
    pub urgency_correct: bool,
    pub needs_review: bool,
    pub unsafe_automatic: bool,
}

/// Parse and validate a JSONL dataset file.
///
/// Mirrors awesome-jev `load_dataset`: duplicate ids, unknown departments and
/// invalid urgency labels are hard errors; `require_review` must be boolean.
pub fn load_dataset(path: &str, departments: &[&str]) -> Result<Vec<Case>> {
    let raw = fs::read_to_string(path)?;
    let mut cases = Vec::new();
    let mut seen = std::collections::HashSet::new();
    for (i, line) in raw.lines().enumerate() {
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        let v: Value = serde_json::from_str(line)
            .map_err(|e| anyhow!("dataset line {}: invalid JSON: {e}", i + 1))?;
        let get = |k: &str| -> Result<String> {
            v.get(k)
                .and_then(|x| x.as_str())
                .map(str::to_string)
                .filter(|s| !s.trim().is_empty())
                .ok_or_else(|| anyhow!("dataset line {}: {k} must be a nonempty string", i + 1))
        };
        let id = get("id")?;
        if !seen.insert(id.clone()) {
            return Err(anyhow!("dataset: duplicate id {id:?}"));
        }
        let split = get("split")?;
        if split != "development" && split != "holdout" {
            return Err(anyhow!("dataset: {id} has invalid split {split:?}"));
        }
        let department = get("expected_department")?;
        if !departments.contains(&department.as_str()) {
            return Err(anyhow!("dataset: {id} has unknown department {department:?}"));
        }
        let urgency = get("expected_urgency")?;
        if urgency != "high" && urgency != "ordinary" {
            return Err(anyhow!("dataset: {id} has invalid urgency {urgency:?}"));
        }
        let require_review = match v.get("require_review") {
            Some(Value::Bool(b)) => *b,
            Some(_) => return Err(anyhow!("dataset: {id} require_review must be boolean")),
            None => false,
        };
        cases.push(Case {
            id,
            split,
            slice: get("slice").unwrap_or_else(|_| "unlabelled".to_string()),
            message: get("message")?,
            expected_department: department,
            expected_urgency: urgency,
            require_review,
        });
    }
    if cases.is_empty() {
        return Err(anyhow!("dataset {path:?}: no cases"));
    }
    Ok(cases)
}

/// A generic "how to read the decision out of a finished workflow state".
///
/// The four pattern specs each name their route/urgency keys differently;
/// a spec declares `"evaluation": {"route": "...", "urgency": "...",
/// "confidence": "..."}` and this reader picks those keys out of the result.
pub fn read_decision(
    spec: &Value,
    result: &Value,
) -> Result<(String, String, Option<f64>)> {
    let ev = spec
        .get("evaluation")
        .and_then(|e| e.as_object())
        .ok_or_else(|| anyhow!("spec needs an 'evaluation' block declaring route/urgency keys"))?;
    let route_key = ev.get("route").and_then(|k| k.as_str()).unwrap_or("route");
    let urgency_key = ev.get("urgency").and_then(|k| k.as_str()).unwrap_or("urgency");
    let conf_key = ev.get("confidence").and_then(|k| k.as_str());
    let route = result
        .get(route_key)
        .and_then(|v| v.as_str())
        .ok_or_else(|| anyhow!("result missing route key {route_key:?}"))?
        .to_string();
    let urgency = result
        .get(urgency_key)
        .and_then(|v| v.as_str())
        .unwrap_or("review")
        .to_string();
    let confidence = conf_key.and_then(|k| result.get(k).and_then(|v| v.as_f64()));
    Ok((route, urgency, confidence))
}

/// Evaluate the workflow against a labelled dataset.
///
/// `run` is a closure taking the ticket message and returning the finished
/// workflow state (so the CLI wires `spec::load_file(...).run(backend, ...)`
/// while tests can inject a scripted backend). Returns the records plus a
/// summary with denominators, per-slice and per-split breakdowns, confusion
/// matrix and the review queue.
pub fn evaluate(
    cases: &[Case],
    _departments: &[&str],
    mut run: impl FnMut(&str) -> Result<Value>,
) -> Result<Value> {
    let mut records: Vec<Record> = Vec::new();
    for c in cases {
        let state = run(&c.message)?;
        let (route, urgency, confidence) = read_decision_from_run(&state)?;
        let needs_review = route == "human_review";
        let department_correct = route == c.expected_department;
        let urgency_correct = urgency == c.expected_urgency;
        let unsafe_automatic = !needs_review && (department_correct == false || c.require_review);
        records.push(Record {
            id: c.id.clone(),
            split: c.split.clone(),
            slice: c.slice.clone(),
            expected_department: c.expected_department.clone(),
            expected_urgency: c.expected_urgency.clone(),
            require_review: c.require_review,
            route,
            urgency,
            confidence,
            department_correct,
            urgency_correct,
            needs_review,
            unsafe_automatic,
        });
    }
    Ok(build_summary(&records, _departments))
}

/// Read the decision out of a workflow outcome's `result` field (the CLI shape
/// is `{"result": {…}, "trace": …}`).
fn read_decision_from_run(outcome: &Value) -> Result<(String, String, Option<f64>)> {
    let result = outcome.get("result").cloned().unwrap_or_else(|| outcome.clone());
    let route = result
        .get("route")
        .and_then(|v| v.as_str())
        .or_else(|| result.get("action_question").and_then(|_| None))
        .ok_or_else(|| anyhow!("workflow result has no 'route' decision"))?
        .to_string();
    let urgency = result
        .get("urgency")
        .and_then(|v| v.as_str())
        .unwrap_or("review")
        .to_string();
    let confidence = result.get("confidence").and_then(|v| v.as_f64());
    Ok((route, urgency, confidence))
}

fn rate(ok: usize, total: usize) -> Value {
    json!({
        "numerator": ok,
        "denominator": total,
        "value": if total == 0 { Value::Null } else { json!((ok as f64) / (total as f64)) },
    })
}

fn build_summary(records: &[Record], _departments: &[&str]) -> Value {
    let total = records.len();
    let automatic: Vec<&Record> = records.iter().filter(|r| !r.needs_review).collect();
    let review: Vec<&Record> = records.iter().filter(|r| r.needs_review).collect();
    let urgency_decided: Vec<&Record> = records
        .iter()
        .filter(|r| r.urgency != "review")
        .collect();
    let required_review: Vec<&Record> = records.iter().filter(|r| r.require_review).collect();
    let high: Vec<&Record> = records.iter().filter(|r| r.expected_urgency == "high").collect();
    let ordinary: Vec<&Record> = records.iter().filter(|r| r.expected_urgency == "ordinary").collect();

    // per-slice
    let mut by_slice: BTreeMap<String, Vec<&Record>> = BTreeMap::new();
    for r in records {
        by_slice.entry(r.slice.clone()).or_default().push(r);
    }
    let slices: Value = by_slice
        .iter()
        .map(|(k, v)| {
            json!({
                "slice": k,
                "count": v.len(),
                "department_accuracy": rate(v.iter().filter(|r| r.department_correct).count(), v.len()),
                "review": rate(v.iter().filter(|r| r.needs_review).count(), v.len()),
            })
        })
        .collect();

    // per-split
    let mut by_split: BTreeMap<String, Vec<&Record>> = BTreeMap::new();
    for r in records {
        by_split.entry(r.split.clone()).or_default().push(r);
    }
    let splits: Value = by_split
        .iter()
        .map(|(k, v)| {
            json!({
                "split": k,
                "count": v.len(),
                "department_accuracy": rate(v.iter().filter(|r| r.department_correct).count(), v.len()),
                "review": rate(v.iter().filter(|r| r.needs_review).count(), v.len()),
            })
        })
        .collect();

    // confusion matrix expected -> observed
    let mut confusion: BTreeMap<String, BTreeMap<String, usize>> = BTreeMap::new();
    for r in records {
        *confusion
            .entry(r.expected_department.clone())
            .or_default()
            .entry(r.route.clone())
            .or_insert(0) += 1;
    }
    let confusion_value: Value = confusion
        .iter()
        .map(|(exp, rows)| {
            let row: Value = rows
                .iter()
                .map(|(obs, n)| (obs.clone(), json!(n)))
                .collect();
            (exp.clone(), row)
        })
        .collect();

    // review queue: cases that routed to review, or got an unsafe automatic
    let queue: Vec<Value> = records
        .iter()
        .filter(|r| r.needs_review || r.unsafe_automatic)
        .map(|r| {
            json!({
                "id": r.id,
                "split": r.split,
                "slice": r.slice,
                "expected_department": r.expected_department,
                "route": r.route,
                "urgency": r.urgency,
                "needs_review": r.needs_review,
                "unsafe_automatic": r.unsafe_automatic,
            })
        })
        .collect();

    json!({
        "counts": {
            "total": total,
            "automatic": automatic.len(),
            "review": review.len(),
        },
        "department_accuracy": rate(records.iter().filter(|r| r.department_correct).count(), total),
        "automatic_coverage": rate(automatic.len(), total),
        "review_rate": rate(review.len(), total),
        "unsafe_automatic": rate(automatic.iter().filter(|r| r.unsafe_automatic).count(), automatic.len()),
        "required_review_missed": rate(required_review.iter().filter(|r| !r.needs_review).count(), required_review.len()),
        "urgency_accuracy": rate(urgency_decided.iter().filter(|r| r.urgency_correct).count(), urgency_decided.len()),
        "urgency_review": rate(records.iter().filter(|r| r.urgency == "review").count(), total),
        "high_urgency_downgraded": rate(high.iter().filter(|r| r.urgency == "ordinary").count(), high.len()),
        "ordinary_urgency_escalated": rate(ordinary.iter().filter(|r| r.urgency == "high").count(), ordinary.len()),
        "high_urgency_review": rate(high.iter().filter(|r| r.urgency == "review").count(), high.len()),
        "confusion": confusion_value,
        "slices": slices,
        "splits": splits,
        "review_queue": queue,
    })
}

/// sha256 of the spec's JSON (used to freeze the configuration before a
/// holdout run — a holdout evaluation against a *changed* spec is rejected,
/// mirroring awesome-jev's fingerprint rule).
pub fn config_sha256(spec: &Value) -> Result<String> {
    let canonical = serde_json::to_string(spec)?;
    Ok(crate::capability::net::sha256_hex(canonical.as_bytes()))
}
