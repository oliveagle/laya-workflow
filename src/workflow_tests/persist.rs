//! persistence / rewind / replay sections for `laya-workflow-tests`.
#![allow(unused_imports)]
use crate::*;
use laya_workflow::persist::{NodeRecord, NodeStore};

fn mk_nodes() -> Vec<WorkflowNode> {
    // Each node's action payload writes `n` into the state, so the state grows
    // as we replay and resume.
    let a = WorkflowNode::new(
        "a",
        q(&[("x", "?")]),
        Edge::new(&[("go", "b")], None, 0.0),
    ).with_action(|state: &Value, _v: &laya_workflow::workflow::Verdict| {
        let prev = state.get("n").and_then(|x| x.as_i64()).unwrap_or(0);
        Ok(json!({"n": prev + 1, "step": "a"}))
    });
    let b = WorkflowNode::new(
        "b",
        q(&[("x", "?")]),
        Edge::new(&[("go", "STOP")], None, 0.0),
    ).with_action(|state: &Value, _v: &laya_workflow::workflow::Verdict| {
        let prev = state.get("n").and_then(|x| x.as_i64()).unwrap_or(0);
        Ok(json!({"n": prev + 1, "step": "b"}))
    });
    vec![a, b]
}

fn mk_backend() -> ScriptedBackend {
    ScriptedBackend::new(vec![
        verdict(&[("x", choice("go", &[("go", 1.0), ("no", 0.0)]))]),
        verdict(&[("x", choice("go", &[("go", 1.0), ("no", 0.0)]))]),
    ])
}

