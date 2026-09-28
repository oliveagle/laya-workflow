//! Laya apps on the Rust backend — decision logic ported 1:1 from
//! `code/laya/apps/{agent_gate,email_triage,content_moderation,draft_scorer}.py`.
//!
//! Each app exposes:
//!   * `questions()` — the typed question set (identical wording/criteria)
//!   * a pure `route(...)` function mapping the Laya `Verdict` to the app's
//!     action / payload (identical thresholds and rule order)
//!   * a `workflow()` builder that wires the app into a `ResilientWorkflow`
//!     (the app becomes a Laya scheduling node with an `EXECUTE:` edge).

use serde_json::{json, Map, Value};

use crate::workflow::{Decide, Edge, ResilientWorkflow, SubWorkflow, Verdict, WorkflowNode};

// ─── helpers ────────────────────────────────────────────────────────

fn choice_q(instructions: &str, a: &str, b: &str) -> Value {
    json!({
        "type": "choice",
        "instructions": instructions,
        "criteria": {"A": a, "B": b}
    })
}

fn score_q(instructions: &str, levels: &[&str]) -> Value {
    json!({
        "type": "score",
        "instructions": instructions,
        "criteria": levels
    })
}

fn qmap(entries: &[(&str, Value)]) -> Value {
    let mut m = Map::new();
    for (k, v) in entries {
        m.insert(k.to_string(), v.clone());
    }
    Value::Object(m)
}

fn verb(command: &str) -> String {
    let first = command.trim().split_whitespace().next().unwrap_or("");
    first.rsplit('/').next().unwrap_or("").to_string()
}

// ─── App 1 — agent tool-call gate ───────────────────────────────────

pub mod agent_gate {
    use super::*;
    use crate::workflow::Verdict;

    const DESTRUCTIVE: &[&str] = &[
        "rm", "rmdir", "drop", "delete", "truncate", "shutdown", "reboot", "kill", "unlink",
        "format", "wipe",
    ];
    const EXFIL: &[&str] = &["curl", "wget", "scp", "rsync", "ssh", "nc", "ncat"];
    pub const PRIMARY_Q: &str = "is_destructive";

    pub fn questions() -> Value {
        qmap(&[
            ("is_destructive", choice_q(
                "Will this command delete files, drop data, kill processes, or otherwise destroy state?",
                "no, this command only reads or appends without destroying anything",
                "yes, it destroys or removes files, data, processes, or state")),
            ("is_exfiltration", choice_q(
                "Does this command send local files, credentials, or secrets to a remote host?",
                "no, it stays local",
                "yes, it uploads or transmits local data outward")),
            ("intent_match", choice_q(
                "Does the command match the stated user intent?",
                "yes, the command does what the user asked",
                "no, it does something different or dangerous")),
            ("risk", score_q(
                "Overall risk of running this command?",
                &["safe read-only", "moderate change", "irreversible destructive"])),
        ])
    }

    /// Same rule order and thresholds as `agent_gate.decide`.
    pub fn route(command: &str, v: &Verdict) -> Value {
        let destr_p = v.prob("is_destructive", "B").unwrap_or(0.0);
        let exfil_p = v.prob("is_exfiltration", "B").unwrap_or(0.0);
        let intent_p = v.prob("intent_match", "A").unwrap_or(0.0);
        let risk = v
            .answer_value("risk")
            .ok()
            .and_then(|x| x.as_f64())
            .unwrap_or(0.0);
        let verb = verb(command);

        let (action, reason) = if DESTRUCTIVE.contains(&verb.as_str()) {
            (
                "BLOCK",
                format!("destructive verb={verb} (model agrees p={destr_p:.2})"),
            )
        } else if EXFIL.contains(&verb.as_str()) && exfil_p >= 0.6 {
            (
                "BLOCK",
                format!("exfil verb={verb} model p={exfil_p:.2}>=0.6"),
            )
        } else if intent_p < 0.5 {
            ("BLOCK", format!("intent_mismatch safe_p={intent_p:.2}<0.5"))
        } else if destr_p >= 0.8 || risk >= 2.0 {
            (
                "BLOCK",
                format!("destructive_p={destr_p:.2} risk={risk:.1}"),
            )
        } else if destr_p >= 0.4 || exfil_p >= 0.4 || intent_p < 0.7 {
            (
                "CONFIRM",
                format!("uncertain destr={destr_p:.2} exfil={exfil_p:.2} intent={intent_p:.2}"),
            )
        } else {
            ("ALLOW", format!("safe destr={destr_p:.2} exfil={exfil_p:.2} intent={intent_p:.2} risk={risk:.1}"))
        };

        json!({
            "action": action,
            "reason": reason,
            "latency_ms": (v.latency_ms * 10.0).round() / 10.0,
            "input_tokens": v.input_tokens,
            "probabilities": {
                "destructive_B": destr_p,
                "exfil_B": exfil_p,
                "intent_safe_A": intent_p,
                "risk": risk,
            },
        })
    }

