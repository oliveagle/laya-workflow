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

pub fn test_rules_heuristic_added_lines(h: &mut Harness) {
    // (1) diff splitter: file headers dropped, `+`/`-` bodies kept verbatim.
    let (added, removed) = ab::split_diff_added_removed(
        "diff --git a/f b/f\nindex 111..222 100644\n--- a/f\n+++ b/f\n@@ -1,2 +1,2 @@\n ctx\n-old1\n+new1\n+new2\n",
    );
    h.eq("split: added bodies", added, "new1\nnew2\n".to_string());
    h.eq("split: removed bodies", removed, "old1\n".to_string());

    // (2) an added line that merely begins with `+` is still content.
    let (added, _) = ab::split_diff_added_removed("+++ b/f\n+++x;\n");
    h.eq("split: `+++` w/o space is content", added, "++x;\n".to_string());

    // (3) state walk: `diff` -> added/removed buckets; other strings -> added.
    let state = json!({
        "file": "AGENTS.md",
        "task": "update the index row",
        "diff": "+++ b/AGENTS.md\n@@ -1 +1 @@\n-k /mnt/data\n+k /mnt/data v2\n"
    });
    let (added, removed) = ab::heuristic_diff_text(&state);
    h.check("walk: task text in added", added.contains("update the index row"));
    h.check("walk: file path in added", added.contains("AGENTS.md"));
    h.check("walk: kept token in added", added.contains("k /mnt/data v2"));
    h.check("walk: same token in removed", removed.contains("k /mnt/data"));

    let rule = model_rule(
        "no-mnt",
        ab::RuleWhen::Edit,
        boolean_q(Some(ab::Heuristic {
            match_any: Some(vec!["/mnt/".to_string()]),
            p_violated: Some(0.95),
            p_ok: Some(0.05),
        })),
        None,
    );
    let backend = ab::RulesHeuristicBackend;
    let judge = |text: &str| {
        let request = ab::CheckRequest {
            phase: ab::RuleWhen::Edit,
            file_diffs: vec![ab::FileDiff { file: "AGENTS.md".to_string(), text: text.to_string() }],
            task: None,
            rules: vec![rule.clone()],
            thresholds: ab::DEFAULT_THRESHOLDS,
            include_heuristic: true,
        };
        ab::run_check(&request, &backend).unwrap().verdicts[0].band
    };

    // (4) a brand-new line introducing `/mnt/` is a violation.
    h.eq(
        "new private path -> act",
        judge("+++ b/AGENTS.md\n@@ -1 +1,2 @@\n keep\n+see /mnt/data/foo\n"),
        ab::Band::Act,
    );
    // (5) touching a line that *already* carried `/mnt/` (it appears on both the
    //     removed and the added side) is grandfathered, not a new leak.
    h.eq(
        "rewriting an existing private path -> clear",
        judge("+++ b/AGENTS.md\n@@ -1 +1 @@\n-row 2026-09-19 /mnt/data\n+row 2026-09-20 /mnt/data\n"),
        ab::Band::Clear,
    );
    // (6) deleting a line that carries `/mnt/` is a cleanup, never a violation.
    h.eq(
        "deleting a private path -> clear",
        judge("+++ b/AGENTS.md\n@@ -1,2 +1 @@\n-see /mnt/data/foo\n keep\n"),
        ab::Band::Clear,
    );
    // (7) a token seen only in context (unchanged) lines is not a hit.
    h.eq(
        "context-only private path -> clear",
        judge("+++ b/AGENTS.md\n@@ -1,3 +1,3 @@\n line /mnt/data here\n-old\n+new\n"),
        ab::Band::Clear,
    );
    // (8) suppression is per token: a genuinely new `/mnt/` still fires even
    //     when some *other* line (a different token) was removed in the same diff.
    h.eq(
        "new token among removed token -> act",
        judge("+++ b/AGENTS.md\n@@ -1,2 +1,2 @@\n-old /root/x\n+see /mnt/data for logs\n"),
        ab::Band::Act,
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

pub fn test_rules_check_diff_json_file(h: &mut Harness) {
    // `rules check` used to take the whole diff only as an argv (`--diff-json`).
    // A single argument over MAX_ARG_STRLEN (128 KiB) fails with E2BIG *before*
    // the binary runs, and a pre-commit hook swallowing that failure silently
    // leaves a commit ungated. `--diff-json-file <PATH>` (or `-` for stdin)
    // removes the ceiling; this test proves the three routes agree and that a
    // >128 KiB payload survives the file route.
    let saved_laya_home = std::env::var_os("LAYA_HOME");
    let laya_home = tmpdir("check-file-laya-home");
    std::env::set_var("LAYA_HOME", &laya_home);

    let root = tmpdir("check-file");
    ab::cmd_init(&root).unwrap();
    // The staged file must exist under the root: an act verdict on a file that
    // is not there downgrades to a notice (there is nothing to repair).
    std::fs::write(root.join("AGENTS.md"), "hello\n").unwrap();
    let rubric = ab::Rubric {
        version: 1,
        compiled_at: "2026-10-16T00:00:00Z".to_string(),
        compiled_by: Some("test".to_string()),
        sources: vec![ab::RubricSource {
            path: "AGENTS.md".to_string(),
            sha: Some("a".repeat(64)),
            scope: None,
        }],
        thresholds: None,
        rules: vec![model_rule(
            "no-mnt",
            ab::RuleWhen::Edit,
            boolean_q(Some(ab::Heuristic {
                match_any: Some(vec!["/mnt/".to_string()]),
                p_violated: Some(0.95),
                p_ok: Some(0.05),
            })),
            None,
        )],
    };
    ab::write_rubric(&ab::rubric_path(&root), &rubric).unwrap();

    let mut bin = std::env::current_exe().expect("test binary path");
    bin.pop();
    if bin.ends_with("deps") {
        bin.pop();
    }
    bin.push("laya-workflow");
    let run = |args: &[&str], stdin: Option<&str>| {
        let mut cmd = std::process::Command::new(&bin);
        cmd.args(args);
        cmd.stdin(if stdin.is_some() {
            std::process::Stdio::piped()
        } else {
            std::process::Stdio::null()
        });
        cmd.stdout(std::process::Stdio::piped());
        cmd.stderr(std::process::Stdio::piped());
        let mut child = cmd.spawn().expect("spawn laya-workflow");
        if let Some(text) = stdin {
            use std::io::Write;
            child
                .stdin
                .take()
                .unwrap()
                .write_all(text.as_bytes())
                .unwrap();
        }
        let out = child.wait_with_output().unwrap();
        let stdout = String::from_utf8_lossy(&out.stdout).to_string();
        let kind = serde_json::from_str::<serde_json::Value>(&stdout)
            .ok()
            .and_then(|v| v.get("kind").and_then(|k| k.as_str()).map(String::from))
            .unwrap_or_default();
        (out.status.success(), kind)
    };

    // A diff that introduces a fresh `/mnt/` line must block on every route.
    let bad = json!([{"file": "AGENTS.md", "text": "+++ b/AGENTS.md\n@@ -1 +1,2 @@\n keep\n+see /mnt/data for logs\n"}]);
    let root_s = root.to_string_lossy().to_string();
    let (ok_argv, kind_argv) = run(
        &["rules", "check", "--root", &root_s, "--diff-json", &bad.to_string(), "--phase", "edit"],
        None,
    );
    h.check("check: --diff-json runs", ok_argv);
    h.eq("check: --diff-json blocks", kind_argv.clone(), "block".to_string());

    let jf = laya_home.join("diff.json");
    std::fs::write(&jf, bad.to_string()).unwrap();
    let jf_s = jf.to_string_lossy().to_string();
    let (ok_file, kind_file) = run(
        &["rules", "check", "--root", &root_s, "--diff-json-file", &jf_s, "--phase", "edit"],
        None,
    );
    h.check("check: --diff-json-file runs", ok_file);
    h.eq("check: --diff-json-file blocks like argv", kind_file, kind_argv.clone());

    let (ok_stdin, kind_stdin) = run(
        &["rules", "check", "--root", &root_s, "--diff-json-file", "-", "--phase", "edit"],
        Some(&bad.to_string()),
    );
    h.check("check: stdin '-' runs", ok_stdin);
    h.eq("check: stdin '-' blocks like argv", kind_stdin, kind_argv.clone());

    // A benign edit passes on the file route.
    let good = json!([{"file": "AGENTS.md", "text": "+++ b/AGENTS.md\n@@ -1 +1,2 @@\n keep\n+plain text\n"}]);
    std::fs::write(&jf, good.to_string()).unwrap();
    let (ok_good, kind_good) = run(
        &["rules", "check", "--root", &root_s, "--diff-json-file", &jf_s, "--phase", "edit"],
        None,
    );
    h.check("check: benign file diff runs", ok_good);
    h.eq("check: benign file diff silent", kind_good, "silent".to_string());

    // The point of the flag: a payload well past MAX_ARG_STRLEN (128 KiB) is
    // handed over intact. (The same payload as an argv would E2BIG *before*
    // exec, so it cannot be expressed as a `--diff-json` comparison here.)
    let big_line = "y".repeat(200 * 1024);
    let big = json!([{"file": "AGENTS.md", "text": format!("+++ b/AGENTS.md\n@@ -1 +1,2 @@\n{big_line}\n+plain text\n")}]);
    std::fs::write(&jf, big.to_string()).unwrap();
    h.check("check: 200 KiB payload is written", std::fs::metadata(&jf).map(|m| m.len() > 200 * 1024).unwrap_or(false));
    let (ok_big, kind_big) = run(
        &["rules", "check", "--root", &root_s, "--diff-json-file", &jf_s, "--phase", "edit"],
        None,
    );
    h.check("check: 200 KiB payload runs via file", ok_big);
    h.eq("check: 200 KiB payload silent", kind_big, "silent".to_string());

    match saved_laya_home {
        Some(v) => std::env::set_var("LAYA_HOME", v),
        None => std::env::remove_var("LAYA_HOME"),
    }
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
