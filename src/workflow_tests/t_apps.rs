//! apps regression sections for `laya-workflow-tests`.
#![allow(unused_imports)]
use crate::*;
pub fn test_apps_rule_logic_with_scripted_verdicts(h: &mut Harness) {
    let v = verdict(&[
        ("is_destructive", choice("A", &[("A", 0.86), ("B", 0.14)])),
        ("is_exfiltration", choice("A", &[("A", 0.9), ("B", 0.1)])),
        ("intent_match", choice("A", &[("A", 0.97), ("B", 0.03)])),
        ("risk", score(0.2, &[0.8, 0.15, 0.05])),
    ]);
    h.eq("agent_gate: ls -> ALLOW", apps::agent_gate::route("ls -la /tmp", &v)["action"].as_str().unwrap().to_string(), "ALLOW".to_string());
    let v_rm = verdict(&[
        ("is_destructive", choice("B", &[("A", 0.04), ("B", 0.96)])),
        ("is_exfiltration", choice("A", &[("A", 0.9), ("B", 0.1)])),
        ("intent_match", choice("A", &[("A", 0.9), ("B", 0.1)])),
        ("risk", score(2.2, &[0.02, 0.08, 0.9])),
    ]);
    h.eq("agent_gate: rm -rf -> BLOCK (verb rule)", apps::agent_gate::route("rm -rf ./build", &v_rm)["action"].as_str().unwrap().to_string(), "BLOCK".to_string());
    // a non-listed verb with high destructive prob must still BLOCK
    h.eq("agent_gate: unknown verb high p -> BLOCK", apps::agent_gate::route("mytool --purge", &v_rm)["action"].as_str().unwrap().to_string(), "BLOCK".to_string());

    let v_phish = verdict(&[
        ("category", choice("account", &[("billing", 0.2), ("account", 0.5), ("other", 0.3)])),
        ("spam", choice("B", &[("A", 0.4), ("B", 0.6)])),
        ("phishing", choice("B", &[("A", 0.05), ("B", 0.95)])),
        ("urgency", score(2.0, &[0.05, 0.15, 0.8])),
        ("needs_reply", choice("B", &[("A", 0.3), ("B", 0.7)])),
        ("sentiment", score(1.0, &[0.2, 0.4, 0.3, 0.1])),
    ]);
    h.eq("email: phishing -> QUARANTINE", apps::email_triage::route(&v_phish)["action"].as_str().unwrap().to_string(), "QUARANTINE".to_string());

    let v_bill = verdict(&[
        ("category", choice("billing", &[("billing", 0.7), ("other", 0.3)])),
        ("spam", choice("A", &[("A", 0.9), ("B", 0.1)])),
        ("phishing", choice("A", &[("A", 0.95), ("B", 0.05)])),
        ("urgency", score(2.4, &[0.05, 0.15, 0.8])),
        ("needs_reply", choice("B", &[("A", 0.2), ("B", 0.8)])),
        ("sentiment", score(1.2, &[0.2, 0.4, 0.3, 0.1])),
    ]);
    h.eq("email: billing urgent -> PAGER_BILLING", apps::email_triage::route(&v_bill)["action"].as_str().unwrap().to_string(), "PAGER_BILLING".to_string());
    h.eq("email: _clean strips quote", apps::email_triage::clean("Hi\n> quoted\nEnd"), "Hi\nEnd".to_string());

    let v_hate = verdict(&[
        ("is_hate", choice("B", &[("A", 0.05), ("B", 0.95)])),
        ("is_threat", choice("A", &[("A", 0.8), ("B", 0.2)])),
        ("is_pii", choice("A", &[("A", 0.9), ("B", 0.1)])),
        ("is_spam", choice("A", &[("A", 0.9), ("B", 0.1)])),
        ("severity", score(2.0, &[0.05, 0.15, 0.8])),
        ("category", choice("hateful", &[("safe", 0.05), ("hateful", 0.9), ("other", 0.05)])),
    ]);
    h.eq("moderation: hate -> BLOCK", apps::content_moderation::route(&v_hate)["action"].as_str().unwrap().to_string(), "BLOCK".to_string());

    let v_ok = verdict(&[
        ("is_hate", choice("A", &[("A", 0.97), ("B", 0.03)])),
        ("is_threat", choice("A", &[("A", 0.97), ("B", 0.03)])),
        ("is_pii", choice("A", &[("A", 0.97), ("B", 0.03)])),
        ("is_spam", choice("A", &[("A", 0.97), ("B", 0.03)])),
        ("severity", score(0.1, &[0.9, 0.08, 0.02])),
        ("category", choice("safe", &[("safe", 0.95), ("off_topic", 0.05)])),
    ]);
    h.eq("moderation: clean -> APPROVE", apps::content_moderation::route(&v_ok)["action"].as_str().unwrap().to_string(), "APPROVE".to_string());

    let v_typo = verdict(&[
        ("tone", score(2.0, &[0.05, 0.1, 0.6, 0.25])),
        ("clarity", score(1.2, &[0.1, 0.6, 0.2, 0.1])),
        ("professionalism", score(2.0, &[0.05, 0.15, 0.7, 0.1])),
        ("has_typo", choice("B", &[("A", 0.3), ("B", 0.7)])),
        ("is_sensitive", choice("A", &[("A", 0.8), ("B", 0.2)])),
        ("send_now", choice("polish", &[("send_now", 0.2), ("polish", 0.5), ("sleep", 0.2), ("rewrite", 0.1)])),
    ]);
    h.eq("draft: typo+low clarity -> polish", apps::draft_scorer::route(&v_typo)["suggestion"].as_str().unwrap().to_string(), "polish".to_string());
    let v_sens = verdict(&[
        ("tone", score(0.5, &[0.6, 0.3, 0.08, 0.02])),
        ("clarity", score(2.0, &[0.05, 0.1, 0.6, 0.25])),
        ("professionalism", score(2.0, &[0.05, 0.15, 0.7, 0.1])),
        ("has_typo", choice("A", &[("A", 0.8), ("B", 0.2)])),
        ("is_sensitive", choice("B", &[("A", 0.2), ("B", 0.8)])),
        ("send_now", choice("send_now", &[("send_now", 0.6), ("polish", 0.2), ("sleep", 0.15), ("rewrite", 0.05)])),
    ]);
    h.eq("draft: sensitive+angry -> sleep", apps::draft_scorer::route(&v_sens)["suggestion"].as_str().unwrap().to_string(), "sleep".to_string());

    // noul helper sanity (used by callers that map boolean questions)
    let d = noul(0.83);
    h.eq("noul p(true)", d.prob("true"), 0.83);
    h.eq("noul p(false)", d.prob("false"), 1.0 - 0.83);
    h.check("noul answer is float", d.answer.as_f64().unwrap() == 0.83);

    // ── app workflows are wired ─────────────────────────────────────
}


