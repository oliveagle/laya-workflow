//! rules tests (ported from abide): rubric schema, band/probability, scope globs, runner,
//! events, and the hook output shapes. Offline only — no model needed.

use super::Harness;
use serde_json::json;
use std::collections::BTreeMap;

use laya_workflow::rules as ab;

fn tmpdir(tag: &str) -> std::path::PathBuf {
    let d = std::env::temp_dir().join(format!(
        "laya-rules-test-{}-{}",
        tag,
        std::process::id()
    ));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).unwrap();
    d
}

fn model_rule(id: &str, when: ab::RuleWhen, q: ab::Question, scope: Option<Vec<String>>) -> ab::Rule {
    ab::Rule {
        id: id.to_string(),
        text: format!("rule {id}"),
        source: ab::RuleSource {
            path: "AGENTS.md".to_string(),
            line: Some(3),
        },
        scope,
        when: Some(when),
        check: ab::Check::Model {
            question: q,
            overlaps: None,
            heuristic: None,
        },
        status: ab::RuleStatus::Active,
    }
}

fn boolean_q(heuristic: Option<ab::Heuristic>) -> ab::Question {
    ab::Question::Boolean {
        instructions: "does this edit break the rule?".to_string(),
        criteria: Some(json!({"true": "yes", "false": "no"})),
        heuristic,
    }
}

fn choice_q() -> ab::Question {
    let mut criteria = BTreeMap::new();
    criteria.insert("ok".to_string(), "fine".to_string());
    criteria.insert("bad".to_string(), "violates".to_string());
    ab::Question::Choice {
        instructions: "which option applies?".to_string(),
        criteria,
        violating: vec!["bad".to_string()],
        heuristic: None,
    }
}

fn score_q() -> ab::Question {
    ab::Question::Score {
        instructions: "rate it".to_string(),
        criteria: vec!["clean".to_string(), "ok".to_string(), "bad".to_string()],
        violating_from: 2,
        heuristic: None,
    }
}

pub fn test_rules_schema(h: &mut Harness) {
    // valid rubric round-trips (camelCase JSON field names)
    let rubric = ab::Rubric {
        version: 1,
        compiled_at: "2026-10-08T00:00:00Z".to_string(),
        compiled_by: Some("test".to_string()),
        sources: vec![ab::RubricSource {
            path: "AGENTS.md".to_string(),
            sha: Some("a".repeat(64)),
            scope: None,
        }],
        thresholds: None,
        rules: vec![model_rule("no-stdout", ab::RuleWhen::Edit, boolean_q(None), None)],
    };
    let json = serde_json::to_value(&rubric).unwrap();
    h.check("rubric serializes compiledAt", json.get("compiledAt").is_some());
    h.check(
        "rubric serializes compiledBy",
        json.get("compiledBy").is_some(),
    );
    let back: ab::Rubric = serde_json::from_value(json).unwrap();
    h.eq("rubric round-trips", back.rules.len(), 1usize);

    // score violatingFrom camelCase
    let sq = serde_json::to_value(score_q()).unwrap();
    h.check(
        "score question serializes violatingFrom",
        sq.get("violatingFrom").is_some(),
    );
    let back_q: ab::Question = serde_json::from_value(sq).unwrap();
    h.check("score question round-trips", matches!(back_q, ab::Question::Score { .. }));

    // invalid rubrics
    let mut dup = rubric.clone();
    dup.rules.push(model_rule("no-stdout", ab::RuleWhen::Edit, boolean_q(None), None));
    h.check("duplicate id rejected", !ab::validate_rubric(&dup).is_empty());

    // model rule without when
    let mut no_when = rubric.clone();
    no_when.rules[0].when = None;
    h.check("model rule without when rejected", !ab::validate_rubric(&no_when).is_empty());

    // choice with violating not in criteria
    let mut cq = choice_q();
    if let ab::Question::Choice { violating, .. } = &mut cq {
        violating.push("ghost".to_string());
    }
    let mut bad_choice = rubric.clone();
    bad_choice.rules[0].check = ab::Check::Model {
        question: cq,
        overlaps: None,
        heuristic: None,
    };
    h.check(
        "choice violating outside criteria rejected",
        !ab::validate_rubric(&bad_choice).is_empty(),
    );
}

