//! engine regression sections for `laya-workflow-tests`.
#![allow(unused_imports)]
use crate::*;
pub fn test_edge(h: &mut Harness) {
    let e = Edge::new(&[("yes", "a"), ("no", "b")], Some("d"), 0.0);
    h.eq("edge exact", e.resolve(&json!("yes"), 1.0), Some("a".into()));
    h.eq("edge none-default", e.resolve(&json!("maybe"), 1.0), Some("d".into()));
    let gated = Edge::new(&[("yes", "a")], None, 0.8);
    h.eq("edge min_conf blocks", gated.resolve(&json!("yes"), 0.5), None);
    h.eq("edge min_conf passes", gated.resolve(&json!("yes"), 0.9), Some("a".into()));

    // ── Node actions ────────────────────────────────────────────────
}


pub fn test_node(h: &mut Harness) {
    let node_self = WorkflowNode::new("n", q(&[("x", "?")]), Edge::new(&[("yes", "n")], None, 0.0));
    let be = ScriptedBackend::new(vec![verdict(&[("x", choice("yes", &[("yes", 0.9), ("no", 0.1)]))])]);
    h.eq("node self-loop -> retry", node_self.run(&be, &json!({})).unwrap().action, NodeAction::Retry);

    let node_stop = WorkflowNode::new("n", q(&[("x", "?")]), Edge::new(&[("yes", "STOP")], None, 0.0));
    let be = ScriptedBackend::new(vec![verdict(&[("x", choice("yes", &[("yes", 0.9), ("no", 0.1)]))])]);
    h.eq("node STOP", node_stop.run(&be, &json!({})).unwrap().action, NodeAction::Stop);

    let node_exec = WorkflowNode::new("n", q(&[("x", "?")]), Edge::new(&[("yes", "EXECUTE:next")], None, 0.0));
    let be = ScriptedBackend::new(vec![verdict(&[("x", choice("yes", &[("yes", 0.9), ("no", 0.1)]))])]);
    let r = node_exec.run(&be, &json!({})).unwrap();
    h.eq("node EXECUTE action", r.action, NodeAction::Execute);
    h.eq("node EXECUTE target", r.next_node, Some("next".into()));

    let node_esc = WorkflowNode::new("n", q(&[("x", "?")]), Edge::new(&[("yes", "a")], None, 0.8));
    let be = ScriptedBackend::new(vec![verdict(&[("x", choice("yes", &[("yes", 0.5), ("no", 0.5)]))])]);
    h.eq("node low conf -> escalate", node_esc.run(&be, &json!({})).unwrap().action, NodeAction::Escalate);

    // ── Workflow flows ──────────────────────────────────────────────
}


