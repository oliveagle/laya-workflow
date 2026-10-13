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

/// Regression: an unreachable (dead-code) node must be rejected. P24's
/// edge-existence check missed this class — `audit` existed but had no inbound
/// edge, so it never ran and silently hid the misroute.
pub fn test_validate_dead_node(h: &mut Harness) {
    let spec = base_spec(
        "a",
        json!([
            node(
                "a",
                json!({"q": {"type": "choice", "instructions": "?", "criteria": {"A": "no", "B": "yes"}}}),
                json!({"condition": {"A": "STOP", "B": "STOP"}, "default": "STOP"}),
                json!({"kind": "copy_keys", "keys": ["q"]})
            ),
            node(
                "orphan",
                json!({"r": {"type": "choice", "instructions": "?", "criteria": {"A": "no", "B": "yes"}}}),
                json!({"condition": {}, "default": "STOP"}),
                json!({"kind": "copy_keys", "keys": ["r"]})
            )
        ]),
    );
    let r = spec::from_spec(&spec);
    h.check("validate: unreachable dead node rejected", r.is_err());
}

/// Regression: duplicate node names make edges ambiguous.
pub fn test_validate_duplicate_names(h: &mut Harness) {
    let spec = base_spec(
        "a",
        json!([
            node("a", json!({"q": {"type": "choice", "instructions": "?", "criteria": {"A": "no", "B": "yes"}}}),
                 json!({"condition": {}, "default": "STOP"}), json!({"kind": "copy_keys", "keys": ["q"]})),
            node("a", json!({"q": {"type": "choice", "instructions": "?", "criteria": {"A": "no", "B": "yes"}}}),
                 json!({"condition": {}, "default": "STOP"}), json!({"kind": "copy_keys", "keys": ["q"]}))
        ]),
    );
    let r = spec::from_spec(&spec);
    h.check("validate: duplicate node names rejected", r.is_err());
}

/// Regression: `primary_q` must name a declared question.
pub fn test_validate_primary_q_missing(h: &mut Harness) {
    let spec = base_spec(
        "a",
        json!([{
            "name": "a",
            "primary_q": "ghost_q",
            "questions": {"q": {"type": "choice", "instructions": "?", "criteria": {"A": "no", "B": "yes"}}},
            "edge": {"condition": {}, "default": "STOP"},
            "min_confidence": 0.0,
            "max_retries": 1,
            "action": {"kind": "copy_keys", "keys": ["q"]}
        }]),
    );
    let r = spec::from_spec(&spec);
    h.check("validate: missing primary_q rejected", r.is_err());
}

/// Regression: action.question on a gate must name a declared question.
pub fn test_validate_action_q_missing(h: &mut Harness) {
    let spec = base_spec(
        "a",
        json!([node(
            "a",
            json!({"q": {"type": "choice", "instructions": "?", "criteria": {"A": "no", "B": "yes"}}}),
            json!({"condition": {}, "default": "STOP"}),
            json!({"kind": "gate", "question": "ghost_q", "option": "B", "allow_at": 0.8, "block_at": 0.8})
        )]),
    );
    let r = spec::from_spec(&spec);
    h.check("validate: gate action question missing rejected", r.is_err());
}

/// Regression: gate.option must be a choice of the question's criteria.
pub fn test_validate_gate_option(h: &mut Harness) {
    let spec = base_spec(
        "a",
        json!([node(
            "a",
            json!({"q": {"type": "choice", "instructions": "?", "criteria": {"A": "no", "B": "yes"}}}),
            json!({"condition": {}, "default": "STOP"}),
            json!({"kind": "gate", "question": "q", "option": "Z", "allow_at": 0.8, "block_at": 0.8})
        )]),
    );
    let r = spec::from_spec(&spec);
    h.check("validate: gate option not a choice rejected", r.is_err());
}

/// Regression: rubric dimension question must be declared.
pub fn test_validate_rubric_dim(h: &mut Harness) {
    let spec = base_spec(
        "a",
        json!([node(
            "a",
            json!({"q": {"type": "score", "instructions": "?", "criteria": ["low", "high"]}}),
            json!({"condition": {}, "default": "STOP"}),
            json!({"kind": "rubric", "dimensions": [{"question": "ghost_dim", "weight": 1.0, "levels": 3}]})
        )]),
    );
    let r = spec::from_spec(&spec);
    h.check("validate: rubric dimension question missing rejected", r.is_err());
}

/// Regression: combine.choice / combine.signal must be declared questions.
pub fn test_validate_combine_q(h: &mut Harness) {
    let spec = base_spec(
        "a",
        json!([node(
            "a",
            json!({"c": {"type": "choice", "instructions": "?", "criteria": {"A": "no", "B": "yes"}},
                   "s": {"type": "noul", "instructions": "?", "criteria": {"A": "no", "B": "yes"}}}),
            json!({"condition": {}, "default": "STOP"}),
            json!({"kind": "combine", "choice": "c", "signal": "ghost_signal"})
        )]),
    );
    let r = spec::from_spec(&spec);
    h.check("validate: combine signal question missing rejected", r.is_err());
}

