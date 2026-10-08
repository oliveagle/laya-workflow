//! rules — rule enforcement for coding agents (Rust port).
//!
//! Port of <https://github.com/coldteadotai/abide>: turn the repo's
//! AGENTS.md / CLAUDE.md into a machine-checkable rubric, then judge every
//! edit (and every turn) against it with a Jev-class model. When a rule is
//! clearly violated the agent is asked to repair the change; when the model is
//! unsure it is only flagged; otherwise the check is silent.
//!
//! Faithful pieces (1:1 with the TypeScript package):
//!   * rubric schema (`version 1`: sources / rules / thresholds)
//!   * `violation_probability` + `band_for` (act=0.8 / flag=0.5)
//!   * `select_rules` / `group_by_scope` / `loudest_verdicts` / `run_check`
//!   * `repair_reason` / `flag_notice` — the block / notice / silent output
//!   * `<repo>/.rules/rubric.json` — the rubric itself, committed with the repo
//!   * `~/.laya-workflow/rules/events.jsonl` — per-user audit log (never committed)
//!
//! One extension for offline determinism: a model question may carry an
//! optional `heuristic` block (`match_any` tokens + `p_violated` / `p_ok`).
//! The offline backend answers from it; a model question without one is
//! treated as compliant offline. With `--base-url` the real model judges
//! instead (same rubric, same bands).

use anyhow::{anyhow, bail, Result};
use serde::{Deserialize, Serialize};
use serde_json::{json, Map, Value};
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::time::Instant;

use crate::backend::LayaBackend;
use crate::workflow::{Decide, Decision};

// ─── constants (mirror packages/cli/src/lib/constants.ts) ──────────

/// Default act / flag bands: >= act is a hard block, >= flag is a soft flag.
pub const DEFAULT_THRESHOLDS: Thresholds = Thresholds { act: 0.8, flag: 0.5 };
pub const EDIT_CHECK_TIMEOUT_MS: u64 = 8_000;
pub const TURN_CHECK_TIMEOUT_MS: u64 = 15_000;
/// Largest diff sent as state; beyond this the diff is cut and marked.
pub const MAX_STATE_CHARS: usize = 24_000;
pub const MAX_TASK_CHARS: usize = 600;
/// How many times one rule may block the same file within one turn.
pub const MAX_BLOCKS_PER_RULE_PER_TURN: usize = 2;
/// Largest diff a check is attempted on at all.
pub const MAX_DIFF_INPUT_CHARS: usize = 1_000_000;
/// List price observed 2026-09-17: $0.042 per million input tokens, output free.
pub const JEV_USD_PER_INPUT_TOKEN: f64 = 0.042 / 1_000_000.0;

// ─── schema (mirror packages/schema/src/rubric.ts) ──────────────────

pub const RUBRIC_VERSION: u32 = 1;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RuleWhen {
    Edit,
    Turn,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RuleStatus {
    Active,
    Weak,
    Noisy,
    Disabled,
}

#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct Thresholds {
    pub act: f64,
    pub flag: f64,
}

#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Band {
    Act,
    Flag,
    Clear,
}