pub fn test_app_workflows(h: &mut Harness) {
    for (name, wf) in [
        ("agent_gate", apps::agent_gate::workflow()),
        ("email_triage", apps::email_triage::workflow()),
        ("content_moderation", apps::content_moderation::workflow()),
        ("draft_scorer", apps::draft_scorer::workflow()),
        ("gateway", apps::gateway_workflow()),
    ] {
        let d = wf.describe();
        h.check(&format!("{name}: has nodes"), d["nodes"].as_object().map(|o| !o.is_empty()).unwrap_or(false));
        h.check(&format!("{name}: has start"), d["start"].is_string());
    }

    // mock-driven end-to-end on the gateway graph
    let be = ScriptedBackend::new(vec![verdict(&[
        ("is_destructive", choice("B", &[("A", 0.04), ("B", 0.96)])),
        ("is_exfiltration", choice("A", &[("A", 0.9), ("B", 0.1)])),
        ("intent_match", choice("A", &[("A", 0.95), ("B", 0.05)])),
        ("risk", score(2.0, &[0.02, 0.08, 0.9])),
    ])]);
    let out = apps::gateway_workflow().run(&be, &json!({"command": "rm -rf ./build", "intent": "clean"})).unwrap();
    h.eq("gateway: unsafe -> BLOCK payload merged", out.state["action"].as_str().unwrap().to_string(), "BLOCK".to_string());

    // ── optimizer loops ─────────────────────────────────────────────
}