/// Regression: threshold rule `when` may only use known operators — an unknown
/// key silently never matches at runtime.
pub fn test_validate_when_operator(h: &mut Harness) {
    let spec = base_spec(
        "a",
        json!([node(
            "a",
            json!({"severity": {"type": "score", "instructions": "?", "criteria": ["low", "high"]}}),
            json!({"condition": {}, "default": "STOP"}),
            json!({"kind": "threshold", "question": "severity",
                   "rules": [{"when": {"value_get": 1.5}, "label": "PAGE"}]})
        )]),
    );
    let r = spec::from_spec(&spec);
    h.check("validate: unknown when operator rejected", r.is_err());
}

/// Positive: sub-workflow completion handoff. A reference node with an
/// `edge.default` continuation must redirect the sub's internal STOPs to the
/// caller's node, so a nested pipeline runs sub-graph THEN downstream step.
/// The composed graph must route prefilter -> intake(sub) -> audit and have no
/// dead node.
pub fn test_validate_sub_continue(h: &mut Harness) {
    // in-memory ref: build the sub spec inline via a temp dir so
    // resolve_ref_dir can find it.
    let tmp = std::env::temp_dir().join(format!("laya_sub_continue_{}", std::process::id()));
    let _ = std::fs::create_dir_all(&tmp);
    let sub = tmp.join("sub.json");
    std::fs::write(
        &sub,
        serde_json::to_string(&json!({
            "name": "sub",
            "start": "first",
            "nodes": [
                {"name": "first", "primary_q": "q",
                 "questions": {"q": {"type": "choice", "instructions": "?", "criteria": {"A": "no", "B": "yes"}}},
                 "edge": {"condition": {"A": "STOP", "B": "STOP"}, "default": "STOP"},
                 "min_confidence": 0.0, "max_retries": 1,
                 "action": {"kind": "copy_keys", "keys": ["q"]}}
            ]
        }))
        .unwrap(),
    )
    .unwrap();
    let outer = json!({
        "name": "outer",
        "start": "gate",
        "nodes": [
            {"name": "gate", "primary_q": "g",
             "questions": {"g": {"type": "choice", "instructions": "?", "criteria": {"A": "no", "B": "yes"}}},
             "edge": {"condition": {"A": "sub", "B": "STOP"}, "default": "STOP"},
             "min_confidence": 0.0, "max_retries": 1,
             "action": {"kind": "copy_keys", "keys": ["g"]}},
            {"name": "sub", "workflow": "sub",
             "edge": {"condition": {}, "default": "audit"}},
            {"name": "audit", "primary_q": "n",
             "questions": {"n": {"type": "choice", "instructions": "?", "criteria": {"A": "no", "B": "yes"}}},
             "edge": {"condition": {}, "default": "STOP"},
             "min_confidence": 0.0, "max_retries": 1,
             "action": {"kind": "copy_keys", "keys": ["n"]}}
        ]
    });
    let wf = spec::from_spec_in(&outer, Some(&tmp));
    match &wf {
        Ok(w) => {
            h.check("validate: sub continue parses", true);
            // composed graph: gate -> sub::first -> audit
            h.check("validate: composed has sub::first", w.nodes.contains_key("sub::first"));
            h.check("validate: composed has audit", w.nodes.contains_key("audit"));
            let audit_reachable = w.nodes
                .get("sub::first")
                .map(|n| {
                    n.edge.condition.values().any(|d| d == "audit")
                        || n.edge.default.as_deref() == Some("audit")
                })
                .unwrap_or(false);
            h.check("validate: sub STOP redirected to audit", audit_reachable);
            // run offline: gate A -> sub -> audit, no dead code
            let b = laya_workflow::backend::HeuristicBackend;
            let wf = w.clone();
            let out = wf.run(&b, &json!({"text": "hello"})).unwrap();
            let chain: Vec<String> = out
                .trace
                .steps
                .iter()
                .filter_map(|s| s.get("node").and_then(|v| v.as_str()).map(str::to_string))
                .collect();
            h.check(
                "validate: sub continue runs through audit",
                chain.last().map(|s| s == "audit").unwrap_or(false),
            );
        }
        Err(e) => {
            h.check(&format!("validate: sub continue parse error: {e}"), false);
        }
    }
    let _ = std::fs::remove_dir_all(&tmp);
}

/// Regression: an edge condition key that is not a criterion of the primary
/// question can never fire at runtime (silently dead routing).
pub fn test_validate_edge_key(h: &mut Harness) {
    let spec = base_spec(
        "a",
        json!([node(
            "a",
            json!({"q": {"type": "choice", "instructions": "?", "criteria": {"A": "no", "B": "yes"}}}),
            json!({"condition": {"A": "STOP", "X": "STOP"}, "default": "STOP"}),
            json!({"kind": "copy_keys", "keys": ["q"]})
        )]),
    );
    let r = spec::from_spec(&spec);
    h.check("validate: edge key not an answer rejected", r.is_err());
    // control: all keys are answers → parses
    let ok_spec = base_spec(
        "a",
        json!([node(
            "a",
            json!({"q": {"type": "choice", "instructions": "?", "criteria": {"A": "no", "B": "yes"}}}),
            json!({"condition": {"A": "STOP", "B": "STOP"}, "default": "STOP"}),
            json!({"kind": "copy_keys", "keys": ["q"]})
        )]),
    );
    let r = spec::from_spec(&ok_spec);
    h.check("validate: valid edge keys parse (no false positive)", r.is_ok());
}