/// Offline determinism extension. A model question may carry one; the offline
/// backend then answers from tokens instead of asking a model. Never sent to
/// a live model.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Heuristic {
    /// Any one of these tokens in the state text counts as a violation signal.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub match_any: Option<Vec<String>>,
    /// Violation probability when the signal is present (default 0.95).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub p_violated: Option<f64>,
    /// Violation probability when it is absent (default 0.05).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub p_ok: Option<f64>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum Question {
    Boolean {
        instructions: String,
        #[serde(skip_serializing_if = "Option::is_none")]
        criteria: Option<Value>,
        #[serde(skip_serializing_if = "Option::is_none")]
        heuristic: Option<Heuristic>,
    },
    Choice {
        instructions: String,
        criteria: std::collections::BTreeMap<String, String>,
        /// Options that count as a violation. The rest are compliant.
        violating: Vec<String>,
        #[serde(skip_serializing_if = "Option::is_none")]
        heuristic: Option<Heuristic>,
    },
    Score {
        instructions: String,
        /// Ordered levels from fully compliant (index 0) to worst.
        criteria: Vec<String>,
        /// Zero-based level index from which the answer counts as a violation.
        #[serde(rename = "violatingFrom")]
        violating_from: u32,
        #[serde(skip_serializing_if = "Option::is_none")]
        heuristic: Option<Heuristic>,
    },
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum Check {
    Lint {
        #[serde(skip_serializing_if = "Option::is_none")]
        how: Option<String>,
        #[serde(skip_serializing_if = "Option::is_none")]
        pattern: Option<String>,
        #[serde(skip_serializing_if = "Option::is_none")]
        overlaps: Option<String>,
    },
    Model {
        question: Question,
        #[serde(skip_serializing_if = "Option::is_none")]
        overlaps: Option<String>,
        /// Trap for the most common authoring mistake: `heuristic` belongs on
        /// the *question*, not on the check. Serde would otherwise silently
        /// drop it and the offline backend would never fire. Captured here so
        /// `validate` can name the bug; it is never read by the engines.
        #[serde(skip_serializing_if = "Option::is_none")]
        heuristic: Option<Heuristic>,
    },
    Deferred {
        reason: String,
    },
    Unenforceable {
        reason: String,
    },
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct RuleSource {
    pub path: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub line: Option<u32>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Rule {
    pub id: String,
    /// The rule as the user wrote it, quoted or lightly shortened.
    pub text: String,
    pub source: RuleSource,
    /// Globs relative to the repo root. Absent means every file.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub scope: Option<Vec<String>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub when: Option<RuleWhen>,
    pub check: Check,
    #[serde(default = "default_rule_status")]
    pub status: RuleStatus,
}

fn default_rule_status() -> RuleStatus {
    RuleStatus::Active
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct RubricSource {
    /// Repo-relative path, or "~/..." for a file in the home directory.
    pub path: String,
    /// sha256 hex of the file bytes. Filled by `rules compile` / `validate`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub sha: Option<String>,
    /// Glob the file's rules apply to. Defaults to the file's directory.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub scope: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Rubric {
    pub version: u32,
    #[serde(rename = "compiledAt")]
    pub compiled_at: String,
    #[serde(skip_serializing_if = "Option::is_none", rename = "compiledBy")]
    pub compiled_by: Option<String>,
    pub sources: Vec<RubricSource>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub thresholds: Option<Thresholds>,
    pub rules: Vec<Rule>,
}

impl Rubric {
    pub fn thresholds(&self) -> Thresholds {
        self.thresholds.unwrap_or(DEFAULT_THRESHOLDS)
    }
}

// ─── verdict (mirror packages/schema/src/verdict.ts) ────────────────

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Verdict {
    pub rule_id: String,
    /// Probability that the rule is violated, 0 to 1.
    pub probability: f64,
    pub band: Band,
    /// For choice and score answers, what the model picked.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub answer: Option<String>,
    /// Set only when judged on one file.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub file: Option<String>,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Usage {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub input_tokens: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub output_tokens: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cost_usd: Option<f64>,
}

/// One changed file, repo-relative path + unified-diff text.
#[derive(Clone, Debug, PartialEq)]
pub struct FileDiff {
    pub file: String,
    pub text: String,
}

// ─── glob (picomatch-compatible subset: *, **, ?, [..]) ─────────────

/// Compile a glob into a regex that matches a repo-relative posix path.
/// `*` never crosses `/`; `**` does; `?` is a single non-`/` char; `[..]`
/// character classes are copied through. `dot` behaviour: leading-dot path
/// segments still match (abide's scope matcher uses `{ dot: true }`).
pub fn glob_to_regex(pattern: &str) -> Result<String> {
    let mut out = String::from("^");
    let chars: Vec<char> = pattern.chars().collect();
    let mut i = 0usize;
    while i < chars.len() {
        match chars[i] {
            '*' => {
                if i + 1 < chars.len() && chars[i + 1] == '*' {
                    while i + 1 < chars.len() && chars[i + 1] == '*' {
                        i += 1;
                    }
                    if i + 1 < chars.len() && chars[i + 1] == '/' {
                        // a/**/b matches a/b (zero segments)
                        out.push_str("(?:.*/)?");
                        i += 1;
                    } else {
                        out.push_str(".*");
                    }
                } else {
                    out.push_str("[^/]*");
                }
            }
            '?' => out.push_str("[^/]"),
            '[' => {
                let mut j = i + 1;
                let mut cls = String::new();
                if j < chars.len() && (chars[j] == '!' || chars[j] == '^') {
                    cls.push('^');
                    j += 1;
                }
                let mut closed = false;
                while j < chars.len() {
                    if chars[j] == ']' {
                        closed = true;
                        break;
                    }
                    cls.push(chars[j]);
                    j += 1;
                }
                if closed {
                    out.push('[');
                    out.push_str(&cls);
                    out.push(']');
                    i = j;
                } else {
                    out.push_str("\\[");
                }
            }
            c if c.is_ascii_alphanumeric() || c == '_' || c == '-' => out.push(c),
            c => {
                out.push('\\');
                out.push(c);
            }
        }
        i += 1;
    }
    out.push('$');
    Ok(out)
}

/// Rule matches a repo-relative path: no scope means every file.
pub fn rule_applies_to(rule: &Rule, relative_path: &str) -> bool {
    match &rule.scope {
        None => true,
        Some(globs) => globs.iter().any(|g| glob_match(g, relative_path)),
    }
}

pub fn glob_match(pattern: &str, path: &str) -> bool {
    match glob_to_regex(pattern) {
        Ok(re) => regex_lite::Regex::new(&re)
            .map(|rx| rx.is_match(path))
            .unwrap_or(false),
        Err(_) => false,
    }
}

// ─── probability + band (mirror packages/cli/src/lib/band.ts) ───────

/// The model's raw answer, whatever the question type.
#[derive(Clone, Debug)]
pub enum ModelAnswer {
    Boolean { probability: f64 },
    Choice { choice: String, probabilities: Option<std::collections::BTreeMap<String, f64>> },
    Score { score: f64, probabilities: Option<std::collections::BTreeMap<String, f64>> },
}

fn clamp01(n: f64) -> f64 { n.max(0.0).min(1.0) }

/// Probability that the rule is violated, whatever the question type.
pub fn violation_probability(question: &Question, answer: &ModelAnswer) -> (f64, Option<String>) {
    match question {
        Question::Boolean { .. } => match answer {
            ModelAnswer::Boolean { probability } => (clamp01(*probability), None),
            _ => (0.0, None),
        },
        Question::Choice {
            criteria, violating, ..
        } => {
            let ModelAnswer::Choice {
                choice,
                probabilities,
            } = answer
            else {
                return (0.0, None);
            };
            let violating: HashSet<&str> = violating.iter().map(|s| s.as_str()).collect();
            let mass: f64 = if let Some(probs) = probabilities {
                probs
                    .iter()
                    .filter(|(name, _)| violating.contains(name.as_str()))
                    .map(|(_, p)| *p)
                    .sum()
            } else if violating.contains(choice.as_str()) {
                1.0
            } else {
                0.0
            };
            let _ = criteria; // criteria used by the caller for labels; keep the match arm explicit
            (clamp01(mass), Some(choice.clone()))
        }
        Question::Score {
            criteria,
            violating_from,
            ..
        } => {
            let ModelAnswer::Score { score, probabilities } = answer else {
                return (0.0, None);
            };
            let level = score.round() as isize;
            let label = criteria
                .get(level.max(0) as usize)
                .cloned()
                .unwrap_or_else(|| level.to_string());
            let mass: f64 = if let Some(probs) = probabilities {
                probs
                    .iter()
                    .filter(|(index, _)| {
                        index.parse::<i64>().map(|v| v >= *violating_from as i64).unwrap_or(false)
                    })
                    .map(|(_, p)| *p)
                    .sum()
            } else if (level as i64) >= *violating_from as i64 {
                1.0
            } else {
                0.0
            };
            (clamp01(mass), Some(label))
        }
    }
}

pub fn band_for(probability: f64, thresholds: Thresholds) -> Band {
    if probability >= thresholds.act {
        Band::Act
    } else if probability >= thresholds.flag {
        Band::Flag
    } else {
        Band::Clear
    }
}

// ─── model answer from a Decide response ────────────────────────────

fn model_answer_from_decision(question: &Question, decision: &Decision) -> ModelAnswer {
    let probabilities = if decision.probabilities.is_empty() {
        None
    } else {
        let mut m: std::collections::BTreeMap<String, f64> = std::collections::BTreeMap::new();
        for (k, v) in decision.probabilities.iter() {
            m.insert(k.clone(), v.as_f64().unwrap_or(0.0));
        }
        Some(m)
    };
    match question {
        Question::Boolean { .. } => ModelAnswer::Boolean {
            probability: decision.answer.as_f64().unwrap_or(0.0),
        },
        Question::Choice { .. } => ModelAnswer::Choice {
            choice: decision.as_str(),
            probabilities,
        },
        Question::Score { .. } => ModelAnswer::Score {
            score: decision.answer.as_f64().unwrap_or(0.0),
            probabilities,
        },
    }
}

// ─── run check (mirror packages/cli/src/lib/checkRunner.ts) ─────────

#[derive(Clone)]
pub struct CheckRequest {
    pub phase: RuleWhen,
    /// One entry per touched file, paths repo-relative.
    pub file_diffs: Vec<FileDiff>,
    pub task: Option<String>,
    pub rules: Vec<Rule>,
    pub thresholds: Thresholds,
    /// True only for the offline backend, which reads the `heuristic`
    /// extension. A live model never sees it.
    pub include_heuristic: bool,
}

pub struct CheckOutcome {
    pub verdicts: Vec<Verdict>,
    /// Rule ids that took part in the model check.
    pub model_rules: Vec<String>,
    /// How many model calls the check took: one per distinct set of in-scope files.
    pub calls: usize,
    pub input_tokens: u64,
    pub latency_ms: u64,
}

impl Default for CheckOutcome {
    fn default() -> Self {
        Self {
            verdicts: vec![],
            model_rules: vec![],
            calls: 0,
            input_tokens: 0,
            latency_ms: 0,
        }
    }
}

/// Only model rules run. A lint-shaped rule is the linter's to enforce, and
/// abide only reports it.
fn runs_in_phase(rule: &Rule, phase: RuleWhen) -> bool {
    rule.status == RuleStatus::Active
        && matches!(rule.check, Check::Model { .. })
        && rule.when == Some(phase)
}

/// Rules that are active, belong to this phase, and apply to at least one file.
pub fn select_rules(rules: &[Rule], phase: RuleWhen, files: &[String]) -> Vec<Rule> {
    rules
        .iter()
        .filter(|rule| runs_in_phase(rule, phase) && files.iter().any(|f| rule_applies_to(rule, f)))
        .cloned()
        .collect()
}

fn render_files(file_diffs: &[FileDiff]) -> String {
    file_diffs
        .iter()
        .map(|f| format!("--- a/{}\n+++ b/{}\n{}", f.file, f.file, f.text))
        .collect::<Vec<_>>()
        .join("\n")
}

pub struct ScopeGroup<'a> {
    pub rules: Vec<&'a Rule>,
    pub file_diffs: Vec<FileDiff>,
}

/// Rules grouped by the files each one applies to, so no rule ever sees a
/// file outside its scope. Group key is the sorted-in-order file list.
pub fn group_by_scope<'a>(rules: &[&'a Rule], file_diffs: &[FileDiff]) -> Vec<ScopeGroup<'a>> {
    let mut order: Vec<String> = Vec::new();
    let mut groups: HashMap<String, ScopeGroup<'a>> = HashMap::new();
    for rule in rules {
        let in_scope: Vec<FileDiff> = file_diffs
            .iter()
            .filter(|f| rule_applies_to(rule, &f.file))
            .cloned()
            .collect();
        if in_scope.is_empty() {
            continue;
        }
        let key = in_scope
            .iter()
            .map(|f| f.file.clone())
            .collect::<Vec<_>>()
            .join("\n");
        let group = groups.entry(key.clone()).or_insert_with(|| {
            order.push(key.clone());
            ScopeGroup {
                rules: vec![],
                file_diffs: in_scope,
            }
        });
        group.rules.push(*rule);
    }
    order
        .into_iter()
        .filter_map(|k| groups.remove(&k))
        .collect()
}