pub fn test_optimizer(h: &mut Harness) {

    // build_laya_state mirrors the Python transformer
    let optim = json!({
        "task": "outlier_detect",
        "score": -3.0,
        "best_score": -2.5,
        "step": 4,
        "strategy": "aggressive_step",
        "params": {"k": 3, "z": 1.5},
        "history": [
            {"step": 3, "trial_score": -2.8, "accepted": true},
            {"step": 4, "trial_score": -3.0, "accepted": false},
        ],
    });
    let ls = optimizer::build_laya_state(&optim);
    h.eq("optimizer: task passthrough", ls["task"].as_str().unwrap().to_string(), "outlier_detect".to_string());
    h.eq("optimizer: history_len", ls["history_len"].as_u64().unwrap(), 2);
    h.eq("optimizer: recent_scores len", ls["recent_scores"].as_array().unwrap().len(), 2);
    h.eq("optimizer: consecutive_rejects", ls["consecutive_rejects"].as_u64().unwrap(), 1);
    h.check("optimizer: params_summary bounded", ls["params_summary"].as_str().unwrap().len() <= 200);

    // parse_proposal: JSON extraction + fallback
    let p = optimizer::parse_proposal("blah {\"delta_params\": {\"k\": 5}, \"rationale\": \"r\"} tail");
    h.eq("optimizer: parse delta", p["delta_params"]["k"].as_u64().unwrap(), 5);
    let p2 = optimizer::parse_proposal("no json here at all");
    h.check("optimizer: parse fallback empty delta", p2["delta_params"].as_object().unwrap().is_empty());

    // workflow graph wiring
    let wf = optimizer::make_optimizer_workflow(&["a".into(), "b".into()], 20);
    let d = wf.describe();
    h.eq("optimizer: workflow start", d["start"].as_str().unwrap().to_string(), "continue_check".to_string());
    h.check("optimizer: has continue_check", d["nodes"]["continue_check"].is_object());
    h.check("optimizer: has strategy_check", d["nodes"]["strategy_check"].is_object());

    // LayaOptimizerLoop: Laya says "stop" on the first check -> 1 step then stop
    let mut loop_ = optimizer::LayaOptimizerLoop::new(vec!["s1".into(), "s2".into()], true, 10);
    let stop_be = ScriptedBackend::new(vec![
        verdict(&[("should_continue", choice("stop", &[("stop", 0.9), ("continue", 0.1)]))]),
    ]);
    let propose = |_p: &str| "{\"delta_params\": {\"k\": 2}}".to_string();
    let eval = |params: &Value| {
        let k = params.get("k").and_then(|v| v.as_f64()).unwrap_or(1.0);
        Ok(-(k - 5.0) * (k - 5.0)) // maximum 0 at k = 5
    };
    let r = loop_.run(&stop_be, &propose, &eval).unwrap();
    h.eq("optloop: stopped after 1 step", r["steps"].as_u64().unwrap(), 1);
    h.eq("optloop: final action stop", r["trace"]["final_action"].as_str().unwrap().to_string(), "stop".to_string());
    h.eq("optloop: best params applied", r["best_params"]["k"].as_u64().unwrap(), 2);

    // LayaOptimizerLoop: always continue -> climbs toward k=5
    let mut loop2 = optimizer::LayaOptimizerLoop::new(vec!["s1".into()], true, 3);
    let go_be = ScriptedBackend::new(vec![
        verdict(&[("should_continue", choice("continue", &[("stop", 0.1), ("continue", 0.9)]))]),
    ]);
    let propose2 = || "{\"delta_params\": {\"k\": 5}}".to_string();
    let r2 = loop2.run(&go_be, &|_p| propose2(), &eval).unwrap();
    h.check("optloop: improved to optimum", r2["best_score"].as_f64().unwrap() == 0.0);
    h.eq("optloop: best k = 5", r2["best_params"]["k"].as_u64().unwrap(), 5);
    h.check("optloop: has history", r2["history"].as_array().map(|a| !a.is_empty()).unwrap_or(false));

    // LayaOptimizerLoop: goal judged by maximize/minimize direction (minimize)
    let mut loop3 = optimizer::LayaOptimizerLoop::new(vec!["s1".into()], false, 1);
    let go_be3 = ScriptedBackend::new(vec![
        verdict(&[("should_continue", choice("continue", &[("stop", 0.1), ("continue", 0.9)]))]),
    ]);
    let eval_min = |params: &Value| {
        let k = params.get("k").and_then(|v| v.as_f64()).unwrap_or(0.0);
        Ok((k - 1.0).abs())
    };
    let r3 = loop3.run(&go_be3, &|_p| "{\"delta_params\": {\"k\": 1}}".to_string(), &eval_min).unwrap();
    h.check("optloop: minimize reaches 0", r3["best_score"].as_f64().unwrap() == 0.0);

    // ResilientLoop: stop decision honoured + checkpoint written
    let dir = format!("{}/laya_wf_loop_test", std::env::temp_dir().display());
    let _ = std::fs::remove_dir_all(&dir);
    let rl = optimizer::ResilientLoop::new("t1", &dir);
    let stop_be = ScriptedBackend::new(vec![
        verdict(&[("should_continue", choice("stop", &[("stop", 0.95), ("continue", 0.05)]))]),
    ]);
    let step = |s: &Value| {
        let n = s.get("n").and_then(|v| v.as_u64()).unwrap_or(0) + 1;
        Ok(json!({"n": n, "score": -(n as f64)}))
    };
    let out = rl.run(&stop_be, &json!({"n": 0, "score": 0.0}), step, 10).unwrap();
    h.eq("resilient loop: stopped at 1", out["iterations"].as_u64().unwrap(), 1);
    h.check("resilient loop: checkpoint exists", std::path::Path::new(&dir).join("t1_checkpoint.json").exists());
    h.check("resilient loop: decisions recorded", out["decisions"].as_array().map(|a| !a.is_empty()).unwrap_or(false));
    let out2 = optimizer::ResilientLoop::new("t1", &dir)
        .run(&ScriptedBackend::new(vec![verdict(&[("should_continue", choice("continue", &[("stop", 0.05), ("continue", 0.95)]))])]),
             &json!({"n": 0, "score": 0.0}), step, 2)
        .unwrap();
    h.eq("resilient loop: resume continues from checkpoint", out2["iterations"].as_u64().unwrap(), 2);
    let _ = std::fs::remove_dir_all(&dir);

    // ── generic JSON spec ───────────────────────────────────────────
}

