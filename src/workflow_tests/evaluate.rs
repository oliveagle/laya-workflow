//! Evaluate / labeled-dataset sections for `laya-workflow-tests`.
//! Port of awesome-jev `evaluations/run.py` (labeled JSONL, dev/holdout,
//! review queue, confusion matrix, config freeze fingerprint).
#![allow(unused_imports)]
use crate::*;

const DEPARTMENTS: [&str; 4] = ["billing", "technical", "account", "other"];

fn tmpfile(name: &str) -> String {
    std::env::temp_dir()
        .join(format!(
            "laya-eval-{}-{name}.jsonl",
            std::process::id()
        ))
        .to_string_lossy()
        .into_owned()
}

fn write_dataset(path: &str, body: &str) {
    std::fs::write(path, body).unwrap();
}

/// A labelled case with the common defaults, so each rejection test only
/// varies the field it is actually about.
fn case_json(id: &str, extra: &[(&str, &str)]) -> String {
    let mut v = serde_json::json!({
        "id": id,
        "split": "development",
        "message": "some ticket",
        "expected_department": "technical",
        "expected_urgency": "high",
    });
    if let Some(o) = v.as_object_mut() {
        for (k, val) in extra {
            o.insert(k.to_string(), serde_json::Value::String(val.to_string()));
        }
    }
    v.to_string()
}

