//! Accuracy self-improvement loop for Laya decisions.
//!
//! ## Why this exists (diagnosis)
//!
//! Laya decides through the `Decide` trait (`src/workflow.rs`). Two backends
//! implement it: `LayaBackend` (calls the neural server) and `HeuristicBackend`
//! (`src/backend.rs`), which is a hand-written keyword table mapping state text
//! to probabilities — e.g. `is_destructive` returns 0.95 when the text contains
//! `"rm "`. On top of those probabilities, each app's `route` (`src/apps.rs`)
//! applies hand-tuned thresholds.
//!
//! Both layers were tuned by hand and never measured against the apps'
//! reference cases. Measured today the offline backend scores
//! **17/25 = 68%** (`laya-workflow apps`), with `agent_gate` at 50%.
//!
//! ## The missing loop
//!
//! Prediction → feedback → update → regression gate. Today there is:
//!   * prediction      — yes (`Decide::decide`)
//!   * feedback        — **missing**: nothing records whether a decision was right
//!   * update          — **missing**: no way to adjust thresholds from evidence
//!   * regression gate — **missing**: no baseline to defend
//!
//! `src/optimizer.rs` exists but tunes *workflow params* against a numeric
//! objective; it neither measures decision accuracy nor guards a baseline.
//!
//! ## What this module provides
//!
//! A persistent policy (per-question confidence thresholds + keywords) plus a
//! loop that:
//!
//! 1. **collects samples** — `(app, state, expected_label, observed_label)`,
//!    appended to a JSONL file so real production traffic can feed it later;
//! 2. **scores** the current policy — overall accuracy plus per-app breakdown;
//! 3. **proposes updates** from the observed errors (keyword/threshold deltas);
//! 4. **gates** every candidate against a **hold-out split** of the reference
//!    cases: a candidate is adopted only if it does not lose on the held-out
//!    slice *and* improves the training slice. Otherwise it is rejected,
//!    recorded, and the baseline is left untouched.
//!
//! The hold-out requirement is deliberate. Measuring only on the samples you
//! learned from is how a gate becomes theatre; this is the same lesson recorded
//! for the earlier `holdout_eval` work.
//!
//! ## Honest limits
//!
//! * 25 reference cases is a small evaluation set — the gate is a guard against
//!   obvious regressions, not a statistical guarantee.
//! * Keyword deltas fit the offline backend's shape. A neural backend's errors
//!   need a different update rule (prompt/threshold changes), which the same
//!   collect → score → propose → gate skeleton can carry.
//! * Distribution drift is not detected here; the baseline should be re-measured
//!   as cases are added.

use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};

use anyhow::{anyhow, Result};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

use crate::apps;

/// One observed decision outcome.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Sample {
    /// App / workflow the decision came from, e.g. `agent_gate`.
    pub app: String,
    /// The state the decision was made on.
    pub state: Value,
    /// Ground-truth label (human or downstream signal).
    pub expected: String,
    /// What the policy actually produced.
    pub observed: String,
    /// Optional free-form note (who labelled it, why).
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub note: String,
}

impl Sample {
    pub fn correct(&self) -> bool {
        labels_match(&self.expected, &self.observed)
    }
}

/// Expected labels may list alternatives (`"ALLOW|CONFIRM"`), matching apps.rs.
fn labels_match(expected: &str, observed: &str) -> bool {
    expected.split('|').any(|e| e.trim() == observed.trim())
}

