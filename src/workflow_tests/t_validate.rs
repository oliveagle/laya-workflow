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

/// Regression (P141): a decision action must read a question whose *declared
/// type* produces what the action reads — the action-level sibling of P122
/// (which holds a threshold rule leaf's operator to its question's type). A gate
/// reads `prob(option)` (needs a choice), merge_open_probs reads keyed
/// probabilities (needs a choice), combine.choice routes on an option key (needs
/// a choice) while combine.signal is read as a number (needs score/noul), and a
/// threshold rule's `answer`/`answer_in` must name a real option (else it can
/// never fire). Each mismatch quietly computes 0.0 / never matches; each shape
/// with the right type still passes.
pub fn test_validate_action_question_type(h: &mut Harness) {
    let edge = json!({"condition": {}, "default": "STOP"});
    let choice_q = json!({"q": {"type": "choice", "instructions": "?", "criteria": {"A": "a", "B": "b"}}});
    let score_q = json!({"q": {"type": "score", "instructions": "?", "criteria": ["0", "1", "2"]}});
    let mk = |qs: Value, primary: Value, action: Value| {
        let mut n = json!({"name": "a", "questions": qs, "edge": edge.clone(),
                           "min_confidence": 0.0, "max_retries": 1, "action": action});
        if let Some(p) = primary.as_str() { n["primary_q"] = json!(p); }
        spec::from_spec(&base_spec("a", json!([n]))).is_err()
    };

    h.check(
        "validate: gate on a score question rejected",
        mk(score_q.clone(), json!(null), json!({"kind": "gate", "question": "q", "option": "B"})),
    );
    h.check(
        "validate: gate on a choice question OK",
        !mk(choice_q.clone(), json!(null), json!({"kind": "gate", "question": "q", "option": "B", "block_at": 0.6, "allow_at": 0.7})),
    );
    h.check(
        "validate: merge_open_probs on a score question rejected",
        mk(score_q.clone(), json!("q"), json!({"kind": "merge_open_probs", "question": "q"})),
    );
    h.check(
        "validate: merge_open_probs on a choice question OK",
        !mk(choice_q.clone(), json!("q"), json!({"kind": "merge_open_probs", "question": "q"})),
    );
    let combine_qs = json!({"c": {"type": "choice", "instructions": "?", "criteria": {"A": "a", "B": "b"}},
                            "s": {"type": "noul", "instructions": "?", "criteria": {"truthy": "y", "falsy": "n"}},
                            "sc": {"type": "score", "instructions": "?", "criteria": ["0", "1", "2"]}});
    h.check(
        "validate: combine.choice on a score question rejected",
        mk(combine_qs.clone(), json!("sc"), json!({"kind": "combine", "choice": "sc", "signal": "s"})),
    );
    h.check(
        "validate: combine.signal on a choice question rejected",
        mk(combine_qs.clone(), json!("c"), json!({"kind": "combine", "choice": "c", "signal": "c"})),
    );
    h.check(
        "validate: combine choice + noul signal OK",
        !mk(combine_qs.clone(), json!("c"), json!({"kind": "combine", "choice": "c", "signal": "s"})),
    );

    let thr = |rules: Value| mk(choice_q.clone(), json!(null), json!({"kind": "threshold", "question": "q", "rules": rules}));
    h.check(
        "validate: threshold rule answer not in the question's options rejected",
        thr(json!([{"when": {"answer": "C"}, "label": "X"}, {"label": "Y"}])),
    );
    h.check(
        "validate: threshold rule answer_in with an unknown member rejected",
        thr(json!([{"when": {"answer_in": ["A", "Z"]}, "label": "X"}, {"label": "Y"}])),
    );
    h.check(
        "validate: threshold rule answer in the options OK",
        !thr(json!([{"when": {"answer": "A"}, "label": "X"}, {"label": "Y"}])),
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

/// Regression (P142): the node's `state` projection must be a shape `project_state`
/// actually reads (`{"keep": [<key>…], "rename": {<out>: <key>}}`). Anything else
/// — a non-object `state`, a `keep` that is not a string array, a `rename` that is
/// not a string→string map, an unrecognised key, or a `state` object that declares
/// neither — is silently ignored, so the node sends the WHOLE state to the backend:
/// a projection that never projects. `validate` must reject it up front.
pub fn test_validate_state_contract(h: &mut Harness) {
    let edge = json!({"condition": {}, "default": "STOP"});
    let qs = json!({"q": {"type": "choice", "instructions": "?", "criteria": {"A": "a", "B": "b"}}});
    let mk = |state: Value| {
        let mut n = json!({"name": "a", "questions": qs.clone(), "edge": edge.clone(),
                           "min_confidence": 0.0, "max_retries": 1,
                           "action": {"kind": "copy_keys", "keys": ["q"]}});
        n["state"] = state;
        spec::from_spec(&base_spec("a", json!([n]))).is_err()
    };

    // negatives — each silently degrades to "send the whole state"
    h.check("validate: non-object state rejected", mk(json!(5)));
    h.check("validate: string state rejected", mk(json!("keep q")));
    h.check("validate: array state rejected", mk(json!(["q"])));
    h.check("validate: state.keep as a string rejected", mk(json!({"keep": "q"})));
    h.check("validate: state.keep with a non-string entry rejected", mk(json!({"keep": [1, "q"]})));
    h.check("validate: state with an unrecognised key rejected", mk(json!({"keeep": ["q"]})));
    h.check("validate: state.rename as a non-object rejected", mk(json!({"rename": ["q"]})));
    h.check("validate: state.rename with a non-string source rejected", mk(json!({"rename": {"out": 5}})));
    h.check("validate: state declaring neither keep nor rename rejected", mk(json!({})));

    // positives — the shapes `project_state` reads
    h.check("validate: non-empty state.keep OK", !mk(json!({"keep": ["q"]})));
    h.check("validate: empty state.keep OK (the compiled 'no keeps' encoding)", !mk(json!({"keep": []})));
    h.check("validate: state.rename OK", !mk(json!({"rename": {"out": "q"}})));
    h.check("validate: state keep + rename OK", !mk(json!({"keep": ["q"], "rename": {"out": "q"}})));
    h.check("validate: null state OK (no projection)", !mk(json!(null)));
}


/// P145: a node (and its `edge`) may only carry the keys the engine actually
/// reads. Any other key — a misspelling (`questoins`, `actoin`, `primmary_q`) or
/// a stray field — is never touched by the parser or the engine, so it is
/// silently dropped while `validate` still passes (the declared questions /
/// action / routing quietly vanish). `has_action` is the one derived key: it is
/// emitted by describe / round-trip, so it is recognised but must agree with the
/// presence of `action`.
pub fn test_validate_node_keys(h: &mut Harness) {
    let edge = json!({"condition": {}, "default": "STOP"});
    let qs = json!({"q": {"type": "choice", "instructions": "?", "criteria": {"A": "a", "B": "b"}}});
    let action = json!({"kind": "copy_keys", "keys": ["q"]});
    let ok = |n: Value| spec::from_spec(&base_spec("a", json!([n]))).is_ok();
    let full = || {
        json!({"name": "a", "questions": qs.clone(), "edge": edge.clone(),
               "min_confidence": 0.0, "max_retries": 1, "primary_q": "q",
               "action": action.clone()})
    };

    // positives — every recognised key on the node and on the edge
    h.check("validate: a fully declared node is OK", ok(full()));
    h.check("validate: edge.min_confidence is OK", {
        let mut n = full();
        n["edge"] = json!({"condition": {}, "default": "STOP", "min_confidence": 0.5});
        ok(n)
    });
    h.check("validate: a `state` projection is OK", {
        let mut n = full();
        n["state"] = json!({"keep": ["q"]});
        ok(n)
    });
    h.check("validate: has_action:true agreeing with `action` is OK", {
        let mut n = full();
        n["has_action"] = json!(true);
        ok(n)
    });
    h.check("validate: has_action:false with no action is OK", ok(json!({
        "name": "a", "questions": qs.clone(), "edge": edge.clone(), "has_action": false
    })));

    // negatives — an unrecognised node key, silently dropped by the parser
    let with_key = |k: &str| {
        let mut n = full();
        n[k] = json!("x");
        !ok(n)
    };
    h.check("validate: node key typo `questoins` rejected", with_key("questoins"));
    h.check("validate: node key typo `actoin` rejected", with_key("actoin"));
    h.check("validate: node key typo `primmary_q` rejected", with_key("primmary_q"));
    h.check("validate: a stray node key (e.g. `notes`) rejected", with_key("notes"));

    // negatives — an unrecognised key on the edge object
    h.check("validate: edge key typo `condtion` rejected", {
        let mut n = full();
        n["edge"] = json!({"condition": {}, "default": "STOP", "condtion": {}});
        !ok(n)
    });

    // negatives — `has_action` is derived; it must agree with `action`
    h.check("validate: has_action:true without an action rejected", {
        let mut n = full();
        n.as_object_mut().unwrap().remove("action");
        n["has_action"] = json!(true);
        !ok(n)
    });
    h.check("validate: has_action:false with an action rejected", {
        let mut n = full();
        n["has_action"] = json!(false);
        !ok(n)
    });
    h.check("validate: a non-boolean has_action rejected", {
        let mut n = full();
        n["has_action"] = json!("yes");
        !ok(n)
    });
}


/// P146: a question object and its `heuristic` may only carry the keys the engine
/// reads. A typo (`critera`, `answer_feild`) or a stray key is silently dropped, so
/// `validate` passes while the question's options / offline read-out vanish. Same
/// class as P145, one level deeper.
pub fn test_validate_question_keys(h: &mut Harness) {
    let edge = json!({"condition": {}, "default": "STOP"});
    let action = json!({"kind": "copy_keys", "keys": ["q"]});
    let ok = |qs: Value| {
        let n = json!({"name": "a", "primary_q": "q", "questions": qs,
                       "edge": edge.clone(), "action": action.clone()});
        spec::from_spec(&base_spec("a", json!([n]))).is_ok()
    };

    // positives — recognised question and heuristic keys
    h.check("validate: a fully declared choice question is OK", ok(json!({"q": {
        "type": "choice", "instructions": "?", "criteria": {"A": "a", "B": "b"},
        "heuristic": {"field": "text", "match_any": ["x"], "p_hit": 0.9, "p_miss": 0.1, "fallback": "B"}}})));
    h.check("validate: question.violating is OK", ok(json!({"q": {
        "type": "choice", "instructions": "?", "criteria": {"A": "a", "B": "b"}, "violating": ["B"]}})));
    h.check("validate: question.truthy/falsy are OK", ok(json!({"q": {
        "type": "noul", "instructions": "?", "truthy": "yes", "falsy": "no"}})));
    h.check("validate: heuristic.answer is OK", ok(json!({"q": {
        "type": "choice", "instructions": "?", "criteria": {"A": "a", "B": "b"}, "heuristic": {"answer": "A"}}})));

    // negatives — unrecognised question keys
    h.check("validate: question key typo `critera` rejected", !ok(json!({"q": {
        "type": "choice", "instructions": "?", "criteria": {"A": "a", "B": "b"}, "critera": {"A": "a"}}})));
    h.check("validate: a stray question key is rejected", !ok(json!({"q": {
        "type": "choice", "instructions": "?", "criteria": {"A": "a", "B": "b"}, "notes": "x"}})));

    // negatives — unrecognised heuristic keys
    h.check("validate: heuristic key typo `answer_feild` rejected", !ok(json!({"q": {
        "type": "choice", "instructions": "?", "criteria": {"A": "a", "B": "b"},
        "heuristic": {"answer_field": "text", "answer_feild": "x"}}})));
    h.check("validate: heuristic key typo `matc_any` rejected", !ok(json!({"q": {
        "type": "choice", "instructions": "?", "criteria": {"A": "a", "B": "b"},
        "heuristic": {"match_any": ["x"], "matc_any": ["y"]}}})));
    h.check("validate: a stray heuristic key is rejected", !ok(json!({"q": {
        "type": "choice", "instructions": "?", "criteria": {"A": "a", "B": "b"},
        "heuristic": {"field": "text", "weights": [1, 2]}}})));
}


/// P147: an action object, its `rules[]` / `dimensions[]` and its `chain[]`
/// steps are read by name too. A misspelled `capabilty` / `rulez` / `dimenions`
/// / `wieght` is silently ignored: `validate` is green yet the action loses its
/// capability / rules / dimensions. Same class as P145/P146, one level deeper.
pub fn test_validate_action_keys(h: &mut Harness) {
    let edge = json!({"condition": {}, "default": "STOP"});
    let qs = json!({"q": {"type": "choice", "instructions": "?", "criteria": {"A": "a"}}});
    let ok = |action: Value| {
        let n = json!({"name": "a", "primary_q": "q", "questions": qs.clone(),
                       "edge": edge.clone(), "action": action});
        spec::from_spec(&base_spec("a", json!([n]))).is_ok()
    };

    // positives — the keys each kind actually reads
    h.check("validate: a threshold action with rules is OK", ok(json!({"kind": "threshold", "rules": [{"when": {"always": true}, "label": "X"}]})));
    h.check("validate: a rubric action with dimensions is OK", ok(json!({"kind": "rubric", "min_confidence": 0.8, "dimensions": [{"question": "q", "levels": 3, "weight": 1.0}]})));
    h.check("validate: a ui_proposal action is OK", ok(json!({"kind": "ui_proposal", "operation": "q", "field": "q", "amount": "q", "surface": "surface", "snapshot_hash": "snapshot_hash", "min_confidence": 0.8, "value_key": "k"})));
    h.check("validate: a copy_keys action is OK", ok(json!({"kind": "copy_keys", "keys": ["q"]})));

    // negatives — a stray / mistyped key on the action object
    h.check("validate: action key typo `capabilty` rejected", !ok(json!({"kind": "copy_keys", "keys": ["q"], "capabilty": "x"})));
    h.check("validate: a stray action key is rejected", !ok(json!({"kind": "threshold", "rules": [{"when": {"always": true}, "label": "X"}], "note": "x"})));

    // negatives — the nested rule / dimension / chain-step objects
    h.check("validate: threshold rule key typo `labl` rejected", !ok(json!({"kind": "threshold", "rules": [{"when": {"always": true}, "labl": "X"}]})));
    h.check("validate: rubric dimension key typo `wieght` rejected", !ok(json!({"kind": "rubric", "dimensions": [{"question": "q", "levels": 3, "wieght": 1.0}]})));
    h.check("validate: chain step key typo `capabilty` rejected", !ok(json!({"kind": "call", "capability": "c", "chain": [{"capabilty": "d", "as": "d"}]})));
}


/// P148: the spec object (and its `policy` / `_bdd` sub-objects) is read by name
/// too. A misspelled `satrt` / `policiy` / `allow_exex` is silently ignored:
/// `validate` is green yet the workflow starts elsewhere / the policy never
/// applies. Same class as P142-P147, now at the spec level.
pub fn test_validate_spec_keys(h: &mut Harness) {
    let nodes = json!([node(
        "a",
        json!({"q": {"type": "choice", "instructions": "?", "criteria": {"A": "a"}}}),
        json!({"condition": {}, "default": "STOP"}),
        json!({"kind": "copy_keys", "keys": ["q"]})
    )]);
    let ok = |spec: Value| spec::from_spec(&spec).is_ok();

    // positives — the keys the engine actually reads
    h.check("validate: a well-formed spec is OK", ok(base_spec("a", nodes.clone())));
    let mut with_policy = base_spec("a", nodes.clone());
    with_policy["policy"] = json!({"allow_exec": false, "allow_hosts": [], "allow_paths": [],
        "max_timeout_ms": 1000, "max_output": 1024, "retries": 0});
    h.check("validate: a fully declared policy is OK", ok(with_policy));
    let mut with_bdd = base_spec("a", nodes.clone());
    with_bdd["_bdd"] = json!({"feature": "f", "feature_path": "p", "mode": "decision"});
    h.check("validate: compiler `_bdd` metadata is OK", ok(with_bdd));

    // negatives — a stray / mistyped top-level key
    let mut typo = base_spec("a", nodes.clone());
    typo["satrt"] = json!("a");
    h.check("validate: spec key typo `satrt` rejected", !ok(typo));

    let mut stray = base_spec("a", nodes.clone());
    stray["title"] = json!("x");
    h.check("validate: a stray spec key is rejected", !ok(stray));

    // negatives — the `policy` / `_bdd` sub-objects
    let mut badpol = base_spec("a", nodes.clone());
    badpol["policy"] = json!({"allow_exex": true});
    h.check("validate: policy key typo `allow_exex` rejected", !ok(badpol));

    let mut badbdd = base_spec("a", nodes.clone());
    badbdd["_bdd"] = json!({"featur": "f"});
    h.check("validate: `_bdd` key typo `featur` rejected", !ok(badbdd));

    // ── P168: a policy value's JSON type must be the one the engine reads ────
    // The engine reads every policy key leniently (`as_bool` / `as_array` /
    // `as_u64`) and silently keeps its default on the wrong type, so a policy
    // that declares `"allow_exec": "yes"` runs with exec *off* and one that
    // declares `"allow_hosts": "a.com"` runs with *no* host restriction.
    let pol = |v: Value| {
        let mut s = base_spec("a", nodes.clone());
        s["policy"] = v;
        s
    };
    let pol_err = |v: Value| -> String {
        match spec::from_spec(&pol(v)) {
            Ok(_) => String::new(),
            Err(e) => format!("{e:#}"),
        }
    };
    let m = pol_err(json!({"allow_exec": "yes"}));
    h.check(
        "validate: policy allow_exec as a string rejected (was silently default false)",
        !m.is_empty(),
    );
    h.check("validate: the allow_exec error names the key", m.contains("allow_exec"));
    h.check("validate: the allow_exec error says boolean", m.contains("boolean"));
    h.check("validate: policy allow_exec true OK", ok(pol(json!({"allow_exec": true}))));
    h.check("validate: policy allow_exec false OK", ok(pol(json!({"allow_exec": false}))));
    let m = pol_err(json!({"allow_hosts": "a.com"}));
    h.check(
        "validate: policy allow_hosts as a string rejected (was silently [])",
        !m.is_empty() && m.contains("allow_hosts") && m.contains("array"),
    );
    h.check(
        "validate: policy allow_hosts with a non-string element rejected",
        !ok(pol(json!({"allow_hosts": ["a.com", 5]}))),
    );
    h.check(
        "validate: policy allow_hosts list of strings OK",
        ok(pol(json!({"allow_hosts": ["a.com", "b.com"]}))),
    );
    h.check("validate: policy allow_hosts empty list OK", ok(pol(json!({"allow_hosts": []}))));
    h.check(
        "validate: policy max_timeout_ms as a string rejected",
        !ok(pol(json!({"max_timeout_ms": "60000"}))),
    );
    h.check("validate: policy retries negative rejected", !ok(pol(json!({"retries": -1}))));
    h.check("validate: policy retries 0 OK", ok(pol(json!({"retries": 0}))));
    h.check(
        "validate: policy allow_paths array of strings OK",
        ok(pol(json!({"allow_paths": ["/tmp"]}))),
    );
}


/// P149: a capability *definition* must be runnable. P128 refuses options a kind
/// never reads; this refuses a kind whose mandatory target field is missing —
/// `rpc` without `url`, `s3` without `bucket`, `tcp` without `port`, … still
/// `validate` green, but the runtime only `expand`s *into* the configured string
/// (an empty template stays empty), so every call bails `"<kind> needs …"`. The
/// fields a call may instead supply via `with` (`pdf.path`, `db.sqlite`,
/// `cron.expr`, `computer_use.allowed_surface`) stay legal empty.
pub fn test_validate_capability_runnable(h: &mut Harness) {
    let nodes = json!([node(
        "a",
        json!({"q": {"type": "choice", "instructions": "?", "criteria": {"A": "a"}}}),
        json!({"condition": {}, "default": "STOP"}),
        json!({"kind": "copy_keys", "keys": ["q"]})
    )]);
    let ok = |cap: Value| {
        let mut s = base_spec("a", nodes.clone());
        s["capabilities"] = json!({ "c": cap });
        spec::from_spec(&s).is_ok()
    };

    // positives — a definition that carries its target field runs
    h.check("validate: rpc with url is OK", ok(json!({"kind": "rpc", "url": "http://x"})));
    h.check("validate: s3 with endpoint+bucket is OK", ok(json!({"kind": "s3", "endpoint": "http://x", "bucket": "b"})));
    h.check("validate: tcp with host+port is OK", ok(json!({"kind": "tcp", "host": "127.0.0.1", "port": 9})));
    h.check("validate: mcp http with url+tool is OK", ok(json!({"kind": "mcp", "transport": "http", "url": "http://x", "tool": "t"})));
    h.check("validate: agent stdio with command is OK", ok(json!({"kind": "agent", "transport": "stdio", "command": ["x"]})));

    // negatives — the definition can never run (runtime bails `... needs ...`)
    h.check("validate: rpc without url rejected", !ok(json!({"kind": "rpc"})));
    h.check("validate: graphql without url rejected", !ok(json!({"kind": "graphql"})));
    h.check("validate: llm without url rejected", !ok(json!({"kind": "llm"})));
    h.check("validate: s3 without bucket rejected", !ok(json!({"kind": "s3", "endpoint": "http://x"})));
    h.check("validate: tcp without port rejected", !ok(json!({"kind": "tcp", "host": "127.0.0.1"})));
    h.check("validate: mcp http without url rejected", !ok(json!({"kind": "mcp", "transport": "http", "tool": "t"})));
    h.check("validate: mcp without tool rejected", !ok(json!({"kind": "mcp", "transport": "http", "url": "http://x"})));
    h.check("validate: agent http without url rejected", !ok(json!({"kind": "agent", "transport": "http"})));
    h.check("validate: sqlite without db rejected", !ok(json!({"kind": "sqlite"})));
    h.check("validate: web_search without endpoint rejected", !ok(json!({"kind": "web_search"})));

    // boundary — fields the *call* may supply via `with` stay legal empty
    h.check("validate: pdf without path is OK (with supplies it)", ok(json!({"kind": "pdf"})));
    h.check("validate: db without sqlite is OK (with supplies it)", ok(json!({"kind": "db"})));
    h.check("validate: cron without expr is OK (with supplies it)", ok(json!({"kind": "cron"})));
    h.check("validate: computer_use without surface is OK (with supplies it)", ok(json!({"kind": "computer_use"})));

    // the message names the missing option and why
    let mut s = base_spec("a", nodes.clone());
    s["capabilities"] = json!({"c": {"kind": "rpc"}});
    match spec::from_spec(&s) {
        Err(e) => {
            let m = e.to_string();
            h.check("validate: the error names the missing option", m.contains("url"));
            h.check("validate: the error explains the run failure", m.contains("fails at run"));
        }
        Ok(_) => h.check("validate: expected an error", false),
    }
}


/// P150: a capability option whose *value* the parser throws away is the same
/// bug as a key it never reads. A present `timeout_ms` that is not a
/// non-negative integer (`-1`, `1.5`, `"9000"`) is silently defaulted, so the
/// capability runs with a timeout nobody chose; a `port` above 65535 is cast to
/// `u16` and wraps (`70000` -> `4464`), so it quietly dials the wrong port.
pub fn test_validate_option_domains(h: &mut Harness) {
    let nodes = json!([node(
        "a",
        json!({"q": {"type": "choice", "instructions": "?", "criteria": {"A": "a"}}}),
        json!({"condition": {}, "default": "STOP"}),
        json!({"kind": "copy_keys", "keys": ["q"]})
    )]);
    let ok = |cap: Value| {
        let mut s = base_spec("a", nodes.clone());
        s["capabilities"] = json!({ "c": cap });
        spec::from_spec(&s).is_ok()
    };

    // positives — in-domain values
    h.check("validate: tcp port 1 is OK", ok(json!({"kind": "tcp", "host": "h", "port": 1})));
    h.check("validate: tcp port 65535 is OK", ok(json!({"kind": "tcp", "host": "h", "port": 65535})));
    h.check("validate: redis port 6379 is OK", ok(json!({"kind": "redis", "port": 6379})));
    h.check("validate: smtp port 25 is OK", ok(json!({"kind": "smtp", "host": "h", "from": "a@b"})));
    h.check("validate: http timeout_ms 1000 is OK", ok(json!({"kind": "http", "url": "http://x", "timeout_ms": 1000})));
    h.check("validate: computer_use min_confidence 0.9 is OK", ok(json!({"kind": "computer_use", "allowed_surface": "s", "min_confidence": 0.9})));

    // negatives — out-of-domain values that silently wrap / default
    h.check("validate: tcp port 70000 rejected", !ok(json!({"kind": "tcp", "host": "h", "port": 70000})));
    h.check("validate: redis port 99999 rejected", !ok(json!({"kind": "redis", "port": 99999})));
    h.check("validate: http timeout_ms -1 rejected", !ok(json!({"kind": "http", "url": "http://x", "timeout_ms": -1})));
    h.check("validate: exec timeout_ms 1.5 rejected", !ok(json!({"kind": "exec", "argv": ["x"], "timeout_ms": 1.5})));
    h.check("validate: llm max_tokens string rejected", !ok(json!({"kind": "llm", "url": "http://x", "max_tokens": "256"})));
    h.check("validate: computer_use min_confidence string rejected", !ok(json!({"kind": "computer_use", "allowed_surface": "s", "min_confidence": "0.9"})));

    // the messages name the constraint
    let mut s = base_spec("a", nodes.clone());
    s["capabilities"] = json!({"c": {"kind": "tcp", "host": "h", "port": 70000}});
    if let Err(e) = spec::from_spec(&s) {
        let m = e.to_string();
        h.check("validate: the port error names the range", m.contains("1..=65535"));
        h.check("validate: the port error explains the wrap", m.contains("u16"));
    } else {
        h.check("validate: expected a port error", false);
    }

    let mut s2 = base_spec("a", nodes.clone());
    s2["capabilities"] = json!({"c": {"kind": "http", "url": "http://x", "timeout_ms": -1}});
    if let Err(e) = spec::from_spec(&s2) {
        h.check("validate: the timeout error says non-negative integer", e.to_string().contains("non-negative integer"));
    } else {
        h.check("validate: expected a timeout error", false);
    }
}

/// P151: a *string* option whose value falls outside the closed set the runtime
/// accepts is the same "a declared value the engine throws away" bug as P128
/// (unread key) / P149 (missing target) / P150 (numeric domain). The runtime's
/// dispatch accepts only a fixed set (`op`, `mode`, `driver`, `format`,
/// `transport`, `channel`, `method`, `hash`), so a value outside it is never
/// honoured — the call bails (`text op "len" unsupported …`) or a default is
/// silently substituted. `validate` refuses it instead (`check_option_values`),
/// mirroring the engine's own empty-string handling via `allow_empty`.
pub fn test_validate_option_values(h: &mut Harness) {
    let nodes = json!([node(
        "a",
        json!({"q": {"type": "choice", "instructions": "?", "criteria": {"A": "a"}}}),
        json!({"condition": {}, "default": "STOP"}),
        json!({"kind": "copy_keys", "keys": ["q"]})
    )]);
    let ok = |cap: Value| {
        let mut s = base_spec("a", nodes.clone());
        s["capabilities"] = json!({ "c": cap });
        spec::from_spec(&s).is_ok()
    };

    // positives — a member of each kind's closed set
    h.check("validate: db op analytics OK", ok(json!({"kind": "db", "op": "analytics"})));
    h.check("validate: db mode server OK", ok(json!({"kind": "db", "mode": "server"})));
    h.check("validate: db format csv OK", ok(json!({"kind": "db", "format": "csv"})));
    h.check("validate: datetime op format OK", ok(json!({"kind": "datetime", "op": "format"})));
    h.check("validate: text op hash OK", ok(json!({"kind": "text", "op": "hash"})));
    h.check("validate: hash op crc32 OK", ok(json!({"kind": "hash", "op": "crc32"})));
    h.check("validate: math op stats OK", ok(json!({"kind": "math", "op": "stats"})));
    h.check("validate: http method get (lowercase) OK", ok(json!({"kind": "http", "url": "http://x", "method": "get"})));
    h.check("validate: computer_use op verify OK", ok(json!({"kind": "computer_use", "op": "verify"})));
    h.check("validate: notify channel macos OK", ok(json!({"kind": "notify", "channel": "macos"})));
    h.check("validate: sql driver psql OK", ok(json!({"kind": "sql", "dsn": "x", "driver": "psql"})));
    h.check("validate: vector op upsert OK", ok(json!({"kind": "vector", "url": "http://x", "op": "upsert"})));
    h.check("validate: queue op peek OK", ok(json!({"kind": "queue", "op": "peek"})));
    h.check("validate: needle op embed OK", ok(json!({"kind": "needle", "op": "embed"})));
    h.check("validate: mcp transport stdio OK", ok(json!({"kind": "mcp", "tool": "t", "transport": "stdio", "command": ["x"]})));

    // negatives — a value outside the set is never honoured
    h.check("validate: db op qurey rejected", !ok(json!({"kind": "db", "op": "qurey"})));
    h.check("validate: db mode cluster rejected", !ok(json!({"kind": "db", "mode": "cluster"})));
    h.check("validate: db format yaml rejected", !ok(json!({"kind": "db", "format": "yaml"})));
    h.check("validate: datetime op todate rejected", !ok(json!({"kind": "datetime", "op": "todate"})));
    h.check("validate: text op len rejected", !ok(json!({"kind": "text", "op": "len"})));
    h.check("validate: text hash md5 rejected", !ok(json!({"kind": "text", "hash": "md5"})));
    h.check("validate: file op cat rejected", !ok(json!({"kind": "file", "op": "cat"})));
    h.check("validate: json op flat rejected", !ok(json!({"kind": "json", "op": "flat"})));
    h.check("validate: csv op gen rejected", !ok(json!({"kind": "csv", "op": "gen"})));
    h.check("validate: hash op sha1 rejected", !ok(json!({"kind": "hash", "op": "sha1"})));
    h.check("validate: math op mean rejected", !ok(json!({"kind": "math", "op": "mean"})));
    h.check("validate: http method FETCH rejected", !ok(json!({"kind": "http", "url": "http://x", "method": "FETCH"})));
    h.check("validate: computer_use op snap rejected", !ok(json!({"kind": "computer_use", "op": "snap"})));
    h.check("validate: notify channel slack rejected", !ok(json!({"kind": "notify", "channel": "slack"})));
    h.check("validate: sql driver oracle rejected", !ok(json!({"kind": "sql", "dsn": "x", "driver": "oracle"})));
    h.check("validate: vector op delete rejected", !ok(json!({"kind": "vector", "url": "http://x", "op": "delete"})));
    h.check("validate: queue op enqueue rejected", !ok(json!({"kind": "queue", "op": "enqueue"})));
    h.check("validate: needle op guess rejected", !ok(json!({"kind": "needle", "op": "guess"})));
    h.check("validate: archive op zip rejected", !ok(json!({"kind": "archive", "op": "zip"})));
    h.check("validate: sqlite op exec2 rejected", !ok(json!({"kind": "sqlite", "op": "exec2"})));

    // an alias folds to the canonical kind (`database` -> sql, `md` -> markdown)
    h.check("validate: database (alias) driver psql OK", ok(json!({"kind": "database", "dsn": "x", "driver": "psql"})));
    h.check("validate: md (alias) op outline OK", ok(json!({"kind": "md", "op": "outline"})));
    h.check("validate: md (alias) op bogus rejected", !ok(json!({"kind": "md", "op": "bogus"})));

    // empty mirrors the engine: accepted where it already resolves to a member,
    // rejected where the runtime would bail
    h.check("validate: db op empty OK (engine defaults to query)", ok(json!({"kind": "db", "op": ""})));
    h.check("validate: text hash empty OK (engine defaults to fnv1a64)", ok(json!({"kind": "text", "hash": ""})));
    h.check("validate: notify channel empty OK (engine defaults to log)", ok(json!({"kind": "notify", "channel": ""})));
    h.check("validate: text op empty rejected (engine would bail)", !ok(json!({"kind": "text", "op": ""})));
    h.check("validate: http method empty rejected (engine would bail)", !ok(json!({"kind": "http", "url": "http://x", "method": ""})));
    h.check("validate: mcp transport empty rejected (engine would bail)", !ok(json!({"kind": "mcp", "tool": "t", "transport": "", "url": "http://x", "command": ["x"]})));

    // a present option of the wrong type is silently dropped, like P150's numbers
    h.check("validate: text op as number rejected", !ok(json!({"kind": "text", "op": 5})));

    // the messages name the offending value, the accepted set and the reason
    let mut s = base_spec("a", nodes.clone());
    s["capabilities"] = json!({"c": {"kind": "text", "op": "len"}});
    if let Err(e) = spec::from_spec(&s) {
        let m = e.to_string();
        h.check("validate: the value error names the value", m.contains("\"len\""));
        h.check("validate: the value error names the accepted set", m.contains("\"length\""));
        h.check("validate: the value error explains the silent-substitute/bail", m.contains("never honoured"));
    } else {
        h.check("validate: expected a value error", false);
    }

    let mut s2 = base_spec("a", nodes.clone());
    s2["capabilities"] = json!({"c": {"kind": "text", "op": 5}});
    if let Err(e) = spec::from_spec(&s2) {
        h.check("validate: the non-string error says must be a string", e.to_string().contains("must be a string"));
    } else {
        h.check("validate: expected a non-string error", false);
    }
}

/// Regression (P152): a capability option the engine reads as a boolean must be
/// a real JSON boolean — `"false"` / `0` / `"yes"` are silently replaced by the
/// option's default (the `"false"`-is-truthy footgun). And an out-of-set value
/// for a *silent-fallback* string option (`pdf.op` → `text`,
/// `web_fetch.format` → `text`, `web_search.method` → `GET`) is refused too,
/// because the engine substitutes a default instead of bailing — the typo would
/// run to completion with the wrong behaviour.
pub fn test_validate_option_values_2(h: &mut Harness) {
    let nodes = json!([node(
        "a",
        json!({"q": {"type": "choice", "instructions": "?", "criteria": {"A": "a"}}}),
        json!({"condition": {}, "default": "STOP"}),
        json!({"kind": "copy_keys", "keys": ["q"]})
    )]);
    let ok = |cap: Value| {
        let mut s = base_spec("a", nodes.clone());
        s["capabilities"] = json!({ "c": cap });
        spec::from_spec(&s).is_ok()
    };

    // positives — a real JSON boolean on a boolean option is accepted
    h.check("validate: http expect_json true OK", ok(json!({"kind": "http", "url": "http://x", "expect_json": true})));
    h.check("validate: http allow_error_status false OK", ok(json!({"kind": "http", "url": "http://x", "allow_error_status": false})));
    h.check("validate: sqlite readonly false OK", ok(json!({"kind": "sqlite", "db": "/tmp/x.db", "readonly": false})));
    h.check("validate: browser human true OK", ok(json!({"kind": "browser", "human": true})));
    h.check("validate: browser launch false OK", ok(json!({"kind": "browser", "launch": false})));
    h.check("validate: csv headers false OK", ok(json!({"kind": "csv", "headers": false})));
    h.check("validate: notify bell true OK", ok(json!({"kind": "notify", "bell": true})));
    h.check("validate: web_fetch body_urls_array true OK", ok(json!({"kind": "web_fetch", "body_urls_array": true})));

    // positives — a member of each silent-fallback set
    h.check("validate: pdf op info OK", ok(json!({"kind": "pdf", "path": "/tmp/x.pdf", "op": "info"})));
    h.check("validate: web_fetch format markdown OK", ok(json!({"kind": "web_fetch", "format": "markdown"})));
    h.check("validate: web_search method GET OK", ok(json!({"kind": "web_search", "endpoint": "http://x", "method": "GET"})));
    h.check("validate: web_search method get (lowercase) OK", ok(json!({"kind": "web_search", "endpoint": "http://x", "method": "get"})));

    // negatives — a non-boolean on a boolean option is silently defaulted
    h.check("validate: expect_json as string \"false\" rejected", !ok(json!({"kind": "http", "url": "http://x", "expect_json": "false"})));
    h.check("validate: expect_json as 0 rejected", !ok(json!({"kind": "http", "url": "http://x", "expect_json": 0})));
    h.check("validate: expect_json as 1 rejected", !ok(json!({"kind": "http", "url": "http://x", "expect_json": 1})));
    h.check("validate: sqlite readonly as \"false\" rejected", !ok(json!({"kind": "sqlite", "db": "/tmp/x.db", "readonly": "false"})));
    h.check("validate: browser human as \"yes\" rejected", !ok(json!({"kind": "browser", "human": "yes"})));
    h.check("validate: csv headers as 0 rejected", !ok(json!({"kind": "csv", "headers": 0})));
    h.check("validate: notify bell as \"true\" rejected", !ok(json!({"kind": "notify", "bell": "true"})));

    // negatives — a value outside a silent-fallback set is refused
    h.check("validate: pdf op txt rejected (falls back to text)", !ok(json!({"kind": "pdf", "path": "/tmp/x.pdf", "op": "txt"})));
    h.check("validate: web_fetch format html rejected (falls back to text)", !ok(json!({"kind": "web_fetch", "format": "html"})));
    h.check("validate: web_search method PUT rejected (falls back to GET)", !ok(json!({"kind": "web_search", "endpoint": "http://x", "method": "PUT"})));

    // the messages name the option and the reason
    let mut s = base_spec("a", nodes.clone());
    s["capabilities"] = json!({"c": {"kind": "http", "url": "http://x", "expect_json": "false"}});
    if let Err(e) = spec::from_spec(&s) {
        let m = e.to_string();
        h.check("validate: the bool error says must be true or false", m.contains("must be true or false"));
        h.check("validate: the bool error names the option", m.contains("expect_json"));
    } else {
        h.check("validate: expected a bool-type error", false);
    }

    let mut s2 = base_spec("a", nodes.clone());
    s2["capabilities"] = json!({"c": {"kind": "web_fetch", "format": "html"}});
    if let Err(e) = spec::from_spec(&s2) {
        let m = e.to_string();
        h.check("validate: the fallback error says silently falls back", m.contains("silently falls back to"));
        h.check("validate: the fallback error names the accepted set", m.contains("markdown"));
    } else {
        h.check("validate: expected a fallback-value error", false);
    }
}

/// Regression (P153): the last "a declared value the engine throws away" holes
/// on a capability option —
///  * `metrics.what` / `sysinfo.what` is a comma-separated subset of a fixed
///    vocabulary; an unknown token is silently dropped (`want.contains` never
///    matches it), so the metric you asked for is missing from the result;
///  * `csv.delimiter` is read via `chars().next()`, so a multi-character value
///    is silently truncated to its first character;
///  * `goal_runner.runner` is a fixed two-entry set; an unknown name makes the
///    run bail (`resolve_runner`), so validate refuses it here.
pub fn test_validate_option_values_3(h: &mut Harness) {
    let nodes = json!([node(
        "a",
        json!({"q": {"type": "choice", "instructions": "?", "criteria": {"A": "a"}}}),
        json!({"condition": {}, "default": "STOP"}),
        json!({"kind": "copy_keys", "keys": ["q"]})
    )]);
    let ok = |cap: Value| {
        let mut s = base_spec("a", nodes.clone());
        s["capabilities"] = json!({ "c": cap });
        spec::from_spec(&s).is_ok()
    };

    // positives — a member of each closed set / a valid list / a single char
    h.check("validate: metrics what mem,load,disk OK", ok(json!({"kind": "metrics", "what": "mem,load,disk"})));
    h.check("validate: metrics what single mem OK", ok(json!({"kind": "metrics", "what": "mem"})));
    h.check("validate: metrics what empty OK (engine defaults)", ok(json!({"kind": "metrics", "what": ""})));
    h.check("validate: metrics what with whitespace OK", ok(json!({"kind": "metrics", "what": " mem , load "})));
    h.check("validate: sysinfo (alias) what cpu OK", ok(json!({"kind": "sysinfo", "what": "cpu"})));
    h.check("validate: goal_runner runner cxgo OK", ok(json!({"kind": "goal_runner", "runner": "cxgo"})));
    h.check("validate: goal_runner runner cmdgo OK", ok(json!({"kind": "goal_runner", "runner": "cmdgo"})));
    h.check("validate: goal_runner runner empty OK (engine defaults to cxgo)", ok(json!({"kind": "goal_runner", "runner": ""})));
    h.check("validate: agent_goal (alias) runner cxgo OK", ok(json!({"kind": "agent_goal", "runner": "cxgo"})));
    h.check("validate: csv delimiter | OK", ok(json!({"kind": "csv", "delimiter": "|"})));
    h.check("validate: csv delimiter ; OK", ok(json!({"kind": "csv", "delimiter": ";"})));
    h.check("validate: csv delimiter tab OK", ok(json!({"kind": "csv", "delimiter": "\t"})));
    h.check("validate: csv delimiter empty OK (engine defaults to ',')", ok(json!({"kind": "csv", "delimiter": ""})));
    h.check("validate: csv delimiter multibyte single char OK", ok(json!({"kind": "csv", "delimiter": "╎"})));

    // negatives — an unknown token is silently dropped / a multi-char truncates
    h.check("validate: metrics what momery rejected", !ok(json!({"kind": "metrics", "what": "momery"})));
    h.check("validate: metrics what mem,momery rejected", !ok(json!({"kind": "metrics", "what": "mem,momery"})));
    h.check("validate: metrics what cupy rejected", !ok(json!({"kind": "metrics", "what": "cupy"})));
    h.check("validate: sysinfo (alias) what bad rejected", !ok(json!({"kind": "sysinfo", "what": "momery"})));
    h.check("validate: goal_runner runner bogus rejected", !ok(json!({"kind": "goal_runner", "runner": "bogus"})));
    h.check("validate: agent_goal (alias) runner bad rejected", !ok(json!({"kind": "agent_goal", "runner": "bogus"})));
    h.check("validate: csv delimiter || rejected (truncates)", !ok(json!({"kind": "csv", "delimiter": "||"})));
    h.check("validate: csv delimiter tab,tab rejected (truncates)", !ok(json!({"kind": "csv", "delimiter": "\t\t"})));

    // a non-string value is silently dropped, like P151/P152
    h.check("validate: metrics what as number rejected", !ok(json!({"kind": "metrics", "what": 5})));
    h.check("validate: csv delimiter as number rejected", !ok(json!({"kind": "csv", "delimiter": 9})));

    // the messages name the offending value and the reason
    let mut s = base_spec("a", nodes.clone());
    s["capabilities"] = json!({"c": {"kind": "metrics", "what": "mem,momery"}});
    if let Err(e) = spec::from_spec(&s) {
        let m = e.to_string();
        h.check("validate: the metrics error names the bad token", m.contains("\"momery\""));
        h.check("validate: the metrics error says silently dropped", m.contains("silently dropped"));
    } else {
        h.check("validate: expected a metrics-list error", false);
    }

    let mut s2 = base_spec("a", nodes.clone());
    s2["capabilities"] = json!({"c": {"kind": "csv", "delimiter": "||"}});
    if let Err(e) = spec::from_spec(&s2) {
        let m = e.to_string();
        h.check("validate: the delimiter error says first character", m.contains("keeps only the first character"));
        h.check("validate: the delimiter error counts the chars", m.contains("2 characters"));
    } else {
        h.check("validate: expected a delimiter error", false);
    }
}

/// Regression (P154): the last "a declared value the engine throws away" hole —
/// an option the engine reads as a `Vec<String>` (from a JSON array, keeping
/// only the string elements) or as a JSON object. A non-string array element
/// (`["--max-turns", 8]` — an unquoted number) or a non-array value is silently
/// dropped; a non-object `headers`/`env` drops the whole map. `validate` now
/// refuses both.
pub fn test_validate_option_values_4(h: &mut Harness) {
    let nodes = json!([node(
        "a",
        json!({"q": {"type": "choice", "instructions": "?", "criteria": {"A": "a"}}}),
        json!({"condition": {}, "default": "STOP"}),
        json!({"kind": "copy_keys", "keys": ["q"]})
    )]);
    let ok = |cap: Value| {
        let mut s = base_spec("a", nodes.clone());
        s["capabilities"] = json!({ "c": cap });
        spec::from_spec(&s).is_ok()
    };

    // positives — every element a string, or a real JSON object
    h.check("validate: exec argv all strings OK", ok(json!({"kind": "exec", "argv": ["echo", "A", "5", "B"]})));
    h.check("validate: exec argv empty OK", ok(json!({"kind": "exec", "argv": []})));
    h.check("validate: goal_runner args all strings OK", ok(json!({"kind": "goal_runner", "args": ["--max-turns", "8", "--fresh"]})));
    h.check("validate: agent command all strings (http) OK", ok(json!({"kind": "agent", "transport": "http", "url": "http://x", "command": ["cmd", "-x"]})));
    h.check("validate: mcp command all strings (stdio) OK", ok(json!({"kind": "mcp", "tool": "t", "transport": "stdio", "command": ["cmd"]})));
    h.check("validate: computer_use allowed_fields all strings OK", ok(json!({"kind": "computer_use", "allowed_fields": ["a", "b"]})));
    h.check("validate: file allow_roots all strings OK", ok(json!({"kind": "file", "allow_roots": ["/tmp"]})));
    h.check("validate: http headers object OK", ok(json!({"kind": "http", "url": "http://x", "headers": {"accept": "application/json"}})));
    h.check("validate: http headers empty object OK", ok(json!({"kind": "http", "url": "http://x", "headers": {}})));
    h.check("validate: exec env object OK", ok(json!({"kind": "exec", "argv": ["x"], "env": {"A": "1"}})));
    h.check("validate: object_store (alias) headers object OK", ok(json!({"kind": "object_store", "endpoint": "http://x", "bucket": "b", "headers": {"a": "1"}})));

    // negatives — a non-string element is silently dropped
    h.check("validate: exec argv with a number rejected", !ok(json!({"kind": "exec", "argv": ["echo", "hi", 5]})));
    h.check("validate: exec argv with a bool rejected", !ok(json!({"kind": "exec", "argv": ["echo", true]})));
    h.check("validate: goal_runner args with a number rejected", !ok(json!({"kind": "goal_runner", "args": ["--max-turns", 8]})));
    h.check("validate: goal_runner args non-array rejected", !ok(json!({"kind": "goal_runner", "args": "--max-turns 8"})));
    h.check("validate: agent command with a number rejected", !ok(json!({"kind": "agent", "transport": "http", "url": "http://x", "command": ["cmd", 3]})));
    h.check("validate: mcp command with a bool rejected", !ok(json!({"kind": "mcp", "tool": "t", "transport": "stdio", "command": ["cmd", true]})));
    h.check("validate: computer_use allowed_fields with a number rejected", !ok(json!({"kind": "computer_use", "allowed_fields": ["a", 1]})));
    h.check("validate: file allow_roots with a number rejected", !ok(json!({"kind": "file", "allow_roots": [1]})));

    // negatives — a non-object container drops the whole map
    h.check("validate: http headers as array rejected", !ok(json!({"kind": "http", "url": "http://x", "headers": ["x"]})));
    h.check("validate: http headers as string rejected", !ok(json!({"kind": "http", "url": "http://x", "headers": "accept: x"})));
    h.check("validate: exec env as array rejected", !ok(json!({"kind": "exec", "argv": ["x"], "env": ["A=1"]})));
    h.check("validate: object_store (alias) headers as array rejected", !ok(json!({"kind": "object_store", "endpoint": "http://x", "bucket": "b", "headers": [1]})));

    // the messages name the element / the reason
    let mut s = base_spec("a", nodes.clone());
    s["capabilities"] = json!({"c": {"kind": "goal_runner", "args": ["--max-turns", 8]}});
    if let Err(e) = spec::from_spec(&s) {
        let m = e.to_string();
        h.check("validate: the array error names the non-string element", m.contains("non-string element 8"));
        h.check("validate: the array error says silently dropped", m.contains("silently dropped"));
    } else {
        h.check("validate: expected an array-element error", false);
    }

    let mut s2 = base_spec("a", nodes.clone());
    s2["capabilities"] = json!({"c": {"kind": "http", "url": "http://x", "headers": ["x"]}});
    if let Err(e) = spec::from_spec(&s2) {
        let m = e.to_string();
        h.check("validate: the object error says must be a JSON object", m.contains("must be a JSON object"));
        h.check("validate: the object error names the option", m.contains("headers"));
    } else {
        h.check("validate: expected an object-shape error", false);
    }
}

/// P156: a `plugin`/`script` capability names its plugin one of three ways --
/// `plugin`/`name` (a search-layer id), `dir` (an explicit path), or a script
/// `entry`. `load_sources` needs at least one; with none of the three the engine
/// has nothing to load, so the capability `validate`s but every call bails
/// "plugin capability needs 'plugin' (a name), 'dir', or a script 'entry'". This
/// is the plugin sibling of P149's "a `rpc` without `url` can never run".
pub fn test_validate_capability_plugin(h: &mut Harness) {
    let nodes = json!([node(
        "a",
        json!({"q": {"type": "choice", "instructions": "?", "criteria": {"A": "a"}}}),
        json!({"condition": {}, "default": "STOP"}),
        json!({"kind": "copy_keys", "keys": ["q"]})
    )]);
    let ok = |cap: Value| {
        let mut s = base_spec("a", nodes.clone());
        s["capabilities"] = json!({ "c": cap });
        spec::from_spec(&s).is_ok()
    };

    // positives — any one of the three identities is enough
    h.check("validate: plugin id is OK", ok(json!({"kind": "plugin", "plugin": "alphaxiv"})));
    h.check("validate: plugin `name` alias is OK", ok(json!({"kind": "plugin", "name": "goofish"})));
    h.check("validate: plugin `dir` is OK", ok(json!({"kind": "script", "dir": "dsl/capabilities/hello.rhai"})));
    h.check("validate: plugin script `entry` is OK", ok(json!({"kind": "plugin", "entry": "run.rhai"})));
    // P159: `plugin` and its `name` alias are the same thing under two names, so
    // a definition that gives both is redundant, not wrong. The parser read the
    // alias with `get("plugin").or_else(|| get("name"))`, which called
    // `get("name")` only when `plugin` was *absent* -- so the P128 "declares
    // option(s) this engine never reads" check never saw it and refused the
    // pair, naming `name` as though it were a typo. Both keys are now read
    // unconditionally and `plugin` still decides.
    h.check(
        "validate: plugin id + `name` alias together OK",
        ok(json!({"kind": "plugin", "plugin": "alphaxiv", "name": "alphaxiv"})),
    );
    h.check(
        "validate: script id + `name` alias together OK",
        ok(json!({"kind": "script", "plugin": "p", "name": "p", "dir": "d.rhai"})),
    );
    h.check(
        "validate: plugin id wins over the `name` alias",
        ok(json!({"kind": "plugin", "plugin": "alphaxiv", "name": "goofish"})),
    );

    // negatives — the definition can never run (runtime bails `... needs ...`)
    h.check("validate: plugin with no identity rejected", !ok(json!({"kind": "plugin"})));
    h.check("validate: script with no identity rejected", !ok(json!({"kind": "script"})));
    h.check(
        "validate: plugin with only unrelated keys rejected",
        !ok(json!({"kind": "plugin", "op": "run", "timeout_ms": 5000})),
    );
    // a present-but-empty identity is the same as absent
    h.check("validate: plugin with empty id rejected", !ok(json!({"kind": "plugin", "plugin": "  "})));
    h.check("validate: plugin with empty dir rejected", !ok(json!({"kind": "plugin", "dir": "", "entry": ""})));

    // the message names the three identities and why it fails at run
    let mut s = base_spec("a", nodes.clone());
    s["capabilities"] = json!({"c": {"kind": "plugin"}});
    match spec::from_spec(&s) {
        Err(e) => {
            let m = e.to_string();
            h.check("validate: the plugin error names `plugin`/`dir`/`entry`", m.contains("plugin") && m.contains("dir") && m.contains("entry"));
            h.check("validate: the plugin error explains the run failure", m.contains("fails at run"));
        }
        Ok(_) => h.check("validate: expected a plugin-identity error", false),
    }
}

/// P159: every option the engine reads has a declared JSON *type*, and the type
/// tables are provably still in step with the parser.
///
/// P150 closed the numeric domain, P151 the closed-set string, P152 the boolean,
/// P153 the comma list / one-character string / fixed runner, P154 the string
/// array and the object container. What was left is the plainest case of all: a
/// **string** option (and a signed-integer one) given a value of some other JSON
/// type. `as_str()` returns `None`, the parser falls back to its default, and
/// `validate` stays green — `"format": 5` on a `datetime` capability quietly
/// means the empty format nobody wrote. Same "a declared value the engine throws
/// away" class, now about the last two JSON shapes: `string` and `i64`.
pub fn test_validate_option_string(h: &mut Harness) {
    let nodes = json!([node(
        "a",
        json!({"q": {"type": "choice", "instructions": "?", "criteria": {"A": "a"}}}),
        json!({"condition": {}, "default": "STOP"}),
        json!({"kind": "copy_keys", "keys": ["q"]})
    )]);
    let ok = |cap: Value| {
        let mut s = base_spec("a", nodes.clone());
        s["capabilities"] = json!({ "c": cap });
        spec::from_spec(&s).is_ok()
    };
    let err_of = |cap: Value| -> String {
        let mut s = base_spec("a", nodes.clone());
        s["capabilities"] = json!({ "c": cap });
        match spec::from_spec(&s) {
            Ok(_) => String::new(),
            Err(e) => format!("{e:#}"),
        }
    };

    // positives — the declared value is the type the engine reads
    h.check(
        "validate: text pattern/replacement/separator strings OK",
        ok(json!({"kind": "text", "op": "replace", "pattern": "a", "replacement": "b", "separator": ","})),
    );
    h.check(
        "validate: datetime format string OK",
        ok(json!({"kind": "datetime", "op": "format", "format": "%Y-%m-%d"})),
    );
    h.check("validate: exec cwd string OK", ok(json!({"kind": "exec", "argv": ["pwd"], "cwd": "/tmp"})));
    h.check(
        "validate: datetime offset_secs integer OK",
        ok(json!({"kind": "datetime", "op": "now", "offset_secs": 3600})),
    );
    h.check(
        "validate: cache ttl_secs integer OK",
        ok(json!({"kind": "cache", "op": "get", "path": "p", "ttl_secs": 60})),
    );
    h.check(
        "validate: sysinfo (alias of metrics) disk_path string OK",
        ok(json!({"kind": "sysinfo", "what": "disk", "disk_path": "/"})),
    );
    h.check(
        "validate: object_store (alias) prefix string OK",
        ok(json!({"kind": "object_store", "endpoint": "http://x", "bucket": "b", "prefix": "logs/"})),
    );
    h.check(
        "validate: fetch_url (alias) text_field string OK",
        ok(json!({"kind": "fetch_url", "endpoint": "http://x", "text_field": "body"})),
    );
    h.check(
        "validate: shell command/cwd strings OK",
        ok(json!({"kind": "shell", "command": "echo hi", "cwd": "/tmp"})),
    );
    h.check("validate: rpc id integer OK", ok(json!({"kind": "rpc", "url": "http://x", "id": 7})));

    // negatives — a non-string is silently discarded by `as_str`
    h.check(
        "validate: text separator as a number rejected",
        !ok(json!({"kind": "text", "op": "split", "separator": 5})),
    );
    h.check(
        "validate: text separator as an array rejected",
        !ok(json!({"kind": "text", "op": "split", "separator": ["\n"]})),
    );
    h.check(
        "validate: datetime format as a number rejected",
        !ok(json!({"kind": "datetime", "op": "format", "format": 5})),
    );
    h.check(
        "validate: exec cwd as a bool rejected",
        !ok(json!({"kind": "exec", "argv": ["pwd"], "cwd": true})),
    );
    h.check(
        "validate: sysinfo disk_path as an array rejected",
        !ok(json!({"kind": "sysinfo", "what": "disk", "disk_path": ["/"]})),
    );
    h.check(
        "validate: object_store prefix as a number rejected",
        !ok(json!({"kind": "object_store", "endpoint": "http://x", "bucket": "b", "prefix": 5})),
    );
    h.check(
        "validate: fetch_url text_field as a number rejected",
        !ok(json!({"kind": "fetch_url", "endpoint": "http://x", "text_field": 5})),
    );
    h.check(
        "validate: shell cwd as a number rejected",
        !ok(json!({"kind": "shell", "command": "echo", "cwd": 5})),
    );
    h.check(
        "validate: plugin name as a number rejected",
        !ok(json!({"kind": "plugin", "plugin": "x", "name": 5})),
    );
    h.check(
        "validate: tcp until as a number rejected",
        !ok(json!({"kind": "tcp", "host": "127.0.0.1", "port": 80, "until": 5})),
    );

    // negatives — a non-integer is silently discarded by `as_i64`
    h.check(
        "validate: datetime offset_secs as a string rejected",
        !ok(json!({"kind": "datetime", "op": "now", "offset_secs": "60"})),
    );
    h.check(
        "validate: cache ttl_secs as a string rejected",
        !ok(json!({"kind": "cache", "op": "get", "path": "p", "ttl_secs": "60"})),
    );
    h.check(
        "validate: rpc id as a string rejected",
        !ok(json!({"kind": "rpc", "url": "http://x", "id": "7"})),
    );

    // the messages name the option, the type, and the silent fallback
    let m = err_of(json!({"kind": "datetime", "op": "format", "format": 5}));
    h.check("validate: the string error names the option", m.contains("\"format\""));
    h.check("validate: the string error says must be a string", m.contains("must be a string"));
    h.check("validate: the string error names the as_str fallback", m.contains("as_str()"));
    let m2 = err_of(json!({"kind": "cache", "op": "get", "path": "p", "ttl_secs": "60"}));
    h.check(
        "validate: the integer error says must be an integer",
        m2.contains("must be an integer"),
    );
    h.check("validate: the integer error names the as_i64 fallback", m2.contains("as_i64()"));

    // A minimal valid definition of `kind`, so probing one option with a value
    // of the wrong type is not short-circuited by a missing required field.
    let probe_extras = |kind: &str| -> Value {
        let mut e = serde_json::Map::new();
        match kind {
            "http" | "rpc" | "graphql" | "llm" | "chat" | "vector" | "webhook" | "sse"
            | "prometheus" | "prom" | "kafka" | "agent" => {
                e.insert("url".into(), json!("http://probe"));
            }
            "mcp" => {
                e.insert("tool".into(), json!("t"));
                e.insert("url".into(), json!("http://probe"));
            }
            "web_search" => {
                e.insert("endpoint".into(), json!("http://probe"));
            }
            "s3" => {
                e.insert("endpoint".into(), json!("http://probe"));
                e.insert("bucket".into(), json!("b"));
            }
            "sql" => {
                e.insert("dsn".into(), json!("sqlite3://p"));
            }
            "sqlite" => {
                e.insert("db".into(), json!("/tmp/p.db"));
            }
            "tcp" | "udp" => {
                e.insert("host".into(), json!("127.0.0.1"));
                e.insert("port".into(), json!(80));
            }
            "exec" => {
                e.insert("argv".into(), json!(["probe"]));
            }
            "shell" => {
                e.insert("command".into(), json!("probe"));
            }
            "plugin" | "script" => {
                e.insert("plugin".into(), json!("probe"));
                e.insert("name".into(), json!("probe"));
                e.insert("dir".into(), json!("probe"));
                e.insert("entry".into(), json!("probe"));
            }
            _ => {}
        }
        Value::Object(e)
    };

    // ── structural guard: the read site is the source of truth (P160) ───────
    // The JSON type an option is read as is now declared *at the read site*, so
    // there is no side table that can drift from the parser. What remains is the
    // opposite risk — a read that bypasses the typed accessors — so this scans
    // the real parser (the flattened text, so a receiver on its own line still
    // counts) and holds four invariants.
    let src = include_str!("../capability/parse.rs");
    let body = &src[src.find("\nfn parse_kind(").expect("parse_kind fn")..];
    let flat = regex_lite::Regex::new(r"\n\s*\.").unwrap().replace_all(body, ".");
    let arm_re =
        regex_lite::Regex::new(r#"(?m)^        ("[^"]+"(?:\s*\|\s*"[^"]+")*)\s*=>"#).unwrap();
    let call_re =
        regex_lite::Regex::new(r#"f\.(any|has|arr|obj|s|u64|i64|f64|b)\("([A-Za-z_0-9]+)"\)"#)
            .unwrap();
    let arms: Vec<(usize, String)> = arm_re
        .captures_iter(&flat)
        .map(|c| {
            let kind = c.get(1).unwrap().as_str();
            (c.get(0).unwrap().start(), kind.split('|').next().unwrap().trim().trim_matches('"').to_string())
        })
        .collect();
    // (kind, key, accessor) for every read in the parser
    let sites: Vec<(String, String, String)> = call_re
        .captures_iter(&flat)
        .map(|c| {
            let at = c.get(0).unwrap().start();
            let acc = c.get(1).unwrap().as_str().to_string();
            let key = c.get(2).unwrap().as_str().to_string();
            let kind = arms
                .iter()
                .rev()
                .find(|(p, _)| *p < at)
                .map(|(_, k)| k.clone())
                .unwrap_or_default();
            (kind, key, acc)
        })
        .collect();

    // 1. every read is an accessor call — no raw `f.get(..)` survives (comments
    //    stripped: the P156 comment quotes the chain it replaced)
    let raw = flat
        .lines()
        .map(|l| match l.find("//") {
            Some(i) => &l[..i],
            None => l,
        })
        .filter(|l| l.contains("f.get("))
        .count();
    h.check(
        &format!("validate: no untyped `f.get(` read left in parse_kind ({raw} found)"),
        raw == 0,
    );

    // 2. `any(k)` skips the type check, so its type must be owned elsewhere: an
    //    opaque JSON value, or the domain check (`port`/`what`/`delimiter`).
    //    Adding an entry here is the deliberate act the guard exists to force.
    //    The closed-set options (`op`, `method`, `format`, `mode`, `hash`,
    //    `channel`, `driver`, `runner`, `transport`) are no longer here — P167
    //    typed them, so the `as_str()` value table can no longer be handed a
    //    non-string and skip the check (a `{"kind":"datetime","op":4}` used to
    //    compile to the default `op:"now"`).
    const JUSTIFIED_UNTYPED: &[&str] = &[
        "body", "expected", "variables", // opaque JSON, consumed as-is
        "port", "what", "delimiter", // owned by check_option_domains
    ];
    let any_n = sites.iter().filter(|(_, _, a)| a == "any").count();
    let unjustified: Vec<String> = sites
        .iter()
        .filter(|(_, k, a)| a == "any" && !JUSTIFIED_UNTYPED.contains(&k.as_str()))
        .map(|(kind, k, _)| format!("{kind}.{k}"))
        .collect();
    h.check(
        &format!("validate: every `.any(..)` read is justified ({any_n} of them)"),
        unjustified.is_empty(),
    );
    //    and the reverse — no justification is left behind for a key that is now
    //    read only through a typed accessor, which would hide a future `any(..)`.
    let mut unused: Vec<String> = Vec::new();
    for k in JUSTIFIED_UNTYPED {
        if !sites.iter().any(|(_, key, a)| a == "any" && key.as_str() == *k) {
            unused.push((*k).to_string());
        }
    }
    h.check(
        &format!("validate: no unused `.any(..)` justification ({unused:?})"),
        unused.is_empty(),
    );

    // 3. `has(k)` is a presence test, so the same key must also be read with a
    //    type somewhere in that arm
    let has_n = sites.iter().filter(|(_, _, a)| a == "has").count();
    let mut unbacked: Vec<String> = Vec::new();
    for (kind, key, acc) in &sites {
        if acc != "has" {
            continue;
        }
        let typed = sites
            .iter()
            .any(|(k2, key2, a2)| k2 == kind && key2 == key && a2 != "any" && a2 != "has");
        if !typed {
            unbacked.push(format!("{kind}.{key}"));
        }
    }
    h.check(
        &format!("validate: every `has(..)` presence test is backed by a typed read ({has_n} of them)"),
        unbacked.is_empty(),
    );

    // 4. behaviour: every typed read site refuses a value of the wrong JSON type,
    //    and its error names that option (the type error does; so does the "needs
    //    <field>" error when the option is required). A `never reads` hint means
    //    the site names an option its kind does not read — drift.
    let mut drifted: Vec<String> = Vec::new();
    let mut probed = 0usize;
    for (kind, opt, acc) in &sites {
        let ty = match acc.as_str() {
            "s" => "string",
            "u64" => "u64",
            "i64" => "i64",
            "f64" => "f64",
            "b" => "bool",
            "arr" => "str_array",
            "obj" => "object",
            _ => continue,
        };
        let wrong = match ty {
            "bool" => json!("false"),
            "str_array" | "object" | "string" => json!(5),
            _ => json!("60"),
        };
        let mut cap = probe_extras(kind);
        cap["kind"] = json!(kind);
        cap[opt.as_str()] = wrong;
        let mut s = base_spec("a", nodes.clone());
        s["capabilities"] = json!({ "c": cap });
        probed += 1;
        match spec::from_spec(&s) {
            Ok(_) => drifted.push(format!("{kind}.{opt} ({ty}) was accepted")),
            Err(e) => {
                let m = format!("{e:#}");
                if m.contains("never reads") {
                    drifted.push(format!("{kind}.{opt} ({ty}): {m}"));
                } else if !m.contains(opt.as_str()) {
                    drifted.push(format!("{kind}.{opt} ({ty}) is not named by its own error: {m}"));
                }
            }
        }
    }
    h.check(
        &format!("validate: every typed read site refuses a wrong-typed value ({probed} probed)"),
        drifted.is_empty(),
    );
    if !drifted.is_empty() {
        for d in drifted.iter().take(8) {
            h.check(&format!("validate: typed-read drift {d}"), false);
        }
    }

    // ── P167: the *definition* side of the same silent no-op ────────────────
    // The `with.<key>` value table (P166) read the option with `as_str()`, so a
    // non-string value silently fell back to the default. P167 typed the read
    // sites of every closed-set option, so the type is refused before the value
    // table ever sees it. A `{"kind":"datetime","op":4}` used to compile to
    // `op:"now"`.
    h.check(
        "P167: datetime op as a number rejected (was silently op:\"now\")",
        !ok(json!({"kind": "datetime", "op": 4})),
    );
    let m = err_of(json!({"kind": "datetime", "op": 4}));
    h.check("P167: the error names the option", m.contains("\"op\""));
    h.check("P167: the error says must be a string", m.contains("must be a string"));
    h.check("P167: the error names the as_str fallback", m.contains("as_str()"));
    h.check("P167: db op as a number rejected", !ok(json!({"kind": "db", "op": 4})));
    h.check(
        "P167: cache op as a bool rejected",
        !ok(json!({"kind": "cache", "op": true, "path": "p"})),
    );
    h.check(
        "P167: sqlite op as a number rejected (required db supplied)",
        !ok(json!({"kind": "sqlite", "db": "/tmp/p.db", "op": 4})),
    );
    h.check(
        "P167: http method as a number rejected",
        !ok(json!({"kind": "http", "method": 4, "url": "http://x"})),
    );
    h.check(
        "P167: mcp transport as a number rejected (was silently http)",
        !ok(json!({"kind": "mcp", "tool": "t", "url": "http://x", "transport": 4})),
    );
    h.check(
        "P167: agent transport as a number rejected",
        !ok(json!({"kind": "agent", "url": "http://x", "transport": 4})),
    );
    h.check(
        "P167: sql driver as a number rejected",
        !ok(json!({"kind": "sql", "dsn": "sqlite3://p", "driver": 4})),
    );
    h.check(
        "P167: goal_runner runner as a number rejected",
        !ok(json!({"kind": "goal_runner", "runner": 4})),
    );
    // the value table still owns the *value*: a string typo is still refused,
    // and a correct value still compiles — the type gate added a layer, it did
    // not replace the closed-set check.
    h.check(
        "P167: a string op outside the closed set is still refused by the table",
        !ok(json!({"kind": "datetime", "op": "noww"})),
    );
    h.check(
        "P167: a correct closed-set op still compiles",
        ok(json!({"kind": "datetime", "op": "now"})),
    );
    // opaque JSON options are untouched: still any type
    h.check(
        "P167: http body stays opaque (any JSON)",
        ok(json!({"kind": "http", "url": "http://x", "body": 5})),
    );
    h.check(
        "P167: graphql variables stays opaque (any JSON)",
        ok(json!({"kind": "graphql", "url": "http://x", "variables": {"a": 1}})),
    );
}


/// P174: the `browser` capability clamps two numeric knobs at load, so a value
/// outside the band is never the one that runs - a `max_owned_pages` of 1000
/// runs as 500, and an `owned_idle_ms` of 500 ("reclaim almost immediately")
/// runs as 1000. `validate` accepted both in silence. P150 asserted the *type*
/// of these options; this asserts their *range*, from the same constants the
/// clamp uses, so the two cannot drift apart.
pub fn test_validate_option_clamps(h: &mut Harness) {
    let nodes = json!([node(
        "a",
        json!({"q": {"type": "choice", "instructions": "?", "criteria": {"A": "a"}}}),
        json!({"condition": {}, "default": "STOP"}),
        json!({"kind": "copy_keys", "keys": ["q"]})
    )]);
    let ok = |cap: Value| {
        let mut s = base_spec("a", nodes.clone());
        s["capabilities"] = json!({ "c": cap });
        spec::from_spec(&s).is_ok()
    };
    let err = |cap: Value| -> String {
        let mut s = base_spec("a", nodes.clone());
        s["capabilities"] = json!({ "c": cap });
        spec::from_spec(&s).err().map(|e| e.to_string()).unwrap_or_default()
    };

    // positives — inside the band, on the canonical kind and on its alias
    h.check("validate: browser max_owned_pages 1 is OK", ok(json!({"kind": "browser", "max_owned_pages": 1})));
    h.check("validate: browser max_owned_pages 500 is OK", ok(json!({"kind": "browser", "max_owned_pages": 500})));
    h.check("validate: browser owned_idle_ms 1000 is OK", ok(json!({"kind": "browser", "owned_idle_ms": 1000})));
    h.check("validate: chrome_cdp owned_idle_ms 86400000 is OK",
        ok(json!({"kind": "chrome_cdp", "owned_idle_ms": 86_400_000u64})));

    // negatives — outside the band the runtime runs a different value
    h.check("validate: browser max_owned_pages 501 rejected",
        !ok(json!({"kind": "browser", "max_owned_pages": 501})));
    h.check("validate: browser max_owned_pages 0 rejected",
        !ok(json!({"kind": "browser", "max_owned_pages": 0})));
    h.check("validate: browser owned_idle_ms 999 rejected",
        !ok(json!({"kind": "browser", "owned_idle_ms": 999})));
    h.check("validate: chrome_cdp owned_idle_ms 86400001 rejected",
        !ok(json!({"kind": "chrome_cdp", "owned_idle_ms": 86_400_001u64})));

    // the message names the band, the option and the silent replacement
    let m = err(json!({"kind": "chrome_cdp", "max_owned_pages": 1000}));
    h.check("validate: the clamp error names the band", m.contains("1..=500"));
    h.check("validate: the clamp error names the option", m.contains("max_owned_pages"));
    h.check("validate: the clamp error says the value is silently replaced",
        m.contains("silently replaced"));
    let m = err(json!({"kind": "browser", "owned_idle_ms": 250}));
    h.check("validate: the idle-band error names its bounds", m.contains("1000..=86400000"));

    // ── the guard's band IS the runtime's band ──
    // `check_option_domains` refuses a value outside `OWNED_PAGES_MIN..=OWNED_PAGES_MAX`
    // / `OWNED_IDLE_MS_MIN..=OWNED_IDLE_MS_MAX`, and `parse_kind` builds the cap through
    // `clamp_owned_pages` / `clamp_owned_idle_ms`, which clamp against those same
    // constants. So an in-band pair survives parse untouched and an out-of-band value
    // lands exactly on the bound the error named. If either side moved alone, one of
    // these disagrees.
    let inband = |cap: Value| -> (usize, u64) {
        let s = json!({"name": "c", "start": "a", "nodes": [],
                       "capabilities": {"c": cap}});
        let reg = capability::Registry::from_spec(&s)
            .expect("registry accepts an in-band browser cap");
        match reg.get("c").expect("cap") {
            capability::Capability::Browser(b) => (b.max_owned_pages, b.owned_idle_ms),
            _ => panic!("expected a browser capability"),
        }
    };
    h.check("validate: an in-band pair survives parse as written",
        inband(json!({"kind": "browser", "max_owned_pages": 32, "owned_idle_ms": 300000}))
            == (32, 300_000));
    h.check("validate: the runtime clamp is the band the guard names (pages)",
        capability::clamp_owned_pages(1000) == 500
            && capability::clamp_owned_pages(0) == 1
            && capability::clamp_owned_pages(32) == 32);
    h.check("validate: the runtime clamp is the band the guard names (idle ms)",
        capability::clamp_owned_idle_ms(500) == 1_000
            && capability::clamp_owned_idle_ms(u64::MAX) == 86_400_000
            && capability::clamp_owned_idle_ms(300_000) == 300_000);
}

/// P176: `browser` with `attach: true` returns on the attach branch
/// (`browser.rs`: `if c.attach { ... }`) before the launch path is ever entered,
/// so `launch`, `profile_dir`, `chrome_binary` and `startup_timeout_ms` are read
/// by nobody - `launch` itself because "attach never spawns", the other three
/// because only the launch path consumes them (`profile_dir` builds the profile
/// Chrome is spawned against, `chrome_binary` is the binary it spawns, and
/// `startup_timeout_ms` is the spawn wait - the attach path bounds its own
/// reachability probe by `timeout_ms`). A spec that sets them while attaching
/// believes Laya will start Chrome; the runtime instead bails on an unreachable
/// endpoint and never reads them. `validate` accepted the dead pair in silence.
/// `launch: false` is the one exception - it agrees with what attach already
/// does (never spawn), so it is not a value the engine discards.
pub fn test_validate_option_attach(h: &mut Harness) {
    let nodes = json!([node(
        "a",
        json!({"q": {"type": "choice", "instructions": "?", "criteria": {"A": "a"}}}),
        json!({"condition": {}, "default": "STOP"}),
        json!({"kind": "copy_keys", "keys": ["q"]})
    )]);
    let ok = |cap: Value| {
        let mut s = base_spec("a", nodes.clone());
        s["capabilities"] = json!({ "c": cap });
        spec::from_spec(&s).is_ok()
    };
    let err = |cap: Value| -> String {
        let mut s = base_spec("a", nodes.clone());
        s["capabilities"] = json!({ "c": cap });
        spec::from_spec(&s).err().map(|e| e.to_string()).unwrap_or_default()
    };
    let ep = "http://127.0.0.1:9222";

    // positives - attach on its own, and the options the attach branch *does* read
    h.check(
        "validate: browser attach alone is OK",
        ok(json!({"kind": "browser", "endpoint": ep, "attach": true})),
    );
    h.check(
        "validate: attach + launch:false is OK (attaching never spawns anyway)",
        ok(json!({"kind": "browser", "endpoint": ep, "attach": true, "launch": false})),
    );
    h.check(
        "validate: attach + timeout_ms is OK (the attach probe reads it)",
        ok(json!({"kind": "browser", "endpoint": ep, "attach": true, "timeout_ms": 5000})),
    );
    h.check(
        "validate: attach + extension_path is OK (the attach branch injects it)",
        ok(json!({"kind": "browser", "endpoint": ep, "attach": true, "extension_path": "/tmp/ext"})),
    );
    h.check(
        "validate: chrome_cdp alias attach alone is OK",
        ok(json!({"kind": "chrome_cdp", "endpoint": ep, "attach": true})),
    );
    h.check(
        "validate: the four launch-only options stay OK without attach",
        ok(json!({"kind": "browser", "endpoint": ep, "launch": true, "profile_dir": "/tmp/p",
                  "chrome_binary": "/usr/bin/chrome", "startup_timeout_ms": 30000})),
    );

    // negatives - attach + a launch-only option is a setting that never runs
    h.check(
        "validate: attach + launch:true rejected",
        !ok(json!({"kind": "browser", "endpoint": ep, "attach": true, "launch": true})),
    );
    h.check(
        "validate: attach + profile_dir rejected",
        !ok(json!({"kind": "browser", "endpoint": ep, "attach": true, "profile_dir": "/tmp/p"})),
    );
    h.check(
        "validate: attach + chrome_binary rejected",
        !ok(json!({"kind": "browser", "endpoint": ep, "attach": true, "chrome_binary": "/usr/bin/chrome"})),
    );
    h.check(
        "validate: attach + startup_timeout_ms rejected",
        !ok(json!({"kind": "browser", "endpoint": ep, "attach": true, "startup_timeout_ms": 30000})),
    );
    h.check(
        "validate: chrome_cdp alias attach + launch:true rejected",
        !ok(json!({"kind": "chrome_cdp", "endpoint": ep, "attach": true, "launch": true})),
    );
    // an empty profile_dir is the default the launch path falls back to, not a value anybody wrote
    h.check(
        "validate: attach + empty profile_dir is OK (the default, not a setting)",
        ok(json!({"kind": "browser", "endpoint": ep, "attach": true, "profile_dir": ""})),
    );

    // the message names the dead option, says it never runs, and says why
    let m = err(json!({"kind": "browser", "endpoint": ep, "attach": true, "profile_dir": "/tmp/p"}));
    h.check("validate: the attach error names the dead option", m.contains("profile_dir"));
    h.check("validate: the attach error says the option never runs", m.contains("never runs"));
    h.check("validate: the attach error names the swallow", m.contains("silently ignored"));
    let m = err(json!({"kind": "browser", "endpoint": ep, "attach": true, "launch": true, "profile_dir": "/tmp/p"}));
    h.check("validate: the attach error names the first dead option in order", m.contains("\"launch\""));

    // the guard covers exactly the launch-only options: knobs both paths read survive
    h.check(
        "validate: attach + human is OK (the action path reads it)",
        ok(json!({"kind": "browser", "endpoint": ep, "attach": true, "human": false})),
    );
    h.check(
        "validate: attach + max_text is OK (not a launch-path option)",
        ok(json!({"kind": "browser", "endpoint": ep, "attach": true, "max_text": 100})),
    );
}

/// P177: a hand-written / side-loaded JSON spec is held to the same `call`-step
/// `with` contract the BDD compile path enforces - a key no call of the kind
/// reads (P164), a value of the wrong type (P166), or a number outside the read
/// site's band (P175). Those three guards first lived only in `bdd.rs`, so a
/// `validate --spec` on hand-written JSON passed all of them with exit 0. The
/// decision is now shared (`capability::with_keys::with_step_issue`), with the
/// BDD path prefixing `line N:` and this one `node <name>:`.
pub fn test_validate_call_with(h: &mut Harness) {
    use crate::capability::with_keys::{is_open, kind_reads, with_issue_body, with_step_issue, WithIssue};
    // A chrome (`chrome_cdp`) call whose `with` is clean, plus a `plugin` call
    // (open kind - any key is legal).
    let spec_with_caps = |with: Value, chain: Value| -> Value {
        let mut s = base_spec(
            "a",
            json!([
                node(
                    "a",
                    json!({"q": {"type": "choice", "instructions": "?", "criteria": {"A": "a"}}}),
                    json!({"condition": {}, "default": "STOP"}),
                    json!({"kind": "call", "capability": "chrome", "with": with, "chain": chain})
                )
            ]),
        );
        s["capabilities"] = json!({
            "chrome": {"kind": "chrome_cdp", "endpoint": "http://127.0.0.1:9222"},
            "plug": {"kind": "plugin", "plugin": "websites/x"},
            "raw": {"kind": "passthrough"}
        });
        s
    };
    let ok = |with: Value, chain: Value| spec::from_spec(&spec_with_caps(with, chain)).is_ok();
    let err = |with: Value, chain: Value| -> String {
        spec::from_spec(&spec_with_caps(with, chain))
            .err()
            .map(|e| e.to_string())
            .unwrap_or_default()
    };
    // the same spec with the outer action retargeted at another capability
    let okc = |cap: &str, with: Value, chain: Value| {
        let mut s = spec_with_caps(with, chain);
        s["nodes"][0]["action"]["capability"] = json!(cap);
        spec::from_spec(&s).is_ok()
    };

    // ── the clean cases stay clean ──
    h.check(
        "validate call-with: a key the kind reads is OK",
        ok(json!({"op": "open", "url": "http://127.0.0.1:9222"}), json!([])),
    );
    h.check(
        "validate call-with: a `${state.x}` template is not judged",
        ok(json!({"op": "open", "url": "${state.url}"}), json!([])),
    );
    h.check(
        "validate call-with: a `${state.x}` template is OK on a numeric read",
        ok(json!({"op": "snapshot", "result_count": "${state.n}"}), json!([])),
    );
    h.check(
        "validate call-with: a key named only by a `${with.X}` sibling counts as read",
        ok(json!({"op": "click", "selector": "${with.targetId}", "targetId": "#a"}), json!([])),
    );
    h.check(
        "validate call-with: an open kind (plugin) accepts any key",
        okc("plug", json!({"op": "run", "whatever": 1}), json!([])),
    );
    h.check(
        "validate call-with: a chain sub-call on an open kind is not judged",
        ok(
            json!({"op": "open"}),
            json!([{"as": "x", "capability": "plug", "with": {"anything": 1}}]),
        ),
    );
    h.check(
        "validate call-with: an empty `with` is OK",
        ok(json!({}), json!([])),
    );
    // a template whose *value* is a string but names a with-ref is not a wrong type
    h.check(
        "validate call-with: a `${with.X}` string is not a wrong-typed number",
        ok(json!({"op": "snapshot", "result_count": "${with.n}", "n": 3}), json!([])),
    );

    // ── P164: a key no call of the kind reads ──
    let m = err(json!({"op": "open", "modes": "search"}), json!([]));
    h.check("validate call-with: an unread key is refused", !m.is_empty());
    h.check("validate call-with: the unread-key error names the node", m.contains("node a:"));
    h.check("validate call-with: the unread-key error names the key", m.contains("with.modes"));
    h.check(
        "validate call-with: the unread-key error says the value is silently dropped",
        m.contains("silently dropped"),
    );
    h.check(
        "validate call-with: the unread-key error suggests the near miss",
        err(json!({"op": "snapshot", "result_counts": 3}), json!([]))
            .contains("Did you mean `with.result_count`?"),
    );
    h.check(
        "validate call-with: an unread key in a chain entry is refused",
        err(
            json!({"op": "open"}),
            json!([{"as": "x", "capability": "chrome", "with": {"nope": 1}}]),
        )
        .contains("with.nope"),
    );

    // ── P166: a value of the wrong type ──
    let m = err(json!({"op": "snapshot", "result_count": "twenty"}), json!([]));
    h.check("validate call-with: a wrong-typed value is refused", !m.is_empty());
    h.check(
        "validate call-with: the wrong-type error says the value is discarded",
        m.contains("silently discarded"),
    );
    h.check("validate call-with: the wrong-type error names the read type", m.contains("reads it as"));

    // ── P175: a number outside the read site's band ──
    let m = err(json!({"op": "snapshot", "result_count": 50}), json!([]));
    h.check("validate call-with: an out-of-band number is refused", !m.is_empty());
    h.check("validate call-with: the band error names the band", m.contains("1..=20"));
    h.check(
        "validate call-with: the band error says the value is replaced at the read site",
        m.contains("silently replaced at the read site"),
    );
    h.check(
        "validate call-with: an in-band number is OK",
        ok(json!({"op": "snapshot", "result_count": 20}), json!([])),
    );

    // ── the shared decision itself (no drift between the two paths) ──
    let caps: serde_json::Map<String, Value> = serde_json::from_value(json!({
        "chrome": {"kind": "chrome_cdp"}
    }))
    .unwrap();
    let with: serde_json::Map<String, Value> =
        serde_json::from_value(json!({"modes": "x"})).unwrap();
    let issue = with_step_issue(&caps, "chrome", &with, &[]).expect("unread key");
    h.check(
        "validate call-with: with_step_issue reports the unread key",
        matches!(issue, WithIssue::Unread { ref key, .. } if key == "modes"),
    );
    h.check(
        "validate call-with: with_issue_body is the one message both paths print",
        with_issue_body(&issue).contains("no \"chrome_cdp\" call reads that key"),
    );
    h.check(
        "validate call-with: an open kind yields no issue",
        with_step_issue(
            &serde_json::from_value::<serde_json::Map<String, Value>>(
                json!({"p": {"kind": "plugin"}}),
            )
            .unwrap(),
            "p",
            &with,
            &[],
        )
        .is_none(),
    );
    // P177: `passthrough` (and its alias `noop`) returns the whole expanded
    // `with`, so every key is read - it is an open kind, and a guard that judged
    // it would refuse a key it cannot prove is unread.
    h.check(
        "validate call-with: passthrough and noop are open kinds",
        is_open("passthrough") && is_open("noop") && kind_reads("passthrough").is_none(),
    );
    h.check(
        "validate call-with: a passthrough call accepts any `with` key",
        okc("raw", json!({"token": "x", "n": 1}), json!([])),
    );
}

/// P178: a hand-written / side-loaded JSON spec is held to the same `call`-step
/// `project` contract the BDD compile path enforces (P165) - a pointer the engine
/// skips (it reads a pointer with `as_str()`, so any other JSON type leaves the
/// state key unwritten), an empty projection key, or a `chain` row keyed by an
/// explicit empty alias. Those three checks first lived only in `bdd.rs`, so a
/// `validate --spec` on hand-written JSON dropped them all with exit 0. The
/// decision is now shared (`capability::call_project::project_step_issue`), with
/// the BDD path prefixing `line N:` and this one `node <name>:`.
pub fn test_validate_call_project(h: &mut Harness) {
    use crate::capability::call_project::{project_issue_body, project_step_issue, ProjectIssue};
    // A `clock` (`datetime`) call carrying a `project` table and an optional
    // `chain`; only the `project` half is under test here.
    let spec = |project: Value, chain: Value| -> Value {
        let mut s = base_spec(
            "a",
            json!([
                node(
                    "a",
                    json!({"q": {"type": "choice", "instructions": "?", "criteria": {"A": "a"}}}),
                    json!({"condition": {}, "default": "STOP"}),
                    json!({"kind": "call", "capability": "clock", "project": project, "chain": chain})
                )
            ]),
        );
        s["capabilities"] = json!({ "clock": {"kind": "datetime", "op": "now"} });
        s
    };
    let ok = |project: Value, chain: Value| spec::from_spec(&spec(project, chain)).is_ok();
    let err = |project: Value, chain: Value| -> String {
        spec::from_spec(&spec(project, chain))
            .err()
            .map(|e| e.to_string())
            .unwrap_or_default()
    };

    // ── the clean cases stay clean ──
    h.check(
        "validate call-project: a string pointer is OK",
        ok(json!({"now": "/epoch"}), json!([])),
    );
    h.check(
        "validate call-project: an empty projection table is OK",
        ok(json!({}), json!([])),
    );
    h.check(
        "validate call-project: a string pointer on a chain entry is OK",
        ok(
            json!({"now": "/epoch"}),
            json!([{"as": "x", "capability": "clock", "project": {"base": "/offset_secs"}}]),
        ),
    );
    // a chain entry with *no* `as` is legal in a spec: the engine falls back to
    // the capability name, so nothing is filed under a dead key.
    h.check(
        "validate call-project: a chain entry with no `as` is OK (falls back to the cap name)",
        ok(
            json!({"now": "/epoch"}),
            json!([{"capability": "clock", "project": {"base": "/offset_secs"}}]),
        ),
    );

    // ── a pointer of any non-string JSON type is skipped ──
    let m = err(json!({"count": 3}), json!([]));
    h.check("validate call-project: a numeric pointer is refused", !m.is_empty());
    h.check("validate call-project: the error names the node", m.contains("node a:"));
    h.check("validate call-project: the error names the row", m.contains("project.count"));
    h.check(
        "validate call-project: the error names the state key it would never write",
        m.contains("state.count"),
    );
    h.check(
        "validate call-project: the error says the pointer is not a JSON string",
        m.contains("not a JSON string"),
    );
    h.check(
        "validate call-project: a boolean pointer is refused",
        !err(json!({"ok": true}), json!([])).is_empty(),
    );
    h.check(
        "validate call-project: a null pointer is refused",
        !err(json!({"v": null}), json!([])).is_empty(),
    );

    // ── an empty projection key writes a state key nothing can name ──
    let m = err(json!({"": "/epoch"}), json!([]));
    h.check("validate call-project: an empty projection key is refused", !m.is_empty());
    h.check("validate call-project: the empty-key error says empty name", m.contains("empty name"));

    // ── a chain row keyed by an explicit empty alias ──
    let m = err(json!({"now": "/epoch"}), json!([{"as": "", "capability": "clock"}]));
    h.check("validate call-project: an explicit empty chain alias is refused", !m.is_empty());
    h.check("validate call-project: the empty-alias error says empty alias", m.contains("empty alias"));
    h.check(
        "validate call-project: the empty alias is refused before its project rows",
        err(
            json!({"now": "/epoch"}),
            json!([{"as": "", "capability": "clock", "project": {"k": 3}}]),
        )
        .contains("empty alias"),
    );

    // ── a pointer on a *chain* entry is judged like the action's own ──
    let m = err(
        json!({"now": "/epoch"}),
        json!([{"as": "x", "capability": "clock", "project": {"ok": true}}]),
    );
    h.check("validate call-project: a non-string chain pointer is refused", !m.is_empty());
    h.check(
        "validate call-project: the chain error names the chain row",
        m.contains("chain.x.project.ok"),
    );

    // ── the shared decision itself (no drift between the two paths) ──
    let proj = |v: Value| -> serde_json::Map<String, Value> {
        serde_json::from_value(v).unwrap()
    };
    let issue = project_step_issue(&proj(json!({"count": 3})), &[]).expect("non-string pointer");
    h.check(
        "validate call-project: project_step_issue reports the non-string pointer",
        matches!(issue, ProjectIssue::NotString { ref row, ref key, .. } if row == "project.count" && key == "count"),
    );
    h.check(
        "validate call-project: project_issue_body is the one message both paths print",
        project_issue_body(&issue).contains("not a JSON string"),
    );
    h.check(
        "validate call-project: an empty key is reported as an empty key",
        matches!(
            project_step_issue(&proj(json!({"": "/x"})), &[]),
            Some(ProjectIssue::EmptyKey { .. })
        ),
    );
    h.check(
        "validate call-project: an explicit empty `as` is reported as an empty alias",
        matches!(
            project_step_issue(&proj(json!({})), &[json!({"as": "", "capability": "clock"})]),
            Some(ProjectIssue::EmptyAlias { .. })
        ),
    );
    h.check(
        "validate call-project: a chain entry with no `as` yields no issue",
        project_step_issue(
            &proj(json!({})),
            &[json!({"capability": "clock", "project": {"base": "/x"}})],
        )
        .is_none(),
    );
    h.check(
        "validate call-project: a clean call yields no issue",
        project_step_issue(&proj(json!({"now": "/epoch"})), &[]).is_none(),
    );
}

/// P179: a node/spec value of the wrong JSON *type* is silently replaced by the
/// engine default — the parser reads every structural value with a typed accessor
/// (`as_f64` / `as_u64` / `as_str` / `as_object`) and falls back when it returns
/// `None`, so a hand-written spec that says `"min_confidence": "0.9"` runs with
/// 0.0, `"max_retries": "5"` runs with 2, and `"edge.default": 5` leaves the node
/// with no fallback route at all — every one of them green under `validate`.
/// `guard_spec_value_types` rejects them; *range* (`min_confidence` ∈ [0, 1]) stays
/// a separate axis, and `null` (the describe/round-trip spelling of an absent
/// optional) and a `${...}` template are never judged.
pub fn test_validate_spec_value_types(h: &mut Harness) {
    let base = |node: Value| -> Value { json!({"name": "probe", "start": "a", "nodes": [node]}) };
    // A structurally complete node — one `choice` question, one routable edge,
    // no action — so a rejection can only come from the value under test.
    let pnode = |extra: Value| -> Value {
        let mut n = json!({
            "name": "a",
            "questions": {"q": {"type": "choice", "instructions": "?", "criteria": {"A": "STOP", "B": "STOP"}}},
            "edge": {"condition": {"A": "STOP", "B": "STOP"}},
        });
        if let (Some(m), Some(e)) = (n.as_object_mut(), extra.as_object()) {
            for (k, v) in e {
                m.insert(k.clone(), v.clone());
            }
        }
        n
    };
    let ok = |s: &Value| spec::from_spec(s).is_ok();
    let err = |s: &Value| spec::from_spec(s).err().map(|e| e.to_string()).unwrap_or_default();

    // ── clean stays clean (no false refusal) ──
    h.check("validate value-types: a plain node is OK", ok(&base(pnode(json!({})))));
    h.check(
        "validate value-types: `action: null` (describe's absent spelling) is OK",
        ok(&base(pnode(json!({"action": null})))),
    );
    h.check(
        "validate value-types: `edge.default: null` is OK",
        ok(&base(pnode(json!({"edge": {"condition": {"A": "STOP", "B": "STOP"}, "default": null}})))),
    );
    h.check(
        "validate value-types: a `${...}` template is not judged",
        ok(&base(pnode(json!({"min_confidence": "${X}"})))),
    );
    h.check(
        "validate value-types: an integer min_confidence is OK",
        ok(&base(pnode(json!({"min_confidence": 1})))),
    );
    h.check(
        "validate value-types: max_retries 0 is OK",
        ok(&base(pnode(json!({"max_retries": 0})))),
    );
    h.check(
        "validate value-types: a string spec name is OK",
        ok(&json!({"name": "probe", "start": "a", "convergence_eps": 0.5, "nodes": [pnode(json!({}))]})),
    );

    // ── the spec's own values ──
    h.check(
        "validate value-types: a non-string spec `name` is rejected",
        !err(&json!({"name": 5, "start": "a", "nodes": [pnode(json!({}))]})).is_empty(),
    );
    h.check(
        "validate value-types: a string `convergence_window` is rejected",
        err(&json!({"name": "probe", "start": "a", "convergence_window": "3", "nodes": [pnode(json!({}))]}))
            .contains("convergence_window"),
    );
    h.check(
        "validate value-types: a fractional `convergence_window` is rejected",
        err(&json!({"name": "probe", "start": "a", "convergence_window": 2.5, "nodes": [pnode(json!({}))]}))
            .contains("convergence_window"),
    );
    h.check(
        "validate value-types: a string `convergence_eps` is rejected",
        err(&json!({"name": "probe", "start": "a", "convergence_eps": "0.001", "nodes": [pnode(json!({}))]}))
            .contains("convergence_eps"),
    );

    // ── the node's own values ──
    h.check(
        "validate value-types: a string `min_confidence` names `as_f64()`",
        err(&base(pnode(json!({"min_confidence": "0.9"})))).contains("as_f64()"),
    );
    h.check(
        "validate value-types: a string `edge.min_confidence` is rejected",
        err(&base(pnode(json!({"edge": {"condition": {"A": "STOP", "B": "STOP"}, "min_confidence": "0.9"}}))))
            .contains("edge.min_confidence"),
    );
    h.check(
        "validate value-types: a string `max_retries` names `as_u64()`",
        err(&base(pnode(json!({"max_retries": "5"})))).contains("as_u64()"),
    );
    h.check(
        "validate value-types: a negative `max_retries` is rejected",
        !err(&base(pnode(json!({"max_retries": -1})))).is_empty(),
    );
    h.check(
        "validate value-types: a fractional `max_retries` is rejected",
        !err(&base(pnode(json!({"max_retries": 1.5})))).is_empty(),
    );
    h.check(
        "validate value-types: a numeric `edge.default` names `as_str()`",
        err(&base(pnode(json!({"edge": {"condition": {"A": "STOP", "B": "STOP"}, "default": 5}}))))
            .contains("as_str()"),
    );
    h.check(
        "validate value-types: a non-object `edge` is rejected",
        err(&base(pnode(json!({"edge": 5})))).contains("`edge`"),
    );
    h.check(
        "validate value-types: an array `edge` is rejected",
        !err(&base(pnode(json!({"edge": []})))).is_empty(),
    );
    h.check(
        "validate value-types: a non-object `edge.condition` is rejected",
        err(&base(pnode(json!({"edge": {"condition": 5}})))).contains("edge.condition"),
    );
    h.check(
        "validate value-types: a non-object `questions` is rejected",
        err(&base(pnode(json!({"questions": "q"})))).contains("`questions`"),
    );
    h.check(
        "validate value-types: a non-object question entry is rejected",
        !err(&base(pnode(json!({"questions": {"q": "x"}})))).is_empty(),
    );
    h.check(
        "validate value-types: a non-object `action` is rejected",
        err(&base(pnode(json!({"action": "copy"})))).contains("`action`"),
    );
    h.check(
        "validate value-types: a numeric `primary_q` is rejected",
        !err(&base(pnode(json!({"primary_q": 5})))).is_empty(),
    );
}

/// P180: a node/spec value in the wrong *range* is silently obeyed with a nonsense
/// outcome — the twin of P179's wrong-type guard. `convergence_window: 0` makes the
/// engine's convergence test fold an **empty** window (`mx - mn = -inf <= eps`), so
/// the run stops after the first node claiming `final_action: "converged"`; a
/// negative `convergence_eps` can never be met, so the guard never fires and the run
/// goes to `max_iterations`. Both are rejected; the legal values are untouched (the
/// corpus uses only `convergence_window` 3/5 and positive `convergence_eps`).
pub fn test_validate_spec_value_ranges(h: &mut Harness) {
    let base = |extra: Value| -> Value {
        let mut s = json!({"name": "probe", "start": "a", "nodes": [json!({
            "name": "a",
            "questions": {"q": {"type": "choice", "instructions": "?", "criteria": {"A": "STOP", "B": "STOP"}}},
            "edge": {"condition": {"A": "STOP", "B": "STOP"}},
        })]});
        if let (Some(m), Some(e)) = (s.as_object_mut(), extra.as_object()) {
            for (k, v) in e {
                m.insert(k.clone(), v.clone());
            }
        }
        s
    };
    let ok = |s: &Value| spec::from_spec(s).is_ok();
    let err = |s: &Value| spec::from_spec(s).err().map(|e| e.to_string()).unwrap_or_default();

    h.check(
        "validate value-ranges: a plain spec (no convergence knobs) is OK",
        ok(&base(json!({}))),
    );
    h.check(
        "validate value-ranges: convergence_window 1 is OK",
        ok(&base(json!({"convergence_window": 1}))),
    );
    h.check(
        "validate value-ranges: convergence_window 5 is OK",
        ok(&base(json!({"convergence_window": 5}))),
    );
    h.check(
        "validate value-ranges: convergence_eps 0 is OK (exact-match convergence)",
        ok(&base(json!({"convergence_eps": 0}))),
    );
    h.check(
        "validate value-ranges: convergence_eps 1e-4 is OK",
        ok(&base(json!({"convergence_eps": 0.0001}))),
    );
    h.check(
        "validate value-ranges: convergence_window 0 is rejected and names the window",
        err(&base(json!({"convergence_window": 0}))).contains("`convergence_window` is 0"),
    );
    h.check(
        "validate value-ranges: a negative convergence_eps is rejected",
        err(&base(json!({"convergence_eps": -0.5}))).contains("`convergence_eps`"),
    );
}

/// P181: a question's `type` is read with `unwrap_or("choice")` (backend.rs /
/// workflow.rs / bdd.rs / bdd_mcp.rs) and `unwrap_or("boolean")` (rules.rs), so an
/// unrecognised or missing value is silently replaced by `choice`. The question is
/// then answered and routed as a choice — its answer keys the node's edge condition —
/// and the tooling reports the type as `choice`, so the author never sees the slip.
/// Measured (real run, `t_scor` vs `t_score`): a `score`-shaped question mis-typed
/// `scor` made `run` route on the *answer string* (`a` -> `b`) where the real `score`
/// answer (a number) matched no edge and stopped at `a`; both `validate` calls exited 0.
pub fn test_validate_question_type(h: &mut Harness) {
    let base = |qt: Value| -> Value {
        json!({"name": "probe", "start": "a", "nodes": [{
            "name": "a",
            "questions": {"q": qt},
            "edge": {"condition": {"A": "STOP", "B": "STOP"}},
        }]})
    };
    let ok = |s: &Value| spec::from_spec(s).is_ok();
    let err = |s: &Value| spec::from_spec(s).err().map(|e| e.to_string()).unwrap_or_default();

    // ── every type the engine knows stays accepted ──
    h.check(
        "validate question-type: `choice` is OK",
        ok(&base(json!({"type": "choice", "instructions": "?", "criteria": {"A": "a", "B": "b"}}))),
    );
    h.check(
        "validate question-type: `score` is OK",
        ok(&base(json!({"type": "score", "instructions": "?", "criteria": ["A", "B"]}))),
    );
    h.check(
        "validate question-type: `noul` is OK",
        ok(&base(json!({"type": "noul", "instructions": "?", "truthy": "yes", "falsy": "no"}))),
    );
    h.check(
        "validate question-type: `boolean` (the rules backend) is OK",
        ok(&{
            let mut s = base(json!({"type": "boolean", "instructions": "?", "criteria": {"true": "y", "false": "n"}}));
            s["nodes"][0]["edge"]["condition"] = json!({"true": "STOP", "false": "STOP"});
            s
        }),
    );
    h.check(
        "validate question-type: a typed question with a heuristic is OK",
        ok(&base(json!({"type": "choice", "instructions": "?", "criteria": {"A": "a", "B": "b"},
            "heuristic": {"match_any": ["x"], "p_hit": 0.9}}))),
    );

    // ── a typo or a missing type is rejected, naming the value and the fix ──
    let e = err(&base(json!({"type": "chocie", "instructions": "?", "criteria": {"A": "a", "B": "b"}})));
    h.check(
        "validate question-type: `chocie` is rejected",
        e.contains("chocie") && e.contains("silently replaced by `choice`"),
    );
    h.check(
        "validate question-type: the typo gets a did-you-mean hint",
        e.contains("did you mean") && e.contains("choice"),
    );
    h.check(
        "validate question-type: `scor` is rejected and named",
        err(&base(json!({"type": "scor", "instructions": "?", "criteria": ["A", "B"]}))).contains("scor"),
    );
    h.check(
        "validate question-type: a missing `type` is rejected as the same silent default",
        err(&base(json!({"instructions": "?", "criteria": {"A": "a", "B": "b"}})))
            .contains("declares no `type`"),
    );
    h.check(
        "validate question-type: a non-string `type` is rejected",
        !err(&base(json!({"type": 5, "instructions": "?", "criteria": {"A": "a", "B": "b"}}))).is_empty(),
    );
}
