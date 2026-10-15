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

/// Regression (P136): a node that routes on a question (non-empty edge
/// condition keys) must name its `primary_q` when it declares more than one
/// question. Routing matches the *primary* answer against the condition keys,
/// and `WorkflowNode::primary()` falls back to the first-declared question —
/// so with several questions and no explicit primary the routed question would
/// be inferred from declaration order and a reorder would silently misroute.
pub fn test_validate_primary_required(h: &mut Harness) {
    let two_q = json!({
        "event": {"type": "choice", "instructions": "?", "criteria": {"login": "yes", "logout": "no"}},
        "risk": {"type": "score", "instructions": "?", "criteria": ["low", "high"]}
    });
    // routes on a question, two questions, no primary -> reject
    let bad = base_spec(
        "a",
        json!([node(
            "a",
            two_q.clone(),
            json!({"condition": {"login": "STOP", "logout": "STOP"}, "default": "STOP"}),
            json!({"kind": "copy_keys", "keys": ["event"]})
        )]),
    );
    h.check("validate: multi-question routing node without primary_q rejected", spec::from_spec(&bad).is_err());
    // control: same node names the primary -> parses
    let ok = base_spec(
        "a",
        json!([{
            "name": "a",
            "primary_q": "event",
            "questions": two_q.clone(),
            "edge": {"condition": {"login": "STOP", "logout": "STOP"}, "default": "STOP"},
            "min_confidence": 0.0,
            "max_retries": 1,
            "action": {"kind": "copy_keys", "keys": ["event"]}
        }]),
    );
    h.check("validate: multi-question routing node with primary_q parses (no false positive)", spec::from_spec(&ok).is_ok());
    // control: two questions but NO routing (condition empty) -> primary not required
    let no_route = base_spec(
        "a",
        json!([node(
            "a",
            two_q.clone(),
            json!({"condition": {}, "default": "STOP"}),
            json!({"kind": "copy_keys", "keys": ["event", "risk"]})
        )]),
    );
    h.check("validate: multi-question node with no routing needs no primary_q", spec::from_spec(&no_route).is_ok());
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

/// Regression (P139): a spec that `validate`s clean must be *runnable*. An action
/// whose answer question is unresolvable (threshold/gate/merge_open_probs with
/// neither `action.question` nor a node `primary_q`) — or a rubric/combine action
/// missing the fields the runtime reads — used to pass validation and then bail
/// on the first walk (`action … needs 'question' or a primary_q`, `rubric
/// dimension … needs 'levels'`, `action combine: needs 'choice'`, …). Each is now
/// rejected up front; each shape with the field present still passes. The compiler
/// always emits these fields, so this guards hand-written / imported specs.
pub fn test_validate_action_answer_contract(h: &mut Harness) {
    let q = json!({"q": {"type": "choice", "instructions": "?", "criteria": {"A": "no", "B": "yes"}}});
    let score_q = json!({"q": {"type": "score", "instructions": "?", "criteria": ["low", "high"]}});
    let edge = json!({"condition": {}, "default": "STOP"});

    // threshold / gate / merge_open_probs: the answer question must resolve.
    let thr = |action: Value, extra: Value| {
        let mut n = node("a", q.clone(), edge.clone(), action);
        if let Some(m) = extra.as_object() {
            for (k, v) in m {
                n[k.as_str()] = v.clone();
            }
        }
        spec::from_spec(&base_spec("a", json!([n]))).is_err()
    };
    h.check(
        "validate: threshold action with no question and no primary_q rejected",
        thr(json!({"kind": "threshold", "rules": [{"label": "X"}]}), json!({})),
    );
    h.check(
        "validate: gate action with no question and no primary_q rejected",
        thr(json!({"kind": "gate", "option": "B"}), json!({})),
    );
    h.check(
        "validate: merge_open_probs with no question and no primary_q rejected",
        thr(json!({"kind": "merge_open_probs"}), json!({})),
    );
    h.check(
        "validate: threshold action OK when the node declares primary_q",
        !thr(json!({"kind": "threshold", "rules": [{"label": "X"}]}), json!({"primary_q": "q"})),
    );
    h.check(
        "validate: threshold action OK when the action names the question",
        !thr(json!({"kind": "threshold", "question": "q", "rules": [{"label": "X"}]}), json!({})),
    );

    // rubric: dimensions must be a non-empty array of {question, levels}.
    let rub = |dims: Value| {
        spec::from_spec(&base_spec(
            "a",
            json!([node("a", score_q.clone(), edge.clone(), json!({"kind": "rubric", "dimensions": dims}))]),
        ))
        .is_err()
    };
    h.check("validate: rubric with no dimensions rejected", rub(json!(null)));
    h.check("validate: rubric with an empty dimensions array rejected", rub(json!([])));
    h.check(
        "validate: rubric dimension with no question rejected",
        rub(json!([{"levels": 3, "weight": 1.0}])),
    );
    h.check(
        "validate: rubric dimension with no levels rejected",
        rub(json!([{"question": "q", "weight": 1.0}])),
    );
    h.check(
        "validate: rubric with a complete dimension OK",
        !rub(json!([{"question": "q", "levels": 3, "weight": 1.0}])),
    );

    // combine: choice and signal must both be present (and declared).
    let comb = |action: Value| {
        let qs = json!({"c": {"type": "choice", "instructions": "?", "criteria": {"A": "no", "B": "yes"}},
                        "s": {"type": "noul", "instructions": "?", "criteria": {"A": "no", "B": "yes"}}});
        spec::from_spec(&base_spec("a", json!([node("a", qs, edge.clone(), action)]))).is_err()
    };
    h.check(
        "validate: combine with no choice rejected",
        comb(json!({"kind": "combine", "signal": "s"})),
    );
    h.check(
        "validate: combine with no signal rejected",
        comb(json!({"kind": "combine", "choice": "c"})),
    );
    h.check(
        "validate: combine with choice + signal OK",
        !comb(json!({"kind": "combine", "choice": "c", "signal": "s"})),
    );
}

/// Regression (P140): the *rest* of the "validate must imply runnable" contract —
/// a `call` action must name a capability (`run_call_action` bails `action
/// kind=call needs 'capability'`), each `chain` step must name one (`chain step
/// {i} needs 'capability'`), and a `ui_proposal` must declare the questions it
/// reads by name (`ui_proposal: no answer {name}`; the names default to
/// operation/field/amount). Each shape with the field present still passes.
pub fn test_validate_call_and_ui_contract(h: &mut Harness) {
    let q = json!({"q": {"type": "choice", "instructions": "?", "criteria": {"A": "no", "B": "yes"}}});
    let edge = json!({"condition": {}, "default": "STOP"});
    let with_caps = |caps: Value, action: Value| {
        let mut spec = base_spec("a", json!([node("a", q.clone(), edge.clone(), action)]));
        spec["capabilities"] = caps;
        spec
    };

    h.check(
        "validate: call action with no capability rejected",
        spec::from_spec(&with_caps(json!({}), json!({"kind": "call"}))).is_err(),
    );
    h.check(
        "validate: call chain step with no capability rejected",
        spec::from_spec(&with_caps(
            json!({"shell": {"kind": "datetime"}}),
            json!({"kind": "call", "capability": "shell", "chain": [{"as": "x"}]}),
        ))
        .is_err(),
    );
    h.check(
        "validate: call action with a declared capability OK",
        spec::from_spec(&with_caps(json!({"shell": {"kind": "datetime"}}), json!({"kind": "call", "capability": "shell"}))).is_ok(),
    );

    let ui = |action: Value| spec::from_spec(&base_spec("a", json!([node("a", q.clone(), edge.clone(), action)]))).is_err();
    h.check(
        "validate: ui_proposal whose questions are undeclared rejected",
        ui(json!({"kind": "ui_proposal"})),
    );
    let q3 = json!({
        "operation": {"type": "choice", "instructions": "?", "criteria": {"fill": 1, "done": 1, "wait": 1, "blocked": 1}},
        "field": {"type": "choice", "instructions": "?", "criteria": {"a": 1, "b": 1}},
        "amount": {"type": "choice", "instructions": "?", "criteria": {"none": 1, "full": 1}},
    });
    let ok_ui = base_spec(
        "a",
        json!([node("a", q3, edge.clone(), json!({"kind": "ui_proposal", "operation": "operation", "field": "field", "amount": "amount"}))]),
    );
    h.check(
        "validate: ui_proposal with operation/field/amount declared OK",
        spec::from_spec(&ok_ui).is_ok(),
    );
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


/// Regression (P39): a `call` node whose `action.capability` is not declared in
/// the spec's `capabilities` map must be rejected at parse time. Previously it
/// passed `validate` cleanly and only hard-failed at runtime
/// (`capability "ghost" is not declared in this spec`) — the same dangling-
/// reference class as P24 edges / P27 copy_keys.
pub fn test_validate_call_capability(h: &mut Harness) {
    let spec = base_spec(
        "greet",
        json!([node(
            "greet",
            json!({"q": {"type": "choice", "instructions": "?", "criteria": {"A": "no", "B": "yes"}}}),
            json!({"condition": {}, "default": "STOP"}),
            json!({"kind": "call", "capability": "ghost"})
        )]),
    );
    let r = spec::from_spec(&spec);
    h.check("validate: call to undeclared capability rejected", r.is_err());
    if let Err(e) = &r {
        let m = e.to_string();
        h.check("validate: call capability error names the node", m.contains("greet"));
        h.check("validate: call capability error names the capability", m.contains("ghost"));
    }
}

/// Regression (P39): every `action.chain[].capability` step is checked too, and
/// a spec that declares every capability it calls (base + every chain step)
/// still parses — no false positive.
pub fn test_validate_call_chain_capability(h: &mut Harness) {
    let mut bad = base_spec(
        "greet",
        json!([node(
            "greet",
            json!({"q": {"type": "choice", "instructions": "?", "criteria": {"A": "no", "B": "yes"}}}),
            json!({"condition": {}, "default": "STOP"}),
            json!({"kind": "call", "capability": "greet", "chain": [{"capability": "grete"}]})
        )]),
    );
    bad["capabilities"] = json!({"greet": {"kind": "plugin", "plugin": "websites/x"}});
    let r = spec::from_spec(&bad);
    h.check("validate: call chain step undeclared capability rejected", r.is_err());
    if let Err(e) = &r {
        h.check("validate: call chain capability did-you-mean", e.to_string().contains("did you mean"));
    }

    let mut ok = base_spec(
        "greet",
        json!([node(
            "greet",
            json!({"q": {"type": "choice", "instructions": "?", "criteria": {"A": "no", "B": "yes"}}}),
            json!({"condition": {}, "default": "STOP"}),
            json!({"kind": "call", "capability": "greet", "chain": [{"capability": "wrap"}]})
        )]),
    );
    ok["capabilities"] = json!({
        "greet": {"kind": "plugin", "plugin": "websites/x"},
        "wrap": {"kind": "plugin", "plugin": "websites/y"}
    });
    h.check("validate: declared call capabilities parse (no false positive)", spec::from_spec(&ok).is_ok());
}

/// Regression (P128): a capability option the engine never reads is a silent
/// no-op — `max_operatons` leaves the plugin at the default operation cap,
/// `chorme_binary` leaves Chrome to `$PATH`, and nothing says so. `parse_cap`
/// now records every key a kind's parser consumes and refuses the rest, so the
/// typo is a `validate` error with a did-you-mean. The keys the engine reads
/// *outside* a parser (`platforms` drives the smoke/BDD platform gate,
/// `description` is an annotation) stay legal.
pub fn test_validate_unread_option(h: &mut Harness) {
    // A typo'd option on a real kind: rejected, and named with its near match.
    let mut bad = base_spec(
        "greet",
        json!([node(
            "greet",
            json!({"q": {"type": "choice", "instructions": "?", "criteria": {"A": "no", "B": "yes"}}}),
            json!({"condition": {}, "default": "STOP"}),
            json!({"kind": "call", "capability": "greet", "with": {}})
        )]),
    );
    bad["capabilities"] = json!({
        "greet": {"kind": "plugin", "plugin": "textdigest", "max_operatons": 2_000_000}
    });
    let r = spec::from_spec(&bad);
    h.check("validate: a typo'd capability option is rejected", r.is_err());
    match &r {
        Err(e) => {
            let m = e.to_string();
            h.check("validate: the error names the unread option", m.contains("max_operatons"));
            h.check("validate: the error says the engine never reads it", m.contains("never reads"));
            h.check("validate: the error suggests the real option", m.contains("max_operations"));
        }
        Ok(_) => h.check("validate: expected an error", false),
    }
    // A kind with a unit parser: *every* option is unread, not just typos.
    let mut dead = base_spec(
        "greet",
        json!([node(
            "greet",
            json!({"q": {"type": "choice", "instructions": "?", "criteria": {"A": "no", "B": "yes"}}}),
            json!({"condition": {}, "default": "STOP"}),
            json!({"kind": "call", "capability": "v", "with": {}})
        )]),
    );
    dead["capabilities"] = json!({"v": {"kind": "validate", "op": "check"}});
    let r2 = spec::from_spec(&dead);
    h.check("validate: an option on a unit-parser kind (validate) is unread", r2.is_err());
    if let Err(e) = &r2 {
        h.check("validate: it names 'op'", e.to_string().contains("op"));
    }
    // No false positive: the options a parser *does* read, and the two read
    // elsewhere, all stay legal on the same spec.
    let mut ok = base_spec(
        "greet",
        json!([node(
            "greet",
            json!({"q": {"type": "choice", "instructions": "?", "criteria": {"A": "no", "B": "yes"}}}),
            json!({"condition": {}, "default": "STOP"}),
            json!({"kind": "call", "capability": "greet", "with": {}})
        )]),
    );
    ok["capabilities"] = json!({
        "greet": {
            "kind": "plugin", "plugin": "textdigest", "dir": "", "entry": "", "op": "run",
            "browser": "", "max_operations": 2_000_000, "timeout_ms": 1000,
            "platforms": ["unix"], "description": "an annotated capability"
        }
    });
    h.check("validate: every option a parser reads stays legal", spec::from_spec(&ok).is_ok());
}

/// Regression (P45): `min_confidence` outside [0, 1] is meaningless. A value
/// > 1 makes *every* route fail (the node always ESCALATEs) and < 0 is a typo;
/// both silently misroute at runtime instead of failing up front.
pub fn test_validate_min_conf_range(h: &mut Harness) {
    let mk = |mc: Value| {
        let mut spec = base_spec(
            "a",
            json!([node(
                "a",
                json!({"q": {"type": "choice", "instructions": "?", "criteria": {"A": "no", "B": "yes"}}}),
                json!({"condition": {"A": "STOP", "B": "STOP"}, "default": "STOP"}),
                json!({"kind": "copy_keys", "keys": ["q"]})
            )]),
        );
        spec["nodes"][0]["min_confidence"] = mc;
        spec
    };
    let over = spec::from_spec(&mk(json!(2.5)));
    h.check("validate: min_confidence > 1 rejected", over.is_err());
    if let Err(e) = &over {
        h.check(
            "validate: min_confidence error names the field",
            e.to_string().contains("min_confidence"),
        );
    }
    h.check(
        "validate: min_confidence < 0 rejected",
        spec::from_spec(&mk(json!(-0.1))).is_err(),
    );
    h.check(
        "validate: min_confidence in range parses (no false positive)",
        spec::from_spec(&mk(json!(0.5))).is_ok(),
    );
}

/// Regression (P45): `max_iterations` must be a positive integer — 0 (or a
/// negative / non-integer value) runs the graph zero times or silently falls
/// back to the default 50, so a spec that says it bounds the loop does not.
pub fn test_validate_max_iterations(h: &mut Harness) {
    let mk = |mi: Value| {
        let mut spec = base_spec(
            "a",
            json!([node(
                "a",
                json!({"q": {"type": "choice", "instructions": "?", "criteria": {"A": "no", "B": "yes"}}}),
                json!({"condition": {"A": "STOP", "B": "STOP"}, "default": "STOP"}),
                json!({"kind": "copy_keys", "keys": ["q"]})
            )]),
        );
        spec["max_iterations"] = mi;
        spec
    };
    let zero = spec::from_spec(&mk(json!(0)));
    h.check("validate: max_iterations 0 rejected", zero.is_err());
    if let Err(e) = &zero {
        h.check(
            "validate: max_iterations error names the field",
            e.to_string().contains("max_iterations"),
        );
    }
    h.check(
        "validate: negative max_iterations rejected",
        spec::from_spec(&mk(json!(-1))).is_err(),
    );
    h.check(
        "validate: non-integer max_iterations rejected",
        spec::from_spec(&mk(json!(2.5))).is_err(),
    );
    h.check(
        "validate: positive max_iterations parses (no false positive)",
        spec::from_spec(&mk(json!(7))).is_ok(),
    );
}

/// A node with a score question and a threshold ladder built from `rules`.
fn thr_node(rules: Value) -> Value {
    base_spec(
        "a",
        json!([node(
            "a",
            json!({"severity": {"type": "score", "instructions": "?", "criteria": ["minor", "degraded"]}}),
            json!({"condition": {}, "default": "STOP"}),
            json!({"kind": "threshold", "question": "severity", "rules": rules})
        )]),
    )
}

/// Regression (P45): a threshold ladder that does not end in an unconditional
/// catch-all emits the literal label "unmatched" at runtime for scores no rule
/// matches — a silent misroute rather than a decision.
pub fn test_validate_threshold_not_exhaustive(h: &mut Harness) {
    let r = spec::from_spec(&thr_node(json!([
        {"when": {"value_gte": 1.5}, "label": "PAGE"},
        {"when": {"value_lt": 1.5}, "label": "QUEUE"}
    ])));
    h.check("validate: non-exhaustive threshold ladder rejected", r.is_err());
    if let Err(e) = &r {
        h.check(
            "validate: non-exhaustive error mentions unmatched",
            e.to_string().contains("unmatched"),
        );
    }
    // control: a ladder ending in a catch-all parses — no false positive.
    h.check(
        "validate: exhaustive threshold ladder parses (no false positive)",
        spec::from_spec(&thr_node(json!([
            {"when": {"value_gte": 1.5}, "label": "PAGE"},
            {"when": {"always": true}, "label": "QUEUE"}
        ])))
        .is_ok(),
    );
}

/// Regression (P45): a repeated `when` can never fire (first match wins), a
/// catch-all that is not last makes the rules after it dead code, and a rule
/// with no `label` emits the literal "unknown".
pub fn test_validate_threshold_dead_rules(h: &mut Harness) {
    let dup = spec::from_spec(&thr_node(json!([
        {"when": {"value_gte": 1.5}, "label": "PAGE"},
        {"when": {"value_gte": 1.5}, "label": "PAGE_AGAIN"},
        {"when": {"always": true}, "label": "QUEUE"}
    ])));
    h.check("validate: duplicate threshold when rejected", dup.is_err());
    if let Err(e) = &dup {
        h.check(
            "validate: duplicate when error says repeats",
            e.to_string().contains("repeats"),
        );
    }
    h.check(
        "validate: catch-all before last rule rejected",
        spec::from_spec(&thr_node(json!([
            {"when": {"value_gte": 1.5}, "label": "PAGE"},
            {"when": {"always": true}, "label": "QUEUE"},
            {"when": {"value_lt": 1.5}, "label": "DEAD"}
        ])))
        .is_err(),
    );
    h.check(
        "validate: threshold rule without label rejected",
        spec::from_spec(&thr_node(json!([{"when": {"always": true}}]))).is_err(),
    );
}

/// P45 invariant: every workflow spec checked into `dsl/` still parses under
/// the stricter graph rules — the machine-checked form of the "zero false
/// positives across all checked-in specs" claim.
pub fn test_validate_repo_specs_strict(h: &mut Harness) {
    // Same scratch env + fresh secret snapshot the CLI/dsl-smoke take, so specs
    // that interpolate `${env.HOME}` / `${env.LAYA_STORE_DIR}` (policy
    // allow_paths) parse instead of bailing on an unresolved reference.
    if let Ok(env) = laya_workflow::dsl_smoke::ensure_env_dirs() {
        for (k, v) in &env {
            std::env::set_var(k, v);
        }
    }
    let _ = laya_workflow::capability::secret::init(None);
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("dsl");
    let specs = match laya_workflow::dsl_smoke::discover_specs(&root) {
        Ok(s) => s,
        Err(e) => {
            h.check(&format!("validate: discover dsl specs failed: {e}"), false);
            return;
        }
    };
    h.check("validate: repo dsl specs discovered", specs.len() >= 90);
    let mut ok = 0usize;
    let mut first_err: Option<String> = None;
    for p in &specs {
        let raw = match std::fs::read_to_string(p) {
            Ok(s) => s,
            Err(e) => {
                first_err = Some(format!("{}: {e}", p.display()));
                break;
            }
        };
        let v: Value = match serde_json::from_str(&raw) {
            Ok(v) => v,
            Err(e) => {
                first_err = Some(format!("{}: {e}", p.display()));
                break;
            }
        };
        match spec::from_spec_in(&v, p.parent()) {
            Ok(_) => ok += 1,
            Err(e) => {
                first_err = Some(format!("{}: {e}", p.display()));
                break;
            }
        }
    }
    match first_err {
        Some(msg) => h.check(
            &format!("validate: repo spec rejected by stricter rules: {msg}"),
            false,
        ),
        None => h.check(
            &format!("validate: all {ok} repo dsl specs satisfy the stricter rules"),
            ok == specs.len(),
        ),
    }
}

/// P122: a threshold rule leaf's operator must read the shape its question's
/// *declared type* produces. The engine reads `option`/`gte`/`lt` through
/// `Decision::prob`, `value_gte`/`value_lt` through `Decision::as_f64`, and
/// `answer`/`answer_in` through `Decision::as_str`; a `choice` answer is an
/// option-key string, a `score` answer is a number with ordinal probabilities,
/// and a `noul` answer is a probability number. A leaf whose operator reads a
/// shape its question never produces compares against 0.0 (or a number's text)
/// and silently takes a branch nobody chose — the same class as a dead
/// `copy_keys` key or a dangling edge, caught here instead of misrouting.
pub fn test_validate_rule_scope_coherence(h: &mut Harness) {
    fn node_with(questions: Value, action_q: &str, rules: Value) -> Value {
        base_spec(
            "a",
            json!([node(
                "a",
                questions,
                json!({"condition": {}, "default": "STOP"}),
                json!({"kind": "threshold", "question": action_q, "rules": rules})
            )]),
        )
    }
    let choice_q = json!({"c": {"type": "choice", "instructions": "?", "criteria": {"A": "no", "B": "yes"}}});
    let score_q = json!({"s": {"type": "score", "instructions": "?", "criteria": ["minor", "degraded"]}});
    let noul_q = json!({"n": {"type": "noul", "instructions": "?", "truthy": "yes", "falsy": "no"}});

    // `value_gte`/`value_lt` read `as_f64()` — a choice answer is a string -> 0.0.
    let bad = node_with(
        choice_q.clone(),
        "c",
        json!([
            {"when": {"value_gte": 0.5}, "label": "A"},
            {"when": {"always": true}, "label": "B"}
        ]),
    );
    h.check(
        "validate: value_gte on a choice question rejected",
        spec::from_spec(&bad).is_err(),
    );
    if let Err(e) = spec::from_spec(&bad) {
        let m = e.to_string();
        h.check(
            "validate: choice/value_gte error names the mismatch",
            m.contains("value_gte") && m.contains("choice"),
        );
    }

    // `option` reads `prob(key)` — a score's probabilities are ordinal, never "A".
    let bad = node_with(
        score_q.clone(),
        "s",
        json!([
            {"when": {"option": "A", "gte": 0.5}, "label": "A"},
            {"when": {"always": true}, "label": "B"}
        ]),
    );
    h.check(
        "validate: option on a score question rejected",
        spec::from_spec(&bad).is_err(),
    );

    // `answer`/`answer_in` read `as_str()` — a noul answer is a number's text.
    let bad = node_with(
        noul_q.clone(),
        "n",
        json!([
            {"when": {"answer_in": ["true"]}, "label": "A"},
            {"when": {"always": true}, "label": "B"}
        ]),
    );
    h.check(
        "validate: answer_in on a noul question rejected",
        spec::from_spec(&bad).is_err(),
    );

    // `gte`/`lt` outside an `option` block are never read at all.
    let bad = node_with(
        score_q.clone(),
        "s",
        json!([
            {"when": {"gte": 0.5}, "label": "A"},
            {"when": {"always": true}, "label": "B"}
        ]),
    );
    h.check(
        "validate: gte without option rejected",
        spec::from_spec(&bad).is_err(),
    );

    // The correct idioms still pass: a noul answers to `option`+`gte` on
    // "true"/"false" (the rag_triage shape), a score to `value_gte`/`value_lt`,
    // and a choice to `answer_in`/`option`.
    let good = node_with(
        noul_q.clone(),
        "n",
        json!([
            {"when": {"option": "false", "gte": 0.85}, "label": "EXCLUDE"},
            {"when": {"option": "true", "gte": 0.85}, "label": "INCLUDE"},
            {"when": {"always": true}, "label": "REVIEW"}
        ]),
    );
    h.check(
        "validate: option+gte on a noul question accepted",
        spec::from_spec(&good).is_ok(),
    );
    let good = node_with(
        score_q.clone(),
        "s",
        json!([
            {"when": {"value_gte": 1.5}, "label": "A"},
            {"when": {"value_lt": 0.5}, "label": "B"},
            {"when": {"always": true}, "label": "C"}
        ]),
    );
    h.check(
        "validate: value_gte/value_lt on a score question accepted",
        spec::from_spec(&good).is_ok(),
    );
    let good = node_with(
        choice_q.clone(),
        "c",
        json!([
            {"when": {"answer_in": ["A"]}, "label": "A"},
            {"when": {"option": "B", "gte": 0.5}, "label": "B"},
            {"when": {"always": true}, "label": "C"}
        ]),
    );
    h.check(
        "validate: answer_in/option on a choice question accepted",
        spec::from_spec(&good).is_ok(),
    );

    // The scope is the leaf's own `question`, else the action's — a leaf naming a
    // different question is checked against *that* question's type, not the
    // action's. Here the action is a score but the leaf reads the choice question.
    let cross = base_spec(
        "a",
        json!([node(
            "a",
            json!({
                "s": {"type": "score", "instructions": "?", "criteria": ["lo", "hi"]},
                "c": {"type": "choice", "instructions": "?", "criteria": {"A": "no", "B": "yes"}}
            }),
            json!({"condition": {}, "default": "STOP"}),
            json!({"kind": "threshold", "question": "s", "rules": json!([
                {"when": {"question": "c", "value_gte": 0.5}, "label": "X"},
                {"when": {"always": true}, "label": "Y"}
            ])})
        )]),
    );
    h.check(
        "validate: a cross-question leaf is checked against its own question's type",
        spec::from_spec(&cross).is_err(),
    );
}