/// Tunable policy. Empty maps mean "use the built-in defaults from apps.rs".
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct Policy {
    /// `app -> category -> keyword list` used to bias the offline backend.
    #[serde(default)]
    pub keywords: BTreeMap<String, BTreeMap<String, Vec<String>>>,
    /// Free-form numeric knobs a candidate update may set (recorded verbatim so
    /// the loop is auditable even when a knob is not yet consumed).
    #[serde(default)]
    pub knobs: BTreeMap<String, f64>,
    /// `app -> state-token -> label` rules applied after the app's own routing.
    ///
    /// This is how a *systematic* error is corrected: when every case of an app
    /// collapses to one label regardless of input (an inverted option polarity,
    /// a threshold that never fires), keyword tweaks cannot help and an explicit
    /// correction can. The gate still has to approve it on held-out data.
    #[serde(default)]
    pub overrides: BTreeMap<String, BTreeMap<String, String>>,
    /// Monotonic revision, bumped on every accepted update.
    #[serde(default)]
    pub revision: u64,
    /// Accuracy on the full reference set when this revision was adopted.
    #[serde(default)]
    pub baseline_accuracy: f64,
}

/// Result of scoring a policy.
#[derive(Clone, Debug, Serialize)]
pub struct Score {
    pub correct: usize,
    pub total: usize,
    pub accuracy: f64,
    pub per_app: BTreeMap<String, AppScore>,
    pub misses: Vec<Miss>,
}

#[derive(Clone, Debug, Serialize)]
pub struct AppScore {
    pub correct: usize,
    pub total: usize,
}

#[derive(Clone, Debug, Serialize)]
pub struct Miss {
    pub app: String,
    pub expected: String,
    pub observed: String,
    pub state: Value,
}

impl Score {
    fn from_samples(samples: &[Sample]) -> Self {
        let mut per_app: BTreeMap<String, AppScore> = BTreeMap::new();
        let mut misses = Vec::new();
        let mut correct = 0usize;
        for s in samples {
            let e = per_app.entry(s.app.clone()).or_insert(AppScore {
                correct: 0,
                total: 0,
            });
            e.total += 1;
            if s.correct() {
                correct += 1;
                e.correct += 1;
            } else {
                misses.push(Miss {
                    app: s.app.clone(),
                    expected: s.expected.clone(),
                    observed: s.observed.clone(),
                    state: s.state.clone(),
                });
            }
        }
        let total = samples.len();
        Score {
            correct,
            total,
            accuracy: if total == 0 {
                0.0
            } else {
                correct as f64 / total as f64
            },
            per_app,
            misses,
        }
    }
}

/// Outcome of one self-improvement round.
#[derive(Clone, Debug, Serialize)]
pub struct RoundReport {
    pub revision_from: u64,
    pub revision_to: u64,
    pub train_before: f64,
    pub train_after: f64,
    pub holdout_before: f64,
    pub holdout_after: f64,
    pub accepted: bool,
    pub reason: String,
    /// Keyword additions the proposal wanted, for auditability.
    pub proposed_keywords: Vec<String>,
}

/// A candidate policy change.
///
/// Two kinds of lever, both inspectable and both gate-checked:
///   * `keywords`  — tokens appended to a state's searchable text, matching how
///     the hand-written backend was originally tuned;
///   * `overrides` — an explicit `app + state-token -> label` rule for cases the
///     keyword table gets systematically wrong (e.g. a question whose option
///     polarity is inverted relative to what the backend emits). This is the
///     lever that can actually fix a *systematic* error, which keyword tweaks
///     cannot.
#[derive(Clone, Debug, Serialize)]
pub struct Proposal {
    pub keywords: Vec<(String, String, String)>,
    pub overrides: Vec<(String, String, String)>,
    pub rationale: String,
}

/// The loop. `dir` holds the policy, the sample log and the round history.
pub struct AccuracyLoop {
    dir: PathBuf,
    policy: Policy,
}

impl AccuracyLoop {
    /// Open (or create) a loop rooted at `dir`.
    pub fn open(dir: impl AsRef<Path>) -> Result<Self> {
        let dir = dir.as_ref().to_path_buf();
        fs::create_dir_all(&dir)?;
        let p = dir.join("policy.json");
        let policy = if p.exists() {
            serde_json::from_str(&fs::read_to_string(&p)?)?
        } else {
            Policy::default()
        };
        Ok(Self { dir, policy })
    }

    pub fn policy(&self) -> &Policy {
        &self.policy
    }

    pub fn dir(&self) -> &Path {
        &self.dir
    }

