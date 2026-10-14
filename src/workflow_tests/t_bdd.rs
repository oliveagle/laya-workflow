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
    for pat in ["gate", "loop"] {
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
    h.check("bdd: score corpus tasks >= 13", files.len() >= 13);
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
    h.check("bdd: decision score tasks >= 10", decision >= 10);
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
    h.eq("bdd: score --run covers all decision tasks", tasks.len(), 10);
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

/// `bdd port-check`: the cross-repo conversion is closed. The pure
/// symmetric-difference rules are pinned here, plus one real IO round-trip
/// against the checked-in `ole_eval` ports using a throwaway source tree.
pub fn test_bdd_port_check(h: &mut Harness) {
    use laya_workflow::bdd::{diff_port_sets, port_check, PortCheckOptions};
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
    };
    h.check(
        "bdd port-check: a missing source dir is an error, not a pass",
        port_check(&bad).is_err(),
    );

    let _ = std::fs::remove_dir_all(&src);
    let _ = std::fs::remove_file(&rep);
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
        let declared = declared_outputs(&raw);
        let opaque = has_opaque_outputs(&raw);
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
            let src = label_source(&raw, result, fa);
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
        for (label, st) in smap {
            // (1) the offline heuristic walk, capturing each node's answer set.
            let mut heur: Vec<(String, Vec<(String, Value, Value, f64)>)> = Vec::new();
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
                heur.push((r.node_name.clone(), ans));
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
            //      legitimate and skipped.
            for (node_name, ans) in &heur {
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
                    if accepted.is_empty() {
                        continue;
                    }
                    let Some(chosen) = a.as_str() else { continue };
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
            }
            // (2)+(3) the contract must be exactly what the node asked, and the
            // script is rebuilt from it alone.
            let mut script: Vec<Value> = Vec::new();
            let mut ok = true;
            // The node-keyed policy: one entry per visited node, plus a note of any
            // node whose entry differs between visits (inexpressible as a policy).
            let mut per_node: serde_json::Map<String, Value> = serde_json::Map::new();
            let mut keyed_conflict_nodes: Vec<String> = Vec::new();
            for (node, ans) in &heur {
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
            let hnodes: Vec<&String> = heur.iter().map(|(n, _)| n).collect();
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