/// One verdict per rule: the loudest.
pub fn loudest_verdicts(verdicts: &[Verdict]) -> Vec<Verdict> {
    let mut best: HashMap<String, Verdict> = HashMap::new();
    for v in verdicts {
        match best.get(&v.rule_id) {
            Some(have) if have.probability >= v.probability => {}
            _ => {
                best.insert(v.rule_id.clone(), v.clone());
            }
        }
    }
    let mut out: Vec<Verdict> = best.into_values().collect();
    out.sort_by(|a, b| a.rule_id.cmp(&b.rule_id));
    out
}

/// Serialise a question for a backend. `include_heuristic` is true only for
/// the offline backend, which reads the extension; a live model never sees it.
fn question_json(question: &Question, include_heuristic: bool) -> Value {
    let mut v = serde_json::to_value(question).unwrap_or(Value::Null);
    if !include_heuristic {
        if let Some(o) = v.as_object_mut() {
            o.remove("heuristic");
        }
    }
    v
}

fn build_state(task: &Option<String>, phase: RuleWhen, only: Option<&FileDiff>, file_diffs: &[FileDiff]) -> Value {
    let mut m = Map::new();
    if let Some(t) = task {
        m.insert("task".to_string(), json!(t));
    }
    match (phase, only) {
        (RuleWhen::Edit, Some(only)) => {
            m.insert("file".to_string(), json!(only.file));
            m.insert("diff".to_string(), json!(only.text));
        }
        _ => {
            m.insert(
                "files".to_string(),
                json!(file_diffs.iter().map(|f| f.file.clone()).collect::<Vec<_>>()),
            );
            m.insert("diff".to_string(), json!(render_files(file_diffs)));
        }
    }
    Value::Object(m)
}

fn build_questions(rules: &[&Rule], include_heuristic: bool) -> Value {
    let mut m = Map::new();
    for rule in rules {
        if let Check::Model { question, .. } = &rule.check {
            m.insert(rule.id.clone(), question_json(question, include_heuristic));
        }
    }
    Value::Object(m)
}

/// The check itself. One backend call per scope group; each rule's verdict is
/// derived from the answer, the violation probability and the band.
pub fn run_check(request: &CheckRequest, backend: &dyn Decide) -> Result<CheckOutcome> {
    let files: Vec<String> = request.file_diffs.iter().map(|f| f.file.clone()).collect();
    let selected = select_rules(&request.rules, request.phase, &files);
    let refs: Vec<&Rule> = selected.iter().collect();
    let groups = group_by_scope(&refs, &request.file_diffs);
    let group_count = groups.len();
    if groups.is_empty() {
        return Ok(CheckOutcome {
            model_rules: selected.iter().map(|r| r.id.clone()).collect(),
            ..Default::default()
        });
    }
    let started = Instant::now();
    let mut all_verdicts: Vec<Verdict> = Vec::new();
    let mut input_tokens: u64 = 0;
    for group in groups {
        let only = if group.file_diffs.len() == 1 {
            Some(&group.file_diffs[0])
        } else {
            None
        };
        let state = build_state(&request.task, request.phase, only, &group.file_diffs);
        let questions = build_questions(&group.rules, request.include_heuristic);
        let verdict = backend.decide(&state, &questions)?;
        input_tokens += verdict.input_tokens as u64;
        for rule in &group.rules {
            if let Check::Model { question, .. } = &rule.check {
                let Some(decision) = verdict.answers.get(&rule.id) else {
                    continue;
                };
                let answer = model_answer_from_decision(question, decision);
                let (probability, picked) = violation_probability(question, &answer);
                let mut v = Verdict {
                    rule_id: rule.id.clone(),
                    probability,
                    band: band_for(probability, request.thresholds),
                    answer: picked,
                    file: None,
                };
                if let Some(only) = only {
                    v.file = Some(only.file.clone());
                }
                all_verdicts.push(v);
            }
        }
    }
    let verdicts = loudest_verdicts(&all_verdicts);
    Ok(CheckOutcome {
        model_rules: selected.iter().map(|r| r.id.clone()).collect(),
        calls: group_count,
        input_tokens,
        latency_ms: started.elapsed().as_millis() as u64,
        verdicts,
    })
}