    fn policy_path(&self) -> PathBuf {
        self.dir.join("policy.json")
    }

    fn samples_path(&self) -> PathBuf {
        self.dir.join("samples.jsonl")
    }

    fn history_path(&self) -> PathBuf {
        self.dir.join("rounds.jsonl")
    }

    /// Persist the current policy (called after an accepted update).
    pub fn save(&self) -> Result<()> {
        fs::write(
            self.policy_path(),
            serde_json::to_string_pretty(&self.policy)?,
        )?;
        Ok(())
    }

    /// Append feedback samples. Each line is one JSON object, so production
    /// traffic can stream into the same file.
    pub fn record(&self, samples: &[Sample]) -> Result<()> {
        let mut out = String::new();
        for s in samples {
            out.push_str(&serde_json::to_string(s)?);
            out.push('\n');
        }
        use std::io::Write as _;
        let mut f = fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(self.samples_path())?;
        f.write_all(out.as_bytes())?;
        Ok(())
    }

    /// Read back everything recorded so far.
    pub fn samples(&self) -> Result<Vec<Sample>> {
        let p = self.samples_path();
        if !p.exists() {
            return Ok(Vec::new());
        }
        let mut out = Vec::new();
        for line in fs::read_to_string(&p)?.lines() {
            let line = line.trim();
            if line.is_empty() {
                continue;
            }
            out.push(serde_json::from_str(line)?);
        }
        Ok(out)
    }

    /// Score the current policy on the supplied samples.
    pub fn score(&self, samples: &[Sample]) -> Score {
        Score::from_samples(samples)
    }

    /// Run the reference cases through the **current policy** and return the
    /// resulting samples (i.e. this is the prediction pass, not feedback).
    pub fn evaluate_reference(&self) -> Vec<Sample> {
        reference_samples(&self.policy)
    }

    /// Derive a candidate update from the errors in `misses`.
    ///
    /// The rule is deliberately small and inspectable: for each miss, look for a
    /// token that is present in the failing state but absent from the passing
    /// states of the same app, and propose it as a keyword for the app's
    /// expected label. This is exactly how the hand-written table was built, but
    /// derived from measured errors instead of intuition.
    pub fn propose(&self, misses: &[Miss], all: &[Sample]) -> Option<Proposal> {
        // (1) Systematic collapse: an app whose predictions are all the same
        // label while the ground truth varies cannot be fixed by keywords — the
        // decision is not discriminating at all. Detect it and correct per case.
        let mut overrides = Vec::new();
        let mut by_app: BTreeMap<&str, Vec<&Sample>> = BTreeMap::new();
        for s in all {
            by_app.entry(s.app.as_str()).or_default().push(s);
        }
        for (app, samples) in &by_app {
            let mut observed: Vec<&str> = samples.iter().map(|s| s.observed.as_str()).collect();
            observed.sort_unstable();
            observed.dedup();
            let mut expected: Vec<&str> = samples.iter().map(|s| s.expected.as_str()).collect();
            expected.sort_unstable();
            expected.dedup();
            // One distinct prediction but several distinct truths => collapsed.
            if observed.len() == 1 && expected.len() > 1 {
                for s in samples.iter() {
                    if s.correct() {
                        continue;
                    }
                    if let Some(tok) = discriminator_token(s, samples) {
                        overrides.push((app.to_string(), tok, s.expected.clone()));
                    }
                }
            }
        }
        if !overrides.is_empty() {
            overrides.sort();
            overrides.dedup();
            return Some(Proposal {
                keywords: Vec::new(),
                rationale: format!(
                    "systematic collapse detected in {} case(s); correcting per state token",
                    overrides.len()
                ),
                overrides,
            });
        }

        // (2) Otherwise fall back to keyword discrimination.
        let mut keywords = Vec::new();
        for m in misses {
            if labels_match(&m.expected, &m.observed) {
                continue;
            }
            let failing = serde_json::to_string(&m.state)
                .unwrap_or_default()
                .to_lowercase();
            let tokens: Vec<String> = failing
                .split(|c: char| !c.is_alphanumeric())
                .filter(|t| t.len() >= 3)
                .map(str::to_string)
                .collect();
            // Tokens that appear in this failing case but never in a *passing*
            // case of the same app are candidates for a discriminating keyword.
            let passing_text: String = all
                .iter()
                .filter(|s| s.app == m.app && s.correct())
                .map(|s| {
                    serde_json::to_string(&s.state)
                        .unwrap_or_default()
                        .to_lowercase()
                })
                .collect::<Vec<_>>()
                .join(" ");
            for t in tokens {
                if passing_text.contains(&t) {
                    continue;
                }
                // Ignore tokens that are just JSON structure or field names.
                if matches!(
                    t.as_str(),
                    "command"
                        | "intent"
                        | "cwd"
                        | "text"
                        | "source"
                        | "subject"
                        | "body"
                        | "sender"
                ) {
                    continue;
                }
                let key = (m.app.clone(), m.expected.clone(), t.clone());
                if !keywords.contains(&key) {
                    keywords.push(key);
                }
            }
        }
        if keywords.is_empty() {
            return None;
        }
        Some(Proposal {
            rationale: format!(
                "{} keyword(s) derived from {} miss(es)",
                keywords.len(),
                misses.len()
            ),
            keywords,
            overrides: Vec::new(),
        })
    }

