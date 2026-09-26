//! spec regression sections for `laya-workflow-tests`.
#![allow(unused_imports)]
use crate::*;
pub fn test_spec(h: &mut Harness) {
    let spec = serde_json::json!({
        "name": "ticket_router",
        "start": "classify",
        "max_iterations": 7,
        "convergence_window": 3,
        "convergence_eps": 0.001,
        "nodes": [
            {
                "name": "classify",
                "primary_q": "is_urgent",
                "questions": {"is_urgent": {"type": "choice", "instructions": "urgent?",
                                            "criteria": {"A": "no", "B": "yes"}}},
                "edge": {"condition": {"B": "escalate", "A": "STOP"}, "default": "STOP"},
                "min_confidence": 0.0,
                "max_retries": 1,
                "action": {"kind": "gate", "question": "is_urgent", "option": "B", "allow_at": 0.8, "block_at": 0.8}
            },
            {
                "name": "escalate",
                "primary_q": "severity",
                "questions": {"severity": {"type": "score", "instructions": "how bad?",
                                           "criteria": ["minor", "degraded", "outage"]}},
                "edge": {"condition": {}, "default": "STOP"},
                "action": {"kind": "threshold", "question": "severity",
                           "rules": [{"when": {"value_gte": 1.5}, "label": "PAGE_ONCALL"},
                                     {"when": {"always": true}, "label": "QUEUE_HIGH"}]}
            }
        ]
    });
    let wf = spec::from_spec(&spec).unwrap();
    h.eq("spec: start", wf.start.clone(), "classify".to_string());
    h.eq("spec: max_iterations", wf.max_iterations, 7);
    h.eq("spec: convergence_window", wf.convergence_window, 3);
    h.eq("spec: node count", wf.nodes.len(), 2);
    h.eq("spec: max_retries parsed", wf.nodes["classify"].max_retries, 1);
    h.eq("spec: has action closure", wf.nodes["classify"].action_fn.is_some(), true);

    // routing + gate action from data
    let urgent = ScriptedBackend::new(vec![
        verdict(&[("is_urgent", choice("B", &[("A", 0.05), ("B", 0.95)]))]),
        verdict(&[("severity", score(2.0, &[0.05, 0.15, 0.8]))]),
    ]);
    let out = wf.run(&urgent, &json!({"text": "prod down"})).unwrap();
    h.eq("spec: routed to escalate then stop", out.final_action().to_string(), "stop".to_string());
    h.eq("spec: two steps", out.trace.steps.len(), 2);
    h.eq("spec: gate action BLOCK (fail-closed)", out.state["gate_action"].as_str().unwrap().to_string(), "BLOCK".to_string());
    h.eq("spec: threshold label PAGE_ONCALL", out.state["label"].as_str().unwrap().to_string(), "PAGE_ONCALL".to_string());

    // non-urgent path stops immediately
    let calm = ScriptedBackend::new(vec![
        verdict(&[("is_urgent", choice("A", &[("A", 0.9), ("B", 0.1)]))]),
    ]);
    let out = wf.run(&calm, &json!({"text": "office closed Monday"})).unwrap();
    h.eq("spec: calm path one step", out.trace.steps.len(), 1);
    h.eq("spec: calm path ALLOW", out.state["gate_action"].as_str().unwrap().to_string(), "ALLOW".to_string());

    // threshold fallback + copy_keys + merge_open_probs + state projection
    let v1 = verdict(&[
        ("is_urgent", choice("A", &[("A", 0.9), ("B", 0.1)])),
        ("severity", score(0.4, &[0.7, 0.2, 0.1])),
    ]);
    let th = spec::run_action(
        &json!({"kind": "threshold", "question": "severity",
                "rules": [{"when": {"value_gte": 1.5}, "label": "PAGE"}]}),
        &json!({}), &v1, None).unwrap();
    h.eq("spec: threshold unmatched", th["label"].as_str().unwrap().to_string(), "unmatched".to_string());
    let ck = spec::run_action(&json!({"kind": "copy_keys", "keys": ["severity"]}), &json!({}), &v1, None).unwrap();
    h.check("spec: copy_keys picks answer", ck["severity"].as_f64().unwrap() == 0.4);
    let mp = spec::run_action(&json!({"kind": "merge_open_probs", "question": "is_urgent"}), &json!({}), &v1, None).unwrap();
    h.eq("spec: merge_open_probs question", mp["action_question"].as_str().unwrap().to_string(), "is_urgent".to_string());
    h.check("spec: merge_open_probs spread", (mp["spread"].as_f64().unwrap() - 0.8).abs() < 1e-6);
    h.eq("spec: unknown kind rejected",
         spec::run_action(&json!({"kind": "bogus"}), &json!({}), &v1, None).is_err(), true);

    // state projection (keep/rename) + to_spec marks rust actions
    let proj_spec = json!({
        "name": "p", "start": "n",
        "nodes": [{"name": "n", "questions": {"q": {"type": "choice", "instructions": "?",
                                                    "criteria": {"A": "a", "B": "b"}}},
                   "edge": {"default": "STOP"},
                   "state": {"keep": ["text"], "rename": {"cmd": "command"}}}]
    });
    let pwf = spec::from_spec(&proj_spec).unwrap();
    let seen = ScriptedBackend::new(vec![verdict(&[("q", choice("A", &[("A", 0.9), ("B", 0.1)]))])]);
    let outp = pwf.run(&seen, &json!({"text": "hi", "command": "ls", "junk": 1})).unwrap();
    h.check("spec: projection ran", outp.iterations == 1);
    let exported = spec::to_spec(&apps::agent_gate::workflow(), "agent_gate");
    h.eq("spec: export marks rust action",
         exported["nodes"][0]["action"]["kind"].as_str().unwrap().to_string(), "rust".to_string());
    h.check("spec: export has questions", exported["nodes"][0]["questions"]["is_destructive"].is_object());

    // DSL loop: self-loop edge -> retry until budget, then escalate
    let loop_spec = json!({
        "name": "refine", "start": "review", "max_iterations": 20,
        "nodes": [{
            "name": "review",
            "primary_q": "verdict",
            "questions": {"verdict": {"type": "choice", "instructions": "accept or revise?",
                                      "criteria": {"accept": "ok", "revise": "again"}},
                          "quality": {"type": "score", "instructions": "how good?",
                                      "criteria": ["poor", "usable", "ready"]}},
            "edge": {"condition": {"accept": "STOP", "revise": "review"}, "default": "STOP"},
            "max_retries": 2,
            "action": {"kind": "merge_open_probs", "question": "verdict"}
        }]
    });
    let lwf = spec::from_spec(&loop_spec).unwrap();
    let always_revise = ScriptedBackend::new(vec![verdict(&[
        ("verdict", choice("revise", &[("accept", 0.2), ("revise", 0.8)])),
        ("quality", score(0.5, &[0.3, 0.5, 0.2])),
    ])]);
    let lout = lwf.run(&always_revise, &json!({"draft": "x", "round": 1})).unwrap();
    h.eq("spec-loop: retry budget escalates", lout.final_action().to_string(), "escalate".to_string());
    h.check("spec-loop: visited more than once", lout.iterations >= 2);
    h.check("spec-loop: merge payload present on escalate", lout.state["merged"].as_bool().unwrap_or(false));

    // DSL loop: accept on the 3rd visit -> STOP with 3 steps
    let revise_then_accept = ScriptedBackend::new(vec![
        verdict(&[("verdict", choice("revise", &[("accept", 0.2), ("revise", 0.8)])),
                  ("quality", score(0.5, &[0.3, 0.5, 0.2]))]),
        verdict(&[("verdict", choice("revise", &[("accept", 0.3), ("revise", 0.7)])),
                  ("quality", score(0.6, &[0.2, 0.5, 0.3]))]),
        verdict(&[("verdict", choice("accept", &[("accept", 0.9), ("revise", 0.1)])),
                  ("quality", score(0.9, &[0.05, 0.15, 0.8]))]),
    ]);
    let mut loop_spec3 = loop_spec.clone();
    loop_spec3["nodes"][0]["max_retries"] = json!(5);
    let lwf2 = spec::from_spec(&loop_spec3).unwrap();
    let lout2 = lwf2.run(&revise_then_accept, &json!({"draft": "x", "round": 1})).unwrap();
    h.eq("spec-loop: accepted after revisions", lout2.final_action().to_string(), "stop".to_string());
    h.eq("spec-loop: three review passes", lout2.iterations, 3);
    h.eq("spec-loop: accept answer recorded", lout2.state["action_answer"].as_str().unwrap().to_string(), "accept".to_string());

    // ── nested workflow references ──────────────────────────────────
}