/// A single model call carrying every rule — the shape `checkWithModel` takes.
/// `include_heuristic` is true for the offline backend only.
pub fn check_with_model(
    rules: &[&Rule],
    state: &Value,
    thresholds: Thresholds,
    backend: &dyn Decide,
    include_heuristic: bool,
) -> Result<Vec<Verdict>> {
    if rules.is_empty() {
        return Ok(vec![]);
    }
    let questions = build_questions(rules, include_heuristic);
    let verdict = backend.decide(state, &questions)?;
    let mut out = Vec::new();
    for rule in rules {
        if let Check::Model { question, .. } = &rule.check {
            let Some(decision) = verdict.answers.get(&rule.id) else {
                continue;
            };
            let answer = model_answer_from_decision(question, decision);
            let (probability, picked) = violation_probability(question, &answer);
            out.push(Verdict {
                rule_id: rule.id.clone(),
                probability,
                band: band_for(probability, thresholds),
                answer: picked,
                file: None,
            });
        }
    }
    Ok(out)
}

// ─── offline backend (deterministic; reads the heuristic extension) ──

/// Deterministic rules backend for offline runs and CI. Reads each question's
/// optional `heuristic` block and answers from token matches on the serialised
/// state. A question without one is answered compliant (probability ~0).
pub struct RulesHeuristicBackend;

impl Decide for RulesHeuristicBackend {
    fn decide(&self, state: &Value, questions: &Value) -> Result<crate::workflow::Verdict> {
        let text = serde_json::to_string(state).unwrap_or_default();
        let mut answers = HashMap::new();
        let Some(qs) = questions.as_object() else {
            return Ok(crate::workflow::Verdict::default());
        };
        for (qid, qdef) in qs {
            let qtype = qdef.get("type").and_then(|v| v.as_str()).unwrap_or("boolean");
            let h = qdef.get("heuristic");
            let hits = h
                .and_then(|h| h.get("match_any"))
                .and_then(|v| v.as_array())
                .map(|arr| {
                    arr.iter()
                        .filter_map(|t| t.as_str())
                        .any(|tok| crate::backend::contains_any(&text, &[tok]))
                })
                .unwrap_or(false);
            let p_v = h
                .and_then(|h| h.get("p_violated"))
                .and_then(|v| v.as_f64())
                .unwrap_or(0.95);
            let p_ok = h
                .and_then(|h| h.get("p_ok"))
                .and_then(|v| v.as_f64())
                .unwrap_or(0.05);
            let (answer, probs, conf) = match qtype {
                "boolean" => {
                    let p = if hits { p_v } else { p_ok };
                    let mut m = Map::new();
                    m.insert("false".to_string(), json!(1.0 - p));
                    m.insert("true".to_string(), json!(p));
                    (json!(p), m, p.max(1.0 - p))
                }
                "choice" => {
                    let criteria = qdef
                        .get("criteria")
                        .and_then(|c| c.as_object())
                        .cloned()
                        .unwrap_or_default();
                    let violating: Vec<String> = qdef
                        .get("violating")
                        .and_then(|v| v.as_array())
                        .map(|arr| arr.iter().filter_map(|s| s.as_str().map(String::from)).collect())
                        .unwrap_or_default();
                    let keys: Vec<String> = criteria.keys().cloned().collect();
                    let vs: HashSet<&str> = violating.iter().map(|s| s.as_str()).collect();
                    let chosen = if hits {
                        violating.iter().find(|k| criteria.contains_key(k.as_str())).cloned()
                    } else {
                        keys.iter().find(|k| !vs.contains(k.as_str())).cloned()
                    }
                    .unwrap_or_else(|| keys.first().cloned().unwrap_or_default());
                    let mut m = Map::new();
                    for k in &keys {
                        let is_violating = vs.contains(k.as_str());
                        let p = if is_violating {
                            if hits && k == &chosen {
                                p_v
                            } else {
                                0.0
                            }
                        } else if !hits && k == &chosen {
                            1.0
                        } else {
                            0.0
                        };
                        m.insert(k.clone(), json!(p));
                    }
                    (json!(chosen), m, if hits { p_v } else { 1.0 - p_ok })
                }
                "score" => {
                    let criteria: Vec<String> = qdef
                        .get("criteria")
                        .and_then(|c| c.as_array())
                        .map(|arr| arr.iter().filter_map(|s| s.as_str().map(String::from)).collect())
                        .unwrap_or_default();
                    let violating_from = qdef
                        .get("violatingFrom")
                        .or_else(|| qdef.get("violating_from"))
                        .and_then(|v| v.as_u64())
                        .unwrap_or(1) as usize;
                    let level = if hits {
                        violating_from.min(criteria.len().saturating_sub(1))
                    } else {
                        0
                    };
                    let mut m = Map::new();
                    for (i, _) in criteria.iter().enumerate() {
                        let is_violating = i >= violating_from;
                        let p = if is_violating {
                            if hits && i == level {
                                p_v
                            } else {
                                0.0
                            }
                        } else if !hits && i == level {
                            1.0
                        } else {
                            0.0
                        };
                        m.insert(i.to_string(), json!(p));
                    }
                    (json!(level as f64), m, if hits { p_v } else { 1.0 - p_ok })
                }
                other => bail!("unknown rules question type {other:?} for {qid}"),
            };
            answers.insert(
                qid.clone(),
                Decision {
                    answer,
                    probabilities: probs,
                    confidence: conf,
                },
            );
        }
        Ok(crate::workflow::Verdict {
            answers,
            input_tokens: 0,
            latency_ms: 0.0,
        })
    }
}

/// Backend factory: live Laya when `base_url` is set, else offline heuristic.
pub fn make_backend(base_url: Option<&str>) -> Box<dyn Decide> {
    match base_url {
        Some(u) => Box::new(LayaBackend::new(u)),
        None => Box::new(RulesHeuristicBackend),
    }
}

// ─── events (mirror packages/cli/src/lib/events.ts + schema/events) ──

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum RulesEvent {
    Check {
        at: String,
        phase: RuleWhen,
        #[serde(skip_serializing_if = "Option::is_none")]
        session_id: Option<String>,
        #[serde(skip_serializing_if = "Option::is_none")]
        prompt_id: Option<String>,
        files: Vec<String>,
        rules: u32,
        latency_ms: u64,
        #[serde(skip_serializing_if = "Option::is_none")]
        model_latency_ms: Option<u64>,
        #[serde(skip_serializing_if = "Option::is_none")]
        usage: Option<Usage>,
        verdicts: Vec<Verdict>,
        blocked: bool,
    },
    Skip {
        at: String,
        phase: RuleWhen,
        #[serde(skip_serializing_if = "Option::is_none")]
        session_id: Option<String>,
        reason: String,
        #[serde(skip_serializing_if = "Option::is_none")]
        files: Option<Vec<String>>,
    },
    Error {
        at: String,
        /// A check phase, or "session".
        phase: String,
        #[serde(skip_serializing_if = "Option::is_none")]
        session_id: Option<String>,
        code: String,
        message: String,
        #[serde(skip_serializing_if = "Option::is_none")]
        latency_ms: Option<u64>,
    },
    CompileNeeded {
        at: String,
        #[serde(skip_serializing_if = "Option::is_none")]
        session_id: Option<String>,
        reason: String,
        sources: Vec<String>,
    },
}