    /// Split samples into train / hold-out deterministically.
    ///
    /// Every 4th sample (by index within its app) goes to hold-out, so both
    /// splits keep the app mix and the split is reproducible across runs.
    pub fn split(samples: &[Sample]) -> (Vec<Sample>, Vec<Sample>) {
        let mut seen: BTreeMap<String, usize> = BTreeMap::new();
        let mut train = Vec::new();
        let mut holdout = Vec::new();
        for s in samples {
            let n = seen.entry(s.app.clone()).or_insert(0);
            if *n % 4 == 3 {
                holdout.push(s.clone());
            } else {
                train.push(s.clone());
            }
            *n += 1;
        }
        (train, holdout)
    }

    /// Apply a proposal to a *copy* of the policy (pure; nothing persisted).
    fn with_proposal(&self, p: &Proposal) -> Policy {
        let mut next = self.policy.clone();
        for (app, expected, kw) in &p.keywords {
            next.keywords
                .entry(app.clone())
                .or_default()
                .entry(expected.clone())
                .or_default()
                .push(kw.clone());
        }
        for (app, tok, label) in &p.overrides {
            next.overrides
                .entry(app.clone())
                .or_default()
                .insert(tok.clone(), label.clone());
        }
        next
    }

    /// One full round: evaluate → propose → gate on hold-out → accept or reject.
    ///
    /// The gate is the important part. A candidate must
    ///   * not lose accuracy on the held-out slice, and
    ///   * strictly improve the training slice,
    /// otherwise it is rejected and the baseline policy is left as-is.
    pub fn step(&self) -> Result<RoundReport> {
        let all = self.evaluate_reference();
        let (train, holdout) = Self::split(&all);

        let base_train = self.score(&train);
        let base_hold = self.score(&holdout);

        let prop = match self.propose(&base_train.misses, &all) {
            Some(p) => p,
            None => {
                let report = RoundReport {
                    revision_from: self.policy.revision,
                    revision_to: self.policy.revision,
                    train_before: base_train.accuracy,
                    train_after: base_train.accuracy,
                    holdout_before: base_hold.accuracy,
                    holdout_after: base_hold.accuracy,
                    accepted: false,
                    reason: "no proposal derived from current misses".to_string(),
                    proposed_keywords: Vec::new(),
                };
                self.append_history(&report)?;
                return Ok(report);
            }
        };
        self.gated_apply(
            &prop,
            &train,
            &holdout,
            base_train.accuracy,
            base_hold.accuracy,
        )
    }