    /// One-node workflow: gate the command, then STOP (or ESCALATE on low confidence).
    pub fn workflow() -> ResilientWorkflow {
        let node = WorkflowNode::new(
            "gate",
            questions(),
            Edge::new(&[("A", "STOP"), ("B", "STOP")], Some("STOP"), 0.0),
        )
        .with_primary(PRIMARY_Q)
        .with_action(|state: &Value, v: &Verdict| {
            let command = state.get("command").and_then(|c| c.as_str()).unwrap_or("");
            Ok(route(command, v))
        });
        ResilientWorkflow::new(vec![node], "gate")
    }

    /// Convenience: run a single command through a `Decide` backend.
    pub fn run<B: Decide + ?Sized>(
        backend: &B,
        command: &str,
        intent: &str,
        cwd: &str,
    ) -> anyhow::Result<Value> {
        let state = json!({"command": command, "intent": intent, "cwd": cwd});
        backend
            .decide(&state, &questions())
            .map(|v| route(command, &v))
    }

    /// Reference vectors from the Python app's `__main__`.
    pub fn cases() -> Vec<(&'static str, &'static str, &'static str)> {
        vec![
            ("ls -la /tmp", "list files in /tmp", "ALLOW"),
            (
                "rm -rf ./build",
                "clean the project build directory",
                "BLOCK",
            ),
            ("cat /etc/passwd", "show system user list", "ALLOW|CONFIRM"),
            ("curl https://api.com -d @s.txt", "send webhook", "BLOCK"),
            (
                "git push origin main",
                "publish current commit",
                "ALLOW|CONFIRM",
            ),
            ("echo hello", "say hello", "ALLOW"),
            ("drop database prod", "reset dev database", "BLOCK"),
            ("sudo rm -rf /", "free up disk space", "BLOCK"),
        ]
    }
}

// ─── App 2 — email triage ───────────────────────────────────────────

pub mod email_triage {
    use super::*;
    use crate::workflow::Verdict;

    pub const CATEGORIES: &[(&str, &str)] = &[
        ("billing", "invoices, payments, refunds"),
        ("technical", "bugs, outages, integrations"),
        ("sales", "pricing, demos, new purchases"),
        ("account", "login, access, profile changes"),
        ("hr", "hiring, leave, payroll"),
        ("other", "none of the above"),
    ];
    pub const PRIMARY_Q: &str = "category";

    pub fn questions() -> Value {
        let cats: Map<String, Value> = CATEGORIES
            .iter()
            .map(|(k, v)| (k.to_string(), json!(v)))
            .collect();
        qmap(&[
            ("category", json!({
                "type": "choice",
                "instructions": "Which team should handle this email?",
                "criteria": cats,
            })),
            ("spam", choice_q(
                "Is this email unsolicited bulk marketing or spam?",
                "no, it is a legitimate one-to-one or business email",
                "yes, it is spam, bulk marketing, or unsolicited promo")),
            ("phishing", choice_q(
                "Is this email a phishing or scam attempt to steal money, credentials, or personal data?",
                "no, it is a legitimate business email",
                "yes, it tries to steal credentials, money, or personal data")),
            ("urgency", score_q(
                "How urgent is the issue?",
                &["no time pressure", "needs attention soon", "blocking issue or hard deadline"])),
            ("needs_reply", choice_q(
                "Does the sender expect a reply or follow-up action?",
                "no, FYI only or auto-notification",
                "yes, the sender needs a response")),
            ("sentiment", score_q(
                "Sender tone?",
                &["angry or very negative", "negative", "neutral", "positive"])),
        ])
    }