pub fn test_evaluate(h: &mut Harness) {
    use laya_workflow::evaluate::{self, Case};

    // ── load_dataset: the happy path ─────────────────────────────────────
    let p = tmpfile("load");
    write_dataset(
        &p,
        &format!(
            "{}\n{}\n",
            case_json("t-1", &[("slice", "clear")]),
            case_json("t-2", &[("split", "holdout"), ("require_review_flag", "x")]),
        )
        .replace(",\"require_review_flag\":\"x\"", ""),
    );
    let cases = evaluate::load_dataset(&p, &DEPARTMENTS).unwrap();
    h.eq("eval: load_dataset count", cases.len(), 2usize);
    h.eq("eval: first split", cases[0].split.clone(), "development".to_string());
    h.eq("eval: second split", cases[1].split.clone(), "holdout".to_string());
    h.eq("eval: first slice", cases[0].slice.clone(), "clear".to_string());
    // slice is optional → default "unlabelled"
    h.eq("eval: missing slice defaults", cases[1].slice.clone(), "unlabelled".to_string());
    // require_review is optional → default false
    h.eq("eval: require_review defaults false", cases[1].require_review, false);
    let _ = std::fs::remove_file(&p);

    // ── load_dataset rejections (each mirrors awesome-jev's hard errors) ──
    let expect_reject = |h: &mut Harness, name: &str, body: &str, needle: &str| {
        let p = tmpfile(name);
        write_dataset(&p, body);
        let err = evaluate::load_dataset(&p, &DEPARTMENTS).unwrap_err().to_string();
        h.check(&format!("eval: {name} rejected"), err.contains(needle));
        let _ = std::fs::remove_file(&p);
    };

    expect_reject(
        h, "duplicate-id",
        &format!("{}\n{}\n", case_json("x", &[]), case_json("x", &[])),
        "duplicate id",
    );
    expect_reject(
        h, "unknown-department",
        &format!("{}\n", case_json("x", &[("expected_department", "zzz")])),
        "unknown department",
    );
    expect_reject(
        h, "invalid-urgency",
        &format!("{}\n", case_json("x", &[("expected_urgency", "critical")])),
        "invalid urgency",
    );
    expect_reject(
        h, "invalid-split",
        &format!("{}\n", case_json("x", &[("split", "dev")])),
        "invalid split",
    );
    expect_reject(
        h, "empty-id",
        &format!("{}\n", case_json("", &[])),
        "id must be a nonempty string",
    );
    expect_reject(h, "empty-dataset", "\n\n", "no cases");
    expect_reject(h, "bad-json", "{not json\n", "invalid JSON");

    // require_review must be boolean (a string is a hard error)
    let p = tmpfile("rr");
    let bad = r#"{"id":"x","split":"development","message":"m","expected_department":"billing","expected_urgency":"high","require_review":"yes"}"#;
    write_dataset(&p, bad);
    let err = evaluate::load_dataset(&p, &DEPARTMENTS).unwrap_err().to_string();
    h.check("eval: non-bool require_review rejected", err.contains("require_review"));
    let _ = std::fs::remove_file(&p);

    // ── evaluate() + build_summary(): scripted closure, known counts ─────
    // 4 cases: 2 correct auto, 1 auto but wrong department (unsafe), 1 human_review.
    let scripted = [
        ("a", "development", "clear", "technical", "high", false, "technical", "high"),
        ("b", "development", "clear", "technical", "high", false, "technical", "review"),
        ("c", "holdout",  "clear", "billing",  "ordinary", false, "technical", "high"), // wrong
        ("d", "holdout",  "ambiguous", "other", "ordinary", true, "human_review", "review"),
    ];
    let cases: Vec<Case> = scripted
        .iter()
        .map(|(id, split, slice, dept, urg, require_review, _, _)| Case {
            id: id.to_string(),
            split: split.to_string(),
            slice: slice.to_string(),
            message: format!("ticket {id}"),
            expected_department: dept.to_string(),
            expected_urgency: urg.to_string(),
            require_review: *require_review,
        })
        .collect();

    let mut answers = scripted.iter();
    let summary = evaluate::evaluate(&cases, &DEPARTMENTS, |_| {
        let (_, _, _, _, _, _, route, urg) = answers.next().expect("one closure call per case");
        let mut result = serde_json::Map::new();
        result.insert("route".into(), serde_json::json!(route));
        result.insert("urgency".into(), serde_json::json!(urg));
        result.insert("confidence".into(), serde_json::json!(0.9));
        Ok(serde_json::json!({ "result": serde_json::Value::Object(result) }))
    })
    .unwrap();

    h.eq("eval: summary total", summary["counts"]["total"].as_u64(), Some(4));
    h.eq("eval: automatic", summary["counts"]["automatic"].as_u64(), Some(3));
    h.eq("eval: review", summary["counts"]["review"].as_u64(), Some(1));
    // department correct: a, b, d(route review != other). Expected a technical, b technical, c billing, d other
    // routes: a technical (correct), b technical (correct), c technical (wrong vs billing), d human_review (wrong vs other)
    h.eq("eval: department correct count", summary["department_accuracy"]["numerator"].as_u64(), Some(2));
    h.eq("eval: department denominator", summary["department_accuracy"]["denominator"].as_u64(), Some(4));
    h.check("eval: department rate 0.5",
        (summary["department_accuracy"]["value"].as_f64().unwrap_or(-1.0) - 0.5).abs() < 1e-9);
    h.eq("eval: review_rate numerator", summary["review_rate"]["numerator"].as_u64(), Some(1));
    h.eq("eval: review_rate denominator", summary["review_rate"]["denominator"].as_u64(), Some(4));
    h.eq("eval: automatic_coverage 3/4", summary["automatic_coverage"]["numerator"].as_u64(), Some(3));
    // urgency decisions: a high(high ✓), b review(expected high ✗ but not counted as decided),
    //                    c high(expected ordinary ✗), d review.
    // urgency_decided = urgency != "review" → a, c (2); correct → a only (1)
    h.eq("eval: urgency decided denom", summary["urgency_accuracy"]["denominator"].as_u64(), Some(2));
    h.eq("eval: urgency correct num", summary["urgency_accuracy"]["numerator"].as_u64(), Some(1));
    // required_review cases: d; it routed to review → missed = 0/1
    h.eq("eval: required_review_missed num", summary["required_review_missed"]["numerator"].as_u64(), Some(0));
    h.eq("eval: required_review_missed denom", summary["required_review_missed"]["denominator"].as_u64(), Some(1));
    // high urgency expected: a, b, c → downgraded to ordinary: 0
    h.eq("eval: high_urgency_downgraded", summary["high_urgency_downgraded"]["numerator"].as_u64(), Some(0));
    h.eq("eval: high_urgency_downgraded denom", summary["high_urgency_downgraded"]["denominator"].as_u64(), Some(2));
    // review queue = cases that routed to human_review OR unsafe auto (c wrong + a/b automatic correct)
    // c: unsafe_automatic = true (wrong dept, not review); d: needs_review
    let queue = summary["review_queue"].as_array().unwrap();
    h.eq("eval: review queue length", queue.len(), 2usize);
    let mut queued_ids: Vec<&str> = queue.iter().filter_map(|r| r["id"].as_str()).collect();
    queued_ids.sort_unstable();
    h.eq("eval: queue ids", queued_ids, vec!["c", "d"]);
    // confusion matrix: expected -> observed route
    h.eq("eval: confusion technical->technical", summary["confusion"]["technical"]["technical"].as_u64(), Some(2));
    h.eq("eval: confusion billing->technical", summary["confusion"]["billing"]["technical"].as_u64(), Some(1));
    h.eq("eval: confusion other->human_review", summary["confusion"]["other"]["human_review"].as_u64(), Some(1));
    // slices: "ambiguous" and "clear"
    let slices = summary["slices"].as_array().unwrap();
    h.eq("eval: slice count", slices.len(), 2usize);
    // splits: development and holdout
    let splits = summary["splits"].as_array().unwrap();
    h.eq("eval: split count", splits.len(), 2usize);

    // ── evaluate() with a routed-to-review first case (empty denominator) ─
    // unsafe_automatic over the *automatic* slice; review-only closure works too.
    let all_review_cases: Vec<Case> = vec![Case {
        id: "r1".into(),
        split: "development".into(),
        slice: "s".into(),
        message: "m".into(),
        expected_department: "other".into(),
        expected_urgency: "ordinary".into(),
        require_review: false,
    }];
    let s2 = evaluate::evaluate(&all_review_cases, &DEPARTMENTS, |_| {
        Ok(serde_json::json!({ "result": {"route": "human_review", "urgency": "review"} }))
    })
    .unwrap();
    h.eq("eval: all-review automatic", s2["counts"]["automatic"].as_u64(), Some(0));
    // automatic_coverage = 0/1; unsafe_automatic denom = 0 → value null
    h.check("eval: unsafe_automatic null on empty denom",
        s2["unsafe_automatic"]["value"].is_null());

    // ── config_sha256: stable, 64 hex, changes on mutation ───────────────
    let spec_text = std::fs::read_to_string(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/dsl/capabilities/support_routing.json"
    ))
    .unwrap();
    let spec: Value = serde_json::from_str(&spec_text).unwrap();
    let fp1 = evaluate::config_sha256(&spec).unwrap();
    let fp2 = evaluate::config_sha256(&spec).unwrap();
    h.eq("eval: sha256 stable", fp1.clone(), fp2);
    h.eq("eval: sha256 len", fp1.len(), 64usize);
    h.check("eval: sha256 hex", fp1.chars().all(|c| c.is_ascii_hexdigit()));
    let mut changed = spec.clone();
    changed["name"] = serde_json::json!("tampered");
    let fp3 = evaluate::config_sha256(&changed).unwrap();
    h.check("eval: sha256 changes on spec edit", fp3 != fp1);

    // ── end-to-end: the real support_routing spec + the real dataset ─────
    let dataset_path =
        concat!(env!("CARGO_MANIFEST_DIR"), "/bench/evaluations/support_routing.jsonl");
    let e2e_cases = evaluate::load_dataset(dataset_path, &DEPARTMENTS).unwrap();
    h.eq("eval: fixture dataset count", e2e_cases.len(), 6usize);
    h.eq("eval: fixture development count",
        e2e_cases.iter().filter(|c| c.split == "development").count(), 3);
    h.eq("eval: fixture holdout count",
        e2e_cases.iter().filter(|c| c.split == "holdout").count(), 3);

    let wf = laya_workflow::spec::load_file(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/dsl/capabilities/support_routing.json"
    ))
    .unwrap();
    let backend = laya_workflow::backend::HeuristicBackend;
    let e2e = evaluate::evaluate(&e2e_cases, &DEPARTMENTS, |message| {
        let mut b = laya_workflow::backend::HeuristicBackend;
        let _ = &backend;
        let mut state = serde_json::json!({ "message": message });
        let out = wf.run(&mut b, &mut state).unwrap();
        Ok(out.to_json())
    })
    .unwrap();

    h.eq("eval: e2e total", e2e["counts"]["total"].as_u64(), Some(6));
    // t-5 "How do I do a thing?" has no match_rules hit → fallback "other"
    // low confidence → human_review → must be in the review queue.
    let e2e_queue = e2e["review_queue"].as_array().unwrap();
    h.check("eval: e2e t-5 queued for review",
        e2e_queue.iter().any(|r| r["id"].as_str() == Some("t-5") && r["needs_review"] == true));
    // the other 5 are clear-cut → department accuracy 5/6 (t-5 expected other
    // but observed human_review, which is the correct safe behaviour).
    h.eq("eval: e2e department correct", e2e["department_accuracy"]["numerator"].as_u64(), Some(5));
    h.eq("eval: e2e department denominator", e2e["department_accuracy"]["denominator"].as_u64(), Some(6));
    h.check("eval: e2e unsafe_automatic is 0",
        e2e["unsafe_automatic"]["numerator"].as_u64().unwrap_or(9) == 0);
}