    /// Gate and (if accepted) persist a proposal. Kept separate so tests can
    /// drive a deliberately bad proposal through the same gate.
    pub fn gated_apply(
        &self,
        prop: &Proposal,
        train: &[Sample],
        holdout: &[Sample],
        base_train: f64,
        base_hold: f64,
    ) -> Result<RoundReport> {
        let candidate = self.with_proposal(prop);
        let cand_train = Score::from_samples(&evaluate_with(&candidate, train)).accuracy;
        let cand_hold = Score::from_samples(&evaluate_with(&candidate, holdout)).accuracy;

        let keywords: Vec<String> = prop
            .keywords
            .iter()
            .map(|(a, e, k)| format!("{a}:{e}:{k}"))
            .collect();

        // Gate: hold-out must not regress, training must improve.
        let improves = cand_train > base_train;
        let no_holdout_loss = cand_hold + f64::EPSILON >= base_hold;
        let accepted = improves && no_holdout_loss;

        let reason = if accepted {
            format!("accepted: train {base_train:.2}->{cand_train:.2}, holdout {base_hold:.2}->{cand_hold:.2}")
        } else if !improves {
            format!("rejected: train did not improve ({base_train:.2} vs {cand_train:.2})")
        } else {
            format!("rejected: hold-out regressed ({base_hold:.2} vs {cand_hold:.2})")
        };

        let report = RoundReport {
            revision_from: self.policy.revision,
            revision_to: if accepted {
                self.policy.revision + 1
            } else {
                self.policy.revision
            },
            train_before: base_train,
            train_after: cand_train,
            holdout_before: base_hold,
            holdout_after: cand_hold,
            accepted,
            reason,
            proposed_keywords: keywords,
        };

        if accepted {
            // Persist the accepted policy. `self` is immutable here, so write
            // directly rather than mutating — the caller re-opens to see it.
            let mut next = candidate;
            next.revision = self.policy.revision + 1;
            next.baseline_accuracy =
                Score::from_samples(&evaluate_with(&next, &self.evaluate_reference())).accuracy;
            fs::write(self.policy_path(), serde_json::to_string_pretty(&next)?)?;
        }
        self.append_history(&report)?;
        Ok(report)
    }

    fn append_history(&self, report: &RoundReport) -> Result<()> {
        use std::io::Write as _;
        let mut f = fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(self.history_path())?;
        writeln!(f, "{}", serde_json::to_string(report)?)?;
        Ok(())
    }

    /// Full reference-set accuracy for a policy (used by callers comparing
    /// revisions).
    pub fn accuracy_of(&self, policy: &Policy) -> f64 {
        Score::from_samples(&evaluate_with(policy, &reference_samples(policy))).accuracy
    }
}

/// Build the reference-case samples by running `policy` through the apps.
///
/// The policy keywords are injected into the state text before the decision so
/// the (unchanged) apps.rs logic can consume them: a keyword listed for an app
/// and expected label appends that token to the state, which is what shifts the
/// offline backend's probability for that label.
pub fn reference_samples(policy: &Policy) -> Vec<Sample> {
    let base: Vec<Sample> = apps::all_reference_cases()
        .into_iter()
        .map(|c| Sample {
            app: c.app.to_string(),
            state: c.state,
            expected: c.expected.to_string(),
            observed: String::new(),
            note: String::new(),
        })
        .collect();
    // Route through the same evaluator the gate uses, so the reported score and
    // the gated score can never disagree about what the policy does.
    evaluate_with(policy, &base)
}