pub fn test_rules_probability_and_band(h: &mut Harness) {
    // boolean
    let q = boolean_q(None);
    let (p, a) = ab::violation_probability(
        &q,
        &ab::ModelAnswer::Boolean { probability: 0.9 },
    );
    h.eq("boolean p maps through", (p, a), (0.9, None));
    let (p, _) = ab::violation_probability(&q, &ab::ModelAnswer::Boolean { probability: 1.3 });
    h.eq("boolean p clamps high", p, 1.0);
    let (p, _) = ab::violation_probability(&q, &ab::ModelAnswer::Boolean { probability: -0.2 });
    h.eq("boolean p clamps low", p, 0.0);

    // choice with probabilities
    let q = choice_q();
    let mut probs = BTreeMap::new();
    probs.insert("ok".to_string(), 0.2f64);
    probs.insert("bad".to_string(), 0.8f64);
    let (p, a) = ab::violation_probability(
        &q,
        &ab::ModelAnswer::Choice {
            choice: "bad".to_string(),
            probabilities: Some(probs.clone()),
        },
    );
    h.eq("choice sums violating mass", (p, a.as_deref()), (0.8, Some("bad")));
    // choice without probabilities: picked option decides
    let (p, _) = ab::violation_probability(
        &q,
        &ab::ModelAnswer::Choice {
            choice: "bad".to_string(),
            probabilities: None,
        },
    );
    h.eq("choice picked violating -> 1", p, 1.0);
    let (p, _) = ab::violation_probability(
        &q,
        &ab::ModelAnswer::Choice {
            choice: "ok".to_string(),
            probabilities: None,
        },
    );
    h.eq("choice picked compliant -> 0", p, 0.0);

    // score
    let q = score_q();
    let mut probs = BTreeMap::new();
    probs.insert("0".to_string(), 0.1f64);
    probs.insert("1".to_string(), 0.1f64);
    probs.insert("2".to_string(), 0.8f64);
    let (p, a) = ab::violation_probability(
        &q,
        &ab::ModelAnswer::Score {
            score: 2.0,
            probabilities: Some(probs),
        },
    );
    h.eq("score sums levels >= violatingFrom", (p, a.as_deref()), (0.8, Some("bad")));
    let (p, _) = ab::violation_probability(
        &q,
        &ab::ModelAnswer::Score {
            score: 1.0,
            probabilities: None,
        },
    );
    h.eq("score below violatingFrom -> 0", p, 0.0);
    let (p, _) = ab::violation_probability(
        &q,
        &ab::ModelAnswer::Score {
            score: 2.0,
            probabilities: None,
        },
    );
    h.eq("score at violatingFrom -> 1", p, 1.0);

    // bands
    let t = ab::DEFAULT_THRESHOLDS;
    h.eq("band act", ab::band_for(0.8, t), ab::Band::Act);
    h.eq("band flag", ab::band_for(0.5, t), ab::Band::Flag);
    h.eq("band clear", ab::band_for(0.49, t), ab::Band::Clear);
}