pub fn test_workflow(h: &mut Harness) {
    let a = WorkflowNode::new("a", q(&[("x", "?")]), Edge::new(&[("go", "b")], None, 0.0));
    let b = WorkflowNode::new("b", q(&[("x", "?")]), Edge::new(&[("go", "STOP")], None, 0.0));
    let wf = ResilientWorkflow::new(vec![a, b], "a");
    let be = ScriptedBackend::new(vec![
        verdict(&[("x", choice("go", &[("go", 1.0)]))]),
        verdict(&[("x", choice("go", &[("go", 1.0)]))]),
    ]);
    let out = wf.run(&be, &json!({})).unwrap();
    h.eq("linear flow final", out.final_action().to_string(), "stop".to_string());
    h.eq("linear flow iterations", out.iterations, 2);
    h.eq("linear flow steps", out.trace.steps.len(), 2);

    // retry budget -> escalate
    let r1 = WorkflowNode::new("r", q(&[("x", "?")]), Edge::new(&[("no", "r")], None, 0.0)).with_max_retries(2);
    let wf = ResilientWorkflow::new(vec![r1], "r").with_max_iterations(10);
    let be = ScriptedBackend::new(vec![verdict(&[("x", choice("no", &[("no", 1.0)]))])]);
    let out = wf.run(&be, &json!({})).unwrap();
    h.eq("retry budget escalates", out.final_action().to_string(), "escalate".to_string());

    // convergence (score window flat) -> converged
    let s1 = WorkflowNode::new("s", q(&[("x", "?")]), Edge::new(&[("no", "s")], None, 0.0)).with_max_retries(50);
    let wf = ResilientWorkflow::new(vec![s1], "s").with_max_iterations(20).with_convergence(3, 1e-4);
    let be = ScriptedBackend::new(vec![verdict(&[("x", choice("no", &[("no", 1.0)]))])]);
    let out = wf.run(&be, &json!({"score": 5.0})).unwrap();
    h.eq("convergence detected", out.final_action().to_string(), "converged".to_string());
    h.check("loop_detected flag", out.trace.loop_detected);

    // max_iterations safety valve
    let m = WorkflowNode::new("m", q(&[("x", "?")]), Edge::new(&[("no", "m")], None, 0.0)).with_max_retries(100);
    let wf = ResilientWorkflow::new(vec![m], "m").with_max_iterations(4);
    let be = ScriptedBackend::new(vec![verdict(&[("x", choice("no", &[("no", 1.0)]))])]);
    let out = wf.run(&be, &json!({})).unwrap();
    h.eq("max_iterations valve", out.final_action().to_string(), "max_iterations".to_string());
    h.eq("max_iterations capped", out.iterations, 4);

    // missing node
    let wf = ResilientWorkflow::new(vec![WorkflowNode::new("a", q(&[("x", "?")]), Edge::new(&[("go", "ghost")], None, 0.0))], "a");
    let be = ScriptedBackend::new(vec![verdict(&[("x", choice("go", &[("go", 1.0)]))])]);
    let out = wf.run(&be, &json!({})).unwrap();
    h.eq("missing node error", out.final_action().to_string(), "error_node_missing".to_string());

    // escalation via low confidence
    let e1 = WorkflowNode::new("e", q(&[("x", "?")]), Edge::new(&[("go", "STOP")], None, 0.9));
    let wf = ResilientWorkflow::new(vec![e1], "e");
    let be = ScriptedBackend::new(vec![verdict(&[("x", choice("go", &[("go", 0.3)]))])]);
    let out = wf.run(&be, &json!({})).unwrap();
    h.eq("low confidence escalates", out.final_action().to_string(), "escalate".to_string());

    // describe
    let d = ResilientWorkflow::new(vec![WorkflowNode::new("x", q(&[("q1", "?")]), Edge::new(&[("a", "STOP")], Some("x"), 0.5))], "x").describe();
    h.eq("describe start", d["start"].as_str().unwrap().to_string(), "x".to_string());
    h.eq("describe node keys", d["nodes"]["x"]["edge_condition"]["a"].as_str().unwrap().to_string(), "STOP".to_string());

    // ── composition ─────────────────────────────────────────────────
}


pub fn test_composition(h: &mut Harness) {
    let inner_a = WorkflowNode::new("ia", q(&[("x", "?")]), Edge::new(&[("go", "STOP")], None, 0.0));
    let inner = ResilientWorkflow::new(vec![inner_a], "ia");
    let sub = SubWorkflow::new("sub", inner);
    let be = ScriptedBackend::new(vec![verdict(&[("x", choice("go", &[("go", 1.0)]))])]);
    let r = sub.execute(&be, &json!({})).unwrap();
    h.eq("subworkflow name", r["_subworkflow"].as_str().unwrap().to_string(), "sub".to_string());
    h.check("subworkflow has trace", r["_subworkflow_trace"]["steps"].is_array());

    let mk = |n: &str| {
        ResilientWorkflow::new(
            vec![WorkflowNode::new(n, q(&[("x", "?")]), Edge::new(&[("go", "STOP")], None, 0.0))],
            n,
        )
    };
    let fan = FanOut::new("fan", vec![("b1".into(), mk("b1")), ("b2".into(), mk("b2"))]);
    let be = ScriptedBackend::new(vec![verdict(&[("x", choice("go", &[("go", 1.0)]))])]);
    let r = fan.execute(&be, &json!({})).unwrap();
    h.eq("fanout branches", r["_fanout_results"].as_array().unwrap().len(), 2);
    h.eq("fanout merge total iters", r["_merged"]["total_iterations"].as_u64().unwrap(), 2);

    // checkpoint round-trip
    let dir = format!("{}/laya_wf_ckpt_test", std::env::temp_dir().display());
    let _ = std::fs::remove_dir_all(&dir);
    let ck = Checkpoint::new(&format!("{dir}/c.json"));
    h.check("checkpoint absent", !ck.exists());
    ck.save(&json!({"score": 1.5}), 3, &[json!({"i": 1})], Some(&json!({"k": "v"}))).unwrap();
    let (st, it, hist, extra) = ck.load().unwrap().unwrap();
    h.eq("checkpoint iteration", it, 3);
    h.eq("checkpoint state", st["score"].as_f64().unwrap(), 1.5);
    h.eq("checkpoint history len", hist.len(), 1);
    h.eq("checkpoint extra", extra["k"].as_str().unwrap().to_string(), "v".to_string());
    ck.delete().unwrap();
    h.check("checkpoint deleted", !ck.exists());
    let _ = std::fs::remove_dir_all(&dir);

    h.eq("consecutive_same", consecutive_same(&[1.0, 2.0, 2.0, 2.0], 1e-6), 3);

    // ── apps (scripted verdicts replicating the reference labels) ────
}

