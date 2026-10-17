//! heuristics regression sections for `laya-workflow-tests`.
#![allow(unused_imports)]
use crate::*;
pub fn test_heuristic_fixes(h: &mut Harness) {
    {
        use laya_workflow::backend::HeuristicBackend;
        use laya_workflow::workflow::Decide;

        let be = HeuristicBackend;

        // (1) Polarity bug: `intent_match` has A = "yes, matches intent", but the
        // backend emitted bchoice(0.9) => P(B)=0.9 => "no, dangerous" for every
        // command, so agent_gate's `intent_p < 0.5` branch blocked everything.
        let q = laya_workflow::apps::agent_gate::questions();
        let safe = json!({"command": "ls -la /tmp", "intent": "list files in /tmp", "cwd": "/"});
        let v = be.decide(&safe, &q).unwrap();
        h.check(
            "fix:intent_match reports a matching command as A (matches)",
            v.prob("intent_match", "A").unwrap_or(0.0) >= 0.5,
        );
        h.eq(
            "fix: a benign, intent-matching command is ALLOW (was BLOCK)",
            laya_workflow::apps::agent_gate::route("ls -la /tmp", &v)["action"]
                .as_str()
                .unwrap_or(""),
            "ALLOW",
        );

        // (2) `cat /etc/passwd` matches its stated intent, so it must NOT be an
        // intent mismatch; it is a sensitive read and belongs at CONFIRM.
        let v2 = be.decide(
            &json!({"command": "cat /etc/passwd", "intent": "show system user list", "cwd": "/"}),
            &q,
        )
        .unwrap();
        h.check(
            "fix: a sensitive read still matches intent",
            v2.prob("intent_match", "A").unwrap_or(0.0) >= 0.5,
        );
        h.check(
            "fix: reading /etc/passwd routes to CONFIRM, not BLOCK",
            matches!(
                laya_workflow::apps::agent_gate::route("cat /etc/passwd", &v2)["action"].as_str(),
                Some("CONFIRM") | Some("ALLOW")
            ),
        );

        // (3) `category` had no backend branch for two apps that share the qid
        // with different criteria, so default_choice returned the first key
        // ("billing" / "safe") for everything.
        let eq = laya_workflow::apps::email_triage::questions();
        let outage = be
            .decide(
                &json!({"subject": "ALL HANDS: prod outage", "body": "Payments API 500 since 14:32 UTC. Need hotfix before sprint end.", "sender": "sre@x.com"}),
                &eq,
            )
            .unwrap();
        h.check(
            "fix: an outage is categorised as technical, not the first key",
            outage.answer_value("category").unwrap().as_str() == Some("technical"),
        );
        let modq = laya_workflow::apps::content_moderation::questions();
        let hate = be
            .decide(&json!({"text": "All [group] are vermin who should be deported.", "source": "user"}), &modq)
            .unwrap();
        h.check(
            "fix: a hateful message is not the first key ('safe')",
            hate.answer_value("category").unwrap().as_str() == Some("hateful"),
        );

        // (4) Word-boundary bug: `"need"` matched inside `"needed"`, so a message
        // saying "No action needed." was classified as needing a reply.
        let fyi = be
            .decide(
                &json!({"subject": "Office closed Monday", "body": "Reminder office closed Monday. No action needed.", "sender": "mgr@acme.com"}),
                &eq,
            )
            .unwrap();
        h.check(
            "fix: 'needed' no longer reads as 'needs a reply'",
            fyi.prob("needs_reply", "B").unwrap_or(0.0) < 0.5,
        );
        h.check(
            "fix: a no-action notice routes to FYI_ONLY",
            laya_workflow::apps::email_triage::route(&fyi)["action"].as_str() == Some("FYI_ONLY"),
        );

        // (5) `clarity` keyed only off typo words, so a one-word "ok" reply scored
        // "crystal clear"; brevity is the signal that it conveys too little.
        let dq = laya_workflow::apps::draft_scorer::questions();
        let short = be
            .decide(&json!({"audience": "manager", "text": "ok"}), &dq)
            .unwrap();
        let long = be
            .decide(
                &json!({"audience": "team", "text": "Hey everyone, I will be out of office next week and back on the 15th. Ping me if anything is urgent."}),
                &dq,
            )
            .unwrap();
        h.check(
            "fix: a one-word reply is not 'crystal clear'",
            short
                .answer_value("clarity")
                .unwrap()
                .as_f64()
                .unwrap_or(9.0)
                < long
                    .answer_value("clarity")
                    .unwrap()
                    .as_f64()
                    .unwrap_or(0.0),
        );
        h.check(
            "fix: a terse reply routes away from send_now",
            laya_workflow::apps::draft_scorer::route(&short)["suggestion"].as_str()
                != Some("send_now"),
        );

        // (6) `tone` only looked for two hostile phrases, so a resignation letter
        // scored "neutral" and never reached the sleep branch.
        let resign = be
            .decide(
                &json!({"audience": "manager", "text": "I've decided to leave. Here's my 2-week plan and handoffs."}),
                &dq,
            )
            .unwrap();
        h.check(
            "fix: a high-stakes message is not scored as neutral-positive",
            resign.answer_value("tone").unwrap().as_f64().unwrap_or(9.0) < 2.0,
        );

        // (8) The `mcp` capability had two bugs that made an authenticated
        // streamable-HTTP server unreachable: no `initialize` / `Mcp-Session-Id`
        // handshake, and no way to send auth headers at all.
        //
        // The handshake is proven end-to-end against a real MCP server in
        // docs/benchmarks/laya_capability_live_readiness_20260926.md (status 200
        // + session). Here we assert the request *shape* without a TCP server:
        // a socket-based mock was flaky because ureq pools connections and the
        // second POST could arrive on the first socket.
        {
            let mut npol = capability::Policy::default();
            npol.allow_hosts = vec!["127.0.0.1".to_string()];

            // A server that is not reachable still fails loudly (not silently),
            // and the config error for a missing url is explicit.
            // P149: a url-less mcp can never run, so it is now rejected at
            // *parse* (validate/build), not deferred to a run-time call error.
            let e = match capability::registry_from(
                &[(
                    "mcp_no_url",
                    json!({"kind": "mcp", "transport": "http", "tool": "t"}),
                )],
                Some(npol.clone()),
            ) {
                Ok(_) => String::new(),
                Err(err) => err.to_string(),
            };
            h.check(
                "fix: mcp http without a url is rejected up front",
                e.contains("needs \"url\""),
            );

            // Headers are part of the parsed capability, so an authenticated
            // server is configurable at all (this field did not exist before).
            let reg2 = capability::registry_from(
                &[(
                    "mcp_auth",
                    json!({
                        "kind": "mcp", "transport": "http",
                        "url": "http://127.0.0.1:1/mcp", "tool": "t",
                        "headers": {"x-api-key": "${env.LAYA_TEST_MCP_KEY}"},
                        "timeout_ms": 1500
                    }),
                )],
                Some(npol.clone()),
            )
            .unwrap();
            // A header built from an unset secret must fail closed rather than
            // sending a literal (the old behaviour produced "Bearer null").
            std::env::remove_var("LAYA_TEST_MCP_KEY");
            let with_key = json!({"token": "Bearer ${env.LAYA_TEST_MCP_KEY}"});
            let e2 = reg2
                .call("mcp_auth", &with_key, &json!({}))
                .unwrap_err()
                .to_string();
            h.check(
                "fix: an unset secret in a header fails closed",
                e2.contains("unresolved"),
            );
            h.check(
                "fix: the failure names the missing reference",
                e2.contains("LAYA_TEST_MCP_KEY"),
            );
        }

        // (6b) `match_mid_any` adds the ambiguous middle band: strong-positive
        // still wins outright, a mid token lands on `p_mid` (default 0.5), and
        // only an outright miss gets `p_miss`. This is what lets a threshold
        // rule's middle fallback mean "uncertain, hand to a human" rather than
        // silently collapsing to "no" (the two-band p_hit/p_miss shape).
        let noul_mid = json!({
            "q": {"type": "noul", "instructions": "x",
                  "heuristic": {"field": "route",
                                "match_any": ["high", "yes"],
                                "match_mid_any": ["mid", "unclear"],
                                "p_hit": 0.9, "p_miss": 0.1}}
        });
        let ask = |state: Value| {
            be.decide(&state, &noul_mid).unwrap().answer_value("q").unwrap().as_f64().unwrap_or(-1.0)
        };
        h.eq("mid-band: strong hit stays at p_hit", ask(json!({"route": "high"})), 0.9);
        h.eq("mid-band: strong miss stays at p_miss", ask(json!({"route": "low"})), 0.1);
        h.eq("mid-band: mid token returns p_mid (0.5 default)", ask(json!({"route": "mid"})), 0.5);
        h.eq("mid-band: unclear token also lands on p_mid", ask(json!({"route": "unclear"})), 0.5);
        h.eq(
            "mid-band: score honouring an explicit score_mid",
            be.decide(&json!({"route": "mid"}), &json!({
                "q": {"type": "score", "criteria": ["low", "mid", "high"],
                      "heuristic": {"field": "route", "match_any": ["high"],
                                    "match_mid_any": ["mid"],
                                    "score_hit": 2.0, "score_mid": 1.3, "score_miss": 0.5}}
            })).unwrap().answer_value("q").unwrap().as_f64().unwrap_or(-1.0),
            1.3,
        );
        h.eq(
            "mid-band: choice honouring p_mid on the B pole",
            be.decide(&json!({"route": "mid"}), &json!({
                "q": {"type": "choice", "criteria": {"A": "no", "B": "yes"},
                      "heuristic": {"field": "route", "match_any": ["high"],
                                    "match_mid_any": ["mid"], "p_hit": 0.9, "p_miss": 0.1}}
            })).unwrap().prob("q", "B").unwrap_or(-1.0),
            0.5,
        );
        h.check(
            "mid-band: choice keeps a strong miss strong-negative",
            be.decide(&json!({"route": "low"}), &json!({
                "q": {"type": "choice", "criteria": {"A": "no", "B": "yes"},
                      "heuristic": {"field": "route", "match_any": ["high"],
                                    "match_mid_any": ["mid"], "p_hit": 0.9, "p_miss": 0.1}}
            })).unwrap().prob("q", "B").unwrap_or(-1.0) < 0.2,
        );

        // (7) The whole reference set now passes without any learned policy.
        let dir = std::env::temp_dir().join("laya_fix_tests");
        let _ = std::fs::remove_dir_all(&dir);
        let lp = laya_workflow::accuracy::AccuracyLoop::open(&dir).unwrap();
        let s = lp.score(&lp.evaluate_reference());
        h.eq(
            "fix: all reference cases pass with no learned policy",
            s.correct,
            s.total,
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    // ── accuracy self-improvement loop ──────────────────────────────
}

pub fn test_accuracy(h: &mut Harness) {
    {
        use laya_workflow::accuracy::{self, AccuracyLoop, Policy, Sample};

        // (a) the evaluation set is the apps' reference cases and did not shrink
        h.eq(
            "accuracy: reference case count",
            accuracy::reference_case_count(),
            25,
        );

        // (b) baseline: the shipped policy's accuracy on those cases is measurable
        let dir = std::env::temp_dir().join("laya_acc_tests");
        let _ = std::fs::remove_dir_all(&dir);
        let lp = AccuracyLoop::open(&dir).unwrap();
        let base = lp.score(&lp.evaluate_reference());
        h.eq("accuracy: baseline totals", base.total, 25);
        h.check(
            "accuracy: baseline is measured (not assumed)",
            base.accuracy > 0.0,
        );
        // The shipped heuristic backend is now correct on every reference case
        // (the polarity / word-boundary / category bugs are fixed), so a healthy
        // baseline has no misses. Assert that, and separately assert the scorer
        // still *detects* misses on a deliberately broken backend.
        h.eq("accuracy: shipped baseline is clean", base.misses.len(), 0);
        {
            use laya_workflow::workflow::{Decide, Verdict};
            struct Wrong;
            impl Decide for Wrong {
                fn decide(&self, _s: &Value, q: &Value) -> anyhow::Result<Verdict> {
                    let mut answers = std::collections::HashMap::new();
                    for qid in q
                        .as_object()
                        .map(|o| o.keys().cloned().collect::<Vec<_>>())
                        .unwrap_or_default()
                    {
                        let mut probs = serde_json::Map::new();
                        probs.insert("A".to_string(), json!(1.0));
                        probs.insert("B".to_string(), json!(0.0));
                        answers.insert(
                            qid,
                            laya_workflow::workflow::Decision {
                                answer: json!("A"),
                                probabilities: probs,
                                confidence: 1.0,
                            },
                        );
                    }
                    Ok(Verdict {
                        answers,
                        input_tokens: 0,
                        latency_ms: 0.0,
                    })
                }
            }
            let mut broken = Vec::new();
            for c in laya_workflow::apps::all_reference_cases() {
                let observed = match Wrong.decide(&c.state, &json!({})) {
                    Ok(_) => "A".to_string(),
                    Err(_) => "?".to_string(),
                };
                broken.push(Sample {
                    app: c.app.to_string(),
                    state: c.state,
                    expected: c.expected.to_string(),
                    observed,
                    note: String::new(),
                });
            }
            let bs = lp.score(&broken);
            h.check(
                "accuracy: scorer still detects misses",
                !bs.misses.is_empty(),
            );
            h.check(
                "accuracy: a broken backend scores below the clean one",
                bs.accuracy < base.accuracy,
            );
        }

        // (c) hold-out split is deterministic and keeps both sides non-empty
        let all = lp.evaluate_reference();
        let (train, holdout) = AccuracyLoop::split(&all);
        h.check("accuracy: train split non-empty", !train.is_empty());
        h.check("accuracy: holdout split non-empty", !holdout.is_empty());
        h.eq(
            "accuracy: split is total",
            train.len() + holdout.len(),
            all.len(),
        );
        let (t2, h2) = AccuracyLoop::split(&all);
        h.eq(
            "accuracy: split is deterministic",
            (t2.len(), h2.len()),
            (train.len(), holdout.len()),
        );

        // (d) feedback is recorded and read back (production traffic can feed it)
        let sample = Sample {
            app: "agent_gate".to_string(),
            state: json!({"command": "echo hi", "intent": "say hi", "cwd": "/"}),
            expected: "ALLOW".to_string(),
            observed: "ALLOW".to_string(),
            note: "labelled by operator".to_string(),
        };
        lp.record(&[sample.clone()]).unwrap();
        let back = lp.samples().unwrap();
        h.eq("accuracy: feedback round-trips", back.len(), 1);
        h.check(
            "accuracy: feedback keeps the note",
            back[0].note.contains("operator"),
        );
        h.check("accuracy: correct() agrees", back[0].correct());

        // (e) label alternatives match apps.rs semantics ("ALLOW|CONFIRM")
        let alt = Sample {
            app: "a".to_string(),
            state: json!({}),
            expected: "ALLOW|CONFIRM".to_string(),
            observed: "CONFIRM".to_string(),
            note: String::new(),
        };
        h.check("accuracy: alternatives count as correct", alt.correct());

        // (f) the gate REJECTS an update that does not improve (rollback path)
        let bad = accuracy::Proposal {
            keywords: vec![(
                "agent_gate".to_string(),
                "ALLOW".to_string(),
                "zzzznomatch".to_string(),
            )],
            overrides: vec![(
                "agent_gate".to_string(),
                "zzzznomatch".to_string(),
                "BLOCK".to_string(),
            )],
            rationale: "deliberately useless".to_string(),
        };
        let base_hold = lp.score(&holdout).accuracy;
        let rep = lp
            .gated_apply(&bad, &train, &holdout, lp.score(&train).accuracy, base_hold)
            .unwrap();
        h.check("accuracy: bad update rejected", !rep.accepted);
        h.check(
            "accuracy: rejection explains itself",
            rep.reason.contains("did not improve"),
        );
        h.eq(
            "accuracy: rejected update does not bump revision",
            rep.revision_to,
            rep.revision_from,
        );

        // (g) the gate ACCEPTS a real improvement — and the score goes up
        // With a clean baseline the loop must decline to change anything: it
        // reports that no proposal follows from the (empty) miss set rather than
        // fabricating one. Accepting a no-op update would be a false claim.
        let step = lp.step().unwrap();
        h.check(
            "accuracy: no update invented for a clean baseline",
            !step.accepted,
        );
        h.check(
            "accuracy: clean baseline reports no proposal",
            step.reason.contains("no proposal"),
        );
        h.check(
            "accuracy: hold-out not regressed by a no-op round",
            step.holdout_after + f64::EPSILON >= step.holdout_before,
        );

        // (h) persistence: a *fresh* loop sees the accepted revision and its gain
        let reopened = AccuracyLoop::open(&dir).unwrap();
        h.eq(
            "accuracy: revision unchanged for a clean baseline",
            reopened.policy().revision,
            0,
        );
        let after = reopened.score(&reopened.evaluate_reference());
        h.check(
            "accuracy: reopened policy still scores the clean baseline",
            (after.accuracy - base.accuracy).abs() < f64::EPSILON,
        );
        // The reported score and the gated score use the same evaluator.
        let recomputed = reopened.score(&accuracy::reference_samples(reopened.policy()));
        h.eq(
            "accuracy: reported score matches re-evaluation",
            recomputed.correct,
            after.correct,
        );

        // (i) history is append-only and auditable
        let hist = std::fs::read_to_string(dir.join("rounds.jsonl")).unwrap();
        h.check(
            "accuracy: round history recorded",
            hist.lines().count() >= 2,
        );
        h.check(
            "accuracy: history is JSON per line",
            hist.lines().all(|l| l.starts_with('{')),
        );

        // (j) a hand-written policy applies overrides deterministically
        let mut p = Policy::default();
        p.overrides
            .entry("agent_gate".to_string())
            .or_default()
            .insert("echo".to_string(), "ALLOW".to_string());
        let scored = accuracy::reference_samples(&p);
        let echo_case = scored
            .iter()
            .find(|s| s.app == "agent_gate" && s.state["command"].as_str() == Some("echo hello"));
        h.check(
            "accuracy: override applies to the matching case",
            echo_case.map(|s| s.observed == "ALLOW").unwrap_or(false),
        );

        let _ = std::fs::remove_dir_all(&dir);
    }
}

/// P41: spec-declared offline determinism — a question that declares
/// `heuristic.answer_field` is answered from that state field *even when the
/// question name also has a bespoke handler* (e.g. the shared dialogue-policy
/// "intent"), and a dynamic-criteria (`criteria.from_state`) question accepts
/// a state-supplied id that is not a literal criteria key. Together these let
/// dialogue_policy and span_selection walk their whole graph offline.
pub fn test_answer_field_authority(h: &mut Harness) {
    {
        use laya_workflow::backend::HeuristicBackend;
        use laya_workflow::workflow::Decide;
        let be = HeuristicBackend;

        // A question that also has a *named* handler ("intent") still honours
        // its declared `heuristic.answer_field`: the offline verdict comes from
        // the state field, so a spec can drive a shared question name
        // deterministically — this is what lets dialogue_policy walk its whole
        // DFA offline.
        let q_intent = json!({"intent": {"type": "choice", "instructions": "intent",
            "criteria": {"greet": "g", "escalate": "e", "abort": "a"},
            "heuristic": {"answer_field": "i_greet"}}});
        let v_named = be.decide(&json!({"i_greet": "abort"}), &q_intent).unwrap();
        h.eq(
            "p41: answer_field wins over the named intent handler",
            v_named.answer_value("intent").unwrap().as_str().unwrap_or(""),
            "abort",
        );

        // Dynamic criteria (`criteria.from_state`): the real options are
        // materialised at decide time from a state list, so a state-supplied id
        // that is not a literal criteria key is a valid pick — *when the state
        // actually offers it*. The backend materialises the ids itself (P103), so
        // an id the state does not offer fails closed to `none`, never silently.
        let q_dyn = json!({"bc": {"type": "choice", "instructions": "pick",
            "criteria": {"from_state": "candidates", "id_key": "id", "value_key": "value", "none": "nothing"},
            "heuristic": {"answer_field": "selected_id"}}});
        let v_dyn = be
            .decide(
                &json!({"selected_id": "candidate_0",
                        "candidates": [{"id": "candidate_0", "value": "a@b.com"},
                                       {"id": "candidate_1", "value": "c@d.com"}]}),
                &q_dyn,
            )
            .unwrap();
        h.eq(
            "p41: dynamic-criteria answer_field accepts a state-supplied id the state offers",
            v_dyn.answer_value("bc").unwrap().as_str().unwrap_or(""),
            "candidate_0",
        );
        // The id is *not* in the state's candidate list: the offline heuristic must
        // not trust it — that would silently take the option (and the edge's route
        // branch) for a pick the candidate list never offered.
        let v_dyn_bad = be
            .decide(
                &json!({"selected_id": "candidate_9",
                        "candidates": [{"id": "candidate_0", "value": "a@b.com"}]}),
                &q_dyn,
            )
            .unwrap();
        h.eq(
            "p41: dynamic-criteria id outside the state's candidates fails closed to none",
            v_dyn_bad.answer_value("bc").unwrap().as_str().unwrap_or(""),
            "none",
        );
        let v_dyn_none = be.decide(&json!({}), &q_dyn).unwrap();
        h.eq(
            "p41: dynamic-criteria missing id fails closed to none",
            v_dyn_none.answer_value("bc").unwrap().as_str().unwrap_or(""),
            "none",
        );

        // The smoke gate treats a multi-node state with no expect entry as a
        // hard failure — `missing_trace_pins(Null, n)` is non-empty — so smoke
        // and the P38 regression test can never silently disagree.
        h.check(
            "p41: a multi-node state with no expect entry is rejected",
            !laya_workflow::dsl_smoke::missing_trace_pins(&Value::Null, 2).is_empty(),
        );

        // P252: `heuristic.answer` is not documentation; it owns the answer. Use a
        // shared qid with a named handler and a state whose text would otherwise
        // select `authenticate`, while the declared fixed answer is `abort` (not
        // the first criterion either).
        let q_fixed = json!({"intent": {"type": "choice", "instructions": "intent",
            "criteria": {"greet": "g", "authenticate": "auth", "abort": "a"},
            "heuristic": {"answer": "abort"}}});
        let v_fixed = be.decide(&json!({"text": "authenticate with mfa_code"}), &q_fixed).unwrap();
        let d_fixed = v_fixed.answers.get("intent").unwrap();
        h.eq(
            "p252: heuristic.answer beats the named handler and first criterion",
            d_fixed.as_str(),
            "abort".to_string(),
        );
        h.check(
            "p252: heuristic.answer gives the declared option probability 1",
            (d_fixed.prob("abort") - 1.0).abs() < 1e-12,
        );
        h.check(
            "p252: heuristic.answer gives the declared option confidence 1",
            (d_fixed.confidence - 1.0).abs() < 1e-12,
        );

        // P245: a declared `match_any` / `match_rules` / `extract_numeric` heuristic must
        // win over a *named* Rust handler for the same question id, exactly as
        // `answer_field` does above. Before P245 the named arms sat between the
        // `answer_field` arm and the `match_*` guards, so a spec that migrated a
        // built-in question (`risk`, `is_destructive`, …) was silently ignored — the
        // "new project == new JSON only" promise was false precisely where it mattered.
        h.check(
            "p245: `risk` still has a named handler, so the probe below is meaningful",
            laya_workflow::backend::has_builtin_handler("risk"),
        );
        let q_risk = json!({"risk": {"type": "score", "instructions": "how risky?",
            "criteria": ["low", "mid", "high"],
            "heuristic": {"match_any": ["zzz_unique_marker"], "score_hit": 3.0, "score_miss": 0.0}}});
        let v_hit = be.decide(&json!({"note": "zzz_unique_marker"}), &q_risk).unwrap();
        h.check(
            "p245: a declared `match_any` beats the named `risk` handler on a hit",
            v_hit.answer_value("risk").unwrap().as_f64() == Some(3.0),
        );
        let v_miss = be.decide(&json!({"note": "nothing"}), &q_risk).unwrap();
        h.check(
            "p245: ... and on a miss — the declared heuristic owns the answer outright",
            v_miss.answer_value("risk").unwrap().as_f64() == Some(0.0),
        );
        // A question with no declaration still falls to the named handler (0.2 here):
        // the reordering changes *precedence*, not the default.
        let q_plain = json!({"risk": {"type": "score", "instructions": "how risky?",
            "criteria": ["low", "mid", "high"]}});
        let v_plain = be.decide(&json!({"note": "nothing"}), &q_plain).unwrap();
        h.check(
            "p245: with no declaration the named handler is still the default (risk = 0.2)",
            v_plain.answer_value("risk").unwrap().as_f64() == Some(0.2),
        );
        // And the *order* itself is pinned, so no later edit can quietly move the
        // guards back behind the named handlers: in `src/backend.rs` the
        // `answer_field` arm comes first, then `match_rules`, then `match_any`, and
        // only then the first literal question-id arm.
        {
            let src = std::fs::read_to_string(laya_workflow::bdd::repo_root().join("src/backend.rs"))
                .expect("read src/backend.rs");
            let pos = |needle: &str| src.find(needle).unwrap_or_else(|| panic!("missing {needle}"));
            let answer_field = pos("h.get(\"answer_field\")");
            let match_rules = pos("h.get(\"match_rules\")");
            let match_any = pos("h.get(\"match_any\")");
            let first_literal = pos("\"is_destructive\" => {");
            h.check(
                "p245: the declared-heuristic guards sit before the named-handler arms",
                answer_field < match_rules && match_rules < match_any && match_any < first_literal,
            );
        }

        // P246: the migration of the built-in handlers into specs needed two
        // compile shapes the vocabulary could not spell.
        //   (a) A score that is one needle *set* against a single miss value:
        //       `score "..." when the text contains ... scoring X miss Y`. The
        //       choice step could not stand in - it writes `p_hit` / `p_miss`,
        //       which P184 refuses above 1.0 and the score reader never reads.
        //   (b) A middle band with no `field`: `answer "..." with mid M when the
        //       text contains ...`, which matches the whole serialised state the
        //       way the built-in handlers do (the pre-P246 step accepted a field
        //       and silently dropped it).
        let src46 = r#"
@decision
Feature: p246shapes
  Scenario: gate
    Given a score question "risk" asks "how risky?"
      | level |
      | low   |
      | mid   |
      | high  |
    And score "2.0" when the text contains "rm ", "drop " or "delete" scoring 2.0 miss 0.2
    And a choice question "exfil" asks "does it phone home with a secret?"
      | answer | meaning |
      | A      | no      |
      | B      | yes     |
    And answer "B" when the text contains "curl " with hit 0.9 and miss 0.1
    And answer "B" with mid 0.45 when the text contains "/etc/passwd"
    Then label "HOT" when "risk" is at least 2
    And label "LEAK" when the answer to "exfil" is "B"
    And label "COLD" otherwise
"#;
        let f46 = laya_workflow::bdd::parse(src46, "<p246>").expect("parse p246 feature");
        let s46 = laya_workflow::bdd::compile_decision(&f46, &json!({})).expect("compile p246 shapes");
        let q46 = &s46["nodes"][0]["questions"]["risk"]["heuristic"];
        h.eq(
            "p246: `score ... when the text contains ... scoring X miss Y` writes score_hit",
            q46["score_hit"].clone(),
            json!(2.0),
        );
        h.eq("p246: ... and score_miss", q46["score_miss"].clone(), json!(0.2));
        h.check(
            "p246: ... and no probability key on a score question",
            q46.get("p_hit").is_none() && q46.get("p_miss").is_none(),
        );
        h.eq(
            "p246: ... keeping every needle of the set",
            q46["match_any"].clone(),
            json!(["rm ", "drop ", "delete"]),
        );
        h.check(
            "p246: ... and declaring no field, so it matches the whole serialised state",
            q46.get("field").is_none(),
        );
        let h46 = &s46["nodes"][0]["questions"]["exfil"]["heuristic"];
        h.eq(
            "p246: a mid band spelled `with mid M when the text contains ...` writes match_mid_any",
            h46["match_mid_any"].clone(),
            json!(["/etc/passwd"]),
        );
        h.eq("p246: ... at the declared p_mid", h46["p_mid"].clone(), json!(0.45));
        h.check("p246: ... with no field", h46.get("field").is_none());

        // Behaviour, not shape: the declared heuristics own the two questions.
        let be46 = HeuristicBackend;
        let qs46 = &s46["nodes"][0]["questions"];
        let hit46 = be46.decide(&json!({"command": "rm -rf /"}), qs46).unwrap();
        h.check(
            "p246: the declared score heuristic answers 2.0 on a hit",
            hit46.answer_value("risk").unwrap().as_f64() == Some(2.0),
        );
        let miss46 = be46.decide(&json!({"command": "ls -la /tmp"}), qs46).unwrap();
        h.check(
            "p246: ... and 0.2 on a miss (not the 2.0 / 0.5 default pair)",
            miss46.answer_value("risk").unwrap().as_f64() == Some(0.2),
        );
        let mid46 = be46.decide(&json!({"command": "cat /etc/passwd"}), qs46).unwrap();
        h.check(
            "p246: a field-less mid band reads the whole state at its p_mid",
            (mid46.prob("exfil", "B").unwrap_or(-1.0) - 0.45).abs() < 1e-9,
        );

        // The inverse direction: both shapes must come back out of `import`, and
        // the round trip must be a fixed point on the decision graph (the shape the
        // `round-trip` / `import-check` gates hold the whole corpus to).
        let back46 = laya_workflow::bdd::import_spec(&s46).expect("import the p246 spec");
        h.check(
            "p246: the score step round-trips through `import`",
            back46.contains("scoring 2 miss 0.2"),
        );
        h.check(
            "p246: the field-less mid step round-trips through `import`",
            back46.contains("with mid 0.45 when the text contains"),
        );
        let f46b = laya_workflow::bdd::parse(&back46, "<p246b>").expect("parse the imported p246 feature");
        let s46b = laya_workflow::bdd::compile_decision(&f46b, &json!({})).expect("recompile the imported p246 feature");
        h.check(
            "p246: feature -> spec -> feature -> spec is a fixed point on the graph",
            s46["nodes"] == s46b["nodes"],
        );

        // P247: the multi-way rule step's `field` is optional, exactly as the
        // backend already reads it. content_policy_gate's moderation `category` is a
        // six-way pick the built-in handler computes from the whole serialised state,
        // and `answer by rules on the field "F"` demanded a key that does not exist --
        // the same seam P246 opened for the mid band and the score step.
        let src47 = r#"
@decision
Feature: p247rules
  Scenario: moderate
    Given a choice question "category" asks "Best single label?"
      | answer      | meaning |
      | safe        | no concerns |
      | hateful     | hate or harassment |
      | threatening | violent threat |
      | spam        | spam or promo |
    And answer by rules on the text with hit 0.9 and fallback "safe"
      | label       | needles |
      | hateful     | "vermin", "deport" |
      | threatening | "kill you" |
      | spam        | "get rich" |
    And a choice question "flavoured" asks "Which flavour?"
      | answer | meaning |
      | A      | no |
      | B      | yes |
    And answer by rules on the field "note" with hit 0.94 and fallback "A"
      | label | needles |
      | B     | "yes" |
    Then label "WARM" when the answer to "category" is one of "hateful", "threatening", "spam"
    And label "COOL" otherwise
"#;
        let f47 = laya_workflow::bdd::parse(src47, "<p247>").expect("parse p247 feature");
        let s47 = laya_workflow::bdd::compile_decision(&f47, &json!({})).expect("compile p247 shapes");
        let c47 = &s47["nodes"][0]["questions"]["category"]["heuristic"];
        h.check(
            "p247: `answer by rules on the text ...` writes match_rules with no field",
            c47.get("field").is_none() && c47["match_rules"].is_object(),
        );
        h.eq("p247: ... at the declared hit", c47["p_hit"].clone(), json!(0.9));
        h.eq(
            "p247: ... and the declared fallback",
            c47["fallback"].clone(),
            json!("safe"),
        );
        h.check(
            "p247: ... keeping the labels in declared order",
            c47["match_rules"]
                .as_object()
                .unwrap()
                .keys()
                .cloned()
                .collect::<Vec<String>>()
                == vec![
                    "hateful".to_string(),
                    "threatening".to_string(),
                    "spam".to_string(),
                ],
        );
        h.eq(
            "p247: the field form still declares its field",
            s47["nodes"][0]["questions"]["flavoured"]["heuristic"]["field"].clone(),
            json!("note"),
        );

        // Behaviour, not shape: the declared rules own the multi-way pick.
        let be47 = HeuristicBackend;
        let qs47 = &s47["nodes"][0]["questions"];
        let pick47 = |state: serde_json::Value| -> String {
            be47.decide(&state, qs47)
                .unwrap()
                .answer_value("category")
                .unwrap()
                .as_str()
                .unwrap()
                .to_string()
        };
        h.eq(
            "p247: a hit picks its label",
            pick47(json!({"text": "you vermin"})),
            "hateful".to_string(),
        );
        h.eq(
            "p247: a miss falls back to the declared label, not the `other` default",
            pick47(json!({"text": "thanks for the help"})),
            "safe".to_string(),
        );
        h.eq(
            "p247: declared order wins over alphabetical order (`spam` < `threatening`)",
            pick47(json!({"text": "get rich and kill you"})),
            "threatening".to_string(),
        );

        // The inverse direction: the field-less rule step must come back out of
        // `import`, and the round trip must be a fixed point on the graph (the shape
        // `round-trip` / `import-check` hold the whole corpus to).
        let back47 = laya_workflow::bdd::import_spec(&s47).expect("import the p247 spec");
        h.check(
            "p247: the field-less rule step round-trips through `import`",
            back47.contains("answer by rules on the text with hit 0.9 and fallback \"safe\""),
        );
        h.check(
            "p247: ... and the field form keeps its field",
            back47.contains("answer by rules on the field \"note\" with hit 0.94 and fallback \"A\""),
        );
        let f47c =
            laya_workflow::bdd::parse(&back47, "<p247b>").expect("parse the imported p247 feature");
        let s47c = laya_workflow::bdd::compile_decision(&f47c, &json!({}))
            .expect("recompile the imported p247 feature");
        h.check(
            "p247: feature -> spec -> feature -> spec is a fixed point on the graph",
            s47["nodes"] == s47c["nodes"],
        );

        // P248: content_safety_guard needs two more shapes. Six of its choice
        // questions are conjunctions (“one semantic needle AND one context
        // needle”), while `match_any` means only “one of these needles”; its
        // score question is a nine-row ordered ladder, while `score_hit` has one
        // hit value. Spell both as rules rather than Rust handlers.
        let src48 = r#"
@decision
Feature: p248shapes
  Scenario: safety
    Given a choice question "age_blocking" asks "is it age blocking?"
      | answer | meaning |
      | A      | no      |
      | B      | yes     |
    And answer "B" when the text matches every group with hit 0.9 and miss 0.05
      | group    | needles |
      | semantic | "graphic_violence", "self_harm" |
      | audience | "child", "teen" |
    And a choice question "scoped" asks "is the note age blocking?"
      | answer | meaning |
      | A      | no      |
      | B      | yes     |
    And answer "B" when the field "note" matches every group with hit 0.91 and miss 0.06
      | group    | needles |
      | semantic | "artistic_nudity" |
      | audience | "child" |
    And a score question "effective_severity" asks "how severe?"
      | level |
      | 0     |
      | 50    |
      | 90    |
      | 100   |
    And score "effective_severity" by rules on the text miss 10
      | score | needles |
      | 100   | "targeted_abuse", "prohibited" |
      | 95    | "graphic_violence", "self_harm" |
      | 60    | "violence_simulation" |
      | 45    | "contact_information" |
    When any other answer goes to "STOP"
    Then label "BLOCK" when the answer to "age_blocking" is "B"
    And label "BLOCK" when "effective_severity" is at least 90
    And label "REVIEW" when "effective_severity" is at least 45
    And label "ALLOW" otherwise
"#;
        let f48 = laya_workflow::bdd::parse(src48, "<p248>").expect("parse p248 feature");
        let s48 = laya_workflow::bdd::compile_decision(&f48, &json!({})).expect("compile p248 shapes");
        let a48 = &s48["nodes"][0]["questions"]["age_blocking"]["heuristic"];
        h.eq(
            "p248: `matches every group` writes nested OR-groups",
            a48["match_all"].clone(),
            json!([["graphic_violence", "self_harm"], ["child", "teen"]]),
        );
        h.eq("p248: ... at the declared hit", a48["p_hit"].clone(), json!(0.9));
        h.eq("p248: ... and miss", a48["p_miss"].clone(), json!(0.05));
        h.check("p248: ... with no field (whole serialised state)", a48.get("field").is_none());
        h.eq(
            "p248: the field form keeps its named match space",
            s48["nodes"][0]["questions"]["scoped"]["heuristic"]["field"].clone(),
            json!("note"),
        );
        let e48 = &s48["nodes"][0]["questions"]["effective_severity"]["heuristic"];
        h.eq(
            "p248: `score by rules` writes an ordered numeric map",
            e48["score_rules"].clone(),
            json!({
                "100": ["targeted_abuse", "prohibited"],
                "95": ["graphic_violence", "self_harm"],
                "60": ["violence_simulation"],
                "45": ["contact_information"]
            }),
        );
        h.eq("p248: ... and the miss score", e48["score_miss"].clone(), json!(10.0));

        // Behaviour: the AND cannot fire on one group alone, and ladder order --
        // not numerical or alphabetical order -- picks the first matching row.
        let be48 = HeuristicBackend;
        let qs48 = &s48["nodes"][0]["questions"];
        let choice48 = |state: serde_json::Value| -> String {
            be48.decide(&state, qs48)
                .unwrap()
                .answer_value("age_blocking")
                .unwrap()
                .as_str()
                .unwrap()
                .to_string()
        };
        h.eq(
            "p248: one group alone is a miss (true AND)",
            choice48(json!({"text": "graphic_violence for adults"})),
            "A".to_string(),
        );
        h.eq(
            "p248: one hit in every group fires",
            choice48(json!({"text": "graphic_violence for a teen"})),
            "B".to_string(),
        );
        h.eq(
            "p248: a declared field does not cross-fire into the envelope",
            be48
                .decide(&json!({"text": "artistic_nudity child", "note": "science for a child"}), qs48)
                .unwrap()
                .answer_value("scoped")
                .unwrap()
                .as_str()
                .unwrap(),
            "A",
        );
        let sev48 = |state: serde_json::Value| -> f64 {
            be48.decide(&state, qs48)
                .unwrap()
                .answer_value("effective_severity")
                .unwrap()
                .as_f64()
                .unwrap()
        };
        h.eq(
            "p248: the first matching score row wins",
            sev48(json!({"text": "self_harm targeted_abuse"})),
            100.0,
        );
        h.eq(
            "p248: a lower-priority row still wins when it is the only match",
            sev48(json!({"text": "please send contact_information"})),
            45.0,
        );
        h.eq(
            "p248: no row falls to score_miss",
            sev48(json!({"text": "educational_science adult"})),
            10.0,
        );
        let sev_conf48 = |state: serde_json::Value| -> (f64, f64, f64, f64) {
            let v = be48.decide(&state, qs48).unwrap();
            (
                v.confidence("effective_severity").unwrap(),
                v.prob("effective_severity", "0").unwrap(),
                v.prob("effective_severity", "1").unwrap(),
                v.prob("effective_severity", "2").unwrap(),
            )
        };
        h.eq(
            "p249: a wide-scale ladder keeps the handler's miss confidence",
            sev_conf48(json!({"text": "educational_science adult"})),
            (0.9, 0.9, 0.02, 0.02),
        );
        h.eq(
            "p249: ... and its review band probability",
            sev_conf48(json!({"text": "violence_simulation"})),
            (0.9, 0.02, 0.9, 0.02),
        );
        h.eq(
            "p249: ... and its blocking band probability",
            sev_conf48(json!({"text": "self_harm targeted_abuse"})),
            (0.9, 0.02, 0.02, 0.9),
        );

        // Both new table steps must survive import and recompile bit-for-bit at
        // the graph level (the gates the whole 177-spec corpus is held to).
        let back48 = laya_workflow::bdd::import_spec(&s48).expect("import the p248 spec");
        h.check(
            "p248: the conjunction step round-trips through `import`",
            back48.contains("matches every group with hit 0.9 and miss 0.05"),
        );
        h.check(
            "p248: the score ladder round-trips through `import`",
            back48.contains("score \"effective_severity\" by rules on the text miss 10"),
        );
        let f48b = laya_workflow::bdd::parse(&back48, "<p248b>").expect("parse the imported p248 feature");
        let s48b = laya_workflow::bdd::compile_decision(&f48b, &json!({}))
            .expect("recompile the imported p248 feature");
        h.check(
            "p248: feature -> spec -> feature -> spec is a fixed point on the graph",
            s48["nodes"] == s48b["nodes"],
        );

        // P250: intrusion-style verdicts need ordered labels where one label can
        // require several signal classes simultaneously. `match_rules` is only an
        // OR over needles; `match_all` is only one binary answer. This shape maps
        // label -> alternative conjunctions -> OR-groups -> needles.
        let src50 = r#"
@decision
Feature: p250shapes
  Scenario: verdict
    Given a choice question "verdict" asks "which verdict?"
      | answer    | meaning |
      | allow     | none    |
      | monitor   | weak    |
      | challenge | strong  |
      | block     | urgent  |
    And answer by all-group rules on the text with hit 0.9 and miss 0.02 and fallback "allow" at 0.88
      | label     | all of |
      | block     | "signature" |
      | block     | "failed_login", "5 failed" | "port_scan" |
      | challenge | "failed_login", "5 failed" |
      | challenge | "unusual_country" | "off_hours" |
      | monitor   | "unusual_country" |
    When any other answer goes to "STOP"
    Then the primary question is "verdict"
"#;
        let f50 = laya_workflow::bdd::parse(src50, "<p250>").expect("parse p250 feature");
        let s50 = laya_workflow::bdd::compile_decision(&f50, &json!({})).expect("compile p250 shape");
        let v50 = &s50["nodes"][0]["questions"]["verdict"]["heuristic"];
        h.eq(
            "p250: all-group rules preserve label, alternative and OR-group nesting",
            v50["match_rules_all"].clone(),
            json!({
                "block": [
                    [["signature"]],
                    [["failed_login", "5 failed"], ["port_scan"]]
                ],
                "challenge": [
                    [["failed_login", "5 failed"]],
                    [["unusual_country"], ["off_hours"]]
                ],
                "monitor": [[["unusual_country"]]]
            }),
        );
        h.eq("p250: ... hit probability", v50["p_hit"].clone(), json!(0.9));
        h.eq("p250: ... miss probability", v50["p_miss"].clone(), json!(0.02));
        h.eq("p250: ... fallback", v50["fallback"].clone(), json!("allow"));
        h.eq("p250: ... fallback confidence", v50["p_fallback"].clone(), json!(0.88));

        let be50 = HeuristicBackend;
        let qs50 = &s50["nodes"][0]["questions"];
        let pick50 = |state: serde_json::Value| -> String {
            be50.decide(&state, qs50)
                .unwrap()
                .answer_value("verdict")
                .unwrap()
                .as_str()
                .unwrap()
                .to_string()
        };
        h.eq("p250: a single-group alternative fires", pick50(json!({"text":"signature"})), "block".to_string());
        h.eq("p250: one class alone is not enough for the AND row", pick50(json!({"text":"5 failed routine"})), "challenge".to_string());
        h.eq("p250: an OR variant still satisfies its group", pick50(json!({"text":"5 failed port_scan"})), "block".to_string());
        h.eq("p250: two weak classes fire the challenge conjunction", pick50(json!({"text":"unusual_country off_hours"})), "challenge".to_string());
        h.eq("p250: one weak class falls to monitor", pick50(json!({"text":"unusual_country"})), "monitor".to_string());
        h.eq("p250: no rule uses the declared fallback", pick50(json!({"text":"routine access"})), "allow".to_string());
        let out50 = be50.decide(&json!({"text":"routine access"}), qs50).unwrap();
        h.eq("p250: fallback keeps its declared confidence", out50.confidence("verdict").unwrap(), 0.88);
        h.eq("p250: fallback probability is emitted", out50.prob("verdict", "allow").unwrap(), 0.88);
        h.eq("p250: non-fallback options use p_miss", out50.prob("verdict", "block").unwrap(), 0.02);

        let back50 = laya_workflow::bdd::import_spec(&s50).expect("import the p250 spec");
        h.check(
            "p250: the all-group rule step round-trips through import",
            back50.contains("answer by all-group rules on the text with hit 0.9 and miss 0.02 and fallback \"allow\" at 0.88"),
        );
        let f50b = laya_workflow::bdd::parse(&back50, "<p250b>").expect("parse the imported p250 feature");
        let s50b = laya_workflow::bdd::compile_decision(&f50b, &json!({}))
            .expect("recompile the imported p250 feature");
        h.check(
            "p250: feature -> spec -> feature -> spec is a fixed point on the graph",
            s50["nodes"] == s50b["nodes"],
        );

        // P253: migrate adaptive_risk_control without a qid-specific Rust arm.
        // Compile the checked-in feature, then reconstruct its pre-migration
        // question contract (no heuristics; the old 0/40/70 score anchors) and
        // compare every answer, confidence and probability emitted by the named
        // handlers to the spec-declared rules. The 20 anchor is not a new action
        // threshold: it declares the handler's existing low/mid/high read-out.
        let f53 = laya_workflow::bdd::parse(
            include_str!("../../bdd/workflows/ole_eval/adaptive_risk_control.feature"),
            "<p253-adaptive>",
        )
        .expect("parse the checked-in adaptive risk feature");
        let s53 = laya_workflow::bdd::compile_decision(&f53, &json!({}))
            .expect("compile the checked-in adaptive risk feature");
        let qs53 = &s53["nodes"][0]["questions"];
        let mut old53 = qs53.clone();
        for q in old53.as_object_mut().expect("adaptive questions").values_mut() {
            q.as_object_mut().expect("question object").remove("heuristic");
        }
        old53["composite_risk"]["criteria"] = json!([
            "0 clean", "40 reviewable", "70 deny", "100 maximum"
        ]);
        let be53 = HeuristicBackend;
        let p53_states = [
            json!({"text":"credential_stuffing micro_transaction_burst mfa_failed"}),
            json!({"text":"card_testing new_device_payment challenge_pending"}),
            json!({"text":"account_takeover recovery_then_withdrawal"}),
            json!({"text":"recovery"}),
            json!({"text":"micro_transaction"}),
            json!({"text":"new device repeated failure"}),
            json!({"text":"inconsistent session signals"}),
            json!({"text":"unknown routine event"}),
        ];
        for state in p53_states.iter() {
            let before = be53.decide(state, &old53).expect("old adaptive handlers");
            let after = be53.decide(state, qs53).expect("declared adaptive rules");
            let equivalent = ["intent", "behaviour", "mfa_failed_pending", "composite_risk"]
                .iter()
                .all(|qid| {
                    let answer_same = before.answer_value(qid).ok() == after.answer_value(qid).ok();
                    let conf_same =
                        (before.confidence(qid).unwrap_or(-1.0)
                            - after.confidence(qid).unwrap_or(-2.0))
                        .abs()
                            < 1e-12;
                    let choice_keys: Vec<String> = old53[*qid]["criteria"]
                        .as_object()
                        .map(|o| o.keys().cloned().collect())
                        .unwrap_or_default();
                    let score_keys: Vec<String> =
                        ["0", "1", "2"].iter().map(|k| k.to_string()).collect();
                    let keys: Vec<String> = if old53[*qid]["type"] == "choice" {
                        choice_keys
                    } else {
                        score_keys
                    };
                    let probs_same = keys.iter().all(|k| {
                        (before.prob(qid, k).unwrap_or(-1.0)
                            - after.prob(qid, k).unwrap_or(-2.0))
                        .abs()
                            < 1e-12
                    });
                    answer_same && conf_same && probs_same
                });
            h.check(
                "p253: adaptive answers/confidences/probabilities match the old handlers",
                equivalent,
            );
        }
        let bands25 = be53
            .decide(&json!({"text":"repeated_failures"}), qs53)
            .expect("adaptive low-middle band");
        h.eq(
            "p253: a 25 score keeps the handler's middle-band probabilities",
            (
                bands25.answer_value("composite_risk").unwrap().as_f64().unwrap(),
                bands25.prob("composite_risk", "0").unwrap(),
                bands25.prob("composite_risk", "1").unwrap(),
                bands25.prob("composite_risk", "2").unwrap(),
            ),
            (25.0, 0.02, 0.9, 0.02),
        );
        let bands45 = be53
            .decide(&json!({"text":"new_device_payment"}), qs53)
            .expect("adaptive high band");
        h.eq(
            "p253: a 45 score keeps the handler's high-band probabilities",
            (
                bands45.answer_value("composite_risk").unwrap().as_f64().unwrap(),
                bands45.prob("composite_risk", "0").unwrap(),
                bands45.prob("composite_risk", "1").unwrap(),
                bands45.prob("composite_risk", "2").unwrap(),
            ),
            (45.0, 0.02, 0.02, 0.9),
        );

        // P254: migrate support_ticket_router_hybrid without another Rust arm.
        // The checked-in category ladder includes shipping/account/hr even though
        // this node does not accept them as edges: the old shared category handler
        // returned those ids too, so preserving the fail-closed behaviour matters
        // more than making an unrelated text “nicer”. Severity keeps the handler's
        // odd-but-real four-token high-score rule and its 0.5 miss.
        let f54 = laya_workflow::bdd::parse(
            include_str!("../../bdd/workflows/routing/support_ticket_router_hybrid.feature"),
            "<p254-ticket-hybrid>",
        )
        .expect("parse the checked-in ticket hybrid feature");
        let s54 = laya_workflow::bdd::compile_decision(&f54, &json!({}))
            .expect("compile the checked-in ticket hybrid feature");
        let qs54 = json!({
            "category": s54["nodes"][0]["questions"]["category"].clone(),
            "severity": s54["nodes"][2]["questions"]["severity"].clone(),
        });
        let mut old54 = qs54.clone();
        for q in old54.as_object_mut().expect("ticket questions").values_mut() {
            q.as_object_mut().expect("question object").remove("heuristic");
        }
        let be54 = HeuristicBackend;
        let p54_states = [
            json!({"text":"outage"}),
            json!({"text":"refund"}),
            json!({"text":"pricing"}),
            json!({"text":"package"}),
            json!({"text":"login"}),
            json!({"text":"payroll"}),
            json!({"text":"production"}),
            json!({"text":"outage and refund"}),
        ];
        for state in p54_states.iter() {
            let before = be54.decide(state, &old54).expect("old ticket handlers");
            let after = be54.decide(state, &qs54).expect("declared ticket rules");
            let category_keys =
                ["billing", "technical", "sales", "shipping", "account", "hr", "other"];
            let equivalent = ["category", "severity"].iter().all(|qid| {
                let answer_same = before.answer_value(qid).ok() == after.answer_value(qid).ok();
                let conf_same = (before.confidence(qid).unwrap_or(-1.0)
                    - after.confidence(qid).unwrap_or(-2.0))
                    .abs()
                        < 1e-12;
                let keys: Vec<&str> = if *qid == "category" {
                    category_keys.to_vec()
                } else {
                    vec!["0", "1", "2"]
                };
                let probs_same = keys.iter().all(|k| {
                    (before.prob(qid, k).unwrap_or(-1.0)
                        - after.prob(qid, k).unwrap_or(-2.0))
                    .abs()
                        < 1e-12
                });
                answer_same && conf_same && probs_same
            });
            h.check(
                "p254: ticket category/severity match the old handlers",
                equivalent,
            );
        }
        let miss54 = be54
            .decide(&json!({"text":"outage with no risk words"}), &qs54)
            .expect("ticket severity miss");
        h.eq(
            "p254: a severity miss keeps the handler's ordinal read-out",
            (
                miss54.answer_value("severity").unwrap().as_f64().unwrap(),
                miss54.confidence("severity").unwrap(),
                miss54.prob("severity", "2").unwrap(),
            ),
            (0.5, 0.833, 0.5),
        );
        let hit54 = be54
            .decide(&json!({"text":"outage and 123-45-6789"}), &qs54)
            .expect("ticket severity hit");
        h.eq(
            "p254: a severity hit keeps the handler's high-band read-out",
            (
                hit54.answer_value("severity").unwrap().as_f64().unwrap(),
                hit54.confidence("severity").unwrap(),
                hit54.prob("severity", "2").unwrap(),
            ),
            (2.0, 1.0, 1.0),
        );

        // P255: migrate aml_screener. Its Rust score is genuinely additive --
        // independent signal groups each contribute once, then a sanctioned
        // override sets the total to 100. Represent that as generic
        // `score_sum` rows plus an optional cap; no qid-specific arm remains in
        // the migrated spec. The exhaustive loop compares every emitted answer,
        // confidence and probability across all 2^7 Boolean signal combinations.
        let f55 = laya_workflow::bdd::parse(
            include_str!("../../bdd/workflows/ole_eval/aml_screener.feature"),
            "<p255-aml>",
        )
        .expect("parse the checked-in AML feature");
        let s55 = laya_workflow::bdd::compile_decision(&f55, &json!({}))
            .expect("compile the checked-in AML feature");
        let qs55 = &s55["nodes"][0]["questions"];
        let score_h55 = &qs55["aml_risk"]["heuristic"];
        h.eq(
            "p255: score_sum preserves every independent point row",
            score_h55["score_sum"].clone(),
            json!([
                {"score":30.0,"needles":["crypto","bitcoin","ethereum"]},
                {"score":25.0,"needles":["large_amount","large amount","above threshold"]},
                {"score":35.0,"needles":["high_risk","high risk","belarus","russia","country_by","country_ru"]},
                {"score":20.0,"needles":["crypto_exchange","exchange"]},
                {"score":15.0,"needles":["near_threshold","near threshold"]},
                {"score":20.0,"needles":["charity_high_risk","charity"]},
                {"score":100.0,"needles":["sanctioned","country_cu","country_ir","country_kp","country_sy"]}
            ]),
        );
        h.eq("p255: the optional cap is declared", score_h55["score_max"].clone(), json!(100.0));

        let mut old55 = qs55.clone();
        for q in old55.as_object_mut().expect("AML questions").values_mut() {
            q.as_object_mut().expect("question object").remove("heuristic");
        }
        let be55 = HeuristicBackend;
        let signals55 = [
            "crypto",
            "large_amount",
            "belarus",
            "exchange",
            "near_threshold",
            "charity",
            "country_cu",
        ];
        let mut exhaustive55 = true;
        for mask in 0..(1_usize << signals55.len()) {
            let mut text = String::new();
            for (i, signal) in signals55.iter().enumerate() {
                if mask & (1 << i) != 0 {
                    if !text.is_empty() { text.push(' '); }
                    text.push_str(signal);
                }
            }
            let state = json!({"text": text});
            let before = be55.decide(&state, &old55).expect("old AML handlers");
            let after = be55.decide(&state, qs55).expect("declared AML rules");
            let choice_same = ["sanctioned_country", "crypto_category", "crypto_exchange_counterparty", "high_risk_country"]
                .iter()
                .all(|qid| {
                    before.answer_value(qid).ok() == after.answer_value(qid).ok()
                        && (before.confidence(qid).unwrap_or(-1.0)
                            - after.confidence(qid).unwrap_or(-2.0))
                            .abs()
                            < 1e-12
                        && ["A", "B"].iter().all(|k| {
                            (before.prob(qid, k).unwrap_or(-1.0)
                                - after.prob(qid, k).unwrap_or(-2.0))
                                .abs()
                                < 1e-12
                        })
                });
            let score_same = before.answer_value("aml_risk").ok() == after.answer_value("aml_risk").ok()
                && (before.confidence("aml_risk").unwrap_or(-1.0)
                    - after.confidence("aml_risk").unwrap_or(-2.0))
                    .abs()
                    < 1e-12
                && ["0", "1", "2"].iter().all(|k| {
                    (before.prob("aml_risk", k).unwrap_or(-1.0)
                        - after.prob("aml_risk", k).unwrap_or(-2.0))
                        .abs()
                        < 1e-12
                });
            if !choice_same || !score_same {
                exhaustive55 = false;
                eprintln!("p255 AML mismatch for mask {mask}: {text}");
                break;
            }
        }
        h.check("p255: all 128 AML signal combinations match the old handlers", exhaustive55);

        let score55 = |text: &str| -> f64 {
            be55.decide(&json!({"text":text}), qs55)
                .unwrap()
                .answer_value("aml_risk")
                .unwrap()
                .as_f64()
                .unwrap()
        };
        h.eq("p255: one additive row contributes once", score55("bitcoin"), 30.0);
        h.eq("p255: independent rows sum", score55("bitcoin large_amount"), 55.0);
        h.eq(
            "p255: equal-point rows remain independent",
            score55("exchange charity"),
            40.0,
        );
        h.eq(
            "p255: a 100-point sanctioned row plus lower rows is capped",
            score55("crypto large_amount belarus exchange country_cu"),
            100.0,
        );
        let bands55 = |text: &str| -> (f64, f64, f64, f64, f64) {
            let v = be55.decide(&json!({"text":text}), qs55).unwrap();
            (
                v.answer_value("aml_risk").unwrap().as_f64().unwrap(),
                v.confidence("aml_risk").unwrap(),
                v.prob("aml_risk", "0").unwrap(),
                v.prob("aml_risk", "1").unwrap(),
                v.prob("aml_risk", "2").unwrap(),
            )
        };
        h.eq(
            "p255: additive scores keep the handler's three read-out bands",
            (
                bands55("ordinary transaction"),
                bands55("large_amount"),
                bands55("crypto exchange charity"),
            ),
            (
                (0.0, 0.9, 0.9, 0.02, 0.02),
                (25.0, 0.9, 0.02, 0.9, 0.02),
                (70.0, 0.9, 0.02, 0.02, 0.9),
            ),
        );

        // The grammar also has the generic field form (and an omitted cap), so a
        // state-scoped additive score need not read the serialised envelope.
        let src55 = r#"
@decision
Feature: p255field
  Scenario: score
    Given a score question "scoped" asks "how risky?"
      | level |
      | 0     |
      | 10    |
      | 30    |
    And score "scoped" by summing rules on the field "note"
      | points | needles |
      | 7 | "red" |
      | 11 | "blue" |
    When any other answer goes to "STOP"
    Then label "LOW" when "scoped" is below 10
    And label "HIGH" otherwise
    Then the primary question is "scoped"
"#;
        let f55field = laya_workflow::bdd::parse(src55, "<p255-field>").expect("parse p255 field form");
        let s55field = laya_workflow::bdd::compile_decision(&f55field, &json!({}))
            .expect("compile p255 field form");
        let h55field = &s55field["nodes"][0]["questions"]["scoped"]["heuristic"];
        h.eq("p255: the field form keeps its named match space", h55field["field"].clone(), json!("note"));
        h.check("p255: an omitted cap stays omitted", h55field.get("score_max").is_none());
        let field55 = be55
            .decide(&json!({"text":"red blue","note":"blue"}), &s55field["nodes"][0]["questions"])
            .expect("scoped additive score");
        h.eq(
            "p255: a declared score_sum field does not cross-fire into the envelope",
            field55.answer_value("scoped").unwrap().as_f64().unwrap(),
            11.0,
        );

        let back55 = laya_workflow::bdd::import_spec(&s55).expect("import the p255 spec");
        h.check(
            "p255: the additive step round-trips through import",
            back55.contains("score \"aml_risk\" by summing rules on the text capped at 100"),
        );
        let f55b = laya_workflow::bdd::parse(&back55, "<p255b>").expect("parse the imported p255 feature");
        let s55b = laya_workflow::bdd::compile_decision(&f55b, &json!({}))
            .expect("recompile the imported p255 feature");
        h.check(
            "p255: feature -> spec -> feature -> spec is a fixed point on the graph",
            s55["nodes"] == s55b["nodes"],
        );
    }
}

// keep `Decide` referenced for the trait-object import path
#[allow(dead_code)]
fn _assert_trait_object() {
    let _: Option<&dyn Decide> = None;
}