pub fn test_rules_glob(h: &mut Harness) {
    h.check("glob star matches same dir", ab::glob_match("*.rs", "a.rs"));
    h.check("glob star no slash", !ab::glob_match("*.rs", "src/a.rs"));
    h.check(
        "glob dstar crosses dirs",
        ab::glob_match("src/**/*.rs", "src/a/b.rs"),
    );
    h.check(
        "glob dstar-slash zero segs",
        ab::glob_match("src/**/*.rs", "src/a.rs"),
    );
    h.check(
        "glob scope every file",
        ab::glob_match("**", ".env"),
    );
    h.check("glob dotfile", ab::glob_match(".env*", ".env"));
    h.check("glob question", ab::glob_match("a?.rs", "ab.rs"));
    h.check("glob question not slash", !ab::glob_match("a?.rs", "a/b.rs"));
    // rule_applies_to
    let rule = model_rule(
        "scoped",
        ab::RuleWhen::Edit,
        boolean_q(None),
        Some(vec!["src/**/*.rs".to_string()]),
    );
    h.check("rule applies in scope", ab::rule_applies_to(&rule, "src/a/b.rs"));
    h.check("rule no apply outside", !ab::rule_applies_to(&rule, "docs/a.md"));
    let open = model_rule("open", ab::RuleWhen::Edit, boolean_q(None), None);
    h.check("rule without scope applies everywhere", ab::rule_applies_to(&open, "any/file"));
}

pub fn test_rules_runner(h: &mut Harness) {
    // select_rules: phase + scope
    let edit_rule = model_rule("edit-rule", ab::RuleWhen::Edit, boolean_q(None), None);
    let turn_rule = model_rule("turn-rule", ab::RuleWhen::Turn, boolean_q(None), None);
    let disabled = {
        let mut r = model_rule("disabled", ab::RuleWhen::Edit, boolean_q(None), None);
        r.status = ab::RuleStatus::Disabled;
        r
    };
    let lint_rule = ab::Rule {
        id: "linty".to_string(),
        text: "lint me".to_string(),
        source: ab::RuleSource { path: "AGENTS.md".to_string(), line: None },
        scope: None,
        when: Some(ab::RuleWhen::Edit),
        check: ab::Check::Lint { how: Some("fmt".to_string()), pattern: None, overlaps: None },
        status: ab::RuleStatus::Active,
    };
    let rules = vec![edit_rule.clone(), turn_rule.clone(), disabled, lint_rule];
    let selected = ab::select_rules(&rules, ab::RuleWhen::Edit, &["src/a.rs".to_string()]);
    h.eq("select edit rules only", selected.len(), 1usize);
    h.eq("selected is the edit rule", selected[0].id.clone(), "edit-rule".to_string());
    let selected_turn = ab::select_rules(&rules, ab::RuleWhen::Turn, &["src/a.rs".to_string()]);
    h.eq("select turn rule", selected_turn.len(), 1usize);
    h.eq("turn selected id", selected_turn[0].id.clone(), "turn-rule".to_string());

    // group_by_scope
    let scoped_a = model_rule(
        "a-only",
        ab::RuleWhen::Edit,
        boolean_q(None),
        Some(vec!["src/**".to_string()]),
    );
    let scoped_b = model_rule(
        "b-only",
        ab::RuleWhen::Edit,
        boolean_q(None),
        Some(vec!["docs/**".to_string()]),
    );
    let open_rule = model_rule("open", ab::RuleWhen::Edit, boolean_q(None), None);
    let diffs = vec![
        ab::FileDiff { file: "src/a.rs".to_string(), text: "x".to_string() },
        ab::FileDiff { file: "docs/b.md".to_string(), text: "y".to_string() },
    ];
    let all = vec![&scoped_a, &scoped_b, &open_rule];
    let groups = ab::group_by_scope(&all, &diffs);
    // three groups: {src}, {docs}, and the open rule covers BOTH files so it
    // gets its own group (rules groups by the exact in-scope file set)
    h.eq("three scope groups", groups.len(), 3usize);
    let has_both = groups
        .iter()
        .any(|g| g.file_diffs.len() == 2 && g.rules.iter().any(|r| r.id == "open"));
    h.check("open rule has its own both-files group", has_both);
    let g0_files: Vec<String> = groups[0].file_diffs.iter().map(|f| f.file.clone()).collect();
    let g1_files: Vec<String> = groups[1].file_diffs.iter().map(|f| f.file.clone()).collect();
    h.check(
        "group0 has src/a.rs",
        g0_files.iter().any(|f| f == "src/a.rs"),
    );
    h.check(
        "group1 has docs/b.md",
        g1_files.iter().any(|f| f == "docs/b.md"),
    );
}