/// Best effort. The log must never take the caller down with it.
pub fn append_event(root: &Path, event: &RulesEvent) {
    let file = events_path(root);
    if let Some(dir) = file.parent() {
        let _ = std::fs::create_dir_all(dir);
    }
    let line = serde_json::to_string(event).unwrap_or_default();
    use std::io::Write;
    if let Ok(mut f) = std::fs::OpenOptions::new().create(true).append(true).open(&file) {
        let _ = writeln!(f, "{line}");
    }
}

/// Read every parseable event, skipping torn lines.
pub fn read_events(root: &Path) -> Vec<RulesEvent> {
    let file = events_path(root);
    let raw = match std::fs::read_to_string(&file) {
        Ok(r) => r,
        Err(_) => return vec![],
    };
    let mut out = Vec::new();
    for line in raw.lines() {
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        if let Ok(ev) = serde_json::from_str::<RulesEvent>(line) {
            out.push(ev);
        }
    }
    out
}

// ─── reason (mirror packages/cli/src/lib/reason.ts) ─────────────────

type Violation<'a> = (&'a Rule, Verdict);

fn where_of(rule: &Rule) -> String {
    match rule.source.line {
        Some(line) => format!("{} line {line}", rule.source.path),
        None => rule.source.path.clone(),
    }
}

fn quote(text: &str) -> String {
    let trimmed: String = text.split_whitespace().collect::<Vec<_>>().join(" ");
    if trimmed.chars().count() > 220 {
        let mut out: String = trimmed.chars().take(217).collect();
        out.push_str("...");
        out
    } else {
        trimmed
    }
}

fn blames(rule: &Rule, verdict: &Verdict, file: &str) -> bool {
    match &verdict.file {
        None => rule_applies_to(rule, file),
        Some(vf) => vf == file,
    }
}

/// Files an `act` verdict blames. A deleted file cannot be repaired and is
/// excluded upstream (the caller filters `repairable`).
pub fn files_to_repair(violations: &[Violation], files: &[String]) -> Vec<String> {
    let mut seen = HashSet::new();
    let mut out = Vec::new();
    for f in files {
        if seen.insert(f.clone()) && violations.iter().any(|(rule, v)| blames(rule, v, f)) {
            out.push(f.clone());
        }
    }
    out
}

fn evidence(rule: &Rule, verdict: &Verdict) -> String {
    let score = format!("{:.2}", verdict.probability);
    let at = match &verdict.file {
        Some(f) => format!(" in {f}"),
        None => String::new(),
    };
    if matches!(rule.check, Check::Lint { .. }) {
        if let Some(a) = &verdict.answer {
            return format!(" Matched{at}: {a}");
        }
        return String::new();
    }
    match &verdict.answer {
        Some(a) => format!(" Judged{at}: {a} ({score})."),
        None if verdict.file.is_some() => format!(" Scored {score}{at}."),
        None => format!(" ({score})"),
    }
}

/// One line per broken rule. Never the whole instruction file.
pub fn repair_reason(phase: RuleWhen, violations: &[Violation], files: &[String]) -> String {
    let mut by_rule: Vec<(&Rule, Vec<&Verdict>)> = Vec::new();
    let mut order: Vec<String> = Vec::new();
    let mut index: HashMap<String, usize> = HashMap::new();
    for (rule, verdict) in violations {
        let i = match index.get(&rule.id) {
            Some(i) => *i,
            None => {
                index.insert(rule.id.clone(), by_rule.len());
                order.push(rule.id.clone());
                by_rule.push((rule, vec![]));
                by_rule.len() - 1
            }
        };
        by_rule[i].1.push(verdict);
    }
    let lines: Vec<String> = order
        .iter()
        .filter_map(|id| {
            let i = index.get(id)?;
            let (rule, verdicts) = &by_rule[*i];
            let ev: String = verdicts.iter().map(|v| evidence(rule, v)).collect();
            Some(format!(
                "Rule \"{}\" from {}: \"{}\".{}",
                rule.id,
                where_of(rule),
                quote(&rule.text),
                ev
            ))
        })
        .collect();
    let targets = files_to_repair(violations, files);
    let subject = match phase {
        RuleWhen::Edit => "This edit appears",
        RuleWhen::Turn => "The changes in this turn appear",
    };
    let target = if targets.len() == 1 {
        targets[0].clone()
    } else {
        format!("{} files ({})", targets.len(), targets.join(", "))
    };
    let ask = match phase {
        RuleWhen::Edit => format!("Repair {target} now, then continue with the task."),
        RuleWhen::Turn => format!("Repair {target} before you finish. Keep the fix to what the rule asks."),
    };
    let count_phrase = if lines.len() == 1 { "a rule".to_string() } else { format!("{} rules", lines.len()) };
    format!(
        "Rules: {subject} to break {count_phrase} from this repository's instructions.\n{}\n{ask}",
        lines.iter().map(|l| format!("- {l}")).collect::<Vec<_>>().join("\n")
    )
}

pub fn flag_notice(phase: RuleWhen, flagged: &[Violation], files: &[String]) -> String {
    let list: Vec<String> = flagged
        .iter()
        .map(|(rule, v)| {
            let p = format!("{:.2}", v.probability);
            match &v.file {
                Some(f) => format!("{} {p} in {f}", rule.id),
                None => format!("{} {p}", rule.id),
            }
        })
        .collect();
    let on = if flagged.iter().any(|(_, v)| v.file.is_none()) {
        format!(" on {}", files.join(", "))
    } else {
        String::new()
    };
    let phase_label = match phase {
        RuleWhen::Edit => "edit",
        RuleWhen::Turn => "turn",
    };
    format!(
        "rules: uncertain about {}{on} ({phase_label}). Not sent to the agent. Details in ~/.laya-workflow/rules/events.jsonl.",
        list.join(", ")
    )
}

// ─── paths + rubric read/write (mirror paths.ts / rubricFile.ts) ─────

