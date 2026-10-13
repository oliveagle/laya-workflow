//! Semantic graph validation regression tests (`validate_graph_semantics`).
//! These specs are structurally parseable but semantically broken; the engine
//! used to only fail (or silently misroute) at *runtime*. Now `spec::from_spec`
//! must reject them up front.
#![allow(unused_imports)]
use crate::*;

fn base_spec(start: &str, nodes: Value) -> Value {
    json!({
        "name": "validate_case",
        "start": start,
        "max_iterations": 7,
        "convergence_window": 3,
        "convergence_eps": 0.001,
        "nodes": nodes,
    })
}

fn node(name: &str, questions: Value, edge: Value, action: Value) -> Value {
    json!({
        "name": name,
        "primary_q": "q",
        "questions": questions,
        "edge": edge,
        "min_confidence": 0.0,
        "max_retries": 1,
        "action": action,
    })
}

/// Regression: unknown action `kind` must be rejected at parse time.
pub fn test_validate_unknown_action_kind(h: &mut Harness) {
    let spec = base_spec(
        "a",
        json!([node(
            "a",
            json!({"q": {"type": "choice", "instructions": "?", "criteria": {"A": "no", "B": "yes"}}}),
            json!({"condition": {"A": "STOP", "B": "STOP"}, "default": "STOP"}),
            json!({"kind": "teleport", "question": "q"})
        )]),
    );
    let r = spec::from_spec(&spec);
    h.check("validate: unknown action kind rejected", r.is_err());
}

/// Regression: edge target naming a node that does not exist must be rejected.
pub fn test_validate_dangling_edge(h: &mut Harness) {
    let spec = base_spec(
        "a",
        json!([node(
            "a",
            json!({"q": {"type": "choice", "instructions": "?", "criteria": {"A": "no", "B": "yes"}}}),
            json!({"condition": {"A": "ghost_node", "B": "STOP"}, "default": "STOP"}),
            json!({"kind": "copy_keys", "keys": ["q"]})
        )]),
    );
    let r = spec::from_spec(&spec);
    h.check("validate: dangling edge target rejected", r.is_err());
}

/// Regression: threshold rule referencing a question that does not exist must
/// be rejected (unless it is the node's own action question).
pub fn test_validate_unknown_question_ref(h: &mut Harness) {
    let spec = base_spec(
        "a",
        json!([node(
            "a",
            json!({"severity": {"type": "score", "instructions": "?", "criteria": ["minor", "degraded"]}}),
            json!({"condition": {}, "default": "STOP"}),
            json!({"kind": "threshold", "question": "severity",
                   "rules": [{"when": {"question": "nonexistent_q", "value_gte": 1.5}, "label": "PAGE"}]})
        )]),
    );
    let r = spec::from_spec(&spec);
    h.check("validate: unknown threshold question ref rejected", r.is_err());
}

/// Control: a semantically valid spec (all kinds known, edges resolve,
/// question refs exist) must still parse fine — no false positives.
pub fn test_validate_control_ok(h: &mut Harness) {
    let spec = base_spec(
        "classify",
        json!([
            node(
                "classify",
                json!({"is_urgent": {"type": "choice", "instructions": "?", "criteria": {"A": "no", "B": "yes"}}}),
                json!({"condition": {"A": "STOP", "B": "escalate"}, "default": "STOP"}),
                json!({"kind": "gate", "question": "is_urgent", "option": "B", "allow_at": 0.8, "block_at": 0.8})
            ),
            node(
                "escalate",
                json!({"severity": {"type": "score", "instructions": "?", "criteria": ["minor", "degraded"]}}),
                json!({"condition": {}, "default": "STOP"}),
                json!({"kind": "threshold", "question": "severity",
                       "rules": [{"when": {"value_gte": 1.5}, "label": "PAGE_ONCALL"},
                                 {"when": {"always": true}, "label": "QUEUE_HIGH"}]})
            )
        ]),
    );
    let wf = spec::from_spec(&spec);
    h.check("validate: valid spec parses (no false positive)", wf.is_ok());
}