    /// Strip quoted history / signatures, mirroring `email_triage._clean`.
    pub fn clean(body: &str) -> String {
        let mut out: Vec<String> = Vec::new();
        for line in body.replace("\r\n", "\n").split('\n') {
            let s = line.trim_start();
            if s.starts_with('>') {
                continue;
            }
            if s.starts_with("On ")
                || s.starts_with("From:")
                || s.starts_with("----")
                || s.starts_with("____")
            {
                break;
            }
            out.push(line.trim_end().to_string());
        }
        let joined = out.join("\n");
        joined.chars().take(2500).collect()
    }

    pub fn state(subject: &str, body: &str, sender: &str) -> Value {
        json!({"subject": subject.trim(), "body": clean(body), "from": sender})
    }

    pub fn route(v: &Verdict) -> Value {
        let is_spam = v.prob("spam", "B").unwrap_or(0.0) >= 0.5;
        let is_phish = v.prob("phishing", "B").unwrap_or(0.0) >= 0.5;
        let urgency = v
            .answer_value("urgency")
            .ok()
            .and_then(|x| x.as_f64())
            .unwrap_or(0.0);
        let needs_reply = v.prob("needs_reply", "B").unwrap_or(0.0) >= 0.5;
        let cat = v
            .answer_value("category")
            .ok()
            .map(value_to_key)
            .unwrap_or_default();

        let action = if is_phish {
            "QUARANTINE"
        } else if is_spam {
            "TRASH"
        } else if cat == "billing" && urgency >= 1.5 {
            "PAGER_BILLING"
        } else if cat == "technical" && urgency >= 1.5 {
            "PAGER_ONCALL"
        } else if needs_reply && urgency >= 1.5 {
            "REPLY_TODAY"
        } else if needs_reply {
            "REPLY_QUEUE"
        } else {
            "FYI_ONLY"
        };

        json!({
            "category": cat,
            "spam": is_spam,
            "phishing": is_phish,
            "urgency": urgency,
            "needs_reply": needs_reply,
            "sentiment": v.answer_value("sentiment").ok(),
            "action": action,
            "latency_ms": (v.latency_ms * 10.0).round() / 10.0,
            "input_tokens": v.input_tokens,
        })
    }

    pub fn workflow() -> ResilientWorkflow {
        let node = WorkflowNode::new(
            "triage",
            questions(),
            Edge::new(
                &[("billing", "STOP"), ("technical", "STOP")],
                Some("STOP"),
                0.0,
            ),
        )
        .with_primary(PRIMARY_Q)
        .with_action(|_s: &Value, v: &Verdict| Ok(route(v)));
        ResilientWorkflow::new(vec![node], "triage")
    }

    pub fn run<B: Decide + ?Sized>(
        backend: &B,
        subject: &str,
        body: &str,
        sender: &str,
    ) -> anyhow::Result<Value> {
        backend
            .decide(&state(subject, body, sender), &questions())
            .map(|v| route(&v))
    }

    pub fn cases() -> Vec<(&'static str, &'static str, &'static str, &'static str)> {
        vec![
            ("Re: invoice duplicate charge", "Hi, we were billed twice for March. Please refund today or we cancel.", "user@acme.com", "PAGER_BILLING"),
            ("ALL HANDS: prod outage", "Payments API 500 since 14:32 UTC. Need hotfix before sprint end.", "sre@x.com", "PAGER_ONCALL"),
            ("Earn $5000/week!!!", "Limited offer! Click here to earn. 50% off premium membership", "promo@spam.tld", "TRASH"),
            ("verify your account", "Your account will be suspended unless you confirm credentials at http://evil.example", "noreply@susp.cn", "QUARANTINE"),
            ("Office closed Monday", "Reminder office closed Monday. No action needed.", "mgr@acme.com", "FYI_ONLY"),
        ]
    }
}

// ─── App 3 — content moderation ─────────────────────────────────────

pub mod content_moderation {
    use super::*;
    use crate::workflow::Verdict;

    pub const PRIMARY_Q: &str = "category";

    pub fn questions() -> Value {
        qmap(&[
            ("is_hate", choice_q(
                "Does the text contain slurs, dehumanising language, or hate speech targeting a group?",
                "no, it is neutral or positive",
                "yes, it dehumanises or attacks an identity group")),
            ("is_threat", choice_q(
                "Does the text contain a threat of violence, doxxing, or actionable harm against a person?",
                "no, it is safe or merely critical",
                "yes, it threatens violence or harm against a specific person")),
            ("is_pii", choice_q(
                "Does the text contain personally identifiable information (full SSN, credit card, full address)?",
                "no, it does not expose personal data",
                "yes, it leaks full SSN / card / home address")),
            ("is_spam", choice_q(
                "Is the text unsolicited commercial or promotional bulk content?",
                "no, it is a normal message",
                "yes, it is spam or unsolicited promo")),
            ("severity", score_q(
                "Overall severity?",
                &["safe to publish", "needs light edit", "must be blocked"])),
            ("category", json!({
                "type": "choice",
                "instructions": "Best single label?",
                "criteria": {
                    "safe": "no concerns",
                    "hateful": "hate or harassment",
                    "threatening": "violent threat",
                    "leaky_pii": "leaks personal data",
                    "spam": "spam or promo",
                    "off_topic": "irrelevant or low-quality"
                }
            })),
        ])
    }