pub fn test_spec_nesting(h: &mut Harness) {
    let sub = json!({
        "name": "sub_router", "start": "pick",
        "nodes": [
            {"name": "pick", "primary_q": "kind",
             "questions": {"kind": {"type": "choice", "instructions": "kind?",
                                    "criteria": {"a": "alpha", "b": "beta"}}},
             "edge": {"condition": {"b": "deep"}, "default": "STOP"},
             "action": {"kind": "copy_keys", "keys": ["kind"]}},
            {"name": "deep", "primary_q": "depth",
             "questions": {"depth": {"type": "score", "instructions": "how deep?",
                                     "criteria": ["shallow", "mid", "deep"]}},
             "edge": {"condition": {}, "default": "STOP"},
             "action": {"kind": "threshold", "question": "depth",
                        "rules": [{"when": {"value_gte": 1.5}, "label": "DEEP_PATH"},
                                  {"when": {"always": true}, "label": "SHALLOW_PATH"}]}}
        ]
    });
    spec::register("sub_router_test", sub.clone());

    let parent = json!({
        "name": "parent", "start": "pre",
        "nodes": [
            {"name": "pre", "primary_q": "ok",
             "questions": {"ok": {"type": "choice", "instructions": "proceed?",
                                  "criteria": {"A": "yes", "B": "no"}}},
             "edge": {"condition": {"A": "nested"}, "default": "STOP"}},
            {"name": "nested", "workflow": "sub_router_test"},
            {"name": "post", "primary_q": "keep",
             "questions": {"keep": {"type": "choice", "instructions": "keep?",
                                    "criteria": {"A": "yes", "B": "no"}}},
             "edge": {"condition": {}, "default": "STOP"},
             "action": {"kind": "copy_keys", "keys": ["keep"]}}
        ]
    });
    let pwf = spec::from_spec(&parent).unwrap();
    h.eq("nest: explicit start kept", pwf.start.clone(), "pre".to_string());
    h.eq("nest: nodes expanded + namespaced", pwf.nodes.len(), 4);
    h.check("nest: sub node exists", pwf.nodes.contains_key("nested::deep"));
    h.eq("nest: sub edge rewritten",
         pwf.nodes["nested::pick"].edge.condition["b"].clone(), "nested::deep".to_string());
    h.eq("nest: external edge enters sub start", pwf.nodes["pre"].edge.condition["A"].clone(), "nested::pick".to_string());

    // run the nested graph with a mock: pre(A) -> nested::pick(b) -> nested::deep -> STOP
    let nest_be = ScriptedBackend::new(vec![
        verdict(&[("ok", choice("A", &[("A", 0.9), ("B", 0.1)]))]),
        verdict(&[("kind", choice("b", &[("a", 0.1), ("b", 0.9)]))]),
        verdict(&[("depth", score(2.0, &[0.05, 0.15, 0.8]))]),
    ]);
    let nout = pwf.run(&nest_be, &json!({"text": "x"})).unwrap();
    h.eq("nest: reaches sub leaf", nout.trace.steps.len(), 3);
    h.eq("nest: sub label merged", nout.state["label"].as_str().unwrap().to_string(), "DEEP_PATH".to_string());
    h.eq("nest: sub answer merged", nout.state["kind"].as_str().unwrap().to_string(), "b".to_string());

    // inline nesting (no registry / no file)
    let inline_parent = json!({
        "name": "inline_parent", "start": "go",
        "nodes": [
            {"name": "go", "workflow": {"inline": sub.clone()}, "edge_default": "STOP"}
        ]
    });
    let iwf = spec::from_spec(&inline_parent).unwrap();
    h.eq("nest-inline: start redirected (ref-node name prefix)", iwf.start.clone(), "go::pick".to_string());
    h.eq("nest-inline: expanded nodes", iwf.nodes.len(), 2);
    let iout = iwf
        .run(&ScriptedBackend::new(vec![
            verdict(&[("kind", choice("b", &[("a", 0.1), ("b", 0.9)]))]),
            verdict(&[("depth", score(0.4, &[0.7, 0.2, 0.1]))]),
        ]), &json!({}))
        .unwrap();
    h.eq("nest-inline: runs sub graph", iout.state["label"].as_str().unwrap().to_string(), "SHALLOW_PATH".to_string());

    // ── folder structure: relative refs + tree lookup ───────────────
}


