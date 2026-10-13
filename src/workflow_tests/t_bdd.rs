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
    let single = bdd::scaffold_body("intake_triage", "routing", "option", "one node", 1);
    let f1 = bdd::parse(&single, "<scaffold>").expect("single parses");
    let s1 = bdd::compile_decision(&f1, &json!({})).expect("single compiles");
    h.eq("bdd: scaffold --single node count", s1["nodes"].as_array().unwrap().len(), 1);
    let _ = crate::spec::from_spec_with_dir(&s1, None, None).expect("single validates");
    // CLI stdout mode is self-verifying and returns 0.
    let rc = bdd::scaffold(&bdd::ScaffoldOptions {
        name: "intake_triage".into(),
        cat: "routing".into(),
        intent: None,
        stdout: true,
        force: false,
        primary_q: String::new(),
        single: false,
    })
    .expect("scaffold CLI runs");
    h.eq("bdd: scaffold stdout rc", rc, 0);
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
    let _ = std::fs::remove_dir_all(&root);
}

/// The behavioural-smoke data: every expected label must correspond to a
/// registered sample state, and the number of labelled expectation sets must
/// not silently regress below the pinned floor (35). This guards the
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
    h.check("bdd: smoke expect stems >= 35", expect_obj.len() >= 35);
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