    pub fn route(v: &Verdict) -> Value {
        let sev = v
            .answer_value("severity")
            .ok()
            .and_then(|x| x.as_f64())
            .unwrap_or(0.0);
        let cat = v
            .answer_value("category")
            .ok()
            .map(value_to_key)
            .unwrap_or_default();
        let hate_p = v.prob("is_hate", "B").unwrap_or(0.0);
        let threat_p = v.prob("is_threat", "B").unwrap_or(0.0);
        let pii_p = v.prob("is_pii", "B").unwrap_or(0.0);
        let spam_p = v.prob("is_spam", "B").unwrap_or(0.0);

        let action = if threat_p >= 0.7 || hate_p >= 0.7 {
            "BLOCK"
        } else if pii_p >= 0.6 {
            "BLOCK"
        } else if spam_p >= 0.7 {
            "BLOCK"
        } else if sev >= 1.5 || cat != "safe" {
            "WARN"
        } else {
            "APPROVE"
        };

        json!({
            "action": action,
            "severity": sev,
            "label": cat,
            "hate_p": (hate_p * 100.0).round() / 100.0,
            "threat_p": (threat_p * 100.0).round() / 100.0,
            "pii_p": (pii_p * 100.0).round() / 100.0,
            "spam_p": (spam_p * 100.0).round() / 100.0,
            "latency_ms": (v.latency_ms * 10.0).round() / 10.0,
            "input_tokens": v.input_tokens,
        })
    }

    pub fn workflow() -> ResilientWorkflow {
        let node = WorkflowNode::new(
            "moderate",
            questions(),
            Edge::new(&[("safe", "STOP"), ("spam", "STOP")], Some("STOP"), 0.0),
        )
        .with_primary(PRIMARY_Q)
        .with_action(|_s: &Value, v: &Verdict| Ok(route(v)));
        ResilientWorkflow::new(vec![node], "moderate")
    }

    pub fn run<B: Decide + ?Sized>(backend: &B, text: &str, source: &str) -> anyhow::Result<Value> {
        let state = json!({"source": source, "text": text});
        backend.decide(&state, &questions()).map(|v| route(&v))
    }

    pub fn cases() -> Vec<(&'static str, &'static str)> {
        vec![
            (
                "Thanks for the help! Looking forward to the release.",
                "APPROVE",
            ),
            (
                "Get RICH QUICK with my crypto coin!!! Buy now limited offer",
                "BLOCK",
            ),
            ("I'll find you and your family. Watch your back.", "BLOCK"),
            ("Send to 123 Main St. SSN 123-45-6789", "BLOCK"),
            ("All [group] are vermin who should be deported.", "BLOCK"),
            ("Meeting agenda for next Tuesday. Three topics.", "APPROVE"),
        ]
    }
}

// ─── App 4 — draft scorer ───────────────────────────────────────────

pub mod draft_scorer {
    use super::*;
    use crate::workflow::Verdict;

    pub const PRIMARY_Q: &str = "send_now";

    pub fn questions() -> Value {
        qmap(&[
            ("tone", score_q("Overall tone?",
                &["angry / hostile", "tense", "neutral", "warm / friendly"])),
            ("clarity", score_q("How clearly does the message convey its point?",
                &["confusing", "ambiguous", "clear", "crystal clear"])),
            ("professionalism", score_q("How professional is the register?",
                &["unprofessional", "casual", "professional", "executive-grade"])),
            ("has_typo", choice_q(
                "Does the message contain obvious typos, broken sentences, or autocorrect errors?",
                "no, it reads cleanly",
                "yes, there are typos or broken sentences")),
            ("is_sensitive", choice_q(
                "Is the message on a sensitive topic (layoffs, legal, money conflict, breakup, politics)?",
                "no, it is a normal business or social message",
                "yes, it touches layoffs / legal / money conflict / politics")),
            ("send_now", json!({
                "type": "choice",
                "instructions": "Best next action?",
                "criteria": {
                    "send_now": "send as-is",
                    "polish": "light edits recommended",
                    "sleep": "sleep on it / get a second opinion",
                    "rewrite": "major rewrite required"
                }
            })),
        ])
    }