pub fn test_spec_folders(h: &mut Harness) {
    let root = std::env::temp_dir().join("laya_dsl_folders_test");
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(root.join("domain/billing")).unwrap();
    std::fs::write(
        root.join("domain/billing/refund.json"),
        serde_json::to_string(&json!({
            "name": "refund", "start": "band",
            "nodes": [{"name": "band", "primary_q": "amount",
                       "questions": {"amount": {"type": "score", "instructions": "how big?",
                                                "criteria": ["small", "medium", "large"]}},
                       "edge": {"condition": {}, "default": "STOP"},
                       "action": {"kind": "threshold", "question": "amount",
                                  "rules": [{"when": {"value_gte": 1.5}, "label": "MANAGER"},
                                            {"when": {"always": true}, "label": "AUTO"}]}}]
        })).unwrap(),
    )
    .unwrap();
    // relative ref from a sibling folder: ./billing/refund.json
    std::fs::write(
        root.join("domain/parent.json"),
        serde_json::to_string(&json!({
            "name": "parent", "start": "go",
            "nodes": [{"name": "go", "workflow": "./billing/refund.json"}]
        })).unwrap(),
    )
    .unwrap();
    let pw = spec::load_file(root.join("domain/parent.json").to_str().unwrap()).unwrap();
    h.eq("folders: relative ref resolved", pw.start.clone(), "go::band".to_string());
    h.eq("folders: node namespaced", pw.nodes.len(), 1);
    let fout = pw
        .run(&ScriptedBackend::new(vec![verdict(&[("amount", score(2.0, &[0.05, 0.15, 0.8]))])]), &json!({}))
        .unwrap();
    h.eq("folders: sub action ran", fout.state["label"].as_str().unwrap().to_string(), "MANAGER".to_string());

    // tree lookup: bare name into a nested folder, no relative path given
    spec::set_dsl_dir(root.to_str().unwrap());
    let bare = spec::from_spec(&json!({
        "name": "bare", "start": "x",
        "nodes": [{"name": "x", "workflow": "refund"}]
    }))
    .unwrap();
    h.eq("folders: tree lookup by bare name", bare.start, "x::band".to_string());

    // discover() walks the tree and lists specs
    let found = spec::discover(root.to_str().unwrap()).unwrap();
    let names: Vec<&str> = found.iter().map(|(n, _)| n.as_str()).collect();
    h.check("folders: discover finds nested specs",
            names.contains(&"refund") && names.contains(&"parent"));
    spec::set_dsl_dir("dsl"); // restore the default used by the CLI
    let _ = std::fs::remove_dir_all(&root);

    // ── DSL versioning ──────────────────────────────────────────────
}