/// The nearest ancestor that looks like a repository root, else the start dir.
pub fn find_repo_root(start: &Path) -> PathBuf {
    let mut dir = if start.is_dir() {
        start.to_path_buf()
    } else {
        start
            .parent()
            .map(|p| p.to_path_buf())
            .unwrap_or_else(|| start.to_path_buf())
    };
    let mut fallback: Option<PathBuf> = None;
    let markers = [".git", ".rules", "AGENTS.md", "CLAUDE.md"];
    loop {
        if dir.join(".git").exists() {
            return dir;
        }
        if fallback.is_none() && markers.iter().any(|m| dir.join(m).exists()) {
            fallback = Some(dir.clone());
        }
        match dir.parent() {
            Some(parent) => {
                if parent == dir {
                    break;
                }
                dir = parent.to_path_buf();
            }
            None => break,
        }
    }
    fallback.unwrap_or_else(|| start.to_path_buf())
}

/// The repo-local rubric dir: `<repo>/.rules`. Committed with the repo so the
/// team and CI share one rule set — this is the file `rules compile` writes.
pub fn rules_dir(root: &Path) -> PathBuf {
    root.join(".rules")
}

pub fn rubric_path(root: &Path) -> PathBuf {
    rules_dir(root).join("rubric.json")
}

/// The per-user audit log: `$LAYA_HOME/rules/events.jsonl`
/// (`~/.laya-workflow/rules/events.jsonl` by default). Runtime output — the
/// repo never sees it, so `git status` stays clean while `report` / `audit`
/// still aggregate every check this user ran.
pub fn events_path(_root: &Path) -> PathBuf {
    let base = crate::state::state_dir()
        .unwrap_or_else(|| std::path::PathBuf::from(".laya-workflow"));
    base.join("rules").join("events.jsonl")
}

#[derive(Clone, Debug)]
pub enum RubricRead {
    Missing { path: PathBuf },
    Invalid { path: PathBuf, issues: Vec<String> },
    Ok { path: PathBuf, rubric: Rubric },
}

/// Parse + structurally validate a rubric file.
pub fn read_rubric(file: &Path) -> RubricRead {
    let raw = match std::fs::read_to_string(file) {
        Ok(r) => r,
        Err(_) => return RubricRead::Missing { path: file.to_path_buf() },
    };
    let parsed: Result<Rubric, _> = serde_json::from_str(&raw);
    let rubric = match parsed {
        Ok(r) => r,
        Err(e) => {
            return RubricRead::Invalid {
                path: file.to_path_buf(),
                issues: vec![e.to_string()],
            }
        }
    };
    let issues = validate_rubric(&rubric);
    if issues.is_empty() {
        RubricRead::Ok {
            path: file.to_path_buf(),
            rubric,
        }
    } else {
        RubricRead::Invalid {
            path: file.to_path_buf(),
            issues,
        }
    }
}

/// Structural validation mirroring the zod schema's refine/superRefine rules.
/// Returns a list of human-readable issues; empty means valid.
pub fn validate_rubric(rubric: &Rubric) -> Vec<String> {
    let mut issues = Vec::new();
    if rubric.version != RUBRIC_VERSION {
        issues.push(format!(
            "version: expected {RUBRIC_VERSION}, got {}",
            rubric.version
        ));
    }
    let mut seen: HashSet<&str> = HashSet::new();
    for (i, rule) in rubric.rules.iter().enumerate() {
        let p = format!("rules[{i}]");
        if !rule.id.contains('-') && rule.id.chars().any(|c| !c.is_ascii_alphanumeric() && c != '-') {
            issues.push(format!("{p}.id \"{}\" is not kebab-case", rule.id));
        }
        if rule.text.is_empty() || rule.text.chars().count() > 600 {
            issues.push(format!("{p}.text must be 1..600 chars"));
        }
        if seen.contains(rule.id.as_str()) {
            issues.push(format!("{p}.id: duplicate rule id \"{}\"", rule.id));
        }
        seen.insert(&rule.id);
        match &rule.check {
            Check::Model {
                question,
                heuristic: misplaced,
                ..
            } => {
                if rule.when.is_none() {
                    issues.push(format!(
                        "{p}: rule \"{}\" is model-checked and needs \"when\": \"edit\" or \"turn\"",
                        rule.id
                    ));
                }
                if misplaced.is_some() {
                    issues.push(format!(
                        "{p}.check: unknown field \"heuristic\" — move it to {p}.check.question.heuristic \
                         (the offline backend reads the question's heuristic, not the check's)"
                    ));
                }
                validate_question(question, &format!("{p}.check.question"), &mut issues);
            }
            Check::Lint { how, pattern, .. } => {
                if how.is_none() && pattern.is_none() {
                    issues.push(format!(
                        "{p}: lint rule \"{}\" must say how a linter would enforce it, or give the pattern",
                        rule.id
                    ));
                }
            }
            Check::Deferred { reason } | Check::Unenforceable { reason } => {
                if reason.is_empty() {
                    issues.push(format!("{p}: reason must not be empty"));
                }
            }
        }
    }
    if let Some(t) = &rubric.thresholds {
        if !(0.0..=1.0).contains(&t.act) || !(0.0..=1.0).contains(&t.flag) {
            issues.push("thresholds: act and flag must be 0..1".to_string());
        }
        if t.flag >= t.act {
            issues.push("thresholds: flag must sit below act".to_string());
        }
    }
    issues
}

fn validate_question(q: &Question, path: &str, issues: &mut Vec<String>) {
    if q.instructions().is_empty() || q.instructions().chars().count() > 2000 {
        issues.push(format!("{path}.instructions must be 1..2000 chars"));
    }
    match q {
        Question::Choice { criteria, violating, .. } => {
            if criteria.len() < 2 {
                issues.push(format!("{path}: a choice question needs at least two options"));
            }
            let vs: HashSet<&str> = violating.iter().map(|s| s.as_str()).collect();
            for v in violating {
                if !criteria.contains_key(v) {
                    issues.push(format!("{path}: violating option \"{v}\" must be one of the criteria"));
                }
            }
            if vs.len() >= criteria.len() {
                issues.push(format!("{path}: at least one option must be compliant"));
            }
        }
        Question::Score { criteria, violating_from, .. } => {
            if criteria.len() < 2 {
                issues.push(format!("{path}: a score question needs at least two levels"));
            }
            if *violating_from >= criteria.len() as u32 {
                issues.push(format!("{path}: violatingFrom must point at one of the levels"));
            }
        }
        Question::Boolean { .. } => {}
    }
}

impl Question {
    pub fn instructions(&self) -> &str {
        match self {
            Question::Boolean { instructions, .. }
            | Question::Choice { instructions, .. }
            | Question::Score { instructions, .. } => instructions,
        }
    }
}

/// Write a rubric, creating the parent directory.
pub fn write_rubric(file: &Path, rubric: &Rubric) -> Result<()> {
    if let Some(parent) = file.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let text = serde_json::to_string_pretty(rubric)?;
    std::fs::write(file, format!("{text}\n"))?;
    Ok(())
}