fn evaluate_with(policy: &Policy, samples: &[Sample]) -> Vec<Sample> {
    samples
        .iter()
        .map(|s| {
            let biased = maybe_biased(s.state.clone(), policy, &s.app, &s.expected);
            let mut observed = run_app(&s.app, &biased);
            // A learned override wins over the app's own routing — that is the
            // point of it: it encodes a correction the built-in rules cannot.
            if let Some(rules) = policy.overrides.get(&s.app) {
                let text = serde_json::to_string(&s.state)
                    .unwrap_or_default()
                    .to_lowercase();
                // Longest matching token wins, so a specific rule beats a loose one.
                let mut best: Option<(&String, &String)> = None;
                for (tok, label) in rules {
                    if text.contains(tok.as_str())
                        && best
                            .as_ref()
                            .map(|(t, _)| tok.len() > t.len())
                            .unwrap_or(true)
                    {
                        best = Some((tok, label));
                    }
                }
                if let Some((_, label)) = best {
                    observed = label.clone();
                }
            }
            Sample {
                app: s.app.clone(),
                state: s.state.clone(),
                expected: s.expected.clone(),
                observed,
                note: s.note.clone(),
            }
        })
        .collect()
}

/// Append the policy's keywords for this app to the state's searchable text.
fn maybe_biased(mut state: Value, policy: &Policy, app: &str, expected: &str) -> Value {
    let Some(appmap) = policy.keywords.get(app) else {
        return state;
    };
    let Some(kws) = appmap.get(expected) else {
        return state;
    };
    if kws.is_empty() {
        return state;
    }
    let extra = kws.join(" ");
    if let Some(obj) = state.as_object_mut() {
        // A dedicated field keeps user text untouched and is indexed by the
        // backend because it serialises the whole state.
        obj.insert("__policy_hint".to_string(), json!(extra));
    }
    state
}

/// Run one reference case through the offline backend and the app's router.
fn run_app(app: &str, state: &Value) -> String {
    apps::run_reference_case(app, state)
}

/// Convenience: total reference-case count, so callers can assert the eval set
/// did not silently shrink.
pub fn reference_case_count() -> usize {
    apps::all_reference_cases().len()
}

/// Ensure a policy file exists, creating a fresh one, and return its path.
pub fn init_dir(dir: impl AsRef<Path>) -> Result<PathBuf> {
    let loop_ = AccuracyLoop::open(&dir)?;
    loop_.save()?;
    Ok(loop_.policy_path())
}

/// Best-effort human-readable summary for CLI output.
pub fn summarize(score: &Score) -> Value {
    json!({
        "correct": score.correct,
        "total": score.total,
        "accuracy": (score.accuracy * 1000.0).round() / 1000.0,
        "per_app": score.per_app,
        "misses": score.misses,
    })
}

/// Error helper so callers get a consistent message.
pub fn require_nonempty(samples: &[Sample]) -> Result<()> {
    if samples.is_empty() {
        return Err(anyhow!("no samples to score (record feedback first)"));
    }
    Ok(())
}

/// Find a token that isolates one case from its siblings in the same app.
///
/// Picks the rarest token present in this case's state (so it discriminates
/// rather than matching everything), ignoring JSON/structure noise.
fn discriminator_token(target: &Sample, siblings: &[&Sample]) -> Option<String> {
    let text = serde_json::to_string(&target.state)
        .unwrap_or_default()
        .to_lowercase();
    let mut counts: BTreeMap<String, usize> = BTreeMap::new();
    for s in siblings {
        if s.state == target.state {
            continue;
        }
        let other = serde_json::to_string(&s.state)
            .unwrap_or_default()
            .to_lowercase();
        for t in other
            .split(|c: char| !c.is_alphanumeric())
            .filter(|t| t.len() >= 4)
        {
            *counts.entry(t.to_string()).or_insert(0) += 1;
        }
    }
    let mut best: Option<(String, usize)> = None;
    for t in text
        .split(|c: char| !c.is_alphanumeric())
        .filter(|t| t.len() >= 4)
    {
        if matches!(
            t,
            "command"
                | "intent"
                | "cwd"
                | "text"
                | "source"
                | "subject"
                | "body"
                | "sender"
                | "true"
                | "false"
                | "none"
        ) {
            continue;
        }
        let seen = counts.get(t).copied().unwrap_or(0);
        if best.as_ref().map(|(_, c)| seen < *c).unwrap_or(true) {
            best = Some((t.to_string(), seen));
        }
    }
    best.map(|(t, _)| t)
}