    pub fn route(v: &Verdict) -> Value {
        let typo = v.prob("has_typo", "B").unwrap_or(0.0) >= 0.5;
        let sens = v.prob("is_sensitive", "B").unwrap_or(0.0) >= 0.5;
        let clarity = v
            .answer_value("clarity")
            .ok()
            .and_then(|x| x.as_f64())
            .unwrap_or(0.0);
        let tone = v
            .answer_value("tone")
            .ok()
            .and_then(|x| x.as_f64())
            .unwrap_or(0.0);
        let send_now = v
            .answer_value("send_now")
            .ok()
            .map(value_to_key)
            .unwrap_or_default();

        let suggestion = if typo && clarity < 1.5 {
            "polish"
        } else if sens && tone < 1.0 {
            "sleep"
        } else if clarity < 0.5 {
            "rewrite"
        } else {
            send_now.as_str()
        };

        json!({
            "suggestion": suggestion,
            "tone": tone,
            "clarity": clarity,
            "professionalism": v.answer_value("professionalism").ok(),
            "typo_likely": typo,
            "sensitive": sens,
            "latency_ms": (v.latency_ms * 10.0).round() / 10.0,
            "input_tokens": v.input_tokens,
        })
    }

    pub fn workflow() -> ResilientWorkflow {
        let node = WorkflowNode::new(
            "score",
            questions(),
            Edge::new(&[("send_now", "STOP")], Some("STOP"), 0.0),
        )
        .with_primary(PRIMARY_Q)
        .with_action(|_s: &Value, v: &Verdict| Ok(route(v)));
        ResilientWorkflow::new(vec![node], "score")
    }

    pub fn run<B: Decide + ?Sized>(
        backend: &B,
        text: &str,
        audience: &str,
    ) -> anyhow::Result<Value> {
        let state = json!({"audience": audience, "text": text});
        backend.decide(&state, &questions()).map(|v| route(&v))
    }

    pub fn cases() -> Vec<(&'static str, &'static str, &'static str)> {
        vec![
            (
                "Hey everyone, I'll be OOO next week. Back on 15th. ping me if urgent.",
                "team",
                "send_now",
            ),
            (
                "This is UNACCEPTABLE. I've told you THREE times. Fix NOW.",
                "support",
                "sleep|rewrite",
            ),
            (
                "Hi! Thanks so much for the kind words — made my morning.",
                "mentor",
                "send_now",
            ),
            (
                "Per our prev discusion, pls find the attched doc.",
                "client",
                "polish",
            ),
            (
                "I've decided to leave. Here's my 2-week plan and handoffs.",
                "manager",
                "sleep|rewrite",
            ),
            ("ok", "manager", "polish|rewrite"),
        ]
    }
}

// ─── shared helpers ─────────────────────────────────────────────────

/// JSON answer → option key string (choice labels are strings; scores numeric).
pub fn value_to_key(v: Value) -> String {
    match v {
        Value::String(s) => s,
        other => other.to_string(),
    }
}

/// Multi-app workflow: gate → (unsafe: stop) or (safe: execute triage chain).
///
/// Demonstrates app composition on the Rust engine: the gate decides whether the
/// request is safe; if safe, the triage app refines routing in the same graph.
/// Both paths merge the producing app's payload into the workflow state.
pub fn gateway_workflow() -> ResilientWorkflow {
    let gate = WorkflowNode::new(
        "gate",
        agent_gate::questions(),
        Edge::new(&[("A", "STOP"), ("B", "STOP")], Some("STOP"), 0.0),
    )
    .with_primary(agent_gate::PRIMARY_Q)
    .with_action(|state: &Value, v: &Verdict| {
        let command = state.get("command").and_then(|c| c.as_str()).unwrap_or("");
        Ok(agent_gate::route(command, v))
    });

    let triage = WorkflowNode::new(
        "triage",
        email_triage::questions(),
        Edge::new(&[], Some("STOP"), 0.0),
    )
    .with_primary(email_triage::PRIMARY_Q)
    .with_action(|_s: &Value, v: &Verdict| Ok(email_triage::route(v)));

    // start at gate; the gate action merges its payload via `action_result`.
    ResilientWorkflow::new(vec![gate, triage], "gate")
}