/// Regression: `copy_keys.keys` must name questions declared on the node.
/// An unknown key silently no-ops at runtime (the verdict never contains it).
pub fn test_validate_copy_keys(h: &mut Harness) {
    // one key not declared anywhere on the node -> reject
    let bad = base_spec(
        "a",
        json!([node(
            "a",
            json!({"q": {"type": "choice", "instructions": "?", "criteria": {"A": "no", "B": "yes"}}}),
            json!({"condition": {"A": "STOP", "B": "STOP"}, "default": "STOP"}),
            json!({"kind": "copy_keys", "keys": ["q", "ghost"]})
        )]),
    );
    let r = spec::from_spec(&bad);
    h.check("validate: copy_keys unknown question rejected", r.is_err());
    // control: all keys declared -> parses (no false positive)
    let ok = base_spec(
        "a",
        json!([node(
            "a",
            json!({"q": {"type": "choice", "instructions": "?", "criteria": {"A": "no", "B": "yes"}}}),
            json!({"condition": {"A": "STOP", "B": "STOP"}, "default": "STOP"}),
            json!({"kind": "copy_keys", "keys": ["q"]})
        )]),
    );
    h.check("validate: copy_keys declared keys parse (no false positive)", spec::from_spec(&ok).is_ok());
}

/// Regression: `combine.fallback` must be a possible answer of the choice
/// question. An unknown fallback can never fire -> silently dead branch.
pub fn test_validate_combine_fallback(h: &mut Harness) {
    let mk = |fallback: &str| {
        base_spec(
            "a",
            json!([{
                "name": "a",
                "primary_q": "department",
                "questions": {
                    "department": {"type": "choice", "instructions": "?", "criteria": {"billing": "b", "technical": "t", "other": "o"}},
                    "urgency": {"type": "noul", "instructions": "?", "criteria": {"truthy": "yes", "falsy": "no"}}
                },
                "edge": {"condition": {}, "default": "STOP"},
                "min_confidence": 0.0, "max_retries": 1,
                "action": {"kind": "combine", "choice": "department", "signal": "urgency",
                           "fallback": fallback, "review_label": "human_review"}
            }]),
        )
    };
    let r = spec::from_spec(&mk("other"));
    h.check("validate: combine fallback in criteria parses", r.is_ok());
    let r2 = spec::from_spec(&mk("bogus"));
    h.check("validate: combine fallback outside criteria rejected", r2.is_err());
}

/// Regression: nested `workflow` references must not form a cycle (A -> B -> A).
/// A cycle would recurse forever (stack overflow); it must be rejected up front.
/// A diamond (two branches referencing the same leaf) stays valid.
pub fn test_validate_ref_cycle(h: &mut Harness) {
    let leaf = json!({"name": "p27_leaf", "start": "s",
        "nodes": [{"name": "s", "primary_q": "q",
                   "questions": {"q": {"type": "choice", "instructions": "?", "criteria": {"A": "yes"}}},
                   "edge": {"condition": {}, "default": "STOP"}}]});
    spec::register("p27_cycle_a", json!({"name": "p27_cycle_a", "start": "s",
        "nodes": [{"name": "s", "workflow": "p27_cycle_b"}]}));
    spec::register("p27_cycle_b", json!({"name": "p27_cycle_b", "start": "s",
        "nodes": [{"name": "s", "workflow": "p27_cycle_a"}]}));
    let outer = json!({"name": "p27_outer", "start": "go",
        "nodes": [{"name": "go", "workflow": "p27_cycle_a"}]});
    let r = spec::from_spec(&outer);
    h.check("validate: transitive ref cycle rejected", r.is_err());
    if let Err(e) = &r {
        h.check("validate: cycle error names the chain", e.to_string().contains("cycle"));
    }
    // diamond: two branches reference the same leaf -> no false positive
    spec::register("p27_leaf", leaf.clone());
    spec::register("p27_dia_b", json!({"name": "p27_dia_b", "start": "s",
        "nodes": [{"name": "s", "workflow": "p27_leaf"}]}));
    spec::register("p27_dia_c", json!({"name": "p27_dia_c", "start": "s",
        "nodes": [{"name": "s", "workflow": "p27_leaf"}]}));
    let outer2 = json!({"name": "p27_outer2", "start": "pre",
        "nodes": [
            {"name": "pre", "primary_q": "q",
             "questions": {"q": {"type": "choice", "instructions": "?", "criteria": {"A": "go", "B": "ho"}}},
             "edge": {"condition": {"A": "go", "B": "ho"}, "default": "STOP"}},
            {"name": "go", "workflow": "p27_dia_b"},
            {"name": "ho", "workflow": "p27_dia_c"},
        ]});
    h.check("validate: diamond (shared leaf) parses", spec::from_spec(&outer2).is_ok());
}
