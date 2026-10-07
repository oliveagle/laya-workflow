//! computer-use (awesome-jev) regression sections for `laya-workflow-tests`.
//! Bounded UI decision cycle: fingerprint before inference, prepare in code,
//! stale-guarded single-fill executor, independent oracle.
#![allow(unused_imports)]
use crate::*;
use std::collections::HashMap;

fn fixture() -> Value {
    json!({
        "goal": "Fill the accounts payable email with the supplied billing_contact, leave the delivery contact unchanged, and read the invoice amount due. Do not submit the form.",
        "values": {"billing_contact": "billing@example.invalid"},
        "snapshot": {
            "surface": "https://invoice.example.invalid/fixture",
            "revision": 1,
            "submitted": false,
            "elements": [
                {"id": "e1", "role": "textbox", "label": "Delivery contact", "enabled": true, "value": "shipping@example.invalid"},
                {"id": "e2", "role": "textbox", "label": "Accounts payable", "enabled": true, "value": ""},
                {"id": "e3", "role": "button", "label": "Pay invoice", "enabled": true}
            ],
            "texts": [
                {"id": "t1", "label": "Subtotal", "text": "€100.00"},
                {"id": "t2", "label": "Amount due (tax included)", "text": "€120.00"}
            ]
        }
    })
}

fn ui_cap() -> serde_json::Value {
    json!({
        "kind": "computer_use",
        "allowed_surface": "https://invoice.example.invalid/fixture",
        "allowed_fields": ["e1", "e2"],
        "expected": {
            "billing_field": "e2",
            "billing_value": "billing@example.invalid",
            "delivery_field": "e1",
            "delivery_value": "shipping@example.invalid",
            "amount_source": "t2",
            "amount_value": "€120.00"
        }
    })
}

/// Build a ready proposal as `ui_proposal` would: action + extracted.
/// `hash` is the captured snapshot fingerprint (the executor rechecks it).
fn ready_proposal(action: Value, hash: &str) -> Value {
    json!({
        "status": "ready",
        "snapshot_hash": hash,
        "surface": "https://invoice.example.invalid/fixture",
        "action": action,
        "extracted": {
            "source_id": "t2",
            "value": "€120.00",
            "surface": "https://invoice.example.invalid/fixture"
        }
    })
}

fn call_ui(op: &str, with: &Value) -> anyhow::Result<Value> {
    let reg = capability::Registry::from_spec(&json!({
        "capabilities": {"ui": ui_cap()}
    }))
    .unwrap();
    let mut w = with.clone();
    if let Some(o) = w.as_object_mut() {
        o.insert("op".to_string(), json!(op));
    }
    reg.call("ui", &w, &json!({}))
}