pub fn test_rules_check_offline(h: &mut Harness) {
    // Offline check: heuristic rule matches a bad diff, clears a good one.
    let rule = model_rule(
        "no-println",
        ab::RuleWhen::Edit,
        boolean_q(Some(ab::Heuristic {
            match_any: Some(vec!["println!".to_string()]),
            p_violated: Some(0.95),
            p_ok: Some(0.05),
        })),
        None,
    );
    let request = ab::CheckRequest {
        phase: ab::RuleWhen::Edit,
        file_diffs: vec![ab::FileDiff {
            file: "src/worker.rs".to_string(),
            text: "--- a/src/worker.rs\n+++ b/src/worker.rs\n+    println!(\"hi\");\n".to_string(),
        }],
        task: Some("add logging".to_string()),
        rules: vec![rule],
        thresholds: ab::DEFAULT_THRESHOLDS,
        include_heuristic: true,
    };
    let backend = ab::RulesHeuristicBackend;
    let out = ab::run_check(&request, &backend).unwrap();
    h.eq("bad diff verdict count", out.verdicts.len(), 1usize);
    h.eq(
        "bad diff verdict band act",
        out.verdicts[0].band,
        ab::Band::Act,
    );
    h.check("bad diff prob ~0.95", (out.verdicts[0].probability - 0.95).abs() < 1e-9);

    // good diff
    let good = ab::CheckRequest {
        file_diffs: vec![ab::FileDiff {
            file: "src/worker.rs".to_string(),
            text: "--- a/src/worker.rs\n+++ b/src/worker.rs\n+    let x = 1;\n".to_string(),
        }],
        ..request.clone()
    };
    let out = ab::run_check(&good, &backend).unwrap();
    h.eq("good diff verdict clear", out.verdicts[0].band, ab::Band::Clear);

    // scope filtering: rule scoped to src/** does not see docs
    let scoped = model_rule(
        "no-println",
        ab::RuleWhen::Edit,
        boolean_q(Some(ab::Heuristic {
            match_any: Some(vec!["println!".to_string()]),
            p_violated: Some(0.95),
            p_ok: Some(0.05),
        })),
        Some(vec!["src/**".to_string()]),
    );
    let docs = ab::CheckRequest {
        file_diffs: vec![ab::FileDiff {
            file: "docs/x.md".to_string(),
            text: "println! here\n".to_string(),
        }],
        rules: vec![scoped.clone()],
        ..request.clone()
    };
    let out = ab::run_check(&docs, &backend).unwrap();
    h.eq("out-of-scope yields no verdicts", out.verdicts.len(), 0usize);
}