// ─── hook output shapes (mirror schema/hooks.ts HookOutput) ─────────

/// What a hook prints to stdout. Only these shapes ever reach the host.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "kebab-case")]
pub enum HookOutput {
    Silent,
    SessionContext {
        #[serde(rename = "additionalContext")]
        additional_context: String,
        #[serde(skip_serializing_if = "Option::is_none")]
        system_message: Option<String>,
    },
    Block {
        reason: String,
        #[serde(skip_serializing_if = "Option::is_none")]
        system_message: Option<String>,
    },
    Notice {
        system_message: String,
    },
}

/// Assemble the hook output for a check outcome, exactly like the postToolUse
/// / stop hooks: `act` blocks with a repair reason, `flag` notices without
/// blocking, everything else is silent. `repairable` is the set of files that
/// still exist (deleted files cannot be repaired).
pub fn hook_output(
    phase: RuleWhen,
    rules: &[Rule],
    verdicts: &[Verdict],
    repairable: &[String],
) -> HookOutput {
    let by_id: HashMap<&str, &Rule> = rules.iter().map(|r| (r.id.as_str(), r)).collect();
    let all_files = repairable.to_vec();
    // acting: `act` verdicts that blame at least one *repairable* file. A
    // deleted file cannot be repaired, so an act on it only flags (abide).
    let acting: Vec<(&Rule, Verdict)> = verdicts
        .iter()
        .filter(|v| v.band == Band::Act)
        .filter(|v| {
            by_id
                .get(v.rule_id.as_str())
                .map(|rule| !files_to_repair(&[(&**rule, (*v).clone())], &all_files).is_empty())
                .unwrap_or(false)
        })
        .map(|v| (by_id[v.rule_id.as_str()], (*v).clone()))
        .collect();
    let acting_ids: HashSet<&str> = acting.iter().map(|(r, _)| r.id.as_str()).collect();
    // flagged: band != clear and the rule is not already acting.
    let flagged: Vec<(&Rule, Verdict)> = verdicts
        .iter()
        .filter(|v| v.band != Band::Clear)
        .filter(|v| !acting_ids.contains(v.rule_id.as_str()))
        .filter_map(|v| by_id.get(v.rule_id.as_str()).map(|rule| (*rule, (*v).clone())))
        .collect();
    let system_message = if flagged.is_empty() {
        None
    } else {
        Some(flag_notice(phase, &flagged, &all_files))
    };
    if !acting.is_empty() {
        return HookOutput::Block {
            reason: repair_reason(phase, &acting, &all_files),
            system_message,
        };
    }
    match system_message {
        Some(msg) => HookOutput::Notice { system_message: msg },
        None => HookOutput::Silent,
    }
}

// ─── diff input parsing ─────────────────────────────────────────────

/// Parse a `--diff-json` payload: either a single `{file, text}` object or an
/// array of them.
pub fn parse_diff_json(raw: &str) -> Result<Vec<FileDiff>> {
    let value: Value = serde_json::from_str(raw)?;
    let arr: Vec<&Value> = match &value {
        Value::Array(a) => a.iter().collect(),
        _ => vec![&value],
    };
    let mut out = Vec::new();
    for item in arr {
        let file = item
            .get("file")
            .and_then(|v| v.as_str())
            .ok_or_else(|| anyhow!("diff entry missing string \"file\""))?;
        let text = item
            .get("text")
            .and_then(|v| v.as_str())
            .ok_or_else(|| anyhow!("diff entry missing string \"text\""))?;
        out.push(FileDiff {
            file: file.to_string(),
            text: text.to_string(),
        });
    }
    Ok(out)
}

/// Cut a state-sized payload. Mirrors abide's `boundState`: keeps the first
/// `max` chars and marks the cut so the model knows it saw a prefix.
pub fn bound_state(text: &str, max: usize) -> String {
    if text.chars().count() <= max {
        return text.to_string();
    }
    let cut: String = text.chars().take(max).collect();
    format!("{cut}\n… [state truncated at {max} chars]")
}

// ─── compile prompt (mirrors compile.ts + compilePrompt.ts) ─────────

/// Instruction files a compile run starts from, in priority order.
pub const INSTRUCTION_FILES: &[&str] = &["AGENTS.md", "CLAUDE.md"];

/// Emit the prompt that turns AGENTS.md / CLAUDE.md into `.rules/rubric.json`.
/// abide delegates this to a headless agent; here we print the prompt for the
/// user to run in an agent session in the repo (abide's own fallback path).
pub fn compile_prompt(root: &Path) -> String {
    let mut sources: Vec<String> = Vec::new();
    let mut bodies: Vec<String> = Vec::new();
    for name in INSTRUCTION_FILES {
        let p = root.join(name);
        if let Ok(body) = std::fs::read_to_string(&p) {
            sources.push(name.to_string());
            bodies.push(format!("=== {name} ===\n{body}"));
        }
    }
    let sources_list = if sources.is_empty() {
        "AGENTS.md".to_string()
    } else {
        sources.join(", ")
    };
    let bodies_text = bodies.join("\n\n");
    format!(
        "You are compiling {sources_list} into a machine-checkable rubric for the rules rule-enforcement tool.\n\
\n\
Read the instruction files below, then write `.rules/rubric.json` at the repo root.\n\
\n\
{bodies_text}\n\
\n\
The rubric is JSON version 1:\n\
{{\n  \"version\": 1,\n  \"compiledAt\": \"<ISO timestamp>\",\n  \"sources\": [ {{\"path\": \"AGENTS.md\", \"scope\": \"**\"}} ],\n  \"thresholds\": {{ \"act\": 0.8, \"flag\": 0.5 }},\n  \"rules\": [ <rule> ]\n}}\n\
\n\
A rule:\n\
{{\n  \"id\": \"kebab-case-id\",\n  \"text\": \"the rule as written in the source\",\n  \"source\": {{\"path\": \"AGENTS.md\", \"line\": <line>}},\n  \"when\": \"edit\" | \"turn\",\n  \"check\": {{\n    \"type\": \"model\",\n    \"question\": <question>\n  }},\n  \"status\": \"active\"\n}}\n\
\n\
Question shapes:\n\
- boolean: {{\"type\":\"boolean\",\"instructions\":\"…\",\"criteria\":{{\"true\":\"…\",\"false\":\"…\"}}}}\n\
- choice:  {{\"type\":\"choice\",\"instructions\":\"…\",\"criteria\":{{\"option\":\"…\"}},\"violating\":[\"bad-option\"]}}\n\
- score:   {{\"type\":\"score\",\"instructions\":\"…\",\"criteria\":[\"best\",\"ok\",\"bad\"],\"violatingFrom\":2}}\n\
\n\
Rules of thumb:\n\
- Model-checked rules MUST set \"when\": \"edit\" or \"turn\" (edit = per hunk, turn = whole change).\n\
- Keep rule ids kebab-case and unique. Quote each rule's text from the source.\n\
- Decompose prose into single, testable rules; prefer boolean/choice questions for sharp rules.\n\
- Rules a linter already enforces become {{\"type\":\"lint\",\"pattern\":\"…\"}} (reported, never run).\n\
- Optional offline determinism: add \"heuristic\": {{\"match_any\":[\"token\"],\"p_violated\":0.95,\"p_ok\":0.05}}\n\
  to a model question so `rules check` can judge it without a model. Without it,\n\
  offline checks treat the rule as compliant (live model judging via --base-url still applies).\n\
\n\
Write only the JSON file. Do not print it.\n"
    )
}

