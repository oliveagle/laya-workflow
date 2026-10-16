//! BDD decision-workflow compiler tests (`@decision` features): Gherkin →
//! decision-graph spec, with criteria tables, edge routing, ordered threshold
//! rules, copy_keys and heuristics.
#![allow(unused_imports)]
use crate::*;
use laya_workflow::bdd;
use laya_workflow::mcp::{DynTool, McpTool, McpToolSet};

pub fn test_bdd_decision(h: &mut Harness) {
    let src = r#"
@decision
Feature: support_ticket_router
  Route by category then severity.

  Scenario: classify
    Given a choice question "category" asks "Which team should handle this ticket?"
      | answer    | meaning                       |
      | billing   | invoices, payments, refunds   |
      | technical | bugs, outages, system errors  |
      | other     | everything else               |
    When the answer "technical" goes to "severity_probe"
    And the answer "billing" goes to "severity_probe"
    And any other answer goes to "STOP"
    Then keep the answer to "category"
    And require confidence at least 0.4
    And allow up to 3 retries

  Scenario: severity_probe
    Given a score question "severity" asks "How severe is the impact?"
      | level    |
      | minor    |
      | degraded |
      | outage   |
    When any other answer goes to "STOP"
    Then label "PAGE_ONCALL" when "severity" is at least 1.5
    And label "BACKLOG" otherwise
"#;
    let feature = bdd::parse(src, "<test>").expect("parse decision feature");
    let spec = bdd::compile_decision(&feature, &json!({})).expect("compile decision");
    h.eq("bdd: decision spec name", spec["name"].clone(), json!("support_ticket_router"));
    h.eq("bdd: decision start", spec["start"].clone(), json!("classify"));
    let nodes = spec["nodes"].as_array().unwrap();
    h.eq("bdd: decision node count", nodes.len(), 2);

    let classify = &nodes[0];
    h.eq("bdd: classify primary_q", classify["primary_q"].clone(), json!("category"));
    h.eq(
        "bdd: classify criteria billing",
        classify["questions"]["category"]["criteria"]["billing"].clone(),
        json!("invoices, payments, refunds"),
    );
    h.eq(
        "bdd: classify edge technical",
        classify["edge"]["condition"]["technical"].clone(),
        json!("severity_probe"),
    );
    h.eq("bdd: classify edge default", classify["edge"]["default"].clone(), json!("STOP"));
    h.eq("bdd: classify action kind", classify["action"]["kind"].clone(), json!("copy_keys"));
    h.eq("bdd: classify min_confidence", classify["min_confidence"].clone(), json!(0.4));
    h.eq("bdd: classify max_retries", classify["max_retries"].clone(), json!(3));

    let sev = &nodes[1];
    h.eq("bdd: severity primary_q", sev["primary_q"].clone(), json!("severity"));
    h.eq(
        "bdd: severity criteria",
        sev["questions"]["severity"]["criteria"].clone(),
        json!(["minor", "degraded", "outage"]),
    );
    let rules = sev["action"]["rules"].as_array().unwrap();
    h.eq("bdd: severity rule count", rules.len(), 2);
    h.eq("bdd: severity rule1 label", rules[0]["label"].clone(), json!("PAGE_ONCALL"));
    h.eq("bdd: severity rule1 gte", rules[0]["when"]["value_gte"].clone(), json!(1.5));
    h.eq("bdd: severity rule2 always", rules[1]["when"]["always"].clone(), json!(true));

    // heuristics attach to the most recent question
    let src2 = r#"
@decision
Feature: hw
  Scenario: gate
    Given a choice question "hit" asks "is it there?"
      | answer | meaning |
      | A      | no      |
      | B      | yes     |
    And answer "B" when the text contains "needle" with hit 0.95
    Then label "GO" when the answer to "hit" is "B"
    And label "NO" otherwise
"#;
    let f2 = bdd::parse(src2, "<test2>").expect("parse heuristic feature");
    let s2 = bdd::compile_decision(&f2, &json!({})).expect("compile heuristic");
    let q = &s2["nodes"][0]["questions"]["hit"];
    h.eq("bdd: heuristic match_any", q["heuristic"]["match_any"].clone(), json!(["needle"]));
    h.eq("bdd: heuristic p_hit", q["heuristic"]["p_hit"].clone(), json!(0.95));
    h.eq("bdd: heuristic p_miss", q["heuristic"]["p_miss"].clone(), json!(0.05));

    // an out-of-vocabulary decision step must fail loudly, not compile
    let src3 = r#"
@decision
Feature: bad
  Scenario: gate
    Given a choice question "q" asks "?"
      | answer | meaning |
      | A      | no      |
      | B      | yes     |
    When the answer "A" teleports to "elsewhere"
"#;
    let f3 = bdd::parse(src3, "<test3>").expect("parse bad feature");
    h.check(
        "bdd: unknown decision step is a hard error",
        bdd::compile_decision(&f3, &json!({})).is_err(),
    );
}
/// `Then the primary question is "Q"` overrides the first-question default, and
/// `Then the action reads the question "Q"` lets a threshold action read a
/// different question than the primary one (e.g. a numeric heuristic question).
pub fn test_bdd_decision_primary_q(h: &mut Harness) {
    let src = r#"
@decision
Feature: content_safety_guard
  Guard content by policy first, then severity.

  Scenario: policy
    Given a choice question "policy" asks "Which policy applies?"
      | answer   | meaning              |
      | moderate | requires moderation  |
      | allow    | safe to post         |
    And a choice question "channel" asks "Which channel?"
      | answer | meaning      |
      | chat   | chat message |
      | mail   | email        |
    When the answer "moderate" goes to "moderate"
    And any other answer goes to "STOP"
    Then the primary question is "channel"
    And keep the answer to "policy"

  Scenario: moderate
    Given a score question "severity" asks "How severe?"
      | level    |
      | low      |
      | high     |
    When any other answer goes to "STOP"
    Then the primary question is "severity"
    And the action reads the question "severity"
    And label "BLOCK" when "severity" is at least 1.5
    And label "REVIEW" otherwise
"#;
    let feature = bdd::parse(src, "<test>").expect("parse primary-q feature");
    let spec = bdd::compile_decision(&feature, &json!({})).expect("compile primary-q");
    let nodes = spec["nodes"].as_array().unwrap();

    // policy node: explicit primary is the *second* question, not the first
    let policy = &nodes[0];
    h.eq("bdd: policy primary_q", policy["primary_q"].clone(), json!("channel"));
    h.eq(
        "bdd: policy questions present",
        policy["questions"]["policy"]["type"].clone(),
        json!("choice"),
    );
    h.eq(
        "bdd: policy action kind",
        policy["action"]["kind"].clone(),
        json!("copy_keys"),
    );

    // moderate node: action.question reads "severity" explicitly
    let moderate = &nodes[1];
    h.eq("bdd: moderate primary_q", moderate["primary_q"].clone(), json!("severity"));
    h.eq(
        "bdd: moderate action question",
        moderate["action"]["question"].clone(),
        json!("severity"),
    );
    let rules = moderate["action"]["rules"].as_array().unwrap();
    h.eq("bdd: moderate rule count", rules.len(), 2);
    h.eq("bdd: moderate rule1 label", rules[0]["label"].clone(), json!("BLOCK"));
    h.eq("bdd: moderate rule1 gte", rules[0]["when"]["value_gte"].clone(), json!(1.5));
    h.eq("bdd: moderate rule2 always", rules[1]["when"]["always"].clone(), json!(true));
}

/// `is one of "A", "B"` (answer_in) and bare `the answer is "A"` (implicit
/// action.question) threshold rules, both used by the ole_eval ports.
pub fn test_bdd_decision_rule_shapes(h: &mut Harness) {
    let src = r#"
@decision
Feature: risk_edges
  Scenario: screen
    Given a choice question "intent" asks "Which intent?"
      | answer  | meaning                 |
      | ok      | normal                  |
      | stuffing | credential stuffing     |
      | takeover | account takeover        |
    When any other answer goes to "STOP"
    Then label "DENY" when the answer to "intent" is one of "stuffing", "takeover"
    And label "ALLOW" otherwise

  Scenario: confirm
    Given a choice question "intent" asks "Confirm?"
      | answer  | meaning           |
      | confirm | confirm transfer  |
      | cancel  | cancel            |
    When any other answer goes to "STOP"
    Then label "execute_transfer" when the answer is "confirm"
    And label "abort" otherwise
"#;
    let feature = bdd::parse(src, "<test>").expect("parse rule-shapes feature");
    let spec = bdd::compile_decision(&feature, &json!({})).expect("compile rule-shapes");

    let screen = &spec["nodes"][0];
    let rules = screen["action"]["rules"].as_array().unwrap();
    h.eq("bdd: screen rule count", rules.len(), 2);
    h.eq("bdd: answer_in question", rules[0]["when"]["question"].clone(), json!("intent"));
    h.eq(
        "bdd: answer_in list",
        rules[0]["when"]["answer_in"].clone(),
        json!(["stuffing", "takeover"]),
    );
    h.eq("bdd: answer_in label", rules[0]["label"].clone(), json!("DENY"));
    h.eq("bdd: always fallback", rules[1]["when"]["always"].clone(), json!(true));

    let confirm = &spec["nodes"][1];
    let rules2 = confirm["action"]["rules"].as_array().unwrap();
    h.eq("bdd: confirm rule count", rules2.len(), 2);
    h.eq("bdd: implicit answer", rules2[0]["when"]["answer"].clone(), json!("confirm"));
    h.eq(
        "bdd: implicit answer has no question key",
        rules2[0]["when"].get("question").is_none(),
        true,
    );
    h.eq("bdd: implicit answer label", rules2[0]["label"].clone(), json!("execute_transfer"));
}

/// value_lt / option+gte / score_miss / empty pass-through node — the shapes
/// that unblocked the laya_mem + guards + versioned + pipelines batch.
pub fn test_bdd_decision_extended(h: &mut Harness) {
    let src = r#"
@decision
Feature: extended_shapes

  Scenario: stop_check
    Given a noul question "evidence_sufficient" asks "Is the evidence sufficient?"
    And score "evidence_sufficient" from the numeric field "evidence_status" with miss 0.5
    When any other answer goes to "STOP"
    Then label "CONTINUE" when "evidence_sufficient" is below 0.85
    And label "CONTINUE" when "contradiction" is at least 0.4
    And label "STOP_EVIDENCE_OK" otherwise

  Scenario: moderate
    Given a choice question "is_hate" asks "Is it hate speech?"
      | answer | meaning      |
      | A      | no           |
      | B      | yes          |
    When any other answer goes to "STOP"
    Then label "BLOCK" when the answer to "is_hate" is "B" with at least 0.7 confidence
    And label "APPROVE" otherwise

  Scenario: passthrough
    When any other answer goes to "STOP"
"#;
    let feature = bdd::parse(src, "<test>").expect("parse extended feature");
    let spec = bdd::compile_decision(&feature, &json!({})).expect("compile extended");

    let stop = &spec["nodes"][0];
    h.eq("bdd: score_miss", stop["questions"]["evidence_sufficient"]["heuristic"]["score_miss"].clone(), json!(0.5));
    h.eq("bdd: score_miss field", stop["questions"]["evidence_sufficient"]["heuristic"]["extract_numeric"].clone(), json!("evidence_status"));
    let r = stop["action"]["rules"].as_array().unwrap();
    h.eq("bdd: value_lt rule", r[0]["when"]["value_lt"].clone(), json!(0.85));
    h.eq("bdd: value_lt question", r[0]["when"]["question"].clone(), json!("evidence_sufficient"));
    h.eq("bdd: value_gte with q", r[1]["when"]["question"].clone(), json!("contradiction"));
    h.eq("bdd: value_gte", r[1]["when"]["value_gte"].clone(), json!(0.4));

    let mnode = &spec["nodes"][1];
    let r2 = mnode["action"]["rules"].as_array().unwrap();
    h.eq("bdd: opt_gte option", r2[0]["when"]["option"].clone(), json!("B"));
    h.eq("bdd: opt_gte gte", r2[0]["when"]["gte"].clone(), json!(0.7));
    h.eq("bdd: opt_gte question", r2[0]["when"]["question"].clone(), json!("is_hate"));

    // empty pass-through node still compiles (0 questions)
    let nodes = spec["nodes"].as_array().unwrap();
    h.eq("bdd: node count", nodes.len(), 3);
    let pt = &nodes[2];
    h.eq("bdd: passthrough name", pt["name"].clone(), json!("passthrough"));
    h.eq("bdd: passthrough empty questions", pt["questions"].as_object().map(|m| m.len()).unwrap_or(99), 0);
    h.eq("bdd: passthrough primary_q default", pt["primary_q"].clone(), json!("ok"));
}

/// `call the capability` + with/project table, and the `with mid M` middle-band
/// heuristic (match_mid_any) — the two shapes that unblocked the capabilities
/// batch (178 call nodes across the corpus).
pub fn test_bdd_decision_call(h: &mut Harness) {
    let src = r#"
@decision
Feature: textdigest_plugin

  Scenario: digest
    Given a choice question "done" asks "Digest the text with the local plugin?"
      | answer | meaning |
      | A      | yes     |
      | B      | no      |
    When any other answer goes to "STOP"
    Then call the capability "digest"
      | with.text      | ${state.text} |
      | project.chars  | /chars        |
      | project.words  | /words        |
      | project.top    | /top          |
"#;
    let feature = bdd::parse(src, "<test>").expect("parse call feature");
    let spec = bdd::compile_decision(&feature, &json!({})).expect("compile call");
    let n = &spec["nodes"][0];
    let act = &n["action"];
    h.eq("bdd: call kind", act["kind"].clone(), json!("call"));
    h.eq("bdd: call capability", act["capability"].clone(), json!("digest"));
    h.eq("bdd: call with.text", act["with"]["text"].clone(), json!("${state.text}"));
    h.eq("bdd: call project.chars", act["project"]["chars"].clone(), json!("/chars"));
    h.eq("bdd: call project count", act["project"].as_object().map(|m| m.len()).unwrap_or(99), 3);

    // middle-band heuristic
    let src2 = r#"
@decision
Feature: rag_triage
  Scenario: triage
    Given a noul question "relevant" asks "Is the passage relevant?"
    And answer "B" when the field "route_relevant" contains "high", "yes" with hit 0.9 and miss 0.1
    And answer "B" with mid 0.5 when the field "route_relevant" contains "mid", "maybe", "unclear"
    When any other answer goes to "STOP"
    Then label "HIT" when "relevant" is at least 0.5
    And label "MISS" otherwise
"#;
    let f2 = bdd::parse(src2, "<test2>").expect("parse mid feature");
    let s2 = bdd::compile_decision(&f2, &json!({})).expect("compile mid");
    let hq = &s2["nodes"][0]["questions"]["relevant"]["heuristic"];
    h.eq("bdd: mid match_mid_any", hq["match_mid_any"].clone(), json!(["mid", "maybe", "unclear"]));
    h.eq("bdd: mid p_mid", hq["p_mid"].clone(), json!(0.5));
    h.eq("bdd: mid preserves match_any", hq["match_any"].clone(), json!(["high", "yes"]));
    h.eq("bdd: mid preserves p_hit", hq["p_hit"].clone(), json!(0.9));
    h.eq("bdd: mid preserves p_miss", hq["p_miss"].clone(), json!(0.1));
}
pub fn test_bdd_decision_regex(h: &mut Harness) {
    // match_regex heuristic: field-scoped (incl. an escaped `["']` inside a
    // pattern), field + explicit miss, and the whole-envelope text variant.
    let src = r#"
@decision
Feature: commit_msg_gate

  Scenario: check
    Given a choice question "msg_empty" asks "Is the commit message empty?"
      | answer | meaning   |
      | A      | non-empty |
      | B      | empty     |
    And answer "B" when the field "message" matches "^\s*$", "^.{0,4}$" with hit 0.95 and miss 0.05
    Given a choice question "secret_clean" asks "Any credential pattern?"
      | answer | meaning       |
      | A      | no secret     |
      | B      | credential hit |
    And answer "B" when the field "secret_diff" matches "(?i)(api[_-]?key|secret|token)\s*[:=]\s*['\"]([a-z0-9]{16,}|sk-[a-zA-Z0-9]+)" with hit 0.95
    Given a choice question "file_scan" asks "Any critical pattern?"
      | answer | meaning            |
      | A      | clean              |
      | B      | critical found     |
    And answer "B" when the text matches "curl.*\|.*sh", "eval.*base64" with hit 0.9 and miss 0.1
    When any other answer goes to "STOP"
    Then label "FAIL" when the answer to "msg_empty" is "B"
    And label "PASS" otherwise
"#;
    let feature = bdd::parse(src, "<regex>").expect("parse regex feature");
    let spec = bdd::compile_decision(&feature, &json!({})).expect("compile regex");
    let n = &spec["nodes"][0];
    let qs = &n["questions"];

    // field + explicit miss
    let h0 = &qs["msg_empty"]["heuristic"];
    h.eq("bdd: regex field", h0["field"].clone(), json!("message"));
    h.eq("bdd: regex match_regex", h0["match_regex"].clone(), json!(["^\\s*$", "^.{0,4}$"]));
    h.eq("bdd: regex p_hit", h0["p_hit"].clone(), json!(0.95));
    h.eq("bdd: regex p_miss", h0["p_miss"].clone(), json!(0.05));

    // escaped literal double-quote survives round-trip: `['\"]` -> `['"]`
    let h1 = &qs["secret_clean"]["heuristic"];
    h.eq("bdd: regex esc field", h1["field"].clone(), json!("secret_diff"));
    h.eq("bdd: regex esc pattern", h1["match_regex"][0].clone(),
        json!("(?i)(api[_-]?key|secret|token)\\s*[:=]\\s*['\"]([a-z0-9]{16,}|sk-[a-zA-Z0-9]+)"));
    h.eq("bdd: regex esc p_miss default", h1["p_miss"].clone(), json!(0.05));

    // whole-envelope variant (no field), explicit miss
    let h2 = &qs["file_scan"]["heuristic"];
    h.check("bdd: regex text variant has no field", h2.get("field").is_none());
    h.eq("bdd: regex text match_regex", h2["match_regex"].clone(), json!(["curl.*\\|.*sh", "eval.*base64"]));
    h.eq("bdd: regex text p_hit", h2["p_hit"].clone(), json!(0.9));
    h.eq("bdd: regex text p_miss", h2["p_miss"].clone(), json!(0.1));
}

pub fn test_bdd_decision_actions(h: &mut Harness) {
    // gate / merge_open_probs / combine / rubric / chain-in-call
    let src = r#"
@decision
Feature: action_kinds

  Scenario: gate_node
    Given a choice question "is_destructive" asks "Does it destroy state?"
      | answer | meaning |
      | A      | safe    |
      | B      | unsafe  |
    When any other answer goes to "STOP"
    Then gate "is_destructive" on option "B" with block 0.7 allow 0.7

  Scenario: merge_node
    Given a choice question "verdict" asks "Accept or revise?"
      | answer | meaning |
      | accept | good    |
      | revise | improve |
    When any other answer goes to "STOP"
    Then merge open probabilities on "verdict"

  Scenario: combine_node
    Given a choice question "department" asks "Which team?"
      | answer | meaning     |
      | billing | money      |
      | technical | crash    |
    And a noul question "explicit_urgency" asks "Urgent?"
    When any other answer goes to "STOP"
    Then combine "department" with signal "explicit_urgency" fallback "other" review "human_review" requiring 0.8 high 0.85 low 0.15

  Scenario: rubric_node
    Given a score question "rubric_usefulness" asks "Useful?"
      | level |
      | poor  |
      | good  |
    And a score question "rubric_clarity" asks "Clear?"
      | level |
      | bad   |
      | ok    |
      | great |
    When any other answer goes to "STOP"
    Then rubric "rubric_usefulness" at 3 levels weight 0.6 and "rubric_clarity" at 4 levels weight 0.4 requiring 0.8

  Scenario: chain_node
    Given a choice question "go" asks "Go?"
      | answer | meaning |
      | A      | yes     |
    When any other answer goes to "STOP"
    Then call the capability "stamp"
      | with.epoch              | ${with.clock.epoch} |
      | project.stamped         | /text               |
      | chain.clock.capability  | clock               |
"#;
    let feature = bdd::parse(src, "<actions>").expect("parse actions feature");
    let spec = bdd::compile_decision(&feature, &json!({})).expect("compile actions");
    let nodes = &spec["nodes"];
    let by_name = |n: &str| {
        nodes
            .as_array()
            .unwrap()
            .iter()
            .find(|x| x["name"] == n)
            .cloned()
            .unwrap()
    };

    let gate = by_name("gate_node");
    h.eq("bdd: gate kind", gate["action"]["kind"].clone(), json!("gate"));
    h.eq("bdd: gate question", gate["action"]["question"].clone(), json!("is_destructive"));
    h.eq("bdd: gate option", gate["action"]["option"].clone(), json!("B"));
    h.eq("bdd: gate block_at", gate["action"]["block_at"].clone(), json!(0.7));
    h.eq("bdd: gate allow_at", gate["action"]["allow_at"].clone(), json!(0.7));

    let merge = by_name("merge_node");
    h.eq("bdd: merge kind", merge["action"]["kind"].clone(), json!("merge_open_probs"));
    h.eq("bdd: merge question", merge["action"]["question"].clone(), json!("verdict"));

    let combine = by_name("combine_node");
    h.eq("bdd: combine kind", combine["action"]["kind"].clone(), json!("combine"));
    h.eq("bdd: combine choice", combine["action"]["choice"].clone(), json!("department"));
    h.eq("bdd: combine signal", combine["action"]["signal"].clone(), json!("explicit_urgency"));
    h.eq("bdd: combine fallback", combine["action"]["fallback"].clone(), json!("other"));
    h.eq("bdd: combine review", combine["action"]["review_label"].clone(), json!("human_review"));
    h.eq("bdd: combine min", combine["action"]["min_confidence"].clone(), json!(0.8));
    h.eq("bdd: combine high", combine["action"]["high_threshold"].clone(), json!(0.85));
    h.eq("bdd: combine low", combine["action"]["low_threshold"].clone(), json!(0.15));

    let rubric = by_name("rubric_node");
    h.eq("bdd: rubric kind", rubric["action"]["kind"].clone(), json!("rubric"));
    h.eq("bdd: rubric min", rubric["action"]["min_confidence"].clone(), json!(0.8));
    h.eq("bdd: rubric dims", rubric["action"]["dimensions"].clone(),
        json!([
            {"question": "rubric_usefulness", "levels": 3, "weight": 0.6},
            {"question": "rubric_clarity", "levels": 4, "weight": 0.4},
        ]));

    let chain = by_name("chain_node");
    h.eq("bdd: chain kind", chain["action"]["kind"].clone(), json!("call"));
    h.eq("bdd: chain list", chain["action"]["chain"].clone(),
        json!([{"capability": "clock", "as": "clock"}]));
    h.eq("bdd: chain with.epoch", chain["action"]["with"]["epoch"].clone(), json!("${with.clock.epoch}"));
    h.eq("bdd: chain project.stamped", chain["action"]["project"]["stamped"].clone(), json!("/text"));
}

pub fn test_bdd_decision_heuristics2(h: &mut Harness) {
    // match_rules / score bands / deterministic answer / answer_field
    let src = r#"
@decision
Feature: heuristics2

  Scenario: rules_node
    Given a choice question "department" asks "Which team?"
      | answer | meaning     |
      | billing | money      |
      | technical | crash    |
    And answer by rules on the field "message" with hit 0.93 and fallback "other"
      | label     | needles                    |
      | billing   | "invoice", "charged"       |
      | technical | "crash", "outage", "500"   |
    When any other answer goes to "STOP"
    Then label "BILLING" when the answer to "department" is "billing"
    And label "PASS" otherwise

  Scenario: score_bands_node
    Given a score question "rubric_usefulness" asks "Useful?"
      | level |
      | poor  |
      | good  |
    And score "rubric_usefulness" when the field "rubric_usefulness_hint" contains "high" scoring 1.8 and "mid" scoring 1.0 miss 0.2
    When any other answer goes to "STOP"
    Then label "HIGH" when "rubric_usefulness" is at least 1.5

  Scenario: answer_node
    Given a choice question "go" asks "Go?"
      | answer | meaning |
      | A      | yes     |
    And the answer is always "A"
    When any other answer goes to "STOP"

  Scenario: answer_field_node
    Given a choice question "billing_contact" asks "Which contact?"
      | answer | meaning |
      | x      | one     |
    And the answer comes from the field "selected_id"
    When any other answer goes to "STOP"
"#;
    let feature = bdd::parse(src, "<h2>").expect("parse heuristics2");
    let spec = bdd::compile_decision(&feature, &json!({})).expect("compile heuristics2");
    let nodes = &spec["nodes"];
    let by_name = |n: &str| {
        nodes.as_array().unwrap().iter().find(|x| x["name"] == n).cloned().unwrap()
    };

    let rules = by_name("rules_node");
    let hr = &rules["questions"]["department"]["heuristic"];
    h.eq("bdd: mr field", hr["field"].clone(), json!("message"));
    h.eq("bdd: mr rules.billing", hr["match_rules"]["billing"].clone(), json!(["invoice", "charged"]));
    h.eq("bdd: mr rules.technical", hr["match_rules"]["technical"].clone(), json!(["crash", "outage", "500"]));
    h.eq("bdd: mr p_hit", hr["p_hit"].clone(), json!(0.93));
    h.eq("bdd: mr fallback", hr["fallback"].clone(), json!("other"));

    let sb = by_name("score_bands_node");
    let hs = &sb["questions"]["rubric_usefulness"]["heuristic"];
    h.eq("bdd: sb field", hs["field"].clone(), json!("rubric_usefulness_hint"));
    h.eq("bdd: sb match_any", hs["match_any"].clone(), json!(["high"]));
    h.eq("bdd: sb match_mid_any", hs["match_mid_any"].clone(), json!(["mid"]));
    h.eq("bdd: sb score_hit", hs["score_hit"].clone(), json!(1.8));
    h.eq("bdd: sb score_mid", hs["score_mid"].clone(), json!(1.0));
    h.eq("bdd: sb score_miss", hs["score_miss"].clone(), json!(0.2));

    let an = by_name("answer_node");
    h.eq("bdd: answer", an["questions"]["go"]["heuristic"].clone(), json!({"answer": "A"}));

    let af = by_name("answer_field_node");
    h.eq("bdd: answer_field", af["questions"]["billing_contact"]["heuristic"].clone(), json!({"answer_field": "selected_id"}));
}

pub fn test_bdd_decision_policy(h: &mut Harness) {
    // policy background step + per-node max_retries/min_confidence/keep_state,
    // and the `Then` keyword guarantee for a node with only When-edge steps.
    let src = r#"
@decision
Feature: policy_workflow

  Background:
    Given the workflow runs at most 6 iterations
    Given the policy
      | allow_exec     | false                |
      | allow_paths    | "${env.LAYA_WORK_DIR}" |
      | max_timeout_ms | 8000                 |
      | max_output     | 65536                |
      | retries        | 1                    |

  Scenario: route
    Given a choice question "channel" asks "Which path?"
      | answer | meaning  |
      | inbox  | email    |
      | api    | webhook  |
    When any other answer goes to "STOP"
    Then keep the answer to "channel"
    Then allow up to 3 retries

  Scenario: probe
    Given a score question "severity" asks "How bad?"
      | level |
      | low   |
      | high  |
    When any other answer goes to "STOP"
    Then label "PAGE" when "severity" is at least 1.5
    And label "BACKLOG" otherwise
    Then require confidence at least 0.4
    Then keep the state key "ticket_id"
"#;
    let feature = bdd::parse(src, "<policy>").expect("parse policy feature");
    let spec = bdd::compile_decision(&feature, &json!({})).expect("compile policy");

    h.eq("bdd: policy max_iterations", spec["max_iterations"].clone(), json!(6));
    h.eq("bdd: policy allow_exec", spec["policy"]["allow_exec"].clone(), json!(false));
    h.eq("bdd: policy allow_paths", spec["policy"]["allow_paths"].clone(),
        json!(["${env.LAYA_WORK_DIR}"]));
    h.eq("bdd: policy max_timeout_ms", spec["policy"]["max_timeout_ms"].clone(), json!(8000));
    h.eq("bdd: policy max_output", spec["policy"]["max_output"].clone(), json!(65536));
    h.eq("bdd: policy retries", spec["policy"]["retries"].clone(), json!(1));

    let nodes = &spec["nodes"];
    let by_name = |n: &str| {
        nodes
            .as_array()
            .unwrap()
            .iter()
            .find(|x| x["name"] == n)
            .cloned()
            .unwrap()
    };
    let route = by_name("route");
    h.eq("bdd: policy node max_retries", route["max_retries"].clone(), json!(3));
    h.eq("bdd: policy node copy_keys", route["action"].clone(),
        json!({"kind": "copy_keys", "keys": ["channel"]}));

    let probe = by_name("probe");
    h.eq("bdd: policy node min_conf", probe["min_confidence"].clone(), json!(0.4));
    h.eq("bdd: policy node keep", probe["state"].clone(), json!({"keep": ["ticket_id"]}));
    h.eq("bdd: policy node threshold", probe["action"]["kind"].clone(), json!("threshold"));

    // Sidecar config policy merges under the Gherkin policy for missing keys.
    let cfg = json!({"policy": {"allow_hosts": ["127.0.0.1"]}});
    let spec2 = bdd::compile_decision(&feature, &cfg).expect("compile policy + sidecar");
    h.eq("bdd: policy merged hosts", spec2["policy"]["allow_hosts"].clone(), json!(["127.0.0.1"]));
    h.eq("bdd: policy merged exec", spec2["policy"]["allow_exec"].clone(), json!(false));

    // No policy authoring at all -> no policy block (round-trip stays clean).
    let bare = r#"
@decision
Feature: bare_workflow

  Scenario: only_node
    Given a choice question "go" asks "Go?"
      | answer | meaning |
      | A      | yes     |
    When any other answer goes to "STOP"
"#;
    let fb = bdd::parse(bare, "<bare>").expect("parse bare feature");
    let sb = bdd::compile_decision(&fb, &json!({})).expect("compile bare");
    h.eq("bdd: bare no policy", sb.get("policy").is_none(), true);

    // ── P168: the policy table's declared values must mean what they say ────
    // The engine reads `allow_exec` as the exact word `true` / `false` (any other
    // spelling — `yes`, `True`, `1` — is silently taken as *false*), and reads the
    // list keys as the *quoted* spans of the cell (bare text is dropped, and an
    // emptied list means *no* restriction). Both were accepted and quietly
    // discarded the author's value; the compiler now refuses them where the raw
    // text is still in hand.
    let with_policy = |rows: &str| {
        format!(
            r#"
@decision
Feature: p168_policy

  Background:
    Given the policy
{rows}
    Given the capabilities
      | x | {{"kind":"passthrough"}} |

  Scenario: a
    Given a choice question "q" asks "ok?"
      | answer | meaning |
      | A      | yes     |
    When any other answer goes to "STOP"
    Then call the capability "x"
"#
        )
    };
    let policy_err = |rows: &str| -> String {
        match bdd::parse(&with_policy(rows), "<p168>")
            .and_then(|f| bdd::compile_decision(&f, &json!({})))
        {
            Ok(_) => String::new(),
            Err(e) => format!("{e:#}"),
        }
    };
    let rows_ok = |rows: &str| policy_err(rows).is_empty();

    let m = policy_err("      | allow_exec | yes |");
    h.check(
        "bdd: policy allow_exec `yes` refused (was silently false)",
        !m.is_empty() && m.contains("allow_exec") && m.contains("true") && m.contains("false"),
    );
    h.check(
        "bdd: policy allow_exec `True` refused",
        !policy_err("      | allow_exec | True |").is_empty(),
    );
    h.check("bdd: policy allow_exec `true` ok", rows_ok("      | allow_exec | true |"));
    h.check("bdd: policy allow_exec `false` ok", rows_ok("      | allow_exec | false |"));
    let m = policy_err("      | allow_exec | true |\n      | allow_hosts | example.com |");
    h.check(
        "bdd: policy unquoted allow_hosts refused (was silently dropped)",
        !m.is_empty() && m.contains("allow_hosts") && m.contains("example.com") && m.contains("quotes"),
    );
    let m2 = policy_err("      | allow_paths | ${env.HOME}/tmp |");
    h.check("bdd: policy unquoted allow_paths refused", !m2.is_empty() && m2.contains("allow_paths"));
    h.check(
        "bdd: policy quoted list ok",
        rows_ok("      | allow_hosts | \"a.com\", \"b.com\" |"),
    );
    h.check("bdd: policy trailing comma ok", rows_ok("      | allow_hosts | \"a.com\", |"));
    h.check("bdd: policy empty list cell ok (round-trips to [])", rows_ok("      | allow_hosts |  |"));
    h.check(
        "bdd: policy quoted path with an inner comma ok",
        rows_ok("      | allow_paths | \"${env.HOME}/a,b\" |"),
    );
}

pub fn test_bdd_decision_ui_proposal(h: &mut Harness) {
    // ui_proposal action + capabilities background step.
    let src = r#"
@decision
Feature: computer_use_bdd

  Background:
    Given the workflow runs at most 6 iterations
    Given the capabilities
      | ui | {"kind": "computer_use", "allowed_surface": "https://x.invalid/fixture"} |

  Scenario: decide
    Given a choice question "operation" asks "Next op?"
      | answer | meaning        |
      | fill   | edit a field   |
      | done   | finished       |
    And a choice question "field" asks "Which field?"
      | answer | meaning |
      | e1     | delivery |
      | e2     | accounts payable |
    And a choice question "amount" asks "Which amount?"
      | answer | meaning |
      | t1     | subtotal |
      | t2     | total   |
    When any other answer goes to "STOP"
    Then prepare a UI proposal from operation "operation" field "field" amount "amount" surface "surface" hash "snapshot_hash" requiring 0.8 value "billing_contact"

  Scenario: step
    Given a choice question "go" asks "Go?"
      | answer | meaning |
      | A      | yes     |
    When any other answer goes to "STOP"
    Then call the capability "ui"
      | with.op | fingerprint |
"#;
    let feature = bdd::parse(src, "<computer_use_bdd>").expect("parse computer_use_bdd feature");
    let spec = bdd::compile_decision(&feature, &json!({})).expect("compile computer_use_bdd");

    h.eq("bdd: ui_proposal capabilities", spec["capabilities"]["ui"]["kind"].clone(), json!("computer_use"));
    h.eq("bdd: ui_proposal max_iterations", spec["max_iterations"].clone(), json!(6));

    let nodes = &spec["nodes"];
    let by_name = |n: &str| {
        nodes.as_array().unwrap().iter().find(|x| x["name"] == n).cloned().unwrap()
    };
    let decide = by_name("decide");
    let act = &decide["action"];
    h.eq("bdd: ui_proposal kind", act["kind"].clone(), json!("ui_proposal"));
    h.eq("bdd: ui_proposal operation", act["operation"].clone(), json!("operation"));
    h.eq("bdd: ui_proposal field", act["field"].clone(), json!("field"));
    h.eq("bdd: ui_proposal amount", act["amount"].clone(), json!("amount"));
    h.eq("bdd: ui_proposal surface", act["surface"].clone(), json!("surface"));
    h.eq("bdd: ui_proposal snapshot_hash", act["snapshot_hash"].clone(), json!("snapshot_hash"));
    h.eq("bdd: ui_proposal min", act["min_confidence"].clone(), json!(0.8));
    h.eq("bdd: ui_proposal value_key", act["value_key"].clone(), json!("billing_contact"));

    let step = by_name("step");
    h.eq("bdd: ui_proposal call kind", step["action"]["kind"].clone(), json!("call"));
    h.eq("bdd: ui_proposal call cap", step["action"]["capability"].clone(), json!("ui"));
}

/// noul `truthy`/`falsy` annotations and `the sub workflow` reference nodes —
/// the two vocabulary additions that closed the dsl round-trip for the noul
/// (laya_mem / codegraph) and nested-pipeline (intake_pipeline_nested) corpora.
pub fn test_bdd_decision_roundtrip_vocab(h: &mut Harness) {
    let src = r#"
@decision
Feature: roundtrip_vocab

  Scenario: route
    Given a noul question "semantic" asks "Would finding related memories help?"
      | truthy | Recall of related facts is useful. |
      | falsy  | No related-memory lookup is needed. |
    When any other answer goes to "STOP"

  Scenario: intake
    Given the sub workflow "support_ticket_router"
"#;
    let feature = bdd::parse(src, "<test>").expect("parse roundtrip vocab");
    let spec = bdd::compile_decision(&feature, &json!({})).expect("compile roundtrip vocab");
    let nodes = spec["nodes"].as_array().unwrap();
    h.eq("bdd: node count", nodes.len(), 2);
    let q = &nodes[0]["questions"]["semantic"];
    h.eq("bdd: noul truthy", q["truthy"].clone(), json!("Recall of related facts is useful."));
    h.eq("bdd: noul falsy", q["falsy"].clone(), json!("No related-memory lookup is needed."));
    let sub = &nodes[1];
    h.eq("bdd: sub workflow node name", sub["name"].clone(), json!("intake"));
    h.eq("bdd: sub workflow ref", sub["workflow"].clone(), json!("support_ticket_router"));
    h.eq("bdd: sub workflow has no primary_q", sub.get("primary_q").cloned().unwrap_or(json!(null)), Value::Null);

    // mixing sub-workflow with node content must be a compile error
    let bad = r#"
@decision
Feature: bad_sub
  Scenario: intake
    Given the sub workflow "support_ticket_router"
    When any other answer goes to "STOP"
"#;
    let bf = bdd::parse(bad, "<test>").expect("parse bad sub");
    h.eq("bdd: sub workflow refuses mixed content", bdd::compile_decision(&bf, &json!({})).is_err(), true);
}

/// `\|` escape in a table cell carries a literal pipe (a shell `df ... | tail`
/// inside a JSON capability config), so it round-trips through `cells`.
pub fn test_bdd_table_pipe_escape(h: &mut Harness) {
    let src = r#"
@decision
Feature: pipe_caps

  Scenario: check
    Given a choice question "ok" asks "ok?"
      | answer | meaning |
      | A      | no      |
      | B      | yes     |
    When any other answer goes to "STOP"
    Then call the capability "disk_check"
      | with.cmd | echo 'x' \| tail -1 |
"#;
    let feature = bdd::parse(src, "<test>").expect("parse pipe escape");
    let spec = bdd::compile_decision(&feature, &json!({})).expect("compile pipe escape");
    let act = &spec["nodes"][0]["action"];
    h.eq("bdd: pipe escaped in cell", act["with"]["cmd"].clone(), json!("echo 'x' | tail -1"));
}

// ─── BDD authoring MCP tools ──────────────────────────────────────────────────

fn bdd_tool(name: &str) -> DynTool {
    laya_workflow::bdd_mcp::BddTools
        .tools()
        .into_iter()
        .find(|t| t.name() == name)
        .unwrap_or_else(|| panic!("missing bdd mcp tool {name}"))
}

/// bdd_validate: a known-good `@decision` feature compiles and validates with
/// 100% step coverage — the exact authoring gate an agent runs before commit.
pub fn test_bdd_mcp_validate_ok(h: &mut Harness) {
    let t = bdd_tool("bdd_validate");
    let src = r#"
@decision
Feature: mcp_validate_ok
  Route by category.

  Scenario: classify
    Given a choice question "category" asks "Which team?"
      | answer | meaning  |
      | A      | first    |
      | B      | second   |
    When any other answer goes to "STOP"
    Then keep the answer to "category"
"#;
    let r = t.call(&json!({"feature_text": src})).unwrap();
    h.eq("bdd_validate: ok true", r["ok"].as_bool().unwrap(), true);
    h.eq("bdd_validate: decision", r["decision"].as_bool().unwrap(), true);
    h.eq("bdd_validate: feature name", r["feature"].clone(), json!("mcp_validate_ok"));
    h.eq("bdd_validate: coverage 100%", r["coverage"]["pct"].as_f64().unwrap(), 100.0);
    h.check("bdd_validate: validate.ok", r["validate"]["ok"].as_bool().unwrap());
    h.check("bdd_validate: scenario ok", r["scenarios"][0]["ok"].as_bool().unwrap());
}

/// bdd_validate: a broken feature fails with ok=false and a line-numbered
/// error — so a mid-authoring agent sees the same red as `bdd build`.
pub fn test_bdd_mcp_validate_err(h: &mut Harness) {
    let t = bdd_tool("bdd_validate");
    let src = r#"
@decision
Feature: mcp_validate_bad

  Scenario: broken
    Given a nonsense step that dec_match rejects
"#;
    let r = t.call(&json!({"feature_text": src})).unwrap();
    h.eq("bdd_validate: broken ok false", r["ok"].as_bool().unwrap(), false);
    h.check("bdd_validate: broken error set", r["validate"]["error"].is_string());
    let err = r["validate"]["error"].as_str().unwrap_or("");
    h.check("bdd_validate: error mentions line", err.contains("line"));
}

/// bdd_validate: no input errors cleanly instead of panicking.
pub fn test_bdd_mcp_validate_no_input(h: &mut Harness) {
    let t = bdd_tool("bdd_validate");
    h.check("bdd_validate: no input errors", t.call(&json!({})).is_err());
}

/// bdd_catalog: the completeness audit behind "convert all known workflows to
/// BDD" — every dsl spec has a @decision feature and vice versa.
pub fn test_bdd_mcp_catalog(h: &mut Harness) {
    let t = bdd_tool("bdd_catalog");
    let r = t.call(&json!({})).unwrap();
    h.eq("bdd_catalog: complete", r["complete"].as_bool().unwrap(), true);
    h.check("bdd_catalog: no orphan specs", r["orphan_specs"].as_array().unwrap().is_empty());
    h.check("bdd_catalog: no features without spec", r["features_without_spec"].as_array().unwrap().is_empty());
    let total = r["total_dsl_specs"].as_u64().unwrap();
    let feats = r["total_decision_features"].as_u64().unwrap();
    h.check("bdd_catalog: dsl specs == decision features", total == feats);
    h.check("bdd_catalog: at least 99 specs", total >= 99);
}

/// `bdd transpile` on an `@decision` feature emits the greenfield spec with
/// dsl_version stripped (matching the checked-in dsl convention) — so an author
/// writes a new .feature, generates its dsl/<cat>/<name>.json in one command,
/// and `bdd round-trip` then proves the pair agree.
pub fn test_bdd_transpile_decision(h: &mut Harness) {
    let dir = std::env::temp_dir().join(format!("laya-bdd-transpile-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let src = r#"
@decision
Feature: transpile_demo
  A minimal one-node decision graph.

  Scenario: classify
    Given a choice question "category" asks "Which team should handle this ticket?"
      | answer    | meaning                     |
      | billing   | invoices, payments, refunds |
      | technical | bugs, outages, system errors|
    When the answer "technical" goes to "STOP"
    And any other answer goes to "STOP"
    Then keep the answer to "category"
"#;
    let fp = dir.join("transpile_demo.feature");
    std::fs::write(&fp, src).unwrap();
    let out = dir.join("out");
    let rc = bdd::transpile_cli(
        &[fp.to_string_lossy().into_owned()],
        Some(out.to_str().unwrap()),
        false,
        false,
        None,
    )
    .expect("transpile @decision feature");
    h.eq("bdd: transpile @decision rc", rc, 0);
    let generated = out.join("transpile_demo.json");
    h.check("bdd: transpile @decision wrote spec", generated.is_file());
    let payload: Value =
        serde_json::from_str(&std::fs::read_to_string(&generated).unwrap()).unwrap();
    h.eq("bdd: transpile @decision name", payload["name"].clone(), json!("transpile_demo"));
    h.check("bdd: transpile @decision dsl_version stripped", payload.get("dsl_version").is_none());
    h.eq(
        "bdd: transpile @decision node count",
        payload["nodes"].as_array().unwrap().len(),
        1,
    );
    let _ = crate::spec::from_spec_with_dir(&payload, None, None).expect("generated spec validates");
    // --check and --list forms both succeed on the same feature.
    let rc2 = bdd::transpile_cli(
        &[fp.to_string_lossy().into_owned()],
        Some(out.to_str().unwrap()),
        false,
        true,
        None,
    )
    .expect("transpile @decision --check");
    h.eq("bdd: transpile @decision --check rc", rc2, 0);
    let rc3 = bdd::transpile_cli(
        &[fp.to_string_lossy().into_owned()],
        None,
        true,
        false,
        None,
    )
    .expect("transpile @decision --list");
    h.eq("bdd: transpile @decision --list rc", rc3, 0);
    let _ = std::fs::remove_dir_all(&dir);
}

/// P34/P35: semantic-validation errors carry a "did you mean ...?" hint naming
/// the closest known identifier, and P35 adds the `.feature` line the offending
/// step sits on — so an agent author sees both *what* to change and *where*.
/// Each case compiles cleanly (100% step coverage) but fails
/// `validate_graph_semantics` on one typo.
pub fn test_bdd_validate_suggestion(h: &mut Harness) {
    // (label, feature source, unique snippet of the offending line)
    let cases: Vec<(&str, &str, &str)> = vec![
        (
            "routing target",
            r#"@decision
Feature: s_routing
  Scenario: root
    Given a choice question "intent" asks "what?"
      | answer | meaning |
      | billing | invoices |
      | shipping | parcels |
    When the answer "billing" goes to "biling"
    And any other answer goes to "STOP"
    Then label "ok" otherwise
    Then allow up to 1 retries"#,
            r#"goes to "biling""#,
        ),
        (
            "primary question",
            r#"@decision
Feature: s_primaryq
  Scenario: root
    Given a choice question "intent" asks "what?"
      | answer | meaning |
      | billing | invoices |
      | shipping | parcels |
    When any other answer goes to "STOP"
    Then the primary question is "intet"
    Then label "ok" otherwise
    Then allow up to 1 retries"#,
            r#"the primary question is "intet""#,
        ),
        (
            "question ref in rule",
            r#"@decision
Feature: s_qref
  Scenario: root
    Given a choice question "intent" asks "what?"
      | answer | meaning |
      | billing | invoices |
      | shipping | parcels |
    When any other answer goes to "STOP"
    Then label "ok" when the answer to "intet" is "billing"
    Then label "nope" otherwise
    Then allow up to 1 retries"#,
            r#"the answer to "intet""#,
        ),
        (
            "answer edge key",
            r#"@decision
Feature: s_answer
  Scenario: root
    Given a choice question "intent" asks "what?"
      | answer | meaning |
      | billing | invoices |
      | shipping | parcels |
    When the answer "billng" goes to "STOP"
    And any other answer goes to "STOP"
    Then label "ok" otherwise
    Then allow up to 1 retries"#,
            r#"the answer "billng""#,
        ),
        (
            "gate option",
            r#"@decision
Feature: s_gateopt
  Scenario: root
    Given a choice question "verdict" asks "what verdict?"
      | answer | meaning |
      | approve | good |
      | reject | bad |
    When any other answer goes to "STOP"
    Then gate "verdict" on option "aprove" with block 0.6 allow 0.7"#,
            r#"on option "aprove""#,
        ),
        (
            "combine fallback",
            r#"@decision
Feature: s_combine
  Scenario: root
    Given a choice question "department" asks "which dept?"
      | answer | meaning |
      | billing | money |
      | support | product |
      | other | none |
    And a score question "explicit_urgency" asks "urgency?"
      | level |
      | low |
      | high |
    When any other answer goes to "STOP"
    Then combine "department" with signal "explicit_urgency" fallback "otrher" review "human_review" requiring 0.8 high 0.85 low 0.15"#,
            r#"fallback "otrher""#,
        ),
        (
            "dynamic criteria key",
            r#"@decision
Feature: s_dynkey
  Scenario: select
    Given a choice question "pick" asks "which candidate?"
      | answer | meaning |
      | from_state | candidates |
      | id_keey | id |
      | none | nothing |
    And the answer comes from the field "selected_id"
    When any other answer goes to "STOP"
    Then allow up to 1 retries"#,
            r#"a choice question "pick" asks"#,
        ),
    ];
    for (what, src, snippet) in cases {
        let feature = bdd::parse(src, "<sug>").expect("suggestion feature parses");
        let spec =
            bdd::compile_decision(&feature, &json!({})).expect("suggestion feature compiles");
        let err = crate::spec::from_spec_with_dir(&spec, None, None)
            .err()
            .expect("suggestion case must fail validation")
            .to_string();
        h.check(
            &format!("bdd: suggestion [{what}] carries did-you-mean hint"),
            err.contains("did you mean"),
        );
        h.check(
            &format!("bdd: suggestion [{what}] hints at a concrete value"),
            err.contains('"'),
        );
        // P35: the locator must return the offending step's real line.
        let expected = src
            .lines()
            .position(|l| l.contains(snippet))
            .map(|n| n + 1)
            .expect("snippet present in source");
        h.eq(
            &format!("bdd: suggestion [{what}] locates the offending line"),
            bdd::locate_semantic_error_line(&feature, &err),
            Some(expected),
        );
    }
    // Negative: a name that is far from every candidate must NOT get a guess —
    // no false-positive hint on the error path.
    let no_hint = r#"@decision
Feature: s_far
  Scenario: root
    Given a choice question "intent" asks "what?"
      | answer | meaning |
      | billing | invoices |
      | shipping | parcels |
    When the answer "complete" goes to "STOP"
    And any other answer goes to "STOP"
    Then label "ok" otherwise
    Then allow up to 1 retries"#;
    let feature = bdd::parse(no_hint, "<sug>").expect("far feature parses");
    let spec = bdd::compile_decision(&feature, &json!({})).expect("far feature compiles");
    let err = crate::spec::from_spec_with_dir(&spec, None, None)
        .err()
        .expect("far case must fail validation")
        .to_string();
    h.check(
        "bdd: suggestion far-from-everything has no hint",
        !err.contains("did you mean"),
    );
    // Even with no hint, the offending answer key still localises to its line.
    h.eq(
        "bdd: suggestion far-from-everything still locates the line",
        bdd::locate_semantic_error_line(&feature, &err),
        Some(8),
    );

    // P39: an undeclared `call` capability is a semantic error too, and it must
    // localise to the `call the capability "..."` step (not the Scenario header)
    // with a did-you-mean hint when a near-miss capability IS declared.
    let cap_src = r#"@decision
Feature: s_cap
  Scenario: capcheck
    Given a choice question "done" asks "digest?"
      | answer | meaning |
      | A      | yes     |
      | B      | no      |
    When any other answer goes to "STOP"
    Then call the capability "digestt""#;
    let cap_feature = bdd::parse(cap_src, "<sugcap>").expect("cap feature parses");
    let want_line = cap_src
        .lines()
        .position(|l| l.contains(r#"call the capability "digestt""#))
        .map(|n| n + 1);
    // near-miss declared ("digest") -> did-you-mean hint + exact line
    let cap_spec = bdd::compile_decision(
        &cap_feature,
        &json!({"capabilities": {"digest": {"kind": "plugin", "plugin": "websites/x"}}}),
    )
    .expect("cap feature compiles");
    let cap_err = crate::spec::from_spec_with_dir(&cap_spec, None, None)
        .err()
        .expect("undeclared capability must fail validation")
        .to_string();
    h.check(
        "bdd: suggestion [call capability] carries did-you-mean hint",
        cap_err.contains("did you mean"),
    );
    h.eq(
        "bdd: suggestion [call capability] locates the call line",
        bdd::locate_semantic_error_line(&cap_feature, &cap_err),
        want_line,
    );
    // nothing declared -> no hint, but the line still resolves
    let cap_bare = bdd::compile_decision(&cap_feature, &json!({})).expect("cap feature compiles (bare)");
    let cap_bare_err = crate::spec::from_spec_with_dir(&cap_bare, None, None)
        .err()
        .expect("bare undeclared capability must fail validation")
        .to_string();
    h.check(
        "bdd: suggestion [call capability bare] has no hint",
        !cap_bare_err.contains("did you mean"),
    );
    h.eq(
        "bdd: suggestion [call capability bare] still locates the call line",
        bdd::locate_semantic_error_line(&cap_feature, &cap_bare_err),
        want_line,
    );
}

/// `bdd scaffold` generates a minimal valid `@decision` skeleton (already
/// passing `bdd build`), so an agent starts the authoring loop from a legal
/// file instead of a blank page. Asserts on the pure `scaffold_body` output
/// and on the self-verifying CLI path (stdout mode, no filesystem writes).
pub fn test_bdd_scaffold(h: &mut Harness) {
    let body = bdd::scaffold_body(
        "intake_triage",
        "routing",
        "option",
        "Route an intake request to the owning team or escalate",
        2,
        false,
    );
    h.check("bdd: scaffold has @decision tag", body.contains("@decision\n"));
    h.check("bdd: scaffold has Feature name", body.contains("Feature: intake_triage\n"));
    h.check("bdd: scaffold has intent as description", body.contains("Route an intake request to the owning team or escalate"));
    h.check("bdd: scaffold has starter question", body.contains("Given a choice question \"option\" asks \"Choose an option\""));
    h.check("bdd: scaffold keeps the answer", body.contains("Then keep the answer to \"option\""));
    // 2-node canonical starter: a score node with threshold labels is present.
    h.check("bdd: scaffold has score node", body.contains("Given a score question \"severity\" asks \"How severe is the impact?\""));
    h.check("bdd: scaffold has threshold label", body.contains("label \"PAGE_ONCALL\" when \"severity\" is at least 1.5"));
    // The skeleton must compile to a valid two-node decision graph.
    let feature = bdd::parse(&body, "<scaffold>").expect("scaffold parses");
    let spec = bdd::compile_decision(&feature, &json!({})).expect("scaffold compiles");
    h.eq("bdd: scaffold node count", spec["nodes"].as_array().unwrap().len(), 2);
    h.eq("bdd: scaffold start", spec["start"].clone(), json!("intake_triage"));
    let _ = crate::spec::from_spec_with_dir(&spec, None, None).expect("scaffold validates");
    // --single emits a 1-node start that is still valid.
    let single = bdd::scaffold_body("intake_triage", "routing", "option", "one node", 1, false);
    let f1 = bdd::parse(&single, "<scaffold>").expect("single parses");
    let s1 = bdd::compile_decision(&f1, &json!({})).expect("single compiles");
    h.eq("bdd: scaffold --single node count", s1["nodes"].as_array().unwrap().len(), 1);
    let _ = crate::spec::from_spec_with_dir(&s1, None, None).expect("single validates");
    // P37: named patterns beyond single/two_node/nested — gate + loop — each
    // must compile to a valid, fully-wired decision graph. Unknown names error.
    for pat in ["gate", "loop", "dynamic"] {
        let body = bdd::scaffold_body_pattern("pat_demo", "routing", "risk", "pattern demo", pat)
            .unwrap_or_else(|e| panic!("pattern {pat} builds: {e}"));
        let f = bdd::parse(&body, "<pat>").unwrap_or_else(|e| panic!("pattern {pat} parses: {e}"));
        let sp = bdd::compile_decision(&f, &json!({}))
            .unwrap_or_else(|e| panic!("pattern {pat} compiles: {e}"));
        let _ = crate::spec::from_spec_with_dir(&sp, None, None)
            .unwrap_or_else(|e| panic!("pattern {pat} validates: {e:#}"));
        h.check(
            &format!("bdd: scaffold pattern [{pat}] self-validates"),
            sp["nodes"].as_array().map(|a| !a.is_empty()).unwrap_or(false),
        );
    }
    // gate pattern carries a gate action; loop pattern routes back to itself.
    let gate = bdd::scaffold_body_pattern("g", "routing", "risk", "d", "gate").unwrap();
    h.check("bdd: gate scaffold has a gate action", gate.contains("Then gate \"risk\" on option \"B\""));
    let lp = bdd::scaffold_body_pattern("l", "loops", "v", "d", "loop").unwrap();
    h.check("bdd: loop scaffold routes back to itself", lp.contains("And the answer \"revise\" goes to \"l\""));
    let dynb = bdd::scaffold_body_pattern("dyn", "routing", "candidate", "d", "dynamic").unwrap();
    h.check(
        "bdd: dynamic scaffold carries the from_state table dialect",
        dynb.contains("| from_state | candidates |")
            && dynb.contains("| id_key | id |")
            && dynb.contains("| value_key | value |")
            && dynb.contains("| none |"),
    );
    h.check(
        "bdd: dynamic scaffold extracts candidates via a capability",
        dynb.contains("Then call the capability"),
    );
    h.check(
        "bdd: dynamic scaffold documents itself as the dynamic pattern",
        dynb.contains("Pattern: dynamic"),
    );
    h.check("bdd: unknown pattern is rejected", bdd::scaffold_body_pattern("x", "routing", "q", "d", "nope").is_err());

    // CLI stdout mode is self-verifying and returns 0.
    let rc = bdd::scaffold(&bdd::ScaffoldOptions {
        name: "intake_triage".into(),
        cat: "routing".into(),
        intent: None,
        stdout: true,
        force: false,
        primary_q: String::new(),
        single: false,
        nested: false,
        pattern: None,
    })
    .expect("scaffold CLI runs");
    h.eq("bdd: scaffold stdout rc", rc, 0);

    // --nested emits a 3-scenario nested-pipeline starter: prefilter -> sub
    // workflow (with completion handoff) -> audit. Must compile, validate
    // (sub ref resolves, all nodes reachable through the continuation) and be
    // 100% covered.
    let nested = bdd::scaffold_body("pipeline_flow", "pipelines", "is_noise", "nested pipeline", 2, true);
    h.check("bdd: nested scaffold has continuation", nested.contains("when the sub workflow completes continue at"));
    h.check("bdd: nested scaffold has audit labels", nested.contains("label \"AUDIT_REQUIRED\""));
    let f2 = bdd::parse(&nested, "<scaffold>").expect("nested parses");
    let s2 = bdd::compile_decision(&f2, &json!({})).expect("nested compiles");
    h.eq("bdd: nested scaffold node count", s2["nodes"].as_array().unwrap().len(), 3);
    // sub ref must resolve to the existing support_ticket_router + validate
    // (reachability through the continuation holds).
    let _ = crate::spec::from_spec_with_dir(&s2, None, None).expect("nested validates");
}

/// `bdd_new` MCP tool: from an intent, generate a minimal valid `@decision`
/// `.feature` skeleton — the greenfield start of the in-process authoring loop
/// (new → validate → extend → audit). Asserts the returned text compiles, and
/// that `write=true` persists to the requested root without touching the repo.
pub fn test_bdd_mcp_new(h: &mut Harness) {
    let t = bdd_tool("bdd_new");
    let r = t
        .call(&json!({
            "name": "intake_triage",
            "cat": "routing",
            "intent": "Route an intake request to the owning team or escalate",
        }))
        .unwrap();
    h.eq("bdd_new: valid", r["valid"].as_bool().unwrap(), true);
    h.eq("bdd_new: name", r["name"].clone(), json!("intake_triage"));
    h.eq("bdd_new: node count (2-node default)", r["nodes"].as_u64().unwrap(), 2);
    let text = r["feature_text"].as_str().unwrap_or("");
    h.check("bdd_new: @decision tag", text.contains("@decision\n"));
    h.check("bdd_new: Feature line", text.contains("Feature: intake_triage\n"));
    h.check("bdd_new: intent as description", text.contains("Route an intake request to the owning team or escalate"));
    h.check("bdd_new: score node in default", text.contains("Given a score question \"severity\" asks"));
    h.check(
        "bdd_new: next hands off to bdd_vocabulary",
        r["next"]
            .as_array()
            .unwrap()
            .iter()
            .any(|v| v.as_str().unwrap_or("").contains("bdd_vocabulary")),
    );
    h.check("bdd_new: write=false by default", r["written"].is_null());
    // single=true emits a 1-node start.
    let r1 = t.call(&json!({"name": "intake_triage", "single": true})).unwrap();
    h.eq("bdd_new: single node count", r1["nodes"].as_u64().unwrap(), 1);
    // The returned text must itself pass the authoring gate.
    let v = bdd_tool("bdd_validate").call(&json!({"feature_text": text})).unwrap();
    h.eq("bdd_new: skeleton passes bdd_validate", v["ok"].as_bool().unwrap(), true);
    // write=true persists under an explicit root (temp dir), not the repo.
    let root = std::env::temp_dir().join(format!("laya-bdd-new-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    let r2 = t
        .call(&json!({"name": "intake_triage", "cat": "routing", "write": true, "root": root.to_string_lossy().into_owned()}))
        .unwrap();
    h.eq("bdd_new: written path set", r2["written"].is_string(), true);
    let dest = r2["written"].as_str().unwrap().to_string();
    h.check("bdd_new: file on disk", std::path::Path::new(&dest).is_file());
    h.check("bdd_new: file under root", dest.starts_with(&root.to_string_lossy().into_owned()));
    // Second write refuses (exists=true) without overwriting.
    let r3 = t
        .call(&json!({"name": "intake_triage", "cat": "routing", "write": true, "root": root.to_string_lossy().into_owned()}))
        .unwrap();
    h.eq("bdd_new: exists flag on second write", r3["exists"].as_bool().unwrap(), true);
    h.eq("bdd_new: not overwritten", r3["written"].is_null(), true);
    // Bad input: empty name errors instead of panicking.
    h.check("bdd_new: empty name errors", t.call(&json!({"name": ""})).is_err());
    // nested=true emits the nested-pipeline starter (3 nodes, valid).
    let rn = t
        .call(&json!({"name": "pipeline_flow", "cat": "pipelines", "nested": true}))
        .unwrap();
    h.eq("bdd_new: nested valid", rn["valid"].as_bool().unwrap(), true);
    h.eq("bdd_new: nested node count", rn["nodes"].as_u64().unwrap(), 3);
    h.check(
        "bdd_new: nested has continuation",
        rn["feature_text"].as_str().unwrap_or("").contains("completes continue at"),
    );
    let _ = std::fs::remove_dir_all(&root);
}

/// The behavioural-smoke data: every expected label must correspond to a
/// registered sample state, and the number of labelled expectation sets must
/// not silently regress below the pinned floor (38). This guards the
/// correctness coverage that `bdd dsl smoke` enforces per-run.
pub fn test_bdd_smoke_expect_coverage(h: &mut Harness) {
    let root = bdd::repo_root();
    let expect: Value = serde_json::from_str(
        &std::fs::read_to_string(root.join("bench").join("dsl_smoke_expect.json")).unwrap(),
    )
    .unwrap();
    let states: Value = serde_json::from_str(
        &std::fs::read_to_string(root.join("bench").join("dsl_smoke_states.json")).unwrap(),
    )
    .unwrap();
    let expect_obj = expect.as_object().unwrap();
    let states_obj = states.as_object().unwrap();
    h.check("bdd: smoke expect stems >= 55", expect_obj.len() >= 55);
    // Stems that pin decision outputs by dotted path (object-valued expects)
    // must not silently regress either; the per-run smoke gate enforces the
    // actual values.
    let field_assert_stems = expect_obj
        .values()
        .filter(|v| {
            v.as_object()
                .map(|m| m.values().any(|x| x.is_object()))
                .unwrap_or(false)
        })
        .count();
    h.check("bdd: smoke expect field-assert stems >= 17", field_assert_stems >= 17);
    // Every expect stem must have registered states, and every expected label
    // must be one of that stem's state keys (no dangling expectations).
    for (stem, emap) in expect_obj {
        h.check(&format!("bdd: expect stem {stem} has states"), states_obj.contains_key(stem));
        if let Some(smap) = states_obj.get(stem).and_then(|v| v.as_object()) {
            for label in emap.as_object().unwrap().keys() {
                h.check(&format!("bdd: expect label {stem}:{label} has a state"), smap.contains_key(label));
            }
        }
    }
}


/// P31 decision-trace structural gate: every registered decision state that
/// walks more than one node must pin its route in the expect table
/// (trace.steps.N.node/.action + trace.step_count). A final label alone lets
/// an authored feature route through wrong intermediate nodes and still pass;
/// this test runs the engine exactly like `dsl smoke` (network-bound specs
/// skip offline via network_bound_offline) and fails any multi-node state whose
/// expect omits the trace path. The static floor (17) guards the pinned set
/// from silently shrinking; it is mode-independent because it reads the table,
/// not the network.
pub fn test_bdd_smoke_trace_path_floor(h: &mut Harness) {
    use laya_workflow::backend::HeuristicBackend;
    use laya_workflow::dsl_smoke::{ensure_env_dirs, network_bound_external, network_bound_offline};
    let root = bdd::repo_root();
    // Mirror dsl_smoke's startup so specs referencing ${env.X}/${secret.X}
    // (e.g. support_ticket_router_hybrid's allow_paths) resolve exactly as
    // they do in the smoke harness: scratch env vars + a fresh secrets
    // snapshot before any spec loads.
    if let Ok(env) = ensure_env_dirs() {
        for (k, v) in &env {
            std::env::set_var(k, v);
        }
    }
    let _ = laya_workflow::capability::secret::init(None);
    let expect: Value = serde_json::from_str(
        &std::fs::read_to_string(root.join("bench").join("dsl_smoke_expect.json")).unwrap(),
    )
    .unwrap();
    let states: Value = serde_json::from_str(
        &std::fs::read_to_string(root.join("bench").join("dsl_smoke_states.json")).unwrap(),
    )
    .unwrap();
    let expect_obj = expect.as_object().unwrap();
    let states_obj = states.as_object().unwrap();

    // Static floor: stems whose expect pins at least one state's trace path.
    // P31 pinned 15 multi-node stems + web_research + intake_pipeline_nested
    // (pre-existing) = 17.
    let trace_pinned_stems = expect_obj
        .values()
        .filter(|v| {
            v.as_object()
                .map(|m| {
                    m.values().any(|x| {
                        x.as_object()
                            .map(|o| o.contains_key("trace.step_count"))
                            .unwrap_or(false)
                    })
                })
                .unwrap_or(false)
        })
        .count();
    h.check("bdd: trace-pinned stems >= 17", trace_pinned_stems >= 17);

    // P38 static floor: the corpus pins a routing *answer* at every step (on
    // top of node+action), not just the first hop. Count the pinned
    // trace.steps.N.answer keys across all stems.
    let answer_pins: usize = expect_obj
        .values()
        .filter_map(Value::as_object)
        .flat_map(|m| m.values())
        .filter_map(Value::as_object)
        .map(|o| {
            o.keys()
                .filter(|k| k.starts_with("trace.steps.") && k.ends_with(".answer"))
                .count()
        })
        .sum();
    h.check("bdd: trace answer pins >= 60", answer_pins >= 60);

    // P38 pure checks on the shared gate: a first-hop-only pin is rejected (the
    // pre-P38 gate accepted it), a fully pinned path is accepted, and a missing
    // object (string label) for a multi-node walk is rejected wholesale.
    let first_hop_only = serde_json::json!({
        "trace.step_count": 3,
        "trace.steps.0.node": "a",
    });
    h.check(
        "bdd: first-hop-only trace pin is rejected",
        !laya_workflow::dsl_smoke::missing_trace_pins(&first_hop_only, 3).is_empty(),
    );
    h.check(
        "bdd: node-only path (no action/answer) is rejected",
        !laya_workflow::dsl_smoke::missing_trace_pins(
            &serde_json::json!({
                "trace.step_count": 2,
                "trace.steps.0.node": "a", "trace.steps.1.node": "b",
            }),
            2,
        )
        .is_empty(),
    );
    h.check(
        "bdd: fully pinned trace path is accepted",
        laya_workflow::dsl_smoke::missing_trace_pins(
            &serde_json::json!({
                "trace.step_count": 2,
                "trace.steps.0.node": "a", "trace.steps.0.action": "route", "trace.steps.0.answer": "x",
                "trace.steps.1.node": "b", "trace.steps.1.action": "stop", "trace.steps.1.answer": "A",
            }),
            2,
        )
        .is_empty(),
    );
    h.check(
        "bdd: string label for a multi-node walk is rejected",
        !laya_workflow::dsl_smoke::missing_trace_pins(&Value::String("PASS".into()), 2).is_empty(),
    );

    // Runtime structural: discover dsl specs, run every registered state,
    // and require multi-node states to pin their trace path.
    let dsl_root = root.join("dsl");
    let mut specs = Vec::new();
    let mut stack = vec![dsl_root.clone()];
    while let Some(dir) = stack.pop() {
        for entry in std::fs::read_dir(&dir).unwrap() {
            let p = entry.unwrap().path();
            if p.is_dir() {
                stack.push(p);
            } else if p.extension().and_then(|e| e.to_str()) == Some("json")
                && !p
                    .file_name()
                    .map(|n| n.to_string_lossy().ends_with(".config.json"))
                    .unwrap_or(false)
            {
                specs.push(p);
            }
        }
    }
    specs.sort();
    let mut multi_states = 0usize;
    for spec in &specs {
        let stem = spec.file_stem().and_then(|s| s.to_str()).unwrap_or("").to_string();
        let Some(smap) = states_obj.get(&stem).and_then(|v| v.as_object()) else {
            continue;
        };
        let raw_spec: Value = std::fs::read_to_string(spec)
            .ok()
            .and_then(|t| serde_json::from_str(&t).ok())
            .unwrap_or(Value::Null);
        let wf = match laya_workflow::spec::load_file(&spec.to_string_lossy()) {
            Ok(w) => w,
            Err(e) => {
                h.check(&format!("bdd: trace {stem} spec loads ({e})"), false);
                continue;
            }
        };
        for (label, st) in smap {
            // Mirror dsl smoke's P33 rule: offline harness never performs
            // external internet I/O, so a spec that is network-bound to an
            // external host has its states skipped (validate-only), exactly
            // like `dsl smoke`. hf_trending (huggingface.co via plugin) would
            // otherwise hang the test for ~16 s.
            if network_bound_external(&raw_spec) {
                continue;
            }
            match wf.run(&HeuristicBackend, st) {
                Ok(out) => {
                    let steps = out
                        .to_json()
                        .pointer("/trace/steps")
                        .and_then(Value::as_array)
                        .cloned()
                        .unwrap_or_default();
                    if steps.len() > 1 {
                        multi_states += 1;
                        let want = expect_obj
                            .get(&stem)
                            .and_then(|v| v.as_object())
                            .and_then(|m| m.get(label))
                            .cloned()
                            .unwrap_or(Value::Null);
                        let missing =
                            laya_workflow::dsl_smoke::missing_trace_pins(&want, steps.len());
                        h.check(
                            &format!("bdd: multi-node {stem}:{label} pins full trace path"),
                            missing.is_empty(),
                        );
                        // P38 load-bearing negative: corrupt one pinned routing
                        // answer and prove the shared check_expect catches it —
                        // the answer pin actually discriminates, not just exists.
                        if let Some(k) = want.as_object().and_then(|o| {
                            o.keys()
                                .find(|k| k.starts_with("trace.steps.") && k.ends_with(".answer"))
                                .cloned()
                        }) {
                            let mut tampered = want.clone();
                            tampered[k.as_str()] = Value::String("P38-TAMPER".into());
                            let caught =
                                !laya_workflow::dsl_smoke::check_expect(&out, &tampered).is_empty();
                            h.check(
                                &format!("bdd: tampering {stem}:{label}.{k} is caught"),
                                caught,
                            );
                        }
                    }
                }
                Err(_e) => {
                    // Offline harness: a network-bound spec whose endpoints are
                    // unreachable (or that depends on an external host in this
                    // offline run) skips its states (validate-only), exactly
                    // like `dsl smoke`. The mocks-up integration run checks
                    // those for real.
                    if !(network_bound_offline(&raw_spec)
                        || network_bound_external(&raw_spec))
                    {
                        h.check(
                            &format!("bdd: trace {stem}:{label} runs offline"),
                            false,
                        );
                    }
                }
            }
        }
    }
    h.check("bdd: multi-node trace states > 0", multi_states > 0);
}

/// P126: the *coverage* half of the decision-state gate. `missing_trace_pins`
/// only ever judges a state whose expect entry **exists**; a state whose entry
/// is missing (or `{}`) ran and was asserted against nothing, and the gate
/// stayed green because `check_expect` was never reached. Measured before this
/// test existed: `routing` (4 states), `traversal` (3) and one
/// `support_ticket_router` label ran 8 states against nothing, and the only
/// thing the corpus pinned about the first two was their *existence*.
///
/// This walks the real corpus exactly like `dsl smoke` (same env bootstrap, same
/// offline-external / platform skip) and requires `uncovered_state` to come back
/// empty, so the regression and the gate cannot silently disagree; it then
/// re-runs the pure rule on synthetic inputs, including the load-bearing
/// negative — drop a real state's expect entry and it must be named.
pub fn test_dsl_smoke_unasserted_state(h: &mut Harness) {
    use laya_workflow::backend::HeuristicBackend;
    use laya_workflow::dsl_smoke::{
        ensure_env_dirs, network_bound_external, platform_bound_here, uncovered_state,
    };
    let root = bdd::repo_root();
    if let Ok(env) = ensure_env_dirs() {
        for (k, v) in &env {
            std::env::set_var(k, v);
        }
    }
    let _ = laya_workflow::capability::secret::init(None);
    let load = |p: &str| -> Value {
        serde_json::from_str(&std::fs::read_to_string(root.join(p)).unwrap()).unwrap()
    };
    let states_obj = load("bench/dsl_smoke_states.json");
    let expect_obj = load("bench/dsl_smoke_expect.json");
    let states_obj = states_obj.as_object().unwrap();
    let expect_obj = expect_obj.as_object().unwrap();
    let want_of = |stem: &str, label: &str| -> Option<Value> {
        expect_obj.get(stem)?.as_object()?.get(label).cloned()
    };

    // The pure rule, on synthetic inputs.
    h.check(
        "dsl-smoke: a label string covers its state",
        uncovered_state("s", "l", Some(&json!("PASS")), 1).is_none(),
    );
    h.check(
        "dsl-smoke: a dotted-path object covers its state",
        uncovered_state("s", "l", Some(&json!({"result.x": 1})), 1).is_none(),
    );
    h.check(
        "dsl-smoke: an empty object is still uncovered",
        uncovered_state("s", "l", Some(&json!({})), 1).is_some(),
    );
    h.check(
        "dsl-smoke: a missing expect entry is uncovered",
        uncovered_state("s", "l", None, 1).is_some(),
    );
    h.check(
        "dsl-smoke: a blank label is uncovered",
        uncovered_state("s", "l", Some(&json!("  ")), 1).is_some(),
    );
    h.check(
        "dsl-smoke: a multi-node state is left to missing_trace_pins",
        uncovered_state("s", "l", None, 3).is_none(),
    );

    // The corpus walk, mirroring `dsl smoke`.
    let dsl_root = root.join("dsl");
    let mut specs = Vec::new();
    let mut stack = vec![dsl_root.clone()];
    while let Some(dir) = stack.pop() {
        for entry in std::fs::read_dir(&dir).unwrap() {
            let p = entry.unwrap().path();
            if p.is_dir() {
                stack.push(p);
            } else if p.extension().and_then(|e| e.to_str()) == Some("json")
                && !p
                    .file_name()
                    .map(|n| n.to_string_lossy().ends_with(".config.json"))
                    .unwrap_or(false)
            {
                specs.push(p);
            }
        }
    }
    specs.sort();
    let mut ran = 0usize;
    let mut single = 0usize;
    let mut holes: Vec<String> = Vec::new();
    for spec in &specs {
        let stem = spec.file_stem().and_then(|s| s.to_str()).unwrap_or("").to_string();
        let Some(smap) = states_obj.get(&stem).and_then(|v| v.as_object()) else {
            continue;
        };
        let raw_spec: Value = std::fs::read_to_string(spec)
            .ok()
            .and_then(|t| serde_json::from_str(&t).ok())
            .unwrap_or(Value::Null);
        if network_bound_external(&raw_spec) || platform_bound_here(&raw_spec) {
            continue;
        }
        let wf = match laya_workflow::spec::load_file(&spec.to_string_lossy()) {
            Ok(w) => w,
            Err(_) => continue,
        };
        for (label, st) in smap {
            let Ok(out) = wf.run(&HeuristicBackend, st) else {
                continue;
            };
            let steps = out
                .to_json()
                .pointer("/trace/steps")
                .and_then(Value::as_array)
                .map(|a| a.len())
                .unwrap_or(0);
            ran += 1;
            if steps == 1 {
                single += 1;
            }
            let want = want_of(&stem, label);
            if let Some(m) = uncovered_state(&stem, label, want.as_ref(), steps) {
                holes.push(m);
            }
        }
    }
    h.check(
        &format!("dsl-smoke: {ran} labelled states ran and every one asserts something"),
        holes.is_empty(),
    );
    for m in &holes {
        h.check(&format!("dsl-smoke: uncovered state — {m}"), false);
    }
    h.check("dsl-smoke: the coverage walk ran states", ran > 0);
    // The hole P126 closed is exactly the single-node one; floor it so the walk
    // cannot quietly stop visiting them.
    h.check(
        &format!("dsl-smoke: single-node states are pinned too (saw {single})"),
        single >= 8,
    );

    // Load-bearing negative: a real state is covered, and is *named* the moment
    // its entry disappears.
    let (stem, label) = ("traversal", "relevant");
    h.check(
        "dsl-smoke: negative — dropping a real expect entry is named",
        uncovered_state(stem, label, None, 1)
            .map(|m| m.contains(stem) && m.contains(label))
            .unwrap_or(false),
    );
    h.check(
        "dsl-smoke: negative — the same state is covered when its entry is present",
        uncovered_state(stem, label, want_of(stem, label).as_ref(), 1).is_none(),
    );
}

/// P33 offline-external skip: a spec that is network-bound to an external
/// (non-local) host must be classified so the offline smoke skips its states
/// (validate-only) instead of performing real internet I/O. Before this, the
/// `hf_trending` plugin spec reached huggingface.co for real and cost ~16 s
/// per gate run. A comment that only *mentions* a URL (cua's github link) must
/// NOT be misclassified, and local-mock endpoints must stay runnable.
pub fn test_bdd_smoke_offline_external_skip(h: &mut Harness) {
    use laya_workflow::capability::plugin::plugin_script_network_targets;
    use laya_workflow::dsl_smoke::network_bound_external;
    let root = bdd::repo_root();
    let load = |p: &str| -> Value {
        serde_json::from_str(&std::fs::read_to_string(root.join(p)).unwrap()).unwrap()
    };

    // Static script scan: the hf-trending plugin's http_get reaches
    // huggingface.co:443.
    let tgt = plugin_script_network_targets("websites/hf-trending");
    h.check(
        "p33: hf-trending plugin scans to huggingface.co",
        tgt.iter().any(|(h, p)| h == "huggingface.co" && *p == 443),
    );
    let hf = load("dsl/capabilities/hf_trending.json");
    h.check("p33: hf_trending spec is network-bound external", network_bound_external(&hf));

    // cua's plugin only *mentions* github.com in a comment, never calls
    // http_get → must not be classified external (it is a trace-pinned stem
    // and must keep running offline).
    let cua = load("dsl/capabilities/cua_desktop.json");
    h.check(
        "p33: cua_desktop comment URL not misclassified",
        !network_bound_external(&cua),
    );

    // A local-mock spec (127.0.0.1) is not external; it is offline only when
    // its local listener is absent.
    let refund = load("dsl/capabilities/refund_with_external_checks.json");
    h.check(
        "p33: local mock spec not external",
        !network_bound_external(&refund),
    );

    // End to end: the offline smoke skips hf_trending's state (exit 0) and
    // reports the external-network skip instead of running it.
    let mut bin = std::env::current_exe().expect("test binary path");
    bin.pop();
    if bin.ends_with("deps") {
        bin.pop();
    }
    bin.push("laya-workflow");
    let out = std::process::Command::new(&bin)
        .args(["dsl", "smoke", "--filter", "hf_trending"])
        .current_dir(&root)
        .output()
        .expect("run dsl smoke");
    h.check("p33: hf_trending offline smoke exits 0", out.status.success());
    let err = String::from_utf8_lossy(&out.stderr).to_string();
    h.check(
        "p33: hf_trending state skipped external",
        err.contains("external network target(s)"),
    );

    // network_bound_offline's P29 contract (local-mock spec offline when its
    // listener is gone) is exercised by the P31 trace-path-floor test and the
    // mocks-down integration run; not repeated here.
}

/// P196: a capability whose optional on-device asset is absent must be skipped
/// validate-only, not reported as a regression. `needle` is the only such
/// capability (engine `libneedle.so` + the 35 MB `needle3.cact`), and
/// `ensure_env_dirs` injects a `NEEDLE3_CACT` placeholder that lets the graph
/// *start* — so before this, `scripts/bdd/check.sh` (run by CI on a browserless,
/// model-less runner, and whose own header promises "no model") died with
/// `libneedle.so not found` on `needle_demo` / `needle_ticket` /
/// `support_ticket_router_hybrid`. Force the assets away with env + a private
/// HOME and prove the states are skipped with exit 0.
pub fn test_bdd_smoke_offline_asset_skip(h: &mut Harness) {
    use laya_workflow::dsl_smoke::offline_asset_bound;
    let root = bdd::repo_root();
    let load = |p: &str| -> Value {
        serde_json::from_str(&std::fs::read_to_string(root.join(p)).unwrap()).unwrap()
    };

    // Machine-independent half: a spec that never calls a needle capability
    // carries no asset bound, so the skip can never widen to cover it.
    h.check(
        "p196: needle-free spec has no asset bound",
        offline_asset_bound(&load("dsl/versioned/refund_policy.v1.json")).is_none(),
    );

    let mut bin = std::env::current_exe().expect("test binary path");
    bin.pop();
    if bin.ends_with("deps") {
        bin.pop();
    }
    bin.push("laya-workflow");

    let home = std::env::temp_dir().join(format!("laya-p196-nomodel-{}", std::process::id()));
    std::fs::create_dir_all(&home).expect("tmp home");
    let absent = home.join("absent-model");
    let run = |filter: &str| {
        std::process::Command::new(&bin)
            .args(["dsl", "smoke", "--filter", filter])
            .current_dir(&root)
            .env("HOME", &home)
            .env("NEEDLE3_LIB_PATH", &absent)
            .env("NEEDLE3_CACT", &absent)
            .output()
            .expect("run dsl smoke")
    };

    // End to end: with the model forced absent the spec still passes the gate
    // (its state is skipped validate-only), instead of failing with a run
    // error the host cannot act on.
    let out = run("needle_demo");
    let err = String::from_utf8_lossy(&out.stderr).to_string();
    let stdout = String::from_utf8_lossy(&out.stdout).to_string();
    h.check("p196: needle spec with no model exits 0", out.status.success());
    h.check(
        "p196: needle state skipped, not failed",
        err.contains("on-device needle engine") && err.contains("skipped, validate only"),
    );
    h.check("p196: no run failure reported", !err.contains("[FAIL]"));
    h.check("p196: needle spec still counts as OK", stdout.contains("1/1 specs OK"));

    // Control: a needle-free spec under the same forced-absent env is not
    // touched by the new skip.
    let out2 = run("refund_policy.v1");
    let err2 = String::from_utf8_lossy(&out2.stderr).to_string();
    h.check(
        "p196: needle-free spec unaffected by the asset skip",
        out2.status.success() && !err2.contains("on-device needle engine"),
    );
}

/// P197: a `shell` capability whose **fixed program** is not installed is
/// skipped validate-only instead of failing — the third member of the
/// "unreachable here ⇒ validate-only" family, after a network endpoint with no
/// listener (P33) and an absent on-device asset (P196).
///
/// It covers the one *silent* miss: the platform shell `/bin/sh -c` still starts
/// and exits 127, which the workflow's routing reads as a plain "no"; `exec` /
/// `db` misses instead surface as a `spawn … failed` error and are caught by the
/// run-error classifier (`missing_program`), which is what keeps a spec whose
/// registered states never reach the node running for real.
pub fn test_bdd_smoke_offline_binary_skip(h: &mut Harness) {
    use laya_workflow::dsl_smoke::{binary_bound, missing_program};
    let root = bdd::repo_root();
    let load = |p: &str| -> Value {
        serde_json::from_str(&std::fs::read_to_string(root.join(p)).unwrap()).unwrap()
    };

    // ── Machine-independent half ──
    // The skip can never widen past `shell`: a db-only spec and an exec-only
    // spec both carry no bound, because their misses stay a run-time spawn
    // error (which is a per-state skip that never touches a state that does not
    // call them — `cua_desktop`'s lone `route` state, for one).
    h.check(
        "p197: db-only spec has no binary bound",
        binary_bound(&load("dsl/laya_mem/persist_memory.json")).is_none(),
    );
    h.check(
        "p197: exec-only spec has no binary bound",
        binary_bound(&load("dsl/browser/browser_orchestrate_probe.json")).is_none(),
    );
    h.check(
        "p197: shell-free spec has no binary bound",
        binary_bound(&load("dsl/versioned/refund_policy.v1.json")).is_none(),
    );

    // A called shell capability naming a program that cannot exist is bound…
    let fake = json!({
        "capabilities": { "run": { "kind": "shell", "command": "p197-nosuch-program-xyz --flag" } },
        "nodes": [ { "name": "start", "action": { "kind": "call", "capability": "run" } } ]
    });
    let why = binary_bound(&fake).expect("a missing shell program must bound the spec");
    h.check(
        "p197: a missing shell program names itself in the reason",
        why.contains("p197-nosuch-program-xyz"),
    );
    // …but a command starting with a placeholder is not decided statically.
    let placeholder = json!({
        "capabilities": { "run": { "kind": "shell", "command": "${state.tool} --flag" } },
        "nodes": [ { "name": "start", "action": { "kind": "call", "capability": "run" } } ]
    });
    h.check(
        "p197: a placeholder-led shell command carries no binary bound",
        binary_bound(&placeholder).is_none(),
    );
    // A declared-but-uncalled shell capability must not knock a spec out.
    let uncalled = json!({
        "capabilities": { "run": { "kind": "shell", "command": "p197-nosuch-program-xyz" } },
        "nodes": [ { "name": "start", "action": { "kind": "stop" } } ]
    });
    h.check(
        "p197: an uncalled shell capability does not bound the spec",
        binary_bound(&uncalled).is_none(),
    );

    // The run-error classifier: a bare not-found names the program; an explicit
    // path (a real defect) and a non-ENOENT failure do not.
    h.check(
        "p197: a bare spawn-ENOENT yields the program",
        missing_program("spawn \"sqlite3\" failed: No such file or directory (os error 2)")
            .as_deref()
            == Some("sqlite3"),
    );
    h.check(
        "p197: an explicit-path spawn-ENOENT is a defect, not an environment gap",
        missing_program("spawn \"/opt/x/tool\" failed: No such file or directory (os error 2)")
            .is_none(),
    );
    h.check(
        "p197: a non-ENOENT spawn failure is not an environment gap",
        missing_program("spawn \"sh\" failed: Permission denied (os error 13)").is_none(),
    );

    // ── End to end under a stripped $PATH ──
    let mut bin = std::env::current_exe().expect("test binary path");
    bin.pop();
    if bin.ends_with("deps") {
        bin.pop();
    }
    bin.push("laya-workflow");

    let home = std::env::temp_dir().join(format!("laya-p197-nopath-{}", std::process::id()));
    std::fs::create_dir_all(&home).expect("tmp home");
    let run = |filter: &str| {
        std::process::Command::new(&bin)
            .args(["dsl", "smoke", "--filter", filter])
            .current_dir(&root)
            .env("HOME", &home)
            .env("PATH", "/nonexistent")
            .output()
            .expect("run dsl smoke")
    };

    // A shell miss is silent (exit 127) → the pre-flight skip catches it.
    let out = run("feishu_unread_digest");
    let err = String::from_utf8_lossy(&out.stderr).to_string();
    let stdout = String::from_utf8_lossy(&out.stdout).to_string();
    h.check("p197: shell spec with no program exits 0", out.status.success());
    h.check(
        "p197: shell state skipped, naming the program",
        err.contains("needs the external program `sh` on $PATH") && err.contains("skipped, validate only"),
    );
    h.check("p197: no run failure reported for the shell spec", !err.contains("[FAIL]"));
    h.check("p197: shell spec still counts as OK", stdout.contains("1/1 specs OK"));

    // A db miss is a spawn error → the run-error classification catches it.
    let out2 = run("persist_memory");
    let err2 = String::from_utf8_lossy(&out2.stderr).to_string();
    let stdout2 = String::from_utf8_lossy(&out2.stdout).to_string();
    h.check("p197: db spec with no sqlite3 exits 0", out2.status.success());
    h.check(
        "p197: db state skipped, naming sqlite3",
        err2.contains("needs the external program `sqlite3` on $PATH"),
    );
    h.check("p197: no run failure reported for the db spec", !err2.contains("[FAIL]"));
    h.check("p197: db spec still counts as OK", stdout2.contains("1/1 specs OK"));

    // Control: an external-program-free spec, same stripped $PATH, untouched.
    let out3 = run("refund_policy.v1");
    let err3 = String::from_utf8_lossy(&out3.stderr).to_string();
    h.check(
        "p197: external-program-free spec unaffected",
        out3.status.success()
            && !err3.contains("needs the external program")
            && !err3.contains("[FAIL]"),
    );
}

/// P42: structural branch-coverage gate — generalises P40's manual "状态覆盖广度"
/// audit into a structural invariant. For every spec that carries registered
/// smoke states, the union of nodes visited across those states (**real engine
/// run**) must cover every node **reachable from `start` in the graph** —
/// except a small, justified allow-list of nodes the offline harness cannot
/// reach. A newly added reachable node with no state that walks to it becomes a
/// hard failure, exactly like P38 made trace pinning structural.
///
/// Also proves P42's vocabulary fix: `check_expect` now understands a boolean
/// decision output (a projected `truncated`/`blocked`/`seen` field). The old
/// `_ => false` arm rejected *every* boolean for *every* value, so no author
/// could pin one — the gate silently had no vocabulary for a whole JSON type.
pub fn test_bdd_smoke_branch_coverage(h: &mut Harness) {
    use laya_workflow::backend::HeuristicBackend;
    use laya_workflow::dsl_smoke::{check_expect, ensure_env_dirs, network_bound_external};
    use std::collections::{BTreeMap, BTreeSet};
    let root = bdd::repo_root();
    if let Ok(env) = ensure_env_dirs() {
        for (k, v) in &env {
            std::env::set_var(k, v);
        }
    }
    let _ = laya_workflow::capability::secret::init(None);

    // P42 vocabulary fix, proven both ways on a hand-built outcome.
    let mk = |state: Value| laya_workflow::workflow::WorkflowOutcome {
        state,
        trace: Default::default(),
        history: Vec::new(),
        node_visits: Default::default(),
        iterations: 1,
    };
    h.check(
        "p42: check_expect pins a true boolean",
        check_expect(
            &mk(serde_json::json!({ "truncated": true })),
            &serde_json::json!({ "result.truncated": true }),
        )
        .is_empty(),
    );
    h.check(
        "p42: check_expect rejects a mismatched boolean",
        !check_expect(
            &mk(serde_json::json!({ "truncated": true })),
            &serde_json::json!({ "result.truncated": false }),
        )
        .is_empty(),
    );

    let states: Value = serde_json::from_str(
        &std::fs::read_to_string(root.join("bench").join("dsl_smoke_states.json")).unwrap(),
    )
    .unwrap();
    let states_obj = states.as_object().unwrap();

    // Nodes that are graph-reachable but **cannot** be visited by the offline
    // smoke harness. Every entry is asserted below to still be a real,
    // reachable node, so this allow-list cannot rot into a silent hole.
    //
    // It is down to one entry: the two that used to sit here were not really
    // environment limits. `intake_pipeline_nested:intake` is an *inline
    // sub-workflow container* — the engine expands it in place, so its steps
    // surface as `intake::classify` / `intake::severity_probe` and the node
    // name `intake` is never a trace step by construction (see `containers`
    // below, which is now handled structurally instead of allow-listed).
    // `support_ticket_router_hybrid:needle_fallback` was only unreachable
    // because `ensure_env_dirs` did not seed `${env.NEEDLE3_CACT}`; with the
    // placeholder in place the offline backend drives the hybrid fallback and
    // the `sales` state walks it, so it is covered for real.
    let allowed: &[(&str, &str)] = &[
        // calls the real `cua-driver` exec; the offline heuristic never routes here
        ("cua_desktop", "exec"),
    ];
    let mut allow_seen: BTreeMap<String, bool> =
        allowed.iter().map(|(sp, node)| (format!("{sp}:{node}"), false)).collect();

    // Reachable nodes from `start`, following condition/default string targets.
    // Returns (reachable node names, container node names). A *container* is a
    // node that carries a `workflow` key: the engine expands the referenced
    // workflow inline, so the container's own name is never a trace step (its
    // children surface as `<container>::<child>`). Containers are therefore
    // expanded-but-never-visited by construction and are excluded from the
    // must-be-visited set; their continuation still is, because we keep
    // following their `edge`.
    let reachable = |raw: &Value| -> (BTreeSet<String>, BTreeSet<String>) {
        let nodes = raw
            .get("nodes")
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default();
        let names: BTreeSet<String> = nodes
            .iter()
            .filter_map(|n| n.get("name").and_then(Value::as_str).map(str::to_string))
            .collect();
        let containers: BTreeSet<String> = nodes
            .iter()
            .filter(|n| n.get("workflow").is_some())
            .filter_map(|n| n.get("name").and_then(Value::as_str).map(str::to_string))
            .collect();
        let mut seen = BTreeSet::new();
        let mut stack: Vec<String> = raw
            .get("start")
            .and_then(Value::as_str)
            .map(|s| vec![s.to_string()])
            .unwrap_or_default();
        while let Some(n) = stack.pop() {
            if !names.contains(&n) || !seen.insert(n.clone()) {
                continue;
            }
            let Some(node) = nodes
                .iter()
                .find(|x| x.get("name").and_then(Value::as_str) == Some(n.as_str()))
            else {
                continue;
            };
            let edge = node.get("edge").cloned().unwrap_or(Value::Null);
            let mut targets: Vec<String> = Vec::new();
            if let Some(cond) = edge.get("condition").and_then(Value::as_object) {
                for v in cond.values() {
                    if let Some(s) = v.as_str() {
                        targets.push(s.to_string());
                    }
                }
            }
            if let Some(d) = edge.get("default").and_then(Value::as_str) {
                targets.push(d.to_string());
            }
            for t in targets {
                if names.contains(&t) && !seen.contains(&t) {
                    stack.push(t);
                }
            }
        }
        (seen, containers)
    };

    let dsl_root = root.join("dsl");
    let mut specs = Vec::new();
    let mut walk = vec![dsl_root.clone()];
    while let Some(dir) = walk.pop() {
        for entry in std::fs::read_dir(&dir).unwrap() {
            let p = entry.unwrap().path();
            if p.is_dir() {
                walk.push(p);
            } else if p.extension().and_then(|e| e.to_str()) == Some("json")
                && !p
                    .file_name()
                    .map(|n| n.to_string_lossy().ends_with(".config.json"))
                    .unwrap_or(false)
            {
                specs.push(p);
            }
        }
    }
    specs.sort();

    let mut checked = 0usize;
    for spec in &specs {
        let stem = spec
            .file_stem()
            .and_then(|s| s.to_str())
            .unwrap_or("")
            .to_string();
        let Some(smap) = states_obj.get(&stem).and_then(Value::as_object) else {
            continue;
        };
        let raw: Value = std::fs::read_to_string(spec)
            .ok()
            .and_then(|t| serde_json::from_str(&t).ok())
            .unwrap_or(Value::Null);
        // Offline harness: never reach an external host (mirrors the P33 rule).
        if network_bound_external(&raw) {
            continue;
        }
        let wf = match laya_workflow::spec::load_file(&spec.to_string_lossy()) {
            Ok(w) => w,
            Err(e) => {
                h.check(&format!("bdd: branch coverage {stem} spec loads ({e})"), false);
                continue;
            }
        };
        let mut visited: BTreeSet<String> = BTreeSet::new();
        let mut ok = 0usize;
        let mut errored = 0usize;
        for (_label, st) in smap {
            match wf.run(&HeuristicBackend, st) {
                Ok(out) => {
                    ok += 1;
                    if let Some(steps) =
                        out.to_json().pointer("/trace/steps").and_then(Value::as_array)
                    {
                        for s in steps {
                            if let Some(n) = s.get("node").and_then(Value::as_str) {
                                visited.insert(n.to_string());
                            }
                        }
                    }
                }
                Err(_) => errored += 1,
            }
        }
        // Not fully verifiable offline (local mock down, or a state the offline
        // heuristic cannot drive): the mocks-up integration run covers it.
        if ok == 0 || errored > 0 {
            continue;
        }
        checked += 1;
        let (reach, containers) = reachable(&raw);
        for n in reach {
            if visited.contains(&n) {
                continue;
            }
            // Inline sub-workflow container: expanded in place, never a trace
            // step of its own. Its children and continuation are checked above
            // (children via the `::` steps the container's states record;
            // continuation because we still follow its edge).
            if containers.contains(&n) {
                continue;
            }
            let key = format!("{stem}:{n}");
            if let Some(hit) = allow_seen.get_mut(&key) {
                *hit = true;
                continue;
            }
            h.check(
                &format!(
                    "bdd: branch coverage — {stem} can reach node {n} but no registered state visits it"
                ),
                false,
            );
        }
    }
    // Every allow-listed node must still be real and reachable, else the entry
    // is stale and hides a hole.
    for (key, hit) in &allow_seen {
        h.check(
            &format!("bdd: allow-listed {key} is still a reachable node"),
            *hit,
        );
    }
    h.check(
        &format!("bdd: branch coverage checked {checked} specs (floor 40)"),
        checked >= 40,
    );
}

/// P43: the coverage ledger gate. Every dsl spec is either offline-registered
/// (behaviour-verified by `dsl smoke` on its sample states) or declared
/// live-only in `bench/dsl_coverage_ledger.json` with the gate that covers it
/// and why the offline smoke cannot. Nothing is silently unverified, and no
/// declaration may rot: a spec that becomes offline-registered, or is deleted,
/// leaves a stale entry that fails here (see the two negative cases below).
pub fn test_bdd_dsl_coverage_ledger(h: &mut Harness) {
    use laya_workflow::dsl_smoke::{coverage_ledger_audit, discover_specs};
    use std::collections::BTreeMap;
    let root = bdd::repo_root();
    let dsl_root = root.join("dsl");
    let specs = discover_specs(&dsl_root).expect("discover dsl specs");
    let raw: Value = serde_json::from_str(
        &std::fs::read_to_string(root.join("bench").join("dsl_smoke_states.json")).unwrap(),
    )
    .unwrap();
    let states: BTreeMap<String, Value> = raw
        .as_object()
        .unwrap()
        .clone()
        .into_iter()
        .collect();

    let (offline, live, problems) =
        coverage_ledger_audit(&specs, &dsl_root, &states).expect("coverage ledger audit");
    h.check(
        "bdd: coverage ledger is complete and rot-free",
        problems.is_empty(),
    );
    for p in &problems {
        h.check(&format!("bdd: coverage ledger problem — {p}"), false);
    }
    h.check(
        &format!(
            "bdd: coverage ledger accounts for every dsl spec ({offline} offline-verified + {live} live-declared = {} of {})",
            offline + live,
            specs.len()
        ),
        offline + live == specs.len() && offline > 0 && live > 0,
    );

    // Negative 1: a spec that is neither registered nor declared must fail —
    // this is the "silently unverified workflow" the ledger exists to stop.
    let mut undeclared = specs.clone();
    undeclared.push(dsl_root.join("__absent_probe__.json"));
    let (_, _, probs) = coverage_ledger_audit(&undeclared, &dsl_root, &states).expect("audit");
    h.check(
        "bdd: an undeclared, unregistered spec is flagged (no silent hole)",
        probs.iter().any(|p| p.contains("__absent_probe__")),
    );

    // Negative 2: a declaration for a spec that no longer exists is stale.
    let mut deleted: Vec<std::path::PathBuf> = specs
        .iter()
        .filter(|p| {
            !p.file_name()
                .and_then(|n| n.to_str())
                .map(|n| n == "arxiv.json")
                .unwrap_or(false)
        })
        .cloned()
        .collect();
    deleted.sort();
    let (_, _, probs) = coverage_ledger_audit(&deleted, &dsl_root, &states).expect("audit");
    h.check(
        "bdd: a declaration whose spec vanished is flagged stale",
        probs.iter().any(|p| p.contains("browser/arxiv") && p.contains("stale")),
    );
}

/// `bdd_transpile` MCP tool: compile a `.feature` into its dsl spec — the

/// "emit the dsl spec" step of the authoring loop, in-process. Asserts a known
/// `@decision` feature transpiles to a valid decision-graph spec with
/// dsl_version stripped (matching the dsl convention), and that write=true
/// persists under the requested root with an exists-guard.
pub fn test_bdd_mcp_transpile(h: &mut Harness) {
    let t = bdd_tool("bdd_transpile");
    let src = r#"
@decision
Feature: transpile_demo
  A minimal decision graph.

  Scenario: classify
    Given a choice question "category" asks "Which team should handle this ticket?"
      | answer    | meaning                     |
      | billing   | invoices, payments, refunds |
      | technical | bugs, outages, system errors|
    When the answer "technical" goes to "STOP"
    And any other answer goes to "STOP"
    Then keep the answer to "category"
"#;
    let r = t.call(&json!({"feature_text": src, "cat": "routing"})).unwrap();
    h.eq("bdd_transpile: valid", r["valid"].as_bool().unwrap(), true);
    h.eq("bdd_transpile: decision", r["decision"].as_bool().unwrap(), true);
    h.eq("bdd_transpile: name", r["name"].clone(), json!("transpile_demo"));
    h.eq("bdd_transpile: spec_count", r["spec_count"].as_u64().unwrap(), 1);
    let spec = &r["spec"];
    h.eq("bdd_transpile: start", spec["start"].clone(), json!("classify"));
    h.eq("bdd_transpile: node count", spec["nodes"].as_array().unwrap().len(), 1);
    h.check("bdd_transpile: dsl_version stripped", spec.get("dsl_version").is_none());
    // The emitted spec must validate and run through the engine's loader.
    let _ = crate::spec::from_spec_with_dir(spec, None, None).expect("emitted spec validates");
    // write=true persists under an explicit root (temp dir), not the repo.
    let root = std::env::temp_dir().join(format!("laya-bdd-transpile-mcp-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    let r2 = t
        .call(&json!({"feature_text": src, "cat": "routing", "write": true, "root": root.to_string_lossy().into_owned()}))
        .unwrap();
    h.eq("bdd_transpile: written path set", r2["written"].is_string(), true);
    let dest = r2["written"].as_str().unwrap().to_string();
    h.check("bdd_transpile: file on disk", std::path::Path::new(&dest).is_file());
    h.check("bdd_transpile: file under dsl", dest.contains("/dsl/routing/"));
    // Second write refuses (exists=true) without overwriting.
    let r3 = t
        .call(&json!({"feature_text": src, "cat": "routing", "write": true, "root": root.to_string_lossy().into_owned()}))
        .unwrap();
    h.eq("bdd_transpile: exists flag", r3["exists"].as_bool().unwrap(), true);
    h.eq("bdd_transpile: not overwritten", r3["written"].is_null(), true);
    // Bad input: neither feature_text nor feature_path errors cleanly.
    h.check("bdd_transpile: no input errors", t.call(&json!({})).is_err());
    let _ = std::fs::remove_dir_all(&root);
}

/// The authoring-score corpus must keep proving the distinct decision-graph
/// shapes, not just the first three. A task is "decision" when its reference
/// feature carries `@decision`; the floor is deliberately above the original
/// three so deleting a shape (sub-workflow, loop+merge, noul+field heuristic)
/// turns this red instead of silently shrinking the proof surface.
pub fn test_bdd_score_corpus_floor(h: &mut Harness) {
    let root = bdd::repo_root();
    let tasks_dir = root.join("bench").join("bdd").join("tasks");
    let mut files: Vec<_> = std::fs::read_dir(&tasks_dir)
        .unwrap()
        .flatten()
        .map(|e| e.path())
        .filter(|p| p.extension().map(|e| e == "json").unwrap_or(false))
        .collect();
    files.sort();
    h.check("bdd: score corpus tasks >= 14", files.len() >= 14);
    let mut decision = 0usize;
    let mut browser = 0usize;
    for f in &files {
        let meta: serde_json::Value = serde_json::from_str(&std::fs::read_to_string(f).unwrap()).unwrap();
        let id = meta.get("id").and_then(serde_json::Value::as_str).unwrap_or("").to_string();
        let feat_rel = meta.get("feature").and_then(serde_json::Value::as_str).unwrap_or("").to_string();
        h.check(&format!("bdd: task {id} has intent"), meta.get("intent").and_then(serde_json::Value::as_str).map(|s| !s.is_empty()).unwrap_or(false));
        let known = meta.get("known_steps").and_then(serde_json::Value::as_array).map(|a| a.len()).unwrap_or(0);
        let unknown = meta.get("unknown_steps").and_then(serde_json::Value::as_array).map(|a| a.len()).unwrap_or(0);
        h.check(&format!("bdd: task {id} has known steps"), known >= 6);
        h.check(&format!("bdd: task {id} has unknown steps"), unknown >= 4);
        let feat = root.join(&feat_rel);
        h.check(&format!("bdd: task {id} feature exists"), feat.is_file());
        let text = std::fs::read_to_string(&feat).unwrap();
        if text.lines().any(|l| l.starts_with("@decision")) {
            decision += 1;
        } else {
            browser += 1;
        }
    }
    h.check("bdd: decision score tasks >= 11", decision >= 11);
    h.check("bdd: browser score tasks >= 3", browser >= 3);
}

/// `bdd score --run` must run the @decision tasks offline through the heuristic
/// engine on their registered states (the browser half needs Chrome and is
/// exercised separately by check.sh / live runs). This pins the fix where a
/// decision task used to be reported e2e=FAIL because `bdd run`'s browser
/// compiler rejects @decision steps by design.
pub fn test_bdd_score_e2e_decision(h: &mut Harness) {
    use laya_workflow::bdd::ScoreOptions;
    let ids = [
        "decision_support_ticket",
        "decision_content_safety",
        "decision_agent_probe",
        "decision_pipeline_nested",
        "decision_loop_refine",
        "decision_noul_field_heuristic",
        "decision_dialogue_state_machine",
        "decision_memory_admission",
        "decision_gate_action",
        "decision_risk_control",
        "decision_dynamic_candidates",
    ];
    let report = std::env::temp_dir().join(format!("laya-bdd-score-e2e-{}.json", std::process::id()));
    let _ = std::fs::remove_file(&report);
    let rc = bdd::score(&ScoreOptions {
        check: true,
        run: true,
        json: Some(report.to_string_lossy().into_owned()),
        tasks: ids.iter().map(|s| s.to_string()).collect(),
    });
    h.eq("bdd: score --run decision gate passes", rc.unwrap_or(-1), 0);
    let d: serde_json::Value = serde_json::from_str(&std::fs::read_to_string(&report).unwrap()).unwrap();
    let tasks = d["tasks"].as_array().unwrap();
    h.eq("bdd: score --run covers all decision tasks", tasks.len(), 11);
    for t in tasks {
        let id = t["id"].as_str().unwrap_or("?");
        h.check(
            &format!("bdd: score --run {id} e2e PASS"),
            t.get("e2e").and_then(|v| v.as_bool()).unwrap_or(false),
        );
    }
    let _ = std::fs::remove_file(&report);
}

/// Regression (P25): the `when the sub workflow completes continue at "X"` step
/// emits the reference node's completion-handoff edge, and compiling a nested
/// pipeline feature yields a reachable downstream node (no dead code).

/// P30: `bdd score --run`'s decision e2e asserts the authored feature's
/// decision outputs against the pinned expect (same `check_expect` semantics
/// as `dsl smoke`) — an authored feature that routes to the wrong node but does
/// not error must FAIL e2e, not pass. Pins the shared helper's PASS and FAIL
/// paths so a refactor cannot silently drop the assertion.
pub fn test_bdd_decision_e2e_output_assert(h: &mut Harness) {
    use laya_workflow::backend::HeuristicBackend;
    use laya_workflow::dsl_smoke::check_expect;
    let src = r#"
@decision
Feature: route_outputs
  Background:
    Given the workflow runs at most 10 iterations
  Scenario: classify
    Given a choice question "route" asks "Which path?"
      | answer | meaning |
      | A      | first   |
      | B      | second  |
    When the answer "A" goes to "STOP"
    And any other answer goes to "STOP"
    Then keep the answer to "route"
"#;
    let feature = bdd::parse(src, "<test>").expect("parse");
    let spec = bdd::compile_decision(&feature, &json!({})).expect("compile");
    let wf = laya_workflow::spec::from_spec_with_dir(&spec, None, None).expect("load");
    let out = wf.run(&HeuristicBackend, &json!({"hint": "second"})).expect("run");
    // The heuristic picks the first choice criterion ("A") → stops with route=A.
    h.eq(
        "bdd: decision e2e passes on a correct decision",
        check_expect(&out, &json!({"result.route": "A"})).len(),
        0,
    );
    let msgs = check_expect(&out, &json!({"result.route": "B"}));
    h.eq(
        "bdd: decision e2e reports a wrong decision",
        msgs.len(),
        1,
    );
    h.check(
        "bdd: decision e2e failure names path and got value",
        msgs[0].contains("result.route") && msgs[0].contains('A'),
    );
}

pub fn test_bdd_sub_workflow_continue(h: &mut Harness) {
    let src = r#"
@decision
Feature: nested_continue
  Background:
    Given the workflow runs at most 30 iterations
  Scenario: gate
    Given a choice question "g" asks "go?"
      | answer | meaning |
      | A      | no      |
      | B      | yes     |
    When the answer "A" goes to "sub"
    And any other answer goes to "STOP"
  Scenario: sub
    Given the sub workflow "support_ticket_router"
    And when the sub workflow completes continue at "audit"
  Scenario: audit
    Given a choice question "n" asks "ok?"
      | answer | meaning |
      | A      | no      |
      | B      | yes     |
    When any other answer goes to "STOP"
"#;
    let feature = bdd::parse(src, "<test>").expect("parse");
    let spec = bdd::compile_decision(&feature, &json!({})).expect("compile");
    let nodes = spec["nodes"].as_array().unwrap();
    let sub = nodes.iter().find(|n| n["name"] == "sub").unwrap();
    h.eq(
        "bdd: sub continue edge default",
        sub["edge"]["default"].clone(),
        json!("audit"),
    );
    h.eq("bdd: sub continue edge condition", sub["edge"]["condition"].clone(), json!({}));
}

/// P28: unmatched decision steps get a "did you mean" suggestion from the
/// decision vocabulary (same-kind typo + wrong-keyword-kind), not a browser op.
pub fn test_bdd_decision_suggest(h: &mut Harness) {
    // same-kind typo: missing "the" in the edge-routing step
    let s1 = bdd::decision_suggest("when", r#"answer "A" goes to "STOP""#);
    h.check(
        "bdd: suggest edge template for missing 'the'",
        s1.as_deref().unwrap_or("").contains(r#"the answer "<a>" goes to "<node>""#),
    );
    // wrong keyword kind: primary_q is a Then step, author wrote it as Given/And
    let s2 = bdd::decision_suggest("given", r#"the primary question is "nope""#);
    h.check(
        "bdd: suggest primary_q template across kinds",
        s2.as_deref().unwrap_or("").contains(r#"the primary question is "<q>""#),
    );
    // a fully-valid step (already handled by match_step) still suggests the
    // *same* template — decision_suggest is only consulted after a match miss,
    // so this is harmless; what matters is it never points at a wrong template.
    let s3 = bdd::decision_suggest("when", r#"the answer "A" goes to "STOP""#);
    h.check(
        "bdd: valid edge step suggests the edge template",
        s3.as_deref().unwrap_or("").contains(r#"the answer "<a>" goes to "<node>""#),
    );
    // unrelated text produces no suggestion (no false positive)
    h.check(
        "bdd: unrelated text has no suggestion",
        bdd::decision_suggest("given", "take out the trash").is_none(),
    );
}

/// P131: the decision-side vocabulary-drift gate. `decision_templates` is the
/// single source behind `bdd vocabulary` and the "did you mean" hints, and it
/// sits next to the compiler's own tables, so the two can drift silently. This
/// proves both directions on the live tables, purely (so a truncated list
/// fires), and that the CLI agrees.
pub fn test_bdd_decision_vocab_check(h: &mut Harness) {
    // Positive: the live index covers every compiler shape and every template it
    // advertises compiles.
    h.check(
        "bdd decision-vocab-check: the live vocabulary has no drift",
        bdd::decision_vocabulary_gaps(bdd::decision_templates()).is_empty(),
    );
    // The two halves are non-trivial: there are shapes to cover, across 3 kinds.
    let shapes = bdd::decision_shape_tags();
    let total_shapes: usize = shapes.values().map(|s| s.len()).sum();
    h.check(
        "bdd decision-vocab-check: the grammar has >= 40 shapes across three kinds",
        total_shapes >= 40 && shapes.len() == 3,
    );
    // Negative A — a compiler shape no advertised template covers is named
    // (truncate the index to a single `when` template: `edge_default` is then
    // uncovered).
    let gaps = bdd::decision_vocabulary_gaps(&[("when", r#"the answer "<a>" goes to "<node>""#)]);
    h.check(
        "bdd decision-vocab-check: an uncovered compiler shape is named",
        gaps.iter().any(|g| g.contains("edge_default")),
    );
    // Negative B — a template the compiler rejects is named (a typo the index
    // would hand to an author).
    let gaps2 = bdd::decision_vocabulary_gaps(&[(
        "then",
        r#"label "<label>" whenn the answer to "<q>" is "<a>""#,
    )]);
    h.check(
        "bdd decision-vocab-check: a non-compiling template is named",
        gaps2.iter().any(|g| g.contains("does not compile")),
    );
    // The per-template compiler probe discriminates a good template from a
    // wrong-kind one.
    h.check(
        "bdd decision-vocab-check: a good template compiles",
        bdd::decision_template_compiles("then", r#"label "<label>" otherwise"#),
    );
    h.check(
        "bdd decision-vocab-check: a wrong-kind template does not compile",
        !bdd::decision_template_compiles("when", r#"a choice question "x" asks "y""#),
    );
    // CLI: the gate is wired into `bdd` and green.
    let root = bdd::repo_root();
    let mut bin = std::env::current_exe().expect("test binary path");
    bin.pop();
    if bin.ends_with("deps") {
        bin.pop();
    }
    bin.push("laya-workflow");
    let out = std::process::Command::new(&bin)
        .args(["bdd", "decision-vocab-check"])
        .current_dir(&root)
        .output()
        .expect("run bdd decision-vocab-check");
    let text = String::from_utf8_lossy(&out.stdout);
    h.check(
        "bdd decision-vocab-check: CLI exits 0 and prints the coverage line",
        out.status.success() && text.contains("cover all"),
    );
}

/// P44: the bdd/json separation invariant, as a gate. `bdd/` (all `.feature`
/// sources) and `dsl/` (all workflow `.json` specs) are two fully separate,
/// hierarchical trees — a `.json` in `bdd/`, a `.feature` in `dsl/`, a source
/// loose at either tree root, a `.feature` anywhere else, or a workflow spec
/// `.json` anywhere outside `dsl/` all break it. The live repo must pass; a
/// synthetic tree must pass while each of the five violations is flagged, so
/// the check cannot go green by accident on an empty or mis-shaped tree.
pub fn test_bdd_json_isolation(h: &mut Harness) {
    use laya_workflow::dsl_smoke::isolation_audit;

    // Positive: the checked-in repo satisfies the invariant.
    let repo = bdd::repo_root();
    let problems = isolation_audit(&repo);
    h.check(
        &format!(
            "bdd: repo bdd/dsl separation holds ({} problem(s))",
            problems.len()
        ),
        problems.is_empty(),
    );
    for p in &problems {
        h.check(&format!("bdd: isolation problem — {p}"), false);
    }

    // A throwaway tree drives the positive + negative cases. Start from a
    // well-formed layout plus a support file that must NOT trip the check.
    let base = std::env::temp_dir().join(format!("laya-iso-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&base);
    let mk = |rel: &str| {
        let p = base.join(rel);
        std::fs::create_dir_all(p.parent().unwrap()).unwrap();
        std::fs::write(&p, "x").unwrap();
    };
    mk("bdd/features/a.feature");
    mk("bdd/workflows/routing/a.feature");
    mk("bdd/fixtures/a.html");
    mk("dsl/routing/a.json");
    h.check(
        "bdd: a well-formed two-tree layout passes isolation",
        isolation_audit(&base).is_empty(),
    );

    // Negative 1: a json dropped into the BDD tree.
    mk("bdd/routing/leak.json");
    let p = isolation_audit(&base);
    h.check(
        "bdd: json in bdd/ is flagged",
        p.iter().any(|m| {
            m.contains("bdd/routing/leak.json") && m.contains("json in the BDD tree")
        }),
    );
    std::fs::remove_file(base.join("bdd/routing/leak.json")).unwrap();

    // Negative 2: a .feature dropped into the json tree.
    mk("dsl/routing/leak.feature");
    let p = isolation_audit(&base);
    h.check(
        "bdd: .feature in dsl/ is flagged",
        p.iter().any(|m| {
            m.contains("dsl/routing/leak.feature") && m.contains("json tree")
        }),
    );
    std::fs::remove_file(base.join("dsl/routing/leak.feature")).unwrap();

    // Negative 3: a source loose at a tree root (not filed under a category).
    mk("dsl/loose.json");
    let p = isolation_audit(&base);
    h.check(
        "bdd: a loose dsl/loose.json is flagged (hierarchy enforced)",
        p.iter().any(|m| m.contains("dsl/loose.json") && m.contains("loose")),
    );
    std::fs::remove_file(base.join("dsl/loose.json")).unwrap();

    // Negative 4: a .feature outside the one bdd/ tree.
    mk("elsewhere/stray.feature");
    let p = isolation_audit(&base);
    h.check(
        "bdd: a .feature outside bdd/ is flagged",
        p.iter().any(|m| m.contains("outside bdd/")),
    );

    // Negative 5: a workflow spec json outside the one dsl/ tree (the mirror of
    // Negative 4). A non-spec json at the same spot must NOT be flagged, or the
    // rule would sweep in plugin manifests / eval data.
    let spec = base.join("elsewhere/stray_spec.json");
    std::fs::create_dir_all(spec.parent().unwrap()).unwrap();
    std::fs::write(&spec, "{\"start\":0,\"nodes\":[]}").unwrap();
    std::fs::write(base.join("elsewhere/manifest.json"), "{\"name\":\"m\",\"version\":\"1\"}").unwrap();
    let p = isolation_audit(&base);
    h.check(
        "bdd: a workflow spec json outside dsl/ is flagged",
        p.iter()
            .any(|m| m.contains("elsewhere/stray_spec.json") && m.contains("outside dsl/")),
    );
    h.check(
        "bdd: a non-spec json outside dsl/ is left alone",
        !p.iter().any(|m| m.contains("elsewhere/manifest.json")),
    );

    let _ = std::fs::remove_dir_all(&base);
}

/// The doc gate, driven as a pure function so the negatives are checked on every
/// run instead of proven once by hand. `bdd doc-check` compares every labelled
/// number in `bdd/README.md` **and** `bdd/AGENT.md` against the gate that owns
/// it, and requires each quoted gate line to be exactly what the gate prints.
/// It also requires every concrete `bdd/…` / `bench/…` path the docs quote to
/// exist on disk, so a moved corpus cannot leave a phantom path behind.
/// AGENT.md is the guide an agent reads before it edits anything, and it had
/// already drifted a document-length behind (expect 124/978 vs 125/980, states
/// 127/987 vs 128/989, ledger 162 vs 163) with nobody noticing - that is what
/// this section now catches.
pub fn test_bdd_doc_check(h: &mut Harness) {
    use laya_workflow::bdd::{doc_check_impl, repo_root};

    let root = repo_root();
    let readme = std::fs::read_to_string(root.join("bdd/README.md")).unwrap();
    let agent = std::fs::read_to_string(root.join("bdd/AGENT.md")).unwrap();

    // The AGENT.md half is green *in this process*: every one of its labels is
    // owned by a checker that runs here (the two sample tables, the ledger, the
    // round-trip line). The README's `bdd vocabulary:` / `bdd args probes:` labels
    // are owned by gates that shell out to the real binary, so those are only
    // checked by `check.sh` running `bdd doc-check` - not asserted here, where
    // the process is this test runner and those sub-gates cannot run.
    let (_checked, total, _paths, problems) = doc_check_impl(&readme, &agent).unwrap();
    let agent_problems: Vec<&String> = problems
        .iter()
        .filter(|p| p.contains("bdd/AGENT.md"))
        .collect();
    h.check(
        &format!(
            "bdd doc-check: the checked-in AGENT.md is green ({total} labels, {} AGENT problem(s))",
            agent_problems.len()
        ),
        agent_problems.is_empty(),
    );
    for p in &agent_problems {
        h.check(&format!("bdd doc-check: live problem — {p}"), false);
    }
    h.check(
        "bdd doc-check: it reads both documents (AGENT.md contributes labels)",
        total > 14,
    );

    let problems_of = |r: &str, a: &str| -> Vec<String> {
        doc_check_impl(r, a).map(|(_, _, _, p)| p).unwrap_or_default()
    };
    let any = |ps: &[String], needle: &str| ps.iter().any(|p| p.contains(needle));

    // Negative 1: AGENT.md's expect-table count drifts from the table it quotes.
    // Both the planted drift and the number the gate is expected to quote are
    // derived from what the doc says *today*, so growing the corpus cannot
    // silently neuter this negative — it was hard-coded at `125 / 980` and the
    // corpus has since grown past it (P126).
    let table_count_re =
        regex_lite::Regex::new(r"\*\*(\d+)\*\* 个 spec / \*\*(\d+)\*\* 个 state").unwrap();
    let caps = table_count_re
        .captures(&agent)
        .expect("AGENT.md quotes the expect-table counts");
    let (cur_specs, cur_states) = (caps[1].to_string(), caps[2].to_string());
    let stale_specs = (cur_specs.parse::<u64>().unwrap() - 1).to_string();
    let stale_states = (cur_states.parse::<u64>().unwrap() - 1).to_string();
    let drifted_agent = table_count_re
        .replace(
            &agent,
            format!("**{stale_specs}** 个 spec / **{stale_states}** 个 state"),
        )
        .into_owned();
    h.check(
        "bdd doc-check: the negative plants a real drift (anchor is live)",
        drifted_agent != agent,
    );
    let p = problems_of(&readme, &drifted_agent);
    h.check(
        "bdd doc-check: a stale AGENT.md table count is named with both numbers",
        any(
            &p,
            &format!(
                "bdd/AGENT.md says [{stale_specs}, {stale_states}] where the truth is [{cur_specs}, {cur_states}]"
            ),
        ),
    );

    // Negative 2: AGENT.md's ledger total drifts.
    let p = problems_of(&readme, &agent.replace("= 163 specs", "= 162 specs"));
    h.check(
        "bdd doc-check: a stale AGENT.md ledger total is caught",
        any(&p, "bdd/AGENT.md says [128, 35, 162] where the truth is [128, 35, 163]"),
    );

    // Negative 3: AGENT.md's gate lines drift.
    let p = problems_of(&readme, &agent.replace("`dsl smoke 163/163`", "`dsl smoke 162/162`"));
    h.check(
        "bdd doc-check: a stale AGENT.md gate claim (dsl smoke) is caught",
        any(&p, "bdd/AGENT.md says [162, 162] where the truth is [163, 163]"),
    );

    // Negative 4: the README's own count drifts - the file the gate was born for.
    let p = problems_of(
        &readme.replace("Current state: **25 scenarios", "Current state: **5 scenarios"),
        &agent,
    );
    h.check(
        "bdd doc-check: a stale README scenario count is still caught",
        any(&p, "bdd/README.md says"),
    );

    // Negative 5: a labelled sentence is deleted, so its number goes unchecked.
    let label_line = agent
        .lines()
        .find(|l| l.starts_with("**当前语料现状**"))
        .unwrap()
        .to_string();
    let p = problems_of(&readme, &agent.replace(&label_line, "current state: see the gates"));
    h.check(
        "bdd doc-check: a dropped AGENT.md label is reported, not silently unchecked",
        any(&p, "bdd/AGENT.md no longer says anything matching"),
    );

    // Negative 6 (P123): a quoted path that no longer exists is caught. The
    // corpus move that orphaned `bench/bdd/live/*.feature` is exactly this
    // failure, and nothing recomputed the report that quoted it. Inject a
    // phantom into the README and expect it named.
    let phantom = readme.replace("bdd/fixtures/app.html", "bdd/fixtures/ghost_page.html");
    let p = problems_of(&phantom, &agent);
    h.check(
        "bdd doc-check: a quoted path that no longer exists is named (P123)",
        any(&p, "quotes bdd/fixtures/ghost_page.html, which does not exist on disk"),
    );

    // Negative 7 (P123): the `dsl/` tree is a spec root too, so a doc that quotes
    // a moved `dsl/…/x.json` must be caught the same way. Inject a phantom dsl
    // path into the README (whose real text quotes six probe specs) and expect it
    // named.
    let dsl_phantom = readme.replace(
        "dsl/browser/browser_base_probe.json",
        "dsl/browser/ghost_base_probe.json",
    );
    let p = problems_of(&dsl_phantom, &agent);
    h.check(
        "bdd doc-check: a quoted dsl/ path that no longer exists is named (P125)",
        any(&p, "quotes dsl/browser/ghost_base_probe.json, which does not exist on disk"),
    );

    // Negative 8 (P125): the scan used to read only `bench/bdd/`, so the documents
    // a user actually reads first (`docs/*.md`) were unchecked - measured, that is
    // where both live stale paths hid. Drop a throwaway doc under `docs/` quoting
    // a phantom (one bdd/ path and one dsl/ path) and expect both named *as that
    // doc*; the file is removed before the assertion so a red test leaves no dirt.
    let probe = root.join("docs").join("__p125_scope_probe.md");
    let _ = std::fs::remove_file(&probe);
    std::fs::write(
        &probe,
        "probe: bdd/workflows/guards/__p125_ghost__.feature and dsl/guards/__p125_ghost__.json\n",
    )
    .unwrap();
    let p = problems_of(&readme, &agent);
    let _ = std::fs::remove_file(&probe);
    h.check(
        "bdd doc-check: a doc under docs/ is scanned for a bdd/ path (P125)",
        any(
            &p,
            "docs/__p125_scope_probe.md quotes bdd/workflows/guards/__p125_ghost__.feature",
        ),
    );
    h.check(
        "bdd doc-check: a doc under docs/ is scanned for a dsl/ path too (P125)",
        any(&p, "docs/__p125_scope_probe.md quotes dsl/guards/__p125_ghost__.json"),
    );

    // ...and the checked-in docs quote no missing path (the negatives above are
    // real detections, not a blanket failure). The distinct (doc, path) count is a
    // second, structural witness of the widened scope: the three docs the gate used
    // to read contribute ~45, the whole checked-in doc tree 189 (measured). A
    // regression that quietly narrows the walk back would drop below this floor.
    let (_c, _t, n_paths, all) = doc_check_impl(&readme, &agent).unwrap();
    h.check(
        "bdd doc-check: the checked-in docs quote no missing path",
        !any(&all, "does not exist on disk"),
    );
    h.check(
        &format!("bdd doc-check: every checked-in doc is scanned, not just the BDD toolchain's (P125) [{n_paths} paths]"),
        n_paths > 150,
    );
}

/// The scope census (P124): "convert all known workflows to BDD" is only a
/// checkable claim if the workflows that are *not* decision graphs are named,
/// with reasons, next to the ones that are. `bdd scope-check` reads
/// `bench/bdd/scope_census.json` and, with a checkout, makes it match the live
/// `.py` files. Driven here as a pure function so every negative runs.
pub fn test_bdd_scope_census(h: &mut Harness) {
    use laya_workflow::bdd::{repo_root, scope_check_impl};

    let root = repo_root();
    let census = std::fs::read_to_string(root.join("bench/bdd/scope_census.json")).unwrap();

    // Positive: the checked-in census is green on its own.
    let (counts, problems) = scope_check_impl(&root, &census, None).unwrap();
    h.check(
        &format!(
            "bdd scope-check: the checked-in census is green ({} universe(s), {} ported + {} \
             search-space)",
            counts.universes, counts.ported, counts.search_space
        ),
        problems.is_empty(),
    );
    h.eq("bdd scope-check: reads all three universes", counts.universes, 3);
    h.eq(
        "bdd scope-check: 9 self_evolving workflows named as search-spaces",
        counts.search_space,
        9,
    );
    h.eq(
        "bdd scope-check: 17 explicit ports (agent_permission + the 16 agent-toolbox workflows)",
        counts.ported,
        17,
    );
    h.eq(
        "bdd scope-check: 1 universe ported in full (ole_eval)",
        counts.ported_all,
        1,
    );
    h.eq(
        "bdd scope-check: 1 non-enumerable universe (agent_toolbox - a private upstream)",
        counts.non_enumerable,
        1,
    );
    // The agent-toolbox universe is a private upstream (`--sources` cannot list it), so
    // its 16 names are pinned here. A census that *dropped* it would report 2, which the
    // count above forbids; and a ported feature that vanished is named by the existence
    // check the positive case already requires. Pin the 16 by name so a rename cannot
    // quietly shrink the bound.
    let census_json: serde_json::Value = serde_json::from_str(&census).unwrap();
    let toolbox = census_json["universes"]
        .as_array()
        .unwrap()
        .iter()
        .find(|u| u["id"] == "agent_toolbox")
        .expect("agent_toolbox universe");
    h.eq(
        "bdd scope-check: 16 agent-toolbox workflows are pinned by name",
        toolbox["ported"].as_object().map(|m| m.len()).unwrap_or(0),
        16,
    );
    h.check(
        "bdd scope-check: agent_toolbox is marked non-enumerable",
        toolbox["enumerate"] == serde_json::json!(false),
    );

    let probs_of = |c: &str| {
        scope_check_impl(&root, c, None)
            .map(|(_, p)| p)
            .unwrap_or_default()
    };
    let any = |ps: &[String], needle: &str| ps.iter().any(|p| p.contains(needle));

    // Negative 1: a `ported` entry whose feature does not exist is named.
    let ghost = census.replace(
        "bdd/workflows/guards/agent_permission_gate.feature",
        "bdd/workflows/guards/ghost_permission_gate.feature",
    );
    h.check(
        "bdd scope-check: a ported entry with no feature is named (P124)",
        any(
            &probs_of(&ghost),
            "ghost_permission_gate.feature, but that file does not exist",
        ),
    );

    // Negative 2: a search-space with no reason is a silent skip.
    let no_reason = census.replace(
        "\"outlier_detect\": \"optimizer search-space over (window, threshold, fill); no deterministic policy\"",
        "\"outlier_detect\": \"\"",
    );
    h.check(
        "bdd scope-check: a search-space entry with no reason is named",
        any(
            &probs_of(&no_reason),
            "search_space[\"outlier_detect\"] needs a non-empty reason",
        ),
    );

    // Negative 3 (the real failure): against a checkout, an upstream workflow the
    // census never mentions must be caught. Build a temp root that holds exactly
    // the 10 self_evolving names, prove it is green, then add an 11th.
    let tag = std::process::id();
    let base = std::env::temp_dir().join(format!("laya-scope-{tag}"));
    let _ = std::fs::remove_dir_all(&base);
    let se = base.join("code/qwen_vm/self_evolving/workflows");
    std::fs::create_dir_all(&se).unwrap();
    for n in [
        "agent_permission",
        "anomaly_detection",
        "c_program_runner",
        "data_cleaning",
        "data_cleaning_columnar",
        "data_cleaning_multistep",
        "data_cleaning_real",
        "label_stability",
        "outlier_detect",
        "wasm_action_picker",
    ] {
        std::fs::write(se.join(format!("{n}.py")), "pass\n").unwrap();
    }
    std::fs::write(se.join("__init__.py"), "\n").unwrap();
    std::fs::create_dir_all(base.join("code/qwen_vm/app_scenarios")).unwrap();

    let (_, clean) = scope_check_impl(&root, &census, Some(&base)).unwrap();
    h.check(
        "bdd scope-check: the census matches a checkout holding exactly its names",
        clean.is_empty(),
    );

    std::fs::write(se.join("brand_new_workflow.py"), "pass\n").unwrap();
    let (_, drifted) = scope_check_impl(&root, &census, Some(&base)).unwrap();
    h.check(
        "bdd scope-check: a new upstream workflow not in the census is named (the real failure)",
        any(
            &drifted,
            "code/qwen_vm/self_evolving/workflows/brand_new_workflow.py is not in the census",
        ),
    );
    let _ = std::fs::remove_dir_all(&base);
}

/// A scenario that opens a page, asserts on it, then *releases* it must still
/// declare the `browser` handle in its generated spec. The release clears the
/// compiler's "a page is open now" flag, but the assert before it needed a live
/// browser, so keying the binding on the *final* state would strip the handle
/// and the plugin would die at run time with "this plugin has no browser
/// handle". `bdd/features/release.feature` is exactly this shape.
pub fn test_bdd_browser_binding(h: &mut Harness) {
    let open_assert_release = r##"
Feature: releasing a page
  Background:
    Given the browser is ready
    Given I am on "http://example.test/index.html"

  Scenario: releasing closes the tab the scenario was using
    Then the element "#heading" is visible
    When I release the page
"##;
    let f = bdd::parse(open_assert_release, "<test>").expect("parse browser feature");
    let spec = bdd::compile_scenario(&f, &f.scenarios[0], &json!({}), "chrome-headless.sh")
        .expect("compile open -> assert -> release scenario");
    h.eq(
        "bdd: open -> assert -> release still binds the browser handle",
        spec["capabilities"]["bdd"]["browser"].clone(),
        json!("chrome"),
    );

    // Negative control: a page-free exec scenario must NOT drag a browser along.
    // The plugin host brings up `browser` before any op, so a stray binding would
    // launch (or fail to find) Chrome for a scenario that never needs it.
    let exec_only = r##"
Feature: exec only
  Scenario: run and check
    When I run the command "true"
    Then the exit code is 0
"##;
    let f2 = bdd::parse(exec_only, "<test>").expect("parse exec feature");
    let spec2 = bdd::compile_scenario(&f2, &f2.scenarios[0], &json!({}), "chrome-headless.sh")
        .expect("compile exec-only scenario");
    h.check(
        "bdd: a page-free scenario declares no browser handle",
        spec2["capabilities"]["bdd"].get("browser").is_none(),
    );
}

/// The "is there a browser at all?" contract of `bdd run`: the probe policy
/// (explicit override beats `$PATH` beats platform defaults) and the
/// skip-versus-fail decision when nothing is found. Both are pure, so they are
/// pinned offline instead of only on a machine that happens to lack a browser.
pub fn test_bdd_browser_gate(h: &mut Harness) {
    use laya_workflow::bdd::{resolve_run_chrome_in, run_browser_gate, RunBrowserGate};

    let tag = std::process::id();
    let empty = std::env::temp_dir().join(format!("laya-bgate-empty-{tag}"));
    let branded = std::env::temp_dir().join(format!("laya-bgate-branded-{tag}"));
    let dirshadow = std::env::temp_dir().join(format!("laya-bgate-dir-{tag}"));
    for d in [&empty, &branded, &dirshadow] {
        let _ = std::fs::remove_dir_all(d);
        std::fs::create_dir_all(d).unwrap();
    }
    std::fs::write(branded.join("chromium-browser"), b"#!/bin/sh\n").unwrap();
    std::fs::write(branded.join("google-chrome"), b"#!/bin/sh\n").unwrap();
    std::fs::create_dir_all(dirshadow.join("chromium")).unwrap();

    // An explicit override wins outright - no filesystem, no platform probing -
    // because scripts/bdd/chrome-headless.sh `exec`s it and fails loudly if it is
    // wrong, so the gate must not second-guess it.
    h.eq(
        "bdd gate: an explicit override is taken as-is",
        resolve_run_chrome_in(Some("/opt/none/chrome"), &[empty.clone()]),
        Some("/opt/none/chrome".to_string()),
    );
    h.eq(
        "bdd gate: an empty override falls through to the PATH search",
        resolve_run_chrome_in(Some(""), &[empty.clone()]),
        None,
    );
    h.eq(
        "bdd gate: an empty directory yields no browser",
        resolve_run_chrome_in(None, &[empty.clone()]),
        None,
    );
    // Branded name dominates, and a *directory* named like the binary is not one.
    h.eq(
        "bdd gate: google-chrome on PATH wins over chromium-browser",
        resolve_run_chrome_in(None, &[branded.clone()]),
        Some(branded.join("google-chrome").to_string_lossy().into_owned()),
    );
    h.eq(
        "bdd gate: a directory named chromium is not a browser",
        resolve_run_chrome_in(None, &[dirshadow.clone()]),
        None,
    );

    // The decision itself: a browser always runs; its absence skips unless the
    // caller demanded one, in which case the absence is a failure.
    h.eq(
        "bdd gate: browser present, no requirement -> Proceed",
        run_browser_gate(true, false),
        RunBrowserGate::Proceed,
    );
    h.eq(
        "bdd gate: browser present, required -> Proceed",
        run_browser_gate(true, true),
        RunBrowserGate::Proceed,
    );
    h.eq(
        "bdd gate: no browser, required -> Fail",
        run_browser_gate(false, true),
        RunBrowserGate::Fail,
    );
    h.eq(
        "bdd gate: no browser, not required -> Skip",
        run_browser_gate(false, false),
        RunBrowserGate::Skip,
    );

    for d in [&empty, &branded, &dirshadow] {
        let _ = std::fs::remove_dir_all(d);
    }
}

/// The rule behind `bdd.release` refusing a release it must refuse: a close
/// counts only when the target has *left* the browser's list.
///
/// Chrome's `/json/close/{id}` is asynchronous - it keeps answering 200 "Target
/// is closing" while the target is still listed - so counting an accepted
/// request as a closed tab made `bdd.release` unable to refuse releasing a tab it
/// had already released. `bdd_release_probe` is the end-to-end gate for that; the
/// membership test it rests on is pinned here so the parsing cannot drift
/// without a browser being present.
pub fn test_bdd_release_teardown(h: &mut Harness) {
    use laya_workflow::capability::browser::target_listed_in;

    let list = json!([
        {"id": "AAA", "type": "page"},
        {"id": "BBB", "type": "background_page"}
    ]);
    h.eq(
        "release teardown: a listed target is found",
        target_listed_in(&list, "BBB"),
        true,
    );
    h.eq(
        "release teardown: an unlisted target is not",
        target_listed_in(&list, "CCC"),
        false,
    );
    h.eq(
        "release teardown: an empty list lists nothing",
        target_listed_in(&json!([]), "AAA"),
        false,
    );
    h.eq(
        "release teardown: a payload that is not a list lists nothing",
        target_listed_in(&json!({"id": "AAA"}), "AAA"),
        false,
    );
    h.eq(
        "release teardown: an entry whose id is not a string is skipped",
        target_listed_in(&json!([{"id": 7}, {"no_id": true}]), "7"),
        false,
    );
    h.eq(
        "release teardown: a non-string id never matches a string lookup",
        target_listed_in(&json!([{"id": 7}]), "null"),
        false,
    );
}

/// `bdd run --jobs N` gives every worker its own CDP endpoint, so two workers
/// must never be handed the same port. `free_port()` binds and releases, so it
/// can hand back a port an earlier worker already holds; the draw below is pinned
/// to the sequence a real host produced (4100, 4100, 4101).
pub fn test_bdd_jobs_ports(h: &mut Harness) {
    use laya_workflow::bdd::unique_worker_port;

    h.eq(
        "bdd jobs: the first draw is taken as-is",
        unique_worker_port(&[], || Ok(4100)).ok(),
        Some(4100u16),
    );
    let draws = std::cell::RefCell::new(vec![4100u16, 4100, 4101]);
    h.eq(
        "bdd jobs: a port an earlier worker holds is redrawn, never reused",
        unique_worker_port(&[4100], || Ok(draws.borrow_mut().remove(0))).ok(),
        Some(4101u16),
    );
    h.eq(
        "bdd jobs: a failing draw propagates instead of handing over a held port",
        unique_worker_port(&[4100], || Err(anyhow::anyhow!("no port"))).is_err(),
        true,
    );
}

/// `bdd run --jobs 0` sizes the pool from the core count, capped so a big box
/// does not cold-start one Chrome per core (the cap is measured - see
/// `BDD_AUTO_JOBS_CAP`) and never wider than the plan. The header line names
/// the reduction, so "the gate chose N" is never silently different from what
/// the flags said (P191).
pub fn test_bdd_jobs_auto(h: &mut Harness) {
    use laya_workflow::bdd::{auto_jobs, BDD_AUTO_JOBS_CAP};

    h.eq(
        "bdd jobs: zero cores still means one worker, never zero",
        auto_jobs(0, 25),
        1,
    );
    h.eq(
        "bdd jobs: auto takes one worker per core below the cap",
        auto_jobs(6, 25),
        6,
    );
    h.eq(
        "bdd jobs: auto stops at the cap even on a 96-core box",
        auto_jobs(96, 25),
        BDD_AUTO_JOBS_CAP,
    );
    h.eq(
        "bdd jobs: auto never spawns a worker with no scenario",
        auto_jobs(96, 3),
        3,
    );
    h.eq(
        "bdd jobs: a one-scenario plan stays serial under auto",
        auto_jobs(96, 1),
        1,
    );
}

/// `bdd port-check`: the cross-repo conversion is closed. The pure
/// symmetric-difference rules are pinned here, plus one real IO round-trip
/// against the checked-in `ole_eval` ports using a throwaway source tree.
pub fn test_bdd_port_check(h: &mut Harness) {
    use laya_workflow::bdd::{diff_port_sets, port_check, repo_root, PortCheckOptions};
    use std::collections::BTreeSet;
    let set = |xs: &[&str]| -> BTreeSet<String> { xs.iter().map(|s| s.to_string()).collect() };

    let same = set(&["a", "b", "c"]);
    h.check(
        "bdd port-check: identical sets have no drift",
        diff_port_sets(&same, &same, &same).is_empty(),
    );

    // Each drift direction lands on its own side and nowhere else.
    let d = diff_port_sets(
        &set(&["a", "b", "c"]),
        &set(&["a", "b"]),
        &set(&["a", "b", "c"]),
    );
    h.eq(
        "bdd port-check: a source missing its feature is flagged",
        d.sources_no_feature,
        vec!["c".to_string()],
    );
    h.check(
        "bdd port-check: ... and only that side is flagged",
        d.sources_no_spec.is_empty()
            && d.features_no_source.is_empty()
            && d.specs_no_source.is_empty(),
    );

    let d = diff_port_sets(
        &set(&["a"]),
        &set(&["a", "ghost_f"]),
        &set(&["a", "ghost_s"]),
    );
    h.eq(
        "bdd port-check: a feature with no source is flagged",
        d.features_no_source,
        vec!["ghost_f".to_string()],
    );
    h.eq(
        "bdd port-check: a spec with no source is flagged",
        d.specs_no_source,
        vec!["ghost_s".to_string()],
    );

    // The IO half: a throwaway source tree with a name that IS a port
    // (dialogue_policy), one that is not (brand_new), a __pycache__ dir and a
    // README file that must both be ignored.
    let tag = std::process::id();
    let src = std::env::temp_dir().join(format!("laya-portcheck-{tag}"));
    let rep = std::env::temp_dir().join(format!("laya-portcheck-{tag}.json"));
    let _ = std::fs::remove_dir_all(&src);
    std::fs::create_dir_all(src.join("dialogue_policy")).unwrap();
    std::fs::create_dir_all(src.join("brand_new")).unwrap();
    std::fs::create_dir_all(src.join("__pycache__")).unwrap();
    std::fs::write(src.join("README.md"), b"x").unwrap();

    let o = PortCheckOptions {
        sources: src.clone(),
        cat: "ole_eval".into(),
        json: Some(rep.to_string_lossy().into_owned()),
        ..PortCheckOptions::default()
    };
    h.eq(
        "bdd port-check: a drifted source tree exits 1",
        port_check(&o).ok(),
        Some(1),
    );
    let rj: Value = serde_json::from_str(&std::fs::read_to_string(&rep).unwrap()).unwrap();
    let list = |k: &str| -> Vec<String> {
        rj["diff"][k]
            .as_array()
            .map(|a| {
                a.iter()
                    .filter_map(Value::as_str)
                    .map(str::to_string)
                    .collect()
            })
            .unwrap_or_default()
    };
    h.check(
        "bdd port-check: an unported source is reported missing a feature",
        list("sources_no_feature").contains(&"brand_new".to_string()),
    );
    h.check(
        "bdd port-check: ... and missing a spec",
        list("sources_no_spec").contains(&"brand_new".to_string()),
    );
    h.check(
        "bdd port-check: __pycache__ is not a scenario",
        !list("sources_no_feature").contains(&"__pycache__".to_string()),
    );
    h.check(
        "bdd port-check: a real port (dialogue_policy) is not reported",
        !list("sources_no_feature").contains(&"dialogue_policy".to_string()),
    );

    // A source dir that does not exist is an error, never a silent pass.
    let bad = PortCheckOptions {
        sources: src.join("definitely-absent"),
        cat: "ole_eval".into(),
        json: None,
        ..PortCheckOptions::default()
    };
    h.check(
        "bdd port-check: a missing source dir is an error, not a pass",
        port_check(&bad).is_err(),
    );

    // ── the seal: the same gate's in-repo half ─────────────────────────────
    // A normal run has no second checkout, so the closure is proven against a
    // sealed source set checked in under bench/bdd/port_pins/. The names come
    // from the seal, never from the port set (that would be the port against
    // itself), and the README's "prove a cross-repo conversion is closed" is
    // exactly the claim a tautology would fake.
    let pin = std::env::temp_dir().join(format!("laya-portcheck-{tag}.pin.json"));
    let _ = std::fs::remove_file(&pin);
    h.check(
        "bdd port-check: --write-pin without --sources is an error, never an empty seal",
        port_check(&PortCheckOptions {
            cat: "ole_eval".into(),
            write_pin: Some(pin.clone()),
            ..PortCheckOptions::default()
        })
        .is_err(),
    );
    h.eq(
        "bdd port-check: sealing the live checkout is an act, and it exits 0",
        port_check(&PortCheckOptions {
            sources: src.clone(),
            cat: "ole_eval".into(),
            write_pin: Some(pin.clone()),
            ..PortCheckOptions::default()
        })
        .ok(),
        Some(0),
    );
    let sealed: Value = serde_json::from_str(&std::fs::read_to_string(&pin).unwrap()).unwrap();
    let sealed_names: Vec<String> = sealed["sources"]
        .as_array()
        .map(|a| {
            a.iter()
                .filter_map(|x| x["name"].as_str())
                .map(str::to_string)
                .collect()
        })
        .unwrap_or_default();
    h.check(
        "bdd port-check: the seal records every scenario and nothing else",
        sealed_names == vec!["brand_new".to_string(), "dialogue_policy".to_string()],
    );
    h.check(
        "bdd port-check: the seal carries a source fingerprint per scenario",
        sealed["sources"]
            .as_array()
            .map(|a| {
                a.iter().all(|x| {
                    x["fingerprint"].as_str().map(|f| f.len() == 16).unwrap_or(false)
                })
            })
            .unwrap_or(false),
    );
    // A seal whose names came from the source set still proves the port: the
    // unported `brand_new` in the seal is what makes this fail.
    h.eq(
        "bdd port-check: --pin proves the port against the seal (unported source)",
        port_check(&PortCheckOptions {
            cat: "ole_eval".into(),
            pin: Some(pin.clone()),
            ..PortCheckOptions::default()
        })
        .ok(),
        Some(1),
    );
    // The seal is the authority for the *names*, never the port set itself, so a
    // normal `check.sh` (no second checkout) runs against the checked-in seal.
    // This is the headline: the in-repo half closes.
    let checked_in = repo_root().join("bench").join("bdd").join("port_pins").join("ole_eval.json");
    h.eq(
        "bdd port-check: the checked-in seal closes the conversion in-repo",
        port_check(&PortCheckOptions {
            cat: "ole_eval".into(),
            pin: Some(checked_in.clone()),
            ..PortCheckOptions::default()
        })
        .ok(),
        Some(0),
    );
    // The checked-in copy is name-only - the corpus lives in another repo, so no
    // bytes of it (nor a hash of them) are quoted here. A *live* checkout then
    // still verifies it by name; only a fingerprint the seal actually carries is
    // compared against the source.
    let sealed: Value = serde_json::from_str(&std::fs::read_to_string(&checked_in).unwrap()).unwrap();
    let sealed_entries = sealed["sources"].as_array().cloned().unwrap_or_default();
    h.check(
        "bdd port-check: the checked-in seal is name-only, quoting no other repo's bytes",
        !sealed_entries.is_empty()
            && sealed_entries
                .iter()
                .all(|x| x["fingerprint"].as_str() == Some("")),
    );

    // Rebuild that name set into a live tree: a directory per sealed name. A live
    // checkout that is exactly the seal closes even with no fingerprint sealed.
    let live = std::env::temp_dir().join(format!("laya-portcheck-{tag}.live"));
    let _ = std::fs::remove_dir_all(&live);
    for e in &sealed_entries {
        let d = live.join(e["name"].as_str().unwrap());
        std::fs::create_dir_all(&d).unwrap();
        std::fs::write(d.join("engine.py"), b"pass\n").unwrap();
    }
    h.eq(
        "bdd port-check: a live checkout matching a name-only seal closes (no fingerprint sealed)",
        port_check(&PortCheckOptions {
            sources: live.clone(),
            cat: "ole_eval".into(),
            pin: Some(checked_in.clone()),
            ..PortCheckOptions::default()
        })
        .ok(),
        Some(0),
    );
    // A seal that *does* carry a fingerprint is compared against the source, so a
    // moved source cannot pass unnoticed.
    let moved = std::env::temp_dir().join(format!("laya-portcheck-{tag}.pin1.json"));
    let mut drifted = sealed.clone();
    drifted["sources"][0]["fingerprint"] = json!("0000000000000000");
    std::fs::write(&moved, serde_json::to_string_pretty(&drifted).unwrap()).unwrap();
    h.eq(
        "bdd port-check: a sealed fingerprint that no longer matches fails",
        port_check(&PortCheckOptions {
            sources: live.clone(),
            cat: "ole_eval".into(),
            pin: Some(moved.clone()),
            ..PortCheckOptions::default()
        })
        .ok(),
        Some(1),
    );
    // A seal naming a source the port never converted does not close...
    let ghost = std::env::temp_dir().join(format!("laya-portcheck-{tag}.pin2.json"));
    let mut with_ghost = sealed.clone();
    with_ghost["sources"]
        .as_array_mut()
        .unwrap()
        .push(json!({"name": "ghost_source", "fingerprint": ""}));
    std::fs::write(&ghost, serde_json::to_string_pretty(&with_ghost).unwrap()).unwrap();
    h.eq(
        "bdd port-check: --pin alone fails when the seal names an unported source",
        port_check(&PortCheckOptions {
            cat: "ole_eval".into(),
            pin: Some(ghost.clone()),
            ..PortCheckOptions::default()
        })
        .ok(),
        Some(1),
    );
    // ...and neither does a seal that has lost one port (a port with no source).
    let short = std::env::temp_dir().join(format!("laya-portcheck-{tag}.pin3.json"));
    let mut missing = sealed.clone();
    missing["sources"].as_array_mut().unwrap().pop();
    std::fs::write(&short, serde_json::to_string_pretty(&missing).unwrap()).unwrap();
    h.eq(
        "bdd port-check: --pin alone fails when a port has no source in the seal",
        port_check(&PortCheckOptions {
            cat: "ole_eval".into(),
            pin: Some(short.clone()),
            ..PortCheckOptions::default()
        })
        .ok(),
        Some(1),
    );
    // With both, the seal is put on trial: the live checkout holds a source the
    // seal never saw, so the seal itself is what fails.
    h.eq(
        "bdd port-check: --pin + --sources puts the seal on trial",
        port_check(&PortCheckOptions {
            sources: src.clone(),
            cat: "ole_eval".into(),
            pin: Some(ghost.clone()),
            ..PortCheckOptions::default()
        })
        .ok(),
        Some(1),
    );
    h.check(
        "bdd port-check: a seal that is not there is an error, not a pass",
        port_check(&PortCheckOptions {
            cat: "ole_eval".into(),
            pin: Some(src.join("no-such-seal.json")),
            ..PortCheckOptions::default()
        })
        .is_err(),
    );
    h.check(
        "bdd port-check: neither --sources nor --pin is an error, not a silent pass",
        port_check(&PortCheckOptions {
            cat: "ole_eval".into(),
            ..PortCheckOptions::default()
        })
        .is_err(),
    );

    let _ = std::fs::remove_dir_all(&src);
    let _ = std::fs::remove_file(&rep);
    let _ = std::fs::remove_file(&pin);
    let _ = std::fs::remove_dir_all(&live);
    let _ = std::fs::remove_file(&moved);
    let _ = std::fs::remove_file(&ghost);
    let _ = std::fs::remove_file(&short);
}

pub fn test_bdd_import(h: &mut Harness) {
    use laya_workflow::bdd::{compile_decision, import_spec, parse, repo_root, same_graph};

    // A spec that exercises the whole surface: a capability config holding a
    // `|`, a match_regex pattern holding a `"`, a heuristic with defaults to
    // fill, a threshold rule that omits its question, `otherwise`, keep-keys, a
    // confidence floor and a retry budget.
    let spec = json!({
        "name": "import_probe",
        "dsl_version": 2,
        "description": "A probe with a | pipe and a \"quote\".",
        "start": "classify",
        "max_iterations": 7,
        "convergence_window": 5,
        "convergence_eps": 0.0001,
        "policy": {
            "allow_exec": false,
            "allow_hosts": ["a.example"],
            "max_timeout_ms": 2000
        },
        "capabilities": {
            "disk": {"kind": "shell", "command": "du -sk \"${state.work_dir}\" | awk '{print $1}'"},
            "cls": {"kind": "needle", "op": "complete", "cact": "${env.NEEDLE3_CACT}"}
        },
        "nodes": [
            {
                "name": "classify",
                "primary_q": "intent",
                "questions": {
                    "intent": {
                        "type": "choice",
                        "instructions": "pick one",
                        "criteria": {"A": "first", "B": "second"},
                        "heuristic": {"answer_field": "i_intent"}
                    }
                },
                "edge": {"condition": {"A": "done"}, "default": "STOP"},
                "min_confidence": 0.4
            },
            {
                "name": "done",
                "primary_q": "verdict",
                "questions": {
                    "verdict": {
                        "type": "choice",
                        "instructions": "",
                        "criteria": {"ok": "fine"},
                        "heuristic": {
                            "field": "text",
                            "match_regex": ["\"quoted\"", "a|b"],
                            "p_hit": 0.9,
                            "p_miss": 0.1
                        }
                    }
                },
                "edge": {"condition": {}, "default": "STOP"},
                "action": {
                    "kind": "threshold",
                    "question": "verdict",
                    "rules": [
                        {"when": {"answer": "ok"}, "label": "ACCEPT"},
                        {"when": {"value_gte": 1.5}, "label": "STRONG"},
                        {"when": {"always": true}, "label": "NOOP"}
                    ]
                },
                "state": {"keep": ["alpha", "beta"]},
                "max_retries": 2
            }
        ]
    });

    let text = match import_spec(&spec) {
        Ok(t) => t,
        Err(e) => {
            h.check(&format!("bdd import: a full spec imports ({e:#})"), false);
            return;
        }
    };
    h.check(
        "bdd import: the feature opens with the @decision tag",
        text.starts_with("@decision\nFeature: import_probe\n"),
    );
    h.check(
        "bdd import: a `|` in a capability config is escaped for the table",
        text.contains("\\| awk"),
    );
    h.check(
        "bdd import: a `\"` in a match_regex pattern is escaped for the token list",
        text.contains("\"\\\"quoted\\\"\""),
    );
    h.check(
        "bdd import: a question-less value_gte rule is spelled with the action's question",
        text.contains("when \"verdict\" is at least 1.5"),
    );
    h.check(
        "bdd import: a start node is emitted first",
        text.find("Scenario: classify").unwrap_or(usize::MAX)
            < text.find("Scenario: done").unwrap_or(usize::MAX),
    );

    let back = parse(&text, "<import>").expect("the imported feature parses");
    let rebuilt = compile_decision(&back, &json!({})).expect("the imported feature compiles");
    h.check(
        "bdd import: the imported feature compiles back to the same graph",
        same_graph(&spec, &rebuilt),
    );

    // The IO half on a real checked-in spec: its capability config holds a real
    // shell pipe, so this pins the table-cell escape against real data.
    let real = repo_root()
        .join("dsl")
        .join("capabilities")
        .join("data_pipeline_local.json");
    match std::fs::read_to_string(&real) {
        Ok(t) => {
            let rs: Value = serde_json::from_str(&t).unwrap();
            match import_spec(&rs) {
                Ok(ft) => match parse(&ft, "<import>") {
                    Ok(f) => {
                        let rb = compile_decision(&f, &json!({})).unwrap();
                        h.check(
                            "bdd import: a real spec (data_pipeline_local) recompiles identically",
                            same_graph(&rs, &rb),
                        );
                    }
                    Err(e) => h.check(&format!("bdd import: real feature parses ({e:#})"), false),
                },
                Err(e) => h.check(&format!("bdd import: real spec imports ({e:#})"), false),
            }
        }
        Err(e) => h.check(&format!("bdd import: real spec is present ({e})"), false),
    }

    // A sub-workflow reference node whose handoff default is `STOP` must survive
    // the round-trip: the compiler records an explicit `edge.default: "STOP"` for
    // `continue at "STOP"` but *no edge at all* for a reference node with no
    // handoff step, so the importer has to re-emit the `continue at "STOP"` step
    // (omitting it recompiles to a node with no edge and `same_graph` fails). The
    // live case is `dsl/apps/gateway_triage.json` (safe path -> the email_triage
    // chain), whose `triage` node hands off at `STOP`.
    let compose = repo_root().join("dsl").join("apps").join("gateway_triage.json");
    match std::fs::read_to_string(&compose) {
        Ok(t) => {
            let cs: Value = serde_json::from_str(&t).unwrap();
            match import_spec(&cs) {
                Ok(ft) => {
                    h.check(
                        "bdd import: a STOP handoff on a sub-workflow node is re-emitted",
                        ft.contains("completes continue at \"STOP\""),
                    );
                    match parse(&ft, "<import>") {
                        Ok(f) => match compile_decision(&f, &json!({})) {
                            Ok(rb) => h.check(
                                "bdd import: apps/gateway_triage (sub-workflow STOP handoff) round-trips",
                                same_graph(&cs, &rb),
                            ),
                            Err(e) => h.check(
                                &format!("bdd import: gateway feature compiles ({e:#})"),
                                false,
                            ),
                        },
                        Err(e) => h.check(
                            &format!("bdd import: gateway feature parses ({e:#})"),
                            false,
                        ),
                    }
                }
                Err(e) => h.check(&format!("bdd import: gateway spec imports ({e:#})"), false),
            }
        }
        Err(e) => h.check(&format!("bdd import: gateway spec is present ({e})"), false),
    }

    // Inexpressible specs are refused by name, never silently mis-imported.
    let bad_kind = json!({"name":"x","start":"n","nodes":[{"name":"n","primary_q":"ok","questions":{},"edge":{"condition":{},"default":"STOP"},"action":{"kind":"teleport"}}]});
    h.check(
        "bdd import: an unknown action kind is an error, not a silent pass",
        import_spec(&bad_kind).is_err(),
    );
    let bad_when = json!({"name":"x","start":"n","nodes":[{"name":"n","primary_q":"ok","questions":{},"edge":{"condition":{},"default":"STOP"},"action":{"kind":"threshold","question":"ok","rules":[{"when":{"telepathy":1},"label":"X"}]}}]});
    h.check(
        "bdd import: an unsupported threshold `when` is an error",
        import_spec(&bad_when).is_err(),
    );
    let bad_start = json!({"name":"x","start":"missing","nodes":[{"name":"n","primary_q":"ok","questions":{},"edge":{"condition":{},"default":"STOP"}}]});
    h.check(
        "bdd import: a start node that does not exist is an error",
        import_spec(&bad_start).is_err(),
    );
    let not_a_spec = json!(["not", "an", "object"]);
    h.check(
        "bdd import: a non-object is an error",
        import_spec(&not_a_spec).is_err(),
    );
    // P143: a node `state.rename` projection now has a feature step, so `import`
    // reproduces it and the recompiled graph is identical (P142 made it a loud
    // refusal only because the step did not exist yet).
    let with_keep = json!({"name":"x","start":"n","nodes":[{"name":"n","primary_q":"ok","questions":{"ok":{"type":"choice","instructions":"?","criteria":{"A":"a","B":"b"}}},"edge":{"condition":{"A":"STOP"},"default":"STOP"},"state":{"keep":["a"]},"action":{"kind":"copy_keys","keys":["ok"]}}]});
    h.check(
        "bdd import: a plain state.keep projection imports",
        import_spec(&with_keep).is_ok(),
    );
    let with_rename = json!({"name":"x","start":"n","nodes":[{"name":"n","primary_q":"ok","questions":{"ok":{"type":"choice","instructions":"?","criteria":{"A":"a","B":"b"}}},"edge":{"condition":{"A":"STOP"},"default":"STOP"},"state":{"keep":["a"],"rename":{"out":"a"}},"action":{"kind":"copy_keys","keys":["ok"]}}]});
    match import_spec(&with_rename) {
        Ok(ft) => {
            h.check(
                "bdd import: a state.rename projection emits a `map the state key` step",
                ft.contains("map the state key \"out\" from \"a\""),
            );
            match parse(&ft, "<import>") {
                Ok(f) => match compile_decision(&f, &json!({})) {
                    Ok(re) => h.check(
                        "bdd import: a state.rename projection recompiles to the same state",
                        re["nodes"][0]["state"] == json!({"keep": ["a"], "rename": {"out": "a"}}),
                    ),
                    Err(e) => h.check(&format!("bdd import: rename feature compiles ({e:#})"), false),
                },
                Err(e) => h.check(&format!("bdd import: rename feature parses ({e:#})"), false),
            }
        }
        Err(e) => h.check(&format!("bdd import: rename spec imports ({e:#})"), false),
    }
}

/// `bdd import --dir`: lift a whole tree of JSON workflows into `.feature`s, one
/// per decision spec, each verified before it is written. `*.config.json`
/// sidecars are skipped by name, any json without a `nodes` array is skipped as
/// "not a decision spec", and a directory with nothing importable is an error
/// rather than a silent pass — the `--dir` twin of the single-file `bdd import`.
/// The `--into` filing (`repo_cat_paths` + `normalise_name`) is what makes a
/// ported spec/feature pair pass both `round-trip` and `import-check`.
pub fn test_bdd_import_dir(h: &mut Harness) {
    use laya_workflow::bdd::{
        collect_spec_dir, compile_decision, import, import_spec, normalise_name, parse,
        repo_cat_paths, same_graph, ImportOptions,
    };

    let base = std::env::temp_dir().join(format!("laya-import-dir-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&base);
    let src = base.join("src");
    let out = base.join("out");
    let mk = |rel: &str, body: &str| {
        let p = src.join(rel);
        std::fs::create_dir_all(p.parent().unwrap()).unwrap();
        std::fs::write(&p, body).unwrap();
    };

    // A one-node decision spec that survives import: a choice question -> STOP,
    // with `max_iterations` pinned the way every checked-in spec pins it.
    let one = |name: &str| {
        json!({
            "name": name,
            "description": "probe",
            "start": "go",
            "max_iterations": 12,
            "nodes": [{
                "name": "go",
                "primary_q": "ok",
                "questions": {"ok": {"type": "choice", "instructions": "i", "criteria": {"A": "a", "B": "b"}}},
                "edge": {"condition": {}, "default": "STOP"}
            }]
        })
    };
    mk("alpha.json", &serde_json::to_string(&one("alpha")).unwrap());
    mk(
        "nested/beta.json",
        &serde_json::to_string(&one("beta")).unwrap(),
    );
    mk("notes.json", "{\"note\": \"this is not a workflow\"}");
    mk("bad.json", "{ not json at all");
    mk("side.config.json", "{\"pages\": 3}");

    // (1) discovery: decision specs found recursively; non-specs skipped.
    let (found, skipped) = collect_spec_dir(&src).expect("collect_spec_dir");
    let names: Vec<String> = found.iter().map(|f| f.name.clone()).collect();
    h.check(
        "bdd import --dir: finds decision specs recursively (alpha + nested/beta)",
        names == vec!["alpha".to_string(), "beta".to_string()],
    );
    h.check(
        "bdd import --dir: `*.config.json` is ignored, never a skip complaint",
        skipped.iter().all(|s| !s.contains("side.config.json")),
    );
    h.check(
        "bdd import --dir: a non-workflow json and a broken json are both skipped",
        skipped.len() == 2
            && skipped.iter().any(|s| s.contains("notes.json"))
            && skipped.iter().any(|s| s.contains("bad.json")),
    );

    // (2) every found spec imports and recompiles to the same graph.
    for f in &found {
        let ok = match import_spec(&f.spec) {
            Ok(t) => match parse(&t, "<import-dir>") {
                Ok(p) => compile_decision(&p, &json!({}))
                    .map(|rb| same_graph(&f.spec, &rb))
                    .unwrap_or(false),
                Err(_) => false,
            },
            Err(_) => false,
        };
        h.check(
            &format!("bdd import --dir: `{}` round-trips through import", f.name),
            ok,
        );
    }

    // (3) the write half: `--out <DIR>` writes one `.feature` per decision spec.
    let dir_arg = src.to_string_lossy().into_owned();
    let out_arg = out.to_string_lossy().into_owned();
    let rc = import(&ImportOptions {
        dir: Some(dir_arg.clone()),
        out: Some(out_arg.clone()),
        ..Default::default()
    })
    .expect("import --dir --out");
    h.check(
        "bdd import --dir --out: exits 0 when everything imports",
        rc == 0,
    );
    h.check(
        "bdd import --dir --out: a .feature lands for each decision spec",
        out.join("alpha.feature").is_file() && out.join("beta.feature").is_file(),
    );
    h.check(
        "bdd import --dir --out: no file is written for a skipped json",
        !out.join("notes.feature").exists() && !out.join("bad.feature").exists(),
    );

    // (4) a second run collides (rc 1); --force clears it.
    let rc_again = import(&ImportOptions {
        dir: Some(dir_arg.clone()),
        out: Some(out_arg.clone()),
        ..Default::default()
    })
    .expect("import --dir --out (again)");
    h.check(
        "bdd import --dir --out: an existing target is a collision (rc 1)",
        rc_again == 1,
    );
    let rc_force = import(&ImportOptions {
        dir: Some(dir_arg.clone()),
        out: Some(out_arg.clone()),
        force: true,
        ..Default::default()
    })
    .expect("import --dir --out --force");
    h.check(
        "bdd import --dir --out --force: overwrites and exits 0",
        rc_force == 0,
    );

    // (5) a dry run (no --out / --into) writes nothing and exits 0.
    let rc_dry = import(&ImportOptions {
        dir: Some(dir_arg.clone()),
        ..Default::default()
    })
    .expect("import --dir (dry run)");
    h.check(
        "bdd import --dir: a dry run exits 0 and writes nothing",
        rc_dry == 0 && !out.join("gamma.feature").exists(),
    );

    // (6) a directory with no decision spec is an error, never a silent pass.
    let empty = base.join("empty");
    std::fs::create_dir_all(&empty).unwrap();
    h.check(
        "bdd import --dir: a directory with no decision spec is an error",
        import(&ImportOptions {
            dir: Some(empty.to_string_lossy().into_owned()),
            ..Default::default()
        })
        .is_err(),
    );

    // (7) `<SPEC>` and `--dir` are mutually exclusive.
    h.check(
        "bdd import: <SPEC> together with --dir is rejected",
        import(&ImportOptions {
            spec: Some("a.json".into()),
            dir: Some("b".into()),
            ..Default::default()
        })
        .is_err(),
    );

    // (8) the `--into` filing: feature under bdd/workflows/<cat>/, spec under dsl/<cat>/.
    let root = std::path::Path::new("/repo");
    let (feat, spec_out) = repo_cat_paths(root, "scratch", "alpha");
    h.check(
        "bdd import --into: the port files under bdd/workflows/<cat>/ and dsl/<cat>/",
        feat == root.join("bdd/workflows/scratch/alpha.feature")
            && spec_out == root.join("dsl/scratch/alpha.json"),
    );
    let pinned = normalise_name(&one("a display name"), "alpha");
    h.check(
        "bdd import --into: the filed spec's name is pinned to the port stem",
        pinned.get("name").and_then(Value::as_str) == Some("alpha"),
    );
    let pinned_feature = match import_spec(&pinned) {
        Ok(t) => t,
        Err(e) => {
            h.check(
                &format!("bdd import --into: pinned spec imports ({e:#})"),
                false,
            );
            return;
        }
    };
    let pinned_back =
        compile_decision(&parse(&pinned_feature, "<import-dir>").unwrap(), &json!({})).unwrap();
    h.check(
        "bdd import --into: the pinned spec is self-consistent (name == stem)",
        same_graph(&pinned, &pinned_back),
    );

    let _ = std::fs::remove_dir_all(&base);
}


/// `bdd_import` MCP tool: lift a checked-in dsl spec back into a `@decision`
/// feature, in-process — the agent-facing counterpart of `bdd import`, and the
/// in-process proof that any known JSON workflow can be lifted into BDD. The
/// emitted feature must recompile to the very same graph (`graph_matches`), a
/// non-workflow JSON must be rejected, and a missing input must error cleanly.
pub fn test_bdd_mcp_import(h: &mut Harness) {
    let t = bdd_tool("bdd_import");
    let r = t
        .call(&json!({"spec_path": "dsl/guards/agent_permission_gate.json"}))
        .unwrap();
    h.eq("bdd_import: ok", r["ok"].as_bool().unwrap(), true);
    h.eq("bdd_import: graph matches", r["graph_matches"].as_bool().unwrap(), true);
    let text = r["feature_text"].as_str().unwrap_or("").to_string();
    h.check("bdd_import: emits @decision", text.starts_with("@decision\n"));
    // The emitted text must itself pass the authoring gate.
    let v = bdd_tool("bdd_validate")
        .call(&json!({"feature_text": text}))
        .unwrap();
    h.eq("bdd_import: imported feature passes bdd_validate", v["ok"].as_bool().unwrap(), true);
    // A non-workflow JSON (no nodes) is refused, not silently accepted.
    h.check(
        "bdd_import: non-spec JSON errors",
        t.call(&json!({"spec_json": "{\"hello\": 1}"})).is_err(),
    );
    // No input errors cleanly instead of panicking.
    h.check("bdd_import: no input errors", t.call(&json!({})).is_err());
}

/// `bdd_decide` MCP tool: run a `@decision` workflow against a state and get
/// the verdict + full trace — the callable core of a BDD graph for an agent.
/// Uses the same deterministic offline engine as `dsl smoke`.
pub fn test_bdd_mcp_decide(h: &mut Harness) {
    let t = bdd_tool("bdd_decide");
    // A 1-step high-risk path: BLOCK.
    let r = t
        .call(&json!({
            "feature_path": "bdd/workflows/guards/agent_permission_gate.feature",
            "state": {"ev_unauthenticated": "halt", "ev_risk": "high"}
        }))
        .unwrap();
    h.eq("bdd_decide: ok", r["ok"].as_bool().unwrap(), true);
    h.eq("bdd_decide: label BLOCK", r["label"].clone(), json!("BLOCK"));
    h.eq("bdd_decide: one step", r["trace"]["steps"].as_array().unwrap().len(), 1);
    // The same workflow on a checked-in spec, a 3-step login→authorize→halt path.
    let r2 = t
        .call(&json!({
            "spec_path": "dsl/guards/agent_permission_gate.json",
            "state": {"ev_unauthenticated": "login", "ev_authenticated": "authorize", "ev_authorized": "halt", "ev_risk": "low"}
        }))
        .unwrap();
    h.eq("bdd_decide: spec_path label EXECUTE", r2["label"].clone(), json!("EXECUTE"));
    h.eq("bdd_decide: 3 steps", r2["trace"]["steps"].as_array().unwrap().len(), 3);
    h.eq("bdd_decide: step0 node", r2["trace"]["steps"][0]["node"].clone(), json!("unauthenticated"));
    h.check("bdd_decide: result object", r2["result"].is_object());
    // A browser-step feature is refused (that path is `bdd run`).
    h.check(
        "bdd_decide: browser feature refused",
        t.call(&json!({"feature_path": "bdd/features/navigation.feature", "state": {}})).is_err(),
    );
    // `state` is required.
    h.check(
        "bdd_decide: no state errors",
        t.call(&json!({"feature_path": "bdd/workflows/guards/agent_permission_gate.feature"})).is_err(),
    );
    // The decision contract is reported: `inputs` is the declared set of offline
    // state fields the graph reads and `missing_inputs` is that set minus what the
    // caller supplied — the honest signal that an under-fed call returns defaults.
    let empty = t
        .call(&json!({"name": "agent_permission_gate", "state": {}}))
        .unwrap();
    let declared: Vec<String> = empty["inputs"]
        .as_array()
        .unwrap()
        .iter()
        .filter_map(|v| v.as_str().map(str::to_string))
        .collect();
    let missing: Vec<String> = empty["missing_inputs"]
        .as_array()
        .unwrap()
        .iter()
        .filter_map(|v| v.as_str().map(str::to_string))
        .collect();
    h.check(
        "bdd_decide: inputs lists the declared offline fields",
        declared
            == vec!["ev_authenticated", "ev_authorized", "ev_blocked", "ev_risk", "ev_unauthenticated"],
    );
    h.check(
        "bdd_decide: an empty state flags every declared input missing",
        missing == declared && !missing.is_empty(),
    );
    let fed = t
        .call(&json!({"name": "agent_permission_gate", "state": {
            "ev_unauthenticated": "login", "ev_authenticated": "authorize",
            "ev_authorized": "halt", "ev_risk": "low", "ev_blocked": "no"}}))
        .unwrap();
    h.check(
        "bdd_decide: a fully-fed state has nothing missing",
        fed["missing_inputs"].as_array().unwrap().is_empty(),
    );
    // The label is the terminal decision's own projection, not just `stop`: a
    // `combine` graph returns its `route`, a `rubric` graph its `status`.
    let route = t
        .call(&json!({"name": "support_routing", "state": {
            "message": "invoice due today, customer charged twice, needs refund urgently"}}))
        .unwrap();
    h.eq("bdd_decide: combine graph returns its route", route["label"].clone(), json!("billing"));
    h.check(
        "bdd_decide: combine result carries route + urgency",
        route["result"]["route"] == json!("billing") && route["result"]["urgency"] == json!("high"),
    );
    // The same declared set drives `missing_inputs`: a heuristic-`field`-only
    // graph (support_routing) now reports its `message` field as missing on an
    // empty state, instead of pretending it needs nothing.
    let sr_empty = t.call(&json!({"name": "support_routing", "state": {}})).unwrap();
    h.check(
        "bdd_decide: a heuristic-field graph flags its field missing",
        sr_empty["inputs"].clone() == json!(["message"])
            && sr_empty["missing_inputs"].clone() == json!(["message"]),
    );
    let rubric = t
        .call(&json!({"name": "quality_rubric", "state": {
            "rubric_usefulness_hint": "high", "rubric_clarity_hint": "high"}}))
        .unwrap();
    h.eq("bdd_decide: rubric graph returns its status", rubric["label"].clone(), json!("scored"));
    // Offline honesty rides along with the decision contract: an offline-verified
    // graph reports offline=true and no live gate; a live-only graph cannot be
    // walked offline, so the call fails with the live gate named rather than
    // returning a defaults-only label.
    let off = t.call(&json!({"name": "agent_permission_gate", "state": {}})).unwrap();
    h.eq("bdd_decide: an offline graph reports offline=true", off["offline"].clone(), json!(true));
    h.check(
        "bdd_decide: an offline graph carries no live gate",
        off["verified_by"].is_null() && off["why_live"].is_null(),
    );
    let live_err = t
        .call(&json!({"name": "arxiv", "state": {}}))
        .unwrap_err()
        .to_string();
    h.check(
        "bdd_decide: a live-only graph fails offline and names its live gate",
        live_err.contains("not offline-verifiable") && live_err.contains("bdd live"),
    );
}


/// P89: the authoring diagnostics of `bdd build` must carry the **cause**, not
/// just the line. Three fixes, end-to-end through the real CLI binary:
///   1. a step-level error keeps its anyhow cause chain (a forgotten `@decision`
///      says so instead of printing a bare step echo);
///   2. a duplicate Scenario name is reported at the Scenario's own `line N:`
///      (it is a node id, and every Scenario is one node);
///   3. a mangled step keyword (`Givn`) suggests the `Given` keyword.
/// All three are the same red an agent sees mid-authoring, so they are the
/// difference between "fix the typo" and "stare at a bare line".
pub fn test_bdd_cli_diagnostics(h: &mut Harness) {
    let root = bdd::repo_root();
    let mut bin = std::env::current_exe().expect("test binary path");
    bin.pop();
    if bin.ends_with("deps") {
        bin.pop();
    }
    bin.push("laya-workflow");
    let dir = std::env::temp_dir().join(format!("laya-bdd-diag-{}", std::process::id()));
    std::fs::create_dir_all(&dir).expect("temp diag dir");
    let run = |name: &str, text: &str| -> String {
        let f = dir.join(format!("{name}.feature"));
        std::fs::write(&f, text).expect("write feature");
        let out = std::process::Command::new(&bin)
            .args([
                "bdd",
                "build",
                &f.to_string_lossy(),
                "--out",
                &dir.join(format!("out-{name}")).to_string_lossy(),
            ])
            .current_dir(&root)
            .output()
            .expect("run bdd build");
        format!(
            "{}\n{}",
            String::from_utf8_lossy(&out.stdout),
            String::from_utf8_lossy(&out.stderr)
        )
    };

    // 1. A decision-vocabulary feature that forgot `@decision` must name the tag.
    let no_tag = "\
Feature: diag_no_tag
  x

  Scenario: classify
    Given a choice question \"q\" asks \"Which team?\"
      | answer | meaning |
      | A      | first   |
    When any other answer goes to \"STOP\"
    Then keep the answer to \"q\"
";
    let out = run("diag_no_tag", no_tag);
    h.check(
        "p89: missing @decision names the tag, not just the step",
        out.contains("@decision") && out.contains("only valid in a @decision feature"),
    );
    h.check(
        "p89: missing @decision still carries the step line",
        out.contains("line 5:"),
    );

    // 2. Duplicate Scenario name = duplicate node name, located at the Scenario.
    let dup = "\
@decision
Feature: diag_dup
  dup

  Scenario: classify
    Given a choice question \"q\" asks \"Which team?\"
      | answer | meaning |
      | A      | first   |
    When any other answer goes to \"STOP\"
    Then keep the answer to \"q\"

  Scenario: classify
    Given a score question \"s\" asks \"How big?\"
      | level |
      | low   |
    When any other answer goes to \"STOP\"
    Then label \"X\" when \"s\" is at least 1.0
    And label \"Y\" otherwise
";
    let out = run("diag_dup", dup);
    h.check(
        "p89: duplicate node name is rejected with the scenario's line",
        out.contains("line 12: duplicate node name \"classify\""),
    );

    // 3. A mangled step keyword suggests the keyword it meant.
    let typo = "\
@decision
Feature: diag_typo
  typo

  Scenario: classify
    Givn a choice question \"q\" asks \"Which team?\"
      | answer | meaning |
      | A      | first   |
";
    let out = run("diag_typo", typo);
    h.check(
        "p89: a mangled step keyword suggests the keyword",
        out.contains("did you mean the `Given` keyword"),
    );
}

/// `bdd_list` MCP tool: the callable inventory behind "use a BDD graph as a
/// decision core" — every @decision workflow with the questions and state
/// inputs an agent must know to call it. Also proves `bdd_decide {name}`
/// resolves a port name through the same corpus.
pub fn test_bdd_mcp_list(h: &mut Harness) {
    let t = bdd_tool("bdd_list");
    let all = t.call(&json!({})).unwrap();
    let total = all["count"].as_u64().unwrap();
    h.check("bdd_list: lists the whole corpus", total >= 160);
    // Offline honesty partitions the corpus: `offline_count` + `live_count` == the
    // returned count, and every graph says whether offline behaviour-verification
    // exists (same ledger `dsl smoke` counts as the live-declared set).
    let offline = all["offline_count"].as_u64().unwrap();
    let live = all["live_count"].as_u64().unwrap();
    h.check(
        "bdd_list: offline_count + live_count == count",
        offline + live == total,
    );
    h.eq("bdd_list: unfiltered live_count is the ledger's 35", live, 35);
    let live_wf = all["workflows"]
        .as_array()
        .unwrap()
        .iter()
        .find(|w| w["name"] == "arxiv")
        .expect("browser/arxiv is in the corpus");
    h.eq(
        "bdd_list: a live-only graph reports offline=false",
        live_wf["offline"].clone(),
        json!(false),
    );
    h.check(
        "bdd_list: a live-only graph names its live gate + reason",
        live_wf["verified_by"].as_str().map(|s| !s.is_empty()).unwrap_or(false)
            && live_wf["why_live"].as_str().map(|s| !s.is_empty()).unwrap_or(false),
    );
    let off_wf = t.call(&json!({"name": "agent_permission_gate"})).unwrap();
    let off_entry = &off_wf["workflows"][0];
    h.eq(
        "bdd_list: an offline graph reports offline=true",
        off_entry["offline"].clone(),
        json!(true),
    );
    h.check(
        "bdd_list: an offline graph carries no live gate",
        off_entry["verified_by"].is_null() && off_entry["why_live"].is_null(),
    );
    let per_cat = t.call(&json!({"cat": "guards"})).unwrap();
    h.check(
        "bdd_list: cat filter narrows",
        per_cat["count"].as_u64().unwrap() < total,
    );
    let one = t.call(&json!({"name": "agent_permission_gate"})).unwrap();
    h.eq("bdd_list: name filter is exact", one["count"].as_u64().unwrap(), 1);
    // `query` is the discovery filter: the unfiltered inventory is hundreds of
    // KB, so an agent finds a graph by keyword rather than reading it whole.
    h.eq("bdd_list: total is the unfiltered corpus size", all["total"].clone(), json!(total));
    let q_one = t.call(&json!({"query": "AGENT_PERMISSION_GATE"})).unwrap();
    h.check(
        "bdd_list: query is case-insensitive and matches a name",
        q_one["count"].as_u64().unwrap() >= 1
            && q_one["workflows"]
                .as_array()
                .unwrap()
                .iter()
                .any(|w| w["name"] == "agent_permission_gate"),
    );
    h.eq("bdd_list: query leaves total at the corpus size", q_one["total"].clone(), json!(total));
    let q_and = t.call(&json!({"query": "agent_permission_gate zzznosuchterm"})).unwrap();
    h.eq(
        "bdd_list: query terms are ANDed (an unmatched term empties the set)",
        q_and["count"].as_u64().unwrap(),
        0,
    );
    let q_kw = t.call(&json!({"query": "memory store"})).unwrap();
    let q_kw_n = q_kw["count"].as_u64().unwrap();
    h.check(
        "bdd_list: a multi-term query narrows the corpus",
        q_kw_n >= 1 && q_kw_n < total,
    );
    // `summary` is the cheap discovery projection: no per-node `nodes` and no
    // workflow-level `questions`, but the fields a chooser needs stay.
    let summ = t.call(&json!({"name": "agent_permission_gate", "summary": true})).unwrap();
    let sw = &summ["workflows"][0];
    h.check(
        "bdd_list: summary drops the heavy nodes + questions",
        sw.get("nodes").is_none() && sw.get("questions").is_none(),
    );
    h.check(
        "bdd_list: summary keeps the discovery fields",
        sw.get("outputs").is_some()
            && sw.get("inputs").is_some()
            && sw.get("node_count").is_some()
            && sw.get("offline").is_some(),
    );
    h.check(
        "bdd_list: summary is smaller than the full graph",
        serde_json::to_string(sw).unwrap().len() < serde_json::to_string(&one["workflows"][0]).unwrap().len(),
    );
    let wf = &one["workflows"][0];
    h.eq("bdd_list: cat", wf["cat"].clone(), json!("guards"));
    h.eq("bdd_list: start", wf["start"].clone(), json!("unauthenticated"));
    h.eq("bdd_list: node_count", wf["node_count"].as_u64().unwrap(), 5);
    let inputs = wf["inputs"].as_array().unwrap();
    h.check(
        "bdd_list: inputs carry the ev_ state fields",
        inputs.iter().any(|v| v == "ev_risk") && inputs.iter().any(|v| v == "ev_unauthenticated"),
    );
    let qids: Vec<String> = wf["questions"]
        .as_array()
        .unwrap()
        .iter()
        .filter_map(|q| q["id"].as_str().map(str::to_string))
        .collect();
    h.check(
        "bdd_list: questions include event + risk",
        qids.contains(&"event".to_string()) && qids.contains(&"risk".to_string()),
    );
    // `outputs` is the closed decision vocabulary: a threshold ladder's labels or
    // a gate node's verbs, per node and unioned at the top. For a single-node graph
    // it is the exact output set.
    let labels = |j: &Value| -> Vec<String> {
        j["outputs"]
            .as_array()
            .unwrap()
            .iter()
            .filter_map(|v| v.as_str().map(str::to_string))
            .collect()
    };
    let apg = t.call(&json!({"name": "agent_permission_gate"})).unwrap();
    h.check(
        "bdd_list: outputs carry the agent_permission_gate label set",
        labels(&apg["workflows"][0]) == vec!["BLOCK", "ESCALATE", "EXECUTE"],
    );
    let acg = t.call(&json!({"name": "agent_command_gate"})).unwrap();
    h.check(
        "bdd_list: outputs keep the agent_command_gate ladder order",
        labels(&acg["workflows"][0]) == vec!["BLOCK", "CONFIRM", "ALLOW"],
    );
    // P105: a node's question contract marks the question its routing is keyed by
    // (`primary_q`) and, on that question, maps each answer to the node it routes
    // to (`edge.condition`, else `edge.default`) — so a step-wise caller plans the
    // next hop from one reply instead of cross-referencing the node's `routes`.
    let acg_node = &acg["workflows"][0]["nodes"][0];
    let aq = acg_node["questions"].as_array().unwrap();
    h.check(
        "bdd_list: exactly one question in a node is primary",
        aq.iter().filter(|q| q["primary"] == json!(true)).count() == 1,
    );
    let prim = aq.iter().find(|q| q["primary"] == json!(true)).unwrap();
    h.eq("bdd_list: the primary question is the node's primary_q", prim["id"].clone(), json!("is_destructive"));
    h.check(
        "bdd_list: the other questions are marked non-primary",
        aq.iter().all(|q| (q["id"] == "is_destructive") == (q["primary"] == json!(true))),
    );
    h.check(
        "bdd_list: a non-primary question carries no satisfied_by",
        aq.iter()
            .filter(|q| q["id"] != "is_destructive")
            .all(|q| !q.as_object().unwrap().contains_key("satisfied_by")),
    );
    // agent_command_gate's primary question has an empty `edge.condition`, so every
    // answer routes to the node's `default` (STOP).
    h.check(
        "bdd_list: satisfied_by routes each answer through edge.condition else default",
        prim["satisfied_by"]["A"] == json!("STOP") && prim["satisfied_by"]["B"] == json!("STOP"),
    );
    // A routing node: `edge.condition` splits the primary answers across the graph.
    let str_wf = t.call(&json!({"name": "support_ticket_router"})).unwrap();
    let cq = str_wf["workflows"][0]["nodes"][0]["questions"]
        .as_array()
        .unwrap()
        .iter()
        .find(|q| q["id"] == "category")
        .cloned()
        .unwrap();
    h.check(
        "bdd_list: satisfied_by reads edge.condition per answer, else the default",
        cq["primary"] == json!(true)
            && cq["satisfied_by"]["technical"] == json!("severity_probe")
            && cq["satisfied_by"]["billing"] == json!("severity_probe")
            && cq["satisfied_by"]["sales"] == json!("STOP")
            && cq["satisfied_by"]["other"] == json!("STOP"),
    );
    // A dynamic (`criteria.from_state`) primary question has no literal answers, but
    // its `none` key still routes — satisfied_by carries that edge, so the fail-closed
    // branch is legible from the contract.
    let ss = t.call(&json!({"name": "span_selection"})).unwrap();
    let sq = ss["workflows"][0]["nodes"]
        .as_array()
        .unwrap()
        .iter()
        .find(|n| n["name"] == "select")
        .and_then(|n| n["questions"].as_array())
        .and_then(|qs| qs.iter().find(|q| q["id"] == "billing_contact"))
        .cloned()
        .unwrap();
    h.check(
        "bdd_list: a dynamic primary question maps its edge.condition key (none) to the destination",
        sq["primary"] == json!(true)
            && sq["answers"].as_array().unwrap().is_empty()
            && sq["satisfied_by"]["none"] == json!("not_found"),
    );
    // A single-node graph's top-level outputs equal its only node's outputs.
    let et = t.call(&json!({"name": "email_triage"})).unwrap();
    let etw = &et["workflows"][0];
    let et_top = labels(etw);
    let et_node = labels(&etw["nodes"][0]);
    h.check(
        "bdd_list: a single-node graph's outputs equal its node's outputs",
        et_top == et_node
            && et_top
                == vec![
                    "QUARANTINE",
                    "TRASH",
                    "PAGER_BILLING",
                    "PAGER_ONCALL",
                    "REPLY_TODAY",
                    "REPLY_QUEUE",
                    "FYI_ONLY",
                ],
    );
    // A gate node contributes its three verbs, not a threshold ladder.
    let ttm = t.call(&json!({"name": "triage_then_moderate"})).unwrap();
    let ttmw = &ttm["workflows"][0];
    let gate_node = ttmw["nodes"]
        .as_array()
        .unwrap()
        .iter()
        .find(|n| n["name"] == "safety_gate")
        .unwrap();
    h.check(
        "bdd_list: a gate node's outputs are BLOCK/ALLOW/CONFIRM",
        labels(gate_node) == vec!["BLOCK", "ALLOW", "CONFIRM"],
    );
    // Structural invariant across the whole corpus: the top-level `outputs` is
    // exactly the deduped union of every node's outputs.
    let mut union_ok = true;
    for w in all["workflows"].as_array().unwrap() {
        let mut uni: Vec<String> = Vec::new();
        for n in w["nodes"].as_array().unwrap() {
            for o in labels(n) {
                if !uni.contains(&o) {
                    uni.push(o);
                }
            }
        }
        if labels(w) != uni {
            union_ok = false;
        }
    }
    h.check(
        "bdd_list: top-level outputs == deduped union of node outputs (whole corpus)",
        union_ok,
    );

    // `inputs` counts every form of state read, not just `answer_field`: the
    // heuristic `field` (text to match) and `${state.*}` interpolations too.
    let sr_in = t.call(&json!({"name": "support_routing"})).unwrap();
    h.check(
        "bdd_list: inputs count a heuristic `field` (text source)",
        sr_in["workflows"][0]["inputs"].clone() == json!(["message"]),
    );
    let gr = t.call(&json!({"name": "goal_runner"})).unwrap();
    h.check(
        "bdd_list: inputs count ${state.*} interpolations",
        gr["workflows"][0]["inputs"].clone() == json!(["goal", "workdir"]),
    );
    // The boundary the other way: a graph that genuinely reads no state reports
    // an empty inputs, so `inputs == []` still means "the offline engine is blind
    // to the state here".
    let cpg = t.call(&json!({"name": "content_policy_gate"})).unwrap();
    h.check(
        "bdd_list: a state-free graph reports empty inputs",
        cpg["workflows"][0]["inputs"].as_array().unwrap().is_empty(),
    );

    // A `combine` node's decision is its `route`: the choice's criteria minus the
    // fallback, plus the review label. A `rubric` node's is its `status`.
    let sr = t.call(&json!({"name": "support_routing"})).unwrap();
    h.check(
        "bdd_list: a combine node's outputs are its route values",
        labels(&sr["workflows"][0]) == vec!["billing", "technical", "account", "human_review"],
    );
    let qr = t.call(&json!({"name": "quality_rubric"})).unwrap();
    h.check(
        "bdd_list: a rubric node's outputs are scored/human_review",
        labels(&qr["workflows"][0]) == vec!["scored", "human_review"],
    );

    // The scan is recursive (same walk as `spec::discover`), so a nested category
    // is both discoverable here and resolvable by `bdd_decide {name}`; `cat` is the
    // spec's directory relative to the dsl root. Proven on a throwaway tree.
    {
        let base = std::env::temp_dir().join(format!("laya-scan-probe-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&base);
        let dsl = base.join("dsl");
        for rel in ["routing/a.json", "routing/sub/b.json", "apps/c.json", "apps/skip.config.json"] {
            let f = dsl.join(rel);
            std::fs::create_dir_all(f.parent().unwrap()).unwrap();
            std::fs::write(&f, "{}").unwrap();
        }
        let found = laya_workflow::bdd_mcp::scan_dsl_specs(&dsl);
        let pairs: Vec<(String, String)> = found
            .iter()
            .map(|(c, p)| (c.clone(), p.file_name().unwrap().to_string_lossy().into_owned()))
            .collect();
        h.check(
            "bdd_list: recursive scan keeps nested cats as relative paths",
            pairs
                == vec![
                    ("apps".to_string(), "c.json".to_string()),
                    ("routing".to_string(), "a.json".to_string()),
                    ("routing/sub".to_string(), "b.json".to_string()),
                ],
        );
        h.check("bdd_list: recursive scan skips *.config.json sidecars", found.len() == 3);
        let _ = std::fs::remove_dir_all(&base);
    }

    // bdd_decide resolves the same port name through the corpus.
    let d = bdd_tool("bdd_decide")
        .call(&json!({"name": "agent_permission_gate", "state": {"ev_unauthenticated": "halt", "ev_risk": "high"}}))
        .unwrap();
    h.eq("bdd_decide(name): label BLOCK", d["label"].clone(), json!("BLOCK"));
    // An unknown port name errors cleanly instead of panicking.
    h.check(
        "bdd_decide(name): unknown name errors",
        bdd_tool("bdd_decide")
            .call(&json!({"name": "no_such_workflow_xyz", "state": {}}))
            .is_err(),
    );
}

/// P82: the built-in Rust apps and their BDD ports must decide the same label.
///
/// `src/apps.rs` carries built-in apps whose `route()` is native Rust; two of
/// them already have a BDD port in `dsl/guards/` (`agent_gate` →
/// `agent_command_gate`, `content_moderation` → `content_policy_gate`). A port
/// is only worth having if it decides what the app decides, so this drives the
/// port's compiled threshold action and the app's `route()` on the same offline
/// verdict and asserts they agree over a grid of model outputs.
///
/// Where exact equality is impossible it pins the *documented* fold instead of
/// hiding it: `agent_gate` also hard-blocks on the command *verb* (a pre-model
/// guard the decision vocabulary has no input slot for), so the port folds the
/// exfil-verb check into the exfiltration probability. The fold is asserted to
/// be fail-closed — the port never blocks less than the app, never allows what
/// the app blocks, and every extra block sits in the folded `exfil >= 0.6` band.
pub fn test_bdd_app_port_parity(h: &mut Harness) {
    use laya_workflow::backend::HeuristicBackend;
    use laya_workflow::workflow::{Decision, Verdict};
    let root = bdd::repo_root();
    let load = |p: &str| -> Value {
        serde_json::from_str(&std::fs::read_to_string(root.join(p)).unwrap()).unwrap()
    };
    let label_of = |spec: &Value, v: &Verdict| -> String {
        let node = &spec["nodes"][0];
        spec::run_action(
            &node["action"].clone(),
            &json!({}),
            v,
            node["primary_q"].as_str(),
        )
        .unwrap()["label"]
            .as_str()
            .unwrap()
            .to_string()
    };
    let bchoice = |b: f64| -> Map<String, Value> {
        let mut m = Map::new();
        m.insert("A".into(), json!(1.0 - b));
        m.insert("B".into(), json!(b));
        m
    };
    let dec = |answer: Value, probs: Map<String, Value>| Decision {
        answer,
        probabilities: probs,
        confidence: 0.9,
    };

    // ── content_moderation ↔ content_policy_gate: exact parity ──
    let cm = load("dsl/guards/content_policy_gate.json");
    let cats = ["safe", "hateful", "threatening", "leaky_pii", "spam", "off_topic"];
    let (mut cm_cases, mut cm_bad) = (0usize, 0usize);
    for &p_threat in &[0.0, 0.65, 0.75] {
        for &p_hate in &[0.0, 0.65, 0.75] {
            for &p_pii in &[0.0, 0.55, 0.65] {
                for &p_spam in &[0.0, 0.65, 0.75] {
                    for &sev in &[0.5, 1.5, 2.0] {
                        for cat in cats {
                            let mut answers = std::collections::HashMap::new();
                            answers.insert("is_threat".into(), dec(json!("B"), bchoice(p_threat)));
                            answers.insert("is_hate".into(), dec(json!("B"), bchoice(p_hate)));
                            answers.insert("is_pii".into(), dec(json!("B"), bchoice(p_pii)));
                            answers.insert("is_spam".into(), dec(json!("B"), bchoice(p_spam)));
                            answers.insert("severity".into(), dec(json!(sev), Map::new()));
                            answers.insert("category".into(), dec(json!(cat), Map::new()));
                            let v = Verdict { answers, input_tokens: 0, latency_ms: 0.0 };
                            let app = apps::content_moderation::route(&v)["action"]
                                .as_str()
                                .unwrap()
                                .to_string();
                            let port = label_of(&cm, &v);
                            cm_cases += 1;
                            if app != port {
                                cm_bad += 1;
                            }
                        }
                    }
                }
            }
        }
    }
    h.check(
        &format!("bdd: content_policy_gate port == app on the grid ({cm_cases} verdicts)"),
        cm_bad == 0,
    );

    // ── agent_gate ↔ agent_command_gate: model-signal parity + the documented
    //    exfil-verb fold, and fail-closedness ──
    let ag = load("dsl/guards/agent_command_gate.json");
    let (mut ag_cases, mut ag_under, mut ag_open, mut ag_fold_off) = (0usize, 0usize, 0usize, 0usize);
    for &p_destr in &[0.0, 0.3, 0.45, 0.65, 0.85] {
        for &p_exfil in &[0.0, 0.3, 0.45, 0.65, 0.9] {
            for &p_intent_a in &[0.0, 0.4, 0.6, 0.9] {
                for &risk in &[0.0, 1.0, 2.0] {
                    let mut answers = std::collections::HashMap::new();
                    answers.insert("is_destructive".into(), dec(json!("B"), bchoice(p_destr)));
                    answers.insert("is_exfiltration".into(), dec(json!("B"), bchoice(p_exfil)));
                    answers.insert(
                        "intent_match".into(),
                        dec(json!("A"), bchoice(1.0 - p_intent_a)),
                    );
                    answers.insert("risk".into(), dec(json!(risk), Map::new()));
                    let v = Verdict { answers, input_tokens: 0, latency_ms: 0.0 };
                    // a verb that is neither a destructive verb nor an exfil
                    // tool, so the app's pre-model guards never fire.
                    let app = apps::agent_gate::route("echo hi", &v)["action"]
                        .as_str()
                        .unwrap()
                        .to_string();
                    let port = label_of(&ag, &v);
                    ag_cases += 1;
                    if app == "BLOCK" && port != "BLOCK" {
                        ag_under += 1;
                    }
                    if port == "ALLOW" && app != "ALLOW" {
                        ag_open += 1;
                    }
                    if port == "BLOCK" && app != "BLOCK" && p_exfil < 0.6 {
                        ag_fold_off += 1;
                    }
                }
            }
        }
    }
    h.check(
        &format!("bdd: agent_command_gate never under-blocks the app ({ag_cases} verdicts)"),
        ag_under == 0,
    );
    h.check(
        "bdd: agent_command_gate never fails open (ALLOW only where the app allows)",
        ag_open == 0,
    );
    h.check(
        "bdd: agent_command_gate's extra blocks are exactly the folded exfil>=0.6 band",
        ag_fold_off == 0,
    );

    // The apps' own reference cases, driven by the *offline heuristic* the
    // gates use, so the parity holds end to end and not only on hand-built
    // verdicts.
    let hb = HeuristicBackend;
    let aq = apps::agent_gate::questions();
    let mut ref_bad = 0usize;
    for (cmd, intent, _want) in apps::agent_gate::cases() {
        let state = json!({"command": cmd, "intent": intent, "cwd": "."});
        let v = hb.decide(&state, &aq).unwrap();
        let app = apps::agent_gate::route(cmd, &v)["action"].as_str().unwrap().to_string();
        if app != label_of(&ag, &v) {
            ref_bad += 1;
        }
    }
    h.check(
        "bdd: agent_command_gate matches the app on its 8 reference cases",
        ref_bad == 0,
    );

    let cq = apps::content_moderation::questions();
    let mut cref_bad = 0usize;
    for (text, _want) in apps::content_moderation::cases() {
        let state = json!({"source": "bench", "text": text});
        let v = hb.decide(&state, &cq).unwrap();
        let app = apps::content_moderation::route(&v)["action"]
            .as_str()
            .unwrap()
            .to_string();
        if app != label_of(&cm, &v) {
            cref_bad += 1;
        }
    }
    h.check(
        "bdd: content_policy_gate matches the app on its 6 reference cases",
        cref_bad == 0,
    );

    // ── email_triage ↔ email_triage port: exact parity, including the two
    //    `category AND urgency` conjunction branches ──
    let et = load("dsl/apps/email_triage.json");
    let ecats = ["billing", "technical", "sales", "account", "hr", "other"];
    let (mut et_cases, mut et_bad) = (0usize, 0usize);
    for cat in ecats {
        for &p_spam in &[0.0, 0.5, 0.8] {
            for &p_phish in &[0.0, 0.5, 0.8] {
                for &urg in &[0.0, 1.0, 1.5, 2.0] {
                    for &p_reply in &[0.0, 0.5, 0.8] {
                        let mut answers = std::collections::HashMap::new();
                        answers.insert("category".into(), dec(json!(cat), Map::new()));
                        answers.insert("spam".into(), dec(json!("B"), bchoice(p_spam)));
                        answers.insert("phishing".into(), dec(json!("B"), bchoice(p_phish)));
                        answers.insert("urgency".into(), dec(json!(urg), Map::new()));
                        answers.insert(
                            "needs_reply".into(),
                            dec(json!("B"), bchoice(p_reply)),
                        );
                        let v = Verdict { answers, input_tokens: 0, latency_ms: 0.0 };
                        let app = apps::email_triage::route(&v)["action"]
                            .as_str()
                            .unwrap()
                            .to_string();
                        let port = label_of(&et, &v);
                        et_cases += 1;
                        if app != port {
                            et_bad += 1;
                        }
                    }
                }
            }
        }
    }
    h.check(
        &format!("bdd: email_triage port == app on the grid ({et_cases} verdicts)"),
        et_bad == 0,
    );

    // ── draft_scorer ↔ draft_scorer port: exact parity, including the
    //    `typo AND low clarity` / `sensitive AND hostile tone` conjunctions and
    //    the fall-through to the author's own `send_now` answer ──
    let ds = load("dsl/apps/draft_scorer.json");
    let (mut ds_cases, mut ds_bad) = (0usize, 0usize);
    for &p_typo in &[0.0, 0.5, 0.8] {
        for &p_sens in &[0.0, 0.5, 0.8] {
            for &clarity in &[0.0, 1.0, 1.5, 3.0] {
                for &tone in &[0.0, 1.0, 2.0] {
                    for key in ["send_now", "polish", "sleep", "rewrite"] {
                        let mut answers = std::collections::HashMap::new();
                        answers.insert("has_typo".into(), dec(json!("B"), bchoice(p_typo)));
                        answers.insert("is_sensitive".into(), dec(json!("B"), bchoice(p_sens)));
                        answers.insert("clarity".into(), dec(json!(clarity), Map::new()));
                        answers.insert("tone".into(), dec(json!(tone), Map::new()));
                        answers.insert("send_now".into(), dec(json!(key), Map::new()));
                        let v = Verdict { answers, input_tokens: 0, latency_ms: 0.0 };
                        let app = apps::draft_scorer::route(&v)["suggestion"]
                            .as_str()
                            .unwrap()
                            .to_string();
                        let port = label_of(&ds, &v);
                        ds_cases += 1;
                        if app != port {
                            ds_bad += 1;
                        }
                    }
                }
            }
        }
    }
    h.check(
        &format!("bdd: draft_scorer port == app on the grid ({ds_cases} verdicts)"),
        ds_bad == 0,
    );

    // The two apps' own reference cases, driven by the offline heuristic, so the
    // parity holds end to end and not only on hand-built verdicts.
    let eq = apps::email_triage::questions();
    let mut eref_bad = 0usize;
    for (subject, body, from, _want) in apps::email_triage::cases() {
        let state = json!({"subject": subject, "body": body, "from": from});
        let v = hb.decide(&state, &eq).unwrap();
        let app = apps::email_triage::route(&v)["action"]
            .as_str()
            .unwrap()
            .to_string();
        if app != label_of(&et, &v) {
            eref_bad += 1;
        }
    }
    h.check(
        "bdd: email_triage port matches the app on its 5 reference emails",
        eref_bad == 0,
    );

    let dq = apps::draft_scorer::questions();
    let mut dref_bad = 0usize;
    for (text, audience, _want) in apps::draft_scorer::cases() {
        let state = json!({"audience": audience, "text": text});
        let v = hb.decide(&state, &dq).unwrap();
        let app = apps::draft_scorer::route(&v)["suggestion"]
            .as_str()
            .unwrap()
            .to_string();
        if app != label_of(&ds, &v) {
            dref_bad += 1;
        }
    }
    h.check(
        "bdd: draft_scorer port matches the app on its 6 reference drafts",
        dref_bad == 0,
    );

    // ── gateway (composed) ↔ gateway_triage port: parity on the gate half
    //    *and* the composed triage half, driven by the offline heuristic end
    //    to end. This is the composition port: the built-in `gateway_workflow`
    //    gates a tool call and, when it is safe, runs the email-triage chain in
    //    the same graph; the spec reproduces both the gate's BLOCK (unsafe) and
    //    the triage label the merged state carries (safe). The app writes the
    //    verdict to `state.action`, the spec to `state.label`; compare those.
    let gw_spec = spec::load_file(
        &root
            .join("dsl/apps/gateway_triage.json")
            .to_string_lossy(),
    )
    .unwrap();
    let gw_states = [
        (
            "unsafe",
            json!({"command": "sudo rm -rf /", "intent": "free up disk space", "cwd": "/"}),
        ),
        (
            "safe_billing",
            json!({
                "command": "ls -la /tmp",
                "intent": "list files",
                "cwd": "/tmp",
                "subject": "Re: invoice duplicate charge",
                "body": "Hi, we were billed twice for March. Please refund today or we cancel.",
                "from": "user@acme.com"
            }),
        ),
    ];
    let (mut gw_cases, mut gw_bad) = (0usize, 0usize);
    for (_name, state) in &gw_states {
        let app = apps::gateway_workflow().run(&hb, state).unwrap();
        let port = gw_spec.run(&hb, state).unwrap();
        let app_label = app.state["action"].as_str().unwrap_or("?");
        let port_label = port.state["label"].as_str().unwrap_or("?");
        gw_cases += 1;
        if app_label != port_label {
            gw_bad += 1;
        }
    }
    h.check(
        &format!(
            "bdd: gateway_triage port == gateway app on the gate + composed triage states ({gw_cases} states)"
        ),
        gw_bad == 0,
    );
}

/// P90: `bdd_vocabulary` (MCP) and `bdd vocabulary` (CLI) expose the decision
/// authoring grammar - the curated step templates behind the compiler's
/// `did you mean` hints - so an agent can read the vocabulary *before* writing
/// a step instead of only discovering it from a red build. This pins the two
/// surfaces to the single in-code source (`bdd::decision_templates`) and to
/// each other, so the listing cannot drift from the suggestion index.
pub fn test_bdd_vocabulary(h: &mut Harness) {
    let templates = bdd::decision_templates();
    let want_kinds: Vec<String> = bdd::VOCABULARY_KINDS.iter().map(|s| s.to_string()).collect();
    let want_tmpls: Vec<String> = templates.iter().map(|(_, t)| t.to_string()).collect();

    // ── MCP `bdd_vocabulary` ──────────────────────────────────────────
    let t = bdd_tool("bdd_vocabulary");
    let v = t.call(&json!({})).unwrap();
    h.eq(
        "bdd_vocabulary: count == the in-code template list",
        v["count"].as_u64().unwrap() as usize,
        templates.len(),
    );
    h.check(
        "bdd_vocabulary: no accidental shrink (>= 38 templates)",
        templates.len() >= 38,
    );
    let kinds: Vec<String> = v["kinds"]
        .as_array()
        .unwrap()
        .iter()
        .filter_map(|k| k.as_str().map(str::to_string))
        .collect();
    h.eq("bdd_vocabulary: kinds are given/when/then", kinds, want_kinds);
    let steps = v["steps"].as_array().unwrap();
    h.check(
        "bdd_vocabulary: every step names a known kind and a non-empty template",
        steps.iter().all(|s| {
            let k = s["kind"].as_str().unwrap_or("");
            let tm = s["template"].as_str().unwrap_or("");
            !tm.is_empty() && bdd::VOCABULARY_KINDS.contains(&k)
        }),
    );
    let got_tmpls: Vec<String> = steps
        .iter()
        .filter_map(|s| s["template"].as_str().map(str::to_string))
        .collect();
    h.eq(
        "bdd_vocabulary: steps == the in-code templates, in order",
        got_tmpls.clone(),
        want_tmpls.clone(),
    );
    let mut uniq = std::collections::BTreeSet::new();
    h.check(
        "bdd_vocabulary: templates are unique",
        got_tmpls.iter().all(|s| uniq.insert(s.clone())),
    );
    // by_kind is the same list, partitioned; its size must sum to count.
    let bk = v["by_kind"].as_object().unwrap();
    let bk_total: usize = bk
        .values()
        .map(|a| a.as_array().map(|x| x.len()).unwrap_or(0))
        .sum();
    h.eq("bdd_vocabulary: by_kind partitions the templates", bk_total, templates.len());
    h.check(
        "bdd_vocabulary: a known template is present",
        got_tmpls.iter().any(|s| s.contains("call the capability"))
            && got_tmpls.iter().any(|s| s.contains("the policy")),
    );
    // P107: table dialects (same step, special table rows) an author cannot guess
    // from the one-line template — the dynamic-candidate grammar in particular —
    // are exposed so the grammar is discoverable before it is written.
    let dialects = v["dialects"].as_array().cloned().unwrap_or_default();
    h.check(
        "bdd_vocabulary: exposes the dynamic-criteria table dialect",
        dialects.iter().any(|d| {
            d["step"].as_str().map(|s| s.contains("a choice question")).unwrap_or(false)
                && d["table"]
                    .as_array()
                    .map(|rows| rows.iter().any(|r| r.as_str() == Some("| from_state | <state array key, e.g. candidates> |")))
                    .unwrap_or(false)
                && d["table"]
                    .as_array()
                    .map(|rows| rows.iter().any(|r| r.as_str().map(|s| s.starts_with("| none |")).unwrap_or(false)))
                    .unwrap_or(false)
        }),
    );

    // P157: the `call the capability` table dialect (with./project./chain.) is a
    // central agentic shape - a parameterised call, a state projection and a
    // chained sub-call - but it was only in prose, not in the machine-readable
    // grammar. It is now advertised next to the dynamic-criteria one, and this
    // proves the advertised rows really compile (a dialect nobody can use is a lie).
    h.check(
        "bdd_vocabulary: exposes the call capability table dialect",
        dialects.iter().any(|d| {
            d["step"].as_str().map(|s| s == "call the capability \"<cap>\"").unwrap_or(false)
                && d["table"]
                    .as_array()
                    .map(|rows| rows.iter().any(|r| r.as_str() == Some("| with.<key> | <value> |")))
                    .unwrap_or(false)
                && d["table"]
                    .as_array()
                    .map(|rows| rows.iter().any(|r| r.as_str().map(|s| s.starts_with("| chain.")).unwrap_or(false)))
                    .unwrap_or(false)
        }),
    );
    {
        let call_src = r#"
@decision
Feature: call_dialect_probe
  Background:
    Given the capabilities
      | digest | {"kind": "text", "op": "length"} |
  Scenario: root
    Given a choice question "go" asks "go?"
      | answer | meaning |
      | yes | y |
      | no  | n |
    When the answer "yes" goes to "STOP"
    And any other answer goes to "STOP"
    Then call the capability "digest"
      | with.text | ${state.text} |
      | project.count | /count |
      | chain.again.capability | digest |
      | chain.again.with.text | ${state.text} |
      | chain.again.project.n | /count |
    Then allow up to 1 retries
"#;
        let feat = bdd::parse(call_src, "<call-dialect>").expect("call-dialect feature parses");
        let spec = bdd::compile_decision(&feat, &json!({})).expect("call-dialect feature compiles");
        let act = &spec["nodes"][0]["action"];
        h.eq("bdd_vocabulary: the advertised call dialect compiles (project)", act["project"]["count"].clone(), json!("/count"));
        h.eq(
            "bdd_vocabulary: the advertised call dialect compiles (chain)",
            act["chain"][0]["capability"].clone(),
            json!("digest"),
        );
        h.check(
            "bdd_vocabulary: the advertised call dialect validates",
            crate::spec::from_spec(&spec).is_ok(),
        );
    }

    // ── CLI `bdd vocabulary --json` and the human form ────────────────
    let root = bdd::repo_root();
    let mut bin = std::env::current_exe().expect("test binary path");
    bin.pop();
    if bin.ends_with("deps") {
        bin.pop();
    }
    bin.push("laya-workflow");
    let cli_json = std::process::Command::new(&bin)
        .args(["bdd", "vocabulary", "--json"])
        .current_dir(&root)
        .output()
        .expect("run bdd vocabulary --json");
    let cj: Value = serde_json::from_slice(&cli_json.stdout).expect("--json is valid JSON");
    h.eq(
        "bdd vocabulary --json: count == the in-code template list",
        cj["count"].as_u64().unwrap() as usize,
        templates.len(),
    );
    let cj_tmpls: Vec<String> = cj["steps"]
        .as_array()
        .unwrap()
        .iter()
        .filter_map(|s| s["template"].as_str().map(str::to_string))
        .collect();
    h.eq("bdd vocabulary --json == bdd_vocabulary", cj_tmpls, want_tmpls.clone());
    h.eq(
        "bdd vocabulary --json: dialects == bdd_vocabulary's dialects",
        cj["dialects"].clone(),
        v["dialects"].clone(),
    );

    let cli_human = std::process::Command::new(&bin)
        .args(["bdd", "vocabulary"])
        .current_dir(&root)
        .output()
        .expect("run bdd vocabulary");
    let text = String::from_utf8_lossy(&cli_human.stdout);
    h.check(
        "bdd vocabulary: human form prints the header, the kinds and a template",
        text.contains("decision vocabulary:")
            && text.contains("given")
            && text.contains("call the capability"),
    );
    h.check(
        "bdd vocabulary: human form names the dynamic table dialect",
        text.contains("table dialects")
            && text.contains("from_state")
            && text.contains("id_key"),
    );
    h.check(
        "bdd vocabulary: human form names the call-capability table dialect",
        text.contains("with.<key>") && text.contains("chain.<as>.capability"),
    );
}

/// P158: a Scenario carries at most one action.
///
/// Every action-carrying `Then` step compiles to a different action kind, and
/// the assembly in `compile_decision_node` is a priority chain, so a second
/// action used to be dropped in silence: `bdd build` on a feature with both a
/// `Then label …` and a `Then call the capability …` exited 0 with the `call`
/// simply absent from the emitted spec. Refuse the mix instead, by name, and
/// refuse a repeated singleton action (a second `gate` / `call` / … overwrites
/// the first). Several `label …` rules are one threshold action and stay legal.
pub fn test_bdd_single_action(h: &mut Harness) {
    // The failing shape: a `call the capability` sharing a Scenario with a
    // `label` threshold. Both are actions; only the threshold used to survive.
    let mixed = r#"
@decision
Feature: two_actions
  Background:
    Given the capabilities
      | digest | {"kind": "text", "op": "length"} |
  Scenario: root
    Given a choice question "go" asks "go?"
      | answer | meaning |
      | yes | y |
      | no  | n |
    When the answer "yes" goes to "STOP"
    And any other answer goes to "STOP"
    Then call the capability "digest"
      | with.text | ${state.text} |
    Then label "done" otherwise
    Then allow up to 1 retries
"#;
    let feat = bdd::parse(mixed, "<mixed>").expect("mixed feature parses");
    let err = match bdd::compile_decision(&feat, &json!({})) {
        Ok(_) => String::new(),
        Err(e) => format!("{e:#}"),
    };
    h.check(
        "bdd single action: refuses a scenario that mixes two actions",
        err.contains("carries at most one action")
            && err.contains("call")
            && err.contains("threshold"),
    );

    // A repeated singleton action is the same loss in a different guise: the
    // second `call` overwrites the first. Refuse it too.
    let twice = r#"
@decision
Feature: two_calls
  Background:
    Given the capabilities
      | digest | {"kind": "text", "op": "length"} |
  Scenario: root
    Given a choice question "go" asks "go?"
      | answer | meaning |
      | yes | y |
      | no  | n |
    When the answer "yes" goes to "STOP"
    And any other answer goes to "STOP"
    Then call the capability "digest"
      | with.text | ${state.text} |
    Then call the capability "digest"
      | with.text | ${state.text} |
"#;
    let feat = bdd::parse(twice, "<twice>").expect("two-call feature parses");
    let err = match bdd::compile_decision(&feat, &json!({})) {
        Ok(_) => String::new(),
        Err(e) => format!("{e:#}"),
    };
    h.check(
        "bdd single action: refuses a repeated call step",
        err.contains("second `call` action"),
    );

    // Several rules of the *same* kind are one threshold action - they must
    // still compile, or every real corpus feature would break.
    let same = r#"
@decision
Feature: one_action
  Scenario: root
    Given a choice question "intent" asks "Which intent?"
      | answer  | meaning             |
      | ok      | normal              |
      | stuffing | credential stuffing |
    When any other answer goes to "STOP"
    Then label "DENY" when the answer to "intent" is one of "stuffing"
    And label "ALLOW" otherwise
"#;
    let feat = bdd::parse(same, "<same>").expect("single-action feature parses");
    h.check(
        "bdd single action: several rules of one kind are still one action",
        bdd::compile_decision(&feat, &json!({})).is_ok(),
    );
}

/// P161: a step table is one JSON object, so a repeated row key is a silent drop.
///
/// Every decision-vocabulary table that carries `| key | value |` rows compiles
/// into a single JSON object - a choice question's `criteria`, a `call` step's
/// `with` / `project` / `chain`, the `policy`, the `capabilities`, a
/// `match_rules` heuristic's label map. A row that repeats an earlier row's key
/// overwrites it, so the declaration the author wrote first is thrown away while
/// `bdd build` still exits 0. That is P128's "a declared option nobody reads" one
/// level up, in the authoring layer, so the compiler now refuses it and names
/// both lines. The `score` level table is the deliberate exception: its rows are
/// a *list*, where a repeat is kept rather than dropped.
///
/// The keys are checked exactly as the compiler reads them, so `with.text` and
/// `project.text` (two maps, both real in the corpus) stay legal, and so does a
/// `with.text` beside a `chain.<as>.with.text`.
pub fn test_bdd_table_key_once(h: &mut Harness) {
    // Compile a `@decision` feature and return its error text ("" when it compiles).
    let err_of = |text: &str| -> String {
        match bdd::parse(text, "<dup>") {
            Ok(f) => match bdd::compile_decision(&f, &json!({})) {
                Ok(_) => String::new(),
                Err(e) => format!("{e:#}"),
            },
            Err(e) => format!("{e:#}"),
        }
    };
    // The 1-based source line of the `nth` line that contains `needle`.
    let line_of = |text: &str, needle: &str, nth: usize| -> usize {
        text.lines()
            .enumerate()
            .filter(|(_, l)| l.contains(needle))
            .nth(nth)
            .map(|(i, _)| i + 1)
            .unwrap_or(0)
    };

    // A `call` step table: two `with.text` rows compile into one `with` object.
    let call = r#"
@decision
Feature: dup_call
Background:
  Given the capabilities
    | digest | {"kind": "text", "op": "length"} |
Scenario: root
  Given a choice question "go" asks "go?"
    | answer | meaning |
    | yes | y |
    | no  | n |
  When any other answer goes to "STOP"
  Then call the capability "digest"
    | with.text | ${state.text} |
    | with.text | ${state.again} |
"#;
    let err = err_of(call);
    let first = line_of(call, "| with.text |", 0);
    let second = line_of(call, "| with.text |", 1);
    h.check(
        "bdd table key once: refuses a call step table that repeats with.text",
        err.contains(&format!("line {second}:"))
            && err.contains("call step table")
            && err.contains("declares \"with.text\" twice")
            && err.contains(&format!("the first is at line {first}")),
    );

    // A choice question: two rows with the same answer compile into one criteria.
    let choice = r#"
@decision
Feature: dup_choice
Scenario: root
  Given a choice question "go" asks "go?"
    | answer | meaning |
    | yes | y |
    | yes | again |
  When any other answer goes to "STOP"
"#;
    let err = err_of(choice);
    let first = line_of(choice, "| yes |", 0);
    let second = line_of(choice, "| yes |", 1);
    h.check(
        "bdd table key once: refuses a choice question that repeats an answer row",
        err.contains(&format!("line {second}:"))
            && err.contains("choice question \"go\" table")
            && err.contains("declares \"yes\" twice")
            && err.contains(&format!("the first is at line {first}")),
    );

    // A `match_rules` heuristic: two rows with the same label compile into one
    // rules object.
    let rules = r#"
@decision
Feature: dup_rules
Scenario: root
  Given a choice question "intent" asks "Which intent?"
    | answer | meaning |
    | ok     | normal  |
    | reject | fraud   |
  Given answer by rules on the field "text" with hit 0.9 and fallback "ok"
    | label  | needles    |
    | reject | "fraud"    |
    | reject | "scripted" |
  When any other answer goes to "STOP"
  Then label "REJECTED" when the answer is "reject"
  And label "OK" otherwise
"#;
    let err = err_of(rules);
    let first = line_of(rules, "\"fraud\"", 0);
    let second = line_of(rules, "\"scripted\"", 0);
    h.check(
        "bdd table key once: refuses a match_rules table that repeats a label",
        err.contains(&format!("line {second}:"))
            && err.contains("match_rules step")
            && err.contains("declares \"reject\" twice")
            && err.contains(&format!("the first is at line {first}")),
    );

    // The policy table: a repeated key compiles into one policy object.
    let policy = r#"
@decision
Feature: dup_policy
Background:
  Given the policy
    | allow_exec | true  |
    | allow_exec | false |
Scenario: root
  Given a choice question "go" asks "go?"
    | answer | meaning |
    | yes | y |
    | no  | n |
  When any other answer goes to "STOP"
"#;
    let err = err_of(policy);
    let first = line_of(policy, "| allow_exec | true", 0);
    let second = line_of(policy, "| allow_exec | false", 0);
    h.check(
        "bdd table key once: refuses a policy table that repeats a key",
        err.contains(&format!("line {second}:"))
            && err.contains("policy table")
            && err.contains("declares \"allow_exec\" twice")
            && err.contains(&format!("the first is at line {first}")),
    );

    // The capabilities table: a repeated name compiles into one name -> config map.
    let caps = r#"
@decision
Feature: dup_caps
Background:
  Given the capabilities
    | digest | {"kind": "text", "op": "length"} |
    | digest | {"kind": "text", "op": "words"}  |
Scenario: root
  Given a choice question "go" asks "go?"
    | answer | meaning |
    | yes | y |
    | no  | n |
  When any other answer goes to "STOP"
"#;
    let err = err_of(caps);
    let first = line_of(caps, "\"length\"", 0);
    let second = line_of(caps, "\"words\"", 0);
    h.check(
        "bdd table key once: refuses a capabilities table that repeats a name",
        err.contains(&format!("line {second}:"))
            && err.contains("capabilities table")
            && err.contains("declares \"digest\" twice")
            && err.contains(&format!("the first is at line {first}")),
    );

    // A noul question's annotations, the same loss one table over.
    let noul = r#"
@decision
Feature: dup_noul
Scenario: root
  Given a noul question "urgent" asks "Is this urgent?"
    | truthy | a deadline |
    | truthy | blocked    |
  When any other answer goes to "STOP"
"#;
    let err = err_of(noul);
    let first = line_of(noul, "a deadline", 0);
    let second = line_of(noul, "blocked", 0);
    h.check(
        "bdd table key once: refuses a noul table that repeats a key",
        err.contains(&format!("line {second}:"))
            && err.contains("noul question \"urgent\" table")
            && err.contains("declares \"truthy\" twice")
            && err.contains(&format!("the first is at line {first}")),
    );

    // Positive: the raw row key is what is compared, so `with.text` and
    // `project.text` (two different maps, both in the real corpus) still compile.
    let both = r#"
@decision
Feature: with_and_project
Background:
  Given the capabilities
    | digest | {"kind": "text", "op": "length"} |
Scenario: root
  Given a choice question "go" asks "go?"
    | answer | meaning |
    | yes | y |
    | no  | n |
  When any other answer goes to "STOP"
  Then call the capability "digest"
    | with.text | ${state.text} |
    | project.text | /text |
"#;
    h.check(
        "bdd table key once: with.text beside project.text is two maps, still compiles",
        err_of(both).is_empty(),
    );

    // Positive: an outer `with.text` beside a nested `chain.<as>.with.text` is the
    // same key in two different rows of the grammar, so it stays legal.
    let chained = r#"
@decision
Feature: chained
Background:
  Given the capabilities
    | digest | {"kind": "text", "op": "length"} |
    | echo   | {"kind": "text", "op": "words"}  |
Scenario: root
  Given a choice question "go" asks "go?"
    | answer | meaning |
    | yes | y |
    | no  | n |
  When any other answer goes to "STOP"
  Then call the capability "echo"
    | with.text | ${state.text} |
    | chain.digest.capability | digest |
    | chain.digest.with.text | ${state.text} |
"#;
    h.check(
        "bdd table key once: with.text beside chain.<as>.with.text still compiles",
        err_of(chained).is_empty(),
    );

    // Positive: a `score` question's level table is a list, so a repeated level is
    // kept (not dropped) and stays legal.
    let score = r#"
@decision
Feature: dup_score_level
Scenario: root
  Given a score question "quality" asks "How good is it?"
    | level |
    | high  |
    | high  |
  When any other answer goes to "STOP"
"#;
    h.check(
        "bdd table key once: a score level table is a list, a repeated level still compiles",
        err_of(score).is_empty(),
    );
}

/// P162: a *step* that sets something one-per-node, declared twice, is a silent
/// drop too.
///
/// P161 closed the table layer (a repeated row key inside one table). The same
/// loss happens one layer up, where the declaration is a whole step: `edge_condition`
/// is a map keyed by the answer, `edge_default` / `min_confidence` / `max_retries` /
/// the sub-workflow reference / the primary question / the action question are
/// `Option`s, `state.rename` is a map keyed by the output key, and a question holds
/// one `heuristic` object. Each of those keeps the *last* writer, so the step the
/// author wrote first is thrown away while `bdd build` still exits 0 - and unlike a
/// dropped row, a dropped route or setting changes what the node does.
///
/// The keys are per node and, for the heuristic, per question, so the normal corpus
/// shapes (several heuristics in one Scenario, one per question; a route plus a
/// catch-all; `keep the state key` twice, which is a list) stay legal - the positive
/// cases below pin exactly those.
pub fn test_bdd_step_once(h: &mut Harness) {
    // Compile a `@decision` feature and return its error text ("" when it compiles).
    let err_of = |text: &str| -> String {
        match bdd::parse(text, "<dup>") {
            Ok(f) => match bdd::compile_decision(&f, &json!({})) {
                Ok(_) => String::new(),
                Err(e) => format!("{e:#}"),
            },
            Err(e) => format!("{e:#}"),
        }
    };
    // The 1-based source line of the `nth` line that contains `needle`.
    let line_of = |text: &str, needle: &str, nth: usize| -> usize {
        text.lines()
            .enumerate()
            .filter(|(_, l)| l.contains(needle))
            .nth(nth)
            .map(|(i, _)| i + 1)
            .unwrap_or(0)
    };

    // A repeated route for the same answer: the second `goes to` overwrites the first.
    let route = r#"
@decision
Feature: dup_route
Scenario: root
  Given a choice question "go" asks "go?"
    | answer | meaning |
    | yes | y |
    | no  | n |
  When the answer "yes" goes to "STOP"
  And the answer "yes" goes to "STOP"
"#;
    let err = err_of(route);
    let first = line_of(route, "the answer \"yes\" goes to", 0);
    let second = line_of(route, "the answer \"yes\" goes to", 1);
    h.check(
        "bdd step once: refuses a scenario that routes one answer twice",
        err.contains(&format!("line {second}:"))
            && err.contains("the route for answer \"yes\"")
            && err.contains(&format!("the first is at line {first}")),
    );

    // A repeated catch-all: `any other answer goes to` is one Option.
    let fall = r#"
@decision
Feature: dup_fallthrough
Scenario: root
  Given a choice question "go" asks "go?"
    | answer | meaning |
    | yes | y |
    | no  | n |
  When the answer "yes" goes to "STOP"
  And any other answer goes to "STOP"
  And any other answer goes to "STOP"
"#;
    let err = err_of(fall);
    let first = line_of(fall, "any other answer goes to", 0);
    let second = line_of(fall, "any other answer goes to", 1);
    h.check(
        "bdd step once: refuses a scenario with two catch-all routes",
        err.contains(&format!("line {second}:"))
            && err.contains("a catch-all route")
            && err.contains(&format!("the first is at line {first}")),
    );

    // Two heuristics on the *same* question: the question keeps one `heuristic`
    // object, so the second step replaces the first.
    let heur = r#"
@decision
Feature: dup_heuristic
Scenario: root
  Given a choice question "go" asks "go?"
    | answer | meaning |
    | yes | y |
    | no  | n |
  Given answer "yes" when the text contains "a" with hit 0.9
  Given answer "yes" when the text contains "b" with hit 0.9
"#;
    let err = err_of(heur);
    let first = line_of(heur, "contains \"a\" with hit", 0);
    let second = line_of(heur, "contains \"b\" with hit", 0);
    h.check(
        "bdd step once: refuses a second heuristic on the same question",
        err.contains(&format!("line {second}:"))
            && err.contains("a heuristic on question \"go\"")
            && err.contains(&format!("the first is at line {first}")),
    );

    // Node-wide singletons, one case each: confidence floor, retry budget, primary
    // question, and a rename target.
    let conf = r#"
@decision
Feature: dup_confidence
Scenario: root
  Given a choice question "go" asks "go?"
    | answer | meaning |
    | yes | y |
    | no  | n |
  Then require confidence at least 0.8
  And require confidence at least 0.9
"#;
    let err = err_of(conf);
    let first = line_of(conf, "require confidence at least", 0);
    let second = line_of(conf, "require confidence at least", 1);
    h.check(
        "bdd step once: refuses a second confidence floor",
        err.contains(&format!("line {second}:"))
            && err.contains("`require confidence at least N`")
            && err.contains(&format!("the first is at line {first}")),
    );

    let retries = r#"
@decision
Feature: dup_retries
Scenario: root
  Given a choice question "go" asks "go?"
    | answer | meaning |
    | yes | y |
    | no  | n |
  Then allow up to 1 retries
  And allow up to 2 retries
"#;
    let err = err_of(retries);
    let first = line_of(retries, "allow up to", 0);
    let second = line_of(retries, "allow up to", 1);
    h.check(
        "bdd step once: refuses a second retry budget",
        err.contains(&format!("line {second}:"))
            && err.contains("`allow up to N retries`")
            && err.contains(&format!("the first is at line {first}")),
    );

    let primary = r#"
@decision
Feature: dup_primary
Scenario: root
  Given a choice question "a" asks "a?"
    | answer | meaning |
    | yes | y |
    | no  | n |
  Given a choice question "b" asks "b?"
    | answer | meaning |
    | yes | y |
    | no  | n |
  Then the primary question is "a"
  And the primary question is "b"
"#;
    let err = err_of(primary);
    let first = line_of(primary, "the primary question is ", 0);
    let second = line_of(primary, "the primary question is ", 1);
    h.check(
        "bdd step once: refuses a second primary question",
        err.contains(&format!("line {second}:"))
            && err.contains("the primary question")
            && err.contains(&format!("the first is at line {first}")),
    );

    let rename = r#"
@decision
Feature: dup_rename
Scenario: root
  Given a choice question "go" asks "go?"
    | answer | meaning |
    | yes | y |
    | no  | n |
  Then map the state key "out" from "a"
  And map the state key "out" from "b"
"#;
    let err = err_of(rename);
    let first = line_of(rename, "map the state key \"out\"", 0);
    let second = line_of(rename, "map the state key \"out\"", 1);
    h.check(
        "bdd step once: refuses a second rename of the same state key",
        err.contains(&format!("line {second}:"))
            && err.contains("the state rename for \"out\"")
            && err.contains(&format!("the first is at line {first}")),
    );

    // Positive: the corpus shape of the agents/* features - several heuristics in
    // one Scenario, one per question - must keep compiling.
    let perq = r#"
@decision
Feature: one_heuristic_per_question
Scenario: gate
  Given a choice question "staged" asks "staged?"
    | answer | meaning |
    | A      | no      |
    | B      | yes     |
  And answer "B" when the text contains "a" with hit 0.95
  And a choice question "repo" asks "repo?"
    | answer | meaning |
    | A      | no      |
    | B      | yes     |
  And answer "B" when the text contains "b" with hit 0.9
  Then label "FAIL" when the answer to "staged" is "B"
  And label "WARN" when the answer to "repo" is "B"
  And label "PASS" otherwise
"#;
    h.check(
        "bdd step once: one heuristic per question is the normal shape, still compiles",
        err_of(perq).is_empty(),
    );

    // Positive: distinct answers routed, plus a catch-all, compile.
    let many = r#"
@decision
Feature: two_routes
Scenario: root
  Given a choice question "go" asks "go?"
    | answer | meaning |
    | yes | y |
    | no  | n |
  When the answer "yes" goes to "STOP"
  And the answer "no" goes to "STOP"
  And any other answer goes to "STOP"
"#;
    h.check(
        "bdd step once: distinct answers plus a catch-all still compile",
        err_of(many).is_empty(),
    );

    // Positive: `keep the state key` targets a list, so a repeat is kept, not dropped.
    let keep = r#"
@decision
Feature: keep_twice
Scenario: root
  Given a choice question "go" asks "go?"
    | answer | meaning |
    | yes | y |
    | no  | n |
  Then keep the state key "a"
  And keep the state key "a"
"#;
    h.check(
        "bdd step once: keep the state key twice is a list, still compiles",
        err_of(keep).is_empty(),
    );

    // P163 closes the rest of the family: a question id declared twice in one
    // Scenario replaces the first question outright (options and heuristic and
    // all), and the Background's own one-per-feature steps - the iteration budget,
    // the policy, the capabilities - each set a single spec field, so a second one
    // silently wins.
    let dupq = r#"
@decision
Feature: dup_question
Scenario: root
  Given a choice question "go" asks "go?"
    | answer | meaning |
    | yes | y |
    | no  | n |
  And a choice question "go" asks "go again?"
    | answer | meaning |
    | yes | y |
    | no  | n |
  When any other answer goes to "STOP"
"#;
    let err = err_of(dupq);
    let first = line_of(dupq, "a choice question \"go\"", 0);
    let second = line_of(dupq, "a choice question \"go\"", 1);
    h.check(
        "bdd step once: refuses the same question id twice in one scenario",
        err.contains(&format!("line {second}:"))
            && err.contains("the question \"go\"")
            && err.contains(&format!("the first is at line {first}")),
    );

    let bg = r#"
@decision
Feature: dup_background
Background:
  Given the policy
    | allow_exec | true |
  Given the policy
    | allow_exec | false |
Scenario: root
  Given a choice question "go" asks "go?"
    | answer | meaning |
    | yes | y |
    | no  | n |
  When any other answer goes to "STOP"
"#;
    let err = err_of(bg);
    let first = line_of(bg, "Given the policy", 0);
    let second = line_of(bg, "Given the policy", 1);
    h.check(
        "bdd step once: refuses a second `Given the policy` in the Background",
        err.contains(&format!("line {second}:"))
            && err.contains("`Given the policy`")
            && err.contains(&format!("the first is at line {first}")),
    );

    let bgc = r#"
@decision
Feature: dup_background_caps
Background:
  Given the capabilities
    | digest | {"kind": "text", "op": "length"} |
  Given the capabilities
    | echo | {"kind": "text", "op": "words"} |
Scenario: root
  Given a choice question "go" asks "go?"
    | answer | meaning |
    | yes | y |
    | no  | n |
  When any other answer goes to "STOP"
"#;
    let err = err_of(bgc);
    let first = line_of(bgc, "Given the capabilities", 0);
    let second = line_of(bgc, "Given the capabilities", 1);
    h.check(
        "bdd step once: refuses a second `Given the capabilities` in the Background",
        err.contains(&format!("line {second}:"))
            && err.contains("`Given the capabilities`")
            && err.contains(&format!("the first is at line {first}")),
    );

    let bgi = r#"
@decision
Feature: dup_background_iterations
Background:
  Given the workflow runs at most 3 iterations
  Given the workflow runs at most 9 iterations
Scenario: root
  Given a choice question "go" asks "go?"
    | answer | meaning |
    | yes | y |
    | no  | n |
  When any other answer goes to "STOP"
"#;
    let err = err_of(bgi);
    let first = line_of(bgi, "runs at most", 0);
    let second = line_of(bgi, "runs at most", 1);
    h.check(
        "bdd step once: refuses a second iteration budget",
        err.contains(&format!("line {second}:"))
            && err.contains("`the workflow runs at most N iterations`")
            && err.contains(&format!("the first is at line {first}")),
    );

    // Positive: the normal Background carries all three settings at once.
    let okbg = r#"
@decision
Feature: full_background
Background:
  Given the workflow runs at most 4 iterations
  Given the policy
    | allow_exec | true |
  Given the capabilities
    | digest | {"kind": "text", "op": "length"} |
Scenario: root
  Given a choice question "go" asks "go?"
    | answer | meaning |
    | yes | y |
    | no  | n |
  When any other answer goes to "STOP"
"#;
    h.check(
        "bdd step once: one of each Background setting still compiles",
        err_of(okbg).is_empty(),
    );
}

/// P91: the declared output set must be real.
///
/// For every offline **single-node** `@decision` graph, `bdd_list`'s `outputs`
/// is the graph's *full* output set — there is no later node whose terminal
/// label could hide a member. So every declared label must be produced by at
/// least one registered smoke state; a declared-but-never-produced label is a
/// dead rule or an undriven branch, and an agent that plans against it (writes a
/// handler for a label the graph never returns) is being misled.
///
/// Two labels are allow-listed because the *offline* keyword heuristic cannot
/// reach them while a real model can; each entry carries its reason and is
/// asserted to still be a declared label, so the list cannot rot into a hole.
pub fn test_bdd_output_coverage(h: &mut Harness) {
    use laya_workflow::backend::HeuristicBackend;
    use laya_workflow::bdd_mcp::{declared_outputs, decision_label};
    use laya_workflow::dsl_smoke::{
        coverage_ledger_live_only, ensure_env_dirs, network_bound_external,
    };
    use std::collections::{BTreeMap, BTreeSet};

    if let Ok(env) = ensure_env_dirs() {
        for (k, v) in &env {
            std::env::set_var(k, v);
        }
    }
    let _ = laya_workflow::capability::secret::init(None);

    let root = bdd::repo_root();
    let dsl_root = root.join("dsl");
    let states: Value = serde_json::from_str(
        &std::fs::read_to_string(root.join("bench").join("dsl_smoke_states.json")).unwrap(),
    )
    .unwrap();
    let states_obj = states.as_object().unwrap();
    let live_only = coverage_ledger_live_only().unwrap_or_default();

    // (stem, label, why the offline heuristic cannot produce it)
    let allowed: &[(&str, &str, &str)] = &[
        (
            "quality_rubric",
            "human_review",
            "the rubric withholds the composite only when a dimension's answer confidence falls below the 0.8 floor; the offline keyword heuristic answers every hint (hit or miss) at a fixed confidence at or above the floor, so offline it always scores — a real model reaches human_review",
        ),
        (
            "content_policy_gate",
            "WARN",
            "every WARN antecedent (a severity of 1.5+, or a non-safe category) is only reachable through the same keywords that trip a BLOCK rule, and BLOCK is evaluated first, so the offline probe always blocks or approves; a real model can read a high severity with a safe category",
        ),
    ];
    let mut allow_seen: BTreeMap<String, bool> = allowed
        .iter()
        .map(|(s, l, _)| (format!("{s}:{l}"), false))
        .collect();

    let specs =
        laya_workflow::spec::discover(&dsl_root.to_string_lossy()).expect("discover dsl specs");
    let mut checked = 0usize;
    let mut declared_seen: BTreeSet<String> = BTreeSet::new();
    for (stem, path) in &specs {
        let Some(smap) = states_obj.get(stem).and_then(Value::as_object) else {
            continue;
        };
        let rel = path
            .strip_prefix(&dsl_root)
            .unwrap_or(path)
            .with_extension("")
            .to_string_lossy()
            .replace('\\', "/");
        if live_only.contains_key(&rel) {
            continue;
        }
        let raw: Value = std::fs::read_to_string(path)
            .ok()
            .and_then(|t| serde_json::from_str(&t).ok())
            .unwrap_or(Value::Null);
        // Single node only: a multi-node graph's terminal label is its last
        // node's and its earlier nodes' outputs surface through the trace, so
        // `outputs` there is not a pure "produced by a run" set.
        if raw
            .get("nodes")
            .and_then(Value::as_array)
            .map(|n| n.len())
            .unwrap_or(0)
            != 1
        {
            continue;
        }
        if network_bound_external(&raw) {
            continue;
        }
        let wf = match laya_workflow::spec::load_file(&path.to_string_lossy()) {
            Ok(w) => w,
            Err(e) => {
                h.check(&format!("bdd: output coverage {stem} loads ({e})"), false);
                continue;
            }
        };
        let mut produced: BTreeSet<String> = BTreeSet::new();
        let mut errored = false;
        for (_lab, st) in smap {
            match wf.run(&HeuristicBackend, st) {
                Ok(out) => {
                    let j = out.to_json();
                    produced.insert(decision_label(
                        j.get("result").unwrap_or(&Value::Null),
                        out.final_action(),
                    ));
                }
                Err(_) => errored = true,
            }
        }
        if errored {
            // A state the offline heuristic cannot drive is the integration
            // run's job; skip rather than report a coverage hole.
            continue;
        }
        checked += 1;
        for label in declared_outputs(&raw) {
            declared_seen.insert(format!("{stem}:{label}"));
            if produced.contains(&label) {
                continue;
            }
            let key = format!("{stem}:{label}");
            if let Some(hit) = allow_seen.get_mut(&key) {
                *hit = true;
                continue;
            }
            h.check(
                &format!(
                    "bdd: output coverage — {stem} declares {label} but no registered state produces it"
                ),
                false,
            );
        }
    }
    // Each allow-listed label must still be declared by its spec, else the entry
    // is stale and hides a hole.
    for (key, hit) in &allow_seen {
        h.check(
            &format!("bdd: allow-listed output {key} is still a declared label"),
            *hit && declared_seen.contains(key),
        );
        h.check(
            &format!("bdd: allow-listed output {key} has a stated reason"),
            allowed
                .iter()
                .any(|(s, l, why)| key == &format!("{s}:{l}") && why.len() > 40),
        );
    }
    h.check(
        &format!("bdd: output coverage checked {checked} single-node offline specs (floor 40)"),
        checked >= 40,
    );
}

/// bdd-list-routes: `bdd_list` exposes each node's decision *topology* — the
/// `routes` map (answer → destination) and `default` — so a multi-node graph is
/// a readable DFA, not a bag of nodes. Two ways this is held to reality:
///
///   1. projection identity — every listed node's `routes` / `default` equals its
///      `edge` in the checked-in spec (the inventory cannot invent a route);
///   2. engine cross-check — for every registered offline state of every offline
///      spec, each trace step's `answer` resolves through the node's edge to the
///      `next` the engine actually took (STOP / escalate → null, retry → self,
///      `EXECUTE:x` → x). A projection that disagreed with the engine would fail
///      here even if it matched the spec.
pub fn test_bdd_list_routes(h: &mut Harness) {
    use laya_workflow::backend::HeuristicBackend;
    use laya_workflow::dsl_smoke::{
        coverage_ledger_live_only, ensure_env_dirs, network_bound_external,
    };

    if let Ok(env) = ensure_env_dirs() {
        for (k, v) in &env {
            std::env::set_var(k, v);
        }
    }
    let _ = laya_workflow::capability::secret::init(None);

    let root = bdd::repo_root();
    let dsl_root = root.join("dsl");
    let states: Value = serde_json::from_str(
        &std::fs::read_to_string(root.join("bench").join("dsl_smoke_states.json")).unwrap(),
    )
    .unwrap();
    let states_obj = states.as_object().unwrap();
    let live_only = coverage_ledger_live_only().unwrap_or_default();

    let spec_edge = |node: &Value| -> (Value, Value) {
        let edge = node
            .get("edge")
            .or_else(|| node.get("edges"))
            .cloned()
            .unwrap_or_else(|| json!({}));
        let routes = edge
            .get("condition")
            .cloned()
            .filter(|c| c.is_object())
            .unwrap_or_else(|| json!({}));
        let default = edge.get("default").cloned().unwrap_or(Value::Null);
        (routes, default)
    };

    // 1) projection identity across the whole inventory.
    let list = bdd_tool("bdd_list").call(&json!({})).unwrap();
    let wfs = list["workflows"].as_array().unwrap();
    let mut checked_nodes = 0usize;
    for wf in wfs {
        let cat = wf["cat"].as_str().unwrap();
        let name = wf["name"].as_str().unwrap();
        let spec_path = dsl_root.join(cat).join(format!("{name}.json"));
        let Ok(txt) = std::fs::read_to_string(&spec_path) else {
            continue;
        };
        let raw: Value = match serde_json::from_str(&txt) {
            Ok(v) => v,
            Err(_) => continue,
        };
        // P102: `bdd_list` reports the graph the engine *walks* — `{"workflow": …}`
        // reference nodes inlined (namespaced) — so the projection identity is
        // against the *expanded* spec, which is also what makes a composed spec
        // (e.g. intake_pipeline_nested) comparable at all.
        let raw = laya_workflow::spec::expand_spec_nodes(&raw, spec_path.parent())
            .unwrap_or(raw);
        let spec_nodes = raw["nodes"].as_array().cloned().unwrap_or_default();
        let listed_nodes = wf["nodes"].as_array().unwrap();
        h.eq(
            &format!("bdd_list routes: {cat}/{name} node count matches the spec"),
            listed_nodes.len(),
            spec_nodes.len(),
        );
        for ln in listed_nodes {
            let nm = ln["name"].as_str().unwrap();
            let Some(sn) = spec_nodes.iter().find(|n| n["name"] == nm) else {
                h.check(&format!("bdd_list routes: {cat}/{name}.{nm} exists in spec"), false);
                continue;
            };
            let (want_routes, want_default) = spec_edge(sn);
            h.eq(
                &format!("bdd_list routes: {cat}/{name}.{nm} routes == spec edge"),
                ln["routes"].clone(),
                want_routes,
            );
            h.eq(
                &format!("bdd_list routes: {cat}/{name}.{nm} default == spec edge"),
                ln["default"].clone(),
                want_default,
            );
            h.check(
                &format!("bdd_list routes: {cat}/{name}.{nm} exposes a routes object"),
                ln["routes"].is_object(),
            );
            checked_nodes += 1;
        }
    }
    h.check(
        &format!("bdd_list routes: exposed topology for {checked_nodes} nodes (floor 300)"),
        checked_nodes >= 300,
    );

    // 2) engine cross-check: the trace's transitions agree with the exposed edge.
    let specs = laya_workflow::spec::discover(&dsl_root.to_string_lossy()).expect("discover dsl");
    let mut checked_specs = 0usize;
    let mut checked_steps = 0usize;
    for (stem, path) in &specs {
        let Some(smap) = states_obj.get(stem).and_then(Value::as_object) else {
            continue;
        };
        let rel = path
            .strip_prefix(&dsl_root)
            .unwrap_or(path)
            .with_extension("")
            .to_string_lossy()
            .replace('\\', "/");
        if live_only.contains_key(&rel) {
            continue;
        }
        let raw: Value = std::fs::read_to_string(path)
            .ok()
            .and_then(|t| serde_json::from_str(&t).ok())
            .unwrap_or(Value::Null);
        if network_bound_external(&raw) {
            continue;
        }
        let wf = match laya_workflow::spec::load_file(&path.to_string_lossy()) {
            Ok(w) => w,
            Err(_) => continue,
        };
        // Same expanded view as part 1: after inlining, a nested step's node name
        // (`{ref}::{inner}`) is a real entry here, so its transition is checked
        // rather than skipped.
        let raw = laya_workflow::spec::expand_spec_nodes(&raw, path.parent()).unwrap_or(raw);
        let nodes = raw["nodes"].as_array().cloned().unwrap_or_default();
        let mut errored = false;
        let mut agrees = true;
        for (_lab, st) in smap {
            let out = match wf.run(&HeuristicBackend, st) {
                Ok(o) => o,
                Err(_) => {
                    errored = true;
                    break;
                }
            };
            let j = out.to_json();
            let steps = j["trace"]["steps"].as_array().cloned().unwrap_or_default();
            for step in &steps {
                let nm = step["node"].as_str().unwrap_or("");
                let answer = &step["answer"];
                let next = step["next"].clone();
                let Some(sn) = nodes.iter().find(|n| n["name"] == nm) else {
                    // Every step the engine took must be an expanded node: a miss
                    // is a real hole (a name the inventory does not own).
                    agrees = false;
                    continue;
                };
                let (routes, default) = spec_edge(sn);
                let key = match answer {
                    Value::String(s) => s.clone(),
                    other => other.to_string(),
                };
                let dest = routes
                    .get(&key)
                    .cloned()
                    .or_else(|| if default.is_null() { None } else { Some(default.clone()) });
                // A destination that is itself a nested sub-workflow *container*
                // (a node carrying a `workflow` key) is entered by the engine as
                // `<container>::<inner start>`; that flattening belongs to the
                // nesting engine, so skip the hand-off step rather than model it.
                if let Some(d) = dest.as_ref().and_then(Value::as_str) {
                    if nodes
                        .iter()
                        .any(|n| n["name"] == d && n.get("workflow").is_some())
                    {
                        checked_steps += 1;
                        continue;
                    }
                }
                let expected_next = match dest.as_ref().and_then(Value::as_str) {
                    None => Value::Null,
                    Some("STOP") => Value::Null,
                    Some(d) if d == nm => json!(nm),
                    Some(d) => match d.strip_prefix("EXECUTE:") {
                        Some(t) => json!(t),
                        None => json!(d),
                    },
                };
                if expected_next != next {
                    agrees = false;
                }
                checked_steps += 1;
            }
        }
        if errored {
            continue;
        }
        h.check(
            &format!("bdd_list routes: {rel} trace transitions agree with the exposed edge"),
            agrees,
        );
        checked_specs += 1;
    }
    h.check(
        &format!(
            "bdd_list routes: engine-checked {checked_specs} offline specs / {checked_steps} trace steps (floor 60 specs, 400 steps)"
        ),
        checked_specs >= 60 && checked_steps >= 400,
    );
}

/// bdd-label-accounted: every label `bdd_decide` returns is *accounted for* — it
/// is either one of the graph's declared outputs (the enumerable decision), the
/// run's terminal verb (`final_action`, e.g. stop/escalate), or — only on a graph
/// with an opaque `call` node — a label a capability payload supplied
/// (`label_source == "capability"`). A graph that returns an undeclared,
/// non-terminal label with no opaque node is a hole and fails here. This is the
/// mirror of `bdd-output-coverage`: that gate stops `outputs` advertising labels
/// no run returns; this one stops a run returning a label `outputs` never
/// advertised.
pub fn test_bdd_label_accounted(h: &mut Harness) {
    use laya_workflow::backend::HeuristicBackend;
    use laya_workflow::bdd_mcp::{
        declared_outputs, decision_label, has_opaque_outputs, label_source,
    };
    use laya_workflow::dsl_smoke::{
        coverage_ledger_live_only, ensure_env_dirs, network_bound_external,
    };

    if let Ok(env) = ensure_env_dirs() {
        for (k, v) in &env {
            std::env::set_var(k, v);
        }
    }
    let _ = laya_workflow::capability::secret::init(None);

    let root = bdd::repo_root();
    let dsl_root = root.join("dsl");
    let states: Value = serde_json::from_str(
        &std::fs::read_to_string(root.join("bench").join("dsl_smoke_states.json")).unwrap(),
    )
    .unwrap();
    let states_obj = states.as_object().unwrap();
    let live_only = coverage_ledger_live_only().unwrap_or_default();

    let specs = laya_workflow::spec::discover(&dsl_root.to_string_lossy()).expect("discover dsl");
    let mut checked = 0usize;
    let mut capability_specs: Vec<String> = Vec::new();
    let mut capability_labels = 0usize;
    for (stem, path) in &specs {
        let Some(smap) = states_obj.get(stem).and_then(Value::as_object) else {
            continue;
        };
        let rel = path
            .strip_prefix(&dsl_root)
            .unwrap_or(path)
            .with_extension("")
            .to_string_lossy()
            .replace('\\', "/");
        if live_only.contains_key(&rel) {
            continue;
        }
        let raw: Value = std::fs::read_to_string(path)
            .ok()
            .and_then(|t| serde_json::from_str(&t).ok())
            .unwrap_or(Value::Null);
        if network_bound_external(&raw) {
            continue;
        }
        let wf = match laya_workflow::spec::load_file(&path.to_string_lossy()) {
            Ok(w) => w,
            Err(_) => continue,
        };
        // Report the graph the engine *walks*, mirroring `bdd_decide` /
        // `bdd_list`: inline every `{"workflow": …}` reference node so a
        // composed workflow's child labels belong to its own declared output
        // set. Without this a composition whose child emits the returned label
        // (e.g. `gateway_triage`, safe path -> the email-triage chain) reads as
        // an unaccounted label even though `bdd_list` enumerates it.
        let expanded = laya_workflow::spec::expand_spec_nodes(&raw, path.parent())
            .unwrap_or_else(|_| raw.clone());
        let declared = declared_outputs(&expanded);
        let opaque = has_opaque_outputs(&expanded);
        let mut errored = false;
        for (_lab, st) in smap {
            let out = match wf.run(&HeuristicBackend, st) {
                Ok(o) => o,
                Err(_) => {
                    errored = true;
                    break;
                }
            };
            let j = out.to_json();
            let result = j.get("result").unwrap_or(&Value::Null);
            let fa = out.final_action();
            let label = decision_label(result, fa);
            let src = label_source(&expanded, result, fa);
            let expect_src = if declared.iter().any(|o| o == &label) {
                "decision"
            } else if label == fa {
                "terminal"
            } else if opaque {
                "capability"
            } else {
                "unknown"
            };
            h.check(
                &format!("bdd: {rel} label_source({label}) is {expect_src}"),
                src == expect_src,
            );
            if src == "capability" {
                capability_labels += 1;
                if !capability_specs.contains(&rel) {
                    capability_specs.push(rel.clone());
                }
                h.check(
                    &format!("bdd: {rel} capability label {label} needs an opaque call node"),
                    opaque,
                );
            }
            if src == "unknown" {
                h.check(
                    &format!("bdd: {rel} returned an unaccounted label {label}"),
                    false,
                );
            }
        }
        if errored {
            continue;
        }
        checked += 1;
    }
    h.check(
        &format!("bdd: label accounting checked {checked} offline specs (floor 60)"),
        checked >= 60,
    );
    // The opaque escape is real but tiny: only a capability graph whose payload
    // emits an undeclared decision label needs it. A growing set means the
    // inventory is silently incomplete and should be investigated, not widened.
    h.check(
        &format!(
            "bdd: the capability-output escape names a tiny, bounded set ({:?})",
            capability_specs
        ),
        capability_specs.len() <= 3,
    );
    h.check(
        &format!("bdd: the capability-output escape is exercised ({capability_labels} labels)"),
        capability_labels > 0,
    );

    // The MCP surface carries the same contract: bdd_list flags a graph whose
    // `call` node can emit an undeclared label, and bdd_decide names the source.
    let list = bdd_tool("bdd_list").call(&json!({})).unwrap();
    let find = |n: &str| -> Value {
        list["workflows"]
            .as_array()
            .unwrap()
            .iter()
            .find(|w| w["name"] == n)
            .cloned()
            .unwrap_or(Value::Null)
    };
    h.eq(
        "bdd_list: cua_desktop reports outputs_complete=false (its call payload emits a label)",
        find("cua_desktop")["outputs_complete"].clone(),
        json!(false),
    );
    h.eq(
        "bdd_list: agent_permission_gate reports outputs_complete=true (no call node)",
        find("agent_permission_gate")["outputs_complete"].clone(),
        json!(true),
    );
}


/// P94: `bdd_decide` accepts caller-supplied `answers` — an agent / real model
/// drives the graph with its own perceptions while the graph stays the
/// deterministic decision core. Step-wise: `answers: []` names the start node's
/// question, and a short script stops at the next node instead of silently
/// falling back to the offline heuristic for the remaining steps.
pub fn test_bdd_mcp_answers(h: &mut Harness) {
    let t = bdd_tool("bdd_decide");
    let spec = "dsl/guards/agent_permission_gate.json";

    // (1) `answers: []` starts a step-wise walk: no step runs, the reply names
    //     the start node and its question instead of inventing answers.
    let start = t
        .call(&json!({"spec_path": spec, "state": {}, "answers": []}))
        .unwrap();
    h.eq("answers: [] is not ok (it needs an answer)", start["ok"].clone(), json!(false));
    h.eq("answers: [] reports backend=answers", start["backend"].clone(), json!("answers"));
    h.eq("answers: [] asks at the start node", start["at_node"].clone(), json!("unauthenticated"));
    h.eq("answers: [] sets needs_answer", start["needs_answer"].clone(), json!(true));
    h.eq("answers: [] ran no step", start["steps_so_far"].as_array().unwrap().len(), 0);
    // The `ask` payload is the node's own *contract* — the same shape `bdd_list`
    // reports at `nodes[].questions` — so a step-wise caller answers straight
    // from the reply, with no separate inventory call.
    let ask = start["ask"].as_array().cloned().unwrap_or_default();
    h.check(
        "answers: [] ask is a per-node contract list (id/type/answers/from_state)",
        !ask.is_empty()
            && ask.iter().all(|q| {
                q.get("id").is_some()
                    && q.get("type").is_some()
                    && q.get("answers").is_some()
                    && q.get("from_state").is_some()
            }),
    );
    let event = ask.iter().find(|q| q["id"] == "event").cloned().unwrap_or(Value::Null);
    let event_answers: Vec<&str> = event["answers"]
        .as_array()
        .map(|a| a.iter().filter_map(Value::as_str).collect())
        .unwrap_or_default();
    h.eq("answers: [] names the event question", event["id"].clone(), json!("event"));
    h.check(
        "answers: [] ask lists event's accepted answers",
        event_answers.len() == 6
            && event_answers.contains(&"login")
            && event_answers.contains(&"halt"),
    );
    // P105: the reply's `ask` contract marks the primary question and maps its
    // answers to the nodes they route to — the whole next-hop plan, in one reply.
    let prim = ask.iter().find(|q| q["primary"] == json!(true)).cloned().unwrap_or(Value::Null);
    h.eq("answers: [] ask marks the primary question (primary_q)", prim["id"].clone(), json!("event"));
    h.check(
        "answers: [] ask routes the primary answers to their destinations",
        prim["satisfied_by"]["login"] == json!("authenticated")
            && prim["satisfied_by"]["halt"] == json!("STOP"),
    );
    h.check(
        "answers: [] ask leaves the non-primary question un-routed",
        ask.iter()
            .find(|q| q["id"] == "risk")
            .map(|q| q["primary"] == json!(false) && !q.as_object().unwrap().contains_key("satisfied_by"))
            .unwrap_or(false),
    );
    // `ask` and `bdd_list`'s per-node `questions` are the same contract: a caller
    // can read the next step from either.
    let listed = bdd_tool("bdd_list")
        .call(&json!({"name": "agent_permission_gate"}))
        .unwrap();
    let listed_q = listed["workflows"]
        .as_array()
        .and_then(|ws| ws.iter().find(|w| w["name"] == "agent_permission_gate"))
        .and_then(|w| w["nodes"].as_array())
        .and_then(|ns| ns.iter().find(|n| n["name"] == "unauthenticated"))
        .map(|n| n["questions"].clone())
        .unwrap_or(Value::Null);
    h.eq(
        "answers: [] ask == bdd_list's per-node contract (same shape, same node)",
        start["ask"].clone(),
        listed_q,
    );

    // (2) A full caller script drives the graph; the trace echoes the caller's
    //     answers (login -> authorize -> halt), not the heuristic's.
    let step = |e: &str, r: &str| {
        json!({
            "event": {"type": "choice", "choice": e, "confidence": 1.0},
            "risk":  {"type": "choice", "choice": r, "confidence": 1.0}
        })
    };
    let driven = t
        .call(&json!({"spec_path": spec, "state": {},
            "answers": [step("login", "low"), step("authorize", "low"), step("halt", "high")]}))
        .unwrap();
    h.eq("answers: a full script returns ok", driven["ok"].clone(), json!(true));
    h.eq("answers: the driven call reports backend=answers", driven["backend"].clone(), json!("answers"));
    let steps = driven["trace"]["steps"].as_array().unwrap();
    h.eq("answers: three steps walked", steps.len(), 3);
    let fed: Vec<&str> = steps.iter().filter_map(|s| s["answer"].as_str()).collect();
    h.check("answers: the trace echoes the caller's answers", fed == vec!["login", "authorize", "halt"]);
    let nodes: Vec<&str> = steps.iter().filter_map(|s| s["node"].as_str()).collect();
    h.check(
        "answers: the walk follows the caller's route",
        nodes == vec!["unauthenticated", "authenticated", "authorized"],
    );
    h.eq("answers: the caller's risk band fixes the terminal label", driven["label"].clone(), json!("BLOCK"));

    // (2b) Control: the SAME empty state through the heuristic backend walks the
    //      defaults-only path — its first answer is the invented "none", not the
    //      caller's "login". So the answers really drove the run.
    let heur = t.call(&json!({"spec_path": spec, "state": {}})).unwrap();
    h.eq("answers: the heuristic backend is labelled", heur["backend"].clone(), json!("heuristic"));
    h.eq(
        "answers: the heuristic invents 'none' with no state",
        heur["trace"]["steps"][0]["answer"].clone(),
        json!("none"),
    );

    // (3) A short script stops at the next node and asks again — it does NOT
    //     silently fall back to the heuristic for the remaining steps.
    let short = t
        .call(&json!({"spec_path": spec, "state": {}, "answers": [step("login", "low")]}))
        .unwrap();
    h.eq("answers: a short script stops (not ok)", short["ok"].clone(), json!(false));
    h.eq("answers: the short script stops at the next node", short["at_node"].clone(), json!("authenticated"));
    h.eq("answers: the short script ran the one step it had", short["steps_so_far"].as_array().unwrap().len(), 1);
    h.check(
        "answers: the short script's ask is the next node's contract",
        short["ask"]
            .as_array()
            .map(|a| a.iter().any(|q| q["id"] == "event"))
            .unwrap_or(false),
    );

    // (4) `answers` that is neither an array nor an object is rejected before any
    //     walk. (An object is now a valid *node-keyed* shape — see (7); an object
    //     whose entries are not per-node answer objects is caught inside it.)
    h.check(
        "answers: a non-array, non-object is rejected",
        t.call(&json!({"spec_path": spec, "state": {}, "answers": "login"})).is_err()
            && t.call(&json!({"spec_path": spec, "state": {}, "answers": 42})).is_err(),
    );

    // (5) An entry that omits a question its node asks is rejected, naming both.
    let err = t
        .call(&json!({"spec_path": spec, "state": {},
            "answers": [{"risk": {"type": "choice", "choice": "low", "confidence": 1.0}}]}))
        .unwrap_err()
        .to_string();
    h.check(
        "answers: an entry missing an asked question is rejected, naming it",
        err.contains("answers[0]") && err.contains("event"),
    );

    // (6) A choice outside the question's accepted answers fails loudly instead of
    //     silently taking the edge's default branch.
    let bad = t
        .call(&json!({"spec_path": spec, "state": {},
            "answers": [{"event": {"type": "choice", "choice": "logn", "confidence": 1.0},
                         "risk": {"type": "choice", "choice": "low", "confidence": 1.0}}]}))
        .unwrap_err()
        .to_string();
    h.check(
        "answers: an out-of-contract choice is rejected, naming it",
        bad.contains("logn") && bad.contains("event") && bad.contains("default branch"),
    );

    // (7) Node-keyed `answers`: the same walk stated as a per-node policy. The
    //     object keyed by node name must reproduce the visit-order script exactly
    //     — same nodes, same answers, same terminal label — so a caller can state
    //     a policy once instead of replaying the visit sequence.
    let keyed = t
        .call(&json!({"spec_path": spec, "state": {},
            "answers": {
                "unauthenticated": step("login", "low"),
                "authenticated":   step("authorize", "low"),
                "authorized":      step("halt", "high")
            }}))
        .unwrap();
    h.eq("answers[keyed]: a node-keyed policy returns ok", keyed["ok"].clone(), json!(true));
    h.eq("answers[keyed]: reports backend=answers", keyed["backend"].clone(), json!("answers"));
    let keyed_steps = keyed["trace"]["steps"].as_array().unwrap();
    let keyed_nodes: Vec<&str> = keyed_steps.iter().filter_map(|s| s["node"].as_str()).collect();
    let keyed_fed: Vec<&str> = keyed_steps.iter().filter_map(|s| s["answer"].as_str()).collect();
    h.check(
        "answers[keyed]: the keyed walk == the visit-order walk (nodes)",
        keyed_nodes == vec!["unauthenticated", "authenticated", "authorized"],
    );
    h.check(
        "answers[keyed]: the keyed walk == the visit-order walk (answers)",
        keyed_fed == fed,
    );
    h.eq("answers[keyed]: the terminal label matches", keyed["label"].clone(), driven["label"].clone());

    // (8) A keyed entry is reused on every visit to its node: a self-loop edge
    //     (logout -> unauthenticated) is retried up to the node's max_retries,
    //     each retry re-reading the same keyed entry rather than advancing a
    //     cursor — the property that makes a keyed policy idempotent on loops.
    let looped = t
        .call(&json!({"spec_path": spec, "state": {},
            "answers": {"unauthenticated": step("logout", "low")}}))
        .unwrap();
    let loop_steps = looped["trace"]["steps"].as_array().unwrap();
    h.check(
        "answers[keyed]: a keyed entry is reused across the visits of a self-loop",
        loop_steps.len() >= 2
            && loop_steps.iter().all(|s| s["node"] == "unauthenticated"),
    );
    h.eq(
        "answers[keyed]: the self-loop exhausts the node's retries and escalates",
        looped["trace"]["final_action"].clone(),
        json!("escalate"),
    );

    // (9) A node the walk reaches with no keyed entry stops step-wise and names it.
    let miss = t
        .call(&json!({"spec_path": spec, "state": {},
            "answers": {"authenticated": step("authorize", "low")}}))
        .unwrap();
    h.eq("answers[keyed]: a missing node stops (not ok)", miss["ok"].clone(), json!(false));
    h.eq("answers[keyed]: the missing node is the start node", miss["at_node"].clone(), json!("unauthenticated"));
    h.eq("answers[keyed]: a missing node sets needs_answer", miss["needs_answer"].clone(), json!(true));
    h.check(
        "answers[keyed]: the missing-node reply carries that node's contract",
        miss["ask"].as_array().map(|a| a.iter().any(|q| q["id"] == "event")).unwrap_or(false),
    );

    // (10) A keyed entry with an out-of-contract answer still fails loudly (the
    //      P96 check is shared by both shapes).
    let keyed_bad = t
        .call(&json!({"spec_path": spec, "state": {},
            "answers": {"unauthenticated": step("logn", "low")}}))
        .unwrap_err()
        .to_string();
    h.check(
        "answers[keyed]: an out-of-contract keyed answer is rejected, naming node + value",
        keyed_bad.contains("logn")
            && keyed_bad.contains("unauthenticated")
            && keyed_bad.contains("default branch"),
    );

    // (11) `{}` starts a step-wise walk keyed by node — like `[]` for visit order.
    let keyed_start = t
        .call(&json!({"spec_path": spec, "state": {}, "answers": {}}))
        .unwrap();
    h.eq("answers[keyed]: {} starts a step-wise walk", keyed_start["needs_answer"].clone(), json!(true));
    h.eq("answers[keyed]: {} waits at the start node", keyed_start["at_node"].clone(), json!("unauthenticated"));

    // (12) P106: a dynamic (`criteria.from_state`) question carries an empty
    //      `answers` contract, so driving it step-wise needs the materialised
    //      options. The reply now carries `pending_options` (the waiting node's
    //      dynamic options built from the accumulated state) and `state_so_far`,
    //      so the walk closes from the reply alone.
    let ss = "dsl/capabilities/span_selection.json";
    let ss_state = json!({"text": "Please bill the invoice to billing@acme.com thanks."});
    let ss1 = t
        .call(&json!({"spec_path": ss, "state": ss_state, "answers": []}))
        .unwrap();
    h.check(
        "answers[dynamic]: the start node has no dynamic question (pending_options empty)",
        ss1["pending_options"] == json!({}),
    );
    let ss2 = t
        .call(&json!({"spec_path": ss, "state": ss_state,
            "answers": [{"has_text": {"type": "choice", "choice": "B", "confidence": 1.0}}]}))
        .unwrap();
    h.eq("answers[dynamic]: the walk stalls at the dynamic node", ss2["at_node"].clone(), json!("select"));
    let opts = ss2["pending_options"]["billing_contact"].clone();
    h.check(
        "answers[dynamic]: pending_options materialises the dynamic question's options",
        opts["candidate_0"].as_str().map(|s| s.contains("billing@acme.com")).unwrap_or(false)
            && opts.get("none").is_some(),
    );
    h.check(
        "answers[dynamic]: state_so_far carries the upstream candidates the options came from",
        ss2["state_so_far"]["candidates"]
            .as_array()
            .map(|a| a.iter().any(|c| c["id"] == "candidate_0"))
            .unwrap_or(false),
    );
    // Answer the dynamic question with an id taken straight from pending_options.
    let ss3 = t
        .call(&json!({"spec_path": ss, "state": ss_state, "answers": [
            {"has_text": {"type": "choice", "choice": "B", "confidence": 1.0}},
            {"billing_contact": {"type": "choice", "choice": "candidate_0", "confidence": 0.95}},
            {"ok": {"type": "choice", "choice": "A", "confidence": 1.0}},
        ]}))
        .unwrap();
    h.eq("answers[dynamic]: the full walk driven from the reply completes", ss3["ok"].clone(), json!(true));
    h.check(
        "answers[dynamic]: the picked id re-slices the real value from the candidate",
        ss3["result"]["selected"]["value"] == json!("billing@acme.com"),
    );
}


/// P95: the inventory is the contract. `bdd_list` now exposes, per node, the
/// questions that node asks (id / type / accepted answers). This gate proves the
/// loop is closed end-to-end: for **every offline spec × registered state** it
/// (1) walks the graph with the offline heuristic, (2) checks the inventory's
/// per-node contract is *exactly* the questions the engine asked, (3) rebuilds an
/// `answers` script from that contract alone, and (4) drives the graph with it —
/// asserting the two walks are identical (same nodes, same per-step answers,
/// same terminal label). So a caller can drive any verified graph straight from
/// `bdd_list`, and the `answers` path provably reproduces offline-verified
/// behaviour, corpus-wide.
pub fn test_bdd_answers_from_inventory(h: &mut Harness) {
    use laya_workflow::backend::{AnswersBackend, HeuristicBackend};
    use laya_workflow::bdd_mcp::decision_label;
    use laya_workflow::dsl_smoke::{ensure_env_dirs, network_bound_external};
    use laya_workflow::workflow::NodeResult;
    use std::collections::{BTreeSet, HashMap};

    let root = bdd::repo_root();
    if let Ok(env) = ensure_env_dirs() {
        for (k, v) in &env {
            std::env::set_var(k, v);
        }
    }
    let _ = laya_workflow::capability::secret::init(None);

    // The inventory, as an agent sees it: workflow name -> node name -> questions.
    let list = bdd_tool("bdd_list").call(&json!({})).unwrap();
    let mut contract: HashMap<String, HashMap<String, Value>> = HashMap::new();
    for w in list["workflows"].as_array().cloned().unwrap_or_default() {
        let Some(nm) = w["name"].as_str() else { continue };
        let nodes = w["nodes"].as_array().cloned().unwrap_or_default();
        let m: HashMap<String, Value> = nodes
            .iter()
            .filter_map(|n| Some((n["name"].as_str()?.to_string(), n["questions"].clone())))
            .collect();
        contract.insert(nm.to_string(), m);
    }
    let exposed = contract
        .values()
        .flat_map(|m| m.values())
        .any(|q| q.as_array().map(|a| !a.is_empty()).unwrap_or(false));
    h.check(
        &format!(
            "answers-from-list: the inventory exposes per-node questions ({} workflows)",
            contract.len()
        ),
        exposed,
    );
    // The per-node contract is the node's OWN questions, not the workflow union:
    // in agent_permission_gate only some nodes ask `risk`.
    let apg = contract.get("agent_permission_gate").cloned().unwrap_or_default();
    let asks = |node: &str, qid: &str| {
        apg.get(node)
            .and_then(Value::as_array)
            .map(|qs| qs.iter().any(|x| x["id"].as_str() == Some(qid)))
            .unwrap_or(false)
    };
    h.check(
        "answers-from-list: a node's contract is its own questions, not the union",
        asks("unauthenticated", "event")
            && asks("unauthenticated", "risk")
            && asks("refused", "risk")
            && !asks("refused", "event"),
    );
    let risk_answers = apg
        .get("unauthenticated")
        .and_then(Value::as_array)
        .and_then(|qs| qs.iter().find(|x| x["id"] == "risk"))
        .map(|x| x["answers"].clone())
        .unwrap_or(Value::Null);
    h.check(
        "answers-from-list: a question exposes the answers it accepts",
        risk_answers
            .as_array()
            .map(|a| a.iter().any(|v| v == "high" || v == "low"))
            .unwrap_or(false),
    );

    let states: Value = serde_json::from_str(
        &std::fs::read_to_string(root.join("bench").join("dsl_smoke_states.json")).unwrap(),
    )
    .unwrap();
    let states_obj = states.as_object().unwrap();

    // Discover dsl specs exactly as `dsl smoke` does.
    let dsl_root = root.join("dsl");
    let mut specs = Vec::new();
    let mut stack = vec![dsl_root.clone()];
    while let Some(dir) = stack.pop() {
        for entry in std::fs::read_dir(&dir).unwrap() {
            let p = entry.unwrap().path();
            if p.is_dir() {
                stack.push(p);
            } else if p.extension().and_then(|e| e.to_str()) == Some("json")
                && !p
                    .file_name()
                    .map(|n| n.to_string_lossy().ends_with(".config.json"))
                    .unwrap_or(false)
            {
                specs.push(p);
            }
        }
    }
    specs.sort();

    let mut driven_states = 0usize;
    let mut contract_ok = 0usize;
    let mut specs_driven: BTreeSet<String> = BTreeSet::new();
    // Corpus-wide heuristic-path contract audit (P98): every answer the offline
    // heuristic itself produced must be one its question accepts (or the DSL's
    // fail-closed `none`) — the same silent-`default` bug class P96 hard-fails on
    // the caller path, checked here on the heuristic path so a new
    // out-of-contract heuristic (e.g. a `match_any` A/B guess on a 4-option
    // choice) cannot route a graph through its `default` branch unnoticed.
    let mut heur_answers_checked = 0usize;
    let mut heur_out_of_contract: Vec<String> = Vec::new();
    // P110: the same audit, extended to dynamic (`criteria.from_state`) questions,
    // whose candidate ids are materialised from the state (their literal criteria
    // is empty, so the static check above skips them).
    let mut heur_dyn_checked = 0usize;
    let mut heur_dyn_out_of_contract: Vec<String> = Vec::new();
    // P100: the node-keyed `answers` shape must reproduce the same walk for the
    // same driven states. `keyed_driven` counts the states it reproduced;
    // `keyed_conflict` counts states where some node's heuristic answer *differed
    // between its own visits*, which a one-entry-per-node policy cannot express
    // (that is a real limit of the shape, so it is counted + named, never dropped).
    let mut keyed_driven = 0usize;
    let mut keyed_conflict = 0usize;
    let mut keyed_conflict_names: BTreeSet<String> = BTreeSet::new();
    for spec in &specs {
        let stem = spec.file_stem().and_then(|s| s.to_str()).unwrap_or("").to_string();
        let Some(smap) = states_obj.get(&stem).and_then(Value::as_object) else {
            continue;
        };
        let raw: Value = std::fs::read_to_string(spec)
            .ok()
            .and_then(|t| serde_json::from_str(&t).ok())
            .unwrap_or(Value::Null);
        if network_bound_external(&raw) {
            continue;
        }
        let wf = match laya_workflow::spec::load_file(&spec.to_string_lossy()) {
            Ok(w) => w,
            Err(e) => {
                h.check(&format!("answers-from-list: {stem} loads ({e})"), false);
                continue;
            }
        };
        let nodes = contract.get(&stem);
        // P110: the graph the engine walks, so a composed child node's own raw
        // questions (with their `criteria.from_state`) are reachable by name.
        let expanded = laya_workflow::spec::expand_spec_nodes(&raw, spec.parent())
            .unwrap_or_else(|_| raw.clone());
        // node name -> its raw `questions` object (so a dynamic question's
        // `criteria.from_state` is reachable as `node_q[node][qid]`).
        let node_q: HashMap<String, Value> = expanded["nodes"]
            .as_array()
            .map(|ns| {
                ns.iter()
                    .filter_map(|n| Some((n["name"].as_str()?.to_string(), n["questions"].clone())))
                    .collect()
            })
            .unwrap_or_default();
        for (label, st) in smap {
            // (1) the offline heuristic walk, capturing each node's answer set.
            let mut heur: Vec<(String, Vec<(String, Value, Value, f64)>, Option<Value>)> =
                Vec::new();
            let mut h_answers: Vec<Value> = Vec::new();
            let hout = wf.run_with_progress(&HeuristicBackend, st, &mut |r: &NodeResult| {
                let mut ans: Vec<(String, Value, Value, f64)> = r
                    .verdict
                    .answers
                    .iter()
                    .map(|(q, d)| {
                        (
                            q.clone(),
                            d.answer.clone(),
                            Value::Object(d.probabilities.clone()),
                            d.confidence,
                        )
                    })
                    .collect();
                ans.sort_by(|a, b| a.0.cmp(&b.0));
                heur.push((r.node_name.clone(), ans, r.action_result.clone()));
                h_answers.push(r.edge_answer.clone());
            });
            let hout = match hout {
                Ok(o) => o,
                Err(_) => continue,
            };
            // (1b) The heuristic's own answers must be inside each question's
            //      contract: a choice answer outside `criteria` silently takes the
            //      edge's `default` branch (P96's bug class) — so catch it here,
            //      corpus-wide, before it can route. `none` is the DSL's explicit
            //      fail-closed answer and a dynamic question materialises its
            //      options from the state (empty literal criteria), so both are
            //      legitimate and audited separately below.
            // Mirror the engine's state: `st` plus each prior node's merged
            // payload (`merge_payload`), so a dynamic question is materialised
            // from the very state the engine handed that node.
            let mut acc = st.clone();
            for (node_name, ans, action_result) in &heur {
                let Some(qs) = nodes
                    .and_then(|m| m.get(node_name))
                    .and_then(Value::as_array)
                else {
                    continue;
                };
                for (qid, a, _p, _c) in ans {
                    let Some(qc) = qs.iter().find(|x| x["id"].as_str() == Some(qid.as_str()))
                    else {
                        continue;
                    };
                    if qc["type"].as_str().unwrap_or("choice") != "choice" {
                        continue;
                    }
                    let accepted: Vec<&str> = qc["answers"]
                        .as_array()
                        .map(|a| a.iter().filter_map(Value::as_str).collect())
                        .unwrap_or_default();
                    let Some(chosen) = a.as_str() else { continue };
                    // P110: a dynamic (`criteria.from_state`) question has no
                    // literal answers — its candidate ids are materialised from
                    // the state. Extend the audit to it: materialise exactly as
                    // the engine does and require the heuristic's chosen id to be
                    // one of them (or the DSL's fail-closed `none`), so an
                    // invented id cannot slip through uncovered just because the
                    // criteria were dynamic.
                    let qdef = node_q
                        .get(node_name)
                        .and_then(|qs| qs.get(qid))
                        .cloned();
                    let is_dynamic = qdef
                        .as_ref()
                        .and_then(|q| q.pointer("/criteria/from_state"))
                        .and_then(Value::as_str)
                        .is_some();
                    if is_dynamic && accepted.is_empty() {
                        let Some(qdef) = qdef else { continue };
                        let one = json!({ qid.clone(): qdef });
                        let mat = match laya_workflow::workflow::expand_criteria_from_state(&acc, &one)
                        {
                            Ok(m) => m,
                            Err(_) => continue,
                        };
                        let cands: BTreeSet<String> = mat
                            .get(qid)
                            .and_then(|q| q.get("criteria"))
                            .and_then(Value::as_object)
                            .map(|o| o.keys().cloned().collect())
                            .unwrap_or_default();
                        heur_dyn_checked += 1;
                        if chosen != "none" && !cands.contains(chosen) {
                            heur_dyn_out_of_contract.push(format!(
                                "{stem}:{label}:{node_name} heuristic answered dynamic {qid}={chosen:?}, not among the materialised candidates {cands:?}"
                            ));
                        }
                        continue;
                    }
                    if accepted.is_empty() {
                        continue;
                    }
                    if chosen == "none" {
                        continue;
                    }
                    heur_answers_checked += 1;
                    if !accepted.contains(&chosen) {
                        heur_out_of_contract.push(format!(
                            "{stem}:{label}:{node_name} heuristic answered {qid}={chosen:?}, not in {accepted:?}"
                        ));
                    }
                }
                // Mirror the engine: this node's payload is merged into the state
                // the next node sees.
                laya_workflow::workflow::merge_payload(&mut acc, action_result);
            }
            // (2)+(3) the contract must be exactly what the node asked, and the
            // script is rebuilt from it alone.
            let mut script: Vec<Value> = Vec::new();
            let mut ok = true;
            // The node-keyed policy: one entry per visited node, plus a note of any
            // node whose entry differs between visits (inexpressible as a policy).
            let mut per_node: serde_json::Map<String, Value> = serde_json::Map::new();
            let mut keyed_conflict_nodes: Vec<String> = Vec::new();
            for (node, ans, _ar) in &heur {
                let qc = nodes.and_then(|m| m.get(node)).cloned().unwrap_or(Value::Null);
                let listed: BTreeSet<String> = qc
                    .as_array()
                    .map(|a| {
                        a.iter()
                            .filter_map(|q| q["id"].as_str().map(str::to_string))
                            .collect()
                    })
                    .unwrap_or_default();
                let asked: BTreeSet<String> = ans.iter().map(|(q, _, _, _)| q.clone()).collect();
                if listed != asked {
                    ok = false;
                }
                let mut entry = serde_json::Map::new();
                for (qid, a, probs, conf) in ans {
                    let qtype = qc
                        .as_array()
                        .and_then(|arr| arr.iter().find(|x| x["id"].as_str() == Some(qid.as_str())))
                        .and_then(|x| x["type"].as_str())
                        .unwrap_or("choice");
                    let mut o = serde_json::Map::new();
                    o.insert("type".into(), json!(qtype));
                    let field = match qtype {
                        "score" => "score",
                        "noul" => "noul",
                        "boolean" => "probability",
                        _ => "choice",
                    };
                    o.insert(field.into(), a.clone());
                    if probs.as_object().map(|m| !m.is_empty()).unwrap_or(false) {
                        o.insert("probabilities".into(), probs.clone());
                    }
                    o.insert("confidence".into(), json!(conf));
                    entry.insert(qid.clone(), Value::Object(o));
                }
                let entry_v = Value::Object(entry);
                match per_node.get(node) {
                    Some(prev) if prev != &entry_v => {
                        if !keyed_conflict_nodes.contains(node) {
                            keyed_conflict_nodes.push(node.clone());
                        }
                    }
                    Some(_) => {}
                    None => {
                        per_node.insert(node.clone(), entry_v.clone());
                    }
                }
                script.push(entry_v);
            }
            if ok {
                contract_ok += 1;
            } else {
                h.check(
                    &format!("answers-from-list: {stem}:{label} contract == the questions asked"),
                    false,
                );
            }
            // (4) drive the graph from the contract-built script and compare.
            let be = match AnswersBackend::from_json(&Value::Array(script)) {
                Ok(b) => b,
                Err(e) => {
                    h.check(
                        &format!("answers-from-list: {stem}:{label} script builds ({e:#})"),
                        false,
                    );
                    continue;
                }
            };
            let mut awalk: Vec<(String, Value)> = Vec::new();
            let aout = match wf.run_with_progress(&be, st, &mut |r: &NodeResult| {
                awalk.push((r.node_name.clone(), r.edge_answer.clone()));
            }) {
                Ok(o) => o,
                Err(e) => {
                    h.check(
                        &format!("answers-from-list: {stem}:{label} answers walk runs ({e:#})"),
                        false,
                    );
                    continue;
                }
            };
            let hnodes: Vec<&String> = heur.iter().map(|(n, _, _)| n).collect();
            let anodes: Vec<&String> = awalk.iter().map(|(n, _)| n).collect();
            let ha: Vec<&Value> = h_answers.iter().collect();
            let aa: Vec<&Value> = awalk.iter().map(|(_, a)| a).collect();
            let hj = hout.to_json();
            let aj = aout.to_json();
            let hl = decision_label(&hj["result"], hout.final_action());
            let al = decision_label(&aj["result"], aout.final_action());
            if hnodes == anodes && ha == aa && hl == al {
                driven_states += 1;
                specs_driven.insert(stem.clone());
                // (5) P100: the same walk stated as a per-node policy (the node-keyed
                //     `answers` shape) must reproduce it. A node whose answer differs
                //     between its visits has no keyed form — counted + named above.
                if keyed_conflict_nodes.is_empty() {
                    match AnswersBackend::from_json(&Value::Object(per_node.clone())) {
                        Ok(keyed_be) => {
                            let mut kwalk: Vec<(String, Value)> = Vec::new();
                            match wf.run_with_progress(&keyed_be, st, &mut |r: &NodeResult| {
                                kwalk.push((r.node_name.clone(), r.edge_answer.clone()));
                            }) {
                                Ok(kout) => {
                                    let knodes: Vec<&String> =
                                        kwalk.iter().map(|(n, _)| n).collect();
                                    let ka: Vec<&Value> =
                                        kwalk.iter().map(|(_, a)| a).collect();
                                    let kj = kout.to_json();
                                    let kl = decision_label(&kj["result"], kout.final_action());
                                    if knodes == anodes && ka == aa && kl == al {
                                        keyed_driven += 1;
                                    } else {
                                        h.check(
                                            &format!(
                                                "answers-from-list[keyed]: {stem}:{label} the node-keyed policy reproduces the visit-order walk (v={anodes:?} k={knodes:?} vl={al} kl={kl})"
                                            ),
                                            false,
                                        );
                                    }
                                }
                                Err(e) => h.check(
                                    &format!("answers-from-list[keyed]: {stem}:{label} keyed walk runs ({e:#})"),
                                    false,
                                ),
                            }
                        }
                        Err(e) => h.check(
                            &format!("answers-from-list[keyed]: {stem}:{label} keyed policy builds ({e:#})"),
                            false,
                        ),
                    }
                } else {
                    keyed_conflict += 1;
                    for n in &keyed_conflict_nodes {
                        keyed_conflict_names.insert(format!("{stem}:{n}"));
                    }
                }
            } else {
                h.check(
                    &format!(
                        "answers-from-list: {stem}:{label} the answers walk reproduces the heuristic walk (h={hnodes:?} a={anodes:?} hl={hl} al={al})"
                    ),
                    false,
                );
            }
        }
    }
    h.check(
        &format!(
            "answers-from-list: drove {driven_states} offline states across {} specs from the inventory (contract-matched {contract_ok})",
            specs_driven.len()
        ),
        driven_states == contract_ok && specs_driven.len() >= 125,
    );
    // P102: a composed (sub-workflow) spec is no longer skipped — `bdd_list`
    // now reports the inlined node list, so its per-node contract describes the
    // sub-graph's nodes and the whole walk is drivable from the inventory.
    h.check(
        "answers-from-list: the composed (sub-workflow) spec is driven from the inventory, not skipped",
        specs_driven.contains("intake_pipeline_nested"),
    );
    h.check(
        &format!(
            "answers-from-list: every offline heuristic answer is inside its question's contract ({heur_answers_checked} choice answers checked; {} out-of-contract)",
            heur_out_of_contract.len()
        ),
        heur_out_of_contract.is_empty() && heur_answers_checked >= 1500,
    );
    if !heur_out_of_contract.is_empty() {
        for v in heur_out_of_contract.iter().take(10) {
            h.check(&format!("answers-from-list: heuristic out-of-contract: {v}"), false);
        }
    }
    // P110: the corpus audit now covers dynamic questions too — every heuristic
    // answer on a `criteria.from_state` question is a candidate materialised from
    // the state (or the fail-closed `none`), never an invented id.
    h.check(
        &format!(
            "answers-from-list: every heuristic answer on a dynamic question is a materialised candidate ({heur_dyn_checked} dynamic answers checked; {} invented)",
            heur_dyn_out_of_contract.len()
        ),
        heur_dyn_out_of_contract.is_empty() && heur_dyn_checked >= 2,
    );
    if !heur_dyn_out_of_contract.is_empty() {
        for v in heur_dyn_out_of_contract.iter().take(10) {
            h.check(
                &format!("answers-from-list: dynamic heuristic out-of-contract: {v}"),
                false,
            );
        }
    }
    // P100: every driven state was also reproduced from a node-keyed policy, or is
    // named as keyed-inexpressible (a node whose answer differed between visits).
    h.check(
        &format!(
            "answers-from-list[keyed]: the node-keyed policy reproduces the visit-order walk for {keyed_driven} offline states ({keyed_conflict} keyed-inexpressible)",
            ),
        keyed_driven + keyed_conflict == driven_states && keyed_driven >= 950,
    );
    if keyed_conflict > 0 {
        let names: Vec<String> = keyed_conflict_names.iter().cloned().collect();
        h.check(
            &format!("answers-from-list[keyed]: keyed-inexpressible nodes (a node answering differently across its own visits) = {names:?}"),
            names.len() <= 8,
        );
    }
    // P111: the *caller's* `answers` path fails closed on two answers P96's
    // choice check alone lets through — the silent-`default` hole P110 closed on
    // the heuristic path:
    //   (a) an *invented* dynamic candidate id: a `criteria.from_state` question's
    //       literal `criteria` is empty, so the check can only bite if it sees the
    //       *materialised* criteria — which `WorkflowNode::run` builds from the
    //       node's input state before it asks the backend; and
    //   (b) a *wrong-typed* answer (a `score` object for a `choice` question): it
    //       carries no `criteria` key, so the choice check skips it and the engine
    //       silently takes the edge's `default` branch.
    // Prove both on the visit-order array and the node-keyed object, while the
    // real, correctly-typed candidate drives the very same graph to STOP.
    {
        let spec = root.join("dsl").join("capabilities").join("span_selection.json");
        let wf = laya_workflow::spec::load_file(&spec.to_string_lossy()).unwrap();
        // `extract` projects state.candidates from this text (one email id).
        let st = json!({ "text": "please reach me at billing@example.com for the invoice" });
        let has_text = json!({"has_text": {"type":"choice","choice":"B","confidence":0.99}});
        let ok_step = json!({"ok": {"type":"choice","choice":"A","confidence":0.99}});
        let good =
            json!({"billing_contact": {"type":"choice","choice":"candidate_0","confidence":0.99}});
        let bad =
            json!({"billing_contact": {"type":"choice","choice":"candidate_99","confidence":0.99}});

        // Positive control: the real candidate passes `select` and walks to STOP.
        let arr_ok = AnswersBackend::from_json(&json!([has_text.clone(), good.clone(), ok_step.clone()])).unwrap();
        h.check(
            "answers-from-list[dynamic]: the caller's real candidate drives the dynamic graph to STOP",
            matches!(wf.run(&arr_ok, &st), Ok(o) if o.final_action() == "stop"),
        );
        // Negative (visit-order array): an invented id is refused, by name.
        let arr_bad = AnswersBackend::from_json(&json!([has_text.clone(), bad.clone()])).unwrap();
        let e = wf.run(&arr_bad, &st).unwrap_err().to_string();
        h.check(
            &format!("answers-from-list[dynamic]: an invented id on the visit-order path fails closed ({e})"),
            e.contains("candidate_99") && e.contains("not one of its accepted answers"),
        );
        // Negative (node-keyed object): the same id is refused on that shape too.
        let obj_bad = AnswersBackend::from_json(
            &json!({"extract": has_text, "select": bad}),
        )
        .unwrap();
        let e2 = wf.run(&obj_bad, &st).unwrap_err().to_string();
        h.check(
            &format!("answers-from-list[dynamic]: an invented id on the node-keyed path fails closed ({e2})"),
            e2.contains("candidate_99") && e2.contains("not one of its accepted answers"),
        );
        // Negative (wrong type): a `score` object for a `choice` question is
        // refused on a static question and on the dynamic one alike — the same
        // silent `default` reached by the other door.
        let score_static =
            json!({"has_text": {"type":"score","score":0.9,"confidence":0.9}});
        let score_dyn = json!({"billing_contact": {"type":"score","score":0.9,"confidence":0.9}});
        let static_ok = json!({"has_text": {"type":"choice","choice":"B","confidence":0.99}});
        let e3 = wf
            .run(
                &AnswersBackend::from_json(&json!([score_static, good.clone(), ok_step.clone()]))
                    .unwrap(),
                &st,
            )
            .unwrap_err()
            .to_string();
        h.check(
            &format!("answers-from-list[dynamic]: a score-typed answer on a static choice question fails closed ({e3})"),
            e3.contains("\"has_text\"") && e3.contains("but the question is a \"choice\" one"),
        );
        let e4 = wf
            .run(
                &AnswersBackend::from_json(&json!([static_ok, score_dyn])).unwrap(),
                &st,
            )
            .unwrap_err()
            .to_string();
        h.check(
            &format!("answers-from-list[dynamic]: a score-typed answer on a dynamic choice question fails closed ({e4})"),
            e4.contains("\"billing_contact\"") && e4.contains("but the question is a \"choice\" one"),
        );
    }
    // The registered set minus what we drove is the audited skip set: only the
    // spec whose offline walk needs an external network endpoint (it runs in the
    // integration job). It must stay tiny + named.
    let mut no_walk: BTreeSet<String> = states_obj.keys().cloned().collect();
    for s in &specs_driven {
        no_walk.remove(s);
    }
    h.check(
        &format!("answers-from-list: not driven (external only) = {no_walk:?}"),
        no_walk.len() == 1 && no_walk.contains("hf_trending"),
    );
}

/// P135: the step-wise `bdd_decide` walk is the documented agent loop — call
/// with `answers: []`, read `at_node` + `ask` from the reply, append one answer
/// object, repeat until `ok:true`. This gate drives that loop, corpus-wide,
/// through the *real MCP tool*: for every offline-walkable decision state it
/// captures the authoritative per-node answer script at the engine level, then
/// replays it one answer at a time and, at each prefix, asserts the reply
/// localizes the walk — `at_node` is the node the walk visits next and `ask` is
/// that node's own *non-empty* contract (the same shape `bdd_list` reports) —
/// until the final prefix returns `ok:true` with the walk's label. So the loop is
/// proven to close for the whole corpus and a step-wise caller is never left on a
/// node with an empty `ask` (a stuck walk is a hard failure). It also pins the
/// first reply's trust fields, so a caller learns up front whether the graph can
/// be walked offline at all.
pub fn test_bdd_decide_stepwise_walk(h: &mut Harness) {
    use laya_workflow::backend::HeuristicBackend;
    use laya_workflow::bdd_mcp::decision_label;
    use laya_workflow::dsl_smoke::{coverage_ledger_live_only, ensure_env_dirs, network_bound_external};
    use laya_workflow::workflow::NodeResult;
    use std::collections::{BTreeSet, HashMap};

    let root = bdd::repo_root();
    if let Ok(env) = ensure_env_dirs() {
        for (k, v) in &env {
            std::env::set_var(k, v);
        }
    }
    let _ = laya_workflow::capability::secret::init(None);

    let t = bdd_tool("bdd_decide");

    // The inventory contract, so every stepped `ask` can be checked against the
    // exact per-node questions `bdd_list` reports.
    let list = bdd_tool("bdd_list").call(&json!({})).unwrap();
    let mut listed: HashMap<String, HashMap<String, Value>> = HashMap::new();
    for w in list["workflows"].as_array().cloned().unwrap_or_default() {
        let Some(nm) = w["name"].as_str() else { continue };
        let m: HashMap<String, Value> = w["nodes"]
            .as_array()
            .cloned()
            .unwrap_or_default()
            .iter()
            .filter_map(|n| Some((n["name"].as_str()?.to_string(), n["questions"].clone())))
            .collect();
        listed.insert(nm.to_string(), m);
    }

    let states: Value = serde_json::from_str(
        &std::fs::read_to_string(root.join("bench").join("dsl_smoke_states.json")).unwrap(),
    )
    .unwrap();
    let states_obj = states.as_object().unwrap();

    // Discover dsl specs exactly as `dsl smoke` / `answers-from-inventory` do.
    let dsl_root = root.join("dsl");
    let mut specs = Vec::new();
    let mut stack = vec![dsl_root.clone()];
    while let Some(dir) = stack.pop() {
        for entry in std::fs::read_dir(&dir).unwrap() {
            let p = entry.unwrap().path();
            if p.is_dir() {
                stack.push(p);
            } else if p.extension().and_then(|e| e.to_str()) == Some("json")
                && !p
                    .file_name()
                    .map(|n| n.to_string_lossy().ends_with(".config.json"))
                    .unwrap_or(false)
            {
                specs.push(p);
            }
        }
    }
    specs.sort();

    let mut walkable = 0usize;
    let mut walked = 0usize;
    let mut steps_total = 0usize;
    let mut composed_states = 0usize;
    let mut specs_walked: BTreeSet<String> = BTreeSet::new();
    let mut stuck: Vec<String> = Vec::new();
    let mut ask_drift: Vec<String> = Vec::new();
    let mut label_drift: Vec<String> = Vec::new();

    for spec in &specs {
        let stem = spec.file_stem().and_then(|s| s.to_str()).unwrap_or("").to_string();
        let Some(smap) = states_obj.get(&stem).and_then(Value::as_object) else { continue };
        let raw: Value = std::fs::read_to_string(spec)
            .ok()
            .and_then(|t| serde_json::from_str(&t).ok())
            .unwrap_or(Value::Null);
        if network_bound_external(&raw) {
            continue;
        }
        let wf = match laya_workflow::spec::load_file(&spec.to_string_lossy()) {
            Ok(w) => w,
            Err(_) => continue,
        };
        // node name -> its raw `questions` object, so each captured answer carries
        // the `type` the question declares (the shape `AnswersBackend` checks).
        let expanded = laya_workflow::spec::expand_spec_nodes(&raw, spec.parent())
            .unwrap_or_else(|_| raw.clone());
        let node_q: HashMap<String, Value> = expanded["nodes"]
            .as_array()
            .map(|ns| {
                ns.iter()
                    .filter_map(|n| Some((n["name"].as_str()?.to_string(), n["questions"].clone())))
                    .collect()
            })
            .unwrap_or_default();
        let spec_str = spec.to_string_lossy().to_string();

        for (label, st) in smap {
            // (1) the authoritative per-node answer script, captured at the engine
            //     level: every question the heuristic answered for each visited node.
            let mut script: Vec<Value> = Vec::new();
            let mut order: Vec<String> = Vec::new();
            let hout = wf.run_with_progress(&HeuristicBackend, st, &mut |r: &NodeResult| {
                order.push(r.node_name.clone());
                let mut entry = serde_json::Map::new();
                for (qid, d) in r.verdict.answers.iter() {
                    let qtype = node_q
                        .get(&r.node_name)
                        .and_then(|qs| qs.get(qid.as_str()))
                        .and_then(|q| q.get("type"))
                        .and_then(Value::as_str)
                        .unwrap_or("choice");
                    let mut o = serde_json::Map::new();
                    o.insert("type".into(), json!(qtype));
                    let field = match qtype {
                        "score" => "score",
                        "noul" => "noul",
                        "boolean" => "probability",
                        _ => "choice",
                    };
                    o.insert(field.into(), d.answer.clone());
                    let probs = Value::Object(d.probabilities.clone());
                    if probs.as_object().map(|m| !m.is_empty()).unwrap_or(false) {
                        o.insert("probabilities".into(), probs);
                    }
                    o.insert("confidence".into(), json!(d.confidence));
                    entry.insert(qid.clone(), Value::Object(o));
                }
                script.push(Value::Object(entry));
            });
            let hout = match hout {
                Ok(o) => o,
                Err(_) => continue,
            };
            walkable += 1;
            let hj = hout.to_json();
            let hlabel = decision_label(&hj["result"], hout.final_action());

            // (2)+(3) replay the loop one answer at a time through the real tool.
            let mut ok = true;
            for k in 0..=script.len() {
                let mut args = serde_json::Map::new();
                args.insert("spec_path".into(), json!(spec_str));
                args.insert("state".into(), st.clone());
                args.insert("answers".into(), Value::Array(script[..k].to_vec()));
                let r = match t.call(&Value::Object(args)) {
                    Ok(v) => v,
                    Err(e) => {
                        stuck.push(format!("{stem}:{label} step {k} errored: {e:#}"));
                        ok = false;
                        break;
                    }
                };
                if r["ok"] == json!(true) {
                    if k != script.len() || r["label"] != json!(hlabel) {
                        label_drift.push(format!(
                            "{stem}:{label} closed at step {k} (want {}) label {} != {hlabel}",
                            script.len(),
                            r["label"]
                        ));
                        ok = false;
                    }
                    break;
                }
                let at = r["at_node"].clone();
                let ask = r["ask"].as_array().cloned().unwrap_or_default();
                if at.is_null() || ask.is_empty() {
                    stuck.push(format!(
                        "{stem}:{label} step {k}: at_node={at} ask={}",
                        serde_json::to_string(&ask).unwrap_or_default()
                    ));
                    ok = false;
                    break;
                }
                if k >= order.len() || at != json!(order[k]) {
                    ask_drift.push(format!(
                        "{stem}:{label} step {k}: at_node {at} != the walk's node {:?}",
                        order.get(k)
                    ));
                    ok = false;
                    break;
                }
                let want = listed
                    .get(&stem)
                    .and_then(|m| m.get(order[k].as_str()))
                    .cloned()
                    .unwrap_or(Value::Null);
                if Value::Array(ask.clone()) != want {
                    ask_drift.push(format!(
                        "{stem}:{label} step {k}: ask != bdd_list contract at {}",
                        order[k]
                    ));
                    ok = false;
                    break;
                }
            }
            if ok {
                walked += 1;
                steps_total += script.len();
                if order.iter().any(|n| n.contains("::")) {
                    composed_states += 1;
                }
                specs_walked.insert(stem.clone());
            }
        }
    }

    h.check(
        &format!(
            "bdd-decide-stepwise: the loop closes for {walked} offline states across {} specs ({steps_total} steps replayed)",
            specs_walked.len()
        ),
        walked == walkable && walked >= 980 && specs_walked.len() >= 125,
    );
    h.check(
        "bdd-decide-stepwise: no walk stalls on a null at_node or an empty ask",
        stuck.is_empty(),
    );
    for v in stuck.iter().take(5) {
        h.check(&format!("bdd-decide-stepwise: stuck — {v}"), false);
    }
    h.check(
        "bdd-decide-stepwise: every stepped ask == bdd_list's per-node contract",
        ask_drift.is_empty(),
    );
    for v in ask_drift.iter().take(5) {
        h.check(&format!("bdd-decide-stepwise: ask drift — {v}"), false);
    }
    h.check(
        "bdd-decide-stepwise: the final reply's label == the heuristic walk's label",
        label_drift.is_empty(),
    );
    for v in label_drift.iter().take(5) {
        h.check(&format!("bdd-decide-stepwise: label drift — {v}"), false);
    }
    // A composed (sub-workflow) graph is stepped by name into its *inlined*
    // children (`intake::classify`), not the container - so the walker always has
    // a contract to answer.
    h.check(
        &format!("bdd-decide-stepwise: a composed graph steps into its inlined children ({composed_states} such states)"),
        composed_states > 0,
    );

    // (4) the first reply is self-describing about trust: an offline graph says so,
    //     from the very first `needs_answer` reply, not only from the final one.
    {
        let spec = "dsl/guards/agent_permission_gate.json";
        let r = t
            .call(&json!({"spec_path": spec, "state": {}, "answers": []}))
            .unwrap();
        h.check(
            "bdd-decide-stepwise: the first reply carries the trust fields (offline/verified_by/why_live)",
            r.get("offline").is_some() && r.get("verified_by").is_some() && r.get("why_live").is_some(),
        );
        h.eq(
            "bdd-decide-stepwise: an offline graph's first reply says offline=true",
            r["offline"].clone(),
            json!(true),
        );
    }
    // (5) a live-only graph says offline=false (routed to its live gate) from the
    //     first reply — the caller knows not to expect an offline walk.
    {
        let live_only = coverage_ledger_live_only().unwrap_or_default();
        let mut checked = 0usize;
        for (rel, entry) in live_only.iter() {
            let p = root.join("dsl").join(format!("{rel}.json"));
            if !p.is_file() {
                continue;
            }
            let v = match t.call(&json!({"spec_path": p.to_string_lossy(), "state": {}, "answers": []})) {
                Ok(v) => v,
                Err(_) => continue,
            };
            h.eq(
                &format!("bdd-decide-stepwise: live-only {rel} says offline=false on the first reply"),
                v["offline"].clone(),
                json!(false),
            );
            h.check(
                &format!("bdd-decide-stepwise: live-only {rel} names the gate that verifies it"),
                v["verified_by"].as_str().map(|s| !s.is_empty()).unwrap_or(false),
            );
            let _ = entry;
            checked += 1;
            if checked >= 3 {
                break;
            }
        }
        h.check(
            &format!("bdd-decide-stepwise: the live-only trust path is exercised ({checked} graphs)"),
            checked > 0,
        );
    }
    // (6) a dynamic (`criteria.from_state`) question: the reply carries an empty
    //     `answers` list (its candidates live in `pending_options`), and an answer
    //     outside the materialised candidates is refused, never silently defaulted.
    {
        let spec = "dsl/capabilities/span_selection.json";
        let st = json!({"text": "please reach me at billing@example.com for the invoice"});
        let r1 = t
            .call(&json!({"spec_path": spec, "state": st, "answers": [
                {"has_text": {"type":"choice","choice":"A","confidence":0.99}}
            ]}))
            .unwrap();
        let ask = r1["ask"].as_array().cloned().unwrap_or_default();
        let q = ask.iter().find(|q| q["id"] == "billing_contact").cloned().unwrap_or(Value::Null);
        h.check(
            "bdd-decide-stepwise: a dynamic question's ask carries an empty answers list + from_state",
            q["answers"].as_array().map(|a| a.is_empty()).unwrap_or(false)
                && q["from_state"] == json!("candidates"),
        );
        h.check(
            "bdd-decide-stepwise: the reply materialises the dynamic options in pending_options",
            r1["pending_options"]["billing_contact"]
                .as_object()
                .map(|m| m.contains_key("candidate_0"))
                .unwrap_or(false),
        );
        let bad = t.call(&json!({"spec_path": spec, "state": st, "answers": [
            {"has_text": {"type":"choice","choice":"A","confidence":0.99}},
            {"billing_contact": {"type":"choice","choice":"A","confidence":0.99}}
        ]}));
        h.check(
            "bdd-decide-stepwise: an out-of-contract dynamic answer is refused, not silently defaulted",
            bad.is_err(),
        );
    }
    // (7) the caller-answers *error* path names a live-only graph the same way the
    //     heuristic path does - the message routes to the live gate, so a caller is
    //     not left with a bare runtime error. An offline graph's error stays bare.
    {
        let live_bad = t.call(&json!({"spec_path": "dsl/browser/arxiv.json", "state": {}, "answers": [
            {"__probe__": {"type":"choice","choice":"A","confidence":0.9}}
        ]}));
        h.check(
            "bdd-decide-stepwise: a live-only graph's answers error names the live gate",
            live_bad
                .as_ref()
                .err()
                .map(|e| format!("{e:#}").contains("not offline-verifiable"))
                .unwrap_or(false),
        );
        let off_bad = t.call(&json!({"spec_path": "dsl/guards/agent_permission_gate.json", "state": {}, "answers": [
            {"__probe__": {"type":"choice","choice":"A","confidence":0.9}}
        ]}));
        h.check(
            "bdd-decide-stepwise: an offline graph's answers error stays bare (no live-gate note)",
            off_bad
                .as_ref()
                .err()
                .map(|e| !format!("{e:#}").contains("not offline-verifiable"))
                .unwrap_or(false),
        );
    }
}

/// The `platforms` gate behind the POSIX-shell skip: a spec that *calls* a
/// capability gated to another OS is skipped validate-only (not failed), so a
/// `["/bin/sh","-c","df … | awk …"]` exec or a `du … | awk` shell command can
/// live in the corpus without breaking the Windows smoke. Pure and
/// host-independent: the "current" tags are derived from `cfg!`, so the test
/// asserts both directions on whatever OS it runs on.
pub fn test_bdd_platform_bound_skip(h: &mut Harness) {
    use laya_workflow::dsl_smoke::platform_bound_here;

    let (here, other) = if cfg!(windows) {
        ("windows", "unix")
    } else {
        ("unix", "windows")
    };

    // Called + gated to the other platform -> bound here (skip).
    let other_only = json!({
        "capabilities": { "c": { "kind": "exec", "argv": ["x"], "platforms": [other] } },
        "nodes": [{ "name": "a", "action": { "kind": "call", "capability": "c" } }],
    });
    h.check(
        "bdd: platform-bound (called, other OS) is skipped",
        platform_bound_here(&other_only),
    );

    // Called + gated to this platform -> not bound.
    let here_only = json!({
        "capabilities": { "c": { "kind": "shell", "command": "x", "platforms": [here] } },
        "nodes": [{ "name": "a", "action": { "kind": "call", "capability": "c" } }],
    });
    h.check(
        "bdd: platform-available (called, this OS) is not skipped",
        !platform_bound_here(&here_only),
    );

    // Declared-but-uncalled other-OS capability must not knock out a portable
    // spec: only a capability a node actually calls can bind the run.
    let uncalled = json!({
        "capabilities": { "c": { "kind": "exec", "argv": ["x"], "platforms": [other] } },
        "nodes": [{ "name": "a", "action": { "kind": "call", "capability": "d" } }],
    });
    h.check(
        "bdd: uncalled other-OS capability does not bind the spec",
        !platform_bound_here(&uncalled),
    );

    // No `platforms` key at all -> portable.
    let portable = json!({
        "capabilities": { "c": { "kind": "exec", "argv": ["x"] } },
        "nodes": [{ "name": "a", "action": { "kind": "call", "capability": "c" } }],
    });
    h.check(
        "bdd: a capability with no `platforms` is portable",
        !platform_bound_here(&portable),
    );

    // Windows `Path::display()` yields `\`; the coverage-ledger keys, the
    // `[spec]` lines and the audit messages are all authored with `/`. The
    // normaliser is what keeps the ledger lookup from missing every entry on
    // Windows (`browser\github` vs `browser/github`). Pinned offline with a
    // literal backslash so it holds on any OS.
    use laya_workflow::dsl_smoke::slashed;
    use std::path::Path as P;
    h.eq(
        "bdd: slashed normalises backslashes to forward slashes",
        slashed(P::new("browser\\github.json"), P::new("dsl")),
        "browser/github.json".to_string(),
    );
    h.eq(
        "bdd: slashed strips the root prefix",
        slashed(P::new("/repo/dsl/a/b.json"), P::new("/repo/dsl")),
        "a/b.json".to_string(),
    );
}

/// P112: the engine holds **every** backend to the node's question contract — the
/// `choice` answer of *any* backend (a served System One without `--gbnf-strict`,
/// an offline heuristic on an unseen state, the caller's `answers`) must be one of
/// the question's `criteria` keys, or the DSL's fail-closed `none`; otherwise it
/// fails closed instead of silently taking the edge's `default` branch. This is the
/// same hole `AnswersBackend::accept` closes on the caller path and P98/P110 audit
/// on the heuristic, but guarded where it cannot be bypassed: in the engine.
pub fn test_bdd_engine_contract(h: &mut Harness) {
    use laya_workflow::backend::{choice, score, verdict, ScriptedBackend};
    use laya_workflow::workflow::Decision;
    use laya_workflow::spec::{from_spec, load_file};

    // gate --"A"--> allow, anything else --> deny; both terminal nodes ask `ok`.
    let spec = json!({
        "name": "contract_demo",
        "dsl_version": 2,
        "start": "gate",
        "max_iterations": 4,
        "nodes": [
            {"name": "gate", "primary_q": "verdict",
             "questions": {"verdict": {"type": "choice", "criteria": {"A": "allow it", "B": "deny it"}}},
             "edge": {"condition": {"A": "allow"}, "default": "deny"},
             "min_confidence": 0.0, "max_retries": 1},
            {"name": "allow", "primary_q": "ok",
             "questions": {"ok": {"type": "choice", "criteria": {"A": "yes"}}},
             "edge": {"condition": {}, "default": "STOP"},
             "min_confidence": 0.0, "max_retries": 1},
            {"name": "deny", "primary_q": "ok",
             "questions": {"ok": {"type": "choice", "criteria": {"A": "yes"}}},
             "edge": {"condition": {}, "default": "STOP"},
             "min_confidence": 0.0, "max_retries": 1}
        ]
    });
    let wf = from_spec(&spec).unwrap();

    // Baseline: a valid criteria key walks the named edge (A -> allow -> STOP).
    let ok = ScriptedBackend::new(vec![
        verdict(&[("verdict", choice("A", &[("A", 0.9), ("B", 0.1)]))]),
        verdict(&[("ok", choice("A", &[("A", 0.9)]))]),
    ]);
    h.check(
        "engine-contract: a valid criteria key walks its named edge (A -> allow -> STOP)",
        matches!(wf.run(&ok, &json!({})), Ok(o) if o.final_action() == "stop"),
    );

    // The hole: a backend answer OUTSIDE the question's criteria used to silently
    // take the edge's `default` branch (the model path with `--gbnf-strict` off).
    let bad = ScriptedBackend::new(vec![verdict(&[(
        "verdict",
        choice("Z", &[("A", 0.5), ("B", 0.5)]),
    )])]);
    let e = wf.run(&bad, &json!({})).unwrap_err().to_string();
    h.check(
        &format!("engine-contract: an answer outside the question's criteria fails closed, naming node/question/value/accepted ({e})"),
        e.contains("node gate")
            && e.contains("\"verdict\"")
            && e.contains("\"Z\"")
            && e.contains("[\"A\", \"B\"]"),
    );

    // A wrong-typed answer on the model path: the response decoder puts a `score` /
    // `boolean` / `noul` value (a number) in `answer`, so a `choice` question answered
    // with `type:"score"` is not a choice answer at all. P111 rejects that on the
    // caller path by reading the declared type; the engine fails closed on the shape.
    let typed = ScriptedBackend::new(vec![verdict(&[(
        "verdict",
        score(1.0, &[0.5, 0.5]),
    )])]);
    let te = wf.run(&typed, &json!({})).unwrap_err().to_string();
    h.check(
        &format!("engine-contract: a non-string (wrong-typed) answer to a choice question fails closed ({te})"),
        te.contains("node gate")
            && te.contains("\"verdict\"")
            && te.contains("non-string value")
            && te.contains("[\"A\", \"B\"]"),
    );

    // `none` stays the DSL's fail-closed answer even when it is not a criteria key.
    let none = ScriptedBackend::new(vec![
        verdict(&[("verdict", choice("none", &[("A", 0.5), ("B", 0.5)]))]),
        verdict(&[("ok", choice("A", &[("A", 0.9)]))]),
    ]);
    h.check(
        "engine-contract: `none` stays accepted (it routes the default branch, not a silent typo)",
        matches!(wf.run(&none, &json!({})), Ok(o) if o.final_action() == "stop"),
    );

    // A dynamic (`criteria.from_state`) question is held to its *materialised*
    // candidates: the engine expands them from the node's input state before it
    // asks the backend, so a model id that is not a candidate fails closed.
    let root = bdd::repo_root();
    let dynp = root.join("dsl").join("capabilities").join("span_selection.json");
    let dwf = load_file(&dynp.to_string_lossy()).unwrap();
    let dst = json!({ "text": "please reach me at billing@example.com for the invoice" });
    let dbad = ScriptedBackend::new(vec![
        verdict(&[("has_text", choice("B", &[("A", 0.1), ("B", 0.9)]))]),
        verdict(&[(
            "billing_contact",
            choice("candidate_99", &[("candidate_0", 0.4), ("none", 0.4)]),
        )]),
    ]);
    let de = dwf.run(&dbad, &dst).unwrap_err().to_string();
    h.check(
        &format!("engine-contract: a model answer outside a dynamic question's materialised candidates fails closed ({de})"),
        de.contains("node select") && de.contains("candidate_99"),
    );

    // Regression (P113): the offline heuristic's named handler for `urgency` is a
    // *score* rule (email_triage), but notify_macos declares the same id as a
    // *choice* — and notify_macos is external-only, so the corpus audit that drives
    // only state-carrying specs never saw it. The heuristic must answer the declared
    // type (the engine guard now fails closed otherwise). A minimal replay of that
    // node keeps the check self-contained (no capability/policy/env to resolve).
    let nm_like = from_spec(&json!({
        "name": "notify_like",
        "dsl_version": 2,
        "start": "announce",
        "max_iterations": 2,
        "nodes": [
            {"name": "announce", "primary_q": "urgency",
             "questions": {"urgency": {"type": "choice",
                "criteria": {"A": "informational, no action needed", "B": "action needed soon"}}},
             "edge": {"condition": {}, "default": "STOP"},
             "min_confidence": 0.0, "max_retries": 1}
        ]
    }))
    .unwrap();
    h.check(
        "engine-contract: the heuristic answers a choice `urgency` (a reused score-handler id) with a string option key, in-contract",
        matches!(nm_like.run(&laya_workflow::backend::HeuristicBackend, &json!({})),
                 Ok(o) if o.final_action() == "stop"),
    );

    // P114: the same contract on the *numeric* side. A `score`/`noul` answer is read
    // with `as_f64()`, which yields 0.0 for anything that is not a number — so a
    // string score would let a threshold rule gate on a silent 0.0. Decode the exact
    // model-path shape (`{"type":"score","score":"high"}` keeps the string in `answer`)
    // and drive a score question with it.
    let score_spec = from_spec(&json!({
        "name": "score_demo",
        "dsl_version": 2,
        "start": "rate",
        "max_iterations": 2,
        "nodes": [
            {"name": "rate", "primary_q": "rating",
             "questions": {"rating": {"type": "score"}},
             "edge": {"condition": {}, "default": "STOP"},
             "min_confidence": 0.0, "max_retries": 1}
        ]
    }))
    .unwrap();
    let string_score = laya_workflow::backend::verdict_from_response(&json!({
        "answers": {"rating": {"type": "score", "score": "high"}}
    }))
    .unwrap();
    let se = match score_spec.run(&ScriptedBackend::new(vec![string_score]), &json!({})) {
        Ok(_) => "NO ERROR — the string score was silently read as 0.0".to_string(),
        Err(e) => e.to_string(),
    };
    h.check(
        &format!("engine-contract: a non-numeric (wrong-typed) answer to a score question fails closed ({se})"),
        se.contains("node rate") && se.contains("\"rating\"") && se.contains("non-numeric value"),
    );

    // A `noul` question answered with a string (only reachable from a hand-written
    // backend) fails closed the same way.
    let noul_spec = from_spec(&json!({
        "name": "noul_demo",
        "dsl_version": 2,
        "start": "think",
        "max_iterations": 2,
        "nodes": [
            {"name": "think", "primary_q": "done",
             "questions": {"done": {"type": "noul"}},
             "edge": {"condition": {}, "default": "STOP"},
             "min_confidence": 0.0, "max_retries": 1}
        ]
    }))
    .unwrap();
    let string_noul = verdict(&[("done", Decision {
        answer: json!("true"),
        probabilities: serde_json::Map::new(),
        confidence: 0.9,
    })]);
    let ne = match noul_spec.run(&ScriptedBackend::new(vec![string_noul]), &json!({})) {
        Ok(_) => "NO ERROR — the string noul was silently read as 0.0".to_string(),
        Err(e) => e.to_string(),
    };
    h.check(
        &format!("engine-contract: a non-numeric (wrong-typed) answer to a noul question fails closed ({ne})"),
        ne.contains("node think") && ne.contains("\"done\"") && ne.contains("non-numeric value"),
    );

    // P115: a shape-correctness census over *every* spec — not just the 128 the
    // ledger's states drive. `bdd-answers-from-inventory` audits only
    // state-carrying specs, so the 35 live-only specs (notify_macos, hf_trending,
    // …) are never driven offline: a qid-keyed heuristic handler that returns the
    // wrong shape for a spec's declared type can hide there — exactly the P113
    // `urgency`-as-score-vs-choice bug. Drive the offline heuristic on each
    // spec's own statically-declared questions straight from the spec tree and
    // hold every answer to the engine's shape contract. Unlike the answer-keyed
    // audit this exercises the *named-handler* path (the one `default_choice`
    // never reaches), so it has teeth on both halves.
    {
        use laya_workflow::backend::HeuristicBackend;
        use laya_workflow::workflow::Decide;
        let root = bdd::repo_root();
        let specs = laya_workflow::spec::discover(&root.join("dsl").to_string_lossy()).unwrap();
        let mut specs_seen = 0usize;
        let mut checked_choice = 0usize;
        let mut checked_num = 0usize;
        let mut skipped_dynamic = 0usize;
        let mut bad: Vec<String> = Vec::new();
        for (stem, path) in specs {
            let Ok(text) = std::fs::read_to_string(&path) else {
                continue;
            };
            let Ok(raw) = serde_json::from_str::<Value>(&text) else {
                continue;
            };
            if raw.get("start").is_none() {
                continue;
            }
            let Some(nodes) = raw.get("nodes").and_then(Value::as_array) else {
                continue;
            };
            specs_seen += 1;
            for n in nodes {
                let Some(qs) = n.get("questions").and_then(Value::as_object) else {
                    continue;
                };
                for (qid, qdef) in qs {
                    // A `criteria.from_state` question materialises its candidates
                    // from the input state, so a bare-question drive has nothing to
                    // hold it to; the engine-state-driven audit (P110) owns it.
                    if qdef.pointer("/criteria/from_state").is_some() {
                        skipped_dynamic += 1;
                        continue;
                    }
                    let qtype = qdef.get("type").and_then(Value::as_str).unwrap_or("choice");
                    let one = json!({ qid.clone(): qdef.clone() });
                    let v = match HeuristicBackend.decide(&json!({}), &one) {
                        Ok(v) => v,
                        Err(e) => {
                            bad.push(format!("{stem}:{qid}: the heuristic refused to answer ({e})"));
                            continue;
                        }
                    };
                    let Some(d) = v.answers.get(qid) else {
                        bad.push(format!("{stem}:{qid}: the heuristic returned no answer"));
                        continue;
                    };
                    match qtype {
                        "choice" => {
                            checked_choice += 1;
                            let Some(a) = d.answer.as_str() else {
                                bad.push(format!(
                                    "{stem}:{qid}: a choice answer is not a string ({})",
                                    d.answer
                                ));
                                continue;
                            };
                            // `none` is the DSL's fail-closed answer; an empty
                            // criteria fails open (the engine accepts anything there).
                            if a == "none" {
                                continue;
                            }
                            let keys: Vec<String> = qdef
                                .get("criteria")
                                .and_then(Value::as_object)
                                .map(|o| o.keys().cloned().collect())
                                .unwrap_or_default();
                            if keys.is_empty() {
                                continue;
                            }
                            if !keys.iter().any(|k| k == a) {
                                bad.push(format!(
                                    "{stem}:{qid}: a choice answer {a:?} is not one of its criteria {keys:?}"
                                ));
                            }
                        }
                        "score" | "noul" => {
                            checked_num += 1;
                            if !d.answer.is_number() {
                                bad.push(format!(
                                    "{stem}:{qid}: a {qtype} answer is not a number ({})",
                                    d.answer
                                ));
                            }
                        }
                        _ => {}
                    }
                }
            }
        }
        h.check(
            &format!(
                "engine-contract: the shape census covers every workflow spec ({specs_seen} specs; {checked_choice} choice + {checked_num} score/noul answers held to the declared shape; {skipped_dynamic} dynamic skipped)"
            ),
            specs_seen == 163 && checked_choice >= 600 && checked_num >= 100 && skipped_dynamic == 1,
        );
        h.check(
            &format!(
                "engine-contract: every spec's heuristic answer obeys its question's declared shape ({} violation(s))",
                bad.len()
            ),
            bad.is_empty(),
        );
        for b in bad.iter().take(10) {
            h.check(&format!("engine-contract: shape violation: {b}"), false);
        }
    }

    // P118: the engine also holds every backend to the node's question *set*. A
    // backend that leaves a question the node asked unanswered used to be
    // tolerated (`hold_in_contract` `continue`d); the walk then read the missing
    // answer as "no match" (`rule_matches_multi`'s `None => false`), so a `label
    // ... when {question: q}` rule silently took a branch the backend never chose.
    // The caller path already fails closed on a missing answer (P96); now so does
    // the engine, for every backend — including the served System One.
    let two_q = from_spec(&json!({
        "name": "two_q",
        "dsl_version": 2,
        "start": "ask",
        "max_iterations": 2,
        "nodes": [
            {"name": "ask", "primary_q": "category",
             "questions": {
                "category": {"type": "choice", "criteria": {"billing": "bills", "other": "else"}},
                "urgency": {"type": "score"}},
             "edge": {"condition": {"billing": "STOP"}, "default": "STOP"},
             "min_confidence": 0.0, "max_retries": 1}
        ]
    }))
    .unwrap();
    let partial = ScriptedBackend::new(vec![verdict(&[(
        "category",
        choice("billing", &[("billing", 0.9), ("other", 0.1)]),
    )])]);
    let pe = match two_q.run(&partial, &json!({})) {
        Ok(_) => "NO ERROR — a missing answer was silently accepted".to_string(),
        Err(e) => e.to_string(),
    };
    h.check(
        &format!("engine-contract: a backend that leaves a node question unanswered fails closed ({pe})"),
        pe.contains("node ask") && pe.contains("\"urgency\"") && pe.contains("unanswered"),
    );
    let full = ScriptedBackend::new(vec![verdict(&[
        ("category", choice("billing", &[("billing", 0.9), ("other", 0.1)])),
        ("urgency", score(2.0, &[0.1, 0.2, 0.9])),
    ])]);
    h.check(
        "engine-contract: answering every question the node asks still walks (billing -> STOP)",
        matches!(two_q.run(&full, &json!({})), Ok(o) if o.final_action() == "stop"),
    );
}

/// `bdd run --cdp <url>`: attach to an existing browser instead of launching
/// one. Pins the pure pieces (config merge, URL→port, reachability pre-flight,
/// and the capability `attach` flag) so the wiring is not only exercised on a
/// machine that happens to have a browser.
pub fn test_bdd_cdp_attach(h: &mut Harness) {
    use laya_workflow::bdd::{attach_spec, cdp_endpoint_reachable, cdp_url_port};

    // The rewrite points every browser capability at the external endpoint
    // with launch=false/attach=true, and leaves non-browser capabilities alone.
    let mut spec = json!({
        "capabilities": {
            "chrome": {"kind": "chrome_cdp", "endpoint": "http://127.0.0.1:${state.cdp_port}",
                       "launch": true, "profile_dir": "${state.cdp_profile}", "human": true},
            "bdd": {"kind": "plugin", "plugin": "bdd"}
        }
    });
    attach_spec(&mut spec, "http://127.0.0.1:9222");
    h.eq(
        "cdp attach: endpoint rewritten",
        spec["capabilities"]["chrome"]["endpoint"].clone(),
        json!("http://127.0.0.1:9222"),
    );
    h.eq(
        "cdp attach: launch=false",
        spec["capabilities"]["chrome"]["launch"].clone(),
        json!(false),
    );
    h.eq(
        "cdp attach: attach=true",
        spec["capabilities"]["chrome"]["attach"].clone(),
        json!(true),
    );
    h.eq(
        "cdp attach: unrelated chrome keys preserved",
        spec["capabilities"]["chrome"]["human"].clone(),
        json!(true),
    );
    h.check(
        "cdp attach: non-browser capability untouched",
        spec["capabilities"]["bdd"].get("attach").is_none()
            && spec["capabilities"]["bdd"]["kind"] == json!("plugin"),
    );

    // Every browser capability is rewritten, not just the first.
    let mut two = json!({
        "capabilities": {
            "chrome": {"kind": "chrome_cdp", "launch": true},
            "page_open": {"kind": "browser", "endpoint": "http://127.0.0.1:1", "launch": true}
        }
    });
    attach_spec(&mut two, "http://127.0.0.1:9333");
    h.check(
        "cdp attach: every browser capability is rewritten",
        two["capabilities"]["chrome"]["endpoint"] == json!("http://127.0.0.1:9333")
            && two["capabilities"]["page_open"]["endpoint"] == json!("http://127.0.0.1:9333")
            && two["capabilities"]["page_open"]["attach"] == json!(true),
    );

    // URL → port, including the implicit-port forms.
    h.eq("cdp url: explicit port", cdp_url_port("http://127.0.0.1:9222"), Some(9222));
    h.eq("cdp url: http default", cdp_url_port("http://host/path"), Some(80));
    h.eq("cdp url: https default", cdp_url_port("https://host"), Some(443));

    // Reachability pre-flight: nothing listens on port 1 here, and a bogus host
    // must not resolve to "reachable".
    h.check(
        "cdp attach: an unreachable endpoint is detected",
        !cdp_endpoint_reachable("http://127.0.0.1:1"),
    );

    // The capability parses `attach` and does not imply launch.
    let reg = capability::Registry::from_spec(&json!({
        "capabilities": {
            "chrome": {"kind": "chrome_cdp", "endpoint": "http://127.0.0.1:9222", "attach": true}
        }
    }))
    .unwrap();
    match reg.get("chrome").unwrap() {
        capability::Capability::Browser(b) => h.check(
            "cdp attach: capability parses attach=true with launch=false",
            b.attach && !b.launch,
        ),
        _ => h.check("cdp attach: capability is a browser", false),
    }

    // A *compiled* browser cap is not the bare shape above: it carries the
    // launch path's own options too. `--cdp` used to hand exactly that shape to
    // the registry with only `attach` added, and the registry refused it - so
    // attach mode failed every scenario (0 passed / 8 failed, on the P190
    // binary as well). The rewrite must drop the launch-only options, and the
    // rewritten cap must survive the real registry (P191).
    let compiled_like = json!({
        "capabilities": {
            "chrome": {
                "kind": "chrome_cdp",
                "endpoint": "http://127.0.0.1:${state.cdp_port}",
                "profile_dir": "${state.cdp_profile}",
                "launch": true,
                "startup_timeout_ms": 30000,
                "timeout_ms": 15000,
                "chrome_binary": "scripts/bdd/chrome-headless.sh"
            }
        }
    });
    h.check(
        "cdp attach: a compiled launching cap is accepted as written",
        capability::Registry::from_spec(&compiled_like).is_ok(),
    );
    let mut naive = compiled_like.clone();
    naive["capabilities"]["chrome"]["attach"] = json!(true);
    h.check(
        "cdp attach: attach=true beside the launch-only options is refused",
        capability::Registry::from_spec(&naive).is_err(),
    );
    let mut attached = compiled_like;
    attach_spec(&mut attached, "http://127.0.0.1:9222");
    for dead in ["profile_dir", "chrome_binary", "startup_timeout_ms"] {
        h.check(
            &format!("cdp attach: {dead} is dropped by the rewrite"),
            attached["capabilities"]["chrome"].get(dead).is_none(),
        );
    }
    h.check(
        "cdp attach: a compiled browser cap survives the registry after the rewrite",
        capability::Registry::from_spec(&attached).is_ok(),
    );
}

/// P127: `dsl smoke` is cwd-independent. A spec's relative plugin `dir` resolves
/// off the checkout that holds the dsl tree (`dir_bases`: the cwd, each layered
/// spec root and its parent, then the crate root), not the cwd alone. Run the
/// real CLI from a directory outside every checkout — the exact failure this
/// pins was `feishu_unread_digest` (`dir: websites/feishu.com/...`) and
/// `single_file_plugin` (`dir: dsl/capabilities/hello.rhai`) failing off a
/// neutral cwd while the repo-root run stayed green.
pub fn test_dsl_smoke_cwd_independent(h: &mut Harness) {
    let root = bdd::repo_root();
    let total = laya_workflow::dsl_smoke::discover_specs(&root.join("dsl"))
        .expect("discover the corpus")
        .len();
    let mut bin = std::env::current_exe().expect("test binary path");
    bin.pop();
    if bin.ends_with("deps") {
        bin.pop();
    }
    bin.push("laya-workflow");
    // A cwd outside every checkout: no `dsl/`, no `plugins/`, no `.git` above it.
    let cwd = std::env::temp_dir().join(format!("laya-cwd-smoke-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&cwd);
    std::fs::create_dir_all(&cwd).unwrap();
    let out = std::process::Command::new(&bin)
        .args(["dsl", "smoke", "--quiet"])
        .current_dir(&cwd)
        .output()
        .expect("run dsl smoke");
    let text = format!(
        "{}{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
    h.check(
        "dsl-smoke: exits 0 from a cwd outside any checkout",
        out.status.success(),
    );
    if !out.status.success() {
        eprintln!("{}", &text[..text.len().min(4000)]);
    }
    h.check(
        "dsl-smoke: every spec passes off a neutral cwd",
        text.contains(&format!("{total}/{total} specs OK")),
    );
    h.check(
        "dsl-smoke: the isolation audit still runs off a neutral cwd",
        text.contains("isolation: bdd/ holds no json"),
    );
    let _ = std::fs::remove_dir_all(&cwd);
}

/// `bdd features-check` is the corpus-completeness gate: every checked-in
/// `.feature` compiles, and every one sits in a gate bucket. It was added
/// because `bdd/workflows/examples/` - the two features `docs/bdd_quickstart.md`
/// tells a new author to copy - was read by *no* gate, so a documented example
/// could rot (a vocabulary change drops a step) while every check stayed green.
/// This section pins the live corpus (all compile, the examples are counted) and
/// the bucket classifier (a path no gate compiles has no bucket, so the CLI
/// names it instead of letting it ship unverified).
pub fn test_bdd_features_check(h: &mut Harness) {
    use laya_workflow::bdd::{feature_bucket, features_check_impl, repo_root, Feature, Scenario, Step};

    let root = repo_root();
    let (total, compiled, buckets, _verdicts, problems) = features_check_impl(&root).unwrap();
    h.check(
        &format!(
            "bdd features-check: the live corpus compiles ({compiled}/{total}, {} problem(s))",
            problems.len()
        ),
        problems.is_empty() && total == compiled && total >= 200,
    );
    for p in &problems {
        h.check(&format!("bdd features-check: live problem — {p}"), false);
    }
    // The quickstart examples are the whole point of the gate.
    let ex = buckets.get("examples").copied().unwrap_or(0);
    h.check(
        &format!("bdd features-check: the quickstart examples are counted (examples={ex})"),
        ex >= 2,
    );
    h.check(
        "bdd features-check: the browser/decision/features/tasks trees are counted",
        buckets.get("browser").copied().unwrap_or(0) >= 25
            && buckets.get("decision").copied().unwrap_or(0) >= 100
            && buckets.get("features").copied().unwrap_or(0) >= 7
            && buckets.get("tasks").copied().unwrap_or(0) >= 2,
    );

    // The classifier. A directory no gate compiles has no bucket, so the CLI
    // names it rather than letting it ship unverified.
    let feat = |tags: &[&str]| Feature {
        name: "x".to_string(),
        description: String::new(),
        background: Vec::<Step>::new(),
        scenarios: vec![Scenario {
            name: "s".to_string(),
            steps: Vec::new(),
            tags: Vec::new(),
            line: 1,
            outline: false,
            examples: Vec::new(),
        }],
        tags: tags.iter().map(|s| s.to_string()).collect(),
        path: String::new(),
    };
    h.check(
        "bdd features-check: an ungated directory has no bucket",
        feature_bucket("bdd/workflows/tmp_ungated/probe.feature", &feat(&[])).is_none(),
    );
    h.check(
        "bdd features-check: a decision feature has a bucket",
        feature_bucket("bdd/workflows/whatever/x.feature", &feat(&["decision"])) == Some("decision"),
    );
    h.check(
        "bdd features-check: the examples bucket is by path",
        feature_bucket("bdd/workflows/examples/login_smoke.feature", &feat(&[])) == Some("examples"),
    );
}


/// The `dsl smoke` tally: `N/M specs OK` counts spec *documents* that passed,
/// never `total - failing_checks`. The per-spec counting is the fix for the live
/// run that printed `4294967265/1` (a u32 underflow the moment any state failed).
pub fn test_dsl_smoke_tally(h: &mut Harness) {
    use laya_workflow::dsl_smoke::tally;
    h.eq("dsl-smoke tally: empty corpus", tally(&[]), (0u32, 0u32, 0u32));
    h.eq("dsl-smoke tally: all specs pass", tally(&[0, 0, 0]), (3, 0, 0));
    let (ok, failed, checks) = tally(&[0, 0, 5]);
    h.eq("dsl-smoke tally: specs_ok counts specs, not checks", ok, 2);
    h.eq("dsl-smoke tally: the failing spec is named", failed, 1);
    h.eq("dsl-smoke tally: failing checks are summed", checks, 5);
    // The historical bug shape: one spec whose states produced far more failing
    // checks than there are specs. `total - fails` would underflow; the count
    // must simply be 0 of 1.
    let (ok2, failed2, checks2) = tally(&[32]);
    h.eq("dsl-smoke tally: one spec with 32 failing checks -> 0 of 1", ok2, 0);
    h.eq("dsl-smoke tally: still reports the one failed spec", failed2, 1);
    h.eq("dsl-smoke tally: the 32 checks survive", checks2, 32);
    let (ok3, failed3, _) = tally(&[1, 1, 0, 2, 1]);
    h.eq("dsl-smoke tally: mixed corpus ok", ok3, 1);
    h.eq("dsl-smoke tally: mixed corpus failed", failed3, 4);
}

/// P133/P134: the decision-scoped live gate's accounting. A live model's labels
/// are *meant* to differ from the heuristic-shaped offline expectations, so label
/// agreement is information — only a structural error (a compile/validate failure,
/// or a walk that does not TERMINATE: `ResilientWorkflow::run` returns `Ok` even at
/// its safety valve) may fail a spec. This pins that rule, plus the CLI wiring,
/// without needing a live endpoint.
pub fn test_bdd_live_decisions(h: &mut Harness) {
    use laya_workflow::bdd::{summarize_live, walk_outcome_clean, SpecLiveResult};

    // P134: "returned" is not "terminated" — the engine returns Ok at its safety valve.
    h.check(
        "bdd live-decisions: a real terminal (stop/escalate) is a clean walk",
        walk_outcome_clean("stop") && walk_outcome_clean("escalate") && walk_outcome_clean("converged"),
    );
    h.check(
        "bdd live-decisions: the safety valve is NOT a clean walk",
        !walk_outcome_clean("max_iterations") && !walk_outcome_clean("error_node_missing"),
    );

    // A clean spec whose labels all DISAGREE (states_run>0, states_agreed==0)
    // is still clean — a mismatch is not a failure.
    let clean_disagree = SpecLiveResult {
        rel: "routing/a.feature".into(),
        structural_error: None,
        states_run: 2,
        states_agreed: 0,
        states_skipped: 0,
        ..Default::default()
    };
    let clean_agree = SpecLiveResult {
        rel: "routing/b.feature".into(),
        structural_error: None,
        states_run: 3,
        states_agreed: 1,
        states_skipped: 1,
        states_looped: 1,
        ..Default::default()
    };
    let broken = SpecLiveResult {
        rel: "routing/c.feature".into(),
        structural_error: Some("label_x: backend 500".into()),
        states_run: 1,
        states_agreed: 0,
        states_skipped: 0,
        ..Default::default()
    };
    // A non-terminating walk fails the spec even when `structural_error` is unset —
    // `summarize_live` must not count it clean on the strength of a missing error.
    let nonterm = SpecLiveResult {
        rel: "routing/e.feature".into(),
        structural_error: None,
        states_run: 1,
        states_agreed: 0,
        states_skipped: 0,
        states_nonterminating: 1,
        ..Default::default()
    };
    let validate_only = SpecLiveResult {
        rel: "routing/d.feature".into(),
        ..Default::default()
    };

    let s = summarize_live(&[
        clean_disagree.clone(),
        clean_agree.clone(),
        broken.clone(),
        nonterm.clone(),
        validate_only.clone(),
    ]);
    h.check(
        "bdd live-decisions: specs_total counts every walked spec",
        s.specs_total == 5,
    );
    h.check(
        "bdd live-decisions: a label mismatch is NOT a failure (spec stays clean)",
        s.specs_clean == 3,
    );
    h.check(
        "bdd live-decisions: a structural error fails its spec",
        s.specs_clean < s.specs_total,
    );
    h.check(
        "bdd live-decisions: a non-terminating walk fails its spec even with no error string",
        summarize_live(&[nonterm.clone()]).specs_clean == 0,
    );
    h.check(
        "bdd live-decisions: states_run sums across specs",
        s.states_run == 2 + 3 + 1 + 1,
    );
    h.check(
        "bdd live-decisions: states_agreed sums across specs",
        s.states_agreed == 1,
    );
    h.check(
        "bdd live-decisions: states_skipped sums across specs",
        s.states_skipped == 1,
    );
    h.check(
        "bdd live-decisions: states_nonterminating sums across specs",
        s.states_nonterminating == 1,
    );
    h.check(
        "bdd live-decisions: states_looped sums across specs",
        s.states_looped == 1,
    );
    h.check(
        "bdd live-decisions: a validate-only spec is clean",
        summarize_live(&[validate_only.clone()]).specs_clean == 1,
    );
    h.check(
        "bdd live-decisions: an empty corpus is a clean 0/0 pass",
        {
            let z = summarize_live(&[]);
            z.specs_total == 0 && z.specs_clean == 0
        },
    );

    // CLI wiring: `bdd live-decisions` exists and, over a filter that matches
    // nothing, walks an empty corpus cleanly (exit 0) while still printing the
    // summary line. A dead base URL proves no live endpoint is required.
    let root = bdd::repo_root();
    let mut bin = std::env::current_exe().expect("test binary path");
    bin.pop();
    if bin.ends_with("deps") {
        bin.pop();
    }
    bin.push("laya-workflow");
    let out = std::process::Command::new(&bin)
        .args([
            "bdd",
            "live-decisions",
            "--base-url",
            "http://127.0.0.1:1",
            "--filter",
            "zzz_no_such_spec_133",
        ])
        .current_dir(&root)
        .output()
        .expect("run bdd live-decisions");
    let text = String::from_utf8_lossy(&out.stdout);
    h.check(
        "bdd live-decisions: CLI accepts the subcommand and prints the summary",
        out.status.success() && text.contains("live decisions:"),
    );
}

/// P137/P138: the app-parity gate — every built-in app's reference case replayed
/// end to end through the app's checked-in BDD port spec. The port is the app's
/// routing as *data*; the app is *code*. This pins that the two agree on all
/// reference cases (the 4 primitive apps *and* the composed gateway) AND that the
/// gate actually depends on the port spec (a mutated copy diverges), so a silently
/// drifted port fails here instead of passing.
pub fn test_bdd_app_parity(h: &mut Harness) {
    use laya_workflow::bdd::{app_parity_impl, repo_root};

    let root = repo_root();
    let report = app_parity_impl(&root).unwrap();
    let expect_total = laya_workflow::apps::all_reference_cases().len()
        + laya_workflow::apps::gateway_reference_cases().len();
    h.eq(
        "bdd app-parity: the reference set is the apps' cases (primitive + composed)",
        report.total,
        expect_total,
    );
    h.check(
        &format!(
            "bdd app-parity: every case reproduces its label ({}/{} case(s))",
            report.matched, report.total
        ),
        report.problems.is_empty() && report.matched == report.total && report.total == 32,
    );
    for p in &report.problems {
        h.check(&format!("bdd app-parity: divergence — {p}"), false);
    }

    // Flip tests: point the gate at a copy whose port spec has been mutated and
    // prove it now diverges — once for a primitive port and once for the composed
    // gateway port, so each half is shown to read the real spec rather than a
    // baked-in answer.
    let ports = [
        "dsl/guards/agent_command_gate.json",
        "dsl/apps/email_triage.json",
        "dsl/guards/content_policy_gate.json",
        "dsl/apps/draft_scorer.json",
        "dsl/apps/gateway_triage.json",
    ];
    let mutate = |rel: &str, from: &str, to: &str| -> laya_workflow::bdd::AppParityReport {
        let tmp = std::env::temp_dir().join(format!(
            "laya_app_parity_{}_{}",
            std::process::id(),
            rel.rsplit('/').next().unwrap()
        ));
        let _ = std::fs::remove_dir_all(&tmp);
        for p in ports {
            let dst = tmp.join(p);
            std::fs::create_dir_all(dst.parent().unwrap()).unwrap();
            let mut text = std::fs::read_to_string(root.join(p)).unwrap();
            if p == rel {
                assert_eq!(text.matches(from).count(), 1, "anchor {from} not unique in {rel}");
                text = text.replacen(from, to, 1);
            }
            std::fs::write(&dst, text).unwrap();
        }
        let r = app_parity_impl(&tmp).unwrap();
        let _ = std::fs::remove_dir_all(&tmp);
        r
    };

    let prim = mutate(
        "dsl/guards/agent_command_gate.json",
        "\"label\": \"ALLOW\"",
        "\"label\": \"NOPE\"",
    );
    h.check(
        "bdd app-parity: a mutated primitive port diverges (the gate reads the spec)",
        !prim.problems.is_empty() && prim.matched < prim.total,
    );
    let comp = mutate("dsl/apps/gateway_triage.json", "\"A\": \"triage\"", "\"A\": \"STOP\"");
    h.check(
        "bdd app-parity: a mutated composed port diverges (the gateway half reads the spec)",
        !comp.problems.is_empty() && comp.matched < comp.total,
    );
}

/// P164: a `call` step hands the engine a `with` object, and every kind reads a
/// fixed set of keys out of it and ignores the rest. A mistyped key is a silent
/// no-op - the value is dropped and the call runs with the default - so the
/// compiler refuses a key no call of that kind reads.
pub fn test_bdd_call_with_keys(h: &mut Harness) {
    use laya_workflow::capability::with_keys::{is_open, kind_reads};

    let build = |text: &str| -> (bool, String) {
        let feat = bdd::parse(text, "<probe>").expect("probe parses");
        match bdd::compile_decision(&feat, &json!({})) {
            Ok(_) => (true, String::new()),
            Err(e) => (false, format!("{e:#}")),
        }
    };

    // The failing shape: `with.epock` for `clock` (kind datetime). `datetime`
    // reads `epoch`; the typo is refused and the nearest key is named.
    let (ok, err) = build(
        r#"
@decision
Feature: bad_key
  Background:
    Given the capabilities
      | clock | {"kind": "datetime", "op": "now"} |
  Scenario: root
    Given a choice question "go" asks "go?"
      | answer | meaning |
      | A | yes |
      | B | no |
    When the answer "A" goes to "STOP"
    And any other answer goes to "STOP"
    Then call the capability "clock"
      | with.epock | 5 |
"#,
    );
    h.check(
        "bdd call with-keys: refuses a `with.<key>` no kind reads",
        !ok && err.contains("with.epock") && err.contains("datetime"),
    );
    h.check(
        "bdd call with-keys: names the nearest key the kind does read",
        err.contains("with.epoch"),
    );

    // A key the kind reads, a `project.` JSON pointer (not a `with` key), and an
    // aliased kind (`chrome_cdp` -> browser) all stay legal.
    let (ok, err) = build(
        r#"
@decision
Feature: good_key
  Background:
    Given the capabilities
      | clock | {"kind": "datetime", "op": "now"} |
      | page  | {"kind": "chrome_cdp", "endpoint": "http://127.0.0.1:9222"} |
  Scenario: root
    Given a choice question "go" asks "go?"
      | answer | meaning |
      | A | yes |
      | B | no |
    When the answer "A" goes to "STOP"
    And any other answer goes to "STOP"
    Then call the capability "clock"
      | with.epoch | 5 |
      | project.now | /text |
  Scenario: shot
    Given a choice question "go" asks "go?"
      | answer | meaning |
      | A | yes |
      | B | no |
    When the answer "A" goes to "STOP"
    And any other answer goes to "STOP"
    Then call the capability "page"
      | with.op | snapshot |
"#,
    );
    h.check(
        "bdd call with-keys: a read key, a project pointer and an alias kind stay legal",
        ok,
        );
    if !ok {
        h.check(&format!("  (unexpected: {err})"), false);
    }

    // An open kind (`plugin`, whose script reads whatever it wants) accepts any
    // key; and a key the capability's *definition* names as `${with.X}` is read
    // even though no `with.get` literal does.
    let (ok, err) = build(
        r#"
@decision
Feature: open_and_def
  Background:
    Given the capabilities
      | site  | {"kind": "plugin", "plugin": "websites/x"} |
      | vendor | {"kind": "http", "url": "http://127.0.0.1:9/", "body": {"text": "${with.text}"}} |
  Scenario: root
    Given a choice question "go" asks "go?"
      | answer | meaning |
      | A | yes |
      | B | no |
    When the answer "A" goes to "STOP"
    And any other answer goes to "STOP"
    Then call the capability "site"
      | with.whatever | 1 |
  Scenario: post
    Given a choice question "go" asks "go?"
      | answer | meaning |
      | A | yes |
      | B | no |
    When the answer "A" goes to "STOP"
    And any other answer goes to "STOP"
    Then call the capability "vendor"
      | with.text | hello |
"#,
    );
    h.check(
        "bdd call with-keys: an open kind and a `${with.X}`-named key are legal",
        ok,
    );
    if !ok {
        h.check(&format!("  (unexpected: {err})"), false);
    }

    // A `chain.<as>` entry is judged against *its own* capability's kind, not the
    // outer call's: a typo there must be refused too.
    let (ok, err) = build(
        r#"
@decision
Feature: chain_key
  Background:
    Given the capabilities
      | page  | {"kind": "chrome_cdp", "endpoint": "http://127.0.0.1:9222"} |
      | clock | {"kind": "datetime", "op": "now"} |
  Scenario: root
    Given a choice question "go" asks "go?"
      | answer | meaning |
      | A | you bet |
      | B | no |
    When any other answer goes to "STOP"
    Then call the capability "page"
      | with.op | open |
      | chain.cap.capability | clock |
      | chain.cap.with.epock | 5 |
"#,
    );
    h.check(
        "bdd call with-keys: a chain entry is judged by its own capability's kind",
        !ok && err.contains("with.epock"),
    );

    // The contract is read off the sources, not a table: `browser` really reads
    // `op`; `exec` reads no key by name (its `with` arrives through its argv
    // template); `plugin` / `agent` are open.
    h.check(
        "bdd call with-keys: the read set is read off the capability sources",
        kind_reads("browser").expect("browser").contains("op")
            && kind_reads("chrome_cdp").is_some()
            && kind_reads("exec").expect("exec").is_empty()
            && is_open("plugin")
            && is_open("agent")
            // `passthrough` / `noop` return the whole expanded `with`, so every
            // key is read - open, not an empty closed set.
            && is_open("passthrough")
            && is_open("noop")
            && kind_reads("datetime").expect("datetime").contains("epoch"),
    );
}

/// P165 — the `project` half of the `call` table: a row the engine can only
/// silently discard. `project.<key> | <pointer>` is applied by reading the
/// pointer with `as_str()`, so a pointer of another JSON type is skipped (the
/// state key is never written); an empty projection key or an empty chain alias
/// names something no reference can address. Each is refused.
pub fn test_bdd_call_project(h: &mut Harness) {
    let build = |text: &str| -> (bool, String) {
        let feat = bdd::parse(text, "<probe>").expect("probe parses");
        match bdd::compile_decision(&feat, &json!({})) {
            Ok(_) => (true, String::new()),
            Err(e) => (false, format!("{e:#}")),
        }
    };
    let head = r#"
@decision
Feature: proj
  Background:
    Given the capabilities
      | clock | {"kind": "datetime", "op": "now"} |
  Scenario: root
    Given a choice question "go" asks "go?"
      | answer | meaning |
      | A | yes |
      | B | no |
    When any other answer goes to "STOP"
    Then call the capability "clock"
"#;

    // A pointer of any JSON type but a string is skipped by `as_str()`: the
    // state key is never written, yet the compiler counts it as produced.
    let (ok, err) = build(&format!("{head}      | project.count | 3 |\n"));
    h.check(
        "bdd call project: a non-string pointer is refused",
        !ok && err.contains("project.count") && err.contains("not a JSON string"),
    );
    h.check(
        "bdd call project: the refusal names the state key it would never write",
        err.contains("state.count"),
    );

    // The same on a `chain` entry's own projection.
    let (ok, err) = build(&format!(
        "{head}      | project.now | /epoch |\n      | chain.cap.capability | clock |\n      | chain.cap.project.ok | true |\n"
    ));
    h.check(
        "bdd call project: a chain entry's non-string pointer is refused too",
        !ok && err.contains("chain.cap.project.ok"),
    );

    // An empty projection key writes a state key `${state.<key>}` cannot name.
    let (ok, err) = build(&format!("{head}      | project. | /epoch |\n"));
    h.check(
        "bdd call project: an empty projection key is refused",
        !ok && err.contains("project.") && err.contains("empty name"),
    );

    // An empty chain alias files the sub-call's result under a dead name.
    let (ok, err) = build(&format!("{head}      | project.now | /epoch |\n      | chain..capability | clock |\n"));
    h.check(
        "bdd call project: an empty chain alias is refused",
        !ok && err.contains("empty alias"),
    );

    // The legal shape: string pointers on the call and on a chain entry.
    let (ok, err) = build(&format!(
        "{head}      | project.now | /epoch |\n      | chain.extra.capability | clock |\n      | chain.extra.project.base | /offset_secs |\n"
    ));
    h.check(
        "bdd call project: string pointers on the call and a chain entry stay legal",
        ok,
    );
    if !ok {
        h.check(&format!("  (unexpected: {err})"), false);
    }
}

/// P166 — the *value* half of the `call` table: a `with.<key>` whose JSON type the
/// kind's read site does not accept is silently replaced by the default, so the
/// compiler refuses it. The wanted type is read off the `with` read site.
pub fn test_bdd_call_with_types(h: &mut Harness) {
    use laya_workflow::capability::with_keys::with_value_rejected;

    let build = |text: &str| -> (bool, String) {
        let feat = bdd::parse(text, "<probe>").expect("probe parses");
        match bdd::compile_decision(&feat, &json!({})) {
            Ok(_) => (true, String::new()),
            Err(e) => (false, format!("{e:#}")),
        }
    };
    let head = r#"
@decision
Feature: wty
  Background:
    Given the capabilities
      | clock | {"kind": "datetime", "op": "now"} |
      | page  | {"kind": "chrome_cdp", "endpoint": "http://127.0.0.1:9222"} |
      | cache | {"kind": "cache", "op": "set", "path": "/tmp/x.json", "ttl_secs": 60} |
  Scenario: root
    Given a choice question "go" asks "go?"
      | answer | meaning |
      | A | yes |
      | B | no |
    When any other answer goes to "STOP"
    Then call the capability "page"
"#;

    // `browser` reads `timeout_ms` with `as_u64()`: a string is silently dropped.
    let (ok, err) = build(&format!("{head}      | with.op | open |\n      | with.timeout_ms | \"5000\" |\n"));
    h.check(
        "bdd call with-types: a string on an integer read is refused",
        !ok && err.contains("with.timeout_ms") && err.contains("non-negative integer"),
    );
    h.check(
        "bdd call with-types: the refusal names the silent fallback",
        err.contains("silently discarded"),
    );

    // A number on a boolean read (`browser` reads `keep_open` with `as_bool()`).
    let (ok, err) = build(&format!("{head}      | with.op | open |\n      | with.keep_open | 1 |\n"));
    h.check(
        "bdd call with-types: a number on a boolean read is refused",
        !ok && err.contains("with.keep_open") && err.contains("boolean"),
    );

    // A chain entry is judged by *its own* capability's kind (`cache.ttl_secs` i64).
    let (ok, err) = build(&format!(
        "{head}      | with.op | open |\n      | chain.cap.capability | cache |\n      | chain.cap.with.ttl_secs | \"abc\" |\n"
    ));
    h.check(
        "bdd call with-types: a chain entry is judged by its own read site",
        !ok && err.contains("with.ttl_secs") && err.contains("\"cache\""),
    );

    // The legal shapes: a matching type, an untyped key (`op` goes through
    // `as_str`, and any key `get_text` stringifies is not judged), and a
    // `${state.x}` template whose runtime type is dynamic.
    let (ok, err) = build(&format!(
        "{head}      | with.op | open |\n      | with.timeout_ms | 5000 |\n      | with.keep_open | true |\n"
    ));
    h.check("bdd call with-types: matching types stay legal", ok);
    if !ok {
        h.check(&format!("  (unexpected: {err})"), false);
    }
    let (ok, err) = build(&format!(
        "{head}      | with.op | open |\n      | with.timeout_ms | ${{state.t}} |\n"
    ));
    h.check("bdd call with-types: a `${state.x}` template is not judged", ok);
    if !ok {
        h.check(&format!("  (unexpected: {err})"), false);
    }

    // The contract is read off the sources, not a table: a typed read refuses, an
    // untyped read (`op`) and an open kind (`plugin`) are never judged.
    h.check(
        "bdd call with-types: the type is read off the `with` read site",
        with_value_rejected("browser", "timeout_ms", &json!("x")).is_some()
            && with_value_rejected("cache", "ttl_secs", &json!("x")).is_some()
            && with_value_rejected("browser", "op", &json!(5)).is_none()
            && with_value_rejected("plugin", "keep_open", &json!(1)).is_none()
            && with_value_rejected("browser", "timeout_ms", &json!(5000)).is_none(),
    );
}

pub fn test_bdd_outputs_tag_shape(h: &mut Harness) {
    // P169: `@outputs(...)` is a contract, or it is an error. A payload tag the
    // reader cannot honour - a bare or emptied `@outputs`, a near-miss name, any
    // `@outputs(...)` in a @decision feature, or a tag line the parser would take
    // for description text - used to be dropped while every gate stayed green.
    let probs = |tags: &[&str], decision: bool| -> Vec<String> {
        let v: Vec<String> = tags.iter().map(|s| s.to_string()).collect();
        bdd::tag_shape_problems(&v, decision)
    };
    let has = |p: &[String], needle: &str| p.iter().any(|m| m.contains(needle));

    h.check(
        "bdd outputs tag: a well-formed list is left alone",
        probs(&["outputs(title, url)"], false).is_empty()
            && probs(&["decision", "outputs(title)", "expected_failure(typo)"], false).is_empty(),
    );
    h.check(
        "bdd outputs tag: an unrelated payload tag is not dragged in",
        probs(&["issue(PROJ-1)", "production", "smoke"], false).is_empty(),
    );
    let bare = probs(&["outputs"], false);
    h.check("bdd outputs tag: a bare @outputs is refused", bare.len() == 1);
    h.check(
        "bdd outputs tag: the bare message names the cause",
        has(&bare, "declares no outputs"),
    );
    h.check(
        "bdd outputs tag: `@outputs()` / `@outputs( )` / `@outputs(,)` are all empty",
        [["outputs()"], ["outputs(  )"], ["outputs(,)"]]
            .iter()
            .all(|t| probs(t, false).len() == 1),
    );
    let d = probs(&["outputs(label)"], true);
    h.check("bdd outputs tag: a @decision feature refuses a well-formed @outputs", d.len() == 1);
    h.check("bdd outputs tag: the @decision message says why", has(&d, "@decision"));
    let typo = probs(&["outptus(exit_code)"], false);
    h.check("bdd outputs tag: a transposed name is refused", typo.len() == 1);
    h.check("bdd outputs tag: the typo message suggests @outputs", has(&typo, "did you mean"));
    h.check(
        "bdd outputs tag: a singular `@output(...)` is refused too",
        has(&probs(&["output(exit_code)"], false), "did you mean"),
    );

    // End to end: the compiler refuses, and a green feature is untouched.
    let mk = |tag: &str| {
        format!(
            "Feature: outs\n  {tag}\n  Scenario: s\n    When I run the command \"true\"\n    Then the exit code is 0\n"
        )
    };
    let good = bdd::parse(&mk("@outputs(exit_code)"), "<outs-good>").expect("parse good");
    h.check(
        "bdd outputs tag: a produced output still compiles",
        bdd::compile_scenario(&good, &good.scenarios[0], &json!({}), "chrome-headless.sh").is_ok(),
    );
    let bad = bdd::parse(&mk("@outputs"), "<outs-bad>").expect("parse bad");
    let e = bdd::compile_scenario(&bad, &bad.scenarios[0], &json!({}), "chrome-headless.sh")
        .err()
        .map(|e| e.to_string())
        .unwrap_or_default();
    h.check(
        "bdd outputs tag: a bare @outputs fails the compile",
        e.contains("declares no outputs"),
    );
    let decl = bdd::parse("@decision\nFeature: d\n  @outputs(label)\n  Scenario: r\n    Given a choice question \"c\" asks \"?\"\n      | answer | meaning |\n      | a      | first   |\n      | b      | second  |\n    When any other answer goes to \"STOP\"\n    Then label \"A\" when the answer to \"c\" is \"a\"\n    And label \"B\" otherwise\n", "<outs-dec>").expect("parse decision");
    let e = bdd::compile_decision(&decl, &json!({}))
        .err()
        .map(|e| e.to_string())
        .unwrap_or_default();
    h.check(
        "bdd outputs tag: a @decision feature with @outputs is refused",
        e.contains("@decision"),
    );

    // A `@`-leading line the tag grammar does not accept must not become prose.
    for (name, src) in [
        ("an unclosed tag line", mk("@outputs(exit_code")),
        ("a space before the paren", mk("@outputs (exit_code)")),
    ] {
        let e = bdd::parse(&src, "<outs-shape>")
            .err()
            .map(|e| e.to_string())
            .unwrap_or_default();
        h.check(
            &format!("bdd outputs tag: {name} is refused, not read as prose"),
            e.contains("starts with '@'"),
        );
    }
}

pub fn test_bdd_payload_tags(h: &mut Harness) {
    // P170: the payload-tag guard covers *every* tag the toolchain reads a payload
    // from, and it cannot drift away from the readers (the P159/P160 drift-guard
    // shape, applied to tags).
    let probs = |tags: &[&str], decision: bool| -> Vec<String> {
        let v: Vec<String> = tags.iter().map(|s| s.to_string()).collect();
        bdd::tag_shape_problems(&v, decision)
    };
    let has = |p: &[String], needle: &str| p.iter().any(|m| m.contains(needle));

    // ── the drift scan: the readers themselves decide what the table must cover ──
    let src = include_str!("../bdd.rs");
    let mut readers: std::collections::BTreeSet<String> = std::collections::BTreeSet::new();
    let lit = regex_lite::Regex::new(r#"strip_prefix\("([a-z_][a-z0-9_]*)\("\)"#).unwrap();
    for c in lit.captures_iter(src) {
        readers.insert(c[1].to_string());
    }
    // `tag_reason(scenario, <ident>)`, with `<ident>` resolved through its binding.
    let mut bound: std::collections::BTreeMap<String, String> = std::collections::BTreeMap::new();
    for rx in [
        r#"const\s+([A-Za-z_][A-Za-z0-9_]*)\s*:\s*&str\s*=\s*"([a-z_][a-z0-9_]*)""#,
        r#"let\s+([A-Za-z_][A-Za-z0-9_]*)\s*=\s*"([a-z_][a-z0-9_]*)""#,
    ] {
        let rx = regex_lite::Regex::new(rx).unwrap();
        for c in rx.captures_iter(src) {
            bound.insert(c[1].to_string(), c[2].to_string());
        }
    }
    let call = regex_lite::Regex::new(
        r#"tag_reason\(\s*[A-Za-z_][A-Za-z0-9_:.]*\s*,\s*([A-Za-z_][A-Za-z0-9_]*)\s*\)"#,
    )
    .unwrap();
    let mut unresolved: Vec<String> = Vec::new();
    for c in call.captures_iter(src) {
        let ident = c[1].to_string();
        match bound.get(&ident) {
            Some(name) => {
                readers.insert(name.clone());
            }
            None => unresolved.push(ident),
        }
    }
    h.check(
        "bdd payload tags: every `tag_reason` call resolves to a literal name",
        unresolved.is_empty(),
    );
    if !unresolved.is_empty() {
        h.check(&format!("  (unresolved: {unresolved:?})"), false);
    }
    let covered: std::collections::BTreeSet<String> =
        bdd::PAYLOAD_TAGS.iter().map(|s| s.to_string()).collect();
    h.check(
        "bdd payload tags: PAYLOAD_TAGS is exactly the set of payload readers",
        readers == covered,
    );
    if readers != covered {
        h.check(
            &format!("  (readers={readers:?} covered={covered:?})"),
            false,
        );
    }

    // ── the guard's behaviour on the names the scan found ──
    h.check(
        "bdd payload tags: a bare @expected_failure is refused",
        has(&probs(&["expected_failure"], false), "declares no reason"),
    );
    h.check(
        "bdd payload tags: `@expected_failure()` is empty too",
        probs(&["expected_failure()"], false).len() == 1,
    );
    h.check(
        "bdd payload tags: a transposed @expected_failur is refused",
        has(
            &probs(&["expected_failur(bdd.assert: FAIL)"], false),
            "did you mean @expected_failure",
        ),
    );
    h.check(
        "bdd payload tags: a reason makes it legal",
        probs(&["expected_failure(bdd.assert: FAIL equals)"], false).is_empty(),
    );
    let d = probs(&["expected_failure(x)"], true);
    h.check(
        "bdd payload tags: @decision refuses an xfail it never reads",
        d.len() == 1 && has(&d, "@decision"),
    );
    h.check(
        "bdd payload tags: @decision refuses a bare @expected_failure too",
        probs(&["expected_failure"], true).len() == 1,
    );

    // ── a wrong refusal is worse than a missed typo: similar names stay legal ──
    h.check(
        "bdd payload tags: a merely similar name is not refused",
        probs(
            &[
                "inputs(seed)",
                "options(a, b)",
                "objects(x)",
                "expected_thing(y)",
                "output_dir(z)",
                "decision",
                "production",
                "smoke",
            ],
            false,
        )
        .is_empty(),
    );

    // ── end to end: the compiler refuses the empty one, the proper one compiles ──
    let mk = |tag: &str| {
        format!(
            "Feature: xf\n  {tag}\n  Scenario: s\n    When I run the command \"true\"\n    Then the exit code is 0\n"
        )
    };
    let good = bdd::parse(&mk("@expected_failure(bdd.assert: FAIL equals)"), "<xf-good>")
        .expect("parse good xfail");
    h.check(
        "bdd payload tags: a browser scenario with a reason still compiles",
        bdd::compile_scenario(&good, &good.scenarios[0], &json!({}), "chrome-headless.sh").is_ok(),
    );
    let bad = bdd::parse(&mk("@expected_failure"), "<xf-bad>").expect("parse bare xfail");
    let e = bdd::compile_scenario(&bad, &bad.scenarios[0], &json!({}), "chrome-headless.sh")
        .err()
        .map(|e| e.to_string())
        .unwrap_or_default();
    h.check(
        "bdd payload tags: a bare @expected_failure fails the compile",
        e.contains("declares no reason"),
    );
}

pub fn test_bdd_selector_tags(h: &mut Harness) {
    // A selector tag written almost right selects nothing: `bdd build --profile
    // production` picks up only a scenario tagged exactly `@production`, so a
    // near-miss is silently skipped (verified: a `@prodution` feature compiles
    // locally at 100% coverage yet contributes zero production-IT). The guard
    // refuses the near-miss, and the table it uses cannot drift from the
    // `has_tag` readers.
    let probs = |tags: &[&str], decision: bool| -> Vec<String> {
        let v: Vec<String> = tags.iter().map(|s| s.to_string()).collect();
        bdd::tag_shape_problems(&v, decision)
    };
    let has = |p: &[String], needle: &str| p.iter().any(|m| m.contains(needle));

    // ── the drift scan: every `has_tag(<tags>, …)` reader is accounted for ──
    let src = include_str!("../bdd.rs");
    let mut readers: std::collections::BTreeSet<String> = std::collections::BTreeSet::new();
    let lit = regex_lite::Regex::new(r#"has_tag\([^,]*,\s*"([a-z_][a-z0-9_]*)"\s*\)"#).unwrap();
    for c in lit.captures_iter(src) {
        readers.insert(c[1].to_string());
    }
    // An ident second argument (`has_tag(tags, prod_tag)`) resolves through its
    // binding; an unresolvable ident (`fname`, `tf`) is a dynamic file-name
    // filter, not a reserved tag, so it is skipped rather than failed.
    let mut bound: std::collections::BTreeMap<String, String> = std::collections::BTreeMap::new();
    for rx in [
        r#"const\s+([A-Za-z_][A-Za-z0-9_]*)\s*:\s*&str\s*=\s*"([a-z_][a-z0-9_]*)""#,
        r#"let\s+([A-Za-z_][A-Za-z0-9_]*)\s*=\s*"([a-z_][a-z0-9_]*)""#,
    ] {
        let rx = regex_lite::Regex::new(rx).unwrap();
        for c in rx.captures_iter(src) {
            bound.insert(c[1].to_string(), c[2].to_string());
        }
    }
    let ident =
        regex_lite::Regex::new(r#"has_tag\([^,]*,\s*([A-Za-z_][A-Za-z0-9_]*)\s*\)"#).unwrap();
    for c in ident.captures_iter(src) {
        if let Some(name) = bound.get(&c[1].to_string()) {
            readers.insert(name.clone());
        }
    }
    // `decision` is read by `has_tag` too but is deliberately not guarded: its
    // near-miss already fails loudly, and refusing it would risk a false refusal
    // on ordinary words (`decide`, `decisions`). The exclusion is pinned here so a
    // new bare-flag reader cannot slip in under its cover.
    let loud: std::collections::BTreeSet<String> =
        ["decision"].iter().map(|s| s.to_string()).collect();
    let expected: std::collections::BTreeSet<String> = bdd::SELECTOR_TAGS
        .iter()
        .map(|s| s.to_string())
        .chain(loud.iter().cloned())
        .collect();
    h.check(
        "bdd selector tags: the `has_tag` readers are exactly SELECTOR_TAGS + the loud `decision`",
        readers == expected,
    );
    if readers != expected {
        h.check(&format!("  (readers={readers:?} expected={expected:?})"), false);
    }

    // ── the guard's behaviour ──
    h.check(
        "bdd selector tags: a transposed @prodution is refused",
        has(&probs(&["prodution"], false), "did you mean @production"),
    );
    h.check(
        "bdd selector tags: a case typo @Production is refused",
        has(&probs(&["Production"], false), "did you mean @production"),
    );
    h.check(
        "bdd selector tags: the exact @production stays legal",
        probs(&["production"], false).is_empty(),
    );
    h.check(
        "bdd selector tags: a payload-tag near-miss is not re-framed as a selector",
        has(&probs(&["outptus(x)"], false), "did you mean @outputs"),
    );

    // ── a wrong refusal is worse than a missed typo: similar names stay legal ──
    h.check(
        "bdd selector tags: a merely similar name is not refused",
        probs(
            &[
                "inputs(seed)",
                "options(a, b)",
                "objects(x)",
                "expected_thing(y)",
                "output_dir(z)",
                "smoke",
                "prod",
                "decide",
                "example",
                "decision",
            ],
            false,
        )
        .is_empty(),
    );

    // ── end to end: the compiler refuses the near-miss, accepts the exact tag ──
    let mk = |tag: &str| {
        format!(
            "Feature: sel\n  {tag}\n  Scenario: s\n    When I run the command \"true\"\n    Then the exit code is 0\n"
        )
    };
    let bad = bdd::parse(&mk("@prodution"), "<sel-bad>").expect("parse @prodution");
    let e = bdd::compile_scenario(&bad, &bad.scenarios[0], &json!({}), "chrome-headless.sh")
        .err()
        .map(|e| e.to_string())
        .unwrap_or_default();
    h.check(
        "bdd selector tags: a @prodution scenario fails the compile",
        e.contains("did you mean @production"),
    );
    let ok = bdd::parse(&mk("@production"), "<sel-ok>").expect("parse @production");
    h.check(
        "bdd selector tags: a @production scenario still compiles",
        bdd::compile_scenario(&ok, &ok.scenarios[0], &json!({}), "chrome-headless.sh").is_ok(),
    );
}


pub fn test_bdd_selector_placement(h: &mut Harness) {
    // P172: a selector tag placed where its reader never looks selects nothing.
    // The production profile filters on *Scenario* tags only (`bdd build`/`bdd run`
    // both read `scenario.tags`), so a Feature-level `@production` is silently
    // inert - verified: it compiles locally at 100% coverage yet contributes zero
    // production-IT to a `--profile production` build that still exits 0. And a
    // `@decision` feature never runs as a browser integration test at all, so any
    // `@production` on it selects nothing either.
    let probs = |feature_tags: &[&str], all: &[&str], decision: bool| -> Vec<String> {
        let ft: Vec<String> = feature_tags.iter().map(|s| s.to_string()).collect();
        let at: Vec<String> = all.iter().map(|s| s.to_string()).collect();
        bdd::selector_placement_problems(&ft, &at, decision)
    };
    let has = |p: &[String], needle: &str| p.iter().any(|m| m.contains(needle));

    // ── the Feature line is not where the selector reads ──
    h.check(
        "bdd selector placement: @production on the Feature line is refused",
        has(
            &probs(&["production"], &["production"], false),
            "on the Feature line selects nothing",
        ),
    );
    // ── the Scenario line is exactly where the selector reads ──
    h.check(
        "bdd selector placement: @production on a Scenario stays legal",
        probs(&[], &["production"], false).is_empty(),
    );
    h.check(
        "bdd selector placement: a plain browser scenario is unaffected",
        probs(&[], &["smoke"], false).is_empty(),
    );
    h.check(
        "bdd selector placement: a non-selector feature tag is not refused",
        probs(&["decision"], &["decision"], false).is_empty(),
    );

    // ── a @decision feature never runs as a browser IT ──
    h.check(
        "bdd selector placement: @production on a @decision feature is refused",
        has(
            &probs(&[], &["production"], true),
            "selects nothing on a @decision feature",
        ),
    );
    h.check(
        "bdd selector placement: a payload-carrying @production on a @decision feature is refused",
        has(
            &probs(&["production(x)"], &["production(x)"], true),
            "selects nothing on a @decision feature",
        ),
    );
    h.check(
        "bdd selector placement: a @decision feature with no selector tag is unaffected",
        probs(&[], &["decision"], true).is_empty(),
    );

    // ── end to end: the compiler refuses the misplacement, accepts the good one ──
    let browser = |feature_tag: &str, scenario_tag: &str| {
        format!(
            "{feature_tag}Feature: place\n{scenario_tag}  Scenario: s\n    \
             When I run the command \"true\"\n    Then the exit code is 0\n"
        )
    };
    let bad = bdd::parse(&browser("@production\n", ""), "<place-feat>")
        .expect("parse feature-level @production");
    let e = bdd::compile_scenario(&bad, &bad.scenarios[0], &json!({}), "chrome-headless.sh")
        .err()
        .map(|e| e.to_string())
        .unwrap_or_default();
    h.check(
        "bdd selector placement: a Feature-level @production fails the compile",
        e.contains("on the Feature line selects nothing"),
    );
    let ok = bdd::parse(&browser("", "  @production\n"), "<place-scen>")
        .expect("parse scenario-level @production");
    h.check(
        "bdd selector placement: a Scenario-level @production still compiles",
        bdd::compile_scenario(&ok, &ok.scenarios[0], &json!({}), "chrome-headless.sh").is_ok(),
    );

    // A @decision feature that carries @production is refused before its graph is read.
    let decision_src = "@decision\n@production\nFeature: d\n\n  Scenario: only_node\n    \
         Given a choice question \"go\" asks \"Go?\"\n      | answer | meaning |\n      \
         | A      | yes     |\n    When any other answer goes to \"STOP\"\n";
    let df = bdd::parse(decision_src, "<place-decision>").expect("parse @decision + @production");
    let de = bdd::compile_decision(&df, &json!({}))
        .err()
        .map(|e| e.to_string())
        .unwrap_or_default();
    h.check(
        "bdd selector placement: a @decision feature carrying @production fails the compile",
        de.contains("selects nothing on a @decision feature"),
    );
}


pub fn test_bdd_policy_ceilings(h: &mut Harness) {
    // P173: a `policy` integer above its hard ceiling is silently clamped at load
    // (`capability::Registry::from_spec`) - `{"max_timeout_ms": 999999999}` runs at
    // 600000 and the validator prints that - so the budget that runs is not the one
    // written. P168 closed the *type*; this closes the *range*, on both the
    // compiler's `Given the policy` table and a hand-written / sidecar `policy`
    // block. The ceiling is read from the same const the clamp uses, so they cannot
    // drift apart.
    use laya_workflow::capability::{
        Registry, POLICY_MAX_OUTPUT, POLICY_MAX_RETRIES, POLICY_MAX_TIMEOUT_MS,
    };

    // ── the clamp itself: the guard's ceiling must be the runtime's ceiling ──
    let clamp = |pol: serde_json::Value| -> (u64, u64, u64) {
        let spec = json!({"name": "c", "start": "a", "nodes": [], "policy": pol});
        let reg = Registry::from_spec(&spec).expect("registry");
        let p = reg.policy();
        (p.max_timeout_ms, p.max_output as u64, p.retries as u64)
    };
    let (t, o, r) = clamp(json!({
        "max_timeout_ms": 9_999_999_999i64,
        "max_output": 9_999_999_999i64,
        "retries": 42
    }));
    h.eq(
        "bdd policy ceilings: the runtime clamps max_timeout_ms to the ceiling",
        t,
        POLICY_MAX_TIMEOUT_MS,
    );
    h.eq(
        "bdd policy ceilings: the runtime clamps max_output to the ceiling",
        o,
        POLICY_MAX_OUTPUT,
    );
    h.eq(
        "bdd policy ceilings: the runtime clamps retries to the ceiling",
        r,
        POLICY_MAX_RETRIES,
    );
    h.eq(
        "bdd policy ceilings: an in-range value is kept as written",
        clamp(json!({"max_timeout_ms": 8000, "max_output": 65536, "retries": 1})),
        (8000u64, 65536u64, 1u64),
    );

    // ── the compiler side: a `Given the policy` table cell above the ceiling ──
    let with_policy = |rows: &str| {
        format!(
            "@decision\nFeature: p173\n\n  Background:\n    Given the policy\n{rows}\n\
             \n  Scenario: a\n    Given a choice question \"q\" asks \"ok?\"\n      \
             | answer | meaning |\n      | A      | yes     |\n    \
             When any other answer goes to \"STOP\"\n"
        )
    };
    let policy_err = |rows: &str| -> String {
        match bdd::parse(&with_policy(rows), "<p173>")
            .and_then(|f| bdd::compile_decision(&f, &json!({})))
        {
            Ok(_) => String::new(),
            Err(e) => format!("{e:#}"),
        }
    };
    let m = policy_err(&format!("      | max_timeout_ms | {} |", POLICY_MAX_TIMEOUT_MS + 1));
    h.check(
        "bdd policy ceilings: a max_timeout_ms above the ceiling is refused by name",
        !m.is_empty() && m.contains("max_timeout_ms") && m.contains(&POLICY_MAX_TIMEOUT_MS.to_string()),
    );
    let m = policy_err(&format!("      | retries | {} |", POLICY_MAX_RETRIES + 1));
    h.check(
        "bdd policy ceilings: a retries above the ceiling is refused by name",
        !m.is_empty() && m.contains("retries") && m.contains(&POLICY_MAX_RETRIES.to_string()),
    );
    let m = policy_err(&format!("      | max_output | {} |", POLICY_MAX_OUTPUT + 1));
    h.check(
        "bdd policy ceilings: a max_output above the ceiling is refused",
        !m.is_empty() && m.contains("max_output"),
    );
    let m = policy_err("      | retries | -1 |");
    h.check(
        "bdd policy ceilings: a negative retries is refused (the engine never reads it)",
        !m.is_empty() && m.contains("retries") && m.contains("negative"),
    );
    h.check(
        "bdd policy ceilings: the ceiling itself is allowed",
        policy_err(&format!("      | max_timeout_ms | {} |", POLICY_MAX_TIMEOUT_MS)).is_empty(),
    );
    h.check(
        "bdd policy ceilings: an in-range row compiles as written",
        policy_err("      | retries | 3 |").is_empty(),
    );

    // ── the validate side: a hand-written / sidecar `policy` block ──
    let vk = |pol: serde_json::Value| -> String {
        let spec = json!({"name": "v", "start": "a", "nodes": [], "policy": pol});
        match laya_workflow::spec::validate_spec_keys(&spec) {
            Ok(()) => String::new(),
            Err(e) => format!("{e:#}"),
        }
    };
    let m = vk(json!({"max_timeout_ms": 9_999_999_999i64}));
    h.check(
        "bdd policy ceilings: spec max_timeout_ms above the ceiling is rejected by validate",
        !m.is_empty() && m.contains("max_timeout_ms") && m.contains(&POLICY_MAX_TIMEOUT_MS.to_string()),
    );
    let m = vk(json!({"retries": 42}));
    h.check(
        "bdd policy ceilings: spec retries above the ceiling is rejected by validate",
        !m.is_empty() && m.contains("retries"),
    );
    h.check(
        "bdd policy ceilings: an at-ceiling spec policy passes validate",
        vk(json!({"max_timeout_ms": 600000, "max_output": 67108864, "retries": 5})).is_empty(),
    );
    let m = vk(json!({"max_timeout_ms": "60000"}));
    h.check(
        "bdd policy ceilings: a wrong type still hits the type message, not the range one",
        !m.is_empty() && m.contains("non-negative integer"),
    );
}

pub fn test_bdd_call_with_ranges(h: &mut Harness) {
    // P175: P164 refused a `with.<key>` no kind reads, P166 the wrong *type*.
    // This is the *range*: the read site clamps the value, so a number outside
    // the band is replaced at the read site and the call runs with a different
    // one - `| with.result_count | 50 |` runs as 20, and `| with.min_score | 1.5 |`
    // (set to filter harder) runs as 1.0, which filters nothing. Both used to
    // compile with every gate green.
    use laya_workflow::capability::with_keys::{with_band_keys, with_range_rejected};

    let build = |text: &str| -> (bool, String) {
        let feat = bdd::parse(text, "<probe>").expect("probe parses");
        match bdd::compile_decision(&feat, &json!({})) {
            Ok(_) => (true, String::new()),
            Err(e) => (false, format!("{e:#}")),
        }
    };
    let head = r#"
@decision
Feature: wrg
  Background:
    Given the capabilities
      | clock | {"kind": "datetime", "op": "now"} |
      | page  | {"kind": "chrome_cdp", "endpoint": "http://127.0.0.1:9222"} |
      | plug  | {"kind": "plugin", "plugin": "websites/x"} |
  Scenario: root
    Given a choice question "go" asks "go?"
      | answer | meaning |
      | A | yes |
      | B | no |
    When any other answer goes to "STOP"
    Then call the capability "page"
"#;
    let rows = |extra: &str| format!("{head}      | with.op | research |\n{extra}");

    // `browser` clamps `result_count` to 1..=20: 50 runs as 20.
    let (ok, err) = build(&rows("      | with.result_count | 50 |\n"));
    h.check(
        "bdd call with-ranges: an above-band result_count is refused",
        !ok && err.contains("with.result_count") && err.contains("1..=20"),
    );
    h.check(
        "bdd call with-ranges: the refusal names the replaced value and the band",
        err.contains("as 50") && err.contains("silently replaced"),
    );
    // ... and 0 is raised to 1.
    let (ok, err) = build(&rows("      | with.result_count | 0 |\n"));
    h.check(
        "bdd call with-ranges: a below-band result_count is refused",
        !ok && err.contains("with.result_count"),
    );

    // `min_score` clamps to 0..=1 - a "stricter" 1.5 silently becomes 1.0.
    let (ok, err) = build(&rows("      | with.min_score | 1.5 |\n"));
    h.check(
        "bdd call with-ranges: a min_score above 1 is refused",
        !ok && err.contains("with.min_score") && err.contains("0..=1"),
    );

    // `tab_concurrency` clamps to 1..=10, and `concurrency` is its alias - the
    // clamp comes after the alias, so both names carry the band.
    let (ok, err) = build(&rows("      | with.tab_concurrency | 25 |\n"));
    h.check(
        "bdd call with-ranges: a tab_concurrency above 10 is refused",
        !ok && err.contains("with.tab_concurrency") && err.contains("1..=10"),
    );
    let (ok, err) = build(&rows("      | with.concurrency | 25 |\n"));
    h.check(
        "bdd call with-ranges: the `concurrency` alias carries the same band",
        !ok && err.contains("with.concurrency") && err.contains("1..=10"),
    );

    // `max_open` clamps against a literal 30 with a *computed* lower bound, so
    // only the top is a promise the reader can be held to.
    let (ok, err) = build(&rows("      | with.max_open | 100 |\n"));
    h.check(
        "bdd call with-ranges: a max_open above the literal cap is refused",
        !ok && err.contains("with.max_open") && err.contains("at most 30"),
    );

    // The band edges themselves are inside it.
    let (ok, err) = build(&rows("      | with.result_count | 20 |\n      | with.max_open | 30 |\n      | with.tab_concurrency | 1 |\n      | with.min_score | 1 |\n"));
    h.check("bdd call with-ranges: values on the band edges stay legal", ok);
    if !ok {
        h.check(&format!("  (unexpected: {err})"), false);
    }

    // `max_text` is clamped by `research` (500..=20000) but only capped by
    // `snapshot` (<=200000), so the op decides which band applies and the
    // compiler must not guess: a value legal for one op is refused by the other.
    let (ok, err) = build(&rows("      | with.max_text | 100 |\n"));
    h.check(
        "bdd call with-ranges: a key read at two places with different bands is not judged",
        ok,
    );
    if !ok {
        h.check(&format!("  (unexpected: {err})"), false);
    }

    // A `${state.x}` template has no compile-time value, and an open kind
    // (`plugin`) is never judged at all.
    let (ok, err) = build(&rows("      | with.result_count | ${state.n} |\n"));
    h.check("bdd call with-ranges: a `${state.x}` template is not judged", ok);
    if !ok {
        h.check(&format!("  (unexpected: {err})"), false);
    }
    let open = format!(
        "{head}      | with.op | research |\n"
    )
    .replace("Then call the capability \"page\"", "Then call the capability \"plug\"");
    let (ok, err) = build(&format!("{open}      | with.result_count | 50 |\n"));
    h.check("bdd call with-ranges: an open kind is not judged", ok);
    if !ok {
        h.check(&format!("  (unexpected: {err})"), false);
    }

    // A chain entry is judged against *its own* capability's kind.
    let (ok, err) = build(&format!(
        "{head}      | with.op | research |\n      | chain.cap.capability | page |\n      | chain.cap.with.result_count | 50 |\n"
    ));
    h.check(
        "bdd call with-ranges: a chain entry is judged by its own read site",
        !ok && err.contains("with.result_count") && err.contains("1..=20"),
    );

    // ── the bands are read off the sources, not a table ──
    // Pin the derived set: a `clamp` added, removed or re-bounded in a capability
    // module turns this red, and one read too many takes the key out of the set.
    let bands = with_band_keys("browser");
    let want: Vec<(String, String)> = [
        ("concurrency", "1..=10"),
        ("limit", "1..=1000"),
        ("max_open", "at most 30"),
        ("min_score", "0..=1"),
        ("result_count", "1..=20"),
        ("tab_concurrency", "1..=10"),
    ]
    .iter()
    .map(|(k, b)| (k.to_string(), b.to_string()))
    .collect();
    h.check(
        "bdd call with-ranges: the band of every judged key is read off the read site",
        bands == want,
    );
    h.check(
        "bdd call with-ranges: the alias folds to the same kind, an open kind is empty",
        with_band_keys("chrome_cdp") == want && with_band_keys("plugin").is_empty(),
    );
    h.check(
        "bdd call with-ranges: an unclamped key and an open kind are never judged",
        with_range_rejected("browser", "result_count", &json!(50)).is_some()
            && with_range_rejected("browser", "result_count", &json!(20)).is_none()
            && with_range_rejected("browser", "max_text", &json!(100)).is_none()
            && with_range_rejected("browser", "query", &json!(5)).is_none()
            && with_range_rejected("plugin", "result_count", &json!(50)).is_none(),
    );
    h.check(
        "bdd call with-ranges: a non-number is the type guard's business, not this one",
        with_range_rejected("browser", "result_count", &json!("50")).is_none(),
    );
}


/// The args gate's verdict assembly, driven as a pure function. The real gate
/// spawns `laya-workflow run --spec` once per row of `bench/bdd/args_probes.json`
/// - which the tests binary cannot do for itself, because its `current_exe` is
/// the test runner and not a CLI - so the rows are handed in with synthetic
/// outcomes and the verdicts are checked in row order. P188 made those rows run
/// in parallel; this is what keeps a dropped, duplicated or reordered verdict
/// from passing unnoticed.
pub fn test_bdd_args_probe_verdicts(h: &mut Harness) {
    use laya_workflow::bdd::args_probe_verdicts;
    use std::path::PathBuf;

    let job = |name: &str| -> (String, String, String, PathBuf) {
        (
            name.to_string(),
            format!("{name} blurb"),
            format!("refused the {name} call"),
            PathBuf::from(format!("/tmp/{name}.json")),
        )
    };
    let jobs = vec![job("a"), job("b"), job("c"), job("d")];
    let outcomes = vec![
        // a: a cold refusal that says the right thing.
        Some((1, "Error: refused the a call\n".to_string())),
        // b: exits zero - the plugin was supposed to refuse.
        Some((0, String::new())),
        // c: fails, but not with the documented message.
        Some((1, "Error: something else\n".to_string())),
        // d: never came back.
        None,
    ];
    let (ran, problems) = args_probe_verdicts(&jobs, &outcomes);
    // `ran` counts every row that came back at all - the gate's own wording is
    // "{ran}/{rows} ... refuse", and it reads 16/16 only because every checked-in
    // row does refuse. A timeout is the one outcome that is not counted.
    h.eq(
        "bdd args-probe-verdicts: every row that returned is counted, a timeout is not",
        ran,
        3usize,
    );
    h.eq(
        "bdd args-probe-verdicts: one problem per bad row, none for the good one",
        problems.len(),
        3usize,
    );
    h.check(
        "bdd args-probe-verdicts: the exit-0 row is named",
        problems[0].contains("row \"b\""),
    );
    h.check(
        "bdd args-probe-verdicts: the wrong-message row is named with what it said",
        problems[1].contains("row \"c\"") && problems[1].contains("something else"),
    );
    h.check(
        "bdd args-probe-verdicts: the timeout row is named",
        problems[2].contains("row \"d\"") && problems[2].contains("timed out"),
    );
}