pub fn test_rules_hook_output(h: &mut Harness) {
    let rule = model_rule("r1", ab::RuleWhen::Edit, boolean_q(None), None);
    let v_act = ab::Verdict {
        rule_id: "r1".to_string(),
        probability: 0.95,
        band: ab::Band::Act,
        answer: None,
        file: Some("src/a.rs".to_string()),
    };
    let v_flag = ab::Verdict {
        rule_id: "r1".to_string(),
        probability: 0.6,
        band: ab::Band::Flag,
        answer: None,
        file: Some("src/a.rs".to_string()),
    };
    let files = vec!["src/a.rs".to_string()];
    let out = ab::hook_output(ab::RuleWhen::Edit, &[rule.clone()], &[v_act.clone()], &files);
    h.check("act -> block", matches!(out, ab::HookOutput::Block { .. }));
    let out = ab::hook_output(ab::RuleWhen::Edit, &[rule.clone()], &[v_flag.clone()], &files);
    h.check("flag -> notice", matches!(out, ab::HookOutput::Notice { .. }));
    let out = ab::hook_output(
        ab::RuleWhen::Edit,
        &[rule.clone()],
        &[ab::Verdict {
            band: ab::Band::Clear,
            probability: 0.1,
            ..v_act.clone()
        }],
        &files,
    );
    h.check("clear -> silent", matches!(out, ab::HookOutput::Silent));
    // deleted file cannot be repaired -> act downgrades to a flag notice
    let out = ab::hook_output(ab::RuleWhen::Edit, &[rule], &[v_act], &[]);
    h.check(
        "act on deleted file -> notice",
        matches!(out, ab::HookOutput::Notice { .. }),
    );

    // repair_reason mentions the rule id, source and file
    let rule = model_rule("no-stdout", ab::RuleWhen::Edit, boolean_q(None), None);
    let v = ab::Verdict {
        rule_id: "no-stdout".to_string(),
        probability: 0.9,
        band: ab::Band::Act,
        answer: None,
        file: Some("src/a.rs".to_string()),
    };
    let reason = ab::repair_reason(ab::RuleWhen::Edit, &[(&rule, v)], &files);
    h.check("reason names rule", reason.contains("no-stdout"));
    h.check("reason names file", reason.contains("src/a.rs"));
    h.check("reason asks repair", reason.contains("Repair"));
}

pub fn test_rules_events(h: &mut Harness) {
    // events live under the process-wide LAYA_HOME; isolate so this test does
    // not accumulate into the real ~/.laya-workflow across runs.
    let saved_laya_home = std::env::var_os("LAYA_HOME");
    let laya_home = tmpdir("events-laya-home");
    std::env::set_var("LAYA_HOME", &laya_home);

    let root = tmpdir("events");
    ab::cmd_init(&root).unwrap();
    h.check("init creates rubric", root.join(".rules/rubric.json").exists());
    h.check("init creates rulesignore", root.join(".rules/.rulesignore").exists());
    let read = ab::read_rubric(&ab::rubric_path(&root));
    h.check("init rubric is valid", matches!(read, ab::RubricRead::Ok { .. }));

    let ev = ab::RulesEvent::Check {
        at: "t".to_string(),
        phase: ab::RuleWhen::Edit,
        session_id: None,
        prompt_id: None,
        files: vec!["src/a.rs".to_string()],
        rules: 1,
        latency_ms: 2,
        model_latency_ms: Some(1),
        usage: Some(ab::Usage {
            input_tokens: Some(10),
            output_tokens: None,
            cost_usd: None,
        }),
        verdicts: vec![ab::Verdict {
            rule_id: "r1".to_string(),
            probability: 0.95,
            band: ab::Band::Act,
            answer: None,
            file: Some("src/a.rs".to_string()),
        }],
        blocked: true,
    };
    ab::append_event(&root, &ev);
    let events = ab::read_events(&root);
    h.eq("event round-trips", events.len(), 1usize);
    let report = ab::cmd_report(&root).unwrap();
    h.eq("report checks", report["checks"].as_u64(), Some(1));
    h.eq("report blocks", report["blocks"].as_u64(), Some(1));
    h.eq("report per-rule act", report["by_rule"]["r1"]["act"].as_u64(), Some(1));

    // restore LAYA_HOME
    match saved_laya_home {
        Some(v) => std::env::set_var("LAYA_HOME", v),
        None => std::env::remove_var("LAYA_HOME"),
    }
}

pub fn test_rules_compile_prompt(h: &mut Harness) {
    let root = tmpdir("prompt");
    std::fs::write(root.join("AGENTS.md"), "# Rules\n- never print to stdout\n").unwrap();
    let prompt = ab::compile_prompt(&root);
    h.check("prompt names AGENTS.md", prompt.contains("AGENTS.md"));
    h.check("prompt carries rule text", prompt.contains("never print to stdout"));
    h.check("prompt mentions rubric.json", prompt.contains("rubric.json"));
    h.check("prompt mentions heuristic", prompt.contains("heuristic"));
}