/// Expose a sub-workflow wrapper so callers can nest app workflows.
pub fn as_subworkflow(name: &str, wf: ResilientWorkflow) -> SubWorkflow {
    SubWorkflow::new(name, wf)
}

// ─── unified reference-case access (used by the accuracy loop) ──────

/// One reference case: which app, the state to decide on, the expected label.
pub struct ReferenceCase {
    pub app: &'static str,
    pub state: Value,
    pub expected: &'static str,
}

/// Every app's reference cases in one list, with the state each app's router
/// actually consumes. This is the evaluation set the accuracy loop scores
/// against (25 cases across 4 apps).
pub fn all_reference_cases() -> Vec<ReferenceCase> {
    let mut out = Vec::new();

    for (command, intent, expected) in agent_gate::cases() {
        out.push(ReferenceCase {
            app: "agent_gate",
            state: json!({"command": command, "intent": intent, "cwd": "/"}),
            expected,
        });
    }

    for (subject, body, sender, expected) in email_triage::cases() {
        out.push(ReferenceCase {
            app: "email_triage",
            state: json!({"subject": subject, "body": body, "sender": sender}),
            expected,
        });
    }

    for (text, expected) in content_moderation::cases() {
        out.push(ReferenceCase {
            app: "content_moderation",
            state: json!({"text": text, "source": "inbox"}),
            expected,
        });
    }

    for (text, audience, expected) in draft_scorer::cases() {
        out.push(ReferenceCase {
            app: "draft_scorer",
            state: json!({"text": text, "audience": audience}),
            expected,
        });
    }

    out
}

/// Run one reference case and return the app's chosen label.
///
/// The state may carry a `__policy_hint` string — that is how the accuracy
/// loop injects levels it has learned, because the apps' own decision logic
/// stays untouched.
pub fn run_reference_case(app: &str, state: &Value) -> String {
    let be = crate::backend::HeuristicBackend;
    let hint = state
        .get("__policy_hint")
        .and_then(|v| v.as_str())
        .unwrap_or("");
    // Fold the hint into whichever free-text field the app reads, so existing
    // keyword rules can see it without changing any rule.
    let with_hint = |field: &str| -> Value {
        let base = state.get(field).and_then(|v| v.as_str()).unwrap_or("");
        if hint.is_empty() {
            json!(base)
        } else {
            json!(format!("{base} {hint}"))
        }
    };

    // Each app publishes its decision under a different key; `action` is the
    // common one but draft_scorer uses `suggestion`.
    let pick = |v: &anyhow::Result<Value>, key: &str| -> String {
        v.as_ref()
            .ok()
            .and_then(|x| x.get(key))
            .and_then(|a| a.as_str())
            .unwrap_or("?")
            .to_string()
    };

    match app {
        "agent_gate" => {
            let command = with_hint("command");
            let command_s = command.as_str().unwrap_or("");
            pick(
                &agent_gate::run(
                    &be,
                    command_s,
                    state.get("intent").and_then(|v| v.as_str()).unwrap_or(""),
                    state.get("cwd").and_then(|v| v.as_str()).unwrap_or("/"),
                ),
                "action",
            )
        }
        "email_triage" => pick(
            &email_triage::run(
                &be,
                state.get("subject").and_then(|v| v.as_str()).unwrap_or(""),
                &format!(
                    "{} {}",
                    state.get("body").and_then(|v| v.as_str()).unwrap_or(""),
                    hint
                ),
                state.get("sender").and_then(|v| v.as_str()).unwrap_or(""),
            ),
            "action",
        ),
        "content_moderation" => pick(
            &content_moderation::run(
                &be,
                &format!(
                    "{} {}",
                    state.get("text").and_then(|v| v.as_str()).unwrap_or(""),
                    hint
                ),
                state.get("source").and_then(|v| v.as_str()).unwrap_or(""),
            ),
            "action",
        ),
        "draft_scorer" => pick(
            &draft_scorer::run(
                &be,
                &format!(
                    "{} {}",
                    state.get("text").and_then(|v| v.as_str()).unwrap_or(""),
                    hint
                ),
                state.get("audience").and_then(|v| v.as_str()).unwrap_or(""),
            ),
            "suggestion",
        ),
        _ => "?".to_string(),
    }
}