pub fn test_persistence(h: &mut Harness) {
    // ── 1) NodeStore primitive ops: write/read/read_all/materialise/delete
    let dir = std::env::temp_dir().join("laya_persist_store_test");
    let _ = std::fs::remove_dir_all(&dir);
    let mut store = NodeStore::open(dir.to_str().unwrap()).unwrap();
    h.check("persist: open creates dir + runs/", store.runs_dir.is_dir());
    h.check("persist: empty store has no last_iteration", store.last_iteration().unwrap().is_none());

    let rec = NodeRecord {
        node: "a".into(),
        iteration: 1,
        timestamp_ms: 1,
        state_before: json!({"n": 0}),
        state_after: json!({"n": 1, "step": 1}),
        payload: Some(json!({"step": 1})),
        action: "route".into(),
        edge_answer: json!("go"),
        confidence: 1.0,
        latency_ms: 0.1,
        next_node: Some("b".into()),
        detail: Some("routed to b".into()),
        error: None,
    };
    store.write(&rec).unwrap();
    let rec2 = NodeRecord {
        node: "b".into(),
        iteration: 2,
        timestamp_ms: 2,
        state_before: json!({"n": 1, "step": 1}),
        state_after: json!({"n": 2, "step": 2}),
        payload: Some(json!({"step": 2})),
        action: "stop".into(),
        edge_answer: json!("go"),
        confidence: 1.0,
        latency_ms: 0.2,
        next_node: None,
        detail: Some("workflow stopped".into()),
        error: None,
    };
    store.write(&rec2).unwrap();
    h.eq("persist: two records stored", store.iterations().unwrap().len(), 2);
    h.eq("persist: last_iteration", store.last_iteration().unwrap(), Some(2));
    let all = store.read_all().unwrap();
    h.eq("persist: read_all sorted by iteration", all[1].node.clone(), "b".to_string());
    h.eq("persist: read one", store.read(1).unwrap().unwrap().node, "a".to_string());
    h.check("persist: read missing -> None", store.read(99).unwrap().is_none());
    let st = store.materialise_state(2).unwrap();
    h.eq("persist: materialise final state", st["step"].as_u64().unwrap(), 2);
    let st1 = store.materialise_state(1).unwrap();
    h.eq("persist: materialise at iter 1", st1["step"].as_u64().unwrap(), 1);
    h.check("persist: delete_from truncates", store.delete_from(2).unwrap() == 1);
    h.eq("persist: last_iteration after truncate", store.last_iteration().unwrap(), Some(1));
    store.write(&rec2).unwrap();
    let _ = store.clear().unwrap();
    h.check("persist: clear empties store", store.read_all().unwrap().is_empty());
    h.check("persist: clear leaves manifest dir", store.dir.is_dir());

    // ── 2) run_persistent writes one record per executed node
    let dir2 = std::env::temp_dir().join("laya_persist_run_test");
    let _ = std::fs::remove_dir_all(&dir2);
    let mut store2 = NodeStore::open(dir2.to_str().unwrap()).unwrap();
    let wf = ResilientWorkflow::new(mk_nodes(), "a");
    let be = mk_backend();
    let out = wf.run_persistent(&be, &mut store2, Some(&json!({"n": 0})), None).unwrap();
    h.eq("persist: final action", out.final_action().to_string(), "stop".to_string());
    h.eq("persist: one record per node", store2.iterations().unwrap().len(), 2);
    let r1 = store2.read(1).unwrap().unwrap();
    h.eq("persist: iter1 node", r1.node, "a".to_string());
    h.eq("persist: iter1 action", r1.action, "route".to_string());
    h.eq("persist: iter1 next", r1.next_node, Some("b".to_string()));
    h.eq("persist: iter1 state_before n", r1.state_before["n"].as_u64().unwrap(), 0);
    h.eq("persist: iter1 state_after n", r1.state_after["n"].as_u64().unwrap(), 1);
    h.eq("persist: iter2 state_after n", store2.read(2).unwrap().unwrap().state_after["n"].as_u64().unwrap(), 2);
    h.eq("persist: outcome state matches store materialise",
        out.state["n"].as_u64().unwrap(),
        store2.materialise_state(2).unwrap()["n"].as_u64().unwrap());

    // ── 3) rewind: materialise mid-run state, truncate the tail
    let dir3 = std::env::temp_dir().join("laya_persist_rewind_test");
    let _ = std::fs::remove_dir_all(&dir3);
    let mut store3 = NodeStore::open(dir3.to_str().unwrap()).unwrap();
    let wf3 = ResilientWorkflow::new(mk_nodes(), "a");
    let be3 = mk_backend();
    wf3.run_persistent(&be3, &mut store3, Some(&json!({"n": 0})), None).unwrap();
    let mid = wf3.rewind(&mut store3, 1).unwrap();
    h.eq("persist: rewind to iter1 state", mid["n"].as_u64().unwrap(), 1);
    h.eq("persist: rewind truncates iter2", store3.last_iteration().unwrap(), Some(1));

    // ── 4) replay: re-execute just iter 2 from its state_before
    let dir4 = std::env::temp_dir().join("laya_persist_replay_test");
    let _ = std::fs::remove_dir_all(&dir4);
    let mut store4 = NodeStore::open(dir4.to_str().unwrap()).unwrap();
    let wf4 = ResilientWorkflow::new(mk_nodes(), "a");
    let be4 = mk_backend();
    wf4.run_persistent(&be4, &mut store4, Some(&json!({"n": 0})), None).unwrap();
    let r = wf4.replay(&be4, &mut store4, 2).unwrap();
    h.eq("persist: replay keeps node", r.node, "b".to_string());
    h.eq("persist: replay keeps state_after n", r.state_after["n"].as_u64().unwrap(), 2);
    h.eq("persist: replay keeps original state_before n", r.state_before["n"].as_u64().unwrap(), 1);
    h.check("persist: replay errors on missing iter", wf4.replay(&be4, &mut store4, 99).is_err());

    // ── 5) resume: run once with max_iterations=1 (partial), then resume to completion
    let dir5 = std::env::temp_dir().join("laya_persist_resume_test");
    let _ = std::fs::remove_dir_all(&dir5);
    let mut store5 = NodeStore::open(dir5.to_str().unwrap()).unwrap();
    let wf5 = ResilientWorkflow::new(mk_nodes(), "a").with_max_iterations(1);
    let be5 = mk_backend();
    let partial = wf5.run_persistent(&be5, &mut store5, Some(&json!({"n": 0})), None).unwrap();
    h.eq("persist: partial run stops at max_iterations", partial.final_action().to_string(), "max_iterations".to_string());
    h.eq("persist: partial run has one record", store5.iterations().unwrap().len(), 1);
    let wf5b = ResilientWorkflow::new(mk_nodes(), "a").with_max_iterations(10);
    let be5b = mk_backend();
    let full = wf5b.run_persistent(&be5b, &mut store5, None, None).unwrap();
    h.eq("persist: resume completes to stop", full.final_action().to_string(), "stop".to_string());
    h.eq("persist: resume accumulates records", store5.iterations().unwrap().len(), 2);
    h.eq("persist: resume final state matches straight run",
        full.state["n"].as_u64().unwrap(), 2);
    h.eq("persist: resume final state matches materialise",
        full.state["n"].as_u64().unwrap(), store5.materialise_state(2).unwrap()["n"].as_u64().unwrap());

    // ── 6) tracking: every record is queryable via read_all / manifest
    let all5 = store5.read_all().unwrap();
    h.check("persist: read_all returns all stored iters", all5.len() == 2);
    h.eq("persist: node sequence", (all5[0].node.as_str(), all5[1].node.as_str()), ("a", "b"));
    h.check("persist: manifest exists", store5.manifest_path.exists());

    for d in ["laya_persist_store_test","laya_persist_run_test","laya_persist_rewind_test","laya_persist_replay_test","laya_persist_resume_test"] {
        let _ = std::fs::remove_dir_all(std::env::temp_dir().join(d));
    }
}