pub fn test_spec_version(h: &mut Harness) {
    let v = |n: Option<u64>| {
        let mut s = json!({"name": "v", "start": "n",
            "nodes": [{"name": "n", "questions": {"q": {"type": "choice", "instructions": "?",
                                                        "criteria": {"A": "a"}}},
                       "edge": {"default": "STOP"}}]});
        if let Some(n) = n {
            s["dsl_version"] = json!(n);
        }
        s
    };
    h.eq("dsl: absent version accepted", spec::check_version(&v(None)).is_ok(), true);
    h.eq("dsl: absent == v1 upgradable",
         spec::check_version(&v(None)).unwrap(), spec::VersionCheck::Upgradable { from: 1, to: spec::DSL_VERSION });
    h.eq("dsl: current version ok",
         spec::check_version(&v(Some(spec::DSL_VERSION))).unwrap(), spec::VersionCheck::Ok(spec::DSL_VERSION));
    h.check("dsl: version 0 rejected", spec::check_version(&v(Some(0))).is_err());
    h.check("dsl: future version rejected", spec::check_version(&v(Some(spec::DSL_VERSION + 1))).is_err());
    h.check("dsl: future version message mentions both",
            spec::check_version(&v(Some(99))).unwrap_err().to_string().contains("99"));
    h.check("dsl: legacy spec still parses",
            spec::from_spec(&v(Some(1))).is_ok());

    // multi-version resolution: pin with `name@N`, else highest wins
    let vroot = std::env::temp_dir().join("laya_dsl_version_test");
    let _ = std::fs::remove_dir_all(&vroot);
    std::fs::create_dir_all(&vroot).unwrap();
    let mk = |ver: u64, label: &str| {
        json!({
            "name": "svc", "dsl_version": ver, "start": "run",
            "nodes": [{"name": "run", "primary_q": "q",
                       "questions": {"q": {"type": "choice", "instructions": "?",
                                           "criteria": {"A": "a", "B": "b"}}},
                       "edge": {"condition": {}, "default": "STOP"},
                       "action": {"kind": "threshold", "question": "q",
                                  "rules": [{"when": {"always": true}, "label": label}]}}]
        })
    };
    std::fs::write(vroot.join("svc.v1.json"), serde_json::to_string(&mk(1, "V1")).unwrap()).unwrap();
    std::fs::write(vroot.join("svc.v2.json"), serde_json::to_string(&mk(2, "V2")).unwrap()).unwrap();

    let pick = |reference: &str| {
        let spec = json!({"name": "caller", "start": "go",
                          "nodes": [{"name": "go", "workflow": reference}]});
        let wf = spec::from_spec_in(&spec, Some(&vroot)).unwrap();
        let out = wf.run(&ScriptedBackend::new(vec![
            verdict(&[("q", choice("A", &[("A", 0.9), ("B", 0.1)]))]),
        ]), &json!({})).unwrap();
        out.state["label"].as_str().unwrap().to_string()
    };
    spec::set_dsl_dir(vroot.to_str().unwrap());
    h.eq("dsl: unpinned picks highest version", pick("svc"), "V2".to_string());
    h.eq("dsl: pin @1 selects v1", pick("svc@1"), "V1".to_string());
    h.eq("dsl: pin @2 selects v2", pick("svc@2"), "V2".to_string());
    h.check("dsl: pin to missing version errors", {
        let spec = json!({"name": "c", "start": "go", "nodes": [{"name": "go", "workflow": "svc@7"}]});
        spec::from_spec_in(&spec, Some(&vroot)).is_err()
    });
    let listed: Vec<String> = spec::discover(vroot.to_str().unwrap()).unwrap().into_iter().map(|(n, _)| n).collect();
    h.check("dsl: discover lists both versions",
            listed.iter().filter(|n| n.starts_with("svc")).count() == 2);
    spec::set_dsl_dir("dsl");
    let _ = std::fs::remove_dir_all(&vroot);

    // ── external capabilities ───────────────────────────────────────
}

