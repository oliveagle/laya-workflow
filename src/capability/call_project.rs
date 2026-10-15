//! P178 - the `project` half of a `call` step: a row the engine can only
//! silently discard.
//!
//! A `call` action writes state by reading each `project` row's pointer with
//! `as_str()` and resolving it against the call result:
//!
//! ```jsonc
//! { "kind": "call", "capability": "clock",
//!   "project": { "now": "/epoch" },
//!   "chain": [ { "as": "extra", "capability": "clock",
//!                "project": { "base": "/offset_secs" } } ] }
//! ```
//!
//! A pointer of any other JSON type (`{"count": 3}`, `{"ok": true}`) is skipped
//! by `as_str()` and the state key is **never written** - and because the
//! compiler counts the row as a producer, an `@outputs` naming it stays green
//! while the run cannot keep the promise. A projection key that is empty writes
//! a state key the `${state.<key>}` grammar (one character or more) can never
//! name, and a `chain` row explicitly keyed by an empty alias files the
//! sub-call's result under a name no `chain.<alias>.*` row can reach. None of
//! the three can do what it says, so each is refused rather than shipped.
//!
//! P165 refused all three, but only on the `@decision` (BDD) compile path: a
//! hand-written / side-loaded JSON spec passed with `"project": {"count": 3}`
//! intact, because the check lived in `bdd.rs` alone. The decision lives here
//! once, so the BDD guard and the `spec.rs` guard hold both authors to the same
//! contract and cannot drift apart.

use serde_json::{Map, Value};

use crate::capability::with_keys::json_type_name;

/// The first `project` problem a `call` step has, with everything its message
/// needs. [`project_issue_body`] renders it; the caller prefixes `line N:`
/// (BDD) or `node <name>:` (`validate`).
pub enum ProjectIssue {
    /// A projection key that is empty (P165).
    EmptyKey { row: String },
    /// A pointer whose JSON type the read site skips (P165).
    NotString {
        row: String,
        key: String,
        got: &'static str,
    },
    /// A `chain` row explicitly keyed by an empty alias (P165).
    EmptyAlias { cap: String },
}

/// The first `project` problem a call step has, in the order the guard reports
/// them: the action's own `project` rows, then each `chain` entry (an empty
/// alias, then its `project` rows). `None` when the step is clean.
///
/// An absent - or non-string - `as` is legal: the engine falls back to the
/// step's capability name, so only an *explicitly* empty `as` files the result
/// under a name nothing can reach. The BDD compile path always writes a string
/// `as`, so it sees the same behaviour it always did.
pub fn project_step_issue(project: &Map<String, Value>, chain: &[Value]) -> Option<ProjectIssue> {
    let check = |row: String, key: &str, val: &Value| -> Option<ProjectIssue> {
        if key.is_empty() {
            return Some(ProjectIssue::EmptyKey { row });
        }
        if !val.is_string() {
            return Some(ProjectIssue::NotString {
                row,
                key: key.to_string(),
                got: json_type_name(val),
            });
        }
        None
    };
    for (k, v) in project {
        if let Some(issue) = check(format!("project.{k}"), k, v) {
            return Some(issue);
        }
    }
    for e in chain {
        let as_v = e.get("as");
        if matches!(as_v, Some(Value::String(s)) if s.is_empty()) {
            let cap = e.get("capability").and_then(Value::as_str).unwrap_or("");
            return Some(ProjectIssue::EmptyAlias {
                cap: cap.to_string(),
            });
        }
        let as_name = as_v.and_then(Value::as_str).unwrap_or("");
        let effective = if as_name.is_empty() {
            e.get("capability").and_then(Value::as_str).unwrap_or("")
        } else {
            as_name
        };
        if let Some(cp) = e.get("project").and_then(Value::as_object) {
            for (k, v) in cp {
                if let Some(issue) = check(format!("chain.{effective}.project.{k}"), k, v) {
                    return Some(issue);
                }
            }
        }
    }
    None
}

/// Render a [`ProjectIssue`] as the message body the guards print (the caller
/// adds the `line N:` / `node <name>:` locator).
pub fn project_issue_body(issue: &ProjectIssue) -> String {
    match issue {
        ProjectIssue::EmptyKey { row } => format!(
            "`{row}` has an empty name - the engine files the value under the empty key, which \
             no `${{state.<key>}}` reference can name, so the row can never do what it says. \
             Give it a name."
        ),
        ProjectIssue::NotString { row, key, got } => format!(
            "`{row}` is {got}, not a JSON string - the engine reads a projection with `as_str()` \
             and skips any other type, so `state.{key}` is never written and any `@outputs` \
             naming it is silently broken. Write the pointer as a string (e.g. `/count`)."
        ),
        ProjectIssue::EmptyAlias { cap } => format!(
            "a `chain` row is keyed by an empty alias (`| chain..… |`) - the engine files the \
             {cap:?} step's result under the empty name, which no `${{with.<alias>}}` template \
             and no `chain.<alias>.*` row can ever address. Give the sub-call an alias (e.g. \
             `| chain.<alias>.capability | {cap} |`)."
        ),
    }
}