pub fn test_computer_use(h: &mut Harness) {
    let fx = fixture();

    // ── fingerprint: canonical (key-sorted) hashing is deterministic ──────
    let f1 = call_ui("fingerprint", &json!({"snapshot": fx["snapshot"]})).unwrap();
    let f2 = call_ui("fingerprint", &json!({"snapshot": fx["snapshot"]})).unwrap();
    let fp1 = f1["snapshot_hash"].as_str().unwrap_or("").to_string();
    let fp2 = f2["snapshot_hash"].as_str().unwrap_or("").to_string();
    h.check("cu: fingerprint stable", fp1 == fp2);
    h.check("cu: fingerprint is 64-hex", fp1.len() == 64);
    h.eq("cu: fingerprint surface", f1["surface"].as_str().unwrap_or(""), "https://invoice.example.invalid/fixture");
    h.eq("cu: fingerprint revision", f1["revision"].as_i64().unwrap_or(0), 1i64);

    // key order must not matter (serde_json preserve_order would otherwise differ)
    let mut reordered = fx["snapshot"].clone();
    if let Some(o) = reordered.as_object_mut() {
        // rebuild in reverse insertion order
        let items: Vec<(String, Value)> = o.iter().map(|(k, v)| (k.clone(), v.clone())).collect();
        let mut m = serde_json::Map::new();
        for (k, v) in items.into_iter().rev() {
            m.insert(k, v);
        }
        reordered = Value::Object(m);
    }
    let f3 = call_ui("fingerprint", &json!({"snapshot": reordered})).unwrap();
    h.check("cu: fingerprint ignores key order",
        f3["snapshot_hash"].as_str().unwrap_or("") == f1["snapshot_hash"].as_str().unwrap_or(""));

    // ── step: happy path — one field filled, delivery preserved ──────────
    let real_fp = f1["snapshot_hash"].as_str().unwrap_or("").to_string();
    let prop = ready_proposal(json!({
        "kind": "fill", "target_id": "e2", "value_key": "billing_contact"
    }), &real_fp);
    let step = call_ui(
        "step",
        &json!({
            "snapshot": fx["snapshot"],
            "values": fx["values"],
            "proposal": prop
        }),
    )
    .unwrap();
    h.eq("cu: step applied", step["status"].as_str().unwrap_or(""), "applied");
    let after = &step["snapshot"];
    h.eq("cu: e2 filled", after["elements"][1]["value"].as_str().unwrap_or(""), "billing@example.invalid");
    h.eq("cu: e1 preserved", after["elements"][0]["value"].as_str().unwrap_or(""), "shipping@example.invalid");
    h.eq("cu: revision bumped", after["revision"].as_i64().unwrap_or(0), 2i64);
    h.check("cu: nothing submitted", after["submitted"].as_bool() == Some(false));
    h.check("cu: one_field_only", step["one_field_only"] == true);

    // ── stale observation: fingerprint mismatch refuses execution ────────
    let mut tampered = fx["snapshot"].clone();
    tampered["revision"] = json!(999);
    let prop2 = ready_proposal(json!({
        "kind": "fill", "target_id": "e2", "value_key": "billing_contact"
    }), &real_fp);
    // The proposal carries a hash of the ORIGINAL snapshot, but the executor
    // sees the tampered current one → stale.
    let stale = call_ui(
        "step",
        &json!({
            "snapshot": tampered,
            "values": fx["values"],
            "proposal": prop2
        }),
    );
    h.check("cu: stale observation refused", stale.is_err() && stale.unwrap_err().to_string().contains("stale"));

    // ── surface scope: outside the allow-list refused ─────────────────────
    let mut other_surface = fx["snapshot"].clone();
    other_surface["surface"] = json!("https://evil.example.invalid/");
    let prop3 = ready_proposal(json!({
        "kind": "fill", "target_id": "e2", "value_key": "billing_contact"
    }), &real_fp);
    let surf = call_ui(
        "step",
        &json!({
            "snapshot": other_surface,
            "values": fx["values"],
            "proposal": prop3
        }),
    );
    h.check("cu: surface outside allow-list refused",
        surf.is_err() && surf.unwrap_err().to_string().contains("surface"));

    // ── field scope: fill on a field outside allowed_fields refused ──────
    let prop4 = ready_proposal(json!({
        "kind": "fill", "target_id": "e3", "value_key": "billing_contact"
    }), &real_fp);
    let scope = call_ui(
        "step",
        &json!({
            "snapshot": fx["snapshot"],
            "values": fx["values"],
            "proposal": prop4
        }),
    );
    h.check("cu: field outside permission scope refused",
        scope.is_err() && scope.unwrap_err().to_string().contains("permission scope"));

    // ── disabled target refused ──────────────────────────────────────────
    // The disabled snapshot hashes differently from the original, so the
    // proposal must carry the *disabled* snapshot's own fingerprint — the
    // surface/stale checks pass and only the enabled-guard fires.
    let mut dis = fx["snapshot"].clone();
    dis["elements"][1]["enabled"] = json!(false);
    let dis_fp = call_ui("fingerprint", &json!({"snapshot": dis})).unwrap();
    let dis_fp = dis_fp["snapshot_hash"].as_str().unwrap_or("").to_string();
    let prop5 = ready_proposal(json!({
        "kind": "fill", "target_id": "e2", "value_key": "billing_contact"
    }), &dis_fp);
    let dis_r = call_ui(
        "step",
        &json!({
            "snapshot": dis,
            "values": fx["values"],
            "proposal": prop5
        }),
    );
    h.check("cu: disabled field refused",
        dis_r.is_err() && dis_r.unwrap_err().to_string().contains("disabled"));

    // ── action kind outside fill refused ─────────────────────────────────
    let prop6 = ready_proposal(json!({
        "kind": "click", "target_id": "e3"
    }), &real_fp);
    let kind_r = call_ui(
        "step",
        &json!({
            "snapshot": fx["snapshot"],
            "values": fx["values"],
            "proposal": prop6
        }),
    );
    h.check("cu: non-fill action refused",
        kind_r.is_err() && kind_r.unwrap_err().to_string().contains("permission scope"));

    // ── only a ready proposal reaches the executor ───────────────────────
    let prop7 = json!({"status": "human_review", "reason": "uncertain"});
    let not_ready = call_ui(
        "step",
        &json!({
            "snapshot": fx["snapshot"],
            "values": fx["values"],
            "proposal": prop7
        }),
    );
    h.check("cu: non-ready proposal refused",
        not_ready.is_err() && not_ready.unwrap_err().to_string().contains("ready"));

    // ── verify: independent oracle, happy path ────────────────────────────
    let after_filled = step["snapshot"].clone();
    let ok_prop = ready_proposal(json!({
        "kind": "fill", "target_id": "e2", "value_key": "billing_contact"
    }), &real_fp);
    let verify = call_ui(
        "verify",
        &json!({"after": after_filled, "proposal": ok_prop}),
    )
    .unwrap();
    h.eq("cu: verify status", verify["status"].as_str().unwrap_or(""), "verified");
    h.check("cu: billing_value check", verify["checks"]["billing_value"] == true);
    h.check("cu: delivery_unchanged check", verify["checks"]["delivery_unchanged"] == true);
    h.check("cu: not_submitted check", verify["checks"]["not_submitted"] == true);
    h.check("cu: amount_source check", verify["checks"]["amount_source"] == true);
    h.check("cu: amount_value check", verify["checks"]["amount_value"] == true);

    // ── verify: wrong billing value → failed ─────────────────────────────
    let mut wrong = after_filled.clone();
    wrong["elements"][1]["value"] = json!("billing@evil.example.invalid");
    let vfail = call_ui(
        "verify",
        &json!({"after": wrong, "proposal": ok_prop}),
    )
    .unwrap();
    h.eq("cu: wrong value fails oracle", vfail["status"].as_str().unwrap_or(""), "failed");
    h.check("cu: billing_value false", vfail["checks"]["billing_value"] == false);

    // ── verify: delivery changed → failed ────────────────────────────────
    let mut touched = after_filled.clone();
    touched["elements"][0]["value"] = json!("changed@example.invalid");
    let vdel = call_ui(
        "verify",
        &json!({"after": touched, "proposal": ok_prop}),
    )
    .unwrap();
    h.eq("cu: delivery change fails oracle", vdel["status"].as_str().unwrap_or(""), "failed");

    // ── ui_proposal action (prepare in code) ─────────────────────────────
    use laya_workflow::workflow::{Decision, Verdict};
    let mk = |answer: &str, conf: f64| Decision {
        answer: json!(answer),
        probabilities: {
            let mut m = serde_json::Map::new();
            m.insert(answer.to_string(), json!(0.9));
            m.insert("other".to_string(), json!(0.1));
            m
        },
        confidence: conf,
    };
    let act = json!({
        "kind": "ui_proposal",
        "operation": "operation", "field": "field", "amount": "amount",
        "min_confidence": 0.8, "value_key": "billing_contact"
    });
    let mk_verdict = |op: &str, fld: &str, amt: &str| {
        let mut ans = HashMap::new();
        ans.insert("operation".into(), mk(op, 0.94));
        ans.insert("field".into(), mk(fld, 0.94));
        ans.insert("amount".into(), mk(amt, 0.94));
        Verdict { answers: ans, input_tokens: 0, latency_ms: 0.0 }
    };
    // state carries surface/hash (never authored by the model)
    let st = json!({
        "surface": "https://invoice.example.invalid/fixture",
        "snapshot_hash": "abc123",
        "snapshot": fx["snapshot"],
    });
    let out = laya_workflow::spec::run_action(&act, &st, &mk_verdict("fill", "e2", "t2"), Some("operation")).unwrap();
    h.eq("cu: proposal ready", out["prepare_status"].as_str().unwrap_or(""), "ready");
    h.eq("cu: proposal action kind", out["proposal"]["action"]["kind"].as_str().unwrap_or(""), "fill");
    h.eq("cu: proposal target", out["proposal"]["action"]["target_id"].as_str().unwrap_or(""), "e2");
    h.eq("cu: proposal extracted source", out["proposal"]["extracted"]["source_id"].as_str().unwrap_or(""), "t2");
    h.eq("cu: proposal extracted value is source text", out["proposal"]["extracted"]["value"].as_str().unwrap_or(""), "€120.00");
    h.eq("cu: proposal carries state hash", out["proposal"]["snapshot_hash"].as_str().unwrap_or(""), "abc123");

    // low-confidence operation → human_review, no action
    let mut low = HashMap::new();
    low.insert("operation".into(), mk("fill", 0.5));
    low.insert("field".into(), mk("e2", 0.94));
    low.insert("amount".into(), mk("t2", 0.94));
    let low_v = Verdict { answers: low, input_tokens: 0, latency_ms: 0.0 };
    let low_out = laya_workflow::spec::run_action(&act, &st, &low_v, Some("operation")).unwrap();
    h.eq("cu: low-confidence op → human_review", low_out["prepare_status"].as_str().unwrap_or(""), "human_review");
    h.check("cu: no action on review", low_out["proposal"]["action"].is_null());

    // blocked / wait operations stop the cycle
    let blk = laya_workflow::spec::run_action(&act, &st, &mk_verdict("blocked", "none", "t2"), Some("operation")).unwrap();
    h.eq("cu: blocked op stops", blk["prepare_status"].as_str().unwrap_or(""), "blocked");

    // ── end-to-end spec run: happy path ──────────────────────────────────
    let spec_path = concat!(env!("CARGO_MANIFEST_DIR"), "/dsl/capabilities/computer_use.json");
    let wf = laya_workflow::spec::load_file(spec_path).unwrap();
    let mut backend = laya_workflow::backend::HeuristicBackend;
    let mut st = fx.clone();
    let out_run = wf.run(&mut backend, &mut st).unwrap();
    let out = out_run.to_json();
    h.eq("cu: e2e verify verified", out["result"]["verify_status"].as_str().unwrap_or(""), "verified");
    let checks = out["result"]["checks"].clone();
    h.check("cu: e2e all checks true", checks.as_object().map(|o| o.values().all(|v| v == &json!(true))).unwrap_or(false));
    h.eq("cu: e2e e2 filled", out["result"]["after_snapshot"]["elements"][1]["value"].as_str().unwrap_or(""), "billing@example.invalid");
    h.eq("cu: e2e revision bumped", out["result"]["after_snapshot"]["revision"].as_i64().unwrap_or(0), 2i64);
}