// ─── init scaffold ──────────────────────────────────────────────────

/// Create `.rules/` with an empty-but-valid rubric and a starter `.rulesignore`.
pub fn cmd_init(root: &Path) -> Result<PathBuf> {
    let dir = rules_dir(root);
    std::fs::create_dir_all(&dir)?;
    let rb = rubric_path(root);
    if !rb.exists() {
        let rubric = Rubric {
            version: RUBRIC_VERSION,
            compiled_at: now_iso(),
            compiled_by: Some("laya-workflow rules init".to_string()),
            sources: vec![],
            thresholds: None,
            rules: vec![],
        };
        write_rubric(&rb, &rubric)?;
    }
    let ignore = dir.join(".rulesignore");
    if !ignore.exists() {
        std::fs::write(&ignore, "# Paths rules never checks, one glob per line.\n# .env\n# *.key\n")?;
    }
    Ok(dir)
}

fn now_iso() -> String {
    let secs = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    // RFC-3339-ish UTC timestamp without a date crate.
    let days = secs / 86_400;
    let (y, m, d) = civil_from_days(days as i64);
    let sod = secs % 86_400;
    let hh = sod / 3600;
    let mm = (sod % 3600) / 60;
    let ss = sod % 60;
    format!("{y:04}-{m:02}-{d:02}T{hh:02}:{mm:02}:{ss:02}Z")
}

/// Days-since-epoch → (year, month, day) civil date (Howard Hinnant algorithm).
fn civil_from_days(z: i64) -> (i64, u32, u32) {
    let z = z + 719_468;
    let era = if z >= 0 { z } else { z - 146_096 } / 146_097;
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    (if m <= 2 { y + 1 } else { y }, m as u32, d as u32)
}

// ─── check / report / audit handlers ────────────────────────────────

pub struct CheckCliInput {
    pub root: PathBuf,
    pub rubric_file: PathBuf,
    pub phase: RuleWhen,
    pub file_diffs: Vec<FileDiff>,
    pub task: Option<String>,
    pub base_url: Option<String>,
    pub session_id: Option<String>,
    pub prompt_id: Option<String>,
}

/// Run one check and write the event + print the hook-output JSON. Returns the
/// hook output so the caller can decide exit behaviour.
pub fn cmd_check(input: &CheckCliInput) -> Result<HookOutput> {
    let read = read_rubric(&input.rubric_file);
    let rubric = match read {
        RubricRead::Ok { rubric, .. } => rubric,
        RubricRead::Missing { path } => {
            bail!("no rubric at {} — run `rules init` then `rules compile`", path.display())
        }
        RubricRead::Invalid { path, issues } => {
            bail!("invalid rubric at {}: {}", path.display(), issues.join("; "))
        }
    };
    let phase = input.phase;
    let include_heuristic = input.base_url.is_none();
    let backend = make_backend(input.base_url.as_deref());
    let at = now_iso();
    let started = Instant::now();

    let request = CheckRequest {
        phase,
        file_diffs: input.file_diffs.clone(),
        task: input.task.clone(),
        rules: rubric.rules.clone(),
        thresholds: rubric.thresholds(),
        include_heuristic,
    };
    let outcome = run_check(&request, backend.as_ref())?;
    let latency_ms = started.elapsed().as_millis() as u64;

    // Files that still exist can be repaired; deleted ones cannot.
    let repairable: Vec<String> = input
        .file_diffs
        .iter()
        .filter(|f| input.root.join(&f.file).exists())
        .map(|f| f.file.clone())
        .collect();
    let output = hook_output(phase, &rubric.rules, &outcome.verdicts, &repairable);
    let blocked = matches!(output, HookOutput::Block { .. });

    let event = RulesEvent::Check {
        at,
        phase,
        session_id: input.session_id.clone(),
        prompt_id: input.prompt_id.clone(),
        files: input.file_diffs.iter().map(|f| f.file.clone()).collect(),
        rules: outcome.model_rules.len() as u32,
        latency_ms,
        model_latency_ms: Some(outcome.latency_ms),
        usage: Some(Usage {
            input_tokens: Some(outcome.input_tokens),
            output_tokens: None,
            cost_usd: outcome
                .input_tokens
                .checked_mul(1_000_000)
                .map(|n| (n as f64) * JEV_USD_PER_INPUT_TOKEN),
        }),
        verdicts: outcome.verdicts.clone(),
        blocked,
    };
    append_event(&input.root, &event);
    Ok(output)
}

/// Summarise the events log: total checks/blocks, band counts per rule.
pub fn cmd_report(root: &Path) -> Result<Value> {
    let events = read_events(root);
    let mut checks = 0u64;
    let mut skips = 0u64;
    let mut errors = 0u64;
    let mut blocks = 0u64;
    let mut per_rule: Map<String, Value> = Map::new();
    for ev in &events {
        match ev {
            RulesEvent::Check { verdicts, blocked, .. } => {
                checks += 1;
                if *blocked {
                    blocks += 1;
                }
                for v in verdicts {
                    let e = per_rule
                        .entry(v.rule_id.clone())
                        .or_insert_with(|| json!({"checks": 0u64, "act": 0u64, "flag": 0u64, "clear": 0u64}));
                    e["checks"] = json!(e["checks"].as_u64().unwrap_or(0) + 1);
                    let band_key = match v.band {
                        Band::Act => "act",
                        Band::Flag => "flag",
                        Band::Clear => "clear",
                    };
                    e[band_key] = json!(e[band_key].as_u64().unwrap_or(0) + 1);
                }
            }
            RulesEvent::Skip { .. } => skips += 1,
            RulesEvent::Error { .. } => errors += 1,
            RulesEvent::CompileNeeded { .. } => {}
        }
    }
    Ok(json!({
        "events": events.len(),
        "checks": checks,
        "blocks": blocks,
        "skips": skips,
        "errors": errors,
        "by_rule": per_rule,
    }))
}

/// Pretty-print every event in order.
pub fn cmd_audit(root: &Path) -> Result<()> {
    let events = read_events(root);
    if events.is_empty() {
        println!("no events in {}", events_path(root).display());
        return Ok(());
    }
    for ev in &events {
        println!("{}", serde_json::to_string_pretty(ev)?);
    }
    Ok(())
}
